//! Ingress domain types for normalized provider events and processing outcomes.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Whether the event arrived via webhook or polling ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressSource {
    Webhook,
    Polling,
}

impl IngressSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Polling => "polling",
        }
    }
}

impl std::str::FromStr for IngressSource {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "webhook" => Ok(Self::Webhook),
            "polling" => Ok(Self::Polling),
            _ => Err(()),
        }
    }
}

/// Observable ingress processing outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressOutcome {
    Accepted { inbound_event_id: Uuid },
    Duplicate { inbound_event_id: Uuid },
    ModeRejected { inbound_event_id: Uuid },
}

/// Account lifecycle states mirrored from PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleState {
    PendingConsent,
    Active,
    Suspended,
    Deleting,
    Deleted,
}

impl LifecycleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PendingConsent => "pending_consent",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
        }
    }
}

impl std::str::FromStr for LifecycleState {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending_consent" => Ok(Self::PendingConsent),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "deleting" => Ok(Self::Deleting),
            "deleted" => Ok(Self::Deleted),
            _ => Err(()),
        }
    }
}

/// Expense lifecycle state for ingress-loaded snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpenseState {
    AwaitingConfirmation,
    Confirmed,
    Rejected,
}

impl ExpenseState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }
}

impl std::str::FromStr for ExpenseState {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "awaiting_confirmation" => Ok(Self::AwaitingConfirmation),
            "confirmed" => Ok(Self::Confirmed),
            "rejected" => Ok(Self::Rejected),
            _ => Err(()),
        }
    }
}

/// Pending action loaded for the decision callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAction {
    pub action_type: String,
    pub payload_ref: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub version: i32,
    pub expense: Option<RecentExpense>,
}

/// Recent expense row exposed to the pure decision callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentExpense {
    pub id: Uuid,
    pub amount_minor: i64,
    pub currency: String,
    pub occurred_at: DateTime<Utc>,
    pub description: String,
    pub source: String,
    pub state: ExpenseState,
    pub version: i32,
}

/// Account and conversation context passed to the synchronous decision callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionContext {
    pub account_id: Option<Uuid>,
    pub lifecycle_state: Option<LifecycleState>,
    pub consent_version: Option<String>,
    pub pending_action: Option<PendingAction>,
    pub confirmed_today_total_minor: i64,
    pub today_currency: String,
    pub recent_expenses: Vec<RecentExpense>,
    pub sender_allowed: bool,
    pub user_text: String,
    pub timezone: String,
    pub original_receipt_retention_days: u32,
    pub next_expense_id: Uuid,
    pub now: DateTime<Utc>,
}

/// Reply intent enqueued atomically with domain effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyIntent {
    pub body: String,
}

/// Pure decision output applied inside the open ingress transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOutput {
    pub effects: Vec<super::effects::IngressEffect>,
    pub reply: Option<ReplyIntent>,
}

/// Normalized ingress request. User text is never persisted on the inbound event row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressRequest {
    pub source: IngressSource,
    pub provider_scope: String,
    pub provider_event_id: String,
    pub provider_sender_id: String,
    pub provider_chat_id: String,
    pub sender_allowed: bool,
    pub user_text: String,
    pub observed_at: DateTime<Utc>,
}
