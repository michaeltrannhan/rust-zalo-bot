//! M4 vertical slice: image webhook through confirmed expense via HTTP and jobs.

#![allow(clippy::await_holding_lock)]

mod common;

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::{Json, Router, routing::post};
use chrono::Utc;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::error::ErrorClass;
use zl_expense::health::ReadinessState;
use zl_expense::http::{AppState, WebhookService, router};
use zl_expense::ingress::store_with_receipt;
use zl_expense::outbound::OutboundJobExecution;
use zl_expense::provider::{
    InjectedMediaResolver, MediaDownloadPolicy, SECRET_HEADER, ZaloHttpAdapter, ZaloHttpConfig,
    ZaloMediaDownloader,
};
use zl_expense::receipt::{InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle};
use zl_expense::runtime::{JobDeps, dispatch_leased_job};
use zl_expense::work::{ClaimOptions, ClaimedJob, WorkStore};

const PROVIDER_SCOPE: &str = "zalo_bot";
const WEBHOOK_SECRET: &str = "m4-vertical-webhook-secret";
const TEST_HOST: &str = "s120.zdn.vn";
const LOOPBACK_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

struct MediaLoopbackServer {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl MediaLoopbackServer {
    fn spawn_png(body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let body = Arc::new(body);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                let mut shutdown_rx = shutdown_rx;
                loop {
                    let accept = listener.accept();
                    let shutdown = &mut shutdown_rx;
                    let (stream, _) = tokio::select! {
                        res = accept => res.expect("accept"),
                        _ = &mut *shutdown => return,
                    };
                    let body = Arc::clone(&body);
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut stream = stream;
                        let mut buf = [0u8; 1024];
                        let _ = stream.read(&mut buf).await;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(&body).await;
                    });
                }
            });
        });

        Self {
            addr,
            shutdown: Some(shutdown_tx),
        }
    }
}

