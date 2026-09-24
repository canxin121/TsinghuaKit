//! A transport-free catalog of THYou services.
//!
//! The catalog is an application-facing model.  It describes what a service
//! is, how the UI should present it, what operations it advertises, and its
//! current coarse availability state.  It deliberately has no endpoint,
//! request, cookie, ticket, CSRF, password, or HTML field.  Protocol clients
//! can update an entry's availability without making their wire details part
//! of the catalog contract.
//!
//! [`CatalogServiceId`] is intentionally separate from the session-bound
//! `protocol::ServiceId`.  The latter is used to bind credentials and session
//! state; this type is allowed to include user-facing services that do not yet
//! have a session adapter, such as the library and campus-card services.

use core::fmt;

/// Stable IDs for services known to the application catalog.
///
/// The `as_str` values are persistence and bridge keys.  They must not be
/// renamed for a visual refresh; add a migration when a key ever needs to be
/// retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum CatalogServiceId {
    Identity,
    Learn,
    Registrar,
    Info,
    Classroom,
    Electricity,
    Usereg,
    Tunet,
    Library,
    CampusCard,
}

/// Stable catalog order and complete ID set.
pub const ALL_CATALOG_SERVICE_IDS: &[CatalogServiceId] = &[
    CatalogServiceId::Identity,
    CatalogServiceId::Learn,
    CatalogServiceId::Registrar,
    CatalogServiceId::Info,
    CatalogServiceId::Classroom,
    CatalogServiceId::Electricity,
    CatalogServiceId::Usereg,
    CatalogServiceId::Tunet,
    CatalogServiceId::Library,
    CatalogServiceId::CampusCard,
];

impl CatalogServiceId {
    /// Returns the stable machine-readable key used at the application
    /// boundary.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Learn => "learn",
            Self::Registrar => "registrar",
            Self::Info => "info",
            Self::Classroom => "classroom",
            Self::Electricity => "electricity",
            Self::Usereg => "usereg",
            Self::Tunet => "tunet",
            Self::Library => "library",
            Self::CampusCard => "campus_card",
        }
    }

    /// Whether this ID represents an infrastructure service rather than a
    /// normal page in the service grid.
    pub const fn is_infrastructure(self) -> bool {
        matches!(self, Self::Identity)
    }
}

impl fmt::Display for CatalogServiceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable high-level grouping used for navigation and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum ServiceCategory {
    Identity,
    Learning,
    Academic,
    Information,
    Campus,
    Network,
    Library,
    CampusCard,
}

impl ServiceCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Learning => "learning",
            Self::Academic => "academic",
            Self::Information => "information",
            Self::Campus => "campus",
            Self::Network => "network",
            Self::Library => "library",
            Self::CampusCard => "campus_card",
        }
    }

    /// Translation key for a category label.  The UI owns localization and
    /// does not need to infer labels from a protocol endpoint.
    pub const fn display_name_key(self) -> &'static str {
        match self {
            Self::Identity => "service_category.identity",
            Self::Learning => "service_category.learning",
            Self::Academic => "service_category.academic",
            Self::Information => "service_category.information",
            Self::Campus => "service_category.campus",
            Self::Network => "service_category.network",
            Self::Library => "service_category.library",
            Self::CampusCard => "service_category.campus_card",
        }
    }
}

/// Stable presentation metadata.  `icon_key` is a semantic key resolved by
/// Flutter/Forui; it is not a platform-specific icon object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServicePresentation {
    pub display_name: &'static str,
    pub display_name_key: &'static str,
    pub icon_key: &'static str,
}

/// Coarse status safe to expose at the application boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ServiceAvailability {
    /// No adapter has reported a state yet.
    #[default]
    Unknown,
    /// The service can currently perform its advertised operations.
    Available,
    /// The service exists but needs the user to authenticate first.
    RequiresAuthentication,
    /// The user's identity session is available, but this service still needs
    /// its own handoff/session proof before it can be used.
    RequiresServiceSession,
    /// The service has a separate account or network login boundary.
    RequiresIndependentLogin,
    /// The service is waiting for a second-factor step.
    RequiresSecondFactor,
    /// Some operations may work, but the service has a known limitation.
    Degraded,
    /// The service is configured but cannot currently be used.
    Unavailable,
    /// No adapter or deployment configuration is available yet.
    NotConfigured,
}

impl ServiceAvailability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Available => "available",
            Self::RequiresAuthentication => "requires_authentication",
            Self::RequiresServiceSession => "requires_service_session",
            Self::RequiresIndependentLogin => "requires_independent_login",
            Self::RequiresSecondFactor => "requires_second_factor",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
            Self::NotConfigured => "not_configured",
        }
    }

    /// Whether the UI may offer the service as usable right now.
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Available | Self::Degraded)
    }

    /// Translation key for the status label.  Error details remain owned by
    /// the Rust service adapter and are never represented here.
    pub const fn display_name_key(self) -> &'static str {
        match self {
            Self::Unknown => "service_availability.unknown",
            Self::Available => "service_availability.available",
            Self::RequiresAuthentication => "service_availability.requires_authentication",
            Self::RequiresServiceSession => "service_availability.requires_service_session",
            Self::RequiresIndependentLogin => "service_availability.requires_independent_login",
            Self::RequiresSecondFactor => "service_availability.requires_second_factor",
            Self::Degraded => "service_availability.degraded",
            Self::Unavailable => "service_availability.unavailable",
            Self::NotConfigured => "service_availability.not_configured",
        }
    }
}

