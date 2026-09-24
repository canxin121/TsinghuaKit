//! WebVPN-to-identity login bootstrap.
//!
//! The current campus deployment does not expose the identity login form at a
//! stable URL.  The WebVPN entry point first redirects to the OAuth broker and
//! the broker then redirects to an identity URL containing a generated
//! application id.  This module discovers that URL while retaining the
//! caller's [`CampusHttpTransport`] and therefore its cookie jar.
//!
//! This is deliberately a bootstrap helper.  It does not submit credentials,
//! perform SM2 encryption, or promote an identity session.  The caller can use
//! [`WebVpnIdentityBootstrap`] to construct the dynamic identity client
//! profile before starting the existing login flow.

use std::fmt;

use reqwest::{Method, StatusCode, Url, header::LOCATION};
use thiserror::Error;

use crate::{
    identity::{FormEncoding, LoginFormHiddenField},
    transport::{CampusHttpTransport, TransportError},
};

const CURRENT_WEBVPN_ORIGIN: &str = "https://webvpn.tsinghua.edu.cn/";
const CURRENT_OAUTH_ORIGIN: &str = "https://oauth.tsinghua.edu.cn/";
const CURRENT_IDENTITY_ORIGIN: &str = "https://id.tsinghua.edu.cn/";
const ENTRY_PATH: &str = "/login";
const OAUTH_AUTH_PATH: &str = "/thu-oauth/auth";
const IDENTITY_LOGIN_PREFIX: &str = "/do/off/ui/auth/login/form/";
const IDENTITY_SUBMIT_PATH: &str = "/do/off/ui/auth/login/check";
const IDENTITY_SINGLE_SUBMIT_PATH: &str = "/do/off/ui/auth/login/checkSingle";
const MAX_REDIRECT_HOPS: usize = 10;
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_APP_ID_LENGTH: usize = 128;
const MAX_PUBLIC_KEY_LENGTH: usize = 16 * 1024;
const MAX_HIDDEN_FIELD_NAME_LENGTH: usize = 128;
const MAX_HIDDEN_FIELD_VALUE_LENGTH: usize = 16 * 1024;

/// Deployment origins used by the current WebVPN login entry point.
#[derive(Clone, PartialEq, Eq)]
pub struct WebVpnIdentityConfig {
    webvpn_origin: Url,
    oauth_origin: Url,
    identity_origin: Url,
}

impl fmt::Debug for WebVpnIdentityConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebVpnIdentityConfig")
            .field("webvpn_origin", &origin_label(&self.webvpn_origin))
            .field("oauth_origin", &origin_label(&self.oauth_origin))
            .field("identity_origin", &origin_label(&self.identity_origin))
            .finish()
    }
}

impl WebVpnIdentityConfig {
    /// Creates a profile for the three explicitly trusted origins.
    ///
    /// HTTP is accepted for loopback fixture servers.  The production
    /// [`Self::current`] profile uses HTTPS for every origin.
    pub fn new(
        webvpn_origin: impl Into<String>,
        oauth_origin: impl Into<String>,
        identity_origin: impl Into<String>,
    ) -> Result<Self, WebVpnIdentityError> {
        let webvpn_origin = parse_origin(webvpn_origin.into(), "webvpn_origin")?;
        let oauth_origin = parse_origin(oauth_origin.into(), "oauth_origin")?;
        let identity_origin = parse_origin(identity_origin.into(), "identity_origin")?;
        if same_origin(&webvpn_origin, &oauth_origin)
            || same_origin(&webvpn_origin, &identity_origin)
            || same_origin(&oauth_origin, &identity_origin)
        {
            return Err(WebVpnIdentityError::InvalidConfig {
                field: "origins must be distinct",
            });
        }
        Ok(Self {
            webvpn_origin,
            oauth_origin,
            identity_origin,
        })
    }

    /// The currently deployed Tsinghua WebVPN/OAuth/identity origin set.
    pub fn current() -> Self {
        Self::new(
            CURRENT_WEBVPN_ORIGIN,
            CURRENT_OAUTH_ORIGIN,
            CURRENT_IDENTITY_ORIGIN,
        )
        .expect("current WebVPN identity origins are valid")
    }

    pub fn webvpn_origin(&self) -> &Url {
        &self.webvpn_origin
    }

    pub fn oauth_origin(&self) -> &Url {
        &self.oauth_origin
    }

    pub fn identity_origin(&self) -> &Url {
        &self.identity_origin
    }

    /// Builds the only initial URL accepted by this helper.
    pub fn entry_url(&self) -> Result<Url, WebVpnIdentityError> {
        let mut url = self.webvpn_origin.join(ENTRY_PATH).map_err(|_| {
            WebVpnIdentityError::InvalidConfig {
                field: "WebVPN entry path",
            }
        })?;
        url.set_query(Some("oauth_login=true"));
        Ok(url)
    }
}

