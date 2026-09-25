//! Account, scope, and no-replay fixtures. All identities below are synthetic.
use super::*;
use crate::protocol::{CsrfToken, SecondFactorChallenge};
use crate::reference_test_support::{FixtureServer, Reply};
use crate::tunet_client::TunetClientConfig;

#[path = "runtime_captcha_tests.rs"]
mod captcha_tests;

#[test]
fn backend_repair_deep_api_status_does_not_report_expired_identity_as_authenticated() {
    let mut r = runtime();
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(
            ServiceId::Identity,
            username("primary-a"),
            None,
            None,
            Some(Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    assert!(!r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(r.status().state, "expired");
    assert!(!r.status().overview_available);
    assert_eq!(
        r.service_catalog()
            .services
            .iter()
            .find(|e| e.id == "identity")
            .unwrap()
            .availability,
        "requires_authentication"
    );
}

#[tokio::test]
async fn backend_repair_deep_api_missing_identity_blocks_pending_usereg_before_dispatch() {
    let s = FixtureServer::new(vec![]);
    let mut r = pending_login(&s);
    r.coordinator.logout(ServiceId::Identity).unwrap();
    assert!(r.complete_usereg_login("1234".into(), None).await.is_err());
    assert!(r.usereg_pending.is_none());
    assert!(s.requests().is_empty());
}

#[test]
fn backend_repair_deep_api_self_service_logout_keeps_identity_and_invalidates_devices() {
    let server = FixtureServer::new(vec![]);
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity, "primary-a");
    prove(&mut r, ServiceId::Usereg, "network-b");
    r.usereg_adapter = Some(usereg_adapter(&server));
    r.usereg_devices_generation = 7;
    r.usereg_devices.push(crate::usereg_adapter::UseregDevice {
        id: "fixture-device-id".into(),
        ip4: "192.0.2.10".into(),
        ip6: String::new(),
        logged_at: "fixture-time".into(),
        mac: "AA-BB-CC".into(),
        auth_permission: "fixture-permission".into(),
    });

    let status = r.logout_self_service();

    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(!r.service_session_is_proven(ServiceId::Usereg));
    assert_eq!(
        status.identity().state(),
        crate::auth::AccountAuthState::Authenticated
    );
    assert_eq!(
        status.self_service().state(),
        crate::auth::AccountAuthState::SignedOut
    );
    assert_ne!(r.usereg_devices_generation, 7);
    assert!(r.usereg_devices.is_empty());
    assert!(server.requests().is_empty());
}

#[test]
fn backend_repair_deep_api_identity_logout_expires_but_retains_self_service_selection() {
    let root = std::env::temp_dir().join(format!("thyou-identity-logout-{}", Uuid::new_v4()));
    let server = FixtureServer::new(vec![]);
    let mut r = CampusRuntime::new_auto_with_persistence_at(
        "2026-2027-1".into(),
        false,
        String::new(),
        false,
        root.clone(),
    )
    .unwrap();
    prove(&mut r, ServiceId::Identity, "primary-a");
    prove(&mut r, ServiceId::Usereg, "network-b");
    r.usereg_adapter = Some(usereg_adapter(&server));
    r.self_service_account_username = Some("network-b".into());

    let status = r.logout_identity().unwrap();

    assert!(!r.service_session_is_proven(ServiceId::Identity));
    assert!(!r.service_session_is_proven(ServiceId::Usereg));
    assert_eq!(
        status.identity().state(),
        crate::auth::AccountAuthState::SignedOut
    );
    assert_eq!(
        status.self_service().state(),
        crate::auth::AccountAuthState::Expired
    );
    assert_eq!(status.self_service().username(), Some("network-b"));
    assert!(server.requests().is_empty());
    drop(r);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_deep_api_cache_scope_separates_account_semester_and_stage() {
    let path = std::path::Path::new("/tmp/synthetic-overview.json");
    let mut paths = HashSet::new();
    for account in ["primary-a", "primary-b"] {
        for semester in ["2026-2027-1", "2026-2027-2"] {
            for stage in [AcademicStage::Undergraduate, AcademicStage::Graduate] {
                let p = academic_cache_path(path, account, semester, stage);
                assert_eq!(p.parent(), path.parent());
                assert_eq!(p.extension().unwrap(), "json");
                assert!(!p.to_string_lossy().contains(account));
                assert!(paths.insert(p));
            }
        }
    }
    assert_eq!(paths.len(), 8);
}

#[tokio::test]
async fn backend_repair_deep_api_same_scope_cache_remains_explicit_fallback_on_outage() {
    let s = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut r = runtime();
    install_academic(&mut r, &s);
    let dir = std::env::temp_dir().join(format!("thyou-current-scope-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    r.cache_path = dir.join("overview.json");
    let cache = crate::cache::JsonFileCache::<CampusOverviewCachePayload>::new(
        overview_cache_path_for_account_stage(&r.cache_path, "primary-a", r.stage),
        CAMPUS_OVERVIEW_CACHE_SCHEMA_VERSION,
        CAMPUS_OVERVIEW_CACHE_SERVICE,
    );
    let date = NaiveDate::from_ymd_opt(2026, 9, 17).unwrap();
    let overview = crate::domain::CampusOverview {
        date,
        generated_at: Utc::now(),
        semester: Some(r.semester.clone()),
        course_count: 7,
        pending_todo_count: 0,
        completed_todo_count: 0,
        today_schedule: vec![],
        upcoming_todos: vec![],
        next_schedule: None,
    };
    cache
        .write(CampusOverviewCachePayload {
            account_scope: cache_account_scope("primary-a"),
            stage: r.stage,
            date,
            generated_at: overview.generated_at,
            overview,
        })
        .unwrap();
    let result = r.load_overview("2026-09-17".into()).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.overview.unwrap().course_count, 7);
    assert_eq!(s.requests().len(), 0);
}

#[test]
fn backend_repair_deep_api_pending_portal_does_not_disable_proven_library() {
    let s = FixtureServer::new(vec![]);
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity, "primary-a");
    prove(&mut r, ServiceId::Library, "primary-a");
    r.library_adapter = Some(
        LibraryReadAdapter::try_with_transport(
            Url::parse(s.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.service_second_factor = Some(PendingServiceSecondFactor {
        target: ServiceSecondFactorTarget::Portal,
        challenge: SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Wechat],
            masked_phone: None,
            expires_at: None,
        },
    });
    let catalog = r.service_catalog();
    assert_eq!(
        catalog
            .services
            .iter()
            .find(|e| e.id == "library")
            .unwrap()
            .availability,
        "available"
    );
    assert_eq!(
        catalog
            .services
            .iter()
            .find(|e| e.id == "info")
            .unwrap()
            .availability,
        "requires_second_factor"
    );
    assert!(s.requests().is_empty());
}

#[test]
fn backend_repair_deep_api_undergraduate_exam_capability_is_retained() {
    let s = FixtureServer::new(vec![]);
    let mut r = runtime();
    install_academic(&mut r, &s);
    prove(&mut r, ServiceId::Registrar, "primary-a");
    r.grades_source = r.learn_source.clone();
    assert!(
        r.service_catalog()
            .services
            .iter()
            .find(|e| e.id == "registrar")
            .unwrap()
            .capabilities
            .iter()
            .any(|c| c.key == "read_exams")
    );
    assert!(s.requests().is_empty());
}

#[test]
fn backend_repair_deep_api_expired_service_challenge_is_not_advertised_as_actionable() {
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity, "primary-a");
    r.service_second_factor = Some(PendingServiceSecondFactor {
        target: ServiceSecondFactorTarget::CampusCard,
        challenge: SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Wechat],
            masked_phone: None,
            expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
        },
    });
    assert_eq!(r.status().state, "authenticated");
    assert!(r.service_second_factor_methods().is_empty());
    assert_eq!(
        r.service_catalog()
            .services
            .iter()
            .find(|e| e.id == "campus_card")
            .unwrap()
            .availability,
        "requires_service_session"
    );
}

#[tokio::test]
async fn backend_repair_deep_api_tunet_malformed_account_identifier_cannot_prove_online() {
    let s = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: r#"thyouStatus({"error":"ok","online_ip":"192.0.2.10","user_name":{"bad":true}});"#
            .into(),
    }]);
    let mut r = tunet_runtime(&s);
    assert!(load_tunet_target_status(&mut r).await.is_err());
    assert_eq!(s.requests().len(), 1);
}

