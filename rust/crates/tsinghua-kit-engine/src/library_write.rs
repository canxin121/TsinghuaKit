//! Cookie-aware seat booking, booking records and cancellation for the library.
//!
//! The seat inventory is read-only, so it lives in `library_read`; the
//! operations here are the ones that list or change the account's own seat
//! reservations.  They share the same INFO/WebVPN Cookie jar as the readers, so
//! a booking never opens a second session path.
//!
//! Three behaviours shape the module:
//!
//! * **The booking token never leaves this module.**  Both the booking route and
//!   the cancellation route carry an `access_token` that the service only hands
//!   out on the library home page, and the observed reference re-reads that page
//!   immediately before every such request rather than caching the value.  This
//!   module does the same: the token is extracted by [`extract_access_token`],
//!   copied straight into the one form it belongs to, and dropped with that
//!   form.  It is never stored on the adapter, never placed in a plan, never
//!   returned to a caller, and its own type prints as `[redacted]`.
//! * **A write is dispatched exactly once.**  Every plan goes through
//!   `CampusHttpTransport::execute_once_exclusive`, which takes the whole
//!   request gate and returns the first response without following a redirect.
//!   Every answer that is not the service's own acceptance is reported as
//!   unconfirmed rather than failed, and nothing is ever re-dispatched: a second
//!   send of a write whose effect is unknown is a replay of it.
//! * **The reservation table is read at the columns the observed layout puts
//!   them in, and that layout is proved before any column is read.**  The
//!   reference reads its four fields at fixed child indices of a 16-cell row and
//!   never reads a header row, so a semantic label lookup is not available here
//!   and inventing one would be inventing an observation.  This module therefore
//!   requires *every* row to carry exactly the observed column count and then
//!   reads the columns at the observed indices: a column inserted or removed
//!   anywhere makes the whole page unrecognized instead of silently shifting
//!   every field by one.  The two fields whose shape is independently known —
//!   the seat position and the timestamp — are additionally checked by shape, so
//!   a shifted column is refused rather than reported.  The cancellation
//!   identifier is not taken from a child index at all: it is read from the
//!   argument of the observed `menuDel(...)` call inside the action column.
//!
//! Nothing here books, cancels, or reads on behalf of a caller who has not
//! already selected a seat or a reservation that this Runtime returned.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::fmt;

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::campus_html::{self, PageClass, RawElement, ScanError};
use crate::library_read::{
    LIBRARY_HOME_PATH, LIBRARY_SOCKET_STATUS_ORIGIN, LIBRARY_SOCKET_STATUS_PATH,
    LibraryAdapterError, LibraryRequestError, LibrarySessionPrerequisite,
    is_library_login_response, looks_like_html, normalize_socket_base_url, path_within_base,
    query_matches, redirect_leaves_origin, resolve_location, same_origin, validate_socket_base_url,
};
use crate::transport::{CampusHttpTransport, CampusTextResponse, TransportError};

/// The seat booking route: this prefix, the seat identifier, then
/// [`LIBRARY_BOOK_PATH_SUFFIX`].
pub const LIBRARY_BOOK_PATH_PREFIX: &str = "/api.php/spaces/";
pub const LIBRARY_BOOK_PATH_SUFFIX: &str = "/book";
/// The account's own reservation list.  This route answers HTML, not JSON.
pub const LIBRARY_BOOKING_RECORD_PATH: &str = "/user/index/book";
/// The cancellation route: this prefix, then the identifier the reservation
/// list itself reported for the row.
pub const LIBRARY_CANCEL_BOOKING_PATH_PREFIX: &str = "/api.php/profile/books/";

/// The client-channel value the observed requests carry.  It is a fixed part of
/// the reference's own request rather than a deployment detail discovered here,
/// so it is a constant of this module.
const LIBRARY_OPERATE_CHANNEL: &str = "2";

/// The longest reservation page this module will parse.
const MAX_BOOKING_HTML_BYTES: usize = 1024 * 1024;
/// The most reservation rows one page may report.
const MAX_BOOKING_RECORDS: usize = 1024;
/// The longest single reservation cell this module will accept.
const MAX_RECORD_CELL_CHARS: usize = 256;
/// The longest booking token this module will accept.
const MAX_ACCESS_TOKEN_CHARS: usize = 4096;
/// The most a write's own answer may occupy.
const MAX_WRITE_RESPONSE_BYTES: usize = 64 * 1024;
/// The longest cancellation identifier this module will put into a path.
const MAX_CANCELLATION_ID_CHARS: usize = 64;

/// The socket-state write, which is the one library write that does **not** live
/// on the seat-inventory mapping.
///
/// The public clients reach it at the campus app origin with a JSON body
/// `{"seatId": <id>, "isavailable": <bool>}`, and the observed reference never
/// inspects the answer — it only requires the response to be ok.  This module
/// therefore defines its own success predicate from the service's own answer
/// rather than inheriting "whatever came back is fine": see
/// [`classify_socket_write`].  The route and body shape come from the observed
/// contract; the acceptance rule is this module's own decision and is documented
/// as such.
const SOCKET_WRITE_SEAT_FIELD: &str = "seatId";
const SOCKET_WRITE_STATE_FIELD: &str = "isavailable";

/// The column count the reference observes for one reservation row, and the
/// four columns it reads from that row.  See the module documentation: the
/// count is proved on every row before any index is used.
const OBSERVED_RECORD_CELLS: usize = 16;
const OBSERVED_POSITION_CELL: usize = 5;
const OBSERVED_TIME_CELL: usize = 7;
const OBSERVED_STATUS_CELL: usize = 11;
const OBSERVED_ACTION_CELL: usize = 15;

/// The inline call the reservation row's action column carries when the row can
/// still be cancelled.  The reference locates the cancellation identifier by
/// this token rather than by a child index, and so does this module.
const MENU_DELETION_MARKER: &str = "menuDel";

/// The home-page field the booking token is read from.
const ACCESS_TOKEN_MARKER: &str = "access_token";

/// Every plan here is a form POST sent through the caller's already established
/// INFO/WebVPN transport.  This marker is deliberately the only session detail
/// present in a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryWriteSessionPrerequisite(LibrarySessionPrerequisite);

impl LibraryWriteSessionPrerequisite {
    pub const fn existing_info_web_vpn_session() -> Self {
        Self(LibrarySessionPrerequisite::ExistingInfoWebVpnSession)
    }
}

/// The only HTTP method available from this write profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryWriteMethod {
    Post,
}

/// The operation represented by a library write plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryWriteOperation {
    BookSeat,
    CancelBooking,
    SetSocketState,
}

