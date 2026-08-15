//! Milestone 3 supervised worker runtime integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::fs;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use axum::{Json, Router, http::StatusCode, routing::post};
use chrono::Utc;
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tempfile::TempDir;
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    DecisionOutput, IngressEffect, IngressObservation, IngressOutcome, IngressRequest,
    IngressSource, IngressStore, ReplyIntent,
};

const PROVIDER_SCOPE: &str = "zalo_bot";
const ALLOWED_SENDER: &str = "allowed-sender";

struct WorkerTestConfig {
    _dir: TempDir,
    config_path: std::path::PathBuf,
}

fn write_worker_config(database_url: &str, outbound_delivery: u32) -> WorkerTestConfig {
    let dir = TempDir::new().expect("tempdir");
    let credentials_dir = dir.path().join("credentials");
    fs::create_dir_all(&credentials_dir).expect("credentials dir");
    fs::write(credentials_dir.join("database"), database_url).expect("db cred");
    fs::write(credentials_dir.join("zalo-bot"), "test-token").expect("zalo cred");
    fs::write(
        credentials_dir.join("webhook-secret"),
        "test-webhook-secret-value",
    )
    .expect("webhook cred");

    let config_path = dir.path().join("config.toml");
    let contents = format!(
        r#"
[server]
listen_address = "127.0.0.1:0"

[database]
url_credential = "database"
max_connections = 5

[concurrency]
receipt_extraction = 1
outbound_delivery = {outbound_delivery}

[retention]
original_receipt_days = 7

[credentials]
directory = "{}"

[zalo]
bot_token_credential = "zalo-bot"
webhook_secret_credential = "webhook-secret"
"#,
        credentials_dir.display()
    );
    fs::write(&config_path, contents).expect("write config");

    WorkerTestConfig {
        _dir: dir,
        config_path,
    }
}

async fn isolated_test_env(database_url: &str) -> (PgPool, String) {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect admin");
    let schema = format!("m3_runtime_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;

    let search_path = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(10)
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

    let separator = if database_url.contains('?') { "&" } else { "?" };
    let subprocess_database_url =
        format!("{database_url}{separator}options=-c%20search_path%3D{schema}");
    (pool, subprocess_database_url)
}

fn ingress_request(event_id: &str, sender: &str) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: PROVIDER_SCOPE.to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: "/start".to_string(),
        observed_at: Utc::now(),
    }
}

async fn enqueue_reply_job(pool: &PgPool, event_id: &str) -> (Uuid, Uuid) {
    enqueue_reply_job_for_sender(pool, event_id, ALLOWED_SENDER).await
}

async fn enqueue_reply_job_for_sender(pool: &PgPool, event_id: &str, sender: &str) -> (Uuid, Uuid) {
    let store = IngressStore::new(pool.clone());
    let outcome = store
        .process(
            ingress_request(event_id, sender),
            IngressObservation::default(),
            |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::ReadOnly],
                    reply: Some(ReplyIntent {
                        body: format!("reply-{event_id}"),
                    }),
                })
            },
        )
        .await
        .expect("process ingress");
    let inbound_event_id = match outcome {
        IngressOutcome::Accepted { inbound_event_id } => inbound_event_id,
        other => panic!("expected accepted, got {other:?}"),
    };

    let outbound_id: Uuid =
        sqlx::query_scalar("SELECT id FROM outbound_messages WHERE inbound_event_id = $1")
            .bind(inbound_event_id)
            .fetch_one(pool)
            .await
            .expect("outbound id");

    let job_id: Uuid = sqlx::query_scalar("SELECT id FROM jobs WHERE dedupe_key = $1")
        .bind(format!(
            "outbound.deliver:reply:{PROVIDER_SCOPE}:{event_id}"
        ))
        .fetch_one(pool)
        .await
        .expect("job id");

    (outbound_id, job_id)
}

