//! Loopback fixtures and shape tests for the degree-program read slice.

use std::time::Duration;

use crate::program_read::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

use reqwest::Url;

/// One synthetic completion report shaped like the JSP output: the summary
/// block, then the `.table-striped` course table, then the out-of-plan block.
fn completion_page() -> String {
    let mut html = String::from(
        "<html><body><div class=\"p-fbox\">\
        <b>方案内实际完成 <strong>总学分：168.5</strong> \
        其中必修完成总学分：<strong>100</strong> \
        限选完成总学分：<strong>38.5</strong> \
        任选（方案内）完成总学分：<strong>30</strong> \
        重复课程：A、B 属于多个课组</b></div>",
    );
    html.push_str(
        "<table class=\"table-striped\"><tr>\
        <td>属性</td><td>课组</td><td>课号</td><td>课程名</td><td>学分</td>\
        <td>成绩/状态</td><td>绩点</td><td></td><td></td><td></td><td></td><td>是否完成</td>\
        </tr>",
    );
    // An attribute section row: 12 cells, name at 1, first course at 2..6,
    // requirements immediately before the flag at 11.
    html.push_str(
        "<tr>\
        <td>必修</td><td>专业核心课</td><td>1000001</td><td>高等数学</td><td>5</td>\
        <td>A</td><td>4.0</td><td>20</td><td>20</td><td>5</td><td>5</td><td>是</td>\
        </tr>",
    );
    // A further course row of the same set.
    html.push_str("<tr><td>1000002</td><td>线性代数</td><td>3</td><td>B+</td><td>3.3</td></tr>");
    // A sub-group row: name at 0, first course at 1..5, flag at 10.
    html.push_str(
        "<tr>\
        <td>专业选修组</td><td>2000001</td><td>机器学习</td><td>3</td>\
        <td>选课</td><td></td><td>12</td><td>6</td><td>4</td><td>2</td><td>否</td>\
        </tr>",
    );
    // An unfinished course row, which carries no grade point.
    html.push_str("<tr><td>2000002</td><td>计算机视觉</td><td>3</td><td>未修</td><td></td></tr>");
    html.push_str("</table>");
    // The out-of-plan table: its own labeled header, then the course rows.  A
    // course row's identifier cell is not leading, so the shape differs from
    // the in-plan courses.
    html.push_str(
        "<table class=\"table-striped\"><tr><td>课号</td><td>课程名</td><td>学分</td>\
        <td>属性</td><td>说明</td><td>成绩</td><td>绩点</td></tr>",
    );
    html.push_str(
        "<tr><td>3000001</td><td>物理系</td><td>大学物理</td><td>4</td><td></td><td>A</td>\
        <td>4.0</td></tr>",
    );
    html.push_str("</table>");
    html.push_str(
        "<div>本科生已修培养方案外课程完成总学分：<strong>4</strong></div></body></html>",
    );
    html
}

fn completion_table_only(body: &str) -> String {
    format!("<html><body>{body}</body></html>")
}

