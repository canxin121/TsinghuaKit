use super::*;

fn check(id: &str) -> &'static CheckSpec {
    CHECKS.iter().find(|s| s.id == id).unwrap()
}

#[test]
fn backend_repair_latency_ready_courses_precede_unrelated_service_authentication() {
    let ready = vec![
        check("library_session"),
        check("campus_card_session"),
        check("learn_courses"),
        check("info_session"),
    ];
    let selected = schedule::ready_batch(ready, ExecutionOptions::default());
    assert_eq!(
        selected.iter().map(|s| s.id).collect::<Vec<_>>(),
        ["learn_courses"]
    );
}

#[test]
fn backend_repair_latency_auth_completion_releases_next_priority_without_whole_batch_barrier() {
    let ready = vec![check("identity_session"), check("tunet_status")];
    let selected = schedule::ready_batch(ready, ExecutionOptions::default());
    assert_eq!(
        selected.iter().map(|s| s.id).collect::<Vec<_>>(),
        ["identity_session"]
    );
    let ready = vec![
        check("campus_card_session"),
        check("info_session"),
        check("library_session"),
    ];
    assert_eq!(
        schedule::ready_batch(ready, ExecutionOptions::default()).len(),
        1
    );
}

#[test]
fn backend_repair_latency_visible_data_still_admits_bounded_concurrent_readers() {
    let ready = vec![
        check("campus_card_account"),
        check("electricity_remainder"),
        check("info_news"),
        check("library_area_tree"),
        check("classroom_buildings"),
    ];
    assert_eq!(
        schedule::ready_batch(ready, ExecutionOptions::default()).len(),
        4
    );
}

#[test]
fn backend_repair_latency_serial_diagnostic_mode_preserves_user_selected_order() {
    let selected = schedule::ready_batch(
        vec![check("tunet_status"), check("learn_courses")],
        ExecutionOptions {
            mode: ExecutionMode::Serial,
            concurrency: 1,
        },
    );
    assert_eq!(selected[0].id, "tunet_status");
}

#[test]
fn backend_repair_latency_report_exposes_true_http_bound_and_zero_forced_gap() {
    let summary = ExecutionSummary::new(ExecutionOptions::default());
    assert_eq!(summary.runtime_parallelism_limit, 1);
    assert_eq!(summary.http_read_parallelism_limit, 4);
    // The isolated test environment does not set an explicit pacing override.
    assert_eq!(summary.fixed_request_gap_ms, 0);
}
