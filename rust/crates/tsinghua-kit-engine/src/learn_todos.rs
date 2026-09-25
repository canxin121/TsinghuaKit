//! Student assignment data from Web Learning, mapped to the campus todo model.
//!
//! The learning site exposes assignment lists per course.  This adapter keeps
//! those request details in Rust: Flutter receives only the resulting todo
//! records and never sees course IDs, CSRF values, HTML, or endpoint paths.
//!
//! The routes and field names in this module were cross-checked against the
//! current MIT-licensed `thu-learn-lib` implementation and an independent
//! LearnX integration.  The request and response code below is an independent
//! Rust implementation; no source or assets are copied from those projects.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use reqwest::{StatusCode, Url, header::LOCATION};
use serde_json::{Map, Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    campus_live::{
        CampusTodoFailureKind, CampusTodoProviderFailure, CampusTodoReadReport, CampusTodoSource,
    },
    domain::{NewTodo, TodoFilter, TodoItem, TodoPriority, TodoStatus},
    error::ServiceError,
    learn::{LearnError, validate_segment, validate_semester_id},
    learn_client::{
        LearnClient, LearnClientError, LearnCourseRecord, LearnPageClassification,
        is_login_failure_message,
    },
    protocol::{CourseRole, ServiceId},
    session::BoundCsrfToken,
    transport::TransportError,
    webvpn_url::endpoint_is_allowed,
};

const HOMEWORK_LIST_NEW: &str = "/b/wlxt/kczy/zy/student/zyListWj";
const HOMEWORK_LIST_SUBMITTED: &str = "/b/wlxt/kczy/zy/student/zyListYjwg";
const HOMEWORK_LIST_GRADED: &str = "/b/wlxt/kczy/zy/student/zyListYpg";
const BEIJING_OFFSET_SECONDS: i32 = 8 * 60 * 60;

/// The three student assignment lists observed in the current learning site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeworkBucket {
    Pending,
    Submitted,
    Graded,
}

impl HomeworkBucket {
    const fn path(self) -> &'static str {
        match self {
            Self::Pending => HOMEWORK_LIST_NEW,
            Self::Submitted => HOMEWORK_LIST_SUBMITTED,
            Self::Graded => HOMEWORK_LIST_GRADED,
        }
    }

    const fn status(self) -> TodoStatus {
        match self {
            Self::Pending => TodoStatus::Pending,
            Self::Submitted => TodoStatus::InProgress,
            Self::Graded => TodoStatus::Completed,
        }
    }
}

/// Configuration for one learning assignment source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnTodoConfig {
    pub semester: String,
    pub language: String,
    pub course_index: Option<String>,
}

impl LearnTodoConfig {
    pub fn new(semester: impl Into<String>) -> Self {
        Self {
            semester: semester.into(),
            language: "zh".to_owned(),
            course_index: None,
        }
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn with_course_index(mut self, course_index: impl Into<String>) -> Self {
        self.course_index = Some(course_index.into());
        self
    }

    fn validate(&self, role: CourseRole) -> Result<(), ServiceError> {
        if validate_semester_id(&self.semester).is_err()
            || validate_segment(&self.language, LearnError::InvalidPathSegment).is_err()
            || self.course_index.as_deref().is_some_and(|value| {
                validate_segment(value, LearnError::InvalidPathSegment).is_err()
            })
        {
            return Err(adapter_error(
                "learning todo semester or language is invalid",
            ));
        }
        if matches!(role, CourseRole::Teacher)
            && self
                .course_index
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(adapter_error(
                "teacher learning todo source requires a course index",
            ));
        }
        Ok(())
    }
}

/// An assignment record after the unstable learning JSON has been reduced to
/// fields needed by the domain model.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnHomeworkRecord {
    pub student_id: Option<String>,
    pub base_id: Option<String>,
    pub title: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub late_due_at: Option<DateTime<Utc>>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub graded_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub bucket: HomeworkBucket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LearnTodoProviderError {
    kind: CampusTodoFailureKind,
}

impl LearnTodoProviderError {
    const fn new(kind: CampusTodoFailureKind) -> Self {
        Self { kind }
    }
}

impl std::fmt::Display for LearnTodoProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "learning todo provider failed: {}", self.kind)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LearnTodoParseError {
    #[error("assignment response JSON could not be decoded: {message}")]
    Decode { message: String },

    #[error("assignment response root must be an object")]
    InvalidRoot,

    #[error("assignment response result is not success: {message}")]
    ResultFailure { message: String },

    #[error("assignment response object must contain an array")]
    InvalidCollection,

    #[error("assignment record at index {index} must be an object")]
    InvalidRecord { index: usize },

    #[error("assignment record at index {index} has an invalid {field}")]
    InvalidField { index: usize, field: &'static str },

    #[error("assignment record at index {index} is missing {field}")]
    MissingField { index: usize, field: &'static str },

    #[error("assignment record at index {index} has an invalid {field}: {value:?}")]
    InvalidDate {
        index: usize,
        field: String,
        value: String,
    },

    #[error("assignment record at index {index} belongs to course {actual}, expected {expected}")]
    CourseMismatch {
        index: usize,
        expected: String,
        actual: String,
    },
}

// Real-account diagnostics must not contain an assignment, course, date, or
// response value. The fixed codes only identify which parser contract failed.
fn safe_homework_parse_reason(error: &LearnTodoParseError) -> &'static str {
    match error {
        LearnTodoParseError::Decode { .. } => "invalid_json",
        LearnTodoParseError::InvalidRoot => "invalid_root",
        LearnTodoParseError::ResultFailure { .. } => "result_failure",
        LearnTodoParseError::InvalidCollection => "invalid_collection",
        LearnTodoParseError::InvalidRecord { .. } => "invalid_record",
        LearnTodoParseError::InvalidField { .. } => "invalid_field",
        LearnTodoParseError::MissingField { .. } => "missing_field",
        LearnTodoParseError::InvalidDate { .. } => "invalid_date",
        LearnTodoParseError::CourseMismatch { .. } => "course_mismatch",
    }
}

fn safe_homework_parse_field(error: &LearnTodoParseError) -> &'static str {
    let field: &str = match error {
        LearnTodoParseError::InvalidField { field, .. }
        | LearnTodoParseError::MissingField { field, .. } => field,
        LearnTodoParseError::InvalidDate { field, .. } => field,
        _ => return "none",
    };
    match field {
        "course id" => "course_id",
        "student assignment id" => "student_id",
        "assignment id" => "base_id",
        "title" => "title",
        "deadline" => "deadline",
        "late deadline" => "late_deadline",
        "submission time" => "submission_time",
        "grade time" => "grade_time",
        "creation time" => "creation_time",
        _ => "other",
    }
}

fn safe_homework_body_shape(body: &str) -> &'static str {
    match body.trim_start().chars().next() {
        Some('{') => "json_object",
        Some('[') => "json_array",
        Some('<') => "html",
        Some(_) => "other",
        None => "empty",
    }
}

