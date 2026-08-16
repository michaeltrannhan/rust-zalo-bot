//! Atomic usage counters for ingress and worker quota enforcement.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sqlx::{Postgres, Transaction};

pub const SCOPE_ACCOUNT: &str = "account";
pub const SCOPE_GLOBAL: &str = "global";
pub const GLOBAL_SCOPE_ID: &str = "global";

pub const METRIC_RECEIPTS: &str = "receipts";
pub const METRIC_EXTRACTION_PAGES: &str = "extraction_pages";
pub const METRIC_ZALO_MESSAGES: &str = "zalo_messages";
pub const METRIC_INSIGHT_NARRATIVES: &str = "insight_narratives";

/// Result of an atomic quota increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaOutcome {
    pub count: i64,
    pub limit: i64,
}

impl QuotaOutcome {
    pub fn exceeded(self) -> bool {
        self.count > self.limit
    }

    pub fn remaining(self) -> i64 {
        (self.limit - self.count).max(0)
    }
}

/// Quota persistence failure.
#[derive(Debug)]
pub struct QuotaError {
    pub message: String,
}

impl QuotaError {
    fn dependency(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for QuotaError {}

/// Daily period key in the account timezone (`YYYY-MM-DD`).
pub fn daily_period_key(now: DateTime<Utc>, timezone: &str) -> String {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    now.with_timezone(&tz).format("%Y-%m-%d").to_string()
}

/// Monthly period key in UTC (`YYYY-MM`).
pub fn monthly_period_key(now: DateTime<Utc>) -> String {
    now.format("%Y-%m").to_string()
}

/// Remaining quota before another increment would exceed the limit.
pub async fn remaining_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    scope_id: &str,
    period: &str,
    metric: &str,
    limit: i64,
) -> Result<i64, QuotaError> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT count, limit_value
        FROM usage_counters
        WHERE scope = $1 AND scope_id = $2 AND period = $3 AND metric = $4
        "#,
    )
    .bind(scope)
    .bind(scope_id)
    .bind(period)
    .bind(metric)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| QuotaError::dependency("usage counter lookup failed"))?;

    let (count, _) = row.unwrap_or((0, limit));
    Ok((limit - count).max(0))
}

/// Atomically increment a usage counter and return the post-increment totals.
pub async fn increment_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    scope_id: &str,
    period: &str,
    metric: &str,
    limit: i64,
) -> Result<QuotaOutcome, QuotaError> {
    let (count, limit_value): (i64, i64) = sqlx::query_as(
        r#"
        INSERT INTO usage_counters (scope, scope_id, period, metric, count, limit_value)
        VALUES ($1, $2, $3, $4, 1, $5)
        ON CONFLICT (scope, scope_id, period, metric)
        DO UPDATE SET
            count = usage_counters.count + 1,
            limit_value = EXCLUDED.limit_value,
            updated_at = NOW()
        RETURNING count, limit_value
        "#,
    )
    .bind(scope)
    .bind(scope_id)
    .bind(period)
    .bind(metric)
    .bind(limit)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| QuotaError::dependency("usage counter increment failed"))?;

    Ok(QuotaOutcome {
        count,
        limit: limit_value,
    })
}
