//! Periodic receipt maintenance: review expiry and retention sweeps.

use std::time::Duration as StdDuration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::receipt::ReceiptLifecycle;

const TICK_INTERVAL_SECS: u64 = 60;
const LEASE_DURATION_SECS: i64 = 90;
const BATCH_LIMIT: i32 = 50;
const ROLE: &str = "maintenance";

/// Run the maintenance role until shutdown.
pub async fn run_maintenance_role(
    pool: PgPool,
    receipt: ReceiptLifecycle,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), crate::error::AppError> {
    tracing::info!(role = ROLE, "role task started");
    let owner = maintenance_lease_owner();
    let mut interval = tokio::time::interval(StdDuration::from_secs(TICK_INTERVAL_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    while !*shutdown_rx.borrow() {
        tokio::select! {
            _ = interval.tick() => {
                if try_acquire_role_lease(&pool, &owner, LEASE_DURATION_SECS).await {
                    maintenance_tick(&receipt).await;
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

/// One maintenance pass: expire stale reviews then sweep retained originals.
pub async fn maintenance_tick(receipt: &ReceiptLifecycle) {
    if let Err(error) = receipt.expire_reviews(BATCH_LIMIT).await {
        tracing::warn!(
            error_class = %error.class,
            "expire_reviews failed during maintenance"
        );
    }
    if let Err(error) = receipt.retention_sweep(BATCH_LIMIT).await {
        tracing::warn!(
            error_class = %error.class,
            "retention_sweep failed during maintenance"
        );
    }
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

fn maintenance_lease_owner() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "localhost".to_string());
    format!(
        "{host}:{}:{}:maintenance",
        std::process::id(),
        Uuid::new_v4()
    )
}
