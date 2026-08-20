use chrono::NaiveDate;

use super::amount::parse_amount;
use super::categories::resolve_category_key;
use super::fold::{fold, trim_command_punctuation};
use super::locale::Locale;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    None,
    Start,
    Confirm,
    Discard,
    Help,
    Privacy,
    Today,
    Week,
    Month,
    Recent,
    Settings,
    Timezone,
    Schedule,
    Export,
    Delete,
    ManualEntry,
    EditAmount,
    EditMerchant,
    EditCategory,
    EditDate,
    EditType,
    ListCategories,
    RecategorizeLatest,
    SetLanguage,
}

#[derive(Debug, Clone)]
pub struct Intent {
    pub kind: IntentKind,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub amount_text: String,
    pub timezone: String,
    pub schedule_frequency: String,
    pub delivery_minute: i32,
    pub disable_all_schedules: bool,
    pub disable_schedule: bool,
    pub merchant: String,
    pub category_key: String,
    pub transaction_type: String,
    pub occurred_on: Option<NaiveDate>,
    pub locale: String,
}

pub fn parse_intent(text: &str, default_currency: &str) -> Intent {
    let trimmed = trim_command_punctuation(text);
    let f = fold(&trimmed);
    if f.is_empty() {
        return Intent::none();
    }

    if f.starts_with('/') {
        let body = f.trim_start_matches('/');
        if let Some(kind) = slash_command(body) {
            return Intent::of(kind);
        }
        if let Some(intent) = parse_parameterized_slash(&trimmed) {
            return intent;
        }
        return Intent::none();
    }

    if matches!(f.as_str(), "help" | "privacy" | "today" | "recent") {
        return Intent::of(match f.as_str() {
            "help" => IntentKind::Help,
            "privacy" => IntentKind::Privacy,
            "today" => IntentKind::Today,
            _ => IntentKind::Recent,
        });
    }

    if confirm_words(f.as_str()) {
        return Intent::of(IntentKind::Confirm);
    }
    if discard_words(f.as_str()) {
        return Intent::of(IntentKind::Discard);
    }

    if let Some(intent) = parse_edit_or_recat(&trimmed, default_currency) {
        return intent;
    }

    if has_word(&f, "lich su") || has_word(&f, "gan day") {
        return Intent::of(IntentKind::Recent);
    }

    for word in HELP_WORDS {
        if f == word || f.starts_with(&(word.to_string() + " ")) {
            return Intent::of(IntentKind::Help);
        }
    }

    if let Some(intent) = parse_manual_entry(&trimmed, default_currency) {
        return intent;
    }

    Intent::none()
}

pub fn is_explicit_slash_command(kind: IntentKind) -> bool {
    matches!(
        kind,
        IntentKind::Start
            | IntentKind::Help
            | IntentKind::Today
            | IntentKind::Week
            | IntentKind::Month
            | IntentKind::Recent
            | IntentKind::Privacy
            | IntentKind::Settings
            | IntentKind::Timezone
            | IntentKind::Schedule
            | IntentKind::Export
            | IntentKind::Delete
            | IntentKind::ListCategories
            | IntentKind::SetLanguage
            | IntentKind::RecategorizeLatest
    )
}

fn parse_parameterized_slash(text: &str) -> Option<Intent> {
    let trimmed = trim_command_punctuation(text);
    let raw = trimmed.trim_start_matches('/');
    let folded = fold(raw);
    let raw_fields: Vec<&str> = raw.split_whitespace().collect();
    let folded_fields: Vec<&str> = folded.split_whitespace().collect();
    if folded_fields.is_empty() {
        return None;
    }

    if matches_slash_name(folded_fields[0], &["tz", "timezone", "muigio"]) {
        return parse_timezone_intent(&raw_fields, &folded_fields);
    }
    if matches_slash_name(folded_fields[0], &["settings", "caidat", "cai dat"]) {
        return parse_settings_intent(&raw_fields, &folded_fields);
    }
    if matches_slash_name(folded_fields[0], &["sched", "tongket", "tong ket"]) {
        return parse_schedule_intent(&raw_fields, &folded_fields);
    }
    if matches_slash_name(folded_fields[0], &["lang", "language", "ngonngu"]) {
        return parse_language_intent(&folded_fields);
    }
    if matches_slash_name(folded_fields[0], &["recat", "phanloai", "category"]) {
        return parse_recat_slash(&raw_fields, &folded_fields);
    }
    None
}

fn matches_slash_name(folded: &str, names: &[&str]) -> bool {
    names.contains(&folded)
}

