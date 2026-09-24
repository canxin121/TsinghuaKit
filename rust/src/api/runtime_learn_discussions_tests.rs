#[tokio::test]
async fn backend_repair_learn_discussions_runtime_binds_course_and_account() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"result":"success","object":{"resultsList":[{"id":"private-topic","bqid":"private-board","wlkcid":"live-course","bt":"课程讨论","fbrxm":"合成教师","fbsj":"2026-09-24 08:00","hfcs":2}]}}"#,
    )]);
    let base = unique_cache_base("learn-discussions-runtime");
    let user = test_user("fixture-learn-discussion-owner");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".into(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    install_fixture_learn_source_for_user(&mut runtime, server.base(), &user);
    let csrf = runtime
        .coordinator
        .registry()
        .bound_csrf(ServiceId::Learn)
        .unwrap();
    let learn =
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
    let registrar = RegistrarClient::from_transport(
        RegistrarClientConfig {
            user_agent: "fixture-learn-discussions".into(),
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..RegistrarClientConfig::default()
        },
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.learn_source = Some(runtime.build_learn_source(learn, registrar, csrf).unwrap());
    runtime.learn_course_ids = vec!["live-course".into()];
    assert!(
        runtime
            .load_learn_discussions("forged-course".into())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let result = runtime
        .load_learn_discussions("live-course".into())
        .await
        .unwrap();
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert!(result.complete && result.error.is_none());
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].title, "课程讨论");
    assert_eq!(result.items[0].reply_count, 2);
    assert!(!format!("{result:?}").contains("private-topic"));
    assert_eq!(server.requests().len(), 1);
    runtime.reset_sessions();
    assert!(
        runtime
            .load_learn_discussions("live-course".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
    drop(runtime);
    remove_cache_file(base);
}

#[test]
fn backend_repair_learn_discussions_portal_challenge_blocks_selected_read() {
    let mut ledger = crate::live_validation::LiveValidationLedger::default();
    ledger.select_cases("learn_discussions").unwrap();
    super::record_portal_dependent_cases_blocked(&mut ledger);
    assert_eq!(ledger.cases["learn_discussions"].status, "blocked");
    assert!(ledger.should_run("learn_discussions"));
}
