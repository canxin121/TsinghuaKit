//! Cookie-aware, read-only execution for the library seat service.
//!
//! The module contains the complete Rust boundary for the three read stages
//! used by the application: area discovery, opening windows, and seat
//! availability. `LibraryReadAdapter` executes the observed GET routes through
//! a caller-owned `CampusHttpTransport`, so an INFO/WebVPN handoff and every
//! library request share one Cookie jar. The parsers keep the server's JSON
//! envelope strict while treating a valid empty list as a successful result.
//!
//! The wire behavior comes from the local campus-services audit and the pinned
//! open-source reference. Only route and response-shape behavior is used; no
//! source, fixture, or asset is copied from the referenced projects.

use std::{fmt, time::Duration};

use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::transport::{CampusHttpTransport, CampusTextResponse, TransportError};

pub const LIBRARY_AREA_TREE_PATH: &str = "/api.php/areas/1/tree/1";
pub const LIBRARY_AREAS_PATH_PREFIX: &str = "/api.php/areas/";
pub const LIBRARY_DAY_SEGMENTS_PATH_PREFIX: &str = "/api.php/areadays/";
pub const LIBRARY_SEAT_AVAILABILITY_PATH: &str = "/api.php/spaces_old";
/// The independent campus-app endpoint used by the public library clients to
/// report the power-socket state associated with seats.  It is intentionally
/// kept separate from the seat inventory route: the two responses have
/// different origins, shapes, and failure semantics.
pub const LIBRARY_SOCKET_STATUS_ORIGIN: &str = "https://app.cs.tsinghua.edu.cn/";
pub const LIBRARY_SOCKET_STATUS_HOST: &str = "app.cs.tsinghua.edu.cn";
pub const LIBRARY_SOCKET_STATUS_PATH: &str = "/api/socket";
/// Target service reached through the INFO/WebVPN roaming handoff. The
/// WebVPN mapping host and path remain opaque to this module.
pub const LIBRARY_WEBVPN_TARGET_HOST: &str = "seat.lib.tsinghua.cn";
pub const LIBRARY_HOME_PATH: &str = "/home/web/f_second";

/// Every plan is executed through the caller's already-established INFO
/// WebVPN transport. This marker is deliberately the only session detail
/// present in a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// The only HTTP method available from this read-only profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryReadMethod {
    Get,
}

/// The operation represented by a library read request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryReadOperation {
    AreaTree,
    AreaChildren,
    AreaForDay,
    DaySegments,
    SeatAvailability,
    SocketStatus,
}

/// A transport-neutral GET plan.
///
/// The path is always relative to the caller's INFO/WebVPN origin. Query
/// values contain only area, segment, date, and time selectors. Cookies,
/// tickets, access tokens, and absolute URLs have no representation here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryReadRequestPlan {
    pub operation: LibraryReadOperation,
    pub method: LibraryReadMethod,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub session_prerequisite: LibrarySessionPrerequisite,
}

impl LibraryReadRequestPlan {
    pub fn query_parameters(&self) -> &[(String, String)] {
        &self.query
    }

    pub fn is_relative_path(&self) -> bool {
        self.path.starts_with('/') && !self.path.contains("://")
    }
}

