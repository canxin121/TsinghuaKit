//! Public domain values for read-only classroom availability.
//!
//! Building selectors stay inside opaque Client-bound references. The public
//! week and time-slot types are validated at the facade boundary.

use std::fmt;

use chrono::{Datelike, NaiveDate, Weekday};

/// A non-zero week number accepted by the classroom service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassroomWeek(u32);

impl ClassroomWeek {
    /// Creates a week number in the supported bounded range.
    pub fn new(value: u32) -> Option<Self> {
        (1..=100).contains(&value).then_some(Self(value))
    }

    /// Returns the validated week number.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Whether to query the server-provided building week or an explicit week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClassroomWeekSelection {
    /// Use the default week provided with the building directory entry.
    BuildingDefault,
    /// Request one explicitly selected week.
    Week(ClassroomWeek),
}

/// Opaque selector for a building in this Client's latest directory.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BuildingRef {
    pub(crate) owner: uuid::Uuid,
    pub(crate) generation: u64,
    pub(crate) index: u32,
    pub(crate) default_week: ClassroomWeek,
}

impl BuildingRef {
    pub(crate) fn new(
        owner: uuid::Uuid,
        generation: u64,
        index: u32,
        default_week: ClassroomWeek,
    ) -> Self {
        Self {
            owner,
            generation,
            index,
            default_week,
        }
    }

    pub(crate) fn belongs_to(&self, owner: uuid::Uuid, generation: u64) -> bool {
        self.owner == owner && self.generation == generation
    }
}

impl fmt::Debug for BuildingRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BuildingRef(<redacted>)")
    }
}

/// A building shown in the classroom directory.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomBuilding {
    reference: Option<BuildingRef>,
    name: String,
    default_week: Option<ClassroomWeek>,
}

impl ClassroomBuilding {
    pub(crate) fn new(
        reference: Option<BuildingRef>,
        name: String,
        default_week: Option<ClassroomWeek>,
    ) -> Self {
        Self {
            reference,
            name,
            default_week,
        }
    }

    /// Returns a selector for a valid building in this directory.
    pub fn reference(&self) -> Option<&BuildingRef> {
        self.reference.as_ref()
    }

    /// Returns the displayed building name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the default week supplied by the service.
    pub fn default_week(&self) -> Option<ClassroomWeek> {
        self.default_week
    }
}

impl fmt::Debug for ClassroomBuilding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassroomBuilding")
            .field("reference_present", &self.reference.is_some())
            .field("name", &self.name)
            .field("default_week", &self.default_week)
            .finish()
    }
}

/// The classroom buildings returned by one directory read.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomBuildings {
    buildings: Vec<ClassroomBuilding>,
}

impl ClassroomBuildings {
    pub(crate) fn new(buildings: Vec<ClassroomBuilding>) -> Self {
        Self { buildings }
    }

    /// Returns the complete building list.
    pub fn buildings(&self) -> &[ClassroomBuilding] {
        &self.buildings
    }
}

impl fmt::Debug for ClassroomBuildings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassroomBuildings")
            .field("building_count", &self.buildings.len())
            .finish()
    }
}

/// Availability classification for one class-period slot.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClassroomSlotStatus {
    /// No class occupies the period.
    Free,
    /// A class occupies the period.
    Occupied,
    /// An exam occupies the period.
    Exam,
    /// The room is borrowed.
    Borrowed,
    /// The room is disabled.
    Disabled,
    /// The service returned an unrecognized occupancy class.
    Unknown { class_name: String },
}

impl fmt::Debug for ClassroomSlotStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Free => f.write_str("Free"),
            Self::Occupied => f.write_str("Occupied"),
            Self::Exam => f.write_str("Exam"),
            Self::Borrowed => f.write_str("Borrowed"),
            Self::Disabled => f.write_str("Disabled"),
            Self::Unknown { .. } => f
                .debug_struct("Unknown")
                .field("class_name_present", &true)
                .finish(),
        }
    }
}

impl ClassroomSlotStatus {
    pub(crate) fn from_runtime(value: &str) -> Option<Self> {
        match value {
            "free" => Some(Self::Free),
            "occupied" => Some(Self::Occupied),
            "exam" => Some(Self::Exam),
            "borrowed" => Some(Self::Borrowed),
            "disabled" => Some(Self::Disabled),
            value if value.starts_with("unknown:") => {
                let class_name = value["unknown:".len()..].trim();
                safe_label(class_name).then(|| Self::Unknown {
                    class_name: class_name.to_owned(),
                })
            }
            _ => None,
        }
    }

