//! PostgreSQL integration tests for receipt lifecycle.

#![allow(clippy::await_holding_lock)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, Utc};
use uuid::Uuid;
use zl_expense::error::ErrorClass;
use zl_expense::receipt::{
    AcceptSubmissionOutcome, AcceptSubmissionRequest, ConfirmOutcome, ConfirmRequest,
    EditDraftRequest, ExtractOutcome, FakeExtractor, InMemoryObjectStore, IngestOutcome,
    MAX_IMAGE_BYTES, ReceiptConfig, ReceiptError, ReceiptExtractor, ReceiptLifecycle,
    ReceiptObjectStore, ReceiptState, RejectRequest, validate_image,
};
use zl_expense::work::WorkStore;

use common::{
    accept_and_ingest, assert_receipt_state, confirm_submission, corpus_png, drive_to_review,
    expected_extraction, integration_lock, receipt_fresh_pool, receipt_lifecycle,
    seed_active_account, seed_inbound_event, skip_without_database,
};

#[tokio::test]
async fn happy_path_confirm_creates_receipt_expense() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("happy_path_confirm_creates_receipt_expense") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(0);

    drive_to_review(&lifecycle, account_id, submission_id, &bytes).await;
    assert_receipt_state(
        &lifecycle,
        account_id,
        submission_id,
        ReceiptState::ReviewRequired,
    )
    .await;

    let draft = lifecycle
        .get_draft(account_id, submission_id)
        .await
        .expect("draft");
    let expected = expected_extraction(&bytes);
    assert_eq!(draft.merchant, expected.merchant);
    assert_eq!(draft.amount_minor, expected.amount_minor);

    let expense_id = confirm_submission(&lifecycle, account_id, submission_id).await;
    assert_receipt_state(
        &lifecycle,
        account_id,
        submission_id,
        ReceiptState::Confirmed,
    )
    .await;

    let expense_state: String = sqlx::query_scalar("SELECT state FROM expenses WHERE id = $1")
        .bind(expense_id)
        .fetch_one(lifecycle.pool())
        .await
        .expect("expense state");
    assert_eq!(expense_state, "confirmed");
}

#[tokio::test]
async fn duplicate_submission_is_absorbed_without_second_expense() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("duplicate_submission_is_absorbed_without_second_expense")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let bytes = corpus_png(1);
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();

    drive_to_review(&lifecycle, account_id, first, &bytes).await;
    confirm_submission(&lifecycle, account_id, first).await;

    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id: second,
            account_id,
            inbound_event_id: None,
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept duplicate");
    let outcome = lifecycle
        .ingest(account_id, second, &bytes, "image/png", Uuid::new_v4())
        .await
        .expect("ingest duplicate");
    assert!(matches!(outcome, IngestOutcome::DuplicateAbsorbed { .. }));
    assert_receipt_state(
        &lifecycle,
        account_id,
        second,
        ReceiptState::FailedPermanent,
    )
    .await;

    let expense_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM expenses WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("expense count");
    assert_eq!(expense_count, 1);
}

#[tokio::test]
async fn concurrent_ingest_replays_are_idempotent() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("concurrent_ingest_replays_are_idempotent") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = Arc::new(receipt_lifecycle(pool));
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(2);

    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id,
            account_id,
            inbound_event_id: None,
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept");

    let barrier = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let lifecycle = Arc::clone(&lifecycle);
        let bytes = bytes.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.fetch_add(1, Ordering::SeqCst);
            while barrier.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
            lifecycle
                .ingest(
                    account_id,
                    submission_id,
                    &bytes,
                    "image/png",
                    Uuid::new_v4(),
                )
                .await
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("join"));
    }
    assert!(results.iter().all(|result| result.is_ok()));
    assert_receipt_state(&lifecycle, account_id, submission_id, ReceiptState::Stored).await;

    let asset_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receipt_assets WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("asset count");
    assert_eq!(asset_count, 1);
}

