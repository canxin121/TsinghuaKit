// Synthetic responses only. These tests share the existing isolated Runtime
// fixtures and never restore an App session or call a campus server.
fn graduate_exam_fixture_runtime(base: &Path, server: &FixtureServer) -> CampusRuntime {
    let mut runtime = CampusRuntime::new_auto_with_persistence(
        "2026-2027-1".into(),
        true,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    let user = test_user("2026000000");
    install_identity(&mut runtime, &user);
    let source = schedule_source(&runtime, server.base(), AcademicStage::Graduate);
    install_registrar_proof(&mut runtime, &user, source);
    runtime.semester_first_day = NaiveDate::from_ymd_opt(2026, 9, 1);
    runtime.semester_last_day = NaiveDate::from_ymd_opt(2026, 9, 2);
    runtime
}

const GRADUATE_EXAM_BODY: &str = r#"thyouCalendar([{"nr":"期末考试甲","nq":"2026-09-02","kssj":"09:00","jssj":"11:00","fl":"考试"}])"#;

#[tokio::test]
async fn backend_repair_graduate_exams_app_validation_rejects_cache_as_live_proof() {
    for cached in [false, true] {
        let server = FixtureServer::new(vec![Reply::json(GRADUATE_EXAM_BODY)]);
        let base = unique_cache_base("graduate-exam-app-validation");
        let mut runtime = graduate_exam_fixture_runtime(&base, &server);
        runtime.portal_bootstrapped = true;
        if cached {
            runtime
                .load_exams(String::new(), String::new())
                .await
                .unwrap();
        }
        let mut ledger =
            crate::live_validation::load_at(base.with_file_name("debug-results.json")).unwrap();
        ledger.select_cases("registrar_exams").unwrap();
        runtime
            .run_debug_live_cases(&test_user("2026000000"), &mut ledger)
            .await;
        let result = &ledger.cases["registrar_exams"];
        assert_eq!(result.status, if cached { "failed" } else { "passed" });
        if cached {
            assert_eq!(
                result.reason.as_deref(),
                Some("validation_live_result_required")
            );
        }
        assert_eq!(result.attempts, 1);
        assert_eq!(server.requests().len(), 1);
        drop(runtime);
        remove_cache_file(base);
    }
}

#[tokio::test]
async fn backend_repair_graduate_exams_do_not_mix_source_semesters_or_truncate_precision() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"thyouCalendar([{"nr":"精确考试时间","nq":"2026-09-02","kssj":"09:00:30","jssj":"11:00","fl":"考试"}])"#,
    )]);
    let base = unique_cache_base("graduate-exam-semantics");
    let mut runtime = graduate_exam_fixture_runtime(&base, &server);
    runtime.semester = "2026-2027-2".into();
    assert!(
        runtime
            .load_exams(String::new(), String::new())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    runtime.semester = "2026-2027-1".into();
    assert!(
        runtime
            .load_exams(String::new(), String::new())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_graduate_exams_cli_checks_live_results_instead_of_skipping() {
    use super::cli_validation as cli;
    struct NoPrompt;
    impl cli::UserPrompts for NoPrompt {
        fn secret(&mut self, _: &'static str) -> Result<String, String> {
            panic!("unexpected credentials")
        }
        fn confirm(&mut self, _: &'static str) -> Result<bool, String> {
            panic!("unexpected confirmation")
        }
        fn choose_factor(&mut self, _: &[String]) -> Result<String, String> {
            panic!("unexpected factor")
        }
        fn show_captcha(&mut self, _: &[u8], _: &str) -> Result<(), String> {
            panic!("unexpected captcha")
        }
        fn clear_captcha(&mut self) {}
        fn progress(&mut self, _: &cli::CheckSpec, _: cli::CheckStatus, _: &str) {}
    }
    for cached in [false, true] {
        let server = FixtureServer::new(vec![Reply::json(GRADUATE_EXAM_BODY)]);
        let base = unique_cache_base("graduate-exam-cli");
        let mut runtime = super::create_backend_validation_runtime(
            "2026-2027-1".into(),
            true,
            base.to_string_lossy().into_owned(),
        )
        .unwrap();
        let user = test_user("2026000000");
        install_identity(&mut runtime, &user);
        let source = schedule_source(&runtime, server.base(), AcademicStage::Graduate);
        install_registrar_proof(&mut runtime, &user, source);
        runtime.semester_first_day = NaiveDate::from_ymd_opt(2026, 9, 1);
        runtime.semester_last_day = NaiveDate::from_ymd_opt(2026, 9, 2);
        if cached {
            runtime
                .load_exams(String::new(), String::new())
                .await
                .unwrap();
        }
        let selected = cli::select_cases(&["registrar_exams".into()]).unwrap();
        let mut report = cli::ReportWriter::new(
            base.with_file_name("report.json"),
            "fixture".into(),
            &selected,
            true,
        )
        .unwrap();
        for id in selected
            .iter()
            .filter(|id| id.as_str() != "registrar_exams")
        {
            let row = report.report.cases.get_mut(id).unwrap();
            row.status = cli::CheckStatus::Passed;
            row.reason = "verified".into();
        }
        cli::run(&mut runtime, &mut NoPrompt, &mut report, false)
            .await
            .unwrap();
        assert_eq!(
            report.report.cases["registrar_exams"].status,
            if cached {
                cli::CheckStatus::Failed
            } else {
                cli::CheckStatus::Passed
            }
        );
        assert_eq!(report.report.cases["registrar_exams"].attempts, 1);
        assert_eq!(server.requests().len(), 1);
        drop(report);
        drop(runtime);
        remove_cache_file(base);
    }
}

#[tokio::test]
async fn backend_repair_graduate_exams_read_explicit_calendar_events() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"thyouCalendar([
          {"nr":"研究生课程甲","nq":"2026-09-01","kssj":"08：00","jssj":"09：00","fl":"必修"},
          {"nr":"研究生考试甲","nq":"2026-09-02","kssj":"09：00","jssj":"11：00","dd":"教室甲","fl":"考试"},
          {"nr":"考试复习（个人安排）","nq":"2026-09-02","kssj":"13：00","jssj":"14：00","fl":"个人日历"}
        ])"#,
    )]);
    let base = unique_cache_base("graduate-exams");
    let mut runtime = CampusRuntime::new_auto_with_persistence(
        "2026-2027-1".into(),
        true,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    let user = test_user("2026000000");
    install_identity(&mut runtime, &user);
    let source = schedule_source(&runtime, server.base(), AcademicStage::Graduate);
    install_registrar_proof(&mut runtime, &user, source);
    runtime.semester_first_day = NaiveDate::from_ymd_opt(2026, 9, 1);
    runtime.semester_last_day = NaiveDate::from_ymd_opt(2026, 9, 2);
    let report = runtime
        .load_exams(String::new(), String::new())
        .await
        .unwrap();
    assert_eq!(report.stage, "graduate");
    assert_eq!(report.source.as_deref(), Some("live"));
    assert_eq!(report.status.as_deref(), Some("ready"));
    assert_eq!(report.exam_count, 1);
    let exam = &report.exams[0];
    assert_eq!(exam.course_name, "研究生考试甲");
    assert!(exam.course_code.is_empty() && exam.course_sequence.is_empty());
    assert_eq!(exam.exam_month, 9);
    assert_eq!(exam.exam_day, 2);
    assert_eq!(exam.exam_weekday, "wednesday");
    assert_eq!(exam.exam_session, "09:00–11:00");
    assert_eq!(exam.schedule_raw, "2026-09-02 09:00–11:00");
    assert_eq!(exam.location, "教室甲");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("m=yjs_jxrl_all"));
    assert!(requests[0].contains("p_start_date=20260901"));
    assert!(requests[0].contains("p_end_date=20260902"));
    assert!(!requests[0].contains("bks_ksSearch"));
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_graduate_exams_empty_requires_valid_complete_calendar() {
    for body in [
        "thyouCalendar([])",
        r#"thyouCalendar([{"nr":"考试方法论","nq":"2026-09-02","kssj":"09:00","jssj":"11:00","fl":"必修"}])"#,
    ] {
        let server = FixtureServer::new(vec![Reply::json(body)]);
        let base = unique_cache_base("graduate-empty-exams");
        let mut runtime = graduate_exam_fixture_runtime(&base, &server);
        let report = runtime
            .load_exams(String::new(), String::new())
            .await
            .unwrap();
        assert_eq!(report.exam_count, 0);
        assert_eq!(report.source.as_deref(), Some("live"));
        assert_eq!(server.requests().len(), 1);
        drop(runtime);
        remove_cache_file(base);
    }
}

