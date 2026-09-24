//! Pure wire-format fixtures. No account, HTTP or calendar data is accessed.
use super::*;
use serde_json::json;

#[test]
fn backend_repair_time_contract_compact_hours_minutes_are_not_variable_width_seconds() {
    for (text, hour, minute) in [
        ("0800", 8, 0),
        ("0935", 9, 35),
        ("1330", 13, 30),
        ("1930", 19, 30),
    ] {
        assert_eq!(parse_time(text), NaiveTime::from_hms_opt(hour, minute, 0));
    }
    assert_eq!(parse_time("133045"), NaiveTime::from_hms_opt(13, 30, 45));
    assert_eq!(parse_time("13：30"), NaiveTime::from_hms_opt(13, 30, 0));
    assert!(parse_time("1360").is_none());
    assert!(parse_time("2500").is_none());
}

#[test]
fn backend_repair_time_contract_end_time_honors_explicit_next_day() {
    let value = parse_event(&json!({"nr":"fixture", "nq":"2026-09-21", "start":"23:00", "end":"01:00", "endDate":"2026-09-22"}), 0).unwrap();
    assert_eq!((value.ends_at - value.starts_at).num_hours(), 2);
    assert_eq!(
        value.ends_at.date(),
        NaiveDate::from_ymd_opt(2026, 9, 22).unwrap()
    );
}

#[test]
fn backend_repair_time_contract_full_datetimes_in_time_fields_preserve_offsets() {
    let value = parse_event(&json!({"nr":"fixture", "nq":"2026-09-21", "kssj":"2026-09-21T00:00:00Z", "jssj":"2026-09-21T01:35:00Z"}),0).unwrap();
    assert_eq!(
        value.start_time(),
        NaiveTime::from_hms_opt(8, 0, 0).unwrap()
    );
    assert_eq!(value.end_time(), NaiveTime::from_hms_opt(9, 35, 0).unwrap());
}

#[test]
fn backend_repair_time_contract_conflicting_or_invalid_explicit_dates_fail_closed() {
    for date in ["invalid", "2026-02-30", "2026-09-22"] {
        assert!(parse_event(&json!({"nr":"fixture", "nq":date, "start":"2026-09-21T08:00:00+08:00", "end":"2026-09-21T09:00:00+08:00"}),0).is_err());
    }
    assert!(parse_event(&json!({"nr":"fixture", "nq":"2026-09-21", "start":"08:00", "end":"09:00", "endDate":"invalid"}),0).is_err());
    assert!(
        parse_event(
            &json!({"nr":"fixture", "nq":"2026-09-21", "start":"09:00", "end":"08:00"}),
            0
        )
        .is_err()
    );
}
