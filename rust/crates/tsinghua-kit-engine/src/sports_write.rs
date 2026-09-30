//! Sports-venue writes: ordering, unsubscription, the contact-number update,
//! and the captcha image the order route requires.
//!
//! The venue read half lives in `sports_read`; everything here changes the
//! account's own booking state, so it shares one rule with the other write
//! slices: **the request is built and dispatched exactly once**.  Every plan
//! goes through `CampusHttpTransport::execute_once_exclusive`, which takes the
//! whole request gate and returns the first response without following a
//! redirect, and an answer the service did not confirm is reported as
//! unconfirmed rather than failed.  Nothing is ever re-dispatched, and the
//! runtime seam does not re-authenticate and retry: a second send of a state
//! change whose effect is unknown is a replay of it.
//!
//! Four behaviours shape the module:
//!
//! * **The captcha is a human's, always.**  The order route carries an image
//!   challenge, and the observed client only enables its submit control once a
//!   person has typed a value into it.  This module therefore accepts the
//!   challenge as a caller-supplied [`SportsCaptchaCode`] and never invents,
//!   guesses, re-reads or retries one; a second order attempt is a second
//!   deliberate human reading, never something a failed call does by itself.
//!   The image itself is reachable through [`SportsWriteAdapter::read_captcha`],
//!   which returns bounded raster bytes for a UI to display.
//! * **The account's identifiers stay inside the request that consumes them.**
//!   The contact number is held in a [`SportsPhone`] whose `Debug` prints its
//!   length and whose storage is zeroized; the account's own login id is derived
//!   by the runtime from the proven identity and never accepted from a caller;
//!   and the one-time booking hash never enters a plan's body until dispatch.
//!   None of them is returned, logged, or recorded in a failure code.
//! * **The payment chain is recorded and never requested.**  Its only output is
//!   a pay code; see [`SPORTS_PAYMENT_MAPPING_TOKEN`].
//! * **A refusal and an unreadable answer are different, and both are terminal
//!   for this call.**  The order route states its own outcome in words (`msg`),
//!   so a worded answer is a definite refusal and its wording is deliberately
//!   not carried out of this module.  The withdrawal and contact-number routes
//!   have **no** observed outcome wording at all, so neither of them can produce
//!   a refusal: an answer this module cannot read is an unknown effect.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::fmt;

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::campus_html::{self, PageClass};
use crate::transport::{CampusHttpTransport, CampusTextResponse, TransportError};

/// The order route.
///
/// The observed constant carries a **nested** `gymbook/gymbook` path and an
/// injected `vpn-12-o1-50.tsinghua.edu.cn=` query parameter, and that parameter
/// is what evidences the venue host: it is the deployment's own marker naming
/// the campus hostname the mapping resolves to.  Both are reproduced as observed
/// rather than "cleaned up", because the deployment is legacy and the route is
/// what it is.
pub const SPORTS_MAKE_ORDER_PATH: &str = "/gymbook/gymbook/gymBookAction.do";

/// The order route's exact query, reproduced verbatim from the observed request.
/// It carries no caller value.
pub const SPORTS_MAKE_ORDER_QUERY: &str = "vpn-12-o1-50.tsinghua.edu.cn=&ms=saveGymBook";

/// The booking application's own page path.  The withdrawal and the
/// contact-number update are two `ms` values on this one route.
pub const SPORTS_BOOK_ACTION_PATH: &str = "/gymbook/gymBookAction.do";

/// The withdrawal route, which shares the page path and is separated from it by
/// its `ms` value.
pub const SPORTS_UNSUBSCRIBE_QUERY: &str = "ms=unsubscribe";

/// The contact-number route's `ms` value and first parameter name.
///
/// The observed client reaches this route as
/// `…/gymbook/gymBookAction.do?ms=doUpdateContactInformation&cell_phone=<number>&gzzh=<login id>`
/// with an **empty body**: every value the route needs travels in the query, and
/// the account's own login id is the last one.  The engine derives that id from
/// the bound account exactly as the course-score read derives its `XH`
/// parameter — it is never a caller argument, a result field, or a log field.
pub const SPORTS_UPDATE_PHONE_QUERY_PREFIX: &str = "ms=doUpdateContactInformation&cell_phone=";

/// The name of the query parameter that carries the account's own login id on
/// the contact-number route.
pub const SPORTS_UPDATE_PHONE_ACCOUNT_PARAM: &str = "gzzh";

/// The image challenge the order route requires.
pub const SPORTS_CAPTCHA_PATH: &str = "/Kaptcha.jpg";

