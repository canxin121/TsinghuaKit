//! Read-only physical-education test results (`体测成绩`).
//!
//! The service answers one GET with a bare JSON object.  Unlike the card
//! service's envelope, this endpoint has no `success: true` wrapper: it either
//! returns the measurement record or an object whose `success` field is the
//! *string* `"false"`, which is the service's explicit "you have no result"
//! state.  Three distinct outcomes therefore have to stay distinct:
//!
//! * a parsed record with measurements,
//! * an explicit no-result answer, which is a validated empty state rather
//!   than a failure and never a zero-filled record,
//! * a response that is not this deployment at all (an HTML login or expiry
//!   page, a redirect off the mapping, a non-JSON body).
//!
//! The App-side "reference total" is deliberately *not* read from the
//! response: the service never sends it.  It is recomputed here from the
//! fixed weights the public reference uses and is labelled as reference-only,
//! because presenting a locally computed number as a service score would
//! invent a measurement.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

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

use crate::campus_html::{self, PageClass};
use crate::transport::{CampusHttpTransport, TransportError};

/// The registrar roaming selector for the physical-education test report.
///
/// The target host shares the registrar WebVPN mapping, so the resulting
/// request still travels through the already-allowlisted INFO handoff.
pub const PHYSICAL_EXAM_WEBVPN_TARGET: &str = "8BF4F9A706589060488B6B6179E462E5";

/// The report endpoint behind the selector above.
pub const PHYSICAL_EXAM_PATH: &str = "/tyjx.tyjx_tc_xscjb.do";
pub const PHYSICAL_EXAM_QUERY: &str = "m=jsonCj";

/// Upper bound on one report body.
const MAX_BODY_BYTES: usize = 512 * 1024;
/// Upper bound on one optional measurement string taken from the response.
const MAX_MEASUREMENT_CHARS: usize = 64;

/// The service's explicit no-result marker.  It is a JSON *string*, not a
/// boolean, which is why the parser matches on the text.
const NO_RESULT_FLAG: &str = "false";

/// Weights of the locally recomputed reference total.  They are the observed
/// public-reference weights; the service itself never sends a total.
const WEIGHT_HEIGHT_WEIGHT: f64 = 0.15;
const WEIGHT_50M: f64 = 0.2;
const WEIGHT_SIT_AND_REACH: f64 = 0.1;
const WEIGHT_STANDING_LONG_JUMP: f64 = 0.1;
const WEIGHT_PULL_UP: f64 = 0.1;
const WEIGHT_1000M: f64 = 0.2;
const WEIGHT_SIT_UP: f64 = 0.1;
const WEIGHT_800M: f64 = 0.2;
const WEIGHT_VITAL_CAPACITY: f64 = 0.15;

static NEXT_PHYSICAL_EXAM_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This profile only issues GETs, so a write route cannot be smuggled into a
/// read plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalExamMethod {
    Get,
}

/// The one observed physical-education operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalExamOperation {
    ReadResult,
}

/// A physical-education read requires an established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalExamSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path and query only, never an absolute
/// WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalExamRequestPlan {
    pub operation: PhysicalExamOperation,
    pub method: PhysicalExamMethod,
    pub path: &'static str,
    pub query: &'static str,
    pub webvpn_target: &'static str,
    pub session_prerequisite: PhysicalExamSessionPrerequisite,
}

impl PhysicalExamRequestPlan {
    pub fn query_string(&self) -> &str {
        self.query
    }
}

/// Fixed physical-education route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicalExamProfile;

