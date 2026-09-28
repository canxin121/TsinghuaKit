//! Read-only teaching-evaluation questionnaire list (`教学评估`).
//!
//! The evaluation service is a legacy JSP application on its own campus host,
//! reached through the WebVPN mapping bound to selector
//! [`ASSESSMENT_WEBVPN_TARGET`].  One GET returns the list of courses the
//! student can evaluate; each row carries the course name, whether that course
//! has already been evaluated, and an inline JavaScript call that names the
//! form route for that row.
//!
//! Three service behaviours shape this module:
//!
//! * The service answers a 200 HTML page containing
//!   `对不起，现在不是填写问卷时间` while the questionnaire window is closed.
//!   That is an explicit "not open" answer, not an empty list, and it is
//!   reported as its own failure category so a caller cannot present a closed
//!   window as "nothing left to do".
//! * An empty list is also a failure.  The public reference throws on it, and
//!   the reasoning holds here: this page only exists during the window, so
//!   zero rows means the page did not render, not that every course is done.
//! * The list is a positional legacy table.  [`campus_html`] deliberately
//!   refuses positional indexing, so this module owns that decision and pays
//!   for it with a structural anchor: every accepted row must carry the inline
//!   `Body('…')` call in its action cell, and the route that call names must be
//!   a plain same-mapping path.  A page whose columns moved therefore fails
//!   instead of reporting one column's text as another column's value.
//!
//! The form route itself never leaves the engine.  A row exposes an opaque
//! [`AssessmentRef`] that only the adapter instance which produced it can
//! resolve, so a later stage can fetch and submit a form without a caller ever
//! holding — or forging — a service path.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{
    fmt,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::campus_html::{self, PageClass, RawElement, ScanError};
use crate::transport::{CampusHttpTransport, TransportError};

/// The evaluation host's WebVPN roaming selector.
pub const ASSESSMENT_WEBVPN_TARGET: &str = "0D8B99BA23FD2BA22428D9C8AA0AB508";

/// The questionnaire list endpoint behind the selector above.  It takes no
/// query parameters.
pub const ASSESSMENT_LIST_PATH: &str = "/jxpg/f/jxpg/wj/xs/pgkcList";

/// The service's explicit "the questionnaire window is closed" answer.
pub const ASSESSMENT_NOT_OPEN_MARKER: &str = "对不起，现在不是填写问卷时间";

/// The list cell holding the course name, as the legacy table lays it out.
const NAME_CELL: usize = 5;
/// The list cell holding the evaluated flag.
const EVALUATED_CELL: usize = 9;
/// The list cell holding the action that names this row's form route.
const ACTION_CELL: usize = 11;
/// The lowest cell count an accepted row can have, since the action cell is
/// the last one this module reads.
const MIN_ROW_CELLS: usize = ACTION_CELL + 1;
/// The service writes exactly this text in the evaluated cell for a course that
/// has been evaluated.  Any other text means "not evaluated", which is the
/// observed reference behaviour and lets the service reword the negative case.
const EVALUATED_TEXT: &str = "是";
/// The inline call that carries the form route in the action cell.
const ACTION_CALL_OPEN: &str = "Body('";
/// The delimiter that ends the route inside that call.
const ACTION_CALL_CLOSE: &str = "') })";

const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_ITEMS: usize = 2048;
const MAX_NAME_CHARS: usize = 256;
const MAX_ROUTE_CHARS: usize = 512;

static NEXT_ASSESSMENT_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This profile only issues GETs, so a write route cannot be smuggled into a
/// read plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentMethod {
    Get,
}

/// The observed evaluation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentOperation {
    ReadList,
}

/// An evaluation read requires an already established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path and query only, never an absolute
/// WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentRequestPlan {
    pub operation: AssessmentOperation,
    pub method: AssessmentMethod,
    pub path: &'static str,
    pub query: &'static str,
    pub webvpn_target: &'static str,
    pub session_prerequisite: AssessmentSessionPrerequisite,
}

impl AssessmentRequestPlan {
    /// Returns the serialized query without the leading `?`.
    pub fn query_string(&self) -> &str {
        self.query
    }
}

/// Fixed evaluation route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssessmentProfile;

