//! App-equivalent fan-in: concurrent ready callers, one mutable Runtime,
//! priority/FIFO ordering, and the normal Rust-internal source fan-out.
//! Never clones an authenticated runtime or runs multiple credential prompts.
use super::*;
use crate::telemetry::timing::{self, Collector, TimingSnapshot};
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    App,
    Serial,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ExecutionOptions {
    pub mode: ExecutionMode,
    pub concurrency: usize,
}
impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::App,
            concurrency: 4,
        }
    }
}
impl ExecutionOptions {
    pub fn validate(self) -> Result<Self, String> {
        if !(1..=8).contains(&self.concurrency) {
            return Err("invalid_concurrency".into());
        }
        if self.mode == ExecutionMode::Serial && self.concurrency != 1 {
            return Err("serial_requires_concurrency_one".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaseTiming {
    pub ready_after_us: u64,
    pub queued_after_us: u64,
    #[serde(default)]
    pub lock_acquired_after_us: u64,
    pub started_after_us: u64,
    pub finished_after_us: u64,
    pub dependency_wait_us: u64,
    pub scheduler_wait_us: u64,
    pub queue_wait_us: u64,
    pub execution_us: u64,
    pub user_input_us: u64,
    pub active_execution_us: u64,
    pub total_after_ready_us: u64,
    pub measured: bool,
    pub detail: TimingSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub mode: ExecutionMode,
    pub configured_concurrency: usize,
    pub max_submitted: usize,
    pub max_runtime_executing: usize,
    pub runtime_parallelism_limit: usize,
    #[serde(default)]
    pub http_read_parallelism_limit: usize,
    #[serde(default)]
    pub fixed_request_gap_ms: u64,
    pub total_wall_us: u64,
    pub user_input_us: u64,
    pub active_wall_us: u64,
    pub first_completed_after_us: Option<u64>,
    pub first_data_after_us: Option<u64>,
    pub http_scope: String,
    pub phase_totals_are_additive: bool,
    #[serde(default)]
    pub checkpoint_write_count: u64,
    #[serde(default)]
    pub checkpoint_write_us: u64,
}
impl ExecutionSummary {
    pub fn new(options: ExecutionOptions) -> Self {
        Self {
            mode: options.mode,
            configured_concurrency: options.concurrency,
            max_submitted: 0,
            max_runtime_executing: 0,
            runtime_parallelism_limit: 1,
            http_read_parallelism_limit: crate::request_gate::MAX_READ_DISPATCHES as usize,
            fixed_request_gap_ms: crate::request_gate::request_gap_ms(),
            total_wall_us: 0,
            user_input_us: 0,
            active_wall_us: 0,
            first_completed_after_us: None,
            first_data_after_us: None,
            http_scope: "current_case_task_and_inherited_rust_futures".into(),
            phase_totals_are_additive: false,
            checkpoint_write_count: 0,
            checkpoint_write_us: 0,
        }
    }
}

pub fn priority(id: &str) -> u8 {
    match id {
        "identity_session" | "portal_bootstrap" | "learn_session" => 0,
        "learn_courses"
        | "registrar_session"
        | "registrar_schedule"
        | "library_area_tree"
        | "campus_card_account"
        | "electricity_remainder"
        | "info_news"
        | "classroom_buildings" => 1,
        id if id.ends_with("_session") => 2,
        "tunet_status" | "tunet_local_status" => 4,
        _ => 3,
    }
}

/// Do not reserve a whole cohort of handoffs before exposing a ready result.
/// Recompute dependencies immediately after each auth/handoff, so a cheap
/// course/balance read can precede an unrelated service's connection chain.
pub(super) fn ready_batch(
    mut ready: Vec<&CheckSpec>,
    options: ExecutionOptions,
) -> Vec<&CheckSpec> {
    if options.mode == ExecutionMode::App {
        ready.sort_by_key(|spec| priority(spec.id));
        if let Some(first) = ready.first() {
            let best = priority(first.id);
            let handoff = first.id.ends_with("_session") || first.id == "portal_bootstrap";
            ready.retain(|spec| priority(spec.id) == best);
            if handoff {
                ready.truncate(1);
            }
        }
    }
    ready.truncate(options.concurrency);
    ready
}

/// Poll futures in priority/FIFO order, without detached tasks. Dropping this
/// future cancels queued work; its active request remains explicitly unknown.
pub(super) async fn drive(
    jobs: Vec<Pin<Box<dyn Future<Output = Result<(), String>> + '_>>>,
) -> Result<(), String> {
    let mut pending: Vec<_> = jobs.into_iter().map(Some).collect();
    poll_fn(move |cx| {
        let mut finished = true;
        for slot in &mut pending {
            if let Some(job) = slot {
                match job.as_mut().poll(cx) {
                    Poll::Ready(Ok(())) => *slot = None,
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Pending => finished = false,
                }
            }
        }
        if finished {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    })
    .await
}

struct Context<'a, P> {
    runtime: &'a mut CampusRuntime,
    prompt: &'a mut P,
    report: &'a mut ReportWriter,
    evidence: Evidence,
    last_trust: Option<String>,
}

pub(super) async fn execute(
    runtime: &mut CampusRuntime,
    prompt: &mut impl UserPrompts,
    report: &mut ReportWriter,
    include_usereg: bool,
    options: ExecutionOptions,
) -> Result<(), String> {
    let options = options.validate()?;
    if runtime.persist_sessions || runtime.validation_workspace.is_none() {
        return Err("fresh_terminal_runtime_required".into());
    }
    report.report.execution = Some(ExecutionSummary::new(options));
    report.save()?;
    tracing::info!(target:"tsinghua_kit::validation",event="queue_configured",
        configured_concurrency=options.concurrency as u64,runtime_parallelism=1_u64);
    let shared = tokio::sync::Mutex::new(Context {
        runtime,
        prompt,
        report,
        evidence: Evidence::default(),
        last_trust: None,
    });
    loop {
        let mut context = shared.lock().await;
        // Resolve only terminal prerequisites; pending prerequisites must wait,
        // never get mislabeled as failed merely because another task is queued.
        let mut changed = true;
        while changed {
            changed = false;
            for spec in CHECKS {
                if !context
                    .report
                    .report
                    .cases
                    .get(spec.id)
                    .is_some_and(|r| r.status == CheckStatus::Pending)
                {
                    continue;
                }
                let terminal_failure = spec.dependencies.iter().any(|dep| {
                    context.report.report.cases.get(*dep).is_none_or(|r| {
                        !matches!(
                            r.status,
                            CheckStatus::Passed | CheckStatus::Pending | CheckStatus::Running
                        )
                    })
                });
                let skipped = context
                    .report
                    .report
                    .network_environment
                    .skip_reason(spec.id)
                    .or_else(|| {
                        (spec.service == "usereg" && !include_usereg)
                            .then_some("optional_login_not_selected")
                    });
                if skipped.is_some() || terminal_failure {
                    let (status, reason) = if let Some(reason) = skipped {
                        (CheckStatus::Unverified, reason)
                    } else {
                        (CheckStatus::Blocked, "dependency_not_passed")
                    };
                    context.report.end(spec.id, status, reason, None, 0, 0)?;
                    context.prompt.progress(spec, status, reason);
                    changed = true;
                }
            }
        }
        let ready: Vec<_> = CHECKS
            .iter()
            .filter(|spec| {
                context
                    .report
                    .report
                    .cases
                    .get(spec.id)
                    .is_some_and(|r| r.status == CheckStatus::Pending)
                    && spec.dependencies.iter().all(|dep| {
                        context
                            .report
                            .report
                            .cases
                            .get(*dep)
                            .is_some_and(|r| r.status == CheckStatus::Passed)
                    })
            })
            .collect();
        let ready = ready_batch(ready, options);
        if ready.is_empty() {
            if context
                .report
                .report
                .cases
                .values()
                .any(|row| matches!(row.status, CheckStatus::Pending | CheckStatus::Running))
            {
                return Err("validation_dependency_deadlock".into());
            }
            context.prompt.clear_captcha();
            return context.report.finish();
        }
        let now = timing::micros(context.report.clock.elapsed());
        for spec in &ready {
            let dependency = spec
                .dependencies
                .iter()
                .filter_map(|id| {
                    context
                        .report
                        .report
                        .cases
                        .get(*id)
                        .and_then(|r| r.timing.as_ref())
                        .map(|t| t.finished_after_us)
                })
                .max()
                .unwrap_or(0);
            let row = context.report.report.cases.get_mut(spec.id).unwrap();
            row.timing = Some(CaseTiming {
                ready_after_us: dependency,
                dependency_wait_us: dependency,
                queued_after_us: now,
                scheduler_wait_us: now.saturating_sub(dependency),
                ..Default::default()
            });
            tracing::info!(target:"tsinghua_kit::validation",event="case_queued",case=spec.id,
                service=spec.service,dependency_wait_us=dependency,queue_depth=ready.len() as u64);
        }
        if let Some(summary) = &mut context.report.report.execution {
            summary.max_submitted = summary.max_submitted.max(ready.len());
        }
        context.report.save()?;
        drop(context);
        let jobs = ready.into_iter().map(|spec| {
            let shared = &shared;
            Box::pin(async move {
                let mut context = shared.lock().await;
                let Context { runtime, prompt, report, evidence, last_trust } = &mut *context;
                if spec.id != "identity_session" && spec.id != "tunet_status"
                    && runtime.status().state == "requires_second_factor" {
                    report.end(spec.id, CheckStatus::Blocked, "unresolved_factor", None, 0, 0)?;
                    prompt.progress(spec, CheckStatus::Blocked, "unresolved_factor");
                    return Ok(());
                }
                let lock_offset = timing::micros(report.clock.elapsed());
                report.start(spec.id)?;
                let start_offset = timing::micros(report.clock.elapsed());
                let row = report.report.cases.get_mut(spec.id).unwrap();
                let timing = row.timing.as_mut().unwrap();
                timing.started_after_us = start_offset;
                timing.lock_acquired_after_us = lock_offset;
                timing.queue_wait_us = lock_offset.saturating_sub(timing.queued_after_us);
                let collector = Collector::default();
                report.active_metrics = Some(collector.clone());
                if let Some(summary) = &mut report.report.execution { summary.max_runtime_executing = 1; }
                prompt.progress(spec, CheckStatus::Running, "in_progress");
                let start = Instant::now();
                let mut measured_prompt = MeasuredPrompts(&mut **prompt);
                let result = collector.scope(crate::telemetry::observe(spec.service, spec.id,
                    execute_case(runtime, &mut measured_prompt, evidence, spec.id, include_usereg))).await;
                let execution_us = timing::micros(start.elapsed());
                let metrics = collector.snapshot();
                if let Some(status) = runtime.trusted_device_status.as_deref()
                    && last_trust.as_deref() != Some(status) {
                    prompt.device_trust_status(status); *last_trust = Some(status.to_owned());
                }
                let (status, reason, count) = match result {
                    Ok(Outcome::Passed(count)) => (CheckStatus::Passed, "verified", count),
                    Ok(Outcome::ObservedNetwork(reason)) => (CheckStatus::Passed, reason, None),
                    Ok(Outcome::Skipped(reason)) => (CheckStatus::Unverified, reason, None),
                    Err(error) => (CheckStatus::Failed, crate::telemetry::diagnostic_reason(&error), None),
                };
                if let Some(stage) = runtime.resolved_academic_stage() { report.set_graduate(stage == AcademicStage::Graduate)?; }
                let finish_offset = timing::micros(report.clock.elapsed());
                let timing = report.report.cases.get_mut(spec.id).unwrap().timing.as_mut().unwrap();
                timing.finished_after_us = finish_offset;
                timing.execution_us = execution_us;
                timing.user_input_us = metrics.phase_us(timing::Phase::UserInput);
                timing.active_execution_us = execution_us.saturating_sub(timing.user_input_us);
                timing.total_after_ready_us = finish_offset.saturating_sub(timing.ready_after_us);
                timing.measured = true;
                let requests = metrics.requests;
                timing.detail = metrics;
                let queue_wait_us = timing.queue_wait_us;
                report.end(spec.id, status, reason, count, execution_us / 1000, requests)?;
                tracing::info!(target:"tsinghua_kit::validation",event="case_finished",case=spec.id,
                    service=spec.service,outcome=status.key(),reason,execution_us,queue_wait_us,request_count=requests);
                prompt.progress(spec, status, reason);
                if let Some(timing) = report.report.cases.get(spec.id).and_then(|row| row.timing.as_ref()) {
                    prompt.timing(spec, timing);
                }
                Ok(())
            }) as Pin<Box<dyn Future<Output=Result<(), String>> + '_>>
        }).collect();
        drive(jobs).await?;
    }
}
