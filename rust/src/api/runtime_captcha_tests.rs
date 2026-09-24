//! New-only CAPTCHA lifecycle contracts; no real accounts or campus requests.
use super::*;

#[tokio::test]
async fn backend_repair_network_text_json_login_proves_account_and_all_three_reads_in_one_runtime()
{
    let server = FixtureServer::new(vec![
        Reply::html(r#"{"success":true,"message":"fixture-accepted"}"#),
        Reply::html(HOME),
        Reply::html(HOME),
        Reply::html(HOME),
        user_page("network-b"),
        Reply::html(HOME),
        user_page("network-b"),
        Reply::html("<html><span class=\"glyphicon-exclamation-sign\">最多 5 台设备</span></html>"),
        Reply::html(HOME),
        Reply::html(HOME),
    ]);
    let mut runtime = pending_login(&server);
    runtime
        .complete_usereg_login("1234".into(), None)
        .await
        .unwrap();
    assert!(runtime.service_session_is_proven(ServiceId::Usereg));
    assert_eq!(
        runtime.load_usereg_account().await.unwrap().username,
        "network-b"
    );
    assert_eq!(
        runtime.load_usereg_balance().await.unwrap().account_balance,
        "8.10"
    );
    assert!(runtime.load_usereg_devices().await.unwrap().is_empty());
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    let requests = server.requests();
    assert_eq!(requests.len(), 10);
    assert_eq!(
        requests.iter().filter(|r| r.starts_with("POST ")).count(),
        2,
        "synthetic request methods: {:?}",
        requests
            .iter()
            .map(|r| r.lines().next().unwrap_or_default())
            .collect::<Vec<_>>()
    );
    assert!(requests[0].starts_with("POST /site/validate-user "));
    assert!(requests[1].starts_with("POST /login "));
    assert!(requests[2..].iter().all(|r| r.starts_with("GET ")));
}

#[tokio::test]
async fn backend_repair_network_usereg_start_obeys_shared_portal_failure_before_captcha() {
    let mut runtime = runtime();
    prove(&mut runtime, ServiceId::Identity, "primary-a");
    runtime.portal_failure = Some((std::time::Instant::now(), "portal_resume_network".into()));
    let before = crate::telemetry::request_count();
    let error = runtime
        .start_usereg_login("network-b".into(), "fixture-password".into())
        .await
        .unwrap_err();
    assert_eq!(error, "portal_resume_network");
    assert_eq!(crate::telemetry::request_count(), before);
    assert!(runtime.usereg_pending.is_none());
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert!(!runtime.service_session_is_proven(ServiceId::Usereg));
}

#[tokio::test]
async fn backend_repair_network_usereg_validation_retains_safe_root_cause_without_replaying() {
    for (reply, reason, category) in [
        (
            Reply {
                status: 200,
                headers: "Content-Type: image/png\r\n".into(),
                body: "fixture-private-response".into(),
            },
            "usereg_validation_content_type",
            "response",
        ),
        (
            Reply::json("fixture-private-invalid-json"),
            "usereg_validation_response_invalid",
            "response",
        ),
        (
            Reply::json(r#"{"message":"fixture-private-response"}"#),
            "usereg_validation_success_missing",
            "response",
        ),
        (
            Reply {
                status: 503,
                headers: String::new(),
                body: "fixture-private-response".into(),
            },
            "usereg_http_unavailable",
            "network",
        ),
    ] {
        let server = FixtureServer::new(vec![reply]);
        let mut runtime = pending_login(&server);
        let error = runtime
            .complete_usereg_login("1234".into(), None)
            .await
            .unwrap_err();
        assert_eq!(crate::telemetry::diagnostic_reason(&error), reason);
        assert_eq!(crate::live_validation::error_category(&error), category);
        assert!(!error.contains("fixture-private") && !error.contains("1234"));
        assert!(runtime.usereg_pending.is_none());
        assert!(
            runtime
                .complete_usereg_login("1234".into(), None)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
        assert!(server.requests()[0].starts_with("POST /site/validate-user "));
        assert!(runtime.service_session_is_proven(ServiceId::Identity));
    }
}

#[tokio::test]
async fn backend_repair_captcha_terminal_viewer_or_input_cancellation_clears_pending_without_post()
{
    use super::super::cli_validation::{
        CheckSpec, CheckStatus, UserPrompts, finish_usereg_interaction,
    };
    struct CancelPrompt {
        viewer_failure: bool,
        shown: usize,
        cleared: usize,
        inputs: usize,
    }
    impl UserPrompts for CancelPrompt {
        fn secret(&mut self, _: &'static str) -> Result<String, String> {
            self.inputs += 1;
            Err("user_cancelled".into())
        }
        fn confirm(&mut self, _: &'static str) -> Result<bool, String> {
            panic!("no automatic send or login")
        }
        fn choose_factor(&mut self, _: &[String]) -> Result<String, String> {
            panic!("no unrelated authentication")
        }
        fn show_captcha(&mut self, _: &[u8], _: &str) -> Result<(), String> {
            self.shown += 1;
            if self.viewer_failure {
                Err("captcha_view_declined".into())
            } else {
                Ok(())
            }
        }
        fn clear_captcha(&mut self) {
            self.cleared += 1;
        }
        fn progress(&mut self, _: &CheckSpec, _: CheckStatus, _: &str) {}
    }
    for viewer_failure in [true, false] {
        let server = FixtureServer::new(vec![]);
        let mut runtime = pending_login(&server);
        let mut prompt = CancelPrompt {
            viewer_failure,
            shown: 0,
            cleared: 0,
            inputs: 0,
        };
        let result = finish_usereg_interaction(
            &mut runtime,
            &mut prompt,
            UseregCaptchaDto {
                content_type: "image/png".into(),
                bytes: crate::reference_test_support::captcha_png(),
            },
        )
        .await;
        assert!(result.is_err());
        assert!(runtime.usereg_pending.is_none());
        assert!(runtime.service_session_is_proven(ServiceId::Identity));
        assert_eq!(prompt.shown, 1);
        assert_eq!(prompt.cleared, 1);
        assert_eq!(prompt.inputs, usize::from(!viewer_failure));
        assert!(server.requests().is_empty());
    }
}

#[tokio::test]
async fn backend_repair_captcha_rejection_consumes_image_without_replaying_validation() {
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"success":false,"message":"验证码错误"}"#,
    )]);
    let mut runtime = pending_login(&server);
    assert_eq!(runtime.usereg_login_phase(), "captcha_ready");
    assert_eq!(
        runtime
            .complete_usereg_login("1234".into(), None)
            .await
            .unwrap_err(),
        "网络自助验证码错误，请刷新后重试"
    );
    assert_eq!(runtime.usereg_login_phase(), "refresh_required");
    assert_eq!(
        runtime
            .complete_usereg_login("1234".into(), None)
            .await
            .unwrap_err(),
        "网络自助验证码已失效，请手动刷新图片后重试"
    );
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].starts_with("POST /site/validate-user "));
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert!(!runtime.service_session_is_proven(ServiceId::Usereg));
}

