//! Interactive, fresh-runtime endpoint acceptance. No App cookies are loaded.
//! Raw records are kept only in memory; reports contain fixed keys and counts.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct CheckSpec {
    pub id: &'static str,
    pub service: &'static str,
    pub label: &'static str,
    pub dependencies: &'static [&'static str],
}

#[path = "cli_schedule.rs"]
mod schedule;
#[cfg(test)]
#[path = "cli_validation_tests.rs"]
mod tests;
pub use schedule::{CaseTiming, ExecutionMode, ExecutionOptions, ExecutionSummary};
#[cfg(test)]
#[path = "cli_coverage_tests.rs"]
mod coverage_tests;
#[cfg(test)]
#[path = "cli_latency_tests.rs"]
mod latency_tests;
#[cfg(test)]
#[path = "cli_network_scope_tests.rs"]
mod network_scope_tests;
#[cfg(test)]
#[path = "cli_network_services_tests.rs"]
mod network_services_tests;
#[cfg(test)]
#[path = "cli_personal_tests.rs"]
mod personal_tests;
#[cfg(test)]
#[path = "cli_request_audit_tests.rs"]
mod request_audit_tests;
#[cfg(test)]
#[path = "cli_timing_tests.rs"]
mod timing_tests;
macro_rules! check {($id:literal,$service:literal,$label:literal,[$($dep:literal),*])=>{CheckSpec{id:$id,service:$service,label:$label,dependencies:&[$($dep),*]}};}
pub const CHECKS: &[CheckSpec] = &[
    check!("identity_session", "identity", "统一身份认证", []),
    check!(
        "portal_bootstrap",
        "webvpn",
        "WebVPN 门户票据与业务证明",
        ["identity_session"]
    ),
    check!(
        "learn_session",
        "learn",
        "学堂会话与 CSRF",
        ["portal_bootstrap"]
    ),
    check!(
        "learn_courses",
        "learn",
        "当前学期课程列表",
        ["learn_session"]
    ),
    check!(
        "learn_calendar",
        "learn",
        "网络学堂当前与后续学期日历",
        ["learn_session"]
    ),
    check!(
        "school_calendar",
        "learn",
        "学校校历年份与秋季中文图片",
        ["learn_session"]
    ),
    check!(
        "learn_announcements",
        "learn",
        "首门真实课程公告",
        ["learn_courses"]
    ),
    check!(
        "learn_todos",
        "learn",
        "首门真实课程三个作业列表",
        ["learn_courses"]
    ),
    check!(
        "learn_files",
        "learn",
        "首门真实课程资料元数据",
        ["learn_courses"]
    ),
    check!(
        "learn_file_categories",
        "learn",
        "首门真实课程资料分类",
        ["learn_courses"]
    ),
    check!(
        "learn_file_download",
        "learn",
        "首门真实课程首份资料下载响应（只读丢弃，不保存）",
        ["learn_courses"]
    ),
    check!(
        "learn_discussions",
        "learn",
        "首门真实课程讨论列表",
        ["learn_courses"]
    ),
    check!(
        "learn_homework",
        "learn",
        "首门真实课程作业列表",
        ["learn_courses"]
    ),
    check!(
        "learn_homework_detail",
        "learn",
        "最多十二门真实课程中的首条作业详情",
        ["learn_homework"]
    ),
    check!(
        "registrar_session",
        "registrar",
        "教务票据与课表证明",
        ["learn_session"]
    ),
    check!(
        "registrar_schedule",
        "registrar",
        "当日课表",
        ["registrar_session"]
    ),
    check!(
        "registrar_grades",
        "registrar",
        "成绩读取",
        ["registrar_session"]
    ),
    check!(
        "registrar_exams",
        "registrar",
        "当前学段考试安排",
        ["registrar_session"]
    ),
    check!(
        "info_session",
        "info",
        "INFO 业务会话",
        ["portal_bootstrap"]
    ),
    check!(
        "thos_pending",
        "info",
        "网上服务大厅待我处理（完整分页与退回事项）",
        ["info_session"]
    ),
    check!(
        "thos_services",
        "info",
        "网上服务大厅服务目录（完整分页）",
        ["info_session"]
    ),
    check!(
        "thos_completed",
        "info",
        "网上服务大厅已办事项",
        ["info_session"]
    ),
    check!("thos_drafts", "info", "网上服务大厅草稿", ["info_session"]),
    check!(
        "thos_unread",
        "info",
        "网上服务大厅抄送待阅",
        ["info_session"]
    ),
    check!(
        "thos_phases",
        "info",
        "网上服务大厅阶段性事项",
        ["info_session"]
    ),
    check!(
        "thos_phase_steps",
        "info",
        "首个真实阶段性事项步骤",
        ["thos_phases"]
    ),
    check!("info_news", "info", "INFO 新闻第一页", ["info_session"]),
    check!(
        "info_catalog",
        "info",
        "INFO 新闻来源与栏目目录",
        ["info_session"]
    ),
    check!(
        "info_subscriptions",
        "info",
        "INFO 我的新闻订阅规则",
        ["info_session"]
    ),
    check!(
        "info_subscription_feed",
        "info",
        "首条真实订阅规则的新闻第一页",
        ["info_subscriptions"]
    ),
    check!(
        "info_favorites",
        "info",
        "INFO 我的收藏新闻（完整分页）",
        ["info_session"]
    ),
    check!(
        "info_search",
        "info",
        "INFO 固定关键词搜索",
        ["info_session"]
    ),
    check!("info_detail", "info", "首条真实新闻详情", ["info_news"]),
    check!(
        "library_session",
        "library",
        "图书馆区域证明",
        ["info_session"]
    ),
    check!(
        "library_area_tree",
        "library",
        "图书馆区域树",
        ["library_session"]
    ),
    check!(
        "library_day_segments",
        "library",
        "首个真实区域开放时段",
        ["library_area_tree"]
    ),
    check!(
        "library_seats",
        "library",
        "真实开放时段座位",
        ["library_day_segments"]
    ),
    check!(
        "library_socket_status",
        "library",
        "首个真实区域插座",
        ["library_area_tree"]
    ),
    check!(
        "classroom_session",
        "classroom",
        "教室业务证明",
        ["info_session"]
    ),
    check!(
        "classroom_buildings",
        "classroom",
        "教室楼栋列表",
        ["classroom_session"]
    ),
    check!(
        "classroom_state",
        "classroom",
        "首栋楼当前周教室状态",
        ["classroom_buildings"]
    ),
    check!(
        "electricity_session",
        "electricity",
        "电费业务证明",
        ["info_session"]
    ),
    check!(
        "electricity_remainder",
        "electricity",
        "电费余额",
        ["electricity_session"]
    ),
    check!(
        "electricity_history",
        "electricity",
        "电费缴费记录",
        ["electricity_session"]
    ),
    check!(
        "campus_card_session",
        "campus_card",
        "校园卡独立业务证明",
        ["identity_session"]
    ),
    check!(
        "campus_card_account",
        "campus_card",
        "校园卡账户读取",
        ["campus_card_session"]
    ),
    check!(
        "campus_card_transactions",
        "campus_card",
        "最近七天完整有界交易读取",
        ["campus_card_session"]
    ),
    check!(
        "usereg_session",
        "usereg",
        "网络自助账号、图片验证码与登录证明",
        ["portal_bootstrap"]
    ),
    check!(
        "usereg_account",
        "usereg",
        "网络自助账户绑定",
        ["usereg_session"]
    ),
    check!(
        "usereg_balance",
        "usereg",
        "网络自助用量与余额",
        ["usereg_session"]
    ),
    check!(
        "usereg_devices",
        "usereg",
        "网络自助设备只读列表",
        ["usereg_session"]
    ),
    check!(
        "tunet_status",
        "tunet",
        "校园网请求出口状态（不证明本机 IP 在线）",
        []
    ),
    check!(
        "tunet_local_status",
        "tunet",
        "校园网本机物理网卡 IPv4 状态（只读，不执行连接或断开）",
        []
    ),
    check!(
        "overview_live_inputs",
        "overview",
        "概览输入证明（非全量概览请求）",
        ["learn_courses", "learn_todos", "registrar_schedule"]
    ),
];

