//! Learning-platform session handoff from unified identity authentication.
//!
//! The identity ticket is accepted only as a one-time roaming input. The
//! resulting learning CSRF token is bound to `ServiceId::Learn` before it is
//! installed into the shared session coordinator. A 200 response without
//! reliable CSRF evidence never becomes an authenticated session.

use chrono::Utc;
use reqwest::StatusCode;
use thiserror::Error;

use crate::{
    learn_client::{LearnClient, LearnClientError, LearnCurrentSemester, LearnPageClassification},
    protocol::{ServiceId, ServiceSessionState, ServiceTicket, UserIdentity},
    session::{
        BoundCsrfToken, BoundServiceTicket, SessionCoordinator, SessionError, SessionSnapshot,
    },
    transport::CampusHttpTransport,
};

#[derive(Debug, Error)]
pub enum LearnSessionError {
    #[error(transparent)]
    Client(#[from] LearnClientError),

    #[error(transparent)]
    Session(#[from] SessionError),

    #[error("learning platform returned HTTP {status}")]
    HttpStatus { status: StatusCode },

    #[error("learning platform session has expired")]
    LoginExpired,

    #[error("learning platform did not provide CSRF evidence")]
    MissingCsrf,

    #[error("learning platform response could not establish a session")]
    UnexpectedPage,

    #[error("learning platform session is not authenticated for semester discovery")]
    MissingLearnSession,

    #[error("learning platform session user does not match semester discovery user")]
    UserBindingMismatch,

    #[error("learning platform current semester is not present in its available semester list")]
    SemesterMismatch,

    #[error("identity roaming ticket is not bound to the identity service")]
    ForeignRoamingTicket,
}

/// A safe result of the learning roaming handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnSessionResult {
    pub snapshot: SessionSnapshot,
}

/// The result of a semester discovery transaction.
///
/// The returned CSRF value is always bound to `ServiceId::Learn` and comes from
/// the course home fetched after both semester endpoints have completed.  A
/// caller that constructs course or Registrar adapters must use this value
/// rather than retaining the token from an earlier handoff page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnSemesterDiscovery {
    pub current: LearnCurrentSemester,
    pub semester_ids: Vec<String>,
    pub final_learn_csrf: BoundCsrfToken,
}

/// Executes the identity-to-learning roaming handoff and updates one service
/// state machine. The transport can be cloned into a registrar client so the
/// resulting Cookie jar is shared across the next handoff.
pub struct LearnSessionOrchestrator {
    learn: LearnClient,
    transport: CampusHttpTransport,
    coordinator: SessionCoordinator,
}

impl LearnSessionOrchestrator {
    pub fn new(learn: LearnClient, transport: CampusHttpTransport) -> Self {
        Self {
            learn,
            transport,
            coordinator: SessionCoordinator::new(),
        }
    }

    pub fn with_coordinator(
        learn: LearnClient,
        transport: CampusHttpTransport,
        coordinator: SessionCoordinator,
    ) -> Self {
        Self {
            learn,
            transport,
            coordinator,
        }
    }

