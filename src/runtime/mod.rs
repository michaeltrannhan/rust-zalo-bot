//! Supervised Tokio runtime with role-based task supervision.

mod jobs;
mod roles;
mod shutdown;

pub use jobs::{JobDeps, dispatch_leased_job};
pub use roles::{Role, RuntimeOptions, all_roles, parse_roles};
pub use shutdown::ShutdownSignal;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tracing::{error, info, warn};

use crate::config::ResolvedConfig;
use crate::db::{check_connection, check_migrations_current, create_pool, migrate};
use crate::error::{AppError, ExitCode};
use crate::health::ReadinessState;
use crate::http::{AppState, WebhookService, router};
use crate::ingress::store_with_receipt;
use crate::outbound::OutboundJobExecution;
use crate::provider::{ZaloHttpAdapter, ZaloHttpConfig};
use crate::receipt::{InMemoryObjectStore, ReceiptConfig, ReceiptLifecycle};
use crate::work::{ClaimOptions, ClaimedJob, WorkStore};

const CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(250);
const SHUTDOWN_DRAIN_DEADLINE: Duration = Duration::from_secs(30);

type RoleResult = Result<(), AppError>;

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

    let receipt_lifecycle = ReceiptLifecycle::new(
        pool.clone(),
        InMemoryObjectStore::new(),
        ReceiptConfig {
            original_receipt_days: config.original_receipt_days,
            review_expiry_hours: ReceiptConfig::default().review_expiry_hours,
        },
    );

    let webhook = if roles.contains(&Role::Ingress) {
        Some(Arc::new(WebhookService::new(
            Arc::clone(zalo_adapter.as_ref().expect("adapter initialized")),
            store_with_receipt(pool.clone(), receipt_lifecycle.clone()),
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

    let mut role_tasks: HashMap<&'static str, JoinHandle<RoleResult>> = HashMap::new();

    if roles.contains(&Role::Ingress) {
        let listen = config.listen_address.clone();
        let router = router(app_state);
        let shutdown_rx = shutdown_rx.clone();
        role_tasks.insert(
            "ingress",
            tokio::spawn(async move {
                let listener = TcpListener::bind(&listen)
                    .await
                    .map_err(|e| AppError::internal(format!("failed to bind {}: {}", listen, e)))?;
                info!(address = %listen, "http server listening");
                axum::serve(listener, router)
                    .with_graceful_shutdown(wait_for_shutdown(shutdown_rx))
                    .await
                    .map_err(|e| AppError::internal(format!("http server error: {}", e)))
            }),
        );
    }

    if roles.contains(&Role::Worker) {
        let job_deps = Arc::new(jobs::JobDeps::production(
            pool.clone(),
            Arc::clone(zalo_adapter.as_ref().expect("adapter initialized")),
            receipt_lifecycle,
        ));
        role_tasks.insert(
            "worker",
            outbound_worker_task(
                pool.clone(),
                job_deps,
                config.outbound_delivery,
                config.zalo_send_timeout_seconds,
                shutdown_rx.clone(),
            ),
        );
    }

    if roles.contains(&Role::Scheduler) {
        role_tasks.insert(
            "scheduler",
            role_idle_task(Role::Scheduler, shutdown_rx.clone()),
        );
    }

    if roles.contains(&Role::Maintenance) {
        role_tasks.insert(
            "maintenance",
            role_idle_task(Role::Maintenance, shutdown_rx.clone()),
        );
    }

    let mut shutdown_listener = tokio::spawn(shutdown_signal.listen());

    let critical_failure = tokio::select! {
        result = &mut shutdown_listener => {
            match result {
                Ok(()) => false,
                Err(join_error) => {
                    error!(error_class = "internal", "shutdown listener join error: {}", join_error);
                    true
                }
            }
        }
        result = wait_for_role_failure(&mut role_tasks, shutdown_rx.clone()) => {
            if let Some((role, error)) = result {
                error!(role, error_class = %error.class, "critical role task failed");
            } else {
                error!(error_class = "internal", "critical role task panicked");
            }
            true
        }
    };

    if critical_failure {
        shutdown_listener.abort();
        readiness.mark_shutting_down();
        let _ = shutdown_tx.send(true);
        abort_role_tasks(&mut role_tasks);
        let _ = drain_role_tasks(&mut role_tasks, Duration::from_secs(5)).await;
        return ExitCode::RuntimeError;
    }

    if let Err(error) = drain_role_tasks(&mut role_tasks, SHUTDOWN_DRAIN_DEADLINE).await {
        error!(error_class = %error.class, "role task failed during shutdown");
        return ExitCode::RuntimeError;
    }

    info!("runtime shutdown complete");
    ExitCode::Success
}

fn outbound_worker_task(
    pool: sqlx::PgPool,
    job_deps: Arc<jobs::JobDeps>,
    outbound_delivery: u32,
    send_timeout_seconds: u64,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<RoleResult> {
    tokio::spawn(async move {
        info!(role = Role::Worker.as_str(), "role task started");

        let store = WorkStore::new(pool.clone());
        let lease_owner = process_lease_owner();
        let lease_duration_secs = worker_lease_duration_secs(send_timeout_seconds);
        let heartbeat_interval = worker_heartbeat_interval(lease_duration_secs);
        let concurrency = usize::try_from(outbound_delivery).unwrap_or(1).max(1);
        let mut in_flight = JoinSet::new();

        while !*shutdown_rx.borrow() {
            if in_flight.len() >= concurrency {
                tokio::select! {
                    joined = in_flight.join_next() => {
                        observe_handler_result(joined)?;
                    }
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
                continue;
            }

            let claimed = match store
                .claim(ClaimOptions {
                    batch_limit: 1,
                    lease_owner: lease_owner.clone(),
                    lease_duration_secs,
                })
                .await
            {
                Ok(jobs) => jobs,
                Err(error) => {
                    warn!(
                        error_class = %error.class,
                        "work claim unavailable; backing off"
                    );
                    tokio::select! {
                        joined = in_flight.join_next(), if !in_flight.is_empty() => {
                            observe_handler_result(joined)?;
                        }
                        should_stop = sleep_or_shutdown(CLAIM_POLL_INTERVAL, &mut shutdown_rx) => {
                            if should_stop {
                                break;
                            }
                        }
                    }
                    continue;
                }
            };

            let Some(job) = claimed.into_iter().next() else {
                tokio::select! {
                    joined = in_flight.join_next(), if !in_flight.is_empty() => {
                        observe_handler_result(joined)?;
                    }
                    should_stop = sleep_or_shutdown(CLAIM_POLL_INTERVAL, &mut shutdown_rx) => {
                        if should_stop {
                            break;
                        }
                    }
                }
                continue;
            };

            let store = store.clone();
            let job_deps = Arc::clone(&job_deps);
            let mut shutdown_rx = shutdown_rx.clone();
            in_flight.spawn(async move {
                let execution = execute_leased_job(
                    &store,
                    &job_deps,
                    &job,
                    lease_duration_secs,
                    heartbeat_interval,
                    &mut shutdown_rx,
                )
                .await;
                apply_execution_result(&store, &job, execution).await;
            });
        }

        drain_in_flight_handlers(&mut in_flight, SHUTDOWN_DRAIN_DEADLINE).await?;

        info!(role = Role::Worker.as_str(), "role task stopped");
        Ok(())
    })
}

async fn execute_leased_job(
    store: &WorkStore,
    job_deps: &jobs::JobDeps,
    job: &ClaimedJob,
    lease_duration_secs: i64,
    heartbeat_interval: Duration,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> OutboundJobExecution {
    let dispatch = jobs::dispatch_leased_job(job_deps, job);
    tokio::pin!(dispatch);

    let mut heartbeat = tokio::time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = &mut dispatch => return result,
            _ = heartbeat.tick() => {
                if store
                    .heartbeat(job.id, job.lease_token, lease_duration_secs)
                    .await
                    .is_err()
                {
                    return OutboundJobExecution::StaleLease;
                }
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return shutdown_execution(store, job).await;
                }
            }
        }
    }
}

async fn shutdown_execution(store: &WorkStore, job: &ClaimedJob) -> OutboundJobExecution {
    match store
        .fail(job.id, job.lease_token, crate::error::ErrorClass::Transient)
        .await
    {
        Ok(_) => OutboundJobExecution::StaleLease,
        Err(error) if error.class == crate::error::ErrorClass::Conflict => {
            OutboundJobExecution::StaleLease
        }
        Err(error) if error.class == crate::error::ErrorClass::Dependency => {
            OutboundJobExecution::StaleLease
        }
        Err(_) => OutboundJobExecution::StaleLease,
    }
}

async fn apply_execution_result(
    store: &WorkStore,
    job: &ClaimedJob,
    execution: OutboundJobExecution,
) {
    match execution {
        OutboundJobExecution::Complete(result) => {
            if let Err(error) = store.complete(job.id, job.lease_token).await {
                warn!(
                    job_id = %job.id,
                    outcome = result.state.as_str(),
                    error_class = %error.class,
                    "job completion rejected"
                );
            }
        }
        OutboundJobExecution::Fail(class) => {
            if let Err(error) = store.fail(job.id, job.lease_token, class).await {
                warn!(
                    job_id = %job.id,
                    error_class = %error.class,
                    "job failure rejected"
                );
            }
        }
        OutboundJobExecution::InvalidJob => {
            if let Err(error) = store
                .fail(
                    job.id,
                    job.lease_token,
                    crate::error::ErrorClass::Validation,
                )
                .await
            {
                warn!(
                    job_id = %job.id,
                    error_class = %error.class,
                    "invalid job terminal failure rejected"
                );
            }
        }
        OutboundJobExecution::StaleLease => {}
    }
}

fn process_lease_owner() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "localhost".to_string());
    format!("{host}:{}:{}", std::process::id(), uuid::Uuid::new_v4())
}

fn worker_lease_duration_secs(send_timeout_seconds: u64) -> i64 {
    send_timeout_seconds.saturating_mul(3).max(60) as i64
}

fn worker_heartbeat_interval(lease_duration_secs: i64) -> Duration {
    Duration::from_secs((lease_duration_secs as u64 / 3).max(1))
}

async fn sleep_or_shutdown(duration: Duration, shutdown_rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => *shutdown_rx.borrow(),
        changed = shutdown_rx.changed() => changed.is_err() || *shutdown_rx.borrow(),
    }
}

