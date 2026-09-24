//! USEREG login adapter based on the current public client protocol.
//!
//! The portal does not accept the identity-session password verbatim.  The
//! login page supplies two CSRF values and an RSA public key.  The adapter
//! keeps that page material and the encrypted password inside Rust, then
//! creates the following request sequence:
//!
//! 1. `GET /login`;
//! 2. `POST /site/validate-user` with the meta CSRF header and RSA wire
//!    password;
//! 3. `POST /login` with the hidden form CSRF, the same encrypted password,
//!    captcha, and optional SMS code.
//!
//! The protocol facts are independently implemented from public clients.  A
//! successful HTTP status or a JSON `success` field is never treated as an
//! authenticated session without the corresponding response evidence.

use std::{fmt, net::IpAddr};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs1::DecodeRsaPublicKey, pkcs8::DecodePublicKey};
use sha1::{Digest, Sha1};
use thiserror::Error;

use crate::transport::CampusHttpTransport;
use crate::usereg::{
    UseregCsrfToken, UseregDeviceTarget, UseregError, UseregHttpMethod, UseregLoginCredentials,
    UseregOperation, UseregRequestPlan,
};
use crate::usereg_client::{
    UseregClient, UseregClientError, UseregExecutionPlan, UseregHtmlSignal, UseregHttpResponse,
    UseregPageClassification,
};

const MAX_CAPTCHA_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTCHA_REFRESH_BYTES: usize = 32 * 1024;
const MAX_LOGIN_PAGE_BYTES: usize = 512 * 1024;
const MAX_VALIDATION_BYTES: usize = 32 * 1024;

/// Errors produced by the USEREG protocol adapter.  Error values never carry
/// password, captcha, CSRF, cookie, public-key response body, or image data.
#[derive(Debug, Error)]
pub enum UseregAdapterError {
    #[error(transparent)]
    Client(#[from] UseregClientError),

    #[error(transparent)]
    Profile(#[from] UseregError),

    #[error("USEREG login page is too large")]
    LoginPageTooLarge,

    #[error("USEREG authenticated service response was not HTML")]
    InvalidHtmlResponse,

    #[error("USEREG login page response did not remain on the login route")]
    InvalidLoginPageResponse,

    #[error("USEREG response moved to an unexpected service route")]
    RouteChanged,

    #[error("USEREG login page is missing the meta CSRF token")]
    MissingHeaderCsrf,

    #[error("USEREG login page is missing the hidden form CSRF token")]
    MissingFormCsrf,

    #[error("USEREG login page is missing the RSA public key")]
    MissingPublicKey,

    #[error("USEREG login page contains an invalid CSRF token")]
    InvalidHeaderCsrf,

    #[error("USEREG login page contains an unsupported RSA public key")]
    InvalidPublicKey,

    #[error("USEREG password could not be encrypted")]
    PasswordEncryption,

    #[error("USEREG validation response is not valid JSON")]
    InvalidValidationResponse,

    #[error("USEREG validation response did not have a JSON content type")]
    InvalidValidationContentType,

    #[error("USEREG validation response has no boolean success field")]
    MissingValidationSuccess,

    #[error("USEREG captcha refresh response is not the expected JSON object")]
    InvalidCaptchaRefreshResponse,

    #[error("USEREG captcha response is too large")]
    CaptchaTooLarge,

    #[error("USEREG captcha response is not an image")]
    InvalidCaptchaContentType,

    #[error("USEREG response body is not valid UTF-8")]
    InvalidResponseText,

    #[error("USEREG login requires an accepted user validation result")]
    ValidationRequired,

    #[error("USEREG home page did not prove an authenticated session")]
    SessionNotAuthenticated,

    #[error("USEREG service session is missing")]
    SessionMissing,

    #[error("USEREG service session has expired")]
    SessionExpired,

    #[error("USEREG returned an account different from the authenticated service account")]
    AccountMismatch,

    #[error("USEREG home page did not contain a complete authenticated data layout")]
    HomeDataUnavailable,

    #[error("USEREG home page did not contain a valid online-device table")]
    DeviceDataUnavailable,

    #[error("USEREG target device was not present in the authenticated device table")]
    DeviceNotFound,

    #[error("USEREG home page did not contain the expected account data")]
    AccountDataUnavailable,

    #[error("USEREG home page did not contain the expected balance data")]
    BalanceDataUnavailable,

    #[error("USEREG allowed-device response did not contain a valid count")]
    AllowedDeviceDataUnavailable,

    #[error("USEREG device disconnect was not confirmed by the portal")]
    DisconnectNotConfirmed,

    #[error("USEREG certification response was not confirmed by the portal")]
    CertificationNotConfirmed,
}

/// Stable error classes for service actions.  In particular, a missing or
/// expired USEREG session is not interchangeable with a campus-network
/// outage, an HTML login shell, malformed JSON, or a portal business reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregAdapterErrorClass {
    Configuration,
    Transport,
    Http,
    Html,
    Json,
    SessionMissing,
    SessionExpired,
    Business,
    Proof,
}

impl UseregAdapterError {
    /// Source-owned diagnostic codes only. Never copy a response, URL,
    /// account, CSRF token or server error message into logs or reports.
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Client(error) => match error {
                UseregClientError::HttpStatus { status, .. } => match status.as_u16() {
                    401 | 403 => "usereg_http_auth_rejected",
                    429 => "usereg_rate_limited",
                    500..=599 => "usereg_http_unavailable",
                    _ => "usereg_http_rejected",
                },
                _ => match error.class() {
                    crate::usereg_client::UseregClientErrorClass::Configuration => "usereg_config",
                    crate::usereg_client::UseregClientErrorClass::Transport => "usereg_transport",
                    crate::usereg_client::UseregClientErrorClass::Http => "usereg_http_rejected",
                    crate::usereg_client::UseregClientErrorClass::Session => {
                        "usereg_session_expired"
                    }
                    crate::usereg_client::UseregClientErrorClass::Response => {
                        "usereg_response_invalid"
                    }
                },
            },
            Self::InvalidValidationResponse => "usereg_validation_response_invalid",
            Self::InvalidValidationContentType => "usereg_validation_content_type",
            Self::MissingValidationSuccess => "usereg_validation_success_missing",
            Self::InvalidCaptchaRefreshResponse => "usereg_captcha_metadata_invalid",
            Self::InvalidCaptchaContentType => "usereg_captcha_image_invalid",
            Self::CaptchaTooLarge => "usereg_captcha_too_large",
            Self::MissingHeaderCsrf | Self::InvalidHeaderCsrf => "usereg_header_csrf_invalid",
            Self::MissingFormCsrf => "usereg_form_csrf_missing",
            Self::MissingPublicKey | Self::InvalidPublicKey => "usereg_public_key_invalid",
            Self::PasswordEncryption => "usereg_password_encryption",
            Self::InvalidLoginPageResponse | Self::LoginPageTooLarge => "usereg_login_page_invalid",
            Self::RouteChanged => "usereg_route_changed",
            Self::InvalidHtmlResponse | Self::InvalidResponseText => "usereg_response_invalid",
            Self::SessionExpired | Self::SessionMissing => "usereg_session_expired",
            Self::SessionNotAuthenticated | Self::ValidationRequired => "usereg_session_unproven",
            Self::AccountMismatch => "usereg_account_mismatch",
            Self::HomeDataUnavailable => "usereg_home_layout_invalid",
            Self::DeviceDataUnavailable | Self::DeviceNotFound => "usereg_devices_invalid",
            Self::AccountDataUnavailable => "usereg_account_invalid",
            Self::BalanceDataUnavailable => "usereg_balance_invalid",
            Self::AllowedDeviceDataUnavailable => "usereg_device_limit_invalid",
            Self::DisconnectNotConfirmed | Self::CertificationNotConfirmed => {
                "usereg_action_unconfirmed"
            }
            Self::Profile(_) => "usereg_config",
        }
    }

    pub fn class(&self) -> UseregAdapterErrorClass {
        match self {
            Self::Client(error) => match error.class() {
                crate::usereg_client::UseregClientErrorClass::Configuration => {
                    UseregAdapterErrorClass::Configuration
                }
                crate::usereg_client::UseregClientErrorClass::Transport => {
                    UseregAdapterErrorClass::Transport
                }
                crate::usereg_client::UseregClientErrorClass::Http => UseregAdapterErrorClass::Http,
                crate::usereg_client::UseregClientErrorClass::Session => {
                    UseregAdapterErrorClass::SessionExpired
                }
                crate::usereg_client::UseregClientErrorClass::Response => {
                    UseregAdapterErrorClass::Html
                }
            },
            Self::Profile(_) | Self::PasswordEncryption => UseregAdapterErrorClass::Configuration,
            Self::InvalidValidationResponse
            | Self::InvalidValidationContentType
            | Self::MissingValidationSuccess
            | Self::InvalidCaptchaRefreshResponse => UseregAdapterErrorClass::Json,
            Self::InvalidLoginPageResponse
            | Self::InvalidHtmlResponse
            | Self::RouteChanged
            | Self::MissingHeaderCsrf
            | Self::MissingFormCsrf
            | Self::MissingPublicKey
            | Self::InvalidHeaderCsrf
            | Self::InvalidPublicKey
            | Self::InvalidCaptchaContentType
            | Self::InvalidResponseText
            | Self::LoginPageTooLarge => UseregAdapterErrorClass::Html,
            Self::ValidationRequired => UseregAdapterErrorClass::Proof,
            Self::SessionNotAuthenticated | Self::SessionMissing => {
                UseregAdapterErrorClass::SessionMissing
            }
            Self::SessionExpired | Self::AccountMismatch => UseregAdapterErrorClass::SessionExpired,
            Self::HomeDataUnavailable
            | Self::DeviceDataUnavailable
            | Self::DeviceNotFound
            | Self::AccountDataUnavailable
            | Self::BalanceDataUnavailable
            | Self::AllowedDeviceDataUnavailable => UseregAdapterErrorClass::Business,
            Self::CaptchaTooLarge
            | Self::DisconnectNotConfirmed
            | Self::CertificationNotConfirmed => UseregAdapterErrorClass::Proof,
        }
    }

    pub fn is_session_failure(&self) -> bool {
        matches!(
            self.class(),
            UseregAdapterErrorClass::SessionMissing | UseregAdapterErrorClass::SessionExpired
        )
    }
}

/// Safe state returned to callers after the AJAX validation request.  It does
/// not retain the server message because old deployments occasionally echo
/// request data in that field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregValidationOutcome {
    Accepted,
    CaptchaRejected,
    CredentialsRejected,
    SessionExpired,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregValidationResult {
    pub outcome: UseregValidationOutcome,
    encrypted_password: Option<EncryptedPassword>,
    binding: Option<ValidationBinding>,
}

/// A validation response is only usable with the exact login page, account,
/// captcha, and encrypted password that produced it.  The binding is a
/// one-way digest so a public result cannot expose any of those values in
/// debug output or through accidental serialization.
#[derive(Clone, PartialEq, Eq)]
struct ValidationBinding([u8; 20]);

impl fmt::Debug for ValidationBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

/// Safe status for the final form response.  `Authenticated` is emitted only
/// when the response contains a known home-page marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregAuthenticationState {
    Authenticated,
    LoginRequired,
    LoginFailed,
    NonSuccess,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UseregAuthenticationResult {
    pub state: UseregAuthenticationState,
    /// The independent account page has already matched the submitted name.
    /// This is only set after a read through the same transport/Cookie jar.
    pub(crate) account_bound: bool,
}

/// A device row returned by the authenticated USEREG home page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregDevice {
    pub id: String,
    pub ip4: String,
    pub ip6: String,
    pub logged_at: String,
    pub mac: String,
    pub auth_permission: String,
}

/// The balance row returned by the authenticated USEREG home page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregBalance {
    pub product_name: String,
    pub used_bytes: String,
    pub used_seconds: String,
    pub account_balance: String,
    pub settlement_date: String,
}

/// Account details returned by the authenticated USEREG user-info page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregAccountInfo {
    pub username: String,
    pub contact_email: String,
    pub contact_phone: String,
    pub contact_landline: String,
    pub real_name: String,
    pub status: String,
    pub user_group: String,
    pub location: String,
    pub allowed_devices: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UseregDisconnectResult {
    pub confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregCertificationState {
    Confirmed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UseregCertificationResult {
    pub state: UseregCertificationState,
}

/// Metadata for a captcha kept by Rust.  The binary image itself is private
/// to the response/transport layer and is deliberately absent from this DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregCaptchaMetadata {
    pub content_type: Option<String>,
    pub byte_len: usize,
}

/// A validated captcha image which is safe to return to a caller that needs
/// to render it. Its custom `Debug` implementation reports only the byte
/// length, so logging this DTO cannot dump the image contents.
#[derive(Clone, PartialEq, Eq)]
pub struct UseregCaptchaImage {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for UseregCaptchaImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregCaptchaImage")
            .field("content_type", &self.content_type)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

/// Parsed login-page material.  Sensitive values are private and its Debug
/// representation redacts all three values.
#[derive(Clone, PartialEq, Eq)]
pub struct UseregLoginPage {
    form_csrf: UseregCsrfToken,
    header_csrf: String,
    public_key_pem: String,
}

impl fmt::Debug for UseregLoginPage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregLoginPage")
            .field("form_csrf", &"[redacted]")
            .field("header_csrf", &"[redacted]")
            .field("public_key_pem", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct EncryptedPassword(String);

impl fmt::Debug for EncryptedPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("EncryptedPassword")
            .field(&"[redacted]")
            .finish()
    }
}

/// Rust-only adapter for the USEREG wire sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregAdapter {
    client: UseregClient,
}

