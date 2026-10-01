//! Runtime-level fixtures for the teaching-evaluation list read.
//!
//! These use only loopback responses.  They verify that the list is reachable
//! inside the existing INFO/WebVPN session, that the read addresses the
//! evaluation host's own fixed mapping rather than any handoff query, that the
//! service's closed-window answer reaches the caller as its own state rather
//! than as an empty list, and that the per-row form routes stay inside Rust.

use super::*;

use crate::assessment_read::{ASSESSMENT_LIST_PATH, ASSESSMENT_MIN_SCORE};
use crate::info::OpaqueUrl;
use crate::info_session::{InfoSessionAdapter, InfoWebVpnConfig};
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

/// The mapping token of the teaching-evaluation host, as the allowlist binds it.
const ASSESSMENT_MAPPING: &str =
    "77726476706e69737468656265737421faef469069336153301c9aa596522b20e33c1eb39606919f";

fn assessment_user() -> UserIdentity {
    UserIdentity {
        username: "fixture-assessment-owner".to_owned(),
        display_name: None,
    }
}

/// Installs a proven Identity + INFO session whose adapter talks to `server`.
fn assessment_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime =
        CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap();
    let user = assessment_user();
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            runtime
                .coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-assessment-csrf").unwrap())
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

/// One list row shaped like the legacy table: twelve cells with the course
/// name at index 5, the evaluated flag at 9, and the inline action at 11.
fn list_row(name: &str, evaluated: &str, route: &str) -> String {
    let call = format!("javascript:foo.Bar.Body('{route}') }})");
    format!(
        "<tr><td>1</td><td>2</td><td>3</td><td>4</td><td>5</td><td>{name}</td>\
         <td>7</td><td>8</td><td>9</td><td>{evaluated}</td><td>11</td>\
         <td><a href=\"javascript:void(0)\" onclick=\"{call}\">填写</a></td></tr>"
    )
}

fn list_page() -> String {
    format!(
        "<html><body><table class=\"table\"><tbody>{}{}</tbody></table></body></html>",
        list_row(
            "微积分A(2)",
            "是",
            "/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1001&kcbh=1"
        ),
        list_row(
            "大学物理B(1)",
            "否",
            "/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1002&kcbh=2"
        ),
    )
}

/// The reference reads the questionnaire list at its own absolute mapped URL,
/// with the proven INFO session as the precondition rather than as a request in
/// front of the read.  The list page is therefore the first and only request of
/// a list read, and no portal handoff is dispatched to reach it.
fn assessment_list_replies(page: &str) -> Vec<Reply> {
    vec![Reply::html(page)]
}

/// A form page shaped like the legacy one: the transaction container, the
/// overall comment and score, and one person pane repeated so both the teacher
/// and the assistant group are populated.
fn form_page() -> String {
    let question = |label: &str, score: &str, comment: &str| {
        format!(
            "<tr><td>1</td><td>{label}</td><td>3</td><td>\
             <input name=\"{comment}\" class=\"suggest\" value=\"\">\
             <ul><input name=\"{score}\" value=\"5\"></ul>\
             <input name=\"avg_{score}\" avgfs value=\"5\"></td></tr>"
        )
    };
    let rows = format!(
        "{}{}",
        question("老师教学态度认真负责", "pjfs_1", "jtjy_1"),
        question("老师讲解清楚", "pjfs_2", "jtjy_2")
    );
    let table = format!(
        "<table><tbody><tr><td>张老师</td><td></td><td></td><td></td></tr>{rows}</tbody></table>"
    );
    let pane = format!("<div class=\"tab-pane\">{table}</div>");
    format!(
        "<html><body><div id=\"xswjtxFormid\"><input name=\"wjid\" value=\"1001\"></div>\
         <div id=\"kcpgjgDtos[0].jtjy\">课程内容充实</div>\
         <input id=\"kcpjfs\" name=\"kcpjfs\" value=\"6\">{pane}\
         <div class=\"tab-pane\"></div>{pane}</body></html>"
    )
}

