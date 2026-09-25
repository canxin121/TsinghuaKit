use std::{
    fmt,
    io::{BufReader, Cursor},
    path::Path,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::{
    Client, Method, RequestBuilder, StatusCode, Url,
    cookie::CookieStore as ReqwestCookieStore,
    header::{CONTENT_TYPE, HeaderValue},
    redirect::Policy,
};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECT_HOPS: usize = 10;
const ALLOWED_HTTPS_TO_HTTP_REDIRECT_HOSTS: &[&str] = &["zhjw.cic.tsinghua.edu.cn"];

pub struct CampusCookieStore {
    inner: RwLock<cookie_store::CookieStore>,
}

/// The transport-level persistence boundary is enabled only after the Rust
/// runtime has completed a current, account-bound Identity proof.  Keeping
/// this state inside the shared transport means clones held by service
/// adapters all observe the same enable/disable decision and all responses
/// use the same Cookie snapshot writer.
#[derive(Clone)]
struct CookieSnapshotWriter {
    lease: crate::session_persistence::SessionLease,
    root: std::path::PathBuf,
    username: String,
    device_fingerprint: String,
    /// This is a navigation checkpoint only; it is never used as current
    /// authentication proof.  Keep it alongside response-level Cookie
    /// refreshes so a new Cookie snapshot does not accidentally erase the
    /// last known non-sensitive checkpoint.
    portal_bootstrap_completed: bool,
}

impl Default for CampusCookieStore {
    fn default() -> Self {
        Self {
            inner: RwLock::new(cookie_store::CookieStore::default()),
        }
    }
}

impl fmt::Debug for CampusCookieStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self
            .inner
            .read()
            .map(|store| store.iter_unexpired().count())
            .unwrap_or_default();
        formatter
            .debug_struct("CampusCookieStore")
            .field("cookie_count", &count)
            .finish()
    }
}

impl CampusCookieStore {
    pub fn add_cookie_str(&self, value: &str, url: &Url) {
        let Some(cookie) = cookie_store::RawCookie::parse(value)
            .ok()
            .map(|cookie| cookie.into_owned())
        else {
            return;
        };
        if let Ok(mut store) = self.inner.write() {
            store.store_response_cookies(std::iter::once(cookie), url);
        }
    }

    pub(crate) fn snapshot_bytes(&self) -> Result<Vec<u8>, String> {
        let store = self
            .inner
            .read()
            .map_err(|_| String::from("cookie store lock unavailable"))?;
        let mut output = Vec::new();
        cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut output)
            .map_err(|_| String::from("cookie store serialization failed"))?;
        Ok(output)
    }

    pub(crate) fn restore_bytes(&self, bytes: &[u8]) -> Result<usize, String> {
        let restored = cookie_store::serde::json::load(BufReader::new(Cursor::new(bytes)))
            .map_err(|_| String::from("cookie store snapshot is invalid"))?;
        let count = restored.iter_unexpired().count();
        let mut store = self
            .inner
            .write()
            .map_err(|_| String::from("cookie store lock unavailable"))?;
        *store = restored;
        Ok(count)
    }
}

impl ReqwestCookieStore for CampusCookieStore {
    fn set_cookies(&self, values: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let cookies = values.filter_map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| cookie_store::RawCookie::parse(value).ok())
                .map(|cookie| cookie.into_owned())
        });
        if let Ok(mut store) = self.inner.write() {
            store.store_response_cookies(cookies, url);
        }
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let store = self.inner.read().ok()?;
        // Match domain, Secure, path and expiry in CookieStore first. HTTP
        // cookie serialization must then put more-specific paths first
        // (RFC 6265 section 5.4); hash-map iteration is not browser ordering.
        // Do not collapse same-name cookies or move them between origins.
        let mut matching = store.matches(url);
        matching.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        let value = matching
            .into_iter()
            .map(|cookie| {
                let (name, value) = cookie.name_value();
                format!("{name}={value}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            None
        } else {
            HeaderValue::from_str(&value).ok()
        }
    }
}

#[derive(Debug, Error)]
pub enum JsonpError {
    #[error("expected callback {expected:?}, found {actual:?}")]
    CallbackMismatch { expected: String, actual: String },

    #[error("the response callback name is invalid: {actual:?}")]
    InvalidCallback { actual: String },

    #[error("callback {callback:?} has an empty payload")]
    EmptyPayload { callback: String },

    #[error("the expected callback name is invalid: {expected:?}")]
    InvalidExpectedCallback { expected: String },

