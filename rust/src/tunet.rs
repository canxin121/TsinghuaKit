//! Dependency-free TUNet/srun protocol model.
//!
//! The protocol has several deployment profiles.  In particular, auth4 and
//! auth6 must not be treated as one endpoint with a single set of assumptions:
//! the IP family, request method, callback spelling, `ac_id`, and password
//! digest conventions can all vary.  This module therefore keeps those facts
//! in a profile instead of hiding them in global constants.
//!
//! The generic digest enum below is deliberately descriptive only.  It records
//! profile metadata and the generic `OuterDigest` request path carries a value
//! supplied by a caller; this module does not construct or validate an
//! MD5/HMAC-MD5 value.  The current public portal's `srun_bx1` token-keyed
//! HMAC-MD5 construction lives in `tunet_auth`, and `tunet_client` maps only
//! that explicit profile to the Rust-owned derivation.  Other digest
//! candidates remain unsupported instead of triggering an algorithm fallback.
//! The response helpers unwrap JSONP and preserve comma fields, but they do
//! not pretend to be a complete JSON or protocol-status validator.

use std::fmt::{self, Display, Formatter};
use std::net::IpAddr;

const CALLBACK_PARAMETER: &str = "callback";
const AC_ID_PARAMETER: &str = "ac_id";
const IP_PARAMETER: &str = "ip";

const CURRENT_AUTH4_HOST: &str = "auth4.tsinghua.edu.cn";
const CURRENT_AUTH6_HOST: &str = "auth6.tsinghua.edu.cn";
const CURRENT_CHALLENGE_PATH: &str = "/cgi-bin/get_challenge";
const CURRENT_PORTAL_PATH: &str = "/cgi-bin/srun_portal";
const CURRENT_STATUS_PATH: &str = "/cgi-bin/rad_user_info";
const CURRENT_AUTH4_AC_ID: &str = "1";
// Both current public portal pages expose `index_1.html` and set the portal
// acid to 1.  The auth4/auth6 host selects the IP family; it does not imply a
// different service id.  Keep this explicit because older clients guessed 2
// for auth6 and the portal then returns a normal-looking but rejected reply.
const CURRENT_AUTH6_AC_ID: &str = "1";

// The browser portal lets jQuery allocate a callback name.  Rust sends an
// explicit valid name because it does not have a JavaScript callback registry.
const CURRENT_CHALLENGE_CALLBACK: &str = "thyouChallenge";
const CURRENT_PORTAL_CALLBACK: &str = "thyouPortal";
const CURRENT_STATUS_CALLBACK: &str = "thyouStatus";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFamily {
    Auth4,
    Auth6,
}

