use unicode_normalization::UnicodeNormalization;

/// Lowercase, strip Vietnamese diacritics, collapse whitespace.
pub fn fold(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let mut stripped: String = lowered
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    stripped = stripped.replace('đ', "d");
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Trim trailing punctuation used by command and manual-entry matchers.
pub fn trim_command_punctuation(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| matches!(c, '.' | ',' | '!' | '?' | '…' | ';' | ':'))
        .trim()
        .to_string()
}
