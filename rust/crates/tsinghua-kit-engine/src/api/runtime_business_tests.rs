//! Business-stage contracts using only synthetic, loopback responses.
use super::*;
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

fn runtime() -> CampusRuntime {
    CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false).unwrap()
}

const ELECTRICITY_BALANCE: &str = "<html><span id='Netweb_Home_electricity_DetailCtrl1_lblele'>12.50</span><span id='Netweb_Home_electricity_DetailCtrl1_lbltime'>2026-09-18 12:00:00</span></html>";

#[test]
fn backend_repair_followup_electricity_absolute_double_slash_is_a_path_not_a_host() {
    let flow = electricity_auth::ElectricityFlow::current();
    let target =
        Url::parse("http://myhome.tsinghua.edu.cn//default.aspx?ticket=FIXTURE%2Babc%3D").unwrap();
    let next = flow.to_broker(&target).unwrap();
    // Do not resolve an already validated absolute target's path with join:
    // join("//default.aspx") would incorrectly replace the authority.
    assert_eq!(next.host_str(), Some("webvpn.tsinghua.edu.cn"));
    assert!(next.path().ends_with("//default.aspx"));
    assert_eq!(next.query(), target.query());
}

#[test]
fn backend_repair_followup_electricity_same_host_routing_fields_are_inner_uri_data() {
    let flow = electricity_auth::ElectricityFlow::current();
    let target = Url::parse("https://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D&scheme=https&host=myhome.tsinghua.edu.cn&port=443&uri=%2Fhome").unwrap();
    let broker = flow.to_broker(&target).unwrap();
    let pairs = broker.query_pairs().collect::<Vec<_>>();
    assert_eq!(pairs.len(), 4);
    assert_eq!(pairs[1].1, "myhome.tsinghua.edu.cn");
    assert_eq!(
        pairs[3].1,
        "/default.aspx?ticket=FIXTURE%2Babc%3D&scheme=https&host=myhome.tsinghua.edu.cn&port=443&uri=%2Fhome"
    );
    assert_eq!(flow.to_broker(&broker).unwrap(), broker);
}

