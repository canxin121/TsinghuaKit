use super::*;
fn payload(start: &str, end: &str) -> String {
    serde_json::json!({"data":{"list":[{"id":71,"day":"2026-09-19","startTime":{"date":start},"endTime":{"date":end}}]}}).to_string()
}
#[test]
fn backend_repair_lastmile_library_template_dates_are_time_of_day_not_booking_date() {
    for template in ["1970-01-01", "2000-01-01", "2026-09-19"] {
        let parsed = parse_day_segments(&payload(
            &format!("{template} 08:00:00.000000"),
            &format!("{template} 22:30:00.000000"),
        ))
        .unwrap();
        assert_eq!(parsed.segments[0].day, "2026-09-19");
        assert_eq!(parsed.segments[0].start_time, "08:00");
        assert_eq!(parsed.segments[0].end_time, "22:30");
        assert_eq!(parsed.segments[0].id, 71);
    }
}
#[test]
fn backend_repair_lastmile_library_invalid_clock_range_and_calendar_are_not_ignored() {
    for (start, end) in [
        ("1970-01-01 25:00:00", "1970-01-01 26:00:00"),
        ("1970-02-30 08:00:00", "1970-01-01 22:00:00"),
        ("1970-01-01 22:00:00", "1970-01-01 08:00:00"),
        ("1970-01-01 08:00:61", "1970-01-01 22:00:00"),
        ("1970-01-01 08:00:00Zgarbage", "1970-01-01 22:00:00"),
    ] {
        assert!(parse_day_segments(&payload(start, end)).is_err());
    }
}
#[test]
fn backend_repair_lastmile_library_matches_actual_reference_time_extraction_and_bound_day() {
    let fixture: Value =
        serde_json::from_str(include_str!("reference_lastmile_fixtures.json")).unwrap();
    let input = &fixture["library"];
    let parsed = parse_day_segments(&input["wire"].to_string()).unwrap();
    let segment = &parsed.segments[0];
    assert_eq!(segment.start_time, input["expected"]["startTime"]);
    assert_eq!(segment.end_time, input["expected"]["endTime"]);
    assert_eq!(segment.day, input["expected"]["day"]);
    let mut body = input["wire"].clone();
    body["data"]["list"][0]["day"] = serde_json::json!("2026-02-30");
    assert!(parse_day_segments(&body.to_string()).is_err());
}
