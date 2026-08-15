//! Milestone 3 ingress/outbound durable job bridge integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{Json, Router, http::StatusCode, routing::post};
use chrono::Utc;
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    DecisionOutput, IngressEffect, IngressOutcome, IngressRequest, IngressSource, IngressStore,
    ReplyIntent,
};
use zl_expense::outbound::{
    DeliveryResult, DeliveryState, OutboundJobExecution, deliver_for_job, deliver_next,
};
use zl_expense::provider::{ZaloHttpAdapter, ZaloHttpConfig};
use zl_expense::work::{ClaimOptions, EnqueueOutcome, EnqueueRequest, WorkStore};

const PROVIDER_SCOPE: &str = "zalo_bot";
const ALLOWED_SENDER: &str = "allowed-sender";

async fn isolated_pool(database_url: &str) -> PgPool {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect admin");
    let schema = format!("m3_outbound_{}", Uuid::new_v4().simple());
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

fn ingress_request(event_id: &str, sender: &str, allowed: bool) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: PROVIDER_SCOPE.to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: allowed,
        user_text: "/start".to_string(),
        observed_at: Utc::now(),
    }
}

async fn accept_with_reply(pool: &PgPool, event_id: &str) -> (Uuid, Uuid) {
    let store = IngressStore::new(pool.clone());
    let outcome = store
        .process(ingress_request(event_id, ALLOWED_SENDER, true), |_ctx| {
            Ok(DecisionOutput {
                effects: vec![IngressEffect::GrantConsent {
                    consent_version: "v1".to_string(),
                }],
                reply: Some(ReplyIntent {
                    body: "Chào bạn".to_string(),
                }),
            })
        })
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

fn claim_options(owner: &str) -> ClaimOptions {
    ClaimOptions {
        batch_limit: 1,
        lease_owner: owner.to_string(),
        lease_duration_secs: 30,
    }
}

#[tokio::test]
async fn ingress_reply_enqueues_outbound_and_job_atomically() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("ingress_reply_enqueues_outbound_and_job_atomically")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (outbound_id, job_id) = accept_with_reply(&pool, "evt-atomic-success").await;

    let outbound_state: String =
        sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_one(&pool)
            .await
            .expect("outbound state");
    assert_eq!(outbound_state, "queued");

    let job_type: String = sqlx::query_scalar("SELECT job_type FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .expect("job type");
    assert_eq!(job_type, "outbound.deliver");

    let serialization_key: String =
        sqlx::query_scalar("SELECT serialization_key FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&pool)
            .await
            .expect("serialization key");
    let account_id: Uuid =
        sqlx::query_scalar("SELECT account_id FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_one(&pool)
            .await
            .expect("account id");
    assert_eq!(serialization_key, format!("account:{account_id}"));
}

#[tokio::test]
async fn outbound_and_job_roll_back_when_ingress_transaction_aborts() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("outbound_and_job_roll_back_when_ingress_transaction_aborts")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let store = IngressStore::new(pool.clone());
    let expense_id = Uuid::new_v4();
    let occurred_at = Utc::now();

    store
        .process(
            ingress_request("evt-setup-rollback", ALLOWED_SENDER, true),
            move |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::CreateManualExpenseAwaitingConfirmation {
                        expense_id,
                        amount_minor: 50_000,
                        currency: "VND".to_string(),
                        description: "tea".to_string(),
                        occurred_at,
                        optimistic_version: 1,
                        pending_expires_at: occurred_at + chrono::Duration::minutes(15),
                        pending_action_type: "manual_expense_confirmation".to_string(),
                    }],
                    reply: Some(ReplyIntent {
                        body: "pending".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("setup");

    let err = store
        .process(
            ingress_request("evt-rollback", ALLOWED_SENDER, true),
            move |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::ClearPendingAction {
                        expected_version: 99,
                    }],
                    reply: Some(ReplyIntent {
                        body: "should rollback".to_string(),
                    }),
                })
            },
        )
        .await
        .expect_err("version conflict expected");
    assert!(err.to_string().contains("version conflict"));

    let outbound_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages WHERE idempotency_key = $1")
            .bind(format!("reply:{PROVIDER_SCOPE}:evt-rollback"))
            .fetch_one(&pool)
            .await
            .expect("outbound count");
    let job_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE dedupe_key = $1")
        .bind(format!(
            "outbound.deliver:reply:{PROVIDER_SCOPE}:evt-rollback"
        ))
        .fetch_one(&pool)
        .await
        .expect("job count");
    assert_eq!(outbound_count, 0);
    assert_eq!(job_count, 0);
}

