//! Application-level bridge between persistence and the pure conversation seam.

use uuid::Uuid;

use crate::conversation::{
    self, AccountContext, DomainCommand, LifecycleState as ConversationLifecycle, Locale,
    ManualDraftView, PendingConfirmation, PendingKind, PeriodSummary, RecentExpenseLine,
    ScheduleLine, category_display, period_label_month, period_label_today, period_label_week,
    transaction_type_label,
};
use crate::receipt::ReceiptLifecycle;

use super::effects::{IngressEffect, IngressEffectError};
use super::policy::IngressPolicy;
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

/// Build an ingress store with receipt lifecycle and ingress policy.
pub fn store_with_receipt_and_policy(
    pool: sqlx::PgPool,
    receipt: ReceiptLifecycle,
    policy: IngressPolicy,
) -> IngressStore {
    IngressStore::with_receipt_and_policy(pool, receipt, policy)
}

fn decide_text_and_map(context: DecisionContext) -> Result<DecisionOutput, IngressEffectError> {
    let account = to_conversation_context(&context)?;
    let outcome = conversation::decide(&account, &context.user_text, context.now);
    map_outcome(outcome, &context)
}

fn decide_image_and_map(context: DecisionContext) -> Result<DecisionOutput, IngressEffectError> {
    let account = to_conversation_context(&context)?;
    let outcome = conversation::decide_image(&account, context.now);
    map_outcome(outcome, &context)
}

