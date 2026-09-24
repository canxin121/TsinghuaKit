//! Request profiles for the USEREG campus-network self-service portal.
//!
//! USEREG has its own form namespace, CSRF field, and device-management
//! parameters.  Those wire details are kept separate from TUNet and from the
//! unified identity client.  This module plans requests only; encryption,
//! cookie/session handling, and HTML success-page interpretation belong to the
//! client layer that will consume these plans.

use std::fmt;
use std::net::IpAddr;

use thiserror::Error;

const DEFAULT_CAPTCHA_PATH: &str = "/site/captcha";
const DEFAULT_VALIDATE_USER_PATH: &str = "/site/validate-user";
const DEFAULT_LOGIN_PATH: &str = "/login";
const DEFAULT_HOME_PATH: &str = "/home";
const DEFAULT_HOME_DELETE_PATH: &str = "/home/delete";
const DEFAULT_CERTIFICATION_PATH: &str = "/certification";
const DEFAULT_USERS_PATH: &str = "/users";
const DEFAULT_ONLINE_NUM_PATH: &str = "/user/online-num";

const DEFAULT_CSRF_FORM_FIELD: &str = "_csrf-8800";
const DEFAULT_CSRF_HEADER: &str = "X-CSRF-Token";
const DEFAULT_LOGIN_USERNAME_FIELD: &str = "LoginForm[username]";
const DEFAULT_LOGIN_PASSWORD_FIELD: &str = "LoginForm[password]";
const DEFAULT_LOGIN_SMS_CODE_FIELD: &str = "LoginForm[smsCode]";
const DEFAULT_LOGIN_VERIFY_CODE_FIELD: &str = "LoginForm[verifyCode]";
const DEFAULT_CERTIFICATION_IP_FIELD: &str = "CertificationForm[ip]";
const DEFAULT_CERTIFICATION_PASSWORD_FIELD: &str = "CertificationForm[password]";
const DEFAULT_CERTIFICATION_TYPE_FIELD: &str = "CertificationForm[type]";
const DEFAULT_DEVICE_ID_PARAMETER: &str = "id";
const DEFAULT_DEVICE_MAC_PARAMETER: &str = "user_mac";

