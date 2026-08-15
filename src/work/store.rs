//! PostgreSQL-backed durable work queue.

use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::HashSet;
use uuid::Uuid;

use crate::error::ErrorClass;

use super::error::WorkError;
use super::types::{
    AttemptOutcome, AttemptSummary, ClaimOptions, ClaimedJob, EnqueueOutcome, EnqueueRequest,
    FailOutcome, JobState, JobSummary, is_retryable, retry_delay_secs,
};

/// Transactional durable-work store.
#[derive(Clone)]
pub struct WorkStore {
    pool: PgPool,
}

impl WorkStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn enqueue(&self, request: EnqueueRequest) -> Result<EnqueueOutcome, WorkError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| WorkError::dependency("failed to begin enqueue transaction"))?;
        let outcome = Self::enqueue_in_transaction(&mut tx, request).await?;
        tx.commit()
            .await
            .map_err(|_| WorkError::dependency("enqueue commit failed"))?;
        Ok(outcome)
    }

    /// Enqueue inside the caller's domain transaction.
    pub async fn enqueue_in_transaction(
        tx: &mut Transaction<'_, Postgres>,
        request: EnqueueRequest,
    ) -> Result<EnqueueOutcome, WorkError> {
        let payload_version = validate_enqueue_request(&request)?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO jobs (
                id, job_type, payload, payload_version, state, priority, run_at,
                dedupe_key, serialization_key, max_attempts
            )
            VALUES ($1, $2, $3, $4, 'queued', $5, $6, $7, $8, $9)
            ON CONFLICT (dedupe_key) DO NOTHING
            "#,
        )
        .bind(request.id)
        .bind(&request.job_type)
        .bind(&request.payload)
        .bind(payload_version)
        .bind(request.priority)
        .bind(request.run_at)
        .bind(&request.dedupe_key)
        .bind(&request.serialization_key)
        .bind(request.max_attempts)
        .execute(&mut **tx)
        .await
        .map_err(|error| map_insert_error(error, "enqueue failed"))?;

        if inserted.rows_affected() == 1 {
            return Ok(EnqueueOutcome::Enqueued);
        }
        Ok(EnqueueOutcome::Duplicate)
    }

    pub async fn claim(&self, options: ClaimOptions) -> Result<Vec<ClaimedJob>, WorkError> {
        if options.batch_limit <= 0 {
            return Err(WorkError::validation("batch_limit must be positive"));
        }
        if options.lease_owner.is_empty() {
            return Err(WorkError::validation("lease_owner must not be empty"));
        }
        if options.lease_duration_secs <= 0 {
            return Err(WorkError::validation(
                "lease_duration_secs must be positive",
            ));
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| WorkError::dependency("failed to begin claim transaction"))?;

        let candidates = sqlx::query_as::<_, CandidateRow>(
            r#"
            SELECT
                id,
                job_type,
                payload,
                payload_version,
                dedupe_key,
                serialization_key,
                state,
                attempt_count
            FROM jobs
            WHERE (
                (state = 'queued' AND run_at <= NOW())
                OR (state = 'leased' AND lease_deadline < NOW())
            )
              AND (
                  serialization_key IS NULL
                  OR state = 'leased'
                  OR NOT EXISTS (
                      SELECT 1
                      FROM jobs AS active
                      WHERE active.serialization_key = jobs.serialization_key
                        AND active.state = 'leased'
                  )
              )
            ORDER BY priority DESC, run_at ASC, created_at ASC
            FOR UPDATE SKIP LOCKED
            LIMIT $1
            "#,
        )
        .bind(options.batch_limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| WorkError::dependency("claim candidate selection failed"))?;

        let lease_duration = Duration::seconds(options.lease_duration_secs);
        let mut claimed = Vec::with_capacity(candidates.len());
        let mut claimed_serialization_keys = HashSet::new();

        for candidate in candidates {
            if let Some(serialization_key) = &candidate.serialization_key {
                if !claimed_serialization_keys.insert(serialization_key.clone()) {
                    continue;
                }
                let acquired: bool =
                    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
                        .bind(serialization_key)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|_| WorkError::dependency("serialization lock failed"))?;
                if !acquired {
                    continue;
                }
            }
            if candidate.state == JobState::Leased {
                close_open_attempt(&mut tx, candidate.id, AttemptOutcome::LostLease, None).await?;
            }

            let attempt_number = candidate.attempt_count + 1;
            let lease_token = Uuid::new_v4();
            let lease_deadline = Utc::now() + lease_duration;
            let attempt_id = Uuid::new_v4();

            let updated = sqlx::query(
                r#"
                UPDATE jobs
                SET state = 'leased',
                    attempt_count = $2,
                    lease_token = $3,
                    lease_owner = $4,
                    lease_deadline = $5,
                    updated_at = NOW()
                WHERE id = $1
                  AND (
                    (state = 'queued' AND run_at <= NOW())
                    OR (state = 'leased' AND lease_deadline < NOW())
                  )
                "#,
            )
            .bind(candidate.id)
            .bind(attempt_number)
            .bind(lease_token)
            .bind(&options.lease_owner)
            .bind(lease_deadline)
            .execute(&mut *tx)
            .await
            .map_err(|_| WorkError::dependency("claim update failed"))?;

            if updated.rows_affected() != 1 {
                continue;
            }

            sqlx::query(
                r#"
                INSERT INTO job_attempts (
                    id, job_id, attempt_number, lease_token, lease_owner
                )
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(attempt_id)
            .bind(candidate.id)
            .bind(attempt_number)
            .bind(lease_token)
            .bind(&options.lease_owner)
            .execute(&mut *tx)
            .await
            .map_err(|_| WorkError::dependency("claim attempt insert failed"))?;

            claimed.push(ClaimedJob {
                id: candidate.id,
                job_type: candidate.job_type,
                payload: candidate.payload,
                payload_version: candidate.payload_version,
                attempt_number,
                lease_token,
                lease_owner: options.lease_owner.clone(),
                lease_deadline,
                dedupe_key: candidate.dedupe_key,
                serialization_key: candidate.serialization_key,
            });
        }

        tx.commit()
            .await
            .map_err(|_| WorkError::dependency("claim commit failed"))?;

        Ok(claimed)
    }

    pub async fn heartbeat(
        &self,
        job_id: Uuid,
        lease_token: Uuid,
        lease_duration_secs: i64,
    ) -> Result<DateTime<Utc>, WorkError> {
        if lease_duration_secs <= 0 {
            return Err(WorkError::validation(
                "lease_duration_secs must be positive",
            ));
        }

        let lease_deadline = Utc::now() + Duration::seconds(lease_duration_secs);
        let updated = sqlx::query_scalar::<_, DateTime<Utc>>(
            r#"
            UPDATE jobs
            SET lease_deadline = $3,
                updated_at = NOW()
            WHERE id = $1
              AND lease_token = $2
              AND state = 'leased'
              AND lease_deadline >= NOW()
            RETURNING lease_deadline
            "#,
        )
        .bind(job_id)
        .bind(lease_token)
        .bind(lease_deadline)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| WorkError::dependency("heartbeat failed"))?;

        updated.ok_or_else(|| WorkError::conflict("heartbeat rejected for stale lease token"))
    }

    pub async fn complete(&self, job_id: Uuid, lease_token: Uuid) -> Result<(), WorkError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| WorkError::dependency("failed to begin complete transaction"))?;

        let updated = sqlx::query(
            r#"
            UPDATE jobs
            SET state = 'completed',
                lease_token = NULL,
                lease_owner = NULL,
                lease_deadline = NULL,
                completed_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
              AND lease_token = $2
              AND state = 'leased'
              AND lease_deadline >= NOW()
            "#,
        )
        .bind(job_id)
        .bind(lease_token)
        .execute(&mut *tx)
        .await
        .map_err(|_| WorkError::dependency("complete update failed"))?;

        if updated.rows_affected() != 1 {
            return Err(WorkError::conflict(
                "complete rejected for stale or expired lease token",
            ));
        }

        close_open_attempt(&mut tx, job_id, AttemptOutcome::Completed, None).await?;

        tx.commit()
            .await
            .map_err(|_| WorkError::dependency("complete commit failed"))?;
        Ok(())
    }

    pub async fn fail(
        &self,
        job_id: Uuid,
        lease_token: Uuid,
        error_class: ErrorClass,
    ) -> Result<FailOutcome, WorkError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| WorkError::dependency("failed to begin fail transaction"))?;

        let row = sqlx::query_as::<_, FailRow>(
            r#"
            SELECT attempt_count, max_attempts
            FROM jobs
            WHERE id = $1
              AND lease_token = $2
              AND state = 'leased'
              AND lease_deadline >= NOW()
            FOR UPDATE
            "#,
        )
        .bind(job_id)
        .bind(lease_token)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| WorkError::dependency("fail lookup failed"))?;

        let Some(row) = row else {
            return Err(WorkError::conflict(
                "fail rejected for stale or expired lease token",
            ));
        };

        let error_class_str = error_class.as_str();
        close_open_attempt(
            &mut tx,
            job_id,
            AttemptOutcome::Failed,
            Some(error_class_str),
        )
        .await?;

        let retry = is_retryable(error_class) && row.attempt_count < row.max_attempts;
        let outcome = if retry {
            let delay_secs = retry_delay_secs(row.attempt_count);
            let run_at = Utc::now() + Duration::seconds(delay_secs);
            sqlx::query(
                r#"
                UPDATE jobs
                SET state = 'queued',
                    run_at = $2,
                    lease_token = NULL,
                    lease_owner = NULL,
                    lease_deadline = NULL,
                    last_error_class = $3,
                    updated_at = NOW()
                WHERE id = $1
                  AND lease_token = $4
                  AND state = 'leased'
                "#,
            )
            .bind(job_id)
            .bind(run_at)
            .bind(error_class_str)
            .bind(lease_token)
            .execute(&mut *tx)
            .await
            .map_err(|_| WorkError::dependency("fail retry update failed"))?;
            FailOutcome::Retried
        } else {
            sqlx::query(
                r#"
                UPDATE jobs
                SET state = 'dead',
                    lease_token = NULL,
                    lease_owner = NULL,
                    lease_deadline = NULL,
                    last_error_class = $2,
                    completed_at = NOW(),
                    updated_at = NOW()
                WHERE id = $1
                  AND lease_token = $3
                  AND state = 'leased'
                "#,
            )
            .bind(job_id)
            .bind(error_class_str)
            .bind(lease_token)
            .execute(&mut *tx)
            .await
            .map_err(|_| WorkError::dependency("fail dead-letter update failed"))?;
            FailOutcome::DeadLettered
        };

        tx.commit()
            .await
            .map_err(|_| WorkError::dependency("fail commit failed"))?;
        Ok(outcome)
    }

    pub async fn cancel(&self, job_id: Uuid, lease_token: Option<Uuid>) -> Result<(), WorkError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| WorkError::dependency("failed to begin cancel transaction"))?;

        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1 FOR UPDATE")
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| WorkError::dependency("cancel lookup failed"))?;

        let Some(state) = state else {
            return Err(WorkError::not_found("job not found"));
        };

        match state.as_str() {
            "queued" => {
                if lease_token.is_some() {
                    return Err(WorkError::validation(
                        "queued cancellation must not include a lease token",
                    ));
                }
                sqlx::query(
                    r#"
                    UPDATE jobs
                    SET state = 'cancelled',
                        completed_at = NOW(),
                        updated_at = NOW()
                    WHERE id = $1 AND state = 'queued'
                    "#,
                )
                .bind(job_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| WorkError::dependency("cancel queued update failed"))?;
            }
            "leased" => {
                let Some(token) = lease_token else {
                    return Err(WorkError::validation(
                        "leased cancellation requires the current lease token",
                    ));
                };
                let updated = sqlx::query(
                    r#"
                    UPDATE jobs
                    SET state = 'cancelled',
                        lease_token = NULL,
                        lease_owner = NULL,
                        lease_deadline = NULL,
                        completed_at = NOW(),
                        updated_at = NOW()
                    WHERE id = $1
                      AND lease_token = $2
                      AND state = 'leased'
                      AND lease_deadline >= NOW()
                    "#,
                )
                .bind(job_id)
                .bind(token)
                .execute(&mut *tx)
                .await
                .map_err(|_| WorkError::dependency("cancel leased update failed"))?;

                if updated.rows_affected() != 1 {
                    return Err(WorkError::conflict(
                        "cancel rejected for stale or expired lease token",
                    ));
                }

                close_open_attempt(&mut tx, job_id, AttemptOutcome::Cancelled, None).await?;
            }
            _ => {
                return Err(WorkError::conflict(
                    "cancel rejected for non-active job state",
                ));
            }
        }

        tx.commit()
            .await
            .map_err(|_| WorkError::dependency("cancel commit failed"))?;
        Ok(())
    }

    pub async fn recover_dead(&self, job_id: Uuid) -> Result<(), WorkError> {
        let updated = sqlx::query(
            r#"
            UPDATE jobs
            SET state = 'queued',
                run_at = NOW(),
                max_attempts = GREATEST(max_attempts, attempt_count + 1),
                lease_token = NULL,
                lease_owner = NULL,
                lease_deadline = NULL,
                last_error_class = NULL,
                completed_at = NULL,
                updated_at = NOW()
            WHERE id = $1 AND state = 'dead'
            "#,
        )
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(|_| WorkError::dependency("recover dead update failed"))?;

        if updated.rows_affected() != 1 {
            return Err(WorkError::conflict("recover rejected for non-dead job"));
        }
        Ok(())
    }

    pub async fn get_job_summary(&self, job_id: Uuid) -> Result<JobSummary, WorkError> {
        let row = sqlx::query_as::<_, SummaryRow>(
            r#"
            SELECT
                id,
                job_type,
                payload_version,
                state,
                priority,
                run_at,
                dedupe_key,
                serialization_key,
                attempt_count,
                max_attempts,
                last_error_class
            FROM jobs
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| WorkError::dependency("job summary lookup failed"))?;

        let Some(row) = row else {
            return Err(WorkError::not_found("job not found"));
        };

        Ok(row.into_summary())
    }

    pub async fn list_attempts(&self, job_id: Uuid) -> Result<Vec<AttemptSummary>, WorkError> {
        let rows = sqlx::query_as::<_, AttemptRow>(
            r#"
            SELECT
                id,
                job_id,
                attempt_number,
                lease_owner,
                started_at,
                ended_at,
                outcome,
                error_class
            FROM job_attempts
            WHERE job_id = $1
            ORDER BY attempt_number ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| WorkError::dependency("attempt list failed"))?;

        Ok(rows.into_iter().map(AttemptRow::into_summary).collect())
    }
}

