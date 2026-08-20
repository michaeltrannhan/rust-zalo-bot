use super::locale::Locale;
use super::money::format_minor;
use super::types::RecentExpenseLine;

pub fn not_allowed_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Xin lỗi, tài khoản của bạn chưa được cấp quyền dùng bot trong giai đoạn thử nghiệm."
                .to_string()
        }
        Locale::En => "Sorry, your account is not allowlisted for this pilot yet.".to_string(),
    }
}

pub fn suspended_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Tài khoản của bạn đang tạm dừng. Vui lòng liên hệ quản trị viên để được hỗ trợ."
                .to_string()
        }
        Locale::En => "Your account is suspended. Please contact an admin for help.".to_string(),
    }
}

pub fn consent_card_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Xin chào! Tôi giúp ghi nhận và tổng hợp các khoản chi từ ảnh hóa đơn.\n\nBạn chỉ cần gửi ảnh hóa đơn, hoặc nhập nhanh một khoản như \"an sang 500k\".\n\nDữ liệu có thể bao gồm thông tin mua hàng và được lưu để tạo báo cáo chi tiêu. Bạn có thể xóa dữ liệu bất kỳ lúc nào bằng /delete.\n\nTrả lời ok (hoặc \"đồng ý\") để bắt đầu, hoặc /privacy để xem cách dữ liệu được sử dụng.".to_string()
        }
        Locale::En => {
            "Hi! I help log and summarize expenses from receipt photos.\n\nSend a receipt image, or type a quick entry like \"coffee 45k\".\n\nPurchase details may be stored so I can build your private spending reports. Delete everything anytime with /delete.\n\nReply ok (or \"agree\") to start, or /privacy to see how data is used.".to_string()
        }
    }
}

pub fn welcome_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Cảm ơn bạn! Từ giờ bạn có thể:\n\n• Gửi ảnh hóa đơn để tôi đọc và ghi nhận.\n• Nhập nhanh: \"an sang 500k\", \"cafe 45k\".\n• Xem tổng hợp: /today, /week, /month.\n• Đổi ngôn ngữ: /lang en hoặc /lang vi.\n• Xem múi giờ, tiền tệ và lịch tổng kết bằng /settings.\n\nGõ /help để xem tất cả lệnh.".to_string()
        }
        Locale::En => {
            "Thanks! You can now:\n\n• Send a receipt photo for me to read and log.\n• Quick entry: \"lunch 500k\", \"coffee 45k\".\n• Summaries: /today, /week, /month.\n• Language: /lang en or /lang vi.\n• Timezone, currency, and schedules via /settings.\n\nType /help for all commands.".to_string()
        }
    }
}

pub fn privacy_text(locale: Locale, retention_days: u32) -> String {
    match locale {
        Locale::Vi => format!(
            "Cách tôi dùng dữ liệu của bạn:\n\n• Ảnh hóa đơn chỉ dùng để đọc thông tin giao dịch; ảnh gốc được xóa sau {retention_days} ngày.\n• Giao dịch đã ghi nhận được lưu để tổng hợp báo cáo chi tiêu cho riêng bạn.\n\nGửi /export để tải về toàn bộ dữ liệu, /delete để xóa vĩnh viễn."
        ),
        Locale::En => format!(
            "How I use your data:\n\n• Receipt images are only used to read transaction fields; originals are deleted after {retention_days} days.\n• Confirmed transactions are kept so I can build your private spending reports.\n\nSend /export to download everything, or /delete to erase it permanently."
        ),
    }
}

