//! CAB study-room booking (`研读间`): the room catalogue and the account's own
//! reservations.
//!
//! The CAB application is a separate campus deployment on its own host, reached
//! through the WebVPN mapping bound to [`LIBRARY_ROOM_MAPPING_TOKEN`].  Its API
//! is a JSON envelope (`{"code": 0, "data": …}`) rather than the HTML the rest of
//! the library surface returns.
//!
//! That host is **evidenced, not inferred**: a WebVPN mapping token is the fixed
//! ASCII prefix followed by AES-128-CFB of the hostname under the same fixed key
//! and IV, and decoding this module's token yields `cab.lib.tsinghua.edu.cn`
//! exactly.  The reference's own constants agree, carrying
//! `finalAddress=https:%2F%2Fcab.lib.tsinghua.edu.cn` verbatim.
//!
//! # Why this module does not log in
//!
//! The reference reaches this application with a roaming policy of its own
//! (`"cab"`) which, on a rejected read, performs an **identity login**: it fetches
//! `ID_BASE_URL + <payload>` for a public key and posts credentials to the
//! identity login endpoint before retrying.  Its payload is not even a constant —
//! it is extracted from the `…/auth/address` response with
//! `/\/login\/form\/(.+)$/`, so the application id that would be submitted would
//! be chosen by a *response*.
//!
//! This engine does not implement a second campus login, and it must not accept a
//! response-chosen identity-login app id on a credential-submitting request.  The
//! read therefore addresses the application's own fixed WebVPN mapping directly:
//! [`LIBRARY_ROOM_WEBVPN_TARGET`] is recorded for documentation only and is
//! deliberately **not** registered as a roaming selector in
//! `info_session::map_additional_roaming`; no INFO roam is dispatched for this
//! read at all.  The adapter is built from this module's own mapping root
//! ([`LIBRARY_ROOM_MAPPING_TOKEN`]), which is the same absolute mapping the
//! reference's `LIBRARY_ROOM_BOOKING_*_URL` constants carry.  A read whose
//! session has lapsed is reported as [`LibraryRoomAdapterError::SessionExpired`],
//! so the existing INFO refresh path handles it exactly like the other
//! INFO-hosted readers.
//!
//! # What is read, and what is not
//!
//! Two reads are exposed:
//!
//! * [`LibraryRoomAdapter::read_catalog`] — the room kinds and the reservable
//!   devices inside each one.
//! * [`LibraryRoomAdapter::read_records`] — the account's own reservations over a
//!   bounded date window, for the account holder's own review.
//!
//! Everything that changes state — creating a reservation, cancelling one,
//! updating the contact address — is deliberately absent.  So is the account
//! lookup that resolves another person's name to their campus account: this
//! module projects **display names only**, and a member's or owner's account
//! identifier never enters a public type, a plan, a log, or a bridge DTO.
//!
//! # No mock, and no empty result for a page that did not parse
//!
//! The reference answers its mocked calls from built-in fixtures.  Here the
//! service's own envelope is the evidence: a body that is not the envelope, or
//! whose `code` is not zero, is a failure.  An empty room list is only produced
//! from an envelope that really carried an empty array, so "no device is
//! reservable" can never be manufactured from a response that failed to parse.
//!
//! Times are presented exactly as the service wrote them.  The service supplies
//! no zone, so no conversion is invented here — the same rule the Learn
//! discussion times follow.
//!
//! This module reimplements the contract from public reference behavior.  It does
//! not copy source, fixtures, or assets.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use chrono::NaiveDate;
use reqwest::{
    StatusCode, Url,
    header::{ACCEPT, CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::campus_html::{self, PageClass};
use crate::transport::{CampusHttpTransport, TransportError};

/// The mapping token of the CAB study-room application.  It is the only mapping
/// this module will address, so a rewritten URL cannot reach another application.
///
/// Decoding it (ASCII prefix stripped, AES-128-CFB under the fixed WebVPN key and
/// IV) yields exactly `cab.lib.tsinghua.edu.cn`.
pub(crate) const LIBRARY_ROOM_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421f3f643d22b396a1e6a1b80a29f5d363409e413829737d1";

/// The scheme segment of the mapping.  The reference spells every CAB URL as
/// `/https/<token>/…`.
pub(crate) const LIBRARY_ROOM_MAPPING_SCHEME: &str = "https";

/// The reference's roaming policy name for this application.
///
/// Using it would mean performing a campus identity login inside a read, with an
/// application id taken from a response.  This engine does not do that, so the
/// name is retained only so a future reader can see which recovery path was
/// deliberately not implemented.  It is never dispatched and carries no request.
pub const LIBRARY_ROOM_WEBVPN_TARGET: &str = "cab";

/// The account-and-session probe the reference uses before a read, to decide
/// whether to run its own identity login.
///
/// This module **never requests it**: that decision is exactly the second campus
/// login this module does not perform, and the account binding here comes from
/// the INFO/WebVPN session the transport already holds.  The path is recorded so
/// a reader can see which probe was deliberately left out rather than merely
/// forgotten, and it answers no request of its own.
pub const LIBRARY_ROOM_USER_INFO_PATH: &str = "/ic-web/auth/userInfo";

/// The room-kind and device catalogue.
pub const LIBRARY_ROOM_CATALOG_PATH: &str = "/ic-web/roomDevice/roomInfos";

/// The account's own reservation list.
pub const LIBRARY_ROOM_RECORDS_PATH: &str = "/ic-web/reserve/resvInfo";

/// The status filter, ordering column and direction the service is asked for.
///
/// They are the service's own values, not caller preferences, so they stay
/// constants instead of parameters.
const RECORDS_STATUS_FILTER: &str = "8454";
const RECORDS_ORDER_COLUMN: &str = "gmt_create";
const RECORDS_ORDER_MODEL: &str = "desc";

/// The furthest ahead the reservation window may reach.
///
/// The window is at most [`LIBRARY_ROOM_MAX_WINDOW_DAYS`] days wide, so a caller
/// cannot use this read to walk the account's history without bound.
pub const LIBRARY_ROOM_MAX_WINDOW_DAYS: i64 = 31;

/// The envelope key the service uses for its own status, and the value that means
/// the request was answered.
const ENVELOPE_CODE_KEY: &str = "code";
const ENVELOPE_DATA_KEY: &str = "data";
const ENVELOPE_OK: i64 = 0;

/// The largest body this module will read, and the per-field bounds.
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const MAX_KINDS: usize = 256;
const MAX_ROOMS_PER_KIND: usize = 256;
const MAX_RECORDS: usize = 512;
const MAX_MEMBERS: usize = 64;
const MAX_TEXT_CHARS: usize = 256;

static NEXT_LIBRARY_ROOM_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This profile only issues GETs, so no write route can be smuggled into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryRoomMethod {
    Get,
}

/// The observed study-room operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryRoomOperation {
    ReadCatalog,
    ReadRecords,
}

/// A study-room read requires an already established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryRoomSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path and query only, never an absolute
/// WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomRequestPlan {
    pub operation: LibraryRoomOperation,
    pub method: LibraryRoomMethod,
    pub path: &'static str,
    pub query: Option<String>,
    pub webvpn_target: &'static str,
    pub session_prerequisite: LibraryRoomSessionPrerequisite,
}