/// A focused live acceptance: only the two network services and the fresh
/// identity/portal prerequisites required by the App's USEREG flow.
pub fn network_service_cases() -> Result<BTreeSet<String>, String> {
    select_cases(&[
        "usereg_account".into(),
        "usereg_balance".into(),
        "usereg_devices".into(),
        "tunet_status".into(),
        "tunet_local_status".into(),
    ])
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pending,
    Running,
    Passed,
    Failed,
    Blocked,
    Skipped,
    /// Selected but not validated: environment, interaction or sample missing.
    Unverified,
    Interrupted,
}
impl CheckStatus {
    pub fn key(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
            Self::Unverified => "unverified",
            Self::Interrupted => "interrupted",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub status: CheckStatus,
    pub reason: String,
    pub duration_ms: u64,
    pub requests: u64,
    pub count: Option<usize>,
    pub attempts: u32,
    #[serde(default)]
    pub timing: Option<CaseTiming>,
}

/// A user-declared prerequisite, never inferred from a failed HTTP request.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkEnvironment {
    #[default]
    Unspecified,
    OffCampus,
}

impl NetworkEnvironment {
    pub fn skip_reason(self, case: &str) -> Option<&'static str> {
        (self == Self::OffCampus && matches!(case, "tunet_status" | "tunet_local_status"))
            .then_some("off_campus_network_unverified")
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReason {
    NotSelected,
    PreviouslyPassed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OmittedCheck {
    pub reason: OmissionReason,
    pub previous_build_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckReport {
    pub schema: u32,
    pub run_id: String,
    #[serde(default)]
    pub build_revision: String,
    #[serde(default)]
    pub logging_complete: Option<bool>,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: Option<chrono::DateTime<Utc>>,
    pub mode: String,
    pub graduate: bool,
    #[serde(default)]
    pub network_environment: NetworkEnvironment,
    pub sample_policy: String,
    pub cases: BTreeMap<String, CheckResult>,
    /// Full catalog visibility without importing old results as current proof.
    #[serde(default)]
    pub omitted_cases: BTreeMap<String, OmittedCheck>,
    pub request_count: u64,
    pub session_reusable_after_exit: bool,
    #[serde(default)]
    pub execution: Option<ExecutionSummary>,
}
pub struct ReportWriter {
    pub report: CheckReport,
    path: PathBuf,
    request_origin: u64,
    active: Option<(String, Instant, u64)>,
    clock: Instant,
    active_metrics: Option<crate::telemetry::timing::Collector>,
    checkpoint_totals: Cell<(u64, u64)>,
}
impl ReportWriter {
    pub fn new(
        path: PathBuf,
        run_id: String,
        selected: &BTreeSet<String>,
        graduate: bool,
    ) -> Result<Self, String> {
        if selected.is_empty()
            || selected
                .iter()
                .any(|id| !CHECKS.iter().any(|spec| spec.id == id))
            || CHECKS
                .iter()
                .filter(|spec| selected.contains(spec.id))
                .any(|spec| spec.dependencies.iter().any(|dep| !selected.contains(*dep)))
        {
            return Err("invalid_report_plan".into());
        }
        if path.exists() {
            return Err("report_already_exists".into());
        }
        let cases = CHECKS
            .iter()
            .filter(|c| selected.contains(c.id))
            .map(|c| {
                (
                    c.id.to_owned(),
                    CheckResult {
                        status: CheckStatus::Pending,
                        reason: "not_started".into(),
                        duration_ms: 0,
                        requests: 0,
                        count: None,
                        attempts: 0,
                        timing: None,
                    },
                )
            })
            .collect();
        let revision = option_env!("THYOU_BUILD_REVISION")
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap_or("unrecorded");
        let writer = Self {
            path,
            request_origin: crate::telemetry::request_count(),
            active: None,
            clock: Instant::now(),
            active_metrics: None,
            checkpoint_totals: Cell::new((0, 0)),
            report: CheckReport {
                schema: 1,
                run_id,
                build_revision: revision.into(),
                logging_complete: None,
                started_at: Utc::now(),
                finished_at: None,
                mode: "terminal_read_only".into(),
                graduate,
                network_environment: NetworkEnvironment::Unspecified,
                sample_policy: "one_real_course_for_lists_up_to_twelve_courses_for_homework_detail_one_article_one_area_one_building_seven_card_days"
                    .into(),
                cases,
                omitted_cases: CHECKS
                    .iter()
                    .filter(|spec| !selected.contains(spec.id))
                    .map(|spec| {
                        (
                            spec.id.into(),
                            OmittedCheck {
                                reason: OmissionReason::NotSelected,
                                previous_build_revision: None,
                            },
                        )
                    })
                    .collect(),
                request_count: 0,
                session_reusable_after_exit: false,
                execution: None,
            },
        };
        writer.save()?;
        Ok(writer)
    }
    fn save(&self) -> Result<(), String> {
        let started = Instant::now();
        let result = crate::telemetry::write_private_json(&self.path, &self.report)
            .map_err(|_| "report_write_failed".into());
        let (count, elapsed) = self.checkpoint_totals.get();
        self.checkpoint_totals.set((
            count.saturating_add(1),
            elapsed.saturating_add(crate::telemetry::timing::micros(started.elapsed())),
        ));
        result
    }
    pub fn set_graduate(&mut self, graduate: bool) -> Result<(), String> {
        if self.report.graduate != graduate {
            self.report.graduate = graduate;
            self.save()?;
        }
        Ok(())
    }
    pub fn set_previous_report(&mut self, path: &Path) -> Result<(), String> {
        let previous = read_retry_report(path)?;
        for (id, omitted) in &mut self.report.omitted_cases {
            if previous
                .cases
                .get(id)
                .is_some_and(|row| row.status == CheckStatus::Passed)
            {
                omitted.reason = OmissionReason::PreviouslyPassed;
                omitted.previous_build_revision = valid_revision(&previous.build_revision);
            } else if let Some(prior) = previous
                .omitted_cases
                .get(id)
                .filter(|row| row.reason == OmissionReason::PreviouslyPassed)
            {
                omitted.reason = OmissionReason::PreviouslyPassed;
                omitted.previous_build_revision = prior
                    .previous_build_revision
                    .as_deref()
                    .and_then(valid_revision);
            }
        }
        self.save()
    }
    pub fn set_network_environment(&mut self, network: NetworkEnvironment) -> Result<(), String> {
        if self.report.finished_at.is_some()
            || self
                .report
                .cases
                .values()
                .any(|row| row.status != CheckStatus::Pending)
        {
            return Err("network_scope_requires_unstarted_plan".into());
        }
        self.report.network_environment = network;
        self.save()
    }
    pub fn start(&mut self, id: &str) -> Result<(), String> {
        if self.active.is_some() {
            return Err("runtime_operation_already_active".into());
        }
        let row = self.report.cases.get_mut(id).ok_or("unknown_case")?;
        if row.status != CheckStatus::Pending {
            return Err("case_already_attempted".into());
        }
        row.status = CheckStatus::Running;
        row.reason = "in_progress".into();
        row.attempts = 1;
        self.active = Some((
            id.to_owned(),
            Instant::now(),
            crate::telemetry::request_count(),
        ));
        self.save()
    }
    fn end(
        &mut self,
        id: &str,
        status: CheckStatus,
        reason: &str,
        count: Option<usize>,
        elapsed: u64,
        requests: u64,
    ) -> Result<(), String> {
        let row = self.report.cases.get_mut(id).ok_or("unknown_case")?;
        if row.status != CheckStatus::Running && row.status != CheckStatus::Pending {
            return Err("immutable_case_result".into());
        }
        let timing = row.timing.take();
        *row = CheckResult {
            timing,
            status,
            reason: reason.into(),
            count,
            duration_ms: elapsed,
            requests,
            attempts: row.attempts,
        };
        self.report.request_count += requests;
        self.active = None;
        self.active_metrics = None;
        self.save()
    }
    pub fn finish(&mut self) -> Result<(), String> {
        let first_completion = self.report.finished_at.is_none();
        if first_completion {
            self.report.finished_at = Some(Utc::now());
        }
        // The later logging-completeness save must not inflate benchmark wall
        // time with CSV/Markdown formatting or log-worker shutdown.
        if first_completion && let Some(summary) = &mut self.report.execution {
            summary.total_wall_us = crate::telemetry::timing::micros(self.clock.elapsed());
            let (count, duration) = self.checkpoint_totals.get();
            summary.checkpoint_write_count = count;
            summary.checkpoint_write_us = duration;
            summary.user_input_us = self
                .report
                .cases
                .values()
                .filter_map(|r| r.timing.as_ref())
                .map(|t| t.user_input_us)
                .sum();
            summary.active_wall_us = summary.total_wall_us.saturating_sub(summary.user_input_us);
            summary.first_completed_after_us = self
                .report
                .cases
                .values()
                .filter(|row| row.status == CheckStatus::Passed)
                .filter_map(|row| {
                    row.timing
                        .as_ref()
                        .filter(|t| t.measured)
                        .map(|t| t.finished_after_us)
                })
                .min();
            summary.first_data_after_us = self
                .report
                .cases
                .iter()
                .filter(|(id, row)| {
                    row.status == CheckStatus::Passed
                        && !id.ends_with("_session")
                        && id.as_str() != "portal_bootstrap"
                        && id.as_str() != "overview_live_inputs"
                })
                .filter_map(|(_, row)| {
                    row.timing
                        .as_ref()
                        .filter(|t| t.measured)
                        .map(|t| t.finished_after_us)
                })
                .min();
        }
        self.save()
    }
    pub fn interrupted(&mut self) -> Result<(), String> {
        if let Some((id, start, before)) = self.active.take()
            && let Some(row) = self.report.cases.get_mut(&id)
        {
            row.duration_ms = start.elapsed().as_millis() as u64;
            if let Some(metrics) = self.active_metrics.take() {
                let metrics = metrics.snapshot();
                row.requests = metrics.requests;
                if let Some(timing) = &mut row.timing {
                    timing.execution_us = crate::telemetry::timing::micros(start.elapsed());
                    timing.user_input_us =
                        metrics.phase_us(crate::telemetry::timing::Phase::UserInput);
                    timing.active_execution_us =
                        timing.execution_us.saturating_sub(timing.user_input_us);
                    timing.finished_after_us =
                        crate::telemetry::timing::micros(self.clock.elapsed());
                    timing.total_after_ready_us = timing
                        .finished_after_us
                        .saturating_sub(timing.ready_after_us);
                    timing.detail = metrics;
                    timing.measured = true;
                }
            } else {
                row.requests = crate::telemetry::request_count().saturating_sub(before);
            }
        }
        for row in self.report.cases.values_mut() {
            if matches!(row.status, CheckStatus::Running | CheckStatus::Pending) {
                row.status = CheckStatus::Interrupted;
                row.reason = "user_or_process_stopped".into();
            }
        }
        self.report.request_count = if self.report.execution.is_some() {
            self.report.cases.values().map(|row| row.requests).sum()
        } else {
            crate::telemetry::request_count().saturating_sub(self.request_origin)
        };
        self.finish()
    }
    pub fn exit_code(&self) -> u8 {
        if self.report.cases.values().any(|r| {
            matches!(
                r.status,
                CheckStatus::Failed
                    | CheckStatus::Interrupted
                    | CheckStatus::Running
                    | CheckStatus::Pending
            )
        }) {
            1
        } else if self.report.cases.values().any(|r| {
            matches!(
                r.status,
                CheckStatus::Blocked | CheckStatus::Skipped | CheckStatus::Unverified
            )
        }) {
            3
        } else {
            0
        }
    }
}
impl Drop for ReportWriter {
    fn drop(&mut self) {
        if self.report.finished_at.is_none() {
            let _ = self.interrupted();
        }
    }
}

pub fn select_cases(requested: &[String]) -> Result<BTreeSet<String>, String> {
    let mut selected: BTreeSet<String> = if requested.is_empty() {
        CHECKS.iter().map(|s| s.id.to_owned()).collect()
    } else {
        requested.iter().cloned().collect()
    };
    if selected.iter().any(|id| !CHECKS.iter().any(|s| s.id == id)) {
        return Err("unknown_case_selection".into());
    }
    loop {
        let count = selected.len();
        for spec in CHECKS {
            if selected.contains(spec.id) {
                selected.extend(spec.dependencies.iter().map(|d| (*d).to_owned()));
            }
        }
        if selected.len() == count {
            break;
        }
    }
    Ok(selected)
}

/// Retry unfinished cases and their fresh in-process prerequisites. Prior
/// successes are never imported as cookies, data or current session proofs.
pub fn retry_plan(path: &Path) -> Result<(BTreeSet<String>, bool), String> {
    retry_plan_including_usereg(path, false)
}

fn valid_revision(value: &str) -> Option<String> {
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then(|| value.to_owned())
}

fn read_retry_report(path: &Path) -> Result<CheckReport, String> {
    use std::io::Read;
    let meta = fs::symlink_metadata(path).map_err(|_| "retry_report_unreadable")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 1024 * 1024 {
        return Err("retry_report_unsafe".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "retry_report_unreadable")?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "retry_report_unreadable")?;
    if bytes.len() > 1024 * 1024 {
        return Err("retry_report_unsafe".into());
    }
    let report: CheckReport = serde_json::from_slice(&bytes).map_err(|_| "retry_report_invalid")?;
    if report.schema != 1
        || report.mode != "terminal_read_only"
        || report
            .cases
            .keys()
            .any(|id| !CHECKS.iter().any(|spec| spec.id == id))
        || report
            .omitted_cases
            .keys()
            .any(|id| report.cases.contains_key(id) || !CHECKS.iter().any(|spec| spec.id == id))
    {
        return Err("retry_report_invalid".into());
    }
    Ok(report)
}

pub fn retry_plan_including_usereg(
    path: &Path,
    include_usereg: bool,
) -> Result<(BTreeSet<String>, bool), String> {
    let report = read_retry_report(path)?;
    let mut requested = report
        .cases
        .iter()
        .filter(|(_, row)| row.status != CheckStatus::Passed)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    // Explicit selection adds missing independent reads, but never reruns
    // business cases with a prior success. Its login still needs interaction.
    for spec in CHECKS.iter().filter(|spec| spec.service == "usereg") {
        if include_usereg
            && !report
                .cases
                .get(spec.id)
                .is_some_and(|row| row.status == CheckStatus::Passed)
            && !report
                .omitted_cases
                .get(spec.id)
                .is_some_and(|row| row.reason == OmissionReason::PreviouslyPassed)
        {
            requested.push(spec.id.into());
        }
    }
    // An empty selection means "all" to select_cases. Never accidentally
    // rerun an already successful report or force declined optional logins.
    if requested.is_empty() {
        return Err("retry_report_has_no_unfinished_cases".into());
    }
    Ok((select_cases(&requested)?, report.graduate))
}

/// Prompts are a terminal presentation boundary, not a source of reports.
/// Implementations MUST NOT echo/store secret inputs or print raw errors.
pub trait UserPrompts {
    fn secret(&mut self, label: &'static str) -> Result<String, String>;
    fn confirm(&mut self, label: &'static str) -> Result<bool, String>;
    fn choose_factor(&mut self, methods: &[String]) -> Result<String, String>;
    fn show_captcha(&mut self, bytes: &[u8], content_type: &str) -> Result<(), String>;
    fn clear_captcha(&mut self);
    fn progress(&mut self, spec: &CheckSpec, status: CheckStatus, reason: &str);
    fn device_trust_status(&mut self, _status: &str) {}
    fn timing(&mut self, _spec: &CheckSpec, _timing: &CaseTiming) {}
}

struct MeasuredPrompts<'a, P>(&'a mut P);
impl<P: UserPrompts> UserPrompts for MeasuredPrompts<'_, P> {
    fn secret(&mut self, label: &'static str) -> Result<String, String> {
        crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::UserInput, || {
            self.0.secret(label)
        })
    }
    fn confirm(&mut self, label: &'static str) -> Result<bool, String> {
        crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::UserInput, || {
            self.0.confirm(label)
        })
    }
    fn choose_factor(&mut self, methods: &[String]) -> Result<String, String> {
        crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::UserInput, || {
            self.0.choose_factor(methods)
        })
    }
    fn show_captcha(&mut self, bytes: &[u8], content_type: &str) -> Result<(), String> {
        crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::UserInput, || {
            self.0.show_captcha(bytes, content_type)
        })
    }
    fn clear_captcha(&mut self) {
        self.0.clear_captcha();
    }
    fn progress(&mut self, spec: &CheckSpec, status: CheckStatus, reason: &str) {
        self.0.progress(spec, status, reason);
    }
    fn device_trust_status(&mut self, status: &str) {
        self.0.device_trust_status(status);
    }
}

