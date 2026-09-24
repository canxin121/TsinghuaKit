//! Follow-up contracts from the 2026-09-18 real run. Synthetic data only.
use super::*;
use crate::campus_card_read::{
    CampusCardAccountBinding, parse_card_account_response, parse_card_transactions_response,
};
use crate::protocol::SecondFactorChallenge;
use crate::reference_test_support::{FixtureServer, Reply};

fn runtime() -> CampusRuntime {
    let cache_path = std::env::temp_dir()
        .join(format!("thyou-runtime-service-repair-{}", Uuid::new_v4()))
        .join("cache.json");
    CampusRuntime::new_with_persistence(
        "auto".into(),
        false,
        cache_path.to_string_lossy().into_owned(),
        false,
    )
    .unwrap()
}

#[test]
fn backend_repair_service_followup_broker_script_and_meta_preserve_exact_opaque_query() {
    let client = identity_client(
        "https://id.fixture.invalid/",
        "https://oauth.fixture.invalid/",
        "https://vpn.fixture.invalid/",
    );
    let url = "https://oauth.fixture.invalid/thu-oauth/auth?ticket=fixture%2Babc&state=s1";
    for html in [
        format!("<script>window.location.href = '{url}';</script>"),
        format!("<meta http-equiv='refresh' content='0;url={url}'>"),
    ] {
        let anchor = client.parse_login_page(&html).anchor_ticket.unwrap();
        assert_eq!(anchor.href, url);
        assert!(anchor.ticket.is_none());
    }
    for url in [
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=x&%74icket=y",
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=%ZZ",
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=%0d%0a",
        "https://oauth.fixture.invalid/thu-oauth/../other?ticket=x",
    ] {
        assert!(!client.is_safe_handoff_redirect(url));
    }
    assert!(
        !client.is_verified_identity_continuation_url(
            &Url::parse(
                "https://id.fixture.invalid/do/off/ui/auth/login/checkSingle?ticket=fixture"
            )
            .unwrap()
        )
    );
}

#[test]
fn backend_repair_service_followup_precise_money_controls_no_fraction_roundtrip_or_coercion() {
    for amount in [
        "9007199254740993.0000000000001",
        "9223372036854775807.1",
        "-9223372036854775808.1",
        "1e-1",
        "1e+128",
        "1e-128",
        "\"100.00\"",
        "{\"unexpected\":1}",
    ] {
        assert!(parse_card_transactions_response(&transactions(amount, "100")).is_err());
    }
    for (amount, expected) in [
        ("-0.000e-10", 0),
        ("1200e-2", 12),
        ("1.2500e3", 1250),
        ("100e-2", 1),
    ] {
        assert_eq!(
            parse_card_transactions_response(&transactions(amount, "100"))
                .unwrap()
                .transactions[0]
                .amount_cents,
            expected
        );
    }
    let invalid_date = transactions("1.0", "100.0").replace("2026-09-18 12:00:00", "not-a-date");
    assert!(parse_card_transactions_response(&invalid_date).is_err());
    let wrong_id = account("100.0").replace("\"idserial\":\"fixture-user\"", "\"idserial\":1.0");
    assert!(parse_card_account_response(&wrong_id, None).is_err());
}

