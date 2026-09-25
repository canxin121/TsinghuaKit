//! Evidence boundary for Registrar examination lookup.
//!
//! The Registrar calendar route is a schedule/event feed and the grades route
//! is a grade report; neither may be reinterpreted as an examination response.
//! The public page API is backed by the independently evidenced undergraduate
//! `jxmh.do?url=/jxmh.do&m=bks_ksSearch` HTML table. The same page's
//! per-course JSONP action is also exposed as a separately validated contract;
//! its callback, course binding, and response shape are checked before a
//! record is produced. No graduate route is inferred from the undergraduate
//! contract.

use std::{fmt, str::FromStr};

use chrono::NaiveDate;
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[path = "webvpn_url.rs"]
mod webvpn_url;
use self::webvpn_url::{path_is_within_base, resolve_endpoint as resolve_mapped_endpoint};

/// Confirmed undergraduate examination page path.
pub const REGISTRAR_EXAM_PATH: &str = "/jxmh.do";

/// Confirmed undergraduate examination page action.
pub const REGISTRAR_EXAM_ACTION: &str = "bks_ksSearch";

/// Confirmed page return URL parameter value.
pub const REGISTRAR_EXAM_PAGE_RETURN_URL: &str = "/jxmh.do";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrarExamStage {
    Undergraduate,
    Graduate,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrarExamError {
    #[error(
        "Registrar examination lookup is not configured: no independently verified endpoint or response contract exists"
    )]
    NotConfigured,

    #[error("the Registrar examination base URL is invalid: {reason}")]
    InvalidBaseUrl { reason: &'static str },

    #[error("the Registrar examination request is invalid: {reason}")]
    InvalidRequest { reason: &'static str },

    #[error("the graduate Registrar examination route is not evidenced")]
    UnsupportedGraduateRoute,

    #[error("the academic term must match YYYY-YYYY-1, YYYY-YYYY-2, or YYYY-YYYY-3")]
    InvalidTerm,

    #[error("the course code must contain exactly eight ASCII digits")]
    InvalidCourseCode,

    #[error("the course sequence must contain one to three ASCII digits")]
    InvalidCourseSequence,

    #[error("the JSONP callback is not a JavaScript identifier path")]
    InvalidCallback,

    #[error("the Registrar response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("the Registrar response came from an unexpected path")]
    UnexpectedPath,

    #[error("the Registrar response did not retain the confirmed request parameters")]
    MissingConfirmedQuery,

    #[error("the Registrar examination response returned HTTP status {status}")]
    HttpStatus { status: u16 },

    #[error("the Registrar response is a login or timeout page")]
    AuthenticationRequired,

    #[error("the Registrar examination response is empty")]
    EmptyResponse,

    #[error("the Registrar examination page must have a text/html content type")]
    InvalidHtmlContentType,

    #[error("the Registrar examination JSONP response must not have a text/html content type")]
    JsonpHtmlContentType,

    #[error("the Registrar examination JSONP response has an unsupported content type")]
    InvalidJsonpContentType,

    #[error("the Registrar HTML does not contain the exact examination table header")]
    MissingExamTable,

    #[error("the Registrar examination HTML is malformed: {reason}")]
    MalformedHtml { reason: &'static str },

    #[error("the Registrar examination row {row} is invalid: {reason}")]
    InvalidHtmlRow { row: usize, reason: &'static str },

    #[error("the Registrar examination response returned a business failure: {message}")]
    BusinessFailure { message: String },

    #[error("the Registrar examination schedule field is invalid")]
    InvalidSchedule,

    #[error("the Registrar JSONP envelope is invalid: {reason}")]
    InvalidJsonp { reason: &'static str },

    #[error("the Registrar JSONP callback does not match the request")]
    JsonpCallbackMismatch,

    #[error("the Registrar JSONP payload is not an array")]
    JsonpPayloadNotArray,

    #[error("the Registrar JSONP record {index} is not an object")]
    JsonpRecordNotObject { index: usize },

    #[error("the Registrar JSONP record {index} is missing or has a non-text {field} field")]
    JsonpMissingField { index: usize, field: &'static str },

    #[error("the Registrar JSONP record {index} has an invalid {field} field")]
    JsonpInvalidField { index: usize, field: &'static str },

    #[error("the Registrar JSONP record {index} does not match the requested course")]
    JsonpCourseMismatch { index: usize },
}

/// Reports whether the public page lookup has an independently verified
/// contract for at least one academic stage.
pub const fn examination_lookup_is_configured() -> bool {
    true
}

/// Compatibility profile for the confirmed undergraduate page route.
///
/// This profile predates [`RegistrarVerifiedExamPageProfile`], so it remains
/// available for existing callers while delegating URL and response proof to
/// the same independently verified contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrarExamPageProfile {
    stage: RegistrarExamStage,
}

impl RegistrarExamPageProfile {
    pub const fn undergraduate() -> Self {
        Self {
            stage: RegistrarExamStage::Undergraduate,
        }
    }

    pub fn for_stage(stage: RegistrarExamStage) -> Result<Self, RegistrarExamError> {
        match stage {
            RegistrarExamStage::Undergraduate => Ok(Self::undergraduate()),
            RegistrarExamStage::Graduate => Err(RegistrarExamError::UnsupportedGraduateRoute),
        }
    }

    pub const fn stage(self) -> RegistrarExamStage {
        self.stage
    }

    pub const fn path(self) -> &'static str {
        REGISTRAR_EXAM_PATH
    }

    pub const fn action(self) -> &'static str {
        REGISTRAR_EXAM_ACTION
    }

    pub const fn is_configured(self) -> bool {
        matches!(self.stage, RegistrarExamStage::Undergraduate)
    }

    /// Returns the confirmed undergraduate page request shape.
    pub fn request(self) -> RegistrarExamPageRequest {
        RegistrarExamPageRequest {
            method: Method::GET,
            path: REGISTRAR_EXAM_PATH,
            query: [
                ("url", REGISTRAR_EXAM_PAGE_RETURN_URL),
                ("m", REGISTRAR_EXAM_ACTION),
            ],
        }
    }
}

/// The confirmed undergraduate page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarExamPageRequest {
    pub method: Method,
    pub path: &'static str,
    pub query: [(&'static str, &'static str); 2],
}

impl RegistrarExamPageRequest {
    pub fn endpoint_url(&self, base_url: &str) -> Result<Url, RegistrarExamError> {
        let verified = RegistrarVerifiedExamPageRequest {
            method: self.method.clone(),
            path: self.path,
            query: self.query,
        };
        verified.endpoint_url(base_url)
    }
}

/// A strictly validated academic term used by the per-course JSONP API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrarExamTerm {
    start_year: u16,
    end_year: u16,
    part: u8,
}

impl RegistrarExamTerm {
    pub fn new(start_year: u16, end_year: u16, part: u8) -> Result<Self, RegistrarExamError> {
        let valid_years = (1000..=9998).contains(&start_year)
            && (1001..=9999).contains(&end_year)
            && end_year == start_year + 1;
        if !valid_years || !(1..=3).contains(&part) {
            return Err(RegistrarExamError::InvalidTerm);
        }

        Ok(Self {
            start_year,
            end_year,
            part,
        })
    }

    pub fn parse(value: &str) -> Result<Self, RegistrarExamError> {
        let mut pieces = value.split('-');
        let Some(start) = pieces.next() else {
            return Err(RegistrarExamError::InvalidTerm);
        };
        let Some(end) = pieces.next() else {
            return Err(RegistrarExamError::InvalidTerm);
        };
        let Some(part) = pieces.next() else {
            return Err(RegistrarExamError::InvalidTerm);
        };
        if pieces.next().is_some()
            || start.len() != 4
            || end.len() != 4
            || part.len() != 1
            || !start.bytes().all(|byte| byte.is_ascii_digit())
            || !end.bytes().all(|byte| byte.is_ascii_digit())
            || !part.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(RegistrarExamError::InvalidTerm);
        }

        let start_year = start
            .parse::<u16>()
            .map_err(|_| RegistrarExamError::InvalidTerm)?;
        let end_year = end
            .parse::<u16>()
            .map_err(|_| RegistrarExamError::InvalidTerm)?;
        let part = part
            .parse::<u8>()
            .map_err(|_| RegistrarExamError::InvalidTerm)?;
        Self::new(start_year, end_year, part)
    }

    pub const fn start_year(self) -> u16 {
        self.start_year
    }

    pub const fn end_year(self) -> u16 {
        self.end_year
    }

    pub const fn part(self) -> u8 {
        self.part
    }
}

