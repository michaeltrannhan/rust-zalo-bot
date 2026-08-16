//! Database backup and restore via pg_dump/pg_restore.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::ResolvedConfig;
use crate::error::AppError;

pub fn run_backup(config: &ResolvedConfig, output: &Path) -> Result<(), AppError> {
    if output.as_os_str().is_empty() {
        return Err(AppError::usage("backup output path must not be empty"));
    }
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|_| AppError::dependency("failed to create backup parent directory"))?;
    }

    let status = Command::new("pg_dump")
        .arg("--format=custom")
        .arg("--file")
        .arg(output)
        .arg("--dbname")
        .arg(&config.database_url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| AppError::dependency("pg_dump unavailable"))?;

    if status.success() {
        println!("backup written to {}", output.display());
        Ok(())
    } else {
        Err(AppError::dependency("pg_dump failed"))
    }
}

pub fn run_restore(config: &ResolvedConfig, input: &Path, yes: bool) -> Result<(), AppError> {
    if !yes {
        return Err(AppError::usage("restore requires --yes"));
    }
    if !input.exists() {
        return Err(AppError::usage("restore input file does not exist"));
    }

    let status = Command::new("pg_restore")
        .arg("--clean")
        .arg("--if-exists")
        .arg("--dbname")
        .arg(&config.database_url)
        .arg(input)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| AppError::dependency("pg_restore unavailable"))?;

    if status.success() {
        println!("restore completed from {}", input.display());
        Ok(())
    } else {
        Err(AppError::dependency("pg_restore failed"))
    }
}