#[tokio::test]
async fn backend_repair_service_followup_runtime_card_failure_retains_exact_fixed_reason() {
    let body = account("1.1");
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#),
        Reply::json(&body),
    ]);
    let mut r = runtime();
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    r.card_client = Some(
        CampusCardClient::new(
            crate::campus_card_adapter::CampusCardAdapterConfig::new(server.base()).unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.probe_and_bind_card_session(&user).await.unwrap();
    let error = r.load_campus_card_account().await.unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "card_read_cents"
    );
    assert!(!error.contains("1.1"));
    assert!(!error.contains("fixture-user"));
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.service_session_is_proven(ServiceId::CampusCard));
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn backend_repair_service_followup_portal_reason_and_network_mismatch_are_not_success() {
    assert_eq!(
        crate::telemetry::diagnostic_reason(
            "WebVPN 门户未确认（portal_identity_entry_password_form）"
        ),
        "portal_identity_entry_password_form"
    );
    let other = crate::tunet_client::parse_status_response(
        r#"{"error":"ok","online_ip":"192.0.2.20","user_name":"fixture"}"#,
        None,
    )
    .unwrap();
    assert!(!other.is_online_proven_for_ip("192.0.2.21"));
    assert!(!other.is_offline_proven_for_ip("192.0.2.21"));
    assert_eq!(
        other.unproven_reason_for_ip("192.0.2.21"),
        "tunet_address_mismatch"
    );
    let unknown = crate::tunet_client::parse_status_response(
        r#"{"error":"ok","online":false,"online_ip":"192.0.2.21"}"#,
        None,
    )
    .unwrap();
    assert_eq!(
        unknown.unproven_reason_for_ip("192.0.2.21"),
        "tunet_state_unproven"
    );
}
fn identity_client(base: &str, broker: &str, gateway: &str) -> IdentityClient {
    let profile = runtime()
        .identity
        .identity()
        .client()
        .config()
        .profile
        .clone();
    let config = IdentityClientConfig::new(base, profile)
        .unwrap()
        .with_cookie_backed_handoff_route(broker, ["/thu-oauth/", "/lb-auth/"])
        .unwrap()
        .with_cookie_backed_handoff_route(gateway, ["/login", "/"])
        .unwrap();
    IdentityClient::new(config).unwrap()
}
fn pending(r: &mut CampusRuntime) {
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .require_second_factor(
            ServiceId::Identity,
            SecondFactorChallenge {
                methods: vec![SecondFactorMethod::Wechat],
                masked_phone: None,
                expires_at: None,
            },
            Some(UserIdentity {
                username: "fixture-user".into(),
                display_name: None,
            }),
        )
        .unwrap();
}

#[test]
fn backend_repair_service_followup_oauth_ticket_is_navigation_not_learn_ticket() {
    let client = identity_client(
        "https://id.fixture.invalid/",
        "https://oauth.fixture.invalid/",
        "https://vpn.fixture.invalid/",
    );
    let callback =
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=SYNTHETIC-TICKET&state=fixture";
    let page = client.parse_login_page(&format!("<a href='{callback}'>继续</a>"));
    let anchor = page
        .anchor_ticket
        .expect("the broker continuation must not disappear");
    assert_eq!(anchor.href, callback);
    assert!(
        anchor.ticket.is_none(),
        "not a Learn/Registrar service ticket"
    );
    assert!(client.is_safe_handoff_redirect(callback));
}

#[tokio::test]
async fn backend_repair_service_followup_primary_factor_consumes_broker_then_webvpn_cookie() {
    let gateway = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: text/html\r\nSet-Cookie: fixture-vpn=accepted; Path=/\r\n".into(),
        body: "<html>gateway ready</html>".into(),
    }]);
    let broker = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!("Location: {}login?code=fixture-code\r\n", gateway.base()),
        body: String::new(),
    }]);
    let callback = format!(
        "{}thu-oauth/auth?ticket=SYNTHETIC-TICKET&state=fixture",
        broker.base()
    );
    let identity = FixtureServer::new(vec![
        Reply::json(
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        ),
        Reply::html(&format!(
            "<html>登录成功。正在重定向到<a href='{callback}'>继续</a></html>"
        )),
    ]);
    let mut r = runtime();
    r.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        identity_client(identity.base(), broker.base(), gateway.base()),
        crate::transport::CampusHttpTransport::new("THYou/followup-fixture").unwrap(),
    ));
    pending(&mut r);
    let result = r
        .identity
        .complete_second_factor(&mut r.coordinator, SecondAuthMethod::Wechat, "123456")
        .await
        .unwrap();
    assert!(result.identity_ticket.is_none());
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(!r.service_session_is_proven(ServiceId::Info));
    assert_eq!(identity.requests().len(), 2);
    assert_eq!(broker.requests().len(), 1);
    assert_eq!(gateway.requests().len(), 1);
    assert!(!broker.requests()[0].contains("vericode"));
    assert!(!gateway.requests()[0].contains("SYNTHETIC-TICKET"));
}

#[tokio::test]
async fn backend_repair_service_followup_broker_failure_cannot_be_silently_skipped() {
    let gateway = FixtureServer::new(vec![]);
    let broker = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: "unavailable".into(),
    }]);
    let identity = FixtureServer::new(vec![
        Reply::json(
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        ),
        Reply::html(&format!(
            "<a href='{}thu-oauth/auth?ticket=FIXTURE'>继续</a>",
            broker.base()
        )),
    ]);
    let mut r = runtime();
    r.identity = IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
        identity_client(identity.base(), broker.base(), gateway.base()),
        crate::transport::CampusHttpTransport::new("THYou/followup-fixture").unwrap(),
    ));
    pending(&mut r);
    assert!(
        r.identity
            .complete_second_factor(&mut r.coordinator, SecondAuthMethod::Wechat, "123456")
            .await
            .is_err()
    );
    assert_eq!(broker.requests().len(), 1);
    assert!(gateway.requests().is_empty());
    assert!(
        r.identity
            .complete_second_factor(&mut r.coordinator, SecondAuthMethod::Wechat, "123456")
            .await
            .is_err()
    );
    assert_eq!(identity.requests().len(), 2, "consumed code never replayed");
}

