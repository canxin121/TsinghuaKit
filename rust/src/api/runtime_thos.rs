//! THOS target proof and short-lived presentation snapshot, owned by the one
//! application Runtime. No credentials, tickets or response bodies are cached.

use super::*;
use crate::{
    api::thos::{ThosPendingDto, ThosPhaseStepsDto, ThosServicesDto, ThosTaskListDto},
    thos::{ThosClient, ThosError, ThosListKind},
};

#[derive(Default)]
#[flutter_rust_bridge::frb(ignore)]
pub(super) struct ThosRuntimeState {
    owner: Option<UserIdentity>,
    transport: Option<crate::transport::CampusHttpTransport>,
    handoff_attempted: bool,
    snapshot: Option<(std::time::Instant, ThosPendingDto)>,
    services_snapshot: Option<(std::time::Instant, ThosServicesDto)>,
    task_lists: HashMap<String, (std::time::Instant, ThosTaskListDto)>,
    phase_selectors: HashMap<String, String>,
    phase_steps: HashMap<String, (std::time::Instant, ThosPhaseStepsDto)>,
}

#[flutter_rust_bridge::frb(ignore)]
impl ThosRuntimeState {
    fn bind(&mut self, user: &UserIdentity, transport: &crate::transport::CampusHttpTransport) {
        if self.owner.as_ref() != Some(user)
            || self
                .transport
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old.cookie_jar(), transport.cookie_jar()))
        {
            *self = Self {
                owner: Some(user.clone()),
                transport: Some(transport.clone()),
                ..Self::default()
            };
        }
    }

    pub(super) fn invalidate_snapshot(&mut self) {
        self.snapshot = None;
        self.services_snapshot = None;
        self.task_lists.clear();
        self.phase_selectors.clear();
        self.phase_steps.clear();
        // A failed/consumed handoff remains consumed until the login boundary.
    }
}

#[flutter_rust_bridge::frb(ignore)]
pub(super) async fn read(
    runtime: &mut CampusRuntime,
    force_refresh: bool,
) -> Result<ThosPendingDto, String> {
    runtime.allow_live_operation()?;
    let user = runtime
        .ensure_identity_user_for_live_read()
        .await
        .map_err(|_| "请先完成统一认证后读取网上服务大厅待办".to_owned())?;
    runtime
        .ensure_info_reader_session(&user)
        .await
        .map_err(|_| "网上服务大厅所需的门户会话未建立，请重试".to_owned())?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Info)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            != Some(&user)
    {
        return Err("网上服务大厅账号会话未确认，请重新登录".into());
    }
    let info = runtime
        .info_adapter
        .as_ref()
        .ok_or("网上服务大厅缺少门户会话")?;
    let transport = runtime.identity.transport().clone();
    if !Arc::ptr_eq(info.transport().cookie_jar(), transport.cookie_jar()) {
        return Err("网上服务大厅账号会话未确认，请重新登录".into());
    }
    runtime.thos.bind(&user, &transport);
    if !force_refresh {
        if let Some((at, value)) = &runtime.thos.snapshot {
            if at.elapsed() < Duration::from_secs(60) {
                let mut value = value.clone();
                value.source = "cache".into();
                return Ok(value);
            }
        }
    }
    // A failed refresh must not leave a previously live-looking memory result.
    runtime.thos.invalidate_snapshot();
    let client = ThosClient::new(&info.config().webvpn_base_url, transport)
        .map_err(|e| e.message().to_owned())?;
    let counts = match client.counts().await {
        Ok(counts) => counts,
        Err(ThosError::Session) if !runtime.thos.handoff_attempted => {
            // Mark before await: cancellation or an ambiguous response cannot
            // be replayed by a second widget or automatic refresh.
            runtime.thos.handoff_attempted = true;
            info.additional_roaming(crate::thos::ROAM_ID)
                .await
                .map_err(|_| "网上服务大厅会话续接未确认，请重新建立登录会话".to_owned())?;
            client.counts().await.map_err(|e| e.message().to_owned())?
        }
        Err(error) => return Err(error.message().into()),
    };
    // Counts are the target application's business proof. INFO proof alone
    // never authorizes a fabricated empty THOS result.
    let result = client
        .pending(counts)
        .await
        .map_err(|e| e.message().to_owned())?;
    runtime.allow_live_operation()?;
    if result.complete {
        runtime.thos.snapshot = Some((std::time::Instant::now(), result.clone()));
    }
    runtime.persist_resume_state_after_live_read(&user, "info");
    Ok(result)
}

