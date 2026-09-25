#[tokio::test]
async fn backend_repair_school_calendar_runtime_rejects_anonymous_read_before_public_http() {
    let base = unique_cache_base("school-calendar-anonymous");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".into(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    assert!(
        runtime
            .load_school_calendar(None, "autumn".into(), "zh".into())
            .await
            .is_err()
    );
    assert!(!runtime.service_session_is_proven(ServiceId::Learn));
    drop(runtime);
    remove_cache_file(base);
}
