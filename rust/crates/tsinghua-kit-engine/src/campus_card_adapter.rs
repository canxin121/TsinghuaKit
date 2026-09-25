//! Cookie-aware, read-only campus-card adapter.
//!
//! `campus_card_read` owns the safe request plans and strict record parser;
//! this adapter owns the card service origin, the shared Cookie transport,
//! the account-bound `idserial`, the card SSO handoff profile, and the
//! service's encrypted failure envelope. The runtime performs the identity
//! handoff on the same `CampusHttpTransport`, then calls `probe_session` (or
//! `probe_session_for`) before any business read. A successful HTTP status is
//! never treated as authentication proof by itself.

use std::{fmt, sync::Arc};

use reqwest::{StatusCode, Url};
use serde_json::{Value, json};
use thiserror::Error;

use crate::campus_card_read::{
    CARD_ACCOUNT_PATH, CARD_SESSION_REDIRECT_PATH, CARD_SESSION_USER_PATH, CARD_TRANSACTIONS_PATH,
    CampusCardAccount, CampusCardAccountBinding, CampusCardParseError, CampusCardReadOperation,
    CampusCardTransactionQuery, CampusCardTransactionReport, looks_like_html_response,
    looks_like_login_html_response, parse_card_account_response, parse_card_transactions_response,
};
use crate::transport::CampusHttpTransport;

/// The identity policy and target used by the current public card client.
///
/// This is a handoff description, not a ticket.  The identity runtime must
/// perform the handoff on its existing authenticated transport; this adapter
/// never accepts or returns the resulting ticket or cookie.
pub const CAMPUS_CARD_SSO_POLICY: &str = "card";
pub const CAMPUS_CARD_SSO_TARGET: &str = "eea30cbedcaf97c69d28b2d92f22a259/0?/userindex";

const DEFAULT_USER_AGENT: &str = "TsinghuaKit/0.2.0-alpha.1 campus-card-read";
const SESSION_PATH: &str = CARD_SESSION_USER_PATH;
const ACCOUNT_PATH: &str = CARD_ACCOUNT_PATH;
const TRANSACTIONS_PATH: &str = CARD_TRANSACTIONS_PATH;

/// A safe description that lets the runtime perform the card SSO handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CampusCardSsoProfile {
    pub identity_policy: &'static str,
    pub identity_target: &'static str,
}

impl CampusCardSsoProfile {
    pub const fn standard() -> Self {
        Self {
            identity_policy: CAMPUS_CARD_SSO_POLICY,
            identity_target: CAMPUS_CARD_SSO_TARGET,
        }
    }
}

/// Configuration for the direct card-service origin.
///
/// `base_url` is kept in Rust and is never part of a bridge DTO.  A deployment
/// can use a direct card origin or a WebVPN mapped origin as long as it stays
/// on one origin and the caller has already established the card SSO cookie.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardAdapterConfig {
    base_url: Url,
    user_agent: String,
    sso: CampusCardSsoProfile,
}

impl fmt::Debug for CampusCardAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardAdapterConfig")
            .field("base_url", &self.base_url.origin().ascii_serialization())
            .field("user_agent", &self.user_agent)
            .field("sso", &self.sso)
            .finish()
    }
}

impl CampusCardAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, CampusCardAdapterError> {
        Self::with_user_agent(base_url, DEFAULT_USER_AGENT)
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, CampusCardAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| CampusCardAdapterError::InvalidConfig)?;
        if base_url.host_str().is_none()
            || !matches!(base_url.scheme(), "https" | "http")
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !safe_base_path(base_url.path())
        {
            return Err(CampusCardAdapterError::InvalidConfig);
        }

        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(CampusCardAdapterError::InvalidConfig);
        }

        Ok(Self {
            base_url,
            user_agent,
            sso: CampusCardSsoProfile::standard(),
        })
    }

    pub fn with_sso_profile(mut self, sso: CampusCardSsoProfile) -> Self {
        self.sso = sso;
        self
    }

    pub fn sso_profile(&self) -> CampusCardSsoProfile {
        self.sso
    }

    pub fn base_origin(&self) -> String {
        self.base_url.origin().ascii_serialization()
    }
}

/// Stable, body-free adapter failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CampusCardAdapterError {
    #[error("campus card adapter configuration is invalid")]
    InvalidConfig,

    #[error("campus card request could not be sent")]
    Transport,

    #[error("campus card service returned HTTP status {status}")]
    HttpStatus { status: u16 },

    #[error("campus card response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("campus card response ended outside the configured service path")]
    UnexpectedPath,

    #[error("campus card session has expired or is not established")]
    SessionExpired,

    #[error("campus card session probe response is invalid")]
    InvalidSessionResponse,

    #[error("campus card response does not match the expected JSON protocol")]
    UnexpectedResponse,

    #[error("campus card response used an unexpected redirect")]
    UnexpectedRedirect,

    #[error("campus card encrypted failure response is invalid")]
    InvalidEncryptedPayload,

    #[error("campus card service returned an encrypted failure")]
    EncryptedServiceFailure,

    #[error("campus card protocol response could not be parsed: {0}")]
    Parse(#[from] CampusCardParseError),

    #[error("campus card session account does not match the authenticated account")]
    AccountMismatch,
}

