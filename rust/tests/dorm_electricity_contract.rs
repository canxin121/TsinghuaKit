//! Wire-level contract tests for the standalone dorm/electricity read slice.
//!
//! The fixture is a real TCP listener.  The first request simulates the
//! already completed INFO/WebVPN handoff only by setting a synthetic,
//! non-secret cookie; the adapter then has to reuse the same
//! `CampusHttpTransport` cookie jar for both exact legacy GET routes.  No
//! account, ticket, password, verification code, or production cookie is
//! present in this file.

// Exercise the actual compiled product modules. Copying transport.rs into
// this test crate omits its request-gate/WebVPN dependencies and can diverge
// from the runtime boundary that this contract is intended to verify.
use tsinghua_kit::{dorm_electricity_read, transport};

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use dorm_electricity_read::{
    DormElectricityAdapter, DormElectricityAdapterConfig, DormElectricityAdapterError,
    DormElectricityParseError, DormElectricityProfile, DormElectricitySessionPrerequisite,
    ELECTRICITY_PAYMENT_HISTORY_PATH, ELECTRICITY_RECHARGE_NOT_IN_READ_PROFILE,
    ELECTRICITY_REMAINDER_PATH, ELECTRICITY_WEBVPN_TARGET, parse_electricity_payment_history_html,
    parse_electricity_remainder_html,
};
use transport::CampusHttpTransport;

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("fixture read timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let count = stream.read(&mut buffer).expect("fixture request");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    extra_headers: &str,
    body: &[u8],
) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .expect("fixture response head");
    stream.write_all(body).expect("fixture response body");
}

fn assert_cookie_was_reused(request: &str) {
    assert!(request.lines().any(|line| {
        line.to_ascii_lowercase().starts_with("cookie:")
            && line.contains("handoff_state=shared-fixture")
    }));
}

fn remainder_html() -> &'static [u8] {
    br#"<!doctype html><html><body>
      <span id="Netweb_Home_electricity_DetailCtrl1_lblele">88.00</span>
      <span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026/9/1 1:29:01</span>
    </body></html>"#
}

fn history_html_with_gbk_status() -> Vec<u8> {
    let mut body = br#"<!doctype html><html><body><table class="myTable">
      <tr><th>header</th></tr>
      <tr><td></td><td>0</td><td>2026-09-01 01:02:03</td><td></td><td>10.00</td><td>"#
        .to_vec();
    // GBK for “已成功”.  The response declares charset=gbk, exercising the
    // shared reqwest decoder without placing non-ASCII fixture bytes in a
    // log or error value.
    body.extend_from_slice(&[0xD2, 0xD1, 0xB3, 0xC9, 0xB9, 0xA6]);
    body.extend_from_slice(
        br#"</td></tr>
      <tr><td>footer</td></tr>
    </table></body></html>"#,
    );
    body
}

#[test]
fn standard_profile_keeps_observed_routes_and_session_boundary() {
    let profile = DormElectricityProfile::standard();
    let remainder = profile.remainder_request();
    let history = profile.payment_history_request();

    assert_eq!(remainder.path, ELECTRICITY_REMAINDER_PATH);
    assert_eq!(history.path, ELECTRICITY_PAYMENT_HISTORY_PATH);
    assert_eq!(remainder.webvpn_target, ELECTRICITY_WEBVPN_TARGET);
    assert_eq!(history.webvpn_target, ELECTRICITY_WEBVPN_TARGET);
    assert_eq!(
        remainder.session_prerequisite,
        DormElectricitySessionPrerequisite::ExistingInfoWebVpnSession
    );
    assert_eq!(
        remainder.method,
        dorm_electricity_read::DormElectricityMethod::Get
    );
    assert_eq!(
        history.method,
        dorm_electricity_read::DormElectricityMethod::Get
    );
    assert!(!ELECTRICITY_RECHARGE_NOT_IN_READ_PROFILE.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn read_methods_reuse_shared_webvpn_cookie_and_exact_get_paths() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("handoff connection");
        let request = read_http_request(&mut stream);
        assert_eq!(request.lines().next(), Some("GET /handoff HTTP/1.1"));
        write_response(
            &mut stream,
            "200 OK",
            "text/plain; charset=utf-8",
            "Set-Cookie: handoff_state=shared-fixture; Path=/\r\n",
            b"handoff-ready",
        );

        let (mut stream, _) = listener.accept().expect("remainder connection");
        let request = read_http_request(&mut stream);
        assert_eq!(
            request.lines().next(),
            Some("GET /opaque-mapping/Netweb_List/Netweb_Home_electricity_Detail.aspx HTTP/1.1")
        );
        assert_cookie_was_reused(&request);
        write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=gbk",
            "",
            remainder_html(),
        );

        let (mut stream, _) = listener.accept().expect("history connection");
        let request = read_http_request(&mut stream);
        assert_eq!(
            request.lines().next(),
            Some("GET /opaque-mapping/Netweb_List/netweb_ele_pay_record.aspx HTTP/1.1")
        );
        assert_cookie_was_reused(&request);
        let body = history_html_with_gbk_status();
        write_response(&mut stream, "200 OK", "text/html; charset=gbk", "", &body);
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/dorm-electricity-contract",
        Duration::from_secs(5),
    )
    .expect("shared transport");
    transport
        .client()
        .get(format!("http://{address}/handoff"))
        .send()
        .await
        .expect("synthetic WebVPN handoff");

    let adapter = DormElectricityAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("fixture base URL"),
        transport,
    )
    .expect("electricity adapter");

    let remainder = adapter
        .read_remainder_with_proof()
        .await
        .expect("remainder");
    assert_eq!(remainder.value.remainder, 88.0);
    assert_eq!(remainder.value.update_time, "2026/9/1 1:29:01");
    assert!(adapter.business_proof_matches(&remainder.proof));

    let history = adapter
        .read_payment_history_with_proof()
        .await
        .expect("payment history");
    assert_eq!(history.value.len(), 1);
    assert!(adapter.business_proof_matches(&history.proof));
    let record = &history.value.records[0];
    assert_eq!(record.sequence, Some(0));
    assert_eq!(record.occurred_at, "2026-09-01 01:02:03");
    assert_eq!(record.amount, 10.0);
    assert_eq!(record.status, "已成功");
    assert_eq!(record.source_columns[4], "10.00");

    server.join().expect("fixture server");
    let other = DormElectricityAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("second fixture URL"),
        adapter.transport().clone(),
    )
    .expect("second electricity adapter");
    assert!(!other.business_proof_matches(&remainder.proof));
}

