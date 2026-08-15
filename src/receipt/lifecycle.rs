//! PostgreSQL-backed receipt lifecycle seam.

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ErrorClass;
use crate::work::{EnqueueOutcome, EnqueueRequest, WorkStore};

use super::error::ReceiptError;
use super::extractor::{FakeExtractor, ReceiptExtractor};
use super::object_store::ReceiptObjectStore;
use super::types::{
    AcceptSubmissionOutcome, AcceptSubmissionRequest, ConfirmOutcome, ConfirmRequest,
    EditDraftRequest, ExpenseDraftView, ExtractOutcome, IngestOutcome, JOB_TYPE_EXTRACT,
    JOB_TYPE_INGEST, ReceiptConfig, ReceiptJobPayload, ReceiptState, ReceiptStateView,
    RejectRequest, ValidatedImage, account_serialization_key, can_transition, extract_dedupe_key,
    ingest_dedupe_key,
};
use super::validate::{
    object_key, validate_amount_minor, validate_currency, validate_image, validate_merchant,
};

/// Deep receipt lifecycle module backed by PostgreSQL and a pluggable object store.
#[derive(Clone)]
pub struct ReceiptLifecycle {
    pool: PgPool,
    object_store: Arc<dyn ReceiptObjectStore>,
    extractor: Arc<dyn ReceiptExtractor>,
    config: ReceiptConfig,
}

impl ReceiptLifecycle {
    pub fn new(
        pool: PgPool,
        object_store: Arc<dyn ReceiptObjectStore>,
        config: ReceiptConfig,
    ) -> Self {
        Self::with_extractor(pool, object_store, Arc::new(FakeExtractor), config)
    }

    pub fn with_extractor(
        pool: PgPool,
        object_store: Arc<dyn ReceiptObjectStore>,
        extractor: Arc<dyn ReceiptExtractor>,
        config: ReceiptConfig,
    ) -> Self {
        Self {
            pool,
            object_store,
            extractor,
            config,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn config(&self) -> ReceiptConfig {
        self.config
    }

    pub async fn accept_submission(
        &self,
        request: AcceptSubmissionRequest,
    ) -> Result<AcceptSubmissionOutcome, ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let outcome = self
            .accept_submission_in_transaction(&mut tx, request)
            .await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(outcome)
    }

    /// Accept a submission inside an existing transaction without committing.
    pub async fn accept_submission_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: AcceptSubmissionRequest,
    ) -> Result<AcceptSubmissionOutcome, ReceiptError> {
        let inserted = insert_submission(
            tx,
            request.submission_id,
            request.account_id,
            request.inbound_event_id,
        )
        .await?;

        if inserted {
            return finish_accept_in_transaction(
                tx,
                request.submission_id,
                request.account_id,
                request.ingest_job_id,
            )
            .await;
        }

        let existing =
            load_existing_submission(tx, request.submission_id, request.inbound_event_id)
                .await?
                .ok_or_else(|| dependency("replay lookup failed"))?;

        if existing.account_id != request.account_id {
            return Err(ReceiptError::conflict(
                "inbound event belongs to a different account",
            ));
        }

        if existing.state == ReceiptState::Pending {
            return finish_accept_in_transaction(
                tx,
                existing.submission_id,
                existing.account_id,
                request.ingest_job_id,
            )
            .await;
        }

        Ok(AcceptSubmissionOutcome::Replayed {
            submission_id: existing.submission_id,
            state: existing.state,
        })
    }

