//! Course-reserve textbook catalogue (`教参平台`): search and one book's detail.
//!
//! The reserves application is a legacy ASP site on its own campus host,
//! reached through the WebVPN mapping bound to [`RESERVES_MAPPING_TOKEN`].  The
//! reference client talks to it with **absolute** WebVPN URLs, so this is a
//! plain read inside a mapping the already-proven INFO/WebVPN session holds;
//! there is no credential step of its own on this path.
//!
//! That detail decides this module's shape.  The reference's roaming recovery
//! policy for this application is `"id"`, which performs a **campus identity
//! login** (it fetches `…/auth/login/form/<app>` and posts credentials to the
//! identity login endpoint) before retrying.  This engine does not implement a
//! second campus login, and it does not need one: the identity cookie jar and
//! the WebVPN hop are already shared through `CampusHttpTransport`.
//! [`RESERVES_WEBVPN_TARGET`] is therefore recorded for documentation only and
//! is deliberately **not** registered as a roaming selector in
//! `info_session::map_additional_roaming`; no INFO roam is dispatched for this
//! read at all.  The adapter is built directly from this module's own mapping
//! root ([`RESERVES_MAPPING_TOKEN`]), which is the same absolute mapping the
//! reference's `RESERVES_LIB_SEARCH` URL carries.  A read whose session has
//! expired is reported as [`ReservesAdapterError::SessionExpired`], so the
//! existing INFO refresh path handles it exactly like the other INFO-hosted
//! readers.
//!
//! Two reads are exposed:
//!
//! * [`ReservesAdapter::search`] — one page of matches for a caller-supplied
//!   book name.  The name is encoded with the service's own private `%uXXXX`
//!   scheme before it reaches the query string.
//! * [`ReservesAdapter::detail`] — one book, addressed by an opaque
//!   [`ReservesRef`] from the latest search.  The service's `bookId` never
//!   leaves this module.
//!
//! **No mock, and no empty result for a page that did not parse.**  The public
//! reference answers a response with no `.p-fbox` block from a built-in mock
//! object, which cannot distinguish "the service found nothing" from "the page
//! changed".  Here the service's own result counter is the evidence: an empty
//! catalogue is reported only when the page really carried `共 0 条结果,0 页`,
//! and a page missing the counter or a required per-book field is a parse
//! failure instead.
//!
//! Every cover image and chapter href the page prints is rewritten onto this
//! module's own mapping origin, so a response cannot move a caller's image or
//! chapter fetch to another host.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{
    collections::BTreeSet,
    fmt,
    sync::{
        Mutex, MutexGuard,
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

/// The reserves application's WebVPN mapping token.  It is the only mapping
/// this module will address, so a rewritten URL cannot reach another
/// application.
pub(crate) const RESERVES_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421e2f2529935266d43300480aed641303c455d43259619a3eaf6eebb99";

/// The scheme segment of the mapping.  The reference's own constants spell this
/// route as `/http/<token>/…`, so the mapping is an `http` one.
pub(crate) const RESERVES_MAPPING_SCHEME: &str = "http";

/// The reference's roaming recovery payload for this application.
///
/// It is an **identity application id** (`ID_BASE_URL + payload`), not a WebVPN
/// roam selector: using it would mean performing a campus identity login inside
/// a read.  This engine does not do that, so the constant is retained only so a
/// future reader can see which recovery path was deliberately not implemented.
/// It is never dispatched, and it carries no request of its own.
pub const RESERVES_WEBVPN_TARGET: &str = "5bf6e5a699d63ff1cdb082836ebd50f9";

/// The catalogue search endpoint behind the mapping above.
pub const RESERVES_SEARCH_PATH: &str = "/Search/ResBooks";

/// One book's detail page.  The [`ReservesRef`] supplies the `bookId` query
/// value; the path itself is a module constant.
pub const RESERVES_DETAIL_PATH: &str = "/Search/BookDetail";

/// The page's own marker for "this account is not signed in to INFO".  It
/// arrives inside a 200 page, so it is classified before any field is parsed.
pub const RESERVES_LOGIN_MARKER: &str = "请您登录个人INFO账户查看教参全文";

/// The origin every printed image and chapter href is rewritten onto.
const RESERVES_MAPPING_ORIGIN: &str = "https://webvpn.tsinghua.edu.cn";

/// The largest page number a caller may ask for.  The bound exists so a caller
/// cannot use this adapter to walk the catalogue indefinitely.
pub const MAX_RESERVES_PAGE: u32 = 1000;

/// The service's own result-counter line, as the page prints it.
const RESULT_COUNT_ANCHOR: &str = "共";
const RESULT_COUNT_MARKER: &str = "条结果";
const PAGE_COUNT_MARKER: &str = "页";

/// The class on one search-result block, and on the container that prints the
/// result counter.
const RESULT_BLOCK_CLASS: &str = "p-fbox";
const RESULT_SUMMARY_CLASS: &str = "s-list";
/// The class on the detail page's result region.
const DETAIL_RESULT_CLASS: &str = "p-result";

/// The query key the service prints in a result block's link.
const BOOK_ID_QUERY_KEY: &str = "bookId=";

/// The labels the detail page prints beside each field, in the order the page
/// lays them out.
///
/// They are corroboration, not the primary contract: the value of row *n* is
/// always read the way the reference reads it (the row's second `font`).  The
/// labels are consulted only when **all six** are present and distinct, in
/// which case they re-assign a row the service has moved; otherwise the
/// reference's own row order is used.  A partial match therefore falls back
/// rather than shifting one field onto another row.
const DETAIL_FIELD_LABELS: [&str; 6] = ["书名", "著者", "出版社", "ISBN", "版次", "卷册"];

const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESULTS: usize = 4096;
const MAX_CHAPTERS: usize = 4096;
const MAX_TEXT_CHARS: usize = 512;
const MAX_BOOK_NAME_CHARS: usize = 64;
const MAX_PATH_CHARS: usize = 512;
/// One `bookId` as the service prints it.  It is opaque, so only its length and
/// character class are constrained.
const MAX_BOOK_ID_CHARS: usize = 128;

static NEXT_RESERVES_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This profile only issues GETs, so no write route can be smuggled into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservesMethod {
    Get,
}

/// The observed course-reserve operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservesOperation {
    Search,
    ReadDetail,
}

