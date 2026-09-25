//! Execution boundary for the unified identity client.
//!
//! `identity_client` owns route/profile validation and HTML evidence parsing.
//! This module adds the shared Cookie transport around those plans while
//! retaining the existing cryptographic boundary: both URL-encoded and
//! explicitly profiled multipart submission only accept a caller-provided
//! verified wire password. The identity session layer obtains the login-page
//! public key and performs the SM2 conversion before calling this executor.

use std::{fmt, sync::Arc};

use reqwest::{
    Client, Method, StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION, SET_COOKIE},
    redirect::Policy,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    identity::{FormEncoding, SecondAuthAction, SecondAuthMethod},
    identity_client::{
        IdentityClient, IdentityClientError, LoginFormInput, LoginPageEvidence,
        LoginResponseClassification, PasswordInput,
    },
    transport::{CampusHttpTransport, TransportError},
};

const MAX_IDENTITY_REDIRECT_RETRIES: usize = 0; // Never replay one-time authentication continuations automatically.
const MAX_IDENTITY_REDIRECT_HOPS: usize = 10;

/// Errors raised while executing an identity request.
#[derive(Debug, Error)]
pub enum IdentityExecutionError {
    #[error(transparent)]
    Client(#[from] IdentityClientError),

    #[error("identity transport request failed")]
    Transport(
        #[from]
        #[source]
        TransportError,
    ),

    #[error("identity request could not be built")]
    RequestBuild,

    #[error("identity redirect chain exceeded the safe limit")]
    RedirectChainTooLong,
}

/// A response retained for the identity page classifier.
///
/// The body is available to Rust parsers but its debug output contains only
/// its length, so diagnostics cannot print login HTML or tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct IdentityHttpResponse {
    pub status: StatusCode,
    pub final_url: Url,
    /// A redirect location retained when the transport stopped before a
    /// cross-origin handoff.  The query is redacted by `Debug` and is only
    /// consumed by the identity profile's allowlisted ticket parser.
    pub redirect_location: Option<Url>,
    /// Only the presence of a non-empty Set-Cookie update in this response's
    /// explicitly followed redirect chain is retained. Cookie names and
    /// values never enter diagnostics or the session result. An empty cookie
    /// deletion is deliberately not authentication proof.
    cookie_update: bool,
    body: String,
}