fn safe_homework_date_shape(error: &LearnTodoParseError) -> (&'static str, &'static str) {
    let LearnTodoParseError::InvalidDate { value, .. } = error else {
        return ("none", "none");
    };
    let value = value.trim();
    let width = match value.len() {
        10 => "w10",
        13 => "w13",
        16 => "w16",
        19 => "w19",
        21 => "w21",
        23 => "w23",
        24 => "w24",
        25 => "w25",
        29 => "w29",
        _ => "other",
    };
    let shape = if value.is_empty() {
        "empty"
    } else if value.starts_with("/Date(") {
        "dotnet"
    } else if value.bytes().all(|byte| byte.is_ascii_digit()) {
        "digits"
    } else if !value.is_ascii() {
        "unicode"
    } else if matches!(value.as_bytes().get(10), Some(b'T' | b't')) {
        if value.ends_with('Z') || value.ends_with('z') || value.contains('+') {
            "iso_t_zone"
        } else {
            "iso_t_local"
        }
    } else if value.contains('/') {
        "slash"
    } else if value.contains('-') && value.contains(' ') {
        if value.bytes().any(|byte| byte.is_ascii_alphabetic()) {
            "dash_space_alpha"
        } else if value.contains('+') {
            "dash_space_zone"
        } else {
            "dash_space"
        }
    } else if value.contains('-') {
        "dash_date"
    } else if value.contains('.') {
        "dot"
    } else {
        "other"
    };
    (shape, width)
}

/// Parses one student assignment-list response.
///
/// The learning service wraps rows as `{ result: "success", object: { aaData:
/// [...] } }`. The assignment endpoint is accepted only with this observed
/// `object.aaData` collection; a different wrapper is a protocol mismatch.
pub fn parse_homework_list(
    body: &str,
    bucket: HomeworkBucket,
) -> Result<Vec<LearnHomeworkRecord>, LearnTodoParseError> {
    parse_homework_list_for_course(body, bucket, None)
}

fn parse_homework_list_for_course(
    body: &str,
    bucket: HomeworkBucket,
    course_id: Option<&str>,
) -> Result<Vec<LearnHomeworkRecord>, LearnTodoParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let value: Value =
            serde_json::from_str(body).map_err(|error| LearnTodoParseError::Decode {
                message: error.to_string(),
            })?;
        let object = value.as_object().ok_or(LearnTodoParseError::InvalidRoot)?;

        if !result_is_success(object.get("result")) {
            return Err(LearnTodoParseError::ResultFailure {
                message: scalar_text(object.get("msg").or_else(|| object.get("message")))
                    .unwrap_or_else(|| "unknown learning response".to_owned()),
            });
        }

        let rows = match object.get("object") {
            None | Some(Value::Null) => return Err(LearnTodoParseError::InvalidCollection),
            Some(Value::Object(payload)) => payload.get("aaData"),
            Some(_) => None,
        };
        let Some(Value::Array(rows)) = rows else {
            return Err(LearnTodoParseError::InvalidCollection);
        };

        rows.iter()
            .enumerate()
            .map(|(index, value)| parse_homework_record(value, index, bucket, course_id))
            .collect()
    })
}

fn parse_homework_record(
    value: &Value,
    index: usize,
    bucket: HomeworkBucket,
    expected_course_id: Option<&str>,
) -> Result<LearnHomeworkRecord, LearnTodoParseError> {
    let object = value
        .as_object()
        .ok_or(LearnTodoParseError::InvalidRecord { index })?;

    // The current endpoint is already scoped by the requested course and the
    // public client does not require every row to echo `wlkcid`. When the
    // server does echo it, accept only the real field and verify its binding;
    // generic `id`/`courseId` aliases cannot prove course membership.
    if let Some(remote_course_id) = optional_scalar(object, &["wlkcid"], index, "course id")?
        && let Some(expected_course_id) = expected_course_id
        && remote_course_id != expected_course_id
    {
        return Err(LearnTodoParseError::CourseMismatch {
            index,
            expected: expected_course_id.to_owned(),
            actual: remote_course_id,
        });
    }

    // Parse every explicitly supplied temporal field before reducing the row
    // to a todo.  A malformed remote date is a protocol error even when the
    // same row is missing another required field; otherwise a broken row can
    // be discarded or appear as an un-dated pending item.
    let due_at = parse_optional_datetime(object, &["jzsj"], index, "deadline")?;
    let late_due_at = parse_optional_datetime(object, &["bjjzsj"], index, "late deadline")?;
    let submitted_at = parse_optional_datetime(object, &["scsj"], index, "submission time")?;
    let graded_at = parse_optional_datetime(object, &["pysj"], index, "grade time")?;
    let created_at = parse_optional_datetime(object, &["fbsj"], index, "creation time")?;

    // These are the names emitted by the current student endpoints. The
    // student assignment ID, title, and deadline are needed to produce a todo.
    // `zyid` is retained when the list row includes it, but the runtime's
    // minimal list response can omit that detail identifier; a missing `zyid`
    // therefore remains `None` rather than being replaced with a generic `id`.
    let student_id = required_scalar(object, &["xszyid"], index, "student assignment id")?;
    let base_id = optional_scalar(object, &["zyid"], index, "assignment id")?;
    let title = required_text_scalar(object, &["bt"], index, "title")?;
    if due_at.is_none() {
        return Err(LearnTodoParseError::MissingField {
            index,
            field: "deadline",
        });
    }

    Ok(LearnHomeworkRecord {
        student_id: Some(student_id),
        base_id,
        title: Some(title),
        due_at,
        late_due_at,
        submitted_at,
        graded_at,
        created_at,
        bucket,
    })
}

/// A Rust-owned read-only assignment source which implements the campus todo
/// boundary.  The write methods deliberately return an adapter error until a
/// separate assignment submission profile is verified.
#[derive(Clone)]
pub struct LearnTodoSource {
    learn: LearnClient,
    transport: crate::transport::CampusHttpTransport,
    config: LearnTodoConfig,
    csrf: Option<BoundCsrfToken>,
    csrf_parameter: Option<String>,
    // Validation may reuse the course response already proved by this same
    // runtime/session/semester. This data never enters a diagnostic ledger.
    proven_courses: Option<Vec<LearnCourseRecord>>,
}

impl LearnTodoSource {
    pub fn new(
        learn: LearnClient,
        transport: crate::transport::CampusHttpTransport,
        config: LearnTodoConfig,
    ) -> Result<Self, ServiceError> {
        config.validate(learn.config().profile.role)?;
        Ok(Self {
            learn,
            transport,
            config,
            csrf: None,
            csrf_parameter: None,
            proven_courses: None,
        })
    }

    pub fn with_csrf(
        &mut self,
        csrf: BoundCsrfToken,
        parameter_name: impl Into<String>,
    ) -> Result<(), ServiceError> {
        if csrf.service() != ServiceId::Learn {
            return Err(adapter_error(
                "learning todo CSRF token belongs to another service",
            ));
        }
        let parameter_name = parameter_name.into();
        if parameter_name.trim().is_empty()
            || parameter_name
                .chars()
                .any(|character| character.is_control() || matches!(character, '&' | '='))
        {
            return Err(adapter_error(
                "learning todo CSRF query parameter is invalid",
            ));
        }
        self.csrf = Some(csrf);
        self.csrf_parameter = Some(parameter_name);
        Ok(())
    }

