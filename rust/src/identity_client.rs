//! Planning and evidence extraction for the unified identity service.
//!
//! This module intentionally stops at the boundary between an identity
//! client and the HTTP transport.  It knows how to resolve the configured
//! routes, describe a form request, and retain small pieces of evidence from
//! a login page.  It does not keep credentials, send requests, or perform the
//! page-key-driven SM2 conversion.  The session layer performs that conversion
//! after fetching the login page and then hands this client a short-lived,
//! protocol-verified wire password.  The login form encoding remains an
//! explicit profile choice; this module never silently changes URL encoded
//! and multipart requests.

use std::fmt;

use reqwest::{Method, StatusCode, Url};
use thiserror::Error;

use crate::{
    identity::{
        AnchorTicket, FormEncoding, IdentityLoginProfile, InvalidationMarker, InvalidationStatus,
        LoginFormFields, LoginFormHiddenField, LoginPageParseResult, Sm2PublicKey,
        TrustedDeviceProfile,
    },
    protocol::CsrfToken,
};

const URL_ENCODED_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";
const MULTIPART_CONTENT_TYPE: &str = "multipart/form-data";
const APP_ID_PLACEHOLDER: &str = "{appId}";
const IDENTITY_CALLBACK_PATH: &str = "/do/off/ui/auth/login/redirect2Jsp";
// The current public Learn client uses this route as the primary trusted-device
// submit action when the login page already exposes a trusted device.
const IDENTITY_SINGLE_LOGIN_PATH: &str = "/do/off/ui/auth/login/checkSingle";
const CHECK_SINGLE_REMEMBER_FIELD: &str = "i_rememberme";
const CHECK_SINGLE_REMEMBER_VALUE: &str = "on";
const MAX_ANCHOR_TICKET_LENGTH: usize = 4096;
const DEFAULT_ANCHOR_TICKET_PATH_PREFIXES: &[&str] = &[
    "/b/",
    "/f/",
    // The current identity service also exposes a same-origin callback page
    // before the final service handoff.  Recent public clients follow this
    // route (and its redirects) instead of assuming that the ticket is
    // already on a /b/ or /f/ link.
    IDENTITY_CALLBACK_PATH,
];
const WEBVPN_WRAPPER_PREFIXES: &[&str] = &["https", "http", "https-443", "http-80"];
const WEBVPN_LEARN_HANDOFF_TARGETS: &[&str] = &[
    "b/j_spring_security_thauth_roaming_entry",
    "f/j_spring_security_thauth_roaming_entry",
];
/// The identity callback can be returned through the WebVPN wrapper after
/// second-factor verification.  This is a continuation route, never a source
/// of a service ticket; the downstream service still has to prove its own
/// session before the runtime exposes data.
const WEBVPN_IDENTITY_HANDOFF_TARGETS: &[&str] = &[
    "do/off/ui/auth/login/redirect2Jsp",
    "do/off/ui/auth/login/check",
    // A trusted-device or already verified second-factor flow can use the
    // single-login continuation instead of redirect2Jsp.  It is a
    // continuation only; it must never become a service-ticket source.
    "do/off/ui/auth/login/checkSingle",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct CookieBackedHandoffRule {
    origin: Url,
    path_prefixes: Vec<String>,
}

/// Errors raised while validating a profile or creating a request plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityClientError {
    #[error("invalid identity client configuration: {message}")]
    InvalidConfig { message: String },

    #[error("invalid identity URL {url:?}: {message}")]
    InvalidUrl { url: String, message: String },

    #[error("invalid login input for {field}: {message}")]
    InvalidInput {
        field: &'static str,
        message: String,
    },

    #[error("unsupported password cryptography for {algorithm}: {reason}")]
    UnsupportedCrypto { algorithm: String, reason: String },

    #[error("the configured login form encoding {encoding:?} is not supported")]
    UnsupportedFormEncoding { encoding: String },

    #[error("multipart login is available only as an explicit request plan")]
    MultipartRequestPlanOnly,

    #[error("trusted-device saveFinger is not configured for this identity profile")]
    UnsupportedTrustedDevice,

    #[error("multipart boundary is invalid")]
    InvalidMultipartBoundary,
}

/// A field marker whose presence in a page can identify a failed login.
///
/// An empty `accepted_values` list means that the marker itself is enough,
/// including a boolean or empty HTML attribute.  Non-empty values are matched
/// case-insensitively after surrounding whitespace is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFailureMarker {
    pub field_name: String,
    pub accepted_values: Vec<String>,
    pub reason: LoginFailureReason,
}

impl LoginFailureMarker {
    pub fn any(field_name: impl Into<String>, reason: LoginFailureReason) -> Self {
        Self {
            field_name: field_name.into(),
            accepted_values: Vec::new(),
            reason,
        }
    }

    pub fn exact(
        field_name: impl Into<String>,
        accepted_values: impl IntoIterator<Item = impl Into<String>>,
        reason: LoginFailureReason,
    ) -> Self {
        Self {
            field_name: field_name.into(),
            accepted_values: accepted_values.into_iter().map(Into::into).collect(),
            reason,
        }
    }

    fn matches(&self, value: Option<&str>) -> bool {
        if self.accepted_values.is_empty() {
            return true;
        }

        let Some(value) = value else {
            return false;
        };
        self.accepted_values
            .iter()
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(value.trim()))
    }
}

/// A visible text fragment that is sufficiently specific to classify a page
/// as a failed login page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFailureTextMarker {
    pub text: String,
    pub reason: LoginFailureReason,
}

impl LoginFailureTextMarker {
    pub fn new(text: impl Into<String>, reason: LoginFailureReason) -> Self {
        Self {
            text: text.into(),
            reason,
        }
    }
}

/// The reason a response was classified as a failed login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFailureReason {
    InvalidCredentials,
    CaptchaRequired,
    SessionInvalid,
    Generic,
}

/// How a failure was evidenced in the returned HTML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureEvidenceSource {
    Field,
    Text,
    InvalidationMarker,
}

/// A compact failure record.  The parser never stores the complete response
/// body, and it only copies the value of a field explicitly configured as a
/// marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFailureEvidence {
    pub reason: LoginFailureReason,
    pub source: FailureEvidenceSource,
    pub marker: String,
    pub raw_value: Option<String>,
}

impl LoginFailureEvidence {
    /// The failure text may contain echoed account data. Emit only a closed
    /// reason/source/marker classification, never the marker value or body.
    pub(crate) fn trace_safe_diagnostic(&self) {
        let reason = match self.reason {
            LoginFailureReason::InvalidCredentials => "credentials_rejected",
            LoginFailureReason::CaptchaRequired => "image_captcha_required",
            LoginFailureReason::SessionInvalid => "identity_session_invalid",
            LoginFailureReason::Generic => "identity_login_rejected",
        };
        let evidence_source = match self.source {
            FailureEvidenceSource::Field => "field",
            FailureEvidenceSource::Text => "visible_text",
            FailureEvidenceSource::InvalidationMarker => "invalidation_marker",
        };
        let evidence_marker = match self.source {
            FailureEvidenceSource::Field => match self.marker.as_str() {
                "loginError" => "login_error",
                "errorMsg" => "error_msg",
                "errorMessage" => "error_message",
                "captchaError" => "captcha_error",
                "loginInvalid" => "login_invalid",
                _ => "configured_field",
            },
            FailureEvidenceSource::InvalidationMarker => "invalidation_marker",
            FailureEvidenceSource::Text => match self.reason {
                LoginFailureReason::InvalidCredentials => "credentials_text",
                LoginFailureReason::CaptchaRequired => "captcha_text",
                LoginFailureReason::SessionInvalid => "session_text",
                LoginFailureReason::Generic => "generic_text",
            },
        };
        tracing::warn!(target:"tsinghua_kit::auth",event="identity_failure_evidence",service="identity",reason,evidence_source,evidence_marker);
    }
}

/// Evidence extracted from a login or authentication-interstitial page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPageEvidence {
    pub sm2_public_key: Option<Sm2PublicKey>,
    pub anchor_ticket: Option<AnchorTicket>,
    pub csrf: Option<CsrfEvidence>,
    pub invalidation: InvalidationMarker,
    pub failure: Option<LoginFailureEvidence>,
    pub has_login_form: bool,
    pub second_factor_marker: Option<String>,
}

impl LoginPageEvidence {
    pub fn as_protocol_result(&self) -> LoginPageParseResult {
        LoginPageParseResult {
            sm2_public_key: self.sm2_public_key.clone(),
            anchor_ticket: self.anchor_ticket.clone(),
            invalidation: self.invalidation.clone(),
        }
    }
}

/// A CSRF token together with the HTML field that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfEvidence {
    pub field_name: String,
    pub token: CsrfToken,
}

impl CsrfEvidence {
    pub fn as_str(&self) -> &str {
        self.token.as_str()
    }
}

/// The result of classifying one response after the transport has followed
/// any configured same-origin redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginResponseClassification {
    /// A non-success response is kept separate from an HTML login failure.
    /// The body is deliberately not retained here because transport owns it.
    HttpStatus {
        status: StatusCode,
    },
    /// The response was received from a different origin than the configured
    /// identity service.  Its body is never considered login evidence.
    UnexpectedOrigin,
    /// A successful page explicitly indicates that the credentials or session were
    /// rejected.  HTTP success alone is therefore not treated as login
    /// success.
    LoginFailed {
        page: LoginPageEvidence,
        evidence: LoginFailureEvidence,
    },
    RequiresSecondFactor {
        page: LoginPageEvidence,
        marker: String,
    },
    /// A redirect location itself contained a validated service handoff
    /// ticket.  The transport stopped before following it because the next
    /// origin was outside the identity origin.
    AuthenticatedHandoff(LoginPageEvidence),
    /// The response points at the identity service's intermediate callback.
    /// The session layer must GET this callback before it can extract the
    /// final handoff ticket.
    RedirectCallback(LoginPageEvidence),
    /// The response was already fetched from an allowlisted service handoff
    /// route that carries authentication in the shared Cookie jar.  There is
    /// no ticket to extract from this shape; the downstream service must
    /// still prove its own session (for example with Learn CSRF evidence).
    CookieBackedHandoff(LoginPageEvidence),
    LoginPage(LoginPageEvidence),
    /// No reliable login result was found.  In particular, this variant does
    /// not claim that an arbitrary 200 page proves authentication succeeded.
    OtherPage(LoginPageEvidence),
}

/// Configuration for one deployment-specific unified identity entry point.
///
/// The profile contains wire field names and route templates, while this
/// type contains the host and the small set of HTML evidence rules needed to
/// interpret that deployment.  It contains no credential values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityClientConfig {
    pub base_url: Url,
    pub profile: IdentityLoginProfile,
    pub csrf_field_names: Vec<String>,
    pub sm2_public_key_field_names: Vec<String>,
    pub anchor_ticket_query_names: Vec<String>,
    /// Explicit origins from which an authenticated service handoff may be
    /// accepted.  The configured identity origin is always included; other
    /// origins, such as Learn, must be added by the deployment profile.
    pub anchor_ticket_origins: Vec<Url>,
    /// Absolute path prefixes from which an identity handoff ticket may be
    /// accepted.  The default covers the observed `/b/` and `/f/` handoff
    /// route families while keeping unrelated same-origin links out.
    pub anchor_ticket_path_prefixes: Vec<String>,
    /// Additional origins which may complete an authenticated handoff through
    /// the shared Cookie jar without carrying a service ticket.  This is kept
    /// separate from `anchor_ticket_origins`: a WebVPN shell can establish a
    /// Cookie session, but it must never become an accepted source of opaque
    /// service tickets.
    pub cookie_backed_handoff_origins: Vec<Url>,
    /// Path prefixes for the additional Cookie-backed handoff origins.
    pub cookie_backed_handoff_path_prefixes: Vec<String>,
    pub invalidation_field_names: Vec<String>,
    pub failure_markers: Vec<LoginFailureMarker>,
    pub failure_text_markers: Vec<LoginFailureTextMarker>,
    pub second_factor_field_names: Vec<String>,
    pub second_factor_text_markers: Vec<String>,
    cookie_backed_handoff_rules: Vec<CookieBackedHandoffRule>,
}

impl IdentityClientConfig {
    pub fn new(
        base_url: impl Into<String>,
        profile: IdentityLoginProfile,
    ) -> Result<Self, IdentityClientError> {
        let base_url_text = base_url.into();
        let base_url =
            Url::parse(&base_url_text).map_err(|error| IdentityClientError::InvalidUrl {
                url: base_url_text.clone(),
                message: error.to_string(),
            })?;

        let config = Self {
            anchor_ticket_origins: vec![base_url.clone()],
            base_url,
            profile,
            csrf_field_names: default_names(["_csrf", "csrf", "csrfToken", "_csrf_token"]),
            sm2_public_key_field_names: default_names(["sm2publicKey"]),
            anchor_ticket_query_names: default_names(["ticket"]),
            anchor_ticket_path_prefixes: DEFAULT_ANCHOR_TICKET_PATH_PREFIXES
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            cookie_backed_handoff_origins: Vec::new(),
            cookie_backed_handoff_path_prefixes: Vec::new(),
            invalidation_field_names: default_names([
                "loginInvalid",
                "invalidSession",
                "sessionInvalid",
                "sessionExpired",
            ]),
            failure_markers: vec![
                // These containers can carry captcha, session or service
                // failures, not just a credential rejection. The field's
                // validated content determines a more specific cause below.
                LoginFailureMarker::any("loginError", LoginFailureReason::Generic),
                LoginFailureMarker::any("errorMsg", LoginFailureReason::Generic),
                LoginFailureMarker::any("errorMessage", LoginFailureReason::Generic),
                LoginFailureMarker::any("captchaError", LoginFailureReason::CaptchaRequired),
                LoginFailureMarker::exact(
                    "loginInvalid",
                    ["true", "1", "yes", "invalid", "expired"],
                    LoginFailureReason::SessionInvalid,
                ),
            ],
            failure_text_markers: vec![
                LoginFailureTextMarker::new(
                    "用户名或密码错误",
                    LoginFailureReason::InvalidCredentials,
                ),
                LoginFailureTextMarker::new(
                    "您的用户名或密码不正确",
                    LoginFailureReason::InvalidCredentials,
                ),
                LoginFailureTextMarker::new(
                    "用户名或密码不正确",
                    LoginFailureReason::InvalidCredentials,
                ),
                LoginFailureTextMarker::new(
                    "账号或密码错误",
                    LoginFailureReason::InvalidCredentials,
                ),
                LoginFailureTextMarker::new(
                    "invalid credentials",
                    LoginFailureReason::InvalidCredentials,
                ),
                LoginFailureTextMarker::new(
                    "invalid username or password",
                    LoginFailureReason::InvalidCredentials,
                ),
                LoginFailureTextMarker::new("验证码错误", LoginFailureReason::CaptchaRequired),
                LoginFailureTextMarker::new(
                    "您输入的验证码不正确",
                    LoginFailureReason::CaptchaRequired,
                ),
                LoginFailureTextMarker::new("请输入验证码", LoginFailureReason::CaptchaRequired),
                LoginFailureTextMarker::new(
                    "captcha required",
                    LoginFailureReason::CaptchaRequired,
                ),
                LoginFailureTextMarker::new("登录失败", LoginFailureReason::Generic),
                LoginFailureTextMarker::new("login failed", LoginFailureReason::Generic),
            ],
            second_factor_field_names: default_names([
                "doubleAuth",
                "secondAuth",
                "requiresSecondFactor",
                "needSecondAuth",
            ]),
            second_factor_text_markers: vec![
                String::from("二次认证"),
                String::from("二次验证"),
                String::from("double authentication"),
            ],
            cookie_backed_handoff_rules: Vec::new(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Replaces the deployment-specific set of absolute handoff path
    /// prefixes.  A prefix ending in `/` accepts descendants under that
    /// route; every configured prefix must remain on the identity origin.
    pub fn with_anchor_ticket_path_prefixes(
        mut self,
        prefixes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, IdentityClientError> {
        self.anchor_ticket_path_prefixes = prefixes.into_iter().map(Into::into).collect();
        self.validate()?;
        Ok(self)
    }

    /// Adds explicit origins that may receive the one-time service handoff
    /// ticket.  This keeps a Learn handoff possible while rejecting arbitrary
    /// links embedded in a login page.
    pub fn with_anchor_ticket_origins(
        mut self,
        origins: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, IdentityClientError> {
        self.anchor_ticket_origins = vec![origin_url(&self.base_url)];
        for origin in origins {
            let value = origin.into();
            let parsed = Url::parse(&value).map_err(|error| IdentityClientError::InvalidUrl {
                url: value.clone(),
                message: error.to_string(),
            })?;
            self.anchor_ticket_origins.push(origin_url(&parsed));
        }
        self.anchor_ticket_origins
            .sort_by(|left, right| left.as_str().cmp(right.as_str()));
        self.anchor_ticket_origins
            .dedup_by(|left, right| left == right);
        self.validate()?;
        Ok(self)
    }

    /// Replaces the additional origins which may complete a ticketless
    /// Cookie-backed handoff.  These origins are never accepted as ticket
    /// sources by [`Self::find_anchor_ticket_url`].
    pub fn with_cookie_backed_handoff_origins(
        mut self,
        origins: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, IdentityClientError> {
        self.cookie_backed_handoff_rules.clear();
        self.cookie_backed_handoff_origins.clear();
        for origin in origins {
            let value = origin.into();
            let parsed = Url::parse(&value).map_err(|error| IdentityClientError::InvalidUrl {
                url: value.clone(),
                message: error.to_string(),
            })?;
            self.cookie_backed_handoff_origins.push(origin_url(&parsed));
        }
        self.cookie_backed_handoff_origins
            .sort_by(|left, right| left.as_str().cmp(right.as_str()));
        self.cookie_backed_handoff_origins.dedup();
        self.validate()?;
        Ok(self)
    }

    /// Replaces the path prefixes accepted for the additional Cookie-backed
    /// handoff origins.  A prefix of `/` is intentionally explicit: it is
    /// suitable for a WebVPN root handoff only when paired with that exact
    /// allowlisted origin.
    pub fn with_cookie_backed_handoff_path_prefixes(
        mut self,
        prefixes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, IdentityClientError> {
        self.cookie_backed_handoff_rules.clear();
        self.cookie_backed_handoff_path_prefixes = prefixes.into_iter().map(Into::into).collect();
        self.validate()?;
        Ok(self)
    }

    /// Adds one origin-specific Cookie-backed handoff route.  This is used
    /// when several trusted origins have different route namespaces, such as
    /// the WebVPN shell and the OAuth `/thu-oauth/` or `/lb-auth/` callbacks.
    /// A route never becomes a source of service tickets by itself; only the
    /// explicit WebVPN wrapper parser below may consume a ticket.
    pub fn with_cookie_backed_handoff_route(
        mut self,
        origin: impl Into<String>,
        prefixes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, IdentityClientError> {
        if self.cookie_backed_handoff_rules.is_empty() {
            self.cookie_backed_handoff_origins.clear();
            self.cookie_backed_handoff_path_prefixes.clear();
        }
        let value = origin.into();
        let parsed = Url::parse(&value).map_err(|error| IdentityClientError::InvalidUrl {
            url: value.clone(),
            message: error.to_string(),
        })?;
        let origin = origin_url(&parsed);
        let path_prefixes = prefixes.into_iter().map(Into::into).collect::<Vec<_>>();
        if let Some(rule) = self
            .cookie_backed_handoff_rules
            .iter_mut()
            .find(|rule| rule.origin == origin)
        {
            rule.path_prefixes = path_prefixes;
        } else {
            self.cookie_backed_handoff_rules
                .push(CookieBackedHandoffRule {
                    origin: origin.clone(),
                    path_prefixes,
                });
        }
        if !self
            .cookie_backed_handoff_origins
            .iter()
            .any(|candidate| candidate == &origin)
        {
            self.cookie_backed_handoff_origins.push(origin);
        }
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> Result<(), IdentityClientError> {
        if !matches!(self.base_url.scheme(), "http" | "https") {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("base_url must use http or https"),
            });
        }
        if self.base_url.host_str().is_none() {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("base_url must contain a host"),
            });
        }
        if self.base_url.username() != "" || self.base_url.password().is_some() {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("base_url must not contain userinfo"),
            });
        }
        if self.base_url.query().is_some() || self.base_url.fragment().is_some() {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("base_url must not contain a query or fragment"),
            });
        }
        if self.anchor_ticket_origins.is_empty() {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("anchor_ticket_origins must not be empty"),
            });
        }
        for origin in &self.anchor_ticket_origins {
            if !matches!(origin.scheme(), "http" | "https")
                || origin.host_str().is_none()
                || !origin.username().is_empty()
                || origin.password().is_some()
                || origin.query().is_some()
                || origin.fragment().is_some()
            {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from(
                        "anchor_ticket_origins must be http(s) origins without userinfo, query, or fragment",
                    ),
                });
            }
        }
        if self.profile.app_id.trim().is_empty() {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("the identity app id must not be empty"),
            });
        }

        validate_route_template(&self.profile.login_page_path, "login_page_path")?;
        validate_route_template(&self.profile.login_form.submit_path, "submit_path")?;
        validate_field_name(
            &self.profile.login_form.fields.username_field,
            "username_field",
        )?;
        validate_field_name(
            &self.profile.login_form.fields.password_field,
            "password_field",
        )?;
        validate_route_template(
            &self.profile.second_auth.endpoint_path,
            "second_auth.endpoint_path",
        )?;
        validate_field_name(
            &self.profile.second_auth.method_field,
            "second_auth.method_field",
        )?;
        validate_field_name(
            &self.profile.second_auth.action_field,
            "second_auth.action_field",
        )?;
        validate_field_name(
            &self.profile.second_auth.verification_code_field,
            "second_auth.verification_code_field",
        )?;
        for (name, field) in [
            (
                "device_name_field",
                &self.profile.login_form.fields.device_name_field,
            ),
            (
                "fingerprint_field",
                &self.profile.login_form.fields.fingerprint_field,
            ),
            (
                "generated_fingerprint_field",
                &self.profile.login_form.fields.generated_fingerprint_field,
            ),
            (
                "generated_fingerprint_v3_field",
                &self
                    .profile
                    .login_form
                    .fields
                    .generated_fingerprint_v3_field,
            ),
            (
                "captcha_field",
                &self.profile.login_form.fields.captcha_field,
            ),
            (
                "single_login_field",
                &self.profile.login_form.fields.single_login_field,
            ),
        ] {
            if let Some(field) = field {
                validate_field_name(field, name)?;
            }
        }

        if let Some(trusted_device) = self.profile.trusted_device.as_ref() {
            validate_route_template(
                &trusted_device.endpoint_path,
                "trusted_device.endpoint_path",
            )?;
            for (name, field) in [
                (
                    "trusted_device.fingerprint_field",
                    &trusted_device.fingerprint_field,
                ),
                (
                    "trusted_device.device_name_field",
                    &trusted_device.device_name_field,
                ),
                (
                    "trusted_device.decision_field",
                    &trusted_device.decision_field,
                ),
            ] {
                validate_field_name(field, name)?;
            }
            if trusted_device.decision_value.trim().is_empty() {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from("trusted_device.decision_value must not be empty"),
                });
            }
            match (
                trusted_device.single_login_field.as_ref(),
                trusted_device.single_login_value.as_ref(),
            ) {
                (Some(field), Some(value)) => {
                    validate_field_name(field, "trusted_device.single_login_field")?;
                    if value.trim().is_empty() {
                        return Err(IdentityClientError::InvalidConfig {
                            message: String::from(
                                "trusted_device.single_login_value must not be empty",
                            ),
                        });
                    }
                }
                (None, None) => {}
                _ => {
                    return Err(IdentityClientError::InvalidConfig {
                        message: String::from(
                            "trusted_device single-login field and value must be configured together",
                        ),
                    });
                }
            }
        }

        for (collection_name, collection) in [
            ("csrf_field_names", &self.csrf_field_names),
            (
                "sm2_public_key_field_names",
                &self.sm2_public_key_field_names,
            ),
            ("anchor_ticket_query_names", &self.anchor_ticket_query_names),
            ("invalidation_field_names", &self.invalidation_field_names),
            ("second_factor_field_names", &self.second_factor_field_names),
        ] {
            for field in collection {
                validate_field_name(field, collection_name)?;
            }
        }

        if self.anchor_ticket_path_prefixes.is_empty() {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("anchor_ticket_path_prefixes must not be empty"),
            });
        }
        for prefix in &self.anchor_ticket_path_prefixes {
            if prefix.trim().is_empty()
                || !prefix.starts_with('/')
                || prefix.contains(['?', '#'])
                || prefix.chars().any(char::is_control)
            {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from(
                        "anchor_ticket_path_prefixes must be absolute paths without query, fragment, or control characters",
                    ),
                });
            }
        }
        for prefix in &self.cookie_backed_handoff_path_prefixes {
            if prefix.trim().is_empty()
                || !prefix.starts_with('/')
                || prefix.contains(['?', '#'])
                || prefix.chars().any(char::is_control)
            {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from(
                        "cookie_backed_handoff_path_prefixes must be absolute paths without query, fragment, or control characters",
                    ),
                });
            }
        }
        for origin in &self.cookie_backed_handoff_origins {
            if !matches!(origin.scheme(), "http" | "https")
                || origin.host_str().is_none()
                || !origin.username().is_empty()
                || origin.password().is_some()
                || origin.query().is_some()
                || origin.fragment().is_some()
            {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from(
                        "cookie_backed_handoff_origins must be http(s) origins without userinfo, query, or fragment",
                    ),
                });
            }
        }
        for rule in &self.cookie_backed_handoff_rules {
            if !matches!(rule.origin.scheme(), "http" | "https")
                || rule.origin.host_str().is_none()
                || !rule.origin.username().is_empty()
                || rule.origin.password().is_some()
                || rule.origin.query().is_some()
                || rule.origin.fragment().is_some()
                || rule.path_prefixes.is_empty()
            {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from(
                        "cookie-backed handoff routes must have an http(s) origin and at least one path",
                    ),
                });
            }
            for prefix in &rule.path_prefixes {
                if prefix.trim().is_empty()
                    || !prefix.starts_with('/')
                    || prefix.contains(['?', '#'])
                    || prefix.chars().any(char::is_control)
                {
                    return Err(IdentityClientError::InvalidConfig {
                        message: String::from(
                            "cookie-backed handoff route paths must be absolute paths without query, fragment, or control characters",
                        ),
                    });
                }
            }
        }

        for marker in &self.failure_markers {
            validate_field_name(&marker.field_name, "failure_marker.field_name")?;
        }
        for marker in &self.failure_text_markers {
            if marker.text.trim().is_empty() {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from("failure text markers must not be empty"),
                });
            }
        }
        for marker in &self.second_factor_text_markers {
            if marker.trim().is_empty() {
                return Err(IdentityClientError::InvalidConfig {
                    message: String::from("second factor text markers must not be empty"),
                });
            }
        }
        Ok(())
    }
}

