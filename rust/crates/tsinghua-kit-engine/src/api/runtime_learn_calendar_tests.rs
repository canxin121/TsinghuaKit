#[tokio::test]
async fn backend_repair_learn_calendar_runtime_requires_proven_account_and_real_current_next_response()
 {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","result":{"id":"2026-2027-1","kssj":"2026-09-02","jssj":"2027-01-15","xnxq":"2026-2027-1","xnxqmc":"秋季学期"},"resultList":[{"id":"2026-2027-2","kssj":"2027-02-21","jssj":"2027-07-02","xnxqmc":"春季学期"}]}"#,
    )]);
    let base = unique_cache_base("learn-calendar-runtime");
    let user = test_user("fixture-calendar-owner");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".into(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    assert!(runtime.load_learn_term_calendar().await.is_err());
    assert!(server.requests().is_empty());
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
            user_agent: "fixture-learn-calendar".into(),
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..RegistrarClientConfig::default()
        },
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.learn_source = Some(runtime.build_learn_source(learn, registrar, csrf).unwrap());

    let calendar = runtime.load_learn_term_calendar().await.unwrap();
    assert_eq!(calendar.source, "live");
    assert_eq!(calendar.status, "ready");
    assert_eq!(calendar.current.first_day, "2026-08-31");
    assert_eq!(calendar.upcoming.len(), 1);
    assert_eq!(calendar.upcoming[0].first_day, "2027-02-22");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("getCurrentAndNextSemester?_csrf="));
    runtime.reset_sessions();
    assert!(runtime.load_learn_term_calendar().await.is_err());
    assert_eq!(server.requests().len(), 1);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_learn_calendar_runtime_rejects_missing_next_list_without_empty_success() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","result":{"id":"2026-2027-1","kssj":"2026-09-02","jssj":"2027-01-15","xnxq":"2026-2027-1"}}"#,
    )]);
    let base = unique_cache_base("learn-calendar-malformed");
    let user = test_user("fixture-calendar-owner");
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
            user_agent: "fixture-learn-calendar".into(),
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..RegistrarClientConfig::default()
        },
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.learn_source = Some(runtime.build_learn_source(learn, registrar, csrf).unwrap());
    let error = runtime.load_learn_term_calendar().await.unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "learn_response_format"
    );
    assert_eq!(server.requests().len(), 1);
    drop(runtime);
    remove_cache_file(base);
}
