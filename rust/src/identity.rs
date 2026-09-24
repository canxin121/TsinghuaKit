//! Data-only models for the unified identity login protocol.
//!
//! The protocol has several deployment-specific values.  This module keeps
//! those values in profiles and enums so that a later client does not turn
//! one observed deployment into a global constant.  It deliberately stores
//! credential field names, never credential values, and contains no password
//! encryption or request code.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityLoginProfile {
    /// The application id belongs to this login entry profile.
    pub app_id: String,
    /// The page route is kept configurable because the host and route are
    /// deployment details rather than identity-wide constants.
    pub login_page_path: String,
    pub login_form: LoginFormProfile,
    pub second_auth: SecondAuthProfile,
    /// The optional trusted-device endpoint is kept separate from the login
    /// form because the two public clients disagree about optional fields.
    pub trusted_device: Option<TrustedDeviceProfile>,
}

impl IdentityLoginProfile {
    pub fn new(
        app_id: impl Into<String>,
        login_page_path: impl Into<String>,
        login_form: LoginFormProfile,
        second_auth: SecondAuthProfile,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            login_page_path: login_page_path.into(),
            login_form,
            second_auth,
            trusted_device: Some(TrustedDeviceProfile::common()),
        }
    }

    /// Replaces the trusted-device profile with an explicitly configured
    /// deployment profile.
    pub fn with_trusted_device(mut self, profile: TrustedDeviceProfile) -> Self {
        self.trusted_device = Some(profile);
        self
    }

    /// Disables saveFinger for a deployment whose profile has not been
    /// verified.
    pub fn without_trusted_device(mut self) -> Self {
        self.trusted_device = None;
        self
    }
}

/// Wire profile for the identity service's saveFinger endpoint.
///
/// The endpoint and the first three fields are present in the recent
/// thu-calendar-sync and thu-info-app implementations.  `single_login_field`
/// is optional because those implementations disagree about sending it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDeviceProfile {
    pub endpoint_path: String,
    pub fingerprint_field: String,
    pub device_name_field: String,
    pub decision_field: String,
    pub decision_value: String,
    pub single_login_field: Option<String>,
    pub single_login_value: Option<String>,
}

impl TrustedDeviceProfile {
    pub fn new(
        endpoint_path: impl Into<String>,
        fingerprint_field: impl Into<String>,
        device_name_field: impl Into<String>,
        decision_field: impl Into<String>,
        decision_value: impl Into<String>,
    ) -> Self {
        Self {
            endpoint_path: endpoint_path.into(),
            fingerprint_field: fingerprint_field.into(),
            device_name_field: device_name_field.into(),
            decision_field: decision_field.into(),
            decision_value: decision_value.into(),
            single_login_field: None,
            single_login_value: None,
        }
    }

    /// The common field set observed in the two recent reference clients.
    /// The disputed `singleLogin` field is deliberately omitted.
    pub fn common() -> Self {
        Self::new(
            "/b/doubleAuth/personal/saveFinger",
            "fingerprint",
            "deviceName",
            "radioVal",
            "是",
        )
    }

