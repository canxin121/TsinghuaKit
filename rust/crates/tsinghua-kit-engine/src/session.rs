//! In-memory session orchestration for the independent campus services.
//!
//! A session machine owns exactly one [`ServiceSession`] and does not know
//! anything about HTTP, cookies, password handling, or cryptography.  The
//! machine is deliberately not serializable: service credentials stay in
//! memory and the public snapshot contains only their presence.
//!
//! The supported state graph is:
//!
//! ```text
//! anonymous ────────────────> authenticating
//!     ▲                             │
//!     │                             ├──> requires-second-factor
//!     │                             │          │
//!     │                             │          ├──> authenticated
//!     │                             │          ├──> authenticating
//!     │                             │          ├──> anonymous
//!     │                             │          └──> expired
//!     │                             │
//!     │                             └──> authenticated
//!     │                                        │
//!     │                                        ├──> authenticating
//!     │                                        ├──> anonymous
//!     │                                        └──> expired
//!     │
//!     └──────────────── expired <──────────────┘
//!                         │
//!                         ├──> authenticating
//!                         └──> anonymous
//! ```
//!
//! `authenticated -> authenticating` is the explicit re-authentication path.
//! Every transition that installs a ticket or CSRF token requires a binding
//! carrying the same [`ServiceId`] as the machine, so a token acquired for one
//! service cannot be silently installed into another service's session.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::{
    CsrfToken, SecondFactorChallenge, ServiceId, ServiceSession, ServiceSessionState,
    ServiceTicket, UserIdentity,
};

const REGISTERED_SERVICES: [ServiceId; 7] = [
    ServiceId::Identity,
    ServiceId::Learn,
    ServiceId::Registrar,
    ServiceId::Info,
    ServiceId::Usereg,
    ServiceId::Library,
    ServiceId::CampusCard,
];

/// Shared state for one identity service-ticket handoff.
///
/// The state is separate from the ticket value so every clone of a bound
/// ticket observes the same one-time claim.  There is deliberately no reset
/// operation: a ticket that has been claimed is unusable for every later
/// attempt, including after a transport error.
struct TicketLease {
    attempted: AtomicBool,
}

impl TicketLease {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            attempted: AtomicBool::new(false),
        })
    }

    fn try_claim(&self) -> bool {
        self.attempted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn is_available(&self) -> bool {
        !self.attempted.load(Ordering::Acquire)
    }

    fn invalidate(&self) {
        self.attempted.store(true, Ordering::Release);
    }
}

/// A ticket explicitly associated with the service that acquired it.
///
/// The wrapper is intentionally not serializable.  Its value can be claimed
/// by the session machine, but it should not become part of a persisted
/// session snapshot. Clones share the same one-time claim state.
pub struct BoundServiceTicket {
    service: ServiceId,
    ticket: ServiceTicket,
    lease: Arc<TicketLease>,
}

impl Clone for BoundServiceTicket {
    fn clone(&self) -> Self {
        Self {
            service: self.service,
            ticket: self.ticket.clone(),
            lease: Arc::clone(&self.lease),
        }
    }
}

impl PartialEq for BoundServiceTicket {
    fn eq(&self, other: &Self) -> bool {
        self.service == other.service && self.ticket == other.ticket
    }
}

impl Eq for BoundServiceTicket {}

/// The sole outbound form of a claimed service ticket.
///
/// This value is intentionally not `Clone`. A caller obtains it only after
/// the shared lease changes from available to attempted. The caller must use
/// it for the one request that follows immediately; no retry path may retain
/// or recreate the original [`BoundServiceTicket`].
pub struct ClaimedServiceTicket {
    service: ServiceId,
    ticket: ServiceTicket,
}

impl BoundServiceTicket {
    /// Associates an existing protocol ticket with a service.
    fn new(service: ServiceId, ticket: ServiceTicket) -> Self {
        Self {
            service,
            ticket,
            lease: TicketLease::new(),
        }
    }

    fn with_lease(service: ServiceId, ticket: ServiceTicket, lease: Arc<TicketLease>) -> Self {
        Self {
            service,
            ticket,
            lease,
        }
    }

    /// Returns the service that the caller claims acquired this ticket for.
    pub fn service(&self) -> ServiceId {
        self.service
    }

    /// Borrows the underlying protocol ticket for an authenticated request.
    pub fn as_ticket(&self) -> &ServiceTicket {
        &self.ticket
    }

    /// Attempts to claim this ticket for one outbound request.
    ///
    /// Every clone of this binding shares the same state. Once this returns
    /// `Some`, every later claim returns `None`, including claims made after a
    /// request fails at the transport layer.
    pub fn claim(&self) -> Option<ClaimedServiceTicket> {
        self.lease.try_claim().then(|| ClaimedServiceTicket {
            service: self.service,
            ticket: self.ticket.clone(),
        })
    }

    /// Returns whether this binding can still be claimed.
    pub fn is_available(&self) -> bool {
        self.lease.is_available()
    }

    /// Consumes the binding and returns the protocol ticket.
    ///
    /// This legacy escape hatch marks the shared lease as attempted before it
    /// returns. New outbound code must use [`Self::claim`] so the request path
    /// cannot accidentally retain a retryable raw ticket.
    #[deprecated(note = "use BoundServiceTicket::claim for outbound handoff")]
    pub fn into_ticket(self) -> ServiceTicket {
        self.lease.invalidate();
        self.ticket
    }

    fn into_parts(self) -> (ServiceTicket, Arc<TicketLease>) {
        (self.ticket, self.lease)
    }
}

impl ClaimedServiceTicket {
    /// Returns the service that owns this claimed ticket.
    pub fn service(&self) -> ServiceId {
        self.service
    }

    /// Borrows the protocol ticket for the one request associated with this
    /// claim.
    pub fn as_ticket(&self) -> &ServiceTicket {
        &self.ticket
    }

    /// Returns the opaque ticket text for the one request associated with this
    /// claim.
    pub fn as_str(&self) -> &str {
        self.ticket.as_str()
    }
}

impl fmt::Debug for BoundServiceTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundServiceTicket")
            .field("service", &self.service)
            .field("ticket", &"[redacted]")
            .finish()
    }
}

impl fmt::Debug for ClaimedServiceTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedServiceTicket")
            .field("service", &self.service)
            .field("ticket", &"[redacted]")
            .finish()
    }
}

