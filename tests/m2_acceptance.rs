//! Milestone 2 acceptance tests through public HTTP, ingress, and outbound seams.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{Json, Router, routing::post};
use chrono::Utc;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::health::ReadinessState;
use zl_expense::http::{AppState, WebhookService, router};
use zl_expense::ingress::{
    IngressOutcome, IngressRequest, IngressSource, IngressStore, process_text_command,
};
use zl_expense::outbound::{DeliveryState, deliver_next};
use zl_expense::provider::{SECRET_HEADER, ZaloHttpAdapter, ZaloHttpConfig};

const PROVIDER_SCOPE: &str = "zalo_bot";
const WEBHOOK_SECRET: &str = "m2-acceptance-webhook-secret";
const ALLOWED_SENDER: &str = "allowed-family-member";
const DENIED_SENDER: &str = "denied-stranger";
const NOT_ALLOWED_REPLY: &str =
    "Xin lỗi, tài khoản của bạn chưa được cấp quyền dùng bot trong giai đoạn thử nghiệm.";

struct WebhookHarness {
    webhook_url: String,
    client: reqwest::Client,
    webhook_task: tokio::task::JoinHandle<()>,
    zalo_task: tokio::task::JoinHandle<()>,
}

async fn isolated_pool(database_url: &str) -> PgPool {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect admin");
    let schema = format!("m2_accept_{}", Uuid::new_v4().simple());
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

async fn spawn_zalo_loopback() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&sends);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Json(json!({ "ok": true, "result": { "message_id": "provider-acceptance-1" } }))
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

async fn spawn_webhook_harness(
    pool: PgPool,
    allowed_senders: BTreeSet<String>,
    max_body_bytes: usize,
) -> WebhookHarness {
    let (api_base, _send_count, zalo_task) = spawn_zalo_loopback().await;
    let adapter = Arc::new(
        ZaloHttpAdapter::new(ZaloHttpConfig {
            api_base,
            bot_token: "test-token".to_string(),
            webhook_secret: WEBHOOK_SECRET.to_string(),
            provider_scope: PROVIDER_SCOPE.to_string(),
            request_timeout: Duration::from_secs(2),
        })
        .expect("adapter"),
    );
    let webhook = Arc::new(WebhookService::new(
        Arc::clone(&adapter),
        IngressStore::new(pool.clone()),
        allowed_senders,
        max_body_bytes,
    ));
    let app = router(AppState {
        readiness: Arc::new(ReadinessState::new_ready()),
        pool: Some(pool.clone()),
        webhook: Some(webhook),
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

    WebhookHarness {
        webhook_url,
        client: reqwest::Client::new(),
        webhook_task,
        zalo_task,
    }
}

fn text_body(event_id: &str, sender: &str, text: &str) -> Value {
    json!({
        "event_name": "message.text.received",
        "message": {
            "message_id": event_id,
            "from": { "id": sender },
            "chat": { "id": format!("chat-{sender}") },
            "date": Utc::now().timestamp(),
            "text": text
        }
    })
}

impl WebhookHarness {
    async fn post_json(&self, body: &Value) -> reqwest::Response {
        self.client
            .post(&self.webhook_url)
            .header(SECRET_HEADER, WEBHOOK_SECRET)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .expect("webhook request")
    }

    async fn post_raw(
        &self,
        content_type: &str,
        body: impl Into<reqwest::Body>,
    ) -> reqwest::Response {
        self.client
            .post(&self.webhook_url)
            .header(SECRET_HEADER, WEBHOOK_SECRET)
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .expect("webhook request")
    }

    async fn status_field(&self, response: reqwest::Response) -> (StatusCode, String) {
        let status = response.status();
        let value = response.json::<Value>().await.expect("response JSON")["status"]
            .as_str()
            .expect("status field")
            .to_string();
        (status, value)
    }

    fn abort(self) {
        self.webhook_task.abort();
        self.zalo_task.abort();
    }
}

async fn seed_outbound(
    pool: &PgPool,
    id: Uuid,
    state: &str,
    attempt_count: i32,
    provider_message_id: Option<&str>,
) {
    sqlx::query(
        r#"
        INSERT INTO outbound_messages (
            id,
            idempotency_key,
            provider_scope,
            provider_target,
            body,
            state,
            attempt_count,
            provider_message_id
        )
        VALUES ($1, $2, $3, 'chat-acceptance', 'seeded body', $4, $5, $6)
        "#,
    )
    .bind(id)
    .bind(format!("seed:{id}"))
    .bind(PROVIDER_SCOPE)
    .bind(state)
    .bind(attempt_count)
    .bind(provider_message_id)
    .execute(pool)
    .await
    .expect("seed outbound");
}

#[tokio::test]
async fn authenticated_webhook_returns_stable_status_for_rejected_inputs() {
    let Some(database_url) = common::skip_without_database(
        "authenticated_webhook_returns_stable_status_for_rejected_inputs",
    ) else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let harness = spawn_webhook_harness(
        pool.clone(),
        BTreeSet::from([ALLOWED_SENDER.to_string()]),
        512,
    )
    .await;
    let valid = text_body("evt-valid", ALLOWED_SENDER, "/start");

    let (status, value) = harness
        .status_field(
            harness
                .client
                .post(&harness.webhook_url)
                .header("content-type", "application/json")
                .json(&valid)
                .send()
                .await
                .expect("missing secret"),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value, "unauthorized");

    let (status, value) = harness
        .status_field(harness.post_raw("text/plain", valid.to_string()).await)
        .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(value, "invalid_content_type");

    let (status, value) = harness
        .status_field(harness.post_raw("application/json", "{not-json").await)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value, "invalid_payload");

    let (status, value) = harness
        .status_field(harness.post_raw("application/json", "x".repeat(513)).await)
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(value, "payload_too_large");

    sqlx::query("UPDATE ingress_control SET mode = 'polling' WHERE id = 1")
        .execute(&pool)
        .await
        .expect("switch to polling");
    let (status, value) = harness.status_field(harness.post_json(&valid).await).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value, "mode_rejected");

    let inbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbound_events")
        .fetch_one(&pool)
        .await
        .expect("inbound count");
    assert_eq!(inbound_count, 1);
    let rejected_state: String = sqlx::query_scalar(
        "SELECT processing_state FROM inbound_events WHERE provider_event_id = $1",
    )
    .bind("evt-valid")
    .fetch_one(&pool)
    .await
    .expect("mode rejected event");
    assert_eq!(rejected_state, "rejected");

    harness.abort();
}

