//! Periodic summary schedule enqueue loop.

use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::schedule::{
    Frequency, JOB_TYPE_SCHEDULE_EMIT, SCHEDULE_PAYLOAD_VERSION, ScheduleEmitPayload,
    account_serialization_key, latest_delivery, next_delivery, schedule_emit_dedupe_key,
};
use crate::work::{EnqueueRequest, WorkStore};

const TICK_INTERVAL_SECS: u64 = 60;
const LEASE_DURATION_SECS: i64 = 90;
const BATCH_LIMIT: i64 = 100;
const ROLE: &str = "scheduler";

type DueScheduleRow = (Uuid, Uuid, String, i32, Option<DateTime<Utc>>, String);

/// Run the scheduler role until shutdown.
pub async fn run_scheduler_role(
    pool: PgPool,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), crate::error::AppError> {
    tracing::info!(role = ROLE, "role task started");
    let owner = scheduler_lease_owner();
    let mut interval = tokio::time::interval(StdDuration::from_secs(TICK_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    while !*shutdown_rx.borrow() {
        tokio::select! {
            _ = interval.tick() => {
                if try_acquire_role_lease(&pool, &owner, LEASE_DURATION_SECS).await
                    && let Err(error) = scheduler_tick(&pool).await
                {
                    tracing::warn!(
                        error_class = %error.class,
                        "scheduler tick failed"
                    );
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
            }
        }
    }

    tracing::info!(role = ROLE, "role task stopped");
    Ok(())
}

/// One scheduler pass: enqueue due schedules and advance next_run_at.
pub async fn scheduler_tick(pool: &PgPool) -> Result<(), crate::error::AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| crate::error::AppError::internal(format!("scheduler tx begin: {e}")))?;

    let due_rows: Vec<DueScheduleRow> = sqlx::query_as(
        r#"
        SELECT s.id, s.account_id, s.frequency, s.delivery_minute, s.last_emitted_at, a.timezone
        FROM summary_schedules s
        JOIN accounts a ON a.id = s.account_id
        WHERE s.enabled = TRUE
          AND s.next_run_at <= NOW()
          AND a.lifecycle_state = 'active'
        ORDER BY s.next_run_at ASC
        LIMIT $1
        FOR UPDATE OF s SKIP LOCKED
        "#,
    )
    .bind(BATCH_LIMIT)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| crate::error::AppError::internal(format!("scheduler due select: {e}")))?;

    let now = Utc::now();
    for (schedule_id, account_id, frequency, delivery_minute, last_emitted_at, timezone) in due_rows
    {
        let frequency_enum = Frequency::parse(&frequency).ok_or_else(|| {
            crate::error::AppError::internal(format!("invalid schedule frequency: {frequency}"))
        })?;

        let latest =
            latest_delivery(now, &timezone, frequency_enum, delivery_minute).map_err(|error| {
                crate::error::AppError::internal(format!("latest delivery: {error}"))
            })?;
        let next = next_delivery(now, &timezone, frequency_enum, delivery_minute)
            .map_err(|error| crate::error::AppError::internal(format!("next delivery: {error}")))?;

        let skip_emit = last_emitted_at.is_some_and(|emitted| emitted >= latest);

        if !skip_emit {
            let (period, _) = crate::schedule::scheduled_period(frequency_enum, latest, &timezone)
                .map_err(|error| {
                    crate::error::AppError::internal(format!("scheduled period: {error}"))
                })?;
            let payload = ScheduleEmitPayload {
                schema_version: SCHEDULE_PAYLOAD_VERSION,
                account_id,
                schedule_id,
                frequency: frequency.clone(),
                period_start: period.start,
                period_end: period.end,
            };
            let dedupe_key = schedule_emit_dedupe_key(account_id, &frequency, period.start);
            WorkStore::enqueue_in_transaction(
                &mut tx,
                EnqueueRequest {
                    id: Uuid::new_v4(),
                    job_type: JOB_TYPE_SCHEDULE_EMIT.to_string(),
                    payload: json!(payload),
                    dedupe_key,
                    serialization_key: Some(account_serialization_key(account_id)),
                    priority: 0,
                    run_at: now,
                    max_attempts: 10,
                },
            )
            .await
            .map_err(|_| crate::error::AppError::internal("scheduler enqueue failed"))?;
        }

        sqlx::query(
            r#"
            UPDATE summary_schedules
            SET next_run_at = $2,
                last_emitted_at = CASE WHEN $3 THEN NOW() ELSE last_emitted_at END
            WHERE id = $1
            "#,
        )
        .bind(schedule_id)
        .bind(next)
        .bind(!skip_emit)
        .execute(&mut *tx)
        .await
        .map_err(|e| crate::error::AppError::internal(format!("advance schedule: {e}")))?;
    }

    tx.commit()
        .await
        .map_err(|e| crate::error::AppError::internal(format!("scheduler commit: {e}")))?;
    Ok(())
}

async fn try_acquire_role_lease(pool: &PgPool, owner: &str, duration_secs: i64) -> bool {
    let acquired: Option<String> = sqlx::query_scalar(
        r#"
        INSERT INTO role_leases (role, owner, deadline)
        VALUES ($1, $2, NOW() + make_interval(secs => $3))
        ON CONFLICT (role) DO UPDATE
        SET owner = EXCLUDED.owner,
            deadline = EXCLUDED.deadline
        WHERE role_leases.deadline <= NOW()
           OR role_leases.owner = EXCLUDED.owner
        RETURNING role
        "#,
    )
    .bind(ROLE)
    .bind(owner)
    .bind(duration_secs as f64)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    acquired.is_some()
}

fn scheduler_lease_owner() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "localhost".to_string());
    format!("{host}:{}:{}:scheduler", std::process::id(), Uuid::new_v4())
}