    /// Confirm a review-required draft inside an existing transaction without committing.
    pub async fn confirm_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: ConfirmRequest,
    ) -> Result<ConfirmOutcome, ReceiptError> {
        let submission =
            load_submission_for_update(tx, request.submission_id, request.account_id).await?;
        if submission.state == ReceiptState::Confirmed {
            let expense_id = submission
                .confirmed_expense_id
                .ok_or_else(|| ReceiptError::dependency("confirmed submission missing expense"))?;
            return Ok(ConfirmOutcome::AlreadyConfirmed { expense_id });
        }
        if submission.state != ReceiptState::ReviewRequired {
            return Err(ReceiptError::conflict("submission is not awaiting review"));
        }

        let draft = load_draft_for_update(tx, request.submission_id, request.account_id).await?;
        if draft.version != request.expected_draft_version {
            return Err(ReceiptError::conflict("draft version mismatch"));
        }

        insert_confirmed_expense(
            tx,
            request.expense_id,
            request.account_id,
            request.submission_id,
            &draft,
        )
        .await?;
        let expense_id = lookup_confirmed_expense_id(tx, request.submission_id).await?;

        transition_state(
            tx,
            request.submission_id,
            request.account_id,
            ReceiptState::ReviewRequired,
            ReceiptState::Confirmed,
            None,
            None,
            Some(expense_id),
            None,
        )
        .await?;

        Ok(ConfirmOutcome::Confirmed { expense_id })
    }

    /// Reject a review-required draft inside an existing transaction without committing.
    pub async fn reject_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: RejectRequest,
    ) -> Result<(), ReceiptError> {
        let submission =
            load_submission_for_update(tx, request.submission_id, request.account_id).await?;
        if submission.state == ReceiptState::Rejected {
            return Ok(());
        }
        if submission.state != ReceiptState::ReviewRequired {
            return Err(ReceiptError::conflict("submission is not awaiting review"));
        }
        let draft = load_draft_for_update(tx, request.submission_id, request.account_id).await?;
        if draft.version != request.expected_draft_version {
            return Err(ReceiptError::conflict("draft version mismatch"));
        }

        transition_state(
            tx,
            request.submission_id,
            request.account_id,
            ReceiptState::ReviewRequired,
            ReceiptState::Rejected,
            None,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    /// Edit a review draft inside an existing transaction without committing.
    pub async fn edit_draft_in_transaction(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        request: EditDraftRequest,
    ) -> Result<ExpenseDraftView, ReceiptError> {
        let submission =
            load_submission_for_update(tx, request.submission_id, request.account_id).await?;
        if submission.state != ReceiptState::ReviewRequired {
            return Err(ReceiptError::conflict(
                "draft can only be edited while review is required",
            ));
        }

        let draft = load_draft_for_update(tx, request.submission_id, request.account_id).await?;
        if draft.version != request.expected_version {
            return Err(ReceiptError::conflict("draft version mismatch"));
        }

        if let Some(amount_minor) = request.amount_minor {
            validate_amount_minor(amount_minor)?;
        }
        if let Some(ref currency) = request.currency {
            validate_currency(currency)?;
        }
        let merchant = match request.merchant.as_deref() {
            Some(value) => Some(validate_merchant(value)?),
            None => None,
        };
        if let Some(ref category_key) = request.category_key {
            ensure_category(tx, category_key).await?;
        }

        let mut next = draft.clone();
        if let Some(amount_minor) = request.amount_minor
            && amount_minor != draft.amount_minor
        {
            record_correction(
                tx,
                draft.draft_id,
                request.submission_id,
                "amount_minor",
                Some(draft.amount_minor.to_string()),
                Some(amount_minor.to_string()),
            )
            .await?;
            next.amount_minor = amount_minor;
        }
        if let Some(currency) = request.currency
            && currency != draft.currency
        {
            record_correction(
                tx,
                draft.draft_id,
                request.submission_id,
                "currency",
                Some(draft.currency.clone()),
                Some(currency.clone()),
            )
            .await?;
            next.currency = currency;
        }
        if let Some(merchant) = merchant
            && merchant != draft.merchant
        {
            record_correction(
                tx,
                draft.draft_id,
                request.submission_id,
                "merchant",
                Some(draft.merchant.clone()),
                Some(merchant.clone()),
            )
            .await?;
            next.merchant = merchant;
        }
        if let Some(category_key) = request.category_key
            && category_key != draft.category_key
        {
            record_correction(
                tx,
                draft.draft_id,
                request.submission_id,
                "category_key",
                Some(draft.category_key.clone()),
                Some(category_key.clone()),
            )
            .await?;
            next.category_key = category_key;
        }
        if let Some(occurred_at) = request.occurred_at
            && occurred_at != draft.occurred_at
        {
            record_correction(
                tx,
                draft.draft_id,
                request.submission_id,
                "occurred_at",
                Some(draft.occurred_at.to_rfc3339()),
                Some(occurred_at.to_rfc3339()),
            )
            .await?;
            next.occurred_at = occurred_at;
        }

        next.version = request.expected_version;
        update_draft(tx, &next).await?;
        next.version += 1;
        Ok(next)
    }

    /// Move a queued submission to `failed_permanent` for permanent validation failures.
    pub async fn fail_queued(
        &self,
        submission_id: Uuid,
        account_id: Uuid,
        error_class: ErrorClass,
    ) -> Result<(), ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if current.state == ReceiptState::FailedPermanent {
            tx.commit().await.map_err(|_| dependency("commit failed"))?;
            return Ok(());
        }
        if current.state != ReceiptState::Queued {
            return Err(ReceiptError::conflict("submission is not queued"));
        }
        transition_state(
            &mut tx,
            submission_id,
            account_id,
            ReceiptState::Queued,
            ReceiptState::FailedPermanent,
            Some(error_class.as_str()),
            None,
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(())
    }

    pub async fn ingest(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
        bytes: &[u8],
        mime_type: &str,
        extract_job_id: Uuid,
    ) -> Result<IngestOutcome, ReceiptError> {
        let row = load_submission(&self.pool, submission_id, account_id).await?;
        let has_asset = load_asset(&self.pool, submission_id, account_id)
            .await?
            .is_some();
        if row.state == ReceiptState::Stored
            || row.state == ReceiptState::Extracting
            || row.state == ReceiptState::ReviewRequired
            || row.state == ReceiptState::Confirmed
            || (row.state == ReceiptState::FailedTransient && has_asset)
        {
            return Ok(IngestOutcome::AlreadyStored { submission_id });
        }
        if row.state.is_terminal() {
            return Ok(IngestOutcome::AlreadyTerminal { state: row.state });
        }
        if row.state != ReceiptState::Queued && row.state != ReceiptState::FailedTransient {
            return Err(ReceiptError::conflict("submission not ready for ingest"));
        }

        let validated = match validate_image(bytes, mime_type) {
            Ok(validated) => validated,
            Err(error) => {
                self.mark_failure(
                    submission_id,
                    account_id,
                    ReceiptState::FailedPermanent,
                    error.class,
                )
                .await?;
                return Err(error);
            }
        };

        if let Some(original_id) =
            find_active_duplicate(&self.pool, account_id, &validated.content_sha256).await?
            && original_id != submission_id
        {
            self.absorb_duplicate(submission_id, account_id, original_id)
                .await?;
            return Ok(IngestOutcome::DuplicateAbsorbed {
                submission_id,
                original_submission_id: original_id,
            });
        }

        let key = object_key(account_id, submission_id, &validated.content_sha256);
        self.object_store.put(&key, bytes)?;

        let persist = self
            .persist_stored_ingest(account_id, submission_id, &key, &validated, extract_job_id)
            .await;

        match &persist {
            Ok(IngestOutcome::Stored { .. } | IngestOutcome::AlreadyStored { .. }) => {}
            _ => {
                self.compensate_orphan_object(account_id, submission_id, &key)
                    .await;
            }
        }

        persist
    }

    async fn persist_stored_ingest(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
        key: &str,
        validated: &ValidatedImage,
        extract_job_id: Uuid,
    ) -> Result<IngestOutcome, ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if current.state == ReceiptState::Stored
            || current.state == ReceiptState::Extracting
            || current.state == ReceiptState::ReviewRequired
            || current.state == ReceiptState::Confirmed
        {
            tx.commit().await.map_err(|_| dependency("commit failed"))?;
            return Ok(IngestOutcome::AlreadyStored { submission_id });
        }
        if current.state.is_terminal() {
            tx.rollback().await.ok();
            return Ok(IngestOutcome::AlreadyTerminal {
                state: current.state,
            });
        }
        if !matches!(
            current.state,
            ReceiptState::Queued | ReceiptState::FailedTransient
        ) {
            tx.rollback().await.ok();
            return Err(ReceiptError::conflict("submission not ready for ingest"));
        }

        transition_state(
            &mut tx,
            submission_id,
            account_id,
            current.state,
            ReceiptState::Stored,
            None,
            None,
            None,
            None,
        )
        .await?;

        match insert_asset(
            &mut tx,
            Uuid::new_v4(),
            submission_id,
            account_id,
            key,
            validated,
        )
        .await?
        {
            AssetInsert::Inserted => {}
            AssetInsert::UniqueConflict => {
                tx.rollback().await.ok();
                if let Some(original_id) =
                    find_active_duplicate(&self.pool, account_id, &validated.content_sha256).await?
                    && original_id != submission_id
                {
                    self.absorb_duplicate(submission_id, account_id, original_id)
                        .await?;
                    return Ok(IngestOutcome::DuplicateAbsorbed {
                        submission_id,
                        original_submission_id: original_id,
                    });
                }
                return Ok(IngestOutcome::AlreadyStored { submission_id });
            }
        }

        enqueue_receipt_job(
            &mut tx,
            extract_job_id,
            JOB_TYPE_EXTRACT,
            submission_id,
            account_id,
        )
        .await?;

        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(IngestOutcome::Stored {
            submission_id,
            content_sha256: validated.content_sha256.clone(),
        })
    }

    async fn compensate_orphan_object(&self, account_id: Uuid, submission_id: Uuid, key: &str) {
        let has_asset = load_asset(&self.pool, submission_id, account_id)
            .await
            .ok()
            .flatten()
            .is_some();
        if !has_asset {
            let _ = self.object_store.delete(key);
        }
    }

    pub async fn extract(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
    ) -> Result<ExtractOutcome, ReceiptError> {
        match self.claim_extracting(account_id, submission_id).await? {
            ExtractClaim::Done(outcome) => return Ok(outcome),
            ExtractClaim::Proceed => {}
        }

        let started = Instant::now();
        let extracted = self.read_and_extract(account_id, submission_id).await;
        let latency_ms =
            i32::try_from(started.elapsed().as_millis().min(i64::MAX as u128)).unwrap_or(i32::MAX);

        match extracted {
            Ok(extraction) if extraction.unsupported => {
                self.persist_extraction_failure(
                    account_id,
                    submission_id,
                    "failed",
                    ErrorClass::Unsupported,
                    ReceiptState::FailedPermanent,
                    latency_ms,
                )
                .await?;
                Ok(ExtractOutcome::Unsupported)
            }
            Ok(extraction) => {
                self.persist_extraction_success(account_id, submission_id, extraction, latency_ms)
                    .await
            }
            Err(error) => {
                let terminal = failure_state_for(error.class);
                self.persist_extraction_failure(
                    account_id,
                    submission_id,
                    "failed",
                    error.class,
                    terminal,
                    latency_ms,
                )
                .await?;
                Err(error)
            }
        }
    }

    async fn claim_extracting(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
    ) -> Result<ExtractClaim, ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if current.state == ReceiptState::ReviewRequired {
            tx.commit().await.map_err(|_| dependency("commit failed"))?;
            return Ok(ExtractClaim::Done(ExtractOutcome::AlreadyReviewRequired {
                submission_id,
            }));
        }
        if current.state.is_terminal() {
            tx.commit().await.map_err(|_| dependency("commit failed"))?;
            return Ok(ExtractClaim::Done(ExtractOutcome::AlreadyTerminal {
                state: current.state,
            }));
        }
        match current.state {
            ReceiptState::Stored | ReceiptState::FailedTransient => {
                transition_state(
                    &mut tx,
                    submission_id,
                    account_id,
                    current.state,
                    ReceiptState::Extracting,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            }
            ReceiptState::Extracting => {}
            _ => {
                return Err(ReceiptError::conflict(
                    "submission not ready for extraction",
                ));
            }
        }
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(ExtractClaim::Proceed)
    }

    async fn read_and_extract(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
    ) -> Result<super::types::ExtractionResult, ReceiptError> {
        let asset = load_asset(&self.pool, submission_id, account_id)
            .await?
            .ok_or_else(|| ReceiptError::not_found("receipt asset not found"))?;
        let bytes = self
            .object_store
            .get(&asset.object_key)?
            .ok_or_else(|| ReceiptError::not_found("receipt object missing"))?;
        self.extractor.extract(&bytes)
    }

    async fn persist_extraction_success(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
        extraction: super::types::ExtractionResult,
        latency_ms: i32,
    ) -> Result<ExtractOutcome, ReceiptError> {
        let draft_id = Uuid::new_v4();
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if current.state == ReceiptState::ReviewRequired {
            tx.commit().await.map_err(|_| dependency("commit failed"))?;
            return Ok(ExtractOutcome::AlreadyReviewRequired { submission_id });
        }
        if current.state.is_terminal() {
            tx.rollback().await.ok();
            return Ok(ExtractOutcome::AlreadyTerminal {
                state: current.state,
            });
        }
        if current.state != ReceiptState::Extracting {
            tx.rollback().await.ok();
            return Err(ReceiptError::conflict(
                "submission not ready for extraction",
            ));
        }

        record_extraction_attempt(
            &mut tx,
            submission_id,
            current.attempt_count + 1,
            "success",
            None,
            latency_ms,
        )
        .await?;
        upsert_draft(&mut tx, draft_id, submission_id, account_id, &extraction).await?;
        transition_state(
            &mut tx,
            submission_id,
            account_id,
            ReceiptState::Extracting,
            ReceiptState::ReviewRequired,
            None,
            Some(self.config.review_expiry_hours),
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(ExtractOutcome::ReviewRequired {
            submission_id,
            draft_id,
        })
    }

    async fn persist_extraction_failure(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
        outcome: &str,
        error_class: ErrorClass,
        terminal_state: ReceiptState,
        latency_ms: i32,
    ) -> Result<(), ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if current.state != ReceiptState::Extracting {
            tx.rollback().await.ok();
            return Ok(());
        }
        record_extraction_attempt(
            &mut tx,
            submission_id,
            current.attempt_count + 1,
            outcome,
            Some(error_class.as_str()),
            latency_ms,
        )
        .await?;
        transition_state(
            &mut tx,
            submission_id,
            account_id,
            ReceiptState::Extracting,
            terminal_state,
            Some(error_class.as_str()),
            None,
            None,
            None,
        )
        .await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(())
    }

    pub async fn get_state(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
    ) -> Result<ReceiptStateView, ReceiptError> {
        let row = load_submission(&self.pool, submission_id, account_id).await?;
        let asset_deleted = asset_deleted(&self.pool, submission_id).await?;
        Ok(row.into_view(asset_deleted))
    }

    pub async fn get_draft(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
    ) -> Result<ExpenseDraftView, ReceiptError> {
        load_draft(&self.pool, submission_id, account_id).await
    }

    pub async fn edit_draft(
        &self,
        request: EditDraftRequest,
    ) -> Result<ExpenseDraftView, ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let draft = self.edit_draft_in_transaction(&mut tx, request).await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(draft)
    }

    pub async fn confirm(&self, request: ConfirmRequest) -> Result<ConfirmOutcome, ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let outcome = self.confirm_in_transaction(&mut tx, request).await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(outcome)
    }

    pub async fn reject(&self, request: RejectRequest) -> Result<(), ReceiptError> {
        let mut tx = self.begin_tx().await?;
        self.reject_in_transaction(&mut tx, request).await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(())
    }

    pub async fn expire_reviews(&self, batch_limit: i32) -> Result<usize, ReceiptError> {
        if batch_limit <= 0 {
            return Err(ReceiptError::validation("batch_limit must be positive"));
        }
        let mut tx = self.begin_tx().await?;
        let candidates: Vec<(Uuid, Uuid)> = sqlx::query_as(
            r#"
            SELECT id, account_id
            FROM receipt_submissions
            WHERE lifecycle_state = 'review_required'
              AND review_expires_at IS NOT NULL
              AND review_expires_at <= NOW()
            ORDER BY review_expires_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(batch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| dependency("expire candidate lookup failed"))?;

        let mut expired = 0_usize;
        for (submission_id, account_id) in candidates {
            let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
            if current.state != ReceiptState::ReviewRequired {
                continue;
            }
            transition_state(
                &mut tx,
                submission_id,
                account_id,
                ReceiptState::ReviewRequired,
                ReceiptState::Expired,
                None,
                None,
                None,
                None,
            )
            .await?;
            expired += 1;
        }
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(expired)
    }

    pub async fn delete_original(
        &self,
        account_id: Uuid,
        submission_id: Uuid,
    ) -> Result<(), ReceiptError> {
        let asset = load_asset(&self.pool, submission_id, account_id)
            .await?
            .ok_or_else(|| ReceiptError::not_found("receipt asset not found"))?;
        if asset.deletion_state == "deleted" {
            return Ok(());
        }

        self.object_store.delete(&asset.object_key)?;

        let mut tx = self.begin_tx().await?;
        let submission = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if submission.state != ReceiptState::Deleted {
            if !can_transition(submission.state, ReceiptState::Deleted) {
                return Err(ReceiptError::conflict(
                    "submission cannot delete original in current state",
                ));
            }
            transition_state(
                &mut tx,
                submission_id,
                account_id,
                submission.state,
                ReceiptState::Deleted,
                None,
                None,
                None,
                None,
            )
            .await?;
        }

        sqlx::query(
            r#"
            UPDATE receipt_assets
            SET deletion_state = 'deleted',
                original_deleted_at = NOW()
            WHERE submission_id = $1
              AND deletion_state = 'active'
            "#,
        )
        .bind(submission_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| dependency("asset delete update failed"))?;

        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(())
    }

    pub async fn retention_sweep(&self, batch_limit: i32) -> Result<usize, ReceiptError> {
        if batch_limit <= 0 {
            return Err(ReceiptError::validation("batch_limit must be positive"));
        }

        let mut tx = self.begin_tx().await?;
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT a.submission_id, a.object_key
            FROM receipt_assets a
            JOIN receipt_submissions s
              ON s.id = a.submission_id
             AND s.account_id = a.account_id
            WHERE a.deletion_state = 'active'
              AND a.retention_deadline <= NOW()
              AND s.lifecycle_state IN ('confirmed', 'rejected', 'expired', 'failed_permanent')
            ORDER BY a.retention_deadline ASC
            LIMIT $1
            FOR UPDATE OF a SKIP LOCKED
            "#,
        )
        .bind(batch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| dependency("retention candidate lookup failed"))?;

        for (_, object_key_value) in &rows {
            self.object_store.delete(object_key_value)?;
        }

        for (submission_id, _) in &rows {
            sqlx::query(
                r#"
                UPDATE receipt_assets
                SET deletion_state = 'deleted',
                    original_deleted_at = NOW()
                WHERE submission_id = $1
                  AND deletion_state = 'active'
                "#,
            )
            .bind(submission_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| dependency("retention claim update failed"))?;
        }
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(rows.len())
    }

    async fn absorb_duplicate(
        &self,
        submission_id: Uuid,
        account_id: Uuid,
        original_submission_id: Uuid,
    ) -> Result<(), ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if current.duplicate_of_submission_id == Some(original_submission_id)
            && current.state == ReceiptState::FailedPermanent
        {
            tx.commit().await.map_err(|_| dependency("commit failed"))?;
            return Ok(());
        }
        transition_state(
            &mut tx,
            submission_id,
            account_id,
            current.state,
            ReceiptState::FailedPermanent,
            Some(ErrorClass::Duplicate.as_str()),
            None,
            None,
            Some(original_submission_id),
        )
        .await?;
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(())
    }

    async fn mark_failure(
        &self,
        submission_id: Uuid,
        account_id: Uuid,
        terminal_state: ReceiptState,
        error_class: ErrorClass,
    ) -> Result<(), ReceiptError> {
        let mut tx = self.begin_tx().await?;
        let current = load_submission_for_update(&mut tx, submission_id, account_id).await?;
        if can_transition(current.state, terminal_state) {
            transition_state(
                &mut tx,
                submission_id,
                account_id,
                current.state,
                terminal_state,
                Some(error_class.as_str()),
                None,
                None,
                None,
            )
            .await?;
        }
        tx.commit().await.map_err(|_| dependency("commit failed"))?;
        Ok(())
    }

    async fn begin_tx(&self) -> Result<Transaction<'_, Postgres>, ReceiptError> {
        self.pool
            .begin()
            .await
            .map_err(|_| dependency("failed to begin transaction"))
    }
}

