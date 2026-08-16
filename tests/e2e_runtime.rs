//! Process-level e2e: supervised binary, loopback Zalo, operator CLI.
//!
//! Covers the assembled M2–M7 path without external network. Receipt image
//! download stays in `m4_receipt_vertical` because production SSRF policy
//! rejects loopback media hosts.

#![allow(clippy::await_holding_lock)]

mod common;

use std::fs;
use std::process::Command as StdCommand;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use axum::{Json, Router, extract::State, routing::post};
use predicates::prelude::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use tempfile::TempDir;
use tokio::time::sleep;
use uuid::Uuid;
use zl_expense::config::load_config;
use zl_expense::db::create_pool;
use zl_expense::provider::SECRET_HEADER;

const WEBHOOK_SECRET: &str = "e2e-webhook-secret-value";
const BOT_TOKEN: &str = "e2e-bot-token-ok";

struct SpawnedRuntime {
    child: std::process::Child,
}

impl Drop for SpawnedRuntime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn runtime_text_path_and_operator_cli() {
    let _guard = common::integration_lock();
    let Some(db_url) = common::skip_without_database("runtime_text_path_and_operator_cli") else {
        return;
    };

    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let zalo_base = spawn_zalo_loopback(Arc::clone(&captured)).await;

    let app_port = common::available_port();
    let sender = format!("e2e-{}", Uuid::new_v4().simple());
    let cfg = write_e2e_config(&db_url, app_port, &zalo_base, &sender);

    migrate(cfg.path(), &db_url);

    let mut runtime = SpawnedRuntime {
        child: StdCommand::new(cargo_bin("zl-expense"))
            .args([
                "--config",
                cfg.path().to_str().expect("path"),
                "run",
                "--roles",
                "ingress,worker",
            ])
            .env("TEST_DATABASE_URL", &db_url)
            .spawn()
            .expect("spawn runtime"),
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");
    wait_for_http(
        &client,
        &format!("http://127.0.0.1:{app_port}/health/ready"),
        StatusCode::OK,
    )
    .await;

    let metrics = client
        .get(format!("http://127.0.0.1:{app_port}/metrics"))
        .send()
        .await
        .expect("metrics");
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics_body = metrics.text().await.expect("metrics body");
    assert!(metrics_body.contains("jobs_queued"));
    assert!(!metrics_body.contains("account_id"));

    let webhook = format!("http://127.0.0.1:{app_port}/webhooks/zalo");
    let unauthorized = post_text(&client, &webhook, None, &sender, "unauth-1", "/start").await;
    assert_eq!(unauthorized, StatusCode::UNAUTHORIZED);

    let start_id = unique_event("start");
    assert_eq!(
        post_text(
            &client,
            &webhook,
            Some(WEBHOOK_SECRET),
            &sender,
            &start_id,
            "/start",
        )
        .await,
        StatusCode::OK
    );
    let consent = wait_for_outbound(&cfg, &start_id).await;
    assert!(
        consent.contains("đồng ý") || consent.contains("ok"),
        "consent card: {consent}"
    );

    let yes_id = unique_event("yes");
    assert_eq!(
        post_text(
            &client,
            &webhook,
            Some(WEBHOOK_SECRET),
            &sender,
            &yes_id,
            "đồng ý",
        )
        .await,
        StatusCode::OK
    );
    let welcome = wait_for_outbound(&cfg, &yes_id).await;
    assert!(welcome.contains("Cảm ơn bạn"), "welcome: {welcome}");

    let cafe_id = unique_event("cafe");
    assert_eq!(
        post_text(
            &client,
            &webhook,
            Some(WEBHOOK_SECRET),
            &sender,
            &cafe_id,
            "cafe 45k",
        )
        .await,
        StatusCode::OK
    );
    let card = wait_for_outbound(&cfg, &cafe_id).await;
    assert!(card.contains("45.000"), "manual card: {card}");

    let ok_id = unique_event("ok");
    assert_eq!(
        post_text(
            &client,
            &webhook,
            Some(WEBHOOK_SECRET),
            &sender,
            &ok_id,
            "ok",
        )
        .await,
        StatusCode::OK
    );
    let confirmed = wait_for_outbound(&cfg, &ok_id).await;
    assert!(confirmed.contains("Đã ghi nhận"), "confirm: {confirmed}");
    assert!(confirmed.contains("45.000"), "confirm amount: {confirmed}");

    let today_id = unique_event("today");
    assert_eq!(
        post_text(
            &client,
            &webhook,
            Some(WEBHOOK_SECRET),
            &sender,
            &today_id,
            "/today",
        )
        .await,
        StatusCode::OK
    );
    let today = wait_for_outbound(&cfg, &today_id).await;
    assert!(today.contains("Tổng:"), "today: {today}");
    assert!(today.contains("45.000"), "today amount: {today}");

    let week_id = unique_event("week");
    assert_eq!(
        post_text(
            &client,
            &webhook,
            Some(WEBHOOK_SECRET),
            &sender,
            &week_id,
            "/week",
        )
        .await,
        StatusCode::OK
    );
    let week = wait_for_outbound(&cfg, &week_id).await;
    assert!(week.contains("45.000"), "week: {week}");

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "status",
            "--json",
        ])
        .env("TEST_DATABASE_URL", &db_url)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"ready\": true"))
        .stdout(predicate::str::contains("\"jobs\""))
        .stdout(predicate::str::contains("postgres://").not());

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args([
            "--config",
            cfg.path().to_str().expect("path"),
            "jobs",
            "list",
            "--json",
        ])
        .env("TEST_DATABASE_URL", &db_url)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"payload\"").not())
        .stdout(predicate::str::contains("dedupe_key").not());

    Command::cargo_bin("zl-expense")
        .expect("binary")
        .args(["--config", cfg.path().to_str().expect("path"), "doctor"])
        .env("TEST_DATABASE_URL", &db_url)
        .assert()
        .success()
        .stderr(predicate::str::contains("postgres://").not());

    let sent = captured.lock().expect("captured lock");
    assert!(
        sent.iter().any(|text| text.contains("45.000")),
        "loopback Zalo never received a 45.000 reply: {sent:?}"
    );
    drop(sent);

    let _ = runtime.child.kill();
}