impl CampusCardAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        use CampusCardParseError as Parse;
        match self {
            Self::InvalidConfig => "card_read_config",
            Self::Transport => "card_read_transport",
            Self::HttpStatus { .. } => "card_read_http",
            Self::UnexpectedOrigin => "card_read_origin",
            Self::UnexpectedPath => "card_read_path",
            Self::SessionExpired => "card_read_session_expired",
            Self::InvalidSessionResponse => "card_read_probe_format",
            Self::UnexpectedResponse => "card_read_non_json",
            Self::UnexpectedRedirect => "card_read_redirect",
            Self::InvalidEncryptedPayload => "card_read_encrypted_format",
            Self::EncryptedServiceFailure => "card_read_encrypted_rejection",
            Self::AccountMismatch | Self::Parse(Parse::AccountMismatch) => {
                "card_read_account_mismatch"
            }
            Self::Parse(error) => match error {
                Parse::MalformedJson => "card_read_json",
                Parse::HtmlResponse => "card_read_non_json",
                Parse::MalformedEnvelope => "card_read_envelope",
                Parse::SessionExpired => "card_read_session_expired",
                Parse::OuterFailure => "card_read_rejected",
                Parse::MissingResultData => "card_read_result_missing",
                Parse::UnexpectedResultData { .. }
                | Parse::InvalidShape { .. }
                | Parse::InvalidTransactionRow { .. } => "card_read_shape",
                Parse::MissingField { .. } => "card_read_field_missing",
                Parse::InvalidText { .. } => "card_read_text",
                Parse::InvalidCents { .. } => "card_read_cents",
                Parse::InvalidNumber { .. } => "card_read_number",
                Parse::EmptyCardInfos => "card_read_no_card",
                Parse::InvalidTransactionDate { .. } => "card_read_date",
                Parse::TransactionOutsideRequestedRange { .. } => "card_read_date_range",
                Parse::AccountMismatch => "card_read_account_mismatch",
            },
        }
    }

    pub(crate) fn trace_read_failure(&self, operation: &'static str) {
        let field = match self {
            Self::Parse(
                CampusCardParseError::MissingField { field }
                | CampusCardParseError::InvalidShape { field }
                | CampusCardParseError::InvalidText { field }
                | CampusCardParseError::InvalidCents { field, .. }
                | CampusCardParseError::InvalidNumber { field, .. },
            ) => Some(*field),
            _ => None,
        };
        let data_field = match field {
            Some(crate::campus_card_read::CampusCardField::AccountId) => "account_id",
            Some(crate::campus_card_read::CampusCardField::Username) => "display_name",
            Some(crate::campus_card_read::CampusCardField::DepartmentName) => "department_name",
            Some(crate::campus_card_read::CampusCardField::DepartmentId) => "department_id",
            Some(crate::campus_card_read::CampusCardField::BalanceCents) => "balance_cents",
            Some(crate::campus_card_read::CampusCardField::AmountCents) => "amount_cents",
            Some(crate::campus_card_read::CampusCardField::PostBalanceCents) => {
                "post_balance_cents"
            }
            Some(crate::campus_card_read::CampusCardField::DailyLimitCents) => "daily_limit_cents",
            Some(crate::campus_card_read::CampusCardField::OneTimeLimitCents) => {
                "single_limit_cents"
            }
            Some(crate::campus_card_read::CampusCardField::CardStatus) => "card_status",
            Some(crate::campus_card_read::CampusCardField::CardId) => "card_id",
            Some(crate::campus_card_read::CampusCardField::Gender) => "gender",
            Some(crate::campus_card_read::CampusCardField::EffectiveAt) => "effective_date",
            Some(crate::campus_card_read::CampusCardField::ValidUntil) => "valid_until",
            Some(crate::campus_card_read::CampusCardField::LastTransactionAt) => {
                "last_transaction_date"
            }
            Some(crate::campus_card_read::CampusCardField::Summary) => "transaction_summary",
            Some(crate::campus_card_read::CampusCardField::OccurredAt) => "transaction_date",
            Some(crate::campus_card_read::CampusCardField::MerchantAddress) => "merchant_address",
            Some(crate::campus_card_read::CampusCardField::MerchantName) => "merchant_name",
            Some(crate::campus_card_read::CampusCardField::TransactionName) => "transaction_name",
            Some(_) => "other_field",
            None => "not_applicable",
        };
        tracing::warn!(target:"tsinghua_kit::api",event="card_read_failure",service="campus_card",operation,reason=self.diagnostic_code(),data_field);
    }
}

/// A live card-service capability.  Its account serial is private and its
/// cookie jar is held by the caller-provided transport.
pub struct CampusCardSession {
    account_serial: String,
    owner: Arc<()>,
}

impl fmt::Debug for CampusCardSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardSession")
            .field("account_serial", &"[redacted]")
            .finish()
    }
}

impl CampusCardSession {
    pub fn is_authenticated(&self) -> bool {
        !self.account_serial.is_empty()
    }
}

/// Read-only card client.  It never creates a second transport, so the
/// caller can hand it the same Cookie-aware transport used for identity SSO.
#[derive(Clone)]
pub struct CampusCardClient {
    config: CampusCardAdapterConfig,
    transport: CampusHttpTransport,
    session_owner: Arc<()>,
}

impl fmt::Debug for CampusCardClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardClient")
            .field("config", &self.config)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

impl CampusCardClient {
    pub fn new(
        config: CampusCardAdapterConfig,
        transport: CampusHttpTransport,
    ) -> Result<Self, CampusCardAdapterError> {
        Ok(Self {
            config,
            transport,
            session_owner: Arc::new(()),
        })
    }

    pub fn config(&self) -> &CampusCardAdapterConfig {
        &self.config
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Verifies that the card SSO cookie resolves to a non-empty account.
    ///
    /// This is intentionally separate from `read_account`: a successful HTTP
    /// response or an existing identity session alone does not prove that the
    /// card service session is valid.
    pub async fn probe_session(&self) -> Result<CampusCardSession, CampusCardAdapterError> {
        self.probe_session_detailed().await.map_err(map_probe_error)
    }

    async fn probe_session_detailed(&self) -> Result<CampusCardSession, CampusCardAdapterError> {
        let result = self.post_json(SESSION_PATH, json!({})).await?;
        let result_data = decode_result_data(&result.body)?;
        let object = result_data
            .as_object()
            .ok_or(CampusCardAdapterError::InvalidSessionResponse)?;
        let account_binding = object
            .get("loginuser")
            .and_then(probe_account_binding)
            .ok_or(CampusCardAdapterError::InvalidSessionResponse)?;

        Ok(CampusCardSession {
            // Store the constructor's normalized value, rather than the raw
            // server string. This keeps every later idserial body identical
            // to the value used for account binding.
            account_serial: account_binding.as_str().to_owned(),
            owner: Arc::clone(&self.session_owner),
        })
    }

    /// Probes and binds the card session to the active identity account.
    pub async fn probe_session_for(
        &self,
        expected_account: &CampusCardAccountBinding,
    ) -> Result<CampusCardSession, CampusCardAdapterError> {
        let session = self.probe_session().await?;
        if !expected_account.matches(&session.account_serial) {
            return Err(CampusCardAdapterError::AccountMismatch);
        }
        Ok(session)
    }

    /// Exactly one read-only probe, preserving body-free typed errors for
    /// the Runtime's fixed diagnostic codes instead of collapsing envelopes
    /// and account-field errors into one misleading "missing account" state.
    pub(crate) async fn probe_session_for_diagnostic(
        &self,
        expected_account: &CampusCardAccountBinding,
    ) -> Result<CampusCardSession, CampusCardAdapterError> {
        let session = self.probe_session_detailed().await?;
        if !expected_account.matches(&session.account_serial) {
            return Err(CampusCardAdapterError::AccountMismatch);
        }
        Ok(session)
    }

    /// Reads balance/card metadata after a successful session probe.
    pub async fn read_account(
        &self,
        session: &CampusCardSession,
    ) -> Result<CampusCardAccount, CampusCardAdapterError> {
        self.read_account_for(session, None).await
    }

    /// Reads account metadata and verifies the response account binding.
    pub async fn read_account_for(
        &self,
        session: &CampusCardSession,
        expected_account: Option<&CampusCardAccountBinding>,
    ) -> Result<CampusCardAccount, CampusCardAdapterError> {
        self.ensure_session_owner(session)?;
        let session_account = session.account_binding()?;
        if expected_account.is_some_and(|expected| !expected.matches(&session.account_serial)) {
            return Err(CampusCardAdapterError::AccountMismatch);
        }
        let result = self
            .post_json(ACCOUNT_PATH, json!({"idserial": session.account_serial}))
            .await?;
        let envelope = decode_success_envelope(&result.body, CampusCardReadOperation::ReadAccount)?;
        // Always bind the returned business record to the account proven by
        // getUserInfoFromToken. The optional argument is an additional
        // caller-side binding, never a substitute for this check.
        parse_card_account_response(&envelope, Some(&session_account)).map_err(
            |error| match error {
                CampusCardParseError::AccountMismatch => CampusCardAdapterError::AccountMismatch,
                other => CampusCardAdapterError::Parse(other),
            },
        )
    }

    /// Reads a bounded transaction page.  The raw account serial remains in
    /// this method and is never included in the returned report.
    pub async fn read_transactions(
        &self,
        session: &CampusCardSession,
        query: &CampusCardTransactionQuery,
    ) -> Result<CampusCardTransactionReport, CampusCardAdapterError> {
        self.ensure_session_owner(session)?;
        // Keep this check even though the serial is private and is created by
        // probe_session. It makes the invariant explicit at every business
        // boundary and turns an impossible/corrupt session into expiry rather
        // than issuing an account-less request.
        session.account_binding()?;
        let body = json!({
            "idserial": session.account_serial,
            "starttime": query.start_date().as_str(),
            "endtime": query.end_date().as_str(),
            "tradetype": query.transaction_type().wire_value(),
            "pageSize": query.page_size(),
            "pageNumber": query.page_number(),
        });
        let result = self.post_json(TRANSACTIONS_PATH, body).await?;
        let envelope =
            decode_success_envelope(&result.body, CampusCardReadOperation::ReadTransactions)?;
        let report = parse_card_transactions_response(&envelope)?;
        let start = chrono::NaiveDate::parse_from_str(query.start_date().as_str(), "%Y-%m-%d")
            .expect("CampusCardTransactionQuery validates the start date");
        let end = chrono::NaiveDate::parse_from_str(query.end_date().as_str(), "%Y-%m-%d")
            .expect("CampusCardTransactionQuery validates the end date");
        for (row, transaction) in report.transactions.iter().enumerate() {
            let date = crate::campus_card_read::transaction_local_date(&transaction.occurred_at)
                .ok_or(CampusCardParseError::InvalidTransactionDate { row })?;
            if date < start || date > end {
                // Filtering here would hide a backend that ignored the date
                // selector and could make an incomplete page appear final.
                return Err(CampusCardParseError::TransactionOutsideRequestedRange { row }.into());
            }
        }
        Ok(report)
    }

    fn ensure_session_owner(
        &self,
        session: &CampusCardSession,
    ) -> Result<(), CampusCardAdapterError> {
        if Arc::ptr_eq(&self.session_owner, &session.owner) {
            Ok(())
        } else {
            Err(CampusCardAdapterError::SessionExpired)
        }
    }

    async fn post_json(
        &self,
        path: &str,
        body: Value,
    ) -> Result<CardHttpResponse, CampusCardAdapterError> {
        let endpoint = self.endpoint(path)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(
                self.transport
                    .client()
                    .post(endpoint)
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .header(reqwest::header::ACCEPT, "application/json")
                    .json(&body),
            )
            .await
            .map_err(|_| CampusCardAdapterError::Transport)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|_| CampusCardAdapterError::Transport)?;
        if !same_origin(&self.config.base_url, &final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                redirect_leaves_origin(&self.config.base_url, &final_url, location)
            })
        {
            return Err(CampusCardAdapterError::UnexpectedOrigin);
        }

