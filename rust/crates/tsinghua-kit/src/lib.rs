//! Rust-first Tsinghua campus-services SDK.
//!
//! The public crate exposes curated account, network, calendar, academic,
//! Learn, news, read-result, and service-hall contracts. Protocol clients,
//! cookies, authentication tickets, transport, parsers, and Flutter
//! compatibility details stay behind the SDK boundary.

mod client;

/// Storage choices for one Client. Cache data, Auth sessions, Auth passwords,
/// and local network profiles have separate policies.
pub mod config {
    pub use crate::client::{
        ClientCachePolicy, CredentialStoragePolicy, IdentitySessionStoragePolicy,
    };
    pub use tsinghua_kit_engine::network::NetworkProfileStoragePolicy;
}

pub mod auth {
    pub use crate::client::{
        AuthClient, IdentityAuthClient, SelfServiceAuthClient, SelfServiceLoginRequest,
    };
    pub use tsinghua_kit_engine::auth::{
        AccountAuthState, AccountAuthStatus, AuthDomain, AuthStatus, SecondFactorMethod,
        SelfServiceLoginPhase,
    };
    pub use tsinghua_kit_engine::client::{
        IdentityLoginOutcome, IdentityLoginRequest, LoginStage, SelfServiceCaptcha,
        SelfServiceLoginOutcome,
    };

    /// Suggests the academic login route from a supported student-id rule.
    ///
    /// `None` means the value could not be classified. This is a UI hint only;
    /// the caller may still choose another stage, and successful service
    /// responses remain the authority for academic data.
    pub fn suggest_login_stage(username: &str) -> Option<LoginStage> {
        match tsinghua_kit_engine::api::runtime::infer_academic_stage(username.to_owned())? {
            false => Some(LoginStage::Undergraduate),
            true => Some(LoginStage::Graduate),
        }
    }
}

/// School-wide published calendars and Learn academic-term dates.
pub mod calendar {
    pub use crate::client::CalendarClient;
    pub use chrono::NaiveDate;
    pub use tsinghua_kit_engine::calendar_api::{
        AcademicTerm, LearnTermCalendar, SchoolCalendarImage, SchoolCalendarLanguage,
        SchoolCalendarQuery, SchoolCalendarSemester,
    };
}

/// Read-only campus-card account and transaction access.
pub mod campus_card {
    pub use crate::client::CampusCardClient;
    pub use tsinghua_kit_engine::campus_card_api::{
        CampusCardAccount, CampusCardInteraction, CampusCardPasswordRequest, CampusCardTransaction,
        CampusCardTransactionRange, CampusCardTransactionType, CampusCardTransactions,
    };
}

/// Classroom building directories and weekly availability.
pub mod classrooms {
    pub use crate::client::ClassroomsClient;
    pub use chrono::NaiveDate;
    pub use tsinghua_kit_engine::classrooms_api::{
        BuildingRef, ClassroomAvailability, ClassroomBuilding, ClassroomBuildings,
        ClassroomRoomAvailability, ClassroomSlotStatus, ClassroomWeek, ClassroomWeekSelection,
    };
}

pub mod error {
    pub use tsinghua_kit_engine::error::{Error, ErrorCode, Service};
}

/// Dorm-electricity remainder and payment-history reads.
pub mod electricity {
    pub use crate::client::ElectricityClient;
    pub use tsinghua_kit_engine::electricity_api::{
        ElectricityPaymentHistory, ElectricityPaymentRecord, ElectricityRemainder,
    };
}

/// Physical-education test results, with the App-side reference total
/// explicitly labelled as a local recomputation.
pub mod physical_exam {
    pub use crate::client::PhysicalExamClient;
    pub use tsinghua_kit_engine::physical_exam_read::{
        PhysicalExamItem, PhysicalExamItems, PhysicalExamReport,
    };
}

/// Degree-program completion, read from the live service on every call.
pub mod program {
    pub use crate::client::ProgramClient;
    pub use tsinghua_kit_engine::program_read::{
        CourseCompletion, CourseFull, CourseSetCompletion, CourseSetFull, CourseSetKind,
        CourseState, FullProgram, ProgramCompletion,
    };
}

