//! Response identity/date contracts. All network traffic is synthetic loopback.
use crate::{
    campus_card_adapter::{CampusCardAdapterConfig, CampusCardClient},
    campus_card_read::{
        CampusCardTransactionQuery, CampusCardTransactionType, parse_card_transactions_response,
    },
    library_read::{self, LibraryReadAdapter, LibrarySocketState, LibrarySocketStatusAdapter},
    reference_test_support::{FixtureServer, Reply},
    registrar_client::{RegistrarClient, RegistrarClientConfig, RegistrarClientError},
    transport::CampusHttpTransport,
};
use serde_json::{Value, json};

#[test]
fn backend_repair_binding_card_local_date_conversion_preserves_instant_and_rejects_controls() {
    use crate::campus_card_read::transaction_local_date;
    for (raw, expected) in [
        ("2026-09-17T16:00:00Z", "2026-09-18"),
        ("2026-09-18T23:00:00-07:00", "2026-09-19"),
        ("2028-02-29 12:34:56.123456", "2028-02-29"),
        ("2026-09-18", "2026-09-18"),
    ] {
        assert_eq!(transaction_local_date(raw).unwrap().to_string(), expected);
    }
    for raw in [
        "2026-09-18\n12:00:00",
        "2026-09-18 12:00:00\0",
        "2026-02-29 12:00:00",
    ] {
        assert!(transaction_local_date(raw).is_none());
    }
}

#[test]
fn backend_repair_binding_library_clock_only_segments_remain_supported() {
    let value = json!({"id":1,"day":"2026-09-18","startTime":"08:00","endTime":"22:00:00"});
    let parsed = library_read::parse_day_segments(&list(vec![value])).unwrap();
    assert_eq!(parsed.segments[0].start_time, "08:00");
    assert_eq!(parsed.segments[0].end_time, "22:00");
}

#[tokio::test]
async fn backend_repair_binding_exam_location_header_without_redirect_is_not_auth_failure() {
    let target = FixtureServer::new(vec![]);
    let location = format!(
        "Location: {}do/off/ui/auth/login/form/fixture/0\r\n",
        target.base()
    );
    let s = FixtureServer::new(vec![Reply {
        status: 304,
        headers: location,
        body: String::new(),
    }]);
    assert!(!matches!(
        registrar(&s).fetch_exam_page().await.unwrap_err(),
        RegistrarClientError::LoginExpired { .. }
    ));
    assert!(target.requests().is_empty());
    assert_eq!(s.requests().len(), 1);
}

fn list(rows: Vec<Value>) -> String {
    json!({"data":{"list":rows}}).to_string()
}
fn seat(id: u64, status: i64) -> Value {
    json!({"id":id,"name":"Fixture seat","status":status,"area_type":1})
}
fn segment(day: &str, id: u64, start: &str) -> Value {
    json!({"id":id,"day":day,"startTime":{"date":format!("{day} {start}:00.000000")},"endTime":{"date":format!("{day} 22:00:00.000000")}})
}

#[test]
fn backend_repair_binding_socket_conflicting_same_seat_is_rejected() {
    assert!(
        library_read::parse_socket_status(
            r#"[{"seatId":7,"status":"available"},{"seatId":7,"status":"unavailable"}]"#
        )
        .is_err()
    );
}

#[test]
fn backend_repair_binding_socket_identical_copies_are_one_status() {
    let rows = library_read::parse_socket_status(
        r#"[{"seatId":7,"status":"unknown"},{"seatId":"7","status":"unknown"}]"#,
    )
    .unwrap();
    assert_eq!(rows.records.len(), 1);
}

#[test]
fn backend_repair_binding_seat_conflicting_identity_cannot_be_available_and_unavailable() {
    assert!(library_read::parse_seat_availability(&list(vec![seat(7, 1), seat(7, 0)])).is_err());
}

#[test]
fn backend_repair_binding_seat_exact_duplicate_is_not_double_counted() {
    assert_eq!(
        library_read::parse_seat_availability(&list(vec![seat(7, 1), seat(7, 1), seat(8, 1)]))
            .unwrap()
            .seats
            .len(),
        2
    );
}

#[test]
fn backend_repair_binding_area_sibling_id_conflict_is_rejected() {
    assert!(
        library_read::parse_area_tree(&list(vec![
            json!({"id":1,"name":"First"}),
            json!({"id":1,"name":"Other"})
        ]))
        .is_err()
    );
    assert!(library_read::parse_area_tree(&list(vec![json!({"id":1,"name":"Parent","childArea":[{"id":2,"name":"First"},{"id":2,"name":"Other"}]})])).is_err());
}

