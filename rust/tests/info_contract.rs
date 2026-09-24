use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
};

use tsinghua_kit::{
    CampusHttpTransport, CsrfToken, InfoError, InfoSessionAdapter, InfoSessionError,
    InfoWebVpnConfig, NewsParseError, NewsProfile, NewsSearchInput, ServiceId, ServiceSessionState,
    SessionCoordinator, UserIdentity, parse_news_detail, parse_news_list, parse_news_search,
    parse_online_app_redirect,
};

use tsinghua_kit::info_news::{NewsPageClassification, NewsPayloadError};

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let mut header_end = None;
    let mut content_length = 0_usize;

    loop {
        let count = stream.read(&mut buffer).expect("request");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);

        if header_end.is_none() {
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = position + 4;
                let header = String::from_utf8_lossy(&bytes[..end]);
                content_length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                header_end = Some(end);
            }
        }

        if header_end.is_some_and(|end| bytes.len() >= end + content_length) {
            break;
        }
    }

    String::from_utf8_lossy(&bytes).into_owned()
}

fn request_line(request: &str) -> &str {
    request.lines().next().unwrap_or_default()
}

fn request_body(request: &str) -> &str {
    request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("")
}

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).expect("response");
}

fn respond_ok(stream: &mut TcpStream, content_type: &str, body: &str) {
    respond(
        stream,
        "200 OK",
        &format!("Content-Type: {content_type}\r\n"),
        body,
    );
}

fn fixture_config(address: std::net::SocketAddr) -> InfoWebVpnConfig {
    InfoWebVpnConfig::new(format!("http://{address}/"), "/target").expect("INFO config")
}

fn identity_coordinator() -> SessionCoordinator {
    let mut coordinator = SessionCoordinator::new();
    coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity begin");
    coordinator
        .mark_authenticated(
            ServiceId::Identity,
            UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: Some("Fixture User".to_owned()),
            },
            None,
            None,
            None,
        )
        .expect("identity proof");
    coordinator
}

fn info_coordinator() -> SessionCoordinator {
    let mut coordinator = SessionCoordinator::new();
    coordinator
        .begin_authentication(ServiceId::Info)
        .expect("INFO begin");
    let csrf = coordinator.registry().bind_csrf(
        ServiceId::Info,
        CsrfToken::new("bound-csrf").expect("fixture CSRF"),
    );
    coordinator
        .mark_authenticated(
            ServiceId::Info,
            UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: None,
            },
            None,
            Some(csrf),
            None,
        )
        .expect("INFO proof");
    coordinator
}

