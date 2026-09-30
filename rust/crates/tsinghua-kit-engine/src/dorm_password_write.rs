//! The dormitory service's own password reset (家园网 / myhome), dispatched once.
//!
//! The dormitory electricity pages and the dormitory password form are served by
//! the same legacy ASP.NET application behind the same WebVPN mapping, which the
//! read half of this SDK already proves.  This module adds the one write the same
//! session can carry: replacing the dormitory account's own password.
//!
//! Four rules shape the module:
//!
//! * **A write leaves exactly once.**  The plan is dispatched through
//!   `CampusHttpTransport::execute_once_exclusive`, which takes the whole request
//!   gate and returns the first response without following a redirect.  Nothing
//!   here re-authenticates and re-sends, because a password change that may
//!   already be in effect must never be replayed.
//! * **The new password never leaves this module.**  It is held in
//!   [`DormPassword`], which zeroizes on drop and whose `Debug` prints its length
//!   and nothing else.  It is copied straight into the one body it belongs to,
//!   is never a plan field a caller can read back, and is never logged.
//! * **The form's own state is what the service sent.**  The reset is an ASP.NET
//!   postback: the hidden fields of the page this session fetched are echoed
//!   verbatim, so a caller can never hand this module a body it invented.  The
//!   three fields this module sets — the event target and the two password fields
//!   — are the ones the observed client sets, with the old-password field sent
//!   **empty**, which is what the service's own client sends.
//! * **No refusal is invented.**  No response of this route has been observed, so
//!   the observed client discards the answer and reports nothing but success.  A
//!   caller is therefore told the outcome is *unconfirmed* unless the answer
//!   carries affirmative evidence in one of the shapes the campus services are
//!   known to use.  There is no `Refused` outcome, because no refusal wording
//!   exists to recognise; see [`classify_dorm_password_write`].
//!
//! The route and its selector are the observed ones.  This module reimplements the
//! contract; it does not copy the reference client's source, fixtures or session
//! code.

use std::{fmt, time::Duration};

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

use crate::campus_html::{self, PageClass};
use crate::transport::{CampusHttpTransport, CampusTextResponse, TransportError};

/// The dormitory password form.  Both the form fetch and the submission use this
/// exact route; the second request is a postback to the page that rendered it.
pub const DORM_CHANGE_PASSWORD_PATH: &str = "/Netweb_List/ChangePassword.aspx";

/// The identity roaming selector the observed client uses for this application.
///
/// It is recorded because it is the selector this route's own client roams with
/// under its `id` policy — a policy this engine deliberately does not implement as
/// a second campus login.  The reset therefore rides the electricity session the
/// read half proves, which reaches the same mapping this selector resolves to.
pub const DORM_CHANGE_PASSWORD_WEBVPN_TARGET: &str = "051bb58cba58a1c5f67857606497387f";

/// The identifier of the field whose presence says the page really is the reset
/// form.  Its absence is the observed client's own authentication error.
pub const DORM_CHANGE_PASSWORD_ANCHOR: &str = "ChangePasswordCtrl1_txtoldpassword";

/// The postback target the observed client submits.  The separator is a colon:
/// this page's control tree uses one, and a `$` here would address nothing.
pub const DORM_CHANGE_PASSWORD_EVENT_TARGET: &str = "ChangePasswordCtrl1:btnOK";

/// The wire name of the old-password field.
pub const DORM_CHANGE_PASSWORD_OLD_FIELD: &str = "ChangePasswordCtrl1$txtoldpassword";

/// The wire name of the replacement-password field.
pub const DORM_CHANGE_PASSWORD_NEW_FIELD: &str = "ChangePasswordCtrl1$txtnewpassword";

/// The wire name of the replacement-password confirmation field.
pub const DORM_CHANGE_PASSWORD_CONFIRM_FIELD: &str = "ChangePasswordCtrl1$txtnewpassword1";

/// The longest password this module will put into the dormitory body.
///
/// The service's own rule is not observed, so this is this module's local bound:
/// it exists so a mistyped value is refused here rather than sent as a password
/// nobody could have meant.
pub const MAX_DORM_PASSWORD_CHARS: usize = 64;

const MAX_WRITE_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_FORM_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_FORM_FIELDS: usize = 512;
const MAX_FORM_VALUE_BYTES: usize = 256 * 1024;
const USER_AGENT: &str = "THYou/dorm-password";

