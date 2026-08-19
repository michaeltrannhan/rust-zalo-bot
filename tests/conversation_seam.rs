//! Public conversation seam integration tests.

use chrono::{Duration, TimeZone, Utc};
use uuid::Uuid;

use zl_expense::conversation::{
    AccountContext, AmountError, CONSENT_VERSION, ConversationOutcome, DomainCommand,
    LifecycleState, ManualDraftView, PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation,
    PendingKind, PeriodSummary, RecentExpenseLine, decide, format_minor, parse_amount,
};

fn active_ctx(account_id: Uuid) -> AccountContext {
    AccountContext {
        next_expense_id: account_id,
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        original_receipt_retention_days: 7,
        remaining_daily_receipts: 20,
        confirmed_expense_count: 0,
        pending: None,
        today_summary: None,
        week_summary: None,
        month_summary: None,
        schedules: vec![],
        recent_lines: vec![],
    }
}

fn pending_consent_ctx(account_id: Uuid) -> AccountContext {
    AccountContext {
        next_expense_id: account_id,
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::PendingConsent,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        original_receipt_retention_days: 7,
        remaining_daily_receipts: 20,
        confirmed_expense_count: 0,
        pending: None,
        today_summary: None,
        week_summary: None,
        month_summary: None,
        schedules: vec![],
        recent_lines: vec![],
    }
}

fn sample_pending(
    expense_id: Uuid,
    version: u64,
    now: chrono::DateTime<Utc>,
) -> PendingConfirmation {
    PendingConfirmation {
        kind: PendingKind::ManualExpense,
        reference_id: expense_id,
        optimistic_version: version,
        expires_at: now + Duration::seconds(PENDING_CONFIRMATION_TTL_SECS),
        draft: ManualDraftView {
            version,
            amount_minor: 45000,
            currency: "VND".to_string(),
            merchant: "cafe".to_string(),
            category_display: "Khác".to_string(),
            type_label: "Chi tiêu".to_string(),
            date_display: "15/08/2026".to_string(),
        },
    }
}

fn first_reply(outcome: &ConversationOutcome) -> &str {
    outcome.replies.first().expect("reply").body.as_str()
}

#[test]
fn allowlist_denial_is_deterministic() {
    let ctx = AccountContext {
        next_expense_id: Uuid::new_v4(),
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: false,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        original_receipt_retention_days: 7,
        remaining_daily_receipts: 20,
        confirmed_expense_count: 0,
        pending: None,
        today_summary: None,
        week_summary: None,
        month_summary: None,
        schedules: vec![],
        recent_lines: vec![],
    };
    let outcome = decide(&ctx, "/help", Utc::now());
    assert_eq!(
        first_reply(&outcome),
        "Xin lỗi, tài khoản của bạn chưa được cấp quyền dùng bot trong giai đoạn thử nghiệm."
    );
    assert!(outcome.commands.is_empty());
}

#[test]
fn pending_consent_first_contact_shows_consent_card() {
    let ctx = pending_consent_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "xin chào", Utc::now());
    assert!(first_reply(&outcome).contains("Trả lời ok (hoặc \"đồng ý\")"));
    assert!(outcome.commands.is_empty());
}

#[test]
fn pending_consent_start_shows_consent_without_recording_it() {
    let ctx = pending_consent_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/start", Utc::now());
    assert!(first_reply(&outcome).starts_with("Xin chào!"));
    assert!(outcome.commands.is_empty());
}

#[test]
fn privacy_is_available_before_consent() {
    let ctx = pending_consent_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/privacy", Utc::now());
    assert!(first_reply(&outcome).contains("ảnh gốc được xóa sau 7 ngày"));
    assert!(outcome.commands.is_empty());
}

#[test]
fn pending_consent_dong_y_records_consent() {
    let ctx = pending_consent_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "đồng ý", Utc::now());
    assert!(first_reply(&outcome).starts_with("Cảm ơn bạn!"));
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::GrantConsent {
            consent_version: CONSENT_VERSION.to_string(),
        }]
    );
}

#[test]
fn active_start_and_help_show_help_text() {
    let ctx = active_ctx(Uuid::new_v4());
    let start = decide(&ctx, "/start", Utc::now());
    let help = decide(&ctx, "/help", Utc::now());
    assert!(first_reply(&start).contains("/today — chi tiêu hôm nay"));
    assert_eq!(first_reply(&start), first_reply(&help));
}

#[test]
fn unknown_input_returns_unknown_text() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "blah blah blah", Utc::now());
    assert_eq!(
        first_reply(&outcome),
        "Tôi chưa hiểu tin nhắn này. Gõ /help để xem cách dùng, hoặc nhập một khoản như \"an sang 500k\"."
    );
}

