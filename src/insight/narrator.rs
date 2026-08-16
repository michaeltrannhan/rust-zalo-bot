//! Optional aggregate-only insight narrative seam.

use serde_json::Value;

use super::InsightError;
use crate::conversation::format_minor;

/// Narrates a spending period from aggregate JSON only.
pub trait InsightNarrator: Send + Sync {
    fn narrate(&self, aggregate_json: &Value) -> Result<String, InsightError>;
}

/// Deterministic Vietnamese narrative for tests and local runs.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeNarrator;

impl InsightNarrator for FakeNarrator {
    fn narrate(&self, aggregate_json: &Value) -> Result<String, InsightError> {
        let total_minor = aggregate_json
            .get("total_minor")
            .and_then(Value::as_i64)
            .ok_or_else(|| InsightError::validation("aggregate missing total_minor"))?;
        let tx_count = aggregate_json
            .get("tx_count")
            .and_then(Value::as_i64)
            .ok_or_else(|| InsightError::validation("aggregate missing tx_count"))?;
        let currency = aggregate_json
            .get("currency")
            .and_then(Value::as_str)
            .ok_or_else(|| InsightError::validation("aggregate missing currency"))?;
        let amount = format_minor(total_minor, currency);
        Ok(format!(
            "Trong kỳ này bạn đã ghi nhận {tx_count} khoản, tổng {amount}."
        ))
    }
}
