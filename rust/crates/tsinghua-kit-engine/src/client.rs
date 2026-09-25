//! Curated public client entry points.
//!
//! The client owns exactly one Rust runtime. Authenticated state remains in
//! memory and disappears with the client; cache files are kept in the
//! configured cache directory and never contain login credentials.

use std::{fmt, fs, path::PathBuf, time::Instant};

use chrono::{Datelike, NaiveDate, Utc};
use zeroize::Zeroize;

use crate::{
    api::{
        academic::CampusGradeReportDto,
        campus::{CampusScheduleDto, CampusSemesterScheduleDto},
        runtime::{
            CampusExamDto, CampusRuntime, CampusRuntimeStatusDto, ClassroomStateResultDto,
            ElectricityPaymentHistoryDto, ElectricityPaymentHistoryResultDto,
            ElectricityRemainderDto, ElectricityRemainderResultDto, InfoNewsItemDto,
            LearnAnnouncementDto, LearnCourseListDto, LearnDiscussionListDto,
            LearnFileCategoryListDto, LearnFileDownloadDto, LearnFileListDto, LearnTermCalendarDto,
            LearnTermDto, SchoolCalendarDto, TunetNetworkStatusDto, create_sdk_runtime,
            create_sdk_runtime_with_identity_persistence,
        },
        thos::{ThosPhaseStepsDto, ThosServicesDto, ThosTaskListDto},
    },
    auth::{
        AccountAuthState, AccountAuthStatus, AuthDomain, AuthStatus, SecondFactorMethod,
        SelfServiceLoginPhase,
    },
    calendar_api::{AcademicTerm, LearnTermCalendar, SchoolCalendarImage, SchoolCalendarQuery},
    campus_card_api::{
        CampusCardAccount, CampusCardInteraction, CampusCardPasswordRequest,
        CampusCardTransactionRange, CampusCardTransactions,
    },
    classrooms_api::{
        BuildingRef, ClassroomAvailability, ClassroomBuilding, ClassroomBuildings,
        ClassroomRoomAvailability, ClassroomSlotStatus, ClassroomWeek, ClassroomWeekSelection,
        dates_are_monday_first, safe_label as safe_classroom_label,
    },
    electricity_api::{ElectricityPaymentHistory, ElectricityRemainder},
    error::{Error, ErrorCode, Service},
    learn_api::{
        Course, CourseAnnouncement, CourseAnnouncements, CourseCatalog, CourseDiscussion,
        CourseDiscussions, CourseFile, CourseFileCategories, CourseFileCategory, CourseFileRef,
        CourseFiles, CourseRef, Homework, HomeworkAttachment, HomeworkAttachmentKind,
        HomeworkDetail, HomeworkList, HomeworkRef, HomeworkState, SavedCourseFile,
        parse_optional_time, parse_required_time, valid_selector,
    },
    library_api::{
        FloorRef, LibraryAvailability, LibraryDay, LibraryDirectory, LibraryFloor, LibraryFloors,
        LibraryPlace, LibraryRef, LibrarySeat, LibrarySection, LibrarySections,
        LibrarySocketAvailability, LibraryTimeWindow, LibraryTimeWindows, SeatRef, SeatWindowRef,
        SectionRef, merge_socket_statuses, safe_label, validate_areas,
    },
    network::{
        NetworkProfileId, NetworkProfileInput, NetworkProfilePassword, NetworkProfileStoragePolicy,
        NetworkProfileSummary, PortalAddressRegistration, PortalObservation, PreparedNetworkInput,
        StoredNetworkProfile,
    },
    network_profile_store::NetworkProfileStore,
    news::{
        ArticleDetail, ArticleRef, NewsArticle, NewsAttachment, NewsCatalog, NewsCatalogCoverage,
        NewsChannel, NewsChannelRef, NewsFavorites, NewsPage, NewsQuery, NewsQueryKind, NewsSource,
        NewsSourceRef, NewsSubscription, NewsSubscriptionRef, NewsSubscriptions,
    },
    read::{CacheFreshness, IncompleteReason, ReadCoverage, ReadMetadata, ReadResult, ReadSource},
    registrar_api::{
        AcademicStage, CourseGrade, Exam, ExamReport, ExamWeekday, GradeReport, GradeReportKind,
        ScheduleEvent, ScheduleEventKind, SemesterSchedule,
    },
    self_service::{AccountProfile, DeviceRef, OnlineDevice, UsageBalance},
    service_hall::{
        PendingTasks, PhaseDetails, PhaseStep, PhaseStepItem, ServiceDirectory, ServiceEntry,
        ServiceHallReadPolicy, TaskView, WorkflowTask, WorkflowTaskList, WorkflowTaskRef,
    },
};

/// Cache-directory behavior for a client instance.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientCachePolicy {
    /// Create a private temporary directory and remove it when the client is
    /// dropped. Authenticated sessions are always memory-only.
    Ephemeral,
    /// Store service read caches in this host-selected private directory.
    Directory(PathBuf),
}

/// Academic-stage preference used for Identity login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoginStage {
    /// Let the verified Identity session select the applicable stage.
    Auto,
    /// Prefer undergraduate services.
    Undergraduate,
    /// Prefer graduate services.
    Graduate,
}

impl LoginStage {
    fn runtime_value(self) -> (Option<bool>, bool) {
        match self {
            Self::Auto => (None, false),
            Self::Undergraduate => (Some(false), true),
            Self::Graduate => (Some(true), true),
        }
    }
}

/// A one-shot Identity login request. Its password is never formatted or
/// returned by the SDK and is consumed by [`Client::login_identity`].
pub struct IdentityLoginRequest {
    username: String,
    password: String,
    stage: LoginStage,
    trust_device: bool,
}

impl IdentityLoginRequest {
    /// Creates a login request from user-entered credentials.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            stage: LoginStage::Auto,
            trust_device: false,
        }
    }

    /// Sets the academic stage preference for this login only.
    pub fn stage(mut self, stage: LoginStage) -> Self {
        self.stage = stage;
        self
    }

    /// Explicitly asks the school to trust this installation when supported.
    pub fn trust_device(mut self, trust: bool) -> Self {
        self.trust_device = trust;
        self
    }
}

impl fmt::Debug for IdentityLoginRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentityLoginRequest")
            .field("username_present", &!self.username.trim().is_empty())
            .field("password_present", &!self.password.is_empty())
            .field("stage", &self.stage)
            .field("trust_device", &self.trust_device)
            .finish()
    }
}

impl Drop for IdentityLoginRequest {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

/// Result of an explicit Identity login or second-factor step.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentityLoginOutcome {
    /// Identity authentication and its current session proof completed.
    Authenticated(AccountAuthStatus),
    /// The school requires the caller to request or submit another factor.
    NeedsInteraction {
        methods: Vec<SecondFactorMethod>,
        masked_phone: Option<String>,
    },
}

/// A validated image challenge for the independent SelfService account.
#[derive(Clone, PartialEq, Eq)]
pub struct SelfServiceCaptcha {
    content_type: String,
    bytes: Vec<u8>,
}

impl SelfServiceCaptcha {
    /// Returns the validated media type for rendering the image.
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// Returns the one-time captcha image bytes for the explicit login UI.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for SelfServiceCaptcha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelfServiceCaptcha")
            .field("content_type", &self.content_type)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

/// Result of an explicit SelfService captcha login submission.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelfServiceLoginOutcome {
    /// The independently named SelfService account was verified.
    Authenticated(AccountAuthStatus),
    /// The login flow remains active and requires another explicit step.
    NeedsInteraction,
}

/// The single runtime-backed SDK client.
pub struct Client {
    runtime: CampusRuntime,
    cache_root: PathBuf,
    remove_cache_on_drop: bool,
    instance_id: uuid::Uuid,
    news_filter_generation: u64,
    news_subscription_generation: u64,
    learn_course_generation: u64,
    learn_homework_generation: u64,
    learn_file_generation: u64,
    service_hall_phase_generation: u64,
    library_directory_generation: u64,
    library_floor_generation: u64,
    library_section_generation: u64,
    classroom_generation: u64,
    network_profiles: NetworkProfileStore,
}

/// Controls whether the Identity-bound shared cookie snapshot can be restored
/// by a later Client created from the same private directory.
///
/// The default is memory-only. `EncryptedDirectory` is an explicit opt-in to
/// the engine's bounded, device-bound encrypted snapshot. Its encryption key
/// is stored beside the snapshot, so this is not an operating-system keychain
/// and should not be treated as equivalent to one. The configured directory
/// must also be selected as this Client's persistent cache directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdentitySessionStoragePolicy {
    /// Keep Identity and service cookies only in memory.
    MemoryOnly,
    /// Keep an encrypted, device-bound Identity snapshot in this directory.
    EncryptedDirectory(PathBuf),
}

impl Default for IdentitySessionStoragePolicy {
    fn default() -> Self {
        Self::MemoryOnly
    }
}

/// Builds a client with one Rust runtime and no restored account sessions by
/// default.
pub struct ClientBuilder {
    cache_policy: ClientCachePolicy,
    network_profile_storage: NetworkProfileStoragePolicy,
    identity_session_storage: IdentitySessionStoragePolicy,
}

impl ClientBuilder {
    /// Selects where non-credential business read caches are stored.
    pub fn cache_policy(mut self, policy: ClientCachePolicy) -> Self {
        self.cache_policy = policy;
        self
    }

    /// Selects an explicit local profile storage policy. This is independent
    /// of account/session persistence and defaults to memory-only.
    pub fn network_profile_storage(mut self, policy: NetworkProfileStoragePolicy) -> Self {
        self.network_profile_storage = policy;
        self
    }

    /// Selects explicit persistence for the Identity-bound shared cookie
    /// snapshot. The directory must match [`ClientCachePolicy::Directory`].
    /// This never persists account passwords or marks restored sessions as
    /// authenticated; the caller must invoke the explicit Identity
    /// revalidation operation after process restart.
    pub fn identity_session_storage(mut self, policy: IdentitySessionStoragePolicy) -> Self {
        self.identity_session_storage = policy;
        self
    }

    /// Builds one independent client instance. This does not log in or send
    /// network requests.
    pub fn build(self) -> Result<Client, Error> {
        let (cache_root, remove_cache_on_drop) = match self.cache_policy {
            ClientCachePolicy::Ephemeral => {
                let root = std::env::temp_dir()
                    .join("tsinghua-kit")
                    .join(uuid::Uuid::new_v4().to_string());
                fs::create_dir_all(&root)
                    .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
                set_private_directory_permissions(&root)?;
                (root, true)
            }
            ClientCachePolicy::Directory(root) => {
                if !root.is_absolute() {
                    return Err(Error::new(Service::Local, ErrorCode::InvalidInput));
                }
                fs::create_dir_all(&root)
                    .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
                (root, false)
            }
        };
        let runtime = match self.identity_session_storage {
            IdentitySessionStoragePolicy::MemoryOnly => {
                create_sdk_runtime(&cache_root.join("cache.json"), cache_root.clone(), None)?
            }
            IdentitySessionStoragePolicy::EncryptedDirectory(session_root) => {
                if !session_root.is_absolute()
                    || session_root
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(Error::new(Service::Local, ErrorCode::InvalidInput));
                }
                fs::create_dir_all(&session_root)
                    .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
                set_private_directory_permissions(&session_root)?;
                let canonical_session_root = fs::canonicalize(&session_root)
                    .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
                let canonical_cache_root = fs::canonicalize(&cache_root)
                    .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
                if canonical_session_root != canonical_cache_root {
                    return Err(Error::new(Service::Local, ErrorCode::InvalidInput));
                }
                create_sdk_runtime_with_identity_persistence(
                    &cache_root.join("cache.json"),
                    cache_root.clone(),
                )?
            }
        };
        let network_profiles = NetworkProfileStore::open(self.network_profile_storage)?;
        Ok(Client {
            runtime,
            cache_root,
            remove_cache_on_drop,
            instance_id: uuid::Uuid::new_v4(),
            news_filter_generation: 0,
            news_subscription_generation: 0,
            learn_course_generation: 0,
            learn_homework_generation: 0,
            learn_file_generation: 0,
            service_hall_phase_generation: 0,
            library_directory_generation: 0,
            library_floor_generation: 0,
            library_section_generation: 0,
            classroom_generation: 0,
            network_profiles,
        })
    }
}

impl Client {
    /// Starts building a client with isolated, memory-only authentication.
    pub fn builder() -> ClientBuilder {
        ClientBuilder {
            cache_policy: ClientCachePolicy::Ephemeral,
            network_profile_storage: NetworkProfileStoragePolicy::MemoryOnly,
            identity_session_storage: IdentitySessionStoragePolicy::MemoryOnly,
        }
    }

    /// Returns the Identity and SelfService status owned by this client.
    pub fn auth_status(&self) -> AuthStatus {
        self.runtime.auth_status()
    }

    /// Revalidates a restored Identity cookie snapshot after an explicit
    /// caller request. This is a status-only no-op for clients without a
    /// restored session and never submits a saved password.
    pub async fn revalidate_identity_session(&mut self) -> Result<AuthStatus, Error> {
        if self.runtime.auth_status().identity().state()
            != crate::auth::AccountAuthState::RestoredUnverified
        {
            return Ok(self.runtime.auth_status());
        }
        self.invalidate_learn_references();
        self.invalidate_service_hall_references();
        self.invalidate_library_references();
        self.invalidate_classroom_references();
        self.runtime.resume_restored_session().await.map_err(|_| {
            Error::new(
                Service::Auth(AuthDomain::Identity),
                ErrorCode::OutcomeUnconfirmed,
            )
        })?;
        Ok(self.runtime.auth_status())
    }

    /// Returns the currently active Identity or service-level second-factor
    /// challenge, including one that paused a downstream handoff.
    pub fn identity_interaction(&self) -> Result<Option<IdentityLoginOutcome>, Error> {
        let status = self.runtime.status();
        if matches!(
            status.state.as_str(),
            "requires_second_factor" | "authenticating"
        ) {
            outcome_from_status(&self.runtime, status).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Performs an explicit Identity login. Credentials are sent only through
    /// this client's shared Rust transport and are not persisted.
    pub async fn login_identity(
        &mut self,
        mut request: IdentityLoginRequest,
    ) -> Result<IdentityLoginOutcome, Error> {
        self.invalidate_learn_references();
        self.invalidate_service_hall_references();
        self.invalidate_library_references();
        self.invalidate_classroom_references();
        if request.username.trim().is_empty() || request.password.is_empty() {
            return Err(Error::new(
                Service::Auth(AuthDomain::Identity),
                ErrorCode::InvalidInput,
            ));
        }
        let (graduate, stage_explicit) = request.stage.runtime_value();
        let username = std::mem::take(&mut request.username);
        let password = std::mem::take(&mut request.password);
        let status = self
            .runtime
            .login(
                username,
                password,
                graduate,
                stage_explicit,
                request.trust_device,
                false,
            )
            .await
            .map_err(|_| {
                Error::new(
                    Service::Auth(AuthDomain::Identity),
                    ErrorCode::OutcomeUnconfirmed,
                )
            })?;
        outcome_from_status(&self.runtime, status)
    }

    /// Sends a requested second-factor code through the current authentication
    /// interaction. It never starts a new login if the challenge is absent.
    pub async fn send_second_factor_code(
        &mut self,
        method: SecondFactorMethod,
    ) -> Result<IdentityLoginOutcome, Error> {
        let status = self
            .runtime
            .send_second_factor_code(method.as_str().to_owned())
            .await
            .map_err(|_| {
                Error::new(
                    Service::Auth(AuthDomain::Identity),
                    ErrorCode::InteractionRequired,
                )
            })?;
        outcome_from_status(&self.runtime, status)
    }

    /// Completes the currently active second-factor interaction.
    pub async fn complete_second_factor(
        &mut self,
        method: SecondFactorMethod,
        verification_code: String,
    ) -> Result<IdentityLoginOutcome, Error> {
        self.invalidate_learn_references();
        self.invalidate_service_hall_references();
        self.invalidate_library_references();
        self.invalidate_classroom_references();
        let status = self
            .runtime
            .complete_second_factor(method.as_str().to_owned(), verification_code)
            .await
            .map_err(|_| {
                Error::new(
                    Service::Auth(AuthDomain::Identity),
                    ErrorCode::OutcomeUnconfirmed,
                )
            })?;
        outcome_from_status(&self.runtime, status)
    }

    /// Starts the independent SelfService login and obtains its captcha.
    /// This requires a proven Identity/WebVPN access path, but the username
    /// and password remain bound to the SelfService account slot.
    pub async fn start_self_service_login(
        &mut self,
        username: String,
        password: String,
    ) -> Result<SelfServiceCaptcha, Error> {
        if username.trim().is_empty() || password.is_empty() {
            return Err(Error::new(
                Service::Auth(AuthDomain::SelfService),
                ErrorCode::InvalidInput,
            ));
        }
        if let Some(error) = identity_access_error(&self.runtime) {
            return Err(error);
        }
        self.runtime.clear_usereg_failure_code();
        let captcha = self
            .runtime
            .start_usereg_login(username, password)
            .await
            .map_err(|_| {
                usereg_error(
                    &self.runtime,
                    Service::Auth(AuthDomain::SelfService),
                    ErrorCode::ServiceUnavailable,
                    true,
                )
            })?;
        Ok(SelfServiceCaptcha {
            content_type: captcha.content_type,
            bytes: captcha.bytes,
        })
    }

    /// Returns the current local phase of the SelfService captcha challenge.
    /// Invalid or expired challenges are cleared and reported as
    /// `RestartRequired`; this query performs no network I/O.
    pub fn self_service_login_phase(&mut self) -> SelfServiceLoginPhase {
        match self.runtime.usereg_login_phase().as_str() {
            "captcha_ready" => SelfServiceLoginPhase::CaptchaReady,
            "refresh_required" => SelfServiceLoginPhase::RefreshRequired,
            _ => SelfServiceLoginPhase::RestartRequired,
        }
    }

    /// Refreshes the current captcha explicitly. A failed refresh never causes
    /// the previous image to be submitted again.
    pub async fn refresh_self_service_captcha(&mut self) -> Result<SelfServiceCaptcha, Error> {
        self.runtime.clear_usereg_failure_code();
        let captcha = self.runtime.refresh_usereg_captcha().await.map_err(|_| {
            usereg_error(
                &self.runtime,
                Service::Auth(AuthDomain::SelfService),
                ErrorCode::InteractionRequired,
                true,
            )
        })?;
        Ok(SelfServiceCaptcha {
            content_type: captcha.content_type,
            bytes: captcha.bytes,
        })
    }

    /// Submits the human-entered captcha and optional service SMS code once.
    /// The captcha/session is never replayed automatically after an error.
    pub async fn complete_self_service_login(
        &mut self,
        captcha_answer: String,
        sms_code: Option<String>,
    ) -> Result<SelfServiceLoginOutcome, Error> {
        if captcha_answer.trim().is_empty() {
            return Err(Error::new(
                Service::Auth(AuthDomain::SelfService),
                ErrorCode::InvalidInput,
            ));
        }
        if let Some(error) = identity_access_error(&self.runtime) {
            return Err(error);
        }
        self.runtime.clear_usereg_failure_code();
        match self
            .runtime
            .complete_usereg_login(captcha_answer, sms_code)
            .await
        {
            Ok(_) => match self.runtime.auth_status().self_service().state() {
                crate::auth::AccountAuthState::Authenticated => {
                    Ok(SelfServiceLoginOutcome::Authenticated(
                        self.runtime.auth_status().self_service().clone(),
                    ))
                }
                crate::auth::AccountAuthState::NeedsInteraction
                | crate::auth::AccountAuthState::Authenticating => {
                    Ok(SelfServiceLoginOutcome::NeedsInteraction)
                }
                _ => Err(Error::new(
                    Service::Auth(AuthDomain::SelfService),
                    ErrorCode::InvalidResponse,
                )),
            },
            Err(_) if self.runtime.usereg_login_phase() == "refresh_required" => Err(Error::new(
                Service::Auth(AuthDomain::SelfService),
                ErrorCode::InvalidInput,
            )),
            Err(_) => Err(usereg_error(
                &self.runtime,
                Service::Auth(AuthDomain::SelfService),
                ErrorCode::OutcomeUnconfirmed,
                true,
            )),
        }
    }

    /// Cancels only an unfinished SelfService captcha flow.
    pub fn cancel_self_service_login(&mut self) {
        self.runtime.cancel_usereg_login();
    }

    /// Explicitly closes every authenticated service session in this client.
    pub fn logout_all(&mut self) -> Result<AuthStatus, Error> {
        self.invalidate_learn_references();
        self.invalidate_service_hall_references();
        self.invalidate_library_references();
        self.invalidate_classroom_references();
        self.runtime
            .logout()
            .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
        Ok(self.runtime.auth_status())
    }

    /// Logs out Identity and all service proofs derived from it. A selected
    /// SelfService account remains visible as expired because its shared
    /// Identity/WebVPN access route is no longer available.
    pub fn logout_identity(&mut self) -> Result<AuthStatus, Error> {
        self.invalidate_learn_references();
        self.invalidate_service_hall_references();
        self.invalidate_library_references();
        self.invalidate_classroom_references();
        self.runtime
            .logout_identity()
            .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))
    }

