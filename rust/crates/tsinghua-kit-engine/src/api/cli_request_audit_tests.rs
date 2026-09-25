use super::*;

#[test]
fn backend_repair_request_audit_overview_input_case_requires_real_evidence_not_only_a_passed_label()
{
    let root = std::env::temp_dir().join(format!("thyou-input-audit-{}", uuid::Uuid::new_v4()));
    let runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".into(),
        false,
        root.join("cache.json").to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    let empty = Evidence::default();
    assert!(validate_overview_inputs(&runtime, &empty).is_err());
    let fabricated_counts = Evidence {
        courses: vec!["fixture".into()],
        todo_sample_count: Some(0),
        schedule_input_verified: true,
        ..Default::default()
    };
    assert!(validate_overview_inputs(&runtime, &fabricated_counts).is_err());
    drop(runtime);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_request_audit_checkpoint_cost_is_measured_without_inflating_finished_wall_time() {
    let root = std::env::temp_dir().join(format!(
        "tsinghua-kit-checkpoint-audit-{}",
        uuid::Uuid::new_v4()
    ));
    let selected = select_cases(&["identity_session".into()]).unwrap();
    let mut report =
        ReportWriter::new(root.join("report.json"), "fixture".into(), &selected, false).unwrap();
    report.report.execution = Some(ExecutionSummary::new(ExecutionOptions::default()));
    report.start("identity_session").unwrap();
    report
        .end(
            "identity_session",
            CheckStatus::Failed,
            "user_cancelled_factor",
            None,
            0,
            0,
        )
        .unwrap();
    let checkpoints_before_completion = report.checkpoint_totals.get();
    report.finish().unwrap();
    let summary = report.report.execution.as_ref().unwrap();
    assert_eq!(
        summary.checkpoint_write_count,
        checkpoints_before_completion.0
    );
    assert_eq!(summary.checkpoint_write_us, checkpoints_before_completion.1);
    assert!(summary.checkpoint_write_count >= 3);
    assert!(summary.total_wall_us >= summary.checkpoint_write_us);
    let wall = summary.total_wall_us;
    let io = summary.checkpoint_write_us;
    report.report.logging_complete = Some(true);
    report.finish().unwrap();
    let summary = report.report.execution.as_ref().unwrap();
    assert_eq!(summary.total_wall_us, wall);
    assert_eq!(summary.checkpoint_write_us, io);
    let disk: CheckReport =
        serde_json::from_slice(&std::fs::read(root.join("report.json")).unwrap()).unwrap();
    assert_eq!(disk.execution.unwrap().checkpoint_write_us, io);
    assert_eq!(disk.cases["identity_session"].status, CheckStatus::Failed);
    drop(report);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_request_audit_old_reports_without_checkpoint_fields_still_decode() {
    let summary = ExecutionSummary::new(ExecutionOptions::default());
    let mut value = serde_json::to_value(summary).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("checkpoint_write_count");
    value.as_object_mut().unwrap().remove("checkpoint_write_us");
    let old: ExecutionSummary = serde_json::from_value(value).unwrap();
    assert_eq!(old.checkpoint_write_count, 0);
    assert_eq!(old.checkpoint_write_us, 0);
}
