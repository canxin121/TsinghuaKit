//! Fresh-password presence must never force another target-app login.
//! Every request below is synthetic and confined to loopback fixture servers.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

#[path = "runtime_reference_recovery_tests.rs"]
mod reference_recovery_tests;
use crate::{session_persistence::ResumeSnapshot, transport::CampusCookieStore};
use reqwest::Url;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn user() -> UserIdentity {
    UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    }
}

fn current_resume_device(root: &Path) -> String {
    terminal_device::resolve(&root.join("cache.json"))
        .expect("fixture device identity should be available")
}

fn fixture_device() -> &'static str {
    "0123456789abcdef0123456789abcdef"
}

fn persistence_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "thyou-session-reuse-{label}-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).expect("fixture persistence root should be creatable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("fixture persistence root should be private");
    }
    root
}

fn cleanup_persistence_root(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn backend_repair_private_vault_io_failure_does_not_permanently_consume_resume_gate() {
    let root = persistence_root("vault-io-retry");
    let device = current_resume_device(&root);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        "fixture-user",
        Some(AcademicStage::Graduate),
        Some(&device),
        true,
    )
    .expect("safe recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata).unwrap();

    // Make the vault path temporarily unusable without creating any password
    // or cookie fixture. The first recovery attempt must stop before any
    // authentication request and release only the local retry gate.
    fs::write(root.join("credentials"), b"temporarily unavailable")
        .expect("temporary vault obstruction");
    let mut runtime = persistent_runtime(&root);
    let first = runtime
        .resume_restored_session()
        .await
        .expect("local vault failure is reported as a status");
    assert_eq!(first.state, "expired");
    assert!(!runtime.credential_recovery_attempted);
    assert!(!runtime.private_credential_attempted);

    // Once the path becomes readable, the same Runtime may inspect it again.
    // This second attempt sees a valid but empty vault and consumes only the
    // no-record result; it still must not dispatch a login POST.
    fs::remove_file(root.join("credentials")).expect("remove temporary obstruction");
    fs::create_dir(root.join("credentials")).expect("restore vault directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("credentials"), fs::Permissions::from_mode(0o700))
            .expect("private vault directory");
    }
    let second = runtime
        .resume_restored_session()
        .await
        .expect("second local recovery inspection");
    assert_eq!(second.state, "expired");
    assert!(runtime.credential_recovery_attempted);
    assert!(runtime.private_credential_attempted);
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_cache_miss_retries_transient_vault_failure_and_reuses_one_recovery() {
    let root = persistence_root("cache-miss-vault-retry");
    let account = "fixture-student";
    let device = current_resume_device(&root);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        account,
        Some(AcademicStage::Graduate),
        Some(&device),
        true,
    )
    .expect("safe recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata)
        .expect("recovery metadata persists");

    // A cache miss in a newly created Runtime must be able to reach the same
    // Rust-owned recovery boundary as startup.  Begin with a temporary vault
    // obstruction to prove that the first attempt makes no login request and
    // leaves the local gate retryable.
    fs::write(root.join("credentials"), b"temporarily unavailable")
        .expect("temporary vault obstruction");

    let identity = FixtureServer::new(vec![
        Reply::html(&format!(
            "<html><span id='sm2publicKey'>{}</span><form method='post' action='/do/off/ui/auth/login/check'><input name='i_user'><input type='password' name='i_pass'></form></html>",
            recovery_public_key()
        )),
        Reply {
            status: 200,
            headers: "Content-Type: text/html; charset=utf-8\r\nSet-Cookie: identity-session=recovered; Path=/\r\n".into(),
            body: "<html><body>login success; redirecting</body></html>".into(),
        },
    ]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app\r\n",
            identity.base()
        ),
        body: String::new(),
    }]);
    let webvpn = FixtureServer::new(vec![
        Reply {
            status: 302,
            headers: format!("Location: {}thu-oauth/auth?state=fixture\r\n", oauth.base()),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-student"}}"#),
    ]);
    let config = WebVpnIdentityConfig::new(webvpn.base(), oauth.base(), identity.base())
        .expect("loopback identity graph");
    let mut runtime = persistent_runtime(&root);
    inject_webvpn_identity_config(&mut runtime, config);

    let first = runtime.ensure_identity_user_for_live_read().await;
    assert!(
        first.is_err(),
        "a blocked vault must not authorize a live read"
    );
    assert_eq!(
        runtime.resume_attempt,
        ResumeAttemptState::RetryableLocalFailure
    );
    assert!(!runtime.credential_recovery_attempted);
    assert!(!runtime.private_credential_attempted);
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
    assert!(webvpn.requests().is_empty());

    // Restore the private vault without replacing the Runtime. The next cache
    // miss may retry local storage, then perform exactly one bounded primary
    // recovery using the matching account/device/stage record.
    fs::remove_file(root.join("credentials")).expect("remove vault obstruction");
    fs::create_dir(root.join("credentials")).expect("restore vault directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("credentials"), fs::Permissions::from_mode(0o700))
            .expect("private vault directory");
    }
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password-never-logged",
        Some(AcademicStage::Graduate),
        &device,
        false,
    )
    .expect("private credential persists");

    let recovered = runtime
        .ensure_identity_user_for_live_read()
        .await
        .expect("cache miss should recover the saved Identity");
    assert_eq!(recovered.username, account);
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert_eq!(runtime.resume_attempt, ResumeAttemptState::Consumed);
    assert_eq!(identity.requests().len(), 2);
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(webvpn.requests().len(), 3);

    let request_counts = (
        identity.requests().len(),
        oauth.requests().len(),
        webvpn.requests().len(),
    );
    runtime
        .ensure_identity_user_for_live_read()
        .await
        .expect("a second cache miss in the same Runtime reuses the proof");
    assert_eq!(
        request_counts,
        (
            identity.requests().len(),
            oauth.requests().len(),
            webvpn.requests().len(),
        )
    );

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_cache_miss_does_not_reprobe_failed_restored_cookie() {
    let root = persistence_root("cache-miss-restored-cookie-boundary");
    let account = "fixture-user";
    let device = current_resume_device(&root);
    let store = CampusCookieStore::default();
    let seed_url = Url::parse("https://seed.fixture.example/").expect("seed URL");
    store.add_cookie_str("resume-seed=present; Path=/", &seed_url);
    let snapshot = ResumeSnapshot::new(
        account,
        &store.snapshot_bytes().expect("resume cookie snapshot"),
    )
    .expect("resume snapshot");
    let snapshot = snapshot
        .with_device_fingerprint(&device)
        .expect("device-bound snapshot");
    crate::session_persistence::save_at_root(&root, &snapshot).expect("resume snapshot persists");
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        account,
        Some(AcademicStage::Graduate),
        Some(&device),
        true,
    )
    .expect("safe recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata)
        .expect("recovery metadata persists");

    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let oauth = FixtureServer::new(vec![]);
    let config = WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base())
        .expect("loopback identity graph");
    let mut runtime = persistent_runtime(&root);
    inject_webvpn_identity_config(&mut runtime, config);
    assert!(runtime.restored_session);

    let first = runtime.ensure_identity_user_for_live_read().await;
    assert!(
        first.is_err(),
        "unproven restored cookies cannot authorize a miss"
    );
    assert!(runtime.restored_session);
    assert_eq!(portal.requests().len(), 1);
    assert!(!runtime.private_credential_attempted);
    assert!(!runtime.credential_recovery_attempted);

    // The startup/cache recovery gate is consumed by the ambiguous portal
    // result. A second reader must surface the same boundary without probing
    // the one-shot portal again or opening the private credential vault.
    let second = runtime.ensure_identity_user_for_live_read().await;
    assert!(second.is_err());
    assert_eq!(portal.requests().len(), 1);
    assert!(!runtime.private_credential_attempted);
    assert!(!runtime.credential_recovery_attempted);

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_restored_login_boundary_followed_by_network_failure_does_not_read_private_vault()
 {
    let root = persistence_root("restored-login-boundary-network-failure");
    let account = "fixture-user";
    let device = current_resume_device(&root);
    let store = CampusCookieStore::default();
    let seed_url = Url::parse("https://seed.fixture.example/").expect("seed URL");
    store.add_cookie_str("resume-seed=present; Path=/", &seed_url);
    let snapshot = ResumeSnapshot::new(
        account,
        &store.snapshot_bytes().expect("resume cookie snapshot"),
    )
    .expect("resume snapshot")
    .with_device_fingerprint(&device)
    .expect("device-bound snapshot");
    crate::session_persistence::save_at_root(&root, &snapshot).expect("resume snapshot persists");
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        account,
        Some(AcademicStage::Graduate),
        Some(&device),
        true,
    )
    .expect("safe recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata)
        .expect("recovery metadata persists");
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password-never-logged",
        Some(AcademicStage::Graduate),
        &device,
        false,
    )
    .expect("private credential persists");

    // First the target resource explicitly redirects to the WebVPN login
    // boundary. The trusted continuation then receives a transient 503. That
    // second result is not permission to replay the saved password.
    let portal = FixtureServer::new(vec![
        Reply::html(""),
        Reply {
            status: 302,
            headers: "Location: /login\r\n".into(),
            body: String::new(),
        },
        unavailable(),
    ]);
    let oauth = FixtureServer::new(vec![]);
    let identity = FixtureServer::new(vec![]);
    let config = WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base())
        .expect("loopback identity graph");
    let mut runtime = persistent_runtime(&root);
    inject_webvpn_identity_config(&mut runtime, config);
    assert!(runtime.restored_session);

    let first = runtime.ensure_identity_user_for_live_read().await;
    assert!(first.is_err());
    assert!(runtime.restored_session);
    assert_eq!(portal.requests().len(), 3);
    assert!(!runtime.portal_password_required);
    assert!(!runtime.private_credential_attempted);
    assert!(!runtime.credential_recovery_attempted);

    // The consumed resume boundary is shared by later cache readers; no
    // second probe or private-vault read is allowed in the same Runtime.
    let second = runtime.ensure_identity_user_for_live_read().await;
    assert!(second.is_err());
    assert_eq!(portal.requests().len(), 3);
    assert!(!runtime.private_credential_attempted);
    assert!(!runtime.credential_recovery_attempted);

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_unproven_restored_cookie_does_not_turn_cache_read_into_password_login() {
    let root = persistence_root("restored-network-boundary");
    let device = current_resume_device(&root);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        "fixture-user",
        Some(AcademicStage::Graduate),
        Some(&device),
        true,
    )
    .expect("safe recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata).unwrap();

    // If the stale-cache reader incorrectly treats an unproven restored Cookie
    // as an expired live Identity session, the obstructed vault would be
    // inspected here and release the resume gate as a local recovery failure.
    // The correct path calls the already-consumed startup resume gate and does
    // not inspect credentials again after an ambiguous portal/network result.
    fs::write(root.join("credentials"), b"temporarily unavailable")
        .expect("temporary vault obstruction");
    let mut runtime = persistent_runtime(&root);
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity authentication begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Identity, user(), None, None, None)
        .expect("restored identity shell installs");
    runtime.restored_session = true;
    runtime.resume_attempt = ResumeAttemptState::Consumed;

    assert!(!runtime.try_restore_identity_for_cache_refresh().await);
    assert_eq!(runtime.resume_attempt, ResumeAttemptState::Consumed);
    assert!(!runtime.credential_recovery_attempted);
    assert!(!runtime.private_credential_attempted);

    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_empty_cache_path_reuses_the_injected_private_root() {
    let root = persistence_root("cache-root");
    let runtime = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        String::new(),
        true,
        root.clone(),
    )
    .expect("runtime with injected persistence root");

    assert_eq!(
        runtime.cache_path,
        root.join("cache").join("thyou-overview.json")
    );
    assert!(runtime.cache_path.starts_with(&root));
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_persistent_cache_path_stays_inside_private_root() {
    let root = persistence_root("cache-path-allowlist");
    let nested = root.join("cache").join("custom-overview.json");
    let runtime = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        nested.to_string_lossy().into_owned(),
        true,
        root.clone(),
    )
    .expect("a cache path below the private root should be accepted");
    assert_eq!(runtime.cache_path, nested);
    cleanup_persistence_root(&root);

    let root = persistence_root("cache-path-outside");
    let outside = std::env::temp_dir()
        .join(format!("thyou-cache-outside-{}", Uuid::new_v4().simple()))
        .join("overview.json");
    let result = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        outside.to_string_lossy().into_owned(),
        true,
        root.clone(),
    );
    assert!(
        result.is_err(),
        "persistent cache paths must be allowlisted"
    );
    assert!(!outside.exists(), "rejected paths must not be created");
    cleanup_persistence_root(&root);
}

