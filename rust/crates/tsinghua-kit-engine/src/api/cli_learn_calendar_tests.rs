use super::*;

#[tokio::test]
async fn backend_repair_learn_calendar_cli_needs_only_current_learn_session() {
    let plan = select_cases(&["learn_calendar".into()]).unwrap();
    for dependency in ["identity_session", "portal_bootstrap", "learn_session"] {
        assert!(plan.contains(dependency));
    }
    assert!(!plan.contains("learn_courses"));

    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "learn_calendar",
            false,
        )
        .await
        .is_err()
    );
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_school_calendar_cli_uses_one_learn_session_and_no_other_business() {
    let plan = select_cases(&["school_calendar".into()]).unwrap();
    for dependency in ["identity_session", "portal_bootstrap", "learn_session"] {
        assert!(plan.contains(dependency));
    }
    assert!(!plan.contains("learn_courses"));
    assert!(!plan.contains("info_session"));
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "school_calendar",
            false,
        )
        .await
        .is_err()
    );
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
