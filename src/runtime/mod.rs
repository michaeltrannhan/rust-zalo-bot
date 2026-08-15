//! Supervised Tokio runtime with role-based task supervision.

mod roles;
mod shutdown;

pub use roles::{Role, RuntimeOptions, all_roles, parse_roles};
pub use shutdown::ShutdownSignal;

use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{error, info};

use crate::config::ResolvedConfig;
use crate::db::{check_connection, check_migrations_current, create_pool, migrate};
use crate::error::{AppError, ExitCode};
use crate::health::ReadinessState;
use crate::http::{AppState, router};

/// Run the supervised runtime until shutdown or critical failure.
pub async fn run(config: ResolvedConfig, options: RuntimeOptions) -> ExitCode {
    let readiness = Arc::new(ReadinessState::new_ready());

    let pool = match create_pool(&config).await {
        Ok(pool) => {
            if let Err(e) = migrate(&pool).await {
                error!(error_class = %e.class, "migration failed at startup");
                return e.exit_code();
            }
            readiness.set_db_reachable(check_connection(&pool).await.is_ok());
            readiness
                .set_migrations_current(check_migrations_current(&pool).await.unwrap_or(false));
            Some(pool)
        }
        Err(e) => {
            error!(error_class = %e.class, "database unavailable at startup");
            return e.exit_code();
        }
    };

    let app_state = AppState {
        readiness: Arc::clone(&readiness),
        pool: pool.clone(),
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown_signal = ShutdownSignal::new(shutdown_tx.clone(), Arc::clone(&readiness));

    let roles = options.roles;
    info!(roles = ?roles, "starting supervised runtime");

    let http_task = if roles.contains(&Role::Ingress) {
        let listen = config.listen_address.clone();
        let router = router(app_state);
        let shutdown_rx = shutdown_rx.clone();
        Some(tokio::spawn(async move {
            let listener = TcpListener::bind(&listen)
                .await
                .map_err(|e| AppError::internal(format!("failed to bind {}: {}", listen, e)))?;
            info!(address = %listen, "http server listening");
            axum::serve(listener, router)
                .with_graceful_shutdown(wait_for_shutdown(shutdown_rx))
                .await
                .map_err(|e| AppError::internal(format!("http server error: {}", e)))
        }))
    } else {
        None
    };

    let worker_task = role_idle_task(
        Role::Worker,
        roles.contains(&Role::Worker),
        shutdown_rx.clone(),
    );
    let scheduler_task = role_idle_task(
        Role::Scheduler,
        roles.contains(&Role::Scheduler),
        shutdown_rx.clone(),
    );
    let maintenance_task = role_idle_task(
        Role::Maintenance,
        roles.contains(&Role::Maintenance),
        shutdown_rx.clone(),
    );

    let shutdown_listener = tokio::spawn(shutdown_signal.listen());

    let mut critical_failure = false;

    if let Some(task) = http_task {
        match task.await {
            Ok(Err(e)) => {
                error!(error_class = %e.class, "critical http task failed");
                critical_failure = true;
            }
            Err(e) => {
                error!(error_class = "internal", "http task join error: {}", e);
                critical_failure = true;
            }
            Ok(Ok(())) => {}
        }
    } else {
        let _ = shutdown_listener.await;
    }

    if critical_failure {
        readiness.mark_shutting_down();
        let _ = shutdown_tx.send(true);
        cancel_role_tasks(worker_task, scheduler_task, maintenance_task);
        return ExitCode::RuntimeError;
    }

    await_role_tasks(worker_task, scheduler_task, maintenance_task).await;

    info!("runtime shutdown complete");
    ExitCode::Success
}

fn role_idle_task(
    role: Role,
    enabled: bool,
    shutdown_rx: watch::Receiver<bool>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled {
        return None;
    }
    Some(tokio::spawn(async move {
        info!(role = role.as_str(), "role task started");
        while !*shutdown_rx.borrow() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        info!(role = role.as_str(), "role task stopped");
    }))
}

fn cancel_role_tasks(
    worker: Option<tokio::task::JoinHandle<()>>,
    scheduler: Option<tokio::task::JoinHandle<()>>,
    maintenance: Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(t) = worker {
        t.abort();
    }
    if let Some(t) = scheduler {
        t.abort();
    }
    if let Some(t) = maintenance {
        t.abort();
    }
}

async fn await_role_tasks(
    worker: Option<tokio::task::JoinHandle<()>>,
    scheduler: Option<tokio::task::JoinHandle<()>>,
    maintenance: Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(t) = worker {
        let _ = t.await;
    }
    if let Some(t) = scheduler {
        let _ = t.await;
    }
    if let Some(t) = maintenance {
        let _ = t.await;
    }
}

async fn wait_for_shutdown(mut shutdown_rx: watch::Receiver<bool>) {
    while !*shutdown_rx.borrow() {
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }
}
