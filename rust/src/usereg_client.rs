//! Execution and HTML response mapping for the USEREG self-service portal.
//!
//! [`crate::usereg`] owns the USEREG wire profile and creates the five
//! operation-specific request plans.  This module supplies the deployment
//! origin and executes those plans through a caller-provided
//! [`CampusHttpTransport`].  The transport's `reqwest::Client` is used
//! directly, which preserves its Cookie jar and shared same-origin redirect
//! policy.
//!
//! This layer does not transform passwords or invent an encryption scheme.
//! A caller hands it a plan created by the profile, and the plan is sent as
//! supplied.  HTML responses are mapped conservatively: HTTP 200 is not a
//! login success, and the presence of a login page or an explicit failure
//! marker is kept as a separate state.

use std::{fmt, str};

use reqwest::{
    Method, StatusCode, Url,
    header::{HeaderName, HeaderValue},
};
use thiserror::Error;

use crate::transport::{CampusHttpTransport, TransportError};
use crate::usereg::{
    UseregCertificationInput, UseregCsrfToken, UseregDeviceTarget, UseregError, UseregHttpMethod,
    UseregLoginCredentials, UseregProfile, UseregRequestPlan, is_safe_route_path,
};

const DEFAULT_CSRF_FIELD: &str = "_csrf-8800";
const DEFAULT_LOGIN_PATH_HINTS: &[&str] = &["/login", "/site/login", "/auth/login"];
const DEFAULT_EXPIRED_MARKERS: &[&str] = &[
    "登录失效",
    "登录已失效",
    "会话已过期",
    "会话已失效",
    "请先登录",
    "session expired",
    "session invalid",
    "login required",
    "authentication required",
];
const DEFAULT_FAILURE_MARKERS: &[&str] = &[
    "用户名或密码错误",
    "账号或密码错误",
    "验证码错误",
    "invalid credentials",
    "invalid username or password",
    "login failed",
];

/// Errors raised while configuring, executing, or mapping a USEREG request.
/// No variant stores the response body, password, CSRF value, or device
/// identifier, so displaying this error cannot echo request secrets.
#[derive(Debug, Error)]
pub enum UseregClientError {
    #[error("invalid USEREG base URL: {0}")]
    InvalidBaseUrl(String),

    #[error("invalid USEREG client configuration: {0}")]
    InvalidConfig(String),

    #[error("USEREG response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error(transparent)]
    Profile(#[from] UseregError),

    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error("USEREG request could not be built: {0}")]
    RequestBuild(String),

    #[error("USEREG request plan is invalid: {0}")]
    InvalidRequestPlan(String),

    #[error("USEREG request header is invalid: {name}: {message}")]
    InvalidHeader { name: String, message: String },

    #[error("USEREG response returned HTTP status {status} at {url}")]
    HttpStatus { status: StatusCode, url: String },

    #[error("USEREG response body is not valid UTF-8: {message}")]
    InvalidUtf8 { message: String },

    #[error(transparent)]
    Csrf(#[from] UseregCsrfParseError),
}

/// Coarse classification for callers which need to keep transport failures,
/// malformed protocol data, and service-session failures separate.  The
/// concrete error remains available for diagnostics, but callers should not
/// have to match human-readable error text to decide whether a login can be
/// retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregClientErrorClass {
    Configuration,
    Transport,
    Http,
    Session,
    Response,
}

impl UseregClientError {
    pub fn class(&self) -> UseregClientErrorClass {
        match self {
            Self::InvalidBaseUrl(_)
            | Self::InvalidConfig(_)
            | Self::Profile(_)
            | Self::RequestBuild(_)
            | Self::InvalidRequestPlan(_)
            | Self::InvalidHeader { .. } => UseregClientErrorClass::Configuration,
            Self::Transport(error) => match error {
                TransportError::HttpStatus { .. } => UseregClientErrorClass::Http,
                TransportError::Request(_)
                | TransportError::Decode(_)
                | TransportError::DecodeBody { .. }
                | TransportError::Jsonp(_) => UseregClientErrorClass::Transport,
                TransportError::InvalidUrl(_) => UseregClientErrorClass::Configuration,
            },
            Self::HttpStatus { status, .. }
                if *status == StatusCode::UNAUTHORIZED || *status == StatusCode::FORBIDDEN =>
            {
                UseregClientErrorClass::Session
            }
            Self::HttpStatus { .. } => UseregClientErrorClass::Http,
            Self::UnexpectedOrigin
            | Self::InvalidUtf8 { .. }
            | Self::Csrf(UseregCsrfParseError::Missing)
            | Self::Csrf(UseregCsrfParseError::MalformedTag { .. })
            | Self::Csrf(UseregCsrfParseError::MissingAttribute { .. })
            | Self::Csrf(UseregCsrfParseError::EmptyAttribute { .. })
            | Self::Csrf(UseregCsrfParseError::InvalidInputType)
            | Self::Csrf(UseregCsrfParseError::InvalidToken)
            | Self::Csrf(UseregCsrfParseError::ConflictingTokens)
            | Self::Csrf(UseregCsrfParseError::InvalidFieldName)
            | Self::Csrf(UseregCsrfParseError::DuplicateAttribute { .. }) => {
                UseregClientErrorClass::Response
            }
        }
    }

    pub fn is_session_failure(&self) -> bool {
        self.class() == UseregClientErrorClass::Session
    }
}

/// Configuration which belongs to one USEREG deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregClientConfig {
    pub base_url: Url,
    pub profile: UseregProfile,
    pub login_path_hints: Vec<String>,
    pub expired_markers: Vec<String>,
    pub failure_markers: Vec<String>,
}

