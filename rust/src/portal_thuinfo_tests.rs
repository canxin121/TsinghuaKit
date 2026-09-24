//! Expected wire URLs were executed from the pinned THUInfo helper offline.
use super::*;
use crate::{
    identity::{
        FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthActions,
        SecondAuthProfile,
    },
    identity_client::{IdentityClient, IdentityClientConfig},
    reference_test_support::{FixtureServer, Reply},
    transport::CampusHttpTransport,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vector {
    input: String,
    expected: String,
}

#[tokio::test]
async fn backend_repair_thuinfo_http_200_location_metadata_does_not_trigger_another_get() {
    let vpn = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: text/html\r\nLocation: /logout\r\n".into(),
        body: "<html>fixture portal</html>".into(),
    }]);
    let id = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!("Location: {}https/fixturemap/f/info/index\r\n", vpn.base()),
        body: String::new(),
    }]);
    let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    navigate_portal_target(
        &execution(id.base()),
        "fixture",
        &cfg,
        "/https/fixturemap/",
        &Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base())).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(vpn.requests().len(), 1);
    assert_eq!(oauth.requests().len(), 1);
    assert!(id.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_thuinfo_non_redirect_statuses_do_not_become_navigation_success() {
    for status in [204, 206, 300, 304] {
        let vpn = FixtureServer::new(vec![Reply::html("<html>must not reach</html>")]);
        let id = FixtureServer::new(vec![]);
        let oauth = FixtureServer::new(vec![Reply {
            status,
            headers: format!("Location: {}https/fixturemap/f/info/index\r\n", vpn.base()),
            body: String::new(),
        }]);
        let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
        assert!(
            navigate_portal_target(
                &execution(id.base()),
                "fixture",
                &cfg,
                "/https/fixturemap/",
                &Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base())).unwrap()
            )
            .await
            .is_err()
        );
        assert_eq!(oauth.requests().len(), 1);
        assert!(vpn.requests().is_empty());
        assert!(id.requests().is_empty());
    }
}

#[tokio::test]
async fn backend_repair_thuinfo_navigation_must_still_pass_csrf_and_account_proof() {
    use crate::info_session::{InfoSessionAdapter, InfoSessionError, InfoWebVpnConfig};
    use crate::protocol::UserIdentity;
    for mode in ["csrf_missing", "wrong_account", "verified"] {
        let mut replies = vec![Reply::html("<html>gateway home</html>")];
        if mode == "csrf_missing" {
            replies.push(Reply::html(""));
        } else {
            replies.push(Reply {
                status: 200,
                headers: "Content-Type: text/plain\r\n".into(),
                body: "XSRF-TOKEN=fixture-target-csrf;".into(),
            });
            replies.push(Reply::json(if mode == "wrong_account" {
                r#"{"result":"success","object":{"ryh":"fixture-other"}}"#
            } else {
                r#"{"result":"success","object":{"ryh":"fixture-user"}}"#
            }));
        }
        let vpn = FixtureServer::new(replies);
        let id = FixtureServer::new(vec![]);
        let oauth = FixtureServer::new(vec![Reply {
            status: 302,
            headers: format!("Location: {}\r\n", vpn.base()),
            body: String::new(),
        }]);
        let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
        let execution = execution(id.base());
        navigate_portal_target(
            &execution,
            "fixture",
            &cfg,
            "/https/fixturemap/",
            &Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base())).unwrap(),
        )
        .await
        .unwrap();
        let adapter = InfoSessionAdapter::new(
            InfoWebVpnConfig::new(vpn.base(), "/https/fixturemap/").unwrap(),
            execution.transport().clone(),
        )
        .unwrap();
        let result = adapter
            .probe_portal_account_with_handoff(
                &UserIdentity {
                    username: "fixture-user".into(),
                    display_name: None,
                },
                false,
            )
            .await;
        match mode {
            "csrf_missing" => assert!(matches!(result, Err(InfoSessionError::MissingCookieCsrf))),
            "wrong_account" => assert!(matches!(
                result,
                Err(InfoSessionError::PortalAccountMismatch)
            )),
            _ => assert_eq!(result.unwrap().as_str(), "fixture-target-csrf"),
        }
        assert_eq!(
            vpn.requests().len(),
            if mode == "csrf_missing" { 2 } else { 3 }
        );
        assert_eq!(oauth.requests().len(), 1);
        assert!(id.requests().is_empty());
        assert!(vpn.requests()[1].starts_with("GET /wengine-vpn/cookie?"));
        if mode != "csrf_missing" {
            assert!(vpn.requests()[2].contains("/b/info/gxfw_fg/common/grjbxx?"));
        }
    }
}