impl AssessmentProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        ASSESSMENT_WEBVPN_TARGET
    }

    pub fn list_request(self) -> AssessmentRequestPlan {
        AssessmentRequestPlan {
            operation: AssessmentOperation::ReadList,
            method: AssessmentMethod::Get,
            path: ASSESSMENT_LIST_PATH,
            query: "",
            webvpn_target: ASSESSMENT_WEBVPN_TARGET,
            session_prerequisite: AssessmentSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// One parsed row, before the adapter attaches its opaque reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentRow {
    /// The course name exactly as the service rendered it, bounded in length.
    pub name: String,
    /// True when the service marked this course as already evaluated.
    pub evaluated: bool,
    /// The form route this row's action cell named.
    pub route: String,
}

/// An opaque reference to one row's evaluation form.
///
/// The route stays inside the adapter that produced it.  A reference from a
/// different adapter instance, from a superseded list, or beyond the range of
/// the list it came from does not resolve at all, so a caller cannot point a
/// later stage at a route of its own choosing.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AssessmentRef {
    adapter_binding: u64,
    generation: u64,
    index: u32,
}

impl AssessmentRef {
    fn new(adapter_binding: u64, generation: u64, index: u32) -> Self {
        Self {
            adapter_binding,
            generation,
            index,
        }
    }

    /// The position of this row inside the list that produced it.
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl fmt::Debug for AssessmentRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentRef")
            .field("index", &self.index)
            .finish()
    }
}

/// One course awaiting (or already given) a teaching evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct AssessmentItem {
    /// The course name exactly as the service rendered it.
    pub name: String,
    /// True when the service marked this course as already evaluated.
    pub evaluated: bool,
    /// Opaque handle for this row's form route.
    pub reference: AssessmentRef,
}

/// A validated questionnaire list.
#[derive(Debug, Clone, PartialEq)]
pub struct AssessmentList {
    pub items: Vec<AssessmentItem>,
}

impl AssessmentList {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The routes named by one list read, matched to the rows by position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentListRows {
    pub rows: Vec<AssessmentRow>,
}

/// Parser failures retain only stable names.  They never keep response bytes,
/// Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssessmentParseError {
    #[error("assessment response body is empty")]
    EmptyBody,

    #[error("assessment response is an HTML login page")]
    LoginPage,

    #[error("assessment response is an expired or timed-out page")]
    ExpiredPage,

    #[error("the teaching-evaluation questionnaire window is not open")]
    NotOpen,

    #[error("assessment page did not contain the questionnaire table")]
    MissingTable,

    #[error("the questionnaire list had no rows")]
    EmptyList,

    #[error("assessment row {row} has an unrecognized layout")]
    UnrecognizedRow { row: usize },

    #[error("assessment row {row} is missing its course name")]
    MissingName { row: usize },

    #[error("assessment row {row} has no readable form action")]
    MissingAction { row: usize },

    #[error("assessment row {row} named a form route outside the service mapping")]
    InvalidRoute { row: usize },

    #[error("assessment response exceeded the bounded element limit")]
    TooLarge,
}

impl AssessmentParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired INFO session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }

    /// Returns true when the service explicitly reported a closed window.
    pub fn is_not_open(&self) -> bool {
        matches!(self, Self::NotOpen)
    }
}

impl From<ScanError> for AssessmentParseError {
    fn from(_: ScanError) -> Self {
        Self::TooLarge
    }
}

/// Adapter failures are body-free so an HTML login page cannot leak through a
/// debug or bridge DTO.
#[derive(Debug, Error)]
pub enum AssessmentAdapterError {
    #[error("assessment base URL is invalid")]
    InvalidBaseUrl,