#[test]
fn completion_report_parses_summary_sets_and_out_of_plan_group() {
    let page = completion_page();
    let completion = parse_program_completion_html(&page).expect("completion report parses");

    assert_eq!(completion.completed_credit, 168.5);
    assert_eq!(completion.compulsory_credit, 100.0);
    assert_eq!(completion.restricted_credit, 38.5);
    assert_eq!(completion.elective_credit, 30.0);
    assert_eq!(completion.duplicated_courses, vec!["A", "B"]);
    assert_eq!(completion.excluded_credit, Some(4.0));

    assert_eq!(completion.course_sets.len(), 3);
    let compulsory = &completion.course_sets[0];
    assert_eq!(compulsory.name, "专业核心课");
    assert_eq!(compulsory.kind, CourseSetKind::Compulsory);
    assert_eq!(compulsory.required_credit, Some(20.0));
    assert_eq!(compulsory.completed_credit, Some(20.0));
    assert_eq!(compulsory.required_course_count, Some(5));
    assert_eq!(compulsory.completed_course_count, Some(5));
    assert!(compulsory.full_completed);
    assert_eq!(compulsory.courses.len(), 2);
    assert_eq!(compulsory.courses[0].course_id, "1000001");
    assert_eq!(compulsory.courses[0].name, "高等数学");
    assert_eq!(compulsory.courses[0].credit, 5.0);
    assert_eq!(compulsory.courses[0].point, Some(4.0));
    assert_eq!(compulsory.courses[0].state, CourseState::Completed);

    let group = &completion.course_sets[1];
    assert_eq!(group.name, "专业选修组");
    // A sub-group inherits the attribute of the section it sits under.
    assert_eq!(group.kind, CourseSetKind::Compulsory);
    assert_eq!(group.required_credit, Some(12.0));
    assert!(!group.full_completed);
    assert_eq!(group.courses.len(), 2);
    assert_eq!(group.courses[0].state, CourseState::Elected);
    assert_eq!(group.courses[0].grade, None);
    assert_eq!(group.courses[1].state, CourseState::NotCompleted);
    assert_eq!(group.courses[1].point, None);

    let excluded = &completion.course_sets[2];
    assert_eq!(excluded.name, "方案外课程");
    assert_eq!(excluded.kind, CourseSetKind::Excluded);
    assert_eq!(excluded.completed_credit, Some(4.0));
    assert!(!excluded.full_completed);
    assert_eq!(excluded.courses.len(), 1);
    assert_eq!(excluded.courses[0].course_id, "3000001");
    assert_eq!(excluded.courses[0].name, "大学物理");
    assert_eq!(excluded.courses[0].grade.as_deref(), Some("A"));
}

#[test]
fn withdrawal_marks_are_not_completed_courses() {
    let page = completion_page();
    let without = page.replace(
        "<tr><td>2000002</td><td>计算机视觉</td><td>3</td><td>未修</td><td></td></tr>",
        "<tr><td>2000002</td><td>计算机视觉</td><td>3</td><td>W</td><td></td></tr>",
    );
    let completion = parse_program_completion_html(&without).expect("withdrawn course parses");
    let course = &completion.course_sets[1].courses[1];
    assert_eq!(course.state, CourseState::NotCompleted);
    assert_eq!(course.grade.as_deref(), Some("W"));
    assert_eq!(course.point, None);
}

#[test]
fn unknown_attribute_section_is_rejected_not_defaulted() {
    // The attribute cell is the only place the graduation requirement is
    // stated, so an unfamiliar value must not be reported as an elective.
    let body = completion_table_only(
        "<table class=\"table-striped\"><tr><td>属性</td><td>课组</td><td>课号</td></tr>\
        <tr><td>任意选修</td><td>某课组</td><td>1000001</td><td>某课程</td><td>2</td>\
        <td>A</td><td>4.0</td><td>10</td><td>10</td><td>1</td><td>1</td><td>是</td></tr></table>\
        <div>方案内实际完成 <strong>总学分：2</strong> 其中必修完成总学分：<strong>0</strong> \
        限选完成总学分：<strong>0</strong> 任选（方案内）完成总学分：<strong>0</strong> \
        重复课程：无 属于多个课组</div>",
    );
    assert_eq!(
        parse_program_completion_html(&body).unwrap_err(),
        ProgramParseError::UnrecognizedCourseSet { row: 0 }
    );
}

#[test]
fn unrecognized_course_row_is_an_error_not_a_smaller_list() {
    let body = completion_table_only(
        "<table class=\"table-striped\"><tr><td>属性</td><td>课号</td></tr>\
        <tr><td></td><td></td><td></td></tr></table>\
        <div>方案内实际完成 <strong>总学分：0</strong> 其中必修完成总学分：<strong>0</strong> \
        限选完成总学分：<strong>0</strong> 任选（方案内）完成总学分：<strong>0</strong> \
        重复课程：无 属于多个课组</div>",
    );
    assert_eq!(
        parse_program_completion_html(&body).unwrap_err(),
        ProgramParseError::UnrecognizedCourseRow { row: 0 }
    );
}

