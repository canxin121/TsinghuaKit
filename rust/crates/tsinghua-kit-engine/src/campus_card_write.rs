//! Account-bound campus-card state changes: loss reporting and its reversal,
//! the transaction-password change, the spending-limit change, and the one
//! top-up entry that needs no payment link.
//!
//! Everything here shares the card service origin, the shared Cookie transport
//! and the account binding the read side already proves, so no operation opens a
//! second session or a second login.  The templates the profiles build are
//! transport-neutral: the session-bound `idserial`, the physical `cardid` and
//! every password are inserted only inside the adapter, after the card service
//! has proved the same account.
//!
//! Four rules shape the module:
//!
//! * **A write leaves exactly once.**  Every plan is dispatched through
//!   `CampusHttpTransport::execute_once_exclusive`, which takes the whole
//!   request gate and returns the first response without following a redirect.
//!   A write whose answer is not the service's own confirmation is reported as
//!   unconfirmed rather than failed, and nothing is ever re-sent: a second send
//!   of a request whose effect is unknown is a replay of it.
//! * **A password never leaves this module.**  The transaction password and a
//!   new account password are held in [`CampusCardSecret`], which zeroizes on
//!   drop and whose `Debug` prints presence only.  A secret is copied straight
//!   into the one body it belongs to and is never stored in a plan, never
//!   returned to a caller, and never written to a log.
//! * **The billable entries are the ones this module can honestly finish.**  A
//!   card top-up has two observed forms.  The bank form moves money from the
//!   account's own bound bank account and answers `returncode`; it needs no
//!   payment link, so [`CARD_BANK_TOPUP_PATH`] is implemented.
//!   [`CARD_QR_TOPUP_PATH`] is **not**: its only successful answer is a payment
//!   URL, which is a bearer token for one payment, and a pay code must not enter
//!   a DTO or a log — so a caller could not be handed the one thing the route
//!   produces.  It is recorded as a boundary constant instead of being
//!   implemented into a dead end.  See the constant's own documentation for the
//!   second, independent reason.
//! * **The limit fields keep the wire's own names.**  The two wire fields this
//!   service uses for spending limits disagree with each other between the read
//!   and write halves of the observed client, so this module names them after the
//!   wire rather than guessing which one is the daily limit.  See
//!   [`CampusCardWriteProfile::modify_limit_request`].
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or constants beyond the observed wire names.

use std::fmt;

use serde_json::{Map, Value, json};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

/// Reports the card as lost, so the service blocks it.
pub const CARD_REPORT_LOSS_PATH: &str = "/business/cardReportLoss";

/// Reverses a loss report.
pub const CARD_CANCEL_LOSS_PATH: &str = "/business/solutionHang";

/// Changes the transaction password.
///
/// The route's own name says it verifies by phone; the observed request sends no
/// phone number and no verification code, only the old password plus
/// `authOldPwd: true`.  This module sends exactly that and nothing more: a
/// verification code it was never observed to carry would be invented.
pub const CARD_CHANGE_PASSWORD_PATH: &str = "/business/modifyPwdByPhoneVerify";

/// Changes the card's spending limits.
pub const CARD_MODIFY_LIMIT_PATH: &str = "/business/modifyCardMaxConsamt";

/// The bank top-up form, which moves money from the account's own bound bank
/// account onto the card and keeps the typo the service itself uses.
pub const CARD_BANK_TOPUP_PATH: &str = "/business/moblieRecharge";

/// The QR-code top-up entry, recorded and **never requested**.
///
/// Two independent observations keep this route out of the module:
///
/// * Its only successful answer is a payment URL inside the service's own JSON
///   (`bizContent.webUrl`), which is a single-use bearer token for one payment.
///   A pay code must not enter a DTO, a log or a ledger, and there is no other
///   thing this route produces, so implementing it would hand a caller a call
///   that cannot be finished.
/// * The observed client refuses both top-up forms until an app-specific backend
///   reports a card API version at or below 2.  That backend is
///   `app.cs.tsinghua.edu.cn`, which this repository deliberately does not
///   implement, so "top-up is available for this account" has no school-side
///   evidence either.  Without that answer a QR plan could be built for an
///   account the school would refuse, and the refusal would arrive as a payment
///   attempt rather than as a refusal.
///
/// The path is recorded so a reader can see which route was left out on purpose
/// rather than merely forgotten, and it answers no request of its own.
pub const CARD_QR_TOPUP_PATH: &str = "/wx/rechard/qrcode";

