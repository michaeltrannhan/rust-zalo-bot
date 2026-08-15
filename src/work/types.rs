//! Durable-work request and response types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ErrorClass;

/// Minimum fields every job payload must carry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedPayload {
    pub schema_version: i32,
    #[serde(flatten)]
    pub body: Value,
}

impl std::fmt::Debug for VersionedPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VersionedPayload")
            .field("schema_version", &self.schema_version)
            .field("body", &"[REDACTED]")
            .finish()
    }
}

/// Input for enqueueing one durable job.
#[derive(Clone, PartialEq, Eq)]
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

impl std::fmt::Debug for EnqueueRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnqueueRequest")
            .field("id", &self.id)
            .field("job_type", &self.job_type)
            .field("payload", &"[REDACTED]")
            .field("dedupe_key", &"[REDACTED]")
            .field(
                "serialization_key",
                &self.serialization_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("priority", &self.priority)
            .field("run_at", &self.run_at)
            .field("max_attempts", &self.max_attempts)
            .finish()
    }
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
#[derive(Clone, PartialEq, Eq)]
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

impl std::fmt::Debug for ClaimedJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaimedJob")
            .field("id", &self.id)
            .field("job_type", &self.job_type)
            .field("payload", &"[REDACTED]")
            .field("payload_version", &self.payload_version)
            .field("attempt_number", &self.attempt_number)
            .field("lease_token", &"[REDACTED]")
            .field("lease_owner", &self.lease_owner)
            .field("lease_deadline", &self.lease_deadline)
            .field("dedupe_key", &"[REDACTED]")
            .field(
                "serialization_key",
                &self.serialization_key.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Redacted job summary for logging and operator views.
#[derive(Clone, PartialEq, Eq)]
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

impl std::fmt::Debug for JobSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobSummary")
            .field("id", &self.id)
            .field("job_type", &self.job_type)
            .field("payload_version", &self.payload_version)
            .field("state", &self.state)
            .field("priority", &self.priority)
            .field("run_at", &self.run_at)
            .field("dedupe_key", &"[REDACTED]")
            .field(
                "serialization_key",
                &self.serialization_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("attempt_count", &self.attempt_count)
            .field("max_attempts", &self.max_attempts)
            .field("last_error_class", &self.last_error_class)
            .finish()
    }
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
