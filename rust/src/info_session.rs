//! Cookie-aware INFO/WebVPN handoff and read-only news execution.
//
// This adapter is separate from the INFO route profiles and pure news
// parsers. It owns the orchestration needed to use them with the identity
// session's shared Cookie jar. It never exposes a cookie, CSRF value, ticket,
// HTML document, or server-provided URL in a result or diagnostic string.
//
// The WebVPN target prefix and yyfwid payload are configuration inputs. Older
// public clients contain encoded host mappings for one deployment, but those
// mappings are deployment details and must not be copied as constants into a
// new client.

use std::fmt;

use reqwest::{
    Method, StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::{
    info::{InfoError, InfoParameterEncoding, InfoPortalProfile},
    info_client::{InfoClient, InfoClientConfig, InfoClientError, InfoHtmlClassification},
    info_news::{
        NewsChannelOption, NewsDetail, NewsFeedKind, NewsItem, NewsPage, NewsParseError,
        NewsProfile, NewsRequestPlan, NewsSearchInput, NewsSourceOption, NewsSubscriptionRule,
        parse_news_channels, parse_news_detail, parse_news_favorite_page, parse_news_page,
        parse_news_pdf, parse_news_sources, parse_news_subscription_page, parse_news_subscriptions,
        parse_news_system_pdf,
    },
    protocol::{CsrfToken, ServiceId, ServiceSessionState, UserIdentity},
    session::{SessionCoordinator, SessionError, SessionSnapshot},
    transport::{CampusHttpTransport, TransportError},
};

const DEFAULT_COOKIE_PATH: &str = "/wengine-vpn/cookie";
const DEFAULT_COOKIE_HOST: &str = "info.tsinghua.edu.cn";
const DEFAULT_COOKIE_SCHEME: &str = "https";
const DEFAULT_COOKIE_TARGET_PATH: &str = "/f/info/gxfw_fg/common/index";
const DEFAULT_IDENTITY_ORIGIN: &str = "https://id.tsinghua.edu.cn/";
// THUInfo core.verifyAndReLogin probes this account endpoint; F315... is
// Coremail's roaming selector, not an INFO session-establishment target.
const PORTAL_ACCOUNT_PATH: &str = "/b/info/gxfw_fg/common/grjbxx";
const DEFAULT_PORTAL_REDIRECT_PATH: &str =
    "/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect";
const DEFAULT_PDF_STREAM_PATH: &str = "/b/info/wj/downloadPdfStream";
const DEFAULT_SYSTEM_PDF_PATH: &str = "/tsinghua.war/yct.www.api/publish";
const NEWS_SOURCES_PATH: &str = "/b/info/gxfw_fg/common/querySubscribeInformationUnitList";
const NEWS_CHANNELS_PATH: &str = "/b/info/xxfb_fg/teacher/lm/subscribe/getlmListByDwh";
const NEWS_SUBSCRIPTIONS_PATH: &str = "/b/info/gxfw_fg/common/querySubscribeConditionNameList/XXFB";
const NEWS_FAVORITES_PATH: &str = "/b/info/gxfw_fg/common/queryFavoriteXxfbPageList";
const NEWS_SUBSCRIPTION_FEED_PATH: &str = "/b/info/gxfw_fg/common/querySubscribeInfomationPageList";

enum PersonalNewsRequest<'a> {
    Rules,
    Favorites(u32),
    Subscription { page: u32, rule_id: &'a str },
}
// Pinned to THUInfo's SYSC_PDF_NEWS_PREFIX; not an inferred host mapping.
const SYSTEM_PUBLICATION_MAPPING_ID: &str =
    "77726476706e69737468656265737421e3f5468534367f1e6d119aafd641303ceb8f9190006d6afc78336870";
const LOGIN_MARKERS: &[&str] = &[
    "name=\"i_user\"",
    "name='i_user'",
    "name=\"i_pass\"",
    "name='i_pass'",
    "/do/off/ui/auth/login",
    "登录失效",
    "会话已过期",
    "请先登录",
    "请登录",
    "未登录",
    "session expired",
    "not logged in",
    "login required",
    "authentication required",
    "unauthorized",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoWebVpnConfig {
    pub webvpn_base_url: Url,
    identity_origin: Url,
    pub target_prefix: String,
    pub cookie_path: String,
    pub cookie_host: String,
    pub cookie_scheme: String,
    pub cookie_target_path: String,
    pub portal: InfoClientConfig,
    pub news: NewsProfile,
    portal_encoding: InfoParameterEncoding,
}

impl InfoWebVpnConfig {
    pub fn new(
        webvpn_base_url: impl Into<String>,
        target_prefix: impl Into<String>,
    ) -> Result<Self, InfoSessionError> {
        let base_text = webvpn_base_url.into();
        let webvpn_base_url = Url::parse(&base_text)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        validate_origin(&webvpn_base_url)?;

        let target_prefix = normalize_target_prefix(&target_prefix.into())?;
        let portal_path = append_target_path(&target_prefix, DEFAULT_PORTAL_REDIRECT_PATH)?;
        let portal_profile =
            InfoPortalProfile::new(portal_path).map_err(InfoSessionError::Profile)?;
        let portal = InfoClientConfig::new(webvpn_base_url.to_string(), portal_profile)?;

        let config = Self {
            webvpn_base_url,
            identity_origin: Url::parse(DEFAULT_IDENTITY_ORIGIN)
                .expect("default identity origin is a valid URL"),
            target_prefix,
            cookie_path: DEFAULT_COOKIE_PATH.to_owned(),
            cookie_host: DEFAULT_COOKIE_HOST.to_owned(),
            cookie_scheme: DEFAULT_COOKIE_SCHEME.to_owned(),
            cookie_target_path: DEFAULT_COOKIE_TARGET_PATH.to_owned(),
            portal,
            news: NewsProfile::standard(),
            portal_encoding: InfoParameterEncoding::Query,
        };
        config.validate()?;
        Ok(config)
    }

    /// Restrict server-issued login continuations to an explicitly supplied
    /// Identity origin. Production callers keep the deployed Identity origin
    /// above; loopback fixtures may provide their own distinct origin without
    /// weakening the origin or login-path checks.
    pub fn with_identity_origin(
        mut self,
        identity_origin: impl Into<String>,
    ) -> Result<Self, InfoSessionError> {
        let identity_origin = Url::parse(&identity_origin.into())
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        validate_origin(&identity_origin)?;
        self.identity_origin = identity_origin;
        self.validate()?;
        Ok(self)
    }

    pub fn with_cookie_target(
        mut self,
        host: impl Into<String>,
        scheme: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<Self, InfoSessionError> {
        self.cookie_host = host.into();
        self.cookie_scheme = scheme.into();
        self.cookie_target_path = path.into();
        self.validate()?;
        Ok(self)
    }

    pub fn with_portal_encoding(
        mut self,
        encoding: InfoParameterEncoding,
    ) -> Result<Self, InfoSessionError> {
        self.portal_encoding = encoding;
        Ok(self)
    }

    pub fn portal_encoding(&self) -> InfoParameterEncoding {
        self.portal_encoding
    }

    pub fn target_url(&self, path: &str) -> Result<Url, InfoSessionError> {
        let path = append_target_path(&self.target_prefix, path)?;
        let endpoint = self
            .webvpn_base_url
            .join(&path)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        if !same_origin(&self.webvpn_base_url, &endpoint) {
            return Err(InfoSessionError::InvalidConfig(
                "INFO target endpoint must stay on the WebVPN origin".to_owned(),
            ));
        }
        Ok(endpoint)
    }

    fn validate(&self) -> Result<(), InfoSessionError> {
        validate_origin(&self.webvpn_base_url)?;
        validate_origin(&self.identity_origin)?;
        // These fields are public deployment inputs. Revalidate the prefix
        // here so a caller cannot mutate a valid config into an unsafe route
        // before constructing the adapter.
        normalize_target_prefix(&self.target_prefix)?;
        validate_relative_path("cookie_path", &self.cookie_path)?;
        validate_relative_path("cookie_target_path", &self.cookie_target_path)?;
        validate_wire_value("cookie_host", &self.cookie_host, true)?;
        validate_wire_value("cookie_scheme", &self.cookie_scheme, true)?;
        self.news
            .validate()
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        Ok(())
    }

    fn is_identity_login_target(&self, url: &Url) -> bool {
        same_origin(&self.identity_origin, url) && is_identity_login_target(url)
    }
}

impl Default for InfoWebVpnConfig {
    fn default() -> Self {
        Self::new("https://webvpn.invalid/", "/https/")
            .expect("the metadata-only INFO profile must be valid")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalResourceAuthStage {
    HttpRejected,
    WebVpnLogin,
    IdentityLogin,
    DocumentLogin,
}

impl PortalResourceAuthStage {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::HttpRejected => "portal_resource_http_rejected",
            Self::WebVpnLogin => "portal_resource_webvpn_login",
            Self::IdentityLogin => "portal_resource_identity_login",
            Self::DocumentLogin => "portal_resource_document_login",
        }
    }
}

#[derive(Debug, Error)]
pub enum InfoSessionError {
    #[error("invalid INFO/WebVPN configuration: {0}")]
    InvalidConfig(String),

    #[error(transparent)]
    Client(#[from] InfoClientError),

    #[error(transparent)]
    Profile(#[from] InfoError),

    // A reqwest transport error may include the full request URL. News
    // requests carry `_csrf` in their query string, so keep that source
    // private at the INFO service boundary.
    #[error("INFO transport request failed")]
    Transport(#[from] TransportError),

    #[error(transparent)]
    Session(#[from] SessionError),

    #[error("unified identity session is not authenticated")]
    IdentityNotAuthenticated,

    #[error("unified identity session has no bound user")]
    IdentityUserMissing,

    #[error("INFO handoff user does not match the authenticated identity")]
    IdentityUserMismatch,

    #[error("INFO portal account request returned HTTP {status}")]
    PortalAccountStatus { status: StatusCode },

    #[error("INFO portal account response route is invalid")]
    PortalAccountPath,

    #[error("INFO mapped resource explicitly requires authentication")]
    PortalResourceLoginRequired,

    // Fixed phase only; do not carry status text, URL, document, or fields.
    #[error("INFO mapped resource authentication is unavailable")]
    PortalResourceAuthRequired(PortalResourceAuthStage),

    // An ephemeral server-issued navigation target, not authentication.
    // OpaqueUrl's Debug is redacted; Display deliberately omits its value.
    #[error("INFO portal resource requires an identity continuation")]
    PortalResourceContinuation(crate::info::OpaqueUrl),

    #[error("INFO mapped resource returned HTTP {status}")]
    PortalResourceStatus { status: StatusCode },

    #[error("INFO mapped resource did not return the expected portal document")]
    PortalResourceUnexpected,

    #[error("INFO portal account response is invalid")]
    PortalAccountResponse,

    #[error("INFO portal account does not match the restored identity")]
    PortalAccountMismatch,

    #[error("INFO cookie bootstrap returned HTTP {status}")]
    CookieBootstrapStatus { status: StatusCode },

    #[error("INFO cookie bootstrap did not provide CSRF evidence")]
    MissingCookieCsrf,

    #[error("INFO cookie bootstrap ended at an unexpected path")]
    CookieBootstrapPath,

    #[error("INFO handoff returned HTTP {status}")]
    HandoffStatus { status: StatusCode },

    #[error("INFO handoff returned a login page")]
    LoginRequired,

    #[error("INFO handoff did not return an authenticated service page")]
    SessionProofMissing,

    #[error("INFO handoff ended outside a safe mapped service path")]
    HandoffUnexpectedPath,

    #[error("INFO response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("INFO news request returned HTTP {status}")]
    NewsHttpStatus { status: StatusCode },

    #[error("INFO news navigation repeated a URL")]
    NewsNavigationCycle,
    #[error("INFO news navigation exceeded its bounded hop limit")]
    NewsNavigationLimit,

    #[error("INFO news response came from an unexpected origin")]
    NewsUnexpectedOrigin,

    #[error("INFO news response ended outside the configured target path")]
    NewsUnexpectedPath,

    #[error("INFO news response could not be parsed: {0}")]
    News(#[from] NewsParseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoSessionResult {
    pub snapshot: SessionSnapshot,
    /// The server-provided WebVPN handoff is retained only inside the Rust
    /// runtime so follow-up adapters can reuse the same mapped origin. It is
    /// intentionally not an FRB DTO and its Debug representation is redacted.
    pub roaming_url: crate::info::OpaqueUrl,
}

pub struct InfoSessionAdapter {
    config: InfoWebVpnConfig,
    client: InfoClient,
    transport: CampusHttpTransport,
}

enum NewsLinkResolution {
    ApiArticleId(String),
    CompleteDetail(NewsDetail),
}

struct NewsDocumentResponse {
    // Fragment identifiers belong to browser navigation, not the HTTP target.
    // Preserve the validated hash separately from reqwest's response URL.
    navigation_fragment: Option<String>,
    status: StatusCode,
    final_url: Url,
    content_type: Option<String>,
    redirect_location: Option<String>,
    body: Vec<u8>,
}

impl fmt::Debug for InfoSessionAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InfoSessionAdapter")
            .field("config", &self.config)
            .field("client", &self.client)
            .field("transport", &"cookie-aware")
            .finish()
    }
}

impl InfoSessionAdapter {
    pub fn new(
        config: InfoWebVpnConfig,
        transport: CampusHttpTransport,
    ) -> Result<Self, InfoSessionError> {
        config.validate()?;
        let client = InfoClient::new(config.portal.clone())?;
        Ok(Self {
            config,
            client,
            transport,
        })
    }

    pub fn config(&self) -> &InfoWebVpnConfig {
        &self.config
    }

    pub fn client(&self) -> &InfoClient {
        &self.client
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Test restored portal cookies against the same account-bound endpoint
    /// as THUInfo, without entering a credential form or creating an SSO
    /// target session. The parsed user information is discarded in Rust.
    pub(crate) async fn probe_portal_account(
        &self,
        user: &UserIdentity,
    ) -> Result<CsrfToken, InfoSessionError> {
        self.probe_portal_account_with_resource(user, false).await
    }

    pub(crate) async fn probe_portal_account_with_resource(
        &self,
        user: &UserIdentity,
        initialize_resource: bool,
    ) -> Result<CsrfToken, InfoSessionError> {
        self.probe_portal_account_with_handoff(user, initialize_resource)
            .await
            .map_err(|error| match error {
                // Preserve the existing non-navigation API contract.
                InfoSessionError::PortalResourceContinuation(_)
                | InfoSessionError::PortalResourceAuthRequired(_) => {
                    InfoSessionError::PortalResourceLoginRequired
                }
                other => other,
            })
    }

    pub(crate) async fn probe_portal_account_with_handoff(
        &self,
        user: &UserIdentity,
        initialize_resource: bool,
    ) -> Result<CsrfToken, InfoSessionError> {
        if user.username.trim().is_empty() {
            return Err(InfoSessionError::IdentityUserMissing);
        }
        let csrf = match self.bootstrap_cookie().await {
            Err(InfoSessionError::MissingCookieCsrf) if initialize_resource => {
                // The Wengine cookie API is a read of the target jar, not an
                // initializer. Visit the already-authenticated mapped portal
                // once before concluding that its cookie-only session failed.
                self.initialize_portal_resource().await?;
                self.bootstrap_cookie().await?
            }
            result => result?,
        };
        let endpoint = self.config.target_url(PORTAL_ACCOUNT_PATH)?;
        let query = [("_csrf".to_owned(), csrf.as_str().to_owned())];
        let response = self
            .transport
            .send(self.transport.client().get(endpoint.clone()).query(&query))
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        if is_authentication_status(status) || self.config.is_identity_login_target(&final_url) {
            return Err(InfoSessionError::LoginRequired);
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url)
            || final_url.path() != endpoint.path()
            || !query_matches(&query, &final_url)
            || response.headers().contains_key(LOCATION)
        {
            return Err(InfoSessionError::PortalAccountPath);
        }
        if !status.is_success() {
            return Err(InfoSessionError::PortalAccountStatus { status });
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        if is_login_page(&final_url, content_type.as_deref(), &body) {
            return Err(InfoSessionError::LoginRequired);
        }
        validate_portal_account(&body, &user.username)?;
        Ok(csrf)
    }

    async fn initialize_portal_resource(&self) -> Result<(), InfoSessionError> {
        let endpoint = self.config.target_url(DEFAULT_COOKIE_TARGET_PATH)?;
        let mut response = self
            .transport
            .send(self.transport.client().get(endpoint.clone()))
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let target = response
            .headers()
            .get(LOCATION)
            .map(|value| {
                let value = value
                    .to_str()
                    .map_err(|_| InfoSessionError::PortalResourceUnexpected)?;
                final_url
                    .join(value)
                    .map_err(|_| InfoSessionError::PortalResourceUnexpected)
            })
            .transpose()?;
        if let Some(target) = target.as_ref().filter(|_| status.is_redirection()) {
            tracing::debug!(
                target: "tsinghua_kit::auth",
                event = "portal_route_decision",
                service = "webvpn",
                route_class = portal_resource_redirect_route_class(&self.config, target),
                route_decision = "http_location",
                hop = 0_u64,
            );
        }
        let login_target = |url: &Url| self.config.is_identity_login_target(url);
        // A mapped WebVPN resource can redirect to the gateway root before
        // the target application's login/SSO page is exposed.  The shared
        // transport deliberately stops at this cross-mapping transition so
        // it does not silently forward a business request into another
        // WebVPN mapping.  Treat only the public gateway entry routes as an
        // authentication boundary; arbitrary WebVPN paths remain ordinary
        // route/HTTP failures and must not trigger a credential replay.
        let webvpn_login = |url: &Url| {
            same_origin(&self.config.webvpn_base_url, url)
                && url.fragment().is_none()
                && (matches!(url.path(), "/" | "/login" | "/f/login")
                    || crate::portal_resume::mapped_identity_login_path(url))
        };
        if status.is_redirection() {
            if let Some(url) = target.as_ref().filter(|url| {
                self.config.is_identity_login_target(url)
                    && url.fragment().is_none()
                    && (url.path().starts_with("/do/off/ui/auth/login/form/")
                        || url.path() == "/do/off/ui/auth/login/redirect2Jsp")
            }) {
                // Preserve the server-selected app/context in Rust. Do not
                // start an unrelated hard-coded/new WebVPN login instead.
                let handoff = crate::info::OpaqueUrl::new(url.to_string())
                    .map_err(|_| InfoSessionError::PortalResourceUnexpected)?;
                return Err(InfoSessionError::PortalResourceContinuation(handoff));
            }
        }
        if is_authentication_status(status) {
            return Err(InfoSessionError::PortalResourceAuthRequired(
                PortalResourceAuthStage::HttpRejected,
            ));
        }
        if webvpn_login(&final_url) || target.as_ref().is_some_and(webvpn_login) {
            return Err(InfoSessionError::PortalResourceAuthRequired(
                PortalResourceAuthStage::WebVpnLogin,
            ));
        }
        if login_target(&final_url) || target.as_ref().is_some_and(login_target) {
            let stage = PortalResourceAuthStage::IdentityLogin;
            return Err(InfoSessionError::PortalResourceAuthRequired(stage));
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url)
            || target
                .as_ref()
                .is_some_and(|url| !same_origin(&self.config.webvpn_base_url, url))
        {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if !status.is_success() {
            return Err(InfoSessionError::PortalResourceStatus { status });
        }
        if final_url.path() != endpoint.path() || target.is_some() {
            return Err(InfoSessionError::PortalResourceUnexpected);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(TransportError::Decode)? {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(InfoSessionError::PortalResourceUnexpected);
            }
            bytes.extend_from_slice(&chunk);
        }
        let body =
            std::str::from_utf8(&bytes).map_err(|_| InfoSessionError::PortalResourceUnexpected)?;
        if crate::identity_client::service_resource_document_requires_login(body) {
            return Err(InfoSessionError::PortalResourceAuthRequired(
                PortalResourceAuthStage::DocumentLogin,
            ));
        }
        if !looks_like_html_document(content_type.as_deref(), body) {
            return Err(InfoSessionError::PortalResourceUnexpected);
        }
        // This only initializes the target jar. CSRF + bound account proof
        // must still follow before the caller grants an authenticated service.
        Ok(())
    }

    /// INFO already is the information portal. Unlike mail/library targets,
    /// it must not call onlineAppRedirect with an unrelated application's id.
    /// Root proof may supply a freshly obtained CSRF to avoid another read.
    pub(crate) async fn establish_portal(
        &self,
        coordinator: &mut SessionCoordinator,
        user: UserIdentity,
        fresh_csrf: Option<CsrfToken>,
    ) -> Result<InfoSessionResult, InfoSessionError> {
        let identity = coordinator.registry().snapshot_for(ServiceId::Identity);
        if identity.state != ServiceSessionState::Authenticated {
            return Err(InfoSessionError::IdentityNotAuthenticated);
        }
        if identity.user.as_ref().map(|u| &u.username) != Some(&user.username) {
            return Err(InfoSessionError::IdentityUserMismatch);
        }
        coordinator.begin_authentication(ServiceId::Info)?;
        let result = async {
            let csrf = match fresh_csrf {
                Some(csrf) => csrf,
                None => self.bootstrap_cookie().await?,
            };
            let csrf = self.probe_news_with_csrf(csrf).await?;
            let home = self.config.target_url(DEFAULT_COOKIE_TARGET_PATH)?;
            let roaming_url =
                crate::info::OpaqueUrl::new(home.to_string()).map_err(InfoSessionError::Profile)?;
            let bound = coordinator.registry().bind_csrf(ServiceId::Info, csrf);
            let snapshot =
                coordinator.mark_authenticated(ServiceId::Info, user, None, Some(bound), None)?;
            Ok(InfoSessionResult {
                snapshot,
                roaming_url,
            })
        }
        .await;
        if result.is_err() {
            let _ = coordinator.cancel_authentication(ServiceId::Info);
        }
        result
    }

    pub async fn establish(
        &self,
        coordinator: &mut SessionCoordinator,
        user: UserIdentity,
        yyfwid: &str,
    ) -> Result<InfoSessionResult, InfoSessionError> {
        if coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .state
            != ServiceSessionState::Authenticated
        {
            return Err(InfoSessionError::IdentityNotAuthenticated);
        }
        if yyfwid.trim().is_empty() || yyfwid.chars().any(char::is_control) {
            return Err(InfoSessionError::InvalidConfig(
                "INFO yyfwid must be non-empty and contain no control characters".to_owned(),
            ));
        }

        let identity_user = coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .user
            .ok_or(InfoSessionError::IdentityUserMissing)?;
        if identity_user.username != user.username {
            return Err(InfoSessionError::IdentityUserMismatch);
        }

        coordinator.begin_authentication(ServiceId::Info)?;
        let result = self.establish_inner(coordinator, user, yyfwid).await;
        if result.is_err() {
            let _ = coordinator.cancel_authentication(ServiceId::Info);
        }
        result
    }

    /// Obtains and proves another INFO/WebVPN application handoff using the
    /// already authenticated identity Cookie jar.  This is used for services
    /// such as the library whose `yyfwid` differs from the main INFO portal.
    /// It deliberately does not mutate the INFO session registry again.
    pub async fn additional_roaming(
        &self,
        yyfwid: &str,
    ) -> Result<crate::info::OpaqueUrl, InfoSessionError> {
        if yyfwid.trim().is_empty()
            || yyfwid.chars().any(char::is_control)
            || yyfwid.contains(['/', '?', '#', ':'])
        {
            return Err(InfoSessionError::InvalidConfig(
                "INFO yyfwid must be non-empty and contain no control characters".to_owned(),
            ));
        }
        let csrf = self.bootstrap_cookie().await?;
        let roaming = self
            .client
            .online_app_redirect_with_encoding(
                &self.transport,
                yyfwid,
                csrf.as_str(),
                self.config.portal_encoding,
            )
            .await
            .map_err(map_info_handoff_error)?;
        // The portal response is only the handoff entry point.  The WebVPN
        // client may follow one or more same-origin redirects before the
        // application page is reached.  Return the URL that was actually
        // proved, rather than the stale entry point from `roamingurl`; the
        // caller will append service routes to this opaque mapping.
        let mapped = self.map_additional_roaming(yyfwid, roaming.roaming_url())?;
        let final_url = if yyfwid == crate::thos::ROAM_ID {
            crate::thos::follow_handoff(
                &self.transport,
                &self.config.webvpn_base_url,
                mapped.as_str(),
            )
            .await
            .map_err(|error| match error {
                crate::thos::ThosError::Session => InfoSessionError::LoginRequired,
                _ => InfoSessionError::HandoffUnexpectedPath,
            })?
        } else {
            self.probe_additional_roaming_page(mapped.as_str()).await?
        };
        crate::info::OpaqueUrl::new(final_url.to_string()).map_err(InfoSessionError::Profile)
    }

    /// THUInfo default roam applies parseUrl to the direct campus target.
    /// The selector determines the allowed mapping; no untrusted host table
    /// or arbitrary cross-origin request is learned from the response.
    pub(crate) fn map_additional_roaming(
        &self,
        selector: &str,
        target: &crate::info::OpaqueUrl,
    ) -> Result<crate::info::OpaqueUrl, InfoSessionError> {
        let url = Url::parse(target.as_str()).map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        let known = match selector {
            "40470BB47E0849E9EF717983490BC964"
            | "287C0C6D90ABB364CD5FDF1495199962"
            | "BEABB32641DC4EC3510B048BAF42471A"
            | "B7EF0ADF9406335AD7905B30CD7B49B1"
            | "E35232808C08C8C5F199F13BF6B7F5D0" => (
                "zhjw.cic.tsinghua.edu.cn",
                "http",
                "77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290",
            ),
            "3E401364BDD7AEA7EBF1EDE3F15ED4B7" => (
                "learn.tsinghua.edu.cn",
                "https",
                "77726476706e69737468656265737421fcf2408e297e7c4377068ea48d546d30ca8cc97bcc",
            ),
            crate::thos::ROAM_ID => ("thos.tsinghua.edu.cn", "https", crate::thos::MAPPING),
            _ => {
                if same_origin(&self.config.webvpn_base_url, &url) {
                    return Ok(target.clone());
                }
                return Err(InfoSessionError::UnexpectedOrigin);
            }
        };
        let (host, scheme, mapping) = known;
        if same_origin(&self.config.webvpn_base_url, &url) {
            let prefix = format!("/{scheme}/{mapping}/");
            if !url.path().starts_with(&prefix)
                || !crate::webvpn_url::redirect_path_stays_in_mapping(&url, scheme, mapping)
            {
                return Err(InfoSessionError::HandoffUnexpectedPath);
            }
            return Ok(target.clone());
        }
        if url.host_str() != Some(host)
            || url.scheme() != scheme
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || invalid_percent_encoding(url.path())
            || invalid_percent_encoding(url.query().unwrap_or_default())
            || path_contains_encoded_escape(url.path())
        {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        let mut mapped = self.config.webvpn_base_url.clone();
        mapped.set_path(&format!("/{scheme}/{mapping}{}", url.path()));
        mapped.set_query(url.query());
        crate::info::OpaqueUrl::new(mapped.as_str()).map_err(InfoSessionError::Profile)
    }

    async fn establish_inner(
        &self,
        coordinator: &mut SessionCoordinator,
        user: UserIdentity,
        yyfwid: &str,
    ) -> Result<InfoSessionResult, InfoSessionError> {
        let csrf = self.bootstrap_cookie().await?;
        let roaming = self
            .client
            .online_app_redirect_with_encoding(
                &self.transport,
                yyfwid,
                csrf.as_str(),
                self.config.portal_encoding,
            )
            .await
            .map_err(map_info_handoff_error)?;

        let final_url = self
            .probe_roaming_page(roaming.roaming_url().as_str())
            .await?;
        // A non-login HTML handoff is only a navigation result. Prove the
        // actual INFO application session with the real news endpoint before
        // promoting the coordinator state. A valid empty dataList is still
        // accepted because its wrapper and endpoint have been verified.
        let csrf = self.probe_news_session().await?;
        let proved_roaming_url = crate::info::OpaqueUrl::new(final_url.to_string())
            .map_err(InfoSessionError::Profile)?;

        let bound_csrf = coordinator.registry().bind_csrf(ServiceId::Info, csrf);
        let snapshot =
            coordinator.mark_authenticated(ServiceId::Info, user, None, Some(bound_csrf), None)?;
        Ok(InfoSessionResult {
            snapshot,
            roaming_url: proved_roaming_url,
        })
    }

    async fn probe_news_session(&self) -> Result<CsrfToken, InfoSessionError> {
        // A fresh handoff may rotate target cookies before the news proof.
        let csrf = self.bootstrap_cookie().await?;
        self.probe_news_with_csrf(csrf).await
    }

    async fn probe_news_with_csrf(&self, csrf: CsrfToken) -> Result<CsrfToken, InfoSessionError> {
        let plan = self
            .config
            .news
            .list_request(1, 1, None, None)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        let endpoint = self.config.target_url(plan.path())?;
        let expected_path = endpoint.path().to_owned();
        let mut query = plan.query_parameters().to_vec();
        query.push((
            plan.csrf_requirement().field.clone(),
            csrf.as_str().to_owned(),
        ));
        let request = self
            .transport
            .client()
            .get(endpoint)
            .query(&query)
            .build()
            .map_err(|error| {
                InfoSessionError::InvalidConfig(format!("INFO news probe: {error}"))
            })?;
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        let redirect_target = redirect_location
            .as_deref()
            .map(|location| resolve_location(&final_url, location))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        if redirect_target
            .as_ref()
            .is_some_and(|target| !same_origin(&self.config.webvpn_base_url, target))
            || !same_origin(&self.config.webvpn_base_url, &final_url)
        {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || redirect_target
                .as_ref()
                .is_some_and(|target| self.config.is_identity_login_target(target))
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if is_authentication_status(status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if redirect_target
            .as_ref()
            .is_some_and(|target| target.path() != expected_path)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if !status.is_success() {
            return Err(InfoSessionError::NewsHttpStatus { status });
        }
        if is_login_page(&final_url, content_type.as_deref(), &body) {
            return Err(InfoSessionError::LoginRequired);
        }
        if final_url.path() != expected_path || !query_matches(&query, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if !path_has_prefix(final_url.path(), &self.config.target_prefix) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        parse_news_page(&body, NewsFeedKind::List)
            .map(|_| csrf)
            .map_err(|error| match error {
                NewsParseError::HtmlLoginPage | NewsParseError::LoginRequired => {
                    InfoSessionError::LoginRequired
                }
                error => InfoSessionError::News(error),
            })
    }

    async fn bootstrap_cookie(&self) -> Result<CsrfToken, InfoSessionError> {
        let endpoint = self
            .config
            .webvpn_base_url
            .join(&self.config.cookie_path)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        let response = self
            .transport
            .send(
                self.transport
                    .client()
                    .request(Method::GET, endpoint.clone())
                    .query(&[
                        ("method", "get"),
                        ("host", self.config.cookie_host.as_str()),
                        ("scheme", self.config.cookie_scheme.as_str()),
                        ("path", self.config.cookie_target_path.as_str()),
                    ]),
            )
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        let redirect_target = redirect_location
            .as_deref()
            .map(|location| resolve_location(&final_url, location))
            .transpose()
            .map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        if redirect_target
            .as_ref()
            .is_some_and(|target| !same_origin(&self.config.webvpn_base_url, target))
        {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || redirect_target
                .as_ref()
                .is_some_and(|target| self.config.is_identity_login_target(target))
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if is_authentication_status(status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if !status.is_success() {
            return Err(InfoSessionError::CookieBootstrapStatus { status });
        }
        let lower_body = body.to_ascii_lowercase();
        if is_login_page(&final_url, content_type.as_deref(), &lower_body) {
            return Err(InfoSessionError::LoginRequired);
        }
        let expected_query = [
            ("method".to_owned(), "get".to_owned()),
            ("host".to_owned(), self.config.cookie_host.clone()),
            ("scheme".to_owned(), self.config.cookie_scheme.clone()),
            ("path".to_owned(), self.config.cookie_target_path.clone()),
        ];
        if final_url.path() != self.config.cookie_path
            || !query_matches(&expected_query, &final_url)
        {
            return Err(InfoSessionError::CookieBootstrapPath);
        }
        // Wengine's response BODY describes cookies for the requested target
        // host/path. Its own Set-Cookie or a stale WebVPN-root cookie is not
        // evidence of the proxied INFO application's CSRF.
        let token = extract_cookie_token(&body, "XSRF-TOKEN")
            .and_then(CsrfToken::new)
            .ok_or(InfoSessionError::MissingCookieCsrf)?;
        // This is a proxied target cookie, not a Set-Cookie for WebVPN.
        // Pass the token only to the target API query; never overwrite the
        // wrapper's cookie jar or persist a fabricated root-domain CSRF.
        Ok(token)
    }

    async fn probe_roaming_page(&self, roaming_url: &str) -> Result<Url, InfoSessionError> {
        let url = Url::parse(roaming_url).map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        if !same_origin(&self.config.webvpn_base_url, &url) {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if !path_has_prefix(url.path(), &self.config.target_prefix) {
            return Err(InfoSessionError::HandoffUnexpectedPath);
        }
        let response = self
            .transport
            .send(self.transport.client().request(Method::GET, url))
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        let content_type = content_type.map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        let redirect_target = redirect_location
            .as_deref()
            .map(|location| resolve_location(&final_url, location))
            .transpose()
            .map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        if redirect_target
            .as_ref()
            .is_some_and(|target| !same_origin(&self.config.webvpn_base_url, target))
        {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || redirect_target
                .as_ref()
                .is_some_and(|target| self.config.is_identity_login_target(target))
        {
            return Err(InfoSessionError::LoginRequired);
        }
        // The HTTP authentication status is authoritative for this request.
        // Check it before interpreting a same-origin Location path; an
        // expired session may point at an arbitrary error or login route and
        // must not be reported as a route mismatch.
        if is_authentication_status(status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if let Some(location) = redirect_location.as_deref() {
            let location = resolve_location(&final_url, location)?;
            if is_identity_login_path(&location.path().to_ascii_lowercase()) {
                return Err(InfoSessionError::LoginRequired);
            }
            if !path_has_prefix(location.path(), &self.config.target_prefix) {
                return Err(InfoSessionError::HandoffUnexpectedPath);
            }
        }
        if !status.is_success() {
            return Err(InfoSessionError::HandoffStatus { status });
        }
        let lower = body.to_ascii_lowercase();
        if is_login_page(&final_url, content_type.as_deref(), &lower) {
            return Err(InfoSessionError::LoginRequired);
        }
        if !path_has_prefix(final_url.path(), &self.config.target_prefix) {
            return Err(InfoSessionError::SessionProofMissing);
        }
        if body.trim().is_empty() || !looks_like_html_document(content_type.as_deref(), &lower) {
            return Err(InfoSessionError::SessionProofMissing);
        }
        Ok(final_url)
    }

    async fn probe_additional_roaming_page(
        &self,
        roaming_url: &str,
    ) -> Result<Url, InfoSessionError> {
        let url = Url::parse(roaming_url).map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        let mapping_prefix =
            mapped_path_prefix(url.path()).ok_or(InfoSessionError::HandoffUnexpectedPath)?;
        validate_additional_roaming_url(&self.config.webvpn_base_url, &url, &mapping_prefix)?;

        let response = self
            .transport
            .send(self.transport.client().request(Method::GET, url))
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::UnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;

        if let Some(location) = redirect_location.as_deref() {
            let location = resolve_location(&final_url, location)?;
            if is_identity_login_path(&location.path().to_ascii_lowercase()) {
                return Err(InfoSessionError::LoginRequired);
            }
            validate_additional_roaming_url(
                &self.config.webvpn_base_url,
                &location,
                &mapping_prefix,
            )?;
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::UnexpectedOrigin);
        }
        if is_authentication_status(status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if !matches!(status.as_u16(), 200 | 201) {
            return Err(InfoSessionError::HandoffStatus { status });
        }
        let lower = body.to_ascii_lowercase();
        if is_login_page(&final_url, content_type.as_deref(), &lower) {
            return Err(InfoSessionError::LoginRequired);
        }
        validate_additional_roaming_url(&self.config.webvpn_base_url, &final_url, &mapping_prefix)?;
        // This is navigation, not a business read. THUInfo accepts an empty
        // 200/201 response here; callers must still read and parse the real
        // course home or classroom directory before promoting a service.
        Ok(final_url)
    }

    pub async fn fetch_news_list(
        &self,
        coordinator: &SessionCoordinator,
        page: u32,
        page_size: u32,
        source_id: Option<&str>,
        channel_id: Option<&str>,
    ) -> Result<NewsPage, InfoSessionError> {
        let plan = self
            .config
            .news
            .list_request(page, page_size, source_id, channel_id)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        self.execute_news(coordinator, plan, NewsFeedKind::List)
            .await
    }

    pub async fn search_news(
        &self,
        coordinator: &SessionCoordinator,
        input: &NewsSearchInput,
    ) -> Result<NewsPage, InfoSessionError> {
        let plan = self
            .config
            .news
            .search_request(input)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        self.execute_news(coordinator, plan, NewsFeedKind::Search)
            .await
    }

    pub async fn fetch_news_sources(
        &self,
        coordinator: &SessionCoordinator,
    ) -> Result<Vec<NewsSourceOption>, InfoSessionError> {
        let body = self
            .read_news_catalog(coordinator, NEWS_SOURCES_PATH, true)
            .await?;
        parse_news_sources(&body).map_err(map_news_detail_error)
    }

    pub async fn fetch_news_channels(
        &self,
        coordinator: &SessionCoordinator,
    ) -> Result<Vec<NewsChannelOption>, InfoSessionError> {
        let body = self
            .read_news_catalog(coordinator, NEWS_CHANNELS_PATH, false)
            .await?;
        parse_news_channels(&body).map_err(map_news_detail_error)
    }

    /// One fixed INFO subscription list. The server's rule IDs remain in
    /// Rust; neither Flutter nor a log receives the underlying request URL.
    pub async fn fetch_news_subscriptions(
        &self,
        coordinator: &SessionCoordinator,
    ) -> Result<Vec<NewsSubscriptionRule>, InfoSessionError> {
        let body = self
            .read_personal_news(coordinator, PersonalNewsRequest::Rules)
            .await?;
        parse_news_subscriptions(&body).map_err(map_news_detail_error)
    }

    /// One read-only page for a rule ID previously read from this session's
    /// subscription list. Runtime owns the opaque selector/ID binding.
    pub async fn fetch_news_by_subscription(
        &self,
        coordinator: &SessionCoordinator,
        page: u32,
        rule_id: &str,
    ) -> Result<NewsPage, InfoSessionError> {
        let body = self
            .read_personal_news(
                coordinator,
                PersonalNewsRequest::Subscription { page, rule_id },
            )
            .await?;
        parse_news_subscription_page(&body).map_err(map_news_detail_error)
    }

    /// Reads every favorite page under one proven INFO session. A changing
    /// page count, duplicate ID, missing page, or upper bound is an error;
    /// none of those conditions can become a successful partial list.
    pub async fn fetch_all_favorite_news(
        &self,
        coordinator: &SessionCoordinator,
    ) -> Result<Vec<NewsItem>, InfoSessionError> {
        let first = self
            .read_personal_news(coordinator, PersonalNewsRequest::Favorites(1))
            .await?;
        let first = parse_news_favorite_page(&first).map_err(map_news_detail_error)?;
        let total_pages = first.total_pages;
        let mut items = first.items;
        let mut ids = items
            .iter()
            .map(|item| item.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if ids.len() != items.len() || items.len() > 5_000 {
            return Err(InfoSessionError::News(
                NewsParseError::CatalogMalformedPayload { kind: "favorites" },
            ));
        }
        for page in 2..=total_pages {
            let body = self
                .read_personal_news(coordinator, PersonalNewsRequest::Favorites(page))
                .await?;
            let next = parse_news_favorite_page(&body).map_err(map_news_detail_error)?;
            if next.total_pages != total_pages || next.items.is_empty() {
                return Err(InfoSessionError::News(
                    NewsParseError::CatalogMalformedPayload { kind: "favorites" },
                ));
            }
            for item in next.items {
                if !ids.insert(item.id.clone()) || items.len() >= 5_000 {
                    return Err(InfoSessionError::News(
                        NewsParseError::CatalogMalformedPayload { kind: "favorites" },
                    ));
                }
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Only three pinned reference routes are admitted. All share the normal
    /// INFO CSRF bootstrap, cookie jar and CampusHttpTransport gate.
    async fn read_personal_news(
        &self,
        coordinator: &SessionCoordinator,
        operation: PersonalNewsRequest<'_>,
    ) -> Result<String, InfoSessionError> {
        let (path, stage, form) = match operation {
            PersonalNewsRequest::Rules => (NEWS_SUBSCRIPTIONS_PATH, "info_subscriptions", None),
            PersonalNewsRequest::Favorites(page)
                if (1..=crate::info_news::MAX_FAVORITE_PAGES).contains(&page) =>
            {
                (
                    NEWS_FAVORITES_PATH,
                    "info_favorites",
                    Some(vec![("currentPage", page.to_string())]),
                )
            }
            PersonalNewsRequest::Subscription { page, rule_id }
                if (1..=100).contains(&page)
                    && !rule_id.is_empty()
                    && rule_id.len() <= 128
                    && !rule_id.chars().any(|character| {
                        character.is_control()
                            || character.is_whitespace()
                            || matches!(character, '/' | '\\' | '?' | '#' | '&' | '=')
                    }) =>
            {
                (
                    NEWS_SUBSCRIPTION_FEED_PATH,
                    "info_subscription_feed",
                    Some(vec![
                        ("currentPage", page.to_string()),
                        ("dyid", rule_id.to_owned()),
                    ]),
                )
            }
            _ => {
                return Err(InfoSessionError::InvalidConfig(
                    "INFO personal news selector is invalid".to_owned(),
                ));
            }
        };
        let csrf = self.news_csrf(coordinator).await?;
        let endpoint = self.config.target_url(path)?;
        let expected_path = endpoint.path().to_owned();
        let query = [("_csrf".to_owned(), csrf.as_str().to_owned())];
        let request = match form {
            Some(form) => self
                .transport
                .client()
                .post(endpoint)
                .query(&query)
                .form(&form)
                .build()
                .map_err(TransportError::Request)?,
            None => self
                .transport
                .client()
                .get(endpoint)
                .query(&query)
                .build()
                .map_err(TransportError::Request)?,
        };
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        tracing::info!(target: "tsinghua_kit::validation", event = "info_personal_response",
            business_stage = stage, http_status = status.as_u16(), redirect_present = location.is_some());
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || location.as_deref().is_some_and(|value| {
                resolve_location(&final_url, value)
                    .is_ok_and(|target| self.config.is_identity_login_target(&target))
            })
            || is_authentication_status(status)
            || is_login_page(&final_url, content_type.as_deref(), &body)
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if location.is_some()
            || !status.is_success()
            || final_url.path() != expected_path
            || !path_has_prefix(final_url.path(), &self.config.target_prefix)
            || !query_matches(&query, &final_url)
        {
            return Err(if !status.is_success() && location.is_none() {
                InfoSessionError::NewsHttpStatus { status }
            } else {
                InfoSessionError::NewsUnexpectedPath
            });
        }
        Ok(body)
    }

    async fn read_news_catalog(
        &self,
        coordinator: &SessionCoordinator,
        path: &'static str,
        include_empty_channel: bool,
    ) -> Result<String, InfoSessionError> {
        let csrf = self.news_csrf(coordinator).await?;
        let endpoint = self.config.target_url(path)?;
        let expected_path = endpoint.path().to_owned();
        let mut query = Vec::with_capacity(2);
        if include_empty_channel {
            query.push(("lmid".to_owned(), String::new()));
        }
        query.push(("_csrf".to_owned(), csrf.as_str().to_owned()));
        let request = self
            .transport
            .client()
            .get(endpoint)
            .query(&query)
            .build()
            .map_err(TransportError::Request)?;
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        let business_stage = if path == NEWS_SOURCES_PATH {
            "info_catalog_sources"
        } else {
            "info_catalog_channels"
        };
        tracing::info!(target: "tsinghua_kit::validation", event = "info_catalog_response",
            business_stage, http_status = status.as_u16(), redirect_present = location.is_some());
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || location.as_deref().is_some_and(|value| {
                resolve_location(&final_url, value)
                    .is_ok_and(|target| self.config.is_identity_login_target(&target))
            })
            || is_authentication_status(status)
            || is_login_page(&final_url, content_type.as_deref(), &body)
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if location.is_some()
            || !status.is_success()
            || final_url.path() != expected_path
            || !path_has_prefix(final_url.path(), &self.config.target_prefix)
            || !query_matches(&query, &final_url)
        {
            return Err(if !status.is_success() && location.is_none() {
                InfoSessionError::NewsHttpStatus { status }
            } else {
                InfoSessionError::NewsUnexpectedPath
            });
        }
        Ok(body)
    }

    /// Fetches and parses one INFO article through the same authenticated
    /// WebVPN session used by list and search.
    pub async fn fetch_news_detail(
        &self,
        coordinator: &SessionCoordinator,
        article_id: &str,
    ) -> Result<NewsDetail, InfoSessionError> {
        let article_id = article_id.trim();
        if article_id.is_empty() {
            return Err(InfoSessionError::InvalidConfig(
                "INFO article identifier or link must not be empty".to_owned(),
            ));
        }

        // The current INFO API accepts xxid directly, while the reference
        // client receives a list link and first opens it to recover the
        // page's real xxid. Keep both forms at this boundary because the
        // existing runtime action exposes one String and cannot change its
        // bridge signature here. Legacy HTML and PDF links complete inside
        // resolve_news_link and therefore do not fall through to a fake JSON
        // success.
        let resolved = if looks_like_news_link(article_id) {
            self.resolve_news_link(coordinator, article_id).await?
        } else {
            NewsLinkResolution::ApiArticleId(article_id.to_owned())
        };
        match resolved {
            NewsLinkResolution::CompleteDetail(detail) => Ok(detail),
            NewsLinkResolution::ApiArticleId(resolved_id) => {
                let plan = self
                    .config
                    .news
                    .detail_request(&resolved_id)
                    .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
                let mut detail = self.execute_news_detail(coordinator, plan).await?;
                if detail.id.is_empty() {
                    detail.id = resolved_id;
                } else if detail.id != resolved_id {
                    // A valid article envelope is not evidence that the
                    // requested article was returned. Missing IDs may be
                    // filled from the request, but conflicting IDs must not
                    // silently substitute another article or poison caches.
                    return Err(InfoSessionError::News(
                        NewsParseError::DetailMalformedPayload {
                            field: "object.xxDto.xxid".to_owned(),
                            reason: "article identifier does not match the request".to_owned(),
                        },
                    ));
                }
                Ok(detail)
            }
        }
    }

    async fn resolve_news_link(
        &self,
        coordinator: &SessionCoordinator,
        link: &str,
    ) -> Result<NewsLinkResolution, InfoSessionError> {
        // Refresh and prove the same INFO session before opening the article
        // page. The article navigation itself intentionally carries no CSRF
        // query parameter; the subsequent JSON detail request does.
        self.news_csrf(coordinator).await?;
        let (endpoint, _initial_mapping, _) = self.news_link_target(link)?;
        let response = self.get_news_document(endpoint).await?;
        let mapping_prefix = self.approved_news_mapping(&response.final_url)?;
        let publication_id = safe_publication_fragment(response.navigation_fragment.as_deref())?;
        self.validate_news_document_response(&response, &mapping_prefix, None)?;

        if is_pdf_response(&response.final_url, response.content_type.as_deref()) {
            return parse_news_pdf(&response.body)
                .map(NewsLinkResolution::CompleteDetail)
                .map_err(map_news_detail_error);
        }

        let body = document_text(&response)?;
        if let Some(publication_id) = publication_id {
            return self
                .fetch_system_pdf(coordinator, &mapping_prefix, &publication_id)
                .await
                .map(NewsLinkResolution::CompleteDetail);
        }
        if let Some(xxid) = extract_news_xxid(&body) {
            return Ok(NewsLinkResolution::ApiArticleId(xxid));
        }

        if contains_play_file_marker(&body) {
            return self
                .resolve_play_file(coordinator, response.final_url.clone(), &body)
                .await;
        }

        crate::info_news::parse_news_legacy_detail_for_url(&body, response.final_url.as_str())
            .map(NewsLinkResolution::CompleteDetail)
            .map_err(map_news_detail_error)
    }

    fn news_link_url(&self, link: &str) -> Result<Url, InfoSessionError> {
        self.news_link_target(link).map(|(url, _, _)| url)
    }

    fn news_link_target(
        &self,
        link: &str,
    ) -> Result<(Url, String, Option<String>), InfoSessionError> {
        if link.chars().any(char::is_control) {
            return Err(InfoSessionError::InvalidConfig(
                "INFO article link contains a control character".to_owned(),
            ));
        }

        let parsed = Url::parse(link).ok();
        let (path, query, mapping_prefix, publication_id) = if let Some(url) = parsed {
            if !same_origin(&self.config.webvpn_base_url, &url) {
                return Err(InfoSessionError::NewsUnexpectedOrigin);
            }
            let publication_id = safe_publication_fragment(url.fragment())?;
            let path = url.path().to_owned();
            let mapping_prefix = self.mapping_prefix_for_absolute_path(&path)?;
            (
                path,
                url.query().map(str::to_owned),
                mapping_prefix,
                publication_id,
            )
        } else {
            if !link.starts_with('/') || link.starts_with("//") {
                return Err(InfoSessionError::InvalidConfig(
                    "INFO article link must be an absolute same-origin URL or a relative path"
                        .to_owned(),
                ));
            }
            let (without_fragment, fragment) = split_safe_publication_fragment(link)?;
            let (path, query) = without_fragment
                .split_once('?')
                .unwrap_or((without_fragment.as_str(), ""));
            if path.is_empty()
                || path.starts_with("//")
                || path.contains('#')
                || path.contains("..")
                || path.contains("://")
                || invalid_percent_encoding(path)
                || path_contains_encoded_escape(path)
                || invalid_percent_encoding(query)
            {
                return Err(InfoSessionError::InvalidConfig(
                    "INFO article link path is invalid".to_owned(),
                ));
            }
            let (mapping_prefix, target_path) = if path_has_prefix(path, &self.config.target_prefix)
            {
                (self.config.target_prefix.clone(), path.to_owned())
            } else if is_webvpn_mapped_path(path) {
                let Some(mapping_prefix) = mapped_path_prefix(path) else {
                    return Err(InfoSessionError::NewsUnexpectedPath);
                };
                if path == mapping_prefix {
                    return Err(InfoSessionError::NewsUnexpectedPath);
                }
                (mapping_prefix, path.to_owned())
            } else {
                (
                    self.config.target_prefix.clone(),
                    format!("{}{path}", self.config.target_prefix),
                )
            };
            (
                target_path,
                (!query.is_empty()).then(|| query.to_owned()),
                mapping_prefix,
                fragment,
            )
        };

        if path.is_empty()
            || path == mapping_prefix
            || path.contains(['\\', '?', '#'])
            || path.contains("..")
            || invalid_percent_encoding(&path)
            || path_contains_encoded_escape(&path)
            || !path_has_prefix(&path, &mapping_prefix)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        let mut endpoint = self.config.webvpn_base_url.clone();
        endpoint.set_path(&path);
        endpoint.set_query(query.as_deref());
        let fragment = publication_id.as_ref().map(|id| format!("/publish/{id}"));
        endpoint.set_fragment(fragment.as_deref());
        if !same_origin(&self.config.webvpn_base_url, &endpoint)
            || !path_has_prefix(endpoint.path(), &mapping_prefix)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        Ok((endpoint, mapping_prefix, publication_id))
    }

    fn mapping_prefix_for_absolute_path(&self, path: &str) -> Result<String, InfoSessionError> {
        if path_has_prefix(path, &self.config.target_prefix) {
            return Ok(self.config.target_prefix.clone());
        }
        if is_webvpn_mapped_path(path) {
            return mapped_path_prefix(path).ok_or(InfoSessionError::NewsUnexpectedPath);
        }
        Err(InfoSessionError::NewsUnexpectedPath)
    }

    async fn resolve_play_file(
        &self,
        coordinator: &SessionCoordinator,
        final_endpoint: Url,
        initial_body: &str,
    ) -> Result<NewsLinkResolution, InfoSessionError> {
        if let Some(file_id) =
            extract_file_id_from_url(&final_endpoint).or_else(|| extract_play_file_id(initial_body))
        {
            return self
                .fetch_pdf_stream(coordinator, &file_id)
                .await
                .map(NewsLinkResolution::CompleteDetail);
        }
        // Some reference deployments reveal fileId only on a second GET.
        // Never revisit an original ticket-bearing entry URL; retain the
        // transport's existing consuming-navigation guard for this boundary.
        let request = self
            .transport
            .client()
            .get(final_endpoint.clone())
            .build()
            .map_err(TransportError::Request)?;
        if CampusHttpTransport::dispatch_requires_exclusivity(&request) {
            return Err(InfoSessionError::News(
                NewsParseError::DetailLinkMissingFileId,
            ));
        }
        let navigation = self.get_news_document(final_endpoint).await?;
        let mapping = self.approved_news_mapping(&navigation.final_url)?;
        self.validate_news_document_response(&navigation, &mapping, None)?;
        if is_pdf_response(&navigation.final_url, navigation.content_type.as_deref()) {
            return parse_news_pdf(&navigation.body)
                .map(NewsLinkResolution::CompleteDetail)
                .map_err(map_news_detail_error);
        }
        let body = document_text(&navigation)?;
        let file_id = extract_file_id_from_url(&navigation.final_url)
            .or_else(|| extract_play_file_id(body))
            .ok_or(InfoSessionError::News(
                NewsParseError::DetailLinkMissingFileId,
            ))?;
        self.fetch_pdf_stream(coordinator, &file_id)
            .await
            .map(NewsLinkResolution::CompleteDetail)
    }

    async fn fetch_pdf_stream(
        &self,
        coordinator: &SessionCoordinator,
        file_id: &str,
    ) -> Result<NewsDetail, InfoSessionError> {
        let mapping_prefix = &self.config.target_prefix;
        validate_detail_component(file_id, "file_id")?;
        let path = format!("{DEFAULT_PDF_STREAM_PATH}/{file_id}");
        let endpoint = self.mapped_target_url(mapping_prefix, &path)?;
        let csrf = self.news_csrf(coordinator).await?;
        let request = self
            .transport
            .client()
            .get(endpoint.clone())
            .query(&[("_csrf", csrf.as_str())])
            .build()
            .map_err(|error| {
                InfoSessionError::InvalidConfig(format!("INFO PDF request: {error}"))
            })?;
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(TransportError::Request)?;
        let response = self.read_news_response(response, true).await?;
        self.validate_news_document_response(&response, mapping_prefix, Some(endpoint.path()))?;
        parse_news_pdf(&response.body).map_err(map_news_detail_error)
    }

    async fn fetch_system_pdf(
        &self,
        coordinator: &SessionCoordinator,
        mapping_prefix: &str,
        publication_id: &str,
    ) -> Result<NewsDetail, InfoSessionError> {
        self.ensure_info_snapshot(coordinator)?;
        if !news_navigation::is_system_publication_mapping(mapping_prefix) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        validate_detail_component(publication_id, "publication_id")?;
        let path = format!("{DEFAULT_SYSTEM_PDF_PATH}/{publication_id}");
        let endpoint = self.mapped_target_url(mapping_prefix, &path)?;
        let request = self
            .transport
            .client()
            .get(endpoint.clone())
            .build()
            .map_err(|error| {
                InfoSessionError::InvalidConfig(format!("INFO system PDF request: {error}"))
            })?;
        let response = self
            .transport
            .execute_once(self.transport.client(), request)
            .await
            .map_err(TransportError::Request)?;
        let response = self.read_news_response(response, false).await?;
        self.validate_news_document_response(&response, mapping_prefix, Some(endpoint.path()))?;
        let body = document_text(&response)?;
        let mut detail = parse_news_system_pdf(&body).map_err(map_news_detail_error)?;
        if detail.id.is_empty() {
            detail.id = publication_id.to_owned();
        }
        Ok(detail)
    }

    fn mapped_target_url(&self, mapping_prefix: &str, path: &str) -> Result<Url, InfoSessionError> {
        validate_mapping_prefix(mapping_prefix)?;
        validate_relative_path("news_detail_path", path)?;
        let full_path = format!("{}{}", mapping_prefix.trim_end_matches('/'), path);
        let endpoint = self
            .config
            .webvpn_base_url
            .join(&full_path)
            .map_err(|error| InfoSessionError::InvalidConfig(error.to_string()))?;
        if !same_origin(&self.config.webvpn_base_url, &endpoint)
            || !path_has_prefix(endpoint.path(), mapping_prefix)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        Ok(endpoint)
    }

    async fn get_news_document(
        &self,
        endpoint: Url,
    ) -> Result<NewsDocumentResponse, InfoSessionError> {
        self.get_reference_news_document(endpoint).await
    }

    async fn read_news_response(
        &self,
        response: reqwest::Response,
        binary_hint: bool,
    ) -> Result<NewsDocumentResponse, InfoSessionError> {
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let binary_hint = binary_hint || is_pdf_response(&final_url, content_type.as_deref());
        let body = if binary_hint {
            crate::telemetry::timing::read_bytes(response)
                .await
                .map_err(TransportError::Decode)?
                .to_vec()
        } else {
            crate::telemetry::timing::read_text(response)
                .await
                .map_err(TransportError::Decode)?
                .into_bytes()
        };
        Ok(NewsDocumentResponse {
            navigation_fragment: None,
            status,
            final_url,
            content_type,
            redirect_location,
            body,
        })
    }

    fn validate_news_document_response(
        &self,
        response: &NewsDocumentResponse,
        mapping_prefix: &str,
        expected_path: Option<&str>,
    ) -> Result<(), InfoSessionError> {
        let redirect_target = response
            .redirect_location
            .as_deref()
            .filter(|_| matches!(response.status.as_u16(), 301 | 302 | 303 | 307 | 308))
            .map(|location| resolve_location(&response.final_url, location))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        if !same_origin(&self.config.webvpn_base_url, &response.final_url) {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if self.news_login_target(&response.final_url)
            || redirect_target
                .as_ref()
                .is_some_and(|target| self.news_login_target(target))
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if redirect_target
            .as_ref()
            .is_some_and(|target| !same_origin(&self.config.webvpn_base_url, target))
        {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if is_authentication_status(response.status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if !response.status.is_success() {
            return Err(InfoSessionError::NewsHttpStatus {
                status: response.status,
            });
        }
        if !safe_news_mapping_path(response.final_url.path(), mapping_prefix)
            || expected_path.is_some_and(|path| response.final_url.path() != path)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if redirect_target
            .as_ref()
            .is_some_and(|target| !safe_news_mapping_path(target.path(), mapping_prefix))
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if let Ok(body) = std::str::from_utf8(&response.body) {
            if is_login_page(&response.final_url, response.content_type.as_deref(), body) {
                return Err(InfoSessionError::LoginRequired);
            }
        }
        Ok(())
    }

    async fn execute_news(
        &self,
        coordinator: &SessionCoordinator,
        plan: NewsRequestPlan,
        feed: NewsFeedKind,
    ) -> Result<NewsPage, InfoSessionError> {
        let csrf = self.news_csrf(coordinator).await?;
        let endpoint = self.config.target_url(plan.path())?;
        let expected_path = endpoint.path().to_owned();
        let mut query = plan.query_parameters().to_vec();
        let mut form = plan.form_parameters().to_vec();
        match plan.csrf_requirement().placement {
            crate::info_news::NewsParameterPlacement::Query => {
                query.push((
                    plan.csrf_requirement().field.clone(),
                    csrf.as_str().to_owned(),
                ));
            }
            crate::info_news::NewsParameterPlacement::Form => {
                form.push((
                    plan.csrf_requirement().field.clone(),
                    csrf.as_str().to_owned(),
                ));
            }
        }
        let method = match plan.method() {
            crate::info_news::NewsHttpMethod::Get => Method::GET,
            crate::info_news::NewsHttpMethod::Post => Method::POST,
        };
        let builder = self.transport.client().request(method.clone(), endpoint);
        let builder = if method == Method::GET {
            builder.query(&query)
        } else {
            builder.query(&query).form(&form)
        };
        let request = builder.build().map_err(|error| {
            InfoSessionError::InvalidConfig(format!("INFO news request: {error}"))
        })?;
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        let redirect_target = redirect_location
            .as_deref()
            .map(|location| resolve_location(&final_url, location))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        if redirect_target
            .as_ref()
            .is_some_and(|target| !same_origin(&self.config.webvpn_base_url, target))
        {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || redirect_target
                .as_ref()
                .is_some_and(|target| self.config.is_identity_login_target(target))
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if is_authentication_status(status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if redirect_target
            .as_ref()
            .is_some_and(|target| target.path() != expected_path)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if !status.is_success() {
            return Err(InfoSessionError::NewsHttpStatus { status });
        }
        if is_login_page(&final_url, content_type.as_deref(), &body) {
            return Err(InfoSessionError::LoginRequired);
        }
        if final_url.path() != expected_path || !query_matches(&query, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if !path_has_prefix(final_url.path(), &self.config.target_prefix) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        parse_news_page(&body, feed)
            .map(|outcome| outcome.into_page())
            .map_err(|error| match error {
                NewsParseError::HtmlLoginPage | NewsParseError::LoginRequired => {
                    InfoSessionError::LoginRequired
                }
                error => InfoSessionError::News(error),
            })
    }

    async fn execute_news_detail(
        &self,
        coordinator: &SessionCoordinator,
        plan: NewsRequestPlan,
    ) -> Result<NewsDetail, InfoSessionError> {
        let csrf = self.news_csrf(coordinator).await?;
        let endpoint = self.config.target_url(plan.path())?;
        let expected_path = endpoint.path().to_owned();
        let mut query = plan.query_parameters().to_vec();
        let mut form = plan.form_parameters().to_vec();
        match plan.csrf_requirement().placement {
            crate::info_news::NewsParameterPlacement::Query => query.push((
                plan.csrf_requirement().field.clone(),
                csrf.as_str().to_owned(),
            )),
            crate::info_news::NewsParameterPlacement::Form => form.push((
                plan.csrf_requirement().field.clone(),
                csrf.as_str().to_owned(),
            )),
        }
        let method = match plan.method() {
            crate::info_news::NewsHttpMethod::Get => Method::GET,
            crate::info_news::NewsHttpMethod::Post => Method::POST,
        };
        let builder = self.transport.client().request(method.clone(), endpoint);
        let builder = if method == Method::GET {
            builder.query(&query)
        } else {
            builder.query(&query).form(&form)
        };
        let request = builder.build().map_err(|error| {
            InfoSessionError::InvalidConfig(format!("INFO news detail request: {error}"))
        })?;
        let response = self
            .transport
            .execute(request)
            .await
            .map_err(TransportError::Request)?;
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(TransportError::Decode)?;
        let redirect_target = redirect_location
            .as_deref()
            .map(|location| resolve_location(&final_url, location))
            .transpose()
            .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
        if redirect_target
            .as_ref()
            .is_some_and(|target| !same_origin(&self.config.webvpn_base_url, target))
        {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if !same_origin(&self.config.webvpn_base_url, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedOrigin);
        }
        if self.config.is_identity_login_target(&final_url)
            || redirect_target
                .as_ref()
                .is_some_and(|target| self.config.is_identity_login_target(target))
        {
            return Err(InfoSessionError::LoginRequired);
        }
        if is_authentication_status(status) {
            return Err(InfoSessionError::LoginRequired);
        }
        if redirect_target
            .as_ref()
            .is_some_and(|target| target.path() != expected_path)
        {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if !status.is_success() {
            return Err(InfoSessionError::NewsHttpStatus { status });
        }
        if is_login_page(&final_url, content_type.as_deref(), &body) {
            return Err(InfoSessionError::LoginRequired);
        }
        if final_url.path() != expected_path || !query_matches(&query, &final_url) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        if !path_has_prefix(final_url.path(), &self.config.target_prefix) {
            return Err(InfoSessionError::NewsUnexpectedPath);
        }
        parse_news_detail(&body).map_err(|error| match error {
            NewsParseError::HtmlLoginPage | NewsParseError::LoginRequired => {
                InfoSessionError::LoginRequired
            }
            error => InfoSessionError::News(error),
        })
    }

    fn ensure_info_snapshot(
        &self,
        coordinator: &SessionCoordinator,
    ) -> Result<(), InfoSessionError> {
        let info = coordinator.registry().snapshot_for(ServiceId::Info);
        let identity = coordinator.registry().snapshot_for(ServiceId::Identity);
        if info.state != ServiceSessionState::Authenticated
            || identity.state != ServiceSessionState::Authenticated
        {
            return Err(InfoSessionError::IdentityNotAuthenticated);
        }
        let owner = identity
            .user
            .as_ref()
            .ok_or(InfoSessionError::IdentityUserMissing)?;
        if info.user.as_ref().map(|user| &user.username) != Some(&owner.username) {
            return Err(InfoSessionError::IdentityUserMismatch);
        }
        if coordinator.bound_csrf(ServiceId::Info).is_none() {
            return Err(InfoSessionError::SessionProofMissing);
        }
        Ok(())
    }

    async fn news_csrf(
        &self,
        coordinator: &SessionCoordinator,
    ) -> Result<CsrfToken, InfoSessionError> {
        self.ensure_info_snapshot(coordinator)?;

        // The reference client performs the WebVPN cookie exchange before
        // each INFO operation.  Repeating it here keeps the request's `_csrf`
        // value aligned with the current XSRF-TOKEN in the same Cookie jar,
        // including after another application handoff has refreshed it.
        self.bootstrap_cookie().await
    }
}

fn validate_origin(url: &Url) -> Result<(), InfoSessionError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(InfoSessionError::InvalidConfig(
            "WebVPN base URL must be an origin without userinfo, query, or fragment".to_owned(),
        ));
    }
    Ok(())
}

fn is_authentication_status(status: StatusCode) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
}

fn map_info_handoff_error(error: InfoClientError) -> InfoSessionError {
    match error {
        InfoClientError::HtmlPage {
            classification: InfoHtmlClassification::LoginRequired,
            ..
        }
        | InfoClientError::Profile(InfoError::LoginRequired)
        | InfoClientError::HttpStatus {
            status: StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN,
            ..
        } => InfoSessionError::LoginRequired,
        error => InfoSessionError::Client(error),
    }
}

fn normalize_target_prefix(prefix: &str) -> Result<String, InfoSessionError> {
    if prefix.trim().is_empty()
        || !prefix.starts_with('/')
        || prefix.starts_with("//")
        || prefix.contains(['?', '#'])
        || prefix.contains("..")
        || invalid_percent_encoding(prefix)
        || path_contains_encoded_escape(prefix)
        || prefix.chars().any(char::is_control)
    {
        return Err(InfoSessionError::InvalidConfig(
            "INFO target prefix must be a safe relative path".to_owned(),
        ));
    }
    let normalized = prefix.trim_end_matches('/');
    if normalized.is_empty() {
        return Err(InfoSessionError::InvalidConfig(
            "INFO target prefix must not be the WebVPN root".to_owned(),
        ));
    }
    Ok(normalized.to_owned())
}

fn append_target_path(prefix: &str, path: &str) -> Result<String, InfoSessionError> {
    validate_relative_path("target_path", path)?;
    Ok(format!("{}{}", prefix.trim_end_matches('/'), path))
}

fn validate_relative_path(field: &str, path: &str) -> Result<(), InfoSessionError> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['?', '#'])
        || path.contains("..")
        || path.contains("://")
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
        || path.chars().any(char::is_control)
    {
        return Err(InfoSessionError::InvalidConfig(format!(
            "{field} must be a safe relative path"
        )));
    }
    Ok(())
}

fn validate_wire_value(field: &str, value: &str, required: bool) -> Result<(), InfoSessionError> {
    if (required && value.trim().is_empty()) || value.chars().any(char::is_control) {
        return Err(InfoSessionError::InvalidConfig(format!(
            "{field} must be a valid wire value"
        )));
    }
    Ok(())
}

fn same_origin(base: &Url, candidate: &Url) -> bool {
    base.scheme() == candidate.scheme()
        && base.host_str() == candidate.host_str()
        && base.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn redirect_leaves_origin(base: &Url, response_url: &Url, location: &str) -> bool {
    let Ok(candidate) = resolve_location(response_url, location) else {
        return true;
    };
    !same_origin(base, &candidate)
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, InfoSessionError> {
    if invalid_percent_encoding(location) {
        return Err(InfoSessionError::UnexpectedOrigin);
    }
    Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| InfoSessionError::UnexpectedOrigin)
}

fn invalid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
    })
}

fn path_contains_encoded_escape(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.windows(3).any(|window| {
        window[0] == b'%'
            && hex_value(window[1])
                .zip(hex_value(window[2]))
                .is_some_and(|(high, low)| {
                    matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                })
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn mapped_path_prefix(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let first = segments.next()?;
    let required_segments = if matches!(first, "https" | "http" | "http-80" | "https-443") {
        // WebVPN uses both HTTPS and HTTP target mappings. In either form the
        // opaque deployment token is the second path segment; treating
        // `/http` as the whole mapping would allow a handoff to escape into a
        // different HTTP mapping on the same WebVPN origin.
        2
    } else {
        1
    };
    let mut prefix = vec![first];
    while prefix.len() < required_segments {
        prefix.push(segments.next()?);
    }
    (prefix.len() == required_segments).then(|| format!("/{}", prefix.join("/")))
}

fn validate_additional_roaming_url(
    base: &Url,
    candidate: &Url,
    mapping_prefix: &str,
) -> Result<(), InfoSessionError> {
    if !same_origin(base, candidate) {
        return Err(InfoSessionError::UnexpectedOrigin);
    }
    if candidate.fragment().is_some()
        || candidate.path().is_empty()
        || candidate.path() == "/"
        || candidate.path().chars().any(char::is_control)
        || candidate.path().contains(['\\', '?', '#'])
        || candidate.path().contains("..")
        || invalid_percent_encoding(candidate.path())
        || path_contains_encoded_escape(candidate.path())
        || !path_has_prefix(candidate.path(), mapping_prefix)
    {
        return Err(InfoSessionError::HandoffUnexpectedPath);
    }
    if candidate.path() == mapping_prefix {
        return Err(InfoSessionError::HandoffUnexpectedPath);
    }

    let lower_path = candidate.path().to_ascii_lowercase();
    if lower_path == "/wengine-vpn" || lower_path.starts_with("/wengine-vpn/") {
        return Err(InfoSessionError::HandoffUnexpectedPath);
    }
    if is_identity_login_path(&lower_path) {
        return Err(InfoSessionError::LoginRequired);
    }
    Ok(())
}

fn is_identity_login_path(path: &str) -> bool {
    path == "/login"
        || path.ends_with("/login")
        || path.contains("/do/off/ui/auth/login")
        || path.contains("/auth/login")
        || path.contains("/sso/login")
}

fn is_identity_login_target(url: &Url) -> bool {
    is_identity_login_path(&url.path().to_ascii_lowercase())
}

fn portal_resource_redirect_route_class(config: &InfoWebVpnConfig, url: &Url) -> &'static str {
    if same_origin(&config.webvpn_base_url, url) {
        let path = url.path();
        if path == "/" {
            "webvpn_home"
        } else if matches!(path, "/login" | "/f/login") {
            "webvpn_login"
        } else if crate::portal_resume::mapped_identity_login_path(url) {
            "webvpn_identity_route"
        } else if path == config.target_prefix.trim_end_matches('/') {
            "webvpn_info_root"
        } else if path.starts_with(&config.target_prefix) {
            "webvpn_info_resource"
        } else if path.starts_with("/https/") {
            "webvpn_other_mapping"
        } else if path.starts_with("/http/") || path.starts_with("/http-") {
            "webvpn_other_protocol"
        } else {
            "webvpn_other_route"
        }
    } else if same_origin(&config.identity_origin, url) {
        let path = url.path();
        if path.starts_with("/do/off/ui/auth/login/form/") {
            "identity_form"
        } else if path == "/do/off/ui/auth/login/redirect2Jsp" {
            "identity_callback"
        } else {
            "identity_other_route"
        }
    } else if url.host_str() == Some("info.tsinghua.edu.cn") {
        "info_direct"
    } else {
        "foreign_origin"
    }
}

fn validate_portal_account(body: &str, expected: &str) -> Result<(), InfoSessionError> {
    if body.len() > 1024 * 1024 || expected.trim().is_empty() {
        return Err(InfoSessionError::PortalAccountResponse);
    }
    let value: serde_json::Value = serde_json::from_str(body.trim_start_matches('\u{feff}'))
        .map_err(|_| InfoSessionError::PortalAccountResponse)?;
    let envelope = value
        .as_object()
        .ok_or(InfoSessionError::PortalAccountResponse)?;
    if envelope
        .get("success")
        .is_some_and(|v| v != &serde_json::Value::Bool(true))
        || envelope
            .get("result")
            .is_some_and(|v| v.as_str() != Some("success"))
    {
        return Err(InfoSessionError::PortalAccountResponse);
    }
    let actual = envelope
        .get("object")
        .and_then(|v| v.get("ryh"))
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .ok_or(InfoSessionError::PortalAccountResponse)?;
    if actual != expected {
        return Err(InfoSessionError::PortalAccountMismatch);
    }
    Ok(())
}

fn extract_cookie_token<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    body.split([';', ',']).find_map(|segment| {
        let (key, value) = segment.trim().split_once('=')?;
        if key.trim() != name {
            return None;
        }
        let value = value.trim();
        (!value.is_empty()
            && !value.chars().any(|character| {
                character.is_control() || character.is_whitespace() || character == '"'
            }))
        .then_some(value)
    })
}

fn path_has_prefix(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn query_matches(expected: &[(String, String)], candidate: &Url) -> bool {
    let mut expected = expected.to_vec();
    let mut actual = candidate
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    expected.sort_unstable();
    actual.sort_unstable();
    expected == actual
}

#[cfg(test)]
#[path = "info_session_personal_tests.rs"]
mod personal_tests;

fn looks_like_news_link(value: &str) -> bool {
    value.starts_with('/') || value.contains("://")
}

fn extract_news_xxid(body: &str) -> Option<String> {
    // The observed article page exposes `var xxid = "...";`. Search only
    // executable script blocks and skip JavaScript comments/strings before
    // looking for that exact statement. A phrase in ordinary article text,
    // an HTML comment, or a script string is not a service proof.
    let lowercase = body.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(relative_start) = lowercase[search_from..].find("<script") {
        let script_start = search_from + relative_start;
        let tag_end = script_start + "<script".len();
        if lowercase
            .as_bytes()
            .get(tag_end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            search_from = tag_end;
            continue;
        }
        let content_start = lowercase[tag_end..].find('>')? + tag_end + 1;
        let content_end = lowercase[content_start..].find("</script>")? + content_start;
        if let Some(xxid) = extract_xxid_statement(&body[content_start..content_end]) {
            return Some(xxid);
        }
        search_from = content_end + "</script>".len();
    }
    None
}

fn extract_xxid_statement(script: &str) -> Option<String> {
    const MARKER: &[u8] = b"var xxid";
    let bytes = script.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let Some(relative_end) = bytes[index + 2..]
                .windows(2)
                .position(|window| window == b"*/")
            else {
                return None;
            };
            index += 2 + relative_end + 2;
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            index = skip_javascript_string(bytes, index)?;
            continue;
        }

        let previous_is_identifier =
            index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_');
        if !previous_is_identifier && bytes[index..].starts_with(MARKER) {
            let mut cursor = index + MARKER.len();
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b'=') {
                index += 1;
                continue;
            }
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            let quote = *bytes.get(cursor)?;
            if !matches!(quote, b'\'' | b'"') {
                index += 1;
                continue;
            }
            cursor += 1;
            let value_start = cursor;
            while let Some(byte) = bytes.get(cursor) {
                if *byte == b'\\' {
                    // IDs are emitted as a literal value by the page. Escape
                    // sequences are rejected instead of being partially
                    // interpreted by this small boundary parser.
                    return None;
                }
                if *byte == quote {
                    break;
                }
                cursor += 1;
            }
            let value_end = cursor;
            if bytes.get(cursor) != Some(&quote) {
                return None;
            }
            let value = script.get(value_start..value_end)?;
            if value.is_empty()
                || value.chars().any(|character| {
                    character.is_control()
                        || character.is_whitespace()
                        || matches!(character, '\'' | '"' | '\\')
                })
            {
                return None;
            }
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b';') {
                return Some(value.to_owned());
            }
        }
        index += 1;
    }
    None
}

fn skip_javascript_string(bytes: &[u8], start: usize) -> Option<usize> {
    let quote = *bytes.get(start)?;
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == quote {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn looks_like_html_document(content_type: Option<&str>, body: &str) -> bool {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body).trim_start();
    if matches!(body.as_bytes().first(), Some(b'{' | b'[' | b'"')) {
        return false;
    }
    let content_type_is_html = content_type.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.starts_with("text/html") || value.starts_with("application/xhtml+xml")
    });
    content_type_is_html
        || body.starts_with("<!doctype html")
        || body.starts_with("<html")
        || body.starts_with("<head")
        || body.starts_with("<body")
        || body.contains("<html")
}

fn is_login_page(url: &Url, content_type: Option<&str>, body: &str) -> bool {
    let url_signal = url.path().to_ascii_lowercase().contains("/login");
    let lower_body = body
        .strip_prefix('\u{feff}')
        .unwrap_or(body)
        .trim_start()
        .to_ascii_lowercase();
    let html_signal = content_type.is_some_and(|value| {
        let value = value.to_ascii_lowercase();
        value.starts_with("text/html") || value.starts_with("application/xhtml+xml")
    }) || lower_body.starts_with("<!doctype html")
        || lower_body.starts_with("<html")
        || lower_body.contains("<form")
        || lower_body.contains("<input");
    let identity_form = (lower_body.contains("name=\"i_user\"")
        || lower_body.contains("name='i_user'"))
        && (lower_body.contains("name=\"i_pass\"") || lower_body.contains("name='i_pass'"));
    let marker = LOGIN_MARKERS
        .iter()
        .any(|marker| lower_body.contains(&marker.to_ascii_lowercase()));
    (html_signal && (url_signal || identity_form || marker))
        || (url_signal && (identity_form || marker))
}

/// Maps all detail-document parser failures through the INFO session boundary.
/// A response that is itself an identity login page must remain a session
/// failure, while malformed article data stays visible as a parser failure to
/// the caller. In particular, neither case is converted into an empty detail.
fn map_news_detail_error(error: NewsParseError) -> InfoSessionError {
    match error {
        NewsParseError::HtmlLoginPage | NewsParseError::LoginRequired => {
            InfoSessionError::LoginRequired
        }
        error => InfoSessionError::News(error),
    }
}

/// Returns whether a response should be consumed as a PDF document. The
/// content type is authoritative when it explicitly says PDF; the URL suffix
/// is a fallback for WebVPN file routes which omit or generalise the type.
fn is_pdf_response(url: &Url, content_type: Option<&str>) -> bool {
    let media_type = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        media_type.as_str(),
        "application/pdf" | "application/x-pdf" | "application/acrobat"
    ) || url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .is_some_and(|segment| segment.to_ascii_lowercase().ends_with(".pdf"))
}

/// Converts a non-PDF document response to text without lossy replacement.
/// The document parsers need to see the actual bytes so a binary error page
/// cannot be mistaken for a legacy article.
fn document_text(response: &NewsDocumentResponse) -> Result<&str, InfoSessionError> {
    std::str::from_utf8(&response.body).map_err(|_| {
        InfoSessionError::News(NewsParseError::DetailMalformedPayload {
            field: "document".to_owned(),
            reason: "document is not valid UTF-8".to_owned(),
        })
    })
}

fn contains_play_file_marker(body: &str) -> bool {
    body.to_ascii_lowercase().contains("_playfile")
}

/// Extracts a file id from a URL query without accepting a path or arbitrary
/// parameter as an id. `query_pairs` also performs the single URL-decoding
/// pass used by the browser clients.
fn extract_file_id_from_url(url: &Url) -> Option<String> {
    url.query_pairs()
        .find(|(name, _)| name.eq_ignore_ascii_case("fileId"))
        .and_then(|(_, value)| {
            let value = value.into_owned();
            validate_detail_component(&value, "file_id")
                .ok()
                .map(|_| value)
        })
}

/// Extracts the `fileId=` value from the small redirect script used by older
/// INFO news entries. This parser only accepts a scalar URL parameter and
/// stops at URL/script delimiters; it never executes JavaScript.
fn extract_play_file_id(body: &str) -> Option<String> {
    for marker in ["fileId=", "fileid="] {
        let mut search_from = 0;
        while let Some(relative) = body[search_from..].find(marker) {
            let start = search_from + relative + marker.len();
            let end = body[start..]
                .char_indices()
                .find(|(_, character)| {
                    matches!(
                        *character,
                        '&' | '#' | '?' | '"' | '\'' | '<' | '>' | '\r' | '\n' | '\t' | ' '
                    )
                })
                .map(|(index, _)| start + index)
                .unwrap_or(body.len());
            let value = body[start..end].trim();
            if validate_detail_component(value, "file_id").is_ok() {
                return Some(value.to_owned());
            }
            search_from = start.saturating_add(1);
            if search_from >= body.len() {
                break;
            }
        }
    }
    None
}

fn validate_detail_component(value: &str, field: &str) -> Result<(), InfoSessionError> {
    if value.trim().is_empty()
        || value == "."
        || value == ".."
        || value.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '/' | '\\' | '?' | '#' | '&' | '=')
        })
        || invalid_percent_encoding(value)
        || path_contains_encoded_escape(value)
    {
        return Err(InfoSessionError::InvalidConfig(format!(
            "INFO {field} is not a safe path component"
        )));
    }
    Ok(())
}

/// THUInfo uses #/publish/<id>. Retain the historical <id>/publish spelling
/// as well, but never treat arbitrary hash routes or empty ids as publications.
fn safe_publication_fragment(fragment: Option<&str>) -> Result<Option<String>, InfoSessionError> {
    let Some(fragment) = fragment else {
        return Ok(None);
    };
    if fragment.is_empty()
        || fragment
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(InfoSessionError::NewsUnexpectedPath);
    }
    let marker = fragment.strip_prefix('/').unwrap_or(fragment);
    let id = marker
        .strip_prefix("publish/")
        .or_else(|| marker.strip_suffix("/publish"))
        .ok_or(InfoSessionError::NewsUnexpectedPath)?;
    // Publication IDs are opaque scalar components, not encoded routes.
    if id.len() > 512 || id.contains('%') {
        return Err(InfoSessionError::NewsUnexpectedPath);
    }
    validate_detail_component(id, "publication_id")
        .map_err(|_| InfoSessionError::NewsUnexpectedPath)?;
    Ok(Some(id.to_owned()))
}

fn split_safe_publication_fragment(
    link: &str,
) -> Result<(String, Option<String>), InfoSessionError> {
    let Some((without_fragment, fragment)) = link.split_once('#') else {
        return Ok((link.to_owned(), None));
    };
    if without_fragment.is_empty() {
        return Err(InfoSessionError::NewsUnexpectedPath);
    }
    let publication_id =
        safe_publication_fragment(Some(fragment)).map_err(|error| match error {
            InfoSessionError::NewsUnexpectedPath => InfoSessionError::InvalidConfig(
                "INFO article link fragment is not a supported publication marker".to_owned(),
            ),
            error => error,
        })?;
    Ok((without_fragment.to_owned(), publication_id))
}

fn is_webvpn_mapped_path(path: &str) -> bool {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['\\', '?', '#'])
        || path.chars().any(char::is_control)
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
    {
        return false;
    }
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return false;
    };
    matches!(first, "http" | "https" | "http-80" | "https-443") && segments.next().is_some()
}

fn validate_mapping_prefix(prefix: &str) -> Result<(), InfoSessionError> {
    if prefix.trim().is_empty()
        || !prefix.starts_with('/')
        || prefix.starts_with("//")
        || prefix.ends_with('/')
        || prefix.contains(['\\', '?', '#'])
        || prefix.contains("..")
        || prefix.chars().any(char::is_control)
        || invalid_percent_encoding(prefix)
        || path_contains_encoded_escape(prefix)
        || prefix[1..].split('/').any(|segment| segment.is_empty())
    {
        return Err(InfoSessionError::NewsUnexpectedPath);
    }
    Ok(())
}

fn safe_news_mapping_path(path: &str, mapping_prefix: &str) -> bool {
    validate_mapping_prefix(mapping_prefix).is_ok()
        && path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains(['\\', '?', '#'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
        && path_has_prefix(path, mapping_prefix)
        && path != mapping_prefix
}

#[cfg(test)]
mod tests {

    #[test]
    fn backend_repair_followup_registrar_reference_calendar_selectors_map_before_dispatch() {
        let adapter = InfoSessionAdapter::new(
            config("https://vpn.fixture.invalid/"),
            CampusHttpTransport::new("THYou/registrar-fixture").unwrap(),
        )
        .unwrap();
        let raw = crate::info::OpaqueUrl::new(
            "http://zhjw.cic.tsinghua.edu.cn/jxmh.do?ticket=FIXTURE%2Babc%3D",
        )
        .unwrap();
        for selector in [
            "287C0C6D90ABB364CD5FDF1495199962",
            "BEABB32641DC4EC3510B048BAF42471A",
        ] {
            let mapped = adapter.map_additional_roaming(selector, &raw).unwrap();
            let mapped = Url::parse(mapped.as_str()).unwrap();
            assert_eq!(mapped.host_str(), Some("vpn.fixture.invalid"));
            assert_eq!(
                mapped.path(),
                "/http/77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290/jxmh.do"
            );
            assert_eq!(mapped.query(), Some("ticket=FIXTURE%2Babc%3D"));
        }
    }

    #[tokio::test]
    async fn backend_repair_business_roaming_navigation_empty_200_is_not_business_data_proof() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let server = FixtureServer::new(vec![
            Reply::html("XSRF-TOKEN=fixture;"),
            Reply::json(
                r#"{"object":{"roamingurl":"https://learn.tsinghua.edu.cn/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE"}}"#,
            ),
            Reply {
                status: 200,
                headers: "Set-Cookie: fixture-target=ready; Path=/\r\n".into(),
                body: String::new(),
            },
        ]);
        let adapter = InfoSessionAdapter::new(
            config(server.base()),
            CampusHttpTransport::new("THYou/business-fixture").unwrap(),
        )
        .unwrap();
        let target = adapter
            .additional_roaming("3E401364BDD7AEA7EBF1EDE3F15ED4B7")
            .await
            .unwrap();
        assert!(
            target
                .as_str()
                .contains("/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE")
        );
        assert_eq!(server.requests().len(), 3);
    }

    #[test]
    fn backend_repair_business_known_roam_selector_does_not_accept_wrong_same_origin_mapping() {
        let adapter = InfoSessionAdapter::new(
            config("https://vpn.fixture.invalid/"),
            CampusHttpTransport::new("THYou/business-fixture").unwrap(),
        )
        .unwrap();
        for url in [
            "https://vpn.fixture.invalid/http/wrong-map/portal3rd.do?ticket=fixture",
            "https://vpn.fixture.invalid/login?ticket=fixture",
            "https://vpn.fixture.invalid/http/wrong-map%2fportal3rd.do?ticket=fixture",
        ] {
            assert!(
                adapter
                    .map_additional_roaming(
                        "40470BB47E0849E9EF717983490BC964",
                        &crate::info::OpaqueUrl::new(url).unwrap()
                    )
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn backend_repair_business_info_raw_classroom_roamingurl_is_mapped_then_consumed() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let server = FixtureServer::new(vec![
            Reply::html("XSRF-TOKEN=fixture-csrf;"),
            Reply::json(
                r#"{"result":"success","object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/portal3rd.do?ticket=FIXTURE%2Babc&amp;m=home"}}"#,
            ),
            Reply::html("<html>service handoff complete</html>"),
        ]);
        let adapter = InfoSessionAdapter::new(
            config(server.base()),
            CampusHttpTransport::new("THYou/business-fixture").unwrap(),
        )
        .unwrap();
        let url = adapter
            .additional_roaming("40470BB47E0849E9EF717983490BC964")
            .await
            .unwrap();
        assert!(url.as_str().starts_with(server.base()));
        assert_eq!(server.requests().len(), 3);
        assert!(server.requests()[2].starts_with("GET /http/77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290/portal3rd.do?ticket=FIXTURE%2Babc&m=home "));
    }
    #[tokio::test]
    async fn backend_repair_business_info_raw_learn_roamingurl_uses_exact_mapping() {
        use crate::reference_test_support::{FixtureServer, Reply};
        let server = FixtureServer::new(vec![
            Reply::html("XSRF-TOKEN=fixture;"),
            Reply::json(
                r#"{"object":{"roamingurl":"https://learn.tsinghua.edu.cn/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE"}}"#,
            ),
            Reply::html("<html>Learn handoff</html>"),
        ]);
        let adapter = InfoSessionAdapter::new(
            config(server.base()),
            CampusHttpTransport::new("THYou/business-fixture").unwrap(),
        )
        .unwrap();
        adapter
            .additional_roaming("3E401364BDD7AEA7EBF1EDE3F15ED4B7")
            .await
            .unwrap();
        assert_eq!(server.requests().len(), 3);
        assert!(server.requests()[2].contains("/https/77726476706e69737468656265737421fcf2408e297e7c4377068ea48d546d30ca8cc97bcc/b/j_spring_security_thauth_roaming_entry?ticket=FIXTURE"));
    }
    #[tokio::test]
    async fn backend_repair_business_roamingurl_cannot_send_credentials_to_unrelated_target() {
        use crate::reference_test_support::{FixtureServer, Reply};
        for url in [
            "http://evil.invalid/portal3rd.do?ticket=x",
            "http://learn.tsinghua.edu.cn/b/j_spring_security_thauth_roaming_entry?ticket=x",
            "http://zhjw.cic.tsinghua.edu.cn:8080/portal3rd.do?ticket=x",
        ] {
            let server = FixtureServer::new(vec![
                Reply::html("XSRF-TOKEN=fixture;"),
                Reply::json(&serde_json::json!({"object":{"roamingurl":url}}).to_string()),
            ]);
            let adapter = InfoSessionAdapter::new(
                config(server.base()),
                CampusHttpTransport::new("THYou/business-fixture").unwrap(),
            )
            .unwrap();
            assert!(
                adapter
                    .additional_roaming("40470BB47E0849E9EF717983490BC964")
                    .await
                    .is_err()
            );
            assert_eq!(server.requests().len(), 2);
        }
    }
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;
    use crate::protocol::{CsrfToken, ServiceSessionState, UserIdentity};

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        loop {
            let count = stream.read(&mut buffer).expect("request");
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            else {
                continue;
            };
            let header = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = header
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn respond(stream: &mut std::net::TcpStream, content_type: &str, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("response");
    }

    #[tokio::test]
    async fn backend_repair_portal_init_script_login_text_does_not_replace_account_proof() {
        for account in ["fixture-user", "other-user"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                for (path,body) in [
                    ("/wengine-vpn/cookie?",String::new()),
                    ("/target/f/info/gxfw_fg/common/index",r#"<html><script>const notice='请登录';const loginRoute='/do/off/ui/auth/login/form/fixture';</script><a href='/do/off/ui/auth/login/form/fixture'>账号帮助</a><main>portal shell</main></html>"#.to_owned()),
                    ("/wengine-vpn/cookie?","XSRF-TOKEN=fixture-token;".to_owned()),
                    ("/target/b/info/gxfw_fg/common/grjbxx?",format!(r#"{{"result":"success","object":{{"ryh":"{account}"}}}}"#)),
                ] {
                    let (mut stream,_)=listener.accept().unwrap();
                    let request=read_request(&mut stream);assert!(request.starts_with(&format!("GET {path}")));
                    let content=if body.starts_with("<html>") {"text/html"} else {"text/plain"};
                    respond(&mut stream,content,&body);
                }
                listener
            });
            let adapter = InfoSessionAdapter::new(
                config(&base),
                CampusHttpTransport::new("THYou/fixture").unwrap(),
            )
            .unwrap();
            let user = UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: None,
            };
            let result = adapter
                .probe_portal_account_with_resource(&user, true)
                .await;
            if account == "fixture-user" {
                assert_eq!(result.unwrap().as_str(), "fixture-token");
            } else {
                assert!(matches!(
                    result,
                    Err(InfoSessionError::PortalAccountMismatch)
                ));
            }
            let listener = server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[tokio::test]
    async fn backend_repair_portal_resource_returns_ephemeral_server_handoff_without_following() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut cookie, _) = listener.accept().unwrap();
            let _ = read_request(&mut cookie);
            respond(&mut cookie, "text/plain", "");
            let (mut resource, _) = listener.accept().unwrap();
            let _ = read_request(&mut resource);
            resource.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://id.tsinghua.edu.cn/do/off/ui/auth/login/form/fixture/0?flow=fixture-private-flow\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            listener
        });
        let adapter = InfoSessionAdapter::new(
            config(&base),
            CampusHttpTransport::new("THYou/fixture").unwrap(),
        )
        .unwrap();
        let user = UserIdentity {
            username: "fixture-user".to_owned(),
            display_name: None,
        };
        let result = adapter
            .probe_portal_account_with_handoff(&user, true)
            .await
            .unwrap_err();
        assert!(!format!("{result:?}").contains("fixture-private-flow"));
        assert!(!result.to_string().contains("fixture-private-flow"));
        match result {
            InfoSessionError::PortalResourceContinuation(url) => {
                assert!(url.as_str().ends_with("flow=fixture-private-flow"))
            }
            _ => panic!("an allowlisted server-issued handoff was discarded"),
        }
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn backend_repair_portal_resource_login_evidence_requires_real_form_or_whole_notice() {
        let detect = crate::identity_client::service_resource_document_requires_login;
        assert!(detect(
            r#"<html><form method='post' action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass' type='password'></form></html>"#
        ));
        assert!(detect("<html><body>请先登录。</body></html>"));
        assert!(detect(
            r#"<html><form action='/login' method='post'><input name='username'><input name='password' type='password'></form></html>"#
        ));
        assert!(!detect(
            r#"<html><script>const error='请登录';const markup='<input name="i_user"><input name="i_pass">';</script><main>portal shell</main></html>"#
        ));
        assert!(!detect(
            r#"<html><div hidden><form action='/do/off/ui/auth/login/check'><input name='i_user'><input name='i_pass'></form></div><main>portal shell</main></html>"#
        ));
        assert!(!detect(
            r#"<html><a href='/do/off/ui/auth/login/form/fixture'>登录指南</a><article>请登录教学平台后查询通知。</article></html>"#
        ));
    }

    fn config(base: &str) -> InfoWebVpnConfig {
        InfoWebVpnConfig::new(base, "/target").expect("config")
    }

    #[tokio::test]
    async fn backend_repair_missing_portal_csrf_initializes_mapped_resource_then_binds_account() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for (path, kind, body) in [
                ("/wengine-vpn/cookie?", "text/plain", ""),
                (
                    "/target/f/info/gxfw_fg/common/index ",
                    "text/html",
                    "<html><body>INFO fixture</body></html>",
                ),
                (
                    "/wengine-vpn/cookie?",
                    "text/plain",
                    "XSRF-TOKEN=fixture-csrf;",
                ),
                (
                    "/target/b/info/gxfw_fg/common/grjbxx?_csrf=fixture-csrf ",
                    "application/json",
                    r#"{"object":{"ryh":"fixture-user"}}"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let request = read_request(&mut stream);
                assert!(request.starts_with(&format!("GET {path}")));
                respond(&mut stream, kind, body);
            }
            listener
        });
        let adapter = InfoSessionAdapter::new(
            config(&base),
            CampusHttpTransport::new("THYou/fixture").unwrap(),
        )
        .unwrap();
        let user = UserIdentity {
            username: "fixture-user".to_owned(),
            display_name: None,
        };
        assert_eq!(
            adapter
                .probe_portal_account_with_resource(&user, true)
                .await
                .unwrap()
                .as_str(),
            "fixture-csrf"
        );
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn backend_repair_mapped_portal_login_redirect_stops_before_more_cookies_or_account_reads()
     {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut a, _) = listener.accept().unwrap();
            let request = read_request(&mut a);
            assert!(request.starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut a, "text/plain", "");
            let (mut b, _) = listener.accept().unwrap();
            let request = read_request(&mut b);
            assert!(request.starts_with("GET /target/f/info/gxfw_fg/common/index "));
            b.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://id.tsinghua.edu.cn/do/off/ui/auth/login/form/fixture/0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            listener
        });
        let adapter = InfoSessionAdapter::new(
            config(&base),
            CampusHttpTransport::new("THYou/fixture").unwrap(),
        )
        .unwrap();
        let user = UserIdentity {
            username: "fixture-user".to_owned(),
            display_name: None,
        };
        assert!(matches!(
            adapter
                .probe_portal_account_with_resource(&user, true)
                .await,
            Err(InfoSessionError::PortalResourceLoginRequired)
        ));
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn backend_repair_mapped_portal_webvpn_gateway_redirect_is_a_login_boundary() {
        for location in [
            "/",
            "/login?oauth_login=true",
            "/https/77726476706e69737468656265737421f9f30f8834396657761d88e29d51367bcfe7/do/off/ui/auth/login/form/10000ea055dd8d81d09d5a1ba55d39ad/0",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let mapped_prefix = "/https/77726476706e69737468656265737421deadbeef/";
            let location = location.to_owned();
            let server = thread::spawn(move || {
                let (mut cookie, _) = listener.accept().unwrap();
                let _ = read_request(&mut cookie);
                respond(&mut cookie, "text/plain", "");

                let (mut resource, _) = listener.accept().unwrap();
                let request = read_request(&mut resource);
                assert!(request.starts_with(
                    "GET /https/77726476706e69737468656265737421deadbeef/f/info/gxfw_fg/common/index "
                ));
                write!(
                    resource,
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                listener
            });
            let adapter = InfoSessionAdapter::new(
                InfoWebVpnConfig::new(&base, mapped_prefix).unwrap(),
                CampusHttpTransport::new("THYou/fixture").unwrap(),
            )
            .unwrap();
            let user = UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: None,
            };
            let error = adapter
                .probe_portal_account_with_handoff(&user, true)
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                InfoSessionError::PortalResourceAuthRequired(PortalResourceAuthStage::WebVpnLogin)
            ));
            let listener = server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ));
        }
    }

    #[tokio::test]
    async fn backend_repair_mapped_portal_unavailable_is_not_session_expiry_and_never_retries() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut a, _) = listener.accept().unwrap();
            let _ = read_request(&mut a);
            respond(&mut a, "text/plain", "");
            let (mut b, _) = listener.accept().unwrap();
            let _ = read_request(&mut b);
            b.write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            listener
        });
        let adapter = InfoSessionAdapter::new(
            config(&base),
            CampusHttpTransport::new("THYou/fixture").unwrap(),
        )
        .unwrap();
        let user = UserIdentity {
            username: "fixture-user".to_owned(),
            display_name: None,
        };
        assert!(matches!(
            adapter
                .probe_portal_account_with_resource(&user, true)
                .await,
            Err(InfoSessionError::PortalResourceStatus {
                status: StatusCode::SERVICE_UNAVAILABLE
            })
        ));
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn backend_repair_target_csrf_never_overwrites_webvpn_cookie_store() {
        use reqwest::cookie::CookieStore;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            respond(&mut stream, "text/plain", "XSRF-TOKEN=target-fixture;");
        });
        let transport = CampusHttpTransport::new("THYou/fixture").unwrap();
        let origin = Url::parse(&base).unwrap();
        transport
            .cookie_jar()
            .add_cookie_str("XSRF-TOKEN=wrapper-fixture; Path=/", &origin);
        let adapter = InfoSessionAdapter::new(config(&base), transport.clone()).unwrap();
        let token = adapter.bootstrap_cookie().await.unwrap();
        assert_eq!(token.as_str(), "target-fixture");
        let cookie = transport.cookie_jar().cookies(&origin).unwrap();
        assert!(cookie.to_str().unwrap().contains("wrapper-fixture"));
        assert!(!cookie.to_str().unwrap().contains("target-fixture"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn backend_repair_info_rejects_identity_target_suffix_without_dispatch() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let adapter = InfoSessionAdapter::new(
            config(&base),
            CampusHttpTransport::new("THYou/fixture").unwrap(),
        )
        .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            adapter.additional_roaming("0a993de7e533cd43a594459abdcab27d/1"),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(InfoSessionError::InvalidConfig(_))));
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn backend_repair_thuinfo_account_proof_requires_exact_user_and_valid_envelope() {
        assert!(
            validate_portal_account(
                r#"{"result":"success","object":{"ryh":"fixture-user"}}"#,
                "fixture-user"
            )
            .is_ok()
        );
        assert!(matches!(
            validate_portal_account(r#"{"object":{"ryh":"other-user"}}"#, "fixture-user"),
            Err(InfoSessionError::PortalAccountMismatch)
        ));
        for body in [
            "<html>200 OK</html>",
            "{}",
            r#"{"object":{}}"#,
            r#"{"result":"error","object":{"ryh":"fixture-user"}}"#,
            r#"{"success":false,"object":{"ryh":"fixture-user"}}"#,
            r#"{"object":{"ryh":12345}}"#,
        ] {
            assert!(matches!(
                validate_portal_account(body, "fixture-user"),
                Err(InfoSessionError::PortalAccountResponse)
            ));
        }
        assert!(
            !format!(
                "{:?}",
                validate_portal_account(r#"{"object":{"ryh":"private-response"}}"#, "fixture-user")
            )
            .contains("private-response")
        );
    }

    #[tokio::test]
    async fn backend_repair_thuinfo_portal_account_then_news_never_roams_to_mail() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut paths = Vec::new();
            for (needle, body) in [
                ("/wengine-vpn/cookie?", "XSRF-TOKEN=fixture-csrf;"),
                (
                    "/target/b/info/gxfw_fg/common/grjbxx?",
                    r#"{"result":"success","object":{"ryh":"fixture-user"}}"#,
                ),
                (
                    "/target/",
                    r#"{"result":"success","object":{"dataList":[]}}"#,
                ),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let request = read_request(&mut stream);
                assert!(request.starts_with(&format!("GET {needle}")));
                assert!(!request.contains("onlineAppRedirect"));
                assert!(!request.contains("F315577F"));
                paths.push(request.lines().next().unwrap().to_owned());
                respond(&mut stream, "application/json", body);
            }
            (listener, paths)
        });
        let transport = CampusHttpTransport::new("THYou/fixture").unwrap();
        let adapter = InfoSessionAdapter::new(config(&base), transport).unwrap();
        let user = UserIdentity {
            username: "fixture-user".to_owned(),
            display_name: None,
        };
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .unwrap();
        coordinator
            .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
            .unwrap();
        let csrf = adapter.probe_portal_account(&user).await.unwrap();
        let proof = adapter
            .establish_portal(&mut coordinator, user, Some(csrf))
            .await
            .unwrap();
        assert_eq!(proof.snapshot.state, ServiceSessionState::Authenticated);
        let (listener, paths) = server.join().unwrap();
        assert_eq!(paths.len(), 3);
        assert!(paths[2].contains("_csrf=fixture-csrf"));
        listener.set_nonblocking(true).unwrap();
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }

    #[tokio::test]
    async fn backend_repair_wengine_target_body_overrides_unrelated_root_csrf() {
        for (body, expected) in [
            ("", None),
            ("XSRF-TOKEN=info-fixture;", Some("info-fixture")),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = read_request(&mut stream);
                write!(stream,"HTTP/1.1 200 OK\r\nSet-Cookie: XSRF-TOKEN=root-fixture; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            });
            let transport = CampusHttpTransport::new("THYou/fixture").unwrap();
            transport.cookie_jar().add_cookie_str(
                "XSRF-TOKEN=stale-fixture; Path=/",
                &Url::parse(&base).unwrap(),
            );
            let adapter = InfoSessionAdapter::new(config(&base), transport).unwrap();
            match expected {
                Some(value) => {
                    assert_eq!(adapter.bootstrap_cookie().await.unwrap().as_str(), value)
                }
                None => assert!(matches!(
                    adapter.bootstrap_cookie().await,
                    Err(InfoSessionError::MissingCookieCsrf)
                )),
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn target_prefix_is_applied_without_reconstructing_a_host_mapping() {
        let config = config("https://webvpn.example.test/");
        let endpoint = config.target_url("/b/info/news").expect("target endpoint");
        assert_eq!(endpoint.path(), "/target/b/info/news");
        assert!(InfoWebVpnConfig::new("https://webvpn.example.test/", "https://foreign/").is_err());
        assert!(InfoWebVpnConfig::new("https://webvpn.example.test/", "/").is_err());
        assert!(path_has_prefix("/target/home", "/target"));
        assert!(!path_has_prefix("/target-foreign/home", "/target"));
    }

    #[test]
    fn debug_output_does_not_include_cookie_or_csrf_material() {
        let config = config("https://webvpn.example.test/");
        let debug = format!("{config:?}");
        assert!(!debug.contains("cookie-value"));
        assert!(!debug.contains("csrf-value"));
    }

    #[test]
    fn handoff_html_detection_accepts_bom_and_leading_whitespace() {
        assert!(looks_like_html_document(
            None,
            "\u{feff}\n  <!doctype html><html><body>service</body></html>"
        ));
        assert!(!looks_like_html_document(
            Some("text/html; charset=utf-8"),
            "  {\"object\":{}}"
        ));
    }

    #[test]
    fn public_config_fields_are_revalidated_before_adapter_creation() {
        let mut config =
            InfoWebVpnConfig::new("https://webvpn.example.test/", "/target").expect("config");
        config.target_prefix = "/target/../outside".to_owned();
        let transport = CampusHttpTransport::new("THYou/config-test").expect("transport");
        assert!(matches!(
            InfoSessionAdapter::new(config, transport),
            Err(InfoSessionError::InvalidConfig(_))
        ));
    }

    #[test]
    fn cookie_token_matching_requires_the_exact_cookie_name() {
        assert_eq!(
            extract_cookie_token(
                "OTHER-XSRF-TOKEN=wrong; XSRF-TOKEN=right; Path=/",
                "XSRF-TOKEN"
            ),
            Some("right")
        );
        assert_eq!(
            extract_cookie_token("XSRF-TOKEN=wrong\nvalue; Path=/", "XSRF-TOKEN"),
            None
        );
    }

    #[test]
    fn article_links_are_limited_to_the_webvpn_target_and_require_xxid() {
        assert_eq!(
            extract_news_xxid(r#"<script>var xxid = "article-1";</script>"#),
            Some("article-1".to_owned())
        );
        assert_eq!(
            extract_news_xxid("<script>var xxid = 'article-2';</script>"),
            Some("article-2".to_owned())
        );
        assert_eq!(extract_news_xxid("<html>ordinary article</html>"), None);
        assert_eq!(
            extract_news_xxid(r#"<script>// var xxid = "fake"; var label = "text";</script>"#),
            None
        );
        assert_eq!(
            extract_news_xxid(r#"<script>var xxid = "missing-semicolon"</script>"#),
            None
        );
        assert_eq!(
            extract_news_xxid(r#"<div>var xxid = "article-in-text";</div>"#),
            None
        );

        let config = config("https://webvpn.example.test/");
        let endpoint = config
            .target_url("/b/info/article")
            .expect("target endpoint");
        assert!(same_origin(&config.webvpn_base_url, &endpoint));
        assert!(redirect_leaves_origin(
            &config.webvpn_base_url,
            &endpoint,
            "https://identity.example.test/login"
        ));

        let adapter = InfoSessionAdapter::new(
            config,
            CampusHttpTransport::new("THYou/article-link-test").expect("transport"),
        )
        .expect("adapter");
        assert!(matches!(
            adapter.news_link_url("/article?source=fixture#fragment"),
            Err(InfoSessionError::InvalidConfig(_))
        ));
    }

    #[test]
    fn additional_handoff_keeps_an_opaque_mapping_and_rejects_control_paths() {
        let base = Url::parse("https://webvpn.example.test/").expect("base URL");
        let library =
            Url::parse("https://webvpn.example.test/library-target/home").expect("library URL");
        assert!(validate_additional_roaming_url(&base, &library, "/library-target").is_ok());

        for path in ["/", "/wengine-vpn/cookie", "/library-target/../outside"] {
            let url =
                Url::parse(&format!("https://webvpn.example.test{path}")).expect("candidate URL");
            assert!(matches!(
                validate_additional_roaming_url(&base, &url, "/library-target"),
                Err(InfoSessionError::HandoffUnexpectedPath)
            ));
        }
    }

    #[test]
    fn mapped_http_webvpn_paths_keep_the_opaque_token_in_the_prefix() {
        assert_eq!(
            mapped_path_prefix("/http/opaque-token/service/home"),
            Some("/http/opaque-token".to_owned())
        );
        assert_eq!(mapped_path_prefix("/http"), None);
        assert_eq!(
            mapped_path_prefix("/https/opaque-token/service/home"),
            Some("/https/opaque-token".to_owned())
        );
    }

    #[tokio::test]
    async fn additional_handoff_accepts_a_different_same_origin_mapping() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("cookie request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("portal request");
            let request = read_request(&mut stream);
            assert!(request.starts_with(
                "GET /info-target/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect?"
            ));
            assert!(request.contains("yyfwid=library-service"));
            let body =
                format!(r#"{{"object":{{"roamingurl":"http://{address}/library-target/home"}}}}"#);
            respond(&mut stream, "application/json", &body);

            let (mut stream, _) = listener.accept().expect("library handoff probe");
            assert!(read_request(&mut stream).starts_with("GET /library-target/home HTTP/1.1"));
            respond(
                &mut stream,
                "text/html; charset=utf-8",
                "<html><body>library home</body></html>",
            );
        });

        let config =
            InfoWebVpnConfig::new(&format!("http://{address}/"), "/info-target").expect("config");
        let transport = CampusHttpTransport::new("THYou fixture").expect("transport");
        let adapter = InfoSessionAdapter::new(config, transport).expect("adapter");
        let roaming = adapter
            .additional_roaming("library-service")
            .await
            .expect("library handoff");
        assert_eq!(
            roaming.as_str(),
            format!("http://{address}/library-target/home")
        );
        server.join().expect("fixture server");
    }

    #[tokio::test]
    async fn additional_handoff_returns_the_final_proved_url_after_same_origin_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("cookie request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /wengine-vpn/cookie?"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("user-agent: thyou/final-url-fixture")
            );
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("portal request");
            let request = read_request(&mut stream);
            assert!(request.starts_with(
                "GET /info-target/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect?"
            ));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("cookie: xsrf-token=fixture-csrf")
            );
            let body =
                format!(r#"{{"object":{{"roamingurl":"http://{address}/library-target/entry"}}}}"#);
            respond(&mut stream, "application/json", &body);

            let (mut stream, _) = listener.accept().expect("handoff entry");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /library-target/entry HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("cookie: xsrf-token=fixture-csrf")
            );
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /library-target/home\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect");

            let (mut stream, _) = listener.accept().expect("handoff final page");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /library-target/home HTTP/1.1"));
            respond(
                &mut stream,
                "text/html; charset=utf-8",
                "<html><body>library home</body></html>",
            );
        });

        let config =
            InfoWebVpnConfig::new(&format!("http://{address}/"), "/info-target").expect("config");
        let transport = CampusHttpTransport::new("THYou/final-url-fixture").expect("transport");
        let adapter = InfoSessionAdapter::new(config, transport).expect("adapter");
        let roaming = adapter
            .additional_roaming("library-service")
            .await
            .expect("library handoff");

        assert_eq!(
            roaming.as_str(),
            format!("http://{address}/library-target/home")
        );
        server.join().expect("fixture server");
    }

    #[tokio::test]
    async fn fixture_promotes_after_cookie_handoff_probe_and_executes_news() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let roaming_url = format!("http://{address}/target/home");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("cookie request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /wengine-vpn/cookie?"));
            assert!(request.contains("host=info.tsinghua.edu.cn"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("portal request");
            let request = read_request(&mut stream);
            assert!(
                request.contains(
                    "GET /target/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect?"
                )
            );
            assert!(request.contains("machine=p"));
            assert!(request.contains("yyfwid=fixture-service"));
            assert!(request.contains("_csrf=fixture-csrf"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("user-agent: thyou fixture")
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("cookie: xsrf-token=fixture-csrf")
            );
            let body = format!(r#"{{"object":{{"roamingurl":"{roaming_url}"}}}}"#);
            respond(&mut stream, "application/json", &body);

            let (mut stream, _) = listener.accept().expect("probe request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /target/home HTTP/1.1"));
            respond(
                &mut stream,
                "text/html; charset=utf-8",
                "<html><body>INFO home</body></html>",
            );

            let (mut stream, _) = listener.accept().expect("news proof CSRF refresh request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("news probe request");
            let request = read_request(&mut stream);
            assert!(request.contains("GET /target/b/info/xxfb_fg/xnzx/template/more?"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("user-agent: thyou fixture")
            );
            assert!(request.contains("currentPage=1"));
            assert!(request.contains("length=1"));
            assert!(request.contains("_csrf=fixture-csrf"));
            respond(
                &mut stream,
                "application/json",
                r#"{"object":{"dataList":[]}}"#,
            );

            let (mut stream, _) = listener.accept().expect("news CSRF refresh request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("news request");
            let request = read_request(&mut stream);
            assert!(request.contains("GET /target/b/info/xxfb_fg/xnzx/template/more?"));
            assert!(request.contains("oType=xs"));
            assert!(request.contains("lydw="));
            assert!(request.contains("lmid=all"));
            assert!(request.contains("currentPage=1"));
            assert!(request.contains("length=10"));
            assert!(request.contains("_csrf=fixture-csrf"));
            respond(
                &mut stream,
                "application/json",
                r#"{"object":{"dataList":[{"bt":"Fixture news","url":"/article","xxid":"fixture-id","time":"2026-09-11 10:00:00","dwmc_show":"Fixture source","yxzd":"1-","lmid":"LM_JWGG","sfsc":false}]}}"#,
            );

            let (mut stream, _) = listener.accept().expect("search CSRF refresh request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("search request");
            let request = read_request(&mut stream);
            assert!(
                request
                    .contains("POST /target/b/xnzx/search/info/xxfb_fg/teacher/getMobilePageList?")
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("user-agent: thyou fixture")
            );
            assert!(request.contains("_csrf=fixture-csrf"));
            assert!(request.contains("esParamClass="));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("content-type: application/x-www-form-urlencoded")
            );
            assert!(request.contains("params"));
            assert!(request.contains("filterParams"));
            assert!(request.contains("lmmcgroup"));
            assert!(request.contains("orderMap"));
            assert!(request.contains("matchExact"));
            assert!(request.contains("currentPage"));
            respond(
                &mut stream,
                "application/json",
                r#"{"result":"success","object":{"resultsList":[{"bt":"<strong>Fixture search</strong>","url":"/article-search","xxid":"fixture-search-id","time":"2026-09-11 11:00:00","dwmc_show":"Fixture source","yxzd":null,"lmid":"LM_BGTG","sfsc":true}]}}"#,
            );

            let (mut stream, _) = listener.accept().expect("detail CSRF refresh request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("detail article page request");
            let request = read_request(&mut stream);
            assert!(request.starts_with("GET /target/article?source=fixture HTTP/1.1"));
            respond(
                &mut stream,
                "text/html; charset=utf-8",
                r#"<html><script>var xxid = "fixture-id";</script></html>"#,
            );

            let (mut stream, _) = listener.accept().expect("detail API CSRF refresh request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("detail request");
            let request = read_request(&mut stream);
            assert!(request.contains("GET /target/b/info/xxfb_fg/xnzx/template/detail?"));
            assert!(request.contains("xxid=fixture-id"));
            assert!(request.contains("preview="));
            assert!(request.contains("_csrf=fixture-csrf"));
            respond(
                &mut stream,
                "application/json",
                r#"{"result":"success","object":{"xxDto":{"xxid":"fixture-id","bt":"Fixture detail","nr":"%3Cp%3EFixture%20body%3C%2Fp%3E","fjs_template":[{"wjid":"fixture-file","wjmc":"fixture.pdf"}]}}}"#,
            );
        });

        let transport = CampusHttpTransport::new("THYou fixture").expect("transport");
        let adapter = InfoSessionAdapter::new(config(&format!("http://{address}/")), transport)
            .expect("adapter");
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity begin");
        coordinator
            .mark_authenticated(
                ServiceId::Identity,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .expect("identity proof");

        let result = adapter
            .establish(
                &mut coordinator,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                "fixture-service",
            )
            .await
            .expect("INFO session");
        assert_eq!(result.snapshot.state, ServiceSessionState::Authenticated);
        assert!(result.snapshot.csrf_present());

        let page = adapter
            .fetch_news_list(&coordinator, 1, 10, None, None)
            .await
            .expect("news page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "fixture-id");
        let search = adapter
            .search_news(
                &coordinator,
                &NewsSearchInput::new("fixture", 1).with_channel_filter("办公通知"),
            )
            .await
            .expect("search page");
        assert_eq!(search.items[0].id, "fixture-search-id");
        let detail = adapter
            .fetch_news_detail(&coordinator, "/article?source=fixture")
            .await
            .expect("detail");
        assert_eq!(detail.title, "Fixture detail");
        assert_eq!(detail.summary, "Fixture body");
        assert_eq!(detail.attachments[0].name, "fixture.pdf");
        server.join().expect("fixture server");
    }

    #[tokio::test]
    async fn http_200_login_page_is_not_reported_as_an_empty_news_page() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("cookie request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("portal request");
            let request = read_request(&mut stream);
            assert!(request.contains("onlineAppRedirect"));
            let body = format!(r#"{{"object":{{"roamingurl":"http://{address}/target/home"}}}}"#);
            respond(&mut stream, "application/json", &body);

            let (mut stream, _) = listener.accept().expect("probe request");
            assert!(read_request(&mut stream).starts_with("GET /target/home HTTP/1.1"));
            respond(
                &mut stream,
                "text/html; charset=utf-8",
                "<html><body>INFO home</body></html>",
            );

            let (mut stream, _) = listener.accept().expect("news proof CSRF refresh request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("news probe request");
            assert!(
                read_request(&mut stream).contains("/target/b/info/xxfb_fg/xnzx/template/more?")
            );
            respond(
                &mut stream,
                "text/html; charset=utf-8",
                "<!doctype html><html><form><input name=\"i_user\"><input name=\"i_pass\"></form></html>",
            );
        });

        let transport = CampusHttpTransport::new("THYou fixture").expect("transport");
        let adapter = InfoSessionAdapter::new(config(&format!("http://{address}/")), transport)
            .expect("adapter");
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity begin");
        coordinator
            .mark_authenticated(
                ServiceId::Identity,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .expect("identity proof");
        let error = adapter
            .establish(
                &mut coordinator,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                "fixture-service",
            )
            .await
            .expect_err("login page must fail during session proof");
        assert!(matches!(error, InfoSessionError::LoginRequired));
        server.join().expect("fixture server");
    }

    #[tokio::test]
    async fn news_read_rejects_a_valid_envelope_from_another_target_route() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut cookie, _) = listener.accept().expect("cookie request");
            assert!(read_request(&mut cookie).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut cookie, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut redirect, _) = listener.accept().expect("news request");
            let _ = read_request(&mut redirect);
            redirect
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /target/unrelated\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("redirect response");

            let (mut unrelated, _) = listener.accept().expect("unrelated route");
            let _ = read_request(&mut unrelated);
            respond(
                &mut unrelated,
                "application/json",
                r#"{"object":{"dataList":[]}}"#,
            );
        });

        let transport = CampusHttpTransport::new("THYou/info-route-proof-test").expect("transport");
        let adapter = InfoSessionAdapter::new(config(&format!("http://{address}/")), transport)
            .expect("adapter");
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Info)
            .expect("begin INFO session");
        let csrf = coordinator.registry().bind_csrf(
            ServiceId::Info,
            CsrfToken::new("fixture-csrf").expect("csrf"),
        );
        coordinator
            .mark_authenticated(
                ServiceId::Info,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                None,
                Some(csrf),
                None,
            )
            .expect("INFO proof");

        assert!(matches!(
            adapter
                .fetch_news_list(&coordinator, 1, 1, None, None)
                .await,
            Err(InfoSessionError::NewsUnexpectedPath)
        ));
        server.join().expect("server");
    }

    #[tokio::test]
    async fn non_html_handoff_cannot_promote_an_info_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("cookie request");
            assert!(read_request(&mut stream).starts_with("GET /wengine-vpn/cookie?"));
            respond(&mut stream, "text/plain", "XSRF-TOKEN=fixture-csrf;");

            let (mut stream, _) = listener.accept().expect("portal request");
            assert!(read_request(&mut stream).contains("onlineAppRedirect"));
            let body = format!(r#"{{"object":{{"roamingurl":"http://{address}/target/home"}}}}"#);
            respond(&mut stream, "application/json", &body);

            let (mut stream, _) = listener.accept().expect("probe request");
            assert!(read_request(&mut stream).starts_with("GET /target/home HTTP/1.1"));
            respond(&mut stream, "application/json", "{}");
        });

        let transport = CampusHttpTransport::new("THYou fixture").expect("transport");
        let adapter = InfoSessionAdapter::new(config(&format!("http://{address}/")), transport)
            .expect("adapter");
        let mut coordinator = SessionCoordinator::new();
        coordinator
            .begin_authentication(ServiceId::Identity)
            .expect("identity begin");
        coordinator
            .mark_authenticated(
                ServiceId::Identity,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                None,
                None,
                None,
            )
            .expect("identity proof");

        let error = adapter
            .establish(
                &mut coordinator,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                "fixture-service",
            )
            .await
            .expect_err("JSON handoff must not prove an INFO session");
        assert!(matches!(error, InfoSessionError::SessionProofMissing));
        assert_eq!(
            coordinator.registry().snapshot_for(ServiceId::Info).state,
            ServiceSessionState::Anonymous
        );
        server.join().expect("fixture server");
    }

    #[tokio::test]
    async fn missing_identity_session_does_not_touch_the_network() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let transport = CampusHttpTransport::new("THYou fixture").expect("transport");
        let adapter = InfoSessionAdapter::new(config(&format!("http://{address}/")), transport)
            .expect("adapter");
        let mut coordinator = SessionCoordinator::new();
        let error = adapter
            .establish(
                &mut coordinator,
                UserIdentity {
                    username: "fixture-user".to_owned(),
                    display_name: None,
                },
                "fixture-service",
            )
            .await
            .expect_err("identity is required");
        assert!(matches!(error, InfoSessionError::IdentityNotAuthenticated));
        assert_eq!(
            coordinator.registry().snapshot_for(ServiceId::Info).state,
            ServiceSessionState::Anonymous
        );
        let _ = listener;
    }

    #[tokio::test]
    async fn backend_repair_resource_auth_phase_survives_without_response_or_retry() {
        for (status, body, reason) in [
            (
                "401 Unauthorized",
                "fixture-private-body",
                "portal_resource_http_rejected",
            ),
            (
                "200 OK",
                "<html><body>请先登录。</body></html>",
                "portal_resource_document_login",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut cookie, _) = listener.accept().unwrap();
                let _ = read_request(&mut cookie);
                respond(&mut cookie, "text/plain", "");
                let (mut resource, _) = listener.accept().unwrap();
                let req = read_request(&mut resource);
                assert!(req.starts_with("GET /target/f/info/gxfw_fg/common/index HTTP/1.1"));
                write!(resource,"HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                listener
            });
            let adapter = InfoSessionAdapter::new(
                config(&base),
                CampusHttpTransport::new("THYou/fixture").unwrap(),
            )
            .unwrap();
            let user = UserIdentity {
                username: "fixture-user".to_owned(),
                display_name: None,
            };
            let error = adapter
                .probe_portal_account_with_handoff(&user, true)
                .await
                .unwrap_err();
            assert!(!format!("{error:?}").contains("fixture-private-body"));
            match error {
                InfoSessionError::PortalResourceAuthRequired(stage) => {
                    assert_eq!(stage.reason(), reason);
                    assert_eq!(crate::live_validation::error_reason(stage.reason()), reason);
                }
                _ => panic!("resource failure lost its fixed stage"),
            }
            let listener = server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }
}

#[cfg(test)]
#[path = "info_lastmile_tests.rs"]
mod lastmile_tests;

#[path = "info_news_navigation.rs"]
mod news_navigation;

#[cfg(test)]
#[path = "info_news_redirect_repair_tests.rs"]
mod news_redirect_repair_tests;
