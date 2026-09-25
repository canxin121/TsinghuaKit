//! Public domain types for read-only online service-hall workflows.
//!
//! These are separate from network-learning homework. Task references are
//! opaque, short-lived selections produced by a verified read; protocol IDs
//! and service URLs remain inside Rust.

use std::fmt;

use crate::read::ReadCoverage;

/// Cache behavior supported by read-only online service-hall queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServiceHallReadPolicy {
    /// Return an eligible in-memory result or a `cache_miss` error.
    CacheOnly,
    /// Reuse the fresh in-memory snapshot; otherwise read the service.
    PreferFreshCache,
    /// Bypass the in-memory snapshot and read the service now.
    Refresh,
}

/// Cache behavior supported by the pending-task read.
///
/// Kept as a source-compatible name while all service-hall queries converge
/// on [`ServiceHallReadPolicy`].
pub type PendingReadPolicy = ServiceHallReadPolicy;

/// One read-only task view supported by the online service hall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TaskView {
    /// Applications whose workflow is complete.
    Completed,
    /// Applications saved as drafts.
    Drafts,
    /// Read and unread items copied to the current user.
    Copies,
    /// Aggregated multi-step workflows.
    Phases,
}

impl TaskView {
    pub(crate) const fn runtime_key(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Drafts => "drafts",
            Self::Copies => "unread",
            Self::Phases => "phases",
        }
    }
}

/// A reference to a phase workflow selected from this Client's verified list.
///
/// The selector is opaque and scoped to the Client and phase-list generation
/// that produced it. It is not an authorization token and cannot be forged by
/// callers.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WorkflowTaskRef {
    owner: uuid::Uuid,
    generation: u64,
    view: TaskView,
    selector: String,
}

impl WorkflowTaskRef {
    pub(crate) fn new(
        owner: uuid::Uuid,
        generation: u64,
        view: TaskView,
        selector: String,
    ) -> Self {
        Self {
            owner,
            generation,
            view,
            selector,
        }
    }

    pub(crate) fn belongs_to(&self, owner: uuid::Uuid, generation: u64) -> bool {
        self.owner == owner && self.generation == generation && self.view == TaskView::Phases
    }

    pub(crate) fn selector(&self) -> &str {
        &self.selector
    }
}

impl fmt::Debug for WorkflowTaskRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkflowTaskRef(<redacted>)")
    }
}

/// One read-only entry from a completed, draft, copy, or phased task list.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkflowTask {
    phase_reference: Option<WorkflowTaskRef>,
    title: String,
    status: String,
    step: String,
    application_time: String,
    progress_percent: Option<u32>,
}

impl WorkflowTask {
    pub(crate) fn new(
        phase_reference: Option<WorkflowTaskRef>,
        title: String,
        status: String,
        step: String,
        application_time: String,
        progress_percent: Option<u32>,
    ) -> Self {
        Self {
            phase_reference,
            title,
            status,
            step,
            application_time,
            progress_percent,
        }
    }

    /// Returns an opaque detail reference when this item came from a complete
    /// phased-workflow list. Other views and partial lists are display-only.
    pub fn phase_reference(&self) -> Option<&WorkflowTaskRef> {
        self.phase_reference.as_ref()
    }

    /// Returns the displayed workflow title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the source's displayed workflow status.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Returns the current displayed step, when supplied by the source.
    pub fn step(&self) -> &str {
        &self.step
    }

    /// Returns the displayed application, modification, or completion time.
    pub fn application_time(&self) -> &str {
        &self.application_time
    }

    /// Returns the verified progress percentage, when supplied by the source.
    pub fn progress_percent(&self) -> Option<u32> {
        self.progress_percent
    }
}

impl fmt::Debug for WorkflowTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkflowTask")
            .field("phase_reference_present", &self.phase_reference.is_some())
            .field("content_present", &!self.title.is_empty())
            .field("status_present", &!self.status.is_empty())
            .field("step_present", &!self.step.is_empty())
            .field(
                "application_time_present",
                &!self.application_time.is_empty(),
            )
            .field("progress_percent", &self.progress_percent)
            .finish()
    }
}

/// A read-only task view and its source-reported count.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkflowTaskList {
    view: TaskView,
    items: Vec<WorkflowTask>,
    reported_total: u32,
    coverage: ReadCoverage,
}

