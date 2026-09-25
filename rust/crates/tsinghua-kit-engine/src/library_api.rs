//! Public, context-bound domain values for read-only library queries.
//!
//! The service returns numeric area, time-window, and seat identifiers.  They
//! are kept inside opaque references so applications cannot mix a library
//! root with a seat section or reuse selectors from another Client.

use std::{
    collections::{HashMap, HashSet},
    fmt,
};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, Utc};

use crate::error::{Error, ErrorCode, Service};
use crate::library_read::LibrarySocketState;

/// The only dates supported by the library seat directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LibraryDay {
    /// The current date in the campus time zone.
    Today,
    /// The next date in the campus time zone.
    Tomorrow,
}

impl LibraryDay {
    pub(crate) fn date_at(self, now: DateTime<Utc>) -> Option<NaiveDate> {
        let campus_zone = FixedOffset::east_opt(8 * 60 * 60)?;
        let today = now.with_timezone(&campus_zone).date_naive();
        match self {
            Self::Today => Some(today),
            Self::Tomorrow => today.succ_opt(),
        }
    }
}

/// A reference to a root library returned by this Client's latest directory.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LibraryRef {
    pub(crate) owner: uuid::Uuid,
    pub(crate) generation: u64,
    pub(crate) id: u64,
}

impl LibraryRef {
    pub(crate) fn new(owner: uuid::Uuid, generation: u64, id: u64) -> Self {
        Self {
            owner,
            generation,
            id,
        }
    }

    pub(crate) fn belongs_to(&self, owner: uuid::Uuid, generation: u64) -> bool {
        self.owner == owner && self.generation == generation
    }
}

impl fmt::Debug for LibraryRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LibraryRef(<redacted>)")
    }
}

/// A reference to a floor returned for a selected library.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FloorRef {
    pub(crate) library: LibraryRef,
    pub(crate) generation: u64,
    pub(crate) id: u64,
}

impl FloorRef {
    pub(crate) fn new(library: LibraryRef, generation: u64, id: u64) -> Self {
        Self {
            library,
            generation,
            id,
        }
    }

    pub(crate) fn belongs_to(
        &self,
        owner: uuid::Uuid,
        directory_generation: u64,
        floor_generation: u64,
    ) -> bool {
        self.library.belongs_to(owner, directory_generation) && self.generation == floor_generation
    }
}

impl fmt::Debug for FloorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FloorRef(<redacted>)")
    }
}

/// A reference to a floor section selected for a campus date.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SectionRef {
    pub(crate) floor: FloorRef,
    pub(crate) generation: u64,
    pub(crate) id: u64,
    pub(crate) day: NaiveDate,
}

impl SectionRef {
    pub(crate) fn new(floor: FloorRef, generation: u64, id: u64, day: NaiveDate) -> Self {
        Self {
            floor,
            generation,
            id,
            day,
        }
    }

    pub(crate) fn belongs_to(
        &self,
        owner: uuid::Uuid,
        directory_generation: u64,
        floor_generation: u64,
        section_generation: u64,
    ) -> bool {
        self.floor
            .belongs_to(owner, directory_generation, floor_generation)
            && self.generation == section_generation
    }

    /// Returns the campus date for which this section was selected.
    pub fn day(&self) -> NaiveDate {
        self.day
    }
}

impl fmt::Debug for SectionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SectionRef(<redacted>)")
    }
}

/// A time window returned for one selected section and campus date.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SeatWindowRef {
    pub(crate) section: SectionRef,
    pub(crate) segment_id: u64,
    pub(crate) start_time: NaiveTime,
    pub(crate) end_time: NaiveTime,
}

impl SeatWindowRef {
    pub(crate) fn new(
        section: SectionRef,
        segment_id: u64,
        start_time: NaiveTime,
        end_time: NaiveTime,
    ) -> Self {
        Self {
            section,
            segment_id,
            start_time,
            end_time,
        }
    }
}

impl fmt::Debug for SeatWindowRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SeatWindowRef(<redacted>)")
    }
}

/// A seat selector scoped to the section read that returned it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SeatRef {
    pub(crate) section: SectionRef,
    pub(crate) id: u64,
}

impl SeatRef {
    pub(crate) fn new(section: SectionRef, id: u64) -> Self {
        Self { section, id }
    }

