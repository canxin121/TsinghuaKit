//! Loopback fixtures and shape tests for the teaching-evaluation list slice.

use std::time::Duration;

use crate::assessment_read::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

use reqwest::Url;

/// The mapping root the runtime installs for this host.
const MAPPING: &str =
    "/http/77726476706e69737468656265737421faef469069336153301c9aa596522b20e33c1eb39606919f";

/// The inline action call the legacy page writes, `Body('…') })`.
fn action_cell(route: &str) -> String {
    let call = format!("javascript:foo.Bar.Body('{route}') }})");
    format!("<td><a href=\"javascript:void(0)\" onclick=\"{call}\">填写</a></td>")
}

/// One list row shaped like the legacy table: twelve cells with the course
/// name at index 5, the evaluated flag at 9, and the inline action at 11.
fn list_row(name: &str, evaluated: &str, route: &str) -> String {
    format!(
        "<tr><td>1</td><td>2</td><td>3</td><td>4</td><td>5</td><td>{name}</td>\
         <td>7</td><td>8</td><td>9</td><td>{evaluated}</td><td>11</td>{}</tr>",
        action_cell(route)
    )
}

/// A page holding two courses, one evaluated and one not.
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

fn adapter(server: &FixtureServer) -> AssessmentAdapter {
    let base = Url::parse(&format!("{}{MAPPING}", server.base())).unwrap();
    AssessmentAdapter::try_with_transport(
        base,
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap()
}

#[test]
fn list_rows_are_read_by_position_with_a_structural_action_anchor() {
    let parsed = parse_assessment_list_html(&list_page()).expect("list parses");
    assert_eq!(parsed.rows.len(), 2);
    assert_eq!(parsed.rows[0].name, "微积分A(2)");
    assert!(parsed.rows[0].evaluated);
    assert_eq!(parsed.rows[1].name, "大学物理B(1)");
    assert!(!parsed.rows[1].evaluated);
    assert_eq!(
        parsed.rows[1].route,
        "/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1002&kcbh=2"
    );
}

#[test]
fn an_empty_list_is_a_failure_not_an_empty_result() {
    let page = "<html><body><table><tbody></tbody></table></body></html>";
    assert_eq!(
        parse_assessment_list_html(page).unwrap_err(),
        AssessmentParseError::EmptyList
    );
}

#[test]
fn a_closed_window_is_its_own_error_category() {
    let page = format!("<html><body><div>{ASSESSMENT_NOT_OPEN_MARKER}</div></body></html>");
    let error = parse_assessment_list_html(&page).unwrap_err();
    assert_eq!(error, AssessmentParseError::NotOpen);
    assert!(error.is_not_open());
    assert!(!error.is_session_expired());
}

#[test]
fn a_login_and_an_expiry_page_are_session_failures() {
    let login = parse_assessment_list_html("<title>清华大学WebVPN</title>").unwrap_err();
    assert_eq!(login, AssessmentParseError::LoginPage);
    assert!(login.is_session_expired());

    let expired = parse_assessment_list_html("用户登陆超时或访问内容不存在。请重试").unwrap_err();
    assert_eq!(expired, AssessmentParseError::ExpiredPage);
    assert!(expired.is_session_expired());
}

#[test]
fn a_row_without_the_inline_action_is_rejected() {
    // The columns moved: cell 11 holds text instead of the `Body('…')` call.
    let page = "<html><body><table><tbody>\
        <tr><td>1</td><td>2</td><td>3</td><td>4</td><td>5</td><td>微积分A(2)</td>\
        <td>7</td><td>8</td><td>9</td><td>是</td><td>11</td><td>没有动作</td></tr>\
        </tbody></table></body></html>";
    assert_eq!(
        parse_assessment_list_html(page).unwrap_err(),
        AssessmentParseError::MissingAction { row: 0 }
    );
}

#[test]
fn a_row_naming_a_route_outside_the_service_is_rejected() {
    let page = format!(
        "<html><body><table><tbody>{}</tbody></table></body></html>",
        list_row("微积分A(2)", "是", "https://example.invalid/steal")
    );
    assert_eq!(
        parse_assessment_list_html(&page).unwrap_err(),
        AssessmentParseError::InvalidRoute { row: 0 }
    );
}

#[test]
fn a_row_that_is_too_short_is_an_unrecognized_layout() {
    let page = "<html><body><table><tbody>\
        <tr><td>1</td><td>2</td><td>3</td></tr>\
        </tbody></table></body></html>";
    assert_eq!(
        parse_assessment_list_html(page).unwrap_err(),
        AssessmentParseError::UnrecognizedRow { row: 0 }
    );
}

#[test]
fn a_page_without_a_table_body_is_reported_instead_of_parsed_as_empty() {
    let page = "<html><body><table class=\"table\"><tr><td>x</td></tr></table></body></html>";
    assert_eq!(
        parse_assessment_list_html(page).unwrap_err(),
        AssessmentParseError::MissingTable
    );
}

#[tokio::test]
async fn the_adapter_reads_the_list_through_the_cookie_aware_transport() {
    let server = FixtureServer::new(vec![Reply::html(&list_page())]);
    let adapter = adapter(&server);

    let list = adapter.read_list().await.expect("list reads");
    assert_eq!(list.len(), 2);
    assert_eq!(list.items[0].name, "微积分A(2)");
    assert!(list.items[0].evaluated);
    assert!(!list.items[1].evaluated);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].starts_with(&format!("GET {MAPPING}{ASSESSMENT_LIST_PATH} ")),
        "unexpected request: {}",
        requests[0]
    );
}

