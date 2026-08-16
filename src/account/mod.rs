//! Account deletion and export jobs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ErrorClass;
use crate::receipt::ReceiptObjectStore;
use crate::work::{EnqueueRequest, WorkStore};

pub const JOB_TYPE_ACCOUNT_DELETE: &str = "account.delete";
pub const JOB_TYPE_ACCOUNT_EXPORT: &str = "account.export";
pub const ACCOUNT_JOB_PAYLOAD_VERSION: i32 = 1;

/// Versioned `account.delete` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeletePayload {
    pub schema_version: i32,
    pub account_id: Uuid,
    pub deletion_request_id: Uuid,
}

/// Versioned `account.export` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountExportPayload {
    pub schema_version: i32,
    pub account_id: Uuid,
    pub export_id: Uuid,
    pub provider_scope: String,
    pub provider_chat_id: String,
}

pub fn delete_dedupe_key(account_id: Uuid) -> String {
    format!("account.delete:{account_id}")
}

pub fn export_dedupe_key(export_id: Uuid) -> String {
    format!("account.export:{export_id}")
}

pub fn export_object_key(account_id: Uuid, export_id: Uuid, format: &str) -> String {
    format!("exports/{account_id}/{export_id}.{format}")
}

/// Cancel queued account-scoped jobs, then enqueue the delete worker.
pub async fn enqueue_delete_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    deletion_request_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ErrorClass> {
    let serialization_key = crate::receipt::account_serialization_key(account_id);
    WorkStore::cancel_queued_by_serialization_key_in_transaction(tx, &serialization_key)
        .await
        .map_err(|_| ErrorClass::Dependency)?;
    let payload = AccountDeletePayload {
        schema_version: ACCOUNT_JOB_PAYLOAD_VERSION,
        account_id,
        deletion_request_id,
    };
    WorkStore::enqueue_in_transaction(
        tx,
        EnqueueRequest {
            id: Uuid::new_v4(),
            job_type: JOB_TYPE_ACCOUNT_DELETE.to_string(),
            payload: serde_json::to_value(&payload).map_err(|_| ErrorClass::Internal)?,
            dedupe_key: delete_dedupe_key(account_id),
            serialization_key: Some(serialization_key),
            priority: 10,
            run_at: now,
            max_attempts: 10,
        },
    )
    .await
    .map_err(|_| ErrorClass::Dependency)?;
    Ok(())
}

/// Enqueue a one-shot export job for an active account.
pub async fn enqueue_export_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    export_id: Uuid,
    provider_scope: &str,
    provider_chat_id: &str,
    now: DateTime<Utc>,
) -> Result<(), ErrorClass> {
    let payload = AccountExportPayload {
        schema_version: ACCOUNT_JOB_PAYLOAD_VERSION,
        account_id,
        export_id,
        provider_scope: provider_scope.to_string(),
        provider_chat_id: provider_chat_id.to_string(),
    };
    WorkStore::enqueue_in_transaction(
        tx,
        EnqueueRequest {
            id: Uuid::new_v4(),
            job_type: JOB_TYPE_ACCOUNT_EXPORT.to_string(),
            payload: serde_json::to_value(&payload).map_err(|_| ErrorClass::Internal)?,
            dedupe_key: export_dedupe_key(export_id),
            serialization_key: Some(crate::receipt::account_serialization_key(account_id)),
            priority: 0,
            run_at: now,
            max_attempts: 10,
        },
    )
    .await
    .map_err(|_| ErrorClass::Dependency)?;
    Ok(())
}