fn parse_language_intent(folded_fields: &[&str]) -> Option<Intent> {
    if folded_fields.len() == 1 {
        return Some(Intent::of(IntentKind::SetLanguage));
    }
    if folded_fields.len() != 2 {
        return None;
    }
    let locale = match folded_fields[1] {
        "vi" | "vn" | "vie" | "vietnamese" => Locale::Vi.as_account_value(),
        "en" | "eng" | "english" => Locale::En.as_account_value(),
        _ => return Some(Intent::of(IntentKind::SetLanguage)),
    };
    Some(Intent {
        kind: IntentKind::SetLanguage,
        locale: locale.to_string(),
        ..Intent::empty_fields()
    })
}

fn parse_recat_slash(raw_fields: &[&str], folded_fields: &[&str]) -> Option<Intent> {
    if folded_fields.len() < 2 {
        return Some(Intent::of(IntentKind::RecategorizeLatest));
    }
    let value = raw_fields[1..].join(" ");
    Some(Intent {
        kind: IntentKind::RecategorizeLatest,
        category_key: resolve_category_key(&value).unwrap_or("").to_string(),
        description: value,
        ..Intent::empty_fields()
    })
}

fn parse_timezone_intent(raw_fields: &[&str], folded_fields: &[&str]) -> Option<Intent> {
    let value_at = match folded_fields.len() {
        2 if matches_slash_name(folded_fields[0], &["tz", "timezone", "muigio"]) => 1,
        3 if folded_fields[0] == "mui" && folded_fields[1] == "gio" => 2,
        _ => return None,
    };
    if value_at >= raw_fields.len() {
        return None;
    }
    Some(Intent {
        kind: IntentKind::Timezone,
        timezone: raw_fields[value_at].to_string(),
        ..Intent::empty_fields()
    })
}

fn parse_settings_intent(raw_fields: &[&str], folded_fields: &[&str]) -> Option<Intent> {
    if folded_fields.is_empty() {
        return None;
    }
    if folded_fields.len() == 1 {
        return Some(Intent::of(IntentKind::Settings));
    }
    let value_at = if (folded_fields[0] == "tz"
        || folded_fields[0] == "timezone"
        || folded_fields[0] == "muigio")
        && folded_fields.len() == 2
    {
        1
    } else if folded_fields.len() == 3 && folded_fields[0] == "mui" && folded_fields[1] == "gio" {
        2
    } else {
        return None;
    };
    if value_at >= raw_fields.len() {
        return None;
    }
    Some(Intent {
        kind: IntentKind::Timezone,
        timezone: raw_fields[value_at].to_string(),
        ..Intent::empty_fields()
    })
}

fn parse_schedule_intent(raw_fields: &[&str], folded_fields: &[&str]) -> Option<Intent> {
    let (raw_fields, folded_fields) = if matches_slash_name(folded_fields[0], &["sched", "tongket"])
    {
        (&raw_fields[1..], &folded_fields[1..])
    } else {
        (raw_fields, folded_fields)
    };

    if folded_fields.is_empty() {
        return Some(Intent::of(IntentKind::Schedule));
    }
    if matches_slash_name(folded_fields[0], &["off", "tat"]) {
        let mut intent = Intent::of(IntentKind::Schedule);
        if folded_fields.len() == 1
            || (folded_fields.len() == 2 && matches_slash_name(folded_fields[1], &["all", "ca"]))
        {
            intent.disable_all_schedules = true;
            return Some(intent);
        }
        if folded_fields.len() != 2 {
            return None;
        }
        intent.schedule_frequency = parse_schedule_frequency(folded_fields[1])?;
        intent.disable_schedule = true;
        return Some(intent);
    }
    if folded_fields.len() != 2 {
        return None;
    }
    let frequency = parse_schedule_frequency(folded_fields[0])?;
    let minute = parse_delivery_minute(raw_fields[1])?;
    Some(Intent {
        kind: IntentKind::Schedule,
        schedule_frequency: frequency,
        delivery_minute: minute,
        ..Intent::empty_fields()
    })
}

fn parse_schedule_frequency(text: &str) -> Option<String> {
    match text {
        "daily" | "ngay" => Some("daily".to_string()),
        "weekly" | "tuan" => Some("weekly".to_string()),
        "monthly" | "thang" => Some("monthly".to_string()),
        _ => None,
    }
}

fn parse_delivery_minute(token: &str) -> Option<i32> {
    let parts: Vec<&str> = token.split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour = parts[0].parse::<i32>().ok()?;
    let minute = parts[1].parse::<i32>().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    Some(hour * 60 + minute)
}

fn slash_command(body: &str) -> Option<IntentKind> {
    let body = trim_command_punctuation(body.trim());
    SLASH_COMMANDS
        .iter()
        .find(|(name, _)| *name == body)
        .map(|(_, kind)| *kind)
}

