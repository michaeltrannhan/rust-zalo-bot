//! M4 runtime receipt job dispatch integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::error::ErrorClass;
use zl_expense::ingress::{
    IngressOutcome, IngressPolicy, IngressRequest, IngressSource, process_image, store_with_receipt,
};
use zl_expense::outbound::OutboundJobExecution;
use zl_expense::provider::{
    InjectedMediaResolver, MediaDownloadPolicy, ZaloHttpAdapter, ZaloHttpConfig,
    ZaloMediaDownloader,
};
use zl_expense::receipt::{InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle, ReceiptState};
use zl_expense::runtime::{JobDeps, dispatch_leased_job};
use zl_expense::work::{ClaimOptions, ClaimedJob, WorkStore};

const PROVIDER_SCOPE: &str = "zalo:test-bot";
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

async fn fresh_pool() -> PgPool {
    common::receipt_fresh_pool().await
}

fn receipt_lifecycle(pool: PgPool) -> ReceiptLifecycle {
    ReceiptLifecycle::new(
        pool.clone(),
        InMemoryObjectStore::new(),
        ReceiptConfig {
            original_receipt_days: 7,
            review_expiry_hours: 72,
            ..ReceiptConfig::default()
        },
    )
}

async fn seed_sender(pool: &PgPool, sender: &str) -> Uuid {
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

async fn accept_image_submission(
    pool: &PgPool,
    receipt: &ReceiptLifecycle,
    sender: &str,
    event_id: &str,
    media_url: &str,
) -> (Uuid, Uuid) {
    let store = store_with_receipt(pool.clone(), receipt.clone());
    let request = IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: PROVIDER_SCOPE.to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: String::new(),
        observed_at: Utc::now(),
    };
    let outcome = process_image(&store, request, media_url.to_string())
        .await
        .expect("process image");
    assert!(matches!(outcome, IngressOutcome::Accepted { .. }));

    let submission_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM receipt_submissions WHERE account_id = (SELECT account_id FROM provider_identities WHERE provider_sender_id = $1)",
    )
    .bind(sender)
    .fetch_one(pool)
    .await
    .expect("submission");

    let ingest_job_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE job_type = 'receipt.ingest' AND dedupe_key = $1",
    )
    .bind(format!("receipt.ingest:{submission_id}"))
    .fetch_one(pool)
    .await
    .expect("ingest job");

    (submission_id, ingest_job_id)
}

fn test_adapter() -> Arc<ZaloHttpAdapter> {
    Arc::new(
        ZaloHttpAdapter::new(ZaloHttpConfig {
            api_base: "http://127.0.0.1:9".to_string(),
            bot_token: "test-token".to_string(),
            webhook_secret: "secret".to_string(),
            provider_scope: PROVIDER_SCOPE.to_string(),
            request_timeout: Duration::from_secs(2),
        })
        .expect("adapter"),
    )
}

fn job_deps_with_resolver(
    pool: PgPool,
    receipt: ReceiptLifecycle,
    resolver: InjectedMediaResolver,
    policy: MediaDownloadPolicy,
) -> JobDeps<InjectedMediaResolver> {
    JobDeps::new(
        pool,
        test_adapter(),
        receipt,
        IngressPolicy::default(),
        ZaloMediaDownloader::new(policy, resolver),
        Arc::new(zl_expense::insight::FakeNarrator),
    )
}

