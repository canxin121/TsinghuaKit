//! Read-only profile and strict parser for the Tsinghua campus card.
//!
//! The transport and card-specific SSO live in `campus_card_adapter`. This
//! module owns the public request plans and the cleaned account/transaction
//! records used by the runtime. The account serial is deliberately absent
//! from request plans and returned records; the adapter inserts it only after
//! the card service has proved the shared Cookie session.
//!
//! The parser accepts the card service's JSON envelope and classifies HTML
//! responses, explicit session failures, malformed responses, and valid empty
//! transaction lists separately. It never treats an HTML response as proof of
//! session expiry by itself: the adapter needs the response route before it
//! can classify a redirect as an authentication handoff.

use std::fmt;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value};
use thiserror::Error;

/// Path used to verify that the card SSO session resolves to an account.
///
/// The card-service origin is deliberately owned by the adapter/runtime.
pub const CARD_SESSION_USER_PATH: &str = "/login/getUserInfoFromToken";

/// Path used to read account and card information.
pub const CARD_ACCOUNT_PATH: &str = "/business/getCardUserinfo";

/// Path used to read card transactions.
pub const CARD_TRANSACTIONS_PATH: &str = "/business/querySelfTradeList";

/// Same-origin route emitted by the card portal when it needs the TYSF login
/// handoff again. This is a route marker, not a general HTML/login detector.
pub const CARD_SESSION_REDIRECT_PATH: &str = "/getTYSFLoginUrlRedirect";

/// Maximum number of transaction rows requested by one read plan.
///
/// The public reference sends `pageSize=10000`, but the THYou read profile
/// deliberately caps one request at 500 rows. The field name and JSON number
/// remain the observed wire contract while the local bound prevents a single
/// read from turning into an unbounded account export. Callers still choose
/// the page explicitly.
pub const MAX_TRANSACTION_PAGE_SIZE: u16 = 500;

/// Maximum page number accepted by a transaction read plan.
pub const MAX_TRANSACTION_PAGE_NUMBER: u32 = 10_000;

/// Maximum date span accepted by a transaction read plan.
pub const MAX_TRANSACTION_WINDOW_DAYS: u32 = 31;

/// The only HTTP method used by the observed read endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CampusCardRequestMethod {
    Post,
}

/// Authentication is a capability requirement, not a credential.
///
/// The adapter must establish the identity-to-card SSO on the shared
/// `CampusHttpTransport` and then prove the card account before executing a
/// business request. No Cookie, token, or SSO payload is represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampusCardSessionRequirement {
    ExistingIdentityAndCardSso,
}

/// Name of a read operation in the campus card profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampusCardReadOperation {
    VerifySession,
    ReadAccount,
    ReadTransactions,
}

impl CampusCardReadOperation {
    /// Returns the service path without an origin, cookie, or query string.
    pub const fn path(self) -> &'static str {
        match self {
            Self::VerifySession => CARD_SESSION_USER_PATH,
            Self::ReadAccount => CARD_ACCOUNT_PATH,
            Self::ReadTransactions => CARD_TRANSACTIONS_PATH,
        }
    }
}

impl fmt::Display for CampusCardReadOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::VerifySession => "session verification",
            Self::ReadAccount => "account read",
            Self::ReadTransactions => "transaction read",
        })
    }
}

/// A valid calendar date used by a transaction query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CampusCardDate {
    value: String,
    year: u16,
    month: u16,
    day: u16,
}

impl CampusCardDate {
    /// Parses the strict wire format YYYY-MM-DD.
    pub fn parse(value: &str, field: CampusCardDateField) -> Result<Self, CampusCardRequestError> {
        let bytes = value.as_bytes();
        if bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        {
            return Err(CampusCardRequestError::InvalidDate { field });
        }

        let year = parse_date_component(&bytes[0..4]);
        let month = parse_date_component(&bytes[5..7]);
        let day = parse_date_component(&bytes[8..10]);
        let (Some(year), Some(month), Some(day)) = (year, month, day) else {
            return Err(CampusCardRequestError::InvalidDate { field });
        };
        if !(1900..=2200).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=u16::from(days_in_month(year, month))).contains(&day)
        {
            return Err(CampusCardRequestError::InvalidDate { field });
        }

        Ok(Self {
            value: value.to_owned(),
            year,
            month,
            day,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl<'de> Deserialize<'de> for CampusCardDate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireDate {
            value: String,
            year: u16,
            month: u16,
            day: u16,
        }

        let wire = WireDate::deserialize(deserializer)?;
        let parsed =
            Self::parse(&wire.value, CampusCardDateField::StartDate).map_err(de::Error::custom)?;
        if parsed.year != wire.year || parsed.month != wire.month || parsed.day != wire.day {
            return Err(de::Error::custom(
                "campus card date components do not match value",
            ));
        }
        Ok(parsed)
    }
}

impl fmt::Display for CampusCardDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.value)
    }
}

fn parse_date_component(bytes: &[u8]) -> Option<u16> {
    (bytes.len() == 2 || bytes.len() == 4)
        .then(|| {
            bytes.iter().try_fold(0_u16, |value, byte| {
                value.checked_mul(10)?.checked_add(u16::from(byte - b'0'))
            })
        })
        .flatten()
}

fn is_leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: u16, month: u16) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn ordinal_day(date: &CampusCardDate) -> u32 {
    let completed_years = (1900..date.year)
        .map(|year| if is_leap_year(year) { 366 } else { 365 })
        .sum::<u32>();
    let completed_months = (1..date.month)
        .map(|month| u32::from(days_in_month(date.year, month)))
        .sum::<u32>();
    completed_years + completed_months + u32::from(date.day)
}

/// Date fields accepted by a transaction request plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampusCardDateField {
    StartDate,
    EndDate,
}

impl fmt::Display for CampusCardDateField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StartDate => "start date",
            Self::EndDate => "end date",
        })
    }
}

/// Errors raised while constructing a read-only campus card plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CampusCardRequestError {
    #[error("the campus card account binding is empty or contains invalid characters")]
    InvalidAccountBinding,

    #[error("the campus card {field} is not a valid YYYY-MM-DD date")]
    InvalidDate { field: CampusCardDateField },

    #[error("the campus card transaction range ends before it starts")]
    ReversedDateRange,

    #[error("the campus card transaction range exceeds {max_days} days")]
    DateRangeTooLarge { max_days: u32 },

    #[error("the campus card transaction page size must be between 1 and {max}")]
    InvalidPageSize { max: u16 },

    #[error("the campus card transaction page number is greater than {max}")]
    InvalidPageNumber { max: u32 },
}

/// A date-bounded, paginated transaction read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CampusCardTransactionQuery {
    start_date: CampusCardDate,
    end_date: CampusCardDate,
    transaction_type: CampusCardTransactionType,
    page_size: u16,
    page_number: u32,
}