    #[error("the JSONP envelope is malformed: {message}")]
    MalformedEnvelope { message: &'static str },

    #[error("the JSONP response contains non-whitespace content after its payload")]
    TrailingContent,

    #[error("the JSONP payload could not be decoded: {message}")]
    PayloadDecode { message: String },
}

#[cfg(test)]
mod backend_repair_tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut data = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..n]);
            if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                let len = header
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8(data).unwrap()
    }

    #[tokio::test]
    async fn backend_repair_redirect_keeps_cookie_but_drops_post_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/start", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            assert!(read_request(&mut first).contains("fixture_input=yes"));
            first.write_all(b"HTTP/1.1 302 Found\r\nLocation: /next\r\nSet-Cookie: fixture_session=yes; Path=/; HttpOnly\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(first);
            let (mut second, _) = listener.accept().unwrap();
            let request = read_request(&mut second);
            assert!(request.starts_with("GET /next HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("cookie: fixture_session=yes")
            );
            assert!(!request.contains("fixture_input"));
            second
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let transport =
            CampusHttpTransport::with_timeout("fixture", Duration::from_secs(2)).unwrap();
        assert_eq!(
            transport
                .post_form(&endpoint, &[("fixture_input", "yes")])
                .await
                .unwrap(),
            "ok"
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn backend_repair_redirect_does_not_forward_credentials_cross_origin() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let location = format!("http://{}/not-authorized", target.local_addr().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/start", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            write!(stream, "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let transport =
            CampusHttpTransport::with_timeout("fixture", Duration::from_secs(2)).unwrap();
        let response = transport
            .send(
                transport
                    .client()
                    .post(endpoint)
                    .header(reqwest::header::AUTHORIZATION, "Bearer fixture-only")
                    .body("fixture_input=yes"),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert!(matches!(target.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn backend_repair_failed_post_is_not_replayed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/one-time", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(read_request(&mut stream).contains("fixture_input=yes"));
            drop(stream); // Outcome is ambiguous: server consumed the request.
            listener.set_nonblocking(true).unwrap();
            let until = std::time::Instant::now() + Duration::from_millis(150);
            while std::time::Instant::now() < until {
                assert!(
                    matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
                );
                thread::sleep(Duration::from_millis(5));
            }
        });
        let transport =
            CampusHttpTransport::with_timeout("fixture", Duration::from_secs(1)).unwrap();
        assert!(
            transport
                .post_form(&endpoint, &[("fixture_input", "yes")])
                .await
                .is_err()
        );
        server.join().unwrap();
    }

    #[test]
    fn backend_repair_cookie_store_round_trip_keeps_domain_path_and_session_cookie() {
        let store = CampusCookieStore::default();
        let root = Url::parse("https://example.test/").unwrap();
        let nested = Url::parse("https://example.test/private/page").unwrap();
        store.add_cookie_str("root_cookie=one; Path=/; Secure; HttpOnly", &root);
        store.add_cookie_str("private_cookie=two; Path=/private; Secure", &nested);

        let snapshot = store.snapshot_bytes().expect("cookie snapshot");
        assert!(!snapshot.is_empty());

        let restored = CampusCookieStore::default();
        assert_eq!(restored.restore_bytes(&snapshot).unwrap(), 2);
        let root_header = restored
            .cookies(&root)
            .and_then(|value| value.to_str().ok().map(str::to_owned))
            .unwrap();
        assert!(root_header.contains("root_cookie=one"));
        assert!(!root_header.contains("private_cookie=two"));
        let nested_header = restored
            .cookies(&nested)
            .and_then(|value| value.to_str().ok().map(str::to_owned))
            .unwrap();
        assert!(nested_header.contains("root_cookie=one"));
        assert!(nested_header.contains("private_cookie=two"));
    }

    #[test]
    fn backend_repair_cookie_store_restore_reports_zero_for_all_expired_cookies() {
        let store = CampusCookieStore::default();
        let url = Url::parse("https://example.test/").unwrap();
        store.add_cookie_str(
            "expired=one; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/",
            &url,
        );
        let snapshot = store.snapshot_bytes().expect("expired cookie snapshot");

        let restored = CampusCookieStore::default();
        assert_eq!(restored.restore_bytes(&snapshot).unwrap(), 0);
        assert!(restored.cookies(&url).is_none());
    }

    #[test]
    fn backend_repair_cookie_snapshot_refresh_preserves_portal_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "thyou-transport-checkpoint-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        let transport =
            CampusHttpTransport::with_timeout("fixture", Duration::from_secs(2)).unwrap();
        let url = Url::parse("https://example.test/").unwrap();
        transport
            .cookie_jar()
            .add_cookie_str("session=present; Path=/", &url);
        transport
            .enable_cookie_snapshot_persistence(
                &root,
                "fixture-user",
                "0123456789abcdef0123456789abcdef",
                true,
            )
            .unwrap();

        // This is the same hook used after a response has updated the shared
        // Cookie jar. It must retain the checkpoint while replacing the
        // encrypted Cookie payload.
        transport.persist_cookie_snapshot_after_response();

        let snapshot = crate::session_persistence::load_at_root(&root)
            .unwrap()
            .expect("response refresh should save a session snapshot");
        assert!(snapshot.portal_bootstrap_completed);
        let _ = fs::remove_dir_all(root);
    }
}

#[derive(Error)]
pub enum TransportError {
    #[error("invalid request URL: {0}")]
    InvalidUrl(String),

    #[error("HTTP request failed: {0}")]
    Request(#[source] reqwest::Error),

    #[error("HTTP response returned {status}")]
    HttpStatus { status: StatusCode, body: String },

    #[error("response body could not be decoded: {0}")]
    Decode(#[source] reqwest::Error),

    #[error("response body could not be decoded: {message}")]
    DecodeBody { message: String },

    #[error("invalid JSONP response: {0}")]
    Jsonp(#[from] JsonpError),
}

/// A text response with the metadata needed to classify legacy campus pages.
/// The body is kept private to this module's callers and is redacted from
/// debug output because it may contain login HTML or one-time tokens.
pub struct CampusTextResponse {
    pub status: StatusCode,
    pub final_url: Url,
    pub content_type: Option<String>,
    /// A redirect which the shared policy deliberately stopped before
    /// following. This is retained for login-expiry classification and is
    /// intentionally omitted from Debug output because it may carry a ticket.
    pub(crate) redirect_location: Option<String>,
    pub body: String,
}

impl fmt::Debug for CampusTextResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusTextResponse")
            .field("status", &self.status)
            // Auth material can appear in URL userinfo, path, query or
            // fragment. Retain only the origin in diagnostics, not a URL
            // whose query merely happened to be stripped.
            .field("origin", &self.final_url.origin().ascii_serialization())
            .field("content_type", &self.content_type)
            .field(
                "redirect_location_present",
                &self.redirect_location.is_some(),
            )
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[cfg(test)]
mod reference_diagnostic_tests {
    use super::*;

    #[test]
    fn backend_repair_reference_text_response_debug_redacts_navigation_secrets() {
        let response = CampusTextResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://fixture-user:fixture-pass@example.invalid/private/fixture-path-ticket?ticket=fixture-ticket&_csrf=fixture-csrf#fixture-fragment").unwrap(),
            content_type: None, redirect_location: None, body: "fixture-private-body".into(),
        };
        let debug = format!("{response:?}");
        for secret in [
            "fixture-pass",
            "fixture-ticket",
            "fixture-csrf",
            "fixture-fragment",
            "fixture-path-ticket",
            "fixture-private-body",
        ] {
            assert!(
                !debug.contains(secret),
                "diagnostics must not expose navigation or body credentials"
            );
        }
    }
}

impl fmt::Debug for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(_) => formatter.write_str("TransportError::InvalidUrl(..)"),
            Self::Request(_) => formatter.write_str("TransportError::Request(..)"),
            Self::HttpStatus { status, body } => formatter
                .debug_struct("TransportError::HttpStatus")
                .field("status", status)
                .field("body_len", &body.len())
                .finish(),
            Self::Decode(_) => formatter.write_str("TransportError::Decode(..)"),
            Self::DecodeBody { message } => formatter
                .debug_struct("TransportError::DecodeBody")
                .field("message", message)
                .finish(),
            Self::Jsonp(error) => formatter
                .debug_tuple("TransportError::Jsonp")
                .field(error)
                .finish(),
        }
    }
}

/// A witness for one recovery attempt. A changed transport (primary login
/// reset) or a potentially one-shot dispatch prevents automatic replay.
pub(crate) struct ReplayFence {
    counter: Arc<AtomicU64>,
    value: u64,
}
impl ReplayFence {
    pub(crate) fn permits_probe_retry(&self, transport: &CampusHttpTransport) -> bool {
        Arc::ptr_eq(&self.counter, &transport.replay_counter)
            && self.counter.load(Ordering::Relaxed) == self.value
    }
}

pub struct CampusHttpTransport {
    client: Client,
    cookie_jar: Arc<CampusCookieStore>,
    cookie_snapshot_writer: Arc<RwLock<Option<CookieSnapshotWriter>>>,
    replay_counter: Arc<AtomicU64>,
    timeout: Duration,
}

impl Clone for CampusHttpTransport {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            cookie_jar: Arc::clone(&self.cookie_jar),
            cookie_snapshot_writer: Arc::clone(&self.cookie_snapshot_writer),
            replay_counter: Arc::clone(&self.replay_counter),
            timeout: self.timeout,
        }
    }
}