/// A client that owns only deployment configuration.  It does not own a
/// transport or a credential; the caller can use the resulting plans with
/// `CampusHttpTransport` and discard the borrowed form immediately after the
/// request is built.
#[derive(Clone, PartialEq, Eq)]
pub struct IdentityClient {
    config: IdentityClientConfig,
    /// The WebVPN bootstrap may attach a signed OAuth callback query to the
    /// dynamic login page.  Keep the complete URL for the request, but never
    /// put that query string in a Debug representation or a Flutter DTO.
    login_page_override: Option<Url>,
    /// The current login page can change its POST action along with the
    /// dynamically generated page.  The WebVPN bootstrap supplies this only
    /// after validating the action against the identity origin and route.
    login_submit_override: Option<Url>,
}

impl fmt::Debug for IdentityClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentityClient")
            .field("config", &self.config)
            .field(
                "login_page_override",
                &self.login_page_override.as_ref().map(redacted_url),
            )
            .field(
                "login_submit_override",
                &self.login_submit_override.as_ref().map(redacted_url),
            )
            .finish()
    }
}

impl IdentityClient {
    pub fn new(config: IdentityClientConfig) -> Result<Self, IdentityClientError> {
        config.validate()?;
        Ok(Self {
            config,
            login_page_override: None,
            login_submit_override: None,
        })
    }

    /// Returns the same validated deployment profile with the application id
    /// discovered from the current WebVPN login bootstrap. The identity form
    /// is generated per WebVPN entry, so an id retained from an older
    /// deployment can make the login route stale.
    pub fn with_app_id(&self, app_id: impl Into<String>) -> Result<Self, IdentityClientError> {
        let mut config = self.config.clone();
        config.profile.app_id = app_id.into();
        let mut client = Self::new(config)?;
        client.login_page_override = None;
        client.login_submit_override = None;
        Ok(client)
    }

