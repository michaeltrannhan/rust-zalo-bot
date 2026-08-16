//! Typed domain effects applied transactionally during ingress processing.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Effects the ingress store can apply atomically after the decision callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressEffect {
    GrantConsent {
        consent_version: String,
    },
    CreateManualExpenseAwaitingConfirmation {
        expense_id: Uuid,
        amount_minor: i64,
        currency: String,
        description: String,
        occurred_at: DateTime<Utc>,
        optimistic_version: i32,
        pending_expires_at: DateTime<Utc>,
        pending_action_type: String,
    },
    ConfirmExpense {
        expense_id: Uuid,
        expected_version: i32,
    },
    RejectExpense {
        expense_id: Uuid,
        expected_version: i32,
    },
    SetPendingAction {
        action_type: String,
        payload_ref: Option<String>,
        expires_at: DateTime<Utc>,
        expected_version: Option<i32>,
    },
    ClearPendingAction {
        expected_version: i32,
    },
    AcceptReceiptSubmission {
        submission_id: Uuid,
        ingest_job_id: Uuid,
        inbound_event_id: Uuid,
    },
    ConfirmReceipt {
        submission_id: Uuid,
        expense_id: Uuid,
        expected_draft_version: i32,
    },
    RejectReceipt {
        submission_id: Uuid,
        expected_draft_version: i32,
    },
    EditReceiptDraft {
        submission_id: Uuid,
        expected_draft_version: i32,
        amount_minor: i64,
    },
    SetTimezone {
        iana: String,
    },
    UpsertSummarySchedule {
        frequency: String,
        delivery_minute: i32,
    },
    DisableSummarySchedule {
        frequency: Option<String>,
    },
    ArmAccountDeletion {
        expires_at: DateTime<Utc>,
    },
    ConfirmAccountDeletion,
    RequestAccountExport,
    RecordInsightSnapshot {
        period_kind: String,
    },
    ReadOnly,
}

/// Effect application failure that rolls back the ingress transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressEffectError {
    VersionConflict,
    NotFound,
    InvalidTransition,
}

impl std::fmt::Display for IngressEffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionConflict => f.write_str("version conflict"),
            Self::NotFound => f.write_str("not found"),
            Self::InvalidTransition => f.write_str("invalid transition"),
        }
    }
}

impl std::error::Error for IngressEffectError {}
