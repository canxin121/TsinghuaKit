//! Session orchestration for the verified part of unified identity login.
//!
//! This module owns the verified primary login boundary.  It fetches the
//! deployment-provided SM2 public key, converts plaintext credentials into the
//! service's wire value, and only promotes a response to an authenticated
//! identity session when the classifier sees an allowlisted service handoff
//! or a ticketless handoff that was actually fetched with the shared Cookie
//! session. A plain success page or HTTP 200 is never enough.

use std::{collections::HashSet, fmt, sync::Mutex};

use reqwest::{StatusCode, Url};
use thiserror::Error;

use crate::{
    identity::SecondAuthMethod,
    identity::{FormEncoding, InvalidationStatus},
    identity_client::{
        IdentityClientError, LoginFailureReason, LoginFormInput, LoginPageEvidence,
        LoginResponseClassification, PasswordInput, TrustedDeviceInput,
    },
    identity_crypto::{Sm2PasswordError, encrypt_password},
    identity_execution::{IdentityExecutionClient, IdentityExecutionError, IdentityHttpResponse},
    protocol::{SecondFactorChallenge, SecondFactorMethod, ServiceId, ServiceTicket, UserIdentity},
    session::{BoundServiceTicket, SessionCoordinator, SessionError, SessionSnapshot},
};

const MAX_TICKET_REDIRECT_HOPS: usize = 10;

/// Errors returned while establishing the identity session.
#[derive(Debug, Error)]
pub enum IdentitySessionError {
    #[error(transparent)]
    Execution(#[from] IdentityExecutionError),

    #[error(transparent)]
    Session(#[from] SessionError),

    #[error("identity login returned HTTP {status}")]
    HttpStatus { status: StatusCode },

    #[error("identity login response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("identity login failed: {reason:?}")]
    LoginFailed { reason: LoginFailureReason },

    #[error("identity login requires second factor ({marker})")]
    RequiresSecondFactor { marker: String },

    #[error("identity login page did not contain an SM2 public key")]
    MissingSm2PublicKey,

    #[error(transparent)]
    Crypto(#[from] Sm2PasswordError),

    #[error("identity login returned the login page instead of an authenticated page")]
    LoginPageReturned,

    #[error("identity login response did not contain an anchor ticket")]
    MissingAnchorTicket,

    #[error("identity login response contained a marked invalidation signal")]
    InvalidationMarked,

    #[error("identity login response contained an unusable anchor ticket")]
    InvalidAnchorTicket,

    #[error("identity second-factor action is not configured: {action}")]
    MissingSecondFactorAction { action: &'static str },

    #[error("identity second-factor response was not valid JSON")]
    InvalidSecondFactorResponse,

    #[error("identity second-factor response came from an unexpected route")]
    SecondFactorUnexpectedRoute,

    #[error("identity second-factor request failed")]
    SecondFactorFailed,

    #[error("identity verification code was rejected")]
    VerificationCodeRejected,

    #[error("identity second-factor verification request failed")]
    SecondFactorVerificationRequest {
        #[source]
        source: IdentityExecutionError,
    },

    #[error("identity second-factor redirect request failed")]
    SecondFactorRedirectRequest {
        #[source]
        source: IdentityExecutionError,
    },

    #[error("identity second-factor response did not contain an available method")]
    NoAvailableSecondFactor,

    #[error("identity second-factor flow is not waiting for a code")]
    SecondFactorNotPending,

    #[error("identity second-factor method is not available for this challenge: {method}")]
    SecondFactorMethodUnavailable { method: String },

    #[error("identity verification code must not be empty")]
    EmptyVerificationCode,

    #[error("identity second-factor response did not contain a verified flow")]
    SecondFactorNotVerified,

    #[error("identity second-factor response did not contain a redirect URL")]
    MissingSecondFactorRedirect,

    #[error("identity login redirect chain exceeded the safe limit")]
    RedirectChainTooLong,

    #[error("identity trusted-device saveFinger is not configured")]
    UnsupportedTrustedDevice,

    #[error("identity trusted-device input is invalid: {field}")]
    InvalidTrustedDeviceInput { field: &'static str },
}

/// The result of an identity handoff proven by a ticket or a fetched
/// allowlisted Cookie-backed handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySessionResult {
    pub snapshot: SessionSnapshot,
    /// Some current identity deployments complete SSO through a cookie-backed
    /// service handoff. In that shape the success page contains an
    /// allowlisted, ticketless roaming link and the one-time credential is
    /// consumed by the service itself. Keep the ticket optional instead of
    /// manufacturing a value from a URL or a success message.
    pub identity_ticket: Option<BoundServiceTicket>,
    pub trusted_device: Option<TrustedDeviceResult>,
}

struct FollowedHandoff {
    page: LoginPageEvidence,
    /// The response that produced `page`. Keeping the parsed evidence and its
    /// response together preserves a cross-origin Location or static target
    /// discovered after an identity/OAuth/WebVPN continuation.
    response: IdentityHttpResponse,
    /// True only after an allowlisted handoff link was observed and, when it
    /// was ticketless, fetched through the cookie-aware transport.
    handoff_proven: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedDeviceStatus {
    Saved,
    LimitReached,
    Rejected,
    InvalidResponse,
    RequestFailed,
}

/// The saveFinger result never exposes the returned finger3 value in Debug.
/// The value remains owned by Rust for the lifetime of this result.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedDeviceResult {
    pub status: TrustedDeviceStatus,
    fingerprint3: Option<String>,
}

impl fmt::Debug for TrustedDeviceResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedDeviceResult")
            .field("status", &self.status)
            .field(
                "fingerprint3",
                &self.fingerprint3.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl TrustedDeviceResult {
    pub fn is_saved(&self) -> bool {
        self.status == TrustedDeviceStatus::Saved
    }
}

/// The result of the primary identity step.  A second-factor challenge keeps
/// the Cookie-aware orchestrator alive so the caller can continue the same
/// authentication flow without exposing cookies or HTML to the UI layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentitySessionOutcome {
    Authenticated(IdentitySessionResult),
    RequiresSecondFactor(IdentitySecondFactorChallenge),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySecondFactorChallenge {
    pub snapshot: SessionSnapshot,
    pub challenge: SecondFactorChallenge,
}

/// The verified response returned by the identity service's second-factor
/// redirect. Target services must consume this response directly instead of
/// submitting their credential form a second time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedServiceSecondFactor {
    pub(crate) response: IdentityHttpResponse,
}

/// Executes the currently verified identity login flow and writes its state to
/// a caller-owned [`SessionCoordinator`].
pub struct IdentitySessionOrchestrator {
    identity: IdentityExecutionClient,
    pending_trusted_device: Mutex<Option<OwnedTrustedDeviceOptions>>,
    // Attempt receipt is claimed before saveFinger I/O. Multiple target
    // challenges share it; unknown outcomes cannot replay registration.
    trusted_device_attempt: Mutex<Option<TrustedDeviceResult>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedTrustedDeviceOptions {
    fingerprint: String,
    device_name: String,
    single_login: Option<String>,
}

/// Values used when saveFinger should be attempted after second-factor
/// verification. All values are borrowed only for the initial call and then
/// held inside Rust while the same Cookie session completes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TrustedDeviceOptions<'a> {
    pub fingerprint: &'a str,
    pub device_name: &'a str,
    pub single_login: Option<&'a str>,
}

impl fmt::Debug for TrustedDeviceOptions<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedDeviceOptions")
            .field("fingerprint", &"[redacted]")
            .field("device_name", &"[redacted]")
            .field("single_login", &self.single_login.map(|_| "[redacted]"))
            .finish()
    }
}

impl<'a> TrustedDeviceOptions<'a> {
    pub fn new(fingerprint: &'a str, device_name: &'a str) -> Self {
        Self {
            fingerprint,
            device_name,
            single_login: None,
        }
    }

    pub fn with_single_login(mut self, value: &'a str) -> Self {
        self.single_login = Some(value);
        self
    }
}

impl<'a> From<TrustedDeviceOptions<'a>> for OwnedTrustedDeviceOptions {
    fn from(options: TrustedDeviceOptions<'a>) -> Self {
        Self {
            fingerprint: options.fingerprint.to_owned(),
            device_name: options.device_name.to_owned(),
            single_login: options.single_login.map(str::to_owned),
        }
    }
}

impl OwnedTrustedDeviceOptions {
    fn as_input(&self) -> TrustedDeviceInput<'_> {
        let mut input = TrustedDeviceInput::new(&self.fingerprint, &self.device_name);
        if let Some(value) = self.single_login.as_deref() {
            input = input.with_single_login(value);
        }
        input
    }
}

impl IdentitySessionOrchestrator {
    pub fn new(identity: IdentityExecutionClient) -> Self {
        Self {
            identity,
            pending_trusted_device: Mutex::new(None),
            trusted_device_attempt: Mutex::new(None),
        }
    }

    pub fn identity(&self) -> &IdentityExecutionClient {
        &self.identity
    }

    /// Installs the application id discovered by WebVPN bootstrap while
    /// preserving the same Cookie jar for the subsequent identity request.
    pub fn set_app_id(&mut self, app_id: impl Into<String>) -> Result<(), IdentitySessionError> {
        self.identity.set_app_id(app_id)?;
        Ok(())
    }

    /// Retains the complete WebVPN-discovered identity login URL, including
    /// its opaque OAuth callback query, for the next login page request.
    pub fn set_login_page_url(&mut self, login_page_url: &Url) -> Result<(), IdentitySessionError> {
        self.identity.set_login_page_url(login_page_url)?;
        Ok(())
    }

    /// Retains the POST action discovered from the same dynamic identity
    /// login form as the page URL and SM2 key.
    pub fn set_login_submit_url(&mut self, submit_url: &Url) -> Result<(), IdentitySessionError> {
        self.identity.set_login_submit_url(submit_url)?;
        Ok(())
    }

    /// Binds the encoding declared by the current dynamic identity form.
    pub fn set_login_form_encoding(
        &mut self,
        encoding: FormEncoding,
    ) -> Result<(), IdentitySessionError> {
        self.identity.set_login_form_encoding(encoding)?;
        Ok(())
    }

    /// Returns the Cookie-aware transport used by identity login.  Cloning the
    /// transport preserves its shared cookie jar for the Learn handoff.
    pub fn transport(&self) -> &crate::transport::CampusHttpTransport {
        self.identity.transport()
    }

    /// Drops the identity cookie jar and any pending trusted-device values
    /// before a runtime starts a new account session.
    pub fn reset_transport(&mut self, user_agent: &str) -> Result<(), IdentitySessionError> {
        self.clear_pending_trusted_device();
        self.clear_trusted_device_attempt();
        self.identity.reset_transport(user_agent)?;
        Ok(())
    }

    /// Fetches the login page when a bootstrap key was not supplied, encodes
    /// plaintext credentials with its SM2 public key when needed, and
    /// authenticates the identity service only after a ticket or fetched
    /// allowlisted handoff is proven.
    ///
    /// The `user` value is application identity metadata.  The login form
    /// still receives its username from `input`; the state machine validates
    /// that the stored identity is non-empty.  The caller is responsible for
    /// keeping those two usernames consistent until a profile-specific
    /// identity endpoint can return authoritative user data.
    pub async fn establish(
        &self,
        coordinator: &mut SessionCoordinator,
        input: LoginFormInput<'_>,
        user: UserIdentity,
    ) -> Result<IdentitySessionOutcome, IdentitySessionError> {
        self.establish_with_trusted_device(coordinator, input, user, None)
            .await
    }

    /// Starts primary login and, when requested, keeps the trusted-device
    /// values inside Rust until the same Cookie session completes second
    /// factor. The saveFinger call is made only after a verified second-factor
    /// response and before the redirect page is fetched.
    pub async fn establish_with_trusted_device<'a>(
        &self,
        coordinator: &mut SessionCoordinator,
        input: LoginFormInput<'a>,
        user: UserIdentity,
        trusted_device: Option<TrustedDeviceOptions<'a>>,
    ) -> Result<IdentitySessionOutcome, IdentitySessionError> {
        self.establish_with_optional_public_key(coordinator, input, user, trusted_device, None)
            .await
    }

