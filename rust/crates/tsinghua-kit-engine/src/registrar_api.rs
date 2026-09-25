//! Curated, source-aware Registrar results for the public Rust SDK.

use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};

/// The academic stage established by this client's authenticated service proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AcademicStage {
    /// Undergraduate Registrar services.
    Undergraduate,
    /// Graduate Registrar services.
    Graduate,
}

/// The kind of grade report returned for an undergraduate account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GradeReportKind {
    /// First-degree curriculum.
    FirstDegree,
    /// Second-degree curriculum.
    SecondDegree,
    /// Minor curriculum.
    Minor,
}

/// A course-grade value in a Registrar report.
#[derive(Clone, PartialEq)]
pub struct CourseGrade {
    course_name: String,
    credit: f64,
    grade: String,
    grade_point: Option<f64>,
    semester: String,
}

impl CourseGrade {
    pub(crate) fn new(
        course_name: String,
        credit: f64,
        grade: String,
        grade_point: Option<f64>,
        semester: String,
    ) -> Self {
        Self {
            course_name,
            credit,
            grade,
            grade_point,
            semester,
        }
    }

    /// Returns the course name.
    pub fn course_name(&self) -> &str {
        &self.course_name
    }

    /// Returns the credit value.
    pub fn credit(&self) -> f64 {
        self.credit
    }

    /// Returns the grade as displayed by Registrar.
    pub fn grade(&self) -> &str {
        &self.grade
    }

    /// Returns the grade-point value when Registrar supplied one.
    pub fn grade_point(&self) -> Option<f64> {
        self.grade_point
    }

    /// Returns the academic term label.
    pub fn semester(&self) -> &str {
        &self.semester
    }
}

impl fmt::Debug for CourseGrade {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseGrade")
            .field("course_name_present", &!self.course_name.is_empty())
            .field("credit", &self.credit)
            .field("grade_present", &!self.grade.is_empty())
            .field("grade_point_present", &self.grade_point.is_some())
            .field("semester_present", &!self.semester.is_empty())
            .finish()
    }
}

/// A grade report for the stage and curriculum proven by this Client.
#[derive(Clone, PartialEq)]
pub struct GradeReport {
    stage: AcademicStage,
    kind: Option<GradeReportKind>,
    courses: Vec<CourseGrade>,
}

impl GradeReport {
    pub(crate) fn new(
        stage: AcademicStage,
        kind: Option<GradeReportKind>,
        courses: Vec<CourseGrade>,
    ) -> Self {
        Self {
            stage,
            kind,
            courses,
        }
    }

    /// Returns the authenticated academic stage.
    pub fn stage(&self) -> AcademicStage {
        self.stage
    }

    /// Returns the selected undergraduate report kind when applicable.
    pub fn kind(&self) -> Option<GradeReportKind> {
        self.kind
    }

    /// Returns the course rows in the report.
    pub fn courses(&self) -> &[CourseGrade] {
        &self.courses
    }
}

impl fmt::Debug for GradeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GradeReport")
            .field("stage", &self.stage)
            .field("kind", &self.kind)
            .field("course_count", &self.courses.len())
            .finish()
    }
}

/// A stable schedule-event category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScheduleEventKind {
    /// A recurring or single course meeting.
    Course,
    /// An examination.
    Exam,
    /// A school or personal event.
    Event,
    /// A deadline.
    Deadline,
    /// A reminder.
    Reminder,
}

/// One event in a complete semester schedule.
#[derive(Clone, PartialEq, Eq)]
pub struct ScheduleEvent {
    title: String,
    kind: ScheduleEventKind,
    starts_at: DateTime<Utc>,
    ends_at: Option<DateTime<Utc>>,
    all_day: bool,
    location: Option<String>,
    description: Option<String>,
}

impl ScheduleEvent {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        title: String,
        kind: ScheduleEventKind,
        starts_at: DateTime<Utc>,
        ends_at: Option<DateTime<Utc>>,
        all_day: bool,
        location: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            title,
            kind,
            starts_at,
            ends_at,
            all_day,
            location,
            description,
        }
    }

    /// Returns the event's display title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the normalized event category.
    pub fn kind(&self) -> ScheduleEventKind {
        self.kind
    }

    /// Returns the timestamp supplied by the verified schedule result.
    pub fn starts_at(&self) -> DateTime<Utc> {
        self.starts_at
    }

    /// Returns the end timestamp when supplied.
    pub fn ends_at(&self) -> Option<DateTime<Utc>> {
        self.ends_at
    }

    /// Returns whether the event spans a whole day.
    pub fn is_all_day(&self) -> bool {
        self.all_day
    }

    /// Returns the location when supplied.
    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    /// Returns the description when supplied.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl fmt::Debug for ScheduleEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScheduleEvent")
            .field("kind", &self.kind)
            .field("title_present", &!self.title.is_empty())
            .field("starts_at", &self.starts_at)
            .field("ends_at_present", &self.ends_at.is_some())
            .field("all_day", &self.all_day)
            .field("location_present", &self.location.is_some())
            .field("description_present", &self.description.is_some())
            .finish()
    }
}

/// The complete schedule for one Registrar semester observation.
#[derive(Clone, PartialEq, Eq)]
pub struct SemesterSchedule {
    semester: String,
    stage: AcademicStage,
    first_day: NaiveDate,
    last_day: NaiveDate,
    week_count: u32,
    current_week: u32,
    events: Vec<ScheduleEvent>,
}