/// The funding-settlement mapping token, recorded and **never requested**.
///
/// The venue's payment chain leaves the venue's own host: after
/// `pay/payAction.do?ms=newPay` (or `ms=newPayForLater`) the observed client
/// posts to `zjjsfw/zjjs/check.do` and then `zjjsfw/zjjs/webPay.do`, both of
/// which sit behind this second WebVPN mapping.
///
/// The chain is recorded rather than implemented for four independent reasons,
/// and the first alone is decisive:
///
/// * **Its only output is a pay code.**  The chain ends in `generalGetPayCode`,
///   which reads `input[name=qrCode]` out of the payment page and returns the
///   last path segment of that value — a single-use bearer token for one
///   payment.  A pay code must not enter a DTO, a log or a ledger, and there is
///   no other thing this route produces, so implementing it would hand a caller
///   a call that cannot be finished.  This is exactly the situation
///   [`crate::campus_card_write::CARD_QR_TOPUP_PATH`] records.  Note also that
///   `ms=newPay` is not a harmless intermediate: it is itself the money
///   movement, so there is no "first half" worth exposing.
/// * **The mapping would be a second registration for a second host.**  It needs
///   its own arm in [`crate::info_session`]'s allowlist, and while that host is
///   evidenced (the fixed WebVPN prefix plus AES-128-CFB of the hostname under
///   the fixed key and IV yields it, and the reference's own host table keys
///   this very token under the same name) the registration would exist only to
///   carry a payment credential.
/// * **The observed client's own step is unreliable.**  It posts to
///   `paymentResultForm.attr()!.action`, and the reference carries its own
///   comment that `attr()` returns `undefined` — the step this engine would have
///   to reproduce is one the reference does not trust.
/// * **The token step depends on redirect method-downgrading.**  The observed
///   transport drops the body and method on a 303 (and on a 301/302 after a
///   POST), which is how the payment token reaches `webPay.do`.  This module's
///   exclusive dispatch never follows a redirect, so the equivalent step would
///   have to be an explicit second GET — a behaviour that would be this module's
///   invention rather than an observation.
///
/// The token is recorded so a reader can see which route was left out on purpose
/// rather than merely forgotten; it answers no request of its own, and a fixture
/// test pins that a plan can never be built for its path.
pub const SPORTS_PAYMENT_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421f6f60c93293c615e7b469dbf915b243daf0f96e17deaf447b4";

/// The payment chain's own host, evidenced by the mapping token above and by the
/// reference's host table.  Recorded only, as with the token.
pub const SPORTS_PAYMENT_HOST: &str = "fa-online.tsinghua.edu.cn";

/// The payment chain's own paths, recorded only.
pub const SPORTS_MAKE_PAYMENT_PATH: &str = "/pay/payAction.do";
pub const SPORTS_PAYMENT_CHECK_PATH: &str = "/zjjsfw/zjjs/check.do";
pub const SPORTS_PAYMENT_ACTION_PATH: &str = "/zjjsfw/zjjs/webPay.do";

/// The venue's own receipt-title values, reproduced from the observed client.
///
/// Only the recorded payment chain consumes a receipt title, so this list and
/// [`receipt_title_is_valid`] document the values a future slice would have to
/// accept instead of taking a caller-supplied string in their place.
pub const VALID_RECEIPT_TITLES: [&str; 3] = ["清华大学", "清华大学工会", "清华大学教育基金会"];

/// The account's own single-payment ceiling, in the service's cost unit.
///
/// This is the **observed application's** own local bound, not a school rule:
/// the reference's own reservation screen refuses to start a payment above 42
/// with the message "出于安全考虑，使用本 APP 发起支付请求时，单笔金额不得超过 42
/// 元。"  This module keeps that bound, because a bound the observed client
/// enforces is evidence, while a bound this module invented would not be.
pub const SPORTS_MAX_SINGLE_PAYMENT_COST: u32 = 42;

/// The most a write's own answer may occupy.
const MAX_WRITE_RESPONSE_BYTES: usize = 64 * 1024;
/// The most an image challenge may occupy.
const MAX_CAPTCHA_BYTES: usize = 256 * 1024;
/// The longest captcha value this module will send.  The challenge is meant for
/// a person to read and retype, so a long value is a caller error rather than a
/// service input.
pub const MAX_SPORTS_CAPTCHA_CHARS: usize = 12;
/// The longest booking hash this module will accept.
pub const MAX_SPORTS_HASH_CHARS: usize = 64;
/// The longest receipt title this module will accept.
pub const MAX_SPORTS_RECEIPT_CHARS: usize = 32;
/// The longest venue, order, or account identifier this module will accept.  It
/// matches the read half's own bound, so a value the reader returned can always
/// be written back.
const MAX_ID_DIGITS: usize = 10;
/// The longest date token this module will accept.
const MAX_DATE_CHARS: usize = 10;

/// The venue's own order-body field names, reproduced verbatim.
///
/// The order form is not a form at all: the observed client sends a flat object
/// whose keys are the ones the deployment's own page uses, including the
/// JavaBean-style `bookData.` prefix.  They are pinned here so a plan's body is
/// exactly the observed set and nothing else.
const ORDER_FIELD_TOTAL_COST: &str = "bookData.totalCost";
const ORDER_FIELD_PERSON_ID: &str = "bookData.book_person_zjh";
const ORDER_FIELD_PERSON_NAME: &str = "bookData.book_person_name";
const ORDER_FIELD_PERSON_PHONE: &str = "bookData.book_person_phone";
const ORDER_FIELD_MODE: &str = "bookData.book_mode";
const ORDER_FIELD_GYM: &str = "gymnasium_idForCache";
const ORDER_FIELD_ITEM: &str = "item_idForCache";
const ORDER_FIELD_DATE: &str = "time_dateForCache";
const ORDER_FIELD_USER_TYPE: &str = "userTypeNumForCache";
const ORDER_FIELD_RESOURCE_KIND: &str = "putongRes";
const ORDER_FIELD_CAPTCHA: &str = "code";
const ORDER_FIELD_PAY_WAY: &str = "selectedPayWay";
const ORDER_FIELD_ALL_FIELD_TIME: &str = "allFieldTime";