impl AuthFamily {
    pub fn ip_family(self) -> IpFamily {
        match self {
            Self::Auth4 => IpFamily::V4,
            Self::Auth6 => IpFamily::V6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFamily {
    V4,
    V6,
}

impl Display for IpFamily {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4 => formatter.write_str("IPv4"),
            Self::V6 => formatter.write_str("IPv6"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunetOperation {
    Challenge,
    Login,
    Logout,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OuterDigestScheme {
    /// The deployment profile has not selected a wire convention yet.
    Unspecified,
    /// Candidate label only; the value is supplied by the caller.
    CandidatePlaintext,
    /// Candidate label for a generic caller-supplied MD5 value.
    CandidateMd5Hex,
    /// Candidate label for the current portal's token-keyed HMAC-MD5 profile.
    CandidateHmacMd5Hex,
}

#[derive(Clone, PartialEq, Eq)]
pub struct OuterDigest {
    pub scheme: OuterDigestScheme,
    pub wire_value: String,
}

impl fmt::Debug for OuterDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OuterDigest")
            .field("scheme", &self.scheme)
            .field("wire_value", &"[redacted]")
            .finish()
    }
}

impl OuterDigest {
    pub fn new(
        scheme: OuterDigestScheme,
        wire_value: impl Into<String>,
    ) -> Result<Self, ProfileError> {
        let wire_value = wire_value.into();
        if wire_value.is_empty() {
            return Err(ProfileError::EmptyDigest);
        }
        if wire_value
            .chars()
            .any(|character| character.is_control() || matches!(character, '&' | '=' | '?' | '#'))
        {
            return Err(ProfileError::InvalidDigestValue);
        }

        Ok(Self { scheme, wire_value })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunetEndpointScheme {
    Http,
    Https,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsEndpoint {
    pub host: String,
    pub port: u16,
    scheme: TunetEndpointScheme,
}

impl HttpsEndpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self, ProfileError> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(ProfileError::EmptyHost);
        }
        if port == 0 {
            return Err(ProfileError::InvalidPort);
        }
        if !valid_host(&host) {
            return Err(ProfileError::InvalidHost);
        }

        Ok(Self {
            host,
            port,
            scheme: TunetEndpointScheme::Https,
        })
    }

    /// Construct an explicitly HTTP endpoint for a deployment that has
    /// independently verified the legacy/Android HTTP variant.  Current
    /// auth4/auth6 constructors never call this method and remain HTTPS.
    /// Keeping the scheme explicit prevents an HTTP fallback after a failed
    /// HTTPS request.
    pub fn http(host: impl Into<String>, port: u16) -> Result<Self, ProfileError> {
        let mut endpoint = Self::new(host, port)?;
        endpoint.scheme = TunetEndpointScheme::Http;
        Ok(endpoint)
    }

    pub fn scheme(&self) -> TunetEndpointScheme {
        self.scheme
    }

    pub fn path_url(&self, path: &str) -> Result<String, ProfileError> {
        if !valid_path(path) {
            return Err(ProfileError::InvalidPath(path.to_owned()));
        }

        let host = if self.host.contains(':')
            && !(self.host.starts_with('[') && self.host.ends_with(']'))
        {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        let scheme = match self.scheme {
            TunetEndpointScheme::Http => "http",
            TunetEndpointScheme::Https => "https",
        };
        let default_port = match self.scheme {
            TunetEndpointScheme::Http => 80,
            TunetEndpointScheme::Https => 443,
        };
        let authority = if self.port == default_port {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{}", self.port)
        };

        Ok(format!("{authority}{path}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunetPaths {
    pub challenge: String,
    pub portal: String,
    pub status: String,
}

impl TunetPaths {
    pub fn new(
        challenge: impl Into<String>,
        portal: impl Into<String>,
        status: impl Into<String>,
    ) -> Result<Self, ProfileError> {
        let paths = Self {
            challenge: challenge.into(),
            portal: portal.into(),
            status: status.into(),
        };

        for path in [&paths.challenge, &paths.portal, &paths.status] {
            if !valid_path(path) {
                return Err(ProfileError::InvalidPath(path.clone()));
            }
        }

        Ok(paths)
    }

    pub fn path(&self, operation: TunetOperation) -> &str {
        match operation {
            TunetOperation::Challenge => &self.challenge,
            TunetOperation::Login | TunetOperation::Logout => &self.portal,
            TunetOperation::Status => &self.status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodProfile {
    pub challenge: HttpMethod,
    pub login: HttpMethod,
    pub logout: HttpMethod,
    pub status: HttpMethod,
}

impl MethodProfile {
    pub fn new(
        challenge: HttpMethod,
        login: HttpMethod,
        logout: HttpMethod,
        status: HttpMethod,
    ) -> Self {
        Self {
            challenge,
            login,
            logout,
            status,
        }
    }

    pub fn method(self, operation: TunetOperation) -> HttpMethod {
        match operation {
            TunetOperation::Challenge => self.challenge,
            TunetOperation::Login => self.login,
            TunetOperation::Logout => self.logout,
            TunetOperation::Status => self.status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackProfile {
    pub challenge: Option<String>,
    pub portal: Option<String>,
    pub status: Option<String>,
}

impl CallbackProfile {
    pub fn new(
        challenge: Option<&str>,
        portal: Option<&str>,
        status: Option<&str>,
    ) -> Result<Self, ProfileError> {
        let profile = Self {
            challenge: checked_callback(challenge)?,
            portal: checked_callback(portal)?,
            status: checked_callback(status)?,
        };

        Ok(profile)
    }

    pub fn callback(&self, operation: TunetOperation) -> Option<&str> {
        match operation {
            TunetOperation::Challenge => self.challenge.as_deref(),
            TunetOperation::Login | TunetOperation::Logout => self.portal.as_deref(),
            TunetOperation::Status => self.status.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcIdPolicy {
    /// Do not send `ac_id` for any operation.
    Omit,
    /// Send this value for every operation.
    Fixed(String),
    /// Require the caller to provide `ac_id` for every operation.
    FromRequest,
    /// Send this value for portal login and logout only.
    FixedForPortal(String),
    /// Require the caller to provide `ac_id` for portal login and logout only.
    FromRequestForPortal,
}

impl AcIdPolicy {
    pub fn fixed(value: impl Into<String>) -> Result<Self, ProfileError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProfileError::EmptyPolicyValue(AC_ID_PARAMETER));
        }
        validate_parameter_value(AC_ID_PARAMETER, &value)?;
        Ok(Self::Fixed(value))
    }

    pub fn fixed_for_portal(value: impl Into<String>) -> Result<Self, ProfileError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProfileError::EmptyPolicyValue(AC_ID_PARAMETER));
        }
        validate_parameter_value(AC_ID_PARAMETER, &value)?;
        Ok(Self::FixedForPortal(value))
    }

    pub fn from_request_for_portal() -> Self {
        Self::FromRequestForPortal
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum IpPolicy {
    Omit,
    Fixed { family: IpFamily, value: String },
    FromRequest { family: IpFamily },
}

impl fmt::Debug for IpPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Omit => formatter.write_str("IpPolicy::Omit"),
            Self::Fixed { family, .. } => formatter
                .debug_struct("IpPolicy::Fixed")
                .field("family", family)
                .field("value", &"[redacted]")
                .finish(),
            Self::FromRequest { family } => formatter
                .debug_struct("IpPolicy::FromRequest")
                .field("family", family)
                .finish(),
        }
    }
}

impl IpPolicy {
    pub fn fixed(family: IpFamily, value: impl Into<String>) -> Result<Self, ProfileError> {
        let value = value.into();
        validate_ip(&value, family)?;
        Ok(Self::Fixed { family, value })
    }

    pub fn from_request(family: IpFamily) -> Self {
        Self::FromRequest { family }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunetProfile {
    pub auth: AuthFamily,
    pub endpoint: HttpsEndpoint,
    pub paths: TunetPaths,
    pub methods: MethodProfile,
    pub callbacks: CallbackProfile,
    pub ac_id: AcIdPolicy,
    pub ip: IpPolicy,
    pub outer_digest: OuterDigestScheme,
}

/// Optional replacements for the values in a current auth4/auth6 profile.
///
/// The current profile constructors set the verified `srun_bx1` digest
/// profile.  All transport-facing fields remain replaceable so a deployment
/// can adapt a host, path, method, callback, `ac_id`, or address policy while
/// the constructor still rejects an unresolved digest convention.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunetProfileOverrides {
    pub endpoint: Option<HttpsEndpoint>,
    pub paths: Option<TunetPaths>,
    pub methods: Option<MethodProfile>,
    pub callbacks: Option<CallbackProfile>,
    pub ac_id: Option<AcIdPolicy>,
    pub ip: Option<IpPolicy>,
    pub outer_digest: Option<OuterDigestScheme>,
}

impl TunetProfile {
    /// Construct the current TUNet profile for one IP family.
    pub fn current(auth: AuthFamily) -> Result<Self, ProfileError> {
        Self::current_with_overrides(auth, TunetProfileOverrides::default())
    }

    /// Construct the current IPv4 portal profile.
    pub fn current_auth4() -> Result<Self, ProfileError> {
        Self::current(AuthFamily::Auth4)
    }

    /// Construct the current IPv6 portal profile.
    pub fn current_auth6() -> Result<Self, ProfileError> {
        Self::current(AuthFamily::Auth6)
    }

    /// Construct a current profile while replacing selected deployment data.
    ///
    /// The current public pages use the same SRun `srun_bx1` material on both
    /// hosts.  An override may therefore change transport details, but an
    /// unspecified or historical digest candidate is rejected here instead
    /// of being guessed or automatically retried.
    pub fn current_with_overrides(
        auth: AuthFamily,
        overrides: TunetProfileOverrides,
    ) -> Result<Self, ProfileError> {
        let (host, ac_id, ip_family) = match auth {
            AuthFamily::Auth4 => (CURRENT_AUTH4_HOST, CURRENT_AUTH4_AC_ID, IpFamily::V4),
            AuthFamily::Auth6 => (CURRENT_AUTH6_HOST, CURRENT_AUTH6_AC_ID, IpFamily::V6),
        };

        let endpoint = match overrides.endpoint {
            Some(endpoint) => endpoint,
            None => HttpsEndpoint::new(host, 443)?,
        };
        let paths = match overrides.paths {
            Some(paths) => paths,
            None => TunetPaths::new(
                CURRENT_CHALLENGE_PATH,
                CURRENT_PORTAL_PATH,
                CURRENT_STATUS_PATH,
            )?,
        };
        let methods = overrides.methods.unwrap_or_else(|| {
            MethodProfile::new(
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
            )
        });
        let callbacks = match overrides.callbacks {
            Some(callbacks) => callbacks,
            None => CallbackProfile::new(
                Some(CURRENT_CHALLENGE_CALLBACK),
                Some(CURRENT_PORTAL_CALLBACK),
                Some(CURRENT_STATUS_CALLBACK),
            )?,
        };
        let ac_id = match overrides.ac_id {
            Some(ac_id) => ac_id,
            None => AcIdPolicy::fixed_for_portal(ac_id)?,
        };
        let ip = overrides
            .ip
            .unwrap_or_else(|| IpPolicy::from_request(ip_family));
        let outer_digest = overrides
            .outer_digest
            .unwrap_or(OuterDigestScheme::CandidateHmacMd5Hex);

        let profile = Self::new(
            auth,
            endpoint,
            paths,
            methods,
            callbacks,
            ac_id,
            ip,
            outer_digest,
        )?;
        profile.validate_srun_digest()?;
        Ok(profile)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auth: AuthFamily,
        endpoint: HttpsEndpoint,
        paths: TunetPaths,
        methods: MethodProfile,
        callbacks: CallbackProfile,
        ac_id: AcIdPolicy,
        ip: IpPolicy,
        outer_digest: OuterDigestScheme,
    ) -> Result<Self, ProfileError> {
        let expected_ip_family = auth.ip_family();

        match &ip {
            IpPolicy::Omit => {}
            IpPolicy::Fixed { family, value } => {
                if *family != expected_ip_family {
                    return Err(ProfileError::IpFamilyMismatch {
                        expected: expected_ip_family,
                        actual: *family,
                    });
                }
                validate_ip(value, *family)?;
            }
            IpPolicy::FromRequest { family } => {
                if *family != expected_ip_family {
                    return Err(ProfileError::IpFamilyMismatch {
                        expected: expected_ip_family,
                        actual: *family,
                    });
                }
            }
        }

        let profile = Self {
            auth,
            endpoint,
            paths,
            methods,
            callbacks,
            ac_id,
            ip,
            outer_digest,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Validate all public profile data before it reaches an HTTP request.
    /// The constructors validate their own inputs, but the profile fields are
    /// intentionally public so callers can deserialize or modify a profile.
    /// Rechecking here prevents a malformed path, callback, policy value, or
    /// address family from becoming a wire request.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.endpoint.host.trim().is_empty() {
            return Err(ProfileError::EmptyHost);
        }
        if self.endpoint.port == 0 {
            return Err(ProfileError::InvalidPort);
        }
        if !valid_host(&self.endpoint.host) {
            return Err(ProfileError::InvalidHost);
        }
        for path in [
            &self.paths.challenge,
            &self.paths.portal,
            &self.paths.status,
        ] {
            if !valid_path(path) {
                return Err(ProfileError::InvalidPath(path.clone()));
            }
        }
        for callback in [
            &self.callbacks.challenge,
            &self.callbacks.portal,
            &self.callbacks.status,
        ] {
            if let Some(callback) = callback.as_deref()
                && !valid_callback(callback)
            {
                return Err(ProfileError::InvalidCallback(callback.to_owned()));
            }
        }
        match &self.ac_id {
            AcIdPolicy::Fixed(value) | AcIdPolicy::FixedForPortal(value) => {
                if value.is_empty() {
                    return Err(ProfileError::EmptyPolicyValue(AC_ID_PARAMETER));
                }
                validate_parameter_value(AC_ID_PARAMETER, value)?;
            }
            _ => {}
        }

        match &self.ip {
            IpPolicy::Omit => {}
            IpPolicy::Fixed { family, value } => {
                if *family != self.auth.ip_family() {
                    return Err(ProfileError::IpFamilyMismatch {
                        expected: self.auth.ip_family(),
                        actual: *family,
                    });
                }
                validate_ip(value, *family)?;
            }
            IpPolicy::FromRequest { family } if *family != self.auth.ip_family() => {
                return Err(ProfileError::IpFamilyMismatch {
                    expected: self.auth.ip_family(),
                    actual: *family,
                });
            }
            IpPolicy::FromRequest { .. } => {}
        }
        Ok(())
    }

    /// Ensure the profile selects the current Rust-owned SRun digest.
    pub fn validate_srun_digest(&self) -> Result<(), ProfileError> {
        if self.outer_digest == OuterDigestScheme::CandidateHmacMd5Hex {
            Ok(())
        } else {
            Err(ProfileError::UnsupportedDigestScheme(self.outer_digest))
        }
    }

    pub fn build_request(
        &self,
        operation: TunetOperation,
        params: &[(&str, &str)],
    ) -> Result<TunetRequest, ProfileError> {
        let owned_params = params
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        self.build_owned_request(operation, owned_params)
    }

    pub fn build_login_request(
        &self,
        username: &str,
        password: &OuterDigest,
        extra_params: &[(&str, &str)],
    ) -> Result<TunetRequest, ProfileError> {
        if username.is_empty() {
            return Err(ProfileError::EmptyPolicyValue("username"));
        }
        if self.outer_digest != OuterDigestScheme::Unspecified
            && self.outer_digest != password.scheme
        {
            return Err(ProfileError::DigestSchemeMismatch {
                expected: self.outer_digest,
                actual: password.scheme,
            });
        }

        let mut params = extra_params
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<Vec<_>>();
        params.push(("username".to_owned(), username.to_owned()));
        params.push(("password".to_owned(), password.wire_value.clone()));

        self.build_owned_request(TunetOperation::Login, params)
    }

    fn build_owned_request(
        &self,
        operation: TunetOperation,
        mut params: Vec<(String, String)>,
    ) -> Result<TunetRequest, ProfileError> {
        validate_parameters(&params)?;

        ensure_operation_action(&mut params, operation)?;

        match (&self.ac_id, operation) {
            (AcIdPolicy::Omit, _) => {
                reject_supplied_parameter(&params, AC_ID_PARAMETER)?;
            }
            (AcIdPolicy::Fixed(value), _) => {
                add_profile_parameter(&mut params, AC_ID_PARAMETER, value)?
            }
            (AcIdPolicy::FromRequest, _) => {
                require_nonempty_parameter(&params, AC_ID_PARAMETER)?;
            }
            (AcIdPolicy::FixedForPortal(value), TunetOperation::Login | TunetOperation::Logout) => {
                add_profile_parameter(&mut params, AC_ID_PARAMETER, value)?
            }
            (AcIdPolicy::FixedForPortal(_), _) => {
                reject_supplied_parameter(&params, AC_ID_PARAMETER)?;
            }
            (AcIdPolicy::FromRequestForPortal, TunetOperation::Login | TunetOperation::Logout) => {
                require_nonempty_parameter(&params, AC_ID_PARAMETER)?;
            }
            (AcIdPolicy::FromRequestForPortal, _) => {
                reject_supplied_parameter(&params, AC_ID_PARAMETER)?;
            }
        }

        match &self.ip {
            IpPolicy::Omit => {
                reject_supplied_parameter(&params, IP_PARAMETER)?;
            }
            IpPolicy::Fixed { family, value } => {
                add_profile_parameter(&mut params, IP_PARAMETER, value)?;
                validate_ip(value, *family)?;
            }
            IpPolicy::FromRequest { family } => {
                let value = require_nonempty_parameter(&params, IP_PARAMETER)?;
                validate_ip(value, *family)?;
            }
        }

        // Portal.js appends JSONP's callback after the operation parameters.
        // Keep the profile-owned `ac_id` and requested IP before it so the
        // generated query/form has the same stable field order as the live
        // desktop portal.  Servers should not depend on ordering, but a
        // request plan that mirrors the observed client is easier to audit.
        if let Some(callback) = self.callbacks.callback(operation) {
            add_profile_parameter(&mut params, CALLBACK_PARAMETER, callback)?;
        }

        reorder_portal_parameters(&mut params, operation);

        let method = self.methods.method(operation);
        let path = self.paths.path(operation);
        let base_url = self.endpoint.path_url(path)?;
        let encoded = encode_parameters(&params, method == HttpMethod::Post)?;

        let (url, body) = match method {
            HttpMethod::Get => {
                let url = if encoded.is_empty() {
                    base_url
                } else {
                    format!("{base_url}?{encoded}")
                };
                (url, None)
            }
            HttpMethod::Post => (base_url, Some(encoded)),
        };

        let request = TunetRequest {
            operation,
            method,
            url,
            body,
            parameters: params,
        };
        self.validate_request(&request)?;
        Ok(request)
    }

    /// Revalidate a public request plan immediately before it reaches the
    /// transport. `TunetRequest` deliberately exposes its fields so callers
    /// can inspect a planned request; that also means a caller can mutate it
    /// after construction. Rechecking the operation, profile-owned
    /// parameters, method, and encoded target keeps such a mutation from
    /// silently becoming a different wire request.
    pub fn validate_request(&self, request: &TunetRequest) -> Result<(), ProfileError> {
        self.validate()?;

        if request.method != self.methods.method(request.operation) {
            return Err(ProfileError::RequestMethodMismatch {
                operation: request.operation,
            });
        }
        validate_parameters(&request.parameters)?;
        validate_operation_parameters(self, request.operation, &request.parameters)?;

        let base_url = self.endpoint.path_url(self.paths.path(request.operation))?;
        let encoded = encode_parameters(&request.parameters, request.method == HttpMethod::Post)?;
        let expected_url = match request.method {
            HttpMethod::Get if encoded.is_empty() => base_url,
            HttpMethod::Get => format!("{base_url}?{encoded}"),
            HttpMethod::Post => base_url,
        };
        if request.url != expected_url {
            return Err(ProfileError::InvalidRequestEncoding);
        }

        match request.method {
            HttpMethod::Get => {
                if request.body.is_some() {
                    return Err(ProfileError::InvalidRequestEncoding);
                }
            }
            HttpMethod::Post => {
                if request.body.as_deref() != Some(encoded.as_str()) {
                    return Err(ProfileError::InvalidRequestEncoding);
                }
            }
        }

        Ok(())
    }
}

fn reorder_portal_parameters(params: &mut Vec<(String, String)>, operation: TunetOperation) {
    let order: &[&str] = match operation {
        TunetOperation::Login => &[
            "action",
            "username",
            "password",
            "os",
            "name",
            "double_stack",
            "chksum",
            "info",
            "ac_id",
            "ip",
            "n",
            "type",
            CALLBACK_PARAMETER,
        ],
        TunetOperation::Logout => &["action", "username", "ip", "ac_id", CALLBACK_PARAMETER],
        _ => return,
    };

    let mut reordered = Vec::with_capacity(params.len());
    for wanted in order {
        if let Some(position) = params.iter().position(|(name, _)| name == wanted) {
            reordered.push(params.remove(position));
        }
    }
    reordered.append(params);
    *params = reordered;
}

#[derive(Clone, PartialEq, Eq)]
pub struct TunetRequest {
    pub operation: TunetOperation,
    pub method: HttpMethod,
    pub url: String,
    pub body: Option<String>,
    pub parameters: Vec<(String, String)>,
}

impl fmt::Debug for TunetRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TunetRequest")
            .field("operation", &self.operation)
            .field("method", &self.method)
            .field("url", &redact_encoded_parameter_text(&self.url, true))
            .field(
                "body",
                &self
                    .body
                    .as_deref()
                    .map(|body| redact_encoded_parameter_text(body, false)),
            )
            .field("parameters", &RedactedParameters(&self.parameters))
            .finish()
    }
}

struct RedactedParameters<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedParameters<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|(name, value)| {
                let value = if is_sensitive_parameter(name) {
                    "[redacted]"
                } else {
                    value.as_str()
                };
                (name, value)
            }))
            .finish()
    }
}

fn redact_encoded_parameter_text(value: &str, is_url: bool) -> String {
    let (prefix, query) = if is_url {
        value
            .split_once('?')
            .map_or(("", value), |(prefix, query)| (prefix, query))
    } else {
        ("", value)
    };
    let redacted = query
        .split('&')
        .map(|pair| {
            let name = pair.split_once('=').map_or(pair, |(name, _)| name);
            if is_sensitive_parameter(name) {
                format!("{name}=[redacted]")
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    if is_url && !prefix.is_empty() {
        format!("{prefix}?{redacted}")
    } else {
        redacted
    }
}

fn is_sensitive_parameter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "username"
        || name == "user"
        || name == "ip"
        || name == "ip6"
        || name == "password"
        || name.contains("pass")
        || name == "info"
        || name == "chksum"
        || name.contains("challenge")
        || name.contains("token")
        || name.contains("ticket")
        || name.contains("cookie")
        || name.contains("csrf")
        || name.contains("verify")
        || name == "user_mac"
        || name == "mac"
}

#[derive(Clone, PartialEq, Eq)]
pub enum ProfileError {
    EmptyHost,
    InvalidHost,
    InvalidPort,
    InvalidPath(String),
    InvalidCallback(String),
    EmptyPolicyValue(&'static str),
    EmptyDigest,
    InvalidDigestValue,
    MissingParameter(&'static str),
    InvalidParameterName,
    InvalidParameterValue(String),
    DuplicateParameter(String),
    ConflictingParameter(String),
    RequestMethodMismatch {
        operation: TunetOperation,
    },
    InvalidRequestEncoding,
    InvalidIp {
        expected: IpFamily,
        value: String,
    },
    IpFamilyMismatch {
        expected: IpFamily,
        actual: IpFamily,
    },
    DigestSchemeMismatch {
        expected: OuterDigestScheme,
        actual: OuterDigestScheme,
    },
    UnsupportedDigestScheme(OuterDigestScheme),
}

impl fmt::Debug for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The Display form deliberately omits the offending IP address.  Use
        // that same safe form for Debug so a profile validation error cannot
        // reintroduce a personal address through a derived formatter.
        formatter.write_str(&self.to_string())
    }
}

impl Display for ProfileError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyHost => formatter.write_str("HTTPS endpoint host must not be empty"),
            Self::InvalidHost => formatter.write_str("HTTPS endpoint host contains URL syntax"),
            Self::InvalidPort => formatter.write_str("HTTPS endpoint port must be non-zero"),
            Self::InvalidPath(path) => write!(formatter, "invalid endpoint path: {path}"),
            Self::InvalidCallback(callback) => {
                write!(formatter, "invalid JSONP callback name: {callback}")
            }
            Self::EmptyPolicyValue(name) => write!(formatter, "{name} must not be empty"),
            Self::EmptyDigest => formatter.write_str("digest wire value must not be empty"),
            Self::InvalidDigestValue => {
                formatter.write_str("digest wire value contains a request delimiter")
            }
            Self::MissingParameter(name) => write!(formatter, "missing required parameter: {name}"),
            Self::InvalidParameterName => formatter.write_str("parameter name must not be empty"),
            Self::InvalidParameterValue(name) => {
                write!(
                    formatter,
                    "parameter value contains a control character: {name}"
                )
            }
            Self::DuplicateParameter(name) => write!(formatter, "duplicate parameter: {name}"),
            Self::ConflictingParameter(name) => {
                write!(
                    formatter,
                    "profile conflicts with supplied parameter: {name}"
                )
            }
            Self::RequestMethodMismatch { operation } => {
                write!(
                    formatter,
                    "HTTP method does not match TUNet {operation:?} profile"
                )
            }
            Self::InvalidRequestEncoding => {
                formatter.write_str("request URL and body do not match encoded parameters")
            }
            Self::InvalidIp { expected, .. } => {
                write!(formatter, "value is not a valid {expected} address")
            }
            Self::IpFamilyMismatch { expected, actual } => {
                write!(formatter, "profile requires {expected}, received {actual}")
            }
            Self::DigestSchemeMismatch { expected, actual } => {
                write!(
                    formatter,
                    "digest scheme mismatch: profile={expected:?}, value={actual:?}"
                )
            }
            Self::UnsupportedDigestScheme(scheme) => {
                write!(
                    formatter,
                    "unsupported or unresolved digest scheme: {scheme:?}"
                )
            }
        }
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseDecodeError {
    Empty,
    MissingJsonpWrapper,
    InvalidJsonpCallback,
    UnexpectedJsonpCallback { expected: String, actual: String },
    EmptyJsonpPayload,
    TrailingJsonpData,
    NotCommaStatus,
}

impl Display for ResponseDecodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("response body is empty"),
            Self::MissingJsonpWrapper => formatter.write_str("response is not a JSONP wrapper"),
            Self::InvalidJsonpCallback => formatter.write_str("invalid JSONP callback name"),
            Self::UnexpectedJsonpCallback { expected, actual } => {
                write!(
                    formatter,
                    "unexpected JSONP callback: expected {expected}, got {actual}"
                )
            }
            Self::EmptyJsonpPayload => formatter.write_str("JSONP payload is empty"),
            Self::TrailingJsonpData => formatter.write_str("JSONP wrapper has trailing data"),
            Self::NotCommaStatus => formatter.write_str("response is not a comma status record"),
        }
    }
}

impl std::error::Error for ResponseDecodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommaStatusSignal {
    Positive,
    Negative,
    Unknown,
}

#[derive(Clone, PartialEq, Eq)]
pub struct JsonpPayload<'a> {
    pub callback: &'a str,
    pub payload: &'a str,
}

impl fmt::Debug for JsonpPayload<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonpPayload")
            .field("callback", &self.callback)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CommaStatus<'a> {
    pub fields: Vec<&'a str>,
}

impl fmt::Debug for CommaStatus<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommaStatus")
            .field("field_count", &self.fields.len())
            .finish()
    }
}

