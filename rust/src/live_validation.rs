//! Debug-only persistent result ledger for authenticated backend validation.
//! No usernames, records, URLs, credentials, or response bodies are stored.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const SCHEMA: u32 = 1;
const CONTAINER_ID: &str = "com.thyou.thyou";
const ENABLE_FILE: &str = "backend-live-validation.enabled";
const RESULT_FILE: &str = "backend-live-results.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CaseResult {
    pub status: String,
    pub attempts: u32,
    pub category: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LiveValidationLedger {
    schema: u32,
    pub cases: BTreeMap<String, CaseResult>,
    #[serde(skip)]
    selected: Option<BTreeSet<String>>,
    #[serde(skip)]
    destination: Option<PathBuf>,
    pub updated_at: DateTime<Utc>,
}

impl Default for LiveValidationLedger {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            cases: BTreeMap::new(),
            selected: None,
            destination: None,
            updated_at: Utc::now(),
        }
    }
}

pub(crate) fn error_reason(error: &str) -> &'static str {
    for stage in [
        "validation_live_result_required",
        "campus_card_probe_invalid_json",
        "campus_card_probe_invalid_envelope",
        "campus_card_probe_result_missing",
        "campus_card_probe_outer_failure",
        "campus_card_probe_network",
        "campus_card_rate_limited",
        "campus_card_probe_unavailable",
        "campus_card_probe_http",
        "campus_card_cookie_rejected",
        "campus_card_account_mismatch",
        "campus_card_probe_account_missing",
        "campus_card_encrypted_payload_format",
        "campus_card_service_rejected",
        "campus_card_probe_route",
        "campus_card_probe_config",
        "campus_card_probe_format",
        "portal_resume_config",
        "portal_resource_login_required",
        "portal_after_handoff_csrf_missing",
        "portal_after_handoff_login_required",
        "portal_resource_http_rejected",
        "portal_resource_webvpn_login",
        "portal_resource_identity_login",
        "portal_resource_document_login",
        "portal_resource_http",
        "portal_resource_unconfirmed",
        "portal_webvpn_entry_generic_notice",
        "portal_webvpn_home_generic_notice",
        "portal_webvpn_target_generic_notice",
        "portal_oauth_generic_notice",
        "portal_identity_submitted_generic_notice",
        "portal_identity_entry_trusted_form",
        "portal_identity_entry_trusted_script",
        "portal_identity_entry_password_form",
        "portal_identity_entry_form_without_key",
        "portal_identity_entry_no_form",
        "portal_identity_target_trusted_form",
        "portal_identity_target_trusted_script",
        "portal_identity_target_password_form",
        "portal_identity_target_form_without_key",
        "portal_identity_target_no_form",
        "portal_resume_csrf_missing",
        "portal_resume_login_required",
        "portal_resume_cookie_http",
        "portal_resume_cookie_route",
        "portal_resume_account_http",
        "portal_resume_account_route",
        "portal_resume_account_mismatch",
        "portal_resume_account_format",
        "portal_resume_network",
        "portal_resume_unconfirmed",
        "portal_resume_sso_route",
        "portal_resume_sso_network",
        "portal_resume_sso_http",
        "portal_resume_sso_rejected",
        "portal_resume_sso_body_limit",
        "portal_resume_sso_encoding",
        "portal_resume_sso_hop_limit",
        "portal_resume_rate_limited",
        "portal_resume_trusted_network",
        "portal_resume_trusted_unconfirmed",
        "portal_resume_trusted_script_only",
        "portal_resume_trusted_password_form",
        "portal_resume_trusted_form_without_key",
        "portal_resume_trusted_no_form",
        "portal_resume_second_factor_required",
        "portal_resume_identity_rejected",
        "portal_resume_captcha_required",
        "portal_resume_credential_page_rejected",
        "portal_resume_identity_failure",
        "portal_resume_password_required",
        "portal_resume_navigation_unconfirmed",
    ] {
        if error == stage {
            return stage;
        }
    }
    if error.contains("课程讨论数据格式未确认") {
        "learn_discussions_format"
    } else if error.contains("课程讨论来源路径未确认") {
        "learn_discussions_route"
    } else if error.contains("课程讨论网络连接失败") {
        "learn_discussions_network"
    } else if error.contains("课程讨论请求失败") {
        "learn_discussions_http"
    } else if error.contains("课程资料超过 512 MB") {
        "learn_file_download_limit"
    } else if error.contains("资料下载返回内容未确认") {
        "learn_file_download_content"
    } else if error.contains("资料下载目标未确认") {
        "learn_file_download_route"
    } else if error.contains("资料下载网络连接失败") {
        "learn_file_download_network"
    } else if error.contains("资料下载请求失败") {
        "learn_file_download_http"
    } else if error.contains("课程资料数据格式未确认") {
        "learn_files_format"
    } else if error.contains("课程资料来源路径未确认") {
        "learn_files_route"
    } else if error.contains("课程资料网络连接失败") {
        "learn_files_network"
    } else if error.contains("课程资料请求失败") {
        "learn_files_http"
    } else if error.contains("课程作业数据格式未确认") || error.contains("作业详情数据格式未确认")
    {
        "learn_homework_format"
    } else if error.contains("课程作业来源路径未确认") || error.contains("作业详情来源路径未确认")
    {
        "learn_homework_route"
    } else if error.contains("课程作业网络连接失败") || error.contains("作业详情网络连接失败")
    {
        "learn_homework_network"
    } else if error.contains("课程作业请求失败") || error.contains("作业详情请求失败")
    {
        "learn_homework_http"
    } else if error.contains("作业达到单次读取上限") {
        "learn_homework_limit"
    } else if error.contains("WebVPN 门户免密码续接未确认") {
        "portal_trusted_continuation_unconfirmed"
    } else if error.contains("校园卡免密码续接未确认") {
        "campus_card_trusted_continuation_unconfirmed"
    } else if error.contains("WebVPN 门户免密码续接网络失败") {
        "portal_trusted_continuation_network"
    } else if error.contains("WebVPN 门户需要交互认证") {
        "portal_interactive_auth_required"
    } else if error.contains("校园卡免密码续接网络失败") {
        "campus_card_trusted_continuation_network"
    } else if error.contains("校园卡需要交互认证") {
        "campus_card_interactive_auth_required"
    } else if error.contains("校园卡需要二次认证") {
        "campus_card_second_factor_required"
    } else if error.contains("WebVPN 门户需要重新完成统一认证") {
        "portal_identity_required"
    } else if error.contains("WebVPN 门户网络连接失败") {
        "portal_network"
    } else if error.contains("WebVPN 门户登录页响应异常") {
        "portal_login_page_http"
    } else if error.contains("WebVPN 门户登录页格式异常") {
        "portal_login_page_format"
    } else if error.contains("WebVPN 门户登录加密失败") {
        "portal_crypto"
    } else if error.contains("WebVPN 门户登录响应异常") {
        "portal_login_http"
    } else if error.contains("WebVPN 门户需要二次认证") {
        "portal_second_factor_required"
    } else if error.contains("WebVPN 门户目标登录页状态未确认") {
        "portal_fresh_entry_unconfirmed"
    } else if error.contains("WebVPN 门户凭证提交状态未确认") {
        "portal_fresh_submit_unconfirmed"
    } else if error.contains("WebVPN 门户目标页状态未确认") {
        "portal_fresh_target_unconfirmed"
    } else if error.contains("WebVPN 门户登录状态未确认") {
        "portal_session_unconfirmed"
    } else if error.contains("WebVPN 门户未返回目标服务跳转") {
        "portal_target_missing"
    } else if error.contains("WebVPN 门户目标跳转被拒绝") {
        "portal_target_rejected"
    } else if error.contains("WebVPN 门户 OAuth 跳转未确认") {
        "portal_oauth_unconfirmed"
    } else if error.contains("WebVPN 门户 OAuth 跳转被拒绝") {
        "portal_oauth_rejected"
    } else if error.contains("WebVPN 门户配置无效") {
        "portal_config"
    } else if error.contains("学习平台登录状态未确认") {
        "learn_session_unconfirmed"
    } else if error.contains("学习平台服务会话已过期") {
        "learn_session_expired"
    } else if error.contains("学习平台缺少 CSRF") {
        "learn_missing_csrf"
    } else if error.contains("学习平台网络连接失败") {
        "learn_network"
    } else if error.contains("学习平台服务会话尚未建立") {
        "learn_prerequisite_missing"
    } else if error.contains("学习平台暂时不可用") {
        "learn_unavailable"
    } else if error.contains("教务平台网络连接失败") {
        "registrar_network"
    } else if error.contains("教务服务会话已过期") {
        "registrar_session_expired"
    } else if error.contains("教务服务暂时不可用") {
        "registrar_unavailable"
    } else if error.contains("INFO 服务会话已过期") {
        "info_session_expired"
    } else if error.contains("INFO 缺少 CSRF") {
        "info_missing_csrf"
    } else if error.contains("INFO Cookie 初始化响应异常") {
        "info_cookie_bootstrap_http"
    } else if error.contains("INFO Cookie 初始化路径异常") {
        "info_cookie_bootstrap_path"
    } else if error.contains("INFO 登录状态未确认") {
        "info_session_unconfirmed"
    } else if error.contains("INFO 跳转响应异常") {
        "info_handoff_http"
    } else if error.contains("INFO 跳转路径异常") {
        "info_handoff_path"
    } else if error.contains("INFO 跳转来源异常") {
        "info_unexpected_origin"
    } else if error.contains("INFO 新闻接口响应异常") {
        "info_news_http"
    } else if error.contains("INFO 新闻路径异常") {
        "info_news_path"
    } else if error.contains("INFO 新闻响应格式异常") {
        "info_news_parse"
    } else if error.contains("INFO 网络连接失败") {
        "info_network"
    } else if error.contains("INFO 服务暂时不可用") {
        "info_unavailable"
    } else if error.contains("图书馆服务会话已过期") {
        "library_session_expired"
    } else if error.contains("图书馆网络连接失败") {
        "library_network"
    } else if error.contains("图书馆服务暂时不可用") {
        "library_unavailable"
    } else if error.contains("教室服务会话已过期") {
        "classroom_session_expired"
    } else if error.contains("教室服务网络连接失败") {
        "classroom_network"
    } else if error.contains("教室服务暂时不可用") {
        "classroom_unavailable"
    } else if error.contains("电费服务会话已过期") {
        "electricity_session_expired"
    } else if error.contains("电费服务网络连接失败") {
        "electricity_network"
    } else if error.contains("电费服务暂时不可用") {
        "electricity_unavailable"
    } else if error.contains("校园卡服务会话已过期") {
        "campus_card_session_expired"
    } else if error.contains("校园卡登录页响应异常") {
        "campus_card_identity_page_http"
    } else if error.contains("校园卡登录页格式异常") {
        "campus_card_identity_page_format"
    } else if error.contains("校园卡统一认证网络失败") {
        "campus_card_identity_network"
    } else if error.contains("校园卡统一认证未返回服务跳转") {
        "campus_card_target_missing"
    } else if error.contains("校园卡登录状态未确认") {
        "campus_card_session_unconfirmed"
    } else if error.contains("校园卡账号绑定校验失败") {
        "campus_card_account_mismatch"
    } else if error.contains("校园卡服务响应异常") {
        "campus_card_http"
    } else if error.contains("校园卡跳转校验失败") {
        "campus_card_route"
    } else if error.contains("校园卡响应格式异常") {
        "campus_card_response"
    } else if error.contains("校园卡服务返回失败") {
        "campus_card_service_failure"
    } else if error.contains("校园卡网络连接失败") {
        "campus_card_network"
    } else if error.contains("校园卡服务暂时不可用") {
        "campus_card_unavailable"
    } else if error.contains("服务会话建立状态未确认") {
        "service_proof_missing"
    } else if error == "prerequisite_unavailable" || error.contains("became unavailable") {
        "prerequisite_unavailable"
    } else if error.contains("overview live inputs are incomplete") {
        "overview_inputs_incomplete"
    } else if error.contains("response") || error.contains("响应") {
        "response_unconfirmed"
    } else if error.contains("network") || error.contains("网络") || error.contains("transport") {
        "network"
    } else if error.contains("session") || error.contains("会话") || error.contains("登录") {
        "session"
    } else {
        "other"
    }
}