#[tokio::test(flavor = "current_thread")]
async fn a_full_electricity_handoff_page_url_is_reduced_to_its_mapping_root() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("electricity request");
        let request = read_http_request(&mut stream);
        assert_eq!(
            request.lines().next(),
            Some("GET /http/opaque-token/Netweb_List/Netweb_Home_electricity_Detail.aspx HTTP/1.1")
        );
        write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            "",
            remainder_html(),
        );
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/dorm-electricity-full-handoff-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let base_url = format!("http://{address}/http/opaque-token{ELECTRICITY_REMAINDER_PATH}");
    let adapter = DormElectricityAdapter::try_with_transport(
        base_url.parse().expect("fixture URL"),
        transport,
    )
    .expect("adapter");

    let remainder = adapter.read_remainder().await.expect("remainder");
    assert_eq!(remainder.remainder, 88.0);
    server.join().expect("fixture server");
}

#[test]
fn empty_history_requires_header_and_footer_and_missing_table_is_not_empty() {
    let empty = r#"<!doctype html><html><table class="myTable">
        <tr><th>header</th></tr><tr><td>footer</td></tr>
    </table></html>"#;
    let parsed = parse_electricity_payment_history_html(empty).expect("valid empty history");
    assert!(parsed.is_empty());

    let missing = r#"<!doctype html><html><body>没有历史记录</body></html>"#;
    assert!(matches!(
        parse_electricity_payment_history_html(missing),
        Err(DormElectricityParseError::MissingHistoryTable)
    ));

    let only_header =
        r#"<!doctype html><html><table class="myTable"><tr><th>header</th></tr></table></html>"#;
    assert!(matches!(
        parse_electricity_payment_history_html(only_header),
        Err(DormElectricityParseError::InvalidHistoryTable)
    ));
}

