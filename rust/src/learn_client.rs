//! Read-only request planning and response mapping for Web Learning.
//!
//! The learning site is an HTML/JSON boundary rather than a stable typed API.
//! This module keeps that boundary small: it plans the routes already described
//! by [`crate::learn`], extracts only the CSRF evidence needed by a later
//! request, and maps the small stable subset of course fields used by THYou.
//!
//! The HTML parser is intentionally not a general HTML parser.  It understands
//! quoted and unquoted attributes on `meta`, `input`, and `form` tags, skips
//! comments and raw `script`/`style` blocks, and rejects ambiguous CSRF
//! evidence.  It does not build a DOM or execute scripts, and it never infers
//! authentication from the absence of a marker.  Display-field entity decoding
//! is handled explicitly at the JSON mapping boundary. A real HTML parser can
//! replace this boundary if later features need arbitrary page traversal.

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
};

use reqwest::{StatusCode, Url, header::LOCATION};
use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::learn::{
    DEFAULT_COOKIE_ROAMING_ENTRY_PATH, DEFAULT_ROAMING_ENTRY_PATH, LearnCourseRequest, LearnError,
    LearnProfile, validate_route_path, validate_semester_id,
};
use crate::protocol::{CourseRole, CsrfToken, ServiceId, ServiceTicket};
use crate::session::BoundCsrfToken;
use crate::transport::{CampusHttpTransport, TransportError};
use crate::webvpn_url::{endpoint_is_allowed, resolve_endpoint, same_origin};

const DEFAULT_LEARN_BASE_URL: &str = "https://learn.tsinghua.edu.cn/";
// THUInfo uses the concrete index route; thu-learn-lib documents the role
// directory route. Only these exact aliases are accepted, with a single
// missing-route fallback and independent role-specific CSRF proof.
const DEFAULT_STUDENT_COURSE_HOME_PATH: &str = "/f/wlxt/index/course/student/index";
const DEFAULT_TEACHER_COURSE_HOME_PATH: &str = "/f/wlxt/index/course/teacher/index";
const DEFAULT_CSRF_FIELD_NAME: &str = "_csrf";
const DEFAULT_STUDENT_SEMESTER_LIST_PATH: &str = "/b/wlxt/kc/v_wlkc_xs_xktjb_coassb/queryxnxq";
const DEFAULT_TEACHER_SEMESTER_LIST_PATH: &str = "/b/kc/v_wlkc_jsb_jbxxb/queryxnxq";
const DEFAULT_CURRENT_SEMESTER_PATH: &str = "/b/kc/zhjw_v_code_xnxq/getCurrentAndNextSemester";
const DEFAULT_COURSE_TIME_LOCATION_PATH: &str = "/b/kc/v_wlkc_xk_sjddb/detail";
const DEFAULT_LOGIN_PATH_HINTS: &[&str] = &["/do/off/ui/auth/login", "/auth/login", "/login"];
const DEFAULT_EXPIRED_MARKERS: &[&str] = &[
    "登录失效",
    "登录超时",
    "登录失败",
    "未登录或登录失效",
    "未登录",
    "请重新登录",
    "登录已失效",
    "会话已过期",
    "会话已失效",
    "会话过期",
    "会话失效",
    "认证过期",
    "认证失效",
    "认证失败",
    "统一认证失败",
    "未认证",
    "login_timeout",
    "请先登录",
    "session expired",
    "session invalid",
    "login failed",
    "authentication failed",
    "login required",
    "authentication required",
    "unauthenticated",
    "unauthorized",
];

/// Errors raised while configuring or mapping a Web Learning response.
#[derive(Debug, Error)]
pub enum LearnClientError {
    #[error("invalid learning base URL: {0}")]
    InvalidBaseUrl(String),

    /// The Learn origin explicitly rejected an authenticated request.  Keep
    /// this separate from a malformed response or a temporary transport
    /// failure so the runtime can invalidate only the Learn proof and offer a
    /// service-session recovery action.
    #[error("learning session has expired")]
    SessionExpired,

    #[error("learning response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("learning response ended outside the requested route")]
    UnexpectedPath,

    #[error("learning response did not retain the confirmed request query")]
    UnexpectedQuery,

    #[error("invalid CSRF field name: {0}")]
    InvalidCsrfFieldName(String),

    #[error("the learning CSRF token belongs to another service")]
    ForeignCsrf,

