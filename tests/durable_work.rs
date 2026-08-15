//! PostgreSQL integration tests for the durable work engine.

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use zl_expense::db::MIGRATOR;
use zl_expense::error::ErrorClass;
use zl_expense::work::{
    AttemptOutcome, ClaimOptions, EnqueueOutcome, EnqueueRequest, FailOutcome, JobState, WorkError,
    WorkStore,
};

async fn fresh_pool() -> PgPool {
    let url = common::test_database_url().expect("TEST_DATABASE_URL must be set");
    let admin_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test database");
    let schema = format!("m3_work_{}", Uuid::new_v4().simple());
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

fn sample_payload(label: &str) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "label": label
    })
}

fn enqueue_request(
    dedupe_key: &str,
    serialization_key: Option<&str>,
    priority: i32,
    run_at_offset_secs: i64,
) -> EnqueueRequest {
    EnqueueRequest {
        id: Uuid::new_v4(),
        job_type: "test.echo".to_string(),
        payload: sample_payload(dedupe_key),
        dedupe_key: dedupe_key.to_string(),
        serialization_key: serialization_key.map(str::to_string),
        priority,
        run_at: Utc::now() + ChronoDuration::seconds(run_at_offset_secs),
        max_attempts: 5,
    }
}

fn claim_options(batch_limit: i32, owner: &str) -> ClaimOptions {
    ClaimOptions {
        batch_limit,
        lease_owner: owner.to_string(),
        lease_duration_secs: 30,
    }
}

#[tokio::test]
async fn enqueue_requires_versioned_payload_and_dedupes() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("enqueue_requires_versioned_payload_and_dedupes")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool);

    let invalid = EnqueueRequest {
        payload: json!({"label": "missing version"}),
        ..enqueue_request("dedupe-1", None, 0, 0)
    };
    let error = store
        .enqueue(invalid)
        .await
        .expect_err("missing schema_version");
    assert_eq!(error.class, ErrorClass::Validation);

    let request = enqueue_request("dedupe-1", None, 0, 0);
    let job_id = request.id;
    assert_eq!(
        store.enqueue(request.clone()).await.expect("enqueue"),
        EnqueueOutcome::Enqueued
    );
    assert_eq!(
        store.enqueue(request).await.expect("duplicate enqueue"),
        EnqueueOutcome::Duplicate
    );

    let summary = store.get_job_summary(job_id).await.expect("summary");
    assert_eq!(summary.payload_version, 1);
    assert_eq!(summary.state, JobState::Queued);
}

#[test]
fn durable_work_debug_output_redacts_payload_and_keys() {
    let request = enqueue_request("private-dedupe", Some("account:private"), 0, 0);
    let rendered = format!("{request:?}");
    assert!(!rendered.contains("private-dedupe"));
    assert!(!rendered.contains("account:private"));
}

#[tokio::test]
async fn enqueue_can_share_the_callers_transaction_and_roll_back() {
    let _guard = common::integration_lock();
    let Some(_) =
        common::skip_without_database("enqueue_can_share_the_callers_transaction_and_roll_back")
    else {
        return;
    };
    let pool = fresh_pool().await;
    let request = enqueue_request("transactional", None, 0, 0);
    let job_id = request.id;
    let mut tx = pool.begin().await.expect("begin");
    assert_eq!(
        WorkStore::enqueue_in_transaction(&mut tx, request)
            .await
            .expect("enqueue in transaction"),
        EnqueueOutcome::Enqueued
    );
    tx.rollback().await.expect("rollback");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn claim_orders_by_priority_and_run_at_with_bounded_batch() {
    let _guard = common::integration_lock();
    let Some(_) =
        common::skip_without_database("claim_orders_by_priority_and_run_at_with_bounded_batch")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool);

    store
        .enqueue(enqueue_request("low", None, 0, 0))
        .await
        .expect("enqueue low");
    store
        .enqueue(enqueue_request("high", None, 10, 0))
        .await
        .expect("enqueue high");
    store
        .enqueue(enqueue_request("soon", None, 10, 0))
        .await
        .expect("enqueue soon");

    let claimed = store
        .claim(claim_options(2, "worker-a"))
        .await
        .expect("claim batch");
    assert_eq!(claimed.len(), 2);
    assert_eq!(claimed[0].dedupe_key, "high");
    assert_eq!(claimed[1].dedupe_key, "soon");

    let remaining = store
        .claim(claim_options(5, "worker-a"))
        .await
        .expect("claim remaining");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].dedupe_key, "low");
}