impl<'de> Deserialize<'de> for CampusCardTransactionQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireQuery {
            start_date: CampusCardDate,
            end_date: CampusCardDate,
            transaction_type: CampusCardTransactionType,
            page_size: u16,
            page_number: u32,
        }

        let wire = WireQuery::deserialize(deserializer)?;
        Self::new(
            wire.start_date.as_str(),
            wire.end_date.as_str(),
            wire.transaction_type,
            wire.page_size,
            wire.page_number,
        )
        .map_err(de::Error::custom)
    }
}

impl CampusCardTransactionQuery {
    /// Creates a bounded transaction query.
    pub fn new(
        start_date: &str,
        end_date: &str,
        transaction_type: CampusCardTransactionType,
        page_size: u16,
        page_number: u32,
    ) -> Result<Self, CampusCardRequestError> {
        let start_date = CampusCardDate::parse(start_date, CampusCardDateField::StartDate)?;
        let end_date = CampusCardDate::parse(end_date, CampusCardDateField::EndDate)?;
        let start_ordinal = ordinal_day(&start_date);
        let end_ordinal = ordinal_day(&end_date);
        if end_ordinal < start_ordinal {
            return Err(CampusCardRequestError::ReversedDateRange);
        }
        // The bound is an inclusive calendar-day count. For example,
        // 2026-09-01 through 2026-09-30 is 30 days, while 2026-09-01
        // through 2026-10-01 is 31 days and remains valid.
        let calendar_days = end_ordinal - start_ordinal + 1;
        if calendar_days > MAX_TRANSACTION_WINDOW_DAYS {
            return Err(CampusCardRequestError::DateRangeTooLarge {
                max_days: MAX_TRANSACTION_WINDOW_DAYS,
            });
        }
        if !(1..=MAX_TRANSACTION_PAGE_SIZE).contains(&page_size) {
            return Err(CampusCardRequestError::InvalidPageSize {
                max: MAX_TRANSACTION_PAGE_SIZE,
            });
        }
        if page_number > MAX_TRANSACTION_PAGE_NUMBER {
            return Err(CampusCardRequestError::InvalidPageNumber {
                max: MAX_TRANSACTION_PAGE_NUMBER,
            });
        }

        Ok(Self {
            start_date,
            end_date,
            transaction_type,
            page_size,
            page_number,
        })
    }

    pub fn start_date(&self) -> &CampusCardDate {
        &self.start_date
    }

    pub fn end_date(&self) -> &CampusCardDate {
        &self.end_date
    }

    pub const fn transaction_type(&self) -> CampusCardTransactionType {
        self.transaction_type
    }

    pub const fn page_size(&self) -> u16 {
        self.page_size
    }

    pub const fn page_number(&self) -> u32 {
        self.page_number
    }

    /// Returns whether this query addresses the first server page.
    ///
    /// The page number is part of the request contract; callers must not infer
    /// that page zero contains the complete date range.
    pub const fn is_first_page(&self) -> bool {
        self.page_number == 0
    }
}

/// Transaction type values observed by the campus card service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CampusCardTransactionType {
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "consumption")]
    Consumption,
    #[serde(rename = "recharge")]
    Recharge,
    #[serde(rename = "subsidy")]
    Subsidy,
}

impl CampusCardTransactionType {
    pub const fn wire_value(self) -> i32 {
        match self {
            Self::Any => -1,
            Self::Consumption => 1,
            Self::Recharge => 2,
            Self::Subsidy => 3,
        }
    }
}

/// Safe request body description.
///
/// The actual `idserial` JSON field is session-bound and is inserted by the
/// adapter only after `getUserInfoFromToken` has returned a valid account.
/// It is intentionally represented by a capability marker rather than a
/// string here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CampusCardRequestBody {
    SessionProbe,
    CurrentSessionAccount,
    CurrentSessionTransactions { query: CampusCardTransactionQuery },
}

/// A typed read-only request plan.
///
/// This is the safe part of an HTTP request. The adapter resolves the service
/// origin, attaches the existing Cookie jar, and injects the session-bound
/// account parameter at execution time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CampusCardRequestPlan {
    pub operation: CampusCardReadOperation,
    pub method: CampusCardRequestMethod,
    pub authentication: CampusCardSessionRequirement,
    pub body: CampusCardRequestBody,
}

impl<'de> Deserialize<'de> for CampusCardRequestPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WirePlan {
            operation: CampusCardReadOperation,
            method: CampusCardRequestMethod,
            authentication: CampusCardSessionRequirement,
            body: CampusCardRequestBody,
        }

        let wire = WirePlan::deserialize(deserializer)?;
        let valid_body = matches!(
            (wire.operation, &wire.body),
            (
                CampusCardReadOperation::VerifySession,
                CampusCardRequestBody::SessionProbe
            ) | (
                CampusCardReadOperation::ReadAccount,
                CampusCardRequestBody::CurrentSessionAccount,
            ) | (
                CampusCardReadOperation::ReadTransactions,
                CampusCardRequestBody::CurrentSessionTransactions { .. },
            )
        );
        if !valid_body {
            return Err(de::Error::custom(
                "campus card request operation and body do not match",
            ));
        }
        Ok(Self {
            operation: wire.operation,
            method: wire.method,
            authentication: wire.authentication,
            body: wire.body,
        })
    }
}

impl CampusCardRequestPlan {
    pub fn path(&self) -> &'static str {
        self.operation.path()
    }

    /// Returns non-secret body fields that the adapter encodes at execution.
    ///
    /// The session-bound account serial is deliberately absent.  The returned
    /// values are the exact observed wire names for the bounded read fields.
    pub fn safe_body_parameters(&self) -> Vec<(String, String)> {
        match &self.body {
            CampusCardRequestBody::SessionProbe | CampusCardRequestBody::CurrentSessionAccount => {
                Vec::new()
            }
            CampusCardRequestBody::CurrentSessionTransactions { query } => vec![
                ("starttime".to_owned(), query.start_date.to_string()),
                ("endtime".to_owned(), query.end_date.to_string()),
                (
                    "tradetype".to_owned(),
                    query.transaction_type.wire_value().to_string(),
                ),
                ("pageSize".to_owned(), query.page_size.to_string()),
                ("pageNumber".to_owned(), query.page_number.to_string()),
            ],
        }
    }
}

/// Standalone campus card read profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CampusCardReadProfile;