/// The fixed values the observed order body carries.
const ORDER_MODE_VALUE: &str = "from-phone";
const ORDER_RESOURCE_KIND_VALUE: &str = "putongRes";
const ORDER_USER_TYPE_VALUE: &str = "1";
const ORDER_PAY_WAY_VALUE: &str = "1";

/// The withdrawal body's only field.
const UNSUBSCRIBE_FIELD_BOOK_ID: &str = "bookId";

/// The one value of the order answer's own `msg` field that means the order was
/// placed.
const ORDER_ACCEPTED_MESSAGE: &str = "预定成功";

/// Every plan here is a form POST sent through the caller's already established
/// INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsWriteMethod {
    Post,
}

/// The write operations this module models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsWriteOperation {
    /// Places one booking, with a captcha a person read.
    MakeOrder,
    /// Withdraws one booking this Runtime's own reservation read returned.
    Unsubscribe,
    /// Sets the contact number the venue will call about a booking.
    UpdatePhone,
}

impl SportsWriteOperation {
    /// The label this operation is recorded under.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MakeOrder => "sports_make_order",
            Self::Unsubscribe => "sports_unsubscribe",
            Self::UpdatePhone => "sports_update_phone",
        }
    }
}

/// A sports-venue write requires an established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsWriteSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// What one dispatched write asked the service to do, with everything reserved
/// for the single request that carries it.
///
/// A plan is not `Clone` and not `PartialEq`: it holds the values the service
/// answered with, and a copy of a plan is a second way to send one request.  Its
/// `Debug` prints field counts and never a value.
pub struct SportsWritePlan {
    operation: SportsWriteOperation,
    method: SportsWriteMethod,
    path: &'static str,
    query: String,
    inner: PlanInner,
}

enum PlanInner {
    /// The order body, with the captcha and the booking hash held until the
    /// request is built.
    Order {
        cost: String,
        phone: SportsPhone,
        gym_id: String,
        item_id: String,
        date: String,
        captcha: SportsCaptchaCode,
        res_hash: String,
    },
    /// The withdrawal, whose only value is the order identifier this Runtime's
    /// own reservation read reported.
    Unsubscribe { book_id: String },
    /// The contact-number update, whose values travel in the query rather than
    /// in a body.
    UpdatePhone {
        phone: SportsPhone,
        account_id: String,
    },
}

impl SportsWritePlan {
    pub fn operation(&self) -> SportsWriteOperation {
        self.operation
    }

    pub fn method(&self) -> SportsWriteMethod {
        self.method
    }

    pub fn path(&self) -> &'static str {
        self.path
    }

    /// The route's query string.  It is deliberately not `Debug`-printed,
    /// because on the contact-number route it carries the account's own login id
    /// and the contact number.
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn session_prerequisite(&self) -> SportsWriteSessionPrerequisite {
        SportsWriteSessionPrerequisite::ExistingInfoWebVpnSession
    }

    /// Returns the body field names this plan would send.
    ///
    /// The captcha and the booking hash are among them; their values are not.
    /// The contact-number route has no body at all.
    pub fn body_fields(&self) -> Vec<&'static str> {
        match &self.inner {
            PlanInner::Order { .. } => vec![
                ORDER_FIELD_TOTAL_COST,
                ORDER_FIELD_PERSON_ID,
                ORDER_FIELD_PERSON_NAME,
                ORDER_FIELD_PERSON_PHONE,
                ORDER_FIELD_MODE,
                ORDER_FIELD_GYM,
                ORDER_FIELD_ITEM,
                ORDER_FIELD_DATE,
                ORDER_FIELD_USER_TYPE,
                ORDER_FIELD_RESOURCE_KIND,
                ORDER_FIELD_CAPTCHA,
                ORDER_FIELD_PAY_WAY,
                ORDER_FIELD_ALL_FIELD_TIME,
            ],
            PlanInner::Unsubscribe { .. } => vec![UNSUBSCRIBE_FIELD_BOOK_ID],
            PlanInner::UpdatePhone { .. } => Vec::new(),
        }
    }

    /// Returns whether this plan is the order operation.
    pub fn is_order(&self) -> bool {
        matches!(self.inner, PlanInner::Order { .. })
    }

    /// Builds the body this plan sends.
    ///
    /// The returned pairs own their values, so the caller drops them as soon as
    /// the request that carries them has been encoded.
    fn body(&self) -> Vec<(&'static str, String)> {
        match &self.inner {
            PlanInner::Order {
                cost,
                phone,
                gym_id,
                item_id,
                date,
                captcha,
                res_hash,
            } => vec![
                (ORDER_FIELD_TOTAL_COST, cost.clone()),
                (ORDER_FIELD_PERSON_ID, String::new()),
                (ORDER_FIELD_PERSON_NAME, String::new()),
                (ORDER_FIELD_PERSON_PHONE, phone.expose().to_owned()),
                (ORDER_FIELD_MODE, ORDER_MODE_VALUE.to_owned()),
                (ORDER_FIELD_GYM, gym_id.clone()),
                (ORDER_FIELD_ITEM, item_id.clone()),
                (ORDER_FIELD_DATE, date.clone()),
                (ORDER_FIELD_USER_TYPE, ORDER_USER_TYPE_VALUE.to_owned()),
                (
                    ORDER_FIELD_RESOURCE_KIND,
                    ORDER_RESOURCE_KIND_VALUE.to_owned(),
                ),
                (ORDER_FIELD_CAPTCHA, captcha.expose().to_owned()),
                (ORDER_FIELD_PAY_WAY, ORDER_PAY_WAY_VALUE.to_owned()),
                (ORDER_FIELD_ALL_FIELD_TIME, format!("{res_hash}#{date}")),
            ],
            PlanInner::Unsubscribe { book_id } => {
                vec![(UNSUBSCRIBE_FIELD_BOOK_ID, book_id.clone())]
            }
            PlanInner::UpdatePhone { .. } => Vec::new(),
        }
    }
}