/// Teaching-evaluation questionnaires the account may currently fill in.
/// Each item carries an opaque reference; no service route or form value is
/// exposed to the caller.
///
/// A form is a display copy and an answer set carries scores and comments
/// only: the service's own submission state never leaves Rust, so the body a
/// submission sends is always the service's page with the caller's answers
/// filled in.
pub mod assessment {
    pub use crate::client::AssessmentClient;
    pub use tsinghua_kit_engine::assessment_read::{
        ASSESSMENT_MAX_SCORE, ASSESSMENT_MIN_SCORE, AssessmentAnswers, AssessmentFormView,
        AssessmentInputError, AssessmentItem, AssessmentList, AssessmentPersonAnswers,
        AssessmentPersonRole, AssessmentPersonView, AssessmentQuestionAnswer,
        AssessmentQuestionView, AssessmentRef,
    };
}

/// Issued e-invoices and their documents.  Each row carries an opaque document
/// reference; the service's own record identifier and every payer-identifying
/// field stay inside Rust.
pub mod invoice {
    pub use crate::client::InvoiceClient;
    pub use tsinghua_kit_engine::invoice_read::{
        INVOICE_PAGE_SIZE, InvoiceDocument, InvoicePage, InvoiceRecord, InvoiceRef,
        MAX_INVOICE_PAGE,
    };
}

/// Bank payroll receipts and graduate-student income statements.
///
/// Both are money statements, so every amount is exact integer cents: a caller
/// never has to round a decimal to read an earned or transferred figure.  The
/// two payroll ledgers are separate reads on one campus host.
pub mod bank {
    pub use crate::client::BankClient;
    pub use tsinghua_kit_engine::{
        BankLedger, BankPaymentLedger, BankReceiptMonth, BankReceiptRow, GraduateIncomePage,
        GraduateIncomeRecord,
    };
}

pub mod learn {
    pub use crate::client::LearnClient;
    pub use chrono::{DateTime, Utc};
    pub use tsinghua_kit_engine::learn_api::{
        Course, CourseAnnouncement, CourseAnnouncements, CourseCatalog, CourseDiscussion,
        CourseDiscussions, CourseFile, CourseFileCategories, CourseFileCategory, CourseFileRef,
        CourseFiles, CourseRef, Homework, HomeworkAttachment, HomeworkAttachmentKind,
        HomeworkDetail, HomeworkList, HomeworkRef, HomeworkState, SavedCourseFile,
    };
}

/// Library locations, opening windows, seats, and socket availability.
pub mod library {
    pub use crate::client::LibraryClient;
    pub use chrono::{NaiveDate, NaiveTime};
    pub use tsinghua_kit_engine::library_api::{
        FloorRef, LibraryAvailability, LibraryDay, LibraryDirectory, LibraryFloor, LibraryFloors,
        LibraryPlace, LibraryRef, LibrarySeat, LibrarySeatSocket, LibrarySection, LibrarySections,
        LibrarySocketAvailability, LibraryTimeWindow, LibraryTimeWindows, SeatRef, SeatWindowRef,
        SectionRef,
    };
    pub use tsinghua_kit_engine::library_read::LibrarySocketState;
}

pub mod network {
    pub use crate::client::NetworkClient;
    pub use crate::client::NetworkProfilesClient;
    pub use tsinghua_kit_engine::network::{
        NetworkAccessMethod, NetworkProfileId, NetworkProfileInput, NetworkProfilePassword,
        NetworkProfileSummary, PortalAddressRegistration, PortalConnectionResult,
        PortalConnectionState, PortalObservation, PreparedNetworkInput,
    };
}

pub mod news {
    pub use crate::client::NewsClient;
    pub use tsinghua_kit_engine::news::{
        ArticleDetail, ArticleRef, NewsArticle, NewsAttachment, NewsCatalog, NewsCatalogCoverage,
        NewsChannel, NewsChannelRef, NewsFavorites, NewsPage, NewsQuery, NewsSource, NewsSourceRef,
        NewsSubscription, NewsSubscriptionRef, NewsSubscriptions,
    };
}