const HOME: &str = r#"<html><body><input type="hidden" name="_csrf-8800" value="fixture-form"><div id="w1-container"><table><tbody></tbody></table></div><div id="w3-container"><table><tbody><tr><td>学生</td><td>1G</td><td>2h</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div><span class="glyphicon-info-sign">正常</span></body></html>"#;

fn username(name: &str) -> UserIdentity {
    UserIdentity {
        username: name.into(),
        display_name: None,
    }
}
fn runtime() -> CampusRuntime {
    CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false).unwrap()
}
fn prove(r: &mut CampusRuntime, service: ServiceId, name: &str) {
    r.coordinator.begin_authentication(service).unwrap();
    r.coordinator
        .mark_authenticated(service, username(name), None, None, None)
        .unwrap();
}
fn user_page(name: &str) -> Reply {
    Reply::html(&format!(
        "<html><div id=\"w0\"><table><tr><td>{name}</td><td>fixture@example.invalid</td><td>13800000000</td><td>fixture</td><td></td><td>01012345678</td><td>Fixture</td><td>学生</td></tr></table></div></html>"
    ))
}
fn usereg_adapter(server: &FixtureServer) -> UseregAdapter {
    UseregAdapter::new(
        UseregClient::new(
            UseregClientConfig::new(server.base(), UseregProfile::default()).unwrap(),
        )
        .unwrap(),
    )
}
fn usereg_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity, "primary-a");
    prove(&mut r, ServiceId::Usereg, "network-b");
    r.usereg_adapter = Some(usereg_adapter(server));
    r
}
fn pending_login(server: &FixtureServer) -> CampusRuntime {
    let mut r = runtime();
    prove(&mut r, ServiceId::Identity, "primary-a");
    r.coordinator
        .begin_authentication(ServiceId::Usereg)
        .unwrap();
    let adapter = usereg_adapter(server);
    // Public RSA fixture already used by the repository's isolated tests.
    let key = "-----BEGIN PUBLIC KEY-----\nMIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDRXPpskRTB0TMRDcrQU+qve7lr\nXX2/jIBtErat5KH5YGOqZvXjVH1/BU0GfkbIhAI7qOSOzwKdIKtejt8RgrIS5qoL\ncYZZb87CK5wCydkTIZkH59MiyQ5Dt1GeE90Wff9zczIE4BS29AqxgzZIEq1TsPp5\nJ7jNOyN8KKRa62yvlQIDAQAB\n-----END PUBLIC KEY-----";
    let page=adapter.parse_login_page(&format!("<meta name=\"csrf-token\" content=\"fixture-header\"><input type=\"hidden\" name=\"_csrf-8800\" value=\"fixture-form\"><textarea id=\"public\">{key}</textarea>")).unwrap();
    r.usereg_pending = Some(UseregPendingLogin {
        adapter,
        page,
        credentials: UseregLoginCredentials::new("network-b", "synthetic-password").unwrap(),
        identity_owner: username("primary-a"),
        transport: r.identity.transport().clone(),
        created_at: std::time::Instant::now(),
        captcha_ready: true,
    });
    r
}

