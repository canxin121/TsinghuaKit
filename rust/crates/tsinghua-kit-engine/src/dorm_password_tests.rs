//! Loopback fixtures for the dormitory password reset slice.
//!
//! The route has no observed answer, so these tests pin down by hand what the
//! module promises: the postback echoes the page's own hidden state verbatim,
//! the old-password field is sent empty, the body is dispatched exactly once, a
//! login or expiry page never becomes a request carrying a password, and an
//! answer the service did not confirm is reported as unconfirmed rather than as
//! an error or a success.

use std::time::Duration;

use reqwest::Url;

use crate::dorm_password_write::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

/// The mapping root the runtime installs for the dormitory application.
const MAPPING: &str =
    "/http/77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9";

fn adapter(server: &FixtureServer) -> DormPasswordWriteAdapter {
    DormPasswordWriteAdapter::try_with_transport(
        mapped_url(server, ""),
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap()
}

fn mapped_url(server: &FixtureServer, suffix: &str) -> Url {
    let mut url = Url::parse(server.base()).unwrap();
    url.set_path(&format!("{MAPPING}{suffix}"));
    url
}

/// The reset form the service renders, with the hidden state a real page carries.
fn reset_form_body() -> String {
    r#"<html><head><title>修改密码</title></head><body>
         <form name="form1" method="post" action="ChangePassword.aspx" id="form1">
           <input type="hidden" name="__VIEWSTATE" id="__VIEWSTATE" value="/wEPDwUJNDY4Mjc1NjM5ZA==" />
           <input type="hidden" name="__EVENTVALIDATION" id="__EVENTVALIDATION" value="/wEWAgK6+7D8Bg==" />
           <input type="hidden" name="__VIEWSTATEGENERATOR" value="B7BD6FAA" />
           <input type="hidden" name="__PREVIOUSPAGE" value="x1y2z3" />
           <input type="hidden" name="" value="unnamed" />
           <input name="ChangePasswordCtrl1$txtoldpassword" type="password"
                  id="ChangePasswordCtrl1_txtoldpassword" />
         </form>
       </body></html>"#
        .to_owned()
}

/// A page that has no reset form on it at all.
fn other_page_body() -> &'static str {
    r#"<html><body><span id="SomeOtherCtrl1_lbl">宿舍电费</span></body></html>"#
}

/// A generic ASP.NET page: no doctype, no `<html>`, but a leading directive.
fn directive_page_body() -> String {
    format!("<%@ Page Language=\"C#\" %>\n{}", reset_form_body())
}

fn password(value: &str) -> DormPassword {
    DormPassword::new(value).unwrap()
}

#[test]
fn password_rejects_empty_whitespace_control_and_overlong_values() {
    assert_eq!(
        DormPassword::new("").unwrap_err(),
        DormPasswordRequestError::EmptySecret
    );
    assert_eq!(
        DormPassword::new("   ").unwrap_err(),
        DormPasswordRequestError::EmptySecret
    );
    assert_eq!(
        DormPassword::new("abc\u{7}def").unwrap_err(),
        DormPasswordRequestError::SecretInvalid
    );
    assert_eq!(
        DormPassword::new("x".repeat(MAX_DORM_PASSWORD_CHARS + 1)).unwrap_err(),
        DormPasswordRequestError::SecretTooLong {
            max: MAX_DORM_PASSWORD_CHARS
        }
    );
    // A space inside a password is a legitimate character, so the value is kept
    // exactly as typed rather than trimmed.
    let kept = password("a b");
    assert_eq!(kept.len(), 3);
}