impl PhysicalExamProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        PHYSICAL_EXAM_WEBVPN_TARGET
    }

    pub fn result_request(self) -> PhysicalExamRequestPlan {
        PhysicalExamRequestPlan {
            operation: PhysicalExamOperation::ReadResult,
            method: PhysicalExamMethod::Get,
            path: PHYSICAL_EXAM_PATH,
            query: PHYSICAL_EXAM_QUERY,
            webvpn_target: PHYSICAL_EXAM_WEBVPN_TARGET,
            session_prerequisite: PhysicalExamSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// One test item: the recorded raw measurement and the service's own score.
///
/// Both halves are optional and independent.  A student who has not taken an
/// item has neither; the service also reports an item whose score exists
/// before its raw value does, so the two are never merged into one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalExamItem {
    /// The raw recorded value exactly as the service reported it.
    pub measurement: Option<String>,
    /// The service's score for this item.
    pub score: Option<String>,
}

impl PhysicalExamItem {
    /// The empty item used by the service's no-result answer.
    pub const fn empty() -> Self {
        Self {
            measurement: None,
            score: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.measurement.is_none() && self.score.is_none()
    }
}

/// The six scored static/apparatus items plus the two run distances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalExamItems {
    pub height_weight: PhysicalExamItem,
    pub vital_capacity: PhysicalExamItem,
    pub fifty_meter: PhysicalExamItem,
    pub standing_long_jump: PhysicalExamItem,
    pub sit_and_reach: PhysicalExamItem,
    pub sit_up: PhysicalExamItem,
    pub pull_up: PhysicalExamItem,
    pub eight_hundred_meter: PhysicalExamItem,
    pub one_thousand_meter: PhysicalExamItem,
}

impl PhysicalExamItems {
    /// Returns every item as `(measurement, score)` pairs in the observed
    /// display order, skipping the items this student has no record for.
    pub fn reported(&self) -> Vec<(&'static str, &PhysicalExamItem)> {
        [
            ("height_weight", &self.height_weight),
            ("vital_capacity", &self.vital_capacity),
            ("fifty_meter", &self.fifty_meter),
            ("standing_long_jump", &self.standing_long_jump),
            ("sit_and_reach", &self.sit_and_reach),
            ("sit_up", &self.sit_up),
            ("pull_up", &self.pull_up),
            ("eight_hundred_meter", &self.eight_hundred_meter),
            ("one_thousand_meter", &self.one_thousand_meter),
        ]
        .into_iter()
        .filter(|(_, item)| !item.is_empty())
        .collect()
    }
}

/// A validated physical-education report.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalExamReport {
    /// True when the service answered that this account has no score yet.
    /// Every other field is then at its empty value, and the report is a
    /// validated empty state rather than a failed read.
    pub no_result: bool,
    /// The service's explicit exemption flag, absent when the service did not
    /// report one.
    pub exemption: Option<String>,
    /// The service's exemption reason, present only alongside an exemption.
    pub exemption_reason: Option<String>,
    /// The service's own total, distinct from the locally computed reference.
    pub total: Option<String>,
    pub standard_score: Option<String>,
    pub bonus_score: Option<String>,
    pub long_run_bonus_score: Option<String>,
    pub height: Option<String>,
    pub weight: Option<String>,
    pub physical_education_grade: Option<String>,
    pub items: PhysicalExamItems,
    /// A locally recomputed reference total, or `None` when the service did
    /// not report every weight it needs.
    ///
    /// This value is **not** a service score.  It is produced by applying the
    /// observed fixed weights to the service's own item scores, and callers
    /// must present it as reference-only.
    pub reference_total: Option<f64>,
}

impl PhysicalExamReport {
    /// The label callers must show next to [`Self::reference_total`].
    pub const REFERENCE_TOTAL_LABEL: &'static str = "参考成绩（APP自动结算，仅供参考）";

    /// Returns a validated empty report for the service's no-result answer.
    fn empty_report() -> Self {
        Self {
            no_result: true,
            exemption: None,
            exemption_reason: None,
            total: None,
            standard_score: None,
            bonus_score: None,
            long_run_bonus_score: None,
            height: None,
            weight: None,
            physical_education_grade: None,
            items: PhysicalExamItems {
                height_weight: PhysicalExamItem::empty(),
                vital_capacity: PhysicalExamItem::empty(),
                fifty_meter: PhysicalExamItem::empty(),
                standing_long_jump: PhysicalExamItem::empty(),
                sit_and_reach: PhysicalExamItem::empty(),
                sit_up: PhysicalExamItem::empty(),
                pull_up: PhysicalExamItem::empty(),
                eight_hundred_meter: PhysicalExamItem::empty(),
                one_thousand_meter: PhysicalExamItem::empty(),
            },
            reference_total: None,
        }
    }

    /// Returns true when the service reported no measurement at all.
    pub fn is_empty(&self) -> bool {
        self.no_result
            || (self.items.reported().is_empty()
                && self.total.is_none()
                && self.standard_score.is_none()
                && self.exemption.is_none())
    }

    /// True when the service answered that this account has no score yet.
    pub fn no_result(&self) -> bool {
        self.no_result
    }

