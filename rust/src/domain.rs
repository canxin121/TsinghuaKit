use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CourseStatus {
    Planned,
    #[default]
    Enrolled,
    Completed,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CourseMeeting {
    pub weekday: Weekday,
    pub start_period: u8,
    pub end_period: u8,
    pub weeks: Vec<u8>,
    pub start_time: Option<NaiveTime>,
    pub end_time: Option<NaiveTime>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Course {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub instructor: Option<String>,
    pub credits: Option<f32>,
    pub semester: Option<String>,
    pub status: CourseStatus,
    pub meetings: Vec<CourseMeeting>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    #[default]
    Course,
    Exam,
    Event,
    Deadline,
    Reminder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleItem {
    pub id: Uuid,
    pub title: String,
    pub kind: ScheduleKind,
    pub starts_at: DateTime<Utc>,
    pub ends_at: Option<DateTime<Utc>>,
    pub all_day: bool,
    pub location: Option<String>,
    pub course_id: Option<Uuid>,
    pub description: Option<String>,
}

impl ScheduleItem {
    pub fn effective_end(&self) -> DateTime<Utc> {
        self.ends_at.unwrap_or(self.starts_at)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl DateRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Self, DomainError> {
        if start >= end {
            return Err(DomainError::InvalidDateRange);
        }

        Ok(Self { start, end })
    }

    pub fn for_date(date: NaiveDate) -> Result<Self, DomainError> {
        let next_date = date.succ_opt().ok_or(DomainError::DateOverflow)?;
        let start = date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always a valid time")
            .and_utc();
        let end = next_date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is always a valid time")
            .and_utc();

        Self::new(start, end)
    }

    pub fn overlaps(&self, start: DateTime<Utc>, end: Option<DateTime<Utc>>) -> bool {
        let item_end = end.unwrap_or(start);
        if item_end <= start {
            return start >= self.start && start < self.end;
        }

        start < self.end && item_end > self.start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Archived,
}

impl TodoStatus {
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Archived)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

impl TodoPriority {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Normal => 1,
            Self::High => 2,
            Self::Urgent => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: TodoStatus,
    pub priority: TodoPriority,
    pub due_at: Option<DateTime<Utc>>,
    pub course_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewTodo {
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub priority: TodoPriority,
    pub due_at: Option<DateTime<Utc>>,
    pub course_id: Option<Uuid>,
}

impl NewTodo {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.title.trim().is_empty() {
            return Err(DomainError::EmptyTitle);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TodoFilter {
    pub include_completed: bool,
    pub course_id: Option<Uuid>,
    pub due_before: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampusOverview {
    pub date: NaiveDate,
    pub generated_at: DateTime<Utc>,
    pub semester: Option<String>,
    pub course_count: u32,
    pub pending_todo_count: u32,
    pub completed_todo_count: u32,
    pub today_schedule: Vec<ScheduleItem>,
    pub upcoming_todos: Vec<TodoItem>,
    pub next_schedule: Option<ScheduleItem>,
}
