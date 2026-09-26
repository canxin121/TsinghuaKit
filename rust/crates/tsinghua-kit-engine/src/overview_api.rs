//! Source-aware daily overview built by the shared Rust runtime.
//!
//! This is the existing account-bound overview resolver expressed without
//! protocol selectors or bridge DTOs. A failed section remains explicit.

use chrono::{DateTime, NaiveDate, Utc};

use crate::{
    api::campus::{
        CampusOverviewFacadeDto, CampusOverviewSectionErrorsDto, CampusScheduleDto, CampusTodoDto,
    },
    auth::AccountAuthState,
    error::{Error, ErrorCode, Service},
    read::{
        CacheFreshness, IncompleteReason, ReadCoverage, ReadMetadata, ReadPolicy, ReadResult,
        ReadSource,
    },
};

/// The category of a verified daily schedule row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverviewScheduleKind {
    Course,
    Exam,
    Event,
    Deadline,
    Reminder,
}

/// A schedule row from the authenticated daily overview.
#[derive(Clone, PartialEq, Eq)]
pub struct OverviewSchedule {
    title: String,
    kind: OverviewScheduleKind,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    all_day: bool,
    location: Option<String>,
}

impl OverviewSchedule {
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn kind(&self) -> OverviewScheduleKind {
        self.kind
    }
    pub fn starts_at(&self) -> DateTime<Utc> {
        self.starts_at
    }
    pub fn ends_at(&self) -> Option<DateTime<Utc>> {
        self.ends_at
    }
    pub fn is_all_day(&self) -> bool {
        self.all_day
    }
    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }
}

impl std::fmt::Debug for OverviewSchedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverviewSchedule")
            .field("kind", &self.kind)
            .field("title_present", &!self.title.is_empty())
            .finish()
    }
}

/// A current Learn assignment displayed by the daily overview.
#[derive(Clone, PartialEq, Eq)]
pub struct OverviewTodo {
    title: String,
    due_at: Option<DateTime<Utc>>,
}

impl OverviewTodo {
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn due_at(&self) -> Option<DateTime<Utc>> {
        self.due_at
    }
}

impl std::fmt::Debug for OverviewTodo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverviewTodo")
            .field("title_present", &!self.title.is_empty())
            .field("due_at_present", &self.due_at.is_some())
            .finish()
    }
}

/// Sections that failed independently while another academic section worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OverviewSectionFailures {
    pub courses: bool,
    pub schedule: bool,
    pub todos: bool,
    pub services: bool,
    pub updates: bool,
}

impl OverviewSectionFailures {
    pub fn any(self) -> bool {
        self.courses || self.schedule || self.todos || self.services || self.updates
    }
}

/// A daily result produced by the Rust overview resolver.
#[derive(Clone, PartialEq, Eq)]
pub struct DailyOverview {
    date: NaiveDate,
    semester: Option<String>,
    course_count: u32,
    pending_todo_count: u32,
    completed_todo_count: u32,
    today_schedule: Vec<OverviewSchedule>,
    upcoming_todos: Vec<OverviewTodo>,
    next_schedule: Option<OverviewSchedule>,
    section_failures: OverviewSectionFailures,
}

impl DailyOverview {
    pub fn date(&self) -> NaiveDate {
        self.date
    }
    pub fn semester(&self) -> Option<&str> {
        self.semester.as_deref()
    }
    pub fn course_count(&self) -> u32 {
        self.course_count
    }
    pub fn pending_todo_count(&self) -> u32 {
        self.pending_todo_count
    }
    pub fn completed_todo_count(&self) -> u32 {
        self.completed_todo_count
    }
    pub fn today_schedule(&self) -> &[OverviewSchedule] {
        &self.today_schedule
    }
    pub fn upcoming_todos(&self) -> &[OverviewTodo] {
        &self.upcoming_todos
    }
    pub fn next_schedule(&self) -> Option<&OverviewSchedule> {
        self.next_schedule.as_ref()
    }
    pub fn section_failures(&self) -> OverviewSectionFailures {
        self.section_failures
    }
}

impl std::fmt::Debug for DailyOverview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DailyOverview")
            .field("date", &self.date)
            .field("schedule_count", &self.today_schedule.len())
            .field("todo_count", &self.upcoming_todos.len())
            .field("section_failures", &self.section_failures)
            .finish()
    }
}

/// Account-bound overview reads on the owning Client.
pub struct OverviewClient<'client> {
    pub(crate) runtime: &'client mut crate::api::runtime::CampusRuntime,
    pub(crate) cache_source: ReadSource,
}

