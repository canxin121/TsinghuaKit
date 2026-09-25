//! TUNet/srun request planning and response decoding.
//!
//! `tunet.rs` owns the wire profile: endpoint, paths, HTTP methods, callback,
//! `ac_id`, IP policy, and the caller-supplied outer digest metadata.  This
//! module composes that profile with the shared transport and keeps the
//! client-facing operations small.  The generic `plan_login` path does not
//! calculate a password digest: a caller must provide the already calculated
//! wire value, and an unknown or mismatched digest profile is reported
//! explicitly.  The current public `srun_bx1` path is separate:
//! `plan_srun_login` derives the token-keyed HMAC-MD5 password, xencoded
//! `info`, and checksum inside Rust.

use std::{
    future::Future,
    net::IpAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    time::Duration,
};

use reqwest::{Url, header::CONTENT_TYPE};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    error::Service,
    transport::{CampusHttpTransport, CampusTextResponse, TransportError},
    tunet::{
        AcIdPolicy, AuthFamily, CommaStatusSignal, HttpMethod, IpPolicy, OuterDigest,
        OuterDigestScheme, ProfileError, ResponseDecodeError, StatusResponse, TunetOperation,
        TunetProfile, TunetProfileOverrides, TunetRequest, decode_jsonp, decode_status_response,
    },
    tunet_auth::{
        SrunPasswordDigestScheme, build_srun_login_material, srun_request_n, srun_request_type,
    },
};

const DEFAULT_USER_AGENT: &str = "TsinghuaKit/0.2.0-alpha.1";

// The public Portal.js waits about three seconds for the RADIUS state after a
// login/logout CGI response. The first request is immediate; the remaining
// attempts are separated by one second. Tests inject an immediate sleeper
// below, so the boundary remains deterministic without slowing the suite.
const STATUS_CONFIRM_MAX_ATTEMPTS: usize = 4;
const STATUS_CONFIRM_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusExpectation {
    Online,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusPollDecision {
    Confirmed,
    Retry,
    Rejected,
    WrongState,
}

fn classify_status_for_poll_for_ip(
    record: &TunetStatusRecord,
    expectation: StatusExpectation,
    expected_ip: Option<&str>,
) -> StatusPollDecision {
    // `not_online_error` is the one narrowly recognised negative response for
    // an offline proof. The portal uses it when a logout request has already
    // removed the requested session. Every other negative response remains a
    // protocol rejection: credentials, expired challenges, and transport
    // gateway pages must never be turned into a successful logout.
    if record.signal == TunetStatusSignal::Negative {
        if expectation == StatusExpectation::Offline
            && record.is_explicit_offline_proven(expected_ip)
        {
            return StatusPollDecision::Confirmed;
        }
        return StatusPollDecision::Rejected;
    }

    let state = expected_ip
        .map(|ip| record.online_state_for_ip(ip))
        .unwrap_or_else(|| record.online_state());

    match (expectation, state) {
        (StatusExpectation::Online, TunetOnlineState::Online)
            if record.signal == TunetStatusSignal::Positive =>
        {
            StatusPollDecision::Confirmed
        }
        (StatusExpectation::Offline, TunetOnlineState::Offline)
            if record.signal == TunetStatusSignal::Positive =>
        {
            StatusPollDecision::Confirmed
        }
        (_, TunetOnlineState::Unknown) => StatusPollDecision::Retry,
        (_, _) if record.signal == TunetStatusSignal::Unknown => StatusPollDecision::Retry,
        _ => StatusPollDecision::WrongState,
    }
}

/// Polling uses an injected sleeper so the confirmation policy can be tested
/// without waiting. The production crate does not depend on Tokio directly;
/// this small future wakes the async task from a short helper thread instead
/// of blocking the executor thread during the confirmation window.
struct ThreadSleep {
    state: Arc<Mutex<ThreadSleepState>>,
}

struct ThreadSleepState {
    completed: bool,
    waker: Option<Waker>,
}

impl ThreadSleep {
    fn new(duration: Duration) -> Self {
        let state = Arc::new(Mutex::new(ThreadSleepState {
            completed: duration.is_zero(),
            waker: None,
        }));

        if !duration.is_zero() {
            let worker_state = Arc::clone(&state);
            std::thread::spawn(move || {
                std::thread::sleep(duration);
                let waker = {
                    let mut state = worker_state
                        .lock()
                        .expect("TUNet status sleeper mutex must not be poisoned");
                    state.completed = true;
                    state.waker.take()
                };
                if let Some(waker) = waker {
                    waker.wake();
                }
            });
        }

        Self { state }
    }
}

impl Future for ThreadSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self
            .state
            .lock()
            .expect("TUNet status sleeper mutex must not be poisoned");
        if state.completed {
            Poll::Ready(())
        } else {
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

fn status_poll_sleep(duration: Duration) -> ThreadSleep {
    ThreadSleep::new(duration)
}

/// Configuration shared by clients that use one auth4 or auth6 profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunetClientConfig {
    profile: TunetProfile,
    user_agent: String,
}

impl TunetClientConfig {
    /// Build a configuration with TsinghuaKit's default user agent.
    pub fn new(profile: TunetProfile) -> Result<Self, TunetClientError> {
        Self::with_user_agent(profile, DEFAULT_USER_AGENT)
    }

    /// Build a current auth4/auth6 configuration with TsinghuaKit's defaults.
    pub fn current(auth: AuthFamily) -> Result<Self, TunetClientError> {
        Self::new(TunetProfile::current(auth)?)
    }

    /// Build the current IPv4 configuration.
    pub fn auth4() -> Result<Self, TunetClientError> {
        Self::current(AuthFamily::Auth4)
    }

    /// Build the current IPv6 configuration.
    pub fn auth6() -> Result<Self, TunetClientError> {
        Self::current(AuthFamily::Auth6)
    }

    /// Build a current configuration while replacing selected profile data.
    pub fn current_with_overrides(
        auth: AuthFamily,
        overrides: TunetProfileOverrides,
    ) -> Result<Self, TunetClientError> {
        Self::new(TunetProfile::current_with_overrides(auth, overrides)?)
    }

    /// Build a configuration with an application-specific user agent.
    pub fn with_user_agent(
        profile: TunetProfile,
        user_agent: impl Into<String>,
    ) -> Result<Self, TunetClientError> {
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() {
            return Err(TunetClientError::EmptyUserAgent);
        }
        validate_client_profile(&profile)?;

        Ok(Self {
            profile,
            user_agent,
        })
    }

    pub fn profile(&self) -> &TunetProfile {
        &self.profile
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

/// A request produced by a TUNet client plan method.
pub type TunetRequestPlan = TunetRequest;

/// A client-side input list for endpoint-specific parameters.
///
/// The profile remains the owner of callback, fixed `ac_id`, and fixed IP
/// values.  Callers should put an `ip` or `ac_id` pair here only when the
/// profile uses `FromRequest` for that parameter.  Keeping the pairs visible
/// allows a deployment to retain additional srun parameters without a second
/// hard-coded parameter model.
pub type TunetExtraParams<'a> = [(&'a str, &'a str)];

/// Stable errors returned by request planning and TUNet response parsing.
#[derive(Debug, Error)]
pub enum TunetClientError {
    #[error("invalid TUNet profile: {0}")]
    Profile(#[from] ProfileError),

    #[error("TUNet transport failed")]
    Transport(TransportError),

    #[error("TUNet response envelope is invalid: {0}")]
    Response(#[from] ResponseDecodeError),

    #[error("TUNet response returned HTTP status {status}")]
    HttpStatus { status: reqwest::StatusCode },

    #[error("TUNet response did not remain on the requested endpoint")]
    UnexpectedResponseLocation,

    #[error("TUNet response used an unsupported content type")]
    UnsupportedContentType,

    #[error("TUNet JSON payload could not be decoded: {message}")]
    JsonDecode { message: String },

    #[error("challenge response does not contain a non-empty challenge token")]
    MissingChallengeToken,

    #[error("challenge response has an invalid challenge field")]
    InvalidChallengePayload,

    #[error("TUNet challenge request was rejected by the portal (code={code:?})")]
    ChallengeRejected { code: Option<String> },

    #[error("username must not be empty")]
    EmptyUsername,

    #[error("user agent must not be empty")]
    EmptyUserAgent,

    #[error("unsupported TUNet digest: configured={configured:?}, provided={provided:?}")]
    UnsupportedDigest {
        configured: OuterDigestScheme,
        provided: OuterDigestScheme,
    },

    #[error("unsupported SRun login digest profile: configured={configured:?}")]
    UnsupportedSrunDigest { configured: OuterDigestScheme },

    #[error("SRun login material could not be built: {0}")]
    SrunLogin(#[from] crate::tunet_auth::SrunLoginError),

    #[error("unsupported TUNet {operation:?} response variant: {variant}")]
    UnsupportedVariant {
        operation: TunetOperation,
        variant: String,
    },

    #[error("TUNet {operation:?} request was not accepted by the portal (code={code:?})")]
    PortalRejected {
        operation: TunetOperation,
        code: Option<String>,
    },

    #[error("TUNet {operation:?} response did not prove a positive result (signal={signal:?})")]
    ResponseNotProven {
        operation: TunetOperation,
        signal: TunetStatusSignal,
    },

    #[error("TUNet status did not prove that the account is online")]
    OnlineStateUnproven,

    #[error("TUNet status belongs to another or an invalid account")]
    AccountMismatch,
}

impl From<TransportError> for TunetClientError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

/// Coarse, stable classification for UI and runtime callers.  The concrete
/// error variants retain the protocol operation and safe protocol code; this
/// enum lets callers decide whether a failure is retryable or needs new login
/// credentials without inspecting error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunetErrorClass {
    Configuration,
    Transport,
    Http,
    Authentication,
    Protocol,
    Response,
    Proof,
}

impl TunetClientError {
    pub fn class(&self) -> TunetErrorClass {
        match self {
            Self::Profile(_)
            | Self::EmptyUsername
            | Self::EmptyUserAgent
            | Self::UnsupportedDigest { .. }
            | Self::UnsupportedSrunDigest { .. }
            | Self::SrunLogin(_) => TunetErrorClass::Configuration,
            Self::Transport(_) => TunetErrorClass::Transport,
            Self::HttpStatus { status } if is_authentication_status(*status) => {
                TunetErrorClass::Authentication
            }
            Self::HttpStatus { .. } => TunetErrorClass::Http,
            Self::AccountMismatch => TunetErrorClass::Authentication,
            Self::ChallengeRejected { .. } => TunetErrorClass::Protocol,
            Self::PortalRejected { operation, code }
                if *operation == TunetOperation::Login
                    || protocol_code_is_authentication_failure(code.as_deref()) =>
            {
                TunetErrorClass::Authentication
            }
            Self::ResponseNotProven { .. } | Self::OnlineStateUnproven => TunetErrorClass::Proof,
            Self::Response(_)
            | Self::UnexpectedResponseLocation
            | Self::UnsupportedContentType
            | Self::JsonDecode { .. } => TunetErrorClass::Response,
            Self::MissingChallengeToken | Self::InvalidChallengePayload => {
                TunetErrorClass::Protocol
            }
            Self::UnsupportedVariant { .. } => TunetErrorClass::Protocol,
            Self::PortalRejected { .. } => TunetErrorClass::Protocol,
        }
    }

    /// Whether the caller should refresh authentication material instead of
    /// treating this as a transient campus-network outage.
    pub fn is_authentication_failure(&self) -> bool {
        self.class() == TunetErrorClass::Authentication
    }

    /// Whether a bounded status poll may retry this error.
    pub fn is_temporary_network(&self) -> bool {
        match self {
            Self::Transport(TransportError::Request(error))
            | Self::Transport(TransportError::Decode(error)) => {
                error.is_timeout() || error.is_connect()
            }
            Self::Transport(TransportError::HttpStatus { status, .. })
            | Self::HttpStatus { status }
                if is_temporary_http_status(*status) =>
            {
                true
            }
            _ => false,
        }
    }
}

fn is_authentication_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

fn is_temporary_http_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::REQUEST_TIMEOUT
            | reqwest::StatusCode::TOO_EARLY
            | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

fn validate_client_profile(profile: &TunetProfile) -> Result<(), TunetClientError> {
    profile.validate().map_err(TunetClientError::Profile)?;
    match profile.validate_srun_digest() {
        Ok(()) => Ok(()),
        Err(ProfileError::UnsupportedDigestScheme(configured)) => {
            Err(TunetClientError::UnsupportedSrunDigest { configured })
        }
        Err(error) => Err(TunetClientError::Profile(error)),
    }
}

/// Response encodings accepted by the status endpoints.  The current portal
/// normally uses JSONP, while deployments and test gateways also return
/// plain JSON or the legacy comma record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunetStatusEncoding {
    Json,
    Jsonp,
    Comma,
}

/// A conservative status hint.
///
/// `Unknown` is intentional.  A status payload can contain deployment
/// specific fields, and this client must not turn an unfamiliar field layout
/// into an authentication claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunetStatusSignal {
    Positive,
    Negative,
    Unknown,
}

/// An explicit online state returned by the portal status payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunetOnlineState {
    Online,
    Offline,
    Unknown,
}

/// A stable representation of either JSONP or comma-delimited status data.
///
/// JSONP records preserve the decoded JSON value and callback.  Comma records
/// preserve every trimmed field, including trailing empty fields.  The
/// representation never assigns meaning to positional comma fields.
#[derive(Clone, PartialEq, Eq)]
pub struct TunetStatusRecord {
    pub service: Service,
    pub encoding: TunetStatusEncoding,
    pub callback: Option<String>,
    pub signal: TunetStatusSignal,
    pub fields: Vec<String>,
    pub payload: Option<Value>,
    /// When the record came from `TunetClient::status` with a requested IP,
    /// keep the public state accessors bound to that address.  The parser is
    /// also used independently, so records produced by `parse_status_response`
    /// leave this unset and retain their account-level interpretation.
    bound_online_state: Option<TunetOnlineState>,
}

/// Typed traffic and balance fields from the JSON `rad_user_info` response.
///
/// These names and the required numeric shapes are taken from current open
/// client models.  The values remain raw portal units: this type deliberately
/// does not guess whether a balance is yuan, fen, or another deployment unit.
/// A response that only has the legacy positional comma layout is rejected by
/// [`TunetStatusRecord::usage_snapshot`] until a deployment profile provides
/// independent field-position proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunetUsageSnapshot {
    pub used_bytes: u64,
    pub used_seconds: u64,
    pub remaining_bytes: u64,
    pub remaining_seconds: u64,
    pub account_balance: String,
}

impl std::fmt::Debug for TunetStatusRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TunetStatusRecord")
            .field("service", &self.service)
            .field("encoding", &self.encoding)
            .field("callback", &self.callback)
            .field("signal", &self.signal)
            .field("field_count", &self.fields.len())
            .field("payload_present", &self.payload.is_some())
            .finish()
    }
}

