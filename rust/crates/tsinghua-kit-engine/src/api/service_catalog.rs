//! Narrow Flutter bridge types for the transport-free service catalog.
//!
//! The domain catalog deliberately uses enums and borrowed static strings. The
//! bridge uses owned strings and simple scalar fields so its contract stays
//! stable for Dart consumers and does not expose protocol or session details.

#[cfg(feature = "ffi-bridge")]
use flutter_rust_bridge::frb;

use crate::service_catalog::{
    CatalogServiceId, ServiceAuthenticationSource, ServiceAvailability, ServiceCatalog,
    ServiceCatalogEntry, ServicePresentation,
};
use crate::{
    protocol::{ServiceId, ServiceSessionState},
    session::{SessionCoordinator, SessionSnapshot},
};

/// Proof flags supplied by the opaque runtime. A session state is necessary
/// but not sufficient for a service to be advertised as available: the
/// corresponding adapter must still be retained after its response proof.
/// `tunet` is process-local disconnect-target evidence, not an Auth session.
/// The flags contain no cookie, ticket, CSRF value, URL, or response data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RuntimeServiceProofs {
    pub learn: bool,
    pub registrar: bool,
    pub info: bool,
    pub classroom: bool,
    pub electricity: bool,
    pub library: bool,
    pub campus_card: bool,
    pub usereg: bool,
    pub tunet: bool,
}

/// A reader is an entry point, not proof that a remote service is connected.
/// Only the production runtime enables this after checking its current lease
/// and Identity proof. Each listed reader owns cache validation and bounded
/// lazy handoff; login/write capabilities never become speculative reads.
pub(crate) fn enable_automatic_readers(
    catalog: &mut ServiceCatalogDto,
    coordinator: &SessionCoordinator,
) {
    use crate::service_catalog::ServiceCapability as C;
    let identity = coordinator.registry().snapshot_for(ServiceId::Identity);
    if !is_currently_authenticated(&identity) {
        return;
    }
    for entry in &mut catalog.services {
        let (service, readers): (CatalogServiceId, &[C]) = match entry.id.as_str() {
            "learn" => (
                CatalogServiceId::Learn,
                &[C::ReadCourses, C::ReadAnnouncements, C::ReadSchedule],
            ),
            "registrar" => (CatalogServiceId::Registrar, &[C::ReadGrades, C::ReadExams]),
            "info" => (CatalogServiceId::Info, &[C::ReadAnnouncements]),
            "library" => (
                CatalogServiceId::Library,
                &[
                    C::ReadLibraryAreas,
                    C::ReadLibrarySeats,
                    C::ReadLibrarySocketStatus,
                ],
            ),
            "classroom" => (CatalogServiceId::Classroom, &[C::ReadClassroomAvailability]),
            "electricity" => (
                CatalogServiceId::Electricity,
                &[C::ReadElectricityRemainder, C::ReadElectricityHistory],
            ),
            "campus_card" => (
                CatalogServiceId::CampusCard,
                &[C::ReadCardBalance, C::ReadCardTransactions],
            ),
            _ => continue,
        };
        if !matches!(
            entry.availability.as_str(),
            "requires_service_session" | "available" | "degraded"
        ) {
            continue;
        }
        let Some(snapshot) = snapshot_for_catalog_service(coordinator, service) else {
            continue;
        };
        if snapshot.user.is_some() && snapshot.user != identity.user {
            continue; // Never grant a reader using another account's session.
        }
        for reader in readers {
            if !entry
                .capabilities
                .iter()
                .any(|capability| capability.key == reader.as_str())
            {
                entry.capabilities.push(map_capability(*reader, service));
            }
        }
        if entry.availability == "requires_service_session" {
            entry.availability = "load_on_demand".to_owned();
        }
    }
}

/// Cache-backed read capabilities are separate from live service proofs. A
/// degraded catalog entry may expose one of these reads only when the runtime
/// has already validated an account-bound, bounded-age cache envelope. These
/// flags never make a live service session look authenticated: they only let
/// the UI mount the corresponding Rust reader so that a fresh/stale result
/// can be returned before any handoff is attempted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RuntimeCacheCapabilities {
    pub learn_courses: bool,
    pub learn_announcements: bool,
    pub learn_schedule: bool,
    pub registrar_grades: bool,
    pub registrar_exams: bool,
    pub info_news: bool,
    pub library_areas: bool,
    pub electricity_history: bool,
    pub campus_card_account: bool,
    pub campus_card_transactions: bool,
}

/// Presentation metadata for one catalog service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePresentationDto {
    pub display_name: String,
    pub display_name_key: String,
    pub icon_key: String,
}

/// Safe metadata for one advertised service capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCapabilityMetadataDto {
    pub key: String,
    pub display_name_key: String,
    pub access: String,
    pub authentication: String,
}

/// One service entry exposed at the Flutter boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCatalogEntryDto {
    pub id: String,
    pub category: String,
    pub category_display_name_key: String,
    pub presentation: ServicePresentationDto,
    pub availability: String,
    /// Coarse authentication boundary, such as the shared identity session
    /// or an independent network login.  It contains no session material.
    pub authentication_source: Option<String>,
    pub visible_by_default: bool,
    pub capabilities: Vec<ServiceCapabilityMetadataDto>,
}

/// Stable service catalog envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCatalogDto {
    pub services: Vec<ServiceCatalogEntryDto>,
}

/// Lists the standard catalog without performing network or session work.
#[cfg_attr(feature = "ffi-bridge", frb)]
pub fn list_service_catalog() -> ServiceCatalogDto {
    map_catalog_with_filter(&ServiceCatalog::standard(), |_, _| true)
}