/// A CSRF token explicitly associated with the service that acquired it.
#[derive(Clone, PartialEq, Eq)]
pub struct BoundCsrfToken {
    service: ServiceId,
    csrf: CsrfToken,
}

impl BoundCsrfToken {
    /// Associates an existing protocol CSRF token with a service.
    fn new(service: ServiceId, csrf: CsrfToken) -> Self {
        Self { service, csrf }
    }

    /// Returns the service that the caller claims acquired this token for.
    pub fn service(&self) -> ServiceId {
        self.service
    }

    /// Borrows the underlying protocol CSRF token for an authenticated request.
    pub fn as_csrf_token(&self) -> &CsrfToken {
        &self.csrf
    }

    /// Consumes the binding and returns the protocol CSRF token.
    pub fn into_csrf_token(self) -> CsrfToken {
        self.csrf
    }
}

impl fmt::Debug for BoundCsrfToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundCsrfToken")
            .field("service", &self.service)
            .field("csrf", &"[redacted]")
            .finish()
    }
}

/// Identifies which kind of service credential failed an ownership check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCredentialKind {
    Ticket,
    Csrf,
}

/// Errors returned when a command cannot be applied to a service session.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    #[error("cannot transition {service:?} from {from:?} to {to:?}")]
    InvalidTransition {
        service: ServiceId,
        from: ServiceSessionState,
        to: ServiceSessionState,
    },

    #[error(
        "{credential:?} belongs to {actual_service:?}, but the session belongs to {expected_service:?}"
    )]
    CredentialServiceMismatch {
        credential: SessionCredentialKind,
        expected_service: ServiceId,
        actual_service: ServiceId,
    },

    #[error("the second-factor challenge for {service:?} has no available method")]
    EmptySecondFactorChallenge { service: ServiceId },

    #[error("the authenticated identity for {service:?} has an empty username")]
    EmptyUserName { service: ServiceId },

    #[error("the second-factor completion for {service:?} has no user identity")]
    MissingUserIdentity { service: ServiceId },

    #[error("the second-factor challenge for {service:?} is missing")]
    MissingSecondFactorChallenge { service: ServiceId },
}

/// A command in the service session state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTransition {
    /// Start a new authentication attempt or explicitly re-authenticate.
    BeginAuthentication,

    /// Record that primary authentication requires a second factor.
    RequireSecondFactor {
        challenge: SecondFactorChallenge,
        user: Option<UserIdentity>,
    },

    /// Finish primary authentication without a second-factor step.
    AuthenticationSucceeded {
        user: UserIdentity,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    },

    /// Finish an outstanding second-factor challenge.
    SecondFactorSucceeded {
        user: Option<UserIdentity>,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    },

    /// Replace the CSRF proof on an already authenticated session while
    /// preserving its user, one-time ticket lease, and expiry deadline.
    RefreshAuthenticatedCsrf { csrf: BoundCsrfToken },

    /// Abandon a primary or second-factor authentication attempt.
    CancelAuthentication,

    /// Mark an active session unusable and clear its service credentials.
    Expire,

    /// Clear the session and return to the anonymous state.
    Logout,
}

/// A credential-free view of a [`ServiceSession`].
///
/// `has_ticket` and `has_csrf` intentionally carry only presence information;
/// the token values are never copied into this type.  The snapshot is safe to
/// pass to UI code or serialize for non-secret application state.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub service: ServiceId,
    pub user: Option<UserIdentity>,
    pub state: ServiceSessionState,
    pub has_ticket: bool,
    pub has_csrf: bool,
    pub second_factor: Option<SecondFactorChallenge>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl SessionSnapshot {
    fn from_session(session: &ServiceSession, ticket_available: bool) -> Self {
        Self {
            service: session.service,
            user: session.user.clone(),
            state: session.state,
            has_ticket: ticket_available,
            has_csrf: session.csrf.is_some(),
            second_factor: session.second_factor.clone(),
            expires_at: session.expires_at,
        }
    }

    /// Returns whether the in-memory session currently has a ticket.
    pub fn ticket_present(&self) -> bool {
        self.has_ticket
    }

    /// Returns whether the in-memory session currently has a CSRF token.
    pub fn csrf_present(&self) -> bool {
        self.has_csrf
    }
}

struct RedactedPresence(bool);

impl fmt::Debug for RedactedPresence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 {
            formatter.write_str("[redacted]")
        } else {
            formatter.write_str("None")
        }
    }
}

impl fmt::Debug for SessionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionSnapshot")
            .field("service", &self.service)
            .field("user", &self.user)
            .field("state", &self.state)
            .field("ticket", &RedactedPresence(self.has_ticket))
            .field("csrf", &RedactedPresence(self.has_csrf))
            .field("second_factor", &self.second_factor)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A credential-free snapshot of all known service sessions.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRegistrySnapshot {
    pub sessions: Vec<SessionSnapshot>,
}

impl fmt::Debug for SessionRegistrySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionRegistrySnapshot")
            .field("sessions", &self.sessions)
            .finish()
    }
}

/// A state machine for one service.  It stores credentials only in memory as
/// part of the protocol session needed by request adapters.
pub struct ServiceSessionStateMachine {
    session: ServiceSession,
    ticket_lease: Option<Arc<TicketLease>>,
}

impl ServiceSessionStateMachine {
    /// Creates an anonymous machine for one service.
    pub fn new(service: ServiceId) -> Self {
        Self {
            session: ServiceSession::anonymous(service),
            ticket_lease: None,
        }
    }

    /// Returns the service owned by this machine.
    pub fn service(&self) -> ServiceId {
        self.session.service
    }

    /// Returns the current state.
    pub fn state(&self) -> ServiceSessionState {
        self.session.state
    }

    /// Binds a protocol ticket to this machine's service.
    pub fn bind_ticket(&self, ticket: ServiceTicket) -> BoundServiceTicket {
        BoundServiceTicket::new(self.service(), ticket)
    }

    /// Binds a protocol CSRF token to this machine's service.
    pub fn bind_csrf(&self, csrf: CsrfToken) -> BoundCsrfToken {
        BoundCsrfToken::new(self.service(), csrf)
    }

