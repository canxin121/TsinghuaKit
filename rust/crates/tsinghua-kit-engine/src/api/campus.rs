use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::NaiveDate;
#[cfg(feature = "ffi-bridge")]
use flutter_rust_bridge::frb;

use crate::cache::{CacheError, JsonCacheEnvelope, JsonFileCache};
use crate::domain::{
    CampusOverview, ScheduleItem, ScheduleKind, TodoItem, TodoPriority, TodoStatus,
};
#[cfg(test)]
use crate::services::InMemoryCampusService;
use crate::services::{
    CampusOverviewRecoveryHints, CampusOverviewSectionErrors, CampusOverviewSections,
    DynCampusDataSource,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusScheduleDto {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub starts_at: String,
    pub ends_at: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
    pub course_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusTodoDto {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub due_at: Option<String>,
    pub course_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusOverviewDto {
    pub date: String,
    pub generated_at: String,
    pub semester: Option<String>,
    pub course_count: u32,
    pub pending_todo_count: u32,
    pub completed_todo_count: u32,
    pub today_schedule: Vec<CampusScheduleDto>,
    pub upcoming_todos: Vec<CampusTodoDto>,
    pub next_schedule: Option<CampusScheduleDto>,
}

/// The complete, verified schedule for the selected academic semester.
///
/// Unlike [`CampusOverviewDto`], this payload is not keyed by a requested day.
/// The Rust runtime fetches the entire semester through the authenticated
/// Registrar session and Flutter only filters/layouts these already-proven
/// occurrences for the selected week.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusSemesterScheduleDto {
    pub semester: String,
    pub stage: String,
    pub first_day: String,
    pub last_day: String,
    pub week_count: u32,
    pub current_week: u32,
    pub generated_at: String,
    pub source: String,
    pub status: String,
    pub error: Option<String>,
    pub events: Vec<CampusScheduleDto>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusOverviewSectionErrorsDto {
    pub courses: Option<String>,
    pub schedule: Option<String>,
    pub todos: Option<String>,
    pub services: Option<String>,
    pub updates: Option<String>,
}

/// Identifies where an overview was obtained. These values are deliberately kept
/// separate from the overview payload so cached data cannot be mistaken for a
/// live campus response by a bridge consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CampusOverviewSource {
    Live,
    Cache,
}

impl CampusOverviewSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Cache => "cache",
        }
    }
}

/// Describes whether the data can be shown as current, only as a stale fallback,
/// or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CampusOverviewStatus {
    Ready,
    Stale,
    Error,
}

impl CampusOverviewStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Error => "error",
        }
    }
}

/// A live overview needs at least one usable academic core section.  The
/// courses and schedule sections are the two core inputs; when both fail, the
/// aggregate contains no trustworthy academic data and must not be exposed as
/// a successful overview with two empty lists.
const CORE_OVERVIEW_SECTIONS_FAILURE: &str = "overview core sections failed: courses and schedule";

fn fatal_core_section_error(errors: &CampusOverviewSectionErrors) -> Option<&'static str> {
    (errors.courses.is_some() && errors.schedule.is_some())
        .then_some(CORE_OVERVIEW_SECTIONS_FAILURE)
}

/// Stable bridge envelope for a source-backed overview.
///
/// `source` and `status` are wire strings on purpose: the bridge can expose them
/// without making Flutter understand Rust protocol or cache types. A stale
/// cache still carries `overview`; an error carries a stable message and no
/// overview. The application runtime supplies the live adapter through its
/// authenticated opaque API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusOverviewFacadeDto {
    pub requested_date: String,
    pub source: String,
    pub status: String,
    pub overview: Option<CampusOverviewDto>,
    pub error: Option<String>,
    pub section_errors: Option<CampusOverviewSectionErrorsDto>,
}

/// A reusable live-first overview resolver with a typed JSON cache.
///
/// The resolver is independent of Flutter and can be constructed by a Rust
/// application shell once authentication has produced a live data source. The
/// legacy bridge fixture remains demo-backed, while `CampusRuntime` owns the
/// live resolver used by the application.
#[cfg_attr(feature = "ffi-bridge", frb(ignore))]
pub struct CampusOverviewResolver {
    live: DynCampusDataSource,
    cache: JsonFileCache<CampusOverview>,
    max_age: Duration,
}