    pub fn learn(&self) -> &LearnClient {
        &self.learn
    }

    pub(crate) fn with_proven_courses(mut self, courses: Vec<LearnCourseRecord>) -> Self {
        self.proven_courses = Some(courses);
        self
    }

    pub fn config(&self) -> &LearnTodoConfig {
        &self.config
    }

    /// Reads the three real student assignment lists for one course.  This is
    /// the course-scoped operation used by a dedicated homework screen; the
    /// todo adapter below uses the same request path while aggregating every
    /// enrolled course.  Each bucket must return a verified success envelope,
    /// so a missing bucket is an error rather than an empty-success fallback.
    pub async fn list_course_homework(
        &self,
        course_id: &str,
    ) -> Result<Vec<LearnHomeworkRecord>, ServiceError> {
        validate_homework_course_id(course_id)?;
        if !matches!(self.learn.config().profile.role, CourseRole::Student) {
            return Err(adapter_error(
                "teacher learning assignment source is not configured",
            ));
        }

        let mut records = Vec::new();
        for bucket in [
            HomeworkBucket::Pending,
            HomeworkBucket::Submitted,
            HomeworkBucket::Graded,
        ] {
            records.extend(
                self.fetch_homework_page(course_id, bucket)
                    .await
                    .map_err(provider_error_to_service)?,
            );
        }
        let (records, conflicts) = consistent_homework_records(records);
        if conflicts != 0 {
            return Err(adapter_error(
                "learning assignment lists contain conflicting records",
            ));
        }
        Ok(records)
    }

    /// A course workspace needs the exact failure category, while the older
    /// domain-facing method above keeps its ServiceError contract.
    pub(crate) async fn list_course_homework_read(
        &self,
        course_id: &str,
    ) -> Result<Vec<LearnHomeworkRecord>, CampusTodoFailureKind> {
        validate_homework_course_id(course_id).map_err(|_| CampusTodoFailureKind::RouteDrift)?;
        if !matches!(self.learn.config().profile.role, CourseRole::Student) {
            return Err(CampusTodoFailureKind::RouteDrift);
        }

        let mut records = Vec::new();
        for bucket in [
            HomeworkBucket::Pending,
            HomeworkBucket::Submitted,
            HomeworkBucket::Graded,
        ] {
            records.extend(
                self.fetch_homework_page(course_id, bucket)
                    .await
                    .map_err(|error| error.kind)?,
            );
        }
        let (records, conflicts) = consistent_homework_records(records);
        if conflicts != 0 {
            return Err(CampusTodoFailureKind::MalformedResponse);
        }
        Ok(records)
    }

    async fn fetch_course_records(&self) -> Result<Vec<LearnCourseRecord>, ServiceError> {
        let csrf = self
            .csrf
            .as_ref()
            .ok_or_else(|| adapter_error("learning todo CSRF token is not configured"))?;
        let csrf_parameter = self
            .csrf_parameter
            .as_deref()
            .ok_or_else(|| adapter_error("learning todo CSRF query parameter is not configured"))?;
        if let Some(courses) = &self.proven_courses {
            return Ok(courses.clone());
        }
        self.learn
            .fetch_course_records_with_csrf_parameter(
                &self.transport,
                csrf,
                &self.config.semester,
                Some(&self.config.language),
                self.config.course_index.as_deref(),
                csrf_parameter,
            )
            .await
            .map_err(map_learn_client_error)
    }

