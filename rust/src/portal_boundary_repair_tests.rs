//! The gateway and INFO use distinct Identity applications in one shared jar.
//! Fixtures reproduce the run-055042 route without storing real navigation data.
use super::*;
use crate::{
    identity::{
        FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthActions,
        SecondAuthProfile,
    },
    identity_client::{IdentityClient, IdentityClientConfig},
    reference_test_support::{FixtureServer, Reply},
    transport::CampusHttpTransport,
};
use std::net::TcpListener;

const TRUSTED_FORM: &str = "<form method='post' action='/do/off/ui/auth/login/checkSingle'></form>";

fn redirect(target: &str) -> Reply {
    Reply {
        status: 302,
        headers: format!("Location: {target}\r\n"),
        body: String::new(),
    }
}

fn success(target: &str) -> Reply {
    Reply::html(&format!(
        "<p>登录成功。正在重定向到</p><a href='{target}'>继续</a>"
    ))
}

struct Flow {
    vpn: FixtureServer,
    identity: FixtureServer,
    oauth: FixtureServer,
    execution: IdentityExecutionClient,
    config: WebVpnIdentityConfig,
}

impl Flow {
    fn new(gateway_app: &str, gateway_result: &str, info_result: &str) -> Self {
        Self::with_landing(
            gateway_app,
            gateway_result,
            info_result,
            "<html>gateway home</html>",
            Vec::new(),
        )
    }

    fn with_landing(
        gateway_app: &str,
        gateway_result: &str,
        info_result: &str,
        landing: &str,
        proof: Vec<Reply>,
    ) -> Self {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let identity = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", identity.local_addr().unwrap());
        let oauthu = format!("http://{}/", oauth.local_addr().unwrap());
        let config = WebVpnIdentityConfig::new(&vpnu, &oauthu, &idu).unwrap();
        let callback = format!("{oauthu}thu-oauth/callback?ticket=fixture-gateway");
        let gateway_reply = match gateway_result {
            "success" => success(&callback),
            "redirect" => redirect(&callback),
            "ambiguous" => Reply::html(&format!("<a href='{callback}'>继续</a>")),
            "form" => Reply::html(TRUSTED_FORM),
            "unavailable" => Reply {
                status: 503,
                headers: "Retry-After: 1\r\n".into(),
                body: String::new(),
            },
            _ => panic!("unknown synthetic outcome"),
        };
        let target = format!("{oauthu}lb-auth/lbredirect?ticket=fixture-info");
        let info_reply = match info_result {
            "success" => success(&target),
            "form" => Reply::html(TRUSTED_FORM),
            "second_factor" => Reply::html("<html>二次认证</html>"),
            _ => panic!("unknown synthetic outcome"),
        };
        let mut vpn_replies = vec![
            redirect(&format!("{oauthu}thu-oauth/auth?state=fixture")),
            redirect("/"),
            Reply::html(landing),
            Reply::html("<html>INFO landing</html>"),
        ];
        vpn_replies.extend(proof);
        let vpn = FixtureServer::from_listener(vpn, vpn_replies);
        let identity = FixtureServer::from_listener(
            identity,
            vec![
                Reply::html(TRUSTED_FORM),
                gateway_reply,
                Reply::html(TRUSTED_FORM),
                info_reply,
            ],
        );
        let oauth = FixtureServer::from_listener(
            oauth,
            vec![
                redirect(&format!("{idu}do/off/ui/auth/login/form/{gateway_app}/0")),
                redirect(&format!("{vpnu}login?ticket=fixture-callback")),
                redirect(&format!("{vpnu}https/fixturemap/f/info/index")),
            ],
        );
        let profile = IdentityLoginProfile::new(
            "fixture",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                Vec::new(),
                SecondAuthActions::new(None, None, None, None),
            ),
        );
        let execution = IdentityExecutionClient::with_transport(
            IdentityClient::new(IdentityClientConfig::new(&idu, profile).unwrap()).unwrap(),
            CampusHttpTransport::new("THYou/portal-boundary-fixture").unwrap(),
        );
        Self {
            vpn,
            identity,
            oauth,
            execution,
            config,
        }
    }

    async fn navigate(&self) -> Result<(), &'static str> {
        navigate(
            &self.execution,
            "fixture-fingerprint",
            &self.config,
            "/https/fixturemap/",
        )
        .await
    }

    fn posts(&self) -> usize {
        self.identity
            .requests()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count()
    }
}

#[tokio::test]
async fn backend_repair_portal_boundary_gateway_then_info_each_continue_once() {
    for response in ["success", "redirect"] {
        let flow = Flow::new("fixture-gateway-app", response, "success");
        assert_eq!(flow.navigate().await, Ok(()));
        assert_eq!(flow.posts(), 2);
        assert_eq!(flow.vpn.requests().len(), 4);
        assert_eq!(flow.oauth.requests().len(), 3);
        let requests = flow.identity.requests();
        assert_eq!(requests.len(), 4);
        assert!(requests[0].starts_with("GET /do/off/ui/auth/login/form/fixture-gateway-app/0 "));
        assert!(requests[2].starts_with(&format!("GET /do/off/ui/auth/login/form/{PORTAL_APP} ")));
        for request in requests.iter().filter(|r| r.starts_with("POST ")) {
            assert!(request.starts_with("POST /do/off/ui/auth/login/checkSingle "));
            assert!(!request.contains("i_pass"));
            assert!(!request.contains("i_user"));
        }
    }
}

