//! Read-only sports-venue resources and reservation records (`体育场馆`).
//!
//! The venue booking application is a legacy JSP deployment whose pages carry
//! their data **inside an inline script** rather than in the document text.
//! The slot table is built by `resourceArray.push({…})`, priced by
//! `addCost(…)`, and marked by `markResStatus(…)` / `markStatusColor(…)`; the
//! two limits are `var limitBookCount` and `var limitBookInit`.  This module
//! therefore reads those statements with its own bounded scanner rather than
//! through the crate's element scan, because no element carries the values.
//! The scanner parses the statements' argument lists; it never evaluates them.
//!
//! Four service behaviours shape the module:
//!
//! * **A slot is accepted only when the page's own later statement repeats its
//!   identifier.**  The public reference pairs the `resourceArray.push` entry
//!   with the `resourcesm.put` that follows it *positionally*; this module
//!   pairs them by **equal identifier** instead, which is strictly narrower:
//!   a slot whose hash statement names a different slot is dropped rather than
//!   given a neighbouring slot's hash.
//! * **The unpaid table is read relative to a semantic landmark, never from a
//!   bare index.**  The crate's element scan deliberately refuses positional
//!   indexing, so this module owns the decision and pays for it: the row's
//!   *method cell* is located by its own text (`网上支付` / `现场支付`), and the
//!   four value cells the reference reports sit at fixed distances before it.
//!   A row whose landmark is missing or ambiguous is reported as unrecognized
//!   instead of being read with one column's value standing in for another's.
//!   The action
//!   cell sits two cells after the landmark, and the booking / payment
//!   identifiers are taken from the inline `payNow` / `unsubscribeOnline` /
//!   `unsubscribe` calls it carries — never from an index.
//! * **The paid table has no such landmark**, because its rows carry one
//!   hard-coded method and the reference reads four consecutive cells.  This
//!   module accepts a paid row only through the page's own carrier shape — a
//!   `tr` hidden with `style="display:none"` holding a nested `tbody` whose
//!   first row has the cells — and reports any carrier that does not match as
//!   unrecognized rather than as a short or partly-filled record.
//! * **An account with no reservations is a legitimate empty answer, but only
//!   from a page whose structure proves it.**  A response with no table at all
//!   is an error, never an empty list, and a login or expiry banner is a
//!   session failure.
//!
//! Nothing here places an order, pays, or cancels: this module has no write
//! representation at all, so an unreachable operation cannot be issued by
//! accident.  The captcha and payment routes are deliberately absent.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use reqwest::{StatusCode, Url, header::LOCATION};
use thiserror::Error;

use crate::campus_html::{self, PageClass, RawElement};
use crate::sports_write::SportsWriteAdapterError;
use crate::transport::{CampusHttpTransport, TransportError};

/// The sports-venue roaming selector.
///
/// The venue application is reached through the INFO/WebVPN handoff like the
/// other campus reads, so a proven INFO session is its only prerequisite.
pub const SPORTS_WEBVPN_TARGET: &str = "5539ECF8CD815C7D3F5A8EE0A2D72441";

/// The WebVPN mapping token the selector above resolves to.
pub const SPORTS_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421a5a70f8834396657761d88e29d51367b6a00";

/// The slot-resource page, which also carries the two reservation limits.
pub const SPORTS_BOOK_PATH: &str = "/gymbook/gymBookAction.do";
/// The slot-detail page, whose inline script carries the slot table.
pub const SPORTS_DETAIL_PATH: &str = "/gymsite/cacheAction.do";
/// The order page, which lists the account's reservations.
pub const SPORTS_PAY_PATH: &str = "/pay/payAction.do";

/// The longest page this module will parse.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// The longest single string taken out of an inline script.
const MAX_SCRIPT_ARG_BYTES: usize = 256;
/// The most statements of one kind this module will collect from one page.
const MAX_SCRIPT_CALLS: usize = 4096;
/// The most fields this module will read from one object literal.
const MAX_OBJECT_FIELDS: usize = 32;
/// The most slots one resource page may report.
const MAX_RESOURCES: usize = 4096;
/// The most reservation rows one page may report.
const MAX_RECORDS: usize = 4096;
/// The longest date token this module will put into a query.
const MAX_DATE_CHARS: usize = 10;
/// The most digits one venue identifier may carry.
const MAX_ID_DIGITS: usize = 10;

/// The two payment methods the unpaid table prints.  Exactly one of them is
/// present in each reservation row, and it is that row's landmark.
const UNPAID_METHODS: [&str; 2] = ["网上支付", "现场支付"];
/// The method the paid table's rows report.  The page hard-codes it, so it is
/// not a landmark — see the module documentation.
pub const PAID_METHOD: &str = "已支付";

/// The state the phone endpoint answers when the account has configured none.
const NO_PHONE_FLAG: &str = "do_not";

static NEXT_SPORTS_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This module only issues GETs, so a write route cannot be smuggled into a
/// read plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsMethod {
    Get,
}

/// The read operations this module models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsOperation {
    /// Limits, phone number, and the slot table for one venue and date.
    ReadResources,
    /// The account's unpaid and paid reservation rows.
    ReadRecords,
}