impl fmt::Display for RegistrarExamTerm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:04}-{}",
            self.start_year, self.end_year, self.part
        )
    }
}

impl FromStr for RegistrarExamTerm {
    type Err = RegistrarExamError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Parameters for the per-course examination JSONP request.
///
/// This is the request shape used by the existing Registrar page's AJAX
/// action.  It is kept separate from the page request because the two
/// endpoints return different representations and must be parsed with
/// different response contracts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarExamCourseQuery {
    course_code: String,
    course_sequence: String,
    term: RegistrarExamTerm,
    callback: String,
}

impl RegistrarExamCourseQuery {
    pub fn new(
        course_code: &str,
        course_sequence: &str,
        term: RegistrarExamTerm,
        callback: &str,
    ) -> Result<Self, RegistrarExamError> {
        if !valid_course_code(course_code) {
            return Err(RegistrarExamError::InvalidCourseCode);
        }
        if !valid_course_sequence(course_sequence) {
            return Err(RegistrarExamError::InvalidCourseSequence);
        }
        if !valid_jsonp_callback(callback) {
            return Err(RegistrarExamError::InvalidCallback);
        }

        Ok(Self {
            course_code: course_code.to_owned(),
            course_sequence: course_sequence.to_owned(),
            term,
            callback: callback.to_owned(),
        })
    }

    pub fn course_code(&self) -> &str {
        &self.course_code
    }

    pub fn course_sequence(&self) -> &str {
        &self.course_sequence
    }

    pub const fn term(&self) -> RegistrarExamTerm {
        self.term
    }

    pub fn callback(&self) -> &str {
        &self.callback
    }

    pub const fn is_configured(&self) -> bool {
        true
    }

    /// Returns the confirmed per-course request shape.
    pub fn request(&self) -> RegistrarExamCourseRequest {
        RegistrarExamCourseRequest {
            method: Method::GET,
            path: REGISTRAR_EXAM_PATH,
            query: vec![
                ("m".to_owned(), REGISTRAR_EXAM_ACTION.to_owned()),
                ("kch".to_owned(), self.course_code.clone()),
                ("kxh".to_owned(), self.course_sequence.clone()),
                ("p_xnxq".to_owned(), self.term.to_string()),
                ("jsoncallback".to_owned(), self.callback.clone()),
            ],
        }
    }
}

/// A per-course JSONP request with its exact wire query.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistrarExamCourseRequest {
    pub method: Method,
    pub path: &'static str,
    pub query: Vec<(String, String)>,
}

impl fmt::Debug for RegistrarExamCourseRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistrarExamCourseRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .finish()
    }
}

impl RegistrarExamCourseRequest {
    fn validate(&self) -> Result<(), RegistrarExamError> {
        if self.method != Method::GET
            || self.path != REGISTRAR_EXAM_PATH
            || self.query.len() != 5
            || self.query[0] != ("m".to_owned(), REGISTRAR_EXAM_ACTION.to_owned())
            || self.query[1].0 != "kch"
            || self.query[2].0 != "kxh"
            || self.query[3].0 != "p_xnxq"
            || self.query[4].0 != "jsoncallback"
            || !valid_course_code(&self.query[1].1)
            || !valid_course_sequence(&self.query[2].1)
            || RegistrarExamTerm::parse(&self.query[3].1).is_err()
            || !valid_jsonp_callback(&self.query[4].1)
        {
            return Err(RegistrarExamError::InvalidRequest {
                reason: "the course request does not match the confirmed JSONP contract",
            });
        }
        Ok(())
    }

    pub fn endpoint_url(&self, base_url: &str) -> Result<Url, RegistrarExamError> {
        self.validate()?;
        let base = verified_exam_origin(base_url)?;
        let mut endpoint = resolve_mapped_endpoint(&base, self.path).map_err(|_| {
            RegistrarExamError::InvalidBaseUrl {
                reason: "the examination path cannot be joined to the base URL",
            }
        })?;
        if !verified_exam_same_origin(&base, &endpoint) || !path_is_within_base(&base, &endpoint) {
            return Err(RegistrarExamError::UnexpectedOrigin);
        }
        {
            let mut query = endpoint.query_pairs_mut();
            for (name, value) in &self.query {
                query.append_pair(name, value);
            }
        }
        Ok(endpoint)
    }