#[tokio::test]
async fn duplicate_ingress_keeps_one_outbound_and_one_job() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("duplicate_ingress_keeps_one_outbound_and_one_job")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let store = IngressStore::new(pool.clone());
    let request = ingress_request("evt-dup-job", ALLOWED_SENDER, true);

    let first = store
        .process(request.clone(), |_ctx| {
            Ok(DecisionOutput {
                effects: vec![IngressEffect::GrantConsent {
                    consent_version: "v1".to_string(),
                }],
                reply: Some(ReplyIntent {
                    body: "once".to_string(),
                }),
            })
        })
        .await
        .expect("first");
    assert!(matches!(first, IngressOutcome::Accepted { .. }));

    let second = store
        .process(request, |_ctx| {
            Ok(DecisionOutput {
                effects: vec![IngressEffect::GrantConsent {
                    consent_version: "v1".to_string(),
                }],
                reply: Some(ReplyIntent {
                    body: "never".to_string(),
                }),
            })
        })
        .await
        .expect("second");
    assert!(matches!(second, IngressOutcome::Duplicate { .. }));

    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    let job_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("job count");
    assert_eq!(outbound_count, 1);
    assert_eq!(job_count, 1);
}

#[tokio::test]
async fn denied_sender_uses_provider_chat_serialization_key() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("denied_sender_uses_provider_chat_serialization_key")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let store = IngressStore::new(pool.clone());
    store
        .process(
            ingress_request("evt-denied-serial", "blocked", false),
            |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::ReadOnly],
                    reply: Some(ReplyIntent {
                        body: "denied".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("process");

    let serialization_key: String =
        sqlx::query_scalar("SELECT serialization_key FROM jobs WHERE dedupe_key = $1")
            .bind(format!(
                "outbound.deliver:reply:{PROVIDER_SCOPE}:evt-denied-serial"
            ))
            .fetch_one(&pool)
            .await
            .expect("serialization key");
    assert_eq!(
        serialization_key,
        format!("provider_chat:{PROVIDER_SCOPE}:chat-blocked")
    );
}

#[tokio::test]
async fn deliver_for_job_targets_exact_outbound_id() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("deliver_for_job_targets_exact_outbound_id")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (target_outbound_id, job_id) = accept_with_reply(&pool, "evt-exact-id").await;

    let decoy_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO outbound_messages (
            id, idempotency_key, provider_scope, provider_target, body, state
        )
        VALUES ($1, $2, $3, 'decoy-chat', 'decoy', 'queued')
        "#,
    )
    .bind(decoy_id)
    .bind(format!("reply:decoy:{}", Uuid::new_v4()))
    .bind(PROVIDER_SCOPE)
    .execute(&pool)
    .await
    .expect("seed decoy");

    let (api_base, send_count, zalo_task) = spawn_zalo_loopback().await;
    let adapter = ZaloHttpAdapter::new(ZaloHttpConfig {
        api_base,
        bot_token: "test-token".to_string(),
        webhook_secret: "secret".to_string(),
        provider_scope: PROVIDER_SCOPE.to_string(),
        request_timeout: Duration::from_secs(2),
    })
    .expect("adapter");

    let store = WorkStore::new(pool.clone());
    let claimed = store
        .claim(claim_options("worker-exact"))
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, job_id);

    let outcome = deliver_for_job(&pool, &adapter, &claimed[0])
        .await
        .expect("deliver");
    assert_eq!(
        outcome,
        OutboundJobExecution::Complete(DeliveryResult {
            outbound_id: target_outbound_id,
            state: DeliveryState::Sent,
        })
    );
    assert_eq!(send_count.load(Ordering::SeqCst), 1);

    let decoy_state: String =
        sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
            .bind(decoy_id)
            .fetch_one(&pool)
            .await
            .expect("decoy state");
    assert_eq!(decoy_state, "queued");

    zalo_task.abort();
}