impl SportsOperation {
    /// The label this operation is recorded under.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadResources => "read_resource_list",
            Self::ReadRecords => "read_records",
        }
    }
}

/// A sports-venue read requires an established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path and query only, never an absolute
/// WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SportsRequest {
    pub operation: SportsOperation,
    pub method: SportsMethod,
    pub path: &'static str,
    pub query: String,
    pub webvpn_target: &'static str,
    pub session_prerequisite: SportsSessionPrerequisite,
}

impl SportsRequest {
    pub fn query_string(&self) -> &str {
        &self.query
    }
}

/// Fixed sports-venue route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SportsProfile;

impl SportsProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        SPORTS_WEBVPN_TARGET
    }

    /// Builds the three reads one resource lookup needs.
    ///
    /// Every caller-supplied value is bounded here, before it can reach a
    /// query string: a venue identifier is digits only, and a date must be a
    /// real `YYYY-MM-DD` calendar day.  A refused value therefore costs no
    /// network round trip.
    pub fn resources_requests(
        self,
        gym_id: &str,
        item_id: &str,
        date: &str,
    ) -> Result<[SportsRequest; 3], SportsAdapterError> {
        let gym_id = venue_identifier(gym_id)?;
        let item_id = venue_identifier(item_id)?;
        let date = calendar_date(date)?;
        let scope = format!("gymnasium_id={gym_id}&item_id={item_id}&time_date={date}");
        Ok([
            SportsRequest {
                operation: SportsOperation::ReadResources,
                method: SportsMethod::Get,
                path: SPORTS_BOOK_PATH,
                query: format!("ms=viewGymBook&viewType=m&{scope}"),
                webvpn_target: SPORTS_WEBVPN_TARGET,
                session_prerequisite: SportsSessionPrerequisite::ExistingInfoWebVpnSession,
            },
            SportsRequest {
                operation: SportsOperation::ReadResources,
                method: SportsMethod::Get,
                path: SPORTS_DETAIL_PATH,
                query: format!("ms=viewBook&userType=1&{scope}"),
                webvpn_target: SPORTS_WEBVPN_TARGET,
                session_prerequisite: SportsSessionPrerequisite::ExistingInfoWebVpnSession,
            },
            SportsRequest {
                operation: SportsOperation::ReadResources,
                method: SportsMethod::Get,
                path: SPORTS_BOOK_PATH,
                query: "ms=hadContactOrNot".to_owned(),
                webvpn_target: SPORTS_WEBVPN_TARGET,
                session_prerequisite: SportsSessionPrerequisite::ExistingInfoWebVpnSession,
            },
        ])
    }

    /// Builds the two reservation-list reads: unpaid first, paid second.
    pub fn records_requests(self) -> [SportsRequest; 2] {
        [
            SportsRequest {
                operation: SportsOperation::ReadRecords,
                method: SportsMethod::Get,
                path: SPORTS_PAY_PATH,
                query: "ms=getOrdersForNopay".to_owned(),
                webvpn_target: SPORTS_WEBVPN_TARGET,
                session_prerequisite: SportsSessionPrerequisite::ExistingInfoWebVpnSession,
            },
            SportsRequest {
                operation: SportsOperation::ReadRecords,
                method: SportsMethod::Get,
                path: SPORTS_PAY_PATH,
                query: "ms=getOrdersForUnpay".to_owned(),
                webvpn_target: SPORTS_WEBVPN_TARGET,
                session_prerequisite: SportsSessionPrerequisite::ExistingInfoWebVpnSession,
            },
        ]
    }
}

/// The two reservation limits the venue page reports.
///
/// `count` is the most fields this account may hold at once, and `init` is the
/// service's own "can I book right now" figure: a value at or below zero means
/// booking is closed, and when `count` is zero `init` is the number of unpaid
/// orders the account already has.  Both meanings are the service's own and
/// are reported verbatim rather than interpreted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SportsLimits {
    pub count: u32,
    pub init: u32,
}

/// One bookable (or already booked) slot.
#[derive(Clone, PartialEq, Eq)]
pub struct SportsResource {
    /// The slot's own identifier.
    pub res_id: String,
    /// The slot's booking hash.
    ///
    /// It is a single-purpose booking token, so it never appears in a
    /// [`fmt::Debug`] rendering.
    pub res_hash: String,
    /// The service's time-session label, e.g. `20:00-21:00`.
    pub time_session: String,
    /// The court or table the slot belongs to.
    pub field_name: String,
    /// The service's own overlay size, absent when it did not report a
    /// readable number.
    pub overlay_size: Option<u32>,
    /// Whether the service offers this slot for online booking.
    pub can_net_book: bool,
    /// The service's own `addCost` token, preserved verbatim.
    ///
    /// The observed evidence does not establish a unit for this value, so it
    /// is reported as the service wrote it rather than converted into a
    /// currency amount this module cannot justify.
    pub cost: Option<String>,
    /// The order this slot is already attached to, when it is.
    pub book_id: Option<String>,
    /// The service's own lock flag, absent when it did not mark the slot.
    pub locked: Option<bool>,
    /// The user type the service tagged the slot with, when it did.
    pub user_type: Option<String>,
    /// Whether the service marked the slot's payment as settled.
    pub payment_status: Option<bool>,
    /// The venue and item the slot belongs to, in the form the caller named them
    /// when this list was read.
    ///
    /// They are carried so a booking can be built from the read result alone,
    /// rather than from a caller's separate arguments: a slot's own venue and
    /// date are the ones the row came from, and a booking must not be able to mix
    /// one venue's slot with another venue's identifier.
    pub gym_id: String,
    pub item_id: String,
    /// The `YYYY-MM-DD` date this list was read for.
    pub date: String,
    /// The opaque handle that names this slot back to the venue.
    ///
    /// It is minted by the Runtime for the account and the read that produced
    /// this row, and it is the **only** way a booking can name a slot: the
    /// venue's own booking hash, the venue and item identifiers and the date all
    /// stay behind the handle.  A row the venue does not offer for online
    /// booking, or one that came without a hash, carries `None`, because such a
    /// slot cannot be ordered at all.  The parser never fills this in — it is
    /// set by the read that retained the row, so a parser test sees `None`.
    pub selector: Option<String>,
}