    pub fn callback(&self) -> Option<&str> {
        self.query
            .iter()
            .find(|(name, _)| name == "jsoncallback")
            .map(|(_, value)| value.as_str())
    }
}

/// Text response metadata retained for the legacy and verified transport
/// boundaries. The body is parsed only by an enabled profile with a complete
/// request contract.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistrarExamHttpResponse {
    pub status: StatusCode,
    pub final_url: Url,
    pub content_type: Option<String>,
    pub body: String,
}

impl RegistrarExamHttpResponse {
    pub fn new(
        status: StatusCode,
        final_url: Url,
        content_type: Option<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            status,
            final_url,
            content_type,
            body: body.into(),
        }
    }
}

impl fmt::Debug for RegistrarExamHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut redacted_url = self.final_url.clone();
        redacted_url.set_query(None);
        redacted_url.set_fragment(None);
        formatter
            .debug_struct("RegistrarExamHttpResponse")
            .field("status", &self.status)
            .field("final_url", &redacted_url)
            .field("content_type", &self.content_type)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrarExamRecordSource {
    HtmlPage,
    CourseJsonp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrarExamWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrarExamSession {
    Morning,
    Afternoon,
    Evening,
}

/// Exam schedule DTO shared by the public page parser and the per-course JSONP
/// API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarExamSchedule {
    pub month: u8,
    pub day: u8,
    pub weekday: RegistrarExamWeekday,
    pub session: RegistrarExamSession,
    pub raw: String,
}

impl RegistrarExamSchedule {
    /// Parses the exact schedule grammar used by the verified examination
    /// page: `MM.DD` followed by a Chinese weekday and a session label.
    /// Keeping this parser strict prevents a free-form string from becoming a
    /// fabricated examination record.
    pub fn parse(value: &str) -> Result<Self, RegistrarExamError> {
        parse_verified_exam_schedule(value).ok_or(RegistrarExamError::InvalidSchedule)
    }

    pub fn date_in_year(&self, year: i32) -> Result<NaiveDate, RegistrarExamError> {
        NaiveDate::from_ymd_opt(year, u32::from(self.month), u32::from(self.day))
            .ok_or(RegistrarExamError::InvalidSchedule)
    }
}

/// Exam record DTO shared by the public undergraduate page parser and the
/// per-course JSONP API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarExamRecord {
    pub source: RegistrarExamRecordSource,
    pub course_code: String,
    pub course_sequence: String,
    pub course_name: String,
    pub schedule: RegistrarExamSchedule,
    pub location: String,
    pub department: Option<String>,
    pub category: Option<String>,
    pub instructor: Option<String>,
    pub headcount: Option<u32>,
}

/// Parses the confirmed undergraduate page response through the public
/// compatibility boundary. The implementation delegates to the same strict
/// origin/path/query/status/content-type/table proof as the named verified
/// profile below.
pub fn parse_exam_page_response(
    registrar_base_url: &str,
    response: &RegistrarExamHttpResponse,
) -> Result<Vec<RegistrarExamRecord>, RegistrarExamError> {
    let request = RegistrarVerifiedExamPageProfile::undergraduate().request();
    parse_verified_exam_page_response(registrar_base_url, &request, response)
}

/// Parses the confirmed undergraduate nine-column examination table. Calendar
/// and grade tables do not satisfy its exact header contract.
pub fn parse_exam_page_html(body: &str) -> Result<Vec<RegistrarExamRecord>, RegistrarExamError> {
    parse_verified_exam_page_html(body)
}

/// Parses one per-course JSONP response.
///
/// The request and response are checked together.  A syntactically valid
/// JSONP response for another course, an error envelope, or a login page is
/// never converted into an empty examination list.
pub fn parse_exam_course_response(
    registrar_base_url: &str,
    request: &RegistrarExamCourseRequest,
    response: &RegistrarExamHttpResponse,
) -> Result<Vec<RegistrarExamRecord>, RegistrarExamError> {
    request.validate()?;
    let base = verified_exam_origin(registrar_base_url)?;
    let expected_url = request.endpoint_url(registrar_base_url)?;
    if exam_status_is_authentication_failure(response.status) {
        return Err(RegistrarExamError::AuthenticationRequired);
    }
    if verified_exam_looks_like_login(&response.final_url, &response.body) {
        return Err(RegistrarExamError::AuthenticationRequired);
    }
    if !verified_exam_same_origin(&base, &response.final_url)
        || !path_is_within_base(&base, &response.final_url)
    {
        return Err(RegistrarExamError::UnexpectedOrigin);
    }
    if response.final_url.path() != expected_url.path() {
        return Err(RegistrarExamError::UnexpectedPath);
    }
    if response.final_url.query() != expected_url.query() {
        return Err(RegistrarExamError::MissingConfirmedQuery);
    }
    if response.status != StatusCode::OK {
        return Err(RegistrarExamError::HttpStatus {
            status: response.status.as_u16(),
        });
    }
    validate_jsonp_content_type(response.content_type.as_deref())?;

    parse_exam_course_jsonp(
        &response.body,
        request
            .callback()
            .ok_or(RegistrarExamError::InvalidRequest {
                reason: "the course request is missing its JSONP callback",
            })?,
        request,
    )
}

