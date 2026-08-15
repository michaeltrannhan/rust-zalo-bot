//! Pure conversation parsing, state transitions, and deterministic replies.

mod amount;
mod decide;
mod fold;
mod money;
mod parse;
mod templates;
mod types;

pub use amount::{AmountError, parse_amount};
pub use decide::{decide, format_date_vn};
pub use money::format_minor;
pub use parse::{IntentKind, parse_intent};
pub use types::{
    AccountContext, CONSENT_VERSION, ConversationOutcome, DomainCommand, LifecycleState,
    ManualDraftView, PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation, PeriodSummary,
    RecentExpenseLine, ReplyPlan,
};