pub fn help_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Tôi có thể giúp bạn:\n\n• Gửi ảnh hóa đơn — tôi đọc và gửi thẻ xác nhận.\n• Nhập nhanh: \"an sang 500k\", \"cafe 45k\".\n\nKhi có thẻ xác nhận (chưa ok):\n• sua 45k / edit 45000 — sửa số tiền\n• sua cua hang X / edit merchant X — sửa cửa hàng\n• sua danh muc an-uong / edit category Food — sửa danh mục\n• sua ngay 20/08/2026 / edit date 2026-08-20 — sửa ngày\n• sua loai chi|thu / edit type expense|income — sửa loại\n• ok / y — lưu · no / n — hủy\n\nSau khi đã lưu:\n• /recat an-uong hoặc \"phan loai Food\" — đổi danh mục khoản gần nhất\n• /categories — xem danh mục\n\nLệnh:\n/help · /today · /week · /month · /recent\n/settings · /tz Asia/Ho_Chi_Minh · /sched\n/export · /delete · /privacy\n/lang en | /lang vi — đổi ngôn ngữ\n/categories · /recat <danh-muc>".to_string()
        }
        Locale::En => {
            "I can help you:\n\n• Send a receipt photo — I read it and send a review card.\n• Quick entry: \"lunch 500k\", \"coffee 45k\".\n\nWhile a review card is open (before ok):\n• edit 45000 / sua 45k — change amount\n• edit merchant X / sua cua hang X — change merchant\n• edit category Food / sua danh muc an-uong — change category\n• edit date 2026-08-20 / sua ngay 20/08/2026 — change date\n• edit type expense|income / sua loai chi|thu — change type\n• ok / y — save · no / n — discard\n\nAfter saving:\n• /recat Food or \"phan loai an-uong\" — recategorize the latest expense\n• /categories — list categories\n\nCommands:\n/help · /today · /week · /month · /recent\n/settings · /tz Asia/Ho_Chi_Minh · /sched\n/export · /delete · /privacy\n/lang en | /lang vi — switch language\n/categories · /recat <category>".to_string()
        }
    }
}

pub fn unknown_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Tôi chưa hiểu tin nhắn này. Gõ /help để xem cách dùng, hoặc nhập một khoản như \"an sang 500k\"."
                .to_string()
        }
        Locale::En => {
            "I didn't understand that. Type /help for usage, or enter something like \"coffee 45k\"."
                .to_string()
        }
    }
}

pub fn pending_expired_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Yêu cầu trước đó đã hết hạn. Bạn gửi lại ảnh hoặc nhập lại nhé.".to_string(),
        Locale::En => {
            "That confirmation expired. Please send the receipt or entry again.".to_string()
        }
    }
}

pub fn discarded_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Đã bỏ qua, không ghi nhận khoản này.".to_string(),
        Locale::En => "Discarded — this expense was not saved.".to_string(),
    }
}

pub fn confirmed_text(locale: Locale, amount: &str, merchant: &str, category: &str) -> String {
    match locale {
        Locale::Vi => format!("Đã ghi nhận: {amount} tại {merchant} ({category})."),
        Locale::En => format!("Saved: {amount} at {merchant} ({category})."),
    }
}

pub fn manual_confirmation_card(
    locale: Locale,
    merchant: &str,
    amount: &str,
    date: &str,
    type_label: &str,
    category: &str,
) -> String {
    match locale {
        Locale::Vi => format!(
            "Tôi đọc được:\n\nCửa hàng: {merchant}\nSố tiền: {amount}\nNgày: {date}\nLoại: {type_label}\nDanh mục: {category}\n\nTrả lời: ok / y để lưu · no / n để hủy\nSửa: sua 45k · sua cua hang X · sua danh muc an-uong · sua ngay 20/08/2026 · sua loai chi|thu\n(EN: edit amount/merchant/category/date/type)"
        ),
        Locale::En => format!(
            "I read:\n\nMerchant: {merchant}\nAmount: {amount}\nDate: {date}\nType: {type_label}\nCategory: {category}\n\nReply: ok / y to save · no / n to discard\nEdit: edit 45000 · edit merchant X · edit category Food · edit date 2026-08-20 · edit type expense|income\n(VN: sua …)"
        ),
    }
}

