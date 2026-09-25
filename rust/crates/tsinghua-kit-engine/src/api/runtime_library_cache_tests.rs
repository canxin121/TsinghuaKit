//! Loopback-only tests for library directory caches.

use super::*;

use crate::library_read::{LibraryAreaDto, LibraryDaySegmentDto, LibraryReadAdapter};
use crate::reference_test_support::{FixtureServer, Reply};
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use reqwest::Url;

fn cache_base(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("thyou-library-cache-{label}-{}", Uuid::new_v4()))
        .join("cache.json")
}

fn user(username: &str) -> UserIdentity {
    UserIdentity {
        username: username.to_owned(),
        display_name: None,
    }
}

fn install_identity(runtime: &mut CampusRuntime, account: &UserIdentity) {
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Identity, account.clone(), None, None, None)
        .expect("identity proof");
}

fn install_library(runtime: &mut CampusRuntime, account: &UserIdentity, server: &FixtureServer) {
    install_identity(runtime, account);
    runtime
        .coordinator
        .begin_authentication(ServiceId::Library)
        .expect("library begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Library, account.clone(), None, None, None)
        .expect("library proof");
    runtime.library_adapter = Some(
        LibraryReadAdapter::try_with_transport(
            Url::parse(server.base()).expect("library base"),
            runtime.identity.transport().clone(),
        )
        .expect("library adapter"),
    );
}

fn area(id: u64, name: &str, child_areas: Vec<LibraryAreaDto>) -> LibraryAreaDto {
    LibraryAreaDto {
        id,
        name: name.to_owned(),
        name_merge: None,
        english_name: None,
        english_name_merge: None,
        is_valid: Some(true),
        total_count: Some(10),
        unavailable_space: Some(1),
        available_count: Some(9),
        point_x: None,
        point_y: None,
        child_areas,
    }
}

fn segments(section_id: u64) -> LibraryDaySegmentsCachePayload {
    LibraryDaySegmentsCachePayload {
        account_scope: cache_account_scope("fixture-user"),
        section_id,
        generated_at: Utc::now(),
        segments: vec![LibraryDaySegmentDto {
            day: "2026-09-11".to_owned(),
            start_time: "08:00".to_owned(),
            end_time: "22:00".to_owned(),
            id: 9001,
        }],
    }
}

fn write_area_cache(
    base: &std::path::Path,
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let payload = LibraryAreaTreeCachePayload {
        account_scope: cache_account_scope(username),
        generated_at,
        areas: vec![area(351, "缓存阅览区", vec![area(352, "缓存楼层", vec![])])],
    };
    let path = library_area_cache_path(base, username);
    JsonFileCache::<LibraryAreaTreeCachePayload>::new(
        &path,
        LIBRARY_AREA_CACHE_SCHEMA_VERSION,
        LIBRARY_AREA_CACHE_SERVICE,
    )
    .write(&payload)
    .expect("area cache writes");
    path
}

#[test]
fn backend_repair_library_seat_start_tracks_campus_time_and_rejects_expired_windows() {
    let now = Utc.with_ymd_and_hms(2026, 9, 24, 4, 35, 42).unwrap();
    assert_eq!(
        library_seat_request_start_at("2026-09-24", "08:00", "22:00", now),
        Ok("12:35".to_owned())
    );
    assert_eq!(
        library_seat_request_start_at("2026-09-24", "13:00", "22:00", now),
        Ok("13:00".to_owned())
    );
    assert_eq!(
        library_seat_request_start_at("2026-09-25", "08:00", "22:00", now),
        Ok("08:00".to_owned())
    );
    assert_eq!(
        library_seat_request_start_at("2026-09-24", "08:00", "12:35", now),
        Err("图书馆该时段已结束，请选择其他时段")
    );
    assert_eq!(
        library_seat_request_start_at("2026-09-23", "08:00", "22:00", now),
        Err("图书馆查询日期已过期，请重新选择")
    );
    // 23:59 UTC is already the following campus day. OS-local time must not
    // move the library request back into yesterday's window.
    let rollover = Utc.with_ymd_and_hms(2026, 9, 24, 23, 59, 0).unwrap();
    assert_eq!(
        library_seat_request_start_at("2026-09-24", "08:00", "22:00", rollover),
        Err("图书馆查询日期已过期，请重新选择")
    );
    assert_eq!(
        library_seat_request_start_at("2026-09-25", "08:00", "22:00", rollover),
        Ok("08:00".to_owned())
    );
}

#[test]
fn backend_repair_library_campus_date_support_is_stable_at_midnight() {
    let before_midnight = Utc.with_ymd_and_hms(2026, 9, 24, 15, 59, 0).unwrap();
    let after_midnight = Utc.with_ymd_and_hms(2026, 9, 24, 16, 0, 0).unwrap();
    let september_24 = NaiveDate::from_ymd_opt(2026, 9, 24).unwrap();
    let september_25 = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
    let september_26 = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();

    assert!(library_campus_date_is_current_or_tomorrow(
        september_24,
        before_midnight
    ));
    assert!(library_campus_date_is_current_or_tomorrow(
        september_25,
        before_midnight
    ));
    assert!(!library_campus_date_is_current_or_tomorrow(
        september_26,
        before_midnight
    ));
    assert!(!library_campus_date_is_current_or_tomorrow(
        september_24,
        after_midnight
    ));
    assert!(library_campus_date_is_current_or_tomorrow(
        september_25,
        after_midnight
    ));
    assert!(library_campus_date_is_current_or_tomorrow(
        september_26,
        after_midnight
    ));
}

#[tokio::test]
async fn backend_repair_library_exact_campus_date_rejects_stale_selection_before_http() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("exact-campus-date");
    let account = user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_library(&mut runtime, &account, &server);
    let expired_day = campus_date_at(Utc::now()).pred_opt().unwrap();
    let expired_day = expired_day.format("%Y-%m-%d").to_string();

    assert!(
        runtime
            .load_library_sections_for_campus_date(351, expired_day.clone())
            .await
            .is_err()
    );
    assert!(
        runtime
            .load_library_day_segments_for_campus_date_result(353, expired_day)
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_library_seat_directory_binds_selected_day_and_seat() {
    let day = campus_date_at(Utc::now()).format("%Y-%m-%d").to_string();
    let previous_day = campus_date_at(Utc::now())
        .pred_opt()
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"data":{"list":{"childArea":[{"id":352,"name":"阅览楼层","isValid":1}]}}}"#,
        ),
        Reply::json(
            r#"{"data":{"list":{"childArea":[{"id":353,"name":"座位分区","isValid":1}]}}}"#,
        ),
        Reply::json(&format!(
            r#"{{"data":{{"list":[{{"id":9000,"day":"{previous_day}","startTime":{{"date":"1970-01-01 08:00:00"}},"endTime":{{"date":"1970-01-01 22:00:00"}}}},{{"id":9001,"day":"{day}","startTime":{{"date":"1970-01-01 08:00:00"}},"endTime":{{"date":"1970-01-01 22:00:00"}}}}]}}}}"#
        )),
    ]);
    let base = cache_base("seat-directory-levels");
    let account = user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    install_library(&mut runtime, &account, &server);
    runtime.remember_library_section_ids(&[area(351, "图书馆", vec![])]);

    assert!(runtime.load_library_floors(999).await.is_err());
    assert!(runtime.load_library_sections_for_day(351, 0).await.is_err());
    assert!(runtime.load_library_sections_for_day(352, 2).await.is_err());
    assert!(server.requests().is_empty());

    let floors = runtime.load_library_floors(351).await.unwrap();
    assert_eq!(floors.areas[0].id, 352);
    let sections = runtime.load_library_sections_for_day(352, 0).await.unwrap();
    assert_eq!(sections.areas[0].id, 353);
    assert!(runtime.load_library_day_segments_result(351).await.is_err());
    assert!(runtime.load_library_day_segments_result(352).await.is_err());
    assert_eq!(server.requests().len(), 2);

    let segments = runtime
        .load_library_day_segments_for_day_result(353, 0)
        .await
        .unwrap();
    assert_eq!(segments.segments.len(), 1);
    assert_eq!(segments.segments[0].day, day);
    assert!(
        runtime
            .load_library_seats(353, 9000, day.clone(), "08:00".into(), "22:00".into())
            .await
            .is_err()
    );
    assert!(
        runtime
            .load_library_seats(351, 9001, day.clone(), "08:00".into(), "22:00".into())
            .await
            .is_err()
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].contains("/api.php/areas/351 "));
    assert!(requests[1].contains(&format!("/api.php/areas/352/date/{day} ")));
    assert!(requests[2].contains("/api.php/areadays/353 "));
}