#[tokio::test]
async fn a_row_reference_only_resolves_inside_the_adapter_that_read_it() {
    let server = FixtureServer::new(vec![Reply::html(&list_page())]);
    let first = adapter(&server);
    let other_base = Url::parse(&format!("{}{MAPPING}", server.base())).unwrap();
    let second = AssessmentAdapter::try_with_transport(
        other_base,
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap();

    let list = first.read_list().await.expect("list reads");
    let reference = list.items[1].reference.clone();
    assert_eq!(
        first.form_path(&reference).as_deref(),
        Some("/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1002&kcbh=2")
    );
    // A different adapter instance shares no route table, so the reference is
    // inert rather than pointing at whatever it happens to hold.
    assert!(second.form_path(&reference).is_none());

    // The row index is the only thing the reference discloses.
    assert_eq!(reference.index(), 1);
    assert_eq!(format!("{reference:?}"), "AssessmentRef { index: 1 }");
}

#[tokio::test]
async fn a_reference_from_a_superseded_list_no_longer_resolves() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&format!(
            "<html><body><table><tbody>{}</tbody></table></body></html>",
            list_row("新课程", "否", "/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=2001")
        )),
    ]);
    let adapter = adapter(&server);
    let stale = adapter.read_list().await.expect("first read");
    let stale_reference = stale.items[0].reference.clone();
    let generation = adapter.route_generation();

    let fresh = adapter.read_list().await.expect("second read");
    assert_eq!(fresh.len(), 1);
    assert!(adapter.route_generation() > generation);
    assert!(adapter.form_path(&stale_reference).is_none());
    assert_eq!(
        adapter.form_path(&fresh.items[0].reference).as_deref(),
        Some("/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=2001")
    );
}

#[tokio::test]
async fn a_closed_window_reaches_the_caller_as_a_not_open_failure() {
    let server = FixtureServer::new(vec![Reply::html(&format!(
        "<html><body>{ASSESSMENT_NOT_OPEN_MARKER}</body></html>"
    ))]);
    let adapter = adapter(&server);

    let error = adapter.read_list().await.expect_err("closed window fails");
    assert!(error.is_not_open(), "unexpected error: {error:?}");
    assert!(!error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "assessment_not_open");
    assert_eq!(adapter.route_generation(), 0);
}

#[tokio::test]
async fn the_adapter_reports_a_parse_failure_rather_than_an_empty_list() {
    let server = FixtureServer::new(vec![Reply::html(
        "<html><body><table><tbody></tbody></table></body></html>",
    )]);
    let adapter = adapter(&server);

    let error = adapter.read_list().await.expect_err("empty list fails");
    assert_eq!(error.diagnostic_code(), "assessment_list_empty");
    assert_eq!(adapter.route_generation(), 0);
}

#[tokio::test]
async fn a_non_html_answer_is_not_parsed_as_a_list() {
    let server = FixtureServer::new(vec![Reply::json("{}")]);
    let adapter = adapter(&server);

    let error = adapter.read_list().await.expect_err("json fails");
    assert_eq!(error.diagnostic_code(), "assessment_content_type");
}

#[test]
fn the_adapter_debug_output_carries_no_route_or_account_value() {
    let server = FixtureServer::new(vec![]);
    let adapter = adapter(&server);
    let rendered = format!("{adapter:?}");
    assert!(rendered.contains("AssessmentAdapter"));
    // Neither the mapping token nor a service route appears in the
    // diagnostic rendering.
    assert!(!rendered.contains("jxpg"));
    assert!(!rendered.contains("faef4690"));
}

