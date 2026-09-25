use super::*;
use crate::reference_test_support::{FixtureServer, Reply};
const INFO: &str =
    "/https/77726476706e69737468656265737421f9f9479369247b59700f81b9991b2631506205de";
const LEGACY: &str =
    "/http/77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9";
fn adapter(server: &FixtureServer) -> InfoSessionAdapter {
    InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), INFO).unwrap(),
        CampusHttpTransport::new("THYou/info-lastmile").unwrap(),
    )
    .unwrap()
}
fn coordinator() -> SessionCoordinator {
    let mut c = SessionCoordinator::new();
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    for service in [ServiceId::Identity, ServiceId::Info] {
        c.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            c.registry()
                .bind_csrf(service, CsrfToken::new("fixture-csrf").unwrap())
        });
        c.mark_authenticated(service, user.clone(), None, csrf, None)
            .unwrap();
    }
    c
}
#[tokio::test]
async fn backend_repair_lastmile_info_scoped_redirect_uses_final_reference_template_without_csrf() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply {
            status: 302,
            headers: format!("Location: {INFO}/f/redirecting\r\n"),
            body: String::new(),
        },
        Reply {
            status: 302,
            headers: format!("Location: {LEGACY}/notice/fixture.aspx\r\n"),
            body: String::new(),
        },
        Reply::html(
            "<html><div class=box3><table><tr><td>Fixture content from the actual target template</td></tr></table></div></html>",
        ),
    ]);
    let detail = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await
        .unwrap();
    assert!(detail.title.is_empty());
    assert!(detail.summary.contains("Fixture content"));
    assert_eq!(server.requests().len(), 4);
    assert!(!server.requests()[3].contains("_csrf"));
    assert!(!server.requests()[3].contains("Authorization:"));
}
#[tokio::test]
async fn backend_repair_lastmile_info_unknown_mapping_and_foreign_hosts_not_fetched() {
    for target in [
        "/https/77726476706e69737468656265737421deadbeef/unknown",
        "https://evil.invalid/article",
        "/wengine-vpn/cookie?method=get&host=secret",
    ] {
        let server = FixtureServer::new(vec![
            Reply::html("XSRF-TOKEN=fixture-csrf;"),
            Reply {
                status: 302,
                headers: format!("Location: {target}\r\n"),
                body: String::new(),
            },
        ]);
        assert!(
            adapter(&server)
                .fetch_news_detail(&coordinator(), "/f/start")
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 2);
    }
}

#[tokio::test]
async fn backend_repair_lastmile_info_reference_titleless_legacy_policy_is_not_fabricated() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("reference_lastmile_fixtures.json")).unwrap();
    let first = &fixtures["news"][0];
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply {
            status: 302,
            headers: format!("Location: {LEGACY}/notice.aspx\r\n"),
            body: String::new(),
        },
        Reply::html(first["html"].as_str().unwrap()),
    ]);
    let detail = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await
        .unwrap();
    assert_eq!(detail.title, first["expected"][0].as_str().unwrap());
    assert_eq!(
        detail
            .summary
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>(),
        first["expected"][2].as_str().unwrap()
    );
    assert_eq!(server.requests().len(), 3);
}
#[tokio::test]
async fn backend_repair_lastmile_info_redirect_cycle_and_cross_mapping_csrf_never_dispatch() {
    for target in [
        format!("{INFO}/f/start"),
        format!("{LEGACY}/notice.aspx?_csrf=SHOULD-NOT-LEAVE-INFO"),
    ] {
        let server = FixtureServer::new(vec![
            Reply::html("XSRF-TOKEN=fixture-csrf;"),
            Reply {
                status: 302,
                headers: format!("Location: {target}\r\n"),
                body: String::new(),
            },
        ]);
        assert!(
            adapter(&server)
                .fetch_news_detail(&coordinator(), "/f/start")
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 2);
    }
}
#[test]
fn backend_repair_lastmile_news_policies_match_reference_and_sanitize_active_markup() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("reference_lastmile_fixtures.json")).unwrap();
    for sample in fixtures["news"].as_array().unwrap() {
        let parsed = crate::info_news::parse_news_legacy_detail_for_url(
            sample["html"].as_str().unwrap(),
            sample["url"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(parsed.title, sample["expected"][0]);
        assert_eq!(
            parsed
                .summary
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>(),
            sample["expected"][2]
        );
    }
    let url = fixtures["news"][1]["url"].as_str().unwrap();
    let html = "<h1>Fixture</h1><div class=WordSection1><script>secretPayload()</script><p onclick='bad()'>Actual content</p></div>";
    let parsed = crate::info_news::parse_news_legacy_detail_for_url(html, url).unwrap();
    assert!(!parsed.content_html.contains("<script"));
    assert!(!parsed.content_html.contains("onclick"));
    assert!(
        crate::info_news::parse_news_legacy_detail_for_url(
            "<h1>Other</h1><article>unrelated content without reference selector</article>",
            url
        )
        .is_err()
    );
}
#[tokio::test]
async fn backend_repair_lastmile_info_input_csrf_and_encoded_control_do_not_leave_runtime() {
    for query in [
        "_csrf=PRIVATE",
        "%5fcsrf=PRIVATE",
        "CsRf=PRIVATE",
        "name=%0d%0a",
    ] {
        let server = FixtureServer::new(vec![Reply::html("XSRF-TOKEN=fixture-csrf;")]);
        assert!(
            adapter(&server)
                .fetch_news_detail(&coordinator(), &format!("/f/start?{query}"))
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
}
#[test]
fn backend_repair_lastmile_news_every_pinned_selector_is_supported_without_network() {
    // Compile selectors through the actual parser function using harmless
    // empty HTML. An absent DOM match may fail, unsupported selectors must not.
    let policies: Vec<(String, [String; 2])> =
        serde_json::from_str(include_str!("reference_news_policies.json")).unwrap();
    assert_eq!(policies.len(), 19);
    for (key, _) in policies {
        let result = crate::info_news::parse_news_legacy_detail_for_url(
            "<html><body><p>No match</p></body></html>",
            &format!("https://webvpn.fixture.invalid/{key}/article"),
        );
        if let Err(crate::info_news::NewsParseError::DetailMalformedPayload { reason, .. }) = result
        {
            assert!(!reason.contains("unsupported source-owned selector"));
        }
    }
}
