//! THUInfo saveFinger semantics, consent and at-most-once registration.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;
use reqwest::Url;

fn orchestrator(server: &FixtureServer) -> IdentitySessionOrchestrator {
    use crate::identity::{
        FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthAction,
        SecondAuthActions, SecondAuthProfile,
    };
    use crate::identity_client::{IdentityClient, IdentityClientConfig};
    let profile = IdentityLoginProfile::new(
        "fixture",
        "/do/off/ui/auth/login/form/{appId}/0",
        LoginFormProfile::new(
            "/do/off/ui/auth/login/check",
            FormEncoding::UrlEncoded,
            LoginFormFields::common(),
        ),
        SecondAuthProfile::new(
            "/b/doubleAuth/login",
            "type",
            "action",
            vec![SecondAuthMethod::Wechat],
            SecondAuthActions::new(
                Some(SecondAuthAction::FindApproaches),
                Some(SecondAuthAction::SendCode),
                Some(SecondAuthAction::VerifyCode),
                Some(SecondAuthAction::VerifyTotpCode),
            ),
        ),
    );
    IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(server.base(), profile).unwrap()).unwrap(),
        CampusHttpTransport::new("THYou/reuse-fixture").unwrap(),
    ))
}

#[tokio::test]
async fn backend_repair_reuse_cancelled_registration_cannot_be_retried_in_same_context() {
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    let server = FixtureServer::new(vec![Reply::json(r#"{"result":"success"}"#)]);
    let id = orchestrator(&server);
    let owned = OwnedTrustedDeviceOptions::from(options());
    {
        let mut pending = Box::pin(id.save_trusted_device(&owned));
        poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    let result = id.save_trusted_device(&owned).await.unwrap();
    assert_eq!(result.0.status, TrustedDeviceStatus::RequestFailed);
    assert!(server.requests().len() <= 1);
}

#[tokio::test]
async fn backend_repair_reuse_primary_and_later_service_mfa_share_one_trust_registration() {
    let server = FixtureServer::new(vec![
        verified(),
        Reply {
            status: 200,
            headers:
                "Content-Type: application/json\r\nSet-Cookie: fixture-trust=registered; Path=/\r\n"
                    .into(),
            body: r#"{"result":"success"}"#.into(),
        },
        callback(),
        verified(),
        callback(),
        verified(),
        callback(),
    ]);
    let id = orchestrator(&server);
    let mut coordinator = SessionCoordinator::new();
    coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    coordinator
        .require_second_factor(
            ServiceId::Identity,
            challenge(),
            Some(UserIdentity {
                username: "fixture-user".into(),
                display_name: None,
            }),
        )
        .unwrap();
    id.replace_pending_trusted_device(Some(options().into()));
    let primary = id
        .complete_second_factor(&mut coordinator, SecondAuthMethod::Wechat, "001001")
        .await
        .unwrap();
    assert!(primary.trusted_device.unwrap().is_saved());
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "001002",
        Some(options()),
    )
    .await
    .unwrap();
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "001003",
        Some(options()),
    )
    .await
    .unwrap();
    assert_eq!(server.requests().len(), 7);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST /b/doubleAuth/personal/saveFinger "))
            .count(),
        1
    );
    assert!(
        server.requests()[2]
            .to_ascii_lowercase()
            .contains("cookie: fixture-trust=registered")
    );
    assert_eq!(
        id.trusted_device_registration_status(),
        Some(TrustedDeviceStatus::Saved)
    );
}

#[tokio::test]
async fn backend_repair_reuse_first_service_factor_can_register_only_after_explicit_consent() {
    let server = FixtureServer::new(vec![
        verified(),
        Reply::json(r#"{"result":"success"}"#),
        callback(),
        verified(),
        callback(),
    ]);
    let id = orchestrator(&server);
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "001101",
        Some(options()),
    )
    .await
    .unwrap();
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "001102",
        Some(options()),
    )
    .await
    .unwrap();
    assert_eq!(server.requests().len(), 5);
    assert!(server.requests()[0].contains("VERITY_CODE"));
    assert!(server.requests()[1].starts_with("POST /b/doubleAuth/personal/saveFinger "));
    assert!(server.requests()[2].starts_with("GET /do/off/ui/auth/login/redirect2Jsp "));
    assert!(
        !server
            .requests()
            .iter()
            .any(|request| request.contains("SEND_CODE"))
    );
}

#[tokio::test]
async fn backend_repair_reuse_no_consent_or_rejected_code_does_not_register_a_device() {
    let server = FixtureServer::new(vec![
        verified(),
        callback(),
        Reply::json(r#"{"result":"fail"}"#),
    ]);
    let id = orchestrator(&server);
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "002001",
        None,
    )
    .await
    .unwrap();
    assert!(
        id.complete_service_second_factor_with_trusted_device(
            &challenge(),
            SecondAuthMethod::Wechat,
            "002002",
            Some(options())
        )
        .await
        .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    assert!(
        server
            .requests()
            .iter()
            .all(|request| !request.contains("saveFinger"))
    );
    assert!(id.trusted_device_registration_status().is_none());
}

#[tokio::test]
async fn backend_repair_reuse_uncertain_trust_registration_is_not_replayed_for_next_service() {
    let server = FixtureServer::new(vec![
        verified(),
        Reply {
            status: 503,
            headers: String::new(),
            body: "unavailable".into(),
        },
        callback(),
        verified(),
        callback(),
    ]);
    let id = orchestrator(&server);
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "003001",
        Some(options()),
    )
    .await
    .unwrap();
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "003002",
        Some(options()),
    )
    .await
    .unwrap();
    assert_eq!(server.requests().len(), 5);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.contains("saveFinger"))
            .count(),
        1
    );
    assert_eq!(
        id.trusted_device_registration_status(),
        Some(TrustedDeviceStatus::RequestFailed)
    );
}