// --- The form read and the submission -------------------------------------

/// The transaction container a form page must carry.
fn form_page(transaction: &str, panes: &str) -> String {
    format!(
        "<html><body><div id=\"xswjtxFormid\">{transaction}</div>\
         <div id=\"kcpgjgDtos[0].jtjy\">课程内容充实</div>\
         <input id=\"kcpjfs\" name=\"kcpjfs\" value=\"6\">{panes}</body></html>"
    )
}

/// One question row: four cells, the label in the second, the inputs in the
/// fourth — the layout the reference reads.
fn question_row(label: &str, score_name: &str, comment_name: &str) -> String {
    format!(
        "<tr><td>1</td><td>{label}</td><td>3</td><td>\
         <input name=\"{comment_name}\" class=\"suggest\" value=\"\">\
         <ul><input name=\"{score_name}\" value=\"5\"></ul>\
         <input name=\"avg_{score_name}\" avgfs value=\"5\"></td></tr>"
    )
}

/// One person table: a name row, then the given question rows.
fn person_table(person: &str, rows: &str) -> String {
    format!(
        "<table><tbody><tr><td>{person}</td><td></td><td></td><td></td></tr>{rows}</tbody></table>"
    )
}

/// A form page for one teacher with two questions.
fn full_form_page() -> String {
    let teacher = person_table(
        "张老师",
        &format!(
            "{}{}",
            question_row("老师教学态度认真负责", "pjfs_1", "jtjy_1"),
            question_row("老师讲解清楚", "pjfs_2", "jtjy_2")
        ),
    );
    let pane = format!("<div class=\"tab-pane\">{teacher}</div>");
    // The assistant pane is the third `tab-pane` in the page, which is why the
    // adapter reads the first and the third.
    let panes = format!("{pane}<div class=\"tab-pane\"></div>{pane}");
    form_page("<input name=\"wjid\" value=\"1001\">", &panes)
}

#[test]
fn a_form_is_parsed_into_its_transaction_its_overall_value_and_its_people() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    assert_eq!(parsed.transaction.len(), 1);
    assert_eq!(parsed.transaction[0].value(), "1001");
    assert_eq!(parsed.score, 6);
    assert_eq!(
        parsed.suggestion.as_ref().map(AssessmentFormField::value),
        Some("课程内容充实")
    );
    assert_eq!(parsed.teachers.len(), 1);
    let teacher = &parsed.teachers[0];
    assert_eq!(teacher.name(), "张老师");
    assert_eq!(teacher.role(), AssessmentPersonRole::Teacher);
    assert_eq!(teacher.question_count(), 2);
    assert_eq!(teacher.question(0), Some("老师教学态度认真负责"));
    assert_eq!(teacher.question_score(0), Some(ASSESSMENT_MIN_SCORE));
    assert_eq!(teacher.question_suggestion(0), Some(""));
    // The `avgfs` box is carried with its question rather than dropped.
    assert_eq!(teacher.question_field_count(0), Some(3));
}

#[test]
fn a_form_without_its_transaction_identifier_is_refused() {
    let page = form_page("<input name=\"other\" value=\"1\">", "");
    let error = parse_assessment_form_html(&page).unwrap_err();
    assert_eq!(error, AssessmentParseError::MissingForm);
}

#[test]
fn a_form_with_no_questions_is_refused_rather_than_returned_empty() {
    let page = form_page("<input name=\"wjid\" value=\"1001\">", "");
    assert_eq!(
        parse_assessment_form_html(&page).unwrap_err(),
        AssessmentParseError::EmptyForm
    );
}

#[test]
fn a_score_outside_the_service_range_is_refused_locally() {
    let mut parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let teacher = &mut parsed.teachers[0];
    assert_eq!(
        teacher.set_question_score(0, 0).unwrap_err(),
        AssessmentInputError::ScoreOutOfRange
    );
    assert_eq!(
        teacher.set_question_score(0, 8).unwrap_err(),
        AssessmentInputError::ScoreOutOfRange
    );
    assert!(teacher.set_question_score(0, 7).is_ok());
    assert_eq!(teacher.question_score(0), Some(7));

    // A question this person was not asked cannot be scored either.
    assert_eq!(
        teacher.set_question_score(9, 7).unwrap_err(),
        AssessmentInputError::UnknownQuestion
    );
}

