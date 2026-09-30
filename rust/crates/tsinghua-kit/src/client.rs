//! Stable application-facing client and domain facades.

use std::{fmt, path::PathBuf};

use tsinghua_kit_engine::{
    CourseScore,
    assessment_read::{
        AssessmentAnswers as EngineAssessmentAnswers,
        AssessmentFormView as EngineAssessmentFormView, AssessmentList as EngineAssessmentList,
        AssessmentRef,
    },
    auth::{AccountAuthStatus, AuthStatus, SecondFactorMethod, SelfServiceLoginPhase},
    bank_read::{BankLedger, BankPaymentLedger, GraduateIncomePage},
    calendar_api::{LearnTermCalendar, SchoolCalendarImage, SchoolCalendarQuery},
    campus_card_api::{
        CampusCardAccount, CampusCardInteraction, CampusCardPasswordRequest,
        CampusCardTransactionRange, CampusCardTransactions, CampusCardWriteRequest,
    },
    classrooms_api::{
        BuildingRef, ClassroomAvailability, ClassroomBuildings, ClassroomWeekSelection,
    },
    client::{
        AssessmentClient as EngineAssessmentClient, BankClient as EngineBankClient,
        CalendarClient as EngineCalendarClient, CampusCardClient as EngineCampusCardClient,
        ClassroomsClient as EngineClassroomsClient, Client as EngineClient,
        ClientBuilder as EngineClientBuilder, ClientCachePolicy as EngineCachePolicy,
        CourseScoreClient as EngineCourseScoreClient,
        CredentialStoragePolicy as EngineCredentialStoragePolicy,
        ElectricityClient as EngineElectricityClient, IdentityLoginOutcome, IdentityLoginRequest,
        IdentitySessionStoragePolicy as EngineIdentitySessionStoragePolicy,
        InvoiceClient as EngineInvoiceClient, LearnClient as EngineLearnClient,
        LibraryClient as EngineLibraryClient, LibraryRoomClient as EngineLibraryRoomClient,
        NetworkClient as EngineNetworkClient, NetworkProfilesClient as EngineNetworkProfilesClient,
        NewsClient as EngineNewsClient, PhysicalExamClient as EnginePhysicalExamClient,
        ProgramClient as EngineProgramClient, RegistrarClient as EngineRegistrarClient,
        ReservesClient as EngineReservesClient, SelfServiceCaptcha,
        SelfServiceClient as EngineSelfServiceClient, SelfServiceLoginOutcome,
        ServiceHallClient as EngineServiceHallClient, SportsClient as EngineSportsClient,
    },
    electricity_api::{ElectricityPaymentHistory, ElectricityRemainder},
    error::{Error, ErrorCode},
    invoice_read::{InvoiceDocument, InvoicePage, InvoiceRef},
    learn_api::{
        CourseAnnouncements, CourseCatalog, CourseDiscussions, CourseFileCategories, CourseFileRef,
        CourseFiles, CourseRef, HomeworkDetail, HomeworkList, HomeworkRef, SavedCourseFile,
    },
    library_api::{
        FloorRef, LibraryAvailability, LibraryDay, LibraryDirectory, LibraryFloors, LibraryRef,
        LibraryReservationRef, LibraryReservations, LibrarySections, LibrarySocketAvailability,
        LibraryTimeWindows, SeatRef, SeatWindowRef, SectionRef,
    },
    library_room_read::{LibraryRoomCatalog, LibraryRoomRecord},
    network::{
        NetworkProfileId, NetworkProfileInput, NetworkProfilePassword, NetworkProfileStoragePolicy,
        NetworkProfileSummary, PortalConnectionResult, PreparedNetworkInput,
    },
    news::{
        ArticleDetail, ArticleRef, NewsCatalog, NewsChannelRef, NewsFavorites, NewsPage, NewsQuery,
        NewsSourceRef, NewsSubscriptionRef, NewsSubscriptions,
    },
    overview_api::{DailyOverview, OverviewClient as EngineOverviewClient},
    physical_exam_read::PhysicalExamReport,
    program_read::ProgramCompletion,
    read::{ReadPolicy, ReadResult},
    registrar_api::{ExamReport, GradeReport, SemesterSchedule},
    reserves_read::{ReservesBookDetail, ReservesRef, ReservesSearch},
    self_service::{AccountProfile, DeviceRef, OnlineDevice, UsageBalance},
    service_hall::{
        PendingTasks, PhaseDetails, ServiceDirectory, ServiceHallReadPolicy, TaskView,
        WorkflowTaskList, WorkflowTaskRef,
    },
    sports_read::{SportsReservationRecord, SportsResources},
    sports_write::SportsCaptcha,
};
use zeroize::Zeroize;