/// Describes which credential boundary a service uses.  This is safe
/// presentation metadata; it never contains a credential or a session value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceAuthenticationSource {
    None,
    IdentitySession,
    ServiceSession,
    IndependentLogin,
    AdapterUnavailable,
}

impl ServiceAuthenticationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::IdentitySession => "identity_session",
            Self::ServiceSession => "service_session",
            Self::IndependentLogin => "independent_login",
            Self::AdapterUnavailable => "adapter_unavailable",
        }
    }
}

/// Whether a capability only reads data, changes state, or performs an
/// account/network action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapabilityAccess {
    Read,
    Write,
    Action,
}

impl CapabilityAccess {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Action => "action",
        }
    }
}

/// Authentication boundary for a capability.  These are policy labels, never
/// the credentials or tokens that satisfy them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthenticationRequirement {
    Anonymous,
    IdentitySession,
    ServiceSession,
    IdentityAndServiceSession,
}

impl AuthenticationRequirement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::IdentitySession => "identity_session",
            Self::ServiceSession => "service_session",
            Self::IdentityAndServiceSession => "identity_and_service_session",
        }
    }
}

/// A stable operation that a service can advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum ServiceCapability {
    Authenticate,
    Refresh,
    ReadCourses,
    ReadTermCalendar,
    ReadSchoolCalendar,
    ReadSchedule,
    ReadAssignments,
    ReadAnnouncements,
    ReadClassroomAvailability,
    ReadElectricityRemainder,
    ReadElectricityHistory,
    ReadGrades,
    ReadExams,
    BrowsePortal,
    ReadNetworkAccount,
    ReadNetworkBalance,
    ManageTrustedDevices,
    ManageOnlineDevices,
    ReadNetworkStatus,
    ConnectNetwork,
    DisconnectNetwork,
    SearchLibrary,
    ReadLibraryAreas,
    ReadLibraryAccount,
    ReadLibraryLoans,
    ReadLibraryFines,
    ReadLibrarySeats,
    ReadLibrarySocketStatus,
    ReadCardBalance,
    ReadCardTransactions,
}

/// Static metadata for one capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCapabilityMetadata {
    pub key: &'static str,
    pub display_name_key: &'static str,
    pub access: CapabilityAccess,
    pub authentication: AuthenticationRequirement,
}

/// The proof boundary and product route for one service-owned capability.
///
/// This is deliberately kept out of the Flutter DTO: the bridge only needs
/// the stable capability key and its safe presentation metadata.  Rust uses
/// this contract to decide which keys may cross the bridge after a concrete
/// response proof.  The route names are application action keys, never URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCapabilityContract {
    pub proof: &'static str,
    pub route: &'static str,
}

