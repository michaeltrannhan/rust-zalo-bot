//! Conservative Milestone 2 outbox delivery and Milestone 3 job-scoped execution.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ErrorClass;
use crate::provider::ZaloHttpAdapter;
use crate::work::ClaimedJob;

const OUTBOUND_DELIVER_JOB_TYPE: &str = "outbound.deliver";
const OUTBOUND_DELIVER_SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    Sent,
    Failed,
    Ambiguous,
}

impl DeliveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryResult {
    pub outbound_id: Uuid,
    pub state: DeliveryState,
}

/// Result of executing one leased `outbound.deliver` job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundJobExecution {
    /// Outbound reached a terminal observation; the runtime should complete the job.
    Complete(DeliveryResult),
    /// Provider returned a definite error class for job retry or dead-letter handling.
    Fail(ErrorClass),
    /// The job lease was stale before or after the HTTP effect.
    StaleLease,
    /// Job type, payload version, or payload shape was invalid.
    InvalidJob,
}

#[derive(Debug, thiserror::Error)]
#[error("outbound persistence operation failed")]
pub struct OutboundStoreError;

/// Reserve and deliver at most one queued Zalo message.
///
/// Reservation commits before HTTP. An indeterminate provider result becomes
/// `ambiguous`; a crash after reservation leaves `sending`. Neither state is
/// selected again automatically, so M2 cannot blindly duplicate an effect.
pub async fn deliver_next(
    pool: &PgPool,
    adapter: &ZaloHttpAdapter,
) -> Result<Option<DeliveryResult>, OutboundStoreError> {
    let reserved: Option<(Uuid, String, String)> = sqlx::query_as(
        r#"
        WITH candidate AS (
            SELECT id
            FROM outbound_messages
            WHERE state = 'queued' AND provider_scope = 'zalo_bot'
            ORDER BY created_at, id
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE outbound_messages AS outbound
        SET state = 'sending',
            attempt_count = attempt_count + 1,
            updated_at = NOW()
        FROM candidate
        WHERE outbound.id = candidate.id
        RETURNING outbound.id, outbound.provider_target, outbound.body
        "#,
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| OutboundStoreError)?;

    let Some((outbound_id, provider_target, body)) = reserved else {
        return Ok(None);
    };

    match adapter.send_message(&provider_target, &body).await {
        Ok(sent) => {
            let provider_message_id =
                (!sent.provider_message_id.is_empty()).then_some(sent.provider_message_id);
            update_delivery(
                pool,
                outbound_id,
                DeliveryState::Sent,
                provider_message_id.as_deref(),
                None,
            )
            .await?;
            Ok(Some(DeliveryResult {
                outbound_id,
                state: DeliveryState::Sent,
            }))
        }
        Err(error) => {
            let state = if error.class == ErrorClass::ProviderAmbiguous {
                DeliveryState::Ambiguous
            } else {
                DeliveryState::Failed
            };
            update_delivery(pool, outbound_id, state, None, Some(error.class.as_str())).await?;
            Ok(Some(DeliveryResult { outbound_id, state }))
        }
    }
}

/// Execute one leased `outbound.deliver` job against the exact outbound row in its payload.
pub async fn deliver_for_job(
    pool: &PgPool,
    adapter: &ZaloHttpAdapter,
    job: &ClaimedJob,
) -> Result<OutboundJobExecution, OutboundStoreError> {
    let outbound_id = match parse_outbound_id(job) {
        Ok(outbound_id) => outbound_id,
        Err(outcome) => return Ok(outcome),
    };

    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT state, provider_target, body FROM outbound_messages WHERE id = $1")
            .bind(outbound_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| OutboundStoreError)?;

    let Some(state) = row.map(|(state, _, _)| state) else {
        return Ok(OutboundJobExecution::InvalidJob);
    };

    match state.as_str() {
        "sent" => {
            return Ok(OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: DeliveryState::Sent,
            }));
        }
        "ambiguous" => {
            return Ok(OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: DeliveryState::Ambiguous,
            }));
        }
        "sending" => {
            if !job_lease_current(pool, job.id, job.lease_token).await? {
                return Ok(OutboundJobExecution::StaleLease);
            }
            if !mark_ambiguous_under_lease(pool, job.id, job.lease_token, outbound_id).await? {
                return Ok(OutboundJobExecution::StaleLease);
            }
            return Ok(OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: DeliveryState::Ambiguous,
            }));
        }
        "queued" | "failed" => {}
        _ => return Ok(OutboundJobExecution::InvalidJob),
    }

    let reserved = reserve_outbound_under_lease(pool, job.id, job.lease_token, outbound_id).await?;

    let Some((provider_target, body)) = reserved else {
        if !job_lease_current(pool, job.id, job.lease_token).await? {
            return Ok(OutboundJobExecution::StaleLease);
        }
        let refreshed: Option<String> =
            sqlx::query_scalar("SELECT state FROM outbound_messages WHERE id = $1")
                .bind(outbound_id)
                .fetch_optional(pool)
                .await
                .map_err(|_| OutboundStoreError)?;
        return Ok(match refreshed.as_deref() {
            Some("sent") => OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: DeliveryState::Sent,
            }),
            Some("ambiguous") => OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: DeliveryState::Ambiguous,
            }),
            _ => OutboundJobExecution::StaleLease,
        });
    };

    let send_result = adapter.send_message(&provider_target, &body).await;

    if !job_lease_current(pool, job.id, job.lease_token).await? {
        return Ok(OutboundJobExecution::StaleLease);
    }

    match send_result {
        Ok(sent) => {
            let provider_message_id =
                (!sent.provider_message_id.is_empty()).then_some(sent.provider_message_id);
            let updated = update_delivery_under_lease(
                pool,
                job.id,
                job.lease_token,
                outbound_id,
                DeliveryState::Sent,
                provider_message_id.as_deref(),
                None,
            )
            .await?;
            if !updated {
                return Ok(OutboundJobExecution::StaleLease);
            }
            Ok(OutboundJobExecution::Complete(DeliveryResult {
                outbound_id,
                state: DeliveryState::Sent,
            }))
        }
        Err(error) => {
            let state = if error.class == ErrorClass::ProviderAmbiguous {
                DeliveryState::Ambiguous
            } else {
                DeliveryState::Failed
            };
            let updated = update_delivery_under_lease(
                pool,
                job.id,
                job.lease_token,
                outbound_id,
                state,
                None,
                Some(error.class.as_str()),
            )
            .await?;
            if !updated {
                return Ok(OutboundJobExecution::StaleLease);
            }
            if state == DeliveryState::Ambiguous {
                Ok(OutboundJobExecution::Complete(DeliveryResult {
                    outbound_id,
                    state: DeliveryState::Ambiguous,
                }))
            } else {
                Ok(OutboundJobExecution::Fail(error.class))
            }
        }
    }
}