/// The only HTTP method this module dispatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormPasswordWriteMethod {
    Post,
}

/// The one operation this module performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormPasswordWriteOperation {
    ResetHomePassword,
}

impl DormPasswordWriteOperation {
    /// Returns the service path without an origin or a query string.
    pub const fn path(self) -> &'static str {
        match self {
            Self::ResetHomePassword => DORM_CHANGE_PASSWORD_PATH,
        }
    }

    /// Returns whether this operation changes the account's own state.
    ///
    /// It does: the only operation here is a password replacement.  The method
    /// exists so a caller can state that rather than assume it.
    pub const fn is_write(self) -> bool {
        true
    }

    /// Returns the stable, lowercase identifier used in telemetry and errors.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResetHomePassword => "dorm_reset_home_password",
        }
    }
}

impl fmt::Display for DormPasswordWriteOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The runtime must establish this session before the plan may exist.
///
/// The reset route is on the same mapped application the electricity reads use,
/// so the prerequisite is that proven read session rather than a second handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormPasswordWriteSessionPrerequisite {
    ExistingElectricitySession,
}

/// A validated dormitory password, alive only for the one request it belongs to.
///
/// The value is **not** trimmed: a password may legitimately contain a space, and
/// silently changing what a caller typed would mean setting a different password
/// than the one they entered.  What is refused is a value that is empty, longer
/// than [`MAX_DORM_PASSWORD_CHARS`], made only of whitespace, or carrying a
/// control character that could not survive being a form field.
pub struct DormPassword(Zeroizing<String>);

impl DormPassword {
    /// Validates one password.
    pub fn new(value: impl Into<String>) -> Result<Self, DormPasswordRequestError> {
        let value = value.into();
        if value.is_empty() || value.trim().is_empty() {
            return Err(DormPasswordRequestError::EmptySecret);
        }
        if value.chars().count() > MAX_DORM_PASSWORD_CHARS {
            return Err(DormPasswordRequestError::SecretTooLong {
                max: MAX_DORM_PASSWORD_CHARS,
            });
        }
        if value.chars().any(char::is_control) {
            return Err(DormPasswordRequestError::SecretInvalid);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    /// Returns the password for the one body it belongs to.
    ///
    /// Crate-visible on purpose: the value may only reach a request this crate is
    /// about to dispatch, never a DTO, a plan field or an error.
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }

    /// Returns how many characters the password holds.
    pub fn len(&self) -> usize {
        self.0.chars().count()
    }

    /// Returns whether the password is empty.  It never is: [`Self::new`] refuses
    /// an empty value, and this method exists only so a reader does not have to
    /// infer that from the constructor.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for DormPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DormPassword")
            .field("chars", &self.len())
            .finish_non_exhaustive()
    }
}

impl Drop for DormPassword {
    fn drop(&mut self) {
        // `Zeroizing` already clears on drop; this is explicit so the guarantee
        // survives someone later replacing the inner type.
        self.0.zeroize();
    }
}

/// Errors raised while building a dormitory password reset plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DormPasswordRequestError {
    #[error("the dormitory password is empty")]
    EmptySecret,

    #[error("the dormitory password is longer than {max} characters")]
    SecretTooLong { max: usize },

    #[error("the dormitory password contains a control character")]
    SecretInvalid,
}

/// A typed dormitory password reset plan.
///
/// The plan carries the operation, the method, the prerequisite and the password.
/// The form state the service itself rendered is **not** part of the plan: it is
/// read by the adapter from the live page and handed back at dispatch, so a caller
/// cannot substitute a body of its own making.
///
/// The type is deliberately **not** `PartialEq` and not `Clone`.  Equality over a
/// plan would either compare secrets, which would make a password comparable
/// outside this module, or ignore them, which would report two different requests
/// as the same one.  A caller that wants to send the same change twice builds a
/// second plan.
pub struct DormPasswordWritePlan {
    operation: DormPasswordWriteOperation,
    method: DormPasswordWriteMethod,
    prerequisite: DormPasswordWriteSessionPrerequisite,
    password: DormPassword,
}

impl DormPasswordWritePlan {
    pub const fn operation(&self) -> DormPasswordWriteOperation {
        self.operation
    }

