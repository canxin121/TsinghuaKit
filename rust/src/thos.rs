//! Read-only THOS task lists, independently implemented from THU Info's
//! thos-services.ts protocol. Every request uses the caller's shared transport.

use std::collections::{HashMap, HashSet};

use reqwest::{StatusCode, Url, header::LOCATION};
use serde_json::{Map, Value, json};

use crate::{
    api::thos::{
        ThosPendingDto, ThosPhaseStepDto, ThosPhaseStepItemDto, ThosServiceDto, ThosServicesDto,
        ThosTaskDto, ThosTaskListDto,
    },
    transport::CampusHttpTransport,
};

pub(crate) const ROAM_ID: &str = "56B13DDF68BB3DEA13D98E1E3E776D3E";
pub(crate) const MAPPING: &str =
    "77726476706e69737468656265737421e4ff4e8f69247b59700f81b9991b2631ca359dd4";
const COUNTS: &str = "/fp/fp/formHome/allNum";
const TODO: &str = "/fp/fp/taskcenter/getDBSXList";
const ACTIVE: &str = "/fp/fp/myserviceapply/getZBSXList";
const SERVICES: &str = "/fp/fp/formHome/AllSvsByConditionpage";
const COMPLETED: &str = "/fp/fp/myserviceapply/getBJSXList";
const DRAFTS: &str = "/fp/fp/draft/pageDraft";
const UNREAD: &str = "/fp/fp/carboncopy/getDYSXList";
const PHASES: &str = "/fp/fp/aggregation/getAggItemList";
const PHASE_STEPS: &str = "/fp/aggregation/getActWork";
const MAX_PAGES: u32 = 100;
const MAX_BODY: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThosError {
    Session,
    Network,
    Http,
    Route,
    Response,
    Business,
}

impl ThosError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Session => "网上服务大厅会话已失效，请重新建立登录会话",
            Self::Network => "网上服务大厅网络连接失败，请稍后重试",
            Self::Http => "网上服务大厅请求失败，请稍后重试",
            Self::Route => "网上服务大厅返回了未允许的地址，已停止读取",
            Self::Response => "网上服务大厅待办数据格式未确认，请稍后重试",
            Self::Business => "网上服务大厅未能完成待办查询，请稍后重试",
        }
    }
}

#[derive(Clone)]
pub(crate) struct ThosClient {
    base: Url,
    transport: CampusHttpTransport,
}

#[derive(Clone, Copy)]
pub(crate) struct ThosCounts {
    pub todo: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Task {
    process: String,
    key: String,
    actionable: bool,
    dto: ThosTaskDto,
}

struct PageSet {
    tasks: Vec<Task>,
    total: u32,
    complete: bool,
}

pub(crate) struct TaskListRead {
    pub dto: ThosTaskListDto,
    pub phase_selectors: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThosListKind {
    Completed,
    Drafts,
    Unread,
    Phases,
}

impl ThosListKind {
    pub(crate) fn parse(value: &str) -> Result<Self, ThosError> {
        match value {
            "completed" => Ok(Self::Completed),
            "drafts" => Ok(Self::Drafts),
            "unread" => Ok(Self::Unread),
            "phases" => Ok(Self::Phases),
            _ => Err(ThosError::Route),
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Drafts => "drafts",
            Self::Unread => "unread",
            Self::Phases => "phases",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::Completed => COMPLETED,
            Self::Drafts => DRAFTS,
            Self::Unread => UNREAD,
            Self::Phases => PHASES,
        }
    }

    fn page_size(self) -> usize {
        match self {
            Self::Completed => 50,
            Self::Drafts | Self::Unread | Self::Phases => 10,
        }
    }
}

impl ThosClient {
    pub(crate) fn new(webvpn: &Url, transport: CampusHttpTransport) -> Result<Self, ThosError> {
        let mut base = webvpn.clone();
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || (!crate::request_gate::is_loopback(&base)
                && (base.scheme() != "https"
                    || base.host_str() != Some("webvpn.tsinghua.edu.cn")
                    || base.port().is_some()))
        {
            return Err(ThosError::Route);
        }
        base.set_path(&format!("/https/{MAPPING}"));
        Ok(Self { base, transport })
    }

