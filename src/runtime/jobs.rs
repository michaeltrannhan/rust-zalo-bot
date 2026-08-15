//! Leased job dispatch for outbound delivery and receipt processing.

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ErrorClass;
use crate::ingress::enqueue_receipt_review_followup;
use crate::outbound::{DeliveryResult, DeliveryState, OutboundJobExecution, deliver_for_job};
use crate::provider::{
    MediaHostResolver, SystemMediaResolver, ZaloHttpAdapter, ZaloMediaDownloader,
};
use crate::receipt::{
    ExtractOutcome, IngestOutcome, JOB_TYPE_EXTRACT, JOB_TYPE_INGEST, ReceiptJobPayload,
    ReceiptLifecycle, extract_dedupe_key, ingest_dedupe_key,
};
use crate::work::ClaimedJob;

const RECEIPT_PAYLOAD_VERSION: i32 = 1;

/// Injectable dependencies for leased job execution.
pub struct JobDeps<R: MediaHostResolver = SystemMediaResolver> {
    pub pool: PgPool,
    pub adapter: Arc<ZaloHttpAdapter>,
    pub receipt: ReceiptLifecycle,
    pub media_downloader: ZaloMediaDownloader<R>,
}

impl JobDeps<SystemMediaResolver> {
    pub fn production(
        pool: PgPool,
        adapter: Arc<ZaloHttpAdapter>,
        receipt: ReceiptLifecycle,
    ) -> Self {
        Self {
            pool,
            adapter,
            receipt,
            media_downloader: ZaloMediaDownloader::new(
                crate::provider::MediaDownloadPolicy::production_default(),
                SystemMediaResolver,
            ),
        }
    }
}

impl<R: MediaHostResolver> JobDeps<R> {
    pub fn new(
        pool: PgPool,
        adapter: Arc<ZaloHttpAdapter>,
        receipt: ReceiptLifecycle,
        media_downloader: ZaloMediaDownloader<R>,
    ) -> Self {
        Self {
            pool,
            adapter,
            receipt,
            media_downloader,
        }
    }
}

/// Execute one leased job using injected runtime dependencies.
pub async fn dispatch_leased_job<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    match job.job_type.as_str() {
        "outbound.deliver" => match deliver_for_job(&deps.pool, &deps.adapter, job).await {
            Ok(execution) => execution,
            Err(_) => OutboundJobExecution::Fail(ErrorClass::Dependency),
        },
        JOB_TYPE_INGEST => dispatch_receipt_ingest(deps, job).await,
        JOB_TYPE_EXTRACT => dispatch_receipt_extract(deps, job).await,
        _ => OutboundJobExecution::InvalidJob,
    }
}

async fn dispatch_receipt_ingest<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    let submission_id = match parse_receipt_submission_id(job, JOB_TYPE_INGEST) {
        Ok(submission_id) => submission_id,
        Err(execution) => return execution,
    };
    if job.dedupe_key != ingest_dedupe_key(submission_id) {
        return OutboundJobExecution::InvalidJob;
    }

    let Some((account_id, media_url)) = load_submission_media_url(&deps.pool, submission_id).await
    else {
        return OutboundJobExecution::InvalidJob;
    };

    let download = deps.media_downloader.download(&media_url).await;
    let downloaded = match download {
        Ok(result) => result,
        Err(error) => {
            if is_permanent_media_failure(error.class) {
                let _ = deps
                    .receipt
                    .fail_queued(submission_id, account_id, error.class)
                    .await;
            }
            return OutboundJobExecution::Fail(error.class);
        }
    };

    let mime_type = downloaded
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");

    match deps
        .receipt
        .ingest(
            account_id,
            submission_id,
            &downloaded.bytes,
            mime_type,
            Uuid::new_v4(),
        )
        .await
    {
        Ok(
            IngestOutcome::Stored { .. }
            | IngestOutcome::AlreadyStored { .. }
            | IngestOutcome::DuplicateAbsorbed { .. }
            | IngestOutcome::AlreadyTerminal { .. },
        ) => receipt_job_complete(),
        Err(error) => {
            if error.class == ErrorClass::Validation {
                let _ = deps
                    .receipt
                    .fail_queued(submission_id, account_id, error.class)
                    .await;
            }
            OutboundJobExecution::Fail(error.class)
        }
    }
}

async fn dispatch_receipt_extract<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    let submission_id = match parse_receipt_submission_id(job, JOB_TYPE_EXTRACT) {
        Ok(submission_id) => submission_id,
        Err(execution) => return execution,
    };
    if job.dedupe_key != extract_dedupe_key(submission_id) {
        return OutboundJobExecution::InvalidJob;
    }

    let account_id = match load_submission_account_id(&deps.pool, submission_id).await {
        Some(account_id) => account_id,
        None => return OutboundJobExecution::InvalidJob,
    };

    match deps.receipt.extract(account_id, submission_id).await {
        Ok(ExtractOutcome::ReviewRequired { .. })
        | Ok(ExtractOutcome::AlreadyReviewRequired { .. }) => {
            if enqueue_receipt_review_followup(&deps.pool, &deps.receipt, account_id, submission_id)
                .await
                .is_err()
            {
                return OutboundJobExecution::Fail(ErrorClass::Dependency);
            }
            receipt_job_complete()
        }
        Ok(ExtractOutcome::AlreadyTerminal { .. }) => receipt_job_complete(),
        Ok(ExtractOutcome::Unsupported) => OutboundJobExecution::Fail(ErrorClass::Unsupported),
        Err(error) => OutboundJobExecution::Fail(error.class),
    }
}

fn parse_receipt_submission_id(
    job: &ClaimedJob,
    expected_type: &str,
) -> Result<Uuid, OutboundJobExecution> {
    if job.job_type != expected_type || job.payload_version != RECEIPT_PAYLOAD_VERSION {
        return Err(OutboundJobExecution::InvalidJob);
    }
    let payload: ReceiptJobPayload = serde_json::from_value(job.payload.clone())
        .map_err(|_| OutboundJobExecution::InvalidJob)?;
    if payload.schema_version != RECEIPT_PAYLOAD_VERSION {
        return Err(OutboundJobExecution::InvalidJob);
    }
    Ok(payload.receipt_submission_id)
}

async fn load_submission_media_url(pool: &PgPool, submission_id: Uuid) -> Option<(Uuid, String)> {
    sqlx::query_as(
        r#"
        SELECT rs.account_id, ie.media_url
        FROM receipt_submissions rs
        JOIN inbound_events ie ON ie.id = rs.inbound_event_id
        WHERE rs.id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .and_then(|(account_id, media_url): (Uuid, Option<String>)| {
        media_url.map(|url| (account_id, url))
    })
}

async fn load_submission_account_id(pool: &PgPool, submission_id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT account_id FROM receipt_submissions WHERE id = $1")
        .bind(submission_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

fn is_permanent_media_failure(class: ErrorClass) -> bool {
    matches!(
        class,
        ErrorClass::Validation | ErrorClass::Unsupported | ErrorClass::Forbidden
    )
}

fn receipt_job_complete() -> OutboundJobExecution {
    OutboundJobExecution::Complete(DeliveryResult {
        outbound_id: Uuid::nil(),
        state: DeliveryState::Sent,
    })
}