/// Errors produced by USEREG profile validation and request planning.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UseregError {
    #[error("{field} must not be empty")]
    EmptyValue { field: &'static str },

    #[error("{field} contains invalid control characters")]
    InvalidValue { field: &'static str },

    #[error("invalid USEREG endpoint path for {field}: {path}")]
    InvalidPath { field: &'static str, path: String },

    #[error("invalid USEREG wire field name: {name}")]
    InvalidFieldName { name: String },

    #[error("USEREG wire field name is used more than once: {name}")]
    DuplicateFieldName { name: String },

    #[error("invalid USEREG request plan for {operation:?}: {reason}")]
    InvalidRequestPlan {
        operation: UseregOperation,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregHttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseregOperation {
    Captcha,
    LoginPage,
    ValidateUser,
    Login,
    Home,
    HomeDelete,
    Certification,
    CertificationPage,
    Users,
    OnlineNum,
}

/// Field names and parameter names are profile data rather than shared
/// constants.  This makes a changed deployment visible at the adapter edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregFieldProfile {
    pub csrf_form_field: String,
    pub csrf_header: String,
    pub login_username_field: String,
    pub login_password_field: String,
    pub login_sms_code_field: String,
    pub login_verify_code_field: String,
    pub certification_ip_field: String,
    pub certification_password_field: String,
    pub certification_type_field: String,
    pub device_id_parameter: String,
    pub device_mac_parameter: String,
}

impl Default for UseregFieldProfile {
    fn default() -> Self {
        Self {
            csrf_form_field: DEFAULT_CSRF_FORM_FIELD.to_owned(),
            csrf_header: DEFAULT_CSRF_HEADER.to_owned(),
            login_username_field: DEFAULT_LOGIN_USERNAME_FIELD.to_owned(),
            login_password_field: DEFAULT_LOGIN_PASSWORD_FIELD.to_owned(),
            login_sms_code_field: DEFAULT_LOGIN_SMS_CODE_FIELD.to_owned(),
            login_verify_code_field: DEFAULT_LOGIN_VERIFY_CODE_FIELD.to_owned(),
            certification_ip_field: DEFAULT_CERTIFICATION_IP_FIELD.to_owned(),
            certification_password_field: DEFAULT_CERTIFICATION_PASSWORD_FIELD.to_owned(),
            certification_type_field: DEFAULT_CERTIFICATION_TYPE_FIELD.to_owned(),
            device_id_parameter: DEFAULT_DEVICE_ID_PARAMETER.to_owned(),
            device_mac_parameter: DEFAULT_DEVICE_MAC_PARAMETER.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseregPaths {
    pub captcha: String,
    pub validate_user: String,
    pub login: String,
    pub home: String,
    pub home_delete: String,
    pub certification: String,
    pub users: String,
    pub online_num: String,
}

impl Default for UseregPaths {
    fn default() -> Self {
        Self {
            captcha: DEFAULT_CAPTCHA_PATH.to_owned(),
            validate_user: DEFAULT_VALIDATE_USER_PATH.to_owned(),
            login: DEFAULT_LOGIN_PATH.to_owned(),
            home: DEFAULT_HOME_PATH.to_owned(),
            home_delete: DEFAULT_HOME_DELETE_PATH.to_owned(),
            certification: DEFAULT_CERTIFICATION_PATH.to_owned(),
            users: DEFAULT_USERS_PATH.to_owned(),
            online_num: DEFAULT_ONLINE_NUM_PATH.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UseregProfile {
    pub paths: UseregPaths,
    pub fields: UseregFieldProfile,
}

impl UseregProfile {
    pub fn new(paths: UseregPaths, fields: UseregFieldProfile) -> Result<Self, UseregError> {
        let profile = Self { paths, fields };
        profile.validate()?;
        Ok(profile)
    }

    pub fn captcha_request(&self, refresh: bool) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;
        let mut query = Vec::new();
        if refresh {
            query.push(("refresh".to_owned(), "1".to_owned()));
        }
        Ok(UseregRequestPlan {
            operation: UseregOperation::Captcha,
            method: UseregHttpMethod::Get,
            path: self.paths.captcha.clone(),
            query,
            headers: Vec::new(),
            form: Vec::new(),
        })
    }

    /// Fetches the login page which sets the USEREG session cookie and
    /// contains the per-page CSRF values and RSA public key.
    pub fn login_page_request(&self) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;
        Ok(UseregRequestPlan {
            operation: UseregOperation::LoginPage,
            method: UseregHttpMethod::Get,
            path: self.paths.login.clone(),
            query: Vec::new(),
            headers: Vec::new(),
            form: Vec::new(),
        })
    }

    /// Fetches the image after the refresh request.  The cache-busting query
    /// is kept explicit because the public clients use a fresh value here.
    pub fn captcha_image_request(
        &self,
        cache_buster: &str,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;
        require_value(cache_buster, "captcha cache buster")?;
        Ok(UseregRequestPlan {
            operation: UseregOperation::Captcha,
            method: UseregHttpMethod::Get,
            path: self.paths.captcha.clone(),
            query: vec![("_".to_owned(), cache_buster.to_owned())],
            headers: Vec::new(),
            form: Vec::new(),
        })
    }

    /// Plans the AJAX user-validation call.  Its CSRF value belongs in the
    /// `X-CSRF-Token` header; it is deliberately not emitted as
    /// `_csrf-8800` or as a login form field.
    pub fn validate_user_request(
        &self,
        csrf_header_value: &str,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.validate_user_request_with_password(
            csrf_header_value,
            &credentials.username,
            &credentials.password,
            verify_code,
        )
    }

    /// Same validation request, using the encrypted wire password obtained
    /// from the login page's RSA public key.
    pub fn validate_user_request_with_password(
        &self,
        csrf_header_value: &str,
        username: &str,
        wire_password: &str,
        verify_code: &str,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;
        require_value(csrf_header_value, "X-CSRF-Token")?;
        require_value(username, DEFAULT_LOGIN_USERNAME_FIELD)?;
        require_value(wire_password, DEFAULT_LOGIN_PASSWORD_FIELD)?;
        require_value(verify_code, "LoginForm[verifyCode]")?;

        Ok(UseregRequestPlan {
            operation: UseregOperation::ValidateUser,
            method: UseregHttpMethod::Post,
            path: self.paths.validate_user.clone(),
            query: Vec::new(),
            headers: vec![
                (
                    self.fields.csrf_header.clone(),
                    csrf_header_value.to_owned(),
                ),
                ("X-Requested-With".to_owned(), "XMLHttpRequest".to_owned()),
            ],
            form: vec![
                (
                    self.fields.login_username_field.clone(),
                    username.to_owned(),
                ),
                (
                    self.fields.login_password_field.clone(),
                    wire_password.to_owned(),
                ),
                (
                    self.fields.login_verify_code_field.clone(),
                    verify_code.to_owned(),
                ),
            ],
        })
    }

    /// Plans the form submission after user validation succeeds.  The hidden
    /// `_csrf-8800` value and all `LoginForm[...]` fields stay in this form;
    /// no certification or device field is mixed into it.
    pub fn login_request(
        &self,
        csrf: &UseregCsrfToken,
        credentials: &UseregLoginCredentials,
        verify_code: &str,
        sms_code: Option<&str>,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.login_request_with_password(
            csrf,
            &credentials.username,
            &credentials.password,
            verify_code,
            sms_code,
        )
    }

    /// Same form submission, using the encrypted wire password.  The hidden
    /// CSRF token and login fields remain in the form namespace.
    pub fn login_request_with_password(
        &self,
        csrf: &UseregCsrfToken,
        username: &str,
        wire_password: &str,
        verify_code: &str,
        sms_code: Option<&str>,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;
        require_value(username, DEFAULT_LOGIN_USERNAME_FIELD)?;
        require_value(wire_password, DEFAULT_LOGIN_PASSWORD_FIELD)?;
        require_value(verify_code, "LoginForm[verifyCode]")?;
        let sms_code = sms_code.unwrap_or_default();

        Ok(UseregRequestPlan {
            operation: UseregOperation::Login,
            method: UseregHttpMethod::Post,
            path: self.paths.login.clone(),
            query: Vec::new(),
            headers: Vec::new(),
            form: vec![
                (
                    self.fields.csrf_form_field.clone(),
                    csrf.as_str().to_owned(),
                ),
                (
                    self.fields.login_username_field.clone(),
                    username.to_owned(),
                ),
                (
                    self.fields.login_password_field.clone(),
                    wire_password.to_owned(),
                ),
                (
                    self.fields.login_sms_code_field.clone(),
                    sms_code.to_owned(),
                ),
                (
                    self.fields.login_verify_code_field.clone(),
                    verify_code.to_owned(),
                ),
            ],
        })
    }

    /// Plans device deletion.  The device identity is carried by query
    /// parameters, while `_csrf-8800` remains the sole form CSRF field.
    pub fn home_delete_request(
        &self,
        csrf: &UseregCsrfToken,
        device: &UseregDeviceTarget,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;

        Ok(UseregRequestPlan {
            operation: UseregOperation::HomeDelete,
            method: UseregHttpMethod::Post,
            path: self.paths.home_delete.clone(),
            query: vec![
                (self.fields.device_id_parameter.clone(), device.id.clone()),
                (
                    self.fields.device_mac_parameter.clone(),
                    device.user_mac.clone(),
                ),
            ],
            headers: Vec::new(),
            form: vec![(
                self.fields.csrf_form_field.clone(),
                csrf.as_str().to_owned(),
            )],
        })
    }

    /// Plans device certification.  Its password and type are kept under the
    /// `CertificationForm[...]` namespace and never become login fields.
    pub fn certification_page_request(&self) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;
        Ok(UseregRequestPlan {
            operation: UseregOperation::CertificationPage,
            method: UseregHttpMethod::Get,
            path: self.paths.certification.clone(),
            query: Vec::new(),
            headers: Vec::new(),
            form: Vec::new(),
        })
    }

    /// Plans the device-certification form submission.  Its password and type
    /// are kept under the `CertificationForm[...]` namespace and never become
    /// login fields.
    pub fn certification_request(
        &self,
        csrf: &UseregCsrfToken,
        input: &UseregCertificationInput,
    ) -> Result<UseregRequestPlan, UseregError> {
        self.validate()?;

        Ok(UseregRequestPlan {
            operation: UseregOperation::Certification,
            method: UseregHttpMethod::Post,
            path: self.paths.certification.clone(),
            query: Vec::new(),
            headers: Vec::new(),
            form: vec![
                (
                    self.fields.csrf_form_field.clone(),
                    csrf.as_str().to_owned(),
                ),
                (self.fields.certification_ip_field.clone(), input.ip.clone()),
                (
                    self.fields.certification_password_field.clone(),
                    input.password.clone(),
                ),
                (
                    self.fields.certification_type_field.clone(),
                    input.access_type.wire_value().to_owned(),
                ),
            ],
        })
    }

    /// Validate a public request plan against this profile before it is
    /// resolved into an HTTP request.  The plan type is intentionally public
    /// for inspection, so callers can also mutate it after construction; the
    /// client layer repeats this check at the transport boundary.
    pub fn validate_request(&self, request: &UseregRequestPlan) -> Result<(), UseregError> {
        self.validate()?;
        let operation = request.operation;
        let (expected_method, expected_path) = match operation {
            UseregOperation::Captcha => (UseregHttpMethod::Get, &self.paths.captcha),
            UseregOperation::LoginPage => (UseregHttpMethod::Get, &self.paths.login),
            UseregOperation::ValidateUser => (UseregHttpMethod::Post, &self.paths.validate_user),
            UseregOperation::Login => (UseregHttpMethod::Post, &self.paths.login),
            UseregOperation::Home => (UseregHttpMethod::Get, &self.paths.home),
            UseregOperation::HomeDelete => (UseregHttpMethod::Post, &self.paths.home_delete),
            UseregOperation::Certification => (UseregHttpMethod::Post, &self.paths.certification),
            UseregOperation::CertificationPage => {
                (UseregHttpMethod::Get, &self.paths.certification)
            }
            UseregOperation::Users => (UseregHttpMethod::Get, &self.paths.users),
            UseregOperation::OnlineNum => (UseregHttpMethod::Get, &self.paths.online_num),
        };

        if request.method != expected_method {
            return Err(UseregError::InvalidRequestPlan {
                operation,
                reason: "HTTP method does not match the operation",
            });
        }
        if request.path != *expected_path {
            return Err(UseregError::InvalidRequestPlan {
                operation,
                reason: "request path does not match the profile",
            });
        }

        validate_wire_pairs(&request.query, false)?;
        validate_wire_pairs(&request.headers, true)?;
        validate_wire_pairs(&request.form, false)?;

        if request.method == UseregHttpMethod::Get && !request.form.is_empty() {
            return Err(UseregError::InvalidRequestPlan {
                operation,
                reason: "GET requests must not contain form fields",
            });
        }

        match operation {
            UseregOperation::Captcha => {
                if !request.headers.is_empty() || !request.form.is_empty() {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "captcha requests only use the documented query",
                    });
                }
                match request.query.as_slice() {
                    [] => {}
                    [(name, value)] if name == "refresh" && value == "1" => {}
                    [(name, value)] if name == "_" && !value.trim().is_empty() => {}
                    _ => {
                        return Err(UseregError::InvalidRequestPlan {
                            operation,
                            reason: "captcha query must be refresh=1 or one cache buster",
                        });
                    }
                }
            }
            UseregOperation::LoginPage
            | UseregOperation::Home
            | UseregOperation::CertificationPage
            | UseregOperation::Users
            | UseregOperation::OnlineNum => {
                if !request.query.is_empty()
                    || !request.headers.is_empty()
                    || !request.form.is_empty()
                {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "this GET route does not accept query, header, or form fields",
                    });
                }
            }
            UseregOperation::ValidateUser => {
                if !request.query.is_empty() || request.headers.len() != 2 {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "validation request has an unexpected envelope",
                    });
                }
                require_header(&request.headers, &self.fields.csrf_header, None, operation)?;
                require_header(
                    &request.headers,
                    "X-Requested-With",
                    Some("XMLHttpRequest"),
                    operation,
                )?;
                require_form_names(
                    &request.form,
                    &[
                        &self.fields.login_username_field,
                        &self.fields.login_password_field,
                        &self.fields.login_verify_code_field,
                    ],
                    operation,
                )?;
                require_nonempty_form(&request.form, &self.fields.login_username_field, operation)?;
                require_nonempty_form(&request.form, &self.fields.login_password_field, operation)?;
                require_nonempty_form(
                    &request.form,
                    &self.fields.login_verify_code_field,
                    operation,
                )?;
            }
            UseregOperation::Login => {
                if !request.query.is_empty() || !request.headers.is_empty() {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "login request must not contain query or headers",
                    });
                }
                require_form_names(
                    &request.form,
                    &[
                        &self.fields.csrf_form_field,
                        &self.fields.login_username_field,
                        &self.fields.login_password_field,
                        &self.fields.login_sms_code_field,
                        &self.fields.login_verify_code_field,
                    ],
                    operation,
                )?;
                require_nonempty_form(&request.form, &self.fields.csrf_form_field, operation)?;
                require_nonempty_form(&request.form, &self.fields.login_username_field, operation)?;
                require_nonempty_form(&request.form, &self.fields.login_password_field, operation)?;
                require_nonempty_form(
                    &request.form,
                    &self.fields.login_verify_code_field,
                    operation,
                )?;
            }
            UseregOperation::HomeDelete => {
                if request.headers.len() != 0 || request.query.len() != 2 {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "device deletion query or headers are incomplete",
                    });
                }
                require_query_names(
                    &request.query,
                    &[
                        &self.fields.device_id_parameter,
                        &self.fields.device_mac_parameter,
                    ],
                    operation,
                )?;
                let id = require_nonempty_query(
                    &request.query,
                    &self.fields.device_id_parameter,
                    operation,
                )?;
                if !id.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "device id must contain only decimal digits",
                    });
                }
                require_nonempty_query(
                    &request.query,
                    &self.fields.device_mac_parameter,
                    operation,
                )?;
                require_form_names(&request.form, &[&self.fields.csrf_form_field], operation)?;
                require_nonempty_form(&request.form, &self.fields.csrf_form_field, operation)?;
            }
            UseregOperation::Certification => {
                if !request.query.is_empty() || !request.headers.is_empty() {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "certification request must not contain query or headers",
                    });
                }
                require_form_names(
                    &request.form,
                    &[
                        &self.fields.csrf_form_field,
                        &self.fields.certification_ip_field,
                        &self.fields.certification_password_field,
                        &self.fields.certification_type_field,
                    ],
                    operation,
                )?;
                require_nonempty_form(&request.form, &self.fields.csrf_form_field, operation)?;
                let ip = require_nonempty_form(
                    &request.form,
                    &self.fields.certification_ip_field,
                    operation,
                )?;
                if ip.trim() != ip || ip.parse::<IpAddr>().is_err() {
                    return Err(UseregError::InvalidRequestPlan {
                        operation,
                        reason: "certification IP is not a valid address",
                    });
                }
                require_nonempty_form(
                    &request.form,
                    &self.fields.certification_password_field,
                    operation,
                )?;
                require_nonempty_form(
                    &request.form,
                    &self.fields.certification_type_field,
                    operation,
                )?;
            }
        }

        Ok(())
    }

    fn validate(&self) -> Result<(), UseregError> {
        validate_path("captcha", &self.paths.captcha)?;
        validate_path("validate_user", &self.paths.validate_user)?;
        validate_path("login", &self.paths.login)?;
        validate_path("home", &self.paths.home)?;
        validate_path("home_delete", &self.paths.home_delete)?;
        validate_path("certification", &self.paths.certification)?;
        validate_path("users", &self.paths.users)?;
        validate_path("online_num", &self.paths.online_num)?;

        let names = [
            &self.fields.csrf_form_field,
            &self.fields.csrf_header,
            &self.fields.login_username_field,
            &self.fields.login_password_field,
            &self.fields.login_sms_code_field,
            &self.fields.login_verify_code_field,
            &self.fields.certification_ip_field,
            &self.fields.certification_password_field,
            &self.fields.certification_type_field,
            &self.fields.device_id_parameter,
            &self.fields.device_mac_parameter,
        ];
        for name in names {
            validate_field_name(name)?;
        }

        // A profile is data supplied at the deployment boundary.  If two
        // logical values share one wire name, `reqwest::RequestBuilder::form`
        // will emit duplicate fields and the server may accept whichever one
        // it happens to read first.  Such a profile cannot describe the
        // observed USEREG protocol safely, so reject it before any request is
        // built.
        for (index, name) in names.iter().enumerate() {
            if names[..index].iter().any(|previous| *previous == *name) {
                return Err(UseregError::DuplicateFieldName {
                    name: (*name).clone(),
                });
            }
        }
        Ok(())
    }
}