#[tokio::test]
async fn backend_repair_thuinfo_terminal_page_menu_links_are_not_extra_auth_requests() {
    let id = FixtureServer::new(vec![]);
    let vpn = FixtureServer::new(vec![Reply::html(&format!(
        "<html><a href='{}do/off/ui/auth/login/form/unrelated/0'>账号设置</a><a href='/logout'>退出</a></html>",
        id.base()
    ))]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}https/fixturemap/f/info/gxfw_fg/common/index\r\n",
            vpn.base()
        ),
        body: String::new(),
    }]);
    let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    navigate_portal_target(
        &execution(id.base()),
        "fixture",
        &cfg,
        "/https/fixturemap/",
        &Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base())).unwrap(),
    )
    .await
    .unwrap();
    assert!(id.requests().is_empty());
    assert_eq!(vpn.requests().len(), 1);
    assert_eq!(oauth.requests().len(), 1);
}

#[test]
fn backend_repair_thuinfo_raw_codec_preserves_escaped_opaque_values_but_rejects_overrides() {
    let cfg = WebVpnIdentityConfig::current();
    for uri in [
        "/f/info/index?ticket=FIXTURE%2B%2F%3D&lang=zh",
        "/f/info/index?ticket=FIXTURE+abc==",
    ] {
        let url = Url::parse(&format!("https://info.tsinghua.edu.cn{uri}")).unwrap();
        let result = broker_target(&cfg, url.clone()).unwrap();
        assert_eq!(
            result.query().unwrap(),
            format!("scheme=https&host=info.tsinghua.edu.cn&port=443&uri={uri}")
        );
        assert!(validated_info_broker(
            &result,
            &cfg,
            &Url::parse(INFO_ORIGIN).unwrap()
        ));
        assert!(
            allowed(&cfg, "/https/fixturemap/", &url),
            "an exact server HTTP Location can remain a direct INFO URL"
        );
    }
    for uri in [
        "/f/info/index?host=evil.invalid",
        "/f/info/index?%68ost=evil.invalid",
        "/f/info/index?ticket=x&uri=//evil.invalid",
        "/f/info/index?ticket=x&port=80",
        "/f/info/index?ticket=%0d%0a",
        "/f/info/index?ticket=%ZZ",
    ] {
        assert!(
            broker_target(
                &cfg,
                Url::parse(&format!("https://info.tsinghua.edu.cn{uri}")).unwrap()
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn backend_repair_thuinfo_repeated_location_is_reported_without_ticket_replay() {
    let vpn = FixtureServer::new(vec![]);
    let id = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: /thu-oauth/auth?ticket=FIXTURE\r\n".into(),
        body: String::new(),
    }]);
    let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    let result = navigate_portal_target(
        &execution(id.base()),
        "fixture",
        &cfg,
        "/https/fixturemap/",
        &Url::parse(&format!("{}thu-oauth/auth?ticket=FIXTURE", oauth.base())).unwrap(),
    )
    .await;
    assert_eq!(result, Err("portal_navigation_cycle"));
    assert_eq!(oauth.requests().len(), 1);
    assert!(vpn.requests().is_empty());
    assert!(id.requests().is_empty());
}

#[test]
fn backend_repair_thuinfo_blocked_route_diagnostics_do_not_include_queries_or_paths() {
    use crate::telemetry::{LogConfig, LogSession};
    let root =
        std::env::temp_dir().join(format!("thyou-reference-privacy-{}", uuid::Uuid::new_v4()));
    let mut log = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    let denied = FixtureServer::new(vec![]);
    let vpn = FixtureServer::new(vec![]);
    let id = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}private-user-path?ticket=FIXTURE-SECRET\r\n",
            denied.base()
        ),
        body: String::new(),
    }]);
    let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                assert_eq!(
                    navigate_portal_target(
                        &execution(id.base()),
                        "fixture",
                        &cfg,
                        "/https/fixturemap/",
                        &Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base()))
                            .unwrap()
                    )
                    .await,
                    Err("portal_navigation_target_rejected")
                );
            });
        tracing::warn!(target:"tsinghua_kit::security",event="portal_route_decision",route_class="private-user-path",route_decision="FIXTURE-SECRET");
    });
    log.flush();
    let text = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["fields"]["route_class"] == "foreign_origin"
                && e["fields"]["route_decision"] == "reject_route")
    );
    assert!(!text.contains("FIXTURE-SECRET"));
    assert!(!text.contains("private-user-path"));
    assert!(!text.contains(denied.base()));
    assert!(denied.requests().is_empty());
    drop(log);
    std::fs::remove_dir_all(root).unwrap();
}

