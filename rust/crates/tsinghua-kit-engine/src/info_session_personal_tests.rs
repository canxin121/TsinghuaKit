use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

fn proven(coordinator: &mut SessionCoordinator) {
    let user = UserIdentity {
        username: "fixture-user".to_owned(),
        display_name: None,
    };
    coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    coordinator.begin_authentication(ServiceId::Info).unwrap();
    let csrf = coordinator
        .registry()
        .bind_csrf(ServiceId::Info, CsrfToken::new("fixture-csrf").unwrap());
    coordinator
        .mark_authenticated(ServiceId::Info, user, None, Some(csrf), None)
        .unwrap();
}

#[tokio::test]
async fn backend_repair_info_personal_routes_use_proven_csrf_and_complete_favorite_pages() {
    let item = |id: &str| {
        format!(
            r#"{{"bt":"标题","url":"/article/{id}","xxid":"{id}","time":"2026-09-24","dwmc_show":"教务处","lmid":"LM_JWGG","sfsc":true}}"#
        )
    };
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"object":[{"id":"rule_1","titile":"我的通知","pxz":1,"bt":"","fbdwmcList":[],"lmmcList":[]}]}"#,
        ),
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(&format!(
            r#"{{"object":{{"totalPages":2,"resultList":[{}]}}}}"#,
            item("article_1")
        )),
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(&format!(
            r#"{{"object":{{"totalPages":2,"resultList":[{}]}}}}"#,
            item("article_2")
        )),
    ]);
    let adapter = InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), "/target").unwrap(),
        CampusHttpTransport::new("THYou/personal-fixture").unwrap(),
    )
    .unwrap();
    let mut coordinator = SessionCoordinator::new();
    proven(&mut coordinator);

    let rules = adapter
        .fetch_news_subscriptions(&coordinator)
        .await
        .unwrap();
    assert_eq!(rules.len(), 1);
    let favorites = adapter.fetch_all_favorite_news(&coordinator).await.unwrap();
    assert_eq!(
        favorites
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["article_1", "article_2"]
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests[1].starts_with("GET /target/b/info/gxfw_fg/common/querySubscribeConditionNameList/XXFB?_csrf=fixture-csrf HTTP/1.1"));
    for (index, page) in [(3, 1), (5, 2)] {
        assert!(requests[index].starts_with("POST /target/b/info/gxfw_fg/common/queryFavoriteXxfbPageList?_csrf=fixture-csrf HTTP/1.1"));
        assert!(requests[index].contains(&format!("currentPage={page}")));
    }
}

#[tokio::test]
async fn backend_repair_info_personal_pagination_drift_is_not_a_complete_favorite_list() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"object":{"totalPages":2,"resultList":[{"bt":"A","url":"/a","xxid":"a","time":"2026-09-24","dwmc_show":"教务处","lmid":"LM_JWGG","sfsc":true}]}}"#,
        ),
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"object":{"totalPages":3,"resultList":[{"bt":"B","url":"/b","xxid":"b","time":"2026-09-24","dwmc_show":"教务处","lmid":"LM_JWGG","sfsc":true}]}}"#,
        ),
    ]);
    let adapter = InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), "/target").unwrap(),
        CampusHttpTransport::new("THYou/personal-fixture").unwrap(),
    )
    .unwrap();
    let mut coordinator = SessionCoordinator::new();
    proven(&mut coordinator);
    assert!(matches!(
        adapter.fetch_all_favorite_news(&coordinator).await,
        Err(InfoSessionError::News(
            NewsParseError::CatalogMalformedPayload { kind: "favorites" }
        ))
    ));
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn backend_repair_info_subscription_feed_posts_only_fixed_page_and_rule_id() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"object":{"resultList":[{"bt":"通知","url":"/article/1","xxid":"article_1","time":"2026-09-24","dwmc_show":"教务处","lmid":"LM_JWGG","sfsc":false}]}}"#,
        ),
    ]);
    let adapter = InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), "/target").unwrap(),
        CampusHttpTransport::new("THYou/subscription-fixture").unwrap(),
    )
    .unwrap();
    let mut coordinator = SessionCoordinator::new();
    proven(&mut coordinator);
    assert!(
        adapter
            .fetch_news_by_subscription(&coordinator, 101, "rule_1")
            .await
            .is_err()
    );
    assert!(
        adapter
            .fetch_news_by_subscription(&coordinator, 1, "../escape")
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let page = adapter
        .fetch_news_by_subscription(&coordinator, 1, "rule_1")
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("POST /target/b/info/gxfw_fg/common/querySubscribeInfomationPageList?_csrf=fixture-csrf HTTP/1.1"));
    assert!(requests[1].contains("currentPage=1"));
    assert!(requests[1].contains("dyid=rule_1"));
}