impl CommaStatus<'_> {
    pub fn signal(&self) -> CommaStatusSignal {
        match self.fields.first().map(|field| field.to_ascii_lowercase()) {
            Some(value) if matches!(value.as_str(), "ok" | "success") => {
                CommaStatusSignal::Positive
            }
            Some(value) if matches!(value.as_str(), "error" | "fail" | "failed") => {
                CommaStatusSignal::Negative
            }
            _ => CommaStatusSignal::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusResponse<'a> {
    Jsonp(JsonpPayload<'a>),
    Comma(CommaStatus<'a>),
}

/// Unwrap one JSONP call without parsing the payload as JSON.
///
/// `expected_callback` is optional because some profiles return a generated
/// callback name.  Passing it is preferable when the profile knows the name;
/// a mismatched callback is rejected instead of silently accepting another
/// script wrapper.  A single trailing semicolon is accepted.
pub fn decode_jsonp<'a>(
    body: &'a str,
    expected_callback: Option<&str>,
) -> Result<JsonpPayload<'a>, ResponseDecodeError> {
    let trimmed = strip_utf8_bom(body).trim();
    if trimmed.is_empty() {
        return Err(ResponseDecodeError::Empty);
    }

    let without_semicolon = trimmed
        .strip_suffix(';')
        .map(str::trim_end)
        .unwrap_or(trimmed);
    let open = without_semicolon
        .find('(')
        .ok_or(ResponseDecodeError::MissingJsonpWrapper)?;
    let close = without_semicolon
        .rfind(')')
        .ok_or(ResponseDecodeError::MissingJsonpWrapper)?;
    if close <= open {
        return Err(ResponseDecodeError::EmptyJsonpPayload);
    }
    if close + 1 != without_semicolon.len() {
        return Err(ResponseDecodeError::TrailingJsonpData);
    }

    let callback = without_semicolon[..open].trim();
    if !valid_callback(callback) {
        return Err(ResponseDecodeError::InvalidJsonpCallback);
    }
    if let Some(expected) = expected_callback {
        if callback != expected {
            return Err(ResponseDecodeError::UnexpectedJsonpCallback {
                expected: expected.to_owned(),
                actual: callback.to_owned(),
            });
        }
    }

    let payload = without_semicolon[open + 1..close].trim();
    if payload.is_empty() {
        return Err(ResponseDecodeError::EmptyJsonpPayload);
    }

    Ok(JsonpPayload { callback, payload })
}

/// Parse the comma-delimited status form conservatively.
///
/// This intentionally preserves every field, including empty trailing fields,
/// and only provides a hint for a small set of exact first-field tokens.  It
/// does not assign meaning to field positions and therefore cannot by itself
/// establish that a user is online or that authentication succeeded.
pub fn parse_comma_status(body: &str) -> Result<CommaStatus<'_>, ResponseDecodeError> {
    let trimmed = strip_utf8_bom(body).trim();
    if trimmed.is_empty() {
        return Err(ResponseDecodeError::Empty);
    }
    if trimmed.starts_with('<')
        || trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('"')
        || trimmed.contains('(')
        || trimmed.contains(')')
    {
        return Err(ResponseDecodeError::NotCommaStatus);
    }

    let fields: Vec<_> = trimmed.split(',').map(str::trim).collect();
    if fields.len() < 2 || fields.iter().any(|field| field.contains(['\r', '\n'])) {
        return Err(ResponseDecodeError::NotCommaStatus);
    }

    Ok(CommaStatus { fields })
}