    pub fn exemption(&self) -> Option<&str> {
        self.exemption.as_deref()
    }

    pub fn exemption_reason(&self) -> Option<&str> {
        self.exemption_reason.as_deref()
    }

    /// The service's own total, distinct from [`Self::reference_total`].
    pub fn total(&self) -> Option<&str> {
        self.total.as_deref()
    }

    pub fn standard_score(&self) -> Option<&str> {
        self.standard_score.as_deref()
    }

    pub fn bonus_score(&self) -> Option<&str> {
        self.bonus_score.as_deref()
    }

    pub fn long_run_bonus_score(&self) -> Option<&str> {
        self.long_run_bonus_score.as_deref()
    }

    pub fn height(&self) -> Option<&str> {
        self.height.as_deref()
    }

    pub fn weight(&self) -> Option<&str> {
        self.weight.as_deref()
    }

    pub fn physical_education_grade(&self) -> Option<&str> {
        self.physical_education_grade.as_deref()
    }

    /// The locally recomputed reference total, never a service score.
    pub fn reference_total(&self) -> Option<f64> {
        self.reference_total
    }

    pub fn items(&self) -> &PhysicalExamItems {
        &self.items
    }
}

/// Parser failures retain only stable field names.  They never keep response
/// bytes, Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PhysicalExamParseError {
    #[error("physical-exam response body is empty")]
    EmptyBody,

    #[error("physical-exam response is an HTML login page")]
    LoginPage,

    #[error("physical-exam response is an expired or timed-out page")]
    ExpiredPage,

    #[error("physical-exam response is not a JSON document")]
    NotJson,

    #[error("physical-exam response is not a JSON object")]
    NotObject,

    #[error("physical-exam response did not carry the service's success flag")]
    MissingSuccessFlag,

    #[error("physical-exam response field {field} is not a scalar value")]
    InvalidField { field: &'static str },
}

impl PhysicalExamParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired INFO session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

/// Adapter failures are body-free so a login page cannot leak through a debug
/// or bridge DTO.
#[derive(Debug, Error)]
pub enum PhysicalExamAdapterError {
    #[error("physical-exam base URL is invalid")]
    InvalidBaseUrl,

    #[error("physical-exam transport failed")]
    Transport(#[source] TransportError),

    #[error("physical-exam request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("physical-exam response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("physical-exam response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("physical-exam response is not a JSON document")]
    UnexpectedContentType,

    #[error("physical-exam INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("physical-exam response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("physical-exam response could not be parsed: {0}")]
    Parse(#[source] PhysicalExamParseError),
}

impl PhysicalExamAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "physical_exam_config",
            Self::Transport(_) => "physical_exam_network",
            Self::HttpStatus { .. } => "physical_exam_http",
            Self::UnexpectedOrigin => "physical_exam_origin",
            Self::UnexpectedPath => "physical_exam_path",
            Self::UnexpectedContentType => "physical_exam_content_type",
            Self::SessionExpired => "physical_exam_auth_required",
            Self::UnexpectedDeployment => "physical_exam_template",
            Self::Parse(PhysicalExamParseError::EmptyBody) => "physical_exam_body_empty",
            Self::Parse(PhysicalExamParseError::LoginPage) => "physical_exam_auth_required",
            Self::Parse(PhysicalExamParseError::ExpiredPage) => "physical_exam_auth_required",
            Self::Parse(PhysicalExamParseError::NotJson) => "physical_exam_not_json",
            Self::Parse(PhysicalExamParseError::NotObject) => "physical_exam_not_object",
            Self::Parse(PhysicalExamParseError::MissingSuccessFlag) => "physical_exam_envelope",
            Self::Parse(PhysicalExamParseError::InvalidField { .. }) => {
                "physical_exam_field_invalid"
            }
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

/// Configuration for a read-only physical-education adapter.
#[derive(Clone)]
pub struct PhysicalExamAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl PhysicalExamAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, PhysicalExamAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/physical-exam", Duration::from_secs(30))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, PhysicalExamAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, PhysicalExamAdapterError> {
        let base_url =
            Url::parse(base_url).map_err(|_| PhysicalExamAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(PhysicalExamAdapterError::InvalidBaseUrl);
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

    fn transport(&self) -> Result<CampusHttpTransport, PhysicalExamAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(PhysicalExamAdapterError::Transport)
    }
}

impl fmt::Debug for PhysicalExamAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalExamAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one physical-education response.  It is
/// opaque: no Cookie, URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct PhysicalExamBusinessProof {
    adapter_binding: u64,
    operation: PhysicalExamOperation,
}

