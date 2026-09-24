//! Failure taxonomy checks based on synthetic responses, not real credentials.
use super::*;
use crate::identity_client::{LoginFailureReason, LoginResponseClassification};
use crate::identity_session::IdentitySessionError;

fn runtime() -> CampusRuntime {
    CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap()
}
fn failure(reason: LoginFailureReason) -> String {
    let mut runtime = runtime();
    runtime.primary_password = Some("fixture-password-not-real".into());
    let error = runtime.record_primary_auth_error(IdentitySessionError::LoginFailed { reason });
    assert!(runtime.primary_password.is_none());
    assert!(!error.contains("fixture-password-not-real"));
    assert!(!runtime.service_session_is_proven(ServiceId::Identity));
    error
}
fn classified(html: &str) -> LoginResponseClassification {
    let runtime = runtime();
    let profile = runtime
        .identity
        .identity()
        .client()
        .config()
        .profile
        .clone();
    let client = IdentityClient::new(
        IdentityClientConfig::new("https://id.fixture.invalid/", profile).unwrap(),
    )
    .unwrap();
    client.classify_login_response(
        reqwest::StatusCode::OK,
        &Url::parse("https://id.fixture.invalid/do/off/ui/auth/login/check").unwrap(),
        html,
    )
}

#[test]
fn backend_repair_login_diagnosis_generic_failure_does_not_accuse_credentials() {
    let error = failure(LoginFailureReason::Generic);
    assert!(!error.contains("密码错误"));
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "identity_login_rejected"
    );
}

#[test]
fn backend_repair_login_diagnosis_captcha_failure_keeps_its_own_category() {
    let error = failure(LoginFailureReason::CaptchaRequired);
    assert!(!error.contains("密码错误"));
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "image_captcha_required"
    );
}

#[test]
fn backend_repair_login_diagnosis_invalid_session_is_not_bad_password() {
    let error = failure(LoginFailureReason::SessionInvalid);
    assert!(!error.contains("密码错误"));
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "identity_session_invalid"
    );
}

#[test]
fn backend_repair_login_diagnosis_explicit_credential_rejection_remains_failure() {
    let error = failure(LoginFailureReason::InvalidCredentials);
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "credentials_rejected"
    );
    assert_eq!(error, "用户名或密码错误，请检查后重试");
}

#[test]
fn backend_repair_login_diagnosis_generic_error_field_is_not_credential_evidence() {
    for name in ["loginError", "errorMsg", "errorMessage"] {
        let html = format!(
            "<form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'><div id='{name}'>服务维护，请稍后再试</div></form>"
        );
        let LoginResponseClassification::LoginFailed { evidence, .. } = classified(&html) else {
            panic!("generic rejection must remain a rejection");
        };
        assert_eq!(evidence.reason, LoginFailureReason::Generic);
    }
}

#[test]
fn backend_repair_login_diagnosis_captcha_text_in_generic_field_stays_captcha() {
    let html = "<form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'><div id='errorMsg'>请输入验证码</div></form>";
    let LoginResponseClassification::LoginFailed { evidence, .. } = classified(html) else {
        panic!("captcha rejection must not be accepted");
    };
    assert_eq!(evidence.reason, LoginFailureReason::CaptchaRequired);
}

#[test]
fn backend_repair_login_diagnosis_false_error_flags_do_not_claim_credentials_rejected() {
    for value in ["false", "0", "no"] {
        let html = format!(
            "<form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'><input type='hidden' name='loginError' value='{value}'></form>"
        );
        assert!(
            matches!(classified(&html), LoginResponseClassification::LoginPage(_)),
            "negative error flags are neither failure evidence nor authentication proof"
        );
    }
}

#[test]
fn backend_repair_login_diagnosis_visible_credential_sentence_is_still_authoritative() {
    for html in [
        "<form><input name='i_user'><input name='i_pass' type='password'><div id='loginError'>用户名或密码错误</div></form>",
        "<form><input name='i_user'><input name='i_pass' type='password'><span id='msg_note'>您的用户名或密码不正确，请重试！</span></form>",
    ] {
        let LoginResponseClassification::LoginFailed { evidence, .. } = classified(html) else {
            panic!("explicit credentials failure must fail closed");
        };
        assert_eq!(evidence.reason, LoginFailureReason::InvalidCredentials);
    }
}