    /// Retains the complete dynamically discovered login URL, including the
    /// signed OAuth callback query supplied by WebVPN.  The URL must remain on
    /// the configured identity origin; the opaque query is never surfaced in
    /// diagnostics.
    pub fn with_login_page_url(&self, login_page_url: &Url) -> Result<Self, IdentityClientError> {
        let expected_path = self
            .endpoint_url(&self.config.profile.login_page_path)
            .map(|url| url.path().to_owned())?;
        if !same_origin(&self.config.base_url, login_page_url)
            || login_page_url.fragment().is_some()
            || login_page_url.path().is_empty()
            || login_page_url.path() != expected_path
        {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from(
                    "dynamic login page URL must match the profiled identity route without a fragment",
                ),
            });
        }
        let mut client = self.clone();
        client.login_page_override = Some(login_page_url.clone());
        // A new dynamic page is the authority for its form action. Do not
        // carry an action discovered from a previous WebVPN bootstrap into a
        // new login attempt.
        client.login_submit_override = None;
        Ok(client)
    }

    /// Retains the POST action discovered from the same dynamic identity
    /// login form as [`Self::with_login_page_url`].  This is crate-visible on
    /// purpose: callers must obtain the URL from the validated WebVPN
    /// bootstrap rather than accepting an arbitrary page value.
    pub(crate) fn with_login_submit_url(
        &self,
        submit_url: &Url,
    ) -> Result<Self, IdentityClientError> {
        let expected_path = self
            .endpoint_url(&self.config.profile.login_form.submit_path)
            .map(|url| url.path().to_owned())?;
        if !same_origin(&self.config.base_url, submit_url)
            || submit_url.path().is_empty()
            || (submit_url.path() != expected_path
                && submit_url.path() != IDENTITY_SINGLE_LOGIN_PATH)
            || submit_url.query().is_some()
            || submit_url.fragment().is_some()
        {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from(
                    "dynamic login submit URL must match the profiled submit route or checkSingle without query or fragment",
                ),
            });
        }
        let mut client = self.clone();
        client.login_submit_override = Some(submit_url.clone());
        Ok(client)
    }

    /// Rebinds the form encoding discovered from the same dynamic login page.
    /// The bootstrap parser only returns the two encodings supported by the
    /// execution boundary; keeping this as a validated client replacement
    /// prevents a stale profile value from deciding the wire shape.
    pub(crate) fn with_login_form_encoding(
        &self,
        encoding: FormEncoding,
    ) -> Result<Self, IdentityClientError> {
        let mut config = self.config.clone();
        config.profile.login_form.encoding = encoding;
        Self::new(config).map(|mut client| {
            client.login_page_override = self.login_page_override.clone();
            client.login_submit_override = self.login_submit_override.clone();
            client
        })
    }

    pub fn config(&self) -> &IdentityClientConfig {
        &self.config
    }

    pub fn login_page_url(&self) -> Result<Url, IdentityClientError> {
        if let Some(url) = self.login_page_override.as_ref() {
            return Ok(url.clone());
        }
        self.endpoint_url(&self.config.profile.login_page_path)
    }

    /// Resolves an explicitly profiled login page route for a service handoff
    /// whose application path is not represented by the primary `{appId}/0`
    /// template.  The route is still forced onto the configured identity
    /// origin.
    pub fn login_page_url_at_path(&self, path: &str) -> Result<Url, IdentityClientError> {
        self.endpoint_url(path)
    }

    pub fn login_page_request_plan(&self) -> Result<LoginPageRequestPlan, IdentityClientError> {
        Ok(LoginPageRequestPlan {
            method: Method::GET,
            url: self.login_page_url()?,
        })
    }

    pub fn second_auth_url(&self) -> Result<Url, IdentityClientError> {
        self.endpoint_url(&self.config.profile.second_auth.endpoint_path)
    }

    pub(crate) fn is_second_auth_response_url(&self, url: &Url) -> bool {
        self.second_auth_url()
            .is_ok_and(|expected| same_route(&expected, url))
    }

    /// Returns true when a response came back from the profiled primary
    /// login submit route.  After a verified second-factor JSON response some
    /// deployments render a clean, ticketless 200 on this route instead of
    /// the usual redirect page.  The session layer may use this only in that
    /// already-verified continuation; a primary login response still needs
    /// its own success or handoff evidence.
    pub(crate) fn is_login_submission_response_url(&self, url: &Url) -> bool {
        self.login_submission_url()
            .is_ok_and(|expected| same_route(&expected, url))
    }

    /// Returns true for a same-origin identity continuation that a verified
    /// second-factor flow may finish on without a service-ticket anchor.
    /// `checkSingle` is a known route in the current public Learn client, but
    /// it is deliberately excluded from the primary login profile because it
    /// has a different request shape and is not the live form action in every
    /// deployment.
    pub(crate) fn is_verified_identity_continuation_url(&self, url: &Url) -> bool {
        let is_known_route = self.is_login_submission_response_url(url)
            || self
                .endpoint_url(IDENTITY_SINGLE_LOGIN_PATH)
                .is_ok_and(|expected| same_route(&expected, url));
        is_known_route
            && query_parameter(
                url.query().unwrap_or_default(),
                &self.config.anchor_ticket_query_names,
            )
            .is_none()
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    /// Returns true when the current dynamic login form is the trusted-device
    /// form used by the public Learn client. This form deliberately does not
    /// carry a username or password; its multipart body proves that the
    /// caller is reusing the identity service's already trusted device.
    pub(crate) fn is_check_single_login_submission(&self) -> bool {
        self.login_submission_url().is_ok_and(|url| {
            same_origin(&self.config.base_url, &url)
                && url.path() == IDENTITY_SINGLE_LOGIN_PATH
                && url.query().is_none()
                && url.fragment().is_none()
        })
    }

    pub(crate) fn is_trusted_device_response_url(&self, url: &Url) -> bool {
        self.trusted_device_url()
            .is_ok_and(|expected| same_route(&expected, url))
    }

    pub fn trusted_device_url(&self) -> Result<Url, IdentityClientError> {
        let profile = self
            .config
            .profile
            .trusted_device
            .as_ref()
            .ok_or(IdentityClientError::UnsupportedTrustedDevice)?;
        self.endpoint_url(&profile.endpoint_path)
    }

    /// Resolves a redirect returned by the identity service while enforcing
    /// the configured origin or an explicitly allowlisted service handoff
    /// origin.  The redirect may be an absolute URL or a path.  The latter
    /// exception is needed after second-factor verification: the identity
    /// success page can expose a public `/b/` or `/f/` anchor whose next
    /// response carries the one-time ticket.
    pub fn resolve_redirect_url(&self, redirect: &str) -> Result<Url, IdentityClientError> {
        if redirect.chars().any(char::is_control) {
            return Err(IdentityClientError::InvalidUrl {
                url: String::from("[redacted]"),
                message: String::from("redirect contains control characters"),
            });
        }

        let url = self.config.base_url.join(redirect).map_err(|error| {
            IdentityClientError::InvalidUrl {
                url: String::from("[redacted]"),
                message: error.to_string(),
            }
        })?;

        if same_origin(&self.config.base_url, &url)
            || self.handoff_url_allowed(&url)
            || self.cookie_handoff_url_allowed(&url)
        {
            Ok(url)
        } else {
            Err(IdentityClientError::InvalidConfig {
                message: String::from(
                    "identity redirects must remain on the identity origin or an allowlisted handoff origin",
                ),
            })
        }
    }

    /// Returns true for a safe, ticketless handoff URL that the session layer
    /// may GET before it can extract the final ticket.  Recent public clients
    /// follow the identity callback and deployments may insert an explicitly
    /// allowlisted `/b/` or `/f/` anchor between that callback and the ticket.
    /// A service URL carrying a ticket is evidence to consume, not another
    /// URL to follow. The configured OAuth broker is different: its opaque
    /// ticket is exchanged by that broker and never becomes a Learn ticket.
    pub fn is_safe_handoff_redirect(&self, redirect: &str) -> bool {
        let Ok(url) = self.resolve_redirect_url(redirect) else {
            return false;
        };
        (self.is_identity_callback_url(&url)
            || self.handoff_url_allowed(&url)
            || self.cookie_handoff_url_allowed(&url)
            // `checkSingle` is a same-origin continuation emitted by the
            // verified identity flow.  It is intentionally accepted here
            // only as a ticketless route; the session layer decides whether
            // the preceding second-factor proof permits it.
            || (self.is_verified_identity_continuation_url(&url)
                && !self.is_login_submission_response_url(&url)))
            && self.navigation_query_allowed(&url)
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    /// Returns true for a redirect URL that may be returned directly by the
    /// second-factor JSON response. Service `/b/` and `/f/` URLs are excluded:
    /// those are HTML anchor evidence and must be reached only after the
    /// identity callback or an explicitly fetched Cookie handoff.
    pub(crate) fn is_second_factor_redirect_url(&self, redirect: &str) -> bool {
        let Ok(url) = self.resolve_redirect_url(redirect) else {
            return false;
        };
        let continuation = self.is_identity_callback_url(&url)
            || self.is_verified_identity_continuation_url(&url)
            || self.cookie_handoff_url_allowed(&url);
        continuation
            && query_parameter(
                url.query().unwrap_or_default(),
                &self.config.anchor_ticket_query_names,
            )
            .is_none()
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    /// Returns true only for an allowlisted, ticketless service handoff URL
    /// that is safe to fetch with the shared Cookie jar.  The identity
    /// callback is intentionally excluded: fetching that URL again without a
    /// new redirect can create a self-loop and does not itself prove a
    /// service session.
    pub fn is_cookie_backed_handoff_url(&self, url: &Url) -> bool {
        (self.handoff_url_allowed(url) || self.cookie_handoff_url_allowed(url))
            && !is_redirect_callback_path(url.path())
            && self.navigation_query_allowed(url)
    }

    /// Distinguish an explicitly configured broker's callback credential
    /// from a downstream service-ticket capability. Only /thu-oauth/ on an
    /// origin-scoped cookie rule may carry one nonempty, valid ticket during
    /// navigation. Other hosts, routes, duplicates and malformed values keep
    /// the existing fail-closed behavior. The original URL is never rewritten.
    fn navigation_query_allowed(&self, url: &Url) -> bool {
        if query_parameter(
            url.query().unwrap_or_default(),
            &self.config.anchor_ticket_query_names,
        )
        .is_none()
        {
            return true;
        }
        if !self.cookie_handoff_url_allowed(url)
            || !url.path().starts_with("/thu-oauth/")
            || !self.config.cookie_backed_handoff_rules.iter().any(|rule| {
                same_origin(&rule.origin, url)
                    && rule
                        .path_prefixes
                        .iter()
                        .any(|prefix| prefix == "/thu-oauth/")
            })
        {
            return false;
        }
        let values: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| {
                self.config
                    .anchor_ticket_query_names
                    .iter()
                    .any(|name| name == key)
            })
            .collect();
        values.len() == 1 && values[0].0 == "ticket" && valid_anchor_ticket(&values[0].1)
    }

    /// Returns true for a ticketless service handoff route that is already
    /// covered by the configured ticket-origin/path allowlist.  This is used
    /// only after the identity service has returned a verified second-factor
    /// flow and the route has been fetched with the shared Cookie jar.  It is
    /// deliberately separate from `is_cookie_backed_handoff_url`: the Learn
    /// origin is a ticket source in older deployments and a Cookie-backed
    /// destination in newer ones, so callers must still require the Learn
    /// page's own CSRF/session evidence before accepting this shape.
    pub(crate) fn is_ticketless_service_handoff_url(&self, url: &Url) -> bool {
        // Learn can be reached directly on its `/b/` or `/f/` origin, or
        // through the WebVPN wrapper emitted by the OAuth broker.  Both are
        // downstream service handoffs: an identity/verified-flow proof must
        // never promote either shape without Learn's own CSRF or Cookie
        // evidence.
        (self.handoff_url_allowed(url) || self.webvpn_wrapper_url_allowed(url))
            && !is_redirect_callback_path(url.path())
            && query_parameter(
                url.query().unwrap_or_default(),
                &self.config.anchor_ticket_query_names,
            )
            .is_none()
    }

    /// Compatibility name for callers that only handled the original
    /// redirect2Jsp callback.  The session layer uses
    /// [`Self::is_safe_handoff_redirect`] because the deployed flow can also
    /// contain a ticketless allowlisted public anchor.
    pub fn is_ticket_redirect_callback(&self, redirect: &str) -> bool {
        self.is_safe_handoff_redirect(redirect)
    }

    /// Returns true for the identity service's own callback route when it is
    /// still ticketless.  A successful login/check response can send the
    /// browser through this route before the service Cookie is usable.  The
    /// session layer may accept a successful response from this exact route
    /// as a Cookie-backed identity handoff, but only after it rejects login
    /// and failure pages and only when no more specific service anchor is
    /// present.
    pub(crate) fn is_identity_callback_url(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && same_origin(&self.config.base_url, url)
            && is_redirect_callback_path(url.path())
            && query_parameter(
                url.query().unwrap_or_default(),
                &self.config.anchor_ticket_query_names,
            )
            .is_none()
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    pub fn build_second_auth_form<'a>(
        &self,
        action: &'a crate::identity::SecondAuthAction,
        method: Option<&'a crate::identity::SecondAuthMethod>,
        verification_code: Option<&'a str>,
    ) -> Result<UrlEncodedSecondAuthForm<'a>, IdentityClientError> {
        let mut fields = Vec::with_capacity(3);
        let profile = &self.config.profile.second_auth;
        fields.push(UrlEncodedField {
            name: profile.action_field.clone(),
            value: FormValue::Text(action.wire_value()),
        });
        if let Some(method) = method {
            fields.push(UrlEncodedField {
                name: profile.method_field.clone(),
                value: FormValue::Text(method.wire_value()),
            });
        }
        if let Some(code) = verification_code.filter(|value| !value.is_empty()) {
            fields.push(UrlEncodedField {
                name: profile.verification_code_field.clone(),
                value: FormValue::Sensitive(code),
            });
        }
        Ok(UrlEncodedSecondAuthForm {
            url: self.second_auth_url()?,
            fields,
        })
    }

    pub fn trusted_device_request_plan(
        &self,
    ) -> Result<TrustedDeviceRequestPlan, IdentityClientError> {
        let profile = self
            .config
            .profile
            .trusted_device
            .as_ref()
            .ok_or(IdentityClientError::UnsupportedTrustedDevice)?;
        Ok(TrustedDeviceRequestPlan {
            method: Method::POST,
            url: self.trusted_device_url()?,
            content_type: URL_ENCODED_CONTENT_TYPE,
            fields: profile.clone(),
        })
    }

    pub fn build_trusted_device_form(
        &self,
        input: TrustedDeviceInput<'_>,
    ) -> Result<UrlEncodedTrustedDeviceForm, IdentityClientError> {
        self.trusted_device_request_plan()?.build_form(input)
    }

    pub fn login_submission_request_plan(
        &self,
    ) -> Result<LoginSubmissionRequestPlan, IdentityClientError> {
        let url = self.login_submission_url()?;
        if self.is_check_single_login_submission() {
            let fields = &self.config.profile.login_form.fields;
            let fingerprint_field =
                fields
                    .fingerprint_field
                    .clone()
                    .ok_or(IdentityClientError::InvalidConfig {
                        message: String::from(
                            "checkSingle login requires a configured fingerprint field",
                        ),
                    })?;
            let generated_fingerprint_field = fields.generated_fingerprint_field.clone().ok_or(
                IdentityClientError::InvalidConfig {
                    message: String::from(
                        "checkSingle login requires a configured generated fingerprint field",
                    ),
                },
            )?;
            return Ok(LoginSubmissionRequestPlan::TrustedDeviceMultipart(
                TrustedDeviceLoginRequestPlan {
                    method: Method::POST,
                    url,
                    content_type: MULTIPART_CONTENT_TYPE,
                    remember_field: CHECK_SINGLE_REMEMBER_FIELD,
                    fingerprint_field,
                    generated_fingerprint_field,
                },
            ));
        }
        let fields = self.config.profile.login_form.fields.clone();

        match &self.config.profile.login_form.encoding {
            FormEncoding::UrlEncoded => Ok(LoginSubmissionRequestPlan::UrlEncoded(
                UrlEncodedLoginRequestPlan {
                    method: Method::POST,
                    url,
                    content_type: URL_ENCODED_CONTENT_TYPE,
                    fields,
                },
            )),
            FormEncoding::Multipart => Ok(LoginSubmissionRequestPlan::Multipart(
                MultipartLoginRequestPlan {
                    method: Method::POST,
                    url,
                    content_type: MULTIPART_CONTENT_TYPE,
                    field_names: fields,
                    boundary: MultipartBoundaryPlan::GeneratedByTransport,
                },
            )),
            FormEncoding::Other(encoding) => Err(IdentityClientError::UnsupportedFormEncoding {
                encoding: encoding.clone(),
            }),
        }
    }

    pub fn build_urlencoded_login_form<'a>(
        &self,
        input: LoginFormInput<'a>,
    ) -> Result<UrlEncodedLoginForm<'a>, IdentityClientError> {
        match self.login_submission_request_plan()? {
            LoginSubmissionRequestPlan::UrlEncoded(plan) => plan.build_form(input),
            LoginSubmissionRequestPlan::Multipart(_)
            | LoginSubmissionRequestPlan::TrustedDeviceMultipart(_) => {
                Err(IdentityClientError::MultipartRequestPlanOnly)
            }
        }
    }

    pub fn build_multipart_login_form<'a>(
        &self,
        input: LoginFormInput<'a>,
        boundary: impl Into<String>,
    ) -> Result<MultipartLoginForm<'a>, IdentityClientError> {
        let plan = self.multipart_login_request_plan()?;
        plan.build_form(input, boundary)
    }

    /// Builds the trusted-device `checkSingle` body. The public Learn client
    /// submits this route as multipart with `i_rememberme`, `fingerPrint`,
    /// and `fingerGenPrint`; it does not submit identity credentials.
    pub fn build_check_single_multipart_login_form<'a>(
        &self,
        input: LoginFormInput<'a>,
        boundary: impl Into<String>,
    ) -> Result<MultipartLoginForm<'a>, IdentityClientError> {
        let LoginSubmissionRequestPlan::TrustedDeviceMultipart(plan) =
            self.login_submission_request_plan()?
        else {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from(
                    "checkSingle multipart login requires a checkSingle submit route",
                ),
            });
        };
        let boundary = boundary.into();
        if !valid_multipart_boundary(&boundary) {
            return Err(IdentityClientError::InvalidMultipartBoundary);
        }
        let fields =
            build_check_single_login_fields(&self.config.profile.login_form.fields, &plan, input)?;
        Ok(MultipartLoginForm {
            url: plan.url,
            boundary,
            fields,
        })
    }

    pub fn multipart_login_request_plan(
        &self,
    ) -> Result<MultipartLoginRequestPlan, IdentityClientError> {
        match self.login_submission_request_plan()? {
            LoginSubmissionRequestPlan::Multipart(plan) => Ok(plan),
            LoginSubmissionRequestPlan::UrlEncoded(_)
            | LoginSubmissionRequestPlan::TrustedDeviceMultipart(_) => {
                Err(IdentityClientError::InvalidConfig {
                    message: String::from(
                        "multipart_login_request_plan requires a credential-bearing multipart login profile",
                    ),
                })
            }
        }
    }

    pub fn parse_login_page(&self, html: &str) -> LoginPageEvidence {
        self.parse_login_page_at(&self.config.base_url, html)
    }

    /// Fixed structural diagnostic only. Never returns a field/action value,
    /// HTML fragment or credential. Script-only presence is NOT permission
    /// to submit: service_check_single_action remains the authority.
    pub(crate) fn service_form_diagnostic(&self, page_url: &Url, html: &str) -> &'static str {
        if self.service_check_single_action(page_url, html).is_some() {
            return "trusted_form";
        }
        let parsed = parse_html(html);
        if parsed
            .scripts
            .iter()
            .any(|script| script.contains("checkSingle"))
        {
            return "trusted_script";
        }
        let evidence = self.parse_login_page(html);
        if evidence.has_login_form && evidence.sm2_public_key.is_some() {
            return "password_form";
        }
        if evidence.has_login_form {
            return "form_without_key";
        }
        "no_form"
    }

    /// Selects a credential-free continuation from an actual trusted-device
    /// POST form. A mere mention of checkSingle is not authority to submit.
    pub(crate) fn service_check_single_action(&self, page_url: &Url, html: &str) -> Option<Url> {
        if !same_origin(&self.config.base_url, page_url)
            || !page_url.username().is_empty()
            || page_url.password().is_some()
            || page_url.fragment().is_some()
            || !(page_url.path().starts_with("/do/off/ui/auth/login/form/")
                || page_url.path() == IDENTITY_SINGLE_LOGIN_PATH)
        {
            return None;
        }
        let evidence = self.parse_login_page(html);
        let generic_form_notice = page_url.path().starts_with("/do/off/ui/auth/login/form/")
            && evidence
                .failure
                .as_ref()
                .is_some_and(|failure| failure.reason == LoginFailureReason::Generic);
        // A GET form's generic template notice is not a failed submission.
        // Only this structural, passwordless continuation may proceed;
        // explicit invalidation/credentials/captcha/2FA still fail closed.
        if (evidence.failure.is_some() && !generic_form_notice)
            || evidence.second_factor_marker.is_some()
        {
            return None;
        }
        let mut forms = parse_forms(html).into_iter().filter_map(|form| {
            if !form
                .method
                .as_deref()
                .is_some_and(|method| method.eq_ignore_ascii_case("post"))
            {
                return None;
            }
            let action = resolve_untrusted_url(page_url, form.action.as_deref()?)?;
            (same_origin(&self.config.base_url, &action)
                && action.path() == IDENTITY_SINGLE_LOGIN_PATH
                && action.query().is_none())
            .then_some(action)
        });
        let action = forms.next()?;
        forms.next().is_none().then_some(action)
    }

    /// Returns true only for the deployment's explicit identity-success
    /// wording. The session layer combines this with a profiled callback or a
    /// Cookie update; the marker is never accepted as a standalone login
    /// result.
    pub(crate) fn has_identity_success_marker(&self, html: &str) -> bool {
        has_explicit_success_marker(html)
    }

    fn parse_login_page_at(&self, base_url: &Url, html: &str) -> LoginPageEvidence {
        let parsed = parse_html(html);
        let sm2_public_key = find_sm2_public_key(
            &parsed.tags,
            &parsed.scripts,
            &self.config.sm2_public_key_field_names,
        );
        // Inspect every supported handoff source before selecting one.  A
        // success template can contain a ticketless callback anchor before a
        // real ticket in a form, script, or meta refresh.  Chaining these
        // searches with `or_else` would follow the callback first and lose the
        // stronger ticket evidence when that callback renders a terminal page.
        let anchor_ticket = select_preferred_handoff([
            find_anchor_ticket(
                base_url,
                &parsed.tags,
                &self.config.anchor_ticket_query_names,
                &self.config.anchor_ticket_path_prefixes,
                &self.config.anchor_ticket_origins,
            ),
            self.find_webvpn_wrapper_anchor(base_url, &parsed.tags),
            self.find_form_ticket(base_url, html),
            self.find_script_ticket(base_url, &parsed.scripts),
            self.find_meta_refresh_handoff(base_url, &parsed.tags),
            self.find_cookie_backed_handoff_anchor(base_url, &parsed.tags),
        ]);
        let csrf = find_csrf(&parsed.tags, &self.config.csrf_field_names);
        let invalidation = find_invalidation(&parsed.tags, &self.config.invalidation_field_names);
        let mut failure = find_failure_marker(
            &parsed.tags,
            &parsed.visible_text,
            &self.config.failure_markers,
            &self.config.failure_text_markers,
        );
        if failure
            .as_ref()
            .is_none_or(|evidence| evidence.reason == LoginFailureReason::Generic)
            && crate::webvpn_identity::has_active_identity_image_captcha(html)
        {
            failure = Some(LoginFailureEvidence {
                reason: LoginFailureReason::CaptchaRequired,
                source: FailureEvidenceSource::Text,
                marker: "active_image_captcha".to_owned(),
                raw_value: None,
            });
        }
        if failure.is_none() && invalidation.status == InvalidationStatus::Marked {
            failure = Some(LoginFailureEvidence {
                reason: LoginFailureReason::SessionInvalid,
                source: FailureEvidenceSource::InvalidationMarker,
                marker: invalidation
                    .field_name
                    .clone()
                    .unwrap_or_else(|| String::from("invalidation")),
                raw_value: invalidation.raw_value.clone(),
            });
        }

        let has_login_form = has_login_form(
            &parsed.tags,
            &self.config.profile.login_form.fields,
            &self.config.profile.login_form.submit_path,
        );
        let second_factor_marker = find_second_factor_marker(
            &parsed.tags,
            &parsed.visible_text,
            &self.config.second_factor_field_names,
            &self.config.second_factor_text_markers,
        );

        LoginPageEvidence {
            sm2_public_key,
            anchor_ticket,
            csrf,
            invalidation,
            failure,
            has_login_form,
            second_factor_marker,
        }
    }

    /// Returns the first safe anchor on a response page for one explicitly
    /// allowlisted service origin.  A few campus applications (the card
    /// portal in particular) return a direct service URL instead of the
    /// ticket-shaped `/b/` or `/f/` anchor used by Learn.  The URL is still
    /// constrained to the supplied origin and cannot contain userinfo or
    /// control characters.
    pub fn first_anchor_url_for_origin(&self, html: &str, origin: &Url) -> Option<Url> {
        let origin = origin_url(origin);
        parse_html(html).tags.into_iter().find_map(|tag| {
            if tag.name != "a" {
                return None;
            }
            let href = tag.attribute_value("href")?;
            if href.trim().is_empty() || href.chars().any(char::is_control) {
                return None;
            }
            let url = self.config.base_url.join(&href).ok()?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || !same_origin(&origin, &url)
            {
                return None;
            }
            Some(url)
        })
    }

    /// Returns the first static continuation in an identity HTML page that
    /// resolves to one explicitly supplied service origin. This is narrower
    /// than the normal identity handoff parser: callers opt into exactly one
    /// target origin (for example INFO or Campus Card), while this method only
    /// recognizes static browser navigation shapes already supported by the
    /// identity parser: anchor href, form action, literal script redirects and
    /// meta refresh.
    pub fn first_static_redirect_url_for_origin(
        &self,
        base_url: &Url,
        html: &str,
        origin: &Url,
    ) -> Option<Url> {
        let origin = origin_url(origin);
        let safe = |raw: &str| {
            let url = resolve_untrusted_url(base_url, raw)?;
            (matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && query_is_well_formed(url.query().unwrap_or_default())
                && same_origin(&origin, &url))
            .then_some(url)
        };

        let parsed = parse_html(html);
        if let Some(url) = parsed.tags.iter().find_map(|tag| match tag.name.as_str() {
            "a" => tag.attribute_value("href").and_then(|value| safe(&value)),
            "meta" => {
                let http_equiv = tag.attribute_value("http-equiv")?;
                if !http_equiv.eq_ignore_ascii_case("refresh") {
                    return None;
                }
                let content = tag.attribute_value("content")?;
                meta_refresh_url(&content).and_then(&safe)
            }
            _ => None,
        }) {
            return Some(url);
        }

        if let Some(url) = parse_forms(html)
            .into_iter()
            .filter_map(|form| form.action)
            .find_map(|action| safe(&action))
        {
            return Some(url);
        }

        parsed
            .scripts
            .iter()
            .flat_map(|script| find_static_script_redirects(script))
            .find_map(|redirect| safe(&redirect))
    }

    pub fn classify_login_response(
        &self,
        status: StatusCode,
        final_url: &Url,
        html: &str,
    ) -> LoginResponseClassification {
        self.classify_login_response_with_location(status, final_url, None, html)
    }

    /// Classifies a response while retaining a redirect location that the
    /// transport stopped before crossing to another explicitly allowed origin.
    /// This is needed for deployments that return a 302 directly to a Learn
    /// handoff instead of rendering an intermediate HTML anchor.
    pub fn classify_login_response_with_location(
        &self,
        status: StatusCode,
        final_url: &Url,
        redirect_location: Option<&Url>,
        html: &str,
    ) -> LoginResponseClassification {
        if status.is_redirection() {
            if let Some(location) = redirect_location {
                if let Some(classification) =
                    self.classify_handoff_location(final_url, location, html)
                {
                    return classification;
                }
            }
            return LoginResponseClassification::HttpStatus { status };
        }
        if !status.is_success() {
            return LoginResponseClassification::HttpStatus { status };
        }
        if !self.anchor_origin_allowed(final_url) && !self.cookie_handoff_origin_allowed(final_url)
        {
            return LoginResponseClassification::UnexpectedOrigin;
        }
        if let Some(anchor) = self.find_anchor_ticket_url(final_url) {
            let mut page = self.parse_login_page_at(final_url, html);
            page.anchor_ticket = Some(anchor);
            if let Some(evidence) = page.failure.clone() {
                return LoginResponseClassification::LoginFailed { page, evidence };
            }
            return LoginResponseClassification::AuthenticatedHandoff(page);
        }
        let mut page = self.parse_login_page_at(final_url, html);
        if let Some(evidence) = page.failure.clone() {
            return LoginResponseClassification::LoginFailed { page, evidence };
        }
        // Most deployments use a 3xx Location for this handoff, but some
        // identity success templates return HTTP 200/201 while still
        // providing the only service continuation in Location.  The
        // execution layer deliberately retains that header; consume it only
        // through the same origin/path/ticket allowlist used for 3xx so a
        // generic Location header can never authenticate a response.
        if page
            .anchor_ticket
            .as_ref()
            .is_some_and(|anchor| anchor.ticket.is_some())
        {
            return LoginResponseClassification::AuthenticatedHandoff(page);
        }
        if let Some(location) = redirect_location {
            if let Some(classification) = self.classify_handoff_location(final_url, location, html)
            {
                return classification;
            }
        }
        // A valid ticket from an explicitly allowlisted origin and handoff
        // route is the strongest success evidence.  Current identity pages
        // can retain the login form and a hidden default msg_note in the
        // success template, so requiring a particular success sentence here
        // would reject a real handoff.  The ticket is still checked against
        // both origin and path allowlists above and is later proven by the
        // downstream service request.
        if page.anchor_ticket.as_ref().is_some_and(|anchor| {
            anchor.ticket.is_none() && self.is_safe_handoff_redirect(&anchor.href)
        }) {
            // The deployed success template can retain the original login
            // form (including its hidden fields) while exposing the OAuth or
            // WebVPN continuation as an anchor.  The allowlisted continuation
            // is stronger evidence than the stale form markup, but it still
            // has to be fetched by the session layer before authentication is
            // promoted.
            return LoginResponseClassification::RedirectCallback(page);
        }
        if let Some(marker) = page.second_factor_marker.clone() {
            return LoginResponseClassification::RequiresSecondFactor { page, marker };
        }
        if is_redirect_callback_path(final_url.path()) {
            // A callback response may itself render the next public anchor;
            // parse_login_page_at already preserves that more specific
            // continuation.  An empty href or an empty script assignment is
            // deliberately not an anchor: Url::join would turn it into the
            // current callback URL and the session layer could otherwise
            // mistake a terminal, ticketless success page for another GET.
            // The session layer can still recognize this exact callback as a
            // verified ticketless identity handoff when the second-factor
            // JSON has already proved the flow.
            return LoginResponseClassification::RedirectCallback(page);
        }
        if self.is_explicit_success_page(final_url, html) {
            // A success sentence is useful evidence, but it is not itself a
            // Cookie or service-ticket proof. Keep the page in the generic
            // branch so the session layer can require a concrete continuation
            // anchor, a fetched Cookie handoff, or a response Cookie update.
            return LoginResponseClassification::OtherPage(page);
        }
        if page.has_login_form || self.url_is_login_endpoint(final_url) {
            LoginResponseClassification::LoginPage(page)
        } else if self.is_cookie_backed_handoff_url(final_url) && has_cookie_handoff_proof(&page) {
            // The URL itself has already been fetched by the transport. Keep
            // the evidence separate from a ticketless anchor that still
            // needs one GET. A route allowlist and a successful HTTP status
            // alone are not enough: the fetched downstream page must expose
            // the CSRF/session evidence used by the service adapter.
            if page.anchor_ticket.is_none() {
                page.anchor_ticket = Some(AnchorTicket {
                    href: final_url.to_string(),
                    ticket: None,
                });
            }
            LoginResponseClassification::CookieBackedHandoff(page)
        } else {
            LoginResponseClassification::OtherPage(page)
        }
    }

    /// Classifies a Location header that the identity execution layer has
    /// retained.  The header may be present on either a redirect response or
    /// a successful response, so this helper must apply the complete
    /// handoff allowlist before returning any authentication evidence.
    fn classify_handoff_location(
        &self,
        final_url: &Url,
        location: &Url,
        html: &str,
    ) -> Option<LoginResponseClassification> {
        if let Some(anchor) = self.find_anchor_ticket_url(location) {
            let mut page = self.parse_login_page_at(final_url, html);
            page.anchor_ticket = Some(anchor);
            if let Some(evidence) = page.failure.clone() {
                return Some(LoginResponseClassification::LoginFailed { page, evidence });
            }
            return Some(LoginResponseClassification::AuthenticatedHandoff(page));
        }
        if !self.is_safe_handoff_redirect(location.as_str()) {
            return None;
        }

        // A deployment may return a ticketless cross-origin Location. The
        // shared Cookie jar is the authentication proof for this shape, but
        // the target still has to be fetched and proven by the downstream
        // service. Keep it as a continuation rather than treating the
        // response itself as a completed service session.
        let mut page = self.parse_login_page_at(final_url, html);
        page.anchor_ticket = Some(AnchorTicket {
            href: location.to_string(),
            ticket: None,
        });
        if let Some(evidence) = page.failure.clone() {
            return Some(LoginResponseClassification::LoginFailed { page, evidence });
        }
        Some(LoginResponseClassification::RedirectCallback(page))
    }

    fn endpoint_url(&self, route_template: &str) -> Result<Url, IdentityClientError> {
        let encoded_app_id = encode_path_segment(&self.config.profile.app_id);
        let route = route_template.replace(APP_ID_PLACEHOLDER, &encoded_app_id);
        if route.contains('{') || route.contains('}') {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("route contains an unresolved template placeholder"),
            });
        }

        let url = if route.starts_with("http://") || route.starts_with("https://") {
            Url::parse(&route).map_err(|error| IdentityClientError::InvalidUrl {
                url: route.clone(),
                message: error.to_string(),
            })?
        } else {
            self.config
                .base_url
                .join(&route)
                .map_err(|error| IdentityClientError::InvalidUrl {
                    url: route.clone(),
                    message: error.to_string(),
                })?
        };

        if !same_origin(&self.config.base_url, &url) {
            return Err(IdentityClientError::InvalidConfig {
                message: String::from("identity routes must remain on the configured origin"),
            });
        }
        Ok(url)
    }

    fn url_is_login_endpoint(&self, url: &Url) -> bool {
        let Ok(login_page_url) = self.login_page_url() else {
            return false;
        };
        let Ok(submit_url) = self.login_submission_url() else {
            return false;
        };
        same_route(url, &login_page_url) || same_route(url, &submit_url)
    }

    fn is_explicit_success_page(&self, final_url: &Url, html: &str) -> bool {
        let Ok(submit_url) = self.login_submission_url() else {
            return false;
        };
        let is_profiled_success_route = same_route(final_url, &submit_url)
            || (same_origin(&self.config.base_url, final_url)
                && is_redirect_callback_path(final_url.path()));
        is_profiled_success_route && has_explicit_success_marker(html)
    }

    fn login_submission_url(&self) -> Result<Url, IdentityClientError> {
        self.login_submit_override
            .clone()
            .map(Ok)
            .unwrap_or_else(|| self.endpoint_url(&self.config.profile.login_form.submit_path))
    }

    fn find_anchor_ticket_url(&self, url: &Url) -> Option<AnchorTicket> {
        if !self.handoff_url_allowed(url) && !self.webvpn_wrapper_url_allowed(url) {
            return None;
        }
        let (_, ticket) = query_parameter(
            url.query().unwrap_or_default(),
            &self.config.anchor_ticket_query_names,
        )?;
        let ticket = ticket.filter(|value| valid_anchor_ticket(value));
        ticket.map(|ticket| AnchorTicket {
            href: url.to_string(),
            ticket: Some(ticket),
        })
    }

    fn find_webvpn_wrapper_anchor(&self, base_url: &Url, tags: &[HtmlTag]) -> Option<AnchorTicket> {
        tags.iter().filter(|tag| tag.name == "a").find_map(|tag| {
            let href = tag.attribute_value("href")?;
            let url = resolve_untrusted_url(base_url, &href)?;
            self.webvpn_wrapper_url_allowed(&url)
                .then(|| self.find_anchor_ticket_url(&url))
                .flatten()
        })
    }

    fn find_form_ticket(&self, base_url: &Url, html: &str) -> Option<AnchorTicket> {
        parse_forms(html).into_iter().find_map(|form| {
            let action = form.action?;
            let url = resolve_untrusted_url(base_url, &action)?;
            let ticket = hidden_form_ticket(&form.inputs, &self.config.anchor_ticket_query_names)?;
            if !valid_anchor_ticket(&ticket)
                || (!self.handoff_url_allowed(&url) && !self.webvpn_wrapper_url_allowed(&url))
                || query_parameter(
                    url.query().unwrap_or_default(),
                    &self.config.anchor_ticket_query_names,
                )
                .is_some()
            {
                return None;
            }
            Some(AnchorTicket {
                href: url.to_string(),
                ticket: Some(ticket),
            })
        })
    }

    fn find_script_ticket(&self, base_url: &Url, scripts: &[String]) -> Option<AnchorTicket> {
        select_preferred_handoff(scripts.iter().flat_map(|script| {
            find_static_script_redirects(script)
                .into_iter()
                .map(|redirect| {
                    let url = resolve_untrusted_url(base_url, &redirect)?;
                    self.anchor_from_handoff_url(&url)
                })
        }))
    }

    fn find_meta_refresh_handoff(&self, base_url: &Url, tags: &[HtmlTag]) -> Option<AnchorTicket> {
        select_preferred_handoff(tags.iter().filter(|tag| tag.name == "meta").map(|tag| {
            let http_equiv = tag.attribute_value("http-equiv")?;
            if !http_equiv.eq_ignore_ascii_case("refresh") {
                return None;
            }
            let content = tag.attribute_value("content")?;
            let redirect = meta_refresh_url(&content)?;
            let url = resolve_untrusted_url(base_url, redirect)?;
            self.anchor_from_handoff_url(&url)
        }))
    }

    fn anchor_from_handoff_url(&self, url: &Url) -> Option<AnchorTicket> {
        if let Some(anchor) = self.find_anchor_ticket_url(url) {
            return Some(anchor);
        }
        if (self.handoff_url_allowed(url) || self.cookie_handoff_url_allowed(url))
            && self.navigation_query_allowed(url)
        {
            return Some(AnchorTicket {
                href: url.to_string(),
                ticket: None,
            });
        }
        None
    }

    fn find_cookie_backed_handoff_anchor(
        &self,
        base_url: &Url,
        tags: &[HtmlTag],
    ) -> Option<AnchorTicket> {
        tags.iter().filter(|tag| tag.name == "a").find_map(|tag| {
            let href = tag.attribute_value("href")?;
            let url = resolve_untrusted_url(base_url, &href)?;
            if !self.cookie_handoff_url_allowed(&url) || !self.navigation_query_allowed(&url) {
                return None;
            }
            Some(AnchorTicket {
                href: url.to_string(),
                ticket: None,
            })
        })
    }

    fn handoff_url_allowed(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && self.anchor_origin_allowed(url)
            && allowed_anchor_path(url.path(), &self.config.anchor_ticket_path_prefixes)
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    fn cookie_handoff_url_allowed(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && self.cookie_handoff_origin_allowed(url)
            && (self.cookie_handoff_path_allowed(url)
                || self.webvpn_cookie_handoff_path_allowed(url))
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    fn webvpn_wrapper_url_allowed(&self, url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
            && self.cookie_handoff_origin_allowed(url)
            && self.webvpn_wrapper_path_allowed(url)
            && query_is_well_formed(url.query().unwrap_or_default())
            && url.fragment().is_none()
    }

    fn webvpn_wrapper_path_allowed(&self, url: &Url) -> bool {
        is_webvpn_learn_handoff_path(url.path())
    }

    /// Returns true for the explicitly profiled WebVPN paths that may carry an
    /// authenticated Cookie handoff without being a service-ticket source:
    /// the Learn wrapper, the identity callback wrapper, and the root wrapper
    /// emitted by OAuth `lbredirect`.
    ///
    /// A root wrapper has the form `/https/<opaque>%2F` (with equivalent
    /// `http`, `https-443`, or `http-80` prefixes).  It is intentionally
    /// checked as a single encoded path segment so `/https/<opaque>/other`
    /// cannot become an arbitrary WebVPN continuation.  The Learn wrapper
    /// remains separate in `webvpn_wrapper_path_allowed` because only that
    /// route may be used as an opaque ticket source.
    fn webvpn_cookie_handoff_path_allowed(&self, url: &Url) -> bool {
        is_webvpn_learn_handoff_path(url.path())
            || is_webvpn_identity_handoff_path(url.path())
            || is_webvpn_root_handoff_path(url.path())
    }

    fn cookie_handoff_path_allowed(&self, url: &Url) -> bool {
        if !self.config.cookie_backed_handoff_rules.is_empty() {
            return self.config.cookie_backed_handoff_rules.iter().any(|rule| {
                same_origin(&rule.origin, url)
                    && allowed_cookie_handoff_path(url.path(), &rule.path_prefixes)
            });
        }
        !self.config.cookie_backed_handoff_origins.is_empty()
            && !self.config.cookie_backed_handoff_path_prefixes.is_empty()
            && allowed_cookie_handoff_path(
                url.path(),
                &self.config.cookie_backed_handoff_path_prefixes,
            )
    }

    fn cookie_handoff_origin_allowed(&self, url: &Url) -> bool {
        url.username().is_empty()
            && url.password().is_none()
            && (self
                .config
                .cookie_backed_handoff_origins
                .iter()
                .any(|origin| same_origin(origin, url))
                || self
                    .config
                    .cookie_backed_handoff_rules
                    .iter()
                    .any(|rule| same_origin(&rule.origin, url)))
    }

    fn anchor_origin_allowed(&self, url: &Url) -> bool {
        url.username().is_empty()
            && url.password().is_none()
            && self
                .config
                .anchor_ticket_origins
                .iter()
                .any(|origin| same_origin(origin, url))
    }
}

/// A GET plan for obtaining the login page and its cookies/evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPageRequestPlan {
    pub method: Method,
    pub url: Url,
}