#[tokio::test]
async fn backend_repair_followup_electricity_trusted_success_double_slash_reaches_balance() {
    let vpn = FixtureServer::new(vec![
        Reply::html("<html><input id='net_Default_LoginCtrl1_txtUserName'></html>"),
        Reply {
            status: 200,
            headers: "Content-Type: text/html\r\nSet-Cookie: fixture-elec=ready; Path=/\r\n".into(),
            body: "<html>target handoff</html>".into(),
        },
        Reply::html(ELECTRICITY_BALANCE),
    ]);
    let identity = FixtureServer::new(vec![
        Reply::html(&format!(
            "<html><span id='sm2publicKey'>{}</span><form action='/do/off/ui/auth/login/check'><input name='i_user'><input type='password' name='i_pass'></form></html>",
            public_key()
        )),
        Reply::html(
            "<html>登录成功。正在重定向到<a href='http://myhome.tsinghua.edu.cn//default.aspx?ticket=FIXTURE%2Babc%3D'>继续</a></html>",
        ),
    ]);
    let oauth = FixtureServer::new(vec![]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    r.primary_password = Some("synthetic-password".into());
    r.ensure_electricity_with_flow(&user(), &flow)
        .await
        .unwrap();
    assert!(r.electricity_service_is_proven());
    assert!(r.load_electricity_remainder().await.is_ok());
    assert_eq!(identity.requests().len(), 2);
    assert_eq!(
        identity
            .requests()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        1
    );
    assert!(!identity.requests().iter().any(|r| r.contains("doubleAuth")));
    let requests = vpn.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]
            .starts_with("GET /http/electric-fixture//default.aspx?ticket=FIXTURE%2Babc%3D ")
    );
    assert!(
        requests[2]
            .to_ascii_lowercase()
            .contains("cookie: fixture-elec=ready")
    );
    assert!(oauth.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_followup_electricity_double_slash_navigation_is_not_balance_proof() {
    let vpn = FixtureServer::new(vec![
        Reply::html("<html>handoff only</html>"),
        Reply::html("<html><input id='net_Default_LoginCtrl1_txtUserName'></html>"),
    ]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    assert!(
        r.finish_electricity_with_flow(
            &user(),
            Url::parse("http://myhome.tsinghua.edu.cn//default.aspx?ticket=FIXTURE").unwrap(),
            &flow
        )
        .await
        .is_err()
    );
    assert!(!r.electricity_service_is_proven());
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert_eq!(vpn.requests().len(), 2);
    assert!(identity.requests().is_empty() && oauth.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_followup_electricity_mapped_callback_foreign_redirect_is_not_fetched() {
    let foreign = FixtureServer::new(vec![]);
    let vpn = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!("Location: {}private?ticket=FIXTURE\r\n", foreign.base()),
        body: String::new(),
    }]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    let error = r
        .finish_electricity_with_flow(
            &user(),
            Url::parse("http://myhome.tsinghua.edu.cn//default.aspx?ticket=FIXTURE").unwrap(),
            &flow,
        )
        .await
        .unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "electricity_redirect_target_rejected"
    );
    assert_eq!(vpn.requests().len(), 1);
    assert!(
        foreign.requests().is_empty()
            && identity.requests().is_empty()
            && oauth.requests().is_empty()
    );
    assert!(!r.electricity_service_is_proven());
}

#[test]
fn backend_repair_followup_electricity_inner_fields_cannot_override_outer_origin() {
    let flow = electricity_auth::ElectricityFlow::current();
    for (field, value) in [
        ("host", "myhome.tsinghua.edu.cn"),
        ("scheme", "https"),
        ("port", "443"),
    ] {
        let target = Url::parse(&format!(
            "https://myhome.tsinghua.edu.cn/default.aspx?{field}={value}"
        ))
        .unwrap();
        let mapped = flow.to_broker(&target).unwrap();
        assert_eq!(mapped.host_str(), Some("oauth.tsinghua.edu.cn"));
        assert_eq!(mapped.query_pairs().count(), 4);
        assert_eq!(
            mapped.query_pairs().last().unwrap().1,
            format!("/default.aspx?{field}={value}")
        );
        assert_eq!(flow.to_broker(&mapped).unwrap(), mapped);
    }
    for suffix in [
        "host=evil.invalid",
        "scheme=ftp",
        "port=8080",
        "scheme=http&port=443",
        "host=myhome.tsinghua.edu.cn&HOST=evil.invalid",
        "ticket=%0d%0a",
    ] {
        for path in ["/default.aspx", "//default.aspx"] {
            let target =
                Url::parse(&format!("https://myhome.tsinghua.edu.cn{path}?{suffix}")).unwrap();
            assert!(flow.to_broker(&target).is_err());
        }
    }
    for broker in [
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=//evil.invalid/",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=/ok&host=evil.invalid",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=%2Fok&%75ri=%2Fevil",
    ] {
        assert!(flow.to_broker(&Url::parse(broker).unwrap()).is_err());
    }
}

#[test]
fn backend_repair_followup_electricity_callback_rejection_is_not_misreported_as_password_failure() {
    let r = runtime();
    let flow = electricity_auth::ElectricityFlow::current();
    let response = crate::identity_execution::IdentityHttpResponse::from_service_navigation(
        reqwest::StatusCode::OK,
        Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/redirect2Jsp").unwrap(), None,
        "<html>登录成功。正在重定向到<a href='https://myhome.tsinghua.edu.cn/default.aspx?host=evil.invalid'>继续</a></html>".into(),
    );
    assert_eq!(
        electricity_auth::selected_target_checked(r.identity.identity().client(), &response, &flow)
            .unwrap_err(),
        "electricity_callback_query_rejected"
    );
}

#[test]
fn backend_repair_sep19_electricity_nested_uri_is_encoded_inside_fixed_broker() {
    let flow = electricity_auth::ElectricityFlow::current();
    let target = Url::parse("https://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D&uri=%2FNetweb_List%2Fhome.aspx").unwrap();
    let broker = flow.to_broker(&target).unwrap();
    let pairs = broker.query_pairs().collect::<Vec<_>>();
    assert_eq!(pairs.len(), 4);
    assert_eq!(pairs.iter().filter(|(key, _)| key == "uri").count(), 1);
    assert_eq!(
        pairs[3].1,
        "/default.aspx?ticket=FIXTURE%2Babc%3D&uri=%2FNetweb_List%2Fhome.aspx"
    );
    assert_eq!(flow.to_broker(&broker).unwrap(), broker);
}

#[test]
fn backend_repair_sep19_electricity_selection_and_dispatch_share_strict_broker_rules() {
    let flow = electricity_auth::ElectricityFlow::current();
    let r = runtime();
    let client = r.identity.identity().client();
    let response = crate::identity_execution::IdentityHttpResponse::from_service_navigation(
        reqwest::StatusCode::OK,
        Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/redirect2Jsp").unwrap(),
        None,
        "<html>登录成功。正在重定向到<a href='https://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D&amp;uri=%2Fhome'>继续</a></html>".into(),
    );
    let selected = electricity_auth::selected_target(client, &response, &flow).unwrap();
    let broker = flow.to_broker(&selected).unwrap();
    assert_eq!(broker.query_pairs().count(), 4);
    for bad in [
        "http://evil.invalid/default.aspx?ticket=x",
        "http://myhome.tsinghua.edu.cn:8080/default.aspx?ticket=x",
        "http://user@myhome.tsinghua.edu.cn/default.aspx?ticket=x",
        "http://myhome.tsinghua.edu.cn/default.aspx?ticket=x&host=evil.invalid",
        "http://myhome.tsinghua.edu.cn/default.aspx?ticket=%0d%0a",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=/ok&uri=//evil.invalid/",
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=%2Fok&host=evil.invalid",
    ] {
        assert!(flow.to_broker(&Url::parse(bad).unwrap()).is_err());
    }
}

#[tokio::test]
async fn backend_repair_sep19_electricity_compound_callback_reaches_balance_without_new_identity() {
    let vpn = FixtureServer::new(vec![
        Reply::html("<html>handoff completed</html>"),
        Reply::html(ELECTRICITY_BALANCE),
    ]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}http/electric-fixture/default.aspx\r\n",
            vpn.base()
        ),
        body: String::new(),
    }]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    r.finish_electricity_with_flow(
        &user(),
        Url::parse(
            "https://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D&uri=%2Fhome",
        )
        .unwrap(),
        &flow,
    )
    .await
    .unwrap();
    assert!(r.electricity_service_is_proven());
    assert!(r.load_electricity_remainder().await.is_ok());
    assert!(identity.requests().is_empty());
    assert_eq!(vpn.requests().len(), 2);
    let requests = oauth.requests();
    assert_eq!(requests.len(), 1);
    let route = requests[0]
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let sent = Url::parse(oauth.base()).unwrap().join(route).unwrap();
    let pairs = sent.query_pairs().collect::<Vec<_>>();
    assert_eq!(pairs.len(), 4);
    assert_eq!(
        pairs[3].1,
        "/default.aspx?ticket=FIXTURE%2Babc%3D&uri=%2Fhome"
    );
}

