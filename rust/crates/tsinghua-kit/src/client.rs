//! Stable application-facing client and domain facades.

use std::{fmt, path::PathBuf};

use tsinghua_kit_engine::{
    auth::{AccountAuthStatus, AuthStatus, SecondFactorMethod, SelfServiceLoginPhase},
    calendar_api::{LearnTermCalendar, SchoolCalendarImage, SchoolCalendarQuery},
    campus_card_api::{
        CampusCardAccount, CampusCardInteraction, CampusCardPasswordRequest,
        CampusCardTransactionRange, CampusCardTransactions,
    },
    classrooms_api::{
        BuildingRef, ClassroomAvailability, ClassroomBuildings, ClassroomWeekSelection,
    },
    client::{
        CalendarClient as EngineCalendarClient, CampusCardClient as EngineCampusCardClient,
        ClassroomsClient as EngineClassroomsClient, Client as EngineClient,
        ClientBuilder as EngineClientBuilder, ClientCachePolicy as EngineCachePolicy,
        ElectricityClient as EngineElectricityClient, IdentityLoginOutcome, IdentityLoginRequest,
        IdentitySessionStoragePolicy, LearnClient as EngineLearnClient,
        LibraryClient as EngineLibraryClient, NetworkClient as EngineNetworkClient,
        NetworkProfilesClient as EngineNetworkProfilesClient, NewsClient as EngineNewsClient,
        RegistrarClient as EngineRegistrarClient, SelfServiceCaptcha,
        SelfServiceClient as EngineSelfServiceClient, SelfServiceLoginOutcome,
        ServiceHallClient as EngineServiceHallClient,
    },
    electricity_api::{ElectricityPaymentHistory, ElectricityRemainder},
    error::Error,
    learn_api::{
        CourseAnnouncements, CourseCatalog, CourseDiscussions, CourseFileCategories, CourseFileRef,
        CourseFiles, CourseRef, HomeworkDetail, HomeworkList, HomeworkRef, SavedCourseFile,
    },
    library_api::{
        FloorRef, LibraryAvailability, LibraryDay, LibraryDirectory, LibraryFloors, LibraryRef,
        LibrarySections, LibrarySocketAvailability, LibraryTimeWindows, SeatWindowRef, SectionRef,
    },
    network::{
        NetworkProfileId, NetworkProfileInput, NetworkProfilePassword, NetworkProfileStoragePolicy,
        NetworkProfileSummary, PreparedNetworkInput,
    },
    news::{
        ArticleDetail, ArticleRef, NewsCatalog, NewsFavorites, NewsPage, NewsQuery,
        NewsSubscriptionRef, NewsSubscriptions,
    },
    read::ReadResult,
    registrar_api::{ExamReport, GradeReport, SemesterSchedule},
    self_service::{AccountProfile, DeviceRef, OnlineDevice, UsageBalance},
    service_hall::{
        PendingTasks, PhaseDetails, ServiceDirectory, ServiceHallReadPolicy, TaskView,
        WorkflowTaskList, WorkflowTaskRef,
    },
};
use zeroize::Zeroize;

/// Cache-directory behavior for one client instance.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientCachePolicy {
    /// Create a private temporary directory and remove it when the client is
    /// dropped. Authentication sessions remain memory-only.
    Ephemeral,
    /// Store non-credential read caches in this host-selected directory.
    Directory(PathBuf),
}

impl From<ClientCachePolicy> for EngineCachePolicy {
    fn from(value: ClientCachePolicy) -> Self {
        match value {
            ClientCachePolicy::Ephemeral => Self::Ephemeral,
            ClientCachePolicy::Directory(path) => Self::Directory(path),
        }
    }
}

/// Builds a client without performing authentication or network requests.
pub struct ClientBuilder {
    inner: EngineClientBuilder,
}

impl ClientBuilder {
    /// Selects where non-credential business read caches are stored.
    pub fn cache_policy(mut self, policy: ClientCachePolicy) -> Self {
        self.inner = self.inner.cache_policy(policy.into());
        self
    }

