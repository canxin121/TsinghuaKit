//! Recovery bookkeeping shared by normal readers, cache misses and services.
//! Strings are presentation only. Retry permission depends on the fixed error
//! category AND a transport witness proving no one-shot dispatch occurred.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryFailureKind {
    RetryableProbe,
    InteractionRequired,
    LoginRequired,
    Unconfirmed,
}

// Default is internal recovery bookkeeping, never a public bridge factory.
#[cfg_attr(feature = "ffi-bridge", frb(ignore))]
#[derive(Clone, Default)]
pub(super) struct RecoveryLifecycle {
    pub retry_at: HashMap<ServiceId, std::time::Instant>,
    pub failure_kinds: HashMap<ServiceId, RecoveryFailureKind>,
    pub awaiting_business: HashSet<ServiceId>,
    pub target_credentials: HashSet<ServiceSecondFactorTarget>,
    pub target_handoffs: HashSet<ServiceSecondFactorTarget>,
    pub login_required: bool,
    pub login_reason: Option<&'static str>,
    pub resume_retry_at: Option<std::time::Instant>,
    pub resume_failures: u32,
}

pub(super) fn transient_proof_failure(reason: &str) -> bool {
    // Only closed, Rust-owned codes/messages. Unknown failures never grant
    // replay permission, and an unsafe dispatch witness overrides this list.
    matches!(
        reason,
        "THYou 私有凭证库暂时不可用，请稍后重试"
            | "portal_resume_network"
            | "portal_resume_cookie_http"
            | "portal_resume_account_http"
            | "portal_resource_http"
            | "portal_resume_rate_limited"
            | "portal_resume_sso_network"
            | "portal_resume_sso_http"
            | "portal_resume_trusted_network"
            | "portal_resume_trusted_unconfirmed"
            | "campus_card_probe_network"
            | "campus_card_probe_unavailable"
            | "campus_card_rate_limited"
    )
}

impl CampusRuntime {
    pub(crate) fn set_credential_store_root(&mut self, root: std::path::PathBuf) {
        self.credential_store_root = root;
    }

    pub(crate) fn set_credential_store_keys(
        &mut self,
        identity: Option<zeroize::Zeroizing<[u8; 32]>>,
        self_service: Option<zeroize::Zeroizing<[u8; 32]>>,
    ) {
        self.identity_credential_store_key = identity;
        self.self_service_credential_store_key = self_service;
    }

    pub(super) fn load_credential_for_current_authority(
        &self,
        username: &str,
    ) -> Result<
        Option<crate::credential_store::StoredCredential>,
        crate::credential_store::CredentialStoreError,
    > {
        let load = || {
            if let Some(key) = self.identity_credential_store_key.as_ref() {
                crate::credential_store::load_at_root_with_host_key(
                    &self.credential_store_root,
                    username,
                    &self.fingerprint,
                    key,
                )
            } else {
                crate::credential_store::load_at_root(
                    &self.credential_store_root,
                    username,
                    &self.fingerprint,
                )
            }
        };
        if !self.persist_sessions {
            return load();
        }
        let lease = self
            .recovery_lease
            .as_ref()
            .ok_or(crate::credential_store::CredentialStoreError::Backend)?;
        lease
            .with_current(|| Ok(load()))
            .map_err(|_| crate::credential_store::CredentialStoreError::Backend)?
    }

    pub(super) fn save_credential_for_current_authority(
        &self,
        username: &str,
        password: &str,
        stage: Option<AcademicStage>,
    ) -> Result<(), crate::credential_store::CredentialStoreError> {
        let save = || {
            if let Some(key) = self.identity_credential_store_key.as_ref() {
                crate::credential_store::save_at_root_with_host_key(
                    &self.credential_store_root,
                    username,
                    password,
                    stage,
                    &self.fingerprint,
                    self.stage_selection_explicit,
                    key,
                )
            } else {
                crate::credential_store::save_at_root_with_stage_selection(
                    &self.credential_store_root,
                    username,
                    password,
                    stage,
                    &self.fingerprint,
                    self.stage_selection_explicit,
                )
            }
        };
        if !self.persist_sessions {
            return save();
        }
        let lease = self
            .recovery_lease
            .as_ref()
            .ok_or(crate::credential_store::CredentialStoreError::Backend)?;
        lease
            .with_current(|| Ok(save()))
            .map_err(|_| crate::credential_store::CredentialStoreError::Backend)?
    }