/// Cache-directory behavior for one client instance.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientCachePolicy {
    /// Create a private temporary directory and remove it when the client is
    /// dropped. Auth persistence remains controlled by the separate Auth
    /// storage policies, which default to memory-only.
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

/// Storage for credentials the user explicitly chooses to remember.
///
/// This is independent from Identity session snapshots and local network
/// profiles. `JsonDirectory` is the app-managed file mode: Rust writes readable
/// JSON under a host-selected app data directory. This does not call an
/// operating-system credential service.
#[non_exhaustive]
pub enum CredentialStoragePolicy {
    /// Do not persist Auth passwords.
    MemoryOnly,
    /// Store explicitly remembered credentials in this private directory.
    JsonDirectory { root: PathBuf, namespace: String },
}

impl std::fmt::Debug for CredentialStoragePolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemoryOnly => formatter.write_str("CredentialStoragePolicy::MemoryOnly"),
            Self::JsonDirectory { .. } => formatter
                .debug_struct("CredentialStoragePolicy::JsonDirectory")
                .field("configured", &true)
                .finish(),
        }
    }
}

impl From<CredentialStoragePolicy> for EngineCredentialStoragePolicy {
    fn from(value: CredentialStoragePolicy) -> Self {
        match value {
            CredentialStoragePolicy::MemoryOnly => Self::MemoryOnly,
            CredentialStoragePolicy::JsonDirectory { root, namespace } => {
                Self::JsonDirectory { root, namespace }
            }
        }
    }
}

/// Persistence for Identity cookies used by explicit session recovery.
///
/// The default keeps cookies in memory. The file option stores a readable,
/// device-bound JSON snapshot under the application-selected app data
/// directory. Restored sessions remain unverified until explicitly checked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentitySessionStoragePolicy {
    /// Keep Identity cookies only for the owning Client lifetime.
    MemoryOnly,
    /// Store a readable, device-bound snapshot in this directory.
    JsonDirectory(PathBuf),
}

impl From<IdentitySessionStoragePolicy> for EngineIdentitySessionStoragePolicy {
    fn from(value: IdentitySessionStoragePolicy) -> Self {
        match value {
            IdentitySessionStoragePolicy::MemoryOnly => Self::MemoryOnly,
            IdentitySessionStoragePolicy::JsonDirectory(path) => Self::JsonDirectory(path),
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
    /// snapshot. Its path is independent from the service-cache directory.
    /// Restored cookies remain unverified until the caller invokes
    /// [`IdentityAuthClient::revalidate_restored_session`].
    pub fn identity_session_storage(mut self, policy: IdentitySessionStoragePolicy) -> Self {
        self.inner = self.inner.identity_session_storage(policy.into());
        self
    }

    /// Selects a separately scoped store for explicit Auth credential choices.
    pub fn credential_storage(mut self, policy: CredentialStoragePolicy) -> Self {
        self.inner = self.inner.credential_storage(policy.into());
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

    /// Borrows the account-bound daily overview from this Client.
    pub fn overview(&mut self) -> OverviewClient<'_> {
        OverviewClient {
            inner: self.inner.overview(),
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

    /// Borrows the read-only physical-education test report.
    pub fn physical_exam(&mut self) -> PhysicalExamClient<'_> {
        PhysicalExamClient {
            inner: self.inner.physical_exam(),
        }
    }

    /// Borrows the read-only degree-program completion report.
    pub fn program(&mut self) -> ProgramClient<'_> {
        ProgramClient {
            inner: self.inner.program(),
        }
    }

    /// Borrows the read-only teaching-evaluation questionnaire list.
    pub fn assessment(&mut self) -> AssessmentClient<'_> {
        AssessmentClient {
            inner: self.inner.assessment(),
        }
    }

    /// Borrows the read-only e-invoice list and document reads.
    pub fn invoice(&mut self) -> InvoiceClient<'_> {
        InvoiceClient {
            inner: self.inner.invoice(),
        }
    }