    /// Selects opt-in storage for local network profiles. This policy is
    /// independent of Auth sessions and defaults to memory-only.
    pub fn network_profile_storage(mut self, policy: NetworkProfileStoragePolicy) -> Self {
        self.inner = self.inner.network_profile_storage(policy);
        self
    }

    /// Selects explicit persistence for Identity and its shared service-cookie
    /// snapshot. When enabled, the same path must be selected as the cache
    /// directory. Restored cookies remain unverified until the caller invokes
    /// [`IdentityAuthClient::revalidate_restored_session`].
    pub fn identity_session_storage(mut self, policy: IdentitySessionStoragePolicy) -> Self {
        self.inner = self.inner.identity_session_storage(policy);
        self
    }

    /// Builds one Rust runtime with two independent account domains.
    pub fn build(self) -> Result<Client, Error> {
        Ok(Client {
            inner: self.inner.build()?,
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            inner: EngineClient::builder(),
        }
    }
}

/// A single-owner client for Tsinghua services.
///
/// Identity and SelfService sessions are independent and may use different
/// usernames. Local campus-network profiles belong to [`crate::network`] and
/// never create a third account slot.
pub struct Client {
    inner: EngineClient,
}

impl Client {
    /// Starts a client with isolated, memory-only authentication state.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Borrows the two account-domain authentication APIs.
    pub fn auth(&mut self) -> AuthClient<'_> {
        AuthClient {
            inner: &mut self.inner,
        }
    }

    /// Borrows online service-hall workflow reads.
    pub fn service_hall(&mut self) -> ServiceHallClient<'_> {
        ServiceHallClient {
            inner: self.inner.service_hall(),
        }
    }

    /// Borrows read-only INFO news queries and context-bound article details.
    pub fn news(&mut self) -> NewsClient<'_> {
        NewsClient {
            inner: self.inner.news(),
        }
    }

    /// Borrows network-learning course and student-assignment reads.
    pub fn learn(&mut self) -> LearnClient<'_> {
        LearnClient {
            inner: self.inner.learn(),
        }
    }

    /// Borrows Registrar reads for the authenticated academic account.
    pub fn registrar(&mut self) -> RegistrarClient<'_> {
        RegistrarClient {
            inner: self.inner.registrar(),
        }
    }

    /// Borrows read-only library queries through the shared Rust runtime.
    pub fn library(&mut self) -> LibraryClient<'_> {
        LibraryClient {
            inner: self.inner.library(),
        }
    }

    /// Borrows read-only classroom building and weekly-availability reads.
    pub fn classrooms(&mut self) -> ClassroomsClient<'_> {
        ClassroomsClient {
            inner: self.inner.classrooms(),
        }
    }

    /// Borrows read-only campus-card operations through this Client's shared
    /// Rust runtime. The target-specific card password is a one-shot service
    /// interaction, not a third Auth account.
    pub fn campus_card(&mut self) -> CampusCardClient<'_> {
        CampusCardClient {
            inner: self.inner.campus_card(),
        }
    }

    /// Borrows read-only electricity balance and payment-history operations.
    pub fn electricity(&mut self) -> ElectricityClient<'_> {
        ElectricityClient {
            inner: self.inner.electricity(),
        }
    }

    /// Borrows school-wide calendar reads and Learn's term timeline.
    pub fn calendar(&mut self) -> CalendarClient<'_> {
        CalendarClient {
            inner: self.inner.calendar(),
        }
    }

    /// Borrows business data associated with the independent SelfService
    /// account. Login and account state are managed through [`Self::auth`].
    pub fn self_service(&mut self) -> SelfServiceClient<'_> {
        SelfServiceClient {
            inner: self.inner.self_service(),
        }
    }

    /// Borrows read-only observations for the local campus-network path.
    pub fn network(&mut self) -> NetworkClient<'_> {
        NetworkClient {
            inner: self.inner.network(),
        }
    }
}