    /// Starts primary login using the SM2 public key already obtained by the
    /// WebVPN bootstrap. The bootstrap GET is deliberately not repeated: the
    /// signed dynamic login URL can be one-time or Cookie-sensitive on the
    /// deployed identity service.
    pub async fn establish_with_trusted_device_and_public_key<'a>(
        &self,
        coordinator: &mut SessionCoordinator,
        input: LoginFormInput<'a>,
        user: UserIdentity,
        trusted_device: Option<TrustedDeviceOptions<'a>>,
        public_key: &'a str,
    ) -> Result<IdentitySessionOutcome, IdentitySessionError> {
        self.establish_with_optional_public_key(
            coordinator,
            input,
            user,
            trusted_device,
            Some(public_key),
        )
        .await
    }

    async fn establish_with_optional_public_key<'a>(
        &self,
        coordinator: &mut SessionCoordinator,
        input: LoginFormInput<'a>,
        user: UserIdentity,
        trusted_device: Option<TrustedDeviceOptions<'a>>,
        public_key: Option<&'a str>,
    ) -> Result<IdentitySessionOutcome, IdentitySessionError> {
        self.clear_pending_trusted_device();
        coordinator.begin_authentication(ServiceId::Identity)?;
        self.clear_trusted_device_attempt();
        self.replace_pending_trusted_device(trusted_device.map(Into::into));

        let result = self
            .establish_inner(coordinator, input, user, public_key)
            .await;
        match result {
            Ok(result) => {
                if matches!(&result, IdentitySessionOutcome::Authenticated(_)) {
                    self.clear_pending_trusted_device();
                }
                Ok(result)
            }
            Err(error) => {
                if coordinator
                    .registry()
                    .snapshot_for(ServiceId::Identity)
                    .state
                    == crate::protocol::ServiceSessionState::Authenticating
                {
                    let _ = coordinator.cancel_authentication(ServiceId::Identity);
                }
                if coordinator
                    .registry()
                    .snapshot_for(ServiceId::Identity)
                    .state
                    != crate::protocol::ServiceSessionState::RequiresSecondFactor
                {
                    self.clear_pending_trusted_device();
                }
                Err(error)
            }
        }
    }

    async fn establish_inner(
        &self,
        coordinator: &mut SessionCoordinator,
        input: LoginFormInput<'_>,
        user: UserIdentity,
        public_key: Option<&str>,
    ) -> Result<IdentitySessionOutcome, IdentitySessionError> {
        let uses_trusted_device_login = self.identity.client().is_check_single_login_submission();
        let fetched_public_key = if public_key.is_none() && !uses_trusted_device_login {
            let login_page = self.identity.fetch_login_page().await?;
            if login_page.status != StatusCode::OK {
                return Err(IdentitySessionError::HttpStatus {
                    status: login_page.status,
                });
            }
            self.identity
                .classify_login_page(&login_page)
                .sm2_public_key
                .map(|key| key.value)
        } else {
            None
        };
        let wire_password;
        let submission_input = if uses_trusted_device_login {
            // `checkSingle` is the already trusted-device branch. Its
            // multipart contract contains only remember-me and fingerprint
            // fields; the credential value is deliberately ignored by the
            // dedicated request builder.
            input
        } else {
            match input.password {
                PasswordInput::Plaintext(password) => {
                    let public_key = public_key
                        .filter(|value| !value.trim().is_empty())
                        .or(fetched_public_key.as_deref())
                        .ok_or(IdentitySessionError::MissingSm2PublicKey)?;
                    wire_password = encrypt_password(password, public_key)?;
                    LoginFormInput {
                        username: input.username,
                        password: PasswordInput::precomputed_wire(&wire_password),
                        hidden_fields: input.hidden_fields,
                        device_name: input.device_name,
                        fingerprint: input.fingerprint,
                        generated_fingerprint: input.generated_fingerprint,
                        generated_fingerprint_v3: input.generated_fingerprint_v3,
                        captcha: input.captcha,
                        single_login: input.single_login,
                    }
                }
                PasswordInput::PrecomputedWire(_) => input,
            }
        };

        let response = self.identity.submit_login(submission_input).await?;
        let classification = self.identity.classify_response(&response);
        debug_identity_response("primary-submit", &response, &classification);

        let followed = match classification {
            LoginResponseClassification::HttpStatus { status } => {
                return Err(IdentitySessionError::HttpStatus { status });
            }
            LoginResponseClassification::UnexpectedOrigin => {
                return Err(IdentitySessionError::UnexpectedOrigin);
            }
            LoginResponseClassification::LoginFailed { evidence, .. } => {
                return Err(IdentitySessionError::LoginFailed {
                    reason: evidence.reason,
                });
            }
            LoginResponseClassification::RequiresSecondFactor { .. } => {
                let challenge = self.discover_service_second_factor().await?;
                let pending_trusted_device = self.pending_trusted_device();
                self.replace_pending_trusted_device(pending_trusted_device);
                let snapshot = coordinator.require_second_factor(
                    ServiceId::Identity,
                    challenge.clone(),
                    Some(user),
                )?;
                return Ok(IdentitySessionOutcome::RequiresSecondFactor(
                    IdentitySecondFactorChallenge {
                        snapshot,
                        challenge,
                    },
                ));
            }
            // Some deployments keep the POST endpoint as the final URL even
            // after a successful login.  A validated handoff ticket has
            // already been accepted by the classifier; only a response
            // without one is treated as a login page here.
            LoginResponseClassification::LoginPage(page) if !page.has_login_form => {
                if self.primary_trusted_device_ticketless_continuation_is_clean(&response, &page) {
                    FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    }
                } else {
                    let primary_flow_proof = self.primary_success_flow_proof(&response, &page);
                    self.follow_ticket_redirect_if_needed_with_cookie_proof(
                        &response,
                        page,
                        false,
                        primary_flow_proof,
                    )
                    .await?
                }
            }
            LoginResponseClassification::LoginPage(_) => {
                return Err(IdentitySessionError::LoginPageReturned);
            }
            LoginResponseClassification::AuthenticatedHandoff(page)
            | LoginResponseClassification::OtherPage(page) => {
                if self.primary_success_proves_identity(&response, &page)
                    || self
                        .primary_trusted_device_ticketless_continuation_is_clean(&response, &page)
                    || self.cookie_handoff_proves_identity(&response, &page)
                    || self.ticketless_handoff_route_is_clean(&response, &page)
                {
                    FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    }
                } else {
                    let primary_flow_proof = self.primary_success_flow_proof(&response, &page);
                    self.follow_ticket_redirect_if_needed_with_cookie_proof(
                        &response,
                        page,
                        false,
                        primary_flow_proof,
                    )
                    .await?
                }
            }
            LoginResponseClassification::RedirectCallback(page) => {
                if self.identity_callback_proves_cookie_handoff(&response, &page) {
                    FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    }
                } else {
                    // The identity service can leave the original login form
                    // in the callback template even after the POST has
                    // succeeded.  This branch is reached only after the
                    // classifier found the explicit, allowlisted callback;
                    // allow the fetched callback to prove the handoff while
                    // still rejecting failure and invalidation evidence.
                    let primary_flow_proof = self.primary_success_flow_proof(&response, &page);
                    self.follow_ticket_redirect_if_needed_with_cookie_proof(
                        &response,
                        page,
                        true,
                        primary_flow_proof,
                    )
                    .await?
                }
            }
            LoginResponseClassification::CookieBackedHandoff(page) => {
                if !self.cookie_handoff_proves_identity(&response, &page) {
                    return Err(IdentitySessionError::LoginPageReturned);
                }
                FollowedHandoff {
                    page,
                    response: response.clone(),
                    handoff_proven: true,
                }
            }
        };

        debug_identity_handoff(
            "primary-handoff",
            &followed.page,
            followed.handoff_proven,
            false,
        );
        let ticket = ticket_from_success_page(&followed.page, followed.handoff_proven)?;
        let bound_ticket = ticket.map(|ticket| {
            coordinator
                .registry()
                .bind_ticket(ServiceId::Identity, ticket)
        });
        let result_ticket = bound_ticket.clone();
        let snapshot =
            coordinator.mark_authenticated(ServiceId::Identity, user, bound_ticket, None, None)?;

        Ok(IdentitySessionOutcome::Authenticated(
            IdentitySessionResult {
                snapshot,
                identity_ticket: result_ticket,
                trusted_device: None,
            },
        ))
    }

    async fn follow_ticket_redirect_if_needed(
        &self,
        response: &IdentityHttpResponse,
        page: LoginPageEvidence,
        allow_ticketless_handoff: bool,
    ) -> Result<FollowedHandoff, IdentitySessionError> {
        self.follow_ticket_redirect_if_needed_with_cookie_proof(
            response,
            page,
            allow_ticketless_handoff,
            false,
        )
        .await
    }

    async fn follow_ticket_redirect_if_needed_with_cookie_proof(
        &self,
        response: &IdentityHttpResponse,
        mut page: LoginPageEvidence,
        allow_ticketless_handoff: bool,
        verified_flow_proof: bool,
    ) -> Result<FollowedHandoff, IdentitySessionError> {
        let mut followed_response = response.clone();
        let mut followed_urls = HashSet::new();
        for _ in 0..MAX_TICKET_REDIRECT_HOPS {
            let Some(anchor) = page.anchor_ticket.as_ref() else {
                return Ok(FollowedHandoff {
                    page,
                    response: followed_response,
                    handoff_proven: false,
                });
            };
            if !followed_urls.insert(anchor.href.clone()) {
                // Some deployments render redirect2Jsp as a same-URL callback
                // after the Cookie session has already been established. Do
                // not request that callback forever. The earlier GET counts
                // only when its classifier produced explicit downstream
                // proof; an observed loop by itself is not proof.
                return Ok(FollowedHandoff {
                    page,
                    response: followed_response,
                    handoff_proven: false,
                });
            }
            if anchor.ticket.is_some() {
                return Ok(FollowedHandoff {
                    page,
                    response: followed_response,
                    handoff_proven: true,
                });
            }
            if !self
                .identity
                .client()
                .is_safe_handoff_redirect(&anchor.href)
            {
                return Ok(FollowedHandoff {
                    page,
                    response: followed_response,
                    handoff_proven: false,
                });
            }

            let next_response = self.identity.fetch_handoff_redirect(&anchor.href).await?;
            let classification = self.identity.classify_response(&next_response);
            debug_identity_response("handoff-fetch", &next_response, &classification);
            page = match classification {
                LoginResponseClassification::AuthenticatedHandoff(page) => page,
                LoginResponseClassification::OtherPage(page) => {
                    // OAuth/WebVPN continuation pages are not service
                    // session proof. Some deployments nevertheless return a
                    // successful terminal page without a CSRF field. The
                    // URL is constrained by the explicit Cookie-handoff
                    // allowlist, and Learn/INFO/Registrar still prove their
                    // own sessions later.
                    if self.cookie_handoff_proves_identity_with_cookie_proof(
                        &next_response,
                        &page,
                        verified_flow_proof,
                    ) {
                        return Ok(FollowedHandoff {
                            page,
                            response: next_response,
                            handoff_proven: true,
                        });
                    }
                    if allow_ticketless_handoff
                        && self.ticketless_handoff_route_is_clean_with_cookie_proof(
                            &next_response,
                            &page,
                            verified_flow_proof,
                        )
                    {
                        return Ok(FollowedHandoff {
                            page,
                            response: next_response,
                            handoff_proven: true,
                        });
                    }
                    page
                }
                LoginResponseClassification::RedirectCallback(page) => {
                    // Recent public clients accept the login/check success
                    // page followed by a successful, ticketless
                    // redirect2Jsp response and continue with the same
                    // Cookie jar.  The callback itself is not a service
                    // ticket, so it is only proof of the identity handoff;
                    // Learn/INFO still have to prove their own sessions.
                    // Keep following when the callback exposed a distinct
                    // service anchor, otherwise a same-URL callback would
                    // loop and incorrectly become MissingAnchorTicket.
                    if self.identity_callback_proves_cookie_handoff_with_cookie_proof(
                        &next_response,
                        &page,
                        verified_flow_proof,
                    ) {
                        return Ok(FollowedHandoff {
                            page,
                            response: next_response,
                            handoff_proven: true,
                        });
                    }
                    if allow_ticketless_handoff
                        && self.ticketless_handoff_route_is_clean_with_cookie_proof(
                            &next_response,
                            &page,
                            verified_flow_proof,
                        )
                    {
                        return Ok(FollowedHandoff {
                            page,
                            response: next_response,
                            handoff_proven: true,
                        });
                    }
                    page
                }
                LoginResponseClassification::CookieBackedHandoff(page) => {
                    return Ok(FollowedHandoff {
                        page,
                        response: next_response,
                        handoff_proven: true,
                    });
                }
                LoginResponseClassification::HttpStatus { status } => {
                    return Err(IdentitySessionError::HttpStatus { status });
                }
                LoginResponseClassification::UnexpectedOrigin => {
                    return Err(IdentitySessionError::UnexpectedOrigin);
                }
                LoginResponseClassification::LoginFailed { evidence, .. } => {
                    return Err(IdentitySessionError::LoginFailed {
                        reason: evidence.reason,
                    });
                }
                LoginResponseClassification::RequiresSecondFactor { marker, .. } => {
                    return Err(IdentitySessionError::RequiresSecondFactor { marker });
                }
                LoginResponseClassification::LoginPage(page) if !page.has_login_form => page,
                LoginResponseClassification::LoginPage(_) => {
                    return Err(IdentitySessionError::LoginPageReturned);
                }
            };
            followed_response = next_response;
        }

        Err(IdentitySessionError::RedirectChainTooLong)
    }

    pub(crate) async fn discover_service_second_factor(
        &self,
    ) -> Result<SecondFactorChallenge, IdentitySessionError> {
        let action = self
            .identity
            .client()
            .config()
            .profile
            .second_auth
            .actions
            .find_approaches
            .as_ref()
            .ok_or(IdentitySessionError::MissingSecondFactorAction {
                action: "FIND_APPROACHES",
            })?;
        let response = self.identity.submit_second_auth(action, None, None).await?;
        self.ensure_second_factor_response_route(&response)?;
        let value = parse_second_auth_success(&response)?;
        let mut methods = Vec::new();
        let mut masked_phone = None;
        let object = value
            .get("object")
            .ok_or(IdentitySessionError::InvalidSecondFactorResponse)?;
        match object {
            serde_json::Value::Object(object) => {
                if bool_field(object, "hasWeChatBool") {
                    methods.push(SecondFactorMethod::Wechat);
                }
                if bool_field(object, "hasTotp") {
                    methods.push(SecondFactorMethod::Totp);
                }
                masked_phone = string_field(object, "phone");
                // Current public clients use `type=mobile` when the response
                // only advertises a phone number. `sms` is retained for
                // deployments that explicitly expose `hasSmsBool`; a phone
                // value alone is not evidence for that legacy wire value.
                if bool_field(object, "hasMobileBool") {
                    methods.push(SecondFactorMethod::Mobile);
                }
                if bool_field(object, "hasSmsBool") {
                    methods.push(SecondFactorMethod::Sms);
                }
                if masked_phone.is_some()
                    && !methods.contains(&SecondFactorMethod::Mobile)
                    && !methods.contains(&SecondFactorMethod::Sms)
                {
                    methods.push(SecondFactorMethod::Mobile);
                }
            }
            // Older public clients also observed an array of method objects.
            // Consume only recognized wire names from that array; an unknown
            // object is not permission to fall back to static profile values.
            serde_json::Value::Array(entries) => {
                for entry in entries {
                    let Some(entry) = entry.as_object() else {
                        continue;
                    };
                    if masked_phone.is_none() {
                        masked_phone = string_field(entry, "phone");
                    }
                    let method_name =
                        string_field(entry, "name").or_else(|| string_field(entry, "type"));
                    let Some(method_name) = method_name else {
                        continue;
                    };
                    let method = SecondAuthMethod::from_wire(&method_name);
                    let protocol_method = match method {
                        SecondAuthMethod::Wechat => Some(SecondFactorMethod::Wechat),
                        SecondAuthMethod::Mobile => Some(SecondFactorMethod::Mobile),
                        SecondAuthMethod::Sms => Some(SecondFactorMethod::Sms),
                        SecondAuthMethod::Totp => Some(SecondFactorMethod::Totp),
                        SecondAuthMethod::Other(_) => None,
                    };
                    if let Some(protocol_method) = protocol_method
                        && !methods.contains(&protocol_method)
                    {
                        methods.push(protocol_method);
                    }
                }
            }
            _ => return Err(IdentitySessionError::InvalidSecondFactorResponse),
        }
        methods.dedup();
        if methods.is_empty() {
            return Err(IdentitySessionError::NoAvailableSecondFactor);
        }

        Ok(SecondFactorChallenge {
            methods,
            masked_phone,
            expires_at: None,
        })
    }

    /// Requests that the identity service send a code through the selected
    /// method.  The session remains in `RequiresSecondFactor` until the
    /// redirect page supplies a verified anchor ticket.
    pub async fn send_second_factor_code(
        &self,
        coordinator: &SessionCoordinator,
        method: SecondAuthMethod,
    ) -> Result<SessionSnapshot, IdentitySessionError> {
        let snapshot = pending_second_factor_snapshot(coordinator)?;
        ensure_method_available(&snapshot, &method)?;
        let action = self
            .identity
            .client()
            .config()
            .profile
            .second_auth
            .actions
            .send_code
            .as_ref()
            .ok_or(IdentitySessionError::MissingSecondFactorAction {
                action: "SEND_CODE",
            })?;
        let wire_method = canonical_second_factor_method(&snapshot, &method)?;
        let response = self
            .identity
            .submit_second_auth(action, Some(&wire_method), None)
            .await?;
        self.ensure_second_factor_response_route(&response)?;
        let _ = parse_second_auth_success(&response)?;
        Ok(snapshot)
    }

    /// Verifies a code, follows the returned same-origin redirect, and only
    /// completes the session after a ticket or fetched allowlisted handoff is
    /// proven.
    pub async fn complete_second_factor(
        &self,
        coordinator: &mut SessionCoordinator,
        method: SecondAuthMethod,
        verification_code: &str,
    ) -> Result<IdentitySessionResult, IdentitySessionError> {
        let snapshot = pending_second_factor_snapshot(coordinator)?;
        ensure_method_available(&snapshot, &method)?;
        if verification_code.trim().is_empty() {
            return Err(IdentitySessionError::EmptyVerificationCode);
        }

        let action = if matches!(method, SecondAuthMethod::Totp) {
            self.identity
                .client()
                .config()
                .profile
                .second_auth
                .actions
                .verify_totp_code
                .as_ref()
                .ok_or(IdentitySessionError::MissingSecondFactorAction {
                    action: "VERITY_TOTP_CODE",
                })?
        } else {
            self.identity
                .client()
                .config()
                .profile
                .second_auth
                .actions
                .verify_code
                .as_ref()
                .ok_or(IdentitySessionError::MissingSecondFactorAction {
                    action: "VERITY_CODE",
                })?
        };
        // Consume the pending primary interaction before its one-time POST.
        // If this future is cancelled or any later stage is unconfirmed,
        // the same code/challenge cannot be offered for replay. Reconstruct
        // the pending state only for an explicit code rejection or the
        // synchronous final commit after all proof checks have succeeded.
        coordinator.cancel_authentication(ServiceId::Identity)?;
        let result = async {
            let response = self
                .identity
                .submit_second_auth(action, None, Some(verification_code))
                .await
                .map_err(
                    |source| IdentitySessionError::SecondFactorVerificationRequest { source },
                )?;
            self.ensure_second_factor_response_route(&response)?;
            let value = parse_second_auth_verification(&response)?;
            let object = value
                .get("object")
                .and_then(serde_json::Value::as_object)
                .ok_or(IdentitySessionError::SecondFactorNotVerified)?;
            let flow = string_field(object, "flow");
            if !is_verified_second_factor_flow(flow.as_deref()) {
                return Err(IdentitySessionError::SecondFactorNotVerified);
            }
            debug_second_factor_result(
                &flow,
                object.contains_key("redirectUrl"),
                response.has_cookie_update(),
            );
            // A successful VERITY_CODE/VERITY_TOTP_CODE JSON response is itself
            // identity proof. The final OAuth/WebVPN page may be a clean 200
            // without a second Set-Cookie, so carry the verified-flow proof
            // independently of whether that response rotated a Cookie.
            let mut verified_flow_proof = true;
            let trusted_device = if let Some(options) = self.pending_trusted_device() {
                let (result, save_cookie_update) = self.save_trusted_device(&options).await?;
                verified_flow_proof |= save_cookie_update;
                Some(result)
            } else {
                None
            };
            let redirect = string_field(object, "redirectUrl")
                .ok_or(IdentitySessionError::MissingSecondFactorRedirect)?;
            let redirect_response = self
                .identity
                .fetch_redirect(&redirect)
                .await
                .map_err(|source| IdentitySessionError::SecondFactorRedirectRequest { source })?;
            let classification = self.identity.classify_response(&redirect_response);
            debug_identity_response(
                "second-factor-redirect",
                &redirect_response,
                &classification,
            );
            let followed = self
                .completed_second_factor_page_with_cookie_proof(
                    &redirect_response,
                    classification,
                    verified_flow_proof,
                )
                .await?;
            debug_identity_handoff(
                "second-factor-handoff",
                &followed.page,
                followed.handoff_proven,
                verified_flow_proof,
            );
            let ticket = ticket_from_success_page(&followed.page, followed.handoff_proven)?;
            let bound_ticket = ticket.map(|ticket| {
                coordinator
                    .registry()
                    .bind_ticket(ServiceId::Identity, ticket)
            });
            let result_ticket = bound_ticket.clone();
            restore_primary_challenge(coordinator, &snapshot)?;
            let authenticated_snapshot = coordinator.complete_second_factor(
                ServiceId::Identity,
                None,
                bound_ticket,
                None,
                None,
            )?;
            self.clear_pending_trusted_device();
            Ok(IdentitySessionResult {
                snapshot: authenticated_snapshot,
                identity_ticket: result_ticket,
                trusted_device,
            })
        }
        .await;
        if matches!(&result, Err(IdentitySessionError::VerificationCodeRejected))
            && snapshot.second_factor.as_ref().is_some_and(|challenge| {
                challenge
                    .expires_at
                    .is_none_or(|expiry| expiry > chrono::Utc::now())
            })
        {
            restore_primary_challenge(coordinator, &snapshot)?;
        } else if result.is_err() {
            if coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .state
                == crate::protocol::ServiceSessionState::RequiresSecondFactor
            {
                let _ = coordinator.cancel_authentication(ServiceId::Identity);
            }
            self.clear_pending_trusted_device();
        }
        result
    }

    pub(crate) async fn send_service_second_factor_code(
        &self,
        challenge: &SecondFactorChallenge,
        method: SecondAuthMethod,
    ) -> Result<(), IdentitySessionError> {
        let wire_method = self.service_second_factor_wire_method(challenge, method)?;
        let action = self
            .identity
            .client()
            .config()
            .profile
            .second_auth
            .actions
            .send_code
            .as_ref()
            .ok_or(IdentitySessionError::MissingSecondFactorAction {
                action: "SEND_CODE",
            })?;
        let response = self
            .identity
            .submit_second_auth(action, Some(&wire_method), None)
            .await?;
        self.ensure_second_factor_response_route(&response)?;
        parse_second_auth_success(&response)?;
        Ok(())
    }

    pub(crate) async fn complete_service_second_factor(
        &self,
        challenge: &SecondFactorChallenge,
        method: SecondAuthMethod,
        verification_code: &str,
    ) -> Result<CompletedServiceSecondFactor, IdentitySessionError> {
        self.complete_service_second_factor_with_trusted_device(
            challenge,
            method,
            verification_code,
            None,
        )
        .await
    }

    pub(crate) async fn complete_service_second_factor_with_trusted_device(
        &self,
        challenge: &SecondFactorChallenge,
        method: SecondAuthMethod,
        verification_code: &str,
        trusted_device: Option<TrustedDeviceOptions<'_>>,
    ) -> Result<CompletedServiceSecondFactor, IdentitySessionError> {
        let method = self.service_second_factor_wire_method(challenge, method)?;
        if verification_code.trim().is_empty() {
            return Err(IdentitySessionError::EmptyVerificationCode);
        }
        let actions = &self.identity.client().config().profile.second_auth.actions;
        let action = if method == SecondAuthMethod::Totp {
            actions.verify_totp_code.as_ref().ok_or(
                IdentitySessionError::MissingSecondFactorAction {
                    action: "VERITY_TOTP_CODE",
                },
            )?
        } else {
            actions
                .verify_code
                .as_ref()
                .ok_or(IdentitySessionError::MissingSecondFactorAction {
                    action: "VERITY_CODE",
                })?
        };
        let response = self
            .identity
            .submit_second_auth(action, None, Some(verification_code))
            .await
            .map_err(|source| IdentitySessionError::SecondFactorVerificationRequest { source })?;
        self.ensure_second_factor_response_route(&response)?;
        let value = parse_second_auth_verification(&response)?;
        let object = value
            .get("object")
            .and_then(serde_json::Value::as_object)
            .ok_or(IdentitySessionError::SecondFactorNotVerified)?;
        let flow = string_field(object, "flow");
        if !is_verified_second_factor_flow(flow.as_deref()) {
            return Err(IdentitySessionError::SecondFactorNotVerified);
        }
        let redirect = string_field(object, "redirectUrl")
            .ok_or(IdentitySessionError::MissingSecondFactorRedirect)?;
        if let Some(options) = trusted_device {
            // Explicit user consent applies to the first verified MFA in this
            // context, whether primary or service-specific. The shared receipt
            // prevents repeated registrations on later target challenges.
            self.save_trusted_device(&OwnedTrustedDeviceOptions::from(options))
                .await?;
        }
        let redirect_response = self
            .identity
            .fetch_redirect(&redirect)
            .await
            .map_err(|source| IdentitySessionError::SecondFactorRedirectRequest { source })?;
        let classification = self.identity.classify_response(&redirect_response);
        let followed = self
            .completed_second_factor_page_with_cookie_proof(
                &redirect_response,
                classification,
                true,
            )
            .await?;
        // A target-service double-auth redirect may be ticketless because the
        // outer service login must still be retried. Missing ticket evidence
        // is therefore an intermediate outcome, not final service proof.
        if let Err(error @ IdentitySessionError::InvalidAnchorTicket) =
            ticket_from_success_page(&followed.page, followed.handoff_proven)
        {
            return Err(error);
        }
        Ok(CompletedServiceSecondFactor {
            response: followed.response,
        })
    }

    pub(crate) fn service_second_factor_wire_method(
        &self,
        challenge: &SecondFactorChallenge,
        method: SecondAuthMethod,
    ) -> Result<SecondAuthMethod, IdentitySessionError> {
        // Match the primary challenge's phone aliases, but serialize only the
        // method actually advertised by this service challenge. Expired
        // challenges must be rejected before constructing any request.
        if challenge
            .expires_at
            .is_some_and(|expiry| expiry <= chrono::Utc::now())
        {
            return Err(IdentitySessionError::SecondFactorNotPending);
        }
        let protocol_method = match method {
            SecondAuthMethod::Wechat => SecondFactorMethod::Wechat,
            SecondAuthMethod::Totp => SecondFactorMethod::Totp,
            SecondAuthMethod::Mobile => SecondFactorMethod::Mobile,
            SecondAuthMethod::Sms => SecondFactorMethod::Sms,
            SecondAuthMethod::Other(_) => {
                return Err(IdentitySessionError::SecondFactorMethodUnavailable {
                    method: String::from("unknown"),
                });
            }
        };
        let available = challenge.methods.contains(&protocol_method)
            || (method == SecondAuthMethod::Sms
                && challenge.methods.contains(&SecondFactorMethod::Mobile))
            || (method == SecondAuthMethod::Mobile
                && challenge.methods.contains(&SecondFactorMethod::Sms));
        if available {
            match method {
                SecondAuthMethod::Sms if !challenge.methods.contains(&SecondFactorMethod::Sms) => {
                    Ok(SecondAuthMethod::Mobile)
                }
                SecondAuthMethod::Mobile
                    if !challenge.methods.contains(&SecondFactorMethod::Mobile) =>
                {
                    Ok(SecondAuthMethod::Sms)
                }
                _ => Ok(method),
            }
        } else {
            Err(IdentitySessionError::SecondFactorMethodUnavailable {
                method: method.wire_value().to_owned(),
            })
        }
    }

    fn ensure_second_factor_response_route(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
    ) -> Result<(), IdentitySessionError> {
        if self
            .identity
            .client()
            .is_second_auth_response_url(&response.final_url)
        {
            Ok(())
        } else {
            Err(IdentitySessionError::SecondFactorUnexpectedRoute)
        }
    }

    async fn completed_second_factor_page(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        classification: LoginResponseClassification,
    ) -> Result<FollowedHandoff, IdentitySessionError> {
        self.completed_second_factor_page_with_cookie_proof(response, classification, false)
            .await
    }

    async fn completed_second_factor_page_with_cookie_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        classification: LoginResponseClassification,
        verified_flow_proof: bool,
    ) -> Result<FollowedHandoff, IdentitySessionError> {
        match classification {
            LoginResponseClassification::AuthenticatedHandoff(page)
            | LoginResponseClassification::OtherPage(page) => {
                if self.cookie_handoff_proves_identity_with_cookie_proof(
                    response,
                    &page,
                    verified_flow_proof,
                ) || self.ticketless_handoff_route_is_clean_with_cookie_proof(
                    response,
                    &page,
                    verified_flow_proof,
                ) || self
                    .verified_second_factor_ticketless_continuation_is_clean_with_cookie_proof(
                        response,
                        &page,
                        verified_flow_proof,
                    )
                {
                    Ok(FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    })
                } else {
                    self.follow_ticket_redirect_if_needed_with_cookie_proof(
                        response,
                        page,
                        true,
                        verified_flow_proof,
                    )
                    .await
                }
            }
            LoginResponseClassification::RedirectCallback(page) => {
                // The second-factor JSON is the identity proof for this
                // path.  A terminal redirect2Jsp page can therefore be
                // ticketless, but only when it is the exact identity
                // callback, the same verified flow is still in scope, and
                // the page contains no login/failure evidence.  Do not use
                // the broader primary-login callback proof here: it would
                // let an unverified HTTP 200 callback complete this state.
                if self.verified_ticketless_identity_callback_is_clean_with_cookie_proof(
                    response,
                    &page,
                    verified_flow_proof,
                ) {
                    Ok(FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    })
                } else if page.has_login_form {
                    Err(IdentitySessionError::LoginPageReturned)
                } else {
                    self.follow_ticket_redirect_if_needed_with_cookie_proof(
                        response,
                        page,
                        true,
                        verified_flow_proof,
                    )
                    .await
                }
            }
            LoginResponseClassification::CookieBackedHandoff(page) => {
                if self.cookie_handoff_proves_identity_with_cookie_proof(
                    response,
                    &page,
                    verified_flow_proof,
                ) {
                    Ok(FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    })
                } else {
                    Err(IdentitySessionError::LoginPageReturned)
                }
            }
            LoginResponseClassification::LoginPage(page) => {
                // The verification JSON is already an authenticated identity
                // proof.  A few deployments answer its profiled continuation
                // on the primary submit route with a clean 200 page that has
                // no ticket and no fixed success sentence.  Others retain
                // the original login form as stale template markup on that
                // same continuation.  Keep this exception scoped to the
                // profiled route or the known checkSingle continuation and
                // the verified second-factor path; downstream service
                // adapters still have to prove their own sessions.
                if self.verified_second_factor_ticketless_continuation_is_clean_with_cookie_proof(
                    response,
                    &page,
                    verified_flow_proof,
                ) {
                    Ok(FollowedHandoff {
                        page,
                        response: response.clone(),
                        handoff_proven: true,
                    })
                } else if page.has_login_form {
                    Err(IdentitySessionError::LoginPageReturned)
                } else {
                    self.follow_ticket_redirect_if_needed_with_cookie_proof(
                        response,
                        page,
                        true,
                        verified_flow_proof,
                    )
                    .await
                }
            }
            LoginResponseClassification::HttpStatus { status } => {
                Err(IdentitySessionError::HttpStatus { status })
            }
            LoginResponseClassification::UnexpectedOrigin => {
                Err(IdentitySessionError::UnexpectedOrigin)
            }
            LoginResponseClassification::LoginFailed { evidence, .. } => {
                Err(IdentitySessionError::LoginFailed {
                    reason: evidence.reason,
                })
            }
            LoginResponseClassification::RequiresSecondFactor { .. } => {
                Err(IdentitySessionError::SecondFactorNotVerified)
            }
        }
    }

    fn primary_success_proves_identity(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        response.status.is_success()
            && self
                .identity
                .client()
                .is_login_submission_response_url(&response.final_url)
            && self
                .identity
                .client()
                .has_identity_success_marker(response.body())
            && response.has_cookie_update()
            && !page.has_login_form
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && page.anchor_ticket.is_none()
    }

    /// The public clients treat a successful primary submit page plus its
    /// allowlisted continuation as one login flow. The continuation may
    /// finish without another Cookie update because the session Cookie was
    /// established by the bootstrap or the submit response. Keep this proof
    /// narrower than a generic HTTP 200: it must be the profiled submit route,
    /// carry the deployment success marker, and contain no login or failure
    /// evidence. The caller still has to fetch an allowlisted continuation;
    /// this proof cannot authenticate a page that has no handoff anchor.
    fn primary_success_flow_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        response.status.is_success()
            && self
                .identity
                .client()
                .is_login_submission_response_url(&response.final_url)
            && self
                .identity
                .client()
                .has_identity_success_marker(response.body())
            && !page.has_login_form
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
    }

    /// The public trusted-device branch posts the already trusted fingerprint
    /// to `checkSingle`. Some deployments finish that POST by rotating the
    /// identity Cookie and returning a clean continuation page without the
    /// older `/b/` or `/f/` ticket anchor. The Cookie update is the proof for
    /// this narrowly scoped shape; the route and page checks keep an arbitrary
    /// HTTP 200 from becoming an authenticated session.
    fn primary_trusted_device_ticketless_continuation_is_clean(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        response.status.is_success()
            && self.identity.client().is_check_single_login_submission()
            && self
                .identity
                .client()
                .is_verified_identity_continuation_url(&response.final_url)
            && response.has_cookie_update()
            && !page.has_login_form
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && page.anchor_ticket.is_none()
    }

    /// A successful second-factor JSON response is a separate identity proof
    /// from the HTML page that follows it. A ticketless continuation therefore
    /// still needs an explicit identity-success marker or a response Cookie
    /// update; an arbitrary HTTP 200 cannot complete the session.
    fn verified_second_factor_ticketless_continuation_is_clean(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        self.verified_second_factor_ticketless_continuation_is_clean_with_cookie_proof(
            response, page, false,
        )
    }

    fn verified_second_factor_ticketless_continuation_is_clean_with_cookie_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
        verified_flow_proof: bool,
    ) -> bool {
        response.status.is_success()
            && self
                .identity
                .client()
                .is_verified_identity_continuation_url(&response.final_url)
            && (self
                .identity
                .client()
                .has_identity_success_marker(response.body())
                || response.has_cookie_update()
                || verified_flow_proof)
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && !page.has_login_form
            && page.anchor_ticket.is_none()
    }

    /// The current identity service can finish a verified second-factor
    /// exchange at `redirect2Jsp` with a clean HTTP 200 page containing only
    /// an empty link/script target. That page intentionally has no service
    /// ticket. Accept it only as an identity handoff, never as a service
    /// ticket, and only while the verified JSON proof from this exact flow is
    /// available. An actual callback continuation remains in `anchor_ticket`
    /// and is followed by the normal handoff loop.
    fn verified_ticketless_identity_callback_is_clean_with_cookie_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
        verified_flow_proof: bool,
    ) -> bool {
        response.status.is_success()
            && verified_flow_proof
            && self
                .identity
                .client()
                .is_identity_callback_url(&response.final_url)
            && (self
                .identity
                .client()
                .has_identity_success_marker(response.body())
                || verified_flow_proof)
            && !page.has_login_form
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && page.anchor_ticket.is_none()
    }

    fn identity_callback_proves_cookie_handoff(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        self.identity_callback_proves_cookie_handoff_with_cookie_proof(response, page, false)
    }

    fn identity_callback_proves_cookie_handoff_with_cookie_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
        verified_flow_proof: bool,
    ) -> bool {
        response.status.is_success()
            && self
                .identity
                .client()
                .is_identity_callback_url(&response.final_url)
            && !page.has_login_form
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && (self
                .identity
                .client()
                .has_identity_success_marker(response.body())
                || response.has_cookie_update()
                || verified_flow_proof)
            && page.anchor_ticket.is_none()
    }

    fn cookie_handoff_proves_identity(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        self.cookie_handoff_proves_identity_with_cookie_proof(response, page, false)
    }

    /// A successful second-factor response proves the identity flow, while the
    /// final OAuth/WebVPN terminal page can be a clean 200 without a second
    /// Set-Cookie or a service CSRF field. Accept that terminal page only when
    /// its exact route is already in the Cookie-handoff allowlist and the
    /// proof came from this same verified flow. The proof is deliberately not
    /// accepted for ticketless service routes, which still need their own
    /// CSRF or Cookie evidence.
    fn cookie_handoff_proves_identity_with_cookie_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
        verified_flow_proof: bool,
    ) -> bool {
        let is_ticketless_service_route = self
            .identity
            .client()
            .is_ticketless_service_handoff_url(&response.final_url);
        response.status.is_success()
            && self
                .identity
                .client()
                .is_cookie_backed_handoff_url(&response.final_url)
            && !page.has_login_form
            && page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && (page.csrf.is_some()
                || response.has_cookie_update()
                || (verified_flow_proof && !is_ticketless_service_route))
    }

    /// A ticketless callback is accepted only with explicit success wording,
    /// the current response Cookie update, or the verified-flow proof from
    /// this same identity flow. A ticketless service handoff needs CSRF or a
    /// response Cookie update as its downstream session proof; the
    /// identity-flow proof is intentionally not accepted for that branch.
    fn ticketless_handoff_route_is_clean(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
    ) -> bool {
        self.ticketless_handoff_route_is_clean_with_cookie_proof(response, page, false)
    }

    fn ticketless_handoff_route_is_clean_with_cookie_proof(
        &self,
        response: &crate::identity_execution::IdentityHttpResponse,
        page: &LoginPageEvidence,
        verified_flow_proof: bool,
    ) -> bool {
        let clean_page = page.failure.is_none()
            && page.invalidation.status != InvalidationStatus::Marked
            && !page
                .anchor_ticket
                .as_ref()
                .is_some_and(|anchor| anchor.href != response.final_url.as_str());
        if !response.status.is_success() || !clean_page || page.has_login_form {
            return false;
        }

        if self
            .identity
            .client()
            .is_identity_callback_url(&response.final_url)
        {
            return self
                .identity
                .client()
                .has_identity_success_marker(response.body())
                || response.has_cookie_update()
                || verified_flow_proof;
        }

        self.identity
            .client()
            .is_ticketless_service_handoff_url(&response.final_url)
            && (page.csrf.is_some() || response.has_cookie_update())
    }

    fn replace_pending_trusted_device(&self, options: Option<OwnedTrustedDeviceOptions>) {
        let mut pending = self
            .pending_trusted_device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *pending = options;
    }

    fn pending_trusted_device(&self) -> Option<OwnedTrustedDeviceOptions> {
        self.pending_trusted_device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn clear_pending_trusted_device(&self) {
        self.replace_pending_trusted_device(None);
    }

    fn clear_trusted_device_attempt(&self) {
        *self
            .trusted_device_attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
    pub(crate) fn trusted_device_registration_status(&self) -> Option<TrustedDeviceStatus> {
        self.trusted_device_attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|result| result.status)
    }
    async fn save_trusted_device(
        &self,
        options: &OwnedTrustedDeviceOptions,
    ) -> Result<(TrustedDeviceResult, bool), IdentitySessionError> {
        {
            let mut slot = self
                .trusted_device_attempt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(result) = slot.as_ref() {
                return Ok((result.clone(), false));
            }
            // Includes cancellation and unknown transport outcomes: an
            // attempt is not repeated in the same authentication context.
            *slot = Some(TrustedDeviceResult {
                status: TrustedDeviceStatus::RequestFailed,
                fingerprint3: None,
            });
        }
        let result = self.send_trusted_device_registration(options).await;
        if let Ok((value, _)) = &result {
            *self
                .trusted_device_attempt
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value.clone());
        }
        let status = self
            .trusted_device_registration_status()
            .unwrap_or(TrustedDeviceStatus::RequestFailed);
        let outcome = match status {
            TrustedDeviceStatus::Saved => "saved",
            TrustedDeviceStatus::LimitReached => "limit_reached",
            TrustedDeviceStatus::Rejected => "rejected",
            TrustedDeviceStatus::InvalidResponse => "invalid_response",
            TrustedDeviceStatus::RequestFailed => "request_unconfirmed",
        };
        tracing::info!(target:"tsinghua_kit::auth",event="trusted_device_registration",service="identity",trust_result=outcome);
        result
    }

    async fn send_trusted_device_registration(
        &self,
        options: &OwnedTrustedDeviceOptions,
    ) -> Result<(TrustedDeviceResult, bool), IdentitySessionError> {
        let input = options.as_input();
        let response = match self.identity.submit_trusted_device(input).await {
            Ok(response) => response,
            Err(IdentityExecutionError::Client(IdentityClientError::UnsupportedTrustedDevice)) => {
                return Err(IdentitySessionError::UnsupportedTrustedDevice);
            }
            Err(IdentityExecutionError::Client(IdentityClientError::InvalidInput {
                field,
                ..
            })) => return Err(IdentitySessionError::InvalidTrustedDeviceInput { field }),
            Err(_) => {
                return Ok((
                    TrustedDeviceResult {
                        status: TrustedDeviceStatus::RequestFailed,
                        fingerprint3: None,
                    },
                    false,
                ));
            }
        };
        if !self
            .identity
            .client()
            .is_trusted_device_response_url(&response.final_url)
        {
            return Ok((
                TrustedDeviceResult {
                    status: TrustedDeviceStatus::InvalidResponse,
                    fingerprint3: None,
                },
                false,
            ));
        }
        let result = parse_trusted_device_response(&response);
        let cookie_proof = matches!(
            result.status,
            TrustedDeviceStatus::Saved
                | TrustedDeviceStatus::LimitReached
                | TrustedDeviceStatus::Rejected
        ) && response.has_cookie_update();
        Ok((result, cookie_proof))
    }
}

