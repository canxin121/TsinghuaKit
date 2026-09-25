//! Execution and response classification for the INFO service.
//!
//! [`crate::info`] owns the deployment-specific route and wire parameters.
//! This module adds the origin, turns those plans into requests, and executes
//! them with a caller-provided [`CampusHttpTransport`].  Passing the
//! transport in is deliberate: its `reqwest::Client` owns the Cookie jar, so
//! INFO keeps the same session as the caller without creating a second
//! unauthenticated client.
//!
//! The response boundary is conservative.  A successful HTTP status is not
//! enough to claim that INFO returned data: a 200 login page is classified as
//! HTML and the exact `object.roamingurl` shape is still required before an
//! opaque WebVPN URL is returned.  Some legacy proxies attach an HTML content
//! type to a JSON response, so the body shape is considered before the header
//! alone.

use std::fmt;

use reqwest::{
    Method, StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::info::{
    InfoError, InfoParameterEncoding, InfoPortalProfile, InfoPortalRequestPlan, InfoPortalResponse,
};
use crate::transport::{CampusHttpTransport, TransportError};

// Match the current thu-info reference network helper for the INFO roaming
// call.  Keep this scoped to onlineAppRedirect instead of changing the shared
// transport identity used by the other campus services.
const REFERENCE_INFO_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.88 Safari/537.36";
const REFERENCE_INFO_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

const DEFAULT_LOGIN_PATH_HINTS: &[&str] = &["/login", "/auth/login", "/authserver", "/sso/login"];
const DEFAULT_EXPIRED_MARKERS: &[&str] = &[
    "登录失效",
    "登录已失效",
    "会话已过期",
    "会话已失效",
    "请先登录",
    "请登录",
    "未登录",
    "session expired",
    "not logged in",
    "session invalid",
    "login required",
    "authentication required",
    "unauthorized",
];

/// Errors raised while configuring INFO, executing a request, or mapping its
/// response.  HTTP status errors intentionally contain no response body: an
/// error page can echo request values or a roaming URL.
#[derive(Debug, Error)]
pub enum InfoClientError {
    #[error("invalid INFO base URL: {0}")]
    InvalidBaseUrl(String),

    #[error("invalid INFO client configuration: {0}")]
    InvalidConfig(String),

    #[error("INFO response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("INFO response ended outside the requested route")]
    UnexpectedPath,

    #[error(transparent)]
    Profile(#[from] InfoError),

    // Reqwest may include the request URL in a transport error. News calls
    // carry CSRF values in that URL, so keep the source private at this
    // service boundary.
    #[error("INFO transport request failed")]
    Transport(#[from] TransportError),

    #[error("INFO request could not be built: {0}")]
    RequestBuild(String),

    #[error("INFO returned HTTP status {status} at {url}")]
    HttpStatus { status: StatusCode, url: String },

    #[error("INFO returned an HTML page at {url}: {classification:?}")]
    HtmlPage {
        url: String,
        classification: InfoHtmlClassification,
    },
}

/// Client configuration for one INFO deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoClientConfig {
    /// The origin used to resolve the absolute paths in [`InfoPortalProfile`].
    pub base_url: Url,
    /// INFO's route and field names.  WebVPN target mappings do not belong
    /// here; the URL returned by INFO remains opaque.
    pub profile: InfoPortalProfile,
    /// URL fragments which are evidence that a 200 HTML response is a login
    /// page after a redirect.
    pub login_path_hints: Vec<String>,
    /// Explicit page text which indicates that the session has expired.
    pub expired_markers: Vec<String>,
}

impl InfoClientConfig {
    /// Creates an INFO configuration without retaining any credential or
    /// cookie value.
    pub fn new(
        base_url: impl Into<String>,
        profile: InfoPortalProfile,
    ) -> Result<Self, InfoClientError> {
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
        };
        config.validate_markers()?;
        Ok(config)
    }

    /// Replaces the login URL hints for a deployment with a different SSO
    /// route.
    pub fn with_login_path_hints(
        mut self,
        hints: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, InfoClientError> {
        self.login_path_hints = hints.into_iter().map(Into::into).collect();
        self.validate_markers()?;
        Ok(self)
    }

    /// Replaces the explicit session-expiry text markers for a deployment.
    pub fn with_expired_markers(
        mut self,
        markers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, InfoClientError> {
        self.expired_markers = markers.into_iter().map(Into::into).collect();
        self.validate_markers()?;
        Ok(self)
    }

    fn validate_markers(&self) -> Result<(), InfoClientError> {
        for value in self
            .login_path_hints
            .iter()
            .chain(self.expired_markers.iter())
        {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(InfoClientError::InvalidConfig(
                    "INFO response markers must not be empty or contain control characters"
                        .to_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// A client which owns INFO configuration but no credentials or session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoClient {
    config: InfoClientConfig,
}

impl InfoClient {
    pub fn new(config: InfoClientConfig) -> Result<Self, InfoClientError> {
        validate_profile(&config.profile)?;
        validate_base_url(&config.base_url)?;
        config.validate_markers()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &InfoClientConfig {
        &self.config
    }

    pub fn profile(&self) -> &InfoPortalProfile {
        &self.config.profile
    }

    /// Creates a request plan using the selected INFO parameter placement.
    pub fn online_app_redirect_request_plan(
        &self,
        yyfwid: &str,
        csrf: &str,
        encoding: InfoParameterEncoding,
    ) -> Result<InfoRequestPlan, InfoClientError> {
        let request = match encoding {
            InfoParameterEncoding::Query => self
                .config
                .profile
                .online_app_redirect_query(yyfwid, csrf)?,
            InfoParameterEncoding::Form => {
                self.config.profile.online_app_redirect_form(yyfwid, csrf)?
            }
        };
        let endpoint = self.resolve_path(&request.path)?;
        let method = match request.encoding {
            InfoParameterEncoding::Query => Method::GET,
            InfoParameterEncoding::Form => Method::POST,
        };
        Ok(InfoRequestPlan {
            method,
            endpoint,
            request,
        })
    }

    pub fn online_app_redirect_query_plan(
        &self,
        yyfwid: &str,
        csrf: &str,
    ) -> Result<InfoRequestPlan, InfoClientError> {
        self.online_app_redirect_request_plan(yyfwid, csrf, InfoParameterEncoding::Query)
    }

    pub fn online_app_redirect_form_plan(
        &self,
        yyfwid: &str,
        csrf: &str,
    ) -> Result<InfoRequestPlan, InfoClientError> {
        self.online_app_redirect_request_plan(yyfwid, csrf, InfoParameterEncoding::Form)
    }

    /// Builds a request using the transport's configured client.  The caller
    /// can inspect or send it; its Cookie provider is the same one used by
    /// every other request through that transport.
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
        plan: &InfoRequestPlan,
    ) -> Result<reqwest::Request, InfoClientError> {
        plan.build_request(transport)
    }

    /// Executes an INFO request and retains status, final redirect URL, and
    /// body for the response mapper.  A non-2xx response is returned as data
    /// so callers can inspect a protocol-specific error page without losing
    /// the status boundary.
    pub async fn execute(
        &self,
        transport: &CampusHttpTransport,
        plan: &InfoRequestPlan,
    ) -> Result<InfoHttpResponse, InfoClientError> {
        let request = plan.build_request(transport)?;
        let response = transport
            .execute(request)
            .await
            .map_err(|error| InfoClientError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoClientError::UnexpectedOrigin)?;
        if redirect_location.as_deref().is_some_and(|location| {
            redirect_leaves_origin(&self.config.base_url, &final_url, location)
        }) {
            return Err(InfoClientError::UnexpectedOrigin);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| InfoClientError::Transport(TransportError::Decode(error)))?;
        let info_response = InfoHttpResponse {
            status,
            final_url,
            content_type,
            body,
        };

        // A same-origin redirect can still land on a login route. Classify a
        // proved login page before the route gate so a 200 redirect to SSO is
        // recoverable as authentication expiry instead of looking like an
        // arbitrary route error. Unknown HTML and unrelated JSON still fail
        // the exact route check below.
        let redirect_target = redirect_location
            .as_deref()
            .map(|location| resolve_location(&info_response.final_url, location))
            .transpose()
            .map_err(|_| InfoClientError::UnexpectedOrigin)?;
        if info_response.status.is_success()
            && (is_login_path(info_response.final_url.path())
                || redirect_target
                    .as_ref()
                    .is_some_and(|target| is_login_path(target.path())))
        {
            return Err(InfoClientError::HtmlPage {
                url: diagnostic_url(
                    redirect_target
                        .as_ref()
                        .filter(|target| is_login_path(target.path()))
                        .unwrap_or(&info_response.final_url),
                ),
                classification: InfoHtmlClassification::LoginRequired,
            });
        }
        if info_response.status.is_success()
            && matches!(
                self.html_evidence(&info_response),
                Some(InfoHtmlEvidence {
                    classification: InfoHtmlClassification::LoginRequired,
                    ..
                })
            )
        {
            return Err(InfoClientError::HtmlPage {
                url: diagnostic_url(&info_response.final_url),
                classification: InfoHtmlClassification::LoginRequired,
            });
        }
        if matches!(
            info_response.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(InfoClientError::HttpStatus {
                status: info_response.status,
                url: diagnostic_url(&info_response.final_url),
            });
        }

        // A same-origin redirect can still land on a generic WebVPN page or
        // an unrelated JSON endpoint. The online-app redirect contract is
        // tied to its requested route, so a valid JSON envelope from another
        // path is not sufficient proof of this operation.
        if info_response.final_url.path() != plan.endpoint.path() {
            return Err(InfoClientError::UnexpectedPath);
        }
        // The reference INFO client treats this request as a navigation
        // response. The legacy WebVPN front end may normalize, consume, or
        // reorder the query while keeping the handoff endpoint and response
        // body intact. The fixed same-origin path check above, followed by
        // strict JSON envelope parsing below, is the useful boundary here;
        // requiring the final URL to echo CSRF/selector values rejects valid
        // handoffs without adding evidence that the response is authenticated.
        if redirect_location.as_deref().is_some_and(|location| {
            redirect_leaves_requested_path(&info_response.final_url, location, plan.endpoint.path())
        }) {
            return Err(InfoClientError::UnexpectedPath);
        }

        Ok(info_response)
    }

    /// Executes the standard INFO redirect request as a GET with query
    /// parameters.  Use [`Self::online_app_redirect_with_encoding`] when a
    /// deployment requires the form variant.
    pub async fn online_app_redirect(
        &self,
        transport: &CampusHttpTransport,
        yyfwid: &str,
        csrf: &str,
    ) -> Result<InfoPortalResponse, InfoClientError> {
        self.online_app_redirect_with_encoding(
            transport,
            yyfwid,
            csrf,
            InfoParameterEncoding::Query,
        )
        .await
    }

    pub async fn online_app_redirect_with_encoding(
        &self,
        transport: &CampusHttpTransport,
        yyfwid: &str,
        csrf: &str,
        encoding: InfoParameterEncoding,
    ) -> Result<InfoPortalResponse, InfoClientError> {
        let plan = self.online_app_redirect_request_plan(yyfwid, csrf, encoding)?;
        let response = self.execute(transport, &plan).await?;
        self.parse_response(&response)
    }

    /// Maps an INFO response.  Only the exact profile JSON shape yields a
    /// roaming URL.  HTTP 200 HTML is surfaced as an explicit state instead
    /// of being passed to a permissive JSON fallback.
    pub fn parse_response(
        &self,
        response: &InfoHttpResponse,
    ) -> Result<InfoPortalResponse, InfoClientError> {
        match self.classify_response(response)? {
            InfoResponseClassification::Json(value) => Ok(value),
            InfoResponseClassification::LoginRequired(evidence)
            | InfoResponseClassification::UnexpectedHtml(evidence) => {
                Err(InfoClientError::HtmlPage {
                    url: diagnostic_url(&response.final_url),
                    classification: evidence.classification,
                })
            }
        }
    }

    /// Classifies a successful response while keeping HTML authentication
    /// pages separate from JSON protocol failures.
    pub fn classify_response(
        &self,
        response: &InfoHttpResponse,
    ) -> Result<InfoResponseClassification, InfoClientError> {
        if !same_origin(&self.config.base_url, &response.final_url) {
            return Err(InfoClientError::UnexpectedOrigin);
        }
        if !response.status.is_success() {
            return Err(InfoClientError::HttpStatus {
                status: response.status,
                url: diagnostic_url(&response.final_url),
            });
        }

        if let Some(evidence) = self.html_evidence(response) {
            return Ok(match evidence.classification {
                InfoHtmlClassification::LoginRequired => {
                    InfoResponseClassification::LoginRequired(evidence)
                }
                InfoHtmlClassification::UnexpectedHtml => {
                    InfoResponseClassification::UnexpectedHtml(evidence)
                }
            });
        }

        let value = self
            .config
            .profile
            .parse_online_app_redirect(&response.body)?;
        Ok(InfoResponseClassification::Json(value))
    }

    fn resolve_path(&self, path: &str) -> Result<Url, InfoClientError> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains(['?', '#'])
            || path.contains("..")
            || path.contains("://")
            || path.contains('\\')
            || invalid_percent_encoding(path)
            || path_contains_encoded_escape(path)
            || path.chars().any(char::is_control)
        {
            return Err(InfoClientError::InvalidBaseUrl(format!(
                "invalid INFO route path: {path}"
            )));
        }
        let endpoint = self
            .config
            .base_url
            .join(path)
            .map_err(|error| InfoClientError::InvalidBaseUrl(error.to_string()))?;
        if !same_origin(&self.config.base_url, &endpoint) {
            return Err(InfoClientError::InvalidBaseUrl(
                "INFO endpoint must stay on the configured origin".to_owned(),
            ));
        }
        Ok(endpoint)
    }

    fn html_evidence(&self, response: &InfoHttpResponse) -> Option<InfoHtmlEvidence> {
        let body = response
            .body
            .strip_prefix('\u{feff}')
            .unwrap_or(&response.body)
            .trim_start();
        let lower_body = body.to_ascii_lowercase();
        // INFO's front proxy has been observed to retain `text/html` while
        // forwarding a JSON response.  Let JSON-shaped bodies reach the
        // strict JSON parser; a real login document begins with markup or a
        // plain login marker and is still classified below.
        let json_shape = matches!(body.as_bytes().first(), Some(b'{' | b'[' | b'"'));
        let content_type_is_html = response.content_type.as_deref().is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.starts_with("text/html") || value.starts_with("application/xhtml+xml")
        });
        let html_shape = (!json_shape && content_type_is_html)
            || lower_body.starts_with("<!doctype html")
            || lower_body.starts_with("<html")
            || lower_body.starts_with("<head")
            || lower_body.starts_with("<body")
            || lower_body.contains("<form")
            || lower_body.contains("<input");
        let plain_login_marker = !json_shape
            && self
                .config
                .expired_markers
                .iter()
                .any(|marker| lower_body.contains(&marker.to_ascii_lowercase()));
        if !html_shape && !plain_login_marker {
            return None;
        }

        let mut signals = Vec::new();
        if url_matches_hint(&response.final_url, &self.config.login_path_hints) {
            signals.push(InfoHtmlSignal::FinalUrlMatchesLogin);
        }
        if contains_identity_login_form(&lower_body) {
            signals.push(InfoHtmlSignal::IdentityLoginForm);
        }
        if contains_identity_fields(&lower_body) {
            signals.push(InfoHtmlSignal::IdentityFields);
        }
        if plain_login_marker {
            signals.push(InfoHtmlSignal::ExplicitExpiredMarker);
        }

        let classification = if signals.iter().any(|signal| {
            matches!(
                signal,
                InfoHtmlSignal::FinalUrlMatchesLogin
                    | InfoHtmlSignal::IdentityLoginForm
                    | InfoHtmlSignal::IdentityFields
                    | InfoHtmlSignal::ExplicitExpiredMarker
            )
        }) {
            InfoHtmlClassification::LoginRequired
        } else {
            InfoHtmlClassification::UnexpectedHtml
        };

        Some(InfoHtmlEvidence {
            classification,
            signals,
        })
    }
}

/// The resolved INFO request.  `request` remains available so callers can
/// inspect the original profile placement without reconstructing fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoRequestPlan {
    pub method: Method,
    pub endpoint: Url,
    pub request: InfoPortalRequestPlan,
}

impl InfoRequestPlan {
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, InfoClientError> {
        let builder = match self.request.encoding {
            InfoParameterEncoding::Query => transport
                .client()
                .request(self.method.clone(), self.endpoint.clone())
                .query(&self.request.parameters),
            InfoParameterEncoding::Form => transport
                .client()
                .request(self.method.clone(), self.endpoint.clone())
                .form(&self.request.parameters),
        };
        builder
            .header(reqwest::header::USER_AGENT, REFERENCE_INFO_USER_AGENT)
            .header(CONTENT_TYPE, REFERENCE_INFO_CONTENT_TYPE)
            .build()
            .map_err(|error| InfoClientError::RequestBuild(error.to_string()))
    }
}

/// The raw response retained long enough for strict INFO mapping.
#[derive(Clone, PartialEq, Eq)]
pub struct InfoHttpResponse {
    pub status: StatusCode,
    pub final_url: Url,
    pub content_type: Option<String>,
    body: String,
}

impl InfoHttpResponse {
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }
}

impl fmt::Debug for InfoHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InfoHttpResponse")
            .field("status", &self.status)
            .field("final_url", &diagnostic_url(&self.final_url))
            .field("content_type", &self.content_type)
            .field("body", &"[redacted]")
            .finish()
    }
}