#[test]
fn missing_summary_marker_is_an_error() {
    let body = completion_table_only(
        "<table class=\"table-striped\"><tr><td>属性</td><td>课号</td></tr>\
        <tr><td>1000001</td><td>课程</td><td>2</td><td>A</td><td>4.0</td></tr></table>",
    );
    assert_eq!(
        parse_program_completion_html(&body).unwrap_err(),
        ProgramParseError::MissingMarker {
            marker: "方案内实际完成"
        }
    );
}

#[test]
fn a_page_without_the_completion_table_is_an_error() {
    let body = completion_table_only(
        "<div>方案内实际完成 <strong>总学分：0</strong> 其中必修完成总学分：<strong>0</strong> \
        限选完成总学分：<strong>0</strong> 任选（方案内）完成总学分：<strong>0</strong> \
        重复课程：无 属于多个课组</div>",
    );
    assert_eq!(
        parse_program_completion_html(&body).unwrap_err(),
        ProgramParseError::MissingCompletionTable
    );
}

#[test]
fn login_and_expiry_pages_are_reported_as_session_failures() {
    let login = "<html><head><title>清华大学WebVPN</title></head><body></body></html>";
    assert_eq!(
        parse_program_completion_html(login).unwrap_err(),
        ProgramParseError::LoginPage
    );
    let expired = "time out用户登陆超时或访问内容不存在。请重试";
    assert_eq!(
        parse_program_completion_html(expired).unwrap_err(),
        ProgramParseError::ExpiredPage
    );
    assert!(
        ProgramAdapterError::Parse(ProgramParseError::LoginPage).is_session_expired(),
        "a login page must classify as a session failure, not a parse failure"
    );
}

#[test]
fn plan_id_is_read_from_the_index_and_never_defaulted() {
    // The service writes the identifier into an escaped URL inside an
    // attribute, so it is read from the raw markup, not the rendered text.
    let index = "<html><body><iframe src=\"/x?m=pyfakzFrame&amp;fajhh=2024012345&amp;\"></iframe></body></html>";
    assert_eq!(parse_program_plan_id_html(index).unwrap(), 2_024_012_345);

    let missing = "<html><body><iframe src=\"/x?m=pyfakzFrame&amp;\"></iframe></body></html>";
    assert_eq!(
        parse_program_plan_id_html(missing).unwrap_err(),
        ProgramParseError::MissingPlanId
    );

    // A login page must still classify as a session failure before the marker
    // is even looked for.
    let login = "<html><head><title>清华大学WebVPN</title></head><body></body></html>";
    assert_eq!(
        parse_program_plan_id_html(login).unwrap_err(),
        ProgramParseError::LoginPage
    );
}

#[test]
fn full_plan_request_rejects_an_implausible_plan_identifier() {
    let profile = ProgramProfile::standard();
    assert!(profile.full_plan_request(0).is_err());
    assert!(profile.full_plan_request(u64::MAX).is_err());
    let plan = profile.full_plan_request(2_024_012_345).unwrap();
    assert_eq!(plan.path, PROGRAM_FULL_PATH);
    assert_eq!(
        plan.query_string(),
        format!("{PROGRAM_FULL_QUERY_PREFIX}2024012345")
    );
    assert_eq!(plan.webvpn_target, PROGRAM_WEBVPN_TARGET);
}