#[tokio::test]
async fn backend_repair_deep_api_usereg_account_mismatch_never_reaches_dto() {
    let s = FixtureServer::new(vec![
        Reply::html(HOME),
        user_page("network-c"),
        Reply::html("<span class=\"glyphicon-exclamation-sign\"></span><strong>5</strong>"),
    ]);
    let mut r = usereg_runtime(&s);
    assert!(r.load_usereg_account().await.is_err());
    assert!(!r.service_session_is_proven(ServiceId::Usereg));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(
        s.requests().len(),
        2,
        "do not request more account data after mismatch"
    );
}

#[tokio::test]
async fn backend_repair_deep_api_usereg_independent_account_can_differ_from_primary() {
    let s = FixtureServer::new(vec![
        Reply::html(HOME),
        user_page("network-b"),
        Reply::html("<span class=\"glyphicon-exclamation-sign\"></span><strong>5</strong>"),
    ]);
    let mut r = usereg_runtime(&s);
    assert_eq!(r.load_usereg_account().await.unwrap().username, "network-b");
    assert_eq!(r.status().username.as_deref(), Some("primary-a"));
    assert_eq!(s.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_deep_api_usereg_login_records_proven_independent_username() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"success":true}"#),
        Reply::html(HOME),
        Reply::html(HOME),
        Reply::html(HOME),
        user_page("network-b"),
    ]);
    let mut r = pending_login(&s);
    r.complete_usereg_login("1234".into(), None).await.unwrap();
    assert_eq!(
        r.coordinator
            .registry()
            .snapshot_for(ServiceId::Usereg)
            .user
            .unwrap()
            .username,
        "network-b"
    );
    assert_eq!(r.status().username.as_deref(), Some("primary-a"));
    assert!(r.usereg_pending.is_none());
    assert_eq!(
        s.requests().len(),
        5,
        "one validation, one POST, two existing home checks, one user proof"
    );
}