    /// Returns a service-bound ticket for an authenticated request without
    /// exposing it through the credential-free snapshot.
    pub fn bound_ticket(&self) -> Option<BoundServiceTicket> {
        if !self.ticket_available() {
            return None;
        }
        let lease = self.ticket_lease.as_ref()?.clone();
        self.session
            .ticket
            .clone()
            .map(|ticket| BoundServiceTicket::with_lease(self.service(), ticket, lease))
    }

    /// Claims the current ticket for one outbound handoff.
    ///
    /// Claiming removes the ticket from this state machine before the caller
    /// performs I/O. The returned value is therefore the only request
    /// credential; a later call returns `None` and callers should use their
    /// shared-cookie handoff instead.
    pub fn claim_ticket(&mut self) -> Option<ClaimedServiceTicket> {
        let ticket = self.session.ticket.take()?;
        let lease = self.ticket_lease.take()?;
        if !lease.try_claim() {
            return None;
        }
        Some(ClaimedServiceTicket {
            service: self.service(),
            ticket,
        })
    }

    /// Returns a service-bound CSRF token for an authenticated request without
    /// exposing it through the credential-free snapshot.
    pub fn bound_csrf(&self) -> Option<BoundCsrfToken> {
        self.session.csrf.clone().map(|csrf| self.bind_csrf(csrf))
    }

    /// Returns a snapshot that omits the ticket and CSRF token values.
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::from_session(&self.session, self.ticket_available())
    }

    /// Applies a state transition atomically.
    pub fn transition(&mut self, transition: SessionTransition) -> Result<(), SessionError> {
        match transition {
            SessionTransition::BeginAuthentication => {
                self.ensure_state(
                    &[
                        ServiceSessionState::Anonymous,
                        ServiceSessionState::RequiresSecondFactor,
                        ServiceSessionState::Authenticated,
                        ServiceSessionState::Expired,
                    ],
                    ServiceSessionState::Authenticating,
                )?;

                self.clear_ticket();
                self.session = ServiceSession {
                    service: self.service(),
                    user: None,
                    state: ServiceSessionState::Authenticating,
                    ticket: None,
                    csrf: None,
                    second_factor: None,
                    expires_at: None,
                };
            }
            SessionTransition::RequireSecondFactor { challenge, user } => {
                self.ensure_state(
                    &[ServiceSessionState::Authenticating],
                    ServiceSessionState::RequiresSecondFactor,
                )?;
                self.validate_challenge(&challenge)?;
                if let Some(user) = &user {
                    self.validate_user(user)?;
                }

                self.clear_ticket();
                self.session.user = user;
                self.session.state = ServiceSessionState::RequiresSecondFactor;
                self.session.csrf = None;
                self.session.second_factor = Some(challenge);
                self.session.expires_at = None;
            }
            SessionTransition::AuthenticationSucceeded {
                user,
                ticket,
                csrf,
                expires_at,
            } => {
                self.ensure_state(
                    &[ServiceSessionState::Authenticating],
                    ServiceSessionState::Authenticated,
                )?;
                self.install_authenticated(user, ticket, csrf, expires_at)?;
            }
            SessionTransition::SecondFactorSucceeded {
                user,
                ticket,
                csrf,
                expires_at,
            } => {
                self.ensure_state(
                    &[ServiceSessionState::RequiresSecondFactor],
                    ServiceSessionState::Authenticated,
                )?;
                if self.session.second_factor.is_none() {
                    return Err(SessionError::MissingSecondFactorChallenge {
                        service: self.service(),
                    });
                }
                let user = user.or_else(|| self.session.user.clone()).ok_or(
                    SessionError::MissingUserIdentity {
                        service: self.service(),
                    },
                )?;
                self.install_authenticated(user, ticket, csrf, expires_at)?;
            }
            SessionTransition::RefreshAuthenticatedCsrf { csrf } => {
                self.ensure_state(
                    &[ServiceSessionState::Authenticated],
                    ServiceSessionState::Authenticated,
                )?;
                self.validate_credentials(None, Some(&csrf))?;
                self.session.csrf = Some(csrf.into_csrf_token());
            }
            SessionTransition::CancelAuthentication => {
                self.ensure_state(
                    &[
                        ServiceSessionState::Authenticating,
                        ServiceSessionState::RequiresSecondFactor,
                    ],
                    ServiceSessionState::Anonymous,
                )?;
                self.clear_ticket();
                self.session = ServiceSession::anonymous(self.service());
            }
            SessionTransition::Expire => {
                self.ensure_state(
                    &[
                        ServiceSessionState::Authenticating,
                        ServiceSessionState::RequiresSecondFactor,
                        ServiceSessionState::Authenticated,
                    ],
                    ServiceSessionState::Expired,
                )?;
                self.session.state = ServiceSessionState::Expired;
                self.clear_ticket();
                self.session.csrf = None;
                self.session.second_factor = None;
                self.session.expires_at = None;
            }
            SessionTransition::Logout => {
                self.ensure_state(
                    &[
                        ServiceSessionState::Authenticating,
                        ServiceSessionState::RequiresSecondFactor,
                        ServiceSessionState::Authenticated,
                        ServiceSessionState::Expired,
                    ],
                    ServiceSessionState::Anonymous,
                )?;
                self.clear_ticket();
                self.session = ServiceSession::anonymous(self.service());
            }
        }

        Ok(())
    }

    /// Starts an authentication or re-authentication attempt.
    pub fn begin_authentication(&mut self) -> Result<(), SessionError> {
        self.transition(SessionTransition::BeginAuthentication)
    }

    /// Records a primary authentication response that requires a second factor.
    pub fn require_second_factor(
        &mut self,
        challenge: SecondFactorChallenge,
        user: Option<UserIdentity>,
    ) -> Result<(), SessionError> {
        self.transition(SessionTransition::RequireSecondFactor { challenge, user })
    }

    /// Marks primary authentication as complete.
    pub fn mark_authenticated(
        &mut self,
        user: UserIdentity,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), SessionError> {
        self.transition(SessionTransition::AuthenticationSucceeded {
            user,
            ticket,
            csrf,
            expires_at,
        })
    }

    /// Completes the second-factor step and marks the session authenticated.
    pub fn complete_second_factor(
        &mut self,
        user: Option<UserIdentity>,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), SessionError> {
        self.transition(SessionTransition::SecondFactorSucceeded {
            user,
            ticket,
            csrf,
            expires_at,
        })
    }

    /// Updates the CSRF proof of an authenticated session without changing
    /// any other session field.
    pub fn refresh_authenticated_csrf(&mut self, csrf: BoundCsrfToken) -> Result<(), SessionError> {
        self.transition(SessionTransition::RefreshAuthenticatedCsrf { csrf })
    }

    /// Abandons an authentication attempt and returns to anonymous.
    pub fn cancel_authentication(&mut self) -> Result<(), SessionError> {
        self.transition(SessionTransition::CancelAuthentication)
    }

    /// Marks an active session expired and clears service credentials.
    pub fn mark_expired(&mut self) -> Result<(), SessionError> {
        self.transition(SessionTransition::Expire)
    }

    /// Expires the session when its session or challenge deadline has passed.
    ///
    /// Returns `true` only when this call performed the transition.
    pub fn expire_if_due(&mut self, now: DateTime<Utc>) -> Result<bool, SessionError> {
        let due = match self.state() {
            ServiceSessionState::Authenticated => self
                .session
                .expires_at
                .is_some_and(|expires_at| expires_at <= now),
            ServiceSessionState::RequiresSecondFactor => self
                .session
                .second_factor
                .as_ref()
                .and_then(|challenge| challenge.expires_at)
                .is_some_and(|expires_at| expires_at <= now),
            _ => false,
        };

        if due {
            self.mark_expired()?;
        }

        Ok(due)
    }

    /// Logs out any non-anonymous session and clears all session data.
    pub fn logout(&mut self) -> Result<(), SessionError> {
        self.transition(SessionTransition::Logout)
    }

    fn ensure_state(
        &self,
        allowed: &[ServiceSessionState],
        target: ServiceSessionState,
    ) -> Result<(), SessionError> {
        if allowed.contains(&self.state()) {
            Ok(())
        } else {
            Err(SessionError::InvalidTransition {
                service: self.service(),
                from: self.state(),
                to: target,
            })
        }
    }

    fn validate_challenge(&self, challenge: &SecondFactorChallenge) -> Result<(), SessionError> {
        if challenge.methods.is_empty() {
            Err(SessionError::EmptySecondFactorChallenge {
                service: self.service(),
            })
        } else {
            Ok(())
        }
    }

    fn validate_user(&self, user: &UserIdentity) -> Result<(), SessionError> {
        if user.username.trim().is_empty() {
            Err(SessionError::EmptyUserName {
                service: self.service(),
            })
        } else {
            Ok(())
        }
    }

    fn validate_credentials(
        &self,
        ticket: Option<&BoundServiceTicket>,
        csrf: Option<&BoundCsrfToken>,
    ) -> Result<(), SessionError> {
        if let Some(ticket) = ticket {
            if ticket.service() != self.service() {
                return Err(SessionError::CredentialServiceMismatch {
                    credential: SessionCredentialKind::Ticket,
                    expected_service: self.service(),
                    actual_service: ticket.service(),
                });
            }
        }
        if let Some(csrf) = csrf {
            if csrf.service() != self.service() {
                return Err(SessionError::CredentialServiceMismatch {
                    credential: SessionCredentialKind::Csrf,
                    expected_service: self.service(),
                    actual_service: csrf.service(),
                });
            }
        }
        Ok(())
    }

    fn ticket_available(&self) -> bool {
        self.session.ticket.is_some()
            && self
                .ticket_lease
                .as_ref()
                .is_some_and(|lease| lease.is_available())
    }

    fn clear_ticket(&mut self) {
        if let Some(lease) = self.ticket_lease.take() {
            lease.invalidate();
        }
        self.session.ticket = None;
    }

    fn install_authenticated(
        &mut self,
        user: UserIdentity,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), SessionError> {
        self.validate_user(&user)?;
        self.validate_credentials(ticket.as_ref(), csrf.as_ref())?;

        let (ticket, ticket_lease) = ticket
            .map(BoundServiceTicket::into_parts)
            .map_or((None, None), |(ticket, lease)| (Some(ticket), Some(lease)));

        self.session.user = Some(user);
        self.session.state = ServiceSessionState::Authenticated;
        self.clear_ticket();
        self.session.ticket = ticket;
        self.ticket_lease = ticket_lease;
        self.session.csrf = csrf.map(BoundCsrfToken::into_csrf_token);
        self.session.second_factor = None;
        self.session.expires_at = expires_at;
        Ok(())
    }
}