/// Account-bound daily academic summary with explicit cache provenance.
///
/// ```no_run
/// use tsinghua_kit::{Client, overview::NaiveDate, read::ReadPolicy};
/// # async fn example() -> tsinghua_kit::Result<()> {
/// let mut client = Client::builder().build()?;
/// let date = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
/// match client.overview().day(date, ReadPolicy::CacheOnly).await {
///     Ok(result) => {
///         let _source = result.metadata().source();
///         let _failures = result.data().section_failures();
///     }
///     Err(error) if error.code() == tsinghua_kit::error::ErrorCode::CacheMiss => {}
///     Err(error) => return Err(error),
/// }
/// # Ok(())
/// # }
/// ```
pub mod overview {
    pub use crate::client::OverviewClient;
    pub use chrono::{NaiveDate, Utc};
    pub use tsinghua_kit_engine::overview_api::{
        DailyOverview, OverviewSchedule, OverviewScheduleKind, OverviewSectionFailures,
        OverviewTodo,
    };
}

pub mod read {
    pub use tsinghua_kit_engine::read::{
        CacheFreshness, IncompleteReason, ReadCoverage, ReadMetadata, ReadPolicy, ReadResult,
        ReadSource,
    };
}

/// Academic schedules, course grades, and examination reports.
pub mod registrar {
    pub use crate::client::RegistrarClient;
    pub use chrono::{DateTime, NaiveDate, Utc};
    pub use tsinghua_kit_engine::registrar_api::{
        AcademicStage, CourseGrade, Exam, ExamReport, ExamWeekday, GradeReport, GradeReportKind,
        ScheduleEvent, ScheduleEventKind, SemesterSchedule,
    };
}

pub mod service_hall {
    pub use crate::client::ServiceHallClient;
    pub use tsinghua_kit_engine::service_hall::{
        PendingReadPolicy, PendingTasks, PhaseDetails, PhaseStep, PhaseStepItem, ServiceDirectory,
        ServiceEntry, ServiceHallReadPolicy, Task, TaskReference, TaskView, WorkflowTask,
        WorkflowTaskList, WorkflowTaskRef,
    };
}

/// One course result looked up by course number.
///
/// The account's own student id, which the service's query also needs, is
/// derived inside Rust from the proven identity; it is never an argument, a
/// result field, or a log field.
pub mod course_score {
    pub use crate::client::CourseScoreClient;
    pub use tsinghua_kit_engine::CourseScore;
}

pub mod self_service {
    pub use crate::client::SelfServiceClient;
    pub use tsinghua_kit_engine::self_service::{
        AccountProfile, DeviceRef, OnlineDevice, UsageBalance,
    };
}

/// Dormitory laundry rooms served by the three third-party vendors the
/// deployment uses.
///
/// These vendors have no campus account binding, so a read is
/// account-independent: it needs no proven campus session and reports the
/// vendor's own snapshot time rather than pretending to be account data.
/// Only building lists and device status are reachable; no ordering,
/// payment, or other write is modelled.
pub mod laundry {
    pub use tsinghua_kit_engine::laundry_api::{
        LAUNDRY_PROVIDERS, LAUNDRY_STATUSES, LaundryBuilding, LaundryBuildingGroup, LaundryError,
        LaundryMachine, LaundryRoom, LaundryRoomsReport, read_laundry_buildings,
        read_laundry_rooms,
    };
}

/// 清紫源泉 bottled-water delivery account lookup.
///
/// The vendor runs its own plain-HTTP service with no campus account binding,
/// so this is a third-party read: the caller supplies the room's own delivery
/// number and receives the vendor's record.  The vendor's ordering endpoint
/// places a real order and is deliberately not modelled here.
pub mod water {
    pub use tsinghua_kit_engine::laundry_api::{WaterLookupError, read_water_user};
    pub use tsinghua_kit_engine::water_read::{
        WATER_BRANDS, WATER_ORIGIN, WaterAdapter, WaterError, WaterUser, water_brand_name,
    };
}

pub use client::{Client, ClientBuilder};
pub use error::Error;

/// The standard result type for public SDK operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Legacy FFI types are available only to the bridge compatibility package.
#[cfg(feature = "ffi-compat")]
#[doc(hidden)]
pub mod ffi_compat {
    pub use tsinghua_kit_engine::*;
}