    /// Borrows the read-only bank payroll and graduate-income statements.
    pub fn bank(&mut self) -> BankClient<'_> {
        BankClient {
            inner: self.inner.bank(),
        }
    }

    /// Borrows the read-only per-course score lookup.
    pub fn course_score(&mut self) -> CourseScoreClient<'_> {
        CourseScoreClient {
            inner: self.inner.course_score(),
        }
    }

    /// Borrows the read-only sports-venue resources and reservation records.
    pub fn sports(&mut self) -> SportsClient<'_> {
        SportsClient {
            inner: self.inner.sports(),
        }
    }

    /// Borrows the read-only course-reserve textbook catalogue.
    pub fn reserves(&mut self) -> ReservesClient<'_> {
        ReservesClient {
            inner: self.inner.reserves(),
        }
    }

    /// Borrows the read-only CAB study-room catalogue and the account's own
    /// reservations.
    pub fn library_room(&mut self) -> LibraryRoomClient<'_> {
        LibraryRoomClient {
            inner: self.inner.library_room(),
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
    remember_credentials: bool,
}

impl SelfServiceLoginRequest {
    /// Creates a request from the account name and password entered by the
    /// user. The two values are independent from Identity credentials.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            remember_credentials: false,
        }
    }

    /// Explicitly opts this SelfService account into Rust-owned credential
    /// storage. The Client must have a credential store configured. This
    /// never restores a session or bypasses a future captcha.
    pub fn remember_credentials(mut self, remember: bool) -> Self {
        self.remember_credentials = remember;
        self
    }
}

impl fmt::Debug for SelfServiceLoginRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelfServiceLoginRequest")
            .field("username_present", &!self.username.trim().is_empty())
            .field("password_present", &!self.password.is_empty())
            .field("remember_credentials", &self.remember_credentials)
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
                request.remember_credentials,
            )
            .await
    }

    /// Starts the captcha flow using a previously remembered credential. The
    /// password remains in Rust and the user must still complete the captcha.
    pub async fn start_saved_login(
        &mut self,
        username: impl Into<String>,
    ) -> Result<SelfServiceCaptcha, Error> {
        self.inner
            .start_saved_self_service_login(username.into())
            .await
    }

    /// Forgets one account's stored SelfService password without logging out.
    pub fn forget_saved_credentials(&mut self, username: &str) -> Result<(), Error> {
        self.inner.forget_saved_self_service_credentials(username)
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

/// A daily academic summary built inside the same authenticated runtime.
pub struct OverviewClient<'client> {
    inner: EngineOverviewClient<'client>,
}

