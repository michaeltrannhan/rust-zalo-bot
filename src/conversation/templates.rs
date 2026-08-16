use super::money::format_minor;
use super::types::RecentExpenseLine;

pub fn not_allowed_text() -> String {
    "Xin lỗi, tài khoản của bạn chưa được cấp quyền dùng bot trong giai đoạn thử nghiệm."
        .to_string()
}

pub fn suspended_text() -> String {
    "Tài khoản của bạn đang tạm dừng. Vui lòng liên hệ quản trị viên để được hỗ trợ.".to_string()
}

pub fn consent_card_text() -> String {
    "Xin chào! Tôi giúp ghi nhận và tổng hợp các khoản chi từ ảnh hóa đơn.\n\nBạn chỉ cần gửi ảnh hóa đơn, hoặc nhập nhanh một khoản như \"an sang 500k\".\n\nDữ liệu có thể bao gồm thông tin mua hàng và được lưu để tạo báo cáo chi tiêu. Bạn có thể xóa dữ liệu bất kỳ lúc nào bằng /delete.\n\nTrả lời ok (hoặc \"đồng ý\") để bắt đầu, hoặc /privacy để xem cách dữ liệu được sử dụng.".to_string()
}

pub fn welcome_text() -> String {
    "Cảm ơn bạn! Từ giờ bạn có thể:\n\n• Gửi ảnh hóa đơn để tôi đọc và ghi nhận.\n• Nhập nhanh: \"an sang 500k\", \"cafe 45k\".\n• Xem tổng hợp: /today, /week, /month.\n• Xem múi giờ, tiền tệ và lịch tổng kết bằng /settings.\n\nGõ /help để xem tất cả lệnh.".to_string()
}

pub fn privacy_text(retention_days: u32) -> String {
    format!(
        "Cách tôi dùng dữ liệu của bạn:\n\n• Ảnh hóa đơn chỉ dùng để đọc thông tin giao dịch; ảnh gốc được xóa sau {retention_days} ngày.\n• Giao dịch đã ghi nhận được lưu để tổng hợp báo cáo chi tiêu cho riêng bạn.\n\nGửi /export để tải về toàn bộ dữ liệu, /delete để xóa vĩnh viễn."
    )
}

pub fn help_text() -> String {
    "Tôi có thể giúp bạn:\n\n• Gửi ảnh hóa đơn — tôi đọc và gợi ý để bạn xác nhận.\n• Nhập nhanh một khoản: \"an sang 500k\", \"cafe 45k\", \"150k cafe\".\n\nLệnh:\n/help — xem hướng dẫn này\n/today — chi tiêu hôm nay\n/week — chi tiêu tuần này\n/month — chi tiêu tháng này\n/recent — các khoản gần đây\n/settings — múi giờ, tiền tệ\n/tz — đổi múi giờ, ví dụ /tz Asia/Ho_Chi_Minh\n/sched — tổng kết tự động\n/export — tải dữ liệu về\n/delete — xóa toàn bộ dữ liệu (cần ok lần nữa)\n/privacy — cách dữ liệu được sử dụng\n\nok / y — xác nhận · no / n — bỏ qua · edit — sửa số tiền\nCâu tiếng Việt cũ (/homnay, xác nhận, …) vẫn dùng được.".to_string()
}

pub fn unknown_text() -> String {
    "Tôi chưa hiểu tin nhắn này. Gõ /help để xem cách dùng, hoặc nhập một khoản như \"an sang 500k\".".to_string()
}

pub fn pending_expired_text() -> String {
    "Yêu cầu trước đó đã hết hạn. Bạn gửi lại ảnh hoặc nhập lại nhé.".to_string()
}

pub fn discarded_text() -> String {
    "Đã bỏ qua, không ghi nhận khoản này.".to_string()
}

pub fn confirmed_text(amount: &str, merchant: &str, category: &str) -> String {
    format!("Đã ghi nhận: {amount} tại {merchant} ({category}).")
}

pub fn manual_confirmation_card(
    merchant: &str,
    amount: &str,
    date: &str,
    type_label: &str,
    category: &str,
) -> String {
    format!(
        "Tôi đọc được:\n\nCửa hàng: {merchant}\nSố tiền: {amount}\nNgày: {date}\nLoại: {type_label}\nDanh mục: {category}\n\nTrả lời: ok / y để lưu · edit / fix để sửa số tiền · no / n để hủy"
    )
}

pub fn empty_summary_text(period_label: &str) -> String {
    format!(
        "{period_label}: chưa có khoản chi tiêu nào được ghi nhận. Gửi ảnh hóa đơn hoặc nhập: \"an sang 500k\"."
    )
}

pub fn today_summary_text(label: &str, currency: &str, total_minor: i64) -> String {
    let amount = format_minor(total_minor, currency);
    format!("{label} — chi tiêu đã ghi nhận:\n\nTổng: {amount}")
}

pub fn period_summary_text(
    label: &str,
    currency: &str,
    total_minor: i64,
    categories: &[(String, i64)],
) -> String {
    let mut body = today_summary_text(label, currency, total_minor);
    for (display, minor) in categories.iter().take(3) {
        body.push_str(&format!(
            "\n• {display}: {}",
            format_minor(*minor, currency)
        ));
    }
    body
}

