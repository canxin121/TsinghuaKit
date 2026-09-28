//! Loopback fixtures and shape tests for the physical-education read slice.

use std::time::Duration;

use crate::physical_exam_read::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

use reqwest::Url;

/// One complete report, shaped like the service's own JSON: every field is a
/// string, including the numeric ones.
fn complete_report() -> String {
    serde_json::json!({
        "success": "true",
        "sfmc": "否",
        "mcyy": "",
        "zf": "82.5",
        "bzf": "80",
        "fjf": "2.5",
        "cpfjf": "1",
        "sg": "175",
        "tz": "68",
        "sgtzfs": "90",
        "fhl": "4200",
        "fhltzfs": "85",
        "bbmp": "3'30\"",
        "bbmpfs": "88",
        "yqmp": "",
        "yqmpfs": "",
        "wsmp": "7.2",
        "wsmpfs": "80",
        "ldty": "2.35",
        "ldtyfs": "78",
        "zwtqq": "12.5",
        "zwtqqfs": "76",
        "ywqz": "45",
        "ywqzfs": "82",
        "ytxs": "12",
        "ytxsfs": "84",
        "tykcj": "88"
    })
    .to_string()
}

#[test]
fn a_complete_report_parses_every_item_measurement_and_score() {
    let report = parse_physical_exam_json(&complete_report()).expect("report parses");

    assert!(!report.no_result);
    assert!(!report.is_empty());
    assert_eq!(report.exemption.as_deref(), Some("否"));
    assert_eq!(report.total.as_deref(), Some("82.5"));
    assert_eq!(report.standard_score.as_deref(), Some("80"));
    assert_eq!(report.physical_education_grade.as_deref(), Some("88"));
    assert_eq!(report.height.as_deref(), Some("175"));
    assert_eq!(report.weight.as_deref(), Some("68"));

    assert_eq!(
        report.items.vital_capacity.measurement.as_deref(),
        Some("4200")
    );
    assert_eq!(report.items.vital_capacity.score.as_deref(), Some("85"));
    assert_eq!(report.items.sit_up.measurement.as_deref(), Some("45"));
    assert_eq!(report.items.sit_up.score.as_deref(), Some("82"));
    // A 1000m record is reported as its own item, independent of the 800m one.
    assert_eq!(
        report.items.eight_hundred_meter.measurement.as_deref(),
        Some("3'30\"")
    );
    assert!(report.items.one_thousand_meter.is_empty());
}

#[test]
fn the_locally_computed_reference_total_uses_the_fixed_weights() {
    let report = parse_physical_exam_json(&complete_report()).expect("report parses");
    // 85*.15 + 80*.2 + 76*.1 + 78*.1 + 84*.1 + 88*.2 + 82*.1 + 90*.15 =
    // 12.75 + 16 + 7.6 + 7.8 + 8.4 + 17.6 + 8.2 + 13.5 = 91.85
    let expected = 91.85;
    let total = report.reference_total.expect("reference total is computed");
    assert!((total - expected).abs() < 1e-9, "got {total}");

    // The reference total is a local recomputation and must never be confused
    // with the service's own total.
    assert_ne!(report.total.as_deref(), Some("91.85"));
    assert_eq!(
        PhysicalExamReport::REFERENCE_TOTAL_LABEL,
        "参考成绩（APP自动结算，仅供参考）"
    );
}

#[test]
fn an_unrecorded_item_contributes_nothing_to_the_reference_total() {
    // One student runs either the 800m or the 1000m case, never both, so an
    // absent score is a normal record and not a reason to withhold a total.
    let body = serde_json::json!({
        "success": "true",
        "wsmpfs": "80",
        "sgtzfs": "90"
    })
    .to_string();
    let report = parse_physical_exam_json(&body).expect("report parses");
    // 80*.2 + 90*.15 = 16 + 13.5 = 29.5
    let total = report.reference_total.expect("reference total is computed");
    assert!((total - 29.5).abs() < 1e-9, "got {total}");
    // The service's own absent total stays absent rather than becoming zero.
    assert_eq!(report.total, None);
}

#[test]
fn an_unreadable_reported_score_withholds_the_whole_reference_total() {
    // A score the service did report, but which is not a number: the total
    // must be absent rather than quietly smaller.
    let body = serde_json::json!({
        "success": "true",
        "wsmpfs": "80",
        "sgtzfs": "优秀"
    })
    .to_string();
    let report = parse_physical_exam_json(&body).expect("report parses");
    assert_eq!(report.items.height_weight.score.as_deref(), Some("优秀"));
    assert_eq!(report.reference_total, None);
}

