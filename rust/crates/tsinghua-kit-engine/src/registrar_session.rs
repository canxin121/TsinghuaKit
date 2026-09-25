//! Registrar session handoff from an authenticated learning session.
//!
//! The learning platform issues the ALL_ZHJW ticket, while the registrar owns
//! the follow-up cookie session.  This orchestrator keeps those services
//! separate and promotes the registrar session only after a calendar JSONP
//! response has been decoded successfully.

use chrono::Utc;
use thiserror::Error;

use crate::{
    protocol::{AcademicStage, ServiceId, ServiceSessionState, UserIdentity},
    registrar::CalendarWindow,
    registrar_client::{RegistrarClient, RegistrarClientError, parse_all_zhjw_ticket_response},
    session::{BoundCsrfToken, SessionCoordinator, SessionError, SessionSnapshot},
};

#[cfg(test)]
#[path = "registrar_session_sep19_tests.rs"]
mod sep19_tests;

#[derive(Debug, Error)]
pub enum RegistrarSessionError {
    #[error(transparent)]
    Client(#[from] RegistrarClientError),

    #[error(transparent)]
    Session(#[from] SessionError),

    #[error("the learning CSRF token is not bound to the learning service")]
    ForeignLearnCsrf,

    #[error("the learning session is not authenticated or has no current CSRF token")]
    MissingLearnSession,

    #[error("the supplied learning CSRF token does not belong to the current session")]
    StaleLearnCsrf,

    #[error("the Registrar handoff user does not match the authenticated learning user")]
    UserBindingMismatch,
}

impl RegistrarSessionError {
    /// Fixed diagnostics only: never include URLs, tickets or response bodies.
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        use crate::transport::TransportError;
        match self {
            Self::Client(error) => match error {
                RegistrarClientError::LoginExpired { .. } => "registrar_session_expired",
                RegistrarClientError::UnexpectedOrigin => "registrar_origin_rejected",
                RegistrarClientError::InvalidTicketResponse { .. }
                | RegistrarClientError::EmptyTicket => "registrar_ticket_invalid",
                RegistrarClientError::Transport(TransportError::HttpStatus { .. }) => {
                    "registrar_http"
                }
                RegistrarClientError::Transport(TransportError::Request(_)) => "registrar_network",
                RegistrarClientError::Transport(TransportError::InvalidUrl(_)) => {
                    "registrar_config"
                }
                RegistrarClientError::Transport(TransportError::Jsonp(_)) => {
                    "registrar_calendar_jsonp"
                }
                RegistrarClientError::Transport(
                    TransportError::Decode(_) | TransportError::DecodeBody { .. },
                ) => "registrar_response_decode",
                RegistrarClientError::InvalidJsonp { .. } => "registrar_calendar_jsonp",
                RegistrarClientError::CalendarHtmlContentType
                | RegistrarClientError::InvalidCalendarContentType => {
                    "registrar_calendar_content_type"
                }
                RegistrarClientError::InvalidBusinessPayload { .. } => "registrar_calendar_payload",
                _ => "registrar_config",
            },
            _ => "registrar_session_binding",
        }
    }
}

/// The result of a registrar handoff after the calendar proof succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarSessionResult {
    pub snapshot: SessionSnapshot,
}

/// Executes the learning-to-registrar handoff using one shared cookie-aware
/// [`RegistrarClient`] transport.
pub struct RegistrarSessionOrchestrator {
    registrar: RegistrarClient,
}

impl RegistrarSessionOrchestrator {
    pub fn new(registrar: RegistrarClient) -> Self {
        Self { registrar }
    }

    pub fn registrar(&self) -> &RegistrarClient {
        &self.registrar
    }

    /// Returns the configured registrar client after a handoff so a higher
    /// level runtime can keep using the same cookie jar for live reads.
    pub fn into_registrar(self) -> RegistrarClient {
        self.registrar
    }

