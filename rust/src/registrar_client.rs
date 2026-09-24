//! Configurable client boundaries for the academic registrar calendar service.
//!
//! The registrar has two separate concerns that are easy to accidentally mix:
//! obtaining a registrar ticket from the learning platform and reading the
//! calendar JSONP endpoint. This module keeps both request profiles explicit.
//! It also keeps JSONP transport errors separate from a valid JSON document that
//! describes an expired session or an invalid business response.

use std::fmt;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::protocol::{AcademicStage, ServiceTicket};
use crate::registrar::{CalendarWindow, RegistrarError, RegistrarProfile, split_calendar_range};
use crate::registrar_academic::{
    RegistrarGradeReport, RegistrarGradesParseError, RegistrarGradesProfile,
};
#[path = "registrar_exam.rs"]
pub mod registrar_exam;
use crate::transport::{
    CampusHttpTransport, CampusTextResponse, JsonpError, TransportError, parse_jsonp,
};
use crate::webvpn_url::{
    path_is_within_base, resolve_endpoint as resolve_mapped_endpoint, resolve_endpoint_or_absolute,
};
pub use registrar_exam::{
    RegistrarExamCourseQuery, RegistrarExamError, RegistrarExamHttpResponse,
    RegistrarExamPageProfile, RegistrarExamRecord, RegistrarExamRecordSource, RegistrarExamStage,
    RegistrarExamTerm, RegistrarVerifiedExamPageProfile, RegistrarVerifiedExamPageRequest,
    parse_exam_course_response, parse_exam_page_response, parse_verified_exam_page_html,
    parse_verified_exam_page_response,
};

/// Where an optional CSRF parameter belongs in the ALL_ZHJW exchange request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterPlacement {
    Query,
    Form,
}

/// The profile for the learning-platform request that exchanges a session for
/// an ALL_ZHJW registrar ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllZhjwTicketProfile {
    pub path: String,
    pub app_id_parameter: String,
    pub app_id: String,
    pub csrf_parameter: Option<String>,
    pub csrf_placement: ParameterPlacement,
    pub extra_query: Vec<(String, String)>,
    pub extra_form: Vec<(String, String)>,
}

impl Default for AllZhjwTicketProfile {
    fn default() -> Self {
        Self {
            path: "/b/wlxt/common/auth/gnt".to_owned(),
            app_id_parameter: "appId".to_owned(),
            app_id: "ALL_ZHJW".to_owned(),
            csrf_parameter: Some("_csrf".to_owned()),
            csrf_placement: ParameterPlacement::Query,
            extra_query: Vec::new(),
            extra_form: Vec::new(),
        }
    }
}

/// The profile used to establish the registrar-side session after obtaining a
/// ticket from the learning platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarLoginProfile {
    pub path: String,
    pub ticket_parameter: String,
    pub return_url_parameter: Option<String>,
    pub return_url: Option<String>,
    pub extra_query: Vec<(String, String)>,
}

impl Default for RegistrarLoginProfile {
    fn default() -> Self {
        Self {
            path: "/j_acegi_login.do".to_owned(),
            ticket_parameter: "ticket".to_owned(),
            return_url_parameter: Some("url".to_owned()),
            return_url: Some("/".to_owned()),
            extra_query: Vec::new(),
        }
    }
}

/// A stage-specific registrar calendar route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarStageCalendarProfile {
    pub path: String,
    pub method: String,
}

/// The wire parameter profile for the registrar JSONP endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarCalendarProfile {
    pub undergraduate: RegistrarStageCalendarProfile,
    pub graduate: RegistrarStageCalendarProfile,
    pub method_parameter: String,
    pub start_date_parameter: String,
    pub end_date_parameter: String,
    pub callback_parameter: String,
    pub extra_query: Vec<(String, String)>,
}

impl Default for RegistrarCalendarProfile {
    fn default() -> Self {
        let undergraduate = RegistrarProfile::new(AcademicStage::Undergraduate);
        let graduate = RegistrarProfile::new(AcademicStage::Graduate);

        Self {
            undergraduate: RegistrarStageCalendarProfile {
                path: undergraduate.calendar_path().to_owned(),
                method: undergraduate.calendar_method().to_owned(),
            },
            graduate: RegistrarStageCalendarProfile {
                path: graduate.calendar_path().to_owned(),
                method: graduate.calendar_method().to_owned(),
            },
            method_parameter: "m".to_owned(),
            start_date_parameter: "p_start_date".to_owned(),
            end_date_parameter: "p_end_date".to_owned(),
            callback_parameter: "jsoncallback".to_owned(),
            extra_query: Vec::new(),
        }
    }
}

/// All endpoints and wire parameter names used by RegistrarClient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarClientConfig {
    pub user_agent: String,
    pub learn_base_url: String,
    pub registrar_base_url: String,
    pub ticket_exchange: AllZhjwTicketProfile,
    pub login: RegistrarLoginProfile,
    pub calendar: RegistrarCalendarProfile,
}

impl Default for RegistrarClientConfig {
    fn default() -> Self {
        Self {
            user_agent: "THYou/0.1".to_owned(),
            learn_base_url: "https://learn.tsinghua.edu.cn".to_owned(),
            registrar_base_url: "https://zhjw.cic.tsinghua.edu.cn".to_owned(),
            ticket_exchange: AllZhjwTicketProfile::default(),
            login: RegistrarLoginProfile::default(),
            calendar: RegistrarCalendarProfile::default(),
        }
    }
}

impl RegistrarClientConfig {
    fn validate(&self) -> Result<(), RegistrarClientError> {
        require_non_empty(&self.user_agent, "user agent")?;
        validate_base_url(&self.learn_base_url, "learning-platform base URL")?;
        validate_base_url(&self.registrar_base_url, "registrar base URL")?;

        require_non_empty(&self.ticket_exchange.path, "ticket exchange path")?;
        require_non_empty(
            &self.ticket_exchange.app_id_parameter,
            "ticket exchange app-id parameter",
        )?;
        require_non_empty(&self.ticket_exchange.app_id, "ticket exchange app id")?;
        validate_optional_parameter(
            self.ticket_exchange.csrf_parameter.as_deref(),
            "ticket exchange CSRF parameter",
        )?;
        validate_pairs(
            &self.ticket_exchange.extra_query,
            "ticket exchange query parameter",
        )?;
        validate_pairs(
            &self.ticket_exchange.extra_form,
            "ticket exchange form parameter",
        )?;

        require_non_empty(&self.login.path, "registrar login path")?;
        require_non_empty(&self.login.ticket_parameter, "registrar ticket parameter")?;
        validate_optional_parameter(
            self.login.return_url_parameter.as_deref(),
            "registrar return-url parameter",
        )?;
        validate_pairs(&self.login.extra_query, "registrar login query parameter")?;

        require_non_empty(&self.calendar.method_parameter, "calendar method parameter")?;
        require_non_empty(
            &self.calendar.start_date_parameter,
            "calendar start-date parameter",
        )?;
        require_non_empty(
            &self.calendar.end_date_parameter,
            "calendar end-date parameter",
        )?;
        require_non_empty(
            &self.calendar.callback_parameter,
            "calendar callback parameter",
        )?;
        validate_stage_calendar_profile(&self.calendar.undergraduate, "undergraduate")?;
        validate_stage_calendar_profile(&self.calendar.graduate, "graduate")?;
        validate_pairs(&self.calendar.extra_query, "calendar query parameter")?;

        Ok(())
    }
}

/// A planned POST form request to the learning platform's ALL_ZHJW endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct AllZhjwTicketExchangePlan {
    pub method: Method,
    pub url: String,
    pub query: Vec<(String, String)>,
    pub form: Vec<(String, String)>,
}

/// A planned GET request that passes the exchanged ticket to the registrar.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistrarLoginRequestPlan {
    pub method: Method,
    pub url: String,
    pub query: Vec<(String, String)>,
}

/// A planned JSONP request for one inclusive calendar window.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistrarCalendarRequestPlan {
    pub method: Method,
    pub stage: AcademicStage,
    pub window: CalendarWindow,
    pub url: String,
    pub query: Vec<(String, String)>,
    pub callback: String,
}

impl fmt::Debug for AllZhjwTicketExchangePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AllZhjwTicketExchangePlan")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("query", &RedactedParameters(&self.query))
            .field("form", &RedactedParameters(&self.form))
            .finish()
    }
}

impl fmt::Debug for RegistrarLoginRequestPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistrarLoginRequestPlan")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("query", &RedactedParameters(&self.query))
            .finish()
    }
}

impl fmt::Debug for RegistrarCalendarRequestPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistrarCalendarRequestPlan")
            .field("method", &self.method)
            .field("stage", &self.stage)
            .field("window", &self.window)
            .field("url", &self.url)
            .field("query", &RedactedParameters(&self.query))
            .field("callback", &self.callback)
            .finish()
    }
}

struct RedactedParameters<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedParameters<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|(name, value)| {
                let value = if is_sensitive_parameter(name) {
                    "[redacted]"
                } else {
                    value.as_str()
                };
                (name, value)
            }))
            .finish()
    }
}

fn is_sensitive_parameter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("csrf")
        || name.contains("pass")
        || name.contains("ticket")
        || name.contains("token")
}

/// A stable calendar record produced from one registrar JSONP response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarCalendarRecord {
    pub stage: AcademicStage,
    pub window: CalendarWindow,
    pub events: Vec<RegistrarEvent>,
}