#[test]
fn backend_repair_sep19_electricity_simple_reference_wire_and_mapping_boundaries_remain() {
    let flow = electricity_auth::ElectricityFlow::current();
    for (scheme, port) in [("http", "80"), ("https", "443")] {
        let target = Url::parse(&format!(
            "{scheme}://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D"
        ))
        .unwrap();
        assert_eq!(
            flow.to_broker(&target).unwrap().query().unwrap(),
            format!(
                "scheme={scheme}&host=myhome.tsinghua.edu.cn&port={port}&uri=/default.aspx?ticket=FIXTURE%2Babc%3D"
            )
        );
    }
    let callback =
        Url::parse("https://oauth.tsinghua.edu.cn/thu-oauth/fixture?ticket=FIXTURE").unwrap();
    assert_eq!(flow.to_broker(&callback).unwrap(), callback);
    assert!(
        flow.to_broker(
            &Url::parse("https://webvpn.tsinghua.edu.cn/http/other-mapping/default.aspx").unwrap()
        )
        .is_err()
    );
}

#[test]
fn backend_repair_sep19_electricity_raw_broker_duplicates_cannot_hide_inside_a_path() {
    let flow = electricity_auth::ElectricityFlow::current();
    let prefix = "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=";
    for path in ["/ok", "/default.aspx?ticket=FIXTURE%2Babc%3D"] {
        let valid = Url::parse(&format!("{prefix}{path}")).unwrap();
        assert_eq!(flow.to_broker(&valid).unwrap(), valid);
        for key in ["scheme", "host", "port", "uri", "HOST", "%75ri", "%68ost"] {
            let duplicate = Url::parse(&format!("{prefix}{path}&{key}=invalid")).unwrap();
            assert!(
                flow.to_broker(&duplicate).is_err(),
                "duplicate routing field accepted"
            );
        }
    }
    // A genuine nested URI is safe only when it remains a single encoded
    // outer value. Preserve the original ticket escapes after one decode.
    let direct = Url::parse(
        "http://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D&uri=%2Fhome",
    )
    .unwrap();
    let encoded = flow.to_broker(&direct).unwrap();
    assert_eq!(encoded.query_pairs().count(), 4);
    assert_eq!(flow.to_broker(&encoded).unwrap(), encoded);
    assert_eq!(
        encoded.query_pairs().last().unwrap().1,
        "/default.aspx?ticket=FIXTURE%2Babc%3D&uri=%2Fhome"
    );
}