#[cfg(unix)]
#[test]
fn backend_repair_persistent_cache_path_rejects_symlinked_target() {
    use std::os::unix::fs::symlink;

    let root = persistence_root("cache-path-symlink");
    let outside_root = persistence_root("cache-path-symlink-target");
    let outside = outside_root.join("overview.json");
    fs::write(&outside, b"not a cache").expect("fixture target");
    let linked = root.join("overview.json");
    symlink(&outside, &linked).expect("fixture symlink");

    let result = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        linked.to_string_lossy().into_owned(),
        true,
        root.clone(),
    );
    assert!(result.is_err(), "symlinked cache targets must fail closed");

    cleanup_persistence_root(&root);
    cleanup_persistence_root(&outside_root);
}

fn persistent_runtime(root: &Path) -> CampusRuntime {
    CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        true,
        root.to_owned(),
    )
    .expect("fixture runtime should restore from its private root")
}

#[cfg(unix)]
#[test]
fn backend_repair_resume_store_lock_error_is_visible_in_status() {
    use std::os::unix::fs::symlink;

    let root = persistence_root("resume-lock-error");
    symlink(
        root.join("missing-lock-target"),
        root.join("campus-session-store-v1.lock"),
    )
    .expect("fixture lock symlink");

    let runtime = persistent_runtime(&root);
    let status = runtime.status();
    assert_eq!(status.state, "signed_out");
    assert_eq!(
        status.error.as_deref(),
        Some("THYou 本地恢复文件暂时不可读，自动续接已暂停，请检查应用数据目录权限")
    );
    assert!(!runtime.restored_session);
    assert!(runtime.resume_account_metadata.is_none());
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_expired_resume_cookies_do_not_restore_authenticated_identity() {
    let root = persistence_root("expired-cookie");
    let store = CampusCookieStore::default();
    let url = Url::parse("https://fixture.example/").unwrap();
    store.add_cookie_str(
        "expired=one; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/",
        &url,
    );
    let snapshot = ResumeSnapshot::new(
        "fixture-user",
        &store.snapshot_bytes().expect("expired cookie snapshot"),
    )
    .unwrap()
    .with_device_fingerprint(&current_resume_device(&root))
    .unwrap();
    crate::session_persistence::save_at_root(&root, &snapshot).unwrap();

    let runtime = persistent_runtime(&root);
    assert_eq!(runtime.status().state, "signed_out");
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    assert!(
        !crate::session_persistence::load_at_root(&root)
            .unwrap()
            .is_some(),
        "the rejected snapshot must be removed"
    );
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_resume_portal_checkpoint_is_not_current_service_proof() {
    let root = persistence_root("portal-checkpoint");
    let store = CampusCookieStore::default();
    let url = Url::parse("https://fixture.example/").unwrap();
    store.add_cookie_str("session=present; Path=/", &url);
    let mut snapshot = ResumeSnapshot::new(
        "fixture-user",
        &store.snapshot_bytes().expect("session cookie snapshot"),
    )
    .unwrap();
    snapshot = snapshot
        .with_device_fingerprint(&current_resume_device(&root))
        .unwrap();
    snapshot.portal_bootstrap_completed = true;
    crate::session_persistence::save_at_root(&root, &snapshot).unwrap();

    let runtime = persistent_runtime(&root);
    assert_eq!(runtime.status().state, "authenticated");
    assert!(!runtime.portal_bootstrapped);
    assert!(runtime.portal_csrf.is_none());
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_resume_snapshot_from_another_device_is_rejected_before_identity_restore() {
    let root = persistence_root("foreign-device");
    let store = CampusCookieStore::default();
    let url = Url::parse("https://fixture.example/").unwrap();
    store.add_cookie_str("session=foreign; Path=/", &url);
    let snapshot = ResumeSnapshot::new(
        "fixture-user",
        &store.snapshot_bytes().expect("session cookie snapshot"),
    )
    .unwrap()
    .with_device_fingerprint("ffffffffffffffffffffffffffffffff")
    .unwrap();
    crate::session_persistence::save_at_root(&root, &snapshot).unwrap();

    let runtime = persistent_runtime(&root);
    assert_eq!(runtime.status().state, "signed_out");
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    assert!(
        crate::session_persistence::load_at_root(&root)
            .unwrap()
            .is_none()
    );
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_resume_account_mismatch_keeps_metadata_as_expired_cache_shell() {
    let root = persistence_root("account-mismatch");
    let store = CampusCookieStore::default();
    let url = Url::parse("https://fixture.example/").unwrap();
    store.add_cookie_str("session=ambiguous; Path=/", &url);
    let snapshot = ResumeSnapshot::new(
        "fixture-user",
        &store.snapshot_bytes().expect("session cookie snapshot"),
    )
    .unwrap()
    .with_device_fingerprint(&current_resume_device(&root))
    .unwrap();
    crate::session_persistence::save_at_root(&root, &snapshot).unwrap();
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        "another-fixture-user",
        Some(AcademicStage::Undergraduate),
        Some(&current_resume_device(&root)),
        false,
    )
    .unwrap();
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata).unwrap();

    let runtime = persistent_runtime(&root);
    // The surviving metadata is a cache/account scope, not live proof.  The
    // runtime therefore exposes an expired cache shell while keeping all
    // network capabilities unauthenticated.
    assert_eq!(runtime.status().state, "expired");
    assert_eq!(
        runtime.status().username.as_deref(),
        Some("another-fixture-user")
    );
    assert!(
        crate::session_persistence::load_at_root(&root)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        crate::session_persistence::load_account_metadata_at_root(&root)
            .unwrap()
            .as_ref()
            .map(|value| value.username.as_str()),
        Some("another-fixture-user")
    );
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_metadata_only_resume_is_expired_cache_shell_not_identity_proof() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server);
    runtime.coordinator.logout(ServiceId::Identity).unwrap();
    runtime.resume_account_metadata = Some(
        crate::session_persistence::ResumeAccountMetadata::new(
            "fixture-user",
            Some(AcademicStage::Graduate),
            Some(fixture_device()),
            false,
        )
        .expect("safe cache metadata"),
    );

    let status = runtime.status();
    assert_eq!(status.state, "expired");
    assert_eq!(status.username.as_deref(), Some("fixture-user"));
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    assert!(!status.overview_available);
    assert!(!status.automatic_recovery_enabled);
    assert!(server.requests().is_empty());
}

#[test]
fn backend_repair_runtime_restore_preserves_explicit_stage_over_reference_rule() {
    // The Reference digit rule maps this synthetic student id to Graduate.
    // Persisting an explicit Undergraduate choice must remain authoritative
    // when the next Rust Runtime is created from the private recovery files.
    let account = "2026000000";
    let root = persistence_root("explicit-stage");
    let device = current_resume_device(&root);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new_with_stage_selection(
        account,
        Some(AcademicStage::Undergraduate),
        Some(&device),
        true,
        true,
    )
    .expect("explicit stage recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata).unwrap();
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password",
        Some(AcademicStage::Undergraduate),
        &device,
        true,
    )
    .expect("explicit stage private credential");

    let mut runtime = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        true,
        root.clone(),
    )
    .expect("runtime restores explicit stage context");

    assert_eq!(runtime.stage, AcademicStage::Undergraduate);
    assert!(runtime.stage_selection_explicit);
    assert_eq!(
        runtime.resolved_academic_stage(),
        Some(AcademicStage::Undergraduate)
    );

    // Re-running the Reference inference hook must not overwrite the manual
    // choice merely because the username's fifth digit implies Graduate.
    runtime.apply_reference_academic_stage(&UserIdentity {
        username: account.to_owned(),
        display_name: None,
    });
    assert_eq!(runtime.stage, AcademicStage::Undergraduate);
    assert!(runtime.stage_selection_explicit);
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_runtime_restore_rejects_credential_metadata_stage_mismatch_before_login() {
    let account = "2026000000";
    let root = persistence_root("stage-mismatch");
    let device = current_resume_device(&root);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new_with_stage_selection(
        account,
        Some(AcademicStage::Undergraduate),
        Some(&device),
        true,
        true,
    )
    .expect("explicit stage recovery metadata");
    crate::session_persistence::save_account_metadata_at_root(&root, &metadata).unwrap();
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password",
        Some(AcademicStage::Graduate),
        &device,
        true,
    )
    .expect("mismatched private credential");

    let mut runtime = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        true,
        root.clone(),
    )
    .expect("runtime restores metadata before checking credentials");
    let result = runtime
        .recover_restored_session_with_private_credential(
            &UserIdentity {
                username: account.to_owned(),
                display_name: None,
            },
            false,
        )
        .await
        .expect("stage mismatch is a recoverable local boundary error");

    assert!(result.is_none());
    assert_eq!(runtime.stage, AcademicStage::Undergraduate);
    assert!(runtime.stage_selection_explicit);
    assert!(
        runtime
            .credential_storage_warning
            .as_deref()
            .is_some_and(|message| message.contains("学段"))
    );
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    cleanup_persistence_root(&root);
}