pub fn decode_status_response<'a>(
    body: &'a str,
    expected_callback: Option<&str>,
) -> Result<StatusResponse<'a>, ResponseDecodeError> {
    let trimmed = strip_utf8_bom(body).trim();
    if trimmed.is_empty() {
        return Err(ResponseDecodeError::Empty);
    }

    if let Some(open) = trimmed.find('(') {
        let candidate_callback = trimmed[..open].trim();
        if !candidate_callback.is_empty() && valid_callback(candidate_callback) {
            return decode_jsonp(trimmed, expected_callback).map(StatusResponse::Jsonp);
        }
    }

    parse_comma_status(trimmed).map(StatusResponse::Comma)
}

fn strip_utf8_bom(body: &str) -> &str {
    body.strip_prefix('\u{feff}').unwrap_or(body)
}

fn checked_callback(callback: Option<&str>) -> Result<Option<String>, ProfileError> {
    callback
        .map(|callback| {
            if valid_callback(callback) {
                Ok(callback.to_owned())
            } else {
                Err(ProfileError::InvalidCallback(callback.to_owned()))
            }
        })
        .transpose()
}

fn ensure_operation_action(
    params: &mut Vec<(String, String)>,
    operation: TunetOperation,
) -> Result<(), ProfileError> {
    let Some(expected) = (match operation {
        TunetOperation::Login => Some("login"),
        TunetOperation::Logout => Some("logout"),
        TunetOperation::Challenge | TunetOperation::Status => None,
    }) else {
        return Ok(());
    };

    if let Some((_, value)) = params.iter().find(|(name, _)| name == "action") {
        if value == expected {
            return Ok(());
        }
        return Err(ProfileError::ConflictingParameter("action".to_owned()));
    }

    params.insert(0, ("action".to_owned(), expected.to_owned()));
    Ok(())
}