impl Default for WebVpnIdentityConfig {
    fn default() -> Self {
        Self::current()
    }
}

/// A safe, cookie-aware discoverer for the dynamic identity login page.
pub struct WebVpnIdentityBootstrapper {
    config: WebVpnIdentityConfig,
    transport: CampusHttpTransport,
}

impl fmt::Debug for WebVpnIdentityBootstrapper {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebVpnIdentityBootstrapper")
            .field("config", &self.config)
            .field("transport", &"cookie-aware")
            .finish()
    }
}

impl WebVpnIdentityBootstrapper {
    pub fn new(config: WebVpnIdentityConfig, transport: CampusHttpTransport) -> Self {
        Self { config, transport }
    }

    pub fn config(&self) -> &WebVpnIdentityConfig {
        &self.config
    }

    /// Returns a clone of the transport handle.  The underlying cookie jar is
    /// shared, so callers can use the result for the subsequent identity POST.
    pub fn transport(&self) -> CampusHttpTransport {
        self.transport.clone()
    }

    /// Follows the WebVPN → OAuth broker → dynamic identity form bootstrap.
    ///
    /// `CampusHttpTransport` follows same-origin redirects and stops at a
    /// cross-origin redirect.  This method consumes that stopped response,
    /// validates the next origin and route against the explicit transition
    /// graph, then continues with the same reqwest client and cookie jar.
    pub async fn discover(&self) -> Result<WebVpnIdentityBootstrap, WebVpnIdentityError> {
        crate::telemetry::timing::phase(crate::telemetry::timing::Phase::AuthBootstrap, async {
            let mut next_url = self.config.entry_url()?;
            let mut redirects = 0_usize;

            loop {
                let response = self
                    .transport
                    .send(
                        self.transport
                            .client()
                            .request(Method::GET, next_url.clone()),
                    )
                    .await
                    .map_err(|error| {
                        WebVpnIdentityError::Transport(TransportError::Request(error))
                    })?;
                let response_url = response.url().clone();

                if response.status().is_redirection() {
                    if redirects >= MAX_REDIRECT_HOPS {
                        return Err(WebVpnIdentityError::RedirectChainTooLong);
                    }
                    let location = response
                        .headers()
                        .get(LOCATION)
                        .ok_or(WebVpnIdentityError::MissingRedirectLocation)?
                        .to_str()
                        .map_err(|_| WebVpnIdentityError::MalformedRedirect)?;
                    let target = response_url
                        .join(location)
                        .map_err(|_| WebVpnIdentityError::MalformedRedirect)?;
                    validate_redirect(&self.config, &response_url, &target)?;
                    // Consume the body before issuing the next request so this
                    // path behaves well even if a deployment includes a body in
                    // its redirect response.
                    let body = crate::telemetry::timing::read_text(response)
                        .await
                        .map_err(TransportError::Decode);
                    if body.is_err() {
                        return Err(WebVpnIdentityError::ResponseDecode);
                    }
                    redirects += 1;
                    next_url = target;
                    continue;
                }

                let status = response.status();
                let final_url = response_url;
                let body = crate::telemetry::timing::read_text(response)
                    .await
                    .map_err(|_| WebVpnIdentityError::ResponseDecode)?;
                if body.len() > MAX_HTML_BYTES {
                    return Err(WebVpnIdentityError::ResponseTooLarge);
                }
                if !status.is_success() {
                    return Err(WebVpnIdentityError::HttpStatus { status });
                }
                if !same_origin(&self.config.identity_origin, &final_url)
                    || !is_dynamic_identity_login_path(final_url.path())
                {
                    return Err(WebVpnIdentityError::UnexpectedFinalPage);
                }
                return parse_identity_login_page(&self.config, final_url, &body, redirects);
            }
        })
        .await
    }
}

/// The dynamic identity details discovered from the current WebVPN entry.
///
/// The URLs and the public key are intentionally private fields.  Accessors
/// are provided for the runtime integration, while `Debug` never prints the
/// query string, app id, public key, HTML, or cookie state.
#[derive(Clone, PartialEq, Eq)]
pub struct WebVpnIdentityBootstrap {
    app_id: String,
    login_page_url: Url,
    submit_url: Url,
    form_encoding: FormEncoding,
    sm2_public_key: String,
    hidden_fields: Vec<LoginFormHiddenField>,
    redirect_hops: usize,
}

impl fmt::Debug for WebVpnIdentityBootstrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebVpnIdentityBootstrap")
            .field("app_id", &"[redacted]")
            .field("login_page_url", &origin_label(&self.login_page_url))
            .field("submit_url", &origin_label(&self.submit_url))
            .field("form_encoding", &self.form_encoding)
            .field("sm2_public_key", &"[redacted]")
            .field("redirect_hops", &self.redirect_hops)
            .finish()
    }
}

impl WebVpnIdentityBootstrap {
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn login_page_url(&self) -> &Url {
        &self.login_page_url
    }

