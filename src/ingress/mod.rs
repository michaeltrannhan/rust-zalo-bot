//! Normalized webhook/polling acceptance and transactional persistence.

mod application;
mod effects;
mod store;
mod types;

pub use application::process_text_command;
pub use effects::{IngressEffect, IngressEffectError};
pub use store::{IngressError, IngressStore};
pub use types::{
    DecisionContext, DecisionOutput, ExpenseState, IngressOutcome, IngressRequest, IngressSource,
    LifecycleState, PendingAction, RecentExpense, ReplyIntent,
};
