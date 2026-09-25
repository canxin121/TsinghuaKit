//! Read-only electricity access through the existing INFO/WebVPN session.
//!
//! The service is an old ASP.NET application.  The two operations in this
//! module are GETs, so they do not use ASP.NET form state.  ViewState is only
//! observed on the separate recharge POST flow; that write flow is
//! intentionally absent here.  The adapter receives the same
//! [`CampusHttpTransport`] that performed the identity/WebVPN handoff.  A
//! clone of that transport shares its cookie jar, which makes the service
//! response itself the session proof without exposing a ticket or cookie to
//! the caller.
//!
//! Route and selector behavior is based on the redacted local audits and
//! current public implementations cited in `docs/dorm-electricity-read.md`.
//! This module reimplements the contract; it does not copy their parser,
//! fixtures, constants, or session code.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::transport::{CampusHttpTransport, TransportError};

/// The identity roaming service selector used by the electricity profile.
/// This is a route-profile input, not a URL or a credential.
pub const ELECTRICITY_WEBVPN_TARGET: &str = "0a993de7e533cd43a594459abdcab27d/1";

/// The health-chart selector is recorded for documentation and future work.
/// It is deliberately not exposed as a working operation because the public
/// response evidence does not establish whether the second response is an
/// image, base64 text, or another payload.
pub const HEALTH_WEBVPN_TARGET: &str = "0a993de7e533cd43a594459abdcab27d/0";

/// Exact electricity remainder route observed behind the opaque WebVPN
/// mapping directory.
pub const ELECTRICITY_REMAINDER_PATH: &str = "/Netweb_List/Netweb_Home_electricity_Detail.aspx";

/// Exact electricity payment-history route observed behind the same mapping.
pub const ELECTRICITY_PAYMENT_HISTORY_PATH: &str = "/Netweb_List/netweb_ele_pay_record.aspx";

/// The recharge page is intentionally kept as a boundary constant only in
/// the documentation.  There is no write method or route plan for it here.
/// Its ASP.NET ViewState must never be collected by a read-only client.
pub const ELECTRICITY_RECHARGE_NOT_IN_READ_PROFILE: &str = "/netweb_user/recharge_ele.aspx";

const ELECTRICITY_TIMEOUT_MESSAGE: &str = "time out用户登陆超时或访问内容不存在。请重试";
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;

static NEXT_ELECTRICITY_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// The only method in this read-only profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormElectricityMethod {
    Get,
}

/// The two operations with a confirmed HTML response contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormElectricityOperation {
    ReadRemainder,
    ReadPaymentHistory,
}

/// The runtime must establish this handoff before constructing the adapter.
/// Keeping the prerequisite in the request plan prevents a caller from
/// interpreting a direct page URL as a replacement for identity roaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormElectricitySessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan.  It contains no Cookie, ticket, CSRF
/// value, account identifier, or absolute WebVPN mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DormElectricityRequestPlan {
    pub operation: DormElectricityOperation,
    pub method: DormElectricityMethod,
    pub path: &'static str,
    pub webvpn_target: &'static str,
    pub session_prerequisite: DormElectricitySessionPrerequisite,
}

/// The confirmed electricity route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DormElectricityProfile;

impl DormElectricityProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn remainder_request(&self) -> DormElectricityRequestPlan {
        DormElectricityRequestPlan {
            operation: DormElectricityOperation::ReadRemainder,
            method: DormElectricityMethod::Get,
            path: ELECTRICITY_REMAINDER_PATH,
            webvpn_target: ELECTRICITY_WEBVPN_TARGET,
            session_prerequisite: DormElectricitySessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    pub const fn payment_history_request(&self) -> DormElectricityRequestPlan {
        DormElectricityRequestPlan {
            operation: DormElectricityOperation::ReadPaymentHistory,
            method: DormElectricityMethod::Get,
            path: ELECTRICITY_PAYMENT_HISTORY_PATH,
            webvpn_target: ELECTRICITY_WEBVPN_TARGET,
            session_prerequisite: DormElectricitySessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// A validated electricity remainder observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityRemainder {
    /// The service's numeric remainder.  The source does not establish a
    /// unit, so the adapter preserves the number without inventing one.
    pub remainder: f64,
    /// The service-provided timestamp, retained after strict validation.
    pub update_time: String,
}

/// A payment-history row with the six source columns retained because the
/// public contract does not name every legacy column.  The fields with
/// stable evidence are also exposed in typed form.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityPaymentRecord {
    pub source_columns: [String; 6],
    pub sequence: Option<u64>,
    pub occurred_at: String,
    pub amount: f64,
    pub status: String,
}

/// A structurally valid history page.  An empty `records` value is valid only
/// when the table, header row, and footer row are all present.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityPaymentHistory {
    pub records: Vec<ElectricityPaymentRecord>,
}

impl ElectricityPaymentHistory {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
}

/// Proof that this adapter fetched and strictly parsed one electricity
/// business response.  It is intentionally opaque: the proof carries no
/// Cookie, URL, account value, or response body.  A proof is produced by the
/// `*_with_proof` methods only after the corresponding page has passed the
/// route, content-type, authentication-page, and business parser checks.
#[derive(Clone, PartialEq, Eq)]
pub struct ElectricityBusinessProof {
    adapter_binding: u64,
    operation: DormElectricityOperation,
}

impl fmt::Debug for ElectricityBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated remainder together with the business proof created by the
/// same adapter and Cookie-aware transport.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityRemainderRead {
    pub value: ElectricityRemainder,
    pub proof: ElectricityBusinessProof,
}