/// A registry containing one independent state machine for each service.
///
/// The registry itself is intentionally not serializable.  Use
/// [`SessionRegistry::snapshot`] for a credential-free view.
pub struct SessionRegistry {
    sessions: HashMap<ServiceId, ServiceSessionStateMachine>,
}

impl SessionRegistry {
    /// Creates anonymous state machines for every currently supported service.
    pub fn new() -> Self {
        let sessions = REGISTERED_SERVICES
            .into_iter()
            .map(|service| (service, ServiceSessionStateMachine::new(service)))
            .collect();
        Self { sessions }
    }

    /// Borrows one service machine without exposing another service's state.
    pub fn session(&self, service: ServiceId) -> &ServiceSessionStateMachine {
        self.sessions
            .get(&service)
            .expect("all ServiceId variants are initialized")
    }

    /// Applies a transition to one service and returns its safe snapshot.
    pub fn transition(
        &mut self,
        service: ServiceId,
        transition: SessionTransition,
    ) -> Result<SessionSnapshot, SessionError> {
        let machine = self
            .sessions
            .get_mut(&service)
            .expect("all ServiceId variants are initialized");
        let from = crate::telemetry::state_name(machine.state());
        if let Err(error) = machine.transition(transition) {
            tracing::warn!(target:"tsinghua_kit::session",event="transition_rejected",service=crate::telemetry::service_name(service),from,reason="invalid_transition_or_binding");
            return Err(error);
        }
        tracing::info!(target:"tsinghua_kit::session",event="transition_applied",service=crate::telemetry::service_name(service),from,to=crate::telemetry::state_name(machine.state()));
        Ok(machine.snapshot())
    }

    /// Returns a safe snapshot for one service.
    pub fn snapshot_for(&self, service: ServiceId) -> SessionSnapshot {
        self.session(service).snapshot()
    }

