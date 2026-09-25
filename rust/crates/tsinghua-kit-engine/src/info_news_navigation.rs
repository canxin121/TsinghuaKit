use super::*;
use std::collections::HashSet;
// Derived from pinned THUInfo news.ts policyList and publication endpoints.
const REFERENCE_NEWS_MAPPINGS: &[&str] = &[
    "77726476706e69737468656265737421e0f852882e3e6e5f301c9aa596522b2043f84ba24ebecaf8",
    "77726476706e69737468656265737421e3f5468534367f1e6d119aafd641303ceb8f9190006d6afc78336870",
    "77726476706e69737468656265737421e4ff459d207e6b597d469dbf915b243de94c4812e5c2e1599f",
    "77726476706e69737468656265737421e7e056d234297b437c0bc7b88b5c2d3212b31e4d37621d4714d6",
    "77726476706e69737468656265737421e8e442d23323615e79009cadd6502720f9b87b",
    "77726476706e69737468656265737421e8ef439b69336153301c9aa596522b20e1a870705b76e399",
    "77726476706e69737468656265737421e9fd528569336153301c9aa596522b20735d12f268e561f0",
    "77726476706e69737468656265737421f2fa598421322653770bc7b88b5c2d32530b094045c3bd5cabf3",
    "77726476706e69737468656265737421f3f65399222226446d0187ab9040227b8e4026c4ffd2",
    "77726476706e69737468656265737421f8e60f8834396657761d88e29d51367b523e",
    "77726476706e69737468656265737421f9f9479369247b59700f81b9991b2631506205de",
    "77726476706e69737468656265737421fcfe43d23323615e79009cadd6502720703f47",
    "77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9",
    "77726476706e69737468656265737421e2e442d23323615e79009cadd650272001f8dd",
    "77726476706e69737468656265737421fae0429e207e6b597d469dbf915b243d8ae9128e1cdcffb247",
    crate::info_news::GHXT_MAPPING_ID,
];

pub(super) fn is_system_publication_mapping(prefix: &str) -> bool {
    let mut parts = prefix.trim_start_matches('/').split('/');
    matches!(
        parts.next(),
        Some("http" | "https" | "http-80" | "https-443")
    ) && parts.next() == Some(SYSTEM_PUBLICATION_MAPPING_ID)
        && parts.next().is_none()
}

impl InfoSessionAdapter {
    pub(super) fn news_login_target(&self, url: &Url) -> bool {
        if !url.username().is_empty() || url.password().is_some() {
            return false;
        }
        if self.config.is_identity_login_target(url) {
            return true;
        }
        if !same_origin(&self.config.webvpn_base_url, url) {
            return false;
        }
        let path = url.path().to_ascii_lowercase();
        matches!(path.as_str(), "/login" | "/f/login")
            || path.starts_with("/do/off/ui/auth/login/")
            || crate::portal_resume::mapped_identity_login_path(url)
            || crate::portal_resume::mapped_identity_dynamic_login_form_path(url)
            || (self
                .mapping_prefix_for_absolute_path(url.path())
                .is_ok_and(|prefix| {
                    prefix == self.config.target_prefix || reference_mapping(&prefix)
                })
                && is_identity_login_path(&path))
    }

    pub(super) fn approved_news_mapping(&self, url: &Url) -> Result<String, InfoSessionError> {
        self.check_news_mapping(url).map_err(|(_, error)| error)
    }