/// Authentication state and account-specific login flows.
pub struct AuthClient<'client> {
    inner: &'client mut EngineClient,
}

impl AuthClient<'_> {
    /// Returns the current states of both independent account domains.
    pub fn status(&self) -> AuthStatus {
        self.inner.auth_status()
    }

    /// Borrows the unified Identity login and second-factor flow.
    pub fn identity(&mut self) -> IdentityAuthClient<'_> {
        IdentityAuthClient { inner: self.inner }
    }

    /// Borrows the independent network self-service login flow.
    pub fn self_service(&mut self) -> SelfServiceAuthClient<'_> {
        SelfServiceAuthClient { inner: self.inner }
    }

    /// Logs out both account domains and their authenticated service sessions.
    /// Local network connection profiles are unaffected.
    pub fn logout_all(&mut self) -> Result<AuthStatus, Error> {
        self.inner.logout_all()
    }
}

/// Unified Identity authentication operations.
pub struct IdentityAuthClient<'client> {
    inner: &'client mut EngineClient,
}

impl IdentityAuthClient<'_> {
    /// Returns the current Identity account state.
    pub fn status(&self) -> AccountAuthStatus {
        self.inner.auth_status().identity().clone()
    }

    /// Returns an active Identity or downstream service second-factor
    /// challenge. Card target-password prompts remain available from
    /// [`CampusCardClient::pending_interaction`].
    pub fn interaction(&self) -> Result<Option<IdentityLoginOutcome>, Error> {
        self.inner.identity_interaction()
    }

    /// Explicitly revalidates a restored Identity cookie snapshot. This is a
    /// no-op when no restorable session is present and does not submit any
    /// saved password.
    pub async fn revalidate_restored_session(&mut self) -> Result<AuthStatus, Error> {
        self.inner.revalidate_identity_session().await
    }

    /// Starts Identity login with the selected academic stage preference.
    pub async fn login(
        &mut self,
        request: IdentityLoginRequest,
    ) -> Result<IdentityLoginOutcome, Error> {
        self.inner.login_identity(request).await
    }

    /// Sends a second-factor code requested by the current Identity challenge.
    pub async fn send_code(
        &mut self,
        method: SecondFactorMethod,
    ) -> Result<IdentityLoginOutcome, Error> {
        self.inner.send_second_factor_code(method).await
    }

    /// Completes the current Identity second-factor challenge once.
    pub async fn submit_code(
        &mut self,
        method: SecondFactorMethod,
        code: String,
    ) -> Result<IdentityLoginOutcome, Error> {
        self.inner.complete_second_factor(method, code).await
    }

    /// Logs out Identity and invalidates service proofs derived from it. If a
    /// SelfService account was selected, it remains visible as expired because
    /// that service shares Identity's access route.
    pub fn logout(&mut self) -> Result<AuthStatus, Error> {
        self.inner.logout_identity()
    }
}

/// One-shot credentials for an independent SelfService login.
pub struct SelfServiceLoginRequest {
    username: String,
    password: String,
}

impl SelfServiceLoginRequest {
    /// Creates a request from the account name and password entered by the
    /// user. The two values are independent from Identity credentials.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }
}

impl fmt::Debug for SelfServiceLoginRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelfServiceLoginRequest")
            .field("username_present", &!self.username.trim().is_empty())
            .field("password_present", &!self.password.is_empty())
            .finish()
    }
}

impl Drop for SelfServiceLoginRequest {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

/// USEREG authentication operations for the SelfService account domain.
pub struct SelfServiceAuthClient<'client> {
    inner: &'client mut EngineClient,
}