/// Parses the confirmed per-course JSONP envelope and record fields.
pub fn parse_exam_course_jsonp(
    body: &str,
    expected_callback: &str,
    request: &RegistrarExamCourseRequest,
) -> Result<Vec<RegistrarExamRecord>, RegistrarExamError> {
    request.validate()?;
    if !valid_jsonp_callback(expected_callback) {
        return Err(RegistrarExamError::InvalidCallback);
    }
    if request.callback() != Some(expected_callback) {
        return Err(RegistrarExamError::JsonpCallbackMismatch);
    }

    let payload = parse_exam_jsonp_value(body, expected_callback)?;
    if jsonp_login_failure(&payload) {
        return Err(RegistrarExamError::AuthenticationRequired);
    }
    if let Some(message) = jsonp_business_failure(&payload) {
        return Err(RegistrarExamError::BusinessFailure { message });
    }

    let records = payload
        .as_array()
        .ok_or(RegistrarExamError::JsonpPayloadNotArray)?;
    let expected_course = &request.query[1].1;
    let expected_sequence = &request.query[2].1;
    records
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let object = value
                .as_object()
                .ok_or(RegistrarExamError::JsonpRecordNotObject { index })?;
            let course_code = required_jsonp_text(object, index, "kch")?;
            let course_sequence = required_jsonp_text(object, index, "kxh")?;
            if course_code != *expected_course || course_sequence != *expected_sequence {
                return Err(RegistrarExamError::JsonpCourseMismatch { index });
            }
            if !valid_course_code(&course_code) {
                return Err(RegistrarExamError::JsonpInvalidField {
                    index,
                    field: "kch",
                });
            }
            if !valid_course_sequence(&course_sequence) {
                return Err(RegistrarExamError::JsonpInvalidField {
                    index,
                    field: "kxh",
                });
            }
            let course_name = required_jsonp_text(object, index, "kcm")?;
            let schedule_text = required_jsonp_text(object, index, "ksrq")?;
            let schedule = parse_verified_exam_schedule(&schedule_text)
                .ok_or(RegistrarExamError::InvalidSchedule)?;
            let location = required_jsonp_text(object, index, "ksdd")?;

            Ok(RegistrarExamRecord {
                source: RegistrarExamRecordSource::CourseJsonp,
                course_code,
                course_sequence,
                course_name,
                schedule,
                location,
                department: optional_jsonp_text(object, &["kkdw", "kkdwmc", "department"]),
                category: optional_jsonp_text(object, &["kclb", "kcfl", "category"]),
                instructor: optional_jsonp_text(object, &["jsxm", "teacher", "instructor"]),
                headcount: optional_jsonp_u32(object, &["rs", "人数", "headcount"]),
            })
        })
        .collect()
}

fn valid_course_code(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_course_sequence(value: &str) -> bool {
    (1..=3).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_jsonp_callback(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    value.split('.').all(|segment| {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
            return false;
        }
        chars.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    })
}

fn validate_jsonp_content_type(value: Option<&str>) -> Result<(), RegistrarExamError> {
    let Some(value) = value else {
        // A few legacy Registrar fixtures omit Content-Type.  The envelope
        // and payload checks below are the authoritative proof in that case.
        return Ok(());
    };

    let media_type = value
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if media_type == "text/html" {
        return Err(RegistrarExamError::JsonpHtmlContentType);
    }
    if matches!(
        media_type.as_str(),
        "application/javascript"
            | "text/javascript"
            | "application/x-javascript"
            | "application/ecmascript"
            | "text/ecmascript"
            | "application/json"
            | "text/plain"
    ) {
        return Ok(());
    }
    Err(RegistrarExamError::InvalidJsonpContentType)
}

/// Parses a JSONP invocation without accepting arbitrary JavaScript around the
/// JSON value.  The endpoint is legacy JSONP, but treating the body as a small
/// envelope keeps a login page, a second invocation, or a script suffix from
/// becoming an apparently empty examination result.
fn parse_exam_jsonp_value(
    body: &str,
    expected_callback: &str,
) -> Result<Value, RegistrarExamError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(RegistrarExamError::EmptyResponse);
    }
    if !valid_jsonp_callback(expected_callback) {
        return Err(RegistrarExamError::InvalidCallback);
    }

    let Some(open) = trimmed.find('(') else {
        return Err(RegistrarExamError::InvalidJsonp {
            reason: "the JSONP invocation has no opening parenthesis",
        });
    };
    let actual_callback = trimmed[..open].trim();
    if !valid_jsonp_callback(actual_callback) {
        return Err(RegistrarExamError::InvalidJsonp {
            reason: "the JSONP callback is not a JavaScript identifier path",
        });
    }
    if actual_callback != expected_callback {
        return Err(RegistrarExamError::JsonpCallbackMismatch);
    }
    if !trimmed.ends_with(')') || open + 1 >= trimmed.len() {
        return Err(RegistrarExamError::InvalidJsonp {
            reason: "the JSONP invocation is not a single parenthesized value",
        });
    }

    let payload_text = trimmed[open + 1..trimmed.len() - 1].trim();
    if payload_text.is_empty() {
        return Err(RegistrarExamError::InvalidJsonp {
            reason: "the JSONP payload is empty",
        });
    }
    serde_json::from_str(payload_text).map_err(|_| RegistrarExamError::InvalidJsonp {
        reason: "the JSONP payload is not valid JSON",
    })
}