impl UseregClientConfig {
    pub fn new(
        base_url: impl Into<String>,
        profile: UseregProfile,
    ) -> Result<Self, UseregClientError> {
        let base_url_text = base_url.into();
        let base_url = normalize_base_url(&base_url_text)?;
        validate_profile(&profile)?;
        let config = Self {
            base_url,
            profile,
            login_path_hints: DEFAULT_LOGIN_PATH_HINTS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            expired_markers: DEFAULT_EXPIRED_MARKERS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            failure_markers: DEFAULT_FAILURE_MARKERS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        };
        config.validate_markers()?;
        Ok(config)
    }

    pub fn with_login_path_hints(
        mut self,
        hints: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, UseregClientError> {
        self.login_path_hints = hints.into_iter().map(Into::into).collect();
        self.validate_markers()?;
        Ok(self)
    }

    pub fn with_expired_markers(
        mut self,
        markers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, UseregClientError> {
        self.expired_markers = markers.into_iter().map(Into::into).collect();
        self.validate_markers()?;
        Ok(self)
    }

    pub fn with_failure_markers(
        mut self,
        markers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, UseregClientError> {
        self.failure_markers = markers.into_iter().map(Into::into).collect();
        self.validate_markers()?;
        Ok(self)
    }

    fn validate_markers(&self) -> Result<(), UseregClientError> {
        for value in self
            .login_path_hints
            .iter()
            .chain(self.expired_markers.iter())
            .chain(self.failure_markers.iter())
        {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(UseregClientError::InvalidConfig(
                    "USEREG response markers must not be empty or contain control characters"
                        .to_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// A USEREG client which owns deployment configuration only.  Credentials,
/// CSRF values, device values, and cookies live in the short-lived request
/// plan or the caller's transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregClient {
    config: UseregClientConfig,
}

impl UseregClient {
    pub fn new(config: UseregClientConfig) -> Result<Self, UseregClientError> {
        validate_profile(&config.profile)?;
        validate_base_url(&config.base_url)?;
        config.validate_markers()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &UseregClientConfig {
        &self.config
    }

    pub fn profile(&self) -> &UseregProfile {
        &self.config.profile
    }

    pub fn captcha_request_plan(
        &self,
        refresh: bool,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.captcha_request(refresh)?)
    }

    pub fn login_page_request_plan(&self) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.login_page_request()?)
    }

    pub fn validate_user_request_plan(
        &self,
        csrf_header_value: &str,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.validate_user_request(
            csrf_header_value,
            credentials,
            verify_code,
        )?)
    }

    pub fn login_request_plan(
        &self,
        csrf: &UseregCsrfToken,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
        sms_code: Option<&str>,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.login_request(
            csrf,
            credentials,
            verify_code,
            sms_code,
        )?)
    }

    pub fn home_delete_request_plan(
        &self,
        csrf: &UseregCsrfToken,
        device: &UseregDeviceTarget,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.home_delete_request(csrf, device)?)
    }

    pub fn certification_request_plan(
        &self,
        csrf: &UseregCsrfToken,
        input: &UseregCertificationInput,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.certification_request(csrf, input)?)
    }

    pub fn certification_page_request_plan(
        &self,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        self.resolve_request(self.config.profile.certification_page_request()?)
    }

    /// Resolves an already-created profile plan against this deployment.
    /// Keeping this method public lets a caller retain a plan from a custom
    /// profile while still enforcing the configured origin at the execution
    /// boundary.
    pub fn resolve_request(
        &self,
        request: UseregRequestPlan,
    ) -> Result<UseregExecutionPlan, UseregClientError> {
        let endpoint = self.resolve_path(&request.path)?;
        let method = match request.method {
            UseregHttpMethod::Get => Method::GET,
            UseregHttpMethod::Post => Method::POST,
        };
        Ok(UseregExecutionPlan {
            method,
            endpoint,
            request,
        })
    }

    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
        plan: &UseregExecutionPlan,
    ) -> Result<reqwest::Request, UseregClientError> {
        plan.build_request(transport)
    }