#[derive(Debug, Clone)]
struct SubmissionRow {
    submission_id: Uuid,
    account_id: Uuid,
    state: ReceiptState,
    version: i32,
    duplicate_of_submission_id: Option<Uuid>,
    confirmed_expense_id: Option<Uuid>,
    failure_error_class: Option<String>,
    review_expires_at: Option<DateTime<Utc>>,
    attempt_count: i32,
}

impl SubmissionRow {
    fn into_view(self, asset_deleted: bool) -> ReceiptStateView {
        ReceiptStateView {
            submission_id: self.submission_id,
            account_id: self.account_id,
            state: self.state,
            version: self.version,
            duplicate_of_submission_id: self.duplicate_of_submission_id,
            confirmed_expense_id: self.confirmed_expense_id,
            failure_error_class: self.failure_error_class,
            review_expires_at: self.review_expires_at,
            asset_deleted,
        }
    }
}

#[derive(Debug, Clone)]
struct AssetRow {
    object_key: String,
    deletion_state: String,
}

enum AssetInsert {
    Inserted,
    UniqueConflict,
}

enum ExtractClaim {
    Proceed,
    Done(ExtractOutcome),
}

async fn finish_accept_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    account_id: Uuid,
    ingest_job_id: Uuid,
) -> Result<AcceptSubmissionOutcome, ReceiptError> {
    let current = load_submission_for_update(tx, submission_id, account_id).await?;
    if current.state == ReceiptState::Pending {
        transition_state(
            tx,
            submission_id,
            account_id,
            ReceiptState::Pending,
            ReceiptState::Queued,
            None,
            None,
            None,
            None,
        )
        .await?;
    } else if current.state != ReceiptState::Queued {
        return Ok(AcceptSubmissionOutcome::Replayed {
            submission_id,
            state: current.state,
        });
    }

    let outcome = enqueue_receipt_job(
        tx,
        ingest_job_id,
        JOB_TYPE_INGEST,
        submission_id,
        account_id,
    )
    .await?;
    match outcome {
        EnqueueOutcome::Enqueued => Ok(AcceptSubmissionOutcome::Accepted {
            state: ReceiptState::Queued,
        }),
        EnqueueOutcome::Duplicate => Ok(AcceptSubmissionOutcome::DuplicateJob),
    }
}

