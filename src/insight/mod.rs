//! Deterministic insight snapshots and optional aggregate-only narratives.

mod compute;
mod narrator;

pub use compute::{AGGREGATE_SCHEMA_VERSION, SNAPSHOT_SCHEMA_NAME, compute_aggregate};
pub use narrator::{FakeNarrator, InsightNarrator};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::ingress::IngressPolicy;
use crate::quota::{
    METRIC_INSIGHT_NARRATIVES, SCOPE_ACCOUNT, increment_in_transaction, monthly_period_key,
    remaining_in_transaction,
};
use crate::schedule::{Frequency, account_serialization_key, interactive_period};
use crate::work::{EnqueueRequest, WorkStore};

pub const JOB_TYPE_INSIGHT_NARRATE: &str = "insight.narrate";
pub const INSIGHT_NARRATE_PAYLOAD_VERSION: i32 = 1;

/// Insight persistence or narrative failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsightError {
    pub message: String,
}

impl InsightError {
    pub fn dependency(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for InsightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InsightError {}

/// Supported interactive snapshot period kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightPeriodKind {
    Day,
    Week,
    Month,
}

impl InsightPeriodKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    pub fn frequency(self) -> Frequency {
        match self {
            Self::Day => Frequency::Daily,
            Self::Week => Frequency::Weekly,
            Self::Month => Frequency::Monthly,
        }
    }
}

/// Versioned `insight.narrate` job payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsightNarratePayload {
    pub schema_version: i32,
    pub account_id: Uuid,
    pub snapshot_id: Uuid,
    pub aggregate_digest: String,
}

pub fn narrate_dedupe_key(snapshot_id: Uuid, aggregate_digest: &str) -> String {
    format!("insight.narrate:{snapshot_id}:{aggregate_digest}")
}

pub fn aggregate_digest(aggregate: &Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(aggregate).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// Upsert a deterministic snapshot and optionally enqueue narrative generation.
pub async fn record_snapshot_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    period_kind: InsightPeriodKind,
    observed_at: DateTime<Utc>,
    timezone: &str,
    fallback_currency: &str,
    policy: &IngressPolicy,
) -> Result<(Uuid, Value), InsightError> {
    let (period, _label) = interactive_period(period_kind.frequency(), observed_at, timezone)
        .map_err(|_| InsightError::validation("invalid timezone for insight period"))?;
    let aggregate =
        compute_aggregate(tx, account_id, period.start, period.end, fallback_currency).await?;
    let snapshot = upsert_snapshot(
        tx,
        account_id,
        period_kind.as_str(),
        period.start,
        period.end,
        timezone,
        &aggregate,
    )
    .await?;

    if policy.insights_llm_enabled && snapshot.narrative_text.is_none() {
        let limit = i64::try_from(policy.monthly_insight_narratives).unwrap_or(i64::MAX);
        let period_key = monthly_period_key(observed_at);
        let remaining = remaining_in_transaction(
            tx,
            SCOPE_ACCOUNT,
            &account_id.to_string(),
            &period_key,
            METRIC_INSIGHT_NARRATIVES,
            limit,
        )
        .await
        .map_err(|_| InsightError::dependency("insight narrative quota lookup failed"))?;
        if remaining > 0 {
            enqueue_narrate_in_transaction(tx, account_id, snapshot.id, &aggregate, observed_at)
                .await?;
        }
    }

    Ok((snapshot.id, aggregate))
}

struct SnapshotRow {
    id: Uuid,
    narrative_text: Option<String>,
}

async fn upsert_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    period_kind: &str,
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
    timezone: &str,
    aggregate: &Value,
) -> Result<SnapshotRow, InsightError> {
    let snapshot_id = Uuid::new_v4();
    let row: (Uuid, Option<String>) = sqlx::query_as(
        r#"
        INSERT INTO insight_snapshots (
            id, account_id, period_kind, period_start, period_end, timezone, schema_name, aggregate
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (account_id, period_kind, period_start, timezone, schema_name)
        DO UPDATE SET
            period_end = EXCLUDED.period_end,
            aggregate = EXCLUDED.aggregate,
            narrative_text = CASE
                WHEN insight_snapshots.aggregate IS NOT DISTINCT FROM EXCLUDED.aggregate
                THEN insight_snapshots.narrative_text
                ELSE NULL
            END,
            narrative_profile = CASE
                WHEN insight_snapshots.aggregate IS NOT DISTINCT FROM EXCLUDED.aggregate
                THEN insight_snapshots.narrative_profile
                ELSE NULL
            END,
            narrative_generated_at = CASE
                WHEN insight_snapshots.aggregate IS NOT DISTINCT FROM EXCLUDED.aggregate
                THEN insight_snapshots.narrative_generated_at
                ELSE NULL
            END
        RETURNING id, narrative_text
        "#,
    )
    .bind(snapshot_id)
    .bind(account_id)
    .bind(period_kind)
    .bind(period_start)
    .bind(period_end)
    .bind(timezone)
    .bind(SNAPSHOT_SCHEMA_NAME)
    .bind(aggregate)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| InsightError::dependency("insight snapshot upsert failed"))?;
    Ok(SnapshotRow {
        id: row.0,
        narrative_text: row.1,
    })
}