#[test]
fn backend_repair_login_diagnosis_hidden_messages_are_not_visible_rejections() {
    let html = "<form><input name='i_user'><input name='i_pass' type='password'><div hidden><span id='errorMsg'>用户名或密码错误</span></div></form>";
    assert!(matches!(
        classified(html),
        LoginResponseClassification::LoginPage(_)
    ));
}

#[test]
fn backend_repair_login_diagnosis_generic_flag_cannot_promote_stale_ticket() {
    let html = "<input type='hidden' name='loginError' value='true'><a href='/b/learn?ticket=STALE-SYNTHETIC'>continue</a>";
    let LoginResponseClassification::LoginFailed { evidence, .. } = classified(html) else {
        panic!("generic login failure still blocks stale handoff");
    };
    assert_eq!(evidence.reason, LoginFailureReason::Generic);
}

#[test]
fn backend_repair_login_diagnosis_real_failure_path_logs_fixed_evidence_not_response_text() {
    use crate::reference_test_support::{FixtureServer, Reply};
    use crate::telemetry::{LogConfig, LogSession};
    use sm2::elliptic_curve::sec1::ToSec1Point;
    let directory =
        std::env::temp_dir().join(format!("thyou-login-diagnostic-fixture-{}", Uuid::new_v4()));
    let mut logs =
        LogSession::start(&directory, LogConfig::parse("trace", false).unwrap()).unwrap();
    let key = sm2::SecretKey::from_slice(&[1u8; 32]).unwrap();
    let public_key: String = key.public_key().to_sec1_point(false).as_bytes()[1..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let server = FixtureServer::new(vec![Reply::html(
        "<form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'><div id='errorMsg'>请输入验证码 synthetic-user synthetic-response-secret</div></form>",
    )]);
    tracing::dispatcher::with_default(&logs.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut r = runtime();
                let profile = r.identity.identity().client().config().profile.clone();
                r.identity =
                    IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
                        IdentityClient::new(
                            IdentityClientConfig::new(server.base(), profile).unwrap(),
                        )
                        .unwrap(),
                        crate::transport::CampusHttpTransport::new(
                            "THYou/login-diagnostic-fixture",
                        )
                        .unwrap(),
                    ));
                let result = r
                    .identity
                    .establish_with_trusted_device_and_public_key(
                        &mut r.coordinator,
                        LoginFormInput::new(
                            "synthetic-user",
                            PasswordInput::plaintext("synthetic-password"),
                        ),
                        UserIdentity {
                            username: "synthetic-user".into(),
                            display_name: None,
                        },
                        None,
                        &public_key,
                    )
                    .await;
                assert!(matches!(
                    result,
                    Err(IdentitySessionError::LoginFailed {
                        reason: LoginFailureReason::CaptchaRequired
                    })
                ));
                assert!(!r.service_session_is_proven(ServiceId::Identity));
            });
    });
    logs.flush();
    let text = std::fs::read_to_string(logs.directory.join("events.000001.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let failure = events
        .iter()
        .find(|event| event["fields"]["event"] == "identity_failure_evidence")
        .unwrap();
    assert_eq!(failure["fields"]["reason"], "image_captcha_required");
    assert_eq!(failure["fields"]["evidence_source"], "field");
    assert_eq!(failure["fields"]["evidence_marker"], "error_msg");
    assert_eq!(failure["redacted_fields"], 0);
    assert!(
        events
            .iter()
            .any(|event| event["fields"]["event"] == "identity_submission_profile")
    );
    for secret in [
        "synthetic-user",
        "synthetic-password",
        "synthetic-response-secret",
        "请输入验证码",
    ] {
        assert!(!text.contains(secret));
    }
    assert_eq!(
        server.requests().len(),
        1,
        "one synthetic POST only; no login retry"
    );
    assert!(!server.requests()[0].contains("synthetic-password"));
    drop(logs);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn backend_repair_login_diagnosis_identity_only_plan_never_requests_tunet() {
    let selected = super::cli_validation::select_cases(&["identity_session".into()]).unwrap();
    assert_eq!(selected.len(), 1);
    assert!(selected.contains("identity_session"));
    assert!(!selected.contains("tunet_status"));
}
