//! Flutter bridge adapters over the curated public Rust SDK.
//!
//! All bridge DTOs are defined in this crate. FRB-generated trait
//! implementations therefore remain local to the bridge package instead of
//! being implemented for types owned by the SDK or protocol engine.

use std::{collections::HashMap, fmt, path::PathBuf};

use tsinghua_kit_sdk::{
    Client as SdkClient, ClientBuilder, Error as SdkError,
    auth::{
        AccountAuthState, AccountAuthStatus, AuthStatus, IdentityLoginOutcome,
        IdentityLoginRequest, LoginStage, SecondFactorMethod, SelfServiceLoginOutcome,
        SelfServiceLoginPhase, SelfServiceLoginRequest,
    },
    calendar::{
        AcademicTerm, LearnTermCalendar, SchoolCalendarImage, SchoolCalendarLanguage,
        SchoolCalendarQuery, SchoolCalendarSemester,
    },
    campus_card::{
        CampusCardAccount, CampusCardInteraction, CampusCardPasswordRequest,
        CampusCardTransactionRange, CampusCardTransactionType, CampusCardTransactions,
    },
    classrooms::{
        BuildingRef, ClassroomAvailability, ClassroomBuildings, ClassroomSlotStatus, ClassroomWeek,
        ClassroomWeekSelection,
    },
    config::{
        ClientCachePolicy, CredentialStoragePolicy, IdentitySessionStoragePolicy,
        NetworkProfileStoragePolicy,
    },
    electricity::{ElectricityPaymentHistory, ElectricityRemainder},
    learn::{
        CourseAnnouncements, CourseCatalog, CourseDiscussions, CourseFileCategories, CourseFileRef,
        CourseFiles, CourseRef, HomeworkAttachmentKind, HomeworkDetail, HomeworkList, HomeworkRef,
        HomeworkState, SavedCourseFile,
    },
    library::{
        FloorRef, LibraryAvailability, LibraryDay, LibraryDirectory, LibraryRef,
        LibrarySocketAvailability, LibrarySocketState, LibraryTimeWindows, SeatRef, SeatWindowRef,
        SectionRef,
    },
    network::{
        NetworkAccessMethod, NetworkProfileId, NetworkProfileInput, NetworkProfilePassword,
        NetworkProfileSummary, PortalAddressRegistration, PortalConnectionResult,
        PortalConnectionState, PreparedNetworkInput,
    },
    news::{
        ArticleDetail, ArticleRef, NewsArticle, NewsCatalog, NewsCatalogCoverage, NewsChannelRef,
        NewsFavorites, NewsPage, NewsQuery, NewsSourceRef, NewsSubscriptionRef, NewsSubscriptions,
    },
    overview::{DailyOverview, OverviewSchedule, OverviewScheduleKind, OverviewTodo},
    read::{
        CacheFreshness, IncompleteReason, ReadCoverage, ReadMetadata, ReadPolicy, ReadResult,
        ReadSource,
    },
    registrar::{
        AcademicStage, ExamReport, ExamWeekday, GradeReport, GradeReportKind, ScheduleEvent,
        ScheduleEventKind, SemesterSchedule,
    },
    self_service::{AccountProfile, DeviceRef, OnlineDevice, UsageBalance},
    service_hall::{
        PendingTasks, PhaseDetails, ServiceDirectory, ServiceHallReadPolicy, TaskView,
        WorkflowTaskList, WorkflowTaskRef,
    },
};

/// The account state of one of the two independent Auth domains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStateDto {
    SignedOut,
    RestoredUnverified,
    Authenticated,
    NeedsInteraction,
    Authenticating,
    Expired,
    Unknown,
}

/// Redacted status for one account domain.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountStatusDto {
    pub state: AccountStateDto,
    pub username: Option<String>,
}

impl fmt::Debug for AccountStatusDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountStatusDto")
            .field("state", &self.state)
            .field("account_selected", &self.username.is_some())
            .finish()
    }
}

/// Independent unified Identity and network SelfService account status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatusDto {
    pub identity: AccountStatusDto,
    pub self_service: AccountStatusDto,
}

/// A stable, credential-free error crossing the Flutter bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkErrorDto {
    pub service: String,
    pub code: String,
    pub retry_after_ms: Option<u64>,
    pub diagnostic_id: String,
}

impl fmt::Display for SdkErrorDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.service, self.code)
    }
}

impl std::error::Error for SdkErrorDto {}

impl From<SdkError> for SdkErrorDto {
    fn from(error: SdkError) -> Self {
        Self {
            service: error.service().as_str().to_owned(),
            code: error.code().as_str().to_owned(),
            retry_after_ms: error
                .retry_after()
                .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64),
            diagnostic_id: error.diagnostic_id().to_string(),
        }
    }
}

/// Identity academic-stage preference for a single explicit login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStageDto {
    Auto,
    Undergraduate,
    Graduate,
}

impl From<LoginStageDto> for LoginStage {
    fn from(value: LoginStageDto) -> Self {
        match value {
            LoginStageDto::Auto => Self::Auto,
            LoginStageDto::Undergraduate => Self::Undergraduate,
            LoginStageDto::Graduate => Self::Graduate,
        }
    }
}

/// Suggests the Identity login-stage preference without creating a Client or
/// performing authentication/network I/O.
pub fn suggest_identity_login_stage(username: String) -> Option<LoginStageDto> {
    tsinghua_kit_sdk::auth::suggest_login_stage(&username).and_then(|stage| match stage {
        LoginStage::Auto => Some(LoginStageDto::Auto),
        LoginStage::Undergraduate => Some(LoginStageDto::Undergraduate),
        LoginStage::Graduate => Some(LoginStageDto::Graduate),
        _ => None,
    })
}

/// Result of an explicit Identity login or second-factor step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityLoginResultDto {
    pub status: AuthStatusDto,
    pub requires_interaction: bool,
    pub methods: Vec<String>,
    pub masked_phone: Option<String>,
}

/// Supported Identity second-factor method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondFactorMethodDto {
    Sms,
    Wechat,
    Mobile,
    Totp,
}

impl From<SecondFactorMethodDto> for SecondFactorMethod {
    fn from(value: SecondFactorMethodDto) -> Self {
        match value {
            SecondFactorMethodDto::Sms => Self::Sms,
            SecondFactorMethodDto::Wechat => Self::Wechat,
            SecondFactorMethodDto::Mobile => Self::Mobile,
            SecondFactorMethodDto::Totp => Self::Totp,
        }
    }
}

/// Image challenge for an explicit SelfService login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfServiceCaptchaDto {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Result of submitting the SelfService image challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfServiceLoginResultDto {
    pub status: AuthStatusDto,
    pub requires_interaction: bool,
    pub credentials_saved: bool,
}

/// Current local phase of an explicit SelfService captcha login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfServiceLoginPhaseDto {
    CaptchaReady,
    RefreshRequired,
    RestartRequired,
    Unknown,
}

impl From<SelfServiceLoginPhase> for SelfServiceLoginPhaseDto {
    fn from(value: SelfServiceLoginPhase) -> Self {
        match value {
            SelfServiceLoginPhase::CaptchaReady => Self::CaptchaReady,
            SelfServiceLoginPhase::RefreshRequired => Self::RefreshRequired,
            SelfServiceLoginPhase::RestartRequired => Self::RestartRequired,
            _ => Self::Unknown,
        }
    }
}

/// Cache policy for a news read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPolicyDto {
    CacheOnly,
    PreferFreshCache,
    Refresh,
    RefreshOrCached,
}

impl From<ReadPolicyDto> for ReadPolicy {
    fn from(value: ReadPolicyDto) -> Self {
        match value {
            ReadPolicyDto::CacheOnly => Self::CacheOnly,
            ReadPolicyDto::PreferFreshCache => Self::PreferFreshCache,
            ReadPolicyDto::Refresh => Self::Refresh,
            ReadPolicyDto::RefreshOrCached => Self::RefreshOrCached,
        }
    }
}

/// Cache behavior for an explicit service-hall read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceHallReadPolicyDto {
    CacheOnly,
    PreferFreshCache,
    Refresh,
}

impl From<ServiceHallReadPolicyDto> for ServiceHallReadPolicy {
    fn from(value: ServiceHallReadPolicyDto) -> Self {
        match value {
            ServiceHallReadPolicyDto::CacheOnly => Self::CacheOnly,
            ServiceHallReadPolicyDto::PreferFreshCache => Self::PreferFreshCache,
            ServiceHallReadPolicyDto::Refresh => Self::Refresh,
        }
    }
}

/// Stable origin categories for data returned by the SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadSourceDto {
    Live,
    MemoryCache,
    ClientCache,
    PersistentCache,
    Unknown,
}

impl From<ReadSource> for ReadSourceDto {
    fn from(value: ReadSource) -> Self {
        match value {
            ReadSource::Live => Self::Live,
            ReadSource::MemoryCache => Self::MemoryCache,
            ReadSource::ClientCache => Self::ClientCache,
            ReadSource::PersistentCache => Self::PersistentCache,
            _ => Self::Unknown,
        }
    }
}

/// Freshness classification for a cache-backed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheFreshnessDto {
    Fresh,
    Stale,
    NotApplicable,
    Unknown,
}

impl From<CacheFreshness> for CacheFreshnessDto {
    fn from(value: CacheFreshness) -> Self {
        match value {
            CacheFreshness::Fresh => Self::Fresh,
            CacheFreshness::Stale => Self::Stale,
            CacheFreshness::NotApplicable => Self::NotApplicable,
            _ => Self::Unknown,
        }
    }
}

/// Completeness categories for bounded collection reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadCoverageDto {
    Complete,
    PartialReadLimitReached,
    PartialSourceChanged,
    PartialCompletionUnconfirmed,
    Unknown,
}

impl From<ReadCoverage> for ReadCoverageDto {
    fn from(value: ReadCoverage) -> Self {
        match value {
            ReadCoverage::Complete => Self::Complete,
            ReadCoverage::Partial(IncompleteReason::ReadLimitReached) => {
                Self::PartialReadLimitReached
            }
            ReadCoverage::Partial(IncompleteReason::SourceChanged) => Self::PartialSourceChanged,
            ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed) => {
                Self::PartialCompletionUnconfirmed
            }
            ReadCoverage::Partial(_) => Self::Unknown,
            _ => Self::Unknown,
        }
    }
}

/// Source, freshness, observation time, and completeness of a successful read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMetadataDto {
    pub source: ReadSourceDto,
    pub freshness: CacheFreshnessDto,
    pub observed_at_utc: String,
    pub coverage: ReadCoverageDto,
    pub refresh_failure_code: Option<String>,
}

impl From<&ReadMetadata> for ReadMetadataDto {
    fn from(value: &ReadMetadata) -> Self {
        Self {
            source: value.source().into(),
            freshness: value.freshness().into(),
            observed_at_utc: value
                .observed_at()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            coverage: value.coverage().into(),
            refresh_failure_code: value.refresh_failure().map(|code| code.as_str().to_owned()),
        }
    }
}

/// Stable categories for a verified daily overview schedule row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewScheduleKindDto {
    Course,
    Exam,
    Event,
    Deadline,
    Reminder,
    Unknown,
}

impl From<OverviewScheduleKind> for OverviewScheduleKindDto {
    fn from(value: OverviewScheduleKind) -> Self {
        match value {
            OverviewScheduleKind::Course => Self::Course,
            OverviewScheduleKind::Exam => Self::Exam,
            OverviewScheduleKind::Event => Self::Event,
            OverviewScheduleKind::Deadline => Self::Deadline,
            OverviewScheduleKind::Reminder => Self::Reminder,
            _ => Self::Unknown,
        }
    }
}

/// One schedule row supplied by the Rust overview resolver.
#[derive(Clone, PartialEq, Eq)]
pub struct OverviewScheduleDto {
    pub title: String,
    pub kind: OverviewScheduleKindDto,
    pub starts_at_utc: String,
    pub ends_at_utc: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
}

impl fmt::Debug for OverviewScheduleDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OverviewScheduleDto")
            .field("kind", &self.kind)
            .field("title_present", &!self.title.is_empty())
            .finish()
    }
}

/// One current Learn assignment in the daily overview.
#[derive(Clone, PartialEq, Eq)]
pub struct OverviewTodoDto {
    pub title: String,
    pub due_at_utc: Option<String>,
}

impl fmt::Debug for OverviewTodoDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OverviewTodoDto")
            .field("title_present", &!self.title.is_empty())
            .field("due_at_present", &self.due_at_utc.is_some())
            .finish()
    }
}

/// Section-level failure flags; a partial summary is never labeled complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverviewSectionFailuresDto {
    pub courses: bool,
    pub schedule: bool,
    pub todos: bool,
    pub services: bool,
    pub updates: bool,
}

/// One account-bound daily summary without protocol selectors.
#[derive(Clone, PartialEq, Eq)]
pub struct DailyOverviewDto {
    pub date: String,
    pub semester: Option<String>,
    pub course_count: u32,
    pub pending_todo_count: u32,
    pub completed_todo_count: u32,
    pub today_schedule: Vec<OverviewScheduleDto>,
    pub upcoming_todos: Vec<OverviewTodoDto>,
    pub next_schedule: Option<OverviewScheduleDto>,
    pub section_failures: OverviewSectionFailuresDto,
}

impl fmt::Debug for DailyOverviewDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DailyOverviewDto")
            .field("date", &self.date)
            .field("schedule_count", &self.today_schedule.len())
            .field("todo_count", &self.upcoming_todos.len())
            .field("section_failures", &self.section_failures)
            .finish()
    }
}

/// A daily summary with source, freshness, and completeness evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyOverviewResultDto {
    pub data: DailyOverviewDto,
    pub metadata: ReadMetadataDto,
}

fn overview_schedule_dto(value: &OverviewSchedule) -> OverviewScheduleDto {
    OverviewScheduleDto {
        title: value.title().to_owned(),
        kind: value.kind().into(),
        starts_at_utc: value.starts_at().to_rfc3339(),
        ends_at_utc: value.ends_at().map(|instant| instant.to_rfc3339()),
        all_day: value.is_all_day(),
        location: value.location().map(ToOwned::to_owned),
    }
}

fn overview_todo_dto(value: &OverviewTodo) -> OverviewTodoDto {
    OverviewTodoDto {
        title: value.title().to_owned(),
        due_at_utc: value.due_at().map(|instant| instant.to_rfc3339()),
    }
}

fn daily_overview_result(value: ReadResult<DailyOverview>) -> DailyOverviewResultDto {
    let (overview, metadata) = value.into_parts();
    let failures = overview.section_failures();
    DailyOverviewResultDto {
        data: DailyOverviewDto {
            date: overview.date().to_string(),
            semester: overview.semester().map(ToOwned::to_owned),
            course_count: overview.course_count(),
            pending_todo_count: overview.pending_todo_count(),
            completed_todo_count: overview.completed_todo_count(),
            today_schedule: overview
                .today_schedule()
                .iter()
                .map(overview_schedule_dto)
                .collect(),
            upcoming_todos: overview
                .upcoming_todos()
                .iter()
                .map(overview_todo_dto)
                .collect(),
            next_schedule: overview.next_schedule().map(overview_schedule_dto),
            section_failures: OverviewSectionFailuresDto {
                courses: failures.courses,
                schedule: failures.schedule,
                todos: failures.todos,
                services: failures.services,
                updates: failures.updates,
            },
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// Academic stage established by this Client's Registrar proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcademicStageDto {
    Undergraduate,
    Graduate,
    Unknown,
}

impl From<AcademicStage> for AcademicStageDto {
    fn from(value: AcademicStage) -> Self {
        match value {
            AcademicStage::Undergraduate => Self::Undergraduate,
            AcademicStage::Graduate => Self::Graduate,
            _ => Self::Unknown,
        }
    }
}

/// The selected undergraduate grade curriculum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradeReportKindDto {
    FirstDegree,
    SecondDegree,
    Minor,
    Unknown,
}

impl From<GradeReportKind> for GradeReportKindDto {
    fn from(value: GradeReportKind) -> Self {
        match value {
            GradeReportKind::FirstDegree => Self::FirstDegree,
            GradeReportKind::SecondDegree => Self::SecondDegree,
            GradeReportKind::Minor => Self::Minor,
            _ => Self::Unknown,
        }
    }
}

/// A normalized category for one semester-schedule event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleEventKindDto {
    Course,
    Exam,
    Event,
    Deadline,
    Reminder,
    Unknown,
}

impl From<ScheduleEventKind> for ScheduleEventKindDto {
    fn from(value: ScheduleEventKind) -> Self {
        match value {
            ScheduleEventKind::Course => Self::Course,
            ScheduleEventKind::Exam => Self::Exam,
            ScheduleEventKind::Event => Self::Event,
            ScheduleEventKind::Deadline => Self::Deadline,
            ScheduleEventKind::Reminder => Self::Reminder,
            _ => Self::Unknown,
        }
    }
}

/// A validated weekday in an examination report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExamWeekdayDto {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
    Unknown,
}

impl From<ExamWeekday> for ExamWeekdayDto {
    fn from(value: ExamWeekday) -> Self {
        match value {
            ExamWeekday::Monday => Self::Monday,
            ExamWeekday::Tuesday => Self::Tuesday,
            ExamWeekday::Wednesday => Self::Wednesday,
            ExamWeekday::Thursday => Self::Thursday,
            ExamWeekday::Friday => Self::Friday,
            ExamWeekday::Saturday => Self::Saturday,
            ExamWeekday::Sunday => Self::Sunday,
            _ => Self::Unknown,
        }
    }
}

/// A semester selector for the published school calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchoolCalendarSemesterDto {
    Autumn,
    Spring,
    Unknown,
}

/// A language selector for the published school calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchoolCalendarLanguageDto {
    Chinese,
    English,
    Unknown,
}

/// Whether INFO confirmed the complete channel directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsCatalogCoverageDto {
    Complete,
    Partial,
    Unknown,
}

impl From<NewsCatalogCoverage> for NewsCatalogCoverageDto {
    fn from(value: NewsCatalogCoverage) -> Self {
        match value {
            NewsCatalogCoverage::Complete => Self::Complete,
            NewsCatalogCoverage::Partial => Self::Partial,
            _ => Self::Unknown,
        }
    }
}

/// One source option from the latest INFO catalog.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsSourceDto {
    pub reference_id: String,
    pub name: String,
}

impl fmt::Debug for NewsSourceDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsSourceDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("name", &self.name)
            .finish()
    }
}

/// One channel option from the latest INFO catalog.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsChannelDto {
    pub reference_id: String,
    pub title: String,
}

impl fmt::Debug for NewsChannelDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsChannelDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("title", &self.title)
            .finish()
    }
}

/// Validated source and channel choices from INFO.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsCatalogDto {
    pub sources: Vec<NewsSourceDto>,
    pub channels: Vec<NewsChannelDto>,
    pub channel_coverage: NewsCatalogCoverageDto,
}

impl fmt::Debug for NewsCatalogDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsCatalogDto")
            .field("source_count", &self.sources.len())
            .field("channel_count", &self.channels.len())
            .field("channel_coverage", &self.channel_coverage)
            .finish()
    }
}

/// A catalog read with freshness and completeness evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsCatalogResultDto {
    pub data: NewsCatalogDto,
    pub metadata: ReadMetadataDto,
}

/// One article shown in an INFO page. Detail selection stays opaque.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsArticleDto {
    pub reference_id: String,
    pub title: String,
    pub published_at: String,
    pub source: String,
    pub topped: bool,
    pub channel: String,
    pub favorited: bool,
}

impl fmt::Debug for NewsArticleDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsArticleDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("title", &self.title)
            .field("published_at", &self.published_at)
            .field("source", &self.source)
            .field("topped", &self.topped)
            .field("channel", &self.channel)
            .field("favorited", &self.favorited)
            .finish()
    }
}

/// One INFO news page. It does not imply later pages were fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsPageDto {
    pub page: u32,
    pub articles: Vec<NewsArticleDto>,
}

/// A news page and its source, freshness, and query coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsPageResultDto {
    pub data: NewsPageDto,
    pub metadata: ReadMetadataDto,
}

/// One saved INFO subscription rule.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsSubscriptionDto {
    pub reference_id: String,
    pub title: String,
    pub order: i64,
    pub sources: Vec<String>,
    pub channels: Vec<String>,
    pub keyword: String,
}

impl fmt::Debug for NewsSubscriptionDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsSubscriptionDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("title", &self.title)
            .field("order", &self.order)
            .field("source_count", &self.sources.len())
            .field("channel_count", &self.channels.len())
            .field("keyword", &"[redacted]")
            .finish()
    }
}

