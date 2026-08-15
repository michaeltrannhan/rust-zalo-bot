//! PostgreSQL integration tests for ingress transactional persistence.

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::ingress::{
    DecisionContext, DecisionOutput, IngressEffect, IngressObservation, IngressOutcome,
    IngressRequest, IngressSource, IngressStore, ReplyIntent,
};

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m2_ingress_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin_pool)
        .await
        .expect("create isolated schema");
    admin_pool.close().await;

    let search_path = schema;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
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

fn base_request(event_id: &str, sender: &str) -> IngressRequest {
    IngressRequest {
        source: IngressSource::Webhook,
        provider_scope: "zalo:test-bot".to_string(),
        provider_event_id: event_id.to_string(),
        provider_sender_id: sender.to_string(),
        provider_chat_id: format!("chat-{sender}"),
        sender_allowed: true,
        user_text: "150000 ăn trưa".to_string(),
        observed_at: Utc::now(),
    }
}

#[tokio::test]
async fn first_accept_persists_event_reply_and_account() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("first_accept_persists_event_reply_and_account")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let request = base_request("evt-first", "sender-1");

    let outcome = store
        .process(request, IngressObservation::default(), |ctx| {
            assert!(ctx.account_id.is_some());
            assert_eq!(
                ctx.lifecycle_state.as_ref().map(|s| s.as_str()),
                Some("pending_consent")
            );
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
        .expect("process");

    let event_id = match outcome {
        IngressOutcome::Accepted { inbound_event_id } => inbound_event_id,
        other => panic!("expected accepted, got {other:?}"),
    };

    let processing_state: String =
        sqlx::query_scalar("SELECT processing_state FROM inbound_events WHERE id = $1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .expect("event state");
    assert_eq!(processing_state, "accepted");

    let lifecycle: String = sqlx::query_scalar(
        "SELECT lifecycle_state FROM accounts a JOIN provider_identities p ON p.account_id = a.id WHERE p.provider_sender_id = $1",
    )
    .bind("sender-1")
    .fetch_one(&pool)
    .await
    .expect("lifecycle");
    assert_eq!(lifecycle, "active");

    let outbound_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages WHERE idempotency_key = $1")
            .bind("reply:zalo:test-bot:evt-first")
            .fetch_one(&pool)
            .await
            .expect("outbound count");
    assert_eq!(outbound_count, 1);
    let provider_target: String = sqlx::query_scalar(
        "SELECT provider_target FROM outbound_messages WHERE inbound_event_id = $1",
    )
    .bind(event_id)
    .fetch_one(&pool)
    .await
    .expect("provider target");
    assert_eq!(provider_target, "chat-sender-1");
}

#[tokio::test]
async fn sequential_duplicate_short_circuits_without_callback_or_outbound() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("sequential_duplicate_short_circuits") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let request = base_request("evt-dup-seq", "sender-dup");

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_first = Arc::clone(&calls);
    let first = store
        .process(
            request.clone(),
            IngressObservation::default(),
            move |_ctx| {
                calls_first.fetch_add(1, Ordering::SeqCst);
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::ReadOnly],
                    reply: Some(ReplyIntent {
                        body: "ok".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("first");
    assert!(matches!(first, IngressOutcome::Accepted { .. }));

    let calls_second = Arc::clone(&calls);
    let second = store
        .process(request, IngressObservation::default(), move |_ctx| {
            calls_second.fetch_add(1, Ordering::SeqCst);
            Ok(DecisionOutput {
                effects: vec![IngressEffect::ReadOnly],
                reply: Some(ReplyIntent {
                    body: "should-not-send".to_string(),
                }),
            })
        })
        .await
        .expect("second");
    assert!(matches!(second, IngressOutcome::Duplicate { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    assert_eq!(outbound_count, 1);
}

#[tokio::test]
async fn concurrent_duplicate_runs_callback_once() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("concurrent_duplicate_runs_callback_once") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = Arc::new(IngressStore::new(pool.clone()));
    let request = base_request("evt-dup-concurrent", "sender-concurrent");
    let calls = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let store = Arc::clone(&store);
        let request = request.clone();
        let calls = Arc::clone(&calls);
        handles.push(tokio::spawn(async move {
            store
                .process(request, IngressObservation::default(), move |_ctx| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(DecisionOutput {
                        effects: vec![IngressEffect::ReadOnly],
                        reply: Some(ReplyIntent {
                            body: "once".to_string(),
                        }),
                    })
                })
                .await
        }));
    }

    let mut accepted = 0;
    let mut duplicate = 0;
    for handle in handles {
        match handle.await.expect("join") {
            Ok(IngressOutcome::Accepted { .. }) => accepted += 1,
            Ok(IngressOutcome::Duplicate { .. }) => duplicate += 1,
            Ok(other) => panic!("unexpected outcome {other:?}"),
            Err(err) => panic!("process failed: {err}"),
        }
    }
    assert_eq!(accepted, 1);
    assert_eq!(duplicate, 7);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    assert_eq!(outbound_count, 1);
}

#[tokio::test]
async fn mode_mismatch_records_rejected_without_callback() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("mode_mismatch_records_rejected") else {
        return;
    };

    let pool = fresh_pool().await;
    sqlx::query("UPDATE ingress_control SET mode = 'polling' WHERE id = 1")
        .execute(&pool)
        .await
        .expect("set polling mode");

    let store = IngressStore::new(pool.clone());
    let request = base_request("evt-mode", "sender-mode");
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_cb = Arc::clone(&calls);

    let outcome = store
        .process(request, IngressObservation::default(), move |_ctx| {
            calls_cb.fetch_add(1, Ordering::SeqCst);
            Ok(DecisionOutput {
                effects: vec![IngressEffect::ReadOnly],
                reply: None,
            })
        })
        .await
        .expect("process");

    assert!(matches!(outcome, IngressOutcome::ModeRejected { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let processing_state: String = sqlx::query_scalar(
        "SELECT processing_state FROM inbound_events WHERE provider_event_id = $1",
    )
    .bind("evt-mode")
    .fetch_one(&pool)
    .await
    .expect("processing state");
    assert_eq!(processing_state, "rejected");

    let identity_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM provider_identities")
        .fetch_one(&pool)
        .await
        .expect("identity count");
    assert_eq!(identity_count, 0);
}

#[tokio::test]
async fn denied_sender_does_not_create_account_or_identity() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("denied_sender_does_not_create_account") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let mut request = base_request("evt-denied", "sender-denied");
    request.sender_allowed = false;

    let outcome = store
        .process(
            request,
            IngressObservation::default(),
            |ctx: DecisionContext| {
                assert!(ctx.account_id.is_none());
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::ReadOnly],
                    reply: Some(ReplyIntent {
                        body: "Tài khoản chưa được cấp quyền.".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("process");
    assert!(matches!(outcome, IngressOutcome::Accepted { .. }));

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
    let denied_outbound: (Option<Uuid>, String) = sqlx::query_as(
        "SELECT account_id, provider_target FROM outbound_messages WHERE idempotency_key = $1",
    )
    .bind("reply:zalo:test-bot:evt-denied")
    .fetch_one(&pool)
    .await
    .expect("denied reply");
    assert_eq!(denied_outbound, (None, "chat-sender-denied".to_string()));
}

#[tokio::test]
async fn consent_and_outbound_commit_atomically() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("consent_and_outbound_commit_atomically") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let request = base_request("evt-consent", "sender-consent");

    store
        .process(request, IngressObservation::default(), |_ctx| {
            Ok(DecisionOutput {
                effects: vec![IngressEffect::GrantConsent {
                    consent_version: "2026-01".to_string(),
                }],
                reply: Some(ReplyIntent {
                    body: "Đã đồng ý".to_string(),
                }),
            })
        })
        .await
        .expect("process");

    let consent_version: String = sqlx::query_scalar(
        "SELECT consent_version FROM accounts a JOIN provider_identities p ON p.account_id = a.id WHERE p.provider_sender_id = $1",
    )
    .bind("sender-consent")
    .fetch_one(&pool)
    .await
    .expect("consent");
    assert_eq!(consent_version, "2026-01");

    let outbound_body: String =
        sqlx::query_scalar("SELECT body FROM outbound_messages WHERE idempotency_key = $1")
            .bind("reply:zalo:test-bot:evt-consent")
            .fetch_one(&pool)
            .await
            .expect("outbound");
    assert_eq!(outbound_body, "Đã đồng ý");
}

#[tokio::test]
async fn manual_draft_pending_and_confirm_flow() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("manual_draft_pending_and_confirm_flow") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let expense_id = Uuid::new_v4();

    let occurred_at = Utc::now();
    let pending_expires_at = occurred_at + chrono::Duration::minutes(15);
    store
        .process(
            base_request("evt-manual", "sender-manual"),
            IngressObservation::default(),
            move |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::CreateManualExpenseAwaitingConfirmation {
                        expense_id,
                        amount_minor: 150_000,
                        currency: "VND".to_string(),
                        description: "ăn trưa".to_string(),
                        occurred_at,
                        optimistic_version: 1,
                        pending_expires_at,
                        pending_action_type: "manual_expense_confirmation".to_string(),
                    }],
                    reply: Some(ReplyIntent {
                        body: "Xác nhận chi 150000 VND?".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("draft");

    let expense_state: String = sqlx::query_scalar("SELECT state FROM expenses WHERE id = $1")
        .bind(expense_id)
        .fetch_one(&pool)
        .await
        .expect("expense state");
    assert_eq!(expense_state, "awaiting_confirmation");

    let pending_type: String = sqlx::query_scalar(
        "SELECT pending_action_type FROM conversation_states cs JOIN provider_identities p ON p.account_id = cs.account_id WHERE p.provider_sender_id = $1",
    )
    .bind("sender-manual")
    .fetch_one(&pool)
    .await
    .expect("pending");
    assert_eq!(pending_type, "manual_expense_confirmation");
    let stored_expiry: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT expires_at FROM conversation_states cs JOIN provider_identities p ON p.account_id = cs.account_id WHERE p.provider_sender_id = $1",
    )
    .bind("sender-manual")
    .fetch_one(&pool)
    .await
    .expect("pending expiry");
    assert_eq!(stored_expiry, pending_expires_at);

    store
        .process(
            base_request("evt-confirm", "sender-manual"),
            IngressObservation::default(),
            move |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![
                        IngressEffect::ConfirmExpense {
                            expense_id,
                            expected_version: 1,
                        },
                        IngressEffect::ClearPendingAction {
                            expected_version: 1,
                        },
                    ],
                    reply: Some(ReplyIntent {
                        body: "Đã ghi".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("confirm");

    let expense_state: String = sqlx::query_scalar("SELECT state FROM expenses WHERE id = $1")
        .bind(expense_id)
        .fetch_one(&pool)
        .await
        .expect("confirmed state");
    assert_eq!(expense_state, "confirmed");

    let pending_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversation_states WHERE pending_action_type IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("pending cleared");
    assert_eq!(pending_count, 0);

    store
        .process(
            base_request("evt-after-clear", "sender-manual"),
            IngressObservation::default(),
            |ctx| {
                assert!(ctx.pending_action.is_none());
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::ReadOnly],
                    reply: Some(ReplyIntent {
                        body: "still healthy".to_string(),
                    }),
                })
            },
        )
        .await
        .expect("process after pending clear");
}

#[tokio::test]
async fn pending_version_conflict_rolls_back_entire_transaction() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("pending_version_conflict_rolls_back") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let expense_id = Uuid::new_v4();

    let occurred_at = Utc::now();
    store
        .process(
            base_request("evt-conflict-setup", "sender-conflict"),
            IngressObservation::default(),
            move |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![IngressEffect::CreateManualExpenseAwaitingConfirmation {
                        expense_id,
                        amount_minor: 45_000,
                        currency: "VND".to_string(),
                        description: "cafe".to_string(),
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
            base_request("evt-conflict", "sender-conflict"),
            IngressObservation::default(),
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
            .bind("reply:zalo:test-bot:evt-conflict")
            .fetch_one(&pool)
            .await
            .expect("outbound");
    assert_eq!(outbound_count, 0);

    let pending_type: Option<String> = sqlx::query_scalar(
        "SELECT pending_action_type FROM conversation_states cs JOIN provider_identities p ON p.account_id = cs.account_id WHERE p.provider_sender_id = $1",
    )
    .bind("sender-conflict")
    .fetch_optional(&pool)
    .await
    .expect("pending still armed");
    assert_eq!(pending_type.as_deref(), Some("manual_expense_confirmation"));
}

#[tokio::test]
async fn polling_duplicate_after_mode_switch_does_not_rerun_decision() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("polling_duplicate_after_mode_switch") else {
        return;
    };
    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let request = base_request("evt-cross-ingress", "sender-cross");
    store
        .process(request.clone(), IngressObservation::default(), |_| {
            Ok(DecisionOutput {
                effects: vec![IngressEffect::ReadOnly],
                reply: Some(ReplyIntent {
                    body: "one reply".to_string(),
                }),
            })
        })
        .await
        .expect("webhook");
    sqlx::query(
        "UPDATE ingress_control SET mode = 'polling', mode_generation = mode_generation + 1",
    )
    .execute(&pool)
    .await
    .expect("switch mode");
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_cb = Arc::clone(&calls);
    let mut polling = request;
    polling.source = IngressSource::Polling;
    let outcome = store
        .process(polling, IngressObservation::default(), move |_| {
            calls_cb.fetch_add(1, Ordering::SeqCst);
            Ok(DecisionOutput {
                effects: vec![],
                reply: None,
            })
        })
        .await
        .expect("polling duplicate");
    assert!(matches!(outcome, IngressOutcome::Duplicate { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let outbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbound_messages")
        .fetch_one(&pool)
        .await
        .expect("outbound count");
    assert_eq!(outbound_count, 1);
}

#[tokio::test]
async fn invalid_effect_rolls_back_without_partial_domain_changes() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("invalid_effect_rolls_back") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = IngressStore::new(pool.clone());
    let missing_expense = Uuid::new_v4();

    let err = store
        .process(
            base_request("evt-invalid", "sender-invalid"),
            IngressObservation::default(),
            move |_ctx| {
                Ok(DecisionOutput {
                    effects: vec![
                        IngressEffect::GrantConsent {
                            consent_version: "v1".to_string(),
                        },
                        IngressEffect::ConfirmExpense {
                            expense_id: missing_expense,
                            expected_version: 1,
                        },
                    ],
                    reply: Some(ReplyIntent {
                        body: "broken".to_string(),
                    }),
                })
            },
        )
        .await
        .expect_err("confirm missing expense");
    assert!(err.to_string().contains("version conflict"));

    let account_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .expect("accounts");
    assert_eq!(account_count, 0);

    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM inbound_events WHERE provider_event_id = $1")
            .bind("evt-invalid")
            .fetch_one(&pool)
            .await
            .expect("events");
    assert_eq!(event_count, 0);
}
