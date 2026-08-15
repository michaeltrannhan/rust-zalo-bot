//! Chat-facing money formatting (vi-VN conventions).

pub fn currency_decimal_places(currency: &str) -> u32 {
    match currency.to_uppercase().as_str() {
        "VND" | "JPY" | "KRW" => 0,
        _ => 2,
    }
}

pub fn format_minor(amount: i64, currency: &str) -> String {
    let cur = currency.to_uppercase();
    let neg = amount < 0;
    let value = if neg { -amount } else { amount };
    let places = currency_decimal_places(&cur);

    let (int_part, frac_part) = if places == 0 {
        (format!("{value}"), String::new())
    } else {
        let scale = 10_i64.pow(places);
        let int = value / scale;
        let frac = value % scale;
        (
            format!("{int}"),
            format!(",{:0width$}", frac, width = places as usize),
        )
    };

    let grouped = group_digits(&int_part);
    let sign = if neg { "-" } else { "" };
    let symbol = currency_symbol(&cur);
    if symbol.is_empty() {
        format!("{sign}{grouped}{frac_part} {cur}")
    } else {
        format!("{sign}{grouped}{frac_part} {symbol}")
    }
}

fn group_digits(int_part: &str) -> String {
    let digits = int_part.as_bytes();
    let mut grouped = Vec::new();
    for (i, d) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(b'.');
        }
        grouped.push(*d);
    }
    String::from_utf8(grouped).unwrap_or_else(|_| int_part.to_string())
}

fn currency_symbol(cur: &str) -> &'static str {
    match cur {
        "VND" => "₫",
        "AUD" => "A$",
        "USD" => "$",
        _ => "",
    }
}