#[tokio::test]
async fn validation_rejects_oversize_invalid_mime_and_pixels() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("validation_rejects_oversize_invalid_mime_and_pixels")
    else {
        return;
    };

    let oversize = vec![0_u8; MAX_IMAGE_BYTES + 1];
    let error = validate_image(&oversize, "image/png").expect_err("oversize");
    assert_eq!(error.class, ErrorClass::Validation);

    let error = validate_image(&[1, 2, 3], "text/plain").expect_err("mime");
    assert_eq!(error.class, ErrorClass::Unsupported);

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id,
            account_id,
            inbound_event_id: None,
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept");

    let error = lifecycle
        .ingest(
            account_id,
            submission_id,
            &oversize,
            "image/png",
            Uuid::new_v4(),
        )
        .await
        .expect_err("ingest oversize");
    assert_eq!(error.class, ErrorClass::Validation);
    assert_receipt_state(
        &lifecycle,
        account_id,
        submission_id,
        ReceiptState::FailedPermanent,
    )
    .await;
}

#[tokio::test]
async fn extract_replay_is_idempotent() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("extract_replay_is_idempotent") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(0);
    accept_and_ingest(&lifecycle, account_id, submission_id, &bytes).await;

    let first = lifecycle
        .extract(account_id, submission_id)
        .await
        .expect("first extract");
    assert!(matches!(first, ExtractOutcome::ReviewRequired { .. }));

    let second = lifecycle
        .extract(account_id, submission_id)
        .await
        .expect("second extract");
    assert!(matches!(
        second,
        ExtractOutcome::AlreadyReviewRequired { .. }
    ));

    let attempt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM extraction_attempts WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("attempt count");
    assert_eq!(attempt_count, 1);
}

#[tokio::test]
async fn edit_draft_records_corrections_and_rejects_version_conflict() {
    let _guard = integration_lock();
    let Some(_) =
        skip_without_database("edit_draft_records_corrections_and_rejects_version_conflict")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(0);
    drive_to_review(&lifecycle, account_id, submission_id, &bytes).await;

    let draft = lifecycle
        .get_draft(account_id, submission_id)
        .await
        .expect("draft");
    let updated = lifecycle
        .edit_draft(EditDraftRequest {
            account_id,
            submission_id,
            expected_version: draft.version,
            amount_minor: Some(999_000),
            currency: None,
            merchant: Some("Edited Merchant".to_string()),
            category_key: Some("khac".to_string()),
            occurred_at: None,
        })
        .await
        .expect("edit");
    assert_eq!(updated.amount_minor, 999_000);
    assert_eq!(updated.version, draft.version + 1);

    let correction_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_corrections WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("correction count");
    assert_eq!(correction_count, 3);

    let unchanged = lifecycle
        .edit_draft(EditDraftRequest {
            account_id,
            submission_id,
            expected_version: updated.version,
            amount_minor: Some(999_000),
            currency: None,
            merchant: Some("Edited Merchant".to_string()),
            category_key: Some("khac".to_string()),
            occurred_at: None,
        })
        .await
        .expect("unchanged edit");
    assert_eq!(unchanged.version, updated.version + 1);
    let correction_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM draft_corrections WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("correction count after no-op fields");
    assert_eq!(correction_count, 3);

    let error = lifecycle
        .edit_draft(EditDraftRequest {
            account_id,
            submission_id,
            expected_version: unchanged.version,
            amount_minor: Some(0),
            currency: None,
            merchant: None,
            category_key: None,
            occurred_at: None,
        })
        .await
        .expect_err("invalid amount");
    assert_eq!(error.class, ErrorClass::Validation);

    let error = lifecycle
        .edit_draft(EditDraftRequest {
            account_id,
            submission_id,
            expected_version: draft.version,
            amount_minor: Some(1),
            currency: None,
            merchant: None,
            category_key: None,
            occurred_at: None,
        })
        .await
        .expect_err("stale version");
    assert_eq!(error.class, ErrorClass::Conflict);
}