impl OverviewClient<'_> {
    /// Reads a campus date using the specified cache policy. Cache-only reads
    /// never send a request. A partial result retains section-failure flags.
    pub async fn day(
        &mut self,
        date: chrono::NaiveDate,
        policy: ReadPolicy,
    ) -> Result<ReadResult<DailyOverview>, Error> {
        self.inner.day(date, policy).await
    }
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

    /// Applies one card state change: reporting the card lost, reversing that,
    /// changing the transaction password, changing the spending limits, or
    /// topping the card up from its own bound bank account.
    ///
    /// The request is validated and consumed before dispatch, and the change is
    /// sent **exactly once**.  A result reported as `outcome_unconfirmed` was
    /// dispatched but its effect is unknown; it is not sent a second time, and
    /// this call does not re-authenticate and retry either.  The caller learns
    /// what happened by reading [`Self::account`] — the card's own state —
    /// rather than by calling this again.
    ///
    /// An `authentication_rejected` answer is the service's own refusal, so
    /// nothing was applied and the caller may correct its input.
    ///
    /// The transaction password is the card's own service secret.  It is neither
    /// the account password nor the card-service SSO password
    /// [`Self::submit_password`] carries; it is not stored, not logged, and is
    /// zeroized when the request is dropped.
    pub async fn apply_write(&mut self, request: CampusCardWriteRequest) -> Result<(), Error> {
        self.inner.apply_write(request).await
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

    /// Replaces the dormitory service account's own password.
    ///
    /// The dormitory application is the one the electricity reads already reach,
    /// so this uses that proven session; a caller with no proven session is
    /// refused rather than handed a second authentication path.
    ///
    /// The password is copied into exactly one request body, zeroized when that
    /// body is dropped, and is never a result field, a log field or a recorded
    /// failure code.
    ///
    /// The reset is dispatched **exactly once** and is never retried.  An answer
    /// that does not carry affirmative acceptance — which is this route's ordinary
    /// outcome, because the service's own client discards the reply — is reported
    /// as [`ErrorCode::OutcomeUnconfirmed`] and must not be resolved by calling
    /// this again: the change may already be in effect.  Sign in with the new
    /// password to find out what happened.
    pub async fn reset_home_password(&mut self, new_password: &str) -> Result<(), Error> {
        self.inner.reset_home_password(new_password).await
    }
}

/// Read-only physical-education test results through this Client's shared
/// runtime. An explicit no-result answer is data, not an error.
pub struct PhysicalExamClient<'client> {
    inner: EnginePhysicalExamClient<'client>,
}

impl PhysicalExamClient<'_> {
    /// Reads the validated physical-education report.
    pub async fn result(&mut self) -> Result<ReadResult<PhysicalExamReport>, Error> {
        self.inner.result().await
    }
}

/// Read-only degree-program completion through this Client's shared runtime.
/// The report is read live on every call; there is no cached fallback.
pub struct ProgramClient<'client> {
    inner: EngineProgramClient<'client>,
}

impl ProgramClient<'_> {
    /// Reads the plan-wide completion report.
    pub async fn completion(&mut self) -> Result<ReadResult<ProgramCompletion>, Error> {
        self.inner.completion().await
    }
}

/// Read-only teaching-evaluation questionnaire list through this Client's
/// shared runtime. Each item carries an opaque reference; the questionnaire
/// itself is never assembled or returned to the caller.
pub struct AssessmentClient<'client> {
    inner: EngineAssessmentClient<'client>,
}

impl AssessmentClient<'_> {
    /// Reads the questionnaires the account may currently fill in.
    ///
    /// A closed window is reported as the service's own "not available"
    /// state, never as a validated empty list.
    pub async fn list(&mut self) -> Result<ReadResult<EngineAssessmentList>, Error> {
        self.inner.list().await
    }

    /// Reads one questionnaire's questions and the answers the service
    /// currently holds for them.
    ///
    /// The reference must come from this Client's latest [`Self::list`] result.
    /// The returned value is a display copy: it carries no submission state and
    /// cannot be posted back.
    pub async fn form(
        &mut self,
        reference: &AssessmentRef,
    ) -> Result<EngineAssessmentFormView, Error> {
        self.inner.form(reference).await
    }

    /// Stores one filled-in questionnaire.
    ///
    /// The answers are applied to the questionnaire this runtime read, so the
    /// caller supplies scores and comments only — never a body, a route, or the
    /// transaction state the submission carries.
    ///
    /// This is a one-shot write: an unconfirmed outcome is reported as
    /// [`ErrorCode::OutcomeUnconfirmed`](crate::error::ErrorCode::OutcomeUnconfirmed)
    /// and is never replayed, and the same row cannot be submitted twice until
    /// a fresh list read replaces it.
    pub async fn submit(&mut self, answers: &EngineAssessmentAnswers) -> Result<(), Error> {
        self.inner.submit(answers).await
    }
}

