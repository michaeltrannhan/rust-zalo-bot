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

pub fn default_type_label() -> &'static str {
    "Chi tiêu"
}
