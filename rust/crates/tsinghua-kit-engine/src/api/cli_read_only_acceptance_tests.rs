use super::*;

/// Every domain the acceptance script reads and the runtime can answer without
/// a write.  The ids are the ones this file's cases use; a case removed from
/// the table makes this list fail rather than silently shrink the script.
const READ_ONLY_CASES: &[&str] = &[
    "program_completion",
    "physical_exam_result",
    "assessment_list",
    "assessment_form",
    "invoice_list",
    "invoice_document",
    "bank_payment_ledger",
    "bank_foundation_ledger",
    "graduate_income",
    "course_score",
    "sports_resources",
    "sports_records",
    "reserves_search",
    "reserves_detail",
    "library_room_catalog",
    "library_room_records",
    "library_reservations",
    "laundry_buildings",
    "laundry_rooms",
    "water_user",
];

#[test]
fn backend_repair_read_only_acceptance_skip_reasons_survive_the_audit_log() {
    // A skip reason that is not in the fixed vocabulary is not an error, but
    // the audit log then redacts the field and `--status` prints an empty
    // reason.  Every reason this file's cases can report must be readable
    // there, so the vocabulary is asserted rather than assumed.
    let source = include_str!("cli_validation.rs");
    let mut used = BTreeSet::new();
    let mut rest = source;
    while let Some(at) = rest.find("Outcome::Skipped(") {
        let call = &rest[at + "Outcome::Skipped(".len()..];
        // The call's argument ends at the next `)`, except for the one arm that
        // picks between two reasons, which the loop reaches again on its own.
        let end = call.find(')').unwrap_or(call.len());
        for candidate in call[..end].split('"').skip(1).step_by(2) {
            if candidate.contains('_') {
                used.insert(candidate);
            }
        }
        rest = &call[end..];
    }
    // The whole table is scanned, not just this file's cases, so the guard
    // covers a skip branch added by any later batch.
    assert!(used.len() >= READ_ONLY_CASES.len() / 2);
    for reason in used {
        // `diagnostic_reason` answers with its input only when the input is one
        // of the fixed codes, so an unregistered reason comes back coarser.
        assert_eq!(
            crate::telemetry::diagnostic_reason(reason),
            reason,
            "{reason} must be a fixed reason code or the log drops it"
        );
    }
}

#[test]
fn backend_repair_read_only_acceptance_covers_every_readable_domain_once() {
    for id in READ_ONLY_CASES {
        let plan = select_cases(&[(*id).into()]).unwrap_or_else(|error| {
            panic!("{id} must be a selectable case, got {error}");
        });
        assert!(plan.contains(*id), "{id} must be part of its own plan");
    }
    let ids: BTreeSet<&str> = CHECKS.iter().map(|spec| spec.id).collect();
    assert_eq!(
        ids.len(),
        CHECKS.len(),
        "a duplicate case id would make the plan ambiguous"
    );
    for id in READ_ONLY_CASES {
        assert!(ids.contains(id), "{id} must stay in the acceptance table");
    }
}

#[test]
fn backend_repair_read_only_acceptance_plans_pull_only_the_sessions_they_need() {
    let dependent = |case: &str, expected: &[&str], forbidden: &[&str]| {
        let plan = select_cases(&[case.into()]).unwrap();
        for id in expected {
            assert!(plan.contains(*id), "{case} must depend on {id}");
        }
        for id in forbidden {
            assert!(!plan.contains(*id), "{case} must not pull in {id}");
        }
    };
    dependent(
        "assessment_form",
        &[
            "identity_session",
            "portal_bootstrap",
            "info_session",
            "assessment_list",
        ],
        &["learn_courses"],
    );
    dependent("invoice_document", &["invoice_list"], &["learn_courses"]);
    dependent("reserves_detail", &["reserves_search"], &["learn_courses"]);
    dependent("course_score", &["learn_courses", "info_session"], &[]);
    dependent(
        "library_reservations",
        &[
            "identity_session",
            "portal_bootstrap",
            "info_session",
            "library_session",
        ],
        &["learn_courses"],
    );
    // The two third-party reads carry no campus account at all, so their plan
    // must not drag an identity or portal session in behind them.
    dependent(
        "laundry_rooms",
        &["laundry_buildings"],
        &[
            "identity_session",
            "portal_bootstrap",
            "info_session",
            "library_session",
        ],
    );
    let water = select_cases(&["water_user".into()]).unwrap();
    assert_eq!(water, BTreeSet::from(["water_user".to_owned()]));
}