    /// Binds a protocol ticket to one registry service.
    pub fn bind_ticket(&self, service: ServiceId, ticket: ServiceTicket) -> BoundServiceTicket {
        self.session(service).bind_ticket(ticket)
    }

    /// Binds a protocol CSRF token to one registry service.
    pub fn bind_csrf(&self, service: ServiceId, csrf: CsrfToken) -> BoundCsrfToken {
        self.session(service).bind_csrf(csrf)
    }

    pub fn bound_ticket(&self, service: ServiceId) -> Option<BoundServiceTicket> {
        self.session(service).bound_ticket()
    }

    /// Claims one service ticket for an outbound request and removes it from
    /// the registry. A second claim, including one after a network error,
    /// returns `None`; callers should then use the shared-cookie handoff.
    pub fn claim_ticket(&mut self, service: ServiceId) -> Option<ClaimedServiceTicket> {
        self.sessions
            .get_mut(&service)
            .expect("all ServiceId variants are initialized")
            .claim_ticket()
    }

    pub fn bound_csrf(&self, service: ServiceId) -> Option<BoundCsrfToken> {
        self.session(service).bound_csrf()
    }

    /// Refreshes the CSRF proof of one already authenticated service session.
    pub fn refresh_authenticated_csrf(
        &mut self,
        service: ServiceId,
        csrf: BoundCsrfToken,
    ) -> Result<SessionSnapshot, SessionError> {
        self.transition(
            service,
            SessionTransition::RefreshAuthenticatedCsrf { csrf },
        )
    }

    /// Returns safe snapshots in stable `ServiceId` order.
    pub fn snapshot(&self) -> SessionRegistrySnapshot {
        SessionRegistrySnapshot {
            sessions: REGISTERED_SERVICES
                .into_iter()
                .map(|service| self.snapshot_for(service))
                .collect(),
        }
    }

    /// Expires every service whose session or second-factor deadline has
    /// passed. The returned list is stable and contains only sessions changed
    /// by this call.
    pub fn expire_due(&mut self, now: DateTime<Utc>) -> Result<Vec<ServiceId>, SessionError> {
        let mut expired = Vec::new();
        for service in REGISTERED_SERVICES {
            let machine = self
                .sessions
                .get_mut(&service)
                .expect("all ServiceId variants are initialized");
            if machine.expire_if_due(now)? {
                expired.push(service);
            }
        }
        Ok(expired)
    }
}

/// A small orchestration facade over the per-service state machines.
///
/// Protocol clients remain responsible for HTTP, page evidence, and ticket
/// extraction. They call these methods only after a response has been mapped,
/// which keeps authentication transitions atomic and makes expiry handling
/// consistent across services.
pub struct SessionCoordinator {
    registry: SessionRegistry,
}

impl SessionCoordinator {
    pub fn new() -> Self {
        Self {
            registry: SessionRegistry::new(),
        }
    }

    pub fn registry(&self) -> &SessionRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut SessionRegistry {
        &mut self.registry
    }

    pub fn snapshot(&self) -> SessionRegistrySnapshot {
        self.registry.snapshot()
    }

    pub fn begin_authentication(
        &mut self,
        service: ServiceId,
    ) -> Result<SessionSnapshot, SessionError> {
        self.registry
            .transition(service, SessionTransition::BeginAuthentication)
    }

    pub fn require_second_factor(
        &mut self,
        service: ServiceId,
        challenge: SecondFactorChallenge,
        user: Option<UserIdentity>,
    ) -> Result<SessionSnapshot, SessionError> {
        self.registry.transition(
            service,
            SessionTransition::RequireSecondFactor { challenge, user },
        )
    }

    pub fn mark_authenticated(
        &mut self,
        service: ServiceId,
        user: UserIdentity,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<SessionSnapshot, SessionError> {
        self.registry.transition(
            service,
            SessionTransition::AuthenticationSucceeded {
                user,
                ticket,
                csrf,
                expires_at,
            },
        )
    }

    pub fn complete_second_factor(
        &mut self,
        service: ServiceId,
        user: Option<UserIdentity>,
        ticket: Option<BoundServiceTicket>,
        csrf: Option<BoundCsrfToken>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<SessionSnapshot, SessionError> {
        self.registry.transition(
            service,
            SessionTransition::SecondFactorSucceeded {
                user,
                ticket,
                csrf,
                expires_at,
            },
        )
    }

    /// Refreshes an authenticated service's CSRF proof while leaving its
    /// identity, ticket lease, and expiry unchanged.
    pub fn refresh_authenticated_csrf(
        &mut self,
        service: ServiceId,
        csrf: BoundCsrfToken,
    ) -> Result<SessionSnapshot, SessionError> {
        self.registry.refresh_authenticated_csrf(service, csrf)
    }

    pub fn cancel_authentication(
        &mut self,
        service: ServiceId,
    ) -> Result<SessionSnapshot, SessionError> {
        self.registry
            .transition(service, SessionTransition::CancelAuthentication)
    }

    pub fn logout(&mut self, service: ServiceId) -> Result<SessionSnapshot, SessionError> {
        self.registry.transition(service, SessionTransition::Logout)
    }

    pub fn expire_due(&mut self, now: DateTime<Utc>) -> Result<Vec<ServiceId>, SessionError> {
        self.registry.expire_due(now)
    }

    pub fn bound_ticket(&self, service: ServiceId) -> Option<BoundServiceTicket> {
        self.registry.bound_ticket(service)
    }

    /// Claims one service ticket for an outbound request. The claim is
    /// permanent even when the following request fails, so retry code should
    /// use the shared-cookie handoff after receiving `None`.
    pub fn claim_ticket(&mut self, service: ServiceId) -> Option<ClaimedServiceTicket> {
        self.registry.claim_ticket(service)
    }

    pub fn bound_csrf(&self, service: ServiceId) -> Option<BoundCsrfToken> {
        self.registry.bound_csrf(service)
    }
}

impl Default for SessionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ServiceSessionStateMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceSessionStateMachine")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl fmt::Debug for SessionRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionRegistry")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::protocol::SecondFactorMethod;

    fn user() -> UserIdentity {
        UserIdentity {
            username: "student".to_owned(),
            display_name: Some("同学".to_owned()),
        }
    }

