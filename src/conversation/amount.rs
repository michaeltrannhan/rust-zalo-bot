use super::money::currency_decimal_places;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountError {
    Empty,
    Negative,
    InvalidCharacter,
    NoDigits,
    TooLarge,
    NotWholeMinorUnit,
}

const MULTIPLIERS: [(&str, i64); 6] = [
    ("triệu", 1_000_000),
    ("trieu", 1_000_000),
    ("củ", 1_000_000),
    ("cu", 1_000_000),
    ("tr", 1_000_000),
    ("k", 1_000),
];

const CURRENCY_SYMBOLS: [(&str, &str); 3] = [("A$", "AUD"), ("₫", "VND"), ("$", "USD")];

/// Parse a chat amount token into minor units and ISO currency.
pub fn parse_amount(raw: &str, currency_hint: &str) -> Result<(i64, String), AmountError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AmountError::Empty);
    }
    if s.contains('-') {
        return Err(AmountError::Negative);
    }

    let (currency, body) = detect_currency(s, currency_hint);

    if let Some(compound) = try_parse_compound(&body, &currency) {
        return compound;
    }

    parse_single_amount(&body, &currency)
}

fn try_parse_compound(body: &str, currency: &str) -> Option<Result<(i64, String), AmountError>> {
    let lower = body.to_lowercase();
    for (mult_str, _) in MULTIPLIERS {
        if let Some(pos) = lower.find(mult_str) {
            if pos == 0 {
                continue;
            }
            let before = lower[..pos].trim();
            let after = lower[pos + mult_str.len()..].trim();

            if before.is_empty() || after.is_empty() {
                continue;
            }

            let before_is_digits = before
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ',');
            if !before_is_digits {
                continue;
            }

            let after_is_pure_digits = after.chars().all(|c| c.is_ascii_digit());
            if after_is_pure_digits {
                let rewritten = format!("{}.{}{}", before, after, mult_str);
                return Some(parse_single_amount(&rewritten, currency));
            }

            let before_token = format!("{}{}", before, mult_str);
            if let Ok((val_before, _)) = parse_single_amount(&before_token, currency)
                && let Ok((val_after, _)) = parse_amount(after, currency)
            {
                let total = val_before.checked_add(val_after);
                return Some(
                    total
                        .map(|v| (v, currency.to_string()))
                        .ok_or(AmountError::TooLarge),
                );
            }
        }
    }
    None
}

fn parse_single_amount(body: &str, currency: &str) -> Result<(i64, String), AmountError> {
    let mut factor: i64 = 1;
    let mut body = body.trim().to_string();

    let lower = body.to_lowercase();
    for (suffix, mult) in MULTIPLIERS {
        if lower.ends_with(suffix) {
            factor = mult;
            body = body[..body.len() - suffix.len()].trim().to_string();
            break;
        }
    }

    let mut digits_only = String::new();
    for ch in body.chars() {
        match ch {
            '0'..='9' | '.' | ',' => digits_only.push(ch),
            ' ' => {}
            _ => return Err(AmountError::InvalidCharacter),
        }
    }
    if digits_only.is_empty() {
        return Err(AmountError::NoDigits);
    }

    if currency_decimal_places(currency) == 0 && factor == 1 {
        return finish_amount(&strip_separators(&digits_only), currency, factor);
    }

    let last = digits_only
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '.' || *c == ',')
        .map(|(i, _)| i)
        .last();

    let mut decimal_at: Option<usize> = None;
    if let Some(last_idx) = last {
        let sep = digits_only.as_bytes()[last_idx];
        let frac = strip_separators(&digits_only[last_idx + 1..]);
        if currency_decimal_places(currency) == 0 {
            if !frac.is_empty() && frac.len() != 3 {
                decimal_at = Some(last_idx);
            }
        } else if sep == b'.' && frac.len() == 2 {
            decimal_at = Some(last_idx);
        }
    }

    if decimal_at.is_none() {
        return finish_amount(&strip_separators(&digits_only), currency, factor);
    }

    let at = decimal_at.unwrap();
    let int_digits = strip_separators(&digits_only[..at]);
    let frac_digits = strip_separators(&digits_only[at + 1..]);
    if frac_digits.len() > 4 {
        return Err(AmountError::TooLarge);
    }
    if int_digits.len() + frac_digits.len() > 15 {
        return Err(AmountError::TooLarge);
    }

    let combined = parse_digits(&(int_digits + &frac_digits))?;
    let n = frac_digits.len();

    if currency_decimal_places(currency) == 0 {
        let scale = 10_i64.pow(n as u32);
        if factor > 1 && combined > i64::MAX / factor {
            return Err(AmountError::TooLarge);
        }
        let value = combined * factor;
        if value % scale != 0 {
            return Err(AmountError::NotWholeMinorUnit);
        }
        return Ok((value / scale, currency.to_string()));
    }

    let places = currency_decimal_places(currency) as usize;
    if n > places {
        return Err(AmountError::NotWholeMinorUnit);
    }
    let mut minor = combined;
    for _ in n..places {
        if minor > i64::MAX / 10 {
            return Err(AmountError::TooLarge);
        }
        minor *= 10;
    }
    if factor > 1 && minor > i64::MAX / factor {
        return Err(AmountError::TooLarge);
    }
    Ok((minor * factor, currency.to_string()))
}

fn finish_amount(digits: &str, currency: &str, factor: i64) -> Result<(i64, String), AmountError> {
    if digits.is_empty() {
        return Err(AmountError::NoDigits);
    }
    if digits.len() > 15 {
        return Err(AmountError::TooLarge);
    }
    let mut value = parse_digits(digits)?;
    if currency_decimal_places(currency) == 2 {
        if value > i64::MAX / 100 {
            return Err(AmountError::TooLarge);
        }
        value *= 100;
    }
    if factor > 1 && value > i64::MAX / factor {
        return Err(AmountError::TooLarge);
    }
    Ok((value * factor, currency.to_uppercase()))
}

fn parse_digits(digits: &str) -> Result<i64, AmountError> {
    digits.parse::<i64>().map_err(|_| AmountError::TooLarge)
}

fn strip_separators(s: &str) -> String {
    s.chars().filter(|c| *c >= '0' && *c <= '9').collect()
}

fn detect_currency(s: &str, hint: &str) -> (String, String) {
    for (token, currency) in CURRENCY_SYMBOLS {
        if let Some(rest) = s.strip_prefix(token) {
            return (currency.to_string(), rest.trim().to_string());
        }
        if let Some(rest) = s.strip_suffix(token) {
            return (currency.to_string(), rest.trim().to_string());
        }
    }
    let currency = if hint.is_empty() {
        "VND".to_string()
    } else {
        hint.to_uppercase()
    };
    (currency, s.to_string())
}