impl TunetStatusRecord {
    pub(crate) fn unproven_reason_for_ip(&self, requested: &str) -> &'static str {
        if !self.is_online_proven_for_ip(requested)
            && !self.is_offline_proven_for_ip(requested)
            && self.is_positive()
            && let (Ok(expected), Some(object)) = (
                requested.parse::<IpAddr>(),
                self.payload.as_ref().and_then(Value::as_object),
            )
        {
            let key = if expected.is_ipv4() {
                "online_ip"
            } else {
                "online_ip6"
            };
            if let Some(actual) = object
                .get(key)
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<IpAddr>().ok())
                && !actual.is_unspecified()
                && actual != expected
            {
                return "tunet_address_mismatch";
            }
        }
        "tunet_state_unproven"
    }

    /// Describe the existing proof decision without exposing IPs, usernames,
    /// callbacks, balances, payload keys or response bodies. This adds no
    /// request and does not relax online/offline proof requirements.
    pub(crate) fn trace_status_diagnostic(&self, expected_ip: Option<&str>) {
        let response_encoding = match self.encoding {
            TunetStatusEncoding::Json => "json",
            TunetStatusEncoding::Jsonp => "jsonp",
            TunetStatusEncoding::Comma => "legacy_comma",
        };
        let status_signal = match self.signal {
            TunetStatusSignal::Positive => "positive",
            TunetStatusSignal::Negative => "negative",
            TunetStatusSignal::Unknown => "unknown",
        };
        let name = |state| match state {
            TunetOnlineState::Online => "online",
            TunetOnlineState::Offline => "offline",
            TunetOnlineState::Unknown => "unknown",
        };
        let observed_state = name(
            self.payload
                .as_ref()
                .map(json_online_state)
                .unwrap_or(TunetOnlineState::Unknown),
        );
        let bound_state = name(
            expected_ip
                .map(|ip| self.online_state_for_ip(ip))
                .unwrap_or_else(|| self.online_state()),
        );
        let address_field_present = self
            .payload
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|object| {
                object.contains_key("online_ip") || object.contains_key("online_ip6")
            });
        let online_proven = expected_ip
            .map(|ip| self.is_online_proven_for_ip(ip))
            .unwrap_or_else(|| self.is_online_proven());
        let offline_proven = expected_ip
            .map(|ip| self.is_offline_proven_for_ip(ip))
            .unwrap_or_else(|| self.is_offline_proven());
        tracing::debug!(target:"tsinghua_kit::auth",event="tunet_status_evidence",service="tunet",response_encoding,status_signal,observed_state,bound_state,address_field_present,expected_ip_present=expected_ip.is_some(),online_proven,offline_proven);
    }

    /// Enforce an account binding when the endpoint supplies user_name. A
    /// missing field retains legacy IP-only behavior; an explicit different
    /// account or malformed present identifier is never silently accepted.
    pub(crate) fn account_binding_conflicts(&self, expected: &str) -> bool {
        let Some(value) = self
            .payload
            .as_ref()
            .and_then(|payload| payload.get("user_name"))
        else {
            return false;
        };
        match value.as_str() {
            Some(name) if name.trim().is_empty() => self.is_online_proven(),
            Some(name) => name != expected || name.chars().any(char::is_control),
            None => true,
        }
    }

    /// A positive `error` field only proves that the endpoint handled the
    /// request. This method requires an explicit online field as well.
    pub fn online_state(&self) -> TunetOnlineState {
        self.bound_online_state.unwrap_or_else(|| {
            self.payload
                .as_ref()
                .map(json_online_state)
                .unwrap_or(TunetOnlineState::Unknown)
        })
    }

    pub fn is_positive(&self) -> bool {
        self.signal == TunetStatusSignal::Positive
    }

    pub fn is_online_proven(&self) -> bool {
        self.is_positive() && self.online_state() == TunetOnlineState::Online
    }

    pub fn is_online_proven_for_ip(&self, expected_ip: &str) -> bool {
        self.is_positive() && self.online_state_for_ip(expected_ip) == TunetOnlineState::Online
    }

    /// Evaluate the status proof for one requested network address.
    ///
    /// `rad_user_info` can report both stacks at once.  An account-level
    /// `online_ip` therefore is not enough to prove that the address used by
    /// a login or disconnect operation changed state.  When the caller knows
    /// the target IP, this method binds the proof to the corresponding
    /// `online_ip`/`online_ip6` field and rejects a response that only shows a
    /// different stack or a different device.
    pub fn online_state_for_ip(&self, expected_ip: &str) -> TunetOnlineState {
        let Ok(expected) = expected_ip.trim().parse::<IpAddr>() else {
            return TunetOnlineState::Unknown;
        };

        let Some(payload) = self.payload.as_ref() else {
            return TunetOnlineState::Unknown;
        };
        let Some(object) = payload.as_object() else {
            return TunetOnlineState::Unknown;
        };

        let explicit_state = ["online", "is_online", "isOnline", "onlineStatus"]
            .iter()
            .filter_map(|name| object.get(*name))
            .map(json_value_online_state)
            .find(|state| *state != TunetOnlineState::Unknown);

        let field_name = match expected {
            IpAddr::V4(_) => "online_ip",
            IpAddr::V6(_) => "online_ip6",
        };
        let address_state = object.get(field_name).map(|value| {
            let Some(text) = value.as_str().map(str::trim) else {
                return TunetOnlineState::Unknown;
            };
            if text.is_empty() {
                return TunetOnlineState::Offline;
            }
            let Ok(address) = text.parse::<IpAddr>() else {
                return TunetOnlineState::Unknown;
            };
            if address.is_unspecified() {
                TunetOnlineState::Offline
            } else if address == expected {
                TunetOnlineState::Online
            } else {
                // The endpoint returned a concrete address for this stack,
                // but it is not the address being proved.  That is not an
                // offline proof for the requested address: it may be another
                // active session, another device, or a stale response.
                TunetOnlineState::Unknown
            }
        });

        if let (Some(explicit), Some(address)) = (explicit_state, address_state) {
            return if explicit == address {
                explicit
            } else {
                TunetOnlineState::Unknown
            };
        }
        if let Some(address) = address_state {
            return address;
        }

        let total_is_zero = object
            .get("online_device_total")
            .and_then(parse_online_device_total)
            == Some(0);
        if total_is_zero && !has_malformed_online_address(object) {
            match explicit_state {
                Some(TunetOnlineState::Online) => TunetOnlineState::Unknown,
                _ => TunetOnlineState::Offline,
            }
        } else {
            TunetOnlineState::Unknown
        }
    }

    /// Return whether this record proves an offline state. Positive status
    /// records use explicit state fields; the only accepted negative proof is
    /// the validated `not_online_error` shape handled below.
    pub fn is_offline_proven(&self) -> bool {
        (self.is_positive() && self.online_state() == TunetOnlineState::Offline)
            || self.is_explicit_offline_proven(None)
    }

    /// Return whether this record proves that one requested IP is offline.
    /// The proof is bound to that IP when an address field is present.
    pub fn is_offline_proven_for_ip(&self, expected_ip: &str) -> bool {
        (self.is_positive() && self.online_state_for_ip(expected_ip) == TunetOnlineState::Offline)
            || self.is_explicit_offline_proven(Some(expected_ip))
    }

    /// Return whether the response is the portal's narrow negative offline
    /// proof.  SRun deployments commonly answer a status request with
    /// `not_online_error` after logout.  That token is meaningful only when
    /// the same payload also carries a zero online-device count and a
    /// consistent empty/false online state.  An arbitrary error, a missing
    /// count, or a malformed address is never accepted here.
    fn is_explicit_offline_proven(&self, expected_ip: Option<&str>) -> bool {
        if self.signal != TunetStatusSignal::Negative {
            return false;
        }
        let Some(payload) = self.payload.as_ref() else {
            return false;
        };
        let Some(object) = payload.as_object() else {
            return false;
        };
        let Some(error) = object.get("error").and_then(Value::as_str) else {
            return false;
        };
        if !error.trim().eq_ignore_ascii_case("not_online_error") {
            return false;
        }
        if object
            .get("online_device_total")
            .and_then(parse_online_device_total)
            != Some(0)
        {
            return false;
        }
        if has_usable_online_address(object) {
            return false;
        }

        if let Some(expected_ip) = expected_ip {
            return self.online_state_for_ip(expected_ip) == TunetOnlineState::Offline;
        }

        // An account-level proof must not silently ignore an active address
        // from either stack. An explicit false state is sufficient when no
        // usable address contradicts it; otherwise both reported address
        // fields must be present and explicitly empty/unspecified.
        if object
            .get("online")
            .or_else(|| object.get("is_online"))
            .or_else(|| object.get("isOnline"))
            .or_else(|| object.get("onlineStatus"))
            .map(json_value_online_state)
            == Some(TunetOnlineState::Offline)
        {
            return !has_usable_online_address(object);
        }

        ["online_ip", "online_ip6"]
            .into_iter()
            .map(|name| object.get(name))
            .all(|value| value.is_some_and(is_empty_or_unspecified_address))
    }

    /// Return only protocol-controlled diagnostic fields.  The full JSON
    /// payload remains available to the caller through `payload`, but this
    /// accessor never returns arbitrary response text or account fields.
    pub fn protocol_code(&self) -> Option<String> {
        if let Some(payload) = self.payload.as_ref() {
            return protocol_code_from_value(payload);
        }

        if self.signal == TunetStatusSignal::Negative {
            return self
                .fields
                .first()
                .and_then(|value| safe_protocol_token(value))
                .map(|value| format!("status={value}"));
        }

        None
    }

    /// Extract the authenticated account's traffic and balance snapshot from
    /// a positive JSON/JSONP status response.  Missing fields, malformed
    /// numeric values, negative counters, and legacy comma records remain
    /// explicit protocol failures; they never become zero-valued DTOs.
    pub fn usage_snapshot(&self) -> Result<TunetUsageSnapshot, TunetClientError> {
        if !self.is_positive() {
            return if self.signal == TunetStatusSignal::Negative {
                Err(TunetClientError::PortalRejected {
                    operation: TunetOperation::Status,
                    code: self.protocol_code(),
                })
            } else {
                Err(TunetClientError::ResponseNotProven {
                    operation: TunetOperation::Status,
                    signal: self.signal,
                })
            };
        }

        let Some(payload) = self.payload.as_ref() else {
            return Err(TunetClientError::UnsupportedVariant {
                operation: TunetOperation::Status,
                variant: "traffic fields require a JSON object response".to_owned(),
            });
        };
        let Some(object) = payload.as_object() else {
            return Err(TunetClientError::UnsupportedVariant {
                operation: TunetOperation::Status,
                variant: "traffic payload is not a JSON object".to_owned(),
            });
        };

        Ok(TunetUsageSnapshot {
            used_bytes: required_nonnegative_counter(object, "sum_bytes")?,
            used_seconds: required_nonnegative_counter(object, "sum_seconds")?,
            remaining_bytes: required_nonnegative_counter(object, "remain_bytes")?,
            remaining_seconds: required_nonnegative_counter(object, "remain_seconds")?,
            account_balance: required_numeric_text(object, "user_balance")?,
        })
    }

    fn bind_to_requested_ip(&mut self, expected_ip: Option<&str>) {
        self.bound_online_state = expected_ip.map(|ip| self.online_state_for_ip(ip));
    }
}

/// The challenge token returned by `get_challenge`.
///
/// The token is needed by the next login step, so it remains available through
/// [`TunetChallenge::token`].  Its `Debug` implementation redacts the value so
/// accidental diagnostics do not print challenge material.
#[derive(Clone, PartialEq, Eq)]
pub struct TunetChallenge {
    pub token: String,
    pub callback: Option<String>,
}

impl TunetChallenge {
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl std::fmt::Debug for TunetChallenge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TunetChallenge")
            .field("token", &"[redacted]")
            .field("callback", &self.callback)
            .finish()
    }
}

/// A configured TUNet client.
#[derive(Clone)]
pub struct TunetClient {
    config: TunetClientConfig,
    transport: CampusHttpTransport,
}

async fn poll_status_with<F, Fut, S, SleepFut>(
    fetch: F,
    sleep: S,
    expectation: StatusExpectation,
) -> Result<TunetStatusRecord, TunetClientError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<TunetStatusRecord, TunetClientError>>,
    S: FnMut(Duration) -> SleepFut,
    SleepFut: Future<Output = ()>,
{
    poll_status_with_ip(fetch, sleep, expectation, None).await
}