fn parse_edit_or_recat(text: &str, default_currency: &str) -> Option<Intent> {
    let trimmed = trim_command_punctuation(text);
    let folded = fold(&trimmed);
    if folded.is_empty() {
        return None;
    }

    if let Some(rest) = strip_prefix_any(&folded, &["phan loai ", "doi danh muc ", "recat "]) {
        let key = resolve_category_key(rest).unwrap_or("").to_string();
        return Some(Intent {
            kind: IntentKind::RecategorizeLatest,
            category_key: key,
            description: rest.to_string(),
            ..Intent::empty_fields()
        });
    }

    let rest = if let Some(r) = folded.strip_prefix("sua so tien ") {
        r.trim()
    } else if let Some(r) = folded.strip_prefix("sai so tien ") {
        r.trim()
    } else if let Some(r) = folded.strip_prefix("sua ") {
        r.trim()
    } else if let Some(r) = folded.strip_prefix("edit ") {
        r.trim()
    } else if let Some(r) = folded.strip_prefix("fix ") {
        r.trim()
    } else {
        return None;
    };

    if rest.is_empty() || EDIT_PHRASES.iter().any(|phrase| folded == *phrase) {
        return None;
    }

    // Field-tagged edits first.
    if let Some(value) = strip_prefix_any(
        rest,
        &["cua hang ", "merchant ", "ten cua hang ", "shop ", "store "],
    ) {
        if value.is_empty() {
            return None;
        }
        // Recover original casing from raw text after the keyword.
        let merchant = extract_after_keywords(
            &trimmed,
            &[
                "cua hang",
                "cửa hàng",
                "merchant",
                "ten cua hang",
                "shop",
                "store",
            ],
        )
        .unwrap_or_else(|| value.to_string());
        return Some(Intent {
            kind: IntentKind::EditMerchant,
            merchant: merchant.trim().to_string(),
            ..Intent::empty_fields()
        });
    }

    if let Some(value) =
        strip_prefix_any(rest, &["danh muc ", "category ", "cat ", "loai chi tieu "])
    {
        let key = resolve_category_key(value).unwrap_or("").to_string();
        return Some(Intent {
            kind: IntentKind::EditCategory,
            category_key: key,
            description: value.to_string(),
            ..Intent::empty_fields()
        });
    }

    if let Some(value) = strip_prefix_any(rest, &["ngay ", "date ", "ngày "]) {
        let occurred_on = parse_date_token(value)?;
        return Some(Intent {
            kind: IntentKind::EditDate,
            occurred_on: Some(occurred_on),
            description: value.to_string(),
            ..Intent::empty_fields()
        });
    }

    if let Some(value) = strip_prefix_any(rest, &["loai ", "type ", "tx "]) {
        let transaction_type = parse_transaction_type(value)?;
        return Some(Intent {
            kind: IntentKind::EditType,
            transaction_type: transaction_type.to_string(),
            ..Intent::empty_fields()
        });
    }

    // Bare amount after edit/sua/fix.
    if let Ok((minor, currency)) = parse_amount(rest, default_currency) {
        return Some(Intent {
            kind: IntentKind::EditAmount,
            amount_minor: minor,
            currency,
            amount_text: rest.to_string(),
            ..Intent::empty_fields()
        });
    }

    None
}

fn strip_prefix_any<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for prefix in prefixes {
        if let Some(rest) = text.strip_prefix(prefix) {
            return Some(rest.trim());
        }
    }
    None
}

fn extract_after_keywords(raw: &str, keywords: &[&str]) -> Option<String> {
    let lowered = raw.to_lowercase();
    for keyword in keywords {
        if let Some(idx) = lowered.find(keyword) {
            let after = raw[idx + keyword.len()..].trim();
            if !after.is_empty() {
                return Some(after.to_string());
            }
        }
    }
    None
}

fn parse_date_token(token: &str) -> Option<NaiveDate> {
    let t = token.trim();
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return Some(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%d/%m/%Y") {
        return Some(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%d-%m-%Y") {
        return Some(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%d/%m/%y") {
        return Some(d);
    }
    None
}

fn parse_transaction_type(token: &str) -> Option<&'static str> {
    match fold(token).as_str() {
        "chi" | "chi tieu" | "expense" | "spend" | "out" => Some("expense"),
        "thu" | "thu nhap" | "income" | "in" => Some("income"),
        "hoan" | "hoan tien" | "refund" => Some("refund"),
        "chuyen" | "chuyen khoan" | "transfer" => Some("transfer"),
        "dieu chinh" | "adjustment" => Some("adjustment"),
        _ => None,
    }
}

fn parse_manual_entry(text: &str, default_currency: &str) -> Option<Intent> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    if fields.len() < 2 {
        return None;
    }

    let first_desc = text
        .trim()
        .strip_prefix(fields[0])
        .map(trim_command_punctuation)
        .unwrap_or_default();
    if let Some(intent) = manual_from_amount(fields[0], &first_desc, default_currency)
        && !command_like_description(&first_desc)
    {
        return Some(intent);
    }

    let last_token = trim_command_punctuation(fields[fields.len() - 1]);
    let desc = text
        .trim()
        .strip_suffix(fields[fields.len() - 1])
        .map(trim_command_punctuation)
        .unwrap_or_default();
    if command_like_description(&desc) {
        return None;
    }
    manual_from_amount(&last_token, &desc, default_currency)
}