fn execution(base: &str) -> IdentityExecutionClient {
    let profile = IdentityLoginProfile::new(
        "fixture",
        "/do/off/ui/auth/login/form/{appId}/0",
        LoginFormProfile::new(
            "/do/off/ui/auth/login/check",
            FormEncoding::UrlEncoded,
            LoginFormFields::common(),
        ),
        SecondAuthProfile::new(
            "/b/doubleAuth/login",
            "type",
            "action",
            Vec::new(),
            SecondAuthActions::new(None, None, None, None),
        ),
    );
    IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(base, profile).unwrap()).unwrap(),
        CampusHttpTransport::new("THYou/thuinfo-reference-fixture").unwrap(),
    )
}

#[test]
fn backend_repair_thuinfo_broker_builder_matches_executed_reference_bytes() {
    let config = WebVpnIdentityConfig::current();
    let vectors: Vec<Vector> =
        serde_json::from_str(include_str!("portal_thuinfo_fixtures.json")).unwrap();
    for vector in vectors {
        assert_eq!(
            broker_target(&config, Url::parse(&vector.input).unwrap())
                .unwrap()
                .as_str(),
            vector.expected,
            "compare wire bytes, not query_pairs after form decoding"
        );
    }
}

#[test]
fn backend_repair_thuinfo_exact_mapping_root_is_not_a_different_service() {
    let config = WebVpnIdentityConfig::current();
    let prefix = "/https/fixturemap/";
    for path in [
        "/https/fixturemap",
        "/https/fixturemap/",
        "/https/fixturemap%2F",
        "/https/fixturemap%2Ff%2Finfo%2Fgxfw_fg%2Fcommon%2Findex",
    ] {
        assert!(allowed(
            &config,
            prefix,
            &config.webvpn_origin().join(path).unwrap()
        ));
    }
}

