//! Read-only course announcements from Web Learning.
//!
//! Learn course announcements are a different service surface from INFO news.
//! The current public Learn client uses one POST endpoint for the active list
//! and one for expired announcements, with the course id encoded in the
//! `aoData` form field.  This module keeps that route/body contract and the
//! response proof in Rust.  It never falls back to INFO data or treats a
//! missing collection as an empty result.

use std::{collections::BTreeMap, fmt};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use reqwest::{StatusCode, Url, header::LOCATION};
use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    learn_client::{
        LearnClient, LearnClientError, LearnPageClassification, is_login_failure_message,
    },
    protocol::{CourseRole, ServiceId},
    session::BoundCsrfToken,
    transport::{CampusHttpTransport, TransportError},
    webvpn_url::endpoint_is_allowed,
};

const BEIJING_OFFSET_SECONDS: i32 = 8 * 60 * 60;

/// Active student course announcements.
pub const STUDENT_ACTIVE_PATH: &str = "/b/wlxt/kcgg/wlkc_ggb/student/pageListXsbyWgq";
/// Expired student course announcements.
pub const STUDENT_EXPIRED_PATH: &str = "/b/wlxt/kcgg/wlkc_ggb/student/pageListXsbyYgq";
/// Active teacher/TA course announcements.
pub const TEACHER_ACTIVE_PATH: &str = "/b/wlxt/kcgg/wlkc_ggb/teacher/pageListbyWgq";
/// Expired teacher/TA course announcements.
pub const TEACHER_EXPIRED_PATH: &str = "/b/wlxt/kcgg/wlkc_ggb/teacher/pageListbyYgq";

/// The two list responses that the public Learn client combines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnAnnouncementBucket {
    Active,
    Expired,
}

impl LearnAnnouncementBucket {
    fn path(self, role: CourseRole) -> &'static str {
        match (role, self) {
            (CourseRole::Student, Self::Active) => STUDENT_ACTIVE_PATH,
            (CourseRole::Student, Self::Expired) => STUDENT_EXPIRED_PATH,
            (CourseRole::Teacher, Self::Active) => TEACHER_ACTIVE_PATH,
            (CourseRole::Teacher, Self::Expired) => TEACHER_EXPIRED_PATH,
        }
    }

    fn is_expired(self) -> bool {
        matches!(self, Self::Expired)
    }
}

/// Configuration for one role-specific Learn announcement source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnAnnouncementConfig {
    pub role: CourseRole,
    pub csrf_parameter: String,
}

impl LearnAnnouncementConfig {
    pub fn new(role: CourseRole) -> Self {
        Self {
            role,
            csrf_parameter: "_csrf".to_owned(),
        }
    }

    pub fn with_csrf_parameter(
        mut self,
        parameter: impl Into<String>,
    ) -> Result<Self, LearnAnnouncementError> {
        let parameter = parameter.into();
        validate_parameter_name(&parameter)?;
        self.csrf_parameter = parameter;
        Ok(self)
    }
}

/// A request plan exposes the non-secret wire shape for fixture tests and
/// callers that need to inspect a request before execution.  The CSRF value
/// itself is deliberately not stored in the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnAnnouncementRequestPlan {
    pub method: LearnAnnouncementRequestMethod,
    pub endpoint: Url,
    pub path: String,
    pub bucket: LearnAnnouncementBucket,
    pub role: CourseRole,
    pub course_id: String,
    pub csrf_parameter: String,
    pub form: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnAnnouncementRequestMethod {
    Post,
}

/// A normalized course announcement.  `content` is decoded from the
/// service's base64 field when present; the source does not log it and its
/// Debug implementation reports only whether content was present.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct LearnAnnouncement {
    pub course_id: String,
    pub announcement_id: String,
    pub title: String,
    pub publisher: Option<String>,
    pub content: Option<String>,
    pub published_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub read: Option<bool>,
    pub important: Option<bool>,
    pub favorited: Option<bool>,
    pub comment: Option<String>,
    pub expired: bool,
}