#[test]
fn password_debug_and_plan_debug_never_print_the_value() {
    let secret = password("hunter2-unique");
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("hunter2"));
    assert!(rendered.contains("chars"));

    let plan = DormPasswordWriteProfile::new().reset_request(password("hunter2-unique"));
    let rendered = format!("{plan:?}");
    assert!(!rendered.contains("hunter2"));
    assert_eq!(
        plan.operation(),
        DormPasswordWriteOperation::ResetHomePassword
    );
    assert_eq!(plan.method(), DormPasswordWriteMethod::Post);
    assert_eq!(plan.path(), DORM_CHANGE_PASSWORD_PATH);
    assert!(plan.operation().is_write());
    assert_eq!(
        plan.session_prerequisite(),
        DormPasswordWriteSessionPrerequisite::ExistingElectricitySession
    );
    assert_eq!(
        plan.body_fields(),
        vec![
            "__EVENTTARGET",
            DORM_CHANGE_PASSWORD_OLD_FIELD,
            DORM_CHANGE_PASSWORD_NEW_FIELD,
            DORM_CHANGE_PASSWORD_CONFIRM_FIELD,
        ]
    );
}

#[test]
fn plan_fields_send_the_old_password_empty_and_repeat_the_new_one() {
    let plan = DormPasswordWriteProfile::new().reset_request(password("new-secret"));
    let fields: Vec<(String, String)> = plan
        .fields()
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
    assert_eq!(
        fields,
        vec![
            (
                "__EVENTTARGET".to_owned(),
                DORM_CHANGE_PASSWORD_EVENT_TARGET.to_owned()
            ),
            (DORM_CHANGE_PASSWORD_OLD_FIELD.to_owned(), String::new()),
            (
                DORM_CHANGE_PASSWORD_NEW_FIELD.to_owned(),
                "new-secret".to_owned()
            ),
            (
                DORM_CHANGE_PASSWORD_CONFIRM_FIELD.to_owned(),
                "new-secret".to_owned()
            ),
        ]
    );
    // The postback target uses this page's colon separator, not a `$`.
    assert_eq!(
        DORM_CHANGE_PASSWORD_EVENT_TARGET,
        "ChangePasswordCtrl1:btnOK"
    );
}

#[test]
fn form_parse_echoes_every_hidden_field_and_reads_through_a_page_directive() {
    let form = parse_change_password_form(&directive_page_body()).unwrap();
    // The anchor input is not hidden, so it is not echoed; the unnamed hidden
    // input addresses nothing and is left out as well.
    assert_eq!(
        form.field_names(),
        vec![
            "__VIEWSTATE",
            "__EVENTVALIDATION",
            "__VIEWSTATEGENERATOR",
            "__PREVIOUSPAGE",
        ]
    );
    assert_eq!(form.len(), 4);
    assert!(!form.is_empty());
    // The page's values are the service's own state and are echoed unchanged.
    assert!(
        form.fields()
            .iter()
            .any(|(name, value)| name == "__VIEWSTATE" && value == "/wEPDwUJNDY4Mjc1NjM5ZA==")
    );
    let rendered = format!("{form:?}");
    assert!(!rendered.contains("/wEPDwUJNDY4Mjc1NjM5ZA=="));
}

#[test]
fn form_parse_refuses_pages_that_are_not_the_reset_form() {
    assert_eq!(
        parse_change_password_form("").unwrap_err(),
        DormPasswordParseError::EmptyBody
    );
    assert_eq!(
        parse_change_password_form("   \n  ").unwrap_err(),
        DormPasswordParseError::EmptyBody
    );
    assert_eq!(
        parse_change_password_form(other_page_body()).unwrap_err(),
        DormPasswordParseError::FormMissing
    );
    assert_eq!(
        parse_change_password_form("{\"status\":1}").unwrap_err(),
        DormPasswordParseError::UnexpectedHtml
    );
    // A login page and a session-expiry page are never scanned for fields, so an
    // HTTP 200 login page cannot become an empty form.
    let login = r#"<html><body><div id="net_Default_LoginCtrl1_txtUserName"></div></body></html>"#;
    assert_eq!(
        parse_change_password_form(login).unwrap_err(),
        DormPasswordParseError::LoginPage
    );
    let expired = r#"<html><body><h2>用户登陆超时或访问内容不存在</h2></body></html>"#;
    assert_eq!(
        parse_change_password_form(expired).unwrap_err(),
        DormPasswordParseError::ExpiredPage
    );
}

