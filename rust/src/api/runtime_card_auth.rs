//! Card target-password continuation, not a new primary login. Public keys
//! and current-account context are short lived; secrets never enter a DTO.
use super::*;

pub(super) struct CardPasswordBoundary {
    user: UserIdentity,
    public_key: String,
    card_origin: Url,
    created_at: std::time::Instant,
}
impl CardPasswordBoundary {
    pub(super) fn new(user: UserIdentity, public_key: String, card_origin: Url) -> Self {
        Self {
            user,
            public_key,
            card_origin,
            created_at: std::time::Instant::now(),
        }
    }
}

impl CampusRuntime {
    pub(super) fn has_terminal_card_password_boundary(&self) -> bool {
        self.terminal_interactive_auth
            && !self.persist_sessions
            && self.pending_card_password.is_some()
    }

    pub(super) fn cancel_terminal_card_password(&mut self) {
        self.pending_card_password = None;
    }

    pub(super) async fn complete_terminal_card_password(
        &mut self,
        password: String,
    ) -> Result<(), String> {
        // Consume before validation or I/O. Declined, stale, malformed and
        // ambiguous attempts cannot open a second automatic submission.
        let boundary = self
            .pending_card_password
            .take()
            .ok_or("campus_card_password_boundary_missing")?;
        if !self.terminal_interactive_auth
            || self.persist_sessions
            || password.is_empty()
            || password.len() > 4096
            || boundary.created_at.elapsed() > Duration::from_secs(120)
            || !self.service_session_is_proven(ServiceId::Identity)
            || self
                .coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .user
                .as_ref()
                != Some(&boundary.user)
            || self.configured_card_origin()? != boundary.card_origin
        {
            return self.fail("campus_card_password_boundary_invalid");
        }
        let handoff = self
            .submit_card_target_password(&boundary.user, password, boundary.public_key)
            .await?;
        self.finish_card_handoff(&boundary.user, handoff).await?;
        self.last_error = None;
        Ok(())
    }

    pub(super) async fn submit_card_target_password(
        &mut self,
        user: &UserIdentity,
        password: String,
        public_key: String,
    ) -> Result<Url, String> {
        self.allow_new_identity_handoff()?;
        let identity_client = self.identity.identity().client().clone();
        let card_origin = self.configured_card_origin()?;
        let wire_password = encrypt_password(&password, &public_key).map_err(|error| {
            self.record_card_session_error(format!("campus card identity crypto: {error}"))
        })?;
        // Card uses the same five-field service-roam contract as the
        // portal reference. Primary-login/device-registration options
        // must not leak into a separate target-app credential request.
        let input = portal_roam_login_input(&user.username, &wire_password, &self.fingerprint);
        let response = self
            .identity
            .identity()
            .submit_login_for_app_id(
                CAMPUS_CARD_SSO_TARGET.split('/').next().unwrap_or_default(),
                input,
            )
            .await
            .map_err(|error| {
                self.record_card_session_error(format!("campus card identity login: {error}"))
            })?;

        let response_page = identity_client.parse_login_page(response.body());
        if response_page.second_factor_marker.is_some() {
            self.pause_for_service_second_factor(ServiceSecondFactorTarget::CampusCard)
                .await?;
            return Err("campus_card_factor_pending".to_owned());
        }
        if response_page
            .failure
            .as_ref()
            .is_some_and(|failure| failure.reason == LoginFailureReason::InvalidCredentials)
        {
            self.record_identity_session_error(IdentitySessionError::LoginFailed {
                reason: LoginFailureReason::InvalidCredentials,
            });
            return Err(self.require_identity_login("统一认证凭证已失效，请重新登录"));
        }
        card_target_from_identity_evidence(
            &identity_client,
            &response.final_url,
            response.status,
            response.redirect_location.as_ref(),
            response.body(),
            &card_origin,
        )
        .ok_or_else(|| self.record_card_session_error("校园卡统一认证未返回服务跳转"))
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use crate::reference_test_support::FixtureServer;

    #[tokio::test]
    async fn backend_repair_run072714_stale_or_foreign_card_boundary_never_submits() {
        for expired in [true, false] {
            let id = FixtureServer::new(vec![]);
            let card = FixtureServer::new(vec![]);
            let root = std::env::temp_dir().join(format!("card-boundary-{}", Uuid::new_v4()));
            let mut r = create_backend_validation_runtime(
                "2026-2027-1".into(),
                false,
                root.join("device/cache.json")
                    .to_string_lossy()
                    .into_owned(),
            )
            .unwrap();
            let user = UserIdentity {
                username: "fixture-user".into(),
                display_name: None,
            };
            r.coordinator
                .begin_authentication(ServiceId::Identity)
                .unwrap();
            r.coordinator
                .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
                .unwrap();
            r.card_client = Some(
                CampusCardClient::new(
                    CampusCardAdapterConfig::new(card.base()).unwrap(),
                    r.identity.transport().clone(),
                )
                .unwrap(),
            );
            let bound_user = if expired {
                user
            } else {
                UserIdentity {
                    username: "different-fixture".into(),
                    display_name: None,
                }
            };
            let mut boundary = CardPasswordBoundary::new(
                bound_user,
                "not-needed".into(),
                Url::parse(card.base()).unwrap(),
            );
            if expired {
                boundary.created_at = std::time::Instant::now() - Duration::from_secs(121);
            }
            r.pending_card_password = Some(boundary);
            let error = r
                .complete_terminal_card_password("synthetic".into())
                .await
                .unwrap_err();
            assert_eq!(
                crate::telemetry::diagnostic_reason(&error),
                "campus_card_password_boundary_invalid"
            );
            assert!(r.pending_card_password.is_none());
            assert!(id.requests().is_empty());
            assert!(card.requests().is_empty());
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