/// The longest password this module will put into a card body.
pub const MAX_CARD_SECRET_CHARS: usize = 64;

/// The largest spending limit this module will send, in fen.
///
/// The service's own bound is not observed: the read half reports the account's
/// current limits but never states a range, and no reference validates one.  This
/// is therefore this module's own local bound, chosen so a mistyped amount is
/// refused locally instead of sent as a limit nobody could mean.
pub const MAX_CARD_LIMIT_CENTS: i64 = 100_000_000;

/// The smallest top-up the reference client's own input rule accepts, in fen.
///
/// The rule is the observed one: an amount with at most two decimals and no
/// leading zeroes, between 10 and 200 yuan inclusive.  It is the caller-facing
/// bound rather than a service bound, and this module applies it so a bank
/// transfer is never sent for an amount no observed client would send.
pub const MIN_CARD_TOPUP_CENTS: i64 = 1_000;

/// The largest top-up the reference client's own input rule accepts, in fen.
pub const MAX_CARD_TOPUP_CENTS: i64 = 20_000;

/// The only HTTP method used by the observed card write endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CampusCardWriteMethod {
    Post,
}

/// Authentication is a capability requirement, not a credential.
///
/// The adapter must reuse the identity-to-card SSO the read side already
/// established on the shared `CampusHttpTransport`, and must prove the same card
/// account, before executing a write.  No Cookie, ticket or SSO payload is
/// represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampusCardWriteSessionPrerequisite {
    ExistingIdentityAndCardSso,
}

/// Name of a card state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampusCardWriteOperation {
    ReportLoss,
    CancelLoss,
    ChangeTransactionPassword,
    ModifySpendingLimit,
    TopUpFromBank,
}

impl CampusCardWriteOperation {
    /// Returns the service path without an origin, cookie, or query string.
    pub const fn path(self) -> &'static str {
        match self {
            Self::ReportLoss => CARD_REPORT_LOSS_PATH,
            Self::CancelLoss => CARD_CANCEL_LOSS_PATH,
            Self::ChangeTransactionPassword => CARD_CHANGE_PASSWORD_PATH,
            Self::ModifySpendingLimit => CARD_MODIFY_LIMIT_PATH,
            Self::TopUpFromBank => CARD_BANK_TOPUP_PATH,
        }
    }

    /// Returns whether this operation changes the account's own state.
    ///
    /// Every operation in this module does; the method exists so a caller can
    /// state that fact rather than assume it, and so the one shape that is a
    /// genuine read on this surface stays in the read module.
    pub const fn is_write(self) -> bool {
        true
    }

    /// Returns the stable, lowercase identifier used in telemetry and errors.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReportLoss => "card_report_loss",
            Self::CancelLoss => "card_cancel_loss",
            Self::ChangeTransactionPassword => "card_change_password",
            Self::ModifySpendingLimit => "card_modify_limit",
            Self::TopUpFromBank => "card_bank_topup",
        }
    }
}

impl fmt::Display for CampusCardWriteOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One short secret carried by a card write.
///
/// A transaction password and a new account password are the same kind of value
/// here: short, account-bound, and not something this module may retain.  The
/// wrapper exists so both are validated once, zeroized on drop, and impossible to
/// print.
pub struct CampusCardSecret(Zeroizing<String>);