#[test]
fn write_classifier_accepts_only_affirmative_evidence() {
    assert_eq!(
        classify_dorm_password_write(""),
        DormPasswordWriteOutcome::Accepted
    );
    assert_eq!(
        classify_dorm_password_write("  \r\n "),
        DormPasswordWriteOutcome::Accepted
    );
    assert_eq!(
        classify_dorm_password_write("OK"),
        DormPasswordWriteOutcome::Accepted
    );
    assert_eq!(
        classify_dorm_password_write(r#"{"status":1}"#),
        DormPasswordWriteOutcome::Accepted
    );
    assert_eq!(
        classify_dorm_password_write(r#"{"success":true}"#),
        DormPasswordWriteOutcome::Accepted
    );
    assert_eq!(
        classify_dorm_password_write(r#"{"result":"success"}"#),
        DormPasswordWriteOutcome::Accepted
    );
    // No refusal wording has been observed on this route, so an explicit failure
    // is reported as unreadable rather than invented as a refusal.
    assert_eq!(
        classify_dorm_password_write(r#"{"success":false,"message":"错误"}"#),
        DormPasswordWriteOutcome::Unrecognized
    );
    // A re-rendered form is the service still showing the page: it is not
    // acceptance evidence, and it is not read as a refusal either.
    assert_eq!(
        classify_dorm_password_write(&reset_form_body()),
        DormPasswordWriteOutcome::Unrecognized
    );
    assert_eq!(
        classify_dorm_password_write("some plain text"),
        DormPasswordWriteOutcome::Unrecognized
    );
}

#[tokio::test]
async fn reset_dispatches_one_postback_that_echoes_the_pages_hidden_state() {
    let server = FixtureServer::new(vec![Reply::html(&reset_form_body()), Reply::html("")]);
    let adapter = adapter(&server);
    let form = adapter.read_reset_form().await.unwrap();
    let plan = adapter.reset_request(password("brand-new-secret"));
    let outcome = adapter.reset_password(&plan, &form).await.unwrap();
    assert_eq!(outcome, DormPasswordWriteOutcome::Accepted);

    let requests = server.requests();
    assert_eq!(requests.len(), 2, "the reset must dispatch exactly once");
    let post = &requests[1];
    assert!(post.starts_with("POST "), "{post}");
    assert!(post.contains(DORM_CHANGE_PASSWORD_PATH), "{post}");
    assert!(
        post.to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded"),
        "{post}"
    );
    // The page's own hidden state is carried verbatim, urlencoded.
    assert!(
        post.contains("__VIEWSTATE=%2FwEPDwUJNDY4Mjc1NjM5ZA%3D%3D"),
        "{post}"
    );
    assert!(
        post.contains("__EVENTVALIDATION=%2FwEWAgK6%2B7D8Bg%3D%3D"),
        "{post}"
    );
    assert!(post.contains("__VIEWSTATEGENERATOR=B7BD6FAA"), "{post}");
    assert!(post.contains("__PREVIOUSPAGE=x1y2z3"), "{post}");
    // The event target keeps its colon separator.
    assert!(
        post.contains("__EVENTTARGET=ChangePasswordCtrl1%3AbtnOK"),
        "{post}"
    );
    // The old-password field is sent empty, and the new password is sent twice.
    assert!(post.contains("txtoldpassword=&"), "{post}");
    assert!(post.contains("txtnewpassword=brand-new-secret"), "{post}");
    assert!(post.contains("txtnewpassword1=brand-new-secret"), "{post}");
}

#[tokio::test]
async fn reset_reports_an_unreadable_answer_as_unconfirmed() {
    let server = FixtureServer::new(vec![
        Reply::html(&reset_form_body()),
        Reply::html("<html><body>some page</body></html>"),
    ]);
    let adapter = adapter(&server);
    let form = adapter.read_reset_form().await.unwrap();
    let plan = adapter.reset_request(password("brand-new-secret"));
    let outcome = adapter.reset_password(&plan, &form).await.unwrap();
    assert_eq!(outcome, DormPasswordWriteOutcome::Unrecognized);
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn reset_never_posts_when_the_session_is_gone() {
    // The form request itself answers with the WebVPN login page.
    let server = FixtureServer::new(vec![Reply::html(
        r#"<html><head><title>清华大学WebVPN</title></head><body></body></html>"#,
    )]);
    let adapter = adapter(&server);
    let error = adapter.read_reset_form().await.unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "dorm_write_session_expired");
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].starts_with("GET "));
}

#[tokio::test]
async fn reset_never_posts_when_the_page_lost_the_form() {
    let server = FixtureServer::new(vec![Reply::html(other_page_body())]);
    let adapter = adapter(&server);
    let error = adapter.read_reset_form().await.unwrap_err();
    assert_eq!(error.diagnostic_code(), "dorm_write_form_missing");
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn reset_reports_a_login_redirect_as_a_gone_session() {
    let server = FixtureServer::new(vec![
        Reply::html(&reset_form_body()),
        Reply {
            status: 302,
            headers: format!("Location: {MAPPING}/login\r\n"),
            body: String::new(),
        },
    ]);
    let adapter = adapter(&server);
    let form = adapter.read_reset_form().await.unwrap();
    let plan = adapter.reset_request(password("brand-new-secret"));
    let outcome = adapter.reset_password(&plan, &form).await.unwrap();
    assert_eq!(outcome, DormPasswordWriteOutcome::LoginRequired);
    // A login redirect is a definite answer: the change did not happen, and it is
    // still never re-dispatched inside this call.
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn reset_accepts_an_empty_form_and_reports_a_failed_status_as_unconfirmed() {
    let server = FixtureServer::new(vec![]);
    let adapter = adapter(&server);
    let plan = DormPasswordWriteProfile::new().reset_request(password("brand-new-secret"));
    assert_eq!(
        plan.operation(),
        DormPasswordWriteOperation::ResetHomePassword
    );
    let form = DormPasswordFormState::empty();
    // A form with no hidden fields is still a legal postback: the observed client
    // echoes whatever the page offers.
    let outcome = adapter.reset_password(&plan, &form).await.unwrap();
    // The fixture answers 500, which this service never uses for success, so the
    // effect is unknown rather than a refusal.
    assert_eq!(outcome, DormPasswordWriteOutcome::Unrecognized);
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        DormPasswordWriteOperation::ResetHomePassword.as_str(),
        "dorm_reset_home_password"
    );
    assert_eq!(
        DormPasswordWriteOperation::ResetHomePassword.to_string(),
        "dorm_reset_home_password"
    );
}

#[tokio::test]
async fn adapter_rejects_a_base_url_outside_the_mapping_shape() {
    for base in [
        "https://webvpn.tsinghua.edu.cn/http/token/?query=1",
        "https://webvpn.tsinghua.edu.cn/http/token/#fragment",
        "https://user:pass@webvpn.tsinghua.edu.cn/http/token/",
        "ftp://webvpn.tsinghua.edu.cn/http/token/",
    ] {
        assert!(
            DormPasswordWriteAdapterConfig::new(base).is_err(),
            "{base} must be refused"
        );
    }
    // A root carrying the read route is narrowed to the mapping root.
    let config = DormPasswordWriteAdapterConfig::new(&format!(
        "https://webvpn.tsinghua.edu.cn{MAPPING}{DORM_CHANGE_PASSWORD_PATH}"
    ))
    .unwrap();
    assert_eq!(config.base_url().path(), format!("{MAPPING}/"));
}