impl SelfServiceAuthClient<'_> {
    /// Returns the current SelfService account state.
    pub fn status(&self) -> AccountAuthStatus {
        self.inner.auth_status().self_service().clone()
    }

    /// Returns the local captcha phase without making a request.
    ///
    /// An expired or invalid challenge is cleared and reported as
    /// [`SelfServiceLoginPhase::RestartRequired`].
    pub fn login_phase(&mut self) -> SelfServiceLoginPhase {
        self.inner.self_service_login_phase()
    }

    /// Logs out only SelfService. Identity remains active, and the local
    /// campus-network profiles are unchanged.
    pub fn logout(&mut self) -> AuthStatus {
        self.inner.logout_self_service()
    }

    /// Starts one login attempt and returns its image captcha. Identity may
    /// provide the access route, but these credentials remain SelfService
    /// credentials.
    pub async fn start_login(
        &mut self,
        mut request: SelfServiceLoginRequest,
    ) -> Result<SelfServiceCaptcha, Error> {
        self.inner
            .start_self_service_login(
                std::mem::take(&mut request.username),
                std::mem::take(&mut request.password),
            )
            .await
    }

    /// Refreshes the active captcha explicitly. This does not resubmit a
    /// previous password or captcha answer.
    pub async fn refresh_captcha(&mut self) -> Result<SelfServiceCaptcha, Error> {
        self.inner.refresh_self_service_captcha().await
    }

    /// Submits the human-entered captcha and optional service SMS code once.
    pub async fn submit_captcha(
        &mut self,
        answer: String,
        sms_code: Option<String>,
    ) -> Result<SelfServiceLoginOutcome, Error> {
        self.inner
            .complete_self_service_login(answer, sms_code)
            .await
    }

    /// Cancels only the unfinished SelfService captcha flow.
    pub fn cancel_login(&mut self) {
        self.inner.cancel_self_service_login();
    }
}

/// Service-hall reads through this client's shared runtime.
pub struct ServiceHallClient<'client> {
    inner: EngineServiceHallClient<'client>,
}

impl ServiceHallClient<'_> {
    /// Reads pending online workflow tasks with the requested cache policy.
    pub async fn pending(
        &mut self,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<PendingTasks>, Error> {
        self.inner.pending(policy).await
    }

    /// Reads the complete read-only online service catalogue.
    pub async fn services(
        &mut self,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<ServiceDirectory>, Error> {
        self.inner.services(policy).await
    }

    /// Reads completed items, drafts, copies, or phased workflows.
    pub async fn tasks(
        &mut self,
        view: TaskView,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<WorkflowTaskList>, Error> {
        self.inner.tasks(view, policy).await
    }

    /// Reads the verified stages of a task from this Client's recent complete
    /// phased-workflow list.
    pub async fn phase_details(
        &mut self,
        reference: &WorkflowTaskRef,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<PhaseDetails>, Error> {
        self.inner.phase_details(reference, policy).await
    }
}

/// INFO news operations through this client's shared Rust runtime.
pub struct NewsClient<'client> {
    inner: EngineNewsClient<'client>,
}

/// Network-learning courses and read-only student assignments.
pub struct LearnClient<'client> {
    inner: EngineLearnClient<'client>,
}

/// Schedule, grade, and examination reads through the shared Rust runtime.
pub struct RegistrarClient<'client> {
    inner: EngineRegistrarClient<'client>,
}

/// Library directory, opening-window, seat, and socket reads.
pub struct LibraryClient<'client> {
    inner: EngineLibraryClient<'client>,
}

/// Classroom building directories and verified weekly availability.
pub struct ClassroomsClient<'client> {
    inner: EngineClassroomsClient<'client>,
}

/// Read-only campus-card data and the explicit target-password challenge.
pub struct CampusCardClient<'client> {
    inner: EngineCampusCardClient<'client>,
}