impl SemesterSchedule {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        semester: String,
        stage: AcademicStage,
        first_day: NaiveDate,
        last_day: NaiveDate,
        week_count: u32,
        current_week: u32,
        events: Vec<ScheduleEvent>,
    ) -> Self {
        Self {
            semester,
            stage,
            first_day,
            last_day,
            week_count,
            current_week,
            events,
        }
    }

    /// Returns the term selected by the verified Runtime.
    pub fn semester(&self) -> &str {
        &self.semester
    }

    /// Returns the authenticated academic stage.
    pub fn stage(&self) -> AcademicStage {
        self.stage
    }

    /// Returns the first date covered by the schedule.
    pub fn first_day(&self) -> NaiveDate {
        self.first_day
    }

    /// Returns the last date covered by the schedule.
    pub fn last_day(&self) -> NaiveDate {
        self.last_day
    }

    /// Returns the number of teaching weeks in the schedule.
    pub fn week_count(&self) -> u32 {
        self.week_count
    }

    /// Returns the Runtime's current week estimate.
    pub fn current_week(&self) -> u32 {
        self.current_week
    }

    /// Returns every event in this complete semester result.
    pub fn events(&self) -> &[ScheduleEvent] {
        &self.events
    }
}

impl fmt::Debug for SemesterSchedule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemesterSchedule")
            .field("semester_present", &!self.semester.is_empty())
            .field("stage", &self.stage)
            .field("first_day", &self.first_day)
            .field("last_day", &self.last_day)
            .field("week_count", &self.week_count)
            .field("current_week", &self.current_week)
            .field("event_count", &self.events.len())
            .finish()
    }
}

/// A validated weekday label from an examination record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExamWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

/// One examination row from a verified stage-specific report.
///
/// The undergraduate source may report only a month and day. In that case no
/// calendar year is inferred; `schedule_label` preserves the source's own
/// schedule text. Graduate calendar entries retain their displayed date and
/// time range in the same field.
#[derive(Clone, PartialEq, Eq)]
pub struct Exam {
    course_code: String,
    course_sequence: String,
    course_name: String,
    month: u8,
    day: u8,
    weekday: ExamWeekday,
    schedule_label: String,
    location: String,
    department: Option<String>,
    category: Option<String>,
    instructor: Option<String>,
    headcount: Option<u32>,
}

impl Exam {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        course_code: String,
        course_sequence: String,
        course_name: String,
        month: u8,
        day: u8,
        weekday: ExamWeekday,
        schedule_label: String,
        location: String,
        department: Option<String>,
        category: Option<String>,
        instructor: Option<String>,
        headcount: Option<u32>,
    ) -> Self {
        Self {
            course_code,
            course_sequence,
            course_name,
            month,
            day,
            weekday,
            schedule_label,
            location,
            department,
            category,
            instructor,
            headcount,
        }
    }

    /// Returns the course code when Registrar supplied one.
    pub fn course_code(&self) -> &str {
        &self.course_code
    }

    /// Returns the course sequence when Registrar supplied one.
    pub fn course_sequence(&self) -> &str {
        &self.course_sequence
    }

    /// Returns the course name.
    pub fn course_name(&self) -> &str {
        &self.course_name
    }

    /// Returns the reported exam month.
    pub fn month(&self) -> u8 {
        self.month
    }

    /// Returns the reported exam day of month.
    pub fn day(&self) -> u8 {
        self.day
    }

    /// Returns the verified weekday.
    pub fn weekday(&self) -> ExamWeekday {
        self.weekday
    }

    /// Returns the source's schedule or time-range label.
    pub fn schedule_label(&self) -> &str {
        &self.schedule_label
    }

    /// Returns the location when Registrar supplied one.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Returns the department label when supplied.
    pub fn department(&self) -> Option<&str> {
        self.department.as_deref()
    }

    /// Returns the exam category when supplied.
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }

    /// Returns the instructor when supplied.
    pub fn instructor(&self) -> Option<&str> {
        self.instructor.as_deref()
    }

    /// Returns the reported headcount when supplied.
    pub fn headcount(&self) -> Option<u32> {
        self.headcount
    }
}

impl fmt::Debug for Exam {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Exam")
            .field("course_code_present", &!self.course_code.is_empty())
            .field("course_name_present", &!self.course_name.is_empty())
            .field("month", &self.month)
            .field("day", &self.day)
            .field("weekday", &self.weekday)
            .field("schedule_present", &!self.schedule_label.is_empty())
            .field("location_present", &!self.location.is_empty())
            .finish()
    }
}

/// A complete stage-specific examination report.
#[derive(Clone, PartialEq, Eq)]
pub struct ExamReport {
    stage: AcademicStage,
    exams: Vec<Exam>,
}

impl ExamReport {
    pub(crate) fn new(stage: AcademicStage, exams: Vec<Exam>) -> Self {
        Self { stage, exams }
    }

    /// Returns the authenticated stage used to select the source.
    pub fn stage(&self) -> AcademicStage {
        self.stage
    }

    /// Returns the exam rows included in the complete report.
    pub fn exams(&self) -> &[Exam] {
        &self.exams
    }
}

impl fmt::Debug for ExamReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExamReport")
            .field("stage", &self.stage)
            .field("exam_count", &self.exams.len())
            .finish()
    }
}
