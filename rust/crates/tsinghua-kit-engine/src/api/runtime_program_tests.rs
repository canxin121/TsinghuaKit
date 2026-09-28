//! Runtime-level fixtures for the degree-program completion read.
//!
//! These use only loopback responses. They verify that the completion read is
//! reachable inside the existing INFO/WebVPN session, that the consumed
//! handoff query never becomes the adapter's base URL, and that a failed live
//! read is reported as a failure rather than as a stale report.

use super::*;

use crate::info::OpaqueUrl;
use crate::info_session::{InfoSessionAdapter, InfoWebVpnConfig};
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

fn program_user() -> UserIdentity {
    UserIdentity {
        username: "fixture-program-owner".to_owned(),
        display_name: None,
    }
}

/// Installs a proven Identity + INFO session whose adapter talks to `server`.
fn program_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime =
        CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap();
    let user = program_user();
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            runtime
                .coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-program-csrf").unwrap())
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

/// The completion report shaped like the registrar's own output: the summary
/// block, the `.table-striped` course table, and the out-of-plan table.
fn program_completion_page() -> String {
    "<html><body>\
     <div class=\"p-fbox\"><b>方案内实际完成 <strong>总学分：168.5</strong> \
     其中必修完成总学分：<strong>100</strong> \
     限选完成总学分：<strong>38.5</strong> \
     任选（方案内）完成总学分：<strong>30</strong> \
     重复课程：A、B 属于多个课组</b></div>\
     <table class=\"table-striped\"><tr>\
     <td>属性</td><td>课组</td><td>课号</td><td>课程名</td><td>学分</td>\
     <td>成绩/状态</td><td>绩点</td><td></td><td></td><td></td><td></td><td>是否完成</td>\
     </tr>\
     <tr><td>必修</td><td>专业核心课</td><td>1000001</td><td>高等数学</td><td>5</td>\
     <td>A</td><td>4.0</td><td>20</td><td>20</td><td>5</td><td>5</td><td>是</td></tr>\
     </table>\
     <div>本科生已修培养方案外课程完成总学分：<strong>4</strong></div>\
     </body></html>"
        .to_owned()
}

/// The registrar handoff: the portal answers with the target's own portal
/// entry point, and the mapping page is a same-origin HTML document.
fn program_handoff_replies(page: &str) -> Vec<Reply> {
    vec![
        Reply::html("XSRF-TOKEN=fixture-program-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/portal3rd.do?ticket=FIXTURE&mode=home"}}"#,
        ),
        Reply::html("<html>session handoff</html>"),
        Reply::html(page),
    ]
}

#[tokio::test]
async fn backend_repair_program_completion_reads_inside_the_proven_info_session() {
    let server = FixtureServer::new(program_handoff_replies(&program_completion_page()));
    let mut runtime = program_runtime(&server);

    let result = runtime
        .load_program_completion_result()
        .await
        .expect("program completion reads");

    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert!(result.error.is_none());
    assert_eq!(result.report.completed_credit, 168.5);
    assert!(runtime.program_service_is_proven());
    assert!(runtime.service_session_is_proven(ServiceId::Info));

    // The handoff's ticket must never become the adapter's base URL: the
    // fourth request carries only the mapping root and the report's own path.
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let read = &requests[3];
    assert!(
        read.starts_with(
            "GET /http/77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290/jhBks.by_fascjgmxb_gr.do?"
        ),
        "unexpected read request: {read}"
    );
    assert!(read.contains("xsViewFlag=pyfa"));
    assert!(!read.contains("ticket=FIXTURE"));
}

#[tokio::test]
async fn backend_repair_program_completion_failure_is_not_a_stale_report() {
    // The handoff succeeds, but the report itself is an expiry page. The
    // caller must see a failure rather than a report from an earlier read.
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-program-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/portal3rd.do?ticket=FIXTURE&mode=home"}}"#,
        ),
        Reply::html("<html>session handoff</html>"),
        Reply::html("time out用户登陆超时或访问内容不存在。请重试"),
    ]);
    let mut runtime = program_runtime(&server);

    let error = runtime
        .load_program_completion_result()
        .await
        .expect_err("an expiry page is a failure");
    assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    assert!(!runtime.program_service_is_proven());
}

#[tokio::test]
async fn backend_repair_program_completion_requires_a_proven_account() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = program_runtime(&server);
    runtime.invalidate_service_session(ServiceId::Identity);
    runtime.invalidate_service_session(ServiceId::Info);

    assert!(
        runtime.load_program_completion_result().await.is_err(),
        "an unproven account must not read a report"
    );
    assert!(server.requests().is_empty());
}