fn confirm_terminal_device_trust(prompt: &mut impl UserPrompts) -> Result<bool, String> {
    prompt.confirm("信任此终端设备以减少重复验证？仅保存随机设备标识，不保存密码、验证码或 Cookie；学校仍可要求验证。")
}

pub async fn complete_interactive_factor(
    runtime: &mut CampusRuntime,
    prompt: &mut impl UserPrompts,
) -> Result<(), String> {
    let status = runtime.status();
    if status.state != "requires_second_factor" {
        return Err("no_pending_challenge".into());
    }
    let method = prompt.choose_factor(&status.second_factor_methods)?;
    if !status.second_factor_methods.contains(&method) {
        return Err("factor_method_not_advertised".into());
    }
    if method != "totp"
        && prompt.confirm("发送一次验证码？选择否可输入你已收到的验证码；不会自动重发。")?
    {
        tracing::info!(target:"tsinghua_kit::auth",event="factor_send_requested",method=method.as_str());
        runtime.send_second_factor_code(method.clone()).await?;
    }
    let code = prompt.secret("请输入当前验证码（不回显，留空取消）：")?;
    if code.trim().is_empty() || code.len() > 64 {
        return Err("user_cancelled_factor".into());
    }
    tracing::info!(target:"tsinghua_kit::auth",event="factor_submission",method=method.as_str());
    runtime.complete_second_factor(method, code).await?;
    if runtime.status().state == "requires_second_factor" {
        return Err("factor_still_pending".into());
    }
    Ok(())
}