fn pending_second_factor_snapshot(
    coordinator: &SessionCoordinator,
) -> Result<SessionSnapshot, IdentitySessionError> {
    let snapshot = coordinator.registry().snapshot_for(ServiceId::Identity);
    if snapshot.state != crate::protocol::ServiceSessionState::RequiresSecondFactor
        || !snapshot.second_factor.as_ref().is_some_and(|challenge| {
            !challenge.methods.is_empty()
                && challenge
                    .expires_at
                    .is_none_or(|expiry| expiry > chrono::Utc::now())
        })
        || snapshot
            .expires_at
            .is_some_and(|expiry| expiry <= chrono::Utc::now())
    {
        return Err(IdentitySessionError::SecondFactorNotPending);
    }
    Ok(snapshot)
}

fn restore_primary_challenge(
    coordinator: &mut SessionCoordinator,
    snapshot: &SessionSnapshot,
) -> Result<(), IdentitySessionError> {
    let challenge = snapshot
        .second_factor
        .clone()
        .ok_or(IdentitySessionError::SecondFactorNotPending)?;
    coordinator.begin_authentication(ServiceId::Identity)?;
    coordinator.require_second_factor(ServiceId::Identity, challenge, snapshot.user.clone())?;
    Ok(())
}

fn ensure_method_available(
    snapshot: &SessionSnapshot,
    method: &SecondAuthMethod,
) -> Result<(), IdentitySessionError> {
    let protocol_method = match method {
        SecondAuthMethod::Wechat => SecondFactorMethod::Wechat,
        SecondAuthMethod::Mobile => SecondFactorMethod::Mobile,
        SecondAuthMethod::Sms => SecondFactorMethod::Sms,
        SecondAuthMethod::Totp => SecondFactorMethod::Totp,
        SecondAuthMethod::Other(_) => {
            return Err(IdentitySessionError::SecondFactorMethodUnavailable {
                method: method.wire_value().to_owned(),
            });
        }
    };
    let available = snapshot.second_factor.as_ref().is_some_and(|challenge| {
        challenge.methods.contains(&protocol_method)
                // Accept the two historical phone labels for a pending
                // challenge. The selected wire value is still preserved by
                // `SecondAuthMethod`; a phone-only challenge is advertised as
                // `mobile` above so new callers send the value used by current
                // public clients.
                || matches!(method, SecondAuthMethod::Sms)
                    && challenge.methods.contains(&SecondFactorMethod::Mobile)
                || matches!(method, SecondAuthMethod::Mobile)
                    && challenge.methods.contains(&SecondFactorMethod::Sms)
    });
    if available {
        Ok(())
    } else {
        Err(IdentitySessionError::SecondFactorMethodUnavailable {
            method: method.wire_value().to_owned(),
        })
    }
}