#[tokio::test]
async fn confirm_reject_and_expire_review() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("confirm_reject_and_expire_review") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool.clone());
    let reject_id = Uuid::new_v4();
    drive_to_review(&lifecycle, account_id, reject_id, &corpus_png(1)).await;
    let draft = lifecycle
        .get_draft(account_id, reject_id)
        .await
        .expect("draft");
    lifecycle
        .reject(RejectRequest {
            account_id,
            submission_id: reject_id,
            expected_draft_version: draft.version,
        })
        .await
        .expect("reject");
    assert_receipt_state(&lifecycle, account_id, reject_id, ReceiptState::Rejected).await;

    let expire_id = Uuid::new_v4();
    drive_to_review(&lifecycle, account_id, expire_id, &corpus_png(2)).await;
    sqlx::query("UPDATE receipt_submissions SET review_expires_at = $2 WHERE id = $1")
        .bind(expire_id)
        .bind(Utc::now() - Duration::hours(1))
        .execute(&pool)
        .await
        .expect("backdate review expiry");
    let expired = lifecycle.expire_reviews(10).await.expect("expire");
    assert_eq!(expired, 1);
    assert_receipt_state(&lifecycle, account_id, expire_id, ReceiptState::Expired).await;
}

#[tokio::test]
async fn confirm_replay_returns_already_confirmed() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("confirm_replay_returns_already_confirmed") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    drive_to_review(&lifecycle, account_id, submission_id, &corpus_png(0)).await;
    let draft = lifecycle
        .get_draft(account_id, submission_id)
        .await
        .expect("draft");
    let expense_id = Uuid::new_v4();
    let first = lifecycle
        .confirm(ConfirmRequest {
            account_id,
            submission_id,
            expected_draft_version: draft.version,
            expense_id,
        })
        .await
        .expect("confirm");
    assert!(matches!(first, ConfirmOutcome::Confirmed { .. }));

    let second = lifecycle
        .confirm(ConfirmRequest {
            account_id,
            submission_id,
            expected_draft_version: draft.version,
            expense_id: Uuid::new_v4(),
        })
        .await
        .expect("confirm replay");
    assert!(matches!(second, ConfirmOutcome::AlreadyConfirmed { .. }));
}

#[tokio::test]
async fn ingest_and_extract_enqueue_versioned_jobs() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("ingest_and_extract_enqueue_versioned_jobs") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool.clone());
    let work = WorkStore::new(pool);
    let submission_id = Uuid::new_v4();
    let ingest_job_id = Uuid::new_v4();
    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id,
            account_id,
            inbound_event_id: None,
            ingest_job_id,
        })
        .await
        .expect("accept");

    let ingest_summary = work
        .get_job_summary(ingest_job_id)
        .await
        .expect("ingest job");
    assert_eq!(ingest_summary.job_type, "receipt.ingest");
    assert_eq!(ingest_summary.payload_version, 1);
    assert_eq!(
        ingest_summary.serialization_key.as_deref(),
        Some(format!("account:{account_id}").as_str())
    );

    let extract_job_id = Uuid::new_v4();
    lifecycle
        .ingest(
            account_id,
            submission_id,
            &corpus_png(0),
            "image/png",
            extract_job_id,
        )
        .await
        .expect("ingest");
    let extract_summary = work
        .get_job_summary(extract_job_id)
        .await
        .expect("extract job");
    assert_eq!(extract_summary.job_type, "receipt.extract");
    assert_eq!(extract_summary.payload_version, 1);
    assert_eq!(extract_summary.state, zl_expense::work::JobState::Queued);
}

#[tokio::test]
async fn unsupported_fixture_marks_permanent_failure() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("unsupported_fixture_marks_permanent_failure") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(4);
    accept_and_ingest(&lifecycle, account_id, submission_id, &bytes).await;
    let outcome = lifecycle
        .extract(account_id, submission_id)
        .await
        .expect("extract");
    assert!(matches!(outcome, ExtractOutcome::Unsupported));
    assert_receipt_state(
        &lifecycle,
        account_id,
        submission_id,
        ReceiptState::FailedPermanent,
    )
    .await;
}