/// A reserves read requires an already established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservesSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path and query key only, never an absolute
/// WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesRequestPlan {
    pub operation: ReservesOperation,
    pub method: ReservesMethod,
    pub path: &'static str,
    pub webvpn_target: &'static str,
    pub session_prerequisite: ReservesSessionPrerequisite,
}

/// Fixed course-reserve route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReservesProfile;

impl ReservesProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub fn search_request(self) -> ReservesRequestPlan {
        ReservesRequestPlan {
            operation: ReservesOperation::Search,
            method: ReservesMethod::Get,
            path: RESERVES_SEARCH_PATH,
            webvpn_target: RESERVES_WEBVPN_TARGET,
            session_prerequisite: ReservesSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    pub fn detail_request(self) -> ReservesRequestPlan {
        ReservesRequestPlan {
            operation: ReservesOperation::ReadDetail,
            method: ReservesMethod::Get,
            path: RESERVES_DETAIL_PATH,
            webvpn_target: RESERVES_WEBVPN_TARGET,
            session_prerequisite: ReservesSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// One parsed search-result block, before the adapter attaches its reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesSearchRow {
    /// The advertised title.
    pub title: String,
    /// The cover image URL, rewritten onto this module's mapping origin.
    pub image_url: String,
    /// The printed ISBN.
    pub isbn: String,
    /// The printed author line.
    pub author: String,
    /// The printed publication line.
    pub publisher: String,
    /// The service's own book identifier.  It stays inside the module and is
    /// only used to address this row's detail page.
    book_id: String,
}

impl ReservesSearchRow {
    /// The record's opaque identifier.  It never crosses the boundary by value;
    /// [`ReservesRef`] is the handle that does.
    pub(crate) fn book_id(&self) -> &str {
        &self.book_id
    }
}

/// An opaque reference to one catalogue record.
///
/// The `bookId` stays inside the adapter that produced it.  A reference from a
/// different adapter instance, from a superseded search, or beyond the range of
/// the page it came from does not resolve at all.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ReservesRef {
    pub(crate) adapter_binding: u64,
    pub(crate) generation: u64,
    index: u32,
}

impl ReservesRef {
    pub(crate) fn new(adapter_binding: u64, generation: u64, index: u32) -> Self {
        Self {
            adapter_binding,
            generation,
            index,
        }
    }

    /// The position of this record inside the page that produced it.
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl fmt::Debug for ReservesRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReservesRef")
            .field("index", &self.index)
            .finish()
    }
}

/// One catalogue record as exposed to a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesBook {
    pub title: String,
    pub image_url: String,
    pub isbn: String,
    pub author: String,
    pub publisher: String,
    /// Opaque handle for this record's detail page.
    pub reference: ReservesRef,
}

/// One validated page of catalogue records.
///
/// An empty `books` list with a zero `total` is a valid answer: the page's own
/// counter said the search matched nothing.  It is only produced when the
/// counter was present, so "no matches" can never be manufactured from a
/// response that failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesSearch {
    pub books: Vec<ReservesBook>,
    /// The service's own match count for this query.
    pub total: u64,
    /// The service's own page count for this query.
    pub page_count: u64,
    /// The page this result came from.
    pub page: u32,
}

impl ReservesSearch {
    pub fn len(&self) -> usize {
        self.books.len()
    }

    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }
}