/// Lists the catalog using the current opaque runtime session registry.
///
/// This is intentionally a separate bridge method from the transport-free
/// default catalog.  The UI can therefore distinguish an identity session
/// that has already established Learn/Registrar proof from a service that
/// still needs a handoff or an independent login.
pub(crate) fn list_runtime_service_catalog(
    coordinator: &SessionCoordinator,
    proofs: RuntimeServiceProofs,
) -> ServiceCatalogDto {
    // Keep this compatibility helper useful for callers that only have the
    // coarse proof bundle. The opaque CampusRuntime uses the resolver-aware
    // variant below so an academic session pair cannot advertise the
    // aggregate overview route before its resolver has actually been built.
    list_runtime_service_catalog_with_resolver(coordinator, proofs, true)
}

/// Lists the runtime catalog while including the aggregate overview resolver
/// as a separate proof. Learn and Registrar session proofs alone do not make
/// `read_schedule` executable: the resolver must retain the paired live source
/// and validated cache policy as well.
pub(crate) fn list_runtime_service_catalog_with_resolver(
    coordinator: &SessionCoordinator,
    proofs: RuntimeServiceProofs,
    overview_resolver_proven: bool,
) -> ServiceCatalogDto {
    list_runtime_service_catalog_with_resolver_and_cache(
        coordinator,
        proofs,
        overview_resolver_proven,
        RuntimeCacheCapabilities::default(),
    )
}

/// Runtime catalog variant that can advertise narrowly scoped, validated
/// cache reads without turning those reads into live service proof.
pub(crate) fn list_runtime_service_catalog_with_resolver_and_cache(
    coordinator: &SessionCoordinator,
    proofs: RuntimeServiceProofs,
    overview_resolver_proven: bool,
    cache_capabilities: RuntimeCacheCapabilities,
) -> ServiceCatalogDto {
    let mut catalog = ServiceCatalog::standard();
    let identity = coordinator.registry().snapshot_for(ServiceId::Identity);

    catalog.set_availability(CatalogServiceId::Identity, identity_availability(&identity));
    let learn_availability = shared_identity_service_availability(
        &coordinator.registry().snapshot_for(ServiceId::Learn),
        &identity,
        proofs.learn,
    );
    catalog.set_availability(
        CatalogServiceId::Learn,
        availability_with_cache(
            learn_availability,
            cache_capabilities.learn_courses
                || cache_capabilities.learn_announcements
                || cache_capabilities.learn_schedule,
        ),
    );
    let registrar_availability = shared_identity_service_availability(
        &coordinator.registry().snapshot_for(ServiceId::Registrar),
        &identity,
        proofs.registrar,
    );
    catalog.set_availability(
        CatalogServiceId::Registrar,
        availability_with_cache(
            registrar_availability,
            cache_capabilities.registrar_grades || cache_capabilities.registrar_exams,
        ),
    );
    let info_availability = shared_identity_service_availability(
        &coordinator.registry().snapshot_for(ServiceId::Info),
        &identity,
        proofs.info,
    );
    catalog.set_availability(
        CatalogServiceId::Info,
        availability_with_cache(info_availability, cache_capabilities.info_news),
    );
    let info = coordinator.registry().snapshot_for(ServiceId::Info);
    catalog.set_availability(
        CatalogServiceId::Classroom,
        shared_identity_service_availability(&info, &identity, proofs.classroom),
    );
    catalog.set_availability(
        CatalogServiceId::Electricity,
        availability_with_cache(
            shared_identity_service_availability(&info, &identity, proofs.electricity),
            cache_capabilities.electricity_history,
        ),
    );
    let library_availability = shared_identity_service_availability(
        &coordinator.registry().snapshot_for(ServiceId::Library),
        &identity,
        proofs.library,
    );
    catalog.set_availability(
        CatalogServiceId::Library,
        availability_with_cache(library_availability, cache_capabilities.library_areas),
    );
    let campus_card_availability = shared_identity_service_availability(
        &coordinator.registry().snapshot_for(ServiceId::CampusCard),
        &identity,
        proofs.campus_card,
    );
    catalog.set_availability(
        CatalogServiceId::CampusCard,
        availability_with_cache(
            campus_card_availability,
            cache_capabilities.campus_card_account || cache_capabilities.campus_card_transactions,
        ),
    );
    catalog.set_availability(
        CatalogServiceId::Usereg,
        independent_service_availability(
            &coordinator.registry().snapshot_for(ServiceId::Usereg),
            proofs.usereg,
        ),
    );
    // The physical-IP portal probe and the SRun login action are available
    // without an SRun session. Only logout and account-bound operations rely
    // on the independent session proof.
    catalog.set_availability(CatalogServiceId::Tunet, ServiceAvailability::Available);

    map_runtime_catalog(
        &catalog,
        coordinator,
        &identity,
        &proofs,
        overview_resolver_proven,
        &cache_capabilities,
    )
}

