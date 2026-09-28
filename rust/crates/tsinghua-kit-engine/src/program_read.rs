//! Read-only degree-program completion and full program plans.
//!
//! Both operations are legacy JSP reports served by the registrar host, so
//! this module owns the HTML boundary: request construction, route validation,
//! session-expiry classification, and a bounded parser.  It reuses the
//! already-allowlisted registrar roaming selector, so no new WebVPN mapping is
//! introduced and every request still travels through the shared
//! [`CampusHttpTransport`] and its request gate.
//!
//! Two rules shape the parser:
//!
//! * The summary block is read by its own Chinese labels rather than by byte
//!   offsets, so a reordered page cannot silently move a number.
//! * A course row whose layout is not one of the observed shapes is an error.
//!   The public reference has to guess a row's level from a raw DOM child
//!   count and then throws when it sees an unknown one; dropping such a row
//!   here would look like a smaller course list instead of a changed page.
//!
//! This module reimplements the contract from the public reference behavior.
//! It does not copy source, fixtures, or assets.

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

use crate::campus_html::{self, PageClass, RawElement, ScanError};
use crate::transport::{CampusHttpTransport, TransportError};

/// The registrar roaming selector shared with the classroom and calendar
/// reports.  It is a route-profile input, not a URL and not a credential.
pub const PROGRAM_WEBVPN_TARGET: &str = "287C0C6D90ABB364CD5FDF1495199962";

/// The completion report behind the selector above.
pub const PROGRAM_COMPLETION_PATH: &str = "/jhBks.by_fascjgmxb_gr.do";
pub const PROGRAM_COMPLETION_QUERY: &str = "m=queryFaScjgmx_gr&xsViewFlag=pyfa&pathContent=%E5%9F%B9%E5%85%BB%E6%96%B9%E6%A1%88%E5%AE%8C%E6%88%90%E6%83%85%E5%86%B5";

/// The plan index that names the plan identifier used by the full report.
pub const PROGRAM_LIST_PATH: &str = "/jhBks.vjhBksPyfabBs.do";
pub const PROGRAM_LIST_QUERY: &str = "m=grPyfabks&theRole=&theModule=pyfa&pathContent=%E4%B8%AA%E4%BA%BA%E5%9F%B9%E5%85%BB%E6%96%B9%E6%A1%88";

/// The full plan report.  The identifier is a service-assigned integer, never
/// a caller-supplied free-form value.
pub const PROGRAM_FULL_PATH: &str = "/jhBks.vjhBksPyfakcbBs.do";
pub const PROGRAM_FULL_QUERY_PREFIX: &str = "m=index2&theModule=pyfa&p_fajhh=";

const MAX_HTML_BYTES: usize = 8 * 1024 * 1024;
const MAX_COURSE_SETS: usize = 512;
const MAX_COURSES: usize = 8_192;
const MAX_TEXT_CHARS: usize = 512;

/// The anchors that bracket the plan identifier on the plan index page.
///
/// The service writes the identifier into a URL inside an attribute, so the
/// separators are entity-escaped in the raw markup and survive neither tag
/// stripping nor entity decoding.  The identifier is therefore anchored by the
/// frame name and the parameter name instead of by a full escaped substring.
const PLAN_ID_FRAME_MARKER: &str = "pyfakzFrame";
const PLAN_ID_PARAMETER: &str = "fajhh=";
/// The completion-flag column that terminates a course-set header row.
const COMPLETION_FLAG_YES: &str = "是";
/// The column label that identifies a completion table's header row.
const COMPLETION_HEADER_LABEL: &str = "课号";

/// A completion table always opens with a labeled header.  Requiring it keeps
/// a changed page from being read as a table of courses that all happen to be
/// missing their first row.
fn is_completion_header(cells: &[RawElement]) -> bool {
    cells
        .iter()
        .any(|cell| cell.text() == COMPLETION_HEADER_LABEL)
}

static NEXT_PROGRAM_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This profile only issues GETs.  Keeping the method in the plan makes a
/// write route impossible to smuggle into the read-only adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramMethod {
    Get,
}

/// The three observed degree-program operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramOperation {
    ReadCompletion,
    ReadPlanId,
    ReadFullPlan,
}

/// A program request requires an already established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan.  It contains no Cookie, ticket, account
/// identifier, or absolute WebVPN mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramRequestPlan {
    pub operation: ProgramOperation,
    pub method: ProgramMethod,
    pub path: &'static str,
    pub query: String,
    pub webvpn_target: &'static str,
    pub session_prerequisite: ProgramSessionPrerequisite,
}

impl ProgramRequestPlan {
    /// Returns the serialized query without the leading `?`.
    pub fn query_string(&self) -> &str {
        &self.query
    }
}

/// Fixed degree-program route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgramProfile;

