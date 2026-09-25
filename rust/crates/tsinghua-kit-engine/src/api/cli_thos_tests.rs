use super::*;
use serde_json::json;

fn thos_runtime(server: &FixtureServer, root: &Path) -> CampusRuntime {
    let mut runtime = fixture_runtime(server, root);
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime.coordinator.begin_authentication(service).unwrap();
        runtime
            .coordinator
            .mark_authenticated(
                service,
                UserIdentity {
                    username: "synthetic-private-owner".into(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .unwrap();
    }
    runtime.portal_bootstrapped = true;
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/info").unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    runtime.info_roaming_url =
        Some(crate::info::OpaqueUrl::new(format!("{}info/home", server.base())).unwrap());
    runtime
}

fn replies(count: usize, reported: usize) -> Vec<Reply> {
    let items: Vec<_> = (0..count)
        .map(|i| {
            json!({
                "proc_inst_id":format!("synthetic-private-process-{i}"),
                "task_id":format!("synthetic-private-task-{i}"),
                "service_name":"synthetic-private-title",
                "CURRENT_ACT_NAME":"synthetic-private-node"
            })
        })
        .collect();
    vec![
        Reply::json(&json!({"zbNum":0,"AuditSvsNum":reported}).to_string()),
        Reply::json(&json!({"list":items,"total":count,"pageNum":1}).to_string()),
        Reply::json(r#"{"list":[],"total":0,"pageNum":1}"#),
    ]
}

fn report(root: &Path, portal_ok: bool) -> ReportWriter {
    let selected = select_cases(&["thos_pending".into()]).unwrap();
    let mut report = ReportWriter::new(
        root.join("report.json"),
        "fixture-thos-cli".into(),
        &selected,
        false,
    )
    .unwrap();
    for id in ["identity_session", "portal_bootstrap", "info_session"] {
        if !portal_ok && id == "info_session" {
            break;
        }
        report.start(id).unwrap();
        let passed = portal_ok || id == "identity_session";
        report
            .end(
                id,
                if passed {
                    CheckStatus::Passed
                } else {
                    CheckStatus::Failed
                },
                if passed { "verified" } else { "session" },
                None,
                0,
                0,
            )
            .unwrap();
    }
    report
}

#[tokio::test]
async fn backend_repair_thos_phase_steps_cli_uses_fresh_phase_selector_and_counts_only_steps() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"{"list":[{"AGG_PROC_ID":"synthetic-private-aggregate","NAME":"合成阶段"}],"total":1,"pageNum":1}"#,
        ),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(r#"[{"ACT_NAME":"合成步骤","itemInfo":[]}]"#),
    ]);
    let mut runtime = thos_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "thos_phases",
            false
        )
        .await
        .unwrap(),
        Outcome::Passed(Some(1))
    ));
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "thos_phase_steps",
            false
        )
        .await
        .unwrap(),
        Outcome::Passed(Some(1))
    ));
    assert_eq!(prompt.secret_reads, 0);
    assert_eq!(server.requests().len(), 4);
    assert!(server.requests()[3].contains("/fp/aggregation/getActWork"));
    assert!(
        select_cases(&["thos_phase_steps".into()])
            .unwrap()
            .contains("thos_phases")
    );
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_thos_phase_steps_cli_marks_missing_sample_unverified() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(r#"{"list":[],"total":0,"pageNum":1}"#),
    ]);
    let mut runtime = thos_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "thos_phases",
            false
        )
        .await
        .unwrap(),
        Outcome::Passed(Some(0))
    ));
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "thos_phase_steps",
            false
        )
        .await
        .unwrap(),
        Outcome::Skipped("no_phase_selector")
    ));
    assert_eq!(server.requests().len(), 2);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_thos_services_cli_requires_fresh_complete_read() {
    let root = directory();
    let responses = vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(r#"{"list":[{"ID":"fixture-one","NAME":"合成服务一"}],"total":1,"pageNum":1}"#),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"{"list":[{"ID":"fixture-one","NAME":"合成服务一"},{"ID":"fixture-two","NAME":"合成服务二"}],"total":2,"pageNum":1}"#,
        ),
    ];
    let server = FixtureServer::new(responses);
    let mut runtime = thos_runtime(&server, &root);
    let cached = runtime.load_thos_services(false).await.unwrap();
    assert_eq!(cached.items.len(), 1);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    let result = execute_case(
        &mut runtime,
        &mut prompt,
        &mut evidence,
        "thos_services",
        false,
    )
    .await
    .unwrap();
    assert!(matches!(result, Outcome::Passed(Some(2))));
    assert_eq!(server.requests().len(), 4);
    assert_eq!(prompt.secret_reads, 0);
    assert!(
        select_cases(&["thos_services".into()])
            .unwrap()
            .contains("info_session")
    );
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_thos_other_task_cli_requires_fresh_read_and_fixed_kind() {
    let root = directory();
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"{"list":[{"proc_inst_id":"fixture-old","service_name":"合成旧事项"}],"total":1,"pageNum":1}"#,
        ),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"{"list":[{"proc_inst_id":"fixture-new","service_name":"合成新事项"},{"proc_inst_id":"fixture-two","service_name":"合成新事项二"}],"total":2,"pageNum":1}"#,
        ),
    ]);
    let mut runtime = thos_runtime(&server, &root);
    let cached = runtime
        .load_thos_task_list("completed".into(), false)
        .await
        .unwrap();
    assert_eq!(cached.items.len(), 1);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    let result = execute_case(
        &mut runtime,
        &mut prompt,
        &mut evidence,
        "thos_completed",
        false,
    )
    .await
    .unwrap();
    assert!(matches!(result, Outcome::Passed(Some(2))));
    assert_eq!(server.requests().len(), 4);
    for id in [
        "thos_completed",
        "thos_drafts",
        "thos_unread",
        "thos_phases",
    ] {
        let plan = select_cases(&[id.into()]).unwrap();
        assert!(plan.contains("info_session") && plan.contains(id));
        assert!(!plan.contains("thos_pending"));
    }
    assert_eq!(prompt.secret_reads, 0);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_thos_cli_reads_fresh_result_and_reports_only_count() {
    let root = directory();
    let server = FixtureServer::new(replies(1, 1).into_iter().chain(replies(2, 2)).collect());
    let mut runtime = thos_runtime(&server, &root);
    let cached = runtime.load_thos_pending(false).await.unwrap();
    assert_eq!(cached.items.len(), 1);
    let mut report = report(&root, true);
    let mut prompt = Prompt::default();
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    let row = &report.report.cases["thos_pending"];
    assert_eq!(row.status, CheckStatus::Passed);
    assert_eq!(row.count, Some(2));
    assert_eq!(row.attempts, 1);
    assert_eq!(row.requests, 3);
    assert_eq!(report.exit_code(), 0);
    assert_eq!(server.requests().len(), 6);
    assert_eq!(prompt.secret_reads, 0);
    assert_eq!(prompt.confirmations, 0);
    assert!(runtime.learn_source.is_none() && runtime.grades_source.is_none());
    let body = fs::read_to_string(root.join("report.json")).unwrap();
    for private in [
        "synthetic-private-",
        "\"username\"",
        "\"title\"",
        server.base(),
        crate::thos::MAPPING,
    ] {
        assert!(!body.contains(private));
    }
    assert_eq!(
        retry_plan(&root.join("report.json")).unwrap_err(),
        "retry_report_has_no_unfinished_cases"
    );
    drop(report);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_thos_cli_incomplete_result_fails_and_keeps_targeted_retry() {
    let root = directory();
    let server = FixtureServer::new(replies(0, 1));
    let mut runtime = thos_runtime(&server, &root);
    let mut report = report(&root, true);
    let mut prompt = Prompt::default();
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    let row = &report.report.cases["thos_pending"];
    assert_eq!(row.status, CheckStatus::Failed);
    assert_eq!(row.reason, "validation_live_result_required");
    assert_eq!(row.count, None);
    assert_eq!(row.attempts, 1);
    assert_eq!(row.requests, 3);
    assert_eq!(report.exit_code(), 1);
    assert_eq!(server.requests().len(), 3);
    assert_eq!(prompt.secret_reads, 0);
    let path = root.join("report.json");
    let before = fs::read(&path).unwrap();
    assert_eq!(
        retry_plan(&path).unwrap().0,
        select_cases(&["thos_pending".into()]).unwrap()
    );
    assert_eq!(fs::read(path).unwrap(), before);
    drop(report);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_thos_cli_failed_prerequisite_blocks_without_another_handoff() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut report = report(&root, false);
    let mut prompt = Prompt::default();
    run(&mut runtime, &mut prompt, &mut report, false)
        .await
        .unwrap();
    for id in ["info_session", "thos_pending"] {
        let row = &report.report.cases[id];
        assert_eq!(row.status, CheckStatus::Blocked);
        assert_eq!(row.reason, "dependency_not_passed");
        assert_eq!(row.attempts, 0);
        assert_eq!(row.requests, 0);
    }
    assert_eq!(prompt.secret_reads, 0);
    assert_eq!(prompt.confirmations, 0);
    assert!(server.requests().is_empty());
    drop(report);
    drop(runtime);
    fs::remove_dir_all(root).unwrap();
}