    /// Executes a plan with the shared cookie-aware client.  All HTTP status
    /// codes are retained in the response so the HTML mapper can distinguish
    /// a final 200 login page from a transport/status failure.
    pub async fn execute(
        &self,
        transport: &CampusHttpTransport,
        plan: &UseregExecutionPlan,
    ) -> Result<UseregHttpResponse, UseregClientError> {
        let request = plan.build_request(transport)?;
        let response = transport
            .execute(request)
            .await
            .map_err(|error| UseregClientError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_bytes(response)
            .await
            .map_err(|error| UseregClientError::Transport(TransportError::Decode(error)))?
            .to_vec();
        Ok(UseregHttpResponse {
            status,
            final_url,
            content_type,
            body,
        })
    }

    pub async fn execute_success(
        &self,
        transport: &CampusHttpTransport,
        plan: &UseregExecutionPlan,
    ) -> Result<UseregHttpResponse, UseregClientError> {
        let response = self.execute(transport, plan).await?;
        self.ensure_response_origin(&response)?;
        response.ensure_success()?;
        Ok(response)
    }

    /// Fetches a captcha without decoding binary image data as text.  The
    /// returned response retains the content type and bytes for the caller.
    pub async fn captcha(
        &self,
        transport: &CampusHttpTransport,
        refresh: bool,
    ) -> Result<UseregHttpResponse, UseregClientError> {
        let plan = self.captcha_request_plan(refresh)?;
        self.execute_success(transport, &plan).await
    }

    /// Maps an HTTP response without assuming that a 200 page means a login
    /// succeeded.  JSON and binary responses remain `NonHtml`; no unverified
    /// USEREG business schema is invented here.
    pub fn map_response(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<UseregMappedResponse, UseregClientError> {
        self.ensure_response_origin(response)?;
        if response.content_type.is_none() {
            return Ok(UseregMappedResponse {
                status: response.status,
                final_url: response.final_url.clone(),
                csrf: None,
                classification: UseregPageClassification::NonHtml,
            });
        }
        let body = match response.body_text() {
            Ok(body) => body,
            Err(_) => {
                return Ok(UseregMappedResponse {
                    status: response.status,
                    final_url: response.final_url.clone(),
                    csrf: None,
                    classification: UseregPageClassification::NonHtml,
                });
            }
        };
        self.map_html_response_with_content_type(
            response.status,
            &response.final_url,
            response.content_type.as_deref(),
            body,
        )
    }

    /// Maps a text response with the exact HTTP-200 HTML boundary.  This
    /// method is useful for fixture tests and callers which already decoded a
    /// response body; `map_response` is the normal transport-facing entry.
    pub fn map_html_response(
        &self,
        status: StatusCode,
        final_url: &Url,
        body: &str,
    ) -> Result<UseregMappedResponse, UseregClientError> {
        self.map_html_response_with_content_type(status, final_url, None, body)
    }

    pub fn is_login_required(
        &self,
        status: StatusCode,
        final_url: &Url,
        body: &str,
    ) -> Result<bool, UseregClientError> {
        Ok(matches!(
            self.map_html_response(status, final_url, body)?
                .classification,
            UseregPageClassification::LoginRequired(_) | UseregPageClassification::LoginFailed(_)
        ))
    }

    pub fn extract_csrf(&self, html: &str) -> Result<UseregCsrfEvidence, UseregClientError> {
        extract_csrf_with_field(html, &self.config.profile.fields.csrf_form_field)
            .map_err(UseregClientError::Csrf)
    }

    pub(crate) fn ensure_response_origin(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<(), UseregClientError> {
        if same_origin(&self.config.base_url, &response.final_url) {
            Ok(())
        } else {
            Err(UseregClientError::UnexpectedOrigin)
        }
    }

    fn map_html_response_with_content_type(
        &self,
        status: StatusCode,
        final_url: &Url,
        content_type: Option<&str>,
        body: &str,
    ) -> Result<UseregMappedResponse, UseregClientError> {
        if !same_origin(&self.config.base_url, final_url) {
            return Err(UseregClientError::UnexpectedOrigin);
        }
        if !status.is_success() {
            return Ok(UseregMappedResponse {
                status,
                final_url: final_url.clone(),
                csrf: None,
                classification: UseregPageClassification::NonSuccess,
            });
        }

        if !looks_like_html(content_type, body) {
            return Ok(UseregMappedResponse {
                status,
                final_url: final_url.clone(),
                csrf: None,
                classification: UseregPageClassification::NonHtml,
            });
        }

        let csrf = match self.extract_csrf(body) {
            Ok(value) => Some(value),
            Err(UseregClientError::Csrf(UseregCsrfParseError::Missing)) => None,
            Err(error) => return Err(error),
        };
        let lower_body = body.to_ascii_lowercase();
        let mut signals = Vec::new();
        if url_matches_hint(final_url, &self.config.login_path_hints) {
            signals.push(UseregHtmlSignal::FinalUrlMatchesLogin);
        }
        if contains_login_form(
            &lower_body,
            &self.config.profile.fields.login_username_field,
            &self.config.profile.fields.login_password_field,
        ) {
            signals.push(UseregHtmlSignal::LoginForm);
        }
        if self
            .config
            .expired_markers
            .iter()
            .any(|marker| lower_body.contains(&marker.to_ascii_lowercase()))
        {
            signals.push(UseregHtmlSignal::ExplicitExpiredMarker);
        }
        if self
            .config
            .failure_markers
            .iter()
            .any(|marker| lower_body.contains(&marker.to_ascii_lowercase()))
        {
            signals.push(UseregHtmlSignal::FailureMarker);
        }

        let evidence = UseregHtmlEvidence { signals };
        let classification = if evidence.signals.contains(&UseregHtmlSignal::FailureMarker) {
            UseregPageClassification::LoginFailed(evidence.clone())
        } else if evidence.signals.iter().any(|signal| {
            matches!(
                signal,
                UseregHtmlSignal::FinalUrlMatchesLogin
                    | UseregHtmlSignal::LoginForm
                    | UseregHtmlSignal::ExplicitExpiredMarker
            )
        }) {
            UseregPageClassification::LoginRequired(evidence.clone())
        } else {
            UseregPageClassification::HtmlUnknown(evidence.clone())
        };

        Ok(UseregMappedResponse {
            status,
            final_url: final_url.clone(),
            csrf,
            classification,
        })
    }

    fn resolve_path(&self, path: &str) -> Result<Url, UseregClientError> {
        if !path.starts_with('/')
            || path.contains(['?', '#'])
            || path.chars().any(char::is_control)
            || !is_safe_route_path(path)
        {
            return Err(UseregClientError::InvalidRequestPlan(
                "USEREG route path is invalid".to_owned(),
            ));
        }
        // USEREG is normally reached through a WebVPN mapped directory.  A
        // leading slash in `path` is relative to that mapped directory, not
        // to the WebVPN origin. `Url::join` would discard the mapping prefix
        // and send `/login` to WebVPN itself, which then returns the generic
        // portal page and makes every subsequent login look unavailable.
        let mut endpoint = self.config.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        if !same_origin(&self.config.base_url, &endpoint) {
            return Err(UseregClientError::InvalidRequestPlan(
                "USEREG endpoint must stay on the configured origin".to_owned(),
            ));
        }
        Ok(endpoint)
    }

    pub(crate) fn response_matches_plan(
        &self,
        response: &UseregHttpResponse,
        plan: &UseregExecutionPlan,
    ) -> bool {
        same_origin(&self.config.base_url, &response.final_url)
            && response.final_url.path() == plan.endpoint.path()
            && normalized_query(&response.final_url) == normalized_query_pairs(&plan.request.query)
    }

    /// Check that a response stayed on one of the configured service routes.
    /// This is intentionally narrower than an origin check: a same-origin
    /// error page or an unrelated service page must not be allowed to satisfy
    /// the authenticated-home proof merely because it contains similar HTML.
    pub(crate) fn response_matches_profile_route(
        &self,
        response: &UseregHttpResponse,
        path: &str,
    ) -> bool {
        let Ok(endpoint) = self.resolve_path(path) else {
            return false;
        };
        same_origin(&self.config.base_url, &response.final_url)
            && response.final_url.path() == endpoint.path()
            && response.final_url.query().is_none()
            && response.final_url.fragment().is_none()
    }
}

/// A request plan resolved against the configured USEREG origin.
#[derive(Clone, PartialEq, Eq)]
pub struct UseregExecutionPlan {
    pub method: Method,
    pub endpoint: Url,
    pub request: UseregRequestPlan,
}

impl fmt::Debug for UseregExecutionPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregExecutionPlan")
            .field("method", &self.method)
            .field("endpoint", &diagnostic_url(&self.endpoint))
            .field("request", &self.request)
            .finish()
    }
}

impl UseregExecutionPlan {
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, UseregClientError> {
        if self.request.method == UseregHttpMethod::Get && !self.request.form.is_empty() {
            return Err(UseregClientError::InvalidRequestPlan(
                "GET USEREG requests must not contain form fields".to_owned(),
            ));
        }

        let mut builder = transport
            .client()
            .request(self.method.clone(), self.endpoint.clone());
        if !self.request.query.is_empty() {
            builder = builder.query(&self.request.query);
        }
        for (name, value) in &self.request.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                UseregClientError::InvalidHeader {
                    name: name.clone(),
                    message: error.to_string(),
                }
            })?;
            let header_value =
                HeaderValue::from_str(value).map_err(|error| UseregClientError::InvalidHeader {
                    name: name.clone(),
                    message: error.to_string(),
                })?;
            builder = builder.header(header_name, header_value);
        }
        if self.request.method == UseregHttpMethod::Post {
            builder = builder.form(&self.request.form);
        }
        builder
            .build()
            .map_err(|error| UseregClientError::RequestBuild(error.to_string()))
    }
}

