use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{
    CampusOverview, Course, DateRange, NewTodo, ScheduleItem, TodoFilter, TodoItem, TodoStatus,
};
#[cfg(test)]
use crate::domain::{CourseMeeting, CourseStatus, ScheduleKind, TodoPriority, Weekday};
use crate::error::ServiceError;

/// Per-section status returned by adapters that can provide a partial campus
/// response. Values are stable categories for the application boundary; raw
/// protocol errors stay inside Rust.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampusOverviewSectionErrors {
    pub courses: Option<String>,
    pub schedule: Option<String>,
    pub todos: Option<String>,
    pub services: Option<String>,
    pub updates: Option<String>,
}

impl CampusOverviewSectionErrors {
    pub fn is_empty(&self) -> bool {
        self.courses.is_none()
            && self.schedule.is_none()
            && self.todos.is_none()
            && self.services.is_none()
            && self.updates.is_none()
    }
}

/// A domain overview plus independent section failures. A source can return
/// usable course data even when assignments are unavailable, while callers can
/// still render an explicit todo error instead of mistaking it for an empty
/// list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampusOverviewSections {
    pub overview: CampusOverview,
    pub errors: CampusOverviewSectionErrors,
    /// Typed recovery evidence retained inside Rust.  It is deliberately not
    /// part of the bridge DTO: a section's user-facing error remains stable,
    /// while the runtime can distinguish an explicit authentication expiry
    /// from a transport, parser, or business failure before deciding whether
    /// one bounded service refresh is allowed.
    #[serde(skip)]
    pub(crate) recovery: CampusOverviewRecoveryHints,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CampusOverviewRecoveryHints {
    pub learn_session_expired: bool,
    pub registrar_session_expired: bool,
}

impl CampusOverviewRecoveryHints {
    pub(crate) const fn is_empty(self) -> bool {
        !self.learn_session_expired && !self.registrar_session_expired
    }
}

#[async_trait]
pub trait CampusDataSource: Send + Sync {
    async fn list_courses(&self) -> Result<Vec<Course>, ServiceError>;

    async fn list_schedule(&self, range: DateRange) -> Result<Vec<ScheduleItem>, ServiceError>;

    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError>;

    async fn campus_overview(&self, date: NaiveDate) -> Result<CampusOverview, ServiceError>;

    async fn campus_overview_sections(
        &self,
        date: NaiveDate,
    ) -> Result<CampusOverviewSections, ServiceError> {
        self.campus_overview(date)
            .await
            .map(|overview| CampusOverviewSections {
                overview,
                errors: CampusOverviewSectionErrors::default(),
                recovery: CampusOverviewRecoveryHints::default(),
            })
    }

    async fn create_todo(&self, input: NewTodo) -> Result<TodoItem, ServiceError>;

    async fn set_todo_status(&self, id: Uuid, status: TodoStatus)
    -> Result<TodoItem, ServiceError>;
}

pub type DynCampusDataSource = Arc<dyn CampusDataSource>;

/// Data held by the test-only campus fixture source.
///
/// This type remains public for compatibility with the old replaceable-source
/// API, but it is not a production cache or persistence model.  Production
/// builds reject every operation on [`FixtureCampusService`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FixtureCampusData {
    pub semester: Option<String>,
    pub courses: Vec<Course>,
    pub schedule: Vec<ScheduleItem>,
    pub todos: Vec<TodoItem>,
}

/// Compatibility name for callers that still import the old fixture payload.
#[doc(hidden)]
pub type InMemoryData = FixtureCampusData;

/// An in-memory campus source used only by Rust unit-test fixtures.
///
/// The production library keeps the type available so the legacy Rust API can
/// compile, but all reads, snapshots, overviews, and todo writes return an
/// adapter error there.  This prevents a demo source from becoming a macOS
/// runtime data source through a public trait object or bridge call.
#[cfg_attr(feature = "ffi-bridge", flutter_rust_bridge::frb(ignore))]
#[derive(Debug, Clone)]
pub struct FixtureCampusService {
    state: Arc<RwLock<FixtureCampusData>>,
}