/// Request validation errors for the read-only profile.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LibraryRequestError {
    #[error("{field} identifier must be greater than zero")]
    InvalidIdentifier { field: &'static str },

    #[error("day must use a valid YYYY-MM-DD value: {value:?}")]
    InvalidDay { value: String },

    #[error("{field} must use HH:MM or HH:MM:SS: {value:?}")]
    InvalidTime { field: &'static str, value: String },

    #[error("start_time must be earlier than end_time")]
    InvalidTimeRange,
}

/// Fixed route profile for the observed library seat read endpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryReadProfile;

impl LibraryReadProfile {
    pub const fn new() -> Self {
        Self
    }

    pub fn area_tree_request(&self) -> LibraryReadRequestPlan {
        self.plan(
            LibraryReadOperation::AreaTree,
            LIBRARY_AREA_TREE_PATH.to_owned(),
            Vec::new(),
        )
    }

    pub fn area_children_request(
        &self,
        area_id: u64,
    ) -> Result<LibraryReadRequestPlan, LibraryRequestError> {
        let area_id = checked_identifier(area_id, "area_id")?;
        Ok(self.plan(
            LibraryReadOperation::AreaChildren,
            format!("{LIBRARY_AREAS_PATH_PREFIX}{area_id}"),
            Vec::new(),
        ))
    }

    pub fn area_for_day_request(
        &self,
        area_id: u64,
        day: &str,
    ) -> Result<LibraryReadRequestPlan, LibraryRequestError> {
        let area_id = checked_identifier(area_id, "area_id")?;
        let day = normalize_day_for_request(day)?;
        Ok(self.plan(
            LibraryReadOperation::AreaForDay,
            format!("{LIBRARY_AREAS_PATH_PREFIX}{area_id}/date/{day}"),
            Vec::new(),
        ))
    }

    pub fn day_segments_request(
        &self,
        section_id: u64,
    ) -> Result<LibraryReadRequestPlan, LibraryRequestError> {
        let section_id = checked_identifier(section_id, "section_id")?;
        Ok(self.plan(
            LibraryReadOperation::DaySegments,
            format!("{LIBRARY_DAY_SEGMENTS_PATH_PREFIX}{section_id}"),
            Vec::new(),
        ))
    }

    pub fn seat_availability_request(
        &self,
        area_id: u64,
        segment_id: u64,
        day: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<LibraryReadRequestPlan, LibraryRequestError> {
        let area_id = checked_identifier(area_id, "area_id")?;
        let segment_id = checked_identifier(segment_id, "segment_id")?;
        let day = normalize_day_for_request(day)?;
        let start_time = normalize_time_for_request(start_time, "start_time")?;
        let end_time = normalize_time_for_request(end_time, "end_time")?;
        if start_time >= end_time {
            return Err(LibraryRequestError::InvalidTimeRange);
        }

        Ok(self.plan(
            LibraryReadOperation::SeatAvailability,
            LIBRARY_SEAT_AVAILABILITY_PATH.to_owned(),
            vec![
                ("area".to_owned(), area_id.to_string()),
                ("segment".to_owned(), segment_id.to_string()),
                ("day".to_owned(), day),
                ("startTime".to_owned(), start_time),
                ("endTime".to_owned(), end_time),
            ],
        ))
    }

    /// Builds the independent socket-state GET observed in the public
    /// library clients.  The response is not a seat inventory response and
    /// must be parsed by [`parse_socket_status`] separately.
    pub fn socket_status_request(
        &self,
        section_id: u64,
    ) -> Result<LibraryReadRequestPlan, LibraryRequestError> {
        let section_id = checked_identifier(section_id, "section_id")?;
        Ok(self.plan(
            LibraryReadOperation::SocketStatus,
            LIBRARY_SOCKET_STATUS_PATH.to_owned(),
            vec![("sectionid".to_owned(), section_id.to_string())],
        ))
    }

    fn plan(
        &self,
        operation: LibraryReadOperation,
        path: String,
        query: Vec<(String, String)>,
    ) -> LibraryReadRequestPlan {
        LibraryReadRequestPlan {
            operation,
            method: LibraryReadMethod::Get,
            path,
            query,
            session_prerequisite: LibrarySessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// A normalized area tree or area-child response.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryAreaTree {
    pub areas: Vec<LibraryArea>,
}

/// A normalized area node. Optional fields remain optional because the
/// observed top-level and child responses do not expose identical metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryArea {
    pub id: u64,
    pub name: String,
    pub name_merge: Option<String>,
    pub english_name: Option<String>,
    pub english_name_merge: Option<String>,
    pub is_valid: Option<bool>,
    pub total_count: Option<u64>,
    pub unavailable_space: Option<u64>,
    pub available_count: Option<u64>,
    pub point_x: Option<f64>,
    pub point_y: Option<f64>,
    pub child_areas: Vec<LibraryArea>,
}

/// One observed availability window for a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryDaySegment {
    pub day: String,
    pub start_time: String,
    pub end_time: String,
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryDaySegments {
    pub segments: Vec<LibraryDaySegment>,
}

/// A seat record from spaces_old.
///
/// Status and area_type are retained as opaque integer fields. The reference
/// behavior uses status 1 when deriving validity and keeps area_type for
/// internal use; this foundation does not invent a richer meaning. Socket
/// status is a separate endpoint and is merged only by the explicit combined
/// adapter method below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySeat {
    pub id: u64,
    pub name: String,
    pub status: i64,
    pub area_type: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySeatAvailability {
    pub seats: Vec<LibrarySeat>,
}

/// The socket service exposes these three status strings.  An absent socket
/// record is represented by the absence of a record in the returned list;
/// callers must not infer a status from the seat inventory's numeric status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LibrarySocketState {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySocketStatusRecord {
    pub seat_id: u64,
    pub status: LibrarySocketState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySocketStatuses {
    pub records: Vec<LibrarySocketStatusRecord>,
}

/// JSON-safe DTOs returned by [`LibraryReadAdapter`].  They intentionally
/// contain only library records.  The WebVPN URL, cookies, tickets, and any
/// other session material remain inside the adapter and its transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryAreaDto {
    pub id: u64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_merge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub english_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub english_name_merge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_space: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point_y: Option<f64>,
    pub child_areas: Vec<LibraryAreaDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryAreaTreeDto {
    pub areas: Vec<LibraryAreaDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryDaySegmentDto {
    pub day: String,
    pub start_time: String,
    pub end_time: String,
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryDaySegmentsDto {
    pub segments: Vec<LibraryDaySegmentDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibrarySeatDto {
    pub id: u64,
    pub name: String,
    pub status: i64,
    pub area_type: i64,
    /// The public clients use status 1 as the seat-validity signal.  The raw
    /// status is retained as well because the upstream service has other
    /// status values whose meaning is deployment-specific.
    pub is_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibrarySeatAvailabilityDto {
    pub seats: Vec<LibrarySeatDto>,
}

/// One seat after the inventory response and the independent socket response
/// have been joined by `seatId`.
///
/// A missing socket record is represented by [`LibrarySocketState::Unknown`].
/// This makes a valid empty socket array observable to callers without
/// pretending that the seat has an available or unavailable socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibrarySeatWithSocketDto {
    pub id: u64,
    pub name: String,
    pub status: i64,
    pub area_type: i64,
    pub is_valid: bool,
    pub socket_status: LibrarySocketState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibrarySeatAvailabilityWithSocketDto {
    pub seats: Vec<LibrarySeatWithSocketDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibrarySocketStatusRecordDto {
    pub seat_id: u64,
    pub status: LibrarySocketState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibrarySocketStatusesDto {
    pub records: Vec<LibrarySocketStatusRecordDto>,
}

/// Errors raised while executing a read-only library request through an
/// already authenticated INFO/WebVPN transport.
///
/// Response bodies are deliberately absent from every error variant.  A
/// successful response is parsed immediately and a failed response is only
/// classified by status or by the login-page detector.
#[derive(Debug, Error)]
pub enum LibraryAdapterError {
    #[error("library WebVPN base URL is invalid")]
    InvalidBaseUrl,

    #[error("library request is invalid: {0}")]
    Request(#[from] LibraryRequestError),

    // The source is retained for typed matching and Debug redaction, but its
    // Display implementation is intentionally not interpolated: reqwest may
    // include the opaque WebVPN URL in a request error.
    #[error("library transport failed")]
    Transport(#[source] TransportError),

    #[error("library HTTP request returned {status}")]
    HttpStatus { status: StatusCode },

    #[error("library response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("library response ended outside the configured WebVPN mapping")]
    UnexpectedPath,

    #[error("library seat and socket readers do not share the same session context")]
    ForeignSocketContext,

    #[error("library INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("library WebVPN target or deployment returned an unexpected HTML page")]
    UnexpectedDeployment,

    #[error("library response could not be parsed: {0}")]
    Parse(#[source] LibraryReadParseError),
}

impl LibraryAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "library_config",
            Self::Request(_) => "library_request",
            Self::Transport(_) => "library_network",
            Self::HttpStatus { .. } => "library_http",
            Self::UnexpectedOrigin => "library_origin",
            Self::UnexpectedPath => "library_path",
            Self::ForeignSocketContext => "library_foreign_context",
            Self::SessionExpired => "library_auth_required",
            Self::UnexpectedDeployment => "library_html",
            Self::Parse(error) => match error {
                LibraryReadParseError::LoginHtml => "library_auth_required",
                LibraryReadParseError::FailureEnvelope => "library_business_failure",
                LibraryReadParseError::MissingData { .. }
                | LibraryReadParseError::InvalidEnvelope { .. } => "library_data_envelope",
                LibraryReadParseError::InvalidCollection { .. } => "library_collection",
                LibraryReadParseError::InvalidRecord {
                    collection: LibraryRecordKind::DaySegment,
                    field,
                    ..
                } => match field.as_str() {
                    "day" => "library_segment_day",
                    "startTime" | "startTime.date" => "library_segment_start",
                    "endTime" | "endTime.date" => "library_segment_end",
                    "id" => "library_segment_id",
                    _ => "library_segment_record",
                },
                LibraryReadParseError::ConflictingIdentifier { .. } => "library_conflicting_ids",
                LibraryReadParseError::InvalidRecord { .. } => "library_record",
                _ => "library_parse",
            },
        }
    }
    /// Returns whether the caller should discard the library proof and ask
    /// the session layer to establish it again.
    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired | Self::Parse(LibraryReadParseError::LoginHtml)
        )
    }

    /// Returns whether retry policy may treat this error as temporary.
    ///
    /// The error itself remains typed as [`LibraryAdapterError::HttpStatus`]
    /// or [`LibraryAdapterError::Transport`], so callers that need the exact
    /// status or transport source do not lose that evidence. Parsing,
    /// origin, path, deployment, and configuration failures are never
    /// retryable through this classification.
    pub fn is_temporary_network(&self) -> bool {
        match self {
            Self::HttpStatus { status } => {
                *status == StatusCode::REQUEST_TIMEOUT
                    || *status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
            }
            Self::Transport(TransportError::Request(error)) => {
                error.is_timeout() || error.is_connect()
            }
            Self::Transport(TransportError::Decode(_)) => true,
            _ => false,
        }
    }
}

/// Configuration for [`LibraryReadAdapter`].
///
/// `base_url` is the opaque directory returned by the INFO/WebVPN handoff,
/// for example a WebVPN mapped directory ending in `/`.  The adapter only
/// appends the relative library paths from [`LibraryReadProfile`]; it never
/// constructs or inspects a WebVPN mapping token.
#[derive(Clone)]
pub struct LibraryAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl LibraryAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, LibraryAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/library", Duration::from_secs(20))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, LibraryAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(20))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, LibraryAdapterError> {
        let parsed = Url::parse(base_url).map_err(|_| LibraryAdapterError::InvalidBaseUrl)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(LibraryAdapterError::InvalidBaseUrl);
        }

        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(LibraryAdapterError::InvalidBaseUrl);
        }

        let base_url = normalize_library_base_url(parsed)?;
        Ok(Self {
            base_url,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, LibraryAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(LibraryAdapterError::Transport)
    }
}

impl fmt::Debug for LibraryAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Read-only library client that reuses the caller's Cookie-aware transport.
///
/// The authenticated runtime must construct this adapter with
/// [`LibraryReadAdapter::try_with_transport`], passing the transport that
/// performed the INFO/WebVPN handoff. `CampusHttpTransport::clone` shares its
/// underlying Cookie jar, so the adapter never creates a second account
/// session. `new` is retained for callers that already have an independently
/// authenticated library transport; it does not perform INFO login itself.
pub struct LibraryReadAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: LibraryReadProfile,
}

impl LibraryReadAdapter {
    pub fn new(config: LibraryAdapterConfig) -> Result<Self, LibraryAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    fn with_transport(base_url: Url, transport: CampusHttpTransport) -> Self {
        Self {
            base_url,
            transport,
            profile: LibraryReadProfile::new(),
        }
    }

    /// Builds an adapter from the URL returned by INFO roaming.  Public
    /// clients commonly receive the library home page URL; normalize that
    /// known page suffix back to the opaque WebVPN mapping directory before
    /// appending the API routes.
    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, LibraryAdapterError> {
        let base_url = validate_library_base_url(base_url)?;
        let base_url = normalize_library_base_url(base_url)?;
        Ok(Self::with_transport(base_url, transport))
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> LibraryReadProfile {
        self.profile
    }

    /// Creates the socket-status reader from this adapter's transport.  The
    /// returned adapter therefore shares the INFO/WebVPN Cookie jar instead
    /// of silently creating a second unauthenticated client.  Production
    /// callers should use [`Self::app_socket_status_adapter`]; the URL-taking
    /// form is useful when INFO supplies an opaque WebVPN mapping or when a
    /// contract fixture provides a local origin.
    pub fn socket_status_adapter(
        &self,
        base_url: Url,
    ) -> Result<LibrarySocketStatusAdapter, LibraryAdapterError> {
        LibrarySocketStatusAdapter::try_with_transport(base_url, self.transport.clone())
    }

    /// Creates a socket-status reader for the independently hosted campus
    /// app endpoint while retaining this adapter's Cookie-aware transport.
    pub fn app_socket_status_adapter(
        &self,
    ) -> Result<LibrarySocketStatusAdapter, LibraryAdapterError> {
        let base_url = Url::parse(LIBRARY_SOCKET_STATUS_ORIGIN)
            .map_err(|_| LibraryAdapterError::InvalidBaseUrl)?;
        self.socket_status_adapter(base_url)
    }

    pub async fn read_area_tree(&self) -> Result<LibraryAreaTreeDto, LibraryAdapterError> {
        let plan = self.profile.area_tree_request();
        let body = self.execute(&plan).await?;
        parse_area_tree(&body)
            .map(map_area_tree)
            .map_err(Self::map_parse_error)
    }

    pub async fn read_area_children(
        &self,
        area_id: u64,
    ) -> Result<LibraryAreaTreeDto, LibraryAdapterError> {
        let plan = self.profile.area_children_request(area_id)?;
        let body = self.execute(&plan).await?;
        parse_area_children(&body)
            .map(map_area_tree)
            .map_err(Self::map_parse_error)
    }

    pub async fn read_area_for_day(
        &self,
        area_id: u64,
        day: &str,
    ) -> Result<LibraryAreaTreeDto, LibraryAdapterError> {
        let plan = self.profile.area_for_day_request(area_id, day)?;
        let body = self.execute(&plan).await?;
        parse_area_for_day(&body)
            .map(map_area_tree)
            .map_err(Self::map_parse_error)
    }

    pub async fn read_day_segments(
        &self,
        section_id: u64,
    ) -> Result<LibraryDaySegmentsDto, LibraryAdapterError> {
        let plan = self.profile.day_segments_request(section_id)?;
        let body = self.execute(&plan).await?;
        parse_day_segments(&body)
            .map(map_day_segments)
            .map_err(Self::map_parse_error)
    }

    pub async fn read_seat_availability(
        &self,
        area_id: u64,
        segment_id: u64,
        day: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<LibrarySeatAvailabilityDto, LibraryAdapterError> {
        let plan = self
            .profile
            .seat_availability_request(area_id, segment_id, day, start_time, end_time)?;
        let body = self.execute(&plan).await?;
        parse_seat_availability(&body)
            .map(map_seat_availability)
            .map_err(Self::map_parse_error)
    }

    /// Reads the seat inventory and the independent socket status list, then
    /// joins the two verified responses by the seat id.
    ///
    /// The observed public client uses the same selected section id for the
    /// seat query's `area` parameter and the socket query's `sectionid`
    /// parameter. The explicit socket adapter is accepted by reference so a
    /// contract test can provide a local origin; production callers should
    /// obtain it from [`Self::app_socket_status_adapter`]. Both adapters must
    /// be constructed from the same [`CampusHttpTransport`] (or its clone),
    /// which is what preserves the INFO/WebVPN Cookie jar.
    pub async fn read_seat_availability_with_socket(
        &self,
        area_id: u64,
        segment_id: u64,
        day: &str,
        start_time: &str,
        end_time: &str,
        socket: &LibrarySocketStatusAdapter,
    ) -> Result<LibrarySeatAvailabilityWithSocketDto, LibraryAdapterError> {
        // These responses must belong to one caller-owned runtime, as the
        // constructor contract requires. Do not dispatch either request if
        // an independently created Cookie context is supplied for the join.
        if !std::sync::Arc::ptr_eq(self.transport.cookie_jar(), socket.transport.cookie_jar()) {
            return Err(LibraryAdapterError::ForeignSocketContext);
        }
        let seats = self
            .read_seat_availability(area_id, segment_id, day, start_time, end_time)
            .await?;
        let sockets = socket.read_section_status(area_id).await?;

        Ok(LibrarySeatAvailabilityWithSocketDto {
            seats: seats
                .seats
                .into_iter()
                .map(|seat| {
                    let socket_status = sockets
                        .records
                        .iter()
                        .find(|record| record.seat_id == seat.id)
                        .map_or(LibrarySocketState::Unknown, |record| record.status);
                    LibrarySeatWithSocketDto {
                        id: seat.id,
                        name: seat.name,
                        status: seat.status,
                        area_type: seat.area_type,
                        is_valid: seat.is_valid,
                        socket_status,
                    }
                })
                .collect(),
        })
    }

    /// Convenience form of [`Self::read_seat_availability_with_socket`] for
    /// the observed direct campus-app socket origin.
    pub async fn read_seat_availability_with_app_socket(
        &self,
        area_id: u64,
        segment_id: u64,
        day: &str,
        start_time: &str,
        end_time: &str,
    ) -> Result<LibrarySeatAvailabilityWithSocketDto, LibraryAdapterError> {
        let socket = self.app_socket_status_adapter()?;
        self.read_seat_availability_with_socket(
            area_id, segment_id, day, start_time, end_time, &socket,
        )
        .await
    }

    async fn execute(&self, plan: &LibraryReadRequestPlan) -> Result<String, LibraryAdapterError> {
        let endpoint = self.endpoint(&plan.path)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint).query(&plan.query))
            .await
            .map_err(|error| LibraryAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        // The shared campus transport deliberately refuses redirects to a
        // different origin. Keep the Location value only long enough to
        // classify a blocked redirect to the identity login page; never put
        // that potentially opaque URL into an error or DTO.
        let login_redirect_location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| LibraryAdapterError::Transport(TransportError::Decode(error)))?;
        let response = CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location: login_redirect_location.clone(),
            body,
        };

        // A WebVPN login page is frequently returned with HTTP 200 after a
        // redirect. Check the final URL and body before treating status as a
        // normal server error. HTTP 401 is also an unambiguous session signal
        // even when the deployment returns a plain-text body.
        let blocked_login_redirect = login_redirect_location
            .as_deref()
            .is_some_and(|location| looks_like_login_location(&response.final_url, location));
        let cross_origin_response = !same_origin(&self.base_url, &response.final_url)
            || login_redirect_location.as_deref().is_some_and(|location| {
                redirect_leaves_origin(&self.base_url, &response.final_url, location)
            });
        let location_outside_mapping = login_redirect_location
            .as_deref()
            .and_then(|location| resolve_location(&response.final_url, location))
            .is_some_and(|location| {
                same_origin(&self.base_url, &location)
                    && !path_within_base(&self.base_url, &location)
            });
        if response.status == StatusCode::UNAUTHORIZED
            || response.status == StatusCode::FORBIDDEN
            || is_library_login_response(&response)
            || blocked_login_redirect
        {
            return Err(LibraryAdapterError::SessionExpired);
        }
        if cross_origin_response {
            return Err(LibraryAdapterError::UnexpectedOrigin);
        }
        // These endpoints are read-only JSON GET routes. The public reference
        // accepts 200 for every one of the five calls; accepting another 2xx
        // here would let a 201/204 proxy response bypass the route contract.
        if response.status != StatusCode::OK {
            return Err(LibraryAdapterError::HttpStatus {
                status: response.status,
            });
        }
        // The WebVPN mapping is shared by several applications. A response
        // from another route under that mapping can still contain a
        // `data.list` envelope, so the mapping prefix alone is not a proof
        // that this library operation ran.
        if response.final_url.path() != expected_path
            || !query_matches(plan.query_parameters(), &response.final_url)
        {
            return Err(LibraryAdapterError::UnexpectedPath);
        }
        if !path_within_base(&self.base_url, &response.final_url) || location_outside_mapping {
            return Err(LibraryAdapterError::UnexpectedPath);
        }
        if looks_like_html(&response.body) {
            return Err(LibraryAdapterError::UnexpectedDeployment);
        }
        Ok(response.body)
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, LibraryAdapterError> {
        if !relative_path.starts_with('/')
            || relative_path.contains("://")
            || relative_path.contains(['?', '#'])
            || relative_path.contains("..")
            || relative_path.chars().any(char::is_control)
        {
            return Err(LibraryAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        let path = format!("{base_path}{relative_path}");
        endpoint.set_path(&path);
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: LibraryReadParseError) -> LibraryAdapterError {
        match error {
            LibraryReadParseError::LoginHtml => LibraryAdapterError::SessionExpired,
            LibraryReadParseError::UnexpectedHtml => LibraryAdapterError::UnexpectedDeployment,
            other => LibraryAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for LibraryReadAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryReadAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .finish()
    }
}

/// Read-only adapter for the independent socket-status endpoint.
///
/// The socket service is hosted by `app.cs.tsinghua.edu.cn` in the observed
/// deployment, while the seat inventory is reached through a library
/// WebVPN mapping.  Keeping this adapter separate prevents a valid
/// `data.list` response from the inventory service from being mistaken for a
/// socket response.  The adapter accepts an opaque base URL so a caller can
/// supply an INFO/WebVPN mapping without exposing its mapping token to a
/// request plan; the production convenience constructor uses the observed
/// direct origin.
pub struct LibrarySocketStatusAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: LibraryReadProfile,
}

impl LibrarySocketStatusAdapter {
    /// Builds a reader against the observed direct campus-app origin.
    pub fn for_app_service(transport: CampusHttpTransport) -> Result<Self, LibraryAdapterError> {
        let base_url = Url::parse(LIBRARY_SOCKET_STATUS_ORIGIN)
            .map_err(|_| LibraryAdapterError::InvalidBaseUrl)?;
        Self::try_with_transport(base_url, transport)
    }

    /// Builds a reader with a caller-provided origin or opaque WebVPN
    /// mapping, while retaining the caller's Cookie jar.
    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, LibraryAdapterError> {
        let base_url = normalize_socket_base_url(validate_socket_base_url(base_url)?)?;
        Ok(Self {
            base_url,
            transport,
            profile: LibraryReadProfile::new(),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> LibraryReadProfile {
        self.profile
    }

    pub async fn read_section_status(
        &self,
        section_id: u64,
    ) -> Result<LibrarySocketStatusesDto, LibraryAdapterError> {
        let plan = self.profile.socket_status_request(section_id)?;
        let body = self.execute(&plan).await?;
        parse_socket_status(&body)
            .map(map_socket_status)
            .map_err(LibraryReadAdapter::map_parse_error)
    }

    async fn execute(&self, plan: &LibraryReadRequestPlan) -> Result<String, LibraryAdapterError> {
        let endpoint = self.endpoint(&plan.path)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint).query(&plan.query))
            .await
            .map_err(|error| LibraryAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let redirect_location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| LibraryAdapterError::Transport(TransportError::Decode(error)))?;
        let response = CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location: redirect_location.clone(),
            body,
        };

        let blocked_login_redirect = redirect_location
            .as_deref()
            .is_some_and(|location| looks_like_login_location(&response.final_url, location));
        let cross_origin_response = !same_origin(&self.base_url, &response.final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                redirect_leaves_origin(&self.base_url, &response.final_url, location)
            });
        let redirect_outside_contract = redirect_location
            .as_deref()
            .and_then(|location| resolve_location(&response.final_url, location))
            .is_some_and(|location| {
                same_origin(&self.base_url, &location)
                    && (location.path() != expected_path
                        || !query_matches(plan.query_parameters(), &location))
            });

        if response.status == StatusCode::UNAUTHORIZED
            || response.status == StatusCode::FORBIDDEN
            || is_library_login_response(&response)
            || blocked_login_redirect
        {
            return Err(LibraryAdapterError::SessionExpired);
        }
        if cross_origin_response {
            return Err(LibraryAdapterError::UnexpectedOrigin);
        }
        if response.status != StatusCode::OK {
            return Err(LibraryAdapterError::HttpStatus {
                status: response.status,
            });
        }
        if response.final_url.path() != expected_path
            || !query_matches(plan.query_parameters(), &response.final_url)
            || redirect_outside_contract
            || !path_within_base(&self.base_url, &response.final_url)
        {
            return Err(LibraryAdapterError::UnexpectedPath);
        }
        if looks_like_html(&response.body) {
            return Err(LibraryAdapterError::UnexpectedDeployment);
        }
        Ok(response.body)
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, LibraryAdapterError> {
        if !relative_path.starts_with('/')
            || relative_path.contains("://")
            || relative_path.contains(['?', '#'])
            || relative_path.contains("..")
            || relative_path.chars().any(char::is_control)
        {
            return Err(LibraryAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        let path = format!("{base_path}{relative_path}");
        endpoint.set_path(&path);
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }
}

impl fmt::Debug for LibrarySocketStatusAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySocketStatusAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .finish()
    }
}

fn map_area_tree(tree: LibraryAreaTree) -> LibraryAreaTreeDto {
    LibraryAreaTreeDto {
        areas: tree.areas.into_iter().map(map_area).collect(),
    }
}

fn map_area(area: LibraryArea) -> LibraryAreaDto {
    LibraryAreaDto {
        id: area.id,
        name: area.name,
        name_merge: area.name_merge,
        english_name: area.english_name,
        english_name_merge: area.english_name_merge,
        is_valid: area.is_valid,
        total_count: area.total_count,
        unavailable_space: area.unavailable_space,
        available_count: area.available_count,
        point_x: area.point_x,
        point_y: area.point_y,
        child_areas: area.child_areas.into_iter().map(map_area).collect(),
    }
}

fn map_day_segments(segments: LibraryDaySegments) -> LibraryDaySegmentsDto {
    LibraryDaySegmentsDto {
        segments: segments
            .segments
            .into_iter()
            .map(|segment| LibraryDaySegmentDto {
                day: segment.day,
                start_time: segment.start_time,
                end_time: segment.end_time,
                id: segment.id,
            })
            .collect(),
    }
}

fn map_seat_availability(availability: LibrarySeatAvailability) -> LibrarySeatAvailabilityDto {
    LibrarySeatAvailabilityDto {
        seats: availability
            .seats
            .into_iter()
            .map(|seat| LibrarySeatDto {
                id: seat.id,
                name: seat.name,
                status: seat.status,
                area_type: seat.area_type,
                is_valid: seat.status == 1,
            })
            .collect(),
    }
}

fn map_socket_status(statuses: LibrarySocketStatuses) -> LibrarySocketStatusesDto {
    LibrarySocketStatusesDto {
        records: statuses
            .records
            .into_iter()
            .map(|record| LibrarySocketStatusRecordDto {
                seat_id: record.seat_id,
                status: record.status,
            })
            .collect(),
    }
}

fn validate_library_base_url(base_url: Url) -> Result<Url, LibraryAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(LibraryAdapterError::InvalidBaseUrl);
    }
    Ok(base_url)
}

fn validate_socket_base_url(base_url: Url) -> Result<Url, LibraryAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(LibraryAdapterError::InvalidBaseUrl);
    }
    Ok(base_url)
}

fn safe_base_path(path: &str) -> bool {
    !path.contains(['\\', '?', '#'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
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

fn normalize_library_base_url(mut base_url: Url) -> Result<Url, LibraryAdapterError> {
    let path = base_url.path().trim_end_matches('/');
    if let Some(root) = path.strip_suffix(LIBRARY_HOME_PATH) {
        let normalized = if root.is_empty() {
            "/".to_owned()
        } else {
            format!("{root}/")
        };
        base_url.set_path(&normalized);
    }
    Ok(base_url)
}

fn normalize_socket_base_url(mut base_url: Url) -> Result<Url, LibraryAdapterError> {
    let path = base_url.path().trim_end_matches('/');
    let normalized = if path.is_empty() {
        "/".to_owned()
    } else {
        path.to_owned()
    };
    base_url.set_path(&normalized);
    base_url.set_query(None);
    base_url.set_fragment(None);
    Ok(base_url)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryRecordKind {
    Area,
    DaySegment,
    Seat,
    SocketStatus,
}

impl fmt::Display for LibraryRecordKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Area => "area",
            Self::DaySegment => "day segment",
            Self::Seat => "seat",
            Self::SocketStatus => "socket status",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryRecordIssue {
    NotObject,
    MissingField,
    WrongType { expected: &'static str },
    EmptyValue,
    InvalidValue,
}

/// Errors are classified before a response crosses into a higher layer.
///
/// No variant stores the response body. This avoids retaining an HTML login
/// page, a server echo, or any other untrusted response in an error DTO.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LibraryReadParseError {
    #[error("library response is an HTML login page")]
    LoginHtml,

    #[error("library response is an unrecognized HTML page")]
    UnexpectedHtml,

    #[error("library response body is empty")]
    EmptyBody,

    #[error("library response is malformed JSON: {message}")]
    MalformedJson { message: String },

    #[error("library response root must be an object")]
    InvalidRoot,

    #[error("library response contains an explicit failure envelope")]
    FailureEnvelope,

    #[error("library response is missing {field}")]
    MissingData { field: &'static str },

    #[error("library response field {field} must be an object")]
    InvalidEnvelope { field: &'static str },

    #[error("library {collection} list has an invalid shape; expected {expected}")]
    InvalidCollection {
        collection: LibraryRecordKind,
        expected: &'static str,
    },

    #[error("library {collection} record {index} is invalid in {field}: {issue:?}")]
    InvalidRecord {
        collection: LibraryRecordKind,
        index: usize,
        field: String,
        issue: LibraryRecordIssue,
    },

    #[error("library {collection} record {index} conflicts with a previous identifier")]
    ConflictingIdentifier {
        collection: LibraryRecordKind,
        index: usize,
    },
}

/// Parses the root library response from `/api.php/areas/1/tree/1`.
///
/// The root list endpoint returns `data.list` as an array of libraries. The
/// floor and date-section endpoints use a different shape and are parsed by
/// [`parse_area_children`] and [`parse_area_for_day`]. Keeping these parsers
/// separate prevents a valid envelope from the wrong route from becoming a
/// false success.
pub fn parse_area_tree(body: &str) -> Result<LibraryAreaTree, LibraryReadParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let list = decode_data_list(body)?;
        let values = list
            .as_array()
            .ok_or(LibraryReadParseError::InvalidCollection {
                collection: LibraryRecordKind::Area,
                expected: "data.list to be an array",
            })?;
        let areas = parse_area_values(values)?;

        Ok(LibraryAreaTree { areas })
    })
}

/// Parses the floor list from `/api.php/areas/{library_id}`.
///
/// The service wraps this response as `data.list.childArea`. The returned
/// records are floors, while their optional nested `childArea` values remain
/// available because the upstream service uses the same area record shape at
/// every level.
pub fn parse_area_children(body: &str) -> Result<LibraryAreaTree, LibraryReadParseError> {
    parse_child_area_tree(body)
}

/// Parses the date-specific section list from
/// `/api.php/areas/{floor_id}/date/{YYYY-MM-DD}`.
///
/// This has the same wire envelope as the floor endpoint, but it is kept as a
/// named parser so the request route and its response shape stay explicit at
/// the adapter boundary.
pub fn parse_area_for_day(body: &str) -> Result<LibraryAreaTree, LibraryReadParseError> {
    parse_child_area_tree(body)
}

fn parse_child_area_tree(body: &str) -> Result<LibraryAreaTree, LibraryReadParseError> {
    let list = decode_data_list(body)?;
    let object = list
        .as_object()
        .ok_or(LibraryReadParseError::InvalidCollection {
            collection: LibraryRecordKind::Area,
            expected: "data.list to be an object with childArea",
        })?;
    let child_area = object
        .get("childArea")
        .ok_or(LibraryReadParseError::MissingData {
            field: "data.list.childArea",
        })?;
    let child_area = child_area
        .as_array()
        .ok_or(LibraryReadParseError::InvalidCollection {
            collection: LibraryRecordKind::Area,
            expected: "data.list.childArea to be an array",
        })?;
    let areas = parse_area_values(child_area)?;

    Ok(LibraryAreaTree { areas })
}

/// Parses the observed day-segment list under data.list.
pub fn parse_day_segments(body: &str) -> Result<LibraryDaySegments, LibraryReadParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let list = decode_data_list(body)?;
        let values = list
            .as_array()
            .ok_or(LibraryReadParseError::InvalidCollection {
                collection: LibraryRecordKind::DaySegment,
                expected: "data.list to be an array",
            })?;

        let segments = values
            .iter()
            .enumerate()
            .map(|(index, value)| parse_day_segment(value, index))
            .collect::<Result<Vec<_>, _>>()?;

        let segments =
            unique_library_records(segments, LibraryRecordKind::DaySegment, |segment| {
                (segment.day.clone(), segment.id)
            })?;
        Ok(LibraryDaySegments { segments })
    })
}

/// Parses the observed seat list under data.list.
pub fn parse_seat_availability(
    body: &str,
) -> Result<LibrarySeatAvailability, LibraryReadParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let list = decode_data_list(body)?;
        let values = list
            .as_array()
            .ok_or(LibraryReadParseError::InvalidCollection {
                collection: LibraryRecordKind::Seat,
                expected: "data.list to be an array",
            })?;

        let seats = values
            .iter()
            .enumerate()
            .map(|(index, value)| parse_seat(value, index))
            .collect::<Result<Vec<_>, _>>()?;

        let seats = unique_library_records(seats, LibraryRecordKind::Seat, |seat| seat.id)?;
        Ok(LibrarySeatAvailability { seats })
    })
}