    pub const fn method(&self) -> DormPasswordWriteMethod {
        self.method
    }

    pub const fn session_prerequisite(&self) -> DormPasswordWriteSessionPrerequisite {
        self.prerequisite
    }

    /// Returns the service path without an origin or a query string.
    pub const fn path(&self) -> &'static str {
        self.operation.path()
    }

    /// Returns how many characters the replacement password holds.
    pub fn password_len(&self) -> usize {
        self.password.len()
    }

    /// Returns the wire field names this plan's body sets, with no values.
    ///
    /// The hidden fields carried over from the service's own page are absent
    /// because they are the page's, not the plan's; what is left is exactly the
    /// fields this module decides.
    pub fn body_fields(&self) -> Vec<&'static str> {
        vec![
            "__EVENTTARGET",
            DORM_CHANGE_PASSWORD_OLD_FIELD,
            DORM_CHANGE_PASSWORD_NEW_FIELD,
            DORM_CHANGE_PASSWORD_CONFIRM_FIELD,
        ]
    }

    /// Returns the fields this plan adds to the service's own form state.
    ///
    /// Crate-visible because it is the one place the password becomes a wire
    /// value.  The old-password field is sent empty, which is what the observed
    /// client sends; the confirmation field repeats the new password because the
    /// service's own form validates the two against each other.
    pub(crate) fn fields(&self) -> Vec<(&'static str, String)> {
        let password = self.password.expose().to_owned();
        vec![
            (
                "__EVENTTARGET",
                DORM_CHANGE_PASSWORD_EVENT_TARGET.to_owned(),
            ),
            (DORM_CHANGE_PASSWORD_OLD_FIELD, String::new()),
            (DORM_CHANGE_PASSWORD_NEW_FIELD, password.clone()),
            (DORM_CHANGE_PASSWORD_CONFIRM_FIELD, password),
        ]
    }
}

impl fmt::Debug for DormPasswordWritePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DormPasswordWritePlan")
            .field("operation", &self.operation.as_str())
            .field("method", &self.method)
            .field("prerequisite", &self.prerequisite)
            .field("has_relative_path", &self.path().starts_with('/'))
            .field("password_chars", &self.password.len())
            .field("body_fields", &self.body_fields())
            .finish()
    }
}

/// Builds this module's write plans from validated inputs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DormPasswordWriteProfile;

impl DormPasswordWriteProfile {
    pub const fn new() -> Self {
        Self
    }

    /// Builds the plan for one dormitory password replacement.
    pub fn reset_request(&self, password: DormPassword) -> DormPasswordWritePlan {
        DormPasswordWritePlan {
            operation: DormPasswordWriteOperation::ResetHomePassword,
            method: DormPasswordWriteMethod::Post,
            prerequisite: DormPasswordWriteSessionPrerequisite::ExistingElectricitySession,
            password,
        }
    }
}

/// The service's own reset form, as this session fetched it.
///
/// The fields are the page's hidden inputs, echoed verbatim into the postback:
/// the service's own view state, its generator and its event validation are all
/// carried by the page rather than reconstructed here, so a request this module
/// dispatches is a postback of the page it actually received.
///
/// `Debug` prints the field names and counts only: a form's values are the
/// service's state, not something to put in a log.
pub struct DormPasswordFormState {
    fields: Vec<(String, String)>,
}

impl DormPasswordFormState {
    /// Builds the state of a page whose form carried no hidden fields.
    ///
    /// This is a legal postback — the observed client echoes whatever the page
    /// offers and requires no particular field — so it exists rather than being
    /// treated as a failure.
    pub fn empty() -> Self {
        Self { fields: Vec::new() }
    }

    /// Returns how many hidden fields the page carried.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns whether the page carried no hidden fields at all.
    ///
    /// This is not an error on its own: the observed client echoes whatever the
    /// page offers and does not require any particular field to be present.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Returns the field names the page carried, with no values.
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.iter().map(|(name, _)| name.as_str()).collect()
    }

    pub(crate) fn fields(&self) -> &[(String, String)] {
        &self.fields
    }
}

impl fmt::Debug for DormPasswordFormState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DormPasswordFormState")
            .field("field_count", &self.fields.len())
            .field("field_names", &self.field_names())
            .finish()
    }
}

