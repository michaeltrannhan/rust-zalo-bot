use super::money::format_minor;
use super::parse::{IntentKind, is_explicit_slash_command, parse_intent};
use super::templates::{
    confirmed_text, consent_card_text, default_category_display, default_type_label,
    discarded_text, empty_summary_text, help_text, image_received_text, manual_confirmation_card,
    not_allowed_text, pending_expired_text, privacy_text, recent_text, suspended_text,
    today_summary_text, unknown_text, welcome_text,
};
use super::types::{
    AccountContext, CONSENT_VERSION, ConversationOutcome, DomainCommand, LifecycleState,
    PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation, PendingKind, ReplyPlan,
};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// Pure conversation decision: account context + normalized text + clock.
pub fn decide(ctx: &AccountContext, text: &str, now: DateTime<Utc>) -> ConversationOutcome {
    if !ctx.allowlisted {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(not_allowed_text())],
            commands: vec![],
        };
    }

    match ctx.lifecycle {
        LifecycleState::Suspended => ConversationOutcome {
            replies: vec![ReplyPlan::single(suspended_text())],
            commands: vec![],
        },
        LifecycleState::PendingConsent => handle_pending_consent(ctx, text),
        LifecycleState::Active => handle_active(ctx, text, now),
    }
}

/// Pure image-event decision with the same account gates as text commands.
pub fn decide_image(ctx: &AccountContext, _now: DateTime<Utc>) -> ConversationOutcome {
    if !ctx.allowlisted {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(not_allowed_text())],
            commands: vec![],
        };
    }

    match ctx.lifecycle {
        LifecycleState::Suspended => ConversationOutcome {
            replies: vec![ReplyPlan::single(suspended_text())],
            commands: vec![],
        },
        LifecycleState::PendingConsent => ConversationOutcome {
            replies: vec![ReplyPlan::single(consent_card_text())],
            commands: vec![],
        },
        LifecycleState::Active => ConversationOutcome {
            replies: vec![ReplyPlan::single(image_received_text())],
            commands: vec![DomainCommand::AcceptReceiptSubmission {
                submission_id: ctx.next_submission_id,
                ingest_job_id: ctx.next_ingest_job_id,
            }],
        },
    }
}

fn handle_pending_consent(ctx: &AccountContext, text: &str) -> ConversationOutcome {
    let intent = parse_intent(text, "VND");
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(welcome_text())],
            commands: vec![DomainCommand::GrantConsent {
                consent_version: CONSENT_VERSION.to_string(),
            }],
        },
        IntentKind::Privacy => reply_only(privacy_text(ctx.original_receipt_retention_days)),
        _ => ConversationOutcome {
            replies: vec![ReplyPlan::single(consent_card_text())],
            commands: vec![],
        },
    }
}

fn handle_active(ctx: &AccountContext, text: &str, now: DateTime<Utc>) -> ConversationOutcome {
    let intent = parse_intent(text, &ctx.default_currency);

    if let Some(pending) = &ctx.pending
        && !(text.trim_start().starts_with('/') && is_explicit_slash_command(intent.kind))
    {
        if is_pending_stale(pending, now) {
            return expired_outcome(true);
        }
        return resolve_pending(ctx, pending, &intent);
    }

    match intent.kind {
        IntentKind::Start | IntentKind::Help => reply_only(help_text()),
        IntentKind::Privacy => reply_only(privacy_text(ctx.original_receipt_retention_days)),
        IntentKind::Today => render_today(ctx),
        IntentKind::Recent => reply_only(recent_text(&ctx.recent_lines)),
        IntentKind::ManualEntry => create_manual(ctx, &intent, now),
        IntentKind::Confirm | IntentKind::Discard => expired_outcome(false),
        IntentKind::EditAmount => expired_outcome(false),
        IntentKind::None => reply_only(unknown_text()),
    }
}

fn resolve_pending(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
) -> ConversationOutcome {
    if pending.optimistic_version != pending.draft.version {
        return expired_outcome(true);
    }

    match pending.kind {
        PendingKind::ManualExpense => resolve_manual_pending(pending, intent),
        PendingKind::ReceiptReview => resolve_receipt_pending(ctx, pending, intent),
    }
}