    pub(crate) fn belongs_to(&self, section: &SectionRef) -> bool {
        self.section == *section
    }
}

impl fmt::Debug for SeatRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SeatRef(<redacted>)")
    }
}

/// A directory of root library locations.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryDirectory {
    libraries: Vec<LibraryPlace>,
}

impl LibraryDirectory {
    pub(crate) fn new(libraries: Vec<LibraryPlace>) -> Self {
        Self { libraries }
    }

    /// Returns the complete list of returned libraries.
    pub fn libraries(&self) -> &[LibraryPlace] {
        &self.libraries
    }
}

impl fmt::Debug for LibraryDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryDirectory")
            .field("library_count", &self.libraries.len())
            .finish()
    }
}

/// A named library from the verified root directory.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryPlace {
    reference: Option<LibraryRef>,
    name: String,
    english_name: Option<String>,
    is_valid: Option<bool>,
    total_seats: Option<u64>,
    available_seats: Option<u64>,
}

impl LibraryPlace {
    pub(crate) fn new(
        reference: Option<LibraryRef>,
        name: String,
        english_name: Option<String>,
        is_valid: Option<bool>,
        total_seats: Option<u64>,
        available_seats: Option<u64>,
    ) -> Self {
        Self {
            reference,
            name,
            english_name,
            is_valid,
            total_seats,
            available_seats,
        }
    }

    /// Returns a selector only when the service marked this location valid.
    pub fn reference(&self) -> Option<&LibraryRef> {
        self.reference.as_ref()
    }

    /// Returns the displayed location name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the English display name when supplied by the service.
    pub fn english_name(&self) -> Option<&str> {
        self.english_name.as_deref()
    }

    /// Returns the source's validity flag when supplied.
    pub fn is_valid(&self) -> Option<bool> {
        self.is_valid
    }

    /// Returns the total seat count when supplied.
    pub fn total_seats(&self) -> Option<u64> {
        self.total_seats
    }

    /// Returns the source-reported currently available seat count.
    pub fn available_seats(&self) -> Option<u64> {
        self.available_seats
    }
}

impl fmt::Debug for LibraryPlace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryPlace")
            .field("reference_present", &self.reference.is_some())
            .field("name", &self.name)
            .field("english_name_present", &self.english_name.is_some())
            .field("is_valid", &self.is_valid)
            .field("total_seats", &self.total_seats)
            .field("available_seats", &self.available_seats)
            .finish()
    }
}

/// The list of floors under one selected library.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryFloors {
    floors: Vec<LibraryFloor>,
}

impl LibraryFloors {
    pub(crate) fn new(floors: Vec<LibraryFloor>) -> Self {
        Self { floors }
    }

    /// Returns the floors returned for the selected library.
    pub fn floors(&self) -> &[LibraryFloor] {
        &self.floors
    }
}

impl fmt::Debug for LibraryFloors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryFloors")
            .field("floor_count", &self.floors.len())
            .finish()
    }
}

/// A selectable floor under a selected library.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryFloor {
    reference: Option<FloorRef>,
    name: String,
    is_valid: Option<bool>,
    total_seats: Option<u64>,
    available_seats: Option<u64>,
}

impl LibraryFloor {
    pub(crate) fn new(
        reference: Option<FloorRef>,
        name: String,
        is_valid: Option<bool>,
        total_seats: Option<u64>,
        available_seats: Option<u64>,
    ) -> Self {
        Self {
            reference,
            name,
            is_valid,
            total_seats,
            available_seats,
        }
    }

    /// Returns a selector only when the service marked this floor valid.
    pub fn reference(&self) -> Option<&FloorRef> {
        self.reference.as_ref()
    }

    /// Returns the displayed floor name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the source's validity flag when supplied.
    pub fn is_valid(&self) -> Option<bool> {
        self.is_valid
    }

    /// Returns the source-reported seat count when supplied.
    pub fn total_seats(&self) -> Option<u64> {
        self.total_seats
    }

    /// Returns the source-reported available seat count when supplied.
    pub fn available_seats(&self) -> Option<u64> {
        self.available_seats
    }
}

impl fmt::Debug for LibraryFloor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryFloor")
            .field("reference_present", &self.reference.is_some())
            .field("name", &self.name)
            .field("is_valid", &self.is_valid)
            .field("total_seats", &self.total_seats)
            .field("available_seats", &self.available_seats)
            .finish()
    }
}

