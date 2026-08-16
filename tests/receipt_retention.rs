//! Retention and early deletion integration tests.

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{Duration, Utc};
use uuid::Uuid;
use zl_expense::receipt::{
    InMemoryObjectStore, ReceiptConfig, ReceiptError, ReceiptLifecycle, ReceiptObjectStore,
    ReceiptState,
};

struct FailingDeleteObjectStore {
    inner: Arc<InMemoryObjectStore>,
    fail_delete: AtomicBool,
}

impl FailingDeleteObjectStore {
    fn new(inner: Arc<InMemoryObjectStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fail_delete: AtomicBool::new(false),
        })
    }

    fn set_fail_delete(&self, fail: bool) {
        self.fail_delete.store(fail, Ordering::SeqCst);
    }
}

impl ReceiptObjectStore for FailingDeleteObjectStore {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ReceiptError> {
        self.inner.put(key, bytes)
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ReceiptError> {
        self.inner.get(key)
    }

    fn delete(&self, key: &str) -> Result<(), ReceiptError> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(ReceiptError::dependency("simulated object delete failure"));
        }
        self.inner.delete(key)
    }
}

use common::{
    accept_and_ingest, assert_receipt_state, confirm_submission, corpus_png, drive_to_review,
    integration_lock, receipt_fresh_pool, seed_active_account, skip_without_database,
};

#[tokio::test]
async fn early_delete_removes_original_but_expense_survives() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("early_delete_removes_original_but_expense_survives")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let object_store = InMemoryObjectStore::new();
    let lifecycle =
        ReceiptLifecycle::new(pool.clone(), object_store.clone(), ReceiptConfig::default());
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(0);
    drive_to_review(&lifecycle, account_id, submission_id, &bytes).await;
    let expense_id = confirm_submission(&lifecycle, account_id, submission_id).await;

    let object_key: String =
        sqlx::query_scalar("SELECT object_key FROM receipt_assets WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("object key");
    assert!(object_store.get(&object_key).expect("get").is_some());

    lifecycle
        .delete_original(account_id, submission_id)
        .await
        .expect("delete original");
    assert!(object_store.get(&object_key).expect("get").is_none());
    assert_receipt_state(&lifecycle, account_id, submission_id, ReceiptState::Deleted).await;

    let expense_state: String = sqlx::query_scalar("SELECT state FROM expenses WHERE id = $1")
        .bind(expense_id)
        .fetch_one(&pool)
        .await
        .expect("expense state");
    assert_eq!(expense_state, "confirmed");
}

#[tokio::test]
async fn retention_sweep_deletes_expired_originals_in_batches() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("retention_sweep_deletes_expired_originals_in_batches")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let object_store = InMemoryObjectStore::new();
    let lifecycle =
        ReceiptLifecycle::new(pool.clone(), object_store.clone(), ReceiptConfig::default());

    let mut submission_ids = Vec::new();
    for index in 0..3 {
        let submission_id = Uuid::new_v4();
        drive_to_review(&lifecycle, account_id, submission_id, &corpus_png(index)).await;
        confirm_submission(&lifecycle, account_id, submission_id).await;
        submission_ids.push(submission_id);
    }

    for submission_id in &submission_ids {
        sqlx::query("UPDATE receipt_assets SET retention_deadline = $2 WHERE submission_id = $1")
            .bind(submission_id)
            .bind(Utc::now() - Duration::days(1))
            .execute(&pool)
            .await
            .expect("backdate retention");
    }

    let swept = lifecycle.retention_sweep(2).await.expect("first sweep");
    assert_eq!(swept, 2);
    let swept = lifecycle.retention_sweep(2).await.expect("second sweep");
    assert_eq!(swept, 1);

    let active_assets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM receipt_assets WHERE account_id = $1 AND deletion_state = 'active'",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("active assets");
    assert_eq!(active_assets, 0);

    let expense_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM expenses WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("expense count");
    assert_eq!(expense_count, 3);
}

#[tokio::test]
async fn delete_original_is_idempotent() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("delete_original_is_idempotent") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle =
        ReceiptLifecycle::new(pool, InMemoryObjectStore::new(), ReceiptConfig::default());
    let submission_id = Uuid::new_v4();
    drive_to_review(&lifecycle, account_id, submission_id, &corpus_png(1)).await;

    lifecycle
        .delete_original(account_id, submission_id)
        .await
        .expect("first delete");
    lifecycle
        .delete_original(account_id, submission_id)
        .await
        .expect("second delete");
    assert_receipt_state(&lifecycle, account_id, submission_id, ReceiptState::Deleted).await;
}

