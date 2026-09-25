//! Public runtime response-binding controls; no live account or campus URL.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};
use serde_json::json;

const EMPTY_EXAMS: &str = "<html><table><tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr></table></html>";

#[tokio::test]
async fn backend_repair_binding_exam_navigation_mentions_are_not_login_proof() {
    let body = format!(
        "<script>const help='webvpn j_acegi_login i_user i_pass';</script><!-- session expired --><footer>统一认证与WebVPN帮助</footer>{EMPTY_EXAMS}"
    );
    let s = FixtureServer::new(vec![Reply::html(&body)]);
    let mut r = exam_runtime(&s);
    assert!(r.load_exams(String::new(), String::new()).await.is_ok());
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    assert_eq!(s.requests().len(), 1);
}

fn user() -> UserIdentity {
    UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    }
}
fn runtime() -> CampusRuntime {
    CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false).unwrap()
}
fn prove(r: &mut CampusRuntime, service: ServiceId) {
    r.coordinator.begin_authentication(service).unwrap();
    r.coordinator
        .mark_authenticated(service, user(), None, None, None)
        .unwrap();
}
fn exam_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity);
    prove(&mut r, ServiceId::Registrar);
    let learn =
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
    let registrar = RegistrarClient::from_transport(
        RegistrarClientConfig {
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..Default::default()
        },
        r.identity.transport().clone(),
    )
    .unwrap();
    r.grades_source = Some(Arc::new(
        CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2026-2027-1", AcademicStage::Undergraduate),
        )
        .unwrap(),
    ));
    r
}

#[tokio::test]
async fn backend_repair_binding_public_exam_auth_errors_clear_only_registrar() {
    let target = FixtureServer::new(vec![]);
    for reply in [
        Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        },
        Reply::html(
            "<html><form><input name=\"j_username\"><input type=\"password\" name=\"j_password\"></form></html>",
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: {}do/off/ui/auth/login/form/fixture/0\r\n",
                target.base()
            ),
            body: String::new(),
        },
    ] {
        let s = FixtureServer::new(vec![reply]);
        let mut r = exam_runtime(&s);
        assert!(r.load_exams(String::new(), String::new()).await.is_err());
        assert!(!r.service_session_is_proven(ServiceId::Registrar));
        assert!(r.service_session_is_proven(ServiceId::Identity));
        assert_eq!(s.requests().len(), 1);
    }
    assert!(target.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_binding_public_exam_outage_then_explicit_read_keeps_valid_session() {
    let s = FixtureServer::new(vec![
        Reply {
            status: 503,
            headers: String::new(),
            body: "unavailable".into(),
        },
        Reply::html(EMPTY_EXAMS),
    ]);
    let mut r = exam_runtime(&s);
    assert!(r.load_exams(String::new(), String::new()).await.is_err());
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    assert_eq!(s.requests().len(), 1);
    assert!(r.load_exams(String::new(), String::new()).await.is_ok());
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_binding_public_card_later_date_drift_never_returns_partial_transactions() {
    let row = |id: u32, date: &str| json!({"id":id,"summary":"Fixture","txdate":date,"balance":1000,"txamt":-1,"meraddr":"","txname":"消费"});
    let first: Vec<_> = (1..=100).map(|id| row(id, "2026-09-18 12:00:00")).collect();
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#),
        Reply::json(&json!({"success":true,"resultData":{"rows":first}}).to_string()),
        Reply::json(
            &json!({"success":true,"resultData":{"rows":[row(101,"2026-09-19 00:00:00")]}})
                .to_string(),
        ),
    ]);
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity);
    r.card_client = Some(
        CampusCardClient::new(
            crate::campus_card_adapter::CampusCardAdapterConfig::new(s.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.probe_and_bind_card_session(&user()).await.unwrap();
    assert!(
        r.load_campus_card_transactions("2026-09-18".into(), "2026-09-18".into())
            .await
            .is_err()
    );
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(s.requests().len(), 3);
}