/// Compatibility name for the old public source type.  New code should use
/// [`FixtureCampusService`] so the fixture-only boundary is visible at the
/// call site.
#[doc(hidden)]
pub type InMemoryCampusService = FixtureCampusService;

impl FixtureCampusService {
    pub fn new(data: FixtureCampusData) -> Self {
        Self {
            state: Arc::new(RwLock::new(data)),
        }
    }

    pub fn empty() -> Self {
        Self::new(FixtureCampusData::default())
    }

    /// Creates a deterministic fixture for unit tests.
    pub fn fixture() -> Self {
        Self::fixture_on(Utc::now().date_naive())
    }

    /// Creates a deterministic fixture for unit tests.
    ///
    /// The hard-coded records are compiled only for Rust unit tests.  In a
    /// production build this returns an inert source whose operations fail
    /// closed through a private build-mode check.
    pub fn fixture_on(date: NaiveDate) -> Self {
        #[cfg(test)]
        {
            return Self::new(fixture_data(date));
        }

        #[cfg(not(test))]
        {
            let _ = date;
            Self::empty()
        }
    }

    /// Legacy bridge compatibility constructor.  Use [`Self::fixture`] in
    /// tests; the production implementation is deliberately inert.
    pub fn demo() -> Self {
        Self::fixture()
    }

    /// Legacy bridge compatibility constructor.  Use [`Self::fixture_on`] in
    /// tests; the production implementation is deliberately inert.
    pub fn demo_on(date: NaiveDate) -> Self {
        Self::fixture_on(date)
    }

    pub fn snapshot(&self) -> Result<FixtureCampusData, ServiceError> {
        ensure_fixture_build()?;
        Ok(self.state.read()?.clone())
    }
}

#[async_trait]
impl CampusDataSource for FixtureCampusService {
    async fn list_courses(&self) -> Result<Vec<Course>, ServiceError> {
        ensure_fixture_build()?;
        let mut courses = self.state.read()?.courses.to_vec();
        courses.sort_by(|left, right| left.code.cmp(&right.code));
        Ok(courses)
    }

    async fn list_schedule(&self, range: DateRange) -> Result<Vec<ScheduleItem>, ServiceError> {
        ensure_fixture_build()?;
        let mut schedule = self
            .state
            .read()?
            .schedule
            .iter()
            .filter(|item| range.overlaps(item.starts_at, item.ends_at))
            .cloned()
            .collect::<Vec<_>>();
        schedule.sort_by_key(|item| item.starts_at);
        Ok(schedule)
    }