    pub(super) fn clear_credential_for_current_authority(
        &self,
        username: &str,
    ) -> Result<(), crate::credential_store::CredentialStoreError> {
        let clear =
            || crate::credential_store::clear_at_root(&self.credential_store_root, username);
        if !self.persist_sessions {
            return clear();
        }
        let lease = self
            .recovery_lease
            .as_ref()
            .ok_or(crate::credential_store::CredentialStoreError::Backend)?;
        lease
            .with_current(|| Ok(clear()))
            .map_err(|_| crate::credential_store::CredentialStoreError::Backend)?
    }

    pub(crate) fn load_saved_self_service_credentials(
        &self,
        username: &str,
    ) -> Result<
        Option<crate::credential_store::StoredCredential>,
        crate::credential_store::CredentialStoreError,
    > {
        if let Some(key) = self.self_service_credential_store_key.as_ref() {
            crate::credential_store::load_self_service_at_root_with_host_key(
                &self.credential_store_root,
                username,
                &self.fingerprint,
                key,
            )
        } else {
            crate::credential_store::load_self_service_at_root(
                &self.credential_store_root,
                username,
                &self.fingerprint,
            )
        }
    }

    pub(crate) fn save_self_service_credential(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(), crate::credential_store::CredentialStoreError> {
        if let Some(key) = self.self_service_credential_store_key.as_ref() {
            crate::credential_store::save_self_service_at_root_with_host_key(
                &self.credential_store_root,
                username,
                password,
                &self.fingerprint,
                key,
            )
        } else {
            crate::credential_store::save_self_service_at_root(
                &self.credential_store_root,
                username,
                password,
                &self.fingerprint,
            )
        }
    }

    pub(crate) fn clear_saved_self_service_credentials(
        &self,
        username: &str,
    ) -> Result<(), crate::credential_store::CredentialStoreError> {
        crate::credential_store::clear_self_service_at_root(&self.credential_store_root, username)
    }

    pub(super) fn saved_credential_opt_in_matches(&self, user: &UserIdentity) -> bool {
        self.remember_credentials
            && self.credential_username.as_deref() == Some(user.username.as_str())
    }

    /// Called only after the target returned a validated password form. Each
    /// target owns its own one-shot credential boundary; a failed Card login
    /// must neither replay itself nor consume a future Electricity login.
    pub(super) fn load_target_credential(
        &mut self,
        user: &UserIdentity,
        target: ServiceSecondFactorTarget,
    ) -> Result<String, String> {
        if !self.saved_credential_opt_in_matches(user) {
            return Err(self.require_identity_login("统一认证会话已过期，请重新登录"));
        }
        if !self.recovery.target_credentials.insert(target) {
            return Err("服务会话自动续接未完成，请重新建立服务会话".to_owned());
        }
        match self.load_credential_for_current_authority(&user.username) {
            Ok(Some(stored))
                if stored.stage.is_none_or(|stage| {
                    !self.stage_context_available_for_cache() || stage == self.stage
                }) =>
            {
                Ok(stored.password)
            }
            Err(crate::credential_store::CredentialStoreError::Backend) => {
                self.recovery.target_credentials.remove(&target);
                Err(self.record_error("THYou 私有凭证库暂时不可用，请稍后重试"))
            }
            _ => Err(self.require_identity_login("统一认证会话已过期，请重新登录")),
        }
    }

    pub(super) fn service_recovery_context_exists(&self, service: ServiceId) -> bool {
        self.automatic_refresh_attempted.contains(&service)
            || self.automatic_refresh_results.contains_key(&service)
            || self.recovery.awaiting_business.contains(&service)
    }

    pub(super) fn reopen_due_probe(&mut self, service: ServiceId) {
        if self
            .recovery
            .retry_at
            .get(&service)
            .is_some_and(|at| *at <= std::time::Instant::now())
        {
            self.clear_automatic_refresh_state(service);
            self.recovery.retry_at.remove(&service);
            self.recovery.failure_kinds.remove(&service);
            // A later user read is a fresh proof attempt, not a replay of the
            // old request. This path never opens after an auth/handoff POST.
            self.portal_failure = None;
        }
    }

    pub(super) fn record_recovery_result(
        &mut self,
        service: ServiceId,
        result: &Result<(), String>,
        witness: crate::transport::ReplayFence,
    ) {
        if result.is_ok() && self.service_session_is_proven(service) {
            self.clear_automatic_refresh_state(service);
            if service != ServiceId::Identity {
                self.recovery.awaiting_business.insert(service);
            }
            return;
        }
        // A child reader may be waiting for a new primary-login bootstrap.
        // That bootstrap recorded its own witness after replacing transport;
        // preserve its safe retry deadline instead of sealing the child forever.
        if result
            .as_ref()
            .err()
            .is_some_and(|error| error == reference_recovery::RETRY_NOTICE)
            && let Some(deadline) = self.recovery.resume_retry_at
        {
            self.recovery.retry_at.insert(service, deadline);
            self.recovery
                .failure_kinds
                .insert(service, RecoveryFailureKind::RetryableProbe);
            self.automatic_refresh_results
                .insert(service, result.clone());
            return;
        }
        let kind = if self.status().state == "requires_second_factor" {
            RecoveryFailureKind::InteractionRequired
        } else if self.recovery.login_required {
            RecoveryFailureKind::LoginRequired
        } else if result
            .as_ref()
            .err()
            .is_some_and(|e| transient_proof_failure(e))
            && witness.permits_probe_retry(self.identity.transport())
        {
            self.recovery
                .retry_at
                .insert(service, std::time::Instant::now() + Duration::from_secs(60));
            RecoveryFailureKind::RetryableProbe
        } else {
            RecoveryFailureKind::Unconfirmed
        };
        self.recovery.failure_kinds.insert(service, kind);
        self.automatic_refresh_results
            .insert(service, result.clone());
    }

    pub(super) fn invalidate_pending_business_recovery(&mut self, service: ServiceId) {
        if self.recovery.awaiting_business.remove(&service) {
            self.recovery
                .failure_kinds
                .insert(service, RecoveryFailureKind::Unconfirmed);
            self.recovery.retry_at.remove(&service);
            self.automatic_refresh_attempted.insert(service);
            self.automatic_refresh_results.insert(
                service,
                Err("服务会话自动续接后仍已过期，请重新建立服务会话".to_owned()),
            );
        }
    }

    pub(super) fn finish_business_recovery(&mut self, name: &str) {
        let services: &[ServiceId] = match name {
            "learn" => &[ServiceId::Learn],
            "registrar" => &[ServiceId::Registrar],
            "info" | "classroom" | "electricity" => &[ServiceId::Info],
            "library" => &[ServiceId::Library, ServiceId::Info],
            "campus_card" => &[ServiceId::CampusCard],
            "overview" => &[ServiceId::Learn, ServiceId::Registrar],
            _ => &[],
        };
        for &service in services {
            if self.service_session_is_proven(service) {
                self.recovery.awaiting_business.remove(&service);
                self.clear_automatic_refresh_state(service);
            }
        }
    }

    pub(super) fn release_completed_interaction(&mut self) {
        if self.current_service_second_factor().is_some()
            || !self.service_session_is_proven(ServiceId::Identity)
        {
            return;
        }
        let paused = self
            .recovery
            .failure_kinds
            .iter()
            .filter_map(|(service, kind)| {
                (*kind == RecoveryFailureKind::InteractionRequired).then_some(*service)
            })
            .collect::<Vec<_>>();
        for service in paused {
            self.clear_automatic_refresh_state(service);
            self.recovery.awaiting_business.remove(&service);
        }
        self.portal_failure = None;
    }

    /// End online authentication without deleting account-scoped offline data
    /// or treating an outage as a password rejection. Only an explicit login
    /// can clear this terminal boundary; a saved opt-in is not a retry loop.
    pub(super) fn require_identity_login(&mut self, message: &'static str) -> String {
        self.recovery.login_required = true;
        self.recovery.login_reason = Some(message);
        self.identity
            .transport()
            .disable_cookie_snapshot_persistence();
        let state = self
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .state;
        if matches!(
            state,
            ServiceSessionState::Authenticated | ServiceSessionState::Authenticating
        ) {
            let _ = self.coordinator.registry_mut().transition(
                ServiceId::Identity,
                crate::session::SessionTransition::Expire,
            );
        }
        self.restored_session = false;
        self.primary_password = None;
        self.portal_bootstrapped = false;
        self.portal_csrf = None;
        self.last_error = Some(message.to_owned());
        message.to_owned()
    }
}