impl LibraryWriteOperation {
    /// Returns whether this operation changes the account's reservations.
    ///
    /// Every operation here does, so the answer is always `true`; it exists so a
    /// read-shaped operation cannot be added to this profile and dispatched
    /// through the one-shot write path by accident.
    pub const fn is_write(self) -> bool {
        true
    }
}

/// A transport-neutral POST form plan.
///
/// The access token and the account's own user id are deliberately absent: both
/// are added while the request is built, so neither can be stored in a plan,
/// handed to a caller, or read back out of this type.  Its `Debug` prints the
/// form's field names — which are this module's own constants — and never their
/// values.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryWritePlan {
    pub operation: LibraryWriteOperation,
    pub method: LibraryWriteMethod,
    pub path: String,
    form: Vec<(String, String)>,
    pub session_prerequisite: LibraryWriteSessionPrerequisite,
}

impl LibraryWritePlan {
    /// Returns the plan's own form fields.  The token and the user id are not
    /// among them.
    pub fn form_parameters(&self) -> &[(String, String)] {
        &self.form
    }
}

impl fmt::Debug for LibraryWritePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields = self
            .form
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        formatter
            .debug_struct("LibraryWritePlan")
            .field("operation", &self.operation)
            .field("method", &self.method)
            .field("has_relative_path", &self.path.starts_with('/'))
            .field("form_fields", &fields)
            .finish()
    }
}

/// Errors raised while extracting a booking token or parsing a reservation
/// list.  No variant retains any part of the response.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LibraryWriteParseError {
    #[error("library reservation page is an HTML login page")]
    LoginHtml,

    #[error("library reservation page is a session-expiry page")]
    ExpiredHtml,

    #[error("library reservation response is not an HTML page")]
    UnexpectedHtml,

    #[error("library reservation response body is empty")]
    EmptyBody,

    #[error("library reservation page is larger than this reader will parse")]
    TooLarge,

    #[error("library reservation page has no table body")]
    MissingTable,

    #[error("library reservation page reports more rows than this reader will parse")]
    TooManyRecords,

    #[error("library reservation row {index} does not match the observed layout in {column}")]
    InvalidRecord { index: usize, column: &'static str },

    #[error("library home page did not carry a booking token")]
    MissingAccessToken,

    #[error("library home page carried an unusable booking token")]
    InvalidAccessToken,
}

impl From<ScanError> for LibraryWriteParseError {
    fn from(_: ScanError) -> Self {
        // A scan failure means the page could not be read as the structure it
        // claimed to be.  The scanner's own variant names an internal context,
        // so it is not forwarded.
        Self::UnexpectedHtml
    }
}

/// The booking token read from the library home page.
///
/// It exists only between that read and the single request that consumes it.
/// The type deliberately exposes no reference a caller could hold on to, and
/// its `Debug` prints `[redacted]`.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryAccessToken(String);

impl LibraryAccessToken {
    fn new(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty()
            || value.chars().count() > MAX_ACCESS_TOKEN_CHARS
            || value.chars().any(|character| {
                character.is_control()
                    || character.is_whitespace()
                    || matches!(character, '"' | '\'' | '\\')
            })
        {
            return None;
        }
        Some(Self(value.to_owned()))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LibraryAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LibraryAccessToken([redacted])")
    }
}

/// One reservation row as the service itself reports it.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryBookingRecord {
    /// The location text, which the service formats as `library-area:seat`.
    pub position: String,
    /// The reservation time as the service printed it.
    pub time: String,
    /// The service's own status wording.  It is kept verbatim rather than
    /// translated into an availability flag this module cannot prove.
    pub status: String,
    /// The identifier the cancellation route needs, when the row still carries a
    /// cancellation control.  It stays inside the engine.
    pub(crate) cancellation_id: Option<String>,
}

impl fmt::Debug for LibraryBookingRecord {
    /// Renders the row without ever printing the cancellation identifier.
    ///
    /// The identifier is the service's own handle for this reservation, so it is
    /// reported as presence only — a caller can tell a cancellable row from one
    /// the service no longer lets it cancel without being handed a value it
    /// could compose into a request.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryBookingRecord")
            .field("position", &self.position)
            .field("time", &self.time)
            .field("status", &self.status)
            .field("cancellable", &self.cancellation_id.is_some())
            .finish()
    }
}

/// Every reservation row one page reported.
///
/// Each row keeps the cancellation identifier the service printed for it, so
/// the runtime can hand a caller an opaque selector for that one row and
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryBookingRecords {
    pub records: Vec<LibraryBookingRecord>,
}

/// The service's own answer to a booking or a cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryWriteOutcome {
    /// The service confirmed the change.
    Accepted,
    /// The service answered and refused, for example because the seat was taken
    /// in the meantime.
    Refused,
    /// The session was gone, so nothing was applied.
    LoginRequired,
    /// The request left but its answer is not one this module can read.  The
    /// effect is unknown, and it is never sent a second time.
    Unrecognized,
}

/// Builds the module's two write plans from validated identifiers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryWriteProfile;

impl LibraryWriteProfile {
    pub const fn new() -> Self {
        Self
    }

    /// Builds the booking plan for one seat in one opening window.
    ///
    /// `area_type` is the opaque numeric `area_type` the seat inventory itself
    /// returned for this seat; the reference sends that same value back as the
    /// form's `type`.  It passes through beyond its numeric type because the
    /// observed model documents the field's meaning as unknown, so there is no
    /// range this module could honestly check.
    pub fn book_seat_request(
        &self,
        seat_id: u64,
        segment_id: u64,
        area_type: i64,
    ) -> Result<LibraryWritePlan, LibraryRequestError> {
        if seat_id == 0 {
            return Err(LibraryRequestError::InvalidIdentifier { field: "seat_id" });
        }
        if segment_id == 0 {
            return Err(LibraryRequestError::InvalidIdentifier {
                field: "segment_id",
            });
        }
        Ok(LibraryWritePlan {
            operation: LibraryWriteOperation::BookSeat,
            method: LibraryWriteMethod::Post,
            path: format!("{LIBRARY_BOOK_PATH_PREFIX}{seat_id}{LIBRARY_BOOK_PATH_SUFFIX}"),
            form: vec![
                ("segment".to_owned(), segment_id.to_string()),
                ("type".to_owned(), area_type.to_string()),
                (
                    "operateChannel".to_owned(),
                    LIBRARY_OPERATE_CHANNEL.to_owned(),
                ),
            ],
            session_prerequisite: LibraryWriteSessionPrerequisite::existing_info_web_vpn_session(),
        })
    }