/// A validated payment history together with the business proof created by
/// the same adapter and Cookie-aware transport.
#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityPaymentHistoryRead {
    pub value: ElectricityPaymentHistory,
    pub proof: ElectricityBusinessProof,
}

/// Parser failures contain only stable field names and positions.  They do
/// not retain untrusted HTML, Cookie values, tickets, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DormElectricityParseError {
    #[error("electricity response body is empty")]
    EmptyBody,

    #[error("electricity response is an HTML login page")]
    LoginPage,

    #[error("electricity response is an expired or timed-out page")]
    ExpiredPage,

    #[error("electricity response is not a recognized HTML page")]
    UnexpectedHtml,

    #[error("electricity response is missing the {field} field")]
    MissingField { field: &'static str },

    #[error("electricity response contains duplicate {field} fields")]
    DuplicateField { field: &'static str },

    #[error("electricity response contains an empty {field} field")]
    EmptyField { field: &'static str },

    #[error("electricity {field} is not a finite number")]
    InvalidNumber { field: &'static str },

    #[error("electricity {field} is outside the supported numeric range")]
    NumberOutOfRange { field: &'static str },

    #[error("electricity {field} is not a valid timestamp")]
    InvalidTimestamp { field: &'static str },

    #[error("electricity payment history table is missing")]
    MissingHistoryTable,

    #[error("electricity payment history table is ambiguous")]
    DuplicateHistoryTable,

    #[error("electricity payment history table has no header and footer boundary")]
    InvalidHistoryTable,

    #[error("electricity payment history row {row} does not contain exactly six cells")]
    InvalidHistoryCellCount { row: usize },

    #[error("electricity payment history row {row} has an invalid sequence")]
    InvalidHistorySequence { row: usize },

    #[error("electricity payment history row {row} has an invalid timestamp")]
    InvalidHistoryTimestamp { row: usize },

    #[error("electricity payment history row {row} has an invalid amount")]
    InvalidHistoryAmount { row: usize },

    #[error("electricity payment history row {row} has an empty status")]
    EmptyHistoryStatus { row: usize },
}

/// Adapter failures are body-free so an HTML login page cannot leak through
/// a debug or bridge DTO.
#[derive(Debug, Error)]
pub enum DormElectricityAdapterError {
    #[error("electricity WebVPN base URL is invalid")]
    InvalidBaseUrl,

    #[error("electricity transport failed")]
    Transport(#[source] TransportError),

    #[error("electricity request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("electricity response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("electricity response ended outside the configured WebVPN mapping")]
    UnexpectedPath,

    #[error("electricity response is not an HTML document")]
    UnexpectedContentType,

    #[error("electricity WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("electricity page template is not recognized")]
    UnexpectedDeployment,

    #[error("electricity response could not be parsed: {0}")]
    Parse(#[source] DormElectricityParseError),
}

impl DormElectricityAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "electricity_config",
            Self::Transport(_) => "electricity_network",
            Self::HttpStatus { .. } => "electricity_http",
            Self::UnexpectedOrigin => "electricity_origin",
            Self::UnexpectedPath => "electricity_path",
            Self::UnexpectedContentType => "electricity_content_type",
            Self::SessionExpired => "electricity_auth_required",
            Self::UnexpectedDeployment => "electricity_template",
            Self::Parse(DormElectricityParseError::EmptyBody) => "electricity_body_empty",
            Self::Parse(DormElectricityParseError::LoginPage) => "electricity_auth_required",
            Self::Parse(DormElectricityParseError::ExpiredPage) => "electricity_auth_required",
            Self::Parse(DormElectricityParseError::UnexpectedHtml) => "electricity_template",
            Self::Parse(DormElectricityParseError::MissingField { .. }) => {
                "electricity_field_missing"
            }
            Self::Parse(DormElectricityParseError::DuplicateField { .. }) => {
                "electricity_field_duplicate"
            }
            Self::Parse(DormElectricityParseError::EmptyField { .. }) => "electricity_field_empty",
            Self::Parse(DormElectricityParseError::InvalidTimestamp { .. }) => {
                "electricity_timestamp"
            }
            Self::Parse(DormElectricityParseError::InvalidNumber { .. }) => "electricity_amount",
            Self::Parse(DormElectricityParseError::NumberOutOfRange { .. }) => {
                "electricity_number_range"
            }
            Self::Parse(DormElectricityParseError::MissingHistoryTable) => {
                "electricity_history_table_missing"
            }
            Self::Parse(DormElectricityParseError::DuplicateHistoryTable) => {
                "electricity_history_table_duplicate"
            }
            Self::Parse(DormElectricityParseError::InvalidHistoryTable) => {
                "electricity_history_table_shape"
            }
            Self::Parse(DormElectricityParseError::InvalidHistoryCellCount { .. }) => {
                "electricity_history_row_shape"
            }
            Self::Parse(DormElectricityParseError::InvalidHistorySequence { .. }) => {
                "electricity_history_sequence"
            }
            Self::Parse(DormElectricityParseError::InvalidHistoryTimestamp { .. }) => {
                "electricity_history_timestamp"
            }
            Self::Parse(DormElectricityParseError::InvalidHistoryAmount { .. }) => {
                "electricity_history_amount"
            }
            Self::Parse(DormElectricityParseError::EmptyHistoryStatus { .. }) => {
                "electricity_history_status"
            }
            Self::Parse(_) => "electricity_parse",
        }
    }
    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(DormElectricityParseError::LoginPage)
                | Self::Parse(DormElectricityParseError::ExpiredPage)
        )
    }
}

/// Configuration for a read-only electricity adapter.
#[derive(Clone)]
pub struct DormElectricityAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl DormElectricityAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, DormElectricityAdapterError> {
        Self::with_user_agent_and_timeout(
            base_url,
            "THYou/dorm-electricity",
            Duration::from_secs(20),
        )
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, DormElectricityAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(20))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, DormElectricityAdapterError> {
        let base_url =
            Url::parse(base_url).map_err(|_| DormElectricityAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;

        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(DormElectricityAdapterError::InvalidBaseUrl);
        }

        Ok(Self {
            base_url,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, DormElectricityAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(DormElectricityAdapterError::Transport)
    }
}

impl fmt::Debug for DormElectricityAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DormElectricityAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Read-only electricity client.  `try_with_transport` is the normal runtime
/// entry point: the transport must be the one that already carries the
/// identity/INFO/WebVPN cookie jar.  `new` is retained for callers that own a
/// separately established WebVPN transport.
pub struct DormElectricityAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: DormElectricityProfile,
    binding: u64,
}

impl DormElectricityAdapter {
    pub fn new(config: DormElectricityAdapterConfig) -> Result<Self, DormElectricityAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, DormElectricityAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: DormElectricityProfile::standard(),
            binding: NEXT_ELECTRICITY_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> DormElectricityProfile {
        self.profile
    }

    pub async fn read_remainder(
        &self,
    ) -> Result<ElectricityRemainder, DormElectricityAdapterError> {
        self.read_remainder_with_proof()
            .await
            .map(|read| read.value)
    }

    /// Reads and proves the electricity remainder through this adapter.
    /// `read_remainder` remains as the compatibility convenience method, but
    /// callers that need an explicit service capability should retain this
    /// returned proof instead of inferring it from INFO authentication.
    pub async fn read_remainder_with_proof(
        &self,
    ) -> Result<ElectricityRemainderRead, DormElectricityAdapterError> {
        let plan = self.profile.remainder_request();
        let body = self.execute(&plan).await?;
        let value = parse_electricity_remainder_html(&body).map_err(Self::map_parse_error)?;
        Ok(ElectricityRemainderRead {
            value,
            proof: self.business_proof(DormElectricityOperation::ReadRemainder),
        })
    }

    pub async fn read_payment_history(
        &self,
    ) -> Result<ElectricityPaymentHistory, DormElectricityAdapterError> {
        self.read_payment_history_with_proof()
            .await
            .map(|read| read.value)
    }

    /// Reads and proves the electricity payment history through this adapter.
    pub async fn read_payment_history_with_proof(
        &self,
    ) -> Result<ElectricityPaymentHistoryRead, DormElectricityAdapterError> {
        let plan = self.profile.payment_history_request();
        let body = self.execute(&plan).await?;
        let value = parse_electricity_payment_history_html(&body).map_err(Self::map_parse_error)?;
        Ok(ElectricityPaymentHistoryRead {
            value,
            proof: self.business_proof(DormElectricityOperation::ReadPaymentHistory),
        })
    }

    /// Checks that a business proof came from this adapter instance.  The
    /// proof is deliberately adapter-bound because a different adapter may
    /// own a different base URL or Cookie jar even when both use the same
    /// route profile.
    pub fn business_proof_matches(&self, proof: &ElectricityBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    fn business_proof(&self, operation: DormElectricityOperation) -> ElectricityBusinessProof {
        ElectricityBusinessProof {
            adapter_binding: self.binding,
            operation,
        }
    }

    async fn execute(
        &self,
        plan: &DormElectricityRequestPlan,
    ) -> Result<String, DormElectricityAdapterError> {
        let endpoint = self.endpoint(plan.path)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| {
                DormElectricityAdapterError::Transport(TransportError::Request(error))
            })?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| DormElectricityAdapterError::UnexpectedOrigin)?;
        let location_target = location
            .as_deref()
            .map(|value| resolve_location(&final_url, value))
            .transpose()?;
        // When a WebVPN session expires, the mapped URL first reaches the
        // same-origin `/login` endpoint. WebVPN then emits an OAuth Location
        // on another origin. The shared transport deliberately stops at that
        // boundary, so classify the login endpoint before the origin policy
        // turns the response into a generic origin error.
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || location_target.as_ref().is_some_and(|target| {
                same_origin(&self.base_url, target) && looks_like_login_url(target)
            })
        {
            return Err(DormElectricityAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(DormElectricityAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(DormElectricityAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| {
                DormElectricityAdapterError::Transport(TransportError::Decode(error))
            })?;
        if body.len() > MAX_HTML_BYTES {
            return Err(DormElectricityAdapterError::UnexpectedDeployment);
        }

        let page_classification = classify_html_page(&body);
        if matches!(
            page_classification,
            PageClassification::Login | PageClassification::Expired
        ) {
            return Err(DormElectricityAdapterError::SessionExpired);
        }
        if status != StatusCode::OK {
            return Err(DormElectricityAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.query().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(DormElectricityAdapterError::UnexpectedPath);
        }
        if !is_html_content_type(content_type.as_deref()) {
            return Err(DormElectricityAdapterError::UnexpectedContentType);
        }
        if !matches!(page_classification, PageClassification::AuthenticatedHtml) {
            return Err(DormElectricityAdapterError::UnexpectedDeployment);
        }
        Ok(body)
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, DormElectricityAdapterError> {
        if !valid_relative_path(relative_path) {
            return Err(DormElectricityAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{relative_path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: DormElectricityParseError) -> DormElectricityAdapterError {
        match error {
            DormElectricityParseError::LoginPage | DormElectricityParseError::ExpiredPage => {
                DormElectricityAdapterError::SessionExpired
            }
            DormElectricityParseError::UnexpectedHtml => {
                DormElectricityAdapterError::UnexpectedDeployment
            }
            other => DormElectricityAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for DormElectricityAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DormElectricityAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Parses the exact remainder page.  The login marker is checked before
/// selectors so an expired HTTP 200 page cannot become a missing-field data
/// result or an empty number.
pub fn parse_electricity_remainder_html(
    html: &str,
) -> Result<ElectricityRemainder, DormElectricityParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        if html.trim_start_matches('\u{feff}').trim().is_empty() {
            return Err(DormElectricityParseError::EmptyBody);
        }
        match classify_html_page(html) {
            PageClassification::Login => return Err(DormElectricityParseError::LoginPage),
            PageClassification::Expired => return Err(DormElectricityParseError::ExpiredPage),
            PageClassification::AuthenticatedHtml => {}
            PageClassification::Other => return Err(DormElectricityParseError::UnexpectedHtml),
        }

        let remainder_text = unique_element_text(
            html,
            "Netweb_Home_electricity_DetailCtrl1_lblele",
            "remainder",
        )?;
        let update_time = unique_element_text(
            html,
            "Netweb_Home_electricity_DetailCtrl1_lbltime",
            "update_time",
        )?;
        if update_time.is_empty() {
            return Err(DormElectricityParseError::EmptyField {
                field: "update_time",
            });
        }
        if !valid_legacy_timestamp(&update_time) {
            return Err(DormElectricityParseError::InvalidTimestamp {
                field: "update_time",
            });
        }
        let remainder = parse_finite_number(&remainder_text, "remainder")?;

        Ok(ElectricityRemainder {
            remainder,
            update_time,
        })
    })
}

/// Parses the six-cell `.myTable` history contract.  The first and last rows
/// are the observed header/footer boundary.  A table with exactly those two
/// rows is a valid empty history; a missing table is an error.
pub fn parse_electricity_payment_history_html(
    html: &str,
) -> Result<ElectricityPaymentHistory, DormElectricityParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        if html.trim_start_matches('\u{feff}').trim().is_empty() {
            return Err(DormElectricityParseError::EmptyBody);
        }
        match classify_html_page(html) {
            PageClassification::Login => return Err(DormElectricityParseError::LoginPage),
            PageClassification::Expired => return Err(DormElectricityParseError::ExpiredPage),
            PageClassification::AuthenticatedHtml => {}
            PageClassification::Other => return Err(DormElectricityParseError::UnexpectedHtml),
        }

        // The reference selector is `.myTable tr`, so keep the class selector
        // independent of the wrapper tag. Some deployments put `myTable` on a
        // surrounding div rather than directly on the table element. A page can
        // also contain an empty or unrelated node with the same legacy class;
        // only one structurally valid history table may establish the result.
        let tables = find_elements_with_any_class(html, "myTable");
        if tables.is_empty() {
            return Err(DormElectricityParseError::MissingHistoryTable);
        }

        // Cheerio's `.myTable tr` selects each descendant row once. Legacy pages
        // sometimes put the same class on a wrapper and on its nested table;
        // parsing both scopes independently can manufacture an ambiguity even
        // though the reference selector sees one row stream. Keep only the
        // outermost matching scopes, while retaining the separate-sibling check
        // below for genuinely conflicting legacy tables.
        let outermost_tables = tables
            .iter()
            .copied()
            .filter(|candidate| {
                !tables.iter().any(|container| {
                    container.inner_start < candidate.inner_start
                        && container.inner_end > candidate.inner_end
                })
            })
            .collect::<Vec<_>>();

        let mut valid_tables = Vec::new();
        let mut first_error = None;
        for table in outermost_tables {
            let table_inner = &html[table.inner_start..table.inner_end];
            match parse_history_table_inner(table_inner) {
                Ok(history) => valid_tables.push(history),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }

        match valid_tables.as_slice() {
            [] => return Err(first_error.unwrap_or(DormElectricityParseError::InvalidHistoryTable)),
            [history] => return Ok(history.clone()),
            histories if histories.windows(2).all(|pair| pair[0] == pair[1]) => {
                return Ok(histories[0].clone());
            }
            _ => return Err(DormElectricityParseError::DuplicateHistoryTable),
        }
    })
}

fn parse_history_table_inner(
    table_inner: &str,
) -> Result<ElectricityPaymentHistory, DormElectricityParseError> {
    let rows = find_elements_with_tag(table_inner, "tr");
    if rows.len() < 2 {
        return Err(DormElectricityParseError::InvalidHistoryTable);
    }

    let mut records = Vec::with_capacity(rows.len().saturating_sub(2));
    for (row_index, row) in rows[1..rows.len() - 1].iter().enumerate() {
        let row_html = &table_inner[row.inner_start..row.inner_end];
        let cells = find_elements_with_tag(row_html, "td");
        if cells.len() != 6 {
            return Err(DormElectricityParseError::InvalidHistoryCellCount { row: row_index });
        }
        let mut columns = [(); 6].map(|_| String::new());
        for (index, cell) in cells.iter().enumerate() {
            columns[index] = collapse_html_text(&row_html[cell.inner_start..cell.inner_end]);
        }

        let sequence = if columns[1].is_empty() {
            None
        } else {
            Some(columns[1].parse::<u64>().map_err(|_| {
                DormElectricityParseError::InvalidHistorySequence { row: row_index }
            })?)
        };
        if columns[2].is_empty() || !valid_legacy_timestamp(&columns[2]) {
            return Err(DormElectricityParseError::InvalidHistoryTimestamp { row: row_index });
        }
        let amount = parse_finite_number(&columns[4], "history amount").map_err(|error| {
            if matches!(error, DormElectricityParseError::NumberOutOfRange { .. }) {
                DormElectricityParseError::InvalidHistoryAmount { row: row_index }
            } else {
                DormElectricityParseError::InvalidHistoryAmount { row: row_index }
            }
        })?;
        if columns[5].is_empty() {
            return Err(DormElectricityParseError::EmptyHistoryStatus { row: row_index });
        }

        let occurred_at = columns[2].clone();
        let status = columns[5].clone();
        records.push(ElectricityPaymentRecord {
            source_columns: columns,
            sequence,
            occurred_at,
            amount,
            status,
        });
    }

    Ok(ElectricityPaymentHistory { records })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageClassification {
    AuthenticatedHtml,
    Login,
    Expired,
    Other,
}

fn classify_html_page(body: &str) -> PageClassification {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    let lower = trimmed.to_ascii_lowercase();
    let login_marker = lower.contains("net_default_loginctrl1_txtusername")
        || lower.contains("name=\"i_user\"")
        || lower.contains("name='i_user'")
        || lower.contains("name=i_user")
        || lower.contains("name=\"i_pass\"")
        || lower.contains("name='i_pass'")
        || lower.contains("name=i_pass")
        || lower.contains("/do/off/ui/auth/login")
        || lower.contains("统一身份认证")
        || lower.contains("j_acegi_formlogin")
        || (lower.contains("<form")
            && (lower.contains("txtusername") || lower.contains("i_user"))
            && (lower.contains("txtuserpwd") || lower.contains("i_pass")));
    if login_marker {
        return PageClassification::Login;
    }

    let expired_marker = lower.contains(ELECTRICITY_TIMEOUT_MESSAGE)
        || lower.contains("timeout用户登陆超时或访问内容不存在")
        || lower.contains("session expired")
        || lower.contains("login required")
        || lower.contains("会话已过期")
        || lower.contains("登录失效")
        || lower.contains("请先登录")
        || lower.contains("请登录")
        // A generic request/access timeout is a service failure, not proof
        // that authentication expired. Keep only explicit session markers.
        || lower.contains("webvpn timeout");
    if expired_marker {
        return PageClassification::Expired;
    }

    if !looks_like_html(trimmed) {
        return PageClassification::Other;
    }

    PageClassification::AuthenticatedHtml
}

fn parse_finite_number(text: &str, field: &'static str) -> Result<f64, DormElectricityParseError> {
    let normalized = text.trim().replace(',', "");
    if normalized.is_empty() {
        return Err(DormElectricityParseError::InvalidNumber { field });
    }
    let number = normalized
        .parse::<f64>()
        .map_err(|_| DormElectricityParseError::InvalidNumber { field })?;
    if !number.is_finite() {
        return Err(DormElectricityParseError::InvalidNumber { field });
    }
    if number.abs() > 1_000_000_000_000.0 {
        return Err(DormElectricityParseError::NumberOutOfRange { field });
    }
    Ok(number)
}

fn valid_legacy_timestamp(value: &str) -> bool {
    let mut pieces = value.split_whitespace();
    let date = pieces.next().unwrap_or_default();
    let time = pieces.next().unwrap_or_default();
    pieces.next().is_none() && valid_legacy_date(date) && valid_legacy_time(time)
}

fn valid_legacy_date(value: &str) -> bool {
    let separator = if value.contains('/') { '/' } else { '-' };
    let fields = value.split(separator).collect::<Vec<_>>();
    if fields.len() != 3 || fields.iter().any(|field| field.is_empty()) {
        return false;
    }
    let year = fields[0].parse::<i32>().ok();
    let month = fields[1].parse::<u32>().ok();
    let day = fields[2].parse::<u32>().ok();
    match (year, month, day) {
        (Some(year), Some(month), Some(day)) => {
            chrono::NaiveDate::from_ymd_opt(year, month, day).is_some()
        }
        _ => false,
    }
}

fn valid_legacy_time(value: &str) -> bool {
    let fields = value.split(':').collect::<Vec<_>>();
    if !(fields.len() == 2 || fields.len() == 3) || fields.iter().any(|field| field.is_empty()) {
        return false;
    }
    let hour = fields[0].parse::<u32>().ok();
    let minute = fields[1].parse::<u32>().ok();
    let second = fields
        .get(2)
        .map(|field| field.parse::<u32>().ok())
        .unwrap_or(Some(0));
    matches!((hour, minute, second), (Some(hour), Some(minute), Some(second)) if hour < 24 && minute < 60 && second < 60)
}

fn unique_element_text(
    html: &str,
    id: &'static str,
    field: &'static str,
) -> Result<String, DormElectricityParseError> {
    let elements = find_elements_with_id(html, id);
    let element = match elements.as_slice() {
        [] => return Err(DormElectricityParseError::MissingField { field }),
        [element] => element,
        _ => return Err(DormElectricityParseError::DuplicateField { field }),
    };
    let value = collapse_html_text(&html[element.inner_start..element.inner_end]);
    if value.is_empty() {
        return Err(DormElectricityParseError::EmptyField { field });
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy)]
struct ElementSpan {
    inner_start: usize,
    inner_end: usize,
}

fn find_elements_with_id(html: &str, expected_id: &str) -> Vec<ElementSpan> {
    let mut elements = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find('<') {
        let start = cursor + relative;
        let Some((tag_name, start_end, attributes, self_closing)) = parse_start_tag(html, start)
        else {
            cursor = start.saturating_add(1);
            continue;
        };
        cursor = start_end.saturating_add(1);
        if matches!(tag_name.as_str(), "script" | "style") && !self_closing {
            if let Some((_, close_end)) = find_raw_text_close(html, cursor, &tag_name) {
                cursor = close_end.saturating_add(1);
                continue;
            }
        }
        if self_closing || !attribute_equals(&attributes, "id", expected_id) {
            continue;
        }
        let Some((close_start, close_end)) = find_matching_close(&lower, start_end + 1, &tag_name)
        else {
            continue;
        };
        elements.push(ElementSpan {
            inner_start: start_end + 1,
            inner_end: close_start,
        });
        cursor = close_end.saturating_add(1);
    }
    elements
}

fn find_elements_with_any_class(html: &str, expected_class: &str) -> Vec<ElementSpan> {
    find_elements(html, "", |attributes| {
        attribute_contains_class(attributes, expected_class)
    })
}

fn find_elements_with_tag(html: &str, expected_tag: &str) -> Vec<ElementSpan> {
    find_elements(html, expected_tag, |_| true)
}

fn find_elements<F>(html: &str, expected_tag: &str, predicate: F) -> Vec<ElementSpan>
where
    F: Fn(&[(String, Option<String>)]) -> bool,
{
    let mut elements = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find('<') {
        let start = cursor + relative;
        let Some((tag_name, start_end, attributes, self_closing)) = parse_start_tag(html, start)
        else {
            cursor = start.saturating_add(1);
            continue;
        };
        cursor = start_end.saturating_add(1);
        if matches!(tag_name.as_str(), "script" | "style") && !self_closing {
            if let Some((_, close_end)) = find_raw_text_close(html, cursor, &tag_name) {
                cursor = close_end.saturating_add(1);
                continue;
            }
        }
        let tag_matches = expected_tag.is_empty() || tag_name == expected_tag.to_ascii_lowercase();
        if !tag_matches || self_closing || !predicate(&attributes) {
            continue;
        }
        let Some((close_start, close_end)) = find_matching_close(&lower, start_end + 1, &tag_name)
        else {
            continue;
        };
        elements.push(ElementSpan {
            inner_start: start_end + 1,
            inner_end: close_start,
        });
        cursor = close_end.saturating_add(1);
    }
    elements
}

fn parse_start_tag(
    html: &str,
    start: usize,
) -> Option<(String, usize, Vec<(String, Option<String>)>, bool)> {
    let bytes = html.as_bytes();
    if bytes.get(start) != Some(&b'<') || matches!(bytes.get(start + 1), Some(b'/' | b'!' | b'?')) {
        return None;
    }
    let end = find_tag_end(html, start + 1)?;
    let raw = &html[start..=end];
    let mut cursor = start + 1;
    skip_ascii_whitespace(bytes, &mut cursor, end);
    let name_start = cursor;
    while cursor < end && is_tag_name_byte(bytes[cursor]) {
        cursor += 1;
    }
    if name_start == cursor {
        return None;
    }
    let tag_name = html[name_start..cursor].to_ascii_lowercase();
    let self_closing = raw[..raw.len().saturating_sub(1)].trim_end().ends_with('/');
    let mut attributes = Vec::new();
    while cursor < end {
        skip_ascii_whitespace(bytes, &mut cursor, end);
        if cursor >= end || bytes[cursor] == b'/' {
            break;
        }
        let attribute_start = cursor;
        while cursor < end && is_attribute_name_byte(bytes[cursor]) {
            cursor += 1;
        }
        if attribute_start == cursor {
            cursor += 1;
            continue;
        }
        let attribute_name = html[attribute_start..cursor].to_ascii_lowercase();
        skip_ascii_whitespace(bytes, &mut cursor, end);
        let value = if cursor < end && bytes[cursor] == b'=' {
            cursor += 1;
            skip_ascii_whitespace(bytes, &mut cursor, end);
            if cursor >= end {
                Some(String::new())
            } else if matches!(bytes[cursor], b'\'' | b'"') {
                let quote = bytes[cursor];
                cursor += 1;
                let value_start = cursor;
                while cursor < end && bytes[cursor] != quote {
                    cursor += 1;
                }
                let value = html[value_start..cursor].to_owned();
                if cursor < end {
                    cursor += 1;
                }
                Some(value)
            } else {
                let value_start = cursor;
                while cursor < end && !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'>'
                {
                    cursor += 1;
                }
                Some(html[value_start..cursor].trim_end_matches('/').to_owned())
            }
        } else {
            None
        };
        attributes.push((attribute_name, value));
    }
    Some((tag_name, end, attributes, self_closing))
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let bytes = html.as_bytes();
    let mut quote = None;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        if let Some(expected) = quote {
            if *byte == expected {
                quote = None;
            }
        } else if matches!(*byte, b'\'' | b'"') {
            quote = Some(*byte);
        } else if *byte == b'>' {
            return Some(offset);
        }
    }
    None
}

fn find_matching_close(
    lower_html: &str,
    start: usize,
    expected_tag: &str,
) -> Option<(usize, usize)> {
    let mut cursor = start;
    let mut depth = 1usize;
    while let Some(relative) = lower_html[cursor..].find('<') {
        let tag_start = cursor + relative;
        if lower_html[tag_start..].starts_with("<!--") {
            cursor = lower_html[tag_start..]
                .find("-->")
                .map(|end| tag_start + end + 3)
                .unwrap_or(lower_html.len());
            continue;
        }
        let Some(tag_end) = find_tag_end(lower_html, tag_start + 1) else {
            return None;
        };
        let raw = lower_html[tag_start..=tag_end].trim();
        let closing = raw.starts_with("</");
        let name_start = if closing { 2 } else { 1 };
        let mut name_end = name_start;
        while name_end < raw.len() && is_tag_name_byte(raw.as_bytes()[name_end]) {
            name_end += 1;
        }
        let name = &raw[name_start..name_end];
        if !closing && matches!(name, "script" | "style") {
            let (_, close_end) = find_raw_text_close(lower_html, tag_end + 1, name)?;
            cursor = close_end.saturating_add(1);
            continue;
        }
        if name == expected_tag {
            if closing {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((tag_start, tag_end));
                }
            } else if !raw.ends_with("/>") && !is_void_tag(name) {
                depth = depth.saturating_add(1);
            }
        }
        cursor = tag_end.saturating_add(1);
    }
    None
}

fn find_raw_text_close(html: &str, start: usize, tag: &str) -> Option<(usize, usize)> {
    if start >= html.len() {
        return None;
    }
    let needle = format!("</{tag}");
    let mut cursor = start;
    while let Some(relative) = html[cursor..].find(&needle) {
        let close_start = cursor + relative;
        let after_name = close_start + needle.len();
        let boundary = html.as_bytes().get(after_name).copied();
        if boundary.is_some_and(|byte| byte.is_ascii_whitespace() || byte == b'>') {
            let close_end = find_tag_end(html, close_start + 1)?;
            return Some((close_start, close_end));
        }
        cursor = after_name;
    }
    None
}

fn attribute_equals(attributes: &[(String, Option<String>)], name: &str, value: &str) -> bool {
    attributes
        .iter()
        .any(|(attribute, candidate)| attribute == name && candidate.as_deref() == Some(value))
}

fn attribute_contains_class(attributes: &[(String, Option<String>)], expected_class: &str) -> bool {
    attributes.iter().any(|(attribute, value)| {
        attribute == "class"
            && value.as_deref().is_some_and(|classes| {
                classes
                    .split_ascii_whitespace()
                    .any(|class| class.eq_ignore_ascii_case(expected_class))
            })
    })
}

fn collapse_html_text(fragment: &str) -> String {
    let mut text = String::new();
    let bytes = fragment.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'<' {
            if fragment[cursor..].starts_with("<!--") {
                cursor = fragment[cursor..]
                    .find("-->")
                    .map(|end| cursor + end + 3)
                    .unwrap_or(bytes.len());
            } else if let Some(end) = fragment[cursor..].find('>') {
                cursor += end + 1;
            } else {
                break;
            }
            continue;
        }
        if bytes[cursor] == b'&' {
            if let Some(end) = fragment[cursor..].find(';') {
                let entity = &fragment[cursor + 1..cursor + end];
                if let Some(decoded) = decode_html_entity(entity) {
                    text.push(decoded);
                    cursor += end + 1;
                    continue;
                }
            }
        }
        let Some(character) = fragment[cursor..].chars().next() else {
            break;
        };
        text.push(character);
        cursor += character.len_utf8();
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "nbsp" => Some(' '),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"));
            if let Some(number) = number {
                u32::from_str_radix(number, 16)
                    .ok()
                    .and_then(char::from_u32)
            } else {
                entity
                    .strip_prefix('#')
                    .and_then(|number| number.parse::<u32>().ok())
                    .and_then(char::from_u32)
            }
        }
    }
}

fn looks_like_html(body: &str) -> bool {
    let lower = body
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.starts_with("<head")
        || lower.starts_with("<body")
        || lower.starts_with("<form")
        || lower.starts_with("<table")
        || lower.starts_with("<span")
}

fn is_html_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
    })
}

fn looks_like_login_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path == "/login"
        || path.ends_with("/login")
        || path.contains("/login/")
        || path.contains("/do/off/ui/auth/login")
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn path_within_base(base_url: &Url, candidate: &Url) -> bool {
    let base_path = base_url.path().trim_end_matches('/');
    base_path.is_empty()
        || base_path == "/"
        || candidate.path() == base_path
        || candidate.path().starts_with(&format!("{base_path}/"))
}

fn normalize_base_url(mut base_url: Url) -> Result<Url, DormElectricityAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(DormElectricityAdapterError::InvalidBaseUrl);
    }

    let path = base_url.path().trim_end_matches('/');
    let path = opaque_mapping_root(path)
        .or_else(|| strip_known_service_suffix(path))
        .unwrap_or_else(|| {
            if path.is_empty() {
                "/".to_owned()
            } else {
                format!("{path}/")
            }
        });
    base_url.set_path(&path);
    Ok(base_url)
}