#[tokio::test]
async fn backend_repair_thuinfo_gateway_root_redirect_continues_with_exact_wire_url() {
    let vpn=FixtureServer::new(vec![
        Reply {status:302,headers:"Location: /https/fixturemap\r\n".into(),body:String::new()},
        Reply {status:302,headers:"Location: /https/fixturemap/f/info/gxfw_fg/common/index\r\nSet-Cookie: fixture-info=ready; Path=/\r\n".into(),body:String::new()},
        Reply::html("<html>fixture target landing</html>"),
    ]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 307,
        headers: format!(
            "Location: {}https/fixturemap%2F?ticket=FIXTURE%2Babc%3D\r\n",
            vpn.base()
        ),
        body: String::new(),
    }]);
    let id = FixtureServer::new(vec![]);
    let config = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    let input = Url::parse(
        "https://info.tsinghua.edu.cn/f/info/gxfw_fg/common/index?ticket=FIXTURE%2Babc%3D",
    )
    .unwrap();
    let target = broker_target(&config, input).unwrap();
    navigate_portal_target(
        &execution(id.base()),
        "fixture-fingerprint",
        &config,
        "/https/fixturemap/",
        &target,
    )
    .await
    .unwrap();
    let expected = "GET /lb-auth/lbredirect?scheme=https&host=info.tsinghua.edu.cn&port=443&uri=/f/info/gxfw_fg/common/index?ticket=FIXTURE%2Babc%3D HTTP/1.1";
    assert!(oauth.requests()[0].starts_with(expected));
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(vpn.requests().len(), 3);
    assert!(id.requests().is_empty());
    assert!(vpn.requests()[0].starts_with("GET /https/fixturemap%2F?ticket=FIXTURE%2Babc%3D "));
    assert!(
        vpn.requests()[2]
            .to_ascii_lowercase()
            .contains("cookie: fixture-info=ready")
    );
    for request in oauth.requests().iter().chain(vpn.requests().iter()) {
        assert!(!request.contains("i_pass"));
        assert!(!request.contains("vericode"));
    }
}

#[tokio::test]
async fn backend_repair_thuinfo_clean_webvpn_home_returns_for_business_proof_not_new_login() {
    let vpn = FixtureServer::new(vec![Reply::html("<html>gateway home</html>")]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!("Location: {}\r\n", vpn.base()),
        body: String::new(),
    }]);
    let id = FixtureServer::new(vec![]);
    let config = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    let target = Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base())).unwrap();
    navigate_portal_target(
        &execution(id.base()),
        "fixture-fingerprint",
        &config,
        "/https/fixturemap/",
        &target,
    )
    .await
    .unwrap();
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(vpn.requests().len(), 1);
    assert!(id.requests().is_empty());
    // Returning navigation success is NOT an INFO account/CSRF capability.
}

#[test]
fn backend_repair_thuinfo_mapping_scope_still_blocks_siblings_and_nested_escapes() {
    let cfg = WebVpnIdentityConfig::current();
    for path in [
        "/https/fixturemap-extra",
        "/https/other",
        "/https/fixturemap%252Findex",
        "/https/fixturemap%2F%2e%2e%2Fother",
        "/https/fixturemap%2F%5cother",
        "/https/fixturemap%0D%0A",
        "/https/fixturemap%ZZ",
    ] {
        assert!(!allowed(
            &cfg,
            "/https/fixturemap/",
            &cfg.webvpn_origin().join(path).unwrap()
        ));
    }
}

#[tokio::test]
async fn backend_repair_thuinfo_foreign_redirect_is_not_fetched_or_retried() {
    let denied = FixtureServer::new(vec![]);
    let vpn = FixtureServer::new(vec![]);
    let id = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}private?ticket=SECRET-FIXTURE\r\n",
            denied.base()
        ),
        body: String::new(),
    }]);
    let cfg = WebVpnIdentityConfig::new(vpn.base(), oauth.base(), id.base()).unwrap();
    assert!(
        navigate_portal_target(
            &execution(id.base()),
            "fixture",
            &cfg,
            "/https/fixturemap/",
            &Url::parse(&format!("{}thu-oauth/auth?state=fixture", oauth.base())).unwrap()
        )
        .await
        .is_err()
    );
    assert_eq!(oauth.requests().len(), 1);
    assert!(denied.requests().is_empty());
    assert!(vpn.requests().is_empty());
    assert!(id.requests().is_empty());
}
