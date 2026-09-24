use super::*;
#[test]
fn backend_repair_lastmile_electricity_https_same_host_callback_is_preserved_by_reference() {
    let flow = electricity_auth::ElectricityFlow::current();
    let target =
        Url::parse("https://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc%3D").unwrap();
    let broker = flow.to_broker(&target).unwrap();
    assert_eq!(
        broker.as_str(),
        "https://oauth.tsinghua.edu.cn/lb-auth/lbredirect?scheme=https&host=myhome.tsinghua.edu.cn&port=443&uri=/default.aspx?ticket=FIXTURE%2Babc%3D"
    );
    let runtime =
        CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap();
    let response = crate::identity_execution::IdentityHttpResponse::from_service_navigation(
        reqwest::StatusCode::OK,
        Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/redirect2Jsp").unwrap(),
        None,
        format!("<html>登录成功。正在重定向到<a href='{target}'>继续</a></html>"),
    );
    assert_eq!(
        electricity_auth::selected_target(runtime.identity.identity().client(), &response, &flow),
        Some(target)
    );
}
#[test]
fn backend_repair_lastmile_electricity_unrelated_domain_and_ports_still_rejected() {
    let flow = electricity_auth::ElectricityFlow::current();
    for url in [
        "https://evil.invalid/default.aspx?ticket=x",
        "https://myhome.tsinghua.edu.cn:8443/default.aspx?ticket=x",
        "https://user@myhome.tsinghua.edu.cn/default.aspx?ticket=x",
        "https://myhome.tsinghua.edu.cn/default.aspx?ticket=x&host=evil.invalid",
    ] {
        assert!(flow.to_broker(&Url::parse(url).unwrap()).is_err());
    }
}
use crate::reference_test_support::{FixtureServer, Reply};
fn runtime_with_info(server: &FixtureServer) -> CampusRuntime {
    let mut r =
        CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false)
            .unwrap();
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    for service in [ServiceId::Identity, ServiceId::Info] {
        r.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            r.coordinator.registry().bind_csrf(
                service,
                crate::protocol::CsrfToken::new("fixture-info-csrf").unwrap(),
            )
        });
        r.coordinator
            .mark_authenticated(service, user.clone(), None, csrf, None)
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
        Some(crate::info::OpaqueUrl::new(format!("{}info/f/home", server.base())).unwrap());
    r
}
#[tokio::test]
async fn backend_repair_lastmile_runtime_learn_handoff_404_alias_binds_only_new_learn_csrf() {
    let mapping =
        "/https/77726476706e69737468656265737421fcf2408e297e7c4377068ea48d546d30ca8cc97bcc/";
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-info-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"https://learn.tsinghua.edu.cn/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE"}}"#,
        ),
        Reply::html("<html>handoff</html>"),
        Reply {
            status: 404,
            headers: String::new(),
            body: String::new(),
        },
        Reply::html("<html><meta name='_csrf' content='fixture-learn-only'></html>"),
    ]);
    let mut r = runtime_with_info(&server);
    let transport = r.identity.transport().clone();
    let learn = LearnClient::new(
        LearnClientConfig::new(
            &format!("{}{mapping}", server.base().trim_end_matches('/')),
            CourseRole::Student,
        )
        .unwrap(),
    );
    let (_, csrf) = r
        .ensure_learn_session_with_client(
            &transport,
            &UserIdentity {
                username: "fixture-user".into(),
                display_name: None,
            },
            learn,
        )
        .await
        .unwrap();
    assert_eq!(csrf.service(), ServiceId::Learn);
    assert_eq!(csrf.as_csrf_token().as_str(), "fixture-learn-only");
    assert_eq!(server.requests().len(), 5);
    assert!(
        server.requests()[4].starts_with(&format!("GET {mapping}f/wlxt/index/course/student/ "))
    );
    assert!(r.service_session_is_proven(ServiceId::Info));
}
#[tokio::test]
async fn backend_repair_lastmile_electricity_https_broker_consumed_then_business_proof_required() {
    let identity = FixtureServer::new(vec![]);
    let vpn = FixtureServer::new(vec![
        Reply::html("<html>target landing</html>"),
        Reply::html(
            "<span id='Netweb_Home_electricity_DetailCtrl1_lblele'>12.50</span><span id='Netweb_Home_electricity_DetailCtrl1_lbltime'>2026-09-19 12:00:00</span>",
        ),
    ]);
    let broker = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}http/electric-fixture/default.aspx\r\n",
            vpn.base()
        ),
        body: String::new(),
    }]);
    let mut r = runtime_with_info(&vpn);
    let flow = electricity_auth::ElectricityFlow {
        vpn: WebVpnIdentityConfig::new(vpn.base(), broker.base(), identity.base()).unwrap(),
        mapped: Url::parse(&format!("{}http/electric-fixture/", vpn.base())).unwrap(),
    };
    r.finish_electricity_with_flow(
        &UserIdentity {
            username: "fixture-user".into(),
            display_name: None,
        },
        Url::parse("https://myhome.tsinghua.edu.cn/default.aspx?ticket=FIXTURE%2Babc").unwrap(),
        &flow,
    )
    .await
    .unwrap();
    assert!(r.electricity_service_is_proven());
    assert_eq!(broker.requests().len(), 1);
    assert_eq!(vpn.requests().len(), 2);
    assert!(identity.requests().is_empty());
    assert!(broker.requests()[0].contains(
        "scheme=https&host=myhome.tsinghua.edu.cn&port=443&uri=/default.aspx?ticket=FIXTURE%2Babc"
    ));
}
#[test]
fn backend_repair_lastmile_electricity_standard_protocol_mapping_stays_same_host_only() {
    let flow = electricity_auth::ElectricityFlow::current();
    let mapping = "77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9";
    for protocol in ["http", "https", "http-80", "https-443"] {
        let url = Url::parse(&format!(
            "https://webvpn.tsinghua.edu.cn/{protocol}/{mapping}/default.aspx?ticket=FIXTURE"
        ))
        .unwrap();
        assert_eq!(flow.to_broker(&url).unwrap(), url);
    }
    for suffix in [
        format!("http-8080/{mapping}/default.aspx"),
        format!("https/{mapping}extra/default.aspx"),
        format!("https/{mapping}/%2e%2e/other"),
        "https/unknown/default.aspx".to_owned(),
    ] {
        assert!(
            flow.to_broker(
                &Url::parse(&format!("https://webvpn.tsinghua.edu.cn/{suffix}")).unwrap()
            )
            .is_err()
        );
    }
}
#[test]
fn backend_repair_lastmile_new_phase_logs_reject_private_fields_and_values() {
    let root = std::env::temp_dir().join(format!("thyou-lastmile-privacy-{}", Uuid::new_v4()));
    let mut log = crate::telemetry::LogSession::start(
        &root,
        crate::telemetry::LogConfig::parse("trace", false).unwrap(),
    )
    .unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        let mut r = CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false)
            .unwrap();
        let message = r.record_business_failure("learn", "learn_course_home", "learn_home_http");
        assert_eq!(
            crate::telemetry::diagnostic_reason(&message),
            "learn_home_http"
        );
        let response = crate::identity_execution::IdentityHttpResponse::from_service_navigation(
            reqwest::StatusCode::OK,
            Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/redirect2Jsp").unwrap(),
            None,
            "<html>No success marker, PRIVATE-RESPONSE-DATA</html>".into(),
        );
        assert!(
            electricity_auth::selected_target(
                r.identity.identity().client(),
                &response,
                &electricity_auth::ElectricityFlow::current()
            )
            .is_none()
        );
        tracing::warn!(target:"tsinghua_kit::auth",event="news_navigation_boundary",reason="PRIVATE-CODE",url="https://private.invalid/?token=SECRET");
    });
    log.flush();
    let text = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    for secret in [
        "PRIVATE-RESPONSE-DATA",
        "PRIVATE-CODE",
        "private.invalid",
        "SECRET",
    ] {
        assert!(!text.contains(secret));
    }
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        rows.iter()
            .any(|r| r["fields"]["reason"] == "electricity_success_marker_missing")
    );
    drop(log);
    std::fs::remove_dir_all(root).unwrap();
}