#[test]
fn full_plan_report_parses_named_sets_and_their_courses() {
    let page = r#"<html><body><div id="content_1"><table><tbody>
        <tr class="trr2"><td>专业核心课</td><td>必修</td><td>1000001</td><td>高等数学</td><td>5</td></tr>
        <tr class="trr2"><td>1000002</td><td>线性代数</td><td>3</td></tr>
        <tr class="trr2"><td>专业选修课</td><td>任选</td><td>2000001</td><td>机器学习</td><td>3</td></tr>
    </tbody></table></div></body></html>"#;
    let program = parse_full_program_html(page).expect("full plan parses");
    assert_eq!(program.course_sets.len(), 2);
    assert_eq!(program.course_sets[0].name, "专业核心课");
    assert_eq!(program.course_sets[0].kind, CourseSetKind::Compulsory);
    assert_eq!(program.course_sets[0].courses.len(), 2);
    assert_eq!(program.course_sets[0].courses[0].course_id, "1000001");
    assert_eq!(program.course_sets[0].courses[1].course_id, "1000002");
    assert_eq!(program.course_sets[0].courses[1].credit, 3.0);
    assert_eq!(program.course_sets[1].name, "专业选修课");
    assert_eq!(program.course_sets[1].kind, CourseSetKind::Elective);
    assert_eq!(program.course_sets[1].courses.len(), 1);
}

#[test]
fn full_plan_rejects_a_row_shape_the_service_never_renders() {
    // A set row is exactly five cells and a course row exactly three; a row
    // with another width must not be read as either one.
    let widened = r#"<html><body><div id="content_1">
        <tr class="trr2"><td>某课组</td><td>任选课</td><td>1</td><td>x</td><td>2</td><td>extra</td></tr>
    </div></body></html>"#;
    assert_eq!(
        parse_full_program_html(widened).unwrap_err(),
        ProgramParseError::UnrecognizedCourseRow { row: 0 }
    );

    // A set row whose attribute is not a known graduation requirement must not
    // be reported as an elective.
    let unknown = r#"<html><body><div id="content_1">
        <tr class="trr2"><td>某课组</td><td>任选课</td><td>1000001</td><td>某课程</td><td>2</td></tr>
    </div></body></html>"#;
    assert_eq!(
        parse_full_program_html(unknown).unwrap_err(),
        ProgramParseError::UnrecognizedCourseSet { row: 0 }
    );

    let orphan = r#"<html><body><div id="content_1">
        <tr class="trr2"><td>1000001</td><td>课程</td><td>2</td></tr>
    </div></body></html>"#;
    assert_eq!(
        parse_full_program_html(orphan).unwrap_err(),
        ProgramParseError::UnrecognizedCourseRow { row: 0 }
    );
}

#[tokio::test]
async fn completion_report_is_fetched_through_the_cookie_aware_transport() {
    let server = FixtureServer::new(vec![Reply::html(&completion_page())]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter = ProgramAdapter::try_with_transport(base, transport).expect("fixture adapter");

    let read = adapter
        .read_completion_with_proof()
        .await
        .expect("fixture completion read succeeds");
    assert_eq!(read.value.completed_credit, 168.5);
    assert!(adapter.business_proof_matches(&read.proof));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /jhBks.by_fascjgmxb_gr.do?m=queryFaScjgmx_gr"));
    assert!(requests[0].contains("xsViewFlag=pyfa"));
}

#[tokio::test]
async fn session_expiry_response_is_classified_before_parsing() {
    let server = FixtureServer::new(vec![Reply::html(
        "time out用户登陆超时或访问内容不存在。请重试",
    )]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter = ProgramAdapter::try_with_transport(base, transport).expect("fixture adapter");
    let error = adapter.read_completion().await.unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "program_auth_required");
}

#[tokio::test]
async fn cross_origin_redirect_is_refused_before_any_parse() {
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: https://example.invalid/steal\r\n".into(),
        body: Vec::new(),
    }]);
    let base = Url::parse(server.base()).unwrap();
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
        .expect("fixture transport");
    let adapter = ProgramAdapter::try_with_transport(base, transport).expect("fixture adapter");
    let error = adapter.read_completion().await.unwrap_err();
    assert!(matches!(error, ProgramAdapterError::UnexpectedOrigin));
}
