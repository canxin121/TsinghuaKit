//! Loopback-only tests for the remaining slow-changing service caches.
//!
//! These tests deliberately keep the live proof setup inside Rust. No account
//! credential, Cookie, card serial, or response body is written to the cache;
//! the assertions inspect only normalized fixture values and fixed provenance.

use super::*;

use crate::reference_test_support::{FixtureServer, Reply};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use std::path::Path;

fn cache_base(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("thyou-slow-cache-{label}-{}", Uuid::new_v4()))
        .join("cache.json")
}

fn user(username: &str) -> UserIdentity {
    UserIdentity {
        username: username.to_owned(),
        display_name: None,
    }
}

fn runtime(base: &Path) -> CampusRuntime {
    CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config")
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

fn install_info(runtime: &mut CampusRuntime, account: &UserIdentity, server: &FixtureServer) {
    install_identity(runtime, account);
    install_info_proof_only(runtime, account, server);
}

fn install_info_proof_only(
    runtime: &mut CampusRuntime,
    account: &UserIdentity,
    server: &FixtureServer,
) {
    runtime
        .coordinator
        .begin_authentication(ServiceId::Info)
        .expect("info begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Info, account.clone(), None, None, None)
        .expect("info proof");
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/target").expect("info config"),
            runtime.identity.transport().clone(),
        )
        .expect("info adapter"),
    );
    runtime.info_roaming_url = Some(
        crate::info::OpaqueUrl::new(format!("{}target/home", server.base()))
            .expect("info roaming URL"),
    );
}

