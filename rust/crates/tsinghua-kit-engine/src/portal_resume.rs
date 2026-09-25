//! Cookie-only browser navigation for a restored WebVPN/INFO session.
//!
//! Navigation is NOT authentication proof. The caller must subsequently
//! verify INFO's target CSRF and the account-bound grjbxx response. This
//! helper has no password parameter and never retries a failed POST.

use crate::{
    identity_client::LoginFailureReason,
    identity_execution::{IdentityExecutionClient, IdentityHttpResponse},
    webvpn_identity::WebVpnIdentityConfig,
};
use reqwest::{StatusCode, Url, header::LOCATION};
use std::collections::HashSet;

const INFO_ORIGIN: &str = "https://info.tsinghua.edu.cn/";
const PORTAL_APP: &str = "10000ea055dd8d81d09d5a1ba55d39ad";
// The Learn identity application is the public, fixed route whose login page
// is known to offer the server-selected checkSingle trusted-device flow.  It
// is used only to renew the shared Identity cookie jar; Learn itself is not
// marked authenticated here.
const TRUSTED_DEVICE_APP: &str = "bb5df85216504820be7bba2b0ae1535b";
const LEARN_ORIGIN: &str = "https://learn.tsinghua.edu.cn/";
const IDENTITY_WEBVPN_PREFIX: &str =
    "/https/77726476706e69737468656265737421f9f30f8834396657761d88e29d51367bcfe7/";
const MAX_BODY: usize = 2 * 1024 * 1024;

#[cfg(test)]
#[path = "portal_thuinfo_tests.rs"]
mod thuinfo_tests;

#[cfg(test)]
#[path = "portal_boundary_repair_tests.rs"]
mod boundary_repair_tests;

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn clean(url: &Url) -> bool {
    url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && !url.as_str().chars().any(char::is_control)
}

fn normalized_webvpn_path(url: &Url) -> Option<String> {
    // OAuth can encode path separators inside a WebVPN wrapper. Compare a
    // one-pass normalized form only; never rewrite the URL sent to the server
    // and never accept double-encoding or traversal spellings.
    let lower = url.path().to_ascii_lowercase();
    if !safe_uri_octets(url.path())
        || lower.contains("%25")
        || lower.contains("%2e")
        || lower.contains("%5c")
        || url.path().contains('\\')
    {
        return None;
    }
    let path = url.path().replace("%2F", "/").replace("%2f", "/");
    if path.split('/').any(|part| matches!(part, "." | "..")) {
        return None;
    }
    Some(path)
}

fn mapped_info_path(info_prefix: &str, url: &Url) -> bool {
    // A gateway may canonicalize /<mapping>/ to /<mapping> first. This is
    // the exact same mapping root, not permission for a sibling prefix.
    normalized_webvpn_path(url).is_some_and(|path| {
        path == info_prefix.trim_end_matches('/') || path.starts_with(info_prefix)
    })
}

fn mapped_identity_path(url: &Url) -> bool {
    let Some(path) = normalized_webvpn_path(url) else {
        return false;
    };
    let Some(route) = path.strip_prefix(IDENTITY_WEBVPN_PREFIX) else {
        return false;
    };
    matches!(
        route,
        "do/off/ui/auth/login/check"
            | "do/off/ui/auth/login/checkSingle"
            | "do/off/ui/auth/login/redirect2Jsp"
    ) || route == format!("do/off/ui/auth/login/form/{PORTAL_APP}")
        || route == format!("do/off/ui/auth/login/form/{PORTAL_APP}/0")
}

/// Returns whether a WebVPN URL is one of the explicitly allowlisted
/// identity-login routes used by the portal recovery flow.  Resource probes
/// use this narrow fact to distinguish an authentication boundary from an
/// arbitrary sibling WebVPN mapping; they must not treat the whole mapping
/// as a login page.
pub(crate) fn mapped_identity_login_path(url: &Url) -> bool {
    mapped_identity_path(url)
}

/// Classify a server-selected identity form for a different campus app.
/// This is only an authentication-boundary hint for the THOS read path: the
/// URL is never dispatched or used to submit credentials by that reader.
pub(crate) fn mapped_identity_dynamic_login_form_path(url: &Url) -> bool {
    let Some(path) = normalized_webvpn_path(url) else {
        return false;
    };
    let Some(route) = path.strip_prefix(IDENTITY_WEBVPN_PREFIX) else {
        return false;
    };
    let Some(form) = route.strip_prefix("do/off/ui/auth/login/form/") else {
        return false;
    };
    let mut segments = form.split('/');
    let Some(app_id) = segments.next() else {
        return false;
    };
    if app_id.len() != 32 || !app_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    matches!(
        (segments.next(), segments.next()),
        (None, None) | (Some("0"), None)
    )
}

pub(crate) fn inside_identity_webvpn_mapping(url: &Url) -> bool {
    normalized_webvpn_path(url).is_some_and(|path| path.starts_with(IDENTITY_WEBVPN_PREFIX))
}

fn allowed(config: &WebVpnIdentityConfig, info_prefix: &str, url: &Url) -> bool {
    if !clean(url) {
        return false;
    }
    if same_origin(url, config.webvpn_origin()) {
        return matches!(url.path(), "/" | "/login")
            || mapped_info_path(info_prefix, url)
            || mapped_identity_path(url);
    }
    if same_origin(url, config.oauth_origin()) {
        return url.path() == "/lb-auth/lbredirect" || url.path().starts_with("/thu-oauth/");
    }
    if let Ok(info) = Url::parse(INFO_ORIGIN)
        && same_origin(url, &info)
    {
        let mut uri = url.path().to_owned();
        if let Some(query) = url.query() {
            uri.push('?');
            uri.push_str(query);
        }
        return safe_info_uri(&info, &uri);
    }
    if same_origin(url, config.identity_origin()) {
        return url.path().starts_with("/do/off/ui/auth/login/form/")
            || matches!(
                url.path(),
                "/do/off/ui/auth/login/checkSingle"
                    | "/do/off/ui/auth/login/redirect2Jsp"
                    | "/do/off/ui/auth/login/check"
            );
    }
    false
}

/// Match THUInfo core.ts getWebVPNUrl's wire representation exactly.
/// `uri` is already a URL path/query, not an application/x-www-form-urlencoded
/// value. Encoding it again changes '%' and '+' and gateway resource paths.
/// The allowlisted broker stays unchanged; no nested OAuth wrapping.
pub(crate) fn broker_target(
    config: &WebVpnIdentityConfig,
    target: Url,
) -> Result<Url, &'static str> {
    let info = Url::parse(INFO_ORIGIN).map_err(|_| "portal_resume_config")?;
    if !same_origin(&info, &target) {
        return Ok(target);
    }
    if !clean(&target) {
        return Err("portal_resume_sso_route");
    }
    let mut uri = target.path().to_owned();
    if let Some(query) = target.query() {
        uri.push('?');
        uri.push_str(query);
    }
    if !safe_info_uri(&info, &uri) {
        return Err("portal_broker_uri_rejected");
    }
    let mut oauth = config
        .oauth_origin()
        .join("/lb-auth/lbredirect")
        .map_err(|_| "portal_resume_config")?;
    oauth.set_query(Some(&format!(
        "scheme=https&host=info.tsinghua.edu.cn&port=443&uri={uri}"
    )));
    Ok(oauth)
}