/// Current Identity-account INFO subscription rules.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsSubscriptionsDto {
    pub rules: Vec<NewsSubscriptionDto>,
}

impl fmt::Debug for NewsSubscriptionsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsSubscriptionsDto")
            .field("rule_count", &self.rules.len())
            .finish()
    }
}

/// Subscription rules with provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSubscriptionsResultDto {
    pub data: NewsSubscriptionsDto,
    pub metadata: ReadMetadataDto,
}

/// All favorite news items from the complete bounded read.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsFavoritesDto {
    pub articles: Vec<NewsArticleDto>,
}

impl fmt::Debug for NewsFavoritesDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsFavoritesDto")
            .field("article_count", &self.articles.len())
            .finish()
    }
}

/// Complete bounded favorites and their provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsFavoritesResultDto {
    pub data: NewsFavoritesDto,
    pub metadata: ReadMetadataDto,
}

/// One cleaned attachment name without an upstream URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsAttachmentDto {
    pub name: String,
}

/// Read-only article detail selected by a current opaque article reference.
#[derive(Clone, PartialEq, Eq)]
pub struct ArticleDetailDto {
    pub title: String,
    pub content_html: String,
    pub summary: String,
    pub attachments: Vec<NewsAttachmentDto>,
}

impl fmt::Debug for ArticleDetailDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArticleDetailDto")
            .field("title", &self.title)
            .field("content_html", &"[redacted]")
            .field("summary", &"[redacted]")
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

/// Article detail and its source/freshness metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleDetailResultDto {
    pub data: ArticleDetailDto,
    pub metadata: ReadMetadataDto,
}