impl WorkflowTaskList {
    pub(crate) fn new(
        view: TaskView,
        items: Vec<WorkflowTask>,
        reported_total: u32,
        coverage: ReadCoverage,
    ) -> Self {
        Self {
            view,
            items,
            reported_total,
            coverage,
        }
    }

    /// Returns which read-only list this result represents.
    pub fn view(&self) -> TaskView {
        self.view
    }

    /// Returns the entries obtained from this query.
    pub fn items(&self) -> &[WorkflowTask] {
        &self.items
    }

    /// Returns the total reported by the upstream service.
    pub fn reported_total(&self) -> u32 {
        self.reported_total
    }

    /// Returns whether every documented page was read and reconciled.
    pub fn coverage(&self) -> ReadCoverage {
        self.coverage
    }
}

impl fmt::Debug for WorkflowTaskList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkflowTaskList")
            .field("view", &self.view)
            .field("item_count", &self.items.len())
            .field("reported_total", &self.reported_total)
            .field("coverage", &self.coverage)
            .finish()
    }
}

/// A read-only service shown in the online service-hall catalogue.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    name: String,
    department: String,
    kind: Option<String>,
    in_open_period: Option<bool>,
}

impl fmt::Debug for ServiceEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceEntry")
            .field("name_present", &!self.name.is_empty())
            .field("department_present", &!self.department.is_empty())
            .field("kind_present", &self.kind.is_some())
            .field("in_open_period", &self.in_open_period)
            .finish()
    }
}

impl ServiceEntry {
    pub(crate) fn new(
        name: String,
        department: String,
        kind: Option<String>,
        in_open_period: Option<bool>,
    ) -> Self {
        Self {
            name,
            department,
            kind,
            in_open_period,
        }
    }

    /// Returns the displayed service name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the owning department when supplied by the source.
    pub fn department(&self) -> &str {
        &self.department
    }

    /// Returns the service category when supplied by the source.
    pub fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    /// Returns whether the service is currently within its published period.
    pub fn in_open_period(&self) -> Option<bool> {
        self.in_open_period
    }
}

/// A read-only online service-hall catalogue.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceDirectory {
    items: Vec<ServiceEntry>,
    reported_total: u32,
    coverage: ReadCoverage,
}

impl fmt::Debug for ServiceDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceDirectory")
            .field("item_count", &self.items.len())
            .field("reported_total", &self.reported_total)
            .field("coverage", &self.coverage)
            .finish()
    }
}

impl ServiceDirectory {
    pub(crate) fn new(
        items: Vec<ServiceEntry>,
        reported_total: u32,
        coverage: ReadCoverage,
    ) -> Self {
        Self {
            items,
            reported_total,
            coverage,
        }
    }

    /// Returns the entries obtained from the source.
    pub fn items(&self) -> &[ServiceEntry] {
        &self.items
    }

    /// Returns the total reported by the upstream service.
    pub fn reported_total(&self) -> u32 {
        self.reported_total
    }

    /// Returns whether every documented page was read and reconciled.
    pub fn coverage(&self) -> ReadCoverage {
        self.coverage
    }
}

/// A verified stage in a multi-step service-hall workflow.
#[derive(Clone, PartialEq, Eq)]
pub struct PhaseStep {
    order: String,
    name: String,
    state: String,
    items: Vec<PhaseStepItem>,
}

impl fmt::Debug for PhaseStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhaseStep")
            .field("order_present", &!self.order.is_empty())
            .field("name_present", &!self.name.is_empty())
            .field("state_present", &!self.state.is_empty())
            .field("item_count", &self.items.len())
            .finish()
    }
}

impl PhaseStep {
    pub(crate) fn new(
        order: String,
        name: String,
        state: String,
        items: Vec<PhaseStepItem>,
    ) -> Self {
        Self {
            order,
            name,
            state,
            items,
        }
    }

    /// Returns the source's displayed step order.
    pub fn order(&self) -> &str {
        &self.order
    }

    /// Returns the step name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the displayed step state.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Returns the display-only service items attached to this step.
    pub fn items(&self) -> &[PhaseStepItem] {
        &self.items
    }
}

/// A display-only service item within a workflow phase.
#[derive(Clone, PartialEq, Eq)]
pub struct PhaseStepItem {
    name: String,
    state: String,
}

impl fmt::Debug for PhaseStepItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhaseStepItem")
            .field("name_present", &!self.name.is_empty())
            .field("state_present", &!self.state.is_empty())
            .finish()
    }
}