impl Drop for MediaLoopbackServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn isolated_pool(database_url: &str) -> PgPool {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect admin");
    let schema = format!("m4_vertical_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;

    let search_path = schema;
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
                Json(json!({ "ok": true, "result": { "message_id": "provider-vertical-1" } }))
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

fn loopback_policy(port: u16) -> MediaDownloadPolicy {
    let mut policy = MediaDownloadPolicy::production_default();
    policy.require_https = false;
    policy.permit_private_resolved_addresses = true;
    policy.allowed_explicit_port = Some(port);
    policy
}

fn loopback_url(server: &MediaLoopbackServer, path: &str) -> String {
    format!("http://{TEST_HOST}:{}{}", server.addr.port(), path)
}

fn receipt_lifecycle(pool: PgPool) -> ReceiptLifecycle {
    ReceiptLifecycle::new(
        pool,
        InMemoryObjectStore::new(),
        ReceiptConfig {
            original_receipt_days: 7,
            review_expiry_hours: 72,
        },
    )
}

async fn seed_active_sender(pool: &PgPool, sender: &str) -> Uuid {
    let account_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO accounts (id, lifecycle_state, consent_version, consented_at)
        VALUES ($1, 'active', 'v1', NOW())
        "#,
    )
    .bind(account_id)
    .execute(pool)
    .await
    .expect("seed account");
    sqlx::query(
        r#"
        INSERT INTO provider_identities (id, account_id, provider_scope, provider_sender_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(account_id)
    .bind(PROVIDER_SCOPE)
    .bind(sender)
    .execute(pool)
    .await
    .expect("seed identity");
    account_id
}

struct VerticalHarness {
    pool: PgPool,
    account_id: Uuid,
    sender: String,
    media_url: String,
    png: Vec<u8>,
    webhook_url: String,
    client: reqwest::Client,
    deps: JobDeps<InjectedMediaResolver>,
    webhook_task: tokio::task::JoinHandle<()>,
    zalo_task: tokio::task::JoinHandle<()>,
    _media_server: MediaLoopbackServer,
}

impl VerticalHarness {
    async fn new(corpus_index: usize, sender: &str) -> Self {
        let database_url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
        let pool = isolated_pool(&database_url).await;
        let account_id = seed_active_sender(&pool, sender).await;
        let png = common::corpus_png(corpus_index);
        let media_server = MediaLoopbackServer::spawn_png(png.clone());
        let media_url = loopback_url(&media_server, "/receipt.png");

        let (api_base, _, zalo_task) = spawn_zalo_loopback().await;
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
        let receipt = receipt_lifecycle(pool.clone());
        let store = store_with_receipt(pool.clone(), receipt.clone());

        let mut resolver = InjectedMediaResolver::new();
        resolver.insert(TEST_HOST, vec![LOOPBACK_IP]);
        let deps = JobDeps::new(
            pool.clone(),
            Arc::clone(&adapter),
            receipt,
            ZaloMediaDownloader::new(loopback_policy(media_server.addr.port()), resolver),
        );

        let webhook = Arc::new(WebhookService::new(
            adapter,
            store,
            BTreeSet::from([sender.to_string()]),
            512,
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

        Self {
            pool,
            account_id,
            sender: sender.to_string(),
            media_url,
            png,
            webhook_url,
            client: reqwest::Client::new(),
            deps,
            webhook_task,
            zalo_task,
            _media_server: media_server,
        }
    }

    fn image_body(&self, event_id: &str) -> Value {
        json!({
            "event_name": "message.image.received",
            "message": {
                "message_id": event_id,
                "from": { "id": self.sender },
                "chat": { "id": format!("chat-{}", self.sender) },
                "date": Utc::now().timestamp(),
                "photo": self.media_url,
            }
        })
    }

    fn text_body(&self, event_id: &str, text: &str) -> Value {
        json!({
            "event_name": "message.text.received",
            "message": {
                "message_id": event_id,
                "from": { "id": self.sender },
                "chat": { "id": format!("chat-{}", self.sender) },
                "date": Utc::now().timestamp(),
                "text": text
            }
        })
    }

    async fn post_json(&self, body: &Value, secret: Option<&str>) -> reqwest::Response {
        let mut request = self
            .client
            .post(&self.webhook_url)
            .header("content-type", "application/json");
        if let Some(secret) = secret {
            request = request.header(SECRET_HEADER, secret);
        }
        request.json(body).send().await.expect("webhook request")
    }

    async fn status_field(&self, response: reqwest::Response) -> (StatusCode, String) {
        let status = response.status();
        let value = response.json::<Value>().await.expect("response JSON")["status"]
            .as_str()
            .expect("status field")
            .to_string();
        (status, value)
    }

    async fn post_image(&self, event_id: &str) -> (StatusCode, String) {
        self.status_field(
            self.post_json(&self.image_body(event_id), Some(WEBHOOK_SECRET))
                .await,
        )
        .await
    }

    async fn post_text(&self, event_id: &str, text: &str) -> (StatusCode, String) {
        self.status_field(
            self.post_json(&self.text_body(event_id, text), Some(WEBHOOK_SECRET))
                .await,
        )
        .await
    }

    async fn outbound_body(&self, event_id: &str) -> String {
        sqlx::query_scalar(
            r#"
            SELECT o.body
            FROM outbound_messages o
            JOIN inbound_events i ON i.id = o.inbound_event_id
            WHERE i.provider_event_id = $1
            "#,
        )
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .expect("outbound body")
    }

    async fn submission_for_event(&self, event_id: &str) -> Uuid {
        sqlx::query_scalar(
            r#"
            SELECT rs.id
            FROM receipt_submissions rs
            JOIN inbound_events ie ON ie.id = rs.inbound_event_id
            WHERE ie.provider_event_id = $1
            "#,
        )
        .bind(event_id)
        .fetch_one(&self.pool)
        .await
        .expect("submission id")
    }

    async fn ingest_job_for_submission(&self, submission_id: Uuid) -> Uuid {
        sqlx::query_scalar(
            "SELECT id FROM jobs WHERE job_type = 'receipt.ingest' AND dedupe_key = $1",
        )
        .bind(format!("receipt.ingest:{submission_id}"))
        .fetch_one(&self.pool)
        .await
        .expect("ingest job")
    }

    async fn extract_job_for_submission(&self, submission_id: Uuid) -> Uuid {
        sqlx::query_scalar(
            "SELECT id FROM jobs WHERE job_type = 'receipt.extract' AND dedupe_key = $1",
        )
        .bind(format!("receipt.extract:{submission_id}"))
        .fetch_one(&self.pool)
        .await
        .expect("extract job")
    }

    async fn claim_job(&self, job_id: Uuid) -> ClaimedJob {
        let store = WorkStore::new(self.pool.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let claimed = store
                .claim(ClaimOptions {
                    batch_limit: 5,
                    lease_owner: format!("vertical-worker-{}", Uuid::new_v4()),
                    lease_duration_secs: 60,
                })
                .await
                .expect("claim");
            for job in claimed {
                if job.id == job_id {
                    return job;
                }
                if store.complete(job.id, job.lease_token).await.is_err() {
                    let _ = store
                        .fail(job.id, job.lease_token, ErrorClass::Validation)
                        .await;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("expected claimed job {job_id}");
    }

    async fn apply_execution(&self, job: &ClaimedJob, execution: OutboundJobExecution) {
        let store = WorkStore::new(self.pool.clone());
        match execution {
            OutboundJobExecution::Complete(_) => {
                store
                    .complete(job.id, job.lease_token)
                    .await
                    .expect("complete");
            }
            OutboundJobExecution::Fail(class) => {
                store
                    .fail(job.id, job.lease_token, class)
                    .await
                    .expect("fail");
            }
            OutboundJobExecution::InvalidJob => {
                store
                    .fail(job.id, job.lease_token, ErrorClass::Validation)
                    .await
                    .expect("invalid fail");
            }
            OutboundJobExecution::StaleLease => {}
        }
    }

    async fn dispatch_job(&self, job_id: Uuid) -> OutboundJobExecution {
        let job = self.claim_job(job_id).await;
        let execution = dispatch_leased_job(&self.deps, &job).await;
        self.apply_execution(&job, execution.clone()).await;
        execution
    }

    async fn drive_ingest_and_extract(&self, event_id: &str) -> Uuid {
        let submission_id = self.submission_for_event(event_id).await;
        let ingest_job_id = self.ingest_job_for_submission(submission_id).await;
        let ingest = self.dispatch_job(ingest_job_id).await;
        assert!(matches!(ingest, OutboundJobExecution::Complete(_)));

        let extract_job_id = self.extract_job_for_submission(submission_id).await;
        let extract = self.dispatch_job(extract_job_id).await;
        assert!(matches!(extract, OutboundJobExecution::Complete(_)));
        submission_id
    }

    async fn pin_draft_occurred_at(&self, submission_id: Uuid, occurred_at: chrono::DateTime<Utc>) {
        sqlx::query("UPDATE expense_drafts SET occurred_at = $2 WHERE submission_id = $1")
            .bind(submission_id)
            .bind(occurred_at)
            .execute(&self.pool)
            .await
            .expect("pin draft occurred_at");
    }

    fn abort(self) {
        self.webhook_task.abort();
        self.zalo_task.abort();
    }
}

#[tokio::test]
async fn image_webhook_duplicate_event_id_keeps_single_ingest_job() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("image_webhook_duplicate_event_id") else {
        return;
    };

    let harness = VerticalHarness::new(0, "vertical-dup-event").await;
    let (status, value) = harness.post_image("evt-image-dup-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, "accepted");

    let (dup_status, dup_value) = harness.post_image("evt-image-dup-1").await;
    assert_eq!(dup_status, StatusCode::OK);
    assert_eq!(dup_value, "duplicate");

    let ingest_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE job_type = 'receipt.ingest'")
            .fetch_one(&harness.pool)
            .await
            .expect("ingest jobs");
    assert_eq!(ingest_jobs, 1);
    harness.abort();
}

#[tokio::test]
async fn image_webhook_ingest_extract_reaches_review_with_one_outbound() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("image_webhook_ingest_extract_review") else {
        return;
    };

    let harness = VerticalHarness::new(0, "vertical-review").await;
    assert_eq!(harness.post_image("evt-image-review").await.1, "accepted");

    let submission_id = harness.drive_ingest_and_extract("evt-image-review").await;
    let expected = common::expected_extraction(&harness.png);

    let submission_state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&harness.pool)
            .await
            .expect("submission state");
    assert_eq!(submission_state, "review_required");

    let review_outbound: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages WHERE idempotency_key = $1")
            .bind(format!("receipt-review:{submission_id}"))
            .fetch_one(&harness.pool)
            .await
            .expect("review outbound");
    assert_eq!(review_outbound, 1);

    let merchant: String =
        sqlx::query_scalar("SELECT merchant FROM expense_drafts WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(&harness.pool)
            .await
            .expect("draft merchant");
    assert_eq!(merchant, expected.merchant);
    harness.abort();
}

#[tokio::test]
async fn confirm_after_review_lists_expense_in_today_and_recent() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("confirm_after_review_today_recent") else {
        return;
    };

    let harness = VerticalHarness::new(0, "vertical-confirm").await;
    assert_eq!(harness.post_image("evt-image-confirm").await.1, "accepted");
    let submission_id = harness.drive_ingest_and_extract("evt-image-confirm").await;
    let now = Utc::now();
    harness.pin_draft_occurred_at(submission_id, now).await;

    assert_eq!(harness.post_text("evt-review-ok", "ok").await.1, "accepted");

    let confirmed: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM expenses
        WHERE account_id = $1 AND state = 'confirmed' AND source = 'receipt'
        "#,
    )
    .bind(harness.account_id)
    .fetch_one(&harness.pool)
    .await
    .expect("confirmed count");
    assert_eq!(confirmed, 1);

    let expected = common::expected_extraction(&harness.png);
    assert_eq!(harness.post_text("evt-today", "/today").await.1, "accepted");
    let today_body = harness.outbound_body("evt-today").await;
    assert!(today_body.contains("Tổng:"));
    assert!(today_body.contains("325.000"));

    assert_eq!(
        harness.post_text("evt-recent", "/recent").await.1,
        "accepted"
    );
    let recent_body = harness.outbound_body("evt-recent").await;
    assert!(recent_body.contains("Các khoản gần đây:"));
    assert!(recent_body.contains(&expected.merchant));
    harness.abort();
}

#[tokio::test]
async fn edit_amount_then_confirm_persists_edited_amount() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("edit_amount_then_confirm") else {
        return;
    };

    let harness = VerticalHarness::new(1, "vertical-edit").await;
    assert_eq!(harness.post_image("evt-image-edit").await.1, "accepted");
    harness.drive_ingest_and_extract("evt-image-edit").await;

    assert_eq!(
        harness.post_text("evt-review-edit", "sua 12000").await.1,
        "accepted"
    );
    assert_eq!(
        harness.post_text("evt-review-edit-ok", "ok").await.1,
        "accepted"
    );

    let amount_minor: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM expenses WHERE account_id = $1 AND state = 'confirmed'",
    )
    .bind(harness.account_id)
    .fetch_one(&harness.pool)
    .await
    .expect("amount");
    assert_eq!(amount_minor, 12_000);
    harness.abort();
}

#[tokio::test]
async fn reject_leaves_no_confirmed_receipt_expense() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("reject_leaves_no_confirmed") else {
        return;
    };

    let harness = VerticalHarness::new(2, "vertical-reject").await;
    assert_eq!(harness.post_image("evt-image-reject").await.1, "accepted");
    harness.drive_ingest_and_extract("evt-image-reject").await;

    assert_eq!(harness.post_text("evt-review-no", "no").await.1, "accepted");

    let confirmed: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM expenses
        WHERE account_id = $1 AND state = 'confirmed' AND source = 'receipt'
        "#,
    )
    .bind(harness.account_id)
    .fetch_one(&harness.pool)
    .await
    .expect("confirmed count");
    assert_eq!(confirmed, 0);
    harness.abort();
}

#[tokio::test]
async fn hash_duplicate_image_absorbed_without_second_confirmed() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("hash_duplicate_image_absorbed") else {
        return;
    };