#[tokio::test]
async fn backend_repair_library_previous_day_cache_without_selected_date_forces_live_read() {
    let today = campus_date_at(Utc::now()).format("%Y-%m-%d").to_string();
    let previous_day = campus_date_at(Utc::now())
        .pred_opt()
        .unwrap()
        .format("%Y-%m-%d")
        .to_string();
    let server = FixtureServer::new(vec![Reply::json(&format!(
        r#"{{"data":{{"list":[{{"id":9001,"day":"{today}","startTime":{{"date":"1970-01-01 08:00:00"}},"endTime":{{"date":"1970-01-01 22:00:00"}}}}]}}}}"#
    ))]);
    let base = cache_base("previous-day-selection");
    let account = user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    install_library(&mut runtime, &account, &server);
    runtime.library_section_ids.insert(353);
    let campus_timezone = FixedOffset::east_opt(8 * 60 * 60).unwrap();
    let old_local = campus_date_at(Utc::now())
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(campus_timezone)
        .single()
        .unwrap()
        - ChronoDuration::minutes(5);
    let old_payload = LibraryDaySegmentsCachePayload {
        account_scope: cache_account_scope(&account.username),
        section_id: 353,
        generated_at: old_local.with_timezone(&Utc),
        segments: vec![LibraryDaySegmentDto {
            day: previous_day,
            start_time: "08:00".into(),
            end_time: "22:00".into(),
            id: 9000,
        }],
    };
    let cache_path = library_segment_cache_path(&base, &account.username, 353);
    JsonFileCache::<LibraryDaySegmentsCachePayload>::new(
        &cache_path,
        LIBRARY_SEGMENT_CACHE_SCHEMA_VERSION,
        LIBRARY_SEGMENT_CACHE_SERVICE,
    )
    .write(&old_payload)
    .unwrap();
    let result = runtime
        .load_library_day_segments_for_day_result(353, 0)
        .await
        .unwrap();
    assert_eq!(result.source, "live");
    assert_eq!(result.segments.len(), 1);
    assert_eq!(result.segments[0].day, today);
    assert_eq!(server.requests().len(), 1);
}