/// A request plan that contains field names and endpoint details but no
/// credential values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginSubmissionRequestPlan {
    UrlEncoded(UrlEncodedLoginRequestPlan),
    Multipart(MultipartLoginRequestPlan),
    TrustedDeviceMultipart(TrustedDeviceLoginRequestPlan),
}

impl LoginSubmissionRequestPlan {
    pub fn method(&self) -> &Method {
        match self {
            Self::UrlEncoded(plan) => &plan.method,
            Self::Multipart(plan) => &plan.method,
            Self::TrustedDeviceMultipart(plan) => &plan.method,
        }
    }

    pub fn url(&self) -> &Url {
        match self {
            Self::UrlEncoded(plan) => &plan.url,
            Self::Multipart(plan) => &plan.url,
            Self::TrustedDeviceMultipart(plan) => &plan.url,
        }
    }

    pub fn content_type(&self) -> &str {
        match self {
            Self::UrlEncoded(plan) => plan.content_type,
            Self::Multipart(plan) => plan.content_type,
            Self::TrustedDeviceMultipart(plan) => plan.content_type,
        }
    }
}

/// A URL encoded login request template.  The password is deliberately not
/// present until [`Self::build_form`] receives a short-lived borrowed input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlEncodedLoginRequestPlan {
    pub method: Method,
    pub url: Url,
    pub content_type: &'static str,
    pub fields: LoginFormFields,
}

impl UrlEncodedLoginRequestPlan {
    pub fn build_form<'a>(
        &self,
        input: LoginFormInput<'a>,
    ) -> Result<UrlEncodedLoginForm<'a>, IdentityClientError> {
        let fields = build_login_fields(&self.fields, input)?;

        Ok(UrlEncodedLoginForm {
            url: self.url.clone(),
            fields,
        })
    }
}

/// A multipart plan intentionally stops before creating a boundary or body.
/// The eventual adapter must decide how its verified password wire value is
/// supplied and let the HTTP transport generate the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartLoginRequestPlan {
    pub method: Method,
    pub url: Url,
    pub content_type: &'static str,
    pub field_names: LoginFormFields,
    pub boundary: MultipartBoundaryPlan,
}

/// The request shape used by the public trusted-device `checkSingle` flow.
/// It is separate from the credential-bearing multipart profile because this
/// route must never receive username or password fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDeviceLoginRequestPlan {
    pub method: Method,
    pub url: Url,
    pub content_type: &'static str,
    pub remember_field: &'static str,
    pub fingerprint_field: String,
    pub generated_fingerprint_field: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultipartBoundaryPlan {
    GeneratedByTransport,
}

impl MultipartLoginRequestPlan {
    pub fn build_form<'a>(
        &self,
        input: LoginFormInput<'a>,
        boundary: impl Into<String>,
    ) -> Result<MultipartLoginForm<'a>, IdentityClientError> {
        let boundary = boundary.into();
        if !valid_multipart_boundary(&boundary) {
            return Err(IdentityClientError::InvalidMultipartBoundary);
        }
        Ok(MultipartLoginForm {
            url: self.url.clone(),
            boundary,
            fields: build_login_fields(&self.field_names, input)?,
        })
    }
}

/// A short-lived multipart login form. Values are borrowed from the caller and
/// the body is created only at the execution boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct MultipartLoginForm<'a> {
    pub url: Url,
    boundary: String,
    pub fields: Vec<UrlEncodedField<'a>>,
}

impl fmt::Debug for MultipartLoginForm<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MultipartLoginForm")
            .field("url", &self.url)
            .field("boundary", &self.boundary)
            .field("fields", &self.fields)
            .finish()
    }
}

impl MultipartLoginForm<'_> {
    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    pub fn content_type(&self) -> String {
        format!("{MULTIPART_CONTENT_TYPE}; boundary={}", self.boundary)
    }

    pub fn body(&self) -> String {
        let mut body = String::new();
        for field in &self.fields {
            body.push_str("--");
            body.push_str(&self.boundary);
            body.push_str("\r\nContent-Disposition: form-data; name=\"");
            body.push_str(&field.name);
            body.push_str("\"\r\n\r\n");
            body.push_str(field.value.wire_value());
            body.push_str("\r\n");
        }
        body.push_str("--");
        body.push_str(&self.boundary);
        body.push_str("--\r\n");
        body
    }

    pub fn encoded_body(&self) -> String {
        self.body()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDeviceRequestPlan {
    pub method: Method,
    pub url: Url,
    pub content_type: &'static str,
    pub fields: TrustedDeviceProfile,
}

impl TrustedDeviceRequestPlan {
    pub fn build_form(
        &self,
        input: TrustedDeviceInput<'_>,
    ) -> Result<UrlEncodedTrustedDeviceForm, IdentityClientError> {
        if input.fingerprint.trim().is_empty() {
            return Err(IdentityClientError::InvalidInput {
                field: "fingerprint",
                message: String::from("fingerprint must not be empty"),
            });
        }
        if input.device_name.trim().is_empty() {
            return Err(IdentityClientError::InvalidInput {
                field: "device_name",
                message: String::from("device name must not be empty"),
            });
        }

        let mut fields = Vec::with_capacity(4);
        fields.push(TrustedDeviceField {
            name: self.fields.fingerprint_field.clone(),
            value: input.fingerprint.to_owned(),
        });
        fields.push(TrustedDeviceField {
            name: self.fields.device_name_field.clone(),
            value: input.device_name.to_owned(),
        });
        fields.push(TrustedDeviceField {
            name: self.fields.decision_field.clone(),
            value: self.fields.decision_value.clone(),
        });

        if let Some(field_name) = self.fields.single_login_field.as_ref() {
            let value = input
                .single_login
                .or(self.fields.single_login_value.as_deref())
                .ok_or(IdentityClientError::InvalidInput {
                    field: "single_login",
                    message: String::from("single-login value is required by this profile"),
                })?;
            fields.push(TrustedDeviceField {
                name: field_name.clone(),
                value: value.to_owned(),
            });
        } else if input.single_login.is_some() {
            return Err(IdentityClientError::InvalidInput {
                field: "single_login",
                message: String::from("the corresponding field is not configured"),
            });
        }

        Ok(UrlEncodedTrustedDeviceForm {
            url: self.url.clone(),
            fields,
        })
    }
}

/// Borrowed values for one saveFinger request. Debug output redacts every
/// value because a fingerprint and a device token are opaque credentials.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TrustedDeviceInput<'a> {
    pub fingerprint: &'a str,
    pub device_name: &'a str,
    pub single_login: Option<&'a str>,
}

impl fmt::Debug for TrustedDeviceInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedDeviceInput")
            .field("fingerprint", &"[redacted]")
            .field("device_name", &"[redacted]")
            .field("single_login", &self.single_login.map(|_| "[redacted]"))
            .finish()
    }
}

impl<'a> TrustedDeviceInput<'a> {
    pub fn new(fingerprint: &'a str, device_name: &'a str) -> Self {
        Self {
            fingerprint,
            device_name,
            single_login: None,
        }
    }

    pub fn with_single_login(mut self, value: &'a str) -> Self {
        self.single_login = Some(value);
        self
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UrlEncodedTrustedDeviceForm {
    pub url: Url,
    pub fields: Vec<TrustedDeviceField>,
}

impl fmt::Debug for UrlEncodedTrustedDeviceForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UrlEncodedTrustedDeviceForm")
            .field("url", &self.url)
            .field("field_count", &self.fields.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TrustedDeviceField {
    pub name: String,
    value: String,
}

impl fmt::Debug for TrustedDeviceField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedDeviceField")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .finish()
    }
}

impl TrustedDeviceField {
    fn wire_value(&self) -> &str {
        &self.value
    }
}

impl UrlEncodedTrustedDeviceForm {
    pub fn content_type(&self) -> &'static str {
        URL_ENCODED_CONTENT_TYPE
    }

    pub fn encoded_body(&self) -> String {
        let mut body = String::new();
        for (index, field) in self.fields.iter().enumerate() {
            if index > 0 {
                body.push('&');
            }
            append_form_component(&mut body, &field.name);
            body.push('=');
            append_form_component(&mut body, field.wire_value());
        }
        body
    }
}

/// Borrowed input for one URL encoded form construction.  The client and the
/// returned form never own the password.  `PrecomputedWire` is deliberately
/// explicit: this module does not assert that an arbitrary value is a valid
/// SM2 result.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LoginFormInput<'a> {
    pub username: &'a str,
    pub password: PasswordInput<'a>,
    /// Hidden inputs copied from the current dynamic login form.  The values
    /// are kept in the Rust-owned bootstrap/session lifetime and never enter
    /// the public bridge.
    pub hidden_fields: Option<&'a [LoginFormHiddenField]>,
    pub device_name: Option<&'a str>,
    pub fingerprint: Option<&'a str>,
    pub generated_fingerprint: Option<&'a str>,
    pub generated_fingerprint_v3: Option<&'a str>,
    pub captcha: Option<&'a str>,
    pub single_login: Option<&'a str>,
}

impl fmt::Debug for LoginFormInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginFormInput")
            .field("username", &self.username)
            .field("password", &self.password)
            .field(
                "hidden_field_count",
                &self.hidden_fields.map_or(0, |fields| fields.len()),
            )
            .field("device_name", &self.device_name.map(|_| "[redacted]"))
            .field("fingerprint", &self.fingerprint.map(|_| "[redacted]"))
            .field(
                "generated_fingerprint",
                &self.generated_fingerprint.map(|_| "[redacted]"),
            )
            .field(
                "generated_fingerprint_v3",
                &self.generated_fingerprint_v3.map(|_| "[redacted]"),
            )
            .field("captcha", &self.captcha.map(|_| "[redacted]"))
            .field("single_login", &self.single_login.map(|_| "[redacted]"))
            .finish()
    }
}

impl<'a> LoginFormInput<'a> {
    pub fn new(username: &'a str, password: PasswordInput<'a>) -> Self {
        Self {
            username,
            password,
            hidden_fields: None,
            device_name: None,
            fingerprint: None,
            generated_fingerprint: None,
            generated_fingerprint_v3: None,
            captcha: None,
            single_login: None,
        }
    }

    pub fn with_fingerprint(mut self, value: &'a str) -> Self {
        self.fingerprint = Some(value);
        self
    }

    pub fn with_hidden_fields(mut self, fields: &'a [LoginFormHiddenField]) -> Self {
        self.hidden_fields = Some(fields);
        self
    }

    pub fn with_device_name(mut self, value: &'a str) -> Self {
        self.device_name = Some(value);
        self
    }

    pub fn with_generated_fingerprint(mut self, value: &'a str) -> Self {
        self.generated_fingerprint = Some(value);
        self
    }

    pub fn with_generated_fingerprint_v3(mut self, value: &'a str) -> Self {
        self.generated_fingerprint_v3 = Some(value);
        self
    }

    pub fn with_captcha(mut self, value: &'a str) -> Self {
        self.captcha = Some(value);
        self
    }

    pub fn with_single_login(mut self, value: &'a str) -> Self {
        self.single_login = Some(value);
        self
    }
}

/// A password value is either caller-provided plaintext (which this client
/// rejects) or a caller-provided wire value produced elsewhere.  Debug output
/// never includes either value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PasswordInput<'a> {
    Plaintext(&'a str),
    PrecomputedWire(&'a str),
}

impl fmt::Debug for PasswordInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Plaintext(_) => "Plaintext",
            Self::PrecomputedWire(_) => "PrecomputedWire",
        };
        formatter.debug_tuple(label).field(&"[redacted]").finish()
    }
}

impl<'a> PasswordInput<'a> {
    pub fn plaintext(value: &'a str) -> Self {
        Self::Plaintext(value)
    }

    pub fn precomputed_wire(value: &'a str) -> Self {
        Self::PrecomputedWire(value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FormValue<'a> {
    Text(&'a str),
    Sensitive(&'a str),
}

impl fmt::Debug for FormValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => formatter.debug_tuple("Text").field(value).finish(),
            Self::Sensitive(_) => formatter
                .debug_tuple("Sensitive")
                .field(&"[redacted]")
                .finish(),
        }
    }
}

impl FormValue<'_> {
    pub fn wire_value(&self) -> &str {
        match self {
            Self::Text(value) | Self::Sensitive(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlEncodedField<'a> {
    pub name: String,
    pub value: FormValue<'a>,
}

/// A short-lived URL encoded form.  Its values are borrowed from the caller;
/// it cannot outlive the input that supplied the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlEncodedLoginForm<'a> {
    pub url: Url,
    pub fields: Vec<UrlEncodedField<'a>>,
}

impl UrlEncodedLoginForm<'_> {
    pub fn content_type(&self) -> &'static str {
        URL_ENCODED_CONTENT_TYPE
    }

    pub fn encoded_body(&self) -> String {
        encode_fields(&self.fields)
    }
}

/// A short-lived URL encoded second-factor form.  Verification codes remain
/// borrowed and sensitive values are redacted by the field debug formatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlEncodedSecondAuthForm<'a> {
    pub url: Url,
    pub fields: Vec<UrlEncodedField<'a>>,
}

impl UrlEncodedSecondAuthForm<'_> {
    pub fn content_type(&self) -> &'static str {
        URL_ENCODED_CONTENT_TYPE
    }

    pub fn encoded_body(&self) -> String {
        let mut body = String::new();
        for (index, field) in self.fields.iter().enumerate() {
            if index > 0 {
                body.push('&');
            }
            append_form_component(&mut body, &field.name);
            body.push('=');
            append_form_component(&mut body, field.value.wire_value());
        }
        body
    }
}

fn append_optional_field<'a>(
    fields: &mut Vec<UrlEncodedField<'a>>,
    configured_name: &Option<String>,
    value: Option<&'a str>,
    input_name: &'static str,
) -> Result<(), IdentityClientError> {
    let Some(value) = value else {
        return Ok(());
    };
    let Some(configured_name) = configured_name else {
        return Err(IdentityClientError::InvalidInput {
            field: input_name,
            message: String::from("the corresponding field is not configured"),
        });
    };
    fields.push(UrlEncodedField {
        name: configured_name.clone(),
        value: FormValue::Sensitive(value),
    });
    Ok(())
}

fn build_login_fields<'a>(
    configured: &LoginFormFields,
    input: LoginFormInput<'a>,
) -> Result<Vec<UrlEncodedField<'a>>, IdentityClientError> {
    if input.username.trim().is_empty() {
        return Err(IdentityClientError::InvalidInput {
            field: "username",
            message: String::from("username must not be empty"),
        });
    }

    let password = match input.password {
        PasswordInput::Plaintext(_) => {
            return Err(IdentityClientError::UnsupportedCrypto {
                algorithm: String::from("SM2"),
                reason: String::from(
                    "the identity profile exposes an SM2 public key, but this crate does not implement verified SM2 password encoding; provide a verified wire value from a dedicated crypto adapter",
                ),
            });
        }
        PasswordInput::PrecomputedWire(value) => {
            if value.is_empty() {
                return Err(IdentityClientError::InvalidInput {
                    field: "password",
                    message: String::from("the precomputed wire value must not be empty"),
                });
            }
            value
        }
    };

    let mut fields =
        Vec::with_capacity(2 + 6 + input.hidden_fields.map_or(0, <[LoginFormHiddenField]>::len));
    if let Some(hidden_fields) = input.hidden_fields {
        let managed_names = [
            Some(configured.username_field.as_str()),
            Some(configured.password_field.as_str()),
            configured.device_name_field.as_deref(),
            configured.fingerprint_field.as_deref(),
            configured.generated_fingerprint_field.as_deref(),
            configured.generated_fingerprint_v3_field.as_deref(),
            configured.captcha_field.as_deref(),
            configured.single_login_field.as_deref(),
        ];
        for hidden in hidden_fields {
            let name = hidden.name();
            if name.trim().is_empty()
                || name.chars().any(char::is_control)
                || hidden.value().chars().any(char::is_control)
            {
                return Err(IdentityClientError::InvalidInput {
                    field: "hidden_field",
                    message: String::from("dynamic hidden field contains invalid characters"),
                });
            }
            if managed_names
                .into_iter()
                .flatten()
                .any(|managed| managed == name)
                || fields
                    .iter()
                    .any(|field: &UrlEncodedField<'_>| field.name == name)
            {
                continue;
            }
            fields.push(UrlEncodedField {
                name: name.to_owned(),
                value: FormValue::Sensitive(hidden.value()),
            });
        }
    }
    fields.push(UrlEncodedField {
        name: configured.username_field.clone(),
        value: FormValue::Text(input.username),
    });
    fields.push(UrlEncodedField {
        name: configured.password_field.clone(),
        value: FormValue::Sensitive(password),
    });
    append_optional_field(
        &mut fields,
        &configured.single_login_field,
        input.single_login,
        "single_login",
    )?;
    append_optional_field(
        &mut fields,
        &configured.fingerprint_field,
        input.fingerprint,
        "fingerprint",
    )?;
    append_optional_field(
        &mut fields,
        &configured.generated_fingerprint_field,
        input.generated_fingerprint,
        "generated_fingerprint",
    )?;
    append_optional_field(
        &mut fields,
        &configured.generated_fingerprint_v3_field,
        input.generated_fingerprint_v3,
        "generated_fingerprint_v3",
    )?;
    append_optional_field(
        &mut fields,
        &configured.device_name_field,
        input.device_name,
        "device_name",
    )?;
    append_optional_field(
        &mut fields,
        &configured.captcha_field,
        input.captcha,
        "captcha",
    )?;
    Ok(fields)
}

fn build_check_single_login_fields<'a>(
    configured: &LoginFormFields,
    plan: &TrustedDeviceLoginRequestPlan,
    input: LoginFormInput<'a>,
) -> Result<Vec<UrlEncodedField<'a>>, IdentityClientError> {
    let fingerprint = input
        .fingerprint
        .filter(|value| !value.trim().is_empty())
        .ok_or(IdentityClientError::InvalidInput {
            field: "fingerprint",
            message: String::from("checkSingle login requires a fingerprint"),
        })?;
    let generated_fingerprint = input.generated_fingerprint.unwrap_or_default();

    let managed_names = [
        Some(configured.username_field.as_str()),
        Some(configured.password_field.as_str()),
        configured.device_name_field.as_deref(),
        configured.fingerprint_field.as_deref(),
        configured.generated_fingerprint_field.as_deref(),
        configured.generated_fingerprint_v3_field.as_deref(),
        configured.captcha_field.as_deref(),
        configured.single_login_field.as_deref(),
        Some(plan.remember_field),
    ];
    let mut fields =
        Vec::with_capacity(3 + input.hidden_fields.map_or(0, <[LoginFormHiddenField]>::len));
    if let Some(hidden_fields) = input.hidden_fields {
        for hidden in hidden_fields {
            let name = hidden.name();
            if name.trim().is_empty()
                || name.chars().any(char::is_control)
                || hidden.value().chars().any(char::is_control)
            {
                return Err(IdentityClientError::InvalidInput {
                    field: "hidden_field",
                    message: String::from("dynamic hidden field contains invalid characters"),
                });
            }
            if managed_names
                .into_iter()
                .flatten()
                .any(|managed| managed == name)
                || fields
                    .iter()
                    .any(|field: &UrlEncodedField<'_>| field.name == name)
            {
                continue;
            }
            fields.push(UrlEncodedField {
                name: name.to_owned(),
                value: FormValue::Sensitive(hidden.value()),
            });
        }
    }

    fields.push(UrlEncodedField {
        name: plan.remember_field.to_owned(),
        value: FormValue::Text(CHECK_SINGLE_REMEMBER_VALUE),
    });
    fields.push(UrlEncodedField {
        name: plan.fingerprint_field.clone(),
        value: FormValue::Sensitive(fingerprint),
    });
    fields.push(UrlEncodedField {
        name: plan.generated_fingerprint_field.clone(),
        value: FormValue::Sensitive(generated_fingerprint),
    });
    Ok(fields)
}

fn encode_fields(fields: &[UrlEncodedField<'_>]) -> String {
    let mut body = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            body.push('&');
        }
        append_form_component(&mut body, &field.name);
        body.push('=');
        append_form_component(&mut body, field.value.wire_value());
    }
    body
}

fn valid_multipart_boundary(boundary: &str) -> bool {
    !boundary.is_empty()
        && boundary.len() <= 70
        && boundary.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'\''
                        | b'('
                        | b')'
                        | b'+'
                        | b'_'
                        | b','
                        | b'-'
                        | b'.'
                        | b'/'
                        | b':'
                        | b'='
                        | b'?'
                )
        })
}

fn default_names<const N: usize>(names: [&str; N]) -> Vec<String> {
    names.into_iter().map(String::from).collect()
}

fn validate_field_name(name: &str, field: &'static str) -> Result<(), IdentityClientError> {
    if name.trim().is_empty() {
        return Err(IdentityClientError::InvalidConfig {
            message: format!("{field} must not be empty"),
        });
    }
    if name
        .bytes()
        .any(|byte| byte.is_ascii_control() || matches!(byte, b'"' | b'\r' | b'\n'))
    {
        return Err(IdentityClientError::InvalidConfig {
            message: format!("{field} contains an invalid header character"),
        });
    }
    Ok(())
}

fn validate_route_template(route: &str, field: &'static str) -> Result<(), IdentityClientError> {
    if route.trim().is_empty() {
        return Err(IdentityClientError::InvalidConfig {
            message: format!("{field} must not be empty"),
        });
    }
    if route.contains("{app_id}") {
        return Err(IdentityClientError::InvalidConfig {
            message: format!("{field} must use {APP_ID_PLACEHOLDER}, not {{app_id}}"),
        });
    }
    Ok(())
}

fn redacted_url(url: &Url) -> String {
    let mut redacted = url.clone();
    redacted.set_query(None);
    redacted.set_fragment(None);
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted.to_string()
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
        && left.username().is_empty()
        && left.password().is_none()
        && right.username().is_empty()
        && right.password().is_none()
}