fn news_catalog_result(
    value: ReadResult<NewsCatalog>,
    source_references: &mut HashMap<String, NewsSourceRef>,
    channel_references: &mut HashMap<String, NewsChannelRef>,
) -> NewsCatalogResultDto {
    let (catalog, metadata) = value.into_parts();
    let mut next_source_references = HashMap::new();
    let mut next_channel_references = HashMap::new();
    let sources = catalog
        .sources()
        .iter()
        .map(|source| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_source_references.insert(reference_id.clone(), source.reference().clone());
            NewsSourceDto {
                reference_id,
                name: source.name().to_owned(),
            }
        })
        .collect();
    let channels = catalog
        .channels()
        .iter()
        .map(|channel| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_channel_references.insert(reference_id.clone(), channel.reference().clone());
            NewsChannelDto {
                reference_id,
                title: channel.title().to_owned(),
            }
        })
        .collect();
    *source_references = next_source_references;
    *channel_references = next_channel_references;
    NewsCatalogResultDto {
        data: NewsCatalogDto {
            sources,
            channels,
            channel_coverage: catalog.channel_coverage().into(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn news_article_dto(
    value: &NewsArticle,
    references: &mut HashMap<String, ArticleRef>,
) -> NewsArticleDto {
    let reference_id = uuid::Uuid::new_v4().to_string();
    references.insert(reference_id.clone(), value.reference().clone());
    NewsArticleDto {
        reference_id,
        title: value.title().to_owned(),
        published_at: value.published_at().to_owned(),
        source: value.source().to_owned(),
        topped: value.is_topped(),
        channel: value.channel().to_owned(),
        favorited: value.is_favorited(),
    }
}

fn news_page_result(
    value: ReadResult<NewsPage>,
    references: &mut HashMap<String, ArticleRef>,
) -> NewsPageResultDto {
    let (page, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let articles = page
        .items()
        .iter()
        .map(|article| news_article_dto(article, &mut next_references))
        .collect();
    *references = next_references;
    NewsPageResultDto {
        data: NewsPageDto {
            page: page.page(),
            articles,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn news_subscriptions_result(
    value: ReadResult<NewsSubscriptions>,
    references: &mut HashMap<String, NewsSubscriptionRef>,
) -> NewsSubscriptionsResultDto {
    let (subscriptions, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let rules = subscriptions
        .rules()
        .iter()
        .map(|rule| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_references.insert(reference_id.clone(), rule.reference().clone());
            NewsSubscriptionDto {
                reference_id,
                title: rule.title().to_owned(),
                order: rule.order(),
                sources: rule.sources().to_vec(),
                channels: rule.channels().to_vec(),
                keyword: rule.keyword().to_owned(),
            }
        })
        .collect();
    *references = next_references;
    NewsSubscriptionsResultDto {
        data: NewsSubscriptionsDto { rules },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn news_favorites_result(
    value: ReadResult<NewsFavorites>,
    references: &mut HashMap<String, ArticleRef>,
) -> NewsFavoritesResultDto {
    let (favorites, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let articles = favorites
        .items()
        .iter()
        .map(|article| news_article_dto(article, &mut next_references))
        .collect();
    *references = next_references;
    NewsFavoritesResultDto {
        data: NewsFavoritesDto { articles },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn article_detail_result(value: ReadResult<ArticleDetail>) -> ArticleDetailResultDto {
    let (detail, metadata) = value.into_parts();
    ArticleDetailResultDto {
        data: ArticleDetailDto {
            title: detail.title().to_owned(),
            content_html: detail.content_html().to_owned(),
            summary: detail.summary().to_owned(),
            attachments: detail
                .attachments()
                .iter()
                .map(|attachment| NewsAttachmentDto {
                    name: attachment.name().to_owned(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn campus_card_account_result(value: ReadResult<CampusCardAccount>) -> CampusCardAccountResultDto {
    let (account, metadata) = value.into_parts();
    CampusCardAccountResultDto {
        data: CampusCardAccountDto {
            display_name: account.display_name().to_owned(),
            display_name_latin: account.display_name_latin().map(str::to_owned),
            department_name: account.department_name().to_owned(),
            department_name_latin: account.department_name_latin().map(str::to_owned),
            department_id: account.department_id(),
            gender: account.gender().map(str::to_owned),
            effective_at: account.effective_at().to_owned(),
            valid_until: account.valid_until().to_owned(),
            balance_cents: account.balance_cents(),
            card_status: account.card_status().to_owned(),
            last_transaction_at: account.last_transaction_at().to_owned(),
            daily_limit_cents: account.daily_limit_cents(),
            one_time_limit_cents: account.one_time_limit_cents(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn campus_card_transactions_result(
    value: ReadResult<CampusCardTransactions>,
) -> CampusCardTransactionsResultDto {
    let (transactions, metadata) = value.into_parts();
    let range = transactions.range();
    CampusCardTransactionsResultDto {
        data: CampusCardTransactionsDto {
            start_date: range.start_date().to_owned(),
            end_date: range.end_date().to_owned(),
            transaction_type: range.transaction_type().into(),
            items: transactions
                .items()
                .iter()
                .map(|transaction| CampusCardTransactionDto {
                    summary: transaction.summary().to_owned(),
                    occurred_at: transaction.occurred_at().to_owned(),
                    post_balance_cents: transaction.post_balance_cents(),
                    amount_cents: transaction.amount_cents(),
                    merchant_address: transaction.merchant_address().to_owned(),
                    merchant_name: transaction.merchant_name().map(str::to_owned),
                    transaction_name: transaction.transaction_name().to_owned(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn electricity_remainder_result(
    value: ReadResult<ElectricityRemainder>,
) -> ElectricityRemainderResultDto {
    let (remainder, metadata) = value.into_parts();
    ElectricityRemainderResultDto {
        data: ElectricityRemainderDto {
            value: remainder.value(),
            update_time: remainder.update_time().to_owned(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn electricity_payment_history_result(
    value: ReadResult<ElectricityPaymentHistory>,
) -> ElectricityPaymentHistoryResultDto {
    let (history, metadata) = value.into_parts();
    ElectricityPaymentHistoryResultDto {
        data: ElectricityPaymentHistoryDto {
            empty: history.is_empty(),
            records: history
                .records()
                .iter()
                .map(|record| ElectricityPaymentRecordDto {
                    occurred_at: record.occurred_at().to_owned(),
                    amount: record.amount(),
                    status: record.status().to_owned(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn library_directory_result(
    value: ReadResult<LibraryDirectory>,
    references: &mut HashMap<String, LibraryRef>,
) -> LibraryDirectoryResultDto {
    let (directory, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let libraries = directory
        .libraries()
        .iter()
        .map(|library| {
            let reference_id = library.reference().map(|reference| {
                let reference_id = uuid::Uuid::new_v4().to_string();
                next_references.insert(reference_id.clone(), reference.clone());
                reference_id
            });
            LibraryPlaceDto {
                reference_id,
                name: library.name().to_owned(),
                english_name: library.english_name().map(str::to_owned),
                is_valid: library.is_valid(),
                total_seats: library.total_seats(),
                available_seats: library.available_seats(),
            }
        })
        .collect();
    *references = next_references;
    LibraryDirectoryResultDto {
        data: LibraryDirectoryDto { libraries },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn library_floors_result(
    value: ReadResult<tsinghua_kit_sdk::library::LibraryFloors>,
    references: &mut HashMap<String, FloorRef>,
) -> LibraryFloorsResultDto {
    let (floors, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let items = floors
        .floors()
        .iter()
        .map(|floor| {
            let reference_id = floor.reference().map(|reference| {
                let reference_id = uuid::Uuid::new_v4().to_string();
                next_references.insert(reference_id.clone(), reference.clone());
                reference_id
            });
            LibraryFloorDto {
                reference_id,
                name: floor.name().to_owned(),
                is_valid: floor.is_valid(),
                total_seats: floor.total_seats(),
                available_seats: floor.available_seats(),
            }
        })
        .collect();
    *references = next_references;
    LibraryFloorsResultDto {
        data: LibraryFloorsDto { items },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn library_sections_result(
    value: ReadResult<tsinghua_kit_sdk::library::LibrarySections>,
    references: &mut HashMap<String, SectionRef>,
) -> LibrarySectionsResultDto {
    let (sections, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let items = sections
        .sections()
        .iter()
        .map(|section| {
            let reference_id = section.reference().map(|reference| {
                let reference_id = uuid::Uuid::new_v4().to_string();
                next_references.insert(reference_id.clone(), reference.clone());
                reference_id
            });
            LibrarySectionDto {
                reference_id,
                name: section.name().to_owned(),
                is_valid: section.is_valid(),
                total_seats: section.total_seats(),
                available_seats: section.available_seats(),
            }
        })
        .collect();
    *references = next_references;
    LibrarySectionsResultDto {
        data: LibrarySectionsDto {
            day: sections.day().format("%Y-%m-%d").to_string(),
            items,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn library_time_windows_result(
    value: ReadResult<LibraryTimeWindows>,
    references: &mut HashMap<String, SeatWindowRef>,
) -> LibraryTimeWindowsResultDto {
    let (windows, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let items = windows
        .windows()
        .iter()
        .map(|window| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_references.insert(reference_id.clone(), window.reference().clone());
            LibraryTimeWindowDto {
                reference_id,
                starts_at: window.starts_at().format("%H:%M").to_string(),
                ends_at: window.ends_at().format("%H:%M").to_string(),
            }
        })
        .collect();
    *references = next_references;
    LibraryTimeWindowsResultDto {
        data: LibraryTimeWindowsDto {
            day: windows.day().format("%Y-%m-%d").to_string(),
            items,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn library_availability_result(
    value: ReadResult<LibraryAvailability>,
    references: &mut HashMap<String, LibraryAvailability>,
    seat_reference_ids: &mut HashMap<SeatRef, String>,
) -> LibraryAvailabilityResultDto {
    let (availability, metadata) = value.into_parts();
    let availability_reference_id = uuid::Uuid::new_v4().to_string();
    let mut next_seat_reference_ids = HashMap::new();
    let seats = availability
        .seats()
        .iter()
        .map(|seat| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_seat_reference_ids.insert(seat.reference().clone(), reference_id.clone());
            LibrarySeatDto {
                reference_id,
                name: seat.name().to_owned(),
                is_available: seat.is_available(),
            }
        })
        .collect();
    *seat_reference_ids = next_seat_reference_ids;
    references.clear();
    references.insert(availability_reference_id.clone(), availability.clone());
    LibraryAvailabilityResultDto {
        data: LibraryAvailabilityDto {
            reference_id: availability_reference_id,
            day: availability.day().format("%Y-%m-%d").to_string(),
            seats,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn library_sockets_result(
    value: ReadResult<LibrarySocketAvailability>,
    seat_reference_ids: &HashMap<SeatRef, String>,
) -> Result<LibrarySocketsResultDto, SdkErrorDto> {
    let (sockets, metadata) = value.into_parts();
    let statuses = sockets
        .statuses()
        .iter()
        .map(|status| {
            let reference_id = seat_reference_ids
                .get(status.seat())
                .cloned()
                .ok_or_else(|| context_mismatch("library"))?;
            Ok(LibrarySocketStatusDto {
                seat_reference_id: reference_id,
                state: status.state().into(),
            })
        })
        .collect::<Result<Vec<_>, SdkErrorDto>>()?;
    Ok(LibrarySocketsResultDto {
        data: LibrarySocketsDto { statuses },
        metadata: ReadMetadataDto::from(&metadata),
    })
}

fn classroom_buildings_result(
    value: ReadResult<ClassroomBuildings>,
    references: &mut HashMap<String, BuildingRef>,
) -> ClassroomBuildingsResultDto {
    let (buildings, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let items = buildings
        .buildings()
        .iter()
        .map(|building| {
            let reference_id = building.reference().map(|reference| {
                let reference_id = uuid::Uuid::new_v4().to_string();
                next_references.insert(reference_id.clone(), reference.clone());
                reference_id
            });
            ClassroomBuildingDto {
                reference_id,
                name: building.name().to_owned(),
                default_week: building.default_week().map(ClassroomWeek::get),
            }
        })
        .collect();
    *references = next_references;
    ClassroomBuildingsResultDto {
        data: ClassroomBuildingsDto { items },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn classroom_availability_result(
    value: ReadResult<ClassroomAvailability>,
) -> ClassroomAvailabilityResultDto {
    let (availability, metadata) = value.into_parts();
    let week_dates = availability
        .week_dates()
        .iter()
        .map(|date| date.format("%Y-%m-%d").to_string())
        .collect();
    let rooms = availability
        .rooms()
        .iter()
        .map(|room| ClassroomRoomDto {
            name: room.name().to_owned(),
            slots: room
                .slots()
                .iter()
                .map(|slot| {
                    let (status, class_name) = match slot {
                        ClassroomSlotStatus::Free => (ClassroomSlotStatusDto::Free, None),
                        ClassroomSlotStatus::Occupied => (ClassroomSlotStatusDto::Occupied, None),
                        ClassroomSlotStatus::Exam => (ClassroomSlotStatusDto::Exam, None),
                        ClassroomSlotStatus::Borrowed => (ClassroomSlotStatusDto::Borrowed, None),
                        ClassroomSlotStatus::Disabled => (ClassroomSlotStatusDto::Disabled, None),
                        ClassroomSlotStatus::Unknown { class_name } => {
                            (ClassroomSlotStatusDto::Unknown, Some(class_name.clone()))
                        }
                        _ => (ClassroomSlotStatusDto::Unknown, None),
                    };
                    ClassroomSlotDto {
                        status,
                        unknown_class_name: class_name,
                    }
                })
                .collect(),
        })
        .collect();
    ClassroomAvailabilityResultDto {
        data: ClassroomAvailabilityDto {
            week: availability.week().get(),
            valid_weeks: availability
                .valid_weeks()
                .iter()
                .map(|week| week.get())
                .collect(),
            week_dates,
            rooms,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_course_catalog_result(
    value: ReadResult<CourseCatalog>,
    references: &mut HashMap<String, CourseRef>,
) -> LearnCourseCatalogResultDto {
    let (catalog, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let courses = catalog
        .courses()
        .iter()
        .map(|course| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_references.insert(reference_id.clone(), course.reference().clone());
            LearnCourseDto {
                reference_id,
                code: course.code().map(str::to_owned),
                title: course.title().to_owned(),
                instructor: course.instructor().map(str::to_owned),
                semester: course.semester().map(str::to_owned),
            }
        })
        .collect();
    *references = next_references;
    LearnCourseCatalogResultDto {
        data: LearnCourseCatalogDto {
            semester: catalog.semester().to_owned(),
            courses,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_announcements_result(
    value: ReadResult<CourseAnnouncements>,
) -> LearnAnnouncementsResultDto {
    let (announcements, metadata) = value.into_parts();
    LearnAnnouncementsResultDto {
        data: LearnAnnouncementsDto {
            items: announcements
                .items()
                .iter()
                .map(|announcement| LearnAnnouncementDto {
                    title: announcement.title().to_owned(),
                    publisher: announcement.publisher().map(str::to_owned),
                    content: announcement.content().map(str::to_owned),
                    published_at_utc: announcement
                        .published_at()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    expires_at_utc: announcement
                        .expires_at()
                        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                    read: announcement.is_read(),
                    important: announcement.is_important(),
                    favorited: announcement.is_favorited(),
                    expired: announcement.is_expired(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_homework_list_result(
    value: ReadResult<HomeworkList>,
    references: &mut HashMap<String, HomeworkRef>,
) -> LearnHomeworkListResultDto {
    let (list, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let items = list
        .items()
        .iter()
        .map(|homework| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_references.insert(reference_id.clone(), homework.reference().clone());
            LearnHomeworkDto {
                reference_id,
                title: homework.title().to_owned(),
                state: homework.state().into(),
                due_at_utc: homework
                    .due_at()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                late_due_at_utc: homework
                    .late_due_at()
                    .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                submitted_at_utc: homework
                    .submitted_at()
                    .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                graded_at_utc: homework
                    .graded_at()
                    .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                detail_available: homework.detail_available(),
            }
        })
        .collect();
    *references = next_references;
    LearnHomeworkListResultDto {
        data: LearnHomeworkListDto { items },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_homework_detail_result(value: ReadResult<HomeworkDetail>) -> LearnHomeworkDetailResultDto {
    let (detail, metadata) = value.into_parts();
    LearnHomeworkDetailResultDto {
        data: LearnHomeworkDetailDto {
            description: detail.description().map(str::to_owned),
            answer_content: detail.answer_content().map(str::to_owned),
            submitted_content: detail.submitted_content().map(str::to_owned),
            attachments: detail
                .attachments()
                .iter()
                .map(|attachment| LearnHomeworkAttachmentDto {
                    kind: attachment.kind().into(),
                    name: attachment.name().to_owned(),
                    size: attachment.size().map(str::to_owned),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_course_files_result(
    value: ReadResult<CourseFiles>,
    references: &mut HashMap<String, CourseFileRef>,
) -> LearnCourseFilesResultDto {
    let (files, metadata) = value.into_parts();
    let mut next_references = HashMap::new();
    let items = files
        .items()
        .iter()
        .map(|file| {
            let reference_id = uuid::Uuid::new_v4().to_string();
            next_references.insert(reference_id.clone(), file.reference().clone());
            LearnCourseFileDto {
                reference_id,
                title: file.title().to_owned(),
                suggested_filename: file.suggested_filename().to_owned(),
                description: file.description().map(str::to_owned),
                size_label: file.size_label().map(str::to_owned),
                uploaded_at_label: file.uploaded_at_label().map(str::to_owned),
                file_type: file.file_type().map(str::to_owned),
            }
        })
        .collect();
    *references = next_references;
    LearnCourseFilesResultDto {
        data: LearnCourseFilesDto {
            items,
            coverage: metadata.coverage().into(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_course_file_categories_result(
    value: ReadResult<CourseFileCategories>,
) -> LearnCourseFileCategoriesResultDto {
    let (categories, metadata) = value.into_parts();
    LearnCourseFileCategoriesResultDto {
        data: LearnCourseFileCategoriesDto {
            items: categories
                .items()
                .iter()
                .map(|category| LearnCourseFileCategoryDto {
                    title: category.title().to_owned(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn learn_course_discussions_result(
    value: ReadResult<CourseDiscussions>,
) -> LearnCourseDiscussionsResultDto {
    let (discussions, metadata) = value.into_parts();
    LearnCourseDiscussionsResultDto {
        data: LearnCourseDiscussionsDto {
            items: discussions
                .items()
                .iter()
                .map(|discussion| LearnCourseDiscussionDto {
                    title: discussion.title().to_owned(),
                    publisher: discussion.publisher().to_owned(),
                    published_at_label: discussion.published_at_label().to_owned(),
                    last_reply_at_label: discussion.last_reply_at_label().map(str::to_owned),
                    reply_count: discussion.reply_count(),
                })
                .collect(),
            coverage: metadata.coverage().into(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

fn saved_learn_course_file(value: SavedCourseFile) -> SavedLearnCourseFileDto {
    SavedLearnCourseFileDto {
        bytes_written: value.bytes_written(),
    }
}

/// One course returned by the current Learn course directory.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseDto {
    pub reference_id: String,
    pub code: Option<String>,
    pub title: String,
    pub instructor: Option<String>,
    pub semester: Option<String>,
}

impl fmt::Debug for LearnCourseDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("code_present", &self.code.is_some())
            .field("title_present", &!self.title.is_empty())
            .field("instructor_present", &self.instructor.is_some())
            .field("semester_present", &self.semester.is_some())
            .finish()
    }
}

/// The verified current course directory.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseCatalogDto {
    pub semester: String,
    pub courses: Vec<LearnCourseDto>,
}

impl fmt::Debug for LearnCourseCatalogDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseCatalogDto")
            .field("semester_present", &!self.semester.is_empty())
            .field("course_count", &self.courses.len())
            .finish()
    }
}

/// The current Learn directory with source and freshness metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseCatalogResultDto {
    pub data: LearnCourseCatalogDto,
    pub metadata: ReadMetadataDto,
}

/// One announcement associated with a Learn course.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnAnnouncementDto {
    pub title: String,
    pub publisher: Option<String>,
    pub content: Option<String>,
    pub published_at_utc: String,
    pub expires_at_utc: Option<String>,
    pub read: Option<bool>,
    pub important: Option<bool>,
    pub favorited: Option<bool>,
    pub expired: bool,
}

impl fmt::Debug for LearnAnnouncementDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnAnnouncementDto")
            .field("title_present", &!self.title.is_empty())
            .field("publisher_present", &self.publisher.is_some())
            .field("content_present", &self.content.is_some())
            .field("published_at_utc", &self.published_at_utc)
            .field("expires_at_present", &self.expires_at_utc.is_some())
            .field("read", &self.read)
            .field("important", &self.important)
            .field("favorited", &self.favorited)
            .field("expired", &self.expired)
            .finish()
    }
}

/// Active and expired announcements from one course read.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnAnnouncementsDto {
    pub items: Vec<LearnAnnouncementDto>,
}

impl fmt::Debug for LearnAnnouncementsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnAnnouncementsDto")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// Course announcements and their cache metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnAnnouncementsResultDto {
    pub data: LearnAnnouncementsDto,
    pub metadata: ReadMetadataDto,
}

/// The normalized state of one Learn assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnHomeworkStateDto {
    Pending,
    Submitted,
    Graded,
    Unknown,
}

impl From<HomeworkState> for LearnHomeworkStateDto {
    fn from(value: HomeworkState) -> Self {
        match value {
            HomeworkState::Pending => Self::Pending,
            HomeworkState::Submitted => Self::Submitted,
            HomeworkState::Graded => Self::Graded,
            _ => Self::Unknown,
        }
    }
}

/// One assignment in a complete Learn homework response.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnHomeworkDto {
    pub reference_id: String,
    pub title: String,
    pub state: LearnHomeworkStateDto,
    pub due_at_utc: String,
    pub late_due_at_utc: Option<String>,
    pub submitted_at_utc: Option<String>,
    pub graded_at_utc: Option<String>,
    pub detail_available: bool,
}

impl fmt::Debug for LearnHomeworkDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnHomeworkDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("title_present", &!self.title.is_empty())
            .field("state", &self.state)
            .field("due_at_utc", &self.due_at_utc)
            .field("late_due_at_present", &self.late_due_at_utc.is_some())
            .field("submitted_at_present", &self.submitted_at_utc.is_some())
            .field("graded_at_present", &self.graded_at_utc.is_some())
            .field("detail_available", &self.detail_available)
            .finish()
    }
}

/// One complete live homework list for a selected course.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnHomeworkListDto {
    pub items: Vec<LearnHomeworkDto>,
}

impl fmt::Debug for LearnHomeworkListDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnHomeworkListDto")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// A complete homework list and its provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnHomeworkListResultDto {
    pub data: LearnHomeworkListDto,
    pub metadata: ReadMetadataDto,
}

/// A type of attachment associated with a homework detail section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnHomeworkAttachmentKindDto {
    Assignment,
    Answer,
    Submitted,
    Grade,
    Unknown,
}

impl From<HomeworkAttachmentKind> for LearnHomeworkAttachmentKindDto {
    fn from(value: HomeworkAttachmentKind) -> Self {
        match value {
            HomeworkAttachmentKind::Assignment => Self::Assignment,
            HomeworkAttachmentKind::Answer => Self::Answer,
            HomeworkAttachmentKind::Submitted => Self::Submitted,
            HomeworkAttachmentKind::Grade => Self::Grade,
            _ => Self::Unknown,
        }
    }
}

/// Read-only attachment metadata without a download URL.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnHomeworkAttachmentDto {
    pub kind: LearnHomeworkAttachmentKindDto,
    pub name: String,
    pub size: Option<String>,
}

impl fmt::Debug for LearnHomeworkAttachmentDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnHomeworkAttachmentDto")
            .field("kind", &self.kind)
            .field("name_present", &!self.name.is_empty())
            .field("size_present", &self.size.is_some())
            .finish()
    }
}

/// Read-only assignment body and attachment metadata.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnHomeworkDetailDto {
    pub description: Option<String>,
    pub answer_content: Option<String>,
    pub submitted_content: Option<String>,
    pub attachments: Vec<LearnHomeworkAttachmentDto>,
}

impl fmt::Debug for LearnHomeworkDetailDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnHomeworkDetailDto")
            .field("description_present", &self.description.is_some())
            .field("answer_content_present", &self.answer_content.is_some())
            .field(
                "submitted_content_present",
                &self.submitted_content.is_some(),
            )
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

/// Homework detail and its live-source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnHomeworkDetailResultDto {
    pub data: LearnHomeworkDetailDto,
    pub metadata: ReadMetadataDto,
}

/// One file from a bounded Learn course-file read.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseFileDto {
    pub reference_id: String,
    pub title: String,
    pub suggested_filename: String,
    pub description: Option<String>,
    pub size_label: Option<String>,
    pub uploaded_at_label: Option<String>,
    pub file_type: Option<String>,
}

impl fmt::Debug for LearnCourseFileDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseFileDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("title_present", &!self.title.is_empty())
            .field(
                "suggested_filename_present",
                &!self.suggested_filename.is_empty(),
            )
            .field("description_present", &self.description.is_some())
            .field("size_present", &self.size_label.is_some())
            .field("uploaded_at_present", &self.uploaded_at_label.is_some())
            .field("file_type_present", &self.file_type.is_some())
            .finish()
    }
}

/// One verified course-file list and its completeness state.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseFilesDto {
    pub items: Vec<LearnCourseFileDto>,
    pub coverage: ReadCoverageDto,
}

impl fmt::Debug for LearnCourseFilesDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseFilesDto")
            .field("item_count", &self.items.len())
            .field("coverage", &self.coverage)
            .finish()
    }
}

/// Course files with provenance and completeness metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseFilesResultDto {
    pub data: LearnCourseFilesDto,
    pub metadata: ReadMetadataDto,
}

/// One category label from the Learn course files page.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseFileCategoryDto {
    pub title: String,
}

impl fmt::Debug for LearnCourseFileCategoryDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseFileCategoryDto")
            .field("title_present", &!self.title.is_empty())
            .finish()
    }
}

/// Course file category labels from one live read.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseFileCategoriesDto {
    pub items: Vec<LearnCourseFileCategoryDto>,
}

impl fmt::Debug for LearnCourseFileCategoriesDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseFileCategoriesDto")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// Course file categories and live-source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseFileCategoriesResultDto {
    pub data: LearnCourseFileCategoriesDto,
    pub metadata: ReadMetadataDto,
}

/// One topic from the live course discussion list.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseDiscussionDto {
    pub title: String,
    pub publisher: String,
    pub published_at_label: String,
    pub last_reply_at_label: Option<String>,
    pub reply_count: u32,
}

impl fmt::Debug for LearnCourseDiscussionDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseDiscussionDto")
            .field("title_present", &!self.title.is_empty())
            .field("publisher_present", &!self.publisher.is_empty())
            .field("published_at_present", &!self.published_at_label.is_empty())
            .field("last_reply_at_present", &self.last_reply_at_label.is_some())
            .field("reply_count", &self.reply_count)
            .finish()
    }
}

/// A course discussion list with explicit completeness.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnCourseDiscussionsDto {
    pub items: Vec<LearnCourseDiscussionDto>,
    pub coverage: ReadCoverageDto,
}

impl fmt::Debug for LearnCourseDiscussionsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnCourseDiscussionsDto")
            .field("item_count", &self.items.len())
            .field("coverage", &self.coverage)
            .finish()
    }
}

/// Course discussions and their live-source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnCourseDiscussionsResultDto {
    pub data: LearnCourseDiscussionsDto,
    pub metadata: ReadMetadataDto,
}

/// Confirmation for a safely saved Learn course file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedLearnCourseFileDto {
    pub bytes_written: u64,
}

/// Target-specific interaction requested by the campus-card service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampusCardInteractionDto {
    PasswordRequired,
}

impl From<CampusCardInteraction> for CampusCardInteractionDto {
    fn from(_: CampusCardInteraction) -> Self {
        Self::PasswordRequired
    }
}

/// Fixed campus-card ledger filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampusCardTransactionTypeDto {
    Any,
    Consumption,
    Recharge,
    Subsidy,
    Unknown,
}

impl From<CampusCardTransactionType> for CampusCardTransactionTypeDto {
    fn from(value: CampusCardTransactionType) -> Self {
        match value {
            CampusCardTransactionType::Any => Self::Any,
            CampusCardTransactionType::Consumption => Self::Consumption,
            CampusCardTransactionType::Recharge => Self::Recharge,
            CampusCardTransactionType::Subsidy => Self::Subsidy,
            _ => Self::Unknown,
        }
    }
}

impl TryFrom<CampusCardTransactionTypeDto> for CampusCardTransactionType {
    type Error = SdkErrorDto;

    fn try_from(value: CampusCardTransactionTypeDto) -> Result<Self, Self::Error> {
        match value {
            CampusCardTransactionTypeDto::Any => Ok(Self::Any),
            CampusCardTransactionTypeDto::Consumption => Ok(Self::Consumption),
            CampusCardTransactionTypeDto::Recharge => Ok(Self::Recharge),
            CampusCardTransactionTypeDto::Subsidy => Ok(Self::Subsidy),
            CampusCardTransactionTypeDto::Unknown => Err(invalid_input("campus_card")),
        }
    }
}

/// Validated campus-card account display values. Debug omits personal fields.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardAccountDto {
    pub display_name: String,
    pub display_name_latin: Option<String>,
    pub department_name: String,
    pub department_name_latin: Option<String>,
    pub department_id: i64,
    pub gender: Option<String>,
    pub effective_at: String,
    pub valid_until: String,
    pub balance_cents: i64,
    pub card_status: String,
    pub last_transaction_at: String,
    pub daily_limit_cents: i64,
    pub one_time_limit_cents: i64,
}

impl fmt::Debug for CampusCardAccountDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardAccountDto")
            .field("display_name_present", &!self.display_name.is_empty())
            .field("department_name_present", &!self.department_name.is_empty())
            .field("has_balance", &true)
            .field("card_status_present", &!self.card_status.is_empty())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusCardAccountResultDto {
    pub data: CampusCardAccountDto,
    pub metadata: ReadMetadataDto,
}

/// One validated campus-card ledger row without an upstream transaction ID.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardTransactionDto {
    pub summary: String,
    pub occurred_at: String,
    pub post_balance_cents: i64,
    pub amount_cents: i64,
    pub merchant_address: String,
    pub merchant_name: Option<String>,
    pub transaction_name: String,
}

impl fmt::Debug for CampusCardTransactionDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardTransactionDto")
            .field("summary_present", &!self.summary.is_empty())
            .field("timestamp_present", &!self.occurred_at.is_empty())
            .field("amount_present", &true)
            .field("merchant_present", &self.merchant_name.is_some())
            .finish()
    }
}

/// A complete transaction range and all its validated rows.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardTransactionsDto {
    pub start_date: String,
    pub end_date: String,
    pub transaction_type: CampusCardTransactionTypeDto,
    pub items: Vec<CampusCardTransactionDto>,
}

impl fmt::Debug for CampusCardTransactionsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardTransactionsDto")
            .field("start_date", &self.start_date)
            .field("end_date", &self.end_date)
            .field("transaction_type", &self.transaction_type)
            .field("item_count", &self.items.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampusCardTransactionsResultDto {
    pub data: CampusCardTransactionsDto,
    pub metadata: ReadMetadataDto,
}

/// Numeric remainder and source time label. The service has not established a unit.
#[derive(Clone, PartialEq)]
pub struct ElectricityRemainderDto {
    pub value: f64,
    pub update_time: String,
}

impl fmt::Debug for ElectricityRemainderDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityRemainderDto")
            .field("value_present", &true)
            .field("update_time_present", &!self.update_time.is_empty())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityRemainderResultDto {
    pub data: ElectricityRemainderDto,
    pub metadata: ReadMetadataDto,
}

#[derive(Clone, PartialEq)]
pub struct ElectricityPaymentRecordDto {
    pub occurred_at: String,
    pub amount: f64,
    pub status: String,
}

impl fmt::Debug for ElectricityPaymentRecordDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityPaymentRecordDto")
            .field("occurred_at_present", &!self.occurred_at.is_empty())
            .field("amount_present", &true)
            .field("status_present", &!self.status.is_empty())
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ElectricityPaymentHistoryDto {
    pub empty: bool,
    pub records: Vec<ElectricityPaymentRecordDto>,
}

impl fmt::Debug for ElectricityPaymentHistoryDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityPaymentHistoryDto")
            .field("empty", &self.empty)
            .field("record_count", &self.records.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElectricityPaymentHistoryResultDto {
    pub data: ElectricityPaymentHistoryDto,
    pub metadata: ReadMetadataDto,
}

/// TUNet-only registration evidence for the current local IPv4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalAddressRegistrationDto {
    Registered,
    NotRegistered,
    Unknown,
}

impl From<PortalAddressRegistration> for PortalAddressRegistrationDto {
    fn from(value: PortalAddressRegistration) -> Self {
        match value {
            PortalAddressRegistration::Registered => Self::Registered,
            PortalAddressRegistration::NotRegistered => Self::NotRegistered,
            PortalAddressRegistration::Unknown => Self::Unknown,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalObservationDto {
    pub registration: PortalAddressRegistrationDto,
    pub observed_at_utc: String,
}

/// State explicitly confirmed by a Portal connect or disconnect operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalConnectionStateDto {
    Connected,
    Disconnected,
    Unknown,
}

impl From<PortalConnectionState> for PortalConnectionStateDto {
    fn from(value: PortalConnectionState) -> Self {
        match value {
            PortalConnectionState::Connected => Self::Connected,
            PortalConnectionState::Disconnected => Self::Disconnected,
            _ => Self::Unknown,
        }
    }
}

/// Result of one explicitly requested, positively verified Portal operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalConnectionResultDto {
    pub state: PortalConnectionStateDto,
    pub observed_at_utc: String,
}

fn portal_connection_result(value: PortalConnectionResult) -> PortalConnectionResultDto {
    PortalConnectionResultDto {
        state: value.state().into(),
        observed_at_utc: value
            .observed_at()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }
}

/// The only dates currently accepted by the library service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryDayDto {
    Today,
    Tomorrow,
    Unknown,
}

/// Socket state for one seat from the independent library socket service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySocketStateDto {
    Available,
    Unavailable,
    Unknown,
}

impl From<LibrarySocketState> for LibrarySocketStateDto {
    fn from(value: LibrarySocketState) -> Self {
        match value {
            LibrarySocketState::Available => Self::Available,
            LibrarySocketState::Unavailable => Self::Unavailable,
            LibrarySocketState::Unknown => Self::Unknown,
        }
    }
}

/// One root location in the latest library directory.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryPlaceDto {
    pub reference_id: Option<String>,
    pub name: String,
    pub english_name: Option<String>,
    pub is_valid: Option<bool>,
    pub total_seats: Option<u64>,
    pub available_seats: Option<u64>,
}

impl fmt::Debug for LibraryPlaceDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryPlaceDto")
            .field("reference_present", &self.reference_id.is_some())
            .field("name", &self.name)
            .field("english_name_present", &self.english_name.is_some())
            .field("is_valid", &self.is_valid)
            .field("total_seats", &self.total_seats)
            .field("available_seats", &self.available_seats)
            .finish()
    }
}

/// Root library locations returned by one verified directory read.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryDirectoryDto {
    pub libraries: Vec<LibraryPlaceDto>,
}

impl fmt::Debug for LibraryDirectoryDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryDirectoryDto")
            .field("library_count", &self.libraries.len())
            .finish()
    }
}

/// Root library locations and read provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryDirectoryResultDto {
    pub data: LibraryDirectoryDto,
    pub metadata: ReadMetadataDto,
}

/// One floor in a selected library.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryFloorDto {
    pub reference_id: Option<String>,
    pub name: String,
    pub is_valid: Option<bool>,
    pub total_seats: Option<u64>,
    pub available_seats: Option<u64>,
}

impl fmt::Debug for LibraryFloorDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryFloorDto")
            .field("reference_present", &self.reference_id.is_some())
            .field("name", &self.name)
            .field("is_valid", &self.is_valid)
            .field("total_seats", &self.total_seats)
            .field("available_seats", &self.available_seats)
            .finish()
    }
}

/// Floors returned for one selected library.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryFloorsDto {
    pub items: Vec<LibraryFloorDto>,
}

impl fmt::Debug for LibraryFloorsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryFloorsDto")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// Floors and source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFloorsResultDto {
    pub data: LibraryFloorsDto,
    pub metadata: ReadMetadataDto,
}

/// One section in a floor for the selected campus date.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySectionDto {
    pub reference_id: Option<String>,
    pub name: String,
    pub is_valid: Option<bool>,
    pub total_seats: Option<u64>,
    pub available_seats: Option<u64>,
}

impl fmt::Debug for LibrarySectionDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySectionDto")
            .field("reference_present", &self.reference_id.is_some())
            .field("name", &self.name)
            .field("is_valid", &self.is_valid)
            .field("total_seats", &self.total_seats)
            .field("available_seats", &self.available_seats)
            .finish()
    }
}

/// Sections and their verified campus-local date.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySectionsDto {
    pub day: String,
    pub items: Vec<LibrarySectionDto>,
}

impl fmt::Debug for LibrarySectionsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySectionsDto")
            .field("day", &self.day)
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// Sections and source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySectionsResultDto {
    pub data: LibrarySectionsDto,
    pub metadata: ReadMetadataDto,
}

/// One opening time window from the selected section's current read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTimeWindowDto {
    pub reference_id: String,
    pub starts_at: String,
    pub ends_at: String,
}

/// Opening windows and their campus-local date.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryTimeWindowsDto {
    pub day: String,
    pub items: Vec<LibraryTimeWindowDto>,
}

impl fmt::Debug for LibraryTimeWindowsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryTimeWindowsDto")
            .field("day", &self.day)
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// Opening windows and source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTimeWindowsResultDto {
    pub data: LibraryTimeWindowsDto,
    pub metadata: ReadMetadataDto,
}

/// One seat returned by a live library availability request.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySeatDto {
    pub reference_id: String,
    pub name: String,
    pub is_available: bool,
}

impl fmt::Debug for LibrarySeatDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySeatDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("name", &self.name)
            .field("is_available", &self.is_available)
            .finish()
    }
}

/// Live seats and the date selected by the returned availability.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryAvailabilityDto {
    pub reference_id: String,
    pub day: String,
    pub seats: Vec<LibrarySeatDto>,
}

impl fmt::Debug for LibraryAvailabilityDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryAvailabilityDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("day", &self.day)
            .field("seat_count", &self.seats.len())
            .finish()
    }
}

/// Seat availability and live-source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryAvailabilityResultDto {
    pub data: LibraryAvailabilityDto,
    pub metadata: ReadMetadataDto,
}

/// Socket state associated with a seat in one availability result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySocketStatusDto {
    pub seat_reference_id: String,
    pub state: LibrarySocketStateDto,
}

/// Independent socket results for an availability read.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySocketsDto {
    pub statuses: Vec<LibrarySocketStatusDto>,
}

impl fmt::Debug for LibrarySocketsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibrarySocketsDto")
            .field("status_count", &self.statuses.len())
            .finish()
    }
}

/// Socket results and independent-source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySocketsResultDto {
    pub data: LibrarySocketsDto,
    pub metadata: ReadMetadataDto,
}

/// One building returned by the current classroom directory.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomBuildingDto {
    pub reference_id: Option<String>,
    pub name: String,
    pub default_week: Option<u32>,
}

impl fmt::Debug for ClassroomBuildingDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClassroomBuildingDto")
            .field("reference_present", &self.reference_id.is_some())
            .field("name", &self.name)
            .field("default_week", &self.default_week)
            .finish()
    }
}

/// The full building directory.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomBuildingsDto {
    pub items: Vec<ClassroomBuildingDto>,
}

impl fmt::Debug for ClassroomBuildingsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClassroomBuildingsDto")
            .field("building_count", &self.items.len())
            .finish()
    }
}