struct ForcedFailureExtractor {
    error: ReceiptError,
}

impl ReceiptExtractor for ForcedFailureExtractor {
    fn extract(
        &self,
        _bytes: &[u8],
    ) -> Result<zl_expense::receipt::ExtractedAttempt, ReceiptError> {
        Err(self.error.clone())
    }

    fn meta(&self) -> zl_expense::receipt::ExtractionMeta {
        zl_expense::receipt::ExtractionMeta::fake()
    }
}

#[tokio::test]
async fn inbound_replay_is_idempotent_including_concurrent_accepts() {
    let _guard = integration_lock();
    let Some(_) =
        skip_without_database("inbound_replay_is_idempotent_including_concurrent_accepts")
    else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let inbound_event_id = seed_inbound_event(&pool, account_id).await;
    let lifecycle = Arc::new(receipt_lifecycle(pool.clone()));
    let first_submission = Uuid::new_v4();

    let first = lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id: first_submission,
            account_id,
            inbound_event_id: Some(inbound_event_id),
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept");
    assert!(matches!(first, AcceptSubmissionOutcome::Accepted { .. }));

    let replay = lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id: Uuid::new_v4(),
            account_id,
            inbound_event_id: Some(inbound_event_id),
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("replay");
    assert!(matches!(
        replay,
        AcceptSubmissionOutcome::Replayed { submission_id, .. }
            if submission_id == first_submission
    ));

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receipt_submissions WHERE inbound_event_id = $1")
            .bind(inbound_event_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("submission count");
    assert_eq!(count, 1);

    let concurrent_event = seed_inbound_event(&pool, account_id).await;
    let barrier = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let lifecycle = Arc::clone(&lifecycle);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.fetch_add(1, Ordering::SeqCst);
            while barrier.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
            lifecycle
                .accept_submission(AcceptSubmissionRequest {
                    submission_id: Uuid::new_v4(),
                    account_id,
                    inbound_event_id: Some(concurrent_event),
                    ingest_job_id: Uuid::new_v4(),
                })
                .await
        }));
    }
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("join"));
    }
    assert!(
        results.iter().all(|result| result.is_ok()),
        "concurrent inbound replay must not fail: {results:?}"
    );
    let concurrent_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receipt_submissions WHERE inbound_event_id = $1")
            .bind(concurrent_event)
            .fetch_one(lifecycle.pool())
            .await
            .expect("concurrent submission count");
    assert_eq!(concurrent_count, 1);
}