fn jsonp_login_failure(value: &Value) -> bool {
    match value {
        Value::String(value) => text_indicates_jsonp_login_failure(value),
        Value::Array(values) => values.iter().any(jsonp_login_failure),
        Value::Object(object) => {
            for field in [
                "message",
                "msg",
                "error",
                "errorMessage",
                "error_message",
                "reason",
                "detail",
            ] {
                if object
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(text_indicates_jsonp_login_failure)
                {
                    return true;
                }
            }

            for field in ["code", "status", "statusCode", "status_code", "httpCode"] {
                let Some(value) = object.get(field) else {
                    continue;
                };
                let is_auth_status = match value {
                    Value::Number(value) => value
                        .as_i64()
                        .is_some_and(|value| value == 401 || value == 403),
                    Value::String(value) => {
                        matches!(value.trim(), "401" | "403")
                            || text_indicates_jsonp_login_failure(value)
                    }
                    _ => false,
                };
                if is_auth_status {
                    return true;
                }
            }

            for field in [
                "isLogin",
                "is_logged_in",
                "loggedIn",
                "logged_in",
                "authenticated",
            ] {
                if matches!(object.get(field), Some(Value::Bool(false))) {
                    return true;
                }
            }

            // Some deployments put the failure envelope under `data` or
            // `result`; recursively inspect nested values so those responses
            // do not fall through to the array-shape check.
            ["data", "result", "response"]
                .iter()
                .any(|field| object.get(*field).is_some_and(jsonp_login_failure))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn text_indicates_jsonp_login_failure(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    [
        "请先登录",
        "请先登陆",
        "未登录",
        "未登陆",
        "登录失败",
        "登陆失败",
        "认证失败",
        "登录超时",
        "登陆超时",
        "登录失效",
        "登陆失效",
        "重新登录",
        "重新登陆",
        "统一身份认证",
        "统一认证",
        "webvpn",
        "j_acegi_login",
        "服务票据",
        "authentication required",
        "login required",
        "session expired",
        "session timeout",
        "unauthorized",
        "not authorized",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Returns a stable business error label.  Upstream messages are deliberately
/// not copied into the public error so a malformed response cannot echo a
/// credential, ticket, or other sensitive value into logs or UI.
fn jsonp_business_failure(value: &Value) -> Option<String> {
    let Value::Object(object) = value else {
        return None;
    };

    let explicit_failure = object.get("success").is_some_and(jsonp_negative_flag)
        || object.get("ok").is_some_and(jsonp_negative_flag)
        || object.get("status").is_some_and(jsonp_status_failure)
        || object.get("result").is_some_and(jsonp_status_failure)
        || ["code", "statusCode", "status_code", "httpCode"]
            .iter()
            .any(|field| object.get(*field).is_some_and(jsonp_http_failure))
        || object.get("error").is_some_and(|value| {
            !value.is_null() && !value.as_str().is_some_and(|value| value.trim().is_empty())
        });

    explicit_failure.then(|| "upstream examination business request failed".to_owned())
}

fn jsonp_negative_flag(value: &Value) -> bool {
    match value {
        Value::Bool(value) => !*value,
        Value::Number(value) => value.as_i64() == Some(0),
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "false" | "no" | "fail" | "failed" | "failure" | "error"
        ),
        _ => false,
    }
}

fn jsonp_status_failure(value: &Value) -> bool {
    match value {
        Value::Bool(value) => !*value,
        Value::Number(value) => value
            .as_i64()
            .is_some_and(|status| (400..=599).contains(&status)),
        Value::String(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "error" | "failed" | "failure" | "fail" | "false" | "no"
            ) || normalized
                .parse::<i64>()
                .ok()
                .is_some_and(|status| (400..=599).contains(&status))
        }
        _ => false,
    }
}

fn jsonp_http_failure(value: &Value) -> bool {
    match value {
        Value::Number(value) => value
            .as_i64()
            .is_some_and(|status| (400..=599).contains(&status)),
        Value::String(value) => value
            .trim()
            .parse::<i64>()
            .ok()
            .is_some_and(|status| (400..=599).contains(&status)),
        _ => false,
    }
}

fn required_jsonp_text(
    object: &serde_json::Map<String, Value>,
    index: usize,
    field: &'static str,
) -> Result<String, RegistrarExamError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(RegistrarExamError::JsonpMissingField { index, field })
}

fn optional_jsonp_text(object: &serde_json::Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn optional_jsonp_u32(object: &serde_json::Map<String, Value>, fields: &[&str]) -> Option<u32> {
    let Some(value) = fields.iter().find_map(|field| object.get(*field)) else {
        return None;
    };

    // Headcount is optional in both observed representations.  A malformed
    // optional value must not erase an otherwise complete exam record or be
    // turned into a fabricated zero; it is represented as `None` instead.
    match value {
        Value::Number(value) => value.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() || matches!(value, "-" | "—" | "–" | "－" | "null") {
                return None;
            }
            value.parse::<u32>().ok()
        }
        _ => None,
    }
}

// -------------------------------------------------------------------------
// Independently evidenced examination page
// -------------------------------------------------------------------------

/// The current page contract confirmed by the 2026 public JWCH adapter audit.
/// The compatibility [`RegistrarExamPageProfile`] delegates to this same
/// request and response contract. The evidence and its limits are recorded in
/// `docs/registrar-exam-read.md`.
pub const VERIFIED_EXAM_PAGE_PATH: &str = "/jxmh.do";
pub const VERIFIED_EXAM_PAGE_ACTION: &str = "bks_ksSearch";

/// Only the undergraduate page has a current independent route and fixed
/// response-table proof.  No graduate page is enabled until the same evidence
/// exists for a graduate deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrarVerifiedExamPageProfile {
    stage: RegistrarExamStage,
}

impl RegistrarVerifiedExamPageProfile {
    pub const fn undergraduate() -> Self {
        Self {
            stage: RegistrarExamStage::Undergraduate,
        }
    }

    pub fn for_stage(stage: RegistrarExamStage) -> Result<Self, RegistrarExamError> {
        match stage {
            RegistrarExamStage::Undergraduate => Ok(Self::undergraduate()),
            RegistrarExamStage::Graduate => Err(RegistrarExamError::UnsupportedGraduateRoute),
        }
    }

    pub const fn stage(self) -> RegistrarExamStage {
        self.stage
    }

    pub const fn is_configured(self) -> bool {
        matches!(self.stage, RegistrarExamStage::Undergraduate)
    }

    pub const fn request(self) -> RegistrarVerifiedExamPageRequest {
        RegistrarVerifiedExamPageRequest {
            method: Method::GET,
            path: VERIFIED_EXAM_PAGE_PATH,
            query: [
                ("url", VERIFIED_EXAM_PAGE_PATH),
                ("m", VERIFIED_EXAM_PAGE_ACTION),
            ],
        }
    }
}

/// The exact request shape used by the verified undergraduate examination
/// page.  The query contains no credentials and is safe to inspect in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarVerifiedExamPageRequest {
    pub method: Method,
    pub path: &'static str,
    pub query: [(&'static str, &'static str); 2],
}

impl RegistrarVerifiedExamPageRequest {
    fn is_canonical(&self) -> bool {
        self.method == Method::GET
            && self.path == VERIFIED_EXAM_PAGE_PATH
            && self.query
                == [
                    ("url", VERIFIED_EXAM_PAGE_PATH),
                    ("m", VERIFIED_EXAM_PAGE_ACTION),
                ]
    }

    pub fn endpoint_url(&self, base_url: &str) -> Result<Url, RegistrarExamError> {
        if !self.is_canonical() {
            return Err(RegistrarExamError::InvalidRequest {
                reason: "the verified page request does not match the confirmed GET contract",
            });
        }

        let base = verified_exam_origin(base_url)?;

        let mut endpoint = resolve_mapped_endpoint(&base, self.path).map_err(|_| {
            RegistrarExamError::InvalidBaseUrl {
                reason: "the examination path cannot be joined to the base URL",
            }
        })?;
        if !verified_exam_same_origin(&base, &endpoint) || !path_is_within_base(&base, &endpoint) {
            return Err(RegistrarExamError::UnexpectedOrigin);
        }
        {
            let mut query = endpoint.query_pairs_mut();
            for (name, value) in self.query {
                query.append_pair(name, value);
            }
        }
        Ok(endpoint)
    }
}

