//! Runtime-level fixtures for the teaching-evaluation list read.
//!
//! These use only loopback responses.  They verify that the list is reachable
//! inside the existing INFO/WebVPN session, that the consumed handoff query
//! never becomes the adapter's base URL, that the service's closed-window
//! answer reaches the caller as its own state rather than as an empty list,
//! and that the per-row form routes stay inside Rust.

use super::*;

use crate::assessment_read::ASSESSMENT_LIST_PATH;
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

/// The evaluation handoff: the portal answers with the target host's own
/// portal entry point, and the mapping page is a same-origin HTML document.
fn assessment_handoff_replies(page: &str) -> Vec<Reply> {
    vec![
        Reply::html("XSRF-TOKEN=fixture-assessment-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"http://jxgl.cic.tsinghua.edu.cn/portal3rd.do?ticket=FIXTURE&mode=home"}}"#,
        ),
        Reply::html("<html>session handoff</html>"),
        Reply::html(page),
    ]
}

#[tokio::test]
async fn backend_repair_assessment_list_reads_inside_the_proven_info_session() {
    let server = FixtureServer::new(assessment_handoff_replies(&list_page()));
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

    // The handoff's ticket must never become the adapter's base URL: the
    // fourth request carries only the mapping root and the list's own path.
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let read = &requests[3];
    assert!(
        read.starts_with(&format!(
            "GET /http/{ASSESSMENT_MAPPING}{ASSESSMENT_LIST_PATH} "
        )),
        "unexpected read request: {read}"
    );
    assert!(!read.contains("ticket=FIXTURE"));
}

#[tokio::test]
async fn backend_repair_assessment_closed_window_is_not_an_empty_list() {
    // The window is closed: the service answers 200 with its own notice.  The
    // caller must see that state, not a validated empty list.
    let server = FixtureServer::new(assessment_handoff_replies(&format!(
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
    let server = FixtureServer::new(assessment_handoff_replies(
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
    let server = FixtureServer::new(assessment_handoff_replies(&list_page()));
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
