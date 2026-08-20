use super::categories::{category_display, format_category_list};
use super::locale::Locale;
use super::money::format_minor;
use super::parse::{IntentKind, is_explicit_slash_command, parse_intent};
use super::templates::{
    categories_list_text, confirmed_text, consent_card_text, daily_receipt_quota_text,
    default_category_display, default_type_label, delete_accepted_text, delete_cancelled_text,
    delete_confirm_text, discarded_text, empty_summary_text, export_accepted_text, help_text,
    image_received_text, invalid_category_text, invalid_edit_text, invalid_language_text,
    invalid_settings_text, invalid_timezone_text, language_updated_text, manual_confirmation_card,
    no_expense_to_recategorize_text, not_allowed_text, pending_expired_text, period_label_month,
    period_label_today, period_label_week, privacy_text, recategorized_text, recent_text,
    schedule_disabled_text, schedule_invalid_text, schedule_set_text, schedule_text,
    settings_label_timezone, settings_text, settings_updated_text, suspended_text,
    today_summary_text, transaction_type_label, unknown_text, welcome_text,
};
use super::types::{
    AccountContext, CONSENT_VERSION, ConversationOutcome, DomainCommand, LifecycleState,
    PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation, PendingKind, ReplyPlan,
};
use crate::schedule::{parse_timezone, validate_delivery_minute};
use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

fn locale(ctx: &AccountContext) -> Locale {
    Locale::parse(&ctx.locale)
}

/// Pure conversation decision: account context + normalized text + clock.
pub fn decide(ctx: &AccountContext, text: &str, now: DateTime<Utc>) -> ConversationOutcome {
    let loc = locale(ctx);
    if !ctx.allowlisted {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(not_allowed_text(loc))],
            commands: vec![],
        };
    }

    match ctx.lifecycle {
        LifecycleState::Suspended => ConversationOutcome {
            replies: vec![ReplyPlan::single(suspended_text(loc))],
            commands: vec![],
        },
        LifecycleState::PendingConsent => handle_pending_consent(ctx, text),
        LifecycleState::Active => handle_active(ctx, text, now),
    }
}

/// Pure image-event decision with the same account gates as text commands.
pub fn decide_image(ctx: &AccountContext, _now: DateTime<Utc>) -> ConversationOutcome {
    let loc = locale(ctx);
    if !ctx.allowlisted {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(not_allowed_text(loc))],
            commands: vec![],
        };
    }

    match ctx.lifecycle {
        LifecycleState::Suspended => ConversationOutcome {
            replies: vec![ReplyPlan::single(suspended_text(loc))],
            commands: vec![],
        },
        LifecycleState::PendingConsent => ConversationOutcome {
            replies: vec![ReplyPlan::single(consent_card_text(loc))],
            commands: vec![],
        },
        LifecycleState::Active => {
            if ctx.remaining_daily_receipts <= 0 {
                return ConversationOutcome {
                    replies: vec![ReplyPlan::single(daily_receipt_quota_text(loc))],
                    commands: vec![],
                };
            }
            ConversationOutcome {
                replies: vec![ReplyPlan::single(image_received_text(loc))],
                commands: vec![DomainCommand::AcceptReceiptSubmission {
                    submission_id: ctx.next_submission_id,
                    ingest_job_id: ctx.next_ingest_job_id,
                }],
            }
        }
    }
}

fn handle_pending_consent(ctx: &AccountContext, text: &str) -> ConversationOutcome {
    let loc = locale(ctx);
    let intent = parse_intent(text, "VND");
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(welcome_text(loc))],
            commands: vec![DomainCommand::GrantConsent {
                consent_version: CONSENT_VERSION.to_string(),
            }],
        },
        IntentKind::Privacy => reply_only(privacy_text(loc, ctx.original_receipt_retention_days)),
        IntentKind::SetLanguage => handle_set_language(ctx, &intent),
        _ => ConversationOutcome {
            replies: vec![ReplyPlan::single(consent_card_text(loc))],
            commands: vec![],
        },
    }
}