fn unique_event(label: &str) -> String {
    format!("e2e-{label}-{}", Uuid::new_v4().simple())
}

struct E2eConfig {
    #[allow(dead_code)]
    dir: TempDir,
    config_path: std::path::PathBuf,
}

impl E2eConfig {
    fn path(&self) -> &std::path::Path {
        &self.config_path
    }
}

fn write_e2e_config(database_url: &str, app_port: u16, zalo_base: &str, sender: &str) -> E2eConfig {
    let dir = TempDir::new().expect("tempdir");
    let credentials_dir = dir.path().join("credentials");
    fs::create_dir_all(&credentials_dir).expect("credentials");
    fs::write(credentials_dir.join("database"), database_url).expect("db cred");
    fs::write(credentials_dir.join("zalo-bot"), BOT_TOKEN).expect("bot token");
    fs::write(credentials_dir.join("webhook-secret"), WEBHOOK_SECRET).expect("webhook secret");

    let config_path = dir.path().join("config.toml");
    fs::write(
        &config_path,
        format!(
            r#"
[server]
listen_address = "127.0.0.1:{app_port}"

[database]
url_credential = "database"
max_connections = 5

[concurrency]
receipt_extraction = 1
outbound_delivery = 4

[retention]
original_receipt_days = 7

[credentials]
directory = "{}"

[storage]
backend = "memory"

[extraction]
backend = "fake"

[access]
allowed_provider_sender_ids = ["{sender}"]

[zalo]
bot_token_credential = "zalo-bot"
webhook_secret_credential = "webhook-secret"
api_base = "{zalo_base}"
send_timeout_seconds = 5

[metrics]
enabled = true
"#,
            credentials_dir.display()
        ),
    )
    .expect("write config");

    E2eConfig { dir, config_path }
}

fn migrate(config_path: &std::path::Path, db_url: &str) {
    let status = StdCommand::new(cargo_bin("zl-expense"))
        .args([
            "--config",
            config_path.to_str().expect("path"),
            "db",
            "migrate",
        ])
        .env("TEST_DATABASE_URL", db_url)
        .status()
        .expect("migrate");
    assert!(status.success(), "migrate failed");
}

async fn spawn_zalo_loopback(captured: Arc<Mutex<Vec<String>>>) -> String {
    let app = Router::new()
        .route("/{*rest}", post(capture_send))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind zalo");
    let address = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve zalo");
    });
    address
}

async fn capture_send(
    State(captured): State<Arc<Mutex<Vec<String>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    if let Some(text) = body.get("text").and_then(Value::as_str) {
        captured.lock().expect("lock").push(text.to_string());
    }
    Json(json!({ "ok": true, "result": { "message_id": "e2e-provider" } }))
}

async fn post_text(
    client: &reqwest::Client,
    webhook: &str,
    secret: Option<&str>,
    sender: &str,
    event_id: &str,
    text: &str,
) -> StatusCode {
    let mut request = client
        .post(webhook)
        .header("content-type", "application/json")
        .json(&json!({
            "event_name": "message.text.received",
            "message": {
                "message_id": event_id,
                "from": { "id": sender },
                "chat": { "id": format!("chat-{sender}") },
                "date": chrono::Utc::now().timestamp(),
                "text": text
            }
        }));
    if let Some(secret) = secret {
        request = request.header(SECRET_HEADER, secret);
    }
    request.send().await.expect("webhook").status()
}

async fn wait_for_http(client: &reqwest::Client, url: &str, expected: StatusCode) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(response) = client.get(url).send().await
            && response.status() == expected
        {
            return;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {url} to return {expected}");
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_outbound(cfg: &E2eConfig, event_id: &str) -> String {
    let resolved = load_config(Some(cfg.path())).expect("config");
    let pool = create_pool(&resolved).await.expect("pool");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(body) = outbound_sent(&pool, event_id).await {
            return body;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for sent outbound for {event_id}");
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn outbound_sent(pool: &PgPool, event_id: &str) -> Option<String> {
    sqlx::query_scalar(
        r#"
        SELECT o.body
        FROM outbound_messages o
        JOIN inbound_events i ON i.id = o.inbound_event_id
        WHERE i.provider_event_id = $1
          AND o.state = 'sent'
        "#,
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .expect("outbound query")
}