fn validate_wire_pairs(pairs: &[(String, String)], headers: bool) -> Result<(), UseregError> {
    for (index, (name, value)) in pairs.iter().enumerate() {
        validate_field_name(name)?;
        if value.chars().any(char::is_control) {
            return Err(UseregError::InvalidValue {
                field: "USEREG request value",
            });
        }
        if pairs[..index].iter().any(|(previous, _)| {
            if headers {
                previous.eq_ignore_ascii_case(name)
            } else {
                previous == name
            }
        }) {
            return Err(UseregError::DuplicateFieldName { name: name.clone() });
        }
    }
    Ok(())
}

fn invalid_request(operation: UseregOperation, reason: &'static str) -> UseregError {
    UseregError::InvalidRequestPlan { operation, reason }
}

fn require_header(
    headers: &[(String, String)],
    expected_name: &str,
    expected_value: Option<&str>,
    operation: UseregOperation,
) -> Result<(), UseregError> {
    let matches = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(expected_name))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(invalid_request(
            operation,
            "required validation header is missing or duplicated",
        ));
    }
    let value = matches[0].1.as_str();
    if value.trim().is_empty() || expected_value.is_some_and(|expected| value != expected) {
        return Err(invalid_request(
            operation,
            "validation header value is invalid",
        ));
    }
    Ok(())
}

