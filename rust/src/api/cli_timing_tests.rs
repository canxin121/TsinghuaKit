use super::*;
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Prompt {
    reads: usize,
}
impl UserPrompts for Prompt {
    fn secret(&mut self, _: &'static str) -> Result<String, String> {
        self.reads += 1;
        std::thread::sleep(std::time::Duration::from_millis(3));
        Ok(String::new())
    }
    fn confirm(&mut self, _: &'static str) -> Result<bool, String> {
        Ok(false)
    }
    fn choose_factor(&mut self, _: &[String]) -> Result<String, String> {
        Err("fixture_no_challenge".into())
    }
    fn show_captcha(&mut self, _: &[u8], _: &str) -> Result<(), String> {
        Err("fixture_no_captcha".into())
    }
    fn clear_captcha(&mut self) {}
    fn progress(&mut self, _: &CheckSpec, _: CheckStatus, _: &str) {}
}

#[tokio::test]
async fn backend_repair_perf_scheduler_propagates_failed_dependencies_and_separates_user_time() {
    let root =
        std::env::temp_dir().join(format!("thyou-perf-dependencies-{}", uuid::Uuid::new_v4()));
    let mut runtime = create_backend_validation_runtime(
        "auto".into(),
        false,
        root.join("device/cache.json")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let selected = select_cases(&["library_seats".into(), "usereg_devices".into()]).unwrap();
    let mut report =
        ReportWriter::new(root.join("report.json"), "fixture".into(), &selected, false).unwrap();
    let mut prompt = Prompt::default();
    run_with_options(
        &mut runtime,
        &mut prompt,
        &mut report,
        false,
        ExecutionOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(prompt.reads, 2);
    let identity = &report.report.cases["identity_session"];
    assert_eq!(identity.status, CheckStatus::Failed);
    assert!(identity.timing.as_ref().unwrap().user_input_us >= 4000);
    assert_eq!(identity.requests, 0);
    for (id, row) in &report.report.cases {
        if id == "identity_session" {
            continue;
        }
        assert!(matches!(
            row.status,
            CheckStatus::Blocked | CheckStatus::Skipped
        ));
        assert_eq!(row.attempts, 0);
        assert!(row.timing.as_ref().is_none_or(|t| !t.measured));
    }
    assert_eq!(report.report.request_count, 0);
    assert_eq!(
        report
            .report
            .execution
            .as_ref()
            .unwrap()
            .max_runtime_executing,
        1
    );
    drop(runtime);
    drop(report);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn backend_repair_perf_actual_scheduler_admits_multiple_ready_cases_without_fake_http_parallelism()
 {
    let root = std::env::temp_dir().join(format!("thyou-perf-ready-{}", uuid::Uuid::new_v4()));
    let mut runtime = create_backend_validation_runtime(
        "auto".into(),
        false,
        root.join("device/cache.json")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let selected = select_cases(&["registrar_schedule".into(), "info_detail".into()]).unwrap();
    let mut report =
        ReportWriter::new(root.join("report.json"), "fixture".into(), &selected, false).unwrap();
    // Fixture-only prerequisites: no historic result or foreign proof is
    // imported in production. These two bodies fail/skip without network.
    for (id, row) in &mut report.report.cases {
        if !matches!(id.as_str(), "registrar_schedule" | "info_detail") {
            row.status = CheckStatus::Passed;
        }
    }
    let mut prompt = Prompt::default();
    run_with_options(
        &mut runtime,
        &mut prompt,
        &mut report,
        false,
        ExecutionOptions::default(),
    )
    .await
    .unwrap();
    let summary = report.report.execution.as_ref().unwrap();
    assert_eq!(summary.max_submitted, 2);
    assert_eq!(summary.max_runtime_executing, 1);
    assert_eq!(
        report.report.cases["registrar_schedule"].status,
        CheckStatus::Failed
    );
    assert_eq!(
        report.report.cases["info_detail"].status,
        CheckStatus::Skipped
    );
    assert!(
        report.report.cases["info_detail"]
            .timing
            .as_ref()
            .unwrap()
            .measured
    );
    assert_eq!(prompt.reads, 0);
    assert_eq!(report.report.request_count, 0);
    drop(runtime);
    drop(report);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_perf_cancelled_report_accounts_only_for_current_task_and_queued_rows() {
    use crate::telemetry::timing::{Collector, Phase};
    let root = std::env::temp_dir().join(format!("thyou-perf-cancel-{}", uuid::Uuid::new_v4()));
    let selected = select_cases(&["learn_courses".into()]).unwrap();
    let mut report =
        ReportWriter::new(root.join("report.json"), "fixture".into(), &selected, false).unwrap();
    report.report.execution = Some(ExecutionSummary::new(ExecutionOptions::default()));
    report.start("identity_session").unwrap();
    assert!(report.start("portal_bootstrap").is_err());
    report
        .report
        .cases
        .get_mut("identity_session")
        .unwrap()
        .timing = Some(CaseTiming::default());
    let collector = Collector::default();
    collector.update(|metrics| metrics.requests = 2);
    collector.phase(Phase::UserInput, std::time::Duration::from_micros(2));
    report.active_metrics = Some(collector);
    report.interrupted().unwrap();
    assert_eq!(report.report.request_count, 2);
    assert_eq!(report.report.cases["identity_session"].requests, 2);
    assert_eq!(report.report.cases["learn_courses"].attempts, 0);
    assert!(
        report
            .report
            .cases
            .values()
            .all(|row| row.status == CheckStatus::Interrupted)
    );
    drop(report);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_perf_app_mode_is_bounded_and_serial_comparison_is_explicit() {
    let default = ExecutionOptions::default().validate().unwrap();
    assert_eq!(default.mode, ExecutionMode::App);
    assert_eq!(default.concurrency, 4);
    for concurrency in [0, 9, usize::MAX] {
        assert!(
            ExecutionOptions {
                mode: ExecutionMode::App,
                concurrency
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        ExecutionOptions {
            mode: ExecutionMode::Serial,
            concurrency: 4
        }
        .validate()
        .is_err()
    );
    assert!(
        ExecutionOptions {
            mode: ExecutionMode::Serial,
            concurrency: 1
        }
        .validate()
        .is_ok()
    );
}

#[tokio::test]
async fn backend_repair_perf_app_fan_in_polls_concurrent_callers_but_serializes_mutable_runtime() {
    let runtime = tokio::sync::Mutex::new(());
    let entered = Rc::new(Cell::new(0));
    let finished = Rc::new(Cell::new(0));
    let active = Rc::new(Cell::new(0));
    let max_active = Rc::new(Cell::new(0));
    let max_pending = Rc::new(Cell::new(0));
    let jobs = (0..4)
        .map(|_| {
            let (entered, finished, active, max_active, max_pending) = (
                entered.clone(),
                finished.clone(),
                active.clone(),
                max_active.clone(),
                max_pending.clone(),
            );
            let runtime = &runtime;
            Box::pin(async move {
                entered.set(entered.get() + 1);
                max_pending.set(max_pending.get().max(entered.get() - finished.get()));
                let _guard = runtime.lock().await;
                active.set(active.get() + 1);
                max_active.set(max_active.get().max(active.get()));
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                active.set(active.get() - 1);
                finished.set(finished.get() + 1);
                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>
        })
        .collect();
    schedule::drive(jobs).await.unwrap();
    assert_eq!(max_pending.get(), 4);
    assert_eq!(max_active.get(), 1);
    assert_eq!(finished.get(), 4);
}

#[test]
fn backend_repair_perf_report_exports_timing_without_claiming_unmeasured_success() {
    let root = std::env::temp_dir().join(format!("thyou-perf-report-{}", uuid::Uuid::new_v4()));
    let selected = select_cases(&["identity_session".into()]).unwrap();
    let mut writer =
        ReportWriter::new(root.join("report.json"), "fixture".into(), &selected, false).unwrap();
    writer.report.execution = Some(ExecutionSummary::new(ExecutionOptions::default()));
    writer.start("identity_session").unwrap();
    writer
        .report
        .cases
        .get_mut("identity_session")
        .unwrap()
        .timing = Some(CaseTiming {
        ready_after_us: 0,
        queued_after_us: 10,
        started_after_us: 20,
        finished_after_us: 100,
        queue_wait_us: 10,
        execution_us: 80,
        user_input_us: 50,
        active_execution_us: 30,
        measured: true,
        ..Default::default()
    });
    writer
        .end(
            "identity_session",
            CheckStatus::Failed,
            "user_cancelled_factor",
            None,
            0,
            0,
        )
        .unwrap();
    writer.finish().unwrap();
    write_markdown(&writer.report, &root.join("report.md")).unwrap();
    write_timings_csv(&writer.report, &root.join("timings.csv")).unwrap();
    let json = std::fs::read_to_string(root.join("report.json")).unwrap();
    assert!(json.contains("user_input_us"));
    assert_eq!(writer.exit_code(), 1);
    assert_eq!(writer.report.execution.as_ref().unwrap().user_input_us, 50);
    assert!(
        writer
            .report
            .execution
            .as_ref()
            .unwrap()
            .first_completed_after_us
            .is_none()
    );
    assert!(writer.start("identity_session").is_err());
    drop(writer);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_perf_runtime_priorities_match_interactive_foreground_prefetch() {
    assert!(schedule::priority("identity_session") < schedule::priority("learn_courses"));
    assert!(schedule::priority("learn_courses") < schedule::priority("learn_todos"));
    assert_eq!(schedule::priority("library_session"), 2);
}