/// Fixed study-room route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryRoomProfile;

impl LibraryRoomProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub fn catalog_request(self) -> LibraryRoomRequestPlan {
        LibraryRoomRequestPlan {
            // The catalogue carries no query at all, so the plan spells that
            // explicitly rather than leaving an empty string to be reconstructed.
            operation: LibraryRoomOperation::ReadCatalog,
            method: LibraryRoomMethod::Get,
            path: LIBRARY_ROOM_CATALOG_PATH,
            query: None,
            webvpn_target: LIBRARY_ROOM_WEBVPN_TARGET,
            session_prerequisite: LibraryRoomSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    /// Builds the reservation-list request for one already validated window.
    ///
    /// Both dates are rechecked here rather than trusted from the caller, so a
    /// value that reached this function by another route still cannot become a
    /// query.
    pub fn records_request(
        self,
        begin: &str,
        end: &str,
    ) -> Result<LibraryRoomRequestPlan, LibraryRoomAdapterError> {
        let (begin, end) = validated_window(begin, end)?;
        Ok(LibraryRoomRequestPlan {
            operation: LibraryRoomOperation::ReadRecords,
            method: LibraryRoomMethod::Get,
            path: LIBRARY_ROOM_RECORDS_PATH,
            query: Some(format!(
                "needStatus={RECORDS_STATUS_FILTER}&orderKey={RECORDS_ORDER_COLUMN}\
                 &orderModel={RECORDS_ORDER_MODEL}&beginDate={begin}&endDate={end}"
            )),
            webvpn_target: LIBRARY_ROOM_WEBVPN_TARGET,
            session_prerequisite: LibraryRoomSessionPrerequisite::ExistingInfoWebVpnSession,
        })
    }
}

/// One reservable device inside a room kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoom {
    /// The service's own device identifier.  It addresses this room inside this
    /// module and is projected as a plain number because it is not a secret: it
    /// is printed on the room's own page.
    pub device_id: u64,
    pub name: String,
    /// The shortest reservation the service accepts for this device, in minutes.
    pub min_reserve_minutes: u64,
}