/// Parses the direct socket endpoint response.
///
/// Unlike the seat inventory routes, the observed socket endpoint returns a
/// top-level JSON array.  Keeping that distinction explicit is important:
/// `{"data":{"list":[]}}` is not a successful socket response, and a
/// business-error object must not become an empty status list.
pub fn parse_socket_status(body: &str) -> Result<LibrarySocketStatuses, LibraryReadParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let body = strip_utf8_bom(body);
        if looks_like_login_html(body) {
            return Err(LibraryReadParseError::LoginHtml);
        }
        if looks_like_html(body) {
            return Err(LibraryReadParseError::UnexpectedHtml);
        }
        if body.trim().is_empty() {
            return Err(LibraryReadParseError::EmptyBody);
        }

        let root: Value =
            serde_json::from_str(body).map_err(|error| LibraryReadParseError::MalformedJson {
                message: error.to_string(),
            })?;
        let values = match root {
            Value::Array(values) => values,
            Value::Object(object) if has_socket_failure_marker(&object) => {
                return Err(LibraryReadParseError::FailureEnvelope);
            }
            _ => {
                return Err(LibraryReadParseError::InvalidCollection {
                    collection: LibraryRecordKind::SocketStatus,
                    expected: "response root to be an array",
                });
            }
        };

        let records = values
            .iter()
            .enumerate()
            .map(|(index, value)| parse_socket_status_record(value, index))
            .collect::<Result<Vec<_>, _>>()?;
        let records = unique_library_records(records, LibraryRecordKind::SocketStatus, |record| {
            record.seat_id
        })?;
        Ok(LibrarySocketStatuses { records })
    })
}

