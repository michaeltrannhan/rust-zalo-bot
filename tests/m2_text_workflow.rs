//! Milestone 2 vertical workflow through the public ingress and conversation seams.

mod common;

use chrono::{Duration, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    IngressOutcome, IngressRequest, IngressSource, IngressStore, process_text_command,
};

async fn isolated_pool(database_url: &str) -> PgPool {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect admin");
    let schema = format!("m2_workflow_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;

    let search_path = schema;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let statement = format!("SET search_path TO {search_path}");
            Box::pin(async move {
                sqlx::query(&statement).execute(connection).await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect isolated schema");
    MIGRATOR.run(&pool).await.expect("migrate");
    pool
}

fn request(event_id: &str, text: &str, now: chrono::DateTime<Utc>) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo_bot:test".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: "allowed-family-member".to_string(),
        provider_chat_id: "private-chat-42".to_string(),
        sender_allowed: true,
        user_text: text.to_string(),
        observed_at: now,
    }
}

async fn outbound_body(pool: &PgPool, event_id: &str) -> String {
    sqlx::query_scalar(
        r#"
        SELECT o.body
        FROM outbound_messages o
        JOIN inbound_events i ON i.id = o.inbound_event_id
        WHERE i.provider_event_id = $1
        "#,
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .expect("outbound body")
}

#[tokio::test]
async fn duplicate_webhook_and_polling_create_one_command_and_reply() {
    let Some(database_url) =
        common::skip_without_database("duplicate_webhook_and_polling_create_one_command_and_reply")
    else {
        return;
    };
    let pool = isolated_pool(&database_url).await;
    let store = IngressStore::new(pool.clone());
    let now = Utc::now();

    let start = request("evt-start", "/start", now);
    assert!(matches!(
        process_text_command(&store, start.clone())
            .await
            .expect("start"),
        IngressOutcome::Accepted { .. }
    ));
    assert!(
        outbound_body(&pool, "evt-start")
            .await
            .starts_with("Xin chào!")
    );
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle_state FROM accounts")
        .fetch_one(&pool)
        .await
        .expect("account");
    assert_eq!(lifecycle, "pending_consent");

    assert!(matches!(
        process_text_command(&store, start)
            .await
            .expect("duplicate start"),
        IngressOutcome::Duplicate { .. }
    ));

    process_text_command(
        &store,
        request("evt-consent", "đồng ý", now + Duration::seconds(1)),
    )
    .await
    .expect("consent");
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle_state FROM accounts")
        .fetch_one(&pool)
        .await
        .expect("active account");
    assert_eq!(lifecycle, "active");

    process_text_command(
        &store,
        request("evt-manual", "cafe 45k", now + Duration::seconds(2)),
    )
    .await
    .expect("manual");
    assert!(
        outbound_body(&pool, "evt-manual")
            .await
            .contains("45.000 ₫")
    );
    let draft: (Uuid, String) = sqlx::query_as("SELECT id, state FROM expenses")
        .fetch_one(&pool)
        .await
        .expect("draft");
    assert_eq!(draft.1, "awaiting_confirmation");
    let expires_at: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM conversation_states")
            .fetch_one(&pool)
            .await
            .expect("pending expiry");
    assert_eq!(
        expires_at,
        now + Duration::seconds(2) + Duration::minutes(15)
    );

    process_text_command(
        &store,
        request("evt-confirm", "ok", now + Duration::seconds(3)),
    )
    .await
    .expect("confirm");
    let state: String = sqlx::query_scalar("SELECT state FROM expenses WHERE id = $1")
        .bind(draft.0)
        .fetch_one(&pool)
        .await
        .expect("confirmed expense");
    assert_eq!(state, "confirmed");

    process_text_command(
        &store,
        request("evt-today", "/today", now + Duration::seconds(4)),
    )
    .await
    .expect("today");
    assert!(
        outbound_body(&pool, "evt-today")
            .await
            .contains("Tổng: 45.000 ₫")
    );

    let recent = request("evt-recent", "/recent", now + Duration::seconds(5));
    process_text_command(&store, recent.clone())
        .await
        .expect("recent");
    let recent_reply = outbound_body(&pool, "evt-recent").await;
    assert!(recent_reply.contains("Các khoản gần đây:"));
    assert!(recent_reply.contains("45.000 ₫ · cafe · Khác"));

    sqlx::query(
        "UPDATE ingress_control SET mode = 'polling', mode_generation = mode_generation + 1",
    )
    .execute(&pool)
    .await
    .expect("switch to polling");
    let mut polling_duplicate = recent;
    polling_duplicate.source = IngressSource::Polling;
    assert!(matches!(
        process_text_command(&store, polling_duplicate)
            .await
            .expect("polling duplicate"),
        IngressOutcome::Duplicate { .. }
    ));

    let inbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbound_events")
        .fetch_one(&pool)
        .await
        .expect("inbound count");
    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    assert_eq!(inbound_count, 6);
    assert_eq!(outbound_count, 6);
}

#[tokio::test]
async fn pending_confirmation_loads_its_referenced_draft_beyond_recent_window() {
    let Some(database_url) = common::skip_without_database(
        "pending_confirmation_loads_its_referenced_draft_beyond_recent_window",
    ) else {
        return;
    };
    let pool = isolated_pool(&database_url).await;
    let store = IngressStore::new(pool.clone());
    let now = Utc::now();

    process_text_command(&store, request("evt-start-window", "/start", now))
        .await
        .expect("start");
    process_text_command(
        &store,
        request("evt-consent-window", "đồng ý", now + Duration::seconds(1)),
    )
    .await
    .expect("consent");
    process_text_command(
        &store,
        request("evt-manual-window", "cafe 45k", now + Duration::seconds(2)),
    )
    .await
    .expect("manual");

    let (account_id, draft_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT account_id, id FROM expenses WHERE state = 'awaiting_confirmation'")
            .fetch_one(&pool)
            .await
            .expect("pending draft");
    for offset in 0..10 {
        sqlx::query(
            r#"
            INSERT INTO expenses (
                id, account_id, amount_minor, currency, occurred_at, description, source, state
            )
            VALUES ($1, $2, 100, 'VND', $3, 'later', 'manual', 'confirmed')
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(account_id)
        .bind(now + Duration::hours(offset + 1))
        .execute(&pool)
        .await
        .expect("seed later expense");
    }

    process_text_command(
        &store,
        request("evt-confirm-window", "ok", now + Duration::seconds(3)),
    )
    .await
    .expect("confirm referenced draft");
    let state: String = sqlx::query_scalar("SELECT state FROM expenses WHERE id = $1")
        .bind(draft_id)
        .fetch_one(&pool)
        .await
        .expect("draft state");
    assert_eq!(state, "confirmed");
}