    pub fn learn(&self) -> &LearnClient {
        &self.learn
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn coordinator(&self) -> &SessionCoordinator {
        &self.coordinator
    }

    pub fn coordinator_mut(&mut self) -> &mut SessionCoordinator {
        &mut self.coordinator
    }

    /// Performs one identity-ticket roaming request and authenticates the
    /// learning service only after CSRF evidence is extracted.
    pub async fn establish(
        &mut self,
        identity_ticket: &BoundServiceTicket,
        user: UserIdentity,
    ) -> Result<LearnSessionResult, LearnSessionError> {
        if identity_ticket.service() != ServiceId::Identity {
            return Err(LearnSessionError::ForeignRoamingTicket);
        }

        self.coordinator.begin_authentication(ServiceId::Learn)?;

        // Claim the one-time ticket before starting I/O. If another attempt
        // already claimed it, the shared identity Cookie jar is the only safe
        // retry path; never send the same ticket twice.
        let roaming_result = match identity_ticket.claim() {
            Some(claimed_ticket) => {
                self.learn
                    .execute_roaming(&self.transport, claimed_ticket.as_str())
                    .await
            }
            None => self.learn.execute_cookie_roaming(&self.transport).await,
        };
        let result = match roaming_result {
            Ok(response) => self.establish_after_roaming(response, user).await,
            Err(error) => Err(map_client_error(error)),
        };
        self.finish(result)
    }

    /// Performs the same Learn proof using the Cookie-backed handoff emitted
    /// by the current identity success page.  The Cookie jar is deliberately
    /// supplied by the shared transport; this method does not accept a raw
    /// ticket and therefore cannot accidentally turn a ticketless handoff
    /// into a made-up service credential.
    pub async fn establish_cookie_backed(
        &mut self,
        user: UserIdentity,
    ) -> Result<LearnSessionResult, LearnSessionError> {
        self.coordinator.begin_authentication(ServiceId::Learn)?;

        let result = match self.learn.execute_cookie_roaming(&self.transport).await {
            Ok(response) => self.establish_after_roaming(response, user).await,
            Err(error) => Err(map_client_error(error)),
        };
        self.finish(result)
    }

    /// Discovers the current and available semesters and refreshes the Learn
    /// CSRF proof after discovery has completed.
    ///
    /// The existing Learn client intentionally fetches a course home before
    /// each semester AJAX request.  Those page reads may rotate the page CSRF
    /// value, so the values used for the AJAX calls are not sufficient for a
    /// later course or Registrar request.  This method performs one final
    /// course-home proof, commits that value to the Learn session, and returns
    /// the same service-bound value for downstream adapters.
    pub async fn discover_semesters(
        &mut self,
        user: UserIdentity,
    ) -> Result<LearnSemesterDiscovery, LearnSessionError> {
        self.verify_authenticated_learn_user(&user)?;

        let result = self.discover_semesters_inner(user).await;
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                self.invalidate_learn_session();
                Err(error)
            }
        }
    }

    async fn discover_semesters_inner(
        &mut self,
        _user: UserIdentity,
    ) -> Result<LearnSemesterDiscovery, LearnSessionError> {
        let (current, _current_csrf) = self
            .learn
            .fetch_current_semester_with_csrf(&self.transport)
            .await
            .map_err(map_client_error)?;
        let (semester_ids, latest_csrf) = self
            .learn
            .fetch_semester_ids_with_csrf(&self.transport)
            .await
            .map_err(map_client_error)?;

        if !semester_ids.iter().any(|value| value == &current.id) {
            return Err(LearnSessionError::SemesterMismatch);
        }

        // The semester-list course home is the latest page read in this
        // transaction. Its token is the only page token allowed to become the
        // downstream Learn proof; do not perform another page read and then
        // accidentally let a later or older token diverge from this result.
        let final_learn_csrf = self
            .coordinator
            .registry()
            .bind_csrf(ServiceId::Learn, latest_csrf);
        self.replace_authenticated_learn_csrf(final_learn_csrf.clone())?;

        Ok(LearnSemesterDiscovery {
            current,
            semester_ids,
            final_learn_csrf,
        })
    }

    async fn establish_after_roaming(
        &mut self,
        response: crate::learn_client::LearnHttpResponse,
        user: UserIdentity,
    ) -> Result<LearnSessionResult, LearnSessionError> {
        let roaming = self
            .learn
            .map_http_response(&response)
            .map_err(map_client_error)?;

        if matches!(
            roaming.classification,
            LearnPageClassification::LoginExpired(_)
        ) {
            return Err(LearnSessionError::LoginExpired);
        }
        if response.status != StatusCode::OK {
            return Err(LearnSessionError::HttpStatus {
                status: response.status,
            });
        }

        // The roaming response is only a handoff signal. Even if it happens
        // to contain a CSRF-looking value, the authenticated course home is
        // the session proof used by current Learn clients and is fetched on
        // every handoff.
        let home = match self.learn.execute_course_home(&self.transport).await {
            Ok(response) => response,
            Err(error) => return Err(map_client_error(error)),
        };
        let bound_csrf = self.bound_csrf_from_course_home(&home)?;
        let snapshot = match self.coordinator.mark_authenticated(
            ServiceId::Learn,
            user,
            None,
            Some(bound_csrf),
            None,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => return Err(error.into()),
        };

        Ok(LearnSessionResult { snapshot })
    }

    fn bound_csrf_from_course_home(
        &self,
        home: &crate::learn_client::LearnHttpResponse,
    ) -> Result<BoundCsrfToken, LearnSessionError> {
        let mapped = self
            .learn
            .map_http_response(home)
            .map_err(map_client_error)?;

        if matches!(
            mapped.classification,
            LearnPageClassification::LoginExpired(_)
        ) {
            return Err(LearnSessionError::LoginExpired);
        }
        if home.status != StatusCode::OK {
            return Err(LearnSessionError::HttpStatus {
                status: home.status,
            });
        }
        if !matches!(
            mapped.classification,
            LearnPageClassification::Authenticated
        ) {
            return Err(LearnSessionError::UnexpectedPage);
        }

        let csrf = mapped.csrf.ok_or(LearnSessionError::MissingCsrf)?;
        Ok(self
            .coordinator
            .registry()
            .bind_csrf(ServiceId::Learn, csrf.token))
    }

    fn verify_authenticated_learn_user(
        &self,
        user: &UserIdentity,
    ) -> Result<(), LearnSessionError> {
        let snapshot = self.coordinator.registry().snapshot_for(ServiceId::Learn);
        if snapshot.state != ServiceSessionState::Authenticated || !snapshot.csrf_present() {
            return Err(LearnSessionError::MissingLearnSession);
        }
        if snapshot
            .expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
        {
            return Err(LearnSessionError::LoginExpired);
        }
        if snapshot.user.as_ref() != Some(user) {
            return Err(LearnSessionError::UserBindingMismatch);
        }
        Ok(())
    }

    fn replace_authenticated_learn_csrf(
        &mut self,
        csrf: BoundCsrfToken,
    ) -> Result<SessionSnapshot, LearnSessionError> {
        if csrf.service() != ServiceId::Learn {
            return Err(LearnSessionError::ForeignRoamingTicket);
        }
        self.coordinator
            .refresh_authenticated_csrf(ServiceId::Learn, csrf)
            .map_err(Into::into)
    }

    fn finish(
        &mut self,
        result: Result<LearnSessionResult, LearnSessionError>,
    ) -> Result<LearnSessionResult, LearnSessionError> {
        if result.is_err() {
            self.cancel_after_failure();
        }
        result
    }

    fn cancel_after_failure(&mut self) {
        let _ = self.coordinator.cancel_authentication(ServiceId::Learn);
    }

    fn invalidate_learn_session(&mut self) {
        if self
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Learn)
            .state
            != ServiceSessionState::Anonymous
        {
            let _ = self.coordinator.logout(ServiceId::Learn);
        }
    }
}