fn resolve_manual_pending(
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
) -> ConversationOutcome {
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(confirmed_text(
                &format_minor(pending.draft.amount_minor, &pending.draft.currency),
                &pending.draft.merchant,
                &pending.draft.category_display,
            ))],
            commands: vec![
                DomainCommand::ConfirmExpense {
                    expense_id: pending.reference_id,
                    expected_version: pending.optimistic_version,
                },
                DomainCommand::ClearPending,
            ],
        },
        IntentKind::Discard => ConversationOutcome {
            replies: vec![ReplyPlan::single(discarded_text())],
            commands: vec![
                DomainCommand::RejectExpense {
                    expense_id: pending.reference_id,
                    expected_version: pending.optimistic_version,
                },
                DomainCommand::ClearPending,
            ],
        },
        IntentKind::None | IntentKind::ManualEntry => reshow_manual_card(pending),
        _ => reshow_manual_card(pending),
    }
}

fn resolve_receipt_pending(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
) -> ConversationOutcome {
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(confirmed_text(
                &format_minor(pending.draft.amount_minor, &pending.draft.currency),
                &pending.draft.merchant,
                &pending.draft.category_display,
            ))],
            commands: vec![
                DomainCommand::ConfirmReceipt {
                    submission_id: pending.reference_id,
                    expense_id: ctx.next_expense_id,
                    expected_draft_version: pending.optimistic_version,
                },
                DomainCommand::ClearPending,
            ],
        },
        IntentKind::Discard => ConversationOutcome {
            replies: vec![ReplyPlan::single(discarded_text())],
            commands: vec![
                DomainCommand::RejectReceipt {
                    submission_id: pending.reference_id,
                    expected_draft_version: pending.optimistic_version,
                },
                DomainCommand::ClearPending,
            ],
        },
        IntentKind::EditAmount => ConversationOutcome {
            replies: vec![ReplyPlan::single(manual_confirmation_card(
                &pending.draft.merchant,
                &format_minor(intent.amount_minor, &intent.currency),
                &pending.draft.date_display,
                &pending.draft.type_label,
                &pending.draft.category_display,
            ))],
            commands: vec![DomainCommand::EditReceiptAmount {
                submission_id: pending.reference_id,
                expected_draft_version: pending.optimistic_version,
                amount_minor: intent.amount_minor,
            }],
        },
        IntentKind::None | IntentKind::ManualEntry => reshow_manual_card(pending),
        _ => reshow_manual_card(pending),
    }
}

fn reshow_manual_card(pending: &PendingConfirmation) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(manual_confirmation_card(
            &pending.draft.merchant,
            &format_minor(pending.draft.amount_minor, &pending.draft.currency),
            &pending.draft.date_display,
            &pending.draft.type_label,
            &pending.draft.category_display,
        ))],
        commands: vec![],
    }
}

fn create_manual(
    ctx: &AccountContext,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
) -> ConversationOutcome {
    let expense_id = ctx.next_expense_id;
    let optimistic_version = 1;
    let expires_at = now + chrono::Duration::seconds(PENDING_CONFIRMATION_TTL_SECS);
    let merchant = intent.description.clone();
    let date_display = format_date_vn(now, &ctx.timezone);
    let amount_display = format_minor(intent.amount_minor, &intent.currency);
    let category = default_category_display();
    let type_label = default_type_label();

    ConversationOutcome {
        replies: vec![ReplyPlan::single(manual_confirmation_card(
            &merchant,
            &amount_display,
            &date_display,
            type_label,
            category,
        ))],
        commands: vec![DomainCommand::CreateManualAwaitingConfirmation {
            expense_id,
            amount_minor: intent.amount_minor,
            currency: intent.currency.clone(),
            description: intent.description.clone(),
            merchant,
            occurred_at: now,
            optimistic_version,
            pending_expires_at: expires_at,
        }],
    }
}

fn render_today(ctx: &AccountContext) -> ConversationOutcome {
    let summary = ctx.today_summary.as_ref();
    let body = match summary {
        Some(s) if s.tx_count == 0 => empty_summary_text(&s.label),
        Some(s) => today_summary_text(&s.label, &s.currency, s.total_minor),
        None => empty_summary_text("Hôm nay"),
    };
    reply_only(body)
}

fn reply_only(body: String) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(body)],
        commands: vec![],
    }
}

fn expired_outcome(clear_pending: bool) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(pending_expired_text())],
        commands: if clear_pending {
            vec![DomainCommand::ClearPending]
        } else {
            vec![]
        },
    }
}

fn is_pending_stale(pending: &PendingConfirmation, now: DateTime<Utc>) -> bool {
    now >= pending.expires_at || pending.optimistic_version != pending.draft.version
}

pub fn format_date_vn(now: DateTime<Utc>, timezone: &str) -> String {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    now.with_timezone(&tz).format("%d/%m/%Y").to_string()
}