    fn endpoint(&self, path: &str) -> Result<Url, ThosError> {
        if ![
            COUNTS,
            TODO,
            ACTIVE,
            SERVICES,
            COMPLETED,
            DRAFTS,
            UNREAD,
            PHASES,
            PHASE_STEPS,
        ]
        .contains(&path)
        {
            return Err(ThosError::Route);
        }
        let mut url = self.base.clone();
        url.set_path(&format!("{}{path}", self.base.path()));
        Ok(url)
    }

    pub(crate) async fn counts(&self) -> Result<ThosCounts, ThosError> {
        parse_counts(&self.read(COUNTS, json!({})).await?)
    }

    async fn read(&self, path: &str, params: Value) -> Result<Value, ThosError> {
        let url = self.endpoint(path)?;
        let request = self
            .transport
            .client()
            .post(url.clone())
            .header("Content-Type", "application/json;charset=utf-8")
            .body(params.to_string())
            .build()
            .map_err(|_| ThosError::Route)?;
        // Read APIs must not replay their POST at a redirected endpoint.
        // execute_once still enters CampusHttpTransport's shared request gate.
        let response = self
            .transport
            .execute_once(self.transport.client(), request)
            .await
            .map_err(|_| ThosError::Network)?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(ThosError::Session);
        }
        if let Some(location) = response.headers().get(LOCATION) {
            let target = location
                .to_str()
                .ok()
                .and_then(|s| url.join(s).ok())
                .ok_or(ThosError::Route)?;
            let target_class = classify_read_redirect(&target, &self.base);
            // Only a fixed classification enters the redacted log. Never
            // emit Location, including its possible ticket or query values.
            tracing::warn!(target: "tsinghua_kit::security", event = "thos_read_redirect",
                thos_target = target_class, http_status = status.as_u16());
            return Err(
                if status.is_redirection()
                    && matches!(
                        target_class,
                        "identity_login"
                            | "webvpn_home"
                            | "webvpn_login"
                            | "webvpn_identity_login"
                            | "mapped_thos"
                            | "direct_thos"
                    )
                {
                    // A redirect from a read API is an unproved target session.
                    // Do not follow or replay the POST at Location. The runtime
                    // may perform its one bounded, pinned THOS handoff instead.
                    ThosError::Session
                } else {
                    ThosError::Route
                },
            );
        }
        if response.url() != &url {
            return Err(ThosError::Route);
        }
        if !status.is_success() {
            return Err(ThosError::Http);
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(ThosError::Response);
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ThosError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
                return Err(ThosError::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_body(std::str::from_utf8(&bytes).map_err(|_| ThosError::Response)?)
    }

    async fn pages(&self, todo: bool) -> Result<PageSet, ThosError> {
        let mut tasks: Vec<Task> = Vec::new();
        let mut positions = HashMap::<String, usize>::new();
        let mut first_total = None;
        let mut total = 0;
        let mut stable = true;
        for page in 1..=MAX_PAGES {
            let value = self
                .read(if todo { TODO } else { ACTIVE }, page_params(todo, page))
                .await?;
            let (rows, count) = parse_page(&value, page)?;
            total = count;
            if let Some(first) = first_total {
                stable &= first == total;
            } else {
                first_total = Some(total);
            }
            let previous = tasks.len();
            for value in rows {
                let task = parse_task(value, todo)?;
                if let Some(index) = positions.get(&task.key).copied() {
                    // Conflicting snapshots must never choose one arbitrarily.
                    if tasks[index] != task {
                        return Err(ThosError::Response);
                    }
                } else {
                    positions.insert(task.key.clone(), tasks.len());
                    tasks.push(task);
                }
            }
            if tasks.len() >= total as usize {
                let complete = stable && tasks.len() == total as usize;
                return Ok(PageSet {
                    tasks,
                    total,
                    complete,
                });
            }
            if tasks.len() == previous {
                break;
            }
        }
        Ok(PageSet {
            tasks,
            total,
            complete: false,
        })
    }

    pub(crate) async fn pending(&self, counts: ThosCounts) -> Result<ThosPendingDto, ThosError> {
        // Both sets are needed: returned applications may not be in getDBSXList.
        let todo = self.pages(true).await?;
        let active = self.pages(false).await?;
        let complete = todo.complete && active.complete && todo.total == counts.todo;
        let processes: HashSet<_> = todo.tasks.iter().map(|t| t.process.clone()).collect();
        let mut items: Vec<_> = todo.tasks.into_iter().map(|t| t.dto).collect();
        items.extend(
            active
                .tasks
                .into_iter()
                .filter(|t| t.actionable && !processes.contains(&t.process))
                .map(|t| t.dto),
        );
        Ok(ThosPendingDto {
            items,
            reported_todo_count: counts.todo,
            complete,
            generated_at: chrono::Utc::now().to_rfc3339(),
            source: "live".into(),
            status: if complete { "ready" } else { "partial" }.into(),
            error: (!complete).then(|| "待办列表尚未完整读取或查询期间发生变化，请刷新核对".into()),
        })
    }

    pub(crate) async fn services(&self) -> Result<ThosServicesDto, ThosError> {
        let mut items = Vec::<ThosServiceDto>::new();
        let mut positions = HashMap::<String, usize>::new();
        let mut first_total = None;
        let mut total = 0;
        let mut stable = true;
        for page in 1..=MAX_PAGES {
            let value = self
                .read(
                    SERVICES,
                    json!({
                        "pageNum": page, "pageSize": 100, "project_id": "",
                        "category_ids": "", "firstCharacter": "", "unit_id": "",
                        "orderBy": "defaultAsc", "isCollect": "all", "isRecommend": "no"
                    }),
                )
                .await?;
            let (rows, count) = parse_page_with_limit(&value, page, 100)?;
            total = count;
            if let Some(first) = first_total {
                stable &= first == count;
            } else {
                first_total = Some(count);
            }
            let previous = items.len();
            for value in rows {
                let item = parse_service(value)?;
                if let Some(index) = positions.get(&item.id).copied() {
                    if items[index] != item {
                        return Err(ThosError::Response);
                    }
                } else {
                    positions.insert(item.id.clone(), items.len());
                    items.push(item);
                }
            }
            if items.len() >= total as usize {
                let complete = stable && items.len() == total as usize;
                return Ok(service_result(items, total, complete));
            }
            if items.len() == previous {
                break;
            }
        }
        Ok(service_result(items, total, false))
    }

    pub(crate) async fn task_list(&self, kind: ThosListKind) -> Result<ThosTaskListDto, ThosError> {
        self.task_list_with_selectors(kind)
            .await
            .map(|result| result.dto)
    }

    pub(crate) async fn task_list_with_selectors(
        &self,
        kind: ThosListKind,
    ) -> Result<TaskListRead, ThosError> {
        let mut items = Vec::<Task>::new();
        let mut positions = HashMap::<String, usize>::new();
        let mut first_total = None;
        let mut total = 0;
        let mut stable = true;
        for page in 1..=MAX_PAGES {
            let value = self
                .read(kind.endpoint(), list_page_params(kind, page))
                .await?;
            let (rows, count) = parse_page_with_limit(&value, page, kind.page_size())?;
            total = count;
            if let Some(first) = first_total {
                stable &= first == count;
            } else {
                first_total = Some(count);
            }
            let previous = items.len();
            for value in rows {
                let item = parse_list_task(value, kind)?;
                if let Some(index) = positions.get(&item.key).copied() {
                    if items[index] != item {
                        return Err(ThosError::Response);
                    }
                } else {
                    positions.insert(item.key.clone(), items.len());
                    items.push(item);
                }
            }
            if items.len() >= total as usize {
                let complete = stable && items.len() == total as usize;
                return Ok(task_list_read(kind, items, total, complete));
            }
            if items.len() == previous {
                break;
            }
        }
        Ok(task_list_read(kind, items, total, false))
    }

    pub(crate) async fn phase_steps(
        &self,
        aggregate_id: &str,
    ) -> Result<Vec<ThosPhaseStepDto>, ThosError> {
        if aggregate_id.is_empty()
            || aggregate_id.len() > 512
            || aggregate_id.chars().any(char::is_control)
        {
            return Err(ThosError::Route);
        }
        let value = self
            .read(PHASE_STEPS, json!({ "agg_proc_id": aggregate_id }))
            .await?;
        parse_phase_steps(&value)
    }
}

fn task_list_read(
    kind: ThosListKind,
    items: Vec<Task>,
    total: u32,
    complete: bool,
) -> TaskListRead {
    let phase_selectors = if kind == ThosListKind::Phases && complete {
        items
            .iter()
            .map(|item| (item.dto.id.clone(), item.process.clone()))
            .collect()
    } else {
        HashMap::new()
    };
    TaskListRead {
        dto: task_list_result(kind, items, total, complete),
        phase_selectors,
    }
}

fn task_list_result(
    kind: ThosListKind,
    items: Vec<Task>,
    total: u32,
    complete: bool,
) -> ThosTaskListDto {
    ThosTaskListDto {
        kind: kind.key().into(),
        items: items.into_iter().map(|item| item.dto).collect(),
        reported_total: total,
        complete,
        generated_at: chrono::Utc::now().to_rfc3339(),
        source: "live".into(),
        status: if complete { "ready" } else { "partial" }.into(),
        error: (!complete).then(|| "在线服务事项尚未完整读取或查询期间发生变化，请刷新核对".into()),
    }
}

fn service_result(items: Vec<ThosServiceDto>, total: u32, complete: bool) -> ThosServicesDto {
    ThosServicesDto {
        items,
        reported_total: total,
        complete,
        generated_at: chrono::Utc::now().to_rfc3339(),
        source: "live".into(),
        status: if complete { "ready" } else { "partial" }.into(),
        error: (!complete).then(|| "在线服务目录尚未完整读取或查询期间发生变化，请刷新核对".into()),
    }
}

fn known_login_target(url: &Url, base: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let path = url.path().to_ascii_lowercase();
    (url.scheme() == "https"
        && url.port().is_none()
        && matches!(
            url.host_str(),
            Some("id.tsinghua.edu.cn" | "oauth.tsinghua.edu.cn")
        )
        && (path.starts_with("/do/off/ui/auth/") || path.starts_with("/oauth2/")))
        || (crate::webvpn_url::same_origin(base, url)
            && (path == "/login"
                || path == "/login/"
                || path == format!("{}/fp/login", base.path())))
}

fn classify_read_redirect(target: &Url, base: &Url) -> &'static str {
    let mut target = target.clone();
    let mapped_home = format!("/https/{MAPPING}/fp/view");
    if target.fragment() == Some("act=fp/formHome")
        && target.query() == Some("m=fp")
        && (target.path() == "/fp/view" || target.path() == mapped_home)
    {
        target.set_fragment(None);
    }
    if target.fragment().is_some() || !target.username().is_empty() || target.password().is_some() {
        return "unsafe_url";
    }
    if known_login_target(&target, base) {
        return "identity_login";
    }
    let path = target.path();
    if path.contains(['%', '\\']) {
        return "unsafe_url";
    }
    if crate::webvpn_url::same_origin(base, &target) {
        if path == "/" {
            // A mapped WebVPN resource may bounce to the gateway entry when
            // its target cookie is absent. Only the pinned handoff may run.
            return "webvpn_home";
        }
        if matches!(
            path,
            "/f/login" | "/wengine-vpn/login" | "/wengine-vpn/cookie"
        ) {
            return "webvpn_login";
        }
        if crate::portal_resume::mapped_identity_login_path(&target) {
            return "webvpn_identity_login";
        }
        if crate::portal_resume::mapped_identity_dynamic_login_form_path(&target) {
            return "webvpn_identity_login";
        }
        if crate::portal_resume::inside_identity_webvpn_mapping(&target) {
            return "webvpn_identity_mapping_other";
        }
        let prefix = format!("/https/{MAPPING}/fp/");
        if path.starts_with(&prefix)
            && crate::webvpn_url::redirect_path_stays_in_mapping(&target, "https", MAPPING)
        {
            return "mapped_thos";
        }
        let thos_root = format!("/https/{MAPPING}");
        if path == thos_root || path == format!("{thos_root}/") {
            return "webvpn_thos_root";
        }
        if path.starts_with(&format!("{thos_root}/")) {
            return "webvpn_thos_other";
        }
        if path.starts_with("/https/") {
            let Some((_, route)) = path.trim_start_matches("/https/").split_once('/') else {
                return "webvpn_other_mapping_root";
            };
            if route.is_empty() {
                return "webvpn_other_mapping_root";
            }
            if route.starts_with("fp/") {
                return "webvpn_other_mapping_fp";
            }
            if route.starts_with("do/off/ui/auth/") {
                return "webvpn_other_mapping_identity";
            }
            if route.starts_with("b/yyfw/") || route.starts_with("b/info/") {
                return "webvpn_other_mapping_info";
            }
            return "webvpn_other_mapping_route";
        }
        if path.starts_with("/http/") || path.starts_with("/http-") {
            return "webvpn_other_protocol";
        }
        return "webvpn_other_route";
    }
    if target.scheme() == "https"
        && target.host_str() == Some("thos.tsinghua.edu.cn")
        && target.port().is_none()
    {
        if path.starts_with("/fp/") {
            return "direct_thos";
        }
        return "direct_thos_other";
    }
    "foreign_origin"
}

/// The only extra navigation allowed for this selector is the THOS /fp/
/// application. Validate every hop before dispatch, and never submit a form.
pub(crate) async fn follow_handoff(
    transport: &CampusHttpTransport,
    webvpn: &Url,
    target: &str,
) -> Result<Url, ThosError> {
    let mut url = Url::parse(target).map_err(|_| ThosError::Route)?;
    let prefix = format!("/https/{MAPPING}/fp/");
    let mut seen = HashSet::new();
    for _ in 0..8 {
        // THU Info's THOS_HOME is /fp/view?m=fp#act=fp/formHome.
        // Its fragment is a browser route, never part of an HTTP request.
        // Accept only that known homepage route before applying the usual
        // fragment-free origin and path checks; arbitrary fragments stay errors.
        if url.fragment() == Some("act=fp/formHome")
            && url.path() == format!("{prefix}view")
            && url.query() == Some("m=fp")
        {
            url.set_fragment(None);
        }
        if !crate::webvpn_url::same_origin(webvpn, &url)
            || !url.path().starts_with(&prefix)
            // THOS handoff paths are literal ASCII application routes.
            // Encoded separators/dot segments must not escape /fp/ while
            // remaining within the broader WebVPN host mapping.
            || url.path().contains(['%', '\\'])
            || !crate::webvpn_url::redirect_path_stays_in_mapping(&url, "https", MAPPING)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !seen.insert(url.to_string())
        {
            return Err(ThosError::Route);
        }
        let request = transport
            .client()
            .get(url.clone())
            .build()
            .map_err(|_| ThosError::Route)?;
        let response = transport
            .execute_once(transport.client(), request)
            .await
            .map_err(|_| ThosError::Network)?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(ThosError::Session);
        }
        if response.status().is_redirection() {
            let target = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| url.join(v).ok())
                .ok_or(ThosError::Route)?;
            if known_login_target(&target, webvpn) {
                return Err(ThosError::Session);
            }
            url = target;
            continue;
        }
        if !response.status().is_success() {
            return Err(ThosError::Http);
        }
        // Navigation success alone is not service proof. The caller must
        // immediately validate allNum on this same Cookie jar and account.
        return Ok(url);
    }
    Err(ThosError::Route)
}