/// One group of rooms the service files under a shared kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomKind {
    pub kind_id: u64,
    pub kind_name: String,
    pub rooms: Vec<LibraryRoom>,
}

/// The whole reservable catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomCatalog {
    pub kinds: Vec<LibraryRoomKind>,
}

impl LibraryRoomCatalog {
    pub fn is_empty(&self) -> bool {
        self.kinds.iter().all(|kind| kind.rooms.is_empty())
    }

    /// The number of reservable devices in the catalogue.
    pub fn room_count(&self) -> usize {
        self.kinds.iter().map(|kind| kind.rooms.len()).sum()
    }
}

/// One participant of a reservation, by display name only.
///
/// The service's own account identifier for this person is deliberately not
/// projected: the account holder already knows who they invited, and another
/// person's campus account name has no business crossing this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomMember {
    pub name: String,
}

/// One reservation held by the account.
///
/// `date`, `begin_time` and `end_time` are carried exactly as the service wrote
/// them.  The service supplies no zone, so none is assumed here.
///
/// The service's own cancellation handle is deliberately not projected: no
/// cancellation is reachable through this SDK, so a caller has nothing to do with
/// it and it stays inside the module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomRecord {
    /// The name the reservation is held under.
    pub name: String,
    pub device_name: String,
    pub kind_name: String,
    pub date: String,
    pub begin_time: String,
    pub end_time: String,
    pub members: Vec<LibraryRoomMember>,
}

/// Parser failures retain only stable names.  They never keep response bytes,
/// Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LibraryRoomParseError {
    #[error("study-room response body is empty")]
    EmptyBody,

    #[error("study-room response is an HTML login page")]
    LoginPage,

    #[error("study-room response is an expired or timed-out page")]
    ExpiredPage,

    #[error("study-room response is not the expected JSON envelope")]
    NotEnvelope,

    #[error("study-room response carried no result object")]
    MissingData,

    #[error("study-room response is larger than this domain will read")]
    TooLarge,

    #[error("study-room response could not be read: {context}")]
    Malformed { context: &'static str },

    #[error("study-room entry {row} is missing a required field")]
    MissingField { row: usize },

    #[error("study-room entry {row} carried a field this module will not project")]
    UnexpectedValue { row: usize },

    #[error("study-room response carried more entries than this domain will read")]
    TooManyEntries,

    /// The service answered its own envelope with a non-zero status.
    ///
    /// Only the number is kept.  The service's accompanying message is server
    /// text and never becomes part of an error, a log line, or a DTO.
    #[error("study-room service refused the request with status {code}")]
    ServiceRejected { code: i64 },
}