async fn insert_submission(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    account_id: Uuid,
    inbound_event_id: Option<Uuid>,
) -> Result<bool, ReceiptError> {
    let result = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO receipt_submissions (
            id, account_id, inbound_event_id, lifecycle_state
        )
        VALUES ($1, $2, $3, 'pending')
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .bind(inbound_event_id)
    .fetch_optional(&mut **tx)
    .await;

    match result {
        Ok(inserted) => Ok(inserted.is_some()),
        Err(error) if is_unique_violation(&error) => Ok(false),
        Err(_) => Err(dependency("submission insert failed")),
    }
}

async fn load_existing_submission(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    inbound_event_id: Option<Uuid>,
) -> Result<Option<SubmissionRow>, ReceiptError> {
    if let Some(row) = load_submission_tuple_for_update(tx, submission_id, None).await? {
        return Ok(Some(submission_from_tuple(row, 0)?));
    }
    if let Some(inbound_event_id) = inbound_event_id
        && let Some(row) = load_submission_tuple_by_inbound_event(tx, inbound_event_id).await?
    {
        return Ok(Some(submission_from_tuple(row, 0)?));
    }
    Ok(None)
}

async fn load_submission_tuple_for_update(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    account_id: Option<Uuid>,
) -> Result<Option<SubmissionRowTuple>, ReceiptError> {
    sqlx::query_as(
        r#"
        SELECT
            id,
            account_id,
            lifecycle_state,
            version,
            duplicate_of_submission_id,
            confirmed_expense_id,
            failure_error_class,
            review_expires_at
        FROM receipt_submissions
        WHERE id = $1
          AND ($2::uuid IS NULL OR account_id = $2)
        FOR UPDATE
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| dependency("submission lookup failed"))
}