impl CampusOverviewResolver {
    pub const CACHE_SCHEMA_VERSION: u32 = 1;

    pub fn new(
        live: DynCampusDataSource,
        cache_path: impl AsRef<std::path::Path>,
        max_age: Duration,
    ) -> Self {
        Self {
            live,
            cache: JsonFileCache::new(cache_path, Self::CACHE_SCHEMA_VERSION, "overview"),
            max_age,
        }
    }

    pub fn with_cache(
        live: DynCampusDataSource,
        cache: JsonFileCache<CampusOverview>,
        max_age: Duration,
    ) -> Self {
        Self {
            live,
            cache,
            max_age,
        }
    }

    pub fn cache(&self) -> &JsonFileCache<CampusOverview> {
        &self.cache
    }

    pub async fn load(&self, requested_date: NaiveDate) -> CampusOverviewFacadeDto {
        self.load_at(requested_date, SystemTime::now()).await
    }

    pub(crate) async fn load_at(
        &self,
        requested_date: NaiveDate,
        now: SystemTime,
    ) -> CampusOverviewFacadeDto {
        CampusOverviewFacade::load_live_then_cache(
            Arc::clone(&self.live),
            &self.cache,
            requested_date,
            now,
            self.max_age,
        )
        .await
    }
}

/// Source-aware aggregation boundary shared by live adapters and the cache
/// fallback.
///
/// The facade consumes the existing [`CampusDataSource`] contract; it does not
/// know any campus HTTP route or authentication detail. The constructors make
/// provenance explicit so a cache is never labelled as live by default.
pub(crate) struct CampusOverviewFacade {
    source: DynCampusDataSource,
    source_kind: CampusOverviewSource,
}

impl CampusOverviewFacade {
    /// Creates a facade for an adapter that represents a live campus source.
    #[allow(dead_code)]
    pub(crate) fn live(source: DynCampusDataSource) -> Self {
        Self {
            source,
            source_kind: CampusOverviewSource::Live,
        }
    }

    /// Loads and maps an overview through the existing data-source trait.
    ///
    /// Source failures are represented in the returned DTO so callers can render
    /// an error state without confusing it with an empty successful overview.
    pub(crate) async fn load(&self, date: NaiveDate) -> CampusOverviewFacadeDto {
        self.load_with_payload(date).await.0
    }

    /// Loads a live overview and, when every section is complete, also returns
    /// the normalized domain payload that may be persisted by an account-bound
    /// runtime cache.  Keeping the payload beside the DTO avoids reconstructing
    /// UUIDs and timestamps from bridge strings and makes it impossible for a
    /// partial live response to be written as a complete cache entry.
    pub(crate) async fn load_with_payload(
        &self,
        date: NaiveDate,
    ) -> (CampusOverviewFacadeDto, Option<CampusOverview>) {
        let (result, payload, _) = self.load_with_payload_and_recovery(date).await;
        (result, payload)
    }

    /// Same live read as [`Self::load_with_payload`], while retaining typed
    /// authentication-expiry evidence for the Rust runtime.  The evidence is
    /// intentionally kept out of the Flutter DTO so a service can be retried
    /// only when its adapter supplied explicit `SessionExpired` proof.
    pub(crate) async fn load_with_payload_and_recovery(
        &self,
        date: NaiveDate,
    ) -> (
        CampusOverviewFacadeDto,
        Option<CampusOverview>,
        CampusOverviewRecoveryHints,
    ) {
        match self.source.campus_overview_sections(date).await {
            Ok(sections) if sections.overview.date == date => {
                let recovery = sections.recovery;
                if let Some(error) = fatal_core_section_error(&sections.errors) {
                    (
                        CampusOverviewFacadeDto::error(date, self.source_kind, error.to_owned()),
                        None,
                        recovery,
                    )
                } else {
                    // `ready` plus `section_errors` is the explicit partial
                    // live-response semantic.  Non-core failures, and a
                    // single core failure while the other core is usable, are
                    // retained for section-level rendering.
                    let cache_payload = sections
                        .errors
                        .is_empty()
                        .then(|| sections.overview.clone());
                    (
                        CampusOverviewFacadeDto::ready_with_sections(
                            date,
                            self.source_kind,
                            sections,
                        ),
                        cache_payload,
                        recovery,
                    )
                }
            }
            Ok(sections) => (
                CampusOverviewFacadeDto::error(
                    date,
                    self.source_kind,
                    format!(
                        "overview date mismatch: requested {date}, received {}",
                        sections.overview.date
                    ),
                ),
                None,
                CampusOverviewRecoveryHints::default(),
            ),
            Err(error) => (
                CampusOverviewFacadeDto::error(date, self.source_kind, error.to_string()),
                None,
                CampusOverviewRecoveryHints::default(),
            ),
        }
    }