/// Parses a verified examination page response after checking origin, route,
/// query, status, content type, and authentication evidence.
pub fn parse_verified_exam_page_response(
    registrar_base_url: &str,
    request: &RegistrarVerifiedExamPageRequest,
    response: &RegistrarExamHttpResponse,
) -> Result<Vec<RegistrarExamRecord>, RegistrarExamError> {
    if !request.is_canonical() {
        return Err(RegistrarExamError::InvalidRequest {
            reason: "the verified page request does not match the confirmed GET contract",
        });
    }

    let base = verified_exam_origin(registrar_base_url)?;
    let expected_url = request.endpoint_url(registrar_base_url)?;
    if exam_status_is_authentication_failure(response.status) {
        return Err(RegistrarExamError::AuthenticationRequired);
    }

    if verified_exam_looks_like_login(&response.final_url, &response.body) {
        return Err(RegistrarExamError::AuthenticationRequired);
    }

    if !verified_exam_same_origin(&base, &response.final_url)
        || !path_is_within_base(&base, &response.final_url)
    {
        return Err(RegistrarExamError::UnexpectedOrigin);
    }

    if response.final_url.path() != expected_url.path() {
        return Err(RegistrarExamError::UnexpectedPath);
    }

    // Compare the raw query as well as its decoded meaning.  This preserves the
    // exact wire contract (`url=%2Fjxmh.do&m=bks_ksSearch`) and prevents a
    // redirect from silently dropping ordering or percent encoding.
    if response.final_url.query() != expected_url.query() {
        return Err(RegistrarExamError::MissingConfirmedQuery);
    }

    // The confirmed page is a normal 200 HTML document.  A different 2xx code
    // is not promoted to a possibly empty result.
    if response.status != StatusCode::OK {
        return Err(RegistrarExamError::HttpStatus {
            status: response.status.as_u16(),
        });
    }

    let is_html = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/html"));
    if !is_html {
        return Err(RegistrarExamError::InvalidHtmlContentType);
    }

    parse_verified_exam_page_html(&response.body)
}

/// Parses the fixed nine-column examination table independently of the
/// calendar JSONP and grade-report parsers.  A header-only table is a valid
/// empty result; a missing table or malformed row is a protocol error.
pub fn parse_verified_exam_page_html(
    body: &str,
) -> Result<Vec<RegistrarExamRecord>, RegistrarExamError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        if body.trim().is_empty() {
            return Err(RegistrarExamError::EmptyResponse);
        }
        if verified_exam_body_looks_like_login(body) {
            return Err(RegistrarExamError::AuthenticationRequired);
        }

        let tables = extract_verified_exam_tables(body)?;
        let expected_header = [
            "开课系",
            "课程号",
            "课序号",
            "课程名",
            "课程分类",
            "教师",
            "人数",
            "考试日期",
            "考场",
        ];
        let Some(table) = tables.into_iter().find(|table| {
            table.first().is_some_and(|row| {
                row.len() == expected_header.len()
                    && row
                        .iter()
                        .map(String::as_str)
                        .eq(expected_header.into_iter())
            })
        }) else {
            if let Some(message) = verified_exam_business_failure(body) {
                return Err(RegistrarExamError::BusinessFailure {
                    message: message.to_owned(),
                });
            }
            return Err(RegistrarExamError::MissingExamTable);
        };

        let mut records = Vec::with_capacity(table.len().saturating_sub(1));
        for (row_index, row) in table.into_iter().enumerate().skip(1) {
            if row.iter().all(|cell| cell.is_empty()) {
                continue;
            }
            if row.len() != expected_header.len() {
                return Err(RegistrarExamError::InvalidHtmlRow {
                    row: row_index,
                    reason: "the row width differs from the confirmed examination header",
                });
            }

            let course_code = exam_required_cell(&row, row_index, 1, "course code")?;
            if !valid_course_code(&course_code) {
                return Err(RegistrarExamError::InvalidHtmlRow {
                    row: row_index,
                    reason: "course code is not eight ASCII digits",
                });
            }
            let course_sequence = exam_required_cell(&row, row_index, 2, "course sequence")?;
            if !valid_course_sequence(&course_sequence) {
                return Err(RegistrarExamError::InvalidHtmlRow {
                    row: row_index,
                    reason: "course sequence is not one to three ASCII digits",
                });
            }
            let course_name = exam_required_cell(&row, row_index, 3, "course name")?;
            let schedule_text = exam_required_cell(&row, row_index, 7, "exam schedule")?;
            let schedule = parse_verified_exam_schedule(&schedule_text).ok_or(
                RegistrarExamError::InvalidHtmlRow {
                    row: row_index,
                    reason: "exam schedule is not MM.DD weekday and session",
                },
            )?;
            let location = exam_required_cell(&row, row_index, 8, "exam location")?;

            let headcount = parse_optional_exam_headcount(&row[6]);

            records.push(RegistrarExamRecord {
                source: RegistrarExamRecordSource::HtmlPage,
                course_code,
                course_sequence,
                course_name,
                schedule,
                location,
                department: non_empty_exam_cell(&row[0]),
                category: non_empty_exam_cell(&row[4]),
                instructor: non_empty_exam_cell(&row[5]),
                headcount,
            });
        }

        Ok(records)
    })
}

fn exam_required_cell(
    row: &[String],
    row_index: usize,
    column: usize,
    field: &'static str,
) -> Result<String, RegistrarExamError> {
    row.get(column)
        .map(|value| normalize_exam_text(value))
        .filter(|value| !value.is_empty())
        .ok_or(RegistrarExamError::InvalidHtmlRow {
            row: row_index,
            reason: match field {
                "course code" => "course code is empty",
                "course sequence" => "course sequence is empty",
                "course name" => "course name is empty",
                "exam schedule" => "exam schedule is empty",
                "exam location" => "exam location is empty",
                _ => "required examination cell is empty",
            },
        })
}

fn non_empty_exam_cell(value: &str) -> Option<String> {
    let value = normalize_exam_text(value);
    (!value.is_empty()).then_some(value)
}

fn parse_optional_exam_headcount(value: &str) -> Option<u32> {
    let value = normalize_exam_text(value);
    if value.is_empty() || matches!(value.as_str(), "-" | "—" | "–" | "－" | "null") {
        return None;
    }
    value.parse::<u32>().ok()
}