impl fmt::Debug for PhysicalExamBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalExamBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated report together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalExamRead {
    pub value: PhysicalExamReport,
    pub proof: PhysicalExamBusinessProof,
}

/// Read-only physical-education client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct PhysicalExamAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: PhysicalExamProfile,
    binding: u64,
}

impl PhysicalExamAdapter {
    pub fn new(config: PhysicalExamAdapterConfig) -> Result<Self, PhysicalExamAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, PhysicalExamAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: PhysicalExamProfile::standard(),
            binding: NEXT_PHYSICAL_EXAM_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> PhysicalExamProfile {
        self.profile
    }

    /// Reads the report, mapping the service's no-result answer to a
    /// validated empty report.
    pub async fn read_result(&self) -> Result<PhysicalExamReport, PhysicalExamAdapterError> {
        self.read_result_with_proof().await.map(|read| read.value)
    }

    pub async fn read_result_with_proof(
        &self,
    ) -> Result<PhysicalExamRead, PhysicalExamAdapterError> {
        let plan = self.profile.result_request();
        let body = self.execute(&plan).await?;
        let value = parse_physical_exam_json(&body).map_err(Self::map_parse_error)?;
        Ok(PhysicalExamRead {
            value,
            proof: self.business_proof(PhysicalExamOperation::ReadResult),
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &PhysicalExamBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    fn business_proof(&self, operation: PhysicalExamOperation) -> PhysicalExamBusinessProof {
        PhysicalExamBusinessProof {
            adapter_binding: self.binding,
            operation,
        }
    }

    async fn execute(
        &self,
        plan: &PhysicalExamRequestPlan,
    ) -> Result<String, PhysicalExamAdapterError> {
        let endpoint = self.endpoint(plan)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| PhysicalExamAdapterError::Transport(TransportError::Request(error)))?;
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
            .map_err(|_| PhysicalExamAdapterError::UnexpectedOrigin)?;
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
            return Err(PhysicalExamAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(PhysicalExamAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(PhysicalExamAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| PhysicalExamAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(PhysicalExamAdapterError::UnexpectedDeployment);
        }
        // The service's own session-expiry banner arrives as a 200 body, so
        // it is classified here, before the JSON parser can turn it into an
        // "unrecognized format" failure.
        if matches!(
            campus_html::classify_page(&body),
            PageClass::Login | PageClass::Expired
        ) {
            return Err(PhysicalExamAdapterError::SessionExpired);
        }
        if status != StatusCode::OK {
            return Err(PhysicalExamAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.query() != Some(plan.query)
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(PhysicalExamAdapterError::UnexpectedPath);
        }
        if !is_json_content_type(content_type.as_deref()) {
            return Err(PhysicalExamAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    fn endpoint(&self, plan: &PhysicalExamRequestPlan) -> Result<Url, PhysicalExamAdapterError> {
        if !valid_relative_path(plan.path) || plan.query.chars().any(char::is_control) {
            return Err(PhysicalExamAdapterError::InvalidBaseUrl);
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

    fn map_parse_error(error: PhysicalExamParseError) -> PhysicalExamAdapterError {
        match error {
            PhysicalExamParseError::LoginPage | PhysicalExamParseError::ExpiredPage => {
                PhysicalExamAdapterError::SessionExpired
            }
            other => PhysicalExamAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for PhysicalExamAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalExamAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Parses one physical-education report body.
///
/// The service's no-result answer (`success: "false"`) becomes a validated
/// empty report, never a record whose every measurement is zero and never a
/// parse failure.  A body that cannot be read as this deployment's JSON
/// object is an error, because an unparseable answer is not evidence that the
/// student has no score.
pub fn parse_physical_exam_json(body: &str) -> Result<PhysicalExamReport, PhysicalExamParseError> {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    if trimmed.is_empty() {
        return Err(PhysicalExamParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(PhysicalExamParseError::LoginPage),
        PageClass::Expired => return Err(PhysicalExamParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    if !trimmed.starts_with('{') {
        return Err(PhysicalExamParseError::NotJson);
    }
    let root: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|_| PhysicalExamParseError::NotJson)?;
    let Some(object) = root.as_object() else {
        return Err(PhysicalExamParseError::NotObject);
    };
    let Some(success) = object.get("success").and_then(serde_json::Value::as_str) else {
        return Err(PhysicalExamParseError::MissingSuccessFlag);
    };
    if success == NO_RESULT_FLAG {
        return Ok(PhysicalExamReport::empty_report());
    }

    let field = |name: &'static str| -> Result<Option<String>, PhysicalExamParseError> {
        match object.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(value)) => Ok(bounded_text(value)),
            Some(serde_json::Value::Number(value)) => Ok(bounded_text(value.to_string().as_str())),
            Some(_) => Err(PhysicalExamParseError::InvalidField { field: name }),
        }
    };

    let item = |measurement: &'static str,
                score: &'static str|
     -> Result<PhysicalExamItem, PhysicalExamParseError> {
        Ok(PhysicalExamItem {
            measurement: field(measurement)?,
            score: field(score)?,
        })
    };

    let height = field("sg")?;
    let weight = field("tz")?;
    let items = PhysicalExamItems {
        height_weight: PhysicalExamItem {
            measurement: None,
            score: field("sgtzfs")?,
        },
        vital_capacity: item("fhl", "fhltzfs")?,
        fifty_meter: item("wsmp", "wsmpfs")?,
        standing_long_jump: item("ldty", "ldtyfs")?,
        sit_and_reach: item("zwtqq", "zwtqqfs")?,
        sit_up: item("ywqz", "ywqzfs")?,
        pull_up: item("ytxs", "ytxsfs")?,
        eight_hundred_meter: item("bbmp", "bbmpfs")?,
        one_thousand_meter: item("yqmp", "yqmpfs")?,
    };
    let reference_total = reference_total(&items);

    Ok(PhysicalExamReport {
        no_result: false,
        exemption: field("sfmc")?,
        exemption_reason: field("mcyy")?,
        total: field("zf")?,
        standard_score: field("bzf")?,
        bonus_score: field("fjf")?,
        long_run_bonus_score: field("cpfjf")?,
        height,
        weight,
        physical_education_grade: field("tykcj")?,
        items,
        reference_total,
    })
}

/// Applies the observed fixed weights to the service's own item scores.
///
/// An item the student did not take carries no score, and an untaken item
/// contributes nothing to the sum: that is the observed reference behavior,
/// and it is what lets a total exist at all, since one student runs either
/// the 800m or the 1000m case but never both.  A score the service *did*
/// report but that cannot be read as a number is a different situation, and
/// it makes the whole reference total absent rather than silently smaller.
///
/// The result is a local reference value, never a service score.
fn reference_total(items: &PhysicalExamItems) -> Option<f64> {
    let score = |item: &PhysicalExamItem| -> Option<f64> {
        let Some(text) = item.score.as_ref() else {
            // Not recorded: no contribution.
            return Some(0.0);
        };
        let value = text.trim().parse::<f64>().ok()?;
        value.is_finite().then_some(value)
    };
    let weighted = [
        (score(&items.height_weight)?, WEIGHT_HEIGHT_WEIGHT),
        (score(&items.fifty_meter)?, WEIGHT_50M),
        (score(&items.sit_and_reach)?, WEIGHT_SIT_AND_REACH),
        (score(&items.standing_long_jump)?, WEIGHT_STANDING_LONG_JUMP),
        (score(&items.pull_up)?, WEIGHT_PULL_UP),
        (score(&items.vital_capacity)?, WEIGHT_VITAL_CAPACITY),
        (score(&items.sit_up)?, WEIGHT_SIT_UP),
        (score(&items.eight_hundred_meter)?, WEIGHT_800M),
        (score(&items.one_thousand_meter)?, WEIGHT_1000M),
    ];
    let total: f64 = weighted.iter().map(|(value, weight)| value * weight).sum();
    total.is_finite().then_some(total)
}

/// Keeps one optional measurement inside the bounded public shape.
fn bounded_text(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.chars().take(MAX_MEASUREMENT_CHARS).collect())
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, PhysicalExamAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(PhysicalExamAdapterError::InvalidBaseUrl);
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

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, PhysicalExamAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(PhysicalExamAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| PhysicalExamAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(PhysicalExamAdapterError::UnexpectedOrigin);
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