/// Building directory and cache provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassroomBuildingsResultDto {
    pub data: ClassroomBuildingsDto,
    pub metadata: ReadMetadataDto,
}

/// Normalized room-status categories. Unknown labels are carried separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassroomSlotStatusDto {
    Free,
    Occupied,
    Exam,
    Borrowed,
    Disabled,
    Unknown,
}

/// One period in a classroom's Monday-first 42-period week.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomSlotDto {
    pub status: ClassroomSlotStatusDto,
    pub unknown_class_name: Option<String>,
}

impl fmt::Debug for ClassroomSlotDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClassroomSlotDto")
            .field("status", &self.status)
            .field(
                "unknown_class_name_present",
                &self.unknown_class_name.is_some(),
            )
            .finish()
    }
}

/// One classroom and its fixed-width weekly slot matrix.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomRoomDto {
    pub name: String,
    pub slots: Vec<ClassroomSlotDto>,
}

impl fmt::Debug for ClassroomRoomDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClassroomRoomDto")
            .field("name", &self.name)
            .field("slot_count", &self.slots.len())
            .finish()
    }
}

/// One validated building/week matrix with Monday-first campus dates.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomAvailabilityDto {
    pub week: u32,
    pub valid_weeks: Vec<u32>,
    pub week_dates: Vec<String>,
    pub rooms: Vec<ClassroomRoomDto>,
}

impl fmt::Debug for ClassroomAvailabilityDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClassroomAvailabilityDto")
            .field("week", &self.week)
            .field("valid_week_count", &self.valid_weeks.len())
            .field("room_count", &self.rooms.len())
            .finish()
    }
}

/// A classroom matrix and its source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassroomAvailabilityResultDto {
    pub data: ClassroomAvailabilityDto,
    pub metadata: ReadMetadataDto,
}

/// One read-only pending item returned by the online service hall.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallTaskDto {
    pub title: String,
    pub status: String,
    pub step: String,
    pub application_time: String,
    pub progress_percent: Option<u32>,
}

impl fmt::Debug for ServiceHallTaskDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallTaskDto")
            .field("title_present", &!self.title.is_empty())
            .field("status_present", &!self.status.is_empty())
            .field("step_present", &!self.step.is_empty())
            .field(
                "application_time_present",
                &!self.application_time.is_empty(),
            )
            .field("progress_percent", &self.progress_percent)
            .finish()
    }
}

/// Pending tasks and the count reported separately by the service homepage.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallPendingDto {
    pub tasks: Vec<ServiceHallTaskDto>,
    pub reported_pending_count: u32,
    pub coverage: ReadCoverageDto,
}

impl fmt::Debug for ServiceHallPendingDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallPendingDto")
            .field("task_count", &self.tasks.len())
            .field("reported_pending_count", &self.reported_pending_count)
            .field("coverage", &self.coverage)
            .finish()
    }
}

/// A successful pending-task read with its source and completeness evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHallPendingResultDto {
    pub data: ServiceHallPendingDto,
    pub metadata: ReadMetadataDto,
}

fn service_hall_pending_result(value: ReadResult<PendingTasks>) -> ServiceHallPendingResultDto {
    let (data, metadata) = value.into_parts();
    let coverage = data.coverage().into();
    ServiceHallPendingResultDto {
        data: ServiceHallPendingDto {
            tasks: data
                .items()
                .iter()
                .map(|task| ServiceHallTaskDto {
                    title: task.title().to_owned(),
                    status: task.status().to_owned(),
                    step: task.step().to_owned(),
                    application_time: task.application_time().to_owned(),
                    progress_percent: task.progress_percent(),
                })
                .collect(),
            reported_pending_count: data.reported_pending_count(),
            coverage,
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One fixed read-only service-hall task view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceHallTaskViewDto {
    Completed,
    Drafts,
    Copies,
    Phases,
    Unknown,
}

impl TryFrom<ServiceHallTaskViewDto> for TaskView {
    type Error = SdkErrorDto;

    fn try_from(value: ServiceHallTaskViewDto) -> Result<Self, Self::Error> {
        match value {
            ServiceHallTaskViewDto::Completed => Ok(Self::Completed),
            ServiceHallTaskViewDto::Drafts => Ok(Self::Drafts),
            ServiceHallTaskViewDto::Copies => Ok(Self::Copies),
            ServiceHallTaskViewDto::Phases => Ok(Self::Phases),
            ServiceHallTaskViewDto::Unknown => Err(invalid_input("service_hall")),
        }
    }
}

impl From<TaskView> for ServiceHallTaskViewDto {
    fn from(value: TaskView) -> Self {
        match value {
            TaskView::Completed => Self::Completed,
            TaskView::Drafts => Self::Drafts,
            TaskView::Copies => Self::Copies,
            TaskView::Phases => Self::Phases,
            _ => Self::Unknown,
        }
    }
}

/// A read-only catalogue entry from the online service hall.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallServiceDto {
    pub name: String,
    pub department: String,
    pub kind: Option<String>,
    pub in_open_period: Option<bool>,
}

impl fmt::Debug for ServiceHallServiceDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallServiceDto")
            .field("name_present", &!self.name.is_empty())
            .field("department_present", &!self.department.is_empty())
            .field("kind_present", &self.kind.is_some())
            .field("in_open_period", &self.in_open_period)
            .finish()
    }
}

/// The service directory and its separately reported upstream total.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallDirectoryDto {
    pub services: Vec<ServiceHallServiceDto>,
    pub reported_total: u32,
    pub coverage: ReadCoverageDto,
}

impl fmt::Debug for ServiceHallDirectoryDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallDirectoryDto")
            .field("service_count", &self.services.len())
            .field("reported_total", &self.reported_total)
            .field("coverage", &self.coverage)
            .finish()
    }
}

/// A successful service-directory read with provenance and completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHallDirectoryResultDto {
    pub data: ServiceHallDirectoryDto,
    pub metadata: ReadMetadataDto,
}

fn service_hall_directory_result(
    value: ReadResult<ServiceDirectory>,
) -> ServiceHallDirectoryResultDto {
    let (data, metadata) = value.into_parts();
    ServiceHallDirectoryResultDto {
        data: ServiceHallDirectoryDto {
            services: data
                .items()
                .iter()
                .map(|service| ServiceHallServiceDto {
                    name: service.name().to_owned(),
                    department: service.department().to_owned(),
                    kind: service.kind().map(str::to_owned),
                    in_open_period: service.in_open_period(),
                })
                .collect(),
            reported_total: data.reported_total(),
            coverage: data.coverage().into(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// A read-only workflow task. The optional phase handle is opaque and scoped
/// to the Client and complete phase-list generation that produced it.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallWorkflowTaskDto {
    pub title: String,
    pub status: String,
    pub step: String,
    pub application_time: String,
    pub progress_percent: Option<u32>,
    pub phase_reference_id: Option<String>,
}

impl fmt::Debug for ServiceHallWorkflowTaskDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallWorkflowTaskDto")
            .field("title_present", &!self.title.is_empty())
            .field("status_present", &!self.status.is_empty())
            .field("step_present", &!self.step.is_empty())
            .field(
                "application_time_present",
                &!self.application_time.is_empty(),
            )
            .field("progress_percent", &self.progress_percent)
            .field(
                "phase_reference_present",
                &self.phase_reference_id.is_some(),
            )
            .finish()
    }
}

/// One completed, draft, copied, or phased workflow view.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallTaskListDto {
    pub view: ServiceHallTaskViewDto,
    pub tasks: Vec<ServiceHallWorkflowTaskDto>,
    pub reported_total: u32,
    pub coverage: ReadCoverageDto,
}

impl fmt::Debug for ServiceHallTaskListDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallTaskListDto")
            .field("view", &self.view)
            .field("task_count", &self.tasks.len())
            .field("reported_total", &self.reported_total)
            .field("coverage", &self.coverage)
            .finish()
    }
}

/// A successful task-list read with provenance and completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHallTaskListResultDto {
    pub data: ServiceHallTaskListDto,
    pub metadata: ReadMetadataDto,
}

fn service_hall_task_list_result(
    value: ReadResult<WorkflowTaskList>,
    policy: ServiceHallReadPolicyDto,
    phase_references: &mut HashMap<String, WorkflowTaskRef>,
    phase_reference_ids: &mut HashMap<WorkflowTaskRef, String>,
) -> ServiceHallTaskListResultDto {
    let (data, metadata) = value.into_parts();
    let view: ServiceHallTaskViewDto = data.view().into();
    let phase_context_changed = policy == ServiceHallReadPolicyDto::Refresh
        || metadata.source() == ReadSource::Live
        || metadata.freshness() == CacheFreshness::Stale;
    if data.view() == TaskView::Phases && phase_context_changed {
        phase_references.clear();
        phase_reference_ids.clear();
    }
    let tasks = data
        .items()
        .iter()
        .map(|task| {
            let phase_reference_id = task.phase_reference().map(|reference| {
                let id = phase_reference_ids
                    .entry(reference.clone())
                    .or_insert_with(|| uuid::Uuid::new_v4().to_string())
                    .clone();
                phase_references.insert(id.clone(), reference.clone());
                id
            });
            ServiceHallWorkflowTaskDto {
                title: task.title().to_owned(),
                status: task.status().to_owned(),
                step: task.step().to_owned(),
                application_time: task.application_time().to_owned(),
                progress_percent: task.progress_percent(),
                phase_reference_id,
            }
        })
        .collect();
    ServiceHallTaskListResultDto {
        data: ServiceHallTaskListDto {
            view,
            tasks,
            reported_total: data.reported_total(),
            coverage: data.coverage().into(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One display-only item attached to a workflow stage.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallPhaseItemDto {
    pub name: String,
    pub state: String,
}

impl fmt::Debug for ServiceHallPhaseItemDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallPhaseItemDto")
            .field("name_present", &!self.name.is_empty())
            .field("state_present", &!self.state.is_empty())
            .finish()
    }
}

/// One verified stage in a multi-step service-hall workflow.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallPhaseStepDto {
    pub order: String,
    pub name: String,
    pub state: String,
    pub items: Vec<ServiceHallPhaseItemDto>,
}

impl fmt::Debug for ServiceHallPhaseStepDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallPhaseStepDto")
            .field("order_present", &!self.order.is_empty())
            .field("name_present", &!self.name.is_empty())
            .field("state_present", &!self.state.is_empty())
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// The verified stage detail for an opaque current phase-list selection.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceHallPhaseDetailsDto {
    pub steps: Vec<ServiceHallPhaseStepDto>,
}

impl fmt::Debug for ServiceHallPhaseDetailsDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceHallPhaseDetailsDto")
            .field("step_count", &self.steps.len())
            .finish()
    }
}

/// A successful workflow-stage read with provenance and completeness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHallPhaseDetailsResultDto {
    pub data: ServiceHallPhaseDetailsDto,
    pub metadata: ReadMetadataDto,
}

fn service_hall_phase_details_result(
    value: ReadResult<PhaseDetails>,
) -> ServiceHallPhaseDetailsResultDto {
    let (data, metadata) = value.into_parts();
    ServiceHallPhaseDetailsResultDto {
        data: ServiceHallPhaseDetailsDto {
            steps: data
                .steps()
                .iter()
                .map(|step| ServiceHallPhaseStepDto {
                    order: step.order().to_owned(),
                    name: step.name().to_owned(),
                    state: step.state().to_owned(),
                    items: step
                        .items()
                        .iter()
                        .map(|item| ServiceHallPhaseItemDto {
                            name: item.name().to_owned(),
                            state: item.state().to_owned(),
                        })
                        .collect(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// Account details returned by the independently authenticated SelfService
/// account page.
#[derive(Clone, PartialEq, Eq)]
pub struct SelfServiceAccountDto {
    pub contact_email: String,
    pub contact_phone: String,
    pub contact_landline: String,
    pub real_name: String,
    pub status: String,
    pub user_group: String,
    pub location: String,
    pub allowed_devices: u32,
}

impl fmt::Debug for SelfServiceAccountDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfServiceAccountDto")
            .field("contact_email_present", &!self.contact_email.is_empty())
            .field("contact_phone_present", &!self.contact_phone.is_empty())
            .field(
                "contact_landline_present",
                &!self.contact_landline.is_empty(),
            )
            .field("real_name_present", &!self.real_name.is_empty())
            .field("status_present", &!self.status.is_empty())
            .field("user_group_present", &!self.user_group.is_empty())
            .field("location_present", &!self.location.is_empty())
            .field("allowed_devices", &self.allowed_devices)
            .finish()
    }
}

/// A successful SelfService account read with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfServiceAccountResultDto {
    pub data: SelfServiceAccountDto,
    pub metadata: ReadMetadataDto,
}

fn self_service_account_result(value: ReadResult<AccountProfile>) -> SelfServiceAccountResultDto {
    let (account, metadata) = value.into_parts();
    SelfServiceAccountResultDto {
        data: SelfServiceAccountDto {
            contact_email: account.contact_email().to_owned(),
            contact_phone: account.contact_phone().to_owned(),
            contact_landline: account.contact_landline().to_owned(),
            real_name: account.real_name().to_owned(),
            status: account.status().to_owned(),
            user_group: account.user_group().to_owned(),
            location: account.location().to_owned(),
            allowed_devices: account.allowed_devices(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One currently online SelfService device. The reference is opaque and
/// cannot be constructed from its address or display position.
#[derive(Clone, PartialEq, Eq)]
pub struct SelfServiceDeviceDto {
    pub reference_id: String,
    pub ipv4: String,
    pub ipv6: String,
    pub logged_at: String,
    pub authorization: String,
    pub mac_suffix: String,
}

impl fmt::Debug for SelfServiceDeviceDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfServiceDeviceDto")
            .field("reference_present", &!self.reference_id.is_empty())
            .field("ipv4_present", &!self.ipv4.is_empty())
            .field("ipv6_present", &!self.ipv6.is_empty())
            .field("logged_at_present", &!self.logged_at.is_empty())
            .field("authorization_present", &!self.authorization.is_empty())
            .field("mac_suffix_present", &!self.mac_suffix.is_empty())
            .finish()
    }
}

/// A successful complete device-directory read with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfServiceDevicesResultDto {
    pub devices: Vec<SelfServiceDeviceDto>,
    pub metadata: ReadMetadataDto,
}

fn self_service_devices_result(
    value: ReadResult<Vec<OnlineDevice>>,
    references: &mut HashMap<String, DeviceRef>,
) -> SelfServiceDevicesResultDto {
    let (devices, metadata) = value.into_parts();
    references.clear();
    SelfServiceDevicesResultDto {
        devices: devices
            .iter()
            .map(|device| {
                let reference_id = uuid::Uuid::new_v4().to_string();
                references.insert(reference_id.clone(), device.device_ref().clone());
                SelfServiceDeviceDto {
                    reference_id,
                    ipv4: device.ipv4().to_owned(),
                    ipv6: device.ipv6().to_owned(),
                    logged_at: device.logged_at().to_owned(),
                    authorization: device.authorization().to_owned(),
                    mac_suffix: device.mac_suffix().to_owned(),
                }
            })
            .collect(),
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// Service-formatted SelfService usage, balance, and settlement values.
#[derive(Clone, PartialEq, Eq)]
pub struct SelfServiceUsageDto {
    pub product_name: String,
    pub used_bytes: String,
    pub used_seconds: String,
    pub account_balance: String,
    pub settlement_date: String,
}

impl fmt::Debug for SelfServiceUsageDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfServiceUsageDto")
            .field("product_name_present", &!self.product_name.is_empty())
            .field(
                "usage_present",
                &(!self.used_bytes.is_empty() || !self.used_seconds.is_empty()),
            )
            .field("balance_present", &!self.account_balance.is_empty())
            .field("settlement_date_present", &!self.settlement_date.is_empty())
            .finish()
    }
}

/// A successful usage and balance read with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfServiceUsageResultDto {
    pub data: SelfServiceUsageDto,
    pub metadata: ReadMetadataDto,
}

fn self_service_usage_result(value: ReadResult<UsageBalance>) -> SelfServiceUsageResultDto {
    let (usage, metadata) = value.into_parts();
    SelfServiceUsageResultDto {
        data: SelfServiceUsageDto {
            product_name: usage.product_name().to_owned(),
            used_bytes: usage.used_bytes().to_owned(),
            used_seconds: usage.used_seconds().to_owned(),
            account_balance: usage.account_balance().to_owned(),
            settlement_date: usage.settlement_date().to_owned(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One term returned by the authenticated Learn calendar.
#[derive(Clone, PartialEq, Eq)]
pub struct AcademicTermDto {
    pub label: String,
    pub starts_on: String,
    pub ends_on: String,
    pub teaching_week_one: String,
    pub week_count: u32,
}

impl fmt::Debug for AcademicTermDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcademicTermDto")
            .field("label_present", &!self.label.is_empty())
            .field("starts_on", &self.starts_on)
            .field("ends_on", &self.ends_on)
            .field("teaching_week_one", &self.teaching_week_one)
            .field("week_count", &self.week_count)
            .finish()
    }
}

fn academic_term_dto(value: &AcademicTerm) -> AcademicTermDto {
    AcademicTermDto {
        label: value.label().to_owned(),
        starts_on: value.starts_on().to_string(),
        ends_on: value.ends_on().to_string(),
        teaching_week_one: value.teaching_week_one().to_string(),
        week_count: value.week_count(),
    }
}

/// Current and following academic terms from Learn.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnTermCalendarDto {
    pub current: AcademicTermDto,
    pub upcoming: Vec<AcademicTermDto>,
}

impl fmt::Debug for LearnTermCalendarDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnTermCalendarDto")
            .field("current", &self.current)
            .field("upcoming_count", &self.upcoming.len())
            .finish()
    }
}

/// A successful Learn-term read with provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnTermCalendarResultDto {
    pub data: LearnTermCalendarDto,
    pub metadata: ReadMetadataDto,
}

fn learn_term_calendar_result(value: ReadResult<LearnTermCalendar>) -> LearnTermCalendarResultDto {
    let (calendar, metadata) = value.into_parts();
    LearnTermCalendarResultDto {
        data: LearnTermCalendarDto {
            current: academic_term_dto(calendar.current()),
            upcoming: calendar.upcoming().iter().map(academic_term_dto).collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// A validated, published school-calendar image.
#[derive(Clone, PartialEq, Eq)]
pub struct SchoolCalendarImageDto {
    pub latest_year: u32,
    pub year: u32,
    pub semester: SchoolCalendarSemesterDto,
    pub language: SchoolCalendarLanguageDto,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for SchoolCalendarImageDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchoolCalendarImageDto")
            .field("latest_year", &self.latest_year)
            .field("year", &self.year)
            .field("semester", &self.semester)
            .field("language", &self.language)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

/// A successful school-calendar image read with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchoolCalendarImageResultDto {
    pub data: SchoolCalendarImageDto,
    pub metadata: ReadMetadataDto,
}

fn school_calendar_image_result(
    value: ReadResult<SchoolCalendarImage>,
) -> SchoolCalendarImageResultDto {
    let (image, metadata) = value.into_parts();
    SchoolCalendarImageResultDto {
        data: SchoolCalendarImageDto {
            latest_year: image.latest_year(),
            year: image.year(),
            semester: match image.semester() {
                SchoolCalendarSemester::Autumn => SchoolCalendarSemesterDto::Autumn,
                SchoolCalendarSemester::Spring => SchoolCalendarSemesterDto::Spring,
                _ => SchoolCalendarSemesterDto::Unknown,
            },
            language: match image.language() {
                SchoolCalendarLanguage::Chinese => SchoolCalendarLanguageDto::Chinese,
                SchoolCalendarLanguage::English => SchoolCalendarLanguageDto::English,
                _ => SchoolCalendarLanguageDto::Unknown,
            },
            bytes: image.bytes().to_vec(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One schedule event returned by the authenticated Registrar service.
#[derive(Clone, PartialEq, Eq)]
pub struct ScheduleEventDto {
    pub title: String,
    pub kind: ScheduleEventKindDto,
    pub starts_at_utc: String,
    pub ends_at_utc: Option<String>,
    pub is_all_day: bool,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl fmt::Debug for ScheduleEventDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScheduleEventDto")
            .field("title_present", &!self.title.is_empty())
            .field("kind", &self.kind)
            .field("starts_at_utc", &self.starts_at_utc)
            .field("ends_at_present", &self.ends_at_utc.is_some())
            .field("is_all_day", &self.is_all_day)
            .field("location_present", &self.location.is_some())
            .field("description_present", &self.description.is_some())
            .finish()
    }
}

fn schedule_event_dto(value: &ScheduleEvent) -> ScheduleEventDto {
    ScheduleEventDto {
        title: value.title().to_owned(),
        kind: value.kind().into(),
        starts_at_utc: value
            .starts_at()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ends_at_utc: value
            .ends_at()
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        is_all_day: value.is_all_day(),
        location: value.location().map(str::to_owned),
        description: value.description().map(str::to_owned),
    }
}

/// A complete schedule for the selected Registrar semester.
#[derive(Clone, PartialEq, Eq)]
pub struct SemesterScheduleDto {
    pub semester: String,
    pub stage: AcademicStageDto,
    pub first_day: String,
    pub last_day: String,
    pub week_count: u32,
    pub current_week: u32,
    pub events: Vec<ScheduleEventDto>,
}

impl fmt::Debug for SemesterScheduleDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemesterScheduleDto")
            .field("semester_present", &!self.semester.is_empty())
            .field("stage", &self.stage)
            .field("first_day", &self.first_day)
            .field("last_day", &self.last_day)
            .field("week_count", &self.week_count)
            .field("current_week", &self.current_week)
            .field("event_count", &self.events.len())
            .finish()
    }
}

/// A successful schedule read with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemesterScheduleResultDto {
    pub data: SemesterScheduleDto,
    pub metadata: ReadMetadataDto,
}

fn semester_schedule_result(value: ReadResult<SemesterSchedule>) -> SemesterScheduleResultDto {
    let (schedule, metadata) = value.into_parts();
    SemesterScheduleResultDto {
        data: SemesterScheduleDto {
            semester: schedule.semester().to_owned(),
            stage: schedule.stage().into(),
            first_day: schedule.first_day().to_string(),
            last_day: schedule.last_day().to_string(),
            week_count: schedule.week_count(),
            current_week: schedule.current_week(),
            events: schedule.events().iter().map(schedule_event_dto).collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One course result in a Registrar grade report.
#[derive(Clone, PartialEq)]
pub struct CourseGradeDto {
    pub course_name: String,
    pub credit: f64,
    pub grade: String,
    pub grade_point: Option<f64>,
    pub semester: String,
}

impl fmt::Debug for CourseGradeDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseGradeDto")
            .field("course_name_present", &!self.course_name.is_empty())
            .field("credit", &self.credit)
            .field("grade_present", &!self.grade.is_empty())
            .field("grade_point_present", &self.grade_point.is_some())
            .field("semester_present", &!self.semester.is_empty())
            .finish()
    }
}

/// One complete grade report for the authenticated stage.
#[derive(Clone, PartialEq)]
pub struct GradeReportDto {
    pub stage: AcademicStageDto,
    pub kind: Option<GradeReportKindDto>,
    pub courses: Vec<CourseGradeDto>,
}

impl fmt::Debug for GradeReportDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GradeReportDto")
            .field("stage", &self.stage)
            .field("kind", &self.kind)
            .field("course_count", &self.courses.len())
            .finish()
    }
}

/// A successful grade-report read with source metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct GradeReportResultDto {
    pub data: GradeReportDto,
    pub metadata: ReadMetadataDto,
}

fn grade_report_result(value: ReadResult<GradeReport>) -> GradeReportResultDto {
    let (report, metadata) = value.into_parts();
    GradeReportResultDto {
        data: GradeReportDto {
            stage: report.stage().into(),
            kind: report.kind().map(Into::into),
            courses: report
                .courses()
                .iter()
                .map(|course| CourseGradeDto {
                    course_name: course.course_name().to_owned(),
                    credit: course.credit(),
                    grade: course.grade().to_owned(),
                    grade_point: course.grade_point(),
                    semester: course.semester().to_owned(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// One examination entry in the stage-specific report.
#[derive(Clone, PartialEq, Eq)]
pub struct ExamDto {
    pub course_code: String,
    pub course_sequence: String,
    pub course_name: String,
    pub month: u8,
    pub day: u8,
    pub weekday: ExamWeekdayDto,
    pub schedule_label: String,
    pub location: String,
    pub department: Option<String>,
    pub category: Option<String>,
    pub instructor: Option<String>,
    pub headcount: Option<u32>,
}

impl fmt::Debug for ExamDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExamDto")
            .field("course_code_present", &!self.course_code.is_empty())
            .field("course_name_present", &!self.course_name.is_empty())
            .field("month", &self.month)
            .field("day", &self.day)
            .field("weekday", &self.weekday)
            .field("schedule_present", &!self.schedule_label.is_empty())
            .field("location_present", &!self.location.is_empty())
            .finish()
    }
}

/// A complete stage-specific examination report.
#[derive(Clone, PartialEq, Eq)]
pub struct ExamReportDto {
    pub stage: AcademicStageDto,
    pub exams: Vec<ExamDto>,
}

impl fmt::Debug for ExamReportDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExamReportDto")
            .field("stage", &self.stage)
            .field("exam_count", &self.exams.len())
            .finish()
    }
}

/// A successful exam-report read with source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExamReportResultDto {
    pub data: ExamReportDto,
    pub metadata: ReadMetadataDto,
}

fn exam_report_result(value: ReadResult<ExamReport>) -> ExamReportResultDto {
    let (report, metadata) = value.into_parts();
    ExamReportResultDto {
        data: ExamReportDto {
            stage: report.stage().into(),
            exams: report
                .exams()
                .iter()
                .map(|exam| ExamDto {
                    course_code: exam.course_code().to_owned(),
                    course_sequence: exam.course_sequence().to_owned(),
                    course_name: exam.course_name().to_owned(),
                    month: exam.month(),
                    day: exam.day(),
                    weekday: exam.weekday().into(),
                    schedule_label: exam.schedule_label().to_owned(),
                    location: exam.location().to_owned(),
                    department: exam.department().map(str::to_owned),
                    category: exam.category().map(str::to_owned),
                    instructor: exam.instructor().map(str::to_owned),
                    headcount: exam.headcount(),
                })
                .collect(),
        },
        metadata: ReadMetadataDto::from(&metadata),
    }
}

/// The intended use of a local campus-network profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAccessMethodDto {
    Portal,
    SystemWifiEap,
    Unknown,
}

fn access_method(value: NetworkAccessMethodDto) -> Result<NetworkAccessMethod, SdkErrorDto> {
    match value {
        NetworkAccessMethodDto::Portal => Ok(NetworkAccessMethod::Portal),
        NetworkAccessMethodDto::SystemWifiEap => Ok(NetworkAccessMethod::SystemWifiEap),
        NetworkAccessMethodDto::Unknown => Err(SdkErrorDto {
            service: "network".to_owned(),
            code: "unsupported".to_owned(),
            retry_after_ms: None,
            diagnostic_id: uuid::Uuid::new_v4().to_string(),
        }),
    }
}

fn access_method_dto(value: NetworkAccessMethod) -> NetworkAccessMethodDto {
    match value {
        NetworkAccessMethod::Portal => NetworkAccessMethodDto::Portal,
        NetworkAccessMethod::SystemWifiEap => NetworkAccessMethodDto::SystemWifiEap,
        _ => NetworkAccessMethodDto::Unknown,
    }
}

/// Non-secret display and form fields for one saved local network profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkProfileDto {
    pub id: String,
    pub label: String,
    pub method: NetworkAccessMethodDto,
    pub has_saved_password: bool,
}

impl From<&NetworkProfileSummary> for NetworkProfileDto {
    fn from(value: &NetworkProfileSummary) -> Self {
        Self {
            id: value.id().as_str(),
            label: value.label().to_owned(),
            method: access_method_dto(value.method()),
            has_saved_password: value.has_saved_password(),
        }
    }
}

/// Fields explicitly requested to prepare a local application form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkProfileFillDto {
    pub id: String,
    pub label: String,
    pub username: String,
    pub method: NetworkAccessMethodDto,
    pub has_saved_password: bool,
}