impl fmt::Debug for LearnAnnouncement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnAnnouncement")
            .field("course_id", &self.course_id)
            .field("announcement_id", &self.announcement_id)
            .field("title", &self.title)
            .field("publisher", &self.publisher)
            .field("content_present", &self.content.is_some())
            .field("published_at", &self.published_at)
            .field("expires_at", &self.expires_at)
            .field("read", &self.read)
            .field("important", &self.important)
            .field("favorited", &self.favorited)
            .field("comment_present", &self.comment.is_some())
            .field("expired", &self.expired)
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum LearnAnnouncementError {
    #[error("learn announcement course id is empty")]
    EmptyCourseId,

    #[error("learn announcement course id contains a control character")]
    InvalidCourseId,

    #[error("learn announcement CSRF token is not configured")]
    MissingCsrf,

    #[error("learn announcement CSRF token belongs to another service")]
    ForeignCsrf,

    #[error("learn announcement CSRF parameter is invalid")]
    InvalidCsrfParameter,

    #[error("learn announcement route is invalid: {0}")]
    InvalidRoute(String),

    #[error("learn announcement role {configured:?} does not match Learn client role {learn:?}")]
    RoleMismatch {
        learn: CourseRole,
        configured: CourseRole,
    },

    #[error(transparent)]
    LearnClient(#[from] LearnClientError),

    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error("learn announcement session has expired")]
    SessionExpired,

    #[error("learn announcement request returned HTTP {status}")]
    HttpStatus { status: StatusCode },

    #[error("learn announcement lists contain conflicting records")]
    ConflictingRecords,

    #[error(transparent)]
    Parse(#[from] LearnAnnouncementParseError),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LearnAnnouncementParseError {
    #[error("announcement response JSON could not be decoded: {message}")]
    Decode { message: String },

    #[error("announcement response root must be an object")]
    InvalidRoot,

    #[error("announcement response result is not success: {message}")]
    BusinessFailure { message: String },

    #[error("announcement response object must contain aaData or resultsList")]
    MissingCollection,

    #[error("announcement response collection must be an array")]
    InvalidCollection,

    #[error("announcement record at index {index} must be an object")]
    InvalidRecord { index: usize },

    #[error("announcement record at index {index} is missing {field}")]
    MissingField { index: usize, field: &'static str },

    #[error("announcement record at index {index} has an invalid {field}")]
    InvalidField { index: usize, field: &'static str },

    #[error("announcement record at index {index} belongs to course {actual}, expected {expected}")]
    CourseMismatch {
        index: usize,
        expected: String,
        actual: String,
    },

    #[error("announcement record at index {index} has an invalid {field} timestamp")]
    InvalidTimestamp { index: usize, field: &'static str },
}

/// A Cookie-aware, read-only Learn announcement adapter.
pub struct LearnAnnouncementSource {
    learn: LearnClient,
    transport: CampusHttpTransport,
    config: LearnAnnouncementConfig,
    csrf: Option<BoundCsrfToken>,
}

impl Clone for LearnAnnouncementSource {
    fn clone(&self) -> Self {
        Self {
            learn: self.learn.clone(),
            transport: self.transport.clone(),
            config: self.config.clone(),
            csrf: self.csrf.clone(),
        }
    }
}

impl LearnAnnouncementSource {
    pub fn new(learn: LearnClient, transport: CampusHttpTransport) -> Self {
        let config = LearnAnnouncementConfig::new(learn.config().role());
        Self {
            learn,
            transport,
            config,
            csrf: None,
        }
    }

    pub fn with_config(
        learn: LearnClient,
        transport: CampusHttpTransport,
        config: LearnAnnouncementConfig,
    ) -> Result<Self, LearnAnnouncementError> {
        validate_parameter_name(&config.csrf_parameter)?;
        let learn_role = learn.config().role();
        if config.role != learn_role {
            return Err(LearnAnnouncementError::RoleMismatch {
                learn: learn_role,
                configured: config.role,
            });
        }
        Ok(Self {
            learn,
            transport,
            config,
            csrf: None,
        })
    }

    pub fn learn(&self) -> &LearnClient {
        &self.learn
    }

    pub fn config(&self) -> &LearnAnnouncementConfig {
        &self.config
    }

    pub fn with_csrf(&mut self, csrf: BoundCsrfToken) -> Result<(), LearnAnnouncementError> {
        if csrf.service() != ServiceId::Learn {
            return Err(LearnAnnouncementError::ForeignCsrf);
        }
        self.csrf = Some(csrf);
        Ok(())
    }

    pub fn request_plan(
        &self,
        course_id: &str,
        bucket: LearnAnnouncementBucket,
    ) -> Result<LearnAnnouncementRequestPlan, LearnAnnouncementError> {
        validate_course_id(course_id)?;
        let path = bucket.path(self.config.role);
        let endpoint = self
            .learn
            .resolve_endpoint(path)
            .map_err(|error| LearnAnnouncementError::InvalidRoute(error.to_string()))?;
        let ao_data = serde_json::to_string(&[serde_json::json!({
            "name": "wlkcid",
            "value": course_id,
        })])
        .expect("the announcement form contains only serializable literals");
        Ok(LearnAnnouncementRequestPlan {
            method: LearnAnnouncementRequestMethod::Post,
            endpoint,
            path: path.to_owned(),
            bucket,
            role: self.config.role,
            course_id: course_id.to_owned(),
            csrf_parameter: self.config.csrf_parameter.clone(),
            form: vec![("aoData".to_owned(), ao_data)],
        })
    }

    /// Reads active and expired announcements for one course. Both requests
    /// must return a verified Learn success envelope; a failure in either
    /// bucket fails the operation instead of returning partial or fabricated
    /// data.
    pub async fn list_course(
        &self,
        course_id: &str,
    ) -> Result<Vec<LearnAnnouncement>, LearnAnnouncementError> {
        validate_course_id(course_id)?;
        let mut announcements = self
            .fetch_bucket(course_id, LearnAnnouncementBucket::Active)
            .await?;
        announcements.extend(
            self.fetch_bucket(course_id, LearnAnnouncementBucket::Expired)
                .await?,
        );

        let mut seen = BTreeMap::new();
        let mut unique: Vec<LearnAnnouncement> = Vec::new();
        for item in announcements {
            if let Some(index) = seen.get(&item.announcement_id).copied() {
                if unique[index] != item {
                    return Err(LearnAnnouncementError::ConflictingRecords);
                }
            } else {
                seen.insert(item.announcement_id.clone(), unique.len());
                unique.push(item);
            }
        }
        let mut announcements = unique;
        announcements.sort_by(|left, right| {
            right
                .published_at
                .cmp(&left.published_at)
                .then_with(|| left.announcement_id.cmp(&right.announcement_id))
        });
        Ok(announcements)
    }

    async fn fetch_bucket(
        &self,
        course_id: &str,
        bucket: LearnAnnouncementBucket,
    ) -> Result<Vec<LearnAnnouncement>, LearnAnnouncementError> {
        let csrf = self
            .csrf
            .as_ref()
            .ok_or(LearnAnnouncementError::MissingCsrf)?;
        let plan = self.request_plan(course_id, bucket)?;
        let mut endpoint = plan.endpoint.clone();
        endpoint
            .query_pairs_mut()
            .append_pair(&plan.csrf_parameter, csrf.as_csrf_token().as_str());
        let form = AnnouncementPageListForm {
            ao_data: plan.form[0].1.clone(),
        };
        let request = self
            .transport
            .client()
            .post(endpoint)
            .header("Content-Type", LEARN_FORM_CONTENT_TYPE)
            .body(form.multipart_body())
            .build()
            .map_err(|error| LearnAnnouncementError::Transport(TransportError::Request(error)))?;
        let response =
            self.transport.execute(request).await.map_err(|error| {
                LearnAnnouncementError::Transport(TransportError::Request(error))
            })?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| LearnAnnouncementError::LearnClient(LearnClientError::UnexpectedOrigin))?;
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| LearnAnnouncementError::Transport(TransportError::Decode(error)))?;

        let mapped = self.learn.map_html_response_with_redirect(
            status,
            Some(&final_url),
            redirect_location.as_deref(),
            &body,
        )?;
        if matches!(
            mapped.classification,
            LearnPageClassification::LoginExpired(_)
        ) {
            return Err(LearnAnnouncementError::SessionExpired);
        }
        if endpoint_is_allowed(&self.learn.config().base_url, &final_url)
            && final_url.path() != plan.endpoint.path()
        {
            return Err(LearnAnnouncementError::InvalidRoute(
                "announcement response ended outside the requested endpoint".to_owned(),
            ));
        }
        if endpoint_is_allowed(&self.learn.config().base_url, &final_url)
            && !query_matches_exactly(
                &final_url,
                &[(plan.csrf_parameter.as_str(), csrf.as_csrf_token().as_str())],
            )
        {
            return Err(LearnAnnouncementError::InvalidRoute(
                "announcement response did not retain the bound CSRF query".to_owned(),
            ));
        }
        if status != StatusCode::OK {
            return Err(LearnAnnouncementError::HttpStatus { status });
        }

        parse_announcement_list(&body, course_id, bucket).map_err(|error| match error {
            // Some AJAX responses expose only `result: error` plus a login
            // message. Keep that state recoverable even if the page
            // classifier did not see the marker (for example, a deployment
            // used a non-default message field).
            LearnAnnouncementParseError::BusinessFailure { ref message }
                if is_login_failure_message(message) =>
            {
                LearnAnnouncementError::SessionExpired
            }
            error => LearnAnnouncementError::Parse(error),
        })
    }
}

// Keep the field value in the multipart part itself. A plain token boundary
// is accepted by the deployed multipart parser and makes the wire contract
// unambiguous for request inspection and retries.
const LEARN_FORM_BOUNDARY: &str = "----THYouLearnFormBoundary7ma4YWxkTrZu0gW";
const LEARN_FORM_CONTENT_TYPE: &str =
    "multipart/form-data; boundary=----THYouLearnFormBoundary7ma4YWxkTrZu0gW";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnnouncementPageListForm {
    ao_data: String,
}

impl AnnouncementPageListForm {
    fn multipart_body(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(self.ao_data.len() + 160);
        body.extend_from_slice(b"--");
        body.extend_from_slice(LEARN_FORM_BOUNDARY.as_bytes());
        body.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"aoData\"\r\n\r\n");
        body.extend_from_slice(self.ao_data.as_bytes());
        body.extend_from_slice(b"\r\n--");
        body.extend_from_slice(LEARN_FORM_BOUNDARY.as_bytes());
        body.extend_from_slice(b"--\r\n");
        body
    }
}

/// Parses one redacted Learn announcement-list response. A successful empty
/// list is valid only when the server explicitly returns the expected wrapper
/// and an explicit `aaData` array.
pub fn parse_announcement_list(
    body: &str,
    course_id: &str,
    bucket: LearnAnnouncementBucket,
) -> Result<Vec<LearnAnnouncement>, LearnAnnouncementParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        validate_course_id(course_id).map_err(|error| match error {
            LearnAnnouncementError::EmptyCourseId | LearnAnnouncementError::InvalidCourseId => {
                LearnAnnouncementParseError::InvalidField {
                    index: 0,
                    field: "course id",
                }
            }
            _ => unreachable!("course-id validation has no other error variants"),
        })?;
        let value: Value =
            serde_json::from_str(body).map_err(|error| LearnAnnouncementParseError::Decode {
                message: error.to_string(),
            })?;
        let root = value
            .as_object()
            .ok_or(LearnAnnouncementParseError::InvalidRoot)?;
        if !root
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "success")
        {
            return Err(LearnAnnouncementParseError::BusinessFailure {
                message: scalar_text(root.get("msg").or_else(|| root.get("message")))
                    .unwrap_or_else(|| "unknown learning response".to_owned()),
            });
        }

        let payload = root
            .get("object")
            .and_then(Value::as_object)
            .ok_or(LearnAnnouncementParseError::MissingCollection)?;
        // The student and teacher deployments have both used these two names for
        // the same DataTables collection. `aaData: null` has the same meaning as
        // an absent aaData in the public client, so only then do we fall back to
        // resultsList. A non-null wrong type is a protocol error, never `[]`.
        let rows = match payload.get("aaData") {
            Some(Value::Array(rows)) => rows,
            Some(Value::Null) | None => match payload.get("resultsList") {
                Some(Value::Array(rows)) => rows,
                Some(Value::Null) | None => {
                    return Err(LearnAnnouncementParseError::MissingCollection);
                }
                Some(_) => return Err(LearnAnnouncementParseError::InvalidCollection),
            },
            Some(_) => return Err(LearnAnnouncementParseError::InvalidCollection),
        };

        rows.iter()
            .enumerate()
            .map(|(index, value)| parse_announcement_record(value, index, course_id, bucket))
            .collect()
    })
}