/// The list of floor sections selected for a campus date.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySections {
    day: NaiveDate,
    sections: Vec<LibrarySection>,
}

impl LibrarySections {
    pub(crate) fn new(day: NaiveDate, sections: Vec<LibrarySection>) -> Self {
        Self { day, sections }
    }

    /// Returns the campus date used for this section query.
    pub fn day(&self) -> NaiveDate {
        self.day
    }

    /// Returns the sections returned for the selected floor and date.
    pub fn sections(&self) -> &[LibrarySection] {
        &self.sections
    }
}

impl fmt::Debug for LibrarySections {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibrarySections")
            .field("day", &self.day)
            .field("section_count", &self.sections.len())
            .finish()
    }
}

/// A seat section beneath a floor for a specific campus date.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySection {
    reference: Option<SectionRef>,
    name: String,
    is_valid: Option<bool>,
    total_seats: Option<u64>,
    available_seats: Option<u64>,
}

impl LibrarySection {
    pub(crate) fn new(
        reference: Option<SectionRef>,
        name: String,
        is_valid: Option<bool>,
        total_seats: Option<u64>,
        available_seats: Option<u64>,
    ) -> Self {
        Self {
            reference,
            name,
            is_valid,
            total_seats,
            available_seats,
        }
    }

    /// Returns a selector only when the service marked this section valid.
    pub fn reference(&self) -> Option<&SectionRef> {
        self.reference.as_ref()
    }

    /// Returns the displayed section name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the source's validity flag when supplied.
    pub fn is_valid(&self) -> Option<bool> {
        self.is_valid
    }

    /// Returns the source-reported seat count when supplied.
    pub fn total_seats(&self) -> Option<u64> {
        self.total_seats
    }

    /// Returns the source-reported available seat count when supplied.
    pub fn available_seats(&self) -> Option<u64> {
        self.available_seats
    }
}

impl fmt::Debug for LibrarySection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibrarySection")
            .field("reference_present", &self.reference.is_some())
            .field("name", &self.name)
            .field("is_valid", &self.is_valid)
            .field("total_seats", &self.total_seats)
            .field("available_seats", &self.available_seats)
            .finish()
    }
}

/// Opening windows returned for one selected section and campus date.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryTimeWindows {
    section: SectionRef,
    windows: Vec<LibraryTimeWindow>,
}

impl LibraryTimeWindows {
    pub(crate) fn new(section: SectionRef, windows: Vec<LibraryTimeWindow>) -> Self {
        Self { section, windows }
    }

    /// Returns the campus date covered by these windows.
    pub fn day(&self) -> NaiveDate {
        self.section.day
    }

    /// Returns the returned time windows.
    pub fn windows(&self) -> &[LibraryTimeWindow] {
        &self.windows
    }
}

impl fmt::Debug for LibraryTimeWindows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryTimeWindows")
            .field("day", &self.section.day)
            .field("window_count", &self.windows.len())
            .finish()
    }
}

/// One verified opening window that can be used for a live seat query.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryTimeWindow {
    reference: SeatWindowRef,
}

impl LibraryTimeWindow {
    pub(crate) fn new(reference: SeatWindowRef) -> Self {
        Self { reference }
    }

    /// Returns the opaque selector for a seat query.
    pub fn reference(&self) -> &SeatWindowRef {
        &self.reference
    }

    /// Returns the opening time in the campus time zone.
    pub fn starts_at(&self) -> NaiveTime {
        self.reference.start_time
    }

    /// Returns the closing time in the campus time zone.
    pub fn ends_at(&self) -> NaiveTime {
        self.reference.end_time
    }
}

impl fmt::Debug for LibraryTimeWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryTimeWindow")
            .field("starts_at", &self.starts_at())
            .field("ends_at", &self.ends_at())
            .finish()
    }
}

/// Live seat availability returned for one verified time window.
#[derive(Clone, PartialEq, Eq)]
pub struct LibraryAvailability {
    pub(crate) section: SectionRef,
    pub(crate) seats: Vec<LibrarySeat>,
}

impl LibraryAvailability {
    pub(crate) fn new(section: SectionRef, seats: Vec<LibrarySeat>) -> Self {
        Self { section, seats }
    }