/// A prepared, password-free profile reference scoped to its creating Client.
#[cfg_attr(feature = "ffi-bridge", flutter_rust_bridge::frb(opaque))]
pub struct PreparedNetworkProfile {
    inner: PreparedNetworkInput,
}

impl PreparedNetworkProfile {
    /// Returns form fields only after the caller explicitly prepares a fill.
    /// Password material remains in Rust.
    pub fn form_fields(&self) -> NetworkProfileFillDto {
        let summary = self.inner.summary();
        NetworkProfileFillDto {
            id: summary.id().as_str(),
            label: summary.label().to_owned(),
            username: summary.username().to_owned(),
            method: access_method_dto(summary.method()),
            has_saved_password: summary.has_saved_password(),
        }
    }
}

/// A non-cloneable local handle for an explicitly requested password fill.
#[cfg_attr(feature = "ffi-bridge", flutter_rust_bridge::frb(opaque))]
pub struct NetworkProfilePasswordHandle {
    inner: NetworkProfilePassword,
}

impl NetworkProfilePasswordHandle {
    /// Copies the password into a UI field only after the caller explicitly
    /// requests a fill. The Rust-owned source is zeroized when this handle is
    /// dropped.
    pub fn expose_for_form(&self) -> String {
        self.inner.expose_for_form().to_owned()
    }
}

impl fmt::Debug for NetworkProfilePasswordHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkProfilePasswordHandle")
            .field("present", &true)
            .finish()
    }
}

/// One Flutter-owned handle over the public SDK's single Rust Client.
#[cfg_attr(feature = "ffi-bridge", flutter_rust_bridge::frb(opaque))]
pub struct ClientHandle {
    inner: SdkClient,
    service_hall_phase_references: HashMap<String, WorkflowTaskRef>,
    service_hall_phase_reference_ids: HashMap<WorkflowTaskRef, String>,
    self_service_device_references: HashMap<String, DeviceRef>,
    news_source_references: HashMap<String, NewsSourceRef>,
    news_channel_references: HashMap<String, NewsChannelRef>,
    news_article_references: HashMap<String, ArticleRef>,
    news_subscription_references: HashMap<String, NewsSubscriptionRef>,
    learn_course_references: HashMap<String, CourseRef>,
    learn_homework_references: HashMap<String, HomeworkRef>,
    learn_course_file_references: HashMap<String, CourseFileRef>,
    library_references: HashMap<String, LibraryRef>,
    library_floor_references: HashMap<String, FloorRef>,
    library_section_references: HashMap<String, SectionRef>,
    library_window_references: HashMap<String, SeatWindowRef>,
    library_availability_references: HashMap<String, LibraryAvailability>,
    library_seat_reference_ids: HashMap<SeatRef, String>,
    classroom_building_references: HashMap<String, BuildingRef>,
}

impl ClientHandle {
    /// Creates a Client without login or network activity. Explicitly saved
    /// Auth credentials, Identity snapshots and network profiles are written
    /// as separate readable JSON files under caller-selected directories.
    /// They are not encrypted, and no operating-system credential store is
    /// used.
    pub fn new(
        cache_root: Option<String>,
        credential_storage_root: Option<String>,
        credential_storage_namespace: Option<String>,
        profile_storage_root: Option<String>,
        application_namespace: Option<String>,
        identity_session_root: Option<String>,
        identity_session_namespace: Option<String>,
    ) -> Result<Self, SdkErrorDto> {
        let profile_policy = match (profile_storage_root, application_namespace) {
            (None, None) => NetworkProfileStoragePolicy::MemoryOnly,
            (Some(root), Some(namespace)) => {
                NetworkProfileStoragePolicy::json_directory(PathBuf::from(root), namespace)?
            }
            _ => {
                let Err(error) = NetworkProfileStoragePolicy::json_directory("/", "") else {
                    unreachable!("empty application namespace must be rejected")
                };
                return Err(error.into());
            }
        };
        let credential_policy = match (credential_storage_root, credential_storage_namespace) {
            (None, None) => CredentialStoragePolicy::MemoryOnly,
            (Some(root), Some(namespace)) => CredentialStoragePolicy::JsonDirectory {
                root: PathBuf::from(root),
                namespace,
            },
            _ => {
                let Err(error) = NetworkProfileStoragePolicy::json_directory("/", "") else {
                    unreachable!("empty application namespace must be rejected")
                };
                return Err(error.into());
            }
        };
        let mut builder = ClientBuilder::default()
            .network_profile_storage(profile_policy)
            .credential_storage(credential_policy);
        if let Some(root) = cache_root {
            builder = builder.cache_policy(ClientCachePolicy::Directory(PathBuf::from(root)));
        }
        let identity_session_root = match (identity_session_root, identity_session_namespace) {
            (None, None) => None,
            (Some(root), Some(namespace)) => {
                let scoped =
                    NetworkProfileStoragePolicy::json_directory(PathBuf::from(root), namespace)?;
                let NetworkProfileStoragePolicy::JsonDirectory { root, namespace } = scoped else {
                    unreachable!("the JSON-directory constructor returns its policy")
                };
                Some(
                    root.join("TsinghuaKit")
                        .join("identity-session")
                        .join(namespace),
                )
            }
            _ => {
                let Err(error) = NetworkProfileStoragePolicy::json_directory("/", "") else {
                    unreachable!("empty application namespace must be rejected")
                };
                return Err(error.into());
            }
        };
        if let Some(root) = identity_session_root {
            builder =
                builder.identity_session_storage(IdentitySessionStoragePolicy::JsonDirectory(root));
        }
        let inner = builder.build()?;
        Ok(Self {
            inner,
            service_hall_phase_references: HashMap::new(),
            service_hall_phase_reference_ids: HashMap::new(),
            self_service_device_references: HashMap::new(),
            news_source_references: HashMap::new(),
            news_channel_references: HashMap::new(),
            news_article_references: HashMap::new(),
            news_subscription_references: HashMap::new(),
            learn_course_references: HashMap::new(),
            learn_homework_references: HashMap::new(),
            learn_course_file_references: HashMap::new(),
            library_references: HashMap::new(),
            library_floor_references: HashMap::new(),
            library_section_references: HashMap::new(),
            library_window_references: HashMap::new(),
            library_availability_references: HashMap::new(),
            library_seat_reference_ids: HashMap::new(),
            classroom_building_references: HashMap::new(),
        })
    }

    /// Returns the two independent Auth slots owned by this Client.
    pub fn auth_status(&mut self) -> AuthStatusDto {
        map_auth_status(self.inner.auth().status())
    }

    /// Reads a validated, account-bound campus day. CacheOnly performs no
    /// network request and a missing cache yields an explicit CacheMiss.
    pub async fn overview_day(
        &mut self,
        date: String,
        policy: ReadPolicyDto,
    ) -> Result<DailyOverviewResultDto, SdkErrorDto> {
        let parsed = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
            .map_err(|_| invalid_input("overview"))?;
        if parsed.to_string() != date {
            return Err(invalid_input("overview"));
        }
        let result = self.inner.overview().day(parsed, policy.into()).await?;
        Ok(daily_overview_result(result))
    }

    /// Reads the current Identity second-factor interaction without starting
    /// or replaying a login operation.
    pub fn identity_interaction(&mut self) -> Result<Option<IdentityLoginResultDto>, SdkErrorDto> {
        let interaction = self.inner.auth().identity().interaction()?;
        Ok(interaction.map(|outcome| map_identity_outcome(&mut self.inner, outcome)))
    }

    /// Revalidates an explicitly restored Identity session. A fresh Client
    /// returns its current signed-out state without network I/O.
    pub async fn identity_revalidate_restored_session(
        &mut self,
    ) -> Result<AuthStatusDto, SdkErrorDto> {
        let was_restored =
            self.inner.auth().status().identity().state() == AccountAuthState::RestoredUnverified;
        if was_restored {
            self.invalidate_auth_bound_references();
        }
        let status = self
            .inner
            .auth()
            .identity()
            .revalidate_restored_session()
            .await?;
        Ok(map_auth_status(status))
    }

    /// Reads online service-hall pending tasks through this Client's shared
    /// Runtime and with an explicit cache policy.
    pub async fn service_hall_pending(
        &mut self,
        policy: ServiceHallReadPolicyDto,
    ) -> Result<ServiceHallPendingResultDto, SdkErrorDto> {
        let result = self.inner.service_hall().pending(policy.into()).await?;
        Ok(service_hall_pending_result(result))
    }

    /// Reads the complete online service-hall catalogue through the same
    /// Client and with an explicit cache policy.
    pub async fn service_hall_services(
        &mut self,
        policy: ServiceHallReadPolicyDto,
    ) -> Result<ServiceHallDirectoryResultDto, SdkErrorDto> {
        let result = self.inner.service_hall().services(policy.into()).await?;
        Ok(service_hall_directory_result(result))
    }

    /// Reads one fixed service-hall task view. Phase references remain
    /// opaque, short-lived, and bound to this ClientHandle.
    pub async fn service_hall_tasks(
        &mut self,
        view: ServiceHallTaskViewDto,
        policy: ServiceHallReadPolicyDto,
    ) -> Result<ServiceHallTaskListResultDto, SdkErrorDto> {
        let view = TaskView::try_from(view)?;
        let result = self.inner.service_hall().tasks(view, policy.into()).await?;
        Ok(service_hall_task_list_result(
            result,
            policy,
            &mut self.service_hall_phase_references,
            &mut self.service_hall_phase_reference_ids,
        ))
    }

    /// Reads stage details for a phase-list entry previously returned by this
    /// ClientHandle. Raw service selectors never cross the bridge.
    pub async fn service_hall_phase_details(
        &mut self,
        reference_id: String,
        policy: ServiceHallReadPolicyDto,
    ) -> Result<ServiceHallPhaseDetailsResultDto, SdkErrorDto> {
        let Some(reference) = self
            .service_hall_phase_references
            .get(&reference_id)
            .cloned()
        else {
            return Err(context_mismatch("service_hall"));
        };
        let result = self
            .inner
            .service_hall()
            .phase_details(&reference, policy.into())
            .await?;
        Ok(service_hall_phase_details_result(result))
    }

    /// Reads the current INFO source and channel directory. Returned selectors
    /// are random handles scoped to this ClientHandle and catalog generation.
    pub async fn news_catalog(&mut self) -> Result<NewsCatalogResultDto, SdkErrorDto> {
        let result = self.inner.news().catalog().await?;
        Ok(news_catalog_result(
            result,
            &mut self.news_source_references,
            &mut self.news_channel_references,
        ))
    }