pub fn empty_summary_text(locale: Locale, period_label: &str) -> String {
    match locale {
        Locale::Vi => format!(
            "{period_label}: chưa có khoản chi tiêu nào được ghi nhận. Gửi ảnh hóa đơn hoặc nhập: \"an sang 500k\"."
        ),
        Locale::En => format!(
            "{period_label}: no expenses logged yet. Send a receipt photo or type: \"coffee 45k\"."
        ),
    }
}

pub fn today_summary_text(locale: Locale, label: &str, currency: &str, total_minor: i64) -> String {
    let amount = format_minor(total_minor, currency);
    match locale {
        Locale::Vi => format!("{label} — chi tiêu đã ghi nhận:\n\nTổng: {amount}"),
        Locale::En => format!("{label} — logged spending:\n\nTotal: {amount}"),
    }
}

pub fn period_summary_text(
    locale: Locale,
    label: &str,
    currency: &str,
    total_minor: i64,
    categories: &[(String, i64)],
) -> String {
    let mut body = today_summary_text(locale, label, currency, total_minor);
    for (display, minor) in categories.iter().take(3) {
        body.push_str(&format!(
            "\n• {display}: {}",
            format_minor(*minor, currency)
        ));
    }
    body
}

pub fn settings_text(
    locale: Locale,
    timezone: &str,
    currency: &str,
    schedules: &[super::types::ScheduleLine],
) -> String {
    let mut body = match locale {
        Locale::Vi => String::from("Cài đặt của bạn:\n\n"),
        Locale::En => String::from("Your settings:\n\n"),
    };
    match locale {
        Locale::Vi => {
            body.push_str(&format!("• Múi giờ: {timezone}\n"));
            body.push_str(&format!("• Tiền tệ mặc định: {currency}\n"));
            body.push_str("• Tổng kết tự động: ");
        }
        Locale::En => {
            body.push_str(&format!("• Timezone: {timezone}\n"));
            body.push_str(&format!("• Default currency: {currency}\n"));
            body.push_str("• Scheduled summaries: ");
        }
    }
    let active: Vec<&super::types::ScheduleLine> =
        schedules.iter().filter(|line| line.enabled).collect();
    if active.is_empty() {
        body.push_str(match locale {
            Locale::Vi => "chưa bật",
            Locale::En => "off",
        });
    } else {
        for (index, line) in active.iter().enumerate() {
            if index > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                " {} {:02}:{:02}",
                schedule_frequency_label(locale, &line.frequency),
                line.delivery_minute / 60,
                line.delivery_minute % 60
            ));
        }
    }
    match locale {
        Locale::Vi => {
            body.push_str("\n\nThay đổi:\n/tz Asia/Ho_Chi_Minh\n/sched daily 20:00\n/lang en")
        }
        Locale::En => {
            body.push_str("\n\nChange:\n/tz Asia/Ho_Chi_Minh\n/sched daily 20:00\n/lang vi")
        }
    }
    body
}

pub fn settings_updated_text(
    locale: Locale,
    label: &str,
    value: &str,
    timezone: &str,
    currency: &str,
    schedules: &[super::types::ScheduleLine],
) -> String {
    match locale {
        Locale::Vi => format!(
            "Đã cập nhật {label}: {value}.\n\n{}",
            settings_text(locale, timezone, currency, schedules)
        ),
        Locale::En => format!(
            "Updated {label}: {value}.\n\n{}",
            settings_text(locale, timezone, currency, schedules)
        ),
    }
}

pub fn language_updated_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Đã chuyển sang tiếng Việt. Gõ /help để xem hướng dẫn.".to_string(),
        Locale::En => "Switched to English. Type /help for instructions.".to_string(),
    }
}

