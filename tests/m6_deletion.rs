//! Milestone 6 account deletion and export integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::account::{
    AccountDeletePayload, JOB_TYPE_ACCOUNT_DELETE, JOB_TYPE_ACCOUNT_EXPORT, execute_account_delete,
};
use zl_expense::db::MIGRATOR;
use zl_expense::error::ErrorClass;
use zl_expense::ingress::{
    IngressPolicy, IngressRequest, IngressSource, IngressStore, process_text_command,
};
use zl_expense::receipt::{
    InMemoryObjectStore, ReceiptError, ReceiptObjectStore, account_serialization_key, object_key,
};
use zl_expense::work::{EnqueueRequest, WorkStore};

struct FailingDeleteStore {
    inner: Arc<InMemoryObjectStore>,
    fail: AtomicBool,
}

impl FailingDeleteStore {
    fn new(inner: Arc<InMemoryObjectStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fail: AtomicBool::new(true),
        })
    }
}

impl ReceiptObjectStore for FailingDeleteStore {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ReceiptError> {
        self.inner.put(key, bytes)
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ReceiptError> {
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<(), ReceiptError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ReceiptError::dependency("simulated object delete failure"));
        }
        self.inner.delete(key)
    }
}

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m6_deletion_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .expect("create isolated schema");
    admin_pool.close().await;

    let search_path = schema;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .after_connect(move |connection, _metadata| {
            let statement = format!("SET search_path TO {search_path}");
            Box::pin(async move {
                sqlx::query(&statement).execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .expect("connect test database");
    MIGRATOR.run(&pool).await.expect("run migrations");
    pool
}

async fn seed_account(pool: &PgPool, sender: &str) -> Uuid {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (id, lifecycle_state, consent_version, consented_at)
        VALUES ($1, 'active', 'consent-v1', NOW())
        "#,
    )
    .bind(account_id)
    .execute(pool)
    .await
    .expect("seed account");
    sqlx::query(
        r#"
        INSERT INTO provider_identities (id, account_id, provider_scope, provider_sender_id)
        VALUES ($1, $2, 'zalo:test-bot', $3)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .bind(sender)
    .execute(pool)
    .await
    .expect("seed identity");
    account_id
}

fn text_request(sender: &str, event_id: &str, text: &str) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: text.to_string(),
        observed_at: Utc::now(),
    }
}

async fn seed_expense_and_asset(
    pool: &PgPool,
    objects: &dyn ReceiptObjectStore,
    account_id: Uuid,
) -> String {
    let expense_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO expenses (
            id, account_id, amount_minor, currency, occurred_at, description, source, state
        )
        VALUES ($1, $2, 45000, 'VND', NOW(), 'cafe bi mat', 'manual', 'confirmed')
        "#,
    )
    .bind(expense_id)
    .bind(account_id)
    .execute(pool)
    .await
    .expect("seed expense");

    let submission_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO receipt_submissions (id, account_id, inbound_event_id, lifecycle_state)
        VALUES ($1, $2, NULL, 'stored')
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .execute(pool)
    .await
    .expect("seed submission");

    let sha = "a".repeat(64);
    let key = object_key(account_id, submission_id, &sha);
    objects.put(&key, b"receipt-bytes").expect("put object");
    sqlx::query(
        r#"
        INSERT INTO receipt_assets (
            id, submission_id, account_id, object_key, content_sha256, mime_type,
            size_bytes, width_px, height_px, pixel_count, retention_deadline
        )
        VALUES (
            $1, $2, $3, $4, $5, 'image/png', 8, 8, 8, 64, NOW() + INTERVAL '1 day'
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(submission_id)
    .bind(account_id)
    .bind(&key)
    .bind(&sha)
    .execute(pool)
    .await
    .expect("seed asset");
    key
}

#[tokio::test]
async fn two_step_delete_purges_content_keeps_identity_and_cancels_queued_work() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("del-{}", Uuid::new_v4());
    let account_id = seed_account(&pool, &sender).await;
    let objects = InMemoryObjectStore::new();
    let object_key = seed_expense_and_asset(&pool, objects.as_ref(), account_id).await;

    let ingest_job_id = Uuid::new_v4();
    WorkStore::new(pool.clone())
        .enqueue(EnqueueRequest {
            id: ingest_job_id,
            job_type: "receipt.ingest".to_string(),
            payload: serde_json::json!({
                "schema_version": 1,
                "receipt_submission_id": Uuid::new_v4(),
            }),
            dedupe_key: format!("receipt.ingest:{}", Uuid::new_v4()),
            serialization_key: Some(account_serialization_key(account_id)),
            priority: 0,
            run_at: Utc::now(),
            max_attempts: 10,
        })
        .await
        .expect("enqueue ingest");

    let store = IngressStore::with_policy(pool.clone(), IngressPolicy::default());
    process_text_command(&store, text_request(&sender, "del-1", "/delete"))
        .await
        .expect("arm delete");
    process_text_command(&store, text_request(&sender, "del-2", "ok"))
        .await
        .expect("confirm delete");

    let lifecycle: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("lifecycle");
    assert_eq!(lifecycle, "deleting");

    let ingest_state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
        .bind(ingest_job_id)
        .fetch_one(&pool)
        .await
        .expect("ingest state");
    assert_eq!(ingest_state, "cancelled");

    let delete_payload: (Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT id, payload FROM jobs WHERE job_type = $1 AND serialization_key = $2",
    )
    .bind(JOB_TYPE_ACCOUNT_DELETE)
    .bind(account_serialization_key(account_id))
    .fetch_one(&pool)
    .await
    .expect("delete job");
    let payload: AccountDeletePayload = serde_json::from_value(delete_payload.1).expect("payload");

    execute_account_delete(&pool, objects.as_ref(), &payload)
        .await
        .expect("purge");

    let lifecycle: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("deleted");
    assert_eq!(lifecycle, "deleted");

    let expenses: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM expenses WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("expenses");
    assert_eq!(expenses, 0);

    let submissions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM receipt_submissions WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("submissions");
    assert_eq!(submissions, 0);

    assert!(objects.get(&object_key).expect("get").is_none());

    let identities: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM provider_identities WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("identities");
    assert_eq!(identities, 1);

    process_text_command(&store, text_request(&sender, "del-3", "/today"))
        .await
        .expect("post-delete ingress");
    let identities_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM provider_identities WHERE provider_sender_id = $1",
    )
    .bind(&sender)
    .fetch_one(&pool)
    .await
    .expect("identities after");
    assert_eq!(identities_after, 1);
    let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM accounts")
        .fetch_one(&pool)
        .await
        .expect("accounts");
    assert_eq!(accounts, 1);

    let outbound: String = sqlx::query_scalar(
        r#"
        SELECT body FROM outbound_messages
        WHERE provider_target = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(format!("chat-{sender}"))
    .fetch_one(&pool)
    .await
    .expect("latest reply");
    assert!(outbound.contains("tạm dừng") || outbound.contains("xóa"));
    assert!(!outbound.contains("cafe bi mat"));
}

#[tokio::test]
async fn object_delete_failure_leaves_database_intact() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let account_id = seed_account(&pool, &format!("fail-{}", Uuid::new_v4())).await;
    let inner = InMemoryObjectStore::new();
    let objects = FailingDeleteStore::new(Arc::clone(&inner));
    seed_expense_and_asset(&pool, objects.as_ref(), account_id).await;

    sqlx::query(
        r#"
        INSERT INTO deletion_requests (id, account_id, state, expense_count)
        VALUES ($1, $2, 'requested', 1)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("deletion request");
    sqlx::query("UPDATE accounts SET lifecycle_state = 'deleting' WHERE id = $1")
        .bind(account_id)
        .execute(&pool)
        .await
        .expect("mark deleting");

    let request_id: Uuid =
        sqlx::query_scalar("SELECT id FROM deletion_requests WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("request id");
    let err = execute_account_delete(
        &pool,
        objects.as_ref(),
        &AccountDeletePayload {
            schema_version: 1,
            account_id,
            deletion_request_id: request_id,
        },
    )
    .await
    .expect_err("delete must fail");
    assert_eq!(err, ErrorClass::Dependency);

    let expenses: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM expenses WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("expenses");
    assert_eq!(expenses, 1);
}

#[tokio::test]
async fn export_enqueues_job_and_chat_reply_has_no_paths() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("exp-{}", Uuid::new_v4());
    let account_id = seed_account(&pool, &sender).await;
    sqlx::query(
        r#"
        INSERT INTO expenses (
            id, account_id, amount_minor, currency, occurred_at, description, source, state
        )
        VALUES ($1, $2, 12000, 'VND', NOW(), 'an sang', 'manual', 'confirmed')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("seed expense");

    let store = IngressStore::with_policy(pool.clone(), IngressPolicy::default());
    process_text_command(&store, text_request(&sender, "exp-1", "/export"))
        .await
        .expect("export");

    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM jobs WHERE job_type = $1")
            .bind(JOB_TYPE_ACCOUNT_EXPORT)
            .fetch_one(&pool)
            .await
            .expect("jobs");
    assert_eq!(job_count, 1);

    let body: String = sqlx::query_scalar(
        "SELECT body FROM outbound_messages WHERE account_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("reply");
    assert!(!body.contains("/var/"));
    assert!(!body.contains("s3://"));
    assert!(!body.contains(".json"));
    assert!(!body.contains(".csv"));
    assert!(!body.contains("exports/"));
    assert!(body.contains("quản trị"));
}