/// Signals found in a 200 HTML response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoHtmlSignal {
    FinalUrlMatchesLogin,
    IdentityLoginForm,
    IdentityFields,
    ExplicitExpiredMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoHtmlClassification {
    LoginRequired,
    UnexpectedHtml,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoHtmlEvidence {
    pub classification: InfoHtmlClassification,
    pub signals: Vec<InfoHtmlSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InfoResponseClassification {
    Json(InfoPortalResponse),
    LoginRequired(InfoHtmlEvidence),
    UnexpectedHtml(InfoHtmlEvidence),
}

fn normalize_base_url(base_url: &str) -> Result<Url, InfoClientError> {
    let url =
        Url::parse(base_url).map_err(|error| InfoClientError::InvalidBaseUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must use http or https".to_owned(),
        ));
    }
    if url.host_str().is_none() {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must contain a host".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must not contain userinfo".to_owned(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() || !safe_path(url.path()) {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must not contain a query or fragment".to_owned(),
        ));
    }
    Ok(url)
}

fn validate_base_url(url: &Url) -> Result<(), InfoClientError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must use http or https".to_owned(),
        ));
    }
    if url.host_str().is_none() {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must contain a host".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must not contain userinfo".to_owned(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() || !safe_path(url.path()) {
        return Err(InfoClientError::InvalidBaseUrl(
            "INFO base URL must not contain a query or fragment".to_owned(),
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

fn redirect_leaves_origin(base: &Url, response_url: &Url, location: &str) -> bool {
    if invalid_percent_encoding(location) {
        return true;
    }
    let Ok(candidate) = Url::parse(location).or_else(|_| response_url.join(location)) else {
        return true;
    };
    !same_origin(base, &candidate)
}

fn redirect_leaves_requested_path(response_url: &Url, location: &str, expected: &str) -> bool {
    let Ok(candidate) = Url::parse(location).or_else(|_| response_url.join(location)) else {
        return true;
    };
    candidate.fragment().is_some() || candidate.path() != expected
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, ()> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(());
    }
    let candidate = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| ())?;
    if candidate.fragment().is_some() {
        return Err(());
    }
    Ok(candidate)
}

fn is_login_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower
        .split('/')
        .any(|segment| matches!(segment, "login" | "auth" | "authserver"))
        || lower.contains("/do/off/ui/auth/login")
        || lower.contains("/auth/login")
        || lower.contains("/sso/login")
}

fn invalid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
    })
}