/// Raw response data.  The body is available to the parser and caller, but
/// its `Debug` output reports only its length.
#[derive(Clone, PartialEq, Eq)]
pub struct UseregHttpResponse {
    pub status: StatusCode,
    pub final_url: Url,
    pub content_type: Option<String>,
    body: Vec<u8>,
}

impl UseregHttpResponse {
    #[cfg(test)]
    pub(crate) fn fixture(
        status: StatusCode,
        final_url: &str,
        content_type: &str,
        body: &[u8],
    ) -> Self {
        Self {
            status,
            final_url: Url::parse(final_url).expect("fixture URL"),
            content_type: Some(content_type.to_owned()),
            body: body.to_vec(),
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_without_content_type(
        status: StatusCode,
        final_url: &str,
        body: &[u8],
    ) -> Self {
        Self {
            status,
            final_url: Url::parse(final_url).expect("fixture URL"),
            content_type: None,
            body: body.to_vec(),
        }
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn body_text(&self) -> Result<&str, UseregClientError> {
        str::from_utf8(&self.body).map_err(|error| UseregClientError::InvalidUtf8 {
            message: error.to_string(),
        })
    }

    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    pub fn ensure_success(&self) -> Result<(), UseregClientError> {
        if self.status.is_success() {
            Ok(())
        } else {
            Err(UseregClientError::HttpStatus {
                status: self.status,
                url: diagnostic_url(&self.final_url),
            })
        }
    }
}

impl fmt::Debug for UseregHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregHttpResponse")
            .field("status", &self.status)
            .field("final_url", &diagnostic_url(&self.final_url))
            .field("content_type", &self.content_type)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregHtmlSignal {
    FinalUrlMatchesLogin,
    LoginForm,
    ExplicitExpiredMarker,
    FailureMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregHtmlEvidence {
    pub signals: Vec<UseregHtmlSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UseregPageClassification {
    NonSuccess,
    LoginRequired(UseregHtmlEvidence),
    LoginFailed(UseregHtmlEvidence),
    HtmlUnknown(UseregHtmlEvidence),
    NonHtml,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UseregMappedResponse {
    pub status: StatusCode,
    pub final_url: Url,
    pub csrf: Option<UseregCsrfEvidence>,
    pub classification: UseregPageClassification,
}

impl fmt::Debug for UseregMappedResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregMappedResponse")
            .field("status", &self.status)
            .field("final_url", &diagnostic_url(&self.final_url))
            .field("csrf", &self.csrf)
            .field("classification", &self.classification)
            .finish()
    }
}

impl UseregMappedResponse {
    pub fn is_login_required(&self) -> bool {
        matches!(
            self.classification,
            UseregPageClassification::LoginRequired(_) | UseregPageClassification::LoginFailed(_)
        )
    }
}

/// The only CSRF source asserted by the current USEREG profile: a hidden
/// input carrying the profile's form field name.
#[derive(Clone, PartialEq, Eq)]
pub struct UseregCsrfEvidence {
    pub field_name: String,
    pub token: UseregCsrfToken,
}

impl UseregCsrfEvidence {
    pub fn as_str(&self) -> &str {
        self.token.as_str()
    }
}

impl fmt::Debug for UseregCsrfEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregCsrfEvidence")
            .field("field_name", &self.field_name)
            .field("token", &self.token)
            .finish()
    }
}

/// Strict errors from the USEREG hidden-input CSRF parser.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UseregCsrfParseError {
    #[error("no USEREG CSRF input was found")]
    Missing,

    #[error("malformed USEREG input tag: {message}")]
    MalformedTag { message: String },

    #[error("USEREG CSRF input is missing its {attribute} attribute")]
    MissingAttribute { attribute: &'static str },

    #[error("USEREG CSRF input has an empty {attribute} attribute")]
    EmptyAttribute { attribute: &'static str },

    #[error("USEREG CSRF input must have type=hidden")]
    InvalidInputType,

    #[error("USEREG CSRF token contains whitespace or a control character")]
    InvalidToken,

    #[error("USEREG CSRF inputs contain different token values")]
    ConflictingTokens,

    #[error("USEREG CSRF field name must not be empty or contain whitespace")]
    InvalidFieldName,

    #[error("USEREG input tag contains a duplicate {attribute} attribute")]
    DuplicateAttribute { attribute: String },
}

pub fn extract_csrf(html: &str) -> Result<UseregCsrfEvidence, UseregCsrfParseError> {
    extract_csrf_with_field(html, DEFAULT_CSRF_FIELD)
}

pub fn extract_csrf_with_field(
    html: &str,
    field_name: &str,
) -> Result<UseregCsrfEvidence, UseregCsrfParseError> {
    if field_name.trim().is_empty()
        || field_name
            .chars()
            .any(|character| character.is_whitespace())
    {
        return Err(UseregCsrfParseError::InvalidFieldName);
    }

    let tags = scan_input_tags(html)?;
    let mut token: Option<String> = None;
    for tag in tags {
        let Some(name) = tag.attribute("name") else {
            continue;
        };
        if name != field_name {
            continue;
        }
        let input_type = tag
            .attribute("type")
            .ok_or(UseregCsrfParseError::InvalidInputType)?;
        if !input_type.eq_ignore_ascii_case("hidden") {
            return Err(UseregCsrfParseError::InvalidInputType);
        }
        let value = tag
            .attribute("value")
            .ok_or(UseregCsrfParseError::MissingAttribute { attribute: "value" })?;
        let value = validate_token(value)?;
        if token.as_deref().is_some_and(|existing| existing != value) {
            return Err(UseregCsrfParseError::ConflictingTokens);
        }
        token = Some(value.to_owned());
    }

    let token = token.ok_or(UseregCsrfParseError::Missing)?;
    let token = UseregCsrfToken::new(token).map_err(|_| UseregCsrfParseError::InvalidToken)?;
    Ok(UseregCsrfEvidence {
        field_name: field_name.to_owned(),
        token,
    })
}

fn normalize_base_url(base_url: &str) -> Result<Url, UseregClientError> {
    let url = Url::parse(base_url)
        .map_err(|error| UseregClientError::InvalidBaseUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must use http or https".to_owned(),
        ));
    }
    if url.host_str().is_none() {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must contain a host".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must not contain userinfo".to_owned(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must not contain a query or fragment".to_owned(),
        ));
    }
    Ok(url)
}

