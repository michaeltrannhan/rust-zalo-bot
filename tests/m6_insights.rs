//! Milestone 6 deterministic insight snapshots and optional narratives.

#![allow(clippy::await_holding_lock)]

mod common;

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::account::{AccountDeletePayload, execute_account_delete};
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    IngressPolicy, IngressRequest, IngressSource, IngressStore, process_text_command,
};
use zl_expense::insight::{
    FakeNarrator, INSIGHT_NARRATE_PAYLOAD_VERSION, InsightNarratePayload, JOB_TYPE_INSIGHT_NARRATE,
    execute_insight_narrate,
};
use zl_expense::receipt::InMemoryObjectStore;

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m6_insights_{}", Uuid::new_v4().simple());
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

async fn seed_active_account(pool: &PgPool, sender: &str) -> Uuid {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (
            id, lifecycle_state, consent_version, consented_at, timezone, default_currency
        )
        VALUES ($1, 'active', 'consent-v1', NOW(), 'Asia/Ho_Chi_Minh', 'VND')
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

fn text_request(
    sender: &str,
    event_id: &str,
    text: &str,
    observed_at: chrono::DateTime<Utc>,
) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: text.to_string(),
        observed_at,
    }
}

async fn insert_expense(
    pool: &PgPool,
    account_id: Uuid,
    amount_minor: i64,
    description: &str,
    state: &str,
    occurred_at: chrono::DateTime<Utc>,
) {
    sqlx::query(
        r#"
        INSERT INTO expenses (
            id, account_id, amount_minor, currency, occurred_at, description, source, state
        )
        VALUES ($1, $2, $3, 'VND', $4, $5, 'manual', $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .bind(amount_minor)
    .bind(occurred_at)
    .bind(description)
    .bind(state)
    .execute(pool)
    .await
    .expect("insert expense");
}

fn forbidden_aggregate_keys(value: &Value, keys: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if matches!(key.as_str(), "description" | "expense_id" | "object_key") {
                    keys.insert(key.clone());
                }
                forbidden_aggregate_keys(child, keys);
            }
        }
        Value::Array(items) => {
            for item in items {
                forbidden_aggregate_keys(item, keys);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn week_totals_exclude_awaiting_confirmation_and_write_snapshot_without_narrative_job() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("ins-{}", Uuid::new_v4());
    let account_id = seed_active_account(&pool, &sender).await;
    let now = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
    insert_expense(
        &pool,
        account_id,
        100_000,
        "confirmed cafe",
        "confirmed",
        now,
    )
    .await;
    insert_expense(
        &pool,
        account_id,
        500_000,
        "pending lunch",
        "awaiting_confirmation",
        now,
    )
    .await;
    insert_expense(&pool, account_id, 50_000, "rejected snack", "rejected", now).await;

    let store = IngressStore::with_policy(
        pool.clone(),
        IngressPolicy {
            insights_llm_enabled: false,
            ..IngressPolicy::default()
        },
    );
    let outcome = process_text_command(&store, text_request(&sender, "evt-week", "/week", now))
        .await
        .expect("process /week");
    assert!(matches!(
        outcome,
        zl_expense::ingress::IngressOutcome::Accepted { .. }
    ));

    let reply: String = sqlx::query_scalar(
        r#"
        SELECT body
        FROM outbound_messages
        WHERE account_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("outbound reply");
    assert!(reply.contains("100.000"));
    assert!(!reply.contains("500.000"));

    let snapshot: (serde_json::Value, Option<String>) = sqlx::query_as(
        r#"
        SELECT aggregate, narrative_text
        FROM insight_snapshots
        WHERE account_id = $1 AND period_kind = 'week'
        "#,
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("snapshot row");
    assert_eq!(snapshot.1, None);
    assert_eq!(
        snapshot.0.get("total_minor").and_then(Value::as_i64),
        Some(100_000)
    );
    assert_eq!(snapshot.0.get("tx_count").and_then(Value::as_i64), Some(1));

    let mut forbidden = BTreeSet::new();
    forbidden_aggregate_keys(&snapshot.0, &mut forbidden);
    assert!(forbidden.is_empty(), "forbidden keys: {forbidden:?}");

    let narrate_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM jobs WHERE job_type = $1")
            .bind(JOB_TYPE_INSIGHT_NARRATE)
            .fetch_one(&pool)
            .await
            .expect("job count");
    assert_eq!(narrate_jobs, 0);
}

#[tokio::test]
async fn fake_narrator_stores_narrative_from_aggregate_only() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("ins-narr-{}", Uuid::new_v4());
    let account_id = seed_active_account(&pool, &sender).await;
    let now = Utc.with_ymd_and_hms(2026, 8, 16, 15, 0, 0).unwrap();
    insert_expense(
        &pool,
        account_id,
        75_000,
        "confirmed market",
        "confirmed",
        now,
    )
    .await;

    let store = IngressStore::with_policy(
        pool.clone(),
        IngressPolicy {
            insights_llm_enabled: true,
            ..IngressPolicy::default()
        },
    );
    process_text_command(&store, text_request(&sender, "evt-week-llm", "/week", now))
        .await
        .expect("process /week with llm");

    let snapshot_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM insight_snapshots WHERE account_id = $1 AND period_kind = 'week'",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("snapshot id");

    execute_insight_narrate(
        &pool,
        &FakeNarrator,
        &InsightNarratePayload {
            schema_version: INSIGHT_NARRATE_PAYLOAD_VERSION,
            account_id,
            snapshot_id,
            aggregate_digest: String::new(),
        },
        30,
    )
    .await
    .expect("execute narrate");

    let narrative: Option<String> =
        sqlx::query_scalar("SELECT narrative_text FROM insight_snapshots WHERE id = $1")
            .bind(snapshot_id)
            .fetch_one(&pool)
            .await
            .expect("narrative");
    let narrative = narrative.expect("narrative stored");
    assert!(narrative.contains("1 khoản"));
    assert!(narrative.contains("75.000"));
}

#[tokio::test]
async fn account_delete_purges_insight_snapshots() {
    let _lock = common::integration_lock();
    let pool = fresh_pool().await;
    let sender = format!("ins-del-{}", Uuid::new_v4());
    let account_id = seed_active_account(&pool, &sender).await;
    let now = Utc::now();
    insert_expense(&pool, account_id, 20_000, "tea", "confirmed", now).await;

    let store = IngressStore::new(pool.clone());
    process_text_command(&store, text_request(&sender, "evt-today", "/today", now))
        .await
        .expect("seed snapshot");

    let count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM insight_snapshots WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("snapshot count");
    assert_eq!(count_before, 1);

    let objects = InMemoryObjectStore::new();
    execute_account_delete(
        &pool,
        objects.as_ref(),
        &AccountDeletePayload {
            schema_version: 1,
            account_id,
            deletion_request_id: {
                sqlx::query_scalar(
                    r#"
                    INSERT INTO deletion_requests (id, account_id, state, expense_count)
                    VALUES ($1, $2, 'running', 1)
                    RETURNING id
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(account_id)
                .fetch_one(&pool)
                .await
                .expect("deletion request")
            },
        },
    )
    .await
    .expect("delete account");

    let count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM insight_snapshots WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("snapshot count after");
    assert_eq!(count_after, 0);
}