impl ServiceCapability {
    /// Stable machine-readable capability key.
    pub const fn as_str(self) -> &'static str {
        self.metadata().key
    }

    pub const fn metadata(self) -> ServiceCapabilityMetadata {
        use AuthenticationRequirement::{Anonymous, IdentityAndServiceSession, ServiceSession};
        use CapabilityAccess::{Action, Read, Write};

        let (key, access, authentication) = match self {
            Self::Authenticate => ("authenticate", Action, Anonymous),
            Self::Refresh => ("refresh", Action, ServiceSession),
            Self::ReadCourses => ("read_courses", Read, IdentityAndServiceSession),
            Self::ReadTermCalendar => ("read_term_calendar", Read, IdentityAndServiceSession),
            Self::ReadSchoolCalendar => ("read_school_calendar", Read, IdentityAndServiceSession),
            Self::ReadSchedule => ("read_schedule", Read, IdentityAndServiceSession),
            Self::ReadAssignments => ("read_assignments", Read, IdentityAndServiceSession),
            Self::ReadAnnouncements => ("read_announcements", Read, IdentityAndServiceSession),
            Self::ReadClassroomAvailability => (
                "read_classroom_availability",
                Read,
                IdentityAndServiceSession,
            ),
            Self::ReadElectricityRemainder => (
                "read_electricity_remainder",
                Read,
                IdentityAndServiceSession,
            ),
            Self::ReadElectricityHistory => {
                ("read_electricity_history", Read, IdentityAndServiceSession)
            }
            Self::ReadGrades => ("read_grades", Read, IdentityAndServiceSession),
            Self::ReadExams => ("read_exams", Read, IdentityAndServiceSession),
            Self::BrowsePortal => ("browse_portal", Read, IdentityAndServiceSession),
            Self::ReadNetworkAccount => ("read_network_account", Read, ServiceSession),
            Self::ReadNetworkBalance => ("read_network_balance", Read, ServiceSession),
            Self::ManageTrustedDevices => ("manage_trusted_devices", Write, ServiceSession),
            Self::ManageOnlineDevices => ("manage_online_devices", Write, ServiceSession),
            Self::ReadNetworkStatus => ("read_network_status", Read, Anonymous),
            Self::ConnectNetwork => ("connect_network", Action, Anonymous),
            Self::DisconnectNetwork => ("disconnect_network", Action, ServiceSession),
            Self::SearchLibrary => ("search_library", Read, IdentityAndServiceSession),
            Self::ReadLibraryAreas => ("read_library_areas", Read, IdentityAndServiceSession),
            Self::ReadLibraryAccount => ("read_library_account", Read, IdentityAndServiceSession),
            Self::ReadLibraryLoans => ("read_library_loans", Read, IdentityAndServiceSession),
            Self::ReadLibraryFines => ("read_library_fines", Read, IdentityAndServiceSession),
            Self::ReadLibrarySeats => ("read_library_seats", Read, IdentityAndServiceSession),
            Self::ReadLibrarySocketStatus => (
                "read_library_socket_status",
                Read,
                IdentityAndServiceSession,
            ),
            Self::ReadCardBalance => ("read_card_balance", Read, IdentityAndServiceSession),
            Self::ReadCardTransactions => {
                ("read_card_transactions", Read, IdentityAndServiceSession)
            }
        };

        ServiceCapabilityMetadata {
            key,
            display_name_key: match self {
                Self::Authenticate => "service_capability.authenticate",
                Self::Refresh => "service_capability.refresh",
                Self::ReadCourses => "service_capability.read_courses",
                Self::ReadTermCalendar => "service_capability.read_term_calendar",
                Self::ReadSchoolCalendar => "service_capability.read_school_calendar",
                Self::ReadSchedule => "service_capability.read_schedule",
                Self::ReadAssignments => "service_capability.read_assignments",
                Self::ReadAnnouncements => "service_capability.read_announcements",
                Self::ReadClassroomAvailability => "service_capability.read_classroom_availability",
                Self::ReadElectricityRemainder => "service_capability.read_electricity_remainder",
                Self::ReadElectricityHistory => "service_capability.read_electricity_history",
                Self::ReadGrades => "service_capability.read_grades",
                Self::ReadExams => "service_capability.read_exams",
                Self::BrowsePortal => "service_capability.browse_portal",
                Self::ReadNetworkAccount => "service_capability.read_network_account",
                Self::ReadNetworkBalance => "service_capability.read_network_balance",
                Self::ManageTrustedDevices => "service_capability.manage_trusted_devices",
                Self::ManageOnlineDevices => "service_capability.manage_online_devices",
                Self::ReadNetworkStatus => "service_capability.read_network_status",
                Self::ConnectNetwork => "service_capability.connect_network",
                Self::DisconnectNetwork => "service_capability.disconnect_network",
                Self::SearchLibrary => "service_capability.search_library",
                Self::ReadLibraryAreas => "service_capability.read_library_areas",
                Self::ReadLibraryAccount => "service_capability.read_library_account",
                Self::ReadLibraryLoans => "service_capability.read_library_loans",
                Self::ReadLibraryFines => "service_capability.read_library_fines",
                Self::ReadLibrarySeats => "service_capability.read_library_seats",
                Self::ReadLibrarySocketStatus => "service_capability.read_library_socket_status",
                Self::ReadCardBalance => "service_capability.read_card_balance",
                Self::ReadCardTransactions => "service_capability.read_card_transactions",
            },
            access,
            authentication,
        }
    }

    /// Returns the service-specific proof and product route for this
    /// capability.  A capability that is meaningful for another service is
    /// intentionally represented by `None`; this prevents a broad enum value
    /// from becoming an accidental cross-service advertisement.
    pub const fn contract_for(
        self,
        service: CatalogServiceId,
    ) -> Option<ServiceCapabilityContract> {
        use CatalogServiceId::{
            CampusCard, Classroom, Electricity, Identity, Info, Learn, Library, Registrar, Tunet,
            Usereg,
        };

        let (proof, route) = match (service, self) {
            (Identity, Self::Authenticate) => ("identity_authentication", "authenticate"),
            (Learn, Self::Refresh) => ("learn_service_session", "refresh"),
            (Learn, Self::ReadCourses) => ("learn_service_session", "overview"),
            (Learn, Self::ReadTermCalendar) => ("learn_service_session", "learn_calendar"),
            (Learn, Self::ReadSchoolCalendar) => ("learn_service_session", "school_calendar"),
            (Learn, Self::ReadSchedule) => ("overview_resolver", "overview"),
            (Learn, Self::ReadAssignments) => ("learn_todos_session", "overview"),
            (Learn, Self::ReadAnnouncements) => ("learn_service_session", "learn_announcements"),
            (Registrar, Self::Refresh) => ("registrar_service_session", "refresh"),
            (Registrar, Self::ReadGrades) => ("registrar_grades_session", "grades"),
            (Registrar, Self::ReadExams) => ("registrar_exam_session", "exams"),
            (Info, Self::Refresh) => ("info_service_session", "refresh"),
            (Info, Self::ReadAnnouncements) => ("info_service_session", "info_news"),
            (Classroom, Self::Refresh) => ("classroom_service_session", "refresh"),
            (Classroom, Self::ReadClassroomAvailability) => {
                ("classroom_service_session", "classroom")
            }
            (Electricity, Self::Refresh) => ("electricity_service_session", "refresh"),
            (Electricity, Self::ReadElectricityRemainder) => {
                ("electricity_service_session", "electricity_remainder")
            }
            (Electricity, Self::ReadElectricityHistory) => {
                ("electricity_service_session", "electricity_history")
            }
            (Usereg, Self::Refresh) => ("usereg_service_session", "refresh"),
            (Usereg, Self::ReadNetworkAccount) => ("usereg_service_session", "usereg_account"),
            (Usereg, Self::ReadNetworkBalance) => ("usereg_service_session", "usereg_balance"),
            (Usereg, Self::ManageTrustedDevices) => ("usereg_service_session", "usereg_devices"),
            (Usereg, Self::ManageOnlineDevices) => ("usereg_service_session", "usereg_devices"),
            (Tunet, Self::Refresh) => ("tunet_service_session", "refresh"),
            (Tunet, Self::ReadNetworkStatus) => ("tunet_local_status", "tunet_status"),
            (Tunet, Self::ConnectNetwork) => ("tunet_login_action", "tunet_login"),
            (Tunet, Self::DisconnectNetwork) => ("tunet_service_session", "tunet_disconnect"),
            (Library, Self::Refresh) => ("library_service_session", "refresh"),
            (Library, Self::SearchLibrary) => ("library_service_session", "library_search"),
            (Library, Self::ReadLibraryAreas) => ("library_service_session", "library_areas"),
            (Library, Self::ReadLibraryAccount) => ("library_service_session", "library_account"),
            (Library, Self::ReadLibraryLoans) => ("library_service_session", "library_loans"),
            (Library, Self::ReadLibraryFines) => ("library_service_session", "library_fines"),
            (Library, Self::ReadLibrarySeats) => ("library_service_session", "library_seats"),
            (Library, Self::ReadLibrarySocketStatus) => {
                ("library_service_session", "library_socket_status")
            }
            (CampusCard, Self::Refresh) => ("campus_card_service_session", "refresh"),
            (CampusCard, Self::ReadCardBalance) => ("campus_card_service_session", "card_balance"),
            (CampusCard, Self::ReadCardTransactions) => {
                ("campus_card_service_session", "card_transactions")
            }
            _ => return None,
        };

        Some(ServiceCapabilityContract { proof, route })
    }

    /// Returns capability metadata in the context of its owning service.
    ///
    /// `Refresh` is shared by services with different authentication
    /// boundaries.  A learning/INFO/library/card refresh requires both the
    /// unified identity session and the target service proof, while a TUNet
    /// or USEREG refresh only requires that service's independent session.
    /// Keeping the capability key stable while resolving this policy at the
    /// service boundary prevents the bridge from displaying a weaker
    /// authentication requirement than the runtime actually enforces.
    pub const fn metadata_for(self, service: CatalogServiceId) -> ServiceCapabilityMetadata {
        let metadata = self.metadata();
        let authentication = match (self, service) {
            (
                Self::Refresh,
                CatalogServiceId::Learn
                | CatalogServiceId::Registrar
                | CatalogServiceId::Info
                | CatalogServiceId::Classroom
                | CatalogServiceId::Electricity
                | CatalogServiceId::Library
                | CatalogServiceId::CampusCard,
            ) => AuthenticationRequirement::IdentityAndServiceSession,
            _ => metadata.authentication,
        };

        ServiceCapabilityMetadata {
            authentication,
            ..metadata
        }
    }
}

