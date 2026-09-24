use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::Duration,
};

use tsinghua_kit::tunet_client::{TunetClientError, TunetOnlineState};
use tsinghua_kit::{CampusHttpTransport, TunetClient, TunetClientConfig};
use tsinghua_kit::{tunet::HttpsEndpoint, tunet::TunetProfile, tunet::TunetProfileOverrides};

const IP: &str = "192.0.2.10";

struct FixtureResponse {
    status: &'static str,
    content_type: &'static str,
    headers: &'static str,
    body: &'static str,
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
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

fn write_response(stream: &mut TcpStream, response: &FixtureResponse) {
    let wire = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nConnection: close\r\n{headers}\r\n{body}",
        status = response.status,
        content_type = response.content_type,
        length = response.body.len(),
        headers = response.headers,
        body = response.body,
    );
    stream.write_all(wire.as_bytes()).expect("fixture response");
}

fn spawn_fixture(responses: Vec<FixtureResponse>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let mut requests = Vec::with_capacity(responses.len());
        for response in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            requests.push(read_request(&mut stream));
            write_response(&mut stream, &response);
        }
        requests
    });
    (format!("http://{address}"), server)
}

fn client(base_url: &str) -> TunetClient {
    let endpoint = base_url
        .parse::<reqwest::Url>()
        .expect("fixture endpoint URL");
    let endpoint = HttpsEndpoint::http(
        endpoint.host_str().expect("fixture endpoint host"),
        endpoint.port_or_known_default().expect("fixture port"),
    )
    .expect("fixture endpoint");
    let profile = TunetProfile::current_with_overrides(
        tsinghua_kit::tunet::AuthFamily::Auth4,
        TunetProfileOverrides {
            endpoint: Some(endpoint),
            ..TunetProfileOverrides::default()
        },
    )
    .expect("fixture profile");
    let config = TunetClientConfig::new(profile).expect("fixture config");
    let transport =
        CampusHttpTransport::with_timeout("THYou/tunet-contract", Duration::from_secs(3))
            .expect("fixture transport");
    TunetClient::with_transport(config, transport).expect("fixture client")
}

fn ok(body: &'static str) -> FixtureResponse {
    FixtureResponse {
        status: "200 OK",
        content_type: "application/javascript; charset=UTF-8",
        headers: "",
        body,
    }
}

