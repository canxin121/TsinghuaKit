#[tokio::test]
async fn backend_repair_learn_file_runtime_binds_selector_account_and_shared_transport() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"result":"success","object":[{"kjxxid":"row-one","wjid":"private-file-id","bt":"合成课程资料","fileSize":"2 MB"}]}"#,
    )]);
    let base = unique_cache_base("learn-files-runtime");
    let user = test_user("fixture-learn-file-owner");
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
            user_agent: "fixture-learn-files".into(),
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
            .load_learn_files("forged-course".into())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let result = runtime
        .load_learn_files("live-course".into())
        .await
        .unwrap();
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert!(result.complete && result.error.is_none());
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].title, "合成课程资料");
    assert!(!format!("{result:?}").contains("private-file-id"));
    assert_eq!(server.requests().len(), 1);
    runtime.reset_sessions();
    assert!(
        runtime
            .load_learn_files("live-course".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
    drop(runtime);
    remove_cache_file(base);
}

#[test]
fn backend_repair_learn_file_portal_factor_pause_blocks_new_read() {
    let mut ledger = crate::live_validation::LiveValidationLedger::default();
    ledger.select_cases("learn_files").unwrap();
    super::record_portal_dependent_cases_blocked(&mut ledger);
    assert_eq!(ledger.cases["learn_files"].status, "blocked");
    assert!(ledger.should_run("learn_files"));
}

#[test]
fn backend_repair_learn_file_download_portal_challenge_blocks_selected_probe() {
    let mut ledger = crate::live_validation::LiveValidationLedger::default();
    ledger.select_cases("learn_file_download").unwrap();
    super::record_portal_dependent_cases_blocked(&mut ledger);
    assert_eq!(ledger.cases["learn_file_download"].status, "blocked");
    assert!(ledger.should_run("learn_file_download"));
}

#[tokio::test]
async fn backend_repair_learn_file_download_runtime_rechecks_selector_and_account() {
    let list = r#"{"result":"success","object":[{"kjxxid":"row-one","wjid":"private-file-id","bt":"合成课程资料","wjlx":"pdf"}]}"#;
    let server = FixtureServer::new(vec![
        Reply::json(list),
        Reply::json(list),
        Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: "%PDF-1.7\nfixture-pdf".into(),
        },
    ]);
    let base = unique_cache_base("learn-file-download-runtime");
    let user = test_user("fixture-learn-download-owner");
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
            user_agent: "fixture-learn-file-download".into(),
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..RegistrarClientConfig::default()
        },
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.learn_source = Some(runtime.build_learn_source(learn, registrar, csrf).unwrap());
    runtime.learn_course_ids = vec!["live-course".into()];

    let listed = runtime.load_learn_files("live-course".into()).await;
    assert!(
        listed.is_ok(),
        "first list failed after {} fixture requests: {:?}",
        server.requests().len(),
        listed.as_ref().err()
    );
    let listed = listed.unwrap();
    let selector = listed.files[0].id.clone();
    let destination =
        std::env::temp_dir().join(format!("thyou-download-fixture-{}.pdf", Uuid::new_v4()));
    let path = destination.to_string_lossy().into_owned();
    assert!(
        runtime
            .download_learn_file("forged-course".into(), selector.clone(), path.clone())
            .await
            .is_err()
    );
    assert!(
        runtime
            .download_learn_file("live-course".into(), "forged-selector".into(), path.clone())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);

    let result = runtime
        .download_learn_file("live-course".into(), selector.clone(), path.clone())
        .await
        .unwrap();
    assert_eq!(result.file_id, selector);
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert_eq!(result.bytes_written, b"%PDF-1.7\nfixture-pdf".len() as u64);
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"%PDF-1.7\nfixture-pdf"
    );
    assert_eq!(server.requests().len(), 3);
    assert!(!format!("{result:?}").contains("private-file-id"));
    assert!(
        runtime
            .download_learn_file("live-course".into(), selector.clone(), path)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);

    runtime.reset_sessions();
    assert!(
        runtime
            .probe_learn_file_download("live-course".into(), selector)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    drop(runtime);
    std::fs::remove_file(destination).unwrap();
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_learn_file_category_runtime_rejects_foreign_course_and_binds_account() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"result":"success","object":{"rows":[{"kjflid":"private-category","bt":"第一单元"}]}}"#,
    )]);
    let base = unique_cache_base("learn-file-categories-runtime");
    let user = test_user("fixture-learn-category-owner");
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
            user_agent: "fixture-learn-file-categories".into(),
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
            .load_learn_file_categories("forged-course".into())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let result = runtime
        .load_learn_file_categories("live-course".into())
        .await
        .unwrap();
    assert_eq!(result.course_id, "live-course");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert_eq!(result.categories.len(), 1);
    assert_eq!(result.categories[0].title, "第一单元");
    assert!(!format!("{result:?}").contains("private-category"));
    assert_eq!(server.requests().len(), 1);
    runtime.reset_sessions();
    assert!(
        runtime
            .load_learn_file_categories("live-course".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
    drop(runtime);
    remove_cache_file(base);
}