fn classroom_payload(
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> ClassroomBuildingCachePayload {
    ClassroomBuildingCachePayload {
        account_scope: cache_account_scope(username),
        generated_at,
        buildings: vec![ClassroomBuildingCacheRecord {
            name: "缓存教学楼".to_owned(),
            week_number: 3,
        }],
    }
}

fn write_classroom_cache(
    base: &Path,
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let path = classroom_building_cache_path(base, username);
    JsonFileCache::<ClassroomBuildingCachePayload>::new(
        &path,
        CLASSROOM_BUILDING_CACHE_SCHEMA_VERSION,
        CLASSROOM_BUILDING_CACHE_SERVICE,
    )
    .write(&classroom_payload(username, generated_at))
    .expect("classroom cache writes");
    path
}

fn electricity_payload(
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> ElectricityPaymentHistoryCachePayload {
    ElectricityPaymentHistoryCachePayload {
        account_scope: cache_account_scope(username),
        generated_at,
        records: vec![ElectricityPaymentHistoryCacheRecord {
            source_columns: vec![
                "room".to_owned(),
                "7".to_owned(),
                "2026-09-14 10:11:12".to_owned(),
                "payment".to_owned(),
                "20.50".to_owned(),
                "成功".to_owned(),
            ],
            sequence: Some(7),
            occurred_at: "2026-09-14 10:11:12".to_owned(),
            amount: 20.50,
            status: "成功".to_owned(),
        }],
    }
}

fn write_electricity_cache(
    base: &Path,
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let path = electricity_history_cache_path(base, username);
    JsonFileCache::<ElectricityPaymentHistoryCachePayload>::new(
        &path,
        ELECTRICITY_HISTORY_CACHE_SCHEMA_VERSION,
        ELECTRICITY_HISTORY_CACHE_SERVICE,
    )
    .write(&electricity_payload(username, generated_at))
    .expect("electricity cache writes");
    path
}

fn electricity_remainder_payload(
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> ElectricityRemainderCachePayload {
    ElectricityRemainderCachePayload {
        account_scope: cache_account_scope(username),
        generated_at,
        remainder: 12.34,
        update_time: "2026-09-14 09:00:00".to_owned(),
    }
}

fn write_electricity_remainder_cache(
    base: &Path,
    username: &str,
    generated_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let path = electricity_remainder_cache_path(base, username);
    JsonFileCache::<ElectricityRemainderCachePayload>::new(
        &path,
        ELECTRICITY_REMAINDER_CACHE_SCHEMA_VERSION,
        ELECTRICITY_REMAINDER_CACHE_SERVICE,
    )
    .write(&electricity_remainder_payload(username, generated_at))
    .expect("electricity remainder cache writes");
    path
}

fn card_transaction() -> crate::campus_card_read::CampusCardTransaction {
    crate::campus_card_read::CampusCardTransaction {
        transaction_id: "tx-17".to_owned(),
        summary: "fixture meal".to_owned(),
        occurred_at: "2026-09-17 12:00:00".to_owned(),
        post_balance_cents: 12000,
        amount_cents: -345,
        merchant_address: String::new(),
        merchant_name: Some("食堂".to_owned()),
        transaction_name: "消费".to_owned(),
    }
}

fn card_payload(
    username: &str,
    start_date: &str,
    end_date: &str,
    transaction_type: &str,
    generated_at: chrono::DateTime<Utc>,
) -> CampusCardTransactionCachePayload {
    CampusCardTransactionCachePayload {
        account_scope: cache_account_scope(username),
        start_date: start_date.to_owned(),
        end_date: end_date.to_owned(),
        transaction_type: transaction_type.to_owned(),
        generated_at,
        transactions: vec![card_transaction()],
    }
}

fn write_card_cache(
    base: &Path,
    username: &str,
    start_date: &str,
    end_date: &str,
    transaction_type: &str,
    generated_at: chrono::DateTime<Utc>,
) -> std::path::PathBuf {
    let path =
        campus_card_transaction_cache_path(base, username, start_date, end_date, transaction_type);
    JsonFileCache::<CampusCardTransactionCachePayload>::new(
        &path,
        CAMPUS_CARD_TRANSACTION_CACHE_SCHEMA_VERSION,
        CAMPUS_CARD_TRANSACTION_CACHE_SERVICE,
    )
    .write(&card_payload(
        username,
        start_date,
        end_date,
        transaction_type,
        generated_at,
    ))
    .expect("card cache writes");
    path
}

#[tokio::test]
async fn backend_repair_classroom_fresh_cache_skips_handoff_and_keeps_matrix_live_only() {
    let base = cache_base("classroom-fresh");
    let account = user("fixture-user");
    let cache_path = write_classroom_cache(&base, &account.username, Utc::now());
    let server = FixtureServer::new(Vec::new());
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);

    let result = runtime
        .load_classroom_buildings_result()
        .await
        .expect("fresh classroom directory");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.buildings[0].name, "缓存教学楼");
    assert!(!runtime.classroom_service_is_proven());
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_classroom_matrix_rejects_cached_directory_drift_before_dispatch() {
    let base = cache_base("classroom-directory-drift");
    let account = user("fixture-user");
    let server = FixtureServer::new(Vec::new());
    let mut runtime = runtime(&base);
    install_info(&mut runtime, &account, &server);
    runtime.classroom_adapter = Some(
        ClassroomReadAdapter::try_with_transport(
            reqwest::Url::parse(server.base()).expect("classroom fixture URL"),
            runtime.identity.transport().clone(),
        )
        .expect("classroom adapter"),
    );
    runtime.classroom_buildings = Some(ClassroomBuildingList::with_proof(
        vec![
            crate::classroom_read::ClassroomBuilding {
                name: "实时教学楼".to_owned(),
                search_name: "REAL".to_owned(),
                week_number: 1,
            },
            crate::classroom_read::ClassroomBuilding {
                name: "另一教学楼".to_owned(),
                search_name: "OTHER".to_owned(),
                week_number: 2,
            },
        ],
        1,
    ));
    runtime.classroom_cached_directory = Some(vec![
        ClassroomBuildingCacheRecord {
            name: "实时教学楼".to_owned(),
            week_number: 1,
        },
        ClassroomBuildingCacheRecord {
            name: "另一教学楼".to_owned(),
            week_number: 3,
        },
    ]);

    let error = runtime
        .load_classroom_state(0, 1)
        .await
        .expect_err("a changed cached directory must fail closed");
    assert_eq!(error, "楼栋列表已更新，请先刷新楼栋列表");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_classroom_matrix_allows_matching_cached_directory() {
    let base = cache_base("classroom-directory-match");
    let account = user("fixture-user");
    let server = FixtureServer::new(Vec::new());
    let mut runtime = runtime(&base);
    install_info(&mut runtime, &account, &server);
    runtime.classroom_adapter = Some(
        ClassroomReadAdapter::try_with_transport(
            reqwest::Url::parse(server.base()).expect("classroom fixture URL"),
            runtime.identity.transport().clone(),
        )
        .expect("classroom adapter"),
    );
    runtime.classroom_buildings = Some(ClassroomBuildingList::with_proof(
        vec![crate::classroom_read::ClassroomBuilding {
            name: "实时教学楼".to_owned(),
            search_name: "REAL".to_owned(),
            week_number: 1,
        }],
        1,
    ));
    runtime.classroom_cached_directory = Some(vec![ClassroomBuildingCacheRecord {
        name: "实时教学楼".to_owned(),
        week_number: 1,
    }]);

    let error = runtime
        .load_classroom_state(0, 1)
        .await
        .expect_err("the fixture has no state response");
    assert_ne!(error, "楼栋列表已更新，请先刷新楼栋列表");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn backend_repair_classroom_cache_rejects_account_and_directory_drift() {
    let payload = classroom_payload("fixture-user", Utc::now());
    assert!(classroom_building_cache_payload_is_valid(
        &payload,
        "fixture-user"
    ));
    assert!(!classroom_building_cache_payload_is_valid(
        &payload,
        "other-user"
    ));
    let mut invalid = payload.clone();
    invalid.buildings[0].name.clear();
    assert!(!classroom_building_cache_payload_is_valid(
        &invalid,
        "fixture-user"
    ));
    let mut invalid_week = payload;
    invalid_week.buildings[0].week_number = 257;
    assert!(!classroom_building_cache_payload_is_valid(
        &invalid_week,
        "fixture-user"
    ));
}

#[tokio::test]
async fn backend_repair_classroom_live_directory_writes_normalized_cache() {
    let server = FixtureServer::new(vec![Reply::html(
        r#"<html><div class="w30"><a href="/http/fixture/pk.classroomctrl.do?m=qyClassroomState&amp;classroom=FIT&amp;weeknumber=3">实时教学楼</a></div></html>"#,
    )]);
    let base = cache_base("classroom-live");
    let account = user("fixture-user");
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);
    install_info_proof_only(&mut runtime, &account, &server);
    let adapter = ClassroomReadAdapter::try_with_transport(
        reqwest::Url::parse(server.base()).expect("classroom URL"),
        runtime.identity.transport().clone(),
    )
    .expect("classroom adapter");
    let buildings = adapter.read_buildings().await.expect("live directory");
    runtime.classroom_adapter = Some(adapter);
    runtime.classroom_buildings = Some(buildings);

    let result = runtime
        .load_classroom_buildings_result()
        .await
        .expect("live classroom directory");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    let cache_path = classroom_building_cache_path(&base, &account.username);
    let raw = std::fs::read_to_string(&cache_path).expect("classroom cache exists");
    assert!(!raw.contains("search_name"));
    assert!(raw.contains("实时教学楼"));
    assert_eq!(server.requests().len(), 1);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_classroom_stale_cache_falls_back_after_live_failure() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let base = cache_base("classroom-stale");
    let account = user("fixture-user");
    let cache_path = write_classroom_cache(
        &base,
        &account.username,
        Utc::now() - ChronoDuration::hours(13),
    );
    let mut runtime = runtime(&base);
    install_info(&mut runtime, &account, &server);

    let result = runtime
        .load_classroom_buildings_result()
        .await
        .expect("stale classroom fallback");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert!(!server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_electricity_fresh_history_cache_skips_remainder_and_handoff() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("electricity-fresh");
    let account = user("fixture-user");
    let cache_path = write_electricity_cache(&base, &account.username, Utc::now());
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);

    let result = runtime
        .load_electricity_payment_history_result()
        .await
        .expect("fresh electricity history");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.records.len(), 1);
    assert!(runtime.electricity_adapter.is_none());
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_electricity_fresh_remainder_cache_skips_handoff() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("electricity-remainder-fresh");
    let account = user("fixture-user");
    let cache_path = write_electricity_remainder_cache(&base, &account.username, Utc::now());
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);

    let result = runtime
        .load_electricity_remainder_result()
        .await
        .expect("fresh electricity remainder");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert!((result.remainder - 12.34).abs() < f64::EPSILON);
    assert!(runtime.electricity_adapter.is_none());
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[test]
fn backend_repair_electricity_remainder_cache_rejects_time_and_account_drift() {
    let payload = electricity_remainder_payload("fixture-user", Utc::now());
    assert!(electricity_remainder_cache_payload_is_valid(
        &payload,
        "fixture-user"
    ));
    assert!(!electricity_remainder_cache_payload_is_valid(
        &payload,
        "other-user"
    ));
    let mut future = payload.clone();
    future.generated_at = Utc::now() + ChronoDuration::hours(1);
    assert!(!electricity_remainder_cache_payload_is_valid(
        &future,
        "fixture-user"
    ));
    assert!(!electricity_remainder_cache_is_usable(&future.generated_at));
    let old = electricity_remainder_payload("fixture-user", Utc::now() - ChronoDuration::hours(25));
    assert!(electricity_remainder_cache_payload_is_valid(
        &old,
        "fixture-user"
    ));
    assert!(!electricity_remainder_cache_is_usable(&old.generated_at));
    let mut invalid_number = payload.clone();
    invalid_number.remainder = f64::NAN;
    assert!(!electricity_remainder_cache_payload_is_valid(
        &invalid_number,
        "fixture-user"
    ));
    let mut invalid_update_time = payload;
    invalid_update_time.update_time.clear();
    assert!(!electricity_remainder_cache_payload_is_valid(
        &invalid_update_time,
        "fixture-user"
    ));
}

#[tokio::test]
async fn backend_repair_electricity_live_remainder_writes_account_cache() {
    let server = FixtureServer::new(vec![Reply::html(
        r#"<html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">12.34</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-09-14 09:00:00</span></html>"#,
    )]);
    let base = cache_base("electricity-remainder-live");
    let account = user("fixture-user");
    let mut runtime = runtime(&base);
    install_info(&mut runtime, &account, &server);
    runtime
        .establish_electricity_read_at(reqwest::Url::parse(server.base()).expect("electricity URL"))
        .await
        .expect("electricity proof");

    let result = runtime
        .load_electricity_remainder_result()
        .await
        .expect("live electricity remainder");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    let cache_path = electricity_remainder_cache_path(&base, &account.username);
    let raw = std::fs::read_to_string(&cache_path).expect("remainder cache exists");
    assert!(raw.contains("12.34"));
    assert!(raw.contains("2026-09-14 09:00:00"));
    assert_eq!(server.requests().len(), 1);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_electricity_stale_remainder_falls_back_after_live_failure() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("electricity-remainder-stale");
    let account = user("fixture-user");
    let cache_path = write_electricity_remainder_cache(
        &base,
        &account.username,
        Utc::now() - ChronoDuration::hours(7),
    );
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new("http://127.0.0.1:9/", "/target").expect("info config"),
            runtime.identity.transport().clone(),
        )
        .expect("info adapter"),
    );
    runtime.info_roaming_url = Some(
        crate::info::OpaqueUrl::new("http://127.0.0.1:9/target/home").expect("info roaming URL"),
    );
    runtime
        .coordinator
        .begin_authentication(ServiceId::Info)
        .expect("info begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Info, account, None, None, None)
        .expect("info proof");

    let result = runtime
        .load_electricity_remainder_result()
        .await
        .expect("stale electricity remainder fallback");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert!((result.remainder - 12.34).abs() < f64::EPSILON);
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[test]
fn backend_repair_electricity_history_cache_rejects_account_and_record_drift() {
    let payload = electricity_payload("fixture-user", Utc::now());
    assert!(electricity_history_cache_payload_is_valid(
        &payload,
        "fixture-user"
    ));
    assert!(!electricity_history_cache_payload_is_valid(
        &payload,
        "other-user"
    ));
    let mut invalid = payload.clone();
    invalid.records[0].source_columns.pop();
    assert!(!electricity_history_cache_payload_is_valid(
        &invalid,
        "fixture-user"
    ));
    let mut invalid_number = payload;
    invalid_number.records[0].amount = f64::NAN;
    assert!(!electricity_history_cache_payload_is_valid(
        &invalid_number,
        "fixture-user"
    ));
}

#[tokio::test]
async fn backend_repair_electricity_live_history_writes_normalized_cache_only() {
    let server = FixtureServer::new(vec![
        Reply::html(
            r#"<html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">12.34</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-09-14 09:00:00</span></html>"#,
        ),
        Reply::html(
            r#"<html><table class="myTable"><tr><th>header</th></tr><tr data-raw-only-marker="raw-only-marker"><td>room</td><td>7</td><td>2026-09-14 10:11:12</td><td>payment</td><td>20.50</td><td>成功</td></tr><tr><td>footer</td></tr></table></html>"#,
        ),
    ]);
    let base = cache_base("electricity-live");
    let account = user("fixture-user");
    let mut runtime = runtime(&base);
    install_info(&mut runtime, &account, &server);
    runtime
        .establish_electricity_read_at(reqwest::Url::parse(server.base()).expect("electricity URL"))
        .await
        .expect("electricity proof");

    let result = runtime
        .load_electricity_payment_history_result()
        .await
        .expect("live electricity history");
    assert_eq!(result.source, "live");
    assert_eq!(result.records.len(), 1);
    let cache_path = electricity_history_cache_path(&base, &account.username);
    let raw = std::fs::read_to_string(&cache_path).expect("electricity cache exists");
    assert!(raw.contains("20.5"));
    assert!(!raw.contains("raw-only-marker"));
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_electricity_stale_history_falls_back_after_live_failure() {
    let server = FixtureServer::new(vec![
        Reply::html(
            r#"<html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">12.34</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-09-14 09:00:00</span></html>"#,
        ),
        Reply {
            status: 503,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let base = cache_base("electricity-stale");
    let account = user("fixture-user");
    let cache_path = write_electricity_cache(
        &base,
        &account.username,
        Utc::now() - ChronoDuration::hours(7),
    );
    let mut runtime = runtime(&base);
    install_info(&mut runtime, &account, &server);
    runtime
        .establish_electricity_read_at(reqwest::Url::parse(server.base()).expect("electricity URL"))
        .await
        .expect("electricity proof");

    let result = runtime
        .load_electricity_payment_history_result()
        .await
        .expect("stale electricity fallback");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_file(cache_path);
}

fn card_runtime(base: &Path, server: &FixtureServer) -> (CampusRuntime, UserIdentity) {
    let account = user("2026000001");
    let mut runtime = runtime(base);
    install_identity(&mut runtime, &account);
    runtime.card_client = Some(
        CampusCardClient::new(
            CampusCardAdapterConfig::new(server.base()).expect("card config"),
            runtime.identity.transport().clone(),
        )
        .expect("card client"),
    );
    (runtime, account)
}

fn card_reply() -> Reply {
    Reply::json(
        &json!({
            "success": true,
            "resultData": {
                "rows": [{
                    "id": "tx-17",
                    "summary": "fixture meal",
                    "txdate": "2026-09-17 12:00:00",
                    "balance": 12000,
                    "txamt": -345,
                    "meraddr": "",
                    "mername": "食堂",
                    "txname": "消费",
                    "rawOnly": "raw-card-marker"
                }]
            }
        })
        .to_string(),
    )
}

#[tokio::test]
async fn backend_repair_card_fresh_transaction_cache_skips_card_session() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("card-fresh");
    let account = user("2026000001");
    let cache_path = write_card_cache(
        &base,
        &account.username,
        "2026-09-17",
        "2026-09-17",
        "consumption",
        Utc::now(),
    );
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);

    let result = runtime
        .load_campus_card_transactions_result(
            "2026-09-17".to_owned(),
            "2026-09-17".to_owned(),
            "consumption".to_owned(),
        )
        .await
        .expect("fresh card history");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.transaction_type, "consumption");
    assert!(runtime.card_session.is_none());
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[test]
fn backend_repair_card_transaction_cache_rejects_account_query_and_type_drift() {
    let payload = card_payload(
        "2026000001",
        "2026-09-17",
        "2026-09-17",
        "consumption",
        Utc::now(),
    );
    let kind = crate::campus_card_read::CampusCardTransactionType::Consumption;
    assert!(campus_card_transaction_cache_payload_is_valid(
        &payload,
        "2026000001",
        "2026-09-17",
        "2026-09-17",
        kind,
    ));
    assert!(!campus_card_transaction_cache_payload_is_valid(
        &payload,
        "2026000002",
        "2026-09-17",
        "2026-09-17",
        kind,
    ));
    assert!(!campus_card_transaction_cache_payload_is_valid(
        &payload,
        "2026000001",
        "2026-09-16",
        "2026-09-17",
        kind,
    ));
    assert!(!campus_card_transaction_cache_payload_is_valid(
        &payload,
        "2026000001",
        "2026-09-17",
        "2026-09-17",
        crate::campus_card_read::CampusCardTransactionType::Recharge,
    ));
    assert_ne!(
        campus_card_transaction_cache_path(
            Path::new("/tmp/cache.json"),
            "2026000001",
            "2026-09-17",
            "2026-09-17",
            "consumption",
        ),
        campus_card_transaction_cache_path(
            Path::new("/tmp/cache.json"),
            "2026000001",
            "2026-09-17",
            "2026-09-17",
            "recharge",
        )
    );
}

#[tokio::test]
async fn backend_repair_card_live_transactions_write_normalized_cache_only() {
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"2026000001"}}"#),
        card_reply(),
    ]);
    let base = cache_base("card-live");
    let (mut runtime, account) = card_runtime(&base, &server);
    runtime
        .probe_and_bind_card_session(&account)
        .await
        .expect("card proof");

    let result = runtime
        .load_campus_card_transactions_result(
            "2026-09-17".to_owned(),
            "2026-09-17".to_owned(),
            "consumption".to_owned(),
        )
        .await
        .expect("live card history");
    assert_eq!(result.source, "live");
    assert_eq!(result.transactions.len(), 1);
    let cache_path = campus_card_transaction_cache_path(
        &base,
        &account.username,
        "2026-09-17",
        "2026-09-17",
        "consumption",
    );
    let raw = std::fs::read_to_string(&cache_path).expect("card cache exists");
    assert!(raw.contains("tx-17"));
    assert!(!raw.contains("raw-card-marker"));
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_card_stale_transactions_fall_back_after_live_failure() {
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"2026000001"}}"#),
        Reply {
            status: 503,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let base = cache_base("card-stale");
    let account = user("2026000001");
    let cache_path = write_card_cache(
        &base,
        &account.username,
        "2026-09-17",
        "2026-09-17",
        "consumption",
        Utc::now() - ChronoDuration::hours(1),
    );
    let (mut runtime, account) = card_runtime(&base, &server);
    runtime
        .probe_and_bind_card_session(&account)
        .await
        .expect("card proof");

    let result = runtime
        .load_campus_card_transactions_result(
            "2026-09-17".to_owned(),
            "2026-09-17".to_owned(),
            "consumption".to_owned(),
        )
        .await
        .expect("stale card fallback");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_card_stale_transactions_use_existing_expiry_gate_once() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("card-stale-expiry-gate");
    let account = user("2026000001");
    let cache_path = write_card_cache(
        &base,
        &account.username,
        "2026-09-17",
        "2026-09-17",
        "consumption",
        Utc::now() - ChronoDuration::hours(1),
    );
    let mut runtime = runtime(&base);
    install_identity(&mut runtime, &account);
    runtime
        .coordinator
        .begin_authentication(ServiceId::CampusCard)
        .expect("expired card context begins");
    runtime
        .coordinator
        .mark_authenticated(
            ServiceId::CampusCard,
            account.clone(),
            None,
            None,
            Some(Utc::now() - ChronoDuration::seconds(1)),
        )
        .expect("expired card context proof");
    runtime.automatic_refresh_results.insert(
        ServiceId::CampusCard,
        Err("synthetic-card-refresh-failure".to_owned()),
    );

    let result = runtime
        .load_campus_card_transactions_result(
            "2026-09-17".to_owned(),
            "2026-09-17".to_owned(),
            "consumption".to_owned(),
        )
        .await
        .expect("stale card cache remains available after bounded refresh failure");

    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert!(
        runtime
            .automatic_refresh_results
            .contains_key(&ServiceId::CampusCard)
    );
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}