fn handle_active(ctx: &AccountContext, text: &str, now: DateTime<Utc>) -> ConversationOutcome {
    let loc = locale(ctx);
    let intent = parse_intent(text, &ctx.default_currency);

    if let Some(pending) = &ctx.pending
        && !(text.trim_start().starts_with('/') && is_explicit_slash_command(intent.kind))
    {
        if is_pending_stale(pending, now) {
            return expired_outcome(loc, true);
        }
        return resolve_pending(ctx, pending, &intent, now);
    }

    match intent.kind {
        IntentKind::Start | IntentKind::Help => reply_only(help_text(loc)),
        IntentKind::Privacy => reply_only(privacy_text(loc, ctx.original_receipt_retention_days)),
        IntentKind::Today => render_today(ctx),
        IntentKind::Week => render_week(ctx),
        IntentKind::Month => render_month(ctx),
        IntentKind::Recent => reply_only(recent_text(loc, &ctx.recent_lines)),
        IntentKind::Settings => handle_settings(ctx, &intent),
        IntentKind::Timezone => handle_timezone(ctx, &intent),
        IntentKind::Schedule => handle_schedule(ctx, &intent),
        IntentKind::Export => handle_export(loc),
        IntentKind::Delete => handle_delete(ctx, now),
        IntentKind::ManualEntry => create_manual(ctx, &intent, now),
        IntentKind::ListCategories => {
            reply_only(categories_list_text(loc, &format_category_list(loc)))
        }
        IntentKind::SetLanguage => handle_set_language(ctx, &intent),
        IntentKind::RecategorizeLatest => handle_recategorize(ctx, &intent),
        IntentKind::Confirm | IntentKind::Discard => expired_outcome(loc, false),
        IntentKind::EditAmount
        | IntentKind::EditMerchant
        | IntentKind::EditCategory
        | IntentKind::EditDate
        | IntentKind::EditType => expired_outcome(loc, false),
        IntentKind::None => reply_only(unknown_text(loc)),
    }
}

fn handle_set_language(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    let current = locale(ctx);
    if intent.locale.is_empty() {
        return reply_only(invalid_language_text(current));
    }
    let next = Locale::parse(&intent.locale);
    ConversationOutcome {
        replies: vec![ReplyPlan::single(language_updated_text(next))],
        commands: vec![DomainCommand::SetLocale {
            locale: next.as_account_value().to_string(),
        }],
    }
}

fn handle_recategorize(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    let loc = locale(ctx);
    if intent.category_key.is_empty() {
        return reply_only(invalid_category_text(loc, &format_category_list(loc)));
    }
    let display = category_display(loc, &intent.category_key);
    // Merchant filled by effect reply override when known; decide uses placeholder.
    let merchant = ctx
        .recent_lines
        .first()
        .map(|line| line.merchant.clone())
        .unwrap_or_default();
    if merchant.is_empty() && ctx.recent_lines.is_empty() {
        return reply_only(no_expense_to_recategorize_text(loc));
    }
    ConversationOutcome {
        replies: vec![ReplyPlan::single(recategorized_text(
            loc,
            if merchant.is_empty() {
                "—"
            } else {
                &merchant
            },
            &display,
        ))],
        commands: vec![DomainCommand::RecategorizeLatest {
            category_key: intent.category_key.clone(),
        }],
    }
}

fn resolve_pending(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
) -> ConversationOutcome {
    let loc = locale(ctx);
    if pending.optimistic_version != pending.draft.version {
        return expired_outcome(loc, true);
    }

    match pending.kind {
        PendingKind::AccountDeletion => resolve_deletion_pending(ctx, pending, intent),
        PendingKind::ManualExpense => resolve_manual_pending(ctx, pending, intent, now),
        PendingKind::ReceiptReview => resolve_receipt_pending(ctx, pending, intent, now),
    }
}

