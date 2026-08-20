//! Leased job dispatch for outbound delivery and receipt processing.

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use crate::account::{
    ACCOUNT_JOB_PAYLOAD_VERSION, AccountDeletePayload, AccountExportPayload,
    JOB_TYPE_ACCOUNT_DELETE, JOB_TYPE_ACCOUNT_EXPORT, execute_account_delete,
    execute_account_export,
};
use crate::conversation::{empty_summary_text, today_summary_text};
use crate::error::ErrorClass;
use crate::ingress::{
    IngressPolicy, enqueue_outbound_in_transaction, enqueue_receipt_failure_followup,
    enqueue_receipt_review_followup,
};
use crate::insight::{
    FakeNarrator, INSIGHT_NARRATE_PAYLOAD_VERSION, InsightNarratePayload, InsightNarrator,
    JOB_TYPE_INSIGHT_NARRATE, execute_insight_narrate, narrate_dedupe_key,
};
use crate::outbound::{DeliveryResult, DeliveryState, OutboundJobExecution, deliver_for_job};
use crate::provider::{
    MediaHostResolver, SystemMediaResolver, ZaloHttpAdapter, ZaloMediaDownloader,
};
use crate::receipt::{
    ExtractOutcome, IngestOutcome, JOB_TYPE_EXTRACT, JOB_TYPE_INGEST, ReceiptJobPayload,
    ReceiptLifecycle, extract_dedupe_key, ingest_dedupe_key,
};
use crate::schedule::{
    JOB_TYPE_SCHEDULE_EMIT, SCHEDULE_PAYLOAD_VERSION, ScheduleEmitPayload, schedule_emit_dedupe_key,
};
use crate::work::ClaimedJob;

const RECEIPT_PAYLOAD_VERSION: i32 = 1;

/// Injectable dependencies for leased job execution.
pub struct JobDeps<R: MediaHostResolver = SystemMediaResolver> {
    pub pool: PgPool,
    pub adapter: Arc<ZaloHttpAdapter>,
    pub receipt: ReceiptLifecycle,
    pub ingress_policy: IngressPolicy,
    pub media_downloader: ZaloMediaDownloader<R>,
    pub insight_narrator: Arc<dyn InsightNarrator>,
}

impl JobDeps<SystemMediaResolver> {
    pub fn production(
        pool: PgPool,
        adapter: Arc<ZaloHttpAdapter>,
        receipt: ReceiptLifecycle,
        ingress_policy: IngressPolicy,
    ) -> Self {
        Self {
            pool,
            adapter,
            receipt,
            ingress_policy,
            media_downloader: ZaloMediaDownloader::new(
                crate::provider::MediaDownloadPolicy::production_default(),
                SystemMediaResolver,
            ),
            insight_narrator: Arc::new(FakeNarrator),
        }
    }
}