#[tokio::test]
async fn backend_repair_business_electricity_broker_navigation_requires_real_balance_proof() {
    let vpn = FixtureServer::new(vec![
        Reply {
            status: 200,
            headers: "Content-Type: text/html\r\nSet-Cookie: fixture-electric=ready; Path=/\r\n"
                .into(),
            body: "<html>handoff completed</html>".into(),
        },
        Reply::html(ELECTRICITY_BALANCE),
    ]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 307,
        headers: format!(
            "Location: {}http/electric-fixture/default.aspx?ticket=FIXTURE%2Babc\r\n",
            vpn.base()
        ),
        body: String::new(),
    }]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    let target =
        Url::parse("http://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc").unwrap();
    r.finish_electricity_with_flow(&user(), target, &flow)
        .await
        .unwrap();
    assert!(r.electricity_service_is_proven());
    assert!(r.load_electricity_remainder().await.is_ok());
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(vpn.requests().len(), 2);
    assert!(identity.requests().is_empty());
    assert!(oauth.requests()[0].starts_with("GET /lb-auth/lbredirect?scheme=http&host=myhome.tsinghua.edu.cn&port=80&uri=/default.aspx?ticket=FIXTURE%2Babc "));
    assert!(
        vpn.requests()[1]
            .to_ascii_lowercase()
            .contains("cookie: fixture-electric=ready")
    );
    assert!(vpn.requests()[1].contains("/Netweb_List/Netweb_Home_electricity_Detail.aspx"));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.service_session_is_proven(ServiceId::Info));
}

#[tokio::test]
async fn backend_repair_business_electricity_healthy_first_read_does_not_reauthenticate() {
    let vpn = FixtureServer::new(vec![Reply::html(ELECTRICITY_BALANCE)]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    r.ensure_electricity_with_flow(&user(), &flow)
        .await
        .unwrap();
    r.ensure_electricity_with_flow(&user(), &flow)
        .await
        .unwrap();
    assert!(r.load_electricity_remainder().await.is_ok());
    assert_eq!(vpn.requests().len(), 1);
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_business_electricity_navigation_alone_never_marks_service_usable() {
    let vpn = FixtureServer::new(vec![
        Reply::html("<html>navigation only</html>"),
        Reply::html("<html><input id='net_Default_LoginCtrl1_txtUserName'></html>"),
    ]);
    let identity = FixtureServer::new(vec![]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}http/electric-fixture/default.aspx\r\n",
            vpn.base()
        ),
        body: String::new(),
    }]);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    assert!(
        r.finish_electricity_with_flow(
            &user(),
            Url::parse("http://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE").unwrap(),
            &flow
        )
        .await
        .is_err()
    );
    assert!(!r.electricity_service_is_proven());
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(identity.requests().is_empty());
    assert_eq!(vpn.requests().len(), 2);
}

#[test]
fn backend_repair_business_electricity_callback_is_origin_scoped_and_keeps_opaque_ticket() {
    let flow = electricity_auth::ElectricityFlow::current();
    let r = runtime();
    let client = r.identity.identity().client();
    let html = "<html>登录成功。正在重定向到<a href='http://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D'>继续</a></html>";
    let response = crate::identity_execution::IdentityHttpResponse::from_service_navigation(
        reqwest::StatusCode::OK,
        Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/redirect2Jsp").unwrap(),
        None,
        html.into(),
    );
    let target = electricity_auth::selected_target(client, &response, &flow).unwrap();
    assert_eq!(target.query(), Some("ticket=FIXTURE%2Babc%3D"));
    assert!(
        flow.to_broker(&target)
            .unwrap()
            .as_str()
            .ends_with("uri=/default.aspx?ticket=FIXTURE%2Babc%3D")
    );
    for bad in [
        "http://evil.invalid/default.aspx?ticket=x",
        "http://myhome.tsinghua.edu.cn:8080/default.aspx?ticket=x",
        "http://user@myhome.tsinghua.edu.cn/default.aspx?ticket=x",
        "http://myhome.tsinghua.edu.cn/default.aspx?ticket=x&host=evil.invalid",
        "http://myhome.tsinghua.edu.cn/default.aspx?ticket=%0d%0a",
    ] {
        assert!(flow.to_broker(&Url::parse(bad).unwrap()).is_err());
    }
}

#[tokio::test]
async fn backend_repair_business_electricity_factor_failure_cannot_replay_or_affect_info() {
    let identity = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: "unavailable".into(),
    }]);
    let vpn = FixtureServer::new(vec![]);
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    r.service_second_factor = Some(PendingServiceSecondFactor {
        target: ServiceSecondFactorTarget::Electricity,
        challenge: crate::protocol::SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Wechat],
            masked_phone: None,
            expires_at: None,
        },
    });
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert!(
        r.complete_second_factor("wechat".into(), "123456".into())
            .await
            .is_err()
    );
    assert_eq!(identity.requests().len(), 1);
    assert!(vpn.requests().is_empty());
    assert!(r.service_second_factor.is_none());
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(r.service_session_is_proven(ServiceId::Identity));
}
fn user() -> UserIdentity {
    UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    }
}
fn prove_info(r: &mut CampusRuntime, server: &FixtureServer) {
    for service in [ServiceId::Identity, ServiceId::Info] {
        r.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            r.coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-info-csrf").unwrap())
        });
        r.coordinator
            .mark_authenticated(service, user(), None, csrf, None)
            .unwrap();
    }
    r.portal_bootstrapped = true;
    r.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/info/").unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.info_roaming_url =
        Some(crate::info::OpaqueUrl::new(format!("{}info/f/info/index", server.base())).unwrap());
}
const LEARN_MAP: &str =
    "/https/77726476706e69737468656265737421fcf2408e297e7c4377068ea48d546d30ca8cc97bcc/";