/// The identity service has used both `mobile` and `sms` for the same phone
/// challenge over time. The challenge response is the source of truth for the
/// current deployment, so preserve the public method aliases while sending
/// the wire value that this challenge actually advertises.
fn canonical_second_factor_method(
    snapshot: &SessionSnapshot,
    method: &SecondAuthMethod,
) -> Result<SecondAuthMethod, IdentitySessionError> {
    ensure_method_available(snapshot, method)?;
    let Some(challenge) = snapshot.second_factor.as_ref() else {
        return Err(IdentitySessionError::SecondFactorNotPending);
    };
    match method {
        SecondAuthMethod::Sms
            if !challenge.methods.contains(&SecondFactorMethod::Sms)
                && challenge.methods.contains(&SecondFactorMethod::Mobile) =>
        {
            Ok(SecondAuthMethod::Mobile)
        }
        SecondAuthMethod::Mobile
            if !challenge.methods.contains(&SecondFactorMethod::Mobile)
                && challenge.methods.contains(&SecondFactorMethod::Sms) =>
        {
            Ok(SecondAuthMethod::Sms)
        }
        _ => Ok(method.clone()),
    }
}

fn parse_second_auth_success(
    response: &crate::identity_execution::IdentityHttpResponse,
) -> Result<serde_json::Value, IdentitySessionError> {
    if !is_identity_json_success_status(response.status) {
        return Err(IdentitySessionError::HttpStatus {
            status: response.status,
        });
    }
    let value: serde_json::Value = serde_json::from_str(response.body())
        .map_err(|_| IdentitySessionError::InvalidSecondFactorResponse)?;
    if value.get("result").and_then(serde_json::Value::as_str) != Some("success") {
        return Err(IdentitySessionError::SecondFactorFailed);
    }
    Ok(value)
}

fn parse_second_auth_verification(
    response: &crate::identity_execution::IdentityHttpResponse,
) -> Result<serde_json::Value, IdentitySessionError> {
    if !is_identity_json_success_status(response.status) {
        return Err(IdentitySessionError::HttpStatus {
            status: response.status,
        });
    }
    let value: serde_json::Value = serde_json::from_str(response.body())
        .map_err(|_| IdentitySessionError::InvalidSecondFactorResponse)?;
    if value.get("result").and_then(serde_json::Value::as_str) != Some("success") {
        // This parser is used only for the verification-code action.  A
        // well-formed HTTP 200 business failure therefore means the supplied
        // code was rejected (including expired, invalid, or rate-limited
        // code responses).  The raw server message is intentionally not
        // retained or exposed.
        return Err(IdentitySessionError::VerificationCodeRejected);
    }
    Ok(value)
}

#[cfg(test)]
fn contains_verification_code_rejection(value: &serde_json::Value) -> bool {
    fn text_matches(text: &str) -> bool {
        let normalized = text.to_ascii_lowercase();
        (text.contains("验证码")
            && ["错误", "不正确", "无效", "失效", "过期", "失败"]
                .iter()
                .any(|marker| text.contains(marker)))
            || [
                "invalid verification code",
                "verification code is invalid",
                "incorrect verification code",
                "invalid code",
                "incorrect code",
                "wrong code",
                "code expired",
                "code error",
            ]
            .iter()
            .any(|marker| normalized.contains(marker))
    }

    match value {
        serde_json::Value::String(text) => text_matches(text),
        serde_json::Value::Array(values) => values.iter().any(contains_verification_code_rejection),
        serde_json::Value::Object(fields) => {
            fields.values().any(contains_verification_code_rejection)
        }
        _ => false,
    }
}

fn parse_trusted_device_response(
    response: &crate::identity_execution::IdentityHttpResponse,
) -> TrustedDeviceResult {
    if !is_identity_json_success_status(response.status) {
        return TrustedDeviceResult {
            status: TrustedDeviceStatus::RequestFailed,
            fingerprint3: None,
        };
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(response.body()) else {
        return TrustedDeviceResult {
            status: TrustedDeviceStatus::InvalidResponse,
            fingerprint3: None,
        };
    };
    let has_result_marker = value.get("result").is_some() || value.get("success").is_some();
    if !has_result_marker {
        return TrustedDeviceResult {
            status: TrustedDeviceStatus::InvalidResponse,
            fingerprint3: None,
        };
    }

    if trusted_device_response_is_success(&value) {
        // THUInfo requires result=success only. The server can register the
        // supplied stable fingerprint without returning an extra finger3.
        return TrustedDeviceResult {
            status: TrustedDeviceStatus::Saved,
            fingerprint3: trusted_device_fingerprint(&value),
        };
    }

    let is_limit = trusted_device_response_has_limit_marker(&value);
    TrustedDeviceResult {
        status: if is_limit {
            TrustedDeviceStatus::LimitReached
        } else {
            TrustedDeviceStatus::Rejected
        },
        fingerprint3: None,
    }
}

fn is_identity_json_success_status(status: StatusCode) -> bool {
    matches!(status, StatusCode::OK | StatusCode::CREATED)
}

fn trusted_device_response_is_success(value: &serde_json::Value) -> bool {
    let mut seen = false;
    for field in ["result", "success"] {
        if let Some(value) = value.get(field) {
            seen = true;
            let success = match value {
                serde_json::Value::Bool(value) => *value,
                serde_json::Value::Number(value) => value.as_i64() == Some(1),
                serde_json::Value::String(value) => matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "success" | "ok" | "true" | "1"
                ),
                _ => false,
            };
            if !success {
                return false;
            }
        }
    }
    seen
}