fn require_form_names(
    form: &[(String, String)],
    expected: &[&String],
    operation: UseregOperation,
) -> Result<(), UseregError> {
    if form.len() != expected.len()
        || form
            .iter()
            .zip(expected.iter())
            .any(|((name, _), expected)| name != expected.as_str())
    {
        return Err(invalid_request(
            operation,
            "form fields do not match the documented order",
        ));
    }
    Ok(())
}

fn require_query_names(
    query: &[(String, String)],
    expected: &[&String],
    operation: UseregOperation,
) -> Result<(), UseregError> {
    if query.len() != expected.len()
        || query
            .iter()
            .zip(expected.iter())
            .any(|((name, _), expected)| name != expected.as_str())
    {
        return Err(invalid_request(
            operation,
            "query fields do not match the documented order",
        ));
    }
    Ok(())
}

fn require_nonempty_form<'a>(
    form: &'a [(String, String)],
    name: &str,
    operation: UseregOperation,
) -> Result<&'a str, UseregError> {
    form.iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_request(operation, "required form value is empty"))
}

fn require_nonempty_query<'a>(
    query: &'a [(String, String)],
    name: &str,
    operation: UseregOperation,
) -> Result<&'a str, UseregError> {
    query
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_request(operation, "required query value is empty"))
}

