use super::*;

#[derive(Default)]
struct Prompt {
    secret_reads: usize,
}

impl UserPrompts for Prompt {
    fn secret(&mut self, _: &'static str) -> Result<String, String> {
        self.secret_reads += 1;
        Ok(String::new())
    }
    fn confirm(&mut self, _: &'static str) -> Result<bool, String> {
        Ok(false)
    }
    fn choose_factor(&mut self, _: &[String]) -> Result<String, String> {
        Err("fixture_no_factor".into())
    }
    fn show_captcha(&mut self, _: &[u8], _: &str) -> Result<(), String> {
        Err("fixture_no_captcha".into())
    }
    fn clear_captcha(&mut self) {}
    fn progress(&mut self, _: &CheckSpec, _: CheckStatus, _: &str) {}
}

#[tokio::test]
async fn backend_repair_off_campus_scheduler_skips_only_tunet_before_dispatch() {
    let root = std::env::temp_dir().join(format!("thyou-off-campus-{}", uuid::Uuid::new_v4()));
    let mut runtime = create_backend_validation_runtime(
        "auto".into(),
        false,
        root.join("device/cache.json")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let selected = select_cases(&[
        "info_detail".into(),
        "campus_card_account".into(),
        "tunet_status".into(),
    ])
    .unwrap();
    let path = root.join("report.json");
    let mut report = ReportWriter::new(path.clone(), "fixture".into(), &selected, false).unwrap();
    report
        .set_network_environment(NetworkEnvironment::OffCampus)
        .unwrap();
    let mut prompt = Prompt::default();
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    assert_eq!(prompt.secret_reads, 2);
    let tunet = &report.report.cases["tunet_status"];
    assert_eq!(tunet.status, CheckStatus::Unverified);
    assert_eq!(tunet.reason, "off_campus_network_unverified");
    assert_eq!(tunet.attempts, 0);
    assert_eq!(tunet.requests, 0);
    assert!(tunet.timing.is_none());
    assert_eq!(report.report.request_count, 0);
    assert_eq!(
        report.exit_code(),
        1,
        "other failures must still fail acceptance"
    );
    for (id, row) in &report.report.cases {
        if id != "tunet_status" {
            assert!(matches!(
                row.status,
                CheckStatus::Failed | CheckStatus::Blocked
            ));
        }
    }
    let disk: CheckReport = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk.network_environment, NetworkEnvironment::OffCampus);
    let (retry, _) = retry_plan(&path).unwrap();
    assert!(retry.contains("tunet_status"));
    assert!(retry.contains("info_detail"));
    assert!(retry.contains("campus_card_account"));
    let markdown = root.join("report.md");
    write_markdown(&report.report, &markdown).unwrap();
    let text = fs::read_to_string(markdown).unwrap();
    assert!(text.contains("校外模式"));
    assert!(text.contains("未证明校园网在线或离线"));
    assert!(text.contains("unverified | off_campus_network_unverified"));
    drop(report);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_off_campus_cannot_reclassify_an_already_attempted_failure() {
    let root = std::env::temp_dir().join(format!("thyou-network-scope-{}", uuid::Uuid::new_v4()));
    let selected = select_cases(&["tunet_status".into()]).unwrap();
    let mut report =
        ReportWriter::new(root.join("report.json"), "fixture".into(), &selected, false).unwrap();
    report.start("tunet_status").unwrap();
    assert_eq!(
        report
            .set_network_environment(NetworkEnvironment::OffCampus)
            .unwrap_err(),
        "network_scope_requires_unstarted_plan"
    );
    report
        .end(
            "tunet_status",
            CheckStatus::Failed,
            "tunet_status_unconfirmed",
            None,
            20_000,
            1,
        )
        .unwrap();
    assert!(
        report
            .set_network_environment(NetworkEnvironment::OffCampus)
            .is_err()
    );
    assert_eq!(
        report.report.network_environment,
        NetworkEnvironment::Unspecified
    );
    assert_eq!(
        report.report.cases["tunet_status"].status,
        CheckStatus::Failed
    );
    assert_eq!(report.exit_code(), 1);
    report.finish().unwrap();
    drop(report);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_off_campus_is_explicit_and_never_exempts_webvpn_services() {
    for spec in CHECKS {
        assert!(
            NetworkEnvironment::Unspecified
                .skip_reason(spec.id)
                .is_none()
        );
        assert_eq!(
            NetworkEnvironment::OffCampus.skip_reason(spec.id).is_some(),
            spec.id == "tunet_status"
        );
    }
    let root =
        std::env::temp_dir().join(format!("thyou-old-network-report-{}", uuid::Uuid::new_v4()));
    let mut report = ReportWriter::new(
        root.join("report.json"),
        "fixture".into(),
        &select_cases(&[]).unwrap(),
        false,
    )
    .unwrap();
    let mut old = serde_json::to_value(&report.report).unwrap();
    old.as_object_mut().unwrap().remove("network_environment");
    let decoded: CheckReport = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(decoded.network_environment, NetworkEnvironment::Unspecified);
    old["network_environment"] = serde_json::json!("guess_from_timeout");
    assert!(serde_json::from_value::<CheckReport>(old).is_err());
    report.interrupted().unwrap();
    drop(report);
    fs::remove_dir_all(root).unwrap();
}