async fn load_submission_tuple_by_inbound_event(
    tx: &mut Transaction<'_, Postgres>,
    inbound_event_id: Uuid,
) -> Result<Option<SubmissionRowTuple>, ReceiptError> {
    sqlx::query_as(
        r#"
        SELECT
            id,
            account_id,
            lifecycle_state,
            version,
            duplicate_of_submission_id,
            confirmed_expense_id,
            failure_error_class,
            review_expires_at
        FROM receipt_submissions
        WHERE inbound_event_id = $1
        FOR UPDATE
        "#,
    )
    .bind(inbound_event_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| dependency("replay lookup failed"))
}

type SubmissionRowTuple = (
    Uuid,
    Uuid,
    String,
    i32,
    Option<Uuid>,
    Option<Uuid>,
    Option<String>,
    Option<DateTime<Utc>>,
);

fn submission_from_tuple(
    tuple: SubmissionRowTuple,
    attempt_count: i32,
) -> Result<SubmissionRow, ReceiptError> {
    let (
        submission_id,
        account_id,
        state,
        version,
        duplicate_of_submission_id,
        confirmed_expense_id,
        failure_error_class,
        review_expires_at,
    ) = tuple;
    Ok(SubmissionRow {
        submission_id,
        account_id,
        state: ReceiptState::parse(&state).ok_or_else(|| dependency("invalid submission state"))?,
        version,
        duplicate_of_submission_id,
        confirmed_expense_id,
        failure_error_class,
        review_expires_at,
        attempt_count,
    })
}