pub fn invalid_timezone_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Múi giờ không hợp lệ. Hãy dùng tên IANA, ví dụ Asia/Ho_Chi_Minh, Asia/Bangkok hoặc UTC."
                .to_string()
        }
        Locale::En => {
            "Invalid timezone. Use an IANA name like Asia/Ho_Chi_Minh, Asia/Bangkok, or UTC."
                .to_string()
        }
    }
}

pub fn invalid_settings_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Cú pháp chưa đúng. Dùng /settings để xem, /tz Asia/Ho_Chi_Minh để thay đổi múi giờ."
                .to_string()
        }
        Locale::En => {
            "Invalid syntax. Use /settings to view, /tz Asia/Ho_Chi_Minh to change timezone."
                .to_string()
        }
    }
}

pub fn schedule_text(locale: Locale, schedules: &[super::types::ScheduleLine]) -> String {
    let mut body = match locale {
        Locale::Vi => String::from("Tổng kết tự động:\n"),
        Locale::En => String::from("Scheduled summaries:\n"),
    };
    let active: Vec<&super::types::ScheduleLine> =
        schedules.iter().filter(|line| line.enabled).collect();
    if active.is_empty() {
        body.push_str(match locale {
            Locale::Vi => "• Chưa bật lịch nào.\n",
            Locale::En => "• No schedules enabled.\n",
        });
    } else {
        for line in active {
            body.push_str(&format!(
                "• {} lúc {:02}:{:02}\n",
                schedule_frequency_label(locale, &line.frequency),
                line.delivery_minute / 60,
                line.delivery_minute % 60
            ));
        }
    }
    match locale {
        Locale::Vi => body.push_str(
            "\nCài đặt:\n/sched daily 20:00\n/sched weekly 08:00\n/sched monthly 09:00\n/sched off daily — tắt một lịch\n/sched off — tắt tất cả",
        ),
        Locale::En => body.push_str(
            "\nSetup:\n/sched daily 20:00\n/sched weekly 08:00\n/sched monthly 09:00\n/sched off daily — disable one\n/sched off — disable all",
        ),
    }
    body
}

pub fn schedule_set_text(locale: Locale, frequency: &str, delivery_minute: i32) -> String {
    match locale {
        Locale::Vi => format!(
            "Đã bật tổng kết {} lúc {:02}:{:02}.",
            schedule_frequency_label(locale, frequency),
            delivery_minute / 60,
            delivery_minute % 60
        ),
        Locale::En => format!(
            "Enabled {} summary at {:02}:{:02}.",
            schedule_frequency_label(locale, frequency),
            delivery_minute / 60,
            delivery_minute % 60
        ),
    }
}

pub fn schedule_disabled_text(locale: Locale, frequency: Option<&str>) -> String {
    match (locale, frequency) {
        (Locale::Vi, Some(frequency)) => {
            format!(
                "Đã tắt tổng kết {}.",
                schedule_frequency_label(locale, frequency)
            )
        }
        (Locale::Vi, None) => "Đã tắt tất cả lịch tổng kết.".to_string(),
        (Locale::En, Some(frequency)) => {
            format!(
                "Disabled {} summary.",
                schedule_frequency_label(locale, frequency)
            )
        }
        (Locale::En, None) => "Disabled all scheduled summaries.".to_string(),
    }
}

pub fn schedule_invalid_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Cú pháp chưa đúng. Ví dụ: /sched daily 20:00, /sched weekly 08:00, hoặc /sched off."
                .to_string()
        }
        Locale::En => {
            "Invalid syntax. Examples: /sched daily 20:00, /sched weekly 08:00, or /sched off."
                .to_string()
        }
    }
}

pub fn delete_confirm_text(locale: Locale, expense_count: u32) -> String {
    match locale {
        Locale::Vi => format!(
            "Bạn sắp xóa toàn bộ dữ liệu ({expense_count} khoản chi). Trả lời ok để xác nhận, no để hủy. Không thể hoàn tác."
        ),
        Locale::En => format!(
            "You are about to delete all data ({expense_count} expenses). Reply ok to confirm, no to cancel. This cannot be undone."
        ),
    }
}

