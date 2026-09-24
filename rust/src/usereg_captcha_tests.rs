use super::*;

#[test]
fn backend_repair_network_validation_accepts_bounded_json_with_legacy_text_mime() {
    let adapter = adapter();
    for mime in ["text/html; charset=UTF-8", "text/plain", "application/json"] {
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/validate-user",
            mime,
            br#"{"success":true,"message":"fixture-private-message"}"#,
        );
        let result = adapter.parse_validation_response(&response).unwrap();
        assert_eq!(result.outcome, UseregValidationOutcome::Accepted);
        assert!(!format!("{result:?}").contains("fixture-private"));
        // Parsing success alone never grants final login authority: the
        // encrypted password and exact captcha/page binding are still absent.
        assert!(result.encrypted_password.is_none() && result.binding.is_none());
    }
}

#[test]
fn backend_repair_network_text_mime_never_accepts_html_ambiguous_envelopes_or_other_routes() {
    let adapter = adapter();
    for (path, mime, body) in [
        ("site/validate-user", "text/html", "<html><form>login</form></html>".to_owned()),
        ("site/validate-user", "text/html", "{}".to_owned()),
        ("site/validate-user", "text/plain", r#"{"success":"true"}"#.to_owned()),
        ("site/validate-user", "text/html", "[]".to_owned()),
        ("site/validate-user", "text/html", String::new()),
        ("site/validate-user", "text/html", format!(r#"{{"success":true,"message":"{}"}}"#, "x".repeat(MAX_VALIDATION_BYTES))),
        ("site/validate-user", "image/png", r#"{"success":true}"#.to_owned()),
        ("login", "text/html", r#"{"success":true}"#.to_owned()),
        ("site/other", "application/json", r#"{"success":true}"#.to_owned()),
    ] {
        let response = UseregHttpResponse::fixture(StatusCode::OK,
            &format!("https://usereg.example.test/{path}"), mime, body.as_bytes());
        assert!(adapter.parse_validation_response(&response).is_err());
    }
    for (message, expected) in [
        ("验证码错误", UseregValidationOutcome::CaptchaRejected),
        ("用户名或密码错误", UseregValidationOutcome::CredentialsRejected),
        ("验证码已过期", UseregValidationOutcome::SessionExpired),
    ] {
        let response = UseregHttpResponse::fixture(StatusCode::OK,
            "https://usereg.example.test/site/validate-user", "text/html; charset=UTF-8",
            format!(r#"{{"success":false,"message":"{message}"}}"#).as_bytes());
        assert_eq!(adapter.parse_validation_response(&response).unwrap().outcome, expected);
    }
}

#[tokio::test]
async fn backend_repair_captcha_prepare_obeys_reference_order_and_preserves_one_cookie_context() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let png = crate::reference_test_support::captcha_png();
        let replies = [
            (
                "Content-Type: application/json\r\nSet-Cookie: captcha-session=fixture; Path=/\r\n",
                br#"{"hash1":101,"hash2":202,"url":"/site/captcha"}"#.as_slice(),
            ),
            ("Content-Type: image/png\r\n", png.as_slice()),
            ("Content-Type: text/html\r\n", PAGE.as_bytes()),
        ];
        for (headers, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            tx.send(read_http_request(&mut stream)).unwrap();
            write_http_response(&mut stream, headers, body);
        }
    });
    let adapter = UseregAdapter::new(
        UseregClient::new(
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .unwrap(),
        )
        .unwrap(),
    );
    let transport =
        CampusHttpTransport::with_timeout("THYou/captcha-fixture", Duration::from_secs(3)).unwrap();
    let fence = transport.replay_fence();
    let (page, image) = adapter
        .prepare_captcha_login(&transport, "fixture-cache-buster")
        .await
        .unwrap();
    assert_eq!(image.bytes, crate::reference_test_support::captcha_png());
    assert_eq!(image.content_type, "image/png");
    assert!(
        !fence.permits_probe_retry(&transport),
        "a captcha refresh mutates the authentication context"
    );
    let debug = format!("{page:?} {image:?}");
    assert!(!debug.contains("meta-csrf-fixture") && !debug.contains("form-csrf-fixture"));
    server.join().unwrap();
    let requests = rx.try_iter().collect::<Vec<_>>();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("GET /site/captcha?refresh=1 "));
    assert!(requests[1].starts_with("GET /site/captcha?_=fixture-cache-buster "));
    assert!(requests[2].starts_with("GET /login "));
    assert!(
        requests[1..]
            .iter()
            .all(|r| r.contains("captcha-session=fixture"))
    );
    assert!(
        requests
            .iter()
            .all(|r| !r.contains("LoginForm%5Bpassword%5D"))
    );
}

#[test]
fn backend_repair_captcha_adapter_requires_raster_headers_for_image_and_metadata() {
    let adapter = adapter();
    let png = crate::reference_test_support::captcha_png();
    let valid = UseregHttpResponse::fixture(
        StatusCode::OK,
        "https://usereg.example.test/site/captcha",
        "image/png",
        &png,
    );
    assert_eq!(
        adapter.parse_captcha_metadata(&valid).unwrap().byte_len,
        png.len()
    );
    assert_eq!(adapter.parse_captcha_image(&valid).unwrap().bytes, png);
    for (content_type, body) in [
        ("image/png", b"<html>login</html>".as_slice()),
        ("image/svg+xml", b"<svg/>".as_slice()),
        ("image/png", b"\x89PNG\r\n\x1a\n".as_slice()),
        ("image/jpeg", png.as_slice()),
    ] {
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha",
            content_type,
            body,
        );
        assert!(adapter.parse_captcha_metadata(&response).is_err());
        assert!(adapter.parse_captcha_image(&response).is_err());
    }
}