fn add_profile_parameter(
    params: &mut Vec<(String, String)>,
    name: &str,
    value: &str,
) -> Result<(), ProfileError> {
    validate_parameter_name(name)?;
    validate_parameter_value(name, value)?;
    if params.iter().any(|(parameter, _)| parameter == name) {
        return Err(ProfileError::ConflictingParameter(name.to_owned()));
    }
    params.push((name.to_owned(), value.to_owned()));
    Ok(())
}

fn require_nonempty_parameter<'a>(
    params: &'a [(String, String)],
    name: &'static str,
) -> Result<&'a str, ProfileError> {
    params
        .iter()
        .find(|(parameter, _)| parameter == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or(ProfileError::MissingParameter(name))
}

fn validate_parameters(params: &[(String, String)]) -> Result<(), ProfileError> {
    for (index, (name, value)) in params.iter().enumerate() {
        validate_parameter_name(name)?;
        validate_parameter_value(name, value)?;
        if params[..index].iter().any(|(previous, _)| previous == name) {
            return Err(ProfileError::DuplicateParameter(name.clone()));
        }
    }
    Ok(())
}

fn validate_parameter_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'[' | b']')
        })
    {
        return Err(ProfileError::InvalidParameterName);
    }
    Ok(())
}

fn validate_parameter_value(name: &str, value: &str) -> Result<(), ProfileError> {
    // Query/form encoding handles ordinary spaces and punctuation. Control
    // characters are different: they can change the HTTP request framing or
    // make a proxy interpret a value as another header/field.
    if value.chars().any(char::is_control) {
        return Err(ProfileError::InvalidParameterValue(name.to_owned()));
    }
    Ok(())
}

