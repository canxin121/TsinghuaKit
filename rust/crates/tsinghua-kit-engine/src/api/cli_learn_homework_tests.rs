use super::*;

#[tokio::test]
async fn backend_repair_homework_cli_requires_live_course_and_detail_sample() {
    let plan = select_cases(&["learn_homework_detail".into()]).unwrap();
    assert!(plan.contains("identity_session"));
    assert!(plan.contains("portal_bootstrap"));
    assert!(plan.contains("learn_session"));
    assert!(plan.contains("learn_courses"));
    assert!(plan.contains("learn_homework"));
    assert!(plan.contains("learn_homework_detail"));
    assert!(!plan.contains("learn_todos"));

    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_homework",
            false
        )
        .await,
        Ok(Outcome::Skipped("no_course_selector"))
    ));
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_homework_detail",
            false
        )
        .await,
        Ok(Outcome::Skipped("no_homework_selector"))
    ));
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_homework_detail_samples_later_course_once_after_empty_first_course() {
    let root = directory();
    let empty = r#"{"result":"success","object":{"aaData":[]}}"#;
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"First"},{"wlkcid":"course-two","kch":"C002","kcm":"Second"}]}"#,
        ),
        Reply::json(empty),
        Reply::json(empty),
        Reply::json(empty),
        Reply::json(
            r#"{"result":"success","object":{"aaData":[{"wlkcid":"course-two","xszyid":"private-student","zyid":"private-base","bt":"Fixture homework","jzsj":"2026-09-30 23:59:59"}]}}"#,
        ),
        Reply::json(empty),
        Reply::json(empty),
        Reply::html(
            "<div class='list calendar clearfix'><div class='fl right'><div class='c55'>Fixture answer</div></div></div>",
        ),
        Reply::json(r#"{"result":"success","msg":"Fixture description"}"#),
    ]);
    let mut runtime = run113847_learn_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_courses",
            false
        )
        .await
        .unwrap(),
        Outcome::Passed(Some(2))
    ));
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_homework",
            false
        )
        .await
        .unwrap(),
        Outcome::Passed(Some(0))
    ));
    assert!(evidence.homework_selector.is_none());
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_homework_detail",
            false
        )
        .await
        .unwrap(),
        Outcome::Passed(Some(0))
    ));
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(
        requests[1..4]
            .iter()
            .all(|request| request.contains("course-one"))
    );
    assert!(
        requests[4..7]
            .iter()
            .all(|request| request.contains("course-two"))
    );
    assert!(requests[7].starts_with("GET ") && requests[7].contains("course-two"));
    assert!(requests[8].starts_with("POST ") && requests[8].contains("private-base"));
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_homework_detail_keeps_all_empty_courses_unverified() {
    let root = directory();
    let empty = r#"{"result":"success","object":{"aaData":[]}}"#;
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"course-one","kch":"C001","kcm":"First"},{"wlkcid":"course-two","kch":"C002","kcm":"Second"}]}"#,
        ),
        Reply::json(empty),
        Reply::json(empty),
        Reply::json(empty),
        Reply::json(empty),
        Reply::json(empty),
        Reply::json(empty),
    ]);
    let mut runtime = run113847_learn_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut prompt = Prompt::default();
    execute_case(
        &mut runtime,
        &mut prompt,
        &mut evidence,
        "learn_courses",
        false,
    )
    .await
    .unwrap();
    execute_case(
        &mut runtime,
        &mut prompt,
        &mut evidence,
        "learn_homework",
        false,
    )
    .await
    .unwrap();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_homework_detail",
            false
        )
        .await
        .unwrap(),
        Outcome::Skipped("no_homework_selector")
    ));
    assert_eq!(server.requests().len(), 7);
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