#[test]
fn slash_today_aliases_are_diacritics_insensitive() {
    let ctx = AccountContext {
        next_expense_id: Uuid::new_v4(),
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        original_receipt_retention_days: 7,
        remaining_daily_receipts: 20,
        confirmed_expense_count: 0,
        pending: None,
        today_summary: Some(PeriodSummary {
            label: "Hôm nay".to_string(),
            currency: "VND".to_string(),
            total_minor: 325000,
            tx_count: 2,
        }),
        week_summary: None,
        month_summary: None,
        schedules: vec![],
        recent_lines: vec![],
    };
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 10, 0, 0).unwrap();
    let today = decide(&ctx, "/today", now);
    let homnay = decide(&ctx, "/hômnay", now);
    let noisy = decide(&ctx, " /HomNay! ", now);
    assert!(first_reply(&today).contains("325.000 ₫"));
    assert_eq!(first_reply(&today), first_reply(&homnay));
    assert_eq!(first_reply(&today), first_reply(&noisy));
}

#[test]
fn slash_recent_and_history_aliases_match() {
    let ctx = AccountContext {
        next_expense_id: Uuid::new_v4(),
        next_submission_id: Uuid::new_v4(),
        next_ingest_job_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        original_receipt_retention_days: 7,
        remaining_daily_receipts: 20,
        confirmed_expense_count: 0,
        pending: None,
        today_summary: None,
        week_summary: None,
        month_summary: None,
        schedules: vec![],
        recent_lines: vec![RecentExpenseLine {
            date_display: "14/08".to_string(),
            amount_minor: 45000,
            currency: "VND".to_string(),
            merchant: "cafe".to_string(),
            category_display: "Khác".to_string(),
            type_label: None,
        }],
    };
    let recent = decide(&ctx, "/recent", Utc::now());
    let history = decide(&ctx, "/history", Utc::now());
    let phrase = decide(&ctx, "xem lịch sử giao dịch", Utc::now());
    assert!(first_reply(&recent).contains("Các khoản gần đây:"));
    assert_eq!(first_reply(&recent), first_reply(&history));
    assert_eq!(first_reply(&recent), first_reply(&phrase));
}

#[test]
fn manual_entry_leading_and_trailing_forms() {
    let ctx = active_ctx(Uuid::new_v4());
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 8, 0, 0).unwrap();

    let leading = decide(&ctx, "150k cafe", now);
    assert!(first_reply(&leading).contains("Cửa hàng: cafe"));
    assert!(first_reply(&leading).contains("150.000 ₫"));
    assert_eq!(leading.commands.len(), 1);
    let cmd = &leading.commands[0];
    assert!(matches!(
        cmd,
        DomainCommand::CreateManualAwaitingConfirmation {
            amount_minor: 150000,
            currency,
            description,
            ..
        } if currency == "VND" && description == "cafe"
    ));

    let trailing_breakfast = decide(&ctx, "an sang 500k", now);
    assert!(first_reply(&trailing_breakfast).contains("an sang"));
    assert!(first_reply(&trailing_breakfast).contains("500.000 ₫"));

    let trailing_cafe = decide(&ctx, "cafe 45k", now);
    assert!(first_reply(&trailing_cafe).contains("45.000 ₫"));

    let trailing_dotted = decide(&ctx, "com 80.000", now);
    assert!(first_reply(&trailing_dotted).contains("80.000 ₫"));
}

#[test]
fn vnd_formatting_and_amount_rejection() {
    assert_eq!(format_minor(325000, "VND"), "325.000 ₫");
    assert_eq!(
        parse_amount("45k", "VND").unwrap(),
        (45000, "VND".to_string())
    );
    assert_eq!(
        parse_amount("1tr5", "VND").unwrap(),
        (1500000, "VND".to_string())
    );
    assert_eq!(
        parse_amount("1tr500k", "VND").unwrap(),
        (1500000, "VND".to_string())
    );
    assert_eq!(
        parse_amount("2k5", "VND").unwrap(),
        (2500, "VND".to_string())
    );
    assert_eq!(
        parse_amount("1củ5", "VND").unwrap(),
        (1500000, "VND".to_string())
    );
    assert_eq!(
        parse_amount("1cu5", "VND").unwrap(),
        (1500000, "VND".to_string())
    );
    assert_eq!(
        parse_amount("-150k", "VND").unwrap_err(),
        AmountError::Negative
    );
    assert_eq!(
        parse_amount("10000000000000000", "VND").unwrap_err(),
        AmountError::TooLarge
    );
}

