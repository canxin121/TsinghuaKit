//! Read-only 清紫源泉 bottled-water account lookup (`订水查询`).
//!
//! The vendor that serves the campus water delivery runs its own plain-HTTP
//! service at `dingshui.bjqzhd.com`.  It is neither a campus host nor a WebVPN
//! target, and it has no campus account binding: the caller types the room's
//! own delivery number into it.  This module therefore sits in the same
//! deliberately-separate read boundary as the laundry vendors rather than in
//! the INFO roaming allowlist:
//!
//! * It builds its own Cookie-free [`CampusHttpTransport`], so no campus
//!   Cookie can reach the vendor even though the shared transport type is
//!   reused.
//! * Every request still travels through that transport, so it shares the
//!   process-wide request gate and the exclusive-dispatch rule for POSTs.  No
//!   request is issued through `reqwest` directly.
//! * The delivery number is the caller's own input.  It is bounded to a short
//!   alphanumeric token before it can enter a body, it never appears in a
//!   `Debug` rendering, and it never enters a log or a diagnostic.
//! * The profile has **no** submission representation.  The vendor's ordering
//!   endpoint (`/buy/subs.html`) places a real order, so it is not modelled
//!   here at all: an unreachable operation cannot be issued by accident.
//!
//! The vendor's plain HTTP is retained rather than upgraded: the deployment
//! has no TLS listener, and the observed reading reaches it the same way.  The
//! module narrows what that costs by sending only the delivery number, by
//! refusing a response that is not the vendor's own JSON envelope, and by
//! keeping the whole exchange out of every persisted record.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{fmt, time::Duration};

use reqwest::{StatusCode, Url, header::LOCATION};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::transport::{CampusHttpTransport, TransportError};

/// The vendor's service origin.  It is a third-party host with no TLS
/// listener and no WebVPN mapping.
pub const WATER_ORIGIN: &str = "http://dingshui.bjqzhd.com/";
/// The vendor's account lookup endpoint.
pub const WATER_USER_PATH: &str = "/auser/getuser.html";

/// The longest JSON body this module will parse.
const MAX_BODY_BYTES: usize = 64 * 1024;
/// The longest text field kept from the vendor's response.
const MAX_TEXT_CHARS: usize = 128;
/// The longest delivery number this module will put into a request body.
const MAX_DELIVERY_ID: usize = 32;

/// The vendor's water brands, keyed by the identifier its own ordering form
/// uses.  A brand outside this table is reported by its own identifier rather
/// than silently relabelled.
pub const WATER_BRANDS: [(&str, &str); 9] = [
    ("1", "娃哈哈矿泉水"),
    ("5", "清紫源泉纯净水"),
    ("6", "清紫源泉矿泉水（高端）"),
    ("7", "娃哈哈纯净水"),
    ("8", "喜士天然矿泉水（大）"),
    ("9", "喜士天然矿泉水（小）"),
    ("10", "燕园泉矿泉水（高端）"),
    ("11", "清紫源泉矿泉水"),
    ("12", "农夫山泉桶装水（19L）"),
];

/// Returns the vendor's brand label for one brand identifier.
///
/// An identifier the table does not carry is returned as-is, so a brand added
/// by the vendor stays visible instead of appearing as an unknown blank.
pub fn water_brand_name(brand_id: &str) -> String {
    WATER_BRANDS
        .iter()
        .find(|(id, _)| *id == brand_id)
        .map_or_else(|| brand_id.to_owned(), |(_, name)| (*name).to_owned())
}

/// One delivery account's own record.
#[derive(Clone, PartialEq, Eq)]
pub struct WaterUser {
    /// The name the vendor holds for this delivery number.
    pub name: String,
    /// The delivery address the vendor holds for this delivery number.
    pub address: String,
}

/// The record is personal data, so its `Debug` prints only presence and
/// length, never the values themselves.
impl fmt::Debug for WaterUser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WaterUser")
            .field("name_present", &!self.name.is_empty())
            .field("address_len", &self.address.chars().count())
            .finish()
    }
}