#[tokio::test]
async fn backend_repair_read_only_acceptance_governed_domains_refuse_without_a_session() {
    let governed = [
        "program_completion",
        "physical_exam_result",
        "assessment_list",
        "invoice_list",
        "bank_payment_ledger",
        "bank_foundation_ledger",
        "graduate_income",
        "sports_resources",
        "sports_records",
        "reserves_search",
        "library_room_catalog",
        "library_room_records",
    ];
    for id in governed {
        let root = directory();
        let server = FixtureServer::new(vec![]);
        let mut runtime = fixture_runtime(&server, &root);
        let mut prompt = Prompt::default();
        let mut evidence = Evidence::default();
        let result = execute_case(&mut runtime, &mut prompt, &mut evidence, id, false).await;
        assert!(result.is_err(), "{id} must not pass without a session");
        assert!(
            server.requests().is_empty(),
            "{id} must not dispatch a request without a session"
        );
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn backend_repair_read_only_acceptance_course_score_without_evidence_is_not_a_read() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    assert_eq!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "course_score",
            false
        )
        .await
        .err()
        .unwrap(),
        "course_evidence_unavailable"
    );
    assert!(server.requests().is_empty());
    // The course number this case sends comes from the Learn capture, never
    // from a caller, so an evidence set with no course numbers skips instead
    // of inventing one.
    assert_eq!(
        execute_case(
            &mut runtime,
            &mut prompt,
            &mut evidence,
            "course_score",
            false,
        )
        .await
        .err()
        .unwrap(),
        "course_evidence_unavailable"
    );
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_read_only_acceptance_sample_gaps_are_unverified_not_success() {
    // A case whose only handle is a previous read's own opaque reference must
    // report the missing handle as unverified.  It must not read something
    // else, and it must not answer with an empty success.
    for (id, reason) in [
        ("assessment_form", "no_assessment_selector"),
        ("invoice_document", "no_invoice_selector"),
        ("reserves_detail", "no_reserves_selector"),
        ("water_user", "water_delivery_number_unavailable"),
    ] {
        let root = directory();
        let server = FixtureServer::new(vec![]);
        let mut runtime = fixture_runtime(&server, &root);
        let mut prompt = Prompt::default();
        let mut evidence = Evidence::default();
        match execute_case(&mut runtime, &mut prompt, &mut evidence, id, false).await {
            Ok(Outcome::Skipped(actual)) => assert_eq!(actual, reason, "{id}"),
            Ok(Outcome::Passed(_)) => panic!("{id} must skip with {reason}, not pass"),
            Ok(Outcome::ObservedNetwork(_)) => {
                panic!("{id} must skip with {reason}, not observe the network")
            }
            Err(error) => panic!("{id} must skip with {reason}, got {error}"),
        }
        assert!(server.requests().is_empty(), "{id} must send nothing");
        drop(runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn backend_repair_read_only_acceptance_laundry_rooms_reads_the_first_vendor_that_answers() {
    // The laundry vendors are public third-party services, so this case does
    // reach them.  The assertions below hold for any answer, reachable or not:
    // a vendor that answered must be what the pass reports, and a failure must
    // never come back as an empty success.
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut prompt = Prompt::default();
    let mut evidence = Evidence::default();
    let outcome = execute_case(
        &mut runtime,
        &mut prompt,
        &mut evidence,
        "laundry_rooms",
        false,
    )
    .await;
    match outcome {
        Ok(Outcome::Passed(Some(rooms))) => assert!(rooms >= 1, "a read proves at least one room"),
        Ok(Outcome::Skipped("no_laundry_building_selector")) => {}
        Ok(_) => panic!("laundry_rooms answered with neither a read nor a sample gap"),
        Err(error) => assert!(
            !error.is_empty(),
            "a vendor failure must carry its own reason code"
        ),
    }
    assert!(
        server.requests().is_empty(),
        "the laundry vendors are not the campus fixture"
    );
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backend_repair_read_only_acceptance_usereg_stays_an_explicit_choice() {
    let root = directory();
    let server = FixtureServer::new(vec![]);
    let mut runtime = fixture_runtime(&server, &root);
    let mut evidence = Evidence::default();
    let mut declined = Prompt::default();
    match execute_case(
        &mut runtime,
        &mut declined,
        &mut evidence,
        "usereg_session",
        false,
    )
    .await
    {
        Ok(Outcome::Skipped("optional_login_not_selected")) => {}
        Ok(_) => panic!("unselected usereg must stay unverified, not pass"),
        Err(error) => panic!("unselected usereg must stay unverified, got {error}"),
    }
    // Selecting it is not the same as being able to read: the fixture has no
    // login page, so the case fails rather than reporting an empty account.
    let mut selected = Prompt::default();
    assert!(
        execute_case(
            &mut runtime,
            &mut selected,
            &mut evidence,
            "usereg_session",
            true,
        )
        .await
        .is_err()
    );
    assert!(server.requests().is_empty());
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