#[test]
fn backend_repair_service_followup_broker_route_cannot_relax_other_ticket_boundaries() {
    let client = identity_client(
        "https://id.fixture.invalid/",
        "https://oauth.fixture.invalid/",
        "https://vpn.fixture.invalid/",
    );
    for url in [
        "https://evil.fixture.invalid/thu-oauth/auth?ticket=x",
        "https://oauth.fixture.invalid/not-oauth?ticket=x",
        "https://oauth.fixture.invalid/lb-auth/lbredirect?ticket=x",
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=",
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=x&ticket=y",
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=%00",
        "https://oauth.fixture.invalid/thu-oauth/auth?ticket=x#fragment",
        "https://user:password@oauth.fixture.invalid/thu-oauth/auth?ticket=x",
        "https://vpn.fixture.invalid/login?ticket=x",
    ] {
        assert!(
            !client.is_safe_handoff_redirect(url),
            "no broad ticket-bearing navigation permission"
        );
    }
}

fn account(amount: &str) -> String {
    format!(
        r#"{{"success":true,"resultData":{{"idserial":"fixture-user","username":"Fixture","departname":"Fixture","departid":1,"identifyeffectdate":"2026-09-18","validatevalue":"2027-09-18","baseAccount":{{"balance":{amount}}},"cardInfos":[{{"cardid":"fixture-card","accstatus":"0","lasttxdate":"","maxconstolamt":20000.00,"maxconsamt":5000.0}}]}}}}"#
    )
}
fn transactions(amount: &str, balance: &str) -> String {
    format!(
        r#"{{"success":true,"resultData":{{"rows":[{{"id":"fixture-row","summary":"Fixture","txdate":"2026-09-18 12:00:00","balance":{balance},"txamt":{amount},"meraddr":"","txname":"消费"}}]}}}}"#
    )
}

#[test]
fn backend_repair_service_followup_card_decimal_numeric_cents_remain_exact_integers() {
    let expected = CampusCardAccountBinding::new("fixture-user").unwrap();
    let result = parse_card_account_response(&account("12300.00"), Some(&expected)).unwrap();
    assert_eq!(result.balance_cents, 12300);
    assert_eq!(result.daily_limit_cents, 20000);
    assert_eq!(result.one_time_limit_cents, 5000);
    for amount in ["-1250.00", "-1.25e3", "-1250"] {
        let result = parse_card_transactions_response(&transactions(amount, "8800.0")).unwrap();
        assert_eq!(result.transactions[0].amount_cents, -1250);
        assert_eq!(result.transactions[0].post_balance_cents, 8800);
    }
}

#[test]
fn backend_repair_service_followup_card_fractional_fen_cannot_round_into_valid_money() {
    for amount in [
        "1.01",
        "1.0000000000000000000001",
        "-0.00000000000000000001",
        "9223372036854775808.0",
        "1e10000",
        "true",
        "null",
        "\"123.0\"",
    ] {
        assert!(
            parse_card_transactions_response(&transactions(amount, "100")).is_err(),
            "no rounding, overflow or string coercion"
        );
    }
}

#[test]
fn backend_repair_service_followup_card_large_integer_decimals_preserve_every_cent() {
    for (raw, expected) in [
        ("9007199254740993.0", 9007199254740993_i64),
        ("9223372036854775807.00", i64::MAX),
        ("-9223372036854775808.00", i64::MIN),
    ] {
        let result = parse_card_transactions_response(&transactions(raw, "100")).unwrap();
        assert_eq!(result.transactions[0].amount_cents, expected);
    }
}