impl CampusCardSecret {
    /// Validates one secret.
    ///
    /// The value is **not** trimmed: a password may legitimately contain a space,
    /// and silently changing what a caller typed would mean sending a different
    /// secret than the one they entered.  What is refused is a value that is
    /// empty, longer than [`MAX_CARD_SECRET_CHARS`], made only of whitespace, or
    /// carrying a control character that could not survive being a JSON string
    /// field.
    pub fn new(value: impl Into<String>) -> Result<Self, CampusCardWriteRequestError> {
        let value = value.into();
        if value.is_empty() {
            return Err(CampusCardWriteRequestError::EmptySecret);
        }
        if value.chars().count() > MAX_CARD_SECRET_CHARS {
            return Err(CampusCardWriteRequestError::SecretTooLong {
                max: MAX_CARD_SECRET_CHARS,
            });
        }
        if value.trim().is_empty() {
            return Err(CampusCardWriteRequestError::EmptySecret);
        }
        if value.chars().any(char::is_control) {
            return Err(CampusCardWriteRequestError::SecretInvalid);
        }
        Ok(Self(Zeroizing::new(value)))
    }

    /// Returns the secret for the one body it belongs to.
    ///
    /// Crate-visible on purpose: the value may only reach a request this crate is
    /// about to dispatch, never a DTO, a plan field or an error.
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for CampusCardSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardSecret")
            .field("chars", &self.0.chars().count())
            .finish_non_exhaustive()
    }
}

impl Drop for CampusCardSecret {
    fn drop(&mut self) {
        // `Zeroizing` already clears on drop; this is explicit so the guarantee
        // survives someone later replacing the inner type.
        self.0.zeroize();
    }
}

/// Errors raised while building a card write plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CampusCardWriteRequestError {
    #[error("the campus card write secret is empty")]
    EmptySecret,

    #[error("the campus card write secret is longer than {max} characters")]
    SecretTooLong { max: usize },

    #[error("the campus card write secret contains a control character")]
    SecretInvalid,

    #[error("the campus card top-up amount is outside the accepted range")]
    TopUpAmountOutOfRange,

    #[error("the campus card spending limit is negative or above the local bound")]
    LimitOutOfRange,
}

/// The fields a card write carries, with every secret kept private.
///
/// The variant names are the wire's own field names because those are the only
/// names with evidence behind them; see
/// [`CampusCardWriteProfile::modify_limit_request`] for the one place where the
/// two halves of the observed client disagree about what a field means.
pub enum CampusCardWriteBody {
    /// The transaction password, spelled the way this pair of routes spells it.
    TransactionPasswordWithTxpasswd(CampusCardSecret),
    /// The old password and its replacement, plus `authOldPwd: true`.
    TransactionPasswordChange {
        old: CampusCardSecret,
        new: CampusCardSecret,
    },
    /// The transaction password plus the two limit fields, in fen.
    SpendingLimit {
        password: CampusCardSecret,
        maxconsamt_cents: i64,
        maxconstolamt_cents: i64,
    },
    /// The top-up amount in fen.  No password is sent: the observed request
    /// accepts a transaction password argument and never puts it on the wire, so
    /// this module does not offer one either.
    BankTopUp { amount_cents: i64 },
}

impl fmt::Debug for CampusCardWriteBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransactionPasswordWithTxpasswd(_) => formatter
                .debug_tuple("TransactionPasswordWithTxpasswd")
                .field(&"[redacted]")
                .finish(),
            Self::TransactionPasswordChange { .. } => formatter
                .debug_struct("TransactionPasswordChange")
                .field("old", &"[redacted]")
                .field("new", &"[redacted]")
                .finish(),
            Self::SpendingLimit {
                maxconsamt_cents,
                maxconstolamt_cents,
                ..
            } => formatter
                .debug_struct("SpendingLimit")
                .field("password", &"[redacted]")
                .field("maxconsamt_cents", maxconsamt_cents)
                .field("maxconstolamt_cents", maxconstolamt_cents)
                .finish(),
            Self::BankTopUp { amount_cents } => formatter
                .debug_struct("BankTopUp")
                .field("amount_cents", amount_cents)
                .finish(),
        }
    }
}

