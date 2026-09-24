use super::*;

#[tokio::test]
async fn backend_repair_learn_discussions_cli_requires_fresh_course_selector() {
    let plan = select_cases(&["learn_discussions".into()]).unwrap();
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
            "learn_discussions",
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
