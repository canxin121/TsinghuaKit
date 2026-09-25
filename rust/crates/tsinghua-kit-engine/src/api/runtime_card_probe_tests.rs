//! Probe denial is not authenticated success, but may precede one initial
//! card-target handoff. Fixtures never contact a real campus endpoint.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

fn user() -> UserIdentity {
    UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    }
}

#[tokio::test]
async fn backend_repair_cardprobe_target_password_is_encrypted_submitted_once_and_not_primary_relogin()
 {
    use sm2::elliptic_curve::sec1::ToSec1Point;
    let public_key: String = sm2::SecretKey::from_slice(&[1u8; 32])
        .unwrap()
        .public_key()
        .to_sec1_point(false)
        .as_bytes()[1..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let form = format!(
        "<html><span id='sm2publicKey'>{public_key}</span><form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'></form></html>"
    );
    let card = FixtureServer::new(vec![rejected()]);
    let id = FixtureServer::new(vec![Reply::html(&form), challenge_page(), approaches()]);
    let mut r = runtime(&id, &card);
    assert!(r.ensure_card_session(&user()).await.is_err());
    assert_eq!(
        r.current_service_second_factor().unwrap().target,
        ServiceSecondFactorTarget::CampusCard
    );
    assert_eq!(id.requests().len(), 3);
    assert_eq!(card.requests().len(), 1);
    assert!(id.requests()[1].starts_with("POST /do/off/ui/auth/login/check "));
    assert!(id.requests()[1].contains("i_pass=04"));
    assert!(!id.requests()[1].contains("fixture-password-not-real"));
    assert!(r.ensure_card_session(&user()).await.is_err());
    assert_eq!(id.requests().len(), 3);
    assert_other_services_kept(&r);
}

#[tokio::test]
async fn backend_repair_cardprobe_wrong_account_after_sso_never_becomes_success_or_retries() {
    let card = FixtureServer::new(vec![
        rejected(),
        Reply::html("<html>callback</html>"),
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"other-user"}}"#),
    ]);
    let id = FixtureServer::new(vec![success_page(&card)]);
    let mut r = runtime(&id, &card);
    assert_eq!(
        r.ensure_card_session(&user()).await.unwrap_err(),
        "campus_card_account_mismatch"
    );
    assert!(
        r.ensure_card_session(&user())
            .await
            .unwrap_err()
            .contains("campus_card_auth_attempt_exhausted")
    );
    assert!(!r.service_session_is_proven(ServiceId::CampusCard));
    assert_eq!(card.requests().len(), 3);
    assert_eq!(id.requests().len(), 1);
    assert_other_services_kept(&r);
}

#[tokio::test]
async fn backend_repair_cardprobe_target_network_failure_is_not_an_automatic_retry() {
    let card = FixtureServer::new(vec![rejected()]);
    let id = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: "unavailable".into(),
    }]);
    let mut r = runtime(&id, &card);
    assert!(r.ensure_card_session(&user()).await.is_err());
    assert!(r.card_auth_attempted);
    assert!(
        r.ensure_card_session(&user())
            .await
            .unwrap_err()
            .contains("campus_card_auth_attempt_exhausted")
    );
    assert_eq!(card.requests().len(), 1);
    assert_eq!(id.requests().len(), 1);
    assert_other_services_kept(&r);
    r.logout().unwrap();
    assert!(!r.card_auth_attempted);
}

#[tokio::test]
async fn backend_repair_cardprobe_account_read_failure_cannot_enter_initial_probe_fallback() {
    let card = FixtureServer::new(vec![accepted(), rejected()]);
    let id = FixtureServer::new(vec![]);
    let mut r = runtime(&id, &card);
    r.ensure_card_session(&user()).await.unwrap();
    assert!(r.load_campus_card_account().await.is_err());
    assert_eq!(card.requests().len(), 2);
    assert!(id.requests().is_empty());
    assert!(!r.card_auth_attempted);
    assert_other_services_kept(&r);
    assert!(!card_probe_allows_initial_handoff("card_read_rejected"));
    assert!(!card_probe_allows_initial_handoff(
        "campus_card_probe_account_missing"
    ));
    assert!(!card_probe_allows_initial_handoff(
        "campus_card_service_rejected"
    ));
}

