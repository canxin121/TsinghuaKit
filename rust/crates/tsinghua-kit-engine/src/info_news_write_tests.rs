//! Loopback fixtures for the four INFO news writes.
//!
//! Every case asserts the dispatch count as well as the classification: a
//! write is dispatched exactly once, and a response that the service does not
//! confirm must not turn into a second request.

use super::*;
use crate::info_news::{NewsProfile, NewsSubscriptionDraft, NewsWriteOutcome};
use crate::reference_test_support::{FixtureServer, Reply};

const INFO: &str =
    "/https/77726476706e69737468656265737421f9f9479369247b59700f81b9991b2631506205de";

fn adapter(server: &FixtureServer) -> InfoSessionAdapter {
    InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), INFO).unwrap(),
        CampusHttpTransport::new("THYou/info-news-write").unwrap(),
    )
    .unwrap()
}

fn coordinator() -> SessionCoordinator {
    let mut coordinator = SessionCoordinator::new();
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    for service in [ServiceId::Identity, ServiceId::Info] {
        coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-csrf").unwrap())
        });
        coordinator
            .mark_authenticated(service, user.clone(), None, csrf, None)
            .unwrap();
    }
    coordinator
}

fn accepted() -> Reply {
    Reply::json("{\"result\":\"success\"}")
}

/// The cookie bootstrap every write performs before the write itself.
fn bootstrap() -> Reply {
    Reply::html("XSRF-TOKEN=fixture-csrf;")
}

#[tokio::test]
async fn backend_repair_news_add_favorite_accepts_one_dispatch() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let profile = NewsProfile::standard();
    let plan = profile.add_favorite_request("article-1").unwrap();
    let outcome = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap();
    assert_eq!(outcome, NewsWriteOutcome::Accepted);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /wengine-vpn/cookie?"));
    assert!(requests[1].starts_with("GET "));
    assert!(requests[1].contains("/common/addFavorite/XXFB/article-1"));
    assert!(requests[1].contains("_csrf=fixture-csrf"));
    assert!(!requests[1].contains("Authorization:"));
}

#[tokio::test]
async fn backend_repair_news_remove_favorite_uses_the_removal_route() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let plan = NewsProfile::standard()
        .remove_favorite_request("article-2")
        .unwrap();
    let outcome = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap();
    assert_eq!(outcome, NewsWriteOutcome::Accepted);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("/common/delFavorite/XXFB/article-2"));
}

#[tokio::test]
async fn backend_repair_news_add_subscription_sends_one_form_with_the_scope() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let draft = NewsSubscriptionDraft::new()
        .with_channel("channel-1")
        .with_source("source-1")
        .with_keyword("量子");
    let plan = NewsProfile::standard()
        .add_subscription_request(&draft)
        .unwrap();
    let outcome = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap();
    assert_eq!(outcome, NewsWriteOutcome::Accepted);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("POST /"));
    assert!(requests[1].contains("/common/addSubscribeCondition"));
    assert!(requests[1].contains("_csrf=fixture-csrf"));
    // The rule body is one JSON object in the service's own field, and the
    // module is the fixed scope the route requires.
    assert!(requests[1].contains("dygz=%7B"));
    assert!(requests[1].contains("mkid=XXFB"));
}

#[tokio::test]
async fn backend_repair_news_remove_subscription_carries_rule_and_scope() {
    let server = FixtureServer::new(vec![bootstrap(), accepted()]);
    let plan = NewsProfile::standard()
        .remove_subscription_request("rule-9")
        .unwrap();
    let outcome = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap();
    assert_eq!(outcome, NewsWriteOutcome::Accepted);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("/common/deleteSubscribeCondition/rule-9/XXFB"));
}

#[tokio::test]
async fn backend_repair_news_refused_write_is_not_retried() {
    for body in [
        "{\"result\":\"fail\"}",
        "{\"success\":false,\"message\":\"条件无效\"}",
        "<html><body>操作失败</body></html>",
        "{}",
        "not json at all",
    ] {
        let server = FixtureServer::new(vec![bootstrap(), Reply::json(body)]);
        let plan = NewsProfile::standard()
            .add_favorite_request("article-1")
            .unwrap();
        let outcome = adapter(&server)
            .execute_news_write(&coordinator(), plan)
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                NewsWriteOutcome::Refused | NewsWriteOutcome::Unrecognized
            ),
            "unexpected outcome for {body}: {outcome:?}"
        );
        // One bootstrap and one write: nothing is re-sent, whatever the answer
        // said, because the effect of the first dispatch is already unknown.
        assert_eq!(server.requests().len(), 2, "replayed write for {body}");
    }
}

#[tokio::test]
async fn backend_repair_news_redirect_answer_is_not_followed() {
    let server = FixtureServer::new(vec![
        bootstrap(),
        Reply {
            status: 302,
            headers: format!("Location: {INFO}/b/info/gxfw_fg/common/addFavorite/XXFB/other\r\n"),
            body: String::new(),
        },
        accepted(),
    ]);
    let plan = NewsProfile::standard()
        .add_favorite_request("article-1")
        .unwrap();
    let outcome = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap();
    assert_eq!(outcome, NewsWriteOutcome::Unrecognized);
    // The third fixture is deliberately never consumed: a redirect hop would
    // be a second dispatch of a write whose effect is already unknown.
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_login_page_reports_login_required() {
    let server = FixtureServer::new(vec![
        bootstrap(),
        Reply::html(
            "<html><head><title>统一身份认证</title></head><body>请登录 id.tsinghua.edu.cn</body></html>",
        ),
    ]);
    let plan = NewsProfile::standard()
        .add_favorite_request("article-1")
        .unwrap();
    let error = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap_err();
    assert!(matches!(error, InfoSessionError::LoginRequired));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_server_error_is_unconfirmed_not_replayed() {
    let server = FixtureServer::new(vec![
        bootstrap(),
        Reply {
            status: 500,
            headers: String::new(),
            body: "fixture failure".into(),
        },
    ]);
    let plan = NewsProfile::standard()
        .remove_subscription_request("rule-9")
        .unwrap();
    let outcome = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap();
    assert_eq!(outcome, NewsWriteOutcome::Unrecognized);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_read_plan_is_not_a_write() {
    let server = FixtureServer::new(vec![bootstrap()]);
    let plan = NewsProfile::standard()
        .list_request(1, 20, None, None)
        .unwrap();
    let error = adapter(&server)
        .execute_news_write(&coordinator(), plan)
        .await
        .unwrap_err();
    assert!(matches!(error, InfoSessionError::InvalidConfig(_)));
    // A read plan is refused before the cookie bootstrap, so nothing at all
    // was requested.
    assert!(server.requests().is_empty());
}

#[test]
fn backend_repair_news_subscription_condition_requires_one_condition() {
    let profile = NewsProfile::standard();
    let error = profile
        .add_subscription_request(&NewsSubscriptionDraft::new())
        .unwrap_err();
    assert_eq!(
        error,
        crate::info_news::NewsProfileError::EmptySubscriptionCondition
    );
}

#[test]
fn backend_repair_news_write_path_segments_reject_traversal() {
    let profile = NewsProfile::standard();
    for value in ["../secret", "a/b", "a\\b", "a?b", "a#b", "a&b=c", "a%b"] {
        assert!(
            profile.add_favorite_request(value).is_err(),
            "accepted unsafe article id {value}"
        );
        assert!(
            profile.remove_subscription_request(value).is_err(),
            "accepted unsafe rule id {value}"
        );
    }
}