#[tokio::test]
async fn live_fixture_executes_list_search_and_detail_with_shared_cookie_session() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("bootstrap");
        let request = read_request(&mut stream);
        assert!(request_line(&request).starts_with("GET /wengine-vpn/cookie?"));
        assert!(request.contains("host=info.tsinghua.edu.cn"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=bootstrap-csrf;");

        let (mut stream, _) = listener.accept().expect("portal");
        let request = read_request(&mut stream);
        assert!(
            request_line(&request)
                .starts_with("GET /target/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect?")
        );
        assert!(request.contains("yyfwid=fixture-app"));
        assert!(request.contains("_csrf=bootstrap-csrf"));
        assert!(request.contains("machine=p"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("cookie: xsrf-token=bootstrap-csrf")
        );
        let roaming = format!(r#"{{"object":{{"roamingurl":"http://{address}/target/home"}}}}"#);
        respond_ok(&mut stream, "application/json", &roaming);

        let (mut stream, _) = listener.accept().expect("handoff");
        let request = read_request(&mut stream);
        assert_eq!(request_line(&request), "GET /target/home HTTP/1.1");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("cookie: xsrf-token=bootstrap-csrf")
        );
        respond_ok(
            &mut stream,
            "text/html; charset=utf-8",
            "<!doctype html><html><body>INFO service</body></html>",
        );

        let (mut stream, _) = listener.accept().expect("proof bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=proof-csrf;");

        let (mut stream, _) = listener.accept().expect("news proof");
        let request = read_request(&mut stream);
        assert!(
            request_line(&request).starts_with("GET /target/b/info/xxfb_fg/xnzx/template/more?")
        );
        assert!(request.contains("oType=xs"));
        assert!(request.contains("lydw="));
        assert!(request.contains("lmid=all"));
        assert!(request.contains("currentPage=1"));
        assert!(request.contains("length=1"));
        assert!(request.contains("_csrf=proof-csrf"));
        respond_ok(
            &mut stream,
            "application/json",
            r#"{"object":{"dataList":[]}}"#,
        );

        let (mut stream, _) = listener.accept().expect("list bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=list-csrf;");

        let (mut stream, _) = listener.accept().expect("list");
        let request = read_request(&mut stream);
        assert!(
            request_line(&request).starts_with("GET /target/b/info/xxfb_fg/xnzx/template/more?")
        );
        assert!(request.contains("currentPage=2"));
        assert!(request.contains("length=10"));
        assert!(request.contains("lmid=LM_JWGG"));
        assert!(request.contains("_csrf=list-csrf"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("cookie: xsrf-token=list-csrf")
        );
        respond_ok(
            &mut stream,
            "application/json",
            r#"{"object":{"dataList":[{"bt":"Fixture news","url":"/article?xxid=fixture-id","xxid":"fixture-id","time":"2026-09-12 10:00:00","dwmc_show":"Fixture source","yxzd":"1-","lmid":"LM_JWGG","sfsc":false}]}}"#,
        );

        let (mut stream, _) = listener.accept().expect("search bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=search-csrf;");

        let (mut stream, _) = listener.accept().expect("search");
        let request = read_request(&mut stream);
        assert!(
            request_line(&request)
                .starts_with("POST /target/b/xnzx/search/info/xxfb_fg/teacher/getMobilePageList?")
        );
        assert!(request.contains("_csrf=search-csrf"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
        assert!(request_body(&request).contains("esParamClass="));
        assert!(request_body(&request).contains("params"));
        assert!(request_body(&request).contains("filterParams"));
        assert!(request_body(&request).contains("orderMap"));
        assert!(request_body(&request).contains("matchExact"));
        assert!(request_body(&request).contains("currentPage"));
        respond_ok(
            &mut stream,
            "application/json",
            r#"{"result":"success","object":{"resultsList":[{"bt":"<strong>Fixture search</strong>","url":"/article-search","xxid":"fixture-search","time":"2026/09/12 11:00","dwmc_show":"Fixture source","yxzd":null,"lmid":"LM_BGTG","sfsc":true}]}}"#,
        );

        let (mut stream, _) = listener.accept().expect("detail page bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=detail-page-csrf;");

        let (mut stream, _) = listener.accept().expect("detail article page");
        let request = read_request(&mut stream);
        assert_eq!(
            request_line(&request),
            "GET /target/article?source=fixture HTTP/1.1"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("cookie: xsrf-token=detail-page-csrf")
        );
        respond_ok(
            &mut stream,
            "text/html; charset=utf-8",
            r#"<html><script>var xxid = "fixture-detail";</script></html>"#,
        );

        let (mut stream, _) = listener.accept().expect("detail API bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=detail-api-csrf;");

        let (mut stream, _) = listener.accept().expect("detail API");
        let request = read_request(&mut stream);
        assert!(
            request_line(&request).starts_with("GET /target/b/info/xxfb_fg/xnzx/template/detail?")
        );
        assert!(request.contains("xxid=fixture-detail"));
        assert!(request.contains("preview="));
        assert!(request.contains("_csrf=detail-api-csrf"));
        respond_ok(
            &mut stream,
            "application/json",
            r#"{"result":"success","object":{"xxDto":{"xxid":"fixture-detail","bt":"Fixture detail","nr":"%3Cp%3EFixture%20body%3C%2Fp%3E","fjs_template":[{"wjid":"fixture-file","wjmc":"fixture.pdf"}]}}}"#,
        );
    });

    let transport = CampusHttpTransport::new("THYou/info-contract").expect("transport");
    let adapter = InfoSessionAdapter::new(fixture_config(address), transport).expect("adapter");
    let mut coordinator = identity_coordinator();
    let result = adapter
        .establish(
            &mut coordinator,
            UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: Some("Different display label".to_owned()),
            },
            "fixture-app",
        )
        .await
        .expect("INFO session");
    assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
    assert!(result.snapshot.csrf_present());

    let list = adapter
        .fetch_news_list(&coordinator, 2, 10, None, Some("LM_JWGG"))
        .await
        .expect("list");
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].title, "Fixture news");

    let search = adapter
        .search_news(
            &coordinator,
            &NewsSearchInput::new("fixture", 1)
                .with_channel_filter("办公通知")
                .with_exact_match(true),
        )
        .await
        .expect("search");
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].title, "Fixture search");

    let detail = adapter
        .fetch_news_detail(&coordinator, "/article?source=fixture")
        .await
        .expect("detail");
    assert_eq!(detail.title, "Fixture detail");
    assert_eq!(detail.summary, "Fixture body");
    assert_eq!(detail.attachments[0].name, "fixture.pdf");

    server.join().expect("fixture server");
}

