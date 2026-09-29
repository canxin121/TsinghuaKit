//! Loopback fixtures and shape tests for the CAB study-room slice.

use std::time::Duration;

use reqwest::Url;

use crate::library_room_read::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

/// The mapping root the runtime installs for this service.
const MAPPING: &str =
    "/https/77726476706e69737468656265737421f3f643d22b396a1e6a1b80a29f5d363409e413829737d1";

fn adapter(server: &FixtureServer) -> LibraryRoomAdapter {
    LibraryRoomAdapter::try_with_transport(
        mapped_url(server, ""),
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap()
}

/// Builds a URL inside this service's WebVPN mapping on the fixture origin.
fn mapped_url(server: &FixtureServer, suffix: &str) -> Url {
    let mut url = Url::parse(server.base()).unwrap();
    url.set_path(&format!("{MAPPING}{suffix}"));
    url
}

/// Wraps a `data` payload in the service's own envelope.
fn envelope(data: &str) -> String {
    format!(r#"{{"code":0,"message":"ok","data":{data}}}"#)
}

/// One room kind with two reservable devices.
fn catalog_body() -> String {
    envelope(
        r#"[{"kindId":11,"kindName":"研读间 A",
             "roomInfos":[
               {"devId":101,"devName":"A-101","minResvTime":30,"maxResvTime":120},
               {"devId":102,"devName":"A-102","minResvTime":60,"maxResvTime":180}
             ]},
            {"kindId":12,"kindName":"研读间 B",
             "roomInfos":[{"devId":201,"devName":"B-201","minResvTime":30,"maxResvTime":120}]}]"#,
    )
}

/// A catalogue the service answered with a real, empty list.
fn empty_catalog_body() -> String {
    envelope("[]")
}

/// One reservation with one invited member.
fn record_json(members: &str) -> String {
    format!(
        r#"[{{"uuid":"u-1","resvId":9001,"resvName":"张三","logonName":"zhangsan",
             "resvDate":"20260930","resvBeginTime":"2026-09-30 10:00",
             "resvEndTime":"2026-09-30 11:00",
             "resvDevInfoList":[{{"devId":101,"devName":"A-101","kindId":11,"kindName":"研读间 A"}}],
             "resvMemberInfoList":{members}}}]"#
    )
}

fn records_body() -> String {
    envelope(&record_json(
        r#"[{"trueName":"张三","logonName":"zhangsan"}]"#,
    ))
}

#[test]
fn the_catalogue_is_read_from_the_exact_envelope_shape() {
    let parsed = parse_library_room_catalog_json(&catalog_body()).expect("catalogue parses");
    assert_eq!(parsed.kinds.len(), 2);
    assert_eq!(parsed.kinds[0].kind_id, 11);
    assert_eq!(parsed.kinds[0].kind_name, "研读间 A");
    assert_eq!(parsed.kinds[0].rooms.len(), 2);
    assert_eq!(parsed.kinds[0].rooms[0].device_id, 101);
    assert_eq!(parsed.kinds[0].rooms[0].name, "A-101");
    assert_eq!(parsed.kinds[0].rooms[0].min_reserve_minutes, 30);
    assert_eq!(parsed.kinds[1].kind_id, 12);
    assert_eq!(parsed.room_count(), 3);
    assert!(!parsed.is_empty());
}

#[test]
fn a_room_identifier_the_service_sent_as_a_string_is_still_read() {
    // The service is inconsistent about JSON number versus string for the same
    // field, and a stringified device id addresses the same device.
    let body = envelope(
        r#"[{"kindId":"11","kindName":"研读间 A",
             "roomInfos":[{"devId":"101","devName":"A-101","minResvTime":"30"}]}]"#,
    );
    let parsed = parse_library_room_catalog_json(&body).expect("catalogue parses");
    assert_eq!(parsed.kinds[0].kind_id, 11);
    assert_eq!(parsed.kinds[0].rooms[0].device_id, 101);
    assert_eq!(parsed.kinds[0].rooms[0].min_reserve_minutes, 30);
}