#[tokio::test]
async fn backend_repair_deep_api_usereg_unconfirmed_final_post_cannot_replay_login() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"success":true}"#),
        Reply {
            status: 503,
            headers: "Content-Type: text/html\r\n".into(),
            body: "<html>unavailable</html>".into(),
        },
    ]);
    let mut r = pending_login(&s);
    assert!(r.complete_usereg_login("1234".into(), None).await.is_err());
    assert!(
        r.usereg_pending.is_none(),
        "an unknown submitted outcome is not reusable authentication material"
    );
    assert!(r.complete_usereg_login("1234".into(), None).await.is_err());
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_deep_api_usereg_login_owner_mismatch_never_proves_session() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"success":true}"#),
        Reply::html(HOME),
        Reply::html(HOME),
        Reply::html(HOME),
        user_page("network-c"),
    ]);
    let mut r = pending_login(&s);
    assert!(r.complete_usereg_login("1234".into(), None).await.is_err());
    assert!(!r.service_session_is_proven(ServiceId::Usereg));
    assert!(r.usereg_pending.is_none());
    assert!(r.service_session_is_proven(ServiceId::Identity));
}

#[tokio::test]
async fn backend_repair_deep_api_usereg_captcha_rejection_keeps_explicit_retry() {
    let s = FixtureServer::new(vec![Reply::json(
        r#"{"success":false,"message":"验证码错误"}"#,
    )]);
    let mut r = pending_login(&s);
    assert!(r.complete_usereg_login("1234".into(), None).await.is_err());
    assert!(r.usereg_pending.is_some());
    assert_eq!(s.requests().len(), 1);
}