#[allow(clippy::too_many_arguments)]
async fn transition_state(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    account_id: Uuid,
    from: ReceiptState,
    to: ReceiptState,
    failure_error_class: Option<&str>,
    review_expiry_hours: Option<i64>,
    confirmed_expense_id: Option<Uuid>,
    duplicate_of_submission_id: Option<Uuid>,
) -> Result<(), ReceiptError> {
    if !can_transition(from, to) {
        return Err(ReceiptError::conflict("illegal lifecycle transition"));
    }
    let updated = sqlx::query(
        r#"
        UPDATE receipt_submissions
        SET lifecycle_state = $4,
            version = version + 1,
            failure_error_class = CASE
                WHEN $4 IN ('failed_transient', 'failed_permanent') THEN $5
                WHEN $4 = 'deleted' THEN failure_error_class
                ELSE NULL
            END,
            review_expires_at = CASE
                WHEN $6::bigint IS NOT NULL THEN NOW() + ($6::bigint * INTERVAL '1 hour')
                ELSE review_expires_at
            END,
            confirmed_expense_id = COALESCE($7, confirmed_expense_id),
            duplicate_of_submission_id = COALESCE($8, duplicate_of_submission_id),
            updated_at = NOW()
        WHERE id = $1
          AND account_id = $2
          AND lifecycle_state = $3
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .bind(from.as_str())
    .bind(to.as_str())
    .bind(failure_error_class)
    .bind(review_expiry_hours)
    .bind(confirmed_expense_id)
    .bind(duplicate_of_submission_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| dependency("state transition failed"))?;

    if updated.rows_affected() != 1 {
        return Err(ReceiptError::conflict(
            "optimistic lifecycle transition failed",
        ));
    }
    Ok(())
}

async fn insert_asset(
    tx: &mut Transaction<'_, Postgres>,
    asset_id: Uuid,
    submission_id: Uuid,
    account_id: Uuid,
    object_key_value: &str,
    validated: &ValidatedImage,
) -> Result<AssetInsert, ReceiptError> {
    let pixel_count = i64::from(validated.width_px) * i64::from(validated.height_px);
    let result = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO receipt_assets (
            id, submission_id, account_id, object_key, content_sha256, mime_type,
            size_bytes, width_px, height_px, pixel_count, retention_deadline
        )
        SELECT
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            NOW() + (a.retention_preference_days * INTERVAL '1 day')
        FROM accounts a
        WHERE a.id = $3
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(asset_id)
    .bind(submission_id)
    .bind(account_id)
    .bind(object_key_value)
    .bind(&validated.content_sha256)
    .bind(&validated.mime_type)
    .bind(validated.size_bytes)
    .bind(validated.width_px)
    .bind(validated.height_px)
    .bind(pixel_count)
    .fetch_optional(&mut **tx)
    .await;

    match result {
        Ok(Some(_)) => Ok(AssetInsert::Inserted),
        Err(error) if is_unique_violation(&error) => Ok(AssetInsert::UniqueConflict),
        Err(_) => Err(dependency("asset insert failed")),
        Ok(None) => {
            let account_exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1)")
                    .bind(account_id)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(|_| dependency("account lookup failed"))?;
            if account_exists {
                Ok(AssetInsert::UniqueConflict)
            } else {
                Err(ReceiptError::not_found("account not found"))
            }
        }
    }
}

async fn find_active_duplicate(
    pool: &PgPool,
    account_id: Uuid,
    content_sha256: &str,
) -> Result<Option<Uuid>, ReceiptError> {
    let submission_id = sqlx::query_scalar(
        r#"
        SELECT submission_id
        FROM receipt_assets
        WHERE account_id = $1
          AND content_sha256 = $2
          AND deletion_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(account_id)
    .bind(content_sha256)
    .fetch_optional(pool)
    .await
    .map_err(|_| dependency("duplicate lookup failed"))?;
    Ok(submission_id)
}

async fn load_submission(
    pool: &PgPool,
    submission_id: Uuid,
    account_id: Uuid,
) -> Result<SubmissionRow, ReceiptError> {
    let row: Option<SubmissionRowTuple> = sqlx::query_as(
        r#"
        SELECT
            id,
            account_id,
            lifecycle_state,
            version,
            duplicate_of_submission_id,
            confirmed_expense_id,
            failure_error_class,
            review_expires_at
        FROM receipt_submissions
        WHERE id = $1 AND account_id = $2
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| dependency("submission lookup failed"))?;

    let Some(tuple) = row else {
        return Err(ReceiptError::not_found("receipt submission not found"));
    };

    let attempt_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*)::INT FROM extraction_attempts WHERE submission_id = $1",
    )
    .bind(tuple.0)
    .fetch_one(pool)
    .await
    .map_err(|_| dependency("attempt count failed"))?;

    submission_from_tuple(tuple, attempt_count)
}