/// One chapter link on a book's detail page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesChapter {
    pub title: String,
    /// The chapter href, rewritten onto this module's mapping origin.
    pub url: String,
}

/// One book's detail page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesBookDetail {
    pub title: String,
    pub image_url: String,
    pub author: String,
    pub publisher: String,
    pub isbn: String,
    pub version: String,
    pub volume: String,
    pub chapters: Vec<ReservesChapter>,
}

/// The records parsed from one search page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesSearchRows {
    pub rows: Vec<ReservesSearchRow>,
    pub total: u64,
    pub page_count: u64,
}

/// Parser failures retain only stable names.  They never keep response bytes,
/// Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReservesParseError {
    #[error("course-reserve response body is empty")]
    EmptyBody,

    #[error("course-reserve response is an HTML login page")]
    LoginPage,

    #[error("course-reserve response is an expired or timed-out page")]
    ExpiredPage,

    #[error("the course-reserve search result container is missing")]
    MissingResults,

    #[error("the course-reserve result counter is missing")]
    MissingCount,

    #[error("the course-reserve search page exceeded the bounded record limit")]
    TooLarge,

    #[error("the course-reserve page could not be scanned: {context}")]
    Malformed { context: &'static str },

    #[error("course-reserve record {row} is missing its title")]
    MissingTitle { row: usize },

    #[error("course-reserve record {row} is missing its book identifier")]
    MissingBookId { row: usize },

    #[error("course-reserve record {row} carried a link outside the mapped origin")]
    ForeignLink { row: usize },

    #[error("course-reserve record {row} is missing a printed field")]
    MissingField { row: usize },

    #[error("the course-reserve detail page is missing a required field")]
    MissingDetailField,

    #[error("the course-reserve detail page exceeded the bounded chapter limit")]
    TooManyChapters,
}

impl ReservesParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired INFO session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

/// Adapter failures are body-free so a login page cannot leak through a debug
/// or bridge DTO.
#[derive(Debug, Error)]
pub enum ReservesAdapterError {
    #[error("course-reserve base URL is invalid")]
    InvalidBaseUrl,

    #[error("course-reserve transport failed")]
    Transport(#[source] TransportError),

    #[error("course-reserve request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("course-reserve response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("course-reserve response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("course-reserve response is not the expected content type")]
    UnexpectedContentType,

    #[error("course-reserve response is larger than this domain will read")]
    UnexpectedDeployment,

    #[error("course-reserve INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("course-reserve response could not be parsed: {0}")]
    Parse(#[source] ReservesParseError),

    #[error("the requested course-reserve page is outside the allowed bound")]
    PageOutOfRange,

    #[error("the course-reserve search text is not usable")]
    InvalidSearchText,

    #[error("the referenced catalogue record is no longer available")]
    UnknownReference,
}

impl ReservesAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "reserves_config",
            Self::Transport(_) => "reserves_network",
            Self::HttpStatus { .. } => "reserves_http",
            Self::UnexpectedOrigin => "reserves_origin",
            Self::UnexpectedPath => "reserves_path",
            Self::UnexpectedContentType => "reserves_content_type",
            Self::UnexpectedDeployment => "reserves_deployment",
            Self::SessionExpired => "reserves_auth_required",
            Self::Parse(ReservesParseError::EmptyBody) => "reserves_body_empty",
            Self::Parse(ReservesParseError::LoginPage)
            | Self::Parse(ReservesParseError::ExpiredPage) => "reserves_auth_required",
            Self::Parse(ReservesParseError::MissingResults) => "reserves_results_missing",
            Self::Parse(ReservesParseError::MissingCount) => "reserves_count_missing",
            Self::Parse(ReservesParseError::TooLarge)
            | Self::Parse(ReservesParseError::TooManyChapters) => "reserves_size",
            Self::Parse(ReservesParseError::Malformed { .. }) => "reserves_malformed",
            Self::Parse(ReservesParseError::MissingTitle { .. }) => "reserves_row_title",
            Self::Parse(ReservesParseError::MissingBookId { .. }) => "reserves_row_key",
            Self::Parse(ReservesParseError::ForeignLink { .. }) => "reserves_row_link",
            Self::Parse(ReservesParseError::MissingField { .. })
            | Self::Parse(ReservesParseError::MissingDetailField) => "reserves_row_field",
            Self::PageOutOfRange => "reserves_config",
            Self::InvalidSearchText => "reserves_input",
            Self::UnknownReference => "reserves_reference",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::SessionExpired)
            || matches!(self, Self::Parse(error) if error.is_session_expired())
    }
}

/// Configuration for a read-only course-reserve adapter.
#[derive(Clone)]
pub struct ReservesAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl ReservesAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, ReservesAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/reserves", Duration::from_secs(30))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, ReservesAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, ReservesAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| ReservesAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(ReservesAdapterError::InvalidBaseUrl);
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

    fn transport(&self) -> Result<CampusHttpTransport, ReservesAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(ReservesAdapterError::Transport)
    }
}