impl LibraryRoomParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

/// Adapter failures are body-free so a login page cannot leak through a debug or
/// bridge DTO.
#[derive(Debug, Error)]
pub enum LibraryRoomAdapterError {
    #[error("study-room base URL is invalid")]
    InvalidBaseUrl,

    #[error("study-room transport failed")]
    Transport(#[source] TransportError),

    #[error("study-room request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("study-room response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("study-room response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("study-room response is not the expected content type")]
    UnexpectedContentType,

    #[error("study-room response is larger than this domain will read")]
    UnexpectedDeployment,

    #[error("study-room INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("study-room response could not be parsed: {0}")]
    Parse(#[source] LibraryRoomParseError),

    #[error("the requested study-room window is not usable")]
    InvalidWindow,
}

impl LibraryRoomAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "library_room_config",
            Self::Transport(_) => "library_room_network",
            Self::HttpStatus { .. } => "library_room_http",
            Self::UnexpectedOrigin => "library_room_origin",
            Self::UnexpectedPath => "library_room_path",
            Self::UnexpectedContentType => "library_room_content_type",
            Self::UnexpectedDeployment => "library_room_deployment",
            Self::SessionExpired => "library_room_auth_required",
            Self::Parse(LibraryRoomParseError::EmptyBody) => "library_room_body_empty",
            Self::Parse(LibraryRoomParseError::LoginPage)
            | Self::Parse(LibraryRoomParseError::ExpiredPage) => "library_room_auth_required",
            Self::Parse(LibraryRoomParseError::NotEnvelope) => "library_room_envelope",
            Self::Parse(LibraryRoomParseError::MissingData) => "library_room_data",
            Self::Parse(LibraryRoomParseError::TooLarge)
            | Self::Parse(LibraryRoomParseError::TooManyEntries) => "library_room_size",
            Self::Parse(LibraryRoomParseError::Malformed { .. }) => "library_room_malformed",
            Self::Parse(LibraryRoomParseError::MissingField { .. }) => "library_room_field",
            Self::Parse(LibraryRoomParseError::UnexpectedValue { .. }) => "library_room_value",
            Self::Parse(LibraryRoomParseError::ServiceRejected { .. }) => "library_room_rejected",
            Self::InvalidWindow => "library_room_window",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::SessionExpired)
            || matches!(self, Self::Parse(error) if error.is_session_expired())
    }
}

/// Configuration for a read-only study-room adapter.
#[derive(Clone)]
pub struct LibraryRoomAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl LibraryRoomAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, LibraryRoomAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/library-room", Duration::from_secs(30))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, LibraryRoomAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, LibraryRoomAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| LibraryRoomAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(LibraryRoomAdapterError::InvalidBaseUrl);
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

    fn transport(&self) -> Result<CampusHttpTransport, LibraryRoomAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(LibraryRoomAdapterError::Transport)
    }
}

impl fmt::Debug for LibraryRoomAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryRoomAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one study-room response.  It is opaque: no
/// Cookie, URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryRoomBusinessProof {
    adapter_binding: u64,
    generation: u64,
    pub(crate) operation: LibraryRoomOperation,
}

impl fmt::Debug for LibraryRoomBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryRoomBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated catalogue together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomCatalogRead {
    pub value: LibraryRoomCatalog,
    pub proof: LibraryRoomBusinessProof,
}

/// A validated reservation list together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoomRecordsRead {
    pub value: Vec<LibraryRoomRecord>,
    pub proof: LibraryRoomBusinessProof,
}

/// Read-only CAB study-room client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must be
/// the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct LibraryRoomAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: LibraryRoomProfile,
    binding: u64,
    generation: std::sync::atomic::AtomicU64,
}

