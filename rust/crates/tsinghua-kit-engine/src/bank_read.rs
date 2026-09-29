//! Read-only bank payroll receipts (`银行代发`) and graduate income records
//! (`研究生收入`).
//!
//! Both are money documents a student reads as a statement: a department paid a
//! named project, and the row reports what was earned and what was actually
//! transferred.  They live on two different campus hosts behind two WebVPN
//! mappings, so each gets its own roaming selector and its own path constants,
//! while the amount handling, the bounded reads and the body-free failure
//! vocabulary are shared between them.
//!
//! Three service behaviours shape this module:
//!
//! * The payroll page is a **year form**, not a paginated list.  The first
//!   response carries the `<option>` set naming the years this account has
//!   receipts for; the receipts themselves come from a second request that
//!   names one or more of those years.  The year list is therefore the bound for
//!   everything that follows: no year is invented, and a caller cannot ask for
//!   a year the service never offered.
//! * That year request batches.  Several `year=…` fields may be posted at once
//!   and the service answers one document holding several month sections.  How
//!   wide a batch may be is a decision this module owns, because the reference
//!   client fans three requests out concurrently here and one call would then
//!   put three requests on the wire at once.  This module splits the years the
//!   same way but dispatches the batches **sequentially** through the shared
//!   transport and request gate, so a single call stays inside the gate's
//!   bounded read dispatch.
//! * Neither domain is read positionally.  The payroll table's header row names
//!   each column and every column this module reports is located by its
//!   **header label**, so a table whose columns moved fails as an unexpected
//!   header instead of reporting one column's amount as another's.  The
//!   graduate-income rows are keyed by the service's own field names.
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
use crate::money::json_cents;
use crate::transport::{CampusHttpTransport, TransportError};

/// The payroll host's WebVPN roaming selector.
pub const BANK_WEBVPN_TARGET: &str = "2A5182CB3F36E80395FC2091001BDEA6";

/// The education-foundation payroll host's WebVPN roaming selector.  Its rows
/// have the same shape as the main payroll's, so one parser reads both.
pub const FOUNDATION_BANK_WEBVPN_TARGET: &str = "C1ADD6B60D050B64E0C7B8F195CE89EC";

/// The graduate-income host's WebVPN roaming selector.
pub const GRADUATE_INCOME_WEBVPN_TARGET: &str = "C0AE458CEACD0912982A09DDF0C136DA";

/// The payroll year-form endpoint behind the selector above.
pub const BANK_SEARCH_PATH: &str = "/yhdfcx/search.do";

/// The education-foundation payroll year-form endpoint.
pub const FOUNDATION_BANK_SEARCH_PATH: &str = "/yhdfcx_jjh/search.do";

/// The graduate-income list endpoint.
pub const GRADUATE_INCOME_PATH: &str = "/b/yjsjzxt/v_yjszzjl_yjscwdfmx_cx/pageList";

/// The most income rows one graduate-income page is asked for.
pub const GRADUATE_INCOME_PAGE_SIZE: u32 = 1000;

/// The WebVPN mapping token for the payroll host.
pub(crate) const BANK_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421e9ff459a69247b59700f81b9991b26317dbd36ae";

/// The WebVPN mapping token for the graduate-income host.
pub(crate) const GRADUATE_INCOME_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421eaed4b9069377a517a1d88b89d1b37269c624d2b1c6925f37faea82b8d";

/// The longest HTML body this domain will parse.
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
/// The longest JSON body this domain will parse.
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;

/// The most years one payroll batch may name.
const MAX_YEARS_PER_BATCH: usize = 4;
/// The most batches one payroll read may dispatch.
const MAX_YEAR_BATCHES: usize = 24;
/// The most years the service may offer before the form is treated as
/// unrecognizable rather than walked.
const MAX_YEARS: usize = 96;
/// The most month sections one payroll read will report.
const MAX_MONTHS: usize = 512;
/// The most receipt rows one payroll read will report.
const MAX_RECEIPTS: usize = 8192;
/// The most income rows one graduate-income page will report.
const MAX_INCOME_ROWS: usize = 4096;

const MAX_TEXT_CHARS: usize = 512;
const MAX_PATH_CHARS: usize = 512;

/// The suffix every payroll month heading ends with.
const MONTH_HEADING_SUFFIX: &str = "银行代发结果";

/// The service's own field name for one payroll batch.
const YEAR_FIELD: &str = "year";

/// The payroll table's header labels, in the order this module reports them.
///
/// These are the contract: each label must appear in the header row, and the
/// amount labels are the ones whose text is converted to exact cents.
const RECEIPT_COLUMNS: [(&str, &str); 11] = [
    ("department", "代发部门"),
    ("project", "代发项目"),
    ("usage", "代发用途"),
    ("description", "代发说明"),
    ("bank", "开户银行"),
    ("time", "计税时间"),
    ("total", "应发金额"),
    ("deduction", "扣税金额"),
    ("actual", "实发金额"),
    ("deposit", "存折金额"),
    ("cash", "现金金额"),
];

/// The amount columns, which are read as exact integer cents.
const AMOUNT_COLUMNS: [&str; 5] = ["total", "deduction", "actual", "deposit", "cash"];