#[test]
fn manual_entry_arms_pending_with_fifteen_minute_expiry() {
    let account_id = Uuid::new_v4();
    let ctx = active_ctx(account_id);
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
    let outcome = decide(&ctx, "cafe 45k", now);
    let cmd = outcome.commands.first().expect("command");
    match cmd {
        DomainCommand::CreateManualAwaitingConfirmation {
            expense_id,
            optimistic_version,
            pending_expires_at,
            amount_minor,
            ..
        } => {
            assert_eq!(*optimistic_version, 1);
            assert_eq!(*amount_minor, 45000);
            assert_eq!(*pending_expires_at, now + Duration::seconds(900));
            assert_ne!(*expense_id, Uuid::nil());
        }
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(first_reply(&outcome).contains("Trả lời: ok / y để lưu"));
}

#[test]
fn manual_entry_uses_injected_id_deterministically() {
    let expense_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa").unwrap();
    let ctx = active_ctx(expense_id);
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
    let first = decide(&ctx, "cafe 45k", now);
    let second = decide(&ctx, "cafe 45k", now);
    assert_eq!(first.commands, second.commands);
    assert!(matches!(
        first.commands.as_slice(),
        [DomainCommand::CreateManualAwaitingConfirmation { expense_id: id, .. }] if *id == expense_id
    ));
}

#[test]
fn pending_ok_confirms_and_no_rejects() {
    let expense_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 5, 0).unwrap();
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.pending = Some(sample_pending(expense_id, 1, now));

    let confirmed = decide(&ctx, "ok", now);
    assert_eq!(
        first_reply(&confirmed),
        "Đã ghi nhận: 45.000 ₫ tại cafe (Khác)."
    );
    assert_eq!(
        confirmed.commands,
        vec![
            DomainCommand::ConfirmExpense {
                expense_id,
                expected_version: 1,
            },
            DomainCommand::ClearPending,
        ]
    );

    let rejected = decide(&ctx, "no", now);
    assert_eq!(
        first_reply(&rejected),
        "Đã bỏ qua, không ghi nhận khoản này."
    );
    assert_eq!(
        rejected.commands,
        vec![
            DomainCommand::RejectExpense {
                expense_id,
                expected_version: 1,
            },
            DomainCommand::ClearPending,
        ]
    );
}

#[test]
fn new_manual_text_does_not_orphan_an_existing_pending_draft() {
    let expense_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 5, 0).unwrap();
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.pending = Some(sample_pending(expense_id, 1, now));

    let outcome = decide(&ctx, "ăn trưa 80k", now);
    assert!(first_reply(&outcome).contains("45.000 ₫"));
    assert!(outcome.commands.is_empty());
}

#[test]
fn confirmation_without_pending_replies_without_invalid_clear_effect() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "ok", Utc::now());
    assert_eq!(
        first_reply(&outcome),
        "Yêu cầu trước đó đã hết hạn. Bạn gửi lại ảnh hoặc nhập lại nhé."
    );
    assert!(outcome.commands.is_empty());
}

#[test]
fn expired_pending_returns_exact_expiry_reply_and_clears_state() {
    let expense_id = Uuid::new_v4();
    let armed_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
    let mut pending = sample_pending(expense_id, 1, armed_at);
    pending.expires_at = armed_at + Duration::seconds(PENDING_CONFIRMATION_TTL_SECS);
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.pending = Some(pending);

    let after_expiry = armed_at + Duration::seconds(PENDING_CONFIRMATION_TTL_SECS);
    let outcome = decide(&ctx, "ok", after_expiry);
    assert_eq!(
        first_reply(&outcome),
        "Yêu cầu trước đó đã hết hạn. Bạn gửi lại ảnh hoặc nhập lại nhé."
    );
    assert_eq!(outcome.commands, vec![DomainCommand::ClearPending]);
}

#[test]
fn stale_version_returns_expiry_reply_and_clears_state() {
    let expense_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
    let mut pending = sample_pending(expense_id, 1, now);
    pending.draft.version = 2;
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.pending = Some(pending);

    let outcome = decide(&ctx, "ok", now);
    assert_eq!(
        first_reply(&outcome),
        "Yêu cầu trước đó đã hết hạn. Bạn gửi lại ảnh hoặc nhập lại nhé."
    );
    assert_eq!(outcome.commands, vec![DomainCommand::ClearPending]);
}