#[test]
fn backend_repair_restored_cookie_is_not_live_identity_proof_before_portal_revalidation() {
    let root = persistence_root("restored-pending");
    let store = CampusCookieStore::default();
    let url = Url::parse("https://fixture.example/").unwrap();
    store.add_cookie_str("session=pending; Path=/", &url);
    let snapshot = ResumeSnapshot::new(
        "fixture-user",
        &store.snapshot_bytes().expect("session cookie snapshot"),
    )
    .unwrap()
    .with_device_fingerprint(&current_resume_device(&root))
    .unwrap();
    crate::session_persistence::save_at_root(&root, &snapshot).unwrap();

    let runtime = persistent_runtime(&root);
    assert_eq!(runtime.status().state, "authenticated");
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    assert!(runtime.restored_session);
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_reuse_runtime_passes_explicit_trust_consent_to_service_factor_only() {
    for consent in [false, true] {
        let mut replies = vec![Reply::json(
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        )];
        if consent {
            replies.push(Reply::json(r#"{"result":"success"}"#));
        }
        // Deliberately omit a target-business handoff: the device registration
        // result must not falsely prove CampusCard or send any real request.
        replies.push(Reply::html("<html>verified identity callback</html>"));
        let identity = FixtureServer::new(replies);
        let mut r = runtime(&identity);
        r.trust_device_requested = consent;
        r.service_second_factor = Some(PendingServiceSecondFactor {
            target: ServiceSecondFactorTarget::CampusCard,
            challenge: crate::protocol::SecondFactorChallenge {
                methods: vec![crate::protocol::SecondFactorMethod::Wechat],
                masked_phone: None,
                expires_at: None,
            },
        });
        assert!(
            r.complete_second_factor("wechat".into(), "005001".into())
                .await
                .is_err()
        );
        assert_eq!(identity.requests().len(), if consent { 3 } else { 2 });
        assert_eq!(
            identity
                .requests()
                .iter()
                .filter(|request| request.contains("saveFinger"))
                .count(),
            usize::from(consent)
        );
        assert!(!r.service_session_is_proven(ServiceId::CampusCard));
        assert!(r.service_session_is_proven(ServiceId::Identity));
        assert_eq!(
            r.status().trusted_device_status.as_deref(),
            if consent { Some("saved") } else { None }
        );
    }
}

#[test]
fn backend_repair_reuse_log_records_decisions_without_cookie_fingerprint_or_response() {
    let path = std::env::temp_dir().join(format!("thyou-reuse-log-{}", Uuid::new_v4()));
    let mut log = crate::telemetry::LogSession::start(
        &path,
        crate::telemetry::LogConfig::parse("trace", false).unwrap(),
    )
    .unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        trace_session_reuse("campus_card", "cookies_verified");
        tracing::info!(target:"tsinghua_kit::auth",event="trusted_device_registration",service="identity",trust_result="saved");
        tracing::info!(target:"tsinghua_kit::auth",event="session_reuse_decision",service="identity",reuse_result="FIXTURE_PRIVATE_TOKEN",trust_result="PRIVATE_DEVICE_IDENTIFIER");
    });
    log.flush();
    let text = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["fields"]["reuse_result"], "cookies_verified");
    assert_eq!(rows[1]["fields"]["trust_result"], "saved");
    assert_eq!(rows[0]["redacted_fields"], 0);
    assert_eq!(rows[1]["redacted_fields"], 0);
    assert_eq!(rows[2]["redacted_fields"], 2);
    assert!(!text.contains("FIXTURE_PRIVATE_TOKEN"));
    assert!(!text.contains("PRIVATE_DEVICE_IDENTIFIER"));
    drop(log);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn backend_repair_reuse_portal_initialization_can_reuse_sso_without_target_password() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let portal = FixtureServer::new(vec![
        Reply::html(""),
        Reply::html("<html>Existing portal session</html>"),
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-user"}}"#),
    ]);
    let mut r = runtime(&identity);
    let adapter = info(&r, &portal);
    r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
        .await
        .unwrap();
    assert!(r.portal_bootstrapped);
    assert_eq!(portal.requests().len(), 4);
    assert!(identity.requests().is_empty());
    assert!(portal.requests()[1].starts_with("GET /info/f/info/gxfw_fg/common/index "));
}

#[tokio::test]
async fn backend_repair_reuse_real_target_challenges_are_continued_not_replaced_with_password() {
    for target in [
        ServiceSecondFactorTarget::Portal,
        ServiceSecondFactorTarget::CampusCard,
    ] {
        let identity = FixtureServer::new(vec![
            Reply::html("<html>二次认证</html>"),
            Reply::json(r#"{"result":"success","object":{"hasWeChatBool":true,"hasTotp":true}}"#),
        ]);
        let service = FixtureServer::new(vec![Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        }]);
        let mut r = runtime(&identity);
        match target {
            ServiceSecondFactorTarget::Portal => {
                let adapter = info(&r, &service);
                assert!(
                    r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
                        .await
                        .is_err()
                );
            }
            ServiceSecondFactorTarget::CampusCard => {
                card(&mut r, &service);
                assert!(r.ensure_card_session(&user()).await.is_err());
            }
            _ => unreachable!(),
        }
        assert_eq!(r.current_service_second_factor().unwrap().target, target);
        assert_eq!(service.requests().len(), 1);
        assert_eq!(identity.requests().len(), 2);
        assert!(identity.requests()[0].starts_with("GET /do/off/ui/auth/login/form/"));
        assert!(identity.requests()[1].contains("FIND_APPROACHES"));
        assert!(
            identity
                .requests()
                .iter()
                .all(|request| !request.contains("i_pass")
                    && !request.contains("SEND_CODE")
                    && !request.contains("VERITY_CODE"))
        );
        assert!(r.service_session_is_proven(ServiceId::Identity));
    }
}

#[tokio::test]
async fn backend_repair_reuse_proven_card_remains_usable_during_another_pending_factor() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let service = FixtureServer::new(vec![Reply::json(
        r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#,
    )]);
    let mut r = runtime(&identity);
    card(&mut r, &service);
    r.ensure_card_session(&user()).await.unwrap();
    r.service_second_factor = Some(PendingServiceSecondFactor {
        target: ServiceSecondFactorTarget::Portal,
        challenge: crate::protocol::SecondFactorChallenge {
            methods: vec![crate::protocol::SecondFactorMethod::Wechat],
            masked_phone: None,
            expires_at: None,
        },
    });
    r.ensure_card_session(&user()).await.unwrap();
    assert_eq!(service.requests().len(), 1);
    assert!(identity.requests().is_empty());
    assert_eq!(
        r.current_service_second_factor().unwrap().target,
        ServiceSecondFactorTarget::Portal
    );
}

#[test]
fn backend_repair_reuse_ambiguous_portal_failures_are_not_relogin_permissions() {
    for reason in [
        "portal_resume_account_mismatch",
        "portal_resume_account_format",
        "portal_resume_network",
        "portal_resume_cookie_route",
        "portal_after_handoff_login_required",
        "portal_navigation_cycle",
        "portal_navigation_target_rejected",
    ] {
        assert!(!portal_probe_requires_login(reason));
    }
    for reason in [
        "portal_resource_document_login",
        "portal_resource_webvpn_login",
        "portal_resume_csrf_missing",
        "portal_resume_login_required",
    ] {
        assert!(portal_probe_requires_login(reason));
    }
}

