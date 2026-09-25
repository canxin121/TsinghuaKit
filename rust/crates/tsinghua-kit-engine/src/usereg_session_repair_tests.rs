use super::*;

#[test]
fn backend_repair_usereg_nested_dashboard_containers_keep_both_business_tables() {
    let home = HOME_WITH_DEVICE
        .replace(
            "<html><body>",
            "<html><body><div class=\"container\"><div class=\"row\"><div class=\"column\">",
        )
        .replace("</body></html>", "</div></div></div></body></html>");
    let devices = parse_devices(&home).expect("nested device table");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, "17");
    let balance = parse_balance(&home).expect("nested balance table");
    assert_eq!(balance.account_balance, "8.10");
}

#[test]
fn backend_repair_usereg_nested_widget_lookup_ignores_script_and_comment_decoys() {
    let fake = r#"<html><body>
        <!-- <div id="w1-container"><table><tbody></tbody></table></div> -->
        <script>const widget = '<div id="w3-container"><table><tbody><tr><td>fake</td></tr></tbody></table></div>';</script>
        <div class="container"><div class="row"><p>unrelated page</p></div></div>
        </body></html>"#;
    assert!(matches!(
        parse_devices(fake),
        Err(UseregAdapterError::DeviceDataUnavailable)
    ));
    assert!(parse_balance(fake).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn backend_repair_usereg_redirect_home_without_balance_uses_bound_account_proof() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
    let address = listener.local_addr().expect("fixture address");
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let responses = [
            ("200 OK", "Content-Type: text/html\r\n", PAGE),
            (
                "200 OK",
                "Content-Type: text/html\r\n",
                r#"{"success":true}"#,
            ),
            (
                "302 Found",
                "Location: /home\r\nContent-Type: text/html\r\n",
                "",
            ),
            (
                "200 OK",
                "Content-Type: text/html\r\n",
                r#"<html><body><input type="hidden" name="_csrf-8800" value="home-csrf">
                  <div id="w1-container"><table><tbody></tbody></table></div></body></html>"#,
            ),
            ("200 OK", "Content-Type: text/html\r\n", USERS_WITH_ACCOUNT),
        ];
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().expect("request");
            requests_tx
                .send(read_http_request(&mut stream))
                .expect("request capture");
            write_http_response_with_status(&mut stream, status, headers, body);
        }
    });

    let config = UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
        .expect("config");
    let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
    let transport = CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
        .expect("transport");
    let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
    let page = adapter.fetch_login_page(&transport).await.expect("page");
    let validation = adapter
        .validate_user(&transport, &page, &credentials, "ABCD")
        .await
        .expect("validation");
    assert_eq!(validation.outcome, UseregValidationOutcome::Accepted);
    let result = adapter
        .login_after_validation(&transport, &page, &credentials, "ABCD", None, validation)
        .await
        .expect("account-bound session proof");
    assert_eq!(result.state, UseregAuthenticationState::Authenticated);

    server.join().expect("fixture server");
    let requests = requests_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /login ")).count(), 1);
    assert!(requests[4].starts_with("GET /users HTTP/1.1"));
}

#[tokio::test(flavor = "current_thread")]
async fn backend_repair_usereg_each_home_read_ignores_unrelated_missing_table() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        for body in [
            r#"<html><body><input type="hidden" name="_csrf-8800" value="home-csrf">
              <div id="w1-container"><table><tbody></tbody></table></div></body></html>"#,
            r#"<html><body><input type="hidden" name="_csrf-8800" value="home-csrf">
              <div id="w3-container"><table><tbody><tr><td>学生</td><td>0</td>
              <td>0</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div></body></html>"#,
        ] {
            let (mut stream, _) = listener.accept().expect("home request");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("GET /home HTTP/1.1"));
            write_http_response(&mut stream, "Content-Type: text/html\r\n", body);
        }
    });
    let config = UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
        .expect("config");
    let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
    let transport = CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
        .expect("transport");
    assert!(adapter.read_online_devices(&transport).await.expect("devices").is_empty());
    assert_eq!(
        adapter.read_balance(&transport).await.expect("balance").account_balance,
        "8.10"
    );
    server.join().expect("fixture server");
}