impl<R: MediaHostResolver> JobDeps<R> {
    pub fn new(
        pool: PgPool,
        adapter: Arc<ZaloHttpAdapter>,
        receipt: ReceiptLifecycle,
        ingress_policy: IngressPolicy,
        media_downloader: ZaloMediaDownloader<R>,
        insight_narrator: Arc<dyn InsightNarrator>,
    ) -> Self {
        Self {
            pool,
            adapter,
            receipt,
            ingress_policy,
            media_downloader,
            insight_narrator,
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
        JOB_TYPE_SCHEDULE_EMIT => dispatch_schedule_emit(deps, job).await,
        JOB_TYPE_ACCOUNT_DELETE => dispatch_account_delete(deps, job).await,
        JOB_TYPE_ACCOUNT_EXPORT => dispatch_account_export(deps, job).await,
        JOB_TYPE_INSIGHT_NARRATE => dispatch_insight_narrate(deps, job).await,
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
                let _ = enqueue_receipt_failure_followup(
                    &deps.pool,
                    &deps.ingress_policy,
                    account_id,
                    submission_id,
                    error.class,
                )
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
            if error.class == ErrorClass::Validation || error.class == ErrorClass::Unsupported {
                let _ = deps
                    .receipt
                    .fail_queued(submission_id, account_id, error.class)
                    .await;
                let _ = enqueue_receipt_failure_followup(
                    &deps.pool,
                    &deps.ingress_policy,
                    account_id,
                    submission_id,
                    error.class,
                )
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
            if enqueue_receipt_review_followup(
                &deps.pool,
                &deps.receipt,
                &deps.ingress_policy,
                account_id,
                submission_id,
            )
            .await
            .is_err()
            {
                return OutboundJobExecution::Fail(ErrorClass::Dependency);
            }
            receipt_job_complete()
        }
        Ok(ExtractOutcome::AlreadyTerminal { .. }) => receipt_job_complete(),
        Ok(ExtractOutcome::Unsupported) => {
            let _ = enqueue_receipt_failure_followup(
                &deps.pool,
                &deps.ingress_policy,
                account_id,
                submission_id,
                ErrorClass::Unsupported,
            )
            .await;
            OutboundJobExecution::Fail(ErrorClass::Unsupported)
        }
        Err(error) => {
            if matches!(
                error.class,
                ErrorClass::Validation
                    | ErrorClass::Unsupported
                    | ErrorClass::KillSwitch
                    | ErrorClass::QuotaExceeded
                    | ErrorClass::Auth
            ) {
                let _ = enqueue_receipt_failure_followup(
                    &deps.pool,
                    &deps.ingress_policy,
                    account_id,
                    submission_id,
                    error.class,
                )
                .await;
            }
            OutboundJobExecution::Fail(error.class)
        }
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

async fn dispatch_schedule_emit<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    if job.job_type != JOB_TYPE_SCHEDULE_EMIT || job.payload_version != SCHEDULE_PAYLOAD_VERSION {
        return OutboundJobExecution::InvalidJob;
    }

    let payload: ScheduleEmitPayload = match serde_json::from_value(job.payload.clone()) {
        Ok(payload) => payload,
        Err(_) => return OutboundJobExecution::InvalidJob,
    };
    if payload.schema_version != SCHEDULE_PAYLOAD_VERSION {
        return OutboundJobExecution::InvalidJob;
    }
    if job.dedupe_key
        != schedule_emit_dedupe_key(payload.account_id, &payload.frequency, payload.period_start)
    {
        return OutboundJobExecution::InvalidJob;
    }

    let lifecycle: Option<String> =
        sqlx::query_scalar("SELECT lifecycle_state FROM accounts WHERE id = $1")
            .bind(payload.account_id)
            .fetch_optional(&deps.pool)
            .await
            .ok()
            .flatten();
    if matches!(
        lifecycle.as_deref(),
        Some("suspended") | Some("deleting") | Some("deleted") | None
    ) {
        return receipt_job_complete();
    }

    let schedule_row: Option<(String, String, String)> = sqlx::query_as(
        r#"
        SELECT provider_scope, provider_chat_id, frequency
        FROM summary_schedules
        WHERE id = $1 AND account_id = $2 AND enabled = TRUE
        "#,
    )
    .bind(payload.schedule_id)
    .bind(payload.account_id)
    .fetch_optional(&deps.pool)
    .await
    .ok()
    .flatten();
    let Some((provider_scope, provider_chat_id, frequency)) = schedule_row else {
        return receipt_job_complete();
    };
    if frequency != payload.frequency {
        return OutboundJobExecution::InvalidJob;
    }

    let totals: Option<(i64, i64, String)> = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(amount_minor), 0)::BIGINT,
               COUNT(*)::BIGINT,
               COALESCE(MIN(currency), 'VND')
        FROM expenses
        WHERE account_id = $1
          AND state = 'confirmed'
          AND occurred_at >= $2
          AND occurred_at < $3
        "#,
    )
    .bind(payload.account_id)
    .bind(payload.period_start)
    .bind(payload.period_end)
    .fetch_optional(&deps.pool)
    .await
    .ok()
    .flatten();
    let (total_minor, tx_count, currency) = totals.unwrap_or((0, 0, "VND".to_string()));

