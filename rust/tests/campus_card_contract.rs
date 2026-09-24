//! Public contract fixtures for the read-only campus-card adapter.
//!
//! These tests deliberately use a tiny TCP server instead of a mock HTTP
//! client.  That keeps the assertions at the wire boundary: the adapter must
//! reuse the Cookie jar populated by the SSO handoff, send the observed JSON
//! routes and fields, and reject an HTTP 200 login document as a proof.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};

use reqwest::StatusCode;
use serde_json::Value;
use tsinghua_kit::campus_card_read::CampusCardTransactionType;
use tsinghua_kit::{
    CampusCardAccountBinding, CampusCardAdapterConfig, CampusCardAdapterError, CampusCardClient,
    CampusCardReadProfile, CampusCardRequestPlan, CampusCardSsoProfile, CampusCardTransactionQuery,
    CampusHttpTransport,
};

#[test]
fn standard_sso_profile_keeps_the_observed_card_policy_and_target() {
    let profile = CampusCardSsoProfile::standard();
    assert_eq!(profile.identity_policy, "card");
    assert_eq!(
        profile.identity_target,
        "eea30cbedcaf97c69d28b2d92f22a259/0?/userindex"
    );
}

#[test]
fn the_direct_card_production_origin_is_accepted_without_network_access() {
    let config = CampusCardAdapterConfig::new("https://card.tsinghua.edu.cn/")
        .expect("production card origin is a valid configuration");
    assert_eq!(config.base_origin(), "https://card.tsinghua.edu.cn");
}

#[test]
fn serde_cannot_reintroduce_unvalidated_dates_pages_or_route_body_pairs() {
    let query = CampusCardTransactionQuery::new(
        "2026-09-01",
        "2026-09-07",
        CampusCardTransactionType::Any,
        50,
        0,
    )
    .expect("valid query");
    let mut serialized = serde_json::to_value(&query).expect("serialize query");

    serialized["start_date"]["value"] = serde_json::json!("2026-02-30");
    assert!(serde_json::from_value::<CampusCardTransactionQuery>(serialized).is_err());

    let mut serialized = serde_json::to_value(&query).expect("serialize query");
    serialized["page_size"] = serde_json::json!(0);
    assert!(serde_json::from_value::<CampusCardTransactionQuery>(serialized).is_err());

    let plan = CampusCardReadProfile::standard().transactions_request(query);
    let mut serialized = serde_json::to_value(&plan).expect("serialize plan");
    serialized["operation"] = serde_json::json!("verify_session");
    assert!(serde_json::from_value::<CampusCardRequestPlan>(serialized).is_err());
}

const ACCOUNT_RESPONSE: &str = r#"{
  "success": true,
  "data": null,
  "resultData": {
    "idserial": "student-001",
    "username": "Fixture Student",
    "engname": "Fixture Student",
    "departname": "Fixture Department",
    "engdepartname": "Fixture Department",
    "departid": 42,
    "sex": "1",
    "identifyeffectdate": "2026-01-01",
    "validatevalue": "2030-01-01",
    "baseAccount": {"balance": 12345},
    "cardInfos": [{
      "cardid": "card-001",
      "accstatus": "0",
      "lasttxdate": "2026-09-01 12:30:00",
      "maxconstolamt": 20000,
      "maxconsamt": 5000
    }]
  }
}"#;

const TRANSACTIONS_RESPONSE: &str = r#"{
  "success": true,
  "data": null,
  "resultData": {
    "rows": [{
      "id": 17,
      "summary": "Lunch",
      "txdate": "2026-09-01 12:30:00",
      "balance": 12000,
      "txamt": -345,
      "meraddr": "Fixture Hall",
      "mername": null,
      "txname": "Consumption"
    }]
  }
}"#;

