//! All addresses, content and credentials below are isolated synthetic fixtures.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

const INFO: &str =
    "/https/77726476706e69737468656265737421f9f9479369247b59700f81b9991b2631506205de";
const PUBLICATION: &str = "/https/77726476706e69737468656265737421e3f5468534367f1e6d119aafd641303ceb8f9190006d6afc78336870";
const LEGACY: &str =
    "/http/77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9";

#[tokio::test]
async fn backend_repair_news_catalog_reads_only_fixed_info_routes_with_shared_csrf() {
    let server = FixtureServer::new(vec![
        cookie(),
        Reply::json(r#"{"object":[{"id":"unit_1","text":"Fixture unit"}]}"#),
        cookie(),
        Reply::json(r#"{"object":{"lmlist":[{"id":"LM_BGTG","title_zh":"Fixture channel"}]}}"#),
    ]);
    let adapter = adapter(&server);
    let coordinator = coordinator();
    assert_eq!(
        adapter.fetch_news_sources(&coordinator).await.unwrap()[0].id,
        "unit_1"
    );
    assert_eq!(
        adapter.fetch_news_channels(&coordinator).await.unwrap()[0].id,
        "LM_BGTG"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[1].starts_with(&format!(
        "GET {INFO}/b/info/gxfw_fg/common/querySubscribeInformationUnitList?"
    )));
    assert!(requests[1].contains("lmid="));
    assert!(requests[1].contains("_csrf=fixture-csrf"));
    assert!(requests[3].starts_with(&format!(
        "GET {INFO}/b/info/xxfb_fg/teacher/lm/subscribe/getlmListByDwh?"
    )));
    assert!(requests[3].contains("_csrf=fixture-csrf"));
    assert!(!requests[3].contains("lmid="));
}

#[tokio::test]
async fn backend_repair_news_catalog_rejects_redirects_and_login_html() {
    for reply in [
        redirect("https://foreign.invalid/private"),
        Reply::html("<html><form><input name='password'></form></html>"),
    ] {
        let server = FixtureServer::new(vec![cookie(), reply]);
        assert!(
            adapter(&server)
                .fetch_news_sources(&coordinator())
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 2);
    }
}

#[test]
fn backend_repair_news_ghxt_dom_renderer_keeps_article_text_without_active_markup() {
    let url = format!(
        "https://vpn.fixture.invalid/https/{}/article.jsp",
        crate::info_news::GHXT_MAPPING_ID
    );
    let body = r#"<html><body><table><tr><td valign=top>
        <p>Fixture <b>story &amp; details</b>.</p>
        <xmp>literal <broken "quoted</xmp>
        <script>document.cookie='private-script';</script>
        <form><input value='private-form'></form>
        <a href='javascript:alert(1)'>safe link text</a>
        </td></tr></table></body></html>"#;
    let detail = crate::info_news::parse_news_legacy_detail_for_url(body, &url).unwrap();
    assert!(detail.summary.contains("Fixture story & details"));
    assert!(detail.summary.contains("literal <broken"));
    assert!(detail.content_html.contains("&lt;broken"));
    assert!(detail.content_html.contains("<a>safe link text</a>"));
    for private in ["private-script", "private-form", "javascript:"] {
        assert!(!detail.summary.contains(private));
        assert!(!detail.content_html.contains(private));
    }
}

#[tokio::test]
async fn backend_repair_news_ghxt_source_uses_exact_mapping_and_reference_body_policy() {
    let ghxt = format!(
        "/https/{}/legacy/article.jsp?id=fixture",
        crate::info_news::GHXT_MAPPING_ID
    );
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{INFO}/f/bridge")),
        redirect(&ghxt),
        Reply::html(
            "<html><body><table><tr><td valign=top>合成工会新闻正文</td></tr></table></body></html>",
        ),
    ]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await
        .unwrap();
    assert!(result.title.is_empty());
    assert!(result.summary.contains("合成工会新闻正文"));
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[3].starts_with(&format!("GET {ghxt} HTTP/1.1")));
    assert!(!requests[3].contains("_csrf"));
}