        // Only an explicit card-portal handoff route is evidence that the
        // card session has expired. A same-origin 302 to an unrelated page, or
        // a 200 HTML maintenance/login-looking document on the JSON endpoint,
        // is a protocol/path failure and must remain distinguishable.
        if is_explicit_session_redirect(&self.config.base_url, &final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                resolve_location(&final_url, location)
                    .ok()
                    .is_some_and(|target| {
                        same_origin(&self.config.base_url, &target)
                            && is_explicit_session_redirect(&self.config.base_url, &target)
                    })
            })
        {
            return Err(CampusCardAdapterError::SessionExpired);
        }

        if status.is_redirection() {
            return Err(CampusCardAdapterError::UnexpectedRedirect);
        }

        let location_outside_path = redirect_location
            .as_deref()
            .and_then(|location| resolve_location(&final_url, location).ok())
            .is_some_and(|location| {
                same_origin(&self.config.base_url, &location)
                    && !path_within_base(&self.config.base_url, &location)
            });
        if !path_within_base(&self.config.base_url, &final_url) || location_outside_path {
            return Err(CampusCardAdapterError::UnexpectedPath);
        }
        // The card service shares its origin with multiple JSON routes. A
        // valid envelope from a different route is not a proof for the
        // operation that was requested.
        if final_url.path() != expected_path {
            return Err(CampusCardAdapterError::UnexpectedPath);
        }

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(CampusCardAdapterError::SessionExpired);
        }

        // The reference client accepts only the two success statuses used by
        // this legacy service. A 202/204 with an empty body must not become a
        // successful account or an empty transaction list.
        if status != StatusCode::OK && status != StatusCode::CREATED {
            return Err(CampusCardAdapterError::HttpStatus {
                status: status.as_u16(),
            });
        }
        Ok(CardHttpResponse { body })
    }

    fn endpoint(&self, path: &str) -> Result<Url, CampusCardAdapterError> {
        if !path.starts_with('/')
            || path.contains(['?', '#'])
            || path.contains("..")
            || path.contains("://")
            || path.chars().any(char::is_control)
        {
            return Err(CampusCardAdapterError::InvalidConfig);
        }
        let mut endpoint = self.config.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn redirect_leaves_origin(base_url: &Url, response_url: &Url, location: &str) -> bool {
    let Ok(candidate) = resolve_location(response_url, location) else {
        return true;
    };
    !same_origin(base_url, &candidate)
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, ()> {
    if invalid_percent_encoding(location) {
        return Err(());
    }
    Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| ())
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

fn path_within_base(base_url: &Url, candidate: &Url) -> bool {
    let base_path = base_url.path().trim_end_matches('/');
    base_path.is_empty()
        || base_path == "/"
        || candidate.path() == base_path
        || candidate.path().starts_with(&format!("{base_path}/"))
}

fn is_explicit_session_redirect(base_url: &Url, url: &Url) -> bool {
    if !same_origin(base_url, url) {
        return false;
    }

    let base_path = base_url.path().trim_end_matches('/');
    let candidate_path = url.path().trim_end_matches('/');
    let expected_path = if base_path.is_empty() {
        CARD_SESSION_REDIRECT_PATH
    } else {
        // A configured WebVPN-mapped card origin may have a path prefix. Keep
        // the explicit route relative to that prefix instead of accepting a
        // similarly named route elsewhere on the same host.
        return candidate_path == format!("{base_path}{CARD_SESSION_REDIRECT_PATH}");
    };
    candidate_path == expected_path
}

struct CardHttpResponse {
    body: String,
}

impl CampusCardSession {
    fn account_binding(&self) -> Result<CampusCardAccountBinding, CampusCardAdapterError> {
        CampusCardAccountBinding::new(&self.account_serial)
            .map_err(|_| CampusCardAdapterError::SessionExpired)
    }
}

/// Decodes the card service's outer success envelope, including its legacy
/// AES-128-ECB/PKCS#7 failure fallback.  The decrypted data is immediately
/// reduced to a `resultData` value and never leaves this function as an error
/// or debug payload.
fn probe_account_binding(value: &Value) -> Option<CampusCardAccountBinding> {
    match value {
        Value::String(value) => CampusCardAccountBinding::new(value).ok(),
        // Match the business idserial parser's lossless identifier handling,
        // not JS floating-point coercion. Booleans/floats/negative/zero and
        // null cannot establish an authenticated student account.
        Value::Number(value) => value
            .as_u64()
            .filter(|value| *value > 0)
            .and_then(|value| CampusCardAccountBinding::new(&value.to_string()).ok()),
        _ => None,
    }
}

fn decode_result_data(body: &str) -> Result<Value, CampusCardAdapterError> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    let root: Value = crate::campus_card_read::parse_card_json(body).map_err(|error| {
        if matches!(error, CampusCardParseError::MalformedJson)
            && (looks_like_login_html_response(body) || contains_session_marker(body))
        {
            CampusCardAdapterError::SessionExpired
        } else if looks_like_html_response(body) {
            CampusCardAdapterError::UnexpectedResponse
        } else {
            CampusCardAdapterError::Parse(error)
        }
    })?;
    let object = root.as_object().ok_or(CampusCardAdapterError::Parse(
        CampusCardParseError::MalformedEnvelope,
    ))?;
    // THUInfo decrypts `data` whenever the clear success branch is absent.
    // Missing/null/false/zero can designate an encrypted transport envelope;
    // the decrypted inner envelope must still contain boolean success=true.
    let success = match object.get("success") {
        Some(Value::Bool(true)) => true,
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Number(number)) if number.as_i64() == Some(0) => false,
        _ => {
            return Err(CampusCardAdapterError::Parse(
                CampusCardParseError::MalformedEnvelope,
            ));
        }
    };
    if !success && object_has_session_failure_marker(object) {
        return Err(CampusCardAdapterError::SessionExpired);
    }
    if success {
        return object
            .get("resultData")
            .filter(|value| !value.is_null())
            .cloned()
            .ok_or(CampusCardAdapterError::Parse(
                CampusCardParseError::MissingResultData,
            ));
    }

    let Some(data) = object.get("data").and_then(Value::as_str) else {
        return Err(map_plain_failure(body));
    };
    if contains_session_marker(data) {
        return Err(CampusCardAdapterError::SessionExpired);
    }
    let decrypted = decrypt_card_failure(data)?;
    let nested: Value =
        crate::campus_card_read::parse_card_json(&decrypted).map_err(|error| match error {
            CampusCardParseError::MalformedJson => CampusCardAdapterError::InvalidEncryptedPayload,
            other => CampusCardAdapterError::Parse(other),
        })?;
    let nested_object = nested
        .as_object()
        .ok_or(CampusCardAdapterError::InvalidEncryptedPayload)?;
    if nested_object.get("success").and_then(Value::as_bool) != Some(true) {
        if object_has_session_failure_marker(nested_object) {
            return Err(CampusCardAdapterError::SessionExpired);
        }
        return Err(CampusCardAdapterError::EncryptedServiceFailure);
    }
    nested_object
        .get("resultData")
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or(CampusCardAdapterError::Parse(
            CampusCardParseError::MissingResultData,
        ))
}