fn parse_announcement_record(
    value: &Value,
    index: usize,
    course_id: &str,
    bucket: LearnAnnouncementBucket,
) -> Result<LearnAnnouncement, LearnAnnouncementParseError> {
    let object = value
        .as_object()
        .ok_or(LearnAnnouncementParseError::InvalidRecord { index })?;
    if let Some(actual_course_id) = response_course_id(object, index)?
        && actual_course_id != course_id
    {
        return Err(LearnAnnouncementParseError::CourseMismatch {
            index,
            expected: course_id.to_owned(),
            actual: actual_course_id,
        });
    }
    // These names come from the current Learn notification list response.
    // Generic `id`/`title` fields are deliberately excluded: accepting them
    // would turn an unrelated JSON object into an announcement.
    let announcement_id = required_scalar(object, &["ggid"], index, "announcement id")?;
    let title = required_text_scalar(object, &["bt"], index, "title")?;
    let published_raw =
        required_text_scalar(object, &["fbsj", "fbsjStr"], index, "published time")?;
    let published_at = parse_learning_datetime(&published_raw).ok_or(
        LearnAnnouncementParseError::InvalidTimestamp {
            index,
            field: "published time",
        },
    )?;
    let expires_at = parse_optional_timestamp(object, &["jzsj"], index, "expiration time")?;
    let content = parse_optional_content(object, index)?;
    let read = parse_optional_yes_no_flag(object, "sfyd", index, "read")?;
    let important = parse_optional_numeric_flag(object, "sfqd", index, "important")?;
    let favorited = parse_optional_yes_no_flag(object, "sfsc", index, "favorited")?;

    Ok(LearnAnnouncement {
        course_id: course_id.to_owned(),
        announcement_id,
        title,
        publisher: first_scalar(object, &["fbrxm"]),
        content,
        published_at,
        expires_at,
        read,
        important,
        favorited,
        comment: first_text_scalar(object, &["bznr"]),
        expired: bucket.is_expired(),
    })
}