    #[error("assessment transport failed")]
    Transport(#[source] TransportError),

    #[error("assessment request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("assessment response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("assessment response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("assessment response is not an HTML document")]
    UnexpectedContentType,

    #[error("assessment INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("assessment response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("the teaching-evaluation questionnaire window is not open")]
    NotOpen,

    #[error("assessment response could not be parsed: {0}")]
    Parse(#[source] AssessmentParseError),
}

impl AssessmentAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "assessment_config",
            Self::Transport(_) => "assessment_network",
            Self::HttpStatus { .. } => "assessment_http",
            Self::UnexpectedOrigin => "assessment_origin",
            Self::UnexpectedPath => "assessment_path",
            Self::UnexpectedContentType => "assessment_content_type",
            Self::SessionExpired => "assessment_auth_required",
            Self::UnexpectedDeployment => "assessment_template",
            Self::NotOpen => "assessment_not_open",
            Self::Parse(AssessmentParseError::EmptyBody) => "assessment_body_empty",
            Self::Parse(AssessmentParseError::LoginPage) => "assessment_auth_required",
            Self::Parse(AssessmentParseError::ExpiredPage) => "assessment_auth_required",
            Self::Parse(AssessmentParseError::NotOpen) => "assessment_not_open",
            Self::Parse(AssessmentParseError::MissingTable) => "assessment_table_missing",
            Self::Parse(AssessmentParseError::EmptyList) => "assessment_list_empty",
            Self::Parse(AssessmentParseError::UnrecognizedRow { .. }) => "assessment_row_shape",
            Self::Parse(AssessmentParseError::MissingName { .. }) => "assessment_row_name",
            Self::Parse(AssessmentParseError::MissingAction { .. }) => "assessment_row_action",
            Self::Parse(AssessmentParseError::InvalidRoute { .. }) => "assessment_row_route",
            Self::Parse(AssessmentParseError::TooLarge) => "assessment_size",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(AssessmentParseError::LoginPage)
                | Self::Parse(AssessmentParseError::ExpiredPage)
        )
    }

    /// True when the service explicitly answered that the window is closed.
    pub fn is_not_open(&self) -> bool {
        matches!(
            self,
            Self::NotOpen | Self::Parse(AssessmentParseError::NotOpen)
        )
    }
}

/// Configuration for a read-only evaluation adapter.
#[derive(Clone)]
pub struct AssessmentAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl AssessmentAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, AssessmentAdapterError> {
        Self::with_user_agent_and_timeout(
            base_url,
            "THYou/teaching-evaluation",
            Duration::from_secs(30),
        )
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, AssessmentAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AssessmentAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| AssessmentAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(AssessmentAdapterError::InvalidBaseUrl);
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

    fn transport(&self) -> Result<CampusHttpTransport, AssessmentAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(AssessmentAdapterError::Transport)
    }
}

impl fmt::Debug for AssessmentAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one evaluation response.  It is opaque: no
/// Cookie, URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentBusinessProof {
    adapter_binding: u64,
    generation: u64,
    operation: AssessmentOperation,
}

impl fmt::Debug for AssessmentBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated list together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct AssessmentRead {
    pub value: AssessmentList,
    pub proof: AssessmentBusinessProof,
}

/// The form routes named by the most recent successful list read.
#[derive(Default)]
struct AssessmentRoutes {
    generation: u64,
    paths: Vec<String>,
}

/// Read-only evaluation client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct AssessmentAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: AssessmentProfile,
    binding: u64,
    routes: Mutex<AssessmentRoutes>,
}

impl AssessmentAdapter {
    pub fn new(config: AssessmentAdapterConfig) -> Result<Self, AssessmentAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, AssessmentAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: AssessmentProfile::standard(),
            binding: NEXT_ASSESSMENT_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            routes: Mutex::new(AssessmentRoutes::default()),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> AssessmentProfile {
        self.profile
    }

    /// Reads the questionnaire list, replacing the routes retained by the
    /// previous read.
    pub async fn read_list(&self) -> Result<AssessmentList, AssessmentAdapterError> {
        self.read_list_with_proof().await.map(|read| read.value)
    }

