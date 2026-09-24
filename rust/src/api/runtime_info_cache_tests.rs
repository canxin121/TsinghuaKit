//! INFO detail cache boundary tests.
//!
//! These fixtures use only loopback responses. They verify that a durable
//! article body is tied to the account, list-returned link, and article id;
//! no campus credential or live URL is involved.

use super::*;

use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};
use chrono::{Duration as ChronoDuration, Utc};

fn cache_base(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("thyou-info-detail-{label}-{}", Uuid::new_v4()))
        .join("cache.json")
}

fn fixture_user(username: &str) -> UserIdentity {
    UserIdentity {
        username: username.to_owned(),
        display_name: None,
    }
}

fn install_identity(runtime: &mut CampusRuntime, user: &UserIdentity) {
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity authentication begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .expect("identity authentication succeeds");
}

fn install_info_proof(runtime: &mut CampusRuntime, user: &UserIdentity, server: &FixtureServer) {
    install_identity(runtime, user);
    let csrf = runtime.coordinator.registry().bind_csrf(
        ServiceId::Info,
        CsrfToken::new("fixture-info-csrf").expect("fixture csrf"),
    );
    runtime
        .coordinator
        .begin_authentication(ServiceId::Info)
        .expect("info authentication begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Info, user.clone(), None, Some(csrf), None)
        .expect("info authentication succeeds");
    let config = InfoWebVpnConfig::new(server.base(), "/target").expect("info config");
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(config, runtime.identity.transport().clone())
            .expect("info adapter"),
    );
    runtime.info_roaming_url = Some(
        crate::info::OpaqueUrl::new(format!("{}target/home", server.base()))
            .expect("info roaming url"),
    );
}

fn detail_payload(
    username: &str,
    article_id: &str,
    bound_link: &str,
    generated_at: chrono::DateTime<Utc>,
) -> InfoNewsDetailCachePayload {
    InfoNewsDetailCachePayload {
        account_scope: cache_account_scope(username),
        article_id: article_id.to_owned(),
        bound_link: bound_link.to_owned(),
        generated_at,
        title: "缓存详情".to_owned(),
        content_html: "<p>缓存正文</p>".to_owned(),
        summary: "缓存正文".to_owned(),
        attachments: vec![InfoNewsAttachmentDto {
            id: "attachment-a".to_owned(),
            name: "附件.pdf".to_owned(),
        }],
    }
}

fn write_detail_cache_for_user(
    base: &std::path::Path,
    username: &str,
    payload: &InfoNewsDetailCachePayload,
) -> std::path::PathBuf {
    let path =
        info_news_detail_cache_path(base, username, &payload.article_id, &payload.bound_link);
    JsonFileCache::<InfoNewsDetailCachePayload>::new(
        &path,
        INFO_NEWS_DETAIL_CACHE_SCHEMA_VERSION,
        INFO_NEWS_DETAIL_CACHE_SERVICE,
    )
    .write(payload)
    .expect("detail cache writes");
    path
}

#[tokio::test]
async fn backend_repair_info_detail_fresh_cache_skips_info_handoff() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("fresh");
    let user = fixture_user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_identity(&mut runtime, &user);
    runtime.info_news_link_owner = Some(user.username.clone());
    runtime
        .info_news_links
        .insert("article-a".to_owned(), "/article-a".to_owned());
    let payload = detail_payload(&user.username, "article-a", "/article-a", Utc::now());
    let cache_path = write_detail_cache_for_user(&base, &user.username, &payload);

    let result = runtime
        .load_info_news_detail_result("article-a".to_owned())
        .await
        .expect("fresh detail cache should be usable");

    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.id, "article-a");
    assert_eq!(result.content_html, "<p>缓存正文</p>");
    assert!(!runtime.service_session_is_proven(ServiceId::Info));
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_info_detail_fresh_cache_is_discovered_after_runtime_restart() {
    let server = FixtureServer::new(Vec::new());
    let base = cache_base("fresh-restart");
    let user = fixture_user("fixture-user");
    let payload = detail_payload(&user.username, "article-a", "/article-a", Utc::now());
    let cache_path = write_detail_cache_for_user(&base, &user.username, &payload);

    // This runtime has the account context that is safe for a cache read, but
    // deliberately has no INFO list mapping and no INFO service proof.  The
    // Rust-owned probe must recover the link only from the typed cache file
    // and return it without creating a handoff.
    let mut restarted = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("restarted runtime config");
    install_identity(&mut restarted, &user);

    let result = restarted
        .load_info_news_detail_result("article-a".to_owned())
        .await
        .expect("restarted runtime should discover fresh detail cache");

    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "ready");
    assert_eq!(result.id, "article-a");
    assert_eq!(result.content_html, "<p>缓存正文</p>");
    assert!(!restarted.service_session_is_proven(ServiceId::Info));
    assert!(restarted.info_news_links.is_empty());
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_file(cache_path);
}