fn response_course_id(
    object: &Map<String, Value>,
    index: usize,
) -> Result<Option<String>, LearnAnnouncementParseError> {
    let Some(value) = object
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case("wlkcid").then_some(value))
    else {
        // The list request already carries the course binding. The current
        // public client does not require this field to be echoed by every row.
        return Ok(None);
    };
    scalar_text(Some(value))
        .filter(|value| !value.trim().is_empty())
        .map(Some)
        .ok_or(LearnAnnouncementParseError::InvalidField {
            index,
            field: "course id",
        })
}

fn required_scalar(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &'static str,
) -> Result<String, LearnAnnouncementParseError> {
    let Some(value) = aliases.iter().find_map(|alias| {
        object
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(alias).then_some(value))
    }) else {
        return Err(LearnAnnouncementParseError::MissingField { index, field });
    };
    scalar_text(Some(value))
        .filter(|value| !value.trim().is_empty())
        .ok_or(LearnAnnouncementParseError::InvalidField { index, field })
}

fn first_scalar(object: &Map<String, Value>, aliases: &[&str]) -> Option<String> {
    aliases.iter().find_map(|alias| {
        object.iter().find_map(|(key, value)| {
            if !key.eq_ignore_ascii_case(alias) {
                return None;
            }
            scalar_text(Some(value)).filter(|value| !value.trim().is_empty())
        })
    })
}