#[test]
fn the_submission_body_carries_the_service_transaction_state_verbatim() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let form = parsed.into_form();
    let body = form.serialize();
    assert!(body.starts_with("wjid=1001&"), "unexpected body: {body}");
    assert!(body.contains("kcpjfs=6"), "unexpected body: {body}");
    assert!(body.contains("pjfs_1=1"), "unexpected body: {body}");
    assert!(body.contains("pjfs_2=1"), "unexpected body: {body}");
    // The comment keeps the name the service rendered, percent-encoded.
    assert!(body.contains("jtjy_1="), "unexpected body: {body}");
    assert_eq!(
        body.split('&').count(),
        form.field_count(),
        "unexpected body: {body}"
    );
}

#[test]
fn a_comment_is_encoded_rather_than_rejected_and_a_control_character_is_not() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let mut form = parsed.into_form();
    // Separators a student may type are ordinary text; they are encoded.
    assert!(form.set_suggestion("a&b=c").is_ok());
    assert!(form.serialize().contains("a%26b%3Dc"));
    // A value no form could carry is refused instead.
    assert_eq!(
        form.set_suggestion("line\rbreak").unwrap_err(),
        AssessmentInputError::InvalidValue
    );
    assert!(form.set_suggestion("内容很好").is_ok());
    assert!(form.serialize().contains("kcpjfs=6"));
}

#[test]
fn a_comment_with_spaces_and_unicode_is_percent_encoded_as_utf8() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let mut form = parsed.into_form();
    form.teachers_mut()[0]
        .set_question_suggestion(0, "讲得很好 继续")
        .unwrap();
    form.set_suggestion("总体不错").unwrap();
    let body = form.serialize();
    assert!(
        body.contains("jtjy_1=%E8%AE%B2%E5%BE%97%E5%BE%88%E5%A5%BD%20%E7%BB%A7%E7%BB%AD"),
        "unexpected body: {body}"
    );
    // The overall comment keeps the service's own bracketed name, with the
    // brackets encoded the way the reference encoder encodes them.
    assert!(
        body.contains("kcpgjgDtos%5B0%5D.jtjy=%E6%80%BB%E4%BD%93%E4%B8%8D%E9%94%99"),
        "unexpected body: {body}"
    );
    assert!(body.contains("kcpjfs=6"), "unexpected body: {body}");
}

#[tokio::test]
async fn a_form_read_uses_the_route_the_list_named_and_not_a_caller_path() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&full_form_page()),
    ]);
    let adapter = adapter(&server);
    let list = adapter.read_list().await.expect("list reads");

    let form = adapter
        .read_form(&list.items[1].reference)
        .await
        .expect("form reads");
    assert_eq!(form.course(), "大学物理B(1)");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].starts_with(&format!(
            "GET {MAPPING}/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=1002&kcbh=2 "
        )),
        "unexpected request: {}",
        requests[1]
    );
}

#[tokio::test]
async fn a_form_reference_from_a_superseded_list_is_refused() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&format!(
            "<html><body><table><tbody>{}</tbody></table></body></html>",
            list_row("新课程", "否", "/jxpg/f/jxpg/wj/xs/pgkcForm?wjid=2001")
        )),
    ]);
    let adapter = adapter(&server);
    let list = adapter.read_list().await.expect("list reads");
    let stale = list.items[0].reference.clone();
    adapter.read_list().await.expect("second read");

    let error = adapter.read_form(&stale).await.expect_err("stale refused");
    assert_eq!(error.diagnostic_code(), "assessment_form_stale");
}

#[tokio::test]
async fn a_form_reference_from_another_adapter_is_refused() {
    let server = FixtureServer::new(vec![Reply::html(&list_page())]);
    let first = adapter(&server);
    let other_base = Url::parse(&format!("{}{MAPPING}", server.base())).unwrap();
    let second = AssessmentAdapter::try_with_transport(
        other_base,
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap();

    let list = first.read_list().await.expect("list reads");
    let error = second
        .read_form(&list.items[0].reference)
        .await
        .expect_err("foreign refused");
    assert_eq!(error.diagnostic_code(), "assessment_form_foreign");
    // No request was made for it.
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_submission_the_service_confirms_is_reported_as_complete() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&full_form_page()),
        Reply::json("{\"result\":\"success\"}"),
    ]);
    let adapter = adapter(&server);
    let list = adapter.read_list().await.expect("list reads");
    let mut evaluation = adapter
        .read_form(&list.items[0].reference)
        .await
        .expect("form reads");
    evaluation.set_all_scores(7).expect("scores set");

    adapter
        .submit_form(&evaluation)
        .await
        .expect("submission confirmed");
    assert!(!adapter.has_unconfirmed_submission());

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let post = &requests[2];
    assert!(
        post.starts_with(&format!("POST {MAPPING}{ASSESSMENT_SUBMIT_PATH} ")),
        "unexpected request: {post}"
    );
    assert!(post.contains("pjfs_1=7"), "unexpected request: {post}");
}

