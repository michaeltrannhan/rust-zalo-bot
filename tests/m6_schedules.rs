//! Milestone 6 summary schedule integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    IngressPolicy, IngressRequest, IngressSource, IngressStore, process_text_command,
};
use zl_expense::runtime::scheduler_tick;
use zl_expense::schedule::JOB_TYPE_SCHEDULE_EMIT;

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m6_schedules_{}", Uuid::new_v4().simple());
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

async fn seed_account(pool: &PgPool, sender: &str, lifecycle: &str) -> Uuid {
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

#[tokio::test]
async fn sched_daily_persists_schedule_row() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("sched-{}", Uuid::new_v4());
    let account_id = seed_account(&pool, &sender, "active").await;
    let store = IngressStore::with_policy(pool.clone(), IngressPolicy::default());
    process_text_command(
        &store,
        text_request(&sender, "sched-set", "/sched daily 20:00"),
    )
    .await
    .expect("ingress");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM summary_schedules WHERE account_id = $1 AND enabled = TRUE",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn due_schedule_emits_one_job_and_duplicate_tick_is_absorbed() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("due-{}", Uuid::new_v4());
    let account_id = seed_account(&pool, &sender, "active").await;
    let schedule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO summary_schedules (
            id, account_id, frequency, delivery_minute,
            provider_scope, provider_chat_id, enabled, next_run_at
        )
        VALUES ($1, $2, 'daily', 0, 'zalo:test-bot', $3, TRUE, NOW() - INTERVAL '1 minute')
        "#,
    )
    .bind(schedule_id)
    .bind(account_id)
    .bind(format!("chat-{sender}"))
    .execute(&pool)
    .await
    .expect("seed schedule");

    sqlx::query(
        r#"
        INSERT INTO role_leases (role, owner, deadline)
        VALUES ('scheduler', 'test', NOW() + INTERVAL '1 minute')
        ON CONFLICT (role) DO UPDATE SET owner = EXCLUDED.owner, deadline = EXCLUDED.deadline
        "#,
    )
    .execute(&pool)
    .await
    .expect("lease");

    scheduler_tick(&pool).await.expect("tick");
    scheduler_tick(&pool).await.expect("tick");

    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM jobs WHERE job_type = $1")
            .bind(JOB_TYPE_SCHEDULE_EMIT)
            .fetch_one(&pool)
            .await
            .expect("jobs");
    assert_eq!(job_count, 1);
}

#[tokio::test]
async fn timezone_change_moves_next_run() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("tz-{}", Uuid::new_v4());
    let account_id = seed_account(&pool, &sender, "active").await;
    let schedule_id = Uuid::new_v4();
    let before = Utc::now() + Duration::hours(6);
    sqlx::query(
        r#"
        INSERT INTO summary_schedules (
            id, account_id, frequency, delivery_minute,
            provider_scope, provider_chat_id, enabled, next_run_at
        )
        VALUES ($1, $2, 'daily', 480, 'zalo:test-bot', $3, TRUE, $4)
        "#,
    )
    .bind(schedule_id)
    .bind(account_id)
    .bind(format!("chat-{sender}"))
    .bind(before)
    .execute(&pool)
    .await
    .expect("seed schedule");

    let store = IngressStore::with_policy(pool.clone(), IngressPolicy::default());
    process_text_command(&store, text_request(&sender, "tz-set", "/tz UTC"))
        .await
        .expect("ingress");

    let after: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT next_run_at FROM summary_schedules WHERE id = $1")
            .bind(schedule_id)
            .fetch_one(&pool)
            .await
            .expect("next");
    assert_ne!(after, before);
}

#[tokio::test]
async fn non_active_account_schedule_is_not_emitted() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let account_id =
        seed_account(&pool, &format!("suspended-{}", Uuid::new_v4()), "suspended").await;
    sqlx::query(
        r#"
        INSERT INTO summary_schedules (
            id, account_id, frequency, delivery_minute,
            provider_scope, provider_chat_id, enabled, next_run_at
        )
        VALUES ($1, $2, 'daily', 0, 'zalo:test-bot', 'chat', TRUE, NOW() - INTERVAL '1 minute')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("seed schedule");

    sqlx::query(
        r#"
        INSERT INTO role_leases (role, owner, deadline)
        VALUES ('scheduler', 'test', NOW() + INTERVAL '1 minute')
        ON CONFLICT (role) DO UPDATE SET owner = EXCLUDED.owner, deadline = EXCLUDED.deadline
        "#,
    )
    .execute(&pool)
    .await
    .expect("lease");

    scheduler_tick(&pool).await.expect("tick");

    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM jobs WHERE job_type = $1")
            .bind(JOB_TYPE_SCHEDULE_EMIT)
            .fetch_one(&pool)
            .await
            .expect("jobs");
    assert_eq!(job_count, 0);
}
