//! Receipt lifecycle domain types.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const REDACTED: &str = "[REDACTED]";

fn redacted_opt<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| REDACTED)
}

/// Observable receipt lifecycle states from the public seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Pending,
    Queued,
    Stored,
    Extracting,
    ReviewRequired,
    Confirmed,
    Rejected,
    FailedTransient,
    FailedPermanent,
    Expired,
    Deleted,
}

impl ReceiptState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Queued => "queued",
            Self::Stored => "stored",
            Self::Extracting => "extracting",
            Self::ReviewRequired => "review_required",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
            Self::FailedTransient => "failed_transient",
            Self::FailedPermanent => "failed_permanent",
            Self::Expired => "expired",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "queued" => Some(Self::Queued),
            "stored" => Some(Self::Stored),
            "extracting" => Some(Self::Extracting),
            "review_required" => Some(Self::ReviewRequired),
            "confirmed" => Some(Self::Confirmed),
            "rejected" => Some(Self::Rejected),
            "failed_transient" => Some(Self::FailedTransient),
            "failed_permanent" => Some(Self::FailedPermanent),
            "expired" => Some(Self::Expired),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Confirmed
                | Self::Rejected
                | Self::Expired
                | Self::FailedPermanent
                | Self::Deleted
        )
    }
}

/// Whether a lifecycle transition is legal.
pub fn can_transition(from: ReceiptState, to: ReceiptState) -> bool {
    use ReceiptState::*;
    matches!(
        (from, to),
        (Pending, Queued)
            | (Queued, Stored)
            | (Queued, FailedTransient)
            | (Queued, FailedPermanent)
            | (FailedTransient, Queued)
            | (Stored, Extracting)
            | (FailedTransient, Extracting)
            | (Extracting, ReviewRequired)
            | (Extracting, FailedTransient)
            | (Extracting, FailedPermanent)
            | (ReviewRequired, Confirmed)
            | (ReviewRequired, Rejected)
            | (ReviewRequired, Expired)
            | (Stored, Deleted)
            | (Extracting, Deleted)
            | (ReviewRequired, Deleted)
            | (Confirmed, Deleted)
            | (Rejected, Deleted)
            | (Expired, Deleted)
            | (FailedPermanent, Deleted)
            | (FailedTransient, Deleted)
    )
}

/// Versioned job payload for receipt.ingest and receipt.extract.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptJobPayload {
    pub schema_version: i32,
    pub receipt_submission_id: Uuid,
}

impl ReceiptJobPayload {
    pub fn new(submission_id: Uuid) -> Self {
        Self {
            schema_version: 1,
            receipt_submission_id: submission_id,
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("receipt job payload serializes")
    }
}

impl fmt::Debug for ReceiptJobPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceiptJobPayload")
            .field("schema_version", &self.schema_version)
            .field("receipt_submission_id", &REDACTED)
            .finish()
    }
}

pub const JOB_TYPE_INGEST: &str = "receipt.ingest";
pub const JOB_TYPE_EXTRACT: &str = "receipt.extract";

pub fn ingest_dedupe_key(submission_id: Uuid) -> String {
    format!("receipt.ingest:{submission_id}")
}

pub fn extract_dedupe_key(submission_id: Uuid) -> String {
    format!("receipt.extract:{submission_id}")
}

pub fn account_serialization_key(account_id: Uuid) -> String {
    format!("account:{account_id}")
}

/// Request to accept a new receipt submission.
#[derive(Clone, PartialEq, Eq)]
pub struct AcceptSubmissionRequest {
    pub submission_id: Uuid,
    pub account_id: Uuid,
    pub inbound_event_id: Option<Uuid>,
    pub ingest_job_id: Uuid,
}

impl fmt::Debug for AcceptSubmissionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcceptSubmissionRequest")
            .field("submission_id", &REDACTED)
            .field("account_id", &REDACTED)
            .field("inbound_event_id", &redacted_opt(&self.inbound_event_id))
            .field("ingest_job_id", &REDACTED)
            .finish()
    }
}

/// Outcome of accepting a submission.
#[derive(Clone, PartialEq, Eq)]
pub enum AcceptSubmissionOutcome {
    Accepted {
        state: ReceiptState,
    },
    Replayed {
        submission_id: Uuid,
        state: ReceiptState,
    },
    DuplicateJob,
}

impl fmt::Debug for AcceptSubmissionOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted { state } => formatter
                .debug_struct("Accepted")
                .field("state", state)
                .finish(),
            Self::Replayed {
                submission_id: _,
                state,
            } => formatter
                .debug_struct("Replayed")
                .field("submission_id", &REDACTED)
                .field("state", state)
                .finish(),
            Self::DuplicateJob => formatter.debug_tuple("DuplicateJob").finish(),
        }
    }
}