    /// Logs out only the independent SelfService account. Identity and its
    /// shared access transport remain usable.
    pub fn logout_self_service(&mut self) -> AuthStatus {
        self.runtime.logout_self_service()
    }

    /// Borrows the read-only online service-hall API.
    pub fn service_hall(&mut self) -> ServiceHallClient<'_> {
        let owner = self.instance_id;
        let phase_generation = &mut self.service_hall_phase_generation;
        ServiceHallClient {
            runtime: &mut self.runtime,
            owner,
            phase_generation,
        }
    }

    /// Borrows Registrar schedule, grade, and exam reads from this runtime.
    pub fn registrar(&mut self) -> RegistrarClient<'_> {
        RegistrarClient {
            runtime: &mut self.runtime,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
        }
    }

    /// Borrows read-only library directory, opening-window, seat, and socket
    /// queries through this client's shared runtime and transport.
    pub fn library(&mut self) -> LibraryClient<'_> {
        LibraryClient {
            runtime: &mut self.runtime,
            owner: self.instance_id,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
            directory_generation: &mut self.library_directory_generation,
            floor_generation: &mut self.library_floor_generation,
            section_generation: &mut self.library_section_generation,
        }
    }

    /// Borrows read-only classroom directory and weekly availability reads.
    pub fn classrooms(&mut self) -> ClassroomsClient<'_> {
        ClassroomsClient {
            runtime: &mut self.runtime,
            owner: self.instance_id,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
            generation: &mut self.classroom_generation,
        }
    }

    /// Borrows read-only campus-card account and transaction operations.
    pub fn campus_card(&mut self) -> CampusCardClient<'_> {
        CampusCardClient {
            runtime: &mut self.runtime,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
        }
    }

    /// Borrows read-only electricity remainder and payment-history reads.
    pub fn electricity(&mut self) -> ElectricityClient<'_> {
        ElectricityClient {
            runtime: &mut self.runtime,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
        }
    }

    /// Borrows school-wide and academic-term calendar reads.
    pub fn calendar(&mut self) -> CalendarClient<'_> {
        CalendarClient {
            runtime: &mut self.runtime,
        }
    }

    /// Borrows the read-only INFO news API. Article references are scoped to
    /// this client and its current link snapshot.
    pub fn news(&mut self) -> NewsClient<'_> {
        let client_id = self.instance_id;
        NewsClient {
            runtime: &mut self.runtime,
            client_id,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
            filter_generation: &mut self.news_filter_generation,
            subscription_generation: &mut self.news_subscription_generation,
        }
    }

    /// Borrows the Learn course and student-assignment read API. References
    /// returned by this facade are scoped to this client and its latest reads.
    pub fn learn(&mut self) -> LearnClient<'_> {
        let client_id = self.instance_id;
        LearnClient {
            runtime: &mut self.runtime,
            client_id,
            cache_source: if self.remove_cache_on_drop {
                ReadSource::ClientCache
            } else {
                ReadSource::PersistentCache
            },
            course_generation: &mut self.learn_course_generation,
            homework_generation: &mut self.learn_homework_generation,
            file_generation: &mut self.learn_file_generation,
        }
    }

    /// Borrows the read-only API for the independently authenticated
    /// SelfService account.
    pub fn self_service(&mut self) -> SelfServiceClient<'_> {
        let client_id = self.instance_id;
        SelfServiceClient {
            runtime: &mut self.runtime,
            client_id,
        }
    }

    /// Borrows local campus-network observations. Portal status does not
    /// create an Auth session and cannot identify Tsinghua Secure by itself.
    pub fn network(&mut self) -> NetworkClient<'_> {
        let owner = self.instance_id;
        NetworkClient {
            runtime: &mut self.runtime,
            owner,
            profiles: &mut self.network_profiles,
        }
    }

    fn invalidate_learn_references(&mut self) {
        self.learn_course_generation = self.learn_course_generation.wrapping_add(1);
        self.learn_homework_generation = self.learn_homework_generation.wrapping_add(1);
        self.learn_file_generation = self.learn_file_generation.wrapping_add(1);
    }

    fn invalidate_service_hall_references(&mut self) {
        self.service_hall_phase_generation = self.service_hall_phase_generation.wrapping_add(1);
    }

    fn invalidate_library_references(&mut self) {
        self.library_directory_generation = self.library_directory_generation.wrapping_add(1);
        self.library_floor_generation = self.library_floor_generation.wrapping_add(1);
        self.library_section_generation = self.library_section_generation.wrapping_add(1);
    }

    fn invalidate_classroom_references(&mut self) {
        self.classroom_generation = self.classroom_generation.wrapping_add(1);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.remove_cache_on_drop {
            let _ = fs::remove_dir_all(&self.cache_root);
        }
    }
}

/// Online service-hall operations borrowed from one client runtime.
pub struct ServiceHallClient<'client> {
    runtime: &'client mut CampusRuntime,
    owner: uuid::Uuid,
    phase_generation: &'client mut u64,
}

impl ServiceHallClient<'_> {
    /// Reads the pending workflow list according to the requested cache policy.
    pub async fn pending(
        &mut self,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<PendingTasks>, Error> {
        match policy {
            ServiceHallReadPolicy::CacheOnly => self.runtime.cached_thos_pending(),
            ServiceHallReadPolicy::PreferFreshCache => self.runtime.read_thos_pending(false).await,
            ServiceHallReadPolicy::Refresh => self.runtime.read_thos_pending(true).await,
        }
    }

    /// Reads the complete read-only service catalogue.
    pub async fn services(
        &mut self,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<ServiceDirectory>, Error> {
        let dto = match policy {
            ServiceHallReadPolicy::CacheOnly => self.runtime.cached_thos_services()?,
            ServiceHallReadPolicy::PreferFreshCache => self
                .runtime
                .load_thos_services(false)
                .await
                .map_err(|_| service_hall_failure(self.runtime))?,
            ServiceHallReadPolicy::Refresh => self
                .runtime
                .load_thos_services(true)
                .await
                .map_err(|_| service_hall_failure(self.runtime))?,
        };
        map_service_hall_directory(dto)
    }

    /// Reads one of the fixed read-only task views.
    pub async fn tasks(
        &mut self,
        view: TaskView,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<WorkflowTaskList>, Error> {
        if view == TaskView::Phases {
            let fresh_cache = self
                .runtime
                .thos_task_list_cache_is_fresh(view.runtime_key());
            if matches!(policy, ServiceHallReadPolicy::Refresh)
                || (matches!(policy, ServiceHallReadPolicy::PreferFreshCache) && !fresh_cache)
            {
                *self.phase_generation = self.phase_generation.wrapping_add(1);
            }
        }
        let dto = match policy {
            ServiceHallReadPolicy::CacheOnly => {
                self.runtime.cached_thos_task_list(view.runtime_key())?
            }
            ServiceHallReadPolicy::PreferFreshCache => self
                .runtime
                .load_thos_task_list(view.runtime_key().to_owned(), false)
                .await
                .map_err(|_| service_hall_failure(self.runtime))?,
            ServiceHallReadPolicy::Refresh => self
                .runtime
                .load_thos_task_list(view.runtime_key().to_owned(), true)
                .await
                .map_err(|_| service_hall_failure(self.runtime))?,
        };
        map_service_hall_tasks(dto, view, self.owner, *self.phase_generation)
    }

    /// Reads detailed stages for a workflow selected from this Client's
    /// current complete phased-task list.
    pub async fn phase_details(
        &mut self,
        reference: &WorkflowTaskRef,
        policy: ServiceHallReadPolicy,
    ) -> Result<ReadResult<PhaseDetails>, Error> {
        if !reference.belongs_to(self.owner, *self.phase_generation) {
            return Err(Error::new(Service::ServiceHall, ErrorCode::ContextMismatch));
        }
        if self.runtime.auth_status().identity().state() != AccountAuthState::Authenticated {
            return Err(service_hall_failure(self.runtime));
        }
        if !self
            .runtime
            .thos_task_list_cache_is_fresh(TaskView::Phases.runtime_key())
        {
            return Err(Error::new(Service::ServiceHall, ErrorCode::ContextMismatch));
        }
        let dto = match policy {
            ServiceHallReadPolicy::CacheOnly => {
                self.runtime.cached_thos_phase_steps(reference.selector())?
            }
            ServiceHallReadPolicy::PreferFreshCache => self
                .runtime
                .load_thos_phase_steps(reference.selector().to_owned(), false)
                .await
                .map_err(|_| service_hall_failure(self.runtime))?,
            ServiceHallReadPolicy::Refresh => self
                .runtime
                .load_thos_phase_steps(reference.selector().to_owned(), true)
                .await
                .map_err(|_| service_hall_failure(self.runtime))?,
        };
        map_service_hall_phase_details(dto, reference)
    }
}

/// Read-only classroom operations backed by this client's shared runtime.
pub struct ClassroomsClient<'client> {
    runtime: &'client mut CampusRuntime,
    owner: uuid::Uuid,
    cache_source: ReadSource,
    generation: &'client mut u64,
}

impl ClassroomsClient<'_> {
    /// Reads the validated building directory with its actual cache metadata.
    pub async fn buildings(&mut self) -> Result<ReadResult<ClassroomBuildings>, Error> {
        *self.generation = self.generation.wrapping_add(1);
        let generation = *self.generation;
        let dto = self
            .runtime
            .load_classroom_buildings_result()
            .await
            .map_err(|_| classrooms_failure(self.runtime))?;
        let metadata = cached_read_metadata(
            Service::Classrooms,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let buildings = map_classroom_building_records(dto.buildings, self.owner, generation)?;
        Ok(ReadResult::new(
            ClassroomBuildings::new(buildings),
            metadata,
        ))
    }

    /// Reads one building's verified weekly matrix.
    pub async fn availability(
        &mut self,
        building: &BuildingRef,
        week: ClassroomWeekSelection,
    ) -> Result<ReadResult<ClassroomAvailability>, Error> {
        if !building.belongs_to(self.owner, *self.generation) {
            return Err(Error::new(Service::Classrooms, ErrorCode::ContextMismatch));
        }
        let requested_week = match week {
            ClassroomWeekSelection::BuildingDefault => building.default_week,
            ClassroomWeekSelection::Week(week) => week,
        };
        let dto = self
            .runtime
            .load_classroom_state(building.index, requested_week.get())
            .await
            .map_err(|_| classrooms_failure(self.runtime))?;
        let availability = map_classroom_availability(dto, building.clone(), requested_week)?;
        Ok(ReadResult::new(availability, live_read_metadata()))
    }
}

/// Read-only campus-card reads and its explicit one-shot SSO password step.
/// The card password is a target-service interaction, not an Auth account.
pub struct CampusCardClient<'client> {
    runtime: &'client mut CampusRuntime,
    cache_source: ReadSource,
}

impl CampusCardClient<'_> {
    /// Returns the current card-specific interaction, if a read explicitly
    /// requested one. A card two-factor challenge is exposed through
    /// `client.auth().identity().interaction()` instead.
    pub fn pending_interaction(&self) -> Option<CampusCardInteraction> {
        self.runtime
            .has_campus_card_password_challenge()
            .then_some(CampusCardInteraction::PasswordRequired)
    }

    /// Reads the current card account and preserves verified cache metadata.
    /// This may return `InteractionRequired`; inspect [`Self::pending_interaction`]
    /// before asking the caller for the card-service password.
    pub async fn account(&mut self) -> Result<ReadResult<CampusCardAccount>, Error> {
        let dto = self
            .runtime
            .load_campus_card_account_result()
            .await
            .map_err(|_| campus_card_failure(self.runtime))?;
        let metadata = cached_read_metadata(
            Service::CampusCard,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let account = CampusCardAccount::from_runtime(dto.account)?;
        Ok(ReadResult::new(account, metadata))
    }

    /// Reads a complete, at-most-31-day transaction range. Transaction IDs
    /// remain inside Rust and are used only to verify that pages contain no
    /// duplicate records.
    pub async fn transactions(
        &mut self,
        range: CampusCardTransactionRange,
    ) -> Result<ReadResult<CampusCardTransactions>, Error> {
        let dto = self
            .runtime
            .load_campus_card_transactions_result(
                range.start_date().to_owned(),
                range.end_date().to_owned(),
                range.transaction_type().wire_value().to_owned(),
            )
            .await
            .map_err(|_| campus_card_failure(self.runtime))?;
        let metadata = cached_read_metadata(
            Service::CampusCard,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let transactions = CampusCardTransactions::from_runtime(dto, range)?;
        Ok(ReadResult::new(transactions, metadata))
    }

    /// Submits the one-shot target password after this Client's previous
    /// card read reported `PasswordRequired`. The Rust runtime consumes the
    /// challenge before I/O and never retries an ambiguous submission.
    pub async fn submit_password(
        &mut self,
        mut request: CampusCardPasswordRequest,
    ) -> Result<(), Error> {
        if !request.is_valid() {
            return Err(Error::new(Service::CampusCard, ErrorCode::InvalidInput));
        }
        if !self.runtime.has_campus_card_password_challenge() {
            return Err(Error::new(
                Service::CampusCard,
                ErrorCode::InteractionRequired,
            ));
        }
        self.runtime
            .submit_campus_card_password(request.take_password())
            .await
            .map_err(|_| campus_card_failure(self.runtime))
    }

    /// Declines the outstanding target-password prompt without sending a
    /// request. Returns whether a challenge was cancelled.
    pub fn cancel_password_challenge(&mut self) -> bool {
        if !self.runtime.has_campus_card_password_challenge() {
            return false;
        }
        self.runtime.cancel_campus_card_password_challenge();
        true
    }
}

/// Read-only dorm-electricity queries with account-bound cache provenance.
pub struct ElectricityClient<'client> {
    runtime: &'client mut CampusRuntime,
    cache_source: ReadSource,
}

impl ElectricityClient<'_> {
    /// Reads the electricity remainder, retaining the source's numeric value
    /// without inventing an undocumented unit.
    pub async fn remainder(&mut self) -> Result<ReadResult<ElectricityRemainder>, Error> {
        let dto: ElectricityRemainderResultDto = self
            .runtime
            .load_electricity_remainder_result()
            .await
            .map_err(|_| electricity_failure(self.runtime))?;
        let metadata = cached_read_metadata(
            Service::Electricity,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let value = ElectricityRemainder::from_runtime(ElectricityRemainderDto {
            remainder: dto.remainder,
            update_time: dto.update_time,
        })?;
        Ok(ReadResult::new(value, metadata))
    }

    /// Reads the complete, structurally validated payment-history response.
    /// The legacy service's unlabeled columns are kept inside Rust.
    pub async fn payment_history(
        &mut self,
    ) -> Result<ReadResult<ElectricityPaymentHistory>, Error> {
        let dto: ElectricityPaymentHistoryResultDto = self
            .runtime
            .load_electricity_payment_history_result()
            .await
            .map_err(|_| electricity_failure(self.runtime))?;
        let metadata = cached_read_metadata(
            Service::Electricity,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let value = ElectricityPaymentHistory::from_runtime(ElectricityPaymentHistoryDto {
            records: dto.records,
            empty: dto.empty,
        })?;
        Ok(ReadResult::new(value, metadata))
    }
}

fn campus_card_failure(runtime: &CampusRuntime) -> Error {
    let code = if runtime.has_campus_card_password_challenge()
        || runtime.has_campus_card_second_factor_challenge()
    {
        ErrorCode::InteractionRequired
    } else {
        match runtime.auth_status().identity().state() {
            AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
                ErrorCode::SessionRequired
            }
            AccountAuthState::Expired => ErrorCode::SessionExpired,
            AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
            AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
            AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
        }
    };
    Error::new(Service::CampusCard, code)
}

fn electricity_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Electricity, code)
}

fn classrooms_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Classrooms, code)
}

fn map_classroom_building_records(
    records: Vec<crate::api::runtime::ClassroomBuildingDto>,
    owner: uuid::Uuid,
    generation: u64,
) -> Result<Vec<ClassroomBuilding>, Error> {
    if records.len() > 4096 {
        return Err(Error::new(Service::Classrooms, ErrorCode::InvalidResponse));
    }
    let mut names = std::collections::HashSet::new();
    records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            let Some(default_week) = ClassroomWeek::new(record.week_number) else {
                return Err(Error::new(Service::Classrooms, ErrorCode::InvalidResponse));
            };
            if !safe_classroom_label(&record.name) || !names.insert(record.name.clone()) {
                return Err(Error::new(Service::Classrooms, ErrorCode::InvalidResponse));
            }
            let index = u32::try_from(index)
                .map_err(|_| Error::new(Service::Classrooms, ErrorCode::InvalidResponse))?;
            Ok(ClassroomBuilding::new(
                Some(BuildingRef::new(owner, generation, index, default_week)),
                record.name,
                Some(default_week),
            ))
        })
        .collect()
}

