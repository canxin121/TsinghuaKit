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