    /// Reads a validated INFO news list page with optional catalog references.
    pub async fn news_articles(
        &mut self,
        page: u32,
        page_size: u32,
        source_reference_id: Option<String>,
        channel_reference_id: Option<String>,
        policy: ReadPolicyDto,
    ) -> Result<NewsPageResultDto, SdkErrorDto> {
        let source = source_reference_id
            .map(|id| {
                self.news_source_references
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| context_mismatch("news"))
            })
            .transpose()?;
        let channel = channel_reference_id
            .map(|id| {
                self.news_channel_references
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| context_mismatch("news"))
            })
            .transpose()?;
        let mut query = NewsQuery::list(page, page_size)?;
        if let Some(source) = source.as_ref() {
            query = query.with_source(source)?;
        }
        if let Some(channel) = channel.as_ref() {
            query = query.with_channel(channel)?;
        }
        let result = self.inner.news().articles(query, policy.into()).await?;
        Ok(news_page_result(result, &mut self.news_article_references))
    }

    /// Searches INFO news using a caller-provided term and optional channel
    /// reference. Query validation and the requested cache policy stay in Rust.
    pub async fn news_search(
        &mut self,
        page: u32,
        keyword: String,
        channel_reference_id: Option<String>,
        exact_match: bool,
        policy: ReadPolicyDto,
    ) -> Result<NewsPageResultDto, SdkErrorDto> {
        let channel = channel_reference_id
            .map(|id| {
                self.news_channel_references
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| context_mismatch("news"))
            })
            .transpose()?;
        let mut query = NewsQuery::search(page, keyword)?;
        if let Some(channel) = channel.as_ref() {
            query = query.with_search_channel(channel)?;
        }
        query = query.exact_match(exact_match)?;
        let result = self.inner.news().articles(query, policy.into()).await?;
        Ok(news_page_result(result, &mut self.news_article_references))
    }

    /// Reads detail only for an article reference returned by this Client's
    /// current page. The FFI identifier never contains the upstream article ID.
    pub async fn news_article(
        &mut self,
        reference_id: String,
        policy: ReadPolicyDto,
    ) -> Result<ArticleDetailResultDto, SdkErrorDto> {
        let Some(reference) = self.news_article_references.get(&reference_id).cloned() else {
            return Err(context_mismatch("news"));
        };
        let result = self.inner.news().article(&reference, policy.into()).await?;
        Ok(article_detail_result(result))
    }

    /// Reads all current-account favorites after Rust proves bounded pagination
    /// complete. The returned article references are bound to this Client.
    pub async fn news_favorites(&mut self) -> Result<NewsFavoritesResultDto, SdkErrorDto> {
        let result = self.inner.news().favorites().await?;
        Ok(news_favorites_result(
            result,
            &mut self.news_article_references,
        ))
    }

    /// Reads the current Identity account's INFO subscriptions and replaces
    /// handles returned by any earlier subscription-list read.
    pub async fn news_subscriptions(&mut self) -> Result<NewsSubscriptionsResultDto, SdkErrorDto> {
        let result = self.inner.news().subscriptions().await?;
        Ok(news_subscriptions_result(
            result,
            &mut self.news_subscription_references,
        ))
    }

    /// Reads one page selected by a handle from this Client's latest
    /// subscription list.
    pub async fn news_subscription_articles(
        &mut self,
        reference_id: String,
        page: u32,
    ) -> Result<NewsPageResultDto, SdkErrorDto> {
        let Some(reference) = self
            .news_subscription_references
            .get(&reference_id)
            .cloned()
        else {
            return Err(context_mismatch("news"));
        };
        let result = self
            .inner
            .news()
            .subscription_articles(&reference, page)
            .await?;
        Ok(news_page_result(result, &mut self.news_article_references))
    }

    /// Reads the complete schedule for the Runtime-selected semester.
    pub async fn registrar_semester_schedule(
        &mut self,
    ) -> Result<SemesterScheduleResultDto, SdkErrorDto> {
        let result = self.inner.registrar().semester_schedule().await?;
        Ok(semester_schedule_result(result))
    }

    /// Reads the verified grade report for the authenticated academic stage.
    pub async fn registrar_grades(&mut self) -> Result<GradeReportResultDto, SdkErrorDto> {
        let result = self.inner.registrar().grades().await?;
        Ok(grade_report_result(result))
    }

    /// Reads the complete examination report for the authenticated stage.
    pub async fn registrar_exams(&mut self) -> Result<ExamReportResultDto, SdkErrorDto> {
        let result = self.inner.registrar().exams().await?;
        Ok(exam_report_result(result))
    }

    /// Reads Learn's current and following academic terms through this Client.
    pub async fn calendar_learn_terms(
        &mut self,
    ) -> Result<LearnTermCalendarResultDto, SdkErrorDto> {
        let result = self.inner.calendar().learn_terms().await?;
        Ok(learn_term_calendar_result(result))
    }

    /// Reads a verified school-calendar image using fixed semester/language
    /// enums and an optional bounded year.
    pub async fn calendar_school_image(
        &mut self,
        year: Option<u32>,
        semester: SchoolCalendarSemesterDto,
        language: SchoolCalendarLanguageDto,
    ) -> Result<SchoolCalendarImageResultDto, SdkErrorDto> {
        let semester = match semester {
            SchoolCalendarSemesterDto::Autumn => SchoolCalendarSemester::Autumn,
            SchoolCalendarSemesterDto::Spring => SchoolCalendarSemester::Spring,
            SchoolCalendarSemesterDto::Unknown => return Err(invalid_input("calendar")),
        };
        let language = match language {
            SchoolCalendarLanguageDto::Chinese => SchoolCalendarLanguage::Chinese,
            SchoolCalendarLanguageDto::English => SchoolCalendarLanguage::English,
            SchoolCalendarLanguageDto::Unknown => return Err(invalid_input("calendar")),
        };
        let query = match year {
            Some(year) => SchoolCalendarQuery::for_year(year, semester, language)?,
            None => SchoolCalendarQuery::latest(semester, language),
        };
        let result = self.inner.calendar().school_calendar(query).await?;
        Ok(school_calendar_image_result(result))
    }

    /// Reads the verified current Learn course catalog. A successful read
    /// replaces course references and invalidates dependent homework/file refs.
    pub async fn learn_courses(&mut self) -> Result<LearnCourseCatalogResultDto, SdkErrorDto> {
        let result = self.inner.learn().courses().await?;
        self.learn_homework_references.clear();
        self.learn_course_file_references.clear();
        Ok(learn_course_catalog_result(
            result,
            &mut self.learn_course_references,
        ))
    }

    /// Reads announcements for a course returned by this Client's latest
    /// course-directory observation.
    pub async fn learn_announcements(
        &mut self,
        course_reference_id: String,
    ) -> Result<LearnAnnouncementsResultDto, SdkErrorDto> {
        let Some(course) = self
            .learn_course_references
            .get(&course_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let result = self.inner.learn().announcements(&course).await?;
        Ok(learn_announcements_result(result))
    }

    /// Reads all homework buckets for a course selected from this Client's
    /// current catalog. A successful read replaces previous homework handles.
    pub async fn learn_homework(
        &mut self,
        course_reference_id: String,
    ) -> Result<LearnHomeworkListResultDto, SdkErrorDto> {
        let Some(course) = self
            .learn_course_references
            .get(&course_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let result = self.inner.learn().homework(&course).await?;
        Ok(learn_homework_list_result(
            result,
            &mut self.learn_homework_references,
        ))
    }

    /// Reads detail for one assignment from this Client's latest homework list.
    pub async fn learn_homework_detail(
        &mut self,
        homework_reference_id: String,
    ) -> Result<LearnHomeworkDetailResultDto, SdkErrorDto> {
        let Some(homework) = self
            .learn_homework_references
            .get(&homework_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let result = self.inner.learn().homework_detail(&homework).await?;
        Ok(learn_homework_detail_result(result))
    }

    /// Reads the bounded course-file list selected by a current course ref.
    /// Successful results replace the prior file references.
    pub async fn learn_files(
        &mut self,
        course_reference_id: String,
    ) -> Result<LearnCourseFilesResultDto, SdkErrorDto> {
        let Some(course) = self
            .learn_course_references
            .get(&course_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let result = self.inner.learn().files(&course).await?;
        Ok(learn_course_files_result(
            result,
            &mut self.learn_course_file_references,
        ))
    }

    /// Reads verified file-category labels for a current course reference.
    pub async fn learn_file_categories(
        &mut self,
        course_reference_id: String,
    ) -> Result<LearnCourseFileCategoriesResultDto, SdkErrorDto> {
        let Some(course) = self
            .learn_course_references
            .get(&course_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let result = self.inner.learn().file_categories(&course).await?;
        Ok(learn_course_file_categories_result(result))
    }

    /// Reads the live discussion list for a current course reference.
    pub async fn learn_discussions(
        &mut self,
        course_reference_id: String,
    ) -> Result<LearnCourseDiscussionsResultDto, SdkErrorDto> {
        let Some(course) = self
            .learn_course_references
            .get(&course_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let result = self.inner.learn().discussions(&course).await?;
        Ok(learn_course_discussions_result(result))
    }

    /// Saves a file selected from this Client's latest file read to an
    /// explicitly chosen destination. Rust validates the path and refuses
    /// to overwrite an existing file.
    pub async fn learn_save_file(
        &mut self,
        file_reference_id: String,
        destination_path: String,
    ) -> Result<SavedLearnCourseFileDto, SdkErrorDto> {
        let Some(file) = self
            .learn_course_file_references
            .get(&file_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("learn"));
        };
        let saved = self
            .inner
            .learn()
            .save_file(&file, PathBuf::from(destination_path))
            .await?;
        Ok(saved_learn_course_file(saved))
    }

    /// Reads library locations through this Client's shared Runtime.
    pub async fn library_directory(&mut self) -> Result<LibraryDirectoryResultDto, SdkErrorDto> {
        let result = self.inner.library().directory().await?;
        self.library_references.clear();
        self.clear_library_descendants();
        Ok(library_directory_result(
            result,
            &mut self.library_references,
        ))
    }

    /// Reads floors for a location from this Client's latest library directory.
    pub async fn library_floors(
        &mut self,
        library_reference_id: String,
    ) -> Result<LibraryFloorsResultDto, SdkErrorDto> {
        let Some(library) = self.library_references.get(&library_reference_id).cloned() else {
            return Err(context_mismatch("library"));
        };
        let result = self.inner.library().floors(&library).await?;
        self.library_floor_references.clear();
        self.clear_library_section_descendants();
        Ok(library_floors_result(
            result,
            &mut self.library_floor_references,
        ))
    }

    /// Reads sections for today or tomorrow from a current floor reference.
    pub async fn library_sections(
        &mut self,
        floor_reference_id: String,
        day: LibraryDayDto,
    ) -> Result<LibrarySectionsResultDto, SdkErrorDto> {
        let day = match day {
            LibraryDayDto::Today => LibraryDay::Today,
            LibraryDayDto::Tomorrow => LibraryDay::Tomorrow,
            LibraryDayDto::Unknown => return Err(invalid_input("library")),
        };
        let Some(floor) = self
            .library_floor_references
            .get(&floor_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("library"));
        };
        let result = self.inner.library().sections(&floor, day).await?;
        self.library_section_references.clear();
        self.clear_library_window_descendants();
        Ok(library_sections_result(
            result,
            &mut self.library_section_references,
        ))
    }

    /// Reads opening windows for a section from the latest section response.
    pub async fn library_time_windows(
        &mut self,
        section_reference_id: String,
    ) -> Result<LibraryTimeWindowsResultDto, SdkErrorDto> {
        let Some(section) = self
            .library_section_references
            .get(&section_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("library"));
        };
        let result = self.inner.library().time_windows(&section).await?;
        self.library_window_references.clear();
        self.clear_library_availability_descendants();
        Ok(library_time_windows_result(
            result,
            &mut self.library_window_references,
        ))
    }

    /// Reads live seats for one current opening-window reference.
    pub async fn library_seats(
        &mut self,
        window_reference_id: String,
    ) -> Result<LibraryAvailabilityResultDto, SdkErrorDto> {
        let Some(window) = self
            .library_window_references
            .get(&window_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("library"));
        };
        let result = self.inner.library().seats(&window).await?;
        self.library_availability_references.clear();
        self.library_seat_reference_ids.clear();
        Ok(library_availability_result(
            result,
            &mut self.library_availability_references,
            &mut self.library_seat_reference_ids,
        ))
    }

    /// Reads socket states for the seats in a prior availability result.
    pub async fn library_sockets(
        &mut self,
        availability_reference_id: String,
    ) -> Result<LibrarySocketsResultDto, SdkErrorDto> {
        let Some(availability) = self
            .library_availability_references
            .get(&availability_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("library"));
        };
        let result = self.inner.library().sockets(&availability).await?;
        library_sockets_result(result, &self.library_seat_reference_ids)
    }

    /// Reads the classroom building directory and replaces its references.
    pub async fn classroom_buildings(
        &mut self,
    ) -> Result<ClassroomBuildingsResultDto, SdkErrorDto> {
        let result = self.inner.classrooms().buildings().await?;
        Ok(classroom_buildings_result(
            result,
            &mut self.classroom_building_references,
        ))
    }

    /// Reads one building's weekly room matrix. A missing week selects the
    /// source-provided building default; supplied weeks are validated first.
    pub async fn classroom_availability(
        &mut self,
        building_reference_id: String,
        week: Option<u32>,
    ) -> Result<ClassroomAvailabilityResultDto, SdkErrorDto> {
        let selection = match week {
            None => ClassroomWeekSelection::BuildingDefault,
            Some(value) => ClassroomWeek::new(value)
                .map(ClassroomWeekSelection::Week)
                .ok_or_else(|| invalid_input("classrooms"))?,
        };
        let Some(building) = self
            .classroom_building_references
            .get(&building_reference_id)
            .cloned()
        else {
            return Err(context_mismatch("classrooms"));
        };
        let result = self
            .inner
            .classrooms()
            .availability(&building, selection)
            .await?;
        Ok(classroom_availability_result(result))
    }

    /// Returns a one-shot campus-card interaction requested by a prior read.
    pub fn campus_card_pending_interaction(&mut self) -> Option<CampusCardInteractionDto> {
        self.inner
            .campus_card()
            .pending_interaction()
            .map(Into::into)
    }

    /// Reads the current campus-card account through the shared Runtime.
    pub async fn campus_card_account(&mut self) -> Result<CampusCardAccountResultDto, SdkErrorDto> {
        let result = self.inner.campus_card().account().await?;
        Ok(campus_card_account_result(result))
    }

    /// Reads a complete transaction range of at most 31 campus days.
    pub async fn campus_card_transactions(
        &mut self,
        start_date: String,
        end_date: String,
        transaction_type: CampusCardTransactionTypeDto,
    ) -> Result<CampusCardTransactionsResultDto, SdkErrorDto> {
        let transaction_type = transaction_type.try_into()?;
        let range = CampusCardTransactionRange::new(&start_date, &end_date, transaction_type)?;
        let result = self.inner.campus_card().transactions(range).await?;
        Ok(campus_card_transactions_result(result))
    }

    /// Submits the target-specific campus-card password requested by the
    /// current Client. Rust consumes and zeroizes the one-shot input.
    pub async fn campus_card_submit_password(
        &mut self,
        password: String,
    ) -> Result<(), SdkErrorDto> {
        self.inner
            .campus_card()
            .submit_password(CampusCardPasswordRequest::new(password))
            .await?;
        Ok(())
    }

    /// Cancels the pending card password prompt without sending a request.
    pub fn campus_card_cancel_password_challenge(&mut self) -> bool {
        self.inner.campus_card().cancel_password_challenge()
    }

    /// Reads the current dorm-electricity remainder.
    pub async fn electricity_remainder(
        &mut self,
    ) -> Result<ElectricityRemainderResultDto, SdkErrorDto> {
        let result = self.inner.electricity().remainder().await?;
        Ok(electricity_remainder_result(result))
    }

    /// Reads the complete validated dorm-electricity payment history.
    pub async fn electricity_payment_history(
        &mut self,
    ) -> Result<ElectricityPaymentHistoryResultDto, SdkErrorDto> {
        let result = self.inner.electricity().payment_history().await?;
        Ok(electricity_payment_history_result(result))
    }

    /// Reads only the TUNet portal's registration state for the current local
    /// IPv4. This does not identify system-managed Wi-Fi such as Tsinghua Secure.
    pub async fn network_portal_observation(
        &mut self,
    ) -> Result<PortalObservationDto, SdkErrorDto> {
        let observation = self.inner.network().observe_portal_status().await?;
        Ok(PortalObservationDto {
            registration: observation.registration().into(),
            observed_at_utc: observation
                .observed_at()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        })
    }

    /// Explicitly connects a Portal profile. An optional password is a
    /// one-attempt override and is never persisted; absent an override, Rust
    /// uses only a profile password explicitly saved earlier. EAP profiles
    /// are rejected with `unsupported` and remain OS-managed.
    pub async fn network_connect_portal(
        &mut self,
        prepared: &PreparedNetworkProfile,
        password: Option<String>,
    ) -> Result<PortalConnectionResultDto, SdkErrorDto> {
        let result = self
            .inner
            .network()
            .connect_portal_profile(&prepared.inner, password)
            .await?;
        Ok(portal_connection_result(result))
    }

    /// Explicitly disconnects the current process's proven Portal target.
    /// The operation cannot be recreated from a saved profile or an
    /// observation after this Client is disposed.
    pub async fn network_disconnect_portal(
        &mut self,
    ) -> Result<PortalConnectionResultDto, SdkErrorDto> {
        let result = self.inner.network().disconnect_portal().await?;
        Ok(portal_connection_result(result))
    }

    fn clear_library_availability_descendants(&mut self) {
        self.library_availability_references.clear();
        self.library_seat_reference_ids.clear();
    }

    fn clear_library_window_descendants(&mut self) {
        self.library_window_references.clear();
        self.clear_library_availability_descendants();
    }

    fn clear_library_section_descendants(&mut self) {
        self.library_section_references.clear();
        self.clear_library_window_descendants();
    }

    fn clear_library_descendants(&mut self) {
        self.library_floor_references.clear();
        self.clear_library_section_descendants();
    }

    /// Reads the account profile belonging only to the SelfService account.
    pub async fn self_service_account(
        &mut self,
    ) -> Result<SelfServiceAccountResultDto, SdkErrorDto> {
        let result = self.inner.self_service().account().await?;
        Ok(self_service_account_result(result))
    }

    /// Reads the current SelfService device list and creates opaque handles
    /// tied to this ClientHandle and exactly this list snapshot.
    pub async fn self_service_online_devices(
        &mut self,
    ) -> Result<SelfServiceDevicesResultDto, SdkErrorDto> {
        self.self_service_device_references.clear();
        let result = self.inner.self_service().online_devices().await?;
        Ok(self_service_devices_result(
            result,
            &mut self.self_service_device_references,
        ))
    }

    /// Reads SelfService usage and balance values in the units supplied by
    /// the service.
    pub async fn self_service_usage(&mut self) -> Result<SelfServiceUsageResultDto, SdkErrorDto> {
        let result = self.inner.self_service().usage().await?;
        Ok(self_service_usage_result(result))
    }

    /// Disconnects one explicitly selected current SelfService device.
    /// The opaque selection is consumed before dispatch, so ambiguous
    /// one-shot outcomes cannot be submitted again through this handle.
    pub async fn self_service_disconnect_device(
        &mut self,
        reference_id: String,
    ) -> Result<(), SdkErrorDto> {
        let Some(reference) = self.self_service_device_references.remove(&reference_id) else {
            return Err(context_mismatch("self_service"));
        };
        self.self_service_device_references.clear();
        self.inner
            .self_service()
            .disconnect_device(&reference)
            .await?;
        Ok(())
    }

    fn invalidate_auth_bound_references(&mut self) {
        self.service_hall_phase_references.clear();
        self.service_hall_phase_reference_ids.clear();
        self.self_service_device_references.clear();
        self.news_source_references.clear();
        self.news_channel_references.clear();
        self.news_article_references.clear();
        self.news_subscription_references.clear();
        self.learn_course_references.clear();
        self.learn_homework_references.clear();
        self.learn_course_file_references.clear();
        self.library_references.clear();
        self.clear_library_descendants();
        self.classroom_building_references.clear();
    }

    /// Starts one Identity login attempt. The password is consumed by Rust
    /// and is never included in a return value or diagnostic.
    pub async fn login_identity(
        &mut self,
        username: String,
        password: String,
        stage: LoginStageDto,
        trust_device: bool,
        remember_credentials: bool,
    ) -> Result<IdentityLoginResultDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let request = IdentityLoginRequest::new(username, password)
            .stage(stage.into())
            .trust_device(trust_device)
            .remember_credentials(remember_credentials);
        let outcome = self.inner.auth().identity().login(request).await?;
        Ok(map_identity_outcome(&mut self.inner, outcome))
    }

    /// Sends one second-factor code request for the current Identity flow.
    pub async fn send_identity_code(
        &mut self,
        method: SecondFactorMethodDto,
    ) -> Result<IdentityLoginResultDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let outcome = self
            .inner
            .auth()
            .identity()
            .send_code(method.into())
            .await?;
        Ok(map_identity_outcome(&mut self.inner, outcome))
    }

    /// Submits one user-entered Identity code. The SDK does not replay an
    /// ambiguous one-shot submission.
    pub async fn submit_identity_code(
        &mut self,
        method: SecondFactorMethodDto,
        code: String,
    ) -> Result<IdentityLoginResultDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let outcome = self
            .inner
            .auth()
            .identity()
            .submit_code(method.into(), code)
            .await?;
        Ok(map_identity_outcome(&mut self.inner, outcome))
    }

    /// Logs out Identity and its derived services. A selected SelfService
    /// account is retained as expired because it shares Identity's route.
    pub fn logout_identity(&mut self) -> Result<AuthStatusDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        self.inner
            .auth()
            .identity()
            .logout()
            .map(map_auth_status)
            .map_err(Into::into)
    }

    /// Starts the independent SelfService captcha flow.
    pub async fn start_self_service_login(
        &mut self,
        username: String,
        password: String,
        remember_credentials: bool,
    ) -> Result<SelfServiceCaptchaDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let captcha = self
            .inner
            .auth()
            .self_service()
            .start_login(
                SelfServiceLoginRequest::new(username, password)
                    .remember_credentials(remember_credentials),
            )
            .await?;
        Ok(SelfServiceCaptchaDto {
            content_type: captcha.content_type().to_owned(),
            bytes: captcha.bytes().to_vec(),
        })
    }

    /// Starts the SelfService captcha flow using an explicitly requested
    /// stored credential. Passwords remain inside Rust.
    pub async fn start_saved_self_service_login(
        &mut self,
        username: String,
    ) -> Result<SelfServiceCaptchaDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let captcha = self
            .inner
            .auth()
            .self_service()
            .start_saved_login(username)
            .await?;
        Ok(SelfServiceCaptchaDto {
            content_type: captcha.content_type().to_owned(),
            bytes: captcha.bytes().to_vec(),
        })
    }

    /// Forgets one stored SelfService password without changing its session.
    pub fn forget_saved_self_service_credentials(
        &mut self,
        username: String,
    ) -> Result<(), SdkErrorDto> {
        self.inner
            .auth()
            .self_service()
            .forget_saved_credentials(&username)?;
        Ok(())
    }

    /// Explicitly refreshes the current SelfService image challenge.
    pub async fn refresh_self_service_captcha(
        &mut self,
    ) -> Result<SelfServiceCaptchaDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let captcha = self.inner.auth().self_service().refresh_captcha().await?;
        Ok(SelfServiceCaptchaDto {
            content_type: captcha.content_type().to_owned(),
            bytes: captcha.bytes().to_vec(),
        })
    }

    /// Submits a human-entered captcha and optional SMS code once.
    pub async fn submit_self_service_captcha(
        &mut self,
        answer: String,
        sms_code: Option<String>,
    ) -> Result<SelfServiceLoginResultDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        let outcome = self
            .inner
            .auth()
            .self_service()
            .submit_captcha(answer, sms_code)
            .await?;
        let credentials_saved = matches!(
            &outcome,
            SelfServiceLoginOutcome::Authenticated {
                credentials_saved: true,
                ..
            }
        );
        Ok(SelfServiceLoginResultDto {
            status: map_auth_status(self.inner.auth().status()),
            requires_interaction: matches!(outcome, SelfServiceLoginOutcome::NeedsInteraction),
            credentials_saved,
        })
    }

    /// Returns the current local captcha phase without making a request.
    pub fn self_service_login_phase(&mut self) -> SelfServiceLoginPhaseDto {
        self.inner.auth().self_service().login_phase().into()
    }

    /// Cancels only the pending SelfService login interaction.
    pub fn cancel_self_service_login(&mut self) {
        self.invalidate_auth_bound_references();
        self.inner.auth().self_service().cancel_login();
    }

    /// Logs out SelfService without changing Identity or local network data.
    pub fn logout_self_service(&mut self) -> AuthStatusDto {
        self.invalidate_auth_bound_references();
        map_auth_status(self.inner.auth().self_service().logout())
    }

    /// Explicitly logs out both account domains. Local connection profiles
    /// are not affected.
    pub fn logout_all(&mut self) -> Result<AuthStatusDto, SdkErrorDto> {
        self.invalidate_auth_bound_references();
        self.inner
            .auth()
            .logout_all()
            .map(map_auth_status)
            .map_err(Into::into)
    }

    /// Lists this Client's local network profiles without revealing secrets.
    pub fn network_profiles(&mut self) -> Vec<NetworkProfileDto> {
        self.inner
            .network()
            .profiles()
            .list()
            .iter()
            .map(NetworkProfileDto::from)
            .collect()
    }

    /// Saves a local profile. Saving never connects to a network or changes
    /// either Auth domain. Password storage occurs only when `password` is
    /// explicitly supplied.
    pub fn save_network_profile(
        &mut self,
        label: String,
        username: String,
        method: NetworkAccessMethodDto,
        password: Option<String>,
    ) -> Result<NetworkProfileDto, SdkErrorDto> {
        let mut input = NetworkProfileInput::new(label, username, access_method(method)?)?;
        if let Some(password) = password {
            input = input.save_password(password)?;
        }
        let summary = self.inner.network().profiles().save(input)?;
        Ok(NetworkProfileDto::from(&summary))
    }

    /// Updates one local profile and invalidates earlier prepared references.
    pub fn update_network_profile(
        &mut self,
        id: String,
        label: String,
        username: String,
        method: NetworkAccessMethodDto,
        password: Option<String>,
    ) -> Result<NetworkProfileDto, SdkErrorDto> {
        let id = NetworkProfileId::parse(&id).map_err(|_| invalid_input("network"))?;
        let mut input = NetworkProfileInput::new(label, username, access_method(method)?)?;
        if let Some(password) = password {
            input = input.save_password(password)?;
        }
        let summary = self.inner.network().profiles().update(id, input)?;
        Ok(NetworkProfileDto::from(&summary))
    }

    /// Deletes a local profile without changing Auth or network state.
    pub fn delete_network_profile(&mut self, id: String) -> Result<bool, SdkErrorDto> {
        let id = NetworkProfileId::parse(&id).map_err(|_| invalid_input("network"))?;
        Ok(self.inner.network().profiles().delete(id)?)
    }

    /// Prepares non-secret form fields and a Client/version-bound handle.
    pub fn prepare_network_profile_fill(
        &mut self,
        id: String,
    ) -> Result<PreparedNetworkProfile, SdkErrorDto> {
        let id = NetworkProfileId::parse(&id).map_err(|_| invalid_input("network"))?;
        let prepared = self.inner.network().profiles().prepare_fill(id)?;
        Ok(PreparedNetworkProfile { inner: prepared })
    }

    /// Reports whether a prepared form still refers to the current profile.
    pub fn is_current_network_profile_fill(&mut self, prepared: &PreparedNetworkProfile) -> bool {
        self.inner.network().profiles().is_current(&prepared.inner)
    }

    /// Returns an opaque password handle only after an explicit caller action.
    pub fn network_profile_password_for_fill(
        &mut self,
        prepared: &PreparedNetworkProfile,
    ) -> Result<Option<NetworkProfilePasswordHandle>, SdkErrorDto> {
        self.inner
            .network()
            .profiles()
            .password_for_fill(&prepared.inner)
            .map(|password| password.map(|inner| NetworkProfilePasswordHandle { inner }))
            .map_err(Into::into)
    }
}