#[tokio::test]
async fn deliver_for_job_refuses_stale_lease_after_http_effect() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("deliver_for_job_refuses_stale_lease_after_http_effect")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (outbound_id, job_id) = accept_with_reply(&pool, "evt-stale-lease").await;

    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    let sends = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&sends);
    let wait = Arc::clone(&release_rx);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            let wait = Arc::clone(&wait);
            async move {
                if let Some(rx) = wait.lock().await.take() {
                    let _ = rx.await;
                }
                counter.fetch_add(1, Ordering::SeqCst);
                Json(json!({ "ok": true, "result": { "message_id": "provider-1" } }))
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
    let adapter = Arc::new(
        ZaloHttpAdapter::new(ZaloHttpConfig {
            api_base,
            bot_token: "test-token".to_string(),
            webhook_secret: "secret".to_string(),
            provider_scope: PROVIDER_SCOPE.to_string(),
            request_timeout: Duration::from_secs(5),
        })
        .expect("adapter"),
    );

    let store = WorkStore::new(pool.clone());
    let claimed = store
        .claim(claim_options("worker-stale"))
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, job_id);

    let pool_bg = pool.clone();
    let adapter_bg = Arc::clone(&adapter);
    let job = claimed[0].clone();
    let deliver_task =
        tokio::spawn(async move { deliver_for_job(&pool_bg, &adapter_bg, &job).await });

    for _ in 0..100 {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
                .bind(outbound_id)
                .fetch_optional(&pool)
                .await
                .expect("state");
        if state.as_deref() == Some("sending") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    sqlx::query("UPDATE jobs SET lease_deadline = NOW() - INTERVAL '1 second' WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .expect("expire lease");
    let _ = release_tx.send(());

    let outcome = deliver_task.await.expect("join").expect("deliver");
    assert_eq!(outcome, OutboundJobExecution::StaleLease);
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    let outbound_state: String =
        sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_one(&pool)
            .await
            .expect("outbound state");
    assert_eq!(outbound_state, "sending");

    zalo_task.abort();
}

#[tokio::test]
async fn reclaimed_job_marks_sending_outbound_ambiguous_without_resend() {
    let _guard = common::integration_lock();
    let Some(database_url) = common::skip_without_database(
        "reclaimed_job_marks_sending_outbound_ambiguous_without_resend",
    ) else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (outbound_id, job_id) = accept_with_reply(&pool, "evt-reclaim").await;

    sqlx::query("UPDATE outbound_messages SET state = 'sending', attempt_count = 1 WHERE id = $1")
        .bind(outbound_id)
        .execute(&pool)
        .await
        .expect("seed sending");

    let (api_base, send_count, zalo_task) = spawn_zalo_loopback().await;
    let adapter = ZaloHttpAdapter::new(ZaloHttpConfig {
        api_base,
        bot_token: "test-token".to_string(),
        webhook_secret: "secret".to_string(),
        provider_scope: PROVIDER_SCOPE.to_string(),
        request_timeout: Duration::from_secs(2),
    })
    .expect("adapter");

    let store = WorkStore::new(pool.clone());
    sqlx::query(
        r#"
        UPDATE jobs
        SET state = 'leased',
            attempt_count = 1,
            lease_token = $2,
            lease_owner = 'crashed-worker',
            lease_deadline = NOW() - INTERVAL '1 minute'
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .expect("seed expired lease");

    let reclaimed = store
        .claim(claim_options("recovery-worker"))
        .await
        .expect("reclaim");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, job_id);

    let outcome = deliver_for_job(&pool, &adapter, &reclaimed[0])
        .await
        .expect("deliver");
    assert_eq!(
        outcome,
        OutboundJobExecution::Complete(DeliveryResult {
            outbound_id,
            state: DeliveryState::Ambiguous,
        })
    );
    assert_eq!(send_count.load(Ordering::SeqCst), 0);

    let outbound_state: String =
        sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_one(&pool)
            .await
            .expect("outbound state");
    assert_eq!(outbound_state, "ambiguous");

    zalo_task.abort();
}

#[tokio::test]
async fn terminal_sent_and_ambiguous_outbound_are_not_resent() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("terminal_sent_and_ambiguous_outbound_are_not_resent")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (sent_outbound_id, _sent_job_id) = accept_with_reply(&pool, "evt-terminal-sent").await;

    let ambiguous_outbound_id = Uuid::new_v4();
    let ambiguous_job_id = Uuid::new_v4();
    let ambiguous_dedupe =
        format!("outbound.deliver:reply:{PROVIDER_SCOPE}:evt-terminal-ambiguous");
    sqlx::query(
        r#"
        INSERT INTO outbound_messages (
            id, idempotency_key, provider_scope, provider_target, body, state
        )
        VALUES ($1, $2, $3, 'chat-ambiguous', 'ambiguous body', 'ambiguous')
        "#,
    )
    .bind(ambiguous_outbound_id)
    .bind(format!("reply:{PROVIDER_SCOPE}:evt-terminal-ambiguous"))
    .bind(PROVIDER_SCOPE)
    .execute(&pool)
    .await
    .expect("seed ambiguous outbound");
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, job_type, payload, payload_version, state, priority, run_at,
            dedupe_key, serialization_key, max_attempts
        )
        VALUES ($1, 'outbound.deliver', $2, 1, 'queued', 0, NOW(), $3, $4, 10)
        "#,
    )
    .bind(ambiguous_job_id)
    .bind(json!({
        "schema_version": 1,
        "outbound_id": ambiguous_outbound_id,
    }))
    .bind(&ambiguous_dedupe)
    .bind(format!("provider_chat:{PROVIDER_SCOPE}:chat-ambiguous"))
    .execute(&pool)
    .await
    .expect("seed ambiguous job");

    sqlx::query(
        "UPDATE outbound_messages SET state = 'sent', provider_message_id = 'done' WHERE id = $1",
    )
    .bind(sent_outbound_id)
    .execute(&pool)
    .await
    .expect("mark sent");

    let (api_base, send_count, zalo_task) = spawn_zalo_loopback().await;
    let adapter = ZaloHttpAdapter::new(ZaloHttpConfig {
        api_base,
        bot_token: "test-token".to_string(),
        webhook_secret: "secret".to_string(),
        provider_scope: PROVIDER_SCOPE.to_string(),
        request_timeout: Duration::from_secs(2),
    })
    .expect("adapter");

    let store = WorkStore::new(pool.clone());
    let claimed = store
        .claim(ClaimOptions {
            batch_limit: 2,
            lease_owner: "worker-terminal".to_string(),
            lease_duration_secs: 30,
        })
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 2);

    for job in &claimed {
        let outbound_id = job
            .payload
            .get("outbound_id")
            .and_then(|value| value.as_str())
            .and_then(|value| Uuid::parse_str(value).ok())
            .expect("payload outbound id");
        let outcome = deliver_for_job(&pool, &adapter, job)
            .await
            .expect("deliver");
        let expected_state = if outbound_id == sent_outbound_id {
            DeliveryState::Sent
        } else {
            assert_eq!(job.id, ambiguous_job_id);
            DeliveryState::Ambiguous
        };
        assert_eq!(
            outcome,
            OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: expected_state,
            })
        );
    }
    assert_eq!(send_count.load(Ordering::SeqCst), 0);
    assert!(
        deliver_next(&pool, &adapter)
            .await
            .expect("deliver_next")
            .is_none()
    );

    zalo_task.abort();
}

