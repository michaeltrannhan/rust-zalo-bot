//! Account reply locale (VN / EN).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Vi,
    En,
}

impl Locale {
    pub fn parse(value: &str) -> Self {
        let folded = value.trim().to_ascii_lowercase();
        if folded.starts_with("en") {
            Self::En
        } else {
            Self::Vi
        }
    }

    pub fn as_account_value(self) -> &'static str {
        match self {
            Self::Vi => "vi-VN",
            Self::En => "en-US",
        }
    }

    pub fn as_short(self) -> &'static str {
        match self {
            Self::Vi => "vi",
            Self::En => "en",
        }
    }
}

/// Pick VN or EN category display for the account locale.
pub fn category_display_for(locale: Locale, key: &str, name_vi: &str, name_en: &str) -> String {
    let preferred = match locale {
        Locale::Vi => name_vi,
        Locale::En => name_en,
    };
    if preferred.trim().is_empty() {
        key.to_string()
    } else {
        preferred.to_string()
    }
}