    pub fn submit_url(&self) -> &Url {
        &self.submit_url
    }

    /// The encoding declared by the same dynamic HTML form as `submit_url`.
    /// Missing `enctype` is represented as the browser default,
    /// `application/x-www-form-urlencoded`.
    pub fn form_encoding(&self) -> &FormEncoding {
        &self.form_encoding
    }

    /// The page-provided SM2 public key needed by the existing identity
    /// crypto boundary.  The caller must keep it in Rust and must not log it.
    pub fn sm2_public_key(&self) -> &str {
        &self.sm2_public_key
    }

    /// Hidden values from the same dynamic form as the public key.  They are
    /// consumed only by the Rust login request and never cross the bridge.
    pub fn hidden_fields(&self) -> &[LoginFormHiddenField] {
        &self.hidden_fields
    }

    pub fn redirect_hops(&self) -> usize {
        self.redirect_hops
    }

    /// The dynamic route without its query string, suitable for an identity
    /// profile that needs a page path rather than an absolute URL.
    pub fn login_page_path(&self) -> &str {
        self.login_page_url.path()
    }

    pub fn submit_path(&self) -> &str {
        self.submit_url.path()
    }
}

#[derive(Debug, Error)]
pub enum WebVpnIdentityError {
    #[error("invalid WebVPN identity configuration: {field}")]
    InvalidConfig { field: &'static str },
    #[error("WebVPN identity request failed")]
    Transport(#[source] TransportError),
    #[error("WebVPN identity response returned HTTP {status}")]
    HttpStatus { status: StatusCode },
    #[error("WebVPN identity redirect did not include a Location header")]
    MissingRedirectLocation,
    #[error("WebVPN identity redirect was malformed")]
    MalformedRedirect,
    #[error("WebVPN identity redirect chain exceeded the safe limit")]
    RedirectChainTooLong,
    #[error("WebVPN identity redirect used an untrusted origin or route")]
    UnexpectedRedirect,
    #[error("WebVPN identity response ended on an unexpected page")]
    UnexpectedFinalPage,
    #[error("WebVPN identity response could not be decoded")]
    ResponseDecode,
    #[error("WebVPN identity response was too large")]
    ResponseTooLarge,
    #[error("dynamic identity login form was not found")]
    LoginFormMissing,
    #[error("dynamic identity login form action was not allowed")]
    LoginFormActionInvalid,
    #[error("dynamic identity app id was not found")]
    AppIdMissing,
    #[error("dynamic identity app id did not agree across the login page")]
    AppIdMismatch,
    #[error("dynamic identity page did not contain an SM2 public key")]
    PublicKeyMissing,
    #[error("dynamic identity login form declared an unsupported encoding")]
    UnsupportedFormEncoding,
    #[error("identity login page requires an image captcha")]
    ImageCaptchaRequired,
}

fn parse_origin(value: String, field: &'static str) -> Result<Url, WebVpnIdentityError> {
    let url = Url::parse(&value).map_err(|_| WebVpnIdentityError::InvalidConfig { field })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "" && url.path() != "/")
    {
        return Err(WebVpnIdentityError::InvalidConfig { field });
    }
    let mut origin = url;
    origin.set_path("/");
    Ok(origin)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
        && right.username().is_empty()
        && right.password().is_none()
}

fn origin_slot(config: &WebVpnIdentityConfig, url: &Url) -> Option<u8> {
    if same_origin(&config.webvpn_origin, url) {
        Some(0)
    } else if same_origin(&config.oauth_origin, url) {
        Some(1)
    } else if same_origin(&config.identity_origin, url) {
        Some(2)
    } else {
        None
    }
}

fn validate_redirect(
    config: &WebVpnIdentityConfig,
    from: &Url,
    target: &Url,
) -> Result<(), WebVpnIdentityError> {
    if target.username() != ""
        || target.password().is_some()
        || target.fragment().is_some()
        || origin_slot(config, target).is_none()
    {
        return Err(WebVpnIdentityError::UnexpectedRedirect);
    }
    let from_slot = origin_slot(config, from).ok_or(WebVpnIdentityError::UnexpectedRedirect)?;
    let target_slot = origin_slot(config, target).ok_or(WebVpnIdentityError::UnexpectedRedirect)?;
    let route_allowed = match (from_slot, target_slot) {
        (0, 0) => target.path() == ENTRY_PATH,
        (0, 1) => target.path() == OAUTH_AUTH_PATH,
        (1, 1) => target.path() == OAUTH_AUTH_PATH,
        (1, 2) => is_dynamic_identity_login_path(target.path()),
        (2, 2) => is_dynamic_identity_login_path(target.path()),
        _ => false,
    };
    if route_allowed {
        Ok(())
    } else {
        Err(WebVpnIdentityError::UnexpectedRedirect)
    }
}

fn is_dynamic_identity_login_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(IDENTITY_LOGIN_PREFIX) else {
        return false;
    };
    let mut segments = rest.split('/');
    let Some(app_id) = segments.next() else {
        return false;
    };
    segments.next() == Some("0") && segments.next().is_none() && is_safe_app_id(app_id)
}