impl fmt::Debug for SportsWritePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsWritePlan")
            .field("operation", &self.operation)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query_params", &self.query.split('&').count())
            .field("field_count", &self.body_fields().len())
            .finish()
    }
}

/// The account's contact number, as a value that never prints itself.
///
/// It is the account holder's own number, so it is kept in a zeroizing buffer
/// and its `Debug` reports its length instead of its digits.  It is dropped with
/// the request that carries it.  Unlike a password it is not a credential, but
/// it is personal data and has no business in a log or a bridge DTO.
pub struct SportsPhone(Zeroizing<String>);

impl SportsPhone {
    /// Validates and holds one contact number.
    ///
    /// The rule is the observed client's own, reproduced exactly: a mainland
    /// mobile number.  It is applied before any request, so a mistyped number
    /// costs no round trip.
    pub fn new(value: &str) -> Result<Self, SportsWriteError> {
        if value.is_empty() {
            return Err(SportsWriteError::EmptyPhone);
        }
        if value.len() > 32 || value.chars().any(char::is_control) {
            return Err(SportsWriteError::PhoneInvalid);
        }
        if !is_mainland_mobile(value) {
            return Err(SportsWriteError::PhoneInvalid);
        }
        Ok(Self(Zeroizing::new(value.to_owned())))
    }

    /// Returns the number.  It is deliberately `pub(crate)`: the value exists to
    /// be copied into one request and for nothing else.
    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SportsPhone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsPhone")
            .field("digits", &self.0.len())
            .finish()
    }
}

/// The image challenge's value, as a person typed it.
///
/// It is a one-time value the venue issued for one page, so it is held in a
/// zeroizing buffer, its `Debug` reports its length, and it is dropped with the
/// request that carries it.
pub struct SportsCaptchaCode(Zeroizing<String>);

impl SportsCaptchaCode {
    /// Holds one captcha value.
    ///
    /// The value is a person's transcription of an image, so it is kept exactly
    /// as typed apart from refusing an empty or unusable one: trimming would
    /// silently "fix" a reading this module cannot check.
    pub fn new(value: &str) -> Result<Self, SportsWriteError> {
        if value.trim().is_empty() {
            return Err(SportsWriteError::EmptyCaptcha);
        }
        if value.len() > MAX_SPORTS_CAPTCHA_CHARS || value.chars().any(char::is_control) {
            return Err(SportsWriteError::CaptchaInvalid);
        }
        Ok(Self(Zeroizing::new(value.to_owned())))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SportsCaptchaCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsCaptchaCode")
            .field("chars", &self.0.chars().count())
            .finish()
    }
}

/// The image challenge, as bytes a UI can display.
///
/// The bytes are an image the venue itself rendered, not a credential, so they
/// are returned.  They are bounded and the content type is verified to be a
/// raster image before the value is constructed, so a login page or an error
/// document can never arrive here dressed as a captcha.
#[derive(Clone, PartialEq, Eq)]
pub struct SportsCaptcha {
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for SportsCaptcha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsCaptcha")
            .field("content_type", &self.content_type)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

/// The fixed sports-venue write route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SportsWriteProfile;
impl SportsWriteProfile {
    pub const fn new() -> Self {
        Self
    }

    /// Builds one order plan.
    ///
    /// Every caller-supplied value is bounded here, before any request can be
    /// built: the venue and item identifiers are digits, the date is a real
    /// calendar day, and the booking hash is a bounded token.  The phone number
    /// and the captcha are already validated values.
    ///
    /// `total_cost` is the venue's **own** cost token, sent back exactly as the
    /// slot read reported it: the module does not convert it into a currency
    /// amount, because the observed evidence does not establish a unit for it.
    /// The observed application's single-payment ceiling is applied to the same
    /// token's numeric value, which is the comparison that client makes.
    pub fn order_request(
        &self,
        total_cost: &str,
        phone: SportsPhone,
        gym_id: &str,
        item_id: &str,
        date: &str,
        captcha: SportsCaptchaCode,
        res_hash: &str,
    ) -> Result<SportsWritePlan, SportsWriteError> {
        let gym_id = venue_identifier(gym_id)?;
        let item_id = venue_identifier(item_id)?;
        let date = calendar_date(date)?;
        let res_hash = booking_hash(res_hash)?;
        let total_cost = cost_token(total_cost)?;
        if let Ok(cost) = total_cost.parse::<u32>() {
            if cost > SPORTS_MAX_SINGLE_PAYMENT_COST {
                return Err(SportsWriteError::CostAboveLocalCeiling {
                    cost,
                    max: SPORTS_MAX_SINGLE_PAYMENT_COST,
                });
            }
        }
        Ok(SportsWritePlan {
            operation: SportsWriteOperation::MakeOrder,
            method: SportsWriteMethod::Post,
            path: SPORTS_MAKE_ORDER_PATH,
            query: SPORTS_MAKE_ORDER_QUERY.to_owned(),
            inner: PlanInner::Order {
                cost: total_cost.to_owned(),
                phone,
                gym_id: gym_id.to_owned(),
                item_id: item_id.to_owned(),
                date: date.to_owned(),
                captcha,
                res_hash: res_hash.to_owned(),
            },
        })
    }