impl LiveValidationLedger {
    pub(crate) fn failure_summary(&self) -> Option<&'static str> {
        for (name, message) in [
            (
                "portal_bootstrap",
                "门户服务会话尚未建立，已保留其他服务会话",
            ),
            ("learn_courses", "网络学堂服务尚未通过会话证明"),
            ("learn_calendar", "网络学堂学期日历尚未通过实时读取验证"),
            ("school_calendar", "学校校历图片尚未通过实时读取验证"),
            ("learn_files", "网络学堂课程资料尚未通过完整读取验证"),
            ("learn_discussions", "网络学堂课程讨论尚未通过完整读取验证"),
            (
                "learn_file_download",
                "网络学堂资料下载响应尚未通过实时读取验证",
            ),
            ("learn_homework", "网络学堂课程作业尚未通过完整读取验证"),
            (
                "learn_homework_detail",
                "网络学堂课程作业详情尚未通过实时读取验证",
            ),
            ("thos_pending", "网上服务大厅待办尚未通过完整读取验证"),
            ("thos_completed", "网上服务大厅已办事项尚未通过完整读取验证"),
            ("thos_drafts", "网上服务大厅草稿尚未通过完整读取验证"),
            ("thos_unread", "网上服务大厅抄送待阅尚未通过完整读取验证"),
            ("thos_phases", "网上服务大厅阶段性事项尚未通过完整读取验证"),
            ("info_news", "INFO 服务尚未通过会话证明"),
            (
                "info_subscriptions",
                "INFO 新闻订阅规则尚未通过实时读取验证",
            ),
            ("info_favorites", "INFO 收藏新闻尚未通过完整分页验证"),
            (
                "info_subscription_feed",
                "INFO 订阅新闻尚未通过实时读取验证",
            ),
            ("registrar_schedule", "教务服务尚未通过课表读取证明"),
            (
                "campus_card_account",
                "校园卡服务会话尚未建立，其他服务不受影响",
            ),
        ] {
            if self.should_run(name)
                && self
                    .cases
                    .get(name)
                    .is_some_and(|case| case.status == "failed")
            {
                let reason = self.cases[name].reason.as_deref().unwrap_or_default();
                return Some(match reason {
                    "portal_resource_login_required" => {
                        "门户业务入口已返回登录页，当前恢复会话无法读取校园数据；其他会话已保留"
                    }
                    "portal_identity_entry_password_form"
                    | "portal_identity_target_password_form" => {
                        "门户认证入口要求交互登录，当前会话未取得目标服务授权；其他会话已保留"
                    }
                    _ => message,
                });
            }
        }
        if self
            .cases
            .iter()
            .any(|(name, case)| self.should_run(name) && case.status == "blocked")
        {
            return Some("部分后端验证被前置条件阻塞，尚未完成验收");
        }
        None
    }

    /// Mere restoration or a failed navigation must not renew/replace the
    /// last known resumable session. Only new verified success can commit.
    pub(crate) fn has_new_proof_since(&self, earlier: &Self) -> bool {
        self.cases.iter().any(|(name, result)| {
            result.status == "passed"
                && earlier
                    .cases
                    .get(name)
                    .is_none_or(|old| old.status != "passed")
        })
    }

    pub(crate) fn select_cases(&mut self, names: &str) -> Result<(), &'static str> {
        const KNOWN: &[&str] = &[
            "thos_pending",
            "thos_completed",
            "thos_drafts",
            "thos_unread",
            "thos_phases",
            "identity_session",
            "portal_bootstrap",
            "learn_courses",
            "learn_calendar",
            "school_calendar",
            "learn_todos",
            "learn_files",
            "learn_discussions",
            "learn_file_download",
            "learn_homework",
            "learn_homework_detail",
            "info_news",
            "info_subscriptions",
            "info_subscription_feed",
            "info_favorites",
            "registrar_schedule",
            "registrar_grades",
            "registrar_exams",
            "library_area_tree",
            "classroom_buildings",
            "electricity_remainder",
            "campus_card_account",
            "campus_card_transactions",
            "overview_live_inputs",
        ];
        let selected: BTreeSet<String> = names.split(',').map(str::to_owned).collect();
        if selected.is_empty() || selected.iter().any(|name| !KNOWN.contains(&name.as_str())) {
            return Err("live_case_selection_invalid");
        }
        self.selected = Some(selected);
        Ok(())
    }

    pub(crate) fn should_run(&self, name: &str) -> bool {
        if self
            .selected
            .as_ref()
            .is_some_and(|names| !names.contains(name))
        {
            return false;
        }
        !self
            .cases
            .get(name)
            .is_some_and(|case| case.status == "passed")
    }

    pub(crate) fn record(&mut self, name: &str, result: Result<(), String>) {
        if !self.should_run(name) {
            return;
        }
        let previous = self.cases.get(name).map_or(0, |case| case.attempts);
        let blocked = result.as_ref().err().is_some_and(|error| {
            matches!(
                error_reason(error),
                "prerequisite_unavailable" | "overview_inputs_incomplete"
            )
        });
        let (status, category, reason) = match result {
            Ok(()) => ("passed".to_owned(), None, None),
            Err(error) if blocked => (
                "blocked".to_owned(),
                Some("dependency".to_owned()),
                Some(error_reason(&error).to_owned()),
            ),
            Err(error) => (
                "failed".to_owned(),
                Some(error_category(&error).to_owned()),
                Some(error_reason(&error).to_owned()),
            ),
        };
        self.cases.insert(
            name.to_owned(),
            CaseResult {
                status,
                attempts: previous.saturating_add(u32::from(!blocked)),
                category,
                reason,
                updated_at: Utc::now(),
            },
        );
        self.updated_at = Utc::now();
    }
}

