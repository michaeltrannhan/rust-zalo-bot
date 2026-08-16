//! Timezone-aware schedule instants and spending periods.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

use super::types::{Frequency, Period};

/// Schedule calculation or validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    InvalidTimezone,
    InvalidDeliveryMinute,
    UnsupportedFrequency,
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTimezone => f.write_str("invalid timezone"),
            Self::InvalidDeliveryMinute => f.write_str("invalid delivery minute"),
            Self::UnsupportedFrequency => f.write_str("unsupported frequency"),
        }
    }
}

impl std::error::Error for ScheduleError {}

/// Parse an IANA timezone name; invalid names are rejected (no silent UTC fallback).
pub fn parse_timezone(value: &str) -> Result<Tz, ScheduleError> {
    if value.eq_ignore_ascii_case("local") {
        return Err(ScheduleError::InvalidTimezone);
    }
    value
        .parse::<Tz>()
        .map_err(|_| ScheduleError::InvalidTimezone)
}

/// Validate delivery minute is within `[0, 1439]`.
pub fn validate_delivery_minute(minute: i32) -> Result<(), ScheduleError> {
    if !(0..24 * 60).contains(&minute) {
        return Err(ScheduleError::InvalidDeliveryMinute);
    }
    Ok(())
}

/// First scheduled UTC instant strictly after `now`.
pub fn next_delivery(
    now: DateTime<Utc>,
    timezone: &str,
    frequency: Frequency,
    delivery_minute: i32,
) -> Result<DateTime<Utc>, ScheduleError> {
    validate_delivery_minute(delivery_minute)?;
    let tz = parse_timezone(timezone)?;
    let local_now = now.with_timezone(&tz);
    let (year, month, day) = (local_now.year(), local_now.month(), local_now.day());
    let mut candidate = local_delivery_datetime(year, month, day, delivery_minute, tz)?;

    match frequency {
        Frequency::Daily => {
            if !candidate.with_timezone(&Utc).gt(&now) {
                let next_day = candidate.date_naive() + Duration::days(1);
                candidate = local_delivery_datetime(
                    next_day.year(),
                    next_day.month(),
                    next_day.day(),
                    delivery_minute,
                    tz,
                )?;
            }
        }
        Frequency::Weekly => {
            let days_until_monday = (chrono::Weekday::Mon.num_days_from_monday() as i32
                - candidate.weekday().num_days_from_monday() as i32
                + 7)
                % 7;
            if days_until_monday > 0 {
                let shifted = candidate.date_naive() + Duration::days(days_until_monday as i64);
                candidate = local_delivery_datetime(
                    shifted.year(),
                    shifted.month(),
                    shifted.day(),
                    delivery_minute,
                    tz,
                )?;
            }
            if !candidate.with_timezone(&Utc).gt(&now) {
                let shifted = candidate.date_naive() + Duration::days(7);
                candidate = local_delivery_datetime(
                    shifted.year(),
                    shifted.month(),
                    shifted.day(),
                    delivery_minute,
                    tz,
                )?;
            }
        }
        Frequency::Monthly => {
            candidate = local_delivery_datetime(year, month, 1, delivery_minute, tz)?;
            if !candidate.with_timezone(&Utc).gt(&now) {
                let next_month = if month == 12 {
                    NaiveDate::from_ymd_opt(year + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(year, month + 1, 1)
                }
                .expect("valid month boundary");
                candidate = local_delivery_datetime(
                    next_month.year(),
                    next_month.month(),
                    next_month.day(),
                    delivery_minute,
                    tz,
                )?;
            }
        }
    }

    Ok(candidate.with_timezone(&Utc))
}

/// Most recent scheduled UTC instant at or before `now`.
pub fn latest_delivery(
    now: DateTime<Utc>,
    timezone: &str,
    frequency: Frequency,
    delivery_minute: i32,
) -> Result<DateTime<Utc>, ScheduleError> {
    validate_delivery_minute(delivery_minute)?;
    let tz = parse_timezone(timezone)?;
    let local_now = now.with_timezone(&tz);
    let (year, month, day) = (local_now.year(), local_now.month(), local_now.day());
    let mut candidate = local_delivery_datetime(year, month, day, delivery_minute, tz)?;

    match frequency {
        Frequency::Daily => {
            if candidate.with_timezone(&Utc).gt(&now) {
                let prev = candidate.date_naive() - Duration::days(1);
                candidate = local_delivery_datetime(
                    prev.year(),
                    prev.month(),
                    prev.day(),
                    delivery_minute,
                    tz,
                )?;
            }
        }
        Frequency::Weekly => {
            let days_since_monday = candidate.weekday().num_days_from_monday() as i64;
            let monday = candidate.date_naive() - Duration::days(days_since_monday);
            candidate = local_delivery_datetime(
                monday.year(),
                monday.month(),
                monday.day(),
                delivery_minute,
                tz,
            )?;
            if candidate.with_timezone(&Utc).gt(&now) {
                let prev_monday = candidate.date_naive() - Duration::days(7);
                candidate = local_delivery_datetime(
                    prev_monday.year(),
                    prev_monday.month(),
                    prev_monday.day(),
                    delivery_minute,
                    tz,
                )?;
            }
        }
        Frequency::Monthly => {
            candidate = local_delivery_datetime(year, month, 1, delivery_minute, tz)?;
            if candidate.with_timezone(&Utc).gt(&now) {
                let prev_month = if month == 1 {
                    NaiveDate::from_ymd_opt(year - 1, 12, 1)
                } else {
                    NaiveDate::from_ymd_opt(year, month - 1, 1)
                }
                .expect("valid month boundary");
                candidate = local_delivery_datetime(
                    prev_month.year(),
                    prev_month.month(),
                    prev_month.day(),
                    delivery_minute,
                    tz,
                )?;
            }
        }
    }

    Ok(candidate.with_timezone(&Utc))
}

/// Spending period and Vietnamese label for interactive commands.
pub fn interactive_period(
    frequency: Frequency,
    now: DateTime<Utc>,
    timezone: &str,
) -> Result<(Period, &'static str), ScheduleError> {
    let tz = parse_timezone(timezone)?;
    match frequency {
        Frequency::Daily => Ok((today(now, tz), "Hôm nay")),
        Frequency::Weekly => Ok((this_week(now, tz), "Tuần này")),
        Frequency::Monthly => Ok((this_month(now, tz), "Tháng này")),
    }
}

/// Spending period and Vietnamese label for scheduled emission anchored at `scheduled_for`.
pub fn scheduled_period(
    frequency: Frequency,
    scheduled_for: DateTime<Utc>,
    timezone: &str,
) -> Result<(Period, &'static str), ScheduleError> {
    let tz = parse_timezone(timezone)?;
    let anchor = scheduled_for.with_timezone(&tz);
    match frequency {
        Frequency::Daily => Ok((yesterday(anchor, tz), "Hôm qua")),
        Frequency::Weekly => Ok((last_week(anchor, tz), "Tuần trước")),
        Frequency::Monthly => Ok((last_month(anchor, tz), "Tháng trước")),
    }
}

/// Build local delivery time for one calendar day, handling DST gaps and folds.
///
/// If the local wall time does not exist (spring-forward gap), use the next valid
/// local time after the gap preserving minute-of-hour when possible.
/// If the time is ambiguous (fall-back fold), use the earlier offset.
fn local_delivery_datetime(
    year: i32,
    month: u32,
    day: u32,
    delivery_minute: i32,
    tz: Tz,
) -> Result<DateTime<Tz>, ScheduleError> {
    validate_delivery_minute(delivery_minute)?;
    let hour = delivery_minute / 60;
    let minute = delivery_minute % 60;
    let date = NaiveDate::from_ymd_opt(year, month, day).expect("valid date");
    let naive = date
        .and_hms_opt(hour as u32, minute as u32, 0)
        .expect("valid clock time");
    resolve_local_datetime(naive, tz)
}

fn resolve_local_datetime(naive: NaiveDateTime, tz: Tz) -> Result<DateTime<Tz>, ScheduleError> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Ok(dt),
        LocalResult::Ambiguous(earlier, _) => Ok(earlier),
        LocalResult::None => {
            let shifted = naive + Duration::hours(1);
            match tz.from_local_datetime(&shifted) {
                LocalResult::Single(dt) => Ok(dt),
                LocalResult::Ambiguous(earlier, _) => Ok(earlier),
                LocalResult::None => scan_forward_local(naive, tz),
            }
        }
    }
}

