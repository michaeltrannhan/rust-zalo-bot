//! Operator CLI surfaces: status, jobs, doctor, ingress control, backup, diagnose.

mod backup;
mod diagnose;
mod doctor;
mod ingress;
mod jobs;
mod status;

pub use backup::{run_backup, run_restore};
pub use diagnose::run_diagnose;
pub use doctor::{ActiveProbe, run_doctor};
pub use ingress::{run_ingress_status, run_ingress_switch};
pub use jobs::{run_jobs_cancel, run_jobs_list, run_jobs_retry, run_jobs_show};
pub use status::run_status;

use std::process::Command as StdCommand;

/// Tail service logs via journald when available.
pub fn run_logs(follow: bool, since: Option<&str>) -> Result<(), crate::error::AppError> {
    if !std::path::Path::new("/run/systemd/system").exists() {
        println!("journald unavailable on this host");
        return Ok(());
    }

    let mut cmd = StdCommand::new("journalctl");
    cmd.arg("-u").arg("zl-expense.service");
    if follow {
        cmd.arg("-f");
    }
    if let Some(since) = since {
        cmd.arg("--since").arg(since);
    }
    let status = cmd
        .status()
        .map_err(|_| crate::error::AppError::dependency("journalctl failed"))?;
    if status.success() {
        Ok(())
    } else {
        Err(crate::error::AppError::dependency(
            "journalctl exited with error",
        ))
    }
}