    async fn list_todos(&self, filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError> {
        ensure_fixture_build()?;
        let mut todos = self
            .state
            .read()?
            .todos
            .iter()
            .filter(|todo| filter.include_completed || !todo.status.is_closed())
            .filter(|todo| {
                filter
                    .course_id
                    .is_none_or(|course_id| todo.course_id == Some(course_id))
            })
            .filter(|todo| {
                filter
                    .due_before
                    .is_none_or(|due_before| todo.due_at.is_some_and(|due_at| due_at < due_before))
            })
            .cloned()
            .collect::<Vec<_>>();
        sort_todos(&mut todos);
        Ok(todos)
    }

    async fn campus_overview(&self, date: NaiveDate) -> Result<CampusOverview, ServiceError> {
        ensure_fixture_build()?;
        let range = DateRange::for_date(date)?;
        let data = self.snapshot()?;

        let mut today_schedule = data
            .schedule
            .iter()
            .filter(|item| range.overlaps(item.starts_at, item.ends_at))
            .cloned()
            .collect::<Vec<_>>();
        today_schedule.sort_by_key(|item| item.starts_at);

        let pending_todo_count = data
            .todos
            .iter()
            .filter(|todo| !todo.status.is_closed())
            .count();
        let completed_todo_count = data
            .todos
            .iter()
            .filter(|todo| todo.status.is_closed())
            .count();

        let mut upcoming_todos = data
            .todos
            .iter()
            .filter(|todo| !todo.status.is_closed())
            .cloned()
            .collect::<Vec<_>>();
        sort_todos(&mut upcoming_todos);
        upcoming_todos.truncate(5);

        let next_schedule = data
            .schedule
            .iter()
            .filter(|item| item.starts_at >= range.start)
            .min_by(|left, right| left.starts_at.cmp(&right.starts_at))
            .cloned();

        Ok(CampusOverview {
            date,
            generated_at: Utc::now(),
            semester: data.semester,
            course_count: count_as_u32(data.courses.len()),
            pending_todo_count: count_as_u32(pending_todo_count),
            completed_todo_count: count_as_u32(completed_todo_count),
            today_schedule,
            upcoming_todos,
            next_schedule,
        })
    }

    async fn create_todo(&self, input: NewTodo) -> Result<TodoItem, ServiceError> {
        ensure_fixture_build()?;
        input.validate()?;
        let now = Utc::now();
        let todo = TodoItem {
            id: Uuid::new_v4(),
            title: input.title.trim().to_owned(),
            description: input.description,
            status: TodoStatus::Pending,
            priority: input.priority,
            due_at: input.due_at,
            course_id: input.course_id,
            created_at: now,
            updated_at: now,
        };

        self.state.write()?.todos.push(todo.clone());
        Ok(todo)
    }

    async fn set_todo_status(
        &self,
        id: Uuid,
        status: TodoStatus,
    ) -> Result<TodoItem, ServiceError> {
        ensure_fixture_build()?;
        let mut state = self.state.write()?;
        let todo = state
            .todos
            .iter_mut()
            .find(|todo| todo.id == id)
            .ok_or(ServiceError::NotFound { entity: "todo", id })?;
        todo.status = status;
        todo.updated_at = Utc::now();
        Ok(todo.clone())
    }
}

fn ensure_fixture_build() -> Result<(), ServiceError> {
    #[cfg(test)]
    {
        Ok(())
    }

    #[cfg(not(test))]
    {
        Err(ServiceError::Adapter {
            message: "in-memory campus fixture is available only in Rust tests".to_owned(),
        })
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

#[cfg(test)]
fn fixture_data(date: NaiveDate) -> FixtureCampusData {
    let course_id = Uuid::from_u128(0x10000000000000000000000000000001);
    let lesson_id = Uuid::from_u128(0x20000000000000000000000000000001);
    let deadline_id = Uuid::from_u128(0x20000000000000000000000000000002);
    let todo_id = Uuid::from_u128(0x30000000000000000000000000000001);
    let completed_todo_id = Uuid::from_u128(0x30000000000000000000000000000002);

    let lesson_start = fixture_at(date, 9, 0);
    let lesson_end = fixture_at(date, 10, 35);
    let deadline_at = fixture_at(date, 18, 0);
    let created_at = fixture_at(date, 8, 0);

    FixtureCampusData {
        semester: Some("2026-fall".to_owned()),
        courses: vec![Course {
            id: course_id,
            code: "FIXTURE-101".to_owned(),
            name: "Fixture course".to_owned(),
            instructor: Some("Fixture instructor".to_owned()),
            credits: Some(2.0),
            semester: Some("2026-fall".to_owned()),
            status: CourseStatus::Enrolled,
            meetings: vec![CourseMeeting {
                weekday: Weekday::Monday,
                start_period: 1,
                end_period: 2,
                weeks: (1..=16).collect(),
                start_time: Some(at_time(9, 0)),
                end_time: Some(at_time(10, 35)),
                location: Some("Fixture room".to_owned()),
            }],
        }],
        schedule: vec![
            ScheduleItem {
                id: lesson_id,
                title: "Fixture course meeting".to_owned(),
                kind: ScheduleKind::Course,
                starts_at: lesson_start,
                ends_at: Some(lesson_end),
                all_day: false,
                location: Some("Fixture room".to_owned()),
                course_id: Some(course_id),
                description: Some("Fixture schedule record".to_owned()),
            },
            ScheduleItem {
                id: deadline_id,
                title: "Fixture deadline".to_owned(),
                kind: ScheduleKind::Deadline,
                starts_at: deadline_at,
                ends_at: None,
                all_day: false,
                location: None,
                course_id: Some(course_id),
                description: None,
            },
        ],
        todos: vec![
            TodoItem {
                id: todo_id,
                title: "Fixture pending todo".to_owned(),
                description: Some("Fixture todo description".to_owned()),
                status: TodoStatus::Pending,
                priority: TodoPriority::High,
                due_at: Some(deadline_at),
                course_id: Some(course_id),
                created_at,
                updated_at: created_at,
            },
            TodoItem {
                id: completed_todo_id,
                title: "Fixture completed todo".to_owned(),
                description: None,
                status: TodoStatus::Completed,
                priority: TodoPriority::Normal,
                due_at: Some(fixture_at(date, 7, 30)),
                course_id: Some(course_id),
                created_at,
                updated_at: created_at,
            },
        ],
    }
}

#[cfg(test)]
fn fixture_at(date: NaiveDate, hour: u32, minute: u32) -> DateTime<Utc> {
    date.and_hms_opt(hour, minute, 0)
        .expect("fixture time is valid")
        .and_utc()
}

#[cfg(test)]
fn at_time(hour: u32, minute: u32) -> chrono::NaiveTime {
    chrono::NaiveTime::from_hms_opt(hour, minute, 0).expect("fixture time is valid")
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::error::DomainError;

    #[tokio::test]
    async fn fixture_overview_contains_today_data_and_round_trips_as_json() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let service = FixtureCampusService::fixture_on(date);

        let overview = service
            .campus_overview(date)
            .await
            .expect("overview succeeds");

        assert_eq!(overview.course_count, 1);
        assert_eq!(overview.pending_todo_count, 1);
        assert_eq!(overview.completed_todo_count, 1);
        assert_eq!(overview.today_schedule.len(), 2);
        assert_eq!(overview.upcoming_todos.len(), 1);

        let encoded = serde_json::to_string(&overview).expect("overview serializes");
        let decoded: CampusOverview =
            serde_json::from_str(&encoded).expect("overview deserializes");
        assert_eq!(decoded, overview);

        let snapshot = service.snapshot().expect("snapshot succeeds");
        let encoded = serde_json::to_string(&snapshot).expect("snapshot serializes");
        let decoded: FixtureCampusData =
            serde_json::from_str(&encoded).expect("snapshot deserializes");
        assert_eq!(decoded, snapshot);
    }

    #[tokio::test]
    async fn fixture_todo_lifecycle_validates_and_updates_the_shared_store() {
        let service = FixtureCampusService::empty();
        let invalid = NewTodo {
            title: "  ".to_owned(),
            description: None,
            priority: TodoPriority::Normal,
            due_at: None,
            course_id: None,
        };
        assert!(matches!(
            service.create_todo(invalid).await,
            Err(ServiceError::Domain(DomainError::EmptyTitle))
        ));

        let created = service
            .create_todo(NewTodo {
                title: "  阅读资料  ".to_owned(),
                description: None,
                priority: TodoPriority::Urgent,
                due_at: None,
                course_id: None,
            })
            .await
            .expect("todo creates");
        assert_eq!(created.title, "阅读资料");

        let updated = service
            .set_todo_status(created.id, TodoStatus::Completed)
            .await
            .expect("todo updates");
        assert_eq!(updated.status, TodoStatus::Completed);

        let open_todos = service
            .list_todos(TodoFilter::default())
            .await
            .expect("todos list");
        assert!(open_todos.is_empty());

        let all_todos = service
            .list_todos(TodoFilter {
                include_completed: true,
                ..TodoFilter::default()
            })
            .await
            .expect("all todos list");
        assert_eq!(all_todos.len(), 1);
    }

    #[tokio::test]
    async fn schedule_query_returns_only_overlapping_items_in_order() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let service = FixtureCampusService::fixture_on(date);
        let range = DateRange::for_date(date).expect("date range succeeds");

        let schedule = service
            .list_schedule(range)
            .await
            .expect("schedule query succeeds");

        assert_eq!(schedule.len(), 2);
        assert!(schedule[0].starts_at <= schedule[1].starts_at);
    }
}