fn scan_forward_local(naive: NaiveDateTime, tz: Tz) -> Result<DateTime<Tz>, ScheduleError> {
    for step in 1..=180 {
        let candidate = naive + Duration::minutes(step);
        match tz.from_local_datetime(&candidate) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => return Ok(dt),
            LocalResult::None => {}
        }
    }
    Err(ScheduleError::InvalidTimezone)
}

fn local_date_start(date: NaiveDate, tz: Tz) -> DateTime<Tz> {
    resolve_local_datetime(date.and_hms_opt(0, 0, 0).expect("midnight"), tz)
        .expect("local midnight")
}

fn today(now: DateTime<Utc>, tz: Tz) -> Period {
    let start_date = now.with_timezone(&tz).date_naive();
    let start = local_date_start(start_date, tz);
    let end = local_date_start(start_date + Duration::days(1), tz);
    Period {
        start: start.with_timezone(&Utc),
        end: end.with_timezone(&Utc),
    }
}

fn yesterday(anchor: DateTime<Tz>, tz: Tz) -> Period {
    let end_date = anchor.date_naive();
    let start = local_date_start(end_date - Duration::days(1), tz);
    let end = local_date_start(end_date, tz);
    Period {
        start: start.with_timezone(&Utc),
        end: end.with_timezone(&Utc),
    }
}

