//! Application-facing APIs exposed to the Flutter bridge.
//!
//! [`runtime`] owns the authenticated application session and service reads.
//! The other modules provide DTOs, overview composition, presentation metadata,
//! and the read-only backend validation interface.

pub mod academic;
pub mod campus;
pub mod experience;
pub mod runtime;
pub mod service_catalog;
pub mod thos;

pub use academic::{CampusGradeDto, CampusGradeReportDto};
pub use campus::{CampusOverviewFacadeDto, CampusSemesterScheduleDto, load_overview};
pub use service_catalog::{
    ServiceCapabilityMetadataDto, ServiceCatalogDto, ServiceCatalogEntryDto,
    ServicePresentationDto, list_service_catalog,
};