#[tokio::test]
async fn backend_repair_news_redirect_direct_publication_link_preserves_id_outside_http() {
    let server = FixtureServer::new(vec![
        cookie(),
        Reply::html("<html><div id=app>shell</div></html>"),
        publication(),
    ]);
    let detail = adapter(&server)
        .fetch_news_detail(
            &coordinator(),
            &format!("{PUBLICATION}/#/publish/fixture-direct"),
        )
        .await
        .unwrap();
    assert_eq!(detail.id, "fixture-direct");
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].starts_with(&format!(
        "GET {PUBLICATION}{DEFAULT_SYSTEM_PDF_PATH}/fixture-direct HTTP/1.1"
    )));
    assert!(!requests[2].contains("_csrf"));
}

#[tokio::test]
async fn backend_repair_news_redirect_normal_info_api_keeps_requested_xxid_check() {
    for returned_id in ["fixture-article", "different-article"] {
        let body=serde_json::json!({"result":"success","object":{"xxDto":{
            "xxid":returned_id,"bt":"Fixture title","nr":"%3Cp%3EFixture%20body%3C%2Fp%3E","fjs_template":[]
        }}}).to_string();
        let server = FixtureServer::new(vec![cookie(), Reply::json(&body)]);
        let result = adapter(&server)
            .fetch_news_detail(&coordinator(), "fixture-article")
            .await;
        if returned_id == "fixture-article" {
            assert_eq!(result.unwrap().id, "fixture-article");
        } else {
            assert!(matches!(result, Err(InfoSessionError::News(_))));
        }
        assert_eq!(server.requests().len(), 2);
    }
}

#[tokio::test]
async fn backend_repair_news_redirect_known_legacy_mapping_still_parses_after_multiple_hops() {
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{INFO}/f/next")),
        redirect(&format!("{LEGACY}/notice.aspx")),
        Reply::html(
            "<html><div class=box3><table><tr><td>Known fixture content</td></tr></table></div></html>",
        ),
    ]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await
        .unwrap();
    assert!(result.title.is_empty());
    assert!(result.summary.contains("Known fixture content"));
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn backend_repair_news_redirect_same_mapping_location_inherits_publication_fragment() {
    let server = FixtureServer::new(vec![
        cookie(),
        redirect("index.html"),
        Reply::html("<html><div id=app>shell</div></html>"),
        publication(),
    ]);
    let detail = adapter(&server)
        .fetch_news_detail(
            &coordinator(),
            &format!("{PUBLICATION}/#/publish/fixture-inherited"),
        )
        .await
        .unwrap();
    assert_eq!(detail.id, "fixture-inherited");
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].starts_with(&format!("GET {PUBLICATION}/index.html HTTP/1.1")));
    assert!(requests[3].contains("/publish/fixture-inherited HTTP/1.1"));
    assert!(
        !requests
            .iter()
            .any(|request| request.lines().next().unwrap().contains('#'))
    );
}