    #[error(transparent)]
    Learn(#[from] LearnError),

    #[error(transparent)]
    Csrf(#[from] CsrfParseError),

    #[error(transparent)]
    CourseJson(#[from] CourseJsonError),

    #[error(transparent)]
    Semester(#[from] SemesterParseError),

    #[error(transparent)]
    CourseTimeLocation(#[from] CourseTimeLocationParseError),

    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Configuration which belongs to one Web Learning deployment and user role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnClientConfig {
    /// The origin used to resolve the relative routes in [`crate::learn`].
    pub base_url: Url,
    /// Student and teacher routes are intentionally separate profiles.
    pub profile: LearnProfile,
    /// The one-time identity roaming route. The current deployment uses the
    /// `/b/` family for a ticket; the cookie-backed route is configured
    /// separately below.
    pub roaming_entry_path: String,
    /// The identity handoff route used when the authenticated Cookie jar is
    /// already carrying a ticketless Learn SSO handoff.
    pub cookie_roaming_entry_path: String,
    /// The authenticated student landing page used to obtain page CSRF
    /// evidence after roaming.
    pub course_home_path: String,
    /// Role-specific semester list route.
    pub semester_list_path: String,
    /// Current/next semester route shared by the student and teacher portals.
    pub current_semester_path: String,
    /// The field name used by both the CSRF meta and hidden input evidence.
    pub csrf_field_name: String,
    /// Lower-case-insensitive URL fragments which identify the identity login
    /// page after a roaming redirect.
    pub login_path_hints: Vec<String>,
    /// Explicit body markers observed on expired-session pages.
    pub expired_markers: Vec<String>,
}

impl LearnClientConfig {
    /// Builds a profile for the supplied learning origin and role.
    pub fn new(base_url: &str, role: CourseRole) -> Result<Self, LearnClientError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            profile: LearnProfile::new(role),
            roaming_entry_path: DEFAULT_ROAMING_ENTRY_PATH.to_owned(),
            cookie_roaming_entry_path: DEFAULT_COOKIE_ROAMING_ENTRY_PATH.to_owned(),
            course_home_path: match role {
                CourseRole::Student => DEFAULT_STUDENT_COURSE_HOME_PATH.to_owned(),
                CourseRole::Teacher => DEFAULT_TEACHER_COURSE_HOME_PATH.to_owned(),
            },
            semester_list_path: match role {
                CourseRole::Student => DEFAULT_STUDENT_SEMESTER_LIST_PATH.to_owned(),
                CourseRole::Teacher => DEFAULT_TEACHER_SEMESTER_LIST_PATH.to_owned(),
            },
            current_semester_path: DEFAULT_CURRENT_SEMESTER_PATH.to_owned(),
            csrf_field_name: DEFAULT_CSRF_FIELD_NAME.to_owned(),
            login_path_hints: DEFAULT_LOGIN_PATH_HINTS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            expired_markers: DEFAULT_EXPIRED_MARKERS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        })
    }

    /// Uses the currently documented learning origin for one role.
    pub fn default_for(role: CourseRole) -> Self {
        Self::new(DEFAULT_LEARN_BASE_URL, role)
            .expect("the built-in learning base URL must be valid")
    }

    pub fn role(&self) -> CourseRole {
        self.profile.role
    }

    /// Selects a deployment-specific one-time roaming route.
    pub fn with_roaming_entry_path(
        mut self,
        path: impl Into<String>,
    ) -> Result<Self, LearnClientError> {
        let path = path.into();
        validate_route_path(&path)?;
        self.roaming_entry_path = path;
        Ok(self)
    }

    /// Selects the ticketless Cookie-backed roaming route independently from
    /// the legacy ticket route.
    pub fn with_cookie_roaming_entry_path(
        mut self,
        path: impl Into<String>,
    ) -> Result<Self, LearnClientError> {
        let path = path.into();
        validate_route_path(&path)?;
        self.cookie_roaming_entry_path = path;
        Ok(self)
    }

    /// Selects the authenticated course landing page used to obtain CSRF.
    pub fn with_course_home_path(
        mut self,
        path: impl Into<String>,
    ) -> Result<Self, LearnClientError> {
        let path = path.into();
        validate_route_path(&path)?;
        self.course_home_path = path;
        Ok(self)
    }

    /// Selects the role-specific semester list route for a deployment.
    pub fn with_semester_list_path(
        mut self,
        path: impl Into<String>,
    ) -> Result<Self, LearnClientError> {
        let path = path.into();
        validate_route_path(&path)?;
        self.semester_list_path = path;
        Ok(self)
    }

    /// Selects the current/next semester route for a deployment.
    pub fn with_current_semester_path(
        mut self,
        path: impl Into<String>,
    ) -> Result<Self, LearnClientError> {
        let path = path.into();
        validate_route_path(&path)?;
        self.current_semester_path = path;
        Ok(self)
    }

    /// Changes the CSRF field name for a deployment-specific page profile.
    pub fn with_csrf_field_name(
        mut self,
        field_name: impl Into<String>,
    ) -> Result<Self, LearnClientError> {
        let field_name = field_name.into();
        validate_field_name(&field_name)?;
        self.csrf_field_name = field_name;
        Ok(self)
    }
}

/// A read-only learning client.  It owns no credentials or cookies; callers
/// pass the existing [`CampusHttpTransport`] when they execute a plan.
#[derive(Debug, Clone)]
pub struct LearnClient {
    config: LearnClientConfig,
}

impl LearnClient {
    pub fn new(config: LearnClientConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &LearnClientConfig {
        &self.config
    }

    /// Plans the one-time roaming entry request without putting the ticket in
    /// the endpoint URL.  The separate query object can be handed to
    /// `CampusHttpTransport::get_text_with_query`, which performs the normal
    /// URL encoding.
    pub fn roaming_request_plan(
        &self,
        ticket: &str,
    ) -> Result<LearnRoamingRequestPlan, LearnClientError> {
        let request =
            LearnProfile::roaming_entry_request_at(&self.config.roaming_entry_path, ticket)?;
        let ticket = ServiceTicket::new(ticket).ok_or(LearnError::EmptyTicket)?;
        let endpoint = self.resolve_path(&request.path)?;

        Ok(LearnRoamingRequestPlan {
            method: LearnRequestMethod::Get,
            endpoint,
            path: request.path,
            query: LearnRoamingQuery::new(ticket),
        })
    }

    /// Plans the authenticated student landing page. The roaming endpoint
    /// may return only a redirect shell, while the landing page contains the
    /// CSRF link used by the current Learn deployment.
    pub fn course_home_request_plan(&self) -> Result<LearnCourseHomeRequestPlan, LearnClientError> {
        let endpoint = self.resolve_path(&self.config.course_home_path)?;
        Ok(LearnCourseHomeRequestPlan {
            method: LearnRequestMethod::Get,
            endpoint,
            path: self.config.course_home_path.clone(),
        })
    }

    /// Plans the role-specific course list request from the configured
    /// [`CourseRole`].  The route and role-specific validation remain in
    /// [`LearnProfile`] so there is one source of truth for those differences.
    pub fn course_list_request_plan(
        &self,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
    ) -> Result<LearnCourseListRequestPlan, LearnClientError> {
        let request = self
            .config
            .profile
            .course_request(semester, language, course_index)?;
        let endpoint = self.resolve_path(&request.path)?;

        Ok(LearnCourseListRequestPlan {
            method: LearnRequestMethod::Get,
            endpoint,
            role: self.config.role(),
            request,
        })
    }

    /// Plans every course-list request required by the configured role.
    /// Student reads produce one request. Teacher reads produce both public
    /// co-course indexes unless one index is explicitly selected.
    pub fn course_list_request_plans(
        &self,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
    ) -> Result<Vec<LearnCourseListRequestPlan>, LearnClientError> {
        self.config
            .profile
            .course_requests(semester, language, course_index)?
            .into_iter()
            .map(|request| {
                let endpoint = self.resolve_path(&request.path)?;
                Ok(LearnCourseListRequestPlan {
                    method: LearnRequestMethod::Get,
                    endpoint,
                    role: self.config.role(),
                    request,
                })
            })
            .collect()
    }

    /// Builds the roaming GET request through the shared cookie-aware
    /// transport.  The request is not sent by this method.
    pub fn build_roaming_request(
        &self,
        transport: &CampusHttpTransport,
        ticket: &str,
    ) -> Result<reqwest::Request, LearnClientError> {
        self.roaming_request_plan(ticket)?.build_request(transport)
    }

    /// Executes the one-time roaming request and retains the final page for
    /// the page classifier. Non-success statuses are returned as data so the
    /// session layer can distinguish them from a transport failure.
    pub async fn execute_roaming(
        &self,
        transport: &CampusHttpTransport,
        ticket: &str,
    ) -> Result<LearnHttpResponse, LearnClientError> {
        let request = self.build_roaming_request(transport, ticket)?;
        let response = self.execute_page_request(transport, request).await?;
        self.validate_roaming_final_path(&response)?;
        Ok(response)
    }

    /// Executes the cookie-backed Learn SSO entry used by the current identity
    /// deployment.  Newer identity success pages link to the roaming route
    /// without a `ticket` query parameter; the shared Cookie jar carries the
    /// authenticated handoff instead.  The route and final response are still
    /// classified by the same Learn session layer, so an arbitrary HTTP 200
    /// page cannot establish a session.
    pub async fn execute_cookie_roaming(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<LearnHttpResponse, LearnClientError> {
        let request = self.build_cookie_roaming_request(transport)?;
        let response = self.execute_page_request(transport, request).await?;
        self.validate_roaming_final_path(&response)?;
        Ok(response)
    }

    /// Builds the cookie-backed roaming request without sending it.
    pub fn build_cookie_roaming_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, LearnClientError> {
        let endpoint = self.resolve_path(&self.config.cookie_roaming_entry_path)?;
        transport
            .client()
            .get(endpoint)
            .build()
            .map_err(|error| LearnClientError::Transport(TransportError::Request(error)))
    }

    async fn execute_page_request(
        &self,
        transport: &CampusHttpTransport,
        request: reqwest::Request,
    ) -> Result<LearnHttpResponse, LearnClientError> {
        let response = transport
            .execute(request)
            .await
            .map_err(|error| LearnClientError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| LearnClientError::UnexpectedOrigin)?
            .map(|location| {
                Url::parse(&location)
                    .or_else(|_| final_url.join(&location))
                    .map_err(|_| LearnClientError::UnexpectedOrigin)
            })
            .transpose()?;
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| LearnClientError::Transport(TransportError::Decode(error)))?;
        Ok(LearnHttpResponse {
            status,
            final_url,
            redirect_location,
            body,
        })
    }

    /// Executes the authenticated landing page after roaming.
    pub async fn execute_course_home(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<LearnHttpResponse, LearnClientError> {
        let plan = self.course_home_request_plan()?;
        let expected_path = plan.endpoint.path().to_owned();
        let request = plan.build_request(transport)?;
        let response = self.execute_page_request(transport, request).await?;
        // A same-origin redirect to another HTML or JSON route is not proof
        // that the course home was reached. Keep an external identity login
        // available to the classifier below, but reject an unrelated route
        // before callers can use its CSRF-looking markup as Learn evidence.
        if endpoint_is_allowed(&self.config.base_url, &response.final_url)
            && response.final_url.path() != expected_path
            && !self
                .documented_home_alias()
                .and_then(|path| self.resolve_path(path).ok())
                .is_some_and(|url| url.path() == response.final_url.path())
            && !self.response_has_login_expiry(&response)
        {
            return Err(LearnClientError::UnexpectedPath);
        }
        Ok(response)
    }

    fn documented_home_alias(&self) -> Option<&'static str> {
        match self.config.course_home_path.as_str() {
            DEFAULT_STUDENT_COURSE_HOME_PATH => Some("/f/wlxt/index/course/student/"),
            DEFAULT_TEACHER_COURSE_HOME_PATH => Some("/f/wlxt/index/course/teacher/"),
            "/f/wlxt/index/course/student/" => Some(DEFAULT_STUDENT_COURSE_HOME_PATH),
            "/f/wlxt/index/course/teacher/" => Some(DEFAULT_TEACHER_COURSE_HOME_PATH),
            _ => None,
        }
    }

    pub(crate) async fn execute_reference_course_home(
        &mut self,
        transport: &CampusHttpTransport,
    ) -> Result<LearnHttpResponse, LearnClientError> {
        let response = self.execute_course_home(transport).await?;
        if endpoint_is_allowed(&self.config.base_url, &response.final_url)
            && let Some(alias) = self.documented_home_alias()
            && self.resolve_path(alias)?.path() == response.final_url.path()
            && response.status == StatusCode::OK
        {
            self.config.course_home_path = alias.to_owned();
            return Ok(response);
        }
        let alternate = match self.config.course_home_path.as_str() {
            DEFAULT_STUDENT_COURSE_HOME_PATH => Some("/f/wlxt/index/course/student/"),
            DEFAULT_TEACHER_COURSE_HOME_PATH => Some("/f/wlxt/index/course/teacher/"),
            _ => None,
        };
        if matches!(response.status.as_u16(), 404 | 410)
            && endpoint_is_allowed(&self.config.base_url, &response.final_url)
            && response.final_url.path() == self.course_home_request_plan()?.endpoint.path()
            && let Some(alternate) = alternate
        {
            tracing::info!(target:"tsinghua_kit::api",event="learn_home_compatibility",service="learn",reason="learn_reference_directory_home");
            self.config.course_home_path = alternate.to_owned();
            return self.execute_course_home(transport).await;
        }
        Ok(response)
    }

    /// Extracts the configured CSRF evidence from a page.
    pub fn parse_csrf(&self, html: &str) -> Result<CsrfEvidence, CsrfParseError> {
        extract_csrf_with_field(html, &self.config.csrf_field_name)
    }

    /// Maps a JSON course response into stable records while retaining fields
    /// which are not part of THYou's current stable subset.
    pub fn map_course_json(&self, body: &str) -> Result<Vec<LearnCourseRecord>, CourseJsonError> {
        parse_course_records(body)
    }

    /// Fetches one role-specific course list through the supplied authenticated
    /// transport.  The CSRF token must already be bound to `ServiceId::Learn`;
    /// accepting a bare string here would make it possible for a caller to
    /// accidentally reuse an INFO or USEREG token.  The response is accepted
    /// only when the request stays on the configured origin and exact route,
    /// the status is successful, and the Learn course envelope parses.
    pub async fn fetch_course_records(
        &self,
        transport: &CampusHttpTransport,
        csrf: &BoundCsrfToken,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
    ) -> Result<Vec<LearnCourseRecord>, LearnClientError> {
        self.fetch_course_records_with_csrf_parameter(
            transport,
            csrf,
            semester,
            language,
            course_index,
            &self.config.csrf_field_name,
        )
        .await
    }

    /// Same strict course-list proof as [`Self::fetch_course_records`], with
    /// an explicit query parameter name for deployments whose form uses a
    /// CSRF field name different from the Learn profile default.  The source
    /// adapter validates this name before calling the method.
    pub async fn fetch_course_records_with_csrf_parameter(
        &self,
        transport: &CampusHttpTransport,
        csrf: &BoundCsrfToken,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
        csrf_parameter: &str,
    ) -> Result<Vec<LearnCourseRecord>, LearnClientError> {
        if csrf.service() != ServiceId::Learn {
            return Err(LearnClientError::ForeignCsrf);
        }
        validate_field_name(csrf_parameter)?;
        let plan = self.course_list_request_plan(semester, language, course_index)?;
        self.fetch_course_records_for_plan(transport, csrf, &plan, csrf_parameter)
            .await
    }

    /// Fetches every role-specific course list and merges duplicate remote
    /// course IDs. A failure in one teacher co-course list is returned instead
    /// of silently dropping that list.
    pub async fn fetch_all_course_records(
        &self,
        transport: &CampusHttpTransport,
        csrf: &BoundCsrfToken,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
        csrf_parameter: &str,
    ) -> Result<Vec<LearnCourseRecord>, LearnClientError> {
        if csrf.service() != ServiceId::Learn {
            return Err(LearnClientError::ForeignCsrf);
        }
        validate_field_name(csrf_parameter)?;
        let plans = self.course_list_request_plans(semester, language, course_index)?;
        let mut records_by_id = BTreeMap::new();
        for plan in plans {
            for record in self
                .fetch_course_records_for_plan(transport, csrf, &plan, csrf_parameter)
                .await?
            {
                let Some(id) = record.id() else {
                    return Err(CourseJsonError::IncompleteRecord {
                        index: None,
                        field: "course id",
                    }
                    .into());
                };
                records_by_id.entry(id.to_owned()).or_insert(record);
            }
        }
        Ok(records_by_id.into_values().collect())
    }

    async fn fetch_course_records_for_plan(
        &self,
        transport: &CampusHttpTransport,
        csrf: &BoundCsrfToken,
        plan: &LearnCourseListRequestPlan,
        csrf_parameter: &str,
    ) -> Result<Vec<LearnCourseRecord>, LearnClientError> {
        let expected_path = plan.endpoint.path().to_owned();
        let mut request = plan.build_request(transport)?;
        request
            .url_mut()
            .query_pairs_mut()
            .append_pair(csrf_parameter, csrf.as_csrf_token().as_str());

        let response = self.execute_page_request(transport, request).await?;

        if self.response_has_login_expiry(&response) {
            return Err(LearnClientError::SessionExpired);
        }
        self.validate_response_origin(&response)?;
        if response.final_url.path() != expected_path {
            return Err(LearnClientError::UnexpectedPath);
        }
        if !query_matches_exactly(
            &response.final_url,
            &[(csrf_parameter, csrf.as_csrf_token().as_str())],
        ) {
            return Err(LearnClientError::UnexpectedQuery);
        }
        if !response.status.is_success() {
            return Err(LearnClientError::Transport(TransportError::HttpStatus {
                status: response.status,
                body: response.body,
            }));
        }
        parse_confirmed_course_list_response_for_semester(
            response.body(),
            Some(plan.request.semester.as_str()),
        )
        .map_err(map_course_json_error)
    }

    /// Plans the secondary request used by the current Learn clients to read
    /// the course's meeting time and location.  The course-list response does
    /// not contain those values reliably; the detail endpoint returns an
    /// explicit JSON array of display strings instead.
    pub fn course_time_location_request_plan(
        &self,
        course_id: &str,
    ) -> Result<LearnCourseTimeLocationRequestPlan, LearnClientError> {
        validate_course_identifier(course_id)?;
        let endpoint = self.resolve_path(DEFAULT_COURSE_TIME_LOCATION_PATH)?;
        Ok(LearnCourseTimeLocationRequestPlan {
            method: LearnRequestMethod::Get,
            endpoint,
            path: DEFAULT_COURSE_TIME_LOCATION_PATH.to_owned(),
            course_id: course_id.to_owned(),
        })
    }

    /// Fetches the real Learn time/location array with the bound Learn CSRF
    /// token.  An empty JSON array is a verified no-meeting result; every
    /// non-empty item must be a non-empty string from the server.
    pub async fn fetch_course_time_location(
        &self,
        transport: &CampusHttpTransport,
        csrf: &BoundCsrfToken,
        course_id: &str,
    ) -> Result<Vec<String>, LearnClientError> {
        if csrf.service() != ServiceId::Learn {
            return Err(LearnClientError::ForeignCsrf);
        }
        let plan = self.course_time_location_request_plan(course_id)?;
        let expected_path = plan.endpoint.path().to_owned();
        let mut request = plan.build_request(transport)?;
        request
            .url_mut()
            .query_pairs_mut()
            .append_pair("id", &plan.course_id);
        request
            .url_mut()
            .query_pairs_mut()
            .append_pair(&self.config.csrf_field_name, csrf.as_csrf_token().as_str());

        let response = self.execute_page_request(transport, request).await?;

        if self.response_has_login_expiry(&response) {
            return Err(LearnClientError::SessionExpired);
        }
        self.validate_response_origin(&response)?;
        if response.final_url.path() != expected_path {
            return Err(LearnClientError::UnexpectedPath);
        }
        if !query_matches_exactly(
            &response.final_url,
            &[
                ("id", plan.course_id.as_str()),
                (
                    self.config.csrf_field_name.as_str(),
                    csrf.as_csrf_token().as_str(),
                ),
            ],
        ) {
            return Err(LearnClientError::UnexpectedQuery);
        }
        if !response.status.is_success() {
            return Err(LearnClientError::Transport(TransportError::HttpStatus {
                status: response.status,
                body: response.body,
            }));
        }
        if body_indicates_login_failure(response.body()) {
            return Err(LearnClientError::SessionExpired);
        }
        parse_course_time_location_response(response.body()).map_err(Into::into)
    }

    /// Executes the role-specific semester list request with the shared Cookie
    /// jar.  A JSON array is the observed success envelope; a login page or a
    /// JSON object pretending to be a list is rejected by the parser.
    pub async fn fetch_semester_ids(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<Vec<String>, LearnClientError> {
        let (semester_ids, _) = self.fetch_semester_ids_with_csrf(transport).await?;
        Ok(semester_ids)
    }

    /// Same semester-list request as [`Self::fetch_semester_ids`], retaining
    /// the page CSRF token returned by the course home immediately before the
    /// AJAX request.  The runtime uses this token as the newest Learn proof
    /// for all subsequent course and Registrar requests.
    pub(crate) async fn fetch_semester_ids_with_csrf(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<(Vec<String>, CsrfToken), LearnClientError> {
        let csrf = self.fetch_course_home_csrf(transport).await?;
        let body = self
            .fetch_semester_json(transport, &self.config.semester_list_path, &csrf.token)
            .await?;
        let semester_ids =
            parse_semester_ids(&body).map_err(|error| map_semester_parse_error(error, &body))?;
        Ok((semester_ids, csrf.token))
    }

    /// Executes the current/next semester request and verifies its success
    /// marker and date/academic-year fields before returning it.
    pub async fn fetch_current_semester(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<LearnCurrentSemester, LearnClientError> {
        let (current, _) = self.fetch_current_semester_with_csrf(transport).await?;
        Ok(current)
    }

    /// Same current-semester request as [`Self::fetch_current_semester`],
    /// retaining the page CSRF token used for the request.
    pub(crate) async fn fetch_current_semester_with_csrf(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<(LearnCurrentSemester, CsrfToken), LearnClientError> {
        let csrf = self.fetch_course_home_csrf(transport).await?;
        let body = self
            .fetch_semester_json(transport, &self.config.current_semester_path, &csrf.token)
            .await?;
        let current = parse_current_semester(&body)
            .map_err(|error| map_semester_parse_error(error, &body))?;
        Ok((current, csrf.token))
    }

    /// Reads the reference calendar's current and following terms using the
    /// CSRF proof already bound to this Learn session. This is one fixed GET
    /// through the caller's shared transport; it does not start a new roam or
    /// refresh the Cookie jar independently of the owning Runtime.
    pub(crate) async fn fetch_term_calendar_with_bound_csrf(
        &self,
        transport: &CampusHttpTransport,
        csrf: &BoundCsrfToken,
    ) -> Result<LearnTermCalendar, LearnClientError> {
        if csrf.service() != ServiceId::Learn {
            return Err(LearnClientError::ForeignCsrf);
        }
        let body = self
            .fetch_semester_json(
                transport,
                &self.config.current_semester_path,
                csrf.as_csrf_token(),
            )
            .await?;
        parse_term_calendar(&body).map_err(|error| map_semester_parse_error(error, &body))
    }

    /// The semester endpoints are AJAX routes, and current public Learn
    /// clients append the page CSRF token to every such request.  Keep this
    /// proof boundary in the client so callers cannot accidentally send an
    /// unauthenticated bare GET after a successful SSO handoff.
    async fn fetch_course_home_csrf(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<CsrfEvidence, LearnClientError> {
        let response = self.execute_course_home(transport).await?;
        let mapped = self.map_http_response(&response)?;
        match &mapped.classification {
            LearnPageClassification::LoginExpired(_) => Err(LearnClientError::SessionExpired),
            LearnPageClassification::Authenticated => {
                if response.status != StatusCode::OK {
                    return Err(LearnClientError::Transport(TransportError::HttpStatus {
                        status: response.status,
                        body: response.body().to_owned(),
                    }));
                }
                mapped.csrf.ok_or_else(|| {
                    LearnClientError::Transport(TransportError::DecodeBody {
                        message: "learning course home did not provide CSRF evidence".to_owned(),
                    })
                })
            }
            LearnPageClassification::Unknown => {
                if response.status != StatusCode::OK {
                    Err(LearnClientError::Transport(TransportError::HttpStatus {
                        status: response.status,
                        body: response.body().to_owned(),
                    }))
                } else {
                    Err(LearnClientError::Transport(TransportError::DecodeBody {
                        message: "learning course home did not prove an authenticated session"
                            .to_owned(),
                    }))
                }
            }
            LearnPageClassification::NonSuccess => {
                Err(LearnClientError::Transport(TransportError::HttpStatus {
                    status: response.status,
                    body: response.body().to_owned(),
                }))
            }
        }
    }

    async fn fetch_semester_json(
        &self,
        transport: &CampusHttpTransport,
        path: &str,
        csrf: &CsrfToken,
    ) -> Result<String, LearnClientError> {
        let endpoint = self.resolve_path(path)?;
        let request = transport
            .client()
            .get(endpoint.clone())
            .query(&[(self.config.csrf_field_name.as_str(), csrf.as_str())])
            .build()
            .map_err(|error| LearnClientError::Transport(TransportError::Request(error)))?;
        let response = self.execute_page_request(transport, request).await?;
        if self.response_has_login_expiry(&response) {
            return Err(LearnClientError::SessionExpired);
        }
        self.validate_response_origin(&response)?;
        if response.final_url.path() != endpoint.path() {
            return Err(LearnClientError::UnexpectedPath);
        }
        if !query_matches_exactly(
            &response.final_url,
            &[(self.config.csrf_field_name.as_str(), csrf.as_str())],
        ) {
            return Err(LearnClientError::UnexpectedQuery);
        }
        if response.status != StatusCode::OK {
            return Err(LearnClientError::Transport(TransportError::HttpStatus {
                status: response.status,
                body: response.body,
            }));
        }
        Ok(response.body)
    }

    /// Maps a page response and gives HTTP 200 login pages an explicit state.
    /// A page without a login signal and without CSRF evidence remains
    /// `Unknown`; absence of a marker is never treated as authentication.
    pub fn map_html_response(
        &self,
        status: StatusCode,
        final_url: Option<&Url>,
        body: &str,
    ) -> Result<LearnHtmlResponse, LearnClientError> {
        self.map_html_response_with_redirect(status, final_url, None, body)
    }

    /// Maps a page response while retaining a redirect which the shared
    /// transport deliberately stopped before following.  In particular, a
    /// Learn 302 to the external identity login must be classified as an
    /// expired session rather than as a generic non-success response.
    pub fn map_html_response_with_redirect(
        &self,
        status: StatusCode,
        final_url: Option<&Url>,
        redirect_location: Option<&str>,
        body: &str,
    ) -> Result<LearnHtmlResponse, LearnClientError> {
        let redirect_target = redirect_location
            .map(|location| {
                let final_url = final_url.ok_or(LearnClientError::UnexpectedOrigin)?;
                Url::parse(location)
                    .or_else(|_| final_url.join(location))
                    .map_err(|_| LearnClientError::UnexpectedOrigin)
            })
            .transpose()?;
        self.map_html_response_with_redirect_url(status, final_url, redirect_target.as_ref(), body)
    }

    /// Maps the complete response retained by a roaming or page request.
    pub fn map_http_response(
        &self,
        response: &LearnHttpResponse,
    ) -> Result<LearnHtmlResponse, LearnClientError> {
        self.map_html_response_with_redirect_url(
            response.status,
            Some(&response.final_url),
            response.redirect_location.as_ref(),
            response.body(),
        )
    }

    fn map_html_response_with_redirect_url(
        &self,
        status: StatusCode,
        final_url: Option<&Url>,
        redirect_target: Option<&Url>,
        body: &str,
    ) -> Result<LearnHtmlResponse, LearnClientError> {
        let final_url = final_url.cloned();
        // A redirect to the identity login page is an explicit expired-session
        // signal even though that page is outside the Learn origin. Classify
        // that failure before rejecting unrelated cross-origin content; the
        // page can never establish a Learn session because it has no accepted
        // origin or CSRF proof.
        let status_expired = matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            && final_url
                .as_ref()
                .is_some_and(|url| same_origin(&self.config.base_url, url));
        if status_expired {
            return Ok(LearnHtmlResponse {
                status,
                final_url,
                csrf: None,
                classification: LearnPageClassification::LoginExpired(LoginExpiredEvidence {
                    signals: vec![LoginExpiredSignal::HttpStatus(status)],
                }),
            });
        }

        let login_evidence = self.login_expired_evidence(final_url.as_ref(), redirect_target, body);
        if let Some(evidence) = login_evidence {
            return Ok(LearnHtmlResponse {
                status,
                final_url,
                csrf: None,
                classification: LearnPageClassification::LoginExpired(evidence),
            });
        }
        if redirect_target.is_some_and(|url| !endpoint_is_allowed(&self.config.base_url, url)) {
            return Err(LearnClientError::UnexpectedOrigin);
        }
        if final_url
            .as_ref()
            .is_some_and(|url| !endpoint_is_allowed(&self.config.base_url, url))
        {
            return Err(LearnClientError::UnexpectedOrigin);
        }
        if status != StatusCode::OK {
            return Ok(LearnHtmlResponse {
                status,
                final_url,
                csrf: None,
                classification: LearnPageClassification::NonSuccess,
            });
        }

        match self.parse_csrf(body) {
            Ok(csrf) => Ok(LearnHtmlResponse {
                status,
                final_url,
                csrf: Some(csrf),
                classification: LearnPageClassification::Authenticated,
            }),
            Err(CsrfParseError::Missing) => Ok(LearnHtmlResponse {
                status,
                final_url,
                csrf: None,
                classification: LearnPageClassification::Unknown,
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Checks the explicit login-expiry signals without treating an ordinary
    /// empty or unknown page as an authenticated response.
    pub fn is_login_expired_page(
        &self,
        status: StatusCode,
        final_url: Option<&Url>,
        body: &str,
    ) -> bool {
        (matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            && final_url.is_some_and(|url| same_origin(&self.config.base_url, url)))
            || (status == StatusCode::OK
                && self.login_expired_evidence(final_url, None, body).is_some())
    }

    fn login_expired_evidence(
        &self,
        final_url: Option<&Url>,
        redirect_target: Option<&Url>,
        body: &str,
    ) -> Option<LoginExpiredEvidence> {
        detect_login_expired(
            final_url,
            redirect_target,
            body,
            &self.config.login_path_hints,
            &self.config.expired_markers,
        )
    }

    fn response_has_login_expiry(&self, response: &LearnHttpResponse) -> bool {
        (matches!(
            response.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) && same_origin(&self.config.base_url, &response.final_url))
            || self
                .login_expired_evidence(
                    Some(&response.final_url),
                    response.redirect_location.as_ref(),
                    response.body(),
                )
                .is_some()
    }

    fn validate_response_origin(
        &self,
        response: &LearnHttpResponse,
    ) -> Result<(), LearnClientError> {
        if !endpoint_is_allowed(&self.config.base_url, &response.final_url)
            || response
                .redirect_location
                .as_ref()
                .is_some_and(|url| !endpoint_is_allowed(&self.config.base_url, url))
        {
            return Err(LearnClientError::UnexpectedOrigin);
        }
        Ok(())
    }

    fn resolve_path(&self, path: &str) -> Result<Url, LearnClientError> {
        // Public configuration fields can be changed after construction, so
        // validate at the final URL boundary as well as in the fluent
        // setters. This prevents Url::join from normalizing traversal or
        // accepting encoded separators supplied by a caller.
        validate_route_path(path)?;
        resolve_endpoint(&self.config.base_url, path)
            .map_err(|error| LearnClientError::InvalidBaseUrl(error.to_string()))
    }

    /// Resolves a role-specific Learn route for sibling adapters such as
    /// announcements and todos. Keeping this in LearnClient prevents those
    /// adapters from accidentally dropping a WebVPN mapping prefix.
    pub(crate) fn resolve_endpoint(&self, path: &str) -> Result<Url, LearnClientError> {
        self.resolve_path(path)
    }

    fn validate_roaming_final_path(
        &self,
        response: &LearnHttpResponse,
    ) -> Result<(), LearnClientError> {
        // Leave cross-origin responses for the normal page classifier. This
        // preserves the explicit identity-login expiry signal while still
        // rejecting an unrelated external page there.
        if !same_origin(&self.config.base_url, &response.final_url) {
            return Ok(());
        }
        let path = response.final_url.path();
        let allowed = [
            self.resolve_path(&self.config.roaming_entry_path)?
                .path()
                .to_owned(),
            self.resolve_path(&self.config.cookie_roaming_entry_path)?
                .path()
                .to_owned(),
            self.resolve_path(&self.config.course_home_path)?
                .path()
                .to_owned(),
        ];
        if allowed.iter().any(|allowed_path| allowed_path == path)
            || self.response_has_login_expiry(response)
        {
            Ok(())
        } else {
            Err(LearnClientError::UnexpectedPath)
        }
    }
}

fn endpoint_path(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _)| path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnRequestMethod {
    Get,
}

/// Query portion of the roaming entry request.  The ticket remains a
/// non-serializable protocol type; this request-only wrapper provides the
/// explicit wire serialization needed by reqwest without making the token
/// serializable in isolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnRoamingQuery {
    ticket: ServiceTicket,
}

impl LearnRoamingQuery {
    fn new(ticket: ServiceTicket) -> Self {
        Self { ticket }
    }

    pub fn as_str(&self) -> &str {
        self.ticket.as_str()
    }
}

impl Serialize for LearnRoamingQuery {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("LearnRoamingQuery", 1)?;
        state.serialize_field("ticket", self.ticket.as_str())?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnRoamingRequestPlan {
    pub method: LearnRequestMethod,
    pub endpoint: Url,
    pub path: String,
    pub query: LearnRoamingQuery,
}

impl LearnRoamingRequestPlan {
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, LearnClientError> {
        transport
            .client()
            .get(self.endpoint.clone())
            .query(&self.query)
            .build()
            .map_err(|error| TransportError::Request(error).into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseHomeRequestPlan {
    pub method: LearnRequestMethod,
    pub endpoint: Url,
    pub path: String,
}

impl LearnCourseHomeRequestPlan {
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, LearnClientError> {
        transport
            .client()
            .get(self.endpoint.clone())
            .build()
            .map_err(|error| TransportError::Request(error).into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseListRequestPlan {
    pub method: LearnRequestMethod,
    pub endpoint: Url,
    pub role: CourseRole,
    pub request: LearnCourseRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseTimeLocationRequestPlan {
    pub method: LearnRequestMethod,
    pub endpoint: Url,
    pub path: String,
    pub course_id: String,
}

impl LearnCourseTimeLocationRequestPlan {
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, LearnClientError> {
        transport
            .client()
            .get(self.endpoint.clone())
            .build()
            .map_err(|error| TransportError::Request(error).into())
    }
}

impl LearnCourseListRequestPlan {
    pub fn requires_csrf(&self) -> bool {
        self.request.requires_csrf
    }

    /// Builds the role-specific GET request.  CSRF transport headers are left
    /// to the deployment profile because the HTML can advertise a header name
    /// separately from the `_csrf` token field.
    pub fn build_request(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<reqwest::Request, LearnClientError> {
        transport
            .client()
            .get(self.endpoint.clone())
            .build()
            .map_err(|error| TransportError::Request(error).into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrfSource {
    Meta,
    Input,
    Link,
    Query,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CsrfEvidence {
    pub token: CsrfToken,
    pub sources: Vec<CsrfSource>,
}

impl std::fmt::Debug for CsrfEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CsrfEvidence")
            .field("token", &self.token)
            .field("sources", &self.sources)
            .finish()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CsrfParseError {
    #[error("no CSRF page evidence was found")]
    Missing,

    #[error("malformed {element} tag while reading CSRF evidence: {message}")]
    MalformedTag {
        element: &'static str,
        message: String,
    },

    #[error("CSRF {element} is missing its {attribute} attribute")]
    MissingAttribute {
        element: &'static str,
        attribute: &'static str,
    },

    #[error("CSRF {element} has an empty {attribute} attribute")]
    EmptyAttribute {
        element: &'static str,
        attribute: &'static str,
    },

    #[error("CSRF input must be hidden when its type attribute is present")]
    InvalidInputType,

    #[error("CSRF token contains whitespace or a control character")]
    InvalidToken,

    #[error("CSRF page evidence contains different token values")]
    ConflictingTokens,

    #[error("CSRF field name must not be empty or contain whitespace")]
    InvalidFieldName,
}

/// Extracts the conventional `_csrf` token from recognized page evidence.
/// Multiple sources may be present, but they must agree.
pub fn extract_csrf(html: &str) -> Result<CsrfEvidence, CsrfParseError> {
    extract_csrf_with_field(html, DEFAULT_CSRF_FIELD_NAME)
}

pub fn extract_csrf_with_field(
    html: &str,
    field_name: &str,
) -> Result<CsrfEvidence, CsrfParseError> {
    validate_field_name(field_name).map_err(|_| CsrfParseError::InvalidFieldName)?;

    let tags = scan_evidence_tags(html)?;
    let mut token: Option<String> = None;
    let mut sources = Vec::new();

    for tag in tags {
        let candidate = match tag.name.as_str() {
            "meta" => csrf_meta_candidate(&tag, field_name)?,
            "input" => csrf_input_candidate(&tag, field_name)?,
            "a" => csrf_link_candidate(&tag, field_name, "href")?,
            "form" => csrf_link_candidate(&tag, field_name, "action")?,
            _ => None,
        };

        let Some((candidate_token, source)) = candidate else {
            continue;
        };

        if let Some(existing) = &token {
            if existing != &candidate_token {
                return Err(CsrfParseError::ConflictingTokens);
            }
        } else {
            token = Some(candidate_token);
        }
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    // The current Learn page embeds generated course links in a script
    // template. Those links carry the same `_csrf=` query parameter as the
    // visible anchors, but there may be no parsed `<a>` node in the returned
    // HTML. Scan only the parameter shape, ignore comments, and accept only
    // URL-safe token characters; the page still has to come from the trusted
    // Learn origin before this evidence is used.
    for candidate_token in query_csrf_candidates(html, field_name)? {
        if let Some(existing) = &token {
            if existing != &candidate_token {
                return Err(CsrfParseError::ConflictingTokens);
            }
        } else {
            token = Some(candidate_token);
        }
        if !sources.contains(&CsrfSource::Query) {
            sources.push(CsrfSource::Query);
        }
    }

    let token = token.ok_or(CsrfParseError::Missing)?;
    let token = CsrfToken::new(token).ok_or(CsrfParseError::InvalidToken)?;
    Ok(CsrfEvidence { token, sources })
}

fn validate_field_name(field_name: &str) -> Result<(), LearnClientError> {
    if field_name.trim().is_empty() || field_name.chars().any(char::is_whitespace) {
        return Err(LearnClientError::InvalidCsrfFieldName(
            field_name.to_owned(),
        ));
    }
    Ok(())
}

fn normalize_base_url(base_url: &str) -> Result<Url, LearnClientError> {
    let url = Url::parse(base_url)
        .map_err(|error| LearnClientError::InvalidBaseUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(LearnClientError::InvalidBaseUrl(
            "learning base URL must have an http(s) scheme and host".to_owned(),
        ));
    }
    if url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(LearnClientError::InvalidBaseUrl(
            "learning base URL must not contain userinfo, a query, or a fragment".to_owned(),
        ));
    }
    Ok(url)
}

fn query_matches_exactly(url: &Url, expected: &[(&str, &str)]) -> bool {
    let actual: Vec<_> = url.query_pairs().collect();
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|((name, value), (expected_name, expected_value))| {
                name == *expected_name && value == *expected_value
            })
}

fn diagnostic_url(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

fn csrf_meta_candidate(
    tag: &HtmlTag,
    field_name: &str,
) -> Result<Option<(String, CsrfSource)>, CsrfParseError> {
    let Some(name) = tag.attribute_value("name") else {
        return Ok(None);
    };
    if name != field_name {
        return Ok(None);
    }

    let content = tag
        .attribute_value("content")
        .ok_or(CsrfParseError::MissingAttribute {
            element: "meta",
            attribute: "content",
        })?;
    let content = validate_token_attribute(content, "meta", "content")?;
    Ok(Some((content, CsrfSource::Meta)))
}

fn csrf_input_candidate(
    tag: &HtmlTag,
    field_name: &str,
) -> Result<Option<(String, CsrfSource)>, CsrfParseError> {
    let Some(name) = tag.attribute_value("name") else {
        return Ok(None);
    };
    if name != field_name {
        return Ok(None);
    }

    if let Some(input_type) = tag.attribute_value("type") {
        if !input_type.eq_ignore_ascii_case("hidden") {
            return Err(CsrfParseError::InvalidInputType);
        }
    }
    let value = tag
        .attribute_value("value")
        .ok_or(CsrfParseError::MissingAttribute {
            element: "input",
            attribute: "value",
        })?;
    let value = validate_token_attribute(value, "input", "value")?;
    Ok(Some((value, CsrfSource::Input)))
}

fn csrf_link_candidate(
    tag: &HtmlTag,
    field_name: &str,
    attribute: &'static str,
) -> Result<Option<(String, CsrfSource)>, CsrfParseError> {
    let Some(raw_url) = tag.attribute_value(attribute) else {
        return Ok(None);
    };
    let raw_url = raw_url
        .replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&");
    let base = Url::parse("https://learn.invalid/").expect("static URL is valid");
    let Ok(url) = base.join(&raw_url) else {
        return Ok(None);
    };
    let Some(value) = url
        .query_pairs()
        .find_map(|(name, value)| (name == field_name).then_some(value.into_owned()))
    else {
        return Ok(None);
    };
    let value = validate_token_attribute(value.as_str(), "a", attribute)?;
    Ok(Some((value, CsrfSource::Link)))
}

fn query_csrf_candidates(html: &str, field_name: &str) -> Result<Vec<String>, CsrfParseError> {
    let html = without_html_comments(html)?;
    let field_marker = format!("{field_name}=");
    let mut cursor = 0;
    let mut candidates = Vec::new();
    while cursor < html.len() {
        let Some(relative_match) = find_ascii_case_insensitive(&html[cursor..], &field_marker)
        else {
            break;
        };
        let marker_start = cursor + relative_match;
        let boundary = html[..marker_start].chars().next_back();
        if boundary.is_some_and(|character| {
            !matches!(character, '?' | '&' | ';' | '\'' | '"' | '(' | '=' | ':')
        }) {
            cursor = marker_start + field_marker.len();
            continue;
        }

        let value_start = marker_start + field_marker.len();
        let value_end = html[value_start..]
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, '&' | ';' | '\'' | '"' | '<' | '>' | ')' | ']')
            })
            .map_or(html.len(), |relative_end| value_start + relative_end);
        let value = &html[value_start..value_end];
        if value.is_empty() {
            return Err(CsrfParseError::EmptyAttribute {
                element: "query",
                attribute: "value",
            });
        }
        let value = decode_query_component(value).ok_or(CsrfParseError::InvalidToken)?;
        let value = validate_token_attribute(&value, "query", "value")?;
        candidates.push(value);
        cursor = value_end;
    }
    Ok(candidates)
}

fn without_html_comments(html: &str) -> Result<String, CsrfParseError> {
    let mut output = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(relative_start) = html[cursor..].find("<!--") {
        let start = cursor + relative_start;
        output.push_str(&html[cursor..start]);
        let Some(relative_end) = html[start + 4..].find("-->") else {
            return Err(CsrfParseError::MalformedTag {
                element: "html",
                message: "unterminated comment".to_owned(),
            });
        };
        cursor = start + 4 + relative_end + 3;
    }
    output.push_str(&html[cursor..]);
    Ok(output)
}

fn decode_query_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'+' => {
                decoded.push(b' ');
                cursor += 1;
            }
            b'%' => {
                let high = hex_value(*bytes.get(cursor + 1)?)?;
                let low = hex_value(*bytes.get(cursor + 2)?)?;
                decoded.push((high << 4) | low);
                cursor += 3;
            }
            byte => {
                decoded.push(byte);
                cursor += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn validate_token_attribute(
    value: &str,
    element: &'static str,
    attribute: &'static str,
) -> Result<String, CsrfParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CsrfParseError::EmptyAttribute { element, attribute });
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(CsrfParseError::InvalidToken);
    }
    Ok(value.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginExpiredSignal {
    /// The Learn origin rejected the request at the HTTP layer.  A 401/403
    /// from another origin is handled as an origin failure instead.
    HttpStatus(StatusCode),
    FinalUrlMatchesLogin,
    IdentityLoginForm,
    ExplicitExpiredMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginExpiredEvidence {
    pub signals: Vec<LoginExpiredSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearnPageClassification {
    LoginExpired(LoginExpiredEvidence),
    Authenticated,
    Unknown,
    NonSuccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnHtmlResponse {
    pub status: StatusCode,
    pub final_url: Option<Url>,
    pub csrf: Option<CsrfEvidence>,
    pub classification: LearnPageClassification,
}

impl LearnHtmlResponse {
    pub fn is_login_expired(&self) -> bool {
        matches!(
            self.classification,
            LearnPageClassification::LoginExpired(_)
        )
    }
}

/// Raw text retained by the roaming execution boundary.
///
/// The body is available to the Rust classifier but is intentionally omitted
/// from debug output because roaming pages can contain CSRF and session data.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnHttpResponse {
    pub status: StatusCode,
    pub final_url: Url,
    redirect_location: Option<Url>,
    body: String,
}

impl LearnHttpResponse {
    pub fn body(&self) -> &str {
        &self.body
    }
}

impl fmt::Debug for LearnHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnHttpResponse")
            .field("status", &self.status)
            .field("final_url", &diagnostic_url(&self.final_url))
            .field("body_len", &self.body.len())
            .finish()
    }
}

fn detect_login_expired(
    final_url: Option<&Url>,
    redirect_target: Option<&Url>,
    body: &str,
    login_path_hints: &[String],
    expired_markers: &[String],
) -> Option<LoginExpiredEvidence> {
    let mut signals = Vec::new();

    if final_url.is_some_and(|url| url_matches_login(url, login_path_hints))
        || redirect_target.is_some_and(|url| url_matches_login(url, login_path_hints))
    {
        signals.push(LoginExpiredSignal::FinalUrlMatchesLogin);
    }

    let lower_body = body.to_ascii_lowercase();
    if expired_markers
        .iter()
        .any(|marker| !marker.is_empty() && lower_body.contains(&marker.to_ascii_lowercase()))
        || body_indicates_login_failure(body)
    {
        signals.push(LoginExpiredSignal::ExplicitExpiredMarker);
    }

    if body_contains_identity_login_form(body) {
        signals.push(LoginExpiredSignal::IdentityLoginForm);
    }

    (!signals.is_empty()).then_some(LoginExpiredEvidence { signals })
}

fn url_matches_login(url: &Url, login_path_hints: &[String]) -> bool {
    let path = url.path().to_ascii_lowercase();
    let query = url.query().unwrap_or_default().to_ascii_lowercase();
    let path_and_query = format!("{path}?{query}");
    login_path_hints.iter().any(|hint| {
        let hint = hint.to_ascii_lowercase();
        !hint.is_empty() && path_and_query.contains(&hint)
    })
}

fn body_contains_identity_login_form(body: &str) -> bool {
    let Ok(tags) = scan_evidence_tags(body) else {
        return false;
    };

    let has_login_form = tags.iter().any(|tag| {
        tag.name == "form" && tag.attribute_value("action").is_some_and(is_login_action)
    });
    if has_login_form {
        return true;
    }

    let has_user_field = tags.iter().any(|tag| {
        tag.name == "input" && tag.attribute_value("name").is_some_and(is_login_user_field)
    });
    let has_password_field = tags.iter().any(|tag| {
        tag.name == "input"
            && tag
                .attribute_value("name")
                .is_some_and(is_login_password_field)
    });
    has_user_field && has_password_field
}

fn is_login_user_field(name: &str) -> bool {
    [
        "i_user",
        "username",
        "user_name",
        "loginname",
        "login_name",
        "account",
        "user",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn is_login_password_field(name: &str) -> bool {
    ["i_pass", "password", "passwd", "pass", "userpassword"]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn is_login_action(action: &str) -> bool {
    let action = action.to_ascii_lowercase();
    action.contains("/do/off/ui/auth/login")
        || action.contains("/auth/login")
        || action.contains("login/check")
        || action.contains("/signin")
}

/// Returns true only for wording which explicitly says that authentication
/// is missing or expired.  A generic `error` or `failed` message is kept as a
/// business failure by the response parsers; it does not prove that the
/// shared Learn session has expired.
pub(crate) fn is_login_failure_message(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || [
            "login success",
            "authenticated",
            "登录成功",
            "认证成功",
            "会话有效",
        ]
        .iter()
        .any(|marker| value == *marker)
        || value == "logged in"
    {
        return false;
    }
    [
        "not logged in",
        "unauthenticated",
        "login required",
        "authentication required",
        "please login",
        "please log in",
        "need login",
        "login_timeout",
        "login timeout",
        "login failed",
        "authentication failed",
        "session expired",
        "session invalid",
        "unauthorized",
        "未登录",
        "请登录",
        "请先登录",
        "请重新登录",
        "需要登录",
        "登录失效",
        "登录超时",
        "登录失败",
        "未认证",
        "认证过期",
        "认证失效",
        "认证失败",
        "统一认证失败",
        "会话过期",
        "会话已过期",
        "会话失效",
        "会话已失效",
        "会话超时",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

/// Inspects only explicit status/message fields in a JSON response. This is
/// used by JSON endpoints which return HTTP 200 for a login page and keeps a
/// login envelope from being mistaken for an empty collection.
pub(crate) fn body_indicates_login_failure(body: &str) -> bool {
    let json_body = body.trim_start_matches('\u{feff}');
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(json_body) else {
        return is_login_failure_message(body);
    };

    for field in [
        "message",
        "msg",
        "error",
        "errorMessage",
        "errorMsg",
        "reason",
        "statusText",
    ] {
        if let Some(value) = object.get(field).and_then(Value::as_str)
            && is_login_failure_message(value)
        {
            return true;
        }
    }

    for field in ["code", "status", "resultCode"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        let is_auth_status = match value {
            Value::Number(value) => value.as_i64() == Some(401) || value.as_i64() == Some(403),
            Value::String(value) => {
                let value = value.trim();
                value == "401" || value == "403" || is_login_failure_message(value)
            }
            _ => false,
        };
        if is_auth_status {
            return true;
        }
    }

    false
}

/// Stable fields consumed by THYou.  The source service has used several
/// spellings over time; aliases are handled by the mapper, while fields not
/// in this subset remain in `unknown_fields` verbatim as JSON values.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearnCourseRecord {
    pub course_id: Option<String>,
    pub course_code: Option<String>,
    pub name: Option<String>,
    pub instructor: Option<String>,
    pub class_name: Option<String>,
    pub semester: Option<String>,
    pub unknown_fields: BTreeMap<String, Value>,
}

impl LearnCourseRecord {
    pub fn id(&self) -> Option<&str> {
        self.course_id.as_deref()
    }

    pub fn code(&self) -> Option<&str> {
        self.course_code.as_deref()
    }

    pub fn title(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The public Learn response calls the primary display name `zywkcm` and
    /// the Chinese and English variants `kcm` and `ywkcm`.  These accessors
    /// read the latter two fields from the preserved wire-field map without
    /// changing the existing DTO shape used by the runtime.
    pub fn chinese_name(&self) -> Option<String> {
        extra_text_scalar(
            &self.unknown_fields,
            &["kcm", "chineseName", "chinese_name"],
        )
    }

    pub fn english_name(&self) -> Option<String> {
        extra_text_scalar(
            &self.unknown_fields,
            &["ywkcm", "englishName", "english_name"],
        )
    }

    pub fn teacher_number(&self) -> Option<String> {
        extra_scalar(
            &self.unknown_fields,
            &["jsh", "teacherNumber", "teacher_number"],
        )
    }

    pub fn course_index(&self) -> Option<String> {
        extra_scalar(
            &self.unknown_fields,
            &["kxh", "courseIndex", "course_index"],
        )
    }

    pub fn extra_fields(&self) -> &BTreeMap<String, Value> {
        &self.unknown_fields
    }
}

#[derive(Debug, Error)]
pub enum CourseJsonError {
    #[error("course response JSON could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("course response root must be an object or array")]
    InvalidRoot,

    #[error("course collection field {field:?} must contain an array or object")]
    InvalidCollection { field: String },

    #[error("course record must be a JSON object")]
    InvalidRecord { index: Option<usize> },

    #[error("course record at index {index:?} is missing {field}")]
    IncompleteRecord {
        index: Option<usize>,
        field: &'static str,
    },

    #[error("course response contains a business failure: {message}")]
    BusinessFailure { message: String },

    #[error("course response is missing the confirmed success marker")]
    MissingSuccessMarker,

    #[error("course response does not contain a verified collection")]
    MissingCollection,

    #[error("course response contains an empty collection without a success marker")]
    UnverifiedEmptyCollection,

    #[error("course record at index {index:?} has an invalid {field}")]
    InvalidField {
        index: Option<usize>,
        field: &'static str,
    },

    #[error("course record at index {index:?} belongs to semester {actual}, expected {expected}")]
    SemesterMismatch {
        index: Option<usize>,
        expected: String,
        actual: String,
    },
}

fn map_course_json_error(error: CourseJsonError) -> LearnClientError {
    if let CourseJsonError::BusinessFailure { message } = &error
        && is_login_failure_message(message)
    {
        return LearnClientError::SessionExpired;
    }
    LearnClientError::CourseJson(error)
}

/// Errors for the two real Learn semester endpoints.
#[derive(Debug, Error)]
pub enum SemesterParseError {
    #[error("semester response JSON could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("semester list response must be an array")]
    InvalidListRoot,

    #[error("semester entry at index {index} must be a canonical YYYY-YYYY-1/2/3 string")]
    InvalidEntry { index: usize },

    #[error("current-semester response must be an object")]
    InvalidCurrentRoot,

    #[error("current-semester response did not report success: {message}")]
    CurrentFailure { message: String },

    #[error("current-semester response is missing {field}")]
    MissingCurrentField { field: &'static str },

    #[error("current-semester response has an invalid {field}: {value}")]
    InvalidCurrentField { field: &'static str, value: String },

    #[error("current-semester id and xnxq label do not match")]
    CurrentIdLabelMismatch,

    #[error("term calendar resultList must be an array")]
    InvalidCalendarList,

    #[error("term calendar has too many following terms")]
    CalendarTooLarge,

    #[error("term calendar row {index} has an invalid {field}")]
    InvalidCalendarEntry { index: usize, field: &'static str },

    #[error("term calendar repeats a semester id")]
    DuplicateCalendarTerm,
}

fn map_semester_parse_error(error: SemesterParseError, body: &str) -> LearnClientError {
    if body_indicates_login_failure(body)
        || matches!(
            &error,
            SemesterParseError::CurrentFailure { message }
                if is_login_failure_message(message)
        )
    {
        LearnClientError::SessionExpired
    } else {
        LearnClientError::Semester(error)
    }
}

/// A verified current/next semester record returned by Learn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LearnCurrentSemester {
    pub id: String,
    pub label: String,
    pub start_date: String,
    pub end_date: String,
}

/// The current term and bounded following terms returned by the same Learn
/// route used for current-semester discovery. Dates are server evidence;
/// teaching-week alignment is computed by the Runtime after date validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnTermCalendar {
    pub current: LearnCurrentSemester,
    pub upcoming: Vec<LearnCurrentSemester>,
}

pub fn parse_term_calendar(body: &str) -> Result<LearnTermCalendar, SemesterParseError> {
    if body.len() > 128 * 1024 {
        return Err(SemesterParseError::CalendarTooLarge);
    }
    let mut current = parse_current_semester(body)?;
    let root: Value = serde_json::from_str(body)?;
    let object = root
        .as_object()
        .ok_or(SemesterParseError::InvalidCurrentRoot)?;
    if let Some(result) = object.get("result").and_then(Value::as_object) {
        current.label = calendar_display_label(result, &current.id, 0)?;
    }
    let rows = object
        .get("resultList")
        .and_then(Value::as_array)
        .ok_or(SemesterParseError::InvalidCalendarList)?;
    if rows.len() > 8 {
        return Err(SemesterParseError::CalendarTooLarge);
    }
    let mut seen = HashSet::from([current.id.clone()]);
    let mut upcoming = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let row = row
            .as_object()
            .ok_or(SemesterParseError::InvalidCalendarEntry {
                index,
                field: "row",
            })?;
        let required = |field| {
            row.get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or(SemesterParseError::InvalidCalendarEntry { index, field })
        };
        let id = required("id")?;
        let start_date = required("kssj")?;
        let end_date = required("jssj")?;
        if validate_semester_id(&id).is_err()
            || row
                .get("xnxq")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim() != id)
        {
            return Err(SemesterParseError::InvalidCalendarEntry { index, field: "id" });
        }
        if !valid_semester_timestamp(&start_date) || !valid_semester_timestamp(&end_date) {
            return Err(SemesterParseError::InvalidCalendarEntry {
                index,
                field: "date",
            });
        }
        if !seen.insert(id.clone()) {
            return Err(SemesterParseError::DuplicateCalendarTerm);
        }
        let label = calendar_display_label(row, &id, index)?;
        upcoming.push(LearnCurrentSemester {
            id,
            label,
            start_date,
            end_date,
        });
    }
    Ok(LearnTermCalendar { current, upcoming })
}

fn calendar_display_label(
    row: &Map<String, Value>,
    id: &str,
    index: usize,
) -> Result<String, SemesterParseError> {
    let Some(value) = row.get("xnxqmc") else {
        return Ok(id.to_owned());
    };
    if value.is_null() {
        return Ok(id.to_owned());
    }
    let label = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .filter(|value| !value.chars().any(char::is_control) && !value.contains(['<', '>']))
        .ok_or(SemesterParseError::InvalidCalendarEntry {
            index,
            field: "xnxqmc",
        })?;
    Ok(label.to_owned())
}

/// Parses the observed student/teacher semester list response.
pub fn parse_semester_ids(body: &str) -> Result<Vec<String>, SemesterParseError> {
    let value: Value = serde_json::from_str(body)?;
    let items = value
        .as_array()
        .ok_or(SemesterParseError::InvalidListRoot)?;
    let mut semesters = Vec::with_capacity(items.len());
    for (index, value) in items.iter().enumerate() {
        // The current public Learn client filters null entries from this
        // endpoint. A null is an explicitly empty row in a known array
        // envelope; every other malformed entry remains an error.
        if value.is_null() {
            continue;
        }
        match value {
            Value::String(value)
                if !value.trim().is_empty() && validate_semester_id(value.trim()).is_ok() =>
            {
                semesters.push(value.trim().to_owned());
            }
            _ => return Err(SemesterParseError::InvalidEntry { index }),
        }
    }
    Ok(semesters)
}

/// Parses `{ message: "success", result: { id, kssj, jssj, xnxq } }`.
pub fn parse_current_semester(body: &str) -> Result<LearnCurrentSemester, SemesterParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let value: Value = serde_json::from_str(body)?;
        let object = value
            .as_object()
            .ok_or(SemesterParseError::InvalidCurrentRoot)?;
        // The current public Learn endpoint uses the same `message: success`
        // envelope as the course client. Do not promote a generic numeric result
        // code into a semester proof because other legacy endpoints use that code
        // for unrelated operations.
        let success = object
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "success");
        if !success {
            return Err(SemesterParseError::CurrentFailure {
                message: object
                    .get("message")
                    .or_else(|| object.get("msg"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown learning response")
                    .to_owned(),
            });
        }
        let result = object
            .get("result")
            .and_then(Value::as_object)
            .ok_or(SemesterParseError::MissingCurrentField { field: "result" })?;
        let required = |field: &'static str| {
            result
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or(SemesterParseError::MissingCurrentField { field })
        };
        let id = required("id")?;
        let start_date = required("kssj")?;
        let end_date = required("jssj")?;
        let label = required("xnxq")?;
        if validate_semester_id(&id).is_err() {
            return Err(SemesterParseError::InvalidCurrentField {
                field: "id",
                value: id,
            });
        }
        if validate_semester_id(&label).is_err() {
            return Err(SemesterParseError::InvalidCurrentField {
                field: "xnxq",
                value: label,
            });
        }
        if id != label {
            return Err(SemesterParseError::CurrentIdLabelMismatch);
        }
        if !valid_semester_timestamp(&start_date) {
            return Err(SemesterParseError::InvalidCurrentField {
                field: "kssj",
                value: start_date,
            });
        }
        if !valid_semester_timestamp(&end_date) {
            return Err(SemesterParseError::InvalidCurrentField {
                field: "jssj",
                value: end_date,
            });
        }
        Ok(LearnCurrentSemester {
            id,
            label,
            start_date,
            end_date,
        })
    })
}

fn valid_semester_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y/%m/%d %H:%M:%S").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y/%m/%d %H:%M:%S%.f").is_ok()
        || chrono::NaiveDateTime::parse_from_str(value, "%Y/%m/%d %H:%M").is_ok()
        || chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
        || chrono::NaiveDate::parse_from_str(value, "%Y/%m/%d").is_ok()
}

const COURSE_COLLECTION_KEYS: &[&str] = &[
    "data",
    "aaData",
    "rows",
    "courses",
    "courseList",
    "list",
    "result",
    "resultList",
    "course",
];
const COURSE_ID_KEYS: &[&str] = &[
    "wlkcid",
    "wlkcId",
    "courseId",
    "courseID",
    "course_id",
    "id",
];
const COURSE_CODE_KEYS: &[&str] = &[
    "courseCode",
    "course_code",
    "code",
    "kcbh",
    "courseNumber",
    "kch",
];
const COURSE_NAME_KEYS: &[&str] = &[
    "zywkcm",
    "courseName",
    "course_name",
    "name",
    "title",
    "courseTitle",
    "kcmc",
    "kcm",
    "ywkcm",
];
const INSTRUCTOR_KEYS: &[&str] = &[
    "teacherName",
    "teacher_name",
    "teacher",
    "instructor",
    "instructorName",
    "jsxm",
    "jsm",
];
const CLASS_NAME_KEYS: &[&str] = &["className", "class_name", "class", "teachingClass", "bjmc"];
const SEMESTER_KEYS: &[&str] = &["semester", "semesterId", "semester_id", "term", "xq"];

/// Parses either a single course object, an array, or a common response
/// wrapper such as `{ "data": [...] }`.  Non-object array entries are safely
/// ignored because they cannot become a stable course record.
pub fn parse_course_records(body: &str) -> Result<Vec<LearnCourseRecord>, CourseJsonError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let value: Value = serde_json::from_str(body)?;
        parse_course_value(&value)
    })
}

/// Parses the exact envelope used by the authenticated course-list routes.
///
/// The generic [`parse_course_records`] function remains available for
/// compatibility with callers that map already isolated course JSON. An HTTP
/// course-list read must use this stricter boundary: the current public
/// implementation requires `message: "success"` and an array-valued
/// `resultList`, including when the server explicitly reports no courses.
fn parse_confirmed_course_list_response(
    body: &str,
) -> Result<Vec<LearnCourseRecord>, CourseJsonError> {
    parse_confirmed_course_list_response_for_semester(body, None)
}

fn parse_confirmed_course_list_response_for_semester(
    body: &str,
    expected_semester: Option<&str>,
) -> Result<Vec<LearnCourseRecord>, CourseJsonError> {
    let value: Value = serde_json::from_str(body)?;
    let object = value.as_object().ok_or(CourseJsonError::InvalidRoot)?;
    let message = object.get("message").and_then(Value::as_str);
    if !message.is_some_and(|value| value == "success") {
        if let Some(message) = message {
            return Err(CourseJsonError::BusinessFailure {
                message: message.to_owned(),
            });
        }
        return Err(CourseJsonError::MissingSuccessMarker);
    }

    // Keep the reference's required message/resultList contract, but do not
    // accept a proxy or deployment envelope carrying an explicit failure
    // alongside that message. Inspect envelope markers only, never row text.
    for field in ["success", "status", "resultCode"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        let failed = match value {
            Value::Bool(value) => !value,
            Value::Number(value) => value
                .as_i64()
                .is_some_and(|code| code < 0 || (400..=599).contains(&code)),
            Value::String(value) => {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "false" | "fail" | "failed" | "error"
                ) || value
                    .parse::<i64>()
                    .is_ok_and(|code| code < 0 || (400..=599).contains(&code))
            }
            _ => false,
        };
        if failed {
            return Err(CourseJsonError::BusinessFailure {
                message: "conflicting course response status".to_owned(),
            });
        }
    }

    let result_list = object
        .get("resultList")
        .ok_or(CourseJsonError::MissingCollection)?;
    if !result_list.is_array() {
        return Err(CourseJsonError::InvalidCollection {
            field: "resultList".to_owned(),
        });
    }
    require_remote_course_ids(result_list)?;
    let records = parse_course_value_with_empty(result_list, true)?;
    if let Some(expected_semester) = expected_semester {
        validate_course_semester_bindings(result_list, expected_semester)?;
    }
    Ok(records)
}

fn validate_course_semester_bindings(value: &Value, expected: &str) -> Result<(), CourseJsonError> {
    let items = value.as_array().ok_or(CourseJsonError::InvalidCollection {
        field: "resultList".to_owned(),
    })?;
    for (index, item) in items.iter().enumerate() {
        let object = item
            .as_object()
            .ok_or(CourseJsonError::InvalidRecord { index: Some(index) })?;
        for alias in SEMESTER_KEYS {
            let Some(value) = object
                .iter()
                .find_map(|(key, value)| key.eq_ignore_ascii_case(alias).then_some(value))
            else {
                continue;
            };
            let Some(actual) = first_scalar_value(value) else {
                return Err(CourseJsonError::InvalidField {
                    index: Some(index),
                    field: "semester",
                });
            };
            if actual != expected {
                return Err(CourseJsonError::SemesterMismatch {
                    index: Some(index),
                    expected: expected.to_owned(),
                    actual,
                });
            }
        }
    }
    Ok(())
}

/// The authenticated Learn course endpoints use `wlkcid` as the identifier
/// consumed by every course-scoped endpoint.  Generic mappers may accept
/// aliases for compatibility, but an HTTP response from these endpoints must
/// carry the real remote identifier.  Otherwise a record containing an
/// unrelated `id` or course number could be handed to announcements or
/// assignments as if it were a Learn course id.
fn require_remote_course_ids(value: &Value) -> Result<(), CourseJsonError> {
    let items = value.as_array().ok_or(CourseJsonError::InvalidCollection {
        field: "resultList".to_owned(),
    })?;
    for (index, item) in items.iter().enumerate() {
        let object = item
            .as_object()
            .ok_or(CourseJsonError::InvalidRecord { index: Some(index) })?;
        let Some(remote_id) = object
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case("wlkcid").then_some(value))
        else {
            return Err(CourseJsonError::IncompleteRecord {
                index: Some(index),
                field: "course id",
            });
        };
        if first_scalar_value(remote_id).is_none() {
            return Err(CourseJsonError::IncompleteRecord {
                index: Some(index),
                field: "course id",
            });
        }
    }
    Ok(())
}

pub fn parse_course_record(value: &Value) -> Result<LearnCourseRecord, CourseJsonError> {
    let object = value
        .as_object()
        .ok_or(CourseJsonError::InvalidRecord { index: None })?;
    validate_course_record(map_course_object(object), None)
}

fn parse_course_value(value: &Value) -> Result<Vec<LearnCourseRecord>, CourseJsonError> {
    parse_course_value_with_empty(value, false)
}

fn parse_course_value_with_empty(
    value: &Value,
    allow_verified_empty: bool,
) -> Result<Vec<LearnCourseRecord>, CourseJsonError> {
    match value {
        Value::Array(items) => {
            if items.is_empty() && !allow_verified_empty {
                return Err(CourseJsonError::UnverifiedEmptyCollection);
            }
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let object = item
                        .as_object()
                        .ok_or(CourseJsonError::InvalidRecord { index: Some(index) })?;
                    validate_course_record(map_course_object(object), Some(index))
                })
                .collect()
        }
        Value::Object(object) => {
            validate_course_success_marker(object)?;
            if has_known_course_field(object) {
                return Ok(vec![validate_course_record(
                    map_course_object(object),
                    None,
                )?]);
            }

            if let Some((field, nested)) = find_collection(object) {
                return match nested {
                    Value::Array(items) => {
                        if items.is_empty() && !course_response_is_success(object) {
                            return Err(CourseJsonError::UnverifiedEmptyCollection);
                        }
                        parse_course_value_with_empty(nested, course_response_is_success(object))
                    }
                    Value::Object(_) => parse_course_value_with_empty(nested, false),
                    Value::Null => Err(CourseJsonError::InvalidCollection {
                        field: field.to_owned(),
                    }),
                    _ => Err(CourseJsonError::InvalidCollection {
                        field: field.to_owned(),
                    }),
                };
            }

            Err(CourseJsonError::MissingCollection)
        }
        _ => Err(CourseJsonError::InvalidRoot),
    }
}

fn course_response_is_success(object: &Map<String, Value>) -> bool {
    ["message", "status", "success", "resultCode"]
        .iter()
        .find_map(|name| {
            object
                .iter()
                .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
        })
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => Some(value.as_i64() == Some(0) || value.as_i64() == Some(200)),
            Value::String(value) => Some(
                value.eq_ignore_ascii_case("success")
                    || value.eq_ignore_ascii_case("ok")
                    || value == "0"
                    || value == "200",
            ),
            _ => None,
        })
        .unwrap_or(false)
}

fn validate_course_success_marker(object: &Map<String, Value>) -> Result<(), CourseJsonError> {
    let marker = ["message", "status", "success", "resultCode"]
        .iter()
        .find_map(|name| {
            object
                .iter()
                .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value))
        });
    let Some(marker) = marker else {
        return Ok(());
    };
    if course_response_is_success(object) {
        return Ok(());
    }
    let message = match marker {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    Err(CourseJsonError::BusinessFailure { message })
}

fn find_collection(object: &Map<String, Value>) -> Option<(&str, &Value)> {
    let mut invalid_candidate = None;
    for candidate in COURSE_COLLECTION_KEYS {
        if let Some((key, value)) = object.iter().find_map(|(key, value)| {
            key.eq_ignore_ascii_case(candidate)
                .then_some((key.as_str(), value))
        }) {
            if matches!(value, Value::Array(_) | Value::Object(_) | Value::Null) {
                return Some((key, value));
            }
            invalid_candidate.get_or_insert((key, value));
        }
    }
    invalid_candidate
}

fn has_known_course_field(object: &Map<String, Value>) -> bool {
    object.keys().any(|key| is_known_course_field(key.as_str()))
}

fn is_known_course_field(key: &str) -> bool {
    [
        COURSE_ID_KEYS,
        COURSE_CODE_KEYS,
        COURSE_NAME_KEYS,
        INSTRUCTOR_KEYS,
        CLASS_NAME_KEYS,
        SEMESTER_KEYS,
    ]
    .into_iter()
    .flatten()
    .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

fn map_course_object(object: &Map<String, Value>) -> LearnCourseRecord {
    LearnCourseRecord {
        course_id: first_scalar(object, COURSE_ID_KEYS),
        course_code: first_scalar(object, COURSE_CODE_KEYS),
        name: first_text_scalar(object, COURSE_NAME_KEYS),
        instructor: first_text_scalar(object, INSTRUCTOR_KEYS),
        class_name: first_text_scalar(object, CLASS_NAME_KEYS),
        semester: first_scalar(object, SEMESTER_KEYS),
        unknown_fields: object
            .iter()
            .filter(|(key, _)| !is_known_course_field(key) || is_supplemental_course_field(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}

fn is_supplemental_course_field(key: &str) -> bool {
    [
        "kcm",
        "chineseName",
        "chinese_name",
        "ywkcm",
        "englishName",
        "english_name",
        "jsh",
        "teacherNumber",
        "teacher_number",
        "kxh",
        "courseIndex",
        "course_index",
    ]
    .iter()
    .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

fn validate_course_record(
    record: LearnCourseRecord,
    index: Option<usize>,
) -> Result<LearnCourseRecord, CourseJsonError> {
    if record.id().is_none() {
        return Err(CourseJsonError::IncompleteRecord {
            index,
            field: "course id",
        });
    }
    if record.title().is_none() {
        return Err(CourseJsonError::IncompleteRecord {
            index,
            field: "course title",
        });
    }
    Ok(record)
}

fn first_scalar(object: &Map<String, Value>, aliases: &[&str]) -> Option<String> {
    aliases.iter().find_map(|alias| {
        object.iter().find_map(|(key, value)| {
            if !key.eq_ignore_ascii_case(alias) {
                return None;
            }
            match value {
                Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            }
        })
    })
}

fn first_text_scalar(object: &Map<String, Value>, aliases: &[&str]) -> Option<String> {
    aliases.iter().find_map(|alias| {
        object.iter().find_map(|(key, value)| {
            if !key.eq_ignore_ascii_case(alias) {
                return None;
            }
            match value {
                Value::String(value) if !value.trim().is_empty() => {
                    Some(decode_learn_html(value.trim()))
                }
                _ => None,
            }
        })
    })
}

fn first_scalar_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn extra_scalar(object: &BTreeMap<String, Value>, aliases: &[&str]) -> Option<String> {
    aliases.iter().find_map(|alias| {
        object.iter().find_map(|(key, value)| {
            if !key.eq_ignore_ascii_case(alias) {
                return None;
            }
            match value {
                Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            }
        })
    })
}

fn extra_text_scalar(object: &BTreeMap<String, Value>, aliases: &[&str]) -> Option<String> {
    aliases.iter().find_map(|alias| {
        object.iter().find_map(|(key, value)| {
            if !key.eq_ignore_ascii_case(alias) {
                return None;
            }
            match value {
                Value::String(value) if !value.trim().is_empty() => {
                    Some(decode_learn_html(value.trim()))
                }
                _ => None,
            }
        })
    })
}

/// Decodes the HTML entities used by Learn's JSON fields.  The current public
/// client performs this step for course names and announcement text before
/// exposing them to callers.  Keep the decoder small and dependency-free while
/// handling the named entities and numeric forms emitted by the site; unknown
/// entities are retained verbatim so a malformed value cannot silently lose
/// content.
pub(crate) fn decode_learn_html(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('&') else {
            decoded.push_str(&value[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        decoded.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start..].find(';') else {
            decoded.push_str(&value[start..]);
            break;
        };
        let end = start + relative_end;
        let entity = &value[start + 1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            decoded.push(replacement);
            cursor = end + 1;
        } else {
            decoded.push_str(&value[start..=end]);
            cursor = end + 1;
        }
    }
    // Learn's legacy response sometimes starts display text with one of the
    // mojibake prefixes removed by the current public client after entity
    // decoding. Keep the same narrow cleanup and leave all other text intact.
    for prefix in [
        "\u{00c2}\u{009e}\u{00c3}\u{00a9}e",
        "\u{009e}\u{00e9}e",
        "\u{00e9}e",
    ] {
        if let Some(stripped) = decoded.strip_prefix(prefix) {
            return stripped.to_owned();
        }
    }
    decoded
}

#[derive(Debug, Error)]
pub enum CourseTimeLocationParseError {
    #[error("course time/location response JSON could not be decoded: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("course time/location response must be an array")]
    InvalidRoot,

    #[error("course time/location item at index {index} must be a non-empty string")]
    InvalidEntry { index: usize },
}

pub fn parse_course_time_location_response(
    body: &str,
) -> Result<Vec<String>, CourseTimeLocationParseError> {
    let value: Value = serde_json::from_str(body)?;
    let items = value
        .as_array()
        .ok_or(CourseTimeLocationParseError::InvalidRoot)?;
    items
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let text = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or(CourseTimeLocationParseError::InvalidEntry { index })?;
            Ok(text.to_owned())
        })
        .collect()
}

fn validate_course_identifier(value: &str) -> Result<(), LearnClientError> {
    if value.trim().is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '?' | '#'))
    {
        return Err(LearnClientError::Learn(LearnError::InvalidPathSegment));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlAttribute {
    name: String,
    value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HtmlTag {
    name: String,
    attributes: Vec<HtmlAttribute>,
}

impl HtmlTag {
    fn attribute_value(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
            .and_then(|attribute| attribute.value.as_deref())
    }
}

fn scan_evidence_tags(html: &str) -> Result<Vec<HtmlTag>, CsrfParseError> {
    let bytes = html.as_bytes();
    let mut cursor = 0;
    let mut tags = Vec::new();

    while let Some(relative_start) = html[cursor..].find('<') {
        let start = cursor + relative_start;
        if html[start..].starts_with("<!--") {
            let Some(relative_end) = html[start + 4..].find("-->") else {
                return Err(CsrfParseError::MalformedTag {
                    element: "html",
                    message: "unterminated comment".to_owned(),
                });
            };
            cursor = start + 4 + relative_end + 3;
            continue;
        }

        let Some(name_end) = tag_name_end(bytes, start + 1) else {
            cursor = start + 1;
            continue;
        };
        let name = html[start + 1..name_end].to_ascii_lowercase();

        if name == "script" || name == "style" {
            let end = find_tag_end(bytes, start).ok_or_else(|| CsrfParseError::MalformedTag {
                element: "html",
                message: format!("unterminated {name} opening tag"),
            })?;
            let closing = format!("</{name}");
            if let Some(relative_closing) = find_ascii_case_insensitive(&html[end + 1..], &closing)
            {
                cursor = end + 1 + relative_closing;
            } else {
                cursor = html.len();
            }
            continue;
        }

        let is_candidate = matches!(name.as_str(), "meta" | "input" | "form" | "a");
        if !is_candidate {
            cursor = start + 1;
            continue;
        }

        // Closing tags have no attributes and are irrelevant evidence.
        if bytes.get(start + 1) == Some(&b'/') {
            cursor = start + 1;
            continue;
        }

        let end = find_tag_end(bytes, start).ok_or_else(|| CsrfParseError::MalformedTag {
            element: evidence_element_name(&name),
            message: "missing closing `>`".to_owned(),
        })?;
        let source = &html[start + 1..end];
        tags.push(parse_evidence_tag(source, &name)?);
        cursor = end + 1;
    }

    Ok(tags)
}

fn tag_name_end(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    if bytes.get(cursor) == Some(&b'/') {
        cursor += 1;
    }
    let start = cursor;
    while let Some(byte) = bytes.get(cursor) {
        if byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-' {
            cursor += 1;
        } else {
            break;
        }
    }
    (cursor > start).then_some(cursor)
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, byte) in bytes.iter().enumerate().skip(start + 1) {
        match quote {
            Some(expected) if *byte == expected => quote = None,
            Some(_) => {}
            None if *byte == b'\'' || *byte == b'"' => quote = Some(*byte),
            None if *byte == b'>' => return Some(offset),
            None => {}
        }
    }
    None
}

fn parse_evidence_tag(source: &str, expected_name: &str) -> Result<HtmlTag, CsrfParseError> {
    let bytes = source.as_bytes();
    let name_end = tag_name_end(bytes, 0).ok_or_else(|| CsrfParseError::MalformedTag {
        element: evidence_element_name(expected_name),
        message: "missing tag name".to_owned(),
    })?;
    let actual_name = source[..name_end].to_ascii_lowercase();
    if actual_name != expected_name {
        return Err(CsrfParseError::MalformedTag {
            element: evidence_element_name(expected_name),
            message: "tag name changed while parsing".to_owned(),
        });
    }

    let mut cursor = name_end;
    let mut attributes = Vec::new();
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        if bytes[cursor] == b'/' {
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor != bytes.len() {
                return malformed_evidence(expected_name, "unexpected characters after `/`");
            }
            break;
        }

        let attribute_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        if cursor == attribute_start {
            return malformed_evidence(expected_name, "missing attribute name");
        }
        let name = source[attribute_start..cursor].to_ascii_lowercase();
        if attributes
            .iter()
            .any(|attribute: &HtmlAttribute| attribute.name == name)
        {
            return malformed_evidence(expected_name, "duplicate attribute");
        }

        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let value = if bytes.get(cursor) == Some(&b'=') {
            cursor += 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor >= bytes.len() {
                return malformed_evidence(expected_name, "missing attribute value");
            }
            if bytes[cursor] == b'\'' || bytes[cursor] == b'"' {
                let quote = bytes[cursor];
                cursor += 1;
                let value_start = cursor;
                while cursor < bytes.len() && bytes[cursor] != quote {
                    cursor += 1;
                }
                if cursor >= bytes.len() {
                    return malformed_evidence(expected_name, "unterminated quoted attribute");
                }
                let value = source[value_start..cursor].to_owned();
                cursor += 1;
                value
            } else {
                let value_start = cursor;
                while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                if cursor == value_start {
                    return malformed_evidence(expected_name, "empty unquoted attribute");
                }
                source[value_start..cursor].to_owned()
            }
        } else {
            String::new()
        };

        attributes.push(HtmlAttribute {
            name,
            value: if value.is_empty() { None } else { Some(value) },
        });
    }

    Ok(HtmlTag {
        name: actual_name,
        attributes,
    })
}

fn evidence_element_name(name: &str) -> &'static str {
    match name {
        "meta" => "meta",
        "input" => "input",
        "form" => "form",
        "a" => "a",
        "query" => "query",
        _ => "html",
    }
}

fn malformed_evidence(expected_name: &str, message: &str) -> Result<HtmlTag, CsrfParseError> {
    Err(CsrfParseError::MalformedTag {
        element: evidence_element_name(expected_name),
        message: message.to_owned(),
    })
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;
    use serde_json::json;

    const CSRF_HTML_FIXTURE: &str = r#"
        <!doctype html>
        <html>
          <head>
            <meta charset="utf-8">
            <!-- <meta name="_csrf" content="comment-token"> -->
            <meta name='_csrf' content='fixture-csrf-token' />
          </head>
          <body>
            <script>
              const fake = '<input name="_csrf" value="script-token">';
            </script>
            <input type="hidden" name="_csrf" value="fixture-csrf-token">
          </body>
        </html>
    "#;

    const COURSE_JSON_FIXTURE: &str = r#"
        {
          "data": [
            {
              "courseId": 1001,
              "courseCode": "CS-101",
              "courseName": "程序设计基础",
              "teacherName": "教师甲",
              "className": "2026秋-01",
              "semester": "2025-2026-2",
              "newServerField": {"enabled": true},
              "newLabel": ["forward-compatible"]
            },
            {
              "courseId": 1002,
              "courseName": "只有名称的课程",
              "futureScalar": 7
            }
          ],
          "total": 2
        }
    "#;

    fn student_client() -> LearnClient {
        LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("fixture base URL is valid"),
        )
    }

    #[test]
    fn plans_roaming_ticket_without_logging_or_preencoding_it() {
        let client = student_client();
        let plan = client
            .roaming_request_plan("ticket with spaces/+ and opaque bytes")
            .expect("roaming plan builds");

        assert_eq!(plan.method, LearnRequestMethod::Get);
        assert_eq!(plan.path, "/b/j_spring_security_thauth_roaming_entry");
        assert_eq!(
            plan.endpoint.as_str(),
            "https://learn.example.test/b/j_spring_security_thauth_roaming_entry"
        );
        assert_eq!(plan.query.as_str(), "ticket with spaces/+ and opaque bytes");
        assert!(!format!("{plan:?}").contains("ticket with spaces"));

        let transport = CampusHttpTransport::new("THYou/test").expect("transport builds");
        let request = plan
            .build_request(&transport)
            .expect("request plan builds a GET request");
        assert_eq!(request.method(), reqwest::Method::GET);
        let query_pairs: Vec<_> = request
            .url()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(
            query_pairs,
            vec![(
                "ticket".to_owned(),
                "ticket with spaces/+ and opaque bytes".to_owned()
            )]
        );
    }

    #[test]
    fn builds_cookie_backed_roaming_without_a_ticket_query() {
        let client = student_client();
        let transport = CampusHttpTransport::new("THYou/test").expect("transport builds");
        let request = client
            .build_cookie_roaming_request(&transport)
            .expect("cookie roaming request builds");

        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().path(),
            "/f/j_spring_security_thauth_roaming_entry"
        );
        assert!(request.url().query().is_none());
    }

    #[test]
    fn plans_student_and_teacher_course_routes_from_the_role_profile() {
        let student = student_client()
            .course_list_request_plan("2025-2026-2", Some("zh"), None)
            .expect("student plan builds");
        assert_eq!(student.role, CourseRole::Student);
        assert!(student.requires_csrf());
        assert_eq!(
            student.endpoint.path(),
            "/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/zh"
        );

        let teacher_config =
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Teacher)
                .expect("fixture base URL is valid");
        let teacher = LearnClient::new(teacher_config)
            .course_list_request_plan("2025-2026-2", None, Some("7"))
            .expect("teacher plan builds");
        assert_eq!(teacher.role, CourseRole::Teacher);
        assert_eq!(
            teacher.endpoint.path(),
            "/b/kc/v_wlkc_kcb/queryAsorCoCourseList/2025-2026-2/7"
        );
    }

    #[test]
    fn extracts_matching_meta_and_hidden_input_csrf_evidence() {
        let evidence = extract_csrf(CSRF_HTML_FIXTURE).expect("fixture CSRF parses");
        assert_eq!(evidence.token.as_str(), "fixture-csrf-token");
        assert_eq!(evidence.sources, vec![CsrfSource::Meta, CsrfSource::Input]);
        assert!(!format!("{evidence:?}").contains("fixture-csrf-token"));
    }

    #[test]
    fn extracts_csrf_from_an_authenticated_learning_link() {
        let html = r#"
            <a href="/f/wlxt/index/course/student/?foo=bar&amp;_csrf=link-token-123">课程</a>
        "#;
        let evidence = extract_csrf(html).expect("link CSRF parses");
        assert_eq!(evidence.token.as_str(), "link-token-123");
        assert_eq!(evidence.sources, vec![CsrfSource::Link, CsrfSource::Query]);
        assert!(!format!("{evidence:?}").contains("link-token-123"));
    }

    #[test]
    fn decodes_percent_encoded_csrf_once_when_reading_generated_links() {
        let html = r#"
            <script>
              const courseUrl = "/course/student/?_csrf=link-token%2Bvalue";
            </script>
        "#;
        // A plus in the token is encoded as %2B and must not become a second
        // percent-encoding when the later request appends the token.
        let evidence = extract_csrf(html).expect("encoded query CSRF parses");
        assert_eq!(evidence.token.as_str(), "link-token+value");
    }

    #[test]
    fn extracts_csrf_from_a_generated_query_and_ignores_comment_text() {
        let html = r#"
            <!-- "/course?_csrf=comment-token" -->
            <script>
                const courseUrl = "/course/student/?_csrf=query-token-123";
            </script>
        "#;
        let evidence = extract_csrf(html).expect("generated query CSRF parses");
        assert_eq!(evidence.token.as_str(), "query-token-123");
        assert_eq!(evidence.sources, vec![CsrfSource::Query]);
        assert!(!format!("{evidence:?}").contains("query-token-123"));
    }

    #[test]
    fn csrf_query_evidence_must_agree_with_visible_page_evidence() {
        let matching = r#"
            <meta name="_csrf" content="same-page-value">
            <script>const courseUrl = "/course/student/?_csrf=same-page-value";</script>
        "#;
        let evidence = extract_csrf(matching).expect("matching query evidence parses");
        assert_eq!(evidence.sources, vec![CsrfSource::Meta, CsrfSource::Query]);

        let meta_conflict = r#"
            <meta name="_csrf" content="meta-page-value">
            <script>const courseUrl = "/course/student/?_csrf=script-page-value";</script>
        "#;
        assert!(matches!(
            extract_csrf(meta_conflict),
            Err(CsrfParseError::ConflictingTokens)
        ));

        let input_conflict = r#"
            <input type="hidden" name="_csrf" value="input-page-value">
            <script>const courseUrl = "/course/student/?_csrf=script-page-value";</script>
        "#;
        assert!(matches!(
            extract_csrf(input_conflict),
            Err(CsrfParseError::ConflictingTokens)
        ));

        let link_conflict = r#"
            <a href="/course/student/?_csrf=link-page-value">course</a>
            <script>const courseUrl = "/course/student/?_csrf=script-page-value";</script>
        "#;
        assert!(matches!(
            extract_csrf(link_conflict),
            Err(CsrfParseError::ConflictingTokens)
        ));
    }

    #[test]
    fn allows_an_explicit_legacy_roaming_route_override() {
        let config = LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
            .expect("fixture base URL is valid")
            .with_roaming_entry_path("/b/j_spring_security_thauth_roaming_entry")
            .expect("legacy route is valid");
        let plan = LearnClient::new(config)
            .roaming_request_plan("opaque-ticket")
            .expect("roaming plan builds");
        assert_eq!(plan.path, "/b/j_spring_security_thauth_roaming_entry");
    }

    #[test]
    fn csrf_parser_rejects_missing_conflicting_and_malformed_evidence() {
        assert!(matches!(
            extract_csrf("<html><body><input name='other' value='x'></body></html>"),
            Err(CsrfParseError::Missing)
        ));
        assert!(matches!(
            extract_csrf(
                "<meta name='_csrf' content='first'><input type='hidden' name='_csrf' value='second'>"
            ),
            Err(CsrfParseError::ConflictingTokens)
        ));
        assert!(matches!(
            extract_csrf("<input type='hidden' name='_csrf' value='unterminated>"),
            Err(CsrfParseError::MalformedTag {
                element: "input",
                ..
            })
        ));
        assert!(matches!(
            extract_csrf("<input type='text' name='_csrf' value='fixture-token'>"),
            Err(CsrfParseError::InvalidInputType)
        ));
    }

    #[test]
    fn csrf_parser_does_not_read_comments_or_scripts_as_page_evidence() {
        let html = r#"
            <!-- <input type="hidden" name="_csrf" value="comment-token"> -->
            <script>var html = '<meta name="_csrf" content="script-token">';</script>
        "#;
        assert!(matches!(extract_csrf(html), Err(CsrfParseError::Missing)));
    }

    #[test]
    fn maps_minimal_tolerant_course_fixture_and_keeps_unknown_fields() {
        let records = parse_course_records(COURSE_JSON_FIXTURE).expect("fixture JSON parses");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].course_id.as_deref(), Some("1001"));
        assert_eq!(records[0].course_code.as_deref(), Some("CS-101"));
        assert_eq!(records[0].name.as_deref(), Some("程序设计基础"));
        assert_eq!(records[0].instructor.as_deref(), Some("教师甲"));
        assert_eq!(records[0].class_name.as_deref(), Some("2026秋-01"));
        assert_eq!(records[0].semester.as_deref(), Some("2025-2026-2"));
        assert_eq!(
            records[0].unknown_fields.get("newServerField"),
            Some(&json!({"enabled": true}))
        );
        assert_eq!(records[1].course_id.as_deref(), Some("1002"));
        assert_eq!(records[1].name.as_deref(), Some("只有名称的课程"));
        assert_eq!(
            records[1].unknown_fields.get("futureScalar"),
            Some(&json!(7))
        );
    }

    #[test]
    fn maps_learning_platform_course_identifiers_used_by_content_endpoints() {
        let records = parse_course_records(
            r#"{
                "resultList": [
                    {
                        "wlkcid": 7001,
                        "kch": "30240512",
                        "zywkcm": "Data Structures",
                        "kcm": "数据结构",
                        "ywkcm": "Data Structures",
                        "jsm": "教师甲"
                    }
                ]
            }"#,
        )
        .expect("learning course JSON parses");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id(), Some("7001"));
        assert_eq!(records[0].code(), Some("30240512"));
        assert_eq!(records[0].title(), Some("Data Structures"));
        assert_eq!(records[0].instructor.as_deref(), Some("教师甲"));
        assert_eq!(records[0].chinese_name().as_deref(), Some("数据结构"));
        assert_eq!(
            records[0].english_name().as_deref(),
            Some("Data Structures")
        );
    }

    #[test]
    fn decodes_html_entities_in_course_display_fields() {
        let records = parse_course_records(
            r#"{"message":"success","resultList":[{"wlkcid":"7001","zywkcm":"数据结构 &amp; 算法","kcm":"数据结构 &amp; 算法","ywkcm":"Data &amp; Structures"}]}"#,
        )
        .expect("course display fields parse");
        assert_eq!(records[0].title(), Some("数据结构 & 算法"));
        assert_eq!(
            records[0].chinese_name().as_deref(),
            Some("数据结构 & 算法")
        );
        assert_eq!(
            records[0].english_name().as_deref(),
            Some("Data & Structures")
        );
        assert_eq!(
            decode_learn_html("unknown &madeup; &#x2605;"),
            "unknown &madeup; ★"
        );
    }

    #[test]
    fn removes_only_the_public_learn_mojibake_prefixes_after_entity_decoding() {
        for (value, expected) in [
            ("\u{00c2}\u{009e}\u{00c3}\u{00a9}e课程", "课程"),
            ("\u{009e}\u{00e9}e课程", "课程"),
            ("\u{00e9}e课程", "课程"),
            ("\u{00e9}课程", "\u{00e9}课程"),
        ] {
            assert_eq!(decode_learn_html(value), expected);
        }
    }

    #[test]
    fn maps_teacher_number_and_course_index_without_confusing_course_id() {
        let records = parse_course_records(
            r#"{"message":"success","resultList":[{"wlkcid":7001,"kch":"30240512","zywkcm":"课程","jsh":42,"kxh":"02"}]}"#,
        )
        .expect("course response parses");

        assert_eq!(records[0].id(), Some("7001"));
        assert_eq!(records[0].teacher_number().as_deref(), Some("42"));
        assert_eq!(records[0].course_index().as_deref(), Some("02"));
    }

    #[test]
    fn course_time_location_parser_accepts_only_the_observed_string_array() {
        assert_eq!(
            parse_course_time_location_response(r#"["周一 1-2 节 清华园教室","周三 3-4 节"]"#,)
                .expect("time/location array parses"),
            vec!["周一 1-2 节 清华园教室", "周三 3-4 节"]
        );
        assert_eq!(
            parse_course_time_location_response("[]").expect("verified empty array parses"),
            Vec::<String>::new()
        );
        assert!(matches!(
            parse_course_time_location_response(r#"{"data":[]}"#),
            Err(CourseTimeLocationParseError::InvalidRoot)
        ));
        assert!(matches!(
            parse_course_time_location_response(r#"["  "]"#),
            Err(CourseTimeLocationParseError::InvalidEntry { index: 0 })
        ));
    }

    #[test]
    fn does_not_promote_a_course_number_to_a_learning_course_id() {
        for body in [
            r#"{"resultList":[{"kch":"30240512","kcm":"数据结构"}]}"#,
            r#"{"resultList":[{"courseNo":"30240512","courseName":"数据结构"}]}"#,
        ] {
            assert!(matches!(
                parse_course_records(body),
                Err(CourseJsonError::IncompleteRecord {
                    index: Some(0),
                    field: "course id"
                })
            ));
        }
    }

    #[test]
    fn course_mapping_rejects_unverified_or_malformed_success_shapes() {
        assert!(matches!(
            parse_course_records(r#"{"futureField":{"v":1}}"#),
            Err(CourseJsonError::MissingCollection)
        ));

        assert!(matches!(
            parse_course_records(r#"{"data":[null,3,{"name":"kept"}]}"#),
            Err(CourseJsonError::InvalidRecord { index: Some(0) })
        ));

        assert!(matches!(
            parse_course_records(r#"{"data":[{"courseId":"1001"}]}"#),
            Err(CourseJsonError::IncompleteRecord {
                index: Some(0),
                field: "course title"
            })
        ));

        assert!(matches!(
            parse_course_records(r#"{"resultList":[]}"#),
            Err(CourseJsonError::UnverifiedEmptyCollection)
        ));
        assert_eq!(
            parse_course_records(r#"{"message":"success","resultList":[],"status":"success"}"#)
                .expect("explicit empty success parses"),
            Vec::<LearnCourseRecord>::new()
        );
        assert!(matches!(
            parse_course_records(r#"{"message":"请先登录","resultList":[]}"#),
            Err(CourseJsonError::BusinessFailure { .. })
        ));

        assert!(matches!(
            parse_confirmed_course_list_response(r#"{"resultList":[]}"#),
            Err(CourseJsonError::MissingSuccessMarker)
        ));
        assert_eq!(
            parse_confirmed_course_list_response(r#"{"message":"success","resultList":[]}"#)
                .expect("confirmed empty course list parses"),
            Vec::<LearnCourseRecord>::new()
        );
        assert!(matches!(
            parse_confirmed_course_list_response_for_semester(
                r#"{"message":"success","resultList":[{"wlkcid":"7001","kcm":"课程","semester":"2025-2026-1"}]}"#,
                Some("2025-2026-2"),
            ),
            Err(CourseJsonError::SemesterMismatch {
                index: Some(0),
                expected,
                actual,
            }) if expected == "2025-2026-2" && actual == "2025-2026-1"
        ));
        assert!(matches!(
            parse_confirmed_course_list_response_for_semester(
                r#"{"message":"success","resultList":[{"wlkcid":"7001","kcm":"课程","semester":null}]}"#,
                Some("2025-2026-2"),
            ),
            Err(CourseJsonError::InvalidField {
                index: Some(0),
                field: "semester",
            })
        ));

        assert!(matches!(
            parse_course_records("true"),
            Err(CourseJsonError::InvalidRoot)
        ));
    }

    #[test]
    fn parses_role_specific_semesters_and_current_semester_strictly() {
        assert_eq!(
            parse_semester_ids(r#"["2025-2026-1",null,"2025-2026-2"]"#)
                .expect("known null semester rows are filtered"),
            vec!["2025-2026-1", "2025-2026-2"]
        );
        assert!(matches!(
            parse_semester_ids(r#"["2026-fall"]"#),
            Err(SemesterParseError::InvalidEntry { index: 0 })
        ));
        assert!(matches!(
            parse_semester_ids("{}"),
            Err(SemesterParseError::InvalidListRoot)
        ));
        let current = parse_current_semester(
            r#"{"message":"success","result":{"id":"2026-2027-1","kssj":"2026-09-01","jssj":"2027-01-15 00:00:00","xnxq":"2026-2027-1"}}"#,
        )
        .expect("current semester parses");
        assert_eq!(current.id, "2026-2027-1");
        assert!(matches!(
            parse_current_semester(r#"{"message":"error","result":{}}"#),
            Err(SemesterParseError::CurrentFailure { .. })
        ));
        assert!(matches!(
            parse_current_semester(
                r#"{"message":"success","result":{"id":"2026-fall","kssj":"2026-09-01","jssj":"2027-01-15","xnxq":"2026-fall"}}"#
            ),
            Err(SemesterParseError::InvalidCurrentField { field: "id", .. })
        ));
        assert!(matches!(
            parse_current_semester(
                r#"{"message":"success","result":{"id":"2026-2027-1","kssj":"2026-09-01","jssj":"2027-01-15","xnxq":"2026-2027-2"}}"#
            ),
            Err(SemesterParseError::CurrentIdLabelMismatch)
        ));
    }

    #[test]
    fn backend_repair_learn_term_calendar_requires_complete_bounded_following_terms() {
        let body = r#"{"message":"success","result":{"id":"2026-2027-1","kssj":"2026-09-02","jssj":"2027-01-15","xnxq":"2026-2027-1","xnxqmc":"2026-2027学年秋季学期"},"resultList":[{"id":"2026-2027-2","kssj":"2027-02-21","jssj":"2027-07-02","xnxqmc":"春季学期"}]}"#;
        let calendar = parse_term_calendar(body).unwrap();
        assert_eq!(calendar.current.label, "2026-2027学年秋季学期");
        assert_eq!(calendar.upcoming.len(), 1);
        assert_eq!(calendar.upcoming[0].id, "2026-2027-2");
        assert_eq!(calendar.upcoming[0].label, "春季学期");

        assert!(matches!(
            parse_term_calendar(
                r#"{"message":"success","result":{"id":"2026-2027-1","kssj":"2026-09-02","jssj":"2027-01-15","xnxq":"2026-2027-1"}}"#
            ),
            Err(SemesterParseError::InvalidCalendarList)
        ));
        let repeated = body.replace("2026-2027-2", "2026-2027-1");
        assert!(matches!(
            parse_term_calendar(&repeated),
            Err(SemesterParseError::DuplicateCalendarTerm)
        ));
        let invalid = body.replace("2027-02-21", "not-a-date");
        assert!(matches!(
            parse_term_calendar(&invalid),
            Err(SemesterParseError::InvalidCalendarEntry { field: "date", .. })
        ));
    }

    #[tokio::test]
    async fn fetches_semesters_with_real_get_paths_and_reuses_cookie() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let responses = [
                (
                    "/f/wlxt/index/course/student/index",
                    r#"<html><meta name="_csrf" content="learn-csrf"></html>"#,
                    "Set-Cookie: learn-session=present; Path=/\r\n",
                    "text/html",
                ),
                (
                    "/b/wlxt/kc/v_wlkc_xs_xktjb_coassb/queryxnxq?_csrf=learn-csrf",
                    r#"["2025-2026-2"]"#,
                    "",
                    "application/json",
                ),
                (
                    "/f/wlxt/index/course/student/index",
                    r#"<html><meta name="_csrf" content="learn-csrf"></html>"#,
                    "",
                    "text/html",
                ),
                (
                    "/b/kc/zhjw_v_code_xnxq/getCurrentAndNextSemester?_csrf=learn-csrf",
                    r#"{"message":"success","result":{"id":"2025-2026-2","kssj":"2026-02-23 00:00:00","jssj":"2026-07-12 00:00:00","xnxq":"2025-2026-2"}}"#,
                    "",
                    "application/json",
                ),
            ];

            for (expected_target, body, extra_headers, content_type) in responses {
                let (mut stream, _) = listener.accept().expect("connection");
                let request = read_http_request(&mut stream);
                assert!(request.starts_with(&format!("GET {expected_target} HTTP/1.1\r\n")));
                if expected_target.contains("?_csrf=") {
                    assert!(request.contains("_csrf=learn-csrf"));
                    assert!(
                        request
                            .lines()
                            .any(|line| line.eq_ignore_ascii_case("cookie: learn-session=present"))
                    );
                }
                write_http_response(&mut stream, content_type, body, extra_headers);
            }
        });

        let base_url = format!("http://{address}/");
        let client = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("config"),
        );
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");

        assert_eq!(
            client
                .fetch_semester_ids(&transport)
                .await
                .expect("semester request"),
            vec!["2025-2026-2"]
        );
        assert_eq!(
            client
                .fetch_current_semester(&transport)
                .await
                .expect("current semester request")
                .id,
            "2025-2026-2"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn course_home_rejects_a_same_origin_redirect_to_another_route() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("course-home request");
            let _ = read_http_request(&mut first);
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect");

            let (mut second, _) = listener.accept().expect("redirected request");
            let _ = read_http_request(&mut second);
            write_http_response(&mut second, "text/html", CSRF_HTML_FIXTURE, "");
        });

        let client = LearnClient::new(
            LearnClientConfig::new(&format!("http://{address}/"), CourseRole::Student)
                .expect("config"),
        );
        let transport = CampusHttpTransport::with_timeout(
            "THYou/learn-route-proof-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        assert!(matches!(
            client.execute_course_home(&transport).await,
            Err(LearnClientError::UnexpectedPath)
        ));
        server.join().expect("server");
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 512];
        loop {
            let read = stream.read(&mut buffer).expect("request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(request)
            .expect("HTTP request is UTF-8")
            .to_owned()
    }

    fn write_http_response(
        stream: &mut std::net::TcpStream,
        content_type: &str,
        body: &str,
        extra_headers: &str,
    ) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("response");
    }

    #[test]
    fn identifies_http_200_login_expiry_by_final_url_form_or_marker() {
        let client = student_client();
        let login_url =
            Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/form/app/0?service=learn")
                .expect("fixture URL is valid");
        assert!(client.is_login_expired_page(StatusCode::OK, Some(&login_url), "<html />"));

        let form_html = r#"
            <form action="https://id.tsinghua.edu.cn/do/off/ui/auth/login/check">
              <input name="i_user" type="text">
              <input name="i_pass" type="password">
            </form>
        "#;
        let response = client
            .map_html_response(StatusCode::OK, None, form_html)
            .expect("login form maps");
        assert!(matches!(
            response.classification,
            LearnPageClassification::LoginExpired(_)
        ));

        let external_login =
            Url::parse("https://id.tsinghua.edu.cn/do/off/ui/auth/login/form/app/0?service=learn")
                .expect("identity login URL is valid");
        let response = client
            .map_html_response(StatusCode::OK, Some(&external_login), "<html />")
            .expect("identity login redirect is classified");
        assert!(matches!(
            response.classification,
            LearnPageClassification::LoginExpired(_)
        ));

        assert!(client.is_login_expired_page(
            StatusCode::OK,
            None,
            "<html><body>您的会话已过期，请先登录</body></html>"
        ));
        let learn_timeout_url =
            Url::parse("https://learn.example.test/f/wlxt/index/course/student/")
                .expect("Learn URL is valid");
        assert!(client.is_login_expired_page(
            StatusCode::OK,
            Some(&learn_timeout_url),
            "<title>登录超时</title><p>您未登录或登录失效</p>"
        ));
        // A bare final URL cannot prove which redirect was followed. The
        // complete response path must retain and classify the Location header
        // instead of treating every 302 as an expired Learn session.
        assert!(!client.is_login_expired_page(StatusCode::FOUND, Some(&login_url), "<html />"));
    }

    #[test]
    fn maps_authenticated_html_only_when_csrf_evidence_is_available() {
        let client = student_client();
        let response = client
            .map_html_response(StatusCode::OK, None, CSRF_HTML_FIXTURE)
            .expect("authenticated fixture maps");
        assert!(matches!(
            response.classification,
            LearnPageClassification::Authenticated
        ));
        assert_eq!(
            response.csrf.expect("CSRF is retained").token.as_str(),
            "fixture-csrf-token"
        );

        let unknown = client
            .map_html_response(
                StatusCode::OK,
                None,
                "<html><body>course shell</body></html>",
            )
            .expect("unknown page maps without guessing");
        assert!(matches!(
            unknown.classification,
            LearnPageClassification::Unknown
        ));
    }

    #[tokio::test]
    async fn fetches_course_records_with_bound_csrf_cookie_and_route_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("course request");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with(
                "GET /b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/zh?_csrf=learn-csrf HTTP/1.1\r\n"
            ));
            assert!(request.contains("learn-session=present"));
            write_http_response(
                &mut stream,
                "application/json",
                r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240512","kcm":"数据结构"}]}"#,
                "",
            );
        });

        let base_url = format!("http://{address}/");
        let client = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("config"),
        );
        let transport = CampusHttpTransport::with_timeout(
            "THYou/course-proof-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let base = Url::parse(&base_url).expect("base URL");
        transport
            .cookie_jar()
            .add_cookie_str("learn-session=present; Path=/", &base);
        let registry = crate::session::SessionRegistry::new();
        let csrf = registry.bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("learn-csrf").expect("csrf"),
        );

        let records = client
            .fetch_course_records(&transport, &csrf, "2025-2026-2", Some("zh"), None)
            .await
            .expect("course response");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id(), Some("7001"));
        assert_eq!(records[0].title(), Some("数据结构"));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn course_records_reject_an_http_200_login_page() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("course request");
            let _ = read_http_request(&mut stream);
            write_http_response(
                &mut stream,
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#,
                "",
            );
        });
        let base_url = format!("http://{address}/");
        let client = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("config"),
        );
        let transport = CampusHttpTransport::new("THYou/course-proof-test").expect("transport");
        let registry = crate::session::SessionRegistry::new();
        let csrf = registry.bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("learn-csrf").expect("csrf"),
        );
        let error = client
            .fetch_course_records(&transport, &csrf, "2025-2026-2", Some("zh"), None)
            .await
            .expect_err("login page must not be a course response");
        assert!(matches!(error, LearnClientError::SessionExpired));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn fetches_course_time_location_with_id_csrf_cookie_and_exact_route() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("course detail request");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with(
                "GET /b/kc/v_wlkc_xk_sjddb/detail?id=7001&_csrf=learn-csrf HTTP/1.1\r\n"
            ));
            assert!(request.contains("learn-session=present"));
            write_http_response(
                &mut stream,
                "application/json",
                r#"["周一 1-2 节 清华园教室"]"#,
                "",
            );
        });

        let base_url = format!("http://{address}/");
        let client = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("config"),
        );
        let transport = CampusHttpTransport::with_timeout(
            "THYou/course-time-location-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let base = Url::parse(&base_url).expect("base URL");
        transport
            .cookie_jar()
            .add_cookie_str("learn-session=present; Path=/", &base);
        let registry = crate::session::SessionRegistry::new();
        let csrf = registry.bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("learn-csrf").expect("csrf"),
        );