fn app_id_from_path(path: &str) -> Option<String> {
    if !is_dynamic_identity_login_path(path) {
        return None;
    }
    path.strip_prefix(IDENTITY_LOGIN_PREFIX)
        .and_then(|rest| rest.split('/').next())
        .map(str::to_owned)
}

fn is_safe_app_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_APP_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
}

fn parse_identity_login_page(
    config: &WebVpnIdentityConfig,
    login_page_url: Url,
    html: &str,
    redirect_hops: usize,
) -> Result<WebVpnIdentityBootstrap, WebVpnIdentityError> {
    let mut app_id =
        app_id_from_path(login_page_url.path()).ok_or(WebVpnIdentityError::AppIdMissing)?;
    if let Some(query_app_id) = query_value(&login_page_url, "appId") {
        if !is_safe_app_id(&query_app_id) {
            return Err(WebVpnIdentityError::AppIdMissing);
        }
        if query_app_id != app_id {
            return Err(WebVpnIdentityError::AppIdMismatch);
        }
    }

    let forms = open_tags(html, "form");
    let mut submit_url = None;
    let mut trusted_device_submit = false;
    let mut form_encoding = None;
    let mut hidden_fields = Vec::new();
    for form in forms {
        let Some(action) = attribute(&form.attributes, "action") else {
            continue;
        };
        if !form
            .attributes
            .iter()
            .find(|(name, _)| name == "method")
            .is_some_and(|(_, value)| value.eq_ignore_ascii_case("post"))
        {
            continue;
        }
        let action_url = login_page_url
            .join(&action)
            .map_err(|_| WebVpnIdentityError::LoginFormActionInvalid)?;
        if same_origin(&config.identity_origin, &action_url)
            && action_url.query().is_none()
            && action_url.fragment().is_none()
            && matches!(
                action_url.path(),
                IDENTITY_SUBMIT_PATH | IDENTITY_SINGLE_SUBMIT_PATH
            )
        {
            hidden_fields = hidden_fields_for_form(html, &form);
            trusted_device_submit = action_url.path() == IDENTITY_SINGLE_SUBMIT_PATH;
            form_encoding = Some(parse_form_encoding(attribute(&form.attributes, "enctype"))?);
            submit_url = Some(action_url);
            break;
        }
    }
    let submit_url = submit_url.ok_or(WebVpnIdentityError::LoginFormMissing)?;
    let form_encoding = form_encoding.ok_or(WebVpnIdentityError::LoginFormMissing)?;

    if let Some(page_app_id) = find_app_id_input(html) {
        if !is_safe_app_id(&page_app_id) {
            return Err(WebVpnIdentityError::AppIdMissing);
        }
        if page_app_id != app_id {
            return Err(WebVpnIdentityError::AppIdMismatch);
        }
    }

    let public_key = find_element_text_by_id(html, "sm2publicKey")
        .filter(|value| !value.is_empty() && value.len() <= MAX_PUBLIC_KEY_LENGTH)
        .filter(|value| !value.chars().any(char::is_control));
    let sm2_public_key = if trusted_device_submit {
        public_key.unwrap_or_default()
    } else {
        public_key.ok_or(WebVpnIdentityError::PublicKeyMissing)?
    };

    // Keep the binding explicit so a future change cannot accidentally return
    // a page from an untrusted origin after the parser has evolved.
    if !same_origin(&config.identity_origin, &login_page_url) {
        return Err(WebVpnIdentityError::UnexpectedFinalPage);
    }
    if has_active_identity_image_captcha(html) {
        // Reference thu-learn-lib stops at this exact challenge boundary.
        // There is no evidenced image URL/continuation in the references.
        // Do not consume a password POST with an empty i_captcha.
        return Err(WebVpnIdentityError::ImageCaptchaRequired);
    }

    // The path parser returns an owned value only after every route and token
    // check has succeeded.  This assignment makes that invariant obvious to
    // readers and avoids retaining any HTML-derived unvalidated string.
    app_id.shrink_to_fit();
    Ok(WebVpnIdentityBootstrap {
        app_id,
        login_page_url,
        submit_url,
        form_encoding,
        sm2_public_key,
        hidden_fields,
        redirect_hops,
    })
}