/// A stable local-time event extracted from a registrar calendar item.
///
/// The registrar payload does not carry a reliable timezone contract. Keeping
/// the values as NaiveDateTime prevents the adapter from silently converting a
/// Beijing wall-clock time into the host machine's timezone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarEvent {
    pub id: Option<String>,
    pub title: String,
    pub starts_at: NaiveDateTime,
    pub ends_at: NaiveDateTime,
    pub location: Option<String>,
    pub category: Option<String>,
    pub course_code: Option<String>,
    pub instructor: Option<String>,
}

impl RegistrarEvent {
    pub fn date(&self) -> NaiveDate {
        self.starts_at.date()
    }

    pub fn start_time(&self) -> NaiveTime {
        self.starts_at.time()
    }

    pub fn end_time(&self) -> NaiveTime {
        self.ends_at.time()
    }
}

/// A structured error for a valid JSON document that is not a usable registrar
/// calendar payload.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrarPayloadError {
    #[error("calendar payload is not an event array")]
    UnsupportedShape,

    #[error("calendar event at index {index} is not an object")]
    EventNotObject { index: usize },

    #[error("calendar event at index {index} is missing a title")]
    MissingTitle { index: usize },

    /// Kept for compatibility with older callers. Current Registrar calendar
    /// items do not guarantee `grrlID`; a complete item may therefore have no
    /// id and is still accepted by the parser.
    #[error("calendar event at index {index} is missing its server event id")]
    MissingEventId { index: usize },

    #[error("calendar event at index {index} is missing a start date or time")]
    MissingStartTime { index: usize },

    #[error("calendar event at index {index} is missing an end date or time")]
    MissingEndTime { index: usize },

    #[error("calendar event at index {index} has an invalid {field} value {value:?}")]
    InvalidDateTime {
        index: usize,
        field: String,
        value: String,
    },

    #[error("calendar event at index {index} ends no later than it starts")]
    EndNotAfterStart { index: usize },

    #[error("registrar returned a business failure: {message}")]
    BusinessFailure { message: String },
}

/// The response shapes observed for the learning-platform ALL_ZHJW exchange.
///
/// The ticket itself stays opaque.  The parser accepts only a non-empty
/// one-line value or a JSON string containing that value; it does not extract
/// arbitrary text from HTML, objects, or error pages.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AllZhjwTicketResponseError {
    #[error("the ALL_ZHJW response is empty")]
    Empty,

    #[error("the quoted ALL_ZHJW response is malformed")]
    InvalidQuoted,

    #[error("the ALL_ZHJW response is not a plain or quoted ticket")]
    UnsupportedShape,
}

/// Parses the raw response from the learning-platform ALL_ZHJW exchange.
///
/// Recent deployments have returned either a plain ticket or the same ticket
/// surrounded by JSON-style double quotes.  Both forms are kept explicit here
/// so a login page or an unrelated JSON object cannot be mistaken for a
/// registrar credential.  The returned value is never URL-decoded or otherwise
/// transformed.
pub fn parse_all_zhjw_ticket_response(
    body: &str,
) -> Result<ServiceTicket, AllZhjwTicketResponseError> {
    let response = body.trim();
    if response.is_empty() {
        return Err(AllZhjwTicketResponseError::Empty);
    }

    let ticket = match (response.starts_with('"'), response.ends_with('"')) {
        (true, true) => serde_json::from_str::<String>(response)
            .map_err(|_| AllZhjwTicketResponseError::InvalidQuoted)?,
        (false, false) => response.to_owned(),
        _ => return Err(AllZhjwTicketResponseError::InvalidQuoted),
    };

    if ticket.trim().is_empty()
        || ticket.chars().any(char::is_whitespace)
        || ticket.chars().any(char::is_control)
        || ticket.starts_with('<')
        || ticket.starts_with('{')
        || ticket.starts_with('[')
        || looks_like_ticket_error(&ticket)
    {
        return Err(AllZhjwTicketResponseError::UnsupportedShape);
    }

    ServiceTicket::new(ticket).ok_or(AllZhjwTicketResponseError::Empty)
}

/// Errors at the boundary between transport, authentication state and business
/// payload decoding.
#[derive(Debug, Error)]
pub enum RegistrarClientError {
    #[error("registrar client configuration is invalid: {message}")]
    InvalidConfig { message: String },

    #[error("the ALL_ZHJW ticket must not be empty")]
    EmptyTicket,

    #[error("the calendar callback must not be empty")]
    EmptyCallback,

    #[error("registrar authentication or session has expired: {reason}")]
    LoginExpired { reason: String },

    #[error("registrar response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("registrar JSONP is invalid: {source}")]
    InvalidJsonp { source: JsonpError },

    #[error("registrar business payload is invalid: {source}")]
    InvalidBusinessPayload { source: RegistrarPayloadError },

    #[error("registrar grade response is invalid: {source}")]
    InvalidGradeResponse { source: RegistrarGradesParseError },

    #[error("registrar examination response is invalid: {source}")]
    InvalidExamResponse { source: RegistrarExamError },

    #[error("registrar grade response must be HTML")]
    InvalidGradeContentType,

    #[error("registrar calendar JSONP response must not be HTML")]
    CalendarHtmlContentType,

    #[error("registrar calendar JSONP response has an unsupported content type")]
    InvalidCalendarContentType,

    #[error("ALL_ZHJW ticket response is invalid: {source}")]
    InvalidTicketResponse { source: AllZhjwTicketResponseError },

