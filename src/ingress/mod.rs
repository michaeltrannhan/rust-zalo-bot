//! Normalized webhook/polling acceptance and transactional persistence.

mod application;
mod effects;
mod followup;
mod policy;
mod store;
mod types;

pub use application::{
    process_image, process_text_command, store_with_receipt, store_with_receipt_and_policy,
};
pub use effects::{IngressEffect, IngressEffectError};
pub use followup::enqueue_receipt_review_followup;
pub use policy::IngressPolicy;
pub use store::{IngressError, IngressStore, enqueue_outbound_in_transaction};
pub use types::{
    DecisionContext, DecisionOutput, ExpenseState, IngressEventKind, IngressObservation,
    IngressOutcome, IngressRequest, IngressSource, LifecycleState, PendingAction,
    ReceiptDraftSnapshot, RecentExpense, ReplyIntent, SummaryScheduleSnapshot,
};