/// Read-only e-invoice list and document reads.
///
/// The page is read live on every call and never served from a cached copy: a
/// retained page would present a superseded reimbursement state as the current
/// one.  A document reference only resolves against the list this client most
/// recently read.
pub struct InvoiceClient<'client> {
    inner: EngineInvoiceClient<'client>,
}

impl InvoiceClient<'_> {
    /// The one-based page range the service accepts through this client.
    pub const MAX_PAGE: u32 = EngineInvoiceClient::MAX_PAGE;

    /// The page size this client requests from the service.
    pub const PAGE_SIZE: u32 = EngineInvoiceClient::PAGE_SIZE;

    /// Reads one page of issued e-invoices.
    ///
    /// `page` is one-based and bounded by [`InvoiceClient::MAX_PAGE`]; a page
    /// outside that range is refused before any request.
    pub async fn list(&mut self, page: u32) -> Result<ReadResult<InvoicePage>, Error> {
        self.inner.list(page).await
    }

    /// Reads one invoice's document.
    ///
    /// The reference must come from this client's most recent [`Self::list`]
    /// result; a reference from an earlier page or a dropped session does not
    /// resolve, and the service's own PDF bytes are returned only when the
    /// answer really was that document.
    pub async fn document(
        &mut self,
        reference: &InvoiceRef,
    ) -> Result<ReadResult<InvoiceDocument>, Error> {
        self.inner.document(reference).await
    }
}

/// Read-only bank payroll and graduate-income statements.
///
/// Both are money statements, so every amount is exact integer cents.  Each
/// statement is read live on every call and never served from a cached copy: a
/// retained statement would present a superseded disbursement as the current
/// one.
pub struct BankClient<'client> {
    inner: EngineBankClient<'client>,
}

impl BankClient<'_> {
    /// Reads one payroll ledger: the years the service offers this account,
    /// then every receipt those years hold.
    ///
    /// The two ledgers are two path families on one campus host, so each is
    /// read separately; one ledger never reports the other's receipts.
    pub async fn ledger(
        &mut self,
        ledger: BankLedger,
    ) -> Result<ReadResult<BankPaymentLedger>, Error> {
        self.inner.ledger(ledger).await
    }

    /// Reads one page of graduate-income records for a `YYYYMMDD` date range.
    ///
    /// Both bounds must be eight digits; a range that is not is refused before
    /// any request, so caller text never becomes a service-side filter.
    pub async fn graduate_income(
        &mut self,
        begin: &str,
        end: &str,
    ) -> Result<ReadResult<GraduateIncomePage>, Error> {
        self.inner.graduate_income(begin, end).await
    }
}

/// Sports-venue resources, reservation records, and the venue's own state
/// changes.
///
/// Both reads go live on every call and are never served from a cached copy: a
/// venue's availability changes minute by minute and a reservation list is a
/// booking state, so a retained copy would present a taken court as free or a
/// cancelled booking as live.
///
/// The writes are the venue's own three: an order, a withdrawal, and the order
/// form's image challenge.  Each of them is dispatched exactly once and an
/// outcome the venue did not confirm is reported as `outcome_unconfirmed` and
/// never replayed — a venue state change whose answer was lost may already be in
/// effect.  The payment chain that follows an order is deliberately not
/// reachable: see `tsinghua_kit_engine::sports_write`'s boundary constants.
pub struct SportsClient<'client> {
    inner: EngineSportsClient<'client>,
}