    #[error(transparent)]
    Calendar(#[from] RegistrarError),

    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// A client that owns the cookie-aware transport and the registrar request
/// profiles. The client does not invent a callback; callers pass one for each
/// calendar request and the same value is checked while decoding the response.
#[derive(Clone)]
pub struct RegistrarClient {
    transport: CampusHttpTransport,
    config: RegistrarClientConfig,
}

impl RegistrarClient {
    pub fn new(config: RegistrarClientConfig) -> Result<Self, RegistrarClientError> {
        config.validate()?;
        let transport = CampusHttpTransport::new(&config.user_agent)?;
        Ok(Self { transport, config })
    }

    pub fn from_transport(
        config: RegistrarClientConfig,
        transport: CampusHttpTransport,
    ) -> Result<Self, RegistrarClientError> {
        config.validate()?;
        Ok(Self { transport, config })
    }

    pub fn config(&self) -> &RegistrarClientConfig {
        &self.config
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Plans the POST form used to request an ALL_ZHJW ticket from the learning
    /// platform. A CSRF value is optional at this layer because some deployed
    /// profiles put it in the session or inject it through a different adapter.
    pub fn all_zhjw_ticket_exchange_plan(
        &self,
        csrf_token: Option<&str>,
    ) -> Result<AllZhjwTicketExchangePlan, RegistrarClientError> {
        let profile = &self.config.ticket_exchange;
        let url = resolve_endpoint(&self.config.learn_base_url, &profile.path)?;
        let mut query = profile.extra_query.clone();
        let mut form = profile.extra_form.clone();

        form.insert(
            0,
            (profile.app_id_parameter.clone(), profile.app_id.clone()),
        );

        if let (Some(parameter), Some(token)) =
            (profile.csrf_parameter.as_deref(), non_empty(csrf_token))
        {
            let pair = (parameter.to_owned(), token.to_owned());
            match profile.csrf_placement {
                ParameterPlacement::Query => query.push(pair),
                ParameterPlacement::Form => form.push(pair),
            }
        }

        Ok(AllZhjwTicketExchangePlan {
            method: Method::POST,
            url,
            query,
            form,
        })
    }

    /// Executes the configured ALL_ZHJW exchange and returns its raw response.
    /// Callers should pass the result through
    /// [`parse_all_zhjw_ticket_response`] before using it as a registrar
    /// credential.
    pub async fn exchange_all_zhjw_ticket(
        &self,
        csrf_token: Option<&str>,
    ) -> Result<String, RegistrarClientError> {
        let plan = self.all_zhjw_ticket_exchange_plan(csrf_token)?;
        let endpoint = append_query(&plan.url, &plan.query)?;
        let expected_url =
            Url::parse(&endpoint).map_err(|error| RegistrarClientError::InvalidConfig {
                message: format!("ticket exchange URL is invalid: {error}"),
            })?;
        let response = self
            .transport
            .send(self.transport.client().post(endpoint).form(&plan.form))
            .await
            .map_err(|error| RegistrarClientError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let redirect_location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| RegistrarClientError::Transport(TransportError::Decode(error)))?;
        if http_status_is_authentication_failure(status)
            || looks_like_login_response(&final_url, content_type.as_deref(), &body)
            || redirect_points_to_login(&final_url, redirect_location.as_deref())
        {
            return Err(RegistrarClientError::LoginExpired {
                reason: "learning session could not obtain the Registrar ticket".to_owned(),
            });
        }
        if !registrar_origin_is_allowed(&self.config.learn_base_url, &final_url)? {
            return Err(RegistrarClientError::UnexpectedOrigin);
        }
        if final_url.path() != expected_url.path() {
            return Err(RegistrarClientError::InvalidConfig {
                message: "ticket exchange response ended outside the requested route".to_owned(),
            });
        }
        if final_url.query() != expected_url.query() {
            return Err(RegistrarClientError::InvalidConfig {
                message: "ticket exchange response did not retain the confirmed query".to_owned(),
            });
        }
        if status != StatusCode::OK {
            return Err(RegistrarClientError::Transport(
                TransportError::HttpStatus { status, body },
            ));
        }
        Ok(body)
    }

    /// Plans the registrar-side login request that consumes an ALL_ZHJW ticket.
    pub fn registrar_login_request_plan(
        &self,
        ticket: &str,
    ) -> Result<RegistrarLoginRequestPlan, RegistrarClientError> {
        if !valid_ticket_parameter(ticket) {
            return Err(RegistrarClientError::EmptyTicket);
        }

        let profile = &self.config.login;
        let url = resolve_endpoint(&self.config.registrar_base_url, &profile.path)?;
        let mut query = profile.extra_query.clone();
        if let (Some(parameter), Some(value)) = (
            profile.return_url_parameter.as_deref(),
            profile.return_url.as_deref(),
        ) {
            query.push((parameter.to_owned(), value.to_owned()));
        }
        query.push((profile.ticket_parameter.clone(), ticket.to_owned()));

        Ok(RegistrarLoginRequestPlan {
            method: Method::GET,
            url,
            query,
        })
    }

    /// Executes the configured registrar-side login request and returns the raw
    /// response page. The caller can inspect the redirect/session outcome with
    /// the authentication adapter that owns that policy.
    pub async fn establish_registrar_session(
        &self,
        ticket: &str,
    ) -> Result<String, RegistrarClientError> {
        let plan = self.registrar_login_request_plan(ticket)?;
        let endpoint = append_query(&plan.url, &plan.query)?;
        let response = self.transport.get_text_response(endpoint.as_str()).await?;
        if http_status_is_authentication_failure(response.status)
            || looks_like_login_response(
                &response.final_url,
                response.content_type.as_deref(),
                &response.body,
            )
            || redirect_points_to_login(&response.final_url, response.redirect_location.as_deref())
        {
            return Err(RegistrarClientError::LoginExpired {
                reason: "registrar ticket login returned a login or timeout page".to_owned(),
            });
        }
        if !registrar_origin_is_allowed(&self.config.registrar_base_url, &response.final_url)? {
            return Err(RegistrarClientError::UnexpectedOrigin);
        }
        if response.status != StatusCode::OK {
            return Err(RegistrarClientError::Transport(
                TransportError::HttpStatus {
                    status: response.status,
                    body: response.body,
                },
            ));
        }
        Ok(response.body)
    }

    /// Plans one JSONP request for an inclusive calendar window.
    pub fn calendar_request_plan<C>(
        &self,
        stage: AcademicStage,
        window: CalendarWindow,
        callback: C,
    ) -> Result<RegistrarCalendarRequestPlan, RegistrarClientError>
    where
        C: Into<String>,
    {
        window.validate()?;
        let callback = callback.into();
        if callback.trim().is_empty() || !valid_jsonp_callback(&callback) {
            return Err(RegistrarClientError::InvalidConfig {
                message: "calendar callback must be a JavaScript identifier path".to_owned(),
            });
        }

        let stage_profile = match stage {
            AcademicStage::Undergraduate => &self.config.calendar.undergraduate,
            AcademicStage::Graduate => &self.config.calendar.graduate,
        };
        let url = resolve_endpoint(&self.config.registrar_base_url, &stage_profile.path)?;
        let mut query = self.config.calendar.extra_query.clone();
        query.push((
            self.config.calendar.method_parameter.clone(),
            stage_profile.method.clone(),
        ));
        query.push((
            self.config.calendar.start_date_parameter.clone(),
            window.start.format("%Y%m%d").to_string(),
        ));
        query.push((
            self.config.calendar.end_date_parameter.clone(),
            window.end.format("%Y%m%d").to_string(),
        ));
        query.push((
            self.config.calendar.callback_parameter.clone(),
            callback.clone(),
        ));

        Ok(RegistrarCalendarRequestPlan {
            method: Method::GET,
            stage,
            window,
            url,
            query,
            callback,
        })
    }

    /// Splits an inclusive range into the registrar's 28-day windows and plans
    /// one JSONP request for each window.
    pub fn calendar_request_plans<C>(
        &self,
        stage: AcademicStage,
        start: NaiveDate,
        end: NaiveDate,
        callback: C,
    ) -> Result<Vec<RegistrarCalendarRequestPlan>, RegistrarClientError>
    where
        C: Into<String>,
    {
        let callback = callback.into();
        let windows = split_calendar_range(start, end)?;
        windows
            .into_iter()
            .map(|window| self.calendar_request_plan(stage, window, callback.clone()))
            .collect()
    }

    /// Fetches and decodes one JSONP calendar window.
    pub async fn fetch_calendar_window(
        &self,
        stage: AcademicStage,
        window: CalendarWindow,
        callback: &str,
    ) -> Result<RegistrarCalendarRecord, RegistrarClientError> {
        let plan = self.calendar_request_plan(stage, window, callback.to_owned())?;
        let endpoint = append_query(&plan.url, &plan.query)?;
        let response = self.transport.get_text_response(&endpoint).await?;
        if http_status_is_authentication_failure(response.status)
            || registrar_read_response_is_login_page(
                &response.final_url,
                response.content_type.as_deref(),
                &response.body,
            )
            || redirect_points_to_login(&response.final_url, response.redirect_location.as_deref())
        {
            return Err(RegistrarClientError::LoginExpired {
                reason: "registrar session is not authorized for the calendar request".to_owned(),
            });
        }
        if !registrar_origin_is_allowed(&self.config.registrar_base_url, &response.final_url)? {
            return Err(RegistrarClientError::UnexpectedOrigin);
        }
        decode_calendar_response(
            &self.config.registrar_base_url,
            response,
            &plan.callback,
            plan.stage,
            plan.window,
            &plan.url,
            &plan.query,
        )
    }

    /// Fetches and decodes every 28-day window in an inclusive range.
    pub async fn fetch_calendar_range(
        &self,
        stage: AcademicStage,
        start: NaiveDate,
        end: NaiveDate,
        callback: &str,
    ) -> Result<Vec<RegistrarCalendarRecord>, RegistrarClientError> {
        let plans = self.calendar_request_plans(stage, start, end, callback.to_owned())?;
        let mut records = Vec::with_capacity(plans.len());
        for plan in plans {
            let endpoint = append_query(&plan.url, &plan.query)?;
            let response = self.transport.get_text_response(&endpoint).await?;
            if http_status_is_authentication_failure(response.status)
                || registrar_read_response_is_login_page(
                    &response.final_url,
                    response.content_type.as_deref(),
                    &response.body,
                )
                || redirect_points_to_login(
                    &response.final_url,
                    response.redirect_location.as_deref(),
                )
            {
                return Err(RegistrarClientError::LoginExpired {
                    reason: "registrar session is not authorized for the calendar request"
                        .to_owned(),
                });
            }
            if !registrar_origin_is_allowed(&self.config.registrar_base_url, &response.final_url)? {
                return Err(RegistrarClientError::UnexpectedOrigin);
            }
            records.push(decode_calendar_response(
                &self.config.registrar_base_url,
                response,
                &plan.callback,
                plan.stage,
                plan.window,
                &plan.url,
                &plan.query,
            )?);
        }
        Ok(records)
    }

    /// Fetches and verifies one stage-specific Registrar grade report through
    /// this client's existing Cookie jar.  The academic profile owns only the
    /// confirmed route and query parameters; this method owns the HTTP origin,
    /// session reuse, login-page classification, and strict HTML proof.
    pub async fn fetch_grades(
        &self,
        profile: RegistrarGradesProfile,
    ) -> Result<RegistrarGradeReport, RegistrarClientError> {
        let request =
            profile
                .request()
                .map_err(|source| RegistrarClientError::InvalidGradeResponse {
                    source: source.into(),
                })?;
        let endpoint = resolve_endpoint(&self.config.registrar_base_url, &request.path)?;
        let query = request
            .query
            .iter()
            .map(|parameter| (parameter.name.clone(), parameter.value.clone()))
            .collect::<Vec<_>>();
        let endpoint = append_query(&endpoint, &query)?;
        let expected_url =
            Url::parse(&endpoint).map_err(|error| RegistrarClientError::InvalidConfig {
                message: format!("grade request URL is invalid: {error}"),
            })?;
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| RegistrarClientError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let redirect_location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| RegistrarClientError::Transport(TransportError::Decode(error)))?;

        if http_status_is_authentication_failure(status) {
            return Err(RegistrarClientError::LoginExpired {
                reason: "registrar session is not authorized for the grade request".to_owned(),
            });
        }
        // A same-origin redirect to j_acegi_login.do is the normal legacy
        // session-expiry shape.  Classify it before checking the academic
        // route and query; otherwise a perfectly useful auth error is
        // reported as route drift because the login page has different
        // parameters.
        if registrar_read_response_is_login_page(&final_url, content_type.as_deref(), &body)
            || redirect_points_to_login(&final_url, redirect_location.as_deref())
        {
            return Err(RegistrarClientError::LoginExpired {
                reason: "registrar grade request returned a login or timeout page".to_owned(),
            });
        }
        if !registrar_origin_is_allowed(&self.config.registrar_base_url, &final_url)? {
            return Err(RegistrarClientError::UnexpectedOrigin);
        }
        if final_url.path() != expected_url.path() {
            return Err(RegistrarClientError::InvalidConfig {
                message: "grade response ended outside the requested route".to_owned(),
            });
        }
        if final_url.query() != expected_url.query() {
            return Err(RegistrarClientError::InvalidConfig {
                message: "grade response did not retain the confirmed query".to_owned(),
            });
        }
        if status != StatusCode::OK {
            return Err(RegistrarClientError::Transport(
                TransportError::HttpStatus { status, body },
            ));
        }
        if !content_type
            .as_deref()
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/html"))
        {
            return Err(RegistrarClientError::InvalidGradeContentType);
        }

        profile
            .parse_html(&body)
            .map_err(|source| RegistrarClientError::InvalidGradeResponse { source })
    }

    /// Fetches and verifies the confirmed undergraduate examination page.
    /// Graduate requests remain unavailable because no current public
    /// implementation has supplied an independently verified graduate route.
    pub async fn fetch_exam_page(&self) -> Result<Vec<RegistrarExamRecord>, RegistrarClientError> {
        self.fetch_verified_exam_page(RegistrarExamStage::Undergraduate)
            .await
    }

    /// Fetches the independently evidenced undergraduate examination page.
    ///
    /// This method accepts an explicit academic stage. The verified page uses
    /// the exact `url=/jxmh.do&m=bks_ksSearch` request and requires the fixed
    /// nine-column HTML table before it can produce a record.
    pub async fn fetch_verified_exam_page(
        &self,
        stage: RegistrarExamStage,
    ) -> Result<Vec<RegistrarExamRecord>, RegistrarClientError> {
        let profile = RegistrarVerifiedExamPageProfile::for_stage(stage)
            .map_err(|source| RegistrarClientError::InvalidExamResponse { source })?;
        let request = profile.request();
        let endpoint = request
            .endpoint_url(&self.config.registrar_base_url)
            .map_err(|source| RegistrarClientError::InvalidExamResponse { source })?;
        let response = self.transport.get_text_response(endpoint.as_str()).await?;
        if matches!(response.status.as_u16(), 301 | 302 | 303 | 307 | 308)
            && redirect_points_to_login(&response.final_url, response.redirect_location.as_deref())
        {
            return Err(RegistrarClientError::LoginExpired {
                reason: "registrar examination read redirected to login".to_owned(),
            });
        }
        let captured = RegistrarExamHttpResponse::new(
            response.status,
            response.final_url,
            response.content_type,
            response.body,
        );
        parse_verified_exam_page_response(&self.config.registrar_base_url, &request, &captured)
            .map_err(|source| RegistrarClientError::InvalidExamResponse { source })
    }

    /// Fetches and verifies one undergraduate per-course examination JSONP
    /// response through the same Registrar Cookie jar.
    pub async fn fetch_exam_course(
        &self,
        query: &RegistrarExamCourseQuery,
    ) -> Result<Vec<RegistrarExamRecord>, RegistrarClientError> {
        let request = query.request();
        let endpoint = request
            .endpoint_url(&self.config.registrar_base_url)
            .map_err(|source| RegistrarClientError::InvalidExamResponse { source })?;
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| RegistrarClientError::Transport(TransportError::Request(error)))?;
        let captured = capture_exam_response(response).await?;
        parse_exam_course_response(&self.config.registrar_base_url, &request, &captured)
            .map_err(|source| RegistrarClientError::InvalidExamResponse { source })
    }
}

async fn capture_exam_response(
    response: reqwest::Response,
) -> Result<RegistrarExamHttpResponse, RegistrarClientError> {
    let status = response.status();
    let final_url = response.url().clone();
    if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
        && redirect_points_to_login(
            &final_url,
            response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
        )
    {
        return Err(RegistrarClientError::LoginExpired {
            reason: "registrar examination read redirected to login".to_owned(),
        });
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = crate::telemetry::timing::read_text(response)
        .await
        .map_err(|error| RegistrarClientError::Transport(TransportError::Decode(error)))?;
    Ok(RegistrarExamHttpResponse::new(
        status,
        final_url,
        content_type,
        body,
    ))
}

fn decode_calendar_response(
    registrar_base_url: &str,
    response: CampusTextResponse,
    expected_callback: &str,
    stage: AcademicStage,
    window: CalendarWindow,
    expected_endpoint: &str,
    expected_query: &[(String, String)],
) -> Result<RegistrarCalendarRecord, RegistrarClientError> {
    if http_status_is_authentication_failure(response.status) {
        return Err(RegistrarClientError::LoginExpired {
            reason: "registrar session is not authorized for the calendar request".to_owned(),
        });
    }

    if registrar_read_response_is_login_page(
        &response.final_url,
        response.content_type.as_deref(),
        &response.body,
    ) {
        return Err(RegistrarClientError::LoginExpired {
            reason: "registrar returned a login or timeout page instead of calendar JSONP"
                .to_owned(),
        });
    }
    if redirect_points_to_login(&response.final_url, response.redirect_location.as_deref()) {
        return Err(RegistrarClientError::LoginExpired {
            reason: "registrar returned a login or timeout page instead of calendar JSONP"
                .to_owned(),
        });
    }
    if !registrar_origin_is_allowed(registrar_base_url, &response.final_url)? {
        return Err(RegistrarClientError::UnexpectedOrigin);
    }

    let expected_path = Url::parse(expected_endpoint)
        .map_err(|error| RegistrarClientError::InvalidConfig {
            message: format!("calendar endpoint is invalid: {error}"),
        })?
        .path()
        .to_owned();
    if response.final_url.path() != expected_path {
        return Err(RegistrarClientError::InvalidConfig {
            message: "calendar response ended outside the requested route".to_owned(),
        });
    }

    // Query retention is checked at the response boundary, after the
    // login-page classification above, so a session redirect is never
    // mislabeled as a malformed calendar route.
    let expected_url =
        Url::parse(&append_query(expected_endpoint, expected_query)?).map_err(|error| {
            RegistrarClientError::InvalidConfig {
                message: format!("calendar request URL is invalid: {error}"),
            }
        })?;
    if response.final_url.query() != expected_url.query() {
        return Err(RegistrarClientError::InvalidConfig {
            message: "calendar response did not retain the confirmed query".to_owned(),
        });
    }

    if response.status != StatusCode::OK {
        return Err(RegistrarClientError::Transport(
            TransportError::HttpStatus {
                status: response.status,
                body: response.body,
            },
        ));
    }

    validate_calendar_jsonp_content_type(response.content_type.as_deref())?;

    decode_calendar_jsonp(&response.body, expected_callback, stage, window)
}

/// Decodes one registrar JSONP fixture or response through the shared
/// transport decoder, then maps its business payload into stable Rust models.
pub fn decode_calendar_jsonp(
    body: &str,
    expected_callback: &str,
    stage: AcademicStage,
    window: CalendarWindow,
) -> Result<RegistrarCalendarRecord, RegistrarClientError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        let payload = match parse_jsonp::<Value>(body, expected_callback) {
            Ok(payload) => payload,
            Err(TransportError::Jsonp(source)) => {
                if looks_like_login_page(body) {
                    return Err(RegistrarClientError::LoginExpired {
                        reason: "registrar returned a login page instead of calendar JSONP"
                            .to_owned(),
                    });
                }
                return Err(RegistrarClientError::InvalidJsonp { source });
            }
            Err(source) => return Err(RegistrarClientError::Transport(source)),
        };

        if let Some(reason) = login_expired_reason(&payload) {
            return Err(RegistrarClientError::LoginExpired { reason });
        }

        if let Some(message) = business_failure_reason(&payload) {
            return Err(RegistrarClientError::InvalidBusinessPayload {
                source: RegistrarPayloadError::BusinessFailure { message },
            });
        }

        let values = collect_event_values(&payload).map_err(invalid_business_payload)?;
        let events = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| parse_event(value, index))
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid_business_payload)?;

        Ok(RegistrarCalendarRecord {
            stage,
            window,
            events,
        })
    })
}

