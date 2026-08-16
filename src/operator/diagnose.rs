//! Redacted operator diagnostic bundle.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::config::ResolvedConfig;
use crate::error::AppError;
use crate::operator::status::collect_status;
use crate::work::WorkStore;

const FORBIDDEN_SUBSTRINGS: &[&str] = &["postgres://", "BEGIN RSA", "api-key", "x-goog-api-key"];

pub async fn run_diagnose(
    config: &ResolvedConfig,
    config_path: Option<&Path>,
    output_dir: &Path,
) -> Result<(), AppError> {
    let files = vec![
        output_dir.join("status.json"),
        output_dir.join("jobs-dead.json"),
        output_dir.join("config-show.json"),
        output_dir.join("doctor.json"),
    ];

    for file in &files {
        println!("{}", file.display());
    }

    fs::create_dir_all(output_dir)
        .map_err(|_| AppError::dependency("failed to create diagnose output directory"))?;

    let pool = crate::db::create_pool(config).await?;
    let status = collect_status(&pool).await?;
    write_json(&files[0], &status)?;

    let store = WorkStore::new(pool.clone());
    let dead_jobs = store
        .list_jobs(Some("dead"), 200)
        .await
        .map_err(|error| AppError::new(error.class, error.message))?;
    let dead_summaries: Vec<_> = dead_jobs
        .into_iter()
        .map(|job| {
            json!({
                "id": job.id,
                "job_type": job.job_type,
                "state": job.state.as_str(),
                "attempt_count": job.attempt_count,
                "run_at": job.run_at,
                "last_error_class": job.last_error_class,
            })
        })
        .collect();
    write_json(&files[1], &dead_summaries)?;

    write_string(&files[2], &config.show_json())?;

    let doctor_capture = capture_doctor_json(config_path).await?;
    write_string(&files[3], &doctor_capture)?;

    for file in &files {
        scan_file(file)?;
    }

    Ok(())
}

async fn capture_doctor_json(config_path: Option<&Path>) -> Result<String, AppError> {
    let mut lines = Vec::new();
    if let Ok(config) = crate::config::load_config(config_path) {
        lines.push(json!({"check": "config_load", "ok": true}));
        if let Ok(pool) = crate::db::create_pool(&config).await {
            let db_ok = crate::db::check_connection(&pool).await.is_ok();
            lines.push(json!({"check": "db_connection", "ok": db_ok}));
            let migrations = if db_ok {
                crate::db::check_migrations_current(&pool)
                    .await
                    .unwrap_or(false)
            } else {
                false
            };
            lines.push(json!({"check": "migrations_current", "ok": migrations}));
        } else {
            lines.push(json!({"check": "db_connection", "ok": false}));
        }
    } else {
        lines.push(json!({"check": "config_load", "ok": false}));
    }

    serde_json::to_string_pretty(&lines)
        .map_err(|_| AppError::internal("doctor bundle serialization failed"))
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), AppError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|_| AppError::internal("json serialization failed"))?;
    write_string(path, &text)
}

fn write_string(path: &Path, text: &str) -> Result<(), AppError> {
    fs::write(path, text).map_err(|_| AppError::dependency("failed to write diagnose file"))
}

fn scan_file(path: &Path) -> Result<(), AppError> {
    let contents = fs::read_to_string(path)
        .map_err(|_| AppError::dependency("failed to read diagnose file for scan"))?;
    for needle in FORBIDDEN_SUBSTRINGS {
        if contents.contains(needle) {
            return Err(AppError::internal(format!(
                "diagnose bundle contains forbidden substring in {}",
                path.display()
            )));
        }
    }
    Ok(())
}
