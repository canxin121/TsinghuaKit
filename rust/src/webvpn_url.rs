//! URL resolution for campus services reached through a WebVPN mapping.
//!
//! Url::join treats a route beginning with '/' as an origin absolute path.
//! That is correct for a normal origin, but it silently removes the
//! /https/<mapping>/ or /http/<mapping>/ prefix used by Tsinghua WebVPN.
//! Keep this small helper at the request boundary so every adapter applies the
//! same path and origin policy.

use reqwest::Url;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum WebVpnUrlError {
    #[error("base URL must use http(s), have a host, and contain no userinfo, query, or fragment")]
    InvalidBase,

    #[error(
        "endpoint route must be an absolute path without query, fragment, whitespace, or traversal"
    )]
    InvalidRoute,

    #[error("endpoint must stay on the configured origin")]
    OriginMismatch,

    #[error("endpoint must stay inside the configured WebVPN mapping")]
    MappingEscape,
}

/// Resolves a route while retaining a trailing base path such as
/// /https/<mapping>/. Normal origin bases keep the usual Url::join behaviour
/// for backwards-compatible origin-only clients.
pub(crate) fn resolve_endpoint(base: &Url, route: &str) -> Result<Url, WebVpnUrlError> {
    validate_base(base)?;
    validate_route(route)?;

    let endpoint = if let Some(prefix) = base_path_prefix(base) {
        let mut endpoint = base.clone();
        endpoint.set_path(&format!("{prefix}{route}"));
        endpoint
    } else {
        base.join(route).map_err(|_| WebVpnUrlError::InvalidRoute)?
    };

    if !endpoint_is_allowed(base, &endpoint) {
        return Err(if same_origin(base, &endpoint) {
            WebVpnUrlError::MappingEscape
        } else {
            WebVpnUrlError::OriginMismatch
        });
    }
    Ok(endpoint)
}

/// Resolves either a route or an absolute URL and applies the same mapping
/// boundary. Absolute URLs are useful for profiled legacy responses, but they
/// can never switch to another origin or another WebVPN mapping.
pub(crate) fn resolve_endpoint_or_absolute(
    base: &Url,
    route_or_url: &str,
) -> Result<Url, WebVpnUrlError> {
    if let Ok(candidate) = Url::parse(route_or_url) {
        validate_base(base)?;
        if !endpoint_is_allowed(base, &candidate) {
            return Err(if !same_origin(base, &candidate) {
                WebVpnUrlError::OriginMismatch
            } else {
                WebVpnUrlError::MappingEscape
            });
        }
        return Ok(candidate);
    }
    resolve_endpoint(base, route_or_url)
}

/// Compares the URL authority used for a campus request. Fragments and
/// userinfo are never accepted at a request boundary.
pub(crate) fn same_origin(base: &Url, candidate: &Url) -> bool {
    base.scheme() == candidate.scheme()
        && base.host_str() == candidate.host_str()
        && base.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.fragment().is_none()
}

/// Returns whether candidate remains inside the base path when the base has an
/// explicit directory prefix. The comparison is segment-boundary aware, so
/// /https/map-two/... cannot satisfy /https/map/.
pub(crate) fn path_is_within_base(base: &Url, candidate: &Url) -> bool {
    let Some(prefix) = base_path_prefix(base) else {
        return true;
    };
    let candidate_path = candidate.path();
    candidate_path == prefix || candidate_path.starts_with(&format!("{prefix}/"))
}

/// Returns whether a candidate is safe for a request made against base.
pub(crate) fn endpoint_is_allowed(base: &Url, candidate: &Url) -> bool {
    same_origin(base, candidate) && path_is_within_base(base, candidate)
}

/// Validate the fully decoded path before an automatic HTTP redirect is
/// dispatched. The WebVPN gateway can decode separators and nested escapes;
/// comparing only the raw cipher-host prefix misses encoded dot traversal.
/// The caller separately validates the URL origin and the observed mapping.
pub(crate) fn redirect_path_stays_in_mapping(
    candidate: &Url,
    protocol: &str,
    mapping: &str,
) -> bool {
    let mut path = candidate.path().to_owned();
    while path.contains('%') {
        let Some(decoded) = percent_decode(&path) else {
            return false;
        };
        if decoded == path {
            return false;
        }
        path = decoded;
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return false;
    }
    let mut normalized = candidate.clone();
    normalized.set_path(&path);
    let prefix = format!("/{protocol}/{mapping}");
    normalized.path() == prefix || normalized.path().starts_with(&format!("{prefix}/"))
}