#[test]
fn an_empty_catalogue_is_reported_only_when_the_service_sent_an_empty_list() {
    let parsed = parse_library_room_catalog_json(&empty_catalog_body()).expect("empty parses");
    assert!(parsed.kinds.is_empty());
    assert!(parsed.is_empty());
    assert_eq!(parsed.room_count(), 0);
}

#[test]
fn a_kind_whose_room_list_is_missing_is_a_failure_not_a_kind_with_no_rooms() {
    // The reference answers this shape from a built-in mock, which cannot
    // distinguish "no device is reservable" from "the response changed".
    let body = envelope(r#"[{"kindId":11,"kindName":"研读间 A"}]"#);
    assert_eq!(
        parse_library_room_catalog_json(&body).unwrap_err(),
        LibraryRoomParseError::MissingField { row: 0 }
    );
}

#[test]
fn a_kind_without_a_name_is_a_failure() {
    let body = envelope(r#"[{"kindId":11,"roomInfos":[]}]"#);
    assert_eq!(
        parse_library_room_catalog_json(&body).unwrap_err(),
        LibraryRoomParseError::MissingField { row: 0 }
    );
}

#[test]
fn a_kind_with_a_blank_name_is_a_failure() {
    let body = envelope(r#"[{"kindId":11,"kindName":"   ","roomInfos":[]}]"#);
    assert_eq!(
        parse_library_room_catalog_json(&body).unwrap_err(),
        LibraryRoomParseError::UnexpectedValue { row: 0 }
    );
}

#[test]
fn a_negative_identifier_is_a_shape_this_module_does_not_understand() {
    let body = envelope(r#"[{"kindId":-1,"kindName":"研读间 A","roomInfos":[]}]"#);
    assert_eq!(
        parse_library_room_catalog_json(&body).unwrap_err(),
        LibraryRoomParseError::UnexpectedValue { row: 0 }
    );
}

#[test]
fn a_field_carrying_a_control_character_is_refused_rather_than_projected() {
    let body = envelope("[{\"kindId\":11,\"kindName\":\"研读间\\u0000A\",\"roomInfos\":[]}]");
    assert_eq!(
        parse_library_room_catalog_json(&body).unwrap_err(),
        LibraryRoomParseError::UnexpectedValue { row: 0 }
    );
}

#[test]
fn a_field_longer_than_the_bound_is_refused_rather_than_truncated() {
    // Truncating would hand back a name the service never printed.
    let body = envelope(&format!(
        r#"[{{"kindId":11,"kindName":"{}","roomInfos":[]}}]"#,
        "x".repeat(257)
    ));
    assert_eq!(
        parse_library_room_catalog_json(&body).unwrap_err(),
        LibraryRoomParseError::UnexpectedValue { row: 0 }
    );
}

#[test]
fn the_services_own_refusal_is_reported_with_its_number_and_no_message() {
    // The service's accompanying message is server text, so only the code is
    // carried.  The error must not be readable as an empty catalogue.
    let body = r#"{"code":50001,"message":"no permission","data":null}"#;
    let error = parse_library_room_catalog_json(body).unwrap_err();
    assert_eq!(
        error,
        LibraryRoomParseError::ServiceRejected { code: 50001 }
    );
    assert!(!error.is_session_expired());
}

#[test]
fn a_usable_envelope_without_a_payload_is_a_failure() {
    let body = r#"{"code":0,"message":"ok"}"#;
    assert_eq!(
        parse_library_room_catalog_json(body).unwrap_err(),
        LibraryRoomParseError::MissingData
    );
}

#[test]
fn a_body_that_is_not_the_envelope_is_a_failure() {
    for body in [
        "<html><body>hello</body></html>",
        r#"{"result":"ok"}"#,
        "[1,2,3]",
        "not json at all",
    ] {
        let error = parse_library_room_catalog_json(body).unwrap_err();
        assert!(
            matches!(
                error,
                LibraryRoomParseError::NotEnvelope | LibraryRoomParseError::Malformed { .. }
            ),
            "{body:?} should not parse as the envelope, got {error:?}"
        );
    }
}

#[test]
fn a_webvpn_portal_page_is_a_login_failure() {
    let body = "<html><head><title>清华大学WebVPN</title></head><body></body></html>";
    let error = parse_library_room_catalog_json(body).unwrap_err();
    assert_eq!(error, LibraryRoomParseError::LoginPage);
    assert!(error.is_session_expired());
}

#[test]
fn a_timed_out_page_is_an_expiry_failure() {
    let body = "<html><body>用户登陆超时或访问内容不存在</body></html>";
    let error = parse_library_room_catalog_json(body).unwrap_err();
    assert_eq!(error, LibraryRoomParseError::ExpiredPage);
    assert!(error.is_session_expired());
}

#[test]
fn an_empty_body_is_a_failure() {
    assert_eq!(
        parse_library_room_catalog_json("   ").unwrap_err(),
        LibraryRoomParseError::EmptyBody
    );
}

#[test]
fn reservations_are_read_from_the_exact_envelope_shape() {
    let parsed = parse_library_room_records_json(&records_body()).expect("records parse");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].name, "张三");
    assert_eq!(parsed[0].device_name, "A-101");
    assert_eq!(parsed[0].kind_name, "研读间 A");
    assert_eq!(parsed[0].date, "20260930");
    assert_eq!(parsed[0].begin_time, "2026-09-30 10:00");
    assert_eq!(parsed[0].end_time, "2026-09-30 11:00");
    assert_eq!(parsed[0].members.len(), 1);
    assert_eq!(parsed[0].members[0].name, "张三");
}

#[test]
fn a_members_account_identifier_never_reaches_a_projected_record() {
    // The service sends each member's campus account name beside their printed
    // name.  Only the printed name is projected, so the account identifier
    // cannot cross this boundary through a debug print or a bridge DTO.
    let parsed = parse_library_room_records_json(&records_body()).expect("records parse");
    let rendered = format!("{parsed:?}");
    assert!(
        !rendered.contains("zhangsan"),
        "the member's account name must not be projected: {rendered}"
    );
}

#[test]
fn a_reservation_with_no_device_is_a_failure_not_a_roomless_booking() {
    let body = envelope(&record_json("[]").replace(
        r#""resvDevInfoList":[{"devId":101,"devName":"A-101","kindId":11,"kindName":"研读间 A"}],"#,
        r#""resvDevInfoList":[],"#,
    ));
    assert_eq!(
        parse_library_room_records_json(&body).unwrap_err(),
        LibraryRoomParseError::MissingField { row: 0 }
    );
}

#[test]
fn a_reservation_missing_a_printed_field_is_a_failure_not_a_blank_record() {
    let body = envelope(&record_json("[]").replace(r#""resvDate":"20260930","#, ""));
    assert_eq!(
        parse_library_room_records_json(&body).unwrap_err(),
        LibraryRoomParseError::MissingField { row: 0 }
    );
}

#[test]
fn a_reservation_list_the_service_sent_empty_is_an_empty_list() {
    let parsed = parse_library_room_records_json(&envelope("[]")).expect("empty parses");
    assert!(parsed.is_empty());
}

#[test]
fn a_login_page_behind_the_envelope_route_is_a_session_failure() {
    let body = "<html><head><title>清华大学WebVPN</title></head><body></body></html>";
    assert!(
        parse_library_room_records_json(body)
            .unwrap_err()
            .is_session_expired()
    );
}

#[test]
fn a_window_is_validated_before_it_can_become_a_query() {
    let profile = LibraryRoomProfile::standard();
    // Reversed.
    assert!(matches!(
        profile.records_request("2026-09-30", "2026-09-01"),
        Err(LibraryRoomAdapterError::InvalidWindow)
    ));
    // Not a calendar date at all.
    assert!(matches!(
        profile.records_request("2026-02-30", "2026-03-01"),
        Err(LibraryRoomAdapterError::InvalidWindow)
    ));
    assert!(matches!(
        profile.records_request("today", "2026-09-30"),
        Err(LibraryRoomAdapterError::InvalidWindow)
    ));
    // A value that would need escaping is refused here rather than escaped.
    assert!(matches!(
        profile.records_request("2026-09-30'&x=", "2026-09-30"),
        Err(LibraryRoomAdapterError::InvalidWindow)
    ));
}

#[test]
fn an_unpadded_date_is_normalized_rather_than_sent_as_written() {
    // `%m`/`%d` accept one or two digits, so an unpadded date is a real calendar
    // date.  It is re-emitted from the parsed date rather than echoed, which is
    // what keeps the query's alphabet to the module's own constants.
    let profile = LibraryRoomProfile::standard();
    let plan = profile
        .records_request("2026-9-3", "2026-9-4")
        .expect("window builds");
    let query = plan.query.expect("records plan carries a query");
    assert!(query.contains("beginDate=2026-09-03"), "{query}");
    assert!(query.contains("endDate=2026-09-04"), "{query}");
}

#[test]
fn a_window_wider_than_the_bound_is_refused() {
    let profile = LibraryRoomProfile::standard();
    // The bound is inclusive of its last day, so exactly the bound is usable.
    assert!(profile.records_request("2026-09-01", "2026-10-01").is_ok());
    assert!(matches!(
        profile.records_request("2026-09-01", "2026-10-02"),
        Err(LibraryRoomAdapterError::InvalidWindow)
    ));
}

#[test]
fn the_window_is_normalized_before_it_reaches_the_query() {
    let profile = LibraryRoomProfile::standard();
    let plan = profile
        .records_request("2026-09-30", "2026-10-06")
        .expect("window builds");
    let query = plan.query.expect("records plan carries a query");
    assert!(query.contains("beginDate=2026-09-30"));
    assert!(query.contains("endDate=2026-10-06"));
    assert!(query.contains("needStatus=8454"));
    assert!(query.contains("orderKey=gmt_create"));
    assert!(query.contains("orderModel=desc"));
}

#[test]
fn the_catalogue_plan_carries_no_query_at_all() {
    let plan = LibraryRoomProfile::standard().catalog_request();
    assert_eq!(plan.path, LIBRARY_ROOM_CATALOG_PATH);
    assert_eq!(plan.method, LibraryRoomMethod::Get);
    assert!(plan.query.is_none());
    assert_eq!(
        plan.session_prerequisite,
        LibraryRoomSessionPrerequisite::ExistingInfoWebVpnSession
    );
}

#[tokio::test]
async fn the_adapter_reads_the_catalogue_through_the_cookie_aware_transport() {
    let server = FixtureServer::new(vec![Reply::json(&catalog_body())]);
    let read = adapter(&server)
        .read_catalog_with_proof()
        .await
        .expect("catalogue succeeds");
    assert_eq!(read.value.kinds.len(), 2);
    assert_eq!(read.value.room_count(), 3);
    assert_eq!(read.proof.operation, LibraryRoomOperation::ReadCatalog);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET "));
    assert!(
        requests[0].contains(&format!("{MAPPING}{LIBRARY_ROOM_CATALOG_PATH}")),
        "request path should be the mapped catalogue endpoint: {}",
        requests[0]
    );
    assert!(
        !requests[0].contains("?"),
        "the catalogue request must carry no query: {}",
        requests[0]
    );
}

#[tokio::test]
async fn the_records_read_sends_the_window_it_validated() {
    let server = FixtureServer::new(vec![Reply::json(&records_body())]);
    let read = adapter(&server)
        .read_records_with_proof("2026-09-30", "2026-10-06")
        .await
        .expect("records succeed");
    assert_eq!(read.value.len(), 1);
    assert_eq!(read.proof.operation, LibraryRoomOperation::ReadRecords);

    let requests = server.requests();
    assert!(
        requests[0].contains(&format!(
            "{MAPPING}{LIBRARY_ROOM_RECORDS_PATH}?needStatus=8454&orderKey=gmt_create&orderModel=desc\
             &beginDate=2026-09-30&endDate=2026-10-06"
        )),
        "records request should carry the validated window: {}",
        requests[0]
    );
}

#[tokio::test]
async fn an_unusable_window_costs_no_request() {
    let server = FixtureServer::new(vec![]);
    let adapter = adapter(&server);
    assert!(matches!(
        adapter
            .read_records("2026-09-30", "2026-09-01")
            .await
            .unwrap_err(),
        LibraryRoomAdapterError::InvalidWindow
    ));
    assert!(matches!(
        adapter.read_records("", "").await.unwrap_err(),
        LibraryRoomAdapterError::InvalidWindow
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_login_answer_reaches_the_caller_as_a_session_failure() {
    let server = FixtureServer::new(vec![Reply::html(
        "<html><head><title>清华大学WebVPN</title></head><body></body></html>",
    )]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "library_room_auth_required");
}

#[tokio::test]
async fn an_html_page_that_is_not_a_login_page_is_not_folded_into_a_session_failure() {
    // Only the session-level pages are authentication failures.  Any other
    // HTML is a deployment this module does not understand, and reporting it as
    // an expiry would send the runtime down an auth-refresh path that cannot
    // fix it.
    let server = FixtureServer::new(vec![Reply::html(
        "<html><body><h1>Service moved</h1></body></html>",
    )]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(!error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "library_room_envelope");
}

#[tokio::test]
async fn the_services_refusal_reaches_the_caller_with_its_code() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"code":50001,"message":"no permission","data":null}"#,
    )]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert_eq!(error.diagnostic_code(), "library_room_rejected");
    assert!(matches!(
        error,
        LibraryRoomAdapterError::Parse(LibraryRoomParseError::ServiceRejected { code: 50001 })
    ));
}

#[tokio::test]
async fn a_non_json_answer_is_not_parsed_as_a_catalogue() {
    let server = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/pdf\r\n".into(),
        body: "%PDF-1.7".into(),
    }]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(matches!(
        error,
        LibraryRoomAdapterError::UnexpectedContentType
    ));
}

#[tokio::test]
async fn a_server_error_is_reported_with_its_status() {
    let server = FixtureServer::new(vec![Reply {
        status: 500,
        headers: "Content-Type: application/json\r\n".into(),
        body: r#"{"code":0,"data":[]}"#.into(),
    }]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(matches!(error, LibraryRoomAdapterError::HttpStatus { .. }));
}

#[tokio::test]
async fn an_unauthorized_status_is_a_session_failure() {
    let server = FixtureServer::new(vec![Reply {
        status: 401,
        headers: "Content-Type: application/json\r\n".into(),
        body: "{}".into(),
    }]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(error.is_session_expired());
}

#[tokio::test]
async fn a_redirect_to_another_origin_is_refused_before_it_is_followed() {
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: https://elsewhere.invalid/steal\r\n".into(),
        body: String::new(),
    }]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(
        matches!(error, LibraryRoomAdapterError::UnexpectedOrigin),
        "unexpected error: {error:?}"
    );
    // Only the original request was made; the foreign location was not fetched.
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_redirect_within_the_origin_but_outside_the_mapping_is_refused() {
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: /elsewhere/steal\r\n".into(),
        body: String::new(),
    }]);
    let error = adapter(&server).read_catalog().await.unwrap_err();
    assert!(
        matches!(error, LibraryRoomAdapterError::UnexpectedPath),
        "unexpected error: {error:?}"
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_configured_base_path_cannot_widen_the_mapping() {
    // The base URL is reduced to `/{scheme}/{token}/`, so a configured path
    // cannot move this module's reads to another part of the broker.
    let url = Url::parse(&format!(
        "https://webvpn.example.invalid{MAPPING}/some/deeper/place/"
    ))
    .unwrap();
    let adapter = LibraryRoomAdapter::try_with_transport(
        url,
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap();
    let debug = format!("{adapter:?}");
    assert!(
        !debug.contains("some") && !debug.contains("deeper"),
        "the configured deeper path must not survive normalization: {debug}"
    );
    // And a base that is neither a bare origin nor an opaque mapping is refused
    // outright rather than kept as unproven authority.
    let stray = Url::parse("https://webvpn.example.invalid/some/deeper/place/").unwrap();
    assert!(matches!(
        LibraryRoomAdapter::try_with_transport(
            stray,
            CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
        ),
        Err(LibraryRoomAdapterError::InvalidBaseUrl)
    ));
}

#[test]
fn a_mistyped_mapping_token_is_refused_where_it_is_configured() {
    // Without this check a bad token would be accepted here and only fail much
    // later, as a broker response this module could not explain.
    for token in [
        // Truncated.
        "77726476706e69737468656265737421f3f643d2",
        // Upper-case hex, which the fixed prefix is always written in lower.
        "77726476706e69737468656265737421F3F643D22B396A1E",
        // Not the fixed prefix.
        "0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        let url = Url::parse(&format!("https://webvpn.example.invalid/https/{token}/")).unwrap();
        assert!(
            matches!(
                LibraryRoomAdapter::try_with_transport(
                    url,
                    CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10))
                        .unwrap(),
                ),
                Err(LibraryRoomAdapterError::InvalidBaseUrl)
            ),
            "{token} should be refused"
        );
    }
}

#[test]
fn the_profile_is_fixed_and_carries_no_route_a_caller_supplied() {
    // There is no constructor that takes a profile or a path, so a bridge
    // caller cannot aim a read at a route this module did not record.
    let url = Url::parse(
        "https://webvpn.tsinghua.edu.cn/https/\
         77726476706e69737468656265737421f3f643d22b396a1e6a1b80a29f5d363409e413829737d1/",
    )
    .unwrap();
    let adapter = LibraryRoomAdapter::try_with_transport(
        url,
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap();
    assert_eq!(adapter.profile(), LibraryRoomProfile::standard());
}

#[test]
fn the_cab_identity_login_policy_is_never_registered_as_a_roam_selector() {
    // The reference recovers this application by performing a campus identity
    // login, and it derives the application id to submit from the
    // `…/auth/address` response.  This engine implements no second campus login
    // and does not let a response choose an identity-login application id, so
    // the policy name must not be a selector the roaming allow list answers.
    let registered = [
        "40470BB47E0849E9EF717983490BC964",
        "287C0C6D90ABB364CD5FDF1495199962",
        "BEABB32641DC4EC3510B048BAF42471A",
        "B7EF0ADF9406335AD7905B30CD7B49B1",
        "E35232808C08C8C5F199F13BF6B7F5D0",
        "3E401364BDD7AEA7EBF1EDE3F15ED4B7",
    ];
    assert!(!registered.contains(&LIBRARY_ROOM_WEBVPN_TARGET));
}

#[test]
fn the_mapping_token_carries_the_fixed_webvpn_prefix() {
    // A mapping token is the public fixed prefix followed by a hex host
    // selector.  Pinning the prefix here keeps a mistyped token from being
    // registered as a mapping that would never match.
    assert!(LIBRARY_ROOM_MAPPING_TOKEN.starts_with("77726476706e69737468656265737421"));
    assert_eq!(LIBRARY_ROOM_MAPPING_TOKEN.len() % 2, 0);
    assert!(
        LIBRARY_ROOM_MAPPING_TOKEN
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    // The reference spells this route as `/https/<token>/…`, and this module
    // must agree: a scheme mismatch would register a mapping the broker never
    // serves.
    assert_eq!(LIBRARY_ROOM_MAPPING_SCHEME, "https");
}