#[derive(Default)]
struct Evidence {
    courses: Vec<String>,
    course_selection: Option<LearnValidationEvidence>,
    todo_sample_count: Option<usize>,
    schedule_input_verified: bool,
    article: Option<String>,
    subscription_selector: Option<String>,
    phase_task_id: Option<String>,
    homework_selector: Option<String>,
    area: Option<u64>,
    segment: Option<crate::library_read::LibraryDaySegmentDto>,
    building: Option<(u32, u32)>,
}
const MAX_HOMEWORK_COURSE_SAMPLES: usize = 12;

enum Outcome {
    Passed(Option<usize>),
    ObservedNetwork(&'static str),
    Skipped(&'static str),
}

async fn establish(
    runtime: &mut CampusRuntime,
    prompt: &mut impl UserPrompts,
    service: &str,
) -> Result<(), String> {
    let mut result = runtime
        .establish_service_session(service.to_owned())
        .await
        .map(|_| ());
    if service == "campus_card" && runtime.has_terminal_card_password_boundary() {
        let continuation = async {
            if !prompt.confirm("校园卡目标页要求当前账号密码。是否仅为本次校园卡认证输入一次？不会保存或重新登录主账号。")? {
                return Err("campus_card_password_declined".to_owned());
            }
            let password = prompt.secret("当前校园账号密码（仅用于这次校园卡认证，不回显；留空取消）：")?;
            if password.is_empty() { return Err("campus_card_password_declined".to_owned()); }
            runtime.complete_terminal_card_password(password).await
        }.await;
        runtime.cancel_terminal_card_password();
        result = continuation;
    }
    if runtime.status().state == "requires_second_factor" {
        complete_interactive_factor(runtime, prompt).await?;
        // This is explicit continuation after a new verified factor, not a
        // retry of an unknown ticket. Only the selected target is completed.
        let advertised = runtime
            .service_catalog()
            .services
            .into_iter()
            .find(|s| s.id == service);
        if advertised.is_some_and(|s| s.availability == "available") {
            return Ok(());
        }
        return Err("service_proof_missing_after_factor".into());
    }
    result.map(|_| ())
}

async fn execute_case(
    runtime: &mut CampusRuntime,
    prompt: &mut impl UserPrompts,
    evidence: &mut Evidence,
    id: &str,
    include_usereg: bool,
) -> Result<Outcome, String> {
    let today = campus_date_at(Utc::now());
    match id {
        "identity_session" => {
            let username = prompt.secret("校园账号（不回显）：")?;
            let password = prompt.secret("校园密码（不回显）：")?;
            if username.trim().is_empty()
                || password.is_empty()
                || username.len() > 128
                || password.len() > 4096
            {
                return Err("invalid_terminal_credentials".into());
            }
            let trust_device = confirm_terminal_device_trust(prompt)?;
            // Terminal validation keeps the automatic Reference-rule stage
            // detection unless the command has an explicit stage option.
            runtime
                .login(username, password, None, false, trust_device, false)
                .await?;
            if runtime.status().state == "requires_second_factor" {
                complete_interactive_factor(runtime, prompt).await?;
            }
            if !runtime.service_session_is_proven(ServiceId::Identity) {
                return Err("identity_not_proven".into());
            }
        }
        "portal_bootstrap" => {
            let user = runtime
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .user
                .ok_or("identity_not_proven")?;
            let result = runtime.ensure_portal_bootstrap(&user).await;
            if runtime.status().state == "requires_second_factor" {
                complete_interactive_factor(runtime, prompt).await?;
            } else {
                result?;
            }
            if !runtime.portal_bootstrapped {
                return Err("portal_not_proven".into());
            }
        }
        "learn_session" => establish(runtime, prompt, "learn").await?,
        "learn_courses" => {
            let selection = runtime.capture_validation_learn_courses().await?;
            evidence.courses = selection.ids();
            evidence.course_selection = Some(selection);
            return Ok(Outcome::Passed(Some(evidence.courses.len())));
        }
        "learn_calendar" => {
            let calendar = runtime.load_learn_term_calendar().await?;
            validation_scope::require_live_validation_result(&calendar.source, &calendar.status)?;
            if calendar.current.week_count == 0
                || calendar.upcoming.iter().any(|term| term.week_count == 0)
            {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(calendar.upcoming.len() + 1)));
        }
        "school_calendar" => {
            let image = runtime
                .load_school_calendar(None, "autumn".into(), "zh".into())
                .await?;
            validation_scope::require_live_validation_result(&image.source, &image.status)?;
            if image.year != image.latest_year || image.image_bytes.is_empty() {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(1)));
        }
        "learn_announcements" => {
            let Some(id) = evidence.courses.first().cloned() else {
                return Ok(Outcome::Skipped("no_course_selector"));
            };
            return runtime
                .load_learn_announcements(id)
                .await
                .map(|r| Outcome::Passed(Some(r.len())));
        }
        "learn_todos" => {
            let selection = evidence
                .course_selection
                .as_ref()
                .ok_or("course_evidence_unavailable")?;
            if evidence.courses.is_empty() {
                return Ok(Outcome::Skipped("no_course_selector"));
            }
            let count = runtime.read_validation_learn_todos(selection, true).await?;
            evidence.todo_sample_count = Some(count);
            return Ok(Outcome::Passed(Some(count)));
        }
        "learn_files" => {
            let Some(id) = evidence.courses.first().cloned() else {
                return Ok(Outcome::Skipped("no_course_selector"));
            };
            let result = runtime.load_learn_files(id.clone()).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.course_id != id || !result.complete || result.error.is_some() {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.files.len())));
        }
        "learn_file_categories" => {
            let Some(id) = evidence.courses.first().cloned() else {
                return Ok(Outcome::Skipped("no_course_selector"));
            };
            let result = runtime.load_learn_file_categories(id.clone()).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.course_id != id {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.categories.len())));
        }
        "learn_file_download" => {
            let Some(id) = evidence.courses.first().cloned() else {
                return Ok(Outcome::Skipped("no_course_selector"));
            };
            let result = runtime.load_learn_files(id.clone()).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.course_id != id || !result.complete || result.error.is_some() {
                return Err("validation_live_result_required".into());
            }
            let Some(file) = result.files.first() else {
                return Ok(Outcome::Skipped("no_file_selector"));
            };
            let bytes = runtime
                .probe_learn_file_download(id, file.id.clone())
                .await?;
            if bytes == 0 {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(1)));
        }
        "learn_discussions" => {
            let Some(id) = evidence.courses.first().cloned() else {
                return Ok(Outcome::Skipped("no_course_selector"));
            };
            let result = runtime.load_learn_discussions(id.clone()).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.course_id != id || !result.complete || result.error.is_some() {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "learn_homework" => {
            let Some(id) = evidence.courses.first().cloned() else {
                return Ok(Outcome::Skipped("no_course_selector"));
            };
            let result = runtime.load_learn_homework(id.clone()).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.course_id != id {
                return Err("validation_live_result_required".into());
            }
            evidence.homework_selector = result
                .items
                .iter()
                .find(|item| item.detail_available)
                .map(|item| item.selector.clone());
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "learn_homework_detail" => {
            // The list case has already read the first course. A first course
            // without homework is not evidence that no course has a detail.
            // Each later list refreshes the Runtime selector, so stop as soon
            // as a detail-capable item is found and read it before another list.
            if evidence.homework_selector.is_none() {
                for id in evidence
                    .courses
                    .iter()
                    .skip(1)
                    .take(MAX_HOMEWORK_COURSE_SAMPLES - 1)
                {
                    let result = runtime.load_learn_homework(id.clone()).await?;
                    validation_scope::require_live_validation_result(
                        &result.source,
                        &result.status,
                    )?;
                    if result.course_id != *id {
                        return Err("validation_live_result_required".into());
                    }
                    if let Some(item) = result.items.iter().find(|item| item.detail_available) {
                        evidence.homework_selector = Some(item.selector.clone());
                        break;
                    }
                }
            }
            let Some(selector) = evidence.homework_selector.clone() else {
                return Ok(Outcome::Skipped(
                    if evidence.courses.len() > MAX_HOMEWORK_COURSE_SAMPLES {
                        "homework_sample_limit"
                    } else {
                        "no_homework_selector"
                    },
                ));
            };
            let result = runtime.load_learn_homework_detail(selector.clone()).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.selector != selector {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.attachments.len())));
        }
        "registrar_session" => establish(runtime, prompt, "registrar").await?,
        "registrar_schedule" => {
            // Establishment has just performed the same actual calendar read.
            // Its dedicated case documents proof reuse, not a second request.
            if !runtime.service_session_is_proven(ServiceId::Registrar) {
                return Err("registrar_not_proven".into());
            }
            evidence.schedule_input_verified = true;
        }
        "registrar_grades" => {
            return runtime.load_grades().await.and_then(|r| {
                validation_scope::require_live_validation_result(
                    r.source.as_deref().unwrap_or(""),
                    r.status.as_deref().unwrap_or(""),
                )?;
                Ok(Outcome::Passed(Some(r.courses.len())))
            });
        }
        "registrar_exams" => {
            return runtime
                .load_exams(String::new(), String::new())
                .await
                .and_then(|r| {
                    validation_scope::require_live_validation_result(
                        r.source.as_deref().unwrap_or(""),
                        r.status.as_deref().unwrap_or(""),
                    )?;
                    Ok(Outcome::Passed(Some(r.exams.len())))
                });
        }
        "info_session" => establish(runtime, prompt, "info").await?,
        "thos_pending" => {
            let result = runtime.load_thos_pending(true).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if !result.complete || result.error.is_some() {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "thos_services" => {
            let result = runtime.load_thos_services(true).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if !result.complete || result.error.is_some() {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "thos_completed" | "thos_drafts" | "thos_unread" | "thos_phases" => {
            let kind = id.strip_prefix("thos_").expect("fixed THOS case");
            let result = runtime.load_thos_task_list(kind.into(), true).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.kind != kind || !result.complete || result.error.is_some() {
                return Err("validation_live_result_required".into());
            }
            if kind == "phases" {
                evidence.phase_task_id = result.items.first().map(|item| item.id.clone());
            }
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "thos_phase_steps" => {
            let Some(task_id) = evidence.phase_task_id.clone() else {
                return Ok(Outcome::Skipped("no_phase_selector"));
            };
            let result = runtime.load_thos_phase_steps(task_id.clone(), true).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.task_id != task_id {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.steps.len())));
        }
        "info_news" => {
            let page = runtime.load_info_news(1, 10, None, None).await?;
            evidence.article = page.items.first().map(|i| i.link.clone());
            return Ok(Outcome::Passed(Some(page.items.len())));
        }
        "info_catalog" => {
            let catalog = runtime.load_info_news_catalog().await?;
            if catalog.status == "partial" && catalog.channel_error.is_some() {
                return Err("validation_news_catalog_partial".into());
            }
            validation_scope::require_live_validation_result(&catalog.source, &catalog.status)?;
            if catalog.sources.is_empty() || catalog.channels.is_empty() {
                return Err("validation_news_catalog_empty".into());
            }
            return Ok(Outcome::Passed(Some(
                catalog.sources.len() + catalog.channels.len(),
            )));
        }
        "info_subscriptions" => {
            let result = runtime.load_info_news_subscriptions().await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            evidence.subscription_selector = result.rules.first().map(|rule| rule.selector.clone());
            return Ok(Outcome::Passed(Some(result.rules.len())));
        }
        "info_subscription_feed" => {
            let Some(selector) = evidence.subscription_selector.clone() else {
                return Ok(Outcome::Skipped("no_subscription_selector"));
            };
            let result = runtime
                .load_info_news_subscription_page(selector, 1)
                .await?;
            validation_scope::require_live_validation_result(
                result.source.as_deref().unwrap_or(""),
                result.status.as_deref().unwrap_or(""),
            )?;
            if result.feed != "subscription" {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "info_favorites" => {
            let result = runtime.load_info_news_favorites().await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
            if result.empty != result.items.is_empty() {
                return Err("validation_live_result_required".into());
            }
            return Ok(Outcome::Passed(Some(result.items.len())));
        }
        "info_search" => {
            return runtime
                .search_info_news(1, "清华".into(), None, false)
                .await
                .map(|r| Outcome::Passed(Some(r.items.len())));
        }
        "info_detail" => {
            let Some(id) = evidence.article.clone() else {
                return Ok(Outcome::Skipped("no_article_selector"));
            };
            let result = runtime.load_info_news_detail_result(id).await?;
            validation_scope::require_live_validation_result(&result.source, &result.status)?;
        }
        "library_session" => establish(runtime, prompt, "library").await?,
        "library_area_tree" => {
            let tree = runtime.load_library_area_tree().await?;
            // /areas/1/tree/1 lists libraries, not seat sections. Mirror the
            // reference: library -> floors -> today's sections before areadays.
            evidence.area = runtime
                .discover_library_sample_section(&tree.areas, &today.to_string())
                .await?;
            return Ok(Outcome::Passed(Some(tree.areas.len())));
        }
        "library_day_segments" => {
            let Some(id) = evidence.area else {
                return Ok(Outcome::Skipped("no_area_selector"));
            };
            let rows = runtime.load_library_day_segments(id).await?;
            evidence.segment = select_open_library_segment(&rows.segments, Utc::now());
            return Ok(Outcome::Passed(Some(rows.segments.len())));
        }
        "library_seats" => {
            let (Some(id), Some(s)) = (evidence.area, evidence.segment.clone()) else {
                return Ok(Outcome::Skipped("no_open_segment"));
            };
            return runtime
                .load_library_seats(id, s.id, s.day, s.start_time, s.end_time)
                .await
                .map(|r| Outcome::Passed(Some(r.seats.len())));
        }
        "library_socket_status" => {
            let Some(id) = evidence.area else {
                return Ok(Outcome::Skipped("no_area_selector"));
            };
            return runtime
                .load_library_socket_status(id)
                .await
                .map(|r| Outcome::Passed(Some(r.records.len())));
        }
        "classroom_session" => establish(runtime, prompt, "classroom").await?,
        "classroom_buildings" => {
            let rows = runtime.load_classroom_buildings().await?;
            evidence.building = rows
                .iter()
                .enumerate()
                .find(|(_, b)| b.week_number > 0)
                .map(|(i, b)| (i as u32, b.week_number));
            return Ok(Outcome::Passed(Some(rows.len())));
        }
        "classroom_state" => {
            let Some((index, week)) = evidence.building else {
                return Ok(Outcome::Skipped("no_building_selector"));
            };
            return runtime
                .load_classroom_state(index, week)
                .await
                .map(|r| Outcome::Passed(Some(r.classroom_states.len())));
        }
        "electricity_session" => establish(runtime, prompt, "electricity").await?,
        "electricity_remainder" => {
            runtime.load_electricity_remainder().await?;
        }
        "electricity_history" => {
            return runtime
                .load_electricity_payment_history()
                .await
                .map(|r| Outcome::Passed(Some(r.records.len())));
        }
        "campus_card_session" => establish(runtime, prompt, "campus_card").await?,
        "campus_card_account" => {
            runtime.load_campus_card_account().await?;
        }
        "campus_card_transactions" => {
            return runtime
                .load_campus_card_transactions(
                    (today - chrono::Duration::days(6)).to_string(),
                    today.to_string(),
                )
                .await
                .map(|r| Outcome::Passed(Some(r.len())));
        }
        "usereg_session" => {
            if !include_usereg {
                return Ok(Outcome::Skipped("optional_login_not_selected"));
            }
            let username = prompt.secret("网络自助账号（可与统一认证不同，不回显）：")?;
            let password = prompt.secret("网络自助密码（不回显）：")?;
            let image = runtime.start_usereg_login(username, password).await?;
            finish_usereg_interaction(runtime, prompt, image).await?;
            if !runtime.service_session_is_proven(ServiceId::Usereg) {
                return Err("usereg_not_proven".into());
            }
        }
        "usereg_account" => {
            runtime.load_usereg_account().await?;
        }
        "usereg_balance" => {
            runtime.load_usereg_balance().await?;
        }
        "usereg_devices" => {
            return runtime
                .load_usereg_devices()
                .await
                .map(|r| Outcome::Passed(Some(r.len())));
        }
        "tunet_status" => {
            let client = network_status_client(runtime)?;
            let record = client
                .status_for_request_origin()
                .await
                .map_err(|_| "tunet_status_unconfirmed")?;
            return network_observation_outcome(&record);
        }
        "tunet_local_status" => {
            let ip = local_ipv4_address().map_err(|_| "tunet_local_address_unavailable")?;
            let client = network_status_client(runtime)?;
            return local_network_status(&client, &ip).await;
        }
        "overview_live_inputs" => validate_overview_inputs(runtime, evidence)?,
        _ => return Err("unknown_case_selection".into()),
    }
    Ok(Outcome::Passed(None))
}

fn network_status_client(runtime: &CampusRuntime) -> Result<TunetClient, String> {
    let config = crate::tunet_client::TunetClientConfig::current(crate::tunet::AuthFamily::Auth4)
        .map_err(|_| "tunet_config_failed")?;
    TunetClient::with_transport(config, runtime.identity.transport().clone())
        .map_err(|_| "tunet_config_failed".into())
}

async fn local_network_status(client: &TunetClient, ip: &str) -> Result<Outcome, String> {
    // A single read, with the same IP binding as the App. Never challenge,
    // login, logout, poll, or adopt another device's online status.
    let record = client
        .status(&[("ip", ip)])
        .await
        .map_err(|_| "tunet_status_unconfirmed")?;
    if record.is_online_proven_for_ip(ip) {
        Ok(Outcome::ObservedNetwork("tunet_local_online"))
    } else if record.is_offline_proven_for_ip(ip) {
        Ok(Outcome::ObservedNetwork("tunet_local_offline"))
    } else {
        Err(record.unproven_reason_for_ip(ip).into())
    }
}

pub(super) async fn finish_usereg_interaction<P: UserPrompts>(
    runtime: &mut CampusRuntime,
    prompt: &mut P,
    image: UseregCaptchaDto,
) -> Result<(), String> {
    let result = async {
        prompt.show_captcha(&image.bytes, &image.content_type)?;
        let code = prompt.secret("请输入图片验证码（不回显）：")?;
        runtime.complete_usereg_login(code, None).await?;
        Ok(())
    }
    .await;
    // Viewer refusal/failure and cancelled input take this path too. The
    // terminal has no retry dialog, so release unfinished credentials now.
    prompt.clear_captcha();
    runtime.cancel_usereg_login();
    result
}

fn validate_overview_inputs(runtime: &CampusRuntime, evidence: &Evidence) -> Result<(), String> {
    let courses = evidence
        .course_selection
        .as_ref()
        .ok_or("course_evidence_unavailable")?;
    courses.require_current(runtime)?;
    if evidence.courses != courses.ids() || evidence.todo_sample_count.is_none() {
        return Err("course_evidence_unavailable".into());
    }
    if !evidence.schedule_input_verified || !runtime.service_session_is_proven(ServiceId::Registrar)
    {
        return Err("registrar_not_proven".into());
    }
    Ok(())
}

/// The reference clamps today's start to current campus time. Closed
/// windows are not valid samples and must not send impossible seat queries.
fn select_open_library_segment(
    rows: &[crate::library_read::LibraryDaySegmentDto],
    now: chrono::DateTime<Utc>,
) -> Option<crate::library_read::LibraryDaySegmentDto> {
    let campus =
        now.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).expect("campus offset"));
    let day = campus.date_naive().to_string();
    let time = campus.format("%H:%M").to_string();
    rows.iter()
        .find(|row| row.day == day && row.end_time > time)
        .cloned()
        .map(|mut row| {
            if row.start_time < time {
                row.start_time = time;
            }
            row
        })
}

fn network_observation_outcome(record: &TunetStatusRecord) -> Result<Outcome, String> {
    if record.is_online_proven() {
        Ok(Outcome::ObservedNetwork("tunet_request_origin_online"))
    } else if record.is_offline_proven() {
        Ok(Outcome::ObservedNetwork("tunet_request_origin_offline"))
    } else {
        Err("tunet_state_unproven".into())
    }
}

/// App fan-in is the default. Authentication remains exclusive in one
/// Runtime, while backend-internal independent branches keep their concurrency.
pub async fn run(
    runtime: &mut CampusRuntime,
    prompt: &mut impl UserPrompts,
    report: &mut ReportWriter,
    include_usereg: bool,
) -> Result<(), String> {
    run_with_options(
        runtime,
        prompt,
        report,
        include_usereg,
        ExecutionOptions::default(),
    )
    .await
}

pub async fn run_with_options(
    runtime: &mut CampusRuntime,
    prompt: &mut impl UserPrompts,
    report: &mut ReportWriter,
    include_usereg: bool,
    options: ExecutionOptions,
) -> Result<(), String> {
    schedule::execute(runtime, prompt, report, include_usereg, options).await
}

pub fn write_markdown(report: &CheckReport, path: &Path) -> Result<(), String> {
    let mut text = format!(
        "# THYou 真实只读验收\n\n运行 ID：`{}`\n\n范围：每个接口族的有限真实样本，不等于所有账户记录全量测试。\n\n| 检查 | 状态 | 原因 | 耗时 ms | HTTP 请求 |\n|---|---|---|---:|---:|\n",
        report.run_id
    );
    for spec in CHECKS {
        if let Some(row) = report.cases.get(spec.id) {
            text.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                spec.label,
                row.status.key(),
                row.reason,
                row.duration_ms,
                row.requests
            ));
        }
    }
    if report.network_environment == NetworkEnvironment::OffCampus {
        text.push_str("\n网络环境：用户显式选择校外模式。校园网出口状态仍未验证，本轮未发起检查，也未证明校园网在线或离线；此前的超时问题不因此视为已解决。其他已选择的服务仍按真实响应验收。\n");
    }
    if report
        .cases
        .values()
        .any(|row| matches!(row.status, CheckStatus::Skipped | CheckStatus::Unverified))
    {
        text.push_str("\n验收未完整：缺少样本、独立登录未选择和网络环境限制均不等于通过；这些项目保留为未验证，退出码为 3（若另有失败则为 1）。旧 skipped 记录也不算完成。\n");
    }
    let omitted: Vec<_> = CHECKS
        .iter()
        .filter(|spec| !report.cases.contains_key(spec.id))
        .collect();
    if !omitted.is_empty() {
        text.push_str("\n## 本轮未执行的目录项\n\n历史通过仅说明此前结果，不能作为本轮源码或会话证明；未选择不等于不适用。\n\n| 检查 | 未执行原因 |\n|---|---|\n");
        for spec in omitted {
            let reason = if report
                .omitted_cases
                .get(spec.id)
                .is_some_and(|row| row.reason == OmissionReason::PreviouslyPassed)
            {
                "历史通过，本轮未重跑"
            } else {
                "未选择，未验证"
            };
            text.push_str(&format!("| {} | {} |\n", spec.label, reason));
        }
    }
    if let Some(summary) = &report.execution {
        text.push_str(&format!(
            "\n## 调度与测速\n\n模式：`{:?}`；最多并发入队 {} 项；实际峰值入队 {} 项；Runtime 同时执行峰值 {}（上限 {}，与 Flutter 相同）。\n\n本轮墙钟 {:.3} s；人工交互 {:.3} s；扣除交互 {:.3} s。\n",
            summary.mode, summary.configured_concurrency, summary.max_submitted,
            summary.max_runtime_executing, summary.runtime_parallelism_limit,
            summary.total_wall_us as f64 / 1_000_000.0, summary.user_input_us as f64 / 1_000_000.0,
            summary.active_wall_us as f64 / 1_000_000.0));
        text.push_str(&format!("\n检查点写入 {} 次，累计 {:.3} ms（已包含在墙钟，不重复相加；不含验收结束后的报告导出）。\n",
            summary.checkpoint_write_count, summary.checkpoint_write_us as f64 / 1000.0));
        text.push_str("\n| 检查 | 依赖等待 ms | 调度等待 ms | Runtime 排队 ms | 执行 ms | 人工输入 ms | 排除人工 ms | HTTP 响应头 P95 ms |\n|---|---:|---:|---:|---:|---:|---:|---:|\n");
        for spec in CHECKS {
            if let Some(t) = report
                .cases
                .get(spec.id)
                .and_then(|row| row.timing.as_ref())
                .filter(|t| t.measured)
            {
                let headers = t
                    .detail
                    .phases
                    .get(&crate::telemetry::timing::Phase::ResponseHeaders);
                let p95 = headers
                    .filter(|h| h.count > 0)
                    .map(|h| format!("{:.3}", h.p95_us as f64 / 1000.0))
                    .unwrap_or_else(|| "—".into());
                text.push_str(&format!(
                    "| {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {:.3} | {} |\n",
                    spec.label,
                    t.dependency_wait_us as f64 / 1000.0,
                    t.scheduler_wait_us as f64 / 1000.0,
                    t.queue_wait_us as f64 / 1000.0,
                    t.execution_us as f64 / 1000.0,
                    t.user_input_us as f64 / 1000.0,
                    t.active_execution_us as f64 / 1000.0,
                    p95
                ));
            }
        }
        text.push_str("\n所有时间来自单调时钟；响应头耗时包含连接与服务端等待，不伪造 DNS/TCP/TLS 子阶段。正文计时包含读取与字符集解码；文本字节数是解码后长度，不是线路流量。阶段可嵌套或并发，累计阶段不能直接相加当墙钟耗时。HTTP 无样本时显示空值；百分位按最多 4096 个样本计算，超过时显式标记截断。\n");
    }
    text.push_str("\n密码、验证码、Cookie、票据、姓名、课程/成绩/交易等正文均未写入本报告。终端仅复用随机设备标识；进程退出后会话不可恢复。设备信任登记结果以日志中的固定状态为准，不等于所有服务永久免验证码。\n");
    if report
        .cases
        .get("tunet_status")
        .is_some_and(|row| row.reason.starts_with("tunet_request_origin_"))
    {
        text.push_str("\n校园网项只证明状态接口返回了明确的请求出口在线/离线状态；不证明本机某个 IP 或主认证账号在线，不会自动登录、退出或修改代理/VPN。\n");
    }
    if report.cases.contains_key("tunet_local_status") {
        text.push_str("\n本机校园网项单独核对本机 IPv4 与服务返回地址；其他地址在线、格式异常或连接失败不能当作本机离线。在线证明仅针对地址，不代表验证了主身份的网络账号或执行了校园网登录/退出。\n");
    }
    text.push_str(match report.logging_complete {
        Some(true) => "\n日志队列已完成刷新，未检测到文件写入失败。\n",
        Some(false) => "\n警告：日志写入不完整；业务项结果不能替代完整的诊断证据。\n",
        None => "\n日志完整性尚未确认，可能因进程中断而缺少末尾事件。\n",
    });
    let mut file = crate::telemetry::new_private_file(path)
        .map_err(|_| "markdown_report_exists_or_unavailable")?;
    use std::io::Write;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| "markdown_report_write_failed".into())
}