/// A parsing failure on the reset form.  It never retains response bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DormPasswordParseError {
    #[error("the dormitory password form response was empty")]
    EmptyBody,

    #[error("the dormitory password form response is a login page")]
    LoginPage,

    #[error("the dormitory password form response is a session-expiry page")]
    ExpiredPage,

    #[error("the dormitory password form response is not HTML")]
    UnexpectedHtml,

    #[error("the dormitory password form is not present on the page")]
    FormMissing,

    #[error("the dormitory password form is larger than this module will echo")]
    TooLarge,
}

/// Reads the service's own reset form out of one page.
///
/// The login and expiry markers are checked before any field is scraped, so an
/// HTTP 200 login page cannot become an empty form that then produces a request
/// carrying a password.  The readiness anchor is the field the observed client
/// itself requires: without it the page is not the reset form and this module
/// refuses rather than posting to a route it cannot identify.
pub fn parse_change_password_form(
    html: &str,
) -> Result<DormPasswordFormState, DormPasswordParseError> {
    let trimmed = html.trim_start_matches('\u{feff}').trim_start();
    if trimmed.trim().is_empty() {
        return Err(DormPasswordParseError::EmptyBody);
    }
    match crate::campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(DormPasswordParseError::LoginPage),
        PageClass::Expired => return Err(DormPasswordParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    if !looks_like_html_body(trimmed) {
        return Err(DormPasswordParseError::UnexpectedHtml);
    }

    let inputs = campus_html::scan(trimmed, "input").map_err(|_| {
        // A page too large to scan, or one whose input tags never close, is not
        // a form this module can echo faithfully.  Reporting it as unreadable
        // rather than as a shorter field list is the only honest answer.
        DormPasswordParseError::TooLarge
    })?;
    let anchor_present = inputs
        .iter()
        .any(|element| element.attr("id") == Some(DORM_CHANGE_PASSWORD_ANCHOR));
    if !anchor_present {
        return Err(DormPasswordParseError::FormMissing);
    }

    let mut fields: Vec<(String, String)> = Vec::new();
    let mut bytes = 0usize;
    for element in &inputs {
        if !element
            .attr("type")
            .is_some_and(|value| value.eq_ignore_ascii_case("hidden"))
        {
            continue;
        }
        // An unnamed field addresses nothing, so it is not part of the form the
        // service reads back.  The observed client's own scrape keys on the
        // name attribute and would put an empty key into its body; echoing an
        // unnamed field is worse than leaving it out.
        let Some(name) = element.attr("name").filter(|name| !name.is_empty()) else {
            continue;
        };
        let value = element.attr("value").unwrap_or_default();
        bytes = bytes.saturating_add(name.len()).saturating_add(value.len());
        if fields.len() >= MAX_FORM_FIELDS || bytes > MAX_FORM_VALUE_BYTES {
            return Err(DormPasswordParseError::TooLarge);
        }
        fields.push((name.to_owned(), value.to_owned()));
    }

    Ok(DormPasswordFormState { fields })
}

/// Classifies the service's answer to a dormitory password reset.
///
/// This is the honest reading of a route with no observed answer.  The reference
/// client sends the postback and discards whatever comes back, so it offers **no**
/// acceptance evidence at all; inheriting "any answer is fine" would report a
/// refused or unrecognised write as a success.  This module therefore accepts only
/// an answer that says so in one of the shapes the campus services are known to
/// use, and reports everything else — including the service's own re-rendered
/// form — as an outcome it could not confirm:
///
/// * an empty body or a body of only whitespace: the legacy ASP.NET handler
///   answers with no payload when it applies a change;
/// * the literal `OK`;
/// * a JSON envelope whose `status`/`result`/`success` is `1`, `true` or the
///   literal `"success"`, with no conventional failure marker anywhere in it.
///
/// There is deliberately no refusal outcome.  No refusal wording for this route
/// has been observed, and inventing one — or reading a re-rendered form as a
/// refusal — would tell a caller their password is unchanged when the service may
/// simply have answered on the page it was already showing.
pub fn classify_dorm_password_write(body: &str) -> DormPasswordWriteOutcome {
    let trimmed = body.trim_start_matches('\u{feff}').trim();
    if trimmed.len() > MAX_WRITE_RESPONSE_BYTES || looks_like_html(trimmed) {
        return DormPasswordWriteOutcome::Unrecognized;
    }
    if trimmed.is_empty() {
        return DormPasswordWriteOutcome::Accepted;
    }
    if trimmed.eq_ignore_ascii_case("ok") {
        return DormPasswordWriteOutcome::Accepted;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return DormPasswordWriteOutcome::Unrecognized;
    };
    let Some(envelope) = value.as_object() else {
        return DormPasswordWriteOutcome::Unrecognized;
    };
    if has_write_failure_marker(envelope) {
        return DormPasswordWriteOutcome::Unrecognized;
    }
    for field in ["status", "result", "success"] {
        match envelope.get(field) {
            Some(serde_json::Value::Number(number)) => {
                // `1` is the only accepted number.  A zero says nothing this
                // module can act on — the legacy campus envelopes use zero for
                // success in places — so it is unreadable rather than a refusal.
                return if number.as_i64() == Some(1) {
                    DormPasswordWriteOutcome::Accepted
                } else {
                    DormPasswordWriteOutcome::Unrecognized
                };
            }
            Some(serde_json::Value::Bool(true)) => return DormPasswordWriteOutcome::Accepted,
            Some(serde_json::Value::String(text))
                if text.trim().eq_ignore_ascii_case("success") =>
            {
                return DormPasswordWriteOutcome::Accepted;
            }
            _ => {}
        }
    }
    DormPasswordWriteOutcome::Unrecognized
}

/// Reports whether an answer carries conventional failure evidence.
///
/// A marker here does not become a refusal: this route has no observed refusal,
/// so the answer is reported as unreadable instead.  What the check buys is that
/// an explicit failure is never read as an acceptance.
fn has_write_failure_marker(envelope: &serde_json::Map<String, serde_json::Value>) -> bool {
    use serde_json::Value;
    let text_marker = |value: &Value| {
        value.as_str().is_some_and(|text| {
            let text = text.trim().to_ascii_lowercase();
            matches!(
                text.as_str(),
                "error"
                    | "fail"
                    | "failed"
                    | "forgotten"
                    | "forbidden"
                    | "unauthorized"
                    | "错误"
                    | "失败"
                    | "无权限"
                    | "拒绝"
            ) || text.starts_with("error ")
                || text.starts_with("fail")
                || text.starts_with("forbidden")
                || text.starts_with("unauthorized")
        })
    };
    envelope.get("success").and_then(Value::as_bool) == Some(false)
        || envelope.get("result").and_then(Value::as_bool) == Some(false)
        || ["message", "msg", "error", "errorMessage"]
            .iter()
            .filter_map(|field| envelope.get(*field))
            .any(text_marker)
}

/// What a dispatched dormitory password reset turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DormPasswordWriteOutcome {
    /// The service's own answer carried affirmative acceptance evidence.
    Accepted,
    /// The session is gone, so nothing was applied and the caller must sign in.
    LoginRequired,
    /// The request left this process without an answer this module could read.
    ///
    /// This is the expected answer for this route.  It means the request was
    /// dispatched and its effect is unknown, which is exactly what a caller must
    /// be told: the new password may or may not be in effect, and the way to find
    /// out is to use it, never to send the change again.
    Unrecognized,
}