    async fn fetch_homework_page(
        &self,
        course_id: &str,
        bucket: HomeworkBucket,
    ) -> Result<Vec<LearnHomeworkRecord>, LearnTodoProviderError> {
        let endpoint = self
            .learn
            .resolve_endpoint(bucket.path())
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::RouteDrift))?;
        let endpoint = self
            .append_csrf_to_url(endpoint)
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::Transport))?;
        let expected_path = endpoint.path().to_owned();
        let form = PageListForm::for_course(course_id)
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::MalformedResponse))?;
        let request = self
            .transport
            .client()
            .post(endpoint)
            .header("Content-Type", LEARN_FORM_CONTENT_TYPE)
            .body(form.multipart_body())
            .build()
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::Transport))?;
        let (status, final_url, redirect_location, body) = self.execute_request(request).await?;
        self.ensure_not_expired(status, &final_url, redirect_location.as_deref(), &body)?;
        if endpoint_is_allowed(&self.learn.config().base_url, &final_url)
            && final_url.path() != expected_path
        {
            return Err(LearnTodoProviderError::new(
                CampusTodoFailureKind::RouteDrift,
            ));
        }
        let csrf_parameter = self
            .csrf_parameter
            .as_deref()
            .ok_or_else(|| LearnTodoProviderError::new(CampusTodoFailureKind::Transport))?;
        let csrf_value = self
            .csrf
            .as_ref()
            .ok_or_else(|| LearnTodoProviderError::new(CampusTodoFailureKind::Transport))?
            .as_csrf_token();
        if endpoint_is_allowed(&self.learn.config().base_url, &final_url)
            && !query_matches_exactly(&final_url, &[(csrf_parameter, csrf_value.as_str())])
        {
            return Err(LearnTodoProviderError::new(
                CampusTodoFailureKind::RouteDrift,
            ));
        }
        if !status.is_success() {
            return Err(LearnTodoProviderError::new(
                CampusTodoFailureKind::HttpStatus,
            ));
        }
        parse_homework_list_for_course(&body, bucket, Some(course_id)).map_err(|error| {
            let (date_shape, date_width) = safe_homework_date_shape(&error);
            tracing::warn!(
                target: "tsinghua_kit::api",
                event = "homework_parse_rejected",
                bucket = match bucket {
                    HomeworkBucket::Pending => "pending",
                    HomeworkBucket::Submitted => "submitted",
                    HomeworkBucket::Graded => "graded",
                },
                parse_reason = safe_homework_parse_reason(&error),
                parse_field = safe_homework_parse_field(&error),
                body_shape = safe_homework_body_shape(&body),
                date_shape,
                date_width,
            );
            let kind = match &error {
                LearnTodoParseError::ResultFailure { message }
                    if is_login_failure_message(message) =>
                {
                    CampusTodoFailureKind::SessionExpired
                }
                LearnTodoParseError::ResultFailure { .. } => CampusTodoFailureKind::BusinessFailure,
                _ => CampusTodoFailureKind::MalformedResponse,
            };
            LearnTodoProviderError::new(kind)
        })
    }

    async fn execute_request(
        &self,
        request: reqwest::Request,
    ) -> Result<(StatusCode, Url, Option<String>, String), LearnTodoProviderError> {
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::Transport))?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::RouteDrift))?;
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|_| LearnTodoProviderError::new(CampusTodoFailureKind::Transport))?;
        Ok((status, final_url, redirect_location, body))
    }

    fn append_csrf(&self, url: &mut Url) -> Result<(), ServiceError> {
        let csrf = self
            .csrf
            .as_ref()
            .ok_or_else(|| adapter_error("learning todo CSRF token is not configured"))?;
        let parameter_name = self
            .csrf_parameter
            .as_deref()
            .ok_or_else(|| adapter_error("learning todo CSRF query parameter is not configured"))?;
        url.query_pairs_mut()
            .append_pair(parameter_name, csrf.as_csrf_token().as_str());
        Ok(())
    }

    fn append_csrf_to_url(&self, mut url: Url) -> Result<Url, ServiceError> {
        self.append_csrf(&mut url)?;
        Ok(url)
    }

    fn ensure_not_expired(
        &self,
        status: StatusCode,
        final_url: &Url,
        redirect_location: Option<&str>,
        body: &str,
    ) -> Result<(), LearnTodoProviderError> {
        let mapped = self
            .learn
            .map_html_response_with_redirect(status, Some(final_url), redirect_location, body)
            .map_err(|error| LearnTodoProviderError::new(classify_learn_client_error(&error)))?;
        if matches!(
            mapped.classification,
            LearnPageClassification::LoginExpired(_)
        ) {
            return Err(LearnTodoProviderError::new(
                CampusTodoFailureKind::SessionExpired,
            ));
        }
        Ok(())
    }

    async fn list_todos_report_inner(
        &self,
        filter: TodoFilter,
    ) -> Result<CampusTodoReadReport, ServiceError> {
        if !matches!(self.learn.config().profile.role, CourseRole::Student) {
            return Err(adapter_error(
                "teacher learning assignment source is not configured",
            ));
        }

        let courses = self.fetch_course_records().await?;
        let mut todos = Vec::new();
        let mut failures = Vec::new();
        for course in courses {
            let Some(remote_course_id) = course.id().filter(|value| !value.trim().is_empty())
            else {
                return Err(adapter_error(
                    "learning course response contains a course without wlkcid",
                ));
            };
            let domain_course_id = course_domain_id(&course);
            if filter.course_id.is_some_and(|id| id != domain_course_id) {
                continue;
            }

            let mut course_records = Vec::new();
            for bucket in [
                HomeworkBucket::Pending,
                HomeworkBucket::Submitted,
                HomeworkBucket::Graded,
            ] {
                match self.fetch_homework_page(remote_course_id, bucket).await {
                    Ok(records) => course_records.extend(records),
                    Err(error) if error.kind == CampusTodoFailureKind::SessionExpired => {
                        // A Learn session is shared by all three providers;
                        // continuing after its expiry would only create more
                        // misleading provider failures and send unnecessary
                        // requests with a stale CSRF token.
                        return Err(ServiceError::SessionExpired {
                            service: ServiceId::Learn,
                        });
                    }
                    Err(error) => failures.push(CampusTodoProviderFailure {
                        course_id: domain_course_id,
                        provider: homework_provider_name(bucket).to_owned(),
                        kind: error.kind,
                    }),
                }
            }
            let (course_records, conflicts) = consistent_homework_records(course_records);
            if conflicts != 0 {
                failures.push(CampusTodoProviderFailure {
                    course_id: domain_course_id,
                    provider: "learn_homework_consistency".to_owned(),
                    kind: CampusTodoFailureKind::MalformedResponse,
                });
            }
            for record in course_records {
                match map_homework_record(record, domain_course_id) {
                    Some(todo) => todos.push(todo),
                    None => failures.push(CampusTodoProviderFailure {
                        course_id: domain_course_id,
                        provider: "learn_homework_mapping".to_owned(),
                        kind: CampusTodoFailureKind::MalformedResponse,
                    }),
                }
            }
        }

        // IDs exposed to the domain are expected to identify one task. If
        // independently scoped course responses reuse an ID inconsistently,
        // do not silently drop one course's task at the final deduplication.
        let mut first_by_id = HashMap::new();
        let mut conflicting_ids = HashSet::new();
        for (index, todo) in todos.iter().enumerate() {
            if let Some(first) = first_by_id.get(&todo.id).copied() {
                if todos[first] != *todo {
                    conflicting_ids.insert(todo.id);
                }
            } else {
                first_by_id.insert(todo.id, index);
            }
        }
        let mut affected_courses = HashSet::new();
        for todo in &todos {
            if conflicting_ids.contains(&todo.id)
                && let Some(course_id) = todo.course_id
                && affected_courses.insert(course_id)
            {
                failures.push(CampusTodoProviderFailure {
                    course_id,
                    provider: "learn_homework_identity".to_owned(),
                    kind: CampusTodoFailureKind::MalformedResponse,
                });
            }
        }
        let mut seen = HashSet::new();
        todos.retain(|todo| !conflicting_ids.contains(&todo.id) && seen.insert(todo.id));
        todos.retain(|todo| filter.include_completed || !todo.status.is_closed());
        todos.retain(|todo| {
            filter
                .due_before
                .is_none_or(|due_before| todo.due_at.is_some_and(|due_at| due_at < due_before))
        });
        sort_todos(&mut todos);
        Ok(CampusTodoReadReport {
            items: todos,
            failures,
        })
    }
}

#[async_trait]
impl CampusTodoSource for LearnTodoSource {
    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError> {
        let report = self.list_todos_report_inner(filter).await?;
        if !report.failures.is_empty() {
            return Err(provider_failures_to_service(&report.failures));
        }
        Ok(report.items)
    }

    async fn list_todos_report(
        &self,
        filter: TodoFilter,
    ) -> Result<CampusTodoReadReport, ServiceError> {
        self.list_todos_report_inner(filter).await
    }

    async fn list_todos_for_courses(
        &self,
        courses: Vec<LearnCourseRecord>,
        filter: TodoFilter,
    ) -> Result<CampusTodoReadReport, ServiceError> {
        // Clone keeps the same transport and bound CSRF; only this request's
        // in-memory course input changes. It is neither global nor persisted.
        self.clone()
            .with_proven_courses(courses)
            .list_todos_report_inner(filter)
            .await
    }

    async fn create_todo(&self, _input: NewTodo) -> Result<TodoItem, ServiceError> {
        Err(adapter_error(
            "learning assignments do not support local todo creation",
        ))
    }

    async fn set_todo_status(
        &self,
        _id: Uuid,
        _status: TodoStatus,
    ) -> Result<TodoItem, ServiceError> {
        Err(adapter_error(
            "learning assignment status is managed by the learning platform",
        ))
    }
}

// This is a deterministic testable boundary made only from RFC 2046
// boundary characters. The actual `aoData` value is a multipart field; it is
// deliberately not encoded into the boundary or confused with a query
// parameter.
const LEARN_FORM_BOUNDARY: &str = "----THYouLearnFormBoundary7ma4YWxkTrZu0gW";
const LEARN_FORM_CONTENT_TYPE: &str =
    "multipart/form-data; boundary=----THYouLearnFormBoundary7ma4YWxkTrZu0gW";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PageListForm {
    ao_data: String,
}