fn decode_success_envelope(
    body: &str,
    operation: CampusCardReadOperation,
) -> Result<String, CampusCardAdapterError> {
    let result_data = decode_result_data(body)?;
    Ok(json!({
        "success": true,
        "resultData": result_data,
        "data": null,
        "operation": operation,
    })
    .to_string())
}

fn map_plain_failure(body: &str) -> CampusCardAdapterError {
    match parse_card_transactions_response(body) {
        Err(CampusCardParseError::SessionExpired) => CampusCardAdapterError::SessionExpired,
        Err(CampusCardParseError::HtmlResponse) => CampusCardAdapterError::UnexpectedResponse,
        Err(error) => CampusCardAdapterError::Parse(error),
        Ok(_) => CampusCardAdapterError::Parse(CampusCardParseError::OuterFailure),
    }
}

fn map_probe_error(error: CampusCardAdapterError) -> CampusCardAdapterError {
    match error {
        // A probe without an explicit login marker is an invalid proof, not
        // evidence that the session expired. This distinction prevents a
        // transient card-service failure or an HTML maintenance page from
        // triggering an auth reset.
        CampusCardAdapterError::Parse(_) | CampusCardAdapterError::UnexpectedResponse => {
            CampusCardAdapterError::InvalidSessionResponse
        }
        other => other,
    }
}

fn contains_session_marker(value: &str) -> bool {
    let lower = value.to_lowercase();
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
        "请先登录",
        "请先登陆",
        "用户未登录",
        "未登录",
        "登录超时",
        "登陆超时",
        "会话过期",
        "会话已过期",
        "会话已失效",
        "登录失效",
        "登陆失效",
        "重新登录",
        "重新登陆",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn object_has_session_failure_marker(object: &serde_json::Map<String, Value>) -> bool {
    ["message", "msg", "error", "status", "data"]
        .iter()
        .filter_map(|key| object.get(*key))
        .filter_map(Value::as_str)
        .any(contains_session_marker)
}

fn decrypt_card_failure(data: &str) -> Result<String, CampusCardAdapterError> {
    let bytes = data.as_bytes();
    if bytes.len() <= 16
        || !data.is_char_boundary(16)
        || !bytes[..16].iter().all(|byte| byte.is_ascii())
    {
        return Err(CampusCardAdapterError::InvalidEncryptedPayload);
    }
    let key: [u8; 16] = bytes[..16]
        .try_into()
        .map_err(|_| CampusCardAdapterError::InvalidEncryptedPayload)?;
    let ciphertext = decode_base64(&data[16..])?;
    let plaintext = aes128_ecb_pkcs7_decrypt(&key, &ciphertext)?;
    String::from_utf8(plaintext).map_err(|_| CampusCardAdapterError::InvalidEncryptedPayload)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, CampusCardAdapterError> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err(CampusCardAdapterError::InvalidEncryptedPayload);
    }
    let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunk_count = bytes.len() / 4;
    for (chunk_index, chunk) in bytes.chunks_exact(4).enumerate() {
        let a = base64_value(chunk[0])?;
        let b = base64_value(chunk[1])?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            base64_value(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            base64_value(chunk[3])?
        };
        if chunk[2] == b'=' && chunk[3] != b'=' {
            return Err(CampusCardAdapterError::InvalidEncryptedPayload);
        }
        if chunk[2] == b'=' && (b & 0x0f) != 0 {
            return Err(CampusCardAdapterError::InvalidEncryptedPayload);
        }
        if chunk[3] == b'=' && chunk[2] != b'=' && (c & 0x03) != 0 {
            return Err(CampusCardAdapterError::InvalidEncryptedPayload);
        }
        output.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            output.push((c << 6) | d);
        }
        if (chunk[2] == b'=' || chunk[3] == b'=') && chunk_index + 1 != chunk_count {
            return Err(CampusCardAdapterError::InvalidEncryptedPayload);
        }
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Result<u8, CampusCardAdapterError> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(CampusCardAdapterError::InvalidEncryptedPayload),
    }
}

fn aes128_ecb_pkcs7_decrypt(
    key: &[u8; 16],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CampusCardAdapterError> {
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(CampusCardAdapterError::InvalidEncryptedPayload);
    }
    let round_keys = expand_key(key);
    let mut plaintext = Vec::with_capacity(ciphertext.len());
    for block in ciphertext.chunks_exact(16) {
        plaintext.extend_from_slice(&aes128_decrypt_block(&round_keys, block));
    }

    let padding = *plaintext
        .last()
        .ok_or(CampusCardAdapterError::InvalidEncryptedPayload)? as usize;
    if !(1..=16).contains(&padding)
        || plaintext.len() < padding
        || !plaintext[plaintext.len() - padding..]
            .iter()
            .all(|byte| usize::from(*byte) == padding)
    {
        return Err(CampusCardAdapterError::InvalidEncryptedPayload);
    }
    plaintext.truncate(plaintext.len() - padding);
    Ok(plaintext)
}