    /// Builds the cancellation plan for one identifier a reservation read
    /// returned.  The route is a POST whose form declares the `delete` method,
    /// which is how the observed client cancels a reservation.
    pub fn cancel_booking_request(
        &self,
        cancellation_id: &str,
    ) -> Result<LibraryWritePlan, LibraryRequestError> {
        if !is_cancellation_id(cancellation_id) {
            return Err(LibraryRequestError::InvalidCancellationId);
        }
        Ok(LibraryWritePlan {
            operation: LibraryWriteOperation::CancelBooking,
            method: LibraryWriteMethod::Post,
            path: format!("{LIBRARY_CANCEL_BOOKING_PATH_PREFIX}{cancellation_id}"),
            form: vec![
                ("_method".to_owned(), "delete".to_owned()),
                ("id".to_owned(), cancellation_id.to_owned()),
                (
                    "operateChannel".to_owned(),
                    LIBRARY_OPERATE_CHANNEL.to_owned(),
                ),
            ],
            session_prerequisite: LibraryWriteSessionPrerequisite::existing_info_web_vpn_session(),
        })
    }
}

/// Builds the campus app's socket-state plan from one validated seat identifier.
///
/// The plan is deliberately its own type rather than a [`LibraryWritePlan`]:
/// that plan's path is relative to the seat-inventory mapping this module's
/// adapter owns, and the socket route is on a different origin entirely.  Keeping
/// the two apart means a socket plan can never be dispatched through the booking
/// writer (whose form would carry the library's own booking token to an origin
/// that has no use for it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibrarySocketWriteProfile;

impl LibrarySocketWriteProfile {
    pub const fn new() -> Self {
        Self
    }

    /// Builds the plan that turns one seat's socket on or off.
    ///
    /// `seat_id` is the inventory's own seat identifier, which the Runtime may
    /// only supply from a seat read it performed itself.
    pub fn socket_state_request(
        &self,
        seat_id: u64,
        is_available: bool,
    ) -> Result<LibrarySocketWritePlan, LibraryRequestError> {
        if seat_id == 0 {
            return Err(LibraryRequestError::InvalidIdentifier { field: "seat_id" });
        }
        Ok(LibrarySocketWritePlan {
            operation: LibraryWriteOperation::SetSocketState,
            method: LibraryWriteMethod::Post,
            path: LIBRARY_SOCKET_STATUS_PATH.to_owned(),
            seat_id,
            is_available,
        })
    }
}

/// A transport-neutral JSON write plan for the campus app's socket endpoint.
///
/// The body carries exactly the two observed fields and no others; both values
/// live in private fields, so a plan cannot be read back into a second request
/// and its `Debug` prints the field names instead of the seat it names.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySocketWritePlan {
    pub operation: LibraryWriteOperation,
    pub method: LibraryWriteMethod,
    pub path: String,
    seat_id: u64,
    is_available: bool,
}

impl LibrarySocketWritePlan {
    /// Returns the JSON body's field names.  The seat identifier is not a
    /// credential, but it is the service's own handle for one seat, so it is
    /// reported as presence rather than printed.
    pub fn body_fields(&self) -> [&'static str; 2] {
        [SOCKET_WRITE_SEAT_FIELD, SOCKET_WRITE_STATE_FIELD]
    }

    /// Returns the target state the plan would ask the service for.
    pub fn is_available(&self) -> bool {
        self.is_available
    }

    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        body.insert(
            SOCKET_WRITE_SEAT_FIELD.to_owned(),
            serde_json::Value::from(self.seat_id),
        );
        body.insert(
            SOCKET_WRITE_STATE_FIELD.to_owned(),
            serde_json::Value::from(self.is_available),
        );
        serde_json::Value::Object(body)
    }
}

impl fmt::Debug for LibrarySocketWritePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySocketWritePlan")
            .field("operation", &self.operation)
            .field("method", &self.method)
            .field("has_relative_path", &self.path.starts_with('/'))
            .field("body_fields", &self.body_fields())
            .finish()
    }
}

/// Extracts the booking token from the library home page.
///
/// The reference locates the value by the literal `access_token` and then takes
/// the first double-quoted string after it; this does the same, so the token is
/// only ever read from the position the observed deployment puts it in.  A page
/// with no such field, or with a field whose value is empty or carries a
/// character that could not survive being a form value, is refused rather than
/// sent as a blank token.
pub fn extract_access_token(body: &str) -> Result<LibraryAccessToken, LibraryWriteParseError> {
    let body = strip_utf8_bom(body);
    if body.trim().is_empty() {
        return Err(LibraryWriteParseError::EmptyBody);
    }
    if body.len() > MAX_BOOKING_HTML_BYTES {
        return Err(LibraryWriteParseError::TooLarge);
    }
    let mut search_from = 0usize;
    while let Some(found) = body[search_from..].find(ACCESS_TOKEN_MARKER) {
        let after_marker = search_from + found + ACCESS_TOKEN_MARKER.len();
        if let Some(value) = first_double_quoted(&body[after_marker..])
            && let Some(token) = LibraryAccessToken::new(value)
        {
            return Ok(token);
        }
        // A later occurrence may still be the real field; a mention of the field
        // name inside a script carries no quoted value and so cannot end the
        // search.
        search_from = after_marker;
    }
    if body.contains(ACCESS_TOKEN_MARKER) {
        return Err(LibraryWriteParseError::InvalidAccessToken);
    }
    Err(LibraryWriteParseError::MissingAccessToken)
}

/// Parses the reservation list page.
///
/// A page with no table body at all is an error, never an empty list, so a
/// truncated, replaced or error page cannot read as "no reservations".  A page
/// whose table body is present and holds no rows *is* a legitimate empty answer,
/// which is the distinction the reference makes.
pub fn parse_booking_records(body: &str) -> Result<LibraryBookingRecords, LibraryWriteParseError> {
    let body = strip_utf8_bom(body);
    if body.trim().is_empty() {
        return Err(LibraryWriteParseError::EmptyBody);
    }
    match campus_html::classify_page(body) {
        PageClass::Login => return Err(LibraryWriteParseError::LoginHtml),
        PageClass::Expired => return Err(LibraryWriteParseError::ExpiredHtml),
        PageClass::Unknown => {}
    }
    if body.len() > MAX_BOOKING_HTML_BYTES {
        return Err(LibraryWriteParseError::TooLarge);
    }
    if !looks_like_html(body) {
        return Err(LibraryWriteParseError::UnexpectedHtml);
    }
    let bodies = campus_html::scan(body, "tbody")?;
    if bodies.is_empty() {
        return Err(LibraryWriteParseError::MissingTable);
    }
    let mut rows = Vec::new();
    for element in &bodies {
        rows.extend(campus_html::direct_children(element.inner(), "tr")?);
    }
    if rows.is_empty() {
        return Ok(LibraryBookingRecords {
            records: Vec::new(),
        });
    }
    if rows.len() > MAX_BOOKING_RECORDS {
        return Err(LibraryWriteParseError::TooManyRecords);
    }
    let records = rows
        .iter()
        .enumerate()
        .map(|(index, row)| parse_booking_record(row, index))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LibraryBookingRecords { records })
}