#[tokio::test]
async fn allowlist_denial_queues_one_deterministic_reply_without_account() {
    let Some(database_url) = common::skip_without_database(
        "allowlist_denial_queues_one_deterministic_reply_without_account",
    ) else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let harness = spawn_webhook_harness(
        pool.clone(),
        BTreeSet::from([ALLOWED_SENDER.to_string()]),
        512,
    )
    .await;
    let body = text_body("evt-denied", DENIED_SENDER, "/help");

    let (status, value) = harness.status_field(harness.post_json(&body).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, "accepted");

    let account_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .expect("accounts");
    let identity_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_identities")
        .fetch_one(&pool)
        .await
        .expect("identities");
    assert_eq!(account_count, 0);
    assert_eq!(identity_count, 0);

    let outbound: (String, Option<Uuid>) =
        sqlx::query_as("SELECT body, account_id FROM outbound_messages WHERE idempotency_key = $1")
            .bind(format!("reply:{PROVIDER_SCOPE}:evt-denied"))
            .fetch_one(&pool)
            .await
            .expect("denied outbound");
    assert_eq!(outbound, (NOT_ALLOWED_REPLY.to_string(), None));

    let duplicate = harness.status_field(harness.post_json(&body).await).await;
    assert_eq!(duplicate.0, StatusCode::OK);
    assert_eq!(duplicate.1, "duplicate");
    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    assert_eq!(outbound_count, 1);

    harness.abort();
}