/// A typed card write plan.
///
/// The plan is the safe part of a request: the operation, the method, and the
/// fields this module has already validated.  The session-bound `idserial`, the
/// physical `cardid` and the transport's origin and Cookie jar are added by the
/// adapter at the moment of dispatch.
///
/// `Debug` prints the operation and the safe field names rather than the body, so
/// a plan can be logged without a password reaching the log.
///
/// The type is deliberately **not** `PartialEq` and not `Clone`.  Equality over a
/// plan would either compare secrets, which would make a password comparable
/// outside this module, or ignore them, which would report two different requests
/// as the same one.  There is no honest middle, so there is no equality: a caller
/// who wants to check which request shape it built reads
/// [`CampusCardWritePlan::operation`] and [`CampusCardWritePlan::body_fields`],
/// and a caller who wants to send the same change twice builds a second plan.
pub struct CampusCardWritePlan {
    operation: CampusCardWriteOperation,
    method: CampusCardWriteMethod,
    prerequisite: CampusCardWriteSessionPrerequisite,
    body: CampusCardWriteBody,
}

impl CampusCardWritePlan {
    pub const fn operation(&self) -> CampusCardWriteOperation {
        self.operation
    }

    pub const fn method(&self) -> CampusCardWriteMethod {
        self.method
    }

    pub const fn session_prerequisite(&self) -> CampusCardWriteSessionPrerequisite {
        self.prerequisite
    }

    /// Returns the service path without an origin or a query string.
    pub const fn path(&self) -> &'static str {
        self.operation.path()
    }

    /// Returns the observed wire field names this plan's body carries, with no
    /// values.
    ///
    /// The identifiers and the passwords are deliberately absent: `idserial` and
    /// `cardid` are session-bound and are inserted by the adapter, and a password
    /// is a secret.  What is left is exactly the set of field names a reader needs
    /// to see which route shape was built.
    pub fn body_fields(&self) -> Vec<&'static str> {
        match &self.body {
            CampusCardWriteBody::TransactionPasswordWithTxpasswd(_) => {
                vec!["idserial", "txpasswd"]
            }
            CampusCardWriteBody::TransactionPasswordChange { .. } => {
                vec!["idserial", "oldpassword", "txpassword", "authOldPwd"]
            }
            CampusCardWriteBody::SpendingLimit { .. } => {
                vec!["maxconsamt", "maxconstolamt", "txpassword", "cardid"]
            }
            CampusCardWriteBody::BankTopUp { .. } => vec!["idserial", "txamt"],
        }
    }

    /// Returns the plan's non-secret amounts, in fen.
    ///
    /// Only the two operations that carry an amount return anything, and the
    /// values are amounts rather than credentials: the caller supplied them and a
    /// confirmation prompt needs to show them back.
    pub fn amounts_cents(&self) -> Vec<(&'static str, i64)> {
        match &self.body {
            CampusCardWriteBody::SpendingLimit {
                maxconsamt_cents,
                maxconstolamt_cents,
                ..
            } => vec![
                ("maxconsamt", *maxconsamt_cents),
                ("maxconstolamt", *maxconstolamt_cents),
            ],
            CampusCardWriteBody::BankTopUp { amount_cents } => vec![("txamt", *amount_cents)],
            _ => Vec::new(),
        }
    }

    /// Builds the JSON body the adapter will send, with the session-bound
    /// `idserial` and `cardid` supplied by the adapter itself.
    ///
    /// This is crate-visible because it is the one place a secret becomes a wire
    /// value; a caller outside this crate may see the plan but not this.
    pub(crate) fn body_json(&self, idserial: &str, card_id: Option<&str>) -> Value {
        match &self.body {
            CampusCardWriteBody::TransactionPasswordWithTxpasswd(password) => json!({
                "idserial": idserial,
                "txpasswd": password.expose(),
            }),
            CampusCardWriteBody::TransactionPasswordChange { old, new } => json!({
                "idserial": idserial,
                "oldpassword": old.expose(),
                "txpassword": new.expose(),
                "authOldPwd": true,
            }),
            CampusCardWriteBody::SpendingLimit {
                password,
                maxconsamt_cents,
                maxconstolamt_cents,
            } => json!({
                "maxconsamt": maxconsamt_cents,
                "maxconstolamt": maxconstolamt_cents,
                "txpassword": password.expose(),
                "cardid": card_id.unwrap_or_default(),
            }),
            CampusCardWriteBody::BankTopUp { amount_cents } => json!({
                "idserial": idserial,
                "txamt": amount_cents,
            }),
        }
    }

    /// Reports whether this plan needs the account's physical card identifier.
    ///
    /// Only the spending-limit route does, and the identifier is obtained by the
    /// adapter from a fresh account read rather than accepted from a caller, so it
    /// never crosses this boundary as a value.
    pub(crate) const fn needs_card_id(&self) -> bool {
        matches!(self.body, CampusCardWriteBody::SpendingLimit { .. })
    }
}