fn trusted_device_fingerprint(value: &serde_json::Value) -> Option<String> {
    const FINGERPRINT_FIELDS: [&str; 3] = ["finger3", "fingerPrint3", "fingerprint3"];

    fn from_object(object: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
        FINGERPRINT_FIELDS.iter().find_map(|field| {
            object
                .get(*field)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    }

    let object = value.as_object()?;
    from_object(object).or_else(|| {
        value
            .get("object")
            .and_then(serde_json::Value::as_object)
            .and_then(from_object)
    })
}

fn trusted_device_response_has_limit_marker(value: &serde_json::Value) -> bool {
    fn text_has_limit_marker(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        text.contains("上限")
            || text.contains("超过")
            || lower.contains("limit")
            || lower.contains("maximum")
            || lower.contains("max")
    }

    fn field_has_limit_marker(value: &serde_json::Value, field: &str) -> bool {
        match value.get(field) {
            Some(serde_json::Value::String(value)) => text_has_limit_marker(value),
            Some(serde_json::Value::Object(object)) => ["msg", "message", "error", "status"]
                .into_iter()
                .filter_map(|nested| object.get(nested))
                .any(|nested| nested.as_str().is_some_and(text_has_limit_marker)),
            _ => false,
        }
    }

    ["result", "msg", "message", "error", "status", "object"]
        .into_iter()
        .any(|field| field_has_limit_marker(value, field))
}

fn bool_field(object: &serde_json::Map<String, serde_json::Value>, name: &str) -> bool {
    object
        .get(name)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn string_field(object: &serde_json::Map<String, serde_json::Value>, name: &str) -> Option<String> {
    object
        .get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The identity service has two observed successful second-factor flow
/// markers. Keep this allowlist explicit: `result: "success"` plus an
/// arbitrary `redirectUrl` is not enough to prove that the code verification
/// actually completed the identity flow.
fn is_verified_second_factor_flow(flow: Option<&str>) -> bool {
    matches!(flow, Some("VERIFIED" | "REDIRECTIDLOGINPAGE"))
}

fn ticket_from_success_page(
    page: &LoginPageEvidence,
    handoff_proven: bool,
) -> Result<Option<ServiceTicket>, IdentitySessionError> {
    if page.invalidation.status == InvalidationStatus::Marked {
        return Err(IdentitySessionError::InvalidationMarked);
    }

    if let Some(anchor) = page.anchor_ticket.as_ref() {
        if let Some(ticket) = anchor.ticket.as_deref() {
            return ServiceTicket::new(ticket.to_owned())
                .map(Some)
                .ok_or(IdentitySessionError::InvalidAnchorTicket);
        }
    }
    if handoff_proven {
        return Ok(None);
    }
    if page.anchor_ticket.is_none() {
        Err(IdentitySessionError::MissingAnchorTicket)
    } else {
        Err(IdentitySessionError::InvalidAnchorTicket)
    }
}

/// Application-owned structured authentication diagnostics. Only fixed phases,
/// classifications and evidence-presence booleans are emitted; no host names,
/// URLs, payloads, account identifiers or credential values are formatted.
fn debug_identity_response(
    stage: &str,
    response: &crate::identity_execution::IdentityHttpResponse,
    classification: &LoginResponseClassification,
) {
    if let LoginResponseClassification::LoginFailed { evidence, .. } = classification {
        evidence.trace_safe_diagnostic();
    }
    let (ticket_present, csrf_present, login_form_present, factor_present) = match classification {
        LoginResponseClassification::AuthenticatedHandoff(page)
        | LoginResponseClassification::RedirectCallback(page)
        | LoginResponseClassification::CookieBackedHandoff(page)
        | LoginResponseClassification::LoginPage(page)
        | LoginResponseClassification::OtherPage(page)
        | LoginResponseClassification::LoginFailed { page, .. }
        | LoginResponseClassification::RequiresSecondFactor { page, .. } => (
            page.anchor_ticket
                .as_ref()
                .is_some_and(|anchor| anchor.ticket.is_some()),
            page.csrf.is_some(),
            page.has_login_form,
            page.second_factor_marker.is_some(),
        ),
        _ => (false, false, false, false),
    };
    tracing::debug!(target:"tsinghua_kit::auth", event="identity_response",
        phase=diagnostic_phase(stage), endpoint=crate::telemetry::endpoint(&response.final_url),
        http_status=response.status.as_u16(), reason=login_response_classification_name(classification),
        ticket_present, csrf_present, login_form_present, factor_present,
        redirect_present=response.redirect_location.is_some(), cookie_updated=response.has_cookie_update());
}

fn debug_identity_handoff(
    stage: &str,
    page: &LoginPageEvidence,
    handoff_proven: bool,
    verified_flow_proof: bool,
) {
    let ticket_present = page
        .anchor_ticket
        .as_ref()
        .is_some_and(|anchor| anchor.ticket.is_some());
    let reason = if ticket_present || handoff_proven {
        "verified"
    } else if page.invalidation.status == InvalidationStatus::Marked {
        "invalidation_marked"
    } else if page.failure.is_some() {
        "login_failed"
    } else if page.anchor_ticket.is_none() {
        "missing_anchor_ticket"
    } else {
        "unproven_handoff"
    };
    tracing::debug!(target:"tsinghua_kit::auth",event="identity_handoff",phase=diagnostic_phase(stage),reason,
        ticket_present,csrf_present=page.csrf.is_some(),handoff_proven,
        verified_flow=verified_flow_proof,login_form_present=page.has_login_form);
}

fn debug_second_factor_result(
    flow: &Option<String>,
    has_redirect_url: bool,
    verified_flow_cookie_proof: bool,
) {
    tracing::debug!(target:"tsinghua_kit::auth",event="factor_response",reason=diagnostic_second_factor_flow(flow.as_deref()),
        redirect_present=has_redirect_url,verified_flow=verified_flow_cookie_proof);
}

fn diagnostic_phase(stage: &str) -> &'static str {
    match stage {
        "primary-submit" => "primary_submit",
        "primary-handoff" => "primary_handoff",
        "handoff-fetch" => "handoff_fetch",
        "second-factor-redirect" => "second_factor_redirect",
        "second-factor-handoff" => "second_factor_handoff",
        _ => "other",
    }
}

fn diagnostic_second_factor_flow(flow: Option<&str>) -> &'static str {
    match flow {
        Some("VERIFIED") => "verified",
        Some("REDIRECTIDLOGINPAGE") => "redirect_id_login_page",
        Some(_) => "other",
        None => "missing",
    }
}

fn diagnostic_route_name(path: &str) -> &'static str {
    let path = path.trim_end_matches('/');
    match path {
        "" => "root",
        "/do/off/ui/auth/login/check" => "identity_submit",
        "/do/off/ui/auth/login/checkSingle" => "identity_check_single",
        "/do/off/ui/auth/login/redirect2Jsp" => "identity_callback",
        "/b/doubleAuth/login" => "identity_second_auth",
        "/b/doubleAuth/personal/saveFinger" => "identity_trusted_device",
        _ if path.starts_with("/do/off/ui/auth/login/form/") => "identity_login_form",
        _ if path.starts_with("/https/")
            || path.starts_with("/http/")
            || path.starts_with("/https-443/")
            || path.starts_with("/http-80/") =>
        {
            "webvpn_wrapper"
        }
        _ if path.starts_with("/b/") || path.starts_with("/f/") => "service_handoff",
        _ => "other",
    }
}

fn login_response_classification_name(
    classification: &LoginResponseClassification,
) -> &'static str {
    match classification {
        LoginResponseClassification::HttpStatus { .. } => "http_status",
        LoginResponseClassification::UnexpectedOrigin => "unexpected_origin",
        LoginResponseClassification::LoginFailed { .. } => "login_failed",
        LoginResponseClassification::RequiresSecondFactor { .. } => "second_factor_required",
        LoginResponseClassification::AuthenticatedHandoff(_) => "authenticated_handoff",
        LoginResponseClassification::RedirectCallback(_) => "redirect_callback",
        LoginResponseClassification::CookieBackedHandoff(_) => "cookie_backed_handoff",
        LoginResponseClassification::LoginPage(_) => "login_page",
        LoginResponseClassification::OtherPage(_) => "other_page",
    }
}

impl From<IdentitySessionError> for crate::error::ServiceError {
    fn from(error: IdentitySessionError) -> Self {
        crate::error::ServiceError::Adapter {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use reqwest::Url;

    use super::*;
    use crate::{
        identity::{
            FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormHiddenField,
            LoginFormProfile, SecondAuthAction, SecondAuthActions, SecondAuthMethod,
            SecondAuthProfile, TrustedDeviceProfile,
        },
        identity_client::{IdentityClient, IdentityClientConfig, PasswordInput},
        protocol::{ServiceSessionState, UserIdentity},
        transport::CampusHttpTransport,
    };
    use sm2::{SecretKey, elliptic_curve::sec1::ToSec1Point};

    fn executor(base_url: &str) -> IdentityExecutionClient {
        executor_with_profile(
            base_url,
            FormEncoding::UrlEncoded,
            Some(TrustedDeviceProfile::common()),
        )
    }

    fn executor_with_profile(
        base_url: &str,
        encoding: FormEncoding,
        trusted_device: Option<TrustedDeviceProfile>,
    ) -> IdentityExecutionClient {
        let login_profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                encoding,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![
                    SecondAuthMethod::Wechat,
                    SecondAuthMethod::Mobile,
                    SecondAuthMethod::Sms,
                    SecondAuthMethod::Totp,
                ],
                SecondAuthActions::new(
                    Some(SecondAuthAction::FindApproaches),
                    Some(SecondAuthAction::SendCode),
                    Some(SecondAuthAction::VerifyCode),
                    Some(SecondAuthAction::VerifyTotpCode),
                ),
            ),
        );
        let profile = match trusted_device {
            Some(trusted_device) => login_profile.with_trusted_device(trusted_device),
            None => login_profile.without_trusted_device(),
        };
        let client = IdentityClient::new(
            IdentityClientConfig::new(base_url, profile).expect("identity config"),
        )
        .expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        IdentityExecutionClient::with_transport(client, transport)
    }

    fn cookie_handoff_executor() -> IdentityExecutionClient {
        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![
                    SecondAuthMethod::Wechat,
                    SecondAuthMethod::Mobile,
                    SecondAuthMethod::Sms,
                    SecondAuthMethod::Totp,
                ],
                SecondAuthActions::new(
                    Some(SecondAuthAction::FindApproaches),
                    Some(SecondAuthAction::SendCode),
                    Some(SecondAuthAction::VerifyCode),
                    Some(SecondAuthAction::VerifyTotpCode),
                ),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new("https://id.example.test/", profile)
            .expect("identity config")
            .with_cookie_backed_handoff_route("https://webvpn.example.test/", ["/", "/login"])
            .expect("WebVPN cookie handoff config")
            .with_cookie_backed_handoff_route("https://oauth.example.test/", ["/lb-auth/"])
            .expect("OAuth cookie handoff config");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        IdentityExecutionClient::with_transport(client, transport)
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 512];
        loop {
            let read = stream.read(&mut buffer).expect("request");
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let header_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("headers")
            + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("request body");
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn write_response(
        stream: &mut std::net::TcpStream,
        extra_headers: &str,
        content_type: &str,
        body: &str,
    ) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("response");
    }

    fn write_redirect(stream: &mut std::net::TcpStream, status: &str, location: &str) {
        let response = format!(
            "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(response.as_bytes())
            .expect("redirect response");
    }

    fn input() -> LoginFormInput<'static> {
        LoginFormInput::new("student", PasswordInput::precomputed_wire("wire-password"))
    }

    fn user() -> UserIdentity {
        UserIdentity {
            username: "student".to_owned(),
            display_name: Some("同学".to_owned()),
        }
    }

