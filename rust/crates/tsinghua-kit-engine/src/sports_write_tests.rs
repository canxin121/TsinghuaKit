//! Loopback fixtures for the sports-venue write slice: the order, the
//! withdrawal, the contact-number update, and the order form's image challenge.
//!
//! Every case asserts the dispatch count as well as the classification: a venue
//! state change is dispatched exactly once, and an answer the venue did not
//! confirm must not turn into a second request.  The fixture origin is loopback,
//! so no real deployment, account or credential is involved.

use std::time::Duration;

use reqwest::{StatusCode, Url};

use crate::reference_test_support::{FixtureServer, Reply, captcha_png};
use crate::sports_write::*;
use crate::transport::CampusHttpTransport;

/// The bound account's own login id.  The Runtime derives the real one from the
/// proven identity; here it is only a fixture value.
const ACCOUNT_ID: &str = "2020091118";
/// The account's own contact number, as a fixture value.
const PHONE: &str = "13800138000";
/// One booking hash of the shape the venue issues.
const RES_HASH: &str = "F3681513C5BC25CEDDF5FA7C3E8429F1769DBA48B7BD42CC";

fn write_adapter(server: &FixtureServer) -> SportsWriteAdapter {
    let base = Url::parse(server.base()).expect("fixture base");
    let transport =
        CampusHttpTransport::with_timeout("THYou/sports-write", Duration::from_secs(10))
            .expect("transport");
    SportsWriteAdapter::try_with_transport(base, transport).expect("adapter")
}

fn order_plan(adapter: &SportsWriteAdapter, captcha: &str, cost: &str) -> SportsWritePlan {
    adapter
        .profile()
        .order_request(
            cost,
            SportsPhone::new(PHONE).expect("phone"),
            "3998000",
            "4045681",
            "2024-09-20",
            SportsCaptchaCode::new(captcha).expect("captcha"),
            RES_HASH,
        )
        .expect("order plan")
}

/// A JSON answer whose `msg` is the venue's own acceptance.
fn accepted_answer() -> Reply {
    Reply::json("{\"msg\":\"预定成功\"}")
}