impl ProgramProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        PROGRAM_WEBVPN_TARGET
    }

    pub fn completion_request(self) -> ProgramRequestPlan {
        ProgramRequestPlan {
            operation: ProgramOperation::ReadCompletion,
            method: ProgramMethod::Get,
            path: PROGRAM_COMPLETION_PATH,
            query: PROGRAM_COMPLETION_QUERY.to_owned(),
            webvpn_target: PROGRAM_WEBVPN_TARGET,
            session_prerequisite: ProgramSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    pub fn plan_id_request(self) -> ProgramRequestPlan {
        ProgramRequestPlan {
            operation: ProgramOperation::ReadPlanId,
            method: ProgramMethod::Get,
            path: PROGRAM_LIST_PATH,
            query: PROGRAM_LIST_QUERY.to_owned(),
            webvpn_target: PROGRAM_WEBVPN_TARGET,
            session_prerequisite: ProgramSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    /// Builds the full-plan request for a service-assigned plan identifier.
    pub fn full_plan_request(self, plan_id: u64) -> Result<ProgramRequestPlan, ProgramPlanIdError> {
        if plan_id == 0 || plan_id > 9_999_999_999 {
            return Err(ProgramPlanIdError::OutOfRange);
        }
        Ok(ProgramRequestPlan {
            operation: ProgramOperation::ReadFullPlan,
            method: ProgramMethod::Get,
            path: PROGRAM_FULL_PATH,
            query: format!("{PROGRAM_FULL_QUERY_PREFIX}{plan_id}"),
            webvpn_target: PROGRAM_WEBVPN_TARGET,
            session_prerequisite: ProgramSessionPrerequisite::ExistingInfoWebVpnSession,
        })
    }
}

/// A plan identifier that is not a plausible service-assigned value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProgramPlanIdError {
    #[error("plan identifier is outside the supported range")]
    OutOfRange,
}

/// The completion state of one course row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CourseState {
    /// The course has a grade or an explicit completion mark.
    Completed,
    /// The course is currently elected but not yet graded.
    Elected,
    /// The course is not finished, including a withdrawn `W`/`F`/`I` mark.
    NotCompleted,
}

/// One course row from the completion report.
#[derive(Debug, Clone, PartialEq)]
pub struct CourseCompletion {
    pub course_id: String,
    pub name: String,
    pub credit: f64,
    /// Absent for courses that cannot carry a grade point.
    pub point: Option<f64>,
    /// Absent for unelected or unfinished courses.
    pub grade: Option<String>,
    pub state: CourseState,
}

/// The course-attribute group a course set belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CourseSetKind {
    Compulsory,
    Restricted,
    Elective,
    /// Courses completed outside the plan, reported as their own group.
    Excluded,
}

/// One course set with its own credit and course-count requirements.
#[derive(Debug, Clone, PartialEq)]
pub struct CourseSetCompletion {
    pub name: String,
    pub kind: CourseSetKind,
    pub required_credit: Option<f64>,
    pub completed_credit: Option<f64>,
    pub required_course_count: Option<u32>,
    pub completed_course_count: Option<u32>,
    pub full_completed: bool,
    pub courses: Vec<CourseCompletion>,
}

/// The plan-wide completion summary and its course sets.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramCompletion {
    /// The credits completed inside the plan.
    pub completed_credit: f64,
    pub compulsory_credit: f64,
    pub restricted_credit: f64,
    pub elective_credit: f64,
    /// The service-reported duplicated course names.
    pub duplicated_courses: Vec<String>,
    /// Present only when the report carries the out-of-plan summary.
    pub excluded_credit: Option<f64>,
    pub course_sets: Vec<CourseSetCompletion>,
}

/// One course row from the full (planned, not yet completed) report.
#[derive(Debug, Clone, PartialEq)]
pub struct CourseFull {
    pub course_id: String,
    pub name: String,
    pub credit: f64,
}

/// One course set from the full plan report.
#[derive(Debug, Clone, PartialEq)]
pub struct CourseSetFull {
    pub name: String,
    pub kind: CourseSetKind,
    pub courses: Vec<CourseFull>,
}

/// The planned course sets of the student's own program.
#[derive(Debug, Clone, PartialEq)]
pub struct FullProgram {
    pub course_sets: Vec<CourseSetFull>,
}