async fn claim_job(pool: &PgPool, job_id: Uuid) -> ClaimedJob {
    let store = WorkStore::new(pool.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let claimed = store
            .claim(ClaimOptions {
                batch_limit: 5,
                lease_owner: format!("test-worker-{}", Uuid::new_v4()),
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

async fn apply_execution(pool: &PgPool, job: &ClaimedJob, execution: OutboundJobExecution) {
    let store = WorkStore::new(pool.clone());
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

#[tokio::test]
async fn receipt_ingest_job_downloads_media_stores_asset_and_enqueues_extract() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("receipt_ingest_job_downloads_media") else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "runtime-ingest").await;
    let receipt = receipt_lifecycle(pool.clone());
    let png = common::corpus_png(0);
    let server = MediaLoopbackServer::spawn_png(png);
    let media_url = loopback_url(&server, "/receipt.png");

    let (submission_id, ingest_job_id) = accept_image_submission(
        &pool,
        &receipt,
        "runtime-ingest",
        "evt-runtime-ingest",
        "https://placeholder.test/receipt.png",
    )
    .await;

    sqlx::query("UPDATE inbound_events SET media_url = $2 WHERE provider_event_id = $1")
        .bind("evt-runtime-ingest")
        .bind(&media_url)
        .execute(&pool)
        .await
        .expect("set media url");

    let mut resolver = InjectedMediaResolver::new();
    resolver.insert(TEST_HOST, vec![LOOPBACK_IP]);
    let deps = job_deps_with_resolver(
        pool.clone(),
        receipt.clone(),
        resolver,
        loopback_policy(server.addr.port()),
    );

    let job = claim_job(&pool, ingest_job_id).await;
    let execution = dispatch_leased_job(&deps, &job).await;
    assert!(matches!(execution, OutboundJobExecution::Complete(_)));
    apply_execution(&pool, &job, execution).await;

    let account_id: Uuid =
        sqlx::query_scalar("SELECT account_id FROM receipt_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("account");
    common::assert_receipt_state(&receipt, account_id, submission_id, ReceiptState::Stored).await;

    let extract_jobs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE job_type = 'receipt.extract'")
            .fetch_one(&pool)
            .await
            .expect("extract jobs");
    assert_eq!(extract_jobs, 1);
}

#[tokio::test]
async fn receipt_extract_job_emits_one_review_outbound_even_when_run_twice() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("receipt_extract_job_emits_one_review_outbound")
    else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "runtime-extract").await;
    let receipt = receipt_lifecycle(pool.clone());
    let png = common::corpus_png(1);
    let server = MediaLoopbackServer::spawn_png(png);
    let media_url = loopback_url(&server, "/receipt.png");

    let (submission_id, ingest_job_id) = accept_image_submission(
        &pool,
        &receipt,
        "runtime-extract",
        "evt-runtime-extract",
        &media_url,
    )
    .await;

    let mut resolver = InjectedMediaResolver::new();
    resolver.insert(TEST_HOST, vec![LOOPBACK_IP]);
    let deps = job_deps_with_resolver(
        pool.clone(),
        receipt.clone(),
        resolver,
        loopback_policy(server.addr.port()),
    );

    let ingest_job = claim_job(&pool, ingest_job_id).await;
    let ingest_execution = dispatch_leased_job(&deps, &ingest_job).await;
    apply_execution(&pool, &ingest_job, ingest_execution).await;

    let extract_job_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE job_type = 'receipt.extract' AND dedupe_key = $1",
    )
    .bind(format!("receipt.extract:{submission_id}"))
    .fetch_one(&pool)
    .await
    .expect("extract job");

    let extract_job = claim_job(&pool, extract_job_id).await;
    let first = dispatch_leased_job(&deps, &extract_job).await;
    assert!(matches!(first, OutboundJobExecution::Complete(_)));
    let second = dispatch_leased_job(&deps, &extract_job).await;
    assert!(matches!(second, OutboundJobExecution::Complete(_)));
    apply_execution(&pool, &extract_job, second).await;

    let submission_state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("submission state");
    assert_eq!(submission_state, "review_required");

    let review_outbound: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages WHERE idempotency_key = $1")
            .bind(format!("receipt-review:{submission_id}"))
            .fetch_one(&pool)
            .await
            .expect("review outbound count");
    assert_eq!(review_outbound, 1);

    let review_jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE dedupe_key = $1")
        .bind(format!("outbound.deliver:receipt-review:{submission_id}"))
        .fetch_one(&pool)
        .await
        .expect("review deliver jobs");
    assert_eq!(review_jobs, 1);

    let pending: String = sqlx::query_scalar(
        "SELECT pending_action_type FROM conversation_states WHERE pending_payload_ref = $1",
    )
    .bind(submission_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("pending action");
    assert_eq!(pending, "receipt_review");
}

#[tokio::test]
async fn receipt_ingest_ssrf_or_oversize_marks_failed_permanent_and_dead_letters_job() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("receipt_ingest_ssrf_or_oversize") else {
        return;
    };

    let pool = fresh_pool().await;
    seed_sender(&pool, "runtime-fail").await;
    let receipt = receipt_lifecycle(pool.clone());
    let png = common::corpus_png(2);
    let server = MediaLoopbackServer::spawn_png(png);
    let media_url = loopback_url(&server, "/too-large.png");

    let (submission_id, ingest_job_id) = accept_image_submission(
        &pool,
        &receipt,
        "runtime-fail",
        "evt-runtime-fail",
        &media_url,
    )
    .await;

    let mut resolver = InjectedMediaResolver::new();
    resolver.insert(TEST_HOST, vec![LOOPBACK_IP]);
    let mut policy = MediaDownloadPolicy::production_default();
    policy.require_https = false;
    policy.allowed_explicit_port = Some(server.addr.port());
    policy.max_bytes = 16;
    let deps = job_deps_with_resolver(pool.clone(), receipt.clone(), resolver, policy);

    let job = claim_job(&pool, ingest_job_id).await;
    let execution = dispatch_leased_job(&deps, &job).await;
    assert!(matches!(
        execution,
        OutboundJobExecution::Fail(ErrorClass::Validation)
    ));
    apply_execution(&pool, &job, execution).await;

    let submission_state: String =
        sqlx::query_scalar("SELECT lifecycle_state FROM receipt_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("submission state");
    assert_eq!(submission_state, "failed_permanent");

    let job_state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
        .bind(ingest_job_id)
        .fetch_one(&pool)
        .await
        .expect("job state");
    assert_eq!(job_state, "dead");
}

#[tokio::test]
async fn unknown_job_type_is_invalid_job() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("unknown_job_type_is_invalid_job") else {
        return;
    };

    let pool = fresh_pool().await;
    let job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, job_type, payload, payload_version, state, priority, run_at,
            dedupe_key, serialization_key, max_attempts
        )
        VALUES ($1, 'unknown.job', '{"schema_version":1}', 1, 'queued', 0, NOW(), $2, NULL, 3)
        "#,
    )
    .bind(job_id)
    .bind(format!("unknown.job:{job_id}"))
    .execute(&pool)
    .await
    .expect("insert job");

    let receipt = receipt_lifecycle(pool.clone());
    let deps = job_deps_with_resolver(
        pool.clone(),
        receipt,
        InjectedMediaResolver::new(),
        MediaDownloadPolicy::production_default(),
    );
    let job = claim_job(&pool, job_id).await;
    let execution = dispatch_leased_job(&deps, &job).await;
    assert_eq!(execution, OutboundJobExecution::InvalidJob);
}