fn map_classroom_availability(
    dto: ClassroomStateResultDto,
    building: BuildingRef,
    requested_week: ClassroomWeek,
) -> Result<ClassroomAvailability, Error> {
    let invalid = || Error::new(Service::Classrooms, ErrorCode::InvalidResponse);
    if dto.valid_week_numbers.is_empty() || dto.valid_week_numbers.len() > 100 {
        return Err(invalid());
    }
    let mut seen_weeks = std::collections::HashSet::new();
    let valid_weeks = dto
        .valid_week_numbers
        .into_iter()
        .map(|week| {
            let week = ClassroomWeek::new(week).ok_or_else(invalid)?;
            if !seen_weeks.insert(week) {
                return Err(invalid());
            }
            Ok(week)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if dto.current_week_number != requested_week.get() || !valid_weeks.contains(&requested_week) {
        return Err(invalid());
    }
    let dates = dto
        .dates_of_current_week
        .into_iter()
        .map(|value| chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(|_| invalid()))
        .collect::<Result<Vec<_>, _>>()?;
    let week_dates: [chrono::NaiveDate; 7] = dates.try_into().map_err(|_| invalid())?;
    if !dates_are_monday_first(&week_dates) || dto.classroom_states.len() > 4096 {
        return Err(invalid());
    }
    let mut names = std::collections::HashSet::new();
    let rooms = dto
        .classroom_states
        .into_iter()
        .map(|room| {
            if !safe_classroom_label(&room.name)
                || !names.insert(room.name.clone())
                || room.statuses.len() != 42
            {
                return Err(invalid());
            }
            let slots = room
                .statuses
                .iter()
                .map(|status| ClassroomSlotStatus::from_runtime(status).ok_or_else(invalid))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ClassroomRoomAvailability::new(room.name, slots))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ClassroomAvailability::new(
        building,
        requested_week,
        valid_weeks,
        week_dates,
        rooms,
    ))
}

/// Read-only library operations backed by this client's shared runtime.
///
/// The root directory and opening windows use the Runtime's account-bound
/// cache behavior. Floors, sections, seats, and socket states require live
/// reads. The public API accepts only context-bound references, never raw
/// service identifiers.
pub struct LibraryClient<'client> {
    runtime: &'client mut CampusRuntime,
    owner: uuid::Uuid,
    cache_source: ReadSource,
    directory_generation: &'client mut u64,
    floor_generation: &'client mut u64,
    section_generation: &'client mut u64,
}

impl LibraryClient<'_> {
    /// Reads the root library directory and preserves its cache provenance.
    pub async fn directory(&mut self) -> Result<ReadResult<LibraryDirectory>, Error> {
        *self.directory_generation = self.directory_generation.wrapping_add(1);
        *self.floor_generation = self.floor_generation.wrapping_add(1);
        *self.section_generation = self.section_generation.wrapping_add(1);
        let generation = *self.directory_generation;
        let dto = self
            .runtime
            .load_library_area_tree_result()
            .await
            .map_err(|_| library_failure(self.runtime))?;
        let metadata = cached_read_metadata(
            Service::Library,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        if !validate_areas(&dto.areas) {
            return Err(invalid_library_response());
        }
        let libraries = dto
            .areas
            .into_iter()
            .map(|area| {
                LibraryPlace::new(
                    (area.is_valid != Some(false))
                        .then(|| LibraryRef::new(self.owner, generation, area.id)),
                    area.name,
                    area.english_name,
                    area.is_valid,
                    area.total_count,
                    area.available_count,
                )
            })
            .collect();
        Ok(ReadResult::new(LibraryDirectory::new(libraries), metadata))
    }

    /// Reads floors for a library returned by this Client's latest directory.
    /// The Runtime rechecks the root against the current verified directory.
    pub async fn floors(
        &mut self,
        library: &LibraryRef,
    ) -> Result<ReadResult<LibraryFloors>, Error> {
        if !library.belongs_to(self.owner, *self.directory_generation) {
            return Err(Error::new(Service::Library, ErrorCode::ContextMismatch));
        }
        *self.floor_generation = self.floor_generation.wrapping_add(1);
        *self.section_generation = self.section_generation.wrapping_add(1);
        let generation = *self.floor_generation;
        let dto = self
            .runtime
            .load_library_floors(library.id)
            .await
            .map_err(|_| library_failure(self.runtime))?;
        if !validate_areas(&dto.areas) {
            return Err(invalid_library_response());
        }
        let floors = dto
            .areas
            .into_iter()
            .map(|area| {
                LibraryFloor::new(
                    (area.is_valid != Some(false))
                        .then(|| FloorRef::new(library.clone(), generation, area.id)),
                    area.name,
                    area.is_valid,
                    area.total_count,
                    area.available_count,
                )
            })
            .collect();
        Ok(ReadResult::new(
            LibraryFloors::new(floors),
            live_read_metadata(),
        ))
    }

    /// Reads sections for today or tomorrow from a floor returned by this
    /// Client's latest floor read.
    pub async fn sections(
        &mut self,
        floor: &FloorRef,
        day: LibraryDay,
    ) -> Result<ReadResult<LibrarySections>, Error> {
        if !floor.belongs_to(
            self.owner,
            *self.directory_generation,
            *self.floor_generation,
        ) {
            return Err(Error::new(Service::Library, ErrorCode::ContextMismatch));
        }
        let Some(selected_day) = day.date_at(Utc::now()) else {
            return Err(Error::new(Service::Library, ErrorCode::InvalidInput));
        };
        *self.section_generation = self.section_generation.wrapping_add(1);
        let generation = *self.section_generation;
        let dto = self
            .runtime
            .load_library_sections_for_campus_date(
                floor.id,
                selected_day.format("%Y-%m-%d").to_string(),
            )
            .await
            .map_err(|_| library_failure(self.runtime))?;
        if !validate_areas(&dto.areas) {
            return Err(invalid_library_response());
        }
        let sections = dto
            .areas
            .into_iter()
            .map(|area| {
                LibrarySection::new(
                    (area.is_valid != Some(false))
                        .then(|| SectionRef::new(floor.clone(), generation, area.id, selected_day)),
                    area.name,
                    area.is_valid,
                    area.total_count,
                    area.available_count,
                )
            })
            .collect();
        Ok(ReadResult::new(
            LibrarySections::new(selected_day, sections),
            live_read_metadata(),
        ))
    }

    /// Reads the selected section's opening windows for its bound campus day.
    pub async fn time_windows(
        &mut self,
        section: &SectionRef,
    ) -> Result<ReadResult<LibraryTimeWindows>, Error> {
        if !self.section_is_current(section) {
            return Err(Error::new(Service::Library, ErrorCode::ContextMismatch));
        }
        let dto = self
            .runtime
            .load_library_day_segments_for_campus_date_result(
                section.id,
                section.day.format("%Y-%m-%d").to_string(),
            )
            .await
            .map_err(|_| library_failure(self.runtime))?;
        if dto.section_id != section.id {
            return Err(invalid_library_response());
        }
        let metadata = cached_read_metadata(
            Service::Library,
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let mut seen = std::collections::HashSet::new();
        let windows = dto
            .segments
            .into_iter()
            .map(|segment| {
                if segment.day != section.day.format("%Y-%m-%d").to_string()
                    || segment.id == 0
                    || !safe_label(&segment.start_time)
                    || !safe_label(&segment.end_time)
                    || !seen.insert(segment.id)
                {
                    return Err(invalid_library_response());
                }
                let start_time = parse_library_time(&segment.start_time)?;
                let end_time = parse_library_time(&segment.end_time)?;
                if start_time >= end_time {
                    return Err(invalid_library_response());
                }
                Ok(LibraryTimeWindow::new(SeatWindowRef::new(
                    section.clone(),
                    segment.id,
                    start_time,
                    end_time,
                )))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReadResult::new(
            LibraryTimeWindows::new(section.clone(), windows),
            metadata,
        ))
    }

    /// Reads live seat availability using a verified opening-window reference.
    pub async fn seats(
        &mut self,
        window: &SeatWindowRef,
    ) -> Result<ReadResult<LibraryAvailability>, Error> {
        if !self.section_is_current(&window.section) {
            return Err(Error::new(Service::Library, ErrorCode::ContextMismatch));
        }
        let dto = self
            .runtime
            .load_library_seats(
                window.section.id,
                window.segment_id,
                window.section.day.format("%Y-%m-%d").to_string(),
                window.start_time.format("%H:%M").to_string(),
                window.end_time.format("%H:%M").to_string(),
            )
            .await
            .map_err(|_| library_failure(self.runtime))?;
        let mut seen = std::collections::HashSet::new();
        let seats = dto
            .seats
            .into_iter()
            .map(|seat| {
                if seat.id == 0 || !safe_label(&seat.name) || !seen.insert(seat.id) {
                    return Err(invalid_library_response());
                }
                Ok(LibrarySeat::new(
                    SeatRef::new(window.section.clone(), seat.id),
                    seat.name,
                    seat.is_valid,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReadResult::new(
            LibraryAvailability::new(window.section.clone(), seats),
            live_read_metadata(),
        ))
    }

    /// Reads the separate socket service and joins its ids to the exact seat
    /// list returned by [`Self::seats`]. Missing records are `Unknown`; foreign
    /// and duplicate seat ids reject the response.
    pub async fn sockets(
        &mut self,
        availability: &LibraryAvailability,
    ) -> Result<ReadResult<LibrarySocketAvailability>, Error> {
        if !self.section_is_current(&availability.section) {
            return Err(Error::new(Service::Library, ErrorCode::ContextMismatch));
        }
        let dto = self
            .runtime
            .load_library_socket_status(availability.section.id)
            .await
            .map_err(|_| library_failure(self.runtime))?;
        Ok(ReadResult::new(
            merge_socket_statuses(availability, dto.records)?,
            live_read_metadata(),
        ))
    }

    fn section_is_current(&self, section: &SectionRef) -> bool {
        section.belongs_to(
            self.owner,
            *self.directory_generation,
            *self.floor_generation,
            *self.section_generation,
        )
    }
}

fn library_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Library, code)
}

fn invalid_library_response() -> Error {
    Error::new(Service::Library, ErrorCode::InvalidResponse)
}

fn cached_read_metadata(
    service: Service,
    generated_at: &str,
    source: &str,
    status: &str,
    has_refresh_error: bool,
    cache_source: ReadSource,
) -> Result<ReadMetadata, Error> {
    let invalid = || Error::new(service, ErrorCode::InvalidResponse);
    let observed_at = chrono::DateTime::parse_from_rfc3339(generated_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| invalid())?;
    if observed_at > Utc::now() + chrono::Duration::minutes(5) {
        return Err(invalid());
    }
    let (source, freshness, refresh_failure) = match (source, status, has_refresh_error) {
        ("live", "ready", false) => (ReadSource::Live, CacheFreshness::NotApplicable, None),
        ("cache", "ready", false) => (cache_source, CacheFreshness::Fresh, None),
        ("cache", "stale", true) => (
            cache_source,
            CacheFreshness::Stale,
            Some(ErrorCode::ServiceUnavailable),
        ),
        _ => return Err(invalid()),
    };
    Ok(ReadMetadata::new(
        source,
        freshness,
        observed_at,
        ReadCoverage::Complete,
        refresh_failure,
    ))
}

fn live_read_metadata() -> ReadMetadata {
    ReadMetadata::new(
        ReadSource::Live,
        CacheFreshness::NotApplicable,
        Utc::now(),
        ReadCoverage::Complete,
        None,
    )
}

fn parse_library_time(value: &str) -> Result<chrono::NaiveTime, Error> {
    chrono::NaiveTime::parse_from_str(value, "%H:%M")
        .or_else(|_| chrono::NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map_err(|_| invalid_library_response())
}

fn service_hall_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::ServiceHall, code)
}

fn invalid_service_hall_response() -> Error {
    Error::new(Service::ServiceHall, ErrorCode::InvalidResponse)
}

fn map_service_hall_directory(dto: ThosServicesDto) -> Result<ReadResult<ServiceDirectory>, Error> {
    let metadata =
        service_hall_metadata(&dto.generated_at, &dto.source, &dto.status, dto.complete)?;
    if dto.complete != dto.error.is_none()
        || (dto.complete && usize::try_from(dto.reported_total).ok() != Some(dto.items.len()))
        || usize::try_from(dto.reported_total).is_ok_and(|total| total < dto.items.len())
    {
        return Err(invalid_service_hall_response());
    }
    let items = dto
        .items
        .into_iter()
        .map(|item| {
            if item.id.is_empty() || item.name.is_empty() {
                return Err(invalid_service_hall_response());
            }
            Ok(ServiceEntry::new(
                item.name,
                item.department,
                item.kind,
                item.in_open_period,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadResult::new(
        ServiceDirectory::new(items, dto.reported_total, metadata.coverage()),
        metadata,
    ))
}

fn map_service_hall_tasks(
    dto: ThosTaskListDto,
    view: TaskView,
    owner: uuid::Uuid,
    generation: u64,
) -> Result<ReadResult<WorkflowTaskList>, Error> {
    let metadata =
        service_hall_metadata(&dto.generated_at, &dto.source, &dto.status, dto.complete)?;
    if dto.complete != dto.error.is_none()
        || dto.kind != view.runtime_key()
        || (dto.complete && usize::try_from(dto.reported_total).ok() != Some(dto.items.len()))
        || usize::try_from(dto.reported_total).is_ok_and(|total| total < dto.items.len())
    {
        return Err(invalid_service_hall_response());
    }
    let items = dto
        .items
        .into_iter()
        .map(|item| {
            if uuid::Uuid::parse_str(&item.id).is_err()
                || item.title.is_empty()
                || item.status.is_empty()
            {
                return Err(invalid_service_hall_response());
            }
            let reference = (view == TaskView::Phases
                && metadata.coverage() == ReadCoverage::Complete)
                .then(|| WorkflowTaskRef::new(owner, generation, view, item.id));
            Ok(WorkflowTask::new(
                reference,
                item.title,
                item.status,
                item.node,
                item.date,
                item.progress,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadResult::new(
        WorkflowTaskList::new(view, items, dto.reported_total, metadata.coverage()),
        metadata,
    ))
}

fn map_service_hall_phase_details(
    dto: ThosPhaseStepsDto,
    reference: &WorkflowTaskRef,
) -> Result<ReadResult<PhaseDetails>, Error> {
    if dto.task_id != reference.selector() || dto.status != "ready" {
        return Err(invalid_service_hall_response());
    }
    let metadata = service_hall_metadata(&dto.generated_at, &dto.source, &dto.status, true)?;
    let steps = dto
        .steps
        .into_iter()
        .map(|step| {
            if step.name.is_empty() {
                return Err(invalid_service_hall_response());
            }
            let items = step
                .items
                .into_iter()
                .map(|item| {
                    if item.name.is_empty() {
                        Err(invalid_service_hall_response())
                    } else {
                        Ok(PhaseStepItem::new(item.name, item.state))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PhaseStep::new(step.order, step.name, step.state, items))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadResult::new(
        PhaseDetails::new(reference.clone(), steps),
        metadata,
    ))
}

fn service_hall_metadata(
    generated_at: &str,
    source: &str,
    status: &str,
    complete: bool,
) -> Result<ReadMetadata, Error> {
    let invalid = invalid_service_hall_response;
    let observed_at = chrono::DateTime::parse_from_rfc3339(generated_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| invalid())?;
    let (source, freshness) = match source {
        "live" => (ReadSource::Live, CacheFreshness::NotApplicable),
        "cache" => (ReadSource::MemoryCache, CacheFreshness::Fresh),
        _ => return Err(invalid()),
    };
    let coverage = match (complete, status) {
        (true, "ready") => ReadCoverage::Complete,
        (false, "partial") => ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed),
        _ => return Err(invalid()),
    };
    Ok(ReadMetadata::new(
        source,
        freshness,
        observed_at,
        coverage,
        None,
    ))
}

/// Registrar reads through this client's shared Identity and service runtime.
pub struct RegistrarClient<'client> {
    runtime: &'client mut CampusRuntime,
    cache_source: ReadSource,
}

impl RegistrarClient<'_> {
    /// Reads the complete verified schedule for the Runtime-selected semester.
    /// The Runtime may return its validated cache when a live refresh is not
    /// available; source and freshness remain explicit in the result metadata.
    pub async fn semester_schedule(&mut self) -> Result<ReadResult<SemesterSchedule>, Error> {
        let dto = self
            .runtime
            .load_semester_schedule()
            .await
            .map_err(|_| registrar_failure(self.runtime))?;
        let metadata = registrar_metadata(
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        Ok(ReadResult::new(map_semester_schedule(dto)?, metadata))
    }

    /// Reads the current stage-specific course-grade report. The Runtime owns
    /// cache selection and fallback; this facade reports their provenance.
    pub async fn grades(&mut self) -> Result<ReadResult<GradeReport>, Error> {
        let dto = self
            .runtime
            .load_grades()
            .await
            .map_err(|_| registrar_failure(self.runtime))?;
        let metadata = registrar_optional_metadata(
            dto.generated_at.as_deref(),
            dto.source.as_deref(),
            dto.status.as_deref(),
            dto.error.is_some(),
            self.cache_source,
        )?;
        Ok(ReadResult::new(map_grade_report(dto)?, metadata))
    }

    /// Reads the complete stage-specific examination report. The Runtime uses
    /// its verified undergraduate page or graduate semester calendar and does
    /// not accept caller-supplied date filters.
    pub async fn exams(&mut self) -> Result<ReadResult<ExamReport>, Error> {
        // The legacy Runtime signature only permits empty dates. Keeping that
        // constraint here prevents arbitrary/unverified ranges reaching it.
        let dto = self
            .runtime
            .load_exams(String::new(), String::new())
            .await
            .map_err(|_| registrar_failure(self.runtime))?;
        let metadata = registrar_optional_metadata(
            dto.generated_at.as_deref(),
            dto.source.as_deref(),
            dto.status.as_deref(),
            dto.error.is_some(),
            self.cache_source,
        )?;
        let stage = academic_stage(&dto.stage)?;
        if usize::try_from(dto.exam_count).ok() != Some(dto.exams.len()) {
            return Err(invalid_registrar_response());
        }
        let exams = dto
            .exams
            .into_iter()
            .map(map_exam)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ReadResult::new(ExamReport::new(stage, exams), metadata))
    }
}

/// School calendar reads through the same runtime and transport as Auth.
pub struct CalendarClient<'client> {
    runtime: &'client mut CampusRuntime,
}

impl CalendarClient<'_> {
    /// Reads the current and following terms from the authenticated Learn
    /// session. This is live-only and does not substitute a course cache.
    pub async fn learn_terms(&mut self) -> Result<ReadResult<LearnTermCalendar>, Error> {
        let dto = self
            .runtime
            .load_learn_term_calendar()
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        let metadata = live_learn_metadata(&dto.source, &dto.status, &dto.generated_at)?;
        Ok(ReadResult::new(map_learn_term_calendar(dto)?, metadata))
    }

    /// Reads a verified published calendar image. The selection is typed and
    /// the Runtime fetches bytes through its shared throttled Rust transport.
    pub async fn school_calendar(
        &mut self,
        query: SchoolCalendarQuery,
    ) -> Result<ReadResult<SchoolCalendarImage>, Error> {
        let dto = self
            .runtime
            .load_school_calendar(
                query.year(),
                query.semester().as_str().to_owned(),
                query.language().as_str().to_owned(),
            )
            .await
            .map_err(|_| calendar_failure(self.runtime))?;
        let metadata = calendar_metadata(&dto.generated_at, &dto.source, &dto.status)?;
        Ok(ReadResult::new(map_school_calendar(dto, query)?, metadata))
    }
}

fn registrar_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Registrar, code)
}

fn registrar_metadata(
    generated_at: &str,
    source: &str,
    status: &str,
    has_refresh_error: bool,
    cache_source: ReadSource,
) -> Result<ReadMetadata, Error> {
    registrar_optional_metadata(
        Some(generated_at),
        Some(source),
        Some(status),
        has_refresh_error,
        cache_source,
    )
}

fn registrar_optional_metadata(
    generated_at: Option<&str>,
    source: Option<&str>,
    status: Option<&str>,
    has_refresh_error: bool,
    cache_source: ReadSource,
) -> Result<ReadMetadata, Error> {
    let invalid = || Error::new(Service::Registrar, ErrorCode::InvalidResponse);
    let observed_at = generated_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(invalid)?;
    let (source, freshness, refresh_failure) = match (source, status) {
        (Some("live"), Some("ready")) => (ReadSource::Live, CacheFreshness::NotApplicable, None),
        (Some("cache"), Some("ready")) => (cache_source, CacheFreshness::Fresh, None),
        (Some("cache"), Some("stale")) => (
            cache_source,
            CacheFreshness::Stale,
            has_refresh_error.then_some(ErrorCode::ServiceUnavailable),
        ),
        _ => return Err(invalid()),
    };
    Ok(ReadMetadata::new(
        source,
        freshness,
        observed_at,
        ReadCoverage::Complete,
        refresh_failure,
    ))
}

fn academic_stage(value: &str) -> Result<AcademicStage, Error> {
    match value {
        "undergraduate" => Ok(AcademicStage::Undergraduate),
        "graduate" => Ok(AcademicStage::Graduate),
        _ => Err(invalid_registrar_response()),
    }
}

fn map_semester_schedule(dto: CampusSemesterScheduleDto) -> Result<SemesterSchedule, Error> {
    let stage = academic_stage(&dto.stage)?;
    let first_day = parse_campus_date(&dto.first_day)?;
    let last_day = parse_campus_date(&dto.last_day)?;
    if dto.semester.trim().is_empty()
        || dto.semester.chars().any(char::is_control)
        || first_day > last_day
        || dto.week_count == 0
        || dto.current_week > dto.week_count
    {
        return Err(invalid_registrar_response());
    }
    let events = dto
        .events
        .into_iter()
        .map(map_schedule_event)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SemesterSchedule::new(
        dto.semester,
        stage,
        first_day,
        last_day,
        dto.week_count,
        dto.current_week,
        events,
    ))
}

fn map_grade_report(dto: CampusGradeReportDto) -> Result<GradeReport, Error> {
    let stage = academic_stage(&dto.stage)?;
    let kind = match dto.report_kind.as_deref() {
        Some("first_degree") => Some(GradeReportKind::FirstDegree),
        Some("second_degree") => Some(GradeReportKind::SecondDegree),
        Some("minor") => Some(GradeReportKind::Minor),
        None => None,
        Some(_) => return Err(invalid_registrar_response()),
    };
    if (stage == AcademicStage::Undergraduate) != kind.is_some() {
        return Err(invalid_registrar_response());
    }
    let courses = dto
        .courses
        .into_iter()
        .map(|course| {
            if !valid_required_text(&course.course_name)
                || !valid_text(&course.grade)
                || !valid_required_text(&course.semester)
                || !course.credit.is_finite()
                || course.credit < 0.0
                || course.grade_point.is_some_and(|value| !value.is_finite())
            {
                return Err(invalid_registrar_response());
            }
            Ok(CourseGrade::new(
                course.course_name,
                course.credit,
                course.grade,
                course.grade_point,
                course.semester,
            ))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(GradeReport::new(stage, kind, courses))
}

fn parse_campus_date(value: &str) -> Result<NaiveDate, Error> {
    let date =
        NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid_registrar_response())?;
    if date.format("%Y-%m-%d").to_string() != value {
        return Err(invalid_registrar_response());
    }
    Ok(date)
}

fn map_schedule_event(dto: CampusScheduleDto) -> Result<ScheduleEvent, Error> {
    let kind = match dto.kind.as_str() {
        "course" => ScheduleEventKind::Course,
        "exam" => ScheduleEventKind::Exam,
        "event" => ScheduleEventKind::Event,
        "deadline" => ScheduleEventKind::Deadline,
        "reminder" => ScheduleEventKind::Reminder,
        _ => return Err(invalid_registrar_response()),
    };
    if !valid_required_text(&dto.title) {
        return Err(invalid_registrar_response());
    }
    let starts_at = chrono::DateTime::parse_from_rfc3339(&dto.starts_at)
        .map_err(|_| invalid_registrar_response())?
        .with_timezone(&Utc);
    let ends_at = dto
        .ends_at
        .as_deref()
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| invalid_registrar_response())
        })
        .transpose()?;
    if ends_at.is_some_and(|end| end < starts_at) {
        return Err(invalid_registrar_response());
    }
    Ok(ScheduleEvent::new(
        dto.title,
        kind,
        starts_at,
        ends_at,
        dto.all_day,
        optional_display_text(dto.location)?,
        optional_display_text(dto.description)?,
    ))
}

fn map_exam(dto: CampusExamDto) -> Result<Exam, Error> {
    let weekday = match dto.exam_weekday.as_str() {
        "monday" => ExamWeekday::Monday,
        "tuesday" => ExamWeekday::Tuesday,
        "wednesday" => ExamWeekday::Wednesday,
        "thursday" => ExamWeekday::Thursday,
        "friday" => ExamWeekday::Friday,
        "saturday" => ExamWeekday::Saturday,
        "sunday" => ExamWeekday::Sunday,
        _ => return Err(invalid_registrar_response()),
    };
    if !valid_campus_month_day(dto.exam_month, dto.exam_day)
        || !valid_text(&dto.course_code)
        || !valid_text(&dto.course_sequence)
        || !valid_required_text(&dto.course_name)
        || !valid_text(&dto.exam_session)
        || !valid_text(&dto.schedule_raw)
        || !valid_text(&dto.location)
    {
        return Err(invalid_registrar_response());
    }
    let schedule_label = if dto.schedule_raw.trim().is_empty() {
        dto.exam_session.clone()
    } else {
        dto.schedule_raw.clone()
    };
    if schedule_label.trim().is_empty() {
        return Err(invalid_registrar_response());
    }
    Ok(Exam::new(
        dto.course_code,
        dto.course_sequence,
        dto.course_name,
        dto.exam_month,
        dto.exam_day,
        weekday,
        schedule_label,
        dto.location,
        optional_display_text(dto.department)?,
        optional_display_text(dto.category)?,
        optional_display_text(dto.instructor)?,
        dto.headcount,
    ))
}

fn valid_campus_month_day(month: u8, day: u8) -> bool {
    NaiveDate::from_ymd_opt(2000, u32::from(month), u32::from(day)).is_some()
}

fn valid_required_text(value: &str) -> bool {
    !value.trim().is_empty() && valid_text(value)
}

fn valid_text(value: &str) -> bool {
    value.len() <= 8192 && !value.chars().any(char::is_control)
}

fn optional_display_text(value: Option<String>) -> Result<Option<String>, Error> {
    match value {
        None => Ok(None),
        Some(value)
            if value.len() > 8192
                || value.chars().any(|character| {
                    character.is_control() && !matches!(character, '\n' | '\r' | '\t')
                }) =>
        {
            Err(invalid_registrar_response())
        }
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) => Ok(Some(value)),
    }
}

fn invalid_registrar_response() -> Error {
    Error::new(Service::Registrar, ErrorCode::InvalidResponse)
}

fn map_learn_term_calendar(dto: LearnTermCalendarDto) -> Result<LearnTermCalendar, Error> {
    if dto.upcoming.len() > 8 {
        return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
    }
    let mut ids = std::collections::HashSet::with_capacity(dto.upcoming.len() + 1);
    let current = map_learn_term(dto.current, &mut ids)?;
    let upcoming = dto
        .upcoming
        .into_iter()
        .map(|term| map_learn_term(term, &mut ids))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LearnTermCalendar::new(current, upcoming))
}