    /// Maps an already-read cache envelope into the same bridge shape.
    ///
    /// `JsonFileCache` performs schema and service validation before returning an
    /// envelope.  This method only evaluates the payload date and the envelope's
    /// Unix-millisecond `saved_at` value, which keeps freshness policy out of the
    /// cache implementation and makes it deterministic in tests.
    #[allow(dead_code)]
    pub(crate) fn from_cache(
        requested_date: NaiveDate,
        cached: Result<Option<JsonCacheEnvelope<CampusOverview>>, CacheError>,
        now: SystemTime,
        max_age: Duration,
    ) -> CampusOverviewFacadeDto {
        let envelope = match cached {
            Ok(Some(envelope)) => envelope,
            Ok(None) => {
                return CampusOverviewFacadeDto::error(
                    requested_date,
                    CampusOverviewSource::Cache,
                    "cache miss".to_owned(),
                );
            }
            Err(error) => {
                return CampusOverviewFacadeDto::error(
                    requested_date,
                    CampusOverviewSource::Cache,
                    error.to_string(),
                );
            }
        };

        if envelope.payload.date != requested_date {
            return CampusOverviewFacadeDto::error(
                requested_date,
                CampusOverviewSource::Cache,
                format!(
                    "cached overview date mismatch: requested {requested_date}, found {}",
                    envelope.payload.date
                ),
            );
        }

        let saved_at_millis = match envelope.saved_at.parse::<u128>() {
            Ok(value) => value,
            Err(error) => {
                return CampusOverviewFacadeDto::error(
                    requested_date,
                    CampusOverviewSource::Cache,
                    format!("invalid cache saved_at `{}`: {error}", envelope.saved_at),
                );
            }
        };
        let now_millis = match now.duration_since(UNIX_EPOCH) {
            Ok(value) => value.as_millis(),
            Err(error) => {
                return CampusOverviewFacadeDto::error(
                    requested_date,
                    CampusOverviewSource::Cache,
                    format!("cache freshness clock is before Unix epoch: {error}"),
                );
            }
        };
        if saved_at_millis > now_millis {
            return CampusOverviewFacadeDto::error(
                requested_date,
                CampusOverviewSource::Cache,
                "cache saved_at is in the future".to_owned(),
            );
        }

        let age = Duration::from_millis(
            (now_millis - saved_at_millis)
                .min(u64::MAX as u128)
                .try_into()
                .expect("bounded cache age fits in u64"),
        );
        let status = if age > max_age {
            CampusOverviewStatus::Stale
        } else {
            CampusOverviewStatus::Ready
        };

        CampusOverviewFacadeDto::with_status(
            requested_date,
            CampusOverviewSource::Cache,
            status,
            Some(CampusOverviewDto::from(envelope.payload)),
            None,
            None,
        )
    }

