//! DST and timezone schedule instant unit tests (no database).

use chrono::{Offset, TimeZone, Timelike, Utc};
use chrono_tz::America::New_York;
use zl_expense::schedule::{Frequency, interactive_period, latest_delivery, next_delivery};

#[test]
fn hcm_daily_later_today() {
    let now = Utc.with_ymd_and_hms(2026, 7, 23, 3, 0, 0).unwrap();
    let next = next_delivery(now, "Asia/Ho_Chi_Minh", Frequency::Daily, 20 * 60).expect("next");
    let want = Utc.with_ymd_and_hms(2026, 7, 23, 13, 0, 0).unwrap();
    assert_eq!(next, want);
}

#[test]
fn hcm_daily_tomorrow_after_delivery() {
    let now = Utc.with_ymd_and_hms(2026, 7, 23, 13, 0, 0).unwrap();
    let next = next_delivery(now, "Asia/Ho_Chi_Minh", Frequency::Daily, 20 * 60).expect("next");
    let want = Utc.with_ymd_and_hms(2026, 7, 24, 13, 0, 0).unwrap();
    assert_eq!(next, want);
}

#[test]
fn hcm_weekly_next_monday() {
    let now = Utc.with_ymd_and_hms(2026, 7, 23, 3, 0, 0).unwrap();
    let next =
        next_delivery(now, "Asia/Ho_Chi_Minh", Frequency::Weekly, 8 * 60 + 30).expect("next");
    let want = Utc.with_ymd_and_hms(2026, 7, 27, 1, 30, 0).unwrap();
    assert_eq!(next, want);
}

#[test]
fn new_york_spring_forward_daily_0230() {
    // 2026-03-08 spring-forward: 02:30 local does not exist; use next valid after gap.
    let now = Utc.with_ymd_and_hms(2026, 3, 8, 6, 0, 0).unwrap();
    let next = next_delivery(now, "America/New_York", Frequency::Daily, 2 * 60 + 30).expect("next");
    let local = next.with_timezone(&New_York);
    assert_eq!(local.hour(), 3);
    assert_eq!(local.minute(), 30);
}

#[test]
fn new_york_fall_back_daily_0230_uses_earlier_offset_when_ambiguous() {
    // 2026-11-01 fall-back: 01:30 happens twice; earlier offset is EDT (UTC-4).
    let now = Utc.with_ymd_and_hms(2026, 11, 1, 4, 0, 0).unwrap();
    let latest = latest_delivery(now, "America/New_York", Frequency::Daily, 90).expect("latest");
    let local = latest.with_timezone(&New_York);
    assert_eq!(local.hour(), 1);
    assert_eq!(local.minute(), 30);
    assert_eq!(local.offset().fix().local_minus_utc(), -4 * 3600);
}

#[test]
fn invalid_timezone_is_rejected() {
    let now = Utc::now();
    assert!(next_delivery(now, "Not/A_Zone", Frequency::Daily, 0).is_err());
}

#[test]
fn new_york_spring_forward_today_period_uses_calendar_midnights() {
    // 2026-03-08 12:00 EDT is after the gap; today must be [00:00 EST, next 00:00 EDT).
    let now = Utc.with_ymd_and_hms(2026, 3, 8, 16, 0, 0).unwrap();
    let (period, _) =
        interactive_period(Frequency::Daily, now, "America/New_York").expect("period");
    let start = period.start.with_timezone(&New_York);
    let end = period.end.with_timezone(&New_York);
    assert_eq!(start.hour(), 0);
    assert_eq!(start.minute(), 0);
    assert_eq!(start.offset().fix().local_minus_utc(), -5 * 3600);
    assert_eq!(end.hour(), 0);
    assert_eq!(end.minute(), 0);
    assert_eq!(end.offset().fix().local_minus_utc(), -4 * 3600);
    assert_eq!((period.end - period.start).num_hours(), 23);
}