fn map_learn_term(
    dto: LearnTermDto,
    seen_ids: &mut std::collections::HashSet<String>,
) -> Result<AcademicTerm, Error> {
    let invalid = || Error::new(Service::Learn, ErrorCode::InvalidResponse);
    if !valid_required_text(&dto.id)
        || dto.id.len() > 128
        || !seen_ids.insert(dto.id)
        || !valid_required_text(&dto.label)
    {
        return Err(invalid());
    }
    let starts_on = parse_term_date(&dto.start_date)?;
    let ends_on = parse_term_date(&dto.end_date)?;
    let teaching_week_one = parse_term_date(&dto.first_day)?;
    let span = (ends_on - starts_on).num_days();
    let first_day_offset = (teaching_week_one - starts_on).num_days();
    let calculated_weeks = (ends_on - teaching_week_one).num_days().div_euclid(7) + 1;
    if starts_on > ends_on
        || !(0..=370).contains(&span)
        || !(-6..=2).contains(&first_day_offset)
        || teaching_week_one.weekday().number_from_monday() != 1
        || !(1..=80).contains(&dto.week_count)
        || i64::from(dto.week_count) != calculated_weeks
    {
        return Err(invalid());
    }
    Ok(AcademicTerm::new(
        dto.label,
        starts_on,
        ends_on,
        teaching_week_one,
        dto.week_count,
    ))
}

fn parse_term_date(value: &str) -> Result<NaiveDate, Error> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| Error::new(Service::Learn, ErrorCode::InvalidResponse))?;
    if date.format("%Y-%m-%d").to_string() != value {
        return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
    }
    Ok(date)
}

fn map_school_calendar(
    dto: SchoolCalendarDto,
    query: SchoolCalendarQuery,
) -> Result<SchoolCalendarImage, Error> {
    let invalid = || Error::new(Service::Calendar, ErrorCode::InvalidResponse);
    let selected_year = query.year().unwrap_or(dto.latest_year);
    if !(2000..=2100).contains(&dto.latest_year)
        || dto.year < 2000
        || dto.year > dto.latest_year
        || dto.year != selected_year
        || dto.semester != query.semester().as_str()
        || dto.language != query.language().as_str()
        || dto.image_bytes.len() < 4
        || dto.image_bytes.len() > 12 * 1024 * 1024
        || !dto.image_bytes.starts_with(&[0xff, 0xd8])
        || !dto.image_bytes.ends_with(&[0xff, 0xd9])
    {
        return Err(invalid());
    }
    if crate::captcha_image::jpeg_dimensions(&dto.image_bytes)
        .filter(|(width, height)| {
            *width > 0
                && *height > 0
                && *width <= 12_000
                && *height <= 12_000
                && u64::from(*width) * u64::from(*height) <= 80_000_000
        })
        .is_none()
    {
        return Err(invalid());
    }
    let semester = query.semester();
    let language = query.language();
    Ok(SchoolCalendarImage::new(
        dto.latest_year,
        dto.year,
        semester,
        language,
        dto.image_bytes,
    ))
}

fn calendar_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Calendar, code)
}

fn calendar_metadata(
    generated_at: &str,
    source: &str,
    status: &str,
) -> Result<ReadMetadata, Error> {
    if source != "live" || status != "ready" {
        return Err(Error::new(Service::Calendar, ErrorCode::InvalidResponse));
    }
    let observed_at = chrono::DateTime::parse_from_rfc3339(generated_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| Error::new(Service::Calendar, ErrorCode::InvalidResponse))?;
    Ok(ReadMetadata::new(
        ReadSource::Live,
        CacheFreshness::NotApplicable,
        observed_at,
        ReadCoverage::Complete,
        None,
    ))
}

/// INFO news operations borrowed from one client runtime.
pub struct NewsClient<'client> {
    runtime: &'client mut CampusRuntime,
    client_id: uuid::Uuid,
    cache_source: ReadSource,
    filter_generation: &'client mut u64,
    subscription_generation: &'client mut u64,
}

impl NewsClient<'_> {
    /// Reads validated filter choices from INFO. Each successful catalog read
    /// invalidates references returned by earlier catalog reads from this
    /// Client. This is a live-only read and is not served from cache.
    pub async fn catalog(&mut self) -> Result<ReadResult<NewsCatalog>, Error> {
        self.runtime.clear_info_failure_code();
        let dto = self
            .runtime
            .load_info_news_catalog()
            .await
            .map_err(|_| news_failure(self.runtime, ErrorCode::ServiceUnavailable))?;
        if dto.source != "live" {
            return Err(Error::new(Service::News, ErrorCode::InvalidResponse));
        }
        let channel_coverage = match (dto.status.as_str(), dto.channel_error.as_deref()) {
            ("ready", None) => NewsCatalogCoverage::Complete,
            ("partial", Some(_)) => NewsCatalogCoverage::Partial,
            _ => return Err(Error::new(Service::News, ErrorCode::InvalidResponse)),
        };
        let observed_at = chrono::DateTime::parse_from_rfc3339(&dto.generated_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|_| Error::new(Service::News, ErrorCode::InvalidResponse))?;

        *self.filter_generation = self.filter_generation.wrapping_add(1);
        let generation = *self.filter_generation;
        let sources = dto
            .sources
            .into_iter()
            .map(|option| {
                NewsSource::new(
                    NewsSourceRef::new(self.client_id, generation, option.id),
                    option.label,
                )
            })
            .collect();
        let channels = dto
            .channels
            .into_iter()
            .map(|option| {
                NewsChannel::new(
                    NewsChannelRef::new(
                        self.client_id,
                        generation,
                        option.id,
                        option.label.clone(),
                    ),
                    option.label,
                )
            })
            .collect();
        let coverage = match channel_coverage {
            NewsCatalogCoverage::Complete => ReadCoverage::Complete,
            NewsCatalogCoverage::Partial => {
                ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed)
            }
        };
        let metadata = ReadMetadata::new(
            ReadSource::Live,
            CacheFreshness::NotApplicable,
            observed_at,
            coverage,
            None,
        );
        Ok(ReadResult::new(
            NewsCatalog::new(sources, channels, channel_coverage),
            metadata,
        ))
    }

    /// Reads the current account's subscription rules. Every successful read
    /// replaces the runtime selectors and invalidates earlier rule references.
    pub async fn subscriptions(&mut self) -> Result<ReadResult<NewsSubscriptions>, Error> {
        self.runtime.clear_info_failure_code();
        let dto = self
            .runtime
            .load_info_news_subscriptions()
            .await
            .map_err(|_| news_failure(self.runtime, ErrorCode::ServiceUnavailable))?;
        let metadata = news_metadata(
            self.runtime,
            Some(&dto.generated_at),
            Some(&dto.source),
            Some(&dto.status),
            None,
            self.cache_source,
        )?;
        *self.subscription_generation = self.subscription_generation.wrapping_add(1);
        let generation = *self.subscription_generation;
        let rules = dto
            .rules
            .into_iter()
            .map(|rule| {
                NewsSubscription::new(
                    NewsSubscriptionRef::new(self.client_id, generation, rule.selector),
                    rule.title,
                    rule.order,
                    rule.sources,
                    rule.channels,
                    rule.keyword,
                )
            })
            .collect();
        Ok(ReadResult::new(NewsSubscriptions::new(rules), metadata))
    }

    /// Reads one page of articles for a rule selected from this client's most
    /// recent subscription result. It does not claim that later pages have
    /// been fetched.
    pub async fn subscription_articles(
        &mut self,
        reference: &NewsSubscriptionRef,
        page: u32,
    ) -> Result<ReadResult<NewsPage>, Error> {
        if !(1..=100).contains(&page) {
            return Err(Error::new(Service::News, ErrorCode::InvalidInput));
        }
        if !reference.belongs_to(self.client_id, *self.subscription_generation)
            || !self
                .runtime
                .info_news_subscription_reference_is_current(reference.selector())
        {
            return Err(Error::new(Service::News, ErrorCode::ContextMismatch));
        }
        self.runtime.clear_info_failure_code();
        let dto = self
            .runtime
            .load_info_news_subscription_page(reference.selector().to_owned(), page)
            .await
            .map_err(|_| news_failure(self.runtime, ErrorCode::ServiceUnavailable))?;
        if dto.feed != "subscription" {
            return Err(Error::new(Service::News, ErrorCode::InvalidResponse));
        }
        let metadata = news_metadata(
            self.runtime,
            dto.generated_at.as_deref(),
            dto.source.as_deref(),
            dto.status.as_deref(),
            dto.error.as_deref(),
            self.cache_source,
        )?;
        let items = self.map_news_articles(dto.items)?;
        Ok(ReadResult::new(NewsPage::new(page, items), metadata))
    }

    /// Reads the complete bounded favorites collection for the current
    /// Identity account. The Rust runtime rejects incomplete pagination.
    pub async fn favorites(&mut self) -> Result<ReadResult<NewsFavorites>, Error> {
        self.runtime.clear_info_failure_code();
        let dto = self
            .runtime
            .load_info_news_favorites()
            .await
            .map_err(|_| news_failure(self.runtime, ErrorCode::ServiceUnavailable))?;
        let metadata = news_metadata(
            self.runtime,
            Some(&dto.generated_at),
            Some(&dto.source),
            Some(&dto.status),
            None,
            self.cache_source,
        )?;
        if dto.empty != dto.items.is_empty() {
            return Err(Error::new(Service::News, ErrorCode::InvalidResponse));
        }
        let items = self.map_news_articles(dto.items)?;
        Ok(ReadResult::new(NewsFavorites::new(items), metadata))
    }

    /// Reads one validated list or search page with explicit cache behavior.
    pub async fn articles(
        &mut self,
        query: NewsQuery,
        policy: crate::read::ReadPolicy,
    ) -> Result<ReadResult<NewsPage>, Error> {
        let generation = *self.filter_generation;
        let valid_filter_refs = match query.kind() {
            NewsQueryKind::List {
                source, channel, ..
            } => {
                source
                    .as_ref()
                    .is_none_or(|reference| reference.belongs_to(self.client_id, generation))
                    && channel
                        .as_ref()
                        .is_none_or(|reference| reference.belongs_to(self.client_id, generation))
            }
            NewsQueryKind::Search { channel, .. } => channel
                .as_ref()
                .is_none_or(|reference| reference.belongs_to(self.client_id, generation)),
        };
        if !valid_filter_refs {
            return Err(Error::new(Service::News, ErrorCode::ContextMismatch));
        }
        self.runtime.clear_info_failure_code();
        let dto = match query.kind() {
            NewsQueryKind::List {
                page_size,
                source,
                channel,
            } => {
                self.runtime
                    .load_info_news_with_policy(
                        query.page(),
                        page_size.get(),
                        source.as_ref().map(|reference| reference.id().to_owned()),
                        channel.as_ref().map(|reference| reference.id().to_owned()),
                        policy,
                    )
                    .await
            }
            NewsQueryKind::Search {
                keyword,
                channel,
                exact_match,
            } => {
                self.runtime
                    .search_info_news_with_policy(
                        query.page(),
                        keyword.clone(),
                        channel
                            .as_ref()
                            .map(|reference| reference.label().to_owned()),
                        *exact_match,
                        policy,
                    )
                    .await
            }
        }
        .map_err(|_| news_failure(self.runtime, ErrorCode::ServiceUnavailable))?;

        let metadata = news_metadata(
            self.runtime,
            dto.generated_at.as_deref(),
            dto.source.as_deref(),
            dto.status.as_deref(),
            dto.error.as_deref(),
            self.cache_source,
        )?;
        let items = self.map_news_articles(dto.items)?;
        Ok(ReadResult::new(
            NewsPage::new(query.page(), items),
            metadata,
        ))
    }

    fn map_news_articles(
        &mut self,
        items: Vec<InfoNewsItemDto>,
    ) -> Result<Vec<NewsArticle>, Error> {
        let generation = self.runtime.info_news_link_generation();
        if !self.runtime.info_news_link_snapshot_is_current(generation) {
            return Err(Error::new(Service::News, ErrorCode::ContextMismatch));
        }
        items
            .into_iter()
            .map(|item| {
                if !self
                    .runtime
                    .info_news_article_reference_is_current(generation, &item.id)
                {
                    return Err(Error::new(Service::News, ErrorCode::InvalidResponse));
                }
                Ok(NewsArticle::new(
                    ArticleRef::new(self.client_id, generation, item.id),
                    item.title,
                    item.published_at,
                    item.source,
                    item.topped,
                    item.channel,
                    item.favorited,
                ))
            })
            .collect()
    }

    /// Reads detail only for an article selected by this client's current
    /// news list or search result. A stale or foreign reference is rejected
    /// before any cache lookup, handoff, or HTTP request.
    pub async fn article(
        &mut self,
        reference: &ArticleRef,
        policy: crate::read::ReadPolicy,
    ) -> Result<ReadResult<ArticleDetail>, Error> {
        if !reference.belongs_to(self.client_id) {
            return Err(Error::new(Service::News, ErrorCode::ContextMismatch));
        }
        let (generation, article_id) = reference.selector();
        if !self.runtime.info_news_link_snapshot_is_current(generation)
            || !self
                .runtime
                .info_news_article_reference_is_current(generation, article_id)
        {
            return Err(Error::new(Service::News, ErrorCode::ContextMismatch));
        }

        self.runtime.clear_info_failure_code();
        let dto = self
            .runtime
            .load_info_news_detail_result_with_policy(article_id.to_owned(), policy)
            .await
            .map_err(|_| news_failure(self.runtime, ErrorCode::ServiceUnavailable))?;
        let metadata = news_metadata(
            self.runtime,
            Some(&dto.generated_at),
            Some(&dto.source),
            Some(&dto.status),
            dto.error.as_deref(),
            self.cache_source,
        )?;
        let detail = ArticleDetail::new(
            dto.title,
            dto.content_html,
            dto.summary,
            dto.attachments
                .into_iter()
                .map(|attachment| NewsAttachment::new(attachment.name))
                .collect(),
        );
        Ok(ReadResult::new(detail, metadata))
    }
}