pub(crate) fn has_active_identity_image_captcha(html: &str) -> bool {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);
    let container = Selector::parse("#c_code").expect("constant selector");
    let field =
        Selector::parse("input[name='i_captcha']:not([type='hidden'])").expect("constant selector");
    let Some(code) = doc
        .select(&container)
        .find(|node| node.select(&field).next().is_some())
    else {
        return false;
    };
    let style = code
        .value()
        .attr("style")
        .unwrap_or_default()
        .split_whitespace()
        .collect::<String>()
        .to_ascii_lowercase();
    let hidden = code.value().attr("hidden").is_some()
        || code
            .value()
            .classes()
            .any(|class| class.eq_ignore_ascii_case("hidden"))
        || style.contains("display:none")
        || style.contains("visibility:hidden");
    if !hidden {
        return true;
    }
    // Match the known reference signal only in actual script elements, never
    // ordinary text/HTML examples or a hidden template field by itself.
    let scripts = Selector::parse("script").expect("constant selector");
    doc.select(&scripts).any(|script| {
        script
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<String>()
            .replace('\'', "\"")
            .contains("$(\"#c_code\").removeClass(\"hidden\")")
    })
}

fn parse_form_encoding(raw: Option<String>) -> Result<FormEncoding, WebVpnIdentityError> {
    match raw
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("application/x-www-form-urlencoded") => Ok(FormEncoding::UrlEncoded),
        Some("multipart/form-data") => Ok(FormEncoding::Multipart),
        Some(_) => Err(WebVpnIdentityError::UnsupportedFormEncoding),
    }
}

#[cfg(test)]
mod captcha_boundary_tests {
    use super::*;

    #[test]
    fn backend_repair_captcha_primary_hidden_template_is_not_an_active_challenge() {
        let hidden = "<div id='c_code' class='hidden'><input name='i_captcha'></div>";
        assert!(!has_active_identity_image_captcha(hidden));
        assert!(!has_active_identity_image_captcha(&format!(
            "{hidden}<p>$(\"#c_code\").removeClass('hidden');</p>"
        )));
        assert!(!has_active_identity_image_captcha(
            "<script>$(\"#c_code\").removeClass('hidden');</script>"
        ));
        for active in [
            "<div id='c_code'><input name='i_captcha'></div>".to_owned(),
            format!("{hidden}<script>$(\"#c_code\").removeClass('hidden');</script>"),
            format!("{hidden}<script>$( '#c_code' ).removeClass( \"hidden\" );</script>"),
        ] {
            assert!(has_active_identity_image_captcha(&active));
        }
    }

    #[test]
    fn backend_repair_captcha_primary_post_response_retains_typed_failure_without_raw_html() {
        use crate::identity::{
            IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthActions,
            SecondAuthProfile,
        };
        use crate::identity_client::{IdentityClient, IdentityClientConfig, LoginFailureReason};
        let client = IdentityClient::new(
            IdentityClientConfig::new(
                "https://id.example.test/",
                IdentityLoginProfile::new(
                    "fixture-app",
                    "/do/off/ui/auth/login/form/{appId}/0",
                    LoginFormProfile::new(
                        IDENTITY_SUBMIT_PATH,
                        FormEncoding::UrlEncoded,
                        LoginFormFields::common(),
                    ),
                    SecondAuthProfile::new(
                        "/b/doubleAuth/login",
                        "type",
                        "action",
                        vec![],
                        SecondAuthActions::new(None, None, None, None),
                    ),
                ),
            )
            .unwrap(),
        )
        .unwrap();
        let evidence = client.parse_login_page(
            "<div id='c_code'><input name='i_captcha' value='fixture-answer'></div>",
        );
        let failure = evidence.failure.unwrap();
        assert_eq!(failure.reason, LoginFailureReason::CaptchaRequired);
        assert!(failure.raw_value.is_none());
        assert!(!format!("{failure:?}").contains("fixture-answer"));
    }
}

#[derive(Debug, Clone)]
struct OpenTag {
    name: String,
    attributes: Vec<(String, String)>,
    open_end: usize,
}

fn open_tags(html: &str, wanted: &str) -> Vec<OpenTag> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = html[cursor..].find('<') {
        let start = cursor + relative;
        let Some(end) = find_tag_end(html, start + 1) else {
            break;
        };
        let inside = &html[start + 1..end];
        let trimmed = inside.trim_start();
        if !trimmed.starts_with('/') && !trimmed.starts_with('!') && !trimmed.starts_with('?') {
            let name_end = trimmed
                .find(|character: char| character.is_ascii_whitespace() || character == '/')
                .unwrap_or(trimmed.len());
            let name = trimmed[..name_end].to_ascii_lowercase();
            if name == wanted {
                result.push(OpenTag {
                    name,
                    attributes: parse_attributes(&trimmed[name_end..]),
                    open_end: end + 1,
                });
            }
        }
        cursor = end + 1;
    }
    result
}