/// `res_hash` and `book_id` are single-purpose booking tokens, so `Debug`
/// reports only whether they are present.
impl fmt::Debug for SportsResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsResource")
            .field("res_id", &self.res_id)
            .field("time_session", &self.time_session)
            .field("field_name", &self.field_name)
            .field("overlay_size", &self.overlay_size)
            .field("can_net_book", &self.can_net_book)
            .field("cost", &self.cost)
            .field("has_res_hash", &!self.res_hash.is_empty())
            .field("has_book_id", &self.book_id.is_some())
            .field("locked", &self.locked)
            .field("user_type", &self.user_type)
            .field("payment_status", &self.payment_status)
            .field("has_selector", &self.selector.is_some())
            .finish()
    }
}

/// One venue's slots for one date, plus the limits and phone number that come
/// with them.
#[derive(Clone, PartialEq, Eq)]
pub struct SportsResources {
    pub count: u32,
    pub init: u32,
    /// The phone number the account has configured, or `None` when the service
    /// answered that it has none.
    pub phone: Option<String>,
    pub data: Vec<SportsResource>,
}

/// The phone number is personal data, so `Debug` reports only its presence.
impl fmt::Debug for SportsResources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsResources")
            .field("count", &self.count)
            .field("init", &self.init)
            .field("phone_present", &self.phone.is_some())
            .field("slot_count", &self.data.len())
            .finish()
    }
}

/// One reservation row.
///
/// `name`, `field`, `time`, `price` and `method` are the service's own text
/// and are preserved verbatim: the evidence does not establish a unit for the
/// price, so it is not converted into an amount.
#[derive(Clone, PartialEq, Eq)]
pub struct SportsReservationRecord {
    /// The venue the reference labels as the reservation's name.
    pub name: String,
    /// The court or table reserved.
    pub field: String,
    /// The service's time-session label.
    pub time: String,
    /// The service's own price text.
    pub price: String,
    /// The service's payment method, or [`PAID_METHOD`] for a settled row.
    pub method: String,
    /// The service's booking timestamp, when the row carried one.
    pub book_timestamp: Option<i64>,
    /// The booking identifier, present only when the row offers a cancellation.
    pub book_id: Option<String>,
    /// The payment identifier, present only when the row offers a payment.
    pub pay_id: Option<String>,
    /// The opaque handle that names this row back to the venue.
    ///
    /// It is minted by the Runtime for the account and the read that produced
    /// this row, and the venue's own booking identifier stays behind it.  A row
    /// the service printed without a cancellation control carries `None`, which
    /// is the service's own statement about that reservation rather than a read
    /// failure.  A raw parse leaves it `None`.
    pub selector: Option<String>,
}

/// The booking and payment identifiers are single-purpose tokens, so `Debug`
/// reports only whether they are present.
impl fmt::Debug for SportsReservationRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsReservationRecord")
            .field("name", &self.name)
            .field("field", &self.field)
            .field("time", &self.time)
            .field("price", &self.price)
            .field("method", &self.method)
            .field("book_timestamp", &self.book_timestamp)
            .field("has_book_id", &self.book_id.is_some())
            .field("has_pay_id", &self.pay_id.is_some())
            .field("has_selector", &self.selector.is_some())
            .finish()
    }
}

/// Parser failures retain only stable positions.  They never keep response
/// bytes, Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SportsParseError {
    #[error("sports response body is empty")]
    EmptyBody,

    #[error("sports response is an HTML login page")]
    LoginPage,

    #[error("sports response is an expired or timed-out page")]
    ExpiredPage,

    #[error("sports reservation page does not carry the service's table")]
    MissingTable,

    #[error("sports resource page does not carry the service's booking limits")]
    MissingLimit,

    #[error("sports reservation row {row} is not a shape this client reads")]
    UnrecognizedRow { row: usize },

    #[error("sports slot entry {index} is not a shape this client reads")]
    UnrecognizedSlot { index: usize },

    #[error("sports phone answer is not a value this client reads")]
    UnrecognizedPhone,

    #[error("sports response exceeded the bounded element limit")]
    TooLarge,
}

impl SportsParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired INFO session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

impl From<campus_html::ScanError> for SportsParseError {
    fn from(_: campus_html::ScanError) -> Self {
        Self::TooLarge
    }
}