impl CampusCardReadProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn session_probe_request(self) -> CampusCardRequestPlan {
        CampusCardRequestPlan {
            operation: CampusCardReadOperation::VerifySession,
            method: CampusCardRequestMethod::Post,
            authentication: CampusCardSessionRequirement::ExistingIdentityAndCardSso,
            body: CampusCardRequestBody::SessionProbe,
        }
    }

    pub const fn account_request(self) -> CampusCardRequestPlan {
        CampusCardRequestPlan {
            operation: CampusCardReadOperation::ReadAccount,
            method: CampusCardRequestMethod::Post,
            authentication: CampusCardSessionRequirement::ExistingIdentityAndCardSso,
            body: CampusCardRequestBody::CurrentSessionAccount,
        }
    }

    pub fn transactions_request(self, query: CampusCardTransactionQuery) -> CampusCardRequestPlan {
        CampusCardRequestPlan {
            operation: CampusCardReadOperation::ReadTransactions,
            method: CampusCardRequestMethod::Post,
            authentication: CampusCardSessionRequirement::ExistingIdentityAndCardSso,
            body: CampusCardRequestBody::CurrentSessionTransactions { query },
        }
    }
}

/// An expected account binding used only to compare a verified response.
///
/// The value is intentionally not serializable and its Debug implementation
/// is redacted.  It is useful to a session-owning caller without becoming a
/// bridge DTO or a request-plan field.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardAccountBinding(String);

impl CampusCardAccountBinding {
    pub fn new(value: &str) -> Result<Self, CampusCardRequestError> {
        let Some(value) = normalize_identifier(value) else {
            return Err(CampusCardRequestError::InvalidAccountBinding);
        };
        Ok(Self(value))
    }

    pub(crate) fn matches(&self, candidate: &str) -> bool {
        normalize_identifier(candidate).is_some_and(|candidate| candidate == self.0)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CampusCardAccountBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CampusCardAccountBinding")
            .field(&"[redacted]")
            .finish()
    }
}

/// A normalized, read-only account result.
///
/// Account serials, card IDs, phone numbers, photo names, cookies, and
/// session tokens are intentionally omitted from this cleaned record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampusCardAccount {
    pub display_name: String,
    pub display_name_latin: Option<String>,
    pub department_name: String,
    pub department_name_latin: Option<String>,
    pub department_id: i64,
    pub gender: Option<String>,
    pub effective_at: String,
    pub valid_until: String,
    pub balance_cents: i64,
    pub card_status: String,
    pub last_transaction_at: String,
    pub daily_limit_cents: i64,
    pub one_time_limit_cents: i64,
}

/// One normalized transaction record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampusCardTransaction {
    pub transaction_id: String,
    pub summary: String,
    pub occurred_at: String,
    pub post_balance_cents: i64,
    pub amount_cents: i64,
    pub merchant_address: String,
    pub merchant_name: Option<String>,
    pub transaction_name: String,
}

/// A transaction response for one explicit server page.
///
/// The service's public response does not expose a trustworthy total count.
/// An empty vector therefore means only that this validated page is empty. A
/// caller can use [`Self::may_have_next_page`] to avoid presenting a full
/// page as a complete date-range result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampusCardTransactionReport {
    pub transactions: Vec<CampusCardTransaction>,
}

impl CampusCardTransactionReport {
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// Returns true when the page is full and another page may exist.
    ///
    /// This is deliberately conservative: a full page can also be the final
    /// page, but the response envelope gives us no total count with which to
    /// prove that. The caller must request the next page to establish that.
    pub fn may_have_next_page(&self, query: &CampusCardTransactionQuery) -> bool {
        self.transactions.len() == usize::from(query.page_size)
    }
}

/// Stable field labels used by parser errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampusCardField {
    ResultData,
    AccountId,
    Username,
    DepartmentName,
    DepartmentId,
    EffectiveAt,
    ValidUntil,
    BaseAccount,
    BalanceCents,
    CardInfos,
    CardId,
    CardStatus,
    LastTransactionAt,
    DailyLimitCents,
    OneTimeLimitCents,
    Gender,
    Rows,
    TransactionId,
    Summary,
    OccurredAt,
    PostBalanceCents,
    AmountCents,
    MerchantAddress,
    MerchantName,
    TransactionName,
}

impl fmt::Display for CampusCardField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ResultData => "resultData",
            Self::AccountId => "account identity",
            Self::Username => "username",
            Self::DepartmentName => "department name",
            Self::DepartmentId => "department ID",
            Self::EffectiveAt => "effective date",
            Self::ValidUntil => "valid date",
            Self::BaseAccount => "baseAccount",
            Self::BalanceCents => "balance cents",
            Self::CardInfos => "cardInfos",
            Self::CardId => "card ID",
            Self::CardStatus => "card status",
            Self::LastTransactionAt => "last transaction date",
            Self::DailyLimitCents => "daily limit cents",
            Self::OneTimeLimitCents => "one-time limit cents",
            Self::Gender => "gender",
            Self::Rows => "transaction rows",
            Self::TransactionId => "transaction ID",
            Self::Summary => "transaction summary",
            Self::OccurredAt => "transaction date",
            Self::PostBalanceCents => "post-transaction balance cents",
            Self::AmountCents => "transaction amount cents",
            Self::MerchantAddress => "merchant address",
            Self::MerchantName => "merchant name",
            Self::TransactionName => "transaction name",
        })
    }
}

/// Errors produced by the strict envelope and record parsers.
///
/// No variant contains a response body, encrypted payload, cookie, token, or
/// invalid financial value.  A caller can branch on the error kind without
/// accidentally logging personal card data.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CampusCardParseError {
    #[error("the campus card response is not valid JSON")]
    MalformedJson,

    #[error("the campus card response is HTML or another non-JSON document")]
    HtmlResponse,

    #[error("the campus card response envelope is malformed")]
    MalformedEnvelope,

    #[error("the campus card session has expired or requires login")]
    SessionExpired,

    #[error("the campus card service reported an outer failure")]
    OuterFailure,

    #[error("the campus card response is missing resultData")]
    MissingResultData,

    #[error("{operation} resultData has an unexpected shape")]
    UnexpectedResultData { operation: CampusCardReadOperation },

    #[error("the campus card account does not match the authenticated account")]
    AccountMismatch,

    #[error("the campus card response is missing {field}")]
    MissingField { field: CampusCardField },

    #[error("the campus card field {field} has an invalid shape")]
    InvalidShape { field: CampusCardField },

    #[error("the campus card field {field} has invalid text")]
    InvalidText { field: CampusCardField },

    #[error("the campus card field {field} has invalid cents")]
    InvalidCents {
        field: CampusCardField,
        row: Option<usize>,
    },

    #[error("the campus card field {field} has an invalid number")]
    InvalidNumber {
        field: CampusCardField,
        row: Option<usize>,
    },

    #[error("the campus card cardInfos collection is empty")]
    EmptyCardInfos,

    #[error("transaction row {row} has an invalid shape")]
    InvalidTransactionRow { row: usize },

    #[error("transaction row {row} has an invalid date")]
    InvalidTransactionDate { row: usize },

    #[error("transaction row {row} is outside the requested date range")]
    TransactionOutsideRequestedRange { row: usize },
}