#[test]
fn backend_repair_binding_area_exact_copy_and_distinct_children_are_preserved() {
    let parent =
        json!({"id":1,"name":"Parent","childArea":[{"id":2,"name":"A"},{"id":3,"name":"B"}]});
    let parsed = library_read::parse_area_tree(&list(vec![parent.clone(), parent])).unwrap();
    assert_eq!(parsed.areas.len(), 1);
    assert_eq!(parsed.areas[0].child_areas.len(), 2);
}

#[test]
fn backend_repair_binding_library_timestamp_date_must_match_segment_day() {
    let mut value = segment("2026-09-18", 1, "08:00");
    value["startTime"]["date"] = json!("2026-09-17 08:00:00.000000");
    assert!(library_read::parse_day_segments(&list(vec![value])).is_err());
    let mut value = segment("2026-09-18", 1, "08:00");
    value["endTime"] = json!("2026-09-19T22:00:00");
    assert!(library_read::parse_day_segments(&list(vec![value])).is_err());
}

#[test]
fn backend_repair_binding_library_same_day_segment_conflict_is_rejected() {
    assert!(
        library_read::parse_day_segments(&list(vec![
            segment("2026-09-18", 1, "08:00"),
            segment("2026-09-18", 1, "09:00")
        ]))
        .is_err()
    );
}

#[test]
fn backend_repair_binding_library_segment_id_can_recur_on_different_days() {
    let a = segment("2026-09-18", 1, "08:00");
    let b = segment("2026-09-19", 1, "09:00");
    let records = library_read::parse_day_segments(&list(vec![a.clone(), a, b])).unwrap();
    assert_eq!(records.segments.len(), 2);
    assert_eq!(records.segments[0].start_time, "08:00");
    assert_eq!(records.segments[1].day, "2026-09-19");
}