static NEXT_BANK_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);
static NEXT_GRADUATE_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// The payroll profiles only issue a GET and a fixed form POST, so no write
/// route can be smuggled into a read plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankPaymentMethod {
    Get,
    PostForm,
}

/// The observed payroll operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankPaymentOperation {
    ReadYears,
    ReadReceipts,
}

/// The observed graduate-income operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraduateIncomeOperation {
    ReadList,
}

/// Both domains require an already established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// Which payroll ledger a read addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankLedger {
    /// `银行代发`
    Main,
    /// `银行代发（基金会）`
    Foundation,
}

impl BankLedger {
    /// The roaming selector that proves this ledger's mapping.
    pub const fn webvpn_target(self) -> &'static str {
        match self {
            Self::Main => BANK_WEBVPN_TARGET,
            Self::Foundation => FOUNDATION_BANK_WEBVPN_TARGET,
        }
    }

    /// The year-form path that names this ledger's years.
    pub const fn search_path(self) -> &'static str {
        match self {
            Self::Main => BANK_SEARCH_PATH,
            Self::Foundation => FOUNDATION_BANK_SEARCH_PATH,
        }
    }
}

/// A transport-neutral payroll request plan: path, query and form fields only,
/// never an absolute WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankPaymentRequestPlan {
    pub operation: BankPaymentOperation,
    pub method: BankPaymentMethod,
    pub path: &'static str,
    pub query: &'static str,
    /// The serialized form body for `PostForm` plans; empty for `Get` plans.
    pub form: String,
    pub webvpn_target: &'static str,
    pub session_prerequisite: BankSessionPrerequisite,
}

impl BankPaymentRequestPlan {
    /// Returns the serialized query without the leading `?`.
    pub fn query_string(&self) -> &str {
        self.query
    }

    /// Returns the serialized form body.
    pub fn form_body(&self) -> &str {
        &self.form
    }
}

/// Fixed payroll route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BankPaymentProfile;

impl BankPaymentProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub fn years_request(self, ledger: BankLedger) -> BankPaymentRequestPlan {
        BankPaymentRequestPlan {
            operation: BankPaymentOperation::ReadYears,
            method: BankPaymentMethod::Get,
            path: ledger.search_path(),
            query: "",
            form: String::new(),
            webvpn_target: ledger.webvpn_target(),
            session_prerequisite: BankSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    /// Builds the batch that asks for `years` in one request.
    ///
    /// The caller never supplies the serialized body: the years come from the
    /// service's own option set and are re-encoded here, so no caller-supplied
    /// text can reach a request body.
    pub fn receipts_request(self, ledger: BankLedger, years: &[String]) -> BankPaymentRequestPlan {
        BankPaymentRequestPlan {
            operation: BankPaymentOperation::ReadReceipts,
            method: BankPaymentMethod::PostForm,
            path: ledger.search_path(),
            query: "",
            form: encode_year_form(years),
            webvpn_target: ledger.webvpn_target(),
            session_prerequisite: BankSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// A transport-neutral graduate-income request plan.  Its query is owned
/// because the date range is a caller argument, re-encoded from validated
/// digits rather than interpolated as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraduateIncomeRequestPlan {
    pub operation: GraduateIncomeOperation,
    pub method: BankPaymentMethod,
    pub path: &'static str,
    pub query: String,
    pub webvpn_target: &'static str,
    pub session_prerequisite: BankSessionPrerequisite,
}

impl GraduateIncomeRequestPlan {
    /// Returns the serialized query without the leading `?`.
    pub fn query_string(&self) -> &str {
        &self.query
    }
}

/// Fixed graduate-income route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraduateIncomeProfile;

impl GraduateIncomeProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        GRADUATE_INCOME_WEBVPN_TARGET
    }

    /// Builds the list request for a `YYYYMMDD` date range.
    ///
    /// Both bounds must be exactly eight ASCII digits.  A value that is not is
    /// refused here rather than being percent-encoded into the query, so the
    /// service never receives a caller's free text as a filter.
    pub fn list_request(
        self,
        begin: &str,
        end: &str,
    ) -> Result<GraduateIncomeRequestPlan, GraduateIncomeAdapterError> {
        if !is_date_bound(begin) || !is_date_bound(end) {
            return Err(GraduateIncomeAdapterError::InvalidDateRange);
        }
        let query = format!(
            "ffkssj={begin}&ffjssj={end}&_search=false&rows={GRADUATE_INCOME_PAGE_SIZE}&page=1&sidx=id&sord=asc"
        );
        Ok(GraduateIncomeRequestPlan {
            operation: GraduateIncomeOperation::ReadList,
            method: BankPaymentMethod::Get,
            path: GRADUATE_INCOME_PATH,
            query,
            webvpn_target: GRADUATE_INCOME_WEBVPN_TARGET,
            session_prerequisite: BankSessionPrerequisite::ExistingInfoWebVpnSession,
        })
    }
}

/// One parsed payroll row, before the adapter folds it into a month section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankReceiptRow {
    pub department: String,
    pub project: String,
    pub usage: String,
    pub description: String,
    pub bank: String,
    pub time: String,
    pub total_cents: Option<i64>,
    pub deduction_cents: Option<i64>,
    pub actual_cents: Option<i64>,
    pub deposit_cents: Option<i64>,
    pub cash_cents: Option<i64>,
}