fn same_route(left: &Url, right: &Url) -> bool {
    same_origin(left, right)
        && left.path().trim_end_matches('/') == right.path().trim_end_matches('/')
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if is_unreserved(*byte) {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

fn append_form_component(output: &mut String, value: &str) {
    for byte in value.as_bytes() {
        if is_unreserved(*byte) {
            output.push(*byte as char);
        } else if *byte == b' ' {
            output.push('+');
        } else {
            output.push('%');
            output.push(hex_digit(byte >> 4));
            output.push(hex_digit(byte & 0x0f));
        }
    }
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + value - 10) as char,
        _ => unreachable!("a hexadecimal digit is four bits wide"),
    }
}

#[derive(Debug, Default)]
struct ParsedHtml {
    tags: Vec<HtmlTag>,
    visible_text: String,
    scripts: Vec<String>,
}

#[derive(Default)]
struct ParsedForm {
    action: Option<String>,
    method: Option<String>,
    inputs: Vec<HtmlTag>,
}

#[derive(Clone, Debug)]
struct HtmlTag {
    name: String,
    attributes: Vec<HtmlAttribute>,
    text: Option<String>,
    // Only presentation text is hidden; explicit protocol inputs still count.
    hidden: bool,
}

#[derive(Clone, Debug)]
struct HtmlAttribute {
    name: String,
    value: Option<String>,
}

fn element_is_hidden(tag: &HtmlTag) -> bool {
    if tag.attributes.iter().any(|a| a.name == "hidden") {
        return true;
    }
    if tag.attribute_value("class").is_some_and(|value| {
        value
            .split_ascii_whitespace()
            .any(|v| matches!(v, "hidden" | "d-none"))
    }) {
        return true;
    }
    tag.attribute_value("style").is_some_and(|style| {
        style.split(';').any(|declaration| {
            let Some((name, value)) = declaration.split_once(':') else {
                return false;
            };
            let value = value.split('!').next().unwrap_or_default().trim();
            (name.trim().eq_ignore_ascii_case("display") && value.eq_ignore_ascii_case("none"))
                || (name.trim().eq_ignore_ascii_case("visibility")
                    && value.eq_ignore_ascii_case("hidden"))
        })
    })
}

/// Structural classification for an already allowlisted service resource.
/// Inert script strings, help links and hidden form templates are not a
/// server rejection. This returns no page text or credential material.
pub(crate) fn service_resource_document_requires_login(html: &str) -> bool {
    let parsed = parse_html(html);
    let visible: Vec<HtmlTag> = parsed
        .tags
        .into_iter()
        .filter(|tag| {
            !tag.hidden
                && !(tag.name == "input"
                    && tag
                        .attribute_value("type")
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("hidden")))
        })
        .collect();
    if has_login_form(
        &visible,
        &LoginFormFields::common(),
        "/do/off/ui/auth/login/check",
    ) {
        return true;
    }
    let login_action = visible.iter().any(|tag| {
        if tag.name != "form" {
            return false;
        }
        let Some(action) = tag.attribute_value("action") else {
            return false;
        };
        let path = Url::parse(&action)
            .ok()
            .map(|url| url.path().to_owned())
            .unwrap_or_else(|| {
                action
                    .split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            });
        matches!(
            path.trim_end_matches('/'),
            "/login" | "/f/login" | "/security_check"
        ) || path.ends_with("/do/off/ui/auth/login/check")
            || path.ends_with("/do/off/ui/auth/login/checkSingle")
    });
    if login_action {
        return true;
    }
    let notice = parsed
        .visible_text
        .trim()
        .trim_matches(['.', '!', '。', '！'])
        .trim()
        .to_ascii_lowercase();
    matches!(
        notice.as_str(),
        "请登录"
            | "请先登录"
            | "未登录"
            | "登录失效"
            | "会话已过期"
            | "login required"
            | "please login"
            | "please log in"
            | "session expired"
            | "not logged in"
            | "authentication required"
            | "unauthorized"
    )
}

fn parse_html(html: &str) -> ParsedHtml {
    let mut parsed = ParsedHtml::default();
    let bytes = html.as_bytes();
    let mut cursor = 0;
    let mut text_start = 0;
    let mut visibility: Vec<(String, bool)> = Vec::new();

    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }

        if text_start < cursor && !visibility.last().is_some_and(|(_, hidden)| *hidden) {
            parsed
                .visible_text
                .push_str(&html_unescape(&html[text_start..cursor]));
            parsed.visible_text.push(' ');
        }

        if html[cursor..].starts_with("<!--") {
            let comment_end = html[cursor + 4..]
                .find("-->")
                .map(|offset| cursor + 4 + offset + 3)
                .unwrap_or(html.len());
            cursor = comment_end;
            text_start = cursor;
            continue;
        }

        let Some(tag_end) = find_tag_end(bytes, cursor + 1) else {
            break;
        };
        let raw_tag = &html[cursor + 1..tag_end];
        if let Some(closing) = raw_tag.trim().strip_prefix('/') {
            let name = closing.split_ascii_whitespace().next().unwrap_or_default();
            if let Some(index) = visibility
                .iter()
                .rposition(|(open, _)| open.eq_ignore_ascii_case(name))
            {
                visibility.truncate(index);
            }
            cursor = tag_end + 1;
            text_start = cursor;
            continue;
        }
        if let Some(mut tag) = parse_start_tag(raw_tag) {
            tag.hidden =
                visibility.last().is_some_and(|(_, hidden)| *hidden) || element_is_hidden(&tag);
            let is_script = tag.name == "script";
            let is_style = tag.name == "style";
            if !is_script && !is_style && tag.attribute_value("id").is_some() {
                if let Some(close_start) = find_closing_tag(html, tag_end + 1, &tag.name) {
                    tag.text = Some(extract_visible_text(&html[tag_end + 1..close_start]));
                }
            }
            if !is_script
                && !is_style
                && !raw_tag.trim_end().ends_with('/')
                && !matches!(
                    tag.name.as_str(),
                    "area"
                        | "base"
                        | "br"
                        | "col"
                        | "embed"
                        | "hr"
                        | "img"
                        | "input"
                        | "link"
                        | "meta"
                        | "param"
                        | "source"
                        | "track"
                        | "wbr"
                )
            {
                visibility.push((tag.name.clone(), tag.hidden));
            }
            parsed.tags.push(tag);
            cursor = tag_end + 1;

            if is_script || is_style {
                let closing_name = if is_script { "script" } else { "style" };
                if let Some(close_start) = find_closing_tag(html, cursor, closing_name) {
                    if is_script {
                        parsed.scripts.push(html[cursor..close_start].to_owned());
                    }
                    let close_end = find_tag_end(bytes, close_start + 2)
                        .map(|index| index + 1)
                        .unwrap_or(html.len());
                    cursor = close_end;
                    text_start = cursor;
                    continue;
                }
            }
        } else {
            cursor = tag_end + 1;
        }
        text_start = cursor;
    }

    if text_start < html.len() && !visibility.last().is_some_and(|(_, hidden)| *hidden) {
        parsed
            .visible_text
            .push_str(&html_unescape(&html[text_start..]));
    }
    parsed.visible_text = collapse_whitespace(&parsed.visible_text);
    parsed
}

fn parse_forms(html: &str) -> Vec<ParsedForm> {
    let mut forms = Vec::new();
    let mut active = None;
    let bytes = html.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(relative_start) = html[cursor..].find('<') else {
            break;
        };
        cursor += relative_start;
        let Some(tag_end) = find_tag_end(bytes, cursor + 1) else {
            break;
        };
        let raw_tag = &html[cursor + 1..tag_end];
        let trimmed = raw_tag.trim();
        if let Some(closing) = trimmed.strip_prefix('/') {
            let name = closing
                .trim()
                .split_ascii_whitespace()
                .next()
                .unwrap_or_default();
            if name.eq_ignore_ascii_case("form") {
                if let Some(form) = active.take() {
                    forms.push(form);
                }
            }
            cursor = tag_end + 1;
            continue;
        }

        let Some(tag) = parse_start_tag(raw_tag) else {
            cursor = tag_end + 1;
            continue;
        };
        if tag.name == "script" || tag.name == "style" {
            let closing_name = tag.name.clone();
            if let Some(close_start) = find_closing_tag(html, tag_end + 1, &closing_name) {
                cursor = find_tag_end(bytes, close_start + 2)
                    .map(|index| index + 1)
                    .unwrap_or(html.len());
                continue;
            }
        }
        match tag.name.as_str() {
            "form" => {
                if let Some(form) = active.take() {
                    forms.push(form);
                }
                active = Some(ParsedForm {
                    action: tag.attribute_value("action"),
                    method: tag.attribute_value("method"),
                    inputs: Vec::new(),
                });
            }
            "input" => {
                if let Some(form) = active.as_mut() {
                    form.inputs.push(tag);
                }
            }
            _ => {}
        }
        cursor = tag_end + 1;
    }
    if let Some(form) = active {
        forms.push(form);
    }
    forms
}

fn resolve_untrusted_url(base_url: &Url, raw: &str) -> Option<Url> {
    if raw.trim().is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    let url = base_url.join(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

fn hidden_form_ticket(inputs: &[HtmlTag], query_names: &[String]) -> Option<String> {
    inputs.iter().find_map(|input| {
        if input
            .attribute_value("type")
            .is_none_or(|value| !value.eq_ignore_ascii_case("hidden"))
        {
            return None;
        }
        let name = input.attribute_value("name")?;
        if !query_names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&name))
        {
            return None;
        }
        input.attribute_value("value")
    })
}

fn is_webvpn_learn_handoff_path(path: &str) -> bool {
    is_webvpn_target_path(path, WEBVPN_LEARN_HANDOFF_TARGETS)
}

fn is_webvpn_identity_handoff_path(path: &str) -> bool {
    is_webvpn_target_path(path, WEBVPN_IDENTITY_HANDOFF_TARGETS)
}

fn is_webvpn_target_path(path: &str, targets: &[&str]) -> bool {
    let Some((_, rest)) = split_webvpn_wrapper_path(path) else {
        return false;
    };
    // OAuth `lbredirect` currently emits the wrapper as
    // `/https/<mapping>%2F<target with %2F separators>`, while links
    // rendered by older pages use literal slashes. Decode only the path
    // separator marker here; the mapping and target are still compared
    // against strict allowlists below.
    let normalized = rest.replace("%2F", "/").replace("%2f", "/");
    let Some((mapping, target)) = normalized.split_once('/') else {
        return false;
    };
    !mapping.is_empty()
        && mapping.len() <= MAX_ANCHOR_TICKET_LENGTH
        && mapping != "."
        && mapping != ".."
        && !mapping.chars().any(char::is_control)
        && targets.contains(&target)
}

fn is_webvpn_root_handoff_path(path: &str) -> bool {
    let Some((_, rest)) = split_webvpn_wrapper_path(path) else {
        return false;
    };

    // Url keeps the encoded slash in the path. Accept an already-decoded
    // trailing slash as well because different HTTP stacks expose the same
    // Location header differently, but never accept another slash inside the
    // opaque mapping segment.
    let mapping = rest
        .strip_suffix("%2F")
        .or_else(|| rest.strip_suffix("%2f"))
        .or_else(|| rest.strip_suffix('/'))
        .unwrap_or_default();
    !mapping.is_empty()
        && !mapping.contains('/')
        && mapping.len() <= MAX_ANCHOR_TICKET_LENGTH
        && mapping
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
}

/// Split a WebVPN wrapper into its transport prefix and opaque payload.
///
/// The public client derives this prefix from the source URL, so it can be
/// `http`, `https`, `http-80`, `https-443`, or an explicit port such as
/// `http-8080`. Keep the prefix parser narrow: accepting arbitrary
/// `/http-*` or `/https-*` paths would turn unrelated WebVPN pages into
/// authentication continuations.
fn split_webvpn_wrapper_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix('/')?;
    let (prefix, payload) = rest.split_once('/')?;
    if !is_webvpn_wrapper_prefix(prefix) {
        return None;
    }
    Some((prefix, payload))
}

fn is_webvpn_wrapper_prefix(prefix: &str) -> bool {
    if WEBVPN_WRAPPER_PREFIXES.contains(&prefix) {
        return true;
    }

    let Some((scheme, port)) = prefix.split_once('-') else {
        return false;
    };
    if !matches!(scheme, "http" | "https")
        || port.is_empty()
        || port.len() > 5
        || !port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    port.parse::<u16>().is_ok_and(|value| value != 0)
}

fn find_tag_end(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    let mut quote = None;
    while cursor < bytes.len() {
        match (quote, bytes[cursor]) {
            (None, b'\'' | b'"') => quote = Some(bytes[cursor]),
            (Some(current), byte) if byte == current => quote = None,
            (None, b'>') => return Some(cursor),
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn parse_start_tag(raw_tag: &str) -> Option<HtmlTag> {
    let raw_tag = raw_tag.trim();
    if raw_tag.is_empty()
        || raw_tag.starts_with('/')
        || raw_tag.starts_with('!')
        || raw_tag.starts_with('?')
    {
        return None;
    }

    let name_end = raw_tag
        .char_indices()
        .find(|(_, character)| character.is_ascii_whitespace() || *character == '/')
        .map(|(index, _)| index)
        .unwrap_or(raw_tag.len());
    let name = raw_tag[..name_end].to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let mut cursor = name_end;
    let mut attributes = Vec::new();

    while cursor < raw_tag.len() {
        skip_ascii_whitespace(raw_tag, &mut cursor);
        while raw_tag.as_bytes().get(cursor) == Some(&b'/') {
            cursor += 1;
            skip_ascii_whitespace(raw_tag, &mut cursor);
        }
        if cursor >= raw_tag.len() {
            break;
        }

        let attribute_start = cursor;
        while cursor < raw_tag.len()
            && !raw_tag.as_bytes()[cursor].is_ascii_whitespace()
            && !matches!(raw_tag.as_bytes()[cursor], b'=' | b'/')
        {
            cursor += 1;
        }
        if attribute_start == cursor {
            cursor += 1;
            continue;
        }
        let name = raw_tag[attribute_start..cursor].to_ascii_lowercase();
        skip_ascii_whitespace(raw_tag, &mut cursor);

        let value = if raw_tag.as_bytes().get(cursor) == Some(&b'=') {
            cursor += 1;
            skip_ascii_whitespace(raw_tag, &mut cursor);
            if let Some(quote @ (b'\'' | b'"')) = raw_tag.as_bytes().get(cursor).copied() {
                cursor += 1;
                let value_start = cursor;
                while cursor < raw_tag.len() && raw_tag.as_bytes()[cursor] != quote {
                    cursor += 1;
                }
                let value = html_unescape(&raw_tag[value_start..cursor]);
                if cursor < raw_tag.len() {
                    cursor += 1;
                }
                Some(value)
            } else {
                let value_start = cursor;
                while cursor < raw_tag.len() && !raw_tag.as_bytes()[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                Some(html_unescape(&raw_tag[value_start..cursor]))
            }
        } else {
            None
        };
        attributes.push(HtmlAttribute { name, value });
    }

    Some(HtmlTag {
        name,
        attributes,
        text: None,
        hidden: false,
    })
}

fn skip_ascii_whitespace(value: &str, cursor: &mut usize) {
    while *cursor < value.len() && value.as_bytes()[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

fn find_closing_tag(html: &str, start: usize, name: &str) -> Option<usize> {
    let lower = html[start..].to_ascii_lowercase();
    let needle = format!("</{name}");
    lower.find(&needle).map(|offset| start + offset)
}

fn html_unescape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find('&') {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start..].find(';') else {
            output.push_str(&value[start..]);
            return output;
        };
        let end = start + relative_end;
        let entity = &value[start + 1..end];
        if let Some(decoded) = decode_html_entity(entity) {
            output.push(decoded);
        } else {
            output.push_str(&value[start..=end]);
        }
        cursor = end + 1;
    }
    output.push_str(&value[cursor..]);
    output
}

fn extract_visible_text(value: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('<') else {
            output.push_str(&html_unescape(&value[cursor..]));
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&html_unescape(&value[cursor..start]));
        let Some(relative_end) = value[start..].find('>') else {
            break;
        };
        cursor = start + relative_end + 1;
    }
    collapse_whitespace(&output)
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ if entity
            .strip_prefix("#x")
            .or_else(|| entity.strip_prefix("#X"))
            .is_some() =>
        {
            let digits = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))?;
            u32::from_str_radix(digits, 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn collapse_whitespace(value: &str) -> String {
    let mut output = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            output.push(character);
            pending_space = false;
        }
    }
    output
}

fn find_sm2_public_key(
    tags: &[HtmlTag],
    scripts: &[String],
    configured_names: &[String],
) -> Option<Sm2PublicKey> {
    for tag in tags {
        if !matches!(tag.name.as_str(), "input" | "meta" | "textarea") {
            continue;
        }
        let Some(field_name) = matching_field_name(tag, configured_names) else {
            continue;
        };
        let value = tag
            .attribute_value("value")
            .or_else(|| tag.attribute_value("content"))
            .or_else(|| tag.attribute_value("data-value"))
            .filter(|value| !value.trim().is_empty())?;
        return Some(Sm2PublicKey {
            field_name,
            value: value.trim().to_owned(),
        });
    }

    for tag in tags {
        let Some(field_name) = matching_field_name(tag, configured_names) else {
            continue;
        };
        let Some(value) = tag.text.as_deref().filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        return Some(Sm2PublicKey {
            field_name,
            value: value.trim().to_owned(),
        });
    }

    for script in scripts {
        for configured_name in configured_names {
            if let Some(value) = find_script_assignment(script, configured_name) {
                return Some(Sm2PublicKey {
                    field_name: configured_name.clone(),
                    value,
                });
            }
        }
    }
    None
}

fn find_anchor_ticket(
    base_url: &Url,
    tags: &[HtmlTag],
    query_names: &[String],
    path_prefixes: &[String],
    allowed_origins: &[Url],
) -> Option<AnchorTicket> {
    // Prefer the identity callback over a generic public handoff anchor. A
    // success template can contain more than one allowed link, and the
    // callback is the deployment-defined continuation that creates the final
    // ticket.  A public anchor is retained as a fallback for deployments that
    // omit that callback and expose the next hop directly.
    let mut callback_candidate = None;
    let mut public_candidate = None;
    for tag in tags {
        if tag.name != "a" {
            continue;
        }
        let Some(href) = tag.attribute_value("href") else {
            continue;
        };
        let Some(url) = resolve_anchor_url(base_url, &href, path_prefixes, allowed_origins) else {
            continue;
        };
        let ticket_parameter = query_parameter(url.query().unwrap_or_default(), query_names);
        if let Some((_, ticket)) = ticket_parameter {
            // A malformed ticket must never be downgraded to a followable
            // callback.  Doing so would let an arbitrary URL with a bad
            // ticket become the next request in the chain.
            let Some(ticket) = ticket.filter(|value| valid_anchor_ticket(value)) else {
                continue;
            };
            return Some(AnchorTicket {
                href: url.to_string(),
                ticket: Some(ticket),
            });
        }

        let candidate = AnchorTicket {
            href: url.to_string(),
            ticket: None,
        };
        if is_redirect_callback_path(url.path()) {
            if callback_candidate.is_none() {
                callback_candidate = Some(candidate);
            }
        } else if public_candidate.is_none() {
            public_candidate = Some(candidate);
        }
    }
    callback_candidate.or(public_candidate)
}

/// Chooses the strongest handoff evidence while preserving document/source
/// order among candidates of the same strength.
///
/// A ticket is the strongest evidence. If no ticket exists, an identity
/// callback is more useful than a generic ticketless Cookie route because the
/// callback is the next step that can establish the service session. The
/// final fallback keeps the first safe ticketless candidate for deployments
/// that use a different, explicitly allowlisted handoff route.
fn select_preferred_handoff(
    candidates: impl IntoIterator<Item = Option<AnchorTicket>>,
) -> Option<AnchorTicket> {
    let mut best: Option<(u8, AnchorTicket)> = None;
    for candidate in candidates.into_iter().flatten() {
        let rank = handoff_candidate_rank(&candidate);
        if rank == 3 {
            return Some(candidate);
        }
        if best.as_ref().is_none_or(|(best_rank, _)| rank > *best_rank) {
            best = Some((rank, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

fn handoff_candidate_rank(candidate: &AnchorTicket) -> u8 {
    if candidate.ticket.is_some() {
        return 3;
    }
    Url::parse(&candidate.href)
        .ok()
        .map(|url| {
            if is_redirect_callback_path(url.path()) {
                2
            } else {
                1
            }
        })
        .unwrap_or(1)
}

fn resolve_anchor_url(
    base_url: &Url,
    href: &str,
    path_prefixes: &[String],
    allowed_origins: &[Url],
) -> Option<Url> {
    // `Url::join("")` returns the current document URL. An empty success
    // template link is therefore not a continuation and must never become a
    // callback candidate or a same-URL redirect loop.
    if href.trim().is_empty() || href.chars().any(char::is_control) {
        return None;
    }
    let url = base_url.join(href).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !allowed_origins
            .iter()
            .any(|origin| same_origin(origin, &url))
        || !allowed_anchor_path(url.path(), path_prefixes)
        || !query_is_well_formed(url.query().unwrap_or_default())
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

fn origin_url(url: &Url) -> Url {
    let mut origin = url.clone();
    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);
    let _ = origin.set_username("");
    let _ = origin.set_password(None);
    origin
}

fn allowed_anchor_path(path: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|prefix| {
        let prefix = prefix.trim_end_matches('/');
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|remainder| remainder.starts_with('/'))
    })
}

/// A cookie-backed route configured as `/` means the WebVPN root handoff.
/// Treating it as a normal prefix would silently allow every path on that
/// origin, including unrelated pages. Known WebVPN wrapper paths remain
/// separately allowlisted by `webvpn_wrapper_path_allowed`.
fn allowed_cookie_handoff_path(path: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|prefix| {
        if prefix == "/" {
            path == "/"
        } else {
            allowed_anchor_path(path, std::slice::from_ref(prefix))
        }
    })
}

fn is_redirect_callback_path(path: &str) -> bool {
    path == IDENTITY_CALLBACK_PATH
        || path
            .strip_suffix('/')
            .is_some_and(|without_trailing_slash| without_trailing_slash == IDENTITY_CALLBACK_PATH)
}

fn has_explicit_success_marker(html: &str) -> bool {
    let visible_text = parse_html(html).visible_text.to_ascii_lowercase();
    let source = html.to_ascii_lowercase();
    // "redirecting" by itself is common on error/login pages. Require a
    // deployment-specific success phrase; the redirect wording is optional.
    //
    // The identity service has used both ordinary text and template/script
    // fields for this message. The classifier calls this helper only after
    // it has constrained the response to the profiled login/check route and
    // has already rejected explicit failure evidence, so looking at the
    // source as well as visible text does not turn an arbitrary page into a
    // login success. It does, however, preserve the same success evidence
    // that current public clients use when the message is rendered by a
    // script-backed template.
    [
        "登录成功",
        "认证成功",
        "身份认证成功",
        "验证成功",
        "login success",
        "login successful",
        "authentication successful",
        "authentication succeeded",
        "logged in successfully",
    ]
    .into_iter()
    .any(|marker| visible_text.contains(marker) || source.contains(marker))
}

fn has_cookie_handoff_proof(page: &LoginPageEvidence) -> bool {
    // The downstream Learn and WebVPN-backed pages expose the session proof
    // as a CSRF token. A fetched allowlisted page without that evidence is
    // still only an arbitrary HTTP 200 response.
    page.csrf.is_some()
}

fn valid_anchor_ticket(ticket: &str) -> bool {
    !ticket.trim().is_empty()
        && ticket.len() <= MAX_ANCHOR_TICKET_LENGTH
        && !ticket.chars().any(char::is_control)
}

fn find_csrf(tags: &[HtmlTag], field_names: &[String]) -> Option<CsrfEvidence> {
    for tag in tags {
        let Some(field_name) = matching_field_name(tag, field_names) else {
            continue;
        };
        let Some(value) = tag
            .attribute_value("value")
            .or_else(|| tag.attribute_value("content"))
            .or_else(|| tag.attribute_value("data-token"))
        else {
            continue;
        };
        let value = value.trim();
        let token = CsrfToken::new(value.to_owned())?;
        return Some(CsrfEvidence { field_name, token });
    }
    None
}

fn find_invalidation(tags: &[HtmlTag], field_names: &[String]) -> InvalidationMarker {
    for tag in tags {
        let Some(field_name) = matching_field_name(tag, field_names) else {
            continue;
        };
        let raw_value = tag
            .attribute_value("value")
            .or_else(|| tag.attribute_value("content"));
        let status = match raw_value.as_deref().map(str::trim) {
            Some(value) if is_positive_marker(value) => InvalidationStatus::Marked,
            Some(value) if is_negative_marker(value) => InvalidationStatus::Unknown,
            _ => InvalidationStatus::Unknown,
        };
        return InvalidationMarker {
            status,
            field_name: Some(field_name),
            raw_value,
        };
    }
    InvalidationMarker::not_found()
}

fn is_positive_marker(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "invalid" | "expired" | "fail" | "failed" | "error"
    )
}

fn is_negative_marker(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "false" | "0" | "no" | "valid" | "ok"
    )
}

fn find_failure_marker(
    tags: &[HtmlTag],
    visible_text: &str,
    field_markers: &[LoginFailureMarker],
    text_markers: &[LoginFailureTextMarker],
) -> Option<LoginFailureEvidence> {
    for marker in field_markers {
        for tag in tags {
            let Some(actual_name) =
                matching_field_name(tag, std::slice::from_ref(&marker.field_name))
            else {
                continue;
            };
            let value = tag
                .attribute_value("value")
                .or_else(|| tag.attribute_value("content"))
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    if tag.hidden {
                        None
                    } else {
                        tag.text.clone().filter(|v| !v.trim().is_empty())
                    }
                });
            // Templates contain empty errorMsg/loginError placeholders.
            // Presence is not failure. Explicit nonempty hidden protocol
            // inputs and true invalidation markers remain authoritative.
            if marker.accepted_values.is_empty()
                && (value.is_none()
                    || value
                        .as_deref()
                        .is_some_and(|v| is_negative_marker(v.trim())))
            {
                continue;
            }
            if marker.matches(value.as_deref()) {
                let reason = if marker.reason == LoginFailureReason::Generic {
                    let text = value.as_deref().unwrap_or_default().to_ascii_lowercase();
                    text_markers
                        .iter()
                        .find(|text_marker| text.contains(&text_marker.text.to_ascii_lowercase()))
                        .map_or(LoginFailureReason::Generic, |text_marker| {
                            text_marker.reason
                        })
                } else {
                    marker.reason
                };
                return Some(LoginFailureEvidence {
                    reason,
                    source: FailureEvidenceSource::Field,
                    marker: actual_name,
                    raw_value: value,
                });
            }
        }
    }

    let visible_text_lower = visible_text.to_ascii_lowercase();
    for marker in text_markers {
        if visible_text_lower.contains(&marker.text.to_ascii_lowercase()) {
            return Some(LoginFailureEvidence {
                reason: marker.reason,
                source: FailureEvidenceSource::Text,
                marker: marker.text.clone(),
                raw_value: None,
            });
        }
    }
    None
}

