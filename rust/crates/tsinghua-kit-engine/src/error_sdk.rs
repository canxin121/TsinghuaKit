//! Stable, redacted errors exposed by the public SDK.

use std::{fmt, time::Duration};

use uuid::Uuid;

use crate::auth::AuthDomain;

/// A service boundary understood by the public SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Service {
    /// An operation in one account-authentication domain.
    Auth(AuthDomain),
    /// Network-learning courses, assignments, and related content.
    Learn,
    /// Registration-office schedules, grades, and examinations.
    Registrar,
    /// School-wide calendar data.
    Calendar,
    /// INFO news and subscriptions.
    News,
    /// The online service hall and its workflow tasks.
    ServiceHall,
    /// Read-only data owned by the independent network self-service account.
    SelfService,
    /// Library locations, hours, seats, and sockets.
    Library,
    /// Classroom availability.
    Classrooms,
    /// Campus-card account and transaction reads.
    CampusCard,
    /// Dormitory electricity data.
    Electricity,
    /// Local campus-network observation and connection operations.
    Network,
    /// Local SDK storage or client lifecycle.
    Local,
}

impl Service {
    /// Returns the stable, lowercase identifier for this service boundary.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth(AuthDomain::Identity) => "identity",
            Self::Auth(AuthDomain::SelfService) => "self_service_auth",
            Self::Learn => "learn",
            Self::Registrar => "registrar",
            Self::Calendar => "calendar",
            Self::News => "news",
            Self::ServiceHall => "service_hall",
            Self::SelfService => "self_service",
            Self::Library => "library",
            Self::Classrooms => "classrooms",
            Self::CampusCard => "campus_card",
            Self::Electricity => "electricity",
            Self::Network => "network",
            Self::Local => "local",
        }
    }
}

impl fmt::Display for Service {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A stable, credential-free error identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// No usable session has been established for the requested account.
    SessionRequired,
    /// The relevant session expired according to explicit service evidence.
    SessionExpired,
    /// The service requires a user interaction before it can continue.
    InteractionRequired,
    /// The service explicitly rejected credentials or a submitted challenge.
    AuthenticationRejected,
    /// The response belongs to a different account or selection context.
    ContextMismatch,
    /// Another one-shot authentication or mutation interaction is in progress.
    InteractionInProgress,
    /// The request could not reach its service.
    NetworkUnavailable,
    /// The service rejected or throttled requests.
    RateLimited,
    /// The service is temporarily unavailable.
    ServiceUnavailable,
    /// A caller-provided value is invalid for this operation.
    InvalidInput,
    /// The response did not match the expected verified structure.
    InvalidResponse,
    /// A one-shot operation may have been consumed but its result is unknown.
    OutcomeUnconfirmed,
    /// A read returned values without proving complete coverage.
    IncompleteResult,
    /// The requested cache-only read has no eligible cached value.
    CacheMiss,
    /// The requested operation is unsupported by this service or host.
    Unsupported,
    /// The local storage boundary is unavailable or could not be verified.
    StorageUnavailable,
    /// The operation was explicitly cancelled.
    Cancelled,
    /// A failure could not be assigned a narrower stable category.
    Internal,
}

impl ErrorCode {
    /// Returns the stable, lowercase identifier suitable for telemetry keys.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionRequired => "session_required",
            Self::SessionExpired => "session_expired",
            Self::InteractionRequired => "interaction_required",
            Self::AuthenticationRejected => "authentication_rejected",
            Self::ContextMismatch => "context_mismatch",
            Self::InteractionInProgress => "interaction_in_progress",
            Self::NetworkUnavailable => "network_unavailable",
            Self::RateLimited => "rate_limited",
            Self::ServiceUnavailable => "service_unavailable",
            Self::InvalidInput => "invalid_input",
            Self::InvalidResponse => "invalid_response",
            Self::OutcomeUnconfirmed => "outcome_unconfirmed",
            Self::IncompleteResult => "incomplete_result",
            Self::CacheMiss => "cache_miss",
            Self::Unsupported => "unsupported",
            Self::StorageUnavailable => "storage_unavailable",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

/// A structured error containing no raw request, response, or credential.
///
/// `Display` and `Debug` expose only a service, stable code, retry delay, and
/// random diagnostic identifier. The internal protocol error is not retained
/// as a source because it may contain private URLs or response content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    service: Service,
    code: ErrorCode,
    retry_after: Option<Duration>,
    diagnostic_id: Uuid,
}

impl Error {
    pub(crate) fn new(service: Service, code: ErrorCode) -> Self {
        Self {
            service,
            code,
            retry_after: None,
            diagnostic_id: Uuid::new_v4(),
        }
    }

    /// Returns the affected public service boundary.
    pub fn service(&self) -> Service {
        self.service
    }

    /// Returns the stable error code.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns a server-provided or locally enforced wait duration, if known.
    /// This is advice to the caller; it never authorizes replaying a one-shot
    /// authentication or network operation.
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    /// Returns a random identifier that can correlate a redacted diagnostic.
    pub fn diagnostic_id(&self) -> Uuid {
        self.diagnostic_id
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}",
            self.service.as_str(),
            self.code.as_str()
        )
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_and_debug_are_redacted() {
        let error = Error::new(Service::Network, ErrorCode::OutcomeUnconfirmed);
        let rendered = format!("{error} {error:?}");

        assert!(rendered.contains("network:outcome_unconfirmed"));
        assert!(!rendered.contains("cookie"));
        assert!(!rendered.contains("password"));
    }
}
