use super::*;

#[tokio::test]
async fn backend_repair_learn_files_cli_requires_fresh_course_selector() {
    let plan = select_cases(&["learn_files".into()]).unwrap();
    for dependency in [
        "identity_session",
        "portal_bootstrap",
        "learn_session",
        "learn_courses",
    ] {
        assert!(plan.contains(dependency));
    }
    assert!(!plan.contains("learn_todos"));

    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_files",
            false
        )
        .await
        .unwrap(),
        Outcome::Skipped("no_course_selector")
    ));
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_learn_file_categories_cli_requires_fresh_course_selector() {
    let plan = select_cases(&["learn_file_categories".into()]).unwrap();
    for dependency in [
        "identity_session",
        "portal_bootstrap",
        "learn_session",
        "learn_courses",
    ] {
        assert!(plan.contains(dependency));
    }
    assert!(!plan.contains("learn_files"));

    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_file_categories",
            false
        )
        .await
        .unwrap(),
        Outcome::Skipped("no_course_selector")
    ));
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_learn_file_download_cli_requires_fresh_course_selector() {
    let plan = select_cases(&["learn_file_download".into()]).unwrap();
    for dependency in [
        "identity_session",
        "portal_bootstrap",
        "learn_session",
        "learn_courses",
    ] {
        assert!(plan.contains(dependency));
    }
    assert!(!plan.contains("learn_files"));

    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(matches!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_file_download",
            false
        )
        .await
        .unwrap(),
        Outcome::Skipped("no_course_selector")
    ));
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