#[tokio::test]
async fn malformed_success_response_marks_one_ambiguous_attempt() {
    let _guard = common::integration_lock();
    let Some(database_url) =
        common::skip_without_database("malformed_success_response_marks_one_ambiguous_attempt")
    else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let (outbound_id, _job_id) = accept_with_reply(&pool, "evt-malformed").await;

    let sends = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&sends);
    let app = Router::new().route(
        "/bottest-token/sendMessage",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, "{not-json")
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

    let adapter = ZaloHttpAdapter::new(ZaloHttpConfig {
        api_base,
        bot_token: "test-token".to_string(),
        webhook_secret: "secret".to_string(),
        provider_scope: PROVIDER_SCOPE.to_string(),
        request_timeout: Duration::from_secs(2),
    })
    .expect("adapter");

    let store = WorkStore::new(pool.clone());
    let claimed = store
        .claim(claim_options("worker-malformed"))
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);

    let outcome = deliver_for_job(&pool, &adapter, &claimed[0])
        .await
        .expect("deliver");
    assert_eq!(
        outcome,
        OutboundJobExecution::Complete(DeliveryResult {
            outbound_id,
            state: DeliveryState::Ambiguous,
        })
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    let attempt_count: i32 =
        sqlx::query_scalar("SELECT attempt_count FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_one(&pool)
            .await
            .expect("attempt count");
    assert_eq!(attempt_count, 1);

    zalo_task.abort();
}

#[tokio::test]
async fn shared_transaction_enqueue_rolls_back_outbound_and_job_together() {
    let _guard = common::integration_lock();
    let Some(database_url) = common::skip_without_database(
        "shared_transaction_enqueue_rolls_back_outbound_and_job_together",
    ) else {
        return;
    };

    let pool = isolated_pool(&database_url).await;
    let outbound_id = Uuid::new_v4();
    let idempotency_key = format!("reply:{PROVIDER_SCOPE}:txn-rollback");
    let job_dedupe_key = format!("outbound.deliver:{idempotency_key}");

    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(
        r#"
        INSERT INTO outbound_messages (
            id, idempotency_key, provider_scope, provider_target, body, state
        )
        VALUES ($1, $2, $3, 'chat-txn', 'body', 'queued')
        "#,
    )
    .bind(outbound_id)
    .bind(&idempotency_key)
    .bind(PROVIDER_SCOPE)
    .execute(&mut *tx)
    .await
    .expect("insert outbound");

    assert_eq!(
        WorkStore::enqueue_in_transaction(
            &mut tx,
            EnqueueRequest {
                id: Uuid::new_v4(),
                job_type: "outbound.deliver".to_string(),
                payload: json!({
                    "schema_version": 1,
                    "outbound_id": outbound_id,
                }),
                dedupe_key: job_dedupe_key.clone(),
                serialization_key: Some(format!("provider_chat:{PROVIDER_SCOPE}:chat-txn")),
                priority: 0,
                run_at: Utc::now(),
                max_attempts: 10,
            },
        )
        .await
        .expect("enqueue job"),
        EnqueueOutcome::Enqueued
    );

    tx.rollback().await.expect("rollback");

    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    let job_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("job count");
    assert_eq!(outbound_count, 0);
    assert_eq!(job_count, 0);
}