/// Fixed identifiers and numeric fields only; safe to open in a spreadsheet.
pub fn write_timings_csv(report: &CheckReport, path: &Path) -> Result<(), String> {
    use std::io::Write;
    let mut file =
        crate::telemetry::new_private_file(path).map_err(|_| "timing_csv_unavailable")?;
    file.write_all(b"case,status,measured,dependency_wait_us,scheduler_wait_us,queue_wait_us,execution_us,user_input_us,active_execution_us,finished_after_us,requests\n")
        .map_err(|_| "timing_csv_write_failed")?;
    for spec in CHECKS {
        if let Some(row) = report.cases.get(spec.id) {
            let line = if let Some(t) = row.timing.as_ref().filter(|t| t.measured) {
                format!(
                    "{},{},true,{},{},{},{},{},{},{},{}\n",
                    spec.id,
                    row.status.key(),
                    t.dependency_wait_us,
                    t.scheduler_wait_us,
                    t.queue_wait_us,
                    t.execution_us,
                    t.user_input_us,
                    t.active_execution_us,
                    t.finished_after_us,
                    row.requests
                )
            } else {
                format!(
                    "{},{},false,,,,,,,,{}\n",
                    spec.id,
                    row.status.key(),
                    row.requests
                )
            };
            file.write_all(line.as_bytes())
                .map_err(|_| "timing_csv_write_failed")?;
        }
    }
    file.sync_all()
        .map_err(|_| "timing_csv_write_failed".into())
}