    /// Builds one withdrawal plan.
    ///
    /// The identifier is the one the reservation read reported for the row, and
    /// the Runtime only ever supplies it from a read it performed itself.
    pub fn unsubscribe_request(&self, book_id: &str) -> Result<SportsWritePlan, SportsWriteError> {
        let book_id = order_identifier(book_id)?;
        Ok(SportsWritePlan {
            operation: SportsWriteOperation::Unsubscribe,
            method: SportsWriteMethod::Post,
            path: SPORTS_BOOK_ACTION_PATH,
            query: SPORTS_UNSUBSCRIBE_QUERY.to_owned(),
            inner: PlanInner::Unsubscribe {
                book_id: book_id.to_owned(),
            },
        })
    }

    /// Builds one contact-number update plan.
    ///
    /// `account_id` is the **bound account's own login id**, derived by the
    /// runtime from the proven identity — never a caller argument.  It is
    /// validated here as a digit string so the plan can concatenate it into the
    /// observed query shape byte for byte, which is what the deployment's own
    /// client does.
    pub fn update_phone_request(
        &self,
        phone: SportsPhone,
        account_id: &str,
    ) -> Result<SportsWritePlan, SportsWriteError> {
        let account_id = account_identifier(account_id)?;
        let query = format!(
            "{SPORTS_UPDATE_PHONE_QUERY_PREFIX}{}&{SPORTS_UPDATE_PHONE_ACCOUNT_PARAM}={account_id}",
            phone.expose()
        );
        Ok(SportsWritePlan {
            operation: SportsWriteOperation::UpdatePhone,
            method: SportsWriteMethod::Post,
            path: SPORTS_BOOK_ACTION_PATH,
            query,
            inner: PlanInner::UpdatePhone {
                phone,
                account_id: account_id.to_owned(),
            },
        })
    }
}

/// Why a plan could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SportsWriteError {
    #[error("sports contact number is empty")]
    EmptyPhone,

    #[error("sports contact number is not a mainland mobile number")]
    PhoneInvalid,

    #[error("sports captcha value is empty")]
    EmptyCaptcha,

    #[error("sports captcha value is not a value this client will send")]
    CaptchaInvalid,

    #[error("sports venue identifier is not a value this client will send")]
    InvalidVenueIdentifier,

    #[error("sports date is not a calendar day")]
    InvalidDate,

    #[error("sports booking hash is not a value this client will send")]
    InvalidBookingHash,

    #[error("sports order identifier is not a value this client will send")]
    InvalidOrderIdentifier,

    #[error("sports cost token is not a value this client will send back")]
    InvalidCostToken,

    #[error("sports account identifier is not a value this client will send")]
    InvalidAccountIdentifier,

    #[error("sports cost {cost} is above this client's single-payment ceiling {max}")]
    CostAboveLocalCeiling { cost: u32, max: u32 },

    #[error("sports receipt title is not one of the venue's own")]
    InvalidReceiptTitle,
}

/// Adapter failures are body-free so a login page, a captcha value, a booking
/// hash or the account's own login id cannot leak through a debug rendering or a
/// bridge DTO.
#[derive(Debug, Error)]
pub enum SportsWriteAdapterError {
    #[error("sports write base URL is invalid")]
    InvalidBaseUrl,

    #[error("sports write plan belongs to another operation")]
    WriteOperation,

    #[error("sports write transport failed")]
    Transport(#[source] TransportError),

    #[error("sports write returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("sports write answer came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("sports write answer ended outside the configured mapping")]
    UnexpectedPath,

    #[error("sports write answer is not the expected deployment")]
    UnexpectedDeployment,

    #[error("sports write answer is not a captcha image")]
    UnexpectedContentType,

    #[error("sports INFO/WebVPN session has expired or is not established")]
    SessionExpired,
}

impl SportsWriteAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "sports_config",
            Self::WriteOperation => "sports_write_request",
            Self::Transport(_) => "sports_write_network",
            Self::HttpStatus { .. } => "sports_write_http",
            Self::UnexpectedOrigin => "sports_origin",
            Self::UnexpectedPath => "sports_path",
            Self::UnexpectedDeployment => "sports_write_template",
            Self::UnexpectedContentType => "sports_write_content_type",
            Self::SessionExpired => "sports_write_session_expired",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::SessionExpired)
    }
}

/// What one dispatched write turned out to be.
///
/// `Refused` is the service's own worded refusal, and only the order route
/// produces one: it states its outcome in a `msg` field.  The withdrawal and
/// contact-number routes have no observed refusal wording, so they can only be
/// `Accepted`, `LoginRequired` or `Unrecognized`.
///
/// `Unrecognized` is an answer this module could not read, which means the
/// request left and its effect is unknown.  It is never resolved here by sending
/// the request again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SportsWriteOutcome {
    Accepted,
    Refused,
    LoginRequired,
    Unrecognized,
}

/// The sports-venue write client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must be
/// the one that already carries the identity/INFO/WebVPN cookie jar, and the
/// base URL the one the venue's read half proved.
pub struct SportsWriteAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: SportsWriteProfile,
}