#[tokio::test]
async fn every_public_network_operation_uses_real_routes_and_proves_state() {
    let (base_url, server) = spawn_fixture(vec![
        FixtureResponse {
            headers: "Set-Cookie: tunet-session=fixture; Path=/\r\n",
            ..ok("thyouChallenge({\"challenge\":\"fixture-token\",\"ecode\":0,\"error\":\"ok\"});")
        },
        ok(
            "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.10\",\"online_ip6\":\"\",\"online_device_total\":\"1\"});",
        ),
        ok("thyouChallenge({\"challenge\":\"fixture-token-2\",\"ecode\":0,\"error\":\"ok\"});"),
        ok("thyouPortal({\"ecode\":0,\"error\":\"ok\",\"suc_msg\":\"login_ok\"});"),
        ok(
            "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.10\",\"online_ip6\":\"\",\"online_device_total\":\"1\"});",
        ),
        ok("thyouPortal({\"ecode\":0,\"error\":\"ok\",\"suc_msg\":\"logout_ok\"});"),
        ok(
            "thyouStatus({\"error\":\"not_online_error\",\"online_ip\":\"\",\"online_ip6\":\"\",\"online_device_total\":\"0\"});",
        ),
        ok("thyouPortal({\"ecode\":0,\"error\":\"ok\",\"suc_msg\":\"logout_ok\"});"),
        ok(
            "thyouStatus({\"error\":\"not_online_error\",\"online_ip\":\"\",\"online_ip6\":\"\",\"online_device_total\":\"0\"});",
        ),
    ]);
    let client = client(&base_url);
    let params = [("ip", IP)];

    let challenge = client
        .challenge("fixture-user", &params)
        .await
        .expect("challenge proof");
    assert_eq!(challenge.token(), "fixture-token");

    let status = client.status(&params).await.expect("status response");
    assert!(status.is_online_proven_for_ip(IP));

    let login = client
        .login_with_password("fixture-user", "fixture-password", &params)
        .await
        .expect("login proof");
    assert_eq!(login.online_state_for_ip(IP), TunetOnlineState::Online);

    let logout = client
        .logout("fixture-user", &params)
        .await
        .expect("logout proof");
    assert!(logout.is_offline_proven_for_ip(IP));

    let disconnect = client
        .disconnect_verified("fixture-user", &params)
        .await
        .expect("disconnect proof");
    assert!(disconnect.is_offline_proven_for_ip(IP));

    let requests = server.join().expect("fixture server");
    assert_eq!(requests.len(), 9);
    assert!(requests[0].starts_with("GET /cgi-bin/get_challenge?username=fixture-user&ip=192.0.2.10&callback=thyouChallenge HTTP/1.1"));
    assert!(
        requests[1]
            .starts_with("GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1")
    );
    assert!(requests[2].starts_with("GET /cgi-bin/get_challenge?username=fixture-user&ip=192.0.2.10&callback=thyouChallenge HTTP/1.1"));
    assert!(requests[3].starts_with(
        "GET /cgi-bin/srun_portal?action=login&username=fixture-user&password=%7BMD5%7D"
    ));
    assert!(requests[3].contains("&ac_id=1&ip=192.0.2.10&n=200&type=1&callback=thyouPortal"));
    assert!(
        requests[4]
            .starts_with("GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1")
    );
    assert!(requests[5].starts_with("GET /cgi-bin/srun_portal?action=logout&username=fixture-user&ip=192.0.2.10&ac_id=1&callback=thyouPortal HTTP/1.1"));
    assert!(
        requests[6]
            .starts_with("GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1")
    );
    assert!(requests[7].starts_with("GET /cgi-bin/srun_portal?action=logout&username=fixture-user&ip=192.0.2.10&ac_id=1&callback=thyouPortal HTTP/1.1"));
    assert!(
        requests[8]
            .starts_with("GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1")
    );
    assert!(requests[1].contains("tunet-session=fixture"));
    assert!(requests[3].contains("tunet-session=fixture"));
    assert!(!requests[3].contains("fixture-password"));
}

#[tokio::test]
async fn positive_logout_ack_without_target_offline_proof_is_rejected() {
    let (base_url, server) = spawn_fixture(vec![
        ok("thyouPortal({\"ecode\":0,\"error\":\"ok\"});"),
        ok(
            "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.11\",\"online_ip6\":\"\",\"online_device_total\":\"1\"});",
        ),
        ok(
            "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.11\",\"online_ip6\":\"\",\"online_device_total\":\"1\"});",
        ),
        ok(
            "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.11\",\"online_ip6\":\"\",\"online_device_total\":\"1\"});",
        ),
        ok(
            "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.11\",\"online_ip6\":\"\",\"online_device_total\":\"1\"});",
        ),
    ]);
    let client = client(&base_url);
    let error = client
        .logout("fixture-user", &[("ip", IP)])
        .await
        .expect_err("error=ok alone must not prove disconnect");
    assert!(matches!(error, TunetClientError::OnlineStateUnproven));
    let requests = server.join().expect("fixture server");
    assert_eq!(requests.len(), 5);
}

#[tokio::test]
async fn direct_status_binds_the_public_state_to_the_requested_ip() {
    let (base_url, server) = spawn_fixture(vec![ok(
        "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.11\",\"online_ip6\":\"2001:db8::11\",\"online_device_total\":\"2\"});",
    )]);
    let client = client(&base_url);
    let status = client
        .status(&[("ip", IP)])
        .await
        .expect("status response should decode");
    assert_eq!(status.online_state(), TunetOnlineState::Unknown);
    assert!(!status.is_online_proven());
    assert!(!status.is_online_proven_for_ip(IP));
    assert_eq!(server.join().expect("fixture server").len(), 1);
}