/// Reject malformed/control escapes without writing any decoded values to a
/// log. Percent-encoding and literal plus signs otherwise stay byte-for-byte.
fn safe_uri_octets(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let byte = if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return false;
            }
            let Some(high) = (bytes[i + 1] as char).to_digit(16) else {
                return false;
            };
            let Some(low) = (bytes[i + 2] as char).to_digit(16) else {
                return false;
            };
            i += 3;
            ((high << 4) | low) as u8
        } else {
            let byte = bytes[i];
            i += 1;
            byte
        };
        if byte.is_ascii_control() || byte == b'\\' {
            return false;
        }
    }
    true
}

fn safe_info_uri(info: &Url, uri: &str) -> bool {
    if !uri.starts_with('/') || uri.starts_with("//") || !safe_uri_octets(uri) || uri.contains('#')
    {
        return false;
    }
    let path = uri
        .split('?')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.contains("%2e")
        || path.contains("%25")
        || path.split('/').any(|p| matches!(p, "." | ".."))
    {
        return false;
    }
    let Ok(resolved) = info.join(uri) else {
        return false;
    };
    if !same_origin(info, &resolved) || !clean(&resolved) {
        return false;
    }
    // THUInfo leaves the inner query unescaped. Do not let a target's query
    // inject a second broker authority field into that outer representation.
    !resolved.query_pairs().any(|(key, _)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "scheme" | "host" | "port" | "uri"
        )
    })
}

/// Accept the reference's raw URI-tail broker form, plus the old fully
/// encoded form when explicitly supplied by the server. In either form the
/// destination must be exactly INFO/HTTPS/443. Never rewrite a selected URL.
pub(crate) fn validated_info_broker(
    target: &Url,
    config: &WebVpnIdentityConfig,
    info: &Url,
) -> bool {
    if !clean(target)
        || !same_origin(target, config.oauth_origin())
        || target.path() != "/lb-auth/lbredirect"
        || !safe_uri_octets(target.query().unwrap_or_default())
    {
        return false;
    }
    let query = target.query().unwrap_or_default();
    let prefix = format!(
        "scheme=https&host={}&port=443&uri=",
        info.host_str().unwrap_or_default()
    );
    if let Some(uri) = query
        .strip_prefix(&prefix)
        .filter(|uri| uri.starts_with('/'))
    {
        return safe_info_uri(info, uri);
    }
    let mut scheme = None;
    let mut host = None;
    let mut port = None;
    let mut uri = None;
    for (key, value) in target.query_pairs() {
        let slot = match key.as_ref() {
            "scheme" => &mut scheme,
            "host" => &mut host,
            "port" => &mut port,
            "uri" => &mut uri,
            _ => return false,
        };
        if slot.replace(value.into_owned()).is_some() {
            return false;
        }
    }
    matches!((scheme.as_deref(),host.as_deref(),port.as_deref(),uri.as_deref()),
        (Some("https"),Some(host),Some("443"),Some(uri))
        if host.eq_ignore_ascii_case(info.host_str().unwrap_or_default()) && safe_info_uri(info,uri))
}

/// Log only a source-owned category, not the Location, URI/query, host or
/// token. This separates blocked mapping, malformed route and repeated URL.
fn route_class(config: &WebVpnIdentityConfig, info_prefix: &str, url: &Url) -> &'static str {
    if !clean(url) {
        return "unsafe_url_components";
    }
    if same_origin(url, config.webvpn_origin()) {
        if url.path() == "/" {
            return "webvpn_home";
        }
        if url.path() == "/login" {
            return "webvpn_login";
        }
        let Some(path) = normalized_webvpn_path(url) else {
            return "webvpn_invalid_encoding";
        };
        if path == info_prefix.trim_end_matches('/') {
            return "webvpn_info_root";
        }
        if path.starts_with(info_prefix) {
            return "webvpn_info_resource";
        }
        if mapped_identity_path(url) {
            return "webvpn_identity_route";
        }
        if path.starts_with("/wengine-vpn/") {
            return "webvpn_control_route";
        }
        if path.starts_with("/https/") || path.starts_with("/http/") {
            return "webvpn_other_mapping";
        }
        if path.starts_with("/https-") || path.starts_with("/http-") {
            return "webvpn_other_protocol";
        }
        return "webvpn_other_route";
    }
    if same_origin(url, config.oauth_origin()) {
        return if url.path() == "/lb-auth/lbredirect" {
            "oauth_broker"
        } else if url.path().starts_with("/thu-oauth/") {
            "oauth_callback"
        } else {
            "oauth_other_route"
        };
    }
    if same_origin(url, config.identity_origin()) {
        return if url.path().starts_with("/do/off/ui/auth/login/form/") {
            "identity_form"
        } else if url.path() == "/do/off/ui/auth/login/redirect2Jsp" {
            "identity_callback"
        } else if url.path() == "/do/off/ui/auth/login/checkSingle" {
            "identity_trusted_form"
        } else {
            "identity_other_route"
        };
    }
    if Url::parse(INFO_ORIGIN).is_ok_and(|info| same_origin(url, &info)) {
        return "info_direct";
    }
    "foreign_origin"
}
fn trace_route(
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
    url: &Url,
    hop: usize,
    decision: &'static str,
) {
    let route_class = route_class(config, info_prefix, url);
    if decision.starts_with("reject") {
        tracing::warn!(target:"tsinghua_kit::security",event="portal_route_decision",service="webvpn",route_class,route_decision=decision,hop=hop as u64);
    } else {
        tracing::debug!(target:"tsinghua_kit::auth",event="portal_route_decision",service="webvpn",route_class,route_decision=decision,hop=hop as u64);
    }
}

fn success_navigation_target(
    identity: &IdentityExecutionClient,
    config: &WebVpnIdentityConfig,
    page: &IdentityHttpResponse,
) -> Option<Url> {
    if !same_origin(&page.final_url, config.identity_origin())
        || !page.status.is_success()
        || !identity.client().has_identity_success_marker(page.body())
    {
        return None;
    }
    let evidence = identity.client().parse_login_page(page.body());
    if evidence
        .failure
        .as_ref()
        .is_some_and(|failure| failure.reason != LoginFailureReason::Generic)
        || evidence.second_factor_marker.is_some()
    {
        return None;
    }
    let info = Url::parse(INFO_ORIGIN).ok()?;
    [config.oauth_origin(), config.webvpn_origin(), &info]
        .into_iter()
        .find_map(|origin| {
            identity.client().first_static_redirect_url_for_origin(
                &page.final_url,
                page.body(),
                origin,
            )
        })
}

fn page_failure(page: &crate::identity_client::LoginPageEvidence) -> &'static str {
    if page.second_factor_marker.is_some() {
        return "portal_resume_second_factor_required";
    }
    match page.failure.as_ref().map(|e| e.reason) {
        Some(LoginFailureReason::SessionInvalid) => "portal_resume_identity_rejected",
        Some(LoginFailureReason::CaptchaRequired) => "portal_resume_captcha_required",
        Some(LoginFailureReason::InvalidCredentials) => "portal_resume_credential_page_rejected",
        Some(LoginFailureReason::Generic) => "portal_resume_identity_failure",
        None if page.has_login_form || page.sm2_public_key.is_some() => {
            "portal_resume_password_required"
        }
        None => "portal_resume_navigation_unconfirmed",
    }
}