    pub async fn read_list_with_proof(&self) -> Result<AssessmentRead, AssessmentAdapterError> {
        let plan = self.profile.list_request();
        let body = self.execute(&plan).await?;
        let parsed =
            parse_assessment_list_html(&body).map_err(AssessmentAdapter::map_parse_error)?;
        // The generation advances only after the page parsed, so a rejected
        // read leaves the previous routes resolvable instead of pointing them
        // at nothing.
        let generation = self
            .routes
            .lock()
            .map(|routes| routes.generation)
            .unwrap_or_default()
            .wrapping_add(1);
        let items = parsed
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| AssessmentItem {
                name: row.name.clone(),
                evaluated: row.evaluated,
                reference: AssessmentRef::new(self.binding, generation, index as u32),
            })
            .collect();
        if let Ok(mut routes) = self.routes.lock() {
            *routes = AssessmentRoutes {
                generation,
                paths: parsed.rows.iter().map(|row| row.route.clone()).collect(),
            };
        }
        Ok(AssessmentRead {
            value: AssessmentList { items },
            proof: AssessmentBusinessProof {
                adapter_binding: self.binding,
                generation,
                operation: AssessmentOperation::ReadList,
            },
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &AssessmentBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    /// Resolves a row reference to the form route it named, if the reference
    /// still belongs to the list this adapter most recently read.
    pub fn form_path(&self, reference: &AssessmentRef) -> Option<String> {
        if reference.adapter_binding != self.binding {
            return None;
        }
        let routes = self.routes.lock().ok()?;
        if routes.generation != reference.generation {
            return None;
        }
        routes
            .paths
            .get(usize::try_from(reference.index).ok()?)
            .cloned()
    }

    /// The current route generation.  It advances once per accepted read.
    pub fn route_generation(&self) -> u64 {
        self.routes
            .lock()
            .map(|routes| routes.generation)
            .unwrap_or_default()
    }

    async fn execute(
        &self,
        plan: &AssessmentRequestPlan,
    ) -> Result<String, AssessmentAdapterError> {
        let endpoint = self.endpoint(plan)?;
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| AssessmentAdapterError::Transport(TransportError::Request(error)))?;
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
            .map_err(|_| AssessmentAdapterError::UnexpectedOrigin)?;
        let location_target = location
            .as_deref()
            .map(|value| resolve_location(&final_url, value))
            .transpose()?;
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || location_target.as_ref().is_some_and(|target| {
                same_origin(&self.base_url, target) && looks_like_login_url(target)
            })
        {
            return Err(AssessmentAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(AssessmentAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(AssessmentAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| AssessmentAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_HTML_BYTES {
            return Err(AssessmentAdapterError::UnexpectedDeployment);
        }
        // The service's own session-expiry banner and its closed-window answer
        // both arrive as a 200 body, so they are classified here, before the
        // HTML scanner can turn them into a "no table" failure.
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(AssessmentAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        if body.contains(ASSESSMENT_NOT_OPEN_MARKER) {
            return Err(AssessmentAdapterError::NotOpen);
        }
        if status != StatusCode::OK {
            return Err(AssessmentAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.query().map(str::to_owned) != expected_query
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(AssessmentAdapterError::UnexpectedPath);
        }
        if !is_html_content_type(content_type.as_deref()) {
            return Err(AssessmentAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    fn endpoint(&self, plan: &AssessmentRequestPlan) -> Result<Url, AssessmentAdapterError> {
        if !valid_relative_path(plan.path) || plan.query.chars().any(char::is_control) {
            return Err(AssessmentAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{}", plan.path));
        if plan.query.is_empty() {
            endpoint.set_query(None);
        } else {
            endpoint.set_query(Some(plan.query));
        }
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: AssessmentParseError) -> AssessmentAdapterError {
        match error {
            AssessmentParseError::LoginPage | AssessmentParseError::ExpiredPage => {
                AssessmentAdapterError::SessionExpired
            }
            AssessmentParseError::NotOpen => AssessmentAdapterError::NotOpen,
            other => AssessmentAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for AssessmentAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("route_generation", &self.route_generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Runs the session-level guards that must precede any structural parse.
fn guard_page(html: &str) -> Result<&str, AssessmentParseError> {
    let trimmed = html.strip_prefix('\u{feff}').unwrap_or(html).trim();
    if trimmed.is_empty() {
        return Err(AssessmentParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(AssessmentParseError::LoginPage),
        PageClass::Expired => return Err(AssessmentParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    if trimmed.contains(ASSESSMENT_NOT_OPEN_MARKER) {
        return Err(AssessmentParseError::NotOpen);
    }
    Ok(trimmed)
}

/// Parses one questionnaire list page into its rows.
///
/// Every accepted row must carry the inline `Body('…')` action, and the route
/// it names must be a plain path inside this service.  That anchor is what
/// makes a positional read safe: a row whose columns moved cannot satisfy it,
/// so the page is reported as unrecognized instead of being read with one
/// column's value standing in for another's.
///
/// A closed questionnaire window and an empty list are both errors, not an
/// empty result: see the module documentation.
pub fn parse_assessment_list_html(html: &str) -> Result<AssessmentListRows, AssessmentParseError> {
    let trimmed = guard_page(html)?;
    let bodies = campus_html::scan(trimmed, "tbody")?;
    if bodies.is_empty() {
        return Err(AssessmentParseError::MissingTable);
    }
    let mut rows = Vec::new();
    for body in &bodies {
        for row in campus_html::direct_children(body.inner(), "tr")? {
            if rows.len() >= MAX_ITEMS {
                return Err(AssessmentParseError::TooLarge);
            }
            let index = rows.len();
            let cells = campus_html::direct_children(row.inner(), "td")?;
            if cells.len() < MIN_ROW_CELLS {
                return Err(AssessmentParseError::UnrecognizedRow { row: index });
            }
            let name = cells[NAME_CELL].text();
            if name.is_empty() {
                return Err(AssessmentParseError::MissingName { row: index });
            }
            let route = action_route(&cells[ACTION_CELL])
                .ok_or(AssessmentParseError::MissingAction { row: index })?;
            if route_parts(&route).is_none() {
                return Err(AssessmentParseError::InvalidRoute { row: index });
            }
            rows.push(AssessmentRow {
                name: bounded_name(&name),
                evaluated: cells[EVALUATED_CELL].text() == EVALUATED_TEXT,
                route,
            });
        }
    }
    if rows.is_empty() {
        return Err(AssessmentParseError::EmptyList);
    }
    Ok(AssessmentListRows { rows })
}

/// Extracts the form route named by a row's action cell.
///
/// The legacy page writes an inline `javascript:…Body('/path…') })` call.  The
/// route is taken only when both delimiters are present and ordered, so a
/// reworded action is a missing action rather than a sliced-up fragment.
fn action_route(cell: &RawElement) -> Option<String> {
    let source = cell.inner();
    let start = source.find(ACTION_CALL_OPEN)? + ACTION_CALL_OPEN.len();
    let end = source[start..].find(ACTION_CALL_CLOSE)? + start;
    if end <= start {
        return None;
    }
    let route = source[start..end].trim();
    if route.is_empty() || route.len() > MAX_ROUTE_CHARS {
        return None;
    }
    Some(route.to_owned())
}

/// Keeps one course name inside the bounded public shape.
fn bounded_name(value: &str) -> String {
    value.chars().take(MAX_NAME_CHARS).collect()
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, AssessmentAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(AssessmentAdapterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = opaque_mapping_root(path).unwrap_or_else(|| {
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

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, AssessmentAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(AssessmentAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| AssessmentAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(AssessmentAdapterError::UnexpectedOrigin);
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
        && path.len() <= MAX_ROUTE_CHARS
        && !path.contains("://")
        && !path.contains(['?', '#', '\\'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

/// Splits one row's named route into its path and its query, both validated.
///
/// The legacy action is a server-rendered relative URL carrying a query, so a
/// route's query is accepted here — unlike the request plans, whose query is a
/// module constant.  Splitting it in one place is what keeps the parts from
/// being validated under looser rules than the plan they end up in.
fn route_parts(route: &str) -> Option<(&str, Option<&str>)> {
    if !route.starts_with('/')
        || route.len() > MAX_ROUTE_CHARS
        || route.contains("://")
        || route.contains(['#', '\\'])
        || route.chars().any(char::is_control)
    {
        return None;
    }
    let (path, query) = match route.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (route, None),
    };
    if !valid_relative_path(path) {
        return None;
    }
    if let Some(query) = query {
        if query.is_empty()
            || query.chars().any(char::is_control)
            || invalid_percent_encoding(query)
            || path_contains_encoded_escape(query)
        {
            return None;
        }
    }
    Some((path, query))
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