impl CampusCardClient<'_> {
    /// Returns the current service-password interaction, if a preceding read
    /// explicitly requested one.
    pub fn pending_interaction(&self) -> Option<CampusCardInteraction> {
        self.inner.pending_interaction()
    }

    /// Reads the card account with source and freshness metadata.
    pub async fn account(&mut self) -> Result<ReadResult<CampusCardAccount>, Error> {
        self.inner.account().await
    }

    /// Reads a complete, bounded transaction date range.
    pub async fn transactions(
        &mut self,
        range: CampusCardTransactionRange,
    ) -> Result<ReadResult<CampusCardTransactions>, Error> {
        self.inner.transactions(range).await
    }

    /// Submits a card-service password requested by this Client's active
    /// challenge. The password is consumed and zeroized in Rust.
    pub async fn submit_password(
        &mut self,
        request: CampusCardPasswordRequest,
    ) -> Result<(), Error> {
        self.inner.submit_password(request).await
    }

    /// Cancels the pending prompt without sending a request.
    pub fn cancel_password_challenge(&mut self) -> bool {
        self.inner.cancel_password_challenge()
    }
}

/// Read-only dorm-electricity reads through this Client's shared runtime.
pub struct ElectricityClient<'client> {
    inner: EngineElectricityClient<'client>,
}

impl ElectricityClient<'_> {
    /// Reads the source's numeric remainder with cache provenance.
    pub async fn remainder(&mut self) -> Result<ReadResult<ElectricityRemainder>, Error> {
        self.inner.remainder().await
    }

    /// Reads the complete payment-history table with cache provenance.
    pub async fn payment_history(
        &mut self,
    ) -> Result<ReadResult<ElectricityPaymentHistory>, Error> {
        self.inner.payment_history().await
    }
}

/// Published school-calendar images and Learn term dates.
pub struct CalendarClient<'client> {
    inner: EngineCalendarClient<'client>,
}

impl CalendarClient<'_> {
    /// Reads the current and following terms from the authenticated Learn
    /// service, with live-source metadata.
    pub async fn learn_terms(&mut self) -> Result<ReadResult<LearnTermCalendar>, Error> {
        self.inner.learn_terms().await
    }

    /// Reads a selected published school calendar image through Rust HTTP.
    pub async fn school_calendar(
        &mut self,
        query: SchoolCalendarQuery,
    ) -> Result<ReadResult<SchoolCalendarImage>, Error> {
        self.inner.school_calendar(query).await
    }
}

impl RegistrarClient<'_> {
    /// Reads the complete verified schedule for the Runtime-selected semester.
    pub async fn semester_schedule(&mut self) -> Result<ReadResult<SemesterSchedule>, Error> {
        self.inner.semester_schedule().await
    }

    /// Reads the verified grade report for the authenticated academic stage.
    pub async fn grades(&mut self) -> Result<ReadResult<GradeReport>, Error> {
        self.inner.grades().await
    }

    /// Reads the complete exam report from the verified stage-specific source.
    pub async fn exams(&mut self) -> Result<ReadResult<ExamReport>, Error> {
        self.inner.exams().await
    }
}

impl LibraryClient<'_> {
    /// Reads the root library directory and its cache metadata.
    pub async fn directory(&mut self) -> Result<ReadResult<LibraryDirectory>, Error> {
        self.inner.directory().await
    }

    /// Reads floors for a library returned by this Client's latest directory.
    pub async fn floors(
        &mut self,
        library: &LibraryRef,
    ) -> Result<ReadResult<LibraryFloors>, Error> {
        self.inner.floors(library).await
    }

    /// Reads sections for today or tomorrow from a returned floor.
    pub async fn sections(
        &mut self,
        floor: &FloorRef,
        day: LibraryDay,
    ) -> Result<ReadResult<LibrarySections>, Error> {
        self.inner.sections(floor, day).await
    }

    /// Reads opening windows for a returned section and its selected date.
    pub async fn time_windows(
        &mut self,
        section: &SectionRef,
    ) -> Result<ReadResult<LibraryTimeWindows>, Error> {
        self.inner.time_windows(section).await
    }

    /// Reads live seat availability for one returned time-window reference.
    pub async fn seats(
        &mut self,
        window: &SeatWindowRef,
    ) -> Result<ReadResult<LibraryAvailability>, Error> {
        self.inner.seats(window).await
    }

    /// Reads socket state for the seats returned by [`Self::seats`].
    pub async fn sockets(
        &mut self,
        availability: &LibraryAvailability,
    ) -> Result<ReadResult<LibrarySocketAvailability>, Error> {
        self.inner.sockets(availability).await
    }
}