impl fmt::Debug for CampusCardWritePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardWritePlan")
            .field("operation", &self.operation.as_str())
            .field("method", &self.method)
            .field("prerequisite", &self.prerequisite)
            .field("has_relative_path", &self.path().starts_with('/'))
            .field("body_fields", &self.body_fields())
            .finish()
    }
}

/// Builds the module's write plans from validated inputs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CampusCardWriteProfile;

impl CampusCardWriteProfile {
    pub const fn new() -> Self {
        Self
    }

    fn plan(operation: CampusCardWriteOperation, body: CampusCardWriteBody) -> CampusCardWritePlan {
        CampusCardWritePlan {
            operation,
            method: CampusCardWriteMethod::Post,
            prerequisite: CampusCardWriteSessionPrerequisite::ExistingIdentityAndCardSso,
            body,
        }
    }

    /// Reports the card as lost.
    ///
    /// The service requires the transaction password for this, so the request
    /// carries a secret; the read side's account record deliberately holds none.
    pub fn report_loss_request(
        &self,
        transaction_password: CampusCardSecret,
    ) -> CampusCardWritePlan {
        Self::plan(
            CampusCardWriteOperation::ReportLoss,
            CampusCardWriteBody::TransactionPasswordWithTxpasswd(transaction_password),
        )
    }

    /// Reverses a loss report.
    ///
    /// This is the one operation whose success makes a lost card spendable again,
    /// so its plan is built from nothing but the caller's transaction password:
    /// there is no cached "this card is blocked" state in this module that could
    /// make the reversal look routine.
    pub fn cancel_loss_request(
        &self,
        transaction_password: CampusCardSecret,
    ) -> CampusCardWritePlan {
        Self::plan(
            CampusCardWriteOperation::CancelLoss,
            CampusCardWriteBody::TransactionPasswordWithTxpasswd(transaction_password),
        )
    }

    /// Replaces the transaction password.
    pub fn change_password_request(
        &self,
        old_password: CampusCardSecret,
        new_password: CampusCardSecret,
    ) -> CampusCardWritePlan {
        Self::plan(
            CampusCardWriteOperation::ChangeTransactionPassword,
            CampusCardWriteBody::TransactionPasswordChange {
                old: old_password,
                new: new_password,
            },
        )
    }

    /// Changes the two spending limits.
    ///
    /// **The two fields keep the wire's own names, and the names do not agree with
    /// each other.**  The account read maps `maxconstolamt` to the daily limit and
    /// `maxconsamt` to the per-transaction limit, while the observed write puts its
    /// parameter named `maxDailyTranscationAmount` into `maxconsamt` and its
    /// parameter named `maxOneTimeTranscationAmount` into `maxconstolamt` — the
    /// opposite pairing.  Exactly one of the two call sites is transposed, and
    /// there is no observation here that says which, so this module refuses to
    /// guess: the arguments are named after the fields they fill and the caller
    /// decides what those fields mean for their own account.  Naming them "daily"
    /// and "one-time" would bake one of the two readings into the API and send a
    /// limit the caller did not choose whenever the other reading is the true one.
    ///
    /// The two amounts are independent: no observed rule relates them, so this
    /// module imposes none.  Both are bounded by [`MAX_CARD_LIMIT_CENTS`], which is
    /// this module's own local bound rather than an observed service bound.
    pub fn modify_limit_request(
        &self,
        transaction_password: CampusCardSecret,
        maxconsamt_cents: i64,
        maxconstolamt_cents: i64,
    ) -> Result<CampusCardWritePlan, CampusCardWriteRequestError> {
        for amount in [maxconsamt_cents, maxconstolamt_cents] {
            if !(0..=MAX_CARD_LIMIT_CENTS).contains(&amount) {
                return Err(CampusCardWriteRequestError::LimitOutOfRange);
            }
        }
        Ok(Self::plan(
            CampusCardWriteOperation::ModifySpendingLimit,
            CampusCardWriteBody::SpendingLimit {
                password: transaction_password,
                maxconsamt_cents,
                maxconstolamt_cents,
            },
        ))
    }