/// A stable, ordered capability set attached to a service definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCapabilities {
    pub items: &'static [ServiceCapability],
}

impl ServiceCapabilities {
    pub const fn new(items: &'static [ServiceCapability]) -> Self {
        Self { items }
    }

    pub fn contains(&self, capability: ServiceCapability) -> bool {
        self.items.contains(&capability)
    }

    /// Returns capability metadata without an owning service context. Use
    /// [`ServiceDefinition::capability_metadata`] for catalog definitions so
    /// shared capabilities such as `refresh` receive their real
    /// authentication boundary.
    pub fn metadata(&self) -> impl Iterator<Item = ServiceCapabilityMetadata> + '_ {
        self.items.iter().copied().map(ServiceCapability::metadata)
    }
}

/// Immutable definition of a service's identity and advertised behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceDefinition {
    pub id: CatalogServiceId,
    pub category: ServiceCategory,
    pub presentation: ServicePresentation,
    pub capabilities: ServiceCapabilities,
    pub default_availability: ServiceAvailability,
    pub authentication_source: ServiceAuthenticationSource,
    pub visible_by_default: bool,
}

impl ServiceDefinition {
    /// Returns capability metadata with the authentication boundary resolved
    /// for this service.  A capability such as `refresh` is shared by several
    /// services, but its real prerequisite differs between a shared identity
    /// handoff and an independent network login; callers displaying a
    /// definition must use this contextual view rather than the generic
    /// capability metadata.
    pub fn capability_metadata(self) -> impl Iterator<Item = ServiceCapabilityMetadata> {
        self.capabilities
            .items
            .iter()
            .copied()
            .map(move |capability| capability.metadata_for(self.id))
    }
}