#[flutter_rust_bridge::frb(ignore)]
pub(super) async fn read_services(
    runtime: &mut CampusRuntime,
    force_refresh: bool,
) -> Result<ThosServicesDto, String> {
    runtime.allow_live_operation()?;
    let user = runtime
        .ensure_identity_user_for_live_read()
        .await
        .map_err(|_| "请先完成统一认证后读取网上服务大厅服务目录".to_owned())?;
    runtime
        .ensure_info_reader_session(&user)
        .await
        .map_err(|_| "网上服务大厅所需的门户会话未建立，请重试".to_owned())?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Info)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            != Some(&user)
    {
        return Err("网上服务大厅账号会话未确认，请重新登录".into());
    }
    let info = runtime
        .info_adapter
        .as_ref()
        .ok_or("网上服务大厅缺少门户会话")?;
    let transport = runtime.identity.transport().clone();
    if !Arc::ptr_eq(info.transport().cookie_jar(), transport.cookie_jar()) {
        return Err("网上服务大厅账号会话未确认，请重新登录".into());
    }
    runtime.thos.bind(&user, &transport);
    if !force_refresh {
        if let Some((at, value)) = &runtime.thos.services_snapshot {
            if at.elapsed() < Duration::from_secs(60) {
                let mut value = value.clone();
                value.source = "cache".into();
                return Ok(value);
            }
        }
    }
    runtime.thos.services_snapshot = None;
    let client = ThosClient::new(&info.config().webvpn_base_url, transport)
        .map_err(|e| e.message().to_owned())?;
    let _counts = match client.counts().await {
        Ok(counts) => counts,
        Err(ThosError::Session) if !runtime.thos.handoff_attempted => {
            runtime.thos.handoff_attempted = true;
            info.additional_roaming(crate::thos::ROAM_ID)
                .await
                .map_err(|_| "网上服务大厅会话续接未确认，请重新建立登录会话".to_owned())?;
            client.counts().await.map_err(|e| e.message().to_owned())?
        }
        Err(error) => return Err(error.message().into()),
    };
    let result = client.services().await.map_err(|error| {
        if error == ThosError::Response {
            "网上服务大厅服务目录数据格式未确认，请稍后重试".to_owned()
        } else {
            error.message().to_owned()
        }
    })?;
    runtime.allow_live_operation()?;
    if result.complete {
        runtime.thos.services_snapshot = Some((std::time::Instant::now(), result.clone()));
    }
    runtime.persist_resume_state_after_live_read(&user, "info");
    Ok(result)
}

#[flutter_rust_bridge::frb(ignore)]
pub(super) async fn read_task_list(
    runtime: &mut CampusRuntime,
    kind: String,
    force_refresh: bool,
) -> Result<ThosTaskListDto, String> {
    let kind = ThosListKind::parse(&kind)
        .map_err(|_| "网上服务大厅事项类别未确认，已停止读取".to_owned())?;
    runtime.allow_live_operation()?;
    let user = runtime
        .ensure_identity_user_for_live_read()
        .await
        .map_err(|_| "请先完成统一认证后读取网上服务大厅事项".to_owned())?;
    runtime
        .ensure_info_reader_session(&user)
        .await
        .map_err(|_| "网上服务大厅所需的门户会话未建立，请重试".to_owned())?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Info)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            != Some(&user)
    {
        return Err("网上服务大厅账号会话未确认，请重新登录".into());
    }
    let info = runtime
        .info_adapter
        .as_ref()
        .ok_or("网上服务大厅缺少门户会话")?;
    let transport = runtime.identity.transport().clone();
    if !Arc::ptr_eq(info.transport().cookie_jar(), transport.cookie_jar()) {
        return Err("网上服务大厅账号会话未确认，请重新登录".into());
    }
    runtime.thos.bind(&user, &transport);
    if !force_refresh {
        if let Some((at, value)) = runtime.thos.task_lists.get(kind.key()) {
            if at.elapsed() < Duration::from_secs(60) {
                let mut value = value.clone();
                value.source = "cache".into();
                return Ok(value);
            }
        }
    }
    runtime.thos.task_lists.remove(kind.key());
    if kind == ThosListKind::Phases {
        runtime.thos.phase_selectors.clear();
        runtime.thos.phase_steps.clear();
    }
    let client = ThosClient::new(&info.config().webvpn_base_url, transport)
        .map_err(|error| error.message().to_owned())?;
    let _counts = match client.counts().await {
        Ok(counts) => counts,
        Err(ThosError::Session) if !runtime.thos.handoff_attempted => {
            runtime.thos.handoff_attempted = true;
            info.additional_roaming(crate::thos::ROAM_ID)
                .await
                .map_err(|_| "网上服务大厅会话续接未确认，请重新建立登录会话".to_owned())?;
            client
                .counts()
                .await
                .map_err(|error| error.message().to_owned())?
        }
        Err(error) => return Err(error.message().into()),
    };
    let read = client
        .task_list_with_selectors(kind)
        .await
        .map_err(|error| {
            if error == ThosError::Response {
                "网上服务大厅事项数据格式未确认，请稍后重试".to_owned()
            } else {
                error.message().to_owned()
            }
        })?;
    let result = read.dto;
    runtime.allow_live_operation()?;
    if result.complete {
        if kind == ThosListKind::Phases {
            runtime.thos.phase_selectors = read.phase_selectors;
        }
        runtime.thos.task_lists.insert(
            kind.key().into(),
            (std::time::Instant::now(), result.clone()),
        );
    }
    runtime.persist_resume_state_after_live_read(&user, "info");
    Ok(result)
}