fn login_message(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "请先登录",
        "请重新登录",
        "未登录",
        "登录失效",
        "登录已失效",
        "会话已过期",
        "session expired",
        "login required",
        "not logged in",
        "unauthorized",
    ]
    .iter()
    .any(|s| lower.contains(s))
}

fn parse_body(body: &str) -> Result<Value, ThosError> {
    if body.trim_start().starts_with('<') {
        let lower = body.to_ascii_lowercase();
        return Err(
            if login_message(body)
                || lower.contains("<title>登录</title>")
                || lower.contains("name=\"i_user\"")
                || lower.contains("name='i_user'")
                || lower.contains("/do/off/ui/auth/login")
            {
                ThosError::Session
            } else {
                ThosError::Response
            },
        );
    }
    let value: Value = serde_json::from_str(body).map_err(|_| ThosError::Response)?;
    if value.is_array() {
        // The pinned phase-step endpoint returns a top-level array. Object
        // readers still reject it through their own required-field checks.
        return Ok(value);
    }
    let object = value.as_object().ok_or(ThosError::Response)?;
    if ["msg", "message", "error"].iter().any(|k| {
        object
            .get(*k)
            .and_then(Value::as_str)
            .is_some_and(login_message)
    }) || ["code", "status"]
        .iter()
        .any(|k| object.get(*k).is_some_and(|v| v == 401 || v == "401"))
    {
        return Err(ThosError::Session);
    }
    if object.get("success") == Some(&Value::Bool(false))
        || object
            .get("result")
            .and_then(Value::as_str)
            .is_some_and(|s| matches!(s, "error" | "failure" | "failed"))
        || object
            .get("code")
            .is_some_and(|v| !matches!(v.as_str(), Some("0" | "200")) && v != 0 && v != 200)
    {
        return Err(ThosError::Business);
    }
    Ok(value)
}