async fn poll_status_with_ip<F, Fut, S, SleepFut>(
    mut fetch: F,
    mut sleep: S,
    expectation: StatusExpectation,
    expected_ip: Option<&str>,
) -> Result<TunetStatusRecord, TunetClientError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<TunetStatusRecord, TunetClientError>>,
    S: FnMut(Duration) -> SleepFut,
    SleepFut: Future<Output = ()>,
{
    for attempt in 0..STATUS_CONFIRM_MAX_ATTEMPTS {
        let record = match fetch().await {
            Ok(record) => record,
            Err(error) if error.is_temporary_network() => {
                if attempt + 1 == STATUS_CONFIRM_MAX_ATTEMPTS {
                    return Err(error);
                }
                sleep(STATUS_CONFIRM_INTERVAL).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        match classify_status_for_poll_for_ip(&record, expectation, expected_ip) {
            StatusPollDecision::Confirmed => return Ok(record),
            StatusPollDecision::Rejected => {
                return Err(TunetClientError::PortalRejected {
                    operation: TunetOperation::Status,
                    code: record.protocol_code(),
                });
            }
            StatusPollDecision::WrongState => {
                return Err(TunetClientError::OnlineStateUnproven);
            }
            StatusPollDecision::Retry => {
                if attempt + 1 == STATUS_CONFIRM_MAX_ATTEMPTS {
                    return Err(TunetClientError::OnlineStateUnproven);
                }
                sleep(STATUS_CONFIRM_INTERVAL).await;
            }
        }
    }

    Err(TunetClientError::OnlineStateUnproven)
}

impl TunetClient {
    pub fn new(config: TunetClientConfig) -> Result<Self, TunetClientError> {
        validate_client_profile(&config.profile)?;
        let transport = CampusHttpTransport::new(&config.user_agent)?;
        Ok(Self { config, transport })
    }

    pub fn from_profile(profile: TunetProfile) -> Result<Self, TunetClientError> {
        Self::new(TunetClientConfig::new(profile)?)
    }

    /// Build a client for the current IPv4 or IPv6 portal profile.
    pub fn current(auth: AuthFamily) -> Result<Self, TunetClientError> {
        Self::new(TunetClientConfig::current(auth)?)
    }

    /// Build a client for the current IPv4 portal profile.
    pub fn auth4() -> Result<Self, TunetClientError> {
        Self::current(AuthFamily::Auth4)
    }

    /// Build a client for the current IPv6 portal profile.
    pub fn auth6() -> Result<Self, TunetClientError> {
        Self::current(AuthFamily::Auth6)
    }

    /// Construct a client around an already configured cookie-aware transport.
    pub fn with_transport(
        config: TunetClientConfig,
        transport: CampusHttpTransport,
    ) -> Result<Self, TunetClientError> {
        validate_client_profile(&config.profile)?;
        Ok(Self { config, transport })
    }

    pub fn config(&self) -> &TunetClientConfig {
        &self.config
    }

    pub fn profile(&self) -> &TunetProfile {
        self.config.profile()
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Plan an auth4/auth6 challenge request.
    ///
    /// `username` is added by the client.  `extra_params` supplies values such
    /// as `ip` or `ac_id` when the selected profile marks them as
    /// `FromRequest`; fixed profile values are injected by `TunetProfile`.
    pub fn plan_challenge(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetRequestPlan, TunetClientError> {
        self.require_username(username)?;
        let params = prepend_parameter("username", username, extra_params);
        self.build_request(TunetOperation::Challenge, &params)
    }

    /// Plan a `rad_user_info` status request.
    pub fn plan_status(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetRequestPlan, TunetClientError> {
        self.build_request(TunetOperation::Status, extra_params)
    }

    /// Plan a portal logout request.
    pub fn plan_logout(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetRequestPlan, TunetClientError> {
        self.require_username(username)?;
        let mut params = vec![("action", "logout")];
        params.push(("username", username));
        params.extend(extra_params.iter().copied());
        self.build_request(TunetOperation::Logout, &params)
    }

    /// Plan a portal login request using a caller-provided wire password.
    ///
    /// This method never derives, hashes, encrypts, or rewrites `password`.
    /// The configured digest scheme must be explicit and must match the
    /// caller-provided scheme; otherwise [`TunetClientError::UnsupportedDigest`]
    /// is returned.
    pub fn plan_login(
        &self,
        username: &str,
        password: &OuterDigest,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetRequestPlan, TunetClientError> {
        self.require_username(username)?;
        self.validate_digest(password)?;

        let mut params = vec![("action", "login")];
        params.extend(extra_params.iter().copied());
        Ok(self
            .config
            .profile
            .build_login_request(username, password, &params)?)
    }

    /// Plan a login using the current Tsinghua portal's `srun_bx1` profile.
    ///
    /// The plaintext password and challenge token are consumed inside Rust.
    /// The returned request contains only the derived `{MD5}`, `{SRBX1}`, and
    /// `chksum` fields. The current profile is explicit: a generic or
    /// unresolved digest candidate cannot be used by this method.
    pub fn plan_srun_login(
        &self,
        username: &str,
        password: &str,
        challenge_token: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetRequestPlan, TunetClientError> {
        self.require_username(username)?;
        if password.is_empty() {
            return Err(TunetClientError::SrunLogin(
                crate::tunet_auth::SrunLoginError::EmptyField { field: "password" },
            ));
        }

        let configured = self.profile().outer_digest;
        let scheme = match configured {
            OuterDigestScheme::CandidateHmacMd5Hex => SrunPasswordDigestScheme::PortalHmacMd5,
            _ => {
                return Err(TunetClientError::UnsupportedSrunDigest { configured });
            }
        };
        let ac_id = self.ac_id_parameter(extra_params)?;
        let ip = self.ip_parameter(extra_params)?;
        let material =
            build_srun_login_material(scheme, username, password, challenge_token, &ac_id, &ip)?;

        let mut params = vec![
            ("action", "login"),
            ("username", username),
            ("password", material.wire_password()),
            // These are part of the current desktop Portal.js request shape.
            // They are transport metadata only and are intentionally absent
            // from the checksum input assembled by `build_srun_login_material`.
            ("os", "Mac OS"),
            ("name", "Macintosh"),
            ("info", material.info()),
            ("chksum", material.checksum()),
            ("n", srun_request_n()),
            ("type", srun_request_type()),
            // The current desktop Portal.js sends this flag even when the
            // portal has double-stack authentication disabled.  It is part
            // of the request shape, but it is deliberately excluded from
            // the SRun checksum because the public JS checksum string does
            // not include it.
            ("double_stack", "0"),
        ];
        params.extend(extra_params.iter().copied());
        self.build_request(TunetOperation::Login, &params)
    }

    /// Fetch and decode an auth4/auth6 challenge.
    pub async fn challenge(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetChallenge, TunetClientError> {
        let plan = self.plan_challenge(username, extra_params)?;
        let body = self.execute(&plan).await?;
        parse_challenge_response(
            &body,
            self.profile().callbacks.callback(TunetOperation::Challenge),
        )
    }

    /// Fetch and decode a status response.
    pub async fn status(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let plan = self.plan_status(extra_params)?;
        let body = self.execute(&plan).await?;
        let mut record = parse_status_response(
            &body,
            self.profile().callbacks.callback(TunetOperation::Status),
        )?;
        // Public runtime callers consume this record directly.  Bind its
        // account state to the same IP that was sent to `rad_user_info`, so a
        // response reporting only the other stack cannot be promoted to an
        // online DTO by an account-level helper.
        let expected_ip = self.expected_status_ip(extra_params)?;
        record.bind_to_requested_ip(expected_ip.as_deref());
        record.trace_status_diagnostic(expected_ip.as_deref());
        Ok(record)
    }

    /// Read the state observed for this HTTP request, as tunet-helper::flux
    /// does. A UDP local address can differ from the HTTP route behind NAT,
    /// a proxy or a tunnel. This observation is NOT a proof for that local
    /// address and must never authenticate/disconnect a runtime session.
    /// Explicit-IP status/login/logout APIs retain their strict bindings.
    pub async fn status_for_request_origin(&self) -> Result<TunetStatusRecord, TunetClientError> {
        let mut config = self.config.clone();
        config.profile.ip = IpPolicy::Omit;
        let observer = Self::with_transport(config, self.transport.clone())?;
        observer.status(&[]).await
    }

    /// Fetch the portal's status endpoint and decode its typed traffic and
    /// balance fields.  This reuses the same Cookie-aware request and response
    /// checks as [`TunetClient::status`]; it does not fabricate a separate
    /// balance endpoint or treat an online proof as a traffic proof.
    pub async fn usage(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetUsageSnapshot, TunetClientError> {
        let record = self.status(extra_params).await?;
        record.usage_snapshot()
    }

    /// Polls status until the response explicitly proves that the portal
    /// considers the account online. Explicit negative responses stop
    /// immediately; only an unknown state gets a bounded retry window.
    pub async fn status_verified(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let expected_ip = self.expected_status_ip(extra_params)?;
        poll_status_with_ip(
            || self.status(extra_params),
            status_poll_sleep,
            StatusExpectation::Online,
            expected_ip.as_deref(),
        )
        .await
    }

    async fn status_verified_for_account(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
        expectation: StatusExpectation,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let expected_ip = self.expected_status_ip(extra_params)?;
        poll_status_with_ip(
            || async {
                let record = self.status(extra_params).await?;
                if record.account_binding_conflicts(username) {
                    return Err(TunetClientError::AccountMismatch);
                }
                Ok(record)
            },
            status_poll_sleep,
            expectation,
            expected_ip.as_deref(),
        )
        .await
    }

    /// Execute portal logout and require a follow-up offline proof for the
    /// requested IP.  The portal CGI's `error=ok` only acknowledges that it
    /// handled the logout request; it does not prove that the RADIUS session
    /// is gone, so this operation never returns that CGI record as success.
    pub async fn logout(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        self.disconnect_verified(username, extra_params).await
    }

    /// Alias for [`TunetClient::logout`].  The returned record is the
    /// authoritative offline status, rather than the portal acknowledgement.
    pub async fn logout_verified(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        self.logout(username, extra_params).await
    }

    /// Logs out and polls until the follow-up status explicitly proves that
    /// the requested session is offline, including the portal's narrow
    /// `not_online_error` proof when its state fields are consistent.
    pub async fn disconnect_verified(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let record = self.logout_portal(username, extra_params).await?;
        require_positive(&record, TunetOperation::Logout)?;
        self.status_verified_for_account(username, extra_params, StatusExpectation::Offline)
            .await
    }

    /// Execute a portal login with a caller-computed outer digest and require
    /// an online status proof for the requested IP.
    pub async fn login(
        &self,
        username: &str,
        password: &OuterDigest,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let record = self.login_portal(username, password, extra_params).await?;
        require_positive(&record, TunetOperation::Login)?;
        self.status_verified_for_account(username, extra_params, StatusExpectation::Online)
            .await
    }

    async fn login_portal(
        &self,
        username: &str,
        password: &OuterDigest,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let plan = self.plan_login(username, password, extra_params)?;
        let body = self.execute(&plan).await?;
        parse_status_response(
            &body,
            self.profile().callbacks.callback(TunetOperation::Login),
        )
    }

    /// Obtain a challenge, derive the current SRun login material, execute
    /// the portal login, and require an online proof for the requested IP.
    pub async fn login_with_password(
        &self,
        username: &str,
        password: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let challenge = self.challenge(username, extra_params).await?;
        let plan = self.plan_srun_login(username, password, challenge.token(), extra_params)?;
        let body = self.execute(&plan).await?;
        let record = parse_status_response(
            &body,
            self.profile().callbacks.callback(TunetOperation::Login),
        )?;
        require_positive(&record, TunetOperation::Login)?;
        self.status_verified_for_account(username, extra_params, StatusExpectation::Online)
            .await
    }

    /// Performs SRun login and rejects a response without a positive portal
    /// signal.
    pub async fn login_with_password_verified(
        &self,
        username: &str,
        password: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        self.login_with_password(username, password, extra_params)
            .await
    }

    fn require_username(&self, username: &str) -> Result<(), TunetClientError> {
        if username.trim().is_empty() {
            Err(TunetClientError::EmptyUsername)
        } else {
            Ok(())
        }
    }

    fn build_request(
        &self,
        operation: TunetOperation,
        params: &TunetExtraParams<'_>,
    ) -> Result<TunetRequestPlan, TunetClientError> {
        let request = self.config.profile.build_request(operation, params)?;
        self.config.profile.validate_request(&request)?;
        Ok(request)
    }

    async fn logout_portal(
        &self,
        username: &str,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<TunetStatusRecord, TunetClientError> {
        let plan = self.plan_logout(username, extra_params)?;
        let body = self.execute(&plan).await?;
        parse_status_response(
            &body,
            self.profile().callbacks.callback(TunetOperation::Logout),
        )
    }

    fn expected_status_ip(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<Option<String>, TunetClientError> {
        let ip = self.ip_parameter(extra_params)?;
        if ip.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ip))
        }
    }

    fn ac_id_parameter(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<String, TunetClientError> {
        match &self.profile().ac_id {
            AcIdPolicy::Omit => Ok(String::new()),
            AcIdPolicy::Fixed(value) | AcIdPolicy::FixedForPortal(value) => Ok(value.clone()),
            AcIdPolicy::FromRequest | AcIdPolicy::FromRequestForPortal => extra_params
                .iter()
                .find(|(parameter, value)| *parameter == "ac_id" && !value.is_empty())
                .map(|(_, value)| (*value).to_owned())
                .ok_or(TunetClientError::Profile(ProfileError::MissingParameter(
                    "ac_id",
                ))),
        }
    }

    fn ip_parameter(
        &self,
        extra_params: &TunetExtraParams<'_>,
    ) -> Result<String, TunetClientError> {
        match &self.profile().ip {
            IpPolicy::Omit => Ok(String::new()),
            IpPolicy::Fixed { value, .. } => Ok(value.clone()),
            IpPolicy::FromRequest { .. } => extra_params
                .iter()
                .find(|(parameter, value)| *parameter == "ip" && !value.is_empty())
                .map(|(_, value)| (*value).to_owned())
                .ok_or(TunetClientError::Profile(ProfileError::MissingParameter(
                    "ip",
                ))),
        }
    }

    fn validate_digest(&self, password: &OuterDigest) -> Result<(), TunetClientError> {
        let configured = self.profile().outer_digest;
        if configured == OuterDigestScheme::Unspecified
            || password.scheme == OuterDigestScheme::Unspecified
            || configured != password.scheme
        {
            return Err(TunetClientError::UnsupportedDigest {
                configured,
                provided: password.scheme,
            });
        }

        Ok(())
    }

    async fn execute(&self, plan: &TunetRequestPlan) -> Result<String, TunetClientError> {
        self.config.profile.validate_request(plan)?;
        let response = match plan.method {
            HttpMethod::Get => self.transport.get_text_response(&plan.url).await?,
            HttpMethod::Post => {
                let request = build_post_request(&self.transport, plan)?;
                let response = self
                    .transport
                    .execute(request)
                    .await
                    .map_err(TransportError::Request)?;
                let status = response.status();
                let final_url = response.url().clone();
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body = crate::telemetry::timing::read_text(response)
                    .await
                    .map_err(TransportError::Decode)?;
                CampusTextResponse {
                    status,
                    final_url,
                    content_type,
                    redirect_location: None,
                    body,
                }
            }
        };
        validate_response_metadata(plan, &response)?;
        Ok(response.body)
    }
}

fn build_post_request(
    transport: &CampusHttpTransport,
    plan: &TunetRequestPlan,
) -> Result<reqwest::Request, TunetClientError> {
    transport
        .client()
        .post(&plan.url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(plan.body.as_deref().unwrap_or_default().to_owned())
        .build()
        .map_err(TransportError::Request)
        .map_err(TunetClientError::Transport)
}

fn validate_response_metadata(
    plan: &TunetRequestPlan,
    response: &CampusTextResponse,
) -> Result<(), TunetClientError> {
    // The current CGI endpoints return a JSON/JSONP envelope with HTTP 200.
    // A different 2xx status is not a protocol proof and must not be allowed
    // to reach the success parser as if it were the browser response.
    if response.status != reqwest::StatusCode::OK {
        return Err(TunetClientError::HttpStatus {
            status: response.status,
        });
    }
    let expected =
        Url::parse(&plan.url).map_err(|_| TunetClientError::UnexpectedResponseLocation)?;
    if response.final_url.fragment().is_some()
        || expected.scheme() != response.final_url.scheme()
        || expected.host() != response.final_url.host()
        || expected.port_or_known_default() != response.final_url.port_or_known_default()
        || !response_location_matches_operation(plan, &expected, &response.final_url)
    {
        return Err(TunetClientError::UnexpectedResponseLocation);
    }
    let Some(content_type) = response.content_type.as_deref() else {
        return Err(TunetClientError::UnsupportedContentType);
    };
    if !is_protocol_content_type(content_type) {
        return Err(TunetClientError::UnsupportedContentType);
    }
    Ok(())
}

fn response_location_matches_operation(
    plan: &TunetRequestPlan,
    expected: &Url,
    actual: &Url,
) -> bool {
    if expected.path() == actual.path()
        && expected.query() == actual.query()
        && actual.fragment().is_none()
    {
        return true;
    }

    // A login form may finish by redirecting to the portal's ordinary home
    // document. The redirect is accepted only for the login operation, on the
    // same origin, with a small set of explicit home paths and no query or
    // fragment. Challenge, status, and logout responses must remain on their
    // exact CGI route and query.
    plan.operation == TunetOperation::Login
        && expected.origin() == actual.origin()
        && actual.query().is_none()
        && actual.fragment().is_none()
        && matches!(actual.path(), "/" | "/index.html" | "/index_1.html")
}

fn is_protocol_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().map(str::trim).unwrap_or_default();
    matches!(
        media_type.to_ascii_lowercase().as_str(),
        "application/json"
            | "application/javascript"
            | "application/x-javascript"
            | "text/javascript"
            | "text/plain"
    )
}

/// Parse a challenge body that is either plain JSON or JSONP.
pub fn parse_challenge_response(
    body: &str,
    expected_callback: Option<&str>,
) -> Result<TunetChallenge, TunetClientError> {
    let (callback, payload) =
        parse_json_or_jsonp(body, expected_callback, TunetOperation::Challenge)?;
    let object = payload
        .as_object()
        .ok_or_else(|| TunetClientError::UnsupportedVariant {
            operation: TunetOperation::Challenge,
            variant: "challenge payload is not a JSON object".to_owned(),
        })?;

    let error = object
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or(TunetClientError::InvalidChallengePayload)?;
    if !matches!(error.to_ascii_lowercase().as_str(), "ok" | "success") {
        return Err(TunetClientError::ChallengeRejected {
            code: protocol_code_from_object(object),
        });
    }
    let ecode = object
        .get("ecode")
        .and_then(Value::as_i64)
        .ok_or(TunetClientError::InvalidChallengePayload)?;
    if ecode != 0 {
        return Err(TunetClientError::ChallengeRejected {
            code: protocol_code_from_object(object),
        });
    }

    let token = match object.get("challenge") {
        Some(Value::String(value))
            if !value.is_empty()
                && value.trim() == value
                && !value.chars().any(|character| character.is_control()) =>
        {
            value.to_owned()
        }
        Some(Value::String(_)) => return Err(TunetClientError::MissingChallengeToken),
        Some(_) => return Err(TunetClientError::InvalidChallengePayload),
        None => return Err(TunetClientError::MissingChallengeToken),
    };

    Ok(TunetChallenge { token, callback })
}

/// Parse either JSONP status data or a comma-delimited status record.
pub fn parse_status_response(
    body: &str,
    expected_callback: Option<&str>,
) -> Result<TunetStatusRecord, TunetClientError> {
    let trimmed = trim_response_body(body);
    if looks_like_json_value(trimmed) {
        let payload = serde_json::from_str::<Value>(trimmed).map_err(|error| {
            TunetClientError::JsonDecode {
                message: error.to_string(),
            }
        })?;
        if !payload.is_object() {
            return Err(TunetClientError::UnsupportedVariant {
                operation: TunetOperation::Status,
                variant: "status payload is not a JSON object".to_owned(),
            });
        }
        return Ok(TunetStatusRecord {
            service: Service::Network,
            encoding: TunetStatusEncoding::Json,
            callback: None,
            signal: json_status_signal(&payload),
            fields: Vec::new(),
            payload: Some(payload),
            bound_online_state: None,
        });
    }

    let response = decode_status_response(trimmed, expected_callback)?;
    match response {
        StatusResponse::Jsonp(jsonp) => {
            let payload = serde_json::from_str::<Value>(jsonp.payload).map_err(|error| {
                TunetClientError::JsonDecode {
                    message: error.to_string(),
                }
            })?;
            if !payload.is_object() {
                return Err(TunetClientError::UnsupportedVariant {
                    operation: TunetOperation::Status,
                    variant: "status payload is not a JSON object".to_owned(),
                });
            }
            let signal = json_status_signal(&payload);
            Ok(TunetStatusRecord {
                service: Service::Network,
                encoding: TunetStatusEncoding::Jsonp,
                callback: Some(jsonp.callback.to_owned()),
                signal,
                fields: Vec::new(),
                payload: Some(payload),
                bound_online_state: None,
            })
        }
        StatusResponse::Comma(comma) => Ok(TunetStatusRecord {
            service: Service::Network,
            encoding: TunetStatusEncoding::Comma,
            callback: None,
            signal: comma_signal(comma.signal()),
            fields: comma
                .fields
                .iter()
                .map(|field| (*field).to_owned())
                .collect(),
            payload: None,
            bound_online_state: None,
        }),
    }
}

fn parse_json_or_jsonp(
    body: &str,
    expected_callback: Option<&str>,
    operation: TunetOperation,
) -> Result<(Option<String>, Value), TunetClientError> {
    let trimmed = trim_response_body(body);
    if trimmed.is_empty() {
        return Err(TunetClientError::Response(ResponseDecodeError::Empty));
    }

    if looks_like_json_value(trimmed) {
        let payload = serde_json::from_str::<Value>(trimmed).map_err(|error| {
            TunetClientError::JsonDecode {
                message: error.to_string(),
            }
        })?;
        return Ok((None, payload));
    }

    let jsonp = decode_jsonp(trimmed, expected_callback)?;
    let payload = serde_json::from_str::<Value>(jsonp.payload).map_err(|error| {
        TunetClientError::JsonDecode {
            message: error.to_string(),
        }
    })?;

    if !payload.is_object() && operation == TunetOperation::Challenge {
        return Err(TunetClientError::UnsupportedVariant {
            operation,
            variant: "challenge JSONP payload is not a JSON object".to_owned(),
        });
    }

    Ok((Some(jsonp.callback.to_owned()), payload))
}

fn trim_response_body(body: &str) -> &str {
    body.strip_prefix('\u{feff}').unwrap_or(body).trim()
}

fn looks_like_json_value(body: &str) -> bool {
    let Some(first) = body.as_bytes().first().copied() else {
        return false;
    };

    match first {
        b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' => true,
        b't' => body.starts_with("true"),
        b'f' => body.starts_with("false"),
        b'n' => body.starts_with("null"),
        _ => false,
    }
}

fn json_status_signal(payload: &Value) -> TunetStatusSignal {
    let Some(object) = payload.as_object() else {
        return TunetStatusSignal::Unknown;
    };

    let ecode_signal = object.get("ecode").map(|ecode| match parse_ecode(ecode) {
        Some(0) => TunetStatusSignal::Positive,
        Some(_) => TunetStatusSignal::Negative,
        None => TunetStatusSignal::Unknown,
    });
    let error_signal = object.get("error").and_then(Value::as_str).map(|error| {
        match error.trim().to_ascii_lowercase().as_str() {
            "ok" | "success" => TunetStatusSignal::Positive,
            // The portal uses deployment-specific error names such as
            // `not_online_error` and `challenge_expire_error`.  Once an explicit
            // non-empty error is present it is a negative protocol result; an
            // absent/non-string error remains Unknown below.
            value if !value.is_empty() => TunetStatusSignal::Negative,
            _ => TunetStatusSignal::Unknown,
        }
    });

    if error_signal == Some(TunetStatusSignal::Negative)
        || ecode_signal == Some(TunetStatusSignal::Negative)
    {
        TunetStatusSignal::Negative
    } else if ecode_signal == Some(TunetStatusSignal::Unknown) {
        TunetStatusSignal::Unknown
    } else {
        error_signal.unwrap_or(TunetStatusSignal::Unknown)
    }
}

fn json_online_state(payload: &Value) -> TunetOnlineState {
    let Some(object) = payload.as_object() else {
        return TunetOnlineState::Unknown;
    };

    let explicit_state = ["online", "is_online", "isOnline", "onlineStatus"]
        .into_iter()
        .filter_map(|name| object.get(name))
        .map(json_value_online_state)
        .find(|state| *state != TunetOnlineState::Unknown);

    let has_usable_ip = has_usable_online_address(object);
    let has_malformed_ip = has_malformed_online_address(object);
    let total = object
        .get("online_device_total")
        .and_then(parse_online_device_total);

    if has_malformed_ip {
        return TunetOnlineState::Unknown;
    }

    if let Some(explicit) = explicit_state {
        // A concrete address or a zero device count contradicts a boolean
        // state. Preserve an explicit state only when no wire field disputes
        // it; this makes conflicting portal changes fail closed.
        if (explicit == TunetOnlineState::Online
            && (total == Some(0) || !has_usable_ip && has_empty_address_field(object)))
            || (explicit == TunetOnlineState::Offline
                && (has_usable_ip || total.is_some_and(|value| value > 0)))
        {
            return TunetOnlineState::Unknown;
        }
        return explicit;
    }

    // The current auth4/auth6 `rad_user_info` response does not include an
    // `online` boolean. Its positive proof is an explicit usable (parseable,
    // non-unspecified) `online_ip` or `online_ip6`; an explicit zero
    // `online_device_total` proves offline. Do not infer either state from
    // HTTP status or from the presence of unrelated account fields.
    if has_usable_ip {
        return TunetOnlineState::Online;
    }

    if total == Some(0) && has_empty_address_field(object) {
        return TunetOnlineState::Offline;
    }

    TunetOnlineState::Unknown
}

fn json_value_online_state(value: &Value) -> TunetOnlineState {
    match value {
        Value::Bool(true) => TunetOnlineState::Online,
        Value::Bool(false) => TunetOnlineState::Offline,
        Value::Number(number) if number.as_i64() == Some(1) => TunetOnlineState::Online,
        Value::Number(number) if number.as_i64() == Some(0) => TunetOnlineState::Offline,
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "online" | "on" | "已在线" => TunetOnlineState::Online,
            "0" | "false" | "offline" | "off" | "未在线" => TunetOnlineState::Offline,
            _ => TunetOnlineState::Unknown,
        },
        _ => TunetOnlineState::Unknown,
    }
}

fn parse_online_device_total(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
}

fn required_nonnegative_counter(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<u64, TunetClientError> {
    let Some(value) = object.get(field) else {
        return Err(TunetClientError::UnsupportedVariant {
            operation: TunetOperation::Status,
            variant: format!("traffic field {field} is missing"),
        });
    };

    let parsed = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value.as_str().and_then(|text| {
                let text = text.trim();
                (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                    .then(|| text.parse::<u64>().ok())
                    .flatten()
            })
        });
    parsed.ok_or_else(|| TunetClientError::UnsupportedVariant {
        operation: TunetOperation::Status,
        variant: format!("traffic field {field} is not a non-negative integer"),
    })
}

fn required_numeric_text(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, TunetClientError> {
    let Some(value) = object.get(field) else {
        return Err(TunetClientError::UnsupportedVariant {
            operation: TunetOperation::Status,
            variant: format!("traffic field {field} is missing"),
        });
    };

    let text = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.trim().to_owned(),
        _ => String::new(),
    };
    if text.is_empty() || !is_decimal_text(&text) {
        return Err(TunetClientError::UnsupportedVariant {
            operation: TunetOperation::Status,
            variant: format!("traffic field {field} is not numeric"),
        });
    }
    Ok(text)
}

fn is_decimal_text(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    let mut dots = 0;
    let mut digits = 0;
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' => digits += 1,
            b'.' => dots += 1,
            _ => return false,
        }
    }
    digits > 0 && dots <= 1
}

fn parse_ecode(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<i64>().ok())
    })
}

fn is_usable_online_ip(value: &str) -> bool {
    value
        .trim()
        .parse::<IpAddr>()
        .map(|address| !address.is_unspecified())
        .unwrap_or(false)
}

fn is_usable_address(value: &Value) -> bool {
    value.as_str().is_some_and(is_usable_online_ip)
}

fn is_empty_or_unspecified_address(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    let text = text.trim();
    text.is_empty()
        || text
            .parse::<IpAddr>()
            .map(|address| address.is_unspecified())
            .unwrap_or(false)
}

fn has_usable_online_address(object: &Map<String, Value>) -> bool {
    ["online_ip", "online_ip6"]
        .into_iter()
        .filter_map(|name| object.get(name))
        .any(is_usable_address)
}

fn has_malformed_online_address(object: &Map<String, Value>) -> bool {
    ["online_ip", "online_ip6"].into_iter().any(|name| {
        object.get(name).is_some_and(|value| {
            !is_empty_or_unspecified_address(value) && !is_usable_address(value)
        })
    })
}

fn has_empty_address_field(object: &Map<String, Value>) -> bool {
    ["online_ip", "online_ip6"]
        .into_iter()
        .filter_map(|name| object.get(name))
        .any(is_empty_or_unspecified_address)
}

fn protocol_code_from_value(payload: &Value) -> Option<String> {
    payload.as_object().and_then(protocol_code_from_object)
}

fn protocol_code_from_object(object: &Map<String, Value>) -> Option<String> {
    let mut parts = Vec::with_capacity(2);

    if let Some(error) = object.get("error").and_then(Value::as_str)
        && let Some(error) = safe_protocol_token(error)
    {
        parts.push(format!("error={error}"));
    }

    if let Some(ecode) = object.get("ecode")
        && let Some(ecode) = protocol_ecode_text(ecode)
    {
        parts.push(format!("ecode={ecode}"));
    }

    (!parts.is_empty()).then(|| parts.join(","))
}

fn protocol_ecode_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .map(|value| value.to_string())
            .or_else(|| number.as_u64().map(|value| value.to_string())),
        Value::String(value) => safe_protocol_token(value),
        _ => None,
    }
}

fn safe_protocol_token(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return None;
    }
    Some(value.to_owned())
}

fn protocol_code_is_authentication_failure(code: Option<&str>) -> bool {
    let Some(code) = code else {
        return false;
    };
    let code = code.to_ascii_lowercase();
    ["auth", "login", "password", "username", "credential"]
        .iter()
        .any(|marker| code.contains(marker))
}

fn require_positive(
    record: &TunetStatusRecord,
    operation: TunetOperation,
) -> Result<(), TunetClientError> {
    if record.is_positive() {
        Ok(())
    } else if record.signal == TunetStatusSignal::Negative {
        Err(TunetClientError::PortalRejected {
            operation,
            code: record.protocol_code(),
        })
    } else {
        Err(TunetClientError::ResponseNotProven {
            operation,
            signal: record.signal,
        })
    }
}

fn comma_signal(signal: CommaStatusSignal) -> TunetStatusSignal {
    match signal {
        CommaStatusSignal::Positive => TunetStatusSignal::Positive,
        CommaStatusSignal::Negative => TunetStatusSignal::Negative,
        CommaStatusSignal::Unknown => TunetStatusSignal::Unknown,
    }
}

fn prepend_parameter<'a>(
    name: &'a str,
    value: &'a str,
    extra_params: &'a TunetExtraParams<'a>,
) -> Vec<(&'a str, &'a str)> {
    let mut params = Vec::with_capacity(extra_params.len() + 1);
    params.push((name, value));
    params.extend(extra_params.iter().copied());
    params
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn backend_repair_sep19_request_origin_observer_omits_ip_without_weakening_bound_status()
    {
        use crate::reference_test_support::{FixtureServer, Reply};
        let server = FixtureServer::new(vec![
            Reply::json(r#"thyouStatus({"error":"ok","online_ip":"192.0.2.20","online":true});"#),
            Reply::json(r#"thyouStatus({"error":"ok","online_ip":"192.0.2.20","online":true});"#),
        ]);
        let port = Url::parse(server.base()).unwrap().port().unwrap();
        let profile = TunetProfile::current_with_overrides(
            AuthFamily::Auth4,
            TunetProfileOverrides {
                endpoint: Some(HttpsEndpoint::http("127.0.0.1", port).unwrap()),
                ..Default::default()
            },
        )
        .unwrap();
        let client = TunetClient::from_profile(profile).unwrap();
        let observed = client.status_for_request_origin().await.unwrap();
        assert!(observed.is_online_proven());
        assert!(!observed.is_online_proven_for_ip("192.0.2.10"));
        let bound = client.status(&[("ip", "192.0.2.10")]).await.unwrap();
        assert!(!bound.is_online_proven());
        assert!(!bound.is_offline_proven());
        assert_eq!(
            bound.unproven_reason_for_ip("192.0.2.10"),
            "tunet_address_mismatch"
        );
        assert!(matches!(client.profile().ip, IpPolicy::FromRequest { .. }));
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let route = requests[0]
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = Url::parse(server.base()).unwrap().join(route).unwrap();
        assert_eq!(url.path(), "/cgi-bin/rad_user_info");
        assert!(
            !url.query_pairs()
                .any(|(key, _)| key == "ip" || key == "username" || key == "password")
        );
        assert!(
            requests[1]
                .lines()
                .next()
                .unwrap()
                .contains("ip=192.0.2.10")
        );
    }
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::tunet::{
        AcIdPolicy, AuthFamily, CallbackProfile, HttpsEndpoint, IpFamily, IpPolicy, MethodProfile,
        TunetPaths, TunetProfileOverrides,
    };

    const CHALLENGE_JSON_FIXTURE: &str =
        r#"{"challenge":"fixture-challenge-token","ecode":0,"error":"ok"}"#;
    const CHALLENGE_JSONP_FIXTURE: &str =
        r#"thyouChallenge({"challenge":"fixture-challenge-token","ecode":0,"error":"ok"});"#;
    const STATUS_JSONP_FIXTURE: &str =
        r#"thyouStatus({"error":"ok","online":true,"user":"fixture-user"});"#;
    const STATUS_COMMA_FIXTURE: &str = "ok,fixture-user,192.0.2.10,";

    fn paths() -> TunetPaths {
        TunetPaths::new(
            "/cgi-bin/get_challenge",
            "/cgi-bin/srun_portal",
            "/cgi-bin/rad_user_info",
        )
        .expect("fixture paths are valid")
    }

    fn auth4_profile() -> TunetProfile {
        TunetProfile::current_auth4().expect("current auth4 profile is valid")
    }

    fn auth6_post_profile() -> TunetProfile {
        TunetProfile::current_with_overrides(
            AuthFamily::Auth6,
            TunetProfileOverrides {
                methods: Some(MethodProfile::new(
                    HttpMethod::Get,
                    HttpMethod::Post,
                    HttpMethod::Post,
                    HttpMethod::Post,
                )),
                callbacks: Some(
                    CallbackProfile::new(None, Some("thyouPortal"), Some("thyouStatus"))
                        .expect("fixture callbacks are valid"),
                ),
                ip: Some(
                    IpPolicy::fixed(IpFamily::V6, "2001:db8::10").expect("fixture IP is valid"),
                ),
                ..TunetProfileOverrides::default()
            },
        )
        .expect("overridden auth6 profile is valid")
    }

    fn status_record(body: &str) -> TunetStatusRecord {
        parse_status_response(body, None).expect("status fixture should parse")
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fixture read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).expect("fixture request");
            assert!(read > 0, "fixture request ended before headers");
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn write_http_response(stream: &mut TcpStream, headers: &str, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("fixture response");
    }

    #[test]
    fn current_client_config_entry_points_use_the_matching_ip_family() {
        let auth4 = TunetClientConfig::auth4().expect("auth4 config builds");
        assert_eq!(auth4.profile().auth, AuthFamily::Auth4);
        assert_eq!(auth4.profile().endpoint.host, "auth4.tsinghua.edu.cn");
        assert_eq!(auth4.user_agent(), "TsinghuaKit/0.2.0-alpha.1");

        let auth6 = TunetClientConfig::auth6().expect("auth6 config builds");
        assert_eq!(auth6.profile().auth, AuthFamily::Auth6);
        assert_eq!(auth6.profile().endpoint.host, "auth6.tsinghua.edu.cn");

        let auth4_client = TunetClient::auth4().expect("auth4 client builds");
        assert_eq!(auth4_client.profile().auth, AuthFamily::Auth4);
        let auth6_client = TunetClient::auth6().expect("auth6 client builds");
        assert_eq!(auth6_client.profile().auth, AuthFamily::Auth6);
    }

    #[test]
    fn current_auth6_request_plans_use_get_and_allow_an_ip_override() {
        let client = TunetClient::auth6().expect("current auth6 client builds");
        let extra = [("ip", "2001:db8::10")];

        let challenge = client
            .plan_challenge("fixture-user", &extra)
            .expect("auth6 challenge plan builds");
        assert_eq!(challenge.method, HttpMethod::Get);
        assert_eq!(challenge.body, None);
        assert_eq!(
            challenge.url,
            "https://auth6.tsinghua.edu.cn/cgi-bin/get_challenge?username=fixture-user&ip=2001%3Adb8%3A%3A10&callback=thyouChallenge"
        );

        let status = client
            .plan_status(&extra)
            .expect("auth6 status plan builds");
        assert_eq!(status.method, HttpMethod::Get);
        assert_eq!(
            status.url,
            "https://auth6.tsinghua.edu.cn/cgi-bin/rad_user_info?ip=2001%3Adb8%3A%3A10&callback=thyouStatus"
        );

        let logout = client
            .plan_logout("fixture-user", &extra)
            .expect("auth6 logout plan builds");
        assert_eq!(logout.method, HttpMethod::Get);
        assert_eq!(
            logout.url,
            "https://auth6.tsinghua.edu.cn/cgi-bin/srun_portal?action=logout&username=fixture-user&ip=2001%3Adb8%3A%3A10&ac_id=1&callback=thyouPortal"
        );
    }

    #[test]
    fn safe_client_configuration_rejects_unresolved_digest_profiles() {
        for scheme in [
            OuterDigestScheme::Unspecified,
            OuterDigestScheme::CandidatePlaintext,
            OuterDigestScheme::CandidateMd5Hex,
        ] {
            let profile = TunetProfile::new(
                AuthFamily::Auth4,
                HttpsEndpoint::new("auth4.tsinghua.edu.cn", 443)
                    .expect("fixture endpoint is valid"),
                paths(),
                MethodProfile::new(
                    HttpMethod::Get,
                    HttpMethod::Get,
                    HttpMethod::Get,
                    HttpMethod::Get,
                ),
                CallbackProfile::new(None, None, None).expect("fixture callbacks are valid"),
                AcIdPolicy::fixed("1").expect("fixture ac_id is valid"),
                IpPolicy::from_request(IpFamily::V4),
                scheme,
            )
            .expect("metadata profile itself is valid");

            assert!(matches!(
                TunetClientConfig::new(profile),
                Err(TunetClientError::UnsupportedSrunDigest { configured })
                    if configured == scheme
            ));
        }
    }

    #[test]
    fn challenge_json_and_jsonp_fixtures_yield_the_same_token() {
        let json = parse_challenge_response(CHALLENGE_JSON_FIXTURE, None)
            .expect("plain challenge JSON should parse");
        let jsonp = parse_challenge_response(CHALLENGE_JSONP_FIXTURE, Some("thyouChallenge"))
            .expect("challenge JSONP should parse");

        assert_eq!(json.token(), "fixture-challenge-token");
        assert_eq!(jsonp.token(), json.token());
        assert_eq!(json.callback, None);
        assert_eq!(jsonp.callback.as_deref(), Some("thyouChallenge"));
        assert!(!format!("{json:?}").contains("fixture-challenge-token"));
    }

    #[test]
    fn challenge_requires_the_portals_explicit_success_signal() {
        let error = parse_challenge_response(
            r#"thyouChallenge({"challenge":"token","ecode":0,"error":"challenge_expire_error"})"#,
            Some("thyouChallenge"),
        )
        .expect_err("a challenge error must not yield usable login material");
        assert!(matches!(
            error,
            TunetClientError::ChallengeRejected { code: Some(code) }
                if code == "error=challenge_expire_error,ecode=0"
        ));

        let error = parse_challenge_response(
            r#"thyouChallenge({"challenge":"token"})"#,
            Some("thyouChallenge"),
        )
        .expect_err("a challenge without an error field is not a proof");
        assert!(matches!(error, TunetClientError::InvalidChallengePayload));

        let error = parse_challenge_response(
            r#"thyouChallenge({"challenge":"token","error":"ok"})"#,
            Some("thyouChallenge"),
        )
        .expect_err("a challenge without an explicit ecode is not a proof");
        assert!(matches!(error, TunetClientError::InvalidChallengePayload));

        for body in [
            r#"{"challenge":" token ","ecode":0,"error":"ok"}"#,
            r#"{"challenge":"token\n","ecode":0,"error":"ok"}"#,
        ] {
            let error = parse_challenge_response(body, None)
                .expect_err("challenge material must not contain whitespace");
            assert!(matches!(error, TunetClientError::MissingChallengeToken));
        }

        for body in [
            r#"{"challenge":"token","ecode":"0","error":"ok"}"#,
            r#"{"challenge":"token","ecode":0.0,"error":"ok"}"#,
        ] {
            let error = parse_challenge_response(body, None)
                .expect_err("non-integer ecode must not produce a challenge");
            assert!(matches!(error, TunetClientError::InvalidChallengePayload));
        }
    }

    #[test]
    fn response_shape_detection_preserves_json_errors_and_accepts_bom() {
        let challenge = parse_challenge_response(
            "\u{feff}{\"challenge\":\"fixture-token\",\"ecode\":0,\"error\":\"ok\"}",
            None,
        )
        .expect("BOM-prefixed JSON challenge should parse");
        assert_eq!(challenge.callback, None);

        let malformed_json = parse_status_response("{\"error\":", None)
            .expect_err("an object-looking malformed body must remain a JSON error");
        assert!(matches!(
            malformed_json,
            TunetClientError::JsonDecode { .. }
        ));

        let malformed_jsonp = parse_status_response("thyouStatus({\"error\":});", None)
            .expect_err("a JSONP payload must be decoded as JSON after envelope parsing");
        assert!(matches!(
            malformed_jsonp,
            TunetClientError::JsonDecode { .. }
        ));

        let html = parse_status_response("<html><body>login</body></html>", None)
            .expect_err("HTML must not be accepted as a comma status record");
        assert!(matches!(
            html,
            TunetClientError::Response(ResponseDecodeError::NotCommaStatus)
        ));
    }

    #[test]
    fn current_rad_user_info_fields_prove_online_and_offline_states() {
        let online = parse_status_response(
            r#"thyouStatus({"error":"ok","online_ip":"192.0.2.10","online_ip6":"","online_device_total":"1"})"#,
            Some("thyouStatus"),
        )
        .expect("current online response");
        assert_eq!(online.online_state(), TunetOnlineState::Online);
        assert!(online.is_online_proven());

        let offline = parse_status_response(
            r#"thyouStatus({"error":"ok","online_ip":"","online_ip6":"","online_device_total":"0"})"#,
            Some("thyouStatus"),
        )
        .expect("current offline response");
        assert_eq!(offline.online_state(), TunetOnlineState::Offline);
        assert!(!offline.is_online_proven());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_polling_retries_unknown_until_online_proof() {
        let mut records = VecDeque::from([
            Ok(status_record(r#"{"error":"ok"}"#)),
            Ok(status_record(
                r#"{"error":"ok","online_ip":"203.0.113.10"}"#,
            )),
        ]);
        let mut attempts = 0;
        let mut sleeps = 0;

        let result = poll_status_with(
            || {
                attempts += 1;
                let record = records.pop_front().expect("status fixture remains");
                async move { record }
            },
            |_| {
                sleeps += 1;
                async {}
            },
            StatusExpectation::Online,
        )
        .await
        .expect("the second status response proves online");

        assert!(result.is_online_proven());
        assert_eq!(attempts, 2);
        assert_eq!(sleeps, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_polling_stops_at_the_attempt_limit_for_unknown_state() {
        let mut records = VecDeque::from_iter(
            (0..STATUS_CONFIRM_MAX_ATTEMPTS).map(|_| Ok(status_record(r#"{"error":"ok"}"#))),
        );
        let mut attempts = 0;
        let mut sleeps = 0;

        let error = poll_status_with(
            || {
                attempts += 1;
                let record = records.pop_front().expect("status fixture remains");
                async move { record }
            },
            |_| {
                sleeps += 1;
                async {}
            },
            StatusExpectation::Online,
        )
        .await
        .expect_err("unknown status must not become an online proof");

        assert!(matches!(error, TunetClientError::OnlineStateUnproven));
        assert_eq!(attempts, STATUS_CONFIRM_MAX_ATTEMPTS);
        assert_eq!(sleeps, STATUS_CONFIRM_MAX_ATTEMPTS - 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_polling_retries_temporary_http_failures_within_the_bound() {
        let mut responses = VecDeque::from([
            Err(TunetClientError::HttpStatus {
                status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            }),
            Ok(status_record(
                r#"{"error":"ok","online_ip":"203.0.113.10"}"#,
            )),
        ]);
        let mut attempts = 0;
        let mut sleeps = 0;

        let result = poll_status_with(
            || {
                attempts += 1;
                let response = responses.pop_front().expect("status fixture remains");
                async move { response }
            },
            |_| {
                sleeps += 1;
                async {}
            },
            StatusExpectation::Online,
        )
        .await
        .expect("a later online proof should complete the bounded retry");

        assert!(result.is_online_proven());
        assert_eq!(attempts, 2);
        assert_eq!(sleeps, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_polling_does_not_retry_protocol_decode_failures() {
        let mut responses = VecDeque::from([
            Err(TunetClientError::Response(ResponseDecodeError::Empty)),
            Ok(status_record(
                r#"{"error":"ok","online_ip":"203.0.113.10"}"#,
            )),
        ]);
        let mut attempts = 0;
        let mut sleeps = 0;

        let error = poll_status_with(
            || {
                attempts += 1;
                let response = responses.pop_front().expect("status fixture remains");
                async move { response }
            },
            |_| {
                sleeps += 1;
                async {}
            },
            StatusExpectation::Online,
        )
        .await
        .expect_err("a malformed response must stop the poll");

        assert!(matches!(
            error,
            TunetClientError::Response(ResponseDecodeError::Empty)
        ));
        assert_eq!(attempts, 1);
        assert_eq!(sleeps, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_polling_returns_explicit_negative_response_without_retrying() {
        let mut records = VecDeque::from([
            Ok(status_record(r#"{"ecode":1,"error":"auth_error"}"#)),
            Ok(status_record(
                r#"{"error":"ok","online_ip":"203.0.113.10"}"#,
            )),
        ]);
        let mut attempts = 0;
        let mut sleeps = 0;

        let error = poll_status_with(
            || {
                attempts += 1;
                let record = records.pop_front().expect("status fixture remains");
                async move { record }
            },
            |_| {
                sleeps += 1;
                async {}
            },
            StatusExpectation::Online,
        )
        .await
        .expect_err("an explicit negative status must stop immediately");

        assert!(matches!(
            error,
            TunetClientError::PortalRejected {
                operation: TunetOperation::Status,
                code: Some(code),
            } if code == "error=auth_error,ecode=1"
        ));
        assert_eq!(attempts, 1);
        assert_eq!(sleeps, 0);
        assert_eq!(records.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn offline_polling_retries_unknown_until_explicit_offline_proof() {
        let mut records = VecDeque::from([
            Ok(status_record(r#"{"error":"ok"}"#)),
            Ok(status_record(
                r#"{"error":"ok","online_ip":"","online_ip6":"","online_device_total":"0"}"#,
            )),
        ]);
        let mut attempts = 0;
        let mut sleeps = 0;

        let result = poll_status_with(
            || {
                attempts += 1;
                let record = records.pop_front().expect("status fixture remains");
                async move { record }
            },
            |_| {
                sleeps += 1;
                async {}
            },
            StatusExpectation::Offline,
        )
        .await
        .expect("the second status response proves offline");

        assert_eq!(result.online_state(), TunetOnlineState::Offline);
        assert_eq!(attempts, 2);
        assert_eq!(sleeps, 1);
    }

    #[test]
    fn not_online_error_is_an_offline_proof_only_with_consistent_state_fields() {
        let record = parse_status_response(
            r#"{"error":"not_online_error","online_ip":"","online_ip6":"","online_device_total":"0"}"#,
            None,
        )
        .expect("negative offline fixture");
        assert_eq!(record.signal, TunetStatusSignal::Negative);
        assert_eq!(record.online_state(), TunetOnlineState::Offline);
        assert!(record.is_offline_proven());
        assert!(record.is_offline_proven_for_ip("192.0.2.10"));

        for body in [
            r#"{"error":"not_online_error","online_ip":"","online_ip6":""}"#,
            r#"{"error":"not_online_error","online_ip":"not-an-ip","online_ip6":"","online_device_total":"0"}"#,
            r#"{"error":"not_online_error","online_ip":"","online_ip6":"2001:db8::10","online_device_total":"0"}"#,
            r#"{"error":"other_error","online_ip":"","online_ip6":"","online_device_total":"0"}"#,
        ] {
            let record = parse_status_response(body, None).expect("status fixture");
            assert!(!record.is_offline_proven());
            assert!(!record.is_offline_proven_for_ip("192.0.2.10"));
        }
    }

    #[test]
    fn contradictory_status_fields_are_unknown_and_cannot_prove_state() {
        let record = parse_status_response(
            r#"{"error":"ok","online":false,"online_ip":"192.0.2.10","online_device_total":"1"}"#,
            None,
        )
        .expect("contradictory status fixture");
        assert_eq!(record.online_state(), TunetOnlineState::Unknown);
        assert_eq!(
            record.online_state_for_ip("192.0.2.10"),
            TunetOnlineState::Unknown
        );
        assert!(!record.is_online_proven());
        assert!(!record.is_offline_proven_for_ip("192.0.2.10"));

        let malformed_offline = parse_status_response(
            r#"{"error":"ok","online_device_total":"0","online_ip":false}"#,
            None,
        )
        .expect("malformed status fixture");
        assert_eq!(
            malformed_offline.online_state_for_ip("192.0.2.10"),
            TunetOnlineState::Unknown
        );
        assert!(!malformed_offline.is_offline_proven_for_ip("192.0.2.10"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn offline_polling_accepts_the_narrow_negative_portal_proof() {
        let mut records = VecDeque::from([Ok(status_record(
            r#"{"error":"not_online_error","online_ip":"","online_ip6":"","online_device_total":"0"}"#,
        ))]);
        let mut attempts = 0;
        let result = poll_status_with(
            || {
                attempts += 1;
                let record = records.pop_front().expect("negative offline record");
                async move { record }
            },
            |_| async {},
            StatusExpectation::Offline,
        )
        .await
        .expect("not_online_error with explicit offline fields proves offline");

        assert!(result.is_offline_proven());
        assert_eq!(attempts, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn offline_polling_rejects_a_negative_error_without_offline_proof() {
        let mut records = VecDeque::from([Ok(status_record(
            r#"{"error":"not_online_error","online_ip":"","online_ip6":""}"#,
        ))]);
        let error = poll_status_with(
            || {
                let record = records.pop_front().expect("negative record");
                async move { record }
            },
            |_| async {},
            StatusExpectation::Offline,
        )
        .await
        .expect_err("missing zero count must not prove offline");

        assert!(matches!(
            error,
            TunetClientError::PortalRejected {
                operation: TunetOperation::Status,
                ..
            }
        ));
    }

    #[test]
    fn status_jsonp_and_comma_fixtures_become_stable_records() {
        let jsonp = parse_status_response(STATUS_JSONP_FIXTURE, Some("thyouStatus"))
            .expect("status JSONP should parse");
        assert_eq!(jsonp.service, Service::Network);
        assert_eq!(jsonp.encoding, TunetStatusEncoding::Jsonp);
        assert_eq!(jsonp.callback.as_deref(), Some("thyouStatus"));
        assert_eq!(jsonp.signal, TunetStatusSignal::Positive);
        assert!(jsonp.fields.is_empty());
        assert_eq!(
            jsonp
                .payload
                .as_ref()
                .and_then(|value| value["online"].as_bool()),
            Some(true)
        );

        let comma =
            parse_status_response(STATUS_COMMA_FIXTURE, None).expect("comma status should parse");
        assert_eq!(comma.encoding, TunetStatusEncoding::Comma);
        assert_eq!(comma.callback, None);
        assert_eq!(comma.signal, TunetStatusSignal::Positive);
        assert_eq!(comma.fields, vec!["ok", "fixture-user", "192.0.2.10", ""]);
        assert!(comma.payload.is_none());
    }

    #[test]
    fn status_proof_requires_an_explicit_online_field() {
        let online = parse_status_response(
            r#"thyouStatus({"error":"ok","online":true})"#,
            Some("thyouStatus"),
        )
        .expect("online status fixture");
        assert_eq!(online.online_state(), TunetOnlineState::Online);
        assert!(online.is_online_proven());

        let offline = parse_status_response(
            r#"thyouStatus({"error":"ok","online":false})"#,
            Some("thyouStatus"),
        )
        .expect("offline status fixture");
        assert_eq!(offline.online_state(), TunetOnlineState::Offline);
        assert!(!offline.is_online_proven());

        let comma = parse_status_response("ok,fixture-user,192.0.2.10", None)
            .expect("comma status fixture");
        assert_eq!(comma.online_state(), TunetOnlineState::Unknown);
        assert!(!comma.is_online_proven());
    }

    #[test]
    fn usage_snapshot_requires_the_complete_json_traffic_shape() {
        let record = parse_status_response(
            r#"{"error":"ok","sum_bytes":"1234","sum_seconds":42,"remain_bytes":0,"remain_seconds":"0","user_balance":"8.10"}"#,
            None,
        )
        .expect("usage status fixture");
        let usage = record.usage_snapshot().expect("complete usage fixture");
        assert_eq!(
            usage,
            TunetUsageSnapshot {
                used_bytes: 1234,
                used_seconds: 42,
                remaining_bytes: 0,
                remaining_seconds: 0,
                account_balance: "8.10".to_owned(),
            }
        );

        for body in [
            r#"{"error":"ok","sum_bytes":"1234","sum_seconds":42,"remain_bytes":0,"remain_seconds":"0"}"#,
            r#"{"error":"ok","sum_bytes":"-1","sum_seconds":42,"remain_bytes":0,"remain_seconds":"0","user_balance":"8.10"}"#,
            r#"{"error":"ok","sum_bytes":"1234","sum_seconds":42,"remain_bytes":0,"remain_seconds":"0","user_balance":"unknown"}"#,
        ] {
            let record = parse_status_response(body, None).expect("usage status object");
            assert!(matches!(
                record.usage_snapshot(),
                Err(TunetClientError::UnsupportedVariant {
                    operation: TunetOperation::Status,
                    ..
                })
            ));
        }

        let comma = parse_status_response("ok,fixture-user,192.0.2.10,", None)
            .expect("legacy status response");
        assert!(matches!(
            comma.usage_snapshot(),
            Err(TunetClientError::UnsupportedVariant {
                operation: TunetOperation::Status,
                ..
            })
        ));
    }

    #[test]
    fn status_requires_an_object_payload_and_positive_offline_signal() {
        for body in ["[]", r#""ok""#] {
            let error = parse_status_response(body, None)
                .expect_err("a scalar or array is not a status payload");
            assert!(matches!(
                error,
                TunetClientError::UnsupportedVariant {
                    operation: TunetOperation::Status,
                    ..
                }
            ));
        }

        let negative_offline =
            parse_status_response(r#"{"error":"not_online_error","online":false}"#, None)
                .expect("status payload");
        assert_eq!(negative_offline.online_state(), TunetOnlineState::Offline);
        assert!(!negative_offline.is_positive());
        assert!(!negative_offline.is_online_proven());
    }

    #[test]
    fn errors_keep_protocol_codes_and_expose_stable_classification() {
        let login_rejection =
            parse_status_response(r#"{"error":"password_error","ecode":"EAUTH"}"#, None)
                .expect("login rejection should remain a decoded status record");
        let error = require_positive(&login_rejection, TunetOperation::Login)
            .expect_err("negative login signal must be rejected");
        assert!(matches!(
            &error,
            TunetClientError::PortalRejected {
                operation: TunetOperation::Login,
                code: Some(code),
            } if code == "error=password_error,ecode=EAUTH"
        ));
        assert_eq!(error.class(), TunetErrorClass::Authentication);
        assert!(error.is_authentication_failure());
        assert!(!error.is_temporary_network());

        let unknown = parse_status_response(r#"{"message":"pending"}"#, None)
            .expect("unknown response should remain a decoded record");
        let error = require_positive(&unknown, TunetOperation::Logout)
            .expect_err("unknown response must not be treated as logout success");
        assert!(matches!(
            error,
            TunetClientError::ResponseNotProven {
                operation: TunetOperation::Logout,
                signal: TunetStatusSignal::Unknown,
            }
        ));
        assert_eq!(error.class(), TunetErrorClass::Proof);

        let unauthorized = TunetClientError::HttpStatus {
            status: reqwest::StatusCode::UNAUTHORIZED,
        };
        assert!(unauthorized.is_authentication_failure());
        assert_eq!(unauthorized.class(), TunetErrorClass::Authentication);

        let unavailable = TunetClientError::HttpStatus {
            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
        };
        assert!(unavailable.is_temporary_network());
        assert_eq!(unavailable.class(), TunetErrorClass::Http);
    }

    #[test]
    fn nonzero_status_ecode_cannot_become_a_positive_online_result() {
        let record = parse_status_response(
            r#"{"ecode":9,"error":"ok","online_ip":"192.0.2.10","online_device_total":"1"}"#,
            None,
        )
        .expect("status payload");
        assert_eq!(record.signal, TunetStatusSignal::Negative);
        assert_eq!(record.online_state(), TunetOnlineState::Online);
        assert!(!record.is_positive());
        assert!(!record.is_online_proven());
    }

    #[test]
    fn status_ecode_string_zero_is_supported_but_invalid_ecode_is_unknown() {
        let string_ecode = parse_status_response(
            r#"{"error":"ok","ecode":"0","online_ip":"192.0.2.10"}"#,
            None,
        )
        .expect("the portal may encode a zero ecode as a string");
        assert_eq!(string_ecode.signal, TunetStatusSignal::Positive);
        assert!(string_ecode.is_online_proven());

        let invalid_ecode = parse_status_response(
            r#"{"error":"ok","ecode":"unknown","online_ip":"192.0.2.10"}"#,
            None,
        )
        .expect("an object with an invalid ecode remains a decoded record");
        assert_eq!(invalid_ecode.signal, TunetStatusSignal::Unknown);
        assert_eq!(invalid_ecode.online_state(), TunetOnlineState::Online);
        assert!(!invalid_ecode.is_online_proven());
    }

    #[test]
    fn status_online_proof_rejects_invalid_and_unspecified_ip_values() {
        for value in ["", "not-an-ip", "0.0.0.0", "::"] {
            let body = format!("{{\"error\":\"ok\",\"online_ip\":\"{value}\"}}");
            let record = parse_status_response(&body, None).expect("status object should decode");
            assert_eq!(record.online_state(), TunetOnlineState::Unknown);
            assert!(!record.is_online_proven());
        }

        let count_without_ip = parse_status_response(
            r#"{"error":"ok","online_ip":"not-an-ip","online_device_total":"1"}"#,
            None,
        )
        .expect("status object should decode");
        assert_eq!(count_without_ip.online_state(), TunetOnlineState::Unknown);
        assert!(!count_without_ip.is_online_proven());
    }

    #[test]
    fn status_proof_is_bound_to_the_requested_ip() {
        let record = parse_status_response(
            r#"{"error":"ok","online_ip":"192.0.2.11","online_ip6":"2001:db8::11","online_device_total":"2"}"#,
            None,
        )
        .expect("status payload should decode");

        assert_eq!(
            record.online_state_for_ip("192.0.2.10"),
            TunetOnlineState::Unknown
        );
        assert_eq!(
            record.online_state_for_ip("192.0.2.11"),
            TunetOnlineState::Online
        );
        assert_eq!(
            record.online_state_for_ip("2001:db8::10"),
            TunetOnlineState::Unknown
        );
        assert_eq!(
            record.online_state_for_ip("2001:db8::11"),
            TunetOnlineState::Online
        );
        assert!(record.is_online_proven());
        assert!(!record.is_offline_proven_for_ip("192.0.2.10"));
    }

    #[test]
    fn status_proof_rejects_a_boolean_without_an_address_for_a_bound_request() {
        let record = parse_status_response(r#"{"error":"ok","online":true}"#, None)
            .expect("status payload should decode");
        assert_eq!(
            record.online_state_for_ip("192.0.2.10"),
            TunetOnlineState::Unknown
        );
    }

    #[test]
    fn status_record_account_state_is_bound_when_a_client_request_has_an_ip() {
        let mut record = parse_status_response(
            r#"{"error":"ok","online_ip":"203.0.113.10","online_ip6":"2001:db8::10","online_device_total":"2"}"#,
            None,
        )
        .expect("status payload should decode");
        assert_eq!(record.online_state(), TunetOnlineState::Online);

        record.bind_to_requested_ip(Some("192.0.2.10"));
        assert_eq!(record.online_state(), TunetOnlineState::Unknown);
        assert!(!record.is_online_proven());

        record.bind_to_requested_ip(Some("203.0.113.10"));
        assert_eq!(record.online_state(), TunetOnlineState::Online);
        assert!(record.is_online_proven());
    }

    #[test]
    fn non_success_and_route_changes_are_rejected_without_body_or_query_leaks() {
        let client = TunetClient::auth4().expect("auth4 client builds");
        let plan = client
            .plan_challenge("fixture-user", &[("ip", "192.0.2.10")])
            .expect("challenge plan builds");

        let status_error = validate_response_metadata(
            &plan,
            &crate::transport::CampusTextResponse {
                status: reqwest::StatusCode::BAD_GATEWAY,
                final_url: reqwest::Url::parse(&plan.url).expect("planned URL"),
                content_type: Some("text/html".to_owned()),
                redirect_location: None,
                body: "password=fixture-secret&challenge=fixture-token".to_owned(),
            },
        )
        .expect_err("HTTP failure must stop protocol parsing");
        assert!(matches!(
            status_error,
            TunetClientError::HttpStatus {
                status: reqwest::StatusCode::BAD_GATEWAY
            }
        ));
        let status_text = format!("{status_error:?} {status_error}");
        assert!(!status_text.contains("fixture-secret"));
        assert!(!status_text.contains("fixture-token"));
        assert!(!status_text.contains("192.0.2.10"));

        let created = crate::transport::CampusTextResponse {
            status: reqwest::StatusCode::CREATED,
            final_url: reqwest::Url::parse(&plan.url).expect("planned URL"),
            content_type: Some("application/json".to_owned()),
            redirect_location: None,
            body: "{}".to_owned(),
        };
        assert!(matches!(
            validate_response_metadata(&plan, &created),
            Err(TunetClientError::HttpStatus {
                status: reqwest::StatusCode::CREATED
            })
        ));

        let transport_error = TunetClientError::Transport(TransportError::HttpStatus {
            status: reqwest::StatusCode::BAD_GATEWAY,
            body: "fixture-secret".to_owned(),
        });
        let transport_text = format!("{transport_error:?} {transport_error}");
        assert!(!transport_text.contains("fixture-secret"));

        let wrong_path = crate::transport::CampusTextResponse {
            status: reqwest::StatusCode::OK,
            final_url: reqwest::Url::parse(
                "https://auth4.tsinghua.edu.cn/cgi-bin/rad_user_info?ip=192.0.2.10",
            )
            .expect("wrong path URL"),
            content_type: Some("application/json".to_owned()),
            redirect_location: None,
            body: "{\"error\":\"ok\"}".to_owned(),
        };
        assert!(matches!(
            validate_response_metadata(&plan, &wrong_path),
            Err(TunetClientError::UnexpectedResponseLocation)
        ));

        let wrong_origin = crate::transport::CampusTextResponse {
            status: reqwest::StatusCode::OK,
            final_url: reqwest::Url::parse(
                "https://other.example.test/cgi-bin/get_challenge?ip=192.0.2.10",
            )
            .expect("wrong origin URL"),
            content_type: Some("application/json".to_owned()),
            redirect_location: None,
            body: "{\"error\":\"ok\"}".to_owned(),
        };
        assert!(matches!(
            validate_response_metadata(&plan, &wrong_origin),
            Err(TunetClientError::UnexpectedResponseLocation)
        ));

        let html_shell = crate::transport::CampusTextResponse {
            status: reqwest::StatusCode::OK,
            final_url: reqwest::Url::parse(&plan.url).expect("planned URL"),
            content_type: Some("text/html; charset=UTF-8".to_owned()),
            redirect_location: None,
            body: "{\"error\":\"ok\"}".to_owned(),
        };
        assert!(matches!(
            validate_response_metadata(&plan, &html_shell),
            Err(TunetClientError::UnsupportedContentType)
        ));

        let missing_content_type = crate::transport::CampusTextResponse {
            status: reqwest::StatusCode::OK,
            final_url: reqwest::Url::parse(&plan.url).expect("planned URL"),
            content_type: None,
            redirect_location: None,
            body: "{}".to_owned(),
        };
        assert!(matches!(
            validate_response_metadata(&plan, &missing_content_type),
            Err(TunetClientError::UnsupportedContentType)
        ));

        let changed_query = crate::transport::CampusTextResponse {
            status: reqwest::StatusCode::OK,
            final_url: reqwest::Url::parse(
                "https://auth4.tsinghua.edu.cn/cgi-bin/get_challenge?username=fixture-user&ip=192.0.2.10&callback=other",
            )
            .expect("changed query URL"),
            content_type: Some("application/json".to_owned()),
            redirect_location: None,
            body: "{}".to_owned(),
        };
        assert!(matches!(
            validate_response_metadata(&plan, &changed_query),
            Err(TunetClientError::UnexpectedResponseLocation)
        ));
    }

    #[test]
    fn status_debug_redacts_payload_and_comma_field_values() {
        let json = parse_status_response(
            r#"{"error":"ok","username":"fixture-user","online_ip":"192.0.2.10"}"#,
            None,
        )
        .expect("status payload should decode");
        let json_debug = format!("{json:?}");
        assert!(!json_debug.contains("fixture-user"));
        assert!(!json_debug.contains("192.0.2.10"));

        let comma = parse_status_response("error,fixture-user,192.0.2.10", None)
            .expect("comma status should decode");
        let comma_debug = format!("{comma:?}");
        assert!(!comma_debug.contains("fixture-user"));
        assert!(!comma_debug.contains("192.0.2.10"));
    }

    #[test]
    fn auth4_challenge_plan_uses_profile_callback_ac_id_and_requested_ip() {
        let client = TunetClient::from_profile(auth4_profile()).expect("client builds");
        let plan = client
            .plan_challenge("fixture-user", &[("ip", "192.0.2.10")])
            .expect("challenge plan builds");

        assert_eq!(plan.method, HttpMethod::Get);
        assert_eq!(plan.body, None);
        assert_eq!(
            plan.url,
            "https://auth4.tsinghua.edu.cn/cgi-bin/get_challenge?username=fixture-user&ip=192.0.2.10&callback=thyouChallenge"
        );
    }

    #[test]
    fn auth6_post_plans_include_fixed_ip_and_profile_parameters() {
        let client = TunetClient::from_profile(auth6_post_profile()).expect("client builds");
        let status = client
            .plan_status(&[("username", "fixture-user")])
            .expect("status plan builds");
        assert_eq!(status.method, HttpMethod::Post);
        assert_eq!(
            status.body.as_deref(),
            Some("username=fixture-user&ip=2001%3Adb8%3A%3A10&callback=thyouStatus")
        );

        let logout = client
            .plan_logout("fixture-user", &[])
            .expect("logout plan builds");
        assert_eq!(
            logout.body.as_deref(),
            Some(
                "action=logout&username=fixture-user&ip=2001%3Adb8%3A%3A10&ac_id=1&callback=thyouPortal"
            )
        );
    }

    #[test]
    fn post_execution_uses_the_portals_urlencoded_content_type() {
        let client = TunetClient::from_profile(auth6_post_profile()).expect("client builds");
        let plan = client
            .plan_srun_login("fixture-user", "fixture-password", "fixture-challenge", &[])
            .expect("login plan builds");
        let request = build_post_request(client.transport(), &plan).expect("HTTP request");

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("form body");
        assert_eq!(
            body,
            plan.body.as_deref().expect("planned form body").as_bytes()
        );
    }

    #[test]
    fn current_srun_profile_derives_and_form_encodes_login_material() {
        let client = TunetClient::from_profile(auth6_post_profile()).expect("client builds");
        let plan = client
            .plan_srun_login("alice", "password", "challenge", &[])
            .expect("current SRun login plan builds");

        assert_eq!(plan.operation, TunetOperation::Login);
        assert_eq!(plan.method, HttpMethod::Post);
        assert_eq!(
            plan.url,
            "https://auth6.tsinghua.edu.cn/cgi-bin/srun_portal"
        );
        assert_eq!(
            plan.body.as_deref(),
            Some(
                "action=login&username=alice&password=%7BMD5%7De10352c6d4c094363abc1295006e4e9e&os=Mac+OS&name=Macintosh&double_stack=0&chksum=53f0eefc4274b24629035ef1bd84798b9231b298&info=%7BSRBX1%7Di27RkAuC1xEbzQUhqBkaJPztV7rHrLqsgk7g4cK7O6URFirqhmCxmbz43Ot3b0jD9PeJCe5Vx7vnGlkO93twiJNq0vTig6fY%2BqGbuzMilH7FlwGOh1I6D%2BumXCEpCPCrodt4ES%3D%3D&ac_id=1&ip=2001%3Adb8%3A%3A10&n=200&type=1&callback=thyouPortal"
            )
        );
    }

    #[test]
    fn srun_login_debug_redacts_plaintext_and_derived_fields() {
        let client = TunetClient::from_profile(auth6_post_profile()).expect("client builds");
        let plan = client
            .plan_srun_login("alice", "plain-text-secret", "challenge", &[])
            .expect("current SRun login plan builds");
        let debug = format!("{plan:?}");
        let material = crate::tunet_auth::build_srun_login_material(
            SrunPasswordDigestScheme::PortalHmacMd5,
            "alice",
            "plain-text-secret",
            "challenge",
            "2",
            "2001:db8::10",
        )
        .expect("fixture material should build");

        assert!(!debug.contains("plain-text-secret"));
        assert!(!debug.contains(material.wire_password()));
        assert!(!debug.contains(material.info()));
        assert!(!debug.contains(material.checksum()));
    }

    #[test]
    fn unsupported_srun_digest_profiles_are_rejected_without_fallback() {
        assert!(matches!(
            TunetClient::from_profile(
                TunetProfile::new(
                    AuthFamily::Auth4,
                    HttpsEndpoint::new("auth4.tsinghua.edu.cn", 443)
                        .expect("fixture endpoint is valid"),
                    paths(),
                    MethodProfile::new(
                        HttpMethod::Get,
                        HttpMethod::Get,
                        HttpMethod::Get,
                        HttpMethod::Get,
                    ),
                    CallbackProfile::new(None, None, None).expect("fixture callbacks are valid"),
                    AcIdPolicy::fixed("1").expect("fixture ac_id is valid"),
                    IpPolicy::from_request(IpFamily::V4),
                    OuterDigestScheme::CandidateMd5Hex,
                )
                .expect("fixture profile is valid")
            ),
            Err(TunetClientError::UnsupportedSrunDigest {
                configured: OuterDigestScheme::CandidateMd5Hex,
            })
        ));

        let unspecified_profile = TunetProfile::new(
            AuthFamily::Auth4,
            HttpsEndpoint::new("auth4.tsinghua.edu.cn", 443).expect("fixture endpoint is valid"),
            paths(),
            MethodProfile::new(
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
            ),
            CallbackProfile::new(None, None, None).expect("fixture callbacks are valid"),
            AcIdPolicy::fixed("1").expect("fixture ac_id is valid"),
            IpPolicy::from_request(IpFamily::V4),
            OuterDigestScheme::Unspecified,
        )
        .expect("fixture profile is valid");
        assert!(matches!(
            TunetClient::from_profile(unspecified_profile),
            Err(TunetClientError::UnsupportedSrunDigest {
                configured: OuterDigestScheme::Unspecified,
            })
        ));
    }

    #[test]
    fn login_forwards_caller_wire_password_without_calculating_a_digest() {
        let client = TunetClient::from_profile(auth4_profile()).expect("client builds");
        let digest = OuterDigest::new(
            OuterDigestScheme::CandidateHmacMd5Hex,
            "caller-precomputed-wire-value",
        )
        .expect("fixture digest is non-empty");
        let plan = client
            .plan_login("fixture-user", &digest, &[("ip", "192.0.2.10")])
            .expect("login plan builds");

        assert_eq!(
            plan.url,
            "https://auth4.tsinghua.edu.cn/cgi-bin/srun_portal?action=login&username=fixture-user&password=caller-precomputed-wire-value&ac_id=1&ip=192.0.2.10&callback=thyouPortal"
        );
        assert_eq!(plan.body, None);
        assert!(!format!("{plan:?}").contains("caller-precomputed-wire-value"));
        assert!(!format!("{digest:?}").contains("caller-precomputed-wire-value"));
    }

    #[test]
    fn unknown_or_mismatched_digest_is_explicitly_rejected() {
        let client = TunetClient::from_profile(auth4_profile()).expect("client builds");
        let unknown = OuterDigest::new(OuterDigestScheme::Unspecified, "caller-value")
            .expect("fixture digest is non-empty");
        assert!(matches!(
            client.plan_login("fixture-user", &unknown, &[]),
            Err(TunetClientError::UnsupportedDigest {
                configured: OuterDigestScheme::CandidateHmacMd5Hex,
                provided: OuterDigestScheme::Unspecified,
            })
        ));

        let mismatched = OuterDigest::new(OuterDigestScheme::CandidateMd5Hex, "caller-value")
            .expect("fixture digest is non-empty");
        assert!(matches!(
            client.plan_login("fixture-user", &mismatched, &[]),
            Err(TunetClientError::UnsupportedDigest {
                configured: OuterDigestScheme::CandidateHmacMd5Hex,
                provided: OuterDigestScheme::CandidateMd5Hex,
            })
        ));
    }

    #[test]
    fn plain_json_status_is_parsed_without_losing_online_proof() {
        let record = parse_status_response(r#"{"error":"ok","online_ip":"192.0.2.10"}"#, None)
            .expect("plain JSON status");
        assert_eq!(record.encoding, TunetStatusEncoding::Json);
        assert_eq!(record.signal, TunetStatusSignal::Positive);
        assert_eq!(record.online_state(), TunetOnlineState::Online);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_http_fixture_executes_challenge_login_and_status_with_cookie_reuse() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Content-Type: application/javascript; charset=UTF-8\r\nSet-Cookie: portal-session=fixture; Path=/\r\n",
                    CHALLENGE_JSONP_FIXTURE,
                ),
                (
                    "Content-Type: application/javascript; charset=UTF-8\r\n",
                    "thyouPortal({\"error\":\"ok\",\"ecode\":0});",
                ),
                (
                    "Content-Type: application/javascript; charset=UTF-8\r\n",
                    "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.10\"});",
                ),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("fixture connection");
                requests_tx
                    .send(read_http_request(&mut stream))
                    .expect("capture request");
                write_http_response(&mut stream, headers, body);
            }
        });

        let profile = TunetProfile::current_with_overrides(
            AuthFamily::Auth4,
            TunetProfileOverrides {
                endpoint: Some(
                    HttpsEndpoint::http("127.0.0.1", address.port())
                        .expect("HTTP fixture endpoint"),
                ),
                ..TunetProfileOverrides::default()
            },
        )
        .expect("fixture profile");
        let config = TunetClientConfig::new(profile).expect("fixture config");
        let transport =
            CampusHttpTransport::with_timeout("THYou/tunet-fixture", Duration::from_secs(5))
                .expect("fixture transport");
        let client = TunetClient::with_transport(config, transport).expect("fixture client");
        let status = client
            .login_with_password_verified(
                "fixture-user",
                "fixture-password",
                &[("ip", "192.0.2.10")],
            )
            .await
            .expect("fixture login");
        assert_eq!(status.online_state(), TunetOnlineState::Online);
        assert!(status.is_online_proven());

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with(
            "GET /cgi-bin/get_challenge?username=fixture-user&ip=192.0.2.10&callback=thyouChallenge HTTP/1.1"
        ));
        assert!(requests[1].starts_with("GET /cgi-bin/srun_portal?"));
        assert!(requests[1].contains("action=login"));
        assert!(requests[1].contains("password=%7BMD5%7D"));
        assert!(requests[1].contains("info=%7BSRBX1%7D"));
        assert!(requests[1].contains("chksum="));
        assert!(requests[1].contains("double_stack=0&chksum="));
        assert!(requests[1].contains("&ac_id=1&ip=192.0.2.10&n=200&type=1&callback=thyouPortal"));
        assert!(requests[1].contains("portal-session=fixture"));
        assert!(!requests[1].contains("fixture-password"));
        assert!(
            requests[2].starts_with(
                "GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1"
            )
        );
        assert!(requests[2].contains("portal-session=fixture"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_http_fixture_requires_positive_logout_and_follow_up_offline_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Content-Type: application/javascript\r\nSet-Cookie: portal-session=fixture; Path=/\r\n",
                    "thyouPortal({\"error\":\"ok\",\"ecode\":0});",
                ),
                (
                    "Content-Type: application/javascript\r\n",
                    "thyouStatus({\"error\":\"not_online_error\",\"online_ip\":\"\",\"online_ip6\":\"\",\"online_device_total\":0});",
                ),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("fixture connection");
                requests_tx
                    .send(read_http_request(&mut stream))
                    .expect("capture request");
                write_http_response(&mut stream, headers, body);
            }
        });

        let profile = TunetProfile::current_with_overrides(
            AuthFamily::Auth4,
            TunetProfileOverrides {
                endpoint: Some(
                    HttpsEndpoint::http("127.0.0.1", address.port())
                        .expect("HTTP fixture endpoint"),
                ),
                ..TunetProfileOverrides::default()
            },
        )
        .expect("fixture profile");
        let config = TunetClientConfig::new(profile).expect("fixture config");
        let transport =
            CampusHttpTransport::with_timeout("THYou/tunet-fixture", Duration::from_secs(5))
                .expect("fixture transport");
        let client = TunetClient::with_transport(config, transport).expect("fixture client");
        let status = client
            .disconnect_verified("fixture-user", &[("ip", "192.0.2.10")])
            .await
            .expect("fixture logout");
        assert_eq!(status.online_state(), TunetOnlineState::Offline);
        assert!(!status.is_online_proven());

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /cgi-bin/srun_portal?"));
        assert!(requests[0].contains("action=logout"));
        assert!(
            requests[1].starts_with(
                "GET /cgi-bin/rad_user_info?ip=192.0.2.10&callback=thyouStatus HTTP/1.1"
            )
        );
        assert!(requests[1].contains("portal-session=fixture"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_http_fixture_stops_on_login_rejection_without_polling_status() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Content-Type: application/javascript\r\nSet-Cookie: portal-session=fixture; Path=/\r\n",
                    CHALLENGE_JSONP_FIXTURE,
                ),
                (
                    "Content-Type: application/javascript\r\n",
                    "thyouPortal({\"error\":\"password_error\",\"ecode\":1});",
                ),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("fixture connection");
                requests_tx
                    .send(read_http_request(&mut stream))
                    .expect("capture request");
                write_http_response(&mut stream, headers, body);
            }
        });

        let profile = TunetProfile::current_with_overrides(
            AuthFamily::Auth4,
            TunetProfileOverrides {
                endpoint: Some(
                    HttpsEndpoint::http("127.0.0.1", address.port())
                        .expect("HTTP fixture endpoint"),
                ),
                ..TunetProfileOverrides::default()
            },
        )
        .expect("fixture profile");
        let config = TunetClientConfig::new(profile).expect("fixture config");
        let transport =
            CampusHttpTransport::with_timeout("THYou/tunet-fixture", Duration::from_secs(5))
                .expect("fixture transport");
        let client = TunetClient::with_transport(config, transport).expect("fixture client");
        let error = client
            .login_with_password_verified(
                "fixture-user",
                "fixture-password",
                &[("ip", "192.0.2.10")],
            )
            .await
            .expect_err("rejected credentials must not enter status polling");
        assert!(error.is_authentication_failure());
        assert!(matches!(
            error,
            TunetClientError::PortalRejected {
                operation: TunetOperation::Login,
                ..
            }
        ));

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /cgi-bin/get_challenge?"));
        assert!(requests[1].starts_with("GET /cgi-bin/srun_portal?"));
        assert!(!requests[1].contains("fixture-password"));
    }

    #[test]
    fn profile_from_request_parameters_remain_explicit() {
        let client = TunetClient::from_profile(auth4_profile()).expect("client builds");
        let error = client
            .plan_challenge("fixture-user", &[])
            .expect_err("the profile requires a caller IP");
        assert!(matches!(
            error,
            TunetClientError::Profile(ProfileError::MissingParameter("ip"))
        ));
    }
}