pub fn delete_accepted_text(locale: Locale, expense_count: u32) -> String {
    match locale {
        Locale::Vi => {
            format!(
                "Đã nhận yêu cầu xóa ({expense_count} khoản chi). Dữ liệu sẽ được gỡ trong ít phút."
            )
        }
        Locale::En => {
            format!("Deletion requested ({expense_count} expenses). Data will be removed shortly.")
        }
    }
}

pub fn delete_cancelled_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Đã hủy yêu cầu xóa dữ liệu.".to_string(),
        Locale::En => "Account deletion cancelled.".to_string(),
    }
}

pub fn export_accepted_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Đã ghi nhận yêu cầu xuất dữ liệu. File sẽ được giao qua kênh quản trị viên, không gửi đường dẫn trong chat.".to_string()
        }
        Locale::En => {
            "Export requested. The file will be delivered via an admin channel — no paths in chat."
                .to_string()
        }
    }
}

fn schedule_frequency_label(locale: Locale, frequency: &str) -> &str {
    match (locale, frequency) {
        (Locale::Vi, "daily") => "hàng ngày",
        (Locale::Vi, "weekly") => "hàng tuần",
        (Locale::Vi, "monthly") => "hàng tháng",
        (Locale::En, "daily") => "daily",
        (Locale::En, "weekly") => "weekly",
        (Locale::En, "monthly") => "monthly",
        _ => frequency,
    }
}

pub fn no_recent_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Chưa có giao dịch nào được ghi nhận. Gửi ảnh hóa đơn hoặc nhập: \"an sang 500k\"."
                .to_string()
        }
        Locale::En => {
            "No transactions yet. Send a receipt photo or type: \"coffee 45k\".".to_string()
        }
    }
}

pub fn recent_text(locale: Locale, lines: &[RecentExpenseLine]) -> String {
    if lines.is_empty() {
        return no_recent_text(locale);
    }
    let mut body = match locale {
        Locale::Vi => String::from("Các khoản gần đây:\n"),
        Locale::En => String::from("Recent expenses:\n"),
    };
    let expense_label = transaction_type_label(locale, "expense");
    for line in lines {
        let amount = format_minor(line.amount_minor, &line.currency);
        let prefix = match &line.type_label {
            Some(t) if t != expense_label => format!("[{t}] "),
            _ => String::new(),
        };
        body.push_str(&format!(
            "{prefix}{date} · {amount} · {merchant} · {category}\n",
            date = line.date_display,
            merchant = line.merchant,
            category = line.category_display,
        ));
    }
    body.trim_end().to_string()
}

pub fn default_category_display(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Khác".to_string(),
        Locale::En => "Other".to_string(),
    }
}

pub fn categories_list_text(locale: Locale, body: &str) -> String {
    match locale {
        Locale::Vi => format!(
            "Danh mục (dùng key hoặc tên):\n{body}\n\nVí dụ khi đang xác nhận: sua danh muc an-uong\nSau khi lưu: /recat an-uong"
        ),
        Locale::En => format!(
            "Categories (use key or name):\n{body}\n\nWhile reviewing: edit category Food\nAfter save: /recat Food"
        ),
    }
}

pub fn invalid_category_text(locale: Locale, body: &str) -> String {
    match locale {
        Locale::Vi => format!("Không nhận ra danh mục đó.\n\n{body}"),
        Locale::En => format!("I don't recognize that category.\n\n{body}"),
    }
}

pub fn invalid_edit_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Cú pháp sửa chưa đúng. Ví dụ: sua 45k · sua danh muc an-uong · sua cua hang X · sua ngay 20/08/2026 · sua loai chi"
                .to_string()
        }
        Locale::En => {
            "Invalid edit. Examples: edit 45000 · edit category Food · edit merchant X · edit date 2026-08-20 · edit type expense"
                .to_string()
        }
    }
}