impl SportsWriteAdapter {
    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, SportsWriteAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: SportsWriteProfile::new(),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> SportsWriteProfile {
        self.profile
    }

    /// The mapping root this adapter was configured with.
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// Reads the order route's image challenge.
    ///
    /// The venue's own client appends a random parameter so a proxy does not
    /// serve a stale image; this module does the same from a value that is not a
    /// credential, and the answer is accepted only when it is a bounded raster
    /// image.  A login page, an error document, or an oversized body is an error
    /// rather than an empty image.
    pub async fn read_captcha(&self) -> Result<SportsCaptcha, SportsWriteAdapterError> {
        let mut endpoint = self.endpoint(SPORTS_CAPTCHA_PATH)?;
        // The observed client's own cache-buster: a query key with an empty name
        // carrying a small decimal value.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos() % 100)
            .unwrap_or(0);
        endpoint.set_query(Some(&format!("{stamp}=")));
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| SportsWriteAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
        {
            return Err(SportsWriteAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url) {
            return Err(SportsWriteAdapterError::UnexpectedOrigin);
        }
        if !path_within_base(&self.base_url, &final_url) || final_url.path() != SPORTS_CAPTCHA_PATH
        {
            return Err(SportsWriteAdapterError::UnexpectedPath);
        }
        if status != StatusCode::OK {
            return Err(SportsWriteAdapterError::HttpStatus { status });
        }
        let bytes =
            match crate::telemetry::timing::read_bounded_bytes(response, MAX_CAPTCHA_BYTES).await {
                Ok(bytes) => bytes,
                Err(_error) => return Err(SportsWriteAdapterError::UnexpectedDeployment),
            };
        let Some(content_type) =
            content_type.filter(|value| crate::captcha_image::is_bounded_raster(value, &bytes))
        else {
            // The answer is not an image this client can show, so it is not the
            // challenge: an HTML login page arriving with HTTP 200 must not be
            // handed to a UI as a picture.
            return Err(SportsWriteAdapterError::UnexpectedContentType);
        };
        Ok(SportsCaptcha {
            content_type: Some(content_type),
            bytes,
        })
    }

    /// Dispatches one order, exactly once.
    ///
    /// The answer is the venue's own JSON, whose `msg` field states the outcome.
    /// A transport failure after the body left is reported as unconfirmed rather
    /// than as an error: the request has already been sent, and nothing here
    /// sends it again.  The captcha is never re-read and never retried — a
    /// second attempt is a second deliberate human reading.
    pub async fn make_order(
        &self,
        plan: &SportsWritePlan,
    ) -> Result<SportsWriteOutcome, SportsWriteAdapterError> {
        if plan.operation != SportsWriteOperation::MakeOrder
            || plan.method != SportsWriteMethod::Post
            || plan.path != SPORTS_MAKE_ORDER_PATH
        {
            return Err(SportsWriteAdapterError::WriteOperation);
        }
        match self.dispatch(plan).await? {
            Some(response) => Ok(classify_order_answer(&response.body)),
            None => Ok(SportsWriteOutcome::Unrecognized),
        }
    }

    /// Dispatches one withdrawal, exactly once.
    ///
    /// The observed client discards this route's answer entirely, so no
    /// affirmative wording has ever been seen on it.  A readable answer that is
    /// not an affirmative envelope is therefore reported as unconfirmed rather
    /// than invented as a refusal.
    pub async fn unsubscribe(
        &self,
        plan: &SportsWritePlan,
    ) -> Result<SportsWriteOutcome, SportsWriteAdapterError> {
        if plan.operation != SportsWriteOperation::Unsubscribe
            || plan.method != SportsWriteMethod::Post
            || plan.path != SPORTS_BOOK_ACTION_PATH
            || plan.query != SPORTS_UNSUBSCRIBE_QUERY
        {
            return Err(SportsWriteAdapterError::WriteOperation);
        }
        match self.dispatch(plan).await? {
            Some(response) => Ok(classify_unconfirmed_only(&response.body)),
            None => Ok(SportsWriteOutcome::Unrecognized),
        }
    }

    /// Dispatches one contact-number update, exactly once.
    ///
    /// Everything the route needs is in the query, so the body is empty; this is
    /// the observed shape, not a simplification.  The observed client discards
    /// the answer too — its only check is for the sign-in page's `找回密码`
    /// marker, which the shared login classification below already covers — so
    /// this route has no affirmative wording either and cannot report a refusal.
    pub async fn update_phone(
        &self,
        plan: &SportsWritePlan,
    ) -> Result<SportsWriteOutcome, SportsWriteAdapterError> {
        if plan.operation != SportsWriteOperation::UpdatePhone
            || plan.method != SportsWriteMethod::Post
            || plan.path != SPORTS_BOOK_ACTION_PATH
            || !plan.query.starts_with(SPORTS_UPDATE_PHONE_QUERY_PREFIX)
        {
            return Err(SportsWriteAdapterError::WriteOperation);
        }
        match self.dispatch(plan).await? {
            Some(response) => Ok(classify_unconfirmed_only(&response.body)),
            None => Ok(SportsWriteOutcome::Unrecognized),
        }
    }

    /// Builds and dispatches one write.
    ///
    /// `Ok(None)` means the request left but its effect cannot be read, which
    /// every caller in this module reports as an unconfirmed outcome.  The body
    /// is encoded into the request and the plaintext copy this module built is
    /// dropped before the dispatch, so the captcha and the booking hash live in
    /// exactly one request.  The dispatch takes the exclusive gate and never
    /// follows a redirect, so a redirect is reported rather than walked.
    async fn dispatch(
        &self,
        plan: &SportsWritePlan,
    ) -> Result<Option<CampusTextResponse>, SportsWriteAdapterError> {
        let mut endpoint = self.endpoint(plan.path())?;
        endpoint.set_query(Some(plan.query()));
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let body = plan.body();
        let request = self
            .transport
            .client()
            .post(endpoint)
            .form(&body)
            .build()
            .map_err(|_| SportsWriteAdapterError::InvalidBaseUrl)?;
        // The body has been encoded into the request, so the plaintext copy this
        // module built is cleared before the request is dispatched.
        drop(body);
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
                // is deliberately dropped: it may name the request URL, whose
                // query carries the account's own login id and contact number on
                // the update route and the one-time booking hash on the order
                // route.
                return Ok(None);
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
        // alone, before the body is read.
        let login_redirect = redirect_location.as_deref().is_some_and(|location| {
            resolve_location(&final_url, location).is_ok_and(|target| looks_like_login_url(&target))
        });
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || login_redirect
        {
            return Err(SportsWriteAdapterError::SessionExpired);
        }
        let bytes =
            match crate::telemetry::timing::read_bounded_bytes(response, MAX_WRITE_RESPONSE_BYTES)
                .await
            {
                Ok(bytes) => bytes,
                Err(_error) => {
                    // The request has already left, so an oversized or truncated
                    // answer leaves the outcome unknown rather than failed.
                    return Ok(None);
                }
            };
        let body = String::from_utf8_lossy(&bytes).into_owned();
        if matches!(
            campus_html::classify_page(&body),
            PageClass::Login | PageClass::Expired
        ) {
            // A WebVPN login page and an HTTP-200 session-expiry page are both
            // the deployment saying the session is gone.  Neither is dispatch
            // evidence.
            return Err(SportsWriteAdapterError::SessionExpired);
        }
        let response = CampusTextResponse {
            status,
            final_url,
            content_type,
            redirect_location: redirect_location.clone(),
            body,
        };
        let cross_origin = !same_origin(&self.base_url, &response.final_url)
            || redirect_location.as_deref().is_some_and(|location| {
                resolve_location(&response.final_url, location)
                    .is_ok_and(|target| !same_origin(&self.base_url, &target))
            });
        if cross_origin {
            return Err(SportsWriteAdapterError::UnexpectedOrigin);
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
            || response.final_url.query().map(str::to_owned) != expected_query
            || redirect_outside_mapping
            || !path_within_base(&self.base_url, &response.final_url)
        {
            // A redirect this module did not follow, a page for another route, or
            // a route outside the mapping: the request has already left, so the
            // effect is unknown rather than failed.  Never re-dispatched.
            return Ok(None);
        }
        Ok(Some(response))
    }

    fn endpoint(&self, relative_path: &str) -> Result<Url, SportsWriteAdapterError> {
        if !valid_relative_path(relative_path) {
            return Err(SportsWriteAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{relative_path}"));
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }
}

impl fmt::Debug for SportsWriteAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SportsWriteAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Reads the venue's own order outcome out of its answer.
///
/// The route answers a small JSON object whose `msg` states the result.  Only
/// the one message the observed client accepts as success is acceptance; any
/// other readable `msg` is the service's own refusal, and its wording is
/// deliberately left inside this function so a refusal reaches a caller as a
/// category rather than as text.  An answer that is not that object at all — an
/// HTML page, an empty body, a body with no `msg` — is unreadable, which means
/// the effect is unknown.
pub fn classify_order_answer(body: &str) -> SportsWriteOutcome {
    let trimmed = body.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return SportsWriteOutcome::Unrecognized;
    }
    if matches!(
        campus_html::classify_page(trimmed),
        PageClass::Login | PageClass::Expired
    ) {
        return SportsWriteOutcome::LoginRequired;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return SportsWriteOutcome::Unrecognized;
    };
    let Some(object) = value.as_object() else {
        return SportsWriteOutcome::Unrecognized;
    };
    let Some(message) = object.get("msg").and_then(|value| value.as_str()) else {
        return SportsWriteOutcome::Unrecognized;
    };
    if message.trim() == ORDER_ACCEPTED_MESSAGE {
        SportsWriteOutcome::Accepted
    } else {
        // The service stated an outcome and it was not acceptance.  Its wording
        // stays here: a caller gets the category, not the venue's sentence.
        SportsWriteOutcome::Refused
    }
}

/// Reads a route whose answer no client has ever inspected.
///
/// Only affirmative evidence counts as acceptance — an empty body, `OK`, or an
/// affirmative envelope — and everything else is unconfirmed rather than
/// refused, because no refusal wording has ever been observed on the withdrawal
/// or contact-number routes.
pub fn classify_unconfirmed_only(body: &str) -> SportsWriteOutcome {
    let trimmed = body.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("ok") {
        return SportsWriteOutcome::Accepted;
    }
    if matches!(
        campus_html::classify_page(trimmed),
        PageClass::Login | PageClass::Expired
    ) {
        return SportsWriteOutcome::LoginRequired;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return SportsWriteOutcome::Unrecognized;
    };
    let Some(object) = value.as_object() else {
        return SportsWriteOutcome::Unrecognized;
    };
    if object.keys().any(|key| {
        matches!(key.as_str(), "error" | "exception" | "failReason")
            && object.get(key).is_some_and(|value| !value.is_null())
    }) {
        return SportsWriteOutcome::Unrecognized;
    }
    for key in ["status", "result", "success"] {
        match object.get(key) {
            Some(serde_json::Value::Bool(true)) => return SportsWriteOutcome::Accepted,
            Some(serde_json::Value::Number(number)) => {
                return if number.as_i64() == Some(1) {
                    SportsWriteOutcome::Accepted
                } else {
                    SportsWriteOutcome::Unrecognized
                };
            }
            Some(serde_json::Value::String(text)) => {
                return if text.eq_ignore_ascii_case("success") {
                    SportsWriteOutcome::Accepted
                } else {
                    SportsWriteOutcome::Unrecognized
                };
            }
            _ => {}
        }
    }
    SportsWriteOutcome::Unrecognized
}

/// Checks whether a receipt title is one of the venue's own.
///
/// Only the recorded payment chain consumes a receipt title, so this is a
/// documentation-level check for the slice that would implement it.
pub fn receipt_title_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SPORTS_RECEIPT_CHARS
        && VALID_RECEIPT_TITLES.contains(&value)
}

/// Applies the observed client's own contact-number rule.
///
/// It is reproduced exactly as the reference states it, quirks included.  The
/// first alternative already subsumes the other two — `1[3-9][0-9]` matches every
/// `15x` and `18x` head the other branches name, so those two can never select
/// anything the first would not — but it is the rule the deployment's own client
/// applies, so this module reproduces it as written rather than "fixing" it into
/// a different acceptance.  It is a **local** check only: the service has the
/// final word.
fn is_mainland_mobile(value: &str) -> bool {
    let bytes = value.as_bytes();
    let subsumed = matches!(
        (bytes[1], bytes[2]),
        (b'5', b'0' | b'3' | b'6' | b'7' | b'8' | b'9') | (b'8', b'8' | b'9')
    );
    bytes.len() == 11
        && bytes[0] == b'1'
        && (matches!(bytes[1], b'3'..=b'9') || subsumed)
        && bytes.iter().all(u8::is_ascii_digit)
}

/// Bounds one venue identifier before it can enter a body.
fn venue_identifier(value: &str) -> Result<&str, SportsWriteError> {
    if value.is_empty()
        || value.len() > MAX_ID_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SportsWriteError::InvalidVenueIdentifier);
    }
    Ok(value)
}