#[tokio::test]
async fn cross_account_ownership_mismatch_is_rejected() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("cross_account_ownership_mismatch_is_rejected") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_a = seed_active_account(&pool).await;
    let account_b = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool.clone());
    let inbound_event_id = seed_inbound_event(&pool, account_a).await;
    let submission_id = Uuid::new_v4();
    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id,
            account_id: account_a,
            inbound_event_id: Some(inbound_event_id),
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept");

    let error = lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id: Uuid::new_v4(),
            account_id: account_b,
            inbound_event_id: Some(inbound_event_id),
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect_err("cross-account inbound replay");
    assert_eq!(error.class, ErrorClass::Conflict);

    let mismatch = sqlx::query(
        r#"
        INSERT INTO receipt_assets (
            id, submission_id, account_id, object_key, content_sha256, mime_type,
            size_bytes, width_px, height_px, pixel_count, retention_deadline
        )
        VALUES (
            $1, $2, $3, 'mismatch-key', repeat('a', 64), 'image/png',
            8, 8, 8, 64, NOW() + INTERVAL '1 day'
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(submission_id)
    .bind(account_b)
    .execute(&pool)
    .await;
    assert!(mismatch.is_err(), "cross-account asset insert must fail");

    let error = lifecycle
        .get_state(account_b, submission_id)
        .await
        .expect_err("cross-account state");
    assert_eq!(error.class, ErrorClass::NotFound);
}

#[tokio::test]
async fn concurrent_distinct_submissions_same_hash_absorb_without_orphans() {
    let _guard = integration_lock();
    let Some(_) =
        skip_without_database("concurrent_distinct_submissions_same_hash_absorb_without_orphans")
    else {
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
    let bytes = corpus_png(1);
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();

    for submission_id in [first, second] {
        lifecycle
            .accept_submission(AcceptSubmissionRequest {
                submission_id,
                account_id,
                inbound_event_id: None,
                ingest_job_id: Uuid::new_v4(),
            })
            .await
            .expect("accept");
    }

    let barrier = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for submission_id in [first, second] {
        let lifecycle = Arc::clone(&lifecycle);
        let bytes = bytes.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.fetch_add(1, Ordering::SeqCst);
            while barrier.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
            lifecycle
                .ingest(
                    account_id,
                    submission_id,
                    &bytes,
                    "image/png",
                    Uuid::new_v4(),
                )
                .await
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("join"));
    }
    assert!(
        results.iter().all(|result| result.is_ok()),
        "callers must not see unique/dependency failures: {results:?}"
    );
    let stored = results
        .iter()
        .filter(|result| matches!(result, Ok(IngestOutcome::Stored { .. })))
        .count();
    let absorbed = results
        .iter()
        .filter(|result| matches!(result, Ok(IngestOutcome::DuplicateAbsorbed { .. })))
        .count();
    assert_eq!(stored, 1);
    assert_eq!(absorbed, 1);

    let asset_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receipt_assets WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(lifecycle.pool())
            .await
            .expect("asset count");
    assert_eq!(asset_count, 1);
    assert_eq!(object_store.stored_object_count(), 1);
}

#[tokio::test]
async fn extract_retries_failed_transient_via_extracting() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("extract_retries_failed_transient_via_extracting") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let object_store = InMemoryObjectStore::new();
    let failing = ReceiptLifecycle::with_extractor(
        pool.clone(),
        object_store.clone(),
        Arc::new(ForcedFailureExtractor {
            error: ReceiptError::transient("forced extractor failure"),
        }),
        ReceiptConfig::default(),
    );
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(0);
    accept_and_ingest(&failing, account_id, submission_id, &bytes).await;

    let error = failing
        .extract(account_id, submission_id)
        .await
        .expect_err("forced failure");
    assert_eq!(error.class, ErrorClass::Transient);
    assert_receipt_state(
        &failing,
        account_id,
        submission_id,
        ReceiptState::FailedTransient,
    )
    .await;
    let failed_attempts: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM extraction_attempts
        WHERE submission_id = $1 AND outcome = 'failed' AND error_class = 'transient'
        "#,
    )
    .bind(submission_id)
    .fetch_one(failing.pool())
    .await
    .expect("failed attempts");
    assert_eq!(failed_attempts, 1);

    let recovering = ReceiptLifecycle::with_extractor(
        pool,
        object_store,
        Arc::new(FakeExtractor),
        ReceiptConfig::default(),
    );
    let outcome = recovering
        .extract(account_id, submission_id)
        .await
        .expect("retry extract");
    assert!(matches!(outcome, ExtractOutcome::ReviewRequired { .. }));
    assert_receipt_state(
        &recovering,
        account_id,
        submission_id,
        ReceiptState::ReviewRequired,
    )
    .await;
}