#[tokio::test]
async fn backend_repair_news_redirect_publication_id_cannot_move_to_another_mapping() {
    let server = FixtureServer::new(vec![cookie(), redirect(&format!("{LEGACY}/notice.aspx"))]);
    let result = adapter(&server)
        .fetch_news_detail(
            &coordinator(),
            &format!("{PUBLICATION}/#/publish/fixture-owned"),
        )
        .await;
    assert!(matches!(result, Err(InfoSessionError::NewsUnexpectedPath)));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_redirect_arbitrary_hash_cannot_select_publication_api() {
    for target in [
        format!("{PUBLICATION}/#/other/fixture"),
        format!("{PUBLICATION}/#/publish/"),
        format!("{PUBLICATION}/#/publish/%252fprivate"),
        format!("{PUBLICATION}/#/publish/../../private"),
        format!("{INFO}/#/publish/fixture"),
        format!("{LEGACY}/#/publish/fixture"),
    ] {
        let server = FixtureServer::new(vec![cookie(), redirect(&target)]);
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
async fn backend_repair_news_redirect_fragment_changes_do_not_repeat_the_same_http_request() {
    let server = FixtureServer::new(vec![cookie(), redirect("#/publish/fixture-other")]);
    let result = adapter(&server)
        .fetch_news_detail(
            &coordinator(),
            &format!("{PUBLICATION}/#/publish/fixture-one"),
        )
        .await;
    assert!(matches!(result, Err(InfoSessionError::NewsNavigationCycle)));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_redirect_unknown_mappings_and_foreign_login_pages_remain_unfetched() {
    for target in [
        "https://foreign.invalid/login".to_owned(),
        "/https/77726476706e69737468656265737421deadbeef/article".to_owned(),
        "/wengine-vpn/cookie?method=get&host=foreign.invalid".to_owned(),
        format!("{LEGACY}/notice.aspx?%5fcsrf=PRIVATE_CSRF"),
        format!("{LEGACY}/notice.aspx?name=%0d%0a"),
        format!("{LEGACY}/%2e%2e/private"),
    ] {
        let server = FixtureServer::new(vec![cookie(), redirect(&target)]);
        let error = adapter(&server)
            .fetch_news_detail(&coordinator(), "/f/start")
            .await
            .unwrap_err();
        assert!(!matches!(error, InfoSessionError::LoginRequired));
        // These targets and errors are entirely synthetic, never real URLs.
        assert_eq!(
            server.requests().len(),
            2,
            "fixture target {target}: {error:?}"
        );
    }
}

#[tokio::test]
async fn backend_repair_news_redirect_known_identity_login_is_classified_without_requesting_it() {
    let server = FixtureServer::new(vec![
        cookie(),
        redirect("https://id.tsinghua.edu.cn/do/off/ui/auth/login/form/fixture/0"),
    ]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await;
    assert!(matches!(result, Err(InfoSessionError::LoginRequired)));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_redirect_dynamic_identity_form_is_expiry_without_dispatch() {
    const IDENTITY_MAPPING: &str =
        "/https/77726476706e69737468656265737421f9f30f8834396657761d88e29d51367bcfe7";
    let target = format!(
        "{IDENTITY_MAPPING}/do/off/ui/auth/login/form/0123456789abcdef0123456789abcdef/0?service=fixture"
    );
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{INFO}/f/next")),
        redirect(&target),
    ]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await;
    assert!(matches!(result, Err(InfoSessionError::LoginRequired)));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_news_redirect_dynamic_form_in_other_mapping_stays_rejected() {
    let target = "/https/77726476706e69737468656265737421deadbeef/do/off/ui/auth/login/form/0123456789abcdef0123456789abcdef/0?service=fixture";
    let server = FixtureServer::new(vec![cookie(), redirect(target)]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await;
    assert!(matches!(result, Err(InfoSessionError::NewsUnexpectedPath)));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_news_redirect_current_userinfo_and_csrf_boundaries_still_hold() {
    let server = FixtureServer::new(vec![]);
    let mut wrong = coordinator();
    wrong.begin_authentication(ServiceId::Info).unwrap();
    let csrf = wrong
        .registry()
        .bind_csrf(ServiceId::Info, CsrfToken::new("fixture").unwrap());
    wrong
        .mark_authenticated(
            ServiceId::Info,
            UserIdentity {
                username: "other-fixture".into(),
                display_name: None,
            },
            None,
            Some(csrf),
            None,
        )
        .unwrap();
    assert!(matches!(
        adapter(&server).fetch_news_detail(&wrong, "/f/start").await,
        Err(InfoSessionError::IdentityUserMismatch)
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_news_redirect_playfile_uses_final_id_and_info_stream_without_replaying_entry()
 {
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{LEGACY}/viewer?fileId=fixture-file")),
        Reply::html("<html><script>function _playFile() {}</script></html>"),
        cookie(),
        Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: "%PDF-1.4\nfixture\n%%EOF".into(),
        },
    ]);
    let detail = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start?ticket=fixture-once")
        .await
        .unwrap();
    assert_eq!(detail.title, "PdF");
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("ticket=fixture-once"))
            .count(),
        1
    );
    assert!(requests[4].starts_with(&format!(
        "GET {INFO}{DEFAULT_PDF_STREAM_PATH}/fixture-file?"
    )));
    assert!(requests[4].contains("_csrf="));
    assert!(!requests[2].contains("_csrf="));
}

#[tokio::test]
async fn backend_repair_news_redirect_publication_api_login_redirect_is_not_followed_or_hidden() {
    let server = FixtureServer::new(vec![
        cookie(),
        Reply::html("<html><div id=app>shell</div></html>"),
        redirect("/login"),
    ]);
    let result = adapter(&server)
        .fetch_news_detail(
            &coordinator(),
            &format!("{PUBLICATION}/#/publish/fixture-document"),
        )
        .await;
    assert!(matches!(result, Err(InfoSessionError::LoginRequired)));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_news_redirect_publication_empty_or_invalid_body_is_never_success() {
    for body in [
        r#"{"content":""}"#,
        r#"{"content":null}"#,
        r#"{"unrelated":true}"#,
    ] {
        let server = FixtureServer::new(vec![
            cookie(),
            Reply::html("<html><div id=app>shell</div></html>"),
            Reply::json(body),
        ]);
        let result = adapter(&server)
            .fetch_news_detail(
                &coordinator(),
                &format!("{PUBLICATION}/#/publish/fixture-document"),
            )
            .await;
        assert!(matches!(result, Err(InfoSessionError::News(_))));
        assert_eq!(server.requests().len(), 3);
    }
}

#[tokio::test]
async fn backend_repair_news_redirect_hop_limit_remains_bounded() {
    let mut replies = vec![cookie()];
    replies.extend((0..11).map(|index| redirect(&format!("{INFO}/f/hop-{index}"))));
    let server = FixtureServer::new(replies);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await;
    assert!(matches!(result, Err(InfoSessionError::NewsNavigationLimit)));
    assert_eq!(server.requests().len(), 12); // Cookie read + at most 11 document GETs.
}

#[tokio::test]
async fn backend_repair_news_redirect_diagnostics_explain_rejection_without_logging_url_id_or_token()
 {
    use tracing::instrument::WithSubscriber;
    let root = std::env::temp_dir().join(format!("thyou-news-redaction-{}", uuid::Uuid::new_v4()));
    let mut log = crate::telemetry::LogSession::start(
        &root,
        crate::telemetry::LogConfig::parse("debug,http=trace,auth=debug", false).unwrap(),
    )
    .unwrap();
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{PUBLICATION}/#/unknown/PRIVATE_DOCUMENT")),
    ]);
    async {
        assert!(adapter(&server).fetch_news_detail(&coordinator(),"/f/start?ticket=PRIVATE_TICKET").await.is_err());
        tracing::warn!(target:"tsinghua_kit::security",event="news_navigation_boundary",news_target="PRIVATE_TARGET",news_query="PRIVATE_QUERY",url="https://private.invalid/PRIVATE_URL");
    }.with_subscriber(log.dispatch.clone()).await;
    log.flush();
    let text = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    assert!(text.contains("info_news_fragment_invalid"));
    assert!(text.contains("system_publication"));
    assert!(text.contains("unsupported"));
    for secret in [
        "PRIVATE_DOCUMENT",
        "PRIVATE_TICKET",
        "PRIVATE_TARGET",
        "PRIVATE_QUERY",
        "PRIVATE_URL",
        "fixture-csrf",
    ] {
        assert!(!text.contains(secret));
    }
    drop(log);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn backend_repair_news_unknown_mapping_diagnostic_keeps_only_site_selector() {
    use tracing::instrument::WithSubscriber;
    let root = std::env::temp_dir().join(format!("thyou-news-site-{}", uuid::Uuid::new_v4()));
    let mut log = crate::telemetry::LogSession::start(
        &root,
        crate::telemetry::LogConfig::parse("debug,http=trace,auth=debug", false).unwrap(),
    )
    .unwrap();
    let target = format!(
        "/https/{}/article/PRIVATE_DOCUMENT?ticket=PRIVATE_TICKET",
        crate::thos::MAPPING
    );
    let server = FixtureServer::new(vec![cookie(), redirect(&target)]);
    async {
        assert!(
            adapter(&server)
                .fetch_news_detail(&coordinator(), "/f/start")
                .await
                .is_err()
        );
    }
    .with_subscriber(log.dispatch.clone())
    .await;
    log.flush();
    assert_eq!(server.requests().len(), 2);
    let contents = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    let rejection = contents
        .lines()
        .find(|line| line.contains("info_news_mapping_unknown"))
        .unwrap();
    assert!(rejection.contains(&format!("\"news_mapping_id\":\"{}\"", crate::thos::MAPPING)));
    for private in ["PRIVATE_DOCUMENT", "PRIVATE_TICKET", "article/"] {
        assert!(!contents.contains(private));
    }
    assert!(!crate::telemetry::safe_webvpn_mapping_id(
        "77726476706e69737468656265737421deadbeef"
    ));
    assert!(!crate::telemetry::safe_webvpn_mapping_id(
        "77726476706e69737468656265737421PRIVATE_TICKET0000000000000000"
    ));
    drop(log);
    let _ = std::fs::remove_dir_all(root);
}

fn cookie() -> Reply {
    Reply::html("XSRF-TOKEN=fixture-csrf;")
}
fn redirect(target: &str) -> Reply {
    Reply {
        status: 302,
        headers: format!("Location: {target}\r\n"),
        body: String::new(),
    }
}
fn publication() -> Reply {
    Reply::json(r#"{"content":"<p>Fixture publication body</p>"} "#)
}
fn adapter(server: &FixtureServer) -> InfoSessionAdapter {
    InfoSessionAdapter::new(
        InfoWebVpnConfig::new(server.base(), INFO).unwrap(),
        CampusHttpTransport::new("fixture-news-redirect-repair").unwrap(),
    )
    .unwrap()
}
fn coordinator() -> SessionCoordinator {
    let mut coordinator = SessionCoordinator::new();
    let owner = UserIdentity {
        username: "fixture-reader".into(),
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
            .mark_authenticated(service, owner.clone(), None, csrf, None)
            .unwrap();
    }
    coordinator
}

#[test]
fn backend_repair_news_redirect_reference_publication_fragment_has_prefix_not_only_suffix() {
    assert_eq!(
        safe_publication_fragment(Some("/publish/fixture-document")).unwrap(),
        Some("fixture-document".into())
    );
    assert_eq!(
        safe_publication_fragment(Some("/fixture-document/publish")).unwrap(),
        Some("fixture-document".into())
    );
}

#[tokio::test]
async fn backend_repair_news_redirect_two_hops_into_reference_publication_complete_without_info_csrf_leak()
 {
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{INFO}/f/bridge")),
        redirect(&format!("{PUBLICATION}/#/publish/fixture-document")),
        Reply::html("<html><div id='app'>Publication shell</div></html>"),
        publication(),
    ]);
    let detail = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await
        .unwrap();
    assert_eq!(detail.id, "fixture-document");
    assert_eq!(detail.title, "PdF");
    assert_eq!(detail.summary, "Fixture publication body");
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    assert!(requests[3].starts_with(&format!("GET {PUBLICATION}/ HTTP/1.1")));
    assert!(requests[4].starts_with(&format!(
        "GET {PUBLICATION}/tsinghua.war/yct.www.api/publish/fixture-document HTTP/1.1"
    )));
    assert!(!requests[3].contains("_csrf="));
    assert!(!requests[4].contains("_csrf="));
    assert!(
        !requests
            .iter()
            .any(|request| request.lines().next().unwrap().contains('#'))
    );
}

#[tokio::test]
async fn backend_repair_news_redirect_known_login_target_is_expiry_not_unknown_mapping() {
    let server = FixtureServer::new(vec![cookie(), redirect("/login")]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await;
    assert!(matches!(result, Err(InfoSessionError::LoginRequired)));
    assert_eq!(
        server.requests().len(),
        2,
        "a login page is not fetched or submitted by article navigation"
    );
}

#[tokio::test]
async fn backend_repair_news_redirect_http_401_is_typed_session_expiry() {
    let server = FixtureServer::new(vec![
        cookie(),
        Reply {
            status: 401,
            headers: "Content-Type: text/plain\r\n".into(),
            body: "fixture unauthenticated".into(),
        },
    ]);
    let result = adapter(&server)
        .fetch_news_detail(&coordinator(), "/f/start")
        .await;
    assert!(matches!(result, Err(InfoSessionError::LoginRequired)));
    assert_eq!(server.requests().len(), 2);
}