fn find_second_factor_marker(
    tags: &[HtmlTag],
    visible_text: &str,
    field_names: &[String],
    text_markers: &[String],
) -> Option<String> {
    for tag in tags {
        if let Some(field_name) = matching_field_name(tag, field_names) {
            return Some(field_name);
        }
    }
    let visible_text_lower = visible_text.to_ascii_lowercase();
    text_markers
        .iter()
        .find(|marker| visible_text_lower.contains(&marker.to_ascii_lowercase()))
        .cloned()
}

fn has_login_form(tags: &[HtmlTag], fields: &LoginFormFields, submit_path: &str) -> bool {
    let has_username = tags
        .iter()
        .any(|tag| tag.name == "input" && tag_has_field_name(tag, &fields.username_field));
    let has_password = tags
        .iter()
        .any(|tag| tag.name == "input" && tag_has_field_name(tag, &fields.password_field));
    let has_matching_action = tags.iter().any(|tag| {
        tag.name == "form"
            && tag
                .attribute_value("action")
                .map(|action| same_route_path(&action, submit_path))
                .unwrap_or(false)
    });
    (has_username && has_password) || has_matching_action
}

fn same_route_path(action: &str, configured_path: &str) -> bool {
    let action = action.split(['?', '#']).next().unwrap_or(action);
    let configured = configured_path
        .split(['?', '#'])
        .next()
        .unwrap_or(configured_path);
    action == configured || action.trim_end_matches('/') == configured.trim_end_matches('/')
}

fn matching_field_name(tag: &HtmlTag, configured_names: &[String]) -> Option<String> {
    for attribute_name in ["name", "id", "data-name", "class"] {
        let Some(value) = tag.attribute_value(attribute_name) else {
            continue;
        };
        let candidates = if attribute_name == "class" {
            value.split_ascii_whitespace().collect::<Vec<_>>()
        } else {
            vec![value.as_str()]
        };
        for candidate in candidates {
            if configured_names
                .iter()
                .any(|configured| configured.eq_ignore_ascii_case(candidate))
            {
                return Some(candidate.to_owned());
            }
        }
    }
    None
}

fn tag_has_field_name(tag: &HtmlTag, configured_name: &str) -> bool {
    ["name", "id", "data-name"]
        .iter()
        .filter_map(|attribute| tag.attribute_value(attribute))
        .any(|value| value.eq_ignore_ascii_case(configured_name))
}

impl HtmlTag {
    fn attribute_value(&self, name: &str) -> Option<String> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| attribute.value.clone())
    }
}

fn query_parameter(query: &str, names: &[String]) -> Option<(String, Option<String>)> {
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(raw_name)?;
        if names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&name))
        {
            let value = percent_decode(raw_value)?;
            return Some((name, (!value.is_empty()).then_some(value)));
        }
    }
    None
}