impl CampusHttpTransport {
    pub fn new(user_agent: &str) -> Result<Self, TransportError> {
        Self::with_timeout(user_agent, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(user_agent: &str, timeout: Duration) -> Result<Self, TransportError> {
        let cookie_jar = Arc::new(CampusCookieStore::default());
        let client = Client::builder()
            .cookie_provider(Arc::clone(&cookie_jar))
            // Redirect hops are dispatched explicitly below so they obey the
            // same request gate rather than bursting inside reqwest.
            .redirect(Policy::none())
            // A transport failure does not establish whether a one-time
            // ticket or authentication POST was consumed. Never replay it.
            .retry(reqwest::retry::never())
            .timeout(timeout)
            .user_agent(user_agent)
            // Several legacy Tsinghua form endpoints advertise HTTP/2 but
            // intermittently reset AJAX POST streams. The current public
            // reference clients use HTTP/1.1 for these requests, so keep the
            // shared campus transport on the interoperable protocol.
            .http1_only()
            // The identity service sometimes closes an idle HTTP/1.1 keep
            // alive between SEND_CODE and VERITY_CODE. Reusing that stale
            // connection makes reqwest fail before receiving an HTTP response.
            // Cookies remain shared in the jar; only idle socket reuse is
            // disabled for legacy campus endpoints.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(TransportError::Request)?;

        Ok(Self {
            client,
            cookie_jar,
            cookie_snapshot_writer: Arc::new(RwLock::new(None)),
            replay_counter: Arc::new(AtomicU64::new(0)),
            timeout,
        })
    }

    pub(crate) fn replay_fence(&self) -> ReplayFence {
        ReplayFence {
            counter: Arc::clone(&self.replay_counter),
            value: self.replay_counter.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn dispatch_requires_exclusivity(request: &reqwest::Request) -> bool {
        let path = request.url().path();
        !matches!(*request.method(), Method::GET | Method::HEAD)
            || request
                .url()
                .query_pairs()
                .any(|(key, _)| matches!(key.as_ref(), "ticket" | "code" | "SAMLResponse"))
            || [
                "checkSingle",
                "redirect2Jsp",
                "onlineAppRedirect",
                "/thu-oauth/",
                "/lb-auth/",
            ]
            .iter()
            .any(|part| path.contains(part))
            // These GETs mutate authentication state too. Their HTTP verb
            // must not let image refresh or srun login/logout share read slots.
            || ["/site/captcha", "/cgi-bin/get_challenge", "/cgi-bin/srun_portal"]
                .iter()
                .any(|route| path.trim_end_matches('/').ends_with(route))
    }

    fn record_potentially_consuming_dispatch(&self, request: &reqwest::Request) {
        if Self::dispatch_requires_exclusivity(request) {
            self.replay_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn cookie_jar(&self) -> &Arc<CampusCookieStore> {
        &self.cookie_jar
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Enables best-effort Cookie snapshot refreshes for the current,
    /// account-bound live Identity session.  This is deliberately a private
    /// Rust runtime hook: callers cannot enable it from Flutter or from an
    /// unproven restored Cookie snapshot.
    pub(crate) fn enable_cookie_snapshot_persistence(
        &self,
        root: &Path,
        username: &str,
        device_fingerprint: &str,
        portal_bootstrap_completed: bool,
    ) -> Result<(), String> {
        let lease = crate::session_persistence::SessionLease::current(root)?
            .ok_or_else(|| "session authority revoked".to_owned())?;
        self.enable_cookie_snapshot_persistence_with_lease(
            root,
            username,
            device_fingerprint,
            portal_bootstrap_completed,
            lease,
        )
    }

    pub(crate) fn enable_cookie_snapshot_persistence_with_lease(
        &self,
        root: &Path,
        username: &str,
        device_fingerprint: &str,
        portal_bootstrap_completed: bool,
        lease: crate::session_persistence::SessionLease,
    ) -> Result<(), String> {
        if !lease.belongs_to(root) || !lease.is_current() {
            return Err("session authority revoked".to_owned());
        }
        // Validate the same account/device boundary used by the encrypted
        // session file before installing the writer.  The sample payload is
        // never written; it only reuses the persistence type's validation.
        let snapshot = crate::session_persistence::ResumeSnapshot::new(username, b"cookie")?
            .with_device_fingerprint(device_fingerprint)?;
        let _ = snapshot;
        if !root.is_absolute() {
            return Err(String::from("resume session root must be absolute"));
        }
        let writer = CookieSnapshotWriter {
            lease,
            root: root.to_owned(),
            username: username.trim().to_owned(),
            device_fingerprint: device_fingerprint.to_owned(),
            portal_bootstrap_completed,
        };
        let mut current = self
            .cookie_snapshot_writer
            .write()
            .map_err(|_| String::from("cookie snapshot writer lock unavailable"))?;
        *current = Some(writer);
        Ok(())
    }

    /// Stops response-level snapshot writes before an explicit logout, a new
    /// login boundary, or a failed Identity proof.  The caller owns any
    /// separate removal of the old durable snapshot.
    pub(crate) fn disable_cookie_snapshot_persistence(&self) {
        if let Ok(mut current) = self.cookie_snapshot_writer.write() {
            *current = None;
        }
    }

    /// Saves the current Cookie jar after a response has been received.  This
    /// intentionally runs before any caller consumes or parses the body, so a
    /// refreshed Cookie is not lost merely because the business payload is
    /// malformed.  All failures are best effort and use a fixed telemetry
    /// reason; no Cookie, account, URL, or response body crosses diagnostics.
    fn persist_cookie_snapshot_after_response(&self) {
        let writer = match self.cookie_snapshot_writer.read() {
            Ok(current) => current.clone(),
            Err(_) => {
                tracing::warn!(
                    target: "tsinghua_kit::storage",
                    event = "resume_cookie_snapshot_refresh_failed",
                    reason = "storage_io"
                );
                return;
            }
        };
        let Some(writer) = writer else {
            return;
        };
        let result = (|| {
            let cookies = self.cookie_jar.snapshot_bytes()?;
            let snapshot =
                crate::session_persistence::ResumeSnapshot::new(&writer.username, &cookies)?
                    .with_device_fingerprint(&writer.device_fingerprint)?;
            let mut snapshot = snapshot;
            snapshot.portal_bootstrap_completed = writer.portal_bootstrap_completed;
            writer
                .lease
                .with_current(|| crate::session_persistence::save_at_root(&writer.root, &snapshot))
        })();
        if result.is_err() {
            tracing::warn!(
                target: "tsinghua_kit::storage",
                event = "resume_cookie_snapshot_refresh_failed",
                reason = "storage_io"
            );
        }
    }

    /// Executes a builder through the shared pacing and redirect boundary.
    /// `client()` is for building requests, not for bypassing this method.
    pub(crate) async fn send(
        &self,
        builder: RequestBuilder,
    ) -> Result<reqwest::Response, reqwest::Error> {
        self.execute(builder.build()?).await
    }

    pub(crate) async fn execute(
        &self,
        mut request: reqwest::Request,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut previous = Vec::new();
        for hop in 0..=MAX_REDIRECT_HOPS {
            let replay = request.try_clone();
            let response = self.execute_once(&self.client, request).await?;
            let status = response.status();
            if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) || hop == MAX_REDIRECT_HOPS {
                return Ok(response);
            }
            let Some(target) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| response.url().join(value).ok())
            else {
                return Ok(response);
            };
            previous.push(response.url().clone());
            if !redirect_is_safe(&previous, &target) {
                tracing::warn!(target:"tsinghua_kit::security",event="redirect_blocked",endpoint=crate::telemetry::endpoint(response.url()),reason="origin_or_mapping",hop=hop as u64,http_status=status.as_u16());
                return Ok(response);
            }
            tracing::trace!(target:"tsinghua_kit::http",event="redirect_followed",endpoint=crate::telemetry::endpoint(&target),hop=hop as u64,http_status=status.as_u16());
            let Some(mut next) = replay else {
                // Streaming request bodies cannot be replayed safely.
                return Ok(response);
            };
            if matches!(status.as_u16(), 301 | 302 | 303) {
                if next.method() != Method::GET && next.method() != Method::HEAD {
                    *next.method_mut() = Method::GET;
                }
                *next.body_mut() = None;
                for name in [
                    reqwest::header::CONTENT_LENGTH,
                    CONTENT_TYPE,
                    reqwest::header::TRANSFER_ENCODING,
                    reqwest::header::CONTENT_ENCODING,
                ] {
                    next.headers_mut().remove(name);
                }
            }
            // Each hop must consult the Cookie jar, including cookies set by
            // its immediate predecessor. Do not carry an old explicit Cookie.
            next.headers_mut().remove(reqwest::header::COOKIE);
            next.headers_mut().remove(reqwest::header::HOST);
            if next.url().scheme() != target.scheme() {
                next.headers_mut().remove(reqwest::header::AUTHORIZATION);
                next.headers_mut()
                    .remove(reqwest::header::PROXY_AUTHORIZATION);
                next.headers_mut().remove(reqwest::header::REFERER);
            }
            *next.url_mut() = target;
            request = next;
        }
        unreachable!("the bounded redirect loop returns on its last hop")
    }

    /// Identity needs to inspect intermediate Set-Cookie evidence itself.
    /// Its no-redirect client still uses the same process-wide gate.
    pub(crate) async fn execute_once(
        &self,
        client: &Client,
        request: reqwest::Request,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut trace = crate::telemetry::HttpTrace::begin(&request);
        if crate::request_gate::is_loopback(request.url()) {
            trace.dispatched();
            self.record_potentially_consuming_dispatch(&request);
            let mut result = client.execute(request).await;
            trace.finish(&mut result);
            if result.is_ok() {
                self.persist_cookie_snapshot_after_response();
            }
            return result;
        }
        let gate = crate::request_gate::campus_request_gate();
        let (_permit, queue_wait, rate_wait) = gate
            .acquire_for(Self::dispatch_requires_exclusivity(&request))
            .await;
        trace.waited(queue_wait, rate_wait);
        trace.dispatched();
        self.record_potentially_consuming_dispatch(&request);
        let mut result = client.execute(request).await;
        trace.finish(&mut result);
        let response = result?;
        self.persist_cookie_snapshot_after_response();
        gate.observe_response(response.status(), response.headers())
            .await;
        Ok(response)
    }

    pub async fn get_text(&self, endpoint: &str) -> Result<String, TransportError> {
        let request = self.build_request(Method::GET, endpoint)?;
        self.send_text(request).await
    }

    /// Fetches text while retaining the final URL and content type. Legacy
    /// Registrar endpoints return HTTP 200 HTML timeout pages, so callers
    /// need this metadata before attempting JSONP decoding.
    pub async fn get_text_response(
        &self,
        endpoint: &str,
    ) -> Result<CampusTextResponse, TransportError> {
        let request = self.build_request(Method::GET, endpoint)?;
        self.send_text_response(request).await
    }

    pub async fn get_text_with_query<Q>(
        &self,
        endpoint: &str,
        query: &Q,
    ) -> Result<String, TransportError>
    where
        Q: Serialize + ?Sized,
    {
        let request = self.build_get_request(endpoint, query)?;
        self.send_text(request).await
    }

    pub async fn get_json<T>(&self, endpoint: &str) -> Result<T, TransportError>
    where
        T: DeserializeOwned,
    {
        let request = self.build_request(Method::GET, endpoint)?;
        self.send_json(request).await
    }

    pub async fn get_json_with_query<Q, T>(
        &self,
        endpoint: &str,
        query: &Q,
    ) -> Result<T, TransportError>
    where
        Q: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let request = self.build_get_request(endpoint, query)?;
        self.send_json(request).await
    }

    pub async fn post_form<F>(&self, endpoint: &str, form: &F) -> Result<String, TransportError>
    where
        F: Serialize + ?Sized,
    {
        let request = self.build_post_form_request(endpoint, form)?;
        self.send_text(request).await
    }

    /// Sends an already encoded URL-encoded form body through the shared
    /// cookie-aware client. This is useful for protocol profiles whose
    /// parameter ordering and encoding are part of a request plan.
    pub async fn post_form_encoded(
        &self,
        endpoint: &str,
        body: &str,
    ) -> Result<String, TransportError> {
        let request = self
            .build_request(Method::POST, endpoint)?
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body.to_owned());
        self.send_text(request).await
    }

    pub async fn post_json<B>(&self, endpoint: &str, body: &B) -> Result<String, TransportError>
    where
        B: Serialize + ?Sized,
    {
        let request = self.build_post_json_request(endpoint, body)?;
        self.send_text(request).await
    }

    pub async fn get_jsonp<T>(
        &self,
        endpoint: &str,
        expected_callback: &str,
    ) -> Result<T, TransportError>
    where
        T: DeserializeOwned,
    {
        let body = self.get_text(endpoint).await?;
        parse_jsonp(&body, expected_callback)
    }

    pub async fn get_jsonp_with_query<Q, T>(
        &self,
        endpoint: &str,
        query: &Q,
        expected_callback: &str,
    ) -> Result<T, TransportError>
    where
        Q: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let body = self.get_text_with_query(endpoint, query).await?;
        parse_jsonp(&body, expected_callback)
    }

    fn build_request(
        &self,
        method: Method,
        endpoint: &str,
    ) -> Result<RequestBuilder, TransportError> {
        let url = parse_url(endpoint)?;
        Ok(self.client.request(method, url))
    }

    fn build_get_request<Q>(
        &self,
        endpoint: &str,
        query: &Q,
    ) -> Result<RequestBuilder, TransportError>
    where
        Q: Serialize + ?Sized,
    {
        Ok(self.build_request(Method::GET, endpoint)?.query(query))
    }

    fn build_post_form_request<F>(
        &self,
        endpoint: &str,
        form: &F,
    ) -> Result<RequestBuilder, TransportError>
    where
        F: Serialize + ?Sized,
    {
        Ok(self.build_request(Method::POST, endpoint)?.form(form))
    }

    fn build_post_json_request<B>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<RequestBuilder, TransportError>
    where
        B: Serialize + ?Sized,
    {
        Ok(self.build_request(Method::POST, endpoint)?.json(body))
    }

    async fn send_text(&self, request: RequestBuilder) -> Result<String, TransportError> {
        let response = self.send_text_response(request).await?;
        if response.status.is_success() {
            Ok(response.body)
        } else {
            Err(TransportError::HttpStatus {
                status: response.status,
                body: response.body,
            })
        }
    }

    async fn send_text_response(
        &self,
        request: RequestBuilder,
    ) -> Result<CampusTextResponse, TransportError> {
        let response = self.send(request).await.map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let redirect_location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        Ok(CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location,
            body,
        })
    }

    async fn send_json<T>(&self, request: RequestBuilder) -> Result<T, TransportError>
    where
        T: DeserializeOwned,
    {
        let body = self.send_text(request).await?;
        crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
            serde_json::from_str(&body).map_err(|error| TransportError::DecodeBody {
                message: error.to_string(),
            })
        })
    }
}

