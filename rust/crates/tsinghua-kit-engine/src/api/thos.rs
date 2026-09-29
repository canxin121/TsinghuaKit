//! Presentation-only results for the online service hall (THOS).
//! Authentication material and server-issued navigation URLs stay in Rust.

use std::fmt;

use crate::{
    read::{ReadResult, ReadSource},
    service_hall::PendingTasks,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosTaskDto {
    pub id: String,
    pub title: String,
    pub status: String,
    pub node: String,
    /// Upstream application/start time, not an assignment deadline.
    pub date: String,
    pub progress: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosPendingDto {
    pub items: Vec<ThosTaskDto>,
    /// AuditSvsNum from the service homepage. Returned applications can add
    /// further items; this is deliberately distinct from items.len().
    pub reported_todo_count: u32,
    pub complete: bool,
    pub generated_at: String,
    pub source: String,
    pub status: String,
    pub error: Option<String>,
}

/// A complete or explicitly partial read of one THOS task view. The `kind`
/// value is a Rust-validated selector; upstream workflow URLs and identifiers
/// never leave the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosTaskListDto {
    pub kind: String,
    pub items: Vec<ThosTaskDto>,
    pub reported_total: u32,
    pub complete: bool,
    pub generated_at: String,
    pub source: String,
    pub status: String,
    pub error: Option<String>,
}

/// Read-only stage details selected by a task ID from the proven phases list.
/// Raw aggregate/work-item IDs and upstream URLs remain inside Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosPhaseStepItemDto {
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosPhaseStepDto {
    pub order: String,
    pub name: String,
    pub state: String,
    pub items: Vec<ThosPhaseStepItemDto>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosPhaseStepsDto {
    pub task_id: String,
    pub steps: Vec<ThosPhaseStepDto>,
    pub generated_at: String,
    pub source: String,
    pub status: String,
}

/// One entry in the online service hall catalogue. Server-issued navigation
/// URLs stay in Rust until a separate, verified opening flow exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosServiceDto {
    pub id: String,
    pub name: String,
    pub department: String,
    pub kind: Option<String>,
    pub in_open_period: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThosServicesDto {
    pub items: Vec<ThosServiceDto>,
    pub reported_total: u32,
    pub complete: bool,
    pub generated_at: String,
    pub source: String,
    pub status: String,
    pub error: Option<String>,
}

/// One course result looked up by course number.
///
/// The course number the caller supplied and the account's own student id are
/// deliberately absent: the first is the caller's own argument, and the second
/// is derived inside Rust and never leaves a request body.
#[derive(Clone, PartialEq)]
pub struct ThosCourseScoreDto {
    /// The course's own name.
    pub name: String,
    /// The credit value the service printed, when it printed one.
    pub credit: Option<f64>,
    /// The grade as the service rendered it, empty when it printed none.
    pub grade: String,
    /// True when the service reported no result at all for this course.
    ///
    /// This is the service's own answer about this account and course, so the
    /// caller can tell it from a response that failed to parse.
    pub empty: bool,
    pub generated_at: String,
    pub source: String,
    pub status: String,
}

/// A grade result is personal academic data, so its `Debug` prints only the
/// presence and shape of each field and never the values themselves.
impl fmt::Debug for ThosCourseScoreDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ThosCourseScoreDto")
            .field("name_present", &!self.name.is_empty())
            .field("credit", &self.credit)
            .field("grade_present", &!self.grade.is_empty())
            .field("empty", &self.empty)
            .field("generated_at", &self.generated_at)
            .field("source", &self.source)
            .field("status", &self.status)
            .finish()
    }
}

pub(crate) fn pending_dto(result: ReadResult<PendingTasks>) -> ThosPendingDto {
    let (pending, metadata) = result.into_parts();
    let complete = pending.is_complete();
    let items = pending
        .items()
        .iter()
        .map(|task| ThosTaskDto {
            id: task.reference().protocol_id().to_owned(),
            title: task.title().to_owned(),
            status: task.status().to_owned(),
            node: task.step().to_owned(),
            date: task.application_time().to_owned(),
            progress: task.progress_percent(),
        })
        .collect();
    let source = match metadata.source() {
        ReadSource::Live => "live",
        ReadSource::MemoryCache => "cache",
        ReadSource::ClientCache => "cache",
        ReadSource::PersistentCache => "cache",
    };
    ThosPendingDto {
        items,
        reported_todo_count: pending.reported_pending_count(),
        complete,
        generated_at: metadata.observed_at().to_rfc3339(),
        source: source.into(),
        status: if complete { "ready" } else { "partial" }.into(),
        error: (!complete).then(|| "待办列表尚未完整读取或查询期间发生变化，请刷新核对".into()),
    }
}