fn number(object: &Map<String, Value>, key: &str) -> Result<u32, ThosError> {
    match object.get(key) {
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|n| n.try_into().ok())
            .ok_or(ThosError::Response),
        Some(Value::String(s))
            if !s.trim().is_empty() && s.trim().bytes().all(|b| b.is_ascii_digit()) =>
        {
            s.trim().parse().map_err(|_| ThosError::Response)
        }
        _ => Err(ThosError::Response),
    }
}

fn parse_counts(value: &Value) -> Result<ThosCounts, ThosError> {
    let object = value.as_object().ok_or(ThosError::Response)?;
    number(object, "zbNum")?;
    Ok(ThosCounts {
        todo: number(object, "AuditSvsNum")?,
    })
}

fn parse_page(value: &Value, page: u32) -> Result<(&Vec<Value>, u32), ThosError> {
    parse_page_with_limit(value, page, 50)
}

fn parse_page_with_limit(
    value: &Value,
    page: u32,
    limit: usize,
) -> Result<(&Vec<Value>, u32), ThosError> {
    let object = value.as_object().ok_or(ThosError::Response)?;
    if object.contains_key("pageNum") && number(object, "pageNum")? != page {
        return Err(ThosError::Response);
    }
    let rows = object
        .get("list")
        .and_then(Value::as_array)
        .ok_or(ThosError::Response)?;
    if rows.len() > limit {
        return Err(ThosError::Response);
    }
    Ok((rows, number(object, "total")?))
}