/// One month's payroll receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankReceiptMonth {
    /// The month label exactly as the service rendered it, e.g. `2021年12月`.
    pub month: String,
    pub receipts: Vec<BankReceiptRow>,
}

/// Every month section one payroll read produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankPaymentLedger {
    pub months: Vec<BankReceiptMonth>,
}

impl BankPaymentLedger {
    /// The number of receipt rows across every month section.
    pub fn receipt_count(&self) -> usize {
        self.months.iter().map(|month| month.receipts.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.receipt_count() == 0
    }
}

/// One graduate-income record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraduateIncomeRecord {
    pub id: String,
    pub year: String,
    pub month: String,
    pub date: String,
    pub year_month: String,
    pub name: String,
    pub department: String,
    pub before_tax_cents: Option<i64>,
    pub after_tax_cents: Option<i64>,
    pub tax_cents: Option<i64>,
}

/// A validated graduate-income page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraduateIncomePage {
    pub records: Vec<GraduateIncomeRecord>,
    /// The total row count the service reported, when it reported one.
    pub total: Option<u64>,
}

impl GraduateIncomePage {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Parser failures retain only stable names.  They never keep response bytes,
/// Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BankPaymentParseError {
    #[error("payroll response body is empty")]
    EmptyBody,

    #[error("payroll response is an HTML login page")]
    LoginPage,

    #[error("payroll response is an expired or timed-out page")]
    ExpiredPage,

    #[error("the payroll year form carried no selectable years")]
    NoYears,

    #[error("the payroll year form was larger than this domain will walk")]
    TooManyYears,

    #[error("the payroll document carried no month section")]
    NoMonths,

    #[error("the payroll document carried {headings} month headings and {tables} receipt tables")]
    SectionCountMismatch { headings: usize, tables: usize },

    #[error("payroll receipt table {table} had no header row")]
    MissingHeader { table: usize },

    #[error("payroll receipt table {table} header did not carry the expected columns")]
    UnexpectedHeader { table: usize },

    #[error("payroll receipt table {table} row {row} is missing one of its columns")]
    UnrecognizedRow { table: usize, row: usize },

    #[error("payroll receipt table {table} row {row} reported an amount that is not exact")]
    InvalidAmount { table: usize, row: usize },

    #[error("payroll response exceeded the bounded element limit")]
    TooLarge,
}

impl BankPaymentParseError {
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

impl From<ScanError> for BankPaymentParseError {
    fn from(_: ScanError) -> Self {
        Self::TooLarge
    }
}

/// Parser failures for the graduate-income list.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GraduateIncomeParseError {
    #[error("graduate income response body is empty")]
    EmptyBody,

    #[error("graduate income response is an HTML login page")]
    LoginPage,

    #[error("graduate income response is an expired or timed-out page")]
    ExpiredPage,

    #[error("graduate income response is not a JSON document")]
    NotJson,

    #[error("graduate income response had no `object.rows` array")]
    MissingRows,

    #[error("graduate income row {row} is missing a required field")]
    MissingField { row: usize },

    #[error("graduate income row {row} reported an amount that is not exact")]
    InvalidAmount { row: usize },

    #[error("graduate income response exceeded the bounded row limit")]
    TooLarge,
}

impl GraduateIncomeParseError {
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

/// Adapter failures are body-free so an HTML login page cannot leak through a
/// debug or bridge DTO.
#[derive(Debug, Error)]
pub enum BankPaymentAdapterError {
    #[error("payroll base URL is invalid")]
    InvalidBaseUrl,

    #[error("payroll transport failed")]
    Transport(#[source] TransportError),

    #[error("payroll request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("payroll response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("payroll response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("payroll response is not the expected document type")]
    UnexpectedContentType,

    #[error("payroll INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("payroll response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("payroll response could not be parsed: {0}")]
    Parse(#[source] BankPaymentParseError),
}

impl BankPaymentAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "bank_config",
            Self::Transport(_) => "bank_network",
            Self::HttpStatus { .. } => "bank_http",
            Self::UnexpectedOrigin => "bank_origin",
            Self::UnexpectedPath => "bank_path",
            Self::UnexpectedContentType => "bank_content_type",
            Self::SessionExpired => "bank_auth_required",
            Self::UnexpectedDeployment => "bank_template",
            Self::Parse(BankPaymentParseError::EmptyBody) => "bank_body_empty",
            Self::Parse(BankPaymentParseError::LoginPage | BankPaymentParseError::ExpiredPage) => {
                "bank_auth_required"
            }
            Self::Parse(BankPaymentParseError::NoYears) => "bank_years_empty",
            Self::Parse(BankPaymentParseError::TooManyYears) => "bank_years_too_many",
            Self::Parse(BankPaymentParseError::NoMonths) => "bank_months_empty",
            Self::Parse(BankPaymentParseError::SectionCountMismatch { .. }) => {
                "bank_section_mismatch"
            }
            Self::Parse(BankPaymentParseError::MissingHeader { .. }) => "bank_header_missing",
            Self::Parse(BankPaymentParseError::UnexpectedHeader { .. }) => "bank_header_changed",
            Self::Parse(BankPaymentParseError::UnrecognizedRow { .. }) => "bank_row_shape",
            Self::Parse(BankPaymentParseError::InvalidAmount { .. }) => "bank_amount",
            Self::Parse(BankPaymentParseError::TooLarge) => "bank_size",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(BankPaymentParseError::LoginPage)
                | Self::Parse(BankPaymentParseError::ExpiredPage)
        )
    }
}