/// Configuration for the dormitory password reset adapter.
///
/// It only ever builds a standalone transport for fixtures.  The live runtime
/// hands the adapter the session transport it already proved, so this module
/// never creates a second cookie jar.
#[derive(Clone)]
pub struct DormPasswordWriteAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl DormPasswordWriteAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, DormPasswordWriteAdapterError> {
        Self::with_user_agent_and_timeout(base_url, USER_AGENT, Duration::from_secs(20))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, DormPasswordWriteAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(20))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, DormPasswordWriteAdapterError> {
        let base_url =
            Url::parse(base_url).map_err(|_| DormPasswordWriteAdapterError::InvalidBaseUrl)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(DormPasswordWriteAdapterError::InvalidBaseUrl);
        }
        Ok(Self {
            base_url: normalize_base_url(base_url)?,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, DormPasswordWriteAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(|_| DormPasswordWriteAdapterError::InvalidBaseUrl)
    }
}

/// Errors raised while reading or dispatching the dormitory password reset.
#[derive(Debug, Error)]
pub enum DormPasswordWriteAdapterError {
    #[error("the dormitory password route base URL is invalid")]
    InvalidBaseUrl,

    #[error("the dormitory password route could not be reached")]
    Transport(#[source] TransportError),

    #[error("the dormitory password route answered {status}")]
    HttpStatus { status: StatusCode },

    #[error("the dormitory password response came from another origin")]
    UnexpectedOrigin,

    #[error("the dormitory password response is for another route")]
    UnexpectedPath,

    #[error("the dormitory password response is not HTML")]
    UnexpectedContentType,

    #[error("the dormitory session has expired or is not established")]
    SessionExpired,

    #[error("the dormitory password form is missing from the response")]
    FormMissing,

    #[error("the dormitory password page template is not recognized")]
    UnexpectedDeployment,

    #[error("the dormitory password route is not the expected operation")]
    WriteOperation,

    #[error("the dormitory password form could not be parsed: {0}")]
    Parse(#[source] DormPasswordParseError),
}

impl DormPasswordWriteAdapterError {
    /// Returns the stable, lowercase code the runtime records for this failure.
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "dorm_config",
            Self::Transport(_) => "dorm_network",
            Self::HttpStatus { .. } => "dorm_http",
            Self::UnexpectedOrigin => "dorm_origin",
            Self::UnexpectedPath => "dorm_path",
            Self::UnexpectedContentType => "dorm_content_type",
            Self::SessionExpired => "dorm_write_session_expired",
            Self::FormMissing => "dorm_write_form_missing",
            Self::UnexpectedDeployment => "dorm_write_template",
            Self::WriteOperation => "dorm_write_request",
            Self::Parse(DormPasswordParseError::LoginPage)
            | Self::Parse(DormPasswordParseError::ExpiredPage) => "dorm_write_session_expired",
            Self::Parse(_) => "dorm_write_form_unreadable",
        }
    }

