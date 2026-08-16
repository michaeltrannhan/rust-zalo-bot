//! Milestone 2 authenticated webhook-to-real-adapter vertical contract.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{Json, Router, routing::post};
use reqwest::StatusCode;
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::health::ReadinessState;
use zl_expense::http::{AppState, WebhookService, router};
use zl_expense::ingress::IngressStore;
use zl_expense::outbound::{DeliveryState, deliver_next};
use zl_expense::provider::{SECRET_HEADER, ZaloHttpAdapter, ZaloHttpConfig};

async fn isolated_pool(database_url: &str) -> PgPool {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect admin");
    let schema = format!("m2_http_{}", Uuid::new_v4().simple());
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
        .expect("connect isolated");
    MIGRATOR.run(&pool).await.expect("migrate");
    pool
}

async fn spawn_zalo_loopback() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&sends);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Json(json!({ "ok": true, "result": { "message_id": "provider-1" } }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Zalo loopback");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve Zalo loopback");
    });
    (address, sends, task)
}

#[tokio::test]
async fn verified_duplicate_webhook_delivers_exactly_one_reply() {
    let Some(database_url) =
        common::skip_without_database("verified_duplicate_webhook_delivers_exactly_one_reply")
    else {
        return;
    };
    let pool = isolated_pool(&database_url).await;
    let (api_base, send_count, zalo_task) = spawn_zalo_loopback().await;
    let secret = "webhook-secret-for-test";
    let adapter = Arc::new(
        ZaloHttpAdapter::new(ZaloHttpConfig {
            api_base,
            bot_token: "test-token".to_string(),
            webhook_secret: secret.to_string(),
            provider_scope: "zalo_bot".to_string(),
            request_timeout: Duration::from_secs(2),
        })
        .expect("adapter"),
    );
    let webhook = Arc::new(WebhookService::new(
        Arc::clone(&adapter),
        IngressStore::new(pool.clone()),
        BTreeSet::from(["allowed-sender".to_string()]),
        512,
    ));
    let app = router(AppState {
        readiness: Arc::new(ReadinessState::new_ready()),
        pool: Some(pool.clone()),
        webhook: Some(webhook),
        metrics: None,
        metrics_enabled: false,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind webhook");
    let webhook_url = format!(
        "http://{}/webhooks/zalo",
        listener.local_addr().expect("address")
    );
    let webhook_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve webhook");
    });

    let body = json!({
        "event_name": "message.text.received",
        "message": {
            "message_id": "webhook-event-1",
            "from": { "id": "allowed-sender" },
            "chat": { "id": "private-chat" },
            "date": chrono::Utc::now().timestamp(),
            "text": "/start"
        }
    });
    let client = reqwest::Client::new();

    let unauthorized = client
        .post(&webhook_url)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .expect("unauthorized request");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let inbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbound_events")
        .fetch_one(&pool)
        .await
        .expect("inbound count");
    assert_eq!(inbound_count, 0);

    let accepted = client
        .post(&webhook_url)
        .header(SECRET_HEADER, secret)
        .header("content-type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .expect("accepted request");
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(
        accepted.json::<serde_json::Value>().await.expect("JSON")["status"],
        "accepted"
    );

    let duplicate = client
        .post(&webhook_url)
        .header(SECRET_HEADER, secret)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .expect("duplicate request");
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(
        duplicate.json::<serde_json::Value>().await.expect("JSON")["status"],
        "duplicate"
    );

    let delivered = deliver_next(&pool, &adapter)
        .await
        .expect("deliver")
        .expect("queued message");
    assert_eq!(delivered.state, DeliveryState::Sent);
    assert!(
        deliver_next(&pool, &adapter)
            .await
            .expect("second")
            .is_none()
    );
    assert_eq!(send_count.load(Ordering::SeqCst), 1);
    let delivery: (String, Option<String>, i32) =
        sqlx::query_as("SELECT state, provider_message_id, attempt_count FROM outbound_messages")
            .fetch_one(&pool)
            .await
            .expect("delivery state");
    assert_eq!(
        delivery,
        ("sent".to_string(), Some("provider-1".to_string()), 1)
    );

    let oversized = client
        .post(&webhook_url)
        .header(SECRET_HEADER, secret)
        .header("content-type", "application/json")
        .body("x".repeat(513))
        .send()
        .await
        .expect("oversized request");
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

    webhook_task.abort();
    zalo_task.abort();
}
