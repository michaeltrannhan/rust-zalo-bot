//! Deterministic insight-v1 aggregate computation from confirmed expenses.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::InsightError;

pub const AGGREGATE_SCHEMA_VERSION: i32 = 1;
pub const SNAPSHOT_SCHEMA_NAME: &str = "insight-v1";

/// Build the insight-v1 aggregate JSON for confirmed expenses in `[period_start, period_end)`.
pub async fn compute_aggregate(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    fallback_currency: &str,
) -> Result<Value, InsightError> {
    let totals: Option<(i64, i64, String)> = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(amount_minor), 0)::BIGINT,
               COUNT(*)::BIGINT,
               COALESCE(MIN(currency), $4)
        FROM expenses
        WHERE account_id = $1
          AND state = 'confirmed'
          AND occurred_at >= $2
          AND occurred_at < $3
        "#,
    )
    .bind(account_id)
    .bind(period_start)
    .bind(period_end)
    .bind(fallback_currency)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| InsightError::dependency("insight totals query failed"))?;

    let (total_minor, tx_count, currency) = totals.unwrap_or((0, 0, fallback_currency.to_string()));

    type CategoryRow = (String, String, i64, i64);
    let category_rows: Vec<CategoryRow> = sqlx::query_as(
        r#"
        SELECT COALESCE(e.category_key, d.category_key, 'khac') AS category_key,
               c.display_name_vi,
               COALESCE(SUM(e.amount_minor), 0)::BIGINT,
               COUNT(*)::BIGINT
        FROM expenses e
        LEFT JOIN expense_drafts d
          ON d.submission_id = e.receipt_submission_id
         AND d.account_id = e.account_id
        JOIN categories c ON c.key = COALESCE(e.category_key, d.category_key, 'khac')
        WHERE e.account_id = $1
          AND e.state = 'confirmed'
          AND e.occurred_at >= $2
          AND e.occurred_at < $3
        GROUP BY COALESCE(e.category_key, d.category_key, 'khac'), c.display_name_vi
        ORDER BY SUM(e.amount_minor) DESC, c.display_name_vi ASC
        "#,
    )
    .bind(account_id)
    .bind(period_start)
    .bind(period_end)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| InsightError::dependency("insight category query failed"))?;

    let by_category: Vec<Value> = category_rows
        .into_iter()
        .map(|(key, display, total, count)| {
            json!({
                "key": key,
                "display": display,
                "total_minor": total,
                "count": count,
            })
        })
        .collect();

    type MerchantRow = (String, i64, i64);
    let merchant_rows: Vec<MerchantRow> = sqlx::query_as(
        r#"
        SELECT description,
               COALESCE(SUM(amount_minor), 0)::BIGINT,
               COUNT(*)::BIGINT
        FROM expenses
        WHERE account_id = $1
          AND state = 'confirmed'
          AND occurred_at >= $2
          AND occurred_at < $3
        GROUP BY description
        ORDER BY SUM(amount_minor) DESC, description ASC
        LIMIT 5
        "#,
    )
    .bind(account_id)
    .bind(period_start)
    .bind(period_end)
    .fetch_all(&mut **tx)
    .await
    .map_err(|_| InsightError::dependency("insight merchant query failed"))?;

    let top_merchants: Vec<Value> = merchant_rows
        .into_iter()
        .map(|(name, total, count)| {
            json!({
                "name": name,
                "total_minor": total,
                "count": count,
            })
        })
        .collect();

    Ok(json!({
        "schema_version": AGGREGATE_SCHEMA_VERSION,
        "total_minor": total_minor,
        "currency": currency,
        "tx_count": tx_count,
        "by_category": by_category,
        "top_merchants": top_merchants,
    }))
}