async fn enqueue_narrate_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    account_id: Uuid,
    snapshot_id: Uuid,
    aggregate: &Value,
    run_at: DateTime<Utc>,
) -> Result<(), InsightError> {
    let aggregate_digest = aggregate_digest(aggregate);
    let payload = InsightNarratePayload {
        schema_version: INSIGHT_NARRATE_PAYLOAD_VERSION,
        account_id,
        snapshot_id,
        aggregate_digest: aggregate_digest.clone(),
    };
    WorkStore::enqueue_in_transaction(
        tx,
        EnqueueRequest {
            id: Uuid::new_v4(),
            job_type: JOB_TYPE_INSIGHT_NARRATE.to_string(),
            payload: serde_json::to_value(&payload)
                .map_err(|_| InsightError::dependency("insight narrate payload encode failed"))?,
            dedupe_key: narrate_dedupe_key(snapshot_id, &aggregate_digest),
            serialization_key: Some(account_serialization_key(account_id)),
            priority: 0,
            run_at,
            max_attempts: 10,
        },
    )
    .await
    .map_err(|_| InsightError::dependency("insight narrate enqueue failed"))?;
    Ok(())
}

/// Generate and persist an optional narrative from snapshot aggregate JSON only.
pub async fn execute_insight_narrate(
    pool: &PgPool,
    narrator: &dyn InsightNarrator,
    payload: &InsightNarratePayload,
    monthly_limit: u64,
) -> Result<(), InsightError> {
    if payload.schema_version != INSIGHT_NARRATE_PAYLOAD_VERSION {
        return Err(InsightError::validation(
            "unsupported insight narrate payload version",
        ));
    }

    let row: Option<(Value, Option<String>)> = sqlx::query_as(
        r#"
        SELECT aggregate, narrative_text
        FROM insight_snapshots
        WHERE id = $1 AND account_id = $2
        "#,
    )
    .bind(payload.snapshot_id)
    .bind(payload.account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| InsightError::dependency("insight snapshot lookup failed"))?;

    let Some((aggregate, narrative_text)) = row else {
        return Ok(());
    };
    if narrative_text.is_some() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .map_err(|_| InsightError::dependency("insight narrate transaction begin failed"))?;
    let limit = i64::try_from(monthly_limit).unwrap_or(i64::MAX);
    let period_key = monthly_period_key(Utc::now());
    let outcome = increment_in_transaction(
        &mut tx,
        SCOPE_ACCOUNT,
        &payload.account_id.to_string(),
        &period_key,
        METRIC_INSIGHT_NARRATIVES,
        limit,
    )
    .await
    .map_err(|_| InsightError::dependency("insight narrative quota increment failed"))?;
    if outcome.exceeded() {
        tx.rollback()
            .await
            .map_err(|_| InsightError::dependency("insight narrate rollback failed"))?;
        return Ok(());
    }

    let narrative = narrator.narrate(&aggregate)?;
    let profile = "fake";
    sqlx::query(
        r#"
        UPDATE insight_snapshots
        SET narrative_text = $3,
            narrative_profile = $4,
            narrative_generated_at = NOW()
        WHERE id = $1 AND account_id = $2 AND narrative_text IS NULL
        "#,
    )
    .bind(payload.snapshot_id)
    .bind(payload.account_id)
    .bind(&narrative)
    .bind(profile)
    .execute(&mut *tx)
    .await
    .map_err(|_| InsightError::dependency("insight narrative persist failed"))?;
    tx.commit()
        .await
        .map_err(|_| InsightError::dependency("insight narrate transaction commit failed"))?;
    Ok(())
}

/// Top category display lines for reply enrichment (up to three).
pub fn top_category_lines(aggregate: &Value) -> Vec<(String, i64)> {
    aggregate
        .get("by_category")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let display = row.get("display")?.as_str()?.to_string();
                    let total_minor = row.get("total_minor")?.as_i64()?;
                    Some((display, total_minor))
                })
                .take(3)
                .collect()
        })
        .unwrap_or_default()
}