impl fmt::Debug for ReservesAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReservesAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one course-reserve response.  It is opaque:
/// no Cookie, URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct ReservesBusinessProof {
    adapter_binding: u64,
    generation: u64,
    pub(crate) operation: ReservesOperation,
}

impl fmt::Debug for ReservesBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReservesBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated search together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesSearchRead {
    pub value: ReservesSearch,
    pub proof: ReservesBusinessProof,
}

/// A validated detail page together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservesDetailRead {
    pub value: ReservesBookDetail,
    pub proof: ReservesBusinessProof,
}

/// The book identifiers named by the most recent accepted search.
#[derive(Default)]
struct ReservesCatalogue {
    generation: u64,
    book_ids: Vec<String>,
}

/// Read-only course-reserve client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct ReservesAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: ReservesProfile,
    binding: u64,
    catalogue: Mutex<ReservesCatalogue>,
}

impl ReservesAdapter {
    pub fn new(config: ReservesAdapterConfig) -> Result<Self, ReservesAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, ReservesAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: ReservesProfile::standard(),
            binding: NEXT_RESERVES_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            catalogue: Mutex::new(ReservesCatalogue::default()),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> ReservesProfile {
        self.profile
    }

    /// Searches the catalogue for one book name.
    ///
    /// `book_name` is the caller's own text.  It is validated and then encoded
    /// with the service's private `%uXXXX` scheme, so a value this module will
    /// not send is reported as invalid input rather than becoming a service-side
    /// query.
    pub async fn search(
        &self,
        book_name: &str,
        page: u32,
    ) -> Result<ReservesSearch, ReservesAdapterError> {
        self.search_with_proof(book_name, page)
            .await
            .map(|read| read.value)
    }