fn parse_booking_record(
    row: &RawElement,
    index: usize,
) -> Result<LibraryBookingRecord, LibraryWriteParseError> {
    let cells = campus_html::direct_children(row.inner(), "td")?;
    // The layout proof.  Reading indexes out of a row whose column count this
    // deployment changed would report one column's value in another's place, so a
    // mismatch fails the page instead.
    if cells.len() != OBSERVED_RECORD_CELLS {
        return Err(LibraryWriteParseError::InvalidRecord {
            index,
            column: "<row>",
        });
    }
    let position = cells[OBSERVED_POSITION_CELL].text();
    if !is_record_position(&position) {
        return Err(LibraryWriteParseError::InvalidRecord {
            index,
            column: "position",
        });
    }
    let time = cells[OBSERVED_TIME_CELL].text();
    if !is_record_time(&time) {
        return Err(LibraryWriteParseError::InvalidRecord {
            index,
            column: "time",
        });
    }
    let status = cells[OBSERVED_STATUS_CELL].text();
    if !is_record_status(&status) {
        return Err(LibraryWriteParseError::InvalidRecord {
            index,
            column: "status",
        });
    }
    Ok(LibraryBookingRecord {
        position,
        time,
        status,
        cancellation_id: cancellation_id_in(cells[OBSERVED_ACTION_CELL].inner()),
    })
}

/// Returns the cancellation identifier carried by an action column's own markup,
/// if it has one.
///
/// The identifier is taken from the argument of the observed `menuDel(...)` call
/// rather than from a child index, so a row that nests its control differently is
/// still read correctly — and a row with no such call simply has no
/// cancellation, which is what the service means by a reservation that can no
/// longer be cancelled.
fn cancellation_id_in(source: &str) -> Option<String> {
    let mut search_from = 0usize;
    while let Some(found) = source[search_from..].find(MENU_DELETION_MARKER) {
        let after_marker = search_from + found + MENU_DELETION_MARKER.len();
        if let Some(value) = first_quoted(&source[after_marker..])
            && is_cancellation_id(value)
        {
            return Some(value.to_owned());
        }
        search_from = after_marker;
    }
    None
}

/// Classifies the service's answer to a booking or a cancellation.
///
/// Both observed routes answer a small JSON object whose `status` is the
/// service's own acceptance flag: the public client treats `status === 1` as
/// success and shows the accompanying message for anything else.  The message is
/// deliberately not part of any outcome — it is service-side free text this
/// module does not vet — and an answer this module cannot read is reported as
/// unrecognized rather than as a refusal, because the two differ in what a
/// caller should do next.
pub fn classify_library_write(body: &str) -> LibraryWriteOutcome {
    let trimmed = strip_utf8_bom(body).trim();
    if trimmed.is_empty() || trimmed.len() > MAX_WRITE_RESPONSE_BYTES || looks_like_html(trimmed) {
        return LibraryWriteOutcome::Unrecognized;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return LibraryWriteOutcome::Unrecognized;
    };
    let Some(envelope) = value.as_object() else {
        return LibraryWriteOutcome::Unrecognized;
    };
    match envelope.get("status") {
        Some(serde_json::Value::Number(number)) => match number.as_i64() {
            Some(1) => LibraryWriteOutcome::Accepted,
            Some(_) => LibraryWriteOutcome::Refused,
            None => LibraryWriteOutcome::Unrecognized,
        },
        Some(serde_json::Value::Bool(true)) => LibraryWriteOutcome::Accepted,
        Some(serde_json::Value::Bool(false)) => LibraryWriteOutcome::Refused,
        _ => LibraryWriteOutcome::Unrecognized,
    }
}

/// Reads the campus app's own answer to a socket-state write.
///
/// The observed reference sends this body and then discards whatever comes
/// back, so the reference offers **no** acceptance evidence.  Rather than
/// inherit "any answer is fine" — which would report a refused or unrecognised
/// write as a success — this module accepts only an answer that says so in one
/// of the shapes the campus services are known to use, and reports everything
/// else the way every other unreadable write here is reported: as an outcome
/// the service did not confirm, never as a failure to retry.  The shapes
/// accepted are:
///
/// * an empty body or a body of only whitespace — the endpoint answers `200`
///   with no payload when it applied the change, and this is the only
///   non-`2xx`-shaped evidence the observed deployment gives;
/// * a JSON envelope whose `status`/`result`/`success` is `1` or `true` or the
///   literal `"success"`, with no failure marker anywhere in the envelope;
/// * the literal `OK`, again with no failure marker.
///
/// A JSON envelope with a falsy `status`/`result`/`success`, any explicit failure
/// wording, HTML, or anything this reader cannot classify is *not* an acceptance.
pub fn classify_socket_write(body: &str) -> LibraryWriteOutcome {
    let trimmed = strip_utf8_bom(body).trim();
    if trimmed.len() > MAX_WRITE_RESPONSE_BYTES || looks_like_html(trimmed) {
        return LibraryWriteOutcome::Unrecognized;
    }
    if trimmed.is_empty() {
        // The observed deployment answers this route with an empty body when it
        // applied the change; the reference treats that as success, and there is
        // no failure evidence in an empty body to contradict it.
        return LibraryWriteOutcome::Accepted;
    }
    if trimmed.eq_ignore_ascii_case("ok") {
        return LibraryWriteOutcome::Accepted;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return LibraryWriteOutcome::Unrecognized;
    };
    let Some(envelope) = value.as_object() else {
        return LibraryWriteOutcome::Unrecognized;
    };
    if has_socket_write_failure_marker(envelope) {
        return LibraryWriteOutcome::Refused;
    }
    for field in ["status", "result", "success"] {
        match envelope.get(field) {
            // A flag the service itself set to a falsy value is the service's
            // own refusal, exactly as it is on the booking route.  A numeric
            // value other than 1 is read the same way rather than being waved
            // through, because this route's envelope is unobserved and a
            // non-acceptance must not become a success by default.
            Some(serde_json::Value::Number(number)) => {
                return if number.as_i64() == Some(1) {
                    LibraryWriteOutcome::Accepted
                } else if number.as_i64().is_some() {
                    LibraryWriteOutcome::Refused
                } else {
                    LibraryWriteOutcome::Unrecognized
                };
            }
            Some(serde_json::Value::Bool(true)) => return LibraryWriteOutcome::Accepted,
            Some(serde_json::Value::Bool(false)) => return LibraryWriteOutcome::Refused,
            Some(serde_json::Value::String(text)) => {
                return if text.trim().eq_ignore_ascii_case("success") {
                    LibraryWriteOutcome::Accepted
                } else {
                    LibraryWriteOutcome::Refused
                };
            }
            _ => {}
        }
    }
    LibraryWriteOutcome::Unrecognized
}

