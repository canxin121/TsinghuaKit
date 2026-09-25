use super::*;

#[tokio::test]
async fn backend_repair_coverage_scheduler_preserves_environment_and_login_gaps() {
    struct NoPrompt;
    impl UserPrompts for NoPrompt {
        fn secret(&mut self, _: &'static str) -> Result<String, String> {
            panic!("no credentials expected")
        }
        fn confirm(&mut self, _: &'static str) -> Result<bool, String> {
            panic!("no confirmation expected")
        }
        fn choose_factor(&mut self, _: &[String]) -> Result<String, String> {
            panic!("no factor expected")
        }
        fn show_captcha(&mut self, _: &[u8], _: &str) -> Result<(), String> {
            panic!("no captcha expected")
        }
        fn clear_captcha(&mut self) {}
        fn progress(&mut self, _: &CheckSpec, _: CheckStatus, _: &str) {}
    }
    let (root, mut report) = fixture_report(&[
        "tunet_status".into(),
        "usereg_account".into(),
        "usereg_balance".into(),
        "usereg_devices".into(),
    ]);
    let mut runtime = create_backend_validation_runtime(
        "auto".into(),
        true,
        root.join("device/cache.json")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    report
        .set_network_environment(NetworkEnvironment::OffCampus)
        .unwrap();
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
    run(&mut runtime, &mut NoPrompt, &mut report, false)
        .await
        .unwrap();
    assert_eq!(report.exit_code(), 3);
    assert_eq!(report.report.request_count, 0);
    for (id, row) in &report.report.cases {
        if id == "identity_session" {
            continue;
        }
        assert_eq!(row.status, CheckStatus::Unverified);
        assert_eq!(
            row.reason,
            if id == "tunet_status" {
                "off_campus_network_unverified"
            } else {
                "optional_login_not_selected"
            }
        );
        assert_eq!(row.attempts, 0);
        assert_eq!(row.requests, 0);
    }
    assert_eq!(
        retry_plan(&root.join("report.json")).unwrap().0,
        report.report.cases.keys().cloned().collect()
    );
    drop(report);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

fn fixture_report(selected: &[String]) -> (PathBuf, ReportWriter) {
    let root = std::env::temp_dir().join(format!("thyou-coverage-{}", uuid::Uuid::new_v4()));
    let report = ReportWriter::new(
        root.join("report.json"),
        "fixture".into(),
        &select_cases(selected).unwrap(),
        true,
    )
    .unwrap();
    (root, report)
}

#[test]
fn backend_repair_coverage_legacy_skips_are_unfinished_and_not_success() {
    let root = std::env::temp_dir().join(format!("thyou-coverage-{}", uuid::Uuid::new_v4()));
    let selected = select_cases(&["registrar_exams".into(), "tunet_status".into()]).unwrap();
    let path = root.join("report.json");
    let mut report = ReportWriter::new(path.clone(), "fixture".into(), &selected, true).unwrap();
    for id in &selected {
        let (status, reason) = match id.as_str() {
            "registrar_exams" => (CheckStatus::Skipped, "graduate_exam_not_implemented"),
            "tunet_status" => (CheckStatus::Skipped, "off_campus_not_applicable"),
            _ => (CheckStatus::Passed, "verified"),
        };
        report.end(id, status, reason, None, 0, 0).unwrap();
    }
    report.finish().unwrap();
    assert_eq!(
        report.exit_code(),
        3,
        "unverified work is not a complete acceptance"
    );
    let (retry, graduate) = retry_plan(&path).unwrap();
    assert!(graduate && retry.contains("registrar_exams") && retry.contains("tunet_status"));
    drop(report);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_coverage_explicit_usereg_adds_omitted_reads_without_repeating_passes() {
    let (root, mut previous) = fixture_report(&["campus_card_account".into()]);
    for row in previous.report.cases.values_mut() {
        row.status = CheckStatus::Passed;
    }
    previous.finish().unwrap();
    let path = root.join("report.json");
    let before = fs::read(&path).unwrap();
    let (selected, graduate) = retry_plan_including_usereg(&path, true).unwrap();
    assert!(graduate);
    assert_eq!(
        selected,
        select_cases(&[
            "usereg_account".into(),
            "usereg_balance".into(),
            "usereg_devices".into()
        ])
        .unwrap()
    );
    assert!(!selected.contains("campus_card_account"));
    assert_eq!(fs::read(path).unwrap(), before);
    drop(previous);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_coverage_omitted_catalog_and_previous_passes_remain_visible() {
    let (root, mut previous) = fixture_report(&["campus_card_account".into()]);
    previous.report.build_revision = "a".repeat(64);
    for row in previous.report.cases.values_mut() {
        row.status = CheckStatus::Passed;
    }
    previous.finish().unwrap();
    let selected = select_cases(&["registrar_exams".into()]).unwrap();
    let mut next =
        ReportWriter::new(root.join("next.json"), "next".into(), &selected, true).unwrap();
    next.set_previous_report(&root.join("report.json")).unwrap();
    assert_eq!(
        next.report.cases.len() + next.report.omitted_cases.len(),
        CHECKS.len()
    );
    assert_eq!(
        next.report.omitted_cases["campus_card_account"].reason,
        OmissionReason::PreviouslyPassed
    );
    assert_eq!(
        next.report.omitted_cases["usereg_account"].reason,
        OmissionReason::NotSelected
    );
    assert!(!next.report.cases.contains_key("campus_card_account"));
    let md = root.join("next.md");
    write_markdown(&next.report, &md).unwrap();
    let text = fs::read_to_string(md).unwrap();
    assert!(text.contains("历史通过，本轮未重跑") && text.contains("未选择，未验证"));
    drop(next);
    drop(previous);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_coverage_unverified_and_sample_gaps_keep_nonzero_exit() {
    for reason in [
        "no_course_selector",
        "no_article_selector",
        "no_area_selector",
        "no_open_segment",
        "no_building_selector",
        "optional_login_not_selected",
        "off_campus_network_unverified",
    ] {
        let (root, mut report) = fixture_report(&["tunet_status".into()]);
        report
            .end("tunet_status", CheckStatus::Unverified, reason, None, 0, 0)
            .unwrap();
        report.finish().unwrap();
        assert_eq!(report.exit_code(), 3);
        assert!(
            retry_plan(&root.join("report.json"))
                .unwrap()
                .0
                .contains("tunet_status")
        );
        drop(report);
        fs::remove_dir_all(root).unwrap();
    }
}