#[tokio::test(flavor = "current_thread")]
async fn backend_repair_usereg_sparse_home_rejects_mismatched_account_without_replaying_login() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
    let address = listener.local_addr().expect("fixture address");
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let wrong_account = USERS_WITH_ACCOUNT.replace("fixture-user", "someone-else");
        for (headers, body) in [
            ("Content-Type: text/html\r\n", PAGE.to_owned()),
            ("Content-Type: text/html\r\n", r#"{"success":true}"#.into()),
            (
                "Content-Type: text/html\r\n",
                r#"<html><body><div id="w1-container"></div></body></html>"#.into(),
            ),
            ("Content-Type: text/html\r\n", wrong_account),
        ] {
            let (mut stream, _) = listener.accept().expect("request");
            requests_tx
                .send(read_http_request(&mut stream))
                .expect("request capture");
            write_http_response(&mut stream, headers, &body);
        }
    });
    let config = UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
        .expect("config");
    let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
    let transport = CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
        .expect("transport");
    let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
    let page = adapter.fetch_login_page(&transport).await.expect("page");
    let validation = adapter
        .validate_user(&transport, &page, &credentials, "ABCD")
        .await
        .expect("validation");
    assert!(matches!(
        adapter
            .login_after_validation(&transport, &page, &credentials, "ABCD", None, validation)
            .await,
        Err(UseregAdapterError::AccountMismatch)
    ));
    server.join().expect("fixture server");
    let requests = requests_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /login ")).count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn backend_repair_usereg_explicit_login_form_never_uses_account_probe_as_success() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
    let address = listener.local_addr().expect("fixture address");
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for body in [PAGE, r#"{"success":true}"#, LOGIN_FORM] {
            let (mut stream, _) = listener.accept().expect("request");
            requests_tx
                .send(read_http_request(&mut stream))
                .expect("request capture");
            write_http_response(&mut stream, "Content-Type: text/html\r\n", body);
        }
    });
    let config = UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
        .expect("config");
    let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
    let transport = CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
        .expect("transport");
    let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
    let page = adapter.fetch_login_page(&transport).await.expect("page");
    let validation = adapter
        .validate_user(&transport, &page, &credentials, "ABCD")
        .await
        .expect("validation");
    let result = adapter
        .login_after_validation(&transport, &page, &credentials, "ABCD", None, validation)
        .await
        .expect("explicit login state");
    assert_eq!(result.state, UseregAuthenticationState::LoginRequired);
    server.join().expect("fixture server");
    let requests = requests_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests.iter().filter(|r| r.starts_with("POST /login ")).count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn backend_repair_usereg_home_probe_becoming_sparse_still_checks_same_account() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
    let address = listener.local_addr().expect("fixture address");
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for body in [
            PAGE,
            r#"{"success":true}"#,
            HOME_WITH_DEVICE,
            r#"<html><body><div id="w1-container"><table><tbody></tbody></table></div></body></html>"#,
            USERS_WITH_ACCOUNT,
        ] {
            let (mut stream, _) = listener.accept().expect("request");
            requests_tx
                .send(read_http_request(&mut stream))
                .expect("request capture");
            write_http_response(&mut stream, "Content-Type: text/html\r\n", body);
        }
    });
    let config = UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
        .expect("config");
    let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
    let transport = CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
        .expect("transport");
    let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
    let page = adapter.fetch_login_page(&transport).await.expect("page");
    let validation = adapter
        .validate_user(&transport, &page, &credentials, "ABCD")
        .await
        .expect("validation");
    let result = adapter
        .login_after_validation(&transport, &page, &credentials, "ABCD", None, validation)
        .await
        .expect("same-account proof");
    assert_eq!(result.state, UseregAuthenticationState::Authenticated);
    assert!(result.account_bound);
    server.join().expect("fixture server");
    let requests = requests_rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 5);
    assert!(requests[3].starts_with("GET /home HTTP/1.1"));
    assert!(requests[4].starts_with("GET /users HTTP/1.1"));
}