    /// Tries the live source first and falls back to a validated cache entry.
    ///
    /// A successful live response remains usable even if writing the cache
    /// fails.  Cache errors are only relevant after the live request has
    /// already failed, so a transient filesystem problem cannot turn a live
    /// campus response into an error state.
    #[allow(dead_code)]
    pub(crate) async fn load_live_then_cache(
        live: DynCampusDataSource,
        cache: &JsonFileCache<CampusOverview>,
        requested_date: NaiveDate,
        now: SystemTime,
        max_age: Duration,
    ) -> CampusOverviewFacadeDto {
        let live_failure = match live.campus_overview_sections(requested_date).await {
            Ok(sections) if sections.overview.date == requested_date => {
                if let Some(error) = fatal_core_section_error(&sections.errors) {
                    // Treat a two-core partial as a failed live attempt.  This
                    // allows a validated cache to be used, while preventing
                    // the partial live aggregate from being returned as ready.
                    error.to_owned()
                } else {
                    let has_section_errors = !sections.errors.is_empty();
                    if !has_section_errors {
                        let _ = cache.write(&sections.overview);
                    }
                    // `ready` with a non-empty `section_errors` is intentional:
                    // the core aggregate is usable and the failed non-core
                    // section remains visible to the caller as partial data.
                    return CampusOverviewFacadeDto::ready_with_sections(
                        requested_date,
                        CampusOverviewSource::Live,
                        sections,
                    );
                }
            }
            Ok(sections) => format!(
                "overview date mismatch: requested {requested_date}, received {}",
                sections.overview.date
            ),
            Err(error) => error.to_string(),
        };

        let cached = CampusOverviewFacade::from_cache(requested_date, cache.read(), now, max_age);
        if cached.status != CampusOverviewStatus::Error.as_str() {
            return cached;
        }

        CampusOverviewFacadeDto::error(
            requested_date,
            CampusOverviewSource::Live,
            format!(
                "live source failed: {live_failure}; cache fallback failed: {}",
                cached.error.as_deref().unwrap_or("unknown cache error")
            ),
        )
    }
}

impl CampusOverviewFacadeDto {
    #[allow(dead_code)]
    fn ready(
        requested_date: NaiveDate,
        source: CampusOverviewSource,
        overview: CampusOverviewDto,
    ) -> Self {
        Self::with_status(
            requested_date,
            source,
            CampusOverviewStatus::Ready,
            Some(overview),
            None,
            None,
        )
    }

    fn ready_with_sections(
        requested_date: NaiveDate,
        source: CampusOverviewSource,
        sections: CampusOverviewSections,
    ) -> Self {
        let section_errors = (!sections.errors.is_empty())
            .then(|| CampusOverviewSectionErrorsDto::from(sections.errors));
        Self::with_status(
            requested_date,
            source,
            CampusOverviewStatus::Ready,
            Some(CampusOverviewDto::from(sections.overview)),
            None,
            section_errors,
        )
    }

    fn error(requested_date: NaiveDate, source: CampusOverviewSource, error: String) -> Self {
        Self::with_status(
            requested_date,
            source,
            CampusOverviewStatus::Error,
            None,
            Some(error),
            None,
        )
    }

    fn with_status(
        requested_date: NaiveDate,
        source: CampusOverviewSource,
        status: CampusOverviewStatus,
        overview: Option<CampusOverviewDto>,
        error: Option<String>,
        section_errors: Option<CampusOverviewSectionErrorsDto>,
    ) -> Self {
        Self {
            requested_date: requested_date.to_string(),
            source: source.as_str().to_owned(),
            status: status.as_str().to_owned(),
            overview,
            error,
            section_errors,
        }
    }
}

impl From<CampusOverviewSectionErrors> for CampusOverviewSectionErrorsDto {
    fn from(value: CampusOverviewSectionErrors) -> Self {
        Self {
            courses: value.courses,
            schedule: value.schedule,
            todos: value.todos,
            services: value.services,
            updates: value.updates,
        }
    }
}

/// Legacy compatibility symbol retained only because `crate::api` historically
/// re-exported it. It is explicitly excluded from the Flutter bridge and can
/// never return fixture data. Production callers must use the authenticated
/// `CampusRuntime::load_overview` method.
#[cfg_attr(feature = "ffi-bridge", frb(ignore))]
pub async fn load_overview(_date: String) -> Result<CampusOverviewFacadeDto, String> {
    Err("legacy campus overview bridge is unavailable; use CampusRuntime::load_overview".to_owned())
}