#[test]
fn backend_repair_reuse_logout_clears_trust_consent_without_enabling_it_by_default() {
    let identity = FixtureServer::new(vec![]);
    let mut r = runtime(&identity);
    assert!(!r.trust_device_requested);
    r.trust_device_requested = true;
    r.logout().unwrap();
    assert!(!r.trust_device_requested);
    assert!(r.identity.trusted_device_registration_status().is_none());
    assert!(r.trusted_device_status.is_none());
    assert!(identity.requests().is_empty());
}
fn runtime(identity: &FixtureServer) -> CampusRuntime {
    let mut r =
        CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false)
            .unwrap();
    let profile = r.identity.identity().client().config().profile.clone();
    r.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(identity.base(), profile).unwrap()).unwrap(),
        crate::transport::CampusHttpTransport::new("THYou/reuse-fixture").unwrap(),
    ));
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Identity, user(), None, None, None)
        .unwrap();
    r.primary_password = Some("synthetic-password-never-submit".into());
    r
}
fn unavailable() -> Reply {
    Reply {
        status: 503,
        headers: String::new(),
        body: "fixture unavailable".into(),
    }
}
fn info(r: &CampusRuntime, server: &FixtureServer) -> InfoSessionAdapter {
    InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), "/info/").unwrap(),
        r.identity.transport().clone(),
    )
    .unwrap()
}
fn card(r: &mut CampusRuntime, server: &FixtureServer) {
    r.card_client = Some(
        CampusCardClient::new(
            CampusCardAdapterConfig::new(server.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
}

fn recovery_public_key() -> String {
    use sm2::elliptic_curve::sec1::ToSec1Point;

    sm2::SecretKey::from_slice(&[1_u8; 32])
        .expect("fixture SM2 key")
        .public_key()
        .to_sec1_point(false)
        .as_bytes()[1..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn inject_webvpn_identity_config(runtime: &mut CampusRuntime, config: WebVpnIdentityConfig) {
    let profile = runtime
        .identity
        .identity()
        .client()
        .config()
        .profile
        .clone();
    let identity_config = IdentityClientConfig::new(config.identity_origin().to_string(), profile)
        .unwrap()
        .with_anchor_ticket_origins([config.webvpn_origin().to_string()])
        .unwrap()
        .with_cookie_backed_handoff_route(config.webvpn_origin().to_string(), ["/", "/login"])
        .unwrap()
        .with_cookie_backed_handoff_route(
            config.oauth_origin().to_string(),
            ["/thu-oauth/", "/lb-auth/"],
        )
        .unwrap();
    let transport = runtime.identity.transport().clone();
    runtime.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        IdentityClient::new(identity_config).unwrap(),
        transport,
    ));
    runtime.webvpn_identity_config = config;
}

#[tokio::test]
async fn backend_repair_private_credential_recovery_runs_dynamic_bootstrap_once() {
    let root = persistence_root("credential-recovery-loopback");
    let account = "fixture-student";
    let device = current_resume_device(&root);
    crate::session_persistence::save_account_metadata_at_root(
        &root,
        &crate::session_persistence::ResumeAccountMetadata::new(
            account,
            Some(AcademicStage::Graduate),
            Some(&device),
            true,
        )
        .expect("recovery metadata"),
    )
    .expect("recovery metadata persists");
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password-never-logged",
        Some(AcademicStage::Graduate),
        &device,
        false,
    )
    .expect("private credential persists");

    let identity = FixtureServer::new(vec![
        Reply::html(&format!(
            "<html><span id='sm2publicKey'>{}</span><form method='post' action='/do/off/ui/auth/login/check'><input name='i_user'><input type='password' name='i_pass'></form></html>",
            recovery_public_key()
        )),
        Reply {
            status: 200,
            headers: "Content-Type: text/html; charset=utf-8\r\nSet-Cookie: identity-session=recovered; Path=/\r\n".into(),
            body: "<html><body>login success; redirecting</body></html>".into(),
        },
    ]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app\r\n",
            identity.base()
        ),
        body: String::new(),
    }]);
    let webvpn = FixtureServer::new(vec![
        Reply {
            status: 302,
            headers: format!("Location: {}thu-oauth/auth?state=fixture\r\n", oauth.base()),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-student"}}"#),
    ]);
    let config = WebVpnIdentityConfig::new(webvpn.base(), oauth.base(), identity.base())
        .expect("loopback identity graph");

    let mut runtime = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        true,
        root.clone(),
    )
    .expect("persistent recovery runtime");
    inject_webvpn_identity_config(&mut runtime, config);

    assert_eq!(runtime.status().state, "expired");
    let recovered = runtime
        .resume_restored_session()
        .await
        .expect("private credential recovery status");
    assert_eq!(recovered.state, "authenticated");
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert!(runtime.primary_password.is_none());
    assert!(runtime.credential_recovery_attempted == false);
    // One WebVPN request is the dynamic identity bootstrap. The next two are
    // the post-login portal CSRF/account proof; none is repeated by a later
    // resume call.
    assert_eq!(webvpn.requests().len(), 3);
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(identity.requests().len(), 2);
    assert!(identity.requests()[0].starts_with("GET /do/off/ui/auth/login/form/fixture-app/0"));
    assert!(identity.requests()[1].starts_with("POST /do/off/ui/auth/login/check "));
    assert!(!identity.requests()[1].contains("fixture-password-never-logged"));
    assert!(
        !identity
            .requests()
            .iter()
            .any(|request| request.contains("doubleAuth"))
    );

    let request_counts = (
        webvpn.requests().len(),
        oauth.requests().len(),
        identity.requests().len(),
    );
    let second = runtime
        .resume_restored_session()
        .await
        .expect("second resume is bounded");
    assert_eq!(second.state, "authenticated");
    assert_eq!(
        request_counts,
        (
            webvpn.requests().len(),
            oauth.requests().len(),
            identity.requests().len(),
        )
    );

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_proven_portal_releases_private_credential_guard_for_next_expiry() {
    let root = persistence_root("portal-credential-next-cycle");
    let account = "fixture-user";
    let device = current_resume_device(&root);
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password-never-logged",
        Some(AcademicStage::Graduate),
        &device,
        false,
    )
    .expect("private credential persists");

    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-user"}}"#),
    ]);
    let mut runtime = runtime(&identity);
    runtime.persistence_root = root.clone();
    runtime.fingerprint = device;
    runtime.remember_credentials = true;
    runtime.credentials_persisted = true;
    runtime.credential_username = Some(account.to_owned());

    // Model the end of a target-login boundary without placing the password
    // in Runtime state. The actual portal proof below must release the guard;
    // a later independent expiry may then load the same encrypted record once
    // more, while the test never prints or sends the password.
    assert!(
        runtime
            .load_private_credential_for_boundary(account)
            .is_some()
    );
    assert!(runtime.private_credential_attempted);

    let adapter = info(&runtime, &portal);
    runtime
        .ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
        .await
        .expect("current portal proof completes the handoff boundary");
    assert!(runtime.portal_bootstrapped);
    assert!(
        !runtime.private_credential_attempted,
        "a proven portal must release only the completed portal credential boundary"
    );

    assert!(
        runtime
            .load_private_credential_for_boundary(account)
            .is_some()
    );
    assert!(runtime.private_credential_attempted);
    assert!(identity.requests().is_empty());
    assert_eq!(portal.requests().len(), 2);

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_resume_recovery_failure_keeps_locator_for_later_login() {
    let root = persistence_root("recovery-failure-keeps-locator");
    let account = "fixture-student";
    let device = current_resume_device(&root);
    crate::session_persistence::save_account_metadata_at_root(
        &root,
        &crate::session_persistence::ResumeAccountMetadata::new(
            account,
            Some(AcademicStage::Graduate),
            Some(&device),
            true,
        )
        .expect("recovery metadata"),
    )
    .expect("recovery metadata persists");
    crate::credential_store::save_at_root_with_stage_selection(
        &root,
        account,
        "fixture-password-never-logged",
        Some(AcademicStage::Graduate),
        &device,
        false,
    )
    .expect("private credential persists");

    // The failure occurs before a primary login POST. It represents the
    // common case where a process starts offline or the WebVPN bootstrap is
    // temporarily unavailable. Startup must return a usable Rust status, not
    // fail the Flutter Runtime provider, and it must retain the two durable
    // recovery locators for a later explicit login/retry.
    let webvpn = FixtureServer::new(vec![unavailable()]);
    let oauth = FixtureServer::new(vec![]);
    let identity = FixtureServer::new(vec![]);
    let config = WebVpnIdentityConfig::new(webvpn.base(), oauth.base(), identity.base())
        .expect("loopback identity graph");
    let mut runtime = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        true,
        root.clone(),
    )
    .expect("persistent recovery runtime");
    inject_webvpn_identity_config(&mut runtime, config);

    let status = runtime
        .resume_restored_session()
        .await
        .expect("classified startup recovery failure returns a status");
    assert_eq!(status.state, "expired");
    assert_eq!(status.username.as_deref(), Some(account));
    assert!(status.error.is_some());
    assert_eq!(runtime.resume_attempt, ResumeAttemptState::Consumed);
    assert!(runtime.credential_recovery_attempted);
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    assert_eq!(webvpn.requests().len(), 1);
    assert!(oauth.requests().is_empty());
    assert!(identity.requests().is_empty());

    // The recovery attempt is allowed to consume this Runtime's one-shot
    // network boundary, but a transient failure must not erase the account
    // metadata or the encrypted credential record from disk.
    assert_eq!(
        crate::session_persistence::load_account_metadata_at_root(&root)
            .expect("metadata remains readable")
            .as_ref()
            .map(|metadata| metadata.username.as_str()),
        Some(account)
    );
    assert!(
        crate::credential_store::load_at_root(&root, account, &device)
            .expect("credential record remains readable")
            .is_some()
    );

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_reuse_fresh_portal_cookies_skip_password_and_mfa_entirely() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let portal = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"result":"success","object":{"ryh":"fixture-user"}}"#),
    ]);
    let mut r = runtime(&identity);
    let adapter = info(&r, &portal);
    r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
        .await
        .unwrap();
    assert!(r.portal_bootstrapped);
    assert!(r.portal_csrf.is_some());
    assert!(r.service_second_factor.is_none());
    assert!(identity.requests().is_empty());
    assert_eq!(portal.requests().len(), 2);
    r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
        .await
        .unwrap();
    assert_eq!(
        portal.requests().len(),
        2,
        "a proven portal is reused with zero additional requests"
    );
    assert!(r.service_session_is_proven(ServiceId::Identity));
}

#[tokio::test]
async fn backend_repair_restored_cookie_portal_proof_promotes_identity_without_primary_login() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let portal = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-user"}}"#),
    ]);
    let mut r = runtime(&identity);
    let adapter = info(&r, &portal);

    // Model the constructor's restored-cookie boundary.  The Identity
    // registry still contains the saved account shell, but the current
    // Runtime must not call it live until the portal has proved account and
    // CSRF continuity.
    r.restored_session = true;
    r.primary_password = None;
    assert!(!r.service_session_is_proven(ServiceId::Identity));

    r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
        .await
        .expect("the carried portal cookies should prove the account");

    assert!(!r.restored_session);
    assert!(r.service_session_is_proven(ServiceId::Identity));
    r.ensure_identity_proven_for_read()
        .await
        .expect("a proved restored Identity should be reusable by readers");
    assert!(identity.requests().is_empty());
    assert_eq!(portal.requests().len(), 2);
    assert!(r.primary_password.is_none());
}

#[tokio::test]
async fn backend_repair_expired_identity_cookie_first_skips_private_vault() {
    let root = persistence_root("expired-identity-cookie-first");
    // A file at the vault directory path makes private-vault access fail
    // before a credential record can be inspected. If the expiry path jumps
    // straight to password recovery, this test will observe the attempt.
    fs::write(root.join("credentials"), b"vault temporarily unavailable")
        .expect("obstructed private vault");

    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-user"}}"#),
    ]);
    let mut r = runtime(&identity);
    r.persistence_root = root.clone();
    r.fingerprint = current_resume_device(&root);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username.clone());
    r.portal_bootstrapped = true;
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .expect("identity expiry transition");
    assert!(!r.service_session_is_proven(ServiceId::Identity));

    let adapter = info(&r, &portal);
    let portal_config = r.webvpn_identity_config.clone();
    r.refresh_identity_after_expiry_with_adapter_and_config(&adapter, &portal_config)
        .await
        .expect("valid current cookies should revalidate the expired identity");

    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(!r.private_credential_attempted);
    assert!(!r.credential_recovery_attempted);
    assert_eq!(portal.requests().len(), 2);
    assert!(identity.requests().is_empty());
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_cookie_first_identity_refresh_reuses_portal_proof_for_dependent_service() {
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"fixture-user"}}"#),
    ]);
    let mut r = runtime(&identity);
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .expect("identity expiry transition");
    assert!(!r.service_session_is_proven(ServiceId::Identity));

    // This is the nested path used by an expired service reader.  The
    // Cookie-first Identity proof must be marked as belonging to this one
    // refresh chain so a dependent Learn/Registrar/INFO handoff can reuse it.
    r.service_refresh_depth = 1;
    let adapter = info(&r, &portal);
    let portal_config = r.webvpn_identity_config.clone();
    r.refresh_identity_after_expiry_with_adapter_and_config(&adapter, &portal_config)
        .await
        .expect("current portal cookies should revalidate Identity");

    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(
        r.current_recovery_portal_proof,
        CurrentRecoveryPortalProof::CookieRevalidated
    );
    assert_eq!(portal.requests().len(), 2);
    assert!(
        identity.requests().is_empty(),
        "Cookie-first recovery must not submit primary credentials"
    );

    // Re-entering the shared portal prerequisite from the dependent service
    // must consume the proof already obtained above.  A second /cookie or
    // account probe would duplicate the one-shot handoff in the same chain.
    r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
        .await
        .expect("dependent service can reuse the current portal proof");
    assert_eq!(
        portal.requests().len(),
        2,
        "shared portal proof must not be probed twice in one refresh chain"
    );
    assert!(r.private_credential_attempted == false);
    assert!(r.credential_recovery_attempted == false);
}