#[test]
fn the_no_result_answer_is_a_validated_empty_state_not_a_failure() {
    let report = parse_physical_exam_json(r#"{"success":"false"}"#)
        .expect("the no-result answer is not a parse failure");
    assert!(report.no_result);
    assert!(report.is_empty());
    assert_eq!(report.total, None);
    assert_eq!(report.reference_total, None);
    assert!(report.items.reported().is_empty());
}

#[test]
fn a_login_or_expiry_page_is_reported_as_a_session_failure() {
    let login = "<html><title>清华大学WebVPN</title><body>请登录</body></html>";
    assert_eq!(
        parse_physical_exam_json(login).unwrap_err(),
        PhysicalExamParseError::LoginPage
    );
    let expired = "time out用户登陆超时或访问内容不存在。请重试";
    assert_eq!(
        parse_physical_exam_json(expired).unwrap_err(),
        PhysicalExamParseError::ExpiredPage
    );
}

#[test]
fn an_unreadable_body_is_an_error_not_an_empty_report() {
    assert_eq!(
        parse_physical_exam_json("").unwrap_err(),
        PhysicalExamParseError::EmptyBody
    );
    assert_eq!(
        parse_physical_exam_json("<html><body>maintenance</body></html>").unwrap_err(),
        PhysicalExamParseError::NotJson
    );
    assert_eq!(
        parse_physical_exam_json("[1,2,3]").unwrap_err(),
        PhysicalExamParseError::NotJson
    );
    assert_eq!(
        parse_physical_exam_json("\"a string\"").unwrap_err(),
        PhysicalExamParseError::NotJson
    );
}

#[test]
fn a_json_body_without_the_service_flag_is_an_error() {
    assert_eq!(
        parse_physical_exam_json(r#"{"object":{"zf":"80"}}"#).unwrap_err(),
        PhysicalExamParseError::MissingSuccessFlag
    );
    // A boolean flag is not the service's string contract.
    assert_eq!(
        parse_physical_exam_json(r#"{"success":false}"#).unwrap_err(),
        PhysicalExamParseError::MissingSuccessFlag
    );
}

#[test]
fn a_composite_field_value_is_rejected_rather_than_stringified() {
    let body = serde_json::json!({
        "success": "true",
        "sg": {"nested": "175"}
    })
    .to_string();
    assert_eq!(
        parse_physical_exam_json(&body).unwrap_err(),
        PhysicalExamParseError::InvalidField { field: "sg" }
    );
}

#[test]
fn bounded_text_drops_blank_and_control_bearing_values() {
    let body = serde_json::json!({
        "success": "true",
        "sg": "  175  ",
        "tz": "",
        "fhl": "4200\u{0007}",
        "tykcj": "88"
    })
    .to_string();
    let report = parse_physical_exam_json(&body).expect("report parses");
    assert_eq!(report.height.as_deref(), Some("175"));
    assert_eq!(report.weight, None);
    assert_eq!(report.items.vital_capacity.measurement, None);
    assert_eq!(report.physical_education_grade.as_deref(), Some("88"));
}

#[test]
fn the_profile_issues_one_queryable_get_plan() {
    let plan = PhysicalExamProfile::standard().result_request();
    assert_eq!(plan.path, "/tyjx.tyjx_tc_xscjb.do");
    assert_eq!(plan.query_string(), "m=jsonCj");
    assert_eq!(plan.webvpn_target, PHYSICAL_EXAM_WEBVPN_TARGET);
    assert_eq!(plan.method, PhysicalExamMethod::Get);
    assert_eq!(
        plan.session_prerequisite,
        PhysicalExamSessionPrerequisite::ExistingInfoWebVpnSession
    );
}

#[tokio::test]
async fn the_report_is_fetched_through_the_cookie_aware_transport() {
    let server = FixtureServer::new(vec![Reply::json(&complete_report())]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter =
        PhysicalExamAdapter::try_with_transport(base, transport).expect("fixture adapter");

    let read = adapter
        .read_result_with_proof()
        .await
        .expect("fixture read succeeds");
    assert_eq!(read.value.total.as_deref(), Some("82.5"));
    assert!(adapter.business_proof_matches(&read.proof));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /tyjx.tyjx_tc_xscjb.do?m=jsonCj"));
}

#[tokio::test]
async fn the_no_result_answer_still_travels_through_the_adapter() {
    let server = FixtureServer::new(vec![Reply::json(r#"{"success":"false"}"#)]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter =
        PhysicalExamAdapter::try_with_transport(base, transport).expect("fixture adapter");
    let report = adapter
        .read_result()
        .await
        .expect("no-result read succeeds");
    assert!(report.no_result);
}

#[tokio::test]
async fn a_session_expiry_response_is_classified_before_parsing() {
    let server = FixtureServer::new(vec![Reply::json(
        "time out用户登陆超时或访问内容不存在。请重试",
    )]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter =
        PhysicalExamAdapter::try_with_transport(base, transport).expect("fixture adapter");
    let error = adapter.read_result().await.unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "physical_exam_auth_required");
}

#[tokio::test]
async fn a_non_json_content_type_is_refused() {
    let server = FixtureServer::new(vec![Reply::html(&complete_report())]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter =
        PhysicalExamAdapter::try_with_transport(base, transport).expect("fixture adapter");
    let error = adapter.read_result().await.unwrap_err();
    assert!(matches!(
        error,
        PhysicalExamAdapterError::UnexpectedContentType
    ));
    assert_eq!(error.diagnostic_code(), "physical_exam_content_type");
}

#[tokio::test]
async fn a_cross_origin_redirect_is_refused_before_any_parse() {
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: https://example.invalid/steal\r\n".into(),
        body: String::new(),
    }]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter =
        PhysicalExamAdapter::try_with_transport(base, transport).expect("fixture adapter");
    let error = adapter.read_result().await.unwrap_err();
    assert!(matches!(error, PhysicalExamAdapterError::UnexpectedOrigin));
}
