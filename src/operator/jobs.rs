//! Operator job inspection and recovery commands.

use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::work::{JobState, WorkStore};

#[derive(Debug, Serialize)]
struct JobListItem {
    id: Uuid,
    job_type: String,
    state: String,
    attempt_count: i32,
    run_at: chrono::DateTime<chrono::Utc>,
    last_error_class: Option<String>,
}

pub async fn run_jobs_list(
    store: &WorkStore,
    state: Option<&str>,
    limit: u32,
    json: bool,
) -> Result<(), AppError> {
    if let Some(state) = state
        && JobState::parse(state).is_none()
    {
        return Err(AppError::usage(format!("unknown job state: {state}")));
    }

    let limit = limit.clamp(1, 200);
    let rows = store.list_jobs(state, limit).await.map_err(work_error)?;

    if json {
        let items: Vec<JobListItem> = rows
            .into_iter()
            .map(|row| JobListItem {
                id: row.id,
                job_type: row.job_type,
                state: row.state.as_str().to_string(),
                attempt_count: row.attempt_count,
                run_at: row.run_at,
                last_error_class: row.last_error_class,
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items)
                .map_err(|_| AppError::internal("jobs list serialization failed"))?
        );
    } else {
        println!(
            "{:<36} {:<22} {:<10} {:>7} {:<28} LAST_ERROR",
            "ID", "JOB_TYPE", "STATE", "ATTEMPTS", "RUN_AT"
        );
        for row in rows {
            println!(
                "{:<36} {:<22} {:<10} {:>7} {:<28} {}",
                row.id,
                row.job_type,
                row.state.as_str(),
                row.attempt_count,
                row.run_at,
                row.last_error_class.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}

pub async fn run_jobs_show(store: &WorkStore, job_id: Uuid) -> Result<(), AppError> {
    let summary = store.get_job_summary(job_id).await.map_err(work_error)?;
    let attempts = store.list_attempts(job_id).await.map_err(work_error)?;
    let mut output = format!(
        "id={}\njob_type={}\npayload_version={}\nstate={}\npriority={}\nrun_at={}\nattempt_count={}\nmax_attempts={}\nlast_error_class={}\n",
        summary.id,
        summary.job_type,
        summary.payload_version,
        summary.state.as_str(),
        summary.priority,
        summary.run_at,
        summary.attempt_count,
        summary.max_attempts,
        summary.last_error_class.as_deref().unwrap_or("-")
    );
    for attempt in attempts {
        output.push_str(&format!(
            "attempt number={} outcome={} error_class={} started_at={} ended_at={}\n",
            attempt.attempt_number,
            attempt
                .outcome
                .map(crate::work::AttemptOutcome::as_str)
                .unwrap_or("-"),
            attempt.error_class.as_deref().unwrap_or("-"),
            attempt.started_at,
            attempt
                .ended_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    reject_sensitive_operator_output(&output)?;
    print!("{output}");
    Ok(())
}

fn reject_sensitive_operator_output(output: &str) -> Result<(), AppError> {
    const FORBIDDEN: &[&str] = &[
        "\"payload\"",
        "dedupe_key",
        "serialization_key",
        "lease_token",
        "secret_payload",
    ];
    for needle in FORBIDDEN {
        if output.contains(needle) {
            return Err(AppError::internal(
                "operator job output would include a forbidden field",
            ));
        }
    }
    Ok(())
}

pub async fn run_jobs_retry(store: &WorkStore, job_id: Uuid) -> Result<(), AppError> {
    store.recover_dead(job_id).await.map_err(work_error)?;
    println!("job {job_id} requeued");
    Ok(())
}

pub async fn run_jobs_cancel(store: &WorkStore, job_id: Uuid) -> Result<(), AppError> {
    store.cancel_by_operator(job_id).await.map_err(work_error)?;
    println!("job {job_id} cancelled");
    Ok(())
}

fn work_error(error: crate::work::WorkError) -> AppError {
    AppError::new(error.class, error.message)
}