#[tokio::test]
async fn backend_repair_live_read_refreshes_cookie_snapshot_for_next_runtime() {
    let root = persistence_root("live-read-cookie-refresh");
    let account = user();
    let live = FixtureServer::new(vec![
        Reply::html(
            r#"<html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">12.34</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-09-14 09:00:00</span></html>"#,
        ),
        Reply {
            status: 200,
            headers: "Content-Type: text/html; charset=utf-8\r\nSet-Cookie: fixture-refresh=updated; Path=/\r\n".into(),
            body: r#"<html><table class="myTable"><tr><th>header</th></tr><tr><td>room</td><td>7</td><td>2026-09-14 10:11:12</td><td>payment</td><td>20.50</td><td>成功</td></tr><tr><td>footer</td></tr></table></html>"#.into(),
        },
    ]);
    let mut first = persistent_runtime(&root);
    first
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity begins");
    first
        .coordinator
        .mark_authenticated(ServiceId::Identity, account.clone(), None, None, None)
        .expect("identity proof");
    first
        .coordinator
        .begin_authentication(ServiceId::Info)
        .expect("info begins");
    first
        .coordinator
        .mark_authenticated(ServiceId::Info, account.clone(), None, None, None)
        .expect("info proof");
    first.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(live.base(), "/target").expect("info config"),
            first.identity.transport().clone(),
        )
        .expect("info adapter"),
    );
    first.info_roaming_url = Some(
        crate::info::OpaqueUrl::new(format!("{}target/home", live.base()))
            .expect("info roaming URL"),
    );

    first
        .establish_electricity_read_at(Url::parse(live.base()).expect("electricity URL"))
        .await
        .expect("live remainder proof");
    let result = first
        .load_electricity_payment_history_result()
        .await
        .expect("live history read");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert_eq!(live.requests().len(), 2);

    // The second runtime restores only the Rust-owned snapshot.  It does not
    // get an authenticated UI proof; the request below is a fixture transport
    // probe used solely to establish that the refreshed snapshot is carried by
    // the next Rust runtime without inspecting or printing its value.
    let second = persistent_runtime(&root);
    assert!(second.restored_session);
    let probe = FixtureServer::new(vec![Reply::html("ok")]);
    second
        .identity
        .transport()
        .get_text(&format!("{}probe", probe.base()))
        .await
        .expect("restored transport probe");
    let request = probe.requests().into_iter().next().expect("probe request");
    assert!(
        request
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("cookie:")),
        "the next runtime must send the refreshed Cookie snapshot"
    );

    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_transport_refreshes_cookie_snapshot_before_business_parse_succeeds() {
    let root = persistence_root("transport-cookie-refresh-before-parse");
    let account = user();
    let live = FixtureServer::new(vec![
        Reply::html(
            r#"<html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">12.34</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-09-14 09:00:00</span></html>"#,
        ),
        Reply {
            status: 200,
            headers: "Content-Type: text/html; charset=utf-8\r\nSet-Cookie: fixture-refresh=updated; Path=/\r\n".into(),
            // The response is deliberately not a confirmed history table.
            // The transport must persist its Set-Cookie before this parser
            // failure reaches the Runtime business boundary.
            body: "<html><body>unexpected history page</body></html>".into(),
        },
    ]);
    let mut first = CampusRuntime::new_auto_with_persistence_at(
        "auto".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        true,
        root.clone(),
    )
    .expect("persistent fixture runtime");
    first
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity begins");
    first
        .coordinator
        .mark_authenticated(ServiceId::Identity, account.clone(), None, None, None)
        .expect("identity proof");
    first
        .coordinator
        .begin_authentication(ServiceId::Info)
        .expect("info begins");
    first
        .coordinator
        .mark_authenticated(ServiceId::Info, account.clone(), None, None, None)
        .expect("info proof");
    first.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(live.base(), "/target").expect("info config"),
            first.identity.transport().clone(),
        )
        .expect("info adapter"),
    );
    first.info_roaming_url = Some(
        crate::info::OpaqueUrl::new(format!("{}target/home", live.base()))
            .expect("info roaming URL"),
    );

    // Keep the initial snapshot valid without placing a matching Cookie on
    // the live fixture origin. The second runtime can therefore prove that a
    // Cookie header exists only because the malformed response was persisted.
    let seed_url = Url::parse("https://seed.fixture.example/").expect("seed URL");
    first
        .identity
        .transport()
        .cookie_jar()
        .add_cookie_str("resume-seed=present; Path=/", &seed_url);
    first
        .persist_resume_state(&account)
        .expect("initial live identity snapshot");

    first
        .establish_electricity_read_at(Url::parse(live.base()).expect("electricity URL"))
        .await
        .expect("electricity proof");
    let error = first
        .load_electricity_payment_history_result()
        .await
        .expect_err("malformed business content must remain an error");
    assert!(error.contains("服务读取未确认"));
    assert_eq!(live.requests().len(), 2);

    // Reconstructing the Runtime reads only the Rust-owned encrypted
    // snapshot. The probe deliberately checks only the presence of a Cookie
    // header, never its value or any response body.
    let second = persistent_runtime(&root);
    assert!(second.restored_session);
    let probe = FixtureServer::new(vec![Reply::html("ok")]);
    second
        .identity
        .transport()
        .get_text(&format!("{}probe", probe.base()))
        .await
        .expect("restored transport probe");
    let request = probe.requests().into_iter().next().expect("probe request");
    assert!(
        request
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("cookie:")),
        "a response Cookie must be snapshotted before business parsing finishes"
    );
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_expired_identity_reads_private_vault_only_after_login_boundary() {
    let root = persistence_root("expired-identity-login-boundary");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![Reply {
        status: 401,
        headers: String::new(),
        body: "login required".into(),
    }]);
    let mut r = runtime(&identity);
    r.persistence_root = root.clone();
    r.fingerprint = current_resume_device(&root);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username.clone());
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .expect("identity expiry transition");

    let adapter = info(&r, &portal);
    let portal_config = r.webvpn_identity_config.clone();
    let error = r
        .refresh_identity_after_expiry_with_adapter_and_config(&adapter, &portal_config)
        .await
        .expect_err("missing private credential must leave the user signed out");

    assert_eq!(error, "统一认证会话已过期，请重新登录");
    assert!(r.private_credential_attempted);
    assert!(r.credential_recovery_attempted);
    assert!(!r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(portal.requests().len(), 1);
    assert!(identity.requests().is_empty());
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_expired_identity_ambiguous_portal_failure_does_not_read_private_vault() {
    let root = persistence_root("expired-identity-ambiguous");
    fs::write(root.join("credentials"), b"vault temporarily unavailable")
        .expect("obstructed private vault");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let mut r = runtime(&identity);
    r.persistence_root = root.clone();
    r.fingerprint = current_resume_device(&root);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username.clone());
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .expect("identity expiry transition");

    let adapter = info(&r, &portal);
    let portal_config = r.webvpn_identity_config.clone();
    let error = r
        .refresh_identity_after_expiry_with_adapter_and_config(&adapter, &portal_config)
        .await
        .expect_err("an ambiguous portal outage must remain a non-auth error");

    assert_eq!(error, "portal_resume_cookie_http");
    assert!(!r.private_credential_attempted);
    assert!(!r.credential_recovery_attempted);
    assert!(!r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(portal.requests().len(), 1);
    assert!(identity.requests().is_empty());
    cleanup_persistence_root(&root);
}

#[tokio::test]
async fn backend_repair_reuse_portal_wrong_account_stops_without_password_fallback() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let portal = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info;"),
        Reply::json(r#"{"object":{"ryh":"different-fixture-user"}}"#),
    ]);
    let mut r = runtime(&identity);
    let adapter = info(&r, &portal);
    assert_eq!(
        r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
            .await
            .unwrap_err(),
        "portal_resume_account_mismatch"
    );
    assert!(identity.requests().is_empty());
    assert_eq!(portal.requests().len(), 2);
    assert!(!r.portal_bootstrapped);
    assert!(r.service_session_is_proven(ServiceId::Identity));
}

#[tokio::test]
async fn backend_repair_reuse_portal_outage_is_not_permission_to_reauthenticate() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let mut r = runtime(&identity);
    let adapter = info(&r, &portal);
    assert_eq!(
        r.ensure_portal_bootstrap_inner_with_adapter(&user(), &adapter)
            .await
            .unwrap_err(),
        "portal_resume_cookie_http"
    );
    assert!(identity.requests().is_empty());
    assert_eq!(portal.requests().len(), 1);
    assert!(r.service_second_factor.is_none());
}

#[tokio::test]
async fn backend_repair_reuse_fresh_card_cookies_probe_first_and_skip_target_login() {
    let identity = FixtureServer::new(vec![unavailable()]);
    let service = FixtureServer::new(vec![Reply::json(
        r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#,
    )]);
    let mut r = runtime(&identity);
    card(&mut r, &service);
    r.ensure_card_session(&user()).await.unwrap();
    r.ensure_card_session(&user()).await.unwrap();
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert!(r.service_second_factor.is_none());
    assert_eq!(service.requests().len(), 1);
    assert!(identity.requests().is_empty());
    assert!(service.requests()[0].starts_with("POST /login/getUserInfoFromToken "));
}

#[tokio::test]
async fn backend_repair_reuse_card_outage_or_other_account_never_triggers_credentials() {
    for reply in [
        unavailable(),
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"other-fixture"}}"#),
    ] {
        let identity = FixtureServer::new(vec![unavailable()]);
        let service = FixtureServer::new(vec![reply]);
        let mut r = runtime(&identity);
        card(&mut r, &service);
        assert!(r.ensure_card_session(&user()).await.is_err());
        assert_eq!(service.requests().len(), 1);
        assert!(identity.requests().is_empty());
        assert!(!r.service_session_is_proven(ServiceId::CampusCard));
        assert!(r.service_session_is_proven(ServiceId::Identity));
    }
}