fn phase_state(raw: &str) -> &'static str {
    match raw {
        "0" => "未开始",
        "1" => "正在办理",
        "4" => "已完成",
        "5" => "办理失败",
        _ => "状态未知（服务返回）",
    }
}

fn parse_phase_steps(value: &Value) -> Result<Vec<ThosPhaseStepDto>, ThosError> {
    let rows = value.as_array().ok_or(ThosError::Response)?;
    if rows.len() > 200 {
        return Err(ThosError::Response);
    }
    let mut total_items = 0usize;
    rows.iter()
        .enumerate()
        .map(|(index, raw)| {
            let object = raw.as_object().ok_or(ThosError::Response)?;
            let name = text(object, &["ACT_NAME", "act_name"])?;
            let order = text(object, &["ACT_ORDER_ID", "act_order_id"])?;
            let order = if order.is_empty() {
                (index + 1).to_string()
            } else if order.len() <= 32 {
                order
            } else {
                return Err(ThosError::Response);
            };
            if name.is_empty() || name.len() > 512 {
                return Err(ThosError::Response);
            }
            let item_rows = object
                .get("itemInfo")
                .and_then(Value::as_array)
                .ok_or(ThosError::Response)?;
            total_items = total_items.saturating_add(item_rows.len());
            if item_rows.len() > 200 || total_items > 2000 {
                return Err(ThosError::Response);
            }
            let items = item_rows
                .iter()
                .map(|raw_item| {
                    let item = raw_item.as_object().ok_or(ThosError::Response)?;
                    let name = text(item, &["ITEM_NAME", "item_name"])?;
                    if name.is_empty() || name.len() > 512 {
                        return Err(ThosError::Response);
                    }
                    Ok(ThosPhaseStepItemDto {
                        name,
                        state: phase_state(&text(item, &["ITEM_STATE", "item_state"])?).into(),
                    })
                })
                .collect::<Result<Vec<_>, ThosError>>()?;
            Ok(ThosPhaseStepDto {
                order,
                name,
                state: phase_state(&text(object, &["ACT_STATE", "act_state"])?).into(),
                items,
            })
        })
        .collect()
}