/// A definition paired with the latest coarse runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCatalogEntry {
    pub definition: ServiceDefinition,
    pub availability: ServiceAvailability,
}

impl ServiceCatalogEntry {
    pub const fn id(self) -> CatalogServiceId {
        self.definition.id
    }

    pub const fn category(self) -> ServiceCategory {
        self.definition.category
    }

    pub const fn presentation(self) -> ServicePresentation {
        self.definition.presentation
    }

    pub const fn capabilities(self) -> ServiceCapabilities {
        self.definition.capabilities
    }

    pub const fn authentication_source(self) -> ServiceAuthenticationSource {
        self.definition.authentication_source
    }

    pub const fn is_visible(self) -> bool {
        self.definition.visible_by_default
    }
}

/// In-memory catalog state.  It contains only safe metadata and coarse
/// availability values; service adapters retain all transport/session state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCatalog {
    entries: Vec<ServiceCatalogEntry>,
}

impl Default for ServiceCatalog {
    fn default() -> Self {
        Self::standard()
    }
}

impl ServiceCatalog {
    /// Builds the standard THYou catalog in stable order.
    pub fn standard() -> Self {
        Self {
            entries: STANDARD_SERVICE_DEFINITIONS
                .iter()
                .copied()
                .map(|definition| ServiceCatalogEntry {
                    definition,
                    availability: definition.default_availability,
                })
                .collect(),
        }
    }

    pub fn entries(&self) -> &[ServiceCatalogEntry] {
        &self.entries
    }

    pub fn find(&self, id: CatalogServiceId) -> Option<&ServiceCatalogEntry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    pub fn visible_entries(&self) -> impl Iterator<Item = &ServiceCatalogEntry> {
        self.entries.iter().filter(|entry| entry.is_visible())
    }

    pub fn entries_in_category(
        &self,
        category: ServiceCategory,
    ) -> impl Iterator<Item = &ServiceCatalogEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.category() == category)
    }

    /// Updates only the safe status field for a known service.
    pub fn set_availability(&mut self, id: CatalogServiceId, availability: ServiceAvailability) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id() == id) {
            entry.availability = availability;
        }
    }
}

const IDENTITY_CAPABILITIES: &[ServiceCapability] = &[ServiceCapability::Authenticate];
const LEARN_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadCourses,
    ServiceCapability::ReadTermCalendar,
    ServiceCapability::ReadSchoolCalendar,
    ServiceCapability::ReadSchedule,
    ServiceCapability::ReadAnnouncements,
];
const REGISTRAR_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadGrades,
    ServiceCapability::ReadExams,
];
const INFO_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadAnnouncements,
];
const CLASSROOM_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadClassroomAvailability,
];
const ELECTRICITY_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadElectricityRemainder,
    ServiceCapability::ReadElectricityHistory,
];
const USEREG_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadNetworkAccount,
    ServiceCapability::ReadNetworkBalance,
    ServiceCapability::ManageOnlineDevices,
];
const TUNET_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadNetworkStatus,
    ServiceCapability::ConnectNetwork,
    ServiceCapability::DisconnectNetwork,
];
const LIBRARY_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadLibraryAreas,
    ServiceCapability::ReadLibrarySeats,
    ServiceCapability::ReadLibrarySocketStatus,
];
const CAMPUS_CARD_CAPABILITIES: &[ServiceCapability] = &[
    ServiceCapability::Refresh,
    ServiceCapability::ReadCardBalance,
    ServiceCapability::ReadCardTransactions,
];