#[tokio::test]
async fn live_fixture_classifies_http_200_login_and_route_drift_separately() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("login bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=login-csrf;");

        let (mut stream, _) = listener.accept().expect("login news");
        assert!(request_line(&read_request(&mut stream)).contains("/target/b/info/xxfb_fg/"));
        respond_ok(
            &mut stream,
            "text/html; charset=utf-8",
            "<!doctype html><html><form action=\"/do/off/ui/auth/login\"><input name=\"i_user\"><input name=\"i_pass\"></form></html>",
        );

        let (mut stream, _) = listener.accept().expect("route bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=route-csrf;");

        let (mut stream, _) = listener.accept().expect("route request");
        assert!(request_line(&read_request(&mut stream)).contains("/target/b/info/xxfb_fg/"));
        respond(
            &mut stream,
            "200 OK",
            "Location: https://id.example.test/login?ticket=fixture-ticket\r\nContent-Type: application/json\r\n",
            r#"{"object":{"dataList":[]}}"#,
        );

        let (mut stream, _) = listener.accept().expect("route drift bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=drift-csrf;");

        let (mut stream, _) = listener.accept().expect("route drift redirect");
        assert!(request_line(&read_request(&mut stream)).contains("/target/b/info/xxfb_fg/"));
        respond(
            &mut stream,
            "302 Found",
            "Location: /target/other\r\nContent-Type: text/plain\r\n",
            "route drift",
        );

        let (mut stream, _) = listener.accept().expect("route drift target");
        assert_eq!(
            request_line(&read_request(&mut stream)),
            "GET /target/other HTTP/1.1"
        );
        respond_ok(
            &mut stream,
            "application/json",
            r#"{"object":{"dataList":[]}}"#,
        );

        let (mut stream, _) = listener.accept().expect("same-origin login bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=location-login-csrf;");

        let (mut stream, _) = listener.accept().expect("same-origin login location");
        assert!(request_line(&read_request(&mut stream)).contains("/target/b/info/xxfb_fg/"));
        respond(
            &mut stream,
            "200 OK",
            "Location: /target/login\r\nContent-Type: application/json\r\n",
            r#"{"object":{"dataList":[]}}"#,
        );

        let (mut stream, _) = listener.accept().expect("malformed location bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(
            &mut stream,
            "text/plain",
            "XSRF-TOKEN=malformed-location-csrf;",
        );

        let (mut stream, _) = listener.accept().expect("malformed location");
        assert!(request_line(&read_request(&mut stream)).contains("/target/b/info/xxfb_fg/"));
        respond(
            &mut stream,
            "200 OK",
            "Location: %ZZ\r\nContent-Type: application/json\r\n",
            r#"{"object":{"dataList":[]}}"#,
        );

        let _ = address;
    });

    let adapter = InfoSessionAdapter::new(
        fixture_config(address),
        CampusHttpTransport::new("THYou/info-contract-failures").expect("transport"),
    )
    .expect("adapter");

    let coordinator = info_coordinator();
    let login_error = adapter
        .fetch_news_list(&coordinator, 1, 1, None, None)
        .await
        .expect_err("login page");
    assert!(matches!(login_error, InfoSessionError::LoginRequired));

    let cross_origin_error = adapter
        .fetch_news_list(&coordinator, 1, 1, None, None)
        .await
        .expect_err("cross-origin location");
    assert!(matches!(
        cross_origin_error,
        InfoSessionError::NewsUnexpectedOrigin
    ));

    let route_error = adapter
        .fetch_news_list(&coordinator, 1, 1, None, None)
        .await
        .expect_err("same-origin route drift");
    assert!(matches!(route_error, InfoSessionError::NewsUnexpectedPath));

    let location_login_error = adapter
        .fetch_news_list(&coordinator, 1, 1, None, None)
        .await
        .expect_err("same-origin login location");
    assert!(matches!(
        location_login_error,
        InfoSessionError::LoginRequired
    ));

    let malformed_location_error = adapter
        .fetch_news_list(&coordinator, 1, 1, None, None)
        .await
        .expect_err("malformed location");
    assert!(matches!(
        malformed_location_error,
        InfoSessionError::NewsUnexpectedOrigin
    ));

    server.join().expect("fixture server");
}