impl PageListForm {
    fn for_course(course_id: &str) -> Result<Self, serde_json::Error> {
        Ok(Self {
            ao_data: serde_json::to_string(&[json!({
                "name": "wlkcid",
                "value": course_id,
            })])?,
        })
    }

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

/// Collapse exact copies, but never choose one of two conflicting states.
/// A task may move between lists while serial requests are in flight. Keep
/// the unrelated records usable and surface inconsistency explicitly.
fn consistent_homework_records(
    records: Vec<LearnHomeworkRecord>,
) -> (Vec<LearnHomeworkRecord>, usize) {
    let mut positions = HashMap::new();
    let mut unique: Vec<LearnHomeworkRecord> = Vec::new();
    let mut conflicts = HashSet::new();
    for record in records {
        let Some(id) = record
            .student_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .cloned()
        else {
            conflicts.insert(String::new());
            continue;
        };
        match positions.get(&id).copied() {
            Some(index) if unique[index] != record => {
                conflicts.insert(id);
            }
            Some(_) => {}
            None => {
                positions.insert(id, unique.len());
                unique.push(record);
            }
        }
    }
    unique.retain(|record| {
        record
            .student_id
            .as_ref()
            .is_some_and(|id| !conflicts.contains(id))
    });
    (unique, conflicts.len())
}

fn map_homework_record(record: LearnHomeworkRecord, course_id: Uuid) -> Option<TodoItem> {
    let identity = record
        .student_id
        .as_deref()
        .or(record.base_id.as_deref())?
        .trim();
    if identity.is_empty() {
        return None;
    }
    let title = record.title.as_deref()?.trim();
    if title.is_empty() {
        return None;
    }

    // A domain timestamp is required here.  Using the local clock would make
    // an upstream record look newer than it is and would fabricate data when
    // the response is incomplete.
    let created_at = record
        .created_at
        .or(record.submitted_at)
        .or(record.graded_at)
        // Pending rows in the real Learn lists can omit publication,
        // submission, and grading timestamps. The due dates are still
        // upstream timestamps and keep those rows visible without using the
        // local clock to fabricate a creation time.
        .or(record.due_at)
        .or(record.late_due_at)?;
    let updated_at = record
        .graded_at
        .or(record.submitted_at)
        .or(record.created_at)
        .unwrap_or(created_at);
    let status = record.bucket.status();

    Some(TodoItem {
        id: stable_uuid("learn-homework", identity),
        title: title.to_owned(),
        description: None,
        status,
        priority: priority_for(record.due_at, status),
        due_at: record.due_at.or(record.late_due_at),
        course_id: Some(course_id),
        created_at,
        updated_at,
    })
}

fn priority_for(due_at: Option<DateTime<Utc>>, status: TodoStatus) -> TodoPriority {
    if status.is_closed() {
        return TodoPriority::Normal;
    }
    let Some(due_at) = due_at else {
        return TodoPriority::Normal;
    };
    let remaining = due_at.signed_duration_since(Utc::now());
    if remaining <= chrono::Duration::hours(24) {
        TodoPriority::Urgent
    } else if remaining <= chrono::Duration::days(3) {
        TodoPriority::High
    } else {
        TodoPriority::Normal
    }
}

fn course_domain_id(course: &LearnCourseRecord) -> Uuid {
    let identity = course
        .code()
        .or(course.id())
        .or(course.title())
        .unwrap_or("unknown-course");
    stable_uuid("learn-course", identity)
}

pub(crate) fn stable_uuid(namespace: &str, value: &str) -> Uuid {
    let mut high = 0xcbf29ce484222325_u64;
    let mut low = 0x84222325cb29ce4_u64;
    for byte in namespace.bytes().chain(*b":").chain(value.bytes()) {
        high ^= u64::from(byte);
        high = high.wrapping_mul(0x100000001b3);
        low ^= u64::from(byte).rotate_left(1);
        low = low.wrapping_mul(0x100000001b3);
    }
    Uuid::from_u128((u128::from(high) << 64) | u128::from(low))
}

fn sort_todos(todos: &mut [TodoItem]) {
    todos.sort_by(|left, right| {
        left.status
            .is_closed()
            .cmp(&right.status.is_closed())
            .then_with(|| right.priority.rank().cmp(&left.priority.rank()))
            .then_with(|| compare_optional_dates(left.due_at, right.due_at))
            .then_with(|| left.created_at.cmp(&right.created_at))
    });
}

fn compare_optional_dates(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn parse_optional_datetime(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &str,
) -> Result<Option<DateTime<Utc>>, LearnTodoParseError> {
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
    let Some(raw) = scalar_text(Some(value)) else {
        return Err(LearnTodoParseError::InvalidDate {
            index,
            field: field.to_owned(),
            value: value.to_string(),
        });
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    parse_learning_datetime(&raw)
        .map(Some)
        .ok_or_else(|| LearnTodoParseError::InvalidDate {
            index,
            field: field.to_owned(),
            value: raw,
        })
}

fn parse_learning_datetime(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Some(value.with_timezone(&Utc));
    }
    // The current student list may send a Unix millisecond number, matching
    // the reference client's `new Date(h.jzsj)`. Require exactly thirteen
    // ASCII digits so a seconds value or malformed timestamp is not guessed.
    if value.len() == 13 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return DateTime::<Utc>::from_timestamp_millis(value.parse::<i64>().ok()?);
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

fn result_is_success(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| value == "success")
}

fn required_scalar(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &'static str,
) -> Result<String, LearnTodoParseError> {
    let Some(value) = aliases.iter().find_map(|alias| {
        object
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(alias).then_some(value))
    }) else {
        return Err(LearnTodoParseError::MissingField { index, field });
    };
    scalar_text(Some(value))
        .filter(|value| !value.trim().is_empty())
        .ok_or(LearnTodoParseError::InvalidField { index, field })
}

fn optional_scalar(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &'static str,
) -> Result<Option<String>, LearnTodoParseError> {
    let Some(value) = aliases.iter().find_map(|alias| {
        object
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(alias).then_some(value))
    }) else {
        return Ok(None);
    };
    scalar_text(Some(value))
        .filter(|value| !value.trim().is_empty())
        .map(Some)
        .ok_or(LearnTodoParseError::InvalidField { index, field })
}

fn required_text_scalar(
    object: &Map<String, Value>,
    aliases: &[&str],
    index: usize,
    field: &'static str,
) -> Result<String, LearnTodoParseError> {
    let Some(value) = aliases.iter().find_map(|alias| {
        object
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(alias).then_some(value))
    }) else {
        return Err(LearnTodoParseError::MissingField { index, field });
    };
    match value {
        Value::String(value) if !value.trim().is_empty() => {
            Ok(crate::learn_client::decode_learn_html(value.trim()))
        }
        _ => Err(LearnTodoParseError::InvalidField { index, field }),
    }
}

fn scalar_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn map_learn_client_error(error: LearnClientError) -> ServiceError {
    if matches!(&error, LearnClientError::SessionExpired)
        || matches!(
            &error,
            LearnClientError::Transport(TransportError::DecodeBody { message })
                if message.contains("login page")
        )
    {
        return ServiceError::SessionExpired {
            service: ServiceId::Learn,
        };
    }
    adapter_error(format!("learning todo client failed: {error}"))
}

fn adapter_error(message: impl Into<String>) -> ServiceError {
    ServiceError::Adapter {
        message: message.into(),
    }
}