/// The canonical service definitions.  All strings here are safe presentation
/// or metadata keys; they are never endpoint names or authentication material.
pub const STANDARD_SERVICE_DEFINITIONS: &[ServiceDefinition] = &[
    ServiceDefinition {
        id: CatalogServiceId::Identity,
        category: ServiceCategory::Identity,
        presentation: ServicePresentation {
            display_name: "统一认证",
            display_name_key: "service.identity.name",
            icon_key: "shield_person",
        },
        capabilities: ServiceCapabilities::new(IDENTITY_CAPABILITIES),
        default_availability: ServiceAvailability::Unknown,
        authentication_source: ServiceAuthenticationSource::None,
        visible_by_default: false,
    },
    ServiceDefinition {
        id: CatalogServiceId::Learn,
        category: ServiceCategory::Learning,
        presentation: ServicePresentation {
            display_name: "网络学堂",
            display_name_key: "service.learn.name",
            icon_key: "menu_book",
        },
        capabilities: ServiceCapabilities::new(LEARN_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresAuthentication,
        authentication_source: ServiceAuthenticationSource::IdentitySession,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Registrar,
        category: ServiceCategory::Academic,
        presentation: ServicePresentation {
            display_name: "教务系统",
            display_name_key: "service.registrar.name",
            icon_key: "school",
        },
        capabilities: ServiceCapabilities::new(REGISTRAR_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresAuthentication,
        authentication_source: ServiceAuthenticationSource::IdentitySession,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Info,
        category: ServiceCategory::Information,
        presentation: ServicePresentation {
            display_name: "信息门户",
            display_name_key: "service.info.name",
            icon_key: "article",
        },
        capabilities: ServiceCapabilities::new(INFO_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresServiceSession,
        authentication_source: ServiceAuthenticationSource::ServiceSession,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Classroom,
        category: ServiceCategory::Academic,
        presentation: ServicePresentation {
            display_name: "教室查询",
            display_name_key: "service.classroom.name",
            icon_key: "meeting_room",
        },
        capabilities: ServiceCapabilities::new(CLASSROOM_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresServiceSession,
        authentication_source: ServiceAuthenticationSource::ServiceSession,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Electricity,
        category: ServiceCategory::Campus,
        presentation: ServicePresentation {
            display_name: "宿舍电费",
            display_name_key: "service.electricity.name",
            icon_key: "electric_bolt",
        },
        capabilities: ServiceCapabilities::new(ELECTRICITY_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresServiceSession,
        authentication_source: ServiceAuthenticationSource::ServiceSession,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Usereg,
        category: ServiceCategory::Network,
        presentation: ServicePresentation {
            display_name: "网络自助",
            display_name_key: "service.usereg.name",
            icon_key: "manage_accounts",
        },
        capabilities: ServiceCapabilities::new(USEREG_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresIndependentLogin,
        authentication_source: ServiceAuthenticationSource::IndependentLogin,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Tunet,
        category: ServiceCategory::Network,
        presentation: ServicePresentation {
            display_name: "校园网",
            display_name_key: "service.tunet.name",
            icon_key: "wifi",
        },
        capabilities: ServiceCapabilities::new(TUNET_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresIndependentLogin,
        authentication_source: ServiceAuthenticationSource::IndependentLogin,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::Library,
        category: ServiceCategory::Library,
        presentation: ServicePresentation {
            display_name: "图书馆",
            display_name_key: "service.library.name",
            icon_key: "local_library",
        },
        capabilities: ServiceCapabilities::new(LIBRARY_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresServiceSession,
        authentication_source: ServiceAuthenticationSource::ServiceSession,
        visible_by_default: true,
    },
    ServiceDefinition {
        id: CatalogServiceId::CampusCard,
        category: ServiceCategory::CampusCard,
        presentation: ServicePresentation {
            display_name: "校园卡",
            display_name_key: "service.campus_card.name",
            icon_key: "credit_card",
        },
        capabilities: ServiceCapabilities::new(CAMPUS_CARD_CAPABILITIES),
        default_availability: ServiceAvailability::RequiresServiceSession,
        authentication_source: ServiceAuthenticationSource::ServiceSession,
        visible_by_default: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn ids_are_stable_unique_and_include_future_page_services() {
        let keys = ALL_CATALOG_SERVICE_IDS
            .iter()
            .map(|id| id.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(ALL_CATALOG_SERVICE_IDS.len(), 10);
        assert_eq!(keys.len(), ALL_CATALOG_SERVICE_IDS.len());
        assert_eq!(CatalogServiceId::CampusCard.as_str(), "campus_card");
        assert_eq!(CatalogServiceId::Library.as_str(), "library");
        assert!(CatalogServiceId::Identity.is_infrastructure());
        assert!(!CatalogServiceId::Library.is_infrastructure());
    }

    #[test]
    fn standard_definition_order_matches_the_stable_id_order() {
        let catalog = ServiceCatalog::standard();
        let ids = catalog
            .entries()
            .iter()
            .map(|entry| entry.id())
            .collect::<Vec<_>>();

        assert_eq!(ids, ALL_CATALOG_SERVICE_IDS);
    }

    #[test]
    fn standard_catalog_has_complete_safe_metadata() {
        let catalog = ServiceCatalog::standard();

        assert_eq!(catalog.entries().len(), STANDARD_SERVICE_DEFINITIONS.len());
        for entry in catalog.entries() {
            assert_eq!(entry.id().as_str(), entry.definition.id.as_str());
            assert!(!entry.presentation().display_name.is_empty());
            assert!(!entry.presentation().display_name_key.is_empty());
            assert!(!entry.presentation().icon_key.is_empty());
            assert!(!entry.capabilities().items.is_empty());

            for metadata in entry.definition.capability_metadata() {
                assert!(!metadata.key.is_empty());
                assert!(!metadata.display_name_key.is_empty());
            }

            for capability in entry.capabilities().items {
                assert!(
                    capability.contract_for(entry.id()).is_some(),
                    "{} advertises {} without a proof contract",
                    entry.id(),
                    capability.as_str()
                );
            }
        }
    }

    #[test]
    fn definition_metadata_resolves_shared_refresh_to_the_real_boundary() {
        let catalog = ServiceCatalog::standard();
        let learn = catalog.find(CatalogServiceId::Learn).expect("learn");
        let info = catalog.find(CatalogServiceId::Info).expect("info");
        let tunet = catalog.find(CatalogServiceId::Tunet).expect("tunet");

        let refresh_metadata = |entry: &ServiceCatalogEntry| {
            entry
                .definition
                .capability_metadata()
                .find(|metadata| metadata.key == ServiceCapability::Refresh.as_str())
                .expect("refresh metadata")
        };

        assert_eq!(
            refresh_metadata(learn).authentication,
            AuthenticationRequirement::IdentityAndServiceSession
        );
        assert_eq!(
            refresh_metadata(info).authentication,
            AuthenticationRequirement::IdentityAndServiceSession
        );
        assert_eq!(
            refresh_metadata(tunet).authentication,
            AuthenticationRequirement::ServiceSession
        );
    }

    #[test]
    fn required_services_have_distinct_categories_and_expected_presentation() {
        let catalog = ServiceCatalog::standard();

        let cases = [
            (
                CatalogServiceId::Info,
                ServiceCategory::Information,
                "信息门户",
                "article",
            ),
            (
                CatalogServiceId::Usereg,
                ServiceCategory::Network,
                "网络自助",
                "manage_accounts",
            ),
            (
                CatalogServiceId::Tunet,
                ServiceCategory::Network,
                "校园网",
                "wifi",
            ),
            (
                CatalogServiceId::Library,
                ServiceCategory::Library,
                "图书馆",
                "local_library",
            ),
            (
                CatalogServiceId::CampusCard,
                ServiceCategory::CampusCard,
                "校园卡",
                "credit_card",
            ),
        ];

        for (id, category, display_name, icon_key) in cases {
            let entry = catalog.find(id).expect("service is catalogued");
            assert_eq!(entry.category(), category);
            assert_eq!(entry.presentation().display_name, display_name);
            assert_eq!(entry.presentation().icon_key, icon_key);
        }
    }

    #[test]
    fn capability_metadata_describes_access_without_wire_details() {
        let catalog = ServiceCatalog::standard();
        let library = catalog
            .find(CatalogServiceId::Library)
            .expect("library is catalogued");
        let card = catalog
            .find(CatalogServiceId::CampusCard)
            .expect("campus card is catalogued");

        assert!(
            library
                .capabilities()
                .contains(ServiceCapability::ReadLibraryAreas)
        );
        assert!(
            library
                .capabilities()
                .contains(ServiceCapability::ReadLibrarySocketStatus)
        );
        assert!(
            !library
                .capabilities()
                .contains(ServiceCapability::ReadLibraryLoans)
        );
        assert!(
            card.capabilities()
                .contains(ServiceCapability::ReadCardBalance)
        );

        let balance = ServiceCapability::ReadCardBalance.metadata();
        assert_eq!(
            ServiceCapability::ReadCardBalance.as_str(),
            "read_card_balance"
        );
        assert_eq!(balance.key, "read_card_balance");
        assert_eq!(balance.access, CapabilityAccess::Read);
        assert_eq!(balance.access.as_str(), "read");
        assert_eq!(
            balance.authentication,
            AuthenticationRequirement::IdentityAndServiceSession
        );
        assert_eq!(
            balance.authentication.as_str(),
            "identity_and_service_session"
        );

        let debug = format!("{catalog:?}");
        for forbidden in ["http://", "https://", "cookie", "ticket", "csrf", "<html"] {
            assert!(
                !debug.to_ascii_lowercase().contains(forbidden),
                "catalog debug leaked forbidden marker: {forbidden}"
            );
        }
    }

    #[test]
    fn capability_contracts_bind_each_advertised_operation_to_a_proof_and_route() {
        let cases = [
            (
                CatalogServiceId::Learn,
                ServiceCapability::ReadCourses,
                "learn_service_session",
                "overview",
            ),
            (
                CatalogServiceId::Learn,
                ServiceCapability::ReadTermCalendar,
                "learn_service_session",
                "learn_calendar",
            ),
            (
                CatalogServiceId::Learn,
                ServiceCapability::ReadSchoolCalendar,
                "learn_service_session",
                "school_calendar",
            ),
            (
                CatalogServiceId::Learn,
                ServiceCapability::ReadAnnouncements,
                "learn_service_session",
                "learn_announcements",
            ),
            (
                CatalogServiceId::Learn,
                ServiceCapability::ReadSchedule,
                "overview_resolver",
                "overview",
            ),
            (
                CatalogServiceId::Learn,
                ServiceCapability::ReadAssignments,
                "learn_todos_session",
                "overview",
            ),
            (
                CatalogServiceId::Registrar,
                ServiceCapability::ReadGrades,
                "registrar_grades_session",
                "grades",
            ),
            (
                CatalogServiceId::Registrar,
                ServiceCapability::ReadExams,
                "registrar_exam_session",
                "exams",
            ),
            (
                CatalogServiceId::Library,
                ServiceCapability::ReadLibraryAreas,
                "library_service_session",
                "library_areas",
            ),
            (
                CatalogServiceId::Library,
                ServiceCapability::ReadLibrarySocketStatus,
                "library_service_session",
                "library_socket_status",
            ),
            (
                CatalogServiceId::CampusCard,
                ServiceCapability::ReadCardTransactions,
                "campus_card_service_session",
                "card_transactions",
            ),
            (
                CatalogServiceId::Usereg,
                ServiceCapability::ManageOnlineDevices,
                "usereg_service_session",
                "usereg_devices",
            ),
            (
                CatalogServiceId::Tunet,
                ServiceCapability::ReadNetworkStatus,
                "tunet_local_status",
                "tunet_status",
            ),
        ];

        for (service, capability, proof, route) in cases {
            let contract = capability
                .contract_for(service)
                .expect("capability must have an owner contract");
            assert_eq!(contract.proof, proof);
            assert_eq!(contract.route, route);
        }
    }

    #[test]
    fn standard_catalog_does_not_advertise_unproven_cross_service_capabilities() {
        let catalog = ServiceCatalog::standard();
        let learn = catalog.find(CatalogServiceId::Learn).unwrap();
        let registrar = catalog.find(CatalogServiceId::Registrar).unwrap();

        assert!(
            !learn
                .capabilities()
                .contains(ServiceCapability::ReadAssignments)
        );
        assert!(
            !registrar
                .capabilities()
                .contains(ServiceCapability::ReadCourses)
        );
        assert!(
            !registrar
                .capabilities()
                .contains(ServiceCapability::ReadSchedule)
        );
        assert!(
            ServiceCapability::ReadAssignments
                .contract_for(CatalogServiceId::Learn)
                .is_some()
        );
        assert!(
            ServiceCapability::ReadSchedule
                .contract_for(CatalogServiceId::Registrar)
                .is_none()
        );
    }

    #[test]
    fn status_updates_are_local_to_the_catalog_entry() {
        let mut catalog = ServiceCatalog::standard();

        assert_eq!(
            catalog.find(CatalogServiceId::Learn).unwrap().availability,
            ServiceAvailability::RequiresAuthentication
        );
        assert!(!ServiceAvailability::Unknown.is_usable());
        assert!(!ServiceAvailability::Unavailable.is_usable());
        assert!(ServiceAvailability::Available.is_usable());
        assert!(ServiceAvailability::Degraded.is_usable());

        catalog.set_availability(CatalogServiceId::Learn, ServiceAvailability::Available);
        catalog.set_availability(CatalogServiceId::Library, ServiceAvailability::Degraded);

        assert_eq!(
            catalog.find(CatalogServiceId::Learn).unwrap().availability,
            ServiceAvailability::Available
        );
        assert_eq!(
            catalog
                .find(CatalogServiceId::Library)
                .unwrap()
                .availability,
            ServiceAvailability::Degraded
        );
        assert_eq!(
            catalog
                .find(CatalogServiceId::CampusCard)
                .unwrap()
                .availability,
            ServiceAvailability::RequiresServiceSession
        );
    }

    #[test]
    fn service_session_services_start_with_a_service_handoff_requirement() {
        let catalog = ServiceCatalog::standard();

        for id in [
            CatalogServiceId::Info,
            CatalogServiceId::Classroom,
            CatalogServiceId::Electricity,
            CatalogServiceId::Library,
            CatalogServiceId::CampusCard,
        ] {
            let entry = catalog.find(id).expect("service is catalogued");
            assert_eq!(
                entry.definition.default_availability,
                ServiceAvailability::RequiresServiceSession,
                "{id} must expose its service-session boundary"
            );
            assert_eq!(
                entry.authentication_source(),
                ServiceAuthenticationSource::ServiceSession,
                "{id} must describe the target service session boundary"
            );
        }
    }

    #[test]
    fn refresh_metadata_uses_the_owner_service_authentication_boundary() {
        assert_eq!(
            ServiceCapability::Refresh
                .metadata_for(CatalogServiceId::Info)
                .authentication,
            AuthenticationRequirement::IdentityAndServiceSession
        );
        assert_eq!(
            ServiceCapability::Refresh
                .metadata_for(CatalogServiceId::Library)
                .authentication,
            AuthenticationRequirement::IdentityAndServiceSession
        );
        assert_eq!(
            ServiceCapability::Refresh
                .metadata_for(CatalogServiceId::Tunet)
                .authentication,
            AuthenticationRequirement::ServiceSession
        );
    }

    #[test]
    fn visibility_and_category_queries_preserve_canonical_order() {
        let catalog = ServiceCatalog::standard();

        let visible = catalog
            .visible_entries()
            .map(|entry| entry.id())
            .collect::<Vec<_>>();
        assert_eq!(visible.len(), 9);
        assert!(!visible.contains(&CatalogServiceId::Identity));
        assert_eq!(
            catalog
                .entries_in_category(ServiceCategory::Network)
                .map(|entry| entry.id())
                .collect::<Vec<_>>(),
            vec![CatalogServiceId::Usereg, CatalogServiceId::Tunet]
        );
    }
}