fn manual_from_amount(token: &str, desc: &str, default_currency: &str) -> Option<Intent> {
    let desc = trim_command_punctuation(desc);
    if desc.is_empty() {
        return None;
    }
    let (minor, currency) = parse_amount(token, default_currency).ok()?;
    Some(Intent {
        kind: IntentKind::ManualEntry,
        amount_minor: minor,
        currency,
        description: desc,
        amount_text: token.to_string(),
        ..Intent::empty_fields()
    })
}

fn command_like_description(desc: &str) -> bool {
    let f = fold(desc);
    if f.is_empty() {
        return false;
    }
    if EDIT_PHRASES.iter().any(|p| *p == f) {
        return true;
    }
    if confirm_words(&f) || discard_words(&f) {
        return true;
    }
    matches!(
        f.as_str(),
        "help" | "today" | "recent" | "settings" | "sched"
    )
}

fn confirm_words(s: &str) -> bool {
    CONFIRM_WORDS.contains(&s)
}

fn discard_words(s: &str) -> bool {
    DISCARD_WORDS.contains(&s)
}

fn has_word(folded: &str, needle: &str) -> bool {
    format!(" {folded} ").contains(&format!(" {needle} "))
}

impl Intent {
    fn none() -> Self {
        Self::empty_fields_with(IntentKind::None)
    }

    fn of(kind: IntentKind) -> Self {
        Self::empty_fields_with(kind)
    }

    fn empty_fields() -> Self {
        Self::empty_fields_with(IntentKind::None)
    }

    fn empty_fields_with(kind: IntentKind) -> Self {
        Self {
            kind,
            amount_minor: 0,
            currency: String::new(),
            description: String::new(),
            amount_text: String::new(),
            timezone: String::new(),
            schedule_frequency: String::new(),
            delivery_minute: 0,
            disable_all_schedules: false,
            disable_schedule: false,
            merchant: String::new(),
            category_key: String::new(),
            transaction_type: String::new(),
            occurred_on: None,
            locale: String::new(),
        }
    }
}

const SLASH_COMMANDS: [(&str, IntentKind); 28] = [
    ("start", IntentKind::Start),
    ("batdau", IntentKind::Start),
    ("xinchao", IntentKind::Start),
    ("help", IntentKind::Help),
    ("trogiup", IntentKind::Help),
    ("privacy", IntentKind::Privacy),
    ("consent", IntentKind::Privacy),
    ("today", IntentKind::Today),
    ("homnay", IntentKind::Today),
    ("week", IntentKind::Week),
    ("tuan", IntentKind::Week),
    ("month", IntentKind::Month),
    ("thang", IntentKind::Month),
    ("recent", IntentKind::Recent),
    ("history", IntentKind::Recent),
    ("ganday", IntentKind::Recent),
    ("export", IntentKind::Export),
    ("xuatdulieu", IntentKind::Export),
    ("delete", IntentKind::Delete),
    ("xoadulieu", IntentKind::Delete),
    ("ok", IntentKind::Confirm),
    ("y", IntentKind::Confirm),
    ("no", IntentKind::Discard),
    ("n", IntentKind::Discard),
    ("categories", IntentKind::ListCategories),
    ("danhmuc", IntentKind::ListCategories),
    ("cats", IntentKind::ListCategories),
    ("recat", IntentKind::RecategorizeLatest),
];

const CONFIRM_WORDS: [&str; 10] = [
    "xac nhan", "dung", "dung roi", "dong y", "ok", "okay", "yes", "y", "co", "confirm",
];

const DISCARD_WORDS: [&str; 8] = [
    "bo qua", "huy", "khong", "skip", "thoi", "no", "n", "discard",
];

const EDIT_PHRASES: [&str; 4] = ["edit", "fix", "sua so tien", "sai so tien"];

const HELP_WORDS: [&str; 4] = ["help", "giup", "huong dan", "tro giup"];