pub(crate) fn error_category(error: &str) -> &'static str {
    // The service's Chinese name contains “网络”; it must not turn parser,
    // account-binding or HTTP rejection errors into transport failures.
    let diagnostic = crate::telemetry::diagnostic_reason(error);
    if diagnostic.starts_with("usereg_") {
        return match diagnostic {
            "usereg_transport" | "usereg_rate_limited" | "usereg_http_unavailable" => "network",
            "usereg_http_auth_rejected"
            | "usereg_session_expired"
            | "usereg_session_unproven"
            | "usereg_account_mismatch" => "session",
            _ => "response",
        };
    }
    if error.contains("WebVPN 门户目标登录页状态未确认")
        || error.contains("WebVPN 门户凭证提交状态未确认")
    {
        return "session";
    }
    if error.contains("WebVPN 门户目标页状态未确认") {
        return "response";
    }
    if matches!(
        error,
        "campus_card_probe_invalid_json"
            | "campus_card_probe_invalid_envelope"
            | "campus_card_probe_result_missing"
            | "campus_card_probe_outer_failure"
    ) {
        return "response";
    }
    match error {
        "campus_card_cookie_rejected" | "campus_card_account_mismatch" => return "session",
        "campus_card_probe_network"
        | "campus_card_probe_unavailable"
        | "campus_card_rate_limited" => return "network",
        "campus_card_probe_account_missing"
        | "portal_resume_trusted_script_only"
        | "portal_resume_trusted_form_without_key"
        | "portal_resume_trusted_no_form"
        | "campus_card_encrypted_payload_format"
        | "campus_card_service_rejected"
        | "campus_card_probe_route"
        | "campus_card_probe_format"
        | "campus_card_probe_http" => return "response",
        _ => {}
    }
    if matches!(
        error,
        "portal_resource_login_required"
            | "portal_identity_entry_password_form"
            | "portal_identity_target_password_form"
            | "portal_resume_trusted_password_form"
    ) {
        return "session";
    }
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("网络")
        || normalized.contains("network")
        || normalized.contains("transport")
    {
        "network"
    } else if normalized.contains("会话")
        || normalized.contains("登录")
        || normalized.contains("session")
        || normalized.contains("auth")
    {
        "session"
    } else if normalized.contains("响应")
        || normalized.contains("parse")
        || normalized.contains("decode")
        || normalized.contains("format")
    {
        "response"
    } else if normalized.contains("未接入") || normalized.contains("unsupported") {
        "unsupported"
    } else {
        "other"
    }
}