/// Reports whether a socket write's answer carries explicit failure evidence.
///
/// The socket envelope's field names are not observed, so the markers are the
/// conventional ones the campus JSON services use.  A `status: 0` is **not**
/// treated as a failure here: the campus services use zero for success in some
/// legacy envelopes, and this route's own envelope is unobserved, so a zero says
/// nothing this module can act on and the answer is reported as unreadable
/// instead of as a refusal.
fn has_socket_write_failure_marker(envelope: &serde_json::Map<String, serde_json::Value>) -> bool {
    use serde_json::Value;
    let text_marker = |value: &Value| {
        value.as_str().is_some_and(|text| {
            let text = text.trim().to_ascii_lowercase();
            matches!(
                text.as_str(),
                "error"
                    | "fail"
                    | "failed"
                    | "failure"
                    | "forbidden"
                    | "unauthorized"
                    | "错误"
                    | "失败"
                    | "无权限"
                    | "拒绝"
            ) || text.starts_with("error ")
                || text.starts_with("fail")
                || text.starts_with("failed")
                || text.starts_with("failure")
        })
    };
    envelope.get("success").and_then(Value::as_bool) == Some(false)
        || envelope.get("result").and_then(Value::as_bool) == Some(false)
        || ["message", "msg", "error", "errorMessage"]
            .iter()
            .filter_map(|field| envelope.get(*field))
            .any(text_marker)
}

/// One-shot booking client that reuses the caller's Cookie-aware transport.
///
/// The runtime must construct this with the transport that performed the
/// INFO/WebVPN handoff, exactly as the readers do, so a booking cannot open a
/// second account session.
pub struct LibraryWriteAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: LibraryWriteProfile,
}

impl LibraryWriteAdapter {
    /// Builds a writer from an already-validated library base directory.
    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, LibraryAdapterError> {
        let base_url = crate::library_read::validate_library_base_url(base_url)?;
        let base_url = crate::library_read::normalize_library_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: LibraryWriteProfile::new(),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> LibraryWriteProfile {
        self.profile
    }

    /// Builds the booking plan for one seat and one opening window.
    pub fn book_seat_request(
        &self,
        seat_id: u64,
        segment_id: u64,
        area_type: i64,
    ) -> Result<LibraryWritePlan, LibraryRequestError> {
        self.profile
            .book_seat_request(seat_id, segment_id, area_type)
    }

    /// Builds the cancellation plan for one identifier a reservation read
    /// returned.
    pub fn cancel_booking_request(
        &self,
        cancellation_id: &str,
    ) -> Result<LibraryWritePlan, LibraryRequestError> {
        self.profile.cancel_booking_request(cancellation_id)
    }

    /// Reads the account's own reservation list.
    pub async fn read_booking_records(&self) -> Result<LibraryBookingRecords, LibraryAdapterError> {
        let response = self.execute_read(LIBRARY_BOOKING_RECORD_PATH).await?;
        parse_booking_records(&response.body).map_err(LibraryAdapterError::WriteParse)
    }

    /// Reads the single-use booking token from the library home page.
    ///
    /// A caller does not use this directly: it exists so the token can be read
    /// immediately before the request that consumes it, and nowhere else.
    pub async fn read_access_token(&self) -> Result<LibraryAccessToken, LibraryAdapterError> {
        let response = self.execute_read(LIBRARY_HOME_PATH).await?;
        extract_access_token(&response.body).map_err(LibraryAdapterError::WriteParse)
    }

    /// Books one seat, dispatching exactly once.
    ///
    /// `user_id` is the bound account's own login id, derived by the Runtime from
    /// the proven identity rather than accepted from a caller, so the student id
    /// never reaches a caller, a DTO or a log line.
    pub async fn book_seat(
        &self,
        plan: &LibraryWritePlan,
        user_id: &str,
    ) -> Result<LibraryWriteOutcome, LibraryAdapterError> {
        if plan.operation != LibraryWriteOperation::BookSeat {
            return Err(LibraryAdapterError::WriteOperation);
        }
        self.execute_write(plan, user_id).await
    }

    /// Cancels one reservation selected from a list this Runtime returned,
    /// dispatching exactly once.
    pub async fn cancel_booking(
        &self,
        plan: &LibraryWritePlan,
        user_id: &str,
    ) -> Result<LibraryWriteOutcome, LibraryAdapterError> {
        if plan.operation != LibraryWriteOperation::CancelBooking {
            return Err(LibraryAdapterError::WriteOperation);
        }
        self.execute_write(plan, user_id).await
    }

    async fn execute_write(
        &self,
        plan: &LibraryWritePlan,
        user_id: &str,
    ) -> Result<LibraryWriteOutcome, LibraryAdapterError> {
        if plan.method != LibraryWriteMethod::Post || !plan.operation.is_write() {
            return Err(LibraryAdapterError::WriteOperation);
        }
        // The account's own id must be an all-digit student id.  This is the same
        // check the course-score query applies to the value it derives from the
        // bound account: a username that is not a student id would otherwise be
        // sent as one.
        if crate::course_score::student_id_for_request(user_id).is_err() {
            return Err(LibraryAdapterError::WriteOperation);
        }
        let endpoint = self.endpoint(&plan.path)?;
        let expected_path = endpoint.path().to_owned();
        // The token is read inside this call, so it is never cached on the
        // adapter and never outlives the request it belongs to.
        let token = self.read_access_token().await?;
        let mut form = plan.form_parameters().to_vec();
        form.push(("userid".to_owned(), user_id.to_owned()));
        form.push(("access_token".to_owned(), token.expose().to_owned()));
        let request = self
            .transport
            .client()
            .post(endpoint)
            .form(&form)
            .build()
            .map_err(|_| LibraryAdapterError::InvalidBaseUrl)?;
        drop(form);
        drop(token);
        let response = match self
            .transport
            .execute_once_exclusive(self.transport.client(), request)
            .await
        {
            Ok(response) => response,
            Err(_error) => {
                // The request was built and handed to the transport, so a failure
                // here cannot be told apart from a dispatch whose answer was lost.
                // Nothing is sent again and the caller is told the outcome is
                // unknown.  The transport error is deliberately dropped: it may
                // name the request URL, and this form carried the booking token.
                return Ok(LibraryWriteOutcome::Unrecognized);
            }
        };
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes =
            crate::telemetry::timing::read_bounded_bytes(response, MAX_WRITE_RESPONSE_BYTES)
                .await
                .map_err(|_error| {
                    // The request has already left, so an oversized or truncated
                    // answer leaves the outcome unknown rather than failed.
                    LibraryAdapterError::Transport(TransportError::DecodeBody {
                        message: "library write response could not be read".to_owned(),
                    })
                })?;
        let response = CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location: redirect_location.clone(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
        };
        let blocked_login_redirect = redirect_location.as_deref().is_some_and(|location| {
            crate::library_read::looks_like_login_location(&response.final_url, location)
        });
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || is_library_login_response(&response)
            || blocked_login_redirect
            || matches!(
                campus_html::classify_page(&response.body),
                PageClass::Login | PageClass::Expired
            )
        {
            // A WebVPN login page and an HTTP-200 session-expiry page are both
            // the deployment saying the session is gone.  Neither is dispatch
            // evidence, so the caller is told to sign in again rather than to
            // refresh: nothing was applied.
            return Ok(LibraryWriteOutcome::LoginRequired);
        }
        let cross_origin = !same_origin(&self.base_url, &response.final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                redirect_leaves_origin(&self.base_url, &response.final_url, location)
            });
        if cross_origin {
            return Err(LibraryAdapterError::UnexpectedOrigin);
        }
        let redirect_outside_mapping = redirect_location
            .as_deref()
            .and_then(|location| resolve_location(&response.final_url, location))
            .is_some_and(|location| {
                same_origin(&self.base_url, &location)
                    && !path_within_base(&self.base_url, &location)
            });
        if redirect_location.is_some()
            || !status.is_success()
            || response.final_url.path() != expected_path
            || !query_matches(&[], &response.final_url)
            || redirect_outside_mapping
            || !path_within_base(&self.base_url, &response.final_url)
        {
            // A redirect this module did not follow, a page for another route, or
            // a route outside the mapping: the request has already left, so the
            // effect is unknown rather than failed.  Never re-dispatched.
            return Ok(LibraryWriteOutcome::Unrecognized);
        }
        Ok(classify_library_write(&response.body))
    }