/// Bounds one date before it can enter a body.
fn calendar_date(value: &str) -> Result<&str, SportsWriteError> {
    if value.len() != MAX_DATE_CHARS
        || !value.is_ascii()
        || chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()
    {
        return Err(SportsWriteError::InvalidDate);
    }
    Ok(value)
}

/// Bounds one booking hash before it can enter a body.
///
/// The value is the reader's own `res_hash`, so it is a hexadecimal token of the
/// shape the venue issued.  Anything else is refused rather than sent: a hash
/// this module cannot recognise is not one it should carry.
fn booking_hash(value: &str) -> Result<&str, SportsWriteError> {
    if value.is_empty()
        || value.len() > MAX_SPORTS_HASH_CHARS
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(SportsWriteError::InvalidBookingHash);
    }
    Ok(value)
}

/// Bounds one order identifier before it can enter a body.
fn order_identifier(value: &str) -> Result<&str, SportsWriteError> {
    if value.is_empty()
        || value.len() > MAX_ID_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SportsWriteError::InvalidOrderIdentifier);
    }
    Ok(value)
}

/// Bounds the venue's own cost token before it can enter a body.
///
/// The reader reports the token verbatim, so a token this module cannot bound is
/// not one it should send back.  A leading `-` is accepted because the scan may
/// report a negative adjustment; everything else is refused.  The single-payment
/// ceiling is applied to the same string's numeric value by the caller.
fn cost_token(value: &str) -> Result<&str, SportsWriteError> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || digits.len() > MAX_ID_DIGITS
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SportsWriteError::InvalidCostToken);
    }
    Ok(value)
}

