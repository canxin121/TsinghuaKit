//! Request planning and response parsing for the INFO service.
//!
//! INFO is reached through a service-specific portal route.  The route and
//! field names live in [`InfoPortalProfile`] so that a WebVPN deployment
//! detail cannot silently become a cross-service constant.  The roaming URL
//! returned by INFO is intentionally kept opaque: this module neither parses
//! its host mapping nor attempts to reconstruct one from a target host.

use std::fmt;

use reqwest::Url;
use serde_json::Value;
use thiserror::Error;

const DEFAULT_REDIRECT_PATH: &str = "/b/yyfw/vyyfwxx/info/portal_fg/common/onlineAppRedirect";
const DEFAULT_YYFWID_FIELD: &str = "yyfwid";
const DEFAULT_CSRF_FIELD: &str = "_csrf";
const DEFAULT_MACHINE_FIELD: &str = "machine";
const DEFAULT_MACHINE_VALUE: &str = "p";

/// Errors produced while validating an INFO profile, building its request, or
/// parsing an INFO response.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InfoError {
    #[error("{field} must not be empty")]
    EmptyValue { field: &'static str },

    #[error("{field} contains an invalid control character")]
    InvalidValue { field: &'static str },

    #[error("invalid INFO endpoint path: {path}")]
    InvalidPath { path: String },

    #[error("invalid INFO wire field name: {name}")]
    InvalidFieldName { name: String },

    #[error("INFO response is not valid JSON: {message}")]
    InvalidJson { message: String },

    #[error("INFO response result must be a string when present")]
    ResultNotString,

    #[error("INFO response success must be a boolean when present")]
    SuccessNotBoolean,

    #[error("INFO response indicates that authentication is required")]
    LoginRequired,

    #[error("INFO response result indicates a business failure")]
    ResultFailure,

    #[error("INFO response result indicates a business failure despite a roaming target")]
    ResultFailureWithTarget,

    #[error("INFO response result indicates a business failure with an object envelope")]
    ResultFailureWithObject,

    #[error(
        "INFO response result indicates a business failure with an object lacking a roaming target"
    )]
    ResultFailureWithObjectNoTarget,

    #[error("INFO response result indicates a business failure with a message field")]
    ResultFailureWithMessage,

    #[error("INFO response success flag indicates a business failure")]
    SuccessFlagFailure,

    #[error("INFO response error field indicates a business failure")]
    ErrorFieldFailure,

    #[error("INFO response message indicates a business failure")]
    MessageFailure,

    #[error("INFO service returned a business failure")]
    BusinessFailure,

    #[error("INFO response is missing {field}")]
    MissingField { field: &'static str },

    #[error("INFO response field {field} has the wrong type; expected {expected}")]
    WrongFieldType {
        field: &'static str,
        expected: &'static str,
    },

    #[error("INFO response contains an empty roaming URL")]
    EmptyRoamingUrl,

    #[error("INFO roaming URL contains a control character")]
    InvalidRoamingUrl,
}

/// Placement of the online-app redirect parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoParameterEncoding {
    Query,
    Form,
}

/// A request plan that can be passed to the shared HTTP transport.
///
/// The parameters remain as ordered name/value pairs.  This preserves the
/// wire names and lets `reqwest` perform the final URL/form encoding without
/// duplicating its encoding rules in the profile module.
#[derive(Clone, PartialEq, Eq)]
pub struct InfoPortalRequestPlan {
    pub path: String,
    pub encoding: InfoParameterEncoding,
    pub parameters: Vec<(String, String)>,
}

impl InfoPortalRequestPlan {
    pub fn parameters(&self) -> &[(String, String)] {
        &self.parameters
    }

    pub fn query_parameters(&self) -> Option<&[(String, String)]> {
        (self.encoding == InfoParameterEncoding::Query).then_some(&self.parameters)
    }

    pub fn form_parameters(&self) -> Option<&[(String, String)]> {
        (self.encoding == InfoParameterEncoding::Form).then_some(&self.parameters)
    }
}

impl fmt::Debug for InfoPortalRequestPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InfoPortalRequestPlan")
            .field("path", &self.path)
            .field("encoding", &self.encoding)
            .field("parameters", &RedactedParameters(&self.parameters))
            .finish()
    }
}