fn trusted_device_entry_failure(
    identity: &IdentityExecutionClient,
    response: &IdentityHttpResponse,
) -> &'static str {
    let evidence = identity.client().parse_login_page(response.body());
    if evidence.second_factor_marker.is_some()
        || evidence
            .failure
            .as_ref()
            .is_some_and(|failure| failure.reason != LoginFailureReason::Generic)
    {
        return page_failure(&evidence);
    }
    match identity
        .client()
        .service_form_diagnostic(&response.final_url, response.body())
    {
        // An actual trusted form would normally have been consumed by
        // continue_service_check_single. If it is still here, fail closed.
        "trusted_form" => "portal_resume_trusted_unconfirmed",
        // Script text is diagnostic only and is never permission to POST.
        "trusted_script" => "portal_resume_trusted_script_only",
        "password_form" => "portal_resume_trusted_password_form",
        "form_without_key" => "portal_resume_trusted_form_without_key",
        _ => "portal_resume_trusted_no_form",
    }
}

fn terminal_reason(
    identity: &IdentityExecutionClient,
    config: &WebVpnIdentityConfig,
    response: &IdentityHttpResponse,
    submitted: bool,
) -> &'static str {
    let evidence = identity.client().parse_login_page(response.body());
    // Keep explicit expiry/2FA/captcha evidence. Generic notices must also
    // report where they occurred; otherwise a WebVPN page can masquerade as
    // an Identity failure and prompt incorrect authentication fixes.
    if evidence
        .failure
        .as_ref()
        .is_none_or(|f| f.reason != LoginFailureReason::Generic)
    {
        return page_failure(&evidence);
    }
    if same_origin(&response.final_url, config.webvpn_origin()) {
        return match response.final_url.path() {
            "/login" => "portal_webvpn_entry_generic_notice",
            "/" => "portal_webvpn_home_generic_notice",
            _ => "portal_webvpn_target_generic_notice",
        };
    }
    if same_origin(&response.final_url, config.oauth_origin()) {
        return "portal_oauth_generic_notice";
    }
    if submitted {
        return "portal_identity_submitted_generic_notice";
    }
    let shape = identity
        .client()
        .service_form_diagnostic(&response.final_url, response.body());
    if response.final_url.path().contains(PORTAL_APP) {
        return match shape {
            "trusted_form" => "portal_identity_target_trusted_form",
            "trusted_script" => "portal_identity_target_trusted_script",
            "password_form" => "portal_identity_target_password_form",
            "form_without_key" => "portal_identity_target_form_without_key",
            _ => "portal_identity_target_no_form",
        };
    }
    match shape {
        "trusted_form" => "portal_identity_entry_trusted_form",
        "trusted_script" => "portal_identity_entry_trusted_script",
        "password_form" => "portal_identity_entry_password_form",
        "form_without_key" => "portal_identity_entry_form_without_key",
        _ => "portal_identity_entry_no_form",
    }
}

/// Select the failed boundary, never infer expiry from a network/parse error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryBoundary {
    WebVpn,
    Info,
}

pub(crate) fn recovery_boundary(reason: &str) -> Option<RecoveryBoundary> {
    match reason {
        "portal_resource_webvpn_login" => Some(RecoveryBoundary::WebVpn),
        "portal_resume_csrf_missing" | "portal_resume_login_required" => {
            Some(RecoveryBoundary::Info)
        }
        _ => None,
    }
}

/// This entry is used only after the resource explicitly returned WebVPN's
/// login route. Restore the wrapper before its INFO target, using the same
/// transport. Navigation still is not account/service proof.
pub(crate) async fn recover_boundary(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
    boundary: RecoveryBoundary,
) -> Result<(), &'static str> {
    match boundary {
        RecoveryBoundary::WebVpn => {
            match navigate(identity, fingerprint, config, info_prefix).await {
                Ok(()) => Ok(()),
                // A restored WebVPN wrapper can outlive the top-level Identity
                // session.  The reference Learn client can renew that Identity
                // state with only a previously trusted fingerprint.  Do that
                // once, without credentials, then retry the wrapper once.
                Err(
                    reason @ ("portal_identity_entry_password_form"
                    | "portal_resume_password_required"),
                ) => {
                    renew_identity_with_trusted_device(identity, fingerprint, config)
                        .await
                        .map_err(|fallback| {
                            // If the trusted app itself simply requires a
                            // password, preserve the original stage-specific
                            // root reason instead of inventing success.
                            if fallback == "portal_resume_password_required" {
                                reason
                            } else {
                                fallback
                            }
                        })?;
                    navigate(identity, fingerprint, config, info_prefix).await
                }
                Err(reason) => Err(reason),
            }
        }
        RecoveryBoundary::Info => {
            navigate_restored(identity, fingerprint, config, info_prefix).await
        }
    }
}

async fn renew_identity_with_trusted_device(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
) -> Result<(), &'static str> {
    let path = format!("/do/off/ui/auth/login/form/{TRUSTED_DEVICE_APP}/0");
    let page = identity
        .fetch_login_page_at_path(&path)
        .await
        .map_err(|_| "portal_resume_trusted_network")?;
    if !same_origin(&page.final_url, config.identity_origin()) || !clean(&page.final_url) {
        return Err("portal_resume_sso_route");
    }
    let learn = Url::parse(LEARN_ORIGIN).map_err(|_| "portal_resume_config")?;
    let redirected_to_learn = |response: &IdentityHttpResponse| {
        response.redirect_location.as_ref().is_some_and(|target| {
            response.status.is_redirection() && clean(target) && same_origin(target, &learn)
        })
    };
    let marked_learn_success = |response: &IdentityHttpResponse| {
        response.status.is_success()
            && identity
                .client()
                .has_identity_success_marker(response.body())
            && identity
                .client()
                .first_static_redirect_url_for_origin(&response.final_url, response.body(), &learn)
                .is_some()
    };

    // An already-renewed trusted session may immediately hand off to Learn.
    // This is only evidence to retry WebVPN; no service session is marked here.
    if redirected_to_learn(&page) || marked_learn_success(&page) {
        return Ok(());
    }

    let Some(response) = identity
        .continue_service_check_single(&page, fingerprint)
        .await
        .map_err(|_| "portal_resume_trusted_network")?
    else {
        return Err(trusted_device_entry_failure(identity, &page));
    };

    if response.status == StatusCode::TOO_MANY_REQUESTS {
        return Err("portal_resume_rate_limited");
    }
    if matches!(
        response.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err("portal_resume_sso_rejected");
    }
    if !response.status.is_success() && !response.status.is_redirection() {
        return Err("portal_resume_sso_http");
    }
    let evidence = identity.client().parse_login_page(response.body());
    if evidence.second_factor_marker.is_some() {
        return Err("portal_resume_second_factor_required");
    }
    if evidence
        .failure
        .as_ref()
        .is_some_and(|failure| failure.reason != LoginFailureReason::Generic)
    {
        return Err(page_failure(&evidence));
    }
    if redirected_to_learn(&response) || marked_learn_success(&response) {
        return Ok(());
    }
    Err("portal_resume_trusted_unconfirmed")
}