#[cfg(all(debug_assertions, target_os = "macos", not(test)))]
fn base_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let marker = format!("/Library/Containers/{CONTAINER_ID}/Data");
    let base = if home.to_string_lossy().contains(&marker) {
        home
    } else {
        home.join("Library/Containers")
            .join(CONTAINER_ID)
            .join("Data")
    };
    Some(base.join("Library/Application Support/THYou"))
}

#[cfg(not(all(debug_assertions, target_os = "macos", not(test))))]
fn base_dir() -> Option<PathBuf> {
    None
}

pub(crate) fn enabled() -> bool {
    base_dir().is_some_and(|base| base.join(ENABLE_FILE).is_file())
}

/// Consume the opt-in before any request, so a crash/relaunch cannot silently
/// repeat failed authentication continuations. Only an explicit new marker
/// arms another batch; a successful case remains immutable in the ledger.
pub(crate) fn take_request() -> bool {
    base_dir().is_some_and(|base| std::fs::remove_file(base.join(ENABLE_FILE)).is_ok())
}

pub(crate) fn result_path() -> Option<PathBuf> {
    base_dir().map(|base| base.join(RESULT_FILE))
}

pub(crate) fn load() -> Result<LiveValidationLedger, &'static str> {
    let Some(path) = result_path() else {
        return Err("live_ledger_unavailable");
    };
    load_at(path)
}