fn query_is_well_formed(query: &str) -> bool {
    query.split('&').all(|pair| {
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        percent_decode(raw_name).is_some() && percent_decode(raw_value).is_some()
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            // Service tickets are URI opaque values.  Unlike an HTML form
            // component, a literal `+` in the query must stay a `+`; only
            // percent escapes are decoded here.
            b'+' => {
                decoded.push(b'+');
                cursor += 1;
            }
            b'%' if cursor + 2 < bytes.len() => {
                let high = hex_value(bytes[cursor + 1])?;
                let low = hex_value(bytes[cursor + 2])?;
                decoded.push((high << 4) | low);
                cursor += 3;
            }
            b'%' => return None,
            byte => {
                decoded.push(byte);
                cursor += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn find_script_assignment(script: &str, name: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(relative_start) = script[search_from..].find(name) {
        let start = search_from + relative_start;
        let end = start + name.len();
        if !identifier_boundary(script, start, end) {
            search_from = end;
            continue;
        }
        let mut cursor = end;
        skip_ascii_whitespace(script, &mut cursor);
        if matches!(script.as_bytes().get(cursor), Some(b':' | b'=')) {
            cursor += 1;
            skip_ascii_whitespace(script, &mut cursor);
            if let Some(value) = read_script_value(script, &mut cursor) {
                if !value.trim().is_empty() {
                    return Some(value);
                }
            }
        }
        search_from = end;
    }
    None
}

fn find_static_script_redirects(script: &str) -> Vec<String> {
    let mut redirects = Vec::new();
    for pattern in [
        "window.location.href",
        "location.href",
        "window.location.replace",
        "location.replace",
        "document.location",
        "window.location",
        "location",
    ] {
        let mut search_from = 0;
        while let Some(relative_start) = script[search_from..].find(pattern) {
            let start = search_from + relative_start;
            let end = start + pattern.len();
            if !identifier_boundary(script, start, end) {
                search_from = end;
                continue;
            }
            let mut cursor = end;
            skip_ascii_whitespace(script, &mut cursor);
            let expected = if pattern.ends_with("href")
                || pattern == "document.location"
                || pattern == "window.location"
                || pattern == "location"
            {
                b'='
            } else {
                b'('
            };
            if script.as_bytes().get(cursor) != Some(&expected) {
                search_from = end;
                continue;
            }
            cursor += 1;
            skip_ascii_whitespace(script, &mut cursor);
            if !matches!(script.as_bytes().get(cursor), Some(b'\'' | b'"')) {
                search_from = end;
                continue;
            }
            if let Some(value) =
                read_script_value(script, &mut cursor).filter(|value| !value.trim().is_empty())
            {
                redirects.push(value);
            }
            search_from = end;
        }
    }
    redirects
}

fn meta_refresh_url(content: &str) -> Option<&str> {
    content.split(';').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("url") {
            return None;
        }
        let value = value.trim();
        if value.len() >= 2
            && matches!(
                (value.as_bytes().first(), value.as_bytes().last()),
                (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"'))
            )
        {
            return Some(&value[1..value.len() - 1]);
        }
        Some(value)
    })
}

fn identifier_boundary(value: &str, start: usize, end: usize) -> bool {
    let is_identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    !value
        .as_bytes()
        .get(start.wrapping_sub(1))
        .copied()
        .map(is_identifier)
        .unwrap_or(false)
        && !value
            .as_bytes()
            .get(end)
            .copied()
            .map(is_identifier)
            .unwrap_or(false)
}

fn read_script_value(script: &str, cursor: &mut usize) -> Option<String> {
    let first = *script.as_bytes().get(*cursor)?;
    if matches!(first, b'\'' | b'"') {
        let quote = first;
        *cursor += 1;
        let mut value = String::new();
        let mut escaped = false;
        while let Some(byte) = script.as_bytes().get(*cursor).copied() {
            *cursor += 1;
            if escaped {
                value.push(match byte {
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    other => other as char,
                });
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                return Some(value);
            } else {
                value.push(byte as char);
            }
        }
        None
    } else {
        let start = *cursor;
        while let Some(byte) = script.as_bytes().get(*cursor).copied() {
            if byte.is_ascii_whitespace() || matches!(byte, b',' | b';' | b'}') {
                break;
            }
            *cursor += 1;
        }
        Some(script[start..*cursor].to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{
        FormEncoding, LoginFormFields, LoginFormProfile, SecondAuthActions, SecondAuthProfile,
        TrustedDeviceProfile,
    };

    const REDACTED_LOGIN_FIXTURE: &str = r#"
        <!doctype html>
        <html>
          <body>
            <form action="/do/off/ui/auth/login/check" method="post">
              <input type="hidden" id="sm2publicKey" value="PUBLIC_KEY_REDACTED" />
              <input type="hidden" name="_csrf" value="CSRF_TOKEN_REDACTED" />
              <input name="i_user" value="student-redacted" />
              <input name="i_pass" type="password" />
              <a href="/b/learn?ticket=TICKET_REDACTED&amp;next=%2Fhome">continue</a>
            </form>
          </body>
        </html>
    "#;

    fn profile(encoding: FormEncoding) -> IdentityLoginProfile {
        IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                encoding,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                Vec::new(),
                SecondAuthActions::new(None, None, None, None),
            ),
        )
    }

    fn client(encoding: FormEncoding) -> IdentityClient {
        IdentityClient::new(
            IdentityClientConfig::new("https://id.example.test/", profile(encoding))
                .expect("fixture configuration is valid"),
        )
        .expect("fixture client is valid")
    }

    #[test]
    fn backend_repair_trusted_get_form_generic_notice_is_not_a_failed_post() {
        let client = client(FormEncoding::UrlEncoded);
        let body = r#"<div>登录失败</div><form method="post" action="/do/off/ui/auth/login/checkSingle"></form>"#;
        let page = client.login_page_url().unwrap();
        assert!(client.service_check_single_action(&page, body).is_some());
        let submitted = page.join("/do/off/ui/auth/login/checkSingle").unwrap();
        assert!(
            client
                .service_check_single_action(&submitted, body)
                .is_none()
        );
        for error in [
            r#"<input name="loginInvalid" value="true">"#,
            "用户名或密码错误",
            "请输入验证码",
            "二次认证",
        ] {
            let html = format!("{error}{body}");
            assert!(client.service_check_single_action(&page, &html).is_none());
        }
    }

    #[test]
    fn backend_repair_empty_identity_error_placeholders_do_not_reject_trusted_form() {
        let client = client(FormEncoding::UrlEncoded);
        let html = r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"></form><span id="errorMsg"></span><input name="loginError" value="">"#;
        assert!(client.parse_login_page(html).failure.is_none());
        assert!(
            client
                .service_check_single_action(&client.login_page_url().unwrap(), html)
                .is_some()
        );
        assert!(
            client
                .parse_login_page(r#"<div id="errorMsg">explicit failure</div>"#)
                .failure
                .is_some()
        );
        assert!(
            client
                .parse_login_page(
                    r#"<input type="hidden" name="loginError" value="explicit failure">"#
                )
                .failure
                .is_some()
        );
        assert!(
            client
                .parse_login_page(r#"<input type="hidden" name="loginInvalid" value="true">"#)
                .failure
                .is_some()
        );
    }

    #[test]
    fn backend_repair_hidden_identity_default_messages_are_not_visible_failure_evidence() {
        let client = client(FormEncoding::UrlEncoded);
        for markup in [
            "hidden",
            "class='hidden'",
            "style='display: none'",
            "style='visibility:hidden'",
        ] {
            let html = format!(
                r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"></form><div {markup}><span>用户名或密码错误</span><span>请输入验证码</span></div><input type="hidden" name="_csrf" value="fixture-csrf">"#
            );
            let page = client.parse_login_page(&html);
            assert!(page.failure.is_none());
            assert!(page.csrf.is_some());
            assert!(
                client
                    .service_check_single_action(&client.login_page_url().unwrap(), &html)
                    .is_some()
            );
        }
        let html = r#"<div>用户名或密码错误</div><form method="post" action="/do/off/ui/auth/login/checkSingle"></form>"#;
        assert!(client.parse_login_page(html).failure.is_some());
        assert!(
            client
                .service_check_single_action(&client.login_page_url().unwrap(), html)
                .is_none()
        );
        assert!(
            client
                .parse_login_page(r#"<div hidden><input name="loginInvalid" value="true"></div>"#)
                .failure
                .is_some()
        );
    }

    #[test]
    fn backend_repair_service_roam_selects_passwordless_form_not_sm2() {
        let client = client(FormEncoding::UrlEncoded);
        let page = client.login_page_url().unwrap();
        let html = r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"><input type="hidden" name="target" value="fixture-target"></form>"#;
        let action = client
            .service_check_single_action(&page, html)
            .expect("trusted service continuation");
        let single = client.with_login_submit_url(&action).unwrap();
        let form = single
            .build_check_single_multipart_login_form(
                LoginFormInput::new("", PasswordInput::precomputed_wire(""))
                    .with_fingerprint("fixture-fingerprint")
                    .with_generated_fingerprint(""),
                "fixture-boundary",
            )
            .unwrap();
        let body = form.body();
        assert!(body.contains("i_rememberme"));
        assert!(body.contains("fingerPrint"));
        assert!(!body.contains("i_user"));
        assert!(!body.contains("i_pass"));
        assert!(!body.contains("fixture-target"));
    }

    #[test]
    fn backend_repair_service_roam_rejects_untrusted_or_ambiguous_actions() {
        let client = client(FormEncoding::UrlEncoded);
        let page = client.login_page_url().unwrap();
        for html in [
            r#"<script>const note='checkSingle';</script>"#,
            r#"<form method="get" action="/do/off/ui/auth/login/checkSingle"></form>"#,
            r#"<form method="post" action="https://evil.example/checkSingle"></form>"#,
            r#"<form method="post" action="https://user@id.example.test/do/off/ui/auth/login/checkSingle"></form>"#,
            r#"<form method="post" action="/do/off/ui/auth/login/checkSingle?next=fixture"></form>"#,
            r#"<form method="post" action="/do/off/ui/auth/login/checkSingle#fragment"></form>"#,
            r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"></form><form method="post" action="/do/off/ui/auth/login/checkSingle"></form>"#,
        ] {
            assert!(client.service_check_single_action(&page, html).is_none());
        }
        assert!(client.service_check_single_action(
            &Url::parse("https://evil.example/").unwrap(),
            r#"<form method="post" action="https://id.example.test/do/off/ui/auth/login/checkSingle"></form>"#,
        ).is_none());
    }

    #[test]
    fn builds_configured_login_urls_without_cross_origin_routes() {
        let client = client(FormEncoding::UrlEncoded);
        let page = client.login_page_request_plan().expect("page plan");
        assert_eq!(page.method, Method::GET);
        assert_eq!(
            page.url.as_str(),
            "https://id.example.test/do/off/ui/auth/login/form/portal-app/0"
        );

        let submit = client
            .login_submission_request_plan()
            .expect("submission plan");
        assert_eq!(
            submit.url().as_str(),
            "https://id.example.test/do/off/ui/auth/login/check"
        );
        assert_eq!(submit.method(), &Method::POST);
        assert_eq!(submit.content_type(), URL_ENCODED_CONTENT_TYPE);
    }

    #[test]
    fn retains_the_dynamic_webvpn_callback_query_without_exposing_it_in_debug() {
        let client = client(FormEncoding::UrlEncoded);
        let login_url = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/form/portal-app/0?/thu-oauth/callback?sig=SIGNED_QUERY_REDACTED",
        )
        .expect("dynamic login URL");
        let client = client
            .with_login_page_url(&login_url)
            .expect("same-origin dynamic login URL");

        assert_eq!(
            client.login_page_url().expect("login page").as_str(),
            login_url.as_str()
        );
        let debug = format!("{client:?}");
        assert!(!debug.contains("SIGNED_QUERY_REDACTED"));

        let other = client
            .with_app_id("new-app")
            .expect("app id can be rebound");
        assert_eq!(
            other.login_page_url().expect("rebound login page").as_str(),
            "https://id.example.test/do/off/ui/auth/login/form/new-app/0"
        );
    }

    #[test]
    fn uses_the_validated_dynamic_login_form_action_for_submission() {
        let client = client(FormEncoding::UrlEncoded);
        let submit_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("submit URL");
        let client = client
            .with_login_submit_url(&submit_url)
            .expect("same-origin submit URL");
        let plan = client
            .login_submission_request_plan()
            .expect("submission plan");
        assert_eq!(plan.url().as_str(), submit_url.as_str());

        let query_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check?source=dynamic")
                .expect("query submit URL");
        assert!(client.with_login_submit_url(&query_url).is_err());

        let foreign = Url::parse("https://evil.example.test/login/check").expect("foreign URL");
        assert!(client.with_login_submit_url(&foreign).is_err());

        let same_origin_unprofiled =
            Url::parse("https://id.example.test/do/off/ui/auth/login/other").expect("URL");
        assert!(
            client
                .with_login_submit_url(&same_origin_unprofiled)
                .is_err()
        );
    }

    #[test]
    fn builds_the_public_check_single_trusted_device_multipart_shape() {
        let client = client(FormEncoding::UrlEncoded);
        let submit_url = Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle")
            .expect("checkSingle submit URL");
        let client = client
            .with_login_submit_url(&submit_url)
            .expect("checkSingle is an explicitly supported submit route");
        let plan = client
            .login_submission_request_plan()
            .expect("checkSingle plan");
        assert!(matches!(
            plan,
            LoginSubmissionRequestPlan::TrustedDeviceMultipart(_)
        ));
        assert_eq!(plan.content_type(), MULTIPART_CONTENT_TYPE);

        let hidden = [LoginFormHiddenField::new("target", "TARGET_REDACTED")];
        let form = client
            .build_check_single_multipart_login_form(
                LoginFormInput::new(
                    "student-redacted",
                    PasswordInput::plaintext("PASSWORD_REDACTED"),
                )
                .with_hidden_fields(&hidden)
                .with_fingerprint("FINGERPRINT_REDACTED")
                .with_generated_fingerprint(""),
                "fixture-boundary",
            )
            .expect("trusted-device form");
        let body = form.body();
        assert!(body.contains("name=\"i_rememberme\""));
        assert!(body.contains("name=\"fingerPrint\""));
        assert!(body.contains("name=\"fingerGenPrint\""));
        assert!(body.contains("name=\"target\""));
        assert!(!body.contains("i_user"));
        assert!(!body.contains("i_pass"));
        assert!(!body.contains("PASSWORD_REDACTED"));
        assert!(!format!("{form:?}").contains("FINGERPRINT_REDACTED"));
    }

    #[test]
    fn second_factor_redirect_allowlist_excludes_service_ticket_routes() {
        let client = client_with_webvpn_and_oauth_routes();
        assert!(client.is_second_factor_redirect_url(
            "https://id.example.test/do/off/ui/auth/login/redirect2Jsp"
        ));
        assert!(client.is_second_factor_redirect_url(
            "https://id.example.test/do/off/ui/auth/login/checkSingle?continuation=1"
        ));
        assert!(client.is_second_factor_redirect_url(
            "https://oauth.example.test/lb-auth/lbredirect?scheme=https&host=webvpn.example.test&port=443&uri=%2F"
        ));
        assert!(
            !client.is_second_factor_redirect_url(
                "https://learn.example.test/b/learn?ticket=REDACTED"
            )
        );
        assert!(!client.is_second_factor_redirect_url(
            "https://id.example.test/do/off/ui/auth/login/redirect2Jsp?ticket=REDACTED"
        ));
        assert!(!client.is_second_factor_redirect_url(
            "https://id.example.test/do/off/ui/auth/login/redirect2Jsp?bad=%ZZ"
        ));
    }

    #[test]
    fn accepts_a_trailing_slash_on_the_profiled_submit_response_route() {
        let client = client(FormEncoding::UrlEncoded);
        let response_url = Url::parse("https://id.example.test/do/off/ui/auth/login/check/")
            .expect("submit response URL");

        assert!(client.is_login_submission_response_url(&response_url));
        assert!(matches!(
            client.classify_login_response(
                StatusCode::OK,
                &response_url,
                "<html><body>authenticated continuation</body></html>",
            ),
            LoginResponseClassification::LoginPage(page) if !page.has_login_form
        ));
    }

    #[test]
    fn recognizes_check_single_only_as_a_verified_continuation_route() {
        let client = client(FormEncoding::UrlEncoded);
        let response_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle?continuation=1")
                .expect("checkSingle response URL");

        assert!(!client.is_login_submission_response_url(&response_url));
        assert!(client.is_verified_identity_continuation_url(&response_url));

        let foreign = Url::parse("https://evil.example.test/do/off/ui/auth/login/checkSingle")
            .expect("foreign checkSingle URL");
        assert!(!client.is_verified_identity_continuation_url(&foreign));

        let with_ticket =
            Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle?ticket=opaque")
                .expect("ticket-bearing checkSingle URL");
        assert!(!client.is_verified_identity_continuation_url(&with_ticket));

        let with_fragment = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/checkSingle?continuation=1#fragment",
        )
        .expect("fragment-bearing checkSingle URL");
        assert!(!client.is_verified_identity_continuation_url(&with_fragment));
    }

    #[test]
    fn allows_check_single_as_a_ticketless_verified_redirect_only() {
        let client = client(FormEncoding::UrlEncoded);
        let check_single =
            "https://id.example.test/do/off/ui/auth/login/checkSingle?continuation=1";

        assert!(client.is_safe_handoff_redirect(check_single));
        assert!(client.resolve_redirect_url(check_single).is_ok());

        let with_ticket =
            "https://id.example.test/do/off/ui/auth/login/checkSingle?ticket=REDACTED";
        assert!(!client.is_safe_handoff_redirect(with_ticket));

        let ordinary_route = "https://id.example.test/do/off/ui/auth/login/other";
        assert!(!client.is_safe_handoff_redirect(ordinary_route));
    }

    #[test]
    fn percent_encodes_an_app_id_as_one_path_segment() {
        let mut config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid");
        config.profile.app_id = String::from("portal/app");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let url = client.login_page_url().expect("page URL");
        assert_eq!(
            url.as_str(),
            "https://id.example.test/do/off/ui/auth/login/form/portal%2Fapp/0"
        );
    }

    #[test]
    fn extracts_redacted_sm2_csrf_and_anchor_evidence() {
        let client = client(FormEncoding::UrlEncoded);
        let evidence = client.parse_login_page(REDACTED_LOGIN_FIXTURE);

        assert_eq!(
            evidence
                .sm2_public_key
                .as_ref()
                .map(|key| key.value.as_str()),
            Some("PUBLIC_KEY_REDACTED")
        );
        assert_eq!(
            evidence.csrf.as_ref().map(CsrfEvidence::as_str),
            Some("CSRF_TOKEN_REDACTED")
        );
        let anchor = evidence.anchor_ticket.as_ref().expect("ticket anchor");
        assert_eq!(anchor.ticket.as_deref(), Some("TICKET_REDACTED"));
        assert_eq!(evidence.invalidation.status, InvalidationStatus::NotFound);
        assert!(evidence.has_login_form);
        assert!(evidence.failure.is_none());
        let debug = format!("{evidence:?}");
        assert!(!debug.contains("TICKET_REDACTED"));
        assert!(!debug.contains("CSRF_TOKEN_REDACTED"));
    }

    #[test]
    fn accepts_anchor_tickets_only_from_same_origin_handoff_paths() {
        let client = client(FormEncoding::UrlEncoded);
        let evidence = client.parse_login_page(
            r#"
                <a href="https://id.example.test/b/learn?ticket=VALID_TICKET">valid</a>
                <a href="https://evil.example.test/b/learn?ticket=FOREIGN_TICKET">foreign</a>
                <a href="https://id.example.test/unrelated?ticket=UNRELATED_TICKET">unrelated</a>
            "#,
        );

        let anchor = evidence.anchor_ticket.expect("valid handoff anchor");
        assert_eq!(anchor.ticket.as_deref(), Some("VALID_TICKET"));
    }

    #[test]
    fn accepts_a_ticket_from_an_explicit_learn_origin() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let evidence = client.parse_login_page(
            r#"<a href="https://learn.example.test/f/j_spring_security_thauth_roaming_entry?ticket=LEARN_TICKET">continue</a>"#,
        );

        assert_eq!(
            evidence
                .anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("LEARN_TICKET")
        );
    }

    #[test]
    fn retains_a_bare_redirect2jsp_callback_for_the_session_layer() {
        let client = client(FormEncoding::UrlEncoded);
        let evidence =
            client.parse_login_page(r#"<a href="/do/off/ui/auth/login/redirect2Jsp">continue</a>"#);
        let anchor = evidence.anchor_ticket.expect("callback anchor");

        assert!(anchor.ticket.is_none());
        assert!(client.is_ticket_redirect_callback(&anchor.href));
        assert!(!client.is_ticket_redirect_callback("/b/learn?ticket=opaque"));
    }

    #[test]
    fn ignores_empty_anchor_and_script_continuations_in_success_templates() {
        let client = client_with_webvpn_and_oauth_routes();
        let html = r#"
            <a href="">直接跳转</a>
            <a href="   ">空白跳转</a>
            <script>
                window.location.replace("");
                window.location.href = "   ";
            </script>
            <meta http-equiv="refresh" content="0;url=">
        "#;

        let page = client.parse_login_page(html);
        assert!(page.anchor_ticket.is_none());

        let origin = Url::parse("https://id.example.test/").expect("origin");
        assert!(client.first_anchor_url_for_origin(html, &origin).is_none());
    }

    #[test]
    fn resolves_a_relative_success_anchor_against_the_response_url() {
        let client = client(FormEncoding::UrlEncoded);
        let final_url = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let result = client.classify_login_response(
            StatusCode::OK,
            &final_url,
            r#"<html><body><a href="../../../../../b/intermediate">continue</a></body></html>"#,
        );
        let (LoginResponseClassification::RedirectCallback(page)
        | LoginResponseClassification::OtherPage(page)) = result
        else {
            panic!("a ticketless success anchor must remain followable");
        };
        let anchor = page.anchor_ticket.expect("intermediate anchor");
        assert_eq!(anchor.href, "https://id.example.test/b/intermediate");
        assert!(anchor.ticket.is_none());
        assert!(client.is_ticket_redirect_callback(&anchor.href));
    }

    #[test]
    fn follows_only_allowlisted_ticketless_handoff_anchors() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");

        let public = client.parse_login_page(
            r#"<a href="https://learn.example.test/f/intermediate">continue</a>"#,
        );
        let public_anchor = public.anchor_ticket.expect("allowlisted public anchor");
        assert!(public_anchor.ticket.is_none());
        assert!(client.is_ticket_redirect_callback(&public_anchor.href));

        let foreign = client
            .parse_login_page(r#"<a href="https://evil.example.test/f/intermediate">foreign</a>"#);
        assert!(foreign.anchor_ticket.is_none());

        let malformed =
            client.parse_login_page(r#"<a href="/b/intermediate?ticket=%00">malformed</a>"#);
        assert!(malformed.anchor_ticket.is_none());

        let malformed_encoding =
            client.parse_login_page(r#"<a href="/b/intermediate?ticket=%ZZ">malformed</a>"#);
        assert!(malformed_encoding.anchor_ticket.is_none());
    }

    #[test]
    fn classifies_a_cross_origin_redirect_location_with_a_ticket() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let final_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("identity URL");
        let location = Url::parse(
            "https://learn.example.test/f/j_spring_security_thauth_roaming_entry?ticket=REDIRECT_TICKET",
        )
        .expect("Learn handoff URL");

        let result = client.classify_login_response_with_location(
            StatusCode::FOUND,
            &final_url,
            Some(&location),
            "",
        );
        let LoginResponseClassification::AuthenticatedHandoff(page) = result else {
            panic!("allowlisted redirect location must prove the handoff");
        };
        assert_eq!(
            page.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("REDIRECT_TICKET")
        );
    }

    #[test]
    fn classifies_a_success_location_header_with_an_allowlisted_ticket() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let final_url = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("identity callback URL");
        let location = Url::parse(
            "https://learn.example.test/f/j_spring_security_thauth_roaming_entry?ticket=SUCCESS_LOCATION_TICKET",
        )
        .expect("Learn handoff URL");

        let result = client.classify_login_response_with_location(
            StatusCode::OK,
            &final_url,
            Some(&location),
            "<html><body>success page</body></html>",
        );
        let LoginResponseClassification::AuthenticatedHandoff(page) = result else {
            panic!("an allowlisted Location on a successful response must prove the handoff");
        };
        assert_eq!(
            page.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("SUCCESS_LOCATION_TICKET")
        );
    }

    #[test]
    fn classifies_a_cross_origin_ticketless_handoff_location_as_a_continuation() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let final_url = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("identity callback URL");
        let location =
            Url::parse("https://learn.example.test/f/j_spring_security_thauth_roaming_entry")
                .expect("ticketless Learn handoff URL");

        let result = client.classify_login_response_with_location(
            StatusCode::FOUND,
            &final_url,
            Some(&location),
            "",
        );
        let LoginResponseClassification::RedirectCallback(page) = result else {
            panic!("an allowlisted ticketless handoff must remain followable");
        };
        let handoff = page.anchor_ticket.expect("handoff continuation");
        assert_eq!(handoff.href, location.to_string());
        assert!(handoff.ticket.is_none());
    }

    #[test]
    fn classifies_a_final_handoff_url_with_a_ticket() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let final_url = Url::parse(
            "https://id.example.test/do/off/ui/auth/login/redirect2Jsp?ticket=FINAL_TICKET",
        )
        .expect("callback URL");

        let result = client.classify_login_response_with_location(
            StatusCode::OK,
            &final_url,
            None,
            "<html><body>redirected</body></html>",
        );
        let LoginResponseClassification::AuthenticatedHandoff(page) = result else {
            panic!("the final callback URL must be accepted as handoff evidence");
        };
        assert_eq!(
            page.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("FINAL_TICKET")
        );
    }

    #[test]
    fn classifies_a_final_ticketless_allowlisted_handoff_as_cookie_backed() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let final_url =
            Url::parse("https://learn.example.test/f/j_spring_security_thauth_roaming_entry")
                .expect("ticketless handoff URL");

        let result = client.classify_login_response(
            StatusCode::OK,
            &final_url,
            r#"<html><head><meta name="_csrf" content="CSRF_REDACTED"></head><body>service handoff complete</body></html>"#,
        );
        let LoginResponseClassification::CookieBackedHandoff(page) = result else {
            panic!("a fetched ticketless handoff must be classified explicitly");
        };
        let anchor = page.anchor_ticket.expect("handoff evidence");
        assert_eq!(anchor.href, final_url.as_str());
        assert!(anchor.ticket.is_none());
        assert!(client.is_cookie_backed_handoff_url(&final_url));
        assert!(
            !client.is_cookie_backed_handoff_url(
                &Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
                    .expect("callback URL")
            )
        );

        let unrelated = Url::parse("https://webvpn.example.test/unrelated").expect("URL");
        assert!(!client.is_cookie_backed_handoff_url(&unrelated));
    }

    #[test]
    fn follows_a_ticketless_webvpn_handoff_without_treating_webvpn_as_ticket_source() {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured")
        .with_cookie_backed_handoff_origins(["https://webvpn.example.test/"])
        .expect("WebVPN origin is explicitly configured")
        .with_cookie_backed_handoff_path_prefixes(["/", "/login"])
        .expect("WebVPN handoff path is explicitly configured");
        let client = IdentityClient::new(config).expect("fixture client is valid");
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let webvpn = Url::parse("https://webvpn.example.test/").expect("WebVPN URL");

        assert!(client.is_safe_handoff_redirect(webvpn.as_str()));
        assert!(client.is_cookie_backed_handoff_url(&webvpn));
        assert!(client.resolve_redirect_url(webvpn.as_str()).is_ok());

        let webvpn_login =
            Url::parse("https://webvpn.example.test/login?oauth_login=true").expect("WebVPN login");
        assert!(client.is_safe_handoff_redirect(webvpn_login.as_str()));
        assert!(client.is_cookie_backed_handoff_url(&webvpn_login));
        assert!(client.resolve_redirect_url(webvpn_login.as_str()).is_ok());

        let result = client.classify_login_response_with_location(
            StatusCode::FOUND,
            &callback,
            Some(&webvpn),
            "",
        );
        let LoginResponseClassification::RedirectCallback(page) = result else {
            panic!("a WebVPN Cookie handoff must remain followable");
        };
        let handoff = page.anchor_ticket.expect("WebVPN continuation");
        assert_eq!(handoff.href, webvpn.as_str());
        assert!(handoff.ticket.is_none());

        let result = client.classify_login_response(
            StatusCode::OK,
            &webvpn,
            r#"<html><head><meta name="_csrf" content="CSRF_TOKEN_REDACTED"></head><body>authenticated WebVPN shell</body></html>"#,
        );
        assert!(matches!(
            result,
            LoginResponseClassification::CookieBackedHandoff(_)
        ));

        let unproven = client.classify_login_response(
            StatusCode::OK,
            &webvpn,
            "<html><body>authenticated WebVPN shell</body></html>",
        );
        assert!(matches!(
            unproven,
            LoginResponseClassification::OtherPage(_)
        ));

        let page = client
            .parse_login_page(r#"<a href="https://webvpn.example.test/">continue to WebVPN</a>"#);
        let anchor = page.anchor_ticket.expect("WebVPN HTML continuation");
        assert_eq!(anchor.href, webvpn.as_str());
        assert!(anchor.ticket.is_none());

        let ticket_url =
            Url::parse("https://webvpn.example.test/?ticket=opaque").expect("ticket URL");
        assert!(!client.is_cookie_backed_handoff_url(&ticket_url));
    }

    fn client_with_webvpn_and_oauth_routes() -> IdentityClient {
        let config = IdentityClientConfig::new(
            "https://id.example.test/",
            profile(FormEncoding::UrlEncoded),
        )
        .expect("fixture configuration is valid")
        .with_anchor_ticket_origins(["https://learn.example.test/"])
        .expect("Learn origin is explicitly configured")
        .with_cookie_backed_handoff_route("https://webvpn.example.test/", ["/"])
        .expect("WebVPN route is explicitly configured")
        .with_cookie_backed_handoff_route(
            "https://oauth.example.test/",
            ["/thu-oauth/", "/lb-auth/"],
        )
        .expect("OAuth route is explicitly configured");
        IdentityClient::new(config).expect("fixture client is valid")
    }

    #[test]
    fn accepts_the_public_oauth_lb_auth_callback_as_a_cookie_handoff() {
        let client = client_with_webvpn_and_oauth_routes();
        let callback = Url::parse(
            "https://oauth.example.test/lb-auth/lbredirect?scheme=https&host=webvpn.example.test&port=443&uri=%2F",
        )
        .expect("OAuth callback URL");

        assert!(client.is_safe_handoff_redirect(callback.as_str()));
        assert!(client.is_cookie_backed_handoff_url(&callback));
        assert!(client.resolve_redirect_url(callback.as_str()).is_ok());

        let page = client.parse_login_page(&format!(
            r#"<html><body>登录成功<a href="{callback}">继续</a></body></html>"#
        ));
        let anchor = page.anchor_ticket.expect("OAuth callback anchor");
        assert_eq!(anchor.href, callback.as_str());
        assert!(anchor.ticket.is_none());
    }

    #[test]
    fn prefers_an_allowlisted_oauth_continuation_over_a_stale_login_form() {
        let client = client_with_webvpn_and_oauth_routes();
        let callback = "https://oauth.example.test/lb-auth/lbredirect?scheme=https&host=webvpn.example.test&port=443&uri=%2F";
        let final_url = Url::parse("https://id.example.test/do/off/ui/auth/login/check")
            .expect("identity check URL");

        let result = client.classify_login_response(
            StatusCode::OK,
            &final_url,
            &format!(
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form><a href="{callback}">继续</a>"#
            ),
        );
        let LoginResponseClassification::RedirectCallback(page) = result else {
            panic!("the explicit OAuth continuation must win over stale login markup");
        };
        assert_eq!(page.anchor_ticket.expect("OAuth callback").href, callback);
    }

    #[test]
    fn accepts_only_the_encoded_webvpn_root_wrapper_as_a_cookie_handoff() {
        let client = client_with_webvpn_and_oauth_routes();
        for path in [
            "/https/opaque-map%2F",
            "/http/opaque-map%2f",
            "/https-443/opaque-map/",
            "/http-80/opaque-map%2F",
        ] {
            let url = Url::parse(&format!("https://webvpn.example.test{path}"))
                .expect("WebVPN root wrapper");
            assert!(client.is_safe_handoff_redirect(url.as_str()));
            assert!(client.is_cookie_backed_handoff_url(&url));

            let result = client.classify_login_response_with_location(
                StatusCode::TEMPORARY_REDIRECT,
                &Url::parse("https://oauth.example.test/lb-auth/lbredirect")
                    .expect("OAuth callback"),
                Some(&url),
                "",
            );
            let LoginResponseClassification::RedirectCallback(page) = result else {
                panic!("the WebVPN root wrapper must remain a followable continuation");
            };
            let anchor = page.anchor_ticket.expect("root wrapper continuation");
            assert_eq!(anchor.href, url.as_str());
            assert!(anchor.ticket.is_none());
        }

        for path in [
            "/https/opaque-map%2F/extra",
            "/https/opaque-map",
            "/https/opaque/map%2F",
        ] {
            let url =
                Url::parse(&format!("https://webvpn.example.test{path}")).expect("WebVPN URL");
            assert!(!client.is_cookie_backed_handoff_url(&url));
            assert!(!client.is_safe_handoff_redirect(url.as_str()));
        }

        // Root wrappers may carry the Cookie handoff, but they must never be
        // accepted as opaque service-ticket sources.
        let root_ticket = Url::parse(
            "https://webvpn.example.test/https/opaque-map%2F?ticket=ROOT_TICKET_REDACTED",
        )
        .expect("root wrapper ticket URL");
        assert!(
            client
                .parse_login_page(&format!(r#"<a href="{root_ticket}">continue</a>"#))
                .anchor_ticket
                .is_none()
        );
    }

    #[test]
    fn accepts_explicit_webvpn_ports_but_rejects_untrusted_wrapper_prefixes() {
        let client = client_with_webvpn_and_oauth_routes();
        for path in [
            "/http-8080/opaque-map%2F",
            "/https-8443/opaque-map%2f",
            "/http-1/opaque-map/",
            "/https-65535/opaque-map%2F",
        ] {
            let url = Url::parse(&format!("https://webvpn.example.test{path}"))
                .expect("explicit-port WebVPN wrapper");
            assert!(client.is_safe_handoff_redirect(url.as_str()));
            assert!(client.is_cookie_backed_handoff_url(&url));
        }

        for path in [
            "/http-0/opaque-map%2F",
            "/https-65536/opaque-map%2F",
            "/https-123456/opaque-map%2F",
            "/ftp-8080/opaque-map%2F",
            "/https-8080/opaque-map%2F/extra",
        ] {
            let url =
                Url::parse(&format!("https://webvpn.example.test{path}")).expect("WebVPN wrapper");
            assert!(!client.is_safe_handoff_redirect(url.as_str()));
            assert!(!client.is_cookie_backed_handoff_url(&url));
        }
    }

    #[test]
    fn accepts_only_the_allowlisted_webvpn_identity_callback_wrapper() {
        let client = client_with_webvpn_and_oauth_routes();
        for callback_text in [
            "https://webvpn.example.test/https/opaque-map/do/off/ui/auth/login/redirect2Jsp",
            "https://webvpn.example.test/https/opaque-map%2Fdo%2Foff%2Fui%2Fauth%2Flogin%2Fredirect2Jsp",
            "https://webvpn.example.test/https/opaque-map/do/off/ui/auth/login/checkSingle",
            "https://webvpn.example.test/https/opaque-map%2Fdo%2Foff%2Fui%2Fauth%2Flogin%2FcheckSingle",
        ] {
            let callback = Url::parse(callback_text).expect("wrapped identity callback");

            assert!(client.is_safe_handoff_redirect(callback.as_str()));
            assert!(client.is_cookie_backed_handoff_url(&callback));

            let result = client.classify_login_response_with_location(
                StatusCode::FOUND,
                &Url::parse("https://oauth.example.test/thu-oauth/callback")
                    .expect("OAuth callback"),
                Some(&callback),
                "",
            );
            let LoginResponseClassification::RedirectCallback(page) = result else {
                panic!("the wrapped identity callback must remain followable");
            };
            assert_eq!(
                page.anchor_ticket
                    .expect("wrapped callback continuation")
                    .href,
                callback.as_str()
            );

            let page = client.parse_login_page(&format!(
                r#"<a href="{callback}">continue identity handoff</a>"#
            ));
            assert_eq!(
                page.anchor_ticket.expect("wrapped callback anchor").href,
                callback.as_str()
            );
        }

        for path in [
            "/https/opaque-map/do/off/ui/auth/login/other",
            "/https/opaque-map/do/off/ui/auth/login/redirect2Jsp/extra",
            "/https/opaque-map/do/off/ui/auth/login/redirect2Jsp/nested/path",
            "/https/opaque-map/do/off/ui/auth/login/checkSingle/extra",
            "/https/opaque-map%2Fdo%2Foff%2Fui%2Fauth%2Flogin%2Fother",
            "/https/opaque-map%2Fdo%2Foff%2Fui%2Fauth%2Flogin%2Fredirect2Jsp%2Fextra",
            "/https/opaque-map%2Fdo%2Foff%2Fui%2Fauth%2Flogin%2FcheckSingle%2Fextra",
        ] {
            let url = Url::parse(&format!("https://webvpn.example.test{path}"))
                .expect("untrusted wrapper shape");
            assert!(!client.is_cookie_backed_handoff_url(&url));
            assert!(!client.is_safe_handoff_redirect(url.as_str()));
        }

        // The identity wrapper is a Cookie continuation only. A ticket query
        // must not turn it into an opaque service-ticket source.
        let with_ticket = Url::parse(
            "https://webvpn.example.test/https/opaque-map%2Fdo%2Foff%2Fui%2Fauth%2Flogin%2Fredirect2Jsp?ticket=WRAPPED_TICKET_REDACTED",
        )
        .expect("wrapped identity callback with ticket");
        assert!(!client.is_cookie_backed_handoff_url(&with_ticket));
        assert!(
            client
                .parse_login_page(&format!(r#"<a href="{with_ticket}">ticket candidate</a>"#))
                .anchor_ticket
                .is_none()
        );
    }

    #[test]
    fn extracts_only_a_learn_ticket_from_a_webvpn_wrapper_fixture() {
        let client = client_with_webvpn_and_oauth_routes();
        for (prefix, ticket) in [
            (
                "/https/opaque-map/f/j_spring_security_thauth_roaming_entry",
                "WRAPPED_TICKET_REDACTED",
            ),
            (
                "/http/opaque-map/b/j_spring_security_thauth_roaming_entry",
                "WRAPPED_B_TICKET_REDACTED",
            ),
            (
                "/https-443/opaque-map/f/j_spring_security_thauth_roaming_entry",
                "WRAPPED_443_TICKET_REDACTED",
            ),
            (
                "/http-80/opaque-map/b/j_spring_security_thauth_roaming_entry",
                "WRAPPED_80_TICKET_REDACTED",
            ),
        ] {
            let html = format!(
                r#"<a href="https://webvpn.example.test{prefix}?ticket={ticket}">continue</a>"#
            );
            let evidence = client.parse_login_page(&html);
            assert_eq!(
                evidence.anchor_ticket.and_then(|anchor| anchor.ticket),
                Some(ticket.to_owned())
            );
        }

        let plus = client.parse_login_page(
            r#"<a href="https://webvpn.example.test/https/opaque-map/f/j_spring_security_thauth_roaming_entry?ticket=WRAPPED%2BTICKET">continue</a>"#,
        );
        assert_eq!(
            plus.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("WRAPPED+TICKET")
        );

        for href in [
            "https://webvpn.example.test/?ticket=ROOT_TICKET_REDACTED",
            "https://webvpn.example.test/https/opaque-map/unrelated?ticket=UNRELATED_TICKET_REDACTED",
            "https://webvpn.example.test/https/opaque-map/f/other?ticket=OTHER_TICKET_REDACTED",
            "https://oauth.example.test/thu-oauth/callback?ticket=OAUTH_TICKET_REDACTED",
        ] {
            assert!(
                client
                    .parse_login_page(&format!(r#"<a href="{href}">candidate</a>"#))
                    .anchor_ticket
                    .is_none(),
                "untrusted wrapper shape was accepted: {href}"
            );
        }
    }

    #[test]
    fn treats_only_the_explicit_oauth_callback_path_as_a_cookie_handoff_location() {
        let client = client_with_webvpn_and_oauth_routes();
        let final_url = Url::parse("https://id.example.test/do/off/ui/auth/login/check")
            .expect("identity fixture URL");
        let location = Url::parse(
            "https://oauth.example.test/thu-oauth/callback?sig=SIGNED_CALLBACK_REDACTED",
        )
        .expect("OAuth callback fixture URL");
        let result = client.classify_login_response_with_location(
            StatusCode::FOUND,
            &final_url,
            Some(&location),
            "",
        );
        let LoginResponseClassification::RedirectCallback(page) = result else {
            panic!("the explicit OAuth callback Location must remain followable");
        };
        assert_eq!(
            page.anchor_ticket.expect("OAuth continuation").href,
            location.as_str()
        );
        assert!(client.is_cookie_backed_handoff_url(&location));
        assert!(
            !client.is_cookie_backed_handoff_url(
                &Url::parse("https://oauth.example.test/other?sig=SIGNED_CALLBACK_REDACTED")
                    .expect("foreign OAuth path")
            )
        );
        assert!(
            !client.is_cookie_backed_handoff_url(
                &Url::parse(
                    "https://oauth.example.test/thu-oauth/callback?ticket=OAUTH_TICKET_REDACTED"
                )
                .expect("OAuth ticket query")
            )
        );
    }

    #[test]
    fn accepts_hidden_or_static_handoff_tickets_only_on_allowlisted_targets() {
        let client = client_with_webvpn_and_oauth_routes();
        let form = client.parse_login_page(
            r#"<form action="https://webvpn.example.test/https/opaque-map/f/j_spring_security_thauth_roaming_entry"><input type="hidden" name="ticket" value="FORM_TICKET_REDACTED"></form>"#,
        );
        assert_eq!(
            form.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("FORM_TICKET_REDACTED")
        );

        let script = client.parse_login_page(
            r#"<script>window.location.href = 'https://webvpn.example.test/https/opaque-map/b/j_spring_security_thauth_roaming_entry?ticket=SCRIPT_TICKET_REDACTED';</script>"#,
        );
        assert_eq!(
            script
                .anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("SCRIPT_TICKET_REDACTED")
        );

        let ordinary_login_form = client.parse_login_page(
            r#"<form action="/do/off/ui/auth/login/check"><input type="hidden" name="ticket" value="LOGIN_FORM_TICKET_REDACTED"></form>"#,
        );
        assert!(ordinary_login_form.anchor_ticket.is_none());
    }

    #[test]
    fn accepts_allowlisted_meta_refresh_and_direct_location_assignments() {
        let client = client_with_webvpn_and_oauth_routes();
        let meta = client.parse_login_page(
            r#"<meta http-equiv="refresh" content="0; url='https://id.example.test/do/off/ui/auth/login/redirect2Jsp'">"#,
        );
        let meta_anchor = meta.anchor_ticket.expect("meta callback");
        assert!(meta_anchor.ticket.is_none());
        assert!(client.is_ticket_redirect_callback(&meta_anchor.href));

        let script = client.parse_login_page(
            r#"<script>window.location = 'https://webvpn.example.test/https/opaque-map/f/j_spring_security_thauth_roaming_entry?ticket=SCRIPT_TICKET_REDACTED';</script>"#,
        );
        assert_eq!(
            script
                .anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("SCRIPT_TICKET_REDACTED")
        );
    }

    #[test]
    fn prefers_a_later_ticket_over_an_earlier_ticketless_callback() {
        let client = client(FormEncoding::UrlEncoded);
        let page = client.parse_login_page(
            r#"
                <a href="/do/off/ui/auth/login/redirect2Jsp">continue</a>
                <script>
                    window.location.href = "/b/learn?ticket=SCRIPT_TICKET_REDACTED";
                </script>
            "#,
        );

        let anchor = page.anchor_ticket.expect("preferred handoff");
        assert_eq!(anchor.ticket.as_deref(), Some("SCRIPT_TICKET_REDACTED"));
    }

    #[test]
    fn parses_unquoted_url_attributes_without_truncating_slashes() {
        let client = client(FormEncoding::UrlEncoded);
        let page = client.parse_login_page(
            r#"<a href=/b/learn?ticket=UNQUOTED_TICKET>continue</a>
                <meta http-equiv=refresh content=0;url=/do/off/ui/auth/login/redirect2Jsp>"#,
        );

        let anchor = page.anchor_ticket.expect("unquoted handoff");
        assert_eq!(anchor.ticket.as_deref(), Some("UNQUOTED_TICKET"));
    }

    #[test]
    fn continues_past_an_unrelated_script_redirect_to_find_the_allowlisted_handoff() {
        let client = client(FormEncoding::UrlEncoded);
        let page = client.parse_login_page(
            r#"
                <script>
                    window.location = "/unrelated";
                    window.location = "/b/learn?ticket=SCRIPT_TICKET_REDACTED";
                </script>
            "#,
        );

        assert_eq!(
            page.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("SCRIPT_TICKET_REDACTED")
        );
    }

    #[test]
    fn rejects_unsafe_anchor_ticket_sources_and_values() {
        let client = client(FormEncoding::UrlEncoded);
        for href in [
            "https://evil.example.test/b/learn?ticket=FOREIGN_TICKET",
            "https://user:password@id.example.test/b/learn?ticket=USERINFO_TICKET",
            "javascript:window.location='https://id.example.test/b/learn?ticket=SCRIPT_TICKET'",
            "data:text/html,<a href='/b/learn?ticket=DATA_TICKET'>x</a>",
            "/unrelated?ticket=UNRELATED_TICKET",
        ] {
            let html = format!(r#"<a href="{href}">candidate</a>"#);
            let evidence = client.parse_login_page(&html);
            assert!(
                evidence.anchor_ticket.is_none(),
                "unsafe href was accepted: {href}"
            );
        }

        let encoded_control =
            client.parse_login_page(r#"<a href="/b/learn?ticket=bad%00ticket">control</a>"#);
        assert_eq!(
            encoded_control
                .anchor_ticket
                .and_then(|anchor| anchor.ticket),
            None
        );

        let long_ticket = "x".repeat(MAX_ANCHOR_TICKET_LENGTH + 1);
        let long_value = client.parse_login_page(&format!(
            r#"<a href="/b/learn?ticket={long_ticket}">long</a>"#
        ));
        assert_eq!(
            long_value.anchor_ticket.and_then(|anchor| anchor.ticket),
            None
        );
    }

    #[test]
    fn rejects_login_evidence_from_an_unexpected_final_origin() {
        let client = client(FormEncoding::UrlEncoded);
        let final_url = Url::parse("https://evil.example.test/b/learn").expect("foreign URL");
        assert!(matches!(
            client.classify_login_response(StatusCode::OK, &final_url, REDACTED_LOGIN_FIXTURE),
            LoginResponseClassification::UnexpectedOrigin
        ));
    }

    #[test]
    fn classifies_an_http_200_invalid_credentials_page() {
        let client = client(FormEncoding::UrlEncoded);
        let html = r#"
            <form action="/do/off/ui/auth/login/check">
              <input name="i_user" />
              <input name="i_pass" type="password" />
              <div id="loginError">用户名或密码错误</div>
            </form>
        "#;
        let url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("fixture URL");
        let result = client.classify_login_response(StatusCode::OK, &url, html);
        let LoginResponseClassification::LoginFailed { evidence, .. } = result else {
            panic!("a 200 login failure page must not be treated as success");
        };
        assert_eq!(evidence.reason, LoginFailureReason::InvalidCredentials);
        assert_eq!(evidence.source, FailureEvidenceSource::Field);
        assert_eq!(evidence.marker, "loginError");
    }

    #[test]
    fn classifies_the_current_msg_note_invalid_credentials_page() {
        let client = client(FormEncoding::UrlEncoded);
        let html = r#"
            <form action="/do/off/ui/auth/login/check">
              <input name="i_user" />
              <input name="i_pass" type="password" />
              <span id="msg_note">您的用户名或密码不正确，请重试！</span>
            </form>
        "#;
        let url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("fixture URL");
        let result = client.classify_login_response(StatusCode::OK, &url, html);
        let LoginResponseClassification::LoginFailed { evidence, .. } = result else {
            panic!("the current msg_note failure must be classified");
        };
        assert_eq!(evidence.reason, LoginFailureReason::InvalidCredentials);
        assert_eq!(evidence.source, FailureEvidenceSource::Text);
        assert_eq!(evidence.marker, "您的用户名或密码不正确");
    }

    #[test]
    fn keeps_an_explicit_success_page_unproven_without_a_cookie_or_ticket() {
        let client = client(FormEncoding::UrlEncoded);
        let url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("fixture URL");
        let result = client.classify_login_response(
            StatusCode::OK,
            &url,
            "<html><body>登录成功，正在重定向</body></html>",
        );
        let LoginResponseClassification::OtherPage(page) = result else {
            panic!("a success marker without a Cookie or ticket must remain unproven");
        };
        assert!(page.anchor_ticket.is_none());

        let weak = client.classify_login_response(
            StatusCode::OK,
            &url,
            "<html><body>redirecting to the login page</body></html>",
        );
        assert!(matches!(weak, LoginResponseClassification::LoginPage(_)));
    }

    #[test]
    fn keeps_script_backed_identity_success_unproven_without_a_cookie_or_ticket() {
        let client = client(FormEncoding::UrlEncoded);
        let url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("fixture URL");
        let result = client.classify_login_response(
            StatusCode::OK,
            &url,
            r#"<html><body><script>setMessage('身份认证成功');</script></body></html>"#,
        );
        assert!(matches!(result, LoginResponseClassification::OtherPage(_)));
    }

    #[test]
    fn does_not_accept_a_success_marker_from_an_unprofiled_route() {
        let client = client(FormEncoding::UrlEncoded);
        let url = Url::parse("https://id.example.test/ordinary-page").expect("fixture URL");
        let result = client.classify_login_response(
            StatusCode::OK,
            &url,
            "<html><body>登录成功，正在重定向</body></html>",
        );
        assert!(matches!(result, LoginResponseClassification::OtherPage(_)));
    }

    #[test]
    fn classifies_an_explicit_invalidation_marker() {
        let client = client(FormEncoding::UrlEncoded);
        let html = r#"
            <input name="loginInvalid" value="true" />
            <form action="/do/off/ui/auth/login/check">
              <input name="i_user" />
              <input name="i_pass" type="password" />
            </form>
        "#;
        let url = client.login_page_url().expect("page URL");
        let result = client.classify_login_response(StatusCode::OK, &url, html);
        let LoginResponseClassification::LoginFailed { evidence, page } = result else {
            panic!("an invalidation marker must classify the page as failed");
        };
        assert_eq!(evidence.reason, LoginFailureReason::SessionInvalid);
        assert_eq!(page.invalidation.status, InvalidationStatus::Marked);
        assert_eq!(page.invalidation.raw_value.as_deref(), Some("true"));
    }

    #[test]
    fn accepts_an_allowlisted_ticket_even_when_the_success_template_keeps_the_form() {
        let client = client(FormEncoding::UrlEncoded);
        let url = client.login_page_url().expect("page URL");
        let result = client.classify_login_response(StatusCode::OK, &url, REDACTED_LOGIN_FIXTURE);
        let LoginResponseClassification::AuthenticatedHandoff(page) = result else {
            panic!("an allowlisted service ticket must prove the handoff");
        };
        assert_eq!(
            page.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("TICKET_REDACTED")
        );
    }

    #[test]
    fn backend_repair_login_diagnosis_generic_rejection_still_precedes_stale_ticket_contract() {
        let client = client(FormEncoding::UrlEncoded);
        let url =
            Url::parse("https://id.example.test/b/learn?ticket=STALE_TICKET").expect("handoff URL");
        let result = client.classify_login_response(
            StatusCode::OK,
            &url,
            r#"<html><body><input name="loginError" value="true"><a href="/b/learn?ticket=STALE_TICKET">continue</a></body></html>"#,
        );
        let LoginResponseClassification::LoginFailed { evidence, .. } = result else {
            panic!("a failure marker must not be promoted by a stale ticket");
        };
        assert_eq!(evidence.reason, LoginFailureReason::Generic);
    }

    #[test]
    fn classifies_a_bare_redirect_location_as_a_callback_to_follow() {
        let client = client(FormEncoding::UrlEncoded);
        let final_url = client.login_page_url().expect("page URL");
        let location = final_url
            .join("/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let result = client.classify_login_response_with_location(
            StatusCode::FOUND,
            &final_url,
            Some(&location),
            "",
        );
        let LoginResponseClassification::RedirectCallback(page) = result else {
            panic!("a bare redirect2Jsp location must remain a followable callback");
        };
        let callback = page.anchor_ticket.expect("callback evidence");
        assert_eq!(callback.href, location.to_string());
        assert!(callback.ticket.is_none());
    }

    #[test]
    fn accepts_one_normalized_trailing_slash_on_the_identity_callback() {
        let client = client(FormEncoding::UrlEncoded);
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp/")
            .expect("callback URL");
        let result = client.classify_login_response(
            StatusCode::OK,
            &callback,
            "<html><body>identity callback complete</body></html>",
        );
        let LoginResponseClassification::RedirectCallback(page) = result else {
            panic!("a normalized identity callback must remain followable");
        };
        assert!(client.is_identity_callback_url(&callback));
        assert!(page.anchor_ticket.is_none());
    }

    #[test]
    fn does_not_turn_empty_identity_callback_targets_into_a_self_anchor() {
        let client = client(FormEncoding::UrlEncoded);
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let page = client.classify_login_response(
            StatusCode::OK,
            &callback,
            r#"
                <html><body>
                  登录成功。正在重定向到<a href="">直接跳转</a>
                  <script>window.location.replace("");</script>
                </body></html>
            "#,
        );
        let LoginResponseClassification::RedirectCallback(evidence) = page else {
            panic!("a clean identity callback must remain a callback classification");
        };
        assert!(evidence.anchor_ticket.is_none());
    }

    #[test]
    fn accepts_a_successful_non_200_response_with_an_allowlisted_ticket() {
        let client = client(FormEncoding::UrlEncoded);
        let final_url = Url::parse("https://id.example.test/b/learn?ticket=CREATED_TICKET")
            .expect("handoff URL");
        let result = client.classify_login_response(StatusCode::CREATED, &final_url, "");
        assert!(matches!(
            result,
            LoginResponseClassification::AuthenticatedHandoff(_)
        ));
    }

    #[test]
    fn rejects_plaintext_passwords_without_leaking_them() {
        let client = client(FormEncoding::UrlEncoded);
        let input = LoginFormInput::new(
            "student-redacted",
            PasswordInput::plaintext("PASSWORD_REDACTED"),
        );
        assert!(!format!("{input:?}").contains("PASSWORD_REDACTED"));
        let error = client
            .build_urlencoded_login_form(input)
            .expect_err("plaintext password must require an explicit crypto adapter");
        assert!(matches!(
            error,
            IdentityClientError::UnsupportedCrypto { ref algorithm, .. } if algorithm == "SM2"
        ));
        assert!(!format!("{error:?}").contains("PASSWORD_REDACTED"));
    }

    #[test]
    fn builds_a_borrowed_urlencoded_field_mapping_with_redacted_debug() {
        let client = client(FormEncoding::UrlEncoded);
        let input = LoginFormInput::new(
            "student redacted",
            PasswordInput::precomputed_wire("WIRE_PASSWORD_REDACTED"),
        )
        .with_device_name("THYou")
        .with_single_login("1");
        let form = client
            .build_urlencoded_login_form(input)
            .expect("wire form can be planned");
        assert_eq!(form.fields[0].name, "i_user");
        assert!(matches!(form.fields[1].value, FormValue::Sensitive(_)));
        assert_eq!(form.fields[2].name, "singleLogin");
        assert_eq!(form.fields[3].name, "deviceName");
        assert_eq!(
            form.encoded_body(),
            "i_user=student+redacted&i_pass=WIRE_PASSWORD_REDACTED&singleLogin=1&deviceName=THYou"
        );
        assert!(!format!("{form:?}").contains("WIRE_PASSWORD_REDACTED"));
    }

    #[test]
    fn includes_dynamic_hidden_fields_without_overriding_managed_login_fields() {
        let client = client(FormEncoding::UrlEncoded);
        let hidden_fields = [
            LoginFormHiddenField::new("target", "TARGET_REDACTED"),
            LoginFormHiddenField::new("SMRZSeq", "SEQUENCE_REDACTED"),
            LoginFormHiddenField::new("i_user", "STALE_USER_REDACTED"),
            LoginFormHiddenField::new("target", "DUPLICATE_REDACTED"),
        ];
        let input = LoginFormInput::new(
            "student redacted",
            PasswordInput::precomputed_wire("WIRE_PASSWORD_REDACTED"),
        )
        .with_hidden_fields(&hidden_fields)
        .with_device_name("THYou");
        let form = client
            .build_urlencoded_login_form(input)
            .expect("dynamic hidden fields can be encoded");

        assert_eq!(
            form.encoded_body(),
            "target=TARGET_REDACTED&SMRZSeq=SEQUENCE_REDACTED&i_user=student+redacted&i_pass=WIRE_PASSWORD_REDACTED&deviceName=THYou"
        );
        assert!(!format!("{input:?}").contains("TARGET_REDACTED"));
        assert!(!format!("{form:?}").contains("SEQUENCE_REDACTED"));
    }

    #[test]
    fn rejects_control_characters_in_dynamic_hidden_fields() {
        let client = client(FormEncoding::UrlEncoded);
        let hidden_fields = [LoginFormHiddenField::new("target", "bad\nvalue")];
        let input = LoginFormInput::new(
            "student",
            PasswordInput::precomputed_wire("WIRE_PASSWORD_REDACTED"),
        )
        .with_hidden_fields(&hidden_fields);
        assert!(matches!(
            client.build_urlencoded_login_form(input),
            Err(IdentityClientError::InvalidInput {
                field: "hidden_field",
                ..
            })
        ));
    }

    #[test]
    fn multipart_profiles_only_expose_a_bodyless_request_plan() {
        let client = client(FormEncoding::Multipart);
        let plan = client
            .login_submission_request_plan()
            .expect("multipart plan");
        let LoginSubmissionRequestPlan::Multipart(plan) = plan else {
            panic!("fixture profile must produce a multipart plan");
        };
        assert_eq!(plan.content_type, MULTIPART_CONTENT_TYPE);
        assert_eq!(plan.boundary, MultipartBoundaryPlan::GeneratedByTransport);
        assert_eq!(plan.field_names.password_field, "i_pass");
        assert_eq!(
            plan.url.as_str(),
            "https://id.example.test/do/off/ui/auth/login/check"
        );
    }

    #[test]
    fn builds_a_multipart_login_body_only_for_an_explicit_boundary() {
        let client = client(FormEncoding::Multipart);
        let input = LoginFormInput::new(
            "student",
            PasswordInput::precomputed_wire("WIRE_PASSWORD_REDACTED"),
        )
        .with_fingerprint("FINGERPRINT_REDACTED")
        .with_generated_fingerprint("FINGER3_REDACTED")
        .with_generated_fingerprint_v3("FINGER3_V3_REDACTED")
        .with_captcha("")
        .with_single_login("on");
        let form = client
            .build_multipart_login_form(input, "fixture-boundary")
            .expect("multipart form");

        assert_eq!(
            form.content_type(),
            "multipart/form-data; boundary=fixture-boundary"
        );
        let body = form.body();
        assert!(body.starts_with("--fixture-boundary\r\n"));
        assert!(body.contains("name=\"i_user\"\r\n\r\nstudent\r\n"));
        assert!(body.contains("name=\"i_pass\"\r\n\r\nWIRE_PASSWORD_REDACTED\r\n"));
        assert!(body.contains("name=\"fingerPrint\"\r\n\r\nFINGERPRINT_REDACTED\r\n"));
        assert!(body.ends_with("--fixture-boundary--\r\n"));
        let debug = format!("{form:?}");
        assert!(!debug.contains("WIRE_PASSWORD_REDACTED"));
        assert!(!debug.contains("FINGERPRINT_REDACTED"));
    }

    #[test]
    fn builds_save_finger_with_only_profile_confirmed_fields() {
        let profile = profile(FormEncoding::UrlEncoded).with_trusted_device(
            TrustedDeviceProfile::common().with_single_login("singleLogin", "yes"),
        );
        let client = IdentityClient::new(
            IdentityClientConfig::new("https://id.example.test/", profile)
                .expect("fixture configuration is valid"),
        )
        .expect("fixture client is valid");
        let form = client
            .build_trusted_device_form(
                TrustedDeviceInput::new("FINGERPRINT_REDACTED", "THYou desktop")
                    .with_single_login("yes"),
            )
            .expect("saveFinger form");
        let body = form.encoded_body();
        assert!(body.contains("fingerprint=FINGERPRINT_REDACTED"));
        assert!(body.contains("deviceName=THYou+desktop"));
        assert!(body.contains("radioVal=%E6%98%AF"));
        assert!(body.contains("singleLogin=yes"));
        let debug = format!("{form:?}");
        assert!(!debug.contains("FINGERPRINT_REDACTED"));
    }

    #[test]
    fn unsupported_trusted_device_profiles_fail_explicitly() {
        let client = client(FormEncoding::UrlEncoded);
        let profile = client.config().profile.clone().without_trusted_device();
        let client = IdentityClient::new(
            IdentityClientConfig::new("https://id.example.test/", profile)
                .expect("fixture configuration is valid"),
        )
        .expect("fixture client is valid");
        assert!(matches!(
            client.trusted_device_request_plan(),
            Err(IdentityClientError::UnsupportedTrustedDevice)
        ));
    }

    #[test]
    fn parses_sm2_key_from_a_script_assignment_without_copying_page_text() {
        let client = client(FormEncoding::UrlEncoded);
        let html = r#"
            <script>
              const sm2publicKey = "SCRIPT_PUBLIC_KEY_REDACTED";
            </script>
        "#;
        let evidence = client.parse_login_page(html);
        assert_eq!(
            evidence
                .sm2_public_key
                .as_ref()
                .map(|key| key.value.as_str()),
            Some("SCRIPT_PUBLIC_KEY_REDACTED")
        );
    }

    #[test]
    fn parses_sm2_key_from_the_element_text_used_by_current_login_pages() {
        let client = client(FormEncoding::UrlEncoded);
        let evidence = client
            .parse_login_page(r#"<div id="sm2publicKey"> ELEMENT_PUBLIC_KEY_REDACTED </div>"#);
        assert_eq!(
            evidence
                .sm2_public_key
                .as_ref()
                .map(|key| key.value.as_str()),
            Some("ELEMENT_PUBLIC_KEY_REDACTED")
        );
    }

    #[test]
    fn preserves_unknown_invalidation_evidence_without_calling_it_success() {
        let client = client(FormEncoding::UrlEncoded);
        let evidence = client.parse_login_page(
            r#"<input name="loginInvalid" value="pending" /><form action="/do/off/ui/auth/login/check"></form>"#,
        );
        assert_eq!(evidence.invalidation.status, InvalidationStatus::Unknown);
        assert!(evidence.failure.is_none());
    }
}