pub fn settings_text(
    timezone: &str,
    currency: &str,
    schedules: &[super::types::ScheduleLine],
) -> String {
    let mut body = String::from("Cài đặt của bạn:\n\n");
    body.push_str(&format!("• Múi giờ: {timezone}\n"));
    body.push_str(&format!("• Tiền tệ mặc định: {currency}\n"));
    body.push_str("• Tổng kết tự động: ");
    let active: Vec<&super::types::ScheduleLine> =
        schedules.iter().filter(|line| line.enabled).collect();
    if active.is_empty() {
        body.push_str("chưa bật");
    } else {
        for (index, line) in active.iter().enumerate() {
            if index > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                " {} {:02}:{:02}",
                schedule_frequency_label(&line.frequency),
                line.delivery_minute / 60,
                line.delivery_minute % 60
            ));
        }
    }
    body.push_str("\n\nThay đổi:\n/tz Asia/Ho_Chi_Minh\n/sched daily 20:00");
    body
}

pub fn settings_updated_text(
    label: &str,
    value: &str,
    timezone: &str,
    currency: &str,
    schedules: &[super::types::ScheduleLine],
) -> String {
    format!(
        "Đã cập nhật {label}: {value}.\n\n{}",
        settings_text(timezone, currency, schedules)
    )
}

pub fn invalid_timezone_text() -> String {
    "Múi giờ không hợp lệ. Hãy dùng tên IANA, ví dụ Asia/Ho_Chi_Minh, Asia/Bangkok hoặc UTC."
        .to_string()
}

pub fn invalid_settings_text() -> String {
    "Cú pháp chưa đúng. Dùng /settings để xem, /tz Asia/Ho_Chi_Minh để thay đổi múi giờ."
        .to_string()
}

pub fn schedule_text(schedules: &[super::types::ScheduleLine]) -> String {
    let mut body = String::from("Tổng kết tự động:\n");
    let active: Vec<&super::types::ScheduleLine> =
        schedules.iter().filter(|line| line.enabled).collect();
    if active.is_empty() {
        body.push_str("• Chưa bật lịch nào.\n");
    } else {
        for line in active {
            body.push_str(&format!(
                "• {} lúc {:02}:{:02}\n",
                schedule_frequency_label(&line.frequency),
                line.delivery_minute / 60,
                line.delivery_minute % 60
            ));
        }
    }
    body.push_str("\nCài đặt:\n/sched daily 20:00\n/sched weekly 08:00\n/sched monthly 09:00\n/sched off daily — tắt một lịch\n/sched off — tắt tất cả");
    body
}

pub fn schedule_set_text(frequency: &str, delivery_minute: i32) -> String {
    format!(
        "Đã bật tổng kết {} lúc {:02}:{:02}.",
        schedule_frequency_label(frequency),
        delivery_minute / 60,
        delivery_minute % 60
    )
}

pub fn schedule_disabled_text(frequency: Option<&str>) -> String {
    match frequency {
        Some(frequency) => format!("Đã tắt tổng kết {}.", schedule_frequency_label(frequency)),
        None => "Đã tắt tất cả lịch tổng kết.".to_string(),
    }
}

pub fn schedule_invalid_text() -> String {
    "Cú pháp chưa đúng. Ví dụ: /sched daily 20:00, /sched weekly 08:00, hoặc /sched off."
        .to_string()
}

pub fn delete_confirm_text(expense_count: u32) -> String {
    format!(
        "Bạn sắp xóa toàn bộ dữ liệu ({expense_count} khoản chi). Trả lời ok để xác nhận, no để hủy. Không thể hoàn tác."
    )
}

pub fn delete_accepted_text(expense_count: u32) -> String {
    format!("Đã nhận yêu cầu xóa ({expense_count} khoản chi). Dữ liệu sẽ được gỡ trong ít phút.")
}

pub fn delete_cancelled_text() -> String {
    "Đã hủy yêu cầu xóa dữ liệu.".to_string()
}

pub fn export_accepted_text() -> String {
    "Đã ghi nhận yêu cầu xuất dữ liệu. File sẽ được giao qua kênh quản trị viên, không gửi đường dẫn trong chat.".to_string()
}

fn schedule_frequency_label(frequency: &str) -> &str {
    match frequency {
        "daily" => "hàng ngày",
        "weekly" => "hàng tuần",
        "monthly" => "hàng tháng",
        _ => frequency,
    }
}

pub fn no_recent_text() -> String {
    "Chưa có giao dịch nào được ghi nhận. Gửi ảnh hóa đơn hoặc nhập: \"an sang 500k\".".to_string()
}

pub fn recent_text(lines: &[RecentExpenseLine]) -> String {
    if lines.is_empty() {
        return no_recent_text();
    }
    let mut body = String::from("Các khoản gần đây:\n");
    for line in lines {
        let amount = format_minor(line.amount_minor, &line.currency);
        let prefix = match &line.type_label {
            Some(t) if t != "Chi tiêu" => format!("[{t}] "),
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

pub fn default_category_display() -> &'static str {
    "Khác"
}

pub fn image_received_text() -> String {
    "Đã nhận ảnh hóa đơn. Tôi sẽ đọc và gửi lại để bạn xác nhận.".to_string()
}

pub fn daily_receipt_quota_text() -> String {
    "Bạn đã đạt giới hạn hóa đơn hôm nay. Mai gửi tiếp nhé.".to_string()
}

pub fn extraction_kill_switch_text() -> String {
    "Trích xuất hóa đơn đang tạm tắt.".to_string()
}

pub fn transaction_type_label(transaction_type: &str) -> &'static str {
    match transaction_type {
        "income" => "Thu nhập",
        "refund" => "Hoàn tiền",
        "transfer" => "Chuyển khoản",
        "adjustment" => "Điều chỉnh",
        _ => "Chi tiêu",
    }
}

pub fn default_type_label() -> &'static str {
    "Chi tiêu"
}
