use chrono::{DateTime, Utc};
use tsinghua_kit::{
    CampusDataSource, DateRange, InMemoryCampusService, NewTodo, TodoFilter, TodoPriority,
    TodoStatus,
};

#[tokio::test]
async fn in_memory_fixture_and_legacy_overview_bridge_fail_closed_in_production() {
    let service = InMemoryCampusService::empty();
    let source: &dyn CampusDataSource = &service;

    let create_result = source
        .create_todo(NewTodo {
            title: "fixture write".to_owned(),
            description: Some("production boundary check".to_owned()),
            priority: TodoPriority::High,
            due_at: Some(
                DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("valid timestamp"),
            ),
            course_id: None,
        })
        .await;
    assert!(
        matches!(create_result, Err(tsinghua_kit::ServiceError::Adapter { message }) if message == "in-memory campus fixture is available only in Rust tests")
    );

    assert!(matches!(
        source.set_todo_status(uuid::Uuid::nil(), TodoStatus::InProgress).await,
        Err(tsinghua_kit::ServiceError::Adapter { message })
            if message == "in-memory campus fixture is available only in Rust tests"
    ));

    assert!(matches!(
        source.list_todos(TodoFilter::default()).await,
        Err(tsinghua_kit::ServiceError::Adapter { message })
            if message == "in-memory campus fixture is available only in Rust tests"
    ));
    assert!(matches!(
        source.list_courses().await,
        Err(tsinghua_kit::ServiceError::Adapter { message })
            if message == "in-memory campus fixture is available only in Rust tests"
    ));
    assert!(matches!(
        source
            .list_schedule(DateRange::for_date(
                chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid date")
            )
            .expect("valid range"))
            .await,
        Err(tsinghua_kit::ServiceError::Adapter { message })
            if message == "in-memory campus fixture is available only in Rust tests"
    ));
    assert!(matches!(
        source
            .campus_overview(
                chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid date")
            )
            .await,
        Err(tsinghua_kit::ServiceError::Adapter { message })
            if message == "in-memory campus fixture is available only in Rust tests"
    ));
    assert!(matches!(
        service.snapshot(),
        Err(tsinghua_kit::ServiceError::Adapter { message })
            if message == "in-memory campus fixture is available only in Rust tests"
    ));

    let legacy_overview = tsinghua_kit::api::load_overview("2026-09-11".to_owned())
        .await
        .expect_err("legacy bridge must fail closed instead of returning fixture data");
    assert_eq!(
        legacy_overview,
        "legacy campus overview bridge is unavailable; use CampusRuntime::load_overview"
    );

    let invalid_range = DateRange::new(
        DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("valid timestamp"),
        DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("valid timestamp"),
    );
    assert!(invalid_range.is_err());
}