fn parse_service(value: &Value) -> Result<ThosServiceDto, ThosError> {
    let object = value.as_object().ok_or(ThosError::Response)?;
    let id = text(object, &["ID", "SERVICE_ID"])?;
    let name = text(object, &["NAME", "SERVICE_NAME"])?;
    if id.is_empty() || name.is_empty() {
        return Err(ThosError::Response);
    }
    let kind = match text(object, &["UW_TYPE", "TYPE"])?.as_str() {
        "0" => Some("form"),
        "1" => Some("guide"),
        "2" => Some("integration"),
        "3" | "6" => Some("group"),
        _ => None,
    };
    let in_open_period = match text(object, &["IS_TIME_VALID"])?.as_str() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    };
    Ok(ThosServiceDto {
        id,
        name,
        department: text(object, &["UNIT_NAME", "UNIT_SHORT_NAME"])?,
        kind: kind.map(str::to_owned),
        in_open_period,
    })
}

fn text(object: &Map<String, Value>, keys: &[&str]) -> Result<String, ThosError> {
    for key in keys {
        let value = match object.get(*key) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) => s.trim().to_owned(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err(ThosError::Response),
        };
        if value.is_empty() {
            continue;
        }
        if value.len() > 8192 || value.chars().any(char::is_control) {
            return Err(ThosError::Response);
        }
        return Ok(value);
    }
    Ok(String::new())
}

