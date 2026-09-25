//! Composition of the verified learning and registrar clients into a campus
//! data source.
//!
//! This module owns aggregation and domain mapping only.  Authentication still
//! belongs to the session layer: callers must provide a `RegistrarClient`
//! whose transport already contains the required cookies/tickets.  The module
//! therefore never accepts or stores a password, ticket, or CSRF value.

use std::{
    collections::{BTreeSet, HashSet},
    fmt,
    sync::Arc,
};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};
use reqwest::StatusCode;
#[cfg(test)]
use reqwest::Url;
use uuid::Uuid;

use crate::{
    domain::{
        CampusOverview, Course, CourseStatus, DateRange, NewTodo, ScheduleItem, ScheduleKind,
        TodoFilter, TodoItem, TodoStatus,
    },
    error::ServiceError,
    learn::validate_semester_id,
    learn_announcements::{
        LearnAnnouncement, LearnAnnouncementConfig, LearnAnnouncementError, LearnAnnouncementSource,
    },
    learn_client::{LearnClient, LearnClientError, LearnCourseRecord},
    learn_discussions::{LearnDiscussionError, LearnDiscussionRead, read_course_discussions},
    learn_files::{
        LearnFileCategoryRecord, LearnFileError, LearnFileRead, read_course_files,
        read_file_categories,
    },
    learn_homework::{HomeworkDetail, HomeworkDetailError, read_homework_detail},
    learn_todos::{LearnHomeworkRecord, LearnTodoConfig, LearnTodoSource, stable_uuid},
    protocol::{AcademicStage, CourseRole, ServiceId},
    registrar_academic::{RegistrarGradeReport, RegistrarGradesProfile},
    registrar_client::registrar_exam::{RegistrarExamRecord, RegistrarExamStage},
    registrar_client::{RegistrarCalendarRecord, RegistrarClient, RegistrarClientError},
    services::{
        CampusDataSource, CampusOverviewRecoveryHints, CampusOverviewSectionErrors,
        CampusOverviewSections,
    },
    session::BoundCsrfToken,
};

/// Tsinghua's Registrar calendar returns wall-clock values in Beijing time.
/// Keep the offset in one place so date-window construction and runtime
/// authentication probes cannot drift apart at a UTC day boundary.
pub(crate) const CAMPUS_TIMEZONE_OFFSET_MINUTES: i32 = 8 * 60;

/// Converts an absolute instant to the campus calendar date used by the
/// Registrar and Learn services. This is deliberately a fixed offset: the
/// deployment has no daylight-saving transition and the upstream calendar
/// payload does not carry a timezone identifier.
pub(crate) fn campus_date_at(instant: DateTime<Utc>) -> NaiveDate {
    FixedOffset::east_opt(CAMPUS_TIMEZONE_OFFSET_MINUTES * 60)
        .expect("the configured campus timezone offset is valid")
        .from_utc_datetime(&instant.naive_utc())
        .date_naive()
}

/// The todo operations required by the campus aggregator.
///
/// Todo endpoints are deployment-specific and are intentionally kept out of
/// the course/calendar adapter.  A caller can provide a service backed by a
/// verified client once the corresponding routes have been configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampusTodoFailureKind {
    SessionExpired,
    Transport,
    HttpStatus,
    RouteDrift,
    BusinessFailure,
    MalformedResponse,
}