    fn challenge(expires_at: Option<DateTime<Utc>>) -> SecondFactorChallenge {
        SecondFactorChallenge {
            methods: vec![SecondFactorMethod::Sms],
            masked_phone: Some("138****0000".to_owned()),
            expires_at,
        }
    }

    fn ticket(machine: &ServiceSessionStateMachine, value: &str) -> BoundServiceTicket {
        machine.bind_ticket(ServiceTicket::new(value).expect("fixture ticket"))
    }

    fn csrf(machine: &ServiceSessionStateMachine, value: &str) -> BoundCsrfToken {
        machine.bind_csrf(CsrfToken::new(value).expect("fixture csrf"))
    }

    #[test]
    fn registry_snapshot_contains_each_service_once() {
        let registry = SessionRegistry::new();
        let snapshot = registry.snapshot();
        let services: HashSet<_> = snapshot
            .sessions
            .iter()
            .map(|session| session.service)
            .collect();

        assert_eq!(snapshot.sessions.len(), REGISTERED_SERVICES.len());
        assert_eq!(services.len(), REGISTERED_SERVICES.len());
    }

    #[test]
    fn rejects_transitions_that_do_not_match_the_current_state() {
        let mut machine = ServiceSessionStateMachine::new(ServiceId::Learn);

        let error = machine
            .mark_authenticated(user(), None, None, None)
            .expect_err("anonymous cannot complete authentication");
        assert_eq!(
            error,
            SessionError::InvalidTransition {
                service: ServiceId::Learn,
                from: ServiceSessionState::Anonymous,
                to: ServiceSessionState::Authenticated,
            }
        );

        machine.begin_authentication().expect("begin auth");
        machine
            .mark_authenticated(user(), None, None, None)
            .expect("primary auth succeeds");

        let error = machine
            .require_second_factor(challenge(None), None)
            .expect_err("an authenticated session cannot request primary 2FA");
        assert_eq!(
            error,
            SessionError::InvalidTransition {
                service: ServiceId::Learn,
                from: ServiceSessionState::Authenticated,
                to: ServiceSessionState::RequiresSecondFactor,
            }
        );

        machine.logout().expect("logout");
        let error = machine
            .mark_expired()
            .expect_err("anonymous has no active session to expire");
        assert_eq!(
            error,
            SessionError::InvalidTransition {
                service: ServiceId::Learn,
                from: ServiceSessionState::Anonymous,
                to: ServiceSessionState::Expired,
            }
        );
    }

    #[test]
    fn rejects_foreign_ticket_and_csrf_bindings_atomically() {
        let mut learn = ServiceSessionStateMachine::new(ServiceId::Learn);
        let registrar = ServiceSessionStateMachine::new(ServiceId::Registrar);
        learn.begin_authentication().expect("begin auth");

        let foreign_ticket =
            registrar.bind_ticket(ServiceTicket::new("registrar-ticket").expect("ticket"));
        let error = learn
            .mark_authenticated(user(), Some(foreign_ticket), None, None)
            .expect_err("foreign ticket must be rejected");
        assert_eq!(
            error,
            SessionError::CredentialServiceMismatch {
                credential: SessionCredentialKind::Ticket,
                expected_service: ServiceId::Learn,
                actual_service: ServiceId::Registrar,
            }
        );
        assert_eq!(learn.state(), ServiceSessionState::Authenticating);
        assert!(!learn.snapshot().ticket_present());

        let foreign_csrf = registrar.bind_csrf(CsrfToken::new("registrar-csrf").expect("csrf"));
        let error = learn
            .mark_authenticated(user(), None, Some(foreign_csrf), None)
            .expect_err("foreign csrf must be rejected");
        assert_eq!(
            error,
            SessionError::CredentialServiceMismatch {
                credential: SessionCredentialKind::Csrf,
                expected_service: ServiceId::Learn,
                actual_service: ServiceId::Registrar,
            }
        );
        assert_eq!(learn.state(), ServiceSessionState::Authenticating);
        assert!(!learn.snapshot().csrf_present());

        let local_ticket = ticket(&learn, "learn-ticket");
        let local_csrf = csrf(&learn, "learn-csrf");
        learn
            .mark_authenticated(user(), Some(local_ticket), Some(local_csrf), None)
            .expect("local credentials are accepted");
        assert_eq!(learn.state(), ServiceSessionState::Authenticated);
    }

    #[test]
    fn refresh_authenticated_csrf_preserves_the_session_identity_ticket_and_expiry() {
        let mut coordinator = SessionCoordinator::new();
        let expiry = DateTime::from_timestamp(4_000, 0).expect("fixture expiry");
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn authentication begins");
        let ticket = coordinator.registry().bind_ticket(
            ServiceId::Learn,
            ServiceTicket::new("one-time-ticket").expect("fixture ticket"),
        );
        let initial_csrf = coordinator.registry().bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("initial-csrf").expect("fixture csrf"),
        );
        coordinator
            .mark_authenticated(
                ServiceId::Learn,
                user(),
                Some(ticket),
                Some(initial_csrf),
                Some(expiry),
            )
            .expect("learn authentication succeeds");
        let before = coordinator.registry().snapshot_for(ServiceId::Learn);