fn validate_homework_course_id(course_id: &str) -> Result<(), ServiceError> {
    if course_id.trim().is_empty() || course_id.chars().any(char::is_control) {
        return Err(adapter_error("learning assignment course id is invalid"));
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

fn classify_learn_client_error(error: &LearnClientError) -> CampusTodoFailureKind {
    match error {
        LearnClientError::SessionExpired => CampusTodoFailureKind::SessionExpired,
        LearnClientError::UnexpectedOrigin
        | LearnClientError::UnexpectedPath
        | LearnClientError::UnexpectedQuery => CampusTodoFailureKind::RouteDrift,
        LearnClientError::Transport(TransportError::HttpStatus { .. }) => {
            CampusTodoFailureKind::HttpStatus
        }
        LearnClientError::Transport(TransportError::Request(_))
        | LearnClientError::Transport(TransportError::Decode(_)) => {
            CampusTodoFailureKind::Transport
        }
        LearnClientError::CourseJson(error) => match error {
            crate::learn_client::CourseJsonError::BusinessFailure { .. } => {
                CampusTodoFailureKind::BusinessFailure
            }
            _ => CampusTodoFailureKind::MalformedResponse,
        },
        _ => CampusTodoFailureKind::MalformedResponse,
    }
}

fn homework_provider_name(bucket: HomeworkBucket) -> &'static str {
    match bucket {
        HomeworkBucket::Pending => "learn_homework_pending",
        HomeworkBucket::Submitted => "learn_homework_submitted",
        HomeworkBucket::Graded => "learn_homework_graded",
    }
}

fn provider_error_to_service(error: LearnTodoProviderError) -> ServiceError {
    match error.kind {
        CampusTodoFailureKind::SessionExpired => ServiceError::SessionExpired {
            service: ServiceId::Learn,
        },
        kind => adapter_error(format!("learning todo provider failed: {kind}")),
    }
}

fn provider_failures_to_service(failures: &[CampusTodoProviderFailure]) -> ServiceError {
    let kinds = failures
        .iter()
        .map(|failure| failure.kind.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    adapter_error(format!(
        "learning todo providers failed: {} ({kinds})",
        failures.len()
    ))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    use chrono::{Datelike, Timelike};
    use serde_json::json;

    use super::*;
    use crate::{
        learn_client::{LearnClient, LearnClientConfig},
        protocol::{CourseRole, CsrfToken, ServiceId},
        session::SessionRegistry,
        transport::CampusHttpTransport,
    };

    #[test]
    fn backend_repair_homework_live_parse_diagnostic_exposes_only_fixed_codes() {
        let bad_date = LearnTodoParseError::InvalidDate {
            index: 7,
            field: "deadline".into(),
            value: "fixture-private-date".into(),
        };
        assert_eq!(safe_homework_parse_reason(&bad_date), "invalid_date");
        assert_eq!(safe_homework_parse_field(&bad_date), "deadline");
        let mismatch = LearnTodoParseError::CourseMismatch {
            index: 0,
            expected: "fixture-private-course-one".into(),
            actual: "fixture-private-course-two".into(),
        };
        assert_eq!(safe_homework_parse_reason(&mismatch), "course_mismatch");
        assert_eq!(safe_homework_parse_field(&mismatch), "none");
        assert_eq!(
            safe_homework_body_shape(" <p>fixture-private-body</p>"),
            "html"
        );
    }

    #[test]
    fn backend_repair_homework_date_shape_diagnostic_never_contains_date_value() {
        let error = LearnTodoParseError::InvalidDate {
            index: 0,
            field: "deadline".into(),
            value: "2031-07-09T12:34:56".into(),
        };
        assert_eq!(safe_homework_date_shape(&error), ("iso_t_local", "w19"));
        let error = LearnTodoParseError::InvalidDate {
            index: 0,
            field: "deadline".into(),
            value: "2031-07-09 12:34:56 CST".into(),
        };
        assert_eq!(
            safe_homework_date_shape(&error),
            ("dash_space_alpha", "w23")
        );
    }

    #[test]
    fn backend_repair_homework_epoch_milliseconds_keep_absolute_deadline() {
        let deadline = Utc.with_ymd_and_hms(2026, 10, 2, 15, 30, 0).unwrap();
        let millis = deadline.timestamp_millis();
        let response = format!(
            r#"{{"result":"success","object":{{"aaData":[{{"wlkcid":"fixture-course","xszyid":"fixture-student","zyid":"fixture-base","bt":"Fixture","jzsj":{millis}}}]}}}}"#,
        );
        let records = parse_homework_list_for_course(
            &response,
            HomeworkBucket::Pending,
            Some("fixture-course"),
        )
        .unwrap();
        assert_eq!(records[0].due_at, Some(deadline));
        assert_eq!(parse_learning_datetime(&millis.to_string()), Some(deadline));
        assert!(parse_learning_datetime("1721234567").is_none());
        assert!(parse_learning_datetime("17212345678901").is_none());
    }

    #[tokio::test]
    async fn backend_repair_overview_proven_course_input_is_request_local_and_skips_course_get() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let learn = LearnClient::new(LearnClientConfig::new(&base, CourseRole::Student).unwrap());
        let mut source = LearnTodoSource::new(
            learn,
            CampusHttpTransport::new("THYou/fixture").unwrap(),
            LearnTodoConfig::new("2026-2027-1"),
        )
        .unwrap();
        let registry = SessionRegistry::new();
        source
            .with_csrf(
                registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture").unwrap()),
                "_csrf",
            )
            .unwrap();
        let report = source
            .list_todos_for_courses(Vec::new(), TodoFilter::default())
            .await
            .unwrap();
        assert!(report.items.is_empty());
        assert!(report.failures.is_empty());
        assert!(source.proven_courses.is_none());
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    const HOMEWORK_FIXTURE: &str = r#"
        {
          "result": "success",
          "object": {
            "aaData": [
              {
                "xszyid": "student-homework-1",
                "zyid": "homework-1",
                "wlkcid": "7001",
                "bt": "提交数据结构作业",
                "jzsj": "2026-09-13 23:59:59",
                "bjjzsj": null,
                "scsj": null,
                "pysj": null,
                "fbsj": "2026-09-10 08:00:00"
              }
            ]
          }
        }
    "#;

    #[tokio::test]
    async fn backend_repair_todo_reuses_proven_course_response_without_refetch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let learn = LearnClient::new(
            LearnClientConfig::new(
                &format!("http://{}/", listener.local_addr().unwrap()),
                CourseRole::Student,
            )
            .unwrap(),
        );
        let transport = CampusHttpTransport::new("THYou/fixture").unwrap();
        let mut source =
            LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2025-2026-2"))
                .unwrap()
                .with_proven_courses(Vec::new());
        assert!(source.fetch_course_records().await.is_err());
        let registry = SessionRegistry::new();
        source
            .with_csrf(
                registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture").unwrap()),
                "_csrf",
            )
            .unwrap();
        assert!(
            source
                .list_todos(TodoFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
        listener.set_nonblocking(true).unwrap();
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn parses_current_student_assignment_wrapper_and_beijing_deadline() {
        let records = parse_homework_list(HOMEWORK_FIXTURE, HomeworkBucket::Pending)
            .expect("assignment fixture parses");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].student_id.as_deref(), Some("student-homework-1"));
        assert_eq!(records[0].base_id.as_deref(), Some("homework-1"));
        assert_eq!(records[0].title.as_deref(), Some("提交数据结构作业"));
        assert_eq!(records[0].bucket, HomeworkBucket::Pending);
        let deadline = records[0].due_at.expect("deadline exists");
        assert_eq!(deadline.hour(), 15);
        assert_eq!(deadline.day(), 13);
    }

    #[test]
    fn rejects_failed_or_malformed_assignment_responses() {
        assert!(matches!(
            parse_homework_list(
                r#"{"result":"error","msg":"session expired"}"#,
                HomeworkBucket::Pending
            ),
            Err(LearnTodoParseError::ResultFailure { .. })
        ));
        assert!(matches!(
            parse_homework_list(
                r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","jzsj":"not-a-date"}]}}"#,
                HomeworkBucket::Pending
            ),
            Err(LearnTodoParseError::InvalidDate { field, .. }) if field == "deadline"
        ));
        assert!(matches!(
            parse_homework_list(
                r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","xszyid":"missing-time","zyid":"base-missing-time","bt":"没有时间的作业"}]}}"#,
                HomeworkBucket::Pending
            ),
            Err(LearnTodoParseError::MissingField {
                field: "deadline",
                ..
            })
        ));
        assert!(matches!(
            parse_homework_list("[]", HomeworkBucket::Pending),
            Err(LearnTodoParseError::InvalidRoot)
        ));
        let scoped =
            parse_homework_list_for_course(HOMEWORK_FIXTURE, HomeworkBucket::Pending, Some("7001"))
                .expect("matching course binding parses");
        assert_eq!(scoped.len(), 1);
        assert!(matches!(
            parse_homework_list_for_course(
                r#"{"result":"success","object":{"aaData":[{"xszyid":"1","zyid":"base-1","wlkcid":"7002","bt":"作业","jzsj":"2026-09-20"}]}}"#,
                HomeworkBucket::Pending,
                Some("7001"),
            ),
            Err(LearnTodoParseError::CourseMismatch {
                expected,
                actual,
                ..
            }) if expected == "7001" && actual == "7002"
        ));
        assert!(matches!(
            parse_homework_list_for_course(
                r#"{"result":"success","object":{"aaData":[{"xszyid":"1","zyid":"base-1","wlkcid":{},"bt":"作业","jzsj":"2026-09-20"}]}}"#,
                HomeworkBucket::Pending,
                Some("7001"),
            ),
            Err(LearnTodoParseError::InvalidField {
                index: 0,
                field: "course id",
            })
        ));
        assert!(matches!(
            parse_homework_list(
                r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","id":"student-1","title":"未知字段","deadline":"2026-09-20"}]}}"#,
                HomeworkBucket::Pending
            ),
            Err(LearnTodoParseError::MissingField {
                field: "student assignment id",
                ..
            })
        ));
        let decoded = parse_homework_list(
            r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","xszyid":"student-1","zyid":"base-1","bt":"数据结构 &amp; 算法","jzsj":"2026-09-20"}]}}"#,
            HomeworkBucket::Pending,
        )
        .expect("HTML entities in a real title decode");
        assert_eq!(decoded[0].title.as_deref(), Some("数据结构 & 算法"));
    }

    #[test]
    fn page_list_form_matches_learning_platform_parameter_shape() {
        let form = PageListForm::for_course("7001").expect("form serializes");
        assert_eq!(
            form.ao_data,
            json!([{"name":"wlkcid","value":"7001"}]).to_string()
        );
    }

    #[test]
    fn maps_assignment_to_stable_todo_identity_and_status() {
        let due_at = parse_learning_datetime("2026-09-13 23:59:59");
        let todo = map_homework_record(
            LearnHomeworkRecord {
                student_id: Some("student-homework-1".to_owned()),
                base_id: Some("homework-1".to_owned()),
                title: Some("提交作业".to_owned()),
                due_at,
                late_due_at: None,
                submitted_at: None,
                graded_at: None,
                created_at: due_at,
                bucket: HomeworkBucket::Pending,
            },
            stable_uuid("learn-course", "30240512"),
        )
        .expect("todo maps");
        assert_eq!(todo.status, TodoStatus::Pending);
        assert_eq!(
            todo.course_id,
            Some(stable_uuid("learn-course", "30240512"))
        );
        assert_eq!(todo.id, stable_uuid("learn-homework", "student-homework-1"));
        assert!(todo.priority.rank() >= TodoPriority::Normal.rank());
    }

    #[test]
    fn keeps_pending_assignment_when_only_remote_due_time_exists() {
        let due_at = parse_learning_datetime("2026-09-20 23:59:59").expect("fixture date");
        let todo = map_homework_record(
            LearnHomeworkRecord {
                student_id: Some("pending-without-publication-time".to_owned()),
                base_id: Some("base-pending".to_owned()),
                title: Some("待提交作业".to_owned()),
                due_at: Some(due_at),
                late_due_at: None,
                submitted_at: None,
                graded_at: None,
                created_at: None,
                bucket: HomeworkBucket::Pending,
            },
            stable_uuid("learn-course", "30240512"),
        )
        .expect("due date is a sufficient remote timestamp");

        assert_eq!(todo.status, TodoStatus::Pending);
        assert_eq!(todo.due_at, Some(due_at));
        assert_eq!(todo.created_at, due_at);
        assert_eq!(todo.updated_at, due_at);
    }

    #[tokio::test]
    async fn source_fetches_course_and_three_assignment_lists_with_csrf() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local listener");
        listener
            .set_nonblocking(true)
            .expect("listener becomes nonblocking");
        let base_url = format!(
            "http://{}/",
            listener.local_addr().expect("listener address")
        );

        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            while requests.len() < 4 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_http_request(&mut stream);
                        let target = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or_default();
                        let body = if target.contains("loadCourseBySemesterId") {
                            r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240512","kcm":"数据结构"}]}"#
                        } else if target.contains("zyListWj") {
                            r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","xszyid":"pending-1","zyid":"base-1","bt":"提交作业","jzsj":"2026-09-20 23:59:59","fbsj":"2026-09-10 08:00:00"}]}}"#
                        } else if target.contains("zyListYjwg") {
                            r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","xszyid":"submitted-1","zyid":"base-2","bt":"补交材料","jzsj":"2026-09-21 23:59:59","scsj":"2026-09-11 08:00:00","fbsj":"2026-09-10 08:00:00"}]}}"#
                        } else {
                            r#"{"result":"success","object":{"aaData":[{"wlkcid":"7001","xszyid":"graded-1","zyid":"base-3","bt":"已批改作业","jzsj":"2026-09-19 23:59:59","scsj":"2026-09-10 08:00:00","pysj":"2026-09-12 08:00:00","fbsj":"2026-09-09 08:00:00"}]}}"#
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        stream
                            .write_all(response.as_bytes())
                            .expect("fixture response writes");
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            requests
        });

        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let mut source =
            LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2025-2026-2"))
                .expect("todo source");
        let registry = SessionRegistry::new();
        source
            .with_csrf(
                registry.bind_csrf(
                    ServiceId::Learn,
                    CsrfToken::new("csrf-token").expect("csrf token"),
                ),
                "_csrf",
            )
            .expect("source CSRF binding");

        let todos = source
            .list_todos(TodoFilter {
                include_completed: true,
                ..TodoFilter::default()
            })
            .await
            .expect("live assignment source loads");
        let requests = server.join().expect("fixture server joins");

        assert_eq!(todos.len(), 3);
        assert!(todos.iter().any(|todo| todo.status == TodoStatus::Pending));
        assert!(
            todos
                .iter()
                .any(|todo| todo.status == TodoStatus::InProgress)
        );
        assert!(
            todos
                .iter()
                .any(|todo| todo.status == TodoStatus::Completed)
        );
        assert_eq!(requests.len(), 4);
        for request in &requests {
            assert!(request.contains("_csrf=csrf-token"));
        }
        assert!(requests.iter().any(|request| {
            request
                .to_ascii_lowercase()
                .contains("content-type: multipart/form-data; boundary=")
                && request.contains("name=\"aoData\"")
                && request.contains(r#"[{"name":"wlkcid","value":"7001"}]"#)
        }));
    }

    #[tokio::test]
    async fn course_scoped_homework_reads_all_three_real_buckets_without_course_discovery() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local listener");
        listener
            .set_nonblocking(true)
            .expect("listener becomes nonblocking");
        let base_url = format!(
            "http://{}/",
            listener.local_addr().expect("listener address")
        );

        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            while requests.len() < 3 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_http_request(&mut stream);
                        let target = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or_default();
                        let (id, title) = if target.contains("zyListWj") {
                            ("pending-direct", "待提交")
                        } else if target.contains("zyListYjwg") {
                            ("submitted-direct", "已提交")
                        } else {
                            ("graded-direct", "已批改")
                        };
                        let body = format!(
                            r#"{{"result":"success","object":{{"aaData":[{{"wlkcid":"7001","xszyid":"{id}","zyid":"base-{id}","bt":"{title}","jzsj":"2026-09-20 23:59:59"}}]}}}}"#
                        );
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        stream
                            .write_all(response.as_bytes())
                            .expect("fixture response writes");
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            requests
        });

        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let mut source =
            LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2025-2026-2"))
                .expect("todo source");
        let registry = SessionRegistry::new();
        source
            .with_csrf(
                registry.bind_csrf(
                    ServiceId::Learn,
                    CsrfToken::new("csrf-token").expect("csrf token"),
                ),
                "_csrf",
            )
            .expect("source CSRF binding");

        let records = source
            .list_course_homework("7001")
            .await
            .expect("course-scoped assignment source loads");
        let requests = server.join().expect("fixture server joins");

        assert_eq!(records.len(), 3);
        assert_eq!(requests.len(), 3);
        for request in &requests {
            assert!(request.contains("_csrf=csrf-token"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("content-type: multipart/form-data; boundary=")
                    && request.contains("name=\"aoData\"")
                    && request.contains("wlkcid")
                    && request.contains("7001")
            );
        }
        assert!(requests.iter().any(|request| request.contains("zyListWj")));
        assert!(
            requests
                .iter()
                .any(|request| request.contains("zyListYjwg"))
        );
        assert!(requests.iter().any(|request| request.contains("zyListYpg")));
    }

    #[tokio::test]
    async fn backend_repair_api_audit_course_login_html_returns_typed_expiry_contract() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local listener");
        let address = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("course request");
            let request = read_http_request(&mut stream);
            assert!(request.contains("loadCourseBySemesterId"));
            let body = r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("fixture response writes");
        });

        let base_url = format!("http://{address}/");
        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let mut source =
            LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2025-2026-2"))
                .expect("todo source");
        let registry = SessionRegistry::new();
        source
            .with_csrf(
                registry.bind_csrf(
                    ServiceId::Learn,
                    CsrfToken::new("csrf-token").expect("csrf token"),
                ),
                "_csrf",
            )
            .expect("source CSRF binding");

        let error = source
            .list_todos(TodoFilter::default())
            .await
            .expect_err("course login page must fail as an expired session");
        assert!(matches!(
            error,
            ServiceError::SessionExpired {
                service: ServiceId::Learn
            }
        ));
        server.join().expect("fixture server joins");
    }

    #[tokio::test]
    async fn backend_repair_api_audit_assignment_login_redirect_returns_typed_expiry_contract() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local listener");
        listener
            .set_nonblocking(true)
            .expect("listener becomes nonblocking");
        let base_url = format!(
            "http://{}/",
            listener.local_addr().expect("listener address")
        );

        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            while requests.len() < 3 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_http_request(&mut stream);
                        let target = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or_default();
                        let response = if target.contains("loadCourseBySemesterId") {
                            let body = r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240512","kcm":"数据结构"}]}"#;
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            )
                        } else if target.contains("zyListWj") {
                            "HTTP/1.1 302 Found\r\nLocation: /auth/login?url=%2F\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
                        } else {
                            let body = "<html><body>please continue</body></html>";
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            )
                        };
                        stream
                            .write_all(response.as_bytes())
                            .expect("fixture response writes");
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
            requests
        });

        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let mut source =
            LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2025-2026-2"))
                .expect("todo source");
        let registry = SessionRegistry::new();
        source
            .with_csrf(
                registry.bind_csrf(
                    ServiceId::Learn,
                    CsrfToken::new("csrf-token").expect("csrf token"),
                ),
                "_csrf",
            )
            .expect("source CSRF binding");

        let error = source
            .list_todos(TodoFilter::default())
            .await
            .expect_err("login redirect must fail the assignment read");
        assert!(matches!(
            error,
            ServiceError::SessionExpired {
                service: ServiceId::Learn
            }
        ));

        let requests = server.join().expect("fixture server joins");
        assert_eq!(requests.len(), 3);
        assert!(requests[1].starts_with("POST /b/wlxt/kczy/zy/student/zyListWj?_csrf="));
        assert!(requests[1].contains("name=\"aoData\""));
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("content-type: multipart/form-data; boundary=")
        );
        assert!(requests[2].starts_with("GET /auth/login?url=%2F HTTP/1.1"));
    }

    #[test]
    fn source_reuses_student_profile_and_requires_learn_csrf() {
        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let source = LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2025-2026-2"))
            .expect("todo source");
        assert_eq!(source.config().semester, "2025-2026-2");
        assert!(source.csrf.is_none());
    }

    #[test]
    fn source_rejects_noncanonical_semester_ids() {
        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        assert!(LearnTodoSource::new(learn, transport, LearnTodoConfig::new("2026-fall")).is_err());
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_nonblocking(false)
            .expect("request socket becomes blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("request timeout");
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut chunk).expect("request reads");
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
                    .expect("request headers");
                if bytes.len() >= header_end + 4 + expected_len {
                    break;
                }
            } else if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
#[path = "learn_todos_request_audit_tests.rs"]
mod request_audit_tests;