impl SportsClient<'_> {
    /// Reads one venue's limits, configured phone number, and slot list for one
    /// `YYYY-MM-DD` date.
    ///
    /// `gym_id` and `item_id` must be digit strings and `date` a real calendar
    /// day; a value that is not is refused before any request, so caller text
    /// never becomes a service-side filter.
    ///
    /// This read is also what makes an order possible: every slot the venue
    /// offers for online booking comes back with a `selector`, and only that
    /// handle can name the slot to [`Self::make_order`].  Any newer read of the
    /// venue replaces the whole set.
    pub async fn resources(
        &mut self,
        gym_id: &str,
        item_id: &str,
        date: &str,
    ) -> Result<ReadResult<SportsResources>, Error> {
        self.inner.resources(gym_id, item_id, date).await
    }

    /// Reads the account's unpaid reservations followed by its paid ones.
    ///
    /// This read is what makes a withdrawal possible: every row the venue still
    /// lets this account cancel comes back with a `selector`, and only that
    /// handle can name the reservation to [`Self::unsubscribe`].
    pub async fn records(&mut self) -> Result<ReadResult<Vec<SportsReservationRecord>>, Error> {
        self.inner.records().await
    }

    /// Reads the booking form's own image challenge.
    ///
    /// This is a read on the already-proved venue session and may be repeated: a
    /// person whose first image was unreadable asks for another.  The bytes are
    /// the venue's own rendering and the content type is verified in Rust to be
    /// a bounded raster image, so a login page or an error document answered
    /// with HTTP 200 is an error rather than an image.
    pub async fn captcha(&mut self) -> Result<SportsCaptcha, Error> {
        self.inner.captcha().await
    }

    /// Places one order for a slot from this client's latest [`Self::resources`]
    /// read.
    ///
    /// `selector` must be a `SportsResource::selector()` of that read; the
    /// venue's own `res_hash`, the venue and item identifiers, the date and the
    /// cost all come from the same read, so nothing a caller supplies reaches
    /// the venue as an identifier.  The contact number is the one the venue
    /// itself reported for this account.
    ///
    /// `captcha` is a person's own transcription of [`Self::captcha`]'s image.
    /// Nothing here invents, guesses, re-reads or retries one.
    ///
    /// The order is dispatched **exactly once**.  An answer the venue worded as
    /// a refusal is a definite "nothing was booked"; an answer that could not be
    /// read, or a request that left without one, is `outcome_unconfirmed` and
    /// must never be resolved by calling this again — re-read [`Self::records`]
    /// to learn what the account now holds.
    pub async fn make_order(&mut self, selector: &str, captcha: &str) -> Result<(), Error> {
        self.inner.make_order(selector, captcha).await
    }

    /// Withdraws one reservation from this client's latest [`Self::records`]
    /// read.
    ///
    /// `selector` must be a `SportsReservationRecord::selector()` of that read.
    /// The withdrawal is dispatched exactly once; no client has ever observed
    /// this route's refusal wording, so a readable answer that is not
    /// affirmative is reported as `outcome_unconfirmed` rather than as a
    /// refusal, and an unconfirmed outcome is never replayed.
    pub async fn unsubscribe(&mut self, selector: &str) -> Result<(), Error> {
        self.inner.unsubscribe(selector).await
    }
}

/// Read-only course-reserve textbook catalogue.
///
/// Both reads go live on every call and are never served from a cached copy: a
/// holding can change between requests, so a retained catalogue would present a
/// withdrawn book as available. A book reference only resolves against the
/// search this client most recently completed.
pub struct ReservesClient<'client> {
    inner: EngineReservesClient<'client>,
}