/// Parses account information from a successful campus card response.
///
/// When an account binding is supplied, the response idserial must match it.
/// The raw idserial is used only for this comparison and is never returned.
pub fn parse_card_account_response(
    body: &str,
    expected_account: Option<&CampusCardAccountBinding>,
) -> Result<CampusCardAccount, CampusCardParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let result_data = parse_result_data(body, CampusCardReadOperation::ReadAccount)?;
        parse_account_result(&result_data, expected_account)
    })
}

/// Parses transaction information from a successful campus card response.
///
/// An empty rows array is a valid result and produces an empty report.
pub fn parse_card_transactions_response(
    body: &str,
) -> Result<CampusCardTransactionReport, CampusCardParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let result_data = parse_result_data(body, CampusCardReadOperation::ReadTransactions)?;
        parse_transactions_result(&result_data)
    })
}

/// Card servers can serialize integer-fen amounts as JSON decimal numbers.
/// Normalize only those known monetary fields from their original raw token
/// before serde's generic Value can round through f64. No identifier or other
/// service parser is affected; fractional fen and overflow remain errors.
pub(crate) fn parse_card_json(body: &str) -> Result<Value, CampusCardParseError> {
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;
    fn visit(raw: &RawValue, depth: usize) -> Result<Value, CampusCardParseError> {
        if depth > 64 {
            return Err(CampusCardParseError::MalformedJson);
        }
        let text = raw.get().trim();
        if text.starts_with('{') {
            let fields: BTreeMap<String, Box<RawValue>> =
                serde_json::from_str(text).map_err(|_| CampusCardParseError::MalformedJson)?;
            let mut object = Map::new();
            for (key, value) in fields {
                let field = match key.as_str() {
                    "balance" => Some(CampusCardField::BalanceCents),
                    "txamt" => Some(CampusCardField::AmountCents),
                    "maxconstolamt" => Some(CampusCardField::DailyLimitCents),
                    "maxconsamt" => Some(CampusCardField::OneTimeLimitCents),
                    _ => None,
                };
                let token = value.get().trim();
                let parsed = if let Some(field) = field.filter(|_| {
                    token.starts_with('-')
                        || token.as_bytes().first().is_some_and(u8::is_ascii_digit)
                }) {
                    let amount = exact_json_integer(token)
                        .ok_or(CampusCardParseError::InvalidCents { field, row: None })?;
                    Value::Number(amount.into())
                } else {
                    visit(&value, depth + 1)?
                };
                object.insert(key, parsed);
            }
            Ok(Value::Object(object))
        } else if text.starts_with('[') {
            let values: Vec<Box<RawValue>> =
                serde_json::from_str(text).map_err(|_| CampusCardParseError::MalformedJson)?;
            values
                .iter()
                .map(|value| visit(value, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        } else {
            serde_json::from_str(text).map_err(|_| CampusCardParseError::MalformedJson)
        }
    }
    let raw: &RawValue =
        serde_json::from_str(body).map_err(|_| CampusCardParseError::MalformedJson)?;
    visit(raw, 0)
}

fn exact_json_integer(token: &str) -> Option<i64> {
    if token.is_empty() || token.len() > 128 {
        return None;
    }
    let (negative, unsigned) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => (
            &unsigned[..index],
            unsigned[index + 1..].parse::<i32>().ok()?,
        ),
        None => (unsigned, 0),
    };
    if !(-128..=128).contains(&exponent) {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut digits = format!("{whole}{fraction}");
    let scale = exponent.checked_sub(fraction.len() as i32)?;
    if scale < 0 {
        let places = usize::try_from(-scale).ok()?;
        if places > digits.len() {
            return digits.bytes().all(|b| b == b'0').then_some(0);
        }
        if !digits[digits.len() - places..].bytes().all(|b| b == b'0') {
            return None;
        }
        digits.truncate(digits.len() - places);
    }
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some(0);
    }
    let zeros = usize::try_from(scale.max(0)).ok()?;
    if significant.len().checked_add(zeros)? > 19 {
        return None;
    }
    let mut canonical = significant.to_owned();
    canonical.extend(std::iter::repeat_n('0', zeros));
    if negative {
        canonical.insert(0, '-');
    }
    canonical.parse().ok()
}

fn parse_result_data(
    body: &str,
    operation: CampusCardReadOperation,
) -> Result<Value, CampusCardParseError> {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    if trimmed.is_empty() {
        return Err(CampusCardParseError::MalformedJson);
    }
    if looks_like_login_html_response(trimmed) {
        return Err(CampusCardParseError::SessionExpired);
    }
    if looks_like_html_response(trimmed) {
        return Err(CampusCardParseError::HtmlResponse);
    }

    let root: Value = match parse_card_json(trimmed) {
        Ok(root) => root,
        Err(CampusCardParseError::MalformedJson) if contains_session_failure_marker(trimmed) => {
            return Err(CampusCardParseError::SessionExpired);
        }
        Err(error) => return Err(error),
    };
    let Some(envelope) = root.as_object() else {
        return Err(CampusCardParseError::MalformedEnvelope);
    };
    let Some(success) = envelope.get("success").and_then(Value::as_bool) else {
        if envelope_has_session_failure_marker(envelope) {
            return Err(CampusCardParseError::SessionExpired);
        }
        return Err(CampusCardParseError::MalformedEnvelope);
    };
    if !success {
        if envelope_has_session_failure_marker(envelope) {
            return Err(CampusCardParseError::SessionExpired);
        }
        return Err(CampusCardParseError::OuterFailure);
    }

    let Some(result_data) = envelope.get("resultData") else {
        return Err(CampusCardParseError::MissingResultData);
    };
    if result_data.is_null() {
        return Err(CampusCardParseError::MissingResultData);
    }
    if !matches!(
        operation,
        CampusCardReadOperation::ReadAccount | CampusCardReadOperation::ReadTransactions
    ) {
        return Err(CampusCardParseError::UnexpectedResultData { operation });
    }
    Ok(result_data.clone())
}

pub(crate) fn looks_like_html_response(body: &str) -> bool {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let lower = body.trim_start().to_ascii_lowercase();
    // The card endpoints are JSON-only. Keep this as a protocol-shape check;
    // the adapter must inspect the final route before deciding whether an
    // HTML response is an authentication redirect, a maintenance page, or an
    // unrelated same-origin response.
    lower.starts_with('<')
}

fn envelope_has_session_failure_marker(envelope: &Map<String, Value>) -> bool {
    ["message", "msg", "error", "status", "data"]
        .iter()
        .filter_map(|key| envelope.get(*key))
        .filter_map(Value::as_str)
        .any(contains_session_failure_marker)
}

fn contains_session_failure_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "login required",
        "not logged in",
        "please login",
        "please log in",
        "authentication required",
        "not authenticated",
        "session expired",
        "login timeout",
        "session timeout",
        "用户未登录",
        "未登录",
        "登录超时",
        "登陆超时",
        "会话过期",
        "会话已过期",
        "会话已失效",
        "登录失效",
        "登陆失效",
        "请先登录",
        "请先登陆",
        "重新登录",
        "重新登陆",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

/// Returns true for an HTML document that is recognizably an authentication
/// page. A generic HTML response remains [`CampusCardParseError::HtmlResponse`]
/// because a maintenance page is not evidence that the card Cookie expired.
/// This parser-level distinction is useful to callers that do not have the
/// adapter's final-route metadata.
pub(crate) fn looks_like_login_html_response(body: &str) -> bool {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let lower = body.trim_start().to_ascii_lowercase();
    if !looks_like_html_response(&lower) {
        return false;
    }

    let identity_fields = (lower.contains("name=\"i_user\"")
        || lower.contains("name='i_user'")
        || lower.contains("id=\"i_user\"")
        || lower.contains("id='i_user'"))
        && (lower.contains("name=\"i_pass\"")
            || lower.contains("name='i_pass'")
            || lower.contains("id=\"i_pass\"")
            || lower.contains("id='i_pass'"));
    if identity_fields {
        return true;
    }

    let login_marker = [
        "login",
        "log in",
        "登录",
        "登陆",
        "统一身份认证",
        "authentication required",
        "session expired",
        "session timeout",
        "请先登录",
        "请先登陆",
        "登录失效",
        "登陆失效",
        "会话过期",
        "会话已过期",
        "会话已失效",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !login_marker {
        return false;
    }

    let has_form_or_input = lower.contains("<form") || lower.contains("<input");
    let title_mentions_login = lower.contains("<title")
        && [
            "login",
            "log in",
            "登录",
            "登陆",
            "统一身份认证",
            "authentication",
        ]
        .iter()
        .any(|marker| lower.contains(marker));
    has_form_or_input || title_mentions_login || lower.contains("webvpn")
}

fn parse_account_result(
    result_data: &Value,
    expected_account: Option<&CampusCardAccountBinding>,
) -> Result<CampusCardAccount, CampusCardParseError> {
    let Some(account) = result_data.as_object() else {
        return Err(CampusCardParseError::UnexpectedResultData {
            operation: CampusCardReadOperation::ReadAccount,
        });
    };

    let account_id = required_identifier(account, "idserial", CampusCardField::AccountId)?;
    if expected_account.is_some_and(|expected| !expected.matches(&account_id)) {
        return Err(CampusCardParseError::AccountMismatch);
    }

    let display_name = required_text(account, "username", CampusCardField::Username)?;
    let display_name_latin = optional_text(account, "engname", CampusCardField::Username)?;
    let department_name = required_text(account, "departname", CampusCardField::DepartmentName)?;
    let department_name_latin =
        optional_text(account, "engdepartname", CampusCardField::DepartmentName)?;
    let department_id = required_integer(account, "departid", CampusCardField::DepartmentId, None)?;
    let gender = optional_text(account, "sex", CampusCardField::Gender)?;
    let effective_at = required_account_date(
        account,
        "identifyeffectdate",
        CampusCardField::EffectiveAt,
        false,
    )?;
    let valid_until =
        required_account_date(account, "validatevalue", CampusCardField::ValidUntil, false)?;

    let base_account = required_object(account, "baseAccount", CampusCardField::BaseAccount)?;
    let balance_cents =
        required_cents(base_account, "balance", CampusCardField::BalanceCents, None)?;

    let cards = required_array(account, "cardInfos", CampusCardField::CardInfos)?;
    let Some(card) = cards.first().and_then(Value::as_object) else {
        return Err(CampusCardParseError::EmptyCardInfos);
    };
    let _card_id = required_identifier(card, "cardid", CampusCardField::CardId)?;
    let card_status = required_identifier(card, "accstatus", CampusCardField::CardStatus)?;
    let last_transaction_at =
        required_account_date(card, "lasttxdate", CampusCardField::LastTransactionAt, true)?;
    let daily_limit_cents = required_cents(
        card,
        "maxconstolamt",
        CampusCardField::DailyLimitCents,
        None,
    )?;
    let one_time_limit_cents =
        required_cents(card, "maxconsamt", CampusCardField::OneTimeLimitCents, None)?;

    Ok(CampusCardAccount {
        display_name,
        display_name_latin,
        department_name,
        department_name_latin,
        department_id,
        gender,
        effective_at,
        valid_until,
        balance_cents,
        card_status,
        last_transaction_at,
        daily_limit_cents,
        one_time_limit_cents,
    })
}

/// The reference invokes new Date(value) on these fields. Numeric wire
/// values are milliseconds since the epoch, not seconds or a display string.
/// Null/booleans/fractional numbers are not valid evidence for a real date.
fn required_account_date(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
    allow_empty: bool,
) -> Result<String, CampusCardParseError> {
    match object.get(key) {
        Some(Value::Number(value)) => {
            let millis = value
                .as_i64()
                .filter(|ms| ms.unsigned_abs() <= 8_640_000_000_000_000)
                .ok_or(CampusCardParseError::InvalidText { field })?;
            let time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
                .ok_or(CampusCardParseError::InvalidText { field })?;
            Ok(time
                .with_timezone(&FixedOffset::east_opt(8 * 3600).expect("campus timezone"))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, false))
        }
        _ if allow_empty => required_display_text_allow_empty(object, key, field),
        _ => required_text(object, key, field),
    }
}

fn parse_transactions_result(
    result_data: &Value,
) -> Result<CampusCardTransactionReport, CampusCardParseError> {
    let Some(result) = result_data.as_object() else {
        return Err(CampusCardParseError::UnexpectedResultData {
            operation: CampusCardReadOperation::ReadTransactions,
        });
    };
    let rows = required_array(result, "rows", CampusCardField::Rows)?;
    let mut transactions = Vec::with_capacity(rows.len());

    for (row_index, value) in rows.iter().enumerate() {
        let Some(row) = value.as_object() else {
            return Err(CampusCardParseError::InvalidTransactionRow { row: row_index });
        };
        let transaction_id = required_identifier(row, "id", CampusCardField::TransactionId)?;
        let summary = required_text(row, "summary", CampusCardField::Summary)?;
        let occurred_at = required_text(row, "txdate", CampusCardField::OccurredAt)?;
        // Validate the original timestamp before display normalization can
        // collapse whitespace or discard a control character.
        let raw_date = row
            .get("txdate")
            .and_then(Value::as_str)
            .ok_or(CampusCardParseError::InvalidTransactionDate { row: row_index })?;
        if transaction_local_date(raw_date).is_none() {
            return Err(CampusCardParseError::InvalidTransactionDate { row: row_index });
        }
        let post_balance_cents = required_cents(
            row,
            "balance",
            CampusCardField::PostBalanceCents,
            Some(row_index),
        )?;
        // `txamt` is a ledger delta.  Some deployments represent a debit as
        // a negative number, while account balances and card limits remain
        // non-negative.  Do not apply the account-field invariant to this
        // signed transaction field or ordinary consumption rows disappear.
        let amount_cents =
            required_signed_cents(row, "txamt", CampusCardField::AmountCents, Some(row_index))?;
        let merchant_address =
            required_display_text_allow_empty(row, "meraddr", CampusCardField::MerchantAddress)?;
        let merchant_name = optional_text(row, "mername", CampusCardField::MerchantName)?;
        let transaction_name = required_text(row, "txname", CampusCardField::TransactionName)?;

        transactions.push(CampusCardTransaction {
            transaction_id,
            summary,
            occurred_at,
            post_balance_cents,
            amount_cents,
            merchant_address,
            merchant_name,
            transaction_name,
        });
    }

    Ok(CampusCardTransactionReport { transactions })
}

/// Resolve the transaction's campus-local date without modifying the DTO's
/// original display value. Naive ledger timestamps are campus wall time;
/// explicit RFC3339 offsets are converted before checking date selectors.
pub(crate) fn transaction_local_date(value: &str) -> Option<NaiveDate> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = value.trim();
    if let Ok(instant) = DateTime::parse_from_rfc3339(value) {
        let campus = FixedOffset::east_opt(8 * 60 * 60)?;
        return Some(instant.with_timezone(&campus).date_naive());
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(time) = NaiveDateTime::parse_from_str(value, format) {
            return Some(time.date());
        }
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    (date.to_string() == value).then_some(date)
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    field: CampusCardField,
) -> Result<&'a Map<String, Value>, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    value
        .as_object()
        .ok_or(CampusCardParseError::InvalidShape { field })
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    field: CampusCardField,
) -> Result<&'a Vec<Value>, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    value
        .as_array()
        .ok_or(CampusCardParseError::InvalidShape { field })
}

