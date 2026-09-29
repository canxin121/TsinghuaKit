//! Runtime-level fixtures for the per-course score lookup.
//!
//! These use only loopback responses.  They verify that the lookup happens
//! inside the existing proven Identity + INFO session, that the student id the
//! service's query needs is derived from the bound account rather than from the
//! caller, and that a course number this client refuses never reaches the
//! network.

use super::*;

use crate::info::OpaqueUrl;
use crate::info_session::{InfoSessionAdapter, InfoWebVpnConfig};
use crate::protocol::{CsrfToken, UserIdentity};
use crate::reference_test_support::{FixtureServer, Reply};

/// The service's own preset identifier, as the client pins it.
const PRESET_KEY: &str = "103765749452800";

/// A ten-digit login id, so the account is usable as a student id.
const STUDENT_ID: &str = "20260000000";

fn account(username: &str) -> UserIdentity {
    UserIdentity {
        username: username.to_owned(),
        display_name: None,
    }
}

/// Installs a proven Identity + INFO session whose adapter talks to `server`.
fn course_score_runtime(server: &FixtureServer, username: &str) -> CampusRuntime {
    let mut runtime =
        CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap();
    let user = account(username);
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            runtime.coordinator.registry().bind_csrf(
                service,
                CsrfToken::new("fixture-course-score-csrf").unwrap(),
            )
        });
        runtime
            .coordinator
            .mark_authenticated(service, user.clone(), None, csrf, None)
            .unwrap();
    }
    runtime.portal_bootstrapped = true;
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/info/").unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    runtime.info_roaming_url =
        Some(OpaqueUrl::new(format!("{}info/f/info/index", server.base())).unwrap());
    runtime
}

/// The replies one course-score read consumes when its first counted probe
/// finds the target session closed.
///
/// The read begins with the shared counted probe, so an expired target session
/// costs exactly one pinned handoff and then one more probe before the preset
/// query itself.
fn course_score_replies(answer: Reply) -> Vec<Reply> {
    vec![
        // The counted probe finds the target session closed, once.
        Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-course-score-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"https://thos.tsinghua.edu.cn/fp/login?ticket=fixture"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{}/fp/view?m=fp\r\nSet-Cookie: thos_fixture=proved; Path=/\r\n",
                crate::thos::MAPPING
            ),
            body: String::new(),
        },
        Reply::html("<html>service hall</html>"),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        answer,
    ]
}

/// The counted probe's replies alone, for a read that never issues its own
/// query.
fn counted_probe_replies() -> Vec<Reply> {
    let mut replies = course_score_replies(Reply::json("{}"));
    replies.pop();
    replies
}

#[tokio::test]
async fn backend_repair_course_score_reads_inside_proven_session_with_derived_student_id() {
    let server = FixtureServer::new(course_score_replies(Reply::json(
        r#"{"KCMC":"合成课程","XF":2,"DJZCJ":"A"}"#,
    )));
    let mut runtime = course_score_runtime(&server, STUDENT_ID);
    let result = runtime
        .load_course_score_result("CS0001")
        .await
        .expect("course score reads");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert!(!result.empty);
    assert_eq!(result.name, "合成课程");
    assert_eq!(result.credit, Some(2.0));
    assert_eq!(result.grade, "A");
    assert!(runtime.last_course_score_failure_code().is_none());

    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    let lookup = &requests[6];
    assert!(lookup.starts_with(&format!(
        "POST /https/{}/fp/fp/Uniformcommon/selectOnePresetData HTTP/1.1",
        crate::thos::MAPPING
    )));
    let body: serde_json::Value =
        serde_json::from_str(lookup.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["presetKey"], PRESET_KEY);
    assert_eq!(body["param"]["KCH"], "CS0001");
    // The student id is the bound account's, derived inside the Runtime.  It is
    // never an argument of this read and never a field of its result.
    assert_eq!(body["param"]["XH"], STUDENT_ID);
    let rendered = format!("{result:?}");
    assert!(!rendered.contains(STUDENT_ID));
    assert!(!rendered.contains("CS0001"));
    assert!(!rendered.contains("合成课程"));
}

#[tokio::test]
async fn backend_repair_course_score_refused_argument_never_reaches_the_network() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = course_score_runtime(&server, STUDENT_ID);
    // A caller value this client will not send costs no handoff and no request.
    for course_id in ["", "CS 0001", "../../etc/passwd", "CS0001<script>"] {
        let error = runtime
            .load_course_score_result(course_id)
            .await
            .unwrap_err();
        assert!(error.contains("course_score_input"), "error: {error}");
        assert_eq!(
            runtime.last_course_score_failure_code(),
            Some("course_score_input")
        );
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_course_score_requires_a_bound_student_account() {
    // The account is proven and the target session opens, but the bound login
    // id is not an all-digit student id, so it must not be sent as one.
    let server = FixtureServer::new(counted_probe_replies());
    let mut runtime = course_score_runtime(&server, "fixture-owner");
    let error = runtime
        .load_course_score_result("CS0001")
        .await
        .unwrap_err();
    assert!(
        error.contains("course_score_auth_required"),
        "error: {error}"
    );
    assert_eq!(
        runtime.last_course_score_failure_code(),
        Some("course_score_auth_required")
    );
    // The preset query itself never left: the bound account cannot address it.
    assert!(
        server
            .requests()
            .iter()
            .all(|request| !request.contains("selectOnePresetData"))
    );
}

#[tokio::test]
async fn backend_repair_course_score_reports_service_answer_without_empty_success() {
    // The service's own statement that it holds nothing for this course is a
    // successful empty read, not a failure.
    let server = FixtureServer::new(course_score_replies(Reply::json("{}")));
    let mut runtime = course_score_runtime(&server, STUDENT_ID);
    let result = runtime.load_course_score_result("CS0001").await.unwrap();
    assert!(result.empty);
    assert_eq!(result.name, "");
    assert_eq!(result.credit, None);
    assert_eq!(result.grade, "");
    assert!(runtime.last_course_score_failure_code().is_none());

    // A document that is not the expected answer stays an explicit failure.
    let server = FixtureServer::new(course_score_replies(Reply::json(
        "<html>maintenance</html>",
    )));
    let mut runtime = course_score_runtime(&server, STUDENT_ID);
    let error = runtime
        .load_course_score_result("CS0001")
        .await
        .unwrap_err();
    assert!(error.contains("course_score_response"), "error: {error}");
    assert_eq!(
        runtime.last_course_score_failure_code(),
        Some("course_score_response")
    );

    // A refused query is classified through the same diagnostic, and the one
    // pinned handoff is not replayed.
    let server = FixtureServer::new(course_score_replies(Reply::json(
        r#"{"code":401,"message":"请重新登录"}"#,
    )));
    let mut runtime = course_score_runtime(&server, STUDENT_ID);
    let error = runtime
        .load_course_score_result("CS0001")
        .await
        .unwrap_err();
    assert!(
        error.contains("course_score_auth_required"),
        "error: {error}"
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.contains("/wengine-vpn/cookie"))
            .count(),
        1
    );
    assert_eq!(server.requests().len(), 7);
}
