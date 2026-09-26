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
    pub use crate::client::{ClientCachePolicy, CredentialStorageKey, CredentialStoragePolicy};
    pub use tsinghua_kit_engine::client::{
        IdentitySessionStorageKey, IdentitySessionStoragePolicy,
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

pub mod self_service {
    pub use crate::client::SelfServiceClient;
    pub use tsinghua_kit_engine::self_service::{
        AccountProfile, DeviceRef, OnlineDevice, UsageBalance,
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