/// Validated image metadata after bounded acceptance.
#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedImage {
    pub content_sha256: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub width_px: i32,
    pub height_px: i32,
}

impl fmt::Debug for ValidatedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedImage")
            .field("content_sha256", &REDACTED)
            .field("mime_type", &self.mime_type)
            .field("size_bytes", &self.size_bytes)
            .field("width_px", &self.width_px)
            .field("height_px", &self.height_px)
            .finish()
    }
}

/// Outcome of ingest processing.
#[derive(Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Stored {
        submission_id: Uuid,
        content_sha256: String,
    },
    DuplicateAbsorbed {
        submission_id: Uuid,
        original_submission_id: Uuid,
    },
    AlreadyStored {
        submission_id: Uuid,
    },
    AlreadyTerminal {
        state: ReceiptState,
    },
}

impl fmt::Debug for IngestOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stored {
                submission_id: _,
                content_sha256: _,
            } => formatter
                .debug_struct("Stored")
                .field("submission_id", &REDACTED)
                .field("content_sha256", &REDACTED)
                .finish(),
            Self::DuplicateAbsorbed {
                submission_id: _,
                original_submission_id: _,
            } => formatter
                .debug_struct("DuplicateAbsorbed")
                .field("submission_id", &REDACTED)
                .field("original_submission_id", &REDACTED)
                .finish(),
            Self::AlreadyStored { submission_id: _ } => formatter
                .debug_struct("AlreadyStored")
                .field("submission_id", &REDACTED)
                .finish(),
            Self::AlreadyTerminal { state } => formatter
                .debug_struct("AlreadyTerminal")
                .field("state", state)
                .finish(),
        }
    }
}

/// Outcome of extraction processing.
#[derive(Clone, PartialEq, Eq)]
pub enum ExtractOutcome {
    ReviewRequired { submission_id: Uuid, draft_id: Uuid },
    AlreadyReviewRequired { submission_id: Uuid },
    AlreadyTerminal { state: ReceiptState },
    Unsupported,
}

impl fmt::Debug for ExtractOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReviewRequired {
                submission_id: _,
                draft_id: _,
            } => formatter
                .debug_struct("ReviewRequired")
                .field("submission_id", &REDACTED)
                .field("draft_id", &REDACTED)
                .finish(),
            Self::AlreadyReviewRequired { submission_id: _ } => formatter
                .debug_struct("AlreadyReviewRequired")
                .field("submission_id", &REDACTED)
                .finish(),
            Self::AlreadyTerminal { state } => formatter
                .debug_struct("AlreadyTerminal")
                .field("state", state)
                .finish(),
            Self::Unsupported => formatter.debug_tuple("Unsupported").finish(),
        }
    }
}

/// Public state view for a submission.
#[derive(Clone, PartialEq, Eq)]
pub struct ReceiptStateView {
    pub submission_id: Uuid,
    pub account_id: Uuid,
    pub state: ReceiptState,
    pub version: i32,
    pub duplicate_of_submission_id: Option<Uuid>,
    pub confirmed_expense_id: Option<Uuid>,
    pub failure_error_class: Option<String>,
    pub review_expires_at: Option<DateTime<Utc>>,
    pub asset_deleted: bool,
}

impl fmt::Debug for ReceiptStateView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceiptStateView")
            .field("submission_id", &REDACTED)
            .field("account_id", &REDACTED)
            .field("state", &self.state)
            .field("version", &self.version)
            .field(
                "duplicate_of_submission_id",
                &redacted_opt(&self.duplicate_of_submission_id),
            )
            .field(
                "confirmed_expense_id",
                &redacted_opt(&self.confirmed_expense_id),
            )
            .field("failure_error_class", &self.failure_error_class)
            .field("review_expires_at", &self.review_expires_at)
            .field("asset_deleted", &self.asset_deleted)
            .finish()
    }
}

/// Draft fields exposed for review and edit.
#[derive(Clone, PartialEq)]
pub struct ExpenseDraftView {
    pub draft_id: Uuid,
    pub submission_id: Uuid,
    pub account_id: Uuid,
    pub amount_minor: i64,
    pub currency: String,
    pub merchant: String,
    pub category_key: String,
    pub transaction_type: String,
    pub occurred_at: DateTime<Utc>,
    pub confidence: Option<f32>,
    pub version: i32,
}

impl fmt::Debug for ExpenseDraftView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpenseDraftView")
            .field("draft_id", &REDACTED)
            .field("submission_id", &REDACTED)
            .field("account_id", &REDACTED)
            .field("amount_minor", &REDACTED)
            .field("currency", &self.currency)
            .field("merchant", &REDACTED)
            .field("category_key", &self.category_key)
            .field("transaction_type", &self.transaction_type)
            .field("occurred_at", &REDACTED)
            .field("confidence", &REDACTED)
            .field("version", &self.version)
            .finish()
    }
}

