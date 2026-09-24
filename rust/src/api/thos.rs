//! Presentation-only results for the online service hall (THOS).
//! Authentication material and server-issued navigation URLs stay in Rust.

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