fn request_body(request: &str) -> &str {
    request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn request_line(request: &str) -> &str {
    request.lines().next().unwrap_or_default()
}

#[tokio::test]
async fn backend_refactor_sports_order_accepts_only_the_venues_own_message() {
    let server = FixtureServer::new(vec![accepted_answer()]);
    let adapter = write_adapter(&server);
    let plan = order_plan(&adapter, "a1b2", "25");
    assert_eq!(
        adapter.make_order(&plan).await.expect("order"),
        SportsWriteOutcome::Accepted
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let line = request_line(&requests[0]);
    assert!(
        line.starts_with("POST /gymbook/gymbook/gymBookAction.do?"),
        "unexpected request line: {line}"
    );
    // The observed deployment's own query shape, reproduced verbatim.
    assert!(line.contains("vpn-12-o1-50.tsinghua.edu.cn=&ms=saveGymBook"));
    // The order body is exactly the observed field set.
    let body = request_body(&requests[0]);
    for field in [
        "bookData.totalCost=25",
        "bookData.book_person_phone=13800138000",
        "bookData.book_mode=from-phone",
        "gymnasium_idForCache=3998000",
        "item_idForCache=4045681",
        "time_dateForCache=2024-09-20",
        "userTypeNumForCache=1",
        "putongRes=putongRes",
        "code=a1b2",
        "selectedPayWay=1",
    ] {
        assert!(body.contains(field), "body is missing {field}: {body}");
    }
    // The booking hash travels in the order body and nowhere else.
    assert!(
        body.contains(&format!("allFieldTime={RES_HASH}%232024-09-20")),
        "body is missing the booking hash: {body}"
    );
    // The contact-number route's own account parameter is not part of an order.
    assert!(
        !body.contains("gzzh"),
        "order body carries an account id: {body}"
    );
}

#[tokio::test]
async fn backend_refactor_sports_order_refusal_is_distinct_from_an_unreadable_answer() {
    // A worded answer that is not acceptance is the venue's own refusal: a
    // definite "nothing was booked".
    let server = FixtureServer::new(vec![Reply::json("{\"msg\":\"该时间段已满\"}")]);
    let adapter = write_adapter(&server);
    let plan = order_plan(&adapter, "a1b2", "25");
    assert_eq!(
        adapter.make_order(&plan).await.expect("order"),
        SportsWriteOutcome::Refused
    );
    assert_eq!(server.requests().len(), 1);
    // The refusal's wording is a category to the caller, not text.
    let rendered = format!("{:?}", SportsWriteOutcome::Refused);
    assert!(!rendered.contains("已满"));

    for unreadable in [
        Reply::html("<html><body>系统维护中</body></html>"),
        Reply {
            status: 200,
            headers: "Content-Type: application/json\r\n".into(),
            body: Vec::new(),
        },
        Reply::json("{\"detail\":\"no msg here\"}"),
    ] {
        let server = FixtureServer::new(vec![unreadable]);
        let adapter = write_adapter(&server);
        let plan = order_plan(&adapter, "a1b2", "25");
        assert_eq!(
            adapter.make_order(&plan).await.expect("order"),
            SportsWriteOutcome::Unrecognized,
            "an unreadable answer must never be reported as a refusal"
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_refactor_sports_write_reports_an_unfollowed_redirect_as_unconfirmed() {
    // A redirect the exclusive gate does not follow may already have applied the
    // order, so the outcome is unknown rather than failed — and the request is
    // not sent again.
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: /gymbook/gymbook/other.do\r\n".into(),
        body: Vec::new(),
    }]);
    let adapter = write_adapter(&server);
    let plan = order_plan(&adapter, "a1b2", "25");
    assert_eq!(
        adapter.make_order(&plan).await.expect("order"),
        SportsWriteOutcome::Unrecognized
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_refactor_sports_write_requires_a_session_before_the_body_is_read() {
    for reply in [
        Reply {
            status: StatusCode::UNAUTHORIZED.as_u16(),
            headers: String::new(),
            body: "<html><body>login</body></html>".into(),
        },
        Reply {
            status: 302,
            headers: "Location: /login\r\n".into(),
            body: Vec::new(),
        },
    ] {
        let server = FixtureServer::new(vec![reply]);
        let adapter = write_adapter(&server);
        let plan = order_plan(&adapter, "a1b2", "25");
        let error = adapter.make_order(&plan).await.expect_err("expired");
        assert!(error.is_session_expired(), "unexpected error: {error:?}");
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_refactor_sports_captcha_is_an_image_or_an_error_never_a_document() {
    let server = FixtureServer::new(vec![Reply::bytes(200, "image/png", captcha_png())]);
    let adapter = write_adapter(&server);
    let captcha = adapter.read_captcha().await.expect("captcha");
    assert_eq!(captcha.bytes, captcha_png());
    assert_eq!(captcha.content_type.as_deref(), Some("image/png"));
    // The challenge is an ordinary read, so it is dispatched as a GET on its own
    // route and the cache-buster the deployment uses is present.
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        request_line(&requests[0]).starts_with("GET /Kaptcha.jpg?"),
        "unexpected request line: {}",
        request_line(&requests[0])
    );

    // A login page answered with HTTP 200 and an image content type is not a
    // challenge, and neither is a real image labelled as a document.
    for (content_type, body) in [
        ("image/png", b"<html><body>login</body></html>".to_vec()),
        ("text/html; charset=utf-8", captcha_png()),
    ] {
        let server = FixtureServer::new(vec![Reply::bytes(200, content_type, body)]);
        let adapter = write_adapter(&server);
        let error = adapter.read_captcha().await.expect_err("not a captcha");
        assert!(
            matches!(error, SportsWriteAdapterError::UnexpectedContentType),
            "unexpected error: {error:?}"
        );
    }
}

#[tokio::test]
async fn backend_refactor_sports_withdrawal_and_phone_update_never_report_a_refusal() {
    let server = FixtureServer::new(vec![Reply::html(""), Reply::html("")]);
    let adapter = write_adapter(&server);

    // The withdrawal: an empty body is the only affirmative evidence this route
    // has ever offered, and the identifier travels in the body.
    let plan = adapter
        .profile()
        .unsubscribe_request("15674139")
        .expect("withdrawal plan");
    assert_eq!(
        adapter.unsubscribe(&plan).await.expect("withdrawal"),
        SportsWriteOutcome::Accepted
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let line = request_line(&requests[0]);
    assert!(
        line.starts_with("POST /gymbook/gymBookAction.do?ms=unsubscribe "),
        "unexpected request line: {line}"
    );
    assert!(request_body(&requests[0]).contains("bookId=15674139"));

    // A readable answer that is not affirmative is unconfirmed, never refused:
    // no client has ever observed refusal wording on this route.
    let server = FixtureServer::new(vec![Reply::html("<html><body>操作未完成</body></html>")]);
    let adapter = write_adapter(&server);
    let plan = adapter
        .profile()
        .unsubscribe_request("15674139")
        .expect("withdrawal plan");
    assert_eq!(
        adapter.unsubscribe(&plan).await.expect("withdrawal"),
        SportsWriteOutcome::Unrecognized
    );

    // The contact-number update: everything is in the query, so the body is
    // empty.  That is the observed shape, not a simplification.
    let server = FixtureServer::new(vec![Reply::html("")]);
    let adapter = write_adapter(&server);
    let plan = adapter
        .profile()
        .update_phone_request(SportsPhone::new(PHONE).expect("phone"), ACCOUNT_ID)
        .expect("phone plan");
    assert!(plan.body_fields().is_empty());
    assert_eq!(
        adapter.update_phone(&plan).await.expect("phone update"),
        SportsWriteOutcome::Accepted
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let line = request_line(&requests[0]);
    assert!(
        line.starts_with(
            "POST /gymbook/gymBookAction.do?ms=doUpdateContactInformation&cell_phone=13800138000&gzzh=2020091118 "
        ),
        "unexpected request line: {line}"
    );
    assert!(request_body(&requests[0]).is_empty());
}

#[tokio::test]
async fn backend_refactor_sports_write_plan_never_prints_a_secret_or_a_token() {
    let server = FixtureServer::new(vec![accepted_answer()]);
    let adapter = write_adapter(&server);
    let plan = order_plan(&adapter, "a1b2", "25");
    let rendered = format!("{plan:?}");
    for secret in ["a1b2", RES_HASH, PHONE, ACCOUNT_ID] {
        assert!(
            !rendered.contains(secret),
            "plan leaked {secret}: {rendered}"
        );
    }
    assert!(rendered.contains("field_count"));
    // The route's query is not printed either: on the contact-number route it
    // carries the account's own login id and the contact number.
    let phone_plan = adapter
        .profile()
        .update_phone_request(SportsPhone::new(PHONE).expect("phone"), ACCOUNT_ID)
        .expect("phone plan");
    let rendered = format!("{phone_plan:?}");
    assert!(!rendered.contains(PHONE));
    assert!(!rendered.contains(ACCOUNT_ID));
    assert!(!rendered.contains("cell_phone"));
    // A phone value and a captcha value report a length rather than themselves.
    assert_eq!(
        format!("{:?}", SportsPhone::new(PHONE).unwrap()),
        "SportsPhone { digits: 11 }"
    );
    assert_eq!(
        format!("{:?}", SportsCaptchaCode::new("a1b2").unwrap()),
        "SportsCaptchaCode { chars: 4 }"
    );
    let captcha = SportsCaptcha {
        content_type: Some("image/png".to_owned()),
        bytes: captcha_png(),
    };
    assert!(format!("{captcha:?}").contains("byte_len"));
}

#[test]
fn backend_refactor_sports_write_bounds_every_caller_supplied_value() {
    let profile = SportsWriteProfile::new();
    let phone = || SportsPhone::new(PHONE).expect("phone");
    let captcha = || SportsCaptchaCode::new("a1b2").expect("captcha");

    // A cost the observed client's own ceiling refuses is refused locally.
    assert_eq!(
        profile
            .order_request(
                "43",
                phone(),
                "3998000",
                "4045681",
                "2024-09-20",
                captcha(),
                RES_HASH
            )
            .expect_err("above ceiling"),
        SportsWriteError::CostAboveLocalCeiling { cost: 43, max: 42 }
    );
    for (cost, expected) in [
        ("", SportsWriteError::InvalidCostToken),
        ("2 5", SportsWriteError::InvalidCostToken),
        ("1e3", SportsWriteError::InvalidCostToken),
    ] {
        assert_eq!(
            profile
                .order_request(
                    cost,
                    phone(),
                    "3998000",
                    "4045681",
                    "2024-09-20",
                    captcha(),
                    RES_HASH
                )
                .expect_err("cost"),
            expected
        );
    }
    // Venue identifiers and the date are bounded before a body exists.
    for (gym, item, date, expected) in [
        (
            "",
            "4045681",
            "2024-09-20",
            SportsWriteError::InvalidVenueIdentifier,
        ),
        (
            "3998000",
            "x",
            "2024-09-20",
            SportsWriteError::InvalidVenueIdentifier,
        ),
        (
            "3998000",
            "4045681",
            "2024-09-31",
            SportsWriteError::InvalidDate,
        ),
        (
            "3998000",
            "4045681",
            "2024-9-20",
            SportsWriteError::InvalidDate,
        ),
    ] {
        assert_eq!(
            profile
                .order_request("25", phone(), gym, item, date, captcha(), RES_HASH)
                .expect_err("venue"),
            expected
        );
    }
    // A hash the reader could not have produced is not carried.
    for hash in ["", "not-a-hash", &"a".repeat(65)] {
        assert_eq!(
            profile
                .order_request(
                    "25",
                    phone(),
                    "3998000",
                    "4045681",
                    "2024-09-20",
                    captcha(),
                    hash
                )
                .expect_err("hash"),
            SportsWriteError::InvalidBookingHash
        );
    }
    // The observed deployment's own contact-number rule, applied before any
    // request.
    for rejected in [
        "",
        "1380013800",
        "1380013800x",
        "23800138000",
        "138001380000",
    ] {
        assert!(
            SportsPhone::new(rejected).is_err(),
            "{rejected} must not be accepted as a contact number"
        );
    }
    // A captcha is a person's transcription, so an empty or over-long one is a
    // caller error rather than something this module trims into shape.
    for rejected in ["", "   ", &"a".repeat(MAX_SPORTS_CAPTCHA_CHARS + 1)] {
        assert!(SportsCaptchaCode::new(rejected).is_err());
    }
    // The account identifier the runtime derives is a digit string; a caller can
    // never reach this with one of its own.
    assert_eq!(
        profile
            .update_phone_request(phone(), "not-an-id")
            .expect_err("account"),
        SportsWriteError::InvalidAccountIdentifier
    );
}

#[tokio::test]
async fn backend_refactor_sports_write_never_reaches_the_payment_chain() {
    // The payment chain's own mapping token, host and routes are recorded as a
    // documented boundary.  No write here can address them, so a caller cannot
    // move money through this slice even by naming an order.
    let server = FixtureServer::new(vec![accepted_answer()]);
    let adapter = write_adapter(&server);
    let plan = order_plan(&adapter, "a1b2", "25");
    assert_ne!(plan.path(), SPORTS_PAYMENT_CHECK_PATH);
    assert_ne!(plan.path(), SPORTS_PAYMENT_ACTION_PATH);
    assert_ne!(plan.path(), SPORTS_MAKE_PAYMENT_PATH);
    assert_eq!(
        adapter.make_order(&plan).await.expect("order"),
        SportsWriteOutcome::Accepted
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    for payment_route in [
        SPORTS_PAYMENT_CHECK_PATH,
        SPORTS_PAYMENT_ACTION_PATH,
        SPORTS_MAKE_PAYMENT_PATH,
    ] {
        assert!(
            !requests[0].contains(payment_route),
            "a write reached {payment_route}"
        );
    }
    assert!(!requests[0].contains(SPORTS_PAYMENT_HOST));
    // The recorded mapping token is a constant only: it is not the mapping the
    // read half proved, and registering it would be the only reason this module
    // would need a second mapping.
    assert_ne!(
        SPORTS_PAYMENT_MAPPING_TOKEN,
        crate::sports_read::SPORTS_MAPPING_TOKEN
    );
    assert!(receipt_title_is_valid("清华大学"));
    assert!(!receipt_title_is_valid("别的单位"));
}
