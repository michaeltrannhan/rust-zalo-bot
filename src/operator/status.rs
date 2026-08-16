//! Operator status command.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::db::{check_connection, check_migrations_current};
use crate::error::AppError;

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub healthy: bool,
    pub ready: bool,
    pub migrations_current: bool,
    pub ingress: IngressStatus,
    pub jobs: JobCounts,
    pub jobs_dead: i64,
    pub jobs_oldest_queued_age_seconds: Option<i64>,
    pub outbound: OutboundCounts,
    pub last_inbound_received_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct IngressStatus {
    pub mode: String,
    pub mode_generation: i32,
}

#[derive(Debug, Serialize, Default)]
pub struct JobCounts {
    pub queued: i64,
    pub leased: i64,
    pub completed: i64,
    pub cancelled: i64,
    pub dead: i64,
}

#[derive(Debug, Serialize, Default)]
pub struct OutboundCounts {
    pub queued: i64,
    pub sending: i64,
    pub sent: i64,
    pub failed: i64,
    pub suppressed: i64,
    pub ambiguous: i64,
}

pub async fn run_status(pool: &PgPool, json: bool) -> Result<(), AppError> {
    let report = collect_status(pool).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| AppError::internal("status serialization failed"))?
        );
    } else {
        print_text(&report);
    }
    Ok(())
}

pub async fn collect_status(pool: &PgPool) -> Result<StatusReport, AppError> {
    let db_ok = check_connection(pool).await.is_ok();
    let migrations_current = if db_ok {
        check_migrations_current(pool).await.unwrap_or(false)
    } else {
        false
    };

    let ingress = sqlx::query_as::<_, IngressRow>(
        "SELECT mode, mode_generation FROM ingress_control WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::dependency("failed to read ingress control"))?
    .ok_or_else(|| AppError::dependency("ingress control row missing"))?;

    let mut jobs = JobCounts::default();
    let job_rows = sqlx::query_as::<_, StateCount>(
        "SELECT state, COUNT(*)::BIGINT AS count FROM jobs GROUP BY state",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::dependency("failed to count jobs"))?;
    for row in job_rows {
        match row.state.as_str() {
            "queued" => jobs.queued = row.count,
            "leased" => jobs.leased = row.count,
            "completed" => jobs.completed = row.count,
            "cancelled" => jobs.cancelled = row.count,
            "dead" => jobs.dead = row.count,
            _ => {}
        }
    }

    let jobs_oldest_queued_age_seconds: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT EXTRACT(EPOCH FROM (NOW() - MIN(run_at)))::BIGINT
        FROM jobs
        WHERE state = 'queued'
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|_| AppError::dependency("failed to read oldest queued job age"))?;

    let mut outbound = OutboundCounts::default();
    let outbound_rows = sqlx::query_as::<_, StateCount>(
        "SELECT state, COUNT(*)::BIGINT AS count FROM outbound_messages GROUP BY state",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| AppError::dependency("failed to count outbound messages"))?;
    for row in outbound_rows {
        match row.state.as_str() {
            "queued" => outbound.queued = row.count,
            "sending" => outbound.sending = row.count,
            "sent" => outbound.sent = row.count,
            "failed" => outbound.failed = row.count,
            "suppressed" => outbound.suppressed = row.count,
            "ambiguous" => outbound.ambiguous = row.count,
            _ => {}
        }
    }

    let last_inbound_received_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MAX(received_at) FROM inbound_events")
            .fetch_one(pool)
            .await
            .map_err(|_| AppError::dependency("failed to read last inbound event"))?;

    let ready = db_ok && migrations_current;
    let jobs_dead = jobs.dead;
    Ok(StatusReport {
        healthy: db_ok,
        ready,
        migrations_current,
        ingress: IngressStatus {
            mode: ingress.mode,
            mode_generation: ingress.mode_generation,
        },
        jobs,
        jobs_dead,
        jobs_oldest_queued_age_seconds,
        outbound,
        last_inbound_received_at,
    })
}

fn print_text(report: &StatusReport) {
    println!("healthy: {}", report.healthy);
    println!("ready: {}", report.ready);
    println!("migrations_current: {}", report.migrations_current);
    println!(
        "ingress: mode={} generation={}",
        report.ingress.mode, report.ingress.mode_generation
    );
    println!(
        "jobs: queued={} leased={} completed={} cancelled={} dead={}",
        report.jobs.queued,
        report.jobs.leased,
        report.jobs.completed,
        report.jobs.cancelled,
        report.jobs.dead
    );
    if let Some(age) = report.jobs_oldest_queued_age_seconds {
        println!("jobs_oldest_queued_age_seconds: {age}");
    }
    println!(
        "outbound: queued={} sending={} sent={} failed={} suppressed={} ambiguous={}",
        report.outbound.queued,
        report.outbound.sending,
        report.outbound.sent,
        report.outbound.failed,
        report.outbound.suppressed,
        report.outbound.ambiguous
    );
    if let Some(ts) = report.last_inbound_received_at {
        println!("last_inbound_received_at: {ts}");
    }
}

#[derive(Debug, sqlx::FromRow)]
struct IngressRow {
    mode: String,
    mode_generation: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct StateCount {
    state: String,
    count: i64,
}
