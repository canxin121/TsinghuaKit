#![doc = include_str!("../README.md")]
#![allow(unexpected_cfgs)]
// FRB 2.13 emits one newer lint name; keep older supported Rust toolchains
// quiet while leaving the generated file itself untouched.
#![allow(unknown_lints)]
#![recursion_limit = "256"]

pub mod api;
pub mod auth;
pub mod cache;
pub mod calendar_api;
pub mod campus_card_adapter;
pub mod campus_card_api;
pub mod campus_card_read;
pub mod campus_live;
pub mod classroom_read;
pub mod classrooms_api;
pub mod client;
pub mod domain;
pub mod dorm_electricity_read;
pub mod electricity_api;
pub mod error;
pub mod identity;
pub mod identity_client;
pub mod identity_crypto;
pub mod identity_execution;
pub mod identity_session;
pub mod info;
pub mod info_client;
pub mod info_news;
pub mod info_session;
pub mod learn;
pub mod learn_announcements;
pub mod learn_api;
pub mod learn_client;
mod learn_discussions;
mod learn_file_download;
mod learn_files;
mod learn_homework;
pub mod learn_session;
pub mod learn_todos;
pub mod library_api;
pub mod library_read;
pub mod network;
mod network_profile_store;
pub mod news;
pub mod overview_api;
pub mod protocol;
pub mod read;
pub mod registrar;
pub mod registrar_academic;
pub mod registrar_api;
pub mod registrar_client;
pub mod registrar_session;
pub mod self_service;
pub mod service_catalog;
pub mod service_hall;
pub mod services;
pub mod session;
pub mod telemetry;
pub mod transport;
pub mod tunet;
pub mod tunet_auth;
pub mod tunet_client;
pub mod usereg;
pub mod usereg_adapter;
pub mod usereg_client;
pub mod webvpn_identity;

mod captcha_image;
mod credential_store;
mod error_legacy;
mod error_sdk;
mod live_validation;
mod portal_resume;
mod request_gate;
mod school_calendar;
pub(crate) mod session_persistence;
mod thos;
mod webvpn_url;

pub use error::{Error, ErrorCode, Service};