fn parse_url(endpoint: &str) -> Result<Url, TransportError> {
    Url::parse(endpoint).map_err(|error| TransportError::InvalidUrl(error.to_string()))
}

fn redirect_is_safe(previous: &[Url], next: &Url) -> bool {
    previous.len() <= MAX_REDIRECT_HOPS
        && previous.last().is_some_and(|previous| {
            redirect_is_same_origin(previous, next)
                && match webvpn_mapping_scope(previous) {
                    Some(scope) => {
                        webvpn_mapping_scope(next) == Some(scope)
                            && crate::webvpn_url::redirect_path_stays_in_mapping(
                                next, scope.0, scope.1,
                            )
                    }
                    None => true,
                }
        })
}

/// WebVPN maps different origin services onto one HTTPS host. Host equality
/// is not authority to forward a business POST or its credentials to another
/// mapped service. Identity's explicitly validated SSO navigation uses its
/// own execute_once boundary and is not granted by this automatic redirect.
fn webvpn_mapping_scope(url: &Url) -> Option<(&str, &str)> {
    let (protocol, tail) = url.path().strip_prefix('/')?.split_once('/')?;
    let (scheme, port) = protocol.split_once('-').unwrap_or((protocol, ""));
    if !matches!(scheme, "http" | "https")
        || (!port.is_empty() && !port.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    const PREFIX: &str = "77726476706e69737468656265737421";
    if !tail.starts_with(PREFIX) {
        return None;
    }
    let end = tail
        .find(|ch: char| !ch.is_ascii_hexdigit())
        .unwrap_or(tail.len());
    // Also identify the mapping before an encoded slash, which the WebVPN
    // broker sometimes uses between the cipher host and service path.
    Some((protocol, &tail[..end]))
}

fn redirect_is_same_origin(previous: &Url, next: &Url) -> bool {
    let is_registrar_downgrade = previous.scheme() == "https"
        && next.scheme() == "http"
        && next
            .host_str()
            .is_some_and(|host| ALLOWED_HTTPS_TO_HTTP_REDIRECT_HOSTS.contains(&host));
    let same_host = previous.host_str() == next.host_str()
        && next.username().is_empty()
        && next.password().is_none()
        && next.fragment().is_none();
    let same_port = previous.port_or_known_default() == next.port_or_known_default()
        || (is_registrar_downgrade
            && previous.port_or_known_default() == Some(443)
            && next.port_or_known_default() == Some(80));
    if !same_host || !same_port {
        return false;
    }

    previous.scheme() == next.scheme() || is_registrar_downgrade
}

pub fn parse_jsonp<T>(body: &str, expected_callback: &str) -> Result<T, TransportError>
where
    T: DeserializeOwned,
{
    if !valid_callback_name(expected_callback) {
        return Err(JsonpError::InvalidExpectedCallback {
            expected: expected_callback.to_owned(),
        }
        .into());
    }

    let response = body.trim();
    let response = response
        .strip_prefix('\u{feff}')
        .unwrap_or(response)
        .trim_start();
    let Some(opening_parenthesis) = response.find('(') else {
        return Err(JsonpError::MalformedEnvelope {
            message: "missing opening parenthesis",
        }
        .into());
    };

    let actual_callback = response[..opening_parenthesis].trim();
    if !valid_callback_name(actual_callback) {
        return Err(JsonpError::InvalidCallback {
            actual: actual_callback.to_owned(),
        }
        .into());
    }
    if actual_callback != expected_callback {
        return Err(JsonpError::CallbackMismatch {
            expected: expected_callback.to_owned(),
            actual: actual_callback.to_owned(),
        }
        .into());
    }

    let Some(closing_parenthesis) = response.rfind(')') else {
        return Err(JsonpError::MalformedEnvelope {
            message: "missing closing parenthesis",
        }
        .into());
    };

    // One statement terminator is valid JSONP. No comments, additional
    // statements, or executable JavaScript are accepted or evaluated.
    if !matches!(response[closing_parenthesis + 1..].trim(), "" | ";") {
        return Err(JsonpError::TrailingContent.into());
    }

    if closing_parenthesis <= opening_parenthesis {
        return Err(JsonpError::MalformedEnvelope {
            message: "closing parenthesis precedes the payload",
        }
        .into());
    }

    let payload = response[opening_parenthesis + 1..closing_parenthesis].trim();
    if payload.is_empty() {
        return Err(JsonpError::EmptyPayload {
            callback: expected_callback.to_owned(),
        }
        .into());
    }

    serde_json::from_str(payload).map_err(|error| {
        JsonpError::PayloadDecode {
            message: error.to_string(),
        }
        .into()
    })
}

fn valid_callback_name(callback: &str) -> bool {
    if callback.is_empty() {
        return false;
    }

    callback.split('.').all(|segment| {
        let mut characters = segment.chars();
        let Some(first) = characters.next() else {
            return false;
        };
        if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
            return false;
        }
        characters.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn rejects_invalid_urls_before_network_access() {
        let error = parse_url("not a URL").expect_err("invalid URL should fail");
        assert!(matches!(error, TransportError::InvalidUrl(_)));
    }

    #[test]
    fn transport_status_diagnostics_do_not_print_response_bodies() {
        let error = TransportError::HttpStatus {
            status: StatusCode::UNAUTHORIZED,
            body: "password=secret&ticket=opaque".to_owned(),
        };
        assert!(!format!("{error:?}").contains("secret"));
        assert!(!error.to_string().contains("opaque"));
        assert!(format!("{error:?}").contains("body_len"));
    }

    #[test]
    fn builds_a_cookie_aware_client_with_a_custom_timeout() {
        let transport = CampusHttpTransport::with_timeout("THYou/0.1", Duration::from_secs(3))
            .expect("client builds");
        assert_eq!(transport.timeout(), Duration::from_secs(3));
    }

    #[test]
    fn follows_same_origin_redirects_but_stops_at_another_origin() {
        let previous = Url::parse("https://id.example.test/login").expect("previous URL");
        let same_origin = Url::parse("https://id.example.test/next").expect("same-origin URL");
        let different_host = Url::parse("https://evil.example.test/next").expect("host URL");
        let different_scheme = Url::parse("http://id.example.test/next").expect("scheme URL");
        let different_port = Url::parse("https://id.example.test:8443/next").expect("port URL");

        assert!(redirect_is_safe(
            std::slice::from_ref(&previous),
            &same_origin
        ));
        assert!(!redirect_is_safe(
            std::slice::from_ref(&previous),
            &different_host
        ));
        assert!(!redirect_is_safe(
            std::slice::from_ref(&previous),
            &different_scheme
        ));
        assert!(!redirect_is_safe(
            std::slice::from_ref(&previous),
            &different_port
        ));
    }

    #[test]
    fn limits_redirect_chains_and_rejects_redirect_userinfo() {
        let previous = Url::parse("https://id.example.test/login").expect("previous URL");
        let next = Url::parse("https://id.example.test/next").expect("next URL");
        let userinfo =
            Url::parse("https://user:id-secret@id.example.test/next").expect("userinfo URL");

        assert!(!redirect_is_safe(
            &vec![previous.clone(); MAX_REDIRECT_HOPS + 1],
            &next
        ));
        assert!(!redirect_is_safe(
            std::slice::from_ref(&previous),
            &userinfo
        ));
    }

    #[test]
    fn allows_only_the_explicit_registrar_https_to_http_downgrade() {
        let registrar =
            Url::parse("https://zhjw.cic.tsinghua.edu.cn/login").expect("registrar URL");
        let registrar_http =
            Url::parse("http://zhjw.cic.tsinghua.edu.cn/sso_fail.jsp").expect("http URL");
        let other = Url::parse("https://id.example.test/login").expect("identity URL");
        let other_http = Url::parse("http://id.example.test/login").expect("http URL");

        assert!(redirect_is_safe(
            std::slice::from_ref(&registrar),
            &registrar_http
        ));
        assert!(!redirect_is_safe(std::slice::from_ref(&other), &other_http));
    }

    #[derive(Serialize)]
    struct TestQuery<'a> {
        keyword: &'a str,
        page: u8,
    }

    #[derive(Serialize)]
    struct TestForm<'a> {
        username: &'a str,
        remember: bool,
    }

    #[derive(Serialize)]
    struct TestJson {
        course_id: u32,
        archived: bool,
    }

    fn request_body(request: &reqwest::Request) -> &[u8] {
        request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("test request should have an in-memory body")
    }

    #[test]
    fn builds_get_requests_with_query_parameters() {
        let transport = CampusHttpTransport::new("THYou/0.1").expect("client builds");
        let query = TestQuery {
            keyword: "linear algebra",
            page: 2,
        };
        let request = transport
            .build_get_request("https://example.test/courses", &query)
            .expect("request builder should build")
            .build()
            .expect("request should build");

        assert_eq!(request.method(), Method::GET);
        assert_eq!(request.url().path(), "/courses");
        let query_pairs: Vec<_> = request
            .url()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert!(query_pairs.contains(&("keyword".to_owned(), "linear algebra".to_owned())));
        assert!(query_pairs.contains(&("page".to_owned(), "2".to_owned())));
    }

    #[test]
    fn builds_form_and_json_post_requests() {
        let transport = CampusHttpTransport::new("THYou/0.1").expect("client builds");

        let form_request = transport
            .build_post_form_request(
                "https://example.test/login",
                &TestForm {
                    username: "student",
                    remember: true,
                },
            )
            .expect("form request builder should build")
            .build()
            .expect("form request should build");
        assert_eq!(form_request.method(), Method::POST);
        assert_eq!(
            form_request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/x-www-form-urlencoded")
        );
        let form_body = String::from_utf8(request_body(&form_request).to_vec())
            .expect("form body should be UTF-8");
        assert!(form_body.contains("username=student"));
        assert!(form_body.contains("remember=true"));

        let json_request = transport
            .build_post_json_request(
                "https://example.test/courses",
                &TestJson {
                    course_id: 42,
                    archived: false,
                },
            )
            .expect("JSON request builder should build")
            .build()
            .expect("JSON request should build");
        assert_eq!(json_request.method(), Method::POST);
        assert_eq!(
            json_request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let json_body: serde_json::Value =
            serde_json::from_slice(request_body(&json_request)).expect("JSON body should decode");
        assert_eq!(json_body["course_id"], 42);
        assert_eq!(json_body["archived"], false);
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct TestJsonpPayload {
        title: String,
        count: u32,
    }

    #[test]
    fn parses_jsonp_with_whitespace_and_parentheses_in_json_strings() {
        let payload: TestJsonpPayload = parse_jsonp(
            "\n  campusCallback({\"title\":\"Week (1)\",\"count\":3})  \n",
            "campusCallback",
        )
        .expect("valid JSONP should parse");

        assert_eq!(
            payload,
            TestJsonpPayload {
                title: "Week (1)".to_owned(),
                count: 3,
            }
        );
    }

    #[test]
    fn rejects_jsonp_with_wrong_callback() {
        let error =
            parse_jsonp::<serde_json::Value>("otherCallback({\"ok\":true})", "expectedCallback")
                .expect_err("wrong callback should fail");

        assert!(matches!(
            error,
            TransportError::Jsonp(JsonpError::CallbackMismatch { expected, actual })
                if expected == "expectedCallback" && actual == "otherCallback"
        ));
    }

    #[test]
    fn rejects_jsonp_with_an_invalid_response_callback_name() {
        let error =
            parse_jsonp::<serde_json::Value>("callback-name({\"ok\":true})", "safeCallback")
                .expect_err("invalid callback syntax should fail");

        assert!(matches!(
            error,
            TransportError::Jsonp(JsonpError::InvalidCallback { actual })
                if actual == "callback-name"
        ));
    }

    #[test]
    fn rejects_jsonp_with_non_whitespace_leading_content() {
        let error = parse_jsonp::<serde_json::Value>(
            "prefix campusCallback({\"ok\":true})",
            "campusCallback",
        )
        .expect_err("leading content should fail");

        assert!(matches!(
            error,
            TransportError::Jsonp(JsonpError::InvalidCallback { actual })
                if actual == "prefix campusCallback"
        ));
    }

    #[test]
    fn rejects_empty_jsonp_payload() {
        let error = parse_jsonp::<serde_json::Value>("campusCallback(  )", "campusCallback")
            .expect_err("empty payload should fail");

        assert!(matches!(
            error,
            TransportError::Jsonp(JsonpError::EmptyPayload { callback })
                if callback == "campusCallback"
        ));
    }

    #[test]
    fn rejects_jsonp_with_non_whitespace_trailing_content() {
        let error = parse_jsonp::<serde_json::Value>(
            "campusCallback({\"ok\":true}) unexpected",
            "campusCallback",
        )
        .expect_err("trailing content should fail");

        assert!(matches!(
            error,
            TransportError::Jsonp(JsonpError::TrailingContent)
        ));
    }

    #[test]
    fn rejects_invalid_jsonp_payload() {
        let error =
            parse_jsonp::<serde_json::Value>("campusCallback({not-json})", "campusCallback")
                .expect_err("invalid JSON payload should fail");

        assert!(matches!(
            error,
            TransportError::Jsonp(JsonpError::PayloadDecode { .. })
        ));
    }
}

#[cfg(test)]
mod cookie_wire_repairs {
    use super::*;

    #[test]
    fn backend_repair_cookie_header_uses_longest_path_before_wrapper_cookie() {
        let jar = CampusCookieStore::default();
        let mut path = String::from("/");
        let mut expected = Vec::new();
        for index in 0..12 {
            path.push_str("nested/");
            let origin = Url::parse(&format!("https://fixture.example{path}page")).unwrap();
            jar.add_cookie_str(
                &format!("wire_id=fixture-{index}; Path={path}; Secure"),
                &origin,
            );
            expected.push(format!("wire_id=fixture-{index}"));
        }
        let url = Url::parse(&format!("https://fixture.example{path}page")).unwrap();
        jar.add_cookie_str("wire_id=fixture-root; Path=/; Secure", &url);
        expected.reverse();
        expected.push("wire_id=fixture-root".to_owned());
        assert_eq!(
            jar.cookies(&url).unwrap().to_str().unwrap(),
            expected.join("; ")
        );
        let restored = CampusCookieStore::default();
        restored
            .restore_bytes(&jar.snapshot_bytes().unwrap())
            .unwrap();
        assert_eq!(
            restored.cookies(&url).unwrap().to_str().unwrap(),
            expected.join("; ")
        );
        assert!(
            jar.cookies(&Url::parse("https://foreign.example/").unwrap())
                .is_none()
        );
        assert!(
            jar.cookies(&Url::parse(&format!("http://fixture.example{path}page")).unwrap())
                .is_none()
        );
    }
}