fn parse_outbound_id(job: &ClaimedJob) -> Result<Uuid, OutboundJobExecution> {
    if job.job_type != OUTBOUND_DELIVER_JOB_TYPE {
        return Err(OutboundJobExecution::InvalidJob);
    }
    if job.payload_version != OUTBOUND_DELIVER_SCHEMA_VERSION {
        return Err(OutboundJobExecution::InvalidJob);
    }
    job.payload
        .get("outbound_id")
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(OutboundJobExecution::InvalidJob)
}

async fn job_lease_current(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
) -> Result<bool, OutboundStoreError> {
    let current: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM jobs
            WHERE id = $1
              AND lease_token = $2
              AND state = 'leased'
              AND lease_deadline >= NOW()
        )
        "#,
    )
    .bind(job_id)
    .bind(lease_token)
    .fetch_one(pool)
    .await
    .map_err(|_| OutboundStoreError)?;
    Ok(current)
}

async fn reserve_outbound_under_lease(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
    outbound_id: Uuid,
) -> Result<Option<(String, String)>, OutboundStoreError> {
    sqlx::query_as(
        r#"
        UPDATE outbound_messages AS outbound
        SET state = 'sending',
            attempt_count = outbound.attempt_count + 1,
            updated_at = NOW()
        WHERE outbound.id = $1
          AND outbound.state IN ('queued', 'failed')
          AND EXISTS (
              SELECT 1
              FROM jobs
              WHERE jobs.id = $2
                AND jobs.lease_token = $3
                AND jobs.state = 'leased'
                AND jobs.lease_deadline >= NOW()
          )
        RETURNING outbound.provider_target, outbound.body
        "#,
    )
    .bind(outbound_id)
    .bind(job_id)
    .bind(lease_token)
    .fetch_optional(pool)
    .await
    .map_err(|_| OutboundStoreError)
}