impl PhaseStepItem {
    pub(crate) fn new(name: String, state: String) -> Self {
        Self { name, state }
    }

    /// Returns the displayed service item name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the displayed service item state.
    pub fn state(&self) -> &str {
        &self.state
    }
}

/// Detailed, read-only steps for a workflow selected from a recent phases list.
#[derive(Clone, PartialEq, Eq)]
pub struct PhaseDetails {
    reference: WorkflowTaskRef,
    steps: Vec<PhaseStep>,
}

impl PhaseDetails {
    pub(crate) fn new(reference: WorkflowTaskRef, steps: Vec<PhaseStep>) -> Self {
        Self { reference, steps }
    }

    /// Returns the phase-list reference that selected this detail result.
    pub fn reference(&self) -> &WorkflowTaskRef {
        &self.reference
    }

    /// Returns the verified stage list.
    pub fn steps(&self) -> &[PhaseStep] {
        &self.steps
    }
}

impl fmt::Debug for PhaseDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhaseDetails")
            .field("reference", &self.reference)
            .field("step_count", &self.steps.len())
            .finish()
    }
}

/// A runtime-bound reference to an item returned by a verified service-hall
/// read. Its protocol identifier is never exposed by this type.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TaskReference {
    handle: uuid::Uuid,
    protocol_id: String,
}