async fn load_submission_for_update(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    account_id: Uuid,
) -> Result<SubmissionRow, ReceiptError> {
    let row = load_submission_tuple_for_update(tx, submission_id, Some(account_id)).await?;

    let Some(tuple) = row else {
        return Err(ReceiptError::not_found("receipt submission not found"));
    };

    let attempt_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*)::INT FROM extraction_attempts WHERE submission_id = $1",
    )
    .bind(tuple.0)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| dependency("attempt count failed"))?;

    submission_from_tuple(tuple, attempt_count)
}

async fn load_asset(
    pool: &PgPool,
    submission_id: Uuid,
    account_id: Uuid,
) -> Result<Option<AssetRow>, ReceiptError> {
    let row: Option<(String, String)> = sqlx::query_as(
        r#"
        SELECT object_key, deletion_state
        FROM receipt_assets
        WHERE submission_id = $1 AND account_id = $2
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| dependency("asset lookup failed"))?;
    Ok(row.map(|(object_key, deletion_state)| AssetRow {
        object_key,
        deletion_state,
    }))
}

async fn asset_deleted(pool: &PgPool, submission_id: Uuid) -> Result<bool, ReceiptError> {
    let state: Option<String> =
        sqlx::query_scalar("SELECT deletion_state FROM receipt_assets WHERE submission_id = $1")
            .bind(submission_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| dependency("asset state lookup failed"))?;
    Ok(matches!(state.as_deref(), Some("deleted")))
}

async fn record_extraction_attempt(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    attempt_number: i32,
    outcome: &str,
    error_class: Option<&str>,
    latency_ms: i32,
) -> Result<(), ReceiptError> {
    sqlx::query(
        r#"
        INSERT INTO extraction_attempts (
            id, submission_id, attempt_number, provider, model, profile_name,
            prompt_version, outcome, error_class, latency_ms, input_tokens,
            output_tokens, started_at, ended_at
        )
        VALUES ($1, $2, $3, 'fake', 'fake-corpus', 'receipt-fast', 'v1', $4, $5, $6, 0, 0, NOW(), NOW())
        ON CONFLICT (submission_id, attempt_number) DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(submission_id)
    .bind(attempt_number)
    .bind(outcome)
    .bind(error_class)
    .bind(latency_ms)
    .execute(&mut **tx)
    .await
    .map_err(|_| dependency("extraction attempt insert failed"))?;
    Ok(())
}

async fn upsert_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: Uuid,
    submission_id: Uuid,
    account_id: Uuid,
    extraction: &super::types::ExtractionResult,
) -> Result<(), ReceiptError> {
    ensure_category(tx, &extraction.category_key).await?;
    sqlx::query(
        r#"
        INSERT INTO expense_drafts (
            id, submission_id, account_id, amount_minor, currency, merchant,
            category_key, transaction_type, occurred_at, confidence
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (submission_id) DO UPDATE
        SET amount_minor = EXCLUDED.amount_minor,
            currency = EXCLUDED.currency,
            merchant = EXCLUDED.merchant,
            category_key = EXCLUDED.category_key,
            transaction_type = EXCLUDED.transaction_type,
            occurred_at = EXCLUDED.occurred_at,
            confidence = EXCLUDED.confidence,
            version = expense_drafts.version + 1,
            updated_at = NOW()
        "#,
    )
    .bind(draft_id)
    .bind(submission_id)
    .bind(account_id)
    .bind(extraction.amount_minor)
    .bind(&extraction.currency)
    .bind(&extraction.merchant)
    .bind(&extraction.category_key)
    .bind(&extraction.transaction_type)
    .bind(extraction.occurred_at)
    .bind(extraction.confidence)
    .execute(&mut **tx)
    .await
    .map_err(|_| dependency("draft upsert failed"))?;
    Ok(())
}

async fn ensure_category(
    tx: &mut Transaction<'_, Postgres>,
    category_key: &str,
) -> Result<(), ReceiptError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM categories WHERE key = $1)")
        .bind(category_key)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| dependency("category lookup failed"))?;
    if !exists {
        return Err(ReceiptError::validation("unknown category key"));
    }
    Ok(())
}

async fn load_draft(
    pool: &PgPool,
    submission_id: Uuid,
    account_id: Uuid,
) -> Result<ExpenseDraftView, ReceiptError> {
    let row: Option<ExpenseDraftViewRow> = sqlx::query_as(
        r#"
        SELECT
            id,
            submission_id,
            account_id,
            amount_minor,
            currency,
            merchant,
            category_key,
            transaction_type,
            occurred_at,
            confidence,
            version
        FROM expense_drafts
        WHERE submission_id = $1 AND account_id = $2
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| dependency("draft lookup failed"))?;
    row.map(Into::into)
        .ok_or_else(|| ReceiptError::not_found("expense draft not found"))
}

async fn load_draft_for_update(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
    account_id: Uuid,
) -> Result<ExpenseDraftView, ReceiptError> {
    let row: Option<ExpenseDraftViewRow> = sqlx::query_as(
        r#"
        SELECT
            id,
            submission_id,
            account_id,
            amount_minor,
            currency,
            merchant,
            category_key,
            transaction_type,
            occurred_at,
            confidence,
            version
        FROM expense_drafts
        WHERE submission_id = $1 AND account_id = $2
        FOR UPDATE
        "#,
    )
    .bind(submission_id)
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| dependency("draft lookup failed"))?;
    row.map(Into::into)
        .ok_or_else(|| ReceiptError::not_found("expense draft not found"))
}