fn resolve_deletion_pending(
    ctx: &AccountContext,
    _pending: &PendingConfirmation,
    intent: &super::parse::Intent,
) -> ConversationOutcome {
    let loc = locale(ctx);
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(delete_accepted_text(
                loc,
                ctx.confirmed_expense_count,
            ))],
            commands: vec![DomainCommand::ConfirmAccountDeletion],
        },
        IntentKind::Discard => ConversationOutcome {
            replies: vec![ReplyPlan::single(delete_cancelled_text(loc))],
            commands: vec![DomainCommand::ClearPending],
        },
        _ => ConversationOutcome {
            replies: vec![ReplyPlan::single(delete_confirm_text(
                loc,
                ctx.confirmed_expense_count,
            ))],
            commands: vec![],
        },
    }
}

fn handle_export(loc: Locale) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(export_accepted_text(loc))],
        commands: vec![DomainCommand::RequestAccountExport],
    }
}

fn handle_delete(ctx: &AccountContext, now: DateTime<Utc>) -> ConversationOutcome {
    let loc = locale(ctx);
    ConversationOutcome {
        replies: vec![ReplyPlan::single(delete_confirm_text(
            loc,
            ctx.confirmed_expense_count,
        ))],
        commands: vec![DomainCommand::RequestAccountDeletion {
            pending_expires_at: now + chrono::Duration::seconds(PENDING_CONFIRMATION_TTL_SECS),
        }],
    }
}

fn resolve_manual_pending(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
) -> ConversationOutcome {
    let loc = locale(ctx);
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(confirmed_text(
                loc,
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
            replies: vec![ReplyPlan::single(discarded_text(loc))],
            commands: vec![
                DomainCommand::RejectExpense {
                    expense_id: pending.reference_id,
                    expected_version: pending.optimistic_version,
                },
                DomainCommand::ClearPending,
            ],
        },
        IntentKind::EditAmount
        | IntentKind::EditMerchant
        | IntentKind::EditCategory
        | IntentKind::EditDate
        | IntentKind::EditType => apply_manual_edit(ctx, pending, intent, now),
        IntentKind::None | IntentKind::ManualEntry => reshow_card(loc, pending),
        _ => reshow_card(loc, pending),
    }
}

