//! Milestone 6 quota and feature kill-switch integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::conversation::{daily_receipt_quota_text, decide_image};
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    IngressOutcome, IngressPolicy, IngressRequest, IngressSource, IngressStore, process_image,
    process_text_command, store_with_receipt_and_policy,
};
use zl_expense::receipt::{
    FakeExtractor, InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle, ReceiptState,
};

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m6_quotas_{}", Uuid::new_v4().simple());
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

async fn seed_sender(pool: &PgPool, sender: &str, lifecycle: &str) -> Uuid {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (id, lifecycle_state, consent_version, consented_at)
        VALUES ($1, $2, 'consent-v1', NOW())
        "#,
    )
    .bind(account_id)
    .bind(lifecycle)
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

fn image_request(event_id: &str, sender: &str) -> (IngressRequest, String) {
    let media_url = "https://example.test/receipt.png".to_string();
    let request = IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: String::new(),
        observed_at: Utc::now(),
    };
    (request, media_url)
}

fn receipt_lifecycle(pool: PgPool, config: ReceiptConfig) -> ReceiptLifecycle {
    ReceiptLifecycle::with_extractor(
        pool,
        InMemoryObjectStore::new(),
        std::sync::Arc::new(FakeExtractor),
        config,
    )
}

fn receipt_store(pool: PgPool, receipt: ReceiptLifecycle, policy: IngressPolicy) -> IngressStore {
    store_with_receipt_and_policy(pool, receipt, policy)
}

#[tokio::test]
async fn twentieth_daily_receipt_accepted_twenty_first_rejected_without_ingest_job() {
    let _guard = common::integration_lock();
    let Some(_) =
        common::skip_without_database("twentieth_daily_receipt_accepted_twenty_first_rejected")
    else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "quota-sender", "active").await;
    let policy = IngressPolicy {
        per_user_daily_receipts: 20,
        outbound_enabled: true,
        zalo_monthly_messages: 3000,
        ..IngressPolicy::default()
    };
    let receipt = receipt_lifecycle(pool.clone(), ReceiptConfig::default());
    let store = receipt_store(pool.clone(), receipt, policy);

    for index in 0..20 {
        let (request, media_url) = image_request(&format!("evt-quota-{index}"), "quota-sender");
        let outcome = process_image(&store, request, media_url)
            .await
            .expect("process image");
        assert!(matches!(outcome, IngressOutcome::Accepted { .. }));
    }

    let (request, media_url) = image_request("evt-quota-21", "quota-sender");
    let outcome = process_image(&store, request, media_url)
        .await
        .expect("process image");
    assert!(matches!(outcome, IngressOutcome::Accepted { .. }));

    let body: String = sqlx::query_scalar(
        r#"
        SELECT o.body
        FROM outbound_messages o
        JOIN inbound_events ie ON ie.id = o.inbound_event_id
        WHERE ie.provider_event_id = 'evt-quota-21'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("outbound body");
    assert_eq!(
        body,
        daily_receipt_quota_text(zl_expense::conversation::Locale::Vi)
    );

    let submission_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM receipt_submissions rs
        JOIN provider_identities pi ON pi.account_id = rs.account_id
        WHERE pi.provider_sender_id = 'quota-sender'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("submission count");
    assert_eq!(submission_count, 20);

    let ingest_jobs: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM jobs
        WHERE job_type = 'receipt.ingest'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("ingest jobs");
    assert_eq!(ingest_jobs, 20);
}

#[tokio::test]
async fn extraction_disabled_fails_with_kill_switch_without_review() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("extraction_disabled_fails_with_kill_switch")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let account_id = seed_sender(&pool, "kill-extract", "active").await;
    let receipt = receipt_lifecycle(
        pool.clone(),
        ReceiptConfig {
            original_receipt_days: 7,
            review_expiry_hours: 72,
            extraction_enabled: false,
            monthly_extraction_pages: 80,
        },
    );

    let submission_id = Uuid::new_v4();
    common::accept_and_ingest(&receipt, account_id, submission_id, &common::corpus_png(0)).await;

    let error = receipt
        .extract(account_id, submission_id)
        .await
        .expect_err("extract should fail");
    assert_eq!(error.class, zl_expense::error::ErrorClass::KillSwitch);

    let state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("submission state");
    assert_eq!(state, ReceiptState::FailedPermanent.as_str());

    let review_outbound: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM outbound_messages WHERE idempotency_key = $1",
    )
    .bind(format!("receipt-review:{submission_id}"))
    .fetch_one(&pool)
    .await
    .expect("review outbound");
    assert_eq!(review_outbound, 0);
}

#[tokio::test]
async fn outbound_disabled_inserts_suppressed_row_without_deliver_job() {
    let _guard = common::integration_lock();
    let Some(_) =
        common::skip_without_database("outbound_disabled_inserts_suppressed_row_without_deliver")
    else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "outbound-off", "active").await;
    let policy = IngressPolicy {
        per_user_daily_receipts: 20,
        outbound_enabled: false,
        zalo_monthly_messages: 3000,
        ..IngressPolicy::default()
    };
    let store = IngressStore::with_policy(pool.clone(), policy);

    let request = IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: "evt-outbound-off".to_string(),
        provider_sender_id: "outbound-off".to_string(),
        provider_chat_id: "chat-outbound-off".to_string(),
        sender_allowed: true,
        user_text: "/help".to_string(),
        observed_at: Utc::now(),
    };
    process_text_command(&store, request)
        .await
        .expect("process text");

    let outbound_state: String = sqlx::query_scalar(
        r#"
        SELECT o.state
        FROM outbound_messages o
        JOIN inbound_events ie ON ie.id = o.inbound_event_id
        WHERE ie.provider_event_id = 'evt-outbound-off'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("outbound state");
    assert_eq!(outbound_state, "suppressed");

    let deliver_jobs: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM jobs
        WHERE job_type = 'outbound.deliver'
          AND dedupe_key = 'outbound.deliver:reply:zalo:test-bot:evt-outbound-off'
        "#,
    )
    .fetch_one(&pool)
    .await
    .expect("deliver jobs");
    assert_eq!(deliver_jobs, 0);
}

#[test]
fn extraction_kill_switch_copy_is_stable() {
    use zl_expense::conversation::extraction_kill_switch_text;
    assert_eq!(
        extraction_kill_switch_text(zl_expense::conversation::Locale::Vi),
        "Trích xuất hóa đơn đang tạm tắt."
    );
}

#[test]
fn decide_image_quota_reply_matches_ingress_copy() {
    use zl_expense::conversation::{AccountContext, LifecycleState};
    let ctx = AccountContext {
        next_expense_id: Uuid::new_v4(),
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        locale: "vi-VN".to_string(),
        original_receipt_retention_days: 7,
        remaining_daily_receipts: 0,
        confirmed_expense_count: 0,
        pending: None,
        today_summary: None,
        week_summary: None,
        month_summary: None,
        schedules: vec![],
        recent_lines: vec![],
    };
    let outcome = decide_image(&ctx, Utc::now());
    assert_eq!(
        outcome.replies[0].body,
        daily_receipt_quota_text(zl_expense::conversation::Locale::Vi)
    );
    assert!(outcome.commands.is_empty());
}
