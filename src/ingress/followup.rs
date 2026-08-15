//! Post-extraction receipt review follow-up: pending state and outbound card.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::conversation::{format_date_vn, format_minor, transaction_type_label};
use crate::receipt::ReceiptLifecycle;
use crate::work::{EnqueueRequest, WorkStore};

/// Enqueue the receipt review card and pending action in one idempotent transaction.
pub async fn enqueue_receipt_review_followup(
    pool: &PgPool,
    receipt: &ReceiptLifecycle,
    account_id: Uuid,
    submission_id: Uuid,
) -> Result<(), FollowupError> {
    let idempotency_key = review_idempotency_key(submission_id);
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM outbound_messages WHERE idempotency_key = $1")
            .bind(&idempotency_key)
            .fetch_optional(pool)
            .await
            .map_err(|_| FollowupError::dependency("outbound lookup failed"))?;
    if existing.is_some() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| FollowupError::dependency("begin transaction failed"))?;

    let context = load_followup_context(&mut tx, account_id, submission_id).await?;
    let expires_at = Utc::now() + Duration::hours(receipt.config().review_expiry_hours.max(1));
    upsert_receipt_review_pending(&mut tx, account_id, submission_id, expires_at).await?;

    let body = review_card_body(&context);
    let outbound_id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar(
        r#"
        INSERT INTO outbound_messages (
            id,
            account_id,
            inbound_event_id,
            idempotency_key,
            provider_scope,
            provider_target,
            body,
            state
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued')
        ON CONFLICT (idempotency_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(outbound_id)
    .bind(account_id)
    .bind(context.inbound_event_id)
    .bind(&idempotency_key)
    .bind(&context.provider_scope)
    .bind(&context.provider_chat_id)
    .bind(&body)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| FollowupError::dependency("outbound insert failed"))?;

    let outbound_id = if let Some(outbound_id) = inserted {
        outbound_id
    } else {
        tx.rollback().await.ok();
        return Ok(());
    };

    WorkStore::enqueue_in_transaction(
        &mut tx,
        EnqueueRequest {
            id: Uuid::new_v4(),
            job_type: "outbound.deliver".to_string(),
            payload: json!({
                "schema_version": 1,
                "outbound_id": outbound_id,
            }),
            dedupe_key: format!("outbound.deliver:{idempotency_key}"),
            serialization_key: Some(format!("account:{account_id}")),
            priority: 0,
            run_at: Utc::now(),
            max_attempts: 10,
        },
    )
    .await
    .map_err(|_| FollowupError::dependency("followup job enqueue failed"))?;

    tx.commit()
        .await
        .map_err(|_| FollowupError::dependency("commit failed"))?;
    Ok(())
}

fn review_idempotency_key(submission_id: Uuid) -> String {
    format!("receipt-review:{submission_id}")
}

fn review_card_body(context: &FollowupContext) -> String {
    let amount = format_minor(context.amount_minor, &context.currency);
    let date = format_date_vn(context.occurred_at, &context.timezone);
    let type_label = transaction_type_label(&context.transaction_type);
    format!(
        "Tôi đọc được:\n\nCửa hàng: {merchant}\nSố tiền: {amount}\nNgày: {date}\nLoại: {type_label}\nDanh mục: {category}\n\nTrả lời: ok / y để lưu · edit / fix để sửa số tiền · no / n để hủy",
        merchant = context.merchant,
        category = context.category_display,
    )
}

struct FollowupContext {
    inbound_event_id: Option<Uuid>,
    provider_scope: String,
    provider_chat_id: String,
    timezone: String,
    amount_minor: i64,
    currency: String,
    merchant: String,
    category_display: String,
    transaction_type: String,
    occurred_at: DateTime<Utc>,
}

async fn load_followup_context(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    submission_id: Uuid,
) -> Result<FollowupContext, FollowupError> {
    let routing: (Option<Uuid>, String, String, String) = sqlx::query_as(
        r#"
        SELECT rs.inbound_event_id, ie.provider_scope, ie.provider_chat_id, a.timezone
        FROM receipt_submissions rs
        JOIN inbound_events ie ON ie.id = rs.inbound_event_id
        JOIN accounts a ON a.id = rs.account_id
        WHERE rs.id = $1 AND rs.account_id = $2
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| FollowupError::dependency("submission routing lookup failed"))?
    .ok_or_else(|| FollowupError::not_found("submission routing not found"))?;

    let (inbound_event_id, provider_scope, provider_chat_id, timezone) = routing;

    let (amount_minor, currency, merchant, category_display, transaction_type, occurred_at): (
        i64,
        String,
        String,
        String,
        String,
        DateTime<Utc>,
    ) = sqlx::query_as(
        r#"
        SELECT d.amount_minor, d.currency, d.merchant, c.display_name_vi, d.transaction_type, d.occurred_at
        FROM expense_drafts d
        JOIN categories c ON c.key = d.category_key
        WHERE d.submission_id = $1 AND d.account_id = $2
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| FollowupError::dependency("draft lookup failed"))?
    .ok_or_else(|| FollowupError::not_found("expense draft not found"))?;

    Ok(FollowupContext {
        inbound_event_id,
        provider_scope,
        provider_chat_id,
        timezone,
        amount_minor,
        currency,
        merchant,
        category_display,
        transaction_type,
        occurred_at,
    })
}

async fn upsert_receipt_review_pending(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    submission_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<(), FollowupError> {
    let payload_ref = submission_id.to_string();
    let inserted = sqlx::query(
        r#"
        INSERT INTO conversation_states (
            account_id, pending_action_type, pending_payload_ref, expires_at, version
        )
        VALUES ($1, 'receipt_review', $2, $3, 1)
        ON CONFLICT (account_id) DO NOTHING
        "#,
    )
    .bind(account_id)
    .bind(&payload_ref)
    .bind(expires_at)
    .execute(&mut **tx)
    .await
    .map_err(|_| FollowupError::dependency("pending insert failed"))?;

    if inserted.rows_affected() == 1 {
        return Ok(());
    }

    let current_version: i32 =
        sqlx::query_scalar("SELECT version FROM conversation_states WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|_| FollowupError::dependency("pending version lookup failed"))?;

    let updated = sqlx::query(
        r#"
        UPDATE conversation_states
        SET pending_action_type = 'receipt_review',
            pending_payload_ref = $2,
            expires_at = $3,
            version = version + 1,
            updated_at = NOW()
        WHERE account_id = $1 AND version = $4
        "#,
    )
    .bind(account_id)
    .bind(&payload_ref)
    .bind(expires_at)
    .bind(current_version)
    .execute(&mut **tx)
    .await
    .map_err(|_| FollowupError::dependency("pending update failed"))?;

    if updated.rows_affected() == 0 {
        return Err(FollowupError::conflict("pending state version conflict"));
    }
    Ok(())
}

/// Follow-up persistence failure.
#[derive(Debug)]
pub struct FollowupError {
    pub message: String,
}

impl FollowupError {
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn dependency(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
