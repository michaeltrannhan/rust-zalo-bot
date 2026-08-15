//! PostgreSQL pool and sqlx migrations.

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, migrate::Migrator};

use crate::config::ResolvedConfig;
use crate::error::AppError;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn create_pool(config: &ResolvedConfig) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(&config.database_url)
        .await
        .map_err(|_| AppError::dependency("database connection failed"))
}

/// Check database connectivity.
pub async fn check_connection(pool: &PgPool) -> Result<(), AppError> {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .map_err(|_| AppError::dependency("database connection failed"))?;
    Ok(())
}

/// Run pending migrations.
pub async fn migrate(pool: &PgPool) -> Result<(), AppError> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|_| AppError::migration("migration failed"))?;
    Ok(())
}

/// Verify all migrations are applied.
pub async fn check_migrations_current(pool: &PgPool) -> Result<bool, AppError> {
    let applied = match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
    {
        Ok(count) => count,
        Err(sqlx::Error::Database(db_err))
            if db_err.code() == Some(std::borrow::Cow::Borrowed("42P01")) =>
        {
            return Ok(false);
        }
        Err(_) => return Err(AppError::dependency("failed to read migration state")),
    };

    let expected = MIGRATOR.migrations.len() as i64;
    Ok(applied >= expected)
}