fn parse_attributes(input: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let mut cursor = 0;
    let bytes = input.as_bytes();
    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b'/')
        {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        if name_start == cursor {
            cursor += 1;
            continue;
        }
        let name = input[name_start..cursor].to_ascii_lowercase();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let mut value = String::new();
        if cursor < bytes.len() && bytes[cursor] == b'=' {
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && (bytes[cursor] == b'\'' || bytes[cursor] == b'"') {
                let quote = bytes[cursor];
                cursor += 1;
                let value_start = cursor;
                while cursor < bytes.len() && bytes[cursor] != quote {
                    cursor += 1;
                }
                value = html_unescape(&input[value_start..cursor]);
                if cursor < bytes.len() {
                    cursor += 1;
                }
            } else {
                let value_start = cursor;
                while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                value = html_unescape(&input[value_start..cursor]);
            }
        }
        attributes.push((name, value));
    }
    attributes
}

fn attribute(attributes: &[(String, String)], wanted: &str) -> Option<String> {
    attributes
        .iter()
        .find(|(name, _)| name == wanted)
        .map(|(_, value)| value.clone())
}

fn find_app_id_input(html: &str) -> Option<String> {
    open_tags(html, "input").into_iter().find_map(|tag| {
        let id = attribute(&tag.attributes, "id");
        let name = attribute(&tag.attributes, "name");
        let matches = id
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("smrzappid"))
            || name
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("appid"));
        matches.then(|| attribute(&tag.attributes, "value"))?
    })
}

fn hidden_fields_for_form(html: &str, form: &OpenTag) -> Vec<LoginFormHiddenField> {
    let rest = &html[form.open_end..];
    let Some(end) = find_ascii_case_insensitive(rest, "</form") else {
        return Vec::new();
    };
    open_tags(&rest[..end], "input")
        .into_iter()
        .filter_map(|input| {
            let input_type = attribute(&input.attributes, "type")?;
            if !input_type.eq_ignore_ascii_case("hidden") {
                return None;
            }
            let name = attribute(&input.attributes, "name")?;
            let value = attribute(&input.attributes, "value")?;
            if name.is_empty()
                || name.len() > MAX_HIDDEN_FIELD_NAME_LENGTH
                || value.len() > MAX_HIDDEN_FIELD_VALUE_LENGTH
                || name.chars().any(char::is_control)
                || value.chars().any(char::is_control)
            {
                return None;
            }
            Some(LoginFormHiddenField::new(name, value))
        })
        .collect()
}

fn find_element_text_by_id(html: &str, wanted_id: &str) -> Option<String> {
    for tag_name in ["div", "textarea", "span", "input"] {
        for tag in open_tags(html, tag_name) {
            let id = attribute(&tag.attributes, "id");
            if !id
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(wanted_id))
            {
                continue;
            }
            if tag.name == "input" {
                return attribute(&tag.attributes, "value").map(|value| value.trim().to_owned());
            }
            let close = format!("</{}", tag.name);
            let rest = &html[tag.open_end..];
            let end = find_ascii_case_insensitive(rest, &close)?;
            let text = strip_tags(&rest[..end]);
            return Some(html_unescape(text.trim()));
        }
    }
    None
}