fn required_text(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
) -> Result<String, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    let Some(value) = value.as_str() else {
        return Err(CampusCardParseError::InvalidText { field });
    };
    normalize_text(value).ok_or(CampusCardParseError::InvalidText { field })
}

/// Some card display-only strings are legitimately empty in the public
/// reference client (for example a card with no previous transaction or a
/// non-merchant ledger row). Preserve that absence as an empty string while
/// retaining the same control-character rejection and whitespace cleanup as
/// normal text fields. This helper is intentionally not used for identifiers,
/// names, monetary fields, or timestamps that prove a business record.
fn required_display_text_allow_empty(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
) -> Result<String, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    let Some(value) = value.as_str() else {
        return Err(CampusCardParseError::InvalidText { field });
    };
    if value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace())
    {
        return Err(CampusCardParseError::InvalidText { field });
    }
    Ok(normalize_text(value).unwrap_or_default())
}

fn optional_text(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
) -> Result<Option<String>, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_str() else {
        return Err(CampusCardParseError::InvalidText { field });
    };
    if value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace())
    {
        return Err(CampusCardParseError::InvalidText { field });
    }
    Ok(normalize_text(value))
}

fn required_identifier(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
) -> Result<String, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    let candidate = match value {
        Value::String(value) => value.clone(),
        Value::Number(value) if value.is_i64() || value.is_u64() => value.to_string(),
        _ => return Err(CampusCardParseError::InvalidText { field }),
    };
    normalize_identifier(&candidate).ok_or(CampusCardParseError::InvalidText { field })
}