impl fmt::Display for CampusTodoFailureKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::SessionExpired => "session_expired",
            Self::Transport => "transport",
            Self::HttpStatus => "http_status",
            Self::RouteDrift => "route_drift",
            Self::BusinessFailure => "business_failure",
            Self::MalformedResponse => "malformed_response",
        };

        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusTodoProviderFailure {
    pub course_id: Uuid,
    pub provider: String,
    pub kind: CampusTodoFailureKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CampusTodoReadReport {
    pub items: Vec<TodoItem>,
    pub failures: Vec<CampusTodoProviderFailure>,
}

impl CampusTodoReadReport {
    pub fn complete(items: Vec<TodoItem>) -> Self {
        Self {
            items,
            failures: Vec::new(),
        }
    }

    pub fn failure_summary(&self) -> Option<String> {
        if self.failures.is_empty() {
            return None;
        }
        let kinds = self
            .failures
            .iter()
            .map(|failure| failure.kind.to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        Some(format!(
            "{} todo provider(s) failed: {kinds}",
            self.failures.len()
        ))
    }
}

#[async_trait]
pub trait CampusTodoSource: Send + Sync {
    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError>;

    /// Reads todos while retaining provider failures alongside verified
    /// records. The default preserves the strict all-or-nothing behavior of
    /// legacy sources; providers with multiple remote endpoints can override
    /// it to expose partial data without converting a failed endpoint into an
    /// empty success.
    async fn list_todos_report(
        &self,
        filter: TodoFilter,
    ) -> Result<CampusTodoReadReport, ServiceError> {
        self.list_todos(filter)
            .await
            .map(CampusTodoReadReport::complete)
    }

    /// Reuse the course response proved by this same aggregate/session.
    /// Providers that do not consume Learn courses retain their usual path.
    async fn list_todos_for_courses(
        &self,
        _courses: Vec<LearnCourseRecord>,
        filter: TodoFilter,
    ) -> Result<CampusTodoReadReport, ServiceError> {
        self.list_todos_report(filter).await
    }

    async fn create_todo(&self, input: NewTodo) -> Result<TodoItem, ServiceError>;

    async fn set_todo_status(&self, id: Uuid, status: TodoStatus)
    -> Result<TodoItem, ServiceError>;
}

pub type DynCampusTodoSource = Arc<dyn CampusTodoSource>;

/// Explicit placeholder for a deployment where the todo service has not been
/// configured. Returning an adapter error keeps an incomplete live response
/// from being presented as an empty todo list and prevents local todo writes.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnconfiguredTodoSource;

#[async_trait]
impl CampusTodoSource for UnconfiguredTodoSource {
    async fn list_todos(&self, _filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError> {
        Err(todo_unavailable("read"))
    }

    async fn create_todo(&self, _input: NewTodo) -> Result<TodoItem, ServiceError> {
        Err(todo_unavailable("create"))
    }

    async fn set_todo_status(
        &self,
        _id: Uuid,
        _status: TodoStatus,
    ) -> Result<TodoItem, ServiceError> {
        Err(todo_unavailable("update"))
    }
}

/// Compatibility name for the old unconfigured source. New code should use
/// [`UnconfiguredTodoSource`] to make its fail-closed behavior explicit.
#[doc(hidden)]
pub type UnavailableTodoSource = UnconfiguredTodoSource;

/// Deployment values needed to aggregate the learning and registrar payloads.
///
/// The calendar timezone is explicit because registrar events are represented
/// as local wall-clock values by the parser.  The default is Beijing time for
/// the THU deployment, while the field remains configurable for fixtures and
/// future deployments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusLiveConfig {
    pub semester: String,
    pub language: String,
    pub course_index: Option<String>,
    pub academic_stage: AcademicStage,
    pub calendar_callback: String,
    pub calendar_timezone_offset_minutes: i32,
}

impl CampusLiveConfig {
    pub fn new(semester: impl Into<String>, academic_stage: AcademicStage) -> Self {
        Self {
            semester: semester.into(),
            language: "zh".to_owned(),
            course_index: None,
            academic_stage,
            calendar_callback: "thyouCalendar".to_owned(),
            calendar_timezone_offset_minutes: CAMPUS_TIMEZONE_OFFSET_MINUTES,
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

    pub fn with_calendar_callback(mut self, callback: impl Into<String>) -> Self {
        self.calendar_callback = callback.into();
        self
    }

    pub fn with_calendar_timezone_offset_minutes(mut self, offset: i32) -> Self {
        self.calendar_timezone_offset_minutes = offset;
        self
    }

    fn timezone(&self) -> Result<FixedOffset, ServiceError> {
        let seconds = self
            .calendar_timezone_offset_minutes
            .checked_mul(60)
            .ok_or_else(|| adapter_error("campus calendar timezone offset is invalid"))?;
        FixedOffset::east_opt(seconds).ok_or_else(|| {
            adapter_error("campus calendar timezone offset is outside the supported range")
        })
    }

    fn validate_for(&self, _role: CourseRole) -> Result<(), ServiceError> {
        if validate_semester_id(&self.semester).is_err()
            || self.language.trim().is_empty()
            || self.language.contains(['/', '?', '#'])
        {
            return Err(adapter_error("campus semester or language is invalid"));
        }
        if self.calendar_callback.trim().is_empty()
            || self.calendar_callback.chars().any(char::is_control)
        {
            return Err(adapter_error("campus calendar callback is invalid"));
        }
        let _ = self.timezone()?;
        Ok(())
    }
}

/// A live source assembled from the existing learning and registrar clients.
///
/// The registrar client owns the shared cookie-aware transport.  The learning
/// client contributes route planning and parsers, while requests are executed
/// through the registrar client's transport so roaming and registrar cookies
/// remain in one session jar.  `todo_source` is separate because its protocol
/// is not part of either course/calendar client.
#[derive(Clone)]
pub struct CampusLiveDataSource {
    learn: LearnClient,
    registrar: RegistrarClient,
    config: CampusLiveConfig,
    todo_source: DynCampusTodoSource,
    learn_csrf: Option<BoundCsrfToken>,
    learn_csrf_parameter: Option<String>,
    overview_learn_available: bool,
    overview_registrar_available: bool,
}

impl CampusLiveDataSource {
    pub fn new(
        learn: LearnClient,
        registrar: RegistrarClient,
        config: CampusLiveConfig,
        todo_source: DynCampusTodoSource,
    ) -> Result<Self, ServiceError> {
        config.validate_for(learn.config().role())?;
        Ok(Self {
            learn,
            registrar,
            config,
            todo_source,
            learn_csrf: None,
            learn_csrf_parameter: None,
            overview_learn_available: true,
            overview_registrar_available: true,
        })
    }

    pub fn without_todos(
        learn: LearnClient,
        registrar: RegistrarClient,
        config: CampusLiveConfig,
    ) -> Result<Self, ServiceError> {
        Self::new(learn, registrar, config, Arc::new(UnconfiguredTodoSource))
    }

    pub fn learn(&self) -> &LearnClient {
        &self.learn
    }

    pub fn registrar(&self) -> &RegistrarClient {
        &self.registrar
    }

    pub fn config(&self) -> &CampusLiveConfig {
        &self.config
    }

    /// A partial overview may use only independently proven services. Merely
    /// owning a Registrar client for its Cookie transport is not permission
    /// to query Registrar. The disabled sections carry explicit errors.
    pub(crate) fn scoped_to_services(&self, learn: bool, registrar: bool) -> Self {
        let mut scoped = self.clone();
        scoped.overview_learn_available = learn;
        scoped.overview_registrar_available = registrar;
        scoped
    }

    /// Reads Learn course announcements through the same Cookie jar and
    /// service-bound CSRF proof used by the live course source. This is the
    /// smallest runtime-facing Learn entry point; INFO news is never used as
    /// a fallback for this operation.
    pub async fn list_learn_announcements(
        &self,
        course_id: &str,
    ) -> Result<Vec<LearnAnnouncement>, ServiceError> {
        let csrf = self
            .learn_csrf
            .as_ref()
            .ok_or_else(|| adapter_error("learning announcement CSRF token is not configured"))?;
        let parameter_name = self.learn_csrf_parameter.as_deref().ok_or_else(|| {
            adapter_error("learning announcement CSRF query parameter is not configured")
        })?;
        let config = LearnAnnouncementConfig::new(self.learn.config().role())
            .with_csrf_parameter(parameter_name)
            .map_err(|error| map_learn_announcement_error(error))?;
        let mut source = LearnAnnouncementSource::with_config(
            self.learn.clone(),
            self.registrar.transport().clone(),
            config,
        )
        .map_err(map_learn_announcement_error)?;
        source
            .with_csrf(csrf.clone())
            .map_err(map_learn_announcement_error)?;
        source
            .list_course(course_id)
            .await
            .map_err(map_learn_announcement_error)
    }

    /// Reads file metadata for one course through the proven Learn Cookie jar
    /// and its service-bound CSRF. Download URLs stay within the backend.
    pub(crate) async fn list_learn_files(
        &self,
        course_id: &str,
    ) -> Result<LearnFileRead, LearnFileError> {
        let csrf = self.learn_csrf.as_ref().ok_or(LearnFileError::Route)?;
        let parameter = self
            .learn_csrf_parameter
            .as_deref()
            .ok_or(LearnFileError::Route)?;
        read_course_files(
            &self.learn,
            self.registrar.transport(),
            csrf,
            parameter,
            course_id,
        )
        .await
    }

    pub(crate) async fn list_learn_file_categories(
        &self,
        course_id: &str,
    ) -> Result<Vec<LearnFileCategoryRecord>, LearnFileError> {
        let csrf = self.learn_csrf.as_ref().ok_or(LearnFileError::Route)?;
        let parameter = self
            .learn_csrf_parameter
            .as_deref()
            .ok_or(LearnFileError::Route)?;
        read_file_categories(
            &self.learn,
            self.registrar.transport(),
            csrf,
            parameter,
            course_id,
        )
        .await
    }

    /// Lists course discussion topics through the same account-bound Learn
    /// transport and CSRF proof. Raw board/topic IDs stay in Rust.
    pub(crate) async fn list_learn_discussions(
        &self,
        course_id: &str,
    ) -> Result<LearnDiscussionRead, LearnDiscussionError> {
        let csrf = self
            .learn_csrf
            .as_ref()
            .ok_or(LearnDiscussionError::Route)?;
        let parameter = self
            .learn_csrf_parameter
            .as_deref()
            .ok_or(LearnDiscussionError::Route)?;
        read_course_discussions(
            &self.learn,
            self.registrar.transport(),
            csrf,
            parameter,
            course_id,
        )
        .await
    }

    /// The three student homework buckets use the same proven Learn transport
    /// and CSRF as course files. Raw student/base IDs remain in Rust.
    pub(crate) async fn list_learn_homework(
        &self,
        course_id: &str,
    ) -> Result<Vec<LearnHomeworkRecord>, CampusTodoFailureKind> {
        let csrf = self
            .learn_csrf
            .as_ref()
            .ok_or(CampusTodoFailureKind::RouteDrift)?;
        let parameter = self
            .learn_csrf_parameter
            .as_deref()
            .ok_or(CampusTodoFailureKind::RouteDrift)?;
        let mut source = LearnTodoSource::new(
            self.learn.clone(),
            self.registrar.transport().clone(),
            LearnTodoConfig::new(self.config.semester.clone()),
        )
        .map_err(|_| CampusTodoFailureKind::RouteDrift)?;
        source
            .with_csrf(csrf.clone(), parameter)
            .map_err(|_| CampusTodoFailureKind::RouteDrift)?;
        source.list_course_homework_read(course_id).await
    }

    pub(crate) async fn read_learn_homework_detail(
        &self,
        course_id: &str,
        student_id: &str,
        base_id: &str,
    ) -> Result<HomeworkDetail, HomeworkDetailError> {
        let csrf = self.learn_csrf.as_ref().ok_or(HomeworkDetailError::Route)?;
        let parameter = self
            .learn_csrf_parameter
            .as_deref()
            .ok_or(HomeworkDetailError::Route)?;
        read_homework_detail(
            &self.learn,
            self.registrar.transport(),
            csrf,
            parameter,
            course_id,
            student_id,
            base_id,
        )
        .await
    }

    /// Fetches and parses one read-only Registrar grade report through the
    /// already authenticated Cookie-aware session. The profile owns the
    /// relative path and stage-specific query; this adapter owns only the
    /// configured Registrar origin and transport.
    pub async fn list_grades(
        &self,
        profile: RegistrarGradesProfile,
    ) -> Result<RegistrarGradeReport, ServiceError> {
        self.registrar
            .fetch_grades(profile)
            .await
            .map_err(map_registrar_error)
    }

    /// Fetches the independently verified undergraduate examination page
    /// through the existing Registrar Cookie-aware session.  The page is a
    /// standalone HTML table and is intentionally kept separate from the
    /// calendar event source.
    pub async fn list_undergraduate_exams(&self) -> Result<Vec<RegistrarExamRecord>, ServiceError> {
        self.registrar
            .fetch_verified_exam_page(RegistrarExamStage::Undergraduate)
            .await
            .map_err(map_registrar_error)
    }

    /// Supplies the CSRF token extracted from the learning session and the
    /// deployment-specific query parameter used by the course endpoint.
    ///
    /// The service binding is checked before the token enters this adapter, so
    /// a CSRF token from INFO or USEREG cannot be reused accidentally.
    pub fn with_learn_csrf(
        &mut self,
        csrf: BoundCsrfToken,
        parameter_name: impl Into<String>,
    ) -> Result<(), ServiceError> {
        if csrf.service() != crate::protocol::ServiceId::Learn {
            return Err(adapter_error(
                "learning CSRF token belongs to another service",
            ));
        }
        let parameter_name = parameter_name.into();
        if parameter_name.trim().is_empty()
            || parameter_name
                .chars()
                .any(|character| character.is_control() || matches!(character, '&' | '='))
        {
            return Err(adapter_error("learning CSRF query parameter is invalid"));
        }
        self.learn_csrf = Some(csrf);
        self.learn_csrf_parameter = Some(parameter_name);
        Ok(())
    }

    /// Fetches the course list through the same strict Learn client used by
    /// the standalone runtime action.  Keeping this as the only production
    /// course-list entry point prevents the overview source from accepting a
    /// same-origin error page or an unverified JSON shape as a real list.
    pub async fn list_learn_course_records(&self) -> Result<Vec<LearnCourseRecord>, ServiceError> {
        let csrf = self
            .learn_csrf
            .as_ref()
            .ok_or_else(|| adapter_error("learning CSRF token is not configured"))?;
        let parameter_name = self
            .learn_csrf_parameter
            .as_deref()
            .ok_or_else(|| adapter_error("learning CSRF query parameter is not configured"))?;
        self.learn
            .fetch_all_course_records(
                self.registrar.transport(),
                csrf,
                &self.config.semester,
                Some(&self.config.language),
                self.config.course_index.as_deref(),
                parameter_name,
            )
            .await
            .map_err(map_learn_error)
    }

    /// The reference's graduate teaching calendar includes examinations. Keep
    /// the original category and optional course metadata for a strict exam
    /// projection instead of guessing from titles or flattened schedule IDs.
    pub async fn list_graduate_calendar_events(
        &self,
        range: DateRange,
    ) -> Result<Vec<crate::registrar_client::RegistrarEvent>, ServiceError> {
        if self.config.academic_stage != AcademicStage::Graduate {
            return Err(adapter_error("graduate calendar source stage mismatch"));
        }
        let timezone = self.config.timezone()?;
        let records = self.fetch_schedule_records(range.clone()).await?;
        let mut events = Vec::new();
        for record in records {
            if record.stage != AcademicStage::Graduate {
                return Err(adapter_error("graduate calendar response stage mismatch"));
            }
            for event in record.events {
                let start = timezone
                    .from_local_datetime(&event.starts_at)
                    .single()
                    .ok_or_else(|| adapter_error("graduate calendar time is invalid"))?
                    .with_timezone(&Utc);
                if start >= range.start && start < range.end {
                    events.push(event);
                }
            }
        }
        Ok(events)
    }

    async fn fetch_schedule_records(
        &self,
        range: DateRange,
    ) -> Result<Vec<RegistrarCalendarRecord>, ServiceError> {
        // Registrar's `p_start_date`/`p_end_date` are campus-local calendar
        // dates, while the domain range is an absolute UTC interval. Convert
        // both endpoints before forming the inclusive server window. This is
        // essential for an overview range built from Beijing midnight: its
        // UTC start is on the previous day.
        let timezone = self.config.timezone()?;
        let local_start = range.start.with_timezone(&timezone);
        let local_end = range.end.with_timezone(&timezone);
        let start = local_start.date_naive();
        let mut end = local_end.date_naive();
        if local_end.time() == NaiveTime::from_hms_opt(0, 0, 0).expect("midnight is valid") {
            end = end.pred_opt().ok_or_else(|| {
                adapter_error("calendar range end is outside the supported range")
            })?;
        }
        if end < start {
            end = start;
        }
        self.registrar
            .fetch_calendar_range(
                self.config.academic_stage,
                start,
                end,
                &self.config.calendar_callback,
            )
            .await
            .map_err(map_registrar_error)
    }

    /// Builds an absolute range whose endpoints are campus-local midnights.
    /// `DateRange::for_date` is intentionally UTC based for the generic domain
    /// API; the live Registrar source needs this explicit local-day variant so
    /// events shortly after 00:00 are not moved into the preceding UTC day.
    fn campus_date_range(&self, date: NaiveDate) -> Result<DateRange, ServiceError> {
        let timezone = self.config.timezone()?;
        let next_date = date
            .succ_opt()
            .ok_or_else(|| adapter_error("campus calendar date is outside the supported range"))?;
        let midnight = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight is valid");
        let start = timezone
            .from_local_datetime(&date.and_time(midnight))
            .single()
            .ok_or_else(|| adapter_error("campus calendar start time could not be localized"))?
            .with_timezone(&Utc);
        let end = timezone
            .from_local_datetime(&next_date.and_time(midnight))
            .single()
            .ok_or_else(|| adapter_error("campus calendar end time could not be localized"))?
            .with_timezone(&Utc);
        DateRange::new(start, end).map_err(|_| adapter_error("campus calendar range is invalid"))
    }

    fn aggregate_overview(
        &self,
        date: NaiveDate,
        courses: Vec<Course>,
        mut schedule: Vec<ScheduleItem>,
        todos: Vec<TodoItem>,
    ) -> Result<CampusOverview, ServiceError> {
        let _ = DateRange::for_date(date)?;
        let timezone = self.config.timezone()?;
        schedule.sort_by_key(|item| item.starts_at);

        let mut today_schedule = schedule
            .iter()
            .filter(|item| item.starts_at.with_timezone(&timezone).date_naive() == date)
            .cloned()
            .collect::<Vec<_>>();
        today_schedule.sort_by_key(|item| item.starts_at);

        let pending_todo_count = todos.iter().filter(|todo| !todo.status.is_closed()).count();
        let completed_todo_count = todos.iter().filter(|todo| todo.status.is_closed()).count();

        let mut upcoming_todos = todos
            .into_iter()
            .filter(|todo| !todo.status.is_closed())
            .collect::<Vec<_>>();
        sort_todos(&mut upcoming_todos);
        upcoming_todos.truncate(5);

        let next_schedule = schedule
            .into_iter()
            .find(|item| item.starts_at.with_timezone(&timezone).date_naive() >= date);

        Ok(CampusOverview {
            date,
            generated_at: Utc::now(),
            semester: Some(self.config.semester.clone()),
            course_count: count_as_u32(courses.len()),
            pending_todo_count: count_as_u32(pending_todo_count),
            completed_todo_count: count_as_u32(completed_todo_count),
            today_schedule,
            upcoming_todos,
            next_schedule,
        })
    }
}

#[async_trait]
impl CampusDataSource for CampusLiveDataSource {
    async fn list_courses(&self) -> Result<Vec<Course>, ServiceError> {
        let records = self.list_learn_course_records().await?;
        Ok(map_learn_courses(&records, &self.config.semester))
    }

    async fn list_schedule(&self, range: DateRange) -> Result<Vec<ScheduleItem>, ServiceError> {
        let records = self.fetch_schedule_records(range.clone()).await?;
        let mut schedule = map_calendar_records(&records, self.config.timezone()?)?;
        schedule.retain(|item| range.overlaps(item.starts_at, item.ends_at));
        Ok(schedule)
    }

    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError> {
        self.todo_source.list_todos(filter).await
    }

    async fn campus_overview(&self, date: NaiveDate) -> Result<CampusOverview, ServiceError> {
        let sections = self.campus_overview_sections(date).await?;
        if !sections.errors.is_empty() {
            return Err(adapter_error("one or more campus overview sections failed"));
        }
        Ok(sections.overview)
    }

    async fn campus_overview_sections(
        &self,
        date: NaiveDate,
    ) -> Result<CampusOverviewSections, ServiceError> {
        let range = self.campus_date_range(date)?;
        let mut errors = CampusOverviewSectionErrors::default();
        let mut recovery = CampusOverviewRecoveryHints::default();
        // Independent, already-proven source branches may overlap. The Learn
        // prerequisite stays sequential with its todos; the shared HTTP gate
        // still controls every actual dispatch and authentication is not part
        // of either branch. Neither failure cancels the other source.
        let learn_branch = async {
            let courses = if self.overview_learn_available {
                self.list_learn_course_records().await
            } else {
                Err(adapter_error("learning service session is not established"))
            };
            let todos = match courses.as_ref() {
                Ok(records) => {
                    self.todo_source
                        .list_todos_for_courses(
                            records.clone(),
                            TodoFilter {
                                include_completed: true,
                                ..TodoFilter::default()
                            },
                        )
                        .await
                }
                Err(_) => Err(adapter_error("learning course prerequisite is unavailable")),
            };
            (courses, todos)
        };
        let calendar_branch = async {
            if self.overview_registrar_available {
                self.list_schedule(range).await
            } else {
                Err(adapter_error(
                    "registrar service session is not established",
                ))
            }
        };
        let ((courses_result, todos_result), schedule_result) =
            join_overview_branches(learn_branch, calendar_branch).await;
        let courses = match courses_result {
            Ok(records) => map_learn_courses(&records, &self.config.semester),
            Err(error) => {
                if error.is_session_expired(ServiceId::Learn) {
                    recovery.learn_session_expired = true;
                }
                errors.courses = Some("unavailable".to_owned());
                Vec::new()
            }
        };
        let schedule = match schedule_result {
            Ok(schedule) => schedule,
            Err(error) => {
                if error.is_session_expired(ServiceId::Registrar) {
                    recovery.registrar_session_expired = true;
                }
                errors.schedule = Some("unavailable".to_owned());
                Vec::new()
            }
        };
        let todos = match todos_result {
            Ok(report) => {
                if report
                    .failures
                    .iter()
                    .any(|failure| failure.kind == CampusTodoFailureKind::SessionExpired)
                {
                    recovery.learn_session_expired = true;
                }
                if let Some(summary) = report.failure_summary() {
                    errors.todos = Some(summary);
                }
                report.items
            }
            Err(error) => {
                if error.is_session_expired(ServiceId::Learn) {
                    recovery.learn_session_expired = true;
                }
                errors.todos = Some("unavailable".to_owned());
                Vec::new()
            }
        };
        Ok(CampusOverviewSections {
            overview: self.aggregate_overview(date, courses, schedule, todos)?,
            errors,
            recovery,
        })
    }

    async fn create_todo(&self, input: NewTodo) -> Result<TodoItem, ServiceError> {
        self.todo_source.create_todo(input).await
    }

    async fn set_todo_status(
        &self,
        id: Uuid,
        status: TodoStatus,
    ) -> Result<TodoItem, ServiceError> {
        self.todo_source.set_todo_status(id, status).await
    }
}

/// Fixed two-branch fan-out, not an unbounded task pool. Each branch retains
/// its own error and all real requests still pass through CampusHttpTransport.
async fn join_overview_branches<A, B>(
    learn: impl std::future::Future<Output = A>,
    calendar: impl std::future::Future<Output = B>,
) -> (A, B) {
    tokio::join!(learn, calendar)
}

/// Maps the stable course subset exposed by `LearnClient` into THYou's domain
/// model. Records without a title are skipped because an invented title would
/// be more misleading than an incomplete course list.
pub fn map_learn_courses(records: &[LearnCourseRecord], semester: &str) -> Vec<Course> {
    let mut courses = records
        .iter()
        .filter_map(|record| {
            let name = record.title()?.trim();
            if name.is_empty() {
                return None;
            }
            let identity = record.code().or(record.id())?.trim();
            if identity.is_empty() {
                return None;
            }
            let code = identity.to_owned();
            Some(Course {
                id: stable_uuid("learn-course", identity),
                code,
                name: name.to_owned(),
                instructor: record
                    .instructor
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                credits: None,
                semester: record
                    .semester
                    .clone()
                    .or_else(|| (!semester.trim().is_empty()).then(|| semester.to_owned())),
                status: CourseStatus::Enrolled,
                meetings: Vec::new(),
            })
        })
        .collect::<Vec<_>>();
    courses.sort_by(|left, right| left.code.cmp(&right.code));
    courses
}

/// Maps registrar JSONP records into UTC domain values using the configured
/// fixed local offset. Registrar events remain opaque beyond their stable
/// fields; unsupported categories are kept as generic events.
pub fn map_calendar_records(
    records: &[RegistrarCalendarRecord],
    timezone: FixedOffset,
) -> Result<Vec<ScheduleItem>, ServiceError> {
    let mut schedule = Vec::new();
    for record in records {
        for event in &record.events {
            let starts_at = timezone
                .from_local_datetime(&event.starts_at)
                .single()
                .ok_or_else(|| adapter_error("registrar start time could not be localized"))?
                .with_timezone(&Utc);
            let ends_at = timezone
                .from_local_datetime(&event.ends_at)
                .single()
                .ok_or_else(|| adapter_error("registrar end time could not be localized"))?
                .with_timezone(&Utc);
            let identity = if let Some(server_id) = event.id.as_deref() {
                format!("{server_id}|{}", event.starts_at)
            } else {
                // THUInfo distinguishes name, location and category. When
                // the server omits an ID, include the full occurrence rather
                // than dropping different rooms/ends at the same start time.
                serde_json::to_string(&(
                    &event.title,
                    event.starts_at,
                    event.ends_at,
                    &event.location,
                    &event.category,
                    &event.course_code,
                    &event.instructor,
                ))
                .map_err(|_| adapter_error("registrar occurrence identity is invalid"))?
            };
            schedule.push(ScheduleItem {
                id: stable_uuid("registrar-event", &identity),
                title: event.title.clone(),
                kind: schedule_kind(event.category.as_deref(), event.course_code.is_some()),
                starts_at,
                ends_at: Some(ends_at),
                all_day: false,
                location: event.location.clone(),
                course_id: event
                    .course_code
                    .as_deref()
                    .map(|code| stable_uuid("learn-course", code)),
                description: event
                    .instructor
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("教师：{value}")),
            });
        }
    }

    let mut seen = HashSet::new();
    schedule.retain(|item| seen.insert(item.id));
    schedule.sort_by_key(|item| item.starts_at);
    Ok(schedule)
}

fn schedule_kind(category: Option<&str>, has_course_code: bool) -> ScheduleKind {
    let category = category.unwrap_or_default().to_ascii_lowercase();
    if category.contains("exam") || category.contains("考试") {
        ScheduleKind::Exam
    } else if category.contains("deadline") || category.contains("作业") {
        ScheduleKind::Deadline
    } else if category.contains("course") || category.contains("课程") || has_course_code {
        ScheduleKind::Course
    } else {
        ScheduleKind::Event
    }
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

fn compare_optional_dates(
    left: Option<DateTime<Utc>>,
    right: Option<DateTime<Utc>>,
) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn count_as_u32(value: usize) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

fn todo_unavailable(operation: &str) -> ServiceError {
    adapter_error(&format!(
        "todo {operation} is unavailable because no verified todo service is configured"
    ))
}

fn adapter_error(message: &str) -> ServiceError {
    ServiceError::Adapter {
        message: message.to_owned(),
    }
}

fn adapter_status_error(operation: &str, status: StatusCode) -> ServiceError {
    ServiceError::Adapter {
        message: format!("{operation} returned HTTP {status}"),
    }
}

fn adapter_error_with_source(operation: &str, _source: impl std::fmt::Display) -> ServiceError {
    // Do not copy an HTTP error body into a user-facing service error.
    adapter_error(operation)
}

fn map_learn_error(error: LearnClientError) -> ServiceError {
    match error {
        LearnClientError::SessionExpired => ServiceError::SessionExpired {
            service: crate::protocol::ServiceId::Learn,
        },
        LearnClientError::Transport(source) => {
            adapter_error_with_source("learning adapter failed", source)
        }
        other => adapter_error_with_source("learning response could not be mapped", other),
    }
}

fn map_learn_announcement_error(error: LearnAnnouncementError) -> ServiceError {
    match error {
        LearnAnnouncementError::SessionExpired
        | LearnAnnouncementError::LearnClient(LearnClientError::SessionExpired) => {
            ServiceError::SessionExpired {
                service: crate::protocol::ServiceId::Learn,
            }
        }
        other => adapter_error_with_source("learning announcement adapter failed", other),
    }
}

fn map_registrar_error(error: RegistrarClientError) -> ServiceError {
    match error {
        RegistrarClientError::Transport(source) => {
            adapter_error_with_source("registrar adapter failed", source)
        }
        RegistrarClientError::LoginExpired { .. }
        | RegistrarClientError::InvalidExamResponse {
            source:
                crate::registrar_client::registrar_exam::RegistrarExamError::AuthenticationRequired,
        } => ServiceError::SessionExpired {
            service: crate::protocol::ServiceId::Registrar,
        },
        other => adapter_error_with_source("registrar response could not be mapped", other),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use serde_json::json;

    use super::*;
    use crate::{
        learn_client::{LearnClient, LearnClientConfig, parse_course_records},
        protocol::CourseRole,
        registrar::CalendarWindow,
        registrar_client::{RegistrarCalendarRecord, RegistrarClientConfig, RegistrarEvent},
        transport::CampusHttpTransport,
    };

    #[test]
    fn maps_courses_without_inventing_records_or_ids() {
        let records = parse_course_records(
            r#"[
                {"courseId": "course-1", "courseCode": "CS-101", "courseName": "程序设计", "teacherName": "教师甲"},
                {"courseId": "course-2", "courseName": "只有名称"}
            ]"#,
        )
        .expect("course fixture parses");

        let courses = map_learn_courses(&records, "2025-2026-2");
        assert_eq!(courses.len(), 2);
        assert_eq!(courses[0].code, "CS-101");
        assert_eq!(courses[0].instructor.as_deref(), Some("教师甲"));
        assert_eq!(courses[1].semester.as_deref(), Some("2025-2026-2"));
        assert!(courses.iter().all(|course| !course.meetings.is_empty()
            || course.name == "只有名称"
            || course.name == "程序设计"));
    }

    #[tokio::test]
    async fn grades_reuse_registrar_cookie_and_send_the_verified_query_shape() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("grade listener");
        let address = listener.local_addr().expect("grade listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("grade request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).expect("grade request read");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).expect("request is UTF-8");
            assert!(
                request
                    .starts_with("GET /cj.cjCjbAll.do?m=bks_cjdcx&cjdlx=zw&flag=di1 HTTP/1.1\r\n")
            );
            assert!(
                request
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("cookie: registrar-session=present"))
            );

            let body = r#"
                <table cellspacing="1">
                  <tr>
                    <th>序号</th><th>课程号</th><th>课程类别</th><th>课程名</th>
                    <th>属性</th><th>学分</th><th>学时</th><th>成绩</th>
                    <th>备注</th><th>绩点</th><th>教师</th><th>学期</th>
                  </tr>
                  <tr>
                    <td>1</td><td>30240512</td><td>必修</td><td>数据结构</td>
                    <td>必修</td><td>3</td><td>48</td><td>88</td>
                    <td></td><td>3.7</td><td>教师甲</td><td>2025-2026-2</td>
                  </tr>
                </table>
            "#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("grade response");
        });

        let base_url = format!("http://{address}/");
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let base = Url::parse(&base_url).expect("base URL");
        transport
            .cookie_jar()
            .add_cookie_str("registrar-session=present; Path=/", &base);
        let registrar = RegistrarClient::from_transport(
            RegistrarClientConfig {
                registrar_base_url: base_url,
                ..RegistrarClientConfig::default()
            },
            transport,
        )
        .expect("registrar client");
        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("campus source");

        let report = source
            .list_grades(RegistrarGradesProfile::undergraduate(
                crate::registrar_academic::UndergraduateReportKind::FirstDegree,
            ))
            .await
            .expect("grade report parses");
        assert_eq!(report.courses.len(), 1);
        assert_eq!(report.courses[0].course_name, "数据结构");
        assert_eq!(report.courses[0].semester, "2025-2026-2");
        server.join().expect("grade server joins");
    }

    #[tokio::test]
    async fn grades_reject_a_same_origin_redirect_that_drops_the_confirmed_query() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("grade redirect listener");
        let address = listener
            .local_addr()
            .expect("grade redirect listener address");
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().expect("initial grade request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = first.read(&mut buffer).expect("initial grade request read");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).expect("initial request is UTF-8");
            assert!(
                request
                    .starts_with("GET /cj.cjCjbAll.do?m=bks_cjdcx&cjdlx=zw&flag=di1 HTTP/1.1\r\n")
            );
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /cj.cjCjbAll.do\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("grade redirect");

            let (mut second, _) = listener.accept().expect("redirected grade request");
            let mut request = Vec::new();
            loop {
                let read = second
                    .read(&mut buffer)
                    .expect("redirected grade request read");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).expect("redirected request is UTF-8");
            assert!(request.starts_with("GET /cj.cjCjbAll.do HTTP/1.1\r\n"));
            let body = r#"
                <table cellspacing="1">
                  <tr>
                    <th>序号</th><th>课程号</th><th>类别</th><th>课程名</th>
                    <th>属性</th><th>学分</th><th>学时</th><th>成绩</th>
                    <th>备注</th><th>绩点</th><th>教师</th><th>学期</th>
                  </tr>
                  <tr>
                    <td>1</td><td>30240512</td><td>必修</td><td>数据结构</td>
                    <td>必修</td><td>3</td><td>48</td><td>88</td>
                    <td></td><td>3.7</td><td>教师甲</td><td>2025-2026-2</td>
                  </tr>
                </table>
            "#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            second
                .write_all(response.as_bytes())
                .expect("redirected grade response");
        });

        let base_url = format!("http://{address}/");
        let registrar = RegistrarClient::from_transport(
            RegistrarClientConfig {
                registrar_base_url: base_url,
                ..RegistrarClientConfig::default()
            },
            CampusHttpTransport::new("THYou/test").expect("transport"),
        )
        .expect("registrar client");
        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("campus source");

        let error = source
            .list_grades(RegistrarGradesProfile::undergraduate(
                crate::registrar_academic::UndergraduateReportKind::FirstDegree,
            ))
            .await
            .expect_err("grade query drift must not become a successful report");
        assert!(matches!(
            error,
            ServiceError::Adapter { message }
                if message == "registrar response could not be mapped"
        ));
        server.join().expect("grade redirect server joins");
    }

    #[tokio::test]
    async fn backend_repair_grades_reject_a_http_200_login_page_after_same_origin_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("grade login listener");
        let address = listener.local_addr().expect("grade login listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("grade request");
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).expect("grade request read");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /cj.cjCjbAll.do?m=bks_cjdcx&cjdlx=zw&flag=di1"));
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /j_acegi_login.do?url=%2F\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut stream, _) = listener.accept().expect("login request");
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).expect("login request read");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /j_acegi_login.do?url=%2F HTTP/1.1"));
            let body = "<html><body>重新登录</body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("login response");
        });

        let base_url = format!("http://{address}/");
        let transport = CampusHttpTransport::new("THYou/test").expect("transport");
        let registrar = RegistrarClient::from_transport(
            RegistrarClientConfig {
                registrar_base_url: base_url,
                ..RegistrarClientConfig::default()
            },
            transport,
        )
        .expect("registrar client");
        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("campus source");

        let error = source
            .list_grades(RegistrarGradesProfile::undergraduate(
                crate::registrar_academic::UndergraduateReportKind::FirstDegree,
            ))
            .await
            .expect_err("login page must not become a missing-grade report");
        assert!(error.is_session_expired(crate::protocol::ServiceId::Registrar));
        server.join().expect("grade login server joins");
    }

    #[tokio::test]
    async fn without_todos_fails_closed_for_reads_and_writes() {
        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let registrar = RegistrarClient::new(Default::default()).expect("registrar client");
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("campus source");

        assert!(matches!(
            source.list_todos(TodoFilter::default()).await,
            Err(ServiceError::Adapter { message })
                if message == "todo read is unavailable because no verified todo service is configured"
        ));
        assert!(matches!(
            source
                .create_todo(NewTodo {
                    title: "fixture write".to_owned(),
                    description: None,
                    priority: crate::domain::TodoPriority::Normal,
                    due_at: None,
                    course_id: None,
                })
                .await,
            Err(ServiceError::Adapter { message })
                if message == "todo create is unavailable because no verified todo service is configured"
        ));
        assert!(matches!(
            source
                .set_todo_status(Uuid::nil(), TodoStatus::Completed)
                .await,
            Err(ServiceError::Adapter { message })
                if message == "todo update is unavailable because no verified todo service is configured"
        ));
    }

    #[test]
    fn maps_registrar_wall_clock_values_with_an_explicit_timezone() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("date");
        let record = RegistrarCalendarRecord {
            stage: AcademicStage::Undergraduate,
            window: CalendarWindow::new(date, date).expect("window"),
            events: vec![RegistrarEvent {
                id: Some("event-1".to_owned()),
                title: "程序设计".to_owned(),
                starts_at: NaiveDateTime::new(date, NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
                ends_at: NaiveDateTime::new(date, NaiveTime::from_hms_opt(10, 35, 0).unwrap()),
                location: Some("六教".to_owned()),
                category: Some("course".to_owned()),
                course_code: Some("CS-101".to_owned()),
                instructor: Some("教师甲".to_owned()),
            }],
        };
        let schedule = map_calendar_records(
            &[record],
            FixedOffset::east_opt(8 * 60 * 60).expect("offset"),
        )
        .expect("schedule maps");

        assert_eq!(schedule.len(), 1);
        assert_eq!(
            schedule[0].starts_at.to_rfc3339(),
            "2026-09-11T01:00:00+00:00"
        );
        assert_eq!(schedule[0].kind, ScheduleKind::Course);
        assert_eq!(schedule[0].location.as_deref(), Some("六教"));
    }

    #[tokio::test]
    async fn list_schedule_filters_events_to_the_requested_campus_day() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("calendar listener");
        let address = listener.local_addr().expect("calendar listener address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("calendar request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).expect("calendar request read");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).expect("calendar request is UTF-8");
            assert!(request.starts_with(
                "GET /jxmh_out.do?m=bks_jxrl_all&p_start_date=20260911&p_end_date=20260911&jsoncallback=thyouCalendar HTTP/1.1\r\n"
            ));
            let body = r#"thyouCalendar([
                {"grrlID":"before","nr":"范围前","nq":"2026-09-10","kssj":"07：00","jssj":"08：00","dd":"A","fl":"course"},
                {"grrlID":"early","nr":"凌晨课程","nq":"2026-09-11","kssj":"00：30","jssj":"01：30","dd":"B","fl":"course"},
                {"grrlID":"inside","nr":"范围内","nq":"2026-09-11","kssj":"10：00","jssj":"11：00","dd":"B","fl":"course"},
                {"grrlID":"after","nr":"范围后","nq":"2026-09-12","kssj":"10：00","jssj":"11：00","dd":"C","fl":"course"}
            ])"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("calendar response");
        });

        let learn = LearnClient::new(
            LearnClientConfig::new(&format!("http://{address}/"), CourseRole::Student)
                .expect("learn config"),
        );
        let registrar = RegistrarClient::new(RegistrarClientConfig {
            registrar_base_url: format!("http://{address}/"),
            ..RegistrarClientConfig::default()
        })
        .expect("registrar client");
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("campus source");
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("range date");
        // The live overview asks for campus-local midnight to campus-local
        // midnight. In UTC this starts on the preceding day, which is exactly
        // the boundary that used to drop the 00:30 event.
        let range = source.campus_date_range(date).expect("campus date range");

        let schedule = source
            .list_schedule(range)
            .await
            .expect("calendar response maps");
        assert_eq!(schedule.len(), 2);
        assert_eq!(schedule[0].title, "凌晨课程");
        assert_eq!(schedule[1].title, "范围内");
        server.join().expect("calendar server joins");
    }

    #[test]
    fn campus_calendar_date_uses_beijing_date_at_utc_boundaries() {
        let previous_local_day = NaiveDate::from_ymd_opt(2026, 9, 11).expect("date");
        let next_local_day = previous_local_day.succ_opt().expect("next date");

        let before_local_midnight = previous_local_day
            .pred_opt()
            .expect("previous date")
            .and_hms_opt(16, 30, 0)
            .expect("time")
            .and_utc();
        let after_local_midnight = previous_local_day
            .and_hms_opt(16, 30, 1)
            .expect("time")
            .and_utc();
        assert_eq!(campus_date_at(before_local_midnight), previous_local_day);
        assert_eq!(campus_date_at(after_local_midnight), next_local_day);

        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        let registrar = RegistrarClient::new(Default::default()).expect("registrar client");
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("source");
        let range = source
            .campus_date_range(previous_local_day)
            .expect("campus date range");
        assert_eq!(range.start.to_rfc3339(), "2026-09-10T16:00:00+00:00");
        assert_eq!(range.end.to_rfc3339(), "2026-09-11T16:00:00+00:00");
    }

    #[test]
    fn overview_filters_by_campus_local_date_at_the_utc_boundary() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("date");
        let record = RegistrarCalendarRecord {
            stage: AcademicStage::Undergraduate,
            window: CalendarWindow::new(date, date).expect("window"),
            events: vec![RegistrarEvent {
                id: Some("early-event".to_owned()),
                title: "早课".to_owned(),
                starts_at: NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 30, 0).unwrap()),
                ends_at: NaiveDateTime::new(date, NaiveTime::from_hms_opt(1, 0, 0).unwrap()),
                location: None,
                category: Some("course".to_owned()),
                course_code: None,
                instructor: None,
            }],
        };
        let schedule = map_calendar_records(
            &[record],
            FixedOffset::east_opt(8 * 60 * 60).expect("offset"),
        )
        .expect("schedule maps");
        assert_eq!(schedule[0].starts_at.date_naive(), date.pred_opt().unwrap());

        let learn = LearnClient::new(
            crate::learn_client::LearnClientConfig::new(
                "https://learn.example.test/",
                CourseRole::Student,
            )
            .expect("learn config"),
        );
        let registrar = RegistrarClient::new(Default::default()).expect("registrar client");
        let source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("source");
        let overview = source
            .aggregate_overview(date, Vec::new(), schedule, Vec::new())
            .expect("overview aggregates");
        assert_eq!(overview.today_schedule.len(), 1);
        assert_eq!(
            overview
                .next_schedule
                .as_ref()
                .map(|item| item.title.as_str()),
            Some("早课")
        );
    }

    #[test]
    fn teacher_config_can_read_both_public_co_course_indexes() {
        let learn = LearnClient::new(
            crate::learn_client::LearnClientConfig::new(
                "https://learn.example.test/",
                CourseRole::Teacher,
            )
            .expect("learn config"),
        );
        let registrar = RegistrarClient::new(Default::default()).expect("registrar client");
        let result = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn config_rejects_noncanonical_semester_ids() {
        let learn = LearnClient::new(
            crate::learn_client::LearnClientConfig::new(
                "https://learn.example.test/",
                CourseRole::Student,
            )
            .expect("learn config"),
        );
        let registrar = RegistrarClient::new(Default::default()).expect("registrar client");
        let result = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2026-fall", AcademicStage::Undergraduate),
        );
        assert!(matches!(result, Err(ServiceError::Adapter { .. })));
    }

    #[test]
    fn learn_csrf_requires_a_learn_binding_and_explicit_query_parameter() {
        let learn = LearnClient::new(
            crate::learn_client::LearnClientConfig::new(
                "https://learn.example.test/",
                CourseRole::Student,
            )
            .expect("learn config"),
        );
        let registrar = RegistrarClient::new(Default::default()).expect("registrar client");
        let mut source = CampusLiveDataSource::without_todos(
            learn,
            registrar,
            CampusLiveConfig::new("2025-2026-2", AcademicStage::Undergraduate),
        )
        .expect("source");
        let registry = crate::session::SessionRegistry::new();
        let foreign = registry.bind_csrf(
            crate::protocol::ServiceId::Info,
            crate::protocol::CsrfToken::new("info-csrf").expect("csrf"),
        );
        assert!(matches!(
            source.with_learn_csrf(foreign, "_csrf"),
            Err(ServiceError::Adapter { .. })
        ));

        let learn_csrf = registry.bind_csrf(
            crate::protocol::ServiceId::Learn,
            crate::protocol::CsrfToken::new("learn-csrf").expect("csrf"),
        );
        source
            .with_learn_csrf(learn_csrf, "_csrf")
            .expect("learn csrf is accepted");
        assert_eq!(source.learn_csrf_parameter.as_deref(), Some("_csrf"));
    }

    #[test]
    fn config_keeps_callback_and_timezone_deployment_values() {
        let config = CampusLiveConfig::new("2025-2026-2", AcademicStage::Graduate)
            .with_language("en")
            .with_course_index("7")
            .with_calendar_callback("calendarCallback")
            .with_calendar_timezone_offset_minutes(9 * 60);
        assert_eq!(config.language, "en");
        assert_eq!(config.course_index.as_deref(), Some("7"));
        assert_eq!(config.calendar_callback, "calendarCallback");
        assert_eq!(config.calendar_timezone_offset_minutes, 9 * 60);
    }

    #[test]
    fn schedule_mapping_deduplicates_stable_event_ids() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("date");
        let event = RegistrarEvent {
            id: Some("same".to_owned()),
            title: "事件".to_owned(),
            starts_at: NaiveDateTime::new(date, NaiveTime::from_hms_opt(8, 0, 0).unwrap()),
            ends_at: NaiveDateTime::new(date, NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
            location: None,
            category: None,
            course_code: None,
            instructor: None,
        };
        let record = RegistrarCalendarRecord {
            stage: AcademicStage::Undergraduate,
            window: CalendarWindow::new(date, date).expect("window"),
            events: vec![event.clone(), event],
        };
        let schedule = map_calendar_records(
            &[record],
            FixedOffset::east_opt(8 * 60 * 60).expect("offset"),
        )
        .expect("schedule maps");
        assert_eq!(schedule.len(), 1);
    }

    #[test]
    fn json_fixture_keeps_unrelated_source_fields_out_of_domain_mapping() {
        let records = parse_course_records(
            &serde_json::to_string(&json!({
                "data": [{"courseId": "course-1", "courseName": "课程", "unknown": {"internal": true}}]
            }))
            .expect("JSON encodes"),
        )
        .expect("course fixture parses");
        let courses = map_learn_courses(&records, "2025-2026-2");
        assert_eq!(courses[0].name, "课程");
        assert!(courses[0].instructor.is_none());
    }
}