#[test]
fn parser_validates_login_expiry_dates_numbers_and_history_shape() {
    let login = r#"<!doctype html><form><input id="net_Default_LoginCtrl1_txtUserName"><input id="net_Default_LoginCtrl1_txtUserPwd"></form></html>"#;
    assert!(matches!(
        parse_electricity_remainder_html(login),
        Err(DormElectricityParseError::LoginPage)
    ));

    let expired = r#"<!doctype html><html><body>WebVPN timeout</body></html>"#;
    assert!(matches!(
        parse_electricity_remainder_html(expired),
        Err(DormElectricityParseError::ExpiredPage)
    ));
    assert!(matches!(
        parse_electricity_remainder_html("WebVPN timeout"),
        Err(DormElectricityParseError::ExpiredPage)
    ));
    assert!(matches!(
        parse_electricity_remainder_html("\u{feff}  \n"),
        Err(DormElectricityParseError::EmptyBody)
    ));

    let invalid_number = r#"<!doctype html><html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">NaN</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-09-01 01:02:03</span></html>"#;
    assert!(matches!(
        parse_electricity_remainder_html(invalid_number),
        Err(DormElectricityParseError::InvalidNumber { field: "remainder" })
    ));

    let invalid_date = r#"<!doctype html><html><span id="Netweb_Home_electricity_DetailCtrl1_lblele">1</span><span id="Netweb_Home_electricity_DetailCtrl1_lbltime">2026-02-30 01:02:03</span></html>"#;
    assert!(matches!(
        parse_electricity_remainder_html(invalid_date),
        Err(DormElectricityParseError::InvalidTimestamp {
            field: "update_time"
        })
    ));

    let invalid_history = r#"<!doctype html><html><table class="myTable">
      <tr><th>header</th></tr>
      <tr><td></td><td>0</td><td>2026-09-01 01:02:03</td><td></td><td>not-a-number</td><td>已成功</td></tr>
      <tr><td>footer</td></tr>
    </table></html>"#;
    assert!(matches!(
        parse_electricity_payment_history_html(invalid_history),
        Err(DormElectricityParseError::InvalidHistoryAmount { row: 0 })
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn http_200_login_page_cannot_prove_a_service_session() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("login request");
        let request = read_http_request(&mut stream);
        assert_eq!(
            request.lines().next(),
            Some("GET /opaque-mapping/Netweb_List/Netweb_Home_electricity_Detail.aspx HTTP/1.1")
        );
        let body = br#"<!doctype html><html><form><input id="net_Default_LoginCtrl1_txtUserName"><input id="net_Default_LoginCtrl1_txtUserPwd"></form></html>"#;
        write_response(&mut stream, "200 OK", "text/html; charset=utf-8", "", body);
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/dorm-electricity-login-page-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let adapter = DormElectricityAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("fixture URL"),
        transport,
    )
    .expect("adapter");

    assert!(matches!(
        adapter.read_remainder().await,
        Err(DormElectricityAdapterError::SessionExpired)
    ));
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn http_200_non_html_cannot_become_an_empty_electricity_result() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("JSON request");
        let request = read_http_request(&mut stream);
        assert_eq!(
            request.lines().next(),
            Some("GET /opaque-mapping/Netweb_List/Netweb_Home_electricity_Detail.aspx HTTP/1.1")
        );
        write_response(
            &mut stream,
            "200 OK",
            "application/json",
            "",
            br#"{"data":[]}"#,
        );
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/dorm-electricity-json-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let adapter = DormElectricityAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("fixture URL"),
        transport,
    )
    .expect("adapter");

    assert!(matches!(
        adapter.read_remainder().await,
        Err(DormElectricityAdapterError::UnexpectedContentType)
    ));
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn electricity_rejects_cross_origin_locations_and_login_redirects() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("cross-origin request");
        let _ = read_http_request(&mut stream);
        write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            "Location: https://outside.example.test/login\r\n",
            remainder_html(),
        );
    });
    let transport = CampusHttpTransport::with_timeout(
        "THYou/dorm-electricity-cross-origin-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let adapter = DormElectricityAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("fixture URL"),
        transport,
    )
    .expect("adapter");
    assert!(matches!(
        adapter.read_remainder().await,
        Err(DormElectricityAdapterError::UnexpectedOrigin)
    ));
    server.join().expect("fixture server");

    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("redirect request");
        let _ = read_http_request(&mut stream);
        write_response(
            &mut stream,
            "302 Found",
            "text/html; charset=utf-8",
            "Location: /opaque-mapping/login\r\n",
            b"",
        );
        let (mut stream, _) = listener.accept().expect("login request");
        let request = read_http_request(&mut stream);
        assert_eq!(
            request.lines().next(),
            Some("GET /opaque-mapping/login HTTP/1.1")
        );
        write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            "",
            br#"<!doctype html><html><form><input id="net_Default_LoginCtrl1_txtUserName"><input id="net_Default_LoginCtrl1_txtUserPwd"></form></html>"#,
        );
    });
    let transport = CampusHttpTransport::with_timeout(
        "THYou/dorm-electricity-login-redirect-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let adapter = DormElectricityAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("fixture URL"),
        transport,
    )
    .expect("adapter");
    assert!(matches!(
        adapter.read_remainder().await,
        Err(DormElectricityAdapterError::SessionExpired)
    ));
    server.join().expect("fixture server");
}

#[test]
fn invalid_base_urls_and_non_read_routes_are_rejected_by_the_profile() {
    assert!(matches!(
        DormElectricityAdapterConfig::new("https://user:password@example.test/opaque/"),
        Err(DormElectricityAdapterError::InvalidBaseUrl)
    ));
    assert!(matches!(
        DormElectricityAdapterConfig::new("https://example.test/opaque/?ticket=opaque"),
        Err(DormElectricityAdapterError::InvalidBaseUrl)
    ));
    assert!(matches!(
        DormElectricityAdapterConfig::new(
            "https://example.test/http/opaque-token/Netweb_List/Netweb_Home_electricity_Detail.aspx?unknown=1"
        ),
        Err(DormElectricityAdapterError::InvalidBaseUrl)
    ));

    let profile = DormElectricityProfile::standard();
    assert_eq!(
        profile.remainder_request().method,
        dorm_electricity_read::DormElectricityMethod::Get
    );
    assert_eq!(
        profile.payment_history_request().method,
        dorm_electricity_read::DormElectricityMethod::Get
    );
    assert_ne!(
        profile.remainder_request().path,
        ELECTRICITY_RECHARGE_NOT_IN_READ_PROFILE
    );
}