    /// Returns the section date used by this live query.
    pub fn day(&self) -> NaiveDate {
        self.section.day
    }

    /// Returns the seats returned by the service.
    pub fn seats(&self) -> &[LibrarySeat] {
        &self.seats
    }
}

impl fmt::Debug for LibraryAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryAvailability")
            .field("day", &self.section.day)
            .field("seat_count", &self.seats.len())
            .finish()
    }
}

/// One seat's display fields and a reference for its socket state.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySeat {
    pub(crate) reference: SeatRef,
    name: String,
    is_available: bool,
}

impl LibrarySeat {
    pub(crate) fn new(reference: SeatRef, name: String, is_available: bool) -> Self {
        Self {
            reference,
            name,
            is_available,
        }
    }

    /// Returns the opaque reference used to associate socket state.
    pub fn reference(&self) -> &SeatRef {
        &self.reference
    }

    /// Returns the displayed seat label.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether the service marked this seat available.
    pub fn is_available(&self) -> bool {
        self.is_available
    }
}

impl fmt::Debug for LibrarySeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibrarySeat")
            .field("reference_present", &true)
            .field("name", &self.name)
            .field("is_available", &self.is_available)
            .finish()
    }
}

/// Socket state for each seat in a verified live availability result.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySocketAvailability {
    statuses: Vec<LibrarySeatSocket>,
}

impl LibrarySocketAvailability {
    pub(crate) fn new(statuses: Vec<LibrarySeatSocket>) -> Self {
        Self { statuses }
    }

    /// Returns one socket state per seat in the original availability result.
    /// Seats absent from the independent socket response carry `Unknown`.
    pub fn statuses(&self) -> &[LibrarySeatSocket] {
        &self.statuses
    }
}

impl fmt::Debug for LibrarySocketAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibrarySocketAvailability")
            .field("seat_count", &self.statuses.len())
            .finish()
    }
}

/// The socket state associated with one seat reference.
#[derive(Clone, PartialEq, Eq)]
pub struct LibrarySeatSocket {
    seat: SeatRef,
    state: LibrarySocketState,
}

impl LibrarySeatSocket {
    pub(crate) fn new(seat: SeatRef, state: LibrarySocketState) -> Self {
        Self { seat, state }
    }

    /// Returns the opaque seat reference associated with this status.
    pub fn seat(&self) -> &SeatRef {
        &self.seat
    }

    /// Returns the status reported by the separate socket service.
    pub fn state(&self) -> LibrarySocketState {
        self.state
    }
}

impl fmt::Debug for LibrarySeatSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibrarySeatSocket")
            .field("seat_reference_present", &true)
            .field("state", &self.state)
            .finish()
    }
}

/// Validates that one source list contains usable, unique ids and labels.
pub(crate) fn validate_areas(areas: &[crate::library_read::LibraryAreaDto]) -> bool {
    fn visit(
        areas: &[crate::library_read::LibraryAreaDto],
        depth: usize,
        seen: &mut HashSet<u64>,
    ) -> bool {
        depth <= 16
            && areas.len() <= 4096
            && areas.iter().all(|area| {
                area.id > 0
                    && safe_label(&area.name)
                    && area.english_name.as_deref().is_none_or(safe_label)
                    && area.name_merge.as_deref().is_none_or(safe_label)
                    && area.english_name_merge.as_deref().is_none_or(safe_label)
                    && area.total_count.is_none_or(|value| value <= 10_000_000)
                    && area.available_count.is_none_or(|value| value <= 10_000_000)
                    && area
                        .unavailable_space
                        .is_none_or(|value| value <= 10_000_000)
                    && area.point_x.is_none_or(f64::is_finite)
                    && area.point_y.is_none_or(f64::is_finite)
                    && seen.insert(area.id)
                    && seen.len() <= 4096
                    && visit(&area.child_areas, depth + 1, seen)
            })
    }

    visit(areas, 0, &mut HashSet::new())
}

pub(crate) fn safe_label(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 512 && !value.chars().any(char::is_control)
}