impl ReservesClient<'_> {
    /// The highest one-based page the service accepts through this client.
    pub const MAX_PAGE: u32 = EngineReservesClient::MAX_PAGE;

    /// Searches the reserve catalogue by book name.
    ///
    /// The name is escaped with the service's own `%uXXXX` convention inside
    /// the engine, so caller text never arrives as a raw query value; a name
    /// that cannot be represented is refused before any request.
    pub async fn search(
        &mut self,
        book_name: &str,
        page: u32,
    ) -> Result<ReadResult<ReservesSearch>, Error> {
        self.inner.search(book_name, page).await
    }

    /// Reads one book's bibliographic detail and chapter list.
    ///
    /// The reference must come from the search this client most recently
    /// completed; one from an earlier search or another client does not resolve.
    pub async fn detail(
        &mut self,
        reference: &ReservesRef,
    ) -> Result<ReadResult<ReservesBookDetail>, Error> {
        self.inner.detail(reference).await
    }
}

/// Read-only CAB study-room catalogue and the account's own reservations.
///
/// Both reads go live on every call and are never served from a cached copy: a
/// room can be withdrawn or reopened between requests, so a retained catalogue
/// would present an unusable room as reservable.
///
/// This API is read-only by construction. The application's own campus login is
/// deliberately not implemented — the reference derives the application id it
/// would submit from a response — so an expired session is reported as an
/// authentication failure rather than answered with an empty catalogue.
pub struct LibraryRoomClient<'client> {
    inner: EngineLibraryRoomClient<'client>,
}

impl LibraryRoomClient<'_> {
    /// The widest reservation window this client will read, in days.
    pub const MAX_WINDOW_DAYS: i64 = EngineLibraryRoomClient::MAX_WINDOW_DAYS;

    /// Reads the reservable study-room catalogue.
    ///
    /// An empty catalogue is only ever the service's own answer that nothing is
    /// currently reservable; a response that did not carry the service's
    /// envelope is an error instead.
    pub async fn catalog(&mut self) -> Result<ReadResult<LibraryRoomCatalog>, Error> {
        self.inner.catalog().await
    }

    /// Reads the account's own reservations for one bounded window.
    ///
    /// `begin` and `end` are `YYYY-MM-DD`. Both are validated inside Rust before
    /// any request, so a reversed, malformed, or window wider than
    /// [Self::MAX_WINDOW_DAYS] is refused as invalid input without costing a
    /// handoff.
    pub async fn records(
        &mut self,
        begin: &str,
        end: &str,
    ) -> Result<ReadResult<Vec<LibraryRoomRecord>>, Error> {
        self.inner.records(begin, end).await
    }
}

/// One course result looked up by course number.
///
/// The lookup is read live on every call and never served from a cached copy:
/// a grade can be revised by the registrar at any time, so a retained value
/// would present a superseded result as current.
///
/// The account's own student id, which the service's query also needs, is
/// derived inside Rust from the proven identity; it is never an argument, a
/// result field, or a log field.
pub struct CourseScoreClient<'client> {
    inner: EngineCourseScoreClient<'client>,
}