fn week_start(anchor: DateTime<Tz>) -> DateTime<Tz> {
    let date = anchor.date_naive();
    let back = date.weekday().num_days_from_monday() as i64;
    local_date_start(date - Duration::days(back), anchor.timezone())
}

fn this_week(now: DateTime<Utc>, tz: Tz) -> Period {
    let anchor = now.with_timezone(&tz);
    let start = week_start(anchor);
    let full_end = local_date_start(start.date_naive() + Duration::days(7), tz);
    let tomorrow = local_date_start(anchor.date_naive() + Duration::days(1), tz);
    let end = if full_end > tomorrow {
        tomorrow
    } else {
        full_end
    };
    Period {
        start: start.with_timezone(&Utc),
        end: end.with_timezone(&Utc),
    }
}

fn last_week(anchor: DateTime<Tz>, tz: Tz) -> Period {
    let this_start = week_start(anchor);
    let start = local_date_start(this_start.date_naive() - Duration::days(7), tz);
    Period {
        start: start.with_timezone(&Utc),
        end: this_start.with_timezone(&Utc),
    }
}

fn month_start(anchor: DateTime<Tz>) -> DateTime<Tz> {
    let date = anchor.date_naive();
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("month start");
    local_date_start(first, anchor.timezone())
}

fn this_month(now: DateTime<Utc>, tz: Tz) -> Period {
    let anchor = now.with_timezone(&tz);
    let start = month_start(anchor);
    let full_end = if start.month() == 12 {
        resolve_local_datetime(
            NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
                .expect("year boundary")
                .and_hms_opt(0, 0, 0)
                .expect("midnight"),
            tz,
        )
        .expect("next month")
    } else {
        resolve_local_datetime(
            NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
                .expect("month boundary")
                .and_hms_opt(0, 0, 0)
                .expect("midnight"),
            tz,
        )
        .expect("next month")
    };
    let tomorrow = local_date_start(anchor.date_naive() + Duration::days(1), tz);
    let end = if full_end > tomorrow {
        tomorrow
    } else {
        full_end
    };
    Period {
        start: start.with_timezone(&Utc),
        end: end.with_timezone(&Utc),
    }
}

fn last_month(anchor: DateTime<Tz>, tz: Tz) -> Period {
    let this_start = month_start(anchor);
    let prev_month_date = if this_start.month() == 1 {
        NaiveDate::from_ymd_opt(this_start.year() - 1, 12, 1)
    } else {
        NaiveDate::from_ymd_opt(this_start.year(), this_start.month() - 1, 1)
    }
    .expect("previous month");
    let start = resolve_local_datetime(prev_month_date.and_hms_opt(0, 0, 0).expect("midnight"), tz)
        .expect("previous month start");
    Period {
        start: start.with_timezone(&Utc),
        end: this_start.with_timezone(&Utc),
    }
}
