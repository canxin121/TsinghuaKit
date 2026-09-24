//! Reference-aligned recovery tests. All credentials and servers are fixtures.
use super::*;

#[test]
fn backend_repair_startup_overview_recovery_fits_default_worker_stack() {
    let root = LifecycleAuditRoot::new("startup-overview-worker-stack");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![]);
    let mut runtime = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut runtime,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    // The launch crashed while constructing the nested resume observation,
    // before its body could honor this gate. A cooldown must remain a cheap,
    // no-network error even when reached from the overview cache-miss path.
    runtime.schedule_safe_login_probe_retry();
    let (result, timing) = std::thread::Builder::new()
        .name("thyou-startup-stack-fixture".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(crate::telemetry::timing::capture(
                    runtime.load_overview("2026-09-22".into()),
                ))
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(result.unwrap_err(), reference_recovery::RETRY_NOTICE);
    assert_eq!(timing.requests, 0);
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
    assert!(portal.requests().is_empty());
}

#[test]
fn backend_repair_startup_worker_stack_keeps_recovery_probe_once_only() {
    let root = LifecycleAuditRoot::new("startup-recovery-probe-worker-stack");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let mut runtime = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut runtime,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    let (first, first_timing, second, second_timing) = std::thread::Builder::new()
        .name("thyou-startup-probe-stack-fixture".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let (first, first_timing) = crate::telemetry::timing::capture(
                        runtime.load_overview("2026-09-22".into()),
                    )
                    .await;
                    let (second, second_timing) = crate::telemetry::timing::capture(
                        runtime.load_overview("2026-09-22".into()),
                    )
                    .await;
                    assert_eq!(runtime.status().state, "expired");
                    assert!(runtime.primary_password.is_none());
                    assert!(runtime.session_recovery_retry_seconds().is_some());
                    (first, first_timing, second, second_timing)
                })
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(first.unwrap_err(), reference_recovery::RETRY_NOTICE);
    assert_eq!(second.unwrap_err(), reference_recovery::RETRY_NOTICE);
    assert_eq!((first_timing.requests, second_timing.requests), (1, 0));
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
    assert_eq!(portal.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_perf_saved_recovery_timing_preserves_cooldown_and_single_auth_boundary() {
    let root = LifecycleAuditRoot::new("timed-recovery-cooldown");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let mut runtime = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut runtime,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    let (first, timing) =
        crate::telemetry::timing::capture(runtime.resume_restored_session()).await;
    assert_eq!(first.unwrap().state, "expired");
    assert_eq!(timing.requests, 1);
    assert_eq!(
        timing.phases[&crate::telemetry::timing::Phase::AuthBootstrap].count,
        1
    );
    assert!(runtime.session_recovery_retry_seconds().is_some());
    let (second, second_timing) =
        crate::telemetry::timing::capture(runtime.resume_restored_session()).await;
    assert_eq!(second.unwrap().state, "expired");
    assert_eq!(second_timing.requests, 0);
    assert!(identity.requests().is_empty() && oauth.requests().is_empty());
    assert_eq!(portal.requests().len(), 1);
    assert!(runtime.primary_password.is_none());
}

#[tokio::test]
async fn backend_repair_reference_reconciliation_rejects_mismatched_owner_before_network() {
    let root = LifecycleAuditRoot::new("reference-owner-disagreement");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(
            ServiceId::Identity,
            UserIdentity {
                username: "other-fixture-owner".into(),
                display_name: None,
            },
            None,
            None,
            None,
        )
        .unwrap();
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .unwrap();
    r.schedule_safe_login_probe_retry();
    assert!(r.session_recovery_retry_seconds().is_none());
    assert!(r.resume_restored_session().await.is_err());
    assert!(
        portal.requests().is_empty()
            && identity.requests().is_empty()
            && oauth.requests().is_empty()
    );
}

fn login_form() -> Reply {
    Reply::html(&format!(
        "<html><span id='sm2publicKey'>{}</span><form method='post' action='/do/off/ui/auth/login/check'><input name='i_user'><input type='password' name='i_pass'></form></html>",
        recovery_public_key()
    ))
}

#[tokio::test]
async fn backend_repair_reference_malformed_bootstrap_is_not_a_timed_password_retry() {
    let root = LifecycleAuditRoot::new("reference-malformed-bootstrap");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![Reply::html("<html>unexpected fixture page</html>")]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.resume_restored_session().await.unwrap();
    assert!(r.session_recovery_retry_seconds().is_none());
    r.resume_restored_session().await.unwrap();
    assert_eq!(portal.requests().len(), 1);
    assert!(identity.requests().is_empty());
    assert!(r.primary_password.is_none());
}

#[test]
fn backend_repair_reference_safe_recovery_backoff_is_bounded_and_logout_revokes_it() {
    let root = LifecycleAuditRoot::new("reference-backoff");
    lifecycle_audit_metadata(&root.0);
    let mut r = persistent_runtime(&root.0);
    for expected in [60, 120, 240, 300, 300] {
        r.schedule_safe_login_probe_retry();
        assert_eq!(r.session_recovery_retry_seconds(), Some(expected));
    }
    r.logout().unwrap();
    assert!(r.session_recovery_retry_seconds().is_none());
    assert!(r.recovery.resume_retry_at.is_none());
}

#[tokio::test]
async fn backend_repair_reference_later_cookie_expiry_can_recover_more_than_once() {
    let root = LifecycleAuditRoot::new("reference-two-expiries");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![
        lifecycle_cookie(),
        lifecycle_account(),
        lifecycle_cookie(),
        lifecycle_account(),
    ]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Identity, user(), None, None, None)
        .unwrap();
    r.resume_attempt = ResumeAttemptState::Consumed;
    for expected in [2, 4] {
        r.coordinator
            .registry_mut()
            .transition(
                ServiceId::Identity,
                crate::session::SessionTransition::Expire,
            )
            .unwrap();
        assert_eq!(
            r.resume_restored_session().await.unwrap().state,
            "authenticated"
        );
        assert_eq!(portal.requests().len(), expected);
        assert!(r.session_recovery_retry_seconds().is_none());
    }
    assert!(identity.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_reference_pending_verification_and_revoked_authority_dispatch_nothing() {
    let root = LifecycleAuditRoot::new("reference-challenge-boundary");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Identity, user(), None, None, None)
        .unwrap();
    r.service_second_factor = Some(PendingServiceSecondFactor {
        target: ServiceSecondFactorTarget::Portal,
        challenge: crate::protocol::SecondFactorChallenge {
            methods: vec![crate::protocol::SecondFactorMethod::Totp],
            masked_phone: None,
            expires_at: None,
        },
    });
    r.schedule_safe_login_probe_retry();
    assert_eq!(
        r.resume_restored_session().await.unwrap().state,
        "requires_second_factor"
    );
    assert!(r.session_recovery_retry_seconds().is_none());
    r.service_second_factor = None;
    let lease = r.recovery_lease.clone().unwrap();
    crate::session_persistence::revoke_authority(&root.0, Some(&lease), Some("fixture-user"))
        .unwrap();
    assert!(r.resume_restored_session().await.is_err());
    assert!(r.session_recovery_retry_seconds().is_none());
    assert!(
        identity.requests().is_empty()
            && portal.requests().is_empty()
            && oauth.requests().is_empty()
    );
}

#[test]
fn backend_repair_reference_primary_recovery_releases_only_dependent_safe_failures() {
    let root = LifecycleAuditRoot::new("reference-dependent-reader");
    lifecycle_audit_metadata(&root.0);
    let mut r = persistent_runtime(&root.0);
    r.schedule_safe_login_probe_retry();
    r.record_recovery_result(
        ServiceId::Info,
        &Err(reference_recovery::RETRY_NOTICE.into()),
        r.identity.transport().replay_fence(),
    );
    r.automatic_refresh_results.insert(
        ServiceId::CampusCard,
        Err("ambiguous fixture handoff".into()),
    );
    assert!(r.recovery.retry_at.contains_key(&ServiceId::Info));
    r.finish_primary_recovery_cycle();
    assert!(!r.automatic_refresh_results.contains_key(&ServiceId::Info));
    assert!(
        r.automatic_refresh_results
            .contains_key(&ServiceId::CampusCard)
    );
    assert!(r.session_recovery_retry_seconds().is_none());
}

fn login_success() -> Reply {
    Reply { status: 200,
        headers: "Content-Type: text/html; charset=utf-8\r\nSet-Cookie: identity-session=fixture-restored; Path=/\r\n".into(),
        body: "<html><body>login success; redirecting</body></html>".into() }
}

fn oauth_redirect(identity: &FixtureServer) -> Reply {
    Reply {
        status: 302,
        headers: format!(
            "Location: {}do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app\r\n",
            identity.base()
        ),
        body: String::new(),
    }
}

fn webvpn_redirect(oauth: &FixtureServer) -> Reply {
    Reply {
        status: 302,
        headers: format!("Location: {}thu-oauth/auth?state=fixture\r\n", oauth.base()),
        body: String::new(),
    }
}

#[tokio::test]
async fn backend_repair_reference_saved_login_pre_auth_outage_recovers_without_restarting_app() {
    let root = LifecycleAuditRoot::new("reference-preauth-outage");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![login_form(), login_success()]);
    let oauth = FixtureServer::new(vec![oauth_redirect(&identity)]);
    let portal = FixtureServer::new(vec![
        unavailable(),
        webvpn_redirect(&oauth),
        lifecycle_cookie(),
        lifecycle_account(),
    ]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    let first = r.resume_restored_session().await.unwrap();
    assert_eq!(first.state, "expired");
    assert!(first.automatic_recovery_enabled);
    assert!(
        r.recovery.resume_retry_at.is_some(),
        "pre-auth outage needs a scheduled retry, not permanent consumption"
    );
    assert!(r.primary_password.is_none());
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
    r.resume_restored_session().await.unwrap();
    assert_eq!(
        portal.requests().len(),
        1,
        "no request burst during cooldown"
    );
    r.recovery.resume_retry_at = Some(std::time::Instant::now() - Duration::from_secs(1));
    let recovered = r.resume_restored_session().await.unwrap();
    assert_eq!(recovered.state, "authenticated");
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.primary_password.is_none());
    assert_eq!(
        identity
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
    assert_eq!(portal.requests().len(), 4);
    assert!(r.recovery.resume_retry_at.is_none());
}

#[tokio::test]
async fn backend_repair_reference_resume_entrypoint_handles_later_live_identity_expiry() {
    let root = LifecycleAuditRoot::new("reference-later-expiry");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![lifecycle_cookie(), lifecycle_account()]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Identity, user(), None, None, None)
        .unwrap();
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .unwrap();
    r.resume_attempt = ResumeAttemptState::Consumed;
    r.restored_session = false;
    let status = r.resume_restored_session().await.unwrap();
    assert_eq!(
        status.state, "authenticated",
        "foreground reconciliation must not be a startup-only no-op"
    );
    assert_eq!(portal.requests().len(), 2);
    assert!(
        identity.requests().is_empty(),
        "valid cookies never require a password"
    );
}

#[tokio::test]
async fn backend_repair_reference_ambiguous_primary_post_never_becomes_automatic_retry() {
    let root = LifecycleAuditRoot::new("reference-ambiguous-post");
    lifecycle_audit_metadata(&root.0);
    let identity = FixtureServer::new(vec![login_form(), unavailable()]);
    let oauth = FixtureServer::new(vec![oauth_redirect(&identity)]);
    let portal = FixtureServer::new(vec![webvpn_redirect(&oauth)]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.resume_restored_session().await.unwrap();
    assert!(r.recovery.resume_retry_at.is_none());
    assert!(r.credential_recovery_attempted);
    r.resume_restored_session().await.unwrap();
    assert_eq!(
        identity
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
    assert_eq!(portal.requests().len(), 1);
    assert!(r.primary_password.is_none());
}