fn map_course_files(
    dto: LearnFileListDto,
    course: &CourseRef,
    client_id: uuid::Uuid,
    course_generation: u64,
    file_generation: u64,
) -> Result<ReadResult<CourseFiles>, Error> {
    if dto.course_id != course.course_id() {
        return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
    }
    let metadata =
        live_learn_collection_metadata(&dto.source, &dto.status, &dto.generated_at, dto.complete)?;
    let mut seen = std::collections::HashSet::new();
    let files = dto
        .files
        .into_iter()
        .map(|file| {
            if !valid_selector(&file.id)
                || !seen.insert(file.id.clone())
                || file.title.trim().is_empty()
                || file.title.chars().any(char::is_control)
                || file.suggested_filename.trim().is_empty()
                || file.suggested_filename.chars().any(char::is_control)
                || !valid_optional_text(file.description.as_deref(), true)
                || !valid_optional_text(file.size.as_deref(), false)
                || !valid_optional_text(file.uploaded_at.as_deref(), false)
                || !valid_optional_text(file.file_type.as_deref(), false)
            {
                return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
            }
            let reference = CourseFileRef::new(
                client_id,
                course_generation,
                file_generation,
                course.course_id().to_owned(),
                file.id,
            );
            Ok(CourseFile::new(
                reference,
                file.title,
                file.suggested_filename,
                file.description,
                file.size,
                file.uploaded_at,
                file.file_type,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadResult::new(CourseFiles::new(files), metadata))
}

fn map_course_file_categories(
    dto: LearnFileCategoryListDto,
    course: &CourseRef,
) -> Result<ReadResult<CourseFileCategories>, Error> {
    if dto.course_id != course.course_id() {
        return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
    }
    let metadata = live_learn_metadata(&dto.source, &dto.status, &dto.generated_at)?;
    let categories = dto
        .categories
        .into_iter()
        .map(|category| {
            if !valid_selector(&category.selector)
                || category.title.trim().is_empty()
                || category.title.chars().any(char::is_control)
            {
                return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
            }
            Ok(CourseFileCategory::new(category.title))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadResult::new(
        CourseFileCategories::new(categories),
        metadata,
    ))
}

fn map_course_discussions(
    dto: LearnDiscussionListDto,
    course: &CourseRef,
) -> Result<ReadResult<CourseDiscussions>, Error> {
    if dto.course_id != course.course_id() {
        return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
    }
    let metadata =
        live_learn_collection_metadata(&dto.source, &dto.status, &dto.generated_at, dto.complete)?;
    let discussions = dto
        .items
        .into_iter()
        .map(|item| {
            if !valid_selector(&item.selector)
                || item.title.trim().is_empty()
                || item.title.chars().any(char::is_control)
                || item.publisher_name.trim().is_empty()
                || item.publisher_name.chars().any(char::is_control)
                || item.published_at.trim().is_empty()
                || item.published_at.chars().any(char::is_control)
                || !valid_optional_text(item.last_reply_at.as_deref(), false)
            {
                return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
            }
            Ok(CourseDiscussion::new(
                item.title,
                item.publisher_name,
                item.published_at,
                item.last_reply_at,
                item.reply_count,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadResult::new(
        CourseDiscussions::new(discussions),
        metadata,
    ))
}

fn live_learn_collection_metadata(
    source: &str,
    status: &str,
    generated_at: &str,
    complete: bool,
) -> Result<ReadMetadata, Error> {
    let coverage = match (source, status, complete) {
        ("live", "ready", true) => ReadCoverage::Complete,
        ("live", "partial", false) => ReadCoverage::Partial(IncompleteReason::ReadLimitReached),
        _ => return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse)),
    };
    Ok(ReadMetadata::new(
        ReadSource::Live,
        CacheFreshness::NotApplicable,
        parse_required_time(generated_at)?,
        coverage,
        None,
    ))
}

fn valid_optional_text(value: Option<&str>, allow_multiline: bool) -> bool {
    value.is_none_or(|value| {
        value.len() <= 8192
            && !value.trim().is_empty()
            && !value.chars().any(|character| {
                character.is_control()
                    && !(allow_multiline && matches!(character, '\n' | '\r' | '\t'))
            })
    })
}

fn saved_file_matches(dto: &LearnFileDownloadDto, reference: &CourseFileRef) -> bool {
    dto.course_id == reference.course_id()
        && dto.file_id == reference.file_id()
        && dto.source == "live"
        && dto.status == "ready"
}

/// Network-learning course and assignment reads backed by this client's
/// shared Identity and Learn service sessions.
pub struct LearnClient<'client> {
    runtime: &'client mut CampusRuntime,
    client_id: uuid::Uuid,
    cache_source: ReadSource,
    course_generation: &'client mut u64,
    homework_generation: &'client mut u64,
    file_generation: &'client mut u64,
}

impl LearnClient<'_> {
    /// Reads the current course directory. The Runtime may use its validated
    /// account/semester cache or refresh it through the shared transport.
    pub async fn courses(&mut self) -> Result<ReadResult<CourseCatalog>, Error> {
        let dto = self
            .runtime
            .load_learn_courses()
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        let metadata = course_metadata(&dto, self.cache_source)?;
        let generation = self.course_generation.wrapping_add(1);
        let mut seen = std::collections::HashSet::new();
        let courses = dto
            .courses
            .into_iter()
            .map(|course| {
                if !valid_selector(&course.course_id)
                    || course.title.trim().is_empty()
                    || course.title.chars().any(char::is_control)
                    || !seen.insert(course.course_id.clone())
                {
                    return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
                }
                Ok(Course::new(
                    CourseRef::new(self.client_id, generation, course.course_id),
                    course.course_code,
                    course.title,
                    course.instructor,
                    course.semester,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        *self.course_generation = generation;
        *self.homework_generation = self.homework_generation.wrapping_add(1);
        Ok(ReadResult::new(
            CourseCatalog::new(dto.semester, courses),
            metadata,
        ))
    }

    /// Reads the active and expired announcements for a course selected from
    /// this client's latest course directory. Fresh and eligible stale cache
    /// results retain their source and freshness metadata.
    pub async fn announcements(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<CourseAnnouncements>, Error> {
        if !course.belongs_to(self.client_id, *self.course_generation) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let dto = self
            .runtime
            .load_learn_announcement_list(course.course_id().to_owned())
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        if dto.course_id != course.course_id() {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let metadata = learn_cache_metadata(
            &dto.generated_at,
            &dto.source,
            &dto.status,
            dto.error.is_some(),
            self.cache_source,
        )?;
        let items = dto
            .announcements
            .into_iter()
            .map(|announcement| map_course_announcement(announcement, course.course_id()))
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(ReadResult::new(CourseAnnouncements::new(items), metadata))
    }

    /// Reads all three student assignment buckets for a course selected from
    /// this client's latest `courses` result. The list is live-only and does
    /// not return a cache as if it were current.
    pub async fn homework(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<HomeworkList>, Error> {
        if !course.belongs_to(self.client_id, *self.course_generation) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let generation = self.homework_generation.wrapping_add(1);
        *self.homework_generation = generation;
        let selected_at = Instant::now();
        let dto = self
            .runtime
            .load_learn_homework(course.course_id().to_owned())
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        if dto.course_id != course.course_id() {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let metadata = live_learn_metadata(&dto.source, &dto.status, &dto.generated_at)?;
        let items = dto
            .items
            .into_iter()
            .map(|item| {
                if !valid_selector(&item.selector)
                    || item.title.trim().is_empty()
                    || item.title.chars().any(char::is_control)
                {
                    return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
                }
                let state = match item.state.as_str() {
                    "pending" => HomeworkState::Pending,
                    "submitted" => HomeworkState::Submitted,
                    "graded" => HomeworkState::Graded,
                    _ => return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse)),
                };
                Ok(Homework::new(
                    HomeworkRef::new_at(
                        self.client_id,
                        *self.course_generation,
                        generation,
                        course.course_id().to_owned(),
                        item.selector,
                        item.detail_available,
                        selected_at,
                    ),
                    item.title,
                    state,
                    parse_required_time(&item.due_at)?,
                    parse_optional_time(item.late_due_at.as_deref())?,
                    parse_optional_time(item.submitted_at.as_deref())?,
                    parse_optional_time(item.graded_at.as_deref())?,
                ))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(ReadResult::new(HomeworkList::new(items), metadata))
    }

    /// Reads detail for an assignment selected from this client's latest
    /// homework list. Foreign, stale, or unavailable selectors are rejected
    /// before any authentication handoff or network request.
    pub async fn homework_detail(
        &mut self,
        reference: &HomeworkRef,
    ) -> Result<ReadResult<HomeworkDetail>, Error> {
        if !reference.belongs_to(
            self.client_id,
            *self.course_generation,
            *self.homework_generation,
        ) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        if !reference.detail_available() {
            return Err(Error::new(Service::Learn, ErrorCode::InvalidInput));
        }
        let dto = self
            .runtime
            .load_learn_homework_detail(reference.selector().to_owned())
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        if dto.selector != reference.selector() {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let metadata = live_learn_metadata(&dto.source, &dto.status, &dto.generated_at)?;
        let detail = HomeworkDetail::new(
            dto.description,
            dto.answer_content,
            dto.submitted_content,
            dto.attachments
                .into_iter()
                .map(map_homework_attachment)
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(ReadResult::new(detail, metadata))
    }

    /// Reads the bounded live file list for a course selected from this
    /// client's latest course directory. A list capped by the service is
    /// returned with partial coverage rather than represented as complete.
    pub async fn files(&mut self, course: &CourseRef) -> Result<ReadResult<CourseFiles>, Error> {
        if !course.belongs_to(self.client_id, *self.course_generation) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let generation = self.file_generation.wrapping_add(1);
        *self.file_generation = generation;
        let dto = self
            .runtime
            .load_learn_files(course.course_id().to_owned())
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        map_course_files(
            dto,
            course,
            self.client_id,
            *self.course_generation,
            generation,
        )
    }

    /// Reads all verified category labels for a course selected from this
    /// client's latest course directory. Category IDs are not exposed because
    /// no category-specific query is currently part of the public contract.
    pub async fn file_categories(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<CourseFileCategories>, Error> {
        if !course.belongs_to(self.client_id, *self.course_generation) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let dto = self
            .runtime
            .load_learn_file_categories(course.course_id().to_owned())
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        map_course_file_categories(dto, course)
    }

    /// Reads the live discussion list for a course selected from this
    /// client's latest course directory. A list capped by the service carries
    /// partial coverage metadata.
    pub async fn discussions(
        &mut self,
        course: &CourseRef,
    ) -> Result<ReadResult<CourseDiscussions>, Error> {
        if !course.belongs_to(self.client_id, *self.course_generation) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let dto = self
            .runtime
            .load_learn_discussions(course.course_id().to_owned())
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        map_course_discussions(dto, course)
    }

    /// Saves a file selected from this client's latest course-file observation
    /// to a caller-chosen path. The reference expires after five minutes or
    /// when the course/file list is refreshed. The Runtime re-reads the file
    /// directory and refuses to overwrite an existing destination.
    pub async fn save_file(
        &mut self,
        reference: &CourseFileRef,
        destination: impl AsRef<std::path::Path>,
    ) -> Result<SavedCourseFile, Error> {
        if !reference.belongs_to(
            self.client_id,
            *self.course_generation,
            *self.file_generation,
        ) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        let destination = destination
            .as_ref()
            .to_str()
            .ok_or_else(|| Error::new(Service::Local, ErrorCode::InvalidInput))?;
        let dto = self
            .runtime
            .download_learn_file(
                reference.course_id().to_owned(),
                reference.file_id().to_owned(),
                destination.to_owned(),
            )
            .await
            .map_err(|_| learn_failure(self.runtime))?;
        if !saved_file_matches(&dto, reference) {
            return Err(Error::new(Service::Learn, ErrorCode::ContextMismatch));
        }
        parse_required_time(&dto.generated_at)?;
        Ok(SavedCourseFile::new(dto.bytes_written))
    }
}

fn course_metadata(
    dto: &LearnCourseListDto,
    cache_source: ReadSource,
) -> Result<ReadMetadata, Error> {
    learn_cache_metadata(
        &dto.generated_at,
        &dto.source,
        &dto.status,
        dto.error.is_some(),
        cache_source,
    )
}

fn map_course_announcement(
    announcement: LearnAnnouncementDto,
    expected_course_id: &str,
) -> Result<CourseAnnouncement, Error> {
    if announcement.course_id != expected_course_id
        || !valid_selector(&announcement.announcement_id)
        || announcement.title.trim().is_empty()
        || announcement.title.chars().any(char::is_control)
    {
        return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
    }
    Ok(CourseAnnouncement::new(
        announcement.title,
        announcement.publisher,
        announcement.content,
        parse_required_time(&announcement.published_at)?,
        parse_optional_time(announcement.expires_at.as_deref())?,
        announcement.read,
        announcement.important,
        announcement.favorited,
        announcement.expired,
    ))
}

fn learn_cache_metadata(
    generated_at: &str,
    source: &str,
    status: &str,
    has_refresh_error: bool,
    cache_source: ReadSource,
) -> Result<ReadMetadata, Error> {
    let observed_at = parse_required_time(generated_at)?;
    let (source, freshness, refresh_failure) = match (source, status) {
        ("live", "ready") if !has_refresh_error => {
            (ReadSource::Live, CacheFreshness::NotApplicable, None)
        }
        ("cache", "ready") if !has_refresh_error => (cache_source, CacheFreshness::Fresh, None),
        ("cache", "stale") if has_refresh_error => (
            cache_source,
            CacheFreshness::Stale,
            Some(ErrorCode::ServiceUnavailable),
        ),
        _ => return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse)),
    };
    Ok(ReadMetadata::new(
        source,
        freshness,
        observed_at,
        ReadCoverage::Complete,
        refresh_failure,
    ))
}

fn live_learn_metadata(
    source: &str,
    status: &str,
    generated_at: &str,
) -> Result<ReadMetadata, Error> {
    if source != "live" || status != "ready" {
        return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
    }
    Ok(ReadMetadata::new(
        ReadSource::Live,
        CacheFreshness::NotApplicable,
        parse_required_time(generated_at)?,
        ReadCoverage::Complete,
        None,
    ))
}

fn map_homework_attachment(
    attachment: crate::api::runtime::LearnHomeworkAttachmentDto,
) -> Result<HomeworkAttachment, Error> {
    let kind = match attachment.kind.as_str() {
        "assignment" => HomeworkAttachmentKind::Assignment,
        "answer" => HomeworkAttachmentKind::Answer,
        "submitted" => HomeworkAttachmentKind::Submitted,
        "grade" => HomeworkAttachmentKind::Grade,
        _ => return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse)),
    };
    if attachment.name.trim().is_empty() || attachment.name.chars().any(char::is_control) {
        return Err(Error::new(Service::Learn, ErrorCode::InvalidResponse));
    }
    Ok(HomeworkAttachment::new(
        kind,
        attachment.name,
        attachment.size,
    ))
}

fn learn_failure(runtime: &CampusRuntime) -> Error {
    let code = match runtime.auth_status().identity().state() {
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        AccountAuthState::Authenticated => ErrorCode::ServiceUnavailable,
    };
    Error::new(Service::Learn, code)
}

fn news_metadata(
    runtime: &CampusRuntime,
    generated_at: Option<&str>,
    source: Option<&str>,
    status: Option<&str>,
    refresh_error: Option<&str>,
    cache_source: ReadSource,
) -> Result<ReadMetadata, Error> {
    let observed_at = generated_at
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(|| Error::new(Service::News, ErrorCode::InvalidResponse))?;
    let (source, freshness) = match (source, status) {
        (Some("live"), Some("ready")) => (ReadSource::Live, CacheFreshness::NotApplicable),
        (Some("cache"), Some("ready")) => (cache_source, CacheFreshness::Fresh),
        (Some("cache"), Some("stale")) => (cache_source, CacheFreshness::Stale),
        _ => return Err(Error::new(Service::News, ErrorCode::InvalidResponse)),
    };
    let refresh_failure = (status == Some("stale") && refresh_error.is_some()).then(|| {
        runtime
            .last_info_failure_code()
            .map(info_error_code)
            .unwrap_or(ErrorCode::ServiceUnavailable)
    });
    Ok(ReadMetadata::new(
        source,
        freshness,
        observed_at,
        ReadCoverage::Complete,
        refresh_failure,
    ))
}

fn news_failure(runtime: &CampusRuntime, fallback: ErrorCode) -> Error {
    if let Some(code) = runtime.last_info_failure_code() {
        return Error::new(Service::News, info_error_code(code));
    }
    let state = runtime.auth_status().identity().state();
    let code = match state {
        crate::auth::AccountAuthState::SignedOut => ErrorCode::SessionRequired,
        crate::auth::AccountAuthState::Expired => ErrorCode::SessionExpired,
        crate::auth::AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        crate::auth::AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
        crate::auth::AccountAuthState::RestoredUnverified => ErrorCode::SessionRequired,
        crate::auth::AccountAuthState::Authenticated => fallback,
    };
    Error::new(Service::News, code)
}

fn info_error_code(diagnostic: &str) -> ErrorCode {
    match diagnostic {
        "info_news_cache_miss" => ErrorCode::CacheMiss,
        "info_news_account_changed" => ErrorCode::ContextMismatch,
        "info_session_expired" | "info_auth_required" => ErrorCode::SessionExpired,
        "info_transport" | "info_news_transport" => ErrorCode::NetworkUnavailable,
        "info_news_rate_limited" | "info_rate_limited" => ErrorCode::RateLimited,
        "info_news_link_conflict" | "info_news_link_limit" => ErrorCode::InvalidResponse,
        value
            if value.starts_with("info_detail_")
                || value.starts_with("info_catalog_")
                || value.starts_with("info_mapping_")
                || value.starts_with("info_origin_")
                || value.starts_with("info_news_redirect")
                || value == "info_csrf_missing" =>
        {
            ErrorCode::InvalidResponse
        }
        _ => ErrorCode::ServiceUnavailable,
    }
}

/// Operations backed by this client's independent SelfService account session.
pub struct SelfServiceClient<'client> {
    runtime: &'client mut CampusRuntime,
    client_id: uuid::Uuid,
}

impl SelfServiceClient<'_> {
    /// Reads the account summary after checking the SelfService session.
    pub async fn account(&mut self) -> Result<ReadResult<AccountProfile>, Error> {
        require_self_service_session(self.runtime)?;
        self.runtime.clear_usereg_failure_code();
        let value = self.runtime.load_usereg_account().await.map_err(|_| {
            usereg_error(
                self.runtime,
                Service::SelfService,
                ErrorCode::ServiceUnavailable,
                false,
            )
        })?;
        Ok(live_read(AccountProfile::from_runtime(value)))
    }

    /// Reads the complete currently online-device table. Each row contains an
    /// opaque reference bound to this Client and this exact list snapshot.
    pub async fn online_devices(&mut self) -> Result<ReadResult<Vec<OnlineDevice>>, Error> {
        require_self_service_session(self.runtime)?;
        self.runtime.clear_usereg_failure_code();
        let (generation, devices) =
            self.runtime
                .load_usereg_devices_for_sdk()
                .await
                .map_err(|_| {
                    usereg_error(
                        self.runtime,
                        Service::SelfService,
                        ErrorCode::ServiceUnavailable,
                        false,
                    )
                })?;
        Ok(live_read(
            devices
                .into_iter()
                .map(|device| {
                    let reference = DeviceRef::new(self.client_id, generation, device.index);
                    OnlineDevice::from_runtime(device, reference)
                })
                .collect(),
        ))
    }

    /// Explicitly disconnects one device from the current SelfService account.
    ///
    /// The reference must come from this Client's latest `online_devices`
    /// result. A stale or foreign-client reference is rejected before any
    /// request is dispatched. An unclear one-shot result is returned as an
    /// error and is never automatically repeated.
    pub async fn disconnect_device(&mut self, reference: &DeviceRef) -> Result<(), Error> {
        if !reference.belongs_to(self.client_id) {
            return Err(Error::new(Service::SelfService, ErrorCode::ContextMismatch));
        }
        require_self_service_session(self.runtime)?;
        let (generation, index) = reference.selector();
        if !self
            .runtime
            .usereg_device_reference_is_current(generation, index)
        {
            return Err(Error::new(Service::SelfService, ErrorCode::ContextMismatch));
        }
        self.runtime.clear_usereg_failure_code();
        self.runtime
            .disconnect_usereg_device_for_sdk(generation, index)
            .await
            .map_err(|_| {
                usereg_error(
                    self.runtime,
                    Service::SelfService,
                    ErrorCode::OutcomeUnconfirmed,
                    false,
                )
            })
    }

    /// Reads the usage and balance table. Values retain the units and text
    /// representation supplied by the service.
    pub async fn usage(&mut self) -> Result<ReadResult<UsageBalance>, Error> {
        require_self_service_session(self.runtime)?;
        self.runtime.clear_usereg_failure_code();
        let value = self.runtime.load_usereg_balance().await.map_err(|_| {
            usereg_error(
                self.runtime,
                Service::SelfService,
                ErrorCode::ServiceUnavailable,
                false,
            )
        })?;
        Ok(live_read(UsageBalance::from_runtime(value)))
    }
}

/// Read-only campus-network observations through the shared transport.
pub struct NetworkClient<'client> {
    runtime: &'client mut CampusRuntime,
    owner: uuid::Uuid,
    profiles: &'client mut NetworkProfileStore,
}

impl NetworkClient<'_> {
    /// Borrows local connection-profile operations. These records do not
    /// create or modify either Auth account.
    pub fn profiles(&mut self) -> NetworkProfilesClient<'_> {
        NetworkProfilesClient {
            owner: self.owner,
            store: self.profiles,
        }
    }

    /// Reads the TUNet portal's registration status for the local IPv4.
    ///
    /// This request is read-only and does not log in, disconnect, or store a
    /// third account. It says nothing about general Internet reachability or
    /// system-managed Wi-Fi authentication such as Tsinghua Secure.
    pub async fn observe_portal_status(&mut self) -> Result<PortalObservation, Error> {
        let value = self
            .runtime
            .load_tunet_status()
            .await
            .map_err(|_| Error::new(Service::Network, ErrorCode::ServiceUnavailable))?;
        portal_observation(value)
    }
}

/// Client-lifetime local network profiles and non-secret form preparation.
pub struct NetworkProfilesClient<'client> {
    owner: uuid::Uuid,
    store: &'client mut NetworkProfileStore,
}

impl NetworkProfilesClient<'_> {
    /// Lists local profiles in stable label/identifier order.
    pub fn list(&self) -> Vec<NetworkProfileSummary> {
        let mut profiles = self
            .store
            .profiles
            .values()
            .map(StoredNetworkProfile::summary)
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| {
            left.label()
                .cmp(right.label())
                .then_with(|| left.id().as_str().cmp(&right.id().as_str()))
        });
        profiles
    }

    /// Adds a local profile. A supplied password is stored only because the
    /// input explicitly requested it; whether it survives this Client depends
    /// on the independently selected profile-storage policy.
    pub fn save(&mut self, input: NetworkProfileInput) -> Result<NetworkProfileSummary, Error> {
        let (label, username, method, password) = input.into_parts();
        let profile = StoredNetworkProfile {
            id: NetworkProfileId::new(),
            label,
            username,
            method,
            password,
            revision: 1,
        };
        let summary = profile.summary();
        let mut profiles = self.store.profiles.clone();
        profiles.insert(profile.id, profile);
        self.store.commit(profiles)?;
        Ok(summary)
    }

    /// Updates one profile. An existing password is retained only when the
    /// username and access method are unchanged; otherwise a new password
    /// must be supplied explicitly.
    pub fn update(
        &mut self,
        id: NetworkProfileId,
        input: NetworkProfileInput,
    ) -> Result<NetworkProfileSummary, Error> {
        let Some(previous) = self.store.profiles.get(&id) else {
            return Err(Error::new(Service::Network, ErrorCode::ContextMismatch));
        };
        let revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::new(Service::Network, ErrorCode::Internal))?;
        let (label, username, method, submitted_password) = input.into_parts();
        let same_credential_scope = previous.username == username && previous.method == method;
        let password = match submitted_password {
            Some(password) => Some(password),
            None if same_credential_scope => previous.password.clone(),
            None => None,
        };
        let profile = StoredNetworkProfile {
            id,
            label,
            username,
            method,
            password,
            revision,
        };
        let summary = profile.summary();
        let mut profiles = self.store.profiles.clone();
        profiles.insert(id, profile);
        self.store.commit(profiles)?;
        Ok(summary)
    }

    /// Prepares non-secret account fields for an application form. The
    /// returned password status is advisory; the password itself never leaves
    /// Rust.
    pub fn prepare_fill(&self, id: NetworkProfileId) -> Result<PreparedNetworkInput, Error> {
        let profile = self
            .store
            .profiles
            .get(&id)
            .ok_or_else(|| Error::new(Service::Network, ErrorCode::ContextMismatch))?;
        Ok(PreparedNetworkInput::new(
            self.owner,
            profile.id,
            profile.revision,
            profile.summary(),
        ))
    }

    /// Returns the saved password only for a prepared value that still points
    /// to this Client's current profile revision. The caller must explicitly
    /// expose the returned secret to a form.
    pub fn password_for_fill(
        &self,
        prepared: &PreparedNetworkInput,
    ) -> Result<Option<NetworkProfilePassword>, Error> {
        let profile = self
            .store
            .profiles
            .get(&prepared.summary().id())
            .ok_or_else(|| Error::new(Service::Network, ErrorCode::ContextMismatch))?;
        if !prepared.belongs_to(self.owner, profile) {
            return Err(Error::new(Service::Network, ErrorCode::ContextMismatch));
        }
        Ok(profile
            .password
            .as_ref()
            .map(|password| NetworkProfilePassword::new(password.to_string())))
    }

    /// Checks whether a previously prepared form still refers to this
    /// Client's current profile revision.
    pub fn is_current(&self, prepared: &PreparedNetworkInput) -> bool {
        self.store
            .profiles
            .get(&prepared.summary().id())
            .is_some_and(|profile| prepared.belongs_to(self.owner, profile))
    }

    /// Deletes a local profile and zeroizes its in-memory password. Returns
    /// `false` if that profile is no longer present, and reports persistent
    /// storage failures instead of silently treating them as success.
    pub fn delete(&mut self, id: NetworkProfileId) -> Result<bool, Error> {
        if !self.store.profiles.contains_key(&id) {
            return Ok(false);
        }
        let mut profiles = self.store.profiles.clone();
        profiles.remove(&id);
        self.store.commit(profiles)?;
        Ok(true)
    }
}

fn portal_observation(value: TunetNetworkStatusDto) -> Result<PortalObservation, Error> {
    let registration = match (value.state.as_str(), value.online, value.signal.as_str()) {
        ("online", true, "positive") => PortalAddressRegistration::Registered,
        ("offline", false, "negative") => PortalAddressRegistration::NotRegistered,
        ("unknown", false, "unknown") => PortalAddressRegistration::Unknown,
        _ => return Err(Error::new(Service::Network, ErrorCode::InvalidResponse)),
    };
    Ok(PortalObservation::verified(registration, Utc::now()))
}

fn outcome_from_status(
    runtime: &CampusRuntime,
    status: CampusRuntimeStatusDto,
) -> Result<IdentityLoginOutcome, Error> {
    match status.state.as_str() {
        "authenticated" => Ok(IdentityLoginOutcome::Authenticated(
            runtime.auth_status().identity().clone(),
        )),
        "requires_second_factor" | "authenticating" => {
            let methods = status
                .second_factor_methods
                .iter()
                .map(|method| SecondFactorMethod::from_backend(method))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    Error::new(
                        Service::Auth(AuthDomain::Identity),
                        ErrorCode::InvalidResponse,
                    )
                })?;
            Ok(IdentityLoginOutcome::NeedsInteraction {
                methods,
                masked_phone: status.masked_phone,
            })
        }
        _ => Err(Error::new(
            Service::Auth(AuthDomain::Identity),
            ErrorCode::InvalidResponse,
        )),
    }
}