fn parse_verified_exam_schedule(value: &str) -> Option<RegistrarExamSchedule> {
    let raw = normalize_exam_text(value);
    let (schedule_without_session, session_text) =
        ["上午", "下午", "晚上"].into_iter().find_map(|session| {
            raw.strip_suffix(session)
                .map(|prefix| (prefix.trim_end(), session))
        })?;

    let mut cursor = 0;
    let (month, after_month) = parse_exam_ascii_decimal(schedule_without_session, cursor)?;
    cursor = after_month;
    if schedule_without_session.as_bytes().get(cursor) != Some(&b'.') {
        return None;
    }
    cursor += 1;
    let (day, after_day) = parse_exam_ascii_decimal(schedule_without_session, cursor)?;
    cursor = after_day;
    let weekday_text = schedule_without_session[cursor..].trim();
    let weekday = parse_verified_weekday(weekday_text)?;
    let session = match session_text {
        "上午" => RegistrarExamSession::Morning,
        "下午" => RegistrarExamSession::Afternoon,
        "晚上" => RegistrarExamSession::Evening,
        _ => return None,
    };
    NaiveDate::from_ymd_opt(2000, u32::from(month), u32::from(day))?;
    Some(RegistrarExamSchedule {
        month,
        day,
        weekday,
        session,
        raw,
    })
}

fn parse_exam_ascii_decimal(value: &str, start: usize) -> Option<(u8, usize)> {
    let end = start.checked_add(2)?;
    if end > value.len() || !value.as_bytes()[start..end].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let number = value[start..end].parse::<u8>().ok()?;
    Some((number, end))
}

fn parse_verified_weekday(value: &str) -> Option<RegistrarExamWeekday> {
    let value = value
        .strip_prefix("星期")
        .or_else(|| value.strip_prefix("周"))
        .unwrap_or(value);
    match value {
        "一" => Some(RegistrarExamWeekday::Monday),
        "二" => Some(RegistrarExamWeekday::Tuesday),
        "三" => Some(RegistrarExamWeekday::Wednesday),
        "四" => Some(RegistrarExamWeekday::Thursday),
        "五" => Some(RegistrarExamWeekday::Friday),
        "六" => Some(RegistrarExamWeekday::Saturday),
        "日" | "天" => Some(RegistrarExamWeekday::Sunday),
        _ => None,
    }
}

fn verified_exam_origin(base_url: &str) -> Result<Url, RegistrarExamError> {
    let base = Url::parse(base_url).map_err(|_| RegistrarExamError::InvalidBaseUrl {
        reason: "expected an http(s) origin",
    })?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(RegistrarExamError::InvalidBaseUrl {
            reason: "expected an http(s) origin without userinfo, path, query, or fragment",
        });
    }
    Ok(base)
}

fn verified_exam_same_origin(base: &Url, candidate: &Url) -> bool {
    let is_registrar_downgrade = base.scheme() == "https"
        && candidate.scheme() == "http"
        && base.host_str() == Some("zhjw.cic.tsinghua.edu.cn")
        && base.port_or_known_default() == Some(443)
        && candidate.port_or_known_default() == Some(80);
    base.host_str() == candidate.host_str()
        && (base.scheme() == candidate.scheme() || is_registrar_downgrade)
        && (base.port_or_known_default() == candidate.port_or_known_default()
            || is_registrar_downgrade)
        && candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.fragment().is_none()
}

fn verified_exam_looks_like_login(final_url: &Url, body: &str) -> bool {
    let path = final_url.path().to_ascii_lowercase();
    path.contains("j_acegi_login")
        || path == "/login"
        || path.ends_with("/login")
        || path.contains("timeout")
        || path.contains("sso_fail")
        || verified_exam_body_looks_like_login(body)
}

fn verified_exam_body_looks_like_login(body: &str) -> bool {
    // Shared navigation and script templates may mention WebVPN, login
    // paths, or credential field names on a valid examination page. Those
    // strings alone are not proof of session expiry. Inspect actual forms
    // and visible failure notices, excluding comments and script/style text.
    let mut html = body.to_ascii_lowercase();
    while let Some(start) = html.find("<!--") {
        let end = html[start + 4..]
            .find("-->")
            .map(|offset| start + 4 + offset + 3)
            .unwrap_or(html.len());
        html.replace_range(start..end, " ");
    }
    for tag in ["script", "style"] {
        while let Some(open) = find_exam_open_tag(&html, tag, 0) {
            let end = find_exam_close_tag(&html, tag, open.end)
                .map(|close| close.end)
                .unwrap_or(html.len());
            html.replace_range(open.start..end, " ");
        }
    }
    let mut cursor = 0;
    while let Some(open) = find_exam_open_tag(&html, "form", cursor) {
        let end = find_exam_close_tag(&html, "form", open.end)
            .map(|close| close.end)
            .unwrap_or(html.len());
        let form = &html[open.start..end];
        if (form.contains("i_user") && form.contains("i_pass"))
            || (form.contains("j_username") && form.contains("j_password"))
            || (form.contains("password")
                && ["login", "登录", "统一认证", "webvpn"]
                    .iter()
                    .any(|marker| form.contains(marker)))
        {
            return true;
        }
        cursor = end;
    }
    let text = exam_cell_text(&html).unwrap_or_default();
    matches!(
        text.trim(),
        "统一身份认证" | "统一认证" | "清华大学webvpn" | "webvpn"
    ) || [
        "用户登陆超时",
        "登陆超时",
        "登录超时",
        "登录失效",
        "登录失败",
        "认证失败",
        "请先登录",
        "未登录",
        "重新登录",
        "session expired",
        "session timeout",
        "authentication required",
        "unauthorized",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn verified_exam_business_failure(body: &str) -> Option<&'static str> {
    let lower = body.to_ascii_lowercase();
    [
        ("查询失败", "upstream examination query failed"),
        ("系统错误", "upstream examination system error"),
        ("系统异常", "upstream examination system exception"),
        ("无权访问", "upstream examination access denied"),
        ("没有权限", "upstream examination access denied"),
        ("服务不可用", "upstream examination service unavailable"),
        ("暂时不可用", "upstream examination service unavailable"),
        ("网络失败", "upstream examination network failure"),
        (
            "service unavailable",
            "upstream examination service unavailable",
        ),
    ]
    .into_iter()
    .find_map(|(marker, message)| lower.contains(marker).then_some(message))
}

fn extract_verified_exam_tables(body: &str) -> Result<Vec<Vec<Vec<String>>>, RegistrarExamError> {
    let mut tables = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_exam_open_tag(body, "table", cursor) {
        let close = find_exam_close_tag(body, "table", open.end).ok_or(
            RegistrarExamError::MalformedHtml {
                reason: "unclosed examination table",
            },
        )?;
        let table_body = &body[open.end..close.start];
        tables.push(extract_verified_exam_rows(table_body)?);
        cursor = close.end;
    }
    Ok(tables)
}

fn extract_verified_exam_rows(table: &str) -> Result<Vec<Vec<String>>, RegistrarExamError> {
    let mut rows = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_exam_open_tag(table, "tr", cursor) {
        let close = find_exam_close_tag(table, "tr", open.end).ok_or(
            RegistrarExamError::MalformedHtml {
                reason: "unclosed examination row",
            },
        )?;
        let row = extract_verified_exam_cells(&table[open.end..close.start])?;
        if !row.is_empty() {
            rows.push(row);
        }
        cursor = close.end;
    }
    Ok(rows)
}

#[derive(Debug, Clone, Copy)]
struct ExamTagSpan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy)]
struct ExamCellSpan {
    tag: ExamTagSpan,
    header: bool,
}