pub fn invalid_language_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Dùng /lang vi hoặc /lang en.".to_string(),
        Locale::En => "Use /lang vi or /lang en.".to_string(),
    }
}

pub fn recategorized_text(locale: Locale, merchant: &str, category: &str) -> String {
    match locale {
        Locale::Vi => format!("Đã đổi danh mục khoản gần nhất ({merchant}) → {category}."),
        Locale::En => format!("Updated latest expense ({merchant}) → {category}."),
    }
}

pub fn no_expense_to_recategorize_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Chưa có khoản đã lưu để đổi danh mục. Ghi nhận một khoản trước đã.".to_string()
        }
        Locale::En => "No saved expense to recategorize yet. Log one first.".to_string(),
    }
}

pub fn image_received_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Đã nhận ảnh hóa đơn. Tôi sẽ đọc và gửi lại để bạn xác nhận.".to_string(),
        Locale::En => {
            "Got the receipt photo. I'll read it and send a confirmation card.".to_string()
        }
    }
}

pub fn daily_receipt_quota_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Bạn đã đạt giới hạn hóa đơn hôm nay. Mai gửi tiếp nhé.".to_string(),
        Locale::En => "You've hit today's receipt limit. Try again tomorrow.".to_string(),
    }
}

pub fn extraction_kill_switch_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => "Trích xuất hóa đơn đang tạm tắt.".to_string(),
        Locale::En => "Receipt extraction is temporarily disabled.".to_string(),
    }
}

pub fn extraction_failed_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Tôi chưa đọc được hóa đơn này. Bạn gửi lại ảnh rõ hơn, hoặc nhập tay kiểu \"cafe 45k\" nhé."
                .to_string()
        }
        Locale::En => {
            "I couldn't read that receipt. Send a clearer photo, or type something like \"coffee 45k\"."
                .to_string()
        }
    }
}

pub fn extraction_unsupported_text(locale: Locale) -> String {
    match locale {
        Locale::Vi => {
            "Ảnh này không giống hóa đơn. Gửi lại ảnh hóa đơn, hoặc nhập tay kiểu \"cafe 45k\"."
                .to_string()
        }
        Locale::En => {
            "That doesn't look like a receipt. Send a receipt photo, or type \"coffee 45k\"."
                .to_string()
        }
    }
}

pub fn transaction_type_label(locale: Locale, transaction_type: &str) -> &'static str {
    match (locale, transaction_type) {
        (Locale::Vi, "income") => "Thu nhập",
        (Locale::Vi, "refund") => "Hoàn tiền",
        (Locale::Vi, "transfer") => "Chuyển khoản",
        (Locale::Vi, "adjustment") => "Điều chỉnh",
        (Locale::Vi, _) => "Chi tiêu",
        (Locale::En, "income") => "Income",
        (Locale::En, "refund") => "Refund",
        (Locale::En, "transfer") => "Transfer",
        (Locale::En, "adjustment") => "Adjustment",
        (Locale::En, _) => "Expense",
    }
}

pub fn default_type_label(locale: Locale) -> &'static str {
    transaction_type_label(locale, "expense")
}

pub fn period_label_today(locale: Locale) -> &'static str {
    match locale {
        Locale::Vi => "Hôm nay",
        Locale::En => "Today",
    }
}

pub fn period_label_week(locale: Locale) -> &'static str {
    match locale {
        Locale::Vi => "Tuần này",
        Locale::En => "This week",
    }
}

pub fn period_label_month(locale: Locale) -> &'static str {
    match locale {
        Locale::Vi => "Tháng này",
        Locale::En => "This month",
    }
}

pub fn settings_label_timezone(locale: Locale) -> &'static str {
    match locale {
        Locale::Vi => "múi giờ",
        Locale::En => "timezone",
    }
}