        let latest_csrf = coordinator.registry().bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("latest-csrf").expect("latest fixture csrf"),
        );
        let after = coordinator
            .refresh_authenticated_csrf(ServiceId::Learn, latest_csrf)
            .expect("authenticated Learn CSRF refresh succeeds");

        assert_eq!(after.user, before.user);
        assert_eq!(after.state, ServiceSessionState::Authenticated);
        assert_eq!(after.expires_at, before.expires_at);
        assert_eq!(after.ticket_present(), before.ticket_present());
        assert!(after.csrf_present());
        assert_eq!(
            coordinator
                .bound_csrf(ServiceId::Learn)
                .expect("refreshed csrf")
                .as_csrf_token()
                .as_str(),
            "latest-csrf"
        );
        assert!(coordinator.bound_ticket(ServiceId::Learn).is_some());
    }

    #[test]
    fn refresh_authenticated_csrf_rejects_foreign_or_non_authenticated_sessions() {
        let mut coordinator = SessionCoordinator::new();
        let foreign = coordinator.registry().bind_csrf(
            ServiceId::Registrar,
            CsrfToken::new("registrar-csrf").expect("fixture csrf"),
        );
        let error = coordinator
            .refresh_authenticated_csrf(ServiceId::Learn, foreign)
            .expect_err("anonymous Learn cannot refresh a foreign CSRF");
        assert!(matches!(
            error,
            SessionError::InvalidTransition {
                service: ServiceId::Learn,
                from: ServiceSessionState::Anonymous,
                to: ServiceSessionState::Authenticated,
            }
        ));

        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn authentication begins");
        coordinator
            .mark_authenticated(
                ServiceId::Learn,
                user(),
                None,
                Some(coordinator.registry().bind_csrf(
                    ServiceId::Learn,
                    CsrfToken::new("initial-csrf").expect("fixture csrf"),
                )),
                None,
            )
            .expect("learn authentication succeeds");
        let foreign = coordinator.registry().bind_csrf(
            ServiceId::Registrar,
            CsrfToken::new("registrar-csrf").expect("fixture csrf"),
        );
        let error = coordinator
            .refresh_authenticated_csrf(ServiceId::Learn, foreign)
            .expect_err("foreign CSRF cannot replace Learn proof");
        assert_eq!(
            error,
            SessionError::CredentialServiceMismatch {
                credential: SessionCredentialKind::Csrf,
                expected_service: ServiceId::Learn,
                actual_service: ServiceId::Registrar,
            }
        );
        assert_eq!(
            coordinator
                .bound_csrf(ServiceId::Learn)
                .expect("initial csrf remains")
                .as_csrf_token()
                .as_str(),
            "initial-csrf"
        );
    }

    #[test]
    fn coordinator_routes_transitions_to_one_service_and_expires_due_sessions() {
        let mut coordinator = SessionCoordinator::new();
        let started = coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity auth starts");
        assert_eq!(started.state, ServiceSessionState::Authenticating);

        let expires_at = DateTime::from_timestamp(1_100, 0);
        let waiting = coordinator
            .require_second_factor(ServiceId::Identity, challenge(expires_at), Some(user()))
            .expect("identity can wait for second factor");
        assert_eq!(waiting.state, ServiceSessionState::RequiresSecondFactor);
        assert_eq!(
            coordinator.registry().snapshot_for(ServiceId::Learn).state,
            ServiceSessionState::Anonymous
        );

        let expired = coordinator
            .expire_due(DateTime::from_timestamp(1_101, 0).expect("timestamp"))
            .expect("expiry scan succeeds");
        assert_eq!(expired, vec![ServiceId::Identity]);
        assert_eq!(
            coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .state,
            ServiceSessionState::Expired
        );
    }

    #[test]
    fn ticket_claim_is_one_time_across_registry_and_cloned_bindings() {
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity auth starts");
        let bound = coordinator.registry().bind_ticket(
            ServiceId::Identity,
            ServiceTicket::new("fixture").expect("ticket"),
        );
        let result_clone = bound.clone();
        coordinator
            .mark_authenticated(ServiceId::Identity, user(), Some(bound), None, None)
            .expect("identity auth succeeds");

        let claimed = coordinator
            .claim_ticket(ServiceId::Identity)
            .expect("the first claim succeeds");
        assert_eq!(claimed.service(), ServiceId::Identity);
        assert!(
            !coordinator
                .registry()
                .snapshot_for(ServiceId::Identity)
                .ticket_present()
        );
        assert!(coordinator.bound_ticket(ServiceId::Identity).is_none());
        assert!(coordinator.claim_ticket(ServiceId::Identity).is_none());
        assert!(result_clone.claim().is_none());
    }

    #[test]
    fn ticket_claim_remains_consumed_when_the_following_request_fails() {
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity auth starts");
        let bound = coordinator.registry().bind_ticket(
            ServiceId::Identity,
            ServiceTicket::new("fixture").expect("ticket"),
        );
        coordinator
            .mark_authenticated(ServiceId::Identity, user(), Some(bound), None, None)
            .expect("identity auth succeeds");

        let claimed = coordinator
            .claim_ticket(ServiceId::Identity)
            .expect("the first claim succeeds");
        // Dropping the claimed value models a request that has already been
        // started and then failed at the transport boundary.
        drop(claimed);

        assert!(coordinator.claim_ticket(ServiceId::Identity).is_none());
        assert!(coordinator.bound_ticket(ServiceId::Identity).is_none());
    }

    #[test]
    fn reauthentication_invalidates_old_ticket_and_installs_a_new_claim() {
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity auth starts");
        let old_bound = coordinator.registry().bind_ticket(
            ServiceId::Identity,
            ServiceTicket::new("old").expect("ticket"),
        );
        let old_clone = old_bound.clone();
        coordinator
            .mark_authenticated(ServiceId::Identity, user(), Some(old_bound), None, None)
            .expect("identity auth succeeds");

        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("reauthentication starts");
        assert!(old_clone.claim().is_none());

        let new_bound = coordinator.registry().bind_ticket(
            ServiceId::Identity,
            ServiceTicket::new("new").expect("ticket"),
        );
        coordinator
            .mark_authenticated(ServiceId::Identity, user(), Some(new_bound), None, None)
            .expect("new identity auth succeeds");
        assert!(coordinator.claim_ticket(ServiceId::Identity).is_some());
    }

    #[test]
    fn primary_and_second_factor_success_paths_create_independent_claims() {
        let mut machine = ServiceSessionStateMachine::new(ServiceId::Identity);

        machine.begin_authentication().expect("primary auth starts");
        let primary_ticket = ticket(&machine, "primary");
        machine
            .mark_authenticated(user(), Some(primary_ticket), None, None)
            .expect("primary auth succeeds");
        assert!(machine.claim_ticket().is_some());
        assert!(machine.claim_ticket().is_none());

        machine.logout().expect("logout succeeds");
        machine.begin_authentication().expect("second auth starts");
        machine
            .require_second_factor(challenge(None), None)
            .expect("second factor is required");
        let second_factor_ticket = ticket(&machine, "second-factor");
        machine
            .complete_second_factor(Some(user()), Some(second_factor_ticket), None, None)
            .expect("second factor succeeds");
        assert!(machine.claim_ticket().is_some());
        assert!(machine.claim_ticket().is_none());
    }

    #[test]
    fn expiry_and_logout_invalidate_unclaimed_ticket_handles() {
        let now = Utc::now();
        let mut machine = ServiceSessionStateMachine::new(ServiceId::Identity);

        machine.begin_authentication().expect("auth starts");
        let expiry_ticket = ticket(&machine, "expiry");
        let expiry_clone = expiry_ticket.clone();
        machine
            .mark_authenticated(user(), Some(expiry_ticket), None, Some(now))
            .expect("auth succeeds");
        machine.expire_if_due(now).expect("expiry scan succeeds");
        assert!(expiry_clone.claim().is_none());

        machine.begin_authentication().expect("auth starts again");
        let logout_ticket = ticket(&machine, "logout");
        let logout_clone = logout_ticket.clone();
        machine
            .mark_authenticated(user(), Some(logout_ticket), None, None)
            .expect("auth succeeds again");
        machine.logout().expect("logout succeeds");
        assert!(logout_clone.claim().is_none());
    }

    #[test]
    fn completes_second_factor_and_expires_both_session_kinds() {
        let now = Utc::now();
        let challenge_deadline = now + chrono::Duration::minutes(5);
        let mut machine = ServiceSessionStateMachine::new(ServiceId::Identity);

        machine.begin_authentication().expect("begin auth");
        machine
            .require_second_factor(challenge(Some(challenge_deadline)), None)
            .expect("2FA required");
        assert_eq!(machine.state(), ServiceSessionState::RequiresSecondFactor);
        assert!(machine.snapshot().second_factor.is_some());

        let session_deadline = now + chrono::Duration::hours(1);
        let bound_ticket = ticket(&machine, "identity-ticket");
        let bound_csrf = csrf(&machine, "identity-csrf");
        machine
            .complete_second_factor(
                Some(user()),
                Some(bound_ticket),
                Some(bound_csrf),
                Some(session_deadline),
            )
            .expect("2FA succeeds");
        assert_eq!(machine.state(), ServiceSessionState::Authenticated);
        assert!(machine.snapshot().second_factor.is_none());
        assert!(machine.snapshot().ticket_present());
        assert!(machine.snapshot().csrf_present());

        assert!(
            !machine
                .expire_if_due(now + chrono::Duration::minutes(30))
                .expect("not due")
        );
        assert!(
            machine
                .expire_if_due(session_deadline)
                .expect("session is due")
        );
        assert_eq!(machine.state(), ServiceSessionState::Expired);
        assert!(!machine.snapshot().ticket_present());
        assert!(!machine.snapshot().csrf_present());

        machine.begin_authentication().expect("renew after expiry");
        machine
            .require_second_factor(challenge(Some(now)), Some(user()))
            .expect("2FA required again");
        assert!(
            machine
                .expire_if_due(now)
                .expect("challenge is due immediately")
        );
        assert_eq!(machine.state(), ServiceSessionState::Expired);
        assert!(machine.snapshot().second_factor.is_none());
    }

    #[test]
    fn snapshots_redact_credentials_and_are_safe_for_registry_views() {
        let mut registry = SessionRegistry::new();
        registry
            .transition(ServiceId::Learn, SessionTransition::BeginAuthentication)
            .expect("begin auth");

        let learn_machine = registry.session(ServiceId::Learn);
        let learn_ticket = learn_machine
            .bind_ticket(ServiceTicket::new("very-secret-learn-ticket").expect("ticket"));
        let learn_csrf =
            learn_machine.bind_csrf(CsrfToken::new("very-secret-learn-csrf").expect("csrf"));
        registry
            .transition(
                ServiceId::Learn,
                SessionTransition::AuthenticationSucceeded {
                    user: user(),
                    ticket: Some(learn_ticket),
                    csrf: Some(learn_csrf),
                    expires_at: None,
                },
            )
            .expect("authenticate learn");

        let snapshot = registry.snapshot_for(ServiceId::Learn);
        assert!(snapshot.ticket_present());
        assert!(snapshot.csrf_present());
        let debug = format!("{snapshot:?}");
        assert!(debug.contains("ticket: [redacted]"));
        assert!(debug.contains("csrf: [redacted]"));
        assert!(!debug.contains("very-secret-learn-ticket"));
        assert!(!debug.contains("very-secret-learn-csrf"));
        let encoded = serde_json::to_string(&snapshot).expect("snapshot serializes");
        assert!(encoded.contains("\"has_ticket\":true"));
        assert!(encoded.contains("\"has_csrf\":true"));
        assert!(!encoded.contains("very-secret-learn-ticket"));
        assert!(!encoded.contains("very-secret-learn-csrf"));

        assert_eq!(
            registry.snapshot_for(ServiceId::Registrar).state,
            ServiceSessionState::Anonymous
        );
        assert!(!format!("{:?}", registry).contains("very-secret-learn"));
    }

    #[test]
    fn invalid_challenges_and_identities_do_not_mutate_the_machine() {
        let mut machine = ServiceSessionStateMachine::new(ServiceId::Usereg);
        machine.begin_authentication().expect("begin auth");

        let error = machine
            .require_second_factor(
                SecondFactorChallenge {
                    methods: Vec::new(),
                    masked_phone: None,
                    expires_at: None,
                },
                None,
            )
            .expect_err("an empty method list is not actionable");
        assert_eq!(
            error,
            SessionError::EmptySecondFactorChallenge {
                service: ServiceId::Usereg,
            }
        );
        assert_eq!(machine.state(), ServiceSessionState::Authenticating);

        let error = machine
            .mark_authenticated(
                UserIdentity {
                    username: "  ".to_owned(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .expect_err("an empty username is not an identity");
        assert_eq!(
            error,
            SessionError::EmptyUserName {
                service: ServiceId::Usereg,
            }
        );
        assert_eq!(machine.state(), ServiceSessionState::Authenticating);

        machine
            .require_second_factor(challenge(None), None)
            .expect("2FA required");
        let error = machine
            .complete_second_factor(None, None, None, None)
            .expect_err("completion needs an identity when the challenge has none");
        assert_eq!(
            error,
            SessionError::MissingUserIdentity {
                service: ServiceId::Usereg,
            }
        );
        assert_eq!(machine.state(), ServiceSessionState::RequiresSecondFactor);
    }
}