    /// Returns whether this failure means the session must be established again.
    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(DormPasswordParseError::LoginPage)
                | Self::Parse(DormPasswordParseError::ExpiredPage)
        )
    }
}

/// One-shot dormitory password reset client.
///
/// `try_with_transport` is the runtime entry point: the transport must be the one
/// that already carries the identity/INFO/WebVPN cookie jar, and the base URL must
/// be the mapped root the electricity read half proved.
pub struct DormPasswordWriteAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: DormPasswordWriteProfile,
}

impl DormPasswordWriteAdapter {
    pub fn new(
        config: DormPasswordWriteAdapterConfig,
    ) -> Result<Self, DormPasswordWriteAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, DormPasswordWriteAdapterError> {
        Ok(Self {
            base_url: normalize_base_url(base_url)?,
            transport,
            profile: DormPasswordWriteProfile::new(),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> DormPasswordWriteProfile {
        self.profile
    }

    /// Builds the plan for one password replacement.
    pub fn reset_request(&self, password: DormPassword) -> DormPasswordWritePlan {
        self.profile.reset_request(password)
    }

    /// Fetches the service's own reset form and returns its hidden state.
    ///
    /// This is a GET, so the shared redirect policy may follow it; the form's
    /// readiness anchor is what says the session actually reached the reset page.
    pub async fn read_reset_form(
        &self,
    ) -> Result<DormPasswordFormState, DormPasswordWriteAdapterError> {
        let endpoint = self.endpoint(DORM_CHANGE_PASSWORD_PATH)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| {
                DormPasswordWriteAdapterError::Transport(TransportError::Request(error))
            })?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| DormPasswordWriteAdapterError::UnexpectedOrigin)?;
        let location_target = location
            .as_deref()
            .map(|value| resolve_location(&final_url, value))
            .transpose()?;
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || location_target.as_ref().is_some_and(|target| {
                same_origin(&self.base_url, target) && looks_like_login_url(target)
            })
        {
            return Err(DormPasswordWriteAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(DormPasswordWriteAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(DormPasswordWriteAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| {
                DormPasswordWriteAdapterError::Transport(TransportError::Decode(error))
            })?;
        if body.len() > MAX_FORM_RESPONSE_BYTES {
            return Err(DormPasswordWriteAdapterError::UnexpectedDeployment);
        }
        if status != StatusCode::OK {
            return Err(DormPasswordWriteAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.query().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(DormPasswordWriteAdapterError::UnexpectedPath);
        }
        if !is_html_content_type(content_type.as_deref()) {
            return Err(DormPasswordWriteAdapterError::UnexpectedContentType);
        }
        parse_change_password_form(&body).map_err(Self::map_parse_error)
    }

    /// Dispatches one password reset, exactly once.
    ///
    /// The body is the form state this adapter fetched plus the three fields the
    /// plan sets, and it is dispatched through the exclusive gate, which returns
    /// the first response without following a redirect.  A transport failure after
    /// the body left is reported as an unconfirmed outcome rather than as an
    /// error: the request has already been sent, and nothing here sends it again.
    pub async fn reset_password(
        &self,
        plan: &DormPasswordWritePlan,
        form: &DormPasswordFormState,
    ) -> Result<DormPasswordWriteOutcome, DormPasswordWriteAdapterError> {
        if plan.operation() != DormPasswordWriteOperation::ResetHomePassword
            || plan.method() != DormPasswordWriteMethod::Post
            || plan.path() != DORM_CHANGE_PASSWORD_PATH
        {
            return Err(DormPasswordWriteAdapterError::WriteOperation);
        }
        let endpoint = self.endpoint(plan.path())?;
        let expected_path = endpoint.path().to_owned();
        let mut fields: Vec<(String, String)> = form.fields().to_vec();
        fields.extend(
            plan.fields()
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value)),
        );
        let request = self
            .transport
            .client()
            .post(endpoint)
            .form(&fields)
            .build()
            .map_err(|_| DormPasswordWriteAdapterError::InvalidBaseUrl)?;
        // The body has been encoded into the request, so the plaintext copy this
        // module built is cleared before the request is dispatched.
        drop(fields);
        let response = match self
            .transport
            .execute_once_exclusive(self.transport.client(), request)
            .await
        {
            Ok(response) => response,
            Err(_error) => {
                // The request left this process, so its effect cannot be told
                // apart from an answer that was lost.  Nothing is sent again and
                // the caller is told the outcome is unknown.  The transport error
                // is deliberately dropped: it may name the request URL, and the
                // body this module sent carried the new password.
                return Ok(DormPasswordWriteOutcome::Unrecognized);
            }
        };
        let status = response.status();
        let final_url = response.url().clone();
        let redirect_location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        // A session that is gone is decided from the status and the redirect
        // alone, before the body is read: the answer is not dispatch evidence in
        // any case, and reading it first would only mean holding a page this
        // module has already decided it cannot use.
        let login_redirect = redirect_location.as_deref().is_some_and(|location| {
            resolve_location(&final_url, location).is_ok_and(|target| looks_like_login_url(&target))
        });
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || login_redirect
        {
            return Ok(DormPasswordWriteOutcome::LoginRequired);
        }
        let bytes =
            match crate::telemetry::timing::read_bounded_bytes(response, MAX_WRITE_RESPONSE_BYTES)
                .await
            {
                Ok(bytes) => bytes,
                Err(_error) => {
                    // The request has already left, so an oversized or truncated
                    // answer leaves the outcome unknown rather than failed.
                    return Ok(DormPasswordWriteOutcome::Unrecognized);
                }
            };
        let response = CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location: redirect_location.clone(),
            body: String::from_utf8_lossy(&bytes).into_owned(),
        };
        if matches!(
            campus_html::classify_page(&response.body),
            PageClass::Login | PageClass::Expired
        ) {
            // A WebVPN login page and an HTTP-200 session-expiry page are both the
            // deployment saying the session is gone.  Neither is dispatch
            // evidence, so the caller is told to sign in again rather than that
            // the change went through.
            return Ok(DormPasswordWriteOutcome::LoginRequired);
        }
        let cross_origin = !same_origin(&self.base_url, &response.final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                resolve_location(&response.final_url, location)
                    .is_ok_and(|target| !same_origin(&self.base_url, &target))
            });
        if cross_origin {
            return Err(DormPasswordWriteAdapterError::UnexpectedOrigin);
        }
        let redirect_outside_mapping = redirect_location
            .as_deref()
            .and_then(|location| resolve_location(&response.final_url, location).ok())
            .is_some_and(|location| {
                same_origin(&self.base_url, &location)
                    && !path_within_base(&self.base_url, &location)
            });
        if redirect_location.is_some()
            || !status.is_success()
            || response.final_url.path() != expected_path
            || response.final_url.query().is_some()
            || redirect_outside_mapping
            || !path_within_base(&self.base_url, &response.final_url)
        {
            // A redirect this module did not follow, a page for another route, or
            // a route outside the mapping: the request has already left, so the
            // effect is unknown rather than failed.  Never re-dispatched.
            return Ok(DormPasswordWriteOutcome::Unrecognized);
        }
        Ok(classify_dorm_password_write(&response.body))
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, DormPasswordWriteAdapterError> {
        if !valid_relative_path(relative_path) {
            return Err(DormPasswordWriteAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{relative_path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: DormPasswordParseError) -> DormPasswordWriteAdapterError {
        match error {
            DormPasswordParseError::LoginPage | DormPasswordParseError::ExpiredPage => {
                DormPasswordWriteAdapterError::SessionExpired
            }
            DormPasswordParseError::FormMissing => DormPasswordWriteAdapterError::FormMissing,
            DormPasswordParseError::UnexpectedHtml
            | DormPasswordParseError::TooLarge
            | DormPasswordParseError::EmptyBody => {
                DormPasswordWriteAdapterError::UnexpectedDeployment
            }
        }
    }
}

impl fmt::Debug for DormPasswordWriteAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DormPasswordWriteAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Reports whether a form response could be an HTML document at all.
///
/// This is deliberately weaker than [`looks_like_html`]: a legacy ASP.NET page
/// may open with a directive, a comment or a doctype this module has not seen, so
/// what is checked here is only that the body is markup rather than a JSON body.
/// The readiness anchor is what actually identifies the form.
fn looks_like_html_body(body: &str) -> bool {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    !trimmed.starts_with('{') && !trimmed.starts_with('[') && trimmed.contains('<')
}

/// Reports whether an answer is an HTML document.
///
/// Used by the write classifier, where a page is never acceptance evidence, so a
/// strict opening-tag test is the right side to err on.
fn looks_like_html(body: &str) -> bool {
    let lower = body
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.starts_with("<head")
        || lower.starts_with("<body")
        || lower.starts_with("<form")
        || lower.starts_with("<table")
        || lower.starts_with("<span")
}

fn is_html_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
    })
}

fn looks_like_login_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path == "/login"
        || path.ends_with("/login")
        || path.contains("/login/")
        || path.contains("/do/off/ui/auth/login")
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn path_within_base(base_url: &Url, candidate: &Url) -> bool {
    let base_path = base_url.path().trim_end_matches('/');
    base_path.is_empty()
        || base_path == "/"
        || candidate.path() == base_path
        || candidate.path().starts_with(&format!("{base_path}/"))
}

fn normalize_base_url(mut base_url: Url) -> Result<Url, DormPasswordWriteAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(DormPasswordWriteAdapterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = opaque_mapping_root(path)
        .or_else(|| strip_known_service_suffix(path))
        .unwrap_or_else(|| {
            if path.is_empty() {
                "/".to_owned()
            } else {
                format!("{path}/")
            }
        });
    base_url.set_path(&path);
    Ok(base_url)
}

/// Narrows a WebVPN mapped root to its opaque mapping directory.
///
/// The electricity read half hands this adapter the mapping root it proved, which
/// may still carry the read route; the root is what an endpoint is built from, so
/// anything after the token is dropped here as well.
fn opaque_mapping_root(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let scheme = segments.next()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let token = segments.next()?;
    Some(format!("/{scheme}/{token}/"))
}

fn strip_known_service_suffix(path: &str) -> Option<String> {
    [DORM_CHANGE_PASSWORD_PATH].into_iter().find_map(|suffix| {
        let prefix = path.strip_suffix(suffix)?;
        if prefix.is_empty() {
            Some("/".to_owned())
        } else {
            Some(format!("{}/", prefix.trim_end_matches('/')))
        }
    })
}

fn resolve_location(
    response_url: &Url,
    location: &str,
) -> Result<Url, DormPasswordWriteAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(DormPasswordWriteAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| DormPasswordWriteAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(DormPasswordWriteAdapterError::UnexpectedOrigin);
    }
    Ok(target)
}

fn safe_base_path(path: &str) -> bool {
    !path.contains(['\\', '?', '#'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

fn valid_relative_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.contains("://")
        && !path.contains(['?', '#', '\\'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
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
    let lower = path.to_ascii_lowercase();
    lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c") || lower.contains("%25")
}
