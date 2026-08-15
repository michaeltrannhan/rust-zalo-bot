//! Application-level bridge between persistence and the pure conversation seam.

use uuid::Uuid;

use crate::conversation::{
    self, AccountContext, DomainCommand, LifecycleState as ConversationLifecycle, ManualDraftView,
    PendingConfirmation, PendingKind, PeriodSummary, RecentExpenseLine, transaction_type_label,
};
use crate::receipt::ReceiptLifecycle;

use super::effects::{IngressEffect, IngressEffectError};
use super::store::{IngressError, IngressStore};
use super::types::{
    DecisionContext, DecisionOutput, ExpenseState, IngressEventKind, IngressObservation,
    IngressOutcome, IngressRequest, LifecycleState, ReplyIntent,
};

/// Process one normalized text event through the pure conversation seam and
/// commit its state transition plus reply intent in the ingress transaction.
pub async fn process_text_command(
    store: &IngressStore,
    request: IngressRequest,
) -> Result<IngressOutcome, IngressError> {
    store
        .process(request, IngressObservation::default(), decide_text_and_map)
        .await
}

/// Process one normalized image event through the pure conversation seam and
/// commit receipt submission plus reply intent in the ingress transaction.
pub async fn process_image(
    store: &IngressStore,
    request: IngressRequest,
    media_url: String,
) -> Result<IngressOutcome, IngressError> {
    if store.receipt().is_none() {
        return Err(IngressError::new(
            "receipt lifecycle is required for images",
        ));
    }
    store
        .process(
            request,
            IngressObservation {
                event_kind: IngressEventKind::ImageReceived,
                media_url: Some(media_url),
            },
            decide_image_and_map,
        )
        .await
}

/// Build an ingress store wired for image receipt acceptance.
pub fn store_with_receipt(pool: sqlx::PgPool, receipt: ReceiptLifecycle) -> IngressStore {
    IngressStore::with_receipt(pool, receipt)
}

fn decide_text_and_map(context: DecisionContext) -> Result<DecisionOutput, IngressEffectError> {
    let pending_state_version = context
        .pending_action
        .as_ref()
        .map(|pending| pending.version);
    let account = to_conversation_context(&context)?;
    let outcome = conversation::decide(&account, &context.user_text, context.now);
    map_outcome(outcome, &context, pending_state_version)
}

fn decide_image_and_map(context: DecisionContext) -> Result<DecisionOutput, IngressEffectError> {
    let account = to_conversation_context(&context)?;
    let outcome = conversation::decide_image(&account, context.now);
    map_outcome(outcome, &context, None)
}

fn map_outcome(
    outcome: conversation::ConversationOutcome,
    context: &DecisionContext,
    pending_state_version: Option<i32>,
) -> Result<DecisionOutput, IngressEffectError> {
    let inbound_event_id = context
        .inbound_event_id
        .ok_or(IngressEffectError::InvalidTransition)?;
    let effects = outcome
        .commands
        .into_iter()
        .map(|command| map_command(command, pending_state_version, inbound_event_id))
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
        .and_then(|pending| match pending.action_type.as_str() {
            "manual_expense_confirmation" => pending.expense.as_ref().map(|expense| {
                (
                    PendingKind::ManualExpense,
                    expense.id,
                    expense.version as u64,
                    expense.amount_minor,
                    expense.currency.clone(),
                    expense.description.clone(),
                    conversation::format_date_vn(expense.occurred_at, &context.timezone),
                    "Khác".to_string(),
                    "Chi tiêu".to_string(),
                )
            }),
            "receipt_review" => pending.receipt_draft.as_ref().map(|draft| {
                (
                    PendingKind::ReceiptReview,
                    draft.submission_id,
                    draft.version as u64,
                    draft.amount_minor,
                    draft.currency.clone(),
                    draft.merchant.clone(),
                    conversation::format_date_vn(draft.occurred_at, &context.timezone),
                    draft.category_display.clone(),
                    transaction_type_label(&draft.transaction_type).to_string(),
                )
            }),
            _ => None,
        })
        .map(
            |(
                kind,
                reference_id,
                version,
                amount_minor,
                currency,
                merchant,
                date_display,
                category_display,
                type_label,
            )| PendingConfirmation {
                kind,
                reference_id,
                optimistic_version: version,
                expires_at: context
                    .pending_action
                    .as_ref()
                    .expect("pending action")
                    .expires_at,
                draft: ManualDraftView {
                    version,
                    amount_minor,
                    currency,
                    merchant,
                    category_display,
                    type_label,
                    date_display,
                },
            },
        );

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
        next_submission_id: context.next_submission_id,
        next_ingest_job_id: context.next_ingest_job_id,
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
    inbound_event_id: Uuid,
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
        DomainCommand::AcceptReceiptSubmission {
            submission_id,
            ingest_job_id,
        } => Ok(IngressEffect::AcceptReceiptSubmission {
            submission_id,
            ingest_job_id,
            inbound_event_id,
        }),
        DomainCommand::ConfirmReceipt {
            submission_id,
            expense_id,
            expected_draft_version,
        } => Ok(IngressEffect::ConfirmReceipt {
            submission_id,
            expense_id,
            expected_draft_version: i32::try_from(expected_draft_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
        }),
        DomainCommand::RejectReceipt {
            submission_id,
            expected_draft_version,
        } => Ok(IngressEffect::RejectReceipt {
            submission_id,
            expected_draft_version: i32::try_from(expected_draft_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
        }),
        DomainCommand::EditReceiptAmount {
            submission_id,
            expected_draft_version,
            amount_minor,
        } => Ok(IngressEffect::EditReceiptDraft {
            submission_id,
            expected_draft_version: i32::try_from(expected_draft_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
            amount_minor,
        }),
        DomainCommand::ClearPending => Ok(IngressEffect::ClearPendingAction {
            expected_version: pending_state_version.ok_or(IngressEffectError::NotFound)?,
        }),
    }
}