    let label = scheduled_period_label(&payload.frequency);
    let body = if tx_count == 0 {
        empty_summary_text(label)
    } else {
        today_summary_text(label, &currency, total_minor)
    };

    let idempotency_key = format!(
        "scheduled-summary:{}:{}:{}",
        payload.account_id,
        payload.frequency,
        payload.period_start.format("%Y%m%dT%H%M%SZ")
    );

    let mut tx = match deps.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return OutboundJobExecution::Fail(ErrorClass::Dependency),
    };
    if enqueue_outbound_in_transaction(
        &mut tx,
        &deps.ingress_policy,
        chrono::Utc::now(),
        Some(payload.account_id),
        None,
        &provider_scope,
        &provider_chat_id,
        &body,
        &idempotency_key,
    )
    .await
    .is_err()
    {
        return OutboundJobExecution::Fail(ErrorClass::Dependency);
    }
    if tx.commit().await.is_err() {
        return OutboundJobExecution::Fail(ErrorClass::Dependency);
    }

    receipt_job_complete()
}

async fn dispatch_account_delete<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    if job.job_type != JOB_TYPE_ACCOUNT_DELETE || job.payload_version != ACCOUNT_JOB_PAYLOAD_VERSION
    {
        return OutboundJobExecution::InvalidJob;
    }
    let payload: AccountDeletePayload = match serde_json::from_value(job.payload.clone()) {
        Ok(payload) => payload,
        Err(_) => return OutboundJobExecution::InvalidJob,
    };
    match execute_account_delete(&deps.pool, deps.receipt.object_store().as_ref(), &payload).await {
        Ok(()) => receipt_job_complete(),
        Err(ErrorClass::Validation) => OutboundJobExecution::InvalidJob,
        Err(class) => OutboundJobExecution::Fail(class),
    }
}

async fn dispatch_account_export<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    if job.job_type != JOB_TYPE_ACCOUNT_EXPORT || job.payload_version != ACCOUNT_JOB_PAYLOAD_VERSION
    {
        return OutboundJobExecution::InvalidJob;
    }
    let payload: AccountExportPayload = match serde_json::from_value(job.payload.clone()) {
        Ok(payload) => payload,
        Err(_) => return OutboundJobExecution::InvalidJob,
    };
    match execute_account_export(&deps.pool, deps.receipt.object_store().as_ref(), &payload).await {
        Ok(()) => receipt_job_complete(),
        Err(ErrorClass::Validation) => OutboundJobExecution::InvalidJob,
        Err(class) => OutboundJobExecution::Fail(class),
    }
}

async fn dispatch_insight_narrate<R: MediaHostResolver>(
    deps: &JobDeps<R>,
    job: &ClaimedJob,
) -> OutboundJobExecution {
    if !deps.ingress_policy.insights_llm_enabled {
        return receipt_job_complete();
    }
    if job.job_type != JOB_TYPE_INSIGHT_NARRATE
        || job.payload_version != INSIGHT_NARRATE_PAYLOAD_VERSION
    {
        return OutboundJobExecution::InvalidJob;
    }
    let payload: InsightNarratePayload = match serde_json::from_value(job.payload.clone()) {
        Ok(payload) => payload,
        Err(_) => return OutboundJobExecution::InvalidJob,
    };
    if payload.schema_version != INSIGHT_NARRATE_PAYLOAD_VERSION {
        return OutboundJobExecution::InvalidJob;
    }
    if job.dedupe_key != narrate_dedupe_key(payload.snapshot_id, &payload.aggregate_digest) {
        return OutboundJobExecution::InvalidJob;
    }
    match execute_insight_narrate(
        &deps.pool,
        deps.insight_narrator.as_ref(),
        &payload,
        deps.ingress_policy.monthly_insight_narratives,
    )
    .await
    {
        Ok(()) => receipt_job_complete(),
        Err(_) => OutboundJobExecution::Fail(ErrorClass::Dependency),
    }
}

fn scheduled_period_label(frequency: &str) -> &'static str {
    match frequency {
        "daily" => "Hôm qua",
        "weekly" => "Tuần trước",
        "monthly" => "Tháng trước",
        _ => "Tổng kết",
    }
}