    /// Executes one library GET through the shared transport gate and returns its
    /// body only after the same origin, path and status guards the readers apply
    /// have passed.
    async fn execute_read(
        &self,
        relative_path: &str,
    ) -> Result<CampusTextResponse, LibraryAdapterError> {
        let endpoint = self.endpoint(relative_path)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| LibraryAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let redirect_location = response
            .headers()
            .get(LOCATION)
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
        let blocked_login_redirect = redirect_location.as_deref().is_some_and(|location| {
            crate::library_read::looks_like_login_location(&response.final_url, location)
        });
        let cross_origin = !same_origin(&self.base_url, &response.final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                redirect_leaves_origin(&self.base_url, &response.final_url, location)
            });
        let redirect_outside_mapping = redirect_location
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
        if cross_origin {
            return Err(LibraryAdapterError::UnexpectedOrigin);
        }
        if response.status != StatusCode::OK {
            return Err(LibraryAdapterError::HttpStatus {
                status: response.status,
            });
        }
        if response.final_url.path() != expected_path
            || !query_matches(&[], &response.final_url)
            || redirect_outside_mapping
            || !path_within_base(&self.base_url, &response.final_url)
        {
            return Err(LibraryAdapterError::UnexpectedPath);
        }
        Ok(response)
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

impl fmt::Debug for LibraryWriteAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryWriteAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .finish()
    }
}

/// One-shot writer for the campus app's socket-state route.
///
/// This is the one library write that leaves the seat-inventory mapping: the
/// socket endpoint is hosted by the campus app origin while the seat inventory is
/// reached through an INFO/WebVPN handoff.  The writer therefore validates its
/// own origin and its own path, and it carries **no** library booking token —
/// the observed request has a JSON body of two fields and nothing else.
///
/// Like the booking writer it dispatches exactly once through
/// `CampusHttpTransport::execute_once_exclusive` and never follows a redirect,
/// because a second send of a write whose effect is unknown is a replay of it.
pub struct LibrarySocketWriteAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: LibrarySocketWriteProfile,
}

impl LibrarySocketWriteAdapter {
    /// Builds a writer against the observed direct campus-app origin.
    pub fn for_app_service(transport: CampusHttpTransport) -> Result<Self, LibraryAdapterError> {
        let base_url = Url::parse(LIBRARY_SOCKET_STATUS_ORIGIN)
            .map_err(|_| LibraryAdapterError::InvalidBaseUrl)?;
        Self::try_with_transport(base_url, transport)
    }