fn parse_socket_status_record(
    value: &Value,
    index: usize,
) -> Result<LibrarySocketStatusRecord, LibraryReadParseError> {
    let object = value.as_object().ok_or_else(|| {
        invalid_record(
            LibraryRecordKind::SocketStatus,
            index,
            "<record>",
            LibraryRecordIssue::NotObject,
        )
    })?;
    let seat_id = required_u64(
        object,
        "seatId",
        LibraryRecordKind::SocketStatus,
        index,
        true,
    )?;
    let status_text = required_text(object, "status", LibraryRecordKind::SocketStatus, index)?;
    let status = match status_text.as_str() {
        "available" => LibrarySocketState::Available,
        "unavailable" => LibrarySocketState::Unavailable,
        "unknown" => LibrarySocketState::Unknown,
        _ => {
            return Err(invalid_record(
                LibraryRecordKind::SocketStatus,
                index,
                "status",
                LibraryRecordIssue::InvalidValue,
            ));
        }
    };

    Ok(LibrarySocketStatusRecord { seat_id, status })
}

fn decode_data_list(body: &str) -> Result<Value, LibraryReadParseError> {
    let body = strip_utf8_bom(body);
    if looks_like_login_html(body) {
        return Err(LibraryReadParseError::LoginHtml);
    }
    if looks_like_html(body) {
        return Err(LibraryReadParseError::UnexpectedHtml);
    }
    if body.trim().is_empty() {
        return Err(LibraryReadParseError::EmptyBody);
    }

    let root: Value =
        serde_json::from_str(body).map_err(|error| LibraryReadParseError::MalformedJson {
            message: error.to_string(),
        })?;
    let root = root.as_object().ok_or(LibraryReadParseError::InvalidRoot)?;
    if has_explicit_failure_marker(root) {
        return Err(LibraryReadParseError::FailureEnvelope);
    }
    let data = root
        .get("data")
        .ok_or(LibraryReadParseError::MissingData { field: "data" })?;
    let data = data
        .as_object()
        .ok_or(LibraryReadParseError::InvalidEnvelope { field: "data" })?;
    let list = data
        .get("list")
        .ok_or(LibraryReadParseError::MissingData { field: "data.list" })?;

    Ok(list.clone())
}

fn has_socket_failure_marker(root: &Map<String, Value>) -> bool {
    has_explicit_failure_marker(root)
        || root.get("error").is_some_and(is_error_marker)
        || root
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(is_failure_text)
}

/// The library service normally exposes only `data.list`. If a deployment or
/// proxy adds a conventional failure marker alongside that field, do not let
/// an empty list turn that failure into a successful empty result. A boolean
/// `result: false` is included because several campus JSON envelopes use
/// `result` instead of `success`; numeric `status: 0` remains untouched
/// because the campus services use zero for success in some legacy envelopes.
fn has_explicit_failure_marker(root: &Map<String, Value>) -> bool {
    if root.get("success").and_then(Value::as_bool) == Some(false)
        || root.get("result").and_then(Value::as_bool) == Some(false)
        || root.get("status").and_then(Value::as_bool) == Some(false)
        || root
            .get("unavailable")
            .is_some_and(is_positive_failure_flag)
    {
        return true;
    }

    for field in [
        "result",
        "status",
        "message",
        "msg",
        "error",
        "errorMessage",
    ] {
        if root
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(is_failure_text)
        {
            return true;
        }
    }

    if root.get("error").is_some_and(is_error_marker) {
        return true;
    }

    if ["code", "status"]
        .iter()
        .filter_map(|field| root.get(*field))
        .filter_map(parse_i64)
        .any(|code| code < 0 || (400..=599).contains(&code))
    {
        return true;
    }

    // Some gateways preserve the service failure envelope below `data` while
    // still adding an empty `list` for compatibility. Inspect only the
    // envelope markers in that nested object; a normal `{list: []}` remains a
    // valid empty result.
    root.get("data")
        .and_then(Value::as_object)
        .is_some_and(has_nested_failure_marker)
}

fn has_nested_failure_marker(data: &Map<String, Value>) -> bool {
    data.get("success").and_then(Value::as_bool) == Some(false)
        || data.get("result").and_then(Value::as_bool) == Some(false)
        || data.get("status").and_then(Value::as_bool) == Some(false)
        || data
            .get("unavailable")
            .is_some_and(is_positive_failure_flag)
        || [
            "result",
            "status",
            "message",
            "msg",
            "error",
            "errorMessage",
        ]
        .iter()
        .filter_map(|field| data.get(*field))
        .filter_map(Value::as_str)
        .any(is_failure_text)
        || data.get("error").is_some_and(is_error_marker)
        || ["code", "status"]
            .iter()
            .filter_map(|field| data.get(*field))
            .filter_map(parse_i64)
            .any(|code| code < 0 || (400..=599).contains(&code))
}

fn is_positive_failure_flag(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().is_some_and(|value| value != 0),
        Value::String(value) => {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || is_failure_text(value)
        }
        _ => false,
    }
}

fn is_error_marker(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().is_none_or(|value| value != 0),
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(_) => true,
    }
}