// Request-level lifecycle regressions migrated from the failing audit.
struct LifecycleAuditRoot(PathBuf);
impl LifecycleAuditRoot {
    fn new(label: &str) -> Self {
        Self(persistence_root(label))
    }
}
impl Drop for LifecycleAuditRoot {
    fn drop(&mut self) {
        cleanup_persistence_root(&self.0);
    }
}
fn lifecycle_audit_runtime(
    root: &Path,
    identity: &FixtureServer,
    portal: &FixtureServer,
    oauth: &FixtureServer,
) -> CampusRuntime {
    let mut r = runtime(identity);
    r.persistence_root = root.to_owned();
    r.cache_path = root.join("cache.json");
    r.primary_password = None;
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r
}
fn lifecycle_audit_metadata(root: &Path) {
    let device = current_resume_device(root);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        "fixture-user",
        Some(AcademicStage::Graduate),
        Some(&device),
        true,
    )
    .unwrap();
    crate::session_persistence::save_account_metadata_at_root(root, &metadata).unwrap();
    crate::credential_store::save_at_root_with_stage_selection(
        root,
        "fixture-user",
        "synthetic-audit-not-a-real-password",
        Some(AcademicStage::Graduate),
        &device,
        false,
    )
    .unwrap();
}
fn lifecycle_audit_obstruct_locks(root: &Path) {
    for path in [
        root.join("credentials/vault.lock"),
        root.join("campus-session-store-v1.lock"),
    ] {
        if path.is_file() {
            fs::remove_file(&path).unwrap();
        }
        fs::create_dir(path).unwrap();
    }
}

#[test]
fn backend_repair_lifecycle_logout_must_report_failed_durable_revocation() {
    let root = LifecycleAuditRoot::new("audit-logout-error");
    lifecycle_audit_metadata(&root.0);
    let mut r = persistent_runtime(&root.0);
    lifecycle_audit_obstruct_locks(&root.0);
    let status = r.logout().expect("local memory logout remains available");
    // The public DTO spells the registry's Anonymous state `signed_out`.
    assert_eq!(status.state, "signed_out");
    assert!(root.0.join("campus-account-v1.json").exists());
    println!(
        "AUDIT memory_signed_out=true durable_metadata_survives=true warning_present={}",
        status.error.is_some()
    );
    assert!(
        status.error.is_some(),
        "logout must not claim unqualified success when credential and session deletion failed"
    );
}