async fn navigate(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
) -> Result<(), &'static str> {
    let entry = config.entry_url().map_err(|_| "portal_resume_config")?;
    navigate_from(identity, fingerprint, config, info_prefix, entry, false).await
}

/// A restored Runtime already owns its top-level Cookie jar. Do not start
/// the NEW-login WebVPN OAuth chain again: ask the target-app SSO entry to
/// consume the carried Identity cookies, exactly as a service roam does.
/// Any interactive form is returned as a scoped failure, never password POST.
pub(crate) async fn navigate_restored(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
) -> Result<(), &'static str> {
    let entry = config
        .identity_origin()
        .join(&format!("/do/off/ui/auth/login/form/{PORTAL_APP}"))
        .map_err(|_| "portal_resume_config")?;
    navigate_from(identity, fingerprint, config, info_prefix, entry, true).await
}

/// Consume the OAuth handoff selected by a freshly authenticated target
/// service.  The OAuth/WebVPN chain can return to the target application's
/// Identity login form even though the top-level Identity login already
/// succeeded.  Treat that form as an intermediate page and let
/// `navigate_from` apply the same strict, one-shot password-free `checkSingle`
/// continuation used by restored sessions.
pub(crate) async fn navigate_portal_target(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
    target: &Url,
) -> Result<(), &'static str> {
    if !clean(target)
        || !same_origin(target, config.oauth_origin())
        || !(target.path() == "/lb-auth/lbredirect" || target.path().starts_with("/thu-oauth/"))
    {
        return Err("portal_resume_sso_route");
    }
    navigate_from(
        identity,
        fingerprint,
        config,
        info_prefix,
        target.clone(),
        true,
    )
    .await
}

/// Consume the exact allowlisted Identity continuation selected by the
/// resource response. Query/ticket state is used once in memory, never saved
/// or diagnosed. Finishing navigation still does not prove INFO's session.
pub(crate) async fn navigate_handoff(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
    handoff: &crate::info::OpaqueUrl,
) -> Result<(), &'static str> {
    let entry = Url::parse(handoff.as_str()).map_err(|_| "portal_resume_sso_route")?;
    if !clean(&entry)
        || !same_origin(config.identity_origin(), &entry)
        || !(entry.path().starts_with("/do/off/ui/auth/login/form/")
            || entry.path() == "/do/off/ui/auth/login/redirect2Jsp")
    {
        return Err("portal_resume_sso_route");
    }
    navigate_from(identity, fingerprint, config, info_prefix, entry, true).await
}

fn navigation_redirect(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}
fn navigation_success(status: StatusCode) -> bool {
    // THUInfo uFetch accepts 200/201, not cache/partial-content responses.
    matches!(status.as_u16(), 200 | 201)
}

fn trusted_form_application(url: &Url) -> Option<&str> {
    let mut parts = url
        .path()
        .strip_prefix("/do/off/ui/auth/login/form/")?
        .split('/');
    let app = parts.next()?;
    if app.is_empty()
        || app.len() > 128
        || !app
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~-".contains(&byte))
        || !matches!(parts.next(), None | Some("0"))
        || parts.next().is_some()
    {
        return None;
    }
    Some(app)
}