fn payload_version(payload: &serde_json::Value) -> Result<i32, WorkError> {
    payload
        .get("schema_version")
        .and_then(|value| value.as_i64())
        .filter(|value| *value > 0)
        .map(|value| value as i32)
        .ok_or_else(|| WorkError::validation("payload must include positive schema_version"))
}

fn validate_enqueue_request(request: &EnqueueRequest) -> Result<i32, WorkError> {
    let payload_version = payload_version(&request.payload)?;
    if request.max_attempts <= 0 {
        return Err(WorkError::validation("max_attempts must be positive"));
    }
    if request.job_type.trim().is_empty() || request.job_type.chars().count() > 128 {
        return Err(WorkError::validation("job_type has invalid length"));
    }
    if request.dedupe_key.trim().is_empty() || request.dedupe_key.chars().count() > 512 {
        return Err(WorkError::validation("dedupe_key has invalid length"));
    }
    if request
        .serialization_key
        .as_ref()
        .is_some_and(|key| key.trim().is_empty() || key.chars().count() > 512)
    {
        return Err(WorkError::validation(
            "serialization_key has invalid length",
        ));
    }
    Ok(payload_version)
}

fn map_insert_error(error: sqlx::Error, message: &str) -> WorkError {
    if let sqlx::Error::Database(db_error) = &error
        && db_error.code() == Some(std::borrow::Cow::Borrowed("23505"))
    {
        return WorkError::conflict("active serialization key already reserved");
    }
    let _ = error;
    WorkError::dependency(message)
}