#[test]
fn explicit_slash_command_bypasses_pending_resolution() {
    let expense_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.pending = Some(sample_pending(expense_id, 1, now));
    ctx.today_summary = Some(PeriodSummary {
        label: "Hôm nay".to_string(),
        currency: "VND".to_string(),
        total_minor: 100000,
        tx_count: 1,
    });

    let outcome = decide(&ctx, "/today", now);
    assert!(first_reply(&outcome).contains("100.000 ₫"));
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::RecordInsightSnapshot {
            period_kind: "day".to_string(),
        }]
    );
}

#[test]
fn bare_command_does_not_bypass_pending_resolution() {
    let expense_id = Uuid::new_v4();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.pending = Some(sample_pending(expense_id, 1, now));
    ctx.today_summary = Some(PeriodSummary {
        label: "Hôm nay".to_string(),
        currency: "VND".to_string(),
        total_minor: 100000,
        tx_count: 1,
    });

    let outcome = decide(&ctx, "today", now);
    assert!(first_reply(&outcome).starts_with("Tôi đọc được:"));
    assert!(outcome.commands.is_empty());
}

#[test]
fn decide_image_at_daily_quota_returns_vietnamese_limit_copy() {
    use zl_expense::conversation::{daily_receipt_quota_text, decide_image};
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.remaining_daily_receipts = 0;
    let outcome = decide_image(&ctx, Utc::now());
    assert_eq!(first_reply(&outcome), daily_receipt_quota_text());
    assert!(outcome.commands.is_empty());
}

#[test]
fn formatting_handles_minimum_signed_value() {
    assert_eq!(
        format_minor(i64::MIN, "VND"),
        "-9.223.372.036.854.775.808 ₫"
    );
}

#[test]
fn slash_week_renders_week_summary() {
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.week_summary = Some(PeriodSummary {
        label: "Tuần này".to_string(),
        currency: "VND".to_string(),
        total_minor: 120000,
        tx_count: 1,
    });
    let outcome = decide(&ctx, "/week", Utc::now());
    assert!(first_reply(&outcome).contains("120.000 ₫"));
}

#[test]
fn slash_tz_valid_issues_set_timezone_command() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/tz Asia/Bangkok", Utc::now());
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::SetTimezone {
            iana: "Asia/Bangkok".to_string(),
        }]
    );
}

#[test]
fn slash_sched_set_issues_upsert_schedule_command() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/sched daily 20:00", Utc::now());
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::UpsertSchedule {
            frequency: "daily".to_string(),
            delivery_minute: 20 * 60,
        }]
    );
}

#[test]
fn slash_sched_midnight_is_upsert_not_disable() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/sched daily 00:00", Utc::now());
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::UpsertSchedule {
            frequency: "daily".to_string(),
            delivery_minute: 0,
        }]
    );
}

#[test]
fn slash_sched_off_daily_disables_frequency() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/sched off daily", Utc::now());
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::DisableSchedule {
            frequency: Some("daily".to_string()),
        }]
    );
}

#[test]
fn slash_delete_arms_two_step_confirmation_with_counts_only() {
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.confirmed_expense_count = 3;
    let now = Utc::now();
    let outcome = decide(&ctx, "/delete", now);
    assert!(first_reply(&outcome).contains("3 khoản chi"));
    assert!(!first_reply(&outcome).contains("cafe"));
    assert!(matches!(
        outcome.commands.as_slice(),
        [DomainCommand::RequestAccountDeletion { .. }]
    ));
}

#[test]
fn pending_delete_ok_confirms_without_content_in_reply() {
    let mut ctx = active_ctx(Uuid::new_v4());
    ctx.confirmed_expense_count = 2;
    ctx.pending = Some(PendingConfirmation {
        kind: PendingKind::AccountDeletion,
        reference_id: ctx.next_expense_id,
        optimistic_version: 1,
        expires_at: Utc::now() + Duration::minutes(10),
        draft: ManualDraftView {
            version: 1,
            amount_minor: 0,
            currency: "VND".to_string(),
            merchant: "secret-merchant".to_string(),
            category_display: String::new(),
            type_label: String::new(),
            date_display: String::new(),
        },
    });
    let outcome = decide(&ctx, "ok", Utc::now());
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::ConfirmAccountDeletion]
    );
    let body = first_reply(&outcome);
    assert!(body.contains("2 khoản chi"));
    assert!(!body.contains("secret-merchant"));
}

#[test]
fn slash_export_never_mentions_filesystem_paths() {
    let ctx = active_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/export", Utc::now());
    assert_eq!(outcome.commands, vec![DomainCommand::RequestAccountExport]);
    let body = first_reply(&outcome);
    assert!(!body.contains("/var/"));
    assert!(!body.contains("s3://"));
    assert!(!body.contains(".json"));
    assert!(!body.contains(".csv"));
    assert!(!body.contains("exports/"));
}