fn reject_supplied_parameter(params: &[(String, String)], name: &str) -> Result<(), ProfileError> {
    if params.iter().any(|(parameter, _)| parameter == name) {
        return Err(ProfileError::ConflictingParameter(name.to_owned()));
    }
    Ok(())
}

fn require_literal_parameter(
    params: &[(String, String)],
    name: &'static str,
    expected: &str,
) -> Result<(), ProfileError> {
    let actual = require_nonempty_parameter(params, name)?;
    if actual != expected {
        return Err(ProfileError::ConflictingParameter(name.to_owned()));
    }
    Ok(())
}

fn validate_operation_parameters(
    profile: &TunetProfile,
    operation: TunetOperation,
    params: &[(String, String)],
) -> Result<(), ProfileError> {
    let allowed: &[&str] = match operation {
        TunetOperation::Challenge | TunetOperation::Status => &[
            "username",
            IP_PARAMETER,
            AC_ID_PARAMETER,
            CALLBACK_PARAMETER,
        ],
        TunetOperation::Login => &[
            "action",
            "username",
            "password",
            "os",
            "name",
            "double_stack",
            "chksum",
            "info",
            AC_ID_PARAMETER,
            IP_PARAMETER,
            "n",
            "type",
            CALLBACK_PARAMETER,
        ],
        TunetOperation::Logout => &[
            "action",
            "username",
            IP_PARAMETER,
            AC_ID_PARAMETER,
            CALLBACK_PARAMETER,
        ],
    };

    for (name, _) in params {
        if !allowed.contains(&name.as_str()) {
            return Err(ProfileError::InvalidParameterName);
        }
    }

    match operation {
        TunetOperation::Challenge => {
            require_nonempty_parameter(params, "username")?;
        }
        TunetOperation::Login => {
            require_literal_parameter(params, "action", "login")?;
            require_nonempty_parameter(params, "username")?;
            require_nonempty_parameter(params, "password")?;
            if let Some((_, value)) = params.iter().find(|(name, _)| name == "n")
                && value != "200"
            {
                return Err(ProfileError::ConflictingParameter("n".to_owned()));
            }
            if let Some((_, value)) = params.iter().find(|(name, _)| name == "type")
                && value != "1"
            {
                return Err(ProfileError::ConflictingParameter("type".to_owned()));
            }
            if let Some((_, value)) = params.iter().find(|(name, _)| name == "double_stack")
                && !matches!(value.as_str(), "0" | "1")
            {
                return Err(ProfileError::ConflictingParameter(
                    "double_stack".to_owned(),
                ));
            }
        }
        TunetOperation::Logout => {
            require_literal_parameter(params, "action", "logout")?;
            require_nonempty_parameter(params, "username")?;
        }
        TunetOperation::Status => {}
    }

    match (&profile.ac_id, operation) {
        (AcIdPolicy::Omit, _) => reject_supplied_parameter(params, AC_ID_PARAMETER)?,
        (AcIdPolicy::Fixed(expected), _) => {
            require_literal_parameter(params, AC_ID_PARAMETER, expected)?;
        }
        (AcIdPolicy::FromRequest, _) => {
            require_nonempty_parameter(params, AC_ID_PARAMETER)?;
        }
        (AcIdPolicy::FixedForPortal(expected), TunetOperation::Login | TunetOperation::Logout) => {
            require_literal_parameter(params, AC_ID_PARAMETER, expected)?;
        }
        (AcIdPolicy::FixedForPortal(_), _) => {
            reject_supplied_parameter(params, AC_ID_PARAMETER)?;
        }
        (AcIdPolicy::FromRequestForPortal, TunetOperation::Login | TunetOperation::Logout) => {
            require_nonempty_parameter(params, AC_ID_PARAMETER)?;
        }
        (AcIdPolicy::FromRequestForPortal, _) => {
            reject_supplied_parameter(params, AC_ID_PARAMETER)?;
        }
    }

    match &profile.ip {
        IpPolicy::Omit => reject_supplied_parameter(params, IP_PARAMETER)?,
        IpPolicy::Fixed { family, value } => {
            let actual = require_nonempty_parameter(params, IP_PARAMETER)?;
            if actual != value {
                return Err(ProfileError::ConflictingParameter(IP_PARAMETER.to_owned()));
            }
            validate_ip(actual, *family)?;
        }
        IpPolicy::FromRequest { family } => {
            let actual = require_nonempty_parameter(params, IP_PARAMETER)?;
            validate_ip(actual, *family)?;
        }
    }

    match profile.callbacks.callback(operation) {
        Some(expected) => require_literal_parameter(params, CALLBACK_PARAMETER, expected)?,
        None => reject_supplied_parameter(params, CALLBACK_PARAMETER)?,
    }

    Ok(())
}