fn find_tag_end(html: &str, mut cursor: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut quote = None;
    while cursor < bytes.len() {
        match (quote, bytes[cursor]) {
            (Some(current), byte) if byte == current => quote = None,
            (None, b'\'' | b'"') => quote = Some(bytes[cursor]),
            (None, b'>') => return Some(cursor),
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn strip_tags(value: &str) -> &str {
    // The deployed public-key element contains plain text.  Retaining only
    // the substring before a nested tag keeps the parser conservative while
    // avoiding any dependency on an HTML DOM implementation.
    value.split('<').next().unwrap_or(value)
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn query_value(url: &Url, wanted: &str) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name == wanted)
        .map(|(_, value)| value.into_owned())
}

fn find_ascii_case_insensitive(value: &str, wanted: &str) -> Option<usize> {
    let wanted_lower = wanted.to_ascii_lowercase();
    value.char_indices().find_map(|(index, _)| {
        value[index..]
            .get(..wanted.len())
            .and_then(|part| part.eq_ignore_ascii_case(&wanted_lower).then_some(index))
    })
}

fn origin_label(url: &Url) -> String {
    let host = url.host_str().unwrap_or("<no-host>");
    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = stream.read(&mut buffer).expect("fixture request");
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn respond(stream: &mut std::net::TcpStream, status: &str, headers: &str, body: &str) {
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("fixture response");
    }

    #[test]
    fn current_profile_has_the_three_expected_origins() {
        let config = WebVpnIdentityConfig::current();
        assert_eq!(
            config.webvpn_origin().host_str(),
            Some("webvpn.tsinghua.edu.cn")
        );
        assert_eq!(
            config.oauth_origin().host_str(),
            Some("oauth.tsinghua.edu.cn")
        );
        assert_eq!(
            config.identity_origin().host_str(),
            Some("id.tsinghua.edu.cn")
        );
        assert_eq!(
            config.entry_url().expect("entry").query(),
            Some("oauth_login=true")
        );
    }

    #[test]
    fn redirect_policy_rejects_origin_confusion_and_route_confusion() {
        let config = WebVpnIdentityConfig::new(
            "https://webvpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .expect("config");
        let webvpn = Url::parse("https://webvpn.example.test/login").expect("url");
        let oauth = Url::parse("https://oauth.example.test/thu-oauth/auth").expect("url");
        let identity =
            Url::parse("https://id.example.test/do/off/ui/auth/login/form/fixture-app/0")
                .expect("url");
        let foreign =
            Url::parse("https://evil.example.test/do/off/ui/auth/login/form/a/0").expect("url");
        assert!(validate_redirect(&config, &webvpn, &oauth).is_ok());
        assert!(validate_redirect(&config, &oauth, &identity).is_ok());
        assert!(validate_redirect(&config, &webvpn, &foreign).is_err());
        assert!(validate_redirect(&config, &oauth, &webvpn).is_err());
        assert!(
            validate_redirect(
                &config,
                &identity,
                &Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("url")
            )
            .is_err()
        );
    }

    #[test]
    fn parser_requires_matching_dynamic_app_id_and_safe_form_action() {
        let config = WebVpnIdentityConfig::new(
            "https://webvpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .expect("config");
        let page = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app",
        )
        .expect("page");
        let html = r#"
            <form method="post" action="/do/off/ui/auth/login/check">
              <input type="hidden" id="SMRZAppid" value="fixture-app">
            </form>
            <div id="sm2publicKey">fixture-public-key</div>
        "#;
        let result = parse_identity_login_page(&config, page, html, 2).expect("bootstrap");
        assert_eq!(result.app_id(), "fixture-app");
        assert_eq!(
            result.login_page_path(),
            "/do/off/ui/auth/login/form/fixture-app/0"
        );
        assert_eq!(result.submit_path(), IDENTITY_SUBMIT_PATH);
        assert_eq!(result.sm2_public_key(), "fixture-public-key");
        assert!(!format!("{result:?}").contains("fixture-public-key"));
        assert!(!format!("{result:?}").contains("fixture-app"));

        let mismatched = r#"
            <form method="post" action="/do/off/ui/auth/login/check"></form>
            <input id="SMRZAppid" value="another-app">
            <div id="sm2publicKey">fixture-public-key</div>
        "#;
        assert!(matches!(
            parse_identity_login_page(
                &config,
                Url::parse("https://id.example.test/do/off/ui/auth/login/form/fixture-app/0")
                    .expect("page"),
                mismatched,
                1,
            ),
            Err(WebVpnIdentityError::AppIdMismatch)
        ));
    }

    #[test]
    fn parser_keeps_slashes_in_unquoted_dynamic_form_attributes() {
        let config = WebVpnIdentityConfig::new(
            "https://webvpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .expect("config");
        let page = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app",
        )
        .expect("page");
        let html = r#"
            <form method=post action=/do/off/ui/auth/login/check>
              <input type=hidden id=SMRZAppid value=fixture-app>
              <input type=hidden name=target value=/dynamic/target>
            </form>
            <div id=sm2publicKey>fixture-public-key</div>
        "#;

        let result = parse_identity_login_page(&config, page, html, 1).expect("bootstrap");
        assert_eq!(result.submit_path(), IDENTITY_SUBMIT_PATH);
        assert_eq!(
            result
                .hidden_fields()
                .iter()
                .find(|field| field.name() == "target")
                .map(|field| field.value()),
            Some("/dynamic/target")
        );
    }

    #[test]
    fn parser_accepts_check_single_trusted_device_form_without_sm2_key() {
        let config = WebVpnIdentityConfig::new(
            "https://webvpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .expect("config");
        let page = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app",
        )
        .expect("page");
        let html = r#"
            <form method="post" action="/do/off/ui/auth/login/checkSingle">
              <input type="hidden" name="target" value="TARGET_REDACTED">
              <input type="hidden" name="i_user" value="STALE_USER_REDACTED">
            </form>
        "#;

        let result = parse_identity_login_page(&config, page, html, 2).expect("bootstrap");
        assert_eq!(result.submit_path(), IDENTITY_SINGLE_SUBMIT_PATH);
        assert_eq!(result.sm2_public_key(), "");
        assert_eq!(
            result
                .hidden_fields()
                .iter()
                .map(|field| (field.name(), field.value()))
                .collect::<Vec<_>>(),
            vec![
                ("target", "TARGET_REDACTED"),
                ("i_user", "STALE_USER_REDACTED")
            ]
        );
        assert!(!format!("{result:?}").contains("TARGET_REDACTED"));

        let normal_without_key = r#"
            <form method="post" action="/do/off/ui/auth/login/check"></form>
        "#;
        assert!(matches!(
            parse_identity_login_page(
                &config,
                Url::parse("https://id.example.test/do/off/ui/auth/login/form/fixture-app/0")
                    .expect("page"),
                normal_without_key,
                1,
            ),
            Err(WebVpnIdentityError::PublicKeyMissing)
        ));
    }

    #[test]
    fn parser_keeps_hidden_inputs_from_the_real_login_form_only() {
        let config = WebVpnIdentityConfig::new(
            "https://webvpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .expect("config");
        let page = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app",
        )
        .expect("page");
        let html = r#"
            <form action="/language" method="post">
              <input type="hidden" name="foreignLanguage" value="FOREIGN_REDACTED">
            </form>
            <form method="post" action="/do/off/ui/auth/login/check">
              <input type="hidden" name="target" value="TARGET_REDACTED">
              <input type="hidden" name="SMRZAppid" value="fixture-app">
              <input type="hidden" name="SMRZSeq" value="SEQUENCE_REDACTED">
              <input type="hidden" name="i_user" value="OLD_USER_REDACTED">
              <input name="i_user" value="student-redacted">
            </form>
            <div id="sm2publicKey">fixture-public-key</div>
        "#;

        let result = parse_identity_login_page(&config, page, html, 2).expect("bootstrap");
        assert_eq!(
            result
                .hidden_fields()
                .iter()
                .map(|field| (field.name(), field.value()))
                .collect::<Vec<_>>(),
            vec![
                ("target", "TARGET_REDACTED"),
                ("SMRZAppid", "fixture-app"),
                ("SMRZSeq", "SEQUENCE_REDACTED"),
                ("i_user", "OLD_USER_REDACTED"),
            ]
        );
        let debug = format!("{result:?}");
        assert!(!debug.contains("TARGET_REDACTED"));
        assert!(!debug.contains("SEQUENCE_REDACTED"));
        assert!(!debug.contains("FOREIGN_REDACTED"));
    }

    #[tokio::test]
    async fn fixture_follows_cross_origin_chain_and_reuses_same_cookie_jar() {
        let webvpn_listener = TcpListener::bind("127.0.0.1:0").expect("webvpn listener");
        let oauth_listener = TcpListener::bind("127.0.0.1:0").expect("oauth listener");
        let identity_listener = TcpListener::bind("127.0.0.1:0").expect("identity listener");
        let webvpn_address = webvpn_listener.local_addr().expect("webvpn address");
        let oauth_address = oauth_listener.local_addr().expect("oauth address");
        let identity_address = identity_listener.local_addr().expect("identity address");

        let webvpn_server = thread::spawn(move || {
            let (mut stream, _) = webvpn_listener.accept().expect("webvpn request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /login?oauth_login=true HTTP/1.1"));
            respond(
                &mut stream,
                "302 Found",
                &format!(
                    "Location: http://{oauth_address}/thu-oauth/auth?state=fixture\r\nSet-Cookie: bootstrap=fixture; Domain=127.0.0.1; Path=/\r\n"
                ),
                "redirect",
            );
        });
        let oauth_server = thread::spawn(move || {
            let (mut stream, _) = oauth_listener.accept().expect("oauth request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /thu-oauth/auth?state=fixture HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("cookie: bootstrap=fixture")
            );
            respond(
                &mut stream,
                "302 Found",
                &format!(
                    "Location: http://{identity_address}/do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app\r\nSet-Cookie: broker=fixture; Domain=127.0.0.1; Path=/\r\n"
                ),
                "redirect",
            );
        });
        let identity_server = thread::spawn(move || {
            let (mut stream, _) = identity_listener.accept().expect("identity request");
            let request = read_request(&mut stream);
            assert!(request.starts_with(
                "GET /do/off/ui/auth/login/form/fixture-app/0?appId=fixture-app HTTP/1.1"
            ));
            let lower = request.to_ascii_lowercase();
            assert!(lower.contains("cookie:"));
            assert!(lower.contains("bootstrap=fixture"));
            assert!(lower.contains("broker=fixture"));
            respond(
                &mut stream,
                "200 OK",
                "Content-Type: text/html; charset=utf-8\r\n",
                r#"<html><form method="post" action="/do/off/ui/auth/login/check"><input type="hidden" id="SMRZAppid" value="fixture-app"></form><div id="sm2publicKey">fixture-public-key</div></html>"#,
            );
        });

        let config = WebVpnIdentityConfig::new(
            format!("http://{webvpn_address}/"),
            format!("http://{oauth_address}/"),
            format!("http://{identity_address}/"),
        )
        .expect("fixture config");
        let transport = CampusHttpTransport::new("THYou fixture").expect("transport");
        let bootstrapper = WebVpnIdentityBootstrapper::new(config, transport);
        let result = bootstrapper.discover().await.expect("bootstrap");
        assert_eq!(result.app_id(), "fixture-app");
        assert_eq!(result.redirect_hops(), 2);
        assert_eq!(result.login_page_url().query(), Some("appId=fixture-app"));
        assert_eq!(result.submit_path(), IDENTITY_SUBMIT_PATH);

        webvpn_server.join().expect("webvpn server");
        oauth_server.join().expect("oauth server");
        identity_server.join().expect("identity server");
    }
}