        assert_eq!(
            client
                .fetch_course_time_location(&transport, &csrf, "7001")
                .await
                .expect("course detail response"),
            vec!["周一 1-2 节 清华园教室"]
        );
        server.join().expect("server");
    }

    #[test]
    fn classifies_same_origin_401_and_403_as_learn_session_expiry() {
        let client = student_client();
        let endpoint = Url::parse(
            "https://learn.example.test/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/zh",
        )
        .expect("Learn endpoint");

        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            assert!(client.is_login_expired_page(status, Some(&endpoint), ""));
            let mapped = client
                .map_html_response(status, Some(&endpoint), "")
                .expect("same-origin auth failure maps");
            assert!(matches!(
                mapped.classification,
                LearnPageClassification::LoginExpired(LoginExpiredEvidence { .. })
            ));
        }

        let external = Url::parse("https://id.example.test/login").expect("external URL");
        assert!(!client.is_login_expired_page(StatusCode::FORBIDDEN, Some(&external), ""));
    }

    #[test]
    fn rejects_csrf_evidence_from_an_external_final_origin() {
        let client = student_client();
        let external = Url::parse("https://evil.example.test/after").expect("fixture URL");
        let error = client
            .map_html_response(StatusCode::OK, Some(&external), CSRF_HTML_FIXTURE)
            .expect_err("external HTML must not become an authenticated Learn page");
        assert!(matches!(error, LearnClientError::UnexpectedOrigin));
    }
}

#[cfg(test)]
#[path = "learn_lastmile_tests.rs"]
mod lastmile_tests;
