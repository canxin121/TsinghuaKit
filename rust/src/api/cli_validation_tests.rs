use super::*;
use crate::protocol::{SecondFactorChallenge, SecondFactorMethod};
use crate::reference_test_support::{FixtureServer, Reply};
use std::collections::VecDeque;

#[path = "cli_thos_tests.rs"]
mod thos_tests;

#[path = "cli_learn_calendar_tests.rs"]
mod learn_calendar_tests;
#[path = "cli_learn_discussions_tests.rs"]
mod learn_discussions_tests;
#[path = "cli_learn_files_tests.rs"]
mod learn_files_tests;
#[path = "cli_learn_homework_tests.rs"]
mod learn_homework_tests;

#[test]
fn backend_repair_followup_retry_excludes_newly_passed_classroom_and_network() {
    let root = directory();
    let path = root.join("report.json");
    let mut writer = ReportWriter::new(
        path.clone(),
        "fixture_run".into(),
        &select_cases(&[]).unwrap(),
        false,
    )
    .unwrap();
    for row in writer.report.cases.values_mut() {
        row.status = CheckStatus::Passed;
    }
    for id in ["registrar_session", "electricity_session"] {
        writer.report.cases.get_mut(id).unwrap().status = CheckStatus::Failed;
    }
    for id in [
        "registrar_schedule",
        "registrar_grades",
        "registrar_exams",
        "electricity_remainder",
        "electricity_history",
        "overview_live_inputs",
    ] {
        writer.report.cases.get_mut(id).unwrap().status = CheckStatus::Blocked;
    }
    for id in [
        "usereg_session",
        "usereg_account",
        "usereg_balance",
        "usereg_devices",
    ] {
        writer.report.cases.get_mut(id).unwrap().status = CheckStatus::Skipped;
    }
    writer.finish().unwrap();
    let original = fs::read(&path).unwrap();
    let (selected, graduate) = retry_plan(&path).unwrap();
    assert!(!graduate);
    assert_eq!(selected.len(), 18);
    for id in [
        "registrar_session",
        "registrar_schedule",
        "registrar_grades",
        "registrar_exams",
        "electricity_session",
        "electricity_remainder",
        "electricity_history",
        "overview_live_inputs",
    ] {
        assert!(selected.contains(id));
    }
    for id in [
        "classroom_session",
        "classroom_state",
        "tunet_status",
        "library_session",
        "campus_card_session",
        "info_news",
        "learn_announcements",
    ] {
        assert!(!selected.contains(id));
    }
    assert_eq!(fs::read(&path).unwrap(), original);
    drop(writer);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_sep19_terminal_device_survives_new_runtime_without_session_restore() {
    let root = directory();
    let cache = root.join("device").join("unused-cache.json");
    let first = create_backend_validation_runtime(
        "auto".into(),
        false,
        cache.to_string_lossy().into_owned(),
    )
    .unwrap();
    let second = create_backend_validation_runtime(
        "auto".into(),
        false,
        cache.to_string_lossy().into_owned(),
    )
    .unwrap();
    assert!(
        first.fingerprint == second.fingerprint,
        "terminal device changed across runtimes"
    );
    assert_eq!(second.status().state, "signed_out");
    assert!(!second.persist_sessions);
    assert!(!second.service_session_is_proven(ServiceId::Identity));
    assert!(
        !cache.exists(),
        "device metadata must not create a business cache"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_sep19_terminal_trust_requires_explicit_consent_without_sending_code() {
    let before = crate::telemetry::request_count();
    for consent in [false, true] {
        let mut prompt = Prompt {
            send: consent,
            ..Prompt::default()
        };
        assert_eq!(confirm_terminal_device_trust(&mut prompt).unwrap(), consent);
        assert_eq!(prompt.confirmations, 1);
        assert_eq!(prompt.secret_reads, 0);
    }
    assert_eq!(crate::telemetry::request_count(), before);
}

#[test]
fn backend_repair_sep19_terminal_corrupt_device_fails_before_network_without_rotation() {
    let root = directory();
    let cache = root.join("device").join("unused-cache.json");
    let create = || {
        create_backend_validation_runtime(
            "auto".into(),
            false,
            cache.to_string_lossy().into_owned(),
        )
    };
    let first = create().unwrap();
    let device = cache.parent().unwrap().join(DEVICE_FINGERPRINT_FILE);
    let sentinel = cache.parent().unwrap().join("campus-session-v1.json");
    fs::write(&sentinel, "synthetic-session-must-not-be-restored").unwrap();
    let second = create().unwrap();
    assert!(first.fingerprint == second.fingerprint);
    assert_eq!(second.status().state, "signed_out");
    assert_eq!(
        fs::read_to_string(&sentinel).unwrap(),
        "synthetic-session-must-not-be-restored"
    );
    fs::write(&device, "corrupt-device-identity").unwrap();
    let before = crate::telemetry::request_count();
    assert!(create().is_err());
    assert_eq!(
        fs::read_to_string(device).unwrap(),
        "corrupt-device-identity"
    );
    assert_eq!(crate::telemetry::request_count(), before);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn backend_repair_sep19_terminal_device_permissions_and_symlinks_are_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = directory();
    let cache = root.join("device").join("unused-cache.json");
    let create = || {
        create_backend_validation_runtime(
            "auto".into(),
            false,
            cache.to_string_lossy().into_owned(),
        )
    };
    create().unwrap();
    let parent = cache.parent().unwrap();
    let device = parent.join(DEVICE_FINGERPRINT_FILE);
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&device).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&device, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(create().is_err());
    fs::remove_file(&device).unwrap();
    let target = root.join("untouched");
    fs::write(&target, "00000000000000000000000000000000").unwrap();
    symlink(&target, &device).unwrap();
    assert!(create().is_err());
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "00000000000000000000000000000000"
    );
    fs::remove_file(&device).unwrap();
    fs::remove_dir(parent).unwrap();
    let elsewhere = root.join("elsewhere");
    crate::telemetry::private_dir(&elsewhere).unwrap();
    symlink(&elsewhere, parent).unwrap();
    assert!(create().is_err());
    assert!(!elsewhere.join(DEVICE_FINGERPRINT_FILE).exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn backend_repair_sep20_app_device_marker_migration_preserves_strict_validation() {
    use std::os::unix::fs::PermissionsExt;

    let root = directory();
    let cache = root.join("device").join("unused-cache.json");
    create_backend_validation_runtime("auto".into(), false, cache.to_string_lossy().into_owned())
        .unwrap();
    let device = cache.parent().unwrap().join(DEVICE_FINGERPRINT_FILE);
    let before = fs::read(&device).unwrap();
    fs::set_permissions(&device, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(super::super::terminal_device::resolve(&cache).is_err());
    let resolved = super::super::terminal_device::resolve_persistent(&cache).unwrap();

    assert_eq!(resolved.as_bytes(), before.as_slice());
    assert_eq!(fs::read(&device).unwrap(), before);
    assert_eq!(
        fs::metadata(&device).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_sep19_retry_plan_selects_failures_and_dependencies_not_unrelated_successes() {
    let root = directory();
    let path = root.join("report.json");
    let selected = select_cases(&[]).unwrap();
    let mut writer =
        ReportWriter::new(path.clone(), "fixture_run".into(), &selected, false).unwrap();
    for row in writer.report.cases.values_mut() {
        row.status = CheckStatus::Passed;
    }
    for id in [
        "registrar_session",
        "classroom_state",
        "electricity_session",
        "tunet_status",
    ] {
        writer.report.cases.get_mut(id).unwrap().status = CheckStatus::Failed;
    }
    for id in [
        "registrar_schedule",
        "registrar_grades",
        "registrar_exams",
        "electricity_remainder",
        "electricity_history",
        "overview_live_inputs",
    ] {
        writer.report.cases.get_mut(id).unwrap().status = CheckStatus::Blocked;
    }
    for id in [
        "usereg_session",
        "usereg_account",
        "usereg_balance",
        "usereg_devices",
    ] {
        writer.report.cases.get_mut(id).unwrap().status = CheckStatus::Skipped;
    }
    writer.finish().unwrap();
    let before = fs::read(&path).unwrap();
    let (retry, graduate) = retry_plan(&path).unwrap();
    assert!(!graduate);
    for id in [
        "identity_session",
        "portal_bootstrap",
        "learn_session",
        "learn_courses",
        "learn_todos",
        "registrar_session",
        "registrar_schedule",
        "registrar_grades",
        "registrar_exams",
        "info_session",
        "classroom_session",
        "classroom_state",
        "electricity_session",
        "electricity_remainder",
        "electricity_history",
        "tunet_status",
        "overview_live_inputs",
    ] {
        assert!(retry.contains(id), "missing required case: {id}");
    }
    for id in [
        "learn_announcements",
        "info_news",
        "library_session",
        "campus_card_session",
    ] {
        assert!(!retry.contains(id), "unnecessary repeated case: {id}");
    }
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(writer);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_sep19_retry_empty_or_invalid_report_never_expands_to_full_run() {
    let root = directory();
    let path = root.join("report.json");
    let selected = select_cases(&["identity_session".into()]).unwrap();
    let mut writer =
        ReportWriter::new(path.clone(), "fixture_run".into(), &selected, true).unwrap();
    writer
        .report
        .cases
        .get_mut("identity_session")
        .unwrap()
        .status = CheckStatus::Passed;
    writer.finish().unwrap();
    assert_eq!(
        retry_plan(&path).unwrap_err(),
        "retry_report_has_no_unfinished_cases"
    );
    writer
        .report
        .cases
        .get_mut("identity_session")
        .unwrap()
        .status = CheckStatus::Interrupted;
    writer.finish().unwrap();
    assert!(retry_plan(&path).unwrap().1);
    writer.report.schema = 999;
    writer.finish().unwrap();
    assert_eq!(retry_plan(&path).unwrap_err(), "retry_report_invalid");
    drop(writer);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_sep19_network_observation_does_not_prove_a_different_local_ip() {
    let record = crate::tunet_client::parse_status_response(
        r#"{"error":"ok","online_ip":"192.0.2.20","online":true}"#,
        None,
    )
    .unwrap();
    assert!(matches!(
        network_observation_outcome(&record),
        Ok(Outcome::ObservedNetwork("tunet_request_origin_online"))
    ));
    assert!(!record.is_online_proven_for_ip("192.0.2.10"));
    assert!(!record.is_offline_proven_for_ip("192.0.2.10"));
    assert_eq!(
        record.unproven_reason_for_ip("192.0.2.10"),
        "tunet_address_mismatch"
    );
    for body in [
        r#"{"error":"ok"}"#,
        r#"{"error":"ok","online_ip":"invalid"}"#,
        r#"{"error":"ok","online":false,"online_ip":"192.0.2.20"}"#,
    ] {
        let record = crate::tunet_client::parse_status_response(body, None).unwrap();
        assert!(network_observation_outcome(&record).is_err());
    }
    let offline = crate::tunet_client::parse_status_response(
        r#"{"error":"ok","online":false,"online_device_total":0}"#,
        None,
    )
    .unwrap();
    assert!(matches!(
        network_observation_outcome(&offline),
        Ok(Outcome::ObservedNetwork("tunet_request_origin_offline"))
    ));
}

fn directory() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "thyou-cli-fixture-{}",
        uuid::Uuid::new_v4().simple()
    ));
    crate::telemetry::private_dir(&p).unwrap();
    p
}

#[tokio::test]
async fn backend_repair_terminal_interrupted_case_keeps_actual_dispatched_request_count() {
    let root = directory();
    let server = FixtureServer::new(vec![Reply::html("synthetic")]);
    let selected = select_cases(&["identity_session".into()]).unwrap();
    let mut report = ReportWriter::new(
        root.join("report.json"),
        "fixture_run".into(),
        &selected,
        false,
    )
    .unwrap();
    report.start("identity_session").unwrap();
    crate::transport::CampusHttpTransport::new("THYou/interrupt-fixture")
        .unwrap()
        .get_text(server.base())
        .await
        .unwrap();
    report.interrupted().unwrap();
    assert_eq!(
        report.report.cases["identity_session"].status,
        CheckStatus::Interrupted
    );
    assert_eq!(report.report.cases["identity_session"].requests, 1);
    assert_eq!(report.report.request_count, 1);
    assert_eq!(report.exit_code(), 1);
    drop(report);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_terminal_invalid_or_incomplete_plan_cannot_create_passing_report() {
    let root = directory();
    for selected in [
        BTreeSet::new(),
        BTreeSet::from(["unknown_case".into()]),
        BTreeSet::from(["library_seats".into()]),
    ] {
        assert!(
            ReportWriter::new(
                root.join("report.json"),
                "fixture_run".into(),
                &selected,
                false
            )
            .is_err()
        );
        assert!(!root.join("report.json").exists());
    }
    fs::remove_dir_all(root).unwrap();
}
struct Prompt {
    secrets: VecDeque<String>,
    factor: String,
    send: bool,
    confirmations: usize,
    secret_reads: usize,
    progress: Vec<(String, CheckStatus)>,
}
impl Default for Prompt {
    fn default() -> Self {
        Self {
            secrets: VecDeque::new(),
            factor: "wechat".into(),
            send: false,
            confirmations: 0,
            secret_reads: 0,
            progress: vec![],
        }
    }
}
impl UserPrompts for Prompt {
    fn secret(&mut self, _: &'static str) -> Result<String, String> {
        self.secret_reads += 1;
        Ok(self.secrets.pop_front().unwrap_or_default())
    }
    fn confirm(&mut self, _: &'static str) -> Result<bool, String> {
        self.confirmations += 1;
        Ok(self.send)
    }
    fn choose_factor(&mut self, _: &[String]) -> Result<String, String> {
        Ok(self.factor.clone())
    }
    fn show_captcha(&mut self, _: &[u8], _: &str) -> Result<(), String> {
        Err("fixture_no_captcha_viewer".into())
    }
    fn clear_captcha(&mut self) {}
    fn progress(&mut self, spec: &CheckSpec, status: CheckStatus, _: &str) {
        self.progress.push((spec.id.into(), status));
    }
}
fn fixture_runtime(server: &FixtureServer, directory: &Path) -> CampusRuntime {
    let mut runtime = create_backend_validation_runtime(
        "2026-2027-1".into(),
        false,
        directory
            .join("metadata")
            .join("cache.json")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let profile = runtime
        .identity
        .identity()
        .client()
        .config()
        .profile
        .clone();
    let client =
        IdentityClient::new(IdentityClientConfig::new(server.base(), profile).unwrap()).unwrap();
    runtime.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        client,
        crate::transport::CampusHttpTransport::new("THYou/cli-fixture").unwrap(),
    ));
    runtime
}

#[tokio::test]
async fn backend_repair_info_detail_terminal_rejects_fresh_cache_as_live_proof() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let user = UserIdentity {
        username: "fixture-news-owner".into(),
        display_name: None,
    };
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    let payload = InfoNewsDetailCachePayload {
        account_scope: cache_account_scope(&user.username),
        article_id: "fixture-article".into(),
        bound_link: "/fixture-article".into(),
        generated_at: chrono::Utc::now(),
        title: "Fixture title".into(),
        content_html: "<p>Fixture article</p>".into(),
        summary: "Fixture article".into(),
        attachments: vec![],
    };
    let cache_path = info_news_detail_cache_path(
        &runtime.cache_path,
        &user.username,
        &payload.article_id,
        &payload.bound_link,
    );
    JsonFileCache::<InfoNewsDetailCachePayload>::new(
        &cache_path,
        INFO_NEWS_DETAIL_CACHE_SCHEMA_VERSION,
        INFO_NEWS_DETAIL_CACHE_SERVICE,
    )
    .write(&payload)
    .unwrap();
    let mut evidence = Evidence {
        article: Some(payload.article_id.clone()),
        ..Evidence::default()
    };
    let mut prompt = Prompt::default();
    assert_eq!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "info_detail",
            false
        )
        .await
        .err()
        .unwrap(),
        "validation_live_result_required"
    );
    assert!(server.requests().is_empty());
    assert_eq!(prompt.secret_reads, 0);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}
fn pending(runtime: &mut CampusRuntime) {
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    runtime
        .coordinator
        .require_second_factor(
            ServiceId::Identity,
            SecondFactorChallenge {
                methods: vec![SecondFactorMethod::Wechat],
                masked_phone: None,
                expires_at: None,
            },
            Some(UserIdentity {
                username: "fixture-user".into(),
                display_name: None,
            }),
        )
        .unwrap();
}

#[test]
fn backend_repair_thos_cli_catalog_keeps_ordered_read_only_dependencies() {
    assert!(CHECKS.iter().any(|spec| spec.id == "thos_pending"));
    let mut seen = BTreeSet::new();
    for spec in CHECKS {
        assert!(!spec.id.contains("disconnect"));
        assert!(!spec.id.contains("payment_submit"));
        assert!(spec.dependencies.iter().all(|d| seen.contains(d)));
        assert!(seen.insert(spec.id));
    }
    let chosen = select_cases(&["library_seats".into()]).unwrap();
    for id in [
        "identity_session",
        "portal_bootstrap",
        "info_session",
        "library_session",
        "library_area_tree",
        "library_day_segments",
        "library_seats",
    ] {
        assert!(chosen.contains(id));
    }
    assert!(!chosen.contains("campus_card_account"));
    assert!(select_cases(&["disconnect_network".into()]).is_err());
    assert_eq!(
        select_cases(&["thos_pending".into()]).unwrap(),
        BTreeSet::from([
            "identity_session".into(),
            "portal_bootstrap".into(),
            "info_session".into(),
            "thos_pending".into(),
        ])
    );
}

#[test]
fn backend_repair_news_catalog_cli_selects_only_info_session_chain() {
    let selected = select_cases(&["info_catalog".into()]).unwrap();
    assert_eq!(
        selected,
        BTreeSet::from([
            "identity_session".into(),
            "portal_bootstrap".into(),
            "info_session".into(),
            "info_catalog".into(),
        ])
    );
    assert!(!selected.contains("info_news"));
    assert!(!selected.contains("info_detail"));
}

#[test]
fn backend_repair_terminal_report_is_immutable_and_checkpointed_before_attempt() {
    let root = directory();
    let selected = select_cases(&["identity_session".into()]).unwrap();
    let mut report = ReportWriter::new(
        root.join("report.json"),
        "fixture_run".into(),
        &selected,
        false,
    )
    .unwrap();
    report.start("identity_session").unwrap();
    let disk: CheckReport =
        serde_json::from_slice(&fs::read(root.join("report.json")).unwrap()).unwrap();
    assert_eq!(disk.cases["identity_session"].status, CheckStatus::Running);
    report
        .end(
            "identity_session",
            CheckStatus::Passed,
            "verified",
            None,
            20,
            1,
        )
        .unwrap();
    assert!(report.start("identity_session").is_err());
    assert!(
        report
            .end("identity_session", CheckStatus::Failed, "other", None, 0, 0)
            .is_err()
    );
    report.finish().unwrap();
    assert_eq!(report.exit_code(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.join("report.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(report);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_terminal_drop_marks_unfinished_cases_interrupted_not_passed() {
    let root = directory();
    let selected = select_cases(&["learn_courses".into()]).unwrap();
    {
        let mut report = ReportWriter::new(
            root.join("report.json"),
            "fixture_run".into(),
            &selected,
            false,
        )
        .unwrap();
        report.start("identity_session").unwrap();
    }
    let disk: CheckReport =
        serde_json::from_slice(&fs::read(root.join("report.json")).unwrap()).unwrap();
    assert!(
        disk.cases
            .values()
            .all(|c| c.status == CheckStatus::Interrupted)
    );
    assert!(!disk.session_reusable_after_exit);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_terminal_missing_login_blocks_dependencies_without_network_or_retries() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let selected = select_cases(&["library_seats".into()]).unwrap();
    let mut report = ReportWriter::new(
        root.join("report.json"),
        "fixture_run".into(),
        &selected,
        false,
    )
    .unwrap();
    let mut prompt = Prompt::default();
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    assert_eq!(prompt.secret_reads, 2);
    assert_eq!(
        report.report.cases["identity_session"].status,
        CheckStatus::Failed
    );
    assert!(
        report
            .report
            .cases
            .iter()
            .filter(|(k, _)| k.as_str() != "identity_session")
            .all(|(_, v)| v.status == CheckStatus::Blocked && v.attempts == 0)
    );
    assert!(server.requests().is_empty());
    assert_eq!(report.exit_code(), 1);
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    assert_eq!(prompt.secret_reads, 2);
    drop(report);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_terminal_factor_uses_only_advertised_method_and_never_auto_sends() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    pending(&mut runtime);
    let mut prompt = Prompt {
        factor: "totp".into(),
        ..Default::default()
    };
    assert!(
        complete_interactive_factor(&mut runtime, &mut prompt)
            .await
            .is_err()
    );
    assert_eq!(prompt.secret_reads, 0);
    assert_eq!(prompt.confirmations, 0);
    assert!(server.requests().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_terminal_cancelled_factor_has_no_send_or_verification_request() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    pending(&mut runtime);
    let mut prompt = Prompt::default();
    assert!(
        complete_interactive_factor(&mut runtime, &mut prompt)
            .await
            .is_err()
    );
    assert_eq!(prompt.confirmations, 1);
    assert_eq!(prompt.secret_reads, 1);
    assert!(server.requests().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_terminal_explicit_factor_send_once_and_rejection_never_retries() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"result":"success"}"#),
        Reply::json(r#"{"result":"fail"}"#),
    ]);
    let mut runtime = fixture_runtime(&server, &root);
    pending(&mut runtime);
    let mut prompt = Prompt {
        send: true,
        secrets: VecDeque::from(["123456".into()]),
        ..Default::default()
    };
    assert!(
        complete_interactive_factor(&mut runtime, &mut prompt)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
    assert_eq!(prompt.secret_reads, 1);
    assert_eq!(prompt.confirmations, 1);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_terminal_optional_usereg_skip_does_not_read_any_credentials() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let selected = select_cases(&["usereg_devices".into()]).unwrap();
    let mut report = ReportWriter::new(
        root.join("report.json"),
        "fixture_run".into(),
        &selected,
        false,
    )
    .unwrap();
    report.start("identity_session").unwrap();
    report
        .end(
            "identity_session",
            CheckStatus::Passed,
            "verified",
            None,
            0,
            0,
        )
        .unwrap();
    let mut prompt = Prompt::default();
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    assert_eq!(prompt.secret_reads, 0);
    assert_eq!(
        report.report.cases["usereg_devices"].status,
        CheckStatus::Unverified
    );
    assert!(server.requests().is_empty());
    drop(report);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_business_library_sample_descends_library_floor_and_today_section() {
    let root = directory();
    let id = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&id, &root);
    for service in [ServiceId::Identity, ServiceId::Info, ServiceId::Library] {
        runtime.coordinator.begin_authentication(service).unwrap();
        runtime
            .coordinator
            .mark_authenticated(
                service,
                UserIdentity {
                    username: "fixture-user".into(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .unwrap();
    }
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"data":{"list":[{"id":10,"name":"Fixture Library","isValid":1}]}}"#),
        Reply::json(
            r#"{"data":{"list":{"id":10,"childArea":[{"id":20,"name":"Fixture Floor","isValid":1}]}}}"#,
        ),
        Reply::json(
            r#"{"data":{"list":{"id":20,"childArea":[{"id":30,"name":"Fixture Section","isValid":1,"TotalCount":10,"UnavailableSpace":0}]}}}"#,
        ),
    ]);
    runtime.library_adapter = Some(
        LibraryReadAdapter::try_with_transport(
            Url::parse(server.base()).unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    execute_case(
        &mut runtime,
        &mut prompt,
        &mut evidence,
        "library_area_tree",
        false,
    )
    .await
    .unwrap();
    assert_eq!(evidence.area, Some(30));
    assert_eq!(server.requests().len(), 3);
    assert!(server.requests()[1].starts_with("GET /api.php/areas/10 "));
    assert!(server.requests()[2].starts_with("GET /api.php/areas/20/date/"));
    assert!(id.requests().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_business_library_day_request_uses_resolved_section_not_library() {
    let root = directory();
    let id = FixtureServer::new(vec![]);
    let mut r = fixture_runtime(&id, &root);
    for service in [ServiceId::Identity, ServiceId::Library] {
        r.coordinator.begin_authentication(service).unwrap();
        r.coordinator
            .mark_authenticated(
                service,
                UserIdentity {
                    username: "fixture-user".into(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .unwrap();
    }
    let day = campus_date_at(Utc::now()).to_string();
    let server=FixtureServer::new(vec![
        Reply::json(r#"{"data":{"list":[{"id":10,"name":"Library","isValid":1}]}}"#),
        Reply::json(r#"{"data":{"list":{"id":10,"childArea":[{"id":20,"name":"Floor","isValid":1}]}}}"#),
        Reply::json(r#"{"data":{"list":{"id":20,"childArea":[{"id":30,"name":"Section","isValid":1}]}}}"#),
        Reply::json(&serde_json::json!({"data":{"list":[{"id":400,"day":day,"startTime":{"date":format!("{day} 08:00:00.000000")},"endTime":{"date":format!("{day} 22:00:00.000000")}}]}}).to_string()),
    ]);
    r.library_adapter = Some(
        LibraryReadAdapter::try_with_transport(
            Url::parse(server.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    execute_case(
        &mut r,
        &mut prompt,
        &mut evidence,
        "library_area_tree",
        false,
    )
    .await
    .unwrap();
    execute_case(
        &mut r,
        &mut prompt,
        &mut evidence,
        "library_day_segments",
        false,
    )
    .await
    .unwrap();
    assert_eq!(evidence.area, Some(30));
    assert_eq!(evidence.segment.unwrap().id, 400);
    assert_eq!(server.requests().len(), 4);
    assert!(server.requests()[3].starts_with("GET /api.php/areadays/30 "));
    assert!(id.requests().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_run072714_terminal_discovered_section_enters_runtime_allowlist() {
    let root = directory();
    let id = FixtureServer::new(vec![]);
    let mut r = fixture_runtime(&id, &root);
    for service in [ServiceId::Identity, ServiceId::Library] {
        r.coordinator.begin_authentication(service).unwrap();
        r.coordinator
            .mark_authenticated(
                service,
                UserIdentity {
                    username: "fixture-user".into(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .unwrap();
    }
    let day = campus_date_at(Utc::now()).to_string();
    let server=FixtureServer::new(vec![
        Reply::json(r#"{"data":{"list":[{"id":10,"name":"Library","isValid":1}]}}"#),
        Reply::json(r#"{"data":{"list":{"id":10,"childArea":[{"id":20,"name":"Floor","isValid":1}]}}}"#),
        Reply::json(r#"{"data":{"list":{"id":20,"childArea":[{"id":30,"name":"Section","isValid":1}]}}}"#),
        Reply::json(&serde_json::json!({"data":{"list":[{"id":400,"day":day,"startTime":{"date":format!("{day} 00:00:00")},"endTime":{"date":format!("{day} 23:59:00")}}]}}).to_string()),
    ]);
    r.library_adapter = Some(
        LibraryReadAdapter::try_with_transport(
            Url::parse(server.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    execute_case(
        &mut r,
        &mut prompt,
        &mut evidence,
        "library_area_tree",
        false,
    )
    .await
    .unwrap();
    assert!(
        r.library_section_ids.contains(&30),
        "selected section must retain verified directory provenance"
    );
    assert!(!r.library_section_ids.contains(&999));
    execute_case(
        &mut r,
        &mut prompt,
        &mut evidence,
        "library_day_segments",
        false,
    )
    .await
    .unwrap();
    assert_eq!(evidence.segment.as_ref().map(|s| s.id), Some(400));
    assert_eq!(server.requests().len(), 4);
    assert!(server.requests()[3].starts_with("GET /api.php/areadays/30 "));
    assert!(id.requests().is_empty());
    fs::remove_dir_all(root).unwrap();
}

fn run072714_card_password_form() -> Reply {
    use sm2::elliptic_curve::sec1::ToSec1Point;
    let key: String = sm2::SecretKey::from_slice(&[1u8; 32])
        .unwrap()
        .public_key()
        .to_sec1_point(false)
        .as_bytes()[1..]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Reply::html(&format!(
        "<span id='sm2publicKey'>{key}</span><form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'></form>"
    ))
}
fn run072714_card_runtime(id: &FixtureServer, card: &FixtureServer, root: &Path) -> CampusRuntime {
    let mut r = fixture_runtime(id, root);
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(
            ServiceId::Identity,
            UserIdentity {
                username: "fixture-user".into(),
                display_name: None,
            },
            None,
            None,
            None,
        )
        .unwrap();
    r.card_client = Some(
        CampusCardClient::new(
            CampusCardAdapterConfig::new(card.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    assert!(r.terminal_interactive_auth);
    assert!(r.primary_password.is_none());
    assert!(!r.remember_credentials);
    r
}
#[tokio::test]
async fn backend_repair_run072714_terminal_card_prompts_once_after_current_password_form() {
    let root = directory();
    let card = FixtureServer::new(vec![
        Reply::json(r#"{"success":false,"resultData":null}"#),
        Reply::html("callback"),
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#),
    ]);
    let id = FixtureServer::new(vec![
        run072714_card_password_form(),
        Reply {
            status: 302,
            headers: format!("Location: {}handoff\r\n", card.base()),
            body: String::new(),
        },
    ]);
    let mut r = run072714_card_runtime(&id, &card, &root);
    let mut prompt = Prompt {
        secrets: VecDeque::from(["synthetic-card-password".into()]),
        send: true,
        ..Default::default()
    };
    execute_case(
        &mut r,
        &mut prompt,
        &mut Evidence::default(),
        "campus_card_session",
        false,
    )
    .await
    .unwrap();
    assert_eq!(prompt.confirmations, 1);
    assert_eq!(prompt.secret_reads, 1);
    assert!(r.primary_password.is_none());
    assert!(r.pending_card_password.is_none());
    assert!(!r.credentials_persisted);
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert_eq!(id.requests().len(), 2);
    assert!(id.requests()[0].starts_with("GET "));
    assert!(id.requests()[1].starts_with("POST "));
    assert!(
        id.requests()
            .iter()
            .all(|q| !q.contains("synthetic-card-password"))
    );
    assert_eq!(card.requests().len(), 3);
    assert!(
        r.complete_terminal_card_password("never-repeat".into())
            .await
            .is_err()
    );
    assert_eq!(id.requests().len(), 2);
    fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn backend_repair_run072714_terminal_card_decline_does_not_submit_or_reprompt() {
    let root = directory();
    let card = FixtureServer::new(vec![Reply::json(r#"{"success":false,"resultData":null}"#)]);
    let id = FixtureServer::new(vec![run072714_card_password_form()]);
    let mut r = run072714_card_runtime(&id, &card, &root);
    let mut prompt = Prompt::default();
    let error = execute_case(
        &mut r,
        &mut prompt,
        &mut Evidence::default(),
        "campus_card_session",
        false,
    )
    .await
    .err()
    .expect("declined boundary");
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "campus_card_password_declined"
    );
    assert_eq!(prompt.confirmations, 1);
    assert_eq!(prompt.secret_reads, 0);
    assert!(r.pending_card_password.is_none());
    assert_eq!(id.requests().len(), 1);
    assert_eq!(card.requests().len(), 1);
    assert!(r.service_session_is_proven(ServiceId::Identity));
    fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn backend_repair_run072714_terminal_card_transport_failure_never_prompts_password() {
    let root = directory();
    let id = FixtureServer::new(vec![]);
    let card = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut r = run072714_card_runtime(&id, &card, &root);
    let mut prompt = Prompt {
        send: true,
        ..Default::default()
    };
    assert!(
        execute_case(
            &mut r,
            &mut prompt,
            &mut Evidence::default(),
            "campus_card_session",
            false
        )
        .await
        .is_err()
    );
    assert_eq!(prompt.confirmations, 0);
    assert_eq!(prompt.secret_reads, 0);
    assert!(id.requests().is_empty());
    assert!(r.service_session_is_proven(ServiceId::Identity));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn backend_repair_run072714_terminal_seat_sample_uses_current_campus_time() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-21T07:30:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let rows = vec![crate::library_read::LibraryDaySegmentDto {
        id: 42,
        day: "2026-09-21".into(),
        start_time: "08:00".into(),
        end_time: "22:00".into(),
    }];
    let selected = select_open_library_segment(&rows, now).unwrap();
    assert_eq!(selected.start_time, "15:30");
    assert_eq!(selected.end_time, "22:00");
    assert!(select_open_library_segment(&rows, now + chrono::Duration::hours(8)).is_none());
}

fn run113847_learn_runtime(server: &FixtureServer, root: &Path) -> CampusRuntime {
    let mut r = fixture_runtime(server, root);
    for service in [ServiceId::Identity, ServiceId::Learn] {
        r.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Learn).then(|| {
            r.coordinator.registry().bind_csrf(
                service,
                crate::protocol::CsrfToken::new("fixture-csrf").unwrap(),
            )
        });
        r.coordinator
            .mark_authenticated(
                service,
                UserIdentity {
                    username: "fixture-user".into(),
                    display_name: None,
                },
                None,
                csrf,
                None,
            )
            .unwrap();
    }
    r.stage_detected = true;
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
    r.learn_source = Some(
        r.build_learn_source(
            learn,
            registrar,
            r.coordinator.bound_csrf(ServiceId::Learn).unwrap(),
        )
        .unwrap(),
    );
    r
}
fn run113847_cache(r: &CampusRuntime) -> LearnCourseListCachePayload {
    LearnCourseListCachePayload {
        account_scope: cache_account_scope("fixture-user"),
        semester: r.semester.clone(),
        stage: r.stage,
        generated_at: Utc::now(),
        courses: vec![LearnCourseDto {
            course_id: "course-one".into(),
            course_code: Some("C001".into()),
            title: "Fixture".into(),
            instructor: None,
            semester: Some(r.semester.clone()),
        }],
    }
}
#[test]
fn backend_repair_run113847_terminal_device_and_business_cache_have_distinct_lifetimes() {
    let root = directory();
    let path = root
        .join("device/unused-cache.json")
        .to_string_lossy()
        .into_owned();
    let first =
        create_backend_validation_runtime("2026-2027-1".into(), false, path.clone()).unwrap();
    let second = create_backend_validation_runtime("2026-2027-1".into(), false, path).unwrap();
    assert_eq!(first.fingerprint, second.fingerprint);
    assert_ne!(
        first.cache_path, second.cache_path,
        "fresh real-validation runs must never import previous business cache"
    );
    assert_ne!(
        first.persistence_root, second.persistence_root,
        "opt-out must not touch App credentials"
    );
    assert!(!first.persist_sessions && !second.persist_sessions);
    fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn backend_repair_run113847_course_evidence_survives_intervening_cache_projection() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"Fixture"}]}"#,
        ),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
    ]);
    let mut r = run113847_learn_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    execute_case(&mut r, &mut prompt, &mut evidence, "learn_courses", false)
        .await
        .unwrap();
    // A page may adopt cached display data between dependent checks. This
    // deliberately clears the runtime's raw-record scratch field today.
    r.adopt_learn_course_cache_payload(&run113847_cache(&r));
    let result = execute_case(&mut r, &mut prompt, &mut evidence, "learn_todos", false).await;
    assert!(
        result.is_ok(),
        "TODOs must use the captured verified course selection, not mutable scratch records"
    );
    assert_eq!(server.requests().len(), 4);
    assert!(
        server.requests()[1..]
            .iter()
            .all(|request| request.starts_with("POST ") && request.contains("course-one"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_run113847_course_validation_bypasses_stale_display_cache() {
    let root = directory();
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","resultList":[{"wlkcid":"course-live","kch":"CLIVE","kcm":"Live fixture"}]}"#,
    )]);
    let mut r = run113847_learn_runtime(&server, &root);
    let payload = run113847_cache(&r);
    let path = learn_course_list_cache_path(&r.cache_path, "fixture-user", &r.semester, r.stage);
    JsonFileCache::<LearnCourseListCachePayload>::new(
        path,
        LEARN_CACHE_SCHEMA_VERSION,
        LEARN_COURSE_LIST_CACHE_SERVICE,
    )
    .write(&payload)
    .unwrap();
    let display = r.load_learn_courses().await.unwrap();
    assert_eq!(display.source, "cache");
    assert!(server.requests().is_empty());
    let mut evidence = Evidence::default();
    execute_case(
        &mut r,
        &mut Prompt::default(),
        &mut evidence,
        "learn_courses",
        false,
    )
    .await
    .unwrap();
    assert_eq!(evidence.courses, vec!["course-live"]);
    assert_eq!(server.requests().len(), 1);
    fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn backend_repair_run113847_course_evidence_is_bound_to_account_semester_and_session() {
    for change in ["account", "semester", "session"] {
        let root = directory();
        let server = FixtureServer::new(vec![Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"Fixture"}]}"#,
        )]);
        let mut r = run113847_learn_runtime(&server, &root);
        let evidence = r.capture_validation_learn_courses().await.unwrap();
        match change {
            "account" => {
                r.coordinator.logout(ServiceId::Identity).unwrap();
                r.coordinator
                    .begin_authentication(ServiceId::Identity)
                    .unwrap();
                r.coordinator
                    .mark_authenticated(
                        ServiceId::Identity,
                        UserIdentity {
                            username: "other-fixture".into(),
                            display_name: None,
                        },
                        None,
                        None,
                        None,
                    )
                    .unwrap();
            }
            "semester" => r.semester = "2026-2027-2".into(),
            _ => {
                r.identity
                    .reset_transport("THYou/fixture-new-session")
                    .unwrap();
            }
        }
        let error = r
            .read_validation_learn_todos(&evidence, true)
            .await
            .unwrap_err();
        assert_eq!(error, "course_evidence_context_mismatch");
        assert_eq!(server.requests().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
#[tokio::test]
async fn backend_repair_run113847_verified_empty_course_list_skips_sample_without_failure() {
    let root = directory();
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","resultList":[]}"#,
    )]);
    let mut r = run113847_learn_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    execute_case(&mut r, &mut prompt, &mut evidence, "learn_courses", false)
        .await
        .unwrap();
    r.learn_course_records = None;
    assert!(matches!(
        execute_case(&mut r, &mut prompt, &mut evidence, "learn_todos", false)
            .await
            .unwrap(),
        Outcome::Skipped("no_course_selector")
    ));
    assert_eq!(server.requests().len(), 1);
    fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn backend_repair_run113847_todo_provider_failure_cannot_become_empty_success() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"Fixture"}]}"#,
        ),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::json(r#"{"result":"error","message":"synthetic provider unavailable"}"#),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
    ]);
    let mut r = run113847_learn_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    execute_case(&mut r, &mut prompt, &mut evidence, "learn_courses", false)
        .await
        .unwrap();
    assert!(
        execute_case(&mut r, &mut prompt, &mut evidence, "learn_todos", false)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 4);
    assert!(r.service_session_is_proven(ServiceId::Identity));
    fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn backend_repair_run113847_todo_expiry_stops_remaining_buckets_and_invalidates_learn() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"Fixture"}]}"#,
        ),
        Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let mut r = run113847_learn_runtime(&server, &root);
    let evidence = r.capture_validation_learn_courses().await.unwrap();
    assert!(
        r.read_validation_learn_todos(&evidence, true)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
    assert!(!r.service_session_is_proven(ServiceId::Learn));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn backend_repair_run113847_validation_workspace_cleanup_preserves_device_and_other_runs() {
    let root = directory();
    let device = root
        .join("device/unused-cache.json")
        .to_string_lossy()
        .into_owned();
    let first =
        create_backend_validation_runtime("2026-2027-1".into(), false, device.clone()).unwrap();
    let first_root = first.persistence_root.clone();
    let second =
        create_backend_validation_runtime("2026-2027-1".into(), false, device.clone()).unwrap();
    let second_root = second.persistence_root.clone();
    fs::write(first_root.join("fixture-data"), "synthetic").unwrap();
    drop(first);
    assert!(!first_root.exists());
    assert!(second_root.exists());
    let third = create_backend_validation_runtime("2026-2027-1".into(), false, device).unwrap();
    assert_eq!(second.fingerprint, third.fingerprint);
    drop(second);
    drop(third);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn backend_repair_run113847_old_device_business_cache_is_not_read_or_removed() {
    let root = directory();
    let parent = root.join("device");
    crate::telemetry::private_dir(&parent).unwrap();
    let old_cache = parent.join("unused-cache-old-data.json");
    fs::write(&old_cache, "synthetic preserved business cache").unwrap();
    let runtime = create_backend_validation_runtime(
        "2026-2027-1".into(),
        false,
        parent
            .join("unused-cache.json")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    assert_ne!(runtime.cache_path.parent().unwrap(), parent);
    drop(runtime);
    assert_eq!(
        fs::read_to_string(old_cache).unwrap(),
        "synthetic preserved business cache"
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn backend_repair_run113847_cached_or_partial_results_never_count_as_live_validation() {
    assert!(validation_scope::require_live_validation_result("live", "ready").is_ok());
    for (source, status) in [
        ("cache", "ready"),
        ("cache", "stale"),
        ("live", "partial"),
        ("", "ready"),
    ] {
        assert_eq!(
            validation_scope::require_live_validation_result(source, status).unwrap_err(),
            "validation_live_result_required"
        );
    }
}

#[tokio::test]
async fn backend_repair_run113847_terminal_logout_cannot_clear_application_saved_credentials() {
    // This test refuses to run without the fixture runner's isolated root.
    let application =
        PathBuf::from(std::env::var("THYOU_SESSION_DIR").expect("isolated test root required"));
    assert_eq!(
        crate::session_persistence::application_data_dir().unwrap(),
        application
    );
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut r = run113847_learn_runtime(&server, &root);
    crate::credential_store::save_at_root_with_stage_selection(
        &application,
        "fixture-user",
        "synthetic-only-password",
        Some(AcademicStage::Undergraduate),
        &r.fingerprint,
        false,
    )
    .unwrap();
    let device = r.fingerprint.clone();
    r.logout().unwrap();
    assert!(
        crate::credential_store::load_at_root(&application, "fixture-user", &device)
            .unwrap()
            .is_some()
    );
    assert!(server.requests().is_empty());
    drop(r);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_run113847_debug_todos_only_acquires_fresh_evidence() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"Fixture"}]}"#,
        ),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
    ]);
    let mut r = run113847_learn_runtime(&server, &root);
    r.portal_bootstrapped = true;
    let mut ledger = crate::live_validation::load_at(root.join("debug-results.json")).unwrap();
    ledger.record("learn_courses", Ok(()));
    let prior = ledger.cases["learn_courses"].clone();
    ledger.select_cases("learn_todos").unwrap();
    let user = r
        .coordinator
        .registry()
        .snapshot_for(ServiceId::Identity)
        .user
        .unwrap();
    r.run_debug_live_cases(&user, &mut ledger).await;
    assert_eq!(ledger.cases["learn_todos"].status, "passed");
    assert_eq!(
        ledger.cases["learn_courses"], prior,
        "dependency refresh must not rerun or rewrite the historical case"
    );
    assert_eq!(server.requests().len(), 4);
    assert!(server.requests()[0].starts_with("GET "));
    assert!(
        server.requests()[1..]
            .iter()
            .all(|request| request.starts_with("POST ") && request.contains("course-one"))
    );
    drop(r);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_run113847_live_validation_rejects_dropped_course_records() {
    let root = directory();
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"Fixture"},{"wlkcid":"course-two","kch":"C002"}]}"#,
    )]);
    let mut r = run113847_learn_runtime(&server, &root);
    assert!(
        r.capture_validation_learn_courses().await.is_err(),
        "partial mapping must not count as a verified complete course list"
    );
    assert_eq!(server.requests().len(), 1);
    drop(r);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_run113847_debug_course_failure_does_not_trigger_second_read() {
    let root = directory();
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut r = run113847_learn_runtime(&server, &root);
    r.portal_bootstrapped = true;
    let user = r
        .coordinator
        .registry()
        .snapshot_for(ServiceId::Identity)
        .user
        .unwrap();
    let mut ledger = crate::live_validation::load_at(root.join("debug-results.json")).unwrap();
    ledger.select_cases("learn_courses,learn_todos").unwrap();
    r.run_debug_live_cases(&user, &mut ledger).await;
    assert_eq!(ledger.cases["learn_courses"].status, "failed");
    assert_ne!(ledger.cases["learn_todos"].status, "passed");
    assert_eq!(
        server.requests().len(),
        1,
        "the failed dependency must not be fetched again for the homework case"
    );
    assert!(r.service_session_is_proven(ServiceId::Identity));
    drop(r);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn backend_repair_run113847_workspace_drop_does_not_follow_replaced_symlink() {
    use std::os::unix::fs::symlink;
    let root = directory();
    let device = root
        .join("device/unused-cache.json")
        .to_string_lossy()
        .into_owned();
    let r = create_backend_validation_runtime("2026-2027-1".into(), false, device).unwrap();
    let owned = r.persistence_root.clone();
    let parked = root.join("parked-owned-directory");
    let other = root.join("unrelated-data");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("sentinel"), "synthetic unrelated content").unwrap();
    fs::rename(&owned, &parked).unwrap();
    symlink(&other, &owned).unwrap();
    drop(r);
    assert_eq!(
        fs::read_to_string(other.join("sentinel")).unwrap(),
        "synthetic unrelated content"
    );
    assert!(
        fs::symlink_metadata(&owned)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(parked.is_dir());
    fs::remove_file(owned).unwrap();
    fs::remove_dir_all(root).unwrap();
}
