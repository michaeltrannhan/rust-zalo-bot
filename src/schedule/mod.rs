//! Timezone-aware summary schedules and spending periods.

mod next_run;
mod types;

pub use next_run::{
    ScheduleError, interactive_period, latest_delivery, next_delivery, parse_timezone,
    scheduled_period, validate_delivery_minute,
};
pub use types::{Frequency, Period};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const JOB_TYPE_SCHEDULE_EMIT: &str = "schedule.emit";
pub const SCHEDULE_PAYLOAD_VERSION: i32 = 1;

/// Versioned `schedule.emit` job payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleEmitPayload {
    pub schema_version: i32,
    pub account_id: Uuid,
    pub schedule_id: Uuid,
    pub frequency: String,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
}

pub fn schedule_emit_dedupe_key(
    account_id: Uuid,
    frequency: &str,
    period_start: DateTime<Utc>,
) -> String {
    format!(
        "schedule.emit:{}:{}:{}",
        account_id,
        frequency,
        period_start.to_rfc3339()
    )
}

pub fn account_serialization_key(account_id: Uuid) -> String {
    format!("account:{account_id}")
}