fn validate_base_url(url: &Url) -> Result<(), UseregClientError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must use http or https".to_owned(),
        ));
    }
    if url.host_str().is_none() {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must contain a host".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must not contain userinfo".to_owned(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(UseregClientError::InvalidBaseUrl(
            "USEREG base URL must not contain a query or fragment".to_owned(),
        ));
    }
    Ok(())
}

fn same_origin(base: &Url, candidate: &Url) -> bool {
    base.scheme() == candidate.scheme()
        && base.host_str() == candidate.host_str()
        && base.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

/// Compare query parameters as decoded name/value pairs.  The request plan
/// is the source of truth; the order is not significant because query
/// parameters are a map-like part of the HTTP target, while duplicate pairs
/// are retained.  This prevents a redirect from silently changing a captcha
/// refresh marker, image cache buster, or device identity.
fn normalized_query(url: &Url) -> Vec<(String, String)> {
    let mut query = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    query.sort();
    query
}

fn normalized_query_pairs(query: &[(String, String)]) -> Vec<(String, String)> {
    let mut query = query.to_vec();
    query.sort();
    query
}

fn diagnostic_url(url: &Url) -> String {
    let mut url = url.clone();
    // A WebVPN mapping prefix is an opaque session value. Keep diagnostics
    // useful enough to identify the origin, but never print that path (or any
    // query/fragment/userinfo that may carry a handoff value).
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

fn validate_profile(profile: &UseregProfile) -> Result<(), UseregClientError> {
    profile
        .captcha_request(false)
        .map(|_| ())
        .map_err(UseregClientError::Profile)
}

fn url_matches_hint(url: &Url, hints: &[String]) -> bool {
    let path = url.path().to_ascii_lowercase();
    hints.iter().any(|hint| {
        let hint = hint.trim().to_ascii_lowercase();
        if hint.is_empty() || hint.contains(['?', '#']) {
            return false;
        }
        let hint = if hint.starts_with('/') {
            hint
        } else {
            format!("/{hint}")
        };
        path == hint
            || path
                .strip_suffix(&hint)
                .is_some_and(|prefix| prefix.ends_with('/'))
    })
}

fn contains_login_form(body: &str, username_field: &str, password_field: &str) -> bool {
    contains_input_name(body, username_field) && contains_input_name(body, password_field)
}

fn contains_input_name(body: &str, field_name: &str) -> bool {
    let field_name = field_name.to_ascii_lowercase();
    [
        format!("name=\"{field_name}\""),
        format!("name='{field_name}'"),
        format!("name={field_name}"),
    ]
    .iter()
    .any(|candidate| body.contains(candidate))
}

fn looks_like_html(content_type: Option<&str>, body: &str) -> bool {
    if let Some(content_type) = content_type {
        let media_type = content_type
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        return matches!(media_type.as_str(), "text/html" | "application/xhtml+xml");
    }
    let lower = body.trim_start().to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.starts_with("<head")
        || lower.starts_with("<body")
        || lower.contains("<form")
        || lower.contains("<input")
        || lower.contains("<meta")
}

#[derive(Debug, Clone)]
struct HtmlTag {
    attributes: Vec<(String, Option<String>)>,
}

impl HtmlTag {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(attribute, _)| attribute == name)
            .and_then(|(_, value)| value.as_deref())
    }
}