fn identity_access_error(runtime: &CampusRuntime) -> Option<Error> {
    let state = runtime.auth_status().identity().state();
    let code = match state {
        crate::auth::AccountAuthState::Authenticated => return None,
        crate::auth::AccountAuthState::SignedOut => ErrorCode::SessionRequired,
        crate::auth::AccountAuthState::Expired => ErrorCode::SessionExpired,
        crate::auth::AccountAuthState::RestoredUnverified => ErrorCode::SessionRequired,
        crate::auth::AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        crate::auth::AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
    };
    Some(Error::new(Service::Auth(AuthDomain::Identity), code))
}

fn require_self_service_session(runtime: &CampusRuntime) -> Result<(), Error> {
    let status = runtime.auth_status().self_service().state();
    let code = match status {
        AccountAuthState::Authenticated => return Ok(()),
        AccountAuthState::SignedOut | AccountAuthState::RestoredUnverified => {
            ErrorCode::SessionRequired
        }
        AccountAuthState::Expired => ErrorCode::SessionExpired,
        AccountAuthState::NeedsInteraction => ErrorCode::InteractionRequired,
        AccountAuthState::Authenticating => ErrorCode::InteractionInProgress,
    };
    Err(Error::new(Service::Auth(AuthDomain::SelfService), code))
}

fn live_read<T>(value: T) -> ReadResult<T> {
    ReadResult::new(
        value,
        ReadMetadata::new(
            ReadSource::Live,
            CacheFreshness::NotApplicable,
            Utc::now(),
            ReadCoverage::Complete,
            None,
        ),
    )
}

fn usereg_error(
    runtime: &CampusRuntime,
    service: Service,
    fallback: ErrorCode,
    authentication_flow: bool,
) -> Error {
    let code = runtime
        .last_usereg_failure_code()
        .map(|diagnostic| usereg_error_code(diagnostic, authentication_flow))
        .unwrap_or(fallback);
    Error::new(service, code)
}

fn usereg_error_code(diagnostic: &str, authentication_flow: bool) -> ErrorCode {
    match diagnostic {
        "usereg_rate_limited" => ErrorCode::RateLimited,
        "usereg_transport" => ErrorCode::NetworkUnavailable,
        "usereg_http_unavailable" => ErrorCode::ServiceUnavailable,
        "usereg_account_mismatch" => ErrorCode::ContextMismatch,
        "usereg_action_unconfirmed" => ErrorCode::OutcomeUnconfirmed,
        "usereg_session_expired" => ErrorCode::SessionExpired,
        "usereg_http_auth_rejected" if !authentication_flow => ErrorCode::SessionExpired,
        "usereg_http_auth_rejected" if authentication_flow => ErrorCode::AuthenticationRejected,
        "usereg_session_unproven" => ErrorCode::SessionRequired,
        value
            if value.starts_with("usereg_")
                && (value.contains("invalid")
                    || value.contains("missing")
                    || value.contains("unproven")
                    || value.contains("changed")) =>
        {
            ErrorCode::InvalidResponse
        }
        _ => ErrorCode::ServiceUnavailable,
    }
}