async fn close_open_attempt(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    outcome: AttemptOutcome,
    error_class: Option<&str>,
) -> Result<(), WorkError> {
    sqlx::query(
        r#"
        UPDATE job_attempts
        SET ended_at = NOW(),
            outcome = $2,
            error_class = COALESCE($3, error_class)
        WHERE job_id = $1
          AND ended_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(outcome.as_str())
    .bind(error_class)
    .execute(&mut **tx)
    .await
    .map_err(|_| WorkError::dependency("attempt close failed"))?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct CandidateRow {
    id: Uuid,
    job_type: String,
    payload: serde_json::Value,
    payload_version: i32,
    dedupe_key: String,
    serialization_key: Option<String>,
    state: JobState,
    attempt_count: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct FailRow {
    attempt_count: i32,
    max_attempts: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct SummaryRow {
    id: Uuid,
    job_type: String,
    payload_version: i32,
    state: String,
    priority: i32,
    run_at: DateTime<Utc>,
    dedupe_key: String,
    serialization_key: Option<String>,
    attempt_count: i32,
    max_attempts: i32,
    last_error_class: Option<String>,
}

impl SummaryRow {
    fn into_summary(self) -> JobSummary {
        JobSummary {
            id: self.id,
            job_type: self.job_type,
            payload_version: self.payload_version,
            state: JobState::parse(&self.state).unwrap_or(JobState::Queued),
            priority: self.priority,
            run_at: self.run_at,
            dedupe_key: self.dedupe_key,
            serialization_key: self.serialization_key,
            attempt_count: self.attempt_count,
            max_attempts: self.max_attempts,
            last_error_class: self.last_error_class,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AttemptRow {
    id: Uuid,
    job_id: Uuid,
    attempt_number: i32,
    lease_owner: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    outcome: Option<String>,
    error_class: Option<String>,
}

impl AttemptRow {
    fn into_summary(self) -> AttemptSummary {
        AttemptSummary {
            id: self.id,
            job_id: self.job_id,
            attempt_number: self.attempt_number,
            lease_owner: self.lease_owner,
            started_at: self.started_at,
            ended_at: self.ended_at,
            outcome: self.outcome.as_deref().and_then(AttemptOutcome::parse),
            error_class: self.error_class,
        }
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for JobState {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        JobState::parse(raw).ok_or_else(|| format!("invalid job state: {raw}").into())
    }
}

impl sqlx::Type<sqlx::Postgres> for JobState {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}