fn validate_ip(value: &str, expected: IpFamily) -> Result<(), ProfileError> {
    let parsed = value
        .parse::<IpAddr>()
        .map_err(|_| ProfileError::InvalidIp {
            expected,
            value: value.to_owned(),
        })?;
    let actual = match parsed {
        IpAddr::V4(_) => IpFamily::V4,
        IpAddr::V6(_) => IpFamily::V6,
    };
    if actual != expected {
        return Err(ProfileError::InvalidIp {
            expected,
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn encode_parameters(params: &[(String, String)], form: bool) -> Result<String, ProfileError> {
    validate_parameters(params)?;

    let mut encoded = String::new();
    for (index, (name, value)) in params.iter().enumerate() {
        if index != 0 {
            encoded.push('&');
        }
        encoded.push_str(&percent_encode(name, form));
        encoded.push('=');
        encoded.push_str(&percent_encode(value, form));
    }
    Ok(encoded)
}

fn percent_encode(value: &str, form: bool) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_unreserved(byte) {
            encoded.push(byte as char);
        } else if form && byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn is_unreserved(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~')
}

fn valid_host(host: &str) -> bool {
    !host.chars().any(|character| {
        character.is_control() || character.is_whitespace() || matches!(character, '/' | '?' | '#')
    }) && !host.contains("://")
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.starts_with('/')
        && !path.contains("//")
        && !path
            .chars()
            .any(|character| character.is_control() || matches!(character, '?' | '#'))
        && path.split('/').all(|segment| {
            if matches!(segment, "." | "..") {
                return false;
            }
            let Some(decoded) = decode_path_segment(segment) else {
                return false;
            };
            decoded != b"."
                && decoded != b".."
                && !decoded.contains(&b'/')
                && !decoded.contains(&b'\\')
        })
}

fn decode_path_segment(segment: &str) -> Option<Vec<u8>> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return None;
        }
        let high = hex_digit(bytes[index + 1])?;
        let low = hex_digit(bytes[index + 2])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    Some(decoded)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn valid_callback(callback: &str) -> bool {
    if callback.is_empty() {
        return false;
    }

    callback.split('.').all(valid_callback_segment)
}

fn valid_callback_segment(segment: &str) -> bool {
    let mut characters = segment.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    characters
        .all(|character| character == '_' || character == '$' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> TunetPaths {
        TunetPaths::new(
            "/cgi-bin/get_challenge",
            "/cgi-bin/srun_portal",
            "/cgi-bin/rad_user_info",
        )
        .expect("test paths are valid")
    }

    fn auth4_get_profile() -> TunetProfile {
        TunetProfile::new(
            AuthFamily::Auth4,
            HttpsEndpoint::new("auth4.tsinghua.edu.cn", 443).expect("test endpoint is valid"),
            paths(),
            MethodProfile::new(
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
            ),
            CallbackProfile::new(
                Some("challengeCallback"),
                Some("portalCallback"),
                Some("statusCallback"),
            )
            .expect("test callbacks are valid"),
            AcIdPolicy::fixed("1").expect("test ac_id is valid"),
            IpPolicy::from_request(IpFamily::V4),
            OuterDigestScheme::CandidateMd5Hex,
        )
        .expect("test profile is valid")
    }

    fn assert_current_profile(
        profile: &TunetProfile,
        auth: AuthFamily,
        host: &str,
        ac_id: &str,
        ip_family: IpFamily,
    ) {
        assert_eq!(profile.auth, auth);
        assert_eq!(profile.endpoint.host, host);
        assert_eq!(profile.endpoint.port, 443);
        assert_eq!(profile.paths.challenge, CURRENT_CHALLENGE_PATH);
        assert_eq!(profile.paths.portal, CURRENT_PORTAL_PATH);
        assert_eq!(profile.paths.status, CURRENT_STATUS_PATH);
        for operation in [
            TunetOperation::Challenge,
            TunetOperation::Login,
            TunetOperation::Logout,
            TunetOperation::Status,
        ] {
            assert_eq!(profile.methods.method(operation), HttpMethod::Get);
        }
        assert_eq!(
            profile.callbacks.challenge.as_deref(),
            Some(CURRENT_CHALLENGE_CALLBACK)
        );
        assert_eq!(
            profile.callbacks.portal.as_deref(),
            Some(CURRENT_PORTAL_CALLBACK)
        );
        assert_eq!(
            profile.callbacks.status.as_deref(),
            Some(CURRENT_STATUS_CALLBACK)
        );
        assert!(matches!(&profile.ac_id, AcIdPolicy::FixedForPortal(value) if value == ac_id));
        assert!(matches!(
            &profile.ip,
            IpPolicy::FromRequest { family: actual } if *actual == ip_family
        ));
        assert_eq!(profile.outer_digest, OuterDigestScheme::CandidateHmacMd5Hex);
    }

    #[test]
    fn current_auth4_and_auth6_profiles_match_the_public_portal_split() {
        let auth4 = TunetProfile::current_auth4().expect("current auth4 profile is valid");
        assert_current_profile(
            &auth4,
            AuthFamily::Auth4,
            CURRENT_AUTH4_HOST,
            CURRENT_AUTH4_AC_ID,
            IpFamily::V4,
        );

        let auth6 = TunetProfile::current_auth6().expect("current auth6 profile is valid");
        assert_current_profile(
            &auth6,
            AuthFamily::Auth6,
            CURRENT_AUTH6_HOST,
            CURRENT_AUTH6_AC_ID,
            IpFamily::V6,
        );
    }

    #[test]
    fn current_profile_transport_values_can_be_overridden_without_changing_auth_family() {
        let profile = TunetProfile::current_with_overrides(
            AuthFamily::Auth4,
            TunetProfileOverrides {
                endpoint: Some(HttpsEndpoint::new("127.0.0.1", 8443).unwrap()),
                paths: Some(TunetPaths::new("/challenge", "/portal", "/status").unwrap()),
                methods: Some(MethodProfile::new(
                    HttpMethod::Post,
                    HttpMethod::Post,
                    HttpMethod::Get,
                    HttpMethod::Post,
                )),
                callbacks: Some(CallbackProfile::new(None, Some("portal.cb"), None).unwrap()),
                ac_id: Some(AcIdPolicy::FromRequest),
                ip: Some(IpPolicy::fixed(IpFamily::V4, "192.0.2.10").unwrap()),
                outer_digest: None,
            },
        )
        .expect("overridden current profile is valid");

        assert_eq!(profile.auth, AuthFamily::Auth4);
        assert_eq!(profile.endpoint.host, "127.0.0.1");
        assert_eq!(profile.endpoint.port, 8443);
        assert_eq!(profile.paths.challenge, "/challenge");
        assert_eq!(profile.methods.challenge, HttpMethod::Post);
        assert_eq!(profile.methods.login, HttpMethod::Post);
        assert_eq!(profile.methods.logout, HttpMethod::Get);
        assert_eq!(profile.methods.status, HttpMethod::Post);
        assert_eq!(profile.callbacks.challenge, None);
        assert_eq!(profile.callbacks.portal.as_deref(), Some("portal.cb"));
        assert_eq!(profile.callbacks.status, None);
        assert_eq!(profile.ac_id, AcIdPolicy::FromRequest);
        assert_eq!(
            profile.ip,
            IpPolicy::Fixed {
                family: IpFamily::V4,
                value: "192.0.2.10".to_owned()
            }
        );
        assert_eq!(profile.outer_digest, OuterDigestScheme::CandidateHmacMd5Hex);
    }

    #[test]
    fn current_profile_constructor_rejects_unresolved_digest_candidates() {
        for scheme in [
            OuterDigestScheme::Unspecified,
            OuterDigestScheme::CandidatePlaintext,
            OuterDigestScheme::CandidateMd5Hex,
        ] {
            let error = TunetProfile::current_with_overrides(
                AuthFamily::Auth4,
                TunetProfileOverrides {
                    outer_digest: Some(scheme),
                    ..TunetProfileOverrides::default()
                },
            )
            .expect_err("unresolved digest must be rejected");
            assert_eq!(error, ProfileError::UnsupportedDigestScheme(scheme));
        }
    }

    #[test]
    fn endpoint_is_https_and_path_is_profile_owned() {
        let endpoint = HttpsEndpoint::new("auth6.tsinghua.edu.cn", 443).unwrap();
        assert_eq!(
            endpoint.path_url("/cgi-bin/get_challenge").unwrap(),
            "https://auth6.tsinghua.edu.cn/cgi-bin/get_challenge"
        );

        let endpoint = HttpsEndpoint::new("127.0.0.1", 8443).unwrap();
        assert_eq!(
            endpoint.path_url("/custom").unwrap(),
            "https://127.0.0.1:8443/custom"
        );
        assert!(endpoint.path_url("/custom?already=query").is_err());

        let explicit_http = HttpsEndpoint::http("127.0.0.1", 80).unwrap();
        assert_eq!(explicit_http.scheme(), TunetEndpointScheme::Http);
        assert_eq!(
            explicit_http.path_url("/fixture").unwrap(),
            "http://127.0.0.1/fixture"
        );
        assert_eq!(endpoint.scheme(), TunetEndpointScheme::Https);
    }

    #[test]
    fn endpoint_paths_reject_escape_segments_and_malformed_encoding() {
        for path in [
            "/cgi-bin/../srun_portal",
            "/cgi-bin/%2e%2e/srun_portal",
            "/cgi-bin/%2f../srun_portal",
            "/cgi-bin/%5c../srun_portal",
            "/cgi-bin/%",
            "/cgi-bin//srun_portal",
        ] {
            assert!(
                HttpsEndpoint::new("auth4.tsinghua.edu.cn", 443)
                    .expect("endpoint")
                    .path_url(path)
                    .is_err(),
                "unsafe endpoint path unexpectedly accepted: {path}"
            );
        }
    }

    #[test]
    fn auth_family_rejects_the_other_ip_family() {
        let result = TunetProfile::new(
            AuthFamily::Auth6,
            HttpsEndpoint::new("auth6.tsinghua.edu.cn", 443).unwrap(),
            paths(),
            MethodProfile::new(
                HttpMethod::Get,
                HttpMethod::Post,
                HttpMethod::Post,
                HttpMethod::Get,
            ),
            CallbackProfile::new(None, None, None).unwrap(),
            AcIdPolicy::FromRequest,
            IpPolicy::from_request(IpFamily::V4),
            OuterDigestScheme::Unspecified,
        );

        assert!(matches!(
            result,
            Err(ProfileError::IpFamilyMismatch {
                expected: IpFamily::V6,
                actual: IpFamily::V4
            })
        ));
    }

    #[test]
    fn get_profile_puts_callback_ac_id_and_ip_in_encoded_query() {
        let profile = auth4_get_profile();
        let request = profile
            .build_request(
                TunetOperation::Challenge,
                &[("username", "alice@example.edu"), ("ip", "10.0.0.8")],
            )
            .unwrap();

        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.body, None);
        assert_eq!(
            request.url,
            "https://auth4.tsinghua.edu.cn/cgi-bin/get_challenge?username=alice%40example.edu&ip=10.0.0.8&ac_id=1&callback=challengeCallback"
        );
        assert!(
            request
                .parameters
                .iter()
                .any(|(name, value)| { name == "ip" && value == "10.0.0.8" })
        );
    }

    #[test]
    fn post_profile_uses_form_encoding_and_keeps_url_clean() {
        let profile = TunetProfile::new(
            AuthFamily::Auth6,
            HttpsEndpoint::new("auth6.tsinghua.edu.cn", 443).unwrap(),
            paths(),
            MethodProfile::new(
                HttpMethod::Get,
                HttpMethod::Post,
                HttpMethod::Post,
                HttpMethod::Post,
            ),
            CallbackProfile::new(None, Some("portal.cb"), None).unwrap(),
            AcIdPolicy::fixed("2").unwrap(),
            IpPolicy::fixed(IpFamily::V6, "2001:db8::8").unwrap(),
            OuterDigestScheme::CandidateHmacMd5Hex,
        )
        .unwrap();
        let digest =
            OuterDigest::new(OuterDigestScheme::CandidateHmacMd5Hex, "digest+with spaces").unwrap();

        let request = profile
            .build_login_request("alice", &digest, &[("info", "a+b c"), ("type", "1")])
            .unwrap();

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(
            request.url,
            "https://auth6.tsinghua.edu.cn/cgi-bin/srun_portal"
        );
        assert_eq!(
            request.body.as_deref(),
            Some(
                "action=login&username=alice&password=digest%2Bwith+spaces&info=a%2Bb+c&ac_id=2&ip=2001%3Adb8%3A%3A8&type=1&callback=portal.cb"
            )
        );
    }

    #[test]
    fn digest_scheme_is_metadata_and_must_match_the_profile_when_selected() {
        let profile = auth4_get_profile();
        let digest =
            OuterDigest::new(OuterDigestScheme::CandidateHmacMd5Hex, "wire-value").unwrap();
        let error = profile
            .build_login_request("alice", &digest, &[])
            .expect_err("different unverified candidate must be visible");
        assert!(matches!(
            error,
            ProfileError::DigestSchemeMismatch {
                expected: OuterDigestScheme::CandidateMd5Hex,
                actual: OuterDigestScheme::CandidateHmacMd5Hex
            }
        ));

        let unspecified = OuterDigest::new(OuterDigestScheme::Unspecified, "caller-value").unwrap();
        let profile = TunetProfile::new(
            AuthFamily::Auth4,
            HttpsEndpoint::new("auth4.tsinghua.edu.cn", 443).unwrap(),
            paths(),
            MethodProfile::new(
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
                HttpMethod::Get,
            ),
            CallbackProfile::new(None, None, None).unwrap(),
            AcIdPolicy::Omit,
            IpPolicy::Omit,
            OuterDigestScheme::Unspecified,
        )
        .unwrap();
        let request = profile
            .build_login_request("alice", &unspecified, &[])
            .unwrap();
        assert!(request.url.contains("password=caller-value"));
    }

    #[test]
    fn jsonp_decoder_checks_callback_and_allows_one_semicolon() {
        let decoded = decode_jsonp(
            "portalCallback({\"error\":\"ok\"});",
            Some("portalCallback"),
        )
        .unwrap();
        assert_eq!(decoded.callback, "portalCallback");
        assert_eq!(decoded.payload, "{\"error\":\"ok\"}");

        assert!(matches!(
            decode_jsonp("other({})", Some("portalCallback")),
            Err(ResponseDecodeError::UnexpectedJsonpCallback { .. })
        ));
        assert!(matches!(
            decode_jsonp("portalCallback({}) trailing", Some("portalCallback")),
            Err(ResponseDecodeError::TrailingJsonpData)
        ));
    }

    #[test]
    fn comma_status_preserves_empty_fields_and_stays_conservative() {
        let status = parse_comma_status("ok,alice,10.0.0.8,").unwrap();
        assert_eq!(status.fields, vec!["ok", "alice", "10.0.0.8", ""]);
        assert_eq!(status.signal(), CommaStatusSignal::Positive);

        let unknown = parse_comma_status("0,alice,10.0.0.8").unwrap();
        assert_eq!(unknown.signal(), CommaStatusSignal::Unknown);
        assert!(parse_comma_status("{\"error\":\"ok\"}").is_err());
    }

    #[test]
    fn response_decoders_strip_bom_and_reject_html_as_comma_data() {
        let decoded = decode_jsonp("\u{feff}portalCallback({\"error\":\"ok\"});", None)
            .expect("BOM-prefixed JSONP should decode");
        assert_eq!(decoded.callback, "portalCallback");
        assert!(matches!(
            parse_comma_status("<html><body>error</body></html>"),
            Err(ResponseDecodeError::NotCommaStatus)
        ));
    }

    #[test]
    fn profile_and_request_debug_output_redacts_network_and_protocol_values() {
        let policy = IpPolicy::fixed(IpFamily::V4, "192.0.2.10").unwrap();
        assert!(!format!("{policy:?}").contains("192.0.2.10"));

        let profile = TunetProfile::current_with_overrides(
            AuthFamily::Auth4,
            TunetProfileOverrides {
                ip: Some(policy),
                ..TunetProfileOverrides::default()
            },
        )
        .unwrap();
        assert!(!format!("{profile:?}").contains("192.0.2.10"));

        let error = IpPolicy::fixed(IpFamily::V6, "192.0.2.10").unwrap_err();
        let text = format!("{error:?} {error}");
        assert!(!text.contains("192.0.2.10"));
    }

    #[test]
    fn status_decoder_keeps_jsonp_and_comma_responses_distinct() {
        let jsonp =
            decode_status_response("radUserInfo({\"online\":true})", Some("radUserInfo")).unwrap();
        assert!(matches!(jsonp, StatusResponse::Jsonp(_)));

        let comma = decode_status_response("ok,user,10.0.0.8", None).unwrap();
        assert!(matches!(comma, StatusResponse::Comma(_)));
    }

    #[test]
    fn duplicate_or_missing_profile_parameters_are_rejected() {
        let profile = auth4_get_profile();
        assert!(matches!(
            profile.build_request(
                TunetOperation::Challenge,
                &[("username", "alice"), ("ip", "10.0.0.8"), ("ip", "10.0.0.9")]
            ),
            Err(ProfileError::DuplicateParameter(name)) if name == "ip"
        ));

        assert!(matches!(
            profile.build_request(TunetOperation::Challenge, &[("username", "alice")]),
            Err(ProfileError::MissingParameter("ip"))
        ));
    }

    #[test]
    fn callback_names_are_profile_data_and_not_global_constants() {
        let callbacks = CallbackProfile::new(Some("jQuery_123"), Some("dr1003"), None).unwrap();
        assert_eq!(callbacks.challenge.as_deref(), Some("jQuery_123"));
        assert_eq!(callbacks.portal.as_deref(), Some("dr1003"));
        assert_eq!(callbacks.status, None);
        assert!(CallbackProfile::new(Some("invalid-callback"), None, None).is_err());
    }
}