pub(crate) fn load_at(path: PathBuf) -> Result<LiveValidationLedger, &'static str> {
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LiveValidationLedger {
                destination: Some(path),
                ..Default::default()
            });
        }
        Err(_) => return Err("live_ledger_unreadable"),
    };
    let mut ledger = decode_ledger(&bytes)?;
    ledger.destination = Some(path);
    Ok(ledger)
}

fn decode_ledger(bytes: &[u8]) -> Result<LiveValidationLedger, &'static str> {
    let ledger: LiveValidationLedger =
        serde_json::from_slice(bytes).map_err(|_| "live_ledger_invalid")?;
    if ledger.schema != SCHEMA
        || ledger.cases.values().any(|case| {
            !matches!(
                case.status.as_str(),
                "passed" | "failed" | "blocked" | "new" | "unknown"
            )
        })
    {
        return Err("live_ledger_invalid");
    }
    Ok(ledger)
}

pub(crate) fn save(ledger: &LiveValidationLedger) -> Result<(), String> {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = ledger
        .destination
        .clone()
        .or_else(result_path)
        .ok_or_else(|| String::from("live validation path unavailable"))?;
    let parent = path
        .parent()
        .ok_or_else(|| String::from("live validation path invalid"))?;
    fs::create_dir_all(parent)
        .map_err(|_| String::from("live validation directory unavailable"))?;
    #[cfg(unix)]
    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    let temporary = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(ledger)
        .map_err(|_| String::from("live validation result encoding failed"))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|_| String::from("live validation result file unavailable"))?;
    file.write_all(&encoded)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|_| String::from("live validation result write failed"))?;
    fs::rename(&temporary, &path)
        .map_err(|_| String::from("live validation result commit failed"))?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| String::from("live validation result permission failed"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_learn_calendar_live_selection_is_new_and_requires_current_read() {
        let mut ledger = LiveValidationLedger::default();
        ledger.select_cases("learn_calendar").unwrap();
        assert!(ledger.should_run("learn_calendar"));
        assert!(!ledger.should_run("learn_courses"));
        ledger.record(
            "learn_calendar",
            Err("validation_live_result_required".into()),
        );
        assert_eq!(ledger.cases["learn_calendar"].status, "failed");
        assert!(ledger.should_run("learn_calendar"));
    }

    #[test]
    fn backend_repair_school_calendar_live_selection_never_inherits_learn_calendar_pass() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record("learn_calendar", Ok(()));
        ledger.select_cases("school_calendar").unwrap();
        assert!(ledger.should_run("school_calendar"));
        assert!(!ledger.should_run("learn_calendar"));
        ledger.record("school_calendar", Err("school_calendar_unavailable".into()));
        assert_eq!(ledger.cases["school_calendar"].status, "failed");
    }

    #[test]
    fn backend_repair_learn_file_live_selection_does_not_pass_without_course_sample() {
        let mut ledger = LiveValidationLedger::default();
        ledger.select_cases("learn_files").unwrap();
        assert!(ledger.should_run("learn_files"));
        assert!(!ledger.should_run("learn_todos"));
        ledger.record("learn_files", Err("prerequisite_unavailable".into()));
        assert_eq!(ledger.cases["learn_files"].status, "blocked");
        assert!(ledger.should_run("learn_files"));
    }

    #[test]
    fn backend_repair_learn_file_failures_keep_specific_redacted_reasons() {
        for (message, reason) in [
            (
                "网络学堂课程资料数据格式未确认，请稍后重试",
                "learn_files_format",
            ),
            (
                "网络学堂课程资料来源路径未确认，已停止读取",
                "learn_files_route",
            ),
            (
                "网络学堂课程资料网络连接失败，请稍后重试",
                "learn_files_network",
            ),
            ("网络学堂课程资料请求失败，请稍后重试", "learn_files_http"),
        ] {
            assert_eq!(error_reason(message), reason);
        }
    }

    #[test]
    fn backend_repair_blocked_selected_case_cannot_be_reported_as_success() {
        let mut ledger = LiveValidationLedger::default();
        ledger.select_cases("library_area_tree").unwrap();
        ledger.record(
            "library_area_tree",
            Err("portal prerequisite became unavailable".into()),
        );
        assert_eq!(
            ledger.failure_summary(),
            Some("部分后端验证被前置条件阻塞，尚未完成验收")
        );
        assert!(!ledger.has_new_proof_since(&LiveValidationLedger::default()));
        ledger.select_cases("identity_session").unwrap();
        assert!(
            ledger.failure_summary().is_none(),
            "unselected evidence must not contaminate this batch"
        );
    }

    #[test]
    fn backend_repair_unexecuted_dependency_is_blocked_not_failed_attempt() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "learn_courses",
            Err("portal prerequisite became unavailable".into()),
        );
        let case = &ledger.cases["learn_courses"];
        assert_eq!(case.status, "blocked");
        assert_eq!(case.attempts, 0);
        assert_eq!(case.category.as_deref(), Some("dependency"));
        assert!(ledger.should_run("learn_courses"));
        assert!(decode_ledger(&serde_json::to_vec(&ledger).unwrap()).is_ok());
    }

    #[test]
    fn backend_repair_dependency_block_does_not_increment_real_failure_attempts() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record("learn_courses", Err("learning network failure".into()));
        ledger.record(
            "learn_courses",
            Err("portal prerequisite became unavailable".into()),
        );
        assert_eq!(ledger.cases["learn_courses"].attempts, 1);
        assert_eq!(ledger.cases["learn_courses"].status, "blocked");
        ledger.record("learn_courses", Ok(()));
        assert_eq!(ledger.cases["learn_courses"].attempts, 2);
        assert_eq!(ledger.cases["learn_courses"].status, "passed");
    }

    #[test]
    fn backend_repair_card_diagnostics_keep_network_distinct_from_cookie_rejection() {
        for (reason, category) in [
            ("campus_card_probe_network", "network"),
            ("campus_card_probe_unavailable", "network"),
            ("campus_card_rate_limited", "network"),
            ("campus_card_cookie_rejected", "session"),
            ("campus_card_account_mismatch", "session"),
            ("campus_card_probe_account_missing", "response"),
            ("campus_card_probe_route", "response"),
            ("campus_card_encrypted_payload_format", "response"),
        ] {
            assert_eq!(error_category(reason), category);
            assert_eq!(error_reason(reason), reason);
        }
    }

    #[test]
    fn backend_repair_validation_summary_keeps_root_auth_failure_and_excludes_unselected_services()
    {
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "portal_bootstrap",
            Err("portal_resource_login_required".to_owned()),
        );
        ledger.record("learn_courses", Err("prerequisite unavailable".to_owned()));
        ledger.record("campus_card_account", Err("session unconfirmed".to_owned()));
        assert_eq!(
            ledger.cases["portal_bootstrap"].category.as_deref(),
            Some("session")
        );
        assert_eq!(
            ledger.failure_summary(),
            Some("门户业务入口已返回登录页，当前恢复会话无法读取校园数据；其他会话已保留")
        );
        ledger.select_cases("campus_card_account").unwrap();
        assert_eq!(
            ledger.failure_summary(),
            Some("校园卡服务会话尚未建立，其他服务不受影响")
        );
    }

    #[test]
    fn backend_repair_failed_or_skipped_validation_cannot_commit_resume_snapshot() {
        let mut before = LiveValidationLedger::default();
        before.record("identity_session", Ok(()));
        let mut after = before.clone();
        assert!(!after.has_new_proof_since(&before));
        after.record(
            "portal_bootstrap",
            Err("portal_identity_target_password_form".to_owned()),
        );
        assert!(!after.has_new_proof_since(&before));
        after.record(
            "learn_courses",
            Err("portal prerequisite unavailable".to_owned()),
        );
        assert!(!after.has_new_proof_since(&before));
        after.record("identity_session", Ok(()));
        assert!(!after.has_new_proof_since(&before));
        after.record("portal_bootstrap", Ok(()));
        assert!(after.has_new_proof_since(&before));
        assert!(!after.has_new_proof_since(&after));
    }

    #[test]
    fn backend_repair_live_selection_never_runs_passed_or_unselected_cases() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record("identity_session", Ok(()));
        ledger.record("campus_card_account", Err("session".to_owned()));
        let card = ledger.cases["campus_card_account"].clone();
        assert!(
            ledger
                .select_cases("portal_bootstrap,identity_session")
                .is_ok()
        );
        assert!(ledger.should_run("portal_bootstrap"));
        assert!(!ledger.should_run("identity_session"));
        assert!(!ledger.should_run("campus_card_account"));
        ledger.record("campus_card_account", Ok(()));
        assert_eq!(ledger.cases["campus_card_account"], card);
        assert!(ledger.select_cases("unknown_case").is_err());
        let serialized = serde_json::to_string(&ledger).unwrap();
        assert!(!serialized.contains("selected"));
    }

    #[test]
    fn backend_repair_portal_resume_diagnostics_accept_only_fixed_stage_codes() {
        for stage in [
            "portal_resume_csrf_missing",
            "portal_resume_account_mismatch",
            "portal_resume_network",
            "portal_resume_account_format",
        ] {
            assert_eq!(error_reason(stage), stage);
        }
        assert_ne!(
            error_reason("portal_resume_account_format: private-server-content"),
            "portal_resume_account_format"
        );
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "portal_bootstrap",
            Err("portal_resume_account_mismatch".to_owned()),
        );
        assert_eq!(
            ledger.cases["portal_bootstrap"].reason.as_deref(),
            Some("portal_resume_account_mismatch")
        );
    }

    #[test]
    fn backend_repair_live_ledger_corruption_fails_closed() {
        assert!(decode_ledger(b"not-json").is_err());
        let mut ledger = LiveValidationLedger::default();
        ledger.record("portal_bootstrap", Ok(()));
        let bytes = serde_json::to_vec(&ledger).unwrap();
        assert!(
            !decode_ledger(&bytes)
                .unwrap()
                .should_run("portal_bootstrap")
        );
        ledger.schema = 999;
        assert!(decode_ledger(&serde_json::to_vec(&ledger).unwrap()).is_err());
        ledger.schema = SCHEMA;
        ledger.cases.get_mut("portal_bootstrap").unwrap().status = "PASSED".to_owned();
        assert!(decode_ledger(&serde_json::to_vec(&ledger).unwrap()).is_err());
    }

    #[test]
    fn backend_repair_passed_live_record_is_immutable() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record("portal_bootstrap", Ok(()));
        let passed = ledger.cases["portal_bootstrap"].clone();
        ledger.record("portal_bootstrap", Err("network unavailable".to_owned()));
        assert_eq!(ledger.cases["portal_bootstrap"], passed);
        ledger.record("portal_bootstrap", Ok(()));
        assert_eq!(ledger.cases["portal_bootstrap"], passed);
    }

    #[test]
    fn backend_repair_live_ledger_skips_only_passed_cases() {
        let mut ledger = LiveValidationLedger::default();
        assert!(ledger.should_run("learn"));
        ledger.record("learn", Ok(()));
        ledger.record("registrar", Err(String::from("network failure")));
        assert!(!ledger.should_run("learn"));
        assert!(ledger.should_run("registrar"));
        assert_eq!(ledger.cases["learn"].attempts, 1);
        assert_eq!(
            ledger.cases["registrar"].category.as_deref(),
            Some("network")
        );
        assert_eq!(ledger.cases["registrar"].reason.as_deref(), Some("network"));
    }

    #[test]
    fn backend_repair_live_ledger_contains_no_raw_failure_text() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "feature",
            Err(String::from(
                "response format invalid: private response content",
            )),
        );
        let encoded = serde_json::to_string(&ledger).unwrap();
        assert!(!encoded.contains("private response content"));
        assert!(encoded.contains("response"));
    }

    #[test]
    fn backend_repair_live_ledger_uses_fixed_safe_reason_codes() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "learn",
            Err(String::from("学习平台登录状态未确认，请稍后重试")),
        );
        ledger.record("info", Err(String::from("INFO 服务暂时不可用，请稍后重试")));
        assert_eq!(
            ledger.cases["learn"].reason.as_deref(),
            Some("learn_session_unconfirmed")
        );
        assert_eq!(
            ledger.cases["info"].reason.as_deref(),
            Some("info_unavailable")
        );
        let encoded = serde_json::to_string(&ledger).unwrap();
        assert!(!encoded.contains("登录状态未确认"));
        assert!(!encoded.contains("暂时不可用"));
    }

    #[test]
    fn backend_repair_live_ledger_distinguishes_info_and_card_root_failures() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record("info", Err(String::from("INFO 缺少 CSRF 信息，请稍后重试")));
        ledger.record(
            "card",
            Err(String::from("校园卡登录状态未确认，请稍后重试")),
        );
        assert_eq!(
            ledger.cases["info"].reason.as_deref(),
            Some("info_missing_csrf")
        );
        assert_eq!(
            ledger.cases["card"].reason.as_deref(),
            Some("campus_card_session_unconfirmed")
        );
    }

    #[test]
    fn backend_repair_fresh_portal_stage_reasons_are_distinct() {
        let mut ledger = LiveValidationLedger::default();
        for (case, message, reason) in [
            (
                "entry",
                "WebVPN 门户目标登录页状态未确认，请稍后重试",
                "portal_fresh_entry_unconfirmed",
            ),
            (
                "submit",
                "WebVPN 门户凭证提交状态未确认，请稍后重试",
                "portal_fresh_submit_unconfirmed",
            ),
            (
                "target",
                "WebVPN 门户目标页状态未确认，请稍后重试",
                "portal_fresh_target_unconfirmed",
            ),
        ] {
            ledger.record(case, Err(message.to_owned()));
            assert_eq!(ledger.cases[case].reason.as_deref(), Some(reason));
        }
        let encoded = serde_json::to_string(&ledger).unwrap();
        assert!(!encoded.contains("目标登录页状态未确认"));
        assert!(!encoded.contains("凭证提交状态未确认"));
        assert!(!encoded.contains("目标页状态未确认"));
    }

    #[test]
    fn backend_repair_live_ledger_distinguishes_portal_bootstrap_failures() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "portal_bootstrap",
            Err(String::from("WebVPN 门户 OAuth 跳转未确认，请稍后重试")),
        );
        assert_eq!(
            ledger.cases["portal_bootstrap"].reason.as_deref(),
            Some("portal_oauth_unconfirmed")
        );
        let encoded = serde_json::to_string(&ledger).unwrap();
        assert!(!encoded.contains("oauth.tsinghua.edu.cn"));
        assert!(!encoded.contains("ticket="));
    }

    #[test]
    fn backend_repair_live_ledger_distinguishes_card_identity_failures() {
        let mut ledger = LiveValidationLedger::default();
        ledger.record(
            "campus_card_account",
            Err(String::from("校园卡登录页格式异常，请稍后重试")),
        );
        assert_eq!(
            ledger.cases["campus_card_account"].reason.as_deref(),
            Some("campus_card_identity_page_format")
        );
        let encoded = serde_json::to_string(&ledger).unwrap();
        assert!(!encoded.contains("public key"));
        assert!(!encoded.contains("ticket="));
    }
}
