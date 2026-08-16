use super::money::format_minor;
use super::parse::{IntentKind, is_explicit_slash_command, parse_intent};
use super::templates::{
    confirmed_text, consent_card_text, daily_receipt_quota_text, default_category_display,
    default_type_label, delete_accepted_text, delete_cancelled_text, delete_confirm_text,
    discarded_text, empty_summary_text, export_accepted_text, help_text, image_received_text,
    invalid_settings_text, invalid_timezone_text, manual_confirmation_card, not_allowed_text,
    pending_expired_text, privacy_text, recent_text, schedule_disabled_text, schedule_invalid_text,
    schedule_set_text, schedule_text, settings_text, settings_updated_text, suspended_text,
    today_summary_text, unknown_text, welcome_text,
};
use super::types::{
    AccountContext, CONSENT_VERSION, ConversationOutcome, DomainCommand, LifecycleState,
    PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation, PendingKind, ReplyPlan,
};
use crate::schedule::{parse_timezone, validate_delivery_minute};
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
        LifecycleState::Active => {
            if ctx.remaining_daily_receipts <= 0 {
                return ConversationOutcome {
                    replies: vec![ReplyPlan::single(daily_receipt_quota_text())],
                    commands: vec![],
                };
            }
            ConversationOutcome {
                replies: vec![ReplyPlan::single(image_received_text())],
                commands: vec![DomainCommand::AcceptReceiptSubmission {
                    submission_id: ctx.next_submission_id,
                    ingest_job_id: ctx.next_ingest_job_id,
                }],
            }
        }
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
        IntentKind::Week => render_week(ctx),
        IntentKind::Month => render_month(ctx),
        IntentKind::Recent => reply_only(recent_text(&ctx.recent_lines)),
        IntentKind::Settings => handle_settings(ctx, &intent),
        IntentKind::Timezone => handle_timezone(ctx, &intent),
        IntentKind::Schedule => handle_schedule(ctx, &intent),
        IntentKind::Export => handle_export(),
        IntentKind::Delete => handle_delete(ctx, now),
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
        PendingKind::AccountDeletion => resolve_deletion_pending(ctx, pending, intent),
        PendingKind::ManualExpense => resolve_manual_pending(pending, intent),
        PendingKind::ReceiptReview => resolve_receipt_pending(ctx, pending, intent),
    }
}

fn resolve_deletion_pending(
    ctx: &AccountContext,
    _pending: &PendingConfirmation,
    intent: &super::parse::Intent,
) -> ConversationOutcome {
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(delete_accepted_text(
                ctx.confirmed_expense_count,
            ))],
            commands: vec![DomainCommand::ConfirmAccountDeletion],
        },
        IntentKind::Discard => ConversationOutcome {
            replies: vec![ReplyPlan::single(delete_cancelled_text())],
            commands: vec![DomainCommand::ClearPending],
        },
        _ => ConversationOutcome {
            replies: vec![ReplyPlan::single(delete_confirm_text(
                ctx.confirmed_expense_count,
            ))],
            commands: vec![],
        },
    }
}

fn handle_export() -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(export_accepted_text())],
        commands: vec![DomainCommand::RequestAccountExport],
    }
}

fn handle_delete(ctx: &AccountContext, now: DateTime<Utc>) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(delete_confirm_text(
            ctx.confirmed_expense_count,
        ))],
        commands: vec![DomainCommand::RequestAccountDeletion {
            pending_expires_at: now + chrono::Duration::seconds(PENDING_CONFIRMATION_TTL_SECS),
        }],
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
    with_insight_snapshot(
        render_period_summary(ctx.today_summary.as_ref(), "Hôm nay"),
        "day",
    )
}

fn render_week(ctx: &AccountContext) -> ConversationOutcome {
    with_insight_snapshot(
        render_period_summary(ctx.week_summary.as_ref(), "Tuần này"),
        "week",
    )
}

fn render_month(ctx: &AccountContext) -> ConversationOutcome {
    with_insight_snapshot(
        render_period_summary(ctx.month_summary.as_ref(), "Tháng này"),
        "month",
    )
}

fn with_insight_snapshot(
    mut outcome: ConversationOutcome,
    period_kind: &str,
) -> ConversationOutcome {
    outcome.commands.push(DomainCommand::RecordInsightSnapshot {
        period_kind: period_kind.to_string(),
    });
    outcome
}

fn render_period_summary(
    summary: Option<&super::types::PeriodSummary>,
    fallback_label: &str,
) -> ConversationOutcome {
    let body = match summary {
        Some(s) if s.tx_count == 0 => empty_summary_text(&s.label),
        Some(s) => today_summary_text(&s.label, &s.currency, s.total_minor),
        None => empty_summary_text(fallback_label),
    };
    reply_only(body)
}

fn handle_settings(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    if intent.timezone.is_empty() {
        return reply_only(settings_text(
            &ctx.timezone,
            &ctx.default_currency,
            &ctx.schedules,
        ));
    }
    reply_only(invalid_settings_text())
}

fn handle_timezone(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    if intent.timezone.is_empty() || parse_timezone(&intent.timezone).is_err() {
        return reply_only(invalid_timezone_text());
    }
    ConversationOutcome {
        replies: vec![ReplyPlan::single(settings_updated_text(
            "múi giờ",
            &intent.timezone,
            &intent.timezone,
            &ctx.default_currency,
            &ctx.schedules,
        ))],
        commands: vec![DomainCommand::SetTimezone {
            iana: intent.timezone.clone(),
        }],
    }
}

fn handle_schedule(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    if intent.disable_all_schedules {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(schedule_disabled_text(None))],
            commands: vec![DomainCommand::DisableSchedule { frequency: None }],
        };
    }
    if intent.disable_schedule {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(schedule_disabled_text(Some(
                &intent.schedule_frequency,
            )))],
            commands: vec![DomainCommand::DisableSchedule {
                frequency: Some(intent.schedule_frequency.clone()),
            }],
        };
    }
    if intent.schedule_frequency.is_empty() {
        return reply_only(schedule_text(&ctx.schedules));
    }
    if validate_delivery_minute(intent.delivery_minute).is_err() {
        return reply_only(schedule_invalid_text());
    }
    ConversationOutcome {
        replies: vec![ReplyPlan::single(schedule_set_text(
            &intent.schedule_frequency,
            intent.delivery_minute,
        ))],
        commands: vec![DomainCommand::UpsertSchedule {
            frequency: intent.schedule_frequency.clone(),
            delivery_minute: intent.delivery_minute,
        }],
    }
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