#[tokio::test]
async fn backend_repair_business_news_catalog_needs_proven_account_and_both_real_arrays() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":[{"id":"unit_1","text":"Fixture unit"}]}"#),
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":{"lmlist":[{"id":"LM_JWGG","title_zh":"教务通知"}]}}"#),
    ]);
    let mut r = runtime();
    assert!(r.load_info_news_catalog().await.is_err());
    assert!(server.requests().is_empty());
    prove_info(&mut r, &server);
    let catalog = r.load_info_news_catalog().await.unwrap();
    assert_eq!(catalog.source, "live");
    assert_eq!(catalog.status, "ready");
    assert_eq!(catalog.sources[0].id, "unit_1");
    assert_eq!(catalog.channels[0].id, "LM_JWGG");
    assert_eq!(server.requests().len(), 4);

    let malformed = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":{"list":[]}}"#),
    ]);
    let mut rejected = runtime();
    prove_info(&mut rejected, &malformed);
    let error = rejected.load_info_news_catalog().await.unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "info_catalog_format"
    );
    assert_eq!(malformed.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_business_news_catalog_rejects_second_malformed_response() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":[{"id":"unit_1","text":"Fixture unit"}]}"#),
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":{"unexpected":[]}}"#),
    ]);
    let mut runtime = runtime();
    prove_info(&mut runtime, &server);
    let error = runtime.load_info_news_catalog().await.unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "info_catalog_format"
    );
    assert_eq!(server.requests().len(), 4);
}

#[tokio::test]
async fn backend_repair_business_news_catalog_404_keeps_only_proven_sources_and_observed_channels()
{
    let missing = || Reply {
        status: 404,
        headers: "Content-Type: application/json\r\n".into(),
        body: "{}".into(),
    };
    let source = Reply::json(r#"{"object":[{"id":"unit_1","text":"Fixture unit"}]}"#);
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        source,
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        missing(),
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(
            r#"{"object":{"dataList":[{"bt":"Fixture news","url":"/article","xxid":"fixture-id","time":"2026-09-24 10:00:00","dwmc_show":"Fixture source","yxzd":"0","lmid":"LM_JWGG","sfsc":false},{"bt":"Second news","url":"/article-2","xxid":"fixture-id-2","time":"2026-09-24 09:00:00","dwmc_show":"Fixture source","yxzd":"0","lmid":"LM_NEW","sfsc":false}]}}"#,
        ),
    ]);
    let mut reader = runtime();
    prove_info(&mut reader, &server);
    let catalog = reader.load_info_news_catalog().await.unwrap();
    assert_eq!(catalog.source, "live");
    assert_eq!(catalog.status, "partial");
    assert_eq!(catalog.sources[0].id, "unit_1");
    assert_eq!(catalog.channels.len(), 2);
    assert_eq!(catalog.channels[0].id, "LM_JWGG");
    assert_eq!(catalog.channels[0].label, "教务通知");
    assert_eq!(catalog.channels[1].id, "LM_NEW");
    assert_eq!(catalog.channels[1].label, "LM_NEW");
    assert!(
        catalog
            .channel_error
            .as_deref()
            .unwrap()
            .contains("仅显示最新新闻")
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests[5].starts_with("GET /info/b/info/xxfb_fg/xnzx/template/more?"));
    assert!(requests[5].contains("currentPage=1"));
    assert!(requests[5].contains("length=30"));

    let source_missing = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        missing(),
    ]);
    let mut rejected = runtime();
    prove_info(&mut rejected, &source_missing);
    assert!(rejected.load_info_news_catalog().await.is_err());
    assert_eq!(source_missing.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_business_news_catalog_404_reports_observation_failure_as_partial() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":[{"id":"unit_1","text":"Fixture unit"}]}"#),
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply {
            status: 404,
            headers: "Content-Type: application/json\r\n".into(),
            body: "{}".into(),
        },
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply {
            status: 503,
            headers: "Content-Type: application/json\r\n".into(),
            body: "{}".into(),
        },
    ]);
    let mut reader = runtime();
    prove_info(&mut reader, &server);
    let catalog = reader.load_info_news_catalog().await.unwrap();
    assert_eq!(catalog.status, "partial");
    assert_eq!(catalog.sources[0].id, "unit_1");
    assert!(catalog.channels.is_empty());
    assert!(
        catalog
            .channel_error
            .as_deref()
            .unwrap()
            .contains("读取失败")
    );
    assert_eq!(server.requests().len(), 6);
}