#[tokio::test]
async fn a_submission_the_service_declines_is_unconfirmed_and_is_not_replayed() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&full_form_page()),
        Reply::json("{\"result\":\"error\",\"msg\":\"问卷已提交\"}"),
    ]);
    let adapter = adapter(&server);
    let list = adapter.read_list().await.expect("list reads");
    let evaluation = adapter
        .read_form(&list.items[0].reference)
        .await
        .expect("form reads");

    let error = adapter
        .submit_form(&evaluation)
        .await
        .expect_err("declined is not success");
    assert!(error.is_unconfirmed());
    assert_eq!(error.diagnostic_code(), "assessment_submit_unconfirmed");
    assert!(adapter.has_unconfirmed_submission());

    // The same row cannot be dispatched again from this generation.
    let second = adapter
        .submit_form(&evaluation)
        .await
        .expect_err("second dispatch refused");
    assert_eq!(second.diagnostic_code(), "assessment_submit_replayed");
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn a_reworded_submission_answer_is_not_promoted_to_success() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&full_form_page()),
        Reply::html("<html><body>操作成功</body></html>"),
    ]);
    let adapter = adapter(&server);
    let list = adapter.read_list().await.expect("list reads");
    let evaluation = adapter
        .read_form(&list.items[0].reference)
        .await
        .expect("form reads");

    let error = adapter
        .submit_form(&evaluation)
        .await
        .expect_err("html is not a confirmation");
    assert!(error.is_unconfirmed());
}

#[tokio::test]
async fn a_submission_of_one_row_does_not_block_another_row() {
    let server = FixtureServer::new(vec![
        Reply::html(&list_page()),
        Reply::html(&full_form_page()),
        Reply::html(&full_form_page()),
        Reply::json("{\"result\":\"success\"}"),
        Reply::json("{\"result\":\"success\"}"),
    ]);
    let adapter = adapter(&server);
    let list = adapter.read_list().await.expect("list reads");
    let first = adapter
        .read_form(&list.items[0].reference)
        .await
        .expect("first form");
    let second = adapter
        .read_form(&list.items[1].reference)
        .await
        .expect("second form");

    adapter.submit_form(&first).await.expect("first submits");
    adapter.submit_form(&second).await.expect("second submits");
    assert_eq!(server.requests().len(), 5);
}

#[test]
fn the_form_debug_output_carries_no_transaction_value_or_route() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let form = parsed.into_form();
    let rendered = format!("{form:?}");
    assert!(rendered.contains("AssessmentForm"));
    assert!(!rendered.contains("1001"));
    assert!(!rendered.contains("wjid"));
    assert!(!rendered.contains("张老师"));
}

// --- The display copy and the answers a caller supplies --------------------

#[test]
fn a_view_carries_the_questions_and_the_answers_the_service_holds() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let evaluation =
        AssessmentEvaluation::for_test(0, 1, 0, "大学物理B(1)".to_owned(), parsed.into_form());
    let view = evaluation.view();
    assert_eq!(view.course(), "大学物理B(1)");
    assert_eq!(view.score(), 6);
    assert_eq!(view.suggestion(), Some("课程内容充实"));
    assert_eq!(view.teachers().len(), 1);
    let teacher = &view.teachers()[0];
    assert_eq!(teacher.name(), "张老师");
    assert_eq!(teacher.role(), AssessmentPersonRole::Teacher);
    assert_eq!(teacher.question_count(), 2);
    assert_eq!(
        teacher.question(0).map(|q| q.text()),
        Some("老师教学态度认真负责")
    );
    assert_eq!(
        teacher.question(0).map(|q| q.score()),
        Some(ASSESSMENT_MIN_SCORE)
    );
    // The service rendered an empty comment box, which is not the same as no
    // comment box at all.
    assert_eq!(teacher.question(0).and_then(|q| q.suggestion()), Some(""));
    assert_eq!(teacher.question(9), None);
}