/// The answers the fixture form's shape accepts: one teacher and one assistant,
/// two questions each.
fn fixture_answers(reference: crate::assessment_read::AssessmentRef) -> AssessmentAnswers {
    use crate::assessment_read::{AssessmentPersonAnswers, AssessmentQuestionAnswer};
    let questions = || {
        AssessmentPersonAnswers::new(vec![
            AssessmentQuestionAnswer::new(7, Some("讲得很好".to_owned())).expect("answer"),
            AssessmentQuestionAnswer::new(7, None).expect("answer"),
        ])
    };
    let mut answers = AssessmentAnswers::new(reference, 7).expect("overall score");
    answers.set_people(vec![questions()], vec![questions()]);
    answers
}

#[tokio::test]
async fn backend_repair_assessment_form_reads_through_the_row_reference() {
    let mut replies = assessment_list_replies(&list_page());
    replies.push(Reply::html(&form_page()));
    let server = FixtureServer::new(replies);
    let mut runtime = assessment_runtime(&server);

    let list = runtime
        .load_assessment_list_result()
        .await
        .expect("assessment list reads");
    let form = runtime
        .load_assessment_form_result(&list.items[0].reference)
        .await
        .expect("assessment form reads");

    assert_eq!(form.course, "微积分A(2)");
    assert_eq!(form.score, 6);
    assert_eq!(form.comment.as_deref(), Some("课程内容充实"));
    assert!(form.comment_editable);
    assert_eq!(form.teachers.len(), 1);
    assert_eq!(form.assistants.len(), 1);
    assert!(!form.teachers[0].assistant);
    assert!(form.assistants[0].assistant);
    assert_eq!(form.teachers[0].questions.len(), 2);
    assert_eq!(form.teachers[0].questions[0].text, "老师教学态度认真负责");
    assert_eq!(form.teachers[0].questions[0].score, ASSESSMENT_MIN_SCORE);
    assert!(form.field_count > 0);

    // The form read uses the route the list row named, inside the mapping.
    // Only two requests exist: the list read and the form read.
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].starts_with(&format!(
            "GET /http/{ASSESSMENT_MAPPING}/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1001&kcbh=1 "
        )),
        "unexpected form request: {}",
        requests[1]
    );
}