#[tokio::test]
async fn backend_repair_business_news_catalog_404_rejects_expired_observation_session() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(r#"{"object":[{"id":"unit_1","text":"Fixture unit"}]}"#),
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply {
            status: 404,
            headers: "Content-Type: application/json\r\n".into(),
            body: "{}".into(),
        },
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply {
            status: 401,
            headers: "Content-Type: application/json\r\n".into(),
            body: "{}".into(),
        },
    ]);
    let mut reader = runtime();
    prove_info(&mut reader, &server);
    let error = reader.load_info_news_catalog().await.unwrap_err();
    assert!(error.contains("会话已过期"));
    assert!(!reader.service_session_is_proven(ServiceId::Info));
    assert_eq!(server.requests().len(), 6);
}

#[tokio::test]
async fn backend_repair_business_news_ui_id_uses_the_returned_article_link() {
    let list = r#"{"result":"success","object":{"dataList":[{"xxid":"list-row-id","bt":"Fixture notice","url":"/f/info/article?entry=fixture","time":"2026-09-18 12:00:00","dwmc_show":"Fixture","yxzd":"0","lmid":"NEWS","sfsc":false}]}}"#;
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply::json(list),
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply::html("<html><script>var xxid = \"actual-article-id\";</script></html>"),
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply::json(
            r#"{"object":{"xxDto":{"xxid":"actual-article-id","bt":"Fixture notice","nr":"<p>100% complete</p>"}}}"#,
        ),
    ]);
    let mut r = runtime();
    prove_info(&mut r, &server);
    let page = r.load_info_news(1, 10, None, None).await.unwrap();
    let detail = r
        .load_info_news_detail(page.items[0].id.clone())
        .await
        .unwrap();
    assert_eq!(detail.id, "actual-article-id");
    assert_eq!(detail.content_html, "<p>100% complete</p>");
    let req = server.requests();
    assert_eq!(req.len(), 6);
    assert!(req[3].starts_with("GET /info/f/info/article?entry=fixture "));
    assert!(req[5].contains("xxid=actual-article-id"));
    assert!(!req[5].contains("list-row-id"));
}

#[tokio::test]
async fn backend_repair_business_learn_without_ticket_uses_info_roam_then_course_home_proof() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"https://learn.tsinghua.edu.cn/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE-LEARN"}}"#,
        ),
        Reply::html("<html>roaming completed</html>"),
        Reply::html(
            "<html><a href='/f/wlxt/index/course/student/index?_csrf=fixture-learn-csrf'>Courses</a></html>",
        ),
    ]);
    let mut r = runtime();
    prove_info(&mut r, &server);
    let transport = r.identity.transport().clone();
    let learn = LearnClient::new(
        LearnClientConfig::new(
            &format!("{}{LEARN_MAP}", server.base().trim_end_matches('/')),
            CourseRole::Student,
        )
        .unwrap(),
    );
    let (_, csrf) = r
        .ensure_learn_session_with_client(&transport, &user(), learn)
        .await
        .unwrap();
    assert_eq!(csrf.service(), ServiceId::Learn);
    assert_eq!(csrf.as_csrf_token().as_str(), "fixture-learn-csrf");
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /wengine-vpn/cookie?"));
    assert!(requests[1].contains("yyfwid=3E401364BDD7AEA7EBF1EDE3F15ED4B7"));
    assert!(requests[2].starts_with(&format!(
        "GET {LEARN_MAP}b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE-LEARN "
    )));
    assert!(requests[3].starts_with(&format!(
        "GET {LEARN_MAP}f/wlxt/index/course/student/index "
    )));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("i_pass") && !request.starts_with("POST"))
    );
}

#[tokio::test]
async fn backend_repair_business_failed_learn_roam_never_proves_learn_or_erases_info() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply::json(r#"{"result":"error","msg":"fixture unavailable"}"#),
    ]);
    let mut r = runtime();
    prove_info(&mut r, &server);
    let transport = r.identity.transport().clone();
    let learn = LearnClient::new(
        LearnClientConfig::new(
            &format!("{}{LEARN_MAP}", server.base().trim_end_matches('/')),
            CourseRole::Student,
        )
        .unwrap(),
    );
    assert!(
        r.ensure_learn_session_with_client(&transport, &user(), learn)
            .await
            .is_err()
    );
    assert!(!r.service_session_is_proven(ServiceId::Learn));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(server.requests().len(), 2);
    assert!(
        !server
            .requests()
            .iter()
            .any(|req| req.contains("j_spring_security_thauth_roaming_entry"))
    );
}