fn parse_task(value: &Value, todo: bool) -> Result<Task, ThosError> {
    let object = value.as_object().ok_or(ThosError::Response)?;
    let process = text(
        object,
        &["proc_inst_id", "procinst_id", "PROC_INST_ID", "PROCINST_ID"],
    )?;
    let title = text(object, &["service_name", "SERVICE_NAME"])?;
    if process.is_empty() || title.is_empty() {
        return Err(ThosError::Response);
    }
    let task = text(object, &["task_id", "WORKITEM_INS_ID", "TASK_ID"])?;
    // JSON tuple avoids delimiter collisions and never exposes process/task ids to Flutter.
    let key = serde_json::to_string(&(&process, if todo { task.as_str() } else { "" }))
        .map_err(|_| ThosError::Response)?;
    let returned = text(object, &["BUTSTATUS"])? == "1";
    let actionable = returned || text(object, &["APPROVE"])? == "1";
    let raw = text(object, &["SCHEDULE"])?;
    let progress = if raw.is_empty() {
        None
    } else {
        // THU Info treats absent/sentinel progress as unknown. It is optional
        // presentation metadata, not evidence that the workflow is complete.
        raw.trim_end_matches('%')
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
            .map(|n| n.floor() as u32)
    };
    Ok(Task {
        dto: ThosTaskDto {
            id: crate::learn_todos::stable_uuid("thos-task", &key).to_string(),
            title,
            status: if returned {
                "退回修改"
            } else if todo || actionable {
                "待我处理"
            } else {
                "正在办理"
            }
            .into(),
            node: text(object, &["CURRENT_ACT_NAME", "task_name", "ACT_NAME"])?,
            date: text(
                object,
                &["complete_time", "apply_time", "start_time", "START_TIME"],
            )?,
            progress,
        },
        process,
        key,
        actionable,
    })
}

fn parse_list_task(value: &Value, kind: ThosListKind) -> Result<Task, ThosError> {
    let object = value.as_object().ok_or(ThosError::Response)?;
    let (process, title, status, node, date, task_id, progress) = match kind {
        ThosListKind::Completed => {
            let state = text(object, &["current_state", "CURRENT_STATE"])?;
            let status = match state.as_str() {
                "4" => "办理成功",
                "5" => "办理失败",
                "9" => "已撤回",
                _ => "已办结",
            };
            (
                text(
                    object,
                    &["proc_inst_id", "procinst_id", "PROC_INST_ID", "PROCINST_ID"],
                )?,
                text(object, &["service_name", "SERVICE_NAME"])?,
                status,
                text(object, &["CURRENT_ACT_NAME", "task_name", "ACT_NAME"])?,
                text(
                    object,
                    &["complete_time", "apply_time", "start_time", "START_TIME"],
                )?,
                String::new(),
                parse_task_progress(&text(object, &["SCHEDULE"])?)?,
            )
        }
        ThosListKind::Drafts => (
            text(object, &["processInstId", "process_inst_id", "procinst_id"])?,
            text(object, &["draftName", "service_name", "SERVICE_NAME"])?,
            "草稿",
            String::new(),
            text(object, &["modifyTime", "startTime"])?,
            String::new(),
            None,
        ),
        ThosListKind::Unread => {
            let read_state = text(object, &["READSTATE", "readState"])?;
            (
                text(object, &["procinst_id", "proc_inst_id", "PROC_INST_ID"])?,
                text(object, &["service_name", "SERVICE_NAME"])?,
                if read_state.is_empty() {
                    "未阅"
                } else {
                    "已阅"
                },
                text(object, &["curActName", "CURRENT_ACT_NAME", "task_name"])?,
                text(object, &["apply_time", "start_time", "START_TIME"])?,
                text(object, &["task_id", "WORKITEM_INS_ID", "TASK_ID"])?,
                None,
            )
        }
        ThosListKind::Phases => {
            let state = text(object, &["STATE", "state"])?;
            let current = optional_number(object, &["NUMS"])?;
            let total = optional_number(object, &["SUMTEMP"])?;
            let progress = match (current, total) {
                (Some(current), Some(total)) if total > 0 && current <= total => {
                    Some(((current as u64 * 100) / total as u64) as u32)
                }
                _ => None,
            };
            (
                text(object, &["AGG_PROC_ID", "agg_proc_id"])?,
                text(object, &["NAME", "name"])?,
                match state.as_str() {
                    "1" => "正在办理",
                    "4" => "办理成功",
                    "5" => "办理失败",
                    _ => "状态未知",
                },
                match (current, total) {
                    (Some(current), Some(total)) => format!("当前进度 {current}/{total}"),
                    _ => String::new(),
                },
                String::new(),
                String::new(),
                progress,
            )
        }
    };
    if process.is_empty() || title.is_empty() {
        return Err(ThosError::Response);
    }
    let key = serde_json::to_string(&(process.as_str(), task_id.as_str()))
        .map_err(|_| ThosError::Response)?;
    let stable_key = format!("{}:{key}", kind.key());
    Ok(Task {
        dto: ThosTaskDto {
            id: crate::learn_todos::stable_uuid("thos-task", &stable_key).to_string(),
            title,
            status: status.into(),
            node,
            date,
            progress,
        },
        process,
        key,
        actionable: false,
    })
}

