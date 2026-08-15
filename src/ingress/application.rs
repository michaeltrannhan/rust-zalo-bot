//! Application-level bridge between persistence and the pure conversation seam.

use crate::conversation::{
    self, AccountContext, DomainCommand, LifecycleState as ConversationLifecycle, ManualDraftView,
    PendingConfirmation, PeriodSummary, RecentExpenseLine,
};

use super::effects::{IngressEffect, IngressEffectError};
use super::store::{IngressError, IngressStore};
use super::types::{
    DecisionContext, DecisionOutput, ExpenseState, IngressOutcome, IngressRequest, LifecycleState,
    ReplyIntent,
};

/// Process one normalized text event through the pure conversation seam and
/// commit its state transition plus reply intent in the ingress transaction.
pub async fn process_text_command(
    store: &IngressStore,
    request: IngressRequest,
) -> Result<IngressOutcome, IngressError> {
    store.process(request, decide_and_map).await
}

fn decide_and_map(context: DecisionContext) -> Result<DecisionOutput, IngressEffectError> {
    let pending_state_version = context
        .pending_action
        .as_ref()
        .map(|pending| pending.version);
    let account = to_conversation_context(&context)?;
    let outcome = conversation::decide(&account, &context.user_text, context.now);

    let effects = outcome
        .commands
        .into_iter()
        .map(|command| map_command(command, pending_state_version))
        .collect::<Result<Vec<_>, _>>()?;
    let reply = match outcome.replies.as_slice() {
        [] => None,
        [reply] => Some(ReplyIntent {
            body: reply.body.clone(),
        }),
        _ => return Err(IngressEffectError::InvalidTransition),
    };

    Ok(DecisionOutput { effects, reply })
}

fn to_conversation_context(
    context: &DecisionContext,
) -> Result<AccountContext, IngressEffectError> {
    let lifecycle = match context.lifecycle_state {
        Some(LifecycleState::Active) => ConversationLifecycle::Active,
        Some(LifecycleState::Suspended | LifecycleState::Deleting | LifecycleState::Deleted) => {
            ConversationLifecycle::Suspended
        }
        Some(LifecycleState::PendingConsent) | None => ConversationLifecycle::PendingConsent,
    };

    let pending = context
        .pending_action
        .as_ref()
        .and_then(|pending| pending.expense.as_ref().map(|expense| (pending, expense)))
        .map(|(pending, expense)| PendingConfirmation {
            expense_id: expense.id,
            optimistic_version: expense.version as u64,
            expires_at: pending.expires_at,
            draft: ManualDraftView {
                version: expense.version as u64,
                amount_minor: expense.amount_minor,
                currency: expense.currency.clone(),
                merchant: expense.description.clone(),
                category_display: "Khác".to_string(),
                type_label: "Chi tiêu".to_string(),
                date_display: conversation::format_date_vn(expense.occurred_at, &context.timezone),
            },
        });

    let recent_lines = context
        .recent_expenses
        .iter()
        .filter(|expense| expense.state == ExpenseState::Confirmed)
        .map(|expense| RecentExpenseLine {
            date_display: conversation::format_date_vn(expense.occurred_at, &context.timezone),
            amount_minor: expense.amount_minor,
            currency: expense.currency.clone(),
            merchant: expense.description.clone(),
            category_display: "Khác".to_string(),
            type_label: None,
        })
        .collect();

    Ok(AccountContext {
        next_expense_id: context.next_expense_id,
        lifecycle,
        allowlisted: context.sender_allowed,
        default_currency: "VND".to_string(),
        timezone: context.timezone.clone(),
        original_receipt_retention_days: context.original_receipt_retention_days,
        pending,
        today_summary: Some(PeriodSummary {
            label: "Hôm nay".to_string(),
            currency: context.today_currency.clone(),
            total_minor: context.confirmed_today_total_minor,
            tx_count: context.confirmed_today_count,
        }),
        recent_lines,
    })
}

fn map_command(
    command: DomainCommand,
    pending_state_version: Option<i32>,
) -> Result<IngressEffect, IngressEffectError> {
    match command {
        DomainCommand::GrantConsent { consent_version } => {
            Ok(IngressEffect::GrantConsent { consent_version })
        }
        DomainCommand::CreateManualAwaitingConfirmation {
            expense_id,
            amount_minor,
            currency,
            description,
            occurred_at,
            optimistic_version,
            pending_expires_at,
            ..
        } => Ok(IngressEffect::CreateManualExpenseAwaitingConfirmation {
            expense_id,
            amount_minor,
            currency,
            description,
            occurred_at,
            optimistic_version: i32::try_from(optimistic_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
            pending_expires_at,
            pending_action_type: "manual_expense_confirmation".to_string(),
        }),
        DomainCommand::ConfirmExpense {
            expense_id,
            expected_version,
        } => Ok(IngressEffect::ConfirmExpense {
            expense_id,
            expected_version: i32::try_from(expected_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
        }),
        DomainCommand::RejectExpense {
            expense_id,
            expected_version,
        } => Ok(IngressEffect::RejectExpense {
            expense_id,
            expected_version: i32::try_from(expected_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
        }),
        DomainCommand::ClearPending => Ok(IngressEffect::ClearPendingAction {
            expected_version: pending_state_version.ok_or(IngressEffectError::NotFound)?,
        }),
    }
}