/// Maps the runtime catalog only after applying capability-level proof rules.
///
/// `RuntimeServiceProofs` is intentionally coarse because the runtime owns
/// the opaque adapters.  The catalog remains conservative: a Learn proof is
/// enough for Learn courses and announcements, the overview route additionally
/// needs the paired Registrar proof, and the todos capability is never
/// advertised without a dedicated todos proof.  This keeps a
/// `without_todos` source from looking like it supports assignments.
fn map_runtime_catalog(
    catalog: &ServiceCatalog,
    coordinator: &SessionCoordinator,
    identity: &SessionSnapshot,
    proofs: &RuntimeServiceProofs,
    overview_resolver_proven: bool,
    cache_capabilities: &RuntimeCacheCapabilities,
) -> ServiceCatalogDto {
    map_catalog_with_filter(catalog, |service, capability| {
        let service_is_usable = catalog
            .find(service)
            .is_some_and(|entry| entry.availability.is_usable());
        service_is_usable
            && capability_is_proven(
                service,
                capability,
                coordinator,
                identity,
                proofs,
                overview_resolver_proven,
                cache_capabilities,
            )
    })
}

fn capability_is_proven(
    service: crate::service_catalog::CatalogServiceId,
    capability: crate::service_catalog::ServiceCapability,
    coordinator: &SessionCoordinator,
    identity: &SessionSnapshot,
    proofs: &RuntimeServiceProofs,
    overview_resolver_proven: bool,
    cache_capabilities: &RuntimeCacheCapabilities,
) -> bool {
    let Some(contract) = capability.contract_for(service) else {
        return false;
    };

    match contract.proof {
        "tunet_local_status" | "tunet_connect_action" => true,
        "tunet_connection_target" => proofs.tunet,
        "identity_authentication" => is_currently_authenticated(identity),
        "learn_service_session" => {
            let live =
                shared_service_session_is_current(coordinator, CatalogServiceId::Learn, identity)
                    && proofs.learn;
            let cached = match capability {
                crate::service_catalog::ServiceCapability::ReadCourses => {
                    cache_capabilities.learn_courses
                }
                crate::service_catalog::ServiceCapability::ReadAnnouncements => {
                    cache_capabilities.learn_announcements
                }
                _ => false,
            };
            live || cached
        }
        // The runtime establishes the overview resolver only after both the
        // Learn and Registrar proofs are retained. Keeping this predicate
        // here prevents a Registrar grades proof from advertising the shared
        // overview route by itself.
        "overview_resolver" => {
            let live =
                shared_service_session_is_current(coordinator, CatalogServiceId::Learn, identity)
                    && shared_service_session_is_current(
                        coordinator,
                        CatalogServiceId::Registrar,
                        identity,
                    )
                    && proofs.learn
                    && proofs.registrar
                    && overview_resolver_proven;
            live || (capability == crate::service_catalog::ServiceCapability::ReadSchedule
                && cache_capabilities.learn_schedule)
        }
        // There is no todos adapter proof in RuntimeServiceProofs. In
        // particular, the current without_todos source must not advertise
        // assignments merely because its Learn session is usable.
        "learn_todos_session" => false,
        "registrar_service_session" | "registrar_grades_session" | "registrar_exam_session" => {
            let live = shared_service_session_is_current(
                coordinator,
                CatalogServiceId::Registrar,
                identity,
            ) && proofs.registrar;
            let cached = match capability {
                crate::service_catalog::ServiceCapability::ReadGrades => {
                    cache_capabilities.registrar_grades
                }
                crate::service_catalog::ServiceCapability::ReadExams => {
                    cache_capabilities.registrar_exams
                }
                _ => false,
            };
            live || cached
        }
        "info_service_session" => {
            let live_info =
                shared_service_session_is_current(coordinator, CatalogServiceId::Info, identity)
                    && proofs.info;
            // INFO's list/search reader validates the account scope and age
            // of the cache before returning it. This is intentionally
            // read-only and does not promote the INFO session itself to a
            // live proof.
            let cached_info_news = service == CatalogServiceId::Info
                && capability == crate::service_catalog::ServiceCapability::ReadAnnouncements
                && cache_capabilities.info_news;
            live_info || cached_info_news
        }
        "library_service_session" => {
            let live =
                shared_service_session_is_current(coordinator, CatalogServiceId::Library, identity)
                    && proofs.library;
            live || (capability == crate::service_catalog::ServiceCapability::ReadLibraryAreas
                && cache_capabilities.library_areas)
        }
        "electricity_service_session" => {
            let live =
                shared_service_session_is_current(coordinator, CatalogServiceId::Info, identity)
                    && proofs.electricity;
            live || (capability
                == crate::service_catalog::ServiceCapability::ReadElectricityHistory
                && cache_capabilities.electricity_history)
        }
        "campus_card_service_session" => {
            let live = shared_service_session_is_current(
                coordinator,
                CatalogServiceId::CampusCard,
                identity,
            ) && proofs.campus_card;
            live || match capability {
                crate::service_catalog::ServiceCapability::ReadCardBalance => {
                    cache_capabilities.campus_card_account
                }
                crate::service_catalog::ServiceCapability::ReadCardTransactions => {
                    cache_capabilities.campus_card_transactions
                }
                _ => false,
            }
        }
        "classroom_service_session" => {
            shared_service_session_is_current(coordinator, CatalogServiceId::Info, identity)
                && proofs.classroom
        }
        "usereg_service_session" => {
            independent_service_session_is_current(coordinator, CatalogServiceId::Usereg)
                && proofs.usereg
        }
        _ => false,
    }
}

fn identity_availability(snapshot: &SessionSnapshot) -> ServiceAvailability {
    match snapshot.state {
        ServiceSessionState::Authenticated if is_currently_authenticated(snapshot) => {
            ServiceAvailability::Available
        }
        ServiceSessionState::RequiresSecondFactor if second_factor_is_current(snapshot) => {
            ServiceAvailability::RequiresSecondFactor
        }
        ServiceSessionState::Authenticated
        | ServiceSessionState::RequiresSecondFactor
        | ServiceSessionState::Authenticating
        | ServiceSessionState::Anonymous
        | ServiceSessionState::Expired => ServiceAvailability::RequiresAuthentication,
    }
}