/// Adapter failures are body-free so a login page cannot leak through a debug
/// or bridge DTO.
#[derive(Debug, Error)]
pub enum SportsAdapterError {
    #[error("sports base URL is invalid")]
    InvalidBaseUrl,

    #[error("sports request input is not a value this client will send")]
    InvalidInput,

    #[error("sports transport failed")]
    Transport(#[source] TransportError),

    #[error("sports request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("sports response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("sports response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("sports INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("sports response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("sports response could not be parsed: {0}")]
    Parse(#[source] SportsParseError),
}

impl SportsAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "sports_config",
            Self::InvalidInput => "sports_input",
            Self::Transport(_) => "sports_network",
            Self::HttpStatus { .. } => "sports_http",
            Self::UnexpectedOrigin => "sports_origin",
            Self::UnexpectedPath => "sports_path",
            Self::SessionExpired => "sports_auth_required",
            Self::UnexpectedDeployment => "sports_template",
            Self::Parse(SportsParseError::EmptyBody) => "sports_body_empty",
            Self::Parse(SportsParseError::LoginPage) => "sports_auth_required",
            Self::Parse(SportsParseError::ExpiredPage) => "sports_auth_required",
            Self::Parse(SportsParseError::MissingTable) => "sports_table_missing",
            Self::Parse(SportsParseError::MissingLimit) => "sports_limit_missing",
            Self::Parse(SportsParseError::UnrecognizedRow { .. }) => "sports_row_unrecognized",
            Self::Parse(SportsParseError::UnrecognizedSlot { .. }) => "sports_slot_unrecognized",
            Self::Parse(SportsParseError::UnrecognizedPhone) => "sports_phone",
            Self::Parse(SportsParseError::TooLarge) => "sports_too_large",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        match self {
            Self::SessionExpired => true,
            Self::Parse(error) => error.is_session_expired(),
            _ => false,
        }
    }
}

/// Configuration for a read-only sports-venue adapter.
#[derive(Clone)]
pub struct SportsAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl SportsAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, SportsAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/sports", Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, SportsAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| SportsAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(SportsAdapterError::InvalidBaseUrl);
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

    fn transport(&self) -> Result<CampusHttpTransport, SportsAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(SportsAdapterError::Transport)
    }
}

impl fmt::Debug for SportsAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one sports-venue response.  It is opaque: no
/// Cookie, URL, account value, booking token, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct SportsBusinessProof {
    adapter_binding: u64,
    operation: SportsOperation,
}

impl fmt::Debug for SportsBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated slot list together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct SportsResourcesRead {
    pub value: SportsResources,
    pub proof: SportsBusinessProof,
}

/// A validated reservation list together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct SportsRecordsRead {
    pub value: Vec<SportsReservationRecord>,
    pub proof: SportsBusinessProof,
}

/// Read-only sports-venue client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct SportsAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: SportsProfile,
    binding: u64,
}

impl SportsAdapter {
    pub fn new(config: SportsAdapterConfig) -> Result<Self, SportsAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, SportsAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: SportsProfile::standard(),
            binding: NEXT_SPORTS_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> SportsProfile {
        self.profile
    }

    /// The mapping root this adapter was configured with.
    ///
    /// The write half is configured from exactly this root, so a write can only
    /// ever go to the deployment the read half proved.  It is a mapping origin,
    /// not a route: no query, and no path beyond the opaque mapping directory.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// The write client for this same proved session.
    ///
    /// It is built from the adapter's own base URL and the transport that
    /// already carries the identity/INFO/WebVPN cookie jar, so a write cannot
    /// open a session of its own, and it shares this adapter's request gate.
    pub fn write_adapter(
        &self,
    ) -> Result<crate::sports_write::SportsWriteAdapter, SportsWriteAdapterError> {
        crate::sports_write::SportsWriteAdapter::try_with_transport(
            self.base_url.clone(),
            self.transport.clone(),
        )
    }

    /// Reads the limits, the phone number, and the slot table for one venue and
    /// date.
    ///
    /// The public reference dispatches these three reads concurrently; this
    /// module dispatches them **sequentially** through the shared transport and
    /// request gate, so one call stays inside the gate's bounded read
    /// dispatch.
    pub async fn read_resources(
        &self,
        gym_id: &str,
        item_id: &str,
        date: &str,
    ) -> Result<SportsResources, SportsAdapterError> {
        self.read_resources_with_proof(gym_id, item_id, date)
            .await
            .map(|read| read.value)
    }

    pub async fn read_resources_with_proof(
        &self,
        gym_id: &str,
        item_id: &str,
        date: &str,
    ) -> Result<SportsResourcesRead, SportsAdapterError> {
        let requests = self.profile.resources_requests(gym_id, item_id, date)?;
        let book = self.get(&requests[0]).await?;
        let limits = parse_sports_limits_html(&book).map_err(Self::map_parse_error)?;
        let detail = self.get(&requests[1]).await?;
        let data = parse_sports_resources_html(&detail, gym_id, item_id, date)
            .map_err(Self::map_parse_error)?;
        let phone_body = self.get(&requests[2]).await?;
        let phone = parse_sports_phone_body(&phone_body).map_err(Self::map_parse_error)?;
        Ok(SportsResourcesRead {
            value: SportsResources {
                count: limits.count,
                init: limits.init,
                phone,
                data,
            },
            proof: self.business_proof(SportsOperation::ReadResources),
        })
    }