fn opaque_mapping_root(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let scheme = segments.next()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let token = segments.next()?;
    Some(format!("/{scheme}/{token}/"))
}

fn strip_known_service_suffix(path: &str) -> Option<String> {
    [ELECTRICITY_REMAINDER_PATH, ELECTRICITY_PAYMENT_HISTORY_PATH]
        .into_iter()
        .find_map(|suffix| {
            let prefix = path.strip_suffix(suffix)?;
            if prefix.is_empty() {
                Some("/".to_owned())
            } else {
                Some(format!("{}/", prefix.trim_end_matches('/')))
            }
        })
}

fn resolve_location(
    response_url: &Url,
    location: &str,
) -> Result<Url, DormElectricityAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(DormElectricityAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| DormElectricityAdapterError::UnexpectedOrigin)?;
    if target.username() != "" || target.password().is_some() {
        return Err(DormElectricityAdapterError::UnexpectedOrigin);
    }
    Ok(target)
}

fn safe_base_path(path: &str) -> bool {
    !path.contains(['\\', '?', '#'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

fn valid_relative_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains("://")
        && !path.contains(['?', '#', '\\'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
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

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn skip_ascii_whitespace(bytes: &[u8], cursor: &mut usize, end: usize) {
    while *cursor < end && bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

fn is_tag_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-')
}

fn is_attribute_name_byte(byte: u8) -> bool {
    is_tag_name_byte(byte)
}

fn is_void_tag(name: &str) -> bool {
    matches!(
        name,
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
}