#[tokio::test]
async fn backend_repair_captcha_password_and_expiry_rejections_require_full_restart() {
    for (message, expected) in [
        ("用户名或密码错误", "网络自助用户名或密码错误，请检查后重试"),
        (
            "验证码已过期",
            "网络自助登录页面已过期，请重新输入账号密码获取验证码",
        ),
        (
            "verify code expired",
            "网络自助登录页面已过期，请重新输入账号密码获取验证码",
        ),
    ] {
        let server = FixtureServer::new(vec![Reply::json(&format!(
            r#"{{"success":false,"message":"{message}"}}"#
        ))]);
        let mut runtime = pending_login(&server);
        assert_eq!(
            runtime
                .complete_usereg_login("1234".into(), None)
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(runtime.usereg_login_phase(), "restart_required");
        assert!(runtime.usereg_pending.is_none());
        assert!(runtime.refresh_usereg_captcha().await.is_err());
        assert!(
            runtime
                .complete_usereg_login("1234".into(), None)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn backend_repair_captcha_cancel_clears_only_pending_credentials_and_preserves_proven_services() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = pending_login(&server);
    prove(&mut runtime, ServiceId::Library, "primary-a");
    runtime.library_adapter = Some(
        LibraryReadAdapter::try_with_transport(
            Url::parse(server.base()).unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    assert!(runtime.service_session_is_proven(ServiceId::Library));
    runtime.cancel_usereg_login();
    runtime.cancel_usereg_login();
    assert!(runtime.usereg_pending.is_none());
    assert_eq!(runtime.usereg_login_phase(), "restart_required");
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert!(runtime.service_session_is_proven(ServiceId::Library));
    let mut established = usereg_runtime(&server);
    established.cancel_usereg_login();
    assert_eq!(established.usereg_login_phase(), "restart_required");
    assert!(established.service_session_is_proven(ServiceId::Usereg));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_captcha_owner_cookie_lifetime_and_identity_proof_are_required_before_io() {
    for invalidation in ["owner", "cookie", "age", "identity", "service"] {
        let server = FixtureServer::new(vec![]);
        let mut runtime = pending_login(&server);
        match invalidation {
            "owner" => {
                runtime.usereg_pending.as_mut().unwrap().identity_owner = username("other-primary")
            }
            "cookie" => {
                runtime.usereg_pending.as_mut().unwrap().transport =
                    crate::transport::CampusHttpTransport::new("THYou/captcha-fixture").unwrap()
            }
            "age" => {
                runtime.usereg_pending.as_mut().unwrap().created_at =
                    std::time::Instant::now() - Duration::from_secs(301)
            }
            "identity" => {
                runtime.coordinator.logout(ServiceId::Identity).unwrap();
            }
            "service" => {
                runtime.coordinator.logout(ServiceId::Usereg).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(runtime.refresh_usereg_captcha().await.is_err());
        assert!(
            runtime
                .complete_usereg_login("1234".into(), None)
                .await
                .is_err()
        );
        assert!(runtime.usereg_pending.is_none());
        assert!(server.requests().is_empty(), "{invalidation}");
    }
}

#[tokio::test]
async fn backend_repair_captcha_failed_refresh_discards_old_image_and_pending_credentials() {
    let server = FixtureServer::new(vec![Reply::html("<html>login page</html>")]);
    let mut runtime = pending_login(&server);
    assert!(runtime.refresh_usereg_captcha().await.is_err());
    assert_eq!(runtime.usereg_login_phase(), "restart_required");
    assert!(runtime.usereg_pending.is_none());
    assert!(
        runtime
            .complete_usereg_login("1234".into(), None)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].starts_with("GET /site/captcha?refresh=1 "));
}

#[tokio::test]
async fn backend_repair_captcha_ambiguous_validation_or_final_post_cannot_be_replayed() {
    for final_post in [false, true] {
        let mut replies = vec![];
        if final_post {
            replies.push(Reply::json(r#"{"success":true}"#));
        }
        replies.push(Reply::html("<html>unconfirmed response</html>"));
        let server = FixtureServer::new(replies);
        let mut runtime = pending_login(&server);
        assert!(
            runtime
                .complete_usereg_login("1234".into(), None)
                .await
                .is_err()
        );
        assert_eq!(runtime.usereg_login_phase(), "restart_required");
        assert!(runtime.usereg_pending.is_none());
        assert!(
            runtime
                .complete_usereg_login("1234".into(), None)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), if final_post { 2 } else { 1 });
        assert!(!runtime.service_session_is_proven(ServiceId::Usereg));
    }
}

#[tokio::test]
async fn backend_repair_captcha_success_is_proven_independent_account_and_survives_dialog_cleanup()
{
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"success":true}"#),
        Reply::html(HOME),
        Reply::html(HOME),
        Reply::html(HOME),
        user_page("network-b"),
    ]);
    let mut runtime = pending_login(&server);
    runtime
        .complete_usereg_login("1234".into(), None)
        .await
        .unwrap();
    assert_eq!(runtime.usereg_login_phase(), "restart_required");
    runtime.cancel_usereg_login();
    assert!(runtime.service_session_is_proven(ServiceId::Usereg));
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert_eq!(
        runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Usereg)
            .user
            .unwrap()
            .username,
        "network-b"
    );
    assert_eq!(server.requests().len(), 5);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.starts_with("POST /login "))
            .count(),
        1
    );
    let status = format!("{:?}", runtime.status());
    for secret in [
        "synthetic-password",
        "fixture-form",
        "fixture-header",
        "1234",
    ] {
        assert!(!status.contains(secret));
    }
}

#[tokio::test]
async fn backend_repair_captcha_blank_code_is_local_and_does_not_consume_current_image() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = pending_login(&server);
    assert_eq!(
        runtime
            .complete_usereg_login("  ".into(), None)
            .await
            .unwrap_err(),
        "请输入网络自助验证码"
    );
    assert_eq!(runtime.usereg_login_phase(), "captcha_ready");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_captcha_active_primary_challenge_stops_before_password_post_and_drops_password()
 {
    let identity = FixtureServer::new(vec![Reply::html(
        r##"
        <div id="sm2publicKey">fixture-key</div>
        <form method="post" action="/do/off/ui/auth/login/check">
          <input name="i_user"><input name="i_pass" type="password">
          <div id="c_code" class="hidden"><input name="i_captcha"></div>
        </form><script>$("#c_code").removeClass('hidden');</script>"##,
    )]);
    let oauth = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!(
            "Location: {}do/off/ui/auth/login/form/fixture-app/0\r\n",
            identity.base()
        ),
        body: String::new(),
    }]);
    let portal = FixtureServer::new(vec![Reply {
        status: 302,
        headers: format!("Location: {}thu-oauth/auth\r\n", oauth.base()),
        body: String::new(),
    }]);
    let mut runtime = runtime();
    runtime.webvpn_identity_config =
        WebVpnIdentityConfig::new(portal.base(), oauth.base(), identity.base()).unwrap();
    let error = runtime
        .login(
            "fixture-primary".into(),
            "synthetic-password".into(),
            Some(true),
            true,
            false,
            false,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error,
        "统一认证需要图形验证码，App 暂未支持该验证流程，登录未完成"
    );
    assert!(runtime.primary_password.is_none());
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    assert!(runtime.session_recovery_retry_seconds().is_none());
    assert_eq!(identity.requests().len(), 1);
    assert_eq!(oauth.requests().len(), 1);
    assert_eq!(portal.requests().len(), 1);
    assert!(
        identity
            .requests()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}