fn invalid_business_payload(source: RegistrarPayloadError) -> RegistrarClientError {
    RegistrarClientError::InvalidBusinessPayload { source }
}

fn validate_calendar_jsonp_content_type(value: Option<&str>) -> Result<(), RegistrarClientError> {
    let Some(value) = value else {
        // Some legacy Registrar responses omit Content-Type. The strict JSONP
        // envelope and payload parser remains authoritative in that case.
        return Ok(());
    };

    let media_type = value
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if media_type == "text/html" {
        return Err(RegistrarClientError::CalendarHtmlContentType);
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
    Err(RegistrarClientError::InvalidCalendarContentType)
}

/// Extracts an explicit business failure envelope without treating a normal
/// calendar array or an unrecognized object as success.  The current
/// calendar endpoint normally returns a bare array; failure envelopes seen in
/// legacy deployments use `success`, `status`, `result`, `error`, `message`,
/// or `msg` fields.  A failure is classified only when one of those fields
/// carries an explicit negative/error value.
fn business_failure_reason(value: &Value) -> Option<String> {
    let Value::Object(object) = value else {
        return None;
    };

    let explicit_failure = object.get("success").is_some_and(negative_flag)
        || object.get("ok").is_some_and(negative_flag)
        || object.get("status").is_some_and(status_failure)
        || object.get("result").is_some_and(status_failure)
        || ["code", "statusCode", "status_code", "httpCode"]
            .iter()
            .any(|field| object.get(*field).is_some_and(http_failure))
        || object.get("error").is_some_and(|value| {
            !value.is_null() && !value.as_str().is_some_and(|value| value.trim().is_empty())
        });

    explicit_failure.then(|| "registrar calendar business request failed".to_owned())
}

fn negative_flag(value: &Value) -> bool {
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

fn status_failure(value: &Value) -> bool {
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

fn http_failure(value: &Value) -> bool {
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

fn require_non_empty(value: &str, field: &str) -> Result<(), RegistrarClientError> {
    if value.trim().is_empty() {
        return Err(RegistrarClientError::InvalidConfig {
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

fn validate_optional_parameter(
    value: Option<&str>,
    field: &str,
) -> Result<(), RegistrarClientError> {
    if let Some(value) = value {
        require_non_empty(value, field)?;
    }
    Ok(())
}

fn validate_pairs(pairs: &[(String, String)], field: &str) -> Result<(), RegistrarClientError> {
    for (key, _) in pairs {
        require_non_empty(key, field)?;
    }
    Ok(())
}

fn validate_stage_calendar_profile(
    profile: &RegistrarStageCalendarProfile,
    stage: &str,
) -> Result<(), RegistrarClientError> {
    require_non_empty(&profile.path, &format!("{stage} calendar path"))?;
    require_non_empty(&profile.method, &format!("{stage} calendar method"))?;
    Ok(())
}

fn validate_base_url(value: &str, field: &str) -> Result<(), RegistrarClientError> {
    let url = Url::parse(value).map_err(|error| RegistrarClientError::InvalidConfig {
        message: format!("{field} is invalid: {error}"),
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RegistrarClientError::InvalidConfig {
            message: format!(
                "{field} must be an http(s) origin without userinfo, query, or fragment"
            ),
        });
    }
    Ok(())
}

fn resolve_endpoint(base: &str, path_or_url: &str) -> Result<String, RegistrarClientError> {
    let base = Url::parse(base).map_err(|error| RegistrarClientError::InvalidConfig {
        message: format!("base URL is invalid: {error}"),
    })?;
    let url = if Url::parse(path_or_url).is_ok() {
        resolve_endpoint_or_absolute(&base, path_or_url)
    } else {
        resolve_mapped_endpoint(&base, path_or_url)
    }
    .map_err(|error| RegistrarClientError::InvalidConfig {
        message: format!("endpoint is invalid: {error}"),
    })?;
    Ok(url.to_string())
}

pub(crate) fn registrar_origin_is_allowed(
    base: &str,
    final_url: &Url,
) -> Result<bool, RegistrarClientError> {
    let base_url = Url::parse(base).map_err(|error| RegistrarClientError::InvalidConfig {
        message: format!("base URL is invalid: {error}"),
    })?;
    Ok(same_registrar_origin(&base_url, final_url) && path_is_within_base(&base_url, final_url))
}

fn same_registrar_origin(base: &Url, candidate: &Url) -> bool {
    let is_registrar_downgrade = base.scheme() == "https"
        && candidate.scheme() == "http"
        && base.host_str() == Some("zhjw.cic.tsinghua.edu.cn");
    let same_host = base.host_str() == candidate.host_str()
        && candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.fragment().is_none();
    let same_port = base.port_or_known_default() == candidate.port_or_known_default()
        || (is_registrar_downgrade
            && base.port_or_known_default() == Some(443)
            && candidate.port_or_known_default() == Some(80));
    if !same_host || !same_port {
        return false;
    }
    base.scheme() == candidate.scheme() || is_registrar_downgrade
}

fn append_query(
    endpoint: &str,
    query: &[(String, String)],
) -> Result<String, RegistrarClientError> {
    let mut url = Url::parse(endpoint).map_err(|error| RegistrarClientError::InvalidConfig {
        message: format!("request URL is invalid: {error}"),
    })?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

fn valid_ticket_parameter(value: &str) -> bool {
    !value.trim().is_empty()
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}

fn http_status_is_authentication_failure(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

fn looks_like_ticket_error(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "error"
            | "failed"
            | "failure"
            | "invalid"
            | "login"
            | "timeout"
            | "unauthorized"
            | "未登录"
            | "请先登录"
            | "登录失败"
            | "认证失败"
            | "系统错误"
            | "登录超时"
    ) || [
        "authentication required",
        "login required",
        "session expired",
        "session timeout",
        "request failed",
        "authentication failed",
        "sso",
        "webvpn",
        "ticket not found",
        "service ticket",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn valid_jsonp_callback(callback: &str) -> bool {
    if callback.is_empty() {
        return false;
    }

    callback.split('.').all(|segment| {
        let mut characters = segment.chars();
        let Some(first) = characters.next() else {
            return false;
        };
        if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
            return false;
        }
        characters.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    })
}

const WRAPPER_FIELDS: &[&str] = &[
    "data", "result", "rows", "records", "events", "items", "list", "calendar",
];

const TITLE_FIELDS: &[&str] = &[
    "nr",
    "title",
    "name",
    "courseName",
    "course_name",
    "kcmc",
    "course",
    "subject",
];

const DATE_FIELDS: &[&str] = &["nq", "date", "rq", "eventDate", "event_date", "day"];

const START_TIME_FIELDS: &[&str] = &["kssj", "startTime", "start_time", "beginTime", "begin_time"];

const END_TIME_FIELDS: &[&str] = &["jssj", "endTime", "end_time", "finishTime", "stopTime"];

const START_DATETIME_FIELDS: &[&str] = &["start", "startsAt", "starts_at", "begin"];

const END_DATETIME_FIELDS: &[&str] = &["end", "endsAt", "ends_at", "finish", "stop"];

fn collect_event_values(value: &Value) -> Result<Vec<&Value>, RegistrarPayloadError> {
    match value {
        Value::Array(items) => Ok(items.iter().collect()),
        _ => Err(RegistrarPayloadError::UnsupportedShape),
    }
}

fn parse_event(value: &Value, index: usize) -> Result<RegistrarEvent, RegistrarPayloadError> {
    let object = value
        .as_object()
        .ok_or(RegistrarPayloadError::EventNotObject { index })?;

    let title = first_text(object, TITLE_FIELDS)
        .map(|(_, value)| value)
        .ok_or(RegistrarPayloadError::MissingTitle { index })?;
    let starts_at = parse_start(object, index)?;
    let ends_at = parse_end(object, index, starts_at.date())?;
    if ends_at <= starts_at {
        return Err(RegistrarPayloadError::EndNotAfterStart { index });
    }

    // `grrlID` is present on editable personal-calendar entries, but the
    // read-only course feed used by current public clients does not promise
    // it. Preserve an id when the server sends one and retain `None` when it
    // does not; callers must not infer an identifier from display fields.
    let id = first_text(object, &["grrlID", "id", "eventId", "event_id"]).map(|(_, value)| value);

    Ok(RegistrarEvent {
        id,
        title,
        starts_at,
        ends_at,
        location: first_text(object, &["dd", "location", "place", "room"]).map(|(_, value)| value),
        category: first_text(object, &["fl", "category", "type"]).map(|(_, value)| value),
        course_code: first_text(object, &["kcdm", "courseCode", "course_code", "code"])
            .map(|(_, value)| value),
        instructor: first_text(object, &["jsxm", "teacher", "instructor", "teacherName"])
            .map(|(_, value)| value),
    })
}

fn parse_start(
    object: &Map<String, Value>,
    index: usize,
) -> Result<NaiveDateTime, RegistrarPayloadError> {
    let date = first_text(object, DATE_FIELDS)
        .map(|(field, value)| {
            parse_date(&value).ok_or_else(|| invalid_datetime(index, field, value))
        })
        .transpose()?;
    let (field, value) = first_text(object, START_DATETIME_FIELDS)
        .or_else(|| first_text(object, START_TIME_FIELDS))
        .ok_or(RegistrarPayloadError::MissingStartTime { index })?;
    if let Some(datetime) = parse_datetime(&value) {
        if date.is_some_and(|day| day != datetime.date()) {
            return Err(invalid_datetime(index, field, value));
        }
        return Ok(datetime);
    }
    let day = date.ok_or(RegistrarPayloadError::MissingStartTime { index })?;
    parse_time(&value)
        .map(|time| day.and_time(time))
        .ok_or_else(|| invalid_datetime(index, field, value))
}

fn parse_end(
    object: &Map<String, Value>,
    index: usize,
    fallback_date: NaiveDate,
) -> Result<NaiveDateTime, RegistrarPayloadError> {
    // An explicit end date is authoritative even when `end` contains only a
    // time. Previously this branch returned early using the start date.
    let explicit_date = first_text(object, &["endDate", "end_date"])
        .map(|(field, value)| {
            parse_date(&value).ok_or_else(|| invalid_datetime(index, field, value))
        })
        .transpose()?;
    let (field, value) = first_text(object, END_DATETIME_FIELDS)
        .or_else(|| first_text(object, END_TIME_FIELDS))
        .ok_or(RegistrarPayloadError::MissingEndTime { index })?;
    if let Some(datetime) = parse_datetime(&value) {
        if explicit_date.is_some_and(|day| day != datetime.date()) {
            return Err(invalid_datetime(index, field, value));
        }
        return Ok(datetime);
    }
    let day = explicit_date.unwrap_or(fallback_date);
    parse_time(&value)
        .map(|time| day.and_time(time))
        .ok_or_else(|| invalid_datetime(index, field, value))
}

fn invalid_datetime(index: usize, field: String, value: String) -> RegistrarPayloadError {
    RegistrarPayloadError::InvalidDateTime {
        index,
        field,
        value,
    }
}

fn first_text(object: &Map<String, Value>, fields: &[&str]) -> Option<(String, String)> {
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(scalar_text)
            .map(|value| ((*field).to_owned(), value))
    })
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    let value = value.trim().replace('／', "/");
    for format in ["%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d", "%Y%m%d"] {
        if let Ok(date) = NaiveDate::parse_from_str(&value, format) {
            return Some(date);
        }
    }
    parse_datetime(&value).map(|datetime| datetime.date())
}

fn parse_time(value: &str) -> Option<NaiveTime> {
    let value = value.trim().replace('：', ":");
    // Chrono numeric directives accept variable widths. Select the compact
    // grammar first so HHmm is never consumed as H/m/s by a longer format.
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let format = match value.len() {
            4 => "%H%M",
            6 => "%H%M%S",
            _ => return None,
        };
        return NaiveTime::parse_from_str(&value, format).ok();
    }
    ["%H:%M:%S%.f", "%H:%M"]
        .into_iter()
        .find_map(|format| NaiveTime::parse_from_str(&value, format).ok())
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    let original = value.trim();
    if let Ok(datetime) = DateTime::<FixedOffset>::parse_from_rfc3339(original) {
        // Registrar's naive fields are campus wall time. An explicit RFC
        // 3339 offset is an instant, not permission to discard that offset.
        let campus = FixedOffset::east_opt(8 * 60 * 60).expect("Tsinghua campus offset is valid");
        return Some(datetime.with_timezone(&campus).naive_local());
    }

    let normalized = original.replace(['T', 't'], " ").replace('：', ":");
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S%.f",
        "%Y/%m/%d %H:%M",
        "%Y%m%d %H:%M:%S%.f",
        "%Y%m%d %H:%M",
        "%Y%m%d%H%M%S",
        "%Y%m%d%H%M",
    ] {
        if let Ok(datetime) = NaiveDateTime::parse_from_str(&normalized, format) {
            return Some(datetime);
        }
    }
    None
}

fn login_expired_reason(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if text_indicates_login_expiry(value) => {
            Some("registrar payload indicated an expired or unauthenticated session".to_owned())
        }
        Value::Array(items) => items.iter().find_map(login_expired_reason),
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
                if let Some(Value::String(value)) = object.get(field) {
                    if text_indicates_login_expiry(value) {
                        return Some(
                            "registrar payload indicated an expired or unauthenticated session"
                                .to_owned(),
                        );
                    }
                }
            }

            for field in ["code", "status", "statusCode", "status_code", "httpCode"] {
                if let Some(value) = object.get(field) {
                    if matches!(value, Value::Number(number) if number.as_i64() == Some(401) || number.as_i64() == Some(403))
                        || matches!(value, Value::String(value) if value == "401" || value == "403")
                    {
                        return Some(
                            "registrar payload reported an authentication status".to_owned(),
                        );
                    }
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
                    return Some(
                        "registrar payload reported an unauthenticated session".to_owned(),
                    );
                }
            }

            for field in ["loginUrl", "login_url", "redirectUrl", "redirect_url"] {
                if let Some(Value::String(value)) = object.get(field) {
                    if value.to_ascii_lowercase().contains("login") {
                        return Some("registrar payload pointed to a login page".to_owned());
                    }
                }
            }

            for field in WRAPPER_FIELDS {
                if let Some(child) = object.get(*field) {
                    if let Some(reason) = login_expired_reason(child) {
                        return Some(reason);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn text_indicates_login_expiry(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "未登录",
        "请先登录",
        "请登录",
        "登录超时",
        "登录失效",
        "用户登陆超时",
        "登陆超时",
        "session timeout",
        "登录页面",
        "登录失败",
        "认证失败",
        "统一认证",
        "webvpn",
        "sso",
        "session expired",
        "not logged in",
        "authentication required",
        "unauthorized",
        "j_acegi_login",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

pub(crate) fn looks_like_login_page(body: &str) -> bool {
    // WebVPN rewrites/injects script and link URLs into valid business HTML.
    // A transport vendor's name in the source is NOT an expired session.
    // This only rejects login evidence; callers still require calendar JSONP
    // or a parsed, account-bound academic response before promoting a session.
    let document = scraper::Html::parse_document(body);
    let selector = |value| scraper::Selector::parse(value).expect("static auth selector");
    if document
        .select(&selector("input[type='password']"))
        .next()
        .is_some()
    {
        return true;
    }
    if document.select(&selector("form[action]")).any(|form| {
        let action = form
            .value()
            .attr("action")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let path = action.split(['?', '#']).next().unwrap_or_default();
        path.ends_with("j_acegi_login.do")
            || path.ends_with("security_check")
            || path.ends_with("/login")
            || path.contains("/do/off/ui/auth/login/")
    }) {
        return true;
    }
    if document.select(&selector("title")).any(|title| {
        let text = title.text().collect::<String>().to_ascii_lowercase();
        text.contains("webvpn") || text.contains("统一身份认证") || text.contains("统一认证")
    }) {
        return true;
    }
    let visible = document
        .root_element()
        .descendants()
        .filter_map(|node| {
            let text = node.value().as_text()?;
            let hidden = node
                .ancestors()
                .filter_map(|ancestor| ancestor.value().as_element())
                .any(|element| {
                    matches!(element.name(), "script" | "style" | "template" | "noscript")
                        || element.attr("hidden").is_some()
                });
            (!hidden).then(|| text.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    [
        "用户登陆超时",
        "登陆超时",
        "登录超时",
        "登录失效",
        "请先登录",
        "未登录",
        "authentication required",
        "session expired",
        "session timeout",
    ]
    .iter()
    .any(|marker| visible.contains(marker))
}

fn looks_like_login_response(final_url: &Url, content_type: Option<&str>, body: &str) -> bool {
    let path = final_url.path().to_ascii_lowercase();
    let final_url_text = final_url.as_str().to_ascii_lowercase();
    path == "/login"
        || path.ends_with("/login")
        || path.ends_with("/timeout.jsp")
        || path.ends_with("/sso_fail.jsp")
        || final_url_text.contains("timeout.jsp")
        || final_url_text.contains("sso_fail.jsp")
        || content_type
            .map(str::to_ascii_lowercase)
            .is_some_and(|value| value.starts_with("text/html") && looks_like_login_page(body))
        || looks_like_login_page(body)
}

fn redirect_points_to_login(final_url: &Url, location: Option<&str>) -> bool {
    let Some(location) = location else {
        return false;
    };
    let Ok(target) = Url::parse(location).or_else(|_| final_url.join(location)) else {
        return false;
    };
    registrar_read_response_is_login_page(&target, None, "")
        || target
            .path()
            .to_ascii_lowercase()
            .contains("/do/off/ui/auth/login")
}

/// Classifies a response to a verified Registrar read as a login page.
///
/// The ticket-consumption request itself may legitimately finish on
/// `j_acegi_login.do` before the calendar proof is made, so the broader login
/// classifier above intentionally does not reject that path by itself. A
/// later academic read on that path is unambiguously an expired session.
pub(crate) fn registrar_read_response_is_login_page(
    final_url: &Url,
    content_type: Option<&str>,
    body: &str,
) -> bool {
    final_url
        .path()
        .to_ascii_lowercase()
        .ends_with("/j_acegi_login.do")
        || looks_like_login_response(final_url, content_type, body)
}

#[cfg(test)]
mod tests {

    #[test]
    fn backend_repair_sep19_registrar_webvpn_injection_is_not_login_evidence() {
        let landing = r#"<html><head><script src='/wengine-vpn/webvpn.js'></script></head><body><h1>教务门户</h1><a href='/jxmh.do'>教学日历</a></body></html>"#;
        assert!(!super::looks_like_login_page(landing));
        for body in [
            "<html><title>清华大学WebVPN</title><form action='/login'><input name='password' type='password'></form></html>",
            "<html>用户登陆超时，请先登录</html>",
            "<html><form action='/j_acegi_login.do'><input type='password'></form></html>",
        ] {
            assert!(super::looks_like_login_page(body));
        }
    }
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).expect("valid fixture date")
    }

    fn window(start: u32, end: u32) -> CalendarWindow {
        CalendarWindow::new(date(start), date(end)).expect("valid fixture window")
    }

    #[test]
    fn parses_only_the_observed_plain_and_quoted_ticket_shapes() {
        let plain = parse_all_zhjw_ticket_response("  plain-ticket\n").expect("plain ticket");
        assert_eq!(plain.as_str(), "plain-ticket");

        let quoted = parse_all_zhjw_ticket_response(r#""quoted-ticket""#).expect("quoted ticket");
        assert_eq!(quoted.as_str(), "quoted-ticket");
        assert!(!format!("{quoted:?}").contains("quoted-ticket"));
    }

    #[test]
    fn rejects_empty_structured_html_and_multiline_ticket_responses() {
        for body in [
            "",
            "   ",
            r#"{"error":"login"}"#,
            "<html>login</html>",
            "ticket with spaces",
            r#""unterminated"#,
        ] {
            assert!(
                parse_all_zhjw_ticket_response(body).is_err(),
                "response should be rejected: {body:?}"
            );
        }
    }

    #[test]
    fn plans_a_profiled_all_zhjw_exchange_without_hard_coding_csrf() {
        let config = RegistrarClientConfig {
            learn_base_url: "https://learn.example.test/root".to_owned(),
            ticket_exchange: AllZhjwTicketProfile {
                path: "/auth/exchange".to_owned(),
                app_id_parameter: "application".to_owned(),
                app_id: "CUSTOM_REGISTRAR".to_owned(),
                csrf_parameter: Some("csrfToken".to_owned()),
                csrf_placement: ParameterPlacement::Form,
                extra_form: vec![("client".to_owned(), "thyou".to_owned())],
                ..AllZhjwTicketProfile::default()
            },
            ..RegistrarClientConfig::default()
        };

        let client = RegistrarClient::new(config).expect("client builds");
        let plan = client
            .all_zhjw_ticket_exchange_plan(Some("csrf-value"))
            .expect("exchange plan builds");

        assert_eq!(plan.method, Method::POST);
        assert_eq!(plan.url, "https://learn.example.test/auth/exchange");
        assert!(plan.query.is_empty());
        assert_eq!(
            plan.form,
            vec![
                ("application".to_owned(), "CUSTOM_REGISTRAR".to_owned()),
                ("client".to_owned(), "thyou".to_owned()),
                ("csrfToken".to_owned(), "csrf-value".to_owned()),
            ]
        );
        assert!(!format!("{plan:?}").contains("csrf-value"));
    }

    #[test]
    fn plans_stage_routes_and_inclusive_28_day_windows_with_custom_names() {
        let config = RegistrarClientConfig {
            registrar_base_url: "https://registrar.example.test".to_owned(),
            calendar: RegistrarCalendarProfile {
                undergraduate: RegistrarStageCalendarProfile {
                    path: "/undergraduate/calendar".to_owned(),
                    method: "undergradCalendar".to_owned(),
                },
                graduate: RegistrarStageCalendarProfile {
                    path: "/graduate/calendar".to_owned(),
                    method: "graduateCalendar".to_owned(),
                },
                method_parameter: "method".to_owned(),
                start_date_parameter: "from".to_owned(),
                end_date_parameter: "to".to_owned(),
                callback_parameter: "callback".to_owned(),
                ..RegistrarCalendarProfile::default()
            },
            ..RegistrarClientConfig::default()
        };

        let client = RegistrarClient::new(config).expect("client builds");
        let plans = client
            .calendar_request_plans(
                AcademicStage::Graduate,
                date(1),
                NaiveDate::from_ymd_opt(2026, 10, 1).expect("valid fixture date"),
                "fixtureCallback",
            )
            .expect("calendar plans build");

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].window.day_count(), 28);
        assert_eq!(plans[0].window.start, date(1));
        assert_eq!(plans[0].window.end, date(28));
        assert_eq!(plans[1].window.start, date(29));
        assert_eq!(
            plans[1].window.end,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
        );
        assert_eq!(
            plans[0].url,
            "https://registrar.example.test/graduate/calendar"
        );
        assert_eq!(plans[0].callback, "fixtureCallback");
        assert_eq!(
            plans[0].query,
            vec![
                ("method".to_owned(), "graduateCalendar".to_owned()),
                ("from".to_owned(), "20260901".to_owned()),
                ("to".to_owned(), "20260928".to_owned()),
                ("callback".to_owned(), "fixtureCallback".to_owned()),
            ]
        );

        let login = client
            .registrar_login_request_plan("ticket-value")
            .expect("login plan builds");
        assert!(!format!("{login:?}").contains("ticket-value"));
    }

    #[test]
    fn maps_the_current_registrar_fixture_and_ignores_unknown_fields() {
        let payload = r#"fixtureCb([{
            "grrlID": "event-42",
            "nr": "高等数学",
            "nq": "2026-09-11",
            "kssj": "08：00",
            "jssj": "09:35",
            "dd": "六教6A013",
            "fl": "必修",
            "unknownFutureField": "ignored"
        }])"#;

        let record = decode_calendar_jsonp(
            payload,
            "fixtureCb",
            AcademicStage::Undergraduate,
            window(11, 11),
        )
        .expect("fixture should decode");

        assert_eq!(record.events.len(), 1);
        let event = &record.events[0];
        assert_eq!(event.id.as_deref(), Some("event-42"));
        assert_eq!(event.title, "高等数学");
        assert_eq!(event.date(), date(11));
        assert_eq!(
            event.start_time(),
            NaiveTime::from_hms_opt(8, 0, 0).unwrap()
        );
        assert_eq!(event.end_time(), NaiveTime::from_hms_opt(9, 35, 0).unwrap());
        assert_eq!(event.location.as_deref(), Some("六教6A013"));
        assert_eq!(event.category.as_deref(), Some("必修"));
        assert!(event.course_code.is_none());
    }

    #[test]
    fn classifies_the_legacy_http_200_timeout_page_before_jsonp_parsing() {
        let response = CampusTextResponse {
            status: reqwest::StatusCode::OK,
            final_url: Url::parse("http://zhjw.cic.tsinghua.edu.cn/timeout.jsp")
                .expect("timeout URL"),
            content_type: Some("text/html;charset=gbk".to_owned()),
            redirect_location: None,
            body: "<html>用户登陆超时，请重新登录</html>".to_owned(),
        };

        let error = decode_calendar_response(
            "https://zhjw.cic.tsinghua.edu.cn",
            response,
            "fixtureCb",
            AcademicStage::Undergraduate,
            window(11, 11),
            "https://zhjw.cic.tsinghua.edu.cn/jxmh_out.do",
            &[
                ("m".to_owned(), "bks_jxrl_all".to_owned()),
                ("p_start_date".to_owned(), "20260911".to_owned()),
                ("p_end_date".to_owned(), "20260911".to_owned()),
                ("jsoncallback".to_owned(), "fixtureCb".to_owned()),
            ],
        )
        .expect_err("timeout page must not be parsed as JSONP");

        assert!(matches!(error, RegistrarClientError::LoginExpired { .. }));
    }

    #[test]
    fn classifies_a_grade_read_that_finishes_on_the_registrar_login_path() {
        let response_url = Url::parse("https://zhjw.cic.tsinghua.edu.cn/j_acegi_login.do?url=%2F")
            .expect("login URL");

        assert!(registrar_read_response_is_login_page(
            &response_url,
            Some("text/html"),
            "<html><body>重新登录</body></html>",
        ));
    }

    #[test]
    fn rejects_registrar_endpoint_profiles_that_escape_the_configured_origin() {
        let error = resolve_endpoint(
            "https://zhjw.cic.tsinghua.edu.cn",
            "https://evil.example.test/redirect",
        )
        .expect_err("external endpoint must be rejected");
        assert!(matches!(error, RegistrarClientError::InvalidConfig { .. }));
    }

    #[test]
    fn validates_final_registrar_origin_before_grade_or_calendar_parsing() {
        let base = "https://zhjw.cic.tsinghua.edu.cn";
        assert!(
            registrar_origin_is_allowed(
                base,
                &Url::parse("https://zhjw.cic.tsinghua.edu.cn/cj.cjCjbAll.do")
                    .expect("same-origin URL"),
            )
            .expect("origin check succeeds")
        );
        assert!(
            !registrar_origin_is_allowed(
                base,
                &Url::parse("https://evil.example.test/cj.cjCjbAll.do").expect("external URL"),
            )
            .expect("origin check succeeds")
        );
        assert!(
            !registrar_origin_is_allowed(
                base,
                &Url::parse("https://zhjw.cic.tsinghua.edu.cn:444/cj.cjCjbAll.do")
                    .expect("wrong-port URL"),
            )
            .expect("origin check succeeds")
        );
    }

    #[test]
    fn rejects_unverified_calendar_wrappers_instead_of_treating_them_as_empty_or_valid() {
        let error = decode_calendar_jsonp(
            r#"customCb({"status":"ok","data":[]})"#,
            "customCb",
            AcademicStage::Graduate,
            window(12, 12),
        )
        .expect_err("only the observed top-level event array is accepted");
        assert!(matches!(
            error,
            RegistrarClientError::InvalidBusinessPayload {
                source: RegistrarPayloadError::UnsupportedShape
            }
        ));
    }

    #[test]
    fn separates_login_expiry_from_jsonp_and_business_errors() {
        let login_error = decode_calendar_jsonp(
            r#"anyCb({"code":401,"message":"登录已过期"})"#,
            "anyCb",
            AcademicStage::Undergraduate,
            window(1, 1),
        )
        .expect_err("expired session should fail");
        assert!(matches!(
            login_error,
            RegistrarClientError::LoginExpired { .. }
        ));

        let jsonp_error = decode_calendar_jsonp(
            r#"differentCb([])"#,
            "expectedCb",
            AcademicStage::Undergraduate,
            window(1, 1),
        )
        .expect_err("wrong callback should fail");
        assert!(matches!(
            jsonp_error,
            RegistrarClientError::InvalidJsonp {
                source: JsonpError::CallbackMismatch { .. }
            }
        ));

        let business_error = decode_calendar_jsonp(
            r#"expectedCb([{"nq":"2026-09-01","kssj":"08:00","jssj":"09:35"}])"#,
            "expectedCb",
            AcademicStage::Undergraduate,
            window(1, 1),
        )
        .expect_err("missing title should fail");
        assert!(matches!(
            business_error,
            RegistrarClientError::InvalidBusinessPayload {
                source: RegistrarPayloadError::MissingTitle { index: 0 }
            }
        ));

        let without_server_id = decode_calendar_jsonp(
            r#"expectedCb([{"nr":"课程","nq":"2026-09-01","kssj":"08:00","jssj":"09:35"}])"#,
            "expectedCb",
            AcademicStage::Undergraduate,
            window(1, 1),
        )
        .expect("the current read-only feed does not require grrlID");
        assert_eq!(without_server_id.events.len(), 1);
        assert_eq!(without_server_id.events[0].id, None);
    }

    #[test]
    fn accepts_an_empty_calendar_array() {
        let record = decode_calendar_jsonp(
            "arbitraryCb([])",
            "arbitraryCb",
            AcademicStage::Undergraduate,
            window(1, 1),
        )
        .expect("an empty calendar is a valid business response");
        assert!(record.events.is_empty());
    }

    #[tokio::test]
    async fn fetches_calendar_with_real_jsonp_query_and_cookie_reuse() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let expected_paths = [
                "/jxmh_out.do?m=bks_jxrl_all&p_start_date=20260901&p_end_date=20260928&jsoncallback=calendarCallback",
                "/jxmh_out.do?m=bks_jxrl_all&p_start_date=20260929&p_end_date=20260929&jsoncallback=calendarCallback",
            ];
            for (index, expected_path) in expected_paths.into_iter().enumerate() {
                let (mut stream, _) = listener.accept().expect("connection");
                let request = read_http_request(&mut stream);
                assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")));
                if index == 1 {
                    assert!(request.lines().any(|line| {
                        line.eq_ignore_ascii_case("cookie: registrar-session=present")
                    }));
                }
                let extra_headers = if index == 0 {
                    "Set-Cookie: registrar-session=present; Path=/\r\n"
                } else {
                    ""
                };
                write_http_response(
                    &mut stream,
                    "application/javascript",
                    "calendarCallback([])",
                    extra_headers,
                );
            }
        });

        let mut config = RegistrarClientConfig::default();
        config.registrar_base_url = format!("http://{address}");
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let client = RegistrarClient::from_transport(config, transport).expect("client");
        let records = client
            .fetch_calendar_range(
                AcademicStage::Undergraduate,
                date(1),
                date(29),
                "calendarCallback",
            )
            .await
            .expect("calendar requests");

        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record.events.is_empty()));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn rejects_http_200_registrar_login_page_during_session_establishment() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let request = read_http_request(&mut stream);
            assert!(
                request
                    .starts_with("GET /j_acegi_login.do?url=%2F&ticket=opaque-ticket HTTP/1.1\r\n")
            );
            write_http_response(
                &mut stream,
                "text/html",
                "<html><form action=\"/j_acegi_login.do\"><input name=\"password\"></form></html>",
                "",
            );
        });

        let mut config = RegistrarClientConfig::default();
        config.registrar_base_url = format!("http://{address}");
        let client = RegistrarClient::new(config).expect("client");
        let error = client
            .establish_registrar_session("opaque-ticket")
            .await
            .expect_err("login HTML must not establish a session");
        assert!(matches!(error, RegistrarClientError::LoginExpired { .. }));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn fetches_grades_with_existing_registrar_cookie_and_strict_html_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("grades request");
            let request = read_http_request(&mut stream);
            assert!(
                request
                    .starts_with("GET /cj.cjCjbAll.do?m=bks_cjdcx&cjdlx=zw&flag=di1 HTTP/1.1\r\n")
            );
            assert!(request.contains("registrar-session=present"));
            let body = r#"
                <table cellspacing="1">
                  <tr>
                    <th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th>
                    <th>属性</th><th>学分</th><th>学时</th><th>成绩</th>
                    <th>备注</th><th>绩点</th><th>教师</th><th>学期</th>
                  </tr>
                  <tr>
                    <td>1</td><td>30240512</td><td>必修</td><td>数据结构</td>
                    <td>必修</td><td>3</td><td>48</td><td>A</td><td></td><td>4.0</td>
                    <td>教师</td><td>2025-2026-2</td>
                  </tr>
                </table>
            "#;
            write_http_response(&mut stream, "text/html; charset=UTF-8", body, "");
        });

        let base_url = format!("http://{address}/");
        let transport = CampusHttpTransport::with_timeout(
            "THYou/grades-proof-test",
            std::time::Duration::from_secs(5),
        )
        .expect("transport");
        let base = Url::parse(&base_url).expect("base URL");
        transport
            .cookie_jar()
            .add_cookie_str("registrar-session=present; Path=/", &base);
        let mut config = RegistrarClientConfig::default();
        config.registrar_base_url = base_url;
        let client = RegistrarClient::from_transport(config, transport).expect("client");

        let report = client
            .fetch_grades(
                crate::registrar_academic::RegistrarGradesProfile::undergraduate(
                    crate::registrar_academic::UndergraduateReportKind::FirstDegree,
                ),
            )
            .await
            .expect("grades response");
        assert_eq!(report.courses.len(), 1);
        assert_eq!(report.courses[0].course_name, "数据结构");
        server.join().expect("server");
    }

    #[test]
    fn public_exam_page_entry_uses_the_confirmed_undergraduate_contract() {
        let request = RegistrarExamPageProfile::undergraduate().request();
        let endpoint = request
            .endpoint_url("https://registrar.example.test")
            .expect("confirmed exam endpoint");
        assert_eq!(
            endpoint.as_str(),
            "https://registrar.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch"
        );
        assert!(RegistrarExamPageProfile::undergraduate().is_configured());
    }

    #[tokio::test]
    async fn rejects_a_grade_redirect_that_drops_the_confirmed_query() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("initial grades request");
            let request = read_http_request(&mut first);
            assert!(
                request
                    .starts_with("GET /cj.cjCjbAll.do?m=bks_cjdcx&cjdlx=zw&flag=di1 HTTP/1.1\r\n")
            );
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /cj.cjCjbAll.do\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect");

            let (mut second, _) = listener.accept().expect("redirected grades request");
            let redirected = read_http_request(&mut second);
            assert!(redirected.starts_with("GET /cj.cjCjbAll.do HTTP/1.1\r\n"));
            write_http_response(
                &mut second,
                "text/html; charset=UTF-8",
                "<html><body>grade page without query</body></html>",
                "",
            );
        });

        let mut config = RegistrarClientConfig::default();
        config.registrar_base_url = format!("http://{address}/");
        let client = RegistrarClient::new(config).expect("client");
        let error = client
            .fetch_grades(RegistrarGradesProfile::undergraduate(
                crate::registrar_academic::UndergraduateReportKind::FirstDegree,
            ))
            .await
            .expect_err("query-dropping redirect must fail closed");
        assert!(matches!(
            error,
            RegistrarClientError::InvalidConfig { message }
                if message == "grade response did not retain the confirmed query"
        ));
        server.join().expect("server");
    }

    #[test]
    fn exam_course_entry_uses_the_confirmed_jsonp_contract() {
        let query = RegistrarExamCourseQuery::new(
            "30240512",
            "0",
            RegistrarExamTerm::parse("2025-2026-2").expect("term"),
            "examCallback",
        )
        .expect("query");
        let request = query.request();
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.path, "/jxmh.do");
        assert_eq!(
            request.query,
            vec![
                ("m".to_owned(), "bks_ksSearch".to_owned()),
                ("kch".to_owned(), "30240512".to_owned()),
                ("kxh".to_owned(), "0".to_owned()),
                ("p_xnxq".to_owned(), "2025-2026-2".to_owned()),
                ("jsoncallback".to_owned(), "examCallback".to_owned()),
            ]
        );
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
        String::from_utf8(request).expect("HTTP request is UTF-8")
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
}