/// Adapter failures for the graduate-income list.
#[derive(Debug, Error)]
pub enum GraduateIncomeAdapterError {
    #[error("graduate income base URL is invalid")]
    InvalidBaseUrl,

    #[error("graduate income date range must be two YYYYMMDD bounds")]
    InvalidDateRange,

    #[error("graduate income transport failed")]
    Transport(#[source] TransportError),

    #[error("graduate income request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("graduate income response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("graduate income response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("graduate income response is not the expected document type")]
    UnexpectedContentType,

    #[error("graduate income INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("graduate income response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("graduate income response could not be parsed: {0}")]
    Parse(#[source] GraduateIncomeParseError),
}

impl GraduateIncomeAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "graduate_income_config",
            Self::InvalidDateRange => "graduate_income_range",
            Self::Transport(_) => "graduate_income_network",
            Self::HttpStatus { .. } => "graduate_income_http",
            Self::UnexpectedOrigin => "graduate_income_origin",
            Self::UnexpectedPath => "graduate_income_path",
            Self::UnexpectedContentType => "graduate_income_content_type",
            Self::SessionExpired => "graduate_income_auth_required",
            Self::UnexpectedDeployment => "graduate_income_template",
            Self::Parse(GraduateIncomeParseError::EmptyBody) => "graduate_income_body_empty",
            Self::Parse(
                GraduateIncomeParseError::LoginPage | GraduateIncomeParseError::ExpiredPage,
            ) => "graduate_income_auth_required",
            Self::Parse(GraduateIncomeParseError::NotJson) => "graduate_income_not_json",
            Self::Parse(GraduateIncomeParseError::MissingRows) => "graduate_income_rows_missing",
            Self::Parse(GraduateIncomeParseError::MissingField { .. }) => "graduate_income_row",
            Self::Parse(GraduateIncomeParseError::InvalidAmount { .. }) => "graduate_income_amount",
            Self::Parse(GraduateIncomeParseError::TooLarge) => "graduate_income_size",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(GraduateIncomeParseError::LoginPage)
                | Self::Parse(GraduateIncomeParseError::ExpiredPage)
        )
    }
}

/// Configuration for a read-only payroll adapter.
#[derive(Clone)]
pub struct BankPaymentAdapterConfig {
    base_url: Url,
    ledger: BankLedger,
    user_agent: String,
    timeout: Duration,
}

impl BankPaymentAdapterConfig {
    pub fn new(base_url: &str, ledger: BankLedger) -> Result<Self, BankPaymentAdapterError> {
        Self::with_user_agent_and_timeout(
            base_url,
            ledger,
            "THYou/bank-payment",
            Duration::from_secs(30),
        )
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        ledger: BankLedger,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, BankPaymentAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| BankPaymentAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_mapping_root(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(BankPaymentAdapterError::InvalidBaseUrl);
        }
        Ok(Self {
            base_url,
            ledger,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn ledger(&self) -> BankLedger {
        self.ledger
    }
}

impl fmt::Debug for BankPaymentAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BankPaymentAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("ledger", &self.ledger)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that a payroll adapter parsed one response.  It is opaque: no Cookie,
/// URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct BankPaymentBusinessProof {
    adapter_binding: u64,
    generation: u64,
    ledger: BankLedger,
    operation: BankPaymentOperation,
}

impl fmt::Debug for BankPaymentBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BankPaymentBusinessProof")
            .field("ledger", &self.ledger)
            .field("operation", &self.operation)
            .finish()
    }
}

/// Proof that a graduate-income adapter parsed one page.
#[derive(Clone, PartialEq, Eq)]
pub struct GraduateIncomeBusinessProof {
    adapter_binding: u64,
    generation: u64,
    operation: GraduateIncomeOperation,
}

impl fmt::Debug for GraduateIncomeBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraduateIncomeBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// One ledger read together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankPaymentRead {
    pub value: BankPaymentLedger,
    pub proof: BankPaymentBusinessProof,
}

/// One graduate-income page together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraduateIncomeRead {
    pub value: GraduateIncomePage,
    pub proof: GraduateIncomeBusinessProof,
}

/// Read-only payroll client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct BankPaymentAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    ledger: BankLedger,
    profile: BankPaymentProfile,
    binding: u64,
    generation: Mutex<u64>,
}