#[tokio::test]
async fn backend_repair_service_followup_card_adapter_normalizes_before_envelope_roundtrip() {
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"success":true,"resultData":{"loginuser":"fixture-user"}}"#),
        Reply::json(&account("12300.00")),
        Reply::json(&transactions("-1250.00", "8800.00")),
    ]);
    let client = CampusCardClient::new(
        crate::campus_card_adapter::CampusCardAdapterConfig::new(server.base()).unwrap(),
        crate::transport::CampusHttpTransport::new("THYou/card-followup-fixture").unwrap(),
    )
    .unwrap();
    let expected = CampusCardAccountBinding::new("fixture-user").unwrap();
    let session = client.probe_session_for(&expected).await.unwrap();
    assert_eq!(
        client
            .read_account_for(&session, Some(&expected))
            .await
            .unwrap()
            .balance_cents,
        12300
    );
    let query = crate::campus_card_read::CampusCardTransactionQuery::new(
        "2026-09-18",
        "2026-09-18",
        crate::campus_card_read::CampusCardTransactionType::Any,
        100,
        0,
    )
    .unwrap();
    assert_eq!(
        client
            .read_transactions(&session, &query)
            .await
            .unwrap()
            .transactions[0]
            .amount_cents,
        -1250
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn backend_repair_service_followup_card_decimal_support_does_not_change_account_binding() {
    let expected = CampusCardAccountBinding::new("other-user").unwrap();
    assert!(matches!(
        parse_card_account_response(&account("12300.00"), Some(&expected)),
        Err(crate::campus_card_read::CampusCardParseError::AccountMismatch)
    ));
}

#[tokio::test]
async fn backend_repair_nonacademic_refresh_releases_success_for_later_expiry() {
    let mut r = runtime();
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    r.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    r.coordinator.begin_authentication(ServiceId::Info).unwrap();
    let csrf = r.coordinator.registry().bind_csrf(
        ServiceId::Info,
        crate::protocol::CsrfToken::new("fixture").unwrap(),
    );
    r.coordinator
        .mark_authenticated(ServiceId::Info, user, None, Some(csrf), None)
        .unwrap();
    r.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new("http://127.0.0.1:9/", "/target").unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.info_roaming_url =
        Some(crate::info::OpaqueUrl::new("http://127.0.0.1:9/target/home").unwrap());

    r.automatic_refresh_attempted.insert(ServiceId::Info);
    r.automatic_refresh_results.insert(ServiceId::Info, Ok(()));
    assert!(
        r.refresh_nonacademic_service_after_expiry(ServiceId::Info)
            .await
            .is_ok()
    );
    assert!(!r.automatic_refresh_attempted.contains(&ServiceId::Info));
    assert!(!r.automatic_refresh_results.contains_key(&ServiceId::Info));

    r.invalidate_service_session(ServiceId::Info);
    // Keep this synthetic second expiry local and deterministic. The new
    // cycle is allowed to claim its one-shot gate; stop at the in-progress
    // boundary so this unit test never touches the real WebVPN endpoint.
    assert!(r.automatic_refresh_attempted.insert(ServiceId::Info));
    let error = r
        .refresh_nonacademic_service_after_expiry(ServiceId::Info)
        .await
        .unwrap_err();
    assert!(error.contains("正在续接或续接结果未确认"));
    assert!(r.automatic_refresh_attempted.contains(&ServiceId::Info));
    assert!(!r.automatic_refresh_results.contains_key(&ServiceId::Info));
}

#[tokio::test]
async fn backend_repair_live_only_readers_use_existing_expiry_gates() {
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };

    let mut library = runtime();
    library
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    library
        .coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    library
        .coordinator
        .begin_authentication(ServiceId::Library)
        .unwrap();
    library
        .coordinator
        .mark_authenticated(
            ServiceId::Library,
            user.clone(),
            None,
            None,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    library.library_adapter = Some(
        crate::library_read::LibraryReadAdapter::try_with_transport(
            reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
            library.identity.transport().clone(),
        )
        .unwrap(),
    );
    library.automatic_refresh_results.insert(
        ServiceId::Library,
        Err("synthetic-library-expiry".to_owned()),
    );

    let error = library
        .load_library_seats(
            351,
            9001,
            "2026-09-11".to_owned(),
            "08:00".to_owned(),
            "22:00".to_owned(),
        )
        .await
        .unwrap_err();
    assert_eq!(error, "图书馆服务暂时不可用，请稍后重试");
    assert!(library.library_adapter.is_none());

    let mut card = runtime();
    card.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    card.coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    card.coordinator
        .begin_authentication(ServiceId::CampusCard)
        .unwrap();
    card.coordinator
        .mark_authenticated(
            ServiceId::CampusCard,
            user,
            None,
            None,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    card.automatic_refresh_results.insert(
        ServiceId::CampusCard,
        Err("synthetic-card-expiry".to_owned()),
    );

    let error = card.load_campus_card_account().await.unwrap_err();
    assert_eq!(error, "校园卡服务暂时不可用，请稍后重试");
    assert!(card.card_session.is_none());
}

#[tokio::test]
async fn backend_repair_cached_library_read_uses_existing_expiry_gate() {
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    let mut library = runtime();
    library
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    library
        .coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    library
        .coordinator
        .begin_authentication(ServiceId::Library)
        .unwrap();
    library
        .coordinator
        .mark_authenticated(
            ServiceId::Library,
            user,
            None,
            None,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    library.library_adapter = Some(
        crate::library_read::LibraryReadAdapter::try_with_transport(
            reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
            library.identity.transport().clone(),
        )
        .unwrap(),
    );
    library.automatic_refresh_results.insert(
        ServiceId::Library,
        Err("synthetic-library-expiry".to_owned()),
    );

    let error = library.load_library_area_tree_result().await.unwrap_err();

    assert_eq!(error, "图书馆服务暂时不可用，请稍后重试");
    assert!(library.library_adapter.is_none());
    assert!(
        library
            .automatic_refresh_results
            .contains_key(&ServiceId::Library)
    );
}

#[tokio::test]
async fn backend_repair_cached_info_read_uses_existing_expiry_gate() {
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };
    let mut info = runtime();
    info.coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    info.coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    info.coordinator
        .begin_authentication(ServiceId::Info)
        .unwrap();
    info.coordinator
        .mark_authenticated(
            ServiceId::Info,
            user,
            None,
            None,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    info.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new("http://127.0.0.1:9/", "/target").unwrap(),
            info.identity.transport().clone(),
        )
        .unwrap(),
    );
    info.automatic_refresh_results
        .insert(ServiceId::Info, Err("synthetic-info-expiry".to_owned()));

    let error = info.load_info_news(1, 10, None, None).await.unwrap_err();

    assert_eq!(error, "synthetic-info-expiry");
    assert!(info.info_adapter.is_some());
    assert!(
        info.automatic_refresh_results
            .contains_key(&ServiceId::Info)
    );
}

#[tokio::test]
async fn backend_repair_shared_info_child_read_uses_parent_expiry_gate() {
    let user = UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    };

    let mut classroom = runtime();
    classroom
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    classroom
        .coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .unwrap();
    classroom.classroom_adapter = Some(
        crate::classroom_read::ClassroomReadAdapter::try_with_transport(
            reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
            classroom.identity.transport().clone(),
        )
        .unwrap(),
    );
    classroom
        .automatic_refresh_results
        .insert(ServiceId::Info, Err("synthetic-info-expiry".to_owned()));

    let error = classroom.load_classroom_state(0, 1).await.unwrap_err();
    assert_eq!(error, "INFO 服务暂时不可用，请稍后重试");
    assert!(classroom.classroom_adapter.is_some());

    let mut electricity = runtime();
    electricity
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    electricity
        .coordinator
        .mark_authenticated(ServiceId::Identity, user, None, None, None)
        .unwrap();
    electricity.electricity_adapter = Some(
        crate::dorm_electricity_read::DormElectricityAdapter::try_with_transport(
            reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
            electricity.identity.transport().clone(),
        )
        .unwrap(),
    );
    electricity
        .automatic_refresh_results
        .insert(ServiceId::Info, Err("synthetic-info-expiry".to_owned()));

    let error = electricity.load_electricity_remainder().await.unwrap_err();
    assert_eq!(error, "INFO 服务暂时不可用，请稍后重试");
    assert!(electricity.electricity_adapter.is_some());
}

#[tokio::test]
async fn backend_repair_first_live_only_read_keeps_explicit_missing_session_error() {
    let mut library = runtime();
    let error = library
        .load_library_seats(
            351,
            9001,
            "2026-09-11".to_owned(),
            "08:00".to_owned(),
            "22:00".to_owned(),
        )
        .await
        .unwrap_err();
    assert_eq!(error, "图书馆服务会话未建立");
    assert!(library.automatic_refresh_attempted.is_empty());
    assert!(library.automatic_refresh_results.is_empty());

    let mut card = runtime();
    let error = card.load_campus_card_account().await.unwrap_err();
    assert_eq!(error, "校园卡服务会话未建立");
    assert!(card.automatic_refresh_attempted.is_empty());
    assert!(card.automatic_refresh_results.is_empty());
}