#[tokio::test]
async fn retention_deadline_uses_account_preference_not_global_default() {
    let _guard = integration_lock();
    let Some(_) =
        skip_without_database("retention_deadline_uses_account_preference_not_global_default")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = common::seed_active_account_with_retention(&pool, 3).await;
    let lifecycle = ReceiptLifecycle::new(
        pool.clone(),
        InMemoryObjectStore::new(),
        ReceiptConfig {
            original_receipt_days: 7,
            review_expiry_hours: 72,
            ..ReceiptConfig::default()
        },
    );
    let submission_id = Uuid::new_v4();
    accept_and_ingest(&lifecycle, account_id, submission_id, &corpus_png(0)).await;

    let matches_account: bool = sqlx::query_scalar(
        r#"
        SELECT retention_deadline = created_at + INTERVAL '3 days'
        FROM receipt_assets
        WHERE submission_id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_one(&pool)
    .await
    .expect("retention deadline");
    assert!(matches_account);
}

#[tokio::test]
async fn retention_sweep_skips_in_flight_assets() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("retention_sweep_skips_in_flight_assets") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let object_store = InMemoryObjectStore::new();
    let lifecycle =
        ReceiptLifecycle::new(pool.clone(), object_store.clone(), ReceiptConfig::default());
    let stored_id = Uuid::new_v4();
    accept_and_ingest(&lifecycle, account_id, stored_id, &corpus_png(0)).await;
    let review_id = Uuid::new_v4();
    drive_to_review(&lifecycle, account_id, review_id, &corpus_png(1)).await;

    for submission_id in [stored_id, review_id] {
        sqlx::query("UPDATE receipt_assets SET retention_deadline = $2 WHERE submission_id = $1")
            .bind(submission_id)
            .bind(Utc::now() - Duration::days(1))
            .execute(&pool)
            .await
            .expect("backdate retention");
    }

    let swept = lifecycle.retention_sweep(10).await.expect("sweep");
    assert_eq!(swept, 0);
    assert_eq!(object_store.stored_object_count(), 2);
    assert_receipt_state(&lifecycle, account_id, stored_id, ReceiptState::Stored).await;
    assert_receipt_state(
        &lifecycle,
        account_id,
        review_id,
        ReceiptState::ReviewRequired,
    )
    .await;
}

#[tokio::test]
async fn retention_sweep_rolls_back_when_object_delete_fails() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("retention_sweep_rolls_back_when_object_delete_fails")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let inner_store = InMemoryObjectStore::new();
    let object_store = FailingDeleteObjectStore::new(inner_store.clone());
    let lifecycle =
        ReceiptLifecycle::new(pool.clone(), object_store.clone(), ReceiptConfig::default());
    let submission_id = Uuid::new_v4();
    drive_to_review(&lifecycle, account_id, submission_id, &corpus_png(0)).await;
    confirm_submission(&lifecycle, account_id, submission_id).await;

    sqlx::query("UPDATE receipt_assets SET retention_deadline = $2 WHERE submission_id = $1")
        .bind(submission_id)
        .bind(Utc::now() - Duration::days(1))
        .execute(&pool)
        .await
        .expect("backdate retention");

    object_store.set_fail_delete(true);
    lifecycle
        .retention_sweep(10)
        .await
        .expect_err("sweep should fail when object delete fails");

    let deletion_state: String =
        sqlx::query_scalar("SELECT deletion_state FROM receipt_assets WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("deletion state");
    assert_eq!(deletion_state, "active");

    object_store.set_fail_delete(false);
    let swept = lifecycle.retention_sweep(10).await.expect("retry sweep");
    assert_eq!(swept, 1);

    let deletion_state: String =
        sqlx::query_scalar("SELECT deletion_state FROM receipt_assets WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("deletion state after retry");
    assert_eq!(deletion_state, "deleted");
}

#[tokio::test]
async fn concurrent_retention_sweeps_do_not_double_delete() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("concurrent_retention_sweeps_do_not_double_delete") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let object_store = InMemoryObjectStore::new();
    let lifecycle = Arc::new(ReceiptLifecycle::new(
        pool.clone(),
        object_store.clone(),
        ReceiptConfig::default(),
    ));

    for index in 0..3 {
        let submission_id = Uuid::new_v4();
        drive_to_review(&lifecycle, account_id, submission_id, &corpus_png(index)).await;
        confirm_submission(&lifecycle, account_id, submission_id).await;
        sqlx::query("UPDATE receipt_assets SET retention_deadline = $2 WHERE submission_id = $1")
            .bind(submission_id)
            .bind(Utc::now() - Duration::days(1))
            .execute(&pool)
            .await
            .expect("backdate retention");
    }

    let barrier = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..3 {
        let lifecycle = Arc::clone(&lifecycle);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.fetch_add(1, Ordering::SeqCst);
            while barrier.load(Ordering::SeqCst) < 3 {
                tokio::task::yield_now().await;
            }
            lifecycle.retention_sweep(10).await
        }));
    }

    let mut swept = 0_usize;
    for handle in handles {
        swept += handle.await.expect("join").expect("sweep");
    }
    assert_eq!(swept, 3);
    assert_eq!(object_store.stored_object_count(), 0);

    let active_assets: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM receipt_assets WHERE account_id = $1 AND deletion_state = 'active'",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("active assets");
    assert_eq!(active_assets, 0);

    let expense_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM expenses WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&pool)
            .await
            .expect("expense count");
    assert_eq!(expense_count, 3);
}