fn shared_identity_service_availability(
    snapshot: &SessionSnapshot,
    identity: &SessionSnapshot,
    service_proven: bool,
) -> ServiceAvailability {
    // A downstream adapter proof is scoped by the identity session that made
    // the handoff. If that identity session has expired or been logged out,
    // the downstream proof cannot make an identity-and-service capability
    // usable on its own.
    if !is_currently_authenticated(identity) {
        return ServiceAvailability::RequiresAuthentication;
    }

    match snapshot.state {
        ServiceSessionState::Authenticated
            if service_session_is_current(snapshot, identity) && service_proven =>
        {
            ServiceAvailability::Available
        }
        ServiceSessionState::RequiresSecondFactor if second_factor_is_current(snapshot) => {
            ServiceAvailability::RequiresSecondFactor
        }
        ServiceSessionState::Authenticated
        | ServiceSessionState::RequiresSecondFactor
        | ServiceSessionState::Authenticating
        | ServiceSessionState::Anonymous
        | ServiceSessionState::Expired => ServiceAvailability::RequiresServiceSession,
    }
}

fn independent_service_availability(
    snapshot: &SessionSnapshot,
    service_proven: bool,
) -> ServiceAvailability {
    match snapshot.state {
        ServiceSessionState::Authenticated
            if is_currently_authenticated(snapshot) && service_proven =>
        {
            ServiceAvailability::Available
        }
        ServiceSessionState::RequiresSecondFactor if second_factor_is_current(snapshot) => {
            ServiceAvailability::RequiresSecondFactor
        }
        ServiceSessionState::Authenticated | ServiceSessionState::RequiresSecondFactor => {
            ServiceAvailability::RequiresIndependentLogin
        }
        ServiceSessionState::Anonymous
        | ServiceSessionState::Authenticating
        | ServiceSessionState::Expired => ServiceAvailability::RequiresIndependentLogin,
    }
}

/// A validated cache can make one or more read capabilities usable while the
/// corresponding live service still needs a handoff. Keep all other states
/// untouched: a service that is unavailable, not configured, or waiting for a
/// current second factor must not be relabelled as cache-backed by accident.
fn availability_with_cache(
    availability: ServiceAvailability,
    cache_available: bool,
) -> ServiceAvailability {
    if cache_available
        && matches!(
            availability,
            ServiceAvailability::RequiresAuthentication
                | ServiceAvailability::RequiresServiceSession
                | ServiceAvailability::Degraded
        )
    {
        ServiceAvailability::Degraded
    } else {
        availability
    }
}

fn is_currently_authenticated(snapshot: &SessionSnapshot) -> bool {
    snapshot.state == ServiceSessionState::Authenticated
        && snapshot.user.is_some()
        && snapshot
            .expires_at
            .is_none_or(|expires_at| expires_at > chrono::Utc::now())
}

fn second_factor_is_current(snapshot: &SessionSnapshot) -> bool {
    snapshot.state == ServiceSessionState::RequiresSecondFactor
        && snapshot.second_factor.as_ref().is_some_and(|challenge| {
            challenge
                .expires_at
                .is_none_or(|expires_at| expires_at > chrono::Utc::now())
        })
}

fn protocol_service_id(service: CatalogServiceId) -> Option<ServiceId> {
    match service {
        CatalogServiceId::Identity => Some(ServiceId::Identity),
        CatalogServiceId::Learn => Some(ServiceId::Learn),
        CatalogServiceId::Registrar => Some(ServiceId::Registrar),
        CatalogServiceId::Info => Some(ServiceId::Info),
        CatalogServiceId::Usereg => Some(ServiceId::Usereg),
        // TUNet status and its process-local disconnect capability are not
        // authentication sessions and have no registry entry.
        CatalogServiceId::Tunet => None,
        CatalogServiceId::Library => Some(ServiceId::Library),
        CatalogServiceId::CampusCard => Some(ServiceId::CampusCard),
        // These two catalog entries are INFO/WebVPN applications and share
        // the INFO registry session; their own adapter proof is supplied by
        // RuntimeServiceProofs.
        CatalogServiceId::Classroom | CatalogServiceId::Electricity => Some(ServiceId::Info),
    }
}

fn snapshot_for_catalog_service(
    coordinator: &SessionCoordinator,
    service: CatalogServiceId,
) -> Option<SessionSnapshot> {
    protocol_service_id(service).map(|service| coordinator.registry().snapshot_for(service))
}

fn service_session_is_current(snapshot: &SessionSnapshot, identity: &SessionSnapshot) -> bool {
    is_currently_authenticated(identity)
        && is_currently_authenticated(snapshot)
        && snapshot.user == identity.user
}

fn shared_service_session_is_current(
    coordinator: &SessionCoordinator,
    service: CatalogServiceId,
    identity: &SessionSnapshot,
) -> bool {
    snapshot_for_catalog_service(coordinator, service)
        .is_some_and(|snapshot| service_session_is_current(&snapshot, identity))
}

fn independent_service_session_is_current(
    coordinator: &SessionCoordinator,
    service: CatalogServiceId,
) -> bool {
    snapshot_for_catalog_service(coordinator, service)
        .is_some_and(|snapshot| is_currently_authenticated(&snapshot))
}

