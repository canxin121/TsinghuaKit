#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("title must not be empty")]
    EmptyTitle,

    #[error("date range start must be before end")]
    InvalidDateRange,

    #[error("date is outside the representable range")]
    DateOverflow,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    /// Only explicit authentication evidence may invalidate a service session.
    /// Do not classify transport, parsing, business, or rate-limit errors as
    /// expired sessions, and never carry an HTTP body or URL in this variant.
    #[error("{service:?} service session has expired")]
    SessionExpired { service: crate::protocol::ServiceId },

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("{entity} with id {id} was not found")]
    NotFound { entity: &'static str, id: Uuid },

    #[error("in-memory store lock was poisoned")]
    StoragePoisoned,

    #[error("data source adapter failed: {message}")]
    Adapter { message: String },

    #[error(transparent)]
    Learn(#[from] crate::learn::LearnError),

    #[error(transparent)]
    Registrar(#[from] crate::registrar::RegistrarError),

    #[error("TUNet profile is invalid: {0}")]
    TunetProfile(#[source] crate::tunet::ProfileError),

    #[error("TUNet response is invalid: {0}")]
    TunetResponse(#[source] crate::tunet::ResponseDecodeError),

    #[error(transparent)]
    Transport(#[from] crate::transport::TransportError),
}

impl ServiceError {
    pub fn is_session_expired(&self, expected: crate::protocol::ServiceId) -> bool {
        matches!(self, Self::SessionExpired { service } if *service == expected)
    }
}

impl<T> From<std::sync::PoisonError<T>> for ServiceError {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        Self::StoragePoisoned
    }
}
use thiserror::Error;
use uuid::Uuid;