fn resolve_receipt_pending(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
) -> ConversationOutcome {
    let loc = locale(ctx);
    match intent.kind {
        IntentKind::Confirm => ConversationOutcome {
            replies: vec![ReplyPlan::single(confirmed_text(
                loc,
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
            replies: vec![ReplyPlan::single(discarded_text(loc))],
            commands: vec![
                DomainCommand::RejectReceipt {
                    submission_id: pending.reference_id,
                    expected_draft_version: pending.optimistic_version,
                },
                DomainCommand::ClearPending,
            ],
        },
        IntentKind::EditAmount
        | IntentKind::EditMerchant
        | IntentKind::EditCategory
        | IntentKind::EditDate
        | IntentKind::EditType => apply_receipt_edit(ctx, pending, intent, now),
        IntentKind::None | IntentKind::ManualEntry => reshow_card(loc, pending),
        _ => reshow_card(loc, pending),
    }
}

fn apply_receipt_edit(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
) -> ConversationOutcome {
    let loc = locale(ctx);
    let Some(patch) = draft_patch_from_intent(loc, pending, intent, now, &ctx.timezone) else {
        return reply_only(invalid_edit_or_category(loc, intent));
    };
    ConversationOutcome {
        replies: vec![ReplyPlan::single(manual_confirmation_card(
            loc,
            &patch.merchant,
            &format_minor(patch.amount_minor, &pending.draft.currency),
            &patch.date_display,
            &patch.type_label,
            &patch.category_display,
        ))],
        commands: vec![DomainCommand::EditReceiptDraft {
            submission_id: pending.reference_id,
            expected_draft_version: pending.optimistic_version,
            amount_minor: patch.amount_minor_opt,
            merchant: patch.merchant_opt,
            category_key: patch.category_key_opt,
            occurred_at: patch.occurred_at_opt,
            transaction_type: patch.transaction_type_opt,
        }],
    }
}

fn apply_manual_edit(
    ctx: &AccountContext,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
) -> ConversationOutcome {
    let loc = locale(ctx);
    let Some(patch) = draft_patch_from_intent(loc, pending, intent, now, &ctx.timezone) else {
        return reply_only(invalid_edit_or_category(loc, intent));
    };
    ConversationOutcome {
        replies: vec![ReplyPlan::single(manual_confirmation_card(
            loc,
            &patch.merchant,
            &format_minor(patch.amount_minor, &pending.draft.currency),
            &patch.date_display,
            &patch.type_label,
            &patch.category_display,
        ))],
        commands: vec![DomainCommand::EditManualExpense {
            expense_id: pending.reference_id,
            expected_version: pending.optimistic_version,
            amount_minor: patch.amount_minor_opt,
            merchant: patch.merchant_opt,
            category_key: patch.category_key_opt,
            occurred_at: patch.occurred_at_opt,
            transaction_type: patch.transaction_type_opt,
        }],
    }
}

struct DraftPatch {
    amount_minor: i64,
    amount_minor_opt: Option<i64>,
    merchant: String,
    merchant_opt: Option<String>,
    category_display: String,
    category_key_opt: Option<String>,
    type_label: String,
    transaction_type_opt: Option<String>,
    date_display: String,
    occurred_at_opt: Option<DateTime<Utc>>,
}

fn draft_patch_from_intent(
    loc: Locale,
    pending: &PendingConfirmation,
    intent: &super::parse::Intent,
    now: DateTime<Utc>,
    timezone: &str,
) -> Option<DraftPatch> {
    let mut patch = DraftPatch {
        amount_minor: pending.draft.amount_minor,
        amount_minor_opt: None,
        merchant: pending.draft.merchant.clone(),
        merchant_opt: None,
        category_display: pending.draft.category_display.clone(),
        category_key_opt: None,
        type_label: pending.draft.type_label.clone(),
        transaction_type_opt: None,
        date_display: pending.draft.date_display.clone(),
        occurred_at_opt: None,
    };

    match intent.kind {
        IntentKind::EditAmount => {
            patch.amount_minor = intent.amount_minor;
            patch.amount_minor_opt = Some(intent.amount_minor);
        }
        IntentKind::EditMerchant => {
            if intent.merchant.trim().is_empty() {
                return None;
            }
            patch.merchant = intent.merchant.clone();
            patch.merchant_opt = Some(intent.merchant.clone());
        }
        IntentKind::EditCategory => {
            if intent.category_key.is_empty() {
                return None;
            }
            patch.category_display = category_display(loc, &intent.category_key);
            patch.category_key_opt = Some(intent.category_key.clone());
        }
        IntentKind::EditDate => {
            let date = intent.occurred_on?;
            let occurred_at = local_date_to_utc(date, timezone, now)?;
            patch.date_display = format_date_vn(occurred_at, timezone);
            patch.occurred_at_opt = Some(occurred_at);
        }
        IntentKind::EditType => {
            if intent.transaction_type.is_empty() {
                return None;
            }
            patch.type_label = transaction_type_label(loc, &intent.transaction_type).to_string();
            patch.transaction_type_opt = Some(intent.transaction_type.clone());
        }
        _ => return None,
    }
    Some(patch)
}

fn invalid_edit_or_category(loc: Locale, intent: &super::parse::Intent) -> String {
    if intent.kind == IntentKind::EditCategory {
        invalid_category_text(loc, &format_category_list(loc))
    } else {
        invalid_edit_text(loc)
    }
}

fn local_date_to_utc(
    date: chrono::NaiveDate,
    timezone: &str,
    fallback_now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let noon = NaiveTime::from_hms_opt(12, 0, 0)?;
    let local = date.and_time(noon);
    tz.from_local_datetime(&local)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            // Ambiguous/gap: keep clock of fallback day-shift.
            Some(
                fallback_now
                    .with_timezone(&tz)
                    .date_naive()
                    .and_time(noon)
                    .and_local_timezone(tz)
                    .single()?
                    .with_timezone(&Utc),
            )
        })
}

fn reshow_card(loc: Locale, pending: &PendingConfirmation) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(manual_confirmation_card(
            loc,
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
    let loc = locale(ctx);
    let expense_id = ctx.next_expense_id;
    let optimistic_version = 1;
    let expires_at = now + chrono::Duration::seconds(PENDING_CONFIRMATION_TTL_SECS);
    let merchant = intent.description.clone();
    let date_display = format_date_vn(now, &ctx.timezone);
    let amount_display = format_minor(intent.amount_minor, &intent.currency);
    let category = default_category_display(loc);
    let type_label = default_type_label(loc);

    ConversationOutcome {
        replies: vec![ReplyPlan::single(manual_confirmation_card(
            loc,
            &merchant,
            &amount_display,
            &date_display,
            type_label,
            &category,
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
    let loc = locale(ctx);
    with_insight_snapshot(
        render_period_summary(loc, ctx.today_summary.as_ref(), period_label_today(loc)),
        "day",
    )
}

fn render_week(ctx: &AccountContext) -> ConversationOutcome {
    let loc = locale(ctx);
    with_insight_snapshot(
        render_period_summary(loc, ctx.week_summary.as_ref(), period_label_week(loc)),
        "week",
    )
}

fn render_month(ctx: &AccountContext) -> ConversationOutcome {
    let loc = locale(ctx);
    with_insight_snapshot(
        render_period_summary(loc, ctx.month_summary.as_ref(), period_label_month(loc)),
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
    loc: Locale,
    summary: Option<&super::types::PeriodSummary>,
    fallback_label: &str,
) -> ConversationOutcome {
    let body = match summary {
        Some(s) if s.tx_count == 0 => empty_summary_text(loc, &s.label),
        Some(s) => today_summary_text(loc, &s.label, &s.currency, s.total_minor),
        None => empty_summary_text(loc, fallback_label),
    };
    reply_only(body)
}

fn handle_settings(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    let loc = locale(ctx);
    if intent.timezone.is_empty() {
        return reply_only(settings_text(
            loc,
            &ctx.timezone,
            &ctx.default_currency,
            &ctx.schedules,
        ));
    }
    reply_only(invalid_settings_text(loc))
}

fn handle_timezone(ctx: &AccountContext, intent: &super::parse::Intent) -> ConversationOutcome {
    let loc = locale(ctx);
    if intent.timezone.is_empty() || parse_timezone(&intent.timezone).is_err() {
        return reply_only(invalid_timezone_text(loc));
    }
    ConversationOutcome {
        replies: vec![ReplyPlan::single(settings_updated_text(
            loc,
            settings_label_timezone(loc),
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
    let loc = locale(ctx);
    if intent.disable_all_schedules {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(schedule_disabled_text(loc, None))],
            commands: vec![DomainCommand::DisableSchedule { frequency: None }],
        };
    }
    if intent.disable_schedule {
        return ConversationOutcome {
            replies: vec![ReplyPlan::single(schedule_disabled_text(
                loc,
                Some(&intent.schedule_frequency),
            ))],
            commands: vec![DomainCommand::DisableSchedule {
                frequency: Some(intent.schedule_frequency.clone()),
            }],
        };
    }
    if intent.schedule_frequency.is_empty() {
        return reply_only(schedule_text(loc, &ctx.schedules));
    }
    if validate_delivery_minute(intent.delivery_minute).is_err() {
        return reply_only(schedule_invalid_text(loc));
    }
    ConversationOutcome {
        replies: vec![ReplyPlan::single(schedule_set_text(
            loc,
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

fn expired_outcome(loc: Locale, clear_pending: bool) -> ConversationOutcome {
    ConversationOutcome {
        replies: vec![ReplyPlan::single(pending_expired_text(loc))],
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