impl CourseScoreClient<'_> {
    /// Looks up one course result by the caller's course number.
    ///
    /// The course number is validated before any request, so a value this
    /// client will not send is reported as invalid input rather than becoming a
    /// service-side query.
    pub async fn lookup(&mut self, course_id: &str) -> Result<ReadResult<CourseScore>, Error> {
        self.inner.lookup(course_id).await
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

    /// Reads the account's own reservation list.
    ///
    /// This read is what makes a cancellation possible: every row the service
    /// still lets this account cancel is returned with a reference scoped to
    /// this client and this read. The service's own cancellation identifier
    /// never leaves Rust.
    pub async fn reservations(&mut self) -> Result<ReadResult<LibraryReservations>, Error> {
        self.inner.reservations().await
    }

    /// Reserves one seat of a window returned by [`Self::time_windows`].
    ///
    /// The seat must be one of this client's latest [`Self::seats`] result for
    /// the same section. Both the seat and the opening window are re-checked
    /// against the Runtime's own inventory before the request is built, so a
    /// stale or foreign selection is refused rather than sent.
    ///
    /// This is a single dispatch. An outcome the service does not confirm is
    /// reported as `outcome_unconfirmed` and is never retried here or by the
    /// service layer, so the caller must re-read [`Self::reservations`] to learn
    /// what the account now holds.
    pub async fn reserve(&mut self, window: &SeatWindowRef, seat: &SeatRef) -> Result<(), Error> {
        self.inner.reserve(window, seat).await
    }

    /// Cancels one reservation returned by [`Self::reservations`].
    ///
    /// A reference from another client or from an earlier reservation read is
    /// refused before any request. The cancellation is dispatched at most once
    /// and an unconfirmed outcome is never replayed.
    pub async fn cancel(&mut self, reference: &LibraryReservationRef) -> Result<(), Error> {
        self.inner.cancel(reference).await
    }

    /// Turns the power socket of one seat on or off.
    ///
    /// The seat must belong to one of this client's latest [`Self::seats`]
    /// results for the same section, which is re-checked against the Runtime's
    /// own inventory before the request is built.
    ///
    /// This route leaves the seat-inventory mapping: it is a JSON POST to the
    /// campus app origin and carries no library booking token. The write is
    /// dispatched at most once and an outcome the service does not confirm is
    /// reported as `outcome_unconfirmed` rather than replayed; the seat
    /// reference is deliberately not spent, so [`Self::sockets`] stays usable
    /// for re-reading the state this write changed.
    pub async fn set_socket_state(
        &mut self,
        availability: &LibraryAvailability,
        seat: &SeatRef,
        is_available: bool,
    ) -> Result<(), Error> {
        self.inner
            .set_socket_state(availability, seat, is_available)
            .await
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

    /// Adds one article from this client's current news result to the
    /// account's favorites.
    ///
    /// This is a single dispatch. An outcome the service does not confirm is
    /// reported as `outcome_unconfirmed` and is never retried here or by the
    /// service layer, so the caller must re-read `favorites` to learn what the
    /// account now holds.
    pub async fn add_favorite(&mut self, reference: &ArticleRef) -> Result<(), Error> {
        self.inner.add_favorite(reference).await
    }

    /// Removes one article from this client's current news result from the
    /// account's favorites.
    pub async fn remove_favorite(&mut self, reference: &ArticleRef) -> Result<(), Error> {
        self.inner.remove_favorite(reference).await
    }

    /// Adds one subscription rule naming a channel, a source, or both, each
    /// chosen from this client's latest catalog read.
    pub async fn add_subscription(
        &mut self,
        channel: Option<&NewsChannelRef>,
        source: Option<&NewsSourceRef>,
        keyword: Option<&str>,
    ) -> Result<(), Error> {
        self.inner.add_subscription(channel, source, keyword).await
    }

    /// Removes one subscription rule from this client's latest subscription
    /// result.
    pub async fn remove_subscription(
        &mut self,
        reference: &NewsSubscriptionRef,
    ) -> Result<(), Error> {
        self.inner.remove_subscription(reference).await
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

    /// Explicitly connects a saved Portal profile. A per-attempt password
    /// override is never saved; when omitted, Rust uses only a password that
    /// the user previously opted to store with this profile. Profiles for
    /// system-managed Wi-Fi/EAP return `Unsupported`.
    pub async fn connect_portal_profile(
        &mut self,
        prepared: &PreparedNetworkInput,
        password_override: Option<String>,
    ) -> Result<PortalConnectionResult, Error> {
        self.inner
            .connect_portal_profile(prepared, password_override)
            .await
    }

    /// Explicitly disconnects a Portal target verified by this process.
    /// The target is one-shot and is not restored after Client disposal.
    pub async fn disconnect_portal(&mut self) -> Result<PortalConnectionResult, Error> {
        self.inner.disconnect_portal().await
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
