//! Shared protocol-facing types used by service adapters.
//!
//! These types deliberately describe session state and service capabilities without
//! knowing any endpoint, HTML selector, or wire-specific field name. Concrete clients
//! can translate them to their own request profiles while the rest of the application
//! keeps a stable vocabulary.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceId {
    Identity,
    Learn,
    Registrar,
    Info,
    Usereg,
    Tunet,
    Library,
    CampusCard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceSessionState {
    Anonymous,
    Authenticating,
    RequiresSecondFactor,
    Authenticated,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CourseRole {
    Student,
    Teacher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcademicStage {
    Undergraduate,
    Graduate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecondFactorMethod {
    /// The identity service's current phone-code wire value.
    Mobile,
    /// Legacy alias retained for deployments that explicitly advertise SMS.
    Sms,
    Totp,
    Wechat,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ServiceTicket(String);

impl ServiceTicket {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ServiceTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ServiceTicket")
            .field(&"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CsrfToken(String);

impl CsrfToken {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CsrfToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CsrfToken")
            .field(&"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentity {
    pub username: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondFactorChallenge {
    pub methods: Vec<SecondFactorMethod>,
    pub masked_phone: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSession {
    pub service: ServiceId,
    pub user: Option<UserIdentity>,
    pub state: ServiceSessionState,
    pub(crate) ticket: Option<ServiceTicket>,
    pub(crate) csrf: Option<CsrfToken>,
    pub second_factor: Option<SecondFactorChallenge>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl ServiceSession {
    pub fn anonymous(service: ServiceId) -> Self {
        Self {
            service,
            user: None,
            state: ServiceSessionState::Anonymous,
            ticket: None,
            csrf: None,
            second_factor: None,
            expires_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_like_tokens_reject_empty_values_and_redact_debug_output() {
        assert!(ServiceTicket::new("  ").is_none());
        assert!(CsrfToken::new("").is_none());

        let ticket = ServiceTicket::new("ticket-value").expect("ticket exists");
        let csrf = CsrfToken::new("csrf-value").expect("csrf exists");
        assert_eq!(ticket.as_str(), "ticket-value");
        assert_eq!(csrf.as_str(), "csrf-value");
        assert!(!format!("{ticket:?}").contains("ticket-value"));
        assert!(!format!("{csrf:?}").contains("csrf-value"));
    }

    #[test]
    fn service_session_debug_redacts_credentials() {
        let session = ServiceSession {
            service: ServiceId::Learn,
            user: Some(UserIdentity {
                username: "student".to_owned(),
                display_name: Some("同学".to_owned()),
            }),
            state: ServiceSessionState::Authenticated,
            ticket: ServiceTicket::new("ticket-value"),
            csrf: CsrfToken::new("csrf-value"),
            second_factor: None,
            expires_at: None,
        };

        let debug = format!("{session:?}");
        assert!(!debug.contains("ticket-value"));
        assert!(!debug.contains("csrf-value"));
    }
}