    /// Enables the optional field used by thu-calendar-sync.
    pub fn with_single_login(
        mut self,
        field_name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.single_login_field = Some(field_name.into());
        self.single_login_value = Some(value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFormProfile {
    /// The endpoint is separate from the page route and can change by
    /// deployment.
    pub submit_path: String,
    pub encoding: FormEncoding,
    pub fields: LoginFormFields,
}

impl LoginFormProfile {
    pub fn new(
        submit_path: impl Into<String>,
        encoding: FormEncoding,
        fields: LoginFormFields,
    ) -> Self {
        Self {
            submit_path: submit_path.into(),
            encoding,
            fields,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEncoding {
    /// `application/x-www-form-urlencoded`.
    UrlEncoded,
    /// `multipart/form-data`; the boundary is intentionally left to the
    /// eventual HTTP client.
    Multipart,
    /// A profile-specific media type that has not been standardized here.
    Other(String),
}

impl FormEncoding {
    pub fn content_type(&self) -> Option<&str> {
        match self {
            Self::UrlEncoded => Some("application/x-www-form-urlencoded"),
            Self::Multipart => Some("multipart/form-data"),
            Self::Other(value) => Some(value.as_str()),
        }
    }
}

/// Names of fields in the login form.  These are wire names, not submitted
/// values.  Keeping the password field name is enough for a later adapter to
/// construct a request without allowing this model to retain a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFormFields {
    pub username_field: String,
    pub password_field: String,
    pub device_name_field: Option<String>,
    pub fingerprint_field: Option<String>,
    pub generated_fingerprint_field: Option<String>,
    pub generated_fingerprint_v3_field: Option<String>,
    pub captcha_field: Option<String>,
    pub single_login_field: Option<String>,
}

impl LoginFormFields {
    /// The field names observed in the common unified-auth login form.  This
    /// is a starting profile; callers can replace any field name or remove an
    /// optional field for a different deployment.
    pub fn common() -> Self {
        Self {
            username_field: String::from("i_user"),
            password_field: String::from("i_pass"),
            device_name_field: Some(String::from("deviceName")),
            fingerprint_field: Some(String::from("fingerPrint")),
            generated_fingerprint_field: Some(String::from("fingerGenPrint")),
            generated_fingerprint_v3_field: Some(String::from("fingerGenPrint3")),
            captcha_field: Some(String::from("i_captcha")),
            single_login_field: Some(String::from("singleLogin")),
        }
    }
}

/// One hidden value copied from the currently fetched identity login form.
///
/// The identity deployment changes these values with the WebVPN/OAuth
/// bootstrap. They are request data rather than static profile configuration.
/// The value stays in Rust and is redacted from diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub struct LoginFormHiddenField {
    name: String,
    value: String,
}

impl LoginFormHiddenField {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for LoginFormHiddenField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginFormHiddenField")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondAuthProfile {
    pub endpoint_path: String,
    /// Parameter names are configurable because the action and approach
    /// values are not a universal contract across deployments.
    pub method_field: String,
    pub action_field: String,
    /// The verification-code field used by code and TOTP actions.
    pub verification_code_field: String,
    pub available_methods: Vec<SecondAuthMethod>,
    pub actions: SecondAuthActions,
}

impl SecondAuthProfile {
    pub fn new(
        endpoint_path: impl Into<String>,
        method_field: impl Into<String>,
        action_field: impl Into<String>,
        available_methods: Vec<SecondAuthMethod>,
        actions: SecondAuthActions,
    ) -> Self {
        Self {
            endpoint_path: endpoint_path.into(),
            method_field: method_field.into(),
            action_field: action_field.into(),
            verification_code_field: String::from("vericode"),
            available_methods,
            actions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondAuthActions {
    pub find_approaches: Option<SecondAuthAction>,
    pub send_code: Option<SecondAuthAction>,
    pub verify_code: Option<SecondAuthAction>,
    pub verify_totp_code: Option<SecondAuthAction>,
}

impl SecondAuthActions {
    pub fn new(
        find_approaches: Option<SecondAuthAction>,
        send_code: Option<SecondAuthAction>,
        verify_code: Option<SecondAuthAction>,
        verify_totp_code: Option<SecondAuthAction>,
    ) -> Self {
        Self {
            find_approaches,
            send_code,
            verify_code,
            verify_totp_code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondAuthRequest {
    /// Some actions, such as finding available approaches, may not need a
    /// method value.
    pub method: Option<SecondAuthMethod>,
    pub action: SecondAuthAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecondAuthMethod {
    Wechat,
    Mobile,
    Sms,
    Totp,
    /// Preserve a deployment-specific wire value instead of collapsing it
    /// into one of the known methods.
    Other(String),
}

impl SecondAuthMethod {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "wechat" => Self::Wechat,
            "mobile" => Self::Mobile,
            "sms" => Self::Sms,
            "totp" => Self::Totp,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn wire_value(&self) -> &str {
        match self {
            Self::Wechat => "wechat",
            Self::Mobile => "mobile",
            Self::Sms => "sms",
            Self::Totp => "totp",
            Self::Other(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecondAuthAction {
    FindApproaches,
    SendCode,
    VerifyCode,
    VerifyTotpCode,
    /// The spelling and value of an action are service-profile data.
    Other(String),
}

impl SecondAuthAction {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "FIND_APPROACHES" => Self::FindApproaches,
            "SEND_CODE" => Self::SendCode,
            // Keep the service's `VERITY_*` spelling exactly as observed.
            "VERITY_CODE" => Self::VerifyCode,
            "VERITY_TOTP_CODE" => Self::VerifyTotpCode,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn wire_value(&self) -> &str {
        match self {
            Self::FindApproaches => "FIND_APPROACHES",
            Self::SendCode => "SEND_CODE",
            Self::VerifyCode => "VERITY_CODE",
            Self::VerifyTotpCode => "VERITY_TOTP_CODE",
            Self::Other(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPageParseResult {
    pub sm2_public_key: Option<Sm2PublicKey>,
    pub anchor_ticket: Option<AnchorTicket>,
    /// Absence of an invalidation marker is deliberately not treated as proof
    /// of a valid session.  A later parser can combine this with URL and page
    /// structure checks.
    pub invalidation: InvalidationMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sm2PublicKey {
    /// Usually `sm2publicKey`; retaining the actual field name allows a
    /// profile to report a changed page without losing the raw evidence.
    pub field_name: String,
    pub value: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AnchorTicket {
    pub href: String,
    /// The ticket is opaque.  An anchor can be present while its ticket is
    /// absent or cannot be extracted, so the result keeps that distinction.
    pub ticket: Option<String>,
}

impl fmt::Debug for AnchorTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnchorTicket")
            .field("href", &"[redacted]")
            .field("ticket", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidationMarker {
    pub status: InvalidationStatus,
    pub field_name: Option<String>,
    pub raw_value: Option<String>,
}

impl InvalidationMarker {
    pub fn not_found() -> Self {
        Self {
            status: InvalidationStatus::NotFound,
            field_name: None,
            raw_value: None,
        }
    }

    pub fn marked(field_name: impl Into<String>, raw_value: Option<String>) -> Self {
        Self {
            status: InvalidationStatus::Marked,
            field_name: Some(field_name.into()),
            raw_value,
        }
    }

    pub fn unknown(field_name: Option<String>, raw_value: Option<String>) -> Self {
        Self {
            status: InvalidationStatus::Unknown,
            field_name,
            raw_value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidationStatus {
    /// No known invalidation marker was found.  This is not a success claim.
    NotFound,
    /// A marker was found and interpreted as an invalid-session signal.
    Marked,
    /// A candidate marker was found but its meaning is not known to this
    /// profile.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_profile_keeps_app_id_and_form_encoding_configurable() {
        let fields = LoginFormFields::common();
        let form = LoginFormProfile::new(
            "/do/off/ui/auth/login/check",
            FormEncoding::Multipart,
            fields.clone(),
        );
        let second_auth = SecondAuthProfile::new(
            "/b/doubleAuth/login",
            "type",
            "action",
            vec![SecondAuthMethod::Mobile, SecondAuthMethod::Totp],
            SecondAuthActions::new(
                Some(SecondAuthAction::FindApproaches),
                Some(SecondAuthAction::SendCode),
                Some(SecondAuthAction::VerifyCode),
                Some(SecondAuthAction::VerifyTotpCode),
            ),
        );
        let profile = IdentityLoginProfile::new(
            "profile-specific-app",
            "/do/off/ui/auth/login/form/{appId}/0",
            form,
            second_auth,
        );

        assert_eq!(profile.app_id, "profile-specific-app");
        assert_eq!(profile.login_form.encoding, FormEncoding::Multipart);
        assert_eq!(profile.login_form.fields, fields);
        assert_eq!(profile.second_auth.endpoint_path, "/b/doubleAuth/login");
        assert_eq!(
            profile
                .trusted_device
                .as_ref()
                .map(|profile| profile.endpoint_path.as_str()),
            Some("/b/doubleAuth/personal/saveFinger")
        );
    }

    #[test]
    fn form_encoding_exposes_only_the_media_type_shape() {
        assert_eq!(
            FormEncoding::UrlEncoded.content_type(),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(
            FormEncoding::Multipart.content_type(),
            Some("multipart/form-data")
        );
        assert_eq!(
            FormEncoding::Other(String::from("application/custom")).content_type(),
            Some("application/custom")
        );
    }

    #[test]
    fn common_form_contains_field_names_without_credential_values() {
        let fields = LoginFormFields::common();

        assert_eq!(fields.username_field, "i_user");
        assert_eq!(fields.password_field, "i_pass");
        assert_eq!(fields.device_name_field.as_deref(), Some("deviceName"));
        assert_eq!(fields.fingerprint_field.as_deref(), Some("fingerPrint"));
        assert_eq!(
            fields.generated_fingerprint_field.as_deref(),
            Some("fingerGenPrint")
        );
        assert_eq!(
            fields.generated_fingerprint_v3_field.as_deref(),
            Some("fingerGenPrint3")
        );
        assert_eq!(fields.captcha_field.as_deref(), Some("i_captcha"));
        assert_eq!(fields.single_login_field.as_deref(), Some("singleLogin"));
    }

    #[test]
    fn second_auth_methods_and_actions_round_trip_known_wire_values() {
        for (wire, method) in [
            ("wechat", SecondAuthMethod::Wechat),
            ("mobile", SecondAuthMethod::Mobile),
            ("totp", SecondAuthMethod::Totp),
            (
                "device_specific",
                SecondAuthMethod::Other(String::from("device_specific")),
            ),
        ] {
            assert_eq!(SecondAuthMethod::from_wire(wire), method);
            assert_eq!(method.wire_value(), wire);
        }

        for (wire, action) in [
            ("FIND_APPROACHES", SecondAuthAction::FindApproaches),
            ("SEND_CODE", SecondAuthAction::SendCode),
            ("VERITY_CODE", SecondAuthAction::VerifyCode),
            ("VERITY_TOTP_CODE", SecondAuthAction::VerifyTotpCode),
            (
                "SERVICE_DEFINED",
                SecondAuthAction::Other(String::from("SERVICE_DEFINED")),
            ),
        ] {
            assert_eq!(SecondAuthAction::from_wire(wire), action);
            assert_eq!(action.wire_value(), wire);
        }
    }

    #[test]
    fn second_auth_request_keeps_method_and_action_independent() {
        let request = SecondAuthRequest {
            method: Some(SecondAuthMethod::Mobile),
            action: SecondAuthAction::VerifyCode,
        };

        assert_eq!(request.method, Some(SecondAuthMethod::Mobile));
        assert_eq!(request.action.wire_value(), "VERITY_CODE");
    }

    #[test]
    fn login_page_result_preserves_optional_evidence_and_invalid_state() {
        let result = LoginPageParseResult {
            sm2_public_key: Some(Sm2PublicKey {
                field_name: String::from("sm2publicKey"),
                value: String::from("opaque-public-key"),
            }),
            anchor_ticket: Some(AnchorTicket {
                href: String::from("/roaming/entry?ticket=opaque-ticket"),
                ticket: Some(String::from("opaque-ticket")),
            }),
            invalidation: InvalidationMarker::marked("loginInvalid", Some(String::from("true"))),
        };

        assert_eq!(
            result
                .sm2_public_key
                .as_ref()
                .map(|key| key.field_name.as_str()),
            Some("sm2publicKey")
        );
        assert_eq!(
            result
                .anchor_ticket
                .as_ref()
                .and_then(|anchor| anchor.ticket.as_deref()),
            Some("opaque-ticket")
        );
        assert_eq!(result.invalidation.status, InvalidationStatus::Marked);
        assert_eq!(result.invalidation.raw_value.as_deref(), Some("true"));
    }

    #[test]
    fn missing_and_unknown_invalidation_markers_remain_distinct() {
        let missing = InvalidationMarker::not_found();
        let unknown = InvalidationMarker::unknown(
            Some(String::from("session_state")),
            Some(String::from("pending")),
        );

        assert_eq!(missing.status, InvalidationStatus::NotFound);
        assert_eq!(unknown.status, InvalidationStatus::Unknown);
        assert_eq!(unknown.field_name.as_deref(), Some("session_state"));
        assert_eq!(unknown.raw_value.as_deref(), Some("pending"));
    }

    #[test]
    fn trusted_device_profile_keeps_the_disputed_single_login_field_explicit() {
        let common = TrustedDeviceProfile::common();
        assert_eq!(common.fingerprint_field, "fingerprint");
        assert_eq!(common.device_name_field, "deviceName");
        assert_eq!(common.decision_field, "radioVal");
        assert_eq!(common.decision_value, "是");
        assert!(common.single_login_field.is_none());

        let calendar_profile = common.with_single_login("singleLogin", "yes");
        assert_eq!(
            calendar_profile.single_login_field.as_deref(),
            Some("singleLogin")
        );
        assert_eq!(calendar_profile.single_login_value.as_deref(), Some("yes"));
    }
}