#[tokio::test]
async fn backend_repair_cardprobe_no_supplied_password_does_not_trigger_unrequested_auth() {
    let card = FixtureServer::new(vec![rejected()]);
    let id = FixtureServer::new(vec![]);
    let mut r = runtime(&id, &card);
    r.primary_password = None;
    assert_eq!(
        r.ensure_card_session(&user()).await.unwrap_err(),
        "campus_card_probe_outer_failure"
    );
    assert_eq!(card.requests().len(), 1);
    assert!(id.requests().is_empty());
    assert!(!r.card_auth_attempted);
    assert_other_services_kept(&r);
}

#[tokio::test]
async fn backend_repair_cardprobe_foreign_handoff_is_rejected_before_any_fetch() {
    let card = FixtureServer::new(vec![]);
    let denied = FixtureServer::new(vec![]);
    let id = FixtureServer::new(vec![]);
    let mut r = runtime(&id, &card);
    assert!(
        r.finish_card_handoff(
            &user(),
            Url::parse(&format!("{}login?ticket=FIXTURE", denied.base())).unwrap()
        )
        .await
        .is_err()
    );
    assert!(denied.requests().is_empty());
    assert!(card.requests().is_empty());
    assert!(id.requests().is_empty());
    assert_other_services_kept(&r);
}