fn map_catalog_with_filter<F>(catalog: &ServiceCatalog, include: F) -> ServiceCatalogDto
where
    F: Fn(
        crate::service_catalog::CatalogServiceId,
        crate::service_catalog::ServiceCapability,
    ) -> bool,
{
    ServiceCatalogDto {
        services: catalog
            .entries()
            .iter()
            .map(|entry| map_entry(entry, &include))
            .collect(),
    }
}

fn map_entry<F>(entry: &ServiceCatalogEntry, include: &F) -> ServiceCatalogEntryDto
where
    F: Fn(
        crate::service_catalog::CatalogServiceId,
        crate::service_catalog::ServiceCapability,
    ) -> bool,
{
    let category = entry.category();
    let presentation = entry.presentation();
    let capabilities = entry
        .capabilities()
        .items
        .iter()
        .copied()
        .filter(|capability| include(entry.id(), *capability))
        .map(|capability| map_capability(capability, entry.id()))
        .collect::<Vec<_>>();

    ServiceCatalogEntryDto {
        id: entry.id().as_str().to_owned(),
        category: category.as_str().to_owned(),
        category_display_name_key: category.display_name_key().to_owned(),
        presentation: map_presentation(presentation),
        availability: entry.availability.as_str().to_owned(),
        authentication_source: match entry.authentication_source() {
            ServiceAuthenticationSource::None => None,
            source => Some(source.as_str().to_owned()),
        },
        visible_by_default: entry.is_visible(),
        capabilities,
    }
}

fn map_presentation(presentation: ServicePresentation) -> ServicePresentationDto {
    ServicePresentationDto {
        display_name: presentation.display_name.to_owned(),
        display_name_key: presentation.display_name_key.to_owned(),
        icon_key: presentation.icon_key.to_owned(),
    }
}