#[tokio::test]
async fn live_fixture_classifies_news_http_auth_status_as_login_required() {
    for status in ["401 Unauthorized", "403 Forbidden"] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("bootstrap");
            assert!(
                request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?")
            );
            respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=auth-status-csrf;");

            let (mut stream, _) = listener.accept().expect("news");
            assert!(request_line(&read_request(&mut stream)).contains("/target/b/info/xxfb_fg/"));
            respond(
                &mut stream,
                status,
                "Content-Type: application/json\r\n",
                r#"{"object":{"dataList":[]}}"#,
            );
        });

        let adapter = InfoSessionAdapter::new(
            fixture_config(address),
            CampusHttpTransport::new("THYou/info-contract-auth-status").expect("transport"),
        )
        .expect("adapter");
        let coordinator = info_coordinator();
        let error = adapter
            .fetch_news_list(&coordinator, 1, 1, None, None)
            .await
            .expect_err("HTTP auth status");
        assert!(matches!(error, InfoSessionError::LoginRequired));
        server.join().expect("fixture server");
    }
}

#[tokio::test]
async fn establish_classifies_same_origin_handoff_auth_status_before_location_path() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("bootstrap");
        assert!(request_line(&read_request(&mut stream)).starts_with("GET /wengine-vpn/cookie?"));
        respond_ok(&mut stream, "text/plain", "XSRF-TOKEN=handoff-csrf;");

        let (mut stream, _) = listener.accept().expect("portal");
        let request = read_request(&mut stream);
        assert!(request_line(&request).contains("onlineAppRedirect"));
        let roaming = format!(r#"{{"object":{{"roamingurl":"http://{address}/target/home"}}}}"#);
        respond_ok(&mut stream, "application/json", &roaming);

        let (mut stream, _) = listener.accept().expect("handoff");
        assert_eq!(
            request_line(&read_request(&mut stream)),
            "GET /target/home HTTP/1.1"
        );
        respond(
            &mut stream,
            "401 Unauthorized",
            "Location: /target/unrelated\r\nContent-Type: text/html; charset=utf-8\r\n",
            "login required",
        );
    });

    let adapter = InfoSessionAdapter::new(
        fixture_config(address),
        CampusHttpTransport::new("THYou/info-contract-handoff-auth-status").expect("transport"),
    )
    .expect("adapter");
    let mut coordinator = identity_coordinator();
    let error = adapter
        .establish(
            &mut coordinator,
            UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: None,
            },
            "fixture-app",
        )
        .await
        .expect_err("handoff authentication status must be recoverable");
    assert!(matches!(error, InfoSessionError::LoginRequired));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn establish_rejects_identity_user_mismatch_before_network_access() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let adapter = InfoSessionAdapter::new(
        fixture_config(address),
        CampusHttpTransport::new("THYou/info-contract-identity-boundary").expect("transport"),
    )
    .expect("adapter");

    let mut coordinator = identity_coordinator();
    let error = adapter
        .establish(
            &mut coordinator,
            UserIdentity {
                username: "different-user".to_owned(),
                display_name: Some("Fixture User".to_owned()),
            },
            "fixture-app",
        )
        .await
        .expect_err("identity user mismatch");
    assert!(matches!(error, InfoSessionError::IdentityUserMismatch));
    assert_eq!(
        coordinator.registry().snapshot_for(ServiceId::Info).state,
        ServiceSessionState::Anonymous
    );

    drop(listener);
}

