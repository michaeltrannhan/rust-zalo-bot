//! Normalized webhook/polling acceptance and transactional persistence.

mod application;
mod effects;
mod store;
mod types;

pub use application::{process_image, process_text_command, store_with_receipt};
pub use effects::{IngressEffect, IngressEffectError};
pub use store::{IngressError, IngressStore};
pub use types::{
    DecisionContext, DecisionOutput, ExpenseState, IngressEventKind, IngressObservation,
    IngressOutcome, IngressRequest, IngressSource, LifecycleState, PendingAction,
    ReceiptDraftSnapshot, RecentExpense, ReplyIntent,
};
