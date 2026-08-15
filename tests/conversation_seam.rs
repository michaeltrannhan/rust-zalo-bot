//! Public conversation seam integration tests.

use chrono::{Duration, TimeZone, Utc};
use uuid::Uuid;

use zl_expense::conversation::{
    AccountContext, AmountError, CONSENT_VERSION, ConversationOutcome, DomainCommand,
    LifecycleState, ManualDraftView, PENDING_CONFIRMATION_TTL_SECS, PendingConfirmation,
    PeriodSummary, RecentExpenseLine, decide, format_minor, parse_amount,
};

fn active_ctx(account_id: Uuid) -> AccountContext {
    AccountContext {
        account_id,
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        pending: None,
        today_summary: None,
        recent_lines: vec![],
    }
}

fn pending_consent_ctx(account_id: Uuid) -> AccountContext {
    AccountContext {
        account_id,
        lifecycle: LifecycleState::PendingConsent,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        pending: None,
        today_summary: None,
        recent_lines: vec![],
    }
}

fn sample_pending(
    expense_id: Uuid,
    version: u64,
    now: chrono::DateTime<Utc>,
) -> PendingConfirmation {
    PendingConfirmation {
        expense_id,
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
        account_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: false,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        pending: None,
        today_summary: None,
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
fn pending_consent_start_records_consent() {
    let ctx = pending_consent_ctx(Uuid::new_v4());
    let outcome = decide(&ctx, "/start", Utc::now());
    assert!(first_reply(&outcome).starts_with("Cảm ơn bạn!"));
    assert_eq!(
        outcome.commands,
        vec![DomainCommand::GrantConsent {
            consent_version: CONSENT_VERSION.to_string(),
        }]
    );
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
        account_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        pending: None,
        today_summary: Some(PeriodSummary {
            label: "Hôm nay".to_string(),
            currency: "VND".to_string(),
            total_minor: 325000,
            tx_count: 2,
        }),
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
        account_id: Uuid::new_v4(),
        lifecycle: LifecycleState::Active,
        allowlisted: true,
        default_currency: "VND".to_string(),
        timezone: "Asia/Ho_Chi_Minh".to_string(),
        pending: None,
        today_summary: None,
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
    assert!(outcome.commands.is_empty());
}