#[test]
fn parsers_keep_empty_business_malformed_and_detail_diagnostics_distinct() {
    let empty = parse_news_list(r#"{"object":{"dataList":[]}}"#).expect("empty list");
    assert_eq!(empty.classification(), NewsPageClassification::EmptyList);

    assert!(matches!(
        parse_online_app_redirect(
            r#"{"message":"未登录","object":{"roamingurl":"https://webvpn.example/target"}}"#
        ),
        Err(InfoError::LoginRequired)
    ));
    assert!(matches!(
        parse_online_app_redirect(
            r#"{"message":"error","object":{"roamingurl":"https://webvpn.example/target"}}"#
        ),
        Err(InfoError::BusinessFailure)
    ));

    let noted_empty = parse_news_list(r#"{"message":"completed","object":{"dataList":[]}}"#)
        .expect("ordinary response note does not invalidate an empty list");
    assert_eq!(
        noted_empty.classification(),
        NewsPageClassification::EmptyList
    );

    assert!(matches!(
        parse_news_list(r#"{"message":"未登录","object":{"dataList":[]}}"#),
        Err(NewsParseError::LoginRequired)
    ));
    assert!(matches!(
        parse_news_search(r#"{"msg":"error","object":{"resultsList":[]}}"#),
        Err(NewsParseError::BusinessFailure { .. })
    ));

    assert!(matches!(
        parse_news_list(r#"{"success":false,"object":{"dataList":[]}}"#),
        Err(NewsParseError::BusinessFailure { .. })
    ));
    assert!(matches!(
        parse_news_search(r#"{"error":"permission denied","object":{"resultsList":[]}}"#),
        Err(NewsParseError::BusinessFailure { message: Some(message) })
            if message == "permission denied"
    ));
    assert!(matches!(
        parse_news_list(r#"{"result":"error","msg":"session expired","object":{"dataList":[]}}"#),
        Err(NewsParseError::LoginRequired)
    ));
    assert!(matches!(
        parse_news_list(r#"{"success":"false","object":{"dataList":[]}}"#),
        Err(NewsParseError::MalformedPayload {
            source: NewsPayloadError::SuccessNotBoolean
        })
    ));
    for login in [
        "<html><form><input name='i_user'><input name='i_pass'></form></html>",
        "<html><form><input name=i_user><input name=i_pass></form></html>",
    ] {
        assert!(matches!(
            parse_news_list(login),
            Err(NewsParseError::HtmlLoginPage)
        ));
    }

    assert!(matches!(
        parse_news_list(r#"{"object":{"dataList":[{}]}}"#),
        Err(NewsParseError::MalformedPayload {
            source: NewsPayloadError::MissingField { index: 0, .. }
        })
    ));
    assert!(matches!(
        parse_news_detail(r#"{"success":false,"object":{}}"#),
        Err(NewsParseError::DetailBusinessFailure { .. })
    ));
    assert!(matches!(
        parse_news_detail("not-json"),
        Err(NewsParseError::DetailMalformedJson { .. })
    ));

    let detail = parse_news_detail(
        r#"{"object":{"xxDto":{"xxid":"fixture-id","bt":"Title","nr":"<p>private body</p>"}}}"#,
    )
    .expect("detail");
    let debug = format!("{detail:?}");
    assert!(!debug.contains("fixture-id"));
    assert!(!debug.contains("private body"));
    assert!(!debug.contains("<p>"));

    let plan = NewsProfile::standard()
        .search_request(&NewsSearchInput::new("private keyword", 1))
        .expect("search plan");
    let debug = format!("{plan:?}");
    assert!(!debug.contains("private keyword"));
    assert!(!debug.contains("esParamClass=%"));
}

#[test]
fn news_links_decode_html_entities_without_decoding_opaque_percent_escapes() {
    let outcome = parse_news_list(
        r#"{"object":{"dataList":[
            {"bt":"实体链接","url":"/article?next=%26%2F&amp;mode=read","xxid":"opaque-link-1","time":"2026-09-12","dwmc_show":"来源","yxzd":"0-0","lmid":"LM","sfsc":false}
        ]}}"#,
    )
    .expect("news link fixture");

    assert_eq!(
        outcome.page().items[0].link.as_str(),
        "/article?next=%26%2F&mode=read"
    );
    assert!(parse_news_list(
        r#"{"object":{"dataList":[
            {"bt":"非法路径","url":"/article/%252e%252e/login","xxid":"opaque-link-2","time":"2026-09-12","dwmc_show":"来源","yxzd":"0-0","lmid":"LM","sfsc":false}
        ]}}"#,
    )
    .is_err());
}