    let harness = VerticalHarness::new(0, "vertical-hash-dup").await;
    assert_eq!(harness.post_image("evt-image-original").await.1, "accepted");
    let original_submission = harness.drive_ingest_and_extract("evt-image-original").await;
    harness
        .pin_draft_occurred_at(original_submission, Utc::now())
        .await;
    assert_eq!(
        harness.post_text("evt-original-ok", "ok").await.1,
        "accepted"
    );

    assert_eq!(harness.post_image("evt-image-hash-dup").await.1, "accepted");
    let duplicate_submission = harness.submission_for_event("evt-image-hash-dup").await;
    assert_ne!(duplicate_submission, original_submission);

    let ingest_job_id = harness
        .ingest_job_for_submission(duplicate_submission)
        .await;
    let ingest = harness.dispatch_job(ingest_job_id).await;
    assert!(matches!(ingest, OutboundJobExecution::Complete(_)));

    let duplicate_state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(duplicate_submission)
            .fetch_one(&harness.pool)
            .await
            .expect("duplicate state");
    assert_eq!(duplicate_state, "failed_permanent");

    let original_state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(original_submission)
            .fetch_one(&harness.pool)
            .await
            .expect("original state");
    assert_eq!(original_state, "confirmed");

    let confirmed: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM expenses
        WHERE account_id = $1 AND state = 'confirmed' AND source = 'receipt'
        "#,
    )
    .bind(harness.account_id)
    .fetch_one(&harness.pool)
    .await
    .expect("confirmed count");
    assert_eq!(confirmed, 1);
    harness.abort();
}

#[tokio::test]
async fn wrong_secret_unauthorized_and_unsupported_event_ok() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("wrong_secret_and_unsupported") else {
        return;
    };

    let harness = VerticalHarness::new(3, "vertical-auth").await;

    let unauthorized = harness
        .post_json(
            &harness.text_body("evt-bad-secret", "/start"),
            Some("wrong-secret"),
        )
        .await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let unsupported_body = json!({
        "event_name": "message.sticker.received",
        "message": {
            "message_id": "evt-unsupported",
            "from": { "id": harness.sender },
            "chat": { "id": format!("chat-{}", harness.sender) },
            "date": Utc::now().timestamp(),
            "sticker": { "id": "12097", "category": 1 }
        }
    });
    let (status, value) = harness
        .status_field(
            harness
                .post_json(&unsupported_body, Some(WEBHOOK_SECRET))
                .await,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, "unsupported");
    harness.abort();
}