/// Parser failures retain only stable field names and positions.  They never
/// keep untrusted HTML, Cookie values, tickets, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProgramParseError {
    #[error("program response body is empty")]
    EmptyBody,

    #[error("program response is an HTML login page")]
    LoginPage,

    #[error("program response is an expired or timed-out page")]
    ExpiredPage,

    #[error("program response is not a recognized HTML page")]
    UnexpectedHtml,

    #[error("program page did not contain the {marker} marker")]
    MissingMarker { marker: &'static str },

    #[error("program page contained an ambiguous {marker} marker")]
    DuplicateMarker { marker: &'static str },

    #[error("program page has an invalid {field} value")]
    InvalidField { field: &'static str },

    #[error("program plan identifier is missing from the plan index")]
    MissingPlanId,

    #[error("program completion table is missing")]
    MissingCompletionTable,

    #[error("program completion table has no labeled header row")]
    MissingCompletionHeader,

    #[error("program page contains more than one program container")]
    AmbiguousProgramContainer,

    #[error("program completion table has no course rows")]
    EmptyCompletionTable,

    #[error("program course row {row} has an unrecognized layout")]
    UnrecognizedCourseRow { row: usize },

    #[error("program course row {row} is missing its {field} value")]
    MissingCourseField { row: usize, field: &'static str },

    #[error("program course row {row} has an invalid {field} value")]
    InvalidCourseField { row: usize, field: &'static str },

    #[error("program course set {row} has an unrecognized attribute kind")]
    UnrecognizedCourseSet { row: usize },

    #[error("program response exceeded the bounded element limit")]
    TooLarge,
}

impl From<ScanError> for ProgramParseError {
    fn from(_: ScanError) -> Self {
        Self::TooLarge
    }
}

/// Adapter failures are body-free so an HTML login page cannot leak through a
/// debug or bridge DTO.
#[derive(Debug, Error)]
pub enum ProgramAdapterError {
    #[error("program WebVPN base URL is invalid")]
    InvalidBaseUrl,

    #[error("program transport failed")]
    Transport(#[source] TransportError),

    #[error("program request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("program response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("program response ended outside the configured WebVPN mapping")]
    UnexpectedPath,

    #[error("program response is not an HTML document")]
    UnexpectedContentType,

    #[error("program WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("program page template is not recognized")]
    UnexpectedDeployment,

    #[error("program request plan is invalid")]
    InvalidPlan,

    #[error("program response could not be parsed: {0}")]
    Parse(#[source] ProgramParseError),
}

impl ProgramAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "program_config",
            Self::Transport(_) => "program_network",
            Self::HttpStatus { .. } => "program_http",
            Self::UnexpectedOrigin => "program_origin",
            Self::UnexpectedPath => "program_path",
            Self::UnexpectedContentType => "program_content_type",
            Self::SessionExpired => "program_auth_required",
            Self::UnexpectedDeployment => "program_template",
            Self::InvalidPlan => "program_plan",
            Self::Parse(ProgramParseError::EmptyBody) => "program_body_empty",
            Self::Parse(ProgramParseError::LoginPage) => "program_auth_required",
            Self::Parse(ProgramParseError::ExpiredPage) => "program_auth_required",
            Self::Parse(ProgramParseError::UnexpectedHtml) => "program_template",
            Self::Parse(ProgramParseError::MissingMarker { .. }) => "program_marker_missing",
            Self::Parse(ProgramParseError::DuplicateMarker { .. }) => "program_marker_duplicate",
            Self::Parse(ProgramParseError::InvalidField { .. }) => "program_field_invalid",
            Self::Parse(ProgramParseError::MissingPlanId) => "program_plan_id_missing",
            Self::Parse(ProgramParseError::MissingCompletionTable) => "program_table_missing",
            Self::Parse(ProgramParseError::MissingCompletionHeader) => "program_header_missing",
            Self::Parse(ProgramParseError::AmbiguousProgramContainer) => {
                "program_container_ambiguous"
            }
            Self::Parse(ProgramParseError::EmptyCompletionTable) => "program_table_empty",
            Self::Parse(ProgramParseError::UnrecognizedCourseRow { .. }) => "program_row_shape",
            Self::Parse(ProgramParseError::MissingCourseField { .. }) => {
                "program_row_field_missing"
            }
            Self::Parse(ProgramParseError::InvalidCourseField { .. }) => {
                "program_row_field_invalid"
            }
            Self::Parse(ProgramParseError::UnrecognizedCourseSet { .. }) => "program_set_kind",
            Self::Parse(ProgramParseError::TooLarge) => "program_size",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(ProgramParseError::LoginPage)
                | Self::Parse(ProgramParseError::ExpiredPage)
        )
    }
}

/// Configuration for a read-only degree-program adapter.
#[derive(Clone)]
pub struct ProgramAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl ProgramAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, ProgramAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/degree-program", Duration::from_secs(30))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, ProgramAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, ProgramAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| ProgramAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(ProgramAdapterError::InvalidBaseUrl);
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

    fn transport(&self) -> Result<CampusHttpTransport, ProgramAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(ProgramAdapterError::Transport)
    }
}

impl fmt::Debug for ProgramAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProgramAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one degree-program business response.  It is
/// intentionally opaque: the proof carries no Cookie, URL, account value, or
/// response body.
#[derive(Clone, PartialEq, Eq)]
pub struct ProgramBusinessProof {
    adapter_binding: u64,
    operation: ProgramOperation,
}

impl fmt::Debug for ProgramBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProgramBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated completion report together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramCompletionRead {
    pub value: ProgramCompletion,
    pub proof: ProgramBusinessProof,
}

/// A validated full plan together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct FullProgramRead {
    pub value: FullProgram,
    pub proof: ProgramBusinessProof,
}

/// Read-only degree-program client.  `try_with_transport` is the normal
/// runtime entry point: the transport must be the one that already carries the
/// identity/INFO/WebVPN cookie jar.
pub struct ProgramAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: ProgramProfile,
    binding: u64,
}

impl ProgramAdapter {
    pub fn new(config: ProgramAdapterConfig) -> Result<Self, ProgramAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, ProgramAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: ProgramProfile::standard(),
            binding: NEXT_PROGRAM_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> ProgramProfile {
        self.profile
    }

    /// Reads the plan-wide completion report.
    pub async fn read_completion(&self) -> Result<ProgramCompletion, ProgramAdapterError> {
        self.read_completion_with_proof()
            .await
            .map(|read| read.value)
    }

    pub async fn read_completion_with_proof(
        &self,
    ) -> Result<ProgramCompletionRead, ProgramAdapterError> {
        let plan = self.profile.completion_request();
        let body = self.execute(&plan).await?;
        let value = parse_program_completion_html(&body).map_err(Self::map_parse_error)?;
        Ok(ProgramCompletionRead {
            value,
            proof: self.business_proof(ProgramOperation::ReadCompletion),
        })
    }