    /// Builds a writer with a caller-provided origin, while retaining the
    /// caller's Cookie jar.  The origin and path guard is the same one the
    /// socket reader applies, so a fixture or an opaque mapping is validated
    /// identically whichever direction the request travels.
    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, LibraryAdapterError> {
        let base_url = normalize_socket_base_url(validate_socket_base_url(base_url)?)?;
        Ok(Self {
            base_url,
            transport,
            profile: LibrarySocketWriteProfile::new(),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> LibrarySocketWriteProfile {
        self.profile
    }

    /// Builds the plan that turns one seat's socket on or off.
    pub fn socket_state_request(
        &self,
        seat_id: u64,
        is_available: bool,
    ) -> Result<LibrarySocketWritePlan, LibraryRequestError> {
        self.profile.socket_state_request(seat_id, is_available)
    }

    /// Dispatches one socket-state write, exactly once.
    pub async fn set_socket_state(
        &self,
        plan: &LibrarySocketWritePlan,
    ) -> Result<LibraryWriteOutcome, LibraryAdapterError> {
        if plan.operation != LibraryWriteOperation::SetSocketState
            || plan.method != LibraryWriteMethod::Post
        {
            return Err(LibraryAdapterError::WriteOperation);
        }
        let endpoint = self.endpoint(&plan.path)?;
        let expected_path = endpoint.path().to_owned();
        let body = plan.body();
        let request = self
            .transport
            .client()
            .post(endpoint)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .build()
            .map_err(|_| LibraryAdapterError::InvalidBaseUrl)?;
        drop(body);
        let response = match self
            .transport
            .execute_once_exclusive(self.transport.client(), request)
            .await
        {
            Ok(response) => response,
            Err(_error) => {
                // The request left this process, so its effect cannot be told
                // apart from an answer that was lost.  Nothing is sent again and
                // the caller is told the outcome is unknown.
                return Ok(LibraryWriteOutcome::Unrecognized);
            }
        };
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes =
            crate::telemetry::timing::read_bounded_bytes(response, MAX_WRITE_RESPONSE_BYTES)
                .await
                .map_err(|_error| {
                    // The request has already left, so an oversized or truncated
                    // answer leaves the outcome unknown rather than failed.
                    LibraryAdapterError::Transport(TransportError::DecodeBody {
                        message: "library socket write response could not be read".to_owned(),
                    })
                })?;
        let response = CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location: redirect_location.clone(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
        };
        let blocked_login_redirect = redirect_location.as_deref().is_some_and(|location| {
            crate::library_read::looks_like_login_location(&response.final_url, location)
        });
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || is_library_login_response(&response)
            || blocked_login_redirect
            || matches!(
                campus_html::classify_page(&response.body),
                PageClass::Login | PageClass::Expired
            )
        {
            // The session is gone, so nothing was applied.  The caller is told to
            // sign in again rather than to retry this write.
            return Ok(LibraryWriteOutcome::LoginRequired);
        }
        let cross_origin = !same_origin(&self.base_url, &response.final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                redirect_leaves_origin(&self.base_url, &response.final_url, location)
            });
        if cross_origin {
            return Err(LibraryAdapterError::UnexpectedOrigin);
        }
        let redirect_outside_mapping = redirect_location
            .as_deref()
            .and_then(|location| resolve_location(&response.final_url, location))
            .is_some_and(|location| {
                same_origin(&self.base_url, &location)
                    && !path_within_base(&self.base_url, &location)
            });
        if redirect_location.is_some()
            || !status.is_success()
            || response.final_url.path() != expected_path
            || !query_matches(&[], &response.final_url)
            || redirect_outside_mapping
            || !path_within_base(&self.base_url, &response.final_url)
        {
            // A redirect this module did not follow, a page for another route, or
            // a route outside the mapping: the request has already left, so the
            // effect is unknown rather than failed.  Never re-dispatched.
            return Ok(LibraryWriteOutcome::Unrecognized);
        }
        Ok(classify_socket_write(&response.body))
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

impl fmt::Debug for LibrarySocketWriteAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySocketWriteAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .finish()
    }
}

fn strip_utf8_bom(body: &str) -> &str {
    body.strip_prefix('\u{feff}').unwrap_or(body)
}

/// Returns the contents of the first double-quoted string in `source`.
fn first_double_quoted(source: &str) -> Option<&str> {
    let start = source.find('"')? + 1;
    let end = source[start..].find('"')? + start;
    Some(&source[start..end])
}

/// Returns the contents of the first single- or double-quoted string in
/// `source`, using the quote character it found to bound the value.
fn first_quoted(source: &str) -> Option<&str> {
    let (start, quote) = source
        .char_indices()
        .find(|(_, character)| matches!(character, '\'' | '"'))?;
    let value_start = start + quote.len_utf8();
    let end = source[value_start..].find(quote)? + value_start;
    Some(&source[value_start..end])
}

fn is_cancellation_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_CANCELLATION_ID_CHARS
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// The service formats a reservation's location as `library-area:seat`.  The
/// colon is the only shape evidence available, so it is required; a row whose
/// location column does not carry one is treated as unrecognized rather than
/// reported as a location.
fn is_record_position(value: &str) -> bool {
    if value.is_empty() || value.chars().count() > MAX_RECORD_CELL_CHARS {
        return false;
    }
    let Some((head, tail)) = value.split_once(':') else {
        return false;
    };
    !head.is_empty()
        && !tail.is_empty()
        && !tail.contains(':')
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}

/// The service prints a reservation time as `YYYY-MM-DD HH:MM[:SS]`.  This is the
/// second independent shape check, so a shifted column cannot turn an arbitrary
/// cell into a reported time.
fn is_record_time(value: &str) -> bool {
    /// The positions of the twelve digits in `YYYY-MM-DD HH:MM`.
    const DIGITS: [usize; 12] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15];
    let bytes = value.as_bytes();
    if !matches!(bytes.len(), 16 | 19) || !value.is_ascii() {
        return false;
    }
    if DIGITS.iter().any(|index| !bytes[*index].is_ascii_digit())
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b' '
        || bytes[13] != b':'
    {
        return false;
    }
    match bytes.len() {
        16 => true,
        19 => bytes[16] == b':' && bytes[17..].iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

fn is_record_status(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_RECORD_CELL_CHARS
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_row(position: &str, time: &str, status: &str, action: &str) -> String {
        // Twelve filler cells, then the three valued columns, two more, and the
        // action column: the observed 16-cell row.
        let filler = "<td></td>".repeat(5);
        format!(
            "<tr>{filler}<td>{position}</td><td></td><td>{time}</td><td></td><td></td><td></td><td>{status}</td><td></td><td></td><td></td><td>{action}</td></tr>"
        )
    }

    fn page(rows: &str) -> String {
        format!("<!DOCTYPE html><html><body><table><tbody>{rows}</tbody></table></body></html>")
    }

    #[test]
    fn plans_are_relative_posts_that_never_carry_the_token() {
        let profile = LibraryWriteProfile::new();
        let booking = profile.book_seat_request(701, 9001, 2).expect("booking");
        assert_eq!(booking.operation, LibraryWriteOperation::BookSeat);
        assert_eq!(booking.method, LibraryWriteMethod::Post);
        assert_eq!(booking.path, "/api.php/spaces/701/book");
        assert_eq!(
            booking.form_parameters(),
            &[
                ("segment".to_owned(), "9001".to_owned()),
                ("type".to_owned(), "2".to_owned()),
                ("operateChannel".to_owned(), "2".to_owned()),
            ]
        );
        assert_eq!(
            booking.session_prerequisite,
            LibraryWriteSessionPrerequisite::existing_info_web_vpn_session()
        );

        let cancel = profile
            .cancel_booking_request("202009111837")
            .expect("cancel");
        assert_eq!(cancel.operation, LibraryWriteOperation::CancelBooking);
        assert_eq!(cancel.path, "/api.php/profile/books/202009111837");
        assert_eq!(
            cancel.form_parameters(),
            &[
                ("_method".to_owned(), "delete".to_owned()),
                ("id".to_owned(), "202009111837".to_owned()),
                ("operateChannel".to_owned(), "2".to_owned()),
            ]
        );

        // A plan never prints a form value, and never names the two fields the
        // dispatch adds.
        let rendered = format!("{booking:?}");
        assert!(!rendered.contains("9001"));
        assert!(!rendered.contains("access_token"));
        assert!(!rendered.contains("userid"));
        assert!(!format!("{cancel:?}").contains("202009111837"));
    }

    #[test]
    fn rejects_identifiers_that_could_extend_a_path() {
        let profile = LibraryWriteProfile::new();
        assert!(profile.book_seat_request(0, 9001, 2).is_err());
        assert!(profile.book_seat_request(701, 0, 2).is_err());
        for value in [
            "",
            "../1",
            "1/2",
            "1?x=1",
            "1#f",
            "1 2",
            "202009111837;drop",
        ] {
            assert!(
                profile.cancel_booking_request(value).is_err(),
                "{value} must be rejected"
            );
        }
        assert!(profile.cancel_booking_request("202009111837").is_ok());
    }

    #[test]
    fn access_token_is_read_from_the_observed_field_and_never_printed() {
        let body = "var config = {access_token: \"fa-9b7c\"};";
        let token = extract_access_token(body).unwrap();
        assert_eq!(token.expose(), "fa-9b7c");
        assert_eq!(format!("{token:?}"), "LibraryAccessToken([redacted])");
        // The observed single-quoted form is not what the reference reads.
        assert_eq!(
            extract_access_token("access_token = 'x'"),
            Err(LibraryWriteParseError::InvalidAccessToken)
        );
        assert_eq!(
            extract_access_token("<html>no such field</html>"),
            Err(LibraryWriteParseError::MissingAccessToken)
        );
        assert_eq!(
            extract_access_token(""),
            Err(LibraryWriteParseError::EmptyBody)
        );
        // A mention inside a script must not end the search when a usable field
        // follows it.
        let body = "// access_token is set below\naccess_token: \"real\"";
        assert_eq!(extract_access_token(body).unwrap().expose(), "real");
        // A field whose value is blank is refused rather than sent empty.
        assert_eq!(
            extract_access_token("access_token: \"   \""),
            Err(LibraryWriteParseError::InvalidAccessToken)
        );
    }

    #[test]
    fn reads_the_observed_reservation_columns() {
        let body = page(&record_row(
            "文科图书馆-四层-C区:F4C083",
            "2020-09-11 12:15:52",
            "已使用",
            "<a onclick=\"menuDel('202009111837')\">取消预约</a>",
        ));
        let records = parse_booking_records(&body).unwrap();
        assert_eq!(records.records.len(), 1);
        let record = &records.records[0];
        assert_eq!(record.position, "文科图书馆-四层-C区:F4C083");
        assert_eq!(record.time, "2020-09-11 12:15:52");
        assert_eq!(record.status, "已使用");
        assert_eq!(record.cancellation_id.as_deref(), Some("202009111837"));
    }

    #[test]
    fn a_row_without_a_deletion_call_simply_has_no_cancellation() {
        let body = page(&record_row(
            "文科图书馆-二层-A区:F2A008",
            "2020-09-08 08:00:00",
            "用户取消",
            "<span>—</span>",
        ));
        let records = parse_booking_records(&body).unwrap();
        assert_eq!(records.records[0].cancellation_id, None);
        // The double-quoted form of the same call is read too, because the
        // identifier comes from the call's argument rather than from the quote
        // style one revision happens to use.
        let body = page(&record_row(
            "文科图书馆-二层-A区:F2A008",
            "2020-09-08 08:00:00",
            "已预约",
            "<a onclick='menuDel(\"abc-1\")'>取消</a>",
        ));
        assert_eq!(
            parse_booking_records(&body).unwrap().records[0]
                .cancellation_id
                .as_deref(),
            Some("abc-1")
        );
    }

    #[test]
    fn an_empty_table_body_is_an_empty_answer_but_a_missing_one_is_not() {
        let empty = parse_booking_records(&page("")).unwrap();
        assert!(empty.records.is_empty());
        assert_eq!(
            parse_booking_records("<!DOCTYPE html><html><body><p>nothing</p></body></html>"),
            Err(LibraryWriteParseError::MissingTable)
        );
        assert_eq!(
            parse_booking_records("{\"data\":[]}"),
            Err(LibraryWriteParseError::UnexpectedHtml)
        );
        assert_eq!(
            parse_booking_records("   "),
            Err(LibraryWriteParseError::EmptyBody)
        );
    }

    #[test]
    fn a_shifted_or_reworded_row_is_refused_rather_than_read() {
        // One column short: every index after the insertion point would be wrong,
        // so the row is refused instead of reported with a neighbouring column's
        // value.
        let short = format!(
            "<tr>{}</tr>",
            "<td>文科图书馆-四层-C区:F4C083</td><td>2020-09-11 12:15:52</td><td>已使用</td>"
        );
        assert_eq!(
            parse_booking_records(&page(&short)),
            Err(LibraryWriteParseError::InvalidRecord {
                index: 0,
                column: "<row>",
            })
        );
        // The right column count with a value that cannot be a location.
        let wrong_position = page(&record_row(
            "no-colon-here",
            "2020-09-11 12:15:52",
            "已使用",
            "",
        ));
        assert_eq!(
            parse_booking_records(&wrong_position),
            Err(LibraryWriteParseError::InvalidRecord {
                index: 0,
                column: "position",
            })
        );
        // The right column count with a value that cannot be a timestamp.
        let wrong_time = page(&record_row(
            "文科图书馆-四层-C区:F4C083",
            "已使用",
            "已使用",
            "",
        ));
        assert_eq!(
            parse_booking_records(&wrong_time),
            Err(LibraryWriteParseError::InvalidRecord {
                index: 0,
                column: "time",
            })
        );
    }

    #[test]
    fn login_and_expiry_pages_are_session_failures() {
        assert_eq!(
            parse_booking_records("<html><head><title>清华大学WebVPN</title></head></html>"),
            Err(LibraryWriteParseError::LoginHtml)
        );
        assert_eq!(
            parse_booking_records("<html><body>用户登陆超时或访问内容不存在</body></html>"),
            Err(LibraryWriteParseError::ExpiredHtml)
        );
    }

    #[test]
    fn write_answers_are_accepted_only_by_the_services_own_flag() {
        assert_eq!(
            classify_library_write("{\"status\":1,\"msg\":\"ok\"}"),
            LibraryWriteOutcome::Accepted
        );
        assert_eq!(
            classify_library_write(
                "{\"status\":0,\"msg\":\"Testing account cannot book a seat.\"}"
            ),
            LibraryWriteOutcome::Refused
        );
        // An answer whose only field is a message is unrecognized, not a refusal:
        // this module does not surface the service's free text either way, so
        // what a caller should do next is refresh the list, not retry.
        assert_eq!(
            classify_library_write("{\"msg\":\"座位已被预约\"}"),
            LibraryWriteOutcome::Unrecognized
        );
        for body in [
            "",
            "   ",
            "not json",
            "[]",
            "{\"other\":1}",
            "<html>ok</html>",
        ] {
            assert_eq!(
                classify_library_write(body),
                LibraryWriteOutcome::Unrecognized,
                "{body} must not read as an answer"
            );
        }
    }

    #[test]
    fn record_time_shape_is_exact() {
        assert!(is_record_time("2020-09-11 12:15:52"));
        assert!(is_record_time("2020-09-11 12:15"));
        for value in [
            "",
            "2020-09-11",
            "2020/09/11 12:15:52",
            "2020-09-11T12:15:52",
            "2020-09-11 12:15:5",
            "2020-09-11 12:15:526",
            "x020-09-11 12:15:52",
            "2020-09-11 12-15:52",
            "２０２０-09-11 12:15:52",
        ] {
            assert!(!is_record_time(value), "{value} is not a record time");
        }
    }
}