#[tokio::test]
async fn backend_repair_graduate_exams_invalid_or_failed_responses_never_become_empty() {
    for reply in [
        Reply::json("wrongCallback([])"),
        Reply::json(r#"thyouCalendar({"success":false})"#),
        Reply::json(
            r#"thyouCalendar([{"nr":"分类缺失","nq":"2026-09-02","kssj":"09:00","jssj":"11:00"}])"#,
        ),
        Reply::json(
            r#"thyouCalendar([{"nr":"坏日期","nq":"2026-02-30","kssj":"09:00","jssj":"11:00","fl":"考试"}])"#,
        ),
        Reply {
            status: 503,
            headers: String::new(),
            body: "fixture unavailable".into(),
        },
    ] {
        let server = FixtureServer::new(vec![reply]);
        let base = unique_cache_base("graduate-bad-exams");
        let mut runtime = graduate_exam_fixture_runtime(&base, &server);
        assert!(
            runtime
                .load_exams(String::new(), String::new())
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1, "no authentication replay");
        let path =
            registrar_exams_cache_path(&base, "2026000000", "2026-2027-1", AcademicStage::Graduate);
        assert!(
            !path.exists(),
            "failed parsing must not persist successful emptiness"
        );
        drop(runtime);
        remove_cache_file(base);
    }
}

#[tokio::test]
async fn backend_repair_graduate_exams_reads_entire_term_in_bounded_windows() {
    let server = FixtureServer::new(vec![
        Reply::json("thyouCalendar([])"),
        Reply::json(
            r#"thyouCalendar([{"nr":"学期后段考试","nq":"2026-09-30","kssj":"09:00","jssj":"11:00","fl":"期末考试"}])"#,
        ),
    ]);
    let base = unique_cache_base("graduate-term-exams");
    let mut runtime = graduate_exam_fixture_runtime(&base, &server);
    runtime.semester_last_day = NaiveDate::from_ymd_opt(2026, 10, 1);
    let report = runtime
        .load_exams(String::new(), String::new())
        .await
        .unwrap();
    assert_eq!(report.exam_count, 1);
    assert_eq!(report.exams[0].schedule_raw, "2026-09-30 09:00–11:00");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].contains("p_start_date=20260901")
            && requests[0].contains("p_end_date=20260928")
    );
    assert!(
        requests[1].contains("p_start_date=20260929")
            && requests[1].contains("p_end_date=20261001")
    );
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_graduate_exams_cache_is_account_stage_term_and_time_bound() {
    let server = FixtureServer::new(vec![Reply::json(GRADUATE_EXAM_BODY)]);
    let base = unique_cache_base("graduate-bound-exams");
    let mut runtime = graduate_exam_fixture_runtime(&base, &server);
    runtime
        .load_exams(String::new(), String::new())
        .await
        .unwrap();
    let path =
        registrar_exams_cache_path(&base, "2026000000", "2026-2027-1", AcademicStage::Graduate);
    let cache = JsonFileCache::<super::RegistrarExamsCachePayload>::new(
        &path,
        REGISTRAR_EXAMS_CACHE_SCHEMA_VERSION,
        super::graduate_exams::cache_service(AcademicStage::Graduate),
    );
    let payload = cache.read().unwrap().unwrap().payload;
    let valid = |p: &super::RegistrarExamsCachePayload, account, term, stage| {
        super::registrar_exams_cache_payload_is_valid(p, account, term, stage)
    };
    assert!(valid(
        &payload,
        "2026000000",
        "2026-2027-1",
        AcademicStage::Graduate
    ));
    assert!(!valid(
        &payload,
        "other-account",
        "2026-2027-1",
        AcademicStage::Graduate
    ));
    assert!(!valid(
        &payload,
        "2026000000",
        "2025-2026-2",
        AcademicStage::Graduate
    ));
    assert!(!valid(
        &payload,
        "2026000000",
        "2026-2027-1",
        AcademicStage::Undergraduate
    ));
    let mut invalid = payload.clone();
    invalid.calendar_window = None;
    assert!(!valid(
        &invalid,
        "2026000000",
        "auto",
        AcademicStage::Graduate
    ));
    invalid = payload.clone();
    invalid.exams[0].exam_day = 1;
    assert!(!valid(
        &invalid,
        "2026000000",
        "auto",
        AcademicStage::Graduate
    ));
    invalid = payload.clone();
    invalid.exams[0].category = Some("非考试".into());
    assert!(!valid(
        &invalid,
        "2026000000",
        "auto",
        AcademicStage::Graduate
    ));
    invalid = payload;
    invalid.generated_at = Utc::now() + chrono::Duration::minutes(6);
    assert!(!valid(
        &invalid,
        "2026000000",
        "auto",
        AcademicStage::Graduate
    ));
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_graduate_exams_restart_reuses_bound_cache_without_handoff() {
    let server = FixtureServer::new(vec![Reply::json(GRADUATE_EXAM_BODY)]);
    let base = unique_cache_base("graduate-restart-exams");
    let mut runtime = graduate_exam_fixture_runtime(&base, &server);
    let live = runtime
        .load_exams(String::new(), String::new())
        .await
        .unwrap();
    drop(runtime);
    let mut next = CampusRuntime::new_auto_with_persistence(
        "auto".into(),
        true,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    let user = test_user("2026000000");
    install_identity(&mut next, &user);
    let cached = next.load_exams(String::new(), String::new()).await.unwrap();
    assert_eq!(cached.exams, live.exams);
    assert_eq!(cached.source.as_deref(), Some("cache"));
    assert_eq!(cached.status.as_deref(), Some("ready"));
    assert!(!next.service_session_is_proven(ServiceId::Registrar));
    assert_eq!(server.requests().len(), 1);
    assert!(
        next.service_catalog()
            .services
            .iter()
            .any(|entry| entry.id == "registrar"
                && entry.capabilities.iter().any(|cap| cap.key == "read_exams"))
    );
    drop(next);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_graduate_exams_stale_fallback_is_explicit_and_finite() {
    let server = FixtureServer::new(vec![
        Reply::json(GRADUATE_EXAM_BODY),
        Reply {
            status: 503,
            headers: String::new(),
            body: String::new(),
        },
        Reply {
            status: 503,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let base = unique_cache_base("graduate-stale-exams");
    let mut runtime = graduate_exam_fixture_runtime(&base, &server);
    runtime
        .load_exams(String::new(), String::new())
        .await
        .unwrap();
    let path =
        registrar_exams_cache_path(&base, "2026000000", "2026-2027-1", AcademicStage::Graduate);
    let cache = JsonFileCache::<super::RegistrarExamsCachePayload>::new(
        &path,
        REGISTRAR_EXAMS_CACHE_SCHEMA_VERSION,
        super::graduate_exams::cache_service(AcademicStage::Graduate),
    );
    let mut payload = cache.read().unwrap().unwrap().payload;
    payload.generated_at = Utc::now() - chrono::Duration::days(1);
    cache.write(&payload).unwrap();
    let stale = runtime
        .load_exams(String::new(), String::new())
        .await
        .unwrap();
    assert_eq!(stale.source.as_deref(), Some("cache"));
    assert_eq!(stale.status.as_deref(), Some("stale"));
    assert!(stale.error.is_some());
    payload.generated_at = Utc::now() - chrono::Duration::days(8);
    cache.write(&payload).unwrap();
    assert!(
        runtime
            .load_exams(String::new(), String::new())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_graduate_exams_missing_term_or_wrong_source_fails_before_dispatch() {
    let server = FixtureServer::new(vec![]);
    let base = unique_cache_base("graduate-source-exams");
    let mut runtime = graduate_exam_fixture_runtime(&base, &server);
    runtime.semester_first_day = None;
    assert!(
        runtime
            .load_exams(String::new(), String::new())
            .await
            .is_err()
    );
    runtime.semester_first_day = NaiveDate::from_ymd_opt(2026, 9, 1);
    runtime.grades_source = Some(schedule_source(
        &runtime,
        server.base(),
        AcademicStage::Undergraduate,
    ));
    assert!(
        runtime
            .load_exams(String::new(), String::new())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    drop(runtime);
    remove_cache_file(base);
}