impl ClassroomsClient<'_> {
    /// Reads classroom buildings with their actual cache provenance.
    pub async fn buildings(&mut self) -> Result<ReadResult<ClassroomBuildings>, Error> {
        self.inner.buildings().await
    }

    /// Reads one building's weekly matrix using its returned reference.
    pub async fn availability(
        &mut self,
        building: &BuildingRef,
        week: ClassroomWeekSelection,
    ) -> Result<ReadResult<ClassroomAvailability>, Error> {
        self.inner.availability(building, week).await
    }
}

impl LearnClient<'_> {
    /// Reads the current course directory with service-managed cache behavior.
    pub async fn courses(&mut self) -> Result<ReadResult<CourseCatalog>, Error> {
        self.inner.courses().await
    }

    /// Reads active and expired announcements for a course selected from this
    /// client's latest course directory.
    pub async fn announcements(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<CourseAnnouncements>, Error> {
        self.inner.announcements(course).await
    }

    /// Reads every student assignment bucket for a course selected from this
    /// client's latest `courses` result.
    pub async fn homework(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<HomeworkList>, Error> {
        self.inner.homework(course).await
    }

    /// Reads an assignment's detail using a reference from this client's
    /// latest homework list.
    pub async fn homework_detail(
        &mut self,
        reference: &HomeworkRef,
    ) -> Result<ReadResult<HomeworkDetail>, Error> {
        self.inner.homework_detail(reference).await
    }

    /// Reads the bounded live file list for a course selected from this
    /// client's latest course directory. Partial coverage remains explicit.
    pub async fn files(&mut self, course: &CourseRef) -> Result<ReadResult<CourseFiles>, Error> {
        self.inner.files(course).await
    }

    /// Reads the verified category labels for a course. Category selectors are
    /// not exposed until a category-specific query is part of this API.
    pub async fn file_categories(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<CourseFileCategories>, Error> {
        self.inner.file_categories(course).await
    }

    /// Reads a course's live discussion list and reports any 200-item limit.
    pub async fn discussions(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<CourseDiscussions>, Error> {
        self.inner.discussions(course).await
    }

    /// Saves a file selected from the latest file list to an explicit path.
    /// The Runtime revalidates the file and never overwrites an existing path.
    pub async fn save_file(
        &mut self,
        reference: &CourseFileRef,
        destination: impl AsRef<std::path::Path>,
    ) -> Result<SavedCourseFile, Error> {
        self.inner.save_file(reference, destination).await
    }
}

impl NewsClient<'_> {
    /// Reads the current source and channel choices from INFO. Its metadata
    /// marks the channel directory partial when INFO only provides recent
    /// candidates.
    pub async fn catalog(&mut self) -> Result<ReadResult<NewsCatalog>, Error> {
        self.inner.catalog().await
    }

    /// Reads this Identity account's current INFO subscription rules.
    pub async fn subscriptions(&mut self) -> Result<ReadResult<NewsSubscriptions>, Error> {
        self.inner.subscriptions().await
    }

    /// Reads one page for a subscription selected from this client's latest
    /// `subscriptions` result. Each page is a separate live read.
    pub async fn subscription_articles(
        &mut self,
        reference: &NewsSubscriptionRef,
        page: u32,
    ) -> Result<ReadResult<NewsPage>, Error> {
        self.inner.subscription_articles(reference, page).await
    }

    /// Reads the complete bounded set of current-account favorite articles.
    pub async fn favorites(&mut self) -> Result<ReadResult<NewsFavorites>, Error> {
        self.inner.favorites().await
    }

    /// Reads one validated list or search page and preserves its provenance.
    pub async fn articles(
        &mut self,
        query: NewsQuery,
        policy: crate::read::ReadPolicy,
    ) -> Result<ReadResult<NewsPage>, Error> {
        self.inner.articles(query, policy).await
    }

    /// Reads article detail using a reference from this client's current
    /// news result. Foreign and expired references are rejected before I/O.
    pub async fn article(
        &mut self,
        reference: &ArticleRef,
        policy: crate::read::ReadPolicy,
    ) -> Result<ReadResult<ArticleDetail>, Error> {
        self.inner.article(reference, policy).await
    }
}

/// SelfService operations bound to the separate SelfService account.
pub struct SelfServiceClient<'client> {
    inner: EngineSelfServiceClient<'client>,
}

impl SelfServiceClient<'_> {
    /// Reads the SelfService account summary.
    pub async fn account(&mut self) -> Result<ReadResult<AccountProfile>, Error> {
        self.inner.account().await
    }

    /// Reads the complete currently online-device list.
    pub async fn online_devices(&mut self) -> Result<ReadResult<Vec<OnlineDevice>>, Error> {
        self.inner.online_devices().await
    }

    /// Explicitly disconnects a device selected from this client's latest
    /// `online_devices` result. References from another client or an older
    /// list snapshot are rejected before the service request.
    pub async fn disconnect_device(&mut self, reference: &DeviceRef) -> Result<(), Error> {
        self.inner.disconnect_device(reference).await
    }

    /// Reads usage and balance values supplied by the service.
    pub async fn usage(&mut self) -> Result<ReadResult<UsageBalance>, Error> {
        self.inner.usage().await
    }
}