impl LibraryRoomAdapter {
    pub fn new(config: LibraryRoomAdapterConfig) -> Result<Self, LibraryRoomAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, LibraryRoomAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: LibraryRoomProfile::standard(),
            binding: NEXT_LIBRARY_ROOM_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            generation: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> LibraryRoomProfile {
        self.profile
    }

    /// The number of accepted reads this adapter has served.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Reads the room kinds and the devices inside each one.
    ///
    /// The catalogue is read live on every call and never served from a cached
    /// copy: a device can be withdrawn or reopened between requests, so a
    /// retained catalogue would present an unusable room as reservable.
    pub async fn read_catalog(&self) -> Result<LibraryRoomCatalog, LibraryRoomAdapterError> {
        self.read_catalog_with_proof().await.map(|read| read.value)
    }

    pub async fn read_catalog_with_proof(
        &self,
    ) -> Result<LibraryRoomCatalogRead, LibraryRoomAdapterError> {
        let plan = self.profile.catalog_request();
        let body = self.get(&plan).await?;
        let value = parse_library_room_catalog_json(&body).map_err(map_parse_error)?;
        Ok(LibraryRoomCatalogRead {
            value,
            proof: self.business_proof(LibraryRoomOperation::ReadCatalog),
        })
    }

    /// Reads the account's own reservations for one bounded window.
    ///
    /// The window is validated before any request: both dates must be calendar
    /// dates, must not be reversed, and must not span more than
    /// [`LIBRARY_ROOM_MAX_WINDOW_DAYS`] days.
    pub async fn read_records(
        &self,
        begin: &str,
        end: &str,
    ) -> Result<Vec<LibraryRoomRecord>, LibraryRoomAdapterError> {
        self.read_records_with_proof(begin, end)
            .await
            .map(|read| read.value)
    }

