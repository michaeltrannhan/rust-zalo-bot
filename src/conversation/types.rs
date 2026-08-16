//! Public conversation seam types.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Account posture and read-model inputs for a pure decision.
#[derive(Debug, Clone)]
pub struct AccountContext {
    /// Stable ID supplied by the application/randomness seam for a possible new draft.
    pub next_expense_id: Uuid,
    /// Stable ID for a new receipt submission when accepting an image event.
    pub next_submission_id: Uuid,
    /// Stable ID for the receipt.ingest job paired with `next_submission_id`.
    pub next_ingest_job_id: Uuid,
    pub lifecycle: LifecycleState,
    pub allowlisted: bool,
    pub default_currency: String,
    pub timezone: String,
    pub original_receipt_retention_days: u32,
    /// Remaining daily receipt submissions before quota is exceeded.
    pub remaining_daily_receipts: i64,
    pub confirmed_expense_count: u32,
    pub pending: Option<PendingConfirmation>,
    pub today_summary: Option<PeriodSummary>,
    pub week_summary: Option<PeriodSummary>,
    pub month_summary: Option<PeriodSummary>,
    pub schedules: Vec<ScheduleLine>,
    pub recent_lines: Vec<RecentExpenseLine>,
}

#[derive(Debug, Clone)]
pub struct ScheduleLine {
    pub frequency: String,
    pub delivery_minute: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    ManualExpense,
    ReceiptReview,
    AccountDeletion,
}

/// Open confirmation pending action plus the draft it targets.
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub kind: PendingKind,
    pub reference_id: Uuid,
    pub optimistic_version: u64,
    pub expires_at: DateTime<Utc>,
    pub draft: ManualDraftView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    PendingConsent,
    Active,
    Suspended,
}

#[derive(Debug, Clone)]
pub struct ManualDraftView {
    pub version: u64,
    pub amount_minor: i64,
    pub currency: String,
    pub merchant: String,
    pub category_display: String,
    pub type_label: String,
    pub date_display: String,
}

#[derive(Debug, Clone)]
pub struct PeriodSummary {
    pub label: String,
    pub currency: String,
    pub total_minor: i64,
    pub tx_count: u32,
}

#[derive(Debug, Clone)]
pub struct RecentExpenseLine {
    pub date_display: String,
    pub amount_minor: i64,
    pub currency: String,
    pub merchant: String,
    pub category_display: String,
    pub type_label: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationOutcome {
    pub replies: Vec<ReplyPlan>,
    pub commands: Vec<DomainCommand>,
}

#[derive(Debug, Clone)]
pub struct ReplyPlan {
    pub body: String,
}

impl ReplyPlan {
    pub fn single(body: impl Into<String>) -> Self {
        Self { body: body.into() }
    }
}

/// Typed effects the ingress layer applies inside one database transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainCommand {
    GrantConsent {
        consent_version: String,
    },
    CreateManualAwaitingConfirmation {
        expense_id: Uuid,
        amount_minor: i64,
        currency: String,
        description: String,
        merchant: String,
        occurred_at: DateTime<Utc>,
        optimistic_version: u64,
        pending_expires_at: DateTime<Utc>,
    },
    ConfirmExpense {
        expense_id: Uuid,
        expected_version: u64,
    },
    RejectExpense {
        expense_id: Uuid,
        expected_version: u64,
    },
    AcceptReceiptSubmission {
        submission_id: Uuid,
        ingest_job_id: Uuid,
    },
    ConfirmReceipt {
        submission_id: Uuid,
        expense_id: Uuid,
        expected_draft_version: u64,
    },
    RejectReceipt {
        submission_id: Uuid,
        expected_draft_version: u64,
    },
    EditReceiptAmount {
        submission_id: Uuid,
        expected_draft_version: u64,
        amount_minor: i64,
    },
    ClearPending,
    SetTimezone {
        iana: String,
    },
    UpsertSchedule {
        frequency: String,
        delivery_minute: i32,
    },
    DisableSchedule {
        frequency: Option<String>,
    },
    RequestAccountDeletion {
        pending_expires_at: DateTime<Utc>,
    },
    ConfirmAccountDeletion,
    RequestAccountExport,
    RecordInsightSnapshot {
        period_kind: String,
    },
}

/// Versioned consent copy identifier.
pub const CONSENT_VERSION: &str = "consent-v1";

/// Pending manual confirmation lifetime.
pub const PENDING_CONFIRMATION_TTL_SECS: i64 = 15 * 60;