async fn wait_for_job_state(
    pool: &PgPool,
    job_id: Uuid,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let state: Option<String> = sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_optional(pool)
            .await
            .expect("job state");
        if state.as_deref() == Some(expected) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

async fn wait_for_outbound_state(
    pool: &PgPool,
    outbound_id: Uuid,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
                .bind(outbound_id)
                .fetch_optional(pool)
                .await
                .expect("outbound state");
        if state.as_deref() == Some(expected) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn spawn_worker_subprocess(
    config_path: &std::path::Path,
    database_url: &str,
    zalo_api_base: &str,
) -> std::process::Child {
    StdCommand::new(cargo_bin("zl-expense"))
        .args([
            "--config",
            config_path.to_str().expect("config path"),
            "run",
            "--roles",
            "worker",
        ])
        .env("TEST_DATABASE_URL", database_url)
        .env("ZL_EXPENSE_ZALO_API_BASE", zalo_api_base)
        .spawn()
        .expect("spawn worker")
}

async fn spawn_zalo_success_loopback() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
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

async fn spawn_slow_zalo_loopback(
    delay: Duration,
) -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let sends = Arc::new(AtomicUsize::new(0));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let send_counter = Arc::clone(&sends);
    let inflight_counter = Arc::clone(&in_flight);
    let max_counter = Arc::clone(&max_in_flight);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let send_counter = Arc::clone(&send_counter);
            let inflight_counter = Arc::clone(&inflight_counter);
            let max_counter = Arc::clone(&max_counter);
            let delay = delay;
            async move {
                send_counter.fetch_add(1, Ordering::SeqCst);
                let current = inflight_counter.fetch_add(1, Ordering::SeqCst) + 1;
                max_counter.fetch_max(current, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                inflight_counter.fetch_sub(1, Ordering::SeqCst);
                Json(json!({ "ok": true, "result": { "message_id": "provider-slow" } }))
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
    (address, sends, in_flight, max_in_flight, task)
}

async fn test_pool(database_url: &str) -> (PgPool, String) {
    isolated_test_env(database_url).await
}

#[tokio::test]
async fn worker_only_runtime_delivers_enqueued_job() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("worker_only_runtime_delivers_enqueued_job")
    else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let event_id = format!("m3-worker-deliver-{}", Uuid::new_v4());
    let (outbound_id, job_id) = enqueue_reply_job(&pool, &event_id).await;

    let (api_base, send_count, zalo_task) = spawn_zalo_success_loopback().await;
    let cfg = write_worker_config(&database_url, 4);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);

    assert!(
        wait_for_job_state(&pool, job_id, "completed", Duration::from_secs(10)).await,
        "job did not complete"
    );
    assert!(
        wait_for_outbound_state(&pool, outbound_id, "sent", Duration::from_secs(2)).await,
        "outbound not sent"
    );
    assert_eq!(send_count.load(Ordering::SeqCst), 1);

    worker.kill().expect("kill worker");
    let _ = worker.wait().expect("wait worker");
    zalo_task.abort();
}

#[tokio::test]
async fn worker_bounded_concurrency_never_exceeds_config() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("worker_bounded_concurrency_never_exceeds_config")
    else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let run_id = Uuid::new_v4();
    for idx in 0..4 {
        enqueue_reply_job_for_sender(
            &pool,
            &format!("m3-concurrency-{run_id}-{idx}"),
            &format!("sender-{run_id}-{idx}"),
        )
        .await;
    }

    let (api_base, send_count, _in_flight, max_in_flight, zalo_task) =
        spawn_slow_zalo_loopback(Duration::from_millis(400)).await;
    let cfg = write_worker_config(&database_url, 2);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if send_count.load(Ordering::SeqCst) >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let peak = max_in_flight.load(Ordering::SeqCst);
    assert!(peak > 1, "expected concurrent deliveries above one");
    assert!(
        peak <= 2,
        "peak concurrency {peak} exceeded configured bound of 2"
    );

    worker.kill().expect("kill worker");
    let _ = worker.wait().expect("wait worker");
    zalo_task.abort();
}

#[tokio::test]
async fn worker_serializes_deliveries_for_one_account() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("worker_serializes_deliveries_for_one_account")
    else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let run_id = Uuid::new_v4();
    let sender = format!("same-account-{run_id}");
    for idx in 0..3 {
        enqueue_reply_job_for_sender(&pool, &format!("m3-serialized-{run_id}-{idx}"), &sender)
            .await;
    }

    let (api_base, send_count, _in_flight, max_in_flight, zalo_task) =
        spawn_slow_zalo_loopback(Duration::from_millis(250)).await;
    let cfg = write_worker_config(&database_url, 3);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);

    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline && send_count.load(Ordering::SeqCst) < 3 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(send_count.load(Ordering::SeqCst), 3);
    assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);

    worker.kill().expect("kill worker");
    let _ = worker.wait().expect("wait worker");
    zalo_task.abort();
}

#[tokio::test]
async fn worker_retries_after_definite_provider_error() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("worker_retries_after_definite_provider_error")
    else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let event_id = format!("m3-retry-{}", Uuid::new_v4());
    let (outbound_id, job_id) = enqueue_reply_job(&pool, &event_id).await;

    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "provider failed".to_string(),
                    );
                }
                (
                    StatusCode::OK,
                    json!({ "ok": true, "result": { "message_id": "provider-retry" } }).to_string(),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let api_base = format!("http://{}", listener.local_addr().expect("address"));
    let zalo_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let cfg = write_worker_config(&database_url, 4);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);

    assert!(
        wait_for_job_state(&pool, job_id, "completed", Duration::from_secs(15)).await,
        "job did not complete after retry"
    );
    assert!(
        wait_for_outbound_state(&pool, outbound_id, "sent", Duration::from_secs(2)).await,
        "outbound not sent after retry"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    worker.kill().expect("kill worker");
    let _ = worker.wait().expect("wait worker");
    zalo_task.abort();
}