/// Read-only local campus-network observations.
pub struct NetworkClient<'client> {
    inner: EngineNetworkClient<'client>,
}

impl NetworkClient<'_> {
    /// Borrows local, Client-lifetime profile management and form preparation.
    pub fn profiles(&mut self) -> NetworkProfilesClient<'_> {
        NetworkProfilesClient {
            inner: self.inner.profiles(),
        }
    }

    /// Reads TUNet registration status for the current local IPv4 address.
    /// This does not prove general Internet reachability, identify Tsinghua
    /// Secure, or create an Auth account.
    pub async fn observe_portal_status(
        &mut self,
    ) -> Result<crate::network::PortalObservation, Error> {
        self.inner.observe_portal_status().await
    }
}

/// Local profile storage and non-secret form preparation.
pub struct NetworkProfilesClient<'client> {
    inner: EngineNetworkProfilesClient<'client>,
}

impl NetworkProfilesClient<'_> {
    /// Lists this Client's local profiles in stable order.
    pub fn list(&self) -> Vec<NetworkProfileSummary> {
        self.inner.list()
    }

    /// Adds a profile to this Client's Rust-owned local profile store.
    pub fn save(&mut self, input: NetworkProfileInput) -> Result<NetworkProfileSummary, Error> {
        self.inner.save(input)
    }

    /// Updates a profile and invalidates older prepared form values.
    pub fn update(
        &mut self,
        id: NetworkProfileId,
        input: NetworkProfileInput,
    ) -> Result<NetworkProfileSummary, Error> {
        self.inner.update(id, input)
    }

    /// Prepares non-secret form fields while keeping a saved password inside
    /// Rust.
    pub fn prepare_fill(&self, id: NetworkProfileId) -> Result<PreparedNetworkInput, Error> {
        self.inner.prepare_fill(id)
    }

    /// Returns the saved password for a current prepared profile. The caller
    /// must explicitly call `expose_for_form` when the user requests filling.
    pub fn password_for_fill(
        &self,
        prepared: &PreparedNetworkInput,
    ) -> Result<Option<NetworkProfilePassword>, Error> {
        self.inner.password_for_fill(prepared)
    }

    /// Reports whether a prepared form still matches the current profile.
    pub fn is_current(&self, prepared: &PreparedNetworkInput) -> bool {
        self.inner.is_current(prepared)
    }

    /// Deletes a local profile. It does not change either Auth account or the
    /// current network connection.
    pub fn delete(&mut self, id: NetworkProfileId) -> Result<bool, Error> {
        self.inner.delete(id)
    }
}
