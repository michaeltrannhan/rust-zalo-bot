//! Redaction helpers for provider errors and logs.

use std::borrow::Cow;

pub fn redact_value<'a>(
    value: &'a str,
    token: &str,
    chat_id: &str,
    text: &str,
    url: &str,
) -> Cow<'a, str> {
    if value.is_empty() {
        return Cow::Borrowed(value);
    }
    let mut out = Cow::Borrowed(value);
    for needle in [token, chat_id, text, url] {
        if needle.is_empty() {
            continue;
        }
        if out.contains(needle) {
            let owned = out.into_owned().replace(needle, "[REDACTED]");
            out = Cow::Owned(owned);
        }
    }
    if !token.is_empty() {
        for escaped in [url_escape(token), path_escape(token)] {
            if out.contains(escaped.as_str()) {
                let owned = out.into_owned().replace(&escaped, "[REDACTED]");
                out = Cow::Owned(owned);
            }
        }
    }
    if !url.is_empty() {
        for escaped in [url_escape(url), path_escape(url)] {
            if out.contains(escaped.as_str()) {
                let owned = out.into_owned().replace(&escaped, "[REDACTED]");
                out = Cow::Owned(owned);
            }
        }
    }
    out
}

fn url_escape(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            '?' => "%3F".to_string(),
            '#' => "%23".to_string(),
            '[' => "%5B".to_string(),
            ']' => "%5D".to_string(),
            '@' => "%40".to_string(),
            '!' => "%21".to_string(),
            '$' => "%24".to_string(),
            '&' => "%26".to_string(),
            '\'' => "%27".to_string(),
            '(' => "%28".to_string(),
            ')' => "%29".to_string(),
            '*' => "%2A".to_string(),
            '+' => "%2B".to_string(),
            ',' => "%2C".to_string(),
            ';' => "%3B".to_string(),
            '=' => "%3D".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            _ if ch.is_ascii() => ch.to_string(),
            _ => ch.encode_utf8(&mut [0; 4]).to_string(),
        })
        .collect()
}

fn path_escape(value: &str) -> String {
    value.replace(':', "%3A")
}