fn map_capability(
    capability: crate::service_catalog::ServiceCapability,
    service: crate::service_catalog::CatalogServiceId,
) -> ServiceCapabilityMetadataDto {
    let metadata = capability.metadata_for(service);
    ServiceCapabilityMetadataDto {
        key: metadata.key.to_owned(),
        display_name_key: metadata.display_name_key.to_owned(),
        access: metadata.access.as_str().to_owned(),
        authentication: metadata.authentication.as_str().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::UserIdentity;
    use crate::service_catalog::{
        AuthenticationRequirement, CatalogServiceId, ServiceAvailability, ServiceCapability,
        ServiceCategory,
    };

    #[test]
    fn maps_the_standard_catalog_in_canonical_order() {
        let mapped = list_service_catalog();

        assert_eq!(mapped.services.len(), 10);
        assert_eq!(
            mapped
                .services
                .iter()
                .map(|service| service.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "identity",
                "learn",
                "registrar",
                "info",
                "classroom",
                "electricity",
                "usereg",
                "tunet",
                "library",
                "campus_card",
            ]
        );
        assert!(!mapped.services[0].visible_by_default);
        assert_eq!(mapped.services[0].authentication_source, None);
        assert!(
            mapped.services[1..]
                .iter()
                .all(|service| service.visible_by_default)
        );
    }

    #[test]
    fn maps_presentation_status_and_capability_metadata() {
        let mapped = list_service_catalog();
        let library = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::Library.as_str())
            .expect("library is catalogued");

        assert_eq!(library.category, ServiceCategory::Library.as_str());
        assert_eq!(
            library.category_display_name_key,
            ServiceCategory::Library.display_name_key()
        );
        assert_eq!(library.presentation.display_name, "图书馆");
        assert_eq!(
            library.presentation.display_name_key,
            "service.library.name"
        );
        assert_eq!(library.presentation.icon_key, "local_library");
        assert_eq!(
            library.availability,
            ServiceAvailability::RequiresServiceSession.as_str()
        );
        assert_eq!(
            library.authentication_source.as_deref(),
            Some("service_session")
        );

        let info = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::Info.as_str())
            .expect("info is catalogued");
        assert_eq!(
            info.authentication_source.as_deref(),
            Some("service_session")
        );
        let campus_card = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::CampusCard.as_str())
            .expect("campus card is catalogued");
        assert_eq!(
            campus_card.authentication_source.as_deref(),
            Some("service_session")
        );
        let refresh = info
            .capabilities
            .iter()
            .find(|capability| capability.key == ServiceCapability::Refresh.as_str())
            .expect("info refresh capability is advertised");
        assert_eq!(
            refresh.authentication,
            AuthenticationRequirement::IdentityAndServiceSession.as_str()
        );

        let areas = library
            .capabilities
            .iter()
            .find(|capability| capability.key == ServiceCapability::ReadLibraryAreas.as_str())
            .expect("library area capability is advertised");
        let metadata = ServiceCapability::ReadLibraryAreas.metadata();
        assert_eq!(areas.display_name_key, metadata.display_name_key);
        assert_eq!(areas.access, metadata.access.as_str());
        assert_eq!(areas.authentication, metadata.authentication.as_str());

        let registrar = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::Registrar.as_str())
            .expect("registrar is catalogued");
        let grades = registrar
            .capabilities
            .iter()
            .find(|capability| capability.key == ServiceCapability::ReadGrades.as_str())
            .expect("registrar grades capability is advertised");
        let grade_metadata = ServiceCapability::ReadGrades.metadata();
        assert_eq!(grades.display_name_key, grade_metadata.display_name_key);
        assert_eq!(grades.access, grade_metadata.access.as_str());
        assert_eq!(
            grades.authentication,
            grade_metadata.authentication.as_str()
        );
    }

    #[test]
    fn learn_catalog_advertises_the_real_announcement_capability() {
        let mapped = list_service_catalog();
        let learn = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::Learn.as_str())
            .expect("learn is catalogued");
        let announcements = learn
            .capabilities
            .iter()
            .find(|capability| capability.key == ServiceCapability::ReadAnnouncements.as_str())
            .expect("learn announcements capability is advertised");
        let metadata = ServiceCapability::ReadAnnouncements.metadata_for(CatalogServiceId::Learn);

        assert_eq!(announcements.display_name_key, metadata.display_name_key);
        assert_eq!(announcements.access, "read");
        assert_eq!(announcements.access, metadata.access.as_str());
        assert_eq!(announcements.authentication, "identity_and_service_session");
        assert_eq!(
            announcements.authentication,
            metadata.authentication.as_str()
        );
    }

    #[test]
    fn backend_repair_learn_term_calendar_catalog_requires_live_learn_proof() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "fixture-calendar-user".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .unwrap();
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .unwrap();
        let cached = list_runtime_service_catalog_with_resolver_and_cache(
            &coordinator,
            RuntimeServiceProofs::default(),
            false,
            RuntimeCacheCapabilities {
                learn_courses: true,
                ..RuntimeCacheCapabilities::default()
            },
        );
        let has_calendar = |catalog: &ServiceCatalogDto| {
            catalog
                .services
                .iter()
                .find(|service| service.id == "learn")
                .unwrap()
                .capabilities
                .iter()
                .any(|capability| capability.key == "read_term_calendar")
        };
        assert!(!has_calendar(&cached));

        coordinator.begin_authentication(ServiceId::Learn).unwrap();
        coordinator
            .mark_authenticated(ServiceId::Learn, user, None, None, None)
            .unwrap();
        let live = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                ..RuntimeServiceProofs::default()
            },
        );
        assert!(has_calendar(&live));
        assert_eq!(
            ServiceCapability::ReadTermCalendar
                .contract_for(CatalogServiceId::Learn)
                .unwrap()
                .route,
            "learn_calendar"
        );
    }

    #[test]
    fn backend_repair_school_calendar_catalog_requires_live_learn_proof_and_fixed_route() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "fixture-school-calendar".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .unwrap();
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .unwrap();
        let contains = |catalog: ServiceCatalogDto| {
            catalog
                .services
                .into_iter()
                .find(|service| service.id == "learn")
                .unwrap()
                .capabilities
                .iter()
                .any(|capability| capability.key == "read_school_calendar")
        };
        assert!(!contains(list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs::default(),
        )));
        coordinator.begin_authentication(ServiceId::Learn).unwrap();
        coordinator
            .mark_authenticated(ServiceId::Learn, user, None, None, None)
            .unwrap();
        assert!(contains(list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                ..RuntimeServiceProofs::default()
            },
        )));
        let contract = ServiceCapability::ReadSchoolCalendar
            .contract_for(CatalogServiceId::Learn)
            .unwrap();
        assert_eq!(contract.proof, "learn_service_session");
        assert_eq!(contract.route, "school_calendar");
        assert!(
            ServiceCapability::ReadSchoolCalendar
                .contract_for(CatalogServiceId::Info)
                .is_none()
        );
    }

    #[test]
    fn runtime_catalog_scopes_learn_capabilities_to_the_proofs_that_exist() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .expect("identity session");
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn authentication");
        coordinator
            .mark_authenticated(ServiceId::Learn, user, None, None, None)
            .expect("learn session");

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let learn = mapped
            .services
            .iter()
            .find(|service| service.id == "learn")
            .expect("learn catalog entry");
        let keys = learn
            .capabilities
            .iter()
            .map(|capability| capability.key.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                "refresh",
                "read_courses",
                "read_term_calendar",
                "read_school_calendar",
                "read_announcements"
            ]
        );
        assert!(!keys.contains(&"read_assignments"));
        assert!(!keys.contains(&"read_schedule"));
        assert!(
            mapped
                .services
                .iter()
                .find(|service| service.id == "registrar")
                .expect("registrar catalog entry")
                .capabilities
                .is_empty()
        );
    }

    #[test]
    fn runtime_catalog_requires_both_academic_proofs_for_overview_and_keeps_grades_separate() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        for service in [ServiceId::Identity, ServiceId::Learn, ServiceId::Registrar] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                registrar: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let capabilities = |id: &str| {
            mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .expect("catalog entry")
                .capabilities
                .iter()
                .map(|capability| capability.key.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            capabilities("learn"),
            vec![
                "refresh",
                "read_courses",
                "read_term_calendar",
                "read_school_calendar",
                "read_schedule",
                "read_announcements"
            ]
        );
        assert_eq!(
            capabilities("registrar"),
            vec!["refresh", "read_grades", "read_exams"]
        );
    }

    #[test]
    fn runtime_catalog_hides_overview_until_the_resolver_is_retained() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        for service in [ServiceId::Identity, ServiceId::Learn, ServiceId::Registrar] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog_with_resolver(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                registrar: true,
                ..RuntimeServiceProofs::default()
            },
            false,
        );
        let learn = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::Learn.as_str())
            .expect("learn catalog entry");
        assert!(
            learn
                .capabilities
                .iter()
                .all(|capability| capability.key != "read_schedule")
        );
    }

    #[test]
    fn authenticated_but_unproven_services_keep_their_status_and_expose_no_capabilities() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        for service in [ServiceId::Identity, ServiceId::Learn, ServiceId::Library] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog(&coordinator, RuntimeServiceProofs::default());
        for id in ["learn", "library"] {
            let service = mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .expect("catalog entry");
            assert_eq!(
                service.availability,
                ServiceAvailability::RequiresServiceSession.as_str()
            );
            assert!(service.capabilities.is_empty(), "{id} must stay gated");
        }
    }

    #[test]
    fn bridge_mapping_contains_only_safe_catalog_strings() {
        let debug = format!("{:?}", list_service_catalog());
        for forbidden in [
            "http://", "https://", "cookie", "ticket", "csrf", "password", "<html",
        ] {
            assert!(
                !debug.to_ascii_lowercase().contains(forbidden),
                "bridge catalog leaked forbidden marker: {forbidden}"
            );
        }
    }

    #[test]
    fn runtime_mapping_promotes_proven_sessions_and_preserves_boundaries() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .expect("identity session");
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn authentication");
        coordinator
            .mark_authenticated(ServiceId::Learn, user.clone(), None, None, None)
            .expect("learn session");

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let availability = |id: &str| {
            mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .map(|service| service.availability.as_str())
                .expect("catalog entry")
        };

        assert_eq!(availability("identity"), "available");
        assert_eq!(availability("learn"), "available");
        assert_eq!(availability("registrar"), "requires_service_session");
        assert_eq!(availability("info"), "requires_service_session");
        assert_eq!(availability("usereg"), "requires_independent_login");
        assert_eq!(availability("tunet"), "available");
        assert_eq!(availability("library"), "requires_service_session");
        assert_eq!(availability("campus_card"), "requires_service_session");
    }

    #[test]
    fn backend_repair_tunet_is_local_network_state_not_an_auth_session() {
        let coordinator = SessionCoordinator::new();
        let mapped = list_runtime_service_catalog(&coordinator, RuntimeServiceProofs::default());
        let tunet = mapped
            .services
            .iter()
            .find(|entry| entry.id == "tunet")
            .unwrap();
        let keys: Vec<_> = tunet
            .capabilities
            .iter()
            .map(|item| item.key.as_str())
            .collect();
        assert_eq!(tunet.availability, "available");
        assert_eq!(
            tunet.authentication_source.as_deref(),
            Some("local_network_context")
        );
        assert!(keys.contains(&"read_network_status"));
        assert!(keys.contains(&"connect_network"));
        assert!(!keys.contains(&"disconnect_network"));
        assert!(keys.contains(&"refresh"));

        let mapped_with_target = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                tunet: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let tunet_with_target = mapped_with_target
            .services
            .iter()
            .find(|entry| entry.id == "tunet")
            .unwrap();
        assert!(
            tunet_with_target
                .capabilities
                .iter()
                .any(|item| item.key == "disconnect_network")
        );
    }

    #[test]
    fn an_authenticated_session_without_adapter_proof_is_not_available() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .expect("identity session");
        coordinator
            .begin_authentication(ServiceId::Info)
            .expect("info authentication");
        coordinator
            .mark_authenticated(ServiceId::Info, user, None, None, None)
            .expect("unproven info state fixture");

        let mapped = list_runtime_service_catalog(&coordinator, RuntimeServiceProofs::default());
        let info = mapped
            .services
            .iter()
            .find(|service| service.id == "info")
            .expect("info catalog entry");

        assert_eq!(info.availability, "requires_service_session");
    }

    #[test]
    fn a_downstream_proof_without_authenticated_identity_is_not_available() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn authentication");
        coordinator
            .mark_authenticated(ServiceId::Learn, user, None, None, None)
            .expect("learn session");

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let learn = mapped
            .services
            .iter()
            .find(|service| service.id == CatalogServiceId::Learn.as_str())
            .expect("learn catalog entry");

        assert_eq!(learn.availability, "requires_authentication");
    }

    #[test]
    fn authenticated_service_states_without_runtime_proof_never_become_available() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .expect("identity session");

        for service in [
            ServiceId::Learn,
            ServiceId::Registrar,
            ServiceId::Info,
            ServiceId::Library,
            ServiceId::CampusCard,
            ServiceId::Usereg,
        ] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog(&coordinator, RuntimeServiceProofs::default());
        for service in mapped
            .services
            .iter()
            .filter(|service| service.id != "identity")
        {
            assert_ne!(
                service.availability,
                ServiceAvailability::Available.as_str(),
                "{} must stay gated without its runtime proof",
                service.id
            );
        }
    }

    #[test]
    fn proven_shared_adapters_expose_real_capabilities_and_owner_auth_metadata() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        for service in [
            ServiceId::Identity,
            ServiceId::Learn,
            ServiceId::Info,
            ServiceId::Library,
            ServiceId::CampusCard,
        ] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                info: true,
                classroom: true,
                electricity: true,
                library: true,
                campus_card: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let service = |id: &str| {
            mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .expect("catalog entry")
        };
        let keys = |id: &str| {
            service(id)
                .capabilities
                .iter()
                .map(|capability| capability.key.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(service("learn").availability, "available");
        assert_eq!(
            keys("learn"),
            vec![
                "refresh",
                "read_courses",
                "read_term_calendar",
                "read_school_calendar",
                "read_announcements"
            ]
        );
        assert_eq!(service("info").availability, "available");
        assert_eq!(keys("info"), vec!["refresh", "read_announcements"]);
        assert_eq!(service("library").availability, "available");
        assert_eq!(
            keys("library"),
            vec![
                "refresh",
                "read_library_areas",
                "read_library_seats",
                "read_library_socket_status"
            ]
        );
        assert_eq!(service("campus_card").availability, "available");
        assert_eq!(
            keys("campus_card"),
            vec!["refresh", "read_card_balance", "read_card_transactions"]
        );

        for id in ["info", "classroom", "electricity", "library", "campus_card"] {
            assert_eq!(
                service(id).authentication_source.as_deref(),
                Some("service_session")
            );
            let refresh = service(id)
                .capabilities
                .iter()
                .find(|capability| capability.key == "refresh")
                .expect("refresh capability");
            assert_eq!(refresh.authentication, "identity_and_service_session");
        }
    }

    #[test]
    fn shared_services_without_adapter_proof_are_gated_and_filter_all_capabilities() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        for service in [
            ServiceId::Identity,
            ServiceId::Learn,
            ServiceId::Info,
            ServiceId::Library,
            ServiceId::CampusCard,
        ] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog(&coordinator, RuntimeServiceProofs::default());
        for id in [
            "learn",
            "info",
            "classroom",
            "electricity",
            "library",
            "campus_card",
        ] {
            let service = mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .expect("catalog entry");
            assert_eq!(
                service.availability,
                ServiceAvailability::RequiresServiceSession.as_str(),
                "{id} must wait for its adapter proof"
            );
            assert!(
                service.capabilities.is_empty(),
                "{id} must not advertise reads before its adapter proof"
            );
        }
    }

    #[test]
    fn runtime_catalog_exposes_only_validated_cache_reads_without_live_proof() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "2026000000".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(
                ServiceId::Identity,
                user,
                None,
                None,
                Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
            )
            .expect("expired identity context");

        let mapped = list_runtime_service_catalog_with_resolver_and_cache(
            &coordinator,
            RuntimeServiceProofs::default(),
            false,
            RuntimeCacheCapabilities {
                learn_courses: true,
                learn_announcements: true,
                learn_schedule: true,
                registrar_grades: true,
                registrar_exams: true,
                info_news: true,
                library_areas: true,
                electricity_history: true,
                campus_card_account: true,
                campus_card_transactions: true,
            },
        );
        let service = |id: &str| {
            mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .expect("catalog entry")
        };
        let keys = |id: &str| {
            service(id)
                .capabilities
                .iter()
                .map(|capability| capability.key.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(service("learn").availability, "degraded");
        assert_eq!(
            keys("learn"),
            vec!["read_courses", "read_schedule", "read_announcements"]
        );
        assert_eq!(service("registrar").availability, "degraded");
        assert_eq!(keys("registrar"), vec!["read_grades", "read_exams"]);
        assert_eq!(service("info").availability, "degraded");
        assert_eq!(keys("info"), vec!["read_announcements"]);
        assert_eq!(service("library").availability, "degraded");
        assert_eq!(keys("library"), vec!["read_library_areas"]);
        assert_eq!(service("electricity").availability, "degraded");
        assert_eq!(keys("electricity"), vec!["read_electricity_history"]);
        assert_eq!(service("campus_card").availability, "degraded");
        assert_eq!(
            keys("campus_card"),
            vec!["read_card_balance", "read_card_transactions"]
        );
    }

    #[test]
    fn expired_service_proof_is_gated_even_before_the_state_machine_is_expired() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .expect("identity session");
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn authentication");
        coordinator
            .mark_authenticated(
                ServiceId::Learn,
                user,
                None,
                None,
                Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
            )
            .expect("expired learn session fixture");

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let learn = mapped
            .services
            .iter()
            .find(|service| service.id == "learn")
            .expect("learn catalog entry");

        assert_eq!(learn.availability, "requires_service_session");
        assert!(learn.capabilities.is_empty());
    }

    #[test]
    fn expired_identity_gates_all_shared_service_proofs() {
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(
                ServiceId::Identity,
                user.clone(),
                None,
                None,
                Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
            )
            .expect("expired identity fixture");
        for service in [
            ServiceId::Learn,
            ServiceId::Info,
            ServiceId::Library,
            ServiceId::CampusCard,
        ] {
            coordinator
                .begin_authentication(service)
                .expect("service authentication");
            coordinator
                .mark_authenticated(service, user.clone(), None, None, None)
                .expect("service session");
        }

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                learn: true,
                info: true,
                library: true,
                campus_card: true,
                ..RuntimeServiceProofs::default()
            },
        );
        for id in [
            "learn",
            "info",
            "classroom",
            "electricity",
            "library",
            "campus_card",
        ] {
            let service = mapped
                .services
                .iter()
                .find(|service| service.id == id)
                .expect("catalog entry");
            assert_eq!(
                service.availability,
                ServiceAvailability::RequiresAuthentication.as_str(),
                "{id} must follow the expired identity session"
            );
            assert!(service.capabilities.is_empty());
        }
    }

    #[test]
    fn stale_proof_for_another_identity_cannot_make_a_shared_service_available() {
        let mut coordinator = SessionCoordinator::new();
        let identity_user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        let stale_service_user = UserIdentity {
            username: "another-student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity authentication");
        coordinator
            .mark_authenticated(ServiceId::Identity, identity_user, None, None, None)
            .expect("identity session");
        coordinator
            .begin_authentication(ServiceId::Library)
            .expect("library authentication");
        coordinator
            .mark_authenticated(ServiceId::Library, stale_service_user, None, None, None)
            .expect("stale library session fixture");

        let mapped = list_runtime_service_catalog(
            &coordinator,
            RuntimeServiceProofs {
                library: true,
                ..RuntimeServiceProofs::default()
            },
        );
        let library = mapped
            .services
            .iter()
            .find(|service| service.id == "library")
            .expect("library catalog entry");

        assert_eq!(library.availability, "requires_service_session");
        assert!(library.capabilities.is_empty());
    }
}