#[tokio::test]
async fn backend_repair_binding_library_join_rejects_foreign_cookie_context_before_dispatch() {
    let s = FixtureServer::new(vec![]);
    let base = reqwest::Url::parse(s.base()).unwrap();
    let seats = LibraryReadAdapter::try_with_transport(
        base.clone(),
        CampusHttpTransport::new("THYou/response-fixture").unwrap(),
    )
    .unwrap();
    let sockets = LibrarySocketStatusAdapter::try_with_transport(
        base,
        CampusHttpTransport::new("THYou/other-fixture").unwrap(),
    )
    .unwrap();
    assert!(
        seats
            .read_seat_availability_with_socket(1, 2, "2026-09-18", "08:00", "22:00", &sockets)
            .await
            .is_err()
    );
    assert!(s.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_binding_library_shared_context_join_preserves_unknown_socket() {
    let s = FixtureServer::new(vec![
        Reply::json(&list(vec![seat(7, 1), seat(8, 0)])),
        Reply::json(r#"[{"seatId":7,"status":"unavailable"}]"#),
    ]);
    let base = reqwest::Url::parse(s.base()).unwrap();
    let seats = LibraryReadAdapter::try_with_transport(
        base.clone(),
        CampusHttpTransport::new("THYou/response-fixture").unwrap(),
    )
    .unwrap();
    let sockets = seats.socket_status_adapter(base).unwrap();
    let data = seats
        .read_seat_availability_with_socket(1, 2, "2026-09-18", "08:00", "22:00", &sockets)
        .await
        .unwrap();
    assert_eq!(data.seats.len(), 2);
    assert_eq!(data.seats[0].socket_status, LibrarySocketState::Unavailable);
    assert_eq!(data.seats[1].socket_status, LibrarySocketState::Unknown);
    assert_eq!(s.requests().len(), 2);
}

fn transaction(date: &str, cents: Value) -> Value {
    json!({"id":"row-1","summary":"Fixture","txdate":date,"balance":10000,"txamt":cents,"meraddr":"","mername":"Fixture","txname":"消费"})
}
fn transactions(rows: Vec<Value>) -> String {
    json!({"success":true,"resultData":{"rows":rows}}).to_string()
}

#[test]
fn backend_repair_binding_card_transaction_date_is_not_arbitrary_nonempty_text() {
    for value in [
        "not-a-date",
        "2026-02-30 12:00:00",
        "2026-09-18 25:00:00",
        "2026-09-18 12:00:99",
        "2026-09-18 12:00:00garbage",
    ] {
        assert!(
            parse_card_transactions_response(&transactions(vec![transaction(value, json!(-123))]))
                .is_err()
        );
    }
}

#[test]
fn backend_repair_binding_card_valid_dates_and_exact_signed_cents_survive() {
    for date in [
        "2026-09-18 12:30:00",
        "2026-09-18T12:30:00+08:00",
        "2026-09-18",
    ] {
        let report =
            parse_card_transactions_response(&transactions(vec![transaction(date, json!(-123))]))
                .unwrap();
        assert_eq!(report.transactions[0].amount_cents, -123);
    }
    for bad in [
        json!(1.25),
        json!("123.0"),
        json!("9223372036854775808"),
        json!(true),
    ] {
        assert!(
            parse_card_transactions_response(&transactions(vec![transaction(
                "2026-09-18 12:30:00",
                bad
            )]))
            .is_err()
        );
    }
}

async fn card_read(
    date: &str,
) -> (
    Result<
        crate::campus_card_read::CampusCardTransactionReport,
        crate::campus_card_adapter::CampusCardAdapterError,
    >,
    FixtureServer,
) {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#),
        Reply::json(&transactions(vec![transaction(date, json!(-123))])),
    ]);
    let client = CampusCardClient::new(
        CampusCardAdapterConfig::new(s.base()).unwrap(),
        CampusHttpTransport::new("THYou/response-fixture").unwrap(),
    )
    .unwrap();
    let session = client.probe_session().await.unwrap();
    let query = CampusCardTransactionQuery::new(
        "2026-09-18",
        "2026-09-18",
        CampusCardTransactionType::Any,
        100,
        0,
    )
    .unwrap();
    let result = client.read_transactions(&session, &query).await;
    (result, s)
}

#[tokio::test]
async fn backend_repair_binding_card_rows_outside_requested_date_fail_without_filtering() {
    let (result, s) = card_read("2026-09-17 23:59:59").await;
    assert!(result.is_err());
    assert_eq!(s.requests().len(), 2);
    let (result, s) = card_read("2026-09-19 00:00:00").await;
    assert!(result.is_err());
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_binding_card_date_range_is_inclusive_and_timezone_aware() {
    for date in [
        "2026-09-18 00:00:00",
        "2026-09-18 23:59:59",
        "2026-09-17T16:00:00Z",
    ] {
        let (result, s) = card_read(date).await;
        assert_eq!(result.unwrap().len(), 1);
        assert_eq!(s.requests().len(), 2);
    }
}

fn registrar(server: &FixtureServer) -> RegistrarClient {
    RegistrarClient::from_transport(
        RegistrarClientConfig {
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..Default::default()
        },
        CampusHttpTransport::new("THYou/response-fixture").unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn backend_repair_binding_exam_page_blocked_login_redirect_retains_expiry_evidence() {
    let target = FixtureServer::new(vec![]);
    let s = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}do/off/ui/auth/login/form/fixture/0\r\n",
            target.base()
        ),
        body: String::new(),
    }]);
    let error = registrar(&s).fetch_exam_page().await.unwrap_err();
    assert!(matches!(error, RegistrarClientError::LoginExpired { .. }));
    assert_eq!(s.requests().len(), 1);
    assert!(target.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_binding_exam_course_blocked_login_redirect_retains_expiry_evidence() {
    use crate::registrar_client::registrar_exam::{RegistrarExamCourseQuery, RegistrarExamTerm};
    let target = FixtureServer::new(vec![]);
    let s = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}do/off/ui/auth/login/form/fixture/0\r\n",
            target.base()
        ),
        body: String::new(),
    }]);
    let query = RegistrarExamCourseQuery::new(
        "30240512",
        "01",
        RegistrarExamTerm::new(2026, 2027, 1).unwrap(),
        "fixtureExam",
    )
    .unwrap();
    let error = registrar(&s).fetch_exam_course(&query).await.unwrap_err();
    assert!(matches!(error, RegistrarClientError::LoginExpired { .. }));
    assert_eq!(s.requests().len(), 1);
    assert!(target.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_binding_exam_other_redirect_and_outage_do_not_become_expiry() {
    let target = FixtureServer::new(vec![]);
    for reply in [
        Reply {
            status: 302,
            headers: format!("Location: {}maintenance\r\n", target.base()),
            body: String::new(),
        },
        Reply {
            status: 503,
            headers: "Content-Type: text/html\r\n".into(),
            body: "unavailable".into(),
        },
    ] {
        let s = FixtureServer::new(vec![reply]);
        assert!(!matches!(
            registrar(&s).fetch_exam_page().await.unwrap_err(),
            RegistrarClientError::LoginExpired { .. }
        ));
        assert_eq!(s.requests().len(), 1);
    }
    assert!(target.requests().is_empty());
}