fn tunet_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut r = runtime();
    let profile = crate::tunet::TunetProfile::current_with_overrides(
        crate::tunet::AuthFamily::Auth4,
        crate::tunet::TunetProfileOverrides {
            endpoint: Some(
                crate::tunet::HttpsEndpoint::http(
                    "127.0.0.1",
                    Url::parse(server.base()).unwrap().port().unwrap(),
                )
                .unwrap(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    let client = TunetClient::with_transport(
        TunetClientConfig::new(profile).unwrap(),
        r.identity.transport().clone(),
    )
    .unwrap();
    r.tunet_connection_target = Some(super::TunetConnectionTarget {
        client,
        username: "network-b".into(),
        local_ipv4: "192.0.2.10".into(),
    });
    r
}

async fn load_tunet_target_status(
    runtime: &mut CampusRuntime,
) -> Result<super::TunetNetworkStatusDto, String> {
    let target = runtime.tunet_connection_target.as_ref().unwrap();
    runtime
        .load_tunet_status_using(
            target.client.clone(),
            target.local_ipv4.clone(),
            Some(target.username.clone()),
        )
        .await
}

#[tokio::test]
async fn backend_repair_deep_api_tunet_same_ip_wrong_account_is_not_ours() {
    let s = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: r#"thyouStatus({"error":"ok","online_ip":"192.0.2.10","user_name":"network-c"});"#
            .into(),
    }]);
    let mut r = tunet_runtime(&s);
    assert!(load_tunet_target_status(&mut r).await.is_err());
    assert_eq!(s.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_deep_api_tunet_matching_username_and_ip_are_usable() {
    let s = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: r#"thyouStatus({"error":"ok","online_ip":"192.0.2.10","user_name":"network-b"});"#
            .into(),
    }]);
    let mut r = tunet_runtime(&s);
    assert!(load_tunet_target_status(&mut r).await.unwrap().online);
    assert_eq!(s.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_tunet_physical_ip_probe_does_not_create_logout_authority() {
    for (body, expected_state) in [
        (
            r#"thyouStatus({"error":"ok","online_ip":"192.0.2.10","user_name":"another-network-account"});"#,
            "online",
        ),
        (
            r#"thyouStatus({"error":"not_online_error","online_ip":"","online_device_total":0});"#,
            "offline",
        ),
        (
            r#"thyouStatus({"error":"ok","online_ip":"192.0.2.11"});"#,
            "unknown",
        ),
    ] {
        let server = FixtureServer::new(vec![Reply {
            status: 200,
            headers: "Content-Type: application/javascript\r\n".into(),
            body: body.into(),
        }]);
        let mut runtime = tunet_runtime(&server);
        let client = runtime.tunet_connection_target.take().unwrap().client;
        let status = runtime
            .load_tunet_status_using(client, "192.0.2.10".into(), None)
            .await
            .unwrap();
        assert_eq!(status.state, expected_state);
        assert_eq!(status.online, expected_state == "online");
        assert!(runtime.tunet_connection_target.is_none());
        assert!(runtime.disconnect_tunet().await.is_err());
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_repair_tunet_disconnect_unclear_result_consumes_target_without_replay() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut runtime = tunet_runtime(&server);

    assert!(
        runtime
            .disconnect_tunet_using("192.0.2.10".into())
            .await
            .is_err()
    );
    assert!(runtime.tunet_connection_target.is_none());

    assert!(
        runtime
            .disconnect_tunet_using("192.0.2.10".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
}

fn install_academic(r: &mut CampusRuntime, server: &FixtureServer) {
    prove(r, ServiceId::Identity, "primary-a");
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
    let mut source = CampusLiveDataSource::without_todos(
        learn,
        registrar,
        CampusLiveConfig::new(&r.semester, r.stage),
    )
    .unwrap();
    let csrf = r
        .coordinator
        .registry()
        .bind_csrf(ServiceId::Learn, CsrfToken::new("synthetic-csrf").unwrap());
    source.with_learn_csrf(csrf.clone(), "_csrf").unwrap();
    r.coordinator
        .begin_authentication(ServiceId::Learn)
        .unwrap();
    r.coordinator
        .mark_authenticated(
            ServiceId::Learn,
            username("primary-a"),
            None,
            Some(csrf),
            None,
        )
        .unwrap();
    r.learn_source = Some(Arc::new(source));
}

#[tokio::test]
async fn backend_repair_deep_api_legacy_cache_other_semester_cannot_mask_failed_read() {
    let s = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut r = runtime();
    install_academic(&mut r, &s);
    let dir = std::env::temp_dir().join(format!("thyou-scope-fixture-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    r.cache_path = dir.join("overview.json");
    let cache = crate::cache::JsonFileCache::<CampusOverviewCachePayload>::new(
        overview_cache_path_for_account_stage(&r.cache_path, "primary-a", r.stage),
        CAMPUS_OVERVIEW_CACHE_SCHEMA_VERSION,
        CAMPUS_OVERVIEW_CACHE_SERVICE,
    );
    let date = NaiveDate::from_ymd_opt(2026, 9, 17).unwrap();
    let overview = crate::domain::CampusOverview {
        date,
        generated_at: Utc::now(),
        semester: Some("2025-2026-2".into()),
        course_count: 99,
        pending_todo_count: 0,
        completed_todo_count: 0,
        today_schedule: vec![],
        upcoming_todos: vec![],
        next_schedule: None,
    };
    cache
        .write(CampusOverviewCachePayload {
            account_scope: cache_account_scope("primary-a"),
            stage: r.stage,
            date,
            generated_at: overview.generated_at,
            overview,
        })
        .unwrap();
    let result = r.load_overview("2026-09-17".into()).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(result.status, "error");
    assert!(result.overview.is_none());
    assert_eq!(s.requests().len(), 1);
}

#[test]
fn backend_repair_deep_api_catalog_pending_service_matches_status_without_false_proofs() {
    for (target, keys) in [
        (
            ServiceSecondFactorTarget::Portal,
            vec!["info", "library", "classroom", "electricity"],
        ),
        (ServiceSecondFactorTarget::CampusCard, vec!["campus_card"]),
    ] {
        let mut r = runtime();
        prove(&mut r, ServiceId::Identity, "primary-a");
        r.service_second_factor = Some(PendingServiceSecondFactor {
            target,
            challenge: SecondFactorChallenge {
                methods: vec![SecondFactorMethod::Wechat],
                masked_phone: None,
                expires_at: None,
            },
        });
        assert_eq!(r.status().state, "requires_second_factor");
        for key in keys {
            let catalog = r.service_catalog();
            let entry = catalog.services.iter().find(|e| e.id == key).unwrap();
            assert_eq!(entry.availability, "requires_second_factor");
            assert!(entry.capabilities.is_empty());
        }
    }
}

#[test]
fn backend_repair_deep_api_graduate_catalog_never_advertises_unsupported_exam_read() {
    let s = FixtureServer::new(vec![]);
    let mut r = runtime();
    r.stage = AcademicStage::Graduate;
    install_academic(&mut r, &s);
    prove(&mut r, ServiceId::Registrar, "primary-a");
    r.grades_source = r.learn_source.clone();
    let catalog = r.service_catalog();
    let registrar = catalog
        .services
        .iter()
        .find(|e| e.id == "registrar")
        .unwrap();
    assert!(
        registrar
            .capabilities
            .iter()
            .any(|c| c.key == "read_grades")
    );
    assert!(!registrar.capabilities.iter().any(|c| c.key == "read_exams"));
    assert!(s.requests().is_empty());
}