#[tokio::test]
async fn backend_repair_portal_boundary_same_application_alias_does_not_replay() {
    let flow = Flow::new(PORTAL_APP, "success", "success");
    assert_eq!(
        flow.navigate().await,
        Err("portal_resume_trusted_unconfirmed")
    );
    assert_eq!(flow.posts(), 1);
    assert_eq!(flow.identity.requests().len(), 3);
    assert_eq!(flow.oauth.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_portal_boundary_unconfirmed_gateway_cannot_start_info_submission() {
    for result in ["ambiguous", "form", "unavailable"] {
        let flow = Flow::new("fixture-gateway-app", result, "success");
        assert!(flow.navigate().await.is_err());
        assert_eq!(flow.posts(), 1);
        assert_eq!(flow.identity.requests().len(), 2);
        assert!(flow.oauth.requests().len() <= 2);
    }
}

#[tokio::test]
async fn backend_repair_portal_boundary_second_stage_failure_does_not_repeat_either_post() {
    for result in ["form", "second_factor"] {
        let flow = Flow::new("fixture-gateway-app", "success", result);
        let reason = flow.navigate().await.unwrap_err();
        assert_eq!(
            reason,
            if result == "form" {
                "portal_resume_trusted_unconfirmed"
            } else {
                "portal_resume_second_factor_required"
            }
        );
        assert_eq!(flow.posts(), 2);
        assert_eq!(flow.identity.requests().len(), 4);
        assert_eq!(flow.oauth.requests().len(), 2);
        assert_eq!(flow.vpn.requests().len(), 3);
    }
}

#[test]
fn backend_repair_portal_boundary_application_key_is_path_scoped_not_query_scoped() {
    for path in [
        "/do/off/ui/auth/login/form/fixture",
        "/do/off/ui/auth/login/form/fixture/0",
    ] {
        let url = Url::parse(&format!("https://id.example.test{path}?ticket=fixture")).unwrap();
        assert_eq!(trusted_form_application(&url), Some("fixture"));
    }
    for path in [
        "/do/off/ui/auth/login/checkSingle",
        "/do/off/ui/auth/login/form/",
        "/do/off/ui/auth/login/form/fixture/1",
        "/do/off/ui/auth/login/form/fixture/0/extra",
        "/do/off/ui/auth/login/form/fi%78ture",
    ] {
        assert!(
            trusted_form_application(
                &Url::parse(&format!("https://id.example.test{path}")).unwrap()
            )
            .is_none()
        );
    }
}

#[tokio::test]
async fn backend_repair_portal_boundary_two_stages_still_require_csrf_and_account_proof() {
    use crate::{
        info_session::{InfoSessionAdapter, InfoSessionError, InfoWebVpnConfig},
        protocol::UserIdentity,
    };
    for mode in ["missing_csrf", "wrong_account", "verified"] {
        let proof = if mode == "missing_csrf" {
            vec![Reply::html("")]
        } else {
            vec![
                Reply::html("XSRF-TOKEN=fixture-csrf;"),
                Reply::json(if mode == "wrong_account" {
                    r#"{"result":"success","object":{"ryh":"fixture-other"}}"#
                } else {
                    r#"{"result":"success","object":{"ryh":"fixture-user"}}"#
                }),
            ]
        };
        let flow = Flow::with_landing(
            "fixture-gateway-app",
            "success",
            "success",
            "<html>gateway home</html>",
            proof,
        );
        flow.navigate().await.unwrap();
        let config = InfoWebVpnConfig::new(flow.vpn.base(), "/https/fixturemap/")
            .unwrap()
            .with_identity_origin(flow.identity.base())
            .unwrap();
        let adapter = InfoSessionAdapter::new(config, flow.execution.transport().clone()).unwrap();
        let result = adapter
            .probe_portal_account_with_handoff(
                &UserIdentity {
                    username: "fixture-user".into(),
                    display_name: None,
                },
                false,
            )
            .await;
        match mode {
            "missing_csrf" => assert!(matches!(result, Err(InfoSessionError::MissingCookieCsrf))),
            "wrong_account" => assert!(matches!(
                result,
                Err(InfoSessionError::PortalAccountMismatch)
            )),
            _ => assert_eq!(result.unwrap().as_str(), "fixture-csrf"),
        }
        assert_eq!(flow.posts(), 2);
        assert_eq!(
            flow.vpn.requests().len(),
            if mode == "missing_csrf" { 5 } else { 6 }
        );
    }
}

#[tokio::test]
async fn backend_repair_portal_boundary_gateway_challenge_cannot_advance_to_info() {
    for landing in [
        "<html>二次认证</html>",
        "<form method='post'><input name='i_user'><input name='i_pass' type='password'></form>",
    ] {
        let flow = Flow::with_landing(
            "fixture-gateway-app",
            "success",
            "success",
            landing,
            Vec::new(),
        );
        assert!(flow.navigate().await.is_err());
        assert_eq!(flow.posts(), 1);
        assert_eq!(flow.identity.requests().len(), 2);
        assert_eq!(flow.vpn.requests().len(), 3);
    }
}