    /// Exchanges the learning CSRF-bound session for a registrar ticket,
    /// establishes the registrar cookie session, and verifies it with one
    /// calendar JSONP request before updating the shared coordinator.
    pub async fn establish(
        &self,
        coordinator: &mut SessionCoordinator,
        learn_csrf: &BoundCsrfToken,
        user: UserIdentity,
        stage: AcademicStage,
        window: CalendarWindow,
        callback: &str,
    ) -> Result<RegistrarSessionResult, RegistrarSessionError> {
        self.verify_learning_session(coordinator, learn_csrf, &user)?;

        // Validate all caller-controlled wire inputs before changing the
        // Registrar state or making the Learn ticket exchange. This catches a
        // public, manually constructed invalid window as well as an invalid
        // callback without consuming a one-time ticket.
        self.registrar
            .calendar_request_plan(stage, window.clone(), callback.to_owned())?;
        coordinator.begin_authentication(ServiceId::Registrar)?;

        let result = self
            .establish_inner(coordinator, learn_csrf, user, stage, window, callback)
            .await;
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                let _ = coordinator.cancel_authentication(ServiceId::Registrar);
                Err(error)
            }
        }
    }

    async fn establish_inner(
        &self,
        coordinator: &mut SessionCoordinator,
        learn_csrf: &BoundCsrfToken,
        user: UserIdentity,
        stage: AcademicStage,
        window: CalendarWindow,
        callback: &str,
    ) -> Result<RegistrarSessionResult, RegistrarSessionError> {
        let raw_ticket = self
            .registrar
            .exchange_all_zhjw_ticket(Some(learn_csrf.as_csrf_token().as_str()))
            .await?;
        let ticket = parse_all_zhjw_ticket_response(&raw_ticket).map_err(|source| {
            RegistrarSessionError::Client(RegistrarClientError::InvalidTicketResponse { source })
        })?;

        self.registrar
            .establish_registrar_session(ticket.as_str())
            .await?;

        // A registrar login page can still return HTTP 200.  Treat successful
        // calendar JSONP decoding as the proof boundary instead of inferring
        // authentication from that intermediate HTML response.
        self.registrar
            .fetch_calendar_window(stage, window, callback)
            .await?;

        let snapshot =
            coordinator.mark_authenticated(ServiceId::Registrar, user, None, None, None)?;
        Ok(RegistrarSessionResult { snapshot })
    }

    fn verify_learning_session(
        &self,
        coordinator: &SessionCoordinator,
        learn_csrf: &BoundCsrfToken,
        user: &UserIdentity,
    ) -> Result<(), RegistrarSessionError> {
        if learn_csrf.service() != ServiceId::Learn {
            return Err(RegistrarSessionError::ForeignLearnCsrf);
        }

        let snapshot = coordinator.registry().snapshot_for(ServiceId::Learn);
        if snapshot.state != ServiceSessionState::Authenticated
            || !snapshot.csrf_present()
            || snapshot
                .expires_at
                .is_some_and(|expires_at| expires_at <= Utc::now())
        {
            return Err(RegistrarSessionError::MissingLearnSession);
        }

        // The ALL_ZHJW ticket is minted from the Learn session.  Comparing
        // the requested identity with that session before making any network
        // request prevents a caller from attaching a valid registrar cookie
        // proof to a different in-memory user record.  The username is the
        // stable account key; display names can legitimately change.
        let Some(bound_user) = snapshot.user.as_ref() else {
            return Err(RegistrarSessionError::MissingLearnSession);
        };
        if bound_user.username != user.username {
            return Err(RegistrarSessionError::UserBindingMismatch);
        }

        let Some(current) = coordinator.bound_csrf(ServiceId::Learn) else {
            return Err(RegistrarSessionError::MissingLearnSession);
        };
        if current.as_csrf_token() != learn_csrf.as_csrf_token() {
            return Err(RegistrarSessionError::StaleLearnCsrf);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use chrono::{Duration, Utc};

    use super::*;
    use crate::{
        protocol::{CsrfToken, ServiceSessionState},
        registrar_client::{RegistrarClientConfig, RegistrarClientError},
        transport::CampusHttpTransport,
    };

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
                line.strip_prefix("Content-Length:")?
                    .trim()
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("request body");
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn write_response(stream: &mut std::net::TcpStream, content_type: &str, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("response");
    }

    #[tokio::test]
    async fn promotes_registrar_only_after_calendar_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for expected_path in [
                "/b/wlxt/common/auth/gnt",
                "/j_acegi_login.do",
                "/jxmh_out.do",
            ] {
                let (mut stream, _) = listener.accept().expect("connection");
                let request = read_request(&mut stream);
                assert!(request.starts_with(if expected_path.ends_with("gnt") {
                    "POST "
                } else {
                    "GET "
                }));
                assert!(request.contains(expected_path));

                if expected_path.ends_with("gnt") {
                    assert!(request.contains("appId=ALL_ZHJW"));
                    write_response(&mut stream, "text/plain", "\"registrar-ticket\"");
                } else if expected_path.ends_with("j_acegi_login.do") {
                    assert!(request.contains("ticket=registrar-ticket"));
                    write_response(&mut stream, "text/html", "<html>logged</html>");
                } else {
                    write_response(&mut stream, "application/javascript", "fixtureCb([])");
                }
            }
        });

        let base_url = format!("http://{address}");
        let config = RegistrarClientConfig {
            learn_base_url: base_url.clone(),
            registrar_base_url: base_url,
            ..RegistrarClientConfig::default()
        };
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let registrar =
            RegistrarClient::from_transport(config, transport).expect("registrar client");
        let orchestrator = RegistrarSessionOrchestrator::new(registrar);

        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: Some("同学".to_owned()),
        };
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn auth begins");
        let learn_csrf = coordinator.registry().bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("learn-csrf").expect("csrf"),
        );
        coordinator
            .mark_authenticated(
                ServiceId::Learn,
                user.clone(),
                None,
                Some(learn_csrf.clone()),
                None,
            )
            .expect("learn session authenticates");

        let result = orchestrator
            .establish(
                &mut coordinator,
                &learn_csrf,
                user,
                AcademicStage::Undergraduate,
                CalendarWindow::new(
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("date"),
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("date"),
                )
                .expect("window"),
                "fixtureCb",
            )
            .await
            .expect("registrar handoff");

        assert_eq!(result.snapshot.service, ServiceId::Registrar);
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert_eq!(
            coordinator.registry().snapshot_for(ServiceId::Learn).state,
            ServiceSessionState::Authenticated
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn malformed_ticket_does_not_promote_registrar() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let _request = read_request(&mut stream);
            write_response(&mut stream, "application/json", "{\"error\":\"login\"}");
        });

        let base_url = format!("http://{address}");
        let config = RegistrarClientConfig {
            learn_base_url: base_url.clone(),
            registrar_base_url: base_url,
            ..RegistrarClientConfig::default()
        };
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let registrar =
            RegistrarClient::from_transport(config, transport).expect("registrar client");
        let orchestrator = RegistrarSessionOrchestrator::new(registrar);
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn auth begins");
        let learn_csrf = coordinator.registry().bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("learn-csrf").expect("csrf"),
        );
        coordinator
            .mark_authenticated(
                ServiceId::Learn,
                UserIdentity {
                    username: "student".to_owned(),
                    display_name: None,
                },
                None,
                Some(learn_csrf.clone()),
                None,
            )
            .expect("learn session authenticates");

        let error = orchestrator
            .establish(
                &mut coordinator,
                &learn_csrf,
                UserIdentity {
                    username: "student".to_owned(),
                    display_name: None,
                },
                AcademicStage::Undergraduate,
                CalendarWindow::new(
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("date"),
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("date"),
                )
                .expect("window"),
                "fixtureCb",
            )
            .await
            .expect_err("structured error must not become a ticket");
        assert!(matches!(
            error,
            RegistrarSessionError::Client(RegistrarClientError::InvalidTicketResponse { .. })
        ));
        assert_eq!(
            coordinator
                .registry()
                .snapshot_for(ServiceId::Registrar)
                .state,
            ServiceSessionState::Anonymous
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn refuses_an_expired_learning_session_before_ticket_exchange() {
        let registrar = RegistrarClient::new(RegistrarClientConfig::default()).expect("client");
        let orchestrator = RegistrarSessionOrchestrator::new(registrar);
        let mut coordinator = SessionCoordinator::new();
        let user = UserIdentity {
            username: "student".to_owned(),
            display_name: None,
        };
        coordinator
            .begin_authentication(ServiceId::Learn)
            .expect("learn auth begins");
        let learn_csrf = coordinator.registry().bind_csrf(
            ServiceId::Learn,
            CsrfToken::new("learn-csrf").expect("csrf"),
        );
        coordinator
            .mark_authenticated(
                ServiceId::Learn,
                user.clone(),
                None,
                Some(learn_csrf.clone()),
                Some(Utc::now() - Duration::minutes(1)),
            )
            .expect("learn session authenticates");

        let error = orchestrator
            .establish(
                &mut coordinator,
                &learn_csrf,
                user,
                AcademicStage::Undergraduate,
                CalendarWindow::new(
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("date"),
                    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("date"),
                )
                .expect("window"),
                "fixtureCb",
            )
            .await
            .expect_err("expired Learn session must not exchange a ticket");
        assert!(matches!(error, RegistrarSessionError::MissingLearnSession));
        assert_eq!(
            coordinator
                .registry()
                .snapshot_for(ServiceId::Registrar)
                .state,
            ServiceSessionState::Anonymous
        );
    }
}