async fn drain_in_flight_handlers(in_flight: &mut JoinSet<()>, deadline: Duration) -> RoleResult {
    let timeout = tokio::time::sleep(deadline);
    tokio::pin!(timeout);

    while !in_flight.is_empty() {
        tokio::select! {
            _ = &mut timeout => {
                in_flight.abort_all();
                while in_flight.join_next().await.is_some() {}
                return Ok(());
            }
            joined = in_flight.join_next() => observe_handler_result(joined)?,
        }
    }
    Ok(())
}

fn observe_handler_result(joined: Option<Result<(), tokio::task::JoinError>>) -> RoleResult {
    match joined {
        Some(Ok(())) | None => Ok(()),
        Some(Err(error)) => Err(AppError::internal(format!(
            "worker handler task failed: {error}"
        ))),
    }
}

async fn wait_for_role_failure(
    role_tasks: &mut HashMap<&'static str, JoinHandle<RoleResult>>,
    shutdown_rx: watch::Receiver<bool>,
) -> Option<(String, AppError)> {
    if role_tasks.is_empty() {
        return std::future::pending().await;
    }

    loop {
        let mut finished_role = None;
        for (role, handle) in role_tasks.iter() {
            if handle.is_finished() {
                finished_role = Some(*role);
                break;
            }
        }

        let Some(role) = finished_role else {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        };

        let handle = role_tasks.remove(&role).expect("role handle");
        return Some(match handle.await {
            Ok(Ok(())) if *shutdown_rx.borrow() => continue,
            Ok(Ok(())) => (
                role.to_string(),
                AppError::internal("critical role task exited unexpectedly"),
            ),
            Ok(Err(error)) => (role.to_string(), error),
            Err(join_error) => {
                if join_error.is_panic() {
                    return None;
                }
                (
                    role.to_string(),
                    AppError::internal(format!("role task cancelled: {join_error}")),
                )
            }
        });
    }
}

