//! Runtime-level fixtures for the four INFO news writes.
//!
//! These exercise the whole layer a Flutter caller reaches: the account-bound
//! session proof, the reference resolution, the plan, the single dispatch and
//! the runtime's own rejection of a repeated subscription condition.  Every
//! request in these tests goes to a loopback fixture; no live account, cookie
//! or CSRF value exists here.

use super::*;
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

const INFO_TARGET_PREFIX: &str =
    "/https/77726476706e69737468656265737421f9f9479369247b59700f81b9991b2631506205de";

fn runtime_base(label: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("thyou-news-write-{label}-{}", Uuid::new_v4()))
        .join("cache.json")
}

fn fixture_user() -> UserIdentity {
    UserIdentity {
        username: "fixture-user".to_owned(),
        display_name: None,
    }
}

/// A runtime with a proven Identity and INFO session whose INFO adapter points
/// at the fixture, exactly as a real handoff would leave it.
fn runtime_with_info(label: &str, server: &FixtureServer) -> (CampusRuntime, std::path::PathBuf) {
    let base = runtime_base(label);
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    let user = fixture_user();
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime
            .coordinator
            .begin_authentication(service)
            .expect("authentication begins");
        let csrf = (service == ServiceId::Info).then(|| {
            runtime.coordinator.registry().bind_csrf(
                service,
                CsrfToken::new("fixture-csrf").expect("fixture csrf"),
            )
        });
        runtime
            .coordinator
            .mark_authenticated(service, user.clone(), None, csrf, None)
            .expect("authentication succeeds");
    }
    let config = InfoWebVpnConfig::new(server.base(), INFO_TARGET_PREFIX).expect("info config");
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(config, runtime.identity.transport().clone())
            .expect("info adapter"),
    );
    runtime.info_roaming_url = Some(
        crate::info::OpaqueUrl::new(format!("{}f/info/gxfw_fg/common/index", server.base()))
            .expect("info roaming url"),
    );
    runtime.portal_bootstrapped = true;
    (runtime, base)
}

fn accepted() -> Reply {
    Reply::json("{\"result\":\"success\"}")
}

fn bootstrap() -> Reply {
    Reply::html("XSRF-TOKEN=fixture-csrf;")
}

fn remember_article(runtime: &mut CampusRuntime, id: &str, link: &str) {
    runtime
        .remember_info_news_link_pairs(
            vec![(id.to_owned(), link.to_owned())].into_iter(),
            Some(fixture_user().username),
        )
        .expect("link pairs remembered");
}