#[test]
fn an_answer_set_is_applied_to_the_form_it_was_authored_for() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let mut form = parsed.into_form();
    let reference = AssessmentRef::new(0, 1, 0);
    let mut answers = AssessmentAnswers::new(reference.clone(), 6).expect("overall score");
    answers.set_suggestion("总体不错").expect("overall comment");
    let question_answers = vec![
        AssessmentQuestionAnswer::new(7, Some("讲得很好".to_owned())).expect("first answer"),
        AssessmentQuestionAnswer::new(6, None).expect("second answer"),
    ];
    // The fixture page's third pane repeats the first, so the form asks about
    // one assistant as well as one teacher; both groups are answered.
    answers.set_people(
        vec![AssessmentPersonAnswers::new(question_answers.clone())],
        vec![AssessmentPersonAnswers::new(question_answers)],
    );
    assert_eq!(form.assistants().len(), 1);
    answers.apply_to(&mut form).expect("answers apply");

    assert_eq!(form.score(), 6);
    assert_eq!(form.suggestion(), Some("总体不错"));
    assert_eq!(form.teachers()[0].question_score(0), Some(7));
    assert_eq!(form.teachers()[0].question_suggestion(0), Some("讲得很好"));
    // A `None` comment leaves the value the service held untouched.
    assert_eq!(form.teachers()[0].question_score(1), Some(6));
    assert_eq!(form.teachers()[0].question_suggestion(1), Some(""));
    let body = form.serialize();
    assert!(body.contains("pjfs_1=7"), "unexpected body: {body}");
    assert!(
        body.contains("jtjy_1=%E8%AE%B2%E5%BE%97%E5%BE%88%E5%A5%BD"),
        "unexpected body: {body}"
    );
    // The reference an answer set names is the one the view carried.
    assert_eq!(answers.reference(), reference);
}

#[test]
fn an_answer_set_that_does_not_match_the_form_shape_is_refused() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let mut form = parsed.into_form();
    let reference = AssessmentRef::new(0, 1, 0);

    // Two people were answered for, but the form asks about one.
    let mut answers = AssessmentAnswers::new(reference.clone(), 6).expect("overall score");
    answers.set_people(
        vec![
            AssessmentPersonAnswers::new(vec![]),
            AssessmentPersonAnswers::new(vec![]),
        ],
        vec![],
    );
    assert_eq!(
        answers.apply_to(&mut form).unwrap_err(),
        AssessmentInputError::UnknownQuestion
    );

    // One question was answered for, but this person was asked two.
    let mut answers = AssessmentAnswers::new(reference, 6).expect("overall score");
    answers.set_people(
        vec![AssessmentPersonAnswers::new(vec![
            AssessmentQuestionAnswer::new(7, None).expect("answer"),
        ])],
        vec![],
    );
    assert_eq!(
        answers.apply_to(&mut form).unwrap_err(),
        AssessmentInputError::UnknownQuestion
    );

    // A score the service does not accept never becomes an answer.
    assert_eq!(
        AssessmentQuestionAnswer::new(0, None).unwrap_err(),
        AssessmentInputError::ScoreOutOfRange
    );
    assert_eq!(
        AssessmentQuestionAnswer::new(8, None).unwrap_err(),
        AssessmentInputError::ScoreOutOfRange
    );
    assert_eq!(
        AssessmentAnswers::new(AssessmentRef::new(0, 1, 0), 8).unwrap_err(),
        AssessmentInputError::ScoreOutOfRange
    );
    assert_eq!(
        AssessmentQuestionAnswer::new(7, Some("line\rbreak".to_owned())).unwrap_err(),
        AssessmentInputError::InvalidValue
    );
}

#[test]
fn the_view_and_the_answers_carry_no_submission_state_in_their_debug_output() {
    let parsed = parse_assessment_form_html(&full_form_page()).expect("form parses");
    let evaluation =
        AssessmentEvaluation::for_test(0, 1, 0, "大学物理B(1)".to_owned(), parsed.into_form());
    let view = format!("{:?}", evaluation.view());
    assert!(view.contains("AssessmentFormView"));
    assert!(!view.contains("1001"));
    assert!(!view.contains("wjid"));
    assert!(!view.contains("张老师"));
    assert!(!view.contains("课程内容充实"));

    let answers = AssessmentAnswers::new(AssessmentRef::new(0, 1, 0), 7).expect("overall score");
    let rendered = format!("{answers:?}");
    assert!(rendered.contains("AssessmentAnswers"));
    assert!(!rendered.contains("1001"));
    assert_eq!(format!("{:?}", answers.teachers()), "[]");
}
