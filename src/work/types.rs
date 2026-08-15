//! Durable-work request and response types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ErrorClass;

/// Minimum fields every job payload must carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedPayload {
    pub schema_version: i32,
    #[serde(flatten)]
    pub body: Value,
}

/// Input for enqueueing one durable job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueRequest {
    pub id: Uuid,
    pub job_type: String,
    pub payload: Value,
    pub dedupe_key: String,
    pub serialization_key: Option<String>,
    pub priority: i32,
    pub run_at: DateTime<Utc>,
    pub max_attempts: i32,
}

/// Result of an enqueue attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued,
    Duplicate,
}

/// Claim batch configuration supplied by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOptions {
    pub batch_limit: i32,
    pub lease_owner: String,
    pub lease_duration_secs: i64,
}

/// A leased job ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub job_type: String,
    pub payload: Value,
    pub payload_version: i32,
    pub attempt_number: i32,
    pub lease_token: Uuid,
    pub lease_owner: String,
    pub lease_deadline: DateTime<Utc>,
    pub dedupe_key: String,
    pub serialization_key: Option<String>,
}

/// Redacted job summary for logging and operator views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobSummary {
    pub id: Uuid,
    pub job_type: String,
    pub payload_version: i32,
    pub state: JobState,
    pub priority: i32,
    pub run_at: DateTime<Utc>,
    pub dedupe_key: String,
    pub serialization_key: Option<String>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub last_error_class: Option<String>,
}

/// Redacted attempt audit row — never includes payload content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptSummary {
    pub id: Uuid,
    pub job_id: Uuid,
    pub attempt_number: i32,
    pub lease_owner: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub outcome: Option<AttemptOutcome>,
    pub error_class: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Leased,
    Completed,
    Cancelled,
    Dead,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Dead => "dead",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "leased" => Some(Self::Leased),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "dead" => Some(Self::Dead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Completed,
    Failed,
    Cancelled,
    LostLease,
    Superseded,
}

impl AttemptOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::LostLease => "lost_lease",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "lost_lease" => Some(Self::LostLease),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOutcome {
    Retried,
    DeadLettered,
}

/// Whether a classified error should schedule another attempt.
pub fn is_retryable(class: ErrorClass) -> bool {
    matches!(
        class,
        ErrorClass::Transient
            | ErrorClass::Timeout
            | ErrorClass::RateLimited
            | ErrorClass::Dependency
            | ErrorClass::ProviderError
    )
}

/// Bounded exponential backoff for retry scheduling.
pub fn retry_delay_secs(attempt_count: i32) -> i64 {
    let base = 1_i64;
    let max = 300_i64;
    let exponent = attempt_count.saturating_sub(1).clamp(0, 8) as u32;
    base.saturating_mul(1_i64 << exponent).min(max)
}