/// Draft edit request with optimistic version.
#[derive(Clone, PartialEq, Eq)]
pub struct EditDraftRequest {
    pub account_id: Uuid,
    pub submission_id: Uuid,
    pub expected_version: i32,
    pub amount_minor: Option<i64>,
    pub currency: Option<String>,
    pub merchant: Option<String>,
    pub category_key: Option<String>,
    pub occurred_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for EditDraftRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EditDraftRequest")
            .field("account_id", &REDACTED)
            .field("submission_id", &REDACTED)
            .field("expected_version", &self.expected_version)
            .field("amount_minor", &redacted_opt(&self.amount_minor))
            .field("currency", &self.currency)
            .field("merchant", &redacted_opt(&self.merchant))
            .field("category_key", &self.category_key)
            .field("occurred_at", &redacted_opt(&self.occurred_at))
            .finish()
    }
}

/// Confirm request with optimistic draft version.
#[derive(Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    pub account_id: Uuid,
    pub submission_id: Uuid,
    pub expected_draft_version: i32,
    pub expense_id: Uuid,
}

impl fmt::Debug for ConfirmRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfirmRequest")
            .field("account_id", &REDACTED)
            .field("submission_id", &REDACTED)
            .field("expected_draft_version", &self.expected_draft_version)
            .field("expense_id", &REDACTED)
            .finish()
    }
}

/// Outcome of confirming a draft.
#[derive(Clone, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Confirmed { expense_id: Uuid },
    AlreadyConfirmed { expense_id: Uuid },
}

impl fmt::Debug for ConfirmOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confirmed { expense_id: _ } => formatter
                .debug_struct("Confirmed")
                .field("expense_id", &REDACTED)
                .finish(),
            Self::AlreadyConfirmed { expense_id: _ } => formatter
                .debug_struct("AlreadyConfirmed")
                .field("expense_id", &REDACTED)
                .finish(),
        }
    }
}

/// Reject request with optimistic draft version.
#[derive(Clone, PartialEq, Eq)]
pub struct RejectRequest {
    pub account_id: Uuid,
    pub submission_id: Uuid,
    pub expected_draft_version: i32,
}

impl fmt::Debug for RejectRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RejectRequest")
            .field("account_id", &REDACTED)
            .field("submission_id", &REDACTED)
            .field("expected_draft_version", &self.expected_draft_version)
            .finish()
    }
}

/// Deterministic extraction result from the fake extractor.
#[derive(Clone, PartialEq)]
pub struct ExtractionResult {
    pub merchant: String,
    pub amount_minor: i64,
    pub currency: String,
    pub category_key: String,
    pub transaction_type: String,
    pub occurred_at: DateTime<Utc>,
    pub confidence: f32,
    pub unsupported: bool,
}

impl fmt::Debug for ExtractionResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtractionResult")
            .field("merchant", &REDACTED)
            .field("amount_minor", &REDACTED)
            .field("currency", &self.currency)
            .field("category_key", &self.category_key)
            .field("transaction_type", &self.transaction_type)
            .field("occurred_at", &REDACTED)
            .field("confidence", &REDACTED)
            .field("unsupported", &self.unsupported)
            .finish()
    }
}

/// Configuration for receipt lifecycle behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptConfig {
    pub original_receipt_days: u32,
    pub review_expiry_hours: i64,
}

impl Default for ReceiptConfig {
    fn default() -> Self {
        Self {
            original_receipt_days: 7,
            review_expiry_hours: 72,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_debug_redacts_identifiers_and_extraction_fields() {
        let submission_id = Uuid::new_v4();
        let account_id = Uuid::new_v4();
        let request = AcceptSubmissionRequest {
            submission_id,
            account_id,
            inbound_event_id: Some(Uuid::new_v4()),
            ingest_job_id: Uuid::new_v4(),
        };
        let debug = format!("{request:?}");
        assert!(debug.contains(REDACTED));
        assert!(!debug.contains(&submission_id.to_string()));
        assert!(!debug.contains(&account_id.to_string()));

        let extraction = ExtractionResult {
            merchant: "Secret Merchant".to_string(),
            amount_minor: 42_000,
            currency: "VND".to_string(),
            category_key: "khac".to_string(),
            transaction_type: "expense".to_string(),
            occurred_at: Utc::now(),
            confidence: 0.9,
            unsupported: false,
        };
        let debug = format!("{extraction:?}");
        assert!(!debug.contains("Secret Merchant"));
        assert!(!debug.contains("42000"));
        assert!(!debug.contains("0.9"));
    }
}