#[tokio::test(flavor = "current_thread")]
async fn read_methods_reuse_sso_cookie_and_send_observed_wire_shapes() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let (requests_tx, requests_rx) = mpsc::channel::<String>();

    let server = thread::spawn(move || {
        let responses = [
            ("GET", "sso", "ok"),
            (
                "POST",
                "session",
                r#"{"success":false,"data":"0123456789abcdefgfVWr2PSaEthPFsRGZTf9vqBS4j9PHRF8SAlgXTelsT+IxlMOUS+Vv7HKRa/vgBXdFGkFRMHeq4LMoe+2jfHVw=="}"#,
            ),
            ("POST", "account", ACCOUNT_RESPONSE),
            ("POST", "transactions", TRANSACTIONS_RESPONSE),
        ];

        for (method, kind, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let request = read_http_request(&mut stream);
            requests_tx.send(request.clone()).expect("send request");

            if method == "GET" {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nSet-Cookie: card_session=fixture; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write SSO response");
                continue;
            }

            let expected_path = match kind {
                "session" => "/login/getUserInfoFromToken",
                "account" => "/business/getCardUserinfo",
                "transactions" => "/business/querySelfTradeList",
                _ => unreachable!(),
            };
            assert!(request.starts_with(&format!("POST {expected_path} HTTP/1.1")));
            assert!(request.lines().any(|line| {
                line.to_ascii_lowercase().starts_with("cookie:")
                    && line.contains("card_session=fixture")
            }));

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write card response");
        }
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport =
        CampusHttpTransport::with_timeout("THYou/campus-card-contract", Duration::from_secs(5))
            .expect("transport");

    // This is the same shared transport that the identity runtime uses for
    // the card SSO handoff.  The adapter receives a clone, which must retain
    // the same Cookie jar.
    let handoff = transport
        .client()
        .get(format!("http://{address}/sso"))
        .send()
        .await
        .expect("SSO handoff request");
    assert_eq!(handoff.status(), StatusCode::OK);

    let client = CampusCardClient::new(config, transport).expect("card client");
    let binding = CampusCardAccountBinding::new("student-001").expect("account binding");
    let session = client
        .probe_session_for(&binding)
        .await
        .expect("session proof");
    let account = client
        .read_account_for(&session, Some(&binding))
        .await
        .expect("account read");
    assert_eq!(account.balance_cents, 12345);

    let query = CampusCardTransactionQuery::new(
        "2026-09-01",
        "2026-09-07",
        CampusCardTransactionType::Consumption,
        50,
        2,
    )
    .expect("transaction query");
    let report = client
        .read_transactions(&session, &query)
        .await
        .expect("transaction read");
    assert_eq!(report.len(), 1);
    assert_eq!(report.transactions[0].amount_cents, -345);

    server.join().expect("fixture server");
    let requests = requests_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /sso HTTP/1.1"));

    let session_body = request_json(&requests[1]);
    assert_eq!(session_body, serde_json::json!({}));

    let account_body = request_json(&requests[2]);
    assert_eq!(account_body["idserial"], "student-001");
    assert_eq!(account_body.as_object().map(|object| object.len()), Some(1));

    let transactions_body = request_json(&requests[3]);
    assert_eq!(transactions_body["idserial"], "student-001");
    assert_eq!(transactions_body["starttime"], "2026-09-01");
    assert_eq!(transactions_body["endtime"], "2026-09-07");
    assert_eq!(transactions_body["tradetype"], 1);
    assert_eq!(transactions_body["pageSize"], 50);
    assert_eq!(transactions_body["pageNumber"], 2);
    assert_eq!(
        transactions_body.as_object().map(|object| object.len()),
        Some(6)
    );

    for request in requests.iter().skip(1) {
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("content-type: application/json"));
        assert!(lower.contains("accept: application/json"));
        assert!(lower.contains("user-agent: thyou/campus-card-contract"));
        assert!(lower.contains("cookie: card_session=fixture"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_http_200_login_document_cannot_prove_card_session() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept probe");
        let _ = read_http_request(&mut stream);
        let body = "<html><title>统一身份认证登录</title><form><input name=\"i_user\"><input name=\"i_pass\"></form></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write login page");
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport = CampusHttpTransport::with_timeout(
        "THYou/campus-card-login-page-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let client = CampusCardClient::new(config, transport).expect("card client");

    assert!(matches!(
        client.probe_session().await,
        Err(CampusCardAdapterError::SessionExpired)
    ));
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn an_http_200_plain_login_marker_is_session_expiry() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept probe");
        let _ = read_http_request(&mut stream);
        let body = "please log in before continuing";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write login marker");
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport = CampusHttpTransport::with_timeout(
        "THYou/campus-card-plain-login-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let client = CampusCardClient::new(config, transport).expect("card client");

    assert!(matches!(
        client.probe_session().await,
        Err(CampusCardAdapterError::SessionExpired)
    ));
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_empty_rows_array_is_a_valid_transaction_page() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let responses = [
            (
                "GET",
                "/sso",
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nSet-Cookie: card_session=empty-fixture; Path=/\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            ),
            (
                "POST",
                "/login/getUserInfoFromToken",
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 69\r\nConnection: close\r\n\r\n{\"success\":true,\"resultData\":{\"loginuser\":\"student-001\"},\"data\":null}",
            ),
            (
                "POST",
                "/business/querySelfTradeList",
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 53\r\nConnection: close\r\n\r\n{\"success\":true,\"resultData\":{\"rows\":[]},\"data\":null}",
            ),
        ];
        for (method, path, response) in responses {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with(&format!("{method} {path} HTTP/1.1")));
            if method == "POST" {
                assert!(request.lines().any(|line| {
                    line.to_ascii_lowercase().starts_with("cookie:")
                        && line.contains("card_session=empty-fixture")
                }));
            }
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport = CampusHttpTransport::with_timeout(
        "THYou/campus-card-empty-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    transport
        .client()
        .get(format!("http://{address}/sso"))
        .send()
        .await
        .expect("SSO handoff request");

    let client = CampusCardClient::new(config, transport).expect("card client");
    let binding = CampusCardAccountBinding::new("student-001").expect("account binding");
    let session = client
        .probe_session_for(&binding)
        .await
        .expect("session proof");
    let query = CampusCardTransactionQuery::new(
        "2026-09-01",
        "2026-09-01",
        CampusCardTransactionType::Any,
        100,
        0,
    )
    .expect("transaction query");
    let report = client
        .read_transactions(&session, &query)
        .await
        .expect("explicit empty transaction page");
    assert!(report.is_empty());
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn a_cross_origin_redirect_cannot_prove_card_session() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept probe");
        let _ = read_http_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://identity.example.test/login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write redirect");
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport = CampusHttpTransport::with_timeout(
        "THYou/campus-card-cross-origin-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let client = CampusCardClient::new(config, transport).expect("card client");

    assert!(matches!(
        client.probe_session().await,
        Err(CampusCardAdapterError::UnexpectedOrigin)
    ));
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn an_http_200_failure_envelope_cannot_become_a_session() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept probe");
        let _ = read_http_request(&mut stream);
        let body = r#"{"success":false,"message":"temporary card failure"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write failure");
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport = CampusHttpTransport::with_timeout(
        "THYou/campus-card-http-failure-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let client = CampusCardClient::new(config, transport).expect("card client");

    assert!(matches!(
        client.probe_session().await,
        Err(CampusCardAdapterError::InvalidSessionResponse)
    ));
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn an_account_record_for_another_serial_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let mismatched_account = ACCOUNT_RESPONSE.replace(
        "\"idserial\": \"student-001\"",
        "\"idserial\": \"student-002\"",
    );
    let server = thread::spawn(move || {
        let responses = [
            r#"{"success":true,"resultData":{"loginuser":"student-001"},"data":null}"#,
            mismatched_account.as_str(),
        ];
        for body in responses {
            let (mut stream, _) = listener.accept().expect("accept card request");
            let _ = read_http_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });

    let config =
        CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture card config");
    let transport = CampusHttpTransport::with_timeout(
        "THYou/campus-card-binding-contract",
        Duration::from_secs(5),
    )
    .expect("transport");
    let client = CampusCardClient::new(config, transport).expect("card client");
    let binding = CampusCardAccountBinding::new("student-001").expect("account binding");
    let session = client
        .probe_session_for(&binding)
        .await
        .expect("session proof");

    assert!(matches!(
        client.read_account(&session).await,
        Err(CampusCardAdapterError::AccountMismatch)
    ));
    server.join().expect("fixture server");
}

fn request_json(request: &str) -> Value {
    let body = request
        .split_once("\r\n\r\n")
        .expect("request has a body separator")
        .1;
    serde_json::from_str(body).expect("request body is JSON")
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end;
    loop {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).expect("read fixture request");
        assert!(count > 0, "fixture request ended before headers");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }

    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim())
        })
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).expect("read fixture body");
        assert!(count > 0, "fixture request ended before body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    String::from_utf8(bytes[..header_end + content_length].to_vec()).expect("UTF-8 request")
}