/// An INFO profile with deployment-specific route and field names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoPortalProfile {
    pub redirect_path: String,
    pub yyfwid_field: String,
    pub csrf_field: String,
    pub machine_field: String,
    pub machine_value: String,
}

impl Default for InfoPortalProfile {
    fn default() -> Self {
        Self {
            redirect_path: DEFAULT_REDIRECT_PATH.to_owned(),
            yyfwid_field: DEFAULT_YYFWID_FIELD.to_owned(),
            csrf_field: DEFAULT_CSRF_FIELD.to_owned(),
            machine_field: DEFAULT_MACHINE_FIELD.to_owned(),
            machine_value: DEFAULT_MACHINE_VALUE.to_owned(),
        }
    }
}

impl InfoPortalProfile {
    /// Creates a profile using the common INFO wire field names.
    pub fn new(redirect_path: impl Into<String>) -> Result<Self, InfoError> {
        let profile = Self {
            redirect_path: redirect_path.into(),
            ..Self::default()
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn standard() -> Self {
        Self::default()
    }

    pub fn online_app_redirect_query(
        &self,
        yyfwid: &str,
        csrf: &str,
    ) -> Result<InfoPortalRequestPlan, InfoError> {
        self.build_request(InfoParameterEncoding::Query, yyfwid, csrf)
    }

    pub fn online_app_redirect_form(
        &self,
        yyfwid: &str,
        csrf: &str,
    ) -> Result<InfoPortalRequestPlan, InfoError> {
        self.build_request(InfoParameterEncoding::Form, yyfwid, csrf)
    }

    pub fn parse_online_app_redirect(&self, body: &str) -> Result<InfoPortalResponse, InfoError> {
        let _ = self;
        parse_online_app_redirect(body)
    }

    fn build_request(
        &self,
        encoding: InfoParameterEncoding,
        yyfwid: &str,
        csrf: &str,
    ) -> Result<InfoPortalRequestPlan, InfoError> {
        self.validate()?;
        require_value(yyfwid, "yyfwid")?;
        require_value(csrf, "_csrf")?;

        Ok(InfoPortalRequestPlan {
            path: self.redirect_path.clone(),
            encoding,
            parameters: vec![
                (self.yyfwid_field.clone(), yyfwid.to_owned()),
                (self.csrf_field.clone(), csrf.to_owned()),
                (self.machine_field.clone(), self.machine_value.clone()),
            ],
        })
    }

    fn validate(&self) -> Result<(), InfoError> {
        validate_path(&self.redirect_path)?;
        validate_field_name(&self.yyfwid_field)?;
        validate_field_name(&self.csrf_field)?;
        validate_field_name(&self.machine_field)?;
        require_value(&self.machine_value, "machine")?;
        Ok(())
    }
}

/// A URL returned by INFO, retained without interpreting WebVPN's host
/// mapping.  The one normalization performed here is the HTML entity
/// `&amp;`, which the observed INFO implementation sometimes leaves in its
/// JSON string even though it is a URL separator.
#[derive(Clone, PartialEq, Eq)]
pub struct OpaqueUrl(String);

pub type WebVpnUrl = OpaqueUrl;

impl OpaqueUrl {
    pub fn new(value: impl Into<String>) -> Result<Self, InfoError> {
        let value = normalize_roaming_url(&value.into());
        if value.trim().is_empty() {
            return Err(InfoError::EmptyRoamingUrl);
        }
        if value.chars().any(char::is_control) {
            return Err(InfoError::InvalidRoamingUrl);
        }
        // The value is server-provided and later becomes a navigation target.
        // Keep it opaque to the application, but still require a URL shape
        // that the HTTP handoff can safely reason about.  Deferring this
        // check to the session adapter would let an invalid scheme or userinfo
        // value cross the INFO response boundary first.
        let parsed = Url::parse(&value).map_err(|_| InfoError::InvalidRoamingUrl)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
            // `url::Url` normalizes an origin-only value to `/`; an opaque
            // roaming target still needs a concrete path beyond that root.
            || parsed.path().is_empty()
            || parsed.path() == "/"
        {
            return Err(InfoError::InvalidRoamingUrl);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("OpaqueUrl")
            .field(&"[redacted]")
            .finish()
    }
}

/// The strictly extracted INFO response value.
#[derive(Clone, PartialEq, Eq)]
pub struct InfoPortalResponse {
    pub roaming_url: OpaqueUrl,
}

impl InfoPortalResponse {
    pub fn roaming_url(&self) -> &OpaqueUrl {
        &self.roaming_url
    }
}

impl fmt::Debug for InfoPortalResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InfoPortalResponse")
            .field("roaming_url", &"[redacted]")
            .finish()
    }
}

/// Extracts exactly `object.roamingurl` from an INFO JSON response.
pub fn parse_online_app_redirect(body: &str) -> Result<InfoPortalResponse, InfoError> {
    let json_body = body.trim_start_matches('\u{feff}');
    let root: Value = serde_json::from_str(json_body).map_err(|error| InfoError::InvalidJson {
        message: error.to_string(),
    })?;
    let root = root.as_object().ok_or(InfoError::WrongFieldType {
        field: "root",
        expected: "object",
    })?;

    if let Some(result) = root.get("result") {
        let result = result.as_str().ok_or(InfoError::ResultNotString)?;
        if !result.eq_ignore_ascii_case("success") {
            return Err(classify_failure(root, result, InfoError::ResultFailure));
        }
    }

    if let Some(success) = root.get("success") {
        match success {
            Value::Bool(true) => {}
            Value::Bool(false) => {
                return Err(classify_failure(
                    root,
                    "success=false",
                    InfoError::SuccessFlagFailure,
                ));
            }
            _ => return Err(InfoError::SuccessNotBoolean),
        }
    }

    if let Some(error) = root.get("error") {
        let is_failure = match error {
            Value::Null | Value::Bool(false) => false,
            Value::Bool(true) => true,
            Value::String(value) => !value.trim().is_empty(),
            _ => true,
        };
        if is_failure {
            return Err(classify_failure(
                root,
                "error",
                InfoError::ErrorFieldFailure,
            ));
        }
    }

    // The portal has also returned `message`/`msg` without a result marker.
    // Do not let a response such as `{message:"未登录", object:{...}}`
    // establish a roaming target, while preserving ordinary success notes.
    for field in ["message", "msg"] {
        let Some(message) = root.get(field).and_then(Value::as_str) else {
            continue;
        };
        if is_login_failure_message(message) {
            return Err(InfoError::LoginRequired);
        }
        if is_business_failure_message(message) {
            return Err(InfoError::MessageFailure);
        }
    }

    let object = root
        .get("object")
        .ok_or(InfoError::MissingField { field: "object" })?
        .as_object()
        .ok_or(InfoError::WrongFieldType {
            field: "object",
            expected: "object",
        })?;
    let roaming_url = object
        .get("roamingurl")
        .ok_or(InfoError::MissingField {
            field: "object.roamingurl",
        })?
        .as_str()
        .ok_or(InfoError::WrongFieldType {
            field: "object.roamingurl",
            expected: "string",
        })?;

    Ok(InfoPortalResponse {
        roaming_url: OpaqueUrl::new(roaming_url)?,
    })
}

pub fn extract_roaming_url(body: &str) -> Result<OpaqueUrl, InfoError> {
    parse_online_app_redirect(body).map(|response| response.roaming_url)
}

fn classify_failure(
    root: &serde_json::Map<String, Value>,
    fallback: &str,
    business_failure: InfoError,
) -> InfoError {
    let has_login_message = ["msg", "message", "error"].iter().any(|field| {
        root.get(*field)
            .and_then(Value::as_str)
            .is_some_and(is_login_failure_message)
    });
    if has_login_message || is_login_failure_message(fallback) {
        InfoError::LoginRequired
    } else if matches!(business_failure, InfoError::ResultFailure) {
        if has_roaming_target(root) {
            InfoError::ResultFailureWithTarget
        } else if object_has_no_roaming_target(root) {
            InfoError::ResultFailureWithObjectNoTarget
        } else if root.contains_key("object") {
            // Keep the envelope shape as a fixed diagnostic only. The object
            // is never treated as a success target after a failed result.
            InfoError::ResultFailureWithObject
        } else if has_message_field(root) {
            InfoError::ResultFailureWithMessage
        } else {
            business_failure
        }
    } else {
        business_failure
    }
}

fn has_message_field(root: &serde_json::Map<String, Value>) -> bool {
    ["msg", "message"]
        .iter()
        .any(|field| root.contains_key(*field))
}

fn object_has_no_roaming_target(root: &serde_json::Map<String, Value>) -> bool {
    let Some(Value::Object(object)) = root.get("object") else {
        return false;
    };
    match object.get("roamingurl") {
        None => true,
        Some(Value::String(value)) => value.trim().is_empty(),
        Some(_) => true,
    }
}

fn has_roaming_target(root: &serde_json::Map<String, Value>) -> bool {
    root.get("object")
        .and_then(Value::as_object)
        .and_then(|object| object.get("roamingurl"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn is_login_failure_message(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if [
        "login success",
        "authenticated",
        "登录成功",
        "认证成功",
        "会话有效",
    ]
    .iter()
    .any(|marker| value == *marker)
        || value == "logged in"
    {
        return false;
    }
    [
        "not logged in",
        "unauthenticated",
        "login required",
        "authentication required",
        "please login",
        "please log in",
        "login timeout",
        "login failed",
        "authentication failed",
        "session expired",
        "session invalid",
        "unauthorized",
        "未登录",
        "请登录",
        "请先登录",
        "请重新登录",
        "需要登录",
        "登录失效",
        "登录超时",
        "登录失败",
        "未认证",
        "认证过期",
        "认证失效",
        "认证失败",
        "会话过期",
        "会话已过期",
        "会话失效",
        "会话已失效",
        "会话超时",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn is_business_failure_message(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || [
            "success",
            "successful",
            "ok",
            "done",
            "completed",
            "成功",
            "完成",
            "正常",
        ]
        .iter()
        .any(|marker| value == *marker)
    {
        return false;
    }
    [
        "error",
        "failed",
        "failure",
        "fail",
        "forbidden",
        "denied",
        "invalid",
        "bad request",
        "错误",
        "失败",
        "异常",
        "拒绝",
        "禁止",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn require_value(value: &str, field: &'static str) -> Result<(), InfoError> {
    if value.trim().is_empty() {
        return Err(InfoError::EmptyValue { field });
    }
    if value.chars().any(char::is_control) {
        return Err(InfoError::InvalidValue { field });
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), InfoError> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['?', '#'])
        || path.contains("..")
        || path.contains("://")
        || path.contains('\\')
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
        || path.chars().any(char::is_control)
    {
        return Err(InfoError::InvalidPath {
            path: path.to_owned(),
        });
    }
    Ok(())
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

fn validate_field_name(name: &str) -> Result<(), InfoError> {
    if name.trim().is_empty()
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '&' | '=' | '?' | '#'))
    {
        return Err(InfoError::InvalidFieldName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn normalize_roaming_url(value: &str) -> String {
    value.replace("&amp;", "&")
}

struct RedactedParameters<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedParameters<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|(name, _value)| DebugParameter { name }))
            .finish()
    }
}

struct DebugParameter<'a> {
    name: &'a str,
}

impl fmt::Debug for DebugParameter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Parameter")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REDIRECT_FIXTURE: &str = r#"{
        "result": "success",
        "object": {
            "roamingurl": "https://webvpn.tsinghua.edu.cn/https/runtime-generated-target/f/info/calendar?from=portal"
        },
        "msg": ""
    }"#;

    #[test]
    fn builds_query_and_form_fixtures_with_distinct_wire_placement() {
        let profile = InfoPortalProfile::default();
        let query = profile
            .online_app_redirect_query("10000ea055dd8d81d09d5a1ba55d39ad", "csrf-value")
            .expect("query plan builds");
        assert_eq!(query.path, DEFAULT_REDIRECT_PATH);
        assert_eq!(query.encoding, InfoParameterEncoding::Query);
        assert_eq!(
            query.parameters,
            vec![
                (
                    "yyfwid".to_owned(),
                    "10000ea055dd8d81d09d5a1ba55d39ad".to_owned()
                ),
                ("_csrf".to_owned(), "csrf-value".to_owned()),
                ("machine".to_owned(), "p".to_owned()),
            ]
        );

        let form = profile
            .online_app_redirect_form("10000ea055dd8d81d09d5a1ba55d39ad", "csrf-value")
            .expect("form plan builds");
        assert_eq!(form.encoding, InfoParameterEncoding::Form);
        assert_eq!(form.form_parameters().unwrap(), form.parameters.as_slice());
        assert!(form.query_parameters().is_none());
    }

    #[test]
    fn strictly_extracts_the_nested_roaming_url_fixture() {
        let parsed = parse_online_app_redirect(REDIRECT_FIXTURE).expect("fixture parses");
        assert_eq!(
            parsed.roaming_url().as_str(),
            "https://webvpn.tsinghua.edu.cn/https/runtime-generated-target/f/info/calendar?from=portal"
        );

        let debug = format!("{parsed:?}");
        assert!(!debug.contains("runtime-generated-target"));
    }

    #[test]
    fn rejects_missing_or_mistyped_nested_values() {
        assert!(matches!(
            parse_online_app_redirect(r#"{"result":"success"}"#),
            Err(InfoError::MissingField { field: "object" })
        ));
        assert!(matches!(
            parse_online_app_redirect(r#"{"object":{"roamingurl":null}}"#),
            Err(InfoError::WrongFieldType {
                field: "object.roamingurl",
                expected: "string"
            })
        ));
        assert!(matches!(
            parse_online_app_redirect(r#"{"object":{"roamingurl":"  "}}"#),
            Err(InfoError::EmptyRoamingUrl)
        ));
        assert!(matches!(
            parse_online_app_redirect(r#"{"object":{"roamingUrl":"wrong-case"}}"#),
            Err(InfoError::MissingField {
                field: "object.roamingurl"
            })
        ));
        assert!(matches!(
            parse_online_app_redirect(
                r#"{"result":"error","object":{"roamingurl":"https://webvpn.example/target"}}"#
            ),
            Err(InfoError::ResultFailureWithTarget)
        ));
        assert!(matches!(
            parse_online_app_redirect(r#"{"result":"error","object":null}"#),
            Err(InfoError::ResultFailureWithObject)
        ));
        assert!(matches!(
            parse_online_app_redirect(r#"{"result":"error","message":"denied"}"#),
            Err(InfoError::ResultFailureWithMessage)
        ));
        assert!(matches!(
            parse_online_app_redirect(
                r#"{"result":true,"object":{"roamingurl":"https://webvpn.example/target"}}"#
            ),
            Err(InfoError::ResultNotString)
        ));
        assert!(matches!(
            parse_online_app_redirect(
                r#"{"success":false,"object":{"roamingurl":"https://webvpn.example/target"}}"#
            ),
            Err(InfoError::SuccessFlagFailure)
        ));
        assert!(matches!(
            parse_online_app_redirect(
                r#"{"success":"false","object":{"roamingurl":"https://webvpn.example/target"}}"#
            ),
            Err(InfoError::SuccessNotBoolean)
        ));
        assert!(matches!(
            parse_online_app_redirect(
                r#"{"error":"permission denied","object":{"roamingurl":"https://webvpn.example/target"}}"#
            ),
            Err(InfoError::ErrorFieldFailure)
        ));
        assert!(matches!(
            parse_online_app_redirect(
                r#"{"message":"permission denied","object":{"roamingurl":"https://webvpn.example/target"}}"#
            ),
            Err(InfoError::MessageFailure)
        ));
    }

    #[test]
    fn validates_profile_values_without_parsing_webvpn_mapping() {
        let profile = InfoPortalProfile::new("/portal/onlineAppRedirect").expect("profile");
        assert!(matches!(
            InfoPortalProfile::new("/portal/../outside"),
            Err(InfoError::InvalidPath { .. })
        ));
        assert!(matches!(
            profile.online_app_redirect_query("", "csrf"),
            Err(InfoError::EmptyValue { field: "yyfwid" })
        ));
        assert!(matches!(
            profile.online_app_redirect_query("service", "  "),
            Err(InfoError::EmptyValue { field: "_csrf" })
        ));

        let opaque =
            OpaqueUrl::new("https://webvpn.example/https/opaque-map/target").expect("opaque URL");
        assert_eq!(
            opaque.as_str(),
            "https://webvpn.example/https/opaque-map/target"
        );
        assert!(!format!("{opaque:?}").contains("opaque-map"));

        let escaped =
            OpaqueUrl::new("https://webvpn.example/target?a=1&amp;b=2").expect("escaped URL");
        assert_eq!(escaped.as_str(), "https://webvpn.example/target?a=1&b=2");

        for invalid in [
            "javascript:alert(1)",
            "file:///tmp/target",
            "https://user:password@webvpn.example/target",
            "https://webvpn.example/target#fragment",
            "https://webvpn.example",
        ] {
            assert!(matches!(
                OpaqueUrl::new(invalid),
                Err(InfoError::InvalidRoamingUrl)
            ));
        }
    }
}