#[test]
fn backend_repair_cardprobe_log_preserves_probe_phase_without_response_or_secret() {
    let root = std::env::temp_dir().join(format!("thyou-card-probe-log-{}", Uuid::new_v4()));
    let mut log = crate::telemetry::LogSession::start(
        &root,
        crate::telemetry::LogConfig::parse("trace", false).unwrap(),
    )
    .unwrap();
    let card = FixtureServer::new(vec![
        Reply::json(r#"{"success":false,"message":"PRIVATE-RESPONSE-ACCOUNT","resultData":null}"#),
        Reply::html("<html>callback</html>"),
        accepted(),
    ]);
    let id = FixtureServer::new(vec![success_page(&card)]);
    tracing::dispatcher::with_default(&log.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut r = runtime(&id, &card);
                r.ensure_card_session(&user()).await.unwrap();
            });
    });
    log.flush();
    let text = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let probes: Vec<_> = rows
        .iter()
        .filter(|row| row["fields"]["event"] == "card_session_probe")
        .collect();
    assert_eq!(probes.len(), 2);
    assert_eq!(
        probes[0]["fields"]["business_stage"],
        "card_probe_before_handoff"
    );
    assert_eq!(
        probes[0]["fields"]["reason"],
        "campus_card_probe_outer_failure"
    );
    assert_eq!(
        probes[1]["fields"]["business_stage"],
        "card_probe_after_handoff"
    );
    assert_eq!(probes[1]["fields"]["outcome"], "success");
    assert!(probes.iter().all(|row| row["redacted_fields"] == 0));
    for secret in [
        "PRIVATE-RESPONSE-ACCOUNT",
        "FIXTURE-CARD-TICKET",
        "fixture-password-not-real",
        "fixture-user",
    ] {
        assert!(!text.contains(secret));
    }
    drop(log);
    std::fs::remove_dir_all(root).unwrap();
}
fn rejected() -> Reply {
    Reply::json(
        r#"{"success":false,"message":"synthetic session probe denied","resultData":null,"data":null}"#,
    )
}
fn accepted() -> Reply {
    Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#)
}
fn challenge_page() -> Reply {
    Reply::html("<html>二次认证</html>")
}
fn approaches() -> Reply {
    Reply::json(r#"{"result":"success","object":{"hasWeChatBool":true,"hasTotp":true}}"#)
}
fn runtime(id: &FixtureServer, card: &FixtureServer) -> CampusRuntime {
    let mut r =
        CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false)
            .unwrap();
    let profile = r.identity.identity().client().config().profile.clone();
    r.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(id.base(), profile).unwrap()).unwrap(),
        crate::transport::CampusHttpTransport::new("THYou/card-probe-fixture").unwrap(),
    ));
    for service in [ServiceId::Identity, ServiceId::Info] {
        r.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            r.coordinator.registry().bind_csrf(
                service,
                crate::protocol::CsrfToken::new("fixture-info-csrf").unwrap(),
            )
        });
        r.coordinator
            .mark_authenticated(service, user(), None, csrf, None)
            .unwrap();
    }
    r.portal_bootstrapped = true;
    r.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(card.base(), "/info/").unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.info_roaming_url =
        Some(crate::info::OpaqueUrl::new(format!("{}info/f/info/index", card.base())).unwrap());
    r.primary_password = Some("fixture-password-not-real".into());
    r.card_client = Some(
        CampusCardClient::new(
            CampusCardAdapterConfig::new(card.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r
}
fn success_page(card: &FixtureServer) -> Reply {
    Reply::html(&format!(
        "<html>登录成功。正在重定向到<a href='{}login?ticket=FIXTURE-CARD-TICKET'>继续</a></html>",
        card.base()
    ))
}
fn assert_other_services_kept(r: &CampusRuntime) {
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(r.portal_bootstrapped);
}

#[tokio::test]
async fn backend_repair_cardprobe_denial_enters_target_sso_without_replaying_primary_login() {
    let card = FixtureServer::new(vec![rejected()]);
    let id = FixtureServer::new(vec![challenge_page(), approaches()]);
    let mut r = runtime(&id, &card);
    assert!(r.ensure_card_session(&user()).await.is_err());
    assert_eq!(
        r.current_service_second_factor().unwrap().target,
        ServiceSecondFactorTarget::CampusCard
    );
    assert_eq!(card.requests().len(), 1);
    assert_eq!(id.requests().len(), 2);
    assert!(id.requests()[0].starts_with(
        "GET /do/off/ui/auth/login/form/eea30cbedcaf97c69d28b2d92f22a259/0?/userindex "
    ));
    assert!(id.requests()[1].contains("FIND_APPROACHES"));
    assert!(
        id.requests()
            .iter()
            .all(|request| !request.contains("i_pass") && !request.contains("SEND_CODE"))
    );
    assert!(r.ensure_card_session(&user()).await.is_err());
    assert_eq!(card.requests().len(), 1);
    assert_eq!(id.requests().len(), 2);
    assert_other_services_kept(&r);
}

#[tokio::test]
async fn backend_repair_cardprobe_cookie_sso_completes_and_rechecks_account_without_mfa() {
    let card = FixtureServer::new(vec![
        rejected(),
        Reply {
            status: 200,
            headers: "Content-Type: text/html\r\nSet-Cookie: fixture-card=ready; Path=/\r\n".into(),
            body: "<html>card callback</html>".into(),
        },
        accepted(),
    ]);
    let id = FixtureServer::new(vec![success_page(&card)]);
    let mut r = runtime(&id, &card);
    r.ensure_card_session(&user()).await.unwrap();
    r.ensure_card_session(&user()).await.unwrap();
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert!(r.service_second_factor.is_none());
    assert_eq!(card.requests().len(), 3);
    assert_eq!(id.requests().len(), 1);
    assert!(card.requests()[1].starts_with("GET /login?ticket=FIXTURE-CARD-TICKET "));
    assert!(
        card.requests()[2]
            .to_ascii_lowercase()
            .contains("cookie: fixture-card=ready")
    );
    assert_other_services_kept(&r);
}

#[tokio::test]
async fn backend_repair_cardprobe_failure_after_handoff_cannot_restart_the_login_chain() {
    let card = FixtureServer::new(vec![
        rejected(),
        Reply::html("<html>card callback</html>"),
        rejected(),
    ]);
    let id = FixtureServer::new(vec![success_page(&card)]);
    let mut r = runtime(&id, &card);
    assert_eq!(
        r.ensure_card_session(&user()).await.unwrap_err(),
        "campus_card_probe_outer_failure"
    );
    assert!(!r.service_session_is_proven(ServiceId::CampusCard));
    assert_other_services_kept(&r);
    assert_eq!(card.requests().len(), 3);
    assert_eq!(id.requests().len(), 1);
    let error = r.ensure_card_session(&user()).await.unwrap_err();
    assert!(error.contains("campus_card_auth_attempt_exhausted"));
    assert_eq!(card.requests().len(), 3);
    assert_eq!(id.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_cardprobe_user_factor_continues_once_then_binds_real_card_account() {
    let card = FixtureServer::new(vec![
        rejected(),
        Reply::html("<html>card callback</html>"),
        accepted(),
    ]);
    let id = FixtureServer::new(vec![
        challenge_page(),
        approaches(),
        Reply::json(
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        ),
        success_page(&card),
    ]);
    let mut r = runtime(&id, &card);
    assert!(r.ensure_card_session(&user()).await.is_err());
    r.complete_second_factor("wechat".into(), "123456".into())
        .await
        .unwrap();
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert_other_services_kept(&r);
    assert_eq!(id.requests().len(), 4);
    assert_eq!(card.requests().len(), 3);
    assert_eq!(
        id.requests()
            .iter()
            .filter(|r| r.contains("VERITY_CODE"))
            .count(),
        1
    );
    assert!(
        !id.requests()
            .iter()
            .any(|r| r.contains("SEND_CODE") || r.contains("saveFinger") || r.contains("i_pass"))
    );
}

#[tokio::test]
async fn backend_repair_cardprobe_outage_invalid_json_and_wrong_account_do_not_start_sso() {
    for response in [
        Reply {
            status: 503,
            headers: String::new(),
            body: "unavailable".into(),
        },
        Reply::html("<html>maintenance</html>"),
        Reply::json("not-json"),
        Reply::json(r#"{"success":"false"}"#),
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"other-user"}}"#),
    ] {
        let card = FixtureServer::new(vec![response]);
        let id = FixtureServer::new(vec![]);
        let mut r = runtime(&id, &card);
        assert!(r.ensure_card_session(&user()).await.is_err());
        assert_eq!(card.requests().len(), 1);
        assert!(id.requests().is_empty());
        assert_other_services_kept(&r);
    }
}

#[tokio::test]
async fn backend_repair_cardprobe_valid_existing_session_stays_zero_authentication_requests() {
    let card = FixtureServer::new(vec![accepted()]);
    let id = FixtureServer::new(vec![]);
    let mut r = runtime(&id, &card);
    r.ensure_card_session(&user()).await.unwrap();
    r.ensure_card_session(&user()).await.unwrap();
    assert_eq!(card.requests().len(), 1);
    assert!(id.requests().is_empty());
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
}

#[tokio::test]
async fn backend_repair_run072714_unsaved_password_can_use_existing_card_sso() {
    let card = FixtureServer::new(vec![rejected(), Reply::html("callback"), accepted()]);
    let id = FixtureServer::new(vec![success_page(&card)]);
    let mut r = runtime(&id, &card);
    r.primary_password = None;
    r.remember_credentials = false;
    r.credentials_persisted = false;
    r.ensure_card_session(&user())
        .await
        .expect("card SSO does not require a saved password when cookies work");
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert_eq!(id.requests().len(), 1);
    assert!(id.requests()[0].starts_with("GET "));
    assert_eq!(card.requests().len(), 3);
    assert!(r.primary_password.is_none());
    assert!(!r.private_credential_attempted);
    assert_other_services_kept(&r);
}