fn extract_verified_exam_cells(row: &str) -> Result<Vec<String>, RegistrarExamError> {
    let mut cells = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_next_exam_cell(row, cursor) {
        let tag_name = if open.header { "th" } else { "td" };
        let close = find_exam_close_tag(row, tag_name, open.tag.end).ok_or(
            RegistrarExamError::MalformedHtml {
                reason: "unclosed examination cell",
            },
        )?;
        cells.push(exam_cell_text(&row[open.tag.end..close.start])?);
        cursor = close.end;
    }
    Ok(cells)
}

fn find_next_exam_cell(row: &str, from: usize) -> Option<ExamCellSpan> {
    let td = find_exam_open_tag(row, "td", from).map(|tag| ExamCellSpan { tag, header: false });
    let th = find_exam_open_tag(row, "th", from).map(|tag| ExamCellSpan { tag, header: true });
    match (td, th) {
        (Some(td), Some(th)) if td.tag.start < th.tag.start => Some(td),
        (Some(_), Some(th)) => Some(th),
        (Some(td), None) => Some(td),
        (None, Some(th)) => Some(th),
        (None, None) => None,
    }
}

fn exam_cell_text(value: &str) -> Result<String, RegistrarExamError> {
    let mut result = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let remaining = &value[cursor..];
        if remaining.starts_with('<') {
            let end =
                find_exam_tag_end(value, cursor).ok_or(RegistrarExamError::MalformedHtml {
                    reason: "unclosed tag inside examination cell",
                })?;
            let tag = &value[cursor..end];
            if exam_tag_name(tag).is_some_and(|name| matches!(name, "br" | "p" | "div" | "li")) {
                result.push(' ');
            }
            cursor = end;
            continue;
        }
        if remaining.starts_with('&') {
            if let Some(relative_end) = remaining.find(';') {
                let end = cursor + relative_end + 1;
                let entity = &value[cursor + 1..end - 1];
                if let Some(decoded) = decode_exam_entity(entity) {
                    result.push(decoded);
                } else {
                    result.push_str(&value[cursor..end]);
                }
                cursor = end;
                continue;
            }
        }
        let character = remaining
            .chars()
            .next()
            .ok_or(RegistrarExamError::MalformedHtml {
                reason: "invalid UTF-8 boundary inside examination cell",
            })?;
        result.push(character);
        cursor += character.len_utf8();
    }
    Ok(normalize_exam_text(&result))
}

fn normalize_exam_text(value: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() || character == '\u{a0}' {
            pending_space = !result.is_empty();
            continue;
        }
        if pending_space {
            result.push(' ');
            pending_space = false;
        }
        result.push(character);
    }
    result.trim().to_owned()
}

fn exam_status_is_authentication_failure(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

fn decode_exam_entity(entity: &str) -> Option<char> {
    match entity.to_ascii_lowercase().as_str() {
        "nbsp" => Some(' '),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        value if value.starts_with("#x") => u32::from_str_radix(&value[2..], 16)
            .ok()
            .and_then(char::from_u32),
        value if value.starts_with('#') => value[1..].parse().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn find_exam_open_tag(input: &str, wanted: &str, from: usize) -> Option<ExamTagSpan> {
    let mut cursor = from;
    while cursor < input.len() {
        let relative = input[cursor..].find('<')?;
        let start = cursor + relative;
        let mut name_start = start + 1;
        while name_start < input.len() && input.as_bytes()[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        if name_start >= input.len() || matches!(input.as_bytes()[name_start], b'/' | b'!' | b'?') {
            cursor = start + 1;
            continue;
        }
        let name_end = exam_ascii_name_end(input, name_start);
        if name_end > name_start
            && input[name_start..name_end].eq_ignore_ascii_case(wanted)
            && exam_tag_name_boundary(input, name_end)
        {
            let end = find_exam_tag_end(input, start)?;
            return Some(ExamTagSpan { start, end });
        }
        cursor = start + 1;
    }
    None
}

fn find_exam_close_tag(input: &str, wanted: &str, from: usize) -> Option<ExamTagSpan> {
    let mut cursor = from;
    while cursor < input.len() {
        let relative = input[cursor..].find("</")?;
        let start = cursor + relative;
        let name_start = start + 2;
        let name_end = exam_ascii_name_end(input, name_start);
        if name_end > name_start
            && input[name_start..name_end].eq_ignore_ascii_case(wanted)
            && exam_tag_name_boundary(input, name_end)
        {
            let end = find_exam_tag_end(input, start)?;
            return Some(ExamTagSpan { start, end });
        }
        cursor = start + 2;
    }
    None
}

fn find_exam_tag_end(input: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in input.as_bytes()[start..].iter().copied().enumerate() {
        match (quote, character) {
            (Some(expected), value) if value == expected => quote = None,
            (None, b'\'' | b'"') => quote = Some(character),
            (None, b'>') => return Some(start + offset + 1),
            _ => {}
        }
    }
    None
}

fn exam_ascii_name_end(input: &str, start: usize) -> usize {
    let mut end = start;
    while end < input.len() {
        let character = input.as_bytes()[end];
        if character.is_ascii_alphanumeric() || matches!(character, b':' | b'_' | b'-') {
            end += 1;
        } else {
            break;
        }
    }
    end
}

fn exam_tag_name_boundary(input: &str, position: usize) -> bool {
    position >= input.len()
        || input.as_bytes()[position].is_ascii_whitespace()
        || matches!(input.as_bytes()[position], b'>' | b'/')
}

fn exam_tag_name(tag: &str) -> Option<&str> {
    let mut start = 1;
    while start < tag.len() && tag.as_bytes()[start].is_ascii_whitespace() {
        start += 1;
    }
    if start < tag.len() && tag.as_bytes()[start] == b'/' {
        start += 1;
    }
    let end = exam_ascii_name_end(tag, start);
    (end > start).then(|| &tag[start..end])
}