/// Bounds the bound account's login id before it can enter a query.
///
/// The value comes from the proven identity rather than from a caller, and it is
/// still bounded here so the observed query shape can be reproduced by
/// concatenation without any encoding step.
fn account_identifier(value: &str) -> Result<&str, SportsWriteError> {
    if value.is_empty()
        || value.len() > MAX_ID_DIGITS
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SportsWriteError::InvalidAccountIdentifier);
    }
    Ok(value)
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, SportsWriteAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(SportsWriteAdapterError::InvalidBaseUrl);
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
/// The venue read half hands this adapter the mapping root it proved, which may
/// still carry the read route; the root is what an endpoint is built from, so
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
    [
        crate::sports_read::SPORTS_BOOK_PATH,
        crate::sports_read::SPORTS_DETAIL_PATH,
        crate::sports_read::SPORTS_PAY_PATH,
        SPORTS_MAKE_ORDER_PATH,
        SPORTS_CAPTCHA_PATH,
    ]
    .into_iter()
    .find_map(|suffix| {
        let prefix = path.strip_suffix(suffix)?;
        if prefix.is_empty() {
            Some("/".to_owned())
        } else {
            Some(format!("{}/", prefix.trim_end_matches('/')))
        }
    })
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, SportsWriteAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(SportsWriteAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| SportsWriteAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(SportsWriteAdapterError::UnexpectedOrigin);
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