#[tokio::test]
async fn duplicate_webhook_and_polling_event_share_one_command_and_reply() {
    let Some(database_url) = common::skip_without_database(
        "duplicate_webhook_and_polling_event_share_one_command_and_reply",
    ) else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let harness = spawn_webhook_harness(
        pool.clone(),
        BTreeSet::from([ALLOWED_SENDER.to_string()]),
        512,
    )
    .await;
    let body = text_body("evt-cross-mode", ALLOWED_SENDER, "/start");

    let (status, value) = harness.status_field(harness.post_json(&body).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, "accepted");

    let duplicate = harness.status_field(harness.post_json(&body).await).await;
    assert_eq!(duplicate.0, StatusCode::OK);
    assert_eq!(duplicate.1, "duplicate");

    sqlx::query(
        "UPDATE ingress_control SET mode = 'polling', mode_generation = mode_generation + 1",
    )
    .execute(&pool)
    .await
    .expect("switch to polling");

    let store = IngressStore::new(pool.clone());
    let polling = IngressRequest {
        source: IngressSource::Polling,
        provider_scope: PROVIDER_SCOPE.to_string(),
        provider_event_id: "evt-cross-mode".to_string(),
        provider_sender_id: ALLOWED_SENDER.to_string(),
        provider_chat_id: format!("chat-{ALLOWED_SENDER}"),
        sender_allowed: true,
        user_text: "/start".to_string(),
        observed_at: Utc::now(),
    };
    assert!(matches!(
        process_text_command(&store, polling)
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
    assert_eq!(inbound_count, 1);
    assert_eq!(outbound_count, 1);

    harness.abort();
}

#[tokio::test]
async fn deliver_next_skips_sent_sending_and_ambiguous_rows() {
    let Some(database_url) =
        common::skip_without_database("deliver_next_skips_sent_sending_and_ambiguous_rows")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (api_base, send_count, zalo_task) = spawn_zalo_loopback().await;
    let adapter = ZaloHttpAdapter::new(ZaloHttpConfig {
        api_base,
        bot_token: "test-token".to_string(),
        webhook_secret: WEBHOOK_SECRET.to_string(),
        provider_scope: PROVIDER_SCOPE.to_string(),
        request_timeout: Duration::from_secs(2),
    })
    .expect("adapter");

    let sent_id = Uuid::new_v4();
    let sending_id = Uuid::new_v4();
    let ambiguous_id = Uuid::new_v4();
    let queued_id = Uuid::new_v4();
    seed_outbound(&pool, sent_id, "sent", 1, Some("already-sent")).await;
    seed_outbound(&pool, sending_id, "sending", 1, None).await;
    seed_outbound(&pool, ambiguous_id, "ambiguous", 1, None).await;
    seed_outbound(&pool, queued_id, "queued", 0, None).await;

    let delivered = deliver_next(&pool, &adapter)
        .await
        .expect("deliver queued")
        .expect("queued delivery");
    assert_eq!(delivered.outbound_id, queued_id);
    assert_eq!(delivered.state, DeliveryState::Sent);
    assert_eq!(send_count.load(Ordering::SeqCst), 1);

    assert!(
        deliver_next(&pool, &adapter)
            .await
            .expect("no more queued")
            .is_none()
    );
    assert_eq!(send_count.load(Ordering::SeqCst), 1);

    let terminal_rows: Vec<(Uuid, String, i32, Option<String>)> = sqlx::query_as(
        "SELECT id, state, attempt_count, provider_message_id FROM outbound_messages WHERE id = ANY($1)",
    )
    .bind(vec![sent_id, sending_id, ambiguous_id])
    .fetch_all(&pool)
    .await
    .expect("terminal rows");
    assert_eq!(
        terminal_rows,
        vec![
            (
                sent_id,
                "sent".to_string(),
                1,
                Some("already-sent".to_string())
            ),
            (sending_id, "sending".to_string(), 1, None),
            (ambiguous_id, "ambiguous".to_string(), 1, None),
        ]
    );

    let queued_state: (String, i32, Option<String>) = sqlx::query_as(
        "SELECT state, attempt_count, provider_message_id FROM outbound_messages WHERE id = $1",
    )
    .bind(queued_id)
    .fetch_one(&pool)
    .await
    .expect("queued row");
    assert_eq!(
        queued_state,
        (
            "sent".to_string(),
            1,
            Some("provider-acceptance-1".to_string())
        )
    );

    zalo_task.abort();
}