fn scan_input_tags(html: &str) -> Result<Vec<HtmlTag>, UseregCsrfParseError> {
    let lower = html.to_ascii_lowercase();
    let mut tags = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = lower[cursor..].find('<') {
        let start = cursor + relative_start;
        if lower[start..].starts_with("<!--") {
            let Some(end) = lower[start + 4..].find("-->") else {
                return Ok(tags);
            };
            cursor = start + 4 + end + 3;
            continue;
        }

        if let Some(raw_name) = raw_block_name(&lower[start..]) {
            let opening_end =
                find_tag_end(html, start).ok_or_else(|| UseregCsrfParseError::MalformedTag {
                    message: format!("unterminated {raw_name} tag"),
                })?;
            let closing = format!("</{raw_name}");
            let Some(relative_close) = lower[opening_end + 1..].find(&closing) else {
                return Ok(tags);
            };
            cursor = opening_end + 1 + relative_close + closing.len();
            continue;
        }

        if !starts_input_tag(&lower[start..]) {
            cursor = start + 1;
            continue;
        }

        let end = find_tag_end(html, start).ok_or_else(|| UseregCsrfParseError::MalformedTag {
            message: "unterminated input tag".to_owned(),
        })?;
        tags.push(parse_input_tag(&html[start + 1..end])?);
        cursor = end + 1;
    }

    Ok(tags)
}

fn starts_input_tag(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("<input") else {
        return false;
    };
    rest.chars()
        .next()
        .is_none_or(|character| character.is_ascii_whitespace() || matches!(character, '/' | '>'))
}