fn required_integer(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
    row: Option<usize>,
) -> Result<i64, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    parse_integer(value, field, row)
}

fn required_cents(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
    row: Option<usize>,
) -> Result<i64, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    let number = parse_integer(value, field, row).map_err(|error| match error {
        CampusCardParseError::InvalidNumber { .. } => {
            CampusCardParseError::InvalidCents { field, row }
        }
        other => other,
    })?;
    if number < 0 {
        return Err(CampusCardParseError::InvalidCents { field, row });
    }
    Ok(number)
}

fn required_signed_cents(
    object: &Map<String, Value>,
    key: &str,
    field: CampusCardField,
    row: Option<usize>,
) -> Result<i64, CampusCardParseError> {
    let Some(value) = object.get(key) else {
        return Err(CampusCardParseError::MissingField { field });
    };
    parse_integer(value, field, row).map_err(|error| match error {
        CampusCardParseError::InvalidNumber { .. } => {
            CampusCardParseError::InvalidCents { field, row }
        }
        other => other,
    })
}

fn parse_integer(
    value: &Value,
    field: CampusCardField,
    row: Option<usize>,
) -> Result<i64, CampusCardParseError> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .ok_or(CampusCardParseError::InvalidNumber { field, row }),
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty()
                || value.starts_with('+')
                || value
                    .chars()
                    .skip(1)
                    .any(|character| !character.is_ascii_digit())
                || (value.starts_with('-') && value.len() == 1)
                || (!value.starts_with('-')
                    && value.chars().any(|character| !character.is_ascii_digit()))
            {
                return Err(CampusCardParseError::InvalidNumber { field, row });
            }
            value
                .parse::<i64>()
                .map_err(|_| CampusCardParseError::InvalidNumber { field, row })
        }
        _ => Err(CampusCardParseError::InvalidNumber { field, row }),
    }
}

fn normalize_identifier(value: &str) -> Option<String> {
    if value.is_empty()
        || value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}')
        })
    {
        return None;
    }
    Some(value.to_owned())
}

