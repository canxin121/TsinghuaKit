//! Scheduling for safe, pre-authentication recovery failures. This module
//! never reads credentials or dispatches requests; it only manages deadlines.
use super::*;

pub(super) const RETRY_NOTICE: &str = "连接暂时不可用，稍后自动重试";

impl CampusRuntime {
    pub(super) fn schedule_safe_login_probe_retry(&mut self) {
        let shift = self.recovery.resume_failures.min(3);
        let seconds = (60_u64 << shift).min(300);
        self.recovery.resume_failures = self.recovery.resume_failures.saturating_add(1);
        self.recovery.resume_retry_at =
            Some(std::time::Instant::now() + Duration::from_secs(seconds));
        self.resume_attempt = ResumeAttemptState::Consumed;
        self.last_error = Some(RETRY_NOTICE.to_owned());
    }

    /// Only a deadline created by a classified, non-consuming failure may
    /// release a consumed credential attempt. Elapsed wall time alone never
    /// releases an ambiguous POST or one-shot handoff.
    pub(super) fn reopen_safe_login_probe_if_due(&mut self) {
        if self
            .recovery
            .resume_retry_at
            .is_some_and(|at| at <= std::time::Instant::now())
        {
            self.resume_attempt = ResumeAttemptState::Ready;
            self.recovery.resume_retry_at = None;
            self.credential_recovery_attempted = false;
            self.private_credential_attempted = false;
            self.portal_failure = None;
            self.clear_automatic_refresh_state(ServiceId::Identity);
        }
    }

    pub(super) fn login_probe_is_cooling_down(&self) -> bool {
        self.recovery
            .resume_retry_at
            .is_some_and(|at| at > std::time::Instant::now())
    }

    /// Safe presentation hint. None means no automatic retry is authorized,
    /// not that the user is authenticated or that a credential was invalid.
    pub(super) fn recovery_retry_delay(&self) -> Option<u32> {
        let status = self.status();
        if status.username.is_none()
            || matches!(
                status.state.as_str(),
                "signed_out" | "authenticating" | "requires_second_factor"
            )
            || self.recovery.login_required
            || self.allow_live_operation().is_err()
            || self.cache_user_for_read().is_err()
        {
            return None;
        }
        let deadline = self
            .recovery
            .resume_retry_at
            .or_else(|| self.recovery.retry_at.get(&ServiceId::Identity).copied());
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let rounded = remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0));
            return Some(rounded.clamp(1, 300) as u32);
        }
        (self.resume_attempt == ResumeAttemptState::RetryableLocalFailure).then_some(60)
    }

    pub(super) fn finish_primary_recovery_cycle(&mut self) {
        self.recovery.resume_retry_at = None;
        self.recovery.resume_failures = 0;
        self.credential_recovery_attempted = false;
        self.clear_automatic_refresh_state(ServiceId::Identity);
        // Unlock only readers that were waiting for this pre-auth probe.
        // An independently failed/ambiguous target handoff stays blocked.
        let waiting = self
            .automatic_refresh_results
            .iter()
            .filter_map(|(service, result)| {
                result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error == RETRY_NOTICE)
                    .then_some(*service)
            })
            .collect::<Vec<_>>();
        for service in waiting {
            self.clear_automatic_refresh_state(service);
        }
    }
}