    #[tokio::test]
    async fn bootstrap_public_key_path_does_not_refetch_a_dynamic_login_page() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<a href="/b/learn?ticket=bootstrap-ticket">continue</a>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish_with_trusted_device_and_public_key(
                &mut coordinator,
                input(),
                user(),
                None,
                "bootstrap-public-key",
            )
            .await
            .expect("identity authentication using the bootstrap page");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };
        assert_eq!(
            result
                .identity_ticket
                .as_ref()
                .expect("identity ticket")
                .as_ticket()
                .as_str(),
            "bootstrap-ticket"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn multipart_login_uses_a_transport_boundary_and_keeps_cookie_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            let lower = submit_request.to_ascii_lowercase();
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            assert!(lower.contains("content-type: multipart/form-data; boundary=thyou-"));
            assert!(lower.contains("cookie: identity_session=fixture-session"));
            assert!(submit_request.contains("name=\"i_user\"\r\n\r\nstudent\r\n"));
            assert!(submit_request.contains("name=\"i_pass\"\r\n\r\nwire-password\r\n"));
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><a href="/b/learn?ticket=multipart-ticket">continue</a></body></html>"#,
            );
        });

        let base_url = format!("http://{}/", address);
        let orchestrator = IdentitySessionOrchestrator::new(executor_with_profile(
            &base_url,
            FormEncoding::Multipart,
            None,
        ));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("multipart identity authentication");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };
        assert_eq!(
            result
                .identity_ticket
                .as_ref()
                .expect("identity ticket")
                .as_ticket()
                .as_str(),
            "multipart-ticket"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn cookie_and_anchor_ticket_prove_identity_authentication() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let login_request = read_request(&mut login_stream);
            assert!(login_request.starts_with("GET /do/off/ui/auth/login/form/portal-app/0"));
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            assert!(
                submit_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            assert!(submit_request.contains("i_user=student"));
            assert!(submit_request.contains("i_pass=wire-password"));
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><a href="/b/learn?ticket=identity-ticket&amp;next=%2Fhome">continue</a></body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("identity authentication");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(result.snapshot.service, ServiceId::Identity);
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.snapshot.ticket_present());
        assert_eq!(
            result
                .identity_ticket
                .as_ref()
                .expect("identity ticket")
                .as_ticket()
                .as_str(),
            "identity-ticket"
        );
        assert_eq!(
            coordinator
                .bound_ticket(ServiceId::Identity)
                .expect("stored ticket")
                .as_ticket()
                .as_str(),
            "identity-ticket"
        );
        assert!(!format!("{result:?}").contains("identity-ticket"));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn cookie_backed_handoff_can_authenticate_without_a_page_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            assert!(
                submit_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body>登录成功。正在重定向到<a href="/f/j_spring_security_thauth_roaming_entry">继续</a></body></html>"#,
            );

            let (mut handoff_stream, _) = listener.accept().expect("handoff request");
            let handoff_request = read_request(&mut handoff_stream);
            assert!(handoff_request.starts_with("GET /f/j_spring_security_thauth_roaming_entry"));
            assert!(
                handoff_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut handoff_stream,
                "",
                "text/html",
                r#"<html><head><meta name="_csrf" content="CSRF_TOKEN_REDACTED"></head><body>authenticated handoff</body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("cookie-backed identity authentication");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(!result.snapshot.ticket_present());
        assert!(result.identity_ticket.is_none());
        assert!(
            coordinator
                .registry()
                .bound_ticket(ServiceId::Identity)
                .is_none()
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn identity_callback_can_prove_cookie_handoff_without_service_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body>登录成功。正在重定向到<a href="/do/off/ui/auth/login/redirect2Jsp">继续</a></body></html>"#,
            );

            let (mut callback_stream, _) = listener.accept().expect("callback request");
            let callback_request = read_request(&mut callback_stream);
            assert!(callback_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            assert!(
                callback_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut callback_stream,
                "",
                "text/html",
                r#"<html><body>登录成功。正在重定向到</body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("ticketless identity callback authentication");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(!result.snapshot.ticket_present());
        assert!(result.identity_ticket.is_none());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn primary_success_callback_chain_can_finish_without_a_second_cookie_or_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body>登录成功。正在重定向到<a href="/do/off/ui/auth/login/redirect2Jsp">继续</a></body></html>"#,
            );

            let (mut callback_stream, _) = listener.accept().expect("callback request");
            let callback_request = read_request(&mut callback_stream);
            assert!(callback_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            write_response(
                &mut callback_stream,
                "",
                "text/html",
                "<html><body>ticketless callback terminal</body></html>",
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("primary callback chain should complete");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn follows_oauth_webvpn_encoded_root_and_identity_callback_without_ticket() {
        let identity_listener = TcpListener::bind("127.0.0.1:0").expect("identity listener");
        let oauth_listener = TcpListener::bind("127.0.0.1:0").expect("oauth listener");
        let webvpn_listener = TcpListener::bind("127.0.0.1:0").expect("WebVPN listener");
        let identity_address = identity_listener.local_addr().expect("identity address");
        let oauth_address = oauth_listener.local_addr().expect("OAuth address");
        let webvpn_address = webvpn_listener.local_addr().expect("WebVPN address");

        let identity_server = thread::spawn(move || {
            let (mut login_stream, _) = identity_listener.accept().expect("login request");
            let login_request = read_request(&mut login_stream);
            assert!(login_request.starts_with("GET /do/off/ui/auth/login/form/portal-app/0"));
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = identity_listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            assert!(
                submit_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            let oauth_redirect = format!(
                "http://{oauth_address}/lb-auth/lbredirect?scheme=http&host=webvpn.example.test&port=80&uri=%2F"
            );
            write_redirect(&mut submit_stream, "302 Found", &oauth_redirect);

            let (mut callback_stream, _) = identity_listener.accept().expect("callback request");
            let callback_request = read_request(&mut callback_stream);
            assert!(callback_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            assert!(
                callback_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut callback_stream,
                "",
                "text/html",
                "<html><body>登录成功。正在重定向到</body></html>",
            );
        });

        let oauth_server = thread::spawn(move || {
            let (mut stream, _) = oauth_listener.accept().expect("OAuth request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /lb-auth/lbredirect?"));
            let root_wrapper = format!("http://{webvpn_address}/https/opaque-map%2F");
            write_redirect(&mut stream, "307 Temporary Redirect", &root_wrapper);
        });

        let webvpn_server = thread::spawn(move || {
            let (mut stream, _) = webvpn_listener.accept().expect("WebVPN request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /https/opaque-map%2F"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            let callback = format!("http://{identity_address}/do/off/ui/auth/login/redirect2Jsp");
            write_redirect(&mut stream, "302 Found", &callback);
        });

        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![SecondAuthMethod::Sms],
                SecondAuthActions::new(
                    Some(SecondAuthAction::FindApproaches),
                    Some(SecondAuthAction::SendCode),
                    Some(SecondAuthAction::VerifyCode),
                    None,
                ),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new(format!("http://{identity_address}/"), profile)
            .expect("identity config")
            .with_cookie_backed_handoff_route(format!("http://{oauth_address}/"), ["/lb-auth/"])
            .expect("OAuth handoff config")
            .with_cookie_backed_handoff_route(format!("http://{webvpn_address}/"), ["/"])
            .expect("WebVPN handoff config");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        let orchestrator = IdentitySessionOrchestrator::new(
            IdentityExecutionClient::with_transport(client, transport),
        );
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("encoded WebVPN callback handoff");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());

        identity_server.join().expect("identity server");
        oauth_server.join().expect("OAuth server");
        webvpn_server.join().expect("WebVPN server");
    }

    #[tokio::test]
    async fn second_factor_follows_oauth_webvpn_callback_chain_without_ticket() {
        let identity_listener = TcpListener::bind("127.0.0.1:0").expect("identity listener");
        let oauth_listener = TcpListener::bind("127.0.0.1:0").expect("oauth listener");
        let webvpn_listener = TcpListener::bind("127.0.0.1:0").expect("WebVPN listener");
        let identity_address = identity_listener.local_addr().expect("identity address");
        let oauth_address = oauth_listener.local_addr().expect("OAuth address");
        let webvpn_address = webvpn_listener.local_addr().expect("WebVPN address");

        let identity_server = thread::spawn(move || {
            let (mut login_stream, _) = identity_listener.accept().expect("login request");
            let login_request = read_request(&mut login_stream);
            assert!(login_request.starts_with("GET /do/off/ui/auth/login/form/portal-app/0"));
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = identity_listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><div name="doubleAuth">二次认证</div></body></html>"#,
            );

            let (mut approaches_stream, _) =
                identity_listener.accept().expect("approaches request");
            let approaches_request = read_request(&mut approaches_stream);
            assert!(approaches_request.starts_with("POST /b/doubleAuth/login"));
            assert!(approaches_request.contains("action=FIND_APPROACHES"));
            write_response(
                &mut approaches_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"phone":"138****0000"}}"#,
            );

            let (mut send_stream, _) = identity_listener.accept().expect("send request");
            let send_request = read_request(&mut send_stream);
            assert!(send_request.contains("action=SEND_CODE"));
            assert!(send_request.contains("type=mobile"));
            write_response(
                &mut send_stream,
                "",
                "application/json",
                r#"{"result":"success","msg":"sent"}"#,
            );

            let (mut verify_stream, _) = identity_listener.accept().expect("verify request");
            let verify_request = read_request(&mut verify_stream);
            assert!(verify_request.contains("action=VERITY_CODE"));
            assert!(verify_request.contains("vericode=123456"));
            write_response(
                &mut verify_stream,
                "Set-Cookie: VERIFIED_FLOW_SESSION=fixture-verified; Path=/\r\n",
                "application/json",
                r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
            );

            let (mut first_callback_stream, _) =
                identity_listener.accept().expect("first callback request");
            let first_callback_request = read_request(&mut first_callback_stream);
            assert!(first_callback_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            let oauth_redirect = format!("http://{oauth_address}/thu-oauth/auth?state=fixture");
            write_redirect(&mut first_callback_stream, "302 Found", &oauth_redirect);

            let (mut final_callback_stream, _) =
                identity_listener.accept().expect("final callback request");
            let final_callback_request = read_request(&mut final_callback_stream);
            assert!(final_callback_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            let final_callback_request_lower = final_callback_request.to_ascii_lowercase();
            assert!(final_callback_request_lower.contains("cookie:"));
            assert!(final_callback_request_lower.contains("identity_session=fixture-session"));
            assert!(
                final_callback_request_lower.contains("verified_flow_session=fixture-verified")
            );
            write_response(
                &mut final_callback_stream,
                "",
                "text/html",
                "<html><body>ticketless identity callback</body></html>",
            );
        });

        let oauth_server = thread::spawn(move || {
            let (mut stream, _) = oauth_listener.accept().expect("OAuth request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /thu-oauth/auth?"));
            let root_wrapper = format!("http://{webvpn_address}/https/opaque-map%2F");
            write_redirect(&mut stream, "307 Temporary Redirect", &root_wrapper);
        });

        let webvpn_server = thread::spawn(move || {
            let (mut stream, _) = webvpn_listener.accept().expect("WebVPN request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /https/opaque-map%2F"));
            let request_lower = request.to_ascii_lowercase();
            assert!(request_lower.contains("cookie:"));
            assert!(request_lower.contains("identity_session=fixture-session"));
            assert!(request_lower.contains("verified_flow_session=fixture-verified"));
            let callback = format!("http://{identity_address}/do/off/ui/auth/login/redirect2Jsp");
            write_redirect(&mut stream, "302 Found", &callback);
        });

        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![SecondAuthMethod::Sms],
                SecondAuthActions::new(
                    Some(SecondAuthAction::FindApproaches),
                    Some(SecondAuthAction::SendCode),
                    Some(SecondAuthAction::VerifyCode),
                    None,
                ),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new(format!("http://{identity_address}/"), profile)
            .expect("identity config")
            .with_cookie_backed_handoff_route(format!("http://{oauth_address}/"), ["/thu-oauth/"])
            .expect("OAuth handoff config")
            .with_cookie_backed_handoff_route(format!("http://{webvpn_address}/"), ["/"])
            .expect("WebVPN handoff config");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        let orchestrator = IdentitySessionOrchestrator::new(
            IdentityExecutionClient::with_transport(client, transport),
        );
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("second-factor challenge");
        assert!(matches!(
            outcome,
            IdentitySessionOutcome::RequiresSecondFactor(_)
        ));

        orchestrator
            .send_second_factor_code(&coordinator, SecondAuthMethod::Sms)
            .await
            .expect("code send");
        let result = orchestrator
            .complete_second_factor(&mut coordinator, SecondAuthMethod::Sms, "123456")
            .await
            .expect("encoded WebVPN callback handoff");

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());

        identity_server.join().expect("identity server");
        oauth_server.join().expect("OAuth server");
        webvpn_server.join().expect("WebVPN server");
    }

    #[tokio::test]
    async fn backend_repair_service_second_factor_preserves_followed_target_response() {
        let identity_listener = TcpListener::bind("127.0.0.1:0").expect("identity listener");
        let oauth_listener = TcpListener::bind("127.0.0.1:0").expect("OAuth listener");
        let webvpn_listener = TcpListener::bind("127.0.0.1:0").expect("WebVPN listener");
        let info_listener = TcpListener::bind("127.0.0.1:0").expect("INFO listener");
        let identity_address = identity_listener.local_addr().expect("identity address");
        let oauth_address = oauth_listener.local_addr().expect("OAuth address");
        let webvpn_address = webvpn_listener.local_addr().expect("WebVPN address");
        let info_address = info_listener.local_addr().expect("INFO address");

        let identity_server = thread::spawn(move || {
            let (mut verify_stream, _) = identity_listener.accept().expect("verify request");
            let verify_request = read_request(&mut verify_stream);
            assert!(verify_request.starts_with("POST /b/doubleAuth/login"));
            assert!(verify_request.contains("action=VERITY_CODE"));
            assert!(verify_request.contains("vericode=123456"));
            write_response(
                &mut verify_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
            );

            let (mut callback_stream, _) = identity_listener.accept().expect("callback request");
            let callback_request = read_request(&mut callback_stream);
            assert!(callback_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            let oauth = format!("http://{oauth_address}/thu-oauth/auth?state=fixture");
            write_redirect(&mut callback_stream, "302 Found", &oauth);
        });

        let oauth_server = thread::spawn(move || {
            let (mut stream, _) = oauth_listener.accept().expect("OAuth request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /thu-oauth/auth?"));
            let webvpn = format!("http://{webvpn_address}/https/root%2F");
            write_redirect(&mut stream, "307 Temporary Redirect", &webvpn);
        });

        let webvpn_server = thread::spawn(move || {
            let (mut stream, _) = webvpn_listener.accept().expect("WebVPN request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /https/root%2F"));
            let target =
                format!("http://{info_address}/f/info/gxfw_fg/common/index?ticket=service-target");
            write_redirect(&mut stream, "302 Found", &target);
        });

        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![SecondAuthMethod::Sms],
                SecondAuthActions::new(None, None, Some(SecondAuthAction::VerifyCode), None),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new(format!("http://{identity_address}/"), profile)
            .expect("identity config")
            .with_anchor_ticket_origins([format!("http://{info_address}/")])
            .expect("INFO ticket origin")
            .with_cookie_backed_handoff_route(format!("http://{oauth_address}/"), ["/thu-oauth/"])
            .expect("OAuth handoff config")
            .with_cookie_backed_handoff_route(format!("http://{webvpn_address}/"), ["/"])
            .expect("WebVPN handoff config");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        let orchestrator = IdentitySessionOrchestrator::new(
            IdentityExecutionClient::with_transport(client, transport),
        );
        let challenge = SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Sms],
            masked_phone: None,
            expires_at: None,
        };

        let completed = orchestrator
            .complete_service_second_factor(&challenge, SecondAuthMethod::Sms, "123456")
            .await
            .expect("service second-factor handoff");
        let expected_target = Url::parse(&format!(
            "http://{info_address}/f/info/gxfw_fg/common/index?ticket=service-target"
        ))
        .expect("target URL");
        assert_eq!(
            completed.response.redirect_location.as_ref(),
            Some(&expected_target),
            "the Runtime must receive the response that reached the target handoff"
        );

        identity_server.join().expect("identity server");
        oauth_server.join().expect("OAuth server");
        webvpn_server.join().expect("WebVPN server");
        drop(info_listener);
    }

    #[tokio::test]
    async fn an_unverified_ticketless_identity_callback_does_not_complete_second_factor() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            callback,
            "<html><body>登录成功。正在重定向到</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("unverified callback remains unproven");

        assert!(!followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn verified_flow_proof_accepts_clean_callback_without_new_cookie_or_ticket() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            callback,
            "<html><body>ticketless identity callback</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        let followed = orchestrator
            .completed_second_factor_page_with_cookie_proof(&response, classification, true)
            .await
            .expect("verified-flow Cookie proof should complete the clean callback");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
        assert!(matches!(
            ticket_from_success_page(&followed.page, followed.handoff_proven),
            Ok(None)
        ));
    }

    #[tokio::test]
    async fn verified_second_factor_accepts_real_ticketless_callback_template() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            callback,
            r#"
                <!doctype html>
                <html>
                  <head><title>欢迎使用清华大学用户电子身份服务系统</title></head>
                  <body>
                    <div class="wrapper">
                      登录成功。正在重定向到 <b><i> </i></b> ...
                      <br><a href="">直接跳转</a>
                    </div>
                    <script>
                      setTimeout(function () {
                        window.location.replace("");
                      }, 1500);
                    </script>
                  </body>
                </html>
            "#,
        );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::RedirectCallback(ref page)
                if page.anchor_ticket.is_none() && !page.has_login_form
        ));

        let followed = orchestrator
            .completed_second_factor_page_with_cookie_proof(&response, classification, true)
            .await
            .expect("verified ticketless callback template");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
        assert!(matches!(
            ticket_from_success_page(&followed.page, followed.handoff_proven),
            Ok(None)
        ));
    }

    #[tokio::test]
    async fn verified_flow_cookie_proof_does_not_prove_a_ticketless_service_page() {
        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![SecondAuthMethod::Sms],
                SecondAuthActions::new(None, None, None, None),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new("https://id.example.test/", profile)
            .expect("identity config")
            .with_anchor_ticket_origins(["https://learn.example.test/"])
            .expect("Learn origin");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        let orchestrator = IdentitySessionOrchestrator::new(
            IdentityExecutionClient::with_transport(client, transport),
        );
        let service_url =
            Url::parse("https://learn.example.test/f/j_spring_security_thauth_roaming_entry")
                .expect("Learn URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            service_url,
            "<html><body>ticketless service page</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        let followed = orchestrator
            .completed_second_factor_page_with_cookie_proof(&response, classification, true)
            .await
            .expect("an unproven service page should remain a classified response");

        assert!(!followed.handoff_proven);
        assert!(matches!(
            ticket_from_success_page(&followed.page, followed.handoff_proven),
            Err(IdentitySessionError::MissingAnchorTicket)
        ));
    }

    #[tokio::test]
    async fn verified_flow_cookie_proof_does_not_prove_a_webvpn_wrapped_learn_page() {
        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![SecondAuthMethod::Sms],
                SecondAuthActions::new(None, None, None, None),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new("https://id.example.test/", profile)
            .expect("identity config")
            .with_anchor_ticket_origins(["https://learn.example.test/"])
            .expect("Learn origin")
            .with_cookie_backed_handoff_route("https://webvpn.example.test/", ["/"])
            .expect("WebVPN origin");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        let orchestrator = IdentitySessionOrchestrator::new(
            IdentityExecutionClient::with_transport(client, transport),
        );
        let service_url = Url::parse(
            "https://webvpn.example.test/https/opaque-map%2Ff%2Fj_spring_security_thauth_roaming_entry",
        )
        .expect("wrapped Learn URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            service_url,
            "<html><body>ticketless wrapped Learn page</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        let followed = orchestrator
            .completed_second_factor_page_with_cookie_proof(&response, classification, true)
            .await
            .expect("an unproven wrapped Learn page should remain classified");

        assert!(!followed.handoff_proven);
        assert!(matches!(
            ticket_from_success_page(&followed.page, followed.handoff_proven),
            Err(IdentitySessionError::MissingAnchorTicket)
        ));
    }

    #[tokio::test]
    async fn verified_second_factor_rejects_stale_login_form_on_identity_callback() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let callback = Url::parse("https://id.example.test/do/off/ui/auth/login/redirect2Jsp")
            .expect("callback URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            callback,
            r#"<html><body><form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form></body></html>"#,
        );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::RedirectCallback(ref page) if page.has_login_form
        ));

        assert!(matches!(
            orchestrator
                .completed_second_factor_page(&response, classification)
                .await,
            Err(IdentitySessionError::LoginPageReturned)
        ));
    }

    #[tokio::test]
    async fn verified_second_factor_accepts_a_marked_ticketless_submit_response() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let submit_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("submit URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            submit_url,
            "<html><body>登录成功。正在重定向到</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::OtherPage(ref page) if !page.has_login_form
        ));

        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("verified ticketless submit continuation");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn verified_second_factor_rejects_stale_login_form_on_submit_continuation() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let submit_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check").expect("submit URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            submit_url,
            r#"<html><body><form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form></body></html>"#,
        );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::LoginPage(ref page) if page.has_login_form
        ));

        assert!(
            orchestrator
                .completed_second_factor_page(&response, classification)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn verified_second_factor_accepts_a_marked_trailing_slash_submit_response() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let submit_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/check/").expect("submit URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            submit_url,
            "<html><body>登录成功。正在重定向到</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("verified trailing-slash ticketless continuation");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn verified_second_factor_accepts_a_marked_check_single_response() {
        let base_url = "https://id.example.test/";
        let orchestrator = IdentitySessionOrchestrator::new(executor(base_url));
        let response_url =
            Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle?continuation=1")
                .expect("checkSingle response URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            response_url,
            "<html><body>登录成功。正在重定向到</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::OtherPage(ref page) if !page.has_login_form
        ));

        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("verified checkSingle continuation");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn allowlisted_cookie_terminal_without_csrf_proves_only_identity_handoff() {
        let orchestrator = IdentitySessionOrchestrator::new(cookie_handoff_executor());
        let final_url = Url::parse("https://webvpn.example.test/").expect("WebVPN URL");
        let response =
            crate::identity_execution::IdentityHttpResponse::from_parts_with_cookie_update(
                StatusCode::OK,
                final_url,
                "<html><body>webvpn continuation complete</body></html>",
            );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::OtherPage(_)
        ));
        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("allowlisted Cookie continuation");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn learn_ticketless_terminal_with_csrf_proves_the_fetched_service_handoff() {
        let profile = IdentityLoginProfile::new(
            "portal-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            LoginFormProfile::new(
                "/do/off/ui/auth/login/check",
                FormEncoding::UrlEncoded,
                LoginFormFields::common(),
            ),
            SecondAuthProfile::new(
                "/b/doubleAuth/login",
                "type",
                "action",
                vec![SecondAuthMethod::Sms],
                SecondAuthActions::new(None, None, None, None),
            ),
        )
        .without_trusted_device();
        let config = IdentityClientConfig::new("https://id.example.test/", profile)
            .expect("identity config")
            .with_anchor_ticket_origins(["https://learn.example.test/"])
            .expect("Learn origin");
        let client = IdentityClient::new(config).expect("identity client");
        let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(5))
            .expect("transport");
        let orchestrator = IdentitySessionOrchestrator::new(
            IdentityExecutionClient::with_transport(client, transport),
        );
        let final_url =
            Url::parse("https://learn.example.test/f/j_spring_security_thauth_roaming_entry")
                .expect("Learn URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            final_url,
            r#"<html><head><meta name="_csrf" content="LEARN_CSRF_REDACTED"></head><body>authenticated</body></html>"#,
        );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::CookieBackedHandoff(_)
        ));
        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("Learn service proof");

        assert!(followed.handoff_proven);
        assert!(followed.page.csrf.is_some());
        assert!(
            followed
                .page
                .anchor_ticket
                .as_ref()
                .is_some_and(|anchor| anchor.ticket.is_none())
        );
    }

    #[tokio::test]
    async fn webvpn_login_callback_without_csrf_does_not_require_a_service_ticket() {
        let orchestrator = IdentitySessionOrchestrator::new(cookie_handoff_executor());
        let final_url = Url::parse("https://webvpn.example.test/login?oauth_login=true")
            .expect("WebVPN login callback URL");
        let response =
            crate::identity_execution::IdentityHttpResponse::from_parts_with_cookie_update(
                StatusCode::OK,
                final_url,
                "<html><body>WebVPN callback complete</body></html>",
            );
        let classification = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::OtherPage(_)
        ));
        let followed = orchestrator
            .completed_second_factor_page(&response, classification)
            .await
            .expect("the explicit WebVPN callback route proves the Cookie handoff");

        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn verified_flow_cookie_proof_accepts_clean_oauth_terminal_without_ticket() {
        let orchestrator = IdentitySessionOrchestrator::new(cookie_handoff_executor());
        let final_url = Url::parse(
            "https://oauth.example.test/lb-auth/lbredirect?scheme=https&host=webvpn.example.test&port=443&uri=%2F",
        )
        .expect("OAuth terminal URL");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            final_url,
            "<html><body>OAuth handoff complete</body></html>",
        );

        let unproven = orchestrator.identity.classify_response(&response);
        assert!(matches!(
            unproven,
            LoginResponseClassification::OtherPage(_)
        ));
        let followed = orchestrator
            .completed_second_factor_page_with_cookie_proof(&response, unproven, false)
            .await
            .expect("unproven terminal remains classified");
        assert!(!followed.handoff_proven);

        let proven = orchestrator.identity.classify_response(&response);
        let followed = orchestrator
            .completed_second_factor_page_with_cookie_proof(&response, proven, true)
            .await
            .expect("verified-flow Cookie proof should complete OAuth terminal");
        assert!(followed.handoff_proven);
        assert!(followed.page.anchor_ticket.is_none());
    }

    #[tokio::test]
    async fn explicit_identity_success_page_can_complete_without_a_page_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/check"));
            write_response(
                &mut submit_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                "<html><body>login success; redirecting</body></html>",
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish_with_trusted_device_and_public_key(
                &mut coordinator,
                input(),
                user(),
                None,
                "bootstrap-public-key",
            )
            .await
            .expect("explicit identity success page");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn directly_fetched_ticketless_handoff_does_not_require_a_page_ticket_or_loop() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            submit_stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /f/j_spring_security_thauth_roaming_entry\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut handoff_stream, _) = listener.accept().expect("handoff request");
            let handoff_request = read_request(&mut handoff_stream);
            assert!(handoff_request.starts_with("GET /f/j_spring_security_thauth_roaming_entry"));
            assert!(
                handoff_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut handoff_stream,
                "",
                "text/html",
                r#"<html><head><meta name="_csrf" content="CSRF_TOKEN_REDACTED"></head><body>authenticated handoff</body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("direct cookie-backed identity authentication");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(!result.snapshot.ticket_present());
        assert!(result.identity_ticket.is_none());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn plaintext_password_is_encoded_with_the_login_page_key() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let public_key = test_public_key();
        let login_page = format!(
            r#"<div id="sm2publicKey">{public_key}</div><form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#
        );
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(&mut login_stream, "", "text/html", &login_page);

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let submit_request = read_request(&mut submit_stream);
            let body = submit_request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .expect("request body");
            let wire_password = body
                .split('&')
                .find_map(|field| field.strip_prefix("i_pass="))
                .expect("encoded password field");
            assert_eq!(wire_password.len(), 2 * (65 + "password".len() + 32));
            assert!(wire_password.starts_with("04"));
            assert!(wire_password.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(!body.contains("password"));
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><a href="/b/learn?ticket=plaintext-ticket">continue</a></body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let input = LoginFormInput::new("student", PasswordInput::plaintext("password"));
        let outcome = orchestrator
            .establish(&mut coordinator, input, user())
            .await
            .expect("identity authentication");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated identity session");
        };

        assert_eq!(
            result
                .identity_ticket
                .as_ref()
                .expect("identity ticket")
                .as_ticket()
                .as_str(),
            "plaintext-ticket"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn failed_login_rolls_identity_session_back_to_anonymous() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"><div>登录失败</div></form>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let error = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect_err("failed login");

        assert!(matches!(
            error,
            IdentitySessionError::LoginFailed {
                reason: LoginFailureReason::Generic
            }
        ));
        assert_eq!(
            coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .state,
            ServiceSessionState::Anonymous
        );
        assert!(
            !coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .ticket_present()
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn second_factor_response_keeps_the_session_pending_for_completion() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><div name="doubleAuth">二次认证</div></body></html>"#,
            );

            let (mut approaches_stream, _) = listener.accept().expect("approaches request");
            let approaches_request = read_request(&mut approaches_stream);
            assert!(approaches_request.starts_with("POST /b/doubleAuth/login"));
            assert!(approaches_request.contains("action=FIND_APPROACHES"));
            write_response(
                &mut approaches_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"hasWeChatBool":true,"phone":"138****0000","hasTotp":true,"hasMobileBool":true}}"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("second factor challenge");
        let IdentitySessionOutcome::RequiresSecondFactor(challenge) = outcome else {
            panic!("expected second-factor challenge");
        };
        assert_eq!(challenge.challenge.methods.len(), 3);
        assert_eq!(
            challenge.challenge.methods,
            vec![
                SecondFactorMethod::Wechat,
                SecondFactorMethod::Totp,
                SecondFactorMethod::Mobile,
            ]
        );
        assert_eq!(
            challenge.challenge.masked_phone.as_deref(),
            Some("138****0000")
        );
        assert_eq!(
            challenge.snapshot.state,
            ServiceSessionState::RequiresSecondFactor
        );
        assert_eq!(
            coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .state,
            ServiceSessionState::RequiresSecondFactor
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn second_factor_code_and_redirect_complete_the_same_cookie_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><div name="doubleAuth">二次认证</div></body></html>"#,
            );

            let (mut approaches_stream, _) = listener.accept().expect("approaches request");
            let _ = read_request(&mut approaches_stream);
            write_response(
                &mut approaches_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"phone":"138****0000","hasSmsBool":true}}"#,
            );

            let (mut send_stream, _) = listener.accept().expect("send request");
            let send_request = read_request(&mut send_stream);
            assert!(send_request.contains("action=SEND_CODE"));
            assert!(send_request.contains("type=sms"));
            assert!(
                send_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut send_stream,
                "",
                "application/json",
                r#"{"result":"success","msg":"sent"}"#,
            );

            let (mut verify_stream, _) = listener.accept().expect("verify request");
            let verify_request = read_request(&mut verify_stream);
            assert!(verify_request.contains("action=VERITY_CODE"));
            assert!(verify_request.contains("vericode=123456"));
            assert!(!verify_request.contains("type=sms"));
            write_response(
                &mut verify_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
            );

            let (mut redirect_stream, _) = listener.accept().expect("redirect request");
            let redirect_request = read_request(&mut redirect_stream);
            assert!(redirect_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            write_response(
                &mut redirect_stream,
                "",
                "text/html",
                r#"<html><body><a href="/b/identity-intermediate">continue</a></body></html>"#,
            );

            let (mut intermediate_stream, _) = listener.accept().expect("intermediate request");
            let intermediate_request = read_request(&mut intermediate_stream);
            assert!(intermediate_request.starts_with("GET /b/identity-intermediate"));
            write_response(
                &mut intermediate_stream,
                "",
                "text/html",
                r#"<html><body><a href="/f/final?ticket=two-factor-ticket">continue</a></body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("second factor challenge");
        assert!(matches!(
            outcome,
            IdentitySessionOutcome::RequiresSecondFactor(_)
        ));
        assert_eq!(
            coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .second_factor
                .as_ref()
                .map(|challenge| challenge.methods.clone()),
            Some(vec![SecondFactorMethod::Sms])
        );

        let sent = orchestrator
            .send_second_factor_code(&coordinator, SecondAuthMethod::Sms)
            .await
            .expect("code send");
        assert_eq!(sent.state, ServiceSessionState::RequiresSecondFactor);

        let result = orchestrator
            .complete_second_factor(&mut coordinator, SecondAuthMethod::Sms, "123456")
            .await
            .expect("second factor completion");
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert_eq!(
            result
                .identity_ticket
                .as_ref()
                .expect("identity ticket")
                .as_ticket()
                .as_str(),
            "two-factor-ticket"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn second_factor_check_single_redirect_is_fetched_and_proves_cookie_handoff() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><div name="doubleAuth">二次认证</div></body></html>"#,
            );

            let (mut approaches_stream, _) = listener.accept().expect("approaches request");
            let _ = read_request(&mut approaches_stream);
            write_response(
                &mut approaches_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"phone":"138****0000"}}"#,
            );

            let (mut send_stream, _) = listener.accept().expect("send request");
            let send_request = read_request(&mut send_stream);
            assert!(send_request.contains("action=SEND_CODE"));
            assert!(send_request.contains("type=mobile"));
            assert!(
                send_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut send_stream,
                "",
                "application/json",
                r#"{"result":"success","msg":"sent"}"#,
            );

            let (mut verify_stream, _) = listener.accept().expect("verify request");
            let verify_request = read_request(&mut verify_stream);
            assert!(verify_request.contains("action=VERITY_CODE"));
            assert!(verify_request.contains("vericode=123456"));
            write_response(
                &mut verify_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/checkSingle?continuation=1"}}"#,
            );

            let (mut check_single_stream, _) = listener.accept().expect("checkSingle request");
            let check_single_request = read_request(&mut check_single_stream);
            assert!(
                check_single_request
                    .starts_with("GET /do/off/ui/auth/login/checkSingle?continuation=1")
            );
            assert!(
                check_single_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            write_response(
                &mut check_single_stream,
                "",
                "text/html",
                "<html><body>登录成功。正在重定向到</body></html>",
            );
        });

        let base_url = format!("http://{address}/");
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish(&mut coordinator, input(), user())
            .await
            .expect("second factor challenge");
        assert!(matches!(
            outcome,
            IdentitySessionOutcome::RequiresSecondFactor(_)
        ));

        orchestrator
            .send_second_factor_code(&coordinator, SecondAuthMethod::Mobile)
            .await
            .expect("code send");
        let result = orchestrator
            .complete_second_factor(&mut coordinator, SecondAuthMethod::Mobile, "123456")
            .await
            .expect("checkSingle handoff completion");

        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn direct_check_single_login_uses_trusted_device_multipart_and_keeps_ticket_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let login_request = read_request(&mut login_stream);
            assert!(login_request.starts_with("GET /do/off/ui/auth/login/form/portal-app/0"));
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"><input type="hidden" name="target" value="TARGET_REDACTED"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("checkSingle submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/checkSingle"));
            let lower = submit_request.to_ascii_lowercase();
            assert!(lower.contains("content-type: multipart/form-data; boundary="));
            assert!(submit_request.contains("name=\"i_rememberme\""));
            assert!(submit_request.contains("name=\"fingerPrint\""));
            assert!(submit_request.contains("name=\"fingerGenPrint\""));
            assert!(submit_request.contains("TARGET_REDACTED"));
            assert!(!submit_request.contains("name=\"i_user\""));
            assert!(!submit_request.contains("name=\"i_pass\""));
            assert!(!submit_request.contains("wire-password"));
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><a href="/b/learn?ticket=check-single-ticket">continue</a></body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let mut orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        orchestrator
            .identity
            .fetch_login_page()
            .await
            .expect("checkSingle login page");
        let submit_url = Url::parse(&format!(
            "http://{address}/do/off/ui/auth/login/checkSingle"
        ))
        .expect("checkSingle URL");
        orchestrator
            .set_login_submit_url(&submit_url)
            .expect("validated checkSingle action");

        let hidden_fields = [LoginFormHiddenField::new("target", "TARGET_REDACTED")];
        let input = LoginFormInput::new("student", PasswordInput::plaintext("PASSWORD_REDACTED"))
            .with_hidden_fields(&hidden_fields)
            .with_fingerprint("FINGERPRINT_REDACTED")
            .with_generated_fingerprint("");
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish_with_trusted_device_and_public_key(&mut coordinator, input, user(), None, "")
            .await
            .expect("trusted-device checkSingle login");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated checkSingle login");
        };
        assert_eq!(
            result
                .identity_ticket
                .expect("checkSingle ticket")
                .as_ticket()
                .as_str(),
            "check-single-ticket"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn direct_check_single_login_accepts_a_cookie_continuation_without_ticket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let login_request = read_request(&mut login_stream);
            assert!(login_request.starts_with("GET /do/off/ui/auth/login/form/portal-app/0"));
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_BOOTSTRAP=fixture-bootstrap; Path=/\r\n",
                "text/html",
                r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"><input type="hidden" name="target" value="TARGET_REDACTED"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("checkSingle submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/checkSingle"));
            let lower = submit_request.to_ascii_lowercase();
            assert!(lower.contains("content-type: multipart/form-data; boundary="));
            assert!(submit_request.contains("name=\"i_rememberme\""));
            assert!(submit_request.contains("name=\"fingerPrint\""));
            assert!(submit_request.contains("name=\"fingerGenPrint\""));
            assert!(submit_request.contains("TARGET_REDACTED"));
            assert!(!submit_request.contains("name=\"i_user\""));
            assert!(!submit_request.contains("name=\"i_pass\""));
            assert!(!submit_request.contains("wire-password"));
            write_response(
                &mut submit_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<html><body>trusted-device continuation</body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let mut orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        orchestrator
            .identity
            .fetch_login_page()
            .await
            .expect("checkSingle login page");
        let submit_url = Url::parse(&format!(
            "http://{address}/do/off/ui/auth/login/checkSingle"
        ))
        .expect("checkSingle URL");
        orchestrator
            .set_login_submit_url(&submit_url)
            .expect("validated checkSingle action");

        let hidden_fields = [LoginFormHiddenField::new("target", "TARGET_REDACTED")];
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish_with_trusted_device_and_public_key(
                &mut coordinator,
                LoginFormInput::new("student", PasswordInput::plaintext("PASSWORD_REDACTED"))
                    .with_hidden_fields(&hidden_fields)
                    .with_fingerprint("FINGERPRINT_REDACTED")
                    .with_generated_fingerprint(""),
                user(),
                None,
                "",
            )
            .await
            .expect("cookie-backed trusted-device continuation");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated checkSingle login");
        };
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());
        server.join().expect("server");
    }

    #[tokio::test]
    async fn direct_check_single_accepts_cookie_set_on_same_origin_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let login_request = read_request(&mut login_stream);
            assert!(login_request.starts_with("GET /do/off/ui/auth/login/form/portal-app/0"));
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_BOOTSTRAP=fixture-bootstrap; Path=/\r\n",
                "text/html",
                r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"><input type="hidden" name="target" value="TARGET_REDACTED"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("checkSingle submit request");
            let submit_request = read_request(&mut submit_stream);
            assert!(submit_request.starts_with("POST /do/off/ui/auth/login/checkSingle"));
            let lower = submit_request.to_ascii_lowercase();
            assert!(lower.contains("content-type: multipart/form-data; boundary="));
            assert!(submit_request.contains("name=\"i_rememberme\""));
            assert!(submit_request.contains("name=\"fingerPrint\""));
            assert!(submit_request.contains("name=\"fingerGenPrint\""));
            assert!(submit_request.contains("TARGET_REDACTED"));
            assert!(!submit_request.contains("name=\"i_user\""));
            assert!(!submit_request.contains("name=\"i_pass\""));
            submit_stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /do/off/ui/auth/login/checkSingle?continuation=1\r\nSet-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("same-origin redirect response");

            let (mut continuation_stream, _) = listener.accept().expect("continuation request");
            let continuation_request = read_request(&mut continuation_stream);
            assert!(
                continuation_request
                    .starts_with("GET /do/off/ui/auth/login/checkSingle?continuation=1")
            );
            assert!(
                continuation_request
                    .to_ascii_lowercase()
                    .contains("identity_session=fixture-session")
            );
            write_response(
                &mut continuation_stream,
                "",
                "text/html",
                r#"<html><body>trusted-device continuation</body></html>"#,
            );
        });

        let base_url = format!("http://{address}/");
        let mut orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        orchestrator
            .identity
            .fetch_login_page()
            .await
            .expect("checkSingle login page");
        let submit_url = Url::parse(&format!(
            "http://{address}/do/off/ui/auth/login/checkSingle"
        ))
        .expect("checkSingle URL");
        orchestrator
            .set_login_submit_url(&submit_url)
            .expect("validated checkSingle action");

        let hidden_fields = [LoginFormHiddenField::new("target", "TARGET_REDACTED")];
        let mut coordinator = SessionCoordinator::new();
        let outcome = orchestrator
            .establish_with_trusted_device_and_public_key(
                &mut coordinator,
                LoginFormInput::new("student", PasswordInput::plaintext("PASSWORD_REDACTED"))
                    .with_hidden_fields(&hidden_fields)
                    .with_fingerprint("FINGERPRINT_REDACTED")
                    .with_generated_fingerprint(""),
                user(),
                None,
                "",
            )
            .await
            .expect("cookie-backed redirected checkSingle continuation");
        let IdentitySessionOutcome::Authenticated(result) = outcome else {
            panic!("expected authenticated checkSingle login");
        };
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        assert!(!result.snapshot.ticket_present());
        server.join().expect("server");
    }

    #[test]
    fn direct_check_single_plain_200_without_cookie_or_ticket_stays_unproven() {
        let mut orchestrator =
            IdentitySessionOrchestrator::new(executor("https://id.example.test/"));
        let submit_url = Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle")
            .expect("checkSingle URL");
        orchestrator
            .set_login_submit_url(&submit_url)
            .expect("validated checkSingle action");
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::OK,
            Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle?continuation=1")
                .expect("checkSingle continuation URL"),
            "<html><body>trusted-device continuation</body></html>",
        );
        let classification = orchestrator.identity.classify_response(&response);
        let LoginResponseClassification::LoginPage(page) = classification else {
            panic!("a plain checkSingle 200 must remain a page classification");
        };

        assert!(
            !orchestrator.primary_trusted_device_ticketless_continuation_is_clean(&response, &page)
        );
        assert!(matches!(
            ticket_from_success_page(&page, false),
            Err(IdentitySessionError::MissingAnchorTicket)
        ));
    }

    #[tokio::test]
    async fn trusted_device_save_uses_the_authenticated_cookie_before_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut login_stream, _) = listener.accept().expect("login request");
            let _ = read_request(&mut login_stream);
            write_response(
                &mut login_stream,
                "Set-Cookie: IDENTITY_SESSION=fixture-session; Path=/\r\n",
                "text/html",
                r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
            );

            let (mut submit_stream, _) = listener.accept().expect("submit request");
            let _ = read_request(&mut submit_stream);
            write_response(
                &mut submit_stream,
                "",
                "text/html",
                r#"<html><body><div name="doubleAuth">二次认证</div></body></html>"#,
            );

            let (mut approaches_stream, _) = listener.accept().expect("approaches request");
            let _ = read_request(&mut approaches_stream);
            write_response(
                &mut approaches_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"phone":"138****0000"}}"#,
            );

            let (mut verify_stream, _) = listener.accept().expect("verify request");
            let verify_request = read_request(&mut verify_stream);
            assert!(verify_request.contains("action=VERITY_CODE"));
            assert!(verify_request.contains("vericode=123456"));
            write_response(
                &mut verify_stream,
                "",
                "application/json",
                r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
            );

            let (mut save_stream, _) = listener.accept().expect("saveFinger request");
            let save_request = read_request(&mut save_stream);
            assert!(save_request.starts_with("POST /b/doubleAuth/personal/saveFinger"));
            assert!(
                save_request
                    .to_ascii_lowercase()
                    .contains("cookie: identity_session=fixture-session")
            );
            assert!(save_request.contains("fingerprint=FINGERPRINT_REDACTED"));
            assert!(save_request.contains("deviceName=THYou+desktop"));
            assert!(save_request.contains("radioVal=%E6%98%AF"));
            write_response(
                &mut save_stream,
                "Set-Cookie: VERIFIED_FLOW_SESSION=fixture-verified; Path=/\r\n",
                "application/json",
                r#"{"result":"success","object":{"fingerPrint3":"FINGER3_REDACTED"}}"#,
            );

            let (mut redirect_stream, _) = listener.accept().expect("redirect request");
            let redirect_request = read_request(&mut redirect_stream);
            assert!(redirect_request.starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
            assert!(
                redirect_request
                    .to_ascii_lowercase()
                    .contains("verified_flow_session=fixture-verified")
            );
            write_response(
                &mut redirect_stream,
                "",
                "text/html",
                r#"<html><body>ticketless identity callback</body></html>"#,
            );
        });

        let base_url = format!("http://{}/", address);
        let orchestrator = IdentitySessionOrchestrator::new(executor(&base_url));
        let mut coordinator = SessionCoordinator::new();
        let options = TrustedDeviceOptions::new("FINGERPRINT_REDACTED", "THYou desktop");
        let outcome = orchestrator
            .establish_with_trusted_device(&mut coordinator, input(), user(), Some(options))
            .await
            .expect("second factor challenge");
        assert!(matches!(
            outcome,
            IdentitySessionOutcome::RequiresSecondFactor(_)
        ));

        let result = orchestrator
            .complete_second_factor(&mut coordinator, SecondAuthMethod::Sms, "123456")
            .await
            .expect("second factor completion");
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.identity_ticket.is_none());
        let trusted_device = result
            .trusted_device
            .as_ref()
            .expect("trusted-device result");
        assert_eq!(trusted_device.status, TrustedDeviceStatus::Saved);
        assert!(trusted_device.is_saved());
        assert!(!format!("{trusted_device:?}").contains("FINGER3_REDACTED"));
        assert!(!format!("{result:?}").contains("FINGER3_REDACTED"));
        server.join().expect("server");
    }

    #[test]
    fn trusted_device_response_classifies_success_limit_rejection_and_invalid_data() {
        let response = |body: &str| {
            crate::identity_execution::IdentityHttpResponse::from_parts(
                StatusCode::OK,
                Url::parse("https://id.example.test/b/doubleAuth/personal/saveFinger")
                    .expect("URL"),
                body,
            )
        };

        let saved = parse_trusted_device_response(&response(
            r#"{"success":true,"finger3":"FINGER3_REDACTED"}"#,
        ));
        assert_eq!(saved.status, TrustedDeviceStatus::Saved);
        assert!(!format!("{saved:?}").contains("FINGER3_REDACTED"));

        let limited = parse_trusted_device_response(&response(
            r#"{"result":"fail","msg":"设备数量已达到上限"}"#,
        ));
        assert_eq!(limited.status, TrustedDeviceStatus::LimitReached);

        let rejected = parse_trusted_device_response(&response(
            r#"{"success":false,"message":"device rejected"}"#,
        ));
        assert_eq!(rejected.status, TrustedDeviceStatus::Rejected);

        let invalid = parse_trusted_device_response(&response(r#"{"result":"success"}"#));
        assert_eq!(invalid.status, TrustedDeviceStatus::InvalidResponse);

        let request_failed = parse_trusted_device_response(
            &crate::identity_execution::IdentityHttpResponse::from_parts(
                StatusCode::BAD_GATEWAY,
                Url::parse("https://id.example.test/b/doubleAuth/personal/saveFinger")
                    .expect("URL"),
                "not retained",
            ),
        );
        assert_eq!(request_failed.status, TrustedDeviceStatus::RequestFailed);
    }

    #[test]
    fn verification_code_rejection_is_detected_without_retaining_response_text() {
        assert!(contains_verification_code_rejection(
            &serde_json::json!({"result": "fail", "msg": "验证码错误"})
        ));
        assert!(contains_verification_code_rejection(
            &serde_json::json!({"object": {"message": "invalid verification code"}})
        ));
        assert!(!contains_verification_code_rejection(
            &serde_json::json!({"result": "fail", "msg": "请求过于频繁"})
        ));
    }

    #[test]
    fn accepts_created_for_identity_json_actions_but_keeps_business_shape_strict() {
        let approach = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::CREATED,
            Url::parse("https://id.example.test/b/doubleAuth/login").expect("URL"),
            r#"{"result":"success","object":{"phone":"138****0000"}}"#,
        );
        assert!(parse_second_auth_success(&approach).is_ok());

        let verification = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::CREATED,
            Url::parse("https://id.example.test/b/doubleAuth/login").expect("URL"),
            r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#,
        );
        assert!(parse_second_auth_verification(&verification).is_ok());

        let malformed = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::CREATED,
            Url::parse("https://id.example.test/b/doubleAuth/login").expect("URL"),
            r#"{"result":"success","object":[]}"#,
        );
        assert!(parse_second_auth_success(&malformed).is_ok());
        let parsed = parse_second_auth_success(&malformed).expect("JSON success");
        let object = parsed.get("object").and_then(serde_json::Value::as_object);
        assert!(object.is_none());
    }

    #[test]
    fn unknown_second_factor_flow_is_not_a_verified_flow() {
        assert!(is_verified_second_factor_flow(Some("VERIFIED")));
        assert!(is_verified_second_factor_flow(Some("REDIRECTIDLOGINPAGE")));
        assert!(!is_verified_second_factor_flow(Some("FUTURE_SUCCESS_FLOW")));
        assert!(!is_verified_second_factor_flow(None));
    }

    #[test]
    fn accepts_created_for_trusted_device_response() {
        let response = crate::identity_execution::IdentityHttpResponse::from_parts(
            StatusCode::CREATED,
            Url::parse("https://id.example.test/b/doubleAuth/personal/saveFinger").expect("URL"),
            r#"{"success":true,"finger3":"FINGER3_REDACTED"}"#,
        );
        let parsed = parse_trusted_device_response(&response);
        assert_eq!(parsed.status, TrustedDeviceStatus::Saved);
        assert!(!format!("{parsed:?}").contains("FINGER3_REDACTED"));
    }

    #[test]
    fn backend_repair_reference_service_phone_alias_uses_advertised_wire_value() {
        let orchestrator = IdentitySessionOrchestrator::new(executor("http://127.0.0.1:9/"));
        for (advertised, selected, expected) in [
            (
                SecondFactorMethod::Mobile,
                SecondAuthMethod::Sms,
                SecondAuthMethod::Mobile,
            ),
            (
                SecondFactorMethod::Sms,
                SecondAuthMethod::Mobile,
                SecondAuthMethod::Sms,
            ),
        ] {
            let challenge = SecondFactorChallenge {
                methods: vec![advertised],
                masked_phone: None,
                expires_at: None,
            };
            assert_eq!(
                orchestrator
                    .service_second_factor_wire_method(&challenge, selected)
                    .unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn backend_repair_reference_service_totp_uses_totp_verification_action() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let server = FixtureServer::new(vec![Reply::json(r#"{"result":"fail"}"#)]);
        let orchestrator = IdentitySessionOrchestrator::new(executor(server.base()));
        let challenge = SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Totp],
            masked_phone: None,
            expires_at: None,
        };
        assert!(
            orchestrator
                .complete_service_second_factor(&challenge, SecondAuthMethod::Totp, "123456")
                .await
                .is_err()
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("action=VERITY_TOTP_CODE"));
    }

    #[tokio::test]
    async fn backend_repair_reference_expired_service_challenge_dispatches_nothing() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let server = FixtureServer::new(vec![Reply::json(r#"{"result":"success"}"#)]);
        let orchestrator = IdentitySessionOrchestrator::new(executor(server.base()));
        let challenge = SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Wechat],
            masked_phone: None,
            expires_at: Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        };
        assert!(
            orchestrator
                .send_service_second_factor_code(&challenge, SecondAuthMethod::Wechat)
                .await
                .is_err()
        );
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn backend_repair_reference_totp_success_carries_rotated_cookie_to_target_evidence() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let target = FixtureServer::new(vec![]);
        let target_url = format!(
            "{}f/info/gxfw_fg/common/index?ticket=fixture-service-ticket",
            target.base()
        );
        let server = FixtureServer::new(vec![
            Reply {
                status: 200,
                headers: "Content-Type: application/json\r\nSet-Cookie: fixture_identity=rotated; Path=/; HttpOnly\r\n".into(),
                body: r#"{"result":"success","object":{"flow":"REDIRECTIDLOGINPAGE","redirectUrl":"/do/off/ui/auth/login/redirect2Jsp"}}"#.into(),
            },
            Reply::html(&format!("登录成功。正在重定向到 <a href=\"{target_url}\">continue</a>")),
        ]);
        let profile = executor(server.base()).client().config().profile.clone();
        let config = IdentityClientConfig::new(server.base(), profile)
            .unwrap()
            .with_anchor_ticket_origins([target.base()])
            .unwrap();
        let client = IdentityClient::new(config).unwrap();
        let orchestrator =
            IdentitySessionOrchestrator::new(IdentityExecutionClient::with_transport(
                client,
                CampusHttpTransport::with_timeout(
                    "THYou/reference-success",
                    Duration::from_secs(2),
                )
                .unwrap(),
            ));
        let challenge = SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Totp],
            masked_phone: None,
            expires_at: None,
        };
        let result = orchestrator
            .complete_service_second_factor(&challenge, SecondAuthMethod::Totp, "123456")
            .await
            .expect("TOTP callback is verified while target service remains unproven");
        let evidence = orchestrator
            .identity()
            .client()
            .parse_login_page(result.response.body());
        assert_eq!(evidence.anchor_ticket.unwrap().href, target_url);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("action=VERITY_TOTP_CODE"));
        assert!(requests[1].starts_with("GET /do/off/ui/auth/login/redirect2Jsp"));
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("cookie: fixture_identity=rotated")
        );
        assert!(
            !requests[1].contains("vericode"),
            "never forward the code into navigation"
        );
        assert!(
            target.requests().is_empty(),
            "runtime must still own the target-specific proof"
        );
        assert!(!format!("{:?}", result.response).contains("fixture-service-ticket"));
    }

    #[tokio::test]
    async fn backend_repair_reference_service_success_rejects_untrusted_callback_before_get() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let forbidden = FixtureServer::new(vec![]);
        let response = serde_json::json!({"result":"success", "object": {
            "flow":"REDIRECTIDLOGINPAGE", "redirectUrl":format!("{}steal", forbidden.base()),
        }});
        let server = FixtureServer::new(vec![Reply::json(&response.to_string())]);
        let orchestrator = IdentitySessionOrchestrator::new(executor(server.base()));
        let challenge = SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Wechat],
            masked_phone: None,
            expires_at: None,
        };
        assert!(
            orchestrator
                .complete_service_second_factor(&challenge, SecondAuthMethod::Wechat, "123456")
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
        assert!(forbidden.requests().is_empty());
    }

    fn test_public_key() -> String {
        use std::fmt::Write as _;

        let secret = [
            0x3d, 0xd2, 0xa3, 0x67, 0x9b, 0xf6, 0xf1, 0xdf, 0xc3, 0xb4, 0x9d, 0x3e, 0x99, 0x11,
            0x47, 0x18, 0xe4, 0x8e, 0xec, 0x17, 0x0b, 0xe4, 0xe4, 0xd3, 0xa8, 0x20, 0x52, 0xda,
            0xb1, 0x9e, 0x8b, 0x50,
        ];
        let secret_key = SecretKey::from_slice(&secret).expect("secret key");
        let mut public_key = String::with_capacity(128);
        for byte in &secret_key.public_key().to_sec1_point(false).as_bytes()[1..] {
            let _ = write!(public_key, "{byte:02x}");
        }
        public_key
    }
}

#[cfg(test)]
#[path = "identity_reuse_tests.rs"]
mod reuse_tests;