fn validate_base(base: &Url) -> Result<(), WebVpnUrlError> {
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || base.path().contains('\\')
        || base
            .path()
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || unsafe_encoded_path(base.path())
    {
        return Err(WebVpnUrlError::InvalidBase);
    }
    Ok(())
}

fn validate_route(route: &str) -> Result<(), WebVpnUrlError> {
    if route.trim().is_empty()
        || !route.starts_with('/')
        || route.starts_with("//")
        || route.contains(['?', '#', '\\'])
        || route
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || route
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
        || invalid_percent_encoding(route)
        || unsafe_encoded_path(route)
    {
        return Err(WebVpnUrlError::InvalidRoute);
    }
    Ok(())
}

fn base_path_prefix(base: &Url) -> Option<&str> {
    let path = base.path();
    if path.is_empty() || path == "/" || !path.ends_with('/') {
        return None;
    }
    Some(path.trim_end_matches('/'))
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

fn unsafe_encoded_path(value: &str) -> bool {
    let mut current = value.to_owned();
    loop {
        if invalid_percent_encoding(&current) {
            return true;
        }
        let bytes = current.as_bytes();
        if bytes.windows(3).any(|window| {
            window[0] == b'%'
                && hex_value(window[1])
                    .zip(hex_value(window[2]))
                    .is_some_and(|(high, low)| {
                        matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                    })
        }) {
            return true;
        }
        if !current.contains('%') {
            return false;
        }
        let Some(decoded) = percent_decode(&current) else {
            return true;
        };
        if decoded == current {
            return false;
        }
        current = decoded;
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = hex_value(*bytes.get(index + 1)?)?;
        let low = hex_value(*bytes.get(index + 2)?)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("fixture URL")
    }

    #[test]
    fn preserves_learn_mapping_prefix() {
        let base = url("https://webvpn.example.test/https/learn-map/");
        let endpoint =
            resolve_endpoint(&base, "/f/wlxt/index/course/student/index").expect("mapped endpoint");
        assert_eq!(
            endpoint.as_str(),
            "https://webvpn.example.test/https/learn-map/f/wlxt/index/course/student/index"
        );
        assert!(endpoint_is_allowed(&base, &endpoint));
    }

    #[test]
    fn preserves_registrar_mapping_prefix_and_rejects_sibling_mapping() {
        let base = url("https://webvpn.example.test/http/registrar-map/");
        let endpoint = resolve_endpoint(&base, "/jxmh_out.do").expect("mapped endpoint");
        assert_eq!(
            endpoint.as_str(),
            "https://webvpn.example.test/http/registrar-map/jxmh_out.do"
        );
        assert!(!endpoint_is_allowed(
            &base,
            &url("https://webvpn.example.test/http/registrar-map-two/jxmh_out.do")
        ));
        assert!(!endpoint_is_allowed(
            &base,
            &url("https://webvpn.example.test/")
        ));
    }

    #[test]
    fn absolute_endpoints_must_remain_inside_the_mapping() {
        let base = url("https://webvpn.example.test/https/learn-map/");
        assert!(
            resolve_endpoint_or_absolute(
                &base,
                "https://webvpn.example.test/https/learn-map/f/index"
            )
            .is_ok()
        );
        assert!(matches!(
            resolve_endpoint_or_absolute(&base, "https://webvpn.example.test/f/index"),
            Err(WebVpnUrlError::MappingEscape)
        ));
        assert!(matches!(
            resolve_endpoint_or_absolute(&base, "https://evil.example.test/https/learn-map/f"),
            Err(WebVpnUrlError::OriginMismatch)
        ));
    }

    #[test]
    fn rejects_direct_and_nested_encoded_traversal() {
        let base = url("https://webvpn.example.test/https/learn-map/");
        for route in [
            "/f/%2e%2e/login",
            "/f/%2Flogin",
            "/f/%252e%252e/login",
            "/f/ok%00",
        ] {
            assert!(matches!(
                resolve_endpoint(&base, route),
                Err(WebVpnUrlError::InvalidRoute)
            ));
        }
    }

    #[test]
    fn direct_origin_still_uses_origin_root() {
        let base = url("https://learn.example.test/");
        assert_eq!(
            resolve_endpoint(&base, "/b/course")
                .expect("direct endpoint")
                .as_str(),
            "https://learn.example.test/b/course"
        );
    }
}