fn scalar_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn required_text_scalar(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &'static str,
) -> Result<String, LearnAnnouncementParseError> {
    first_text_scalar(object, aliases)
        .ok_or(LearnAnnouncementParseError::MissingField { index, field })
}

fn first_text_scalar(object: &Map<String, Value>, aliases: &[&str]) -> Option<String> {
    aliases.iter().find_map(|alias| {
        object.iter().find_map(|(key, value)| {
            if !key.eq_ignore_ascii_case(alias) {
                return None;
            }
            match value {
                Value::String(value) if !value.trim().is_empty() => {
                    Some(crate::learn_client::decode_learn_html(value.trim()))
                }
                _ => None,
            }
        })
    })
}

fn parse_optional_timestamp(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>, LearnAnnouncementParseError> {
    let Some(value) = aliases.iter().find_map(|alias| {
        object
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(alias).then_some(value))
    }) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = scalar_text(Some(value)).filter(|value| !value.is_empty()) else {
        return Err(LearnAnnouncementParseError::InvalidTimestamp { index, field });
    };
    parse_learning_datetime(&value)
        .map(Some)
        .ok_or(LearnAnnouncementParseError::InvalidTimestamp { index, field })
}

fn parse_optional_content(
    object: &Map<String, Value>,
    index: usize,
) -> Result<Option<String>, LearnAnnouncementParseError> {
    let Some(value) = object
        .iter()
        .find_map(|(key, value)| key.eq_ignore_ascii_case("ggnr").then_some(value))
    else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_str() else {
        return Err(LearnAnnouncementParseError::InvalidField {
            index,
            field: "content",
        });
    };
    let decoded = BASE64
        .decode(value)
        .map_err(|_| LearnAnnouncementParseError::InvalidField {
            index,
            field: "content",
        })?;
    String::from_utf8(decoded)
        .map(|value| Some(crate::learn_client::decode_learn_html(&value)))
        .map_err(|_| LearnAnnouncementParseError::InvalidField {
            index,
            field: "content",
        })
}