#[tokio::test]
async fn extract_persists_extractor_attempt_metadata() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("extract_persists_extractor_attempt_metadata") else {
        return;
    };

    struct MetaExtractor;

    impl ReceiptExtractor for MetaExtractor {
        fn extract(
            &self,
            bytes: &[u8],
        ) -> Result<zl_expense::receipt::ExtractedAttempt, ReceiptError> {
            Ok(zl_expense::receipt::ExtractedAttempt {
                result: zl_expense::receipt::extract(bytes)?,
                meta: self.meta(),
            })
        }

        fn meta(&self) -> zl_expense::receipt::ExtractionMeta {
            zl_expense::receipt::ExtractionMeta {
                provider: "gemini".to_string(),
                model: "gemini-2.5-flash".to_string(),
                profile_name: "receipt-fast".to_string(),
                prompt_version: "extraction-json-v1".to_string(),
                input_tokens: Some(11),
                output_tokens: Some(22),
            }
        }
    }

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = ReceiptLifecycle::with_extractor(
        pool.clone(),
        InMemoryObjectStore::new(),
        Arc::new(MetaExtractor),
        ReceiptConfig::default(),
    );
    let submission_id = Uuid::new_v4();
    accept_and_ingest(&lifecycle, account_id, submission_id, &corpus_png(0)).await;
    lifecycle
        .extract(account_id, submission_id)
        .await
        .expect("extract");

    let row: (String, String, String, String, Option<i32>, Option<i32>) = sqlx::query_as(
        r#"
        SELECT provider, model, profile_name, prompt_version, input_tokens, output_tokens
        FROM extraction_attempts
        WHERE submission_id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_one(lifecycle.pool())
    .await
    .expect("attempt");
    assert_eq!(
        row,
        (
            "gemini".to_string(),
            "gemini-2.5-flash".to_string(),
            "receipt-fast".to_string(),
            "extraction-json-v1".to_string(),
            Some(11),
            Some(22)
        )
    );
}

#[tokio::test]
async fn extract_object_lookup_failure_is_persisted() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("extract_object_lookup_failure_is_persisted") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let object_store = InMemoryObjectStore::new();
    let lifecycle =
        ReceiptLifecycle::new(pool.clone(), object_store.clone(), ReceiptConfig::default());
    let submission_id = Uuid::new_v4();
    accept_and_ingest(&lifecycle, account_id, submission_id, &corpus_png(2)).await;

    let object_key: String =
        sqlx::query_scalar("SELECT object_key FROM receipt_assets WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("object key");
    object_store.delete(&object_key).expect("delete object");

    let error = lifecycle
        .extract(account_id, submission_id)
        .await
        .expect_err("missing object");
    assert_eq!(error.class, ErrorClass::NotFound);
    assert_receipt_state(
        &lifecycle,
        account_id,
        submission_id,
        ReceiptState::FailedPermanent,
    )
    .await;
    let attempt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM extraction_attempts WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("attempt count");
    assert_eq!(attempt_count, 1);
}

#[tokio::test]
async fn mime_mismatch_is_rejected_as_validation_failure() {
    let _guard = integration_lock();
    let Some(_) = skip_without_database("mime_mismatch_is_rejected_as_validation_failure") else {
        return;
    };

    let pool = receipt_fresh_pool().await;
    let account_id = seed_active_account(&pool).await;
    let lifecycle = receipt_lifecycle(pool);
    let submission_id = Uuid::new_v4();
    let bytes = corpus_png(0);
    lifecycle
        .accept_submission(AcceptSubmissionRequest {
            submission_id,
            account_id,
            inbound_event_id: None,
            ingest_job_id: Uuid::new_v4(),
        })
        .await
        .expect("accept");

    let error = lifecycle
        .ingest(
            account_id,
            submission_id,
            &bytes,
            "image/jpeg",
            Uuid::new_v4(),
        )
        .await
        .expect_err("mime mismatch");
    assert_eq!(error.class, ErrorClass::Validation);
    assert_receipt_state(
        &lifecycle,
        account_id,
        submission_id,
        ReceiptState::FailedPermanent,
    )
    .await;
}

#[tokio::test]
async fn public_request_debug_redacts_sensitive_fields() {
    let submission_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let request = AcceptSubmissionRequest {
        submission_id,
        account_id,
        inbound_event_id: Some(Uuid::new_v4()),
        ingest_job_id: Uuid::new_v4(),
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains(&submission_id.to_string()));
    assert!(!debug.contains(&account_id.to_string()));
    assert!(debug.contains("[REDACTED]"));
}