pub use api::academic::{CampusGradeDto, CampusGradeReportDto};
pub use api::campus::{
    CampusOverviewDto, CampusOverviewFacadeDto, CampusOverviewResolver,
    CampusOverviewSectionErrorsDto, CampusScheduleDto, CampusSemesterScheduleDto, CampusTodoDto,
};
pub use api::runtime::{
    CampusCardTransactionsResultDto, CampusRuntime, CampusRuntimeStatusDto, ClassroomBuildingDto,
    ClassroomBuildingsResultDto, ClassroomStateDto, ClassroomStateResultDto,
    ElectricityPaymentHistoryDto, ElectricityPaymentHistoryResultDto, ElectricityPaymentRecordDto,
    ElectricityRemainderDto, UseregCaptchaDto, create_runtime,
};
pub use api::runtime::{
    InfoNewsAttachmentDto, InfoNewsDetailDto, InfoNewsDetailResultDto, InfoNewsItemDto,
    InfoNewsPageDto, LearnAnnouncementDto, LearnCourseListDto, LibraryAreaTreeResultDto,
    LibraryDaySegmentsResultDto,
};
pub use api::service_catalog::{
    ServiceCapabilityMetadataDto, ServiceCatalogDto, ServiceCatalogEntryDto,
    ServicePresentationDto, list_service_catalog,
};
pub use cache::{CacheError, JsonCacheEnvelope, JsonFileCache};
pub use campus_card_adapter::{
    CampusCardAdapterConfig, CampusCardAdapterError, CampusCardClient, CampusCardSession,
    CampusCardSsoProfile,
};
pub use campus_card_read::{
    CampusCardAccount, CampusCardAccountBinding, CampusCardDate, CampusCardParseError,
    CampusCardReadProfile, CampusCardRequestError, CampusCardRequestPlan, CampusCardTransaction,
    CampusCardTransactionQuery, CampusCardTransactionReport, parse_card_account_response,
    parse_card_transactions_response,
};
pub use campus_live::{
    CampusLiveConfig, CampusLiveDataSource, CampusTodoSource, DynCampusTodoSource,
    UnavailableTodoSource, map_calendar_records, map_learn_courses,
};
pub use classroom_read::{
    CLASSROOM_LIST_PATH, CLASSROOM_LIST_QUERY, CLASSROOM_ROAM_SELECTOR, CLASSROOM_STATE_PATH,
    CLASSROOM_STATUS_SLOT_COUNT, ClassroomAdapterConfig, ClassroomAdapterError, ClassroomBuilding,
    ClassroomBuildingList, ClassroomBusinessFailure, ClassroomHttpMethod, ClassroomOperation,
    ClassroomParseError, ClassroomReadAdapter, ClassroomReadProfile, ClassroomRequestError,
    ClassroomRequestPlan, ClassroomSessionPrerequisite, ClassroomSessionProof, ClassroomSlotStatus,
    ClassroomState, ClassroomStateResult, gb2312_percent_encode,
};
pub use client::{
    Client, ClientBuilder, ClientCachePolicy, CredentialStoragePolicy, IdentityLoginOutcome,
    IdentityLoginRequest, IdentitySessionStoragePolicy, LoginStage, ServiceHallClient,
};
pub use domain::*;
pub use dorm_electricity_read::{
    DormElectricityAdapter, DormElectricityAdapterConfig, DormElectricityAdapterError,
    DormElectricityMethod, DormElectricityOperation, DormElectricityParseError,
    DormElectricityProfile, DormElectricityRequestPlan, DormElectricitySessionPrerequisite,
    ELECTRICITY_PAYMENT_HISTORY_PATH, ELECTRICITY_REMAINDER_PATH, ELECTRICITY_WEBVPN_TARGET,
    ElectricityPaymentHistory, ElectricityPaymentRecord, ElectricityRemainder,
    parse_electricity_payment_history_html, parse_electricity_remainder_html,
};
pub use error::{DomainError, ServiceError};
pub use identity::TrustedDeviceProfile;
pub use identity_client::{
    CsrfEvidence as IdentityCsrfEvidence, FailureEvidenceSource, FormValue, IdentityClient,
    IdentityClientConfig, IdentityClientError, LoginFailureEvidence, LoginFailureMarker,
    LoginFailureReason, LoginFailureTextMarker, LoginFormInput, LoginPageEvidence,
    LoginPageRequestPlan, LoginResponseClassification, LoginSubmissionRequestPlan,
    MultipartBoundaryPlan, MultipartLoginRequestPlan, PasswordInput, TrustedDeviceField,
    TrustedDeviceInput, TrustedDeviceRequestPlan, UrlEncodedField, UrlEncodedLoginForm,
    UrlEncodedLoginRequestPlan, UrlEncodedSecondAuthForm, UrlEncodedTrustedDeviceForm,
};
pub use identity_crypto::{Sm2PasswordError, encrypt_password};
pub use identity_execution::{
    IdentityExecutionClient, IdentityExecutionError, IdentityHttpResponse,
};
pub use identity_session::{
    IdentitySecondFactorChallenge, IdentitySessionError, IdentitySessionOrchestrator,
    IdentitySessionOutcome, IdentitySessionResult, TrustedDeviceOptions, TrustedDeviceResult,
    TrustedDeviceStatus,
};
pub use info::{
    InfoError, InfoParameterEncoding, InfoPortalProfile, InfoPortalRequestPlan, InfoPortalResponse,
    OpaqueUrl, WebVpnUrl, extract_roaming_url, parse_online_app_redirect,
};
pub use info_client::{
    InfoClient, InfoClientConfig, InfoClientError, InfoHtmlClassification, InfoHtmlEvidence,
    InfoHtmlSignal, InfoHttpResponse, InfoRequestPlan, InfoResponseClassification,
};
pub use info_news::{
    NewsAttachment, NewsChannelId, NewsDetail, NewsFeedKind, NewsItem, NewsLink, NewsPage,
    NewsParseError, NewsParseOutcome, NewsProfile, NewsProfileError, NewsRequestPlan,
    NewsSearchInput, parse_news_detail, parse_news_list, parse_news_page, parse_news_search,
};
pub use info_session::{InfoSessionAdapter, InfoSessionError, InfoSessionResult, InfoWebVpnConfig};
pub use learn::{
    DEFAULT_ROAMING_ENTRY_PATH, LearnCourseRequest, LearnError, LearnProfile, LearnRoamingRequest,
};
pub use learn_announcements::{
    LearnAnnouncement, LearnAnnouncementBucket, LearnAnnouncementConfig, LearnAnnouncementError,
    LearnAnnouncementParseError, LearnAnnouncementRequestMethod, LearnAnnouncementRequestPlan,
    LearnAnnouncementSource, STUDENT_ACTIVE_PATH, STUDENT_EXPIRED_PATH, TEACHER_ACTIVE_PATH,
    TEACHER_EXPIRED_PATH, parse_announcement_list,
};
pub use learn_client::{
    CourseJsonError, CsrfEvidence, CsrfParseError, CsrfSource, LearnClient, LearnClientConfig,
    LearnClientError, LearnCourseHomeRequestPlan, LearnCourseListRequestPlan, LearnCourseRecord,
    LearnHtmlResponse, LearnHttpResponse, LearnPageClassification, LearnRequestMethod,
    LearnRoamingQuery, LearnRoamingRequestPlan, LoginExpiredEvidence, LoginExpiredSignal,
    extract_csrf, extract_csrf_with_field, parse_course_record, parse_course_records,
};
pub use learn_session::{
    LearnSessionError, LearnSessionOrchestrator, LearnSessionResult, bind_identity_ticket,
};
pub use learn_todos::{
    HomeworkBucket, LearnHomeworkRecord, LearnTodoConfig, LearnTodoParseError, LearnTodoSource,
    parse_homework_list,
};
pub use library_read::{
    LibraryArea, LibraryAreaTree, LibraryDaySegment, LibraryDaySegments, LibraryReadParseError,
    LibraryReadProfile, LibraryReadRequestPlan, LibrarySeat, LibrarySeatAvailability,
    LibrarySocketState, LibrarySocketStatusAdapter, LibrarySocketStatusRecord,
    LibrarySocketStatusRecordDto, LibrarySocketStatuses, LibrarySocketStatusesDto, parse_area_tree,
    parse_day_segments, parse_seat_availability, parse_socket_status,
};
pub use protocol::*;
pub use registrar::{
    CalendarWindow, RegistrarCalendarRequest, RegistrarError, RegistrarProfile,
    split_calendar_range,
};
pub use registrar_academic::{
    GRADES_PATH, RegistrarCourseGrade, RegistrarGradeField, RegistrarGradeReport,
    RegistrarGradesAuthentication, RegistrarGradesParseError, RegistrarGradesProfile,
    RegistrarGradesProfileError, RegistrarGradesRequest, RegistrarQueryParameter,
    RegistrarRequestMethod, UndergraduateReportKind, parse_grades_html,
};
pub use registrar_client::{
    AllZhjwTicketExchangePlan, AllZhjwTicketProfile, AllZhjwTicketResponseError,
    ParameterPlacement, RegistrarCalendarProfile, RegistrarCalendarRecord,
    RegistrarCalendarRequestPlan, RegistrarClient, RegistrarClientConfig, RegistrarClientError,
    RegistrarEvent, RegistrarExamError, RegistrarExamHttpResponse, RegistrarExamRecord,
    RegistrarExamRecordSource, RegistrarExamStage, RegistrarLoginProfile,
    RegistrarLoginRequestPlan, RegistrarPayloadError, RegistrarStageCalendarProfile,
    RegistrarVerifiedExamPageProfile, RegistrarVerifiedExamPageRequest, decode_calendar_jsonp,
    parse_all_zhjw_ticket_response, parse_verified_exam_page_html,
    parse_verified_exam_page_response,
};
pub use registrar_session::{
    RegistrarSessionError, RegistrarSessionOrchestrator, RegistrarSessionResult,
};
pub use service_catalog::{
    ALL_CATALOG_SERVICE_IDS, AuthenticationRequirement, CapabilityAccess, CatalogServiceId,
    STANDARD_SERVICE_DEFINITIONS, ServiceAvailability, ServiceCapabilities, ServiceCapability,
    ServiceCapabilityMetadata, ServiceCatalog, ServiceCatalogEntry, ServiceCategory,
    ServiceDefinition, ServicePresentation,
};
pub use services::{
    CampusDataSource, CampusOverviewSectionErrors, CampusOverviewSections, DynCampusDataSource,
    InMemoryCampusService, InMemoryData,
};
pub use session::{
    BoundCsrfToken, BoundServiceTicket, ServiceSessionStateMachine, SessionCoordinator,
    SessionCredentialKind, SessionError, SessionRegistry, SessionRegistrySnapshot, SessionSnapshot,
    SessionTransition,
};
pub use transport::{CampusHttpTransport, TransportError};
pub use tunet_auth::{
    SrunLoginError, SrunLoginMaterial, SrunPasswordDigestScheme, build_srun_login_material,
    srun_request_n, srun_request_type,
};
pub use tunet_client::{
    TunetChallenge, TunetClient, TunetClientConfig, TunetClientError, TunetExtraParams,
    TunetRequestPlan, TunetStatusEncoding, TunetStatusRecord, TunetStatusSignal,
    parse_challenge_response, parse_status_response,
};
pub use usereg::{
    UseregCertificationInput, UseregCertificationType, UseregCsrfToken, UseregDeviceTarget,
    UseregError, UseregFieldProfile, UseregHttpMethod, UseregLoginCredentials, UseregOperation,
    UseregPaths, UseregProfile, UseregRequestPlan,
};
pub use usereg_adapter::{
    UseregAdapter, UseregAdapterError, UseregAuthenticationResult, UseregAuthenticationState,
    UseregCaptchaImage, UseregCaptchaMetadata, UseregLoginPage, UseregValidationOutcome,
    UseregValidationResult,
};
pub use usereg_client::{
    UseregClient, UseregClientConfig, UseregClientError, UseregCsrfEvidence, UseregCsrfParseError,
    UseregExecutionPlan, UseregHtmlEvidence, UseregHtmlSignal, UseregHttpResponse,
    UseregMappedResponse, UseregPageClassification, extract_csrf as extract_usereg_csrf,
    extract_csrf_with_field as extract_usereg_csrf_with_field,
};
pub use webvpn_identity::{
    WebVpnIdentityBootstrap, WebVpnIdentityBootstrapper, WebVpnIdentityConfig, WebVpnIdentityError,
};

#[cfg(test)]
mod reference_test_support;

#[cfg(test)]
mod backend_api_audit_tests;

#[cfg(test)]
mod deep_api_audit_tests;

#[cfg(test)]
mod api_consistency_tests;

#[cfg(test)]
mod api_response_binding_tests;