#[cfg(test)]
mod academic_data_repairs {
    use super::*;

    #[test]
    fn backend_repair_calendar_explicit_invalid_end_date_is_not_silently_replaced() {
        let body = serde_json::json!({"nr":"fixture","nq":"2026-09-14","kssj":"08:00","jssj":"09:00","endDate":"invalid"});
        assert!(parse_event(&body, 0).is_err());
        let valid = serde_json::json!({"nr":"fixture","nq":"2026-09-14","kssj":"23:00","jssj":"01:00","endDate":"2026-09-15"});
        let event = parse_event(&valid, 0).unwrap();
        assert_eq!((event.ends_at - event.starts_at).num_hours(), 2);
    }

    #[test]
    fn backend_repair_calendar_absolute_timestamps_keep_instant_in_campus_timezone() {
        let event=parse_event(&serde_json::json!({"nr":"fixture","start":"2026-09-13T23:30:00Z","end":"2026-09-14T00:30:00Z"}),0).unwrap();
        assert_eq!(event.starts_at.to_string(), "2026-09-14 07:30:00");
        assert_eq!(event.ends_at.to_string(), "2026-09-14 08:30:00");
        assert_eq!(
            parse_datetime("2026-09-14T07:30:00+08:00"),
            Some(event.starts_at)
        );
        assert_eq!(parse_datetime("2026-09-14 07:30:00"), Some(event.starts_at));
    }
}

#[cfg(test)]
#[path = "registrar_time_repair_tests.rs"]
mod time_repair_tests;