impl OverviewClient<'_> {
    /// Reads the requested campus day under an explicit cache policy.
    /// `CacheOnly` performs no network request; `Refresh` requires a live
    /// result, while `RefreshOrCached` permits a labeled stale fallback.
    pub async fn day(
        &mut self,
        date: NaiveDate,
        policy: ReadPolicy,
    ) -> Result<ReadResult<DailyOverview>, Error> {
        let date_text = date.format("%Y-%m-%d").to_string();
        let result = match policy {
            ReadPolicy::CacheOnly => self
                .runtime
                .peek_overview(date_text)
                .map_err(|_| overview_failure(self.runtime))?
                .ok_or_else(|| Error::new(Service::Overview, ErrorCode::CacheMiss))?,
            ReadPolicy::PreferFreshCache => self
                .runtime
                .load_overview(date_text)
                .await
                .map_err(|_| overview_failure(self.runtime))?,
            ReadPolicy::Refresh | ReadPolicy::RefreshOrCached => self
                .runtime
                .refresh_overview(date_text)
                .await
                .map_err(|_| overview_failure(self.runtime))?,
        };
        if result.status == "error" {
            return Err(Error::new(
                Service::Overview,
                if matches!(policy, ReadPolicy::CacheOnly) {
                    ErrorCode::CacheMiss
                } else {
                    ErrorCode::ServiceUnavailable
                },
            ));
        }
        if matches!(policy, ReadPolicy::Refresh) && result.source != "live" {
            return Err(Error::new(Service::Overview, ErrorCode::ServiceUnavailable));
        }
        map_overview(result, date, self.cache_source, policy)
    }
}

fn invalid() -> Error {
    Error::new(Service::Overview, ErrorCode::InvalidResponse)
}

fn overview_failure(runtime: &crate::api::runtime::CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Overview, code)
}

fn safe_text(value: &str, allow_empty: bool) -> bool {
    (allow_empty || !value.trim().is_empty())
        && value.len() <= 4096
        && !value.chars().any(char::is_control)
}

fn instant(value: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| invalid())
}

fn schedule(value: CampusScheduleDto) -> Result<OverviewSchedule, Error> {
    let kind = match value.kind.as_str() {
        "course" => OverviewScheduleKind::Course,
        "exam" => OverviewScheduleKind::Exam,
        "event" => OverviewScheduleKind::Event,
        "deadline" => OverviewScheduleKind::Deadline,
        "reminder" => OverviewScheduleKind::Reminder,
        _ => return Err(invalid()),
    };
    if !safe_text(&value.title, false)
        || !value
            .location
            .as_deref()
            .is_none_or(|text| safe_text(text, true))
    {
        return Err(invalid());
    }
    let starts_at = instant(&value.starts_at)?;
    let ends_at = value.ends_at.as_deref().map(instant).transpose()?;
    if ends_at.is_some_and(|end| end < starts_at) {
        return Err(invalid());
    }
    Ok(OverviewSchedule {
        title: value.title,
        kind,
        starts_at,
        ends_at,
        all_day: value.all_day,
        location: value.location,
    })
}

fn todo(value: CampusTodoDto) -> Result<OverviewTodo, Error> {
    if !safe_text(&value.title, false)
        || !matches!(value.status.as_str(), "pending" | "in_progress")
        || !matches!(
            value.priority.as_str(),
            "low" | "normal" | "high" | "urgent"
        )
    {
        return Err(invalid());
    }
    Ok(OverviewTodo {
        title: value.title,
        due_at: value.due_at.as_deref().map(instant).transpose()?,
    })
}

fn section_failures(value: Option<CampusOverviewSectionErrorsDto>) -> OverviewSectionFailures {
    let Some(value) = value else {
        return OverviewSectionFailures::default();
    };
    OverviewSectionFailures {
        courses: value.courses.is_some(),
        schedule: value.schedule.is_some(),
        todos: value.todos.is_some(),
        services: value.services.is_some(),
        updates: value.updates.is_some(),
    }
}