fn normalize_text(value: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if matches!(character, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}') {
            continue;
        }
        if character.is_control() && !character.is_whitespace() {
            return None;
        }
        if character.is_whitespace() || matches!(character, '\u{00a0}' | '\u{2007}' | '\u{202f}') {
            if !normalized.is_empty() {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        normalized.push(character);
    }
    let normalized = normalized.trim().to_owned();
    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT_FIXTURE: &str = r#"
    {
      "success": true,
      "data": null,
      "resultData": {
        "idserial": "student-001",
        "username": "  张\u00a0三 ",
        "engname": " Zhang San ",
        "departname": "  计算机\u3000系 ",
        "engdepartname": "Computer Science",
        "departid": "42",
        "sex": "1",
        "identifyeffectdate": "2026-01-01 00:00:00",
        "validatevalue": "2030-06-30 23:59:59",
        "tel": "13800000000",
        "photofile": "private-photo.jpg",
        "baseAccount": { "balance": "12345" },
        "cardInfos": [{
          "cardid": "card-secret-001",
          "accstatus":  "0",
          "lasttxdate": "2026-09-01 12:30:00",
          "maxconstolamt": 20000,
          "maxconsamt": "5000"
        }]
      }
    }
    "#;

    const TRANSACTION_FIXTURE: &str = r#"
    {
      "success": true,
      "data": null,
      "resultData": {
        "rows": [{
          "id": 17,
          "summary": "  午餐\u00a0消费 ",
          "txdate": "2026-09-01 12:30:00",
          "balance": "12000",
          "txamt": 345,
          "meraddr": "  紫荆园\n一层 ",
          "mername": "  餐厅 ",
          "txname": "消费"
        }]
      }
    }
    "#;

    #[test]
    fn builds_safe_read_plans_without_account_serials_or_origins() {
        let profile = CampusCardReadProfile::standard();
        let session = profile.session_probe_request();
        assert_eq!(session.path(), CARD_SESSION_USER_PATH);
        assert_eq!(
            session.authentication,
            CampusCardSessionRequirement::ExistingIdentityAndCardSso
        );
        assert_eq!(
            session.safe_body_parameters(),
            Vec::<(String, String)>::new()
        );

        let query = CampusCardTransactionQuery::new(
            "2026-09-01",
            "2026-09-30",
            CampusCardTransactionType::Consumption,
            100,
            0,
        )
        .expect("query is valid");
        let plan = profile.transactions_request(query);
        assert_eq!(plan.path(), CARD_TRANSACTIONS_PATH);
        assert_eq!(
            plan.safe_body_parameters(),
            vec![
                ("starttime".to_owned(), "2026-09-01".to_owned()),
                ("endtime".to_owned(), "2026-09-30".to_owned()),
                ("tradetype".to_owned(), "1".to_owned()),
                ("pageSize".to_owned(), "100".to_owned()),
                ("pageNumber".to_owned(), "0".to_owned()),
            ]
        );
        let debug = format!("{plan:?}");
        assert!(!debug.contains("idserial"));
        assert!(!debug.contains("http"));
    }

    #[test]
    fn rejects_unbounded_or_invalid_transaction_queries() {
        assert_eq!(
            CampusCardTransactionQuery::new(
                "2026-09-02",
                "2026-09-01",
                CampusCardTransactionType::Any,
                1,
                0
            ),
            Err(CampusCardRequestError::ReversedDateRange)
        );
        assert!(matches!(
            CampusCardTransactionQuery::new(
                "2026-09-01",
                "2026-11-01",
                CampusCardTransactionType::Any,
                1,
                0
            ),
            Err(CampusCardRequestError::DateRangeTooLarge { .. })
        ));
        assert!(matches!(
            CampusCardTransactionQuery::new(
                "2026-09-01",
                "2026-09-01",
                CampusCardTransactionType::Any,
                MAX_TRANSACTION_PAGE_SIZE + 1,
                0
            ),
            Err(CampusCardRequestError::InvalidPageSize { max: 500 })
        ));
        assert!(matches!(
            CampusCardTransactionQuery::new(
                "2026-09-01",
                "2026-09-01",
                CampusCardTransactionType::Any,
                501,
                0
            ),
            Err(CampusCardRequestError::InvalidPageSize { max: 500 })
        ));
        assert!(
            CampusCardTransactionQuery::new(
                "2026-09-01",
                "2026-10-01",
                CampusCardTransactionType::Any,
                MAX_TRANSACTION_PAGE_SIZE,
                0
            )
            .is_ok()
        );
        assert!(matches!(
            CampusCardTransactionQuery::new(
                "2026-09-01",
                "2026-10-02",
                CampusCardTransactionType::Any,
                1,
                0
            ),
            Err(CampusCardRequestError::DateRangeTooLarge { max_days: 31 })
        ));
        assert!(matches!(
            CampusCardTransactionQuery::new(
                "2026-09-01",
                "2026-09-01",
                CampusCardTransactionType::Any,
                1,
                MAX_TRANSACTION_PAGE_NUMBER + 1
            ),
            Err(CampusCardRequestError::InvalidPageNumber { .. })
        ));
    }

    #[test]
    fn parses_and_normalizes_account_without_sensitive_identifiers() {
        let expected = CampusCardAccountBinding::new("student-001").expect("binding is valid");
        let account =
            parse_card_account_response(ACCOUNT_FIXTURE, Some(&expected)).expect("account parses");
        assert_eq!(account.display_name, "张 三");
        assert_eq!(account.department_name, "计算机 系");
        assert_eq!(account.department_id, 42);
        assert_eq!(account.balance_cents, 12345);
        assert_eq!(account.daily_limit_cents, 20000);
        assert_eq!(account.one_time_limit_cents, 5000);
        let encoded = serde_json::to_string(&account).expect("account serializes");
        assert!(!encoded.contains("student-001"));
        assert!(!encoded.contains("card-secret-001"));
        assert!(!encoded.contains("13800000000"));
        assert!(!format!("{account:?}").contains("card-secret-001"));
    }

    #[test]
    fn distinguishes_account_mismatch_and_invalid_cents() {
        let expected = CampusCardAccountBinding::new("different-account").expect("binding");
        assert_eq!(
            parse_card_account_response(ACCOUNT_FIXTURE, Some(&expected)),
            Err(CampusCardParseError::AccountMismatch)
        );

        let invalid = ACCOUNT_FIXTURE.replace("\"balance\": \"12345\"", "\"balance\": \"12.3\"");
        assert_eq!(
            parse_card_account_response(&invalid, None),
            Err(CampusCardParseError::InvalidCents {
                field: CampusCardField::BalanceCents,
                row: None,
            })
        );
    }

    #[test]
    fn rejects_empty_cards_and_negative_account_balances_without_defaults() {
        let mut empty_cards: Value = serde_json::from_str(ACCOUNT_FIXTURE).expect("account JSON");
        empty_cards["resultData"]["cardInfos"] = Value::Array(Vec::new());
        assert_eq!(
            parse_card_account_response(&empty_cards.to_string(), None),
            Err(CampusCardParseError::EmptyCardInfos)
        );

        for (field, expected) in [
            ("balance", CampusCardField::BalanceCents),
            ("maxconstolamt", CampusCardField::DailyLimitCents),
            ("maxconsamt", CampusCardField::OneTimeLimitCents),
        ] {
            let mut negative: Value = serde_json::from_str(ACCOUNT_FIXTURE).expect("account JSON");
            if field == "balance" {
                negative["resultData"]["baseAccount"][field] = Value::from(-1);
            } else {
                negative["resultData"]["cardInfos"][0][field] = Value::from(-1);
            }
            assert_eq!(
                parse_card_account_response(&negative.to_string(), None),
                Err(CampusCardParseError::InvalidCents {
                    field: expected,
                    row: None,
                })
            );
        }
    }

    #[test]
    fn parses_transactions_and_accepts_a_valid_empty_result() {
        let report =
            parse_card_transactions_response(TRANSACTION_FIXTURE).expect("transactions parse");
        assert_eq!(report.len(), 1);
        assert_eq!(report.transactions[0].transaction_id, "17");
        assert_eq!(report.transactions[0].summary, "午餐 消费");
        assert_eq!(report.transactions[0].merchant_address, "紫荆园 一层");
        assert_eq!(report.transactions[0].amount_cents, 345);
        assert_eq!(report.transactions[0].post_balance_cents, 12000);

        let empty = r#"{"success":true,"resultData":{"rows":[]},"data":null}"#;
        let report = parse_card_transactions_response(empty).expect("empty rows are valid");
        assert!(report.is_empty());

        let bom_report =
            parse_card_transactions_response(&format!("\u{feff}{TRANSACTION_FIXTURE}"))
                .expect("BOM-prefixed JSON is valid");
        assert_eq!(bom_report.len(), 1);

        let one_row_query = CampusCardTransactionQuery::new(
            "2026-09-01",
            "2026-09-01",
            CampusCardTransactionType::Any,
            1,
            0,
        )
        .expect("one-row query");
        assert!(bom_report.may_have_next_page(&one_row_query));
        let empty_query = CampusCardTransactionQuery::new(
            "2026-09-01",
            "2026-09-01",
            CampusCardTransactionType::Any,
            1,
            1,
        )
        .expect("second-page query");
        assert!(!report.may_have_next_page(&empty_query));
        assert!(one_row_query.is_first_page());
        assert!(!empty_query.is_first_page());
    }

    #[test]
    fn backend_repair_card_allows_reference_empty_optional_display_strings() {
        let mut account: Value = serde_json::from_str(ACCOUNT_FIXTURE).expect("account JSON");
        account["resultData"]["cardInfos"][0]["lasttxdate"] = Value::String(String::new());
        let parsed = parse_card_account_response(&account.to_string(), None)
            .expect("a card without a previous transaction still has valid account data");
        assert_eq!(parsed.last_transaction_at, "");

        let mut transactions: Value =
            serde_json::from_str(TRANSACTION_FIXTURE).expect("transaction JSON");
        transactions["resultData"]["rows"][0]["meraddr"] = Value::String(String::new());
        let parsed = parse_card_transactions_response(&transactions.to_string())
            .expect("non-merchant card operations may have no merchant address");
        assert_eq!(parsed.transactions[0].merchant_address, "");
    }

    #[test]
    fn distinguishes_outer_failure_malformed_json_and_session_expiry() {
        let secret = r#"{"success":false,"data":"encrypted-payload-that-must-not-leak"}"#;
        let error = parse_card_transactions_response(secret).expect_err("outer failure");
        assert_eq!(error, CampusCardParseError::OuterFailure);
        assert!(!format!("{error:?}").contains("encrypted-payload"));
        assert!(!format!("{error}").contains("encrypted-payload"));

        assert_eq!(
            parse_card_transactions_response("{not-json"),
            Err(CampusCardParseError::MalformedJson)
        );
        assert_eq!(
            parse_card_transactions_response(
                "<html><title>WebVPN login</title><form>Password</form></html>"
            ),
            Err(CampusCardParseError::SessionExpired)
        );
        assert_eq!(
            parse_card_transactions_response(
                "\u{feff}<html><title>WebVPN login</title><form>Password</form></html>"
            ),
            Err(CampusCardParseError::SessionExpired)
        );
        assert_eq!(
            parse_card_transactions_response(
                "<html><form><input name='i_user'><input name='i_pass'></form></html>"
            ),
            Err(CampusCardParseError::SessionExpired)
        );
        assert_eq!(
            parse_card_transactions_response(
                "<html><title>maintenance</title><p>temporary outage</p></html>"
            ),
            Err(CampusCardParseError::HtmlResponse)
        );
        assert_eq!(
            parse_card_transactions_response(
                r#"{"success":false,"data":"please login before continuing"}"#
            ),
            Err(CampusCardParseError::SessionExpired)
        );
        assert_eq!(
            parse_card_transactions_response("please login before continuing"),
            Err(CampusCardParseError::SessionExpired)
        );
        for marker in [
            "authentication required",
            "请先登陆",
            "登录失效",
            "会话已失效",
        ] {
            let body = format!(r#"{{"success":false,"message":"{marker}"}}"#);
            assert_eq!(
                parse_card_transactions_response(&body),
                Err(CampusCardParseError::SessionExpired),
                "marker should classify as session expiry: {marker}"
            );
        }

        let marker_in_valid_record = TRANSACTION_FIXTURE.replace(
            "\"summary\": \"  午餐\\u00a0消费 \"",
            "\"summary\": \"login required is a merchant note\"",
        );
        assert!(parse_card_transactions_response(&marker_in_valid_record).is_ok());
    }

    #[test]
    fn rejects_malformed_rows_and_non_integer_transaction_cents() {
        let invalid_amount = TRANSACTION_FIXTURE.replace("\"txamt\": 345", "\"txamt\": \"3.45\"");
        assert_eq!(
            parse_card_transactions_response(&invalid_amount),
            Err(CampusCardParseError::InvalidCents {
                field: CampusCardField::AmountCents,
                row: Some(0),
            })
        );

        let debit = TRANSACTION_FIXTURE.replace("\"txamt\": 345", "\"txamt\": -345");
        let parsed = parse_card_transactions_response(&debit).expect("signed debit amount");
        assert_eq!(parsed.transactions[0].amount_cents, -345);

        let invalid_row = r#"{"success":true,"resultData":{"rows":[null]},"data":null}"#;
        assert_eq!(
            parse_card_transactions_response(invalid_row),
            Err(CampusCardParseError::InvalidTransactionRow { row: 0 })
        );
    }

    #[test]
    fn redacts_account_binding_debug_output() {
        let binding =
            CampusCardAccountBinding::new("private-account-serial").expect("binding is valid");
        assert!(!format!("{binding:?}").contains("private-account-serial"));
        assert!(CampusCardAccountBinding::new("  ").is_err());
        assert!(CampusCardAccountBinding::new(" private-account-serial").is_err());
        assert!(CampusCardAccountBinding::new("private-account-serial ").is_err());
        assert!(CampusCardAccountBinding::new("private account").is_err());
    }
}

#[cfg(test)]
#[path = "card_lastmile_tests.rs"]
mod lastmile_tests;