    fn check_news_mapping(&self, url: &Url) -> Result<String, (&'static str, InfoSessionError)> {
        if self.news_login_target(url) {
            return Err(("info_news_login_required", InfoSessionError::LoginRequired));
        }
        if !same_origin(&self.config.webvpn_base_url, url)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err((
                "info_news_origin_invalid",
                InfoSessionError::NewsUnexpectedOrigin,
            ));
        }
        if query_kind(url) == "csrf" || matches!(query_kind(url), "malformed" | "control") {
            return Err((
                "info_news_query_invalid",
                InfoSessionError::NewsUnexpectedPath,
            ));
        }
        let publication = safe_publication_fragment(url.fragment())
            .map_err(|error| ("info_news_fragment_invalid", error))?;
        let prefix = self
            .mapping_prefix_for_absolute_path(url.path())
            .map_err(|error| ("info_news_mapping_unknown", error))?;
        if !safe_news_mapping_path(url.path(), &prefix) {
            return Err((
                "info_news_path_invalid",
                InfoSessionError::NewsUnexpectedPath,
            ));
        }
        if prefix != self.config.target_prefix && !reference_mapping(&prefix) {
            return Err((
                "info_news_mapping_unknown",
                InfoSessionError::NewsUnexpectedPath,
            ));
        }
        // A publish hash is permission only for the pinned system-publication
        // mapping, never a way to reinterpret another site's id or API path.
        if publication.is_some() && !is_system_publication_mapping(&prefix) {
            return Err((
                "info_news_publication_scope",
                InfoSessionError::NewsUnexpectedPath,
            ));
        }
        Ok(prefix)
    }

    fn target_kind(&self, url: &Url) -> &'static str {
        if self.news_login_target(url) {
            return "login";
        }
        if !url.username().is_empty() || url.password().is_some() {
            return "userinfo";
        }
        if !same_origin(&self.config.webvpn_base_url, url) {
            return "foreign_origin";
        }
        if url.path().starts_with("/wengine-vpn") {
            return "portal_control";
        }
        let Ok(prefix) = self.mapping_prefix_for_absolute_path(url.path()) else {
            return "unmapped";
        };
        if prefix == self.config.target_prefix {
            "info"
        } else if is_system_publication_mapping(&prefix) {
            "system_publication"
        } else if reference_mapping(&prefix) {
            "legacy_reference"
        } else {
            "unknown_mapping"
        }
    }

    fn navigation_event(
        &self,
        url: &Url,
        reason: &'static str,
        hop: usize,
        status: Option<StatusCode>,
    ) {
        tracing::debug!(target:"tsinghua_kit::auth",event="news_navigation_boundary",service="info",
            reason,hop=hop as u64,http_status=status.map_or(0,|s|s.as_u16()),
            news_target=self.target_kind(url),news_fragment=fragment_kind(url),news_query=query_kind(url));
    }

    fn reject_navigation(&self, url: &Url, reason: &'static str, hop: usize) {
        // A WebVPN mapping identifier is a stable, ticket-free site selector.
        // Record it only for a rejected unknown mapping after validating its
        // exact structural shape. Never record the URL, route, query or hash.
        if self.target_kind(url) == "unknown_mapping" {
            if let Some(mapping) = diagnostic_news_mapping_id(url) {
                tracing::warn!(target:"tsinghua_kit::security",event="news_navigation_boundary",service="info",
                    reason,hop=hop as u64,news_target="unknown_mapping",
                    news_fragment=fragment_kind(url),news_query=query_kind(url),
                    news_mapping_id=mapping);
                return;
            }
        }
        tracing::warn!(target:"tsinghua_kit::security",event="news_navigation_boundary",service="info",
            reason,hop=hop as u64,news_target=self.target_kind(url),
            news_fragment=fragment_kind(url),news_query=query_kind(url));
    }

    pub(super) async fn get_reference_news_document(
        &self,
        mut next: Url,
    ) -> Result<NewsDocumentResponse, InfoSessionError> {
        let mut visited = HashSet::new();
        for hop in 0..=10 {
            let mapping = match self.check_news_mapping(&next) {
                Ok(mapping) => mapping,
                Err((reason, error)) => {
                    self.reject_navigation(&next, reason, hop);
                    return Err(error);
                }
            };
            let fragment = next.fragment().map(str::to_owned);
            let mut wire = next.clone();
            wire.set_fragment(None);
            // Different browser fragments do not authorize repeating the same
            // request (which could contain a one-shot ticket).
            if !visited.insert(wire.as_str().to_owned()) {
                self.reject_navigation(&next, "info_news_redirect_cycle", hop);
                return Err(InfoSessionError::NewsNavigationCycle);
            }
            let request = self
                .transport
                .client()
                .get(wire)
                .build()
                .map_err(TransportError::Request)?;
            let response = self
                .transport
                .execute_once(self.transport.client(), request)
                .await
                .map_err(TransportError::Request)?;
            let status = response.status();
            if is_authentication_status(status) {
                self.navigation_event(&next, "info_news_login_required", hop, Some(status));
                return Err(InfoSessionError::LoginRequired);
            }
            if matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
                if hop == 10 {
                    self.reject_navigation(&next, "info_news_redirect_limit", hop);
                    return Err(InfoSessionError::NewsNavigationLimit);
                }
                let Some(location) = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                else {
                    self.reject_navigation(&next, "info_news_location_invalid", hop);
                    return Err(InfoSessionError::NewsUnexpectedPath);
                };
                // Check wire text before URL normalization can erase encoded
                // traversal, backslashes or control characters.
                let location_path = location.split(['?', '#']).next().unwrap_or_default();
                if location.len() > 8192
                    || location.chars().any(char::is_control)
                    || location.contains('\\')
                    || invalid_percent_encoding(location)
                    || path_contains_encoded_escape(location_path)
                {
                    self.reject_navigation(&next, "info_news_location_invalid", hop);
                    return Err(InfoSessionError::NewsUnexpectedPath);
                }
                let mut target = response
                    .url()
                    .join(location)
                    .map_err(|_| InfoSessionError::NewsUnexpectedOrigin)?;
                if !location.contains('#') && fragment.is_some() {
                    // RFC 9110 Location fragment inheritance, bounded to the
                    // same publication mapping to avoid transferring an id to
                    // an unrelated origin or another service.
                    if self
                        .mapping_prefix_for_absolute_path(target.path())
                        .ok()
                        .as_deref()
                        != Some(&mapping)
                        || !same_origin(&self.config.webvpn_base_url, &target)
                    {
                        self.reject_navigation(&target, "info_news_publication_scope", hop + 1);
                        return Err(InfoSessionError::NewsUnexpectedPath);
                    }
                    target.set_fragment(fragment.as_deref());
                }
                self.navigation_event(&target, "info_news_redirect_followed", hop, Some(status));
                next = target;
                continue;
            }
            if !matches!(status.as_u16(), 200 | 201) {
                self.navigation_event(&next, "info_news_http", hop, Some(status));
                return Err(InfoSessionError::NewsHttpStatus { status });
            }
            let binary = is_pdf_response(
                response.url(),
                response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok()),
            );
            let mut document = self.read_news_response(response, binary).await?;
            document.navigation_fragment = fragment;
            self.navigation_event(&next, "info_news_document_received", hop, Some(status));
            return Ok(document);
        }
        Err(InfoSessionError::NewsNavigationLimit)
    }
}