impl TaskReference {
    fn construct(protocol_id: String) -> Self {
        Self {
            handle: uuid::Uuid::new_v4(),
            protocol_id,
        }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn from_verified(protocol_id: String) -> Self {
        Self::construct(protocol_id)
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn from_verified(protocol_id: String) -> Self {
        Self::construct(protocol_id)
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn protocol_id(&self) -> &str {
        &self.protocol_id
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn protocol_id(&self) -> &str {
        &self.protocol_id
    }
}

impl fmt::Debug for TaskReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TaskReference(<redacted>)")
    }
}

/// One read-only task shown in the online service hall.
#[derive(Clone, PartialEq, Eq)]
pub struct Task {
    reference: TaskReference,
    title: String,
    status: String,
    step: String,
    /// The service's displayed application/start time. This is not a deadline.
    application_time: String,
    progress_percent: Option<u32>,
}

impl Task {
    fn construct(
        protocol_id: String,
        title: String,
        status: String,
        step: String,
        application_time: String,
        progress_percent: Option<u32>,
    ) -> Self {
        Self {
            reference: TaskReference::from_verified(protocol_id),
            title,
            status,
            step,
            application_time,
            progress_percent,
        }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn from_verified(
        protocol_id: String,
        title: String,
        status: String,
        step: String,
        application_time: String,
        progress_percent: Option<u32>,
    ) -> Self {
        Self::construct(
            protocol_id,
            title,
            status,
            step,
            application_time,
            progress_percent,
        )
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn from_verified(
        protocol_id: String,
        title: String,
        status: String,
        step: String,
        application_time: String,
        progress_percent: Option<u32>,
    ) -> Self {
        Self::construct(
            protocol_id,
            title,
            status,
            step,
            application_time,
            progress_percent,
        )
    }

    /// Returns the short-lived Rust-validated item reference.
    pub fn reference(&self) -> &TaskReference {
        &self.reference
    }

    /// Returns the displayed task title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the displayed workflow status.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Returns the displayed current workflow step.
    pub fn step(&self) -> &str {
        &self.step
    }

    /// Returns the source's displayed application/start time, when present.
    pub fn application_time(&self) -> &str {
        &self.application_time
    }

    /// Returns a verified progress percentage, when supplied by the service.
    pub fn progress_percent(&self) -> Option<u32> {
        self.progress_percent
    }
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("reference", &self.reference)
            .field("content_present", &!self.title.is_empty())
            .field("status_present", &!self.status.is_empty())
            .field("step_present", &!self.step.is_empty())
            .field(
                "application_time_present",
                &!self.application_time.is_empty(),
            )
            .field("progress_percent", &self.progress_percent)
            .finish()
    }
}

/// A service-hall pending list with its upstream count and completeness proof.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingTasks {
    items: Vec<Task>,
    /// The homepage's reported todo count. It may exclude returned items that
    /// are separately merged into `items`.
    reported_pending_count: u32,
    coverage: ReadCoverage,
}

impl PendingTasks {
    fn construct(items: Vec<Task>, reported_pending_count: u32, coverage: ReadCoverage) -> Self {
        Self {
            items,
            reported_pending_count,
            coverage,
        }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn from_verified(
        items: Vec<Task>,
        reported_pending_count: u32,
        coverage: ReadCoverage,
    ) -> Self {
        Self::construct(items, reported_pending_count, coverage)
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn from_verified(
        items: Vec<Task>,
        reported_pending_count: u32,
        coverage: ReadCoverage,
    ) -> Self {
        Self::construct(items, reported_pending_count, coverage)
    }

    /// Returns the tasks from the proven query range.
    pub fn items(&self) -> &[Task] {
        &self.items
    }

    /// Returns the homepage count as reported by the upstream service.
    pub fn reported_pending_count(&self) -> u32 {
        self.reported_pending_count
    }

    /// Returns whether the complete query range was proven.
    pub fn coverage(&self) -> ReadCoverage {
        self.coverage
    }

    /// Returns whether every documented page was read and reconciled.
    pub fn is_complete(&self) -> bool {
        self.coverage == ReadCoverage::Complete
    }
}

impl fmt::Debug for PendingTasks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingTasks")
            .field("item_count", &self.items.len())
            .field("reported_pending_count", &self.reported_pending_count)
            .field("coverage", &self.coverage)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::IncompleteReason;

    #[test]
    fn backend_refactor_reported_count_and_merged_items_are_independent() {
        let task = Task::from_verified(
            "private-protocol-id".into(),
            "Sensitive task title".into(),
            "退回".into(),
            "补充材料".into(),
            "2026-09-24".into(),
            Some(50),
        );
        let pending = PendingTasks::from_verified(vec![task], 0, ReadCoverage::Complete);

        assert_eq!(pending.reported_pending_count(), 0);
        assert_eq!(pending.items().len(), 1);
        assert!(pending.is_complete());
        let debug = format!("{pending:?} {:?}", pending.items()[0]);
        assert!(!debug.contains("private-protocol-id"));
        assert!(!debug.contains("Sensitive task title"));

        let partial = PendingTasks::from_verified(
            Vec::new(),
            2,
            ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed),
        );
        assert!(!partial.is_complete());
    }

    #[test]
    fn backend_refactor_workflow_references_are_scoped_and_redacted() {
        let owner = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let reference = WorkflowTaskRef::new(
            owner,
            7,
            TaskView::Phases,
            "private-aggregate-selector".into(),
        );

        assert!(reference.belongs_to(owner, 7));
        assert!(!reference.belongs_to(other, 7));
        assert!(!reference.belongs_to(owner, 8));
        assert!(
            !WorkflowTaskRef::new(owner, 7, TaskView::Completed, "x".into()).belongs_to(owner, 7)
        );
        assert!(!format!("{reference:?}").contains("private-aggregate-selector"));
    }

    #[test]
    fn backend_refactor_partial_phases_have_no_detail_reference() {
        let task = WorkflowTask::new(
            None,
            "合成事项".into(),
            "正在办理".into(),
            String::new(),
            String::new(),
            Some(40),
        );
        let debug = format!("{task:?}");

        assert!(task.phase_reference().is_none());
        assert!(!debug.contains("合成事项"));
    }

    #[test]
    fn backend_refactor_service_hall_and_phase_debug_omit_display_text() {
        let directory = ServiceDirectory::new(
            vec![ServiceEntry::new(
                "private service title".into(),
                "private department".into(),
                Some("private category".into()),
                Some(true),
            )],
            1,
            ReadCoverage::Complete,
        );
        let item = PhaseStepItem::new("private phase service".into(), "private state".into());
        let step = PhaseStep::new(
            "1".into(),
            "private phase title".into(),
            "private phase state".into(),
            vec![item],
        );
        let details = PhaseDetails::new(
            WorkflowTaskRef::new(
                uuid::Uuid::new_v4(),
                0,
                TaskView::Phases,
                "private-phase-selector".into(),
            ),
            vec![step.clone()],
        );
        let rendered = format!("{directory:?} {step:?} {:?} {details:?}", step.items()[0]);

        for private_value in [
            "private service title",
            "private department",
            "private category",
            "private phase service",
            "private state",
            "private phase title",
            "private phase state",
            "private-phase-selector",
        ] {
            assert!(!rendered.contains(private_value));
        }
    }
}