fn write_segment_cache(
    base: &std::path::Path,
    username: &str,
    section_id: u64,
    generated_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let mut payload = segments(section_id);
    payload.account_scope = cache_account_scope(username);
    payload.generated_at = generated_at;
    let path = library_segment_cache_path(base, username, section_id);
    JsonFileCache::<LibraryDaySegmentsCachePayload>::new(
        &path,
        LIBRARY_SEGMENT_CACHE_SCHEMA_VERSION,
        LIBRARY_SEGMENT_CACHE_SERVICE,
    )
    .write(&payload)
    .expect("segment cache writes");
    path
}

#[tokio::test]
async fn backend_repair_library_fresh_area_cache_skips_library_handoff() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("area-fresh");
    let account = user("fixture-user");
    let cache_path = write_area_cache(&base, &account.username, Utc::now());
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_identity(&mut runtime, &account);

    let result = runtime
        .load_library_area_tree_result()
        .await
        .expect("fresh area cache");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.areas[0].id, 351);
    assert!(runtime.library_section_ids.contains(&352));
    assert!(!runtime.service_session_is_proven(ServiceId::Library));
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[test]
fn backend_repair_library_area_cache_rejects_account_and_payload_drift() {
    let payload = LibraryAreaTreeCachePayload {
        account_scope: cache_account_scope("fixture-user"),
        generated_at: Utc::now(),
        areas: vec![area(351, "阅览区", vec![area(352, "楼层", vec![])])],
    };
    assert!(library_area_tree_cache_payload_is_valid(
        &payload,
        "fixture-user"
    ));
    assert!(!library_area_tree_cache_payload_is_valid(
        &payload,
        "other-user"
    ));
    let mut duplicate = payload.clone();
    duplicate.areas.push(area(351, "重复区域", vec![]));
    assert!(!library_area_tree_cache_payload_is_valid(
        &duplicate,
        "fixture-user"
    ));
    let mut invalid = payload;
    invalid.areas[0].name = "".to_owned();
    assert!(!library_area_tree_cache_payload_is_valid(
        &invalid,
        "fixture-user"
    ));
}