fn map_account_status(value: &AccountAuthStatus) -> AccountStatusDto {
    let state = match value.state() {
        AccountAuthState::SignedOut => AccountStateDto::SignedOut,
        AccountAuthState::RestoredUnverified => AccountStateDto::RestoredUnverified,
        AccountAuthState::Authenticated => AccountStateDto::Authenticated,
        AccountAuthState::NeedsInteraction => AccountStateDto::NeedsInteraction,
        AccountAuthState::Authenticating => AccountStateDto::Authenticating,
        AccountAuthState::Expired => AccountStateDto::Expired,
        _ => AccountStateDto::Unknown,
    };
    AccountStatusDto {
        state,
        username: value.username().map(str::to_owned),
    }
}

fn map_auth_status(value: AuthStatus) -> AuthStatusDto {
    AuthStatusDto {
        identity: map_account_status(value.identity()),
        self_service: map_account_status(value.self_service()),
    }
}

fn map_identity_outcome(
    client: &mut SdkClient,
    value: IdentityLoginOutcome,
) -> IdentityLoginResultDto {
    let (requires_interaction, methods, masked_phone) = match value {
        IdentityLoginOutcome::Authenticated(_) => (false, Vec::new(), None),
        IdentityLoginOutcome::NeedsInteraction {
            methods,
            masked_phone,
        } => (
            true,
            methods
                .into_iter()
                .map(|method| method.as_str().to_owned())
                .collect(),
            masked_phone,
        ),
        _ => (true, vec!["unknown".to_owned()], None),
    };
    IdentityLoginResultDto {
        status: map_auth_status(client.auth().status()),
        requires_interaction,
        methods,
        masked_phone,
    }
}

fn invalid_input(service: &str) -> SdkErrorDto {
    SdkErrorDto {
        service: service.to_owned(),
        code: "invalid_input".to_owned(),
        retry_after_ms: None,
        diagnostic_id: uuid::Uuid::new_v4().to_string(),
    }
}