#[tokio::test]
async fn backend_repair_assessment_submission_applies_answers_and_is_one_shot() {
    let mut replies = assessment_list_replies(&list_page());
    replies.push(Reply::html(&form_page()));
    replies.push(Reply::json("{\"result\":\"success\"}"));
    let server = FixtureServer::new(replies);
    let mut runtime = assessment_runtime(&server);

    let list = runtime
        .load_assessment_list_result()
        .await
        .expect("assessment list reads");
    runtime
        .load_assessment_form_result(&list.items[0].reference)
        .await
        .expect("assessment form reads");

    let answers = fixture_answers(list.items[0].reference.clone());
    runtime
        .submit_assessment_form(&answers)
        .await
        .expect("submission confirmed");

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let post = &requests[2];
    assert!(
        post.starts_with(&format!(
            "POST /http/{ASSESSMENT_MAPPING}{} ",
            crate::assessment_read::ASSESSMENT_SUBMIT_PATH
        )),
        "unexpected submit request: {post}"
    );
    // The service's own transaction state and the caller's answers are both
    // in the body; neither came from the caller's side of the bridge.
    assert!(
        post.contains("wjid=1001"),
        "unexpected submit request: {post}"
    );
    assert!(
        post.contains("pjfs_1=7"),
        "unexpected submit request: {post}"
    );
    assert!(
        post.contains("jtjy_1=%E8%AE%B2%E5%BE%97%E5%BE%88%E5%A5%BD"),
        "unexpected submit request: {post}"
    );

    // The dispatched form is consumed: a second submission has nothing to
    // send and makes no request at all.
    runtime
        .submit_assessment_form(&answers)
        .await
        .expect_err("the dispatched questionnaire is not available again");
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_assessment_unconfirmed_submission_is_not_replayed() {
    let mut replies = assessment_list_replies(&list_page());
    replies.push(Reply::html(&form_page()));
    replies.push(Reply::json("{\"result\":\"error\",\"msg\":\"问卷已提交\"}"));
    let server = FixtureServer::new(replies);
    let mut runtime = assessment_runtime(&server);

    let list = runtime
        .load_assessment_list_result()
        .await
        .expect("assessment list reads");
    runtime
        .load_assessment_form_result(&list.items[0].reference)
        .await
        .expect("assessment form reads");

    let answers = fixture_answers(list.items[0].reference.clone());
    let error = runtime
        .submit_assessment_form(&answers)
        .await
        .expect_err("a declined submission is not a success");
    assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    assert_eq!(
        runtime.last_assessment_failure_code(),
        Some("assessment_submit_unconfirmed")
    );

    // The one dispatched POST is the only one: the outcome is unknown, so it
    // is reported rather than repeated.
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_assessment_rejects_answers_for_a_row_that_is_not_open() {
    let server = FixtureServer::new(assessment_list_replies(&list_page()));
    let mut runtime = assessment_runtime(&server);

    let list = runtime
        .load_assessment_list_result()
        .await
        .expect("assessment list reads");
    // Answers authored for a row, but no form was read for it: nothing is
    // submitted and no request is made.
    let answers = fixture_answers(list.items[0].reference.clone());
    runtime
        .submit_assessment_form(&answers)
        .await
        .expect_err("a questionnaire that was never opened is not submitted");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_assessment_list_reads_inside_the_proven_info_session() {
    let server = FixtureServer::new(assessment_list_replies(&list_page()));
    let mut runtime = assessment_runtime(&server);

    let result = runtime
        .load_assessment_list_result()
        .await
        .expect("assessment list reads");

    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert!(result.error.is_none());
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.items[0].name, "微积分A(2)");
    assert!(result.items[0].evaluated);
    assert_eq!(result.items[1].name, "大学物理B(1)");
    assert!(!result.items[1].evaluated);
    // The bridge row carries only the position, never the form route.
    assert_eq!(result.items[0].reference.index(), 0);
    assert_eq!(result.items[1].reference.index(), 1);
    assert!(runtime.assessment_service_is_proven());
    assert!(runtime.service_session_is_proven(ServiceId::Info));

    // The read addresses the mapping root and the list's own path, and it is
    // the only request a list read dispatches: the proven INFO session is the
    // precondition, not a portal handoff in front of the read.
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let read = &requests[0];
    assert!(
        read.starts_with(&format!(
            "GET /http/{ASSESSMENT_MAPPING}{ASSESSMENT_LIST_PATH} "
        )),
        "unexpected read request: {read}"
    );
}

#[tokio::test]
async fn backend_repair_assessment_closed_window_is_not_an_empty_list() {
    // The window is closed: the service answers 200 with its own notice.  The
    // caller must see that state, not a validated empty list.
    let server = FixtureServer::new(assessment_list_replies(&format!(
        "<html><body>{}</body></html>",
        crate::assessment_read::ASSESSMENT_NOT_OPEN_MARKER
    )));
    let mut runtime = assessment_runtime(&server);

    let error = runtime
        .load_assessment_list_result()
        .await
        .expect_err("a closed window is not an empty list");
    assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    assert!(!runtime.assessment_service_is_proven());
}

#[tokio::test]
async fn backend_repair_assessment_failure_is_not_a_stale_list() {
    // The handoff succeeds but the list itself is an expiry page.  A list from
    // an earlier read must not be presented as the current one.
    let server = FixtureServer::new(assessment_list_replies(
        "time out用户登陆超时或访问内容不存在。请重试",
    ));
    let mut runtime = assessment_runtime(&server);

    let error = runtime
        .load_assessment_list_result()
        .await
        .expect_err("an expiry page is a failure");
    assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    assert!(!runtime.assessment_service_is_proven());
}

#[tokio::test]
async fn backend_repair_assessment_requires_a_proven_account() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = assessment_runtime(&server);
    runtime.invalidate_service_session(ServiceId::Identity);
    runtime.invalidate_service_session(ServiceId::Info);

    assert!(
        runtime.load_assessment_list_result().await.is_err(),
        "an unproven account must not read a list"
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_assessment_routes_do_not_survive_the_session() {
    let server = FixtureServer::new(assessment_list_replies(&list_page()));
    let mut runtime = assessment_runtime(&server);

    runtime
        .load_assessment_list_result()
        .await
        .expect("assessment list reads");
    let generation = runtime
        .assessment_adapter
        .as_ref()
        .expect("adapter installed")
        .route_generation();
    assert_eq!(generation, 1);

    // Invalidating the INFO session drops the adapter, and with it the route
    // table: no reference handed out earlier can outlive the session it came
    // from.
    runtime.invalidate_service_session(ServiceId::Info);
    assert!(runtime.assessment_adapter.is_none());
}