fn is_failure_text(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    matches!(
        value.as_str(),
        "error"
            | "fail"
            | "failed"
            | "failure"
            | "forbidden"
            | "invalid"
            | "unauthorized"
            | "unavailable"
            | "not found"
            | "timeout"
            | "错误"
            | "失败"
            | "无权限"
            | "不可用"
            | "拒绝"
            | "异常"
            | "超时"
    ) || value.starts_with("error ")
        || value.starts_with("error:")
        || value.starts_with("fail ")
        || value.starts_with("fail:")
        || value.starts_with("failed ")
        || value.starts_with("failed:")
        || value.starts_with("failure ")
        || value.starts_with("failure:")
        || value.starts_with("forbidden ")
        || value.starts_with("unauthorized ")
        || value.starts_with("unavailable ")
        || value.starts_with("timeout ")
        || value.contains("失败")
        || value.contains("错误")
        || value.contains("无权限")
        || value.contains("不可用")
        || value.contains("拒绝")
        || value.contains("异常")
        || value.contains("超时")
        || value.contains("failed")
        || value.contains("unavailable")
        || value.contains("not found")
}

fn parse_area_values(values: &[Value]) -> Result<Vec<LibraryArea>, LibraryReadParseError> {
    let areas = values
        .iter()
        .enumerate()
        .map(|(index, value)| parse_area(value, index))
        .collect::<Result<Vec<_>, _>>()?;
    unique_library_records(areas, LibraryRecordKind::Area, |area| area.id)
}

/// Keep the first identical normalized copy, but never silently choose one
/// of two conflicting records. Keys are scoped by the collection: area IDs
/// are checked among siblings and segment IDs within the same day, not
/// across all dates or hierarchy levels.
fn unique_library_records<T, K>(
    records: Vec<T>,
    collection: LibraryRecordKind,
    key: impl Fn(&T) -> K,
) -> Result<Vec<T>, LibraryReadParseError>
where
    T: PartialEq,
    K: Ord,
{
    let mut positions = std::collections::BTreeMap::new();
    let mut unique: Vec<T> = Vec::new();
    for (index, record) in records.into_iter().enumerate() {
        let id = key(&record);
        if let Some(previous) = positions.get(&id).copied() {
            if unique[previous] != record {
                return Err(LibraryReadParseError::ConflictingIdentifier { collection, index });
            }
        } else {
            positions.insert(id, unique.len());
            unique.push(record);
        }
    }
    Ok(unique)
}

fn parse_area(value: &Value, index: usize) -> Result<LibraryArea, LibraryReadParseError> {
    let object = value.as_object().ok_or_else(|| {
        invalid_record(
            LibraryRecordKind::Area,
            index,
            "<record>",
            LibraryRecordIssue::NotObject,
        )
    })?;

    let id = required_u64(object, "id", LibraryRecordKind::Area, index, true)?;
    let name = required_text(object, "name", LibraryRecordKind::Area, index)?;
    let name_merge = optional_text(object, "nameMerge", LibraryRecordKind::Area, index)?;
    let english_name = optional_text(object, "enname", LibraryRecordKind::Area, index)?;
    let english_name_merge = optional_text(object, "ennameMerge", LibraryRecordKind::Area, index)?;
    let is_valid = optional_bool(object, "isValid", LibraryRecordKind::Area, index)?;
    let total_count = optional_u64(object, "TotalCount", LibraryRecordKind::Area, index)?;
    let unavailable_space =
        optional_u64(object, "UnavailableSpace", LibraryRecordKind::Area, index)?;
    let point_x = optional_f64(object, "point_x2", LibraryRecordKind::Area, index)?;
    let point_y = optional_f64(object, "point_y2", LibraryRecordKind::Area, index)?;

    let available_count = match (total_count, unavailable_space) {
        (Some(total), Some(unavailable)) if unavailable <= total => Some(total - unavailable),
        (Some(_), Some(_)) => {
            return Err(invalid_record(
                LibraryRecordKind::Area,
                index,
                "UnavailableSpace",
                LibraryRecordIssue::InvalidValue,
            ));
        }
        _ => None,
    };

    let child_areas = match object.get("childArea") {
        None => Vec::new(),
        Some(Value::Array(values)) => parse_area_values(values)?,
        Some(_) => {
            return Err(invalid_record(
                LibraryRecordKind::Area,
                index,
                "childArea",
                LibraryRecordIssue::WrongType { expected: "array" },
            ));
        }
    };

    Ok(LibraryArea {
        id,
        name,
        name_merge,
        english_name,
        english_name_merge,
        is_valid,
        total_count,
        unavailable_space,
        available_count,
        point_x,
        point_y,
        child_areas,
    })
}

fn parse_day_segment(
    value: &Value,
    index: usize,
) -> Result<LibraryDaySegment, LibraryReadParseError> {
    let object = value.as_object().ok_or_else(|| {
        invalid_record(
            LibraryRecordKind::DaySegment,
            index,
            "<record>",
            LibraryRecordIssue::NotObject,
        )
    })?;

    let day_value = required_value(object, "day", LibraryRecordKind::DaySegment, index)?;
    let day = required_day(day_value, LibraryRecordKind::DaySegment, index, "day")?;
    let start_time = required_time(
        object,
        "startTime",
        LibraryRecordKind::DaySegment,
        index,
        &day,
    )?;
    let end_time = required_time(
        object,
        "endTime",
        LibraryRecordKind::DaySegment,
        index,
        &day,
    )?;
    if start_time >= end_time {
        return Err(invalid_record(
            LibraryRecordKind::DaySegment,
            index,
            "endTime",
            LibraryRecordIssue::InvalidValue,
        ));
    }
    let id = required_u64(object, "id", LibraryRecordKind::DaySegment, index, true)?;

    Ok(LibraryDaySegment {
        day,
        start_time,
        end_time,
        id,
    })
}

fn parse_seat(value: &Value, index: usize) -> Result<LibrarySeat, LibraryReadParseError> {
    let object = value.as_object().ok_or_else(|| {
        invalid_record(
            LibraryRecordKind::Seat,
            index,
            "<record>",
            LibraryRecordIssue::NotObject,
        )
    })?;

    let id = required_u64(object, "id", LibraryRecordKind::Seat, index, true)?;
    let name = required_text(object, "name", LibraryRecordKind::Seat, index)?;
    let status = required_i64(object, "status", LibraryRecordKind::Seat, index)?;
    let area_type = required_i64(object, "area_type", LibraryRecordKind::Seat, index)?;

    Ok(LibrarySeat {
        id,
        name,
        status,
        area_type,
    })
}

fn required_value<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<&'a Value, LibraryReadParseError> {
    object
        .get(field)
        .ok_or_else(|| invalid_record(collection, index, field, LibraryRecordIssue::MissingField))
}

fn required_text(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<String, LibraryReadParseError> {
    let value = required_value(object, field, collection, index)?;
    let value = value.as_str().ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType { expected: "string" },
        )
    })?;
    normalize_required_text(value)
        .ok_or_else(|| invalid_record(collection, index, field, LibraryRecordIssue::EmptyValue))
}

fn optional_text(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<Option<String>, LibraryReadParseError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value.as_str().ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType { expected: "string" },
        )
    })?;
    Ok(normalize_required_text(value))
}

fn required_u64(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
    require_nonzero: bool,
) -> Result<u64, LibraryReadParseError> {
    let value = required_value(object, field, collection, index)?;
    let parsed = parse_u64(value).ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType {
                expected: "non-negative integer",
            },
        )
    })?;
    if require_nonzero && parsed == 0 {
        return Err(invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::InvalidValue,
        ));
    }
    Ok(parsed)
}

fn required_i64(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<i64, LibraryReadParseError> {
    let value = required_value(object, field, collection, index)?;
    parse_i64(value).ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType {
                expected: "integer",
            },
        )
    })
}

fn optional_u64(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<Option<u64>, LibraryReadParseError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_u64(value).map(Some).ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType {
                expected: "non-negative integer",
            },
        )
    })
}

fn optional_f64(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<Option<f64>, LibraryReadParseError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_f64(value).map(Some).ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType { expected: "number" },
        )
    })
}

fn optional_bool(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
) -> Result<Option<bool>, LibraryReadParseError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_bool(value).map(Some).ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType {
                expected: "boolean or 0/1",
            },
        )
    })
}

fn required_day(
    value: &Value,
    collection: LibraryRecordKind,
    index: usize,
    field: &'static str,
) -> Result<String, LibraryReadParseError> {
    let value = value.as_str().ok_or_else(|| {
        invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::WrongType {
                expected: "YYYY-MM-DD string",
            },
        )
    })?;
    normalize_day(value)
        .ok_or_else(|| invalid_record(collection, index, field, LibraryRecordIssue::InvalidValue))
}

fn required_time(
    object: &Map<String, Value>,
    field: &'static str,
    collection: LibraryRecordKind,
    index: usize,
    expected_day: &str,
) -> Result<String, LibraryReadParseError> {
    let value = required_value(object, field, collection, index)?;
    let raw = match value {
        Value::String(value) => value.as_str(),
        Value::Object(object) => {
            let date = object.get("date").ok_or_else(|| {
                invalid_record(
                    collection,
                    index,
                    format!("{field}.date"),
                    LibraryRecordIssue::MissingField,
                )
            })?;
            date.as_str().ok_or_else(|| {
                invalid_record(
                    collection,
                    index,
                    format!("{field}.date"),
                    LibraryRecordIssue::WrongType { expected: "string" },
                )
            })?
        }
        _ => {
            return Err(invalid_record(
                collection,
                index,
                field,
                LibraryRecordIssue::WrongType {
                    expected: "string or object with date",
                },
            ));
        }
    };

    let raw = raw.trim();
    if value.is_string()
        && matches!(raw.as_bytes().get(10), Some(b' ' | b'T'))
        && raw.get(..10) != Some(expected_day)
    {
        return Err(invalid_record(
            collection,
            index,
            field,
            LibraryRecordIssue::InvalidValue,
        ));
    }
    let candidate = normalize_wire_time(raw);
    normalize_time(candidate.as_deref().unwrap_or_default())
        .ok_or_else(|| invalid_record(collection, index, field, LibraryRecordIssue::InvalidValue))
}

fn parse_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn parse_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn parse_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64().filter(|value| value.is_finite()),
        Value::String(value) => value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite()),
        _ => None,
    }
}

fn parse_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(number) => match number.as_i64()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" => Some(false),
            "1" | "true" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

fn normalize_required_text(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_day_for_request(value: &str) -> Result<String, LibraryRequestError> {
    normalize_day(value).ok_or_else(|| LibraryRequestError::InvalidDay {
        value: value.trim().to_owned(),
    })
}

fn normalize_time_for_request(
    value: &str,
    field: &'static str,
) -> Result<String, LibraryRequestError> {
    normalize_time(value).ok_or_else(|| LibraryRequestError::InvalidTime {
        field,
        value: value.trim().to_owned(),
    })
}

fn normalize_day(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() != 10 || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return None;
    }

    let year = value[0..4].parse::<u32>().ok()?;
    let month = value[5..7].parse::<u32>().ok()?;
    let day = value[8..10].parse::<u32>().ok()?;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => return None,
    };
    (day > 0 && day <= days_in_month).then_some(value.to_owned())
}

fn normalize_time(value: &str) -> Option<String> {
    let value = value.trim();
    if !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    let valid_shape = (value.len() == 5
        && bytes[2] == b':'
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[3..].iter().all(u8::is_ascii_digit))
        || (value.len() == 8
            && bytes[2] == b':'
            && bytes[5] == b':'
            && bytes[..2].iter().all(u8::is_ascii_digit)
            && bytes[3..5].iter().all(u8::is_ascii_digit)
            && bytes[6..].iter().all(u8::is_ascii_digit));
    if !valid_shape {
        return None;
    }

    let hour = value[0..2].parse::<u32>().ok()?;
    let minute = value[3..5].parse::<u32>().ok()?;
    let second = if value.len() == 8 {
        value[6..8].parse::<u32>().ok()?
    } else {
        0
    };
    if hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }

    Some(value[0..5].to_owned())
}