fn aes128_decrypt_block(round_keys: &[u8; 176], block: &[u8]) -> [u8; 16] {
    let mut state = [0_u8; 16];
    state.copy_from_slice(block);
    add_round_key(&mut state, &round_keys[160..176]);
    for round in (1..10).rev() {
        inv_shift_rows(&mut state);
        inv_sub_bytes(&mut state);
        add_round_key(&mut state, &round_keys[round * 16..(round + 1) * 16]);
        inv_mix_columns(&mut state);
    }
    inv_shift_rows(&mut state);
    inv_sub_bytes(&mut state);
    add_round_key(&mut state, &round_keys[..16]);
    state
}

fn expand_key(key: &[u8; 16]) -> [u8; 176] {
    let mut expanded = [0_u8; 176];
    expanded[..16].copy_from_slice(key);
    let mut generated = 16;
    let mut rcon = 1_u8;
    while generated < expanded.len() {
        let mut temp = [0_u8; 4];
        temp.copy_from_slice(&expanded[generated - 4..generated]);
        if generated % 16 == 0 {
            temp.rotate_left(1);
            for byte in &mut temp {
                *byte = sbox(*byte);
            }
            temp[0] ^= rcon;
            rcon = gf_mul(rcon, 2);
        }
        for byte in temp {
            expanded[generated] = expanded[generated - 16] ^ byte;
            generated += 1;
        }
    }
    expanded
}

fn add_round_key(state: &mut [u8; 16], key: &[u8]) {
    for (state_byte, key_byte) in state.iter_mut().zip(key.iter().copied()) {
        *state_byte ^= key_byte;
    }
}

fn inv_sub_bytes(state: &mut [u8; 16]) {
    for byte in state {
        *byte = inverse_sbox(*byte);
    }
}