/// Joins independent socket records to the seat inventory that initiated the
/// query. Raw identifiers remain inside this module and never enter Debug.
pub(crate) fn merge_socket_statuses(
    availability: &LibraryAvailability,
    records: Vec<crate::library_read::LibrarySocketStatusRecordDto>,
) -> Result<LibrarySocketAvailability, Error> {
    let invalid = || Error::new(Service::Library, ErrorCode::InvalidResponse);
    let seat_ids = availability
        .seats
        .iter()
        .map(|seat| seat.reference.id)
        .collect::<HashSet<_>>();
    let mut status_by_seat = HashMap::new();
    for status in records {
        if status.seat_id == 0
            || !seat_ids.contains(&status.seat_id)
            || status_by_seat
                .insert(status.seat_id, status.status)
                .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(LibrarySocketAvailability::new(
        availability
            .seats
            .iter()
            .map(|seat| {
                LibrarySeatSocket::new(
                    seat.reference.clone(),
                    status_by_seat
                        .remove(&seat.reference.id)
                        .unwrap_or(LibrarySocketState::Unknown),
                )
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_read::LibraryAreaDto;

    fn area(id: u64, name: &str, child_areas: Vec<LibraryAreaDto>) -> LibraryAreaDto {
        LibraryAreaDto {
            id,
            name: name.to_owned(),
            name_merge: None,
            english_name: None,
            english_name_merge: None,
            is_valid: Some(true),
            total_count: Some(10),
            unavailable_space: None,
            available_count: None,
            point_x: None,
            point_y: None,
            child_areas,
        }
    }

    #[test]
    fn opaque_library_references_bind_owner_and_generation_without_debug_ids() {
        let owner = uuid::Uuid::new_v4();
        let reference = LibraryRef::new(owner, 7, 351);
        assert!(reference.belongs_to(owner, 7));
        assert!(!reference.belongs_to(owner, 8));
        assert!(!reference.belongs_to(uuid::Uuid::new_v4(), 7));
        assert_eq!(format!("{reference:?}"), "LibraryRef(<redacted>)");
        assert!(!format!("{reference:?}").contains("351"));
    }

    #[test]
    fn library_area_validation_rejects_duplicate_ids_and_unsafe_labels() {
        assert!(validate_areas(&[area(
            351,
            "North",
            vec![area(352, "Floor 1", vec![])]
        )]));
        assert!(!validate_areas(&[area(
            351,
            "North",
            vec![area(351, "duplicate", vec![])]
        )]));
        assert!(!validate_areas(&[area(351, "North\nlogin", vec![])]));
        assert!(!validate_areas(&[area(0, "North", vec![])]));
    }

    #[test]
    fn opaque_hierarchy_references_remain_bound_through_socket_selection() {
        let owner = uuid::Uuid::new_v4();
        let library = LibraryRef::new(owner, 3, 351);
        let floor = FloorRef::new(library, 5, 352);
        let day = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let section = SectionRef::new(floor, 8, 353, day);
        let seat = SeatRef::new(section.clone(), 701);
        let window = SeatWindowRef::new(
            section,
            9001,
            NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
            NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
        );

        assert!(window.section.belongs_to(owner, 3, 5, 8));
        assert!(!window.section.belongs_to(owner, 4, 5, 8));
        assert!(seat.belongs_to(&window.section));
        assert_eq!(format!("{seat:?}"), "SeatRef(<redacted>)");
        assert_eq!(format!("{window:?}"), "SeatWindowRef(<redacted>)");
    }

    #[test]
    fn socket_merge_marks_missing_records_unknown_and_rejects_foreign_ids() {
        let section = SectionRef::new(
            FloorRef::new(LibraryRef::new(uuid::Uuid::new_v4(), 1, 351), 1, 352),
            1,
            353,
            NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(),
        );
        let availability = LibraryAvailability::new(
            section.clone(),
            vec![LibrarySeat::new(
                SeatRef::new(section, 701),
                "A 01".to_owned(),
                true,
            )],
        );
        let merged = merge_socket_statuses(&availability, Vec::new()).unwrap();
        assert_eq!(merged.statuses()[0].state(), LibrarySocketState::Unknown);
        assert_eq!(
            format!("{:?}", merged.statuses()[0].seat()),
            "SeatRef(<redacted>)"
        );

        let error = merge_socket_statuses(
            &availability,
            vec![crate::library_read::LibrarySocketStatusRecordDto {
                seat_id: 999,
                status: LibrarySocketState::Available,
            }],
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidResponse);
    }
}