    /// Moves money from the account's own bound bank account onto the card.
    ///
    /// The amount is bounded by the reference client's own input rule rather than
    /// by the service: no service-side range is observed, and a bank transfer is
    /// not something to discover a bound for by trying amounts.
    pub fn bank_topup_request(
        &self,
        amount_cents: i64,
    ) -> Result<CampusCardWritePlan, CampusCardWriteRequestError> {
        if !(MIN_CARD_TOPUP_CENTS..=MAX_CARD_TOPUP_CENTS).contains(&amount_cents) {
            return Err(CampusCardWriteRequestError::TopUpAmountOutOfRange);
        }
        Ok(Self::plan(
            CampusCardWriteOperation::TopUpFromBank,
            CampusCardWriteBody::BankTopUp { amount_cents },
        ))
    }
}

/// The service's own answer to a card state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampusCardWriteOutcome {
    /// The service confirmed the change.
    Accepted,
    /// The service answered and refused.
    Refused,
    /// The session was gone, so nothing was applied.
    LoginRequired,
    /// The request left but its answer is not one this module can read.  The
    /// effect is unknown, and it is never sent a second time.
    Unrecognized,
}

/// Reads a refusal out of the service's own successful envelope.
///
/// The card service answers a refused state change in one of two ways: with a
/// failure envelope, which the adapter already decodes and reports separately, or
/// with a successful envelope whose payload carries the service's own
/// `returncode`.  The bank route's `returncode` is the observed instance — the
/// client throws when it reads `ERROR` there — and this reads the same field on
/// every route of this module because it is the same service's own field name on
/// the same origin.  Only the literal `ERROR` is a refusal: any other value,
/// including a missing one, is not refusal evidence and leaves the answer to the
/// envelope's own success flag.
pub(crate) fn result_data_reports_refusal(result_data: &Value) -> bool {
    let Value::Object(object) = result_data else {
        return false;
    };
    return_code_is_error(object.get("returncode"))
}

fn return_code_is_error(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => text.trim().eq_ignore_ascii_case("error"),
        _ => false,
    }
}

/// Reports whether a decoded payload is a JSON object at all.
///
/// A route that answered a successful envelope with a scalar has not confirmed a
/// state change, so the answer is unreadable rather than accepted.
pub(crate) fn result_data_is_readable(result_data: &Value) -> bool {
    matches!(result_data, Value::Object(_)) || matches!(result_data, Value::Null)
}

/// Reports whether a decoded payload carries an explicit failure marker of its
/// own, beyond `returncode`.
///
/// The same conventional `message`/`msg`/`error` field names the socket writer
/// already treats as failure evidence are used here, because these are the same
/// service's envelopes.  A falsey `success`/`result` is read the same way.
pub(crate) fn result_data_has_failure_marker(result_data: &Value) -> bool {
    let Value::Object(object) = result_data else {
        return false;
    };
    has_failure_marker(object)
}

