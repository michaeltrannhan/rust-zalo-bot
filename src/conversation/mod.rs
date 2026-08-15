//! Pure conversation parsing, state transitions, and deterministic replies.

mod amount;
mod decide;
mod fold;
mod money;
mod parse;
mod templates;
mod types;

pub use amount::{AmountError, parse_amount};
pub use decide::{decide, decide_image, format_date_vn};
pub use money::format_minor;
pub use parse::{IntentKind, parse_intent};
pub use templates::{image_received_text, manual_confirmation_card, transaction_type_label};
pub use types::{
    AccountContext, CONSENT_VERSION, ConversationOutcome, DomainCommand, LifecycleState,
    ManualDraftView, PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation, PendingKind,
    PeriodSummary, RecentExpenseLine, ReplyPlan,
};