fn normalize_wire_time(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() >= 16 && value.is_ascii() && normalize_day(&value[..10]).is_some() {
        let separator = value.as_bytes()[10];
        if matches!(separator, b' ' | b'T') {
            // PHP's date object carries a complete naive datetime, often
            // with six fractional digits. Validate all of it before reducing
            // to the minute-level selector; truncation must not hide invalid
            // seconds, an unsupported offset, or trailing garbage.
            let suffix = &value[11..];
            let time = if let Some((time, fraction)) = suffix.split_once('.') {
                if time.len() != 8
                    || fraction.is_empty()
                    || fraction.len() > 9
                    || !fraction.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                time
            } else {
                suffix
            };
            return normalize_time(time);
        }
    }
    Some(value.to_owned())
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn checked_identifier(value: u64, field: &'static str) -> Result<u64, LibraryRequestError> {
    (value > 0)
        .then_some(value)
        .ok_or(LibraryRequestError::InvalidIdentifier { field })
}

fn invalid_record(
    collection: LibraryRecordKind,
    index: usize,
    field: impl Into<String>,
    issue: LibraryRecordIssue,
) -> LibraryReadParseError {
    LibraryReadParseError::InvalidRecord {
        collection,
        index,
        field: field.into(),
        issue,
    }
}

fn is_library_login_response(response: &CampusTextResponse) -> bool {
    let body_signal = looks_like_login_html(&response.body);
    let url_signal = looks_like_login_url(&response.final_url);

    body_signal || url_signal
}

fn looks_like_login_location(base_url: &Url, location: &str) -> bool {
    let Ok(url) = Url::parse(location).or_else(|_| base_url.join(location)) else {
        return false;
    };
    looks_like_login_url(&url)
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn redirect_leaves_origin(base_url: &Url, response_url: &Url, location: &str) -> bool {
    let Some(candidate) = resolve_location(response_url, location) else {
        return true;
    };
    !same_origin(base_url, &candidate)
}

fn resolve_location(response_url: &Url, location: &str) -> Option<Url> {
    if invalid_percent_encoding(location) {
        return None;
    }
    Url::parse(location)
        .or_else(|_| response_url.join(location))
        .ok()
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

fn path_within_base(base_url: &Url, candidate: &Url) -> bool {
    let base_path = base_url.path().trim_end_matches('/');
    base_path.is_empty()
        || base_path == "/"
        || candidate.path() == base_path
        || candidate.path().starts_with(&format!("{base_path}/"))
}

fn query_matches(expected: &[(String, String)], candidate: &Url) -> bool {
    let mut expected = expected
        .iter()
        .map(|(key, value)| (key.as_str().to_owned(), value.as_str().to_owned()))
        .collect::<Vec<_>>();
    let mut actual = candidate
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    actual.sort_unstable();
    expected == actual
}

fn looks_like_login_html(body: &str) -> bool {
    let body = strip_utf8_bom(body);
    let lower = body.trim_start().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }

    // These markers are specific enough to identify the identity/WebVPN
    // form or an explicit expired-session response. Do not treat every HTML
    // document containing the word "auth" as a login page.
    let identity_form = (contains_any(
        &lower,
        &[
            "name=\"i_user\"",
            "name='i_user'",
            "id=\"i_user\"",
            "id='i_user'",
        ],
    )) && contains_any(
        &lower,
        &[
            "name=\"i_pass\"",
            "name='i_pass'",
            "id=\"i_pass\"",
            "id='i_pass'",
        ],
    );
    let explicit_session_marker = contains_any(
        &lower,
        &[
            "登录失效",
            "会话已过期",
            "请先登录",
            "重新登录",
            "session expired",
            "login required",
            "authentication required",
        ],
    );
    let login_route_marker = lower.contains("/do/off/ui/auth/login");
    let html = looks_like_html(body);
    let form_or_input = lower.contains("<form") || lower.contains("<input");
    let login_word = contains_any(&lower, &["login", "登录", "统一身份认证"]);
    let title_login = lower.contains("<title") && login_word;
    let webvpn_form = lower.contains("webvpn") && form_or_input;

    if !html {
        return explicit_session_marker || login_route_marker;
    }

    identity_form
        || explicit_session_marker
        || login_route_marker
        || title_login
        || webvpn_form
        || (form_or_input && login_word)
}

fn looks_like_login_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path == "/login"
        || path.ends_with("/login")
        || path.contains("/login/")
        || path.contains("/do/off/ui/auth/login")
}

fn contains_any(value: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}

fn strip_utf8_bom(body: &str) -> &str {
    body.strip_prefix('\u{feff}').unwrap_or(body)
}

fn looks_like_html(body: &str) -> bool {
    let lower = strip_utf8_bom(body).trim_start().to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.starts_with("<head")
        || lower.starts_with("<body")
        || lower.starts_with("<form")
        || lower.starts_with("<input")
        || lower.starts_with("<meta")
        || lower.starts_with("<title")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED_AREAS: &str = r#"
    {
      "data": {
        "list": [
          {
            "id": 35,
            "name": "  北馆   ",
            "nameMerge": " 北馆 ",
            "enname": " Main   Library ",
            "ennameMerge": "Main Library",
            "isValid": 1,
            "childArea": [
              {
                "id": "351",
                "name": "  一层 ",
                "enname": "First Floor",
                "isValid": true,
                "TotalCount": "120",
                "UnavailableSpace": 20,
                "point_x2": "1.5",
                "point_y2": 2.5,
                "childArea": [
                  {
                    "id": 3511,
                    "name": " 东区 ",
                    "isValid": 1,
                    "TotalCount": 50,
                    "UnavailableSpace": 8
                  }
                ]
              }
            ]
          }
        ]
      }
    }
    "#;

    const EMPTY_LISTS: &str = r#"{"data":{"list":[]}}"#;

    const DAY_SEGMENTS: &str = r#"
    {
      "data": {
        "list": [
          {
            "day": " 2026-09-11 ",
            "startTime": {"date": "2026-09-11 08:00:00"},
            "endTime": {"date": "2026-09-11T22:00:00"},
            "id": "9001"
          }
        ]
      }
    }
    "#;

    const SEATS: &str = r#"
    {
      "data": {
        "list": [
          {"id": " 701 ", "name": "  A 01 ", "status": 1, "area_type": "2"}
        ]
      }
    }
    "#;

    #[test]
    fn builds_relative_get_plans_with_explicit_session_prerequisite() {
        // Cross-checked against thu-info-community/thu-info-lib main: the
        // INFO/WebVPN target is seat.lib.tsinghua.cn and the read sequence is
        // tree -> dated areas -> areadays -> spaces_old.
        assert_eq!(LIBRARY_WEBVPN_TARGET_HOST, "seat.lib.tsinghua.cn");
        assert_eq!(LIBRARY_HOME_PATH, "/home/web/f_second");
        let profile = LibraryReadProfile::new();
        let area = profile.area_tree_request();
        assert_eq!(area.operation, LibraryReadOperation::AreaTree);
        assert_eq!(area.method, LibraryReadMethod::Get);
        assert_eq!(area.path, LIBRARY_AREA_TREE_PATH);
        assert!(area.query.is_empty());
        assert_eq!(
            area.session_prerequisite,
            LibrarySessionPrerequisite::ExistingInfoWebVpnSession
        );
        assert!(area.is_relative_path());

        let seats = profile
            .seat_availability_request(351, 9001, " 2026-09-11 ", "08:00:00", "22:00")
            .expect("seat request");
        assert_eq!(seats.path, LIBRARY_SEAT_AVAILABILITY_PATH);
        assert_eq!(
            seats.query,
            vec![
                ("area".to_owned(), "351".to_owned()),
                ("segment".to_owned(), "9001".to_owned()),
                ("day".to_owned(), "2026-09-11".to_owned()),
                ("startTime".to_owned(), "08:00".to_owned()),
                ("endTime".to_owned(), "22:00".to_owned()),
            ]
        );
        assert!(seats.is_relative_path());
        assert!(!format!("{seats:?}").contains("://"));
    }

    #[test]
    fn rejects_request_identifiers_and_invalid_dates_without_accepting_urls() {
        let profile = LibraryReadProfile::new();
        assert!(matches!(
            profile.area_children_request(0),
            Err(LibraryRequestError::InvalidIdentifier { field: "area_id" })
        ));
        assert!(matches!(
            profile.area_for_day_request(35, "2026-02-30"),
            Err(LibraryRequestError::InvalidDay { .. })
        ));
        assert!(matches!(
            profile.seat_availability_request(35, 9, "2026-09-11", "8:00", "22:00"),
            Err(LibraryRequestError::InvalidTime {
                field: "start_time",
                ..
            })
        ));
        assert_eq!(
            profile.seat_availability_request(35, 9, "2026-09-11", "22:00", "08:00"),
            Err(LibraryRequestError::InvalidTimeRange)
        );
        assert_eq!(
            profile.seat_availability_request(35, 9, "2026-09-11", "08:00", "08:00"),
            Err(LibraryRequestError::InvalidTimeRange)
        );
    }

    #[test]
    fn parses_nested_area_tree_and_normalizes_wire_scalars() {
        let parsed = parse_area_tree(NESTED_AREAS).expect("area fixture parses");
        assert_eq!(parsed.areas.len(), 1);
        let library = &parsed.areas[0];
        assert_eq!(library.id, 35);
        assert_eq!(library.name, "北馆");
        assert_eq!(library.name_merge.as_deref(), Some("北馆"));
        assert_eq!(library.english_name.as_deref(), Some("Main Library"));
        assert_eq!(library.is_valid, Some(true));
        assert_eq!(library.child_areas.len(), 1);

        let floor = &library.child_areas[0];
        assert_eq!(floor.id, 351);
        assert_eq!(floor.name, "一层");
        assert_eq!(floor.total_count, Some(120));
        assert_eq!(floor.unavailable_space, Some(20));
        assert_eq!(floor.available_count, Some(100));
        assert_eq!(floor.point_x, Some(1.5));
        assert_eq!(floor.child_areas[0].name, "东区");
        assert_eq!(floor.child_areas[0].available_count, Some(42));
    }

    #[test]
    fn parses_route_specific_area_wrappers_and_accepts_empty_valid_lists() {
        let child_wrapper = r#"{
          "data": {
            "list": {
              "childArea": [
                {"id": 90, "name": "一层", "isValid": 1}
              ]
            }
          }
        }"#;
        let parsed = parse_area_children(child_wrapper).expect("child wrapper parses");
        assert_eq!(parsed.areas[0].id, 90);
        assert_eq!(
            parse_area_for_day(child_wrapper)
                .expect("date section wrapper parses")
                .areas[0]
                .id,
            90
        );
        assert!(matches!(
            parse_area_tree(child_wrapper),
            Err(LibraryReadParseError::InvalidCollection {
                collection: LibraryRecordKind::Area,
                ..
            })
        ));
        assert!(matches!(
            parse_area_children(EMPTY_LISTS),
            Err(LibraryReadParseError::InvalidCollection {
                collection: LibraryRecordKind::Area,
                ..
            })
        ));

        assert!(
            parse_area_tree(EMPTY_LISTS)
                .expect("empty area list parses")
                .areas
                .is_empty()
        );
        assert!(
            parse_day_segments(EMPTY_LISTS)
                .expect("empty day list parses")
                .segments
                .is_empty()
        );
        assert!(
            parse_seat_availability(EMPTY_LISTS)
                .expect("empty seat list parses")
                .seats
                .is_empty()
        );
    }

    #[test]
    fn parses_day_segments_and_normalizes_datetime_objects() {
        let parsed = parse_day_segments(DAY_SEGMENTS).expect("day fixture parses");
        assert_eq!(
            parsed.segments,
            vec![LibraryDaySegment {
                day: "2026-09-11".to_owned(),
                start_time: "08:00".to_owned(),
                end_time: "22:00".to_owned(),
                id: 9001,
            }]
        );
    }

    #[test]
    fn parses_seat_records_with_opaque_status_and_area_type() {
        let parsed = parse_seat_availability(SEATS).expect("seat fixture parses");
        assert_eq!(
            parsed.seats,
            vec![LibrarySeat {
                id: 701,
                name: "A 01".to_owned(),
                status: 1,
                area_type: 2,
            }]
        );
    }

    #[test]
    fn distinguishes_login_html_malformed_json_invalid_root_and_missing_data() {
        assert!(matches!(
            parse_area_tree("<!doctype html><html><title>WebVPN login</title></html>"),
            Err(LibraryReadParseError::LoginHtml)
        ));
        assert!(matches!(
            parse_area_tree("<form><input name='i_user'><input name='i_pass'></form>"),
            Err(LibraryReadParseError::LoginHtml)
        ));
        assert!(matches!(
            parse_area_tree("login required"),
            Err(LibraryReadParseError::LoginHtml)
        ));
        assert!(matches!(
            parse_area_tree("<!doctype html><html><title>maintenance</title></html>"),
            Err(LibraryReadParseError::UnexpectedHtml)
        ));
        assert!(matches!(
            parse_area_tree("\u{feff}   "),
            Err(LibraryReadParseError::EmptyBody)
        ));
        assert_eq!(
            parse_area_tree("\u{feff}{\"data\":{\"list\":[{\"id\":1,\"name\":\"A\"}]}}")
                .expect("BOM JSON parses")
                .areas[0]
                .id,
            1
        );
        assert!(matches!(
            parse_area_tree("{"),
            Err(LibraryReadParseError::MalformedJson { .. })
        ));
        assert!(matches!(
            parse_area_tree("[]"),
            Err(LibraryReadParseError::InvalidRoot)
        ));
        assert!(matches!(
            parse_area_tree(r#"{"data":{}}"#),
            Err(LibraryReadParseError::MissingData { field: "data.list" })
        ));
        assert!(matches!(
            parse_area_tree(r#"{"success":false,"data":{"list":[]}}"#),
            Err(LibraryReadParseError::FailureEnvelope)
        ));
        assert!(matches!(
            parse_area_tree(r#"{"result":false,"data":{"list":[]}}"#),
            Err(LibraryReadParseError::FailureEnvelope)
        ));
        assert!(matches!(
            parse_area_tree(r#"{"code":503,"data":{"list":[]}}"#),
            Err(LibraryReadParseError::FailureEnvelope)
        ));
    }

    #[test]
    fn distinguishes_invalid_collections_and_records() {
        assert!(matches!(
            parse_day_segments(r#"{"data":{"list":{}}}"#),
            Err(LibraryReadParseError::InvalidCollection {
                collection: LibraryRecordKind::DaySegment,
                ..
            })
        ));

        let invalid_area = r#"{"data":{"list":[{"id":1,"name":"A","childArea":{}}]}}"#;
        assert!(matches!(
            parse_area_tree(invalid_area),
            Err(LibraryReadParseError::InvalidRecord {
                collection: LibraryRecordKind::Area,
                field,
                issue: LibraryRecordIssue::WrongType { .. },
                ..
            }) if field == "childArea"
        ));

        let invalid_seat = r#"{"data":{"list":[{"id":1,"name":"Seat","status":1}]}}"#;
        assert!(matches!(
            parse_seat_availability(invalid_seat),
            Err(LibraryReadParseError::InvalidRecord {
                collection: LibraryRecordKind::Seat,
                field,
                issue: LibraryRecordIssue::MissingField,
                ..
            }) if field == "area_type"
        ));

        let invalid_day = r#"{"data":{"list":[{"day":"2026-02-30","startTime":{"date":"2026-02-30 08:00:00"},"endTime":{"date":"2026-02-30 22:00:00"},"id":1}]}}"#;
        assert!(matches!(
            parse_day_segments(invalid_day),
            Err(LibraryReadParseError::InvalidRecord {
                collection: LibraryRecordKind::DaySegment,
                field,
                issue: LibraryRecordIssue::InvalidValue,
                ..
            }) if field == "day"
        ));

        let invalid_range = r#"{"data":{"list":[{"day":"2026-09-11","startTime":{"date":"2026-09-11 22:00:00"},"endTime":{"date":"2026-09-11 08:00:00"},"id":1}]}}"#;
        assert!(matches!(
            parse_day_segments(invalid_range),
            Err(LibraryReadParseError::InvalidRecord {
                collection: LibraryRecordKind::DaySegment,
                field,
                issue: LibraryRecordIssue::InvalidValue,
                ..
            }) if field == "endTime"
        ));
    }

    #[test]
    fn does_not_retain_response_body_or_sensitive_transport_state_in_debug() {
        let error = parse_area_tree("<html>login-secret-cookie-value</html>").unwrap_err();
        let debug = format!("{error:?}");
        assert!(!debug.contains("login-secret-cookie-value"));
        assert!(!format!("{:?}", LibraryReadProfile::new()).contains("http"));

        let transport = LibraryAdapterError::Transport(TransportError::InvalidUrl(
            "https://webvpn.example/opaque-secret".to_owned(),
        ));
        assert_eq!(transport.to_string(), "library transport failed");
        assert!(!format!("{transport:?}").contains("opaque-secret"));
        assert!(LibraryAdapterError::Parse(LibraryReadParseError::LoginHtml).is_session_expired());
    }

    #[test]
    fn classifies_final_login_redirects_using_response_metadata() {
        let login = CampusTextResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://webvpn.example.test/do/off/ui/auth/login")
                .expect("login URL"),
            content_type: Some("text/html; charset=utf-8".to_owned()),
            redirect_location: None,
            body: "<html><body>please authenticate</body></html>".to_owned(),
        };
        assert!(is_library_login_response(&login));

        let json = CampusTextResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://webvpn.example.test/api.php/areas/1/tree/1")
                .expect("API URL"),
            content_type: Some("application/json".to_owned()),
            redirect_location: None,
            body: "{\"data\":{\"list\":[]}}".to_owned(),
        };
        assert!(!is_library_login_response(&json));

        let login_json = CampusTextResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://identity.example.test/login").expect("login URL"),
            content_type: Some("application/json".to_owned()),
            redirect_location: None,
            body: "{\"error\":\"login required\"}".to_owned(),
        };
        assert!(is_library_login_response(&login_json));
    }

    #[test]
    fn classifies_non_login_cross_origin_locations_as_origin_failures() {
        let base = Url::parse("https://webvpn.example.test/https/opaque/").expect("base URL");
        let response = Url::parse("https://webvpn.example.test/https/opaque/api.php").expect("URL");
        assert!(redirect_leaves_origin(
            &base,
            &response,
            "https://unexpected.example.test/landing"
        ));
        assert!(!redirect_leaves_origin(&base, &response, "/login"));
        assert_eq!(
            LibraryAdapterError::UnexpectedOrigin.to_string(),
            "library response came from an unexpected origin"
        );
    }

    #[test]
    fn final_url_query_must_match_the_read_plan_exactly() {
        let profile = LibraryReadProfile::new();
        let plan = profile
            .seat_availability_request(351, 9001, "2026-09-11", "08:00", "22:00")
            .expect("seat request");
        let matching = Url::parse(
            "https://webvpn.example.test/opaque/api.php/spaces_old?endTime=22%3A00&area=351&segment=9001&day=2026-09-11&startTime=08%3A00",
        )
        .expect("matching URL");
        assert!(query_matches(plan.query_parameters(), &matching));

        let extra_parameter = Url::parse(
            "https://webvpn.example.test/opaque/api.php/spaces_old?area=351&segment=9001&day=2026-09-11&startTime=08%3A00&endTime=22%3A00&unexpected=1",
        )
        .expect("extra parameter URL");
        assert!(!query_matches(plan.query_parameters(), &extra_parameter));

        let missing_parameter = Url::parse(
            "https://webvpn.example.test/opaque/api.php/spaces_old?area=351&segment=9001&day=2026-09-11&startTime=08%3A00",
        )
        .expect("missing parameter URL");
        assert!(!query_matches(plan.query_parameters(), &missing_parameter));
    }

    #[tokio::test]
    async fn adapter_rejects_non_ok_status_even_when_the_body_looks_successful() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).expect("request bytes");
            let body = r#"{"data":{"list":[]}}"#;
            let response = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response");
        });

        let config = LibraryAdapterConfig::with_user_agent_and_timeout(
            &format!("http://{address}/https/opaque/"),
            "THYou/library-status-test",
            Duration::from_secs(5),
        )
        .expect("config");
        let adapter = LibraryReadAdapter::new(config).expect("adapter");
        let error = adapter
            .read_area_tree()
            .await
            .expect_err("201 is outside the GET contract");
        assert!(matches!(
            error,
            LibraryAdapterError::HttpStatus {
                status: StatusCode::CREATED
            }
        ));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn adapter_reuses_transport_and_executes_the_cross_checked_read_routes() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let responses = [
                r#"{"data":{"list":[{"id":35,"name":"北馆","isValid":1}]}}"#,
                r#"{"data":{"list":{"childArea":[{"id":351,"name":"一层","isValid":1}]}}}"#,
                r#"{"data":{"list":{"childArea":[{"id":351,"name":"一层","TotalCount":100,"UnavailableSpace":20}]}}}"#,
                r#"{"data":{"list":[{"day":"2026-09-11","startTime":{"date":"2026-09-11 08:00:00"},"endTime":{"date":"2026-09-11 22:00:00"},"id":9001}]}}"#,
                r#"{"data":{"list":[{"id":701,"name":"A 01","status":1,"area_type":2}]}}"#,
            ];
            let mut requests = Vec::new();
            for (index, body) in responses.into_iter().enumerate() {
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
                requests.push(String::from_utf8_lossy(&request).into_owned());
                let cookie = if index == 0 {
                    "Set-Cookie: info-session=shared; Path=/\r\nSet-Cookie: library-session=fixture; Path=/\r\n"
                } else {
                    ""
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{cookie}Connection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
            requests
        });

        let base =
            Url::parse(&format!("http://{address}/https/opaque-mapping/")).expect("base URL");
        let transport =
            CampusHttpTransport::with_timeout("THYou/library-test", Duration::from_secs(5))
                .expect("transport");
        let adapter = LibraryReadAdapter::with_transport(base, transport);

        let tree = adapter.read_area_tree().await.expect("area tree");
        assert_eq!(tree.areas[0].id, 35);
        assert!(tree.areas[0].is_valid.expect("validity"));

        let children = adapter.read_area_children(35).await.expect("children");
        assert_eq!(children.areas[0].id, 351);

        let dated = adapter
            .read_area_for_day(35, "2026-09-11")
            .await
            .expect("dated areas");
        assert_eq!(dated.areas[0].available_count, Some(80));

        let segments = adapter.read_day_segments(351).await.expect("segments");
        assert_eq!(segments.segments[0].id, 9001);

        let seats = adapter
            .read_seat_availability(351, 9001, "2026-09-11", "08:00", "22:00")
            .await
            .expect("seats");
        assert!(seats.seats[0].is_valid);

        let requests = server.join().expect("server");
        assert!(
            requests[0].starts_with("GET /https/opaque-mapping/api.php/areas/1/tree/1 HTTP/1.1")
        );
        assert!(requests[1].starts_with("GET /https/opaque-mapping/api.php/areas/35 "));
        assert!(
            requests[2].starts_with("GET /https/opaque-mapping/api.php/areas/35/date/2026-09-11 ")
        );
        assert!(requests[3].starts_with("GET /https/opaque-mapping/api.php/areadays/351 "));
        assert!(requests[4].starts_with("GET /https/opaque-mapping/api.php/spaces_old?"));
        for request in &requests {
            let lower = request.to_ascii_lowercase();
            assert!(lower.starts_with("get "));
            assert!(lower.contains("user-agent: thyou/library-test"));
        }
        for request in requests.iter().skip(1) {
            assert!(request.to_ascii_lowercase().contains("info-session=shared"));
        }
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("library-session=fixture")
        );
        assert!(requests[4].contains("area=351"));
        assert!(requests[4].contains("segment=9001"));
        assert!(requests[4].contains("day=2026-09-11"));
        assert!(requests[4].contains("startTime=08%3A00"));
        assert!(requests[4].contains("endTime=22%3A00"));
    }

    #[tokio::test]
    async fn adapter_treats_empty_lists_as_successful_empty_results() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request).expect("request");
                let body = r#"{"data":{"list":[]}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
        });

        let base = Url::parse(&format!("http://{address}/https/opaque-mapping/")).expect("base");
        let transport =
            CampusHttpTransport::with_timeout("THYou/library-empty-test", Duration::from_secs(5))
                .expect("transport");
        let adapter = LibraryReadAdapter::try_with_transport(base, transport).expect("adapter");

        assert!(
            adapter
                .read_area_tree()
                .await
                .expect("empty area result")
                .areas
                .is_empty()
        );
        assert!(
            adapter
                .read_day_segments(351)
                .await
                .expect("empty segment result")
                .segments
                .is_empty()
        );
        assert!(
            adapter
                .read_seat_availability(351, 9001, "2026-09-11", "08:00", "22:00")
                .await
                .expect("empty seat result")
                .seats
                .is_empty()
        );

        server.join().expect("server");
    }

    #[test]
    fn json_embedded_html_text_is_not_classified_as_a_document() {
        let body = r#"{"data":{"list":[{"id":1,"name":"<html> reading room"}]}}"#;
        let tree = parse_area_tree(body).expect("JSON record with text markup");
        assert_eq!(tree.areas[0].name, "<html> reading room");
    }

    #[tokio::test]
    async fn adapter_rejects_a_valid_library_envelope_from_an_unrelated_mapped_route() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut redirect, _) = listener.accept().expect("area request");
            let mut request = [0_u8; 2048];
            let _ = redirect.read(&mut request).expect("request");
            redirect
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /https/opaque/unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect");

            let (mut unrelated, _) = listener.accept().expect("unrelated route");
            let _ = unrelated.read(&mut request).expect("redirected request");
            let body = r#"{"data":{"list":[{"id":35,"name":"北馆"}]}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            unrelated.write_all(response.as_bytes()).expect("response");
        });

        let config = LibraryAdapterConfig::with_user_agent_and_timeout(
            &format!("http://{address}/https/opaque/"),
            "THYou/library-route-proof-test",
            Duration::from_secs(5),
        )
        .expect("config");
        let adapter = LibraryReadAdapter::new(config).expect("adapter");
        assert!(matches!(
            adapter.read_area_tree().await,
            Err(LibraryAdapterError::UnexpectedPath)
        ));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn adapter_classifies_a_same_origin_redirect_to_the_login_page() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut redirect_stream, _) = listener.accept().expect("redirect connection");
            let mut redirect_request = [0_u8; 2048];
            let _ = redirect_stream
                .read(&mut redirect_request)
                .expect("redirect request");
            redirect_stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /do/off/ui/auth/login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut login_stream, _) = listener.accept().expect("login connection");
            let mut login_request = [0_u8; 2048];
            let _ = login_stream
                .read(&mut login_request)
                .expect("login request");
            let body = "<html><title>Campus login</title></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            login_stream
                .write_all(response.as_bytes())
                .expect("login response");
        });

        let config = LibraryAdapterConfig::with_user_agent_and_timeout(
            &format!("http://{address}/https/opaque/"),
            "THYou/library-redirect-test",
            Duration::from_secs(5),
        )
        .expect("config");
        let adapter = LibraryReadAdapter::new(config).expect("adapter");
        let error = adapter
            .read_area_tree()
            .await
            .expect_err("redirected login");
        assert!(matches!(error, LibraryAdapterError::SessionExpired));

        server.join().expect("server");
    }

    #[tokio::test]
    async fn adapter_classifies_a_blocked_cross_origin_login_redirect() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("redirect connection");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).expect("request");
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: https://identity.example.test/do/off/ui/auth/login\r\n",
                "Content-Length: 0\r\n",
                "Connection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).expect("response");
        });

        let config = LibraryAdapterConfig::with_user_agent_and_timeout(
            &format!("http://{address}/https/opaque/"),
            "THYou/library-cross-origin-login-test",
            Duration::from_secs(5),
        )
        .expect("config");
        let adapter = LibraryReadAdapter::new(config).expect("adapter");
        let error = adapter
            .read_area_tree()
            .await
            .expect_err("blocked login redirect");
        assert!(matches!(error, LibraryAdapterError::SessionExpired));

        server.join().expect("server");
    }

    #[tokio::test]
    async fn adapter_rejects_valid_json_after_leaving_the_opaque_mapping() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut redirect, _) = listener.accept().expect("redirect connection");
            let mut request = [0_u8; 2048];
            let _ = redirect.read(&mut request).expect("redirect request");
            redirect
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut unrelated, _) = listener.accept().expect("unrelated connection");
            let _ = unrelated.read(&mut request).expect("unrelated request");
            let body = r#"{"data":{"list":[]}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            unrelated
                .write_all(response.as_bytes())
                .expect("unrelated response");
        });

        let config = LibraryAdapterConfig::with_user_agent_and_timeout(
            &format!("http://{address}/https/opaque/"),
            "THYou/library-path-test",
            Duration::from_secs(5),
        )
        .expect("config");
        let adapter = LibraryReadAdapter::new(config).expect("adapter");
        let error = adapter
            .read_area_tree()
            .await
            .expect_err("unrelated same-origin response");
        assert!(matches!(error, LibraryAdapterError::UnexpectedPath));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn adapter_classifies_login_html_and_http_errors_without_retaining_bodies() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for (status, content_type, body) in [
                (
                    "200 OK",
                    "text/html; charset=utf-8",
                    "<html><form><input name=\"i_user\"><input name=\"i_pass\"></form></html>",
                ),
                (
                    "401 Unauthorized",
                    "text/html; charset=utf-8",
                    "<html><title>login</title><p>session-secret-body</p></html>",
                ),
                ("403 Forbidden", "text/plain", "forbidden-session-body"),
                (
                    "503 Service Unavailable",
                    "text/plain",
                    "upstream-secret-body",
                ),
                (
                    "200 OK",
                    "text/html; charset=utf-8",
                    "<!doctype html><html><title>maintenance</title><p>maintenance-secret-body</p></html>",
                ),
                ("200 OK", "application/json", "{"),
                ("200 OK", "application/json", "{}"),
            ] {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).expect("request");
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
        });

        let config = LibraryAdapterConfig::with_user_agent_and_timeout(
            &format!("http://{address}/https/opaque/"),
            "THYou/library-test",
            Duration::from_secs(5),
        )
        .expect("config");
        let adapter = LibraryReadAdapter::new(config).expect("adapter");

        let login = adapter.read_area_tree().await.expect_err("login error");
        assert!(matches!(login, LibraryAdapterError::SessionExpired));
        assert!(login.is_session_expired());
        assert!(!login.is_temporary_network());
        assert!(!format!("{login:?}").contains("session-secret-body"));

        let status = adapter.read_area_tree().await.expect_err("status error");
        assert!(matches!(status, LibraryAdapterError::SessionExpired));
        assert!(!format!("{status:?}").contains("session-secret-body"));

        let forbidden = adapter
            .read_area_tree()
            .await
            .expect_err("forbidden session error");
        assert!(matches!(forbidden, LibraryAdapterError::SessionExpired));
        assert!(!format!("{forbidden:?}").contains("forbidden-session-body"));

        let status = adapter.read_area_tree().await.expect_err("upstream error");
        assert!(matches!(
            status,
            LibraryAdapterError::HttpStatus {
                status: StatusCode::SERVICE_UNAVAILABLE
            }
        ));
        assert!(!status.is_session_expired());
        assert!(status.is_temporary_network());
        assert!(!format!("{status:?}").contains("upstream-secret-body"));

        let html = adapter
            .read_area_tree()
            .await
            .expect_err("unexpected HTML error");
        assert!(matches!(html, LibraryAdapterError::UnexpectedDeployment));
        assert!(!format!("{html:?}").contains("maintenance-secret-body"));

        let malformed = adapter
            .read_area_tree()
            .await
            .expect_err("malformed JSON error");
        assert!(matches!(
            malformed,
            LibraryAdapterError::Parse(LibraryReadParseError::MalformedJson { .. })
        ));

        let empty_object = adapter
            .read_area_tree()
            .await
            .expect_err("an empty JSON object is not an empty list");
        assert!(matches!(
            empty_object,
            LibraryAdapterError::Parse(LibraryReadParseError::MissingData { field: "data" })
        ));

        server.join().expect("server");
    }

    #[test]
    fn adapter_config_rejects_credentials_query_and_fragment() {
        assert!(matches!(
            LibraryAdapterConfig::new("file:///tmp/library/"),
            Err(LibraryAdapterError::InvalidBaseUrl)
        ));
        assert!(matches!(
            LibraryAdapterConfig::new("https://user:password@example.test/library/"),
            Err(LibraryAdapterError::InvalidBaseUrl)
        ));
        assert!(matches!(
            LibraryAdapterConfig::new("https://example.test/library/?ticket=secret"),
            Err(LibraryAdapterError::InvalidBaseUrl)
        ));
        let config = LibraryAdapterConfig::new("https://example.test/opaque/").expect("config");
        let debug = format!("{config:?}");
        assert!(!debug.contains("/opaque/"));
        assert!(!debug.contains("https://"));

        let home =
            LibraryAdapterConfig::new("https://example.test/opaque-mapping/home/web/f_second")
                .expect("home URL config");
        assert_eq!(home.base_url().path(), "/opaque-mapping/");
    }
}

#[cfg(test)]
#[path = "library_lastmile_tests.rs"]
mod lastmile_tests;
