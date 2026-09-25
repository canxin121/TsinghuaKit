//! Typed academic-term and school-calendar values for the public SDK.

use std::fmt;

use chrono::NaiveDate;

use crate::error::{Error, ErrorCode, Service};

/// A verified term in the current/following Learn calendar.
#[derive(Clone, PartialEq, Eq)]
pub struct AcademicTerm {
    label: String,
    starts_on: NaiveDate,
    ends_on: NaiveDate,
    teaching_week_one: NaiveDate,
    week_count: u32,
}

impl AcademicTerm {
    pub(crate) fn new(
        label: String,
        starts_on: NaiveDate,
        ends_on: NaiveDate,
        teaching_week_one: NaiveDate,
        week_count: u32,
    ) -> Self {
        Self {
            label,
            starts_on,
            ends_on,
            teaching_week_one,
            week_count,
        }
    }

    /// Returns the term label supplied by Learn.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the first calendar date of the term.
    pub fn starts_on(&self) -> NaiveDate {
        self.starts_on
    }

    /// Returns the last calendar date of the term.
    pub fn ends_on(&self) -> NaiveDate {
        self.ends_on
    }

    /// Returns the Monday used as teaching week one by Learn's calendar.
    pub fn teaching_week_one(&self) -> NaiveDate {
        self.teaching_week_one
    }

    /// Returns the number of teaching weeks reported by Learn.
    pub fn week_count(&self) -> u32 {
        self.week_count
    }
}

impl fmt::Debug for AcademicTerm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcademicTerm")
            .field("label_present", &!self.label.is_empty())
            .field("starts_on", &self.starts_on)
            .field("ends_on", &self.ends_on)
            .field("teaching_week_one", &self.teaching_week_one)
            .field("week_count", &self.week_count)
            .finish()
    }
}

/// Learn's verified current term and bounded list of following terms.
#[derive(Clone, PartialEq, Eq)]
pub struct LearnTermCalendar {
    current: AcademicTerm,
    upcoming: Vec<AcademicTerm>,
}

impl LearnTermCalendar {
    pub(crate) fn new(current: AcademicTerm, upcoming: Vec<AcademicTerm>) -> Self {
        Self { current, upcoming }
    }

    /// Returns the current term.
    pub fn current(&self) -> &AcademicTerm {
        &self.current
    }

    /// Returns all following terms included in the source response.
    pub fn upcoming(&self) -> &[AcademicTerm] {
        &self.upcoming
    }
}

impl fmt::Debug for LearnTermCalendar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnTermCalendar")
            .field("current", &self.current)
            .field("upcoming_count", &self.upcoming.len())
            .finish()
    }
}

/// The term season requested from the published school-calendar service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SchoolCalendarSemester {
    /// Autumn semester, numbered as the first semester by the source.
    Autumn,
    /// Spring semester, numbered as the second semester by the source.
    Spring,
}

impl SchoolCalendarSemester {
    /// Returns the stable SDK spelling for this semester selection.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Autumn => "autumn",
            Self::Spring => "spring",
        }
    }
}

/// The display language requested for a school-calendar image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SchoolCalendarLanguage {
    /// Simplified Chinese calendar.
    Chinese,
    /// English calendar.
    English,
}

impl SchoolCalendarLanguage {
    /// Returns the stable SDK spelling for this language selection.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chinese => "zh",
            Self::English => "en",
        }
    }
}

/// A validated request for a published school-calendar image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchoolCalendarQuery {
    year: Option<u32>,
    semester: SchoolCalendarSemester,
    language: SchoolCalendarLanguage,
}

impl SchoolCalendarQuery {
    /// Requests the most recent published year for the given semester and
    /// language. The server, rather than the local clock, selects that year.
    pub fn latest(semester: SchoolCalendarSemester, language: SchoolCalendarLanguage) -> Self {
        Self {
            year: None,
            semester,
            language,
        }
    }

    /// Requests one year already within the school-calendar service's
    /// supported year range.
    pub fn for_year(
        year: u32,
        semester: SchoolCalendarSemester,
        language: SchoolCalendarLanguage,
    ) -> Result<Self, Error> {
        if !(2000..=2100).contains(&year) {
            return Err(Error::new(Service::Calendar, ErrorCode::InvalidInput));
        }
        Ok(Self {
            year: Some(year),
            semester,
            language,
        })
    }

    /// Returns the requested year, or `None` when the server should choose
    /// its latest published year.
    pub fn year(self) -> Option<u32> {
        self.year
    }

    /// Returns the selected semester.
    pub fn semester(self) -> SchoolCalendarSemester {
        self.semester
    }

    /// Returns the selected language.
    pub fn language(self) -> SchoolCalendarLanguage {
        self.language
    }
}

/// A verified school-calendar JPEG fetched by the Rust transport.
#[derive(Clone, PartialEq, Eq)]
pub struct SchoolCalendarImage {
    latest_year: u32,
    year: u32,
    semester: SchoolCalendarSemester,
    language: SchoolCalendarLanguage,
    bytes: Vec<u8>,
}

impl SchoolCalendarImage {
    pub(crate) fn new(
        latest_year: u32,
        year: u32,
        semester: SchoolCalendarSemester,
        language: SchoolCalendarLanguage,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            latest_year,
            year,
            semester,
            language,
            bytes,
        }
    }

    /// Returns the newest year advertised by the source.
    pub fn latest_year(&self) -> u32 {
        self.latest_year
    }

    /// Returns the year represented by this image.
    pub fn year(&self) -> u32 {
        self.year
    }

    /// Returns the semester represented by this image.
    pub fn semester(&self) -> SchoolCalendarSemester {
        self.semester
    }

    /// Returns the language represented by this image.
    pub fn language(&self) -> SchoolCalendarLanguage {
        self.language
    }

    /// Returns the verified JPEG bytes. No public network URL is exposed.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for SchoolCalendarImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchoolCalendarImage")
            .field("latest_year", &self.latest_year)
            .field("year", &self.year)
            .field("semester", &self.semester)
            .field("language", &self.language)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}