/// A planned HTTP request.  The three collections are deliberately separate
/// so query device parameters, headers, and form namespaces cannot be
/// accidentally merged by a caller.
#[derive(Clone, PartialEq, Eq)]
pub struct UseregRequestPlan {
    pub operation: UseregOperation,
    pub method: UseregHttpMethod,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub form: Vec<(String, String)>,
}

impl fmt::Debug for UseregRequestPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregRequestPlan")
            .field("operation", &self.operation)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &RedactedParameters(&self.query))
            .field("headers", &RedactedParameters(&self.headers))
            .field("form", &RedactedParameters(&self.form))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UseregCsrfToken(String);

impl UseregCsrfToken {
    pub fn new(value: impl Into<String>) -> Result<Self, UseregError> {
        let value = value.into();
        require_value(&value, DEFAULT_CSRF_FORM_FIELD)?;
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(UseregError::InvalidValue {
                field: DEFAULT_CSRF_FORM_FIELD,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UseregCsrfToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("UseregCsrfToken")
            .field(&"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UseregLoginCredentials {
    pub username: String,
    password: String,
}

impl UseregLoginCredentials {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, UseregError> {
        let username = username.into();
        let password = password.into();
        require_value(&username, DEFAULT_LOGIN_USERNAME_FIELD)?;
        require_value(&password, DEFAULT_LOGIN_PASSWORD_FIELD)?;
        Ok(Self { username, password })
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for UseregLoginCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregLoginCredentials")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UseregCertificationType {
    Internal,
    Internet,
    Other(String),
}

impl UseregCertificationType {
    pub fn wire_value(&self) -> &str {
        match self {
            Self::Internal => "in",
            Self::Internet => "out",
            Self::Other(value) => value,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UseregCertificationInput {
    pub ip: String,
    password: String,
    pub access_type: UseregCertificationType,
}

impl UseregCertificationInput {
    pub fn new(
        ip: impl Into<String>,
        password: impl Into<String>,
        access_type: UseregCertificationType,
    ) -> Result<Self, UseregError> {
        let ip = ip.into();
        let password = password.into();
        require_value(&ip, DEFAULT_CERTIFICATION_IP_FIELD)?;
        if ip.trim() != ip || ip.parse::<IpAddr>().is_err() {
            return Err(UseregError::InvalidValue {
                field: DEFAULT_CERTIFICATION_IP_FIELD,
            });
        }
        require_value(&password, DEFAULT_CERTIFICATION_PASSWORD_FIELD)?;
        require_value(access_type.wire_value(), DEFAULT_CERTIFICATION_TYPE_FIELD)?;
        Ok(Self {
            ip,
            password,
            access_type,
        })
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for UseregCertificationInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregCertificationInput")
            .field("ip", &self.ip)
            .field("password", &"[redacted]")
            .field("access_type", &self.access_type)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct UseregDeviceTarget {
    id: String,
    user_mac: String,
}

impl UseregDeviceTarget {
    pub fn new(id: impl Into<String>, user_mac: impl Into<String>) -> Result<Self, UseregError> {
        let id = id.into();
        let user_mac = user_mac.into();
        require_value(&id, DEFAULT_DEVICE_ID_PARAMETER)?;
        require_value(&user_mac, DEFAULT_DEVICE_MAC_PARAMETER)?;
        if !id.chars().all(|character| character.is_ascii_digit()) {
            return Err(UseregError::InvalidValue {
                field: DEFAULT_DEVICE_ID_PARAMETER,
            });
        }
        Ok(Self { id, user_mac })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn user_mac(&self) -> &str {
        &self.user_mac
    }
}

impl fmt::Debug for UseregDeviceTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UseregDeviceTarget")
            .field("id", &"[redacted]")
            .field("user_mac", &"[redacted]")
            .finish()
    }
}

fn require_value(value: &str, field: &'static str) -> Result<(), UseregError> {
    if value.trim().is_empty() {
        return Err(UseregError::EmptyValue { field });
    }
    if value.chars().any(char::is_control) {
        return Err(UseregError::InvalidValue { field });
    }
    Ok(())
}

fn validate_path(field: &'static str, path: &str) -> Result<(), UseregError> {
    if !path.starts_with('/')
        || path.contains(['?', '#'])
        || path.chars().any(char::is_control)
        || !is_safe_route_path(path)
    {
        return Err(UseregError::InvalidPath {
            field,
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// Keep profile paths inside the configured WebVPN mapping directory. Raw dot
/// segments are unsafe by themselves; percent encoded dot or separator bytes
/// are rejected too because a proxy may decode them before normalising the
/// request target.
pub(crate) fn is_safe_route_path(path: &str) -> bool {
    if path.contains('\\') {
        return false;
    }

    path.split('/').all(|segment| {
        if matches!(segment, "." | "..") {
            return false;
        }
        let Some(decoded) = decode_path_segment(segment) else {
            return false;
        };
        decoded != b"." && decoded != b".." && !decoded.contains(&b'/') && !decoded.contains(&b'\\')
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

fn validate_field_name(name: &str) -> Result<(), UseregError> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'[' | b']')
        })
    {
        return Err(UseregError::InvalidFieldName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

struct RedactedParameters<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedParameters<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(
                self.0
                    .iter()
                    .map(|(name, value)| DebugParameter { name, value }),
            )
            .finish()
    }
}

struct DebugParameter<'a> {
    name: &'a str,
    value: &'a str,
}

impl fmt::Debug for DebugParameter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = if sensitive_field(self.name) {
            "[redacted]"
        } else {
            self.value
        };
        formatter
            .debug_struct("Parameter")
            .field("name", &self.name)
            .field("value", &value)
            .finish()
    }
}

fn sensitive_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("csrf")
        || name.contains("token")
        || name.contains("pass")
        || name.contains("verify")
        || name.contains("smscode")
        || name.contains("username")
        || name == "id"
        || name == "user_mac"
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGIN_FIXTURE_USERNAME: &str = "20260001@example.edu.cn";
    const LOGIN_FIXTURE_PASSWORD: &str = "encrypted-password-fixture";
    const LOGIN_FIXTURE_VERIFY_CODE: &str = "7K4P";
    const CSRF_FIXTURE: &str = "csrf-8800-fixture";

    #[test]
    fn keeps_captcha_and_validation_plans_independent() {
        let profile = UseregProfile::default();
        let captcha = profile.captcha_request(true).expect("captcha plan");
        assert_eq!(captcha.operation, UseregOperation::Captcha);
        assert_eq!(captcha.method, UseregHttpMethod::Get);
        assert_eq!(captcha.path, "/site/captcha");
        assert_eq!(captcha.query, vec![("refresh".to_owned(), "1".to_owned())]);
        assert!(captcha.form.is_empty());

        let credentials =
            UseregLoginCredentials::new(LOGIN_FIXTURE_USERNAME, LOGIN_FIXTURE_PASSWORD)
                .expect("credentials");
        let validate = profile
            .validate_user_request("meta-csrf-fixture", &credentials, LOGIN_FIXTURE_VERIFY_CODE)
            .expect("validate plan");
        assert_eq!(validate.operation, UseregOperation::ValidateUser);
        assert_eq!(validate.path, "/site/validate-user");
        assert_eq!(
            validate.headers,
            vec![
                ("X-CSRF-Token".to_owned(), "meta-csrf-fixture".to_owned()),
                ("X-Requested-With".to_owned(), "XMLHttpRequest".to_owned()),
            ]
        );
        assert_eq!(
            validate.form,
            vec![
                (
                    "LoginForm[username]".to_owned(),
                    LOGIN_FIXTURE_USERNAME.to_owned()
                ),
                (
                    "LoginForm[password]".to_owned(),
                    LOGIN_FIXTURE_PASSWORD.to_owned()
                ),
                (
                    "LoginForm[verifyCode]".to_owned(),
                    LOGIN_FIXTURE_VERIFY_CODE.to_owned()
                ),
            ]
        );
        assert!(!validate.form.iter().any(|(name, _)| name == "_csrf-8800"));
    }

    #[test]
    fn keeps_login_certification_and_device_namespaces_separate() {
        let profile = UseregProfile::default();
        let csrf = UseregCsrfToken::new(CSRF_FIXTURE).expect("csrf");
        let credentials =
            UseregLoginCredentials::new(LOGIN_FIXTURE_USERNAME, LOGIN_FIXTURE_PASSWORD)
                .expect("credentials");

        let login = profile
            .login_request(&csrf, &credentials, LOGIN_FIXTURE_VERIFY_CODE, Some(""))
            .expect("login plan");
        assert_eq!(login.path, "/login");
        assert_eq!(
            login.form,
            vec![
                ("_csrf-8800".to_owned(), CSRF_FIXTURE.to_owned()),
                (
                    "LoginForm[username]".to_owned(),
                    LOGIN_FIXTURE_USERNAME.to_owned()
                ),
                (
                    "LoginForm[password]".to_owned(),
                    LOGIN_FIXTURE_PASSWORD.to_owned()
                ),
                ("LoginForm[smsCode]".to_owned(), String::new()),
                (
                    "LoginForm[verifyCode]".to_owned(),
                    LOGIN_FIXTURE_VERIFY_CODE.to_owned()
                ),
            ]
        );

        let device = UseregDeviceTarget::new("32123", "71-FF-FF-C8-02-3A").expect("device");
        let delete = profile
            .home_delete_request(&csrf, &device)
            .expect("delete plan");
        assert_eq!(delete.path, "/home/delete");
        assert_eq!(
            delete.query,
            vec![
                ("id".to_owned(), "32123".to_owned()),
                ("user_mac".to_owned(), "71-FF-FF-C8-02-3A".to_owned()),
            ]
        );
        assert_eq!(
            delete.form,
            vec![("_csrf-8800".to_owned(), CSRF_FIXTURE.to_owned())]
        );

        let certification = UseregCertificationInput::new(
            "166.111.231.123",
            "network-password-fixture",
            UseregCertificationType::Internet,
        )
        .expect("certification input");
        let certify = profile
            .certification_request(&csrf, &certification)
            .expect("certification plan");
        assert_eq!(certify.path, "/certification");
        assert_eq!(
            certify.form,
            vec![
                ("_csrf-8800".to_owned(), CSRF_FIXTURE.to_owned()),
                (
                    "CertificationForm[ip]".to_owned(),
                    "166.111.231.123".to_owned()
                ),
                (
                    "CertificationForm[password]".to_owned(),
                    "network-password-fixture".to_owned()
                ),
                ("CertificationForm[type]".to_owned(), "out".to_owned()),
            ]
        );
        assert!(
            !certify
                .form
                .iter()
                .any(|(name, _)| name.starts_with("LoginForm["))
        );

        let certification_page = profile
            .certification_page_request()
            .expect("certification page plan");
        assert_eq!(
            certification_page.operation,
            UseregOperation::CertificationPage
        );
        assert_eq!(certification_page.method, UseregHttpMethod::Get);
        assert_eq!(certification_page.path, "/certification");
        assert!(certification_page.query.is_empty());
        assert!(certification_page.form.is_empty());
    }

    #[test]
    fn rejects_empty_sensitive_values_and_redacts_debug_output() {
        assert!(matches!(
            UseregCsrfToken::new("  "),
            Err(UseregError::EmptyValue {
                field: "_csrf-8800"
            })
        ));
        assert!(matches!(
            UseregLoginCredentials::new("user", ""),
            Err(UseregError::EmptyValue {
                field: "LoginForm[password]"
            })
        ));

        let csrf = UseregCsrfToken::new(CSRF_FIXTURE).expect("csrf");
        let credentials =
            UseregLoginCredentials::new(LOGIN_FIXTURE_USERNAME, LOGIN_FIXTURE_PASSWORD)
                .expect("credentials");
        assert!(!format!("{csrf:?}").contains(CSRF_FIXTURE));
        assert!(!format!("{credentials:?}").contains(LOGIN_FIXTURE_PASSWORD));

        let plan = UseregProfile::default()
            .login_request(&csrf, &credentials, LOGIN_FIXTURE_VERIFY_CODE, None)
            .expect("login plan");
        let debug = format!("{plan:?}");
        assert!(!debug.contains(CSRF_FIXTURE));
        assert!(!debug.contains(LOGIN_FIXTURE_PASSWORD));
        assert!(debug.contains("LoginForm[password]"));
        assert!(matches!(
            UseregCsrfToken::new("csrf with whitespace"),
            Err(UseregError::InvalidValue {
                field: "_csrf-8800"
            })
        ));
    }

    #[test]
    fn rejects_invalid_device_and_certification_wire_values_before_request_planning() {
        assert!(matches!(
            UseregDeviceTarget::new("17/other", "AA-BB-CC"),
            Err(UseregError::InvalidValue { field: "id" })
        ));
        assert!(matches!(
            UseregCertificationInput::new(
                "not-an-ip",
                "network-password-fixture",
                UseregCertificationType::Internet,
            ),
            Err(UseregError::InvalidValue {
                field: "CertificationForm[ip]"
            })
        ));
        assert!(matches!(
            UseregLoginCredentials::new("fixture\nuser", "password"),
            Err(UseregError::InvalidValue {
                field: "LoginForm[username]"
            })
        ));
    }

    #[test]
    fn rejects_raw_and_percent_encoded_path_escape_segments() {
        for path in [
            "/../login",
            "/./login",
            "/%2e%2e/login",
            "/%2E%2E/login",
            "/site/%2f../login",
            "/site/%5c..%5clogin",
            "/site/%",
            "/site\\..\\login",
        ] {
            assert!(
                !is_safe_route_path(path),
                "unsafe route path unexpectedly accepted: {path}"
            );
        }
        assert!(is_safe_route_path("/site/captcha"));
        assert!(is_safe_route_path("/webvpn/%E6%B8%85%E5%8D%8E"));
    }

    #[test]
    fn rejects_profiles_that_alias_two_logical_wire_fields() {
        let mut fields = UseregFieldProfile::default();
        fields.login_sms_code_field = fields.login_verify_code_field.clone();
        let error = UseregProfile::new(UseregPaths::default(), fields)
            .expect_err("aliased form fields must not be sent");
        assert!(matches!(
            error,
            UseregError::DuplicateFieldName { name }
                if name == "LoginForm[verifyCode]"
        ));
    }
}