/// Everything that can stop a water account lookup.
#[derive(Debug, Error)]
pub enum WaterError {
    #[error("water service origin is invalid")]
    InvalidBaseUrl,

    #[error("water request failed")]
    Transport(#[source] TransportError),

    #[error("water request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("water response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("water response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("water response is not JSON")]
    NotJson,

    #[error("the delivery number is not one this client will send")]
    InvalidDeliveryId,
}

impl WaterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "water_config",
            Self::Transport(_) => "water_network",
            Self::HttpStatus { .. } => "water_http",
            Self::UnexpectedOrigin => "water_origin",
            Self::UnexpectedDeployment => "water_template",
            Self::NotJson => "water_not_json",
            Self::InvalidDeliveryId => "water_delivery_id",
        }
    }
}

/// Configuration for the read-only water adapter.
#[derive(Clone)]
pub struct WaterAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl WaterAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, WaterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/water", Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, WaterError> {
        let base_url = Url::parse(base_url).map_err(|_| WaterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(WaterError::InvalidBaseUrl);
        }
        Ok(Self {
            base_url,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, WaterError> {
        // A dedicated, Cookie-free transport: this boundary must never carry a
        // campus session to the vendor.  Requests still pass through the
        // shared process-wide request gate.
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(WaterError::Transport)
    }
}

impl fmt::Debug for WaterAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WaterAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Read-only water account client.
pub struct WaterAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
}

impl WaterAdapter {
    /// Builds the adapter at the vendor's own origin.
    ///
    /// This is the only production entry point, so a caller cannot aim the
    /// adapter at a host of its own choosing.
    pub fn new() -> Result<Self, WaterError> {
        Self::try_with_transport(Url::parse(WATER_ORIGIN).map_err(|_| WaterError::InvalidBaseUrl)?)
    }