#[flutter_rust_bridge::frb(ignore)]
pub(super) async fn read_phase_steps(
    runtime: &mut CampusRuntime,
    task_id: String,
    force_refresh: bool,
) -> Result<ThosPhaseStepsDto, String> {
    runtime.allow_live_operation()?;
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.ensure_info_reader_session(&user).await?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Info)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            != Some(&user)
    {
        return Err("网上服务大厅阶段步骤账号会话未确认".into());
    }
    let info = runtime
        .info_adapter
        .as_ref()
        .ok_or("网上服务大厅缺少门户会话")?;
    let transport = runtime.identity.transport().clone();
    if !Arc::ptr_eq(info.transport().cookie_jar(), transport.cookie_jar()) {
        return Err("网上服务大厅阶段步骤账号会话未确认".into());
    }
    runtime.thos.bind(&user, &transport);
    let list_ready = runtime
        .thos
        .task_lists
        .get(ThosListKind::Phases.key())
        .is_some_and(|(at, list)| at.elapsed() < Duration::from_secs(60) && list.complete);
    let aggregate_id = if list_ready {
        runtime.thos.phase_selectors.get(&task_id).cloned()
    } else {
        None
    }
    .ok_or("请先刷新完整的阶段性事项列表，再读取步骤")?;
    if !force_refresh {
        if let Some((at, value)) = runtime.thos.phase_steps.get(&task_id) {
            if at.elapsed() < Duration::from_secs(60) {
                let mut cached = value.clone();
                cached.source = "cache".into();
                return Ok(cached);
            }
        }
    }
    runtime.thos.phase_steps.remove(&task_id);
    let client = ThosClient::new(&info.config().webvpn_base_url, transport)
        .map_err(|error| error.message().to_owned())?;
    match client.counts().await {
        Ok(_) => {}
        Err(ThosError::Session) if !runtime.thos.handoff_attempted => {
            runtime.thos.handoff_attempted = true;
            info.additional_roaming(crate::thos::ROAM_ID)
                .await
                .map_err(|_| "网上服务大厅会话续接未确认，请重新建立登录会话".to_owned())?;
            client
                .counts()
                .await
                .map_err(|error| error.message().to_owned())?;
        }
        Err(error) => return Err(error.message().into()),
    }
    let steps = client.phase_steps(&aggregate_id).await.map_err(|error| {
        if error == ThosError::Response {
            "网上服务大厅阶段步骤数据格式未确认，请稍后重试".to_owned()
        } else {
            error.message().to_owned()
        }
    })?;
    runtime.allow_live_operation()?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Info)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            != Some(&user)
    {
        return Err("网上服务大厅阶段步骤账号会话未确认".into());
    }
    let result = ThosPhaseStepsDto {
        task_id: task_id.clone(),
        steps,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    };
    runtime
        .thos
        .phase_steps
        .insert(task_id, (std::time::Instant::now(), result.clone()));
    runtime.persist_resume_state_after_live_read(&user, "info");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_thos_memory_snapshot_is_account_and_transport_bound() {
        let first = UserIdentity {
            username: "fixture-first".into(),
            display_name: None,
        };
        let second = UserIdentity {
            username: "fixture-second".into(),
            display_name: None,
        };
        let transport = crate::transport::CampusHttpTransport::new("fixture").unwrap();
        let mut state = ThosRuntimeState::default();
        state.bind(&first, &transport);
        state.handoff_attempted = true;
        state.bind(&first, &transport.clone());
        assert!(state.handoff_attempted);
        state.invalidate_snapshot();
        assert!(state.handoff_attempted);
        state.bind(&second, &transport);
        assert!(!state.handoff_attempted && state.snapshot.is_none());
        state.handoff_attempted = true;
        state.bind(
            &second,
            &crate::transport::CampusHttpTransport::new("other-fixture").unwrap(),
        );
        assert!(!state.handoff_attempted && state.snapshot.is_none());
    }
}