    pub async fn search_with_proof(
        &self,
        book_name: &str,
        page: u32,
    ) -> Result<ReservesSearchRead, ReservesAdapterError> {
        if page == 0 || page > MAX_RESERVES_PAGE {
            return Err(ReservesAdapterError::PageOutOfRange);
        }
        let encoded = encode_book_name(book_name)?;
        // The reference's own first-page request carries no `page` parameter at
        // all, so the first page is spelled exactly the way it is spelled there
        // and only a later page adds the parameter.
        let mut query = format!("bookName={encoded}");
        if page > 1 {
            query.push_str("&page=");
            query.push_str(&page.to_string());
        }
        let plan = self.profile.search_request();
        let response = self.execute(&plan, &query).await?;
        let body = self.accept_html(response).await?;
        let parsed = parse_reserves_search_html(&body).map_err(ReservesAdapter::map_parse_error)?;
        let book_ids: Vec<String> = parsed.rows.iter().map(|row| row.book_id.clone()).collect();
        // The catalogue advances only after the page parsed, so a rejected
        // search leaves the previous references resolvable instead of pointing
        // them at nothing.
        let generation = {
            let mut catalogue = self.catalogue();
            catalogue.generation = catalogue.generation.wrapping_add(1);
            catalogue.book_ids = book_ids;
            catalogue.generation
        };
        let books = parsed
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| ReservesBook {
                title: row.title.clone(),
                image_url: row.image_url.clone(),
                isbn: row.isbn.clone(),
                author: row.author.clone(),
                publisher: row.publisher.clone(),
                reference: ReservesRef::new(self.binding, generation, index as u32),
            })
            .collect();
        Ok(ReservesSearchRead {
            value: ReservesSearch {
                books,
                total: parsed.total,
                page_count: parsed.page_count,
                page,
            },
            proof: ReservesBusinessProof {
                adapter_binding: self.binding,
                generation,
                operation: ReservesOperation::Search,
            },
        })
    }

    /// Reads one book's detail page, addressed by a reference this adapter
    /// produced.
    pub async fn detail(
        &self,
        reference: &ReservesRef,
    ) -> Result<ReservesBookDetail, ReservesAdapterError> {
        self.detail_with_proof(reference)
            .await
            .map(|read| read.value)
    }

    pub async fn detail_with_proof(
        &self,
        reference: &ReservesRef,
    ) -> Result<ReservesDetailRead, ReservesAdapterError> {
        let book_id = self
            .book_id(reference)
            .ok_or(ReservesAdapterError::UnknownReference)?;
        let plan = self.profile.detail_request();
        let query = format!("bookId={}", percent_encode_query_value(&book_id));
        let response = self.execute(&plan, &query).await?;
        let body = self.accept_html(response).await?;
        let value = parse_reserves_detail_html(&body).map_err(ReservesAdapter::map_parse_error)?;
        Ok(ReservesDetailRead {
            value,
            proof: ReservesBusinessProof {
                adapter_binding: self.binding,
                generation: self.catalogue().generation,
                operation: ReservesOperation::ReadDetail,
            },
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &ReservesBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    /// Resolves a record reference to its book identifier, if the reference
    /// still belongs to the search this adapter most recently accepted.
    pub(crate) fn book_id(&self, reference: &ReservesRef) -> Option<String> {
        if reference.adapter_binding != self.binding {
            return None;
        }
        let catalogue = self.catalogue();
        if catalogue.generation != reference.generation {
            return None;
        }
        catalogue
            .book_ids
            .get(usize::try_from(reference.index).ok()?)
            .cloned()
    }

    /// The current catalogue generation.  It advances once per accepted search.
    pub fn catalogue_generation(&self) -> u64 {
        self.catalogue().generation
    }

    fn catalogue(&self) -> MutexGuard<'_, ReservesCatalogue> {
        // The guarded section is a couple of field assignments, so a poisoned
        // lock cannot represent a half-applied update: recover the guard rather
        // than turning every later read into an error.
        self.catalogue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Issues one request and applies every guard that must precede a parse.
    ///
    /// The query is placed verbatim rather than re-encoded, because this
    /// service's own `%uXXXX` escapes are not percent-encodings and a
    /// re-encoding step would change the bytes the service is asked to decode.
    /// [`valid_query`] is what constrains the value instead: only the module's
    /// own two escape spellings may reach the wire.
    async fn execute(
        &self,
        plan: &ReservesRequestPlan,
        query: &str,
    ) -> Result<ReservesResponse, ReservesAdapterError> {
        let endpoint = self.endpoint(plan, query)?;
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| ReservesAdapterError::Transport(TransportError::Request(error)))?;
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
            .map_err(|_| ReservesAdapterError::UnexpectedOrigin)?;
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
            return Err(ReservesAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(ReservesAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(ReservesAdapterError::UnexpectedPath);
        }
        if final_url.path() != expected_path
            || final_url.query().map(str::to_owned) != expected_query
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
            || status.is_redirection()
        {
            return Err(ReservesAdapterError::UnexpectedPath);
        }
        if status != StatusCode::OK {
            return Err(ReservesAdapterError::HttpStatus { status });
        }
        Ok(ReservesResponse {
            response,
            content_type,
        })
    }

    /// Reads a text body and classifies the service's own states before any
    /// content-type check can turn them into a format failure.
    async fn accept_html(
        &self,
        response: ReservesResponse,
    ) -> Result<String, ReservesAdapterError> {
        let body = crate::telemetry::timing::read_text(response.response)
            .await
            .map_err(|error| ReservesAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_HTML_BYTES {
            return Err(ReservesAdapterError::UnexpectedDeployment);
        }
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(ReservesAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        // The application's own "not signed in to INFO" notice is a 200 HTML
        // page, so it is a session failure rather than a missing field.
        if body.contains(RESERVES_LOGIN_MARKER) {
            return Err(ReservesAdapterError::SessionExpired);
        }
        if !is_html_content_type(response.content_type.as_deref()) {
            return Err(ReservesAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    fn endpoint(
        &self,
        plan: &ReservesRequestPlan,
        query: &str,
    ) -> Result<Url, ReservesAdapterError> {
        if !valid_relative_path(plan.path) || !valid_query(query) {
            return Err(ReservesAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/').to_owned();
        endpoint.set_path(&format!("{base_path}{}", plan.path));
        endpoint.set_query(Some(query));
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: ReservesParseError) -> ReservesAdapterError {
        if error.is_session_expired() {
            ReservesAdapterError::SessionExpired
        } else {
            ReservesAdapterError::Parse(error)
        }
    }
}

impl fmt::Debug for ReservesAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReservesAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("catalogue_generation", &self.catalogue_generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// A response whose transport-level guards have already passed.
struct ReservesResponse {
    response: reqwest::Response,
    content_type: Option<String>,
}

/// Encodes a book name with the service's own private escape scheme.
///
/// The reference maps every UTF-16 code unit at or above 128 to `%uXXXX` with
/// upper-case hex digits and leaves ASCII characters literal.  Reproducing the
/// *code-unit* mapping rather than a byte encoding is deliberate: a character
/// outside the BMP arrives as its two surrogate escapes, which is what this
/// service's decoder was built for.
///
/// A name that is empty, too long, or carries a control character is refused
/// before any request.  Characters that would change the query's own structure
/// are refused too rather than silently rewritten, because the service decodes
/// only its own escape form and a mixed encoding would be a guess.
pub fn encode_book_name(book_name: &str) -> Result<String, ReservesAdapterError> {
    let trimmed = book_name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_BOOK_NAME_CHARS
        || trimmed.chars().any(char::is_control)
        || trimmed.contains(['&', '=', '#', '%', '+', '?'])
    {
        return Err(ReservesAdapterError::InvalidSearchText);
    }
    let mut encoded = String::with_capacity(trimmed.len());
    for unit in trimmed.encode_utf16() {
        if unit < 128 {
            encoded.push(char::from(unit as u8));
        } else {
            encoded.push_str(&format!("%u{unit:04X}"));
        }
    }
    Ok(encoded)
}

/// Parses one catalogue search page.
///
/// The service's own result counter is the evidence for an empty catalogue: the
/// page must be a business page and must carry the `共 N 条结果,M 页` line.  Only
/// then may a page with no result block mean "no matches"; anything else is an
/// error rather than an empty result.
pub fn parse_reserves_search_html(body: &str) -> Result<ReservesSearchRows, ReservesParseError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(ReservesParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(ReservesParseError::LoginPage),
        PageClass::Expired => return Err(ReservesParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    if trimmed.contains(RESERVES_LOGIN_MARKER) {
        return Err(ReservesParseError::LoginPage);
    }
    let (total, page_count) = parse_result_counter(trimmed)?;
    let blocks = scan_by_class(trimmed, RESULT_BLOCK_CLASS).map_err(map_scan_error)?;
    if blocks.is_empty() {
        if total == 0 {
            return Ok(ReservesSearchRows {
                rows: Vec::new(),
                total,
                page_count,
            });
        }
        return Err(ReservesParseError::MissingResults);
    }
    if blocks.len() > MAX_RESULTS {
        return Err(ReservesParseError::TooLarge);
    }
    let mut rows = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        rows.push(parse_search_block(block, index)?);
    }
    Ok(ReservesSearchRows {
        rows,
        total,
        page_count,
    })
}

/// Reads the service's `共 N 条结果,M 页` counter.
///
/// The counter is located by the container class the reference reads it from, so
/// a page that moved the counter outside that container is a
/// [`ReservesParseError::MissingCount`] rather than a silently empty result.
fn parse_result_counter(body: &str) -> Result<(u64, u64), ReservesParseError> {
    let containers = scan_by_class(body, RESULT_SUMMARY_CLASS).map_err(map_scan_error)?;
    if containers.is_empty() {
        return Err(ReservesParseError::MissingCount);
    }
    for container in &containers {
        if let Some(counts) = read_counter_line(&container.text()) {
            return Ok(counts);
        }
    }
    Err(ReservesParseError::MissingCount)
}

/// Reads `共 N 条结果,M 页` out of one text fragment.
fn read_counter_line(text: &str) -> Option<(u64, u64)> {
    let start = text.find(RESULT_COUNT_ANCHOR)? + RESULT_COUNT_ANCHOR.len();
    let (total, rest) = read_leading_number(text[start..].trim_start())?;
    let rest = rest
        .trim_start()
        .strip_prefix(RESULT_COUNT_MARKER)?
        .trim_start();
    let rest = rest.strip_prefix(',')?.trim_start();
    let (page_count, rest) = read_leading_number(rest)?;
    rest.trim_start().strip_prefix(PAGE_COUNT_MARKER)?;
    Some((total, page_count))
}

/// Reads one leading integer and returns it with the remainder.
fn read_leading_number(text: &str) -> Option<(u64, &str)> {
    let end = text
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// Reads one search-result block.
///
/// Each field is located the way the reference locates it: the book identifier
/// from the block's own `bookId=` link, the title from its `strong`, the cover
/// from its `img`, and the three printed lines from the block's own paragraphs.
/// A block missing any of them is an error, never a record with empty fields.
fn parse_search_block(
    block: &RawElement,
    index: usize,
) -> Result<ReservesSearchRow, ReservesParseError> {
    let inner = block.inner();
    let anchors = campus_html::scan(inner, "a").map_err(map_scan_error)?;
    let book_id = anchors
        .iter()
        .filter_map(|anchor| anchor.attr("href"))
        .find_map(read_book_id)
        .ok_or(ReservesParseError::MissingBookId { row: index })?;

    let images = campus_html::scan(inner, "img").map_err(map_scan_error)?;
    let Some(source) = images.iter().find_map(|image| image.attr("src")) else {
        return Err(ReservesParseError::MissingField { row: index });
    };
    let image_url =
        mapped_asset_url(source).ok_or(ReservesParseError::ForeignLink { row: index })?;

    let titles = campus_html::scan(inner, "strong").map_err(map_scan_error)?;
    let title = titles
        .iter()
        .map(RawElement::text)
        .find_map(|text| bounded_text(&text, MAX_TEXT_CHARS))
        .ok_or(ReservesParseError::MissingTitle { row: index })?;

    let paragraphs = campus_html::scan(inner, "p").map_err(map_scan_error)?;
    let mut isbn = None;
    let mut author = None;
    let mut publisher = None;
    for paragraph in &paragraphs {
        let text = paragraph.text();
        if isbn.is_none() {
            isbn = labelled_tail(&text, "ISBN");
        }
        if author.is_none() {
            author = labelled_tail(&text, "责任者");
        }
        if publisher.is_none() {
            publisher = labelled_tail(&text, "出版项");
        }
    }
    let (Some(isbn), Some(author), Some(publisher)) = (isbn, author, publisher) else {
        return Err(ReservesParseError::MissingField { row: index });
    };

    Ok(ReservesSearchRow {
        title,
        image_url,
        isbn,
        author,
        publisher,
        book_id,
    })
}

/// Reads the text that follows one of the page's own printed labels.
///
/// The fragment is one complete paragraph, so the value is the whole remainder
/// without any further delimiter to guess.
fn labelled_tail(text: &str, label: &str) -> Option<String> {
    let start = text.find(label)? + label.len();
    let rest = text[start..].trim_start_matches(['：', ':']).trim_start();
    bounded_text(rest, MAX_TEXT_CHARS)
}

/// Parses one book's detail page.
///
/// The six printed fields are read from the row's second `font`, which is where
/// the reference reads them.  The page's own labels re-assign a row only when
/// all six are present and distinct, so a partial match falls back to the
/// service's layout order instead of shifting one field onto another row.  A
/// page missing any value is an error, so a renamed deployment cannot present a
/// book with empty fields.
pub fn parse_reserves_detail_html(body: &str) -> Result<ReservesBookDetail, ReservesParseError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(ReservesParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(ReservesParseError::LoginPage),
        PageClass::Expired => return Err(ReservesParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    if trimmed.contains(RESERVES_LOGIN_MARKER) {
        return Err(ReservesParseError::LoginPage);
    }
    let rows = detail_field_rows(trimmed)?;
    let slots: Vec<Option<usize>> = rows
        .iter()
        .map(|row| {
            let text = row.text();
            DETAIL_FIELD_LABELS
                .iter()
                .position(|label| text.contains(label))
        })
        .collect();
    let labelled: BTreeSet<usize> = slots.iter().flatten().copied().collect();
    let mut fields: [Option<String>; 6] = Default::default();
    if labelled.len() == DETAIL_FIELD_LABELS.len() {
        for (row, slot) in rows.iter().zip(slots.iter()) {
            if let Some(slot) = slot
                && fields[*slot].is_none()
            {
                fields[*slot] = detail_value_cell(row);
            }
        }
    } else {
        for (slot, row) in rows.iter().take(DETAIL_FIELD_LABELS.len()).enumerate() {
            fields[slot] = detail_value_cell(row);
        }
    }
    let [title, author, publisher, isbn, version, volume] = fields;
    let (Some(title), Some(author), Some(publisher), Some(isbn), Some(version), Some(volume)) =
        (title, author, publisher, isbn, version, volume)
    else {
        return Err(ReservesParseError::MissingDetailField);
    };

    let mut paragraphs: Vec<RawElement> = Vec::new();
    let mut images: Vec<RawElement> = Vec::new();
    for block in scan_by_class(trimmed, DETAIL_RESULT_CLASS).map_err(map_scan_error)? {
        paragraphs.extend(campus_html::scan(block.inner(), "p").map_err(map_scan_error)?);
        images.extend(campus_html::scan(block.inner(), "img").map_err(map_scan_error)?);
    }
    let image_url = images
        .iter()
        .find_map(|image| image.attr("src"))
        .and_then(mapped_asset_url)
        .ok_or(ReservesParseError::MissingDetailField)?;

    let mut chapters = Vec::new();
    // The reference reads the chapter anchors out of the second paragraph of
    // the result region, so that paragraph is the one this module reads too.
    if let Some(paragraph) = paragraphs.get(1) {
        for anchor in campus_html::scan(paragraph.inner(), "a").map_err(map_scan_error)? {
            let Some(url) = anchor.attr("href").and_then(mapped_asset_url) else {
                continue;
            };
            let Some(title) = bounded_text(&anchor.text(), MAX_TEXT_CHARS) else {
                continue;
            };
            chapters.push(ReservesChapter { title, url });
            if chapters.len() > MAX_CHAPTERS {
                return Err(ReservesParseError::TooManyChapters);
            }
        }
    }

    Ok(ReservesBookDetail {
        title,
        image_url,
        author,
        publisher,
        isbn,
        version,
        volume,
        chapters,
    })
}

/// The table rows carrying one book's printed fields.
fn detail_field_rows(body: &str) -> Result<Vec<RawElement>, ReservesParseError> {
    let bodies = campus_html::scan(body, "tbody").map_err(map_scan_error)?;
    let mut rows = Vec::new();
    for section in &bodies {
        rows.extend(campus_html::scan(section.inner(), "tr").map_err(map_scan_error)?);
    }
    if rows.len() < DETAIL_FIELD_LABELS.len() {
        return Err(ReservesParseError::MissingDetailField);
    }
    Ok(rows)
}

/// Reads a field's value from one row.
///
/// The reference reads the row's *second* `font` element, which is preferred
/// here so the label cell cannot be mistaken for the value.  When a row prints
/// no `font` at all, the row's last cell is the fallback.
fn detail_value_cell(row: &RawElement) -> Option<String> {
    let fonts = campus_html::scan(row.inner(), "font").ok()?;
    if fonts.len() >= 2 {
        return bounded_text(&fonts[1].text(), MAX_TEXT_CHARS);
    }
    let cells = campus_html::scan(row.inner(), "td").ok()?;
    let text = cells
        .last()
        .map(RawElement::text)
        .unwrap_or_else(|| row.text());
    bounded_text(&text, MAX_TEXT_CHARS)
}

/// Reads `bookId=<value>` out of one href.
fn read_book_id(href: &str) -> Option<String> {
    let start = href.find(BOOK_ID_QUERY_KEY)? + BOOK_ID_QUERY_KEY.len();
    let rest = &href[start..];
    let end = rest
        .find(|character: char| matches!(character, '"' | '\'' | '&' | '#' | '<' | '>' | ' '))
        .unwrap_or(rest.len());
    let value = rest[..end].trim();
    if value.is_empty() || value.len() > MAX_BOOK_ID_CHARS || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_owned())
}

/// Rewrites one page-printed asset URL onto this module's mapping origin.
///
/// The service prints both relative paths and absolute `webvpn.tsinghua.edu.cn`
/// paths (the reference prefixes the latter itself).  A value that names any
/// other host is refused rather than followed, so a response cannot move a
/// caller's cover or chapter fetch elsewhere.
fn mapped_asset_url(source: &str) -> Option<String> {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    let path = if let Some(rest) = trimmed.strip_prefix(RESERVES_MAPPING_ORIGIN) {
        if !rest.starts_with('/') {
            return None;
        }
        rest
    } else if trimmed.starts_with("//") || trimmed.contains(':') {
        // A scheme-relative or absolute URL names a host this module did not
        // choose, so it is refused instead of being rewritten.
        return None;
    } else {
        trimmed
    };
    if !path.starts_with('/')
        || path.contains(['\\', '#'])
        || path.contains("..")
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
    {
        return None;
    }
    Some(format!("{RESERVES_MAPPING_ORIGIN}{path}"))
}

/// Scans for a class regardless of the element that carries it.
///
/// The page's own markup is not documented element-by-element and the reference
/// selects these by class alone, so the tags are tried in the order a legacy
/// ASP page is most likely to use.  A tag is only tried when every earlier tag
/// produced nothing, which keeps the returned elements in document order within
/// one tag and keeps the ordering honest.
fn scan_by_class(body: &str, class: &str) -> Result<Vec<RawElement>, ScanError> {
    for tag in ["div", "ul", "li", "table", "tr", "td", "span", "p"] {
        let found = campus_html::scan_with_class(body, tag, class)?;
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

fn bounded_text(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    if trimmed.chars().count() > limit {
        // Truncating would hand back a value the page never printed, so an
        // over-long field is reported as unusable instead.
        return None;
    }
    Some(trimmed.to_owned())
}

fn map_scan_error(error: ScanError) -> ReservesParseError {
    match error {
        ScanError::TooLarge { .. } => ReservesParseError::TooLarge,
        ScanError::Unbalanced { context } => ReservesParseError::Malformed { context },
    }
}

/// Percent-encodes one query value.  The `bookId` is a service value, so it is
/// encoded rather than concatenated.
fn percent_encode_query_value(value: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if UNRESERVED.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Validates a query this module built.
///
/// Only two escape spellings are allowed: the service's own `%uXXXX` and a
/// well-formed `%XX`.  Anything else — including a bare `%` — is refused, so a
/// value that skipped [`encode_book_name`] cannot reach the wire.
fn valid_query(query: &str) -> bool {
    if query.is_empty() || query.chars().any(char::is_control) {
        return false;
    }
    let bytes = query.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }
        if bytes.get(index + 1) == Some(&b'u') {
            let Some(hex) = bytes.get(index + 2..index + 6) else {
                return false;
            };
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return false;
            }
            index += 6;
            continue;
        }
        if index + 2 >= bytes.len()
            || !bytes[index + 1].is_ascii_hexdigit()
            || !bytes[index + 2].is_ascii_hexdigit()
        {
            return false;
        }
        index += 3;
    }
    true
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, ReservesAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(ReservesAdapterError::InvalidBaseUrl);
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

/// Reduces a mapping URL to `/{scheme}/{token}/`, so a configured base can never
/// carry a deeper path of its own.
fn opaque_mapping_root(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let scheme = segments.next()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let token = segments.next()?;
    Some(format!("/{scheme}/{token}/"))
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, ReservesAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(ReservesAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| ReservesAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(ReservesAdapterError::UnexpectedOrigin);
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
        && path.len() <= MAX_PATH_CHARS
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