#[tokio::test]
async fn concurrent_claimers_do_not_double_claim_same_job() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("concurrent_claimers_do_not_double_claim_same_job")
    else {
        return;
    };

    let pool = Arc::new(fresh_pool().await);
    let store = WorkStore::new((*pool).clone());
    store
        .enqueue(enqueue_request("single-job", None, 0, 0))
        .await
        .expect("enqueue");

    let successes = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for idx in 0..8 {
        let pool = Arc::clone(&pool);
        let successes = Arc::clone(&successes);
        handles.push(tokio::spawn(async move {
            let store = WorkStore::new((*pool).clone());
            let claimed = store
                .claim(claim_options(1, &format!("worker-{idx}")))
                .await
                .expect("claim");
            if !claimed.is_empty() {
                successes.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for handle in handles {
        handle.await.expect("join");
    }

    assert_eq!(successes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn serialization_key_allows_one_active_job_with_concurrent_claimers() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database(
        "serialization_key_allows_one_active_job_with_concurrent_claimers",
    ) else {
        return;
    };

    let pool = Arc::new(fresh_pool().await);
    let store = WorkStore::new((*pool).clone());

    store
        .enqueue(enqueue_request("job-a", Some("account:1"), 0, 0))
        .await
        .expect("enqueue first");
    store
        .enqueue(enqueue_request("job-b", Some("account:1"), 0, 0))
        .await
        .expect("queue second serialized job");
    store
        .enqueue(enqueue_request("job-c", Some("account:1"), 0, 0))
        .await
        .expect("queue third serialized job");

    let claimed = store
        .claim(claim_options(10, "worker-a"))
        .await
        .expect("claim first");
    assert_eq!(claimed.len(), 1);

    let still_blocked = store
        .claim(claim_options(10, "worker-b"))
        .await
        .expect("serialized claim blocked");
    assert!(still_blocked.is_empty());

    store
        .complete(claimed[0].id, claimed[0].lease_token)
        .await
        .expect("complete first");

    let successes = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for idx in 0..6 {
        let pool = Arc::clone(&pool);
        let successes = Arc::clone(&successes);
        handles.push(tokio::spawn(async move {
            let store = WorkStore::new((*pool).clone());
            let claimed = store
                .claim(claim_options(1, &format!("worker-{idx}")))
                .await
                .expect("claim");
            if !claimed.is_empty() {
                successes.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for handle in handles {
        handle.await.expect("join");
    }
    assert_eq!(successes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn heartbeat_only_accepts_current_lease_token() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("heartbeat_only_accepts_current_lease_token")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool);
    store
        .enqueue(enqueue_request("heartbeat", None, 0, 0))
        .await
        .expect("enqueue");

    let claimed = store
        .claim(claim_options(1, "worker-a"))
        .await
        .expect("claim");
    let job = &claimed[0];

    let extended = store
        .heartbeat(job.id, job.lease_token, 60)
        .await
        .expect("heartbeat");
    assert!(extended > job.lease_deadline);

    let stale = store
        .heartbeat(job.id, Uuid::new_v4(), 60)
        .await
        .expect_err("stale heartbeat");
    assert_eq!(stale.class, ErrorClass::Conflict);
}

#[tokio::test]
async fn stale_or_expired_lease_cannot_complete_fail_or_cancel() {
    let _guard = common::integration_lock();
    let Some(_) =
        common::skip_without_database("stale_or_expired_lease_cannot_complete_fail_or_cancel")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool.clone());
    let request = enqueue_request("stale-fencing", None, 0, 0);
    let job_id = request.id;
    store.enqueue(request).await.expect("enqueue");

    let first = store
        .claim(claim_options(1, "worker-old"))
        .await
        .expect("claim")[0]
        .clone();

    sqlx::query("UPDATE jobs SET lease_deadline = NOW() - INTERVAL '1 second' WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await
        .expect("expire lease");

    let complete_err = store.complete(job_id, first.lease_token).await;
    assert_eq!(complete_err, Err(stale_conflict("complete")));

    let fail_err = store
        .fail(job_id, first.lease_token, ErrorClass::Transient)
        .await;
    assert_eq!(fail_err, Err(stale_conflict("fail")));

    let cancel_err = store.cancel(job_id, Some(first.lease_token)).await;
    assert_eq!(cancel_err, Err(stale_conflict("cancel")));

    let second = store
        .claim(claim_options(1, "worker-new"))
        .await
        .expect("reclaim")[0]
        .clone();
    assert_ne!(second.lease_token, first.lease_token);
    assert_eq!(second.attempt_number, 2);

    assert_eq!(
        store.complete(job_id, first.lease_token).await,
        Err(stale_conflict("complete"))
    );

    store
        .complete(job_id, second.lease_token)
        .await
        .expect("complete with current token");

    let attempts = store.list_attempts(job_id).await.expect("attempts");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, Some(AttemptOutcome::LostLease));
    assert_eq!(attempts[1].outcome, Some(AttemptOutcome::Completed));
}

#[tokio::test]
async fn classified_retry_and_dead_letter_paths() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("classified_retry_and_dead_letter_paths") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool);

    let mut retryable = enqueue_request("retryable", None, 0, 0);
    retryable.max_attempts = 3;
    let retry_job_id = retryable.id;
    store.enqueue(retryable).await.expect("enqueue retryable");

    let claimed = store
        .claim(claim_options(1, "worker-a"))
        .await
        .expect("claim retryable")[0]
        .clone();
    assert_eq!(
        store
            .fail(claimed.id, claimed.lease_token, ErrorClass::Transient)
            .await
            .expect("retryable fail"),
        FailOutcome::Retried
    );

    let summary = store
        .get_job_summary(retry_job_id)
        .await
        .expect("retry summary");
    assert_eq!(summary.state, JobState::Queued);
    assert!(summary.run_at > Utc::now());
    assert_eq!(summary.last_error_class.as_deref(), Some("transient"));

    let mut terminal = enqueue_request("terminal", None, 0, 0);
    terminal.max_attempts = 1;
    let terminal_id = terminal.id;
    store.enqueue(terminal).await.expect("enqueue terminal");

    let claimed = store
        .claim(claim_options(1, "worker-a"))
        .await
        .expect("claim terminal")[0]
        .clone();
    assert_eq!(
        store
            .fail(claimed.id, claimed.lease_token, ErrorClass::Validation)
            .await
            .expect("terminal fail"),
        FailOutcome::DeadLettered
    );

    let dead = store
        .get_job_summary(terminal_id)
        .await
        .expect("dead summary");
    assert_eq!(dead.state, JobState::Dead);
}

#[tokio::test]
async fn cancellation_and_dead_recovery() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("cancellation_and_dead_recovery") else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool);

    let queued = enqueue_request("cancel-queued", None, 0, 0);
    let queued_id = queued.id;
    store.enqueue(queued).await.expect("enqueue queued");
    store.cancel(queued_id, None).await.expect("cancel queued");
    let summary = store
        .get_job_summary(queued_id)
        .await
        .expect("queued summary");
    assert_eq!(summary.state, JobState::Cancelled);

    let leased = enqueue_request("cancel-leased", None, 0, 0);
    let leased_id = leased.id;
    store.enqueue(leased).await.expect("enqueue leased");
    let claimed = store
        .claim(claim_options(1, "worker-a"))
        .await
        .expect("claim leased")[0]
        .clone();
    store
        .cancel(leased_id, Some(claimed.lease_token))
        .await
        .expect("cancel leased");
    let summary = store
        .get_job_summary(leased_id)
        .await
        .expect("leased summary");
    assert_eq!(summary.state, JobState::Cancelled);

    let mut dead = enqueue_request("recover-dead", None, 0, 0);
    dead.max_attempts = 1;
    let dead_id = dead.id;
    store.enqueue(dead).await.expect("enqueue dead");
    let claimed = store
        .claim(claim_options(1, "worker-a"))
        .await
        .expect("claim dead")[0]
        .clone();
    store
        .fail(claimed.id, claimed.lease_token, ErrorClass::Internal)
        .await
        .expect("dead letter");
    assert_eq!(
        store.get_job_summary(dead_id).await.expect("dead").state,
        JobState::Dead
    );

    store.recover_dead(dead_id).await.expect("recover");
    let recovered = store
        .get_job_summary(dead_id)
        .await
        .expect("recovered summary");
    assert_eq!(recovered.state, JobState::Queued);
    assert_eq!(recovered.attempt_count, 1);

    let reclaimed = store
        .claim(claim_options(1, "worker-after-recovery"))
        .await
        .expect("claim recovered job");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].attempt_number, 2);
    store
        .complete(reclaimed[0].id, reclaimed[0].lease_token)
        .await
        .expect("complete recovered job");
}

#[tokio::test]
async fn job_attempt_audit_contains_no_payload_column() {
    let _guard = common::integration_lock();
    let Some(_) = common::skip_without_database("job_attempt_audit_contains_no_payload_column")
    else {
        return;
    };

    let pool = fresh_pool().await;
    let store = WorkStore::new(pool.clone());

    let request = enqueue_request("audit", None, 0, 0);
    let job_id = request.id;
    store.enqueue(request).await.expect("enqueue");
    let claimed = store
        .claim(claim_options(1, "worker-a"))
        .await
        .expect("claim")[0]
        .clone();
    store
        .complete(claimed.id, claimed.lease_token)
        .await
        .expect("complete");

    let columns: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT column_name
        FROM information_schema.columns
        WHERE table_schema = current_schema()
          AND table_name = 'job_attempts'
        ORDER BY column_name
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("columns");
    assert!(!columns.iter().any(|name| name.contains("payload")));

    let attempts = store.list_attempts(job_id).await.expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].lease_owner, "worker-a");
}

fn stale_conflict(operation: &str) -> WorkError {
    WorkError::new(
        ErrorClass::Conflict,
        format!("{operation} rejected for stale or expired lease token"),
    )
}