fn abort_role_tasks(role_tasks: &mut HashMap<&'static str, JoinHandle<RoleResult>>) {
    for (_role, handle) in role_tasks.drain() {
        handle.abort();
    }
}

async fn drain_role_tasks(
    role_tasks: &mut HashMap<&'static str, JoinHandle<RoleResult>>,
    deadline: Duration,
) -> Result<bool, AppError> {
    let timeout = tokio::time::sleep(deadline);
    tokio::pin!(timeout);

    loop {
        let finished = role_tasks
            .iter()
            .find_map(|(role, handle)| handle.is_finished().then_some(*role));
        if let Some(role) = finished {
            let handle = role_tasks.remove(role).expect("finished role handle");
            match handle.await {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => return Err(error),
                Err(error) => {
                    return Err(AppError::internal(format!(
                        "role task failed during shutdown: {error}"
                    )));
                }
            }
        }
        if role_tasks.is_empty() {
            return Ok(true);
        }

        tokio::select! {
            _ = &mut timeout => {
                abort_role_tasks(role_tasks);
                return Ok(false);
            }
            _ = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

fn role_idle_task(role: Role, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<RoleResult> {
    tokio::spawn(async move {
        info!(role = role.as_str(), "role task started");
        while !*shutdown_rx.borrow() {
            if shutdown_rx.changed().await.is_err() {
                break;
            }
        }
        info!(role = role.as_str(), "role task stopped");
        Ok(())
    })
}

fn valid_runtime_secret(value: &str) -> bool {
    value.len() >= 16 && value != "dev-secret-change-me" && value != "change-me"
}

async fn wait_for_shutdown(mut shutdown_rx: watch::Receiver<bool>) {
    while !*shutdown_rx.borrow() {
        if shutdown_rx.changed().await.is_err() {
            break;
        }
    }
}