#[tokio::test]
async fn worker_dead_letters_after_bounded_provider_failures() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("worker_dead_letters_after_bounded_provider_failures")
    else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let event_id = format!("m3-dead-{}", Uuid::new_v4());
    let (outbound_id, job_id) = enqueue_reply_job(&pool, &event_id).await;
    sqlx::query("UPDATE jobs SET max_attempts = 2 WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .expect("set attempt limit");

    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::INTERNAL_SERVER_ERROR, "provider failed")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let api_base = format!("http://{}", listener.local_addr().expect("address"));
    let zalo_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let cfg = write_worker_config(&database_url, 2);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);
    assert!(
        wait_for_job_state(&pool, job_id, "dead", Duration::from_secs(15)).await,
        "job did not dead-letter"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(wait_for_outbound_state(&pool, outbound_id, "failed", Duration::from_secs(2)).await);

    worker.kill().expect("kill worker");
    let _ = worker.wait().expect("wait worker");
    zalo_task.abort();
}

#[tokio::test]
async fn worker_completes_ambiguous_provider_response_without_resend() {
    let _guard = common::integration_lock();
    let Some(database_url) = common::skip_without_database(
        "worker_completes_ambiguous_provider_response_without_resend",
    ) else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let event_id = format!("m3-ambiguous-{}", Uuid::new_v4());
    let (outbound_id, job_id) = enqueue_reply_job(&pool, &event_id).await;

    let sends = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&sends);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, "{not-json".to_string())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let api_base = format!("http://{}", listener.local_addr().expect("address"));
    let zalo_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let cfg = write_worker_config(&database_url, 4);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);

    assert!(
        wait_for_job_state(&pool, job_id, "completed", Duration::from_secs(10)).await,
        "job did not complete"
    );
    assert!(
        wait_for_outbound_state(&pool, outbound_id, "ambiguous", Duration::from_secs(2)).await,
        "outbound not ambiguous"
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    worker.kill().expect("kill worker");
    let _ = worker.wait().expect("wait worker");
    zalo_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn worker_shutdown_during_delivery_leaves_recoverable_state() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("worker_shutdown_during_delivery_leaves_recoverable_state")
    else {
        return;
    };
    let (pool, worker_database_url) = test_pool(&database_url).await;
    let event_id = format!("m3-shutdown-{}", Uuid::new_v4());
    let (outbound_id, job_id) = enqueue_reply_job(&pool, &event_id).await;

    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    let requests = Arc::new(AtomicUsize::new(0));
    let request_counter = Arc::clone(&requests);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let wait = Arc::clone(&release_rx);
            let request_counter = Arc::clone(&request_counter);
            async move {
                request_counter.fetch_add(1, Ordering::SeqCst);
                if let Some(rx) = wait.lock().await.take() {
                    let _ = rx.await;
                }
                Json(json!({ "ok": true, "result": { "message_id": "late" } }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let api_base = format!("http://{}", listener.local_addr().expect("address"));
    let zalo_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let cfg = write_worker_config(&database_url, 4);
    let mut worker = spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);

    for _ in 0..100 {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
                .bind(outbound_id)
                .fetch_optional(&pool)
                .await
                .expect("outbound state");
        if state.as_deref() == Some("sending") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let pid = worker.id();
    StdCommand::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("send SIGTERM");

    let deadline = Instant::now() + Duration::from_secs(10);
    let exit_status = loop {
        if let Ok(Some(status)) = worker.try_wait() {
            break status;
        }
        if Instant::now() >= deadline {
            worker.kill().expect("kill after deadline");
            panic!("worker did not exit promptly after SIGTERM");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(exit_status.success(), "expected graceful worker exit");

    let job_state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .expect("job state");
    assert_eq!(
        job_state, "queued",
        "shutdown should requeue the in-flight job"
    );

    let outbound_state: String =
        sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_one(&pool)
            .await
            .expect("outbound state");
    assert!(
        outbound_state == "sending" || outbound_state == "failed",
        "unexpected outbound state after shutdown: {outbound_state}"
    );

    let mut recovery_worker =
        spawn_worker_subprocess(&cfg.config_path, &worker_database_url, &api_base);
    assert!(
        wait_for_job_state(&pool, job_id, "completed", Duration::from_secs(10)).await,
        "recovery worker did not finish the durable job"
    );
    assert!(
        wait_for_outbound_state(&pool, outbound_id, "ambiguous", Duration::from_secs(2)).await,
        "unfinished send was not conservatively reconciled"
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "recovery must not issue a second provider request"
    );

    recovery_worker.kill().expect("kill recovery worker");
    let _ = recovery_worker.wait().expect("wait recovery worker");

    let _ = release_tx.send(());
    zalo_task.abort();
}