#[tokio::test]
async fn backend_repair_reuse_device_limit_does_not_delete_existing_devices_or_loop() {
    let server = FixtureServer::new(vec![
        verified(),
        Reply::json(r#"{"result":"fail","msg":"信任设备已达上限"}"#),
        callback(),
        verified(),
        callback(),
    ]);
    let id = orchestrator(&server);
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "004001",
        Some(options()),
    )
    .await
    .unwrap();
    id.complete_service_second_factor_with_trusted_device(
        &challenge(),
        SecondAuthMethod::Wechat,
        "004002",
        Some(options()),
    )
    .await
    .unwrap();
    assert_eq!(server.requests().len(), 5);
    assert_eq!(
        id.trusted_device_registration_status(),
        Some(TrustedDeviceStatus::LimitReached)
    );
    assert!(
        server
            .requests()
            .iter()
            .all(|request| !request.contains("deleteDevice") && !request.contains("SEND_CODE"))
    );
}
fn challenge() -> SecondFactorChallenge {
    SecondFactorChallenge {
        methods: vec![SecondFactorMethod::Wechat],
        masked_phone: None,
        expires_at: None,
    }
}
fn options() -> TrustedDeviceOptions<'static> {
    TrustedDeviceOptions::new("0123456789abcdef0123456789abcdef", "THYou fixture")
}
fn verified() -> Reply {
    Reply::json(
        r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
    )
}
fn callback() -> Reply {
    Reply::html("<html>verified cookie-backed identity callback</html>")
}

fn response(body: &str) -> IdentityHttpResponse {
    IdentityHttpResponse::from_parts(
        StatusCode::OK,
        Url::parse("https://id.fixture.invalid/b/doubleAuth/personal/saveFinger").unwrap(),
        body,
    )
}

#[test]
fn backend_repair_reuse_trust_success_does_not_require_optional_finger3() {
    for body in [
        r#"{"result":"success"}"#,
        r#"{"result":"success","object":null}"#,
        r#"{"result":"success","object":{"finger3":"fixture-private-token"}}"#,
    ] {
        let parsed = parse_trusted_device_response(&response(body));
        assert_eq!(parsed.status, TrustedDeviceStatus::Saved);
        assert!(!format!("{parsed:?}").contains("fixture-private-token"));
    }
}

#[test]
fn backend_repair_reuse_trust_conflicting_failure_markers_cannot_claim_success() {
    for body in [
        r#"{"result":"success","success":false,"finger3":"fixture"}"#,
        r#"{"result":"error","success":true,"finger3":"fixture"}"#,
        r#"{"object":{"finger3":"fixture"}}"#,
    ] {
        assert!(!parse_trusted_device_response(&response(body)).is_saved());
    }
}

#[tokio::test]
async fn backend_repair_reuse_save_device_is_single_attempt_in_one_authentication_context() {
    use crate::identity::{
        FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthActions,
        SecondAuthProfile,
    };
    use crate::identity_client::{IdentityClient, IdentityClientConfig};
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"result":"success","object":{"finger3":"fixture-private-token"}}"#),
        Reply::json(r#"{"result":"success","object":{"finger3":"must-not-request"}}"#),
    ]);
    let profile = IdentityLoginProfile::new(
        "fixture",
        "/do/off/ui/auth/login/form/{appId}/0",
        LoginFormProfile::new(
            "/do/off/ui/auth/login/check",
            FormEncoding::UrlEncoded,
            LoginFormFields::common(),
        ),
        SecondAuthProfile::new(
            "/b/doubleAuth/login",
            "type",
            "action",
            vec![],
            SecondAuthActions::new(None, None, None, None),
        ),
    );
    let identity = IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(server.base(), profile).unwrap()).unwrap(),
        CampusHttpTransport::new("THYou/reuse-fixture").unwrap(),
    );
    let orchestrator = IdentitySessionOrchestrator::new(identity);
    let options = OwnedTrustedDeviceOptions {
        fingerprint: "0123456789abcdef0123456789abcdef".into(),
        device_name: "THYou fixture".into(),
        single_login: None,
    };
    assert!(
        orchestrator
            .save_trusted_device(&options)
            .await
            .unwrap()
            .0
            .is_saved()
    );
    assert!(
        orchestrator
            .save_trusted_device(&options)
            .await
            .unwrap()
            .0
            .is_saved()
    );
    assert_eq!(
        server.requests().len(),
        1,
        "later service MFA must not register the same device again"
    );
    assert!(server.requests()[0].starts_with("POST /b/doubleAuth/personal/saveFinger "));
}