fn set_private_directory_permissions(path: &std::path::Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::new(Service::Local, ErrorCode::StorageUnavailable))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            cache_policy: ClientCachePolicy::Ephemeral,
            network_profile_storage: NetworkProfileStoragePolicy::MemoryOnly,
            identity_session_storage: IdentitySessionStoragePolicy::MemoryOnly,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RemoveTestDirectory(PathBuf);

    impl Drop for RemoveTestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn service_hall_test_time() -> String {
        "2026-09-25T12:00:00+08:00".into()
    }

    #[test]
    fn backend_refactor_service_hall_directory_maps_read_metadata_and_hides_ids() {
        let result = map_service_hall_directory(ThosServicesDto {
            items: vec![crate::api::thos::ThosServiceDto {
                id: "private-service-id".into(),
                name: "Synthetic service".into(),
                department: "Synthetic unit".into(),
                kind: Some("office".into()),
                in_open_period: Some(true),
            }],
            reported_total: 1,
            complete: true,
            generated_at: service_hall_test_time(),
            source: "live".into(),
            status: "ready".into(),
            error: None,
        })
        .unwrap();

        assert_eq!(result.metadata().source(), ReadSource::Live);
        assert_eq!(result.metadata().coverage(), ReadCoverage::Complete);
        assert_eq!(result.data().items()[0].name(), "Synthetic service");
        assert_eq!(result.data().items()[0].kind(), Some("office"));
        assert!(
            !format!("{:?} {:?}", result.data(), result.data().items()[0])
                .contains("private-service-id")
        );
    }

    #[test]
    fn backend_refactor_service_hall_directory_rejects_conflicting_completeness() {
        let error = map_service_hall_directory(ThosServicesDto {
            items: Vec::new(),
            reported_total: 0,
            complete: true,
            generated_at: service_hall_test_time(),
            source: "live".into(),
            status: "ready".into(),
            error: Some("must not be surfaced".into()),
        })
        .unwrap_err();

        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_service_hall_partial_phase_list_has_no_usable_ref() {
        let dto = ThosTaskListDto {
            kind: "phases".into(),
            items: vec![crate::api::thos::ThosTaskDto {
                id: uuid::Uuid::new_v4().to_string(),
                title: "Synthetic phase".into(),
                status: "正在办理".into(),
                node: String::new(),
                date: String::new(),
                progress: Some(20),
            }],
            reported_total: 2,
            complete: false,
            generated_at: service_hall_test_time(),
            source: "live".into(),
            status: "partial".into(),
            error: Some("redacted partial explanation".into()),
        };
        let result =
            map_service_hall_tasks(dto, TaskView::Phases, uuid::Uuid::new_v4(), 4).unwrap();

        assert_eq!(
            result.metadata().coverage(),
            ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed)
        );
        assert!(result.data().items()[0].phase_reference().is_none());
        assert!(!format!("{:?}", result.data()).contains("Synthetic phase"));
    }

    #[test]
    fn backend_refactor_service_hall_complete_phase_list_creates_opaque_scoped_ref() {
        let owner = uuid::Uuid::new_v4();
        let selector = uuid::Uuid::new_v4().to_string();
        let result = map_service_hall_tasks(
            ThosTaskListDto {
                kind: "phases".into(),
                items: vec![crate::api::thos::ThosTaskDto {
                    id: selector.clone(),
                    title: "Synthetic phase".into(),
                    status: "正在办理".into(),
                    node: "当前进度 1/2".into(),
                    date: String::new(),
                    progress: Some(50),
                }],
                reported_total: 1,
                complete: true,
                generated_at: service_hall_test_time(),
                source: "cache".into(),
                status: "ready".into(),
                error: None,
            },
            TaskView::Phases,
            owner,
            5,
        )
        .unwrap();
        let reference = result.data().items()[0].phase_reference().unwrap();

        assert_eq!(result.metadata().source(), ReadSource::MemoryCache);
        assert_eq!(result.metadata().freshness(), CacheFreshness::Fresh);
        assert!(reference.belongs_to(owner, 5));
        assert!(!format!("{:?} {:?}", result.data(), reference).contains(&selector));
        assert!(!format!("{:?}", result.data()).contains("Synthetic phase"));
    }

    #[test]
    fn backend_refactor_service_hall_phase_details_validate_selection_and_hide_payload() {
        let reference = WorkflowTaskRef::new(
            uuid::Uuid::new_v4(),
            2,
            TaskView::Phases,
            "private-phase-selector".into(),
        );
        let result = map_service_hall_phase_details(
            ThosPhaseStepsDto {
                task_id: "private-phase-selector".into(),
                steps: vec![crate::api::thos::ThosPhaseStepDto {
                    order: "1".into(),
                    name: "Private phase name".into(),
                    state: "Private phase state".into(),
                    items: vec![crate::api::thos::ThosPhaseStepItemDto {
                        name: "Private service item".into(),
                        state: "Private item state".into(),
                    }],
                }],
                generated_at: service_hall_test_time(),
                source: "live".into(),
                status: "ready".into(),
            },
            &reference,
        )
        .unwrap();
        let rendered = format!(
            "{:?} {:?} {:?}",
            result.data(),
            result.data().steps()[0],
            result.data().steps()[0].items()[0]
        );

        assert_eq!(result.metadata().coverage(), ReadCoverage::Complete);
        for private_value in [
            "private-phase-selector",
            "Private phase name",
            "Private phase state",
            "Private service item",
            "Private item state",
        ] {
            assert!(!rendered.contains(private_value));
        }
        let mismatched = WorkflowTaskRef::new(
            uuid::Uuid::new_v4(),
            99,
            TaskView::Phases,
            "different-selector".into(),
        );
        let error = map_service_hall_phase_details(
            ThosPhaseStepsDto {
                task_id: "private-phase-selector".into(),
                steps: Vec::new(),
                generated_at: service_hall_test_time(),
                source: "live".into(),
                status: "ready".into(),
            },
            &mismatched,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }

    #[tokio::test]
    async fn backend_refactor_service_hall_phase_reference_rejects_foreign_client_before_io() {
        let owner = Client::builder().build().unwrap();
        let reference = WorkflowTaskRef::new(
            owner.instance_id,
            owner.service_hall_phase_generation,
            TaskView::Phases,
            "private-phase-selector".into(),
        );
        let mut foreign = Client::builder().build().unwrap();

        let error = foreign
            .service_hall()
            .phase_details(&reference, ServiceHallReadPolicy::Refresh)
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::ServiceHall);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert!(!format!("{reference:?}").contains("private-phase-selector"));
        assert!(owner.auth_status().identity().state() == AccountAuthState::SignedOut);
    }

    #[tokio::test]
    async fn backend_refactor_service_hall_phase_reference_rejects_old_generation_before_io() {
        let mut client = Client::builder().build().unwrap();
        let reference = WorkflowTaskRef::new(
            client.instance_id,
            client.service_hall_phase_generation,
            TaskView::Phases,
            "private-phase-selector".into(),
        );
        client.invalidate_service_hall_references();

        let error = client
            .service_hall()
            .phase_details(&reference, ServiceHallReadPolicy::CacheOnly)
            .await
            .unwrap_err();

        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_foreign_course_ref_is_rejected_before_auth_or_io() {
        let mut client = Client::builder().build().unwrap();
        let reference = CourseRef::new(uuid::Uuid::new_v4(), 1, "private-course-id".into());

        let error = client.learn().homework(&reference).await.unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_stale_course_ref_is_rejected_before_auth_or_io() {
        let mut client = Client::builder().build().unwrap();
        client.learn_course_generation = 2;
        let reference = CourseRef::new(client.instance_id, 1, "private-course-id".into());

        let error = client.learn().homework(&reference).await.unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_announcement_foreign_course_ref_is_rejected_before_io() {
        let mut client = Client::builder().build().unwrap();
        let reference = CourseRef::new(uuid::Uuid::new_v4(), 1, "private-course-id".into());

        let error = client.learn().announcements(&reference).await.unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_foreign_file_ref_is_rejected_before_io() {
        let mut client = Client::builder().build().unwrap();
        let reference = CourseFileRef::new(
            uuid::Uuid::new_v4(),
            0,
            0,
            "private-course-id".into(),
            "private-file-id".into(),
        );

        let error = client
            .learn()
            .save_file(&reference, std::env::temp_dir().join("must-not-be-created"))
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[test]
    fn backend_refactor_learn_session_invalidation_expires_course_file_refs() {
        let mut client = Client::builder().build().unwrap();
        let reference = CourseFileRef::new(
            client.instance_id,
            client.learn_course_generation,
            client.learn_file_generation,
            "private-course-id".into(),
            "private-file-id".into(),
        );
        assert!(reference.belongs_to(
            client.instance_id,
            client.learn_course_generation,
            client.learn_file_generation,
        ));

        client.invalidate_learn_references();

        assert!(!reference.belongs_to(
            client.instance_id,
            client.learn_course_generation,
            client.learn_file_generation,
        ));
    }

    #[test]
    fn backend_refactor_learn_files_preserve_bounded_coverage_and_redact_values() {
        let client_id = uuid::Uuid::new_v4();
        let course = CourseRef::new(client_id, 4, "private-course-id".into());
        let make_dto = |complete, status: &str| LearnFileListDto {
            course_id: "private-course-id".into(),
            files: vec![crate::api::runtime::LearnFileDto {
                id: "private-file-selector".into(),
                title: "private-file-title".into(),
                suggested_filename: "private-file.pdf".into(),
                description: Some("private-file-description".into()),
                size: Some("2 MB".into()),
                uploaded_at: Some("2026-09-25 09:00".into()),
                file_type: Some("pdf".into()),
                category_selector: Some("private-category-selector".into()),
            }],
            complete,
            generated_at: Utc::now().to_rfc3339(),
            source: "live".into(),
            status: status.into(),
            error: (!complete).then(|| "internal partial-read explanation".into()),
        };

        let complete = map_course_files(make_dto(true, "ready"), &course, client_id, 4, 9).unwrap();
        assert_eq!(complete.metadata().coverage(), ReadCoverage::Complete);
        assert_eq!(complete.data().items().len(), 1);
        assert!(
            complete.data().items()[0]
                .reference()
                .belongs_to(client_id, 4, 9)
        );
        let debug = format!("{complete:?} {:?}", complete.data().items()[0]);
        for private_value in [
            "private-course-id",
            "private-file-selector",
            "private-file-title",
            "private-file-description",
            "private-category-selector",
            "internal partial-read explanation",
        ] {
            assert!(!debug.contains(private_value));
        }

        let partial =
            map_course_files(make_dto(false, "partial"), &course, client_id, 4, 10).unwrap();
        assert_eq!(
            partial.metadata().coverage(),
            ReadCoverage::Partial(IncompleteReason::ReadLimitReached)
        );
    }

    #[test]
    fn backend_refactor_learn_discussion_result_preserves_partial_coverage_and_redacts() {
        let client_id = uuid::Uuid::new_v4();
        let course = CourseRef::new(client_id, 2, "private-course-id".into());
        let result = map_course_discussions(
            LearnDiscussionListDto {
                course_id: "private-course-id".into(),
                items: vec![crate::api::runtime::LearnDiscussionDto {
                    selector: "private-discussion-selector".into(),
                    title: "private-discussion-title".into(),
                    publisher_name: "private-publisher".into(),
                    published_at: "2026-09-25 09:00".into(),
                    last_reply_at: None,
                    reply_count: 2,
                }],
                complete: false,
                generated_at: Utc::now().to_rfc3339(),
                source: "live".into(),
                status: "partial".into(),
                error: Some("private partial-read detail".into()),
            },
            &course,
        )
        .unwrap();

        assert_eq!(
            result.metadata().coverage(),
            ReadCoverage::Partial(IncompleteReason::ReadLimitReached)
        );
        assert_eq!(result.data().items()[0].reply_count(), 2);
        let debug = format!("{result:?} {:?}", result.data().items()[0]);
        for private_value in [
            "private-course-id",
            "private-discussion-selector",
            "private-discussion-title",
            "private-publisher",
            "private partial-read detail",
        ] {
            assert!(!debug.contains(private_value));
        }
    }

    #[test]
    fn backend_refactor_learn_categories_do_not_expose_protocol_selectors() {
        let course = CourseRef::new(uuid::Uuid::new_v4(), 2, "private-course-id".into());
        let result = map_course_file_categories(
            LearnFileCategoryListDto {
                course_id: "private-course-id".into(),
                categories: vec![crate::api::runtime::LearnFileCategoryDto {
                    selector: "private-category-selector".into(),
                    title: "第一单元".into(),
                }],
                generated_at: Utc::now().to_rfc3339(),
                source: "live".into(),
                status: "ready".into(),
            },
            &course,
        )
        .unwrap();

        assert_eq!(result.data().items()[0].title(), "第一单元");
        assert_eq!(result.metadata().coverage(), ReadCoverage::Complete);
        assert!(
            !format!("{result:?} {:?}", result.data().items()[0])
                .contains("private-category-selector")
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_stale_homework_ref_is_rejected_before_auth_or_io() {
        let mut client = Client::builder().build().unwrap();
        client.learn_course_generation = 3;
        client.learn_homework_generation = 5;
        let reference = HomeworkRef::new(
            client.instance_id,
            3,
            4,
            "private-course-id".into(),
            "private-assignment-selector".into(),
            true,
        );

        let error = client
            .learn()
            .homework_detail(&reference)
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_unavailable_detail_is_rejected_before_auth_or_io() {
        let mut client = Client::builder().build().unwrap();
        client.learn_course_generation = 3;
        client.learn_homework_generation = 5;
        let reference = HomeworkRef::new(
            client.instance_id,
            3,
            5,
            "private-course-id".into(),
            "private-assignment-selector".into(),
            false,
        );

        let error = client
            .learn()
            .homework_detail(&reference)
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_learn_course_read_without_identity_is_a_session_error() {
        let mut client = Client::builder().build().unwrap();

        let error = client.learn().courses().await.unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::SessionRequired);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[test]
    fn backend_refactor_learn_cache_metadata_keeps_staleness_and_hides_error_text() {
        let now = Utc::now().to_rfc3339();
        let dto = LearnCourseListDto {
            semester: "2026-2027-1".into(),
            stage: "graduate".into(),
            generated_at: now,
            source: "cache".into(),
            status: "stale".into(),
            error: Some("private localized failure detail".into()),
            courses: Vec::new(),
        };

        let metadata = course_metadata(&dto, ReadSource::PersistentCache).unwrap();

        assert_eq!(metadata.source(), ReadSource::PersistentCache);
        assert_eq!(metadata.freshness(), CacheFreshness::Stale);
        assert_eq!(
            metadata.refresh_failure(),
            Some(ErrorCode::ServiceUnavailable)
        );
        assert!(!format!("{metadata:?}").contains("private localized failure detail"));
    }

    #[test]
    fn backend_refactor_learn_announcement_cache_metadata_keeps_provenance() {
        let metadata = learn_cache_metadata(
            &Utc::now().to_rfc3339(),
            "cache",
            "stale",
            true,
            ReadSource::ClientCache,
        )
        .unwrap();

        assert_eq!(metadata.source(), ReadSource::ClientCache);
        assert_eq!(metadata.freshness(), CacheFreshness::Stale);
        assert_eq!(
            metadata.refresh_failure(),
            Some(ErrorCode::ServiceUnavailable)
        );
    }

    #[test]
    fn backend_refactor_learn_announcement_mapping_preserves_safe_fields() {
        let published_at = Utc::now().to_rfc3339();
        let dto = LearnAnnouncementDto {
            course_id: "course-validated-by-runtime".into(),
            announcement_id: "private-announcement-id".into(),
            title: "Reading for next week".into(),
            publisher: Some("Course staff".into()),
            content: Some("Bring the assigned reading.".into()),
            published_at,
            expires_at: None,
            read: Some(false),
            important: Some(true),
            favorited: Some(false),
            expired: false,
        };

        let announcement = map_course_announcement(dto, "course-validated-by-runtime").unwrap();

        assert_eq!(announcement.title(), "Reading for next week");
        assert_eq!(announcement.publisher(), Some("Course staff"));
        assert_eq!(announcement.content(), Some("Bring the assigned reading."));
        assert_eq!(announcement.is_read(), Some(false));
        assert_eq!(announcement.is_important(), Some(true));
        assert_eq!(announcement.is_favorited(), Some(false));
        assert!(!announcement.is_expired());
    }

    #[test]
    fn backend_refactor_learn_announcement_mapping_rejects_course_mismatch() {
        let dto = LearnAnnouncementDto {
            course_id: "different-course-id".into(),
            announcement_id: "private-announcement-id".into(),
            title: "Private title".into(),
            publisher: None,
            content: None,
            published_at: Utc::now().to_rfc3339(),
            expires_at: None,
            read: None,
            important: None,
            favorited: None,
            expired: false,
        };

        let error = map_course_announcement(dto, "selected-course-id").unwrap_err();

        assert_eq!(error.service(), Service::Learn);
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_learn_public_debug_hides_selectors_and_submission_text() {
        let client_id = uuid::Uuid::new_v4();
        let course = CourseRef::new(client_id, 1, "private-course-id".into());
        let homework = HomeworkRef::new(
            client_id,
            1,
            1,
            "private-course-id".into(),
            "private-assignment-selector".into(),
            true,
        );
        let detail = HomeworkDetail::new(
            Some("private assignment prompt".into()),
            Some("private reference answer".into()),
            Some("private student response".into()),
            vec![HomeworkAttachment::new(
                HomeworkAttachmentKind::Submitted,
                "private-submission.pdf".into(),
                Some("3 MB".into()),
            )],
        );
        let rendered = format!("{course:?} {homework:?} {detail:?}");

        for secret in [
            "private-course-id",
            "private-assignment-selector",
            "private assignment prompt",
            "private reference answer",
            "private student response",
            "private-submission.pdf",
        ] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn backend_refactor_learn_announcement_debug_hides_announcement_content() {
        let announcement = CourseAnnouncement::new(
            "private announcement title".into(),
            Some("private publisher".into()),
            Some("private announcement body".into()),
            Utc::now(),
            None,
            Some(false),
            Some(true),
            None,
            false,
        );
        let rendered = format!("{announcement:?}");

        for secret in [
            "private announcement title",
            "private publisher",
            "private announcement body",
        ] {
            assert!(!rendered.contains(secret));
        }
    }

    #[test]
    fn usereg_diagnostics_keep_auth_and_read_failures_distinct() {
        assert_eq!(
            usereg_error_code("usereg_http_auth_rejected", true),
            ErrorCode::AuthenticationRejected
        );
        assert_eq!(
            usereg_error_code("usereg_http_auth_rejected", false),
            ErrorCode::SessionExpired
        );
        assert_eq!(
            usereg_error_code("usereg_rate_limited", false),
            ErrorCode::RateLimited
        );
        assert_eq!(
            usereg_error_code("usereg_transport", false),
            ErrorCode::NetworkUnavailable
        );
        assert_eq!(
            usereg_error_code("usereg_balance_invalid", false),
            ErrorCode::InvalidResponse
        );
    }

    #[test]
    fn portal_observations_only_report_registration_for_the_address() {
        let observation = portal_observation(TunetNetworkStatusDto {
            state: "online".into(),
            online: true,
            signal: "positive".into(),
        })
        .unwrap();

        assert_eq!(
            observation.registration(),
            PortalAddressRegistration::Registered
        );
        assert_eq!(
            portal_observation(TunetNetworkStatusDto {
                state: "offline".into(),
                online: false,
                signal: "negative".into(),
            })
            .unwrap()
            .registration(),
            PortalAddressRegistration::NotRegistered
        );
        assert_eq!(
            portal_observation(TunetNetworkStatusDto {
                state: "unknown".into(),
                online: false,
                signal: "unknown".into(),
            })
            .unwrap()
            .registration(),
            PortalAddressRegistration::Unknown
        );
    }

    #[test]
    fn contradictory_portal_status_is_not_reported_as_online_or_offline() {
        let error = portal_observation(TunetNetworkStatusDto {
            state: "online".into(),
            online: false,
            signal: "positive".into(),
        })
        .unwrap_err();

        assert_eq!(error.service(), Service::Network);
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_network_profiles_are_rust_owned_versioned_and_not_auth_accounts() {
        let mut client = Client::builder().build().unwrap();
        let (portal, eap, portal_fill, eap_fill) = {
            let mut network = client.network();
            let mut profiles = network.profiles();
            let portal = profiles
                .save(
                    NetworkProfileInput::new(
                        "Portal login",
                        "shared-display-account",
                        crate::network::NetworkAccessMethod::Portal,
                    )
                    .unwrap()
                    .save_password("portal-only-password")
                    .unwrap(),
                )
                .unwrap();
            let eap = profiles
                .save(
                    NetworkProfileInput::new(
                        "Secure Wi-Fi",
                        "shared-display-account",
                        crate::network::NetworkAccessMethod::SystemWifiEap,
                    )
                    .unwrap(),
                )
                .unwrap();
            let fill = profiles.prepare_fill(portal.id()).unwrap();
            let eap_fill = profiles.prepare_fill(eap.id()).unwrap();
            assert!(profiles.is_current(&fill));
            assert!(profiles.is_current(&eap_fill));
            let portal_password = profiles.password_for_fill(&fill).unwrap().unwrap();
            assert_eq!(portal_password.expose_for_form(), "portal-only-password");
            assert!(!format!("{portal_password:?}").contains("portal-only-password"));
            assert_eq!(profiles.list().len(), 2);
            (portal, eap, fill, eap_fill)
        };

        assert_ne!(portal.id(), eap.id());
        assert_eq!(portal.method(), crate::network::NetworkAccessMethod::Portal);
        assert_eq!(
            eap.method(),
            crate::network::NetworkAccessMethod::SystemWifiEap
        );
        assert!(portal.has_saved_password());
        assert!(!eap.has_saved_password());

        let changed = {
            let mut network = client.network();
            let mut profiles = network.profiles();
            let changed = profiles
                .update(
                    portal.id(),
                    NetworkProfileInput::new(
                        "Secure Wi-Fi for another account",
                        "different-account",
                        crate::network::NetworkAccessMethod::SystemWifiEap,
                    )
                    .unwrap(),
                )
                .unwrap();
            assert!(!profiles.is_current(&portal_fill));
            assert_eq!(
                profiles.password_for_fill(&portal_fill).unwrap_err().code(),
                ErrorCode::ContextMismatch
            );
            assert!(!changed.has_saved_password());
            let fill = profiles.prepare_fill(changed.id()).unwrap();
            assert!(profiles.is_current(&fill));
            assert!(profiles.password_for_fill(&fill).unwrap().is_none());
            assert!(profiles.delete(eap.id()).unwrap());
            assert!(!profiles.is_current(&eap_fill));
            assert_eq!(
                profiles.password_for_fill(&eap_fill).unwrap_err().code(),
                ErrorCode::ContextMismatch
            );
            assert!(profiles.is_current(&fill));
            changed
        };

        assert_eq!(changed.username(), "different-account");
        let mut other = Client::builder().build().unwrap();
        assert!(!other.network().profiles().is_current(&portal_fill));
        assert_eq!(
            other
                .network()
                .profiles()
                .password_for_fill(&portal_fill)
                .unwrap_err()
                .code(),
            ErrorCode::ContextMismatch
        );
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
        assert_eq!(
            client.auth_status().self_service().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_electricity_reads_require_identity_before_dispatch() {
        let mut client = Client::builder().build().unwrap();
        let (remainder_error, history_error) = {
            let mut electricity = client.electricity();
            (
                electricity.remainder().await.unwrap_err(),
                electricity.payment_history().await.unwrap_err(),
            )
        };

        for error in [remainder_error, history_error] {
            assert_eq!(error.service(), Service::Electricity);
            assert_eq!(error.code(), ErrorCode::SessionRequired);
        }
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[test]
    fn backend_refactor_electricity_results_preserve_cache_provenance() {
        let generated_at = Utc::now().to_rfc3339();
        let fresh = cached_read_metadata(
            Service::Electricity,
            &generated_at,
            "cache",
            "ready",
            false,
            ReadSource::PersistentCache,
        )
        .unwrap();
        assert_eq!(fresh.source(), ReadSource::PersistentCache);
        assert_eq!(fresh.freshness(), CacheFreshness::Fresh);
        assert_eq!(fresh.refresh_failure(), None);

        let stale = cached_read_metadata(
            Service::Electricity,
            &generated_at,
            "cache",
            "stale",
            true,
            ReadSource::ClientCache,
        )
        .unwrap();
        assert_eq!(stale.source(), ReadSource::ClientCache);
        assert_eq!(stale.freshness(), CacheFreshness::Stale);
        assert_eq!(stale.refresh_failure(), Some(ErrorCode::ServiceUnavailable));
    }

    #[tokio::test]
    async fn self_service_device_reference_is_bound_to_its_originating_client() {
        let owner = Client::builder().build().unwrap();
        let mut other = Client::builder().build().unwrap();
        let reference = DeviceRef::new(owner.instance_id, 1, 0);

        let error = other
            .self_service()
            .disconnect_device(&reference)
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::SelfService);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            owner.auth_status().self_service().state(),
            AccountAuthState::SignedOut
        );
        assert_eq!(
            other.auth_status().self_service().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_news_article_rejects_foreign_and_expired_references_before_io() {
        let owner = Client::builder().build().unwrap();
        let mut other = Client::builder().build().unwrap();
        let foreign = ArticleRef::new(owner.instance_id, 1, "article-a".to_owned());
        let stale = ArticleRef::new(other.instance_id, 0, "article-b".to_owned());

        let foreign_error = other
            .news()
            .article(&foreign, crate::read::ReadPolicy::Refresh)
            .await
            .unwrap_err();
        let stale_error = other
            .news()
            .article(&stale, crate::read::ReadPolicy::Refresh)
            .await
            .unwrap_err();

        assert_eq!(foreign_error.service(), Service::News);
        assert_eq!(foreign_error.code(), ErrorCode::ContextMismatch);
        assert_eq!(stale_error.service(), Service::News);
        assert_eq!(stale_error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            other.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_news_filter_rejects_foreign_and_stale_catalog_refs_before_io() {
        let owner = Client::builder().build().unwrap();
        let mut other = Client::builder().build().unwrap();
        let foreign = NewsSourceRef::new(owner.instance_id, 1, "private-source-id".to_owned());
        let query = NewsQuery::list(1, 20)
            .unwrap()
            .with_source(&foreign)
            .unwrap();
        let foreign_error = other
            .news()
            .articles(query, crate::read::ReadPolicy::Refresh)
            .await
            .unwrap_err();

        let stale = NewsChannelRef::new(
            other.instance_id,
            1,
            "private-channel-id".to_owned(),
            "private-channel-label".to_owned(),
        );
        other.news_filter_generation = 2;
        let query = NewsQuery::list(1, 20)
            .unwrap()
            .with_channel(&stale)
            .unwrap();
        let stale_error = other
            .news()
            .articles(query, crate::read::ReadPolicy::Refresh)
            .await
            .unwrap_err();

        assert_eq!(foreign_error.service(), Service::News);
        assert_eq!(foreign_error.code(), ErrorCode::ContextMismatch);
        assert_eq!(stale_error.service(), Service::News);
        assert_eq!(stale_error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            other.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_news_subscription_ref_is_checked_before_auth_or_io() {
        let mut client = Client::builder().build().unwrap();
        client.news_subscription_generation = 1;
        let reference =
            NewsSubscriptionRef::new(client.instance_id, 1, "private-runtime-selector".to_owned());

        let error = client
            .news()
            .subscription_articles(&reference, 1)
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::News);
        assert_eq!(error.code(), ErrorCode::ContextMismatch);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[tokio::test]
    async fn backend_refactor_news_cache_only_miss_does_not_start_identity_auth() {
        let mut client = Client::builder().build().unwrap();
        let error = client
            .news()
            .articles(
                NewsQuery::list(1, 20).unwrap(),
                crate::read::ReadPolicy::CacheOnly,
            )
            .await
            .unwrap_err();

        assert_eq!(error.service(), Service::News);
        assert_eq!(error.code(), ErrorCode::CacheMiss);
        assert_eq!(
            client.auth_status().identity().state(),
            AccountAuthState::SignedOut
        );
    }

    #[test]
    fn backend_refactor_info_error_mapping_exposes_only_stable_categories() {
        assert_eq!(
            info_error_code("info_news_cache_miss"),
            ErrorCode::CacheMiss
        );
        assert_eq!(
            info_error_code("info_transport"),
            ErrorCode::NetworkUnavailable
        );
        assert_eq!(
            info_error_code("info_detail_legacy_html"),
            ErrorCode::InvalidResponse
        );
        assert_eq!(
            info_error_code("raw private response body"),
            ErrorCode::ServiceUnavailable
        );
    }

    #[test]
    fn backend_refactor_news_metadata_keeps_cache_freshness_and_refresh_failure() {
        let client = Client::builder().build().unwrap();
        let generated_at = Utc::now().to_rfc3339();
        let fresh = news_metadata(
            &client.runtime,
            Some(&generated_at),
            Some("cache"),
            Some("ready"),
            None,
            ReadSource::ClientCache,
        )
        .unwrap();
        assert_eq!(fresh.source(), ReadSource::ClientCache);
        assert_eq!(fresh.freshness(), CacheFreshness::Fresh);

        let stale = news_metadata(
            &client.runtime,
            Some(&generated_at),
            Some("cache"),
            Some("stale"),
            Some("runtime refresh error text"),
            ReadSource::PersistentCache,
        )
        .unwrap();
        assert_eq!(stale.source(), ReadSource::PersistentCache);
        assert_eq!(stale.freshness(), CacheFreshness::Stale);
        assert_eq!(stale.refresh_failure(), Some(ErrorCode::ServiceUnavailable));
        assert!(!format!("{stale:?}").contains("runtime refresh error text"));
    }

    #[test]
    fn backend_refactor_registrar_metadata_preserves_cache_provenance() {
        let generated_at = Utc::now().to_rfc3339();
        let stale = registrar_metadata(
            &generated_at,
            "cache",
            "stale",
            true,
            ReadSource::PersistentCache,
        )
        .unwrap();

        assert_eq!(stale.source(), ReadSource::PersistentCache);
        assert_eq!(stale.freshness(), CacheFreshness::Stale);
        assert_eq!(stale.refresh_failure(), Some(ErrorCode::ServiceUnavailable));
        assert_eq!(stale.coverage(), ReadCoverage::Complete);

        let invalid = registrar_metadata(
            "not-a-timestamp",
            "cache",
            "ready",
            false,
            ReadSource::ClientCache,
        )
        .unwrap_err();
        assert_eq!(invalid.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_classroom_mapping_checks_references_and_weekly_matrix() {
        let owner = uuid::Uuid::new_v4();
        let building = map_classroom_building_records(
            vec![crate::api::runtime::ClassroomBuildingDto {
                name: "Fixture Hall".into(),
                week_number: 2,
            }],
            owner,
            5,
        )
        .unwrap()
        .remove(0);
        let reference = building.reference().unwrap();
        assert_eq!(building.name(), "Fixture Hall");
        assert_eq!(building.default_week(), ClassroomWeek::new(2));
        assert_eq!(format!("{reference:?}"), "BuildingRef(<redacted>)");

        let monday = chrono::NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let dates = (0..7)
            .map(|offset| {
                (monday + chrono::Days::new(offset))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .collect();
        let matrix = ClassroomStateResultDto {
            valid_week_numbers: vec![1, 2],
            current_week_number: 2,
            dates_of_current_week: dates,
            classroom_states: vec![crate::api::runtime::ClassroomStateDto {
                name: "Room 101".into(),
                statuses: vec!["free".into(); 42],
            }],
        };
        let availability = map_classroom_availability(
            matrix.clone(),
            reference.clone(),
            ClassroomWeek::new(2).unwrap(),
        )
        .unwrap();
        assert_eq!(availability.week().get(), 2);
        assert_eq!(availability.week_dates()[0], monday);
        assert_eq!(availability.rooms()[0].slots().len(), 42);

        let mut invalid_matrix = matrix;
        invalid_matrix.classroom_states[0].statuses.pop();
        let error = map_classroom_availability(
            invalid_matrix,
            reference.clone(),
            ClassroomWeek::new(2).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.service(), Service::Classrooms);
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }

    #[tokio::test]
    async fn backend_refactor_classroom_foreign_and_stale_refs_fail_before_runtime() {
        let mut client = Client::builder().build().unwrap();
        let week = ClassroomWeek::new(1).unwrap();
        let foreign = BuildingRef::new(uuid::Uuid::new_v4(), 0, 0, week);
        let foreign_error = client
            .classrooms()
            .availability(&foreign, ClassroomWeekSelection::BuildingDefault)
            .await
            .unwrap_err();
        assert_eq!(foreign_error.service(), Service::Classrooms);
        assert_eq!(foreign_error.code(), ErrorCode::ContextMismatch);

        let owner = client.instance_id;
        client.classroom_generation = 1;
        let stale = BuildingRef::new(owner, 0, 0, week);
        let stale_error = client
            .classrooms()
            .availability(&stale, ClassroomWeekSelection::BuildingDefault)
            .await
            .unwrap_err();
        assert_eq!(stale_error.service(), Service::Classrooms);
        assert_eq!(stale_error.code(), ErrorCode::ContextMismatch);
    }

    #[test]
    fn backend_refactor_classroom_and_library_cached_metadata_keeps_provenance() {
        let generated_at = Utc::now().to_rfc3339();
        let fresh = cached_read_metadata(
            Service::Library,
            &generated_at,
            "cache",
            "ready",
            false,
            ReadSource::PersistentCache,
        )
        .unwrap();
        assert_eq!(fresh.source(), ReadSource::PersistentCache);
        assert_eq!(fresh.freshness(), CacheFreshness::Fresh);
        assert_eq!(fresh.refresh_failure(), None);

        let stale = cached_read_metadata(
            Service::Classrooms,
            &generated_at,
            "cache",
            "stale",
            true,
            ReadSource::ClientCache,
        )
        .unwrap();
        assert_eq!(stale.source(), ReadSource::ClientCache);
        assert_eq!(stale.freshness(), CacheFreshness::Stale);
        assert_eq!(stale.refresh_failure(), Some(ErrorCode::ServiceUnavailable));

        let invalid = cached_read_metadata(
            Service::Classrooms,
            "not-a-timestamp",
            "cache",
            "ready",
            false,
            ReadSource::ClientCache,
        )
        .unwrap_err();
        assert_eq!(invalid.service(), Service::Classrooms);
        assert_eq!(invalid.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_registrar_grade_mapping_checks_stage_and_redacts_rows() {
        let mapped = map_grade_report(CampusGradeReportDto {
            stage: "undergraduate".into(),
            report_kind: Some("first_degree".into()),
            courses: vec![crate::api::academic::CampusGradeDto {
                course_name: "private course title".into(),
                credit: 3.0,
                grade: "A".into(),
                grade_point: Some(4.0),
                semester: "2026-2027-1".into(),
            }],
            generated_at: None,
            source: None,
            status: None,
            error: None,
        })
        .unwrap();
        assert_eq!(mapped.stage(), AcademicStage::Undergraduate);
        assert_eq!(mapped.kind(), Some(GradeReportKind::FirstDegree));
        assert_eq!(mapped.courses().len(), 1);
        assert!(!format!("{mapped:?}").contains("private course title"));

        let invalid = map_grade_report(CampusGradeReportDto {
            stage: "graduate".into(),
            report_kind: Some("first_degree".into()),
            courses: Vec::new(),
            generated_at: None,
            source: None,
            status: None,
            error: None,
        })
        .unwrap_err();
        assert_eq!(invalid.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_registrar_schedule_mapping_uses_typed_dates_and_hides_ids() {
        let schedule = map_semester_schedule(CampusSemesterScheduleDto {
            semester: "2026-2027-1".into(),
            stage: "graduate".into(),
            first_day: "2026-09-01".into(),
            last_day: "2027-01-20".into(),
            week_count: 21,
            current_week: 3,
            generated_at: Utc::now().to_rfc3339(),
            source: "live".into(),
            status: "ready".into(),
            error: None,
            events: vec![CampusScheduleDto {
                id: "private-event-id".into(),
                title: "private schedule title".into(),
                kind: "course".into(),
                starts_at: "2026-09-07T01:00:00Z".into(),
                ends_at: Some("2026-09-07T02:35:00Z".into()),
                all_day: false,
                location: Some("private room".into()),
                course_id: Some("private-course-id".into()),
                description: Some("private schedule detail".into()),
            }],
        })
        .unwrap();

        assert_eq!(schedule.stage(), AcademicStage::Graduate);
        assert_eq!(
            schedule.first_day(),
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
        );
        assert_eq!(
            schedule.events()[0].starts_at().to_rfc3339(),
            "2026-09-07T01:00:00+00:00"
        );
        let rendered = format!("{schedule:?} {:?}", schedule.events()[0]);
        for private_value in [
            "private-event-id",
            "private-course-id",
            "private schedule title",
            "private room",
            "private schedule detail",
        ] {
            assert!(!rendered.contains(private_value));
        }
    }

    #[test]
    fn backend_refactor_registrar_exam_mapping_preserves_source_range_without_leaking_debug() {
        let exam = map_exam(CampusExamDto {
            course_code: "private-code".into(),
            course_sequence: "private-sequence".into(),
            course_name: "private exam course".into(),
            exam_month: 2,
            exam_day: 29,
            exam_weekday: "sunday".into(),
            exam_session: "08:00–10:00".into(),
            schedule_raw: "2028-02-29 08:00–10:00".into(),
            location: "private exam room".into(),
            department: Some("private department".into()),
            category: Some("private category".into()),
            instructor: Some("private instructor".into()),
            headcount: Some(40),
        })
        .unwrap();

        assert_eq!(exam.month(), 2);
        assert_eq!(exam.day(), 29);
        assert_eq!(exam.weekday(), ExamWeekday::Sunday);
        assert_eq!(exam.schedule_label(), "2028-02-29 08:00–10:00");
        let rendered = format!("{exam:?}");
        for private_value in [
            "private-code",
            "private-sequence",
            "private exam course",
            "private exam room",
            "private instructor",
        ] {
            assert!(!rendered.contains(private_value));
        }

        let invalid = map_exam(CampusExamDto {
            course_code: String::new(),
            course_sequence: String::new(),
            course_name: "Course".into(),
            exam_month: 4,
            exam_day: 5,
            exam_weekday: "monday-ish".into(),
            exam_session: "afternoon".into(),
            schedule_raw: "2026-04-05".into(),
            location: String::new(),
            department: None,
            category: None,
            instructor: None,
            headcount: None,
        });
        assert_eq!(invalid.unwrap_err().code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_calendar_term_mapping_checks_week_alignment_and_hides_ids() {
        let make_term = |id: &str, label: &str, start: &str, end: &str, first: &str| {
            let end_date = NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap();
            let first_date = NaiveDate::parse_from_str(first, "%Y-%m-%d").unwrap();
            LearnTermDto {
                id: id.into(),
                label: label.into(),
                start_date: start.into(),
                end_date: end.into(),
                first_day: first.into(),
                week_count: ((end_date - first_date).num_days().div_euclid(7) + 1) as u32,
            }
        };
        let terms = map_learn_term_calendar(LearnTermCalendarDto {
            current: make_term(
                "private-current-term-id",
                "private current label",
                "2026-09-02",
                "2027-01-15",
                "2026-08-31",
            ),
            upcoming: vec![make_term(
                "private-upcoming-term-id",
                "private upcoming label",
                "2027-02-01",
                "2027-06-20",
                "2027-02-01",
            )],
            generated_at: Utc::now().to_rfc3339(),
            source: "live".into(),
            status: "ready".into(),
        })
        .unwrap();

        assert_eq!(terms.upcoming().len(), 1);
        assert_eq!(
            terms
                .current()
                .teaching_week_one()
                .weekday()
                .number_from_monday(),
            1
        );
        let rendered = format!("{terms:?}");
        for private_value in [
            "private-current-term-id",
            "private-upcoming-term-id",
            "private current label",
            "private upcoming label",
        ] {
            assert!(!rendered.contains(private_value));
        }

        let mut seen = std::collections::HashSet::new();
        let first = make_term(
            "same-term",
            "Current",
            "2026-09-02",
            "2027-01-15",
            "2026-08-31",
        );
        let repeated = LearnTermDto {
            id: "same-term".into(),
            ..make_term(
                "different-id",
                "Next",
                "2027-02-01",
                "2027-06-20",
                "2027-02-01",
            )
        };
        map_learn_term(first, &mut seen).unwrap();
        assert_eq!(
            map_learn_term(repeated, &mut seen).unwrap_err().code(),
            ErrorCode::InvalidResponse
        );
    }

    #[test]
    fn backend_refactor_school_calendar_mapping_binds_selection_and_hides_image_bytes() {
        let query = SchoolCalendarQuery::for_year(
            2026,
            crate::calendar_api::SchoolCalendarSemester::Autumn,
            crate::calendar_api::SchoolCalendarLanguage::Chinese,
        )
        .unwrap();
        let bytes = include_bytes!("fixtures/school_calendar.jpg").to_vec();
        let image = map_school_calendar(
            SchoolCalendarDto {
                latest_year: 2026,
                year: 2026,
                semester: "autumn".into(),
                language: "zh".into(),
                image_bytes: bytes.clone(),
                generated_at: Utc::now().to_rfc3339(),
                source: "live".into(),
                status: "ready".into(),
            },
            query,
        )
        .unwrap();
        assert_eq!(image.bytes(), bytes);
        assert_eq!(image.year(), 2026);
        assert!(!format!("{image:?}").contains(&format!("{bytes:?}")));

        let mismatch = map_school_calendar(
            SchoolCalendarDto {
                latest_year: 2026,
                year: 2026,
                semester: "spring".into(),
                language: "zh".into(),
                image_bytes: bytes,
                generated_at: Utc::now().to_rfc3339(),
                source: "live".into(),
                status: "ready".into(),
            },
            query,
        )
        .unwrap_err();
        assert_eq!(mismatch.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_calendar_metadata_rejects_unverified_provenance() {
        let error = calendar_metadata(&Utc::now().to_rfc3339(), "cache", "ready").unwrap_err();
        assert_eq!(error.service(), Service::Calendar);
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
        assert_eq!(
            SchoolCalendarQuery::for_year(
                1999,
                crate::calendar_api::SchoolCalendarSemester::Autumn,
                crate::calendar_api::SchoolCalendarLanguage::Chinese,
            )
            .unwrap_err()
            .code(),
            ErrorCode::InvalidInput
        );
    }

    #[test]
    fn backend_refactor_identity_cookie_restore_remains_unverified_and_self_service_separate() {
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "tsinghua-kit-identity-restore-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        set_private_directory_permissions(&root).unwrap();
        let _cleanup = RemoveTestDirectory(root.clone());

        let fingerprint = "0123456789abcdef0123456789abcdef";
        let marker_path = root.join(".thyou-device-fingerprint");
        let mut marker = crate::telemetry::new_private_file(&marker_path).unwrap();
        marker.write_all(fingerprint.as_bytes()).unwrap();
        marker.sync_all().unwrap();

        let cookie_store = crate::transport::CampusCookieStore::default();
        let cookie_origin = reqwest::Url::parse("https://identity.fixture.invalid/").unwrap();
        cookie_store.add_cookie_str(
            "fixture_identity=present; Path=/; Secure; HttpOnly",
            &cookie_origin,
        );
        let cookie_snapshot = cookie_store.snapshot_bytes().unwrap();
        let snapshot = crate::session_persistence::ResumeSnapshot::new(
            "fixture-identity-account",
            &cookie_snapshot,
        )
        .unwrap()
        .with_device_fingerprint(fingerprint)
        .unwrap();
        let metadata = crate::session_persistence::ResumeAccountMetadata::new_with_stage_selection(
            "fixture-identity-account",
            None,
            Some(fingerprint),
            false,
            false,
        )
        .unwrap();
        crate::session_persistence::save_resume_state_at_root(&root, &snapshot, &metadata).unwrap();

        let client = ClientBuilder::default()
            .cache_policy(ClientCachePolicy::Directory(root.clone()))
            .identity_session_storage(IdentitySessionStoragePolicy::EncryptedDirectory(
                root.clone(),
            ))
            .build()
            .unwrap();
        let status = client.auth_status();
        assert_eq!(
            status.identity().state(),
            AccountAuthState::RestoredUnverified
        );
        assert_eq!(
            status.identity().username(),
            Some("fixture-identity-account")
        );
        assert_eq!(status.self_service().state(), AccountAuthState::SignedOut);
        assert_eq!(status.self_service().username(), None);
    }
}
