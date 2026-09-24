use super::*;
use crate::protocol::{CsrfToken, ServiceSessionState};
use crate::reference_test_support::{FixtureServer, Reply};

async fn handoff_with_calendar(
    calendar: Reply,
) -> (
    Result<RegistrarSessionResult, RegistrarSessionError>,
    SessionCoordinator,
    Vec<String>,
) {
    let server = FixtureServer::new(vec![
        Reply::json("\"registrar-ticket\""),
        Reply { status: 302, headers: "Location: /http/registrar-fixture/\r\nSet-Cookie: fixture-registrar=ready; Path=/http/registrar-fixture/\r\n".into(), body: String::new() },
        Reply::html("<html><script src='/wengine-vpn/webvpn.js'></script><script>const message='请先登录';</script><h1>教务门户</h1></html>"),
        calendar,
    ]);
    let registrar = RegistrarClient::new(crate::registrar_client::RegistrarClientConfig {
        learn_base_url: format!("{}https/learn-fixture/", server.base()),
        registrar_base_url: format!("{}http/registrar-fixture/", server.base()),
        ..Default::default()
    })
    .unwrap();
    let mut coordinator = SessionCoordinator::new();
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    coordinator.begin_authentication(ServiceId::Learn).unwrap();
    let csrf = coordinator
        .registry()
        .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
    coordinator
        .mark_authenticated(
            ServiceId::Learn,
            user.clone(),
            None,
            Some(csrf.clone()),
            None,
        )
        .unwrap();
    let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 19).unwrap();
    let result = RegistrarSessionOrchestrator::new(registrar)
        .establish(
            &mut coordinator,
            &csrf,
            user,
            AcademicStage::Undergraduate,
            CalendarWindow::new(date, date).unwrap(),
            "fixtureCb",
        )
        .await;
    (result, coordinator, server.requests())
}

#[tokio::test]
async fn backend_repair_sep19_registrar_injected_landing_requires_real_empty_calendar_proof() {
    let (result, coordinator, requests) = handoff_with_calendar(Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: "fixtureCb([])".into(),
    })
    .await;
    assert!(result.is_ok());
    assert_eq!(
        coordinator
            .registry()
            .snapshot_for(ServiceId::Registrar)
            .state,
        ServiceSessionState::Authenticated
    );
    assert_eq!(
        coordinator.registry().snapshot_for(ServiceId::Learn).state,
        ServiceSessionState::Authenticated
    );
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("appId=ALL_ZHJW"))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.lines().next().unwrap().contains("j_acegi_login.do"))
            .count(),
        1
    );
    assert!(requests[3].starts_with("GET /http/registrar-fixture/jxmh_out.do?"));
    assert!(
        requests[3]
            .to_ascii_lowercase()
            .contains("cookie: fixture-registrar=ready")
    );
}

#[tokio::test]
async fn backend_repair_sep19_registrar_neutral_html_never_proves_calendar_or_replays_ticket() {
    let (result, coordinator, requests) =
        handoff_with_calendar(Reply::html("<html><h1>neutral placeholder</h1></html>")).await;
    assert!(result.is_err());
    assert_ne!(
        coordinator
            .registry()
            .snapshot_for(ServiceId::Registrar)
            .state,
        ServiceSessionState::Authenticated
    );
    assert_eq!(
        coordinator.registry().snapshot_for(ServiceId::Learn).state,
        ServiceSessionState::Authenticated
    );
    assert_eq!(requests.len(), 4);
    assert_eq!(
        result.unwrap_err().diagnostic_code(),
        "registrar_calendar_content_type"
    );
}

#[test]
fn backend_repair_sep19_registrar_diagnostics_discard_raw_failure_messages() {
    let error = RegistrarSessionError::Client(RegistrarClientError::LoginExpired {
        reason: "synthetic-secret-must-not-be-a-code".into(),
    });
    assert_eq!(error.diagnostic_code(), "registrar_session_expired");
    let origin = RegistrarSessionError::Client(RegistrarClientError::UnexpectedOrigin);
    assert_eq!(origin.diagnostic_code(), "registrar_origin_rejected");
}