fn parse_optional_yes_no_flag(
    object: &Map<String, Value>,
    field_name: &str,
    index: usize,
    field: &'static str,
) -> Result<Option<bool>, LearnAnnouncementParseError> {
    let Some(value) = object.get(field_name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let parsed = match value {
        Value::String(value) => match value.trim() {
            "是" => Some(true),
            "否" => Some(false),
            _ => None,
        },
        _ => None,
    };
    parsed
        .ok_or(LearnAnnouncementParseError::InvalidField { index, field })
        .map(Some)
}

fn parse_optional_numeric_flag(
    object: &Map<String, Value>,
    field_name: &str,
    index: usize,
    field: &'static str,
) -> Result<Option<bool>, LearnAnnouncementParseError> {
    let Some(value) = object.get(field_name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let parsed = match value {
        Value::Number(value) => match value.as_i64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        Value::String(value) => match value.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        },
        _ => None,
    };
    parsed
        .ok_or(LearnAnnouncementParseError::InvalidField { index, field })
        .map(Some)
}

fn parse_learning_datetime(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Some(value.with_timezone(&Utc));
    }
    let local_offset = FixedOffset::east_opt(BEIJING_OFFSET_SECONDS)?;
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S%.f",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
    ] {
        if let Ok(value) = NaiveDateTime::parse_from_str(value, format) {
            return local_offset
                .from_local_datetime(&value)
                .single()
                .map(|value| value.with_timezone(&Utc));
        }
    }
    if let Ok(value) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return value
            .and_hms_opt(0, 0, 0)
            .and_then(|value| local_offset.from_local_datetime(&value).single())
            .map(|value| value.with_timezone(&Utc));
    }
    None
}

fn validate_course_id(course_id: &str) -> Result<(), LearnAnnouncementError> {
    if course_id.trim().is_empty() {
        return Err(LearnAnnouncementError::EmptyCourseId);
    }
    if course_id.chars().any(char::is_control) {
        return Err(LearnAnnouncementError::InvalidCourseId);
    }
    Ok(())
}

fn validate_parameter_name(parameter: &str) -> Result<(), LearnAnnouncementError> {
    if parameter.trim().is_empty()
        || parameter
            .chars()
            .any(|character| character.is_control() || matches!(character, '&' | '='))
    {
        return Err(LearnAnnouncementError::InvalidCsrfParameter);
    }
    Ok(())
}