    /// Reads the service-assigned plan identifier needed by the full report.
    pub async fn read_plan_id(&self) -> Result<u64, ProgramAdapterError> {
        let plan = self.profile.plan_id_request();
        let body = self.execute(&plan).await?;
        parse_program_plan_id_html(&body).map_err(Self::map_parse_error)
    }

    /// Reads the planned course sets for one plan identifier.
    pub async fn read_full_plan(&self, plan_id: u64) -> Result<FullProgram, ProgramAdapterError> {
        self.read_full_plan_with_proof(plan_id)
            .await
            .map(|read| read.value)
    }

    pub async fn read_full_plan_with_proof(
        &self,
        plan_id: u64,
    ) -> Result<FullProgramRead, ProgramAdapterError> {
        let plan = self
            .profile
            .full_plan_request(plan_id)
            .map_err(|_| ProgramAdapterError::InvalidPlan)?;
        let body = self.execute(&plan).await?;
        let value = parse_full_program_html(&body).map_err(Self::map_parse_error)?;
        Ok(FullProgramRead {
            value,
            proof: self.business_proof(ProgramOperation::ReadFullPlan),
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &ProgramBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    fn business_proof(&self, operation: ProgramOperation) -> ProgramBusinessProof {
        ProgramBusinessProof {
            adapter_binding: self.binding,
            operation,
        }
    }

    async fn execute(&self, plan: &ProgramRequestPlan) -> Result<String, ProgramAdapterError> {
        let endpoint = self.endpoint(plan)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| ProgramAdapterError::Transport(TransportError::Request(error)))?;
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
            .map_err(|_| ProgramAdapterError::UnexpectedOrigin)?;
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
            return Err(ProgramAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(ProgramAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(ProgramAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| ProgramAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_HTML_BYTES {
            return Err(ProgramAdapterError::UnexpectedDeployment);
        }
        let page_class = campus_html::classify_page(&body);
        if matches!(page_class, PageClass::Login | PageClass::Expired) {
            return Err(ProgramAdapterError::SessionExpired);
        }
        if status != StatusCode::OK {
            return Err(ProgramAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(ProgramAdapterError::UnexpectedPath);
        }
        if !is_html_content_type(content_type.as_deref()) {
            return Err(ProgramAdapterError::UnexpectedContentType);
        }
        if !matches!(page_class, PageClass::Unknown) {
            return Err(ProgramAdapterError::UnexpectedDeployment);
        }
        Ok(body)
    }

    fn endpoint(&self, plan: &ProgramRequestPlan) -> Result<Url, ProgramAdapterError> {
        if !valid_relative_path(plan.path) || plan.query.chars().any(char::is_control) {
            return Err(ProgramAdapterError::InvalidPlan);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{}", plan.path));
        if plan.query.is_empty() {
            endpoint.set_query(None);
        } else {
            endpoint.set_query(Some(&plan.query));
        }
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: ProgramParseError) -> ProgramAdapterError {
        match error {
            ProgramParseError::LoginPage | ProgramParseError::ExpiredPage => {
                ProgramAdapterError::SessionExpired
            }
            ProgramParseError::UnexpectedHtml => ProgramAdapterError::UnexpectedDeployment,
            other => ProgramAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for ProgramAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProgramAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Parses the plan index page and returns the service-assigned plan
/// identifier.
///
/// The identifier is the only value taken from this page, and it is read from
/// the raw markup rather than from the rendered text: the service emits it in
/// a URL inside an attribute, where the ampersand separators are entity-escaped
/// and therefore survive neither tag stripping nor entity decoding.  A missing
/// or malformed value is an error: defaulting it would request a different plan
/// than the one the service selected for this account.
pub fn parse_program_plan_id_html(html: &str) -> Result<u64, ProgramParseError> {
    guard_page(html)?;
    // The frame name and the parameter name may be separated by an escaped or
    // a literal ampersand, so each is located on its own and the identifier is
    // read strictly between them.
    let frame = html
        .find(PLAN_ID_FRAME_MARKER)
        .ok_or(ProgramParseError::MissingPlanId)?;
    let anchor = frame + PLAN_ID_FRAME_MARKER.len();
    let remainder = &html[anchor..];
    let parameter = remainder
        .find(PLAN_ID_PARAMETER)
        .ok_or(ProgramParseError::MissingPlanId)?;
    // Anything other than the separator belongs to a different parameter, so
    // the identifier is only accepted when the two anchors are adjacent.
    let separator = &remainder[..parameter];
    if !matches!(separator, "&" | "&amp;") {
        return Err(ProgramParseError::MissingPlanId);
    }
    let digits: String = remainder[parameter + PLAN_ID_PARAMETER.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return Err(ProgramParseError::MissingPlanId);
    }
    digits
        .parse::<u64>()
        .map_err(|_| ProgramParseError::InvalidField { field: "plan_id" })
}

/// Parses the completion report: the labeled summary block plus every course
/// set row.
pub fn parse_program_completion_html(html: &str) -> Result<ProgramCompletion, ProgramParseError> {
    let text = guarded_text(html)?;

    let summary_anchor = text
        .find("方案内实际完成")
        .ok_or(ProgramParseError::MissingMarker {
            marker: "方案内实际完成",
        })?;
    let summary = &text[summary_anchor..];
    let completed_credit = number_after(summary, "总学分：")?;
    let compulsory_credit = number_after(summary, "其中必修完成总学分：")?;
    let restricted_credit = number_after(summary, "限选完成总学分：")?;
    let elective_credit = number_after(summary, "任选（方案内）完成总学分：")?;
    let duplicated_courses = duplicated_courses(summary)?;
    let excluded_credit = number_after(text.as_str(), "培养方案外课程完成总学分：").ok();

    // The report renders every course row inside `.table-striped` tables: the
    // in-plan table first, then a second table holding only the out-of-plan
    // group.  The rows are therefore read as one flat sequence, exactly as the
    // legacy service renders them, and the out-of-plan group is recognized by
    // its own row shape rather than by which table it sits in.
    let tables = campus_html::scan_with_class(html, "table", "table-striped")?;
    if tables.is_empty() {
        return Err(ProgramParseError::MissingCompletionTable);
    }
    // The completion table proper is the one carrying a labeled header.  Any
    // other `.table-striped` table contributes out-of-plan rows; a table whose
    // leading row happens to repeat the header label is skipped so the label
    // cannot be mistaken for a course.
    let mut rows = Vec::new();
    let mut saw_labeled_header = false;
    for table in tables {
        let mut children = campus_html::direct_children(table.inner(), "tr")?.into_iter();
        if let Some(first) = children.next() {
            let cells = campus_html::direct_children(first.inner(), "td")?;
            if is_completion_header(&cells) {
                saw_labeled_header = true;
            } else {
                rows.push(first);
            }
        }
        rows.extend(children);
    }
    if !saw_labeled_header {
        return Err(ProgramParseError::MissingCompletionHeader);
    }
    if rows.is_empty() {
        return Err(ProgramParseError::EmptyCompletionTable);
    }

    let mut course_sets: Vec<CourseSetCompletion> = Vec::new();
    let mut course_count = 0usize;
    let mut excluded: Option<CourseSetCompletion> = None;
    for (index, row) in rows.iter().enumerate() {
        let cells = campus_html::direct_children(row.inner(), "td")?;
        match classify_row(&cells, index)? {
            RowKind::AttributeSection => {
                let name = cell_text(&cells, SECTION_NAME_CELL);
                let kind = course_set_kind(&cell_text(&cells, SECTION_ATTRIBUTE_CELL))
                    .ok_or(ProgramParseError::UnrecognizedCourseSet { row: index })?;
                if course_sets.len() >= MAX_COURSE_SETS {
                    return Err(ProgramParseError::TooLarge);
                }
                let mut set = requirement_header(&cells, SECTION_COMPLETION_CELL, name, kind)?;
                set.courses
                    .push(parse_course_row(&cells, SECTION_FIRST_COURSE_CELL, index)?);
                course_count += 1;
                course_sets.push(set);
            }
            RowKind::GroupSection => {
                // A sub-group inherits the attribute of the section it is
                // nested under, so it is only valid after one has been seen.
                let kind = course_sets
                    .last()
                    .map(|set| set.kind)
                    .ok_or(ProgramParseError::UnrecognizedCourseSet { row: index })?;
                if course_sets.len() >= MAX_COURSE_SETS {
                    return Err(ProgramParseError::TooLarge);
                }
                let mut set = requirement_header(
                    &cells,
                    GROUP_COMPLETION_CELL,
                    cell_text(&cells, GROUP_NAME_CELL),
                    kind,
                )?;
                set.courses
                    .push(parse_course_row(&cells, GROUP_FIRST_COURSE_CELL, index)?);
                course_count += 1;
                course_sets.push(set);
            }
            RowKind::Course => {
                let Some(current) = course_sets.last_mut() else {
                    return Err(ProgramParseError::UnrecognizedCourseRow { row: index });
                };
                let course = parse_course_row(&cells, 0, index)?;
                if course_count >= MAX_COURSES {
                    return Err(ProgramParseError::TooLarge);
                }
                course_count += 1;
                current.courses.push(course);
            }
            RowKind::ExcludedGroup => {
                if excluded.is_some() {
                    return Err(ProgramParseError::AmbiguousProgramContainer);
                }
                excluded = Some(excluded_group(excluded_credit));
            }
            RowKind::ExcludedCourse => {
                // The page may or may not render the group marker before its
                // courses, so the group is materialized on whichever comes
                // first rather than requiring both.
                let group = excluded.get_or_insert_with(|| excluded_group(excluded_credit));
                if course_count >= MAX_COURSES {
                    return Err(ProgramParseError::TooLarge);
                }
                course_count += 1;
                group
                    .courses
                    .push(parse_excluded_course_row(&cells, index)?);
            }
        }
    }

    // An out-of-plan total with no course rows underneath would misreport the
    // group as complete, so the empty case is reported instead.
    if let Some(group) = excluded {
        if group.courses.is_empty() {
            return Err(ProgramParseError::EmptyCompletionTable);
        }
        course_sets.push(group);
    }

    Ok(ProgramCompletion {
        completed_credit,
        compulsory_credit,
        restricted_credit,
        elective_credit,
        duplicated_courses,
        excluded_credit,
        course_sets,
    })
}

/// Parses the full plan report into its planned course sets.
pub fn parse_full_program_html(html: &str) -> Result<FullProgram, ProgramParseError> {
    guarded_text(html)?;

    let containers = campus_html::scan_with_id(html, "div", "content_1")?;
    let scope = match containers.len() {
        0 => html,
        1 => containers[0].inner(),
        _ => return Err(ProgramParseError::AmbiguousProgramContainer),
    };
    let rows = campus_html::scan_with_class(scope, "tr", "trr2")?;

    let mut course_sets: Vec<CourseSetFull> = Vec::new();
    let mut course_count = 0usize;
    for (index, row) in rows.iter().enumerate() {
        let cells = campus_html::direct_children(row.inner(), "td")?;
        // A set row names the set, states its attribute, and carries the
        // set's first course; a course row carries only course fields, at an
        // offset two cells earlier than in a set row.  The legacy service has
        // exactly these two shapes and reports anything else as an unknown
        // course level, so an unrecognized shape is an error here too.
        match classify_full_row(&cells, index)? {
            FullRowKind::Header => {
                let name = cell_text(&cells, 0);
                let kind = full_course_set_kind(&cell_text(&cells, 1))
                    .ok_or(ProgramParseError::UnrecognizedCourseSet { row: index })?;
                if course_sets.len() >= MAX_COURSE_SETS {
                    return Err(ProgramParseError::TooLarge);
                }
                let first = full_course(&cells, FULL_SET_FIRST_COURSE_CELL, index)?;
                course_count += 1;
                course_sets.push(CourseSetFull {
                    name,
                    kind,
                    courses: vec![first],
                });
            }
            FullRowKind::Course => {
                let Some(current) = course_sets.last_mut() else {
                    return Err(ProgramParseError::UnrecognizedCourseRow { row: index });
                };
                if course_count >= MAX_COURSES {
                    return Err(ProgramParseError::TooLarge);
                }
                course_count += 1;
                current
                    .courses
                    .push(full_course(&cells, FULL_COURSE_FIRST_CELL, index)?);
            }
        }
    }

    Ok(FullProgram { course_sets })
}

/// The observed completion row shapes.
///
/// Each shape is identified by the number of cells in the row, because that is
/// the only signal the service actually varies: the same table renders an
/// attribute section, a sub-group, and a plain course with different column
/// counts, and the course fields sit at a different offset in each.  The count
/// is combined with a semantic check (the attribute cell must be a known
/// value) so a reordered or widened row is reported rather than misread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// The first row of a course-attribute section: names the section, states
    /// its requirements, and carries the section's first course.
    AttributeSection,
    /// The first row of a sub-group inside the current attribute section.
    GroupSection,
    /// Any further course row of the current course set.
    Course,
    /// The row that opens the out-of-plan group.
    ExcludedGroup,
    /// A course row inside the out-of-plan group.
    ExcludedCourse,
}

/// Cell counts of the four observed completion row shapes.
const ATTRIBUTE_SECTION_CELLS: usize = 12;
const GROUP_SECTION_CELLS: usize = 11;
const COURSE_CELLS: usize = 5;
const EXCLUDED_CELLS: usize = 7;

/// The cell positions of an attribute-section row.
const SECTION_ATTRIBUTE_CELL: usize = 0;
const SECTION_NAME_CELL: usize = 1;
const SECTION_FIRST_COURSE_CELL: usize = 2;
const SECTION_COMPLETION_CELL: usize = 11;
/// The cell positions of a sub-group row.
const GROUP_NAME_CELL: usize = 0;
const GROUP_FIRST_COURSE_CELL: usize = 1;
const GROUP_COMPLETION_CELL: usize = 10;
/// The cell positions of an out-of-plan row.
const EXCLUDED_ID_CELL: usize = 0;
const EXCLUDED_NAME_CELL: usize = 2;
const EXCLUDED_CREDIT_CELL: usize = 3;
const EXCLUDED_GRADE_CELL: usize = 5;
const EXCLUDED_POINT_CELL: usize = 6;

/// Classifies one completion row.  A row matching no observed shape is an
/// error, never a silently dropped row: a dropped row would look like a
/// smaller course list rather than a changed page.
fn classify_row(cells: &[RawElement], row: usize) -> Result<RowKind, ProgramParseError> {
    match cells.len() {
        ATTRIBUTE_SECTION_CELLS => {
            if course_set_kind(&cell_text(cells, SECTION_ATTRIBUTE_CELL)).is_none() {
                return Err(ProgramParseError::UnrecognizedCourseSet { row });
            }
            if cell_text(cells, SECTION_NAME_CELL).is_empty() {
                return Err(ProgramParseError::MissingCourseField {
                    row,
                    field: "course_set_name",
                });
            }
            Ok(RowKind::AttributeSection)
        }
        GROUP_SECTION_CELLS => {
            if cell_text(cells, GROUP_NAME_CELL).is_empty() {
                return Err(ProgramParseError::MissingCourseField {
                    row,
                    field: "course_set_name",
                });
            }
            Ok(RowKind::GroupSection)
        }
        COURSE_CELLS => Ok(RowKind::Course),
        // The out-of-plan group row and its course rows share one shape, so
        // the identifier cell is what tells them apart, exactly as the legacy
        // page does.
        EXCLUDED_CELLS => {
            if cell_text(cells, EXCLUDED_ID_CELL).is_empty() {
                Ok(RowKind::ExcludedGroup)
            } else {
                Ok(RowKind::ExcludedCourse)
            }
        }
        _ => Err(ProgramParseError::UnrecognizedCourseRow { row }),
    }
}

/// Maps the attribute cell to its group kind.  An unknown attribute is an
/// error: reporting it as an elective would misstate a graduation requirement.
fn course_set_kind(text: &str) -> Option<CourseSetKind> {
    match text {
        "必修" => Some(CourseSetKind::Compulsory),
        "限选" => Some(CourseSetKind::Restricted),
        "任选" => Some(CourseSetKind::Elective),
        _ => None,
    }
}

/// The out-of-plan group, which the page never states requirements for.
fn excluded_group(excluded_credit: Option<f64>) -> CourseSetCompletion {
    CourseSetCompletion {
        name: "方案外课程".to_owned(),
        kind: CourseSetKind::Excluded,
        required_credit: None,
        completed_credit: excluded_credit,
        required_course_count: None,
        completed_course_count: None,
        full_completed: false,
        courses: Vec::new(),
    }
}

/// Reads a requirement header's four numeric cells that precede the flag.
fn requirement_header(
    cells: &[RawElement],
    flag_index: usize,
    name: String,
    kind: CourseSetKind,
) -> Result<CourseSetCompletion, ProgramParseError> {
    if flag_index < 4 || flag_index >= cells.len() {
        return Err(ProgramParseError::UnrecognizedCourseRow { row: flag_index });
    }
    Ok(CourseSetCompletion {
        name,
        kind,
        required_credit: optional_cell_number(cells, flag_index - 4)?,
        completed_credit: optional_cell_number(cells, flag_index - 3)?,
        required_course_count: optional_cell_count(cells, flag_index - 2)?,
        completed_course_count: optional_cell_count(cells, flag_index - 1)?,
        full_completed: cell_text(cells, flag_index) == COMPLETION_FLAG_YES,
        courses: Vec::new(),
    })
}

/// Reads one out-of-plan course row.  Its identifier is the leading cell; the
/// column that follows it names the offering department, so the course name
/// sits one cell further right than in an in-plan row.
fn parse_excluded_course_row(
    cells: &[RawElement],
    row: usize,
) -> Result<CourseCompletion, ProgramParseError> {
    let course_id = cell_text(cells, EXCLUDED_ID_CELL);
    if course_id.is_empty() {
        return Err(ProgramParseError::MissingCourseField {
            row,
            field: "course_id",
        });
    }
    let name = cell_text(cells, EXCLUDED_NAME_CELL);
    if name.is_empty() {
        return Err(ProgramParseError::MissingCourseField {
            row,
            field: "course_name",
        });
    }
    let credit = optional_cell_number(cells, EXCLUDED_CREDIT_CELL)?.ok_or(
        ProgramParseError::MissingCourseField {
            row,
            field: "credit",
        },
    )?;
    let grade = cell_text(cells, EXCLUDED_GRADE_CELL);
    let point = optional_cell_number(cells, EXCLUDED_POINT_CELL)?;
    Ok(CourseCompletion {
        course_id,
        name,
        credit,
        point,
        grade: if grade.is_empty() { None } else { Some(grade) },
        state: CourseState::Completed,
    })
}

/// The two observed full-plan row shapes, told apart by cell count.
///
/// A set row is two cells wider than a course row: it adds the set name and
/// the attribute, and both shapes end with the same three course cells.  The
/// counts are the only signal the page varies, so a row matching neither is an
/// error rather than a course silently appended to the wrong set.
const FULL_SET_CELLS: usize = 5;
const FULL_COURSE_CELLS: usize = 3;
/// The first course's cells inside a set row and inside a course row.
const FULL_SET_FIRST_COURSE_CELL: usize = 2;
const FULL_COURSE_FIRST_CELL: usize = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FullRowKind {
    Header,
    Course,
}

fn classify_full_row(cells: &[RawElement], row: usize) -> Result<FullRowKind, ProgramParseError> {
    match cells.len() {
        FULL_SET_CELLS => {
            if cell_text(&cells, 0).is_empty() {
                return Err(ProgramParseError::MissingCourseField {
                    row,
                    field: "course_set_name",
                });
            }
            Ok(FullRowKind::Header)
        }
        FULL_COURSE_CELLS => Ok(FullRowKind::Course),
        _ => Err(ProgramParseError::UnrecognizedCourseRow { row }),
    }
}

fn full_course_set_kind(text: &str) -> Option<CourseSetKind> {
    match text {
        "必修" => Some(CourseSetKind::Compulsory),
        "限选" => Some(CourseSetKind::Restricted),
        "任选" => Some(CourseSetKind::Elective),
        _ => None,
    }
}

fn full_course(
    cells: &[RawElement],
    level: usize,
    row: usize,
) -> Result<CourseFull, ProgramParseError> {
    let course_id = cell_text(&cells, level);
    if course_id.is_empty() {
        return Err(ProgramParseError::MissingCourseField {
            row,
            field: "course_id",
        });
    }
    let name = cell_text(&cells, level + 1);
    if name.is_empty() {
        return Err(ProgramParseError::MissingCourseField {
            row,
            field: "course_name",
        });
    }
    let credit_text = cell_text(&cells, level + 2);
    let credit = credit_text
        .parse::<f64>()
        .map_err(|_| ProgramParseError::InvalidCourseField {
            row,
            field: "credit",
        })?;
    if !credit.is_finite() || credit < 0.0 {
        return Err(ProgramParseError::InvalidCourseField {
            row,
            field: "credit",
        });
    }
    Ok(CourseFull {
        course_id,
        name,
        credit,
    })
}

fn parse_course_row(
    cells: &[RawElement],
    level: usize,
    row: usize,
) -> Result<CourseCompletion, ProgramParseError> {
    let course_id = cell_text(cells, level);
    if course_id.is_empty() {
        return Err(ProgramParseError::MissingCourseField {
            row,
            field: "course_id",
        });
    }
    // An unfinished course is rendered with a different inner markup, so the
    // name is read from the same cell but tolerates an extra nesting level.
    let name = cell_text(cells, level + 1);
    if name.is_empty() {
        return Err(ProgramParseError::MissingCourseField {
            row,
            field: "course_name",
        });
    }
    let credit_text = cell_text(cells, level + 2);
    let credit = credit_text
        .parse::<f64>()
        .map_err(|_| ProgramParseError::InvalidCourseField {
            row,
            field: "credit",
        })?;
    if !credit.is_finite() || credit < 0.0 {
        return Err(ProgramParseError::InvalidCourseField {
            row,
            field: "credit",
        });
    }
    let grade_or_state = cell_text(cells, level + 3);
    let (state, grade) = match grade_or_state.as_str() {
        "未修" => (CourseState::NotCompleted, None),
        "选课" => (CourseState::Elected, None),
        "" => (CourseState::Completed, None),
        "W" | "F" | "I" => (CourseState::NotCompleted, Some(grade_or_state)),
        other => (CourseState::Completed, Some(other.to_owned())),
    };
    let point_text = cell_text(cells, level + 4);
    let point = if point_text.is_empty() {
        None
    } else {
        Some(
            point_text
                .parse::<f64>()
                .map_err(|_| ProgramParseError::InvalidCourseField {
                    row,
                    field: "point",
                })?,
        )
    };
    Ok(CourseCompletion {
        course_id,
        name,
        credit,
        point,
        grade,
        state,
    })
}

/// Runs the shared page guards and returns the document's normalized text.
fn guarded_text(html: &str) -> Result<String, ProgramParseError> {
    guard_page(html)?;
    let text = campus_html::element_text(html);
    if text.is_empty() {
        return Err(ProgramParseError::UnexpectedHtml);
    }
    Ok(text)
}

/// Runs the guards that classify a login or expired page before any parse.
fn guard_page(html: &str) -> Result<(), ProgramParseError> {
    if html.trim_start_matches('\u{feff}').trim().is_empty() {
        return Err(ProgramParseError::EmptyBody);
    }
    match campus_html::classify_page(html) {
        PageClass::Login => Err(ProgramParseError::LoginPage),
        PageClass::Expired => Err(ProgramParseError::ExpiredPage),
        PageClass::Unknown => Ok(()),
    }
}

fn cell_text(cells: &[RawElement], index: usize) -> String {
    cells.get(index).map(RawElement::text).unwrap_or_default()
}

fn optional_cell_number(
    cells: &[RawElement],
    index: usize,
) -> Result<Option<f64>, ProgramParseError> {
    let text = cell_text(cells, index);
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>()
        .map(Some)
        .map_err(|_| ProgramParseError::InvalidField { field: "credit" })
}

fn optional_cell_count(
    cells: &[RawElement],
    index: usize,
) -> Result<Option<u32>, ProgramParseError> {
    let text = cell_text(cells, index);
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u32>()
        .map(Some)
        .map_err(|_| ProgramParseError::InvalidField {
            field: "course_count",
        })
}

/// Returns the first number that follows `label` inside `text`.
fn number_after(text: &str, label: &'static str) -> Result<f64, ProgramParseError> {
    let anchor = text
        .find(label)
        .ok_or(ProgramParseError::MissingMarker { marker: label })?;
    let rest = &text[anchor + label.len()..];
    let digits: String = rest
        .chars()
        .skip_while(|character| character.is_whitespace())
        .take_while(|character| {
            character.is_ascii_digit() || *character == '.' || *character == '-'
        })
        .collect();
    let value = digits
        .parse::<f64>()
        .map_err(|_| ProgramParseError::InvalidField { field: "credit" })?;
    if !value.is_finite() {
        return Err(ProgramParseError::InvalidField { field: "credit" });
    }
    Ok(value)
}

/// Reads the duplicated-course list from the summary block.
fn duplicated_courses(summary: &str) -> Result<Vec<String>, ProgramParseError> {
    let anchor = summary
        .find("重复课程：")
        .ok_or(ProgramParseError::MissingMarker {
            marker: "重复课程：",
        })?;
    let rest = &summary[anchor + "重复课程：".len()..];
    let end = rest.find("属于多个课组").unwrap_or(rest.len());
    let names = rest[..end]
        .replace(['\t', '{', '}', '&', '#', '0', '3', '4', '"', ';'], " ")
        .split('、')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(MAX_TEXT_CHARS).collect::<String>())
        .collect();
    Ok(names)
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, ProgramAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(ProgramAdapterError::InvalidBaseUrl);
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

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, ProgramAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(ProgramAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| ProgramAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(ProgramAdapterError::UnexpectedOrigin);
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