async fn navigate_from(
    identity: &IdentityExecutionClient,
    fingerprint: &str,
    config: &WebVpnIdentityConfig,
    info_prefix: &str,
    mut next: Url,
    mut entered_portal: bool,
) -> Result<(), &'static str> {
    if !info_prefix.starts_with('/')
        || !info_prefix.ends_with('/')
        || info_prefix.contains(['?', '#'])
    {
        return Err("portal_resume_config");
    }
    let transport = identity.transport();
    // The reference wraps the initial roam destination, not every redirect.
    next = broker_target(config, next)?;
    let mut visited = HashSet::new();
    let mut submitted = false;
    let mut submitted_apps = HashSet::new();
    let mut submission_confirmed = false;
    let mut continuation: Option<IdentityHttpResponse> = None;
    for hop in 0..12 {
        let is_submission_response = continuation.is_some();
        let page = if let Some(page) = continuation.take() {
            page
        } else {
            if !allowed(config, info_prefix, &next) {
                trace_route(config, info_prefix, &next, hop, "reject_route");
                return Err("portal_navigation_target_rejected");
            }
            if !visited.insert(next.as_str().to_owned()) {
                trace_route(config, info_prefix, &next, hop, "reject_cycle");
                return Err("portal_navigation_cycle");
            }
            trace_route(config, info_prefix, &next, hop, "dispatch");
            let request = transport
                .client()
                .get(next.clone())
                .build()
                .map_err(|_| "portal_resume_config")?;
            let mut response = transport
                .execute_once(transport.client(), request)
                .await
                .map_err(|_| "portal_resume_sso_network")?;
            let status = response.status();
            let final_url = response.url().clone();
            let location = match response
                .headers()
                .get(LOCATION)
                .filter(|_| navigation_redirect(status))
            {
                Some(value) => Some(
                    final_url
                        .join(value.to_str().map_err(|_| "portal_resume_sso_route")?)
                        .map_err(|_| "portal_resume_sso_route")?,
                ),
                None => None,
            };
            if !navigation_success(status) && !navigation_redirect(status) {
                return Err(match status {
                    StatusCode::TOO_MANY_REQUESTS => "portal_resume_rate_limited",
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                        "portal_resume_sso_rejected"
                    }
                    _ => "portal_resume_sso_http",
                });
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "portal_resume_sso_network")?
            {
                if bytes.len() + chunk.len() > MAX_BODY {
                    return Err("portal_resume_sso_body_limit");
                }
                bytes.extend_from_slice(&chunk);
            }
            let body = String::from_utf8(bytes).map_err(|_| "portal_resume_sso_encoding")?;
            IdentityHttpResponse::from_service_navigation(status, final_url, location, body)
        };
        if !allowed(config, info_prefix, &page.final_url) {
            trace_route(config, info_prefix, &page.final_url, hop, "reject_response");
            return Err("portal_navigation_response_rejected");
        }
        if !navigation_success(page.status) && !navigation_redirect(page.status) {
            return Err("portal_resume_sso_http");
        }
        if let Some(target) = page
            .redirect_location
            .as_ref()
            .filter(|_| navigation_redirect(page.status))
        {
            if is_submission_response {
                submission_confirmed = true;
            }
            // Preserve Location exactly as THUInfo's fetch loop does. In
            // particular a direct INFO Location must not replay lbredirect.
            trace_route(config, info_prefix, target, hop, "http_location");
            next = target.clone();
            continue;
        }
        if navigation_redirect(page.status) {
            trace_route(config, info_prefix, &page.final_url, hop, "reject_location");
            return Err("portal_navigation_location_missing");
        }
        if let Some(target) = success_navigation_target(identity, config, &page) {
            if is_submission_response {
                submission_confirmed = true;
            }
            // THUInfo follows the explicit success handoff even when the
            // original form/default notice remains in the HTML template.
            // This remains navigation, never standalone authentication proof.
            next = broker_target(config, target)?;
            continue;
        }
        let evidence = identity.client().parse_login_page(page.body());
        if same_origin(&page.final_url, config.identity_origin()) {
            if identity
                .client()
                .service_check_single_action(&page.final_url, page.body())
                .is_some()
            {
                let app = trusted_form_application(&page.final_url)
                    .ok_or("portal_resume_trusted_unconfirmed")?;
                if submitted || !submitted_apps.insert(app.to_owned()) {
                    return Err("portal_resume_trusted_unconfirmed");
                }
            }
            if !submitted {
                if let Some(response) = identity
                    .continue_service_check_single(&page, fingerprint)
                    .await
                    .map_err(|_| "portal_resume_trusted_network")?
                {
                    submitted = true;
                    continuation = Some(response);
                    continue;
                }
            }
            if evidence.failure.is_some() || evidence.second_factor_marker.is_some() {
                return Err(terminal_reason(identity, config, &page, submitted));
            }
        }
        // THUInfo uFetch returns a clean terminal HTTP page; it does not
        // browse the first ordinary navigation/menu anchor on that page.
        // Finishing navigation still does not mark INFO authenticated.
        let clean_page = !evidence.has_login_form
            && evidence.failure.is_none()
            && evidence.second_factor_marker.is_none();
        let vpn_landing = same_origin(&page.final_url, config.webvpn_origin())
            && (mapped_info_path(info_prefix, &page.final_url)
                || (entered_portal && matches!(page.final_url.path(), "/" | "/login")));
        let direct_landing = entered_portal
            && Url::parse(INFO_ORIGIN).is_ok_and(|origin| same_origin(&origin, &page.final_url));
        if clean_page && (vpn_landing || direct_landing) {
            trace_route(
                config,
                info_prefix,
                &page.final_url,
                hop,
                "navigation_complete",
            );
            return Ok(()); // Caller still must prove target CSRF + account.
        }
        // Only clean static browser navigation, not arbitrary JS evaluation.
        // Credential forms never authorize following a service link.
        if !evidence.has_login_form
            && evidence.failure.is_none()
            && evidence.second_factor_marker.is_none()
        {
            let info = Url::parse(INFO_ORIGIN).map_err(|_| "portal_resume_config")?;
            let redirect = [
                config.oauth_origin(),
                config.webvpn_origin(),
                &info,
                config.identity_origin(),
            ]
            .into_iter()
            .find_map(|origin| {
                identity.client().first_static_redirect_url_for_origin(
                    &page.final_url,
                    page.body(),
                    origin,
                )
            });
            if let Some(target) = redirect {
                next = if same_origin(&page.final_url, config.identity_origin()) {
                    broker_target(config, target)?
                } else {
                    target
                };
                continue;
            }
        }
        if same_origin(&page.final_url, config.webvpn_origin()) {
            if !entered_portal && matches!(page.final_url.path(), "/" | "/login") && clean_page {
                // WebVPN's dynamically selected Identity application and
                // INFO's fixed application are distinct handoffs. Only a
                // clean gateway landing after a confirmed submission may
                // advance the phase. The application set survives this
                // transition so /form/app and /form/app/0 cannot be replayed.
                if submitted && !submission_confirmed {
                    return Err("portal_resume_trusted_unconfirmed");
                }
                entered_portal = true;
                submitted = false;
                submission_confirmed = false;
                next = config
                    .identity_origin()
                    .join(&format!("/do/off/ui/auth/login/form/{PORTAL_APP}"))
                    .map_err(|_| "portal_resume_config")?;
                continue;
            }
        }
        return Err(terminal_reason(identity, config, &page, submitted));
    }
    Err("portal_resume_sso_hop_limit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_recovery_dispatch_is_scoped_and_terminal_after_handoff() {
        assert_eq!(
            recovery_boundary("portal_resource_webvpn_login"),
            Some(RecoveryBoundary::WebVpn)
        );
        assert_eq!(
            recovery_boundary("portal_resume_csrf_missing"),
            Some(RecoveryBoundary::Info)
        );
        assert_eq!(
            recovery_boundary("portal_resume_login_required"),
            Some(RecoveryBoundary::Info)
        );
        for reason in [
            "portal_after_handoff_csrf_missing",
            "portal_after_handoff_login_required",
            "portal_resume_network",
            "portal_resource_http",
            "portal_resource_http_rejected",
            "portal_resume_rate_limited",
            "portal_resume_account_mismatch",
            "portal_resource_unconfirmed",
            "portal_resource_webvpn_login:unexpected-detail",
        ] {
            assert_eq!(recovery_boundary(reason), None);
        }
        assert_eq!(
            crate::live_validation::error_reason("portal_after_handoff_csrf_missing"),
            "portal_after_handoff_csrf_missing"
        );
        assert_eq!(
            crate::live_validation::error_reason("portal_after_handoff_login_required"),
            "portal_after_handoff_login_required"
        );
    }
    use crate::identity::{
        FormEncoding, IdentityLoginProfile, LoginFormFields, LoginFormProfile, SecondAuthActions,
        SecondAuthProfile,
    };
    use crate::identity_client::{IdentityClient, IdentityClientConfig};
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    fn executor(base: &str) -> IdentityExecutionClient {
        let profile = IdentityLoginProfile::new(
            "fixture",
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
        IdentityExecutionClient::new(
            IdentityClient::new(IdentityClientConfig::new(base, profile).unwrap()).unwrap(),
            "THYou/fixture",
        )
        .unwrap()
    }

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let n = stream.read(&mut chunk).unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&chunk[..n]);
            let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                assert!(bytes.len() < 32 * 1024);
                continue;
            };
            let header = String::from_utf8_lossy(&bytes[..end]);
            let content_length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or_default();
            if bytes.len() >= end + 4 + content_length {
                return String::from_utf8(bytes).unwrap();
            }
            assert!(bytes.len() < 32 * 1024);
        }
    }

    #[test]
    fn backend_repair_portal_resume_navigation_rejects_foreign_origins_and_queries() {
        let c = WebVpnIdentityConfig::new(
            "https://vpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .unwrap();
        for target in [
            "https://evil.example/login",
            "https://user@vpn.example.test/login",
            "https://vpn.example.test/login#x",
            "https://vpn.example.test/admin",
            "https://id.example.test/b/account/deleteDevice",
        ] {
            assert!(!allowed(&c, "/info/", &Url::parse(target).unwrap()));
        }
        let next = broker_target(
            &c,
            Url::parse("https://info.tsinghua.edu.cn/f/info/index?ticket=fixture").unwrap(),
        )
        .unwrap();
        assert_eq!(next.host_str(), Some("oauth.example.test"));
        assert!(
            next.query_pairs()
                .any(|(k, v)| k == "uri" && v == "/f/info/index?ticket=fixture")
        );
    }

    #[tokio::test]
    async fn backend_repair_portal_resume_follows_cookie_redirects_without_password_or_proof() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let oauthu = format!("http://{}/", oauth.local_addr().unwrap());
        let config = WebVpnIdentityConfig::new(&vpnu, &oauthu, &idu).unwrap();
        let server = thread::spawn(move || {
            let (mut a, _) = vpn.accept().unwrap();
            a.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buf = [0; 8192];
            let n = a.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /login?oauth_login=true"));
            a.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            let (mut b, _) = id.accept().unwrap();
            let n = b.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.starts_with("GET /do/off/ui/auth/login/form/10000ea"));
            assert!(!req.contains("i_pass"));
            write!(b,"HTTP/1.1 302 Found\r\nLocation: {}lb-auth/lbredirect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",oauthu).unwrap();
            let (mut c, _) = oauth.accept().unwrap();
            let _ = c.read(&mut buf).unwrap();
            write!(c,"HTTP/1.1 302 Found\r\nLocation: {}info/f/info/index\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",vpnu).unwrap();
            let (mut d, _) = vpn.accept().unwrap();
            let _ = d.read(&mut buf).unwrap();
            d.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            (vpn, id, oauth)
        });
        let identity = executor(&idu);
        navigate(&identity, "fixture", &config, "/info/")
            .await
            .unwrap();
        for listener in [server.join().unwrap()] {
            for socket in [listener.0, listener.1, listener.2] {
                socket.set_nonblocking(true).unwrap();
                assert!(
                    matches!(socket.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock)
                );
            }
        }
    }

    #[tokio::test]
    async fn backend_repair_portal_trusted_submission_stops_after_service_unavailable() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let config = WebVpnIdentityConfig::new(
            &vpnu,
            format!("http://{}/", oauth.local_addr().unwrap()),
            &idu,
        )
        .unwrap();
        let server = thread::spawn(move || {
            let (mut a, _) = vpn.accept().unwrap();
            let mut buf = [0; 8192];
            a.read(&mut buf).unwrap();
            a.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            let (mut b, _) = id.accept().unwrap();
            b.read(&mut buf).unwrap();
            let body = r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"></form>"#;
            write!(
                b,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            let (mut c, _) = id.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes = Vec::new();
            loop {
                let n = c.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some(end) = text.find("\r\n\r\n") {
                    let len: usize = text[..end]
                        .lines()
                        .find_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
                assert!(bytes.len() < 16384);
            }
            let req = String::from_utf8(bytes).unwrap();
            assert!(req.starts_with("POST /do/off/ui/auth/login/checkSingle HTTP/1.1"));
            assert!(req.contains("fingerPrint"));
            assert!(!req.contains("i_pass"));
            assert!(!req.contains("i_user"));
            c.write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            (vpn, id, oauth)
        });
        let execution = executor(&idu);
        assert_eq!(
            navigate(&execution, "fixture", &config, "/info/").await,
            Err("portal_resume_sso_http")
        );
        let (vpn, id, oauth) = server.join().unwrap();
        for listener in [vpn, id, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[tokio::test]
    async fn backend_repair_portal_target_follows_identity_form_after_oauth_webvpn_round_trip() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let oauthu = format!("http://{}/", oauth.local_addr().unwrap());
        let config = WebVpnIdentityConfig::new(&vpnu, &oauthu, &idu).unwrap();
        let target = Url::parse(&format!("{oauthu}lb-auth/lbredirect?flow=initial")).unwrap();
        let idu_for_server = idu.clone();
        let server = thread::spawn(move || {
            let (mut first_oauth, _) = oauth.accept().unwrap();
            let request = read_request(&mut first_oauth);
            assert!(request.starts_with("GET /lb-auth/lbredirect?flow=initial HTTP/1.1"));
            write!(
                first_oauth,
                "HTTP/1.1 302 Found\r\nLocation: {vpnu}mapped/phase-one\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();

            let (mut first_vpn, _) = vpn.accept().unwrap();
            let request = read_request(&mut first_vpn);
            assert!(request.starts_with("GET /mapped/phase-one HTTP/1.1"));
            write!(
                first_vpn,
                "HTTP/1.1 302 Found\r\nLocation: {oauthu}thu-oauth/auth?flow=renew\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();

            let (mut second_oauth, _) = oauth.accept().unwrap();
            let request = read_request(&mut second_oauth);
            assert!(request.starts_with("GET /thu-oauth/auth?flow=renew HTTP/1.1"));
            write!(
                second_oauth,
                "HTTP/1.1 302 Found\r\nLocation: {idu_for_server}do/off/ui/auth/login/form/10000ea055dd8d81d09d5a1ba55d39ad/0?flow=trusted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();

            let (mut identity_get, _) = id.accept().unwrap();
            let request = read_request(&mut identity_get);
            assert!(request.starts_with(
                "GET /do/off/ui/auth/login/form/10000ea055dd8d81d09d5a1ba55d39ad/0?flow=trusted HTTP/1.1"
            ));
            let body = r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"><input name="fingerPrint"></form>"#;
            write!(
                identity_get,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();

            let (mut identity_post, _) = id.accept().unwrap();
            let request = read_request(&mut identity_post);
            assert!(request.starts_with("POST /do/off/ui/auth/login/checkSingle HTTP/1.1"));
            assert!(request.contains("fixture-fingerprint"));
            assert!(!request.contains("i_user"));
            assert!(!request.contains("i_pass"));
            write!(
                identity_post,
                "HTTP/1.1 302 Found\r\nLocation: {vpnu}mapped/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();

            let (mut final_vpn, _) = vpn.accept().unwrap();
            let request = read_request(&mut final_vpn);
            assert!(request.starts_with("GET /mapped/final HTTP/1.1"));
            final_vpn
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nshell",
                )
                .unwrap();
            (vpn, id, oauth)
        });

        navigate_portal_target(
            &executor(&idu),
            "fixture-fingerprint",
            &config,
            "/mapped/",
            &target,
        )
        .await
        .unwrap();

        let (vpn, id, oauth) = server.join().unwrap();
        for listener in [vpn, id, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ));
        }
    }

    #[test]
    fn backend_repair_identity_success_navigation_outranks_generic_template_only() {
        let cfg = WebVpnIdentityConfig::new(
            "https://vpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .unwrap();
        let identity = executor("https://id.example.test/");
        let url = Url::parse("https://id.example.test/do/off/ui/auth/login/checkSingle").unwrap();
        let body = r#"登录成功。正在重定向到<div>登录失败</div><form method="post" action="/do/off/ui/auth/login/check"></form><a href="https://oauth.example.test/lb-auth/lbredirect?fixture=1">continue</a>"#;
        let page = IdentityHttpResponse::from_service_navigation(
            StatusCode::OK,
            url.clone(),
            None,
            body.to_owned(),
        );
        assert!(success_navigation_target(&identity, &cfg, &page).is_some());
        assert!(!page.has_cookie_update());
        for denied in [
            body.replace("登录成功。正在重定向到", ""),
            body.replace("oauth.example.test", "evil.example"),
            format!("<input name='loginInvalid' value='true'>{body}"),
        ] {
            let page = IdentityHttpResponse::from_service_navigation(
                StatusCode::OK,
                url.clone(),
                None,
                denied,
            );
            assert!(success_navigation_target(&identity, &cfg, &page).is_none());
        }
    }

    #[test]
    fn backend_repair_restored_portal_allows_only_fixed_webvpn_identity_wrapper() {
        let cfg = WebVpnIdentityConfig::new(
            "https://webvpn.tsinghua.edu.cn/",
            "https://oauth.tsinghua.edu.cn/",
            "https://id.tsinghua.edu.cn/",
        )
        .unwrap();
        let info_prefix =
            "/https/77726476706e69737468656265737421f9f9479369247b59700f81b9991b2631506205de/";
        let id_prefix =
            "/https/77726476706e69737468656265737421f9f30f8834396657761d88e29d51367bcfe7/";
        for path in [
            format!("{id_prefix}do/off/ui/auth/login/form/{PORTAL_APP}"),
            format!("{id_prefix}do/off/ui/auth/login/form/{PORTAL_APP}/0"),
            format!("{id_prefix}do/off/ui/auth/login/checkSingle"),
            format!("{id_prefix}do/off/ui/auth/login/redirect2Jsp"),
            format!(
                "{}%2Fdo/off/ui/auth/login/check",
                id_prefix.trim_end_matches('/')
            ),
        ] {
            let url = Url::parse(&format!("https://webvpn.tsinghua.edu.cn{path}")).unwrap();
            assert!(
                allowed(&cfg, info_prefix, &url),
                "expected fixed Identity wrapper: {path}"
            );
        }
        for path in [
            format!("{id_prefix}do/off/ui/auth/login/other"),
            format!("{id_prefix}do/off/ui/auth/login/form/x/%2e%2e/other"),
            "/https/not-the-identity-map/do/off/ui/auth/login/check".to_owned(),
        ] {
            let url = Url::parse(&format!("https://webvpn.tsinghua.edu.cn{path}")).unwrap();
            assert!(
                !allowed(&cfg, info_prefix, &url),
                "must reject unrelated wrapper: {path}"
            );
        }
    }

    #[test]
    fn backend_repair_portal_terminal_reason_separates_transport_stages_without_payloads() {
        let cfg = WebVpnIdentityConfig::new(
            "https://vpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .unwrap();
        let identity = executor("https://id.example.test/");
        for (url, body, expected) in [
            (
                "https://vpn.example.test/login?private=fixture",
                "登录失败",
                "portal_webvpn_entry_generic_notice",
            ),
            (
                "https://vpn.example.test/",
                "登录失败",
                "portal_webvpn_home_generic_notice",
            ),
            (
                "https://oauth.example.test/thu-oauth/auth?private=fixture",
                "登录失败",
                "portal_oauth_generic_notice",
            ),
            (
                "https://id.example.test/do/off/ui/auth/login/form/dynamic/0",
                "登录失败<script>const route='checkSingle';</script>",
                "portal_identity_entry_trusted_script",
            ),
            (
                "https://id.example.test/do/off/ui/auth/login/form/10000ea055dd8d81d09d5a1ba55d39ad",
                "登录失败",
                "portal_identity_target_no_form",
            ),
        ] {
            let page = IdentityHttpResponse::from_service_navigation(
                StatusCode::OK,
                Url::parse(url).unwrap(),
                None,
                body.to_owned(),
            );
            assert_eq!(terminal_reason(&identity, &cfg, &page, false), expected);
            assert!(!expected.contains("fixture"));
            assert!(
                identity
                    .client()
                    .service_check_single_action(&page.final_url, body)
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn backend_repair_restored_portal_starts_at_target_sso_not_new_webvpn_login() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let oauthu = format!("http://{}/", oauth.local_addr().unwrap());
        let cfg = WebVpnIdentityConfig::new(&vpnu, &oauthu, &idu).unwrap();
        let server = thread::spawn(move || {
            let (mut a, _) = id.accept().unwrap();
            a.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut buf = [0; 8192];
            let n = a.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with(
                "GET /do/off/ui/auth/login/form/10000ea055dd8d81d09d5a1ba55d39ad HTTP/1.1"
            ));
            write!(a,"HTTP/1.1 302 Found\r\nLocation: {}lb-auth/lbredirect?fixture=1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",oauthu).unwrap();
            let (mut b, _) = oauth.accept().unwrap();
            let n = b.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /lb-auth/lbredirect?"));
            write!(b,"HTTP/1.1 302 Found\r\nLocation: {}info/f/info/index\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",vpnu).unwrap();
            let (mut c, _) = vpn.accept().unwrap();
            let n = c.read(&mut buf).unwrap();
            assert!(
                String::from_utf8_lossy(&buf[..n]).starts_with("GET /info/f/info/index HTTP/1.1")
            );
            c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            (vpn, id, oauth)
        });
        navigate_restored(&executor(&idu), "fixture", &cfg, "/info/")
            .await
            .unwrap();
        let (vpn, id, oauth) = server.join().unwrap();
        for listener in [vpn, id, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[tokio::test]
    async fn backend_repair_restored_target_password_form_never_submits_or_restarts_login() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let cfg = WebVpnIdentityConfig::new(
            format!("http://{}/", vpn.local_addr().unwrap()),
            format!("http://{}/", oauth.local_addr().unwrap()),
            &idu,
        )
        .unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = id.accept().unwrap();
            let mut bytes = [0; 8192];
            stream.read(&mut bytes).unwrap();
            let body = r#"<div>登录失败</div><div id="sm2publicKey">fixture</div><form method="post" action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            id
        });
        assert_eq!(
            navigate_restored(&executor(&idu), "fixture", &cfg, "/info/").await,
            Err("portal_identity_target_password_form")
        );
        for listener in [server.join().unwrap(), vpn, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[tokio::test]
    async fn backend_repair_trusted_device_entry_generic_notice_keeps_form_shape() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let cfg = WebVpnIdentityConfig::new(
            format!("http://{}/", vpn.local_addr().unwrap()),
            format!("http://{}/", oauth.local_addr().unwrap()),
            &idu,
        )
        .unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = id.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 8192];
            let n = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            assert!(request.starts_with(
                "GET /do/off/ui/auth/login/form/bb5df85216504820be7bba2b0ae1535b/0 HTTP/1.1"
            ));
            let body = r#"<div>登录失败</div><div id="sm2publicKey">fixture-key</div><form method="post" action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            id
        });

        assert_eq!(
            renew_identity_with_trusted_device(
                &executor(&idu),
                "0123456789abcdef0123456789abcdef",
                &cfg,
            )
            .await,
            Err("portal_resume_trusted_password_form")
        );
        for listener in [server.join().unwrap(), vpn, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[tokio::test]
    async fn backend_repair_webvpn_password_entry_uses_one_trusted_device_recovery_before_retry() {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let oauthu = format!("http://{}/", oauth.local_addr().unwrap());
        let cfg = WebVpnIdentityConfig::new(&vpnu, &oauthu, &idu).unwrap();
        let server_idu = idu.clone();
        let server = thread::spawn(move || {
            fn read_request(stream: &mut std::net::TcpStream) -> String {
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 8192];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (k, v) = line.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + len {
                            break;
                        }
                    }
                    assert!(bytes.len() < 32768);
                }
                String::from_utf8(bytes).unwrap()
            }
            fn respond(stream: &mut std::net::TcpStream, status: &str, headers: &str, body: &str) {
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }

            // First WebVPN recovery reaches an Identity password form. It must
            // not submit credentials; the caller should fall back to one
            // trusted-device checkSingle attempt on the known Learn Identity app.
            let (mut v1, _) = vpn.accept().unwrap();
            assert!(read_request(&mut v1).starts_with("GET /login?oauth_login=true HTTP/1.1"));
            respond(
                &mut v1,
                "302 Found",
                &format!("Location: {oauthu}thu-oauth/auth?phase=first\r\n"),
                "",
            );
            let (mut o1, _) = oauth.accept().unwrap();
            assert!(read_request(&mut o1).starts_with("GET /thu-oauth/auth?phase=first HTTP/1.1"));
            respond(
                &mut o1,
                "302 Found",
                &format!("Location: {server_idu}do/off/ui/auth/login/form/dynamic/0\r\n"),
                "",
            );
            let (mut i1, _) = id.accept().unwrap();
            assert!(
                read_request(&mut i1)
                    .starts_with("GET /do/off/ui/auth/login/form/dynamic/0 HTTP/1.1")
            );
            let password = r#"<div>登录失败</div><div id="sm2publicKey">fixture-key</div><form method="post" action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass" type="password"></form>"#;
            respond(&mut i1, "200 OK", "", password);

            let (mut i2, _) = id.accept().unwrap();
            let req = read_request(&mut i2);
            assert!(req.starts_with(
                "GET /do/off/ui/auth/login/form/bb5df85216504820be7bba2b0ae1535b/0 HTTP/1.1"
            ));
            let trusted =
                r#"<form method="post" action="/do/off/ui/auth/login/checkSingle"></form>"#;
            respond(&mut i2, "200 OK", "", trusted);

            let (mut i3, _) = id.accept().unwrap();
            let req = read_request(&mut i3);
            assert!(req.starts_with("POST /do/off/ui/auth/login/checkSingle HTTP/1.1"));
            assert!(req.contains("fingerPrint"));
            assert!(!req.contains("i_user"));
            assert!(!req.contains("i_pass"));
            let success = r#"登录成功。正在重定向到<a href="https://learn.tsinghua.edu.cn/f/wlxt/index/course/student/">continue</a>"#;
            respond(&mut i3, "200 OK", "", success);

            // Retry WebVPN once after trusted-device renewal. The retry is
            // allowed to continue through the normal portal target handoff.
            let (mut v2, _) = vpn.accept().unwrap();
            assert!(read_request(&mut v2).starts_with("GET /login?oauth_login=true HTTP/1.1"));
            respond(
                &mut v2,
                "302 Found",
                &format!("Location: {oauthu}thu-oauth/auth?phase=second\r\n"),
                "",
            );
            let (mut o2, _) = oauth.accept().unwrap();
            assert!(read_request(&mut o2).starts_with("GET /thu-oauth/auth?phase=second HTTP/1.1"));
            respond(&mut o2, "302 Found", &format!("Location: {vpnu}\r\n"), "");
            let (mut v3, _) = vpn.accept().unwrap();
            assert!(read_request(&mut v3).starts_with("GET / HTTP/1.1"));
            respond(&mut v3, "200 OK", "", "<html>webvpn shell</html>");

            let (mut i4, _) = id.accept().unwrap();
            assert!(read_request(&mut i4).starts_with(
                "GET /do/off/ui/auth/login/form/10000ea055dd8d81d09d5a1ba55d39ad HTTP/1.1"
            ));
            respond(
                &mut i4,
                "302 Found",
                &format!("Location: {oauthu}lb-auth/lbredirect?fixture=second\r\n"),
                "",
            );
            let (mut o3, _) = oauth.accept().unwrap();
            assert!(read_request(&mut o3).starts_with("GET /lb-auth/lbredirect?"));
            respond(
                &mut o3,
                "302 Found",
                &format!("Location: {vpnu}info/f/info/index\r\n"),
                "",
            );
            let (mut v4, _) = vpn.accept().unwrap();
            assert!(read_request(&mut v4).starts_with("GET /info/f/info/index HTTP/1.1"));
            respond(&mut v4, "200 OK", "", "<html>target shell</html>");
            (vpn, id, oauth)
        });

        assert_eq!(
            recover_boundary(
                &executor(&idu),
                "0123456789abcdef0123456789abcdef",
                &cfg,
                "/info/",
                RecoveryBoundary::WebVpn,
            )
            .await,
            Ok(())
        );
        let (vpn, id, oauth) = server.join().unwrap();
        for listener in [vpn, id, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }

    #[test]
    fn backend_repair_portal_mapping_accepts_encoded_separator_but_not_mapping_escape() {
        let cfg = WebVpnIdentityConfig::new(
            "https://vpn.example.test/",
            "https://oauth.example.test/",
            "https://id.example.test/",
        )
        .unwrap();
        let prefix = "/https/fixturemapping/";
        for path in [
            "/https/fixturemapping/f/info/index",
            "/https/fixturemapping%2Ff%2Finfo%2Findex",
            "/https/fixturemapping%2ff/info/index",
        ] {
            assert!(allowed(
                &cfg,
                prefix,
                &Url::parse(&format!("https://vpn.example.test{path}")).unwrap()
            ));
        }
        for path in [
            "/https/other%2Ff/info/index",
            "/https/fixturemapping-suffix%2Ff/info/index",
            "/https/fixturemapping%2F%2e%2e%2Fother",
            "/https/fixturemapping%252Ff/info/index",
            "/https/fixturemapping%2F%5cother",
        ] {
            assert!(!allowed(
                &cfg,
                prefix,
                &Url::parse(&format!("https://vpn.example.test{path}")).unwrap()
            ));
        }
    }

    #[tokio::test]
    async fn backend_repair_server_selected_portal_handoff_preserves_context_and_never_restarts_login()
     {
        let vpn = TcpListener::bind("127.0.0.1:0").unwrap();
        let id = TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth = TcpListener::bind("127.0.0.1:0").unwrap();
        let vpnu = format!("http://{}/", vpn.local_addr().unwrap());
        let idu = format!("http://{}/", id.local_addr().unwrap());
        let cfg = WebVpnIdentityConfig::new(
            &vpnu,
            format!("http://{}/", oauth.local_addr().unwrap()),
            &idu,
        )
        .unwrap();
        let handoff = crate::info::OpaqueUrl::new(format!(
            "{idu}do/off/ui/auth/login/form/server-selected/0?flow=fixture-private-flow"
        ))
        .unwrap();
        let identity = executor(&idu);
        let server = thread::spawn(move || {
            let (mut a, _) = id.accept().unwrap();
            let mut b = [0; 8192];
            let n = a.read(&mut b).unwrap();
            assert!(String::from_utf8_lossy(&b[..n]).starts_with("GET /do/off/ui/auth/login/form/server-selected/0?flow=fixture-private-flow HTTP/1.1"));
            write!(a,"HTTP/1.1 302 Found\r\nLocation: {}info/f/info/index\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",vpnu).unwrap();
            let (mut c, _) = vpn.accept().unwrap();
            let n = c.read(&mut b).unwrap();
            assert!(
                String::from_utf8_lossy(&b[..n]).starts_with("GET /info/f/info/index HTTP/1.1")
            );
            c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 31\r\nConnection: close\r\n\r\n<html><body>shell</body></html>").unwrap();
            (vpn, id, oauth)
        });
        navigate_handoff(&identity, "fixture-fingerprint", &cfg, "/info/", &handoff)
            .await
            .unwrap();
        for denied in [
            "https://evil.example/do/off/ui/auth/login/form/x",
            "https://id.tsinghua.edu.cn/logout",
        ] {
            assert_eq!(
                navigate_handoff(
                    &identity,
                    "fixture-fingerprint",
                    &cfg,
                    "/info/",
                    &crate::info::OpaqueUrl::new(denied).unwrap()
                )
                .await,
                Err("portal_resume_sso_route")
            );
        }
        let (vpn, id, oauth) = server.join().unwrap();
        for listener in [vpn, id, oauth] {
            listener.set_nonblocking(true).unwrap();
            assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        }
    }
}