impl BankPaymentAdapter {
    pub fn new(config: BankPaymentAdapterConfig) -> Result<Self, BankPaymentAdapterError> {
        let transport = CampusHttpTransport::with_timeout(&config.user_agent, config.timeout)
            .map_err(BankPaymentAdapterError::Transport)?;
        Self::try_with_transport(config.base_url, config.ledger, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        ledger: BankLedger,
        transport: CampusHttpTransport,
    ) -> Result<Self, BankPaymentAdapterError> {
        let base_url = normalize_mapping_root(base_url)?;
        Ok(Self {
            base_url,
            transport,
            ledger,
            profile: BankPaymentProfile::standard(),
            binding: NEXT_BANK_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            generation: Mutex::new(0),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> BankPaymentProfile {
        self.profile
    }

    pub fn ledger(&self) -> BankLedger {
        self.ledger
    }

    /// The current generation.  It advances once per accepted receipt read.
    pub fn generation(&self) -> u64 {
        self.generation
            .lock()
            .map(|value| *value)
            .unwrap_or_default()
    }

    /// Reads the years the service offers this account.
    ///
    /// The year list is the bound for everything that follows: a caller cannot
    /// name a year the service never offered.
    pub async fn read_years(&self) -> Result<Vec<String>, BankPaymentAdapterError> {
        let plan = self.profile.years_request(self.ledger);
        let body = self.execute(&plan).await?;
        parse_bank_years_html(&body).map_err(BankPaymentAdapter::map_parse_error)
    }

    /// Reads every receipt the account has, across every year the service
    /// offered, in bounded sequential batches.
    pub async fn read_ledger(&self) -> Result<BankPaymentLedger, BankPaymentAdapterError> {
        self.read_ledger_with_proof().await.map(|read| read.value)
    }

    pub async fn read_ledger_with_proof(&self) -> Result<BankPaymentRead, BankPaymentAdapterError> {
        let years = self.read_years().await?;
        let mut months: Vec<BankReceiptMonth> = Vec::new();
        let mut batches = 0usize;
        for batch in years.chunks(MAX_YEARS_PER_BATCH) {
            batches += 1;
            if batches > MAX_YEAR_BATCHES {
                return Err(BankPaymentAdapterError::Parse(
                    BankPaymentParseError::TooManyYears,
                ));
            }
            let plan = self.profile.receipts_request(self.ledger, batch);
            let body = self.execute(&plan).await?;
            let parsed =
                parse_bank_receipts_html(&body).map_err(BankPaymentAdapter::map_parse_error)?;
            months.extend(parsed.months);
        }
        if months.len() > MAX_MONTHS {
            return Err(BankPaymentAdapterError::Parse(
                BankPaymentParseError::TooLarge,
            ));
        }
        let generation = self
            .generation
            .lock()
            .map(|value| value.wrapping_add(1))
            .unwrap_or_default();
        if let Ok(mut current) = self.generation.lock() {
            *current = generation;
        }
        Ok(BankPaymentRead {
            value: BankPaymentLedger { months },
            proof: BankPaymentBusinessProof {
                adapter_binding: self.binding,
                generation,
                ledger: self.ledger,
                operation: BankPaymentOperation::ReadReceipts,
            },
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &BankPaymentBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    async fn execute(
        &self,
        plan: &BankPaymentRequestPlan,
    ) -> Result<String, BankPaymentAdapterError> {
        let endpoint = self.endpoint(plan)?;
        let request = match plan.method {
            BankPaymentMethod::Get => self.transport.client().get(endpoint.clone()),
            BankPaymentMethod::PostForm => {
                if plan.form.is_empty() {
                    return Err(BankPaymentAdapterError::InvalidBaseUrl);
                }
                self.transport
                    .client()
                    .post(endpoint.clone())
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(plan.form.clone())
            }
        };
        let response =
            self.transport.send(request).await.map_err(|error| {
                BankPaymentAdapterError::Transport(TransportError::Request(error))
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
            .map_err(|_| BankPaymentAdapterError::UnexpectedOrigin)?;
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
            return Err(BankPaymentAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(BankPaymentAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(BankPaymentAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| BankPaymentAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_HTML_BYTES {
            return Err(BankPaymentAdapterError::UnexpectedDeployment);
        }
        // The service's own session-expiry banner arrives as a 200 body, so it
        // is classified here, before the scanner can turn it into a "no table"
        // failure.
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(BankPaymentAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        if status != StatusCode::OK {
            return Err(BankPaymentAdapterError::HttpStatus { status });
        }
        if final_url.path() != endpoint.path()
            || final_url.query() != endpoint.query()
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(BankPaymentAdapterError::UnexpectedPath);
        }
        if !is_html_content_type(content_type.as_deref()) {
            return Err(BankPaymentAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    fn endpoint(&self, plan: &BankPaymentRequestPlan) -> Result<Url, BankPaymentAdapterError> {
        if !valid_relative_path(plan.path) || plan.query.chars().any(char::is_control) {
            return Err(BankPaymentAdapterError::InvalidBaseUrl);
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

    fn map_parse_error(error: BankPaymentParseError) -> BankPaymentAdapterError {
        match error {
            BankPaymentParseError::LoginPage | BankPaymentParseError::ExpiredPage => {
                BankPaymentAdapterError::SessionExpired
            }
            other => BankPaymentAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for BankPaymentAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BankPaymentAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("ledger", &self.ledger)
            .field("profile", &self.profile)
            .field("generation", &self.generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Read-only graduate-income client.
pub struct GraduateIncomeAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: GraduateIncomeProfile,
    binding: u64,
    generation: Mutex<u64>,
}

impl GraduateIncomeAdapter {
    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, GraduateIncomeAdapterError> {
        let base_url = normalize_mapping_root_generic(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: GraduateIncomeProfile::standard(),
            binding: NEXT_GRADUATE_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            generation: Mutex::new(0),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> GraduateIncomeProfile {
        self.profile
    }

    pub fn generation(&self) -> u64 {
        self.generation
            .lock()
            .map(|value| *value)
            .unwrap_or_default()
    }

    /// Reads one page of graduate-income records for a `YYYYMMDD` date range.
    pub async fn read_list(
        &self,
        begin: &str,
        end: &str,
    ) -> Result<GraduateIncomePage, GraduateIncomeAdapterError> {
        self.read_list_with_proof(begin, end)
            .await
            .map(|read| read.value)
    }

    pub async fn read_list_with_proof(
        &self,
        begin: &str,
        end: &str,
    ) -> Result<GraduateIncomeRead, GraduateIncomeAdapterError> {
        let plan = self.profile.list_request(begin, end)?;
        let body = self.execute(&plan).await?;
        let parsed =
            parse_graduate_income_json(&body).map_err(GraduateIncomeAdapter::map_parse_error)?;
        let generation = self
            .generation
            .lock()
            .map(|value| value.wrapping_add(1))
            .unwrap_or_default();
        if let Ok(mut current) = self.generation.lock() {
            *current = generation;
        }
        Ok(GraduateIncomeRead {
            value: parsed,
            proof: GraduateIncomeBusinessProof {
                adapter_binding: self.binding,
                generation,
                operation: GraduateIncomeOperation::ReadList,
            },
        })
    }

    pub fn business_proof_matches(&self, proof: &GraduateIncomeBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    async fn execute(
        &self,
        plan: &GraduateIncomeRequestPlan,
    ) -> Result<String, GraduateIncomeAdapterError> {
        if !valid_relative_path(plan.path) || plan.query.chars().any(char::is_control) {
            return Err(GraduateIncomeAdapterError::InvalidBaseUrl);
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
        let response = self
            .transport
            .send(self.transport.client().get(endpoint.clone()))
            .await
            .map_err(|error| {
                GraduateIncomeAdapterError::Transport(TransportError::Request(error))
            })?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
        {
            return Err(GraduateIncomeAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url) {
            return Err(GraduateIncomeAdapterError::UnexpectedOrigin);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| {
                GraduateIncomeAdapterError::Transport(TransportError::Decode(error))
            })?;
        if body.len() > MAX_JSON_BYTES {
            return Err(GraduateIncomeAdapterError::UnexpectedDeployment);
        }
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(GraduateIncomeAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        if status != StatusCode::OK {
            return Err(GraduateIncomeAdapterError::HttpStatus { status });
        }
        if final_url.path() != endpoint.path()
            || final_url.query() != endpoint.query()
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(GraduateIncomeAdapterError::UnexpectedPath);
        }
        if !is_json_content_type(content_type.as_deref()) {
            return Err(GraduateIncomeAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    fn map_parse_error(error: GraduateIncomeParseError) -> GraduateIncomeAdapterError {
        match error {
            GraduateIncomeParseError::LoginPage | GraduateIncomeParseError::ExpiredPage => {
                GraduateIncomeAdapterError::SessionExpired
            }
            other => GraduateIncomeAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for GraduateIncomeAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraduateIncomeAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("generation", &self.generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Serializes the payroll batch body.
///
/// Every year is percent-encoded here and joined with `&`, so a caller cannot
/// reach the request body with text of its own: the only inputs are the years
/// the service itself offered.
fn encode_year_form(years: &[String]) -> String {
    years
        .iter()
        .map(|year| format!("{YEAR_FIELD}={}", percent_encode_query_value(year)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes one form value.
fn percent_encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// True when `value` is exactly eight ASCII digits.
fn is_date_bound(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Runs the session-level guards that must precede any structural parse.
fn guard_page(html: &str) -> Result<&str, BankPaymentParseError> {
    let trimmed = html.strip_prefix('\u{feff}').unwrap_or(html).trim();
    if trimmed.is_empty() {
        return Err(BankPaymentParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(BankPaymentParseError::LoginPage),
        PageClass::Expired => return Err(BankPaymentParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    Ok(trimmed)
}

/// Parses the years the payroll year form offers.
///
/// The values come from the service's own `<option>` set and are bounded in
/// both count and length.  An empty set is a failure, not an empty result: this
/// page exists to offer years, so no options means it did not render.
pub fn parse_bank_years_html(html: &str) -> Result<Vec<String>, BankPaymentParseError> {
    let trimmed = guard_page(html)?;
    let mut years = Vec::new();
    for option in campus_html::scan(trimmed, "option")? {
        let Some(value) = option.attr("value") else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() || value.chars().any(char::is_control) || value.len() > 32 {
            continue;
        }
        if years.iter().any(|existing| existing == value) {
            continue;
        }
        years.push(value.to_owned());
        if years.len() > MAX_YEARS {
            return Err(BankPaymentParseError::TooManyYears);
        }
    }
    if years.is_empty() {
        return Err(BankPaymentParseError::NoYears);
    }
    Ok(years)
}

/// Parses one payroll receipts document.
///
/// The document is a series of `年月银行代发结果` headings, each introducing one
/// receipt table.  Both sets are collected in document order and required to
/// pair up one-to-one: a heading whose table is missing, or a table no heading
/// introduces, is a changed deployment rather than something to guess at.
pub fn parse_bank_receipts_html(html: &str) -> Result<BankPaymentLedger, BankPaymentParseError> {
    let trimmed = guard_page(html)?;
    let mut headings = Vec::new();
    for strong in campus_html::scan(trimmed, "strong")? {
        if let Some(month) = month_from_heading(&strong.text()) {
            headings.push(month);
        }
    }
    let tables: Vec<RawElement> = campus_html::scan(trimmed, "table")?
        .into_iter()
        .filter(|table| receipt_header_positions(table).is_some())
        .collect();
    if headings.is_empty() && tables.is_empty() {
        return Err(BankPaymentParseError::NoMonths);
    }
    if headings.len() != tables.len() {
        return Err(BankPaymentParseError::SectionCountMismatch {
            headings: headings.len(),
            tables: tables.len(),
        });
    }
    if headings.len() > MAX_MONTHS {
        return Err(BankPaymentParseError::TooLarge);
    }
    let mut months = Vec::with_capacity(headings.len());
    for (index, (month, table)) in headings.into_iter().zip(tables.iter()).enumerate() {
        let positions = receipt_header_positions(table)
            .ok_or(BankPaymentParseError::MissingHeader { table: index })?;
        let receipts = parse_receipt_rows(table, index, &positions)?;
        months.push(BankReceiptMonth { month, receipts });
    }
    Ok(BankPaymentLedger { months })
}

/// Extracts `yyyy年MM月` from a payroll heading, rejecting anything else.
///
/// The heading ends with the service's own suffix; the text before it must end
/// with digits after a `年`.  A reworded heading therefore yields no month at
/// all instead of a slice of unrelated text.
fn month_from_heading(text: &str) -> Option<String> {
    let head = text.trim().strip_suffix(MONTH_HEADING_SUFFIX)?.trim();
    let month = head.strip_suffix('月')?;
    let (year, month) = month.rsplit_once('年')?;
    let year = year.trim();
    let month = month.trim();
    if year.is_empty()
        || year.len() > 4
        || !year.bytes().all(|byte| byte.is_ascii_digit())
        || month.is_empty()
        || month.len() > 2
        || !month.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(format!("{year}年{month}月"))
}

/// Locates each reported column in a payroll table's header row.
///
/// Returns `None` when the table's first row does not carry every expected
/// label, which is what keeps a search form's table, a footer's table, or a
/// payroll table whose columns moved from being read as receipts.
fn receipt_header_positions(table: &RawElement) -> Option<Vec<(&'static str, usize)>> {
    let header = table_rows(table).into_iter().next()?;
    let cells = campus_html::direct_children(header.inner(), "th")
        .or_else(|_| campus_html::direct_children(header.inner(), "td"))
        .ok()?;
    if cells.is_empty() {
        return None;
    }
    let labels: Vec<String> = cells
        .iter()
        .map(|cell| cell.text().split_whitespace().collect::<String>())
        .collect();
    let mut positions = Vec::with_capacity(RECEIPT_COLUMNS.len());
    for (field, label) in RECEIPT_COLUMNS {
        let position = labels.iter().position(|value| value == label)?;
        positions.push((field, position));
    }
    Some(positions)
}

/// Returns a payroll table's row elements, preferring its `tbody` when it has
/// one so a wrapper row cannot stand in for the header.
fn table_rows(table: &RawElement) -> Vec<RawElement> {
    if let Ok(bodies) = campus_html::direct_children(table.inner(), "tbody") {
        if let Some(body) = bodies.first() {
            if let Ok(rows) = campus_html::direct_children(body.inner(), "tr") {
                if !rows.is_empty() {
                    return rows;
                }
            }
        }
    }
    campus_html::direct_children(table.inner(), "tr").unwrap_or_default()
}

/// Reads one table's receipt rows using the located header positions.
///
/// The legacy table ends with a totals row that has no receipt columns.  A row
/// that cannot be attributed is therefore skipped when it is the table's last
/// row — the same one row the reference client drops — and is an error
/// anywhere else, so a shape change in the middle of the table still fails.
fn parse_receipt_rows(
    table: &RawElement,
    table_index: usize,
    positions: &[(&'static str, usize)],
) -> Result<Vec<BankReceiptRow>, BankPaymentParseError> {
    let rows = table_rows(table);
    let last = rows.len().saturating_sub(1);
    let mut receipts = Vec::new();
    let mut row_index = 0usize;
    for (position, row) in rows.iter().enumerate() {
        if position == 0 {
            // The header row; its labels were already validated.
            continue;
        }
        if receipts.len() >= MAX_RECEIPTS {
            return Err(BankPaymentParseError::TooLarge);
        }
        let cells = campus_html::direct_children(row.inner(), "td")?;
        let attribute = |column: usize| cells.get(column).map(|cell| bounded(&cell.text()));
        let mut receipt = BankReceiptRow {
            department: String::new(),
            project: String::new(),
            usage: String::new(),
            description: String::new(),
            bank: String::new(),
            time: String::new(),
            total_cents: None,
            deduction_cents: None,
            actual_cents: None,
            deposit_cents: None,
            cash_cents: None,
        };
        let mut complete = true;
        for (field, column) in positions {
            let Some(value) = attribute(*column) else {
                complete = false;
                break;
            };
            let slot = match *field {
                "department" => &mut receipt.department,
                "project" => &mut receipt.project,
                "usage" => &mut receipt.usage,
                "description" => &mut receipt.description,
                "bank" => &mut receipt.bank,
                "time" => &mut receipt.time,
                _ => continue,
            };
            *slot = value;
        }
        if !complete {
            if position == last {
                break;
            }
            return Err(BankPaymentParseError::UnrecognizedRow {
                table: table_index,
                row: row_index,
            });
        }
        for field in AMOUNT_COLUMNS {
            let Some((_, column)) = positions.iter().find(|(name, _)| *name == field) else {
                continue;
            };
            let raw = attribute(*column).unwrap_or_default();
            let amount = if raw.is_empty() {
                None
            } else {
                Some(crate::money::exact_cents(&raw).ok_or(
                    BankPaymentParseError::InvalidAmount {
                        table: table_index,
                        row: row_index,
                    },
                )?)
            };
            match field {
                "total" => receipt.total_cents = amount,
                "deduction" => receipt.deduction_cents = amount,
                "actual" => receipt.actual_cents = amount,
                "deposit" => receipt.deposit_cents = amount,
                "cash" => receipt.cash_cents = amount,
                _ => {}
            }
        }
        receipts.push(receipt);
        row_index += 1;
    }
    Ok(receipts)
}

/// Parses the graduate-income list.
///
/// The list is JSON under `object.rows`.  A missing `rows` key is a failure, not
/// an empty list: the service reports an empty page with `rows: []`, so a
/// response without the key is a changed deployment rather than "no income".
pub fn parse_graduate_income_json(
    body: &str,
) -> Result<GraduateIncomePage, GraduateIncomeParseError> {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    if trimmed.is_empty() {
        return Err(GraduateIncomeParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(GraduateIncomeParseError::LoginPage),
        PageClass::Expired => return Err(GraduateIncomeParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| GraduateIncomeParseError::NotJson)?;
    let object = value
        .get("object")
        .ok_or(GraduateIncomeParseError::MissingRows)?;
    let rows = object
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .ok_or(GraduateIncomeParseError::MissingRows)?;
    if rows.len() > MAX_INCOME_ROWS {
        return Err(GraduateIncomeParseError::TooLarge);
    }
    let mut records = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let Some(fields) = row.as_object() else {
            return Err(GraduateIncomeParseError::MissingField { row: index });
        };
        records.push(GraduateIncomeRecord {
            id: required_income_text(fields, "id", index)?,
            year: income_text(fields, "ffnf"),
            month: income_text(fields, "ffyf"),
            date: income_text(fields, "ffrq"),
            year_month: income_text(fields, "ffrqChs"),
            name: required_income_text(fields, "dfytmc", index)?,
            department: income_text(fields, "xmssbmmc"),
            before_tax_cents: income_cents(fields, "yfje", index)?,
            after_tax_cents: income_cents(fields, "sfje", index)?,
            tax_cents: income_cents(fields, "ksje", index)?,
        });
    }
    let total = object.get("total").and_then(|total| match total {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    });
    Ok(GraduateIncomePage { records, total })
}

fn income_text(fields: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    match fields.get(key) {
        Some(serde_json::Value::String(value)) => bounded(value),
        Some(serde_json::Value::Number(number)) => bounded(&number.to_string()),
        _ => String::new(),
    }
}

fn required_income_text(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    row: usize,
) -> Result<String, GraduateIncomeParseError> {
    let value = income_text(fields, key);
    if value.is_empty() {
        return Err(GraduateIncomeParseError::MissingField { row });
    }
    Ok(value)
}

fn income_cents(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    row: usize,
) -> Result<Option<i64>, GraduateIncomeParseError> {
    json_cents(fields, key).map_err(|_| GraduateIncomeParseError::InvalidAmount { row })
}

/// Keeps one field inside the bounded public shape.
fn bounded(value: &str) -> String {
    value.trim().chars().take(MAX_TEXT_CHARS).collect()
}

fn is_html_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
    })
}

fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("application/json")
            || mime.eq_ignore_ascii_case("text/json")
            || mime.eq_ignore_ascii_case("text/plain")
            || mime.eq_ignore_ascii_case("text/html")
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

fn normalize_mapping_root(base_url: Url) -> Result<Url, BankPaymentAdapterError> {
    match normalize_mapping_root_generic(base_url) {
        Ok(url) => Ok(url),
        Err(GraduateIncomeAdapterError::InvalidBaseUrl) => {
            Err(BankPaymentAdapterError::InvalidBaseUrl)
        }
        Err(_) => Err(BankPaymentAdapterError::InvalidBaseUrl),
    }
}

fn normalize_mapping_root_generic(base_url: Url) -> Result<Url, GraduateIncomeAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(GraduateIncomeAdapterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = opaque_mapping_root(path).unwrap_or_else(|| {
        if path.is_empty() {
            "/".to_owned()
        } else {
            format!("{path}/")
        }
    });
    let mut base_url = base_url;
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

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, BankPaymentAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(BankPaymentAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| BankPaymentAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(BankPaymentAdapterError::UnexpectedOrigin);
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
