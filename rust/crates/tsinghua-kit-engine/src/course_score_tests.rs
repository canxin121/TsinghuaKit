//! Loopback fixtures and shape tests for the per-course score lookup.
//!
//! The lookup runs through the service hall's shared transport, so every test
//! here drives the real client against a synthetic loopback server and inspects
//! the request the client actually built.

use reqwest::Url;
use serde_json::{Value, json};

use crate::course_score::{
    CourseScoreError, course_id_for_request, parse_course_score, student_id_for_request,
};
use crate::reference_test_support::{FixtureServer, Reply};
use crate::thos::{MAPPING, ThosClient, ThosError};
use crate::transport::CampusHttpTransport;

/// The pinned preset path, written out here so the test fails if the module's
/// own constant drifts away from the reference service's endpoint.
const SELECT_ONE_PATH: &str = "/fp/fp/Uniformcommon/selectOnePresetData";

/// The service's own preset identifier.  It is a body parameter, never a URL.
const PRESET_KEY: &str = "103765749452800";

/// The account's own student id, used here as the caller-supplied value the
/// Runtime would derive from the bound identity.
const STUDENT_ID: &str = "20260000000";

fn client(server: &FixtureServer) -> ThosClient {
    ThosClient::new(
        &Url::parse(server.base()).unwrap(),
        CampusHttpTransport::new("fixture-course-score").unwrap(),
    )
    .unwrap()
}

fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[tokio::test]
async fn backend_repair_course_score_uses_pinned_preset_and_derived_student_id() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"KCMC":"合成课程","XF":2.5,"DJZCJ":"A"}"#,
    )]);
    let score = client(&server)
        .course_score("CS0001", STUDENT_ID)
        .await
        .unwrap();
    assert_eq!(score.name(), "合成课程");
    assert_eq!(score.credit(), Some(2.5));
    assert_eq!(score.grade(), "A");
    assert!(!score.is_empty());

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with(&format!("POST /https/{MAPPING}{SELECT_ONE_PATH} HTTP/1.1")));
    assert!(requests[0].contains("application/json;charset=utf-8"));
    let body = body_of(&requests[0]);
    assert_eq!(
        body,
        json!({
            "presetKey": PRESET_KEY,
            "param": {"XH": STUDENT_ID, "KCH": "CS0001"},
        })
    );
}

#[tokio::test]
async fn backend_repair_course_score_keeps_grade_values_out_of_debug_output() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"KCMC":"合成课程名称","XF":"3","DJZCJ":"合成成绩"}"#,
    )]);
    let score = client(&server)
        .course_score("CS0002", STUDENT_ID)
        .await
        .unwrap();
    assert_eq!(score.credit(), Some(3.0));
    let presentation = format!("{score:?}");
    assert!(!presentation.contains("合成课程名称"));
    assert!(!presentation.contains("合成成绩"));
    // The course name and the grade are personal academic data, so the debug
    // form carries their shape rather than their text.
    assert!(presentation.contains("name_present: true"));
    assert!(presentation.contains("grade_present: true"));
    // The student id and the caller's course number are never result fields of
    // any kind, including in the debug form.
    assert!(!presentation.contains(STUDENT_ID));
    assert!(!presentation.contains("CS0002"));
}