fn inject_identity(r: &mut CampusRuntime, base: &str) {
    let profile = r.identity.identity().client().config().profile.clone();
    r.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        IdentityClient::new(IdentityClientConfig::new(base, profile).unwrap()).unwrap(),
        r.identity.transport().clone(),
    ));
}
fn public_key() -> String {
    use sm2::elliptic_curve::sec1::ToSec1Point;
    sm2::SecretKey::from_slice(&[1u8; 32])
        .unwrap()
        .public_key()
        .to_sec1_point(false)
        .as_bytes()[1..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
#[tokio::test]
async fn backend_repair_business_electricity_explicit_login_page_enters_its_id_target_once() {
    let vpn = FixtureServer::new(vec![Reply::html(
        "<html><input id='net_Default_LoginCtrl1_txtUserName'></html>",
    )]);
    let oauth = FixtureServer::new(vec![]);
    let identity = FixtureServer::new(vec![
        Reply::html(&format!(
            "<html><span id='sm2publicKey'>{}</span><form action='/do/off/ui/auth/login/check'><input name='i_user'><input type='password' name='i_pass'></form></html>",
            public_key()
        )),
        Reply::html("<html>二次认证</html>"),
        Reply::json(
            r#"{"result":"success","object":{"hasWeChatBool":true,"hasTotp":true,"phone":"fixture-masked"}}"#,
        ),
    ]);
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    r.primary_password = Some("fixture-password".into());
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    assert!(
        r.ensure_electricity_with_flow(&user(), &flow)
            .await
            .is_err()
    );
    assert!(r.current_service_second_factor().is_some());
    assert_eq!(identity.requests().len(), 3);
    assert_eq!(vpn.requests().len(), 1);
    assert!(oauth.requests().is_empty());
    assert!(
        identity.requests()[0]
            .starts_with("GET /do/off/ui/auth/login/form/0a993de7e533cd43a594459abdcab27d/1 ")
    );
    assert!(identity.requests()[1].starts_with("POST /do/off/ui/auth/login/check "));
    assert!(!identity.requests()[1].contains("fixture-password"));
    assert!(!identity.requests().iter().any(|r| r.contains("SEND_CODE")));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(!r.electricity_service_is_proven());
}
#[tokio::test]
async fn backend_repair_business_electricity_non_auth_failure_must_not_submit_credentials() {
    let vpn = FixtureServer::new(vec![Reply {
        status: 503,
        headers: "Content-Type: text/html\r\n".into(),
        body: "<html>fixture outage</html>".into(),
    }]);
    let oauth = FixtureServer::new(vec![]);
    let identity = FixtureServer::new(vec![]);
    let mut r = runtime();
    inject_identity(&mut r, identity.base());
    prove_info(&mut r, &vpn);
    r.primary_password = Some("fixture-password".into());
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), oauth.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    assert!(
        r.ensure_electricity_with_flow(&user(), &flow)
            .await
            .is_err()
    );
    assert!(identity.requests().is_empty());
    assert!(oauth.requests().is_empty());
    assert_eq!(vpn.requests().len(), 1);
    assert!(r.current_service_second_factor().is_none());
}

#[tokio::test]
async fn backend_repair_business_classroom_consumed_handoff_query_is_not_adapter_base() {
    let mapping =
        "/http/77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290/";
    let body = format!(
        "<html><div class='w30'><a href='{mapping}pk.classroomctrl.do?m=qyClassroomState&amp;classroom=Fixture&amp;weeknumber=3'>Fixture Building</a></div></html>"
    );
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply::json(
            r#"{"object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/portal3rd.do?ticket=FIXTURE&mode=home"}}"#,
        ),
        Reply::html("<html>session handoff</html>"),
        Reply::html(&body),
    ]);
    let mut r = runtime();
    prove_info(&mut r, &server);
    r.ensure_classroom_session(&user()).await.unwrap();
    assert!(r.classroom_service_is_proven());
    assert_eq!(server.requests().len(), 4);
    assert!(server.requests()[3].starts_with(&format!(
        "GET {mapping}portal3rd.do?url=/portal3rd.do&m=jasJy_Xs_Js_index "
    )));
    assert!(!server.requests()[3].contains("ticket=FIXTURE"));
    assert!(r.service_session_is_proven(ServiceId::Info));
}
#[test]
fn backend_repair_business_news_link_cache_is_cleared_on_account_logout_and_info_invalidation() {
    let mut r = runtime();
    r.info_news_links
        .insert("fixture".into(), "/f/private-link".into());
    r.info_news_link_owner = Some("fixture-user".into());
    r.logout().unwrap();
    assert!(r.info_news_links.is_empty());
    assert!(r.info_news_link_owner.is_none());
    r.info_news_links
        .insert("fixture".into(), "/f/private-link".into());
    r.info_news_link_owner = Some("fixture-user".into());
    r.invalidate_service_session(ServiceId::Info);
    assert!(r.info_news_links.is_empty());
    assert!(r.info_news_link_owner.is_none());
}