impl UseregAdapter {
    pub fn new(client: UseregClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &UseregClient {
        &self.client
    }

    pub fn login_page_request_plan(&self) -> Result<UseregExecutionPlan, UseregAdapterError> {
        self.client
            .login_page_request_plan()
            .map_err(UseregAdapterError::from)
    }

    pub fn captcha_refresh_request_plan(&self) -> Result<UseregExecutionPlan, UseregAdapterError> {
        self.client
            .captcha_request_plan(true)
            .map_err(UseregAdapterError::from)
    }

    pub fn captcha_image_request_plan(
        &self,
        cache_buster: &str,
    ) -> Result<UseregExecutionPlan, UseregAdapterError> {
        self.client
            .resolve_request(self.client.profile().captcha_image_request(cache_buster)?)
            .map_err(UseregAdapterError::from)
    }

    /// Performs the login-page GET while retaining the shared cookie jar in
    /// the supplied transport. Only parsed login material crosses this Rust
    /// module boundary; the HTML body is discarded after parsing.
    pub async fn fetch_login_page(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<UseregLoginPage, UseregAdapterError> {
        let plan = self.login_page_request_plan()?;
        let response = self.client.execute_success(transport, &plan).await?;
        if !self.client.response_matches_plan(&response, &plan) {
            return Err(UseregAdapterError::InvalidLoginPageResponse);
        }
        if response
            .content_type
            .as_deref()
            .is_none_or(|value| !is_html_content_type(value))
        {
            return Err(UseregAdapterError::InvalidLoginPageResponse);
        }
        let body = response
            .body_text()
            .map_err(|_| UseregAdapterError::InvalidResponseText)?
            .to_owned();
        self.parse_login_page(&body)
    }

    /// Follow the reference login ordering in one shared cookie context:
    /// refresh/image, then the form that provides the current CSRF and key.
    /// This prepares human input; it never submits credentials.
    pub async fn prepare_captcha_login(
        &self,
        transport: &CampusHttpTransport,
        cache_buster: &str,
    ) -> Result<(UseregLoginPage, UseregCaptchaImage), UseregAdapterError> {
        let image = self.refresh_captcha_image(transport, cache_buster).await?;
        let page = self.fetch_login_page(transport).await?;
        Ok((page, image))
    }

    /// Performs the public two-step captcha refresh and returns the validated
    /// image for rendering. The image is retained only in this return value;
    /// it is never placed in an error or debug representation.
    pub async fn refresh_captcha_image(
        &self,
        transport: &CampusHttpTransport,
        cache_buster: &str,
    ) -> Result<UseregCaptchaImage, UseregAdapterError> {
        let response = self.fetch_captcha_response(transport, cache_buster).await?;
        self.parse_captcha_image(&response)
    }

    /// Performs the public two-step captcha refresh. This legacy metadata
    /// method remains available and shares the same response validation as
    /// [`Self::refresh_captcha_image`].
    pub async fn refresh_captcha(
        &self,
        transport: &CampusHttpTransport,
        cache_buster: &str,
    ) -> Result<UseregCaptchaMetadata, UseregAdapterError> {
        let response = self.fetch_captcha_response(transport, cache_buster).await?;
        self.parse_captcha_metadata(&response)
    }

    async fn fetch_captcha_response(
        &self,
        transport: &CampusHttpTransport,
        cache_buster: &str,
    ) -> Result<UseregHttpResponse, UseregAdapterError> {
        let refresh = self.captcha_refresh_request_plan()?;
        let refresh_response = self.client.execute_success(transport, &refresh).await?;
        if !self
            .client
            .response_matches_plan(&refresh_response, &refresh)
        {
            return Err(UseregAdapterError::InvalidCaptchaRefreshResponse);
        }
        self.parse_captcha_refresh_response(&refresh_response)?;
        let image = self.captcha_image_request_plan(cache_buster)?;
        let image_response = self
            .client
            .execute_success(transport, &image)
            .await
            .map_err(UseregAdapterError::from)?;
        if !self.client.response_matches_plan(&image_response, &image) {
            return Err(UseregAdapterError::InvalidCaptchaContentType);
        }
        Ok(image_response)
    }

    /// Parses only the login-page values needed for the next two requests.
    pub fn parse_login_page(&self, html: &str) -> Result<UseregLoginPage, UseregAdapterError> {
        if html.len() > MAX_LOGIN_PAGE_BYTES {
            return Err(UseregAdapterError::LoginPageTooLarge);
        }
        let form_csrf = self
            .client
            .extract_csrf(html)
            .map_err(|error| match error {
                UseregClientError::Csrf(_) => UseregAdapterError::MissingFormCsrf,
                other => UseregAdapterError::Client(other),
            })?
            .token;
        let header_csrf =
            find_meta_content(html, "csrf-token")?.ok_or(UseregAdapterError::MissingHeaderCsrf)?;
        if !valid_token(&header_csrf) {
            return Err(UseregAdapterError::InvalidHeaderCsrf);
        }
        let public_key_pem =
            find_element_value(html, "public").ok_or(UseregAdapterError::MissingPublicKey)?;
        let public_key_pem = html_unescape(&public_key_pem);
        if !looks_like_rsa_pem(&public_key_pem) {
            return Err(UseregAdapterError::InvalidPublicKey);
        }
        Ok(UseregLoginPage {
            form_csrf,
            header_csrf,
            public_key_pem,
        })
    }

    /// Creates the AJAX validation request with the RSA-encrypted password.
    pub fn validate_user_request_plan(
        &self,
        page: &UseregLoginPage,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
    ) -> Result<UseregExecutionPlan, UseregAdapterError> {
        let encrypted = encrypt_password(page, credentials)?;
        self.validate_user_request_plan_with_password(page, credentials, verify_code, &encrypted)
    }

    fn validate_user_request_plan_with_password(
        &self,
        page: &UseregLoginPage,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
        encrypted: &EncryptedPassword,
    ) -> Result<UseregExecutionPlan, UseregAdapterError> {
        let request = self.client.profile().validate_user_request_with_password(
            &page.header_csrf,
            &credentials.username,
            encrypted.0.as_str(),
            verify_code,
        )?;
        self.client.resolve_request(request).map_err(Into::into)
    }

    /// Creates the final form request.  The caller must only invoke this
    /// after the validation result is `Accepted`.
    pub fn login_request_plan(
        &self,
        page: &UseregLoginPage,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
        sms_code: Option<&str>,
    ) -> Result<UseregExecutionPlan, UseregAdapterError> {
        let encrypted = encrypt_password(page, credentials)?;
        self.login_request_plan_with_password(page, credentials, verify_code, sms_code, &encrypted)
    }

    fn login_request_plan_with_password(
        &self,
        page: &UseregLoginPage,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
        sms_code: Option<&str>,
        encrypted: &EncryptedPassword,
    ) -> Result<UseregExecutionPlan, UseregAdapterError> {
        let request = self.client.profile().login_request_with_password(
            &page.form_csrf,
            &credentials.username,
            encrypted.0.as_str(),
            verify_code,
            sms_code,
        )?;
        self.client.resolve_request(request).map_err(Into::into)
    }

    pub async fn validate_user(
        &self,
        transport: &CampusHttpTransport,
        page: &UseregLoginPage,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
    ) -> Result<UseregValidationResult, UseregAdapterError> {
        let encrypted = encrypt_password(page, credentials)?;
        let plan = self.validate_user_request_plan_with_password(
            page,
            credentials,
            verify_code,
            &encrypted,
        )?;
        let response = self.client.execute(transport, &plan).await?;
        if !self.client.response_matches_plan(&response, &plan) {
            return Err(UseregAdapterError::InvalidValidationResponse);
        }
        let mut result = self.parse_validation_response(&response)?;
        if result.outcome == UseregValidationOutcome::Accepted {
            result.binding = Some(validation_binding(
                page,
                credentials,
                verify_code,
                &encrypted,
            ));
            result.encrypted_password = Some(encrypted);
        }
        Ok(result)
    }

    /// Submits the final login form only after the preceding AJAX validation
    /// has returned `Accepted`.
    pub async fn login_after_validation(
        &self,
        transport: &CampusHttpTransport,
        page: &UseregLoginPage,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
        sms_code: Option<&str>,
        validation: UseregValidationResult,
    ) -> Result<UseregAuthenticationResult, UseregAdapterError> {
        if validation.outcome != UseregValidationOutcome::Accepted {
            return Err(UseregAdapterError::ValidationRequired);
        }
        let encrypted = validation
            .encrypted_password
            .as_ref()
            .ok_or(UseregAdapterError::ValidationRequired)?;
        let expected_binding = validation_binding(page, credentials, verify_code, encrypted);
        if validation.binding.as_ref() != Some(&expected_binding) {
            return Err(UseregAdapterError::ValidationRequired);
        }
        let plan = self.login_request_plan_with_password(
            page,
            credentials,
            verify_code,
            sms_code,
            encrypted,
        )?;
        let response = self.client.execute(transport, &plan).await?;
        let result = self.classify_login_response(&response)?;
        let url_only_login_hint = matches!(
            self.client.map_response(&response)?.classification,
            UseregPageClassification::LoginRequired(evidence)
                if evidence.signals.as_slice() == [UseregHtmlSignal::FinalUrlMatchesLogin]
        );
        if matches!(
            result.state,
            UseregAuthenticationState::LoginFailed | UseregAuthenticationState::NonSuccess
        ) || (result.state == UseregAuthenticationState::LoginRequired && !url_only_login_hint)
        {
            return Ok(result);
        }
        if result.state == UseregAuthenticationState::Unknown
            || (result.state == UseregAuthenticationState::LoginRequired && url_only_login_hint)
        {
            // Current deployments can render an authenticated `/home` shell
            // without every business table. A final POST/redirect alone does
            // not prove login; the exact `/users` account page does.
            if !response.status.is_success()
                || !response
                    .content_type
                    .as_deref()
                    .is_some_and(is_html_content_type)
                || !(self
                    .client
                    .response_matches_profile_route(&response, &self.client.profile().paths.home)
                    || self.client.response_matches_profile_route(
                        &response,
                        &self.client.profile().paths.login,
                    ))
            {
                return Err(UseregAdapterError::SessionNotAuthenticated);
            }
            self.confirm_account_username(transport, &credentials.username)
                .await?;
            return Ok(UseregAuthenticationResult {
                state: UseregAuthenticationState::Authenticated,
                account_bound: true,
            });
        }
        // A complete final home still gets the existing second read. If that
        // home has become sparse, the account page can prove the same cookie
        // session without requiring the unrelated business tables.
        let proof = self.probe_session(transport).await?;
        if proof.state == UseregAuthenticationState::Unknown {
            self.confirm_account_username(transport, &credentials.username)
                .await?;
            return Ok(UseregAuthenticationResult {
                state: UseregAuthenticationState::Authenticated,
                account_bound: true,
            });
        }
        if proof.state != UseregAuthenticationState::Authenticated {
            return Err(UseregAdapterError::SessionNotAuthenticated);
        }
        Ok(proof)
    }

    pub fn parse_validation_response(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<UseregValidationResult, UseregAdapterError> {
        self.client.ensure_response_origin(response)?;
        response.ensure_success()?;
        if !self
            .client
            .response_matches_profile_route(response, &self.client.profile().paths.validate_user)
        {
            return Err(UseregAdapterError::RouteChanged);
        }
        if response.content_type.as_deref().is_none_or(|value| {
            let mime = value.split(';').next().unwrap_or_default().trim();
            !is_json_content_type(value)
                && !mime.eq_ignore_ascii_case("text/html")
                && !mime.eq_ignore_ascii_case("text/plain")
        }) {
            return Err(UseregAdapterError::InvalidValidationContentType);
        }
        // The deployed Yii validation action may return JSON as text/html.
        // Like the reference client's response.json(), parse the BODY, but
        // only on the exact validation route, within a small byte budget,
        // and with the same mandatory boolean envelope. HTML login pages,
        // redirects and empty/ambiguous bodies still cannot authorize login.
        if response.body().is_empty() || response.body().len() > MAX_VALIDATION_BYTES {
            return Err(UseregAdapterError::InvalidValidationResponse);
        }
        let value: serde_json::Value = serde_json::from_slice(response.body())
            .map_err(|_| UseregAdapterError::InvalidValidationResponse)?;
        let success = value
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .ok_or(UseregAdapterError::MissingValidationSuccess)?;
        if success {
            return Ok(UseregValidationResult {
                outcome: UseregValidationOutcome::Accepted,
                encrypted_password: None,
                binding: None,
            });
        }
        let message = value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let outcome =
            if message.contains("失效") || message.contains("过期") || message.contains("expired")
            {
                UseregValidationOutcome::SessionExpired
            } else if message.contains("验证码") || message.contains("verify") {
                UseregValidationOutcome::CaptchaRejected
            } else if message.contains("密码")
                || message.contains("用户名")
                || message.contains("credential")
            {
                UseregValidationOutcome::CredentialsRejected
            } else {
                UseregValidationOutcome::Rejected
            };
        Ok(UseregValidationResult {
            outcome,
            encrypted_password: None,
            binding: None,
        })
    }

    /// The refresh endpoint returns JSON metadata before the image request.
    /// A successful HTTP status alone is not enough: the portal must return
    /// the two integer hash values and a non-empty image URL.  The values are
    /// intentionally validated and then discarded because the public client
    /// obtains the image through the cache-busted request below.
    fn parse_captcha_refresh_response(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<(), UseregAdapterError> {
        self.client.ensure_response_origin(response)?;
        response.ensure_success()?;
        if response.body().is_empty() || response.body().len() > MAX_CAPTCHA_REFRESH_BYTES {
            return Err(UseregAdapterError::InvalidCaptchaRefreshResponse);
        }
        if !response
            .content_type
            .as_deref()
            .is_some_and(is_json_content_type)
        {
            return Err(UseregAdapterError::InvalidCaptchaRefreshResponse);
        }

        let value: serde_json::Value = serde_json::from_slice(response.body())
            .map_err(|_| UseregAdapterError::InvalidCaptchaRefreshResponse)?;
        let object = value
            .as_object()
            .ok_or(UseregAdapterError::InvalidCaptchaRefreshResponse)?;
        let has_integer = |name: &str| {
            object
                .get(name)
                .is_some_and(|value| value.as_u64().is_some())
        };
        let url = object
            .get("url")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or(UseregAdapterError::InvalidCaptchaRefreshResponse)?;
        if !has_integer("hash1")
            || !has_integer("hash2")
            || !is_captcha_metadata_url(url, &self.client.profile().paths.captcha)
        {
            return Err(UseregAdapterError::InvalidCaptchaRefreshResponse);
        }
        Ok(())
    }

    pub fn parse_captcha_image(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<UseregCaptchaImage, UseregAdapterError> {
        let content_type = self.validate_captcha_response(response)?.to_owned();
        Ok(UseregCaptchaImage {
            content_type,
            bytes: response.body().to_vec(),
        })
    }

    pub fn parse_captcha_metadata(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<UseregCaptchaMetadata, UseregAdapterError> {
        let content_type = self.validate_captcha_response(response)?.to_owned();
        Ok(UseregCaptchaMetadata {
            content_type: Some(content_type),
            byte_len: response.body().len(),
        })
    }

    fn validate_captcha_response<'a>(
        &self,
        response: &'a UseregHttpResponse,
    ) -> Result<&'a str, UseregAdapterError> {
        self.client.ensure_response_origin(response)?;
        response.ensure_success()?;
        if response.body().len() > MAX_CAPTCHA_BYTES {
            return Err(UseregAdapterError::CaptchaTooLarge);
        }
        if response.body().is_empty() {
            return Err(UseregAdapterError::InvalidCaptchaContentType);
        }
        let content_type = response
            .content_type
            .as_deref()
            .filter(|value| crate::captcha_image::is_bounded_raster(value, response.body()))
            .ok_or(UseregAdapterError::InvalidCaptchaContentType)?;
        Ok(content_type)
    }

    /// Classifies the final login response and requires a known home marker
    /// before returning `Authenticated`.
    pub fn classify_login_response(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<UseregAuthenticationResult, UseregAdapterError> {
        let mapped = self.client.map_response(response)?;
        // A form POST can legitimately finish at `/login` while rendering the
        // authenticated home in deployments that do not redirect.  A strong
        // home+CSRF proof outranks the URL hint; the URL alone must never be
        // used as a success signal.
        let has_blocking_login_signal = match &mapped.classification {
            UseregPageClassification::LoginFailed(_) => true,
            UseregPageClassification::LoginRequired(evidence) => {
                evidence.signals.iter().any(|signal| {
                    matches!(
                        signal,
                        UseregHtmlSignal::LoginForm
                            | UseregHtmlSignal::ExplicitExpiredMarker
                            | UseregHtmlSignal::FailureMarker
                    )
                })
            }
            _ => false,
        };
        if !has_blocking_login_signal
            && response.status.is_success()
            && response
                .content_type
                .as_deref()
                .is_some_and(is_html_content_type)
            && (self
                .client
                .response_matches_profile_route(response, &self.client.profile().paths.home)
                || self
                    .client
                    .response_matches_profile_route(response, &self.client.profile().paths.login))
        {
            let body = response
                .body_text()
                .map_err(|_| UseregAdapterError::InvalidResponseText)?;
            if has_authenticated_home_proof(body, &self.client) {
                return Ok(UseregAuthenticationResult {
                    state: UseregAuthenticationState::Authenticated,
                    account_bound: false,
                });
            }
        }
        let state = match mapped.classification {
            UseregPageClassification::LoginFailed(_) => UseregAuthenticationState::LoginFailed,
            UseregPageClassification::LoginRequired(_) => UseregAuthenticationState::LoginRequired,
            UseregPageClassification::NonSuccess => UseregAuthenticationState::NonSuccess,
            UseregPageClassification::NonHtml => UseregAuthenticationState::Unknown,
            UseregPageClassification::HtmlUnknown(_) => {
                let body = response
                    .body_text()
                    .map_err(|_| UseregAdapterError::InvalidResponseText)?
                    .to_owned();
                if (self
                    .client
                    .response_matches_profile_route(response, &self.client.profile().paths.home)
                    || self.client.response_matches_profile_route(
                        response,
                        &self.client.profile().paths.login,
                    ))
                    && has_authenticated_home_proof(&body, &self.client)
                {
                    UseregAuthenticationState::Authenticated
                } else {
                    UseregAuthenticationState::Unknown
                }
            }
        };
        Ok(UseregAuthenticationResult {
            state,
            account_bound: false,
        })
    }

    /// Creates a read-only home request for session proof after login.
    pub fn home_request_plan(&self) -> Result<UseregExecutionPlan, UseregAdapterError> {
        self.resolve_get(UseregOperation::Home, &self.client.profile().paths.home)
    }

    pub fn users_request_plan(&self) -> Result<UseregExecutionPlan, UseregAdapterError> {
        self.resolve_get(UseregOperation::Users, &self.client.profile().paths.users)
    }

    pub fn online_num_request_plan(&self) -> Result<UseregExecutionPlan, UseregAdapterError> {
        self.resolve_get(
            UseregOperation::OnlineNum,
            &self.client.profile().paths.online_num,
        )
    }

    /// Reads the authenticated home page and returns the page classification.
    /// `Authenticated` is emitted only when the response is a successful HTML
    /// page containing one of the known home containers.
    pub async fn probe_session(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<UseregAuthenticationResult, UseregAdapterError> {
        let response = self.execute_home(transport).await?;
        self.classify_login_response(&response)
    }

    /// Reads the online-device table from the authenticated home page.
    pub async fn read_online_devices(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<Vec<UseregDevice>, UseregAdapterError> {
        let home = self.authenticated_home(transport).await?;
        parse_devices(&home)
    }

    /// Reads the balance table from the authenticated home page. The table is
    /// required; an unrelated HTML page cannot be treated as a zero balance.
    pub async fn read_balance(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<UseregBalance, UseregAdapterError> {
        let home = self.authenticated_home(transport).await?;
        parse_balance(&home).ok_or(UseregAdapterError::BalanceDataUnavailable)
    }

    /// Reads the account page and the allowed-device count after first proving
    /// the authenticated home session.
    pub async fn read_account_info(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<UseregAccountInfo, UseregAdapterError> {
        self.read_account_info_for(transport, None).await
    }

    /// The independent USEREG account is not necessarily the Identity
    /// account. Check it before fetching more account data or returning DTOs.
    pub(crate) async fn read_account_info_for(
        &self,
        transport: &CampusHttpTransport,
        expected_username: Option<&str>,
    ) -> Result<UseregAccountInfo, UseregAdapterError> {
        let home = self.authenticated_home(transport).await?;
        let status = text_near_marker(&home, "glyphicon-info-sign")
            .ok_or(UseregAdapterError::AccountDataUnavailable)?;
        let cells = self.read_account_cells(transport).await?;
        if expected_username.is_some_and(|expected| cells[0] != expected) {
            return Err(UseregAdapterError::AccountMismatch);
        }

        let devices_plan = self.online_num_request_plan()?;
        let devices_response = self
            .client
            .execute_success(transport, &devices_plan)
            .await?;
        self.ensure_authenticated_html_response(&devices_response, &devices_plan)?;
        let devices_body = devices_response
            .body_text()
            .map_err(|_| UseregAdapterError::InvalidResponseText)?;
        let devices_classification = self.client.map_response(&devices_response)?;
        if devices_classification.is_login_required() {
            return Err(self.session_error_for_response(&devices_response)?);
        }
        if !matches!(
            devices_classification.classification,
            UseregPageClassification::HtmlUnknown(_)
        ) {
            return Err(UseregAdapterError::InvalidHtmlResponse);
        }
        let allowed_devices = first_integer_near_marker(devices_body, "glyphicon-exclamation-sign")
            .ok_or(UseregAdapterError::AllowedDeviceDataUnavailable)?;

        Ok(UseregAccountInfo {
            username: cells[0].clone(),
            contact_email: cells[1].clone(),
            contact_phone: cells[2].clone(),
            contact_landline: cells[5].clone(),
            real_name: cells[6].clone(),
            status,
            user_group: cells[7].clone(),
            location: cells[3].clone(),
            allowed_devices,
        })
    }

    pub(crate) async fn confirm_account_username(
        &self,
        transport: &CampusHttpTransport,
        expected: &str,
    ) -> Result<(), UseregAdapterError> {
        let cells = self.read_account_cells(transport).await?;
        if cells[0] != expected {
            return Err(UseregAdapterError::AccountMismatch);
        }
        Ok(())
    }

    async fn read_account_cells(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<Vec<String>, UseregAdapterError> {
        let plan = self.users_request_plan()?;
        let response = self.client.execute_success(transport, &plan).await?;
        self.ensure_authenticated_html_response(&response, &plan)?;
        let mapped = self.client.map_response(&response)?;
        if mapped.is_login_required() {
            return Err(self.session_error_for_response(&response)?);
        }
        if !matches!(
            mapped.classification,
            UseregPageClassification::HtmlUnknown(_)
        ) {
            return Err(UseregAdapterError::InvalidHtmlResponse);
        }
        let body = response
            .body_text()
            .map_err(|_| UseregAdapterError::InvalidResponseText)?;
        let cells = table_body_by_id(body, "w0")
            .map(|body| element_texts(&body, "td"))
            .ok_or(UseregAdapterError::AccountDataUnavailable)?;
        if cells.len() < 8
            || cells[0].trim().is_empty()
            || cells[7].trim().is_empty()
            || cells[0].chars().any(char::is_control)
        {
            return Err(UseregAdapterError::AccountDataUnavailable);
        }
        Ok(cells)
    }

    /// Removes one currently online device. The home page supplies the CSRF
    /// token and the response must contain the portal's explicit success
    /// marker before the operation is reported as complete.
    pub async fn disconnect_device(
        &self,
        transport: &CampusHttpTransport,
        device: &UseregDeviceTarget,
    ) -> Result<UseregDisconnectResult, UseregAdapterError> {
        let (home, csrf) = self.authenticated_home_with_csrf(transport).await?;
        let initial_devices = parse_devices(&home)?;
        if !initial_devices
            .iter()
            .any(|current| current.id == device.id() && current.mac == device.user_mac())
        {
            return Err(UseregAdapterError::DeviceNotFound);
        }
        let plan = self
            .client
            .home_delete_request_plan(&csrf, device)
            .map_err(UseregAdapterError::from)?;
        let response = self.client.execute(transport, &plan).await?;
        self.client.ensure_response_origin(&response)?;
        response.ensure_success()?;
        self.ensure_authenticated_html_response(&response, &plan)?;
        let body = response
            .body_text()
            .map_err(|_| UseregAdapterError::InvalidResponseText)?;
        if has_nonempty_element_id(&body, "w5-success-0") {
            // The banner proves that the POST was accepted.  A second home
            // read is required to prove that the requested row disappeared
            // from the current server state.
            let devices = self.read_online_devices(transport).await?;
            if devices
                .iter()
                .any(|current| current.id == device.id() && current.mac == device.user_mac())
            {
                return Err(UseregAdapterError::DisconnectNotConfirmed);
            }
            Ok(UseregDisconnectResult { confirmed: true })
        } else {
            Err(UseregAdapterError::DisconnectNotConfirmed)
        }
    }

    /// Loads the certification form with the authenticated session and submits
    /// the observed CSRF-bound form. The portal's `w0-success-0` marker only
    /// proves that it accepted the request; a second authenticated `/home`
    /// read must also contain the certified IP before this write is reported
    /// as complete. A generic 2xx page or a success-looking message is never
    /// promoted to a device-certification result.
    pub async fn certify_device(
        &self,
        transport: &CampusHttpTransport,
        input: &crate::usereg::UseregCertificationInput,
    ) -> Result<UseregCertificationResult, UseregAdapterError> {
        let _ = self.authenticated_home(transport).await?;
        let page_plan = self.client.certification_page_request_plan()?;
        let page_response = self.client.execute(transport, &page_plan).await?;
        self.client.ensure_response_origin(&page_response)?;
        page_response.ensure_success()?;
        self.ensure_authenticated_html_response(&page_response, &page_plan)?;
        let page_body = page_response
            .body_text()
            .map_err(|_| UseregAdapterError::InvalidResponseText)?;
        let csrf = self
            .client
            .extract_csrf(page_body)
            .map_err(|_| UseregAdapterError::MissingFormCsrf)?
            .token;
        let plan = self.client.certification_request_plan(&csrf, input)?;
        let response = self.client.execute(transport, &plan).await?;
        self.client.ensure_response_origin(&response)?;
        response.ensure_success()?;
        self.ensure_authenticated_html_response(&response, &plan)?;
        let body = response
            .body_text()
            .map_err(|_| UseregAdapterError::InvalidResponseText)?;
        if has_nonempty_element_id(body, "w0-success-0") {
            // Certification changes server state. Confirm that state through
            // the canonical device table instead of trusting the POST banner
            // or returning success merely because the request was accepted.
            let home = self.authenticated_home(transport).await?;
            let devices = parse_devices(&home)?;
            if !devices.iter().any(|device| {
                device.ip4.trim() == input.ip.trim() || device.ip6.trim() == input.ip.trim()
            }) {
                return Err(UseregAdapterError::CertificationNotConfirmed);
            }
            return Ok(UseregCertificationResult {
                state: UseregCertificationState::Confirmed,
            });
        }
        if has_nonempty_element_id(body, "w0-error-0") {
            return Ok(UseregCertificationResult {
                state: UseregCertificationState::Rejected,
            });
        }
        Err(UseregAdapterError::CertificationNotConfirmed)
    }

    async fn execute_home(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<UseregHttpResponse, UseregAdapterError> {
        let plan = self.home_request_plan()?;
        let response = self
            .client
            .execute(transport, &plan)
            .await
            .map_err(UseregAdapterError::from)?;
        self.client.ensure_response_origin(&response)?;
        response.ensure_success()?;
        if !self.client.response_matches_plan(&response, &plan) {
            let mapped = self.client.map_response(&response)?;
            if mapped.is_login_required() {
                return Err(UseregAdapterError::SessionExpired);
            }
            return Err(UseregAdapterError::RouteChanged);
        }
        Ok(response)
    }

    async fn authenticated_home(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<String, UseregAdapterError> {
        let response = self.execute_home(transport).await?;
        let plan = self.home_request_plan()?;
        self.ensure_authenticated_html_response(&response, &plan)?;
        response
            .body_text()
            .map(str::to_owned)
            .map_err(|_| UseregAdapterError::InvalidResponseText)
    }

    async fn authenticated_home_with_csrf(
        &self,
        transport: &CampusHttpTransport,
    ) -> Result<(String, UseregCsrfToken), UseregAdapterError> {
        let body = self.authenticated_home(transport).await?;
        // Only a state-changing action needs the form token. Read-only home
        // consumers verify their own business data rather than requiring
        // unrelated tables or a hidden input to be present.
        let csrf = self
            .client
            .extract_csrf(&body)
            .map_err(|_| UseregAdapterError::MissingFormCsrf)?
            .token;
        Ok((body, csrf))
    }

    fn session_error_for_response(
        &self,
        response: &UseregHttpResponse,
    ) -> Result<UseregAdapterError, UseregAdapterError> {
        let mapped = self.client.map_response(response)?;
        let error = match mapped.classification {
            UseregPageClassification::LoginRequired(evidence) => {
                // This helper is called only after a service action has
                // already obtained an authenticated home page.  A later
                // login form therefore means that the previously proven
                // cookie session expired, even if the portal rendered the
                // form at the requested route instead of redirecting.
                let _ = evidence;
                UseregAdapterError::SessionExpired
            }
            UseregPageClassification::LoginFailed(_) => UseregAdapterError::SessionExpired,
            UseregPageClassification::HtmlUnknown(_) => UseregAdapterError::HomeDataUnavailable,
            UseregPageClassification::NonHtml => UseregAdapterError::InvalidHtmlResponse,
            // `execute_home` rejects non-success HTTP responses before this
            // helper is reached. Keep this defensive branch secret-safe and
            // preserve the generic session proof failure if a future caller
            // bypasses that execution path.
            UseregPageClassification::NonSuccess => UseregAdapterError::SessionNotAuthenticated,
        };
        Ok(error)
    }

    /// Validate a response made with an already authenticated service session.
    /// A redirect or final URL change is kept distinct from an HTML login page;
    /// the latter means that the previously proven cookie session expired.
    /// Successful service reads also require HTML before their business parser
    /// is allowed to run.
    fn ensure_authenticated_html_response(
        &self,
        response: &UseregHttpResponse,
        plan: &UseregExecutionPlan,
    ) -> Result<(), UseregAdapterError> {
        if !self.client.response_matches_plan(response, plan) {
            let mapped = self.client.map_response(response)?;
            if mapped.is_login_required() {
                return Err(UseregAdapterError::SessionExpired);
            }
            return Err(UseregAdapterError::RouteChanged);
        }

        let mapped = self.client.map_response(response)?;
        match mapped.classification {
            UseregPageClassification::LoginRequired(_)
            | UseregPageClassification::LoginFailed(_) => Err(UseregAdapterError::SessionExpired),
            UseregPageClassification::HtmlUnknown(_) => Ok(()),
            UseregPageClassification::NonHtml | UseregPageClassification::NonSuccess => {
                Err(UseregAdapterError::InvalidHtmlResponse)
            }
        }
    }

    fn resolve_get(
        &self,
        operation: UseregOperation,
        path: &str,
    ) -> Result<UseregExecutionPlan, UseregAdapterError> {
        let request = UseregRequestPlan {
            operation,
            method: UseregHttpMethod::Get,
            path: path.to_owned(),
            query: Vec::new(),
            headers: Vec::new(),
            form: Vec::new(),
        };
        self.client.resolve_request(request).map_err(Into::into)
    }
}

fn has_authenticated_home_proof(html: &str, client: &UseregClient) -> bool {
    client.extract_csrf(html).is_ok()
        && parse_devices(html).is_ok()
        && parse_balance(html).is_some()
}

fn parse_devices(html: &str) -> Result<Vec<UseregDevice>, UseregAdapterError> {
    let container = element_body_by_id(html, "w1-container")
        .ok_or(UseregAdapterError::DeviceDataUnavailable)?;
    if element_blocks(&container, "table").is_empty() {
        return Err(UseregAdapterError::DeviceDataUnavailable);
    }
    let bodies = element_blocks(&container, "tbody");
    if bodies.is_empty() {
        return Err(UseregAdapterError::DeviceDataUnavailable);
    }
    let mut devices = Vec::new();
    for (_, body) in bodies {
        for (opening, row) in element_blocks(&body, "tr") {
            let cells = element_texts(row.as_str(), "td");
            let Some(id) = opening
                .attr("data-key")
                .map(str::trim)
                .filter(|v| !v.is_empty())
            else {
                // Header rows belong in <thead>. A non-empty row in the
                // actual <tbody> without the portal's key is malformed; it
                // must not silently become an empty device result.
                if cells.iter().any(|value| !value.trim().is_empty()) {
                    return Err(UseregAdapterError::DeviceDataUnavailable);
                }
                continue;
            };
            if !id.chars().all(|character| character.is_ascii_digit()) {
                return Err(UseregAdapterError::DeviceDataUnavailable);
            }
            if cells.len() < 5 {
                return Err(UseregAdapterError::DeviceDataUnavailable);
            }
            if cells[2].trim().is_empty()
                || cells[3].trim().is_empty()
                || cells[4].trim().is_empty()
                || !valid_device_address(&cells[0], false)
                || !valid_device_address(&cells[1], true)
            {
                return Err(UseregAdapterError::DeviceDataUnavailable);
            }
            devices.push(UseregDevice {
                id: id.to_owned(),
                ip4: cells[0].clone(),
                ip6: cells[1].clone(),
                logged_at: cells[2].clone(),
                auth_permission: cells[3].clone(),
                mac: cells[4].clone(),
            });
        }
    }
    Ok(devices)
}

fn parse_balance(html: &str) -> Option<UseregBalance> {
    let container = element_body_by_id(html, "w3-container")?;
    if element_blocks(&container, "table").is_empty() {
        return None;
    }
    let bodies = element_blocks(&container, "tbody");
    let (_, row) = bodies
        .iter()
        .flat_map(|(_, body)| element_blocks(body, "tr"))
        .find(|(_, row)| element_blocks(row, "td").len() >= 5)?;
    let td_cells = element_blocks(&row, "td");
    let has_col_seq = td_cells
        .iter()
        .any(|(opening, _)| opening.attr("data-col-seq").is_some());

    let cells = if has_col_seq {
        let mut by_sequence = std::collections::BTreeMap::new();
        for (opening, body) in &td_cells {
            let sequence = opening.attr("data-col-seq")?.trim();
            if sequence.is_empty()
                || !sequence.chars().all(|character| character.is_ascii_digit())
                || by_sequence
                    .insert(sequence.to_owned(), text_content(body))
                    .is_some()
            {
                return None;
            }
        }
        ["1", "3", "4", "7", "10"]
            .into_iter()
            .map(|sequence| by_sequence.get(sequence).cloned())
            .collect::<Option<Vec<_>>>()?
    } else {
        // Older deployments exposed exactly the five semantic columns in
        // order. Keep this observed compatibility shape, but never accept a
        // short row or synthesize missing values.
        if td_cells.len() != 5 {
            return None;
        }
        td_cells
            .iter()
            .map(|(_, body)| text_content(body))
            .collect::<Vec<_>>()
    };

    (cells.len() == 5 && cells.iter().all(|value| !value.trim().is_empty())).then(|| {
        UseregBalance {
            product_name: cells[0].clone(),
            used_bytes: cells[1].clone(),
            used_seconds: cells[2].clone(),
            account_balance: cells[3].clone(),
            settlement_date: cells[4].clone(),
        }
    })
}

fn is_captcha_metadata_url(value: &str, expected_path: &str) -> bool {
    let value = value.trim();
    value.starts_with('/')
        && !value.starts_with("//")
        && !value
            .chars()
            .any(|character| character.is_control() || character == '#')
        && value.split_once('?').map_or(value, |(path, _)| path) == expected_path
}

fn has_nonempty_element_id(html: &str, id: &str) -> bool {
    element_body_by_id(html, id).is_some_and(|body| !text_content(&body).trim().is_empty())
}

fn element_body_by_id<'a>(html: &'a str, id: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    for tag_name in ["div", "section", "main", "table", "tbody"] {
        let mut cursor = 0;
        while let Some((start, open_end)) = find_next_opening_tag(html, &lower, cursor, tag_name) {
            let opening = parse_opening_tag(&html[start + 1..open_end]);
            if opening.as_ref().and_then(|tag| tag.attr("id")) == Some(id)
                && !is_self_closing_tag(&html[start + 1..open_end])
            {
                let body_start = open_end + 1;
                let (close_start, _) = find_matching_close(html, &lower, body_start, tag_name)?;
                return Some(html[body_start..close_start].to_owned());
            }
            // `element_blocks` advances beyond each complete outer element.
            // Dashboard widgets are nested inside several same-name divs, so
            // look at every opening tag before skipping any subtree.
            cursor = open_end + 1;
        }
    }
    None
}

fn table_body_by_id(html: &str, id: &str) -> Option<String> {
    if let Some((_, body)) = element_blocks(html, "table")
        .into_iter()
        .find(|(opening, _)| opening.attr("id") == Some(id))
    {
        return Some(body);
    }

    for tag_name in ["div", "section", "main"] {
        for (opening, body) in element_blocks(html, tag_name) {
            if opening.attr("id") != Some(id) {
                continue;
            }
            return element_blocks(&body, "table")
                .into_iter()
                .next()
                .map(|(_, table_body)| table_body);
        }
    }
    None
}

fn element_texts(html: &str, tag_name: &str) -> Vec<String> {
    element_blocks(html, tag_name)
        .into_iter()
        .map(|(_, body)| text_content(&body))
        .collect()
}

fn element_blocks(html: &str, tag_name: &str) -> Vec<(OpeningTag, String)> {
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0;
    let mut blocks = Vec::new();
    while let Some((start, open_end)) = find_next_opening_tag(html, &lower, cursor, tag_name) {
        let Some(opening) = parse_opening_tag(&html[start + 1..open_end]) else {
            cursor = open_end + 1;
            continue;
        };
        if is_self_closing_tag(&html[start + 1..open_end]) {
            cursor = open_end + 1;
            continue;
        }
        let body_start = open_end + 1;
        let Some((close_start, close_end)) =
            find_matching_close(html, &lower, body_start, tag_name)
        else {
            break;
        };
        blocks.push((opening, html[body_start..close_start].to_owned()));
        cursor = close_end + 1;
    }
    blocks
}

fn find_next_opening_tag(
    html: &str,
    lower: &str,
    mut cursor: usize,
    tag_name: &str,
) -> Option<(usize, usize)> {
    while let Some(relative) = lower[cursor..].find('<') {
        let start = cursor + relative;
        if let Some(next) = skip_ignored_block(html, lower, start, tag_name) {
            cursor = next;
            continue;
        }
        if matches_tag_at(lower, start, tag_name, false) {
            let open_end = find_tag_end(html, start)?;
            return Some((start, open_end));
        }
        cursor = start + 1;
    }
    None
}

fn find_matching_close(
    html: &str,
    lower: &str,
    mut cursor: usize,
    tag_name: &str,
) -> Option<(usize, usize)> {
    let mut depth = 1;
    while let Some(relative) = lower[cursor..].find('<') {
        let start = cursor + relative;
        if let Some(next) = skip_ignored_block(html, lower, start, tag_name) {
            cursor = next;
            continue;
        }
        if matches_tag_at(lower, start, tag_name, true) {
            let close_end = find_tag_end(html, start)?;
            depth -= 1;
            if depth == 0 {
                return Some((start, close_end));
            }
            cursor = close_end + 1;
            continue;
        }
        if matches_tag_at(lower, start, tag_name, false) {
            let open_end = find_tag_end(html, start)?;
            if !is_self_closing_tag(&html[start + 1..open_end]) {
                depth += 1;
            }
            cursor = open_end + 1;
            continue;
        }
        cursor = start + 1;
    }
    None
}

fn skip_ignored_block(html: &str, lower: &str, start: usize, target_tag: &str) -> Option<usize> {
    if lower[start..].starts_with("<!--") {
        return lower[start + 4..]
            .find("-->")
            .map(|relative| start + 4 + relative + 3)
            .or(Some(html.len()));
    }

    for raw_tag in ["script", "style"] {
        if target_tag.eq_ignore_ascii_case(raw_tag) || !matches_tag_at(lower, start, raw_tag, false)
        {
            continue;
        }
        let Some(open_end) = find_tag_end(html, start) else {
            return Some(html.len());
        };
        let closing_prefix = format!("</{raw_tag}");
        let Some(relative_close) = lower[open_end + 1..].find(&closing_prefix) else {
            return Some(html.len());
        };
        let close_start = open_end + 1 + relative_close;
        let Some(close_end) = find_tag_end(html, close_start) else {
            return Some(html.len());
        };
        return Some(close_end + 1);
    }
    None
}

fn matches_tag_at(lower: &str, start: usize, tag_name: &str, closing: bool) -> bool {
    let prefix = if closing {
        format!("</{tag_name}")
    } else {
        format!("<{tag_name}")
    };
    if !lower[start..].starts_with(&prefix) {
        return false;
    }
    let after_name = start + prefix.len();
    lower.as_bytes().get(after_name).is_some_and(|byte| {
        byte.is_ascii_whitespace() || *byte == b'>' || (!closing && *byte == b'/')
    })
}

fn is_self_closing_tag(contents: &str) -> bool {
    contents.trim_end().ends_with('/')
}

fn text_content(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            value if !in_tag => text.push(value),
            _ => {}
        }
    }
    html_unescape(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn text_content_preserving_whitespace(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            value if !in_tag => text.push(value),
            _ => {}
        }
    }
    html_unescape(&text).trim().to_owned()
}

fn text_near_marker(html: &str, marker: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let marker = marker.to_ascii_lowercase();

    // The current portal renders the icon as an empty element and places the
    // status link beside it under the same parent.  Selecting the marker's
    // parent and then its link mirrors the public client selector and avoids
    // treating an empty icon element as missing account status.
    for tag_name in ["td", "li", "div", "section", "main", "p"] {
        for (_, body) in element_blocks(html, tag_name) {
            if !body.to_ascii_lowercase().contains(&marker) {
                continue;
            }
            if let Some(text) = element_blocks(&body, "a")
                .into_iter()
                .map(|(_, body)| text_content(&body))
                .find(|text| !text.is_empty())
            {
                return Some(text);
            }
            let text = text_content(&body);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    // Keep support for deployments where the marker element itself contains
    // the text, while still rejecting an empty marker.
    let start = lower.find(&marker)?;
    let tag_start = html[..start].rfind('<').unwrap_or(start);
    let content_start = html[tag_start..]
        .find('>')
        .map(|offset| tag_start + offset + 1)
        .unwrap_or(start);
    let end = lower[content_start..]
        .find("</")
        .map(|offset| content_start + offset)
        .unwrap_or((content_start + 512).min(html.len()));
    let text = text_content(&html[content_start..end]);
    (!text.is_empty()).then_some(text)
}

fn first_integer_near_marker(html: &str, marker: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    let marker = marker.to_ascii_lowercase();
    let start = lower.find(&marker)?;
    let content_start = html[start..]
        .find('>')
        .map(|offset| start + offset + 1)
        .unwrap_or(start);
    let window_end = (content_start + 512).min(html.len());
    let window = text_content(&html[content_start..window_end]);
    let bytes = window.as_bytes();
    let mut values = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let is_sign = matches!(bytes[cursor], b'+' | b'-')
            && bytes.get(cursor + 1).is_some_and(u8::is_ascii_digit)
            && (cursor == 0 || !bytes[cursor - 1].is_ascii_digit());
        if !bytes[cursor].is_ascii_digit() && !is_sign {
            cursor += 1;
            continue;
        }

        let negative = is_sign && bytes[cursor] == b'-';
        if is_sign {
            cursor += 1;
        }
        let digits_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if negative {
            return None;
        }
        let value = std::str::from_utf8(&bytes[digits_start..cursor])
            .ok()?
            .parse::<usize>()
            .ok()?;
        values.push(value);
    }
    (values.len() == 1).then(|| values[0])
}

fn valid_device_address(value: &str, ipv6: bool) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    match value.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => !ipv6 && !address.is_unspecified(),
        Ok(IpAddr::V6(address)) => ipv6 && !address.is_unspecified(),
        Err(_) => false,
    }
}

fn validation_binding(
    page: &UseregLoginPage,
    credentials: &UseregLoginCredentials,
    verify_code: &str,
    encrypted: &EncryptedPassword,
) -> ValidationBinding {
    let mut digest = Sha1::new();
    for value in [
        "thyou-usereg-validation-v1",
        page.form_csrf.as_str(),
        page.header_csrf.as_str(),
        credentials.username.as_str(),
        verify_code,
        encrypted.0.as_str(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    ValidationBinding(digest.finalize().into())
}

fn encrypt_password(
    page: &UseregLoginPage,
    credentials: &UseregLoginCredentials,
) -> Result<EncryptedPassword, UseregAdapterError> {
    let key = RsaPublicKey::from_public_key_pem(&page.public_key_pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(&page.public_key_pem))
        .map_err(|_| UseregAdapterError::InvalidPublicKey)?;
    let mut rng = rsa::rand_core::OsRng;
    let encrypted = key
        .encrypt(&mut rng, Pkcs1v15Encrypt, credentials.password().as_bytes())
        .map_err(|_| UseregAdapterError::PasswordEncryption)?;
    Ok(EncryptedPassword(BASE64.encode(encrypted)))
}

fn valid_token(value: &str) -> bool {
    !value.trim().is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn is_html_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().map(str::trim).unwrap_or_default();
    matches!(
        media_type.to_ascii_lowercase().as_str(),
        "text/html" | "application/xhtml+xml"
    )
}

fn is_json_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().map(str::trim).unwrap_or_default();
    let lower = media_type.to_ascii_lowercase();
    lower == "application/json" || lower.ends_with("+json")
}

fn looks_like_rsa_pem(value: &str) -> bool {
    value.contains("BEGIN PUBLIC KEY") || value.contains("BEGIN RSA PUBLIC KEY")
}

fn find_meta_content(html: &str, name: &str) -> Result<Option<String>, UseregAdapterError> {
    let mut found = None;
    for tag in opening_tags(html) {
        if !tag.name.eq_ignore_ascii_case("meta") {
            continue;
        }
        if tag
            .attr("name")
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
        {
            let Some(value) = tag.attr("content").map(str::to_owned) else {
                return Err(UseregAdapterError::InvalidHeaderCsrf);
            };
            if found.is_some() {
                return Err(UseregAdapterError::InvalidHeaderCsrf);
            }
            found = Some(value);
        }
    }
    Ok(found)
}

fn find_element_value(html: &str, id: &str) -> Option<String> {
    for tag in opening_tags(html) {
        if !tag.attr("id").is_some_and(|value| value == id) {
            continue;
        }
        if let Some(value) = tag.attr("value") {
            return Some(value.to_owned());
        }
    }

    // The current deployment renders the RSA key in an input, while newer
    // clients also support a textarea with the same DOM id. Read only the
    // element body here; the caller still validates that it is a PEM key.
    element_blocks(html, "textarea")
        .into_iter()
        .find(|(opening, _)| opening.attr("id") == Some(id))
        .map(|(_, body)| text_content_preserving_whitespace(&body))
        .filter(|value| !value.trim().is_empty())
}

fn html_unescape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find('&') {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let Some(relative_end) = value[start..].find(';') else {
            output.push_str(&value[start..]);
            return output;
        };
        let end = start + relative_end;
        let entity = &value[start + 1..end];
        if let Some(decoded) = decode_html_entity(entity) {
            output.push(decoded);
        } else {
            output.push_str(&value[start..=end]);
        }
        cursor = end + 1;
    }
    output.push_str(&value[cursor..]);
    output
}

fn decode_html_entity(entity: &str) -> Option<char> {
    let named = match entity.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        "hellip" => Some('…'),
        "mdash" => Some('—'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        _ => None,
    };
    if named.is_some() {
        return named;
    }

    let number = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
        .map(|value| u32::from_str_radix(value, 16).ok())
        .or_else(|| {
            entity
                .strip_prefix('#')
                .map(|value| value.parse::<u32>().ok())
        })??;
    char::from_u32(number)
}

#[derive(Debug)]
struct OpeningTag {
    name: String,
    attributes: Vec<(String, String)>,
}

impl OpeningTag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn opening_tags(html: &str) -> Vec<OpeningTag> {
    let lower = html.to_ascii_lowercase();
    let mut tags = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find('<') {
        let start = cursor + relative;
        if lower[start..].starts_with("<!--") {
            if let Some(end) = lower[start + 4..].find("-->") {
                cursor = start + 4 + end + 3;
                continue;
            }
            break;
        }
        if lower[start..].starts_with("</") || lower[start..].starts_with("<!") {
            cursor = start + 1;
            continue;
        }
        let Some(end) = find_tag_end(html, start) else {
            break;
        };
        let contents = &html[start + 1..end];
        let lower_contents = contents.to_ascii_lowercase();
        if lower_contents.starts_with("script") || lower_contents.starts_with("style") {
            let tag_name = if lower_contents.starts_with("script") {
                "script"
            } else {
                "style"
            };
            let closing = format!("</{tag_name}");
            if let Some(relative_close) = lower[end + 1..].find(&closing) {
                cursor = end + 1 + relative_close + closing.len();
                continue;
            }
            break;
        }
        if let Some(tag) = parse_opening_tag(contents) {
            tags.push(tag);
        }
        cursor = end + 1;
    }
    tags
}

fn find_tag_end(html: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in html[start..].char_indices() {
        match (quote, character) {
            (Some(expected), value) if value == expected => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn parse_opening_tag(contents: &str) -> Option<OpeningTag> {
    let mut parts = contents.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim_end_matches('/').to_owned();
    if name.is_empty() {
        return None;
    }
    let rest = parts.next().unwrap_or_default();
    let mut attributes = Vec::new();
    let mut position = 0;
    while position < rest.len() {
        while position < rest.len() && rest.as_bytes()[position].is_ascii_whitespace() {
            position += 1;
        }
        if position >= rest.len() || rest.as_bytes()[position] == b'/' {
            break;
        }
        let name_start = position;
        while position < rest.len()
            && !rest.as_bytes()[position].is_ascii_whitespace()
            && rest.as_bytes()[position] != b'='
        {
            position += 1;
        }
        let attr_name = rest[name_start..position].to_owned();
        while position < rest.len() && rest.as_bytes()[position].is_ascii_whitespace() {
            position += 1;
        }
        if position >= rest.len() || rest.as_bytes()[position] != b'=' {
            attributes.push((attr_name, String::new()));
            continue;
        }
        position += 1;
        while position < rest.len() && rest.as_bytes()[position].is_ascii_whitespace() {
            position += 1;
        }
        if position >= rest.len() {
            break;
        }
        let value = if matches!(rest.as_bytes()[position], b'\'' | b'"') {
            let quote = rest.as_bytes()[position];
            position += 1;
            let value_start = position;
            while position < rest.len() && rest.as_bytes()[position] != quote {
                position += 1;
            }
            let value = rest[value_start..position].to_owned();
            position += usize::from(position < rest.len());
            value
        } else {
            let value_start = position;
            while position < rest.len() && !rest.as_bytes()[position].is_ascii_whitespace() {
                position += 1;
            }
            // In an unquoted HTML attribute, `/` is a valid value character.
            // Removing a trailing slash here corrupts paths such as
            // `/site/captcha/` and WebVPN mapped URLs. The tag scanner already
            // recognizes a self-closing marker independently, so preserve the
            // complete attribute value exactly as the browser tokenizer does.
            rest[value_start..position].to_owned()
        };
        let value = html_unescape(&value);
        attributes.push((attr_name, value));
    }
    Some(OpeningTag { name, attributes })
}

#[cfg(test)]
mod tests {
    use super::*;
    mod captcha_tests {
        include!("usereg_captcha_tests.rs");
    }
    mod session_repair_tests {
        include!("usereg_session_repair_tests.rs");
    }
    use crate::usereg::{UseregCertificationInput, UseregProfile};
    use crate::usereg_client::UseregClientConfig;
    use reqwest::StatusCode;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc,
        thread,
        time::Duration,
    };

    const USERNAME: &str = "fixture-user";
    const PASSWORD: &str = "fixture-password";
    const PAGE: &str = r#"
        <html><head>
          <meta name="csrf-token" content="meta-csrf-fixture">
        </head><body>
          <input type="hidden" name="_csrf-8800" value="form-csrf-fixture">
          <input id="public" value="-----BEGIN PUBLIC KEY-----&#10;MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDRXPpskRTB0TMRDcrQU+qve7lr&#10;XX2/jIBtErat5KH5YGOqZvXjVH1/BU0GfkbIhAI7qOSOzwKdIKtejt8RgrIS5qoL&#10;cYZZb87CK5wCydkTIZkH59MiyQ5Dt1GeE90Wff9zczIE4BS29AqxgzZIEq1TsPp5&#10;J7jNOyN8KKRa62yvlQIDAQAB&#10;-----END PUBLIC KEY-----">
        </body></html>
    "#;

    fn adapter() -> UseregAdapter {
        let config =
            UseregClientConfig::new("https://usereg.example.test/", UseregProfile::default())
                .expect("config");
        UseregAdapter::new(UseregClient::new(config).expect("client"))
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut buffer).expect("request bytes");
            assert!(read > 0, "request ended before headers");
            request.extend_from_slice(&buffer[..read]);
            if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let header_text = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header_text
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length:")
                    .or_else(|| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("request body");
            assert!(read > 0, "request ended before body");
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn write_http_response(stream: &mut TcpStream, headers: &str, body: impl AsRef<[u8]>) {
        write_http_response_with_status(stream, "200 OK", headers, body);
    }

    fn write_http_response_with_status(
        stream: &mut TcpStream,
        status: &str,
        headers: &str,
        body: impl AsRef<[u8]>,
    ) {
        let body = body.as_ref();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len(),
        );
        stream.write_all(response.as_bytes()).expect("response");
        stream.write_all(body).expect("response body");
    }

    fn form_value(request: &str, encoded_name: &str) -> String {
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("HTTP request body");
        let prefix = format!("{encoded_name}=");
        body.split('&')
            .find_map(|field| field.strip_prefix(&prefix))
            .map(str::to_owned)
            .expect("form field")
    }

    const HOME_WITH_DEVICE: &str = r#"<html><body>
      <input type="hidden" name="_csrf-8800" value="home-csrf">
      <div id="w1-container"><table><tbody><tr data-key="17">
        <td>192.0.2.10</td><td></td><td>2026-09-11 10:00:00</td><td>campus</td><td>AA-BB-CC</td>
      </tr></tbody></table></div>
      <div id="w3-container"><table><tbody><tr><td>学生</td><td>0</td><td>0</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div>
    </body></html>"#;

    const HOME_WITHOUT_DEVICE: &str = r#"<html><body>
      <input type="hidden" name="_csrf-8800" value="home-csrf-new">
      <div id="w1-container"><table><tbody></tbody></table></div>
      <div id="w3-container"><table><tbody><tr><td>学生</td><td>0</td><td>0</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div>
    </body></html>"#;

    const HOME_WITH_STATUS: &str = r#"<html><body>
      <input type="hidden" name="_csrf-8800" value="home-csrf">
      <div id="w1-container"><table><tbody><tr data-key="17">
        <td>192.0.2.10</td><td></td><td>2026-09-11 10:00:00</td><td>campus</td><td>AA-BB-CC</td>
      </tr></tbody></table></div>
      <div id="w3-container"><table><tbody><tr><td>学生</td><td>0</td><td>0</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div>
      <span class="glyphicon-info-sign">正常</span>
    </body></html>"#;

    const USERS_WITH_ACCOUNT: &str = r#"<html><body><div id="w0"><table><tr>
      <td>fixture-user</td><td>mail@example.test</td><td>phone</td><td>清华园</td>
      <td>unused</td><td>landline</td><td>fixture-name</td><td>学生</td>
    </tr></table></div></body></html>"#;

    const ONLINE_NUM_WITH_COUNT: &str = r#"<html><body>
      <span class="glyphicon-exclamation-sign"></span><strong>5</strong>
    </body></html>"#;

    const LOGIN_FORM: &str = r#"<html><body>
      <form><input name="LoginForm[username]"><input name="LoginForm[password]"></form>
    </body></html>"#;

    #[test]
    fn models_the_public_three_request_sequence() {
        let adapter = adapter();
        assert_eq!(
            adapter
                .login_page_request_plan()
                .expect("login page")
                .request
                .operation,
            UseregOperation::LoginPage
        );
        assert_eq!(
            adapter
                .captcha_refresh_request_plan()
                .expect("captcha refresh")
                .request
                .query,
            vec![("refresh".to_owned(), "1".to_owned())]
        );
        assert_eq!(
            adapter
                .captcha_image_request_plan("fixture-cache-buster")
                .expect("captcha image")
                .request
                .query,
            vec![("_".to_owned(), "fixture-cache-buster".to_owned())]
        );
    }

    #[test]
    fn parses_csrf_and_public_key_without_exposing_them_in_debug() {
        let adapter = adapter();
        let page = adapter.parse_login_page(PAGE).expect("login page");
        let debug = format!("{page:?}");
        assert!(!debug.contains("meta-csrf-fixture"));
        assert!(!debug.contains("form-csrf-fixture"));
        assert!(!debug.contains("BEGIN PUBLIC KEY"));
        let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
        let plan = adapter
            .validate_user_request_plan(&page, &credentials, "1234")
            .expect("RSA validation plan");
        let transport =
            crate::transport::CampusHttpTransport::new("THYou/test").expect("transport");
        let request = plan.build_request(&transport).expect("request");
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("form body");
        let body = std::str::from_utf8(body).expect("form UTF-8");
        assert!(body.contains("LoginForm%5Busername%5D=fixture-user"));
        assert!(body.contains("LoginForm%5Bpassword%5D="));
        assert!(!body.contains(PASSWORD));

        let login = adapter
            .login_request_plan(&page, &credentials, "1234", Some("sms-fixture"))
            .expect("login plan");
        let request = login.build_request(&transport).expect("login request");
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("login form body");
        let body = std::str::from_utf8(body).expect("login UTF-8");
        assert!(body.contains("LoginForm%5BsmsCode%5D=sms-fixture"));
        assert!(!body.contains(PASSWORD));
    }

    #[test]
    fn login_page_rejects_conflicting_or_missing_meta_csrf_values() {
        let adapter = adapter();
        for html in [
            r#"<meta name="csrf-token" content="one"><meta name="csrf-token" content="two">"#,
            r#"<meta name="csrf-token" content="one"><meta name="csrf-token" content="one">"#,
            r#"<meta name="csrf-token">"#,
        ] {
            assert!(matches!(
                adapter.parse_login_page(&format!(
                    r#"<input type="hidden" name="_csrf-8800" value="form-csrf"><input id="public" value="{}">{}"#,
                    "-----BEGIN PUBLIC KEY-----fixture-----END PUBLIC KEY-----",
                    html
                )),
                Err(UseregAdapterError::InvalidHeaderCsrf)
                    | Err(UseregAdapterError::MissingHeaderCsrf)
            ));
        }
    }

    #[test]
    fn accepts_a_textarea_public_key_and_decodes_common_html_entities() {
        let key = "-----BEGIN PUBLIC KEY-----\nfixture\n-----END PUBLIC KEY-----";
        let html = format!(r#"<textarea id="public">{key}</textarea>"#);
        assert_eq!(find_element_value(&html, "public").as_deref(), Some(key));
        assert_eq!(
            text_content("学生&nbsp;&amp;&nbsp;&#x4E2D;&#25991;"),
            "学生 & 中文"
        );
        assert_eq!(html_unescape("line&#10;break"), "line\nbreak");
    }

    #[test]
    fn maps_validation_json_without_returning_server_message() {
        let adapter = adapter();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/validate-user",
            "application/json",
            r#"{"success":false,"message":"验证码错误，request password=secret"} "#.as_bytes(),
        );
        let mapped = adapter
            .parse_validation_response(&response)
            .expect("validation response");
        assert_eq!(mapped.outcome, UseregValidationOutcome::CaptchaRejected);
        assert!(!format!("{mapped:?}").contains("secret"));

        let missing_content_type = UseregHttpResponse::fixture_without_content_type(
            StatusCode::OK,
            "https://usereg.example.test/site/validate-user",
            br#"{"success":true}"#,
        );
        assert!(matches!(
            adapter.parse_validation_response(&missing_content_type),
            Err(UseregAdapterError::InvalidValidationContentType)
        ));

        let html_response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/validate-user",
            "text/html",
            br#"{"success":true}"#,
        );
        assert_eq!(
            adapter
                .parse_validation_response(&html_response)
                .unwrap()
                .outcome,
            UseregValidationOutcome::Accepted
        );
    }

    #[test]
    fn recognizes_authenticated_home_marker_only_after_html_mapping() {
        let adapter = adapter();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/home",
            "text/html",
            r#"<html><body>
              <input type="hidden" name="_csrf-8800" value="home-csrf">
              <div id="w1-container"><table><tbody></tbody></table></div>
              <div id="w3-container"><table><tbody><tr><td>学生</td><td>0</td><td>0</td><td>0.00</td><td>2026-09-11</td></tr></tbody></table></div>
            </body></html>"#
                .as_bytes(),
        );
        let mapped = adapter
            .classify_login_response(&response)
            .expect("home response");
        assert_eq!(mapped.state, UseregAuthenticationState::Authenticated);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn validation_result_cannot_be_reused_for_another_captcha_or_page() {
        let adapter = adapter();
        let page = adapter.parse_login_page(PAGE).expect("login page");
        let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
        let encrypted = EncryptedPassword("wire-password-fixture".to_owned());
        let validation = UseregValidationResult {
            outcome: UseregValidationOutcome::Accepted,
            encrypted_password: Some(encrypted.clone()),
            binding: Some(validation_binding(
                &page,
                &credentials,
                "original-captcha",
                &encrypted,
            )),
        };
        let transport =
            crate::transport::CampusHttpTransport::new("THYou/test").expect("transport");
        let error = adapter
            .login_after_validation(
                &transport,
                &page,
                &credentials,
                "different-captcha",
                None,
                validation,
            )
            .await
            .expect_err("validation must be bound to its captcha");
        assert!(matches!(error, UseregAdapterError::ValidationRequired));
    }

    #[test]
    fn captcha_metadata_never_contains_image_bytes() {
        let adapter = adapter();
        let png = crate::reference_test_support::captcha_png();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha",
            "image/png",
            &png,
        );
        let metadata = adapter
            .parse_captcha_metadata(&response)
            .expect("captcha metadata");
        assert_eq!(metadata.byte_len, png.len());
        assert!(!format!("{metadata:?}").contains("137"));
    }

    #[test]
    fn captcha_image_preserves_type_and_bytes_but_redacts_bytes_in_debug() {
        let adapter = adapter();
        let png = crate::reference_test_support::captcha_png();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha",
            "Image/PNG; charset=binary",
            &png,
        );
        let image = adapter
            .parse_captcha_image(&response)
            .expect("captcha image");
        assert_eq!(image.content_type, "Image/PNG; charset=binary");
        assert_eq!(image.bytes, png);
        let debug = format!("{image:?}");
        assert!(debug.contains(&format!("byte_len: {}", png.len())));
        assert!(!debug.contains("137"));
        assert!(!debug.contains("80"));
    }

    #[test]
    fn captcha_image_rejects_missing_or_non_image_content_type() {
        let adapter = adapter();
        for content_type in ["text/html", "application/octet-stream", ""] {
            let response = UseregHttpResponse::fixture(
                StatusCode::OK,
                "https://usereg.example.test/site/captcha",
                content_type,
                &[1, 2, 3],
            );
            let error = adapter
                .parse_captcha_image(&response)
                .expect_err("non-image captcha response");
            assert!(matches!(
                error,
                UseregAdapterError::InvalidCaptchaContentType
            ));
        }
    }

    #[test]
    fn captcha_image_rejects_responses_over_the_size_limit() {
        let adapter = adapter();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha",
            "image/png",
            &vec![0; MAX_CAPTCHA_BYTES + 1],
        );
        let error = adapter
            .parse_captcha_image(&response)
            .expect_err("oversized captcha response");
        assert!(matches!(error, UseregAdapterError::CaptchaTooLarge));
    }

    #[test]
    fn captcha_refresh_requires_a_strict_json_metadata_object() {
        let adapter = adapter();
        let valid = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/site/captcha?refresh=1",
            "application/json; charset=UTF-8",
            br#"{"hash1":101,"hash2":202,"url":"/site/captcha"}"#,
        );
        adapter
            .parse_captcha_refresh_response(&valid)
            .expect("valid refresh metadata");

        for (content_type, body) in [
            ("application/json", br#"refreshed"#.as_slice()),
            ("text/html", br#"<html>login</html>"#.as_slice()),
            (
                "application/json",
                br#"{"hash1":101,"url":"/site/captcha"}"#.as_slice(),
            ),
            (
                "application/json",
                br#"{"hash1":1.5,"hash2":202,"url":"/site/captcha"}"#.as_slice(),
            ),
            (
                "application/json",
                br#"{"hash1":-1,"hash2":202,"url":"/site/captcha"}"#.as_slice(),
            ),
            (
                "application/json",
                br#"{"hash1":101,"hash2":202,"url":""}"#.as_slice(),
            ),
        ] {
            let response = UseregHttpResponse::fixture(
                StatusCode::OK,
                "https://usereg.example.test/site/captcha?refresh=1",
                content_type,
                body,
            );
            let error = adapter
                .parse_captcha_refresh_response(&response)
                .expect_err("invalid refresh metadata");
            assert!(matches!(
                error,
                UseregAdapterError::InvalidCaptchaRefreshResponse
            ));
            assert!(!format!("{error:?}").contains("101"));
        }

        for body in [
            br#"{"hash1":101,"hash2":202,"url":"https://usereg.example.test/site/captcha"}"#
                .as_slice(),
            br#"{"hash1":101,"hash2":202,"url":"/site/other"}"#.as_slice(),
            br#"{"hash1":101,"hash2":202,"url":"//other.example.test/site/captcha"}"#.as_slice(),
        ] {
            let response = UseregHttpResponse::fixture(
                StatusCode::OK,
                "https://usereg.example.test/site/captcha?refresh=1",
                "application/json",
                body,
            );
            assert!(matches!(
                adapter.parse_captcha_refresh_response(&response),
                Err(UseregAdapterError::InvalidCaptchaRefreshResponse)
            ));
        }
    }

    #[test]
    fn login_pages_with_expired_or_failure_markers_are_not_authenticated() {
        let adapter = adapter();
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/login",
            "text/html",
            br#"<form><input name="LoginForm[username]"><input name="LoginForm[password]"></form>"#,
        );
        let mapped = adapter
            .classify_login_response(&response)
            .expect("login response");
        assert!(matches!(
            mapped.state,
            UseregAuthenticationState::LoginRequired
        ));
    }

    #[test]
    fn a_login_form_outweighs_stale_home_shaped_markup() {
        let adapter = adapter();
        let body = format!(
            r#"<form><input name="LoginForm[username]"><input name="LoginForm[password]"></form>{HOME_WITH_DEVICE}"#
        );
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/home",
            "text/html",
            body.as_bytes(),
        );
        let result = adapter
            .classify_login_response(&response)
            .expect("classification");
        assert_eq!(result.state, UseregAuthenticationState::LoginRequired);
    }

    #[test]
    fn parses_authenticated_home_devices_balance_and_account_fixtures() {
        let home = r#"
            <div id="w1-container"><table><tbody>
              <tr data-key="17"><td>192.0.2.10</td><td>2001:db8::10</td>
                <td>2026-09-11 10:00:00</td><td>campus</td><td>AA-BB-CC</td></tr>
            </tbody></table></div>
            <div id="w3-container"><table><tbody>
              <tr><td>学生</td><td>1G</td><td>2h</td><td>8.10</td><td>2026-10-01</td></tr>
            </tbody></table></div>
            <span class="glyphicon-info-sign"><a>正常</a></span>
        "#;
        let devices = parse_devices(home).expect("device table");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "17");
        assert_eq!(devices[0].mac, "AA-BB-CC");
        let balance = parse_balance(home).expect("balance table");
        assert_eq!(balance.account_balance, "8.10");

        let users = r#"<table id="w0"><tr>
          <td>student</td><td>mail</td><td>phone</td><td>location</td>
          <td>unused</td><td>landline</td><td>name</td><td>group</td>
        </tr></table>"#;
        let cells = table_body_by_id(users, "w0")
            .map(|body| element_texts(&body, "td"))
            .expect("account table");
        assert_eq!(cells[0], "student");
        assert_eq!(cells[7], "group");
        assert_eq!(
            text_near_marker(home, "glyphicon-info-sign").as_deref(),
            Some("正常")
        );
    }

    #[test]
    fn maps_the_current_data_column_balance_layout_instead_of_display_order() {
        let home = r#"
            <div id="w3-container"><table><thead><tr>
              <th>名称</th><th>无关列</th><th>已用流量</th><th>已用时长</th>
              <th>余额</th><th>结算日期</th>
            </tr></thead><tbody><tr>
              <td data-col-seq="7">8.10</td>
              <td data-col-seq="1">学生</td>
              <td data-col-seq="10">2026-10-01</td>
              <td data-col-seq="3">1G</td>
              <td data-col-seq="4">2h</td>
            </tr></tbody></table></div>
        "#;
        let balance = parse_balance(home).expect("current balance layout");
        assert_eq!(balance.product_name, "学生");
        assert_eq!(balance.used_bytes, "1G");
        assert_eq!(balance.used_seconds, "2h");
        assert_eq!(balance.account_balance, "8.10");
        assert_eq!(balance.settlement_date, "2026-10-01");
    }

    #[test]
    fn rejects_duplicate_or_missing_current_balance_column_sequences() {
        for row in [
            r#"<tr><td data-col-seq="1">学生</td><td data-col-seq="1">重复</td><td data-col-seq="3">1G</td><td data-col-seq="4">2h</td><td data-col-seq="7">8.10</td><td data-col-seq="10">2026-10-01</td></tr>"#,
            r#"<tr><td data-col-seq="1">学生</td><td data-col-seq="3">1G</td><td data-col-seq="4">2h</td><td data-col-seq="7">8.10</td></tr>"#,
        ] {
            let html =
                format!(r#"<div id="w3-container"><table><tbody>{row}</tbody></table></div>"#);
            assert_eq!(parse_balance(&html), None);
        }

        let extra_legacy_column = r#"
            <div id="w3-container"><table><tbody><tr>
              <td>学生</td><td>1G</td><td>2h</td><td>8.10</td>
              <td>2026-10-01</td><td>unexpected</td>
            </tr></tbody></table></div>
        "#;
        assert_eq!(parse_balance(extra_legacy_column), None);
    }

    #[test]
    fn account_status_accepts_the_real_empty_icon_and_sibling_link_shape() {
        let home = r#"
            <td>
              <span class="glyphicon-info-sign"></span>
              <a href="/users">正常</a>
            </td>
        "#;
        assert_eq!(
            text_near_marker(home, "glyphicon-info-sign").as_deref(),
            Some("正常")
        );
    }

    #[test]
    fn allowed_device_count_rejects_multiple_unrelated_numbers() {
        let html = r#"<span class="glyphicon-exclamation-sign"></span>
            <strong>5</strong><span>other 2026</span>"#;
        assert_eq!(
            first_integer_near_marker(html, "glyphicon-exclamation-sign"),
            None
        );
        assert_eq!(
            first_integer_near_marker(
                r#"<span class="glyphicon-exclamation-sign"></span><strong>-5</strong>"#,
                "glyphicon-exclamation-sign"
            ),
            None
        );
        assert_eq!(
            first_integer_near_marker(
                r#"<span class="glyphicon-exclamation-sign"></span><strong>5</strong> 台"#,
                "glyphicon-exclamation-sign"
            ),
            Some(5)
        );
    }

    #[test]
    fn malformed_or_missing_device_tables_are_not_empty_successes() {
        let missing = r#"<div id="w1-container"><p>login required</p></div>"#;
        assert!(matches!(
            parse_devices(missing),
            Err(UseregAdapterError::DeviceDataUnavailable)
        ));

        let malformed = r#"<div id="w1-container"><table><tr data-key="17"><td>only-one-cell</td></tr></table></div>"#;
        assert!(matches!(
            parse_devices(malformed),
            Err(UseregAdapterError::DeviceDataUnavailable)
        ));

        let malformed_body_row = r#"<div id="w1-container"><table><tbody><tr><td>192.0.2.10</td><td></td><td>time</td><td>campus</td><td>AA-BB-CC</td></tr></tbody></table></div>"#;
        assert!(matches!(
            parse_devices(malformed_body_row),
            Err(UseregAdapterError::DeviceDataUnavailable)
        ));

        let malformed_ip = r#"<div id="w1-container"><table><tbody><tr data-key="17"><td>not-an-ip</td><td></td><td>time</td><td>campus</td><td>AA-BB-CC</td></tr></tbody></table></div>"#;
        assert!(matches!(
            parse_devices(malformed_ip),
            Err(UseregAdapterError::DeviceDataUnavailable)
        ));

        let wrong_ip_family = r#"<div id="w1-container"><table><tbody><tr data-key="17"><td>2001:db8::10</td><td></td><td>time</td><td>campus</td><td>AA-BB-CC</td></tr></tbody></table></div>"#;
        assert!(matches!(
            parse_devices(wrong_ip_family),
            Err(UseregAdapterError::DeviceDataUnavailable)
        ));

        let empty = r#"<div id="w1-container"><table><tbody></tbody></table></div>"#;
        assert_eq!(parse_devices(empty).expect("valid empty table"), Vec::new());
    }

    #[test]
    fn incomplete_balance_and_home_proof_are_rejected() {
        let adapter = adapter();
        assert_eq!(
            parse_balance(r#"<div id="w3-container"><table><tr><td>学生</td></tr></table></div>"#),
            None
        );
        assert_eq!(
            parse_balance(
                r#"<div id="w3-container"><tr><td>学生</td><td>1G</td><td>2h</td><td>8.10</td><td>2026-10-01</td></tr></div>"#
            ),
            None
        );
        let response = UseregHttpResponse::fixture(
            StatusCode::OK,
            "https://usereg.example.test/home",
            "text/html",
            br#"<input type="hidden" name="_csrf-8800" value="home-csrf"><div id="w1-container"><table></table></div><div id="w3-container"><table></table></div>"#,
        );
        let result = adapter
            .classify_login_response(&response)
            .expect("classification");
        assert_eq!(result.state, UseregAuthenticationState::Unknown);
    }

    #[test]
    fn adapter_error_classes_keep_protocol_boundaries_distinct() {
        assert_eq!(
            UseregAdapterError::InvalidHtmlResponse.class(),
            UseregAdapterErrorClass::Html
        );
        assert_eq!(
            UseregAdapterError::InvalidValidationResponse.class(),
            UseregAdapterErrorClass::Json
        );
        assert_eq!(
            UseregAdapterError::AccountDataUnavailable.class(),
            UseregAdapterErrorClass::Business
        );
        assert_eq!(
            UseregAdapterError::SessionMissing.class(),
            UseregAdapterErrorClass::SessionMissing
        );
        assert_eq!(
            UseregAdapterError::SessionExpired.class(),
            UseregAdapterErrorClass::SessionExpired
        );
        assert!(UseregAdapterError::SessionExpired.is_session_failure());
        assert!(!UseregAdapterError::ValidationRequired.is_session_failure());
        assert!(!UseregAdapterError::InvalidValidationResponse.is_session_failure());
    }

    #[test]
    fn http_auth_statuses_are_not_collapsed_into_generic_http_failures() {
        for (status, class) in [
            (
                StatusCode::UNAUTHORIZED,
                UseregAdapterErrorClass::SessionExpired,
            ),
            (
                StatusCode::FORBIDDEN,
                UseregAdapterErrorClass::SessionExpired,
            ),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                UseregAdapterErrorClass::Http,
            ),
        ] {
            let response = UseregHttpResponse::fixture(
                status,
                "https://usereg.example.test/home",
                "text/html",
                b"fixture",
            );
            let error = response.ensure_success().expect_err("non-success response");
            let error = UseregAdapterError::from(error);
            assert_eq!(error.class(), class);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn home_login_form_is_a_session_expiry_for_authenticated_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("home request");
            let _ = read_http_request(&mut stream);
            write_http_response(&mut stream, "Content-Type: text/html\r\n", LOGIN_FORM);
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let error = adapter
            .read_balance(&transport)
            .await
            .expect_err("login form cannot be a balance page");
        assert!(matches!(error, UseregAdapterError::SessionExpired));
        assert!(error.is_session_failure());
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn home_redirect_to_an_unprofiled_route_is_route_changed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("home redirect request");
            let _ = read_http_request(&mut stream);
            write_http_response_with_status(
                &mut stream,
                "302 Found",
                "Location: /unexpected\r\nContent-Type: text/html\r\n",
                "redirect",
            );

            let (mut stream, _) = listener.accept().expect("redirect target request");
            let _ = read_http_request(&mut stream);
            write_http_response(
                &mut stream,
                "Content-Type: text/html\r\n",
                "<html><body>unexpected route</body></html>",
            );
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let error = adapter
            .read_balance(&transport)
            .await
            .expect_err("route drift cannot be a balance page");
        assert!(matches!(error, UseregAdapterError::RouteChanged));
        assert_eq!(error.class(), UseregAdapterErrorClass::Html);
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn users_login_html_is_session_expiry_after_home_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            for (headers, body) in [
                (
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    HOME_WITH_STATUS,
                ),
                ("Content-Type: text/html\r\n", LOGIN_FORM),
            ] {
                let (mut stream, _) = listener.accept().expect("account request");
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let error = adapter
            .read_account_info(&transport)
            .await
            .expect_err("users login page cannot be account data");
        assert!(matches!(error, UseregAdapterError::SessionExpired));
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn home_http_statuses_keep_session_and_server_failures_separate() {
        for (status, class) in [
            ("401 Unauthorized", UseregAdapterErrorClass::SessionExpired),
            ("403 Forbidden", UseregAdapterErrorClass::SessionExpired),
            ("500 Internal Server Error", UseregAdapterErrorClass::Http),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
            let address = listener.local_addr().expect("fixture address");
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("home request");
                let _ = read_http_request(&mut stream);
                write_http_response_with_status(
                    &mut stream,
                    status,
                    "Content-Type: text/html\r\n",
                    "fixture failure",
                );
            });

            let config =
                UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                    .expect("config");
            let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
            let transport =
                CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                    .expect("transport");
            let error = adapter
                .read_balance(&transport)
                .await
                .expect_err("HTTP failure");
            assert_eq!(error.class(), class);
            server.join().expect("fixture server");
        }
    }

    #[test]
    fn nested_same_name_containers_do_not_truncate_home_data() {
        let home = r#"
            <div id="w1-container">
              <div class="layout"><div class="nested"></div></div>
              <table><tbody><tr data-key="17">
                <td>192.0.2.10</td><td></td><td>2026-09-11 10:00:00</td><td>campus</td><td>AA-BB-CC</td>
              </tr></tbody></table>
            </div>
        "#;
        let devices = parse_devices(home).expect("nested device container");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "17");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn account_read_requires_the_home_status_marker_instead_of_defaulting_it() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("home request");
            let _ = read_http_request(&mut stream);
            write_http_response(
                &mut stream,
                "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                HOME_WITH_DEVICE,
            );
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let error = adapter
            .read_account_info(&transport)
            .await
            .expect_err("missing status marker");
        assert!(matches!(error, UseregAdapterError::AccountDataUnavailable));
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn account_read_requires_html_online_num_evidence() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    HOME_WITH_STATUS,
                ),
                ("Content-Type: text/html\r\n", USERS_WITH_ACCOUNT),
                (
                    "Content-Type: text/html\r\n",
                    r#"<form><input name="LoginForm[username]"><input name="LoginForm[password]"></form>"#,
                ),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("account request");
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let error = adapter
            .read_account_info(&transport)
            .await
            .expect_err("login-shaped online count response");
        assert!(matches!(error, UseregAdapterError::SessionExpired));
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn account_read_returns_only_a_proven_online_num_count() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    HOME_WITH_STATUS,
                ),
                ("Content-Type: text/html\r\n", USERS_WITH_ACCOUNT),
                ("Content-Type: text/html\r\n", ONLINE_NUM_WITH_COUNT),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("account request");
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let account = adapter
            .read_account_info(&transport)
            .await
            .expect("account info");
        assert_eq!(account.status, "正常");
        assert_eq!(account.allowed_devices, 5);
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn device_and_balance_reads_execute_real_home_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("home request");
                let request = read_http_request(&mut stream);
                requests_tx.send(request).expect("request capture");
                write_http_response(
                    &mut stream,
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    HOME_WITH_DEVICE,
                );
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let devices = adapter
            .read_online_devices(&transport)
            .await
            .expect("devices");
        let balance = adapter.read_balance(&transport).await.expect("balance");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "17");
        assert_eq!(balance.account_balance, "8.10");

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /home HTTP/1.1"));
        assert!(requests[1].starts_with("GET /home HTTP/1.1"));
        assert!(requests[1].contains("usereg-session=fixture"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn login_sequence_reuses_cookie_and_proves_home_after_the_final_post() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let page = r#"<html><head><meta name="csrf-token" content="meta-csrf-fixture"></head><body>
              <input type="hidden" name="_csrf-8800" value="form-csrf-fixture">
              <input id="public" value="-----BEGIN PUBLIC KEY-----&#10;MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDRXPpskRTB0TMRDcrQU+qve7lr&#10;XX2/jIBtErat5KH5YGOqZvXjVH1/BU0GfkbIhAI7qOSOzwKdIKtejt8RgrIS5qoL&#10;cYZZb87CK5wCydkTIZkH59MiyQ5Dt1GeE90Wff9zczIE4BS29AqxgzZIEq1TsPp5&#10;J7jNOyN8KKRa62yvlQIDAQAB&#10;-----END PUBLIC KEY-----">
            </body></html>"#;
            let responses = [
                (
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    page,
                ),
                ("Content-Type: application/json\r\n", r#"{"success":true}"#),
                ("Content-Type: text/html\r\n", HOME_WITH_DEVICE),
                ("Content-Type: text/html\r\n", HOME_WITH_DEVICE),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("request");
                let request = read_http_request(&mut stream);
                requests_tx.send(request).expect("request capture");
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let credentials = UseregLoginCredentials::new(USERNAME, PASSWORD).expect("credentials");
        let page = adapter
            .fetch_login_page(&transport)
            .await
            .expect("login page");
        let validation = adapter
            .validate_user(&transport, &page, &credentials, "ABCD")
            .await
            .expect("validation");
        assert_eq!(validation.outcome, UseregValidationOutcome::Accepted);
        let result = adapter
            .login_after_validation(&transport, &page, &credentials, "ABCD", None, validation)
            .await
            .expect("login result");
        assert_eq!(result.state, UseregAuthenticationState::Authenticated);

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 4);
        assert!(requests[0].starts_with("GET /login HTTP/1.1"));
        assert!(requests[1].starts_with("POST /site/validate-user HTTP/1.1"));
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("x-csrf-token: meta-csrf-fixture")
        );
        assert!(requests[1].contains("LoginForm%5Busername%5D=fixture-user"));
        assert!(requests[1].contains("LoginForm%5Bpassword%5D="));
        assert!(!requests[1].contains(PASSWORD));
        assert!(requests[1].contains("usereg-session=fixture"));
        assert!(requests[2].starts_with("POST /login HTTP/1.1"));
        assert!(requests[2].contains("_csrf-8800=form-csrf-fixture"));
        assert!(requests[2].contains("LoginForm%5BverifyCode%5D=ABCD"));
        assert!(requests[2].contains("usereg-session=fixture"));
        assert_eq!(
            form_value(&requests[1], "LoginForm%5Bpassword%5D"),
            form_value(&requests[2], "LoginForm%5Bpassword%5D")
        );
        assert!(requests[3].starts_with("GET /home HTTP/1.1"));
        assert!(requests[3].contains("usereg-session=fixture"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn captcha_refresh_then_image_uses_the_same_cookie_jar_and_checks_content() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let png = crate::reference_test_support::captcha_png();
            let responses = [
                (
                    "Content-Type: application/json; charset=UTF-8\r\nSet-Cookie: captcha-session=fixture; Path=/\r\n",
                    br#"{"hash1":101,"hash2":202,"url":"/site/captcha"}"#.as_slice(),
                ),
                ("Content-Type: image/png\r\n", png.as_slice()),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("request");
                let request = read_http_request(&mut stream);
                requests_tx.send(request).expect("request capture");
                write_http_response(&mut stream, headers, body);
            }
        });
        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let image = adapter
            .refresh_captcha_image(&transport, "fixture-cache-buster")
            .await
            .expect("captcha image");
        assert_eq!(image.content_type, "image/png");
        assert_eq!(image.bytes, crate::reference_test_support::captcha_png());
        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /site/captcha?refresh=1 HTTP/1.1"));
        assert!(requests[1].starts_with("GET /site/captcha?_=fixture-cache-buster HTTP/1.1"));
        assert!(requests[1].contains("captcha-session=fixture"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn certification_requires_marker_and_follow_up_device_state() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (requests_tx, requests_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    HOME_WITH_DEVICE,
                ),
                (
                    "Content-Type: text/html\r\n",
                    r#"<html><body><input type="hidden" name="_csrf-8800" value="certification-csrf"></body></html>"#,
                ),
                (
                    "Content-Type: text/html\r\n",
                    r#"<div id="w0-success-0">accepted</div>"#,
                ),
                ("Content-Type: text/html\r\n", HOME_WITH_DEVICE),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("certification request");
                let request = read_http_request(&mut stream);
                requests_tx.send(request).expect("request capture");
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let input = UseregCertificationInput::new(
            "192.0.2.10",
            "certification-password-fixture",
            crate::usereg::UseregCertificationType::Internet,
        )
        .expect("certification input");

        let result = adapter
            .certify_device(&transport, &input)
            .await
            .expect("certification result");
        assert_eq!(result.state, UseregCertificationState::Confirmed);

        server.join().expect("fixture server");
        let requests = requests_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 4);
        assert!(requests[0].starts_with("GET /home HTTP/1.1"));
        assert!(requests[1].starts_with("GET /certification HTTP/1.1"));
        assert!(requests[1].contains("usereg-session=fixture"));
        assert!(requests[2].starts_with("POST /certification HTTP/1.1"));
        assert!(
            requests[2]
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
        assert!(requests[2].contains("usereg-session=fixture"));
        assert!(requests[2].contains("_csrf-8800=certification-csrf"));
        assert!(requests[2].contains("CertificationForm%5Bip%5D=192.0.2.10"));
        assert!(requests[2].contains("CertificationForm%5Bpassword%5D="));
        assert!(requests[2].contains("CertificationForm%5Btype%5D=out"));
        assert!(!requests[2].contains("LoginForm%5B"));
        assert!(requests[3].starts_with("GET /home HTTP/1.1"));
        assert!(requests[3].contains("usereg-session=fixture"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn certification_without_a_known_marker_fails_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let responses = [
                ("Content-Type: text/html\r\n", HOME_WITH_DEVICE),
                (
                    "Content-Type: text/html\r\n",
                    r#"<input type="hidden" name="_csrf-8800" value="certification-csrf">"#,
                ),
                ("Content-Type: text/html\r\n", r#"<div>accepted</div>"#),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("certification request");
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let input = UseregCertificationInput::new(
            "192.0.2.20",
            "certification-password-fixture",
            crate::usereg::UseregCertificationType::Internet,
        )
        .expect("certification input");

        let error = adapter
            .certify_device(&transport, &input)
            .await
            .expect_err("unknown certification response must not succeed");
        assert!(matches!(
            error,
            UseregAdapterError::CertificationNotConfirmed
        ));
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn certification_success_marker_without_follow_up_device_proof_fails_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let responses = [
                ("Content-Type: text/html\r\n", HOME_WITH_DEVICE),
                (
                    "Content-Type: text/html\r\n",
                    r#"<input type="hidden" name="_csrf-8800" value="certification-csrf">"#,
                ),
                (
                    "Content-Type: text/html\r\n",
                    r#"<div id="w0-success-0">accepted</div>"#,
                ),
                ("Content-Type: text/html\r\n", HOME_WITHOUT_DEVICE),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("certification request");
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let input = UseregCertificationInput::new(
            "192.0.2.20",
            "certification-password-fixture",
            crate::usereg::UseregCertificationType::Internet,
        )
        .expect("certification input");

        let error = adapter
            .certify_device(&transport, &input)
            .await
            .expect_err("POST marker without device state proof must fail");
        assert!(matches!(
            error,
            UseregAdapterError::CertificationNotConfirmed
        ));
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_requires_the_device_to_disappear_on_a_follow_up_home_read() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let responses = [
                (
                    "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                    HOME_WITH_DEVICE,
                ),
                (
                    "Content-Type: text/html\r\n",
                    r#"<div id="w5-success-0">done</div>"#,
                ),
                ("Content-Type: text/html\r\n", HOME_WITHOUT_DEVICE),
            ];
            for (headers, body) in responses {
                let (mut stream, _) = listener.accept().expect("request");
                let _ = read_http_request(&mut stream);
                write_http_response(&mut stream, headers, body);
            }
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let device = UseregDeviceTarget::new("17", "AA-BB-CC").expect("device");
        let result = adapter
            .disconnect_device(&transport, &device)
            .await
            .expect("disconnect proof");
        assert!(result.confirmed);
        server.join().expect("fixture server");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disconnect_rejects_a_target_that_was_not_present_initially() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture server");
        let address = listener.local_addr().expect("fixture address");
        let (request_tx, request_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("initial home request");
            let request = read_http_request(&mut stream);
            request_tx.send(request).expect("request capture");
            write_http_response(
                &mut stream,
                "Set-Cookie: usereg-session=fixture; Path=/\r\nContent-Type: text/html\r\n",
                HOME_WITHOUT_DEVICE,
            );
        });

        let config =
            UseregClientConfig::new(format!("http://{address}/"), UseregProfile::default())
                .expect("config");
        let adapter = UseregAdapter::new(UseregClient::new(config).expect("client"));
        let transport =
            CampusHttpTransport::with_timeout("THYou/usereg-test", Duration::from_secs(5))
                .expect("transport");
        let device = UseregDeviceTarget::new("17", "AA-BB-CC").expect("device");
        let error = adapter
            .disconnect_device(&transport, &device)
            .await
            .expect_err("missing target device");
        assert!(matches!(error, UseregAdapterError::DeviceNotFound));

        server.join().expect("fixture server");
        let request = request_rx.recv().expect("initial request capture");
        assert!(request.starts_with("GET /home HTTP/1.1"));
    }

    #[test]
    fn disconnect_success_requires_the_explicit_portal_marker() {
        assert!(has_nonempty_element_id(
            r#"<div id="w5-success-0">done</div>"#,
            "w5-success-0"
        ));
        assert!(!has_nonempty_element_id(
            r#"<div id="w5-success-0"></div>"#,
            "w5-success-0"
        ));
        assert!(!has_nonempty_element_id(
            r#"<div id="w5-danger-0">failed</div>"#,
            "w5-success-0"
        ));
    }

    #[test]
    fn unquoted_attributes_keep_url_slashes_and_decode_entities() {
        let tags = opening_tags(
            r#"<meta name=csrf-token content=token&amp;part>
                <a id=handoff href=/site/callback/>
                <input id=public value=key&amp;value>"#,
        );
        let meta = tags
            .iter()
            .find(|tag| tag.name.eq_ignore_ascii_case("meta"))
            .expect("meta tag");
        assert_eq!(meta.attr("content"), Some("token&part"));
        let anchor = tags
            .iter()
            .find(|tag| tag.name.eq_ignore_ascii_case("a"))
            .expect("anchor tag");
        assert_eq!(anchor.attr("href"), Some("/site/callback/"));
        let input = tags
            .iter()
            .find(|tag| tag.name.eq_ignore_ascii_case("input"))
            .expect("input tag");
        assert_eq!(input.attr("value"), Some("key&value"));
    }
}