#[tokio::test]
async fn usage_reads_the_real_status_route_and_rejects_missing_traffic_fields() {
    let (base_url, server) = spawn_fixture(vec![ok(
        "thyouStatus({\"error\":\"ok\",\"sum_bytes\":\"1234\",\"sum_seconds\":42,\"remain_bytes\":0,\"remain_seconds\":\"0\",\"user_balance\":\"8.10\"});",
    )]);
    let usage_client = client(&base_url);
    let usage = usage_client
        .usage(&[("ip", IP)])
        .await
        .expect("traffic and balance proof");
    assert_eq!(usage.used_bytes, 1234);
    assert_eq!(usage.used_seconds, 42);
    assert_eq!(usage.remaining_bytes, 0);
    assert_eq!(usage.remaining_seconds, 0);
    assert_eq!(usage.account_balance, "8.10");
    let requests = server.join().expect("fixture server");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .starts_with("GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1")
    );

    let (base_url, server) = spawn_fixture(vec![ok(
        "thyouStatus({\"error\":\"ok\",\"sum_bytes\":\"1234\",\"sum_seconds\":42,\"remain_bytes\":0,\"remain_seconds\":\"0\"});",
    )]);
    let second_client = client(&base_url);
    let error = second_client
        .usage(&[("ip", IP)])
        .await
        .expect_err("missing balance must stay unavailable");
    assert!(matches!(
        error,
        TunetClientError::UnsupportedVariant {
            operation: tsinghua_kit::tunet::TunetOperation::Status,
            ..
        }
    ));
    assert_eq!(server.join().expect("fixture server").len(), 1);
}

#[tokio::test]
async fn challenge_and_login_failures_stop_before_state_claims() {
    let (base_url, server) = spawn_fixture(vec![ok(
        "thyouChallenge({\"challenge\":\"fixture-token\",\"ecode\":1,\"error\":\"challenge_expire_error\"});",
    )]);
    let challenge_client = client(&base_url);
    let error = challenge_client
        .challenge("fixture-user", &[("ip", IP)])
        .await
        .expect_err("challenge rejection");
    assert!(matches!(error, TunetClientError::ChallengeRejected { .. }));
    assert_eq!(server.join().expect("fixture server").len(), 1);

    let (base_url, server) = spawn_fixture(vec![
        ok("thyouChallenge({\"challenge\":\"fixture-token\",\"ecode\":0,\"error\":\"ok\"});"),
        ok("thyouPortal({\"ecode\":1,\"error\":\"password_error\"});"),
    ]);
    let login_client = client(&base_url);
    let error = login_client
        .login_with_password("fixture-user", "fixture-password", &[("ip", IP)])
        .await
        .expect_err("login rejection");
    assert!(error.is_authentication_failure());
    assert_eq!(server.join().expect("fixture server").len(), 2);
}

#[tokio::test]
async fn status_unknown_and_http_failures_never_become_online() {
    let (base_url, server) = spawn_fixture(vec![
        ok("thyouStatus({\"error\":\"ok\",\"message\":\"accepted\"});"),
        ok("thyouStatus({\"error\":\"ok\",\"message\":\"accepted\"});"),
        ok("thyouStatus({\"error\":\"ok\",\"message\":\"accepted\"});"),
        ok("thyouStatus({\"error\":\"ok\",\"message\":\"accepted\"});"),
    ]);
    let client = client(&base_url);
    let error = client
        .status_verified(&[("ip", IP)])
        .await
        .expect_err("unknown status must not prove online");
    assert!(matches!(error, TunetClientError::OnlineStateUnproven));
    assert_eq!(server.join().expect("fixture server").len(), 4);
}

#[tokio::test]
async fn non_protocol_content_type_is_rejected_before_parsing() {
    let (base_url, server) = spawn_fixture(vec![FixtureResponse {
        status: "200 OK",
        content_type: "text/html; charset=UTF-8",
        headers: "",
        body: "<html>login</html>",
    }]);
    let client = client(&base_url);
    let error = client
        .status(&[("ip", IP)])
        .await
        .expect_err("HTML shell is not a protocol response");
    assert!(matches!(error, TunetClientError::UnsupportedContentType));
    assert_eq!(server.join().expect("fixture server").len(), 1);
}