impl IdentityHttpResponse {
    /// A navigation response carries no trusted identity proof. Cookies stay
    /// inside the shared transport; the service must prove its own session.
    pub(crate) fn from_service_navigation(
        status: StatusCode,
        final_url: Url,
        redirect_location: Option<Url>,
        body: String,
    ) -> Self {
        Self {
            status,
            final_url,
            redirect_location,
            cookie_update: false,
            body,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_parts(status: StatusCode, final_url: Url, body: impl Into<String>) -> Self {
        Self {
            status,
            final_url,
            redirect_location: None,
            cookie_update: false,
            body: body.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_parts_with_cookie_update(
        status: StatusCode,
        final_url: Url,
        body: impl Into<String>,
    ) -> Self {
        Self {
            status,
            final_url,
            redirect_location: None,
            cookie_update: true,
            body: body.into(),
        }
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    pub(crate) fn has_cookie_update(&self) -> bool {
        self.cookie_update
    }
}

impl fmt::Debug for IdentityHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentityHttpResponse")
            .field("status", &self.status)
            .field("final_url", &diagnostic_url(&self.final_url))
            .field("cookie_update", &self.cookie_update)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Cookie-aware executor for the identity request plans.
pub struct IdentityExecutionClient {
    client: IdentityClient,
    transport: CampusHttpTransport,
}

impl IdentityExecutionClient {
    pub fn new(client: IdentityClient, user_agent: &str) -> Result<Self, IdentityExecutionError> {
        let transport = CampusHttpTransport::new(user_agent)?;
        Ok(Self { client, transport })
    }

    pub fn with_transport(client: IdentityClient, transport: CampusHttpTransport) -> Self {
        Self { client, transport }
    }

    pub fn client(&self) -> &IdentityClient {
        &self.client
    }

    /// Rebinds the dynamic identity application id while retaining the
    /// current Cookie-aware transport.
    pub fn set_app_id(&mut self, app_id: impl Into<String>) -> Result<(), IdentityExecutionError> {
        self.client = self.client.with_app_id(app_id)?;
        Ok(())
    }

    /// Binds the complete dynamic WebVPN login URL while retaining the same
    /// Cookie-aware transport.  The URL may contain the signed OAuth callback
    /// query; the identity client keeps it private and redacts it from Debug.
    pub fn set_login_page_url(
        &mut self,
        login_page_url: &Url,
    ) -> Result<(), IdentityExecutionError> {
        self.client = self.client.with_login_page_url(login_page_url)?;
        Ok(())
    }

    /// Binds the POST action discovered from the same dynamic identity form
    /// while retaining the current Cookie-aware transport.
    pub(crate) fn set_login_submit_url(
        &mut self,
        submit_url: &Url,
    ) -> Result<(), IdentityExecutionError> {
        self.client = self.client.with_login_submit_url(submit_url)?;
        Ok(())
    }

    /// Binds the encoding declared by the currently discovered dynamic form
    /// while retaining the same Cookie-aware transport and URL overrides.
    pub(crate) fn set_login_form_encoding(
        &mut self,
        encoding: FormEncoding,
    ) -> Result<(), IdentityExecutionError> {
        self.client = self.client.with_login_form_encoding(encoding)?;
        Ok(())
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    /// Replaces the cookie-aware client with a fresh jar while retaining the
    /// validated identity request profile.  A runtime can be reused after
    /// logout, so the next account must never inherit cookies from the prior
    /// account's identity session.
    pub fn reset_transport(&mut self, user_agent: &str) -> Result<(), IdentityExecutionError> {
        self.transport = CampusHttpTransport::new(user_agent)?;
        Ok(())
    }

    /// Fetches the login page so the transport receives its cookies and the
    /// caller can inspect the configured page evidence.
    pub async fn fetch_login_page(&self) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let plan = self.client.login_page_request_plan()?;
        let request = self
            .transport
            .client()
            .request(plan.method, plan.url)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Continues only a server-selected, strictly allowlisted trusted-device
    /// form using the existing transport. This does not prove any service
    /// session: the caller must still validate the target handoff and data.
    /// There is exactly one POST and no credential-bearing fallback/retry.
    pub(crate) async fn continue_service_check_single(
        &self,
        page: &IdentityHttpResponse,
        fingerprint: &str,
    ) -> Result<Option<IdentityHttpResponse>, IdentityExecutionError> {
        if !page.status.is_success() {
            return Ok(None);
        }
        let Some(action) = self
            .client
            .service_check_single_action(&page.final_url, page.body())
        else {
            return Ok(None);
        };
        let client = self.client.with_login_submit_url(&action)?;
        let execution = Self::with_transport(client, self.transport.clone());
        let input = LoginFormInput::new("", PasswordInput::precomputed_wire(""))
            .with_fingerprint(fingerprint)
            .with_generated_fingerprint("");
        execution
            .submit_check_single_multipart(input)
            .await
            .map(Some)
    }

    /// Fetches another identity application login page on the same origin
    /// while retaining this executor's Cookie jar.  This is used by services
    /// whose SSO target has a fixed application id (for example campus card)
    /// instead of the dynamic WebVPN application id used by primary login.
    pub async fn fetch_login_page_for_app_id(
        &self,
        app_id: &str,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let client = self.client.with_app_id(app_id.to_owned())?;
        let plan = client.login_page_request_plan()?;
        let request = self
            .transport
            .client()
            .request(plan.method, plan.url)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Fetches an explicitly profiled identity login route on the same origin
    /// while retaining the current Cookie jar.  Some legacy service SSO
    /// routes use a compound path such as `0?/userindex`, which must not be
    /// encoded as one `{appId}` path segment.
    pub async fn fetch_login_page_at_path(
        &self,
        path: &str,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let plan = self.client.login_page_url_at_path(path)?;
        let request = self
            .transport
            .client()
            .request(Method::GET, plan)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Submits the login form using the encoding selected by the profile.
    /// Plaintext passwords are converted by the session layer before this
    /// method is reached.
    pub async fn submit_login(
        &self,
        input: LoginFormInput<'_>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        match self.client.login_submission_request_plan()? {
            crate::identity_client::LoginSubmissionRequestPlan::UrlEncoded(_) => {
                tracing::debug!(target:"tsinghua_kit::auth",event="identity_submission_profile",service="identity",submission_encoding="urlencoded",submission_route="credential_check");
                self.submit_urlencoded(input).await
            }
            crate::identity_client::LoginSubmissionRequestPlan::Multipart(_) => {
                tracing::debug!(target:"tsinghua_kit::auth",event="identity_submission_profile",service="identity",submission_encoding="multipart",submission_route="credential_check");
                self.submit_multipart(input).await
            }
            crate::identity_client::LoginSubmissionRequestPlan::TrustedDeviceMultipart(_) => {
                tracing::debug!(target:"tsinghua_kit::auth",event="identity_submission_profile",service="identity",submission_encoding="multipart",submission_route="trusted_check_single");
                self.submit_check_single_multipart(input).await
            }
        }
    }

    /// Submits a URL-encoded identity form against a fixed application id,
    /// using the same cookie-aware transport as the primary session.
    fn service_login_client(&self, app_id: &str) -> Result<IdentityClient, IdentityExecutionError> {
        // A fixed service roam has its own URL-encoded /check contract. The
        // main OAuth entry may have used multipart/checkSingle; neither its
        // action override nor its encoding belongs to this target application.
        Ok(self
            .client
            .with_app_id(app_id.to_owned())?
            .with_login_form_encoding(crate::identity::FormEncoding::UrlEncoded)?)
    }

    pub async fn submit_login_for_app_id(
        &self,
        app_id: &str,
        input: LoginFormInput<'_>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let client = self.service_login_client(app_id)?;
        let form = client.build_urlencoded_login_form(input)?;
        let url = form.url.clone();
        let content_type = form.content_type();
        let body = form.encoded_body();
        let request = self
            .transport
            .client()
            .request(Method::POST, url)
            .header(CONTENT_TYPE, content_type)
            .body(body)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Submits a URL-encoded form using a caller-provided verified wire
    /// password. Plaintext is rejected by `IdentityClient` before a request is
    /// built. `IdentitySessionOrchestrator` is the higher-level caller that
    /// converts plaintext with the login-page SM2 key, and the borrowed form is
    /// dropped after the request body is made.
    pub async fn submit_urlencoded(
        &self,
        input: LoginFormInput<'_>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let form = self.client.build_urlencoded_login_form(input)?;
        let url = form.url.clone();
        let content_type = form.content_type();
        let body = form.encoded_body();
        let request = self
            .transport
            .client()
            .request(Method::POST, url)
            .header(CONTENT_TYPE, content_type)
            .body(body)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Builds and submits one explicit multipart login body. The boundary is
    /// generated here so it never becomes profile data or a caller concern.
    pub async fn submit_multipart(
        &self,
        input: LoginFormInput<'_>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let boundary = format!("THYou-{}", Uuid::new_v4().simple());
        let form = self.client.build_multipart_login_form(input, boundary)?;
        let url = form.url.clone();
        let content_type = form.content_type();
        let body = form.body();
        let request = self
            .transport
            .client()
            .request(Method::POST, url)
            .header(CONTENT_TYPE, content_type)
            .body(body)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Submits the trusted-device `checkSingle` form. This route has a
    /// different multipart shape from the credential-bearing login form and
    /// is built by a dedicated client method that cannot serialize a password.
    pub async fn submit_check_single_multipart(
        &self,
        input: LoginFormInput<'_>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let boundary = format!("THYou-{}", Uuid::new_v4().simple());
        let form = self
            .client
            .build_check_single_multipart_login_form(input, boundary)?;
        let url = form.url.clone();
        let content_type = form.content_type();
        let body = form.body();
        let request = self
            .transport
            .client()
            .request(Method::POST, url)
            .header(CONTENT_TYPE, content_type)
            .body(body)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Sends one of the identity service's URL-encoded second-factor actions.
    /// The code, when present, is borrowed only long enough to build the
    /// request body and is never retained by the executor.
    pub async fn submit_second_auth<'a>(
        &self,
        action: &'a SecondAuthAction,
        method: Option<&'a SecondAuthMethod>,
        verification_code: Option<&'a str>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let form = self
            .client
            .build_second_auth_form(action, method, verification_code)?;
        let url = form.url.clone();
        let content_type = form.content_type();
        let body = form.encoded_body();
        let request = self
            .transport
            .client()
            .request(Method::POST, url)
            .header(CONTENT_TYPE, content_type)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "*/*")
            .body(body)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Sends the explicitly profiled saveFinger request. Response semantics
    /// remain in the session layer so this boundary never exposes a token.
    pub async fn submit_trusted_device(
        &self,
        input: crate::identity_client::TrustedDeviceInput<'_>,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let form = self.client.build_trusted_device_form(input)?;
        let url = form.url.clone();
        let content_type = form.content_type();
        let body = form.encoded_body();
        let request = self
            .transport
            .client()
            .request(Method::POST, url)
            .header(CONTENT_TYPE, content_type)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Accept", "*/*")
            .body(body)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;
        self.execute(request).await
    }

    /// Fetches a same-origin redirect returned after second-factor success.
    pub async fn fetch_redirect(
        &self,
        redirect: &str,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        self.fetch_allowed_redirect(redirect, false).await
    }

    /// Fetches an HTML service-anchor continuation after the session layer has
    /// extracted it from an already received page. This is intentionally
    /// separate from [`Self::fetch_redirect`]: a JSON `redirectUrl` may only
    /// name an identity/WebVPN continuation, while an HTML anchor may name an
    /// allowlisted `/b/` or `/f/` service handoff.
    pub(crate) async fn fetch_handoff_redirect(
        &self,
        redirect: &str,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        self.fetch_allowed_redirect(redirect, true).await
    }

    async fn fetch_allowed_redirect(
        &self,
        redirect: &str,
        service_handoff: bool,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        let url = self.client.resolve_redirect_url(redirect)?;
        // `resolve_redirect_url` also serves callers that only need to
        // inspect a same-origin URL.  A network fetch is narrower: every
        // callback returned by identity must be an explicitly allowlisted
        // handoff route, otherwise a same-origin `redirectUrl` could make us
        // request an unrelated identity endpoint.
        let allowed = if service_handoff {
            self.client.is_safe_handoff_redirect(redirect)
        } else {
            self.client.is_second_factor_redirect_url(redirect)
        };
        if !allowed {
            return Err(IdentityExecutionError::Client(
                IdentityClientError::InvalidConfig {
                    message: if service_handoff {
                        String::from("identity service handoff is not an allowlisted route")
                    } else {
                        String::from(
                            "identity second-factor redirect is not an allowlisted continuation route",
                        )
                    },
                },
            ));
        }
        for attempt in 0..=MAX_IDENTITY_REDIRECT_RETRIES {
            let request = self
                .transport
                .client()
                .request(Method::GET, url.clone())
                .build()
                .map_err(|_| IdentityExecutionError::RequestBuild)?;
            match self.execute(request).await {
                Ok(response) => return Ok(response),
                Err(error)
                    if attempt < MAX_IDENTITY_REDIRECT_RETRIES
                        && retryable_identity_redirect_error(&error) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        unreachable!("identity redirect retry loop must return on every attempt")
    }

    pub fn classify_response(
        &self,
        response: &IdentityHttpResponse,
    ) -> LoginResponseClassification {
        self.client.classify_login_response_with_location(
            response.status,
            &response.final_url,
            response.redirect_location.as_ref(),
            &response.body,
        )
    }

    pub fn classify_login_page(&self, response: &IdentityHttpResponse) -> LoginPageEvidence {
        self.client.parse_login_page(&response.body)
    }

    async fn execute(
        &self,
        request: reqwest::Request,
    ) -> Result<IdentityHttpResponse, IdentityExecutionError> {
        // The shared CampusHttpTransport deliberately follows same-origin
        // redirects. That is safe for ordinary service requests, but it hides
        // an intermediate Set-Cookie from this identity proof boundary. The
        // public identity clients observe every redirect response, so use a
        // short-lived no-redirect client with the same Jar here. Cookies stay
        // shared with the rest of the identity/WebVPN flow, while this layer
        // can retain only the boolean fact that some response updated it.
        let client = Client::builder()
            .cookie_provider(Arc::clone(self.transport.cookie_jar()))
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .timeout(self.transport.timeout())
            .user_agent("THYou/identity")
            .http1_only()
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| IdentityExecutionError::RequestBuild)?;

        let mut request = request;
        let mut cookie_update = false;

        for hop in 0..=MAX_IDENTITY_REDIRECT_HOPS {
            let retry_request = request.try_clone();
            let response = self
                .transport
                .execute_once(&client, request)
                .await
                .map_err(TransportError::Request)?;
            cookie_update |= has_non_empty_cookie_update(response.headers());

            let status = response.status();
            let final_url = response.url().clone();
            let target = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| final_url.join(value).ok());

            if !is_identity_redirect_status(status) {
                let body = crate::telemetry::timing::read_text(response)
                    .await
                    .map_err(TransportError::Decode)?;
                return Ok(IdentityHttpResponse {
                    status,
                    final_url,
                    redirect_location: target,
                    cookie_update,
                    body,
                });
            }

            let Some(target) = target else {
                let body = crate::telemetry::timing::read_text(response)
                    .await
                    .map_err(TransportError::Decode)?;
                return Ok(IdentityHttpResponse {
                    status,
                    final_url,
                    redirect_location: None,
                    cookie_update,
                    body,
                });
            };

            // Match CampusHttpTransport's security boundary: follow only
            // same-origin redirects. A cross-origin Location is returned to
            // identity_client for its explicit service/WebVPN allowlist.
            if !same_identity_origin(&final_url, &target) {
                let body = crate::telemetry::timing::read_text(response)
                    .await
                    .map_err(TransportError::Decode)?;
                return Ok(IdentityHttpResponse {
                    status,
                    final_url,
                    redirect_location: Some(target),
                    cookie_update,
                    body,
                });
            }

            // Keep a valid service ticket even when the identity server puts
            // it on a same-origin redirect.  The executor normally follows
            // same-origin redirects so the cookie jar remains useful, but
            // discarding every intermediate Location loses the only handoff
            // evidence in deployments that redirect through the identity
            // origin before reaching Learn.  Ask the profile classifier about
            // this exact Location and stop only for an explicitly allowlisted
            // ticket-bearing handoff.  Ticketless callbacks continue through
            // the normal chain and arbitrary same-origin URLs are never
            // promoted to authentication evidence.
            if matches!(
                self.client.classify_login_response_with_location(
                    status,
                    &final_url,
                    Some(&target),
                    "",
                ),
                LoginResponseClassification::AuthenticatedHandoff(ref page)
                    if page
                        .anchor_ticket
                        .as_ref()
                        .is_some_and(|anchor| anchor.ticket.is_some())
            ) {
                let body = crate::telemetry::timing::read_text(response)
                    .await
                    .map_err(TransportError::Decode)?;
                return Ok(IdentityHttpResponse {
                    status,
                    final_url,
                    redirect_location: Some(target),
                    cookie_update,
                    body,
                });
            }

            if hop == MAX_IDENTITY_REDIRECT_HOPS {
                return Err(IdentityExecutionError::RedirectChainTooLong);
            }

            // The body is deliberately discarded for an intermediate
            // redirect, but consuming it keeps the local fixture and the
            // deployed HTTP/1.1 connection well behaved.
            crate::telemetry::timing::read_bytes(response)
                .await
                .map_err(TransportError::Decode)?;
            let mut next = retry_request.ok_or(IdentityExecutionError::RequestBuild)?;
            *next.url_mut() = target;
            if status == StatusCode::SEE_OTHER
                || (matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND)
                    && next.method() == Method::POST)
            {
                *next.method_mut() = Method::GET;
                *next.body_mut() = None;
            }
            request = next;
        }

        Err(IdentityExecutionError::RedirectChainTooLong)
    }
}

fn is_identity_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

/// Returns whether a response actually set a usable cookie value.
///
/// `Set-Cookie: name=; Max-Age=0` is a deletion, not evidence that an
/// authenticated session was established. Keep this check deliberately
/// small and boolean-only: the cookie jar still parses and stores the header,
/// but the identity proof boundary must not retain or print its contents.
fn has_non_empty_cookie_update(headers: &reqwest::header::HeaderMap) -> bool {
    headers.get_all(SET_COOKIE).iter().any(|value| {
        let Ok(value) = value.to_str() else {
            return false;
        };
        let first_pair = value.split(';').next().unwrap_or_default().trim();
        let Some((name, cookie_value)) = first_pair.split_once('=') else {
            return false;
        };
        let name = name.trim();
        let cookie_value = cookie_value.trim();
        !name.is_empty()
            && name.chars().all(|character| {
                !character.is_ascii_control()
                    && !character.is_ascii_whitespace()
                    && !matches!(character, '=' | ';' | ',')
            })
            && !cookie_value.is_empty()
            && cookie_value != "\"\""
    })
}

fn same_identity_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
        && left.username().is_empty()
        && left.password().is_none()
        && right.username().is_empty()
        && right.password().is_none()
}

fn retryable_identity_redirect_error(error: &IdentityExecutionError) -> bool {
    let IdentityExecutionError::Transport(transport_error) = error else {
        return false;
    };

    match transport_error {
        TransportError::Request(error) | TransportError::Decode(error) => {
            error.is_timeout() || error.is_connect() || error.is_request()
        }
        TransportError::InvalidUrl(_)
        | TransportError::HttpStatus { .. }
        | TransportError::DecodeBody { .. }
        | TransportError::Jsonp(_) => false,
    }
}

fn diagnostic_url(url: &Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use reqwest::{Url, cookie::CookieStore};

    use super::*;
    use crate::identity::{
        FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthActions,
        SecondAuthProfile,
    };

    fn executor_with_base(base_url: &str) -> IdentityExecutionClient {
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
                Vec::new(),
                SecondAuthActions::new(None, None, None, None),
            ),
        );
        let client = IdentityClient::new(
            crate::identity_client::IdentityClientConfig::new(base_url, profile)
                .expect("identity config"),
        )
        .expect("identity client");
        IdentityExecutionClient::new(client, "THYou/test").expect("executor")
    }

    fn executor() -> IdentityExecutionClient {
        executor_with_base("https://id.example.test/")
    }

    #[test]
    fn backend_repair_service_submission_uses_own_encoding_and_action() {
        use crate::identity_client::LoginSubmissionRequestPlan;
        let mut execution = executor();
        execution
            .set_login_form_encoding(FormEncoding::Multipart)
            .unwrap();
        execution
            .set_login_submit_url(
                &Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle").unwrap(),
            )
            .unwrap();
        let service = execution.service_login_client("service-fixture").unwrap();
        let plan = service.login_submission_request_plan().unwrap();
        assert!(
            matches!(plan, LoginSubmissionRequestPlan::UrlEncoded(_)),
            "fixed service form must not inherit primary multipart encoding"
        );
        assert!(
            matches!(
                execution.client().login_submission_request_plan().unwrap(),
                LoginSubmissionRequestPlan::TrustedDeviceMultipart(_)
            ),
            "service planning must not mutate the primary login configuration"
        );
    }

    #[tokio::test]
    async fn backend_repair_service_trusted_post_is_once_and_credential_free() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 2048];
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some(end) = text.find("\r\n\r\n") {
                    let size: usize = text[..end]
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + size {
                        break;
                    }
                }
                assert!(bytes.len() < 16384);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with("POST /do/off/ui/auth/login/checkSingle HTTP/1.1"));
            assert!(request.contains("i_rememberme"));
            assert!(request.contains("fixture-fingerprint"));
            assert!(!request.contains("i_user"));
            assert!(!request.contains("i_pass"));
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            listener
        });
        let execution = executor_with_base(&base);
        let page = IdentityHttpResponse::from_parts(
            StatusCode::OK,
            execution.client().login_page_url().unwrap(),
            r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"></form>"#,
        );
        let response = execution
            .continue_service_check_single(&page, "fixture-fingerprint")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        assert!(!execution.client().is_check_single_login_submission());
    }

    #[test]
    fn response_debug_redacts_html_and_query_values() {
        let response = IdentityHttpResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://id.example.test/login?ticket=secret-ticket&next=/home")
                .expect("URL"),
            redirect_location: None,
            cookie_update: false,
            body: "password=secret-password&csrf=secret-csrf".to_owned(),
        };
        let debug = format!("{response:?}");
        assert!(!debug.contains("secret-password"));
        assert!(!debug.contains("secret-csrf"));
        assert!(!debug.contains("secret-ticket"));
        assert!(debug.contains("body_len"));
    }

    #[test]
    fn resetting_transport_discards_the_previous_cookie_jar() {
        let mut executor = executor();
        let url = Url::parse("https://id.example.test/").expect("URL");
        executor
            .transport()
            .cookie_jar()
            .add_cookie_str("identity-session=old-account; Path=/", &url);
        assert!(executor.transport().cookie_jar().cookies(&url).is_some());

        executor
            .reset_transport("THYou/test")
            .expect("fresh transport");

        assert!(executor.transport().cookie_jar().cookies(&url).is_none());
    }

    #[test]
    fn classifier_keeps_http_200_login_pages_out_of_success_claims() {
        let executor = executor();
        let response = IdentityHttpResponse {
            status: StatusCode::OK,
            final_url: Url::parse("https://id.example.test/login").expect("URL"),
            redirect_location: None,
            cookie_update: false,
            body: r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#.to_owned(),
        };
        let classification = executor.classify_response(&response);
        assert!(matches!(
            classification,
            LoginResponseClassification::LoginPage(_)
        ));
        assert!(!response.is_success() || response.status == StatusCode::OK);
    }

    #[tokio::test]
    async fn preserves_an_allowlisted_ticket_on_a_same_origin_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("login request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read login request");
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: /b/learn?ticket=SAME_ORIGIN_TICKET\r\n",
                "Content-Length: 0\r\n",
                "Connection: close\r\n",
                "\r\n"
            );
            stream
                .write_all(response.as_bytes())
                .expect("write redirect");
        });

        let base_url = format!("http://{address}/");
        let executor = executor_with_base(&base_url);
        let response = executor
            .fetch_login_page()
            .await
            .expect("same-origin redirect response");
        let classification = executor.classify_response(&response);
        let LoginResponseClassification::AuthenticatedHandoff(page) = classification else {
            panic!("same-origin ticket redirect must remain handoff evidence");
        };
        assert_eq!(
            page.anchor_ticket
                .and_then(|anchor| anchor.ticket)
                .as_deref(),
            Some("SAME_ORIGIN_TICKET")
        );
        assert_eq!(
            response
                .redirect_location
                .as_ref()
                .and_then(|url| url.query_pairs().find(|(name, _)| name == "ticket"))
                .map(|(_, value)| value.into_owned())
                .as_deref(),
            Some("SAME_ORIGIN_TICKET")
        );
        server.join().expect("server");
    }

    #[test]
    fn empty_or_malformed_set_cookie_is_not_identity_cookie_proof() {
        use reqwest::header::{HeaderMap, HeaderValue};

        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_static("SESSION=; Path=/"));
        assert!(!has_non_empty_cookie_update(&headers));

        headers.clear();
        headers.append(SET_COOKIE, HeaderValue::from_static("=value; Path=/"));
        assert!(!has_non_empty_cookie_update(&headers));

        headers.clear();
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("SESSION=fixture-session; Path=/"),
        );
        assert!(has_non_empty_cookie_update(&headers));
    }
}