fn inv_shift_rows(state: &mut [u8; 16]) {
    let original = *state;
    for row in 0..4 {
        for column in 0..4 {
            let source_column = (column + 4 - row) % 4;
            state[4 * column + row] = original[4 * source_column + row];
        }
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for column in state.chunks_exact_mut(4) {
        let [a, b, c, d] = *column else {
            unreachable!()
        };
        column[0] = gf_mul(a, 14) ^ gf_mul(b, 11) ^ gf_mul(c, 13) ^ gf_mul(d, 9);
        column[1] = gf_mul(a, 9) ^ gf_mul(b, 14) ^ gf_mul(c, 11) ^ gf_mul(d, 13);
        column[2] = gf_mul(a, 13) ^ gf_mul(b, 9) ^ gf_mul(c, 14) ^ gf_mul(d, 11);
        column[3] = gf_mul(a, 11) ^ gf_mul(b, 13) ^ gf_mul(c, 9) ^ gf_mul(d, 14);
    }
}

fn sbox(value: u8) -> u8 {
    let inverse = gf_pow(value, 254);
    inverse
        ^ inverse.rotate_left(1)
        ^ inverse.rotate_left(2)
        ^ inverse.rotate_left(3)
        ^ inverse.rotate_left(4)
        ^ 0x63
}

fn inverse_sbox(value: u8) -> u8 {
    // Inverting the affine map first, then taking the multiplicative inverse,
    // avoids embedding a second 256-byte table in this small adapter.
    let affine_inverse = value.rotate_left(1) ^ value.rotate_left(3) ^ value.rotate_left(6) ^ 0x05;
    gf_pow(affine_inverse, 254)
}

fn gf_pow(mut value: u8, mut exponent: u16) -> u8 {
    let mut result = 1_u8;
    while exponent != 0 {
        if exponent & 1 != 0 {
            result = gf_mul(result, value);
        }
        value = gf_mul(value, value);
        exponent >>= 1;
    }
    result
}

fn gf_mul(mut left: u8, mut right: u8) -> u8 {
    let mut result = 0_u8;
    for _ in 0..8 {
        if right & 1 != 0 {
            result ^= left;
        }
        let high = left & 0x80;
        left <<= 1;
        if high != 0 {
            left ^= 0x1b;
        }
        right >>= 1;
    }
    result
}

#[cfg(test)]
mod tests {

    #[test]
    fn backend_repair_business_card_matches_independent_reference_aes_vectors() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("business_card_reference_fixtures.json")).unwrap();
        assert_eq!(cases.len(), 5);
        for case in cases {
            let result = decode_result_data(&case["body"].to_string());
            if case["ok"] == true {
                assert_eq!(result.unwrap()["balance"].as_i64(), Some(12300));
            } else {
                assert!(matches!(
                    result,
                    Err(CampusCardAdapterError::EncryptedServiceFailure)
                ));
            }
        }
    }
    #[tokio::test]
    async fn backend_repair_business_card_full_encrypted_account_and_transaction_read() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let wrap = |plaintext: &str| {
            let cipher =
                aes128_ecb_pkcs7_encrypt_for_test(b"0123456789ABCDEF", plaintext.as_bytes());
            json!({"data":format!("0123456789ABCDEF{}",encode_base64(&cipher))}).to_string()
        };
        let server = FixtureServer::new(vec![
            Reply::json(&wrap(
                r#"{"success":true,"resultData":{"loginuser":"student-001"}}"#,
            )),
            Reply::json(&wrap(ACCOUNT)),
            Reply::json(&wrap(
                r#"{"success":true,"resultData":{"rows":[{"id":"fixture-row","txdate":"2026-09-18 12:00:00","summary":"Fixture","balance":10000.00,"txamt":-125.0,"meraddr":"","txname":"消费"}]}}"#,
            )),
        ]);
        let client = CampusCardClient::new(
            CampusCardAdapterConfig::new(server.base()).unwrap(),
            CampusHttpTransport::new("THYou/card-integration-fixture").unwrap(),
        )
        .unwrap();
        let expected = CampusCardAccountBinding::new("student-001").unwrap();
        let session = client.probe_session_for(&expected).await.unwrap();
        let account = client
            .read_account_for(&session, Some(&expected))
            .await
            .unwrap();
        assert_eq!(account.balance_cents, 12345);
        let query = CampusCardTransactionQuery::new(
            "2026-09-18",
            "2026-09-18",
            CampusCardTransactionType::Any,
            100,
            0,
        )
        .unwrap();
        let rows = client.read_transactions(&session, &query).await.unwrap();
        assert_eq!(rows.transactions[0].amount_cents, -125);
        assert_eq!(server.requests().len(), 3);
        assert!(
            server
                .requests()
                .iter()
                .all(|request| request.starts_with("POST "))
        );
    }

    #[test]
    fn backend_repair_business_card_encrypted_data_does_not_require_outer_success() {
        let clear =
            r#"{"success":true,"resultData":{"balance":12300.00,"idserial":"fixture-user"}}"#;
        let encrypted = aes128_ecb_pkcs7_encrypt_for_test(b"0123456789ABCDEF", clear.as_bytes());
        let data = format!("0123456789ABCDEF{}", encode_base64(&encrypted));
        for outer in [
            json!({"data":data}),
            json!({"success":null,"data":data}),
            json!({"success":false,"data":data}),
            json!({"success":0,"data":data}),
        ] {
            let result = decode_result_data(&outer.to_string()).unwrap();
            assert_eq!(result["balance"].as_i64(), Some(12300));
            assert_eq!(result["idserial"], "fixture-user");
        }
    }
    #[test]
    fn backend_repair_business_card_missing_success_cannot_accept_unverified_plain_result() {
        for body in [
            r#"{"resultData":{"balance":100}}"#,
            r#"{"success":"false","resultData":{"balance":100}}"#,
            r#"{"data":"broken-envelope"}"#,
        ] {
            assert!(decode_result_data(body).is_err());
        }
        let clear =
            r#"{"success":false,"resultData":{"balance":100},"message":"synthetic rejected"}"#;
        let encrypted = aes128_ecb_pkcs7_encrypt_for_test(b"0123456789ABCDEF", clear.as_bytes());
        let body = json!({"data":format!("0123456789ABCDEF{}",encode_base64(&encrypted))});
        assert!(matches!(
            decode_result_data(&body.to_string()),
            Err(CampusCardAdapterError::EncryptedServiceFailure)
        ));
    }
    #[test]
    fn backend_repair_service_followup_encrypted_decimal_amounts_keep_raw_precision() {
        let key = *b"0123456789ABCDEF";
        for (token, expected) in [
            ("9007199254740993.00", Some(9007199254740993_i64)),
            ("100.00000000000000001", None),
        ] {
            let clear = format!(r#"{{"success":true,"resultData":{{"balance":{token}}}}}"#);
            let encrypted = aes128_ecb_pkcs7_encrypt_for_test(&key, clear.as_bytes());
            let outer=json!({"success":false,"data":format!("0123456789ABCDEF{}",encode_base64(&encrypted))}).to_string();
            let result = decode_result_data(&outer);
            match expected {
                Some(value) => assert_eq!(result.unwrap()["balance"].as_i64(), Some(value)),
                None => assert!(matches!(
                    result,
                    Err(CampusCardAdapterError::Parse(
                        CampusCardParseError::InvalidCents { .. }
                    ))
                )),
            }
        }
    }
    use super::*;
    use crate::campus_card_read::{
        CampusCardTransactionType, MAX_TRANSACTION_PAGE_SIZE, MAX_TRANSACTION_WINDOW_DAYS,
    };
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
    };

    const ACCOUNT: &str = r#"{
      "success": true,
      "resultData": {
        "idserial": "student-001",
        "username": "张三",
        "engname": "Zhang San",
        "departname": "计算机系",
        "engdepartname": "Computer Science",
        "departid": 10,
        "sex": "M",
        "identifyeffectdate": "2024-09-01",
        "validatevalue": "2028-09-01",
        "baseAccount": { "balance": "12345" },
        "cardInfos": [{
          "cardid": "card-001", "accstatus": "正常", "lasttxdate": "2026-09-01",
          "maxconstolamt": 50000, "maxconsamt": "20000"
        }]
      },
      "data": null
    }"#;

    fn encrypted_failure(result_data: Value, success: bool) -> String {
        let key = *b"0123456789abcdef";
        let nested = json!({
            "success": success,
            "resultData": result_data,
        })
        .to_string();
        let ciphertext = aes128_ecb_pkcs7_encrypt_for_test(&key, nested.as_bytes());
        format!(
            "{{\"success\":false,\"data\":\"0123456789abcdef{}\"}}",
            encode_base64(&ciphertext)
        )
    }

    fn encrypted_nested_failure(nested: Value) -> String {
        let key = *b"0123456789abcdef";
        let ciphertext = aes128_ecb_pkcs7_encrypt_for_test(&key, nested.to_string().as_bytes());
        format!(
            "{{\"success\":false,\"data\":\"0123456789abcdef{}\"}}",
            encode_base64(&ciphertext)
        )
    }

    // Test-only companion for the independently implemented decryptor. The
    // production adapter never needs encryption and therefore exposes none.
    fn aes128_ecb_pkcs7_encrypt_for_test(key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        let padding = 16 - plaintext.len() % 16;
        let mut padded = plaintext.to_vec();
        padded.extend(std::iter::repeat_n(padding as u8, padding));
        let round_keys = expand_key(key);
        padded
            .chunks_exact(16)
            .map(|block| {
                let mut state = [0_u8; 16];
                state.copy_from_slice(block);
                add_round_key(&mut state, &round_keys[..16]);
                for round in 1..10 {
                    sub_bytes(&mut state);
                    shift_rows(&mut state);
                    mix_columns(&mut state);
                    add_round_key(&mut state, &round_keys[round * 16..(round + 1) * 16]);
                }
                sub_bytes(&mut state);
                shift_rows(&mut state);
                add_round_key(&mut state, &round_keys[160..176]);
                state
            })
            .flat_map(|block| block.into_iter())
            .collect()
    }

    fn sub_bytes(state: &mut [u8; 16]) {
        for byte in state {
            *byte = sbox(*byte);
        }
    }

    fn shift_rows(state: &mut [u8; 16]) {
        let original = *state;
        for row in 0..4 {
            for column in 0..4 {
                state[4 * column + row] = original[4 * ((column + row) % 4) + row];
            }
        }
    }

    fn mix_columns(state: &mut [u8; 16]) {
        for column in state.chunks_exact_mut(4) {
            let [a, b, c, d] = *column else {
                unreachable!()
            };
            column[0] = gf_mul(a, 2) ^ gf_mul(b, 3) ^ c ^ d;
            column[1] = a ^ gf_mul(b, 2) ^ gf_mul(c, 3) ^ d;
            column[2] = a ^ b ^ gf_mul(c, 2) ^ gf_mul(d, 3);
            column[3] = gf_mul(a, 3) ^ b ^ c ^ gf_mul(d, 2);
        }
    }

    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = String::new();
        for chunk in bytes.chunks(3) {
            let a = chunk[0];
            let b = chunk.get(1).copied().unwrap_or(0);
            let c = chunk.get(2).copied().unwrap_or(0);
            output.push(ALPHABET[(a >> 2) as usize] as char);
            output.push(ALPHABET[((a << 4 | b >> 4) & 0x3f) as usize] as char);
            output.push(if chunk.len() > 1 {
                ALPHABET[((b << 2 | c >> 6) & 0x3f) as usize] as char
            } else {
                '='
            });
            output.push(if chunk.len() > 2 {
                ALPHABET[(c & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        output
    }

    #[test]
    fn backend_repair_card_probe_numeric_identifier_matches_business_binding_losslessly() {
        for value in [json!(2026000001_u64), json!("2026000001")] {
            let binding = probe_account_binding(&value).unwrap();
            assert!(binding.matches("2026000001"));
            assert!(!binding.matches("2026000002"));
            assert!(!format!("{binding:?}").contains("2026000001"));
        }
        for value in [
            json!(true),
            json!(false),
            json!(0),
            json!(-1),
            json!(1.5),
            json!(null),
            json!({}),
            json!([]),
            json!(""),
        ] {
            assert!(probe_account_binding(&value).is_none());
        }
        assert!(
            probe_account_binding(&json!("001234"))
                .unwrap()
                .matches("001234")
        );
        assert!(
            !probe_account_binding(&json!(1234))
                .unwrap()
                .matches("001234")
        );
    }

    #[tokio::test]
    async fn backend_repair_card_runtime_probe_preserves_envelope_error_without_replay() {
        for (body, expected) in [
            (
                r#"{"success":true,"resultData":{"loginuser":2026000001}}"#,
                None,
            ),
            (
                r#"{"success":true}"#,
                Some(CampusCardAdapterError::Parse(
                    CampusCardParseError::MissingResultData,
                )),
            ),
            (
                r#"{"success":true,"resultData":{"loginuser":"2026000002"}}"#,
                Some(CampusCardAdapterError::AccountMismatch),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(request.starts_with("POST /login/getUserInfoFromToken HTTP/1.1"));
                assert!(!request.contains("i_pass"));
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                listener
            });
            let client = CampusCardClient::new(
                CampusCardAdapterConfig::new(&base).unwrap(),
                CampusHttpTransport::new("THYou/fixture").unwrap(),
            )
            .unwrap();
            let result = client
                .probe_session_for_diagnostic(&CampusCardAccountBinding::new("2026000001").unwrap())
                .await;
            match expected {
                Some(error) => assert_eq!(result.unwrap_err(), error),
                None => assert!(result.unwrap().is_authenticated()),
            }
            let listener = server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[test]
    fn exposes_only_safe_sso_profile_and_redacts_client_state() {
        let config = CampusCardAdapterConfig::new("https://card.tsinghua.edu.cn")
            .expect("valid card origin");
        assert_eq!(config.sso_profile(), CampusCardSsoProfile::standard());
        assert_eq!(config.base_origin(), "https://card.tsinghua.edu.cn");
        assert!(!format!("{config:?}").contains("loginuser"));
        assert!(!format!("{config:?}").contains("cookie"));
    }

    #[test]
    fn decrypts_crypto_js_style_aes_failure_envelope_without_leaking_payload() {
        let body = encrypted_failure(json!({"loginuser":"student-001"}), true);
        let value = decode_result_data(&body).expect("encrypted success fallback");
        assert_eq!(value["loginuser"], "student-001");
        let error = decode_result_data(&encrypted_failure(json!({"message":"private"}), false))
            .expect_err("nested service failure");
        assert_eq!(error, CampusCardAdapterError::EncryptedServiceFailure);
        assert!(!format!("{error:?}").contains("private"));
    }

    #[test]
    fn plaintext_success_envelope_is_reduced_through_the_existing_parser() {
        let envelope = decode_success_envelope(ACCOUNT, CampusCardReadOperation::ReadAccount)
            .expect("plaintext account envelope");
        let binding = CampusCardAccountBinding::new("student-001").expect("binding");
        let account = parse_card_account_response(&envelope, Some(&binding)).expect("account");
        assert_eq!(account.balance_cents, 12_345);
        assert!(!format!("{account:?}").contains("student-001"));

        let bom_envelope = decode_success_envelope(
            &format!("\u{feff}{ACCOUNT}"),
            CampusCardReadOperation::ReadAccount,
        )
        .expect("BOM-prefixed account envelope");
        assert_eq!(
            parse_card_account_response(&bom_envelope, Some(&binding))
                .expect("BOM account")
                .balance_cents,
            12_345
        );
    }

    #[test]
    fn rejects_invalid_encrypted_payloads_and_classifies_login_pages() {
        assert_eq!(
            decode_result_data(r#"{"success":false,"data":"short"}"#),
            Err(CampusCardAdapterError::InvalidEncryptedPayload)
        );
        assert_eq!(
            decode_result_data(r#"{"success":false,"data":"please login"}"#),
            Err(CampusCardAdapterError::SessionExpired)
        );
        assert_eq!(
            decode_result_data("<html><title>WebVPN login</title><form>Password</form></html>"),
            Err(CampusCardAdapterError::SessionExpired)
        );
        assert_eq!(
            decode_result_data(
                "\u{feff}<html><title>WebVPN login</title><form>Password</form></html>"
            ),
            Err(CampusCardAdapterError::SessionExpired)
        );

        let probe_failure = r#"{"success":false,"message":"session unavailable"}"#;
        assert_eq!(
            map_probe_error(map_plain_failure(probe_failure)),
            CampusCardAdapterError::InvalidSessionResponse
        );
        assert_eq!(
            decode_result_data(&encrypted_nested_failure(json!({
                "success": false,
                "message": "请先登录",
            }))),
            Err(CampusCardAdapterError::SessionExpired)
        );
        assert_eq!(
            decode_result_data(r#"{"success":true,"resultData":null}"#),
            Err(CampusCardAdapterError::Parse(
                CampusCardParseError::MissingResultData
            ))
        );
    }

    #[test]
    fn only_the_explicit_card_handoff_route_is_an_expired_session_signal() {
        let base = Url::parse("https://card.example.test/").expect("base URL");
        let handoff =
            Url::parse("https://card.example.test/getTYSFLoginUrlRedirect").expect("handoff URL");
        let similarly_named =
            Url::parse("https://card.example.test/account/getTYSFLoginUrlRedirect")
                .expect("unrelated URL");
        let foreign = Url::parse("https://identity.example.test/getTYSFLoginUrlRedirect")
            .expect("foreign URL");

        assert!(is_explicit_session_redirect(&base, &handoff));
        assert!(!is_explicit_session_redirect(&base, &similarly_named));
        assert!(!is_explicit_session_redirect(&base, &foreign));
    }

    #[test]
    fn request_query_keeps_reference_type_values_and_local_bounds() {
        let query = CampusCardTransactionQuery::new(
            "2026-09-01",
            "2026-09-31",
            CampusCardTransactionType::Any,
            MAX_TRANSACTION_PAGE_SIZE,
            0,
        );
        assert!(query.is_err());
        assert_eq!(MAX_TRANSACTION_WINDOW_DAYS, 31);
    }

    #[test]
    fn aes_decryptor_matches_nist_single_block_vector() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let ciphertext = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        assert_eq!(
            aes128_decrypt_block(&expand_key(&key), &ciphertext),
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn client_reuses_cookie_transport_and_sends_the_three_observed_read_shapes() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let responses = [
                r#"{"success":true,"resultData":{"loginuser":"student-001"},"data":null}"#,
                ACCOUNT,
                r#"{"success":true,"resultData":{"rows":[]},"data":null}"#,
            ];
            for response_body in responses {
                let (mut stream, _) = listener.accept().expect("accept fixture request");
                let request = read_http_request(&mut stream);
                requests_tx.send(request).expect("send request assertion");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write fixture response");
            }
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        let card_origin = Url::parse(&format!("http://{address}/")).expect("card origin");
        client
            .transport()
            .cookie_jar()
            .add_cookie_str("card-sso=fixture; Path=/", &card_origin);
        let binding = CampusCardAccountBinding::new("student-001").expect("binding");
        let session = client.probe_session_for(&binding).await.expect("probe");
        let account = client
            .read_account_for(&session, Some(&binding))
            .await
            .expect("account");
        assert_eq!(account.balance_cents, 12_345);
        let query = CampusCardTransactionQuery::new(
            "2026-09-01",
            "2026-09-07",
            CampusCardTransactionType::Any,
            50,
            3,
        )
        .expect("query");
        assert!(
            client
                .read_transactions(&session, &query)
                .await
                .expect("transactions")
                .is_empty()
        );

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 3);
        for request in &requests {
            let lower = request.to_ascii_lowercase();
            assert!(lower.contains("user-agent: thyou/campus-card-test"));
            assert!(lower.contains("accept: application/json"));
            assert!(lower.contains("content-type: application/json"));
            assert!(lower.contains("cookie: card-sso=fixture"));
        }
        assert!(requests[0].starts_with("POST /login/getUserInfoFromToken HTTP/1.1"));
        assert!(requests[0].contains("{}"));
        assert!(requests[1].starts_with("POST /business/getCardUserinfo HTTP/1.1"));
        assert!(requests[1].contains(r#""idserial":"student-001""#));
        assert!(requests[2].starts_with("POST /business/querySelfTradeList HTTP/1.1"));
        assert!(requests[2].contains(r#""starttime":"2026-09-01""#));
        assert!(requests[2].contains(r#""tradetype":-1"#));
        assert!(requests[2].contains(r#""pageSize":50"#));
        assert!(requests[2].contains(r#""pageNumber":3"#));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_encrypted_failure_is_not_an_empty_transaction_result() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let responses = [
                r#"{"success":true,"resultData":{"loginuser":"student-001"},"data":null}"#,
                r#"{"success":false,"data":"short"}"#,
            ];
            for body in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let _ = read_http_request(&mut stream);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-encrypted-failure-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        let session = client.probe_session().await.expect("session proof");
        let query = CampusCardTransactionQuery::new(
            "2026-09-01",
            "2026-09-01",
            CampusCardTransactionType::Any,
            1,
            0,
        )
        .expect("query");
        let error = client
            .read_transactions(&session, &query)
            .await
            .expect_err("malformed encrypted envelope");
        assert_eq!(error, CampusCardAdapterError::InvalidEncryptedPayload);
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_cross_origin_redirect_is_rejected_before_session_proof() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_http_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: https://identity.example.test/login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("card config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-origin-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::UnexpectedOrigin)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_same_origin_card_handoff_redirect_is_session_expiry() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("accept initial request");
            let _ = read_http_request(&mut first);
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /getTYSFLoginUrlRedirect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut handoff, _) = listener.accept().expect("accept handoff request");
            let _ = read_http_request(&mut handoff);
            let body = "<html><title>card handoff</title></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            handoff
                .write_all(response.as_bytes())
                .expect("handoff response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("card config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-redirect-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::SessionExpired)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_html_on_the_json_route_is_not_session_expiry() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_http_request(&mut stream);
            let body = "<html><title>maintenance</title></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("card config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-html-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::InvalidSessionResponse)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_unexplained_302_is_not_session_expiry() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_http_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("card config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-302-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::UnexpectedRedirect)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_same_origin_redirect_outside_card_base_is_rejected() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut redirect, _) = listener.accept().expect("redirect request");
            let _ = read_http_request(&mut redirect);
            redirect
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut unrelated, _) = listener.accept().expect("unrelated request");
            let _ = read_http_request(&mut unrelated);
            let body = r#"{"success":true,"resultData":{"loginuser":"student-001"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            unrelated
                .write_all(response.as_bytes())
                .expect("unrelated response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/card/")).expect("card config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-path-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::UnexpectedPath)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_http_same_origin_redirect_inside_card_base_is_still_rejected() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut redirect, _) = listener.accept().expect("redirect request");
            let _ = read_http_request(&mut redirect);
            redirect
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /card/unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut unrelated, _) = listener.accept().expect("unrelated request");
            let _ = read_http_request(&mut unrelated);
            let body = r#"{"success":true,"resultData":{"loginuser":"student-001"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            unrelated.write_all(response.as_bytes()).expect("response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/card/")).expect("config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-route-proof-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::UnexpectedPath)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn card_sso_cookie_is_required_before_loginuser_can_prove_a_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut handoff, _) = listener.accept().expect("accept handoff");
            let handoff_request = read_http_request(&mut handoff);
            requests_tx
                .send(handoff_request)
                .expect("send handoff request");
            let handoff_body = "ok";
            let handoff_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nSet-Cookie: card_session=fixture; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                handoff_body.len(),
                handoff_body
            );
            handoff
                .write_all(handoff_response.as_bytes())
                .expect("write handoff response");

            let (mut probe, _) = listener.accept().expect("accept probe");
            let probe_request = read_http_request(&mut probe);
            let has_cookie = probe_request.lines().any(|line| {
                line.to_ascii_lowercase().starts_with("cookie:")
                    && line.contains("card_session=fixture")
            });
            requests_tx.send(probe_request).expect("send probe request");
            let probe_body = if has_cookie {
                r#"{"success":true,"resultData":{"loginuser":"student-001"},"data":null}"#
            } else {
                r#"{"success":false,"message":"未登录","resultData":null}"#
            };
            let probe_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                probe_body.len(),
                probe_body
            );
            probe
                .write_all(probe_response.as_bytes())
                .expect("write probe response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let handoff_response = transport
            .client()
            .get(format!("http://{address}/sso"))
            .send()
            .await
            .expect("handoff request");
        assert_eq!(handoff_response.status(), StatusCode::OK);

        let client = CampusCardClient::new(config, transport).expect("client");
        let binding = CampusCardAccountBinding::new("student-001").expect("binding");
        let session = client
            .probe_session_for(&binding)
            .await
            .expect("cookie-backed session probe");
        assert!(session.is_authenticated());

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /sso HTTP/1.1"));
        assert!(requests[1].starts_with("POST /login/getUserInfoFromToken HTTP/1.1"));
        assert!(requests[1].contains("content-type: application/json"));
        assert!(requests[1].contains("{}"));
        assert!(
            requests[1]
                .lines()
                .any(|line| line.to_ascii_lowercase().starts_with("cookie:")
                    && line.contains("card_session=fixture"))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn probe_account_binding_must_match_the_authenticated_identity() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept probe");
            let _ = read_http_request(&mut stream);
            let body = r#"{"success":true,"resultData":{"loginuser":"other-account"},"data":null}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("card config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-binding-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        let expected = CampusCardAccountBinding::new("expected-account").expect("binding");
        assert!(matches!(
            client.probe_session_for(&expected).await,
            Err(CampusCardAdapterError::AccountMismatch)
        ));
        server.join().expect("server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn account_response_must_match_the_proven_card_account() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut probe, _) = listener.accept().expect("accept probe");
            let _ = read_http_request(&mut probe);
            let probe_body =
                r#"{"success":true,"resultData":{"loginuser":"student-001"},"data":null}"#;
            let probe_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                probe_body.len(),
                probe_body
            );
            probe
                .write_all(probe_response.as_bytes())
                .expect("write probe response");

            let (mut account, _) = listener.accept().expect("accept account");
            let _ = read_http_request(&mut account);
            let mismatched = ACCOUNT.replace("student-001", "student-002");
            let account_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                mismatched.len(),
                mismatched
            );
            account
                .write_all(account_response.as_bytes())
                .expect("write account response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        let binding = CampusCardAccountBinding::new("student-001").expect("binding");
        let session = client.probe_session_for(&binding).await.expect("probe");
        assert_eq!(
            client.read_account(&session).await,
            Err(CampusCardAdapterError::AccountMismatch)
        );
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_nonstandard_success_status_is_not_a_card_session_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_http_request(&mut stream);
            let body = r#"{"success":true,"resultData":{"loginuser":"student-001"}}"#;
            let response = format!(
                "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });

        let config =
            CampusCardAdapterConfig::new(&format!("http://{address}/")).expect("fixture config");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/campus-card-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let client = CampusCardClient::new(config, transport).expect("client");
        assert!(matches!(
            client.probe_session().await,
            Err(CampusCardAdapterError::HttpStatus { status: 202 })
        ));
        server.join().expect("fixture server");
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let header_end;
        loop {
            let mut chunk = [0_u8; 1024];
            let count = stream.read(&mut chunk).expect("read fixture request");
            assert!(count > 0, "fixture request ended before headers");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                header_end = index + 4;
                break;
            }
        }
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then_some(value)
            })
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let count = stream.read(&mut chunk).expect("read fixture body");
            assert!(count > 0, "fixture request ended before body");
            bytes.extend_from_slice(&chunk[..count]);
        }
        String::from_utf8_lossy(&bytes[..header_end + content_length]).into_owned()
    }
}
