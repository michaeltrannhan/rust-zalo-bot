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
use crate::http::{AppState, WebhookService, router};
use crate::ingress::IngressStore;
use crate::outbound::deliver_next;
use crate::provider::{ZaloHttpAdapter, ZaloHttpConfig};

/// Run the supervised runtime until shutdown or critical failure.
pub async fn run(config: ResolvedConfig, options: RuntimeOptions) -> ExitCode {
    let readiness = Arc::new(ReadinessState::new_ready());
    let roles = options.roles;

    let pool = match create_pool(&config).await {
        Ok(pool) => {
            if let Err(e) = migrate(&pool).await {
                error!(error_class = %e.class, "migration failed at startup");
                return e.exit_code();
            }
            readiness.set_db_reachable(check_connection(&pool).await.is_ok());
            readiness
                .set_migrations_current(check_migrations_current(&pool).await.unwrap_or(false));
            pool
        }
        Err(e) => {
            error!(error_class = %e.class, "database unavailable at startup");
            return e.exit_code();
        }
    };

    let needs_zalo = roles.contains(&Role::Ingress) || roles.contains(&Role::Worker);
    let zalo_adapter = if needs_zalo {
        let bot_token = match config.read_zalo_bot_token() {
            Ok(value) => value,
            Err(error) => return error.exit_code(),
        };
        let webhook_secret = if roles.contains(&Role::Ingress) {
            match config.read_webhook_secret() {
                Ok(value) if valid_runtime_secret(&value) => value,
                Ok(_) => return AppError::config("webhook credential is unsafe").exit_code(),
                Err(error) => return error.exit_code(),
            }
        } else {
            String::new()
        };
        let adapter = match ZaloHttpAdapter::new(ZaloHttpConfig {
            api_base: config.zalo_api_base.clone(),
            bot_token,
            webhook_secret,
            provider_scope: "zalo_bot".to_string(),
            request_timeout: Duration::from_secs(config.zalo_send_timeout_seconds),
        }) {
            Ok(adapter) => Arc::new(adapter),
            Err(_) => return AppError::config("failed to configure Zalo HTTP adapter").exit_code(),
        };
        Some(adapter)
    } else {
        None
    };

    let webhook = if roles.contains(&Role::Ingress) {
        Some(Arc::new(WebhookService::new(
            Arc::clone(zalo_adapter.as_ref().expect("adapter initialized")),
            IngressStore::new(pool.clone()),
            config.allowed_provider_sender_ids.clone(),
            config.webhook_max_body_bytes,
        )))
    } else {
        None
    };

    let app_state = AppState {
        readiness: Arc::clone(&readiness),
        pool: Some(pool.clone()),
        webhook,
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown_signal = ShutdownSignal::new(shutdown_tx.clone(), Arc::clone(&readiness));

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

    let worker_task = if roles.contains(&Role::Worker) {
        Some(outbound_worker_task(
            pool.clone(),
            Arc::clone(zalo_adapter.as_ref().expect("adapter initialized")),
            shutdown_rx.clone(),
        ))
    } else {
        None
    };
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

fn outbound_worker_task(
    pool: sqlx::PgPool,
    adapter: Arc<ZaloHttpAdapter>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!(role = Role::Worker.as_str(), "role task started");
        while !*shutdown_rx.borrow() {
            match deliver_next(&pool, &adapter).await {
                Ok(Some(result)) => {
                    info!(
                        outcome = result.state.as_str(),
                        "outbound delivery finished"
                    );
                    continue;
                }
                Ok(None) => {}
                Err(_) => error!(
                    error_class = "dependency_error",
                    "outbound store unavailable"
                ),
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                changed = shutdown_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
        }
        info!(role = Role::Worker.as_str(), "role task stopped");
    })
}

fn valid_runtime_secret(value: &str) -> bool {
    value.len() >= 16 && value != "dev-secret-change-me" && value != "change-me"
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