async fn mark_ambiguous_under_lease(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
    outbound_id: Uuid,
) -> Result<bool, OutboundStoreError> {
    let updated = sqlx::query(
        r#"
        UPDATE outbound_messages
        SET state = 'ambiguous',
            last_error_class = $4,
            ambiguity_metadata = jsonb_build_object(
                'reason', 'recovered_after_unfinished_send'
            ),
            updated_at = NOW()
        WHERE id = $1
          AND state = 'sending'
          AND EXISTS (
              SELECT 1
              FROM jobs
              WHERE jobs.id = $2
                AND jobs.lease_token = $3
                AND jobs.state = 'leased'
                AND jobs.lease_deadline >= NOW()
          )
        "#,
    )
    .bind(outbound_id)
    .bind(job_id)
    .bind(lease_token)
    .bind(ErrorClass::ProviderAmbiguous.as_str())
    .execute(pool)
    .await
    .map_err(|_| OutboundStoreError)?;
    Ok(updated.rows_affected() == 1)
}

async fn update_delivery(
    pool: &PgPool,
    outbound_id: Uuid,
    state: DeliveryState,
    provider_message_id: Option<&str>,
    error_class: Option<&str>,
) -> Result<(), OutboundStoreError> {
    let updated = sqlx::query(
        r#"
        UPDATE outbound_messages
        SET state = $2,
            provider_message_id = $3,
            last_error_class = $4,
            updated_at = NOW()
        WHERE id = $1 AND state = 'sending'
        "#,
    )
    .bind(outbound_id)
    .bind(state.as_str())
    .bind(provider_message_id)
    .bind(error_class)
    .execute(pool)
    .await
    .map_err(|_| OutboundStoreError)?;
    if updated.rows_affected() != 1 {
        return Err(OutboundStoreError);
    }
    Ok(())
}

async fn update_delivery_under_lease(
    pool: &PgPool,
    job_id: Uuid,
    lease_token: Uuid,
    outbound_id: Uuid,
    state: DeliveryState,
    provider_message_id: Option<&str>,
    error_class: Option<&str>,
) -> Result<bool, OutboundStoreError> {
    let updated = sqlx::query(
        r#"
        UPDATE outbound_messages
        SET state = $2,
            provider_message_id = $3,
            last_error_class = $4,
            updated_at = NOW()
        WHERE id = $1
          AND state = 'sending'
          AND EXISTS (
              SELECT 1
              FROM jobs
              WHERE jobs.id = $5
                AND jobs.lease_token = $6
                AND jobs.state = 'leased'
                AND jobs.lease_deadline >= NOW()
          )
        "#,
    )
    .bind(outbound_id)
    .bind(state.as_str())
    .bind(provider_message_id)
    .bind(error_class)
    .bind(job_id)
    .bind(lease_token)
    .execute(pool)
    .await
    .map_err(|_| OutboundStoreError)?;
    Ok(updated.rows_affected() == 1)
}