fn map_overview(
    value: CampusOverviewFacadeDto,
    requested: NaiveDate,
    cache_source: ReadSource,
    policy: ReadPolicy,
) -> Result<ReadResult<DailyOverview>, Error> {
    if value.requested_date != requested.format("%Y-%m-%d").to_string() {
        return Err(invalid());
    }
    let data = value.overview.ok_or_else(invalid)?;
    if data.date != value.requested_date || data.date != requested.format("%Y-%m-%d").to_string() {
        return Err(invalid());
    }
    if !data
        .semester
        .as_deref()
        .is_none_or(|text| safe_text(text, false))
    {
        return Err(invalid());
    }
    let observed_at = instant(&data.generated_at)?;
    if observed_at > Utc::now() + chrono::Duration::minutes(5) {
        return Err(invalid());
    }
    let (source, freshness) = match (value.source.as_str(), value.status.as_str()) {
        ("live", "ready") if value.error.is_none() => {
            (ReadSource::Live, CacheFreshness::NotApplicable)
        }
        ("cache", "ready") if value.error.is_none() => (cache_source, CacheFreshness::Fresh),
        ("cache", "stale") => (cache_source, CacheFreshness::Stale),
        _ => return Err(invalid()),
    };
    let today_schedule = data
        .today_schedule
        .into_iter()
        .map(schedule)
        .collect::<Result<Vec<_>, _>>()?;
    let upcoming_todos = data
        .upcoming_todos
        .into_iter()
        .map(todo)
        .collect::<Result<Vec<_>, _>>()?;
    let next_schedule = data.next_schedule.map(schedule).transpose()?;
    let section_failures = section_failures(value.section_errors);
    let refresh_failure = if source != ReadSource::Live
        && !matches!(policy, ReadPolicy::CacheOnly)
        && matches!(freshness, CacheFreshness::Stale)
    {
        Some(ErrorCode::ServiceUnavailable)
    } else {
        None
    };
    let metadata = ReadMetadata::new(
        source,
        freshness,
        observed_at,
        if section_failures.any() {
            ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed)
        } else {
            ReadCoverage::Complete
        },
        refresh_failure,
    );
    Ok(ReadResult::new(
        DailyOverview {
            date: requested,
            semester: data.semester,
            course_count: data.course_count,
            pending_todo_count: data.pending_todo_count,
            completed_todo_count: data.completed_todo_count,
            today_schedule,
            upcoming_todos,
            next_schedule,
            section_failures,
        },
        metadata,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::campus::{CampusOverviewDto, CampusOverviewFacadeDto};

    fn fixture(source: &str, status: &str) -> CampusOverviewFacadeDto {
        CampusOverviewFacadeDto {
            requested_date: "2026-09-26".to_owned(),
            source: source.to_owned(),
            status: status.to_owned(),
            overview: Some(CampusOverviewDto {
                date: "2026-09-26".to_owned(),
                generated_at: Utc::now().to_rfc3339(),
                semester: Some("2026-2027-1".to_owned()),
                course_count: 2,
                pending_todo_count: 1,
                completed_todo_count: 0,
                today_schedule: vec![],
                upcoming_todos: vec![],
                next_schedule: None,
            }),
            error: None,
            section_errors: None,
        }
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 26).unwrap()
    }

    #[test]
    fn partial_section_is_labeled_and_not_persisted_as_complete() {
        let mut value = fixture("live", "ready");
        value.section_errors = Some(CampusOverviewSectionErrorsDto {
            courses: Some("internal detail stays private".to_owned()),
            schedule: None,
            todos: None,
            services: None,
            updates: None,
        });
        let result =
            map_overview(value, date(), ReadSource::ClientCache, ReadPolicy::Refresh).unwrap();
        assert!(result.data().section_failures().courses);
        assert_eq!(result.metadata().source(), ReadSource::Live);
        assert_eq!(
            result.metadata().coverage(),
            ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed)
        );
        assert!(!format!("{result:?}").contains("internal detail"));
    }

    #[test]
    fn stale_cache_only_has_no_refresh_failure_but_fallback_does() {
        let cached = map_overview(
            fixture("cache", "stale"),
            date(),
            ReadSource::PersistentCache,
            ReadPolicy::CacheOnly,
        )
        .unwrap();
        assert_eq!(cached.metadata().source(), ReadSource::PersistentCache);
        assert_eq!(cached.metadata().freshness(), CacheFreshness::Stale);
        assert_eq!(cached.metadata().refresh_failure(), None);

        let fallback = map_overview(
            fixture("cache", "stale"),
            date(),
            ReadSource::PersistentCache,
            ReadPolicy::RefreshOrCached,
        )
        .unwrap();
        assert_eq!(
            fallback.metadata().refresh_failure(),
            Some(ErrorCode::ServiceUnavailable)
        );
    }

    #[test]
    fn a_different_requested_day_is_rejected() {
        let error = map_overview(
            fixture("cache", "ready"),
            NaiveDate::from_ymd_opt(2026, 9, 27).unwrap(),
            ReadSource::ClientCache,
            ReadPolicy::CacheOnly,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn zero_length_event_remains_compatible_with_existing_overview_rows() {
        let mut value = fixture("live", "ready");
        value
            .overview
            .as_mut()
            .unwrap()
            .today_schedule
            .push(CampusScheduleDto {
                id: "event-1".to_owned(),
                title: "全天事件".to_owned(),
                kind: "event".to_owned(),
                starts_at: "2026-09-26T00:00:00Z".to_owned(),
                ends_at: Some("2026-09-26T00:00:00Z".to_owned()),
                all_day: true,
                location: None,
                course_id: None,
                description: None,
            });
        let result =
            map_overview(value, date(), ReadSource::ClientCache, ReadPolicy::Refresh).unwrap();
        assert_eq!(result.data().today_schedule().len(), 1);
    }
}