    /// Builds the adapter against a caller-supplied origin, creating its own
    /// Cookie-free transport.  Loopback fixtures use this.
    pub fn try_with_transport(base_url: Url) -> Result<Self, WaterError> {
        let config = WaterAdapterConfig::new(base_url.as_str())?;
        let transport = config.transport()?;
        Ok(Self {
            base_url: config.base_url,
            transport,
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Looks up the vendor's record for one delivery number.
    ///
    /// The number is the caller's own input, so it is bounded before any
    /// request is built: a value this client will not send costs no network
    /// round trip and never reaches the vendor.
    pub async fn read_user(&self, delivery_id: &str) -> Result<WaterUser, WaterError> {
        let delivery_id = delivery_id_for_request(delivery_id)?;
        let body = self
            .post_form(WATER_USER_PATH, &[("name", "pw"), ("param", delivery_id)])
            .await?;
        let root = parse_object(&body)?;
        Ok(WaterUser {
            name: text_field(&root, "name").unwrap_or_default(),
            address: text_field(&root, "address").unwrap_or_default(),
        })
    }

    async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<String, WaterError> {
        let endpoint = self.endpoint(path)?;
        let request = self
            .transport
            .client()
            .post(endpoint)
            .form(form)
            .build()
            .map_err(|error| WaterError::Transport(TransportError::Request(error)))?;
        self.execute(request).await
    }

    async fn execute(&self, request: reqwest::Request) -> Result<String, WaterError> {
        let expected_path = request.url().path().to_owned();
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(|error| WaterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| WaterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(WaterError::UnexpectedDeployment);
        }
        if !same_origin(&self.base_url, &final_url)
            || location
                .as_deref()
                .and_then(|value| final_url.join(value).ok())
                .is_some_and(|target| !same_origin(&self.base_url, &target))
        {
            return Err(WaterError::UnexpectedOrigin);
        }
        if status != StatusCode::OK {
            return Err(WaterError::HttpStatus { status });
        }
        if final_url.path() != expected_path {
            return Err(WaterError::UnexpectedDeployment);
        }
        Ok(body)
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, WaterError> {
        if !relative_path.starts_with('/')
            || relative_path.contains("://")
            || relative_path.contains(['?', '#'])
            || relative_path.contains("..")
            || relative_path.chars().any(char::is_control)
        {
            return Err(WaterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{relative_path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }
}

impl fmt::Debug for WaterAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WaterAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .finish()
    }
}

/// Bounds one caller-supplied delivery number before it can enter a body.
fn delivery_id_for_request(delivery_id: &str) -> Result<&str, WaterError> {
    if delivery_id.is_empty()
        || delivery_id.len() > MAX_DELIVERY_ID
        || !delivery_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(WaterError::InvalidDeliveryId);
    }
    Ok(delivery_id)
}

/// Reads one JSON string field into a bounded string.
fn text_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    let value = match object.get(key)? {
        Value::String(value) => value.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.chars().take(MAX_TEXT_CHARS).collect())
}

/// Parses the vendor's own JSON envelope.
///
/// The vendor is a legacy PHP deployment with no TLS listener and an
/// inconsistent `Content-Type`, so the body's shape is what settles whether
/// this is its answer: a JSON object is accepted, an HTML page is refused as
/// [`WaterError::NotJson`] rather than being read as an empty record.
fn parse_object(body: &str) -> Result<Map<String, Value>, WaterError> {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    if trimmed.is_empty() || looks_like_html(trimmed) {
        return Err(WaterError::NotJson);
    }
    match serde_json::from_str(trimmed).map_err(|_| WaterError::NotJson)? {
        Value::Object(object) => Ok(object),
        _ => Err(WaterError::NotJson),
    }
}

fn looks_like_html(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<!doctype")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<HTML")
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn normalize_base_url(mut base_url: Url) -> Result<Url, WaterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        return Err(WaterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = if path.is_empty() {
        "/".to_owned()
    } else {
        format!("{path}/")
    };
    base_url.set_path(&path);
    Ok(base_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_test_support::{FixtureServer, Reply};

    fn adapter(server: &FixtureServer) -> WaterAdapter {
        let base = Url::parse(server.base()).expect("fixture base");
        WaterAdapter::try_with_transport(base).expect("adapter")
    }

    #[tokio::test]
    async fn a_delivery_number_reads_the_vendors_own_record() {
        let server = FixtureServer::new(vec![Reply::json(
            r#"{"name":"合成住户","address":"紫荆公寓1号楼101"}"#,
        )]);
        let adapter = adapter(&server);
        let user = adapter.read_user("1001").await.expect("user");
        assert_eq!(user.name, "合成住户");
        assert_eq!(user.address, "紫荆公寓1号楼101");
        // The record is personal data, so its Debug carries neither value.
        let rendered = format!("{user:?}");
        assert!(!rendered.contains("合成住户"));
        assert!(!rendered.contains("紫荆公寓1号楼101"));
        assert!(rendered.contains("name_present"));
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /auser/getuser.html "));
        // The delivery number travels as the vendor's own form field.
        assert!(requests[0].contains("name=pw"));
        assert!(requests[0].contains("param=1001"));
    }

    #[tokio::test]
    async fn a_refused_delivery_number_never_reaches_the_network() {
        let server = FixtureServer::new(vec![]);
        let adapter = adapter(&server);
        for rejected in ["", "10 01", "1001/../x", "1001?x=1", "1001<script>"] {
            assert!(
                matches!(
                    adapter.read_user(rejected).await,
                    Err(WaterError::InvalidDeliveryId)
                ),
                "{rejected} must be refused"
            );
        }
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn an_html_page_is_never_an_empty_record() {
        let server = FixtureServer::new(vec![Reply::html("<html><body>系统维护中</body></html>")]);
        let adapter = adapter(&server);
        assert!(matches!(
            adapter.read_user("1001").await,
            Err(WaterError::NotJson)
        ));
    }

    #[tokio::test]
    async fn a_non_success_status_is_not_an_empty_record() {
        let server = FixtureServer::new(vec![Reply {
            status: 502,
            headers: "Content-Type: application/json\r\n".into(),
            body: "{}".into(),
        }]);
        let adapter = adapter(&server);
        assert!(matches!(
            adapter.read_user("1001").await,
            Err(WaterError::HttpStatus {
                status: StatusCode::BAD_GATEWAY
            })
        ));
    }

    #[test]
    fn an_origin_is_never_accepted_with_credentials_or_a_foreign_shape() {
        for rejected in [
            "ftp://dingshui.bjqzhd.com/",
            "http://user:secret@dingshui.bjqzhd.com/",
            "http://dingshui.bjqzhd.com/?token=1",
            "http://dingshui.bjqzhd.com/#fragment",
            "not a url",
        ] {
            assert!(
                matches!(
                    WaterAdapterConfig::new(rejected),
                    Err(WaterError::InvalidBaseUrl)
                ),
                "{rejected} must be refused"
            );
        }
        assert!(WaterAdapterConfig::new(WATER_ORIGIN).is_ok());
    }

    #[test]
    fn a_relative_path_cannot_leave_the_vendor_origin() {
        let adapter = WaterAdapter::try_with_transport(
            Url::parse("http://dingshui.bjqzhd.com/").expect("url"),
        )
        .expect("adapter");
        for rejected in [
            "auser/getuser.html",
            "/auser/../secret",
            "/auser/getuser.html?x=1",
            "/auser/getuser.html#frag",
            "/auser/://evil",
            "/auser/get\u{7}user.html",
        ] {
            assert!(
                matches!(adapter.endpoint(rejected), Err(WaterError::InvalidBaseUrl)),
                "{rejected} must be refused"
            );
        }
        let accepted = adapter.endpoint("/auser/getuser.html").expect("endpoint");
        assert_eq!(
            accepted.as_str(),
            "http://dingshui.bjqzhd.com/auser/getuser.html"
        );
    }

    #[test]
    fn brand_names_cover_the_vendors_own_identifiers() {
        assert_eq!(water_brand_name("6"), "清紫源泉矿泉水（高端）");
        assert_eq!(water_brand_name("12"), "农夫山泉桶装水（19L）");
        assert_eq!(water_brand_name("11"), "清紫源泉矿泉水");
        // A brand the table does not carry stays visible under its own id.
        assert_eq!(water_brand_name("99"), "99");
        // Every entry is reachable by its own identifier.
        for (id, name) in WATER_BRANDS {
            assert_eq!(water_brand_name(id), name);
        }
    }

    #[test]
    fn diagnostic_codes_are_distinct_per_failure_class() {
        let codes = [
            WaterError::InvalidBaseUrl.diagnostic_code(),
            WaterError::UnexpectedOrigin.diagnostic_code(),
            WaterError::UnexpectedDeployment.diagnostic_code(),
            WaterError::NotJson.diagnostic_code(),
            WaterError::InvalidDeliveryId.diagnostic_code(),
        ];
        let mut unique = codes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len());
        assert!(codes.iter().all(|code| code.starts_with("water_")));
    }

    #[test]
    fn debug_output_never_carries_a_query_or_a_credential() {
        let config = WaterAdapterConfig::new(WATER_ORIGIN).expect("config");
        let rendered = format!("{config:?}");
        assert!(rendered.contains("dingshui.bjqzhd.com"));
        assert!(!rendered.contains("token"));
        let adapter = WaterAdapter::try_with_transport(Url::parse(WATER_ORIGIN).expect("url"))
            .expect("adapter");
        let rendered = format!("{adapter:?}");
        assert!(rendered.contains("dingshui.bjqzhd.com"));
        assert!(!rendered.contains("param"));
    }
}