#[tokio::test]
async fn backend_repair_course_score_reported_nothing_is_a_fact_not_a_failure() {
    for body in [
        json!({}),
        json!({"KCMC":null,"XF":null,"DJZCJ":null}),
        json!({"KCMC":"","XF":"","DJZCJ":""}),
    ] {
        let server = FixtureServer::new(vec![Reply::json(&body.to_string())]);
        let score = client(&server)
            .course_score("CS0003", STUDENT_ID)
            .await
            .unwrap();
        assert!(score.is_empty());
        assert_eq!(score.name(), "");
        assert_eq!(score.credit(), None);
        assert_eq!(score.grade(), "");
        assert_eq!(server.requests().len(), 1);
    }
    // A course with a name but no grade yet is not empty: the service did
    // report something about this course.
    let server = FixtureServer::new(vec![Reply::json(r#"{"KCMC":"在修课程"}"#)]);
    let score = client(&server)
        .course_score("CS0004", STUDENT_ID)
        .await
        .unwrap();
    assert!(!score.is_empty());
    assert_eq!(score.name(), "在修课程");
    assert_eq!(score.grade(), "");
}

#[test]
fn backend_repair_course_score_rejects_malformed_fields_instead_of_empty_results() {
    for body in [
        json!({"KCMC":"课程","XF":"甲乙"}),
        json!({"KCMC":"课程","XF":"-1"}),
        json!({"KCMC":"课程","XF":"1e999"}),
        json!({"KCMC":"课程","XF":101}),
        json!({"KCMC":"课程","XF":true}),
        json!({"KCMC":"课程","XF":{"value":1}}),
        json!({"KCMC":"课程","DJZCJ":{"text":"A"}}),
        json!({"KCMC":["课程"],"DJZCJ":"A"}),
        json!([]),
        json!("课程"),
    ] {
        assert_eq!(
            parse_course_score(&body),
            Err(CourseScoreError::Response),
            "body accepted: {body}"
        );
    }
    // The reference reads the fields straight off the answer with no required
    // key set, so an answer carrying none of them is the service's own statement
    // that it holds no result — it stays a successful empty read rather than
    // being turned into a parse failure.
    assert_eq!(
        parse_course_score(&json!({"unrelated":true})).map(|score| score.is_empty()),
        Ok(true)
    );
}

#[test]
fn backend_repair_course_score_argument_bounds_refuse_caller_text() {
    for course_id in [
        "",
        "CS 0001",
        "CS/0001",
        "CS%2F0001",
        "课程号",
        "../../etc/passwd",
        "CS0001<script>",
        &"C".repeat(33),
    ] {
        assert_eq!(
            course_id_for_request(course_id),
            Err(CourseScoreError::InvalidInput),
            "course id accepted: {course_id}"
        );
    }
    assert_eq!(course_id_for_request("CS0001-2_3"), Ok("CS0001-2_3"));
    for student_id in ["", "2026A000000", "20 26", "２０２６", "2026-0000"] {
        assert_eq!(
            student_id_for_request(student_id),
            Err(CourseScoreError::NotAStudentId),
            "student id accepted: {student_id}"
        );
    }
    assert_eq!(student_id_for_request("20260000000"), Ok("20260000000"));
}

#[tokio::test]
async fn backend_repair_course_score_refuses_bad_arguments_before_any_request() {
    let server = FixtureServer::new(vec![Reply::json(r#"{"KCMC":"合成课程"}"#)]);
    assert!(matches!(
        client(&server).course_score("CS 0001", STUDENT_ID).await,
        Err(ThosError::InvalidInput)
    ));
    // A username that is not an all-digit student id is a session-level
    // problem: the bound account is what this query cannot use.
    assert!(matches!(
        client(&server)
            .course_score("CS0001", "fixture-owner")
            .await,
        Err(ThosError::Session)
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_course_score_classifies_login_bodies_and_envelopes() {
    for body in [
        r#"<html><title>清华大学WebVPN</title></html>"#,
        r#"<html><body>您即将登录</body></html>"#,
        r#"<html><body>用户登陆超时或访问内容不存在</body></html>"#,
        r#"{"code":401,"message":"请重新登录"}"#,
        r#"{"success":false,"message":"查询失败"}"#,
    ] {
        let server = FixtureServer::new(vec![Reply::json(body)]);
        let error = client(&server)
            .course_score("CS0001", STUDENT_ID)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ThosError::Session | ThosError::Business),
            "body classified as {error:?}"
        );
        assert_eq!(server.requests().len(), 1);
    }
    for body in [
        r#"<html>maintenance</html>"#,
        r#"{"KCMC":"课程","DJZCJ":"A","XF":"甲乙"}"#,
    ] {
        let server = FixtureServer::new(vec![Reply::json(body)]);
        let error = client(&server)
            .course_score("CS0001", STUDENT_ID)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ThosError::Response | ThosError::Business),
            "body classified as {error:?}"
        );
        assert_eq!(server.requests().len(), 1);
    }
    // A read POST is never replayed at a redirected endpoint.
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: /unrelated\r\n".into(),
        body: String::new(),
    }]);
    assert!(matches!(
        client(&server).course_score("CS0001", STUDENT_ID).await,
        Err(ThosError::Route)
    ));
    assert_eq!(server.requests().len(), 1);
}