    /// Returns the unrecognized source label for an `Unknown` slot.
    pub fn unknown_class_name(&self) -> Option<&str> {
        match self {
            Self::Unknown { class_name } => Some(class_name),
            _ => None,
        }
    }
}

/// One room and its 42 Monday-first weekly slot statuses.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomRoomAvailability {
    name: String,
    slots: Vec<ClassroomSlotStatus>,
}

impl ClassroomRoomAvailability {
    pub(crate) fn new(name: String, slots: Vec<ClassroomSlotStatus>) -> Self {
        Self { name, slots }
    }

    /// Returns the displayed room name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns 42 slot statuses, ordered Monday first.
    pub fn slots(&self) -> &[ClassroomSlotStatus] {
        &self.slots
    }
}

impl fmt::Debug for ClassroomRoomAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassroomRoomAvailability")
            .field("name", &self.name)
            .field("slot_count", &self.slots.len())
            .finish()
    }
}

/// Weekly availability for one verified building and week.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassroomAvailability {
    building: BuildingRef,
    week: ClassroomWeek,
    valid_weeks: Vec<ClassroomWeek>,
    week_dates: [NaiveDate; 7],
    rooms: Vec<ClassroomRoomAvailability>,
}

impl ClassroomAvailability {
    pub(crate) fn new(
        building: BuildingRef,
        week: ClassroomWeek,
        valid_weeks: Vec<ClassroomWeek>,
        week_dates: [NaiveDate; 7],
        rooms: Vec<ClassroomRoomAvailability>,
    ) -> Self {
        Self {
            building,
            week,
            valid_weeks,
            week_dates,
            rooms,
        }
    }

    /// Returns the building reference that produced this matrix.
    pub fn building_reference(&self) -> &BuildingRef {
        &self.building
    }

    /// Returns the validated week requested for the matrix.
    pub fn week(&self) -> ClassroomWeek {
        self.week
    }

    /// Returns the week numbers accepted by the current service page.
    pub fn valid_weeks(&self) -> &[ClassroomWeek] {
        &self.valid_weeks
    }

    /// Returns the seven Monday-first dates represented by the matrix.
    pub fn week_dates(&self) -> &[NaiveDate; 7] {
        &self.week_dates
    }

    /// Returns the classrooms in the selected building.
    pub fn rooms(&self) -> &[ClassroomRoomAvailability] {
        &self.rooms
    }
}

impl fmt::Debug for ClassroomAvailability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassroomAvailability")
            .field("building_reference_present", &true)
            .field("week", &self.week)
            .field("valid_week_count", &self.valid_weeks.len())
            .field("room_count", &self.rooms.len())
            .finish()
    }
}

pub(crate) fn safe_label(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 512 && !value.chars().any(char::is_control)
}

pub(crate) fn dates_are_monday_first(dates: &[NaiveDate; 7]) -> bool {
    dates[0].weekday() == Weekday::Mon
        && dates
            .windows(2)
            .all(|pair| pair[0].succ_opt() == Some(pair[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classroom_selector_debug_hides_indices_and_unknown_class_names() {
        let reference =
            BuildingRef::new(uuid::Uuid::new_v4(), 4, 37, ClassroomWeek::new(3).unwrap());
        assert_eq!(format!("{reference:?}"), "BuildingRef(<redacted>)");
        let status = ClassroomSlotStatus::Unknown {
            class_name: "fixture-secret-class".to_owned(),
        };
        assert!(!format!("{status:?}").contains("fixture-secret-class"));
        assert_eq!(status.unknown_class_name(), Some("fixture-secret-class"));
    }

    #[test]
    fn classroom_week_and_monday_first_date_ranges_are_validated() {
        assert!(ClassroomWeek::new(0).is_none());
        assert!(ClassroomWeek::new(101).is_none());
        let monday = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let dates = std::array::from_fn(|index| monday + chrono::Days::new(index as u64));
        assert!(dates_are_monday_first(&dates));
        let wrong_start = std::array::from_fn(|index| monday + chrono::Days::new(index as u64 + 1));
        assert!(!dates_are_monday_first(&wrong_start));
    }
}