fn context_mismatch(service: &str) -> SdkErrorDto {
    SdkErrorDto {
        service: service.to_owned(),
        code: "context_mismatch".to_owned(),
        retry_after_ms: None,
        diagnostic_id: uuid::Uuid::new_v4().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsinghua_kit_sdk::{
        auth::AuthDomain,
        error::ErrorCode,
        read::{
            CacheFreshness, IncompleteReason, ReadCoverage, ReadMetadata, ReadResult, ReadSource,
        },
        service_hall::{PendingTasks, Task},
    };

    fn client() -> ClientHandle {
        ClientHandle::new(None, None, None, None, None, None, None).expect("memory-only client")
    }

    #[tokio::test]
    async fn overview_bridge_validates_canonical_day_and_cache_miss() {
        let mut client = client();
        let invalid = client
            .overview_day("2026-9-26".to_owned(), ReadPolicyDto::CacheOnly)
            .await
            .unwrap_err();
        assert_eq!(invalid.service, "overview");
        assert_eq!(invalid.code, "invalid_input");

        let miss = client
            .overview_day("2026-09-26".to_owned(), ReadPolicyDto::CacheOnly)
            .await
            .unwrap_err();
        assert_eq!(miss.service, "overview");
        assert_eq!(miss.code, "cache_miss");
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
    }

    #[test]
    fn account_status_keeps_two_names_separate_and_redacts_debug() {
        let status = AuthStatus::new(
            AccountAuthStatus::new(
                AuthDomain::Identity,
                Some("identity-account-private".to_owned()),
                AccountAuthState::Authenticated,
            ),
            AccountAuthStatus::new(
                AuthDomain::SelfService,
                Some("selfservice-account-private".to_owned()),
                AccountAuthState::SignedOut,
            ),
        );
        let dto = map_auth_status(status);

        assert_eq!(
            dto.identity.username.as_deref(),
            Some("identity-account-private")
        );
        assert_eq!(
            dto.self_service.username.as_deref(),
            Some("selfservice-account-private")
        );
        let debug = format!("{dto:?}");
        assert!(!debug.contains("identity-account-private"));
        assert!(!debug.contains("selfservice-account-private"));
        assert!(debug.contains("account_selected: true"));
    }

    #[test]
    fn bridge_client_keeps_auth_slots_independent_and_starts_signed_out() {
        let mut client = client();
        let status = client.auth_status();
        assert_eq!(status.identity.state, AccountStateDto::SignedOut);
        assert_eq!(status.self_service.state, AccountStateDto::SignedOut);
    }

    #[tokio::test]
    async fn bridge_identity_credential_opt_in_requires_configured_storage() {
        let mut client = client();
        let error = client
            .login_identity(
                "fixture-identity".to_owned(),
                "synthetic-password".to_owned(),
                LoginStageDto::Auto,
                false,
                true,
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, "storage_unavailable");
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
        let debug = format!("{error:?}");
        assert!(!debug.contains("fixture-identity"));
        assert!(!debug.contains("synthetic-password"));
    }

    #[tokio::test]
    async fn bridge_self_service_credential_opt_in_requires_separate_storage() {
        let mut client = client();
        let error = client
            .start_self_service_login(
                "fixture-self-service".to_owned(),
                "synthetic-password".to_owned(),
                true,
            )
            .await
            .expect_err("SelfService credentials require a separate store");

        assert_eq!(error.service, "self_service_auth");
        assert_eq!(error.code, "storage_unavailable");
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
        assert_eq!(
            client.auth_status().self_service.state,
            AccountStateDto::SignedOut
        );
        let debug = format!("{error:?}");
        assert!(!debug.contains("fixture-self-service"));
        assert!(!debug.contains("synthetic-password"));
    }

    #[test]
    fn bridge_auth_credential_storage_is_independent_from_session_storage() {
        let root = std::env::temp_dir().join(format!(
            "tsinghua-kit-auth-credentials-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let directory = root
            .join("TsinghuaKit")
            .join("auth-credentials")
            .join("org.example.credentials-test");
        let mut client = ClientHandle::new(
            None,
            Some(root.to_string_lossy().into_owned()),
            Some("org.example.credentials-test".to_owned()),
            None,
            None,
            None,
            None,
        )
        .expect("Auth credential storage can be configured without session snapshots");
        assert!(directory.is_dir());
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
        assert_eq!(
            client.auth_status().self_service.state,
            AccountStateDto::SignedOut
        );
        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn bridge_identity_session_storage_is_opt_in_and_revalidation_is_explicit() {
        let root = std::env::temp_dir().join(format!(
            "tsinghua-kit-identity-session-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut client = ClientHandle::new(
            None,
            None,
            None,
            None,
            None,
            Some(root.to_string_lossy().into_owned()),
            Some("org.example.identity-session-test".to_owned()),
        )
        .expect("explicitly configured private session directory");

        let status = client.auth_status();
        assert_eq!(status.identity.state, AccountStateDto::SignedOut);
        assert_eq!(status.self_service.state, AccountStateDto::SignedOut);
        let reconciled = client
            .identity_revalidate_restored_session()
            .await
            .expect("a fresh session revalidation is a local no-op");
        assert_eq!(reconciled.identity.state, AccountStateDto::SignedOut);
        assert_eq!(reconciled.self_service.state, AccountStateDto::SignedOut);

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bridge_business_cache_and_identity_session_use_independent_directories() {
        let root = std::env::temp_dir().join(format!(
            "tsinghua-kit-independent-storage-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let cache_root = root.join("service-cache");
        let identity_root = root.join("identity-state");
        let identity_session_directory = identity_root
            .join("TsinghuaKit")
            .join("identity-session")
            .join("org.example.independent-storage-test");
        let mut client = ClientHandle::new(
            Some(cache_root.to_string_lossy().into_owned()),
            None,
            None,
            None,
            None,
            Some(identity_root.to_string_lossy().into_owned()),
            Some("org.example.independent-storage-test".to_owned()),
        )
        .expect("service cache and Identity snapshot may use separate roots");

        assert!(cache_root.is_dir());
        assert!(identity_session_directory.is_dir());
        assert_ne!(
            std::fs::canonicalize(&cache_root).unwrap(),
            std::fs::canonicalize(&identity_session_directory).unwrap(),
        );
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
        assert_eq!(
            client.auth_status().self_service.state,
            AccountStateDto::SignedOut
        );

        drop(client);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bridge_profile_fill_is_explicit_redacted_and_version_bound() {
        let mut client = client();
        let profile = client
            .save_network_profile(
                "Tsinghua Secure".to_owned(),
                "local-account".to_owned(),
                NetworkAccessMethodDto::SystemWifiEap,
                Some("synthetic-network-secret".to_owned()),
            )
            .unwrap();
        assert!(
            format!("{profile:?}")
                .find("synthetic-network-secret")
                .is_none()
        );
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
        assert_eq!(
            client.auth_status().self_service.state,
            AccountStateDto::SignedOut
        );

        let prepared = client
            .prepare_network_profile_fill(profile.id.clone())
            .unwrap();
        assert!(prepared.form_fields().has_saved_password);
        assert!(client.is_current_network_profile_fill(&prepared));
        let password = client
            .network_profile_password_for_fill(&prepared)
            .unwrap()
            .unwrap();
        assert_eq!(password.expose_for_form(), "synthetic-network-secret");
        assert!(
            format!("{password:?}")
                .find("synthetic-network-secret")
                .is_none()
        );

        client
            .update_network_profile(
                profile.id.clone(),
                "Tsinghua Secure".to_owned(),
                "local-account".to_owned(),
                NetworkAccessMethodDto::SystemWifiEap,
                None,
            )
            .unwrap();
        assert!(!client.is_current_network_profile_fill(&prepared));
        assert_eq!(
            client
                .network_profile_password_for_fill(&prepared)
                .unwrap_err()
                .code,
            "context_mismatch"
        );
    }

    #[tokio::test]
    async fn bridge_portal_operations_reject_eap_and_unproven_disconnects() {
        let mut client = client();
        let portal = client
            .save_network_profile(
                "Portal".to_owned(),
                "portal-user".to_owned(),
                NetworkAccessMethodDto::Portal,
                None,
            )
            .unwrap();
        let eap = client
            .save_network_profile(
                "Tsinghua Secure".to_owned(),
                "eap-user".to_owned(),
                NetworkAccessMethodDto::SystemWifiEap,
                Some("eap-only-secret".to_owned()),
            )
            .unwrap();
        let portal_fill = client
            .prepare_network_profile_fill(portal.id.clone())
            .unwrap();
        let eap_fill = client.prepare_network_profile_fill(eap.id.clone()).unwrap();

        let missing_portal_password = client
            .network_connect_portal(&portal_fill, None)
            .await
            .unwrap_err();
        assert_eq!(missing_portal_password.service, "network");
        assert_eq!(missing_portal_password.code, "interaction_required");

        let eap_rejection = client
            .network_connect_portal(&eap_fill, Some("eap-only-secret".to_owned()))
            .await
            .unwrap_err();
        assert_eq!(eap_rejection.service, "network");
        assert_eq!(eap_rejection.code, "unsupported");

        client
            .update_network_profile(
                eap.id,
                "Portal after edit".to_owned(),
                "changed-user".to_owned(),
                NetworkAccessMethodDto::Portal,
                None,
            )
            .unwrap();
        let stale_profile_rejection = client
            .network_connect_portal(&eap_fill, Some("one-shot-only".to_owned()))
            .await
            .unwrap_err();
        assert_eq!(stale_profile_rejection.service, "network");
        assert_eq!(stale_profile_rejection.code, "context_mismatch");

        let disconnect_rejection = client.network_disconnect_portal().await.unwrap_err();
        assert_eq!(disconnect_rejection.service, "network");
        assert_eq!(disconnect_rejection.code, "context_mismatch");
        let status = client.auth_status();
        assert_eq!(status.identity.state, AccountStateDto::SignedOut);
        assert_eq!(status.self_service.state, AccountStateDto::SignedOut);
    }

    #[test]
    fn unmatched_storage_configuration_is_rejected_before_client_creation() {
        let error = match ClientHandle::new(
            None,
            None,
            None,
            Some("/tmp/profiles".to_owned()),
            None,
            None,
            None,
        ) {
            Ok(_) => panic!("mismatched storage options must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.service, "network");
        assert_eq!(error.code, "invalid_input");
        assert!(!error.diagnostic_id.is_empty());
    }

    #[test]
    fn service_hall_pending_mapping_preserves_coverage_and_redacts_content() {
        let observed_at = chrono::Utc::now();
        let result = ReadResult::new(
            PendingTasks::from_verified(
                vec![Task::from_verified(
                    "private-protocol-selector".to_owned(),
                    "synthetic confidential task title".to_owned(),
                    "synthetic status".to_owned(),
                    "synthetic step".to_owned(),
                    "synthetic application time".to_owned(),
                    Some(75),
                )],
                3,
                ReadCoverage::Partial(IncompleteReason::ReadLimitReached),
            ),
            ReadMetadata::new(
                ReadSource::MemoryCache,
                CacheFreshness::Stale,
                observed_at,
                ReadCoverage::Partial(IncompleteReason::ReadLimitReached),
                Some(ErrorCode::NetworkUnavailable),
            ),
        );

        let dto = service_hall_pending_result(result);

        assert_eq!(dto.data.tasks.len(), 1);
        assert_eq!(dto.data.reported_pending_count, 3);
        assert_eq!(dto.data.coverage, ReadCoverageDto::PartialReadLimitReached);
        assert_eq!(dto.data.tasks[0].progress_percent, Some(75));
        assert_eq!(dto.metadata.source, ReadSourceDto::MemoryCache);
        assert_eq!(dto.metadata.freshness, CacheFreshnessDto::Stale);
        assert_eq!(
            dto.metadata.coverage,
            ReadCoverageDto::PartialReadLimitReached
        );
        assert_eq!(
            dto.metadata.refresh_failure_code.as_deref(),
            Some("network_unavailable")
        );
        assert_eq!(
            dto.metadata.observed_at_utc,
            observed_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
        let debug = format!("{dto:?}");
        for private_value in [
            "private-protocol-selector",
            "synthetic confidential task title",
            "synthetic status",
            "synthetic step",
            "synthetic application time",
        ] {
            assert!(!debug.contains(private_value));
        }
    }

    #[tokio::test]
    async fn service_hall_pending_bridge_requires_identity_before_reading() {
        let mut client = client();

        let error = client
            .service_hall_pending(ServiceHallReadPolicyDto::Refresh)
            .await
            .expect_err("service-hall reads require an Identity session");

        assert_eq!(error.service, "service_hall");
        assert_eq!(error.code, "session_required");
        assert!(!error.diagnostic_id.is_empty());
    }

    #[tokio::test]
    async fn service_hall_directory_and_task_views_keep_identity_gate() {
        let mut client = client();
        let directory_error = client
            .service_hall_services(ServiceHallReadPolicyDto::Refresh)
            .await
            .unwrap_err();
        assert_eq!(directory_error.service, "service_hall");
        assert_eq!(directory_error.code, "session_required");

        let task_error = client
            .service_hall_tasks(
                ServiceHallTaskViewDto::Phases,
                ServiceHallReadPolicyDto::Refresh,
            )
            .await
            .unwrap_err();
        assert_eq!(task_error.service, "service_hall");
        assert_eq!(task_error.code, "session_required");
    }

    #[tokio::test]
    async fn service_hall_phase_details_reject_unknown_or_cross_handle_tokens() {
        let mut client = client();
        let error = client
            .service_hall_phase_details(
                uuid::Uuid::new_v4().to_string(),
                ServiceHallReadPolicyDto::CacheOnly,
            )
            .await
            .unwrap_err();
        assert_eq!(error.service, "service_hall");
        assert_eq!(error.code, "context_mismatch");
    }

    #[tokio::test]
    async fn service_hall_unknown_task_view_is_rejected_before_reading() {
        let mut client = client();
        let error = client
            .service_hall_tasks(
                ServiceHallTaskViewDto::Unknown,
                ServiceHallReadPolicyDto::Refresh,
            )
            .await
            .unwrap_err();
        assert_eq!(error.service, "service_hall");
        assert_eq!(error.code, "invalid_input");
    }

    #[tokio::test]
    async fn self_service_reads_require_the_separate_self_service_account() {
        let mut client = client();
        let account_error = client.self_service_account().await.unwrap_err();
        assert_eq!(account_error.code, "session_required");

        let devices_error = client.self_service_online_devices().await.unwrap_err();
        assert_eq!(devices_error.code, "session_required");

        let usage_error = client.self_service_usage().await.unwrap_err();
        assert_eq!(usage_error.code, "session_required");
        assert_eq!(
            client.auth_status().identity.state,
            AccountStateDto::SignedOut
        );
        assert_eq!(
            client.auth_status().self_service.state,
            AccountStateDto::SignedOut
        );
    }

    #[tokio::test]
    async fn self_service_disconnect_rejects_unknown_and_cross_handle_device_ids() {
        let mut client = client();
        let error = client
            .self_service_disconnect_device(uuid::Uuid::new_v4().to_string())
            .await
            .unwrap_err();
        assert_eq!(error.service, "self_service");
        assert_eq!(error.code, "context_mismatch");
    }

    #[test]
    fn self_service_bridge_debug_omits_account_device_and_balance_values() {
        let account = SelfServiceAccountDto {
            contact_email: "private-email".into(),
            contact_phone: "private-phone".into(),
            contact_landline: "private-landline".into(),
            real_name: "private-name".into(),
            status: "private-status".into(),
            user_group: "private-group".into(),
            location: "private-location".into(),
            allowed_devices: 1,
        };
        let device = SelfServiceDeviceDto {
            reference_id: "private-reference".into(),
            ipv4: "private-ipv4".into(),
            ipv6: "private-ipv6".into(),
            logged_at: "private-login-time".into(),
            authorization: "private-authorization".into(),
            mac_suffix: "private-mac".into(),
        };
        let usage = SelfServiceUsageDto {
            product_name: "private-product".into(),
            used_bytes: "private-usage".into(),
            used_seconds: "private-seconds".into(),
            account_balance: "private-balance".into(),
            settlement_date: "private-date".into(),
        };
        let debug = format!("{account:?} {device:?} {usage:?}");
        for private_value in [
            "private-email",
            "private-phone",
            "private-landline",
            "private-name",
            "private-location",
            "private-reference",
            "private-ipv4",
            "private-login-time",
            "private-authorization",
            "private-mac",
            "private-usage",
            "private-balance",
        ] {
            assert!(!debug.contains(private_value));
        }
    }

    #[tokio::test]
    async fn registrar_bridge_requires_identity_before_each_read() {
        let mut client = client();

        let schedule = client.registrar_semester_schedule().await.unwrap_err();
        assert_eq!(schedule.service, "registrar");
        assert_eq!(schedule.code, "session_required");

        let grades = client.registrar_grades().await.unwrap_err();
        assert_eq!(grades.service, "registrar");
        assert_eq!(grades.code, "session_required");

        let exams = client.registrar_exams().await.unwrap_err();
        assert_eq!(exams.service, "registrar");
        assert_eq!(exams.code, "session_required");
    }

    #[tokio::test]
    async fn learn_term_calendar_requires_identity_before_reading() {
        let mut client = client();

        let error = client.calendar_learn_terms().await.unwrap_err();

        assert_eq!(error.service, "learn");
        assert_eq!(error.code, "session_required");
    }

    #[tokio::test]
    async fn school_calendar_rejects_unknown_selectors_and_out_of_range_years() {
        let mut client = client();

        let semester_error = client
            .calendar_school_image(
                None,
                SchoolCalendarSemesterDto::Unknown,
                SchoolCalendarLanguageDto::Chinese,
            )
            .await
            .unwrap_err();
        assert_eq!(semester_error.service, "calendar");
        assert_eq!(semester_error.code, "invalid_input");

        let language_error = client
            .calendar_school_image(
                None,
                SchoolCalendarSemesterDto::Spring,
                SchoolCalendarLanguageDto::Unknown,
            )
            .await
            .unwrap_err();
        assert_eq!(language_error.service, "calendar");
        assert_eq!(language_error.code, "invalid_input");

        let year_error = client
            .calendar_school_image(
                Some(1999),
                SchoolCalendarSemesterDto::Spring,
                SchoolCalendarLanguageDto::Chinese,
            )
            .await
            .unwrap_err();
        assert_eq!(year_error.service, "calendar");
        assert_eq!(year_error.code, "invalid_input");
    }

    #[tokio::test]
    async fn news_account_reads_require_identity_and_invalid_queries_fail_early() {
        let mut client = client();

        let catalog_error = client.news_catalog().await.unwrap_err();
        assert_eq!(catalog_error.service, "news");
        assert_eq!(catalog_error.code, "session_required");

        let list_error = client
            .news_articles(1, 20, None, None, ReadPolicyDto::Refresh)
            .await
            .unwrap_err();
        assert_eq!(list_error.service, "news");
        assert_eq!(list_error.code, "session_required");

        let favorites_error = client.news_favorites().await.unwrap_err();
        assert_eq!(favorites_error.service, "news");
        assert_eq!(favorites_error.code, "session_required");

        let subscriptions_error = client.news_subscriptions().await.unwrap_err();
        assert_eq!(subscriptions_error.service, "news");
        assert_eq!(subscriptions_error.code, "session_required");

        let invalid_page = client
            .news_articles(0, 20, None, None, ReadPolicyDto::Refresh)
            .await
            .unwrap_err();
        assert_eq!(invalid_page.service, "news");
        assert_eq!(invalid_page.code, "invalid_input");

        let invalid_search = client
            .news_search(1, "  \n".to_owned(), None, false, ReadPolicyDto::Refresh)
            .await
            .unwrap_err();
        assert_eq!(invalid_search.service, "news");
        assert_eq!(invalid_search.code, "invalid_input");
    }

    #[tokio::test]
    async fn news_article_and_subscription_selectors_reject_unknown_handles() {
        let mut client = client();

        let article_error = client
            .news_article(uuid::Uuid::new_v4().to_string(), ReadPolicyDto::CacheOnly)
            .await
            .unwrap_err();
        assert_eq!(article_error.service, "news");
        assert_eq!(article_error.code, "context_mismatch");

        let subscription_error = client
            .news_subscription_articles(uuid::Uuid::new_v4().to_string(), 1)
            .await
            .unwrap_err();
        assert_eq!(subscription_error.service, "news");
        assert_eq!(subscription_error.code, "context_mismatch");
    }

    #[test]
    fn news_bridge_debug_hides_references_keywords_and_article_content() {
        let source = NewsSourceDto {
            reference_id: "private-source-selector".into(),
            name: "Campus News".into(),
        };
        let article = NewsArticleDto {
            reference_id: "private-article-selector".into(),
            title: "Article title".into(),
            published_at: "today".into(),
            source: "Campus News".into(),
            topped: false,
            channel: "Campus".into(),
            favorited: false,
        };
        let subscription = NewsSubscriptionDto {
            reference_id: "private-subscription-selector".into(),
            title: "Saved search".into(),
            order: 1,
            sources: vec!["Campus News".into()],
            channels: vec!["Campus".into()],
            keyword: "private-keyword".into(),
        };
        let detail = ArticleDetailDto {
            title: "Article title".into(),
            content_html: "private article html".into(),
            summary: "private article summary".into(),
            attachments: vec![],
        };
        let debug = format!("{source:?} {article:?} {subscription:?} {detail:?}");
        for private_value in [
            "private-source-selector",
            "private-article-selector",
            "private-subscription-selector",
            "private-keyword",
            "private article html",
            "private article summary",
        ] {
            assert!(!debug.contains(private_value));
        }
    }

    #[tokio::test]
    async fn learn_course_reads_require_identity_and_reject_unknown_references() {
        let mut client = client();
        let catalog_error = client.learn_courses().await.unwrap_err();
        assert_eq!(catalog_error.service, "learn");
        assert_eq!(catalog_error.code, "session_required");

        let unknown_reference = uuid::Uuid::new_v4().to_string();
        let announcements = client
            .learn_announcements(unknown_reference.clone())
            .await
            .unwrap_err();
        assert_eq!(announcements.service, "learn");
        assert_eq!(announcements.code, "context_mismatch");

        let homework = client
            .learn_homework(unknown_reference.clone())
            .await
            .unwrap_err();
        assert_eq!(homework.code, "context_mismatch");

        let files = client
            .learn_files(unknown_reference.clone())
            .await
            .unwrap_err();
        assert_eq!(files.code, "context_mismatch");

        let categories = client
            .learn_file_categories(unknown_reference.clone())
            .await
            .unwrap_err();
        assert_eq!(categories.code, "context_mismatch");

        let discussions = client
            .learn_discussions(unknown_reference)
            .await
            .unwrap_err();
        assert_eq!(discussions.code, "context_mismatch");
    }

    #[tokio::test]
    async fn learn_detail_and_file_save_reject_unknown_handles_before_io() {
        let mut client = client();
        let homework_error = client
            .learn_homework_detail(uuid::Uuid::new_v4().to_string())
            .await
            .unwrap_err();
        assert_eq!(homework_error.service, "learn");
        assert_eq!(homework_error.code, "context_mismatch");

        let save_error = client
            .learn_save_file(
                uuid::Uuid::new_v4().to_string(),
                "/tmp/tsinghua-kit-unused-file".to_owned(),
            )
            .await
            .unwrap_err();
        assert_eq!(save_error.service, "learn");
        assert_eq!(save_error.code, "context_mismatch");
    }

    #[tokio::test]
    async fn library_reads_require_identity_and_reject_unknown_or_invalid_selectors() {
        let mut client = client();
        let directory = client.library_directory().await.unwrap_err();
        assert_eq!(directory.service, "library");
        assert_eq!(directory.code, "session_required");

        let unknown = uuid::Uuid::new_v4().to_string();
        let floors = client.library_floors(unknown.clone()).await.unwrap_err();
        assert_eq!(floors.service, "library");
        assert_eq!(floors.code, "context_mismatch");

        let invalid_day = client
            .library_sections(unknown.clone(), LibraryDayDto::Unknown)
            .await
            .unwrap_err();
        assert_eq!(invalid_day.service, "library");
        assert_eq!(invalid_day.code, "invalid_input");

        for error in [
            client
                .library_time_windows(unknown.clone())
                .await
                .unwrap_err(),
            client.library_seats(unknown.clone()).await.unwrap_err(),
            client.library_sockets(unknown).await.unwrap_err(),
        ] {
            assert_eq!(error.service, "library");
            assert_eq!(error.code, "context_mismatch");
        }
    }

    #[tokio::test]
    async fn classroom_reads_validate_week_before_io_and_bind_building_refs() {
        let mut client = client();
        let buildings = client.classroom_buildings().await.unwrap_err();
        assert_eq!(buildings.service, "classrooms");
        assert_eq!(buildings.code, "session_required");

        let invalid_week = client
            .classroom_availability(uuid::Uuid::new_v4().to_string(), Some(0))
            .await
            .unwrap_err();
        assert_eq!(invalid_week.service, "classrooms");
        assert_eq!(invalid_week.code, "invalid_input");

        let unknown_building = client
            .classroom_availability(uuid::Uuid::new_v4().to_string(), None)
            .await
            .unwrap_err();
        assert_eq!(unknown_building.service, "classrooms");
        assert_eq!(unknown_building.code, "context_mismatch");
    }

    #[test]
    fn self_service_login_phase_is_fixed_local_state() {
        let mut client = client();
        assert_eq!(
            client.self_service_login_phase(),
            SelfServiceLoginPhaseDto::RestartRequired
        );
    }

    #[test]
    fn identity_interaction_is_empty_for_a_fresh_client() {
        let mut client = client();
        assert!(client.identity_interaction().unwrap().is_none());
    }

    #[test]
    fn identity_login_stage_suggestion_is_local_and_nullable() {
        assert_eq!(
            suggest_identity_login_stage("2026313469".to_owned()),
            Some(LoginStageDto::Graduate)
        );
        assert_eq!(
            suggest_identity_login_stage("2026113469".to_owned()),
            Some(LoginStageDto::Undergraduate)
        );
        assert_eq!(
            suggest_identity_login_stage("not-a-student-id".to_owned()),
            None
        );
    }

    #[tokio::test]
    async fn campus_card_and_electricity_reads_keep_auth_and_range_errors_typed() {
        let mut client = client();
        assert_eq!(client.campus_card_pending_interaction(), None);

        let account = client.campus_card_account().await.unwrap_err();
        assert_eq!(account.service, "campus_card");
        assert_eq!(account.code, "session_required");

        let invalid_range = client
            .campus_card_transactions(
                "2026-09-01".to_owned(),
                "2026-10-02".to_owned(),
                CampusCardTransactionTypeDto::Any,
            )
            .await
            .unwrap_err();
        assert_eq!(invalid_range.service, "campus_card");
        assert_eq!(invalid_range.code, "invalid_input");

        let invalid_type = client
            .campus_card_transactions(
                "2026-09-01".to_owned(),
                "2026-09-02".to_owned(),
                CampusCardTransactionTypeDto::Unknown,
            )
            .await
            .unwrap_err();
        assert_eq!(invalid_type.service, "campus_card");
        assert_eq!(invalid_type.code, "invalid_input");

        let remainder = client.electricity_remainder().await.unwrap_err();
        assert_eq!(remainder.service, "electricity");
        assert_eq!(remainder.code, "session_required");

        let history = client.electricity_payment_history().await.unwrap_err();
        assert_eq!(history.service, "electricity");
        assert_eq!(history.code, "session_required");
    }

    #[test]
    fn campus_card_and_electricity_debug_hide_personal_data_and_amounts() {
        let account = CampusCardAccountDto {
            display_name: "private-card-holder".into(),
            display_name_latin: None,
            department_name: "private-department".into(),
            department_name_latin: None,
            department_id: 1234,
            gender: Some("private-gender".into()),
            effective_at: "private-effective-date".into(),
            valid_until: "private-validity-date".into(),
            balance_cents: 12345,
            card_status: "private-card-status".into(),
            last_transaction_at: "private-card-time".into(),
            daily_limit_cents: 50000,
            one_time_limit_cents: 20000,
        };
        let transaction = CampusCardTransactionDto {
            summary: "private-card-transaction".into(),
            occurred_at: "private-card-transaction-time".into(),
            post_balance_cents: 5000,
            amount_cents: -100,
            merchant_address: "private-merchant-address".into(),
            merchant_name: Some("private-merchant".into()),
            transaction_name: "private-transaction-kind".into(),
        };
        let remainder = ElectricityRemainderDto {
            value: 42.0,
            update_time: "private-electricity-time".into(),
        };
        let debug = format!("{account:?} {transaction:?} {remainder:?}");
        for private_value in [
            "private-card-holder",
            "private-department",
            "private-gender",
            "private-effective-date",
            "private-validity-date",
            "private-card-status",
            "private-card-transaction",
            "private-card-transaction-time",
            "private-merchant-address",
            "private-merchant",
            "private-transaction-kind",
            "private-electricity-time",
        ] {
            assert!(!debug.contains(private_value));
        }
        assert!(!debug.contains("12345"));
        assert!(!debug.contains("42.0"));
    }

    #[test]
    fn library_and_classroom_bridge_debug_hide_selectors_and_unknown_labels() {
        let library = LibraryPlaceDto {
            reference_id: Some("private-library-selector".into()),
            name: "Main Library".into(),
            english_name: None,
            is_valid: Some(true),
            total_seats: Some(20_u64),
            available_seats: Some(3_u64),
        };
        let seat = LibrarySeatDto {
            reference_id: "private-seat-selector".into(),
            name: "A-101".into(),
            is_available: true,
        };
        let classroom_slot = ClassroomSlotDto {
            status: ClassroomSlotStatusDto::Unknown,
            unknown_class_name: Some("private-source-class".into()),
        };
        let debug = format!("{library:?} {seat:?} {classroom_slot:?}");
        for private_value in [
            "private-library-selector",
            "private-seat-selector",
            "private-source-class",
        ] {
            assert!(!debug.contains(private_value));
        }
    }

    #[test]
    fn learn_bridge_debug_hides_content_and_opaque_references() {
        let announcement = LearnAnnouncementDto {
            title: "private-title".into(),
            publisher: Some("private-publisher".into()),
            content: Some("private-announcement-body".into()),
            published_at_utc: "2026-09-25T00:00:00Z".into(),
            expires_at_utc: None,
            read: Some(false),
            important: Some(true),
            favorited: None,
            expired: false,
        };
        let homework = LearnHomeworkDto {
            reference_id: "private-homework-selector".into(),
            title: "private-homework-title".into(),
            state: LearnHomeworkStateDto::Pending,
            due_at_utc: "2026-09-26T00:00:00Z".into(),
            late_due_at_utc: None,
            submitted_at_utc: None,
            graded_at_utc: None,
            detail_available: true,
        };
        let detail = LearnHomeworkDetailDto {
            description: Some("private-description".into()),
            answer_content: Some("private-answer".into()),
            submitted_content: Some("private-submission".into()),
            attachments: vec![LearnHomeworkAttachmentDto {
                kind: LearnHomeworkAttachmentKindDto::Assignment,
                name: "private-file-name".into(),
                size: Some("private-file-size".into()),
            }],
        };
        let file = LearnCourseFileDto {
            reference_id: "private-file-selector".into(),
            title: "private-file-title".into(),
            suggested_filename: "private-filename".into(),
            description: Some("private-file-description".into()),
            size_label: Some("private-size-label".into()),
            uploaded_at_label: Some("private-upload-date".into()),
            file_type: Some("private-file-type".into()),
        };
        let discussion = LearnCourseDiscussionDto {
            title: "private-discussion-title".into(),
            publisher: "private-discussion-publisher".into(),
            published_at_label: "private-discussion-date".into(),
            last_reply_at_label: None,
            reply_count: 2,
        };
        let debug = format!("{announcement:?} {homework:?} {detail:?} {file:?} {discussion:?}");
        for private_value in [
            "private-title",
            "private-publisher",
            "private-announcement-body",
            "private-homework-selector",
            "private-homework-title",
            "private-description",
            "private-answer",
            "private-submission",
            "private-file-name",
            "private-file-selector",
            "private-filename",
            "private-file-description",
            "private-discussion-title",
            "private-discussion-publisher",
            "private-discussion-date",
        ] {
            assert!(!debug.contains(private_value));
        }
    }
}