    /// Reads the account's unpaid reservations followed by its paid ones.
    ///
    /// The two tables are two pages, so the two reads are dispatched in order
    /// rather than concurrently.
    pub async fn read_records(&self) -> Result<Vec<SportsReservationRecord>, SportsAdapterError> {
        self.read_records_with_proof().await.map(|read| read.value)
    }

    pub async fn read_records_with_proof(&self) -> Result<SportsRecordsRead, SportsAdapterError> {
        let requests = self.profile.records_requests();
        let unpaid = self.get(&requests[0]).await?;
        let mut records =
            parse_sports_unpaid_records_html(&unpaid).map_err(Self::map_parse_error)?;
        let paid = self.get(&requests[1]).await?;
        records.extend(parse_sports_paid_records_html(&paid).map_err(Self::map_parse_error)?);
        Ok(SportsRecordsRead {
            value: records,
            proof: self.business_proof(SportsOperation::ReadRecords),
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &SportsBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    fn business_proof(&self, operation: SportsOperation) -> SportsBusinessProof {
        SportsBusinessProof {
            adapter_binding: self.binding,
            operation,
        }
    }

    async fn get(&self, request: &SportsRequest) -> Result<String, SportsAdapterError> {
        let endpoint = self.endpoint(request)?;
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| SportsAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| SportsAdapterError::UnexpectedOrigin)?;
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
            return Err(SportsAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(SportsAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(SportsAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| SportsAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(SportsAdapterError::UnexpectedDeployment);
        }
        if matches!(
            campus_html::classify_page(&body),
            PageClass::Login | PageClass::Expired
        ) {
            return Err(SportsAdapterError::SessionExpired);
        }
        if status != StatusCode::OK {
            return Err(SportsAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.query().map(str::to_owned) != expected_query
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(SportsAdapterError::UnexpectedPath);
        }
        Ok(body)
    }

    fn endpoint(&self, request: &SportsRequest) -> Result<Url, SportsAdapterError> {
        if !valid_relative_path(request.path) || request.query.chars().any(char::is_control) {
            return Err(SportsAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{}", request.path));
        if request.query.is_empty() {
            endpoint.set_query(None);
        } else {
            endpoint.set_query(Some(&request.query));
        }
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: SportsParseError) -> SportsAdapterError {
        match error {
            SportsParseError::LoginPage | SportsParseError::ExpiredPage => {
                SportsAdapterError::SessionExpired
            }
            other => SportsAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for SportsAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Bounds one venue identifier before it can enter a query.
fn venue_identifier(value: &str) -> Result<&str, SportsAdapterError> {
    if value.is_empty()
        || value.len() > MAX_ID_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SportsAdapterError::InvalidInput);
    }
    Ok(value)
}

/// Bounds one date before it can enter a query.
///
/// The value must be a real calendar day in the form the service's own query
/// uses, so a caller cannot turn the date field into a second filter.
fn calendar_date(value: &str) -> Result<&str, SportsAdapterError> {
    if value.len() != MAX_DATE_CHARS
        || !value.is_ascii()
        || chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()
    {
        return Err(SportsAdapterError::InvalidInput);
    }
    Ok(value)
}

/// Runs the session-level guards that must precede any structural parse.
fn guard_page(html: &str) -> Result<&str, SportsParseError> {
    let trimmed = html.strip_prefix('\u{feff}').unwrap_or(html).trim();
    if trimmed.is_empty() {
        return Err(SportsParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(SportsParseError::LoginPage),
        PageClass::Expired => return Err(SportsParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    Ok(trimmed)
}

/// Parses the two booking limits out of the venue page's inline script.
///
/// Both are required: the reference raises when either is absent, because a
/// page without them is not the page whose slot list may be read.
pub fn parse_sports_limits_html(html: &str) -> Result<SportsLimits, SportsParseError> {
    let trimmed = guard_page(html)?;
    let count = script_numeric_assignment(trimmed, "limitBookCount")
        .ok_or(SportsParseError::MissingLimit)?;
    let init = script_numeric_assignment(trimmed, "limitBookInit")
        .ok_or(SportsParseError::MissingLimit)?;
    Ok(SportsLimits { count, init })
}

/// Reads the account's phone number from the phone endpoint's answer.
///
/// The service answers `do_not` when the account has configured none, which is
/// a validated absence.  Anything else must read as a phone token; an HTML
/// page or any other body is an error, never a silent "no phone configured".
pub fn parse_sports_phone_body(body: &str) -> Result<Option<String>, SportsParseError> {
    let trimmed = guard_page(body)?;
    if trimmed == NO_PHONE_FLAG {
        return Ok(None);
    }
    let token = trimmed
        .chars()
        .take(MAX_SCRIPT_ARG_BYTES)
        .collect::<String>();
    let trimmed_token = token.trim();
    if trimmed_token.is_empty()
        || trimmed_token.len() > 32
        || !trimmed_token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-'))
    {
        return Err(SportsParseError::UnrecognizedPhone);
    }
    Ok(Some(trimmed_token.to_owned()))
}

/// Parses the slot table out of the slot-detail page.
///
/// A slot is accepted only when the page's own `resourcesm.put` statement
/// repeats its identifier, so a slot whose hash statement names a different
/// slot is dropped rather than given a neighbouring slot's hash.  A slot entry
/// that carries no identifier is an unrecognized entry, not a dropped one: an
/// unreadable page must not look like an empty venue.
///
/// `gym_id`, `item_id` and `date` are the ones the caller read the list for.
/// They are stamped onto every row so a booking can be built from the read
/// result alone and cannot pair a slot with another venue's identifiers.
pub fn parse_sports_resources_html(
    html: &str,
    gym_id: &str,
    item_id: &str,
    date: &str,
) -> Result<Vec<SportsResource>, SportsParseError> {
    let trimmed = guard_page(html)?;
    let hashes = script_string_calls(trimmed, "resourcesm.put", 2);
    let costs = script_string_calls(trimmed, "addCost", 2);
    let statuses = script_string_calls(trimmed, "markResStatus", 3);
    let colours = script_string_calls(trimmed, "markStatusColor", 4);

    let mut resources: Vec<SportsResource> = Vec::new();
    for entry in script_resource_entries(trimmed) {
        if resources.len() >= MAX_RESOURCES {
            return Err(SportsParseError::TooLarge);
        }
        let index = resources.len();
        let value = |key: &str| entry.iter().find(|(name, _)| *name == key).map(|(_, v)| *v);
        let Some(res_id) = value("id").filter(|id| !id.is_empty()) else {
            return Err(SportsParseError::UnrecognizedSlot { index });
        };
        let Some(res_hash) = hashes
            .iter()
            .find(|args| args[0] == res_id)
            .map(|args| args[1])
        else {
            // The page never named a hash for this slot, so the slot is not
            // one this client can report completely.
            continue;
        };
        let cost = costs
            .iter()
            .find(|args| args[0] == res_id)
            .map(|args| bounded_script_value(args[1]));
        let status = statuses.iter().find(|args| args[1] == res_id);
        let colour = colours.iter().find(|args| args[0] == res_id);
        resources.push(SportsResource {
            res_id: bounded_script_value(res_id),
            res_hash: bounded_script_value(res_hash),
            time_session: bounded_script_value(value("time_session").unwrap_or_default()),
            field_name: bounded_script_value(value("field_name").unwrap_or_default()),
            overlay_size: value("overlaySize").and_then(|size| size.parse::<u32>().ok()),
            can_net_book: value("can_net_book") == Some("1"),
            cost,
            book_id: status.map(|args| bounded_script_value(args[0])),
            locked: status.map(|args| args[2] == "1"),
            user_type: colour.map(|args| bounded_script_value(args[1])),
            payment_status: colour.map(|args| args[2] == "1"),
            gym_id: gym_id.to_owned(),
            item_id: item_id.to_owned(),
            date: date.to_owned(),
            // The handle is minted by the read that retains this row, not by the
            // parser: a raw parse has no Runtime and therefore no account to
            // bind a selector to.
            selector: None,
        });
    }
    Ok(resources)
}

/// Parses the unpaid reservation table.
///
/// The row's landmark is its method cell — the one cell whose text is one of
/// the two payment methods the table prints.  Ambiguity is an error rather than
/// a choice, and a row without a landmark is unrecognized: see the module
/// documentation for why this module owns that decision.
pub fn parse_sports_unpaid_records_html(
    html: &str,
) -> Result<Vec<SportsReservationRecord>, SportsParseError> {
    let trimmed = guard_page(html)?;
    let tables = campus_html::scan(trimmed, "table")?;
    if tables.is_empty() {
        return Err(SportsParseError::MissingTable);
    }
    let mut records = Vec::new();
    let mut row_index = 0usize;
    for body in campus_html::scan(trimmed, "tbody")? {
        for row in campus_html::direct_children(body.inner(), "tr")? {
            // A hidden row is not displayed, so it is not a reservation row of
            // this table; the paid table uses that shape for its own carriers.
            if row
                .attr("style")
                .is_some_and(|style| style.replace(' ', "").contains("display:none"))
            {
                continue;
            }
            if records.len() >= MAX_RECORDS {
                return Err(SportsParseError::TooLarge);
            }
            let cells = campus_html::direct_children(row.inner(), "td")?;
            if cells.is_empty() {
                continue;
            }
            records.push(unpaid_row(&cells, row_index)?);
            row_index += 1;
        }
    }
    Ok(records)
}

/// Parses the paid reservation table.
///
/// A settled reservation is carried by a `tr` hidden with
/// `style="display:none"` that holds a nested `tbody` whose first row carries
/// the cells.  The carrier shape is the only structural evidence this table
/// offers, so a carrier that does not match is unrecognized rather than a
/// partly-filled record.
pub fn parse_sports_paid_records_html(
    html: &str,
) -> Result<Vec<SportsReservationRecord>, SportsParseError> {
    let trimmed = guard_page(html)?;
    let tables = campus_html::scan(trimmed, "table")?;
    if tables.is_empty() {
        return Err(SportsParseError::MissingTable);
    }
    let mut records = Vec::new();
    let mut row_index = 0usize;
    for carrier in campus_html::scan(trimmed, "tr")? {
        let hidden = carrier
            .attr("style")
            .is_some_and(|style| style.replace(' ', "").contains("display:none"));
        if !hidden {
            continue;
        }
        if records.len() >= MAX_RECORDS {
            return Err(SportsParseError::TooLarge);
        }
        let bodies = campus_html::direct_children(carrier.inner(), "tbody")?;
        let Some(body) = bodies.first() else {
            return Err(SportsParseError::UnrecognizedRow { row: row_index });
        };
        let rows = campus_html::direct_children(body.inner(), "tr")?;
        let Some(row) = rows.first() else {
            return Err(SportsParseError::UnrecognizedRow { row: row_index });
        };
        let cells = campus_html::direct_children(row.inner(), "td")?;
        records.push(paid_row(&cells, row_index)?);
        row_index += 1;
    }
    Ok(records)
}

/// Reads one unpaid row relative to its method landmark.
fn unpaid_row(
    cells: &[RawElement],
    row_index: usize,
) -> Result<SportsReservationRecord, SportsParseError> {
    let mut landmark = None;
    for (index, cell) in cells.iter().enumerate() {
        if UNPAID_METHODS.contains(&cell.text().as_str()) {
            if landmark.is_some() {
                // Two method cells means the columns moved; choosing one would
                // silently shift every value on the row.
                return Err(SportsParseError::UnrecognizedRow { row: row_index });
            }
            landmark = Some(index);
        }
    }
    let Some(method_index) = landmark else {
        return Err(SportsParseError::UnrecognizedRow { row: row_index });
    };
    // The four value cells sit at fixed distances before the landmark and the
    // action cell two after it, which is where the observed table places them.
    if method_index < VALUE_OFFSETS.iter().copied().max().unwrap_or_default()
        || cells.len() <= method_index + ACTION_OFFSET
    {
        return Err(SportsParseError::UnrecognizedRow { row: row_index });
    }
    let value = |offset: usize| cells[method_index - offset].text();
    let method = cells[method_index].text();
    let action = cells[method_index + ACTION_OFFSET].inner();
    let book_timestamp = action_span_time(action);
    // The two methods offer different actions, and the row's own method decides
    // which identifiers it can carry.
    let (book_id, pay_id) = if method == UNPAID_METHODS[0] {
        (
            first_call_argument(action, "unsubscribeOnline"),
            first_call_argument(action, "payNow"),
        )
    } else {
        (first_call_argument(action, "unsubscribe"), None)
    };
    Ok(SportsReservationRecord {
        name: bounded_script_value(&value(NAME_OFFSET)),
        field: bounded_script_value(&value(FIELD_OFFSET)),
        time: bounded_script_value(&value(TIME_OFFSET)),
        price: bounded_script_value(&value(PRICE_OFFSET)),
        method: bounded_script_value(&method),
        book_timestamp,
        book_id,
        pay_id,
        // The handle is minted by the read that retains this row, not by the
        // parser: a raw parse has no Runtime and therefore no account to bind a
        // selector to.
        selector: None,
    })
}

/// Reads one settled row from the cells the carrier holds.
fn paid_row(
    cells: &[RawElement],
    row_index: usize,
) -> Result<SportsReservationRecord, SportsParseError> {
    if cells.len() <= PAID_PRICE_CELL {
        return Err(SportsParseError::UnrecognizedRow { row: row_index });
    }
    Ok(SportsReservationRecord {
        name: bounded_script_value(&cells[PAID_NAME_CELL].text()),
        field: bounded_script_value(&cells[PAID_FIELD_CELL].text()),
        time: bounded_script_value(&cells[PAID_TIME_CELL].text()),
        price: bounded_script_value(&cells[PAID_PRICE_CELL].text()),
        method: PAID_METHOD.to_owned(),
        book_timestamp: None,
        book_id: None,
        pay_id: None,
        selector: None,
    })
}

/// The distances from the unpaid row's method landmark to the cells the
/// reference reports.
const NAME_OFFSET: usize = 8;
const FIELD_OFFSET: usize = 6;
const TIME_OFFSET: usize = 4;
const PRICE_OFFSET: usize = 2;
const VALUE_OFFSETS: [usize; 4] = [NAME_OFFSET, FIELD_OFFSET, TIME_OFFSET, PRICE_OFFSET];
/// The distance from the landmark to the row's action cell.
const ACTION_OFFSET: usize = 2;

/// The cells the paid carrier's row carries, as the observed table places them.
const PAID_NAME_CELL: usize = 2;
const PAID_FIELD_CELL: usize = 3;
const PAID_TIME_CELL: usize = 4;
const PAID_PRICE_CELL: usize = 5;

/// Reads the `time` attribute of the first `span` inside an action cell.
fn action_span_time(fragment: &str) -> Option<i64> {
    let spans = campus_html::scan(fragment, "span").ok()?;
    spans
        .iter()
        .find_map(|span| span.attr("time"))
        .and_then(|value| value.trim().parse::<i64>().ok())
}

/// Reads the first argument of the first `name('…')` call in a fragment.
fn first_call_argument(fragment: &str, name: &str) -> Option<String> {
    script_string_calls(fragment, name, 1)
        .into_iter()
        .next()
        .map(|args| bounded_script_value(args[0]))
}

/// Keeps one value inside the bounded public shape.
fn bounded_script_value(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_SCRIPT_ARG_BYTES)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Reads `var <name> = '<digits>';` out of an inline script.
fn script_numeric_assignment(body: &str, name: &str) -> Option<u32> {
    let marker = format!("var {name} = '");
    let start = body.find(&marker)? + marker.len();
    let rest = &body[start..];
    let end = rest.find('\'')?;
    let digits = &rest[..end];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// Collects the string arguments of every `name('…', …)` call in a script.
///
/// Only the argument list is parsed; nothing is evaluated.  A call whose
/// arguments are not all plain quoted strings, or whose arity differs from the
/// observed one, is skipped rather than approximated.
fn script_string_calls<'a>(body: &'a str, name: &str, arity: usize) -> Vec<Vec<&'a str>> {
    let mut calls = Vec::new();
    let mut cursor = 0usize;
    while calls.len() < MAX_SCRIPT_CALLS {
        let Some(offset) = body[cursor..].find(name) else {
            break;
        };
        let start = cursor + offset;
        cursor = start + name.len();
        // The name must start at a token boundary, so `xmarkResStatus(` is not
        // read as `markResStatus(`.
        if body[..start].chars().next_back().is_some_and(|previous| {
            previous.is_alphanumeric() || previous == '_' || previous == '.'
        }) {
            continue;
        }
        let Some(rest) = body[cursor..].strip_prefix('(') else {
            continue;
        };
        let Some((args, consumed)) = parse_string_args(rest, arity) else {
            continue;
        };
        cursor += 1 + consumed;
        calls.push(args);
    }
    calls
}

/// Parses `'a','b',…` up to the closing parenthesis of a call.
///
/// The terminator is optional because the page writes these both as whole
/// statements (`…');`) and inside inline `onclick` handlers (`…')`).
fn parse_string_args(source: &str, arity: usize) -> Option<(Vec<&str>, usize)> {
    let mut index = 0usize;
    let mut args = Vec::with_capacity(arity);
    for position in 0..arity {
        if position > 0 {
            index = skip_spaces(source, index);
            index = expect_byte(source, index, b',')?;
        }
        index = skip_spaces(source, index);
        index = expect_byte(source, index, b'\'')?;
        let rest = &source[index..];
        let end = rest.find('\'')?;
        let value = &rest[..end];
        if value.chars().any(char::is_control) || value.len() > MAX_SCRIPT_ARG_BYTES {
            return None;
        }
        args.push(value);
        index += end + 1;
    }
    index = skip_spaces(source, index);
    index = expect_byte(source, index, b')')?;
    let after = skip_spaces(source, index);
    let after = match source.as_bytes().get(after) {
        Some(b';') => after + 1,
        _ => index,
    };
    Some((args, after))
}

/// Collects the field lists of every `resourceArray.push({…})` call.
fn script_resource_entries(body: &str) -> Vec<Vec<(&str, &str)>> {
    const MARKER: &str = "resourceArray.push({";
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    while entries.len() < MAX_SCRIPT_CALLS {
        let Some(offset) = body[cursor..].find(MARKER) else {
            break;
        };
        let start = cursor + offset + MARKER.len();
        cursor = start;
        let Some((fields, consumed)) = parse_object_literal(&body[start..]) else {
            continue;
        };
        cursor += consumed;
        entries.push(fields);
    }
    entries
}

/// Parses `key:'value',…` up to the object literal's closing brace.
fn parse_object_literal(source: &str) -> Option<(Vec<(&str, &str)>, usize)> {
    let mut index = 0usize;
    let mut fields = Vec::new();
    loop {
        index = skip_spaces(source, index);
        if source.as_bytes().get(index) == Some(&b'}') {
            index += 1;
            break;
        }
        if !fields.is_empty() {
            index = expect_byte(source, index, b',')?;
            index = skip_spaces(source, index);
        }
        let key_start = index;
        while source
            .as_bytes()
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        if index == key_start {
            return None;
        }
        let key = &source[key_start..index];
        index = skip_spaces(source, index);
        index = expect_byte(source, index, b':')?;
        index = skip_spaces(source, index);
        index = expect_byte(source, index, b'\'')?;
        let rest = &source[index..];
        let end = rest.find('\'')?;
        let value = &rest[..end];
        if value.chars().any(char::is_control) || value.len() > MAX_SCRIPT_ARG_BYTES {
            return None;
        }
        fields.push((key, value));
        index += end + 1;
        if fields.len() > MAX_OBJECT_FIELDS {
            return None;
        }
    }
    index = skip_spaces(source, index);
    index = expect_byte(source, index, b')')?;
    let after = skip_spaces(source, index);
    let after = match source.as_bytes().get(after) {
        Some(b';') => after + 1,
        _ => index,
    };
    Some((fields, after))
}

fn skip_spaces(source: &str, mut index: usize) -> usize {
    let bytes = source.as_bytes();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn expect_byte(source: &str, index: usize, expected: u8) -> Option<usize> {
    (source.as_bytes().get(index) == Some(&expected)).then_some(index + 1)
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, SportsAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(SportsAdapterError::InvalidBaseUrl);
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

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, SportsAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(SportsAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| SportsAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(SportsAdapterError::UnexpectedOrigin);
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