#[tokio::test]
async fn backend_repair_library_live_area_refresh_writes_normalized_cache() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"data":{"list":[{"id":351,"name":"实时阅览区","isValid":1,"childArea":[{"id":352,"name":"实时楼层","isValid":1}]}]}}"#,
    )]);
    let base = cache_base("area-live");
    let account = user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_library(&mut runtime, &account, &server);

    let result = runtime
        .load_library_area_tree_result()
        .await
        .expect("live area read");
    assert_eq!(result.source, "live");
    assert_eq!(result.areas[0].name, "实时阅览区");
    assert!(runtime.library_section_ids.contains(&352));
    let cache_path = library_area_cache_path(&base, &account.username);
    let envelope = JsonFileCache::<LibraryAreaTreeCachePayload>::new(
        &cache_path,
        LIBRARY_AREA_CACHE_SCHEMA_VERSION,
        LIBRARY_AREA_CACHE_SERVICE,
    )
    .read()
    .expect("area cache reads")
    .expect("area cache exists");
    assert_eq!(envelope.payload.areas[0].id, 351);
    assert_eq!(server.requests().len(), 1);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_library_stale_area_cache_falls_back_after_live_failure() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let base = cache_base("area-stale");
    let account = user("fixture-user");
    let cache_path = write_area_cache(
        &base,
        &account.username,
        Utc::now() - ChronoDuration::hours(13),
    );
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_library(&mut runtime, &account, &server);

    let result = runtime
        .load_library_area_tree_result()
        .await
        .expect("stale area cache fallback");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert_eq!(server.requests().len(), 1);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_library_fresh_segments_cache_skips_inventory_request() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("segments-fresh");
    let account = user("fixture-user");
    let cache_path = write_segment_cache(&base, &account.username, 351, Utc::now());
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_identity(&mut runtime, &account);
    runtime.library_section_ids.insert(351);

    let result = runtime
        .load_library_day_segments_result(351)
        .await
        .expect("fresh segment cache");
    assert_eq!(result.source, "cache");
    assert_eq!(result.section_id, 351);
    assert_eq!(result.segments[0].id, 9001);
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_library_segments_cache_survives_new_runtime_without_directory_handoff() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"data":{"list":[{"day":"2026-09-11","startTime":{"date":"2026-09-11 08:00:00"},"endTime":{"date":"2026-09-11 22:00:00"},"id":"9001"}]}}"#,
    )]);
    let base = cache_base("segments-cross-runtime");
    let account = user("fixture-user");

    let mut writer = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("writer config");
    install_library(&mut writer, &account, &server);
    writer.library_section_ids.insert(351);
    let live = writer
        .load_library_day_segments_result(351)
        .await
        .expect("writer live segment read");
    assert_eq!(live.source, "live");
    assert_eq!(server.requests().len(), 1);

    // This Runtime has the same account identity but no Library proof and no
    // hydrated in-memory section directory. A durable, account-bound cache
    // must still be readable without starting a new handoff or HTTP request.
    let mut reader = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("reader config");
    install_identity(&mut reader, &account);
    let cached = reader
        .load_library_day_segments_result(351)
        .await
        .expect("reader cache segment");
    assert_eq!(cached.source, "cache");
    assert_eq!(cached.status, "ready");
    assert_eq!(cached.section_id, 351);
    assert_eq!(cached.segments[0].id, 9001);
    assert!(reader.library_section_ids.is_empty());
    assert_eq!(server.requests().len(), 1);

    let _ = std::fs::remove_file(library_segment_cache_path(&base, &account.username, 351));
}

#[tokio::test]
async fn backend_repair_library_live_segments_write_and_stale_fallback_are_explicit() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"data":{"list":[{"day":"2026-09-11","startTime":{"date":"2026-09-11 08:00:00"},"endTime":{"date":"2026-09-11 22:00:00"},"id":"9001"}]}}"#,
    )]);
    let base = cache_base("segments-live");
    let account = user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_library(&mut runtime, &account, &server);
    runtime.library_section_ids.insert(351);

    let result = runtime
        .load_library_day_segments_result(351)
        .await
        .expect("live segment read");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    let cache_path = library_segment_cache_path(&base, &account.username, 351);
    assert!(cache_path.is_file());
    assert_eq!(server.requests().len(), 1);
    let _ = std::fs::remove_file(cache_path);
}