    pub async fn read_records_with_proof(
        &self,
        begin: &str,
        end: &str,
    ) -> Result<LibraryRoomRecordsRead, LibraryRoomAdapterError> {
        let plan = self.profile.records_request(begin, end)?;
        let body = self.get(&plan).await?;
        let value = parse_library_room_records_json(&body).map_err(map_parse_error)?;
        Ok(LibraryRoomRecordsRead {
            value,
            proof: self.business_proof(LibraryRoomOperation::ReadRecords),
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &LibraryRoomBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    fn business_proof(&self, operation: LibraryRoomOperation) -> LibraryRoomBusinessProof {
        LibraryRoomBusinessProof {
            adapter_binding: self.binding,
            generation: self.generation.fetch_add(1, Ordering::Relaxed) + 1,
            operation,
        }
    }

    /// Issues one request and applies every guard that must precede a parse.
    async fn get(&self, plan: &LibraryRoomRequestPlan) -> Result<String, LibraryRoomAdapterError> {
        let endpoint = self.endpoint(plan)?;
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let response = self
            .transport
            .send(
                self.transport
                    .client()
                    .get(endpoint)
                    .header(ACCEPT, "application/json"),
            )
            .await
            .map_err(|error| LibraryRoomAdapterError::Transport(TransportError::Request(error)))?;
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
            .map_err(|_| LibraryRoomAdapterError::UnexpectedOrigin)?;
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
            return Err(LibraryRoomAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(LibraryRoomAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(LibraryRoomAdapterError::UnexpectedPath);
        }
        if !status.is_redirection()
            && (final_url.path() != expected_path
                || final_url.query().map(str::to_owned) != expected_query
                || final_url.fragment().is_some()
                || !path_within_base(&self.base_url, &final_url))
        {
            return Err(LibraryRoomAdapterError::UnexpectedPath);
        }
        if status.is_redirection() {
            return Err(LibraryRoomAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| LibraryRoomAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_JSON_BYTES {
            return Err(LibraryRoomAdapterError::UnexpectedDeployment);
        }
        // The session-level pages are classified first, so an HTML answer that
        // really is a login or expiry page is a session failure rather than a
        // format failure.  Any *other* HTML is not: it is a deployment this
        // module does not understand, and it is reported as such instead of
        // being folded into an authentication error.
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(LibraryRoomAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        if status != StatusCode::OK {
            return Err(LibraryRoomAdapterError::HttpStatus { status });
        }
        if looks_like_html(&body) {
            return Err(LibraryRoomAdapterError::Parse(
                LibraryRoomParseError::NotEnvelope,
            ));
        }
        if !is_json_content_type(content_type.as_deref()) {
            return Err(LibraryRoomAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    fn endpoint(&self, plan: &LibraryRoomRequestPlan) -> Result<Url, LibraryRoomAdapterError> {
        if !valid_relative_path(plan.path) {
            return Err(LibraryRoomAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/').to_owned();
        endpoint.set_path(&format!("{base_path}{}", plan.path));
        match plan.query.as_deref() {
            Some(query) if !valid_query(query) => {
                return Err(LibraryRoomAdapterError::InvalidBaseUrl);
            }
            Some(query) => endpoint.set_query(Some(query)),
            None => endpoint.set_query(None),
        }
        endpoint.set_fragment(None);
        Ok(endpoint)
    }
}

impl fmt::Debug for LibraryRoomAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryRoomAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("generation", &self.generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

fn map_parse_error(error: LibraryRoomParseError) -> LibraryRoomAdapterError {
    if error.is_session_expired() {
        LibraryRoomAdapterError::SessionExpired
    } else {
        LibraryRoomAdapterError::Parse(error)
    }
}

/// Bounds one reservation window before it can enter a query.
///
/// The check lives here rather than only in the runtime so a plan cannot be built
/// from unvalidated dates by any route.
fn validated_window(begin: &str, end: &str) -> Result<(String, String), LibraryRoomAdapterError> {
    let begin_date = NaiveDate::parse_from_str(begin, "%Y-%m-%d")
        .map_err(|_| LibraryRoomAdapterError::InvalidWindow)?;
    let end_date = NaiveDate::parse_from_str(end, "%Y-%m-%d")
        .map_err(|_| LibraryRoomAdapterError::InvalidWindow)?;
    if end_date < begin_date
        || end_date.signed_duration_since(begin_date).num_days() >= LIBRARY_ROOM_MAX_WINDOW_DAYS
    {
        return Err(LibraryRoomAdapterError::InvalidWindow);
    }
    Ok((begin_date.to_string(), end_date.to_string()))
}

/// Reads the service's own JSON envelope.
///
/// The envelope is `{"code": 0, "data": …}`.  A non-zero `code` is the service's
/// own refusal and is surfaced as such, with only its number retained: the
/// accompanying message is server text and never becomes part of an error.
fn envelope(body: &str) -> Result<serde_json::Value, LibraryRoomParseError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(LibraryRoomParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(LibraryRoomParseError::LoginPage),
        PageClass::Expired => return Err(LibraryRoomParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| LibraryRoomParseError::NotEnvelope)?;
    let object = value
        .as_object()
        .ok_or(LibraryRoomParseError::NotEnvelope)?;
    let code = object
        .get(ENVELOPE_CODE_KEY)
        .and_then(serde_json::Value::as_i64)
        .ok_or(LibraryRoomParseError::NotEnvelope)?;
    if code != ENVELOPE_OK {
        return Err(LibraryRoomParseError::ServiceRejected { code });
    }
    object
        .get(ENVELOPE_DATA_KEY)
        .cloned()
        .ok_or(LibraryRoomParseError::MissingData)
}

/// Parses the room catalogue.
pub fn parse_library_room_catalog_json(
    body: &str,
) -> Result<LibraryRoomCatalog, LibraryRoomParseError> {
    let data = envelope(body)?;
    let kinds_json = data.as_array().ok_or(LibraryRoomParseError::Malformed {
        context: "catalogue is not a list",
    })?;
    if kinds_json.len() > MAX_KINDS {
        return Err(LibraryRoomParseError::TooManyEntries);
    }
    let mut kinds = Vec::with_capacity(kinds_json.len());
    for (index, entry) in kinds_json.iter().enumerate() {
        kinds.push(parse_kind(entry, index)?);
    }
    Ok(LibraryRoomCatalog { kinds })
}

fn parse_kind(
    entry: &serde_json::Value,
    index: usize,
) -> Result<LibraryRoomKind, LibraryRoomParseError> {
    let object = entry
        .as_object()
        .ok_or(LibraryRoomParseError::Malformed { context: "kind" })?;
    let kind_id = positive_id(object.get("kindId"), index)?;
    let kind_name = required_text(object.get("kindName"), index)?;
    let rooms_json = object
        .get("roomInfos")
        .and_then(serde_json::Value::as_array)
        .ok_or(LibraryRoomParseError::MissingField { row: index })?;
    if rooms_json.len() > MAX_ROOMS_PER_KIND {
        return Err(LibraryRoomParseError::TooManyEntries);
    }
    let mut rooms = Vec::with_capacity(rooms_json.len());
    for room in rooms_json {
        let room = room
            .as_object()
            .ok_or(LibraryRoomParseError::Malformed { context: "room" })?;
        rooms.push(LibraryRoom {
            device_id: positive_id(room.get("devId"), index)?,
            name: required_text(room.get("devName"), index)?,
            min_reserve_minutes: positive_id(room.get("minResvTime"), index)?,
        });
    }
    Ok(LibraryRoomKind {
        kind_id,
        kind_name,
        rooms,
    })
}

/// Parses the account's own reservation list.
pub fn parse_library_room_records_json(
    body: &str,
) -> Result<Vec<LibraryRoomRecord>, LibraryRoomParseError> {
    let data = envelope(body)?;
    let records = data.as_array().ok_or(LibraryRoomParseError::Malformed {
        context: "records are not a list",
    })?;
    if records.len() > MAX_RECORDS {
        return Err(LibraryRoomParseError::TooManyEntries);
    }
    let mut parsed = Vec::with_capacity(records.len());
    for (index, entry) in records.iter().enumerate() {
        parsed.push(parse_record(entry, index)?);
    }
    Ok(parsed)
}

fn parse_record(
    entry: &serde_json::Value,
    index: usize,
) -> Result<LibraryRoomRecord, LibraryRoomParseError> {
    let object = entry
        .as_object()
        .ok_or(LibraryRoomParseError::Malformed { context: "record" })?;
    // The device list names the room the reservation was made for.  A record
    // without one cannot say which room it holds, so it is a failure rather than
    // a reservation with an empty name.
    let devices = object
        .get("resvDevInfoList")
        .and_then(serde_json::Value::as_array)
        .ok_or(LibraryRoomParseError::MissingField { row: index })?;
    let device = devices
        .first()
        .and_then(serde_json::Value::as_object)
        .ok_or(LibraryRoomParseError::MissingField { row: index })?;
    let members_json = object
        .get("resvMemberInfoList")
        .and_then(serde_json::Value::as_array)
        .ok_or(LibraryRoomParseError::MissingField { row: index })?;
    if members_json.len() > MAX_MEMBERS {
        return Err(LibraryRoomParseError::TooManyEntries);
    }
    let mut members = Vec::with_capacity(members_json.len());
    for member in members_json {
        let member = member
            .as_object()
            .ok_or(LibraryRoomParseError::Malformed { context: "member" })?;
        members.push(LibraryRoomMember {
            // Only the printed name is projected; the service's account
            // identifier for this person is deliberately dropped here.
            name: required_text(member.get("trueName"), index)?,
        });
    }
    Ok(LibraryRoomRecord {
        name: required_text(object.get("resvName"), index)?,
        device_name: required_text(device.get("devName"), index)?,
        kind_name: required_text(device.get("kindName"), index)?,
        date: required_text(object.get("resvDate"), index)?,
        begin_time: required_text(object.get("resvBeginTime"), index)?,
        end_time: required_text(object.get("resvEndTime"), index)?,
        members,
    })
}

/// Reads one required, bounded, control-free text field.
fn required_text(
    value: Option<&serde_json::Value>,
    index: usize,
) -> Result<String, LibraryRoomParseError> {
    let text = match value {
        Some(serde_json::Value::String(text)) => text.trim().to_owned(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        _ => return Err(LibraryRoomParseError::MissingField { row: index }),
    };
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(LibraryRoomParseError::UnexpectedValue { row: index });
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        // Truncating would hand back a value the service never printed.
        return Err(LibraryRoomParseError::UnexpectedValue { row: index });
    }
    Ok(text)
}

/// Reads one non-negative integer identifier the service wrote.
///
/// A negative or fractional value is a shape this module does not understand, so
/// it is refused rather than rounded.
fn positive_id(
    value: Option<&serde_json::Value>,
    index: usize,
) -> Result<u64, LibraryRoomParseError> {
    match value {
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .ok_or(LibraryRoomParseError::UnexpectedValue { row: index }),
        Some(serde_json::Value::String(text)) => text
            .trim()
            .parse::<u64>()
            .map_err(|_| LibraryRoomParseError::UnexpectedValue { row: index }),
        _ => Err(LibraryRoomParseError::MissingField { row: index }),
    }
}

/// Returns true when a body is an HTML document rather than a JSON envelope.
///
/// The application answers a lapsed session with a login page under a 200 status
/// and a JSON content type is not guaranteed on that path, so the body itself is
/// the evidence.
fn looks_like_html(body: &str) -> bool {
    let head = body.trim_start();
    head.starts_with("<!DOCTYPE")
        || head.starts_with("<!doctype")
        || head.starts_with("<html")
        || head.starts_with("<HTML")
}

fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("application/json") || mime.eq_ignore_ascii_case("text/json")
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, LibraryRoomAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(LibraryRoomAdapterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = match opaque_mapping_root(path) {
        Some(root) => root,
        None if path.is_empty() => "/".to_owned(),
        // A base URL that is neither an opaque mapping nor a bare origin is
        // refused rather than kept: a configured path is authority this module
        // did not grant, and letting one through would address a route the
        // module never recorded.
        None => return Err(LibraryRoomAdapterError::InvalidBaseUrl),
    };
    base_url.set_path(&path);
    Ok(base_url)
}

/// Reduces a mapping URL to `/{scheme}/{token}/`, so a configured base can never
/// carry a deeper path of its own.
///
/// The token itself is validated as a WebVPN mapping: the public fixed prefix
/// followed by an even number of lower-case hex digits.  Without that check a
/// mistyped token would be accepted here and only fail much later, as a broker
/// response this module could not explain.
fn opaque_mapping_root(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let scheme = segments.next()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let token = segments.next()?;
    if !safe_webvpn_mapping_id(token) {
        return None;
    }
    Some(format!("/{scheme}/{token}/"))
}

/// A WebVPN mapping is the public fixed prefix followed by a lower-case hex host
/// selector.  It is not a URL, a session ticket, or a credential.
fn safe_webvpn_mapping_id(value: &str) -> bool {
    const PREFIX: &str = "77726476706e69737468656265737421";
    value.starts_with(PREFIX)
        && (64..=96).contains(&value.len())
        && value.len() % 2 == 0
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, LibraryRoomAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(LibraryRoomAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| LibraryRoomAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(LibraryRoomAdapterError::UnexpectedOrigin);
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
        && path.len() <= 128
        && !path.contains("://")
        && !path.contains(['?', '#', '\\'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

/// Validates a query this module built.
///
/// The query is assembled from module constants plus two calendar dates, so the
/// accepted alphabet is deliberately narrow: unreserved characters, `=`, `&` and
/// nothing else.  A value that would need percent-encoding cannot appear in it,
/// and the check says so rather than silently rewriting the query.
fn valid_query(query: &str) -> bool {
    !query.is_empty()
        && query.len() <= 512
        && query.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'=' | b'&')
        })
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