fn query_matches_exactly(url: &Url, expected: &[(&str, &str)]) -> bool {
    let actual = url.query_pairs().collect::<Vec<_>>();
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|((name, value), (expected_name, expected_value))| {
                name == *expected_name && value == *expected_value
            })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use reqwest::Url;

    use super::*;
    use crate::protocol::CsrfToken;
    use crate::session::SessionRegistry;

    const ANNOUNCEMENT_FIXTURE: &str = r#"
        {
          "result": "success",
          "object": {
            "aaData": [
              {
                "ggid": "announcement-1",
                "wlkcid": "7001",
                "bt": "课程通知",
                "ggnr": "QW5ub3VuY2VtZW50IGNvbnRlbnQ=",
                "fbrxm": "教师甲",
                "sfyd": "否",
                "sfqd": 1,
                "fbsj": "2026-09-10 08:00:00",
                "jzsj": null,
                "sfsc": "否",
                "bznr": null
              }
            ]
          }
        }
    "#;

    fn client(base: &str, role: CourseRole) -> LearnClient {
        LearnClient::new(
            crate::learn_client::LearnClientConfig::new(base, role).expect("fixture base URL"),
        )
    }

    #[test]
    fn public_routes_and_form_shape_are_role_specific() {
        let student = LearnAnnouncementSource::new(
            client("https://learn.example.test/", CourseRole::Student),
            CampusHttpTransport::new("THYou/test").expect("transport"),
        );
        let plan = student
            .request_plan("7001", LearnAnnouncementBucket::Active)
            .expect("student plan");
        assert_eq!(plan.method, LearnAnnouncementRequestMethod::Post);
        assert_eq!(plan.path, STUDENT_ACTIVE_PATH);
        assert_eq!(plan.form[0].0, "aoData");
        assert_eq!(plan.form[0].1, r#"[{"name":"wlkcid","value":"7001"}]"#);

        let teacher = LearnAnnouncementSource::new(
            client("https://learn.example.test/", CourseRole::Teacher),
            CampusHttpTransport::new("THYou/test").expect("transport"),
        );
        assert_eq!(
            teacher
                .request_plan("7001", LearnAnnouncementBucket::Expired)
                .expect("teacher plan")
                .path,
            TEACHER_EXPIRED_PATH
        );
    }

    #[test]
    fn parses_redacted_success_and_verified_empty_shapes() {
        let records = parse_announcement_list(
            ANNOUNCEMENT_FIXTURE,
            "7001",
            LearnAnnouncementBucket::Active,
        )
        .expect("announcement fixture parses");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].course_id, "7001");
        assert_eq!(records[0].announcement_id, "announcement-1");
        assert_eq!(records[0].title, "课程通知");
        assert_eq!(records[0].content.as_deref(), Some("Announcement content"));
        assert_eq!(records[0].important, Some(true));
        assert_eq!(records[0].read, Some(false));
        assert!(!records[0].expired);

        let empty = parse_announcement_list(
            r#"{"result":"success","object":{"aaData":[]}}"#,
            "7001",
            LearnAnnouncementBucket::Expired,
        )
        .expect("explicit empty announcement list parses");
        assert!(empty.is_empty());
        assert!(
            parse_announcement_list(
                r#"{"result":"success","object":{}}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_business_failure_malformed_rows_and_invalid_content() {
        assert!(matches!(
            parse_announcement_list(
                r#"{"result":"error","msg":"session expired"}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            ),
            Err(LearnAnnouncementParseError::BusinessFailure { .. })
        ));
        assert!(matches!(
            parse_announcement_list(
                r#"{"result":"success","object":{"aaData":[{"ggid":"1","wlkcid":"7001","bt":"公告"}]}}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            ),
            Err(LearnAnnouncementParseError::MissingField {
                field: "published time",
                ..
            })
        ));
        assert!(matches!(
            parse_announcement_list(
                r#"{"result":"success","object":{"aaData":[{"ggid":"1","wlkcid":"7001","bt":"公告","fbsj":"2026-09-10","ggnr":"%%%"}]}}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            ),
            Err(LearnAnnouncementParseError::InvalidField {
                field: "content",
                ..
            })
        ));
        assert!(matches!(
            parse_announcement_list(
                r#"{"result":"success","object":{"aaData":[{"ggid":"1","wlkcid":"7002","bt":"公告","fbsj":"2026-09-10"}]}}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            ),
            Err(LearnAnnouncementParseError::CourseMismatch {
                expected,
                actual,
                ..
            }) if expected == "7001" && actual == "7002"
        ));
        assert!(matches!(
            parse_announcement_list(
                r#"{"result":"success","object":{"aaData":[{"ggid":"1","wlkcid":{},"bt":"公告","fbsj":"2026-09-10"}]}}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            ),
            Err(LearnAnnouncementParseError::InvalidField {
                field: "course id",
                ..
            })
        ));
        assert!(matches!(
            parse_announcement_list(
                r#"{"result":"success","object":{"aaData":[{"id":"1","wlkcid":"7001","title":"未知字段","fbsj":"2026-09-10"}]}}"#,
                "7001",
                LearnAnnouncementBucket::Active,
            ),
            Err(LearnAnnouncementParseError::MissingField {
                field: "announcement id",
                ..
            })
        ));
        let decoded = parse_announcement_list(
            r#"{"result":"success","object":{"aaData":[{"ggid":"1","wlkcid":"7001","bt":"课程 &amp; 通知","fbsj":"2026-09-10","ggnr":"SGVsbG8gJmx0O2NhbXB1cyZndDs="}]}}"#,
            "7001",
            LearnAnnouncementBucket::Active,
        )
        .expect("announcement HTML entities decode");
        assert_eq!(decoded[0].title, "课程 & 通知");
        assert_eq!(decoded[0].content.as_deref(), Some("Hello <campus>"));
    }

    #[tokio::test]
    async fn executes_both_buckets_with_cookie_csrf_and_form_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for bucket in ["Wgq", "Ygq"] {
                let (mut stream, _) = listener.accept().expect("request");
                let request = read_http_request(&mut stream);
                assert!(request.starts_with("POST /b/wlxt/kcgg/wlkc_ggb/student/pageListXsby"));
                assert!(request.contains("_csrf=fixture-csrf"));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("content-type: multipart/form-data; boundary=")
                );
                assert!(request.contains("name=\"aoData\""));
                assert!(request.contains(r#"[{"name":"wlkcid","value":"7001"}]"#));
                assert!(request.contains("learn-session=present"));
                assert!(request.contains(bucket));
                let body = if bucket == "Wgq" {
                    ANNOUNCEMENT_FIXTURE
                } else {
                    r#"{"result":"success","object":{"aaData":[]}}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
        });

        let base = format!("http://{address}/");
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let base_url = Url::parse(&base).expect("base URL");
        transport
            .cookie_jar()
            .add_cookie_str("learn-session=present; Path=/", &base_url);
        let mut source =
            LearnAnnouncementSource::new(client(&base, CourseRole::Student), transport);
        let registry = SessionRegistry::new();
        source
            .with_csrf(registry.bind_csrf(
                ServiceId::Learn,
                CsrfToken::new("fixture-csrf").expect("csrf"),
            ))
            .expect("csrf binding");

        let records = source.list_course("7001").await.expect("live fixture");
        assert_eq!(records.len(), 1);
        assert!(!records[0].expired);
        server.join().expect("server");
    }

    #[tokio::test]
    async fn treats_an_http_200_login_page_as_expired_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let _ = read_http_request(&mut stream);
            let body = r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("response");
        });
        let base = format!("http://{address}/");
        let mut source = LearnAnnouncementSource::new(
            client(&base, CourseRole::Student),
            CampusHttpTransport::new("THYou/test").expect("transport"),
        );
        let registry = SessionRegistry::new();
        source
            .with_csrf(registry.bind_csrf(
                ServiceId::Learn,
                CsrfToken::new("fixture-csrf").expect("csrf"),
            ))
            .expect("csrf binding");
        let error = source
            .list_course("7001")
            .await
            .expect_err("login page is not an announcement response");
        assert!(matches!(error, LearnAnnouncementError::SessionExpired));
        server.join().expect("server");
    }

    #[test]
    fn refuses_foreign_or_missing_csrf() {
        let mut source = LearnAnnouncementSource::new(
            client("https://learn.example.test/", CourseRole::Student),
            CampusHttpTransport::new("THYou/test").expect("transport"),
        );
        let registry = SessionRegistry::new();
        let foreign =
            registry.bind_csrf(ServiceId::Info, CsrfToken::new("info-csrf").expect("csrf"));
        assert!(matches!(
            source.with_csrf(foreign),
            Err(LearnAnnouncementError::ForeignCsrf)
        ));
        assert!(matches!(
            futures_missing_csrf(&source),
            Err(LearnAnnouncementError::MissingCsrf)
        ));
    }

    #[test]
    fn refuses_an_announcement_role_that_does_not_match_the_learn_client() {
        let result = LearnAnnouncementSource::with_config(
            client("https://learn.example.test/", CourseRole::Student),
            CampusHttpTransport::new("THYou/test").expect("transport"),
            LearnAnnouncementConfig::new(CourseRole::Teacher),
        );
        assert!(matches!(
            result,
            Err(LearnAnnouncementError::RoleMismatch {
                learn: CourseRole::Student,
                configured: CourseRole::Teacher,
            })
        ));
    }

    fn futures_missing_csrf(
        source: &LearnAnnouncementSource,
    ) -> Result<Vec<LearnAnnouncement>, LearnAnnouncementError> {
        // Keep this synchronous helper limited to the request precondition;
        // no request is sent and no credential is needed.
        source
            .csrf
            .as_ref()
            .ok_or(LearnAnnouncementError::MissingCsrf)
            .map(|_| Vec::new())
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("request timeout");
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut chunk).expect("request read");
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if expected_len.is_none() {
                if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    expected_len = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    });
                }
            }
            if let Some(expected_len) = expected_len {
                let header_end = bytes
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .expect("headers");
                if bytes.len() >= header_end + 4 + expected_len {
                    break;
                }
            } else if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[allow(dead_code)]
    fn _fixture_content_is_stable() {
        assert_eq!(
            BASE64.decode("QW5ub3VuY2VtZW50IGNvbnRlbnQ=").unwrap(),
            b"Announcement content"
        );
    }
}