#[cfg(test)]
mod aggregation_repairs {
    use super::*;

    #[tokio::test]
    async fn backend_repair_experience_two_branches_overlap_without_losing_partial_failure() {
        let entered = tokio::sync::Notify::new();
        let learn = async {
            entered.notified().await;
            Err::<usize, _>("fixture source unavailable")
        };
        let calendar = async {
            entered.notify_one();
            Ok::<_, &'static str>(3_usize)
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            join_overview_branches(learn, calendar),
        )
        .await
        .expect("both branches are polled");
        assert_eq!(result, (Err("fixture source unavailable"), Ok(3)));
    }

    use crate::{
        learn_client::LearnClientConfig,
        protocol::{CsrfToken, ServiceId},
        registrar::CalendarWindow,
        registrar_client::{RegistrarClientConfig, RegistrarEvent},
        session::SessionCoordinator,
        transport::CampusHttpTransport,
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };

    #[test]
    fn backend_repair_timetable_same_name_start_different_room_is_not_dropped() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        let event = RegistrarEvent {
            id: None,
            title: "fixture title".to_owned(),
            starts_at: date.and_hms_opt(8, 0, 0).unwrap(),
            ends_at: date.and_hms_opt(9, 0, 0).unwrap(),
            location: Some("fixture room A".to_owned()),
            category: Some("course".to_owned()),
            course_code: None,
            instructor: None,
        };
        let mut other = event.clone();
        other.location = Some("fixture room B".to_owned());
        let record = RegistrarCalendarRecord {
            stage: AcademicStage::Undergraduate,
            window: CalendarWindow::new(date, date).unwrap(),
            events: vec![event.clone(), other, event],
        };
        let schedule =
            map_calendar_records(&[record], FixedOffset::east_opt(8 * 3600).unwrap()).unwrap();
        assert_eq!(schedule.len(), 2);
        assert_ne!(schedule[0].id, schedule[1].id);
        assert!(
            schedule
                .iter()
                .all(|item| item.starts_at.to_rfc3339() == "2026-09-14T00:00:00+00:00")
        );
    }

    struct CountingTodos(Arc<AtomicUsize>);
    #[async_trait]
    impl CampusTodoSource for CountingTodos {
        async fn list_todos(&self, _: TodoFilter) -> Result<Vec<TodoItem>, ServiceError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn create_todo(&self, _: NewTodo) -> Result<TodoItem, ServiceError> {
            Err(todo_unavailable("create"))
        }
        async fn set_todo_status(&self, _: Uuid, _: TodoStatus) -> Result<TodoItem, ServiceError> {
            Err(todo_unavailable("update"))
        }
    }

    #[tokio::test]
    async fn backend_repair_overview_course_failure_blocks_dependent_todo_dispatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut b = [0; 8192];
            stream.read(&mut b).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            listener
        });
        let transport = CampusHttpTransport::new("THYou/fixture").unwrap();
        let registrar = RegistrarClient::from_transport(
            RegistrarClientConfig {
                learn_base_url: base.clone(),
                registrar_base_url: base.clone(),
                ..Default::default()
            },
            transport,
        )
        .unwrap();
        let learn = LearnClient::new(LearnClientConfig::new(&base, CourseRole::Student).unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut source = CampusLiveDataSource::new(
            learn,
            registrar,
            CampusLiveConfig::new("2026-2027-1", AcademicStage::Undergraduate),
            Arc::new(CountingTodos(calls.clone())),
        )
        .unwrap();
        let coordinator = SessionCoordinator::new();
        source
            .with_learn_csrf(
                coordinator
                    .registry()
                    .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture").unwrap()),
                "_csrf",
            )
            .unwrap();
        let sections = source
            .scoped_to_services(true, false)
            .campus_overview_sections(NaiveDate::from_ymd_opt(2026, 9, 14).unwrap())
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(sections.errors.courses.is_some());
        assert!(sections.errors.todos.is_some());
        assert!(sections.recovery.learn_session_expired);
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn backend_repair_overview_marks_only_explicit_learn_expiry_for_refresh() {
        for (status, expired) in [(401, true), (503, false)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 8192];
                stream.read(&mut request).unwrap();
                let response = format!(
                    "HTTP/1.1 {status} Failure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(response.as_bytes()).unwrap();
                listener
            });
            let transport = CampusHttpTransport::new("THYou/overview-expiry-fixture").unwrap();
            let registrar = RegistrarClient::from_transport(
                RegistrarClientConfig {
                    learn_base_url: base.clone(),
                    registrar_base_url: base.clone(),
                    ..Default::default()
                },
                transport,
            )
            .unwrap();
            let learn =
                LearnClient::new(LearnClientConfig::new(&base, CourseRole::Student).unwrap());
            let mut source = CampusLiveDataSource::without_todos(
                learn,
                registrar,
                CampusLiveConfig::new("2026-2027-1", AcademicStage::Undergraduate),
            )
            .unwrap();
            let coordinator = SessionCoordinator::new();
            source
                .with_learn_csrf(
                    coordinator
                        .registry()
                        .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture").unwrap()),
                    "_csrf",
                )
                .unwrap();
            let sections = source
                .scoped_to_services(true, false)
                .campus_overview_sections(NaiveDate::from_ymd_opt(2026, 9, 14).unwrap())
                .await
                .unwrap();
            assert_eq!(sections.recovery.learn_session_expired, expired);
            assert!(!sections.recovery.registrar_session_expired);
            let listener = server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }
}