fn map_outcome(
    outcome: conversation::ConversationOutcome,
    context: &DecisionContext,
) -> Result<DecisionOutput, IngressEffectError> {
    let inbound_event_id = context
        .inbound_event_id
        .ok_or(IngressEffectError::InvalidTransition)?;
    let pending_state_version = context
        .pending_action
        .as_ref()
        .map(|pending| pending.version);

    let mut effects = Vec::with_capacity(outcome.commands.len());
    for command in outcome.commands {
        effects.push(map_command(
            command,
            pending_state_version,
            inbound_event_id,
        )?);
    }

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
    let locale = Locale::parse(context.locale.as_str());
    let lifecycle = match context.lifecycle_state {
        Some(LifecycleState::Active) => ConversationLifecycle::Active,
        Some(LifecycleState::Suspended | LifecycleState::Deleting | LifecycleState::Deleted) => {
            ConversationLifecycle::Suspended
        }
        Some(LifecycleState::PendingConsent) | None => ConversationLifecycle::PendingConsent,
    };

    let pending = match context.pending_action.as_ref() {
        Some(pending) if pending.action_type == "account_deletion" => Some(PendingConfirmation {
            kind: PendingKind::AccountDeletion,
            reference_id: context.account_id.unwrap_or(Uuid::nil()),
            optimistic_version: pending.version as u64,
            expires_at: pending.expires_at,
            draft: ManualDraftView {
                version: pending.version as u64,
                amount_minor: 0,
                currency: context.default_currency.clone(),
                merchant: String::new(),
                category_key: "khac".to_string(),
                category_display: String::new(),
                transaction_type: "expense".to_string(),
                type_label: String::new(),
                date_display: String::new(),
                occurred_at: context.now,
            },
        }),
        Some(pending) => pending
            .expense
            .as_ref()
            .map(|expense| {
                (
                    PendingKind::ManualExpense,
                    expense.id,
                    expense.version as u64,
                    expense.amount_minor,
                    expense.currency.clone(),
                    expense.description.clone(),
                    conversation::format_date_vn(expense.occurred_at, &context.timezone),
                    expense.category_key.clone(),
                    category_display(locale, &expense.category_key),
                    expense.transaction_type.clone(),
                    transaction_type_label(locale, &expense.transaction_type).to_string(),
                    expense.occurred_at,
                )
            })
            .or_else(|| {
                pending.receipt_draft.as_ref().map(|draft| {
                    (
                        PendingKind::ReceiptReview,
                        draft.submission_id,
                        draft.version as u64,
                        draft.amount_minor,
                        draft.currency.clone(),
                        draft.merchant.clone(),
                        conversation::format_date_vn(draft.occurred_at, &context.timezone),
                        draft.category_key.clone(),
                        draft.category_display.clone(),
                        draft.transaction_type.clone(),
                        transaction_type_label(locale, &draft.transaction_type).to_string(),
                        draft.occurred_at,
                    )
                })
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
                    category_key,
                    category_display,
                    transaction_type,
                    type_label,
                    occurred_at,
                )| PendingConfirmation {
                    kind,
                    reference_id,
                    optimistic_version: version,
                    expires_at: pending.expires_at,
                    draft: ManualDraftView {
                        version,
                        amount_minor,
                        currency,
                        merchant,
                        category_key,
                        category_display,
                        transaction_type,
                        type_label,
                        date_display,
                        occurred_at,
                    },
                },
            ),
        None => None,
    };

    let recent_lines = context
        .recent_expenses
        .iter()
        .filter(|expense| expense.state == ExpenseState::Confirmed)
        .map(|expense| RecentExpenseLine {
            date_display: conversation::format_date_vn(expense.occurred_at, &context.timezone),
            amount_minor: expense.amount_minor,
            currency: expense.currency.clone(),
            merchant: expense.description.clone(),
            category_display: category_display(locale, &expense.category_key),
            type_label: Some(transaction_type_label(locale, &expense.transaction_type).to_string()),
        })
        .collect();

    Ok(AccountContext {
        next_expense_id: context.next_expense_id,
        next_submission_id: context.next_submission_id,
        next_ingest_job_id: context.next_ingest_job_id,
        lifecycle,
        allowlisted: context.sender_allowed,
        default_currency: context.default_currency.clone(),
        timezone: context.timezone.clone(),
        locale: context.locale.clone(),
        original_receipt_retention_days: context.original_receipt_retention_days,
        remaining_daily_receipts: context.remaining_daily_receipts,
        confirmed_expense_count: context.confirmed_expense_count,
        pending,
        today_summary: Some(PeriodSummary {
            label: period_label_today(locale).to_string(),
            currency: context.today_currency.clone(),
            total_minor: context.confirmed_today_total_minor,
            tx_count: context.confirmed_today_count,
        }),
        week_summary: Some(PeriodSummary {
            label: period_label_week(locale).to_string(),
            currency: context.week_currency.clone(),
            total_minor: context.week_total_minor,
            tx_count: context.week_tx_count,
        }),
        month_summary: Some(PeriodSummary {
            label: period_label_month(locale).to_string(),
            currency: context.month_currency.clone(),
            total_minor: context.month_total_minor,
            tx_count: context.month_tx_count,
        }),
        schedules: context
            .schedules
            .iter()
            .map(|schedule| ScheduleLine {
                frequency: schedule.frequency.clone(),
                delivery_minute: schedule.delivery_minute,
                enabled: schedule.enabled,
            })
            .collect(),
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
        DomainCommand::EditReceiptDraft {
            submission_id,
            expected_draft_version,
            amount_minor,
            merchant,
            category_key,
            occurred_at,
            transaction_type,
        } => Ok(IngressEffect::EditReceiptDraft {
            submission_id,
            expected_draft_version: i32::try_from(expected_draft_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
            amount_minor,
            merchant,
            category_key,
            occurred_at,
            transaction_type,
        }),
        DomainCommand::EditManualExpense {
            expense_id,
            expected_version,
            amount_minor,
            merchant,
            category_key,
            occurred_at,
            transaction_type,
        } => Ok(IngressEffect::EditManualExpense {
            expense_id,
            expected_version: i32::try_from(expected_version)
                .map_err(|_| IngressEffectError::InvalidTransition)?,
            amount_minor,
            merchant,
            category_key,
            occurred_at,
            transaction_type,
        }),
        DomainCommand::RecategorizeLatest { category_key } => {
            Ok(IngressEffect::RecategorizeLatest { category_key })
        }
        DomainCommand::ClearPending => Ok(IngressEffect::ClearPendingAction {
            expected_version: pending_state_version.ok_or(IngressEffectError::NotFound)?,
        }),
        DomainCommand::SetLocale { locale } => Ok(IngressEffect::SetLocale { locale }),
        DomainCommand::SetTimezone { iana } => Ok(IngressEffect::SetTimezone { iana }),
        DomainCommand::UpsertSchedule {
            frequency,
            delivery_minute,
        } => Ok(IngressEffect::UpsertSummarySchedule {
            frequency,
            delivery_minute,
        }),
        DomainCommand::DisableSchedule { frequency } => {
            Ok(IngressEffect::DisableSummarySchedule { frequency })
        }
        DomainCommand::RequestAccountDeletion { pending_expires_at } => {
            Ok(IngressEffect::ArmAccountDeletion {
                expires_at: pending_expires_at,
            })
        }
        DomainCommand::ConfirmAccountDeletion => Ok(IngressEffect::ConfirmAccountDeletion),
        DomainCommand::RequestAccountExport => Ok(IngressEffect::RequestAccountExport),
        DomainCommand::RecordInsightSnapshot { period_kind } => {
            Ok(IngressEffect::RecordInsightSnapshot { period_kind })
        }
    }
}