impl From<CampusOverview> for CampusOverviewDto {
    fn from(value: CampusOverview) -> Self {
        Self {
            date: value.date.to_string(),
            generated_at: value.generated_at.to_rfc3339(),
            semester: value.semester,
            course_count: value.course_count,
            pending_todo_count: value.pending_todo_count,
            completed_todo_count: value.completed_todo_count,
            today_schedule: value
                .today_schedule
                .into_iter()
                .map(CampusScheduleDto::from)
                .collect(),
            upcoming_todos: value
                .upcoming_todos
                .into_iter()
                .map(CampusTodoDto::from)
                .collect(),
            next_schedule: value.next_schedule.map(CampusScheduleDto::from),
        }
    }
}

impl From<ScheduleItem> for CampusScheduleDto {
    fn from(value: ScheduleItem) -> Self {
        Self {
            id: value.id.to_string(),
            title: value.title,
            kind: schedule_kind_name(value.kind).to_owned(),
            starts_at: value.starts_at.to_rfc3339(),
            ends_at: value.ends_at.map(|value| value.to_rfc3339()),
            all_day: value.all_day,
            location: value.location,
            course_id: value.course_id.map(|value| value.to_string()),
            description: value.description,
        }
    }
}

impl From<TodoItem> for CampusTodoDto {
    fn from(value: TodoItem) -> Self {
        Self {
            id: value.id.to_string(),
            title: value.title,
            description: value.description,
            status: todo_status_name(value.status).to_owned(),
            priority: todo_priority_name(value.priority).to_owned(),
            due_at: value.due_at.map(|value| value.to_rfc3339()),
            course_id: value.course_id.map(|value| value.to_string()),
            created_at: value.created_at.to_rfc3339(),
            updated_at: value.updated_at.to_rfc3339(),
        }
    }
}

fn schedule_kind_name(kind: ScheduleKind) -> &'static str {
    match kind {
        ScheduleKind::Course => "course",
        ScheduleKind::Exam => "exam",
        ScheduleKind::Event => "event",
        ScheduleKind::Deadline => "deadline",
        ScheduleKind::Reminder => "reminder",
    }
}

fn todo_status_name(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in_progress",
        TodoStatus::Completed => "completed",
        TodoStatus::Archived => "archived",
    }
}

fn todo_priority_name(priority: TodoPriority) -> &'static str {
    match priority {
        TodoPriority::Low => "low",
        TodoPriority::Normal => "normal",
        TodoPriority::High => "high",
        TodoPriority::Urgent => "urgent",
    }
}

