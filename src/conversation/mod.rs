//! Pure conversation parsing, state transitions, and deterministic replies.

mod amount;
mod categories;
mod decide;
mod fold;
mod locale;
mod money;
mod parse;
mod templates;
mod types;

pub use amount::{AmountError, parse_amount};
pub use categories::{category_display, format_category_list, resolve_category_key};
pub use decide::{decide, decide_image, format_date_vn};
pub use locale::{Locale, category_display_for};
pub use money::format_minor;
pub use parse::{IntentKind, parse_intent};
pub use templates::{
    daily_receipt_quota_text, empty_summary_text, extraction_failed_text,
    extraction_kill_switch_text, extraction_unsupported_text, image_received_text,
    manual_confirmation_card, period_label_month, period_label_today, period_label_week,
    period_summary_text, today_summary_text, transaction_type_label,
};
pub use types::{
    AccountContext, CONSENT_VERSION, ConversationOutcome, DomainCommand, LifecycleState,
    ManualDraftView, PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation, PendingKind,
    PeriodSummary, RecentExpenseLine, ReplyPlan, ScheduleLine,
};