fn raw_block_name(value: &str) -> Option<&'static str> {
    for name in ["<script", "<style"] {
        if let Some(rest) = value.strip_prefix(name) {
            if rest
                .chars()
                .next()
                .is_none_or(|character| character.is_ascii_whitespace() || character == '>')
            {
                return Some(&name[1..]);
            }
        }
    }
    None
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in html[start..].char_indices() {
        match (quote, character) {
            (Some(expected), value) if value == expected => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn parse_input_tag(contents: &str) -> Result<HtmlTag, UseregCsrfParseError> {
    let mut position;
    let mut attributes = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    let tag_name_end = contents
        .char_indices()
        .find(|(_, character)| character.is_ascii_whitespace() || *character == '/')
        .map_or(contents.len(), |(index, _)| index);
    if !contents[..tag_name_end].eq_ignore_ascii_case("input") {
        return Err(UseregCsrfParseError::MalformedTag {
            message: "expected input tag".to_owned(),
        });
    }
    position = tag_name_end;

    while position < contents.len() {
        while position < contents.len() && contents.as_bytes()[position].is_ascii_whitespace() {
            position += 1;
        }
        if position >= contents.len() || contents.as_bytes()[position] == b'/' {
            break;
        }

        let name_start = position;
        while position < contents.len()
            && !contents.as_bytes()[position].is_ascii_whitespace()
            && !matches!(contents.as_bytes()[position], b'=' | b'/')
        {
            position += 1;
        }
        if name_start == position {
            return Err(UseregCsrfParseError::MalformedTag {
                message: "attribute name is empty".to_owned(),
            });
        }
        let name = contents[name_start..position].to_ascii_lowercase();
        if !seen.insert(name.clone()) {
            return Err(UseregCsrfParseError::DuplicateAttribute { attribute: name });
        }

        while position < contents.len() && contents.as_bytes()[position].is_ascii_whitespace() {
            position += 1;
        }
        let value = if position < contents.len() && contents.as_bytes()[position] == b'=' {
            position += 1;
            while position < contents.len() && contents.as_bytes()[position].is_ascii_whitespace() {
                position += 1;
            }
            if position >= contents.len() {
                return Err(UseregCsrfParseError::MalformedTag {
                    message: format!("attribute {name} has no value"),
                });
            }
            let first = contents.as_bytes()[position];
            if matches!(first, b'\'' | b'"') {
                let quote = first as char;
                position += 1;
                let value_start = position;
                while position < contents.len() && contents.as_bytes()[position] != first {
                    position += 1;
                }
                if position >= contents.len() {
                    return Err(UseregCsrfParseError::MalformedTag {
                        message: format!(
                            "attribute {name} has an unterminated {quote}-quoted value"
                        ),
                    });
                }
                let value = contents[value_start..position].to_owned();
                position += 1;
                Some(value)
            } else {
                let value_start = position;
                while position < contents.len()
                    && !contents.as_bytes()[position].is_ascii_whitespace()
                    && contents.as_bytes()[position] != b'/'
                {
                    position += 1;
                }
                Some(contents[value_start..position].to_owned())
            }
        } else {
            None
        };
        attributes.push((name, value));
    }

    Ok(HtmlTag { attributes })
}

fn validate_token(value: &str) -> Result<&str, UseregCsrfParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(UseregCsrfParseError::EmptyAttribute { attribute: "value" });
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(UseregCsrfParseError::InvalidToken);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usereg::UseregOperation;

    const USERNAME: &str = "20260001@example.edu.cn";
    const PASSWORD: &str = "wire-password-fixture";
    const CSRF: &str = "csrf-fixture-value";

    fn client() -> UseregClient {
        let config =
            UseregClientConfig::new("https://usereg.example.test/", UseregProfile::default())
                .expect("config");
        UseregClient::new(config).expect("client")
    }

    #[test]
    fn resolves_all_profile_operations_against_one_origin() {
        let client = client();
        let captcha = client.captcha_request_plan(true).expect("captcha");
        assert_eq!(captcha.method, Method::GET);
        assert_eq!(captcha.endpoint.path(), "/site/captcha");
        assert_eq!(
            captcha.request.query,
            vec![("refresh".to_owned(), "1".to_owned())]
        );

        let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
        let csrf = UseregCsrfToken::new(CSRF).expect("CSRF");
        let login = client
            .login_request_plan(&csrf, &credentials, "ABCD", Some("123456"))
            .expect("login");
        assert_eq!(login.endpoint.path(), "/login");
        assert_eq!(login.request.form.len(), 5);
    }

    #[test]
    fn resolves_routes_under_a_webvpn_mapping_prefix() {
        let config = UseregClientConfig::new(
            "https://webvpn.example.test/https/opaque-usereg-mapping/",
            UseregProfile::default(),
        )
        .expect("config");
        let client = UseregClient::new(config).expect("client");

        let login_request = client
            .profile()
            .login_page_request()
            .expect("login page request");
        let login = client.resolve_request(login_request).expect("login page");
        assert_eq!(
            login.endpoint.as_str(),
            "https://webvpn.example.test/https/opaque-usereg-mapping/login"
        );
        let captcha = client.captcha_request_plan(true).expect("captcha");
        assert_eq!(
            captcha.endpoint.as_str(),
            "https://webvpn.example.test/https/opaque-usereg-mapping/site/captcha"
        );
        assert_eq!(
            format!("{login:?}").contains("opaque-usereg-mapping"),
            false,
            "request-plan diagnostics must not expose the opaque mapping"
        );
    }

    #[test]
    fn rejects_route_escape_segments_before_webvpn_mapping_is_joined() {
        let client = client();
        for path in [
            "/../login",
            "/./login",
            "/%2e%2e/login",
            "/site/%2f../login",
            "/site\\..\\login",
        ] {
            let request = UseregRequestPlan {
                operation: UseregOperation::LoginPage,
                method: UseregHttpMethod::Get,
                path: path.to_owned(),
                query: Vec::new(),
                headers: Vec::new(),
                form: Vec::new(),
            };
            assert!(
                matches!(
                    client.resolve_request(request),
                    Err(UseregClientError::InvalidRequestPlan(_))
                ),
                "unsafe route path unexpectedly resolved: {path}"
            );
        }
    }

    #[test]
    fn builds_cookie_aware_query_form_and_header_requests() {
        let client = client();
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
        let plan = client
            .validate_user_request_plan("header-csrf", &credentials, "7K4P")
            .expect("validation plan");
        let request = plan.build_request(&transport).expect("request");
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.url().path(), "/site/validate-user");
        assert_eq!(request.headers()["X-CSRF-Token"], "header-csrf");
        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("form body");
        let body = std::str::from_utf8(body).expect("UTF-8 form");
        assert!(body.contains("LoginForm%5Busername%5D=20260001%40example.edu.cn"));
        assert!(body.contains("LoginForm%5Bpassword%5D=wire-password-fixture"));
        assert!(!format!("{plan:?}").contains(PASSWORD));
        assert!(!format!("{plan:?}").contains("header-csrf"));
    }

    #[test]
    fn certification_page_is_a_get_and_each_response_must_keep_its_route() {
        let client = client();
        let page = client
            .certification_page_request_plan()
            .expect("certification page plan");
        assert_eq!(page.method, Method::GET);
        assert_eq!(page.endpoint.path(), "/certification");
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let request = page.build_request(&transport).expect("GET request");
        assert_eq!(request.method(), Method::GET);
        assert!(request.body().is_none());

        let matching = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/certification",
            "text/html",
            b"fixture",
        );
        assert!(client.response_matches_plan(&matching, &page));

        let wrong_path = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/home",
            "text/html",
            b"fixture",
        );
        assert!(!client.response_matches_plan(&wrong_path, &page));

        let wrong_origin = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://other.example.test/certification",
            "text/html",
            b"fixture",
        );
        assert!(!client.response_matches_plan(&wrong_origin, &page));

        let wrong_query = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/certification?next=/home",
            "text/html",
            b"fixture",
        );
        assert!(!client.response_matches_plan(&wrong_query, &page));

        let refresh = client.captcha_request_plan(true).expect("captcha refresh");
        let matching_refresh = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha?refresh=1",
            "application/json",
            b"{}",
        );
        assert!(client.response_matches_plan(&matching_refresh, &refresh));

        let changed_refresh = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha?refresh=0",
            "application/json",
            b"{}",
        );
        assert!(!client.response_matches_plan(&changed_refresh, &refresh));

        let extra_query = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha?refresh=1&unexpected=1",
            "application/json",
            b"{}",
        );
        assert!(!client.response_matches_plan(&extra_query, &refresh));
    }

    #[test]
    fn maps_http_200_login_html_without_claiming_success() {
        let client = client();
        let url = Url::parse("https://usereg.example.test/login").expect("URL");
        let mapped = client
            .map_html_response(
                StatusCode::OK,
                &url,
                r#"<html><form><input type="hidden" name="_csrf-8800" value="csrf-fixture-value"><input name="LoginForm[username]"><input name="LoginForm[password]"></form></html>"#,
            )
            .expect("HTML response");
        assert!(mapped.is_login_required());
        assert_eq!(mapped.csrf.as_ref().unwrap().as_str(), CSRF);
        assert!(matches!(
            mapped.classification,
            UseregPageClassification::LoginRequired(_)
        ));
    }

    #[test]
    fn maps_failure_marker_as_login_failure_even_with_http_200() {
        let client = client();
        let url = Url::parse("https://usereg.example.test/login").expect("URL");
        let mapped = client
            .map_html_response(StatusCode::OK, &url, "<html>用户名或密码错误</html>")
            .expect("HTML response");
        assert!(matches!(
            mapped.classification,
            UseregPageClassification::LoginFailed(_)
        ));
    }

    #[test]
    fn rejects_a_structurally_valid_html_page_from_an_external_origin() {
        let client = client();
        let url = Url::parse("https://evil.example.test/login").expect("URL");
        let error = client
            .map_html_response(StatusCode::OK, &url, "<html>done</html>")
            .expect_err("external HTML must not be mapped as USEREG state");
        assert!(matches!(error, UseregClientError::UnexpectedOrigin));
    }

    #[test]
    fn csrf_parser_skips_comments_and_scripts_and_requires_agreement() {
        let html = r#"
            <!-- <input type="hidden" name="_csrf-8800" value="comment-token"> -->
            <script>const fake = '<input name="_csrf-8800" value="script-token">';</script>
            <input type="hidden" name="_csrf-8800" value="csrf-fixture-value">
        "#;
        let csrf = extract_csrf(html).expect("CSRF");
        assert_eq!(csrf.as_str(), CSRF);

        let conflict = extract_csrf(
            r#"<input type="hidden" name="_csrf-8800" value="one"><input type="hidden" name="_csrf-8800" value="two">"#,
        );
        assert!(matches!(
            conflict,
            Err(UseregCsrfParseError::ConflictingTokens)
        ));
    }

    #[test]
    fn csrf_parser_rejects_visible_inputs_and_malformed_tags() {
        assert!(matches!(
            extract_csrf(r#"<input type="text" name="_csrf-8800" value="token">"#),
            Err(UseregCsrfParseError::InvalidInputType)
        ));
        assert!(matches!(
            extract_csrf(r#"<input name="_csrf-8800" value="token">"#),
            Err(UseregCsrfParseError::InvalidInputType)
        ));
        assert!(matches!(
            extract_csrf(r#"<input type="hidden" name="_csrf-8800">"#),
            Err(UseregCsrfParseError::MissingAttribute { attribute: "value" })
        ));
        assert!(matches!(
            extract_csrf(r#"<input type="hidden" name="_csrf-8800" value="token""#),
            Err(UseregCsrfParseError::MalformedTag { .. })
        ));
    }

    #[test]
    fn response_debug_does_not_print_body_or_csrf_material() {
        let response = UseregHttpResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://usereg.example.test/login").expect("URL"),
            content_type: Some("text/html".to_owned()),
            body: format!("password={PASSWORD}&csrf={CSRF}").into_bytes(),
        };
        let debug = format!("{response:?}");
        assert!(!debug.contains(PASSWORD));
        assert!(!debug.contains(CSRF));
        assert!(debug.contains("body_len"));
    }

    #[test]
    fn diagnostics_do_not_expose_mapping_paths_or_invalid_route_values() {
        let client = client();
        let response = UseregHttpResponse {
            status: StatusCode::BAD_GATEWAY,
            final_url: Url::parse(
                "https://webvpn.example.test/https/opaque-usereg-mapping/login?ticket=secret-ticket",
            )
            .expect("URL"),
            content_type: Some("text/html".to_owned()),
            body: Vec::new(),
        };
        let error = response.ensure_success().expect_err("HTTP failure");
        let error_text = error.to_string();
        assert!(!error_text.contains("opaque-usereg-mapping"));
        assert!(!error_text.contains("secret-ticket"));

        let request = UseregRequestPlan {
            operation: UseregOperation::LoginPage,
            method: UseregHttpMethod::Get,
            path: "route/secret-csrf-value".to_owned(),
            query: Vec::new(),
            headers: Vec::new(),
            form: Vec::new(),
        };
        let error = client
            .resolve_request(request)
            .expect_err("invalid route fixture");
        assert!(!error.to_string().contains("secret-csrf-value"));
    }

    #[test]
    fn mapped_response_debug_redacts_device_query_values() {
        let client = client();
        let url = Url::parse(
            "https://usereg.example.test/home/delete?id=device-secret&user_mac=mac-secret",
        )
        .expect("URL");
        let mapped = client
            .map_html_response(StatusCode::OK, &url, "<html>done</html>")
            .expect("HTML response");
        let debug = format!("{mapped:?}");
        assert!(!debug.contains("device-secret"));
        assert!(!debug.contains("mac-secret"));
    }

    #[test]
    fn non_success_is_kept_out_of_the_200_html_state_machine() {
        let client = client();
        let url = Url::parse("https://usereg.example.test/login").expect("URL");
        let mapped = client
            .map_html_response(StatusCode::UNAUTHORIZED, &url, "<html>login</html>")
            .expect("response");
        assert!(matches!(
            mapped.classification,
            UseregPageClassification::NonSuccess
        ));
    }

    #[test]
    fn explicit_non_html_content_type_cannot_be_promoted_by_html_body() {
        let client = client();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/home",
            "application/json",
            br#"<html><form><input name="LoginForm[username]"><input name="LoginForm[password]"></form></html>"#,
        );
        let mapped = client.map_response(&response).expect("response");
        assert!(matches!(
            mapped.classification,
            UseregPageClassification::NonHtml
        ));
    }

    #[test]
    fn missing_content_type_cannot_be_promoted_by_html_body() {
        let client = client();
        let response = UseregHttpResponse::fixture_without_content_type(
            StatusCode::OK,
            "https://usereg.example.test/home",
            br#"<html><div id="w1-container"></div></html>"#,
        );
        let mapped = client.map_response(&response).expect("response");
        assert!(matches!(
            mapped.classification,
            UseregPageClassification::NonHtml
        ));
    }

    #[test]
    fn login_hint_does_not_match_a_query_value_on_another_path() {
        let client = client();
        let url = Url::parse("https://usereg.example.test/home?next=/login").expect("URL");
        let mapped = client
            .map_html_response(StatusCode::OK, &url, "<html>home</html>")
            .expect("response");
        assert!(matches!(
            mapped.classification,
            UseregPageClassification::HtmlUnknown(_)
        ));
    }
}