fn optional_number(object: &Map<String, Value>, keys: &[&str]) -> Result<Option<u32>, ThosError> {
    let raw = text(object, keys)?;
    if raw.is_empty() {
        Ok(None)
    } else if raw.bytes().all(|b| b.is_ascii_digit()) {
        raw.parse::<u32>()
            .map(Some)
            .map_err(|_| ThosError::Response)
    } else {
        Err(ThosError::Response)
    }
}

fn parse_task_progress(raw: &str) -> Result<Option<u32>, ThosError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let raw = raw.strip_suffix('%').unwrap_or(raw);
    match raw.parse::<f64>() {
        Ok(value) if value.is_finite() && (0.0..=100.0).contains(&value) => {
            Ok(Some(value.floor() as u32))
        }
        _ => Ok(None), // Upstream progress is optional presentation metadata.
    }
}

fn list_page_params(kind: ThosListKind, page: u32) -> Value {
    let mut params = json!({
        "pageNum": if kind == ThosListKind::Completed { json!(page) } else { json!(page.to_string()) },
        "pageSize": if kind == ThosListKind::Completed { json!(50) } else { json!("10") },
    });
    let fields: &[&str] = match kind {
        ThosListKind::Completed => &[
            "serviceName",
            "startTime",
            "endTime",
            "assess",
            "result",
            "completestart_time",
            "completeend_time",
            "procinst_id",
            "summary",
        ],
        ThosListKind::Drafts => &["draftName", "processInstId", "draftSummary"],
        ThosListKind::Unread => &[
            "service_name",
            "start_date",
            "end_date",
            "apply_name",
            "unit_name",
            "procinst_id",
            "summary",
            "readresult",
            "bjstart_date",
            "bjend_date",
            "result",
        ],
        ThosListKind::Phases => &["name", "agg_proc_id", "state"],
    };
    let object = params.as_object_mut().expect("fixed object");
    for field in fields {
        object.insert((*field).into(), Value::String(String::new()));
    }
    params
}

fn page_params(todo: bool, page: u32) -> Value {
    let mut params = json!({ "pageNum": page, "pageSize": 50 });
    let object = params.as_object_mut().expect("fixed object");
    let fields: &[&str] = if todo {
        &[
            "service_name",
            "job_number",
            "unit_name",
            "procinst_id",
            "currentNode",
            "summary",
            "start_date",
            "end_date",
        ]
    } else {
        &[
            "serviceName",
            "startTime",
            "endTime",
            "procinst_id",
            "summary",
            "currentNode",
        ]
    };
    for field in fields {
        object.insert((*field).into(), Value::String(String::new()));
    }
    if todo {
        object.insert("status".into(), Value::String("1".into()));
    }
    params
}

#[cfg(test)]
#[path = "thos_tests.rs"]
mod tests;
