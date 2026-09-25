//! Narrow additional contracts; no old passed test is repeated.
use crate::reference_test_support::{FixtureServer, Reply};
use crate::{
    learn_client::{LearnClient, LearnClientConfig},
    protocol::{CourseRole, CsrfToken, ServiceId},
    session::SessionRegistry,
    transport::CampusHttpTransport,
};

#[tokio::test]
async fn backend_repair_deep_api_learn_consistent_flags_preserve_verified_empty_result() {
    let s = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","success":true,"status":200,"resultCode":0,"resultList":[]}"#,
    )]);
    let client = LearnClient::new(LearnClientConfig::new(s.base(), CourseRole::Student).unwrap());
    let token = SessionRegistry::new()
        .bind_csrf(ServiceId::Learn, CsrfToken::new("synthetic-csrf").unwrap());
    assert!(
        client
            .fetch_course_records(
                &CampusHttpTransport::new("THYou/deep-audit").unwrap(),
                &token,
                "2026-2027-1",
                Some("zh"),
                None
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.requests().len(), 1);
}

#[cfg(unix)]
#[test]
fn backend_repair_deep_api_atomic_cache_replacement_drops_old_world_read_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("thyou-cache-replace-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cache.json");
    std::fs::write(&path, "old synthetic value").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cache = crate::cache::JsonFileCache::<serde_json::Value>::new(&path, 1, "fixture");
    cache
        .write(serde_json::json!({"replacement":true}))
        .unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(cache.read().unwrap().unwrap().payload["replacement"], true);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn backend_repair_deep_api_learn_conflicting_success_flags_are_not_valid_data() {
    for body in [
        r#"{"message":"success","success":false,"resultList":[]}"#,
        r#"{"message":"success","status":503,"resultList":[]}"#,
    ] {
        let s = FixtureServer::new(vec![Reply::json(body)]);
        let client =
            LearnClient::new(LearnClientConfig::new(s.base(), CourseRole::Student).unwrap());
        let token = SessionRegistry::new()
            .bind_csrf(ServiceId::Learn, CsrfToken::new("synthetic-csrf").unwrap());
        assert!(
            client
                .fetch_course_records(
                    &CampusHttpTransport::new("THYou/deep-audit").unwrap(),
                    &token,
                    "2026-2027-1",
                    Some("zh"),
                    None
                )
                .await
                .is_err()
        );
        assert_eq!(s.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_repair_deep_api_learn_mixed_valid_invalid_rows_do_not_silently_drop() {
    let s = FixtureServer::new(vec![Reply::json(
        r#"{"message":"success","resultList":[{"wlkcid":"c1","kcm":"Valid"},{"wlkcid":"c2"}]}"#,
    )]);
    let client = LearnClient::new(LearnClientConfig::new(s.base(), CourseRole::Student).unwrap());
    let token = SessionRegistry::new()
        .bind_csrf(ServiceId::Learn, CsrfToken::new("synthetic-csrf").unwrap());
    assert!(
        client
            .fetch_course_records(
                &CampusHttpTransport::new("THYou/deep-audit").unwrap(),
                &token,
                "2026-2027-1",
                Some("zh"),
                None
            )
            .await
            .is_err()
    );
    assert_eq!(s.requests().len(), 1);
}

#[cfg(unix)]
#[test]
fn backend_repair_deep_api_academic_cache_is_owner_read_write_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("thyou-cache-fixture-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("overview.json");
    let cache = crate::cache::JsonFileCache::<serde_json::Value>::new(&path, 1, "fixture");
    cache.write(serde_json::json!({"fixture":true})).unwrap();
    let permissions = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        permissions, 0o600,
        "academic cache must not depend on a permissive process umask"
    );
}