#[cfg(feature = "ffi-bridge")]
#[cfg_attr(feature = "ffi-bridge", frb(init))]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use chrono::Utc;
    use uuid::Uuid;

    use crate::domain::{
        Course, DateRange, NewTodo, ScheduleItem, TodoFilter, TodoItem, TodoStatus,
    };
    use crate::error::ServiceError;
    use crate::services::CampusDataSource;

    use super::*;

    #[derive(Debug, Clone)]
    struct SectionsFixtureSource {
        sections: CampusOverviewSections,
    }

    impl SectionsFixtureSource {
        fn new(sections: CampusOverviewSections) -> Self {
            Self { sections }
        }
    }

    fn unused_fixture_operation() -> ServiceError {
        ServiceError::Adapter {
            message: "overview section fixture operation is unused".to_owned(),
        }
    }

    #[async_trait::async_trait]
    impl CampusDataSource for SectionsFixtureSource {
        async fn list_courses(&self) -> Result<Vec<Course>, ServiceError> {
            Err(unused_fixture_operation())
        }

        async fn list_schedule(
            &self,
            _range: DateRange,
        ) -> Result<Vec<ScheduleItem>, ServiceError> {
            Err(unused_fixture_operation())
        }

        async fn list_todos(&self, _filter: TodoFilter) -> Result<Vec<TodoItem>, ServiceError> {
            Err(unused_fixture_operation())
        }

        async fn campus_overview(&self, _date: NaiveDate) -> Result<CampusOverview, ServiceError> {
            Ok(self.sections.overview.clone())
        }

        async fn campus_overview_sections(
            &self,
            _date: NaiveDate,
        ) -> Result<CampusOverviewSections, ServiceError> {
            Ok(self.sections.clone())
        }

        async fn create_todo(&self, _input: NewTodo) -> Result<TodoItem, ServiceError> {
            Err(unused_fixture_operation())
        }

        async fn set_todo_status(
            &self,
            _id: Uuid,
            _status: TodoStatus,
        ) -> Result<TodoItem, ServiceError> {
            Err(unused_fixture_operation())
        }
    }

    async fn fixture_sections(
        date: NaiveDate,
        errors: CampusOverviewSectionErrors,
    ) -> CampusOverviewSections {
        let overview = InMemoryCampusService::fixture_on(date)
            .campus_overview(date)
            .await
            .expect("overview fixture loads");
        CampusOverviewSections {
            overview,
            errors,
            recovery: CampusOverviewRecoveryHints::default(),
        }
    }

    fn fixture_source(sections: CampusOverviewSections) -> DynCampusDataSource {
        Arc::new(SectionsFixtureSource::new(sections))
    }

    fn temporary_cache_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "thyou-overview-{label}-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    #[tokio::test]
    async fn facade_uses_the_existing_data_source_trait_for_live_provenance() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let source: DynCampusDataSource = Arc::new(InMemoryCampusService::fixture_on(date));
        let result = CampusOverviewFacade::live(source).load(date).await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "ready");
        assert_eq!(
            result.overview.as_ref().map(|value| value.date.as_str()),
            Some("2026-09-11")
        );
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn facade_preserves_non_core_section_errors_with_partial_data() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let result = CampusOverviewFacade::live(fixture_source(
            fixture_sections(
                date,
                CampusOverviewSectionErrors {
                    todos: Some("unavailable".to_owned()),
                    ..CampusOverviewSectionErrors::default()
                },
            )
            .await,
        ))
        .load(date)
        .await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "ready");
        assert!(result.overview.is_some());
        assert!(result.error.is_none());
        assert_eq!(
            result
                .section_errors
                .as_ref()
                .and_then(|errors| errors.todos.as_deref()),
            Some("unavailable")
        );
    }

    #[tokio::test]
    async fn facade_promotes_both_core_section_failures_to_a_top_level_live_error() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let result = CampusOverviewFacade::live(fixture_source(
            fixture_sections(
                date,
                CampusOverviewSectionErrors {
                    courses: Some("unavailable".to_owned()),
                    schedule: Some("unavailable".to_owned()),
                    ..CampusOverviewSectionErrors::default()
                },
            )
            .await,
        ))
        .load(date)
        .await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "error");
        assert!(result.overview.is_none());
        assert!(result.section_errors.is_none());
        assert_eq!(
            result.error.as_deref(),
            Some(CORE_OVERVIEW_SECTIONS_FAILURE)
        );
    }

    #[tokio::test]
    async fn facade_keeps_one_core_failure_as_explicit_partial_data() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let result = CampusOverviewFacade::live(fixture_source(
            fixture_sections(
                date,
                CampusOverviewSectionErrors {
                    courses: Some("unavailable".to_owned()),
                    ..CampusOverviewSectionErrors::default()
                },
            )
            .await,
        ))
        .load(date)
        .await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "ready");
        assert!(result.overview.is_some());
        assert!(result.error.is_none());
        let section_errors = result
            .section_errors
            .as_ref()
            .expect("single core failure is exposed as section error");
        assert_eq!(section_errors.courses.as_deref(), Some("unavailable"));
        assert!(section_errors.schedule.is_none());
    }

    #[tokio::test]
    async fn backend_repair_overview_facade_preserves_typed_expiry_without_bridge_leak() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let mut sections = fixture_sections(date, CampusOverviewSectionErrors::default()).await;
        sections.recovery.learn_session_expired = true;
        let (result, payload, recovery) = CampusOverviewFacade::live(fixture_source(sections))
            .load_with_payload_and_recovery(date)
            .await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "ready");
        assert!(result.section_errors.is_none());
        assert!(payload.is_some());
        assert_eq!(
            recovery,
            CampusOverviewRecoveryHints {
                learn_session_expired: true,
                registrar_session_expired: false,
            }
        );
    }

    #[tokio::test]
    async fn source_failure_is_exposed_as_an_error_state() {
        let date = NaiveDate::MAX;
        let source: DynCampusDataSource = Arc::new(InMemoryCampusService::empty());
        let result = CampusOverviewFacade::live(source).load(date).await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "error");
        assert!(result.overview.is_none());
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("representable range"))
        );
    }

    #[tokio::test]
    async fn cache_freshness_maps_to_ready_and_stale_states() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let overview = InMemoryCampusService::fixture_on(date)
            .campus_overview(date)
            .await
            .expect("demo overview loads");
        let envelope = JsonCacheEnvelope {
            schema_version: 1,
            service: "overview".to_owned(),
            saved_at: "1000".to_owned(),
            payload: overview,
        };

        let fresh = CampusOverviewFacade::from_cache(
            date,
            Ok(Some(envelope.clone())),
            UNIX_EPOCH + Duration::from_millis(1100),
            Duration::from_millis(200),
        );
        assert_eq!(fresh.source, "cache");
        assert_eq!(fresh.status, "ready");
        assert!(fresh.overview.is_some());
        assert!(fresh.error.is_none());

        let stale = CampusOverviewFacade::from_cache(
            date,
            Ok(Some(envelope)),
            UNIX_EPOCH + Duration::from_millis(2000),
            Duration::from_millis(200),
        );
        assert_eq!(stale.source, "cache");
        assert_eq!(stale.status, "stale");
        assert!(stale.overview.is_some());
        assert!(stale.error.is_none());
    }

    #[tokio::test]
    async fn live_then_cache_returns_a_fresh_cache_after_live_failure() {
        let date = NaiveDate::MAX;
        let path = std::env::temp_dir().join(format!(
            "thyou-overview-fallback-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let cache = JsonFileCache::<CampusOverview>::new(&path, 1, "overview");
        let cached = CampusOverview {
            date,
            generated_at: chrono::DateTime::from_timestamp(1_000, 0).expect("timestamp"),
            semester: Some("2026-fall".to_owned()),
            course_count: 2,
            pending_todo_count: 1,
            completed_todo_count: 0,
            today_schedule: Vec::new(),
            upcoming_todos: Vec::new(),
            next_schedule: None,
        };
        cache.write(&cached).expect("cache writes");

        let source: DynCampusDataSource = Arc::new(InMemoryCampusService::empty());
        let result = CampusOverviewFacade::load_live_then_cache(
            source,
            &cache,
            date,
            SystemTime::now() + Duration::from_secs(1),
            Duration::from_secs(60),
        )
        .await;

        assert_eq!(result.source, "cache");
        assert_eq!(result.status, "ready");
        assert_eq!(
            result.overview.as_ref().map(|value| value.course_count),
            Some(2)
        );
        assert!(result.error.is_none());
        cache.clear().expect("cache clears");
    }

    #[tokio::test]
    async fn live_then_cache_reports_error_when_both_sources_fail() {
        let date = NaiveDate::MAX;
        let path = std::env::temp_dir().join(format!(
            "thyou-overview-fallback-miss-{}-{}.json",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let cache = JsonFileCache::<CampusOverview>::new(&path, 1, "overview");
        let source: DynCampusDataSource = Arc::new(InMemoryCampusService::empty());
        let result = CampusOverviewFacade::load_live_then_cache(
            source,
            &cache,
            date,
            UNIX_EPOCH,
            Duration::from_secs(60),
        )
        .await;

        assert_eq!(result.status, "error");
        assert_eq!(result.source, "live");
        assert!(result.overview.is_none());
        assert!(result.error.is_some());
        cache.clear().expect("cache clears");
    }

    #[tokio::test]
    async fn resolver_reports_both_core_failures_as_live_error_without_cache() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let path = temporary_cache_path("core-failure");
        let cache = JsonFileCache::<CampusOverview>::new(&path, 1, "overview");
        let source = fixture_source(
            fixture_sections(
                date,
                CampusOverviewSectionErrors {
                    courses: Some("unavailable".to_owned()),
                    schedule: Some("unavailable".to_owned()),
                    ..CampusOverviewSectionErrors::default()
                },
            )
            .await,
        );
        let resolver = CampusOverviewResolver::with_cache(source, cache, Duration::from_secs(60));

        let result = resolver.load_at(date, UNIX_EPOCH).await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "error");
        assert!(result.overview.is_none());
        assert!(result.section_errors.is_none());
        assert_eq!(
            result.error.as_deref(),
            Some(
                "live source failed: overview core sections failed: courses and schedule; \
                 cache fallback failed: cache miss"
            )
        );
        resolver.cache().clear().expect("cache clears");
    }

    #[tokio::test]
    async fn resolver_uses_explicit_cache_after_both_core_sections_fail_live() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let path = temporary_cache_path("core-failure-cache");
        let cache = JsonFileCache::<CampusOverview>::new(&path, 1, "overview");
        let mut cached = InMemoryCampusService::fixture_on(date)
            .campus_overview(date)
            .await
            .expect("overview fixture loads");
        cached.course_count = 99;
        cache.write(&cached).expect("cache writes");

        let source = fixture_source(
            fixture_sections(
                date,
                CampusOverviewSectionErrors {
                    courses: Some("unavailable".to_owned()),
                    schedule: Some("unavailable".to_owned()),
                    ..CampusOverviewSectionErrors::default()
                },
            )
            .await,
        );
        let resolver = CampusOverviewResolver::with_cache(source, cache, Duration::from_secs(60));

        let result = resolver
            .load_at(date, SystemTime::now() + Duration::from_secs(1))
            .await;

        assert_eq!(result.source, "cache");
        assert_eq!(result.status, "ready");
        assert_eq!(
            result
                .overview
                .as_ref()
                .map(|overview| overview.course_count),
            Some(99)
        );
        assert!(result.error.is_none());
        assert!(result.section_errors.is_none());
        resolver.cache().clear().expect("cache clears");
    }

    #[tokio::test]
    async fn resolver_keeps_partial_live_data_out_of_the_complete_cache() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let path = temporary_cache_path("partial");
        let cache = JsonFileCache::<CampusOverview>::new(&path, 1, "overview");
        let source = fixture_source(
            fixture_sections(
                date,
                CampusOverviewSectionErrors {
                    todos: Some("unavailable".to_owned()),
                    ..CampusOverviewSectionErrors::default()
                },
            )
            .await,
        );
        let resolver = CampusOverviewResolver::with_cache(source, cache, Duration::from_secs(60));

        let result = resolver.load_at(date, UNIX_EPOCH).await;

        assert_eq!(result.source, "live");
        assert_eq!(result.status, "ready");
        assert!(result.overview.is_some());
        assert!(result.error.is_none());
        assert_eq!(
            result
                .section_errors
                .as_ref()
                .and_then(|errors| errors.todos.as_deref()),
            Some("unavailable")
        );
        assert!(resolver.cache().read().expect("cache reads").is_none());
        resolver.cache().clear().expect("cache clears");
    }

    #[test]
    fn cache_metadata_and_misses_are_error_states() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 11).expect("valid test date");
        let missing =
            CampusOverviewFacade::from_cache(date, Ok(None), UNIX_EPOCH, Duration::from_secs(60));
        assert_eq!(missing.source, "cache");
        assert_eq!(missing.status, "error");
        assert_eq!(missing.error.as_deref(), Some("cache miss"));

        let invalid_timestamp = JsonCacheEnvelope {
            schema_version: 1,
            service: "overview".to_owned(),
            saved_at: "unknown".to_owned(),
            payload: CampusOverview {
                date,
                generated_at: chrono::DateTime::from_timestamp(1_000, 0).expect("valid timestamp"),
                semester: None,
                course_count: 0,
                pending_todo_count: 0,
                completed_todo_count: 0,
                today_schedule: Vec::new(),
                upcoming_todos: Vec::new(),
                next_schedule: None,
            },
        };
        let invalid = CampusOverviewFacade::from_cache(
            date,
            Ok(Some(invalid_timestamp)),
            UNIX_EPOCH + Duration::from_millis(1100),
            Duration::from_secs(60),
        );
        assert_eq!(invalid.status, "error");
        assert!(invalid.overview.is_none());
        assert!(
            invalid
                .error
                .as_deref()
                .is_some_and(|error| error.contains("invalid cache saved_at"))
        );
    }
}