#[derive(Debug, sqlx::FromRow)]
struct ExpenseDraftViewRow {
    id: Uuid,
    submission_id: Uuid,
    account_id: Uuid,
    amount_minor: i64,
    currency: String,
    merchant: String,
    category_key: String,
    transaction_type: String,
    occurred_at: DateTime<Utc>,
    confidence: Option<f32>,
    version: i32,
}

impl From<ExpenseDraftViewRow> for ExpenseDraftView {
    fn from(row: ExpenseDraftViewRow) -> Self {
        Self {
            draft_id: row.id,
            submission_id: row.submission_id,
            account_id: row.account_id,
            amount_minor: row.amount_minor,
            currency: row.currency,
            merchant: row.merchant,
            category_key: row.category_key,
            transaction_type: row.transaction_type,
            occurred_at: row.occurred_at,
            confidence: row.confidence,
            version: row.version,
        }
    }
}

async fn update_draft(
    tx: &mut Transaction<'_, Postgres>,
    draft: &ExpenseDraftView,
) -> Result<(), ReceiptError> {
    ensure_category(tx, &draft.category_key).await?;
    let updated = sqlx::query(
        r#"
        UPDATE expense_drafts
        SET amount_minor = $3,
            currency = $4,
            merchant = $5,
            category_key = $6,
            occurred_at = $7,
            version = version + 1,
            updated_at = NOW()
        WHERE id = $1 AND version = $2
        "#,
    )
    .bind(draft.draft_id)
    .bind(draft.version)
    .bind(draft.amount_minor)
    .bind(&draft.currency)
    .bind(&draft.merchant)
    .bind(&draft.category_key)
    .bind(draft.occurred_at)
    .execute(&mut **tx)
    .await
    .map_err(|_| dependency("draft update failed"))?;
    if updated.rows_affected() != 1 {
        return Err(ReceiptError::conflict("draft version mismatch"));
    }
    Ok(())
}

async fn record_correction(
    tx: &mut Transaction<'_, Postgres>,
    draft_id: Uuid,
    submission_id: Uuid,
    field_name: &str,
    old_value: Option<String>,
    new_value: Option<String>,
) -> Result<(), ReceiptError> {
    sqlx::query(
        r#"
        INSERT INTO draft_corrections (
            id, draft_id, submission_id, field_name, old_value, new_value
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(draft_id)
    .bind(submission_id)
    .bind(field_name)
    .bind(old_value)
    .bind(new_value)
    .execute(&mut **tx)
    .await
    .map_err(|_| dependency("correction insert failed"))?;
    Ok(())
}

async fn lookup_confirmed_expense_id(
    tx: &mut Transaction<'_, Postgres>,
    submission_id: Uuid,
) -> Result<Uuid, ReceiptError> {
    sqlx::query_scalar("SELECT id FROM expenses WHERE receipt_submission_id = $1")
        .bind(submission_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| dependency("confirmed expense lookup failed"))?
        .ok_or_else(|| ReceiptError::dependency("confirmed expense missing"))
}

async fn insert_confirmed_expense(
    tx: &mut Transaction<'_, Postgres>,
    expense_id: Uuid,
    account_id: Uuid,
    submission_id: Uuid,
    draft: &ExpenseDraftView,
) -> Result<(), ReceiptError> {
    sqlx::query(
        r#"
        INSERT INTO expenses (
            id, account_id, amount_minor, currency, occurred_at, description,
            source, state, receipt_submission_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'receipt', 'confirmed', $7)
        ON CONFLICT (receipt_submission_id) WHERE receipt_submission_id IS NOT NULL DO NOTHING
        "#,
    )
    .bind(expense_id)
    .bind(account_id)
    .bind(draft.amount_minor)
    .bind(&draft.currency)
    .bind(draft.occurred_at)
    .bind(&draft.merchant)
    .bind(submission_id)
    .execute(&mut **tx)
    .await
    .map_err(|_| dependency("expense insert failed"))?;
    Ok(())
}

async fn enqueue_receipt_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    job_type: &str,
    submission_id: Uuid,
    account_id: Uuid,
) -> Result<EnqueueOutcome, ReceiptError> {
    let payload = ReceiptJobPayload::new(submission_id).to_value();
    let dedupe_key = if job_type == JOB_TYPE_INGEST {
        ingest_dedupe_key(submission_id)
    } else {
        extract_dedupe_key(submission_id)
    };
    let enqueue = EnqueueRequest {
        id: job_id,
        job_type: job_type.to_string(),
        payload,
        dedupe_key,
        serialization_key: Some(account_serialization_key(account_id)),
        priority: 0,
        run_at: Utc::now(),
        max_attempts: 10,
    };
    let outcome = WorkStore::enqueue_in_transaction(tx, enqueue)
        .await
        .map_err(map_work_error)?;
    if outcome == EnqueueOutcome::Enqueued {
        sqlx::query("UPDATE jobs SET run_at = NOW() WHERE id = $1")
            .bind(job_id)
            .execute(&mut **tx)
            .await
            .map_err(|_| dependency("job run_at update failed"))?;
    }
    Ok(outcome)
}

fn failure_state_for(class: ErrorClass) -> ReceiptState {
    match class {
        ErrorClass::Transient
        | ErrorClass::Timeout
        | ErrorClass::RateLimited
        | ErrorClass::Dependency => ReceiptState::FailedTransient,
        _ => ReceiptState::FailedPermanent,
    }
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(db_error) if db_error.code().as_deref() == Some("23505")
    )
}

fn dependency(message: &str) -> ReceiptError {
    ReceiptError::dependency(message)
}

fn map_work_error(error: crate::work::WorkError) -> ReceiptError {
    ReceiptError::new(error.class, error.message)
}