fn reference_mapping(prefix: &str) -> bool {
    let mut segments = prefix.trim_start_matches('/').split('/');
    let (Some(protocol), Some(mapping), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    matches!(protocol, "http" | "https" | "http-80" | "https-443")
        && REFERENCE_NEWS_MAPPINGS.contains(&mapping)
}

fn diagnostic_news_mapping_id(url: &Url) -> Option<&str> {
    let mut parts = url.path().trim_start_matches('/').split('/');
    let protocol = parts.next()?;
    let mapping = parts.next()?;
    let route = parts.next()?;
    (matches!(protocol, "http" | "https" | "http-80" | "https-443")
        && !route.is_empty()
        && crate::telemetry::safe_webvpn_mapping_id(mapping))
    .then_some(mapping)
}

fn fragment_kind(url: &Url) -> &'static str {
    match url.fragment() {
        None => "none",
        Some(fragment) if fragment.starts_with("/publish/") || fragment.starts_with("publish/") => {
            "publish_prefix"
        }
        Some(fragment) if fragment.ends_with("/publish") => "publish_suffix",
        _ => "unsupported",
    }
}

fn query_kind(url: &Url) -> &'static str {
    if invalid_percent_encoding(url.query().unwrap_or_default()) {
        return "malformed";
    }
    if url
        .query_pairs()
        .any(|(key, _)| key.eq_ignore_ascii_case("_csrf") || key.eq_ignore_ascii_case("csrf"))
    {
        return "csrf";
    }
    if url.query_pairs().any(|(key, value)| {
        key.chars().any(char::is_control) || value.chars().any(char::is_control)
    }) {
        return "control";
    }
    if url.query().is_some() {
        "ordinary"
    } else {
        "none"
    }
}