fn path_contains_encoded_escape(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.windows(3).any(|window| {
        window[0] == b'%'
            && hex_value(window[1])
                .zip(hex_value(window[2]))
                .is_some_and(|(high, low)| {
                    matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                })
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

fn diagnostic_url(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

fn validate_profile(profile: &InfoPortalProfile) -> Result<(), InfoClientError> {
    // The profile keeps its validation method private.  Building a harmless
    // plan invokes that same validation without duplicating route/field rules.
    profile
        .online_app_redirect_query("info-client-validation", "info-client-validation")
        .map(|_| ())
        .map_err(InfoClientError::Profile)
}

fn url_matches_hint(url: &Url, hints: &[String]) -> bool {
    let path_and_query = format!(
        "{}?{}",
        url.path().to_ascii_lowercase(),
        url.query().unwrap_or_default().to_ascii_lowercase()
    );
    hints.iter().any(|hint| {
        let hint = hint.to_ascii_lowercase();
        !hint.is_empty() && path_and_query.contains(&hint)
    })
}

fn contains_identity_fields(body: &str) -> bool {
    let has_user = contains_input_name(body, "i_user");
    let has_password = contains_input_name(body, "i_pass");
    has_user && has_password
}

fn contains_identity_login_form(body: &str) -> bool {
    body.contains("<form")
        && (body.contains("/do/off/ui/auth/login")
            || body.contains("/auth/login")
            || body.contains("login/check"))
}

fn contains_input_name(body: &str, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        format!("name=\"{name}\""),
        format!("name='{name}'"),
        format!("name={name}"),
    ]
    .iter()
    .any(|candidate| body.contains(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> InfoClient {
        let config =
            InfoClientConfig::new("https://info.example.test/", InfoPortalProfile::default())
                .expect("config");
        InfoClient::new(config).expect("client")
    }

    fn response(body: &str, final_url: &str, content_type: Option<&str>) -> InfoHttpResponse {
        InfoHttpResponse {
            status: StatusCode::OK,
            final_url: Url::parse(final_url).expect("URL"),
            content_type: content_type.map(str::to_owned),
            body: body.to_owned(),
        }
    }

    #[test]
    fn resolves_profile_plan_without_preencoding_values() {
        let client = client();
        let plan = client
            .online_app_redirect_query_plan("id with space", "csrf&value")
            .expect("plan");

        assert_eq!(plan.method, Method::GET);
        assert_eq!(
            plan.endpoint.as_str(),
            "https://info.example.test/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect"
        );
        assert_eq!(plan.request.parameters[0].1, "id with space");
        assert_eq!(plan.request.parameters[1].1, "csrf&value");
    }

    #[test]
    fn builds_query_and_form_with_the_shared_transport_client() {
        let client = client();
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let query = client
            .online_app_redirect_query_plan("id with space", "csrf&value")
            .expect("query plan");
        let query_request = query.build_request(&transport).expect("request");
        assert_eq!(query_request.method(), Method::GET);
        assert_eq!(
            query_request.url().query_pairs().next().unwrap().0,
            "yyfwid"
        );
        assert_eq!(
            query_request.url().query_pairs().next().unwrap().1,
            "id with space"
        );

        let form = client
            .online_app_redirect_form_plan("id", "csrf")
            .expect("form plan");
        let form_request = form.build_request(&transport).expect("request");
        assert_eq!(form_request.method(), Method::POST);
        assert!(form_request.url().query().is_none());
        assert_eq!(
            form_request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("form body"),
            b"yyfwid=id&_csrf=csrf&machine=p"
        );
    }

    #[test]
    fn maps_exact_json_and_keeps_the_webvpn_value_opaque() {
        let client = client();
        let response = response(
            r#"{"object":{"roamingurl":"https://webvpn.example/https/runtime/path?x=1"}}"#,
            "https://info.example.test/redirect",
            Some("application/json"),
        );
        let mapped = client.parse_response(&response).expect("JSON response");
        assert_eq!(
            mapped.roaming_url().as_str(),
            "https://webvpn.example/https/runtime/path?x=1"
        );
        assert!(!format!("{mapped:?}").contains("runtime/path"));
    }

    #[test]
    fn response_debug_redacts_query_values() {
        let response = response(
            r#"{"object":{"roamingurl":"https://webvpn.example/runtime"}}"#,
            "https://info.example.test/redirect?_csrf=secret-csrf&yyfwid=secret-id",
            Some("application/json"),
        );
        let debug = format!("{response:?}");
        assert!(!debug.contains("secret-csrf"));
        assert!(!debug.contains("secret-id"));
    }

    #[test]
    fn cross_origin_redirects_are_rejected_without_retaining_location() {
        let base = Url::parse("https://info.example.test/").expect("base URL");
        let response = Url::parse("https://info.example.test/redirect").expect("response URL");
        assert!(redirect_leaves_origin(
            &base,
            &response,
            "https://id.example.test/login?ticket=secret"
        ));
        assert!(!redirect_leaves_origin(&base, &response, "/login"));
        assert!(redirect_leaves_origin(&base, &response, "%%%malformed"));
    }

    #[test]
    fn transport_error_display_does_not_expose_a_csrf_query() {
        let error = InfoClientError::Transport(TransportError::InvalidUrl(
            "https://info.example.test/news?_csrf=secret".to_owned(),
        ));
        assert_eq!(error.to_string(), "INFO transport request failed");
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[test]
    fn classifies_http_200_identity_html_as_login_required() {
        let client = client();
        let response = response(
            r#"<!doctype html><form action="/do/off/ui/auth/login"><input name="i_user"><input name="i_pass"></form>"#,
            "https://info.example.test/do/off/ui/auth/login",
            Some("text/html; charset=utf-8"),
        );
        let classification = client.classify_response(&response).expect("classification");
        let InfoResponseClassification::LoginRequired(evidence) = classification else {
            panic!("expected login-required HTML")
        };
        assert!(
            evidence
                .signals
                .contains(&InfoHtmlSignal::IdentityLoginForm)
        );
        assert!(matches!(
            client.parse_response(&response),
            Err(InfoClientError::HtmlPage {
                classification: InfoHtmlClassification::LoginRequired,
                ..
            })
        ));
    }

    #[test]
    fn rejects_a_structurally_valid_response_from_an_external_origin() {
        let client = client();
        let response = response(
            r#"{"object":{"roamingurl":"https://webvpn.example/runtime"}}"#,
            "https://evil.example.test/redirect",
            Some("application/json"),
        );
        assert!(matches!(
            client.classify_response(&response),
            Err(InfoClientError::UnexpectedOrigin)
        ));
    }

    #[test]
    fn keeps_unrecognized_html_out_of_the_success_path() {
        let client = client();
        let response = response(
            "<html><body>maintenance</body></html>",
            "https://info.example.test/maintenance",
            None,
        );
        assert!(matches!(
            client.classify_response(&response),
            Ok(InfoResponseClassification::UnexpectedHtml(_))
        ));
    }

    #[test]
    fn parses_json_even_when_a_legacy_proxy_labels_it_as_html() {
        let client = client();
        let response = response(
            r#"{"result":"success","object":{"roamingurl":"https://webvpn.example/runtime"}}"#,
            "https://info.example.test/redirect",
            Some("text/html; charset=utf-8"),
        );
        assert!(matches!(
            client.classify_response(&response),
            Ok(InfoResponseClassification::Json(_))
        ));
    }

    #[test]
    fn classifies_a_plain_session_expiry_marker_as_login_required() {
        let client = client();
        let response = response(
            "请先登录",
            "https://info.example.test/redirect",
            Some("text/plain; charset=utf-8"),
        );
        assert!(matches!(
            client.classify_response(&response),
            Ok(InfoResponseClassification::LoginRequired(_))
        ));
    }

    #[test]
    fn keeps_portal_business_failures_distinct_from_login_html() {
        let client = client();
        let response = response(
            r#"{"result":"error","msg":"permission denied"}"#,
            "https://info.example.test/redirect",
            Some("application/json"),
        );
        assert!(matches!(
            client.parse_response(&response),
            Err(InfoClientError::Profile(
                InfoError::ResultFailureWithMessage
            ))
        ));
    }

    #[test]
    fn rejects_non_success_before_parsing_body() {
        let client = client();
        let response = InfoHttpResponse {
            status: StatusCode::UNAUTHORIZED,
            final_url: Url::parse("https://info.example.test/redirect").expect("URL"),
            content_type: Some("text/html".to_owned()),
            body: "<html>secret</html>".to_owned(),
        };
        assert!(matches!(
            client.parse_response(&response),
            Err(InfoClientError::HttpStatus {
                status: StatusCode::UNAUTHORIZED,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn rejects_a_same_origin_redirect_that_ends_at_an_unrelated_route() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("first connection");
            let mut request = [0_u8; 1024];
            let _ = first.read(&mut request).expect("first request");
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect");

            let (mut second, _) = listener.accept().expect("second connection");
            let _ = second.read(&mut request).expect("second request");
            let body = r#"{"object":{"roamingurl":"https://webvpn.example.test/opaque"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            second
                .write_all(response.as_bytes())
                .expect("JSON response");
        });

        let client = InfoClient::new(
            InfoClientConfig::new(format!("http://{address}/"), InfoPortalProfile::standard())
                .expect("config"),
        )
        .expect("client");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/info-path-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let plan = client
            .online_app_redirect_query_plan("id", "csrf")
            .expect("plan");
        assert!(matches!(
            client.execute(&transport, &plan).await,
            Err(InfoClientError::UnexpectedPath)
        ));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn backend_repair_info_accepts_same_origin_handoff_query_normalization() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("first connection");
            let mut request = [0_u8; 2048];
            let _ = first.read(&mut request).expect("first request");
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect");

            let (mut second, _) = listener.accept().expect("second connection");
            let _ = second.read(&mut request).expect("second request");
            let body = r#"{"object":{"roamingurl":"https://webvpn.example.test/opaque"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            second
                .write_all(response.as_bytes())
                .expect("JSON response");
        });

        let client = InfoClient::new(
            InfoClientConfig::new(format!("http://{address}/"), InfoPortalProfile::standard())
                .expect("config"),
        )
        .expect("client");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/info-query-normalization-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let plan = client
            .online_app_redirect_query_plan("id", "csrf")
            .expect("plan");
        let response = client
            .execute(&transport, &plan)
            .await
            .expect("same-origin normalized handoff");
        assert_eq!(
            response.final_url.path(),
            "/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect"
        );
        assert!(response.final_url.query().is_none());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn execution_reuses_the_transport_cookie_jar() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let mut saw_cookie = false;
            for index in 0..2 {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 512];
                loop {
                    let read = stream.read(&mut buffer).expect("request");
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                if index == 1 {
                    saw_cookie = String::from_utf8_lossy(&request)
                        .to_ascii_lowercase()
                        .contains("cookie: info-session=fixture");
                }
                let body = b"{}";
                let cookie = if index == 0 {
                    "Set-Cookie: info-session=fixture; Path=/\r\n"
                } else {
                    ""
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{cookie}Connection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("headers");
                stream.write_all(body).expect("body");
            }
            saw_cookie
        });

        let client =
            InfoClientConfig::new(format!("http://{address}/"), InfoPortalProfile::default())
                .map(InfoClient::new)
                .expect("config")
                .expect("client");
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let plan = client
            .online_app_redirect_query_plan("id", "csrf")
            .expect("plan");
        client
            .execute(&transport, &plan)
            .await
            .expect("first request");
        client
            .execute(&transport, &plan)
            .await
            .expect("second request");

        assert!(server.join().expect("server"));
    }

    #[tokio::test]
    async fn real_reqwest_execution_rejects_a_cross_origin_location() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("request");
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: https://id.example.test/login?ticket=secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("response");
        });

        let client = InfoClient::new(
            InfoClientConfig::new(format!("http://{address}/"), InfoPortalProfile::standard())
                .expect("config"),
        )
        .expect("client");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/info-redirect-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let plan = client
            .online_app_redirect_query_plan("id", "csrf")
            .expect("plan");
        let error = client
            .execute(&transport, &plan)
            .await
            .expect_err("cross-origin redirect");
        assert!(matches!(error, InfoClientError::UnexpectedOrigin));
        assert!(!format!("{error:?}").contains("secret"));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn real_reqwest_execution_rejects_a_cross_origin_location_on_http_200() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("request");
            let body = r#"{"object":{"roamingurl":"https://webvpn.example.test/opaque"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nLocation: https://id.example.test/login?ticket=secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response");
        });

        let client = InfoClient::new(
            InfoClientConfig::new(format!("http://{address}/"), InfoPortalProfile::standard())
                .expect("config"),
        )
        .expect("client");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/info-200-location-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let plan = client
            .online_app_redirect_query_plan("id", "csrf")
            .expect("plan");
        let error = client
            .execute(&transport, &plan)
            .await
            .expect_err("cross-origin location on 200");
        assert!(matches!(error, InfoClientError::UnexpectedOrigin));
        assert!(!format!("{error:?}").contains("secret"));
        server.join().expect("server");
    }

    #[test]
    fn rejects_base_urls_that_can_escape_the_configured_origin() {
        assert!(matches!(
            InfoClientConfig::new("file:///tmp/", InfoPortalProfile::default()),
            Err(InfoClientError::InvalidBaseUrl(_))
        ));
        let config =
            InfoClientConfig::new("https://info.example.test/", InfoPortalProfile::default())
                .expect("config");
        let client = InfoClient::new(config).expect("client");
        assert!(
            client
                .online_app_redirect_request_plan("id", "csrf", InfoParameterEncoding::Query)
                .is_ok()
        );
    }
}
