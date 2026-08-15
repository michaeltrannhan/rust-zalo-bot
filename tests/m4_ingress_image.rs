//! M4 ingress integration tests for image receipt acceptance and review commands.

#![allow(clippy::await_holding_lock)]

mod common;

use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::conversation::{decide_image, image_received_text};
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    IngressOutcome, IngressRequest, IngressSource, process_image, process_text_command,
    store_with_receipt,
};
use zl_expense::receipt::{InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle};

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m4_ingress_{}", Uuid::new_v4().simple());
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

fn receipt_store(pool: PgPool) -> zl_expense::ingress::IngressStore {
    let lifecycle = ReceiptLifecycle::new(
        pool.clone(),
        InMemoryObjectStore::new(),
        ReceiptConfig {
            original_receipt_days: 7,
            review_expiry_hours: 72,
        },
    );
    store_with_receipt(pool, lifecycle)
}

async fn seed_sender(pool: &PgPool, sender: &str, lifecycle_state: &str) -> Uuid {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (id, lifecycle_state, consent_version, consented_at)
        VALUES ($1, $2, 'v1', CASE WHEN $2 = 'active' THEN NOW() ELSE NULL END)
        "#,
    )
    .bind(account_id)
    .bind(lifecycle_state)
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

fn image_request(event_id: &str, sender: &str, allowed: bool) -> (IngressRequest, String) {
    let media_url = "https://example.test/receipt.png".to_string();
    let request = IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: allowed,
        user_text: String::new(),
        observed_at: Utc::now(),
    };
    (request, media_url)
}

fn text_request(
    event_id: &str,
    sender: &str,
    text: &str,
    now: chrono::DateTime<Utc>,
) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: text.to_string(),
        observed_at: now,
    }
}

async fn arm_receipt_review(pool: &PgPool, account_id: Uuid) -> (Uuid, i32) {
    let lifecycle = ReceiptLifecycle::new(
        pool.clone(),
        InMemoryObjectStore::new(),
        ReceiptConfig::default(),
    );
    let submission_id = Uuid::new_v4();
    let bytes = common::corpus_png(0);
    common::drive_to_review(&lifecycle, account_id, submission_id, &bytes).await;
    let draft = lifecycle
        .get_draft(account_id, submission_id)
        .await
        .expect("draft");
    let expires_at = Utc::now() + Duration::minutes(15);
    sqlx::query(
        r#"
        INSERT INTO conversation_states (
            account_id, pending_action_type, pending_payload_ref, expires_at, version
        )
        VALUES ($1, 'receipt_review', $2, $3, 1)
        ON CONFLICT (account_id) DO UPDATE
        SET pending_action_type = EXCLUDED.pending_action_type,
            pending_payload_ref = EXCLUDED.pending_payload_ref,
            expires_at = EXCLUDED.expires_at,
            version = conversation_states.version + 1
        "#,
    )
    .bind(account_id)
    .bind(submission_id.to_string())
    .bind(expires_at)
    .execute(pool)
    .await
    .expect("arm review");
    (submission_id, draft.version)
}

#[tokio::test]
async fn active_sender_image_creates_queued_submission_and_ingest_job() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("active_sender_image_creates_queued_submission")
    else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "sender-image", "active").await;
    let store = receipt_store(pool.clone());

    let (request, media_url) = image_request("evt-image-1", "sender-image", true);

    let outcome = process_image(&store, request, media_url)
        .await
        .expect("process image");
    assert!(matches!(outcome, IngressOutcome::Accepted { .. }));

    let submission_state: String = sqlx::query_scalar(
        "SELECT lifecycle_state FROM receipt_submissions WHERE account_id = (SELECT account_id FROM provider_identities WHERE provider_sender_id = $1)",
    )
    .bind("sender-image")
    .fetch_one(&pool)
    .await
    .expect("submission state");
    assert_eq!(submission_state, "queued");

    let ingest_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE job_type = 'receipt.ingest'")
            .fetch_one(&pool)
            .await
            .expect("ingest jobs");
    assert_eq!(ingest_jobs, 1);

    let event_kind: String =
        sqlx::query_scalar("SELECT kind FROM inbound_events WHERE provider_event_id = $1")
            .bind("evt-image-1")
            .fetch_one(&pool)
            .await
            .expect("event kind");
    assert_eq!(event_kind, "image_received");
}

#[tokio::test]
async fn duplicate_image_event_short_circuits_without_second_job() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("duplicate_image_event_short_circuits") else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "sender-dup-image", "active").await;
    let store = receipt_store(pool.clone());
    let (request, media_url) = image_request("evt-image-dup", "sender-dup-image", true);

    assert!(matches!(
        process_image(&store, request.clone(), media_url.clone())
            .await
            .expect("first"),
        IngressOutcome::Accepted { .. }
    ));
    assert!(matches!(
        process_image(&store, request, media_url)
            .await
            .expect("duplicate"),
        IngressOutcome::Duplicate { .. }
    ));

    let ingest_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE job_type = 'receipt.ingest'")
            .fetch_one(&pool)
            .await
            .expect("ingest jobs");
    assert_eq!(ingest_jobs, 1);
}