#[test]
fn backend_repair_business_new_failure_codes_do_not_echo_private_response_fields() {
    let path = std::env::temp_dir().join(format!("thyou-business-privacy-{}", Uuid::new_v4()));
    let mut log = crate::telemetry::LogSession::start(
        &path,
        crate::telemetry::LogConfig::parse("trace", false).unwrap(),
    )
    .unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        let error =
            InfoSessionError::News(crate::info_news::NewsParseError::DetailMalformedPayload {
                field: "synthetic_private_field".into(),
                reason: "private-body-cookie-ticket".into(),
            });
        let mut r = runtime();
        let message = r.record_business_failure("info", "info_detail", info_failure_code(&error));
        assert_eq!(
            crate::telemetry::diagnostic_reason(&message),
            "info_detail_field"
        );
        assert!(!message.contains("synthetic_private_field"));
        assert!(!message.contains("private-body-cookie-ticket"));
        let message = r.record_business_failure(
            "electricity",
            "electricity_handoff",
            "electricity_handoff_target",
        );
        assert_eq!(
            crate::telemetry::diagnostic_reason(&message),
            "electricity_handoff_target"
        );
    });
    log.flush();
    let text = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    assert!(!text.contains("synthetic_private_field"));
    assert!(!text.contains("private-body-cookie-ticket"));
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["redacted_fields"], 0);
    }
    drop(log);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn backend_repair_news_legacy_failure_classification_keeps_only_fixed_reason() {
    use crate::info_news::NewsParseError;
    for (field, reason, expected) in [
        (
            "legacy.content",
            "reference body selector has no match",
            "info_detail_legacy_selector_missing",
        ),
        (
            "legacy.content",
            "reference-selected article body is empty",
            "info_detail_legacy_empty",
        ),
        (
            "legacy.content",
            "private HTML content",
            "info_detail_legacy_content",
        ),
        ("legacy.title", "private title", "info_detail_legacy_title"),
        (
            "document",
            "document is not valid UTF-8",
            "info_detail_encoding",
        ),
    ] {
        let error = InfoSessionError::News(NewsParseError::DetailMalformedPayload {
            field: field.into(),
            reason: reason.into(),
        });
        let fixed = info_failure_code(&error);
        assert_eq!(fixed, expected);
        assert!(!fixed.contains("private"));
    }
}

#[test]
fn backend_repair_news_legacy_content_stage_diagnostic_is_bounded_and_private() {
    use crate::info_news::NewsParseError;
    for (field, reason, expected) in [
        (
            "legacy.fragment",
            "HTML tag is not terminated",
            "info_detail_legacy_tag_unterminated",
        ),
        (
            "legacy.fragment",
            "private fragment markup",
            "info_detail_legacy_fragment",
        ),
        (
            "legacy.validation",
            "content contains a control character",
            "info_detail_legacy_control",
        ),
        (
            "legacy.summary",
            "unterminated HTML markup",
            "info_detail_legacy_summary_markup",
        ),
        (
            "legacy.summary",
            "invalid HTML entity code point",
            "info_detail_legacy_entity",
        ),
        (
            "legacy.summary",
            "private article text",
            "info_detail_legacy_summary",
        ),
    ] {
        let error = InfoSessionError::News(NewsParseError::DetailMalformedPayload {
            field: field.into(),
            reason: reason.into(),
        });
        let fixed = info_failure_code(&error);
        assert_eq!(fixed, expected);
        assert!(!fixed.contains("private"));
    }
}

#[tokio::test]
async fn backend_repair_business_empty_learn_handoff_never_substitutes_for_course_home_csrf() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply::json(
            r#"{"object":{"roamingurl":"https://learn.tsinghua.edu.cn/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE"}}"#,
        ),
        Reply::html(""),
        Reply::html("<html>No actual course proof</html>"),
    ]);
    let mut r = runtime();
    prove_info(&mut r, &server);
    let transport = r.identity.transport().clone();
    let learn = LearnClient::new(
        LearnClientConfig::new(
            &format!("{}{LEARN_MAP}", server.base().trim_end_matches('/')),
            CourseRole::Student,
        )
        .unwrap(),
    );
    assert!(
        r.ensure_learn_session_with_client(&transport, &user(), learn)
            .await
            .is_err()
    );
    assert!(!r.service_session_is_proven(ServiceId::Learn));
    assert!(r.service_session_is_proven(ServiceId::Info));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert_eq!(server.requests().len(), 4);
}
