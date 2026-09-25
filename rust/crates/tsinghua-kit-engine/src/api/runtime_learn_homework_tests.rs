#[tokio::test]
async fn backend_repair_homework_runtime_binds_course_selector_and_account() {
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"result":"success","object":{"aaData":[{"wlkcid":"live-course","xszyid":"private-student","zyid":"private-base","bt":"合成作业","jzsj":"2026-09-30 23:59:59"}]}}"#,
        ),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#),
        Reply::html(
            "<div class='list calendar clearfix'><div class='fl right'><div class='c55'>说明</div></div></div>",
        ),
        Reply::json(r#"{"result":"success","msg":"<p>请完成题目</p>"}"#),
    ]);
    let base = unique_cache_base("learn-homework-runtime");
    let user = test_user("fixture-homework-owner");
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
            user_agent: "fixture-homework".into(),
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..RegistrarClientConfig::default()
        },
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.learn_source = Some(runtime.build_learn_source(learn, registrar, csrf).unwrap());
    runtime.learn_course_ids = vec!["live-course".into()];
    runtime.learn_course_records = Some(vec![LearnCourseRecord {
        course_id: Some("live-course".into()),
        course_code: None,
        name: Some("合成课程".into()),
        instructor: None,
        class_name: None,
        semester: Some("2026-2027-1".into()),
        unknown_fields: Default::default(),
    }]);

    assert!(
        runtime
            .load_learn_homework("forged-course".into())
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let list = runtime
        .load_learn_homework("live-course".into())
        .await
        .unwrap();
    assert_eq!(list.source, "live");
    assert_eq!(list.items.len(), 1);
    assert!(list.items[0].detail_available);
    assert!(!format!("{list:?}").contains("private-student"));
    assert!(!format!("{list:?}").contains("private-base"));
    assert_eq!(server.requests().len(), 3);
    assert!(
        runtime
            .load_learn_homework_detail("forged-selector".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    let detail = runtime
        .load_learn_homework_detail(list.items[0].selector.clone())
        .await
        .unwrap();
    assert_eq!(detail.description.as_deref(), Some("请完成题目"));
    assert_eq!(server.requests().len(), 5);
    runtime.reset_sessions();
    assert!(
        runtime
            .load_learn_homework_detail(list.items[0].selector.clone())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 5);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_homework_cached_course_cannot_authorize_live_list() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","resultList":[]}"#,
    )]);
    let base = unique_cache_base("learn-homework-cache-proof");
    let user = test_user("fixture-homework-cache-owner");
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
            user_agent: "fixture-homework".into(),
            learn_base_url: server.base().into(),
            registrar_base_url: server.base().into(),
            ..RegistrarClientConfig::default()
        },
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.learn_source = Some(runtime.build_learn_source(learn, registrar, csrf).unwrap());
    runtime.learn_course_ids = vec!["cached-course".into()];
    runtime.learn_course_records = None;

    assert!(
        runtime
            .load_learn_homework("cached-course".into())
            .await
            .is_err()
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].contains("zyList"));
    drop(runtime);
    remove_cache_file(base);
}
