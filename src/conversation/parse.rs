use super::amount::parse_amount;
use super::fold::{fold, trim_command_punctuation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    None,
    Start,
    Confirm,
    Discard,
    Help,
    Today,
    Recent,
    ManualEntry,
}

#[derive(Debug, Clone)]
pub struct Intent {
    pub kind: IntentKind,
    pub amount_minor: i64,
    pub currency: String,
    pub description: String,
    pub amount_text: String,
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
        return Intent::none();
    }

    if matches!(f.as_str(), "help" | "today" | "recent") {
        return Intent::of(match f.as_str() {
            "help" => IntentKind::Help,
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
        IntentKind::Start | IntentKind::Help | IntentKind::Today | IntentKind::Recent
    )
}

fn slash_command(body: &str) -> Option<IntentKind> {
    let body = trim_command_punctuation(body.trim());
    SLASH_COMMANDS
        .iter()
        .find(|(name, _)| *name == body)
        .map(|(_, kind)| *kind)
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
        Self {
            kind: IntentKind::None,
            amount_minor: 0,
            currency: String::new(),
            description: String::new(),
            amount_text: String::new(),
        }
    }

    fn of(kind: IntentKind) -> Self {
        Self {
            kind,
            amount_minor: 0,
            currency: String::new(),
            description: String::new(),
            amount_text: String::new(),
        }
    }
}

const SLASH_COMMANDS: [(&str, IntentKind); 14] = [
    ("start", IntentKind::Start),
    ("batdau", IntentKind::Start),
    ("xinchao", IntentKind::Start),
    ("help", IntentKind::Help),
    ("trogiup", IntentKind::Help),
    ("today", IntentKind::Today),
    ("homnay", IntentKind::Today),
    ("recent", IntentKind::Recent),
    ("history", IntentKind::Recent),
    ("ganday", IntentKind::Recent),
    ("ok", IntentKind::Confirm),
    ("y", IntentKind::Confirm),
    ("no", IntentKind::Discard),
    ("n", IntentKind::Discard),
];

const CONFIRM_WORDS: [&str; 10] = [
    "xac nhan", "dung", "dung roi", "dong y", "ok", "okay", "yes", "y", "co", "confirm",
];

const DISCARD_WORDS: [&str; 8] = [
    "bo qua", "huy", "khong", "skip", "thoi", "no", "n", "discard",
];

const EDIT_PHRASES: [&str; 4] = ["edit", "fix", "sua so tien", "sai so tien"];

const HELP_WORDS: [&str; 4] = ["help", "giup", "huong dan", "tro giup"];