#[test]
fn backend_repair_lifecycle_failed_logout_must_not_reenable_saved_recovery_on_restart() {
    let root = LifecycleAuditRoot::new("audit-logout-restart");
    lifecycle_audit_metadata(&root.0);
    let mut r = persistent_runtime(&root.0);
    lifecycle_audit_obstruct_locks(&root.0);
    r.logout().unwrap();
    fs::remove_dir(root.0.join("credentials/vault.lock")).unwrap();
    fs::remove_dir(root.0.join("campus-session-store-v1.lock")).unwrap();
    let next = persistent_runtime(&root.0);
    println!(
        "AUDIT after_restart_automatic_recovery_enabled={}",
        next.status().automatic_recovery_enabled
    );
    assert!(
        !next.status().automatic_recovery_enabled,
        "an explicit logout must durably suppress saved-login recovery even after a transient cleanup failure"
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_info_read_must_consult_failed_gate_after_invalidation() {
    let root = LifecycleAuditRoot::new("audit-info-gate");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    r.automatic_refresh_attempted.insert(ServiceId::Info);
    r.automatic_refresh_results
        .insert(ServiceId::Info, Err("audit-consumed-boundary".into()));
    r.invalidate_service_session(ServiceId::Info);
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    println!(
        "AUDIT info_read_requests_after_consumed_gate={}",
        portal.requests().len()
    );
    assert_eq!(
        portal.requests().len(),
        0,
        "clearing the expired adapter must not make the next read look like first-ever authentication"
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_library_read_must_consult_failed_gate_after_invalidation() {
    let root = LifecycleAuditRoot::new("audit-library-gate");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable()]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    r.automatic_refresh_attempted.insert(ServiceId::Library);
    r.automatic_refresh_results
        .insert(ServiceId::Library, Err("audit-consumed-boundary".into()));
    r.invalidate_service_session(ServiceId::Library);
    assert!(r.load_library_area_tree_result().await.is_err());
    println!(
        "AUDIT library_read_requests_after_consumed_gate={}",
        portal.requests().len()
    );
    assert_eq!(
        portal.requests().len(),
        0,
        "library cache readers must retain the consumed auth boundary after adapter invalidation"
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_live_portal_password_boundary_must_reach_opt_in_recovery() {
    let root = LifecycleAuditRoot::new("audit-live-password-boundary");
    let password_form = format!(
        r#"<div id="sm2publicKey">{}</div><form method="post" action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#,
        recovery_public_key()
    );
    let identity = FixtureServer::new(vec![Reply::html(&password_form)]);
    let portal = FixtureServer::new(vec![Reply {
        status: 401,
        headers: String::new(),
        body: String::new(),
    }]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_audit_metadata(&root.0);
    // Match the credential's persisted stage; the mismatched-stage case has
    // its own negative contract below.
    r.stage = AcademicStage::Graduate;
    r.stage_detected = true;
    r.fingerprint = current_resume_device(&root.0);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username);
    r.coordinator.begin_authentication(ServiceId::Info).unwrap();
    r.coordinator
        .mark_authenticated(
            ServiceId::Info,
            user(),
            None,
            None,
            Some(Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    assert!(
        r.portal_password_required,
        "fixture must reach an explicit current password boundary"
    );
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.primary_password.is_none());
    println!(
        "AUDIT local_identity_authenticated=true explicit_password_boundary=true private_recovery_attempted={}",
        r.private_credential_attempted || r.credential_recovery_attempted
    );
    assert!(
        r.private_credential_attempted || r.credential_recovery_attempted,
        "a post-login service read must reach opted-in credential recovery, not only startup/explicitly-expired Identity paths"
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_one_read_must_not_duplicate_failed_identity_probe() {
    let root = LifecycleAuditRoot::new("audit-identity-double-probe");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable(), unavailable(), unavailable()]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username);
    r.coordinator
        .registry_mut()
        .transition(
            ServiceId::Identity,
            crate::session::SessionTransition::Expire,
        )
        .unwrap();
    assert!(r.ensure_identity_user_for_live_read().await.is_err());
    assert!(identity.requests().is_empty());
    assert!(!r.private_credential_attempted);
    println!(
        "AUDIT failed_identity_probes_for_one_read={}",
        portal.requests().len()
    );
    assert_eq!(
        portal.requests().len(),
        1,
        "cache recovery and final identity guard must share one failed proof attempt"
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_other_runtime_must_not_recreate_snapshot_after_logout() {
    let root = LifecycleAuditRoot::new("audit-other-runtime-logout");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![Reply::html("synthetic read")]);
    let oauth = FixtureServer::new(vec![]);
    let mut first = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    first.persist_sessions = true;
    first.fingerprint = current_resume_device(&root.0);
    first.persist_resume_state(&user()).unwrap();
    let mut other = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    other.persist_sessions = true;
    other.fingerprint = first.fingerprint.clone();
    other.persist_resume_state(&user()).unwrap();
    first.logout().unwrap();
    assert!(!root.0.join("campus-session-v2.bin").exists());
    let transport = other.identity.transport();
    transport
        .send(transport.client().get(portal.base()))
        .await
        .unwrap();
    println!(
        "AUDIT old_runtime_recreated_cookie_snapshot={}",
        root.0.join("campus-session-v2.bin").exists()
    );
    assert!(
        !root.0.join("campus-session-v2.bin").exists(),
        "a stale runtime needs a durable revocation epoch check before response-level snapshot writes"
    );
}

fn lifecycle_cookie() -> Reply {
    Reply::html("XSRF-TOKEN=fixture-csrf;")
}
fn lifecycle_account() -> Reply {
    Reply::json(r#"{"object":{"ryh":"fixture-user"}}"#)
}
fn lifecycle_news() -> Reply {
    Reply::json(r#"{"object":{"dataList":[]}}"#)
}
fn lifecycle_unauthorized() -> Reply {
    Reply {
        status: 401,
        headers: String::new(),
        body: String::new(),
    }
}
fn lifecycle_install_info(r: &mut CampusRuntime) {
    r.coordinator.begin_authentication(ServiceId::Info).unwrap();
    let csrf = r.coordinator.registry().bind_csrf(
        ServiceId::Info,
        crate::protocol::CsrfToken::new("fixture-csrf").unwrap(),
    );
    r.coordinator
        .mark_authenticated(ServiceId::Info, user(), None, Some(csrf), None)
        .unwrap();
    let config = r.portal_info_config().unwrap();
    r.info_roaming_url = Some(
        crate::info::OpaqueUrl::new(format!(
            "{}{}f/info/gxfw_fg/common/index",
            r.webvpn_identity_config
                .webvpn_origin()
                .as_str()
                .trim_end_matches('/'),
            INFO_TARGET_PREFIX
        ))
        .unwrap(),
    );
    r.info_adapter = Some(InfoSessionAdapter::new(config, r.identity.transport().clone()).unwrap());
    r.portal_bootstrapped = true;
}

#[tokio::test]
async fn backend_repair_lifecycle_two_server_expiries_restore_business_reads_without_password() {
    let root = LifecycleAuditRoot::new("two-business-expiries");
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut replies = Vec::new();
    for _ in 0..2 {
        replies.extend([
            lifecycle_cookie(),
            lifecycle_unauthorized(),
            lifecycle_cookie(),
            lifecycle_account(),
            lifecycle_news(),
            lifecycle_cookie(),
            lifecycle_news(),
        ]);
    }
    let portal = FixtureServer::new(replies);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_install_info(&mut r);
    for page in 1..=2 {
        let result = r
            .load_info_news(page, 10, None, None)
            .await
            .expect("business read resumes after server expiry");
        assert_eq!(result.source.as_deref(), Some("live"));
        assert!(r.service_session_is_proven(ServiceId::Info));
        assert!(!r.service_recovery_context_exists(ServiceId::Info));
        assert_eq!(portal.requests().len(), page as usize * 7);
    }
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
    assert!(!r.private_credential_attempted);
}

#[tokio::test]
async fn backend_repair_lifecycle_second_server_rejection_seals_recovery_without_loop() {
    let root = LifecycleAuditRoot::new("second-rejection");
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![
        lifecycle_cookie(),
        lifecycle_unauthorized(),
        lifecycle_cookie(),
        lifecycle_account(),
        lifecycle_news(),
        lifecycle_cookie(),
        lifecycle_unauthorized(),
    ]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_install_info(&mut r);
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    assert!(!r.service_session_is_proven(ServiceId::Info));
    assert_eq!(portal.requests().len(), 7);
    assert!(r.load_info_news(2, 10, None, None).await.is_err());
    assert_eq!(
        portal.requests().len(),
        7,
        "a second rejection cannot become another expiry cycle"
    );
    assert!(r.service_session_is_proven(ServiceId::Identity));
}

#[tokio::test]
async fn backend_repair_lifecycle_password_fallback_reaches_handoff_and_original_business_read() {
    let root = LifecycleAuditRoot::new("password-through-business");
    let portal = FixtureServer::new(vec![
        lifecycle_cookie(),
        lifecycle_unauthorized(),
        lifecycle_unauthorized(),
        Reply::html("<html>Existing authenticated portal</html>"),
        lifecycle_cookie(),
        lifecycle_account(),
        lifecycle_news(),
        lifecycle_cookie(),
        lifecycle_news(),
    ]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}{}f/info/gxfw_fg/common/index\r\n",
            portal.base().trim_end_matches('/'),
            INFO_TARGET_PREFIX
        ),
        body: String::new(),
    }]);
    let form = format!(
        r#"<div id="sm2publicKey">{}</div><form method="post" action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#,
        recovery_public_key()
    );
    let identity = FixtureServer::new(vec![
        Reply::html(&form),
        Reply::html(&form),
        Reply {
            status: 302,
            headers: format!(
                "Location: {}f/info/gxfw_fg/common/index\r\n",
                INFO_DIRECT_ORIGIN
            ),
            body: String::new(),
        },
    ]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_audit_metadata(&root.0);
    r.stage = AcademicStage::Graduate;
    r.stage_detected = true;
    r.fingerprint = current_resume_device(&root.0);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username);
    lifecycle_install_info(&mut r);
    let result = r.load_info_news(1, 10, None, None).await;
    if result.is_err() {
        let reason = r
            .automatic_refresh_results
            .get(&ServiceId::Info)
            .and_then(|result| result.as_ref().err())
            .map(|error| crate::telemetry::diagnostic_reason(error))
            .unwrap_or("other");
        println!(
            "LIFECYCLE_TRACE identity_requests={} identity_posts={} portal_requests={} oauth_requests={} reason={}",
            identity.requests().len(),
            identity
                .requests()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            portal.requests().len(),
            oauth.requests().len(),
            reason
        );
    }
    let result = result.expect("saved credential must restore original read");
    assert_eq!(result.source.as_deref(), Some("live"));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(r.primary_password.is_none());
    assert!(!r.service_recovery_context_exists(ServiceId::Info));
    let requests = identity.requests();
    assert_eq!(
        requests.iter().filter(|r| r.starts_with("POST ")).count(),
        1
    );
    assert!(
        requests
            .iter()
            .all(|r| !r.contains("synthetic-audit-not-a-real-password"))
    );
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(portal.requests().len(), 9);
}

#[tokio::test]
async fn backend_repair_lifecycle_no_saved_credential_stops_online_state_after_password_boundary() {
    let root = LifecycleAuditRoot::new("no-opt-in");
    let form = format!(
        r#"<div id="sm2publicKey">{}</div><form method="post" action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#,
        recovery_public_key()
    );
    let identity = FixtureServer::new(vec![Reply::html(&form)]);
    let portal = FixtureServer::new(vec![lifecycle_unauthorized()]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    assert_eq!(r.status().state, "expired");
    assert!(r.recovery.login_required);
    assert!(!r.service_session_is_proven(ServiceId::Identity));
    assert!(!r.private_credential_attempted);
    assert!(r.load_info_news(2, 10, None, None).await.is_err());
    assert_eq!(identity.requests().len(), 1);
    assert_eq!(portal.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_lifecycle_transient_probe_cooldown_reopens_only_after_deadline() {
    let root = LifecycleAuditRoot::new("probe-cooldown");
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![
        unavailable(),
        lifecycle_cookie(),
        lifecycle_account(),
        lifecycle_news(),
        lifecycle_cookie(),
        lifecycle_news(),
    ]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    assert!(r.recovery.retry_at.contains_key(&ServiceId::Info));
    assert!(r.load_info_news(2, 10, None, None).await.is_err());
    assert_eq!(portal.requests().len(), 1);
    r.recovery.retry_at.insert(
        ServiceId::Info,
        std::time::Instant::now() - Duration::from_secs(1),
    );
    let recovered = r
        .load_info_news(3, 10, None, None)
        .await
        .expect("new proof after cooldown");
    assert_eq!(recovered.source.as_deref(), Some("live"));
    assert_eq!(portal.requests().len(), 6);
    assert!(identity.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_lifecycle_auth_post_outcome_never_gets_automatic_probe_retry() {
    let root = LifecycleAuditRoot::new("ambiguous-auth-post");
    let identity = FixtureServer::new(vec![unavailable()]);
    let portal = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    let fence = r.identity.transport().replay_fence();
    let transport = r.identity.transport();
    transport
        .send(
            transport
                .client()
                .post(identity.base())
                .body("synthetic-noncredential"),
        )
        .await
        .unwrap();
    r.record_recovery_result(
        ServiceId::Info,
        &Err("portal_resume_network".to_owned()),
        fence,
    );
    assert!(!r.recovery.retry_at.contains_key(&ServiceId::Info));
    assert_eq!(
        r.recovery.failure_kinds[&ServiceId::Info],
        lifecycle::RecoveryFailureKind::Unconfirmed
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_startup_offline_cookie_probe_can_recover_after_cooldown() {
    let root = LifecycleAuditRoot::new("startup-probe-cooldown");
    let device = current_resume_device(&root.0);
    let cookies = CampusCookieStore::default();
    cookies.add_cookie_str(
        "seed=synthetic; Path=/",
        &Url::parse("https://fixture.example/").unwrap(),
    );
    let snapshot = ResumeSnapshot::new("fixture-user", &cookies.snapshot_bytes().unwrap())
        .unwrap()
        .with_device_fingerprint(&device)
        .unwrap();
    crate::session_persistence::save_at_root(&root.0, &snapshot).unwrap();
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![unavailable(), lifecycle_cookie(), lifecycle_account()]);
    let mut r = persistent_runtime(&root.0);
    inject_webvpn_identity_config(
        &mut r,
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap(),
    );
    r.resume_restored_session().await.unwrap();
    assert!(!r.service_session_is_proven(ServiceId::Identity));
    r.resume_restored_session().await.unwrap();
    assert_eq!(portal.requests().len(), 1);
    r.recovery.resume_retry_at = Some(std::time::Instant::now() - Duration::from_secs(1));
    r.resume_restored_session().await.unwrap();
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(portal.requests().len(), 3);
    assert!(identity.requests().is_empty());
}

#[test]
fn backend_repair_lifecycle_stale_logout_does_not_revoke_a_new_explicit_login() {
    let root = LifecycleAuditRoot::new("stale-logout-new-login");
    let old = crate::session_persistence::SessionLease::current(&root.0)
        .unwrap()
        .unwrap();
    let new = crate::session_persistence::begin_explicit_authority(&root.0, "fixture-new").unwrap();
    crate::session_persistence::revoke_authority(&root.0, Some(&old), Some("fixture-old")).unwrap();
    assert!(new.is_current());
    assert!(!old.is_current());
}

#[tokio::test]
async fn backend_repair_lifecycle_revoked_runtime_refuses_cached_and_live_reads() {
    let root = LifecycleAuditRoot::new("revoked-reader");
    lifecycle_audit_metadata(&root.0);
    let mut r = persistent_runtime(&root.0);
    let lease = r.recovery_lease.clone().unwrap();
    crate::session_persistence::revoke_authority(&root.0, Some(&lease), Some("fixture-user"))
        .unwrap();
    assert_eq!(r.status().state, "signed_out");
    assert!(r.cache_user_for_read().is_err());
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
}

#[test]
fn backend_repair_lifecycle_completed_factor_releases_only_interaction_failures() {
    let root = LifecycleAuditRoot::new("completed-factor-gates");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    for (service, kind) in [
        (
            ServiceId::Info,
            lifecycle::RecoveryFailureKind::InteractionRequired,
        ),
        (
            ServiceId::CampusCard,
            lifecycle::RecoveryFailureKind::Unconfirmed,
        ),
    ] {
        r.automatic_refresh_attempted.insert(service);
        r.automatic_refresh_results
            .insert(service, Err("synthetic-boundary".into()));
        r.recovery.failure_kinds.insert(service, kind);
    }
    r.release_completed_interaction();
    assert!(!r.service_recovery_context_exists(ServiceId::Info));
    assert!(r.service_recovery_context_exists(ServiceId::CampusCard));
}

fn lifecycle_enable_saved(r: &mut CampusRuntime, root: &Path) {
    lifecycle_audit_metadata(root);
    r.stage = AcademicStage::Graduate;
    r.stage_detected = true;
    r.fingerprint = current_resume_device(root);
    r.remember_credentials = true;
    r.credentials_persisted = true;
    r.credential_username = Some(user().username);
}
fn lifecycle_password_form() -> Reply {
    Reply::html(&format!(
        r#"<span id="sm2publicKey">{}</span><form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#,
        recovery_public_key()
    ))
}

#[tokio::test]
async fn backend_repair_lifecycle_card_saved_password_recovers_two_independent_read_cycles() {
    let root = LifecycleAuditRoot::new("card-two-password-cycles");
    let accepted = || Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#);
    let denied = || Reply::json(r#"{"success":false,"resultData":null}"#);
    let account = || {
        Reply::json(
            r#"{"success":true,"resultData":{"idserial":"fixture-user","username":"Fixture","departname":"Fixture","departid":1,"identifyeffectdate":"2026-09-18","validatevalue":"2027-09-18","baseAccount":{"balance":100},"cardInfos":[{"cardid":"fixture-card","accstatus":"0","lasttxdate":"","maxconstolamt":20000,"maxconsamt":5000}]}}"#,
        )
    };
    let service = FixtureServer::new(vec![
        denied(),
        Reply::html("callback"),
        accepted(),
        account(),
        lifecycle_unauthorized(),
        denied(),
        Reply::html("callback"),
        accepted(),
        account(),
    ]);
    let callback = || Reply {
        status: 302,
        headers: format!("Location: {}handoff\r\n", service.base()),
        body: String::new(),
    };
    let identity = FixtureServer::new(vec![
        lifecycle_password_form(),
        callback(),
        lifecycle_password_form(),
        callback(),
    ]);
    let portal = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_enable_saved(&mut r, &root.0);
    card(&mut r, &service);
    for cycle in 0..2 {
        if cycle > 0 {
            fs::remove_file(campus_card_account_cache_path(
                &r.cache_path,
                &user().username,
            ))
            .unwrap();
        }
        let result = r
            .load_campus_card_account_result()
            .await
            .expect("card original read recovers");
        assert_eq!(result.source, "live");
        assert!(r.service_session_is_proven(ServiceId::CampusCard));
        assert!(r.primary_password.is_none());
        assert!(
            !r.recovery
                .target_credentials
                .contains(&ServiceSecondFactorTarget::CampusCard)
        );
    }
    assert_eq!(
        identity
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        2
    );
    assert_eq!(service.requests().len(), 9);
    assert!(portal.requests().is_empty());
    assert!(oauth.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_lifecycle_electricity_saved_password_reaches_original_balance_read() {
    let root = LifecycleAuditRoot::new("electricity-saved-boundary");
    let balance = "<span id='Netweb_Home_electricity_DetailCtrl1_lblele'>12.50</span><span id='Netweb_Home_electricity_DetailCtrl1_lbltime'>2026-09-18 12:00:00</span>";
    let portal = FixtureServer::new(vec![
        lifecycle_unauthorized(),
        Reply::html("callback"),
        Reply::html(balance),
    ]);
    let mapped_path = Url::parse(ELECTRICITY_WEBVPN_BASE_URL)
        .unwrap()
        .path()
        .to_owned();
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}{}default.aspx\r\n",
            portal.base().trim_end_matches('/'),
            mapped_path
        ),
        body: String::new(),
    }]);
    let identity = FixtureServer::new(vec![
        lifecycle_password_form(),
        Reply {
            status: 302,
            headers: "Location: http://myhome.tsinghua.edu.cn/default.aspx?ticket=synthetic\r\n"
                .into(),
            body: String::new(),
        },
    ]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_enable_saved(&mut r, &root.0);
    lifecycle_install_info(&mut r);
    let result = r
        .load_electricity_remainder_result()
        .await
        .expect("electricity original balance read recovers");
    assert_eq!(result.source, "live");
    assert!(r.electricity_service_is_proven());
    assert!(r.primary_password.is_none());
    assert!(
        !r.recovery
            .target_handoffs
            .contains(&ServiceSecondFactorTarget::Electricity)
    );
    assert_eq!(
        identity
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
    assert_eq!(portal.requests().len(), 3);
    assert_eq!(oauth.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_lifecycle_portal_rejected_password_revokes_opt_in_and_stops_recovery() {
    let root = LifecycleAuditRoot::new("rejected-saved-password");
    let identity = FixtureServer::new(vec![
        lifecycle_password_form(),
        lifecycle_password_form(),
        Reply::html(
            "<form><input name='i_user'><input name='i_pass' type='password'><div id='loginError'>用户名或密码错误</div></form>",
        ),
    ]);
    let portal = FixtureServer::new(vec![lifecycle_unauthorized()]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_enable_saved(&mut r, &root.0);
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    assert_eq!(r.status().state, "expired");
    assert!(!r.status().automatic_recovery_enabled);
    assert!(
        crate::credential_store::load_at_root(&root.0, "fixture-user", &r.fingerprint)
            .unwrap()
            .is_none()
    );
    assert!(r.load_info_news(2, 10, None, None).await.is_err());
    assert_eq!(
        identity
            .requests()
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
    assert_eq!(portal.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_lifecycle_stage_mismatch_never_submits_saved_password() {
    let root = LifecycleAuditRoot::new("stage-mismatch-target");
    let identity = FixtureServer::new(vec![lifecycle_password_form()]);
    let portal = FixtureServer::new(vec![lifecycle_unauthorized()]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_enable_saved(&mut r, &root.0);
    r.stage = AcademicStage::Undergraduate;
    assert!(r.load_info_news(1, 10, None, None).await.is_err());
    assert!(
        identity
            .requests()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
    assert_eq!(r.status().state, "expired");
}

#[test]
fn backend_repair_lifecycle_opt_out_revokes_recovery_locator_before_login_proof() {
    let root = LifecycleAuditRoot::new("opt-out-before-login");
    lifecycle_audit_metadata(&root.0);
    let lease = crate::session_persistence::begin_explicit_authority_with_opt_in(
        &root.0,
        "fixture-user",
        false,
    )
    .unwrap();
    assert!(lease.is_current());
    let restarted = persistent_runtime(&root.0);
    assert!(!restarted.status().automatic_recovery_enabled);
    assert!(!restarted.remember_credentials);
}

#[test]
fn backend_repair_lifecycle_repeated_logout_without_lease_cannot_revoke_new_window() {
    let root = LifecycleAuditRoot::new("repeat-logout-new-window");
    lifecycle_audit_metadata(&root.0);
    let mut old = persistent_runtime(&root.0);
    old.logout().unwrap();
    let new = crate::session_persistence::begin_explicit_authority(&root.0, "fixture-new").unwrap();
    old.logout().unwrap();
    assert!(new.is_current());
}

#[test]
fn backend_repair_lifecycle_busy_session_lock_cannot_block_logout_or_revocation() {
    use fs2::FileExt;
    let root = LifecycleAuditRoot::new("busy-session-lock");
    lifecycle_audit_metadata(&root.0);
    let mut r = persistent_runtime(&root.0);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.0.join("campus-session-store-v1.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let status = r.logout().unwrap();
    assert_eq!(status.state, "signed_out");
    assert!(status.error.is_some());
    assert!(
        crate::session_persistence::SessionLease::current(&root.0)
            .unwrap()
            .is_none()
    );
    lock.unlock().unwrap();
    assert!(
        !persistent_runtime(&root.0)
            .status()
            .automatic_recovery_enabled
    );
}

#[cfg(unix)]
#[test]
fn backend_repair_lifecycle_authority_symlink_and_corruption_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = LifecycleAuditRoot::new("authority-invalid");
    let path = root.0.join("campus-auth-authority-v1.json");
    let unrelated = root.0.join("unrelated-synthetic-file");
    fs::write(&unrelated, b"synthetic untouched").unwrap();
    symlink(&unrelated, &path).unwrap();
    assert!(crate::session_persistence::SessionLease::current(&root.0).is_err());
    assert_eq!(fs::read(&unrelated).unwrap(), b"synthetic untouched");
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(crate::session_persistence::load_authorized_state(&root.0).is_err());
}

#[tokio::test]
async fn backend_repair_lifecycle_electricity_prefetch_keeps_proof_for_following_live_read() {
    let root = LifecycleAuditRoot::new("electricity-prefetch-proof");
    let balance = "<span id='Netweb_Home_electricity_DetailCtrl1_lblele'>12.50</span><span id='Netweb_Home_electricity_DetailCtrl1_lbltime'>2026-09-18 12:00:00</span>";
    let portal = FixtureServer::new(vec![Reply::html(balance), Reply::html(balance)]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    lifecycle_install_info(&mut r);
    for cycle in 0..2 {
        if cycle > 0 {
            fs::remove_file(electricity_remainder_cache_path(
                &r.cache_path,
                &user().username,
            ))
            .unwrap();
        }
        let result = r.load_electricity_remainder_result().await.unwrap();
        assert_eq!(result.source, "live");
        assert!(r.electricity_service_is_proven());
    }
    assert_eq!(portal.requests().len(), 2);
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
}

#[test]
fn backend_repair_lifecycle_terminal_login_reason_survives_cleared_business_error() {
    let root = LifecycleAuditRoot::new("terminal-reason");
    let identity = FixtureServer::new(vec![]);
    let portal = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    r.require_identity_login("统一认证凭证已失效，请重新登录");
    r.last_error = None; // A successful stale-cache presentation is not recovery.
    assert_eq!(r.status().state, "expired");
    assert_eq!(
        r.status().error.as_deref(),
        Some("统一认证凭证已失效，请重新登录")
    );
}

#[tokio::test]
async fn backend_repair_lifecycle_nested_service_chain_shares_fresh_portal_account_proof() {
    let root = LifecycleAuditRoot::new("nested-portal-proof");
    let portal = FixtureServer::new(vec![
        lifecycle_cookie(),
        lifecycle_account(),
        lifecycle_news(),
    ]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
    // Model the INFO dependency of an already-entered Library recovery. The
    // dependency must publish its fresh proof for the remaining root chain.
    r.service_refresh_depth = 1;
    r.refresh_nonacademic_service_after_expiry(ServiceId::Info)
        .await
        .unwrap();
    assert_eq!(r.service_refresh_depth, 1);
    assert_eq!(
        r.current_recovery_portal_proof,
        CurrentRecoveryPortalProof::CookieRevalidated
    );
    r.ensure_portal_bootstrap(&user()).await.unwrap();
    assert_eq!(portal.requests().len(), 3);
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_lifecycle_unconfirmed_target_pages_never_open_saved_credentials() {
    for target in [
        ServiceSecondFactorTarget::CampusCard,
        ServiceSecondFactorTarget::Electricity,
    ] {
        let root = LifecycleAuditRoot::new("unconfirmed-target-form");
        // A public key alone is not proof that this response requests a
        // password. Do not decrypt a saved record or POST from a generic page.
        let identity = FixtureServer::new(vec![Reply::html(&format!(
            "<span id='sm2publicKey'>{}</span>",
            recovery_public_key()
        ))]);
        let portal = FixtureServer::new(vec![lifecycle_unauthorized()]);
        let oauth = FixtureServer::new(vec![]);
        let card_service =
            FixtureServer::new(vec![Reply::json(r#"{"success":false,"resultData":null}"#)]);
        let mut r = lifecycle_audit_runtime(&root.0, &identity, &portal, &oauth);
        lifecycle_enable_saved(&mut r, &root.0);
        if target == ServiceSecondFactorTarget::CampusCard {
            card(&mut r, &card_service);
            assert!(r.load_campus_card_account_result().await.is_err());
        } else {
            lifecycle_install_info(&mut r);
            assert!(r.load_electricity_remainder_result().await.is_err());
        }
        assert!(!r.recovery.target_credentials.contains(&target));
        assert_eq!(identity.requests().len(), 1);
        assert!(
            identity
                .requests()
                .iter()
                .all(|request| request.starts_with("GET "))
        );
        assert!(oauth.requests().is_empty());
        assert!(r.service_session_is_proven(ServiceId::Identity));
    }
}