fn map_client_error(error: LearnClientError) -> LearnSessionError {
    if matches!(error, LearnClientError::SessionExpired) {
        LearnSessionError::LoginExpired
    } else {
        LearnSessionError::Client(error)
    }
}

impl From<LearnSessionError> for crate::error::ServiceError {
    fn from(error: LearnSessionError) -> Self {
        crate::error::ServiceError::Adapter {
            message: error.to_string(),
        }
    }
}

/// Keeps `ServiceTicket` in the public module's type vocabulary for callers
/// that need to construct a bound identity ticket at the orchestration edge.
pub fn bind_identity_ticket(
    coordinator: &SessionCoordinator,
    ticket: ServiceTicket,
) -> BoundServiceTicket {
    coordinator
        .registry()
        .bind_ticket(ServiceId::Identity, ticket)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;
    use crate::{
        learn_client::LearnClientConfig,
        protocol::{CourseRole, ServiceId},
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

        String::from_utf8(request).expect("HTTP request is UTF-8")
    }

    #[test]
    fn identity_ticket_helper_binds_only_the_identity_service() {
        let coordinator = SessionCoordinator::new();
        let ticket = ServiceTicket::new("opaque-ticket").expect("ticket");
        let bound = bind_identity_ticket(&coordinator, ticket);
        assert_eq!(bound.service(), ServiceId::Identity);
        assert!(!format!("{bound:?}").contains("opaque-ticket"));

        let learn = LearnClient::new(
            LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
                .expect("learn config"),
        );
        assert_eq!(learn.config().role(), CourseRole::Student);
    }

    #[tokio::test]
    async fn roaming_fetches_course_home_before_promoting_the_learning_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for body in [
                br#"<html><body>redirect shell</body></html>"#.as_slice(),
                br#"<script>const courseUrl = "/f/wlxt/index/course/student/index?_csrf=learn-csrf";</script>"#.as_slice(),
            ] {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 256];
                loop {
                    let read = stream.read(&mut buffer).expect("request");
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("headers");
                stream.write_all(body).expect("body");
            }
        });

        let base_url = format!("http://{address}/");
        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let mut orchestrator = LearnSessionOrchestrator::new(learn, transport);
        let ticket = bind_identity_ticket(
            orchestrator.coordinator(),
            ServiceTicket::new("identity-ticket").expect("ticket"),
        );
        let result = orchestrator
            .establish(
                &ticket,
                UserIdentity {
                    username: "student".to_owned(),
                    display_name: Some("同学".to_owned()),
                },
            )
            .await
            .expect("roaming succeeds");

        assert_eq!(result.snapshot.service, ServiceId::Learn);
        assert_eq!(
            result.snapshot.state,
            crate::protocol::ServiceSessionState::Authenticated
        );
        assert!(result.snapshot.csrf_present());
        assert_eq!(
            orchestrator
                .coordinator()
                .bound_csrf(ServiceId::Learn)
                .expect("csrf binding")
                .as_csrf_token()
                .as_str(),
            "learn-csrf"
        );
        assert_eq!(
            orchestrator
                .coordinator()
                .registry()
                .snapshot_for(ServiceId::Registrar)
                .state,
            crate::protocol::ServiceSessionState::Anonymous
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn roaming_page_csrf_does_not_replace_course_home_session_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for body in [
                br#"<meta name="_csrf" content="roaming-only-csrf">"#.as_slice(),
                br#"<html><body>authenticated shell without page proof</body></html>"#.as_slice(),
            ] {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 256];
                loop {
                    let read = stream.read(&mut buffer).expect("request");
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("headers");
                stream.write_all(body).expect("body");
            }
        });

        let base_url = format!("http://{address}/");
        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let mut orchestrator = LearnSessionOrchestrator::new(learn, transport);
        let ticket = bind_identity_ticket(
            orchestrator.coordinator(),
            ServiceTicket::new("identity-ticket").expect("ticket"),
        );

        let error = orchestrator
            .establish(
                &ticket,
                UserIdentity {
                    username: "student".to_owned(),
                    display_name: None,
                },
            )
            .await
            .expect_err("course home must provide the proof");
        assert!(matches!(error, LearnSessionError::UnexpectedPage));
        assert_eq!(
            orchestrator
                .coordinator()
                .registry()
                .snapshot_for(ServiceId::Learn)
                .state,
            crate::protocol::ServiceSessionState::Anonymous
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn cookie_backed_roaming_reuses_identity_cookie_and_proves_course_home() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for (expected_path, body) in [
                (
                    "/f/j_spring_security_thauth_roaming_entry",
                    "<html><body>cookie handoff</body></html>",
                ),
                (
                    "/f/wlxt/index/course/student/index",
                    "<meta name=\"_csrf\" content=\"cookie-csrf\">",
                ),
            ] {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 512];
                loop {
                    let read = stream.read(&mut buffer).expect("request");
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                assert!(request.starts_with(&format!("GET {expected_path}")));
                assert!(request.contains("identity-session=present"));
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
        });

        let base_url = format!("http://{address}/");
        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        transport.cookie_jar().add_cookie_str(
            "identity-session=present; Path=/",
            &reqwest::Url::parse(&base_url).expect("base URL"),
        );
        let mut orchestrator = LearnSessionOrchestrator::new(learn, transport);
        let result = orchestrator
            .establish_cookie_backed(UserIdentity {
                username: "student".to_owned(),
                display_name: None,
            })
            .await
            .expect("cookie-backed handoff succeeds");

        assert_eq!(result.snapshot.service, ServiceId::Learn);
        assert_eq!(
            result.snapshot.state,
            crate::protocol::ServiceSessionState::Authenticated
        );
        assert_eq!(
            orchestrator
                .coordinator()
                .bound_csrf(ServiceId::Learn)
                .expect("csrf binding")
                .as_csrf_token()
                .as_str(),
            "cookie-csrf"
        );
        server.join().expect("server");
    }

    #[tokio::test]
    async fn stopped_identity_redirect_is_login_expiry_and_never_promotes_learn() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("roaming request");
            let request = read_request(&mut stream);
            assert!(request.starts_with(
                "GET /b/j_spring_security_thauth_roaming_entry?ticket=identity-ticket HTTP/1.1\r\n"
            ));
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: https://id.example.test/do/off/ui/auth/login?service=learn\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("login redirect");
        });

        let base_url = format!("http://{address}/");
        let learn = LearnClient::new(
            LearnClientConfig::new(&base_url, CourseRole::Student).expect("learn config"),
        );
        let transport =
            CampusHttpTransport::with_timeout("THYou/test", std::time::Duration::from_secs(5))
                .expect("transport");
        let mut orchestrator = LearnSessionOrchestrator::new(learn, transport);
        let ticket = bind_identity_ticket(
            orchestrator.coordinator(),
            ServiceTicket::new("identity-ticket").expect("ticket"),
        );

        let error = orchestrator
            .establish(
                &ticket,
                UserIdentity {
                    username: "student".to_owned(),
                    display_name: None,
                },
            )
            .await
            .expect_err("identity login redirect must expire Learn");
        assert!(matches!(error, LearnSessionError::LoginExpired));
        assert_eq!(
            orchestrator
                .coordinator()
                .registry()
                .snapshot_for(ServiceId::Learn)
                .state,
            crate::protocol::ServiceSessionState::Anonymous
        );
        server.join().expect("server");
    }
}