fn has_failure_marker(object: &Map<String, Value>) -> bool {
    if object.get("success").and_then(Value::as_bool) == Some(false)
        || object.get("result").and_then(Value::as_bool) == Some(false)
    {
        return true;
    }
    ["message", "msg", "error", "errorMessage"]
        .iter()
        .filter_map(|field| object.get(*field))
        .filter_map(Value::as_str)
        .any(|text| {
            let text = text.trim().to_ascii_lowercase();
            matches!(
                text.as_str(),
                "error" | "fail" | "failed" | "failure" | "错误" | "失败" | "拒绝"
            ) || text.starts_with("error ")
                || text.starts_with("fail")
                || text.starts_with("失败")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(value: &str) -> CampusCardSecret {
        CampusCardSecret::new(value).expect("test secret")
    }

    #[test]
    fn secrets_reject_empty_whitespace_control_and_overlong_values() {
        assert_eq!(
            CampusCardSecret::new("").unwrap_err(),
            CampusCardWriteRequestError::EmptySecret
        );
        assert_eq!(
            CampusCardSecret::new("    ").unwrap_err(),
            CampusCardWriteRequestError::EmptySecret
        );
        assert_eq!(
            CampusCardSecret::new("12\n34").unwrap_err(),
            CampusCardWriteRequestError::SecretInvalid
        );
        assert_eq!(
            CampusCardSecret::new("9".repeat(MAX_CARD_SECRET_CHARS + 1)).unwrap_err(),
            CampusCardWriteRequestError::SecretTooLong {
                max: MAX_CARD_SECRET_CHARS
            }
        );
        // A password may contain a space, and it is kept exactly as given.
        let kept = secret("12 34");
        assert_eq!(kept.expose(), "12 34");
    }

    #[test]
    fn secret_debug_prints_presence_and_length_but_never_the_value() {
        let value = secret("987654");
        let debug = format!("{value:?}");
        assert!(debug.contains("CampusCardSecret"));
        assert!(debug.contains("chars: 6"));
        assert!(!debug.contains("987654"));
    }

    #[test]
    fn plan_debug_prints_wire_field_names_and_never_a_secret() {
        let plan = CampusCardWriteProfile::new().report_loss_request(secret("135790"));
        let debug = format!("{plan:?}");
        // The debug output names the operation the way every other surface does,
        // so a plan read back from a log line can be matched to telemetry.
        assert!(debug.contains("card_report_loss"));
        assert!(debug.contains("txpasswd"));
        assert!(debug.contains("has_relative_path: true"));
        assert!(!debug.contains("135790"));
        assert_eq!(plan.path(), CARD_REPORT_LOSS_PATH);
        assert!(plan.operation().is_write());
    }

    #[test]
    fn every_plan_path_is_the_operation_route_and_carries_no_query() {
        let profile = CampusCardWriteProfile::new();
        for (operation, plan) in [
            (
                CampusCardWriteOperation::ReportLoss,
                profile.report_loss_request(secret("111111")),
            ),
            (
                CampusCardWriteOperation::CancelLoss,
                profile.cancel_loss_request(secret("111111")),
            ),
            (
                CampusCardWriteOperation::ChangeTransactionPassword,
                profile.change_password_request(secret("111111"), secret("222222")),
            ),
            (
                CampusCardWriteOperation::ModifySpendingLimit,
                profile
                    .modify_limit_request(secret("111111"), 20_000, 5_000)
                    .unwrap(),
            ),
            (
                CampusCardWriteOperation::TopUpFromBank,
                profile.bank_topup_request(12_000).unwrap(),
            ),
        ] {
            assert_eq!(plan.operation(), operation);
            assert_eq!(plan.path(), operation.path());
            assert!(plan.path().starts_with('/'));
            assert!(!plan.path().contains(['?', '#', ':']));
            assert_eq!(plan.method(), CampusCardWriteMethod::Post);
        }
    }

    #[test]
    fn the_qr_topup_route_is_recorded_but_is_not_an_operation() {
        // The path exists as documentation; no operation may return it, so a
        // caller cannot build a request that could only answer a pay code.
        assert_eq!(CARD_QR_TOPUP_PATH, "/wx/rechard/qrcode");
        for operation in [
            CampusCardWriteOperation::ReportLoss,
            CampusCardWriteOperation::CancelLoss,
            CampusCardWriteOperation::ChangeTransactionPassword,
            CampusCardWriteOperation::ModifySpendingLimit,
            CampusCardWriteOperation::TopUpFromBank,
        ] {
            assert_ne!(operation.path(), CARD_QR_TOPUP_PATH);
        }
    }

    #[test]
    fn the_limit_plan_sends_the_wire_fields_in_the_observed_pairing() {
        // The write half of the observed client fills `maxconsamt` from its
        // "daily" parameter and `maxconstolamt` from its "one-time" parameter,
        // while its read half pairs the same fields the other way round.  This
        // module names its arguments after the fields, so the assertion here is
        // about the wire and not about a meaning.
        let plan = CampusCardWriteProfile::new()
            .modify_limit_request(secret("111111"), 20_000, 5_000)
            .unwrap();
        assert_eq!(
            plan.amounts_cents(),
            vec![("maxconsamt", 20_000), ("maxconstolamt", 5_000)]
        );
        let body = plan.body_json("serial-1", Some("card-1"));
        assert_eq!(body["maxconsamt"], json!(20_000));
        assert_eq!(body["maxconstolamt"], json!(5_000));
        assert_eq!(body["cardid"], json!("card-1"));
        assert_eq!(body["txpassword"], json!("111111"));
        assert!(plan.needs_card_id());
    }

    #[test]
    fn the_loss_pair_sends_txpasswd_and_the_change_pair_sends_auth_old_pwd() {
        let profile = CampusCardWriteProfile::new();
        let loss = profile
            .report_loss_request(secret("111111"))
            .body_json("serial-1", None);
        assert_eq!(loss, json!({"idserial": "serial-1", "txpasswd": "111111"}));

        let change = profile
            .change_password_request(secret("111111"), secret("222222"))
            .body_json("serial-1", None);
        assert_eq!(
            change,
            json!({
                "idserial": "serial-1",
                "oldpassword": "111111",
                "txpassword": "222222",
                "authOldPwd": true,
            })
        );
    }

    #[test]
    fn the_bank_topup_body_carries_no_password_and_its_own_field_name() {
        let plan = CampusCardWriteProfile::new()
            .bank_topup_request(12_345)
            .unwrap();
        assert_eq!(plan.body_fields(), vec!["idserial", "txamt"]);
        assert_eq!(
            plan.body_json("serial-1", None),
            json!({"idserial": "serial-1", "txamt": 12_345})
        );
        assert!(!plan.needs_card_id());
    }

    #[test]
    fn amounts_outside_the_observed_or_local_bounds_are_refused_before_a_plan() {
        let profile = CampusCardWriteProfile::new();
        for amount in [0, 999, 20_001, -1, i64::MAX] {
            assert_eq!(
                profile.bank_topup_request(amount).unwrap_err(),
                CampusCardWriteRequestError::TopUpAmountOutOfRange
            );
        }
        for (daily, one_time) in [
            (-1, 1_000),
            (1_000, -1),
            (MAX_CARD_LIMIT_CENTS + 1, 1_000),
            (1_000, MAX_CARD_LIMIT_CENTS + 1),
        ] {
            assert_eq!(
                profile
                    .modify_limit_request(secret("111111"), daily, one_time)
                    .unwrap_err(),
                CampusCardWriteRequestError::LimitOutOfRange
            );
        }
        // The local bound itself and a zero limit are both accepted: a zero is a
        // limit a holder may legitimately set, not an invalid number.
        assert!(profile.modify_limit_request(secret("111111"), 0, 0).is_ok());
        assert!(
            profile
                .modify_limit_request(secret("111111"), MAX_CARD_LIMIT_CENTS, 0)
                .is_ok()
        );
    }

    #[test]
    fn only_the_service_own_error_token_is_a_refusal() {
        assert!(result_data_reports_refusal(&json!({"returncode": "ERROR"})));
        assert!(result_data_reports_refusal(&json!({"returncode": "error"})));
        assert!(!result_data_reports_refusal(
            &json!({"returncode": "SUCCESS"})
        ));
        assert!(!result_data_reports_refusal(&json!({"returncode": 1})));
        assert!(!result_data_reports_refusal(&json!({})));
        assert!(!result_data_reports_refusal(&json!("ERROR")));
    }

    #[test]
    fn explicit_payload_failure_markers_are_recognised() {
        assert!(result_data_has_failure_marker(&json!({"success": false})));
        assert!(result_data_has_failure_marker(&json!({"result": false})));
        assert!(result_data_has_failure_marker(&json!({"message": "失败"})));
        assert!(result_data_has_failure_marker(&json!({"error": "failed"})));
        assert!(!result_data_has_failure_marker(&json!({"success": true})));
        assert!(!result_data_has_failure_marker(&json!({"message": "成功"})));
        assert!(result_data_is_readable(&json!({})));
        assert!(result_data_is_readable(&Value::Null));
        assert!(!result_data_is_readable(&json!(1)));
    }
}