#[tokio::test]
async fn backend_refactor_news_add_favorite_dispatches_once_and_records_the_write() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let (mut runtime, base) = runtime_with_info("add-favorite", &server);
    remember_article(&mut runtime, "article-1", "/article/one");
    let generation = runtime.info_news_link_generation();

    runtime
        .add_info_news_favorite(generation, "article-1".to_owned())
        .await
        .expect("favorite write is accepted");

    assert_eq!(server.requests().len(), 2);
    assert!(
        server.requests()[1].contains("/common/addFavorite/XXFB/article-1"),
        "unexpected write request: {}",
        server.requests()[1]
    );
    // The accepted write is remembered for this account so a page cache older
    // than it is not reused as if it still described the account.
    let (owner, _) = runtime
        .info_news_favorite_write_epoch
        .clone()
        .expect("write epoch recorded");
    assert_eq!(owner, fixture_user().username);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_write_rejects_a_foreign_or_replaced_reference() {
    let server = FixtureServer::new(vec![bootstrap()]);
    let (mut runtime, base) = runtime_with_info("stale-reference", &server);
    remember_article(&mut runtime, "article-1", "/article/one");
    let stale = runtime.info_news_link_generation();
    // A later list read in which the same article moved to another link resets
    // the whole snapshot, so the earlier generation no longer names anything
    // even though this article is still present.
    remember_article(&mut runtime, "article-1", "/article/moved");
    assert_ne!(runtime.info_news_link_generation(), stale);

    let error = runtime
        .add_info_news_favorite(stale, "article-1".to_owned())
        .await
        .expect_err("stale reference must be refused");
    assert!(error.contains("引用已失效"), "unexpected error: {error}");
    // Nothing was requested: the refusal happens before a plan exists.
    assert!(server.requests().is_empty());

    let current = runtime.info_news_link_generation();
    let error = runtime
        .remove_info_news_favorite(current, "unknown-article".to_owned())
        .await
        .expect_err("unknown article must be refused");
    assert!(error.contains("引用已失效"), "unexpected error: {error}");
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_write_requires_a_proven_info_session() {
    let server = FixtureServer::new(Vec::new());
    let base = runtime_base("no-session");
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".to_owned(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .expect("runtime config");
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity authentication begins");
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Identity, fixture_user(), None, None, None)
        .expect("identity authentication succeeds");
    runtime
        .remember_info_news_link_pairs(
            vec![("article-1".to_owned(), "/article/one".to_owned())].into_iter(),
            Some(fixture_user().username),
        )
        .expect("link pairs remembered");
    // No INFO adapter at all: this runtime never opened an INFO handoff, which
    // is exactly the state a first write must refuse instead of silently
    // starting a second authentication path.
    assert!(runtime.info_adapter.is_none());

    let generation = runtime.info_news_link_generation();
    let error = runtime
        .add_info_news_favorite(generation, "article-1".to_owned())
        .await
        .expect_err("a write without an INFO proof must be refused");
    assert!(
        error.contains("INFO 服务会话未确认"),
        "unexpected error: {error}"
    );
    // An ordinary write must not open a second authentication path, so no
    // request of any kind was made.
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_unconfirmed_write_is_reported_and_not_retried() {
    let server = FixtureServer::new(vec![
        bootstrap(),
        Reply::json("{\"result\":\"fail\",\"message\":\"条件无效\"}"),
    ]);
    let (mut runtime, base) = runtime_with_info("unconfirmed", &server);
    remember_article(&mut runtime, "article-1", "/article/one");
    let generation = runtime.info_news_link_generation();

    runtime
        .add_info_news_favorite(generation, "article-1".to_owned())
        .await
        .expect_err("a refusal is not a success");
    assert_eq!(
        runtime.last_info_failure_code(),
        Some("info_news_write_unconfirmed")
    );
    // One bootstrap and one write: the refusal is never resolved by asking
    // again, because the first dispatch's effect is already unknown.
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_subscription_condition_is_not_sent_twice() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let (mut runtime, base) = runtime_with_info("repeat-condition", &server);

    runtime
        .add_info_news_subscription(Some("channel-1".to_owned()), None, None)
        .await
        .expect("first condition is accepted");
    assert_eq!(server.requests().len(), 2);

    let error = runtime
        .add_info_news_subscription(Some("channel-1".to_owned()), None, None)
        .await
        .expect_err("the same condition must not be sent twice");
    assert!(
        error.contains("服务读取未确认"),
        "unexpected error: {error}"
    );
    assert_eq!(
        runtime.last_info_failure_code(),
        Some("info_news_write_replayed")
    );
    // The repeat was refused inside Rust, so the service never saw it.
    assert_eq!(server.requests().len(), 2);

    // A different condition is a different rule and is still allowed.
    let server_accepts = FixtureServer::new(vec![bootstrap(), accepted()]);
    let (mut second, second_base) = runtime_with_info("second-condition", &server_accepts);
    second
        .add_info_news_subscription(None, Some("source-1".to_owned()), Some("量子".to_owned()))
        .await
        .expect("a different condition is accepted");
    assert_eq!(server_accepts.requests().len(), 2);

    let _ = std::fs::remove_dir_all(base.parent().unwrap());
    let _ = std::fs::remove_dir_all(second_base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_subscription_requires_one_condition() {
    let server = FixtureServer::new(Vec::new());
    let (mut runtime, base) = runtime_with_info("empty-condition", &server);

    let error = runtime
        .add_info_news_subscription(None, None, None)
        .await
        .expect_err("a rule with no condition cannot be expressed");
    assert!(
        error.contains("必须包含一个关注"),
        "unexpected error: {error}"
    );
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_remove_subscription_uses_a_recent_selector_once() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let (mut runtime, base) = runtime_with_info("remove-rule", &server);
    runtime
        .info_subscription_selectors
        .insert("opaque-selector".to_owned(), "rule-9".to_owned());
    runtime.info_subscription_owner = Some(fixture_user().username);
    runtime.info_subscription_at = Some(std::time::Instant::now());

    runtime
        .remove_info_news_subscription("opaque-selector".to_owned())
        .await
        .expect("the selected rule is removed");
    assert_eq!(server.requests().len(), 2);
    assert!(
        server.requests()[1].contains("/common/deleteSubscribeCondition/rule-9/XXFB"),
        "unexpected write request: {}",
        server.requests()[1]
    );
    // The service confirmed the rule is gone, so this selector can no longer
    // be used, and only a fresh rule read can hand it out again.
    assert!(runtime.info_subscription_selectors.is_empty());
    let stale = runtime
        .remove_info_news_subscription("opaque-selector".to_owned())
        .await
        .expect_err("a spent selector must be refused");
    assert!(
        stale.contains("订阅选择已失效"),
        "unexpected error: {stale}"
    );
    assert_eq!(server.requests().len(), 2);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_refactor_news_remove_subscription_rejects_another_account_selector() {
    let server = FixtureServer::new(Vec::new());
    let (mut runtime, base) = runtime_with_info("foreign-selector", &server);
    runtime
        .info_subscription_selectors
        .insert("opaque-selector".to_owned(), "rule-9".to_owned());
    runtime.info_subscription_owner = Some("another-account".to_owned());
    runtime.info_subscription_at = Some(std::time::Instant::now());

    let error = runtime
        .remove_info_news_subscription("opaque-selector".to_owned())
        .await
        .expect_err("another account's selector must be refused");
    assert!(
        error.contains("订阅选择已失效"),
        "unexpected error: {error}"
    );
    assert!(server.requests().is_empty());
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}