/// Delete object-store bytes first, then purge domain rows, keeping the account tombstone.
pub async fn execute_account_delete(
    pool: &PgPool,
    objects: &dyn ReceiptObjectStore,
    payload: &AccountDeletePayload,
) -> Result<(), ErrorClass> {
    if payload.schema_version != ACCOUNT_JOB_PAYLOAD_VERSION {
        return Err(ErrorClass::Validation);
    }

    let lifecycle: Option<String> =
        sqlx::query_scalar("SELECT lifecycle_state FROM accounts WHERE id = $1")
            .bind(payload.account_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ErrorClass::Dependency)?;
    let Some(lifecycle) = lifecycle else {
        return Ok(());
    };
    if lifecycle == "deleted" {
        return Ok(());
    }

    sqlx::query(
        r#"
        UPDATE deletion_requests
        SET state = 'running'
        WHERE id = $1 AND account_id = $2 AND state IN ('requested', 'running')
        "#,
    )
    .bind(payload.deletion_request_id)
    .bind(payload.account_id)
    .execute(pool)
    .await
    .map_err(|_| ErrorClass::Dependency)?;

    let object_keys = load_account_object_keys(pool, payload.account_id).await?;
    for key in &object_keys {
        objects.delete(key).map_err(|_| ErrorClass::Dependency)?;
    }

    let mut tx = pool.begin().await.map_err(|_| ErrorClass::Dependency)?;
    purge_account_content(&mut tx, payload.account_id).await?;
    sqlx::query(
        r#"
        UPDATE accounts
        SET lifecycle_state = 'deleted',
            consent_version = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(payload.account_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ErrorClass::Dependency)?;
    sqlx::query(
        r#"
        UPDATE deletion_requests
        SET state = 'completed',
            completed_at = NOW()
        WHERE id = $1 AND account_id = $2
        "#,
    )
    .bind(payload.deletion_request_id)
    .bind(payload.account_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| ErrorClass::Dependency)?;
    tx.commit().await.map_err(|_| ErrorClass::Dependency)?;
    Ok(())
}

/// Write JSON and CSV export objects, then persist artifact metadata.
pub async fn execute_account_export(
    pool: &PgPool,
    objects: &dyn ReceiptObjectStore,
    payload: &AccountExportPayload,
) -> Result<(), ErrorClass> {
    if payload.schema_version != ACCOUNT_JOB_PAYLOAD_VERSION {
        return Err(ErrorClass::Validation);
    }

    let lifecycle: Option<String> =
        sqlx::query_scalar("SELECT lifecycle_state FROM accounts WHERE id = $1")
            .bind(payload.account_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| ErrorClass::Dependency)?;
    if !matches!(lifecycle.as_deref(), Some("active")) {
        return Ok(());
    }

    let rows: Vec<(DateTime<Utc>, i64, String, String, String)> = sqlx::query_as(
        r#"
        SELECT occurred_at, amount_minor, currency, description, source
        FROM expenses
        WHERE account_id = $1 AND state = 'confirmed'
        ORDER BY occurred_at ASC, id ASC
        "#,
    )
    .bind(payload.account_id)
    .fetch_all(pool)
    .await
    .map_err(|_| ErrorClass::Dependency)?;

    let json_body = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "account_id": payload.account_id,
        "exported_at": Utc::now(),
        "expenses": rows.iter().map(|(occurred_at, amount_minor, currency, description, source)| {
            serde_json::json!({
                "occurred_at": occurred_at,
                "amount_minor": amount_minor,
                "currency": currency,
                "description": description,
                "source": source,
            })
        }).collect::<Vec<_>>(),
    }))
    .map_err(|_| ErrorClass::Internal)?;

    let mut csv_body = String::from("occurred_at,amount_minor,currency,description,source\n");
    for (occurred_at, amount_minor, currency, description, source) in &rows {
        csv_body.push_str(&format!(
            "{},{},{},{},{}\n",
            occurred_at.to_rfc3339(),
            amount_minor,
            csv_escape(currency),
            csv_escape(description),
            csv_escape(source)
        ));
    }
    let csv_bytes = csv_body.into_bytes();

    let json_key = export_object_key(payload.account_id, payload.export_id, "json");
    let csv_key = export_object_key(payload.account_id, payload.export_id, "csv");
    objects
        .put(&json_key, &json_body)
        .map_err(|_| ErrorClass::Dependency)?;
    objects
        .put(&csv_key, &csv_bytes)
        .map_err(|_| ErrorClass::Dependency)?;

    let mut tx = pool.begin().await.map_err(|_| ErrorClass::Dependency)?;
    insert_export_artifact(
        &mut tx,
        payload.account_id,
        payload.export_id,
        "json",
        &json_key,
        json_body.len(),
    )
    .await?;
    insert_export_artifact(
        &mut tx,
        payload.account_id,
        payload.export_id,
        "csv",
        &csv_key,
        csv_bytes.len(),
    )
    .await?;
    tx.commit().await.map_err(|_| ErrorClass::Dependency)?;
    Ok(())
}

async fn insert_export_artifact(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    export_id: Uuid,
    format: &str,
    object_key: &str,
    byte_size: usize,
) -> Result<(), ErrorClass> {
    sqlx::query(
        r#"
        INSERT INTO export_artifacts (id, account_id, format, object_key, byte_size)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .bind(format)
    .bind(object_key)
    .bind(i64::try_from(byte_size).unwrap_or(i64::MAX))
    .execute(&mut **tx)
    .await
    .map_err(|_| ErrorClass::Dependency)?;
    let _ = export_id;
    Ok(())
}

async fn load_account_object_keys(
    pool: &PgPool,
    account_id: Uuid,
) -> Result<Vec<String>, ErrorClass> {
    let receipt_keys: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT object_key
        FROM receipt_assets
        WHERE account_id = $1 AND deletion_state = 'active'
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .map_err(|_| ErrorClass::Dependency)?;
    let export_keys: Vec<String> =
        sqlx::query_scalar("SELECT object_key FROM export_artifacts WHERE account_id = $1")
            .bind(account_id)
            .fetch_all(pool)
            .await
            .map_err(|_| ErrorClass::Dependency)?;
    let mut keys = receipt_keys;
    keys.extend(export_keys);
    Ok(keys)
}

async fn purge_account_content(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
) -> Result<(), ErrorClass> {
    sqlx::query(
        r#"
        UPDATE receipt_submissions
        SET confirmed_expense_id = NULL,
            lifecycle_state = 'deleted',
            failure_error_class = NULL,
            updated_at = NOW()
        WHERE account_id = $1
        "#,
    )
    .bind(account_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("UPDATE expenses SET receipt_submission_id = NULL WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM expenses WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM receipt_submissions WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM conversation_states WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM summary_schedules WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM outbound_messages WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query(
        r#"
        DELETE FROM usage_counters
        WHERE scope = 'account' AND scope_id = $1
        "#,
    )
    .bind(account_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM export_artifacts WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM insight_snapshots WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query("DELETE FROM account_ai_preferences WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .map_err(|_| ErrorClass::Dependency)?;

    sqlx::query(
        r#"
        UPDATE inbound_events
        SET account_id = NULL,
            media_url = NULL
        WHERE account_id = $1
        "#,
    )
    .bind(account_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| ErrorClass::Dependency)?;

    Ok(())
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
