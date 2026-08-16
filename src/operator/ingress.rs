//! Ingress mode inspection and audited switching.

use sqlx::PgPool;

use crate::error::AppError;

pub async fn run_ingress_status(pool: &PgPool) -> Result<(), AppError> {
    let row = sqlx::query_as::<_, IngressRow>(
        "SELECT mode, mode_generation FROM ingress_control WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::dependency("failed to read ingress control"))?
    .ok_or_else(|| AppError::dependency("ingress control row missing"))?;

    println!("mode: {}", row.mode);
    println!("mode_generation: {}", row.mode_generation);
    Ok(())
}

pub async fn run_ingress_switch(pool: &PgPool, mode: &str) -> Result<(), AppError> {
    let db_mode = match mode {
        "webhook" => "webhook",
        "poll" => "polling",
        other => return Err(AppError::usage(format!("unknown ingress mode: {other}"))),
    };

    let generation: i32 = sqlx::query_scalar(
        r#"
        UPDATE ingress_control
        SET mode = $1,
            mode_generation = mode_generation + 1,
            updated_at = NOW()
        WHERE id = 1
        RETURNING mode_generation
        "#,
    )
    .bind(db_mode)
    .fetch_optional(pool)
    .await
    .map_err(|_| AppError::dependency("ingress mode switch failed"))?
    .ok_or_else(|| AppError::dependency("ingress control row missing"))?;

    println!("mode: {db_mode}");
    println!("mode_generation: {generation}");
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct IngressRow {
    mode: String,
    mode_generation: i32,
}
