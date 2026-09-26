//! Additional request lifecycle regressions. Only synthetic loopback data.
use super::*;
use crate::protocol::SecondFactorChallenge;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::tunet_client::TunetClientConfig;

#[tokio::test]
async fn backend_repair_consistency_primary_success_reconstructs_only_verified_identity() {
    let s = FixtureServer::new(vec![
        Reply::json(
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        ),
        Reply::html("<html><body>ticketless identity callback</body></html>"),
    ]);
    let mut r = pending_identity(&s, false);
    let result = r
        .identity
        .complete_second_factor(&mut r.coordinator, SecondAuthMethod::Wechat, "123456")
        .await
        .unwrap();
    assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
    assert_eq!(result.snapshot.user.unwrap().username, "fixture-primary");
    assert!(
        r.coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .second_factor
            .is_none()
    );
    assert!(!r.service_session_is_proven(ServiceId::Learn));
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_consistency_primary_explicit_retry_then_success_commits_once() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"result":"fail"}"#),
        Reply::json(
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        ),
        Reply::html("<html><body>ticketless identity callback</body></html>"),
    ]);
    let mut r = pending_identity(&s, false);
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert!(r.primary_password.is_some());
    let result = r
        .identity
        .complete_second_factor(&mut r.coordinator, SecondAuthMethod::Wechat, "654321")
        .await
        .unwrap();
    assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
    let requests = s.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].contains("vericode=654321"));
    assert!(!requests[2].contains("vericode"));
}

#[tokio::test]
async fn backend_repair_consistency_cancelled_primary_future_drops_challenge_and_password() {
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    let s = FixtureServer::new(vec![Reply::json(r#"{"result":"fail"}"#)]);
    let mut r = pending_identity(&s, false);
    {
        let mut pending = Box::pin(r.complete_second_factor("wechat".into(), "123456".into()));
        poll_fn(|cx| {
            assert!(
                pending.as_mut().poll(cx).is_pending(),
                "loopback HTTP should yield before response completion"
            );
            Poll::Ready(())
        })
        .await;
    }
    assert_ne!(
        r.coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .state,
        ServiceSessionState::RequiresSecondFactor
    );
    assert!(r.primary_password.is_none());
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert!(s.requests().len() <= 1);
}

fn runtime() -> CampusRuntime {
    CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false).unwrap()
}

fn pending_identity(server: &FixtureServer, expired: bool) -> CampusRuntime {
    let mut r = runtime();
    let profile = r.identity.identity().client().config().profile.clone();
    r.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(server.base(), profile).unwrap()).unwrap(),
        crate::transport::CampusHttpTransport::new("THYou/consistency-fixture").unwrap(),
    ));
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .require_second_factor(
            ServiceId::Identity,
            SecondFactorChallenge {
                methods: vec![SecondFactorMethod::Wechat],
                masked_phone: None,
                expires_at: expired.then(|| Utc::now() - chrono::Duration::seconds(1)),
            },
            Some(UserIdentity {
                username: "fixture-primary".into(),
                display_name: None,
            }),
        )
        .unwrap();
    r.primary_password = Some("synthetic-password".into());
    r
}

#[tokio::test]
async fn backend_repair_consistency_primary_expired_challenge_dispatches_neither_send_nor_verify() {
    let s = FixtureServer::new(vec![Reply::json(r#"{"result":"success"}"#)]);
    let mut r = pending_identity(&s, true);
    assert!(r.send_second_factor_code("wechat".into()).await.is_err());
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert!(s.requests().is_empty());
}

#[test]
fn backend_repair_consistency_primary_expired_challenge_is_not_actionable_in_status() {
    let s = FixtureServer::new(vec![]);
    let r = pending_identity(&s, true);
    let status = r.status();
    assert_eq!(status.state, "expired");
    assert!(status.second_factor_methods.is_empty());
    assert!(status.masked_phone.is_none());
}

#[tokio::test]
async fn backend_repair_consistency_primary_unknown_verify_outcome_cannot_replay() {
    let s = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: "synthetic-outage".into(),
    }]);
    let mut r = pending_identity(&s, false);
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert_ne!(
        r.coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .state,
        ServiceSessionState::RequiresSecondFactor
    );
    assert!(
        r.primary_password.is_none(),
        "discard no-longer-usable pending primary credentials"
    );
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert_eq!(s.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_consistency_primary_verified_flow_missing_redirect_cannot_replay() {
    let s = FixtureServer::new(vec![Reply::json(
        r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE"}}"#,
    )]);
    let mut r = pending_identity(&s, false);
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert_ne!(
        r.coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .state,
        ServiceSessionState::RequiresSecondFactor
    );
    assert!(
        r.complete_second_factor("wechat".into(), "654321".into())
            .await
            .is_err()
    );
    assert_eq!(s.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_consistency_primary_rejected_code_retains_explicit_retry_and_password() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"result":"fail"}"#),
        Reply::json(r#"{"result":"fail"}"#),
    ]);
    let mut r = pending_identity(&s, false);
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert_eq!(r.status().state, "requires_second_factor");
    assert!(r.primary_password.is_some());
    assert!(
        r.complete_second_factor("wechat".into(), "654321".into())
            .await
            .is_err()
    );
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_consistency_primary_unoffered_method_is_local_error_not_abandoned_challenge()
 {
    let s = FixtureServer::new(vec![]);
    let mut r = pending_identity(&s, false);
    assert!(
        r.complete_second_factor("totp".into(), "123456".into())
            .await
            .is_err()
    );
    assert_eq!(r.status().state, "requires_second_factor");
    assert!(r.primary_password.is_some());
    assert!(s.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_consistency_network_runtime_uses_existing_online_proof_once() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"error":"ok","ecode":0,"challenge":"synthetic-token"}"#),
        Reply::json(r#"{"error":"ok","ecode":0}"#),
        Reply::json(r#"{"error":"ok","online_ip":"192.0.2.10","user_name":"fixture-net"}"#),
    ]);
    let profile = crate::tunet::TunetProfile::current_with_overrides(
        crate::tunet::AuthFamily::Auth4,
        crate::tunet::TunetProfileOverrides {
            endpoint: Some(
                crate::tunet::HttpsEndpoint::http(
                    "127.0.0.1",
                    Url::parse(s.base()).unwrap().port().unwrap(),
                )
                .unwrap(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    let client = TunetClient::with_transport(
        TunetClientConfig::new(profile).unwrap(),
        crate::transport::CampusHttpTransport::new("THYou/consistency-fixture").unwrap(),
    )
    .unwrap();
    let mut r = runtime();
    assert!(
        r.login_tunet_using(
            "fixture-net".into(),
            zeroize::Zeroizing::new("synthetic-password".to_owned()),
            "192.0.2.10".into(),
            client
        )
        .await
        .is_ok()
    );
    assert!(r.tunet_connection_target.is_some());
    assert_eq!(
        s.requests().len(),
        3,
        "challenge, login, one successful status proof only"
    );
}