#[test]
fn backend_repair_info_detail_cache_rejects_account_article_and_link_drift() {
    let payload = detail_payload("2026000000", "article-a", "/article-a", Utc::now());
    assert!(info_news_detail_cache_payload_is_valid(
        &payload,
        "2026000000",
        "article-a",
        "/article-a",
    ));
    for (username, article_id, bound_link) in [
        ("2026000001", "article-a", "/article-a"),
        ("2026000000", "article-b", "/article-a"),
        ("2026000000", "article-a", "/article-b"),
    ] {
        assert!(!info_news_detail_cache_payload_is_valid(
            &payload, username, article_id, bound_link,
        ));
    }
    assert_ne!(
        info_news_detail_cache_path(
            std::path::Path::new("/tmp/cache.json"),
            "2026000000",
            "article-a",
            "/article-a",
        ),
        info_news_detail_cache_path(
            std::path::Path::new("/tmp/cache.json"),
            "2026000000",
            "article-a",
            "/article-b",
        )
    );
}

#[tokio::test]
async fn backend_repair_info_detail_live_success_writes_normalized_cache_only() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"xxDto":{"xxid":"article-a","bt":"实时详情","nr":"<p>规范正文</p>","unusedSecretField":"raw-only-marker"}}}"#,
        ),
    ]);
    let base = cache_base("live");
    let user = fixture_user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_info_proof(&mut runtime, &user, &server);
    runtime.info_news_link_owner = Some(user.username.clone());
    runtime
        .info_news_links
        .insert("article-a".to_owned(), "article-a".to_owned());

    let result = runtime
        .load_info_news_detail_result("article-a".to_owned())
        .await
        .expect("live detail should succeed");
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert_eq!(result.content_html, "<p>规范正文</p>");

    let cache_path = info_news_detail_cache_path(&base, &user.username, "article-a", "article-a");
    let raw = std::fs::read_to_string(&cache_path).expect("detail cache exists");
    assert!(raw.contains("规范正文"));
    assert!(!raw.contains("raw-only-marker"));
    let envelope = JsonFileCache::<InfoNewsDetailCachePayload>::new(
        &cache_path,
        INFO_NEWS_DETAIL_CACHE_SCHEMA_VERSION,
        INFO_NEWS_DETAIL_CACHE_SERVICE,
    )
    .read()
    .expect("detail cache reads")
    .expect("detail cache envelope exists");
    assert_eq!(envelope.payload.bound_link, "article-a");
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_info_detail_stale_cache_falls_back_after_live_failure() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let base = cache_base("stale");
    let user = fixture_user("fixture-user");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    install_info_proof(&mut runtime, &user, &server);
    runtime.info_news_link_owner = Some(user.username.clone());
    runtime
        .info_news_links
        .insert("article-a".to_owned(), "article-a".to_owned());
    let payload = detail_payload(
        &user.username,
        "article-a",
        "article-a",
        Utc::now() - ChronoDuration::hours(8),
    );
    let cache_path = write_detail_cache_for_user(&base, &user.username, &payload);

    let result = runtime
        .load_info_news_detail_result("article-a".to_owned())
        .await
        .expect("stale detail should cover a live failure");
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert_eq!(server.requests().len(), 1);
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_info_detail_stale_cache_is_discovered_after_runtime_restart() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let base = cache_base("stale-restart");
    let user = fixture_user("fixture-user");
    let payload = detail_payload(
        &user.username,
        "article-a",
        "article-a",
        Utc::now() - ChronoDuration::hours(8),
    );
    let cache_path = write_detail_cache_for_user(&base, &user.username, &payload);

    // Recreate only the proven INFO service shell.  In particular, do not
    // restore `info_news_links`; the link used for the one live attempt must
    // come from the account/article-bound cache payload.
    let mut restarted = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("restarted runtime config");
    install_info_proof(&mut restarted, &user, &server);

    let result = restarted
        .load_info_news_detail_result("article-a".to_owned())
        .await
        .expect("restarted runtime should fall back to stale detail cache");

    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert_eq!(result.id, "article-a");
    assert!(result.error.is_some());
    assert!(restarted.info_news_links.is_empty());
    assert_eq!(server.requests().len(), 1);
    assert!(restarted.service_session_is_proven(ServiceId::Info));
    let _ = std::fs::remove_file(cache_path);
}

#[path = "runtime_news_redirect_repair_tests.rs"]
mod news_redirect_repair_tests;