#[tokio::test]
async fn consent_pending_image_returns_consent_card_without_submission() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("consent_pending_image_gate") else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "sender-consent-image", "pending_consent").await;
    let store = receipt_store(pool.clone());

    let (request, media_url) = image_request("evt-image-consent", "sender-consent-image", true);

    process_image(&store, request, media_url)
        .await
        .expect("process image");

    let submission_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receipt_submissions")
        .fetch_one(&pool)
        .await
        .expect("submissions");
    assert_eq!(submission_count, 0);

    let body: String = sqlx::query_scalar(
        r#"
        SELECT body FROM outbound_messages o
        JOIN inbound_events i ON i.id = o.inbound_event_id
        WHERE i.provider_event_id = $1
        "#,
    )
    .bind("evt-image-consent")
    .fetch_one(&pool)
    .await
    .expect("reply");
    assert!(body.contains("Trả lời ok"));
}

#[tokio::test]
async fn non_allowlisted_image_is_rejected_without_submission() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("non_allowlisted_image_gate") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = receipt_store(pool.clone());

    let (request, media_url) = image_request("evt-image-denied", "sender-denied-image", false);

    process_image(&store, request, media_url)
        .await
        .expect("process image");

    let submission_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receipt_submissions")
        .fetch_one(&pool)
        .await
        .expect("submissions");
    assert_eq!(submission_count, 0);

    let body: String = sqlx::query_scalar(
        r#"
        SELECT body FROM outbound_messages o
        JOIN inbound_events i ON i.id = o.inbound_event_id
        WHERE i.provider_event_id = $1
        "#,
    )
    .bind("evt-image-denied")
    .fetch_one(&pool)
    .await
    .expect("reply");
    assert!(body.contains("chưa được cấp quyền"));
}

#[tokio::test]
async fn receipt_review_confirm_creates_confirmed_expense_for_today() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("receipt_review_confirm") else {
        return;
    };

    let pool = fresh_pool().await;
    let account_id = seed_sender(&pool, "sender-review-confirm", "active").await;
    let (submission_id, _) = arm_receipt_review(&pool, account_id).await;
    let store = receipt_store(pool.clone());
    let now = Utc::now();

    // The fake extractor emits a fixed corpus date; pin the draft to `now` so
    // the confirmed expense falls inside the local today window.
    sqlx::query("UPDATE expense_drafts SET occurred_at = $2 WHERE submission_id = $1")
        .bind(submission_id)
        .bind(now)
        .execute(&pool)
        .await
        .expect("pin draft occurred_at");

    process_text_command(
        &store,
        text_request("evt-review-confirm", "sender-review-confirm", "ok", now),
    )
    .await
    .expect("confirm");

    let confirmed_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM expenses
        WHERE account_id = $1 AND state = 'confirmed' AND source = 'receipt'
        "#,
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("confirmed");
    assert_eq!(confirmed_count, 1);

    let today_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM expenses
        WHERE account_id = $1
          AND state = 'confirmed'
          AND (occurred_at AT TIME ZONE 'Asia/Ho_Chi_Minh')::date =
              ($2::timestamptz AT TIME ZONE 'Asia/Ho_Chi_Minh')::date
        "#,
    )
    .bind(account_id)
    .bind(now)
    .fetch_one(&pool)
    .await
    .expect("today");
    assert_eq!(today_count, 1);
}

#[tokio::test]
async fn receipt_review_edit_amount_then_confirm_uses_edited_amount() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("receipt_review_edit_confirm") else {
        return;
    };

    let pool = fresh_pool().await;
    let account_id = seed_sender(&pool, "sender-review-edit", "active").await;
    arm_receipt_review(&pool, account_id).await;
    let store = receipt_store(pool.clone());
    let now = Utc::now();

    process_text_command(
        &store,
        text_request("evt-review-edit", "sender-review-edit", "sua 99k", now),
    )
    .await
    .expect("edit");

    process_text_command(
        &store,
        text_request(
            "evt-review-edit-confirm",
            "sender-review-edit",
            "ok",
            now + Duration::seconds(1),
        ),
    )
    .await
    .expect("confirm");

    let amount_minor: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM expenses WHERE account_id = $1 AND state = 'confirmed'",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("amount");
    assert_eq!(amount_minor, 99_000);
}

#[tokio::test]
async fn receipt_review_reject_leaves_no_confirmed_expense() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("receipt_review_reject") else {
        return;
    };

    let pool = fresh_pool().await;
    let account_id = seed_sender(&pool, "sender-review-reject", "active").await;
    arm_receipt_review(&pool, account_id).await;
    let store = receipt_store(pool.clone());

    process_text_command(
        &store,
        text_request(
            "evt-review-reject",
            "sender-review-reject",
            "no",
            Utc::now(),
        ),
    )
    .await
    .expect("reject");

    let confirmed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM expenses WHERE account_id = $1 AND state = 'confirmed'",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("confirmed");
    assert_eq!(confirmed_count, 0);

    let submission_state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("submission state");
    assert_eq!(submission_state, "rejected");
}

#[test]
fn decide_image_active_returns_acknowledgement_copy() {
    use zl_expense::conversation::{AccountContext, LifecycleState};
    let ctx = AccountContext {
        next_expense_id: Uuid::new_v4(),
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        original_receipt_retention_days: 7,
        pending: None,
        today_summary: None,
        recent_lines: vec![],
    };
    let outcome = decide_image(&ctx, Utc::now());
    assert_eq!(outcome.replies[0].body, image_received_text());
    assert_eq!(outcome.commands.len(), 1);
}
