//! Loopback fixtures for the library booking, reservation-list and
//! cancellation paths.
//!
//! Every case asserts the dispatch count as well as the classification: a write
//! is dispatched exactly once, and an answer the service does not confirm must
//! not turn into a second request.  The fixture origin is loopback, so no real
//! deployment, account or credential is involved.

use reqwest::Url;

use crate::library_read::LibraryAdapterError;
use crate::library_write::{
    LibraryWriteAdapter, LibraryWriteOutcome, LibraryWriteParseError, parse_booking_records,
};
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

/// The bound account's own login id.  It is what the Runtime derives from a
/// proven identity; here it is only a fixture value, and no case ever asserts a
/// real one.
const USER_ID: &str = "2020091118";

fn adapter(server: &FixtureServer) -> LibraryWriteAdapter {
    LibraryWriteAdapter::try_with_transport(
        Url::parse(server.base()).unwrap(),
        CampusHttpTransport::new("THYou/library-write").unwrap(),
    )
    .unwrap()
}

/// A reservation row with the observed 16 columns and a cancellation control.
fn booking_page_row(position: &str, time: &str, status: &str, action: &str) -> String {
    let filler = "<td></td>".repeat(5);
    format!(
        "<!DOCTYPE html><html><body><table><tbody><tr>{filler}<td>{position}</td><td></td><td>{time}</td><td></td><td></td><td></td><td>{status}</td><td></td><td></td><td></td><td>{action}</td></tr></tbody></table></body></html>"
    )
}

fn home_page() -> Reply {
    Reply::html("<html><body><script>var cfg = {access_token: \"fa-9b7c\"};</script></body></html>")
}

#[tokio::test]
async fn backend_repair_library_booking_records_dispatch_one_read() {
    let server = FixtureServer::new(vec![Reply::html(&booking_page_row(
        "文科图书馆-四层-C区:F4C083",
        "2020-09-11 12:15:52",
        "已使用",
        "<a onclick=\"menuDel('202009111837')\">取消预约</a>",
    ))]);
    let records = adapter(&server).read_booking_records().await.unwrap();
    assert_eq!(records.records.len(), 1);
    assert_eq!(records.records[0].position, "文科图书馆-四层-C区:F4C083");
    assert_eq!(records.records[0].time, "2020-09-11 12:15:52");
    assert_eq!(records.records[0].status, "已使用");
    assert_eq!(
        records.records[0].cancellation_id.as_deref(),
        Some("202009111837")
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /user/index/book HTTP/1.1"));
    // The DTO carries the four display fields and a flag; the identifier the
    // cancellation route needs stays in the engine.
    let rendered = format!("{records:?}");
    assert!(!rendered.contains("202009111837"));
    assert!(rendered.contains("cancellable: true"));
}

#[tokio::test]
async fn backend_repair_library_records_keep_empty_distinct_from_missing() {
    let empty = FixtureServer::new(vec![Reply::html(
        "<!DOCTYPE html><html><body><table><tbody></tbody></table></body></html>",
    )]);
    assert!(
        adapter(&empty)
            .read_booking_records()
            .await
            .unwrap()
            .records
            .is_empty()
    );

    let replaced = FixtureServer::new(vec![Reply::html(
        "<!DOCTYPE html><html><body><p>session ended</p></body></html>",
    )]);
    let error = adapter(&replaced).read_booking_records().await.unwrap_err();
    assert!(matches!(
        error,
        LibraryAdapterError::WriteParse(LibraryWriteParseError::MissingTable)
    ));
    assert_eq!(error.diagnostic_code(), "library_booking_records");
    assert_eq!(replaced.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_library_booking_reads_the_token_then_dispatches_one_form() {
    let server = FixtureServer::new(vec![
        home_page(),
        Reply::json("{\"status\":1,\"msg\":\"success\"}"),
    ]);
    let adapter = adapter(&server);
    let plan = adapter.book_seat_request(701, 9001, 2).unwrap();
    let outcome = adapter.book_seat(&plan, USER_ID).await.unwrap();
    assert_eq!(outcome, LibraryWriteOutcome::Accepted);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /home/web/f_second HTTP/1.1"));
    assert!(requests[1].starts_with("POST /api.php/spaces/701/book HTTP/1.1"));
    // The token and the derived account id travel in the form, which is the only
    // place either of them belongs, and the plan they came from prints neither.
    assert!(requests[1].contains("access_token=fa-9b7c"));
    assert!(requests[1].contains(&format!("userid={USER_ID}")));
    assert!(requests[1].contains("segment=9001"));
    assert!(requests[1].contains("type=2"));
    assert!(requests[1].contains("operateChannel=2"));
    let rendered = format!("{plan:?}");
    assert!(!rendered.contains("fa-9b7c"));
    assert!(!rendered.contains(USER_ID));
}

#[tokio::test]
async fn backend_repair_library_booking_refusal_is_dispatched_once() {
    let server = FixtureServer::new(vec![
        home_page(),
        Reply::json("{\"status\":0,\"msg\":\"座位已被预约\"}"),
    ]);
    let adapter = adapter(&server);
    let plan = adapter.book_seat_request(701, 9001, 2).unwrap();
    assert_eq!(
        adapter.book_seat(&plan, USER_ID).await.unwrap(),
        LibraryWriteOutcome::Refused
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_library_booking_unreadable_answer_is_never_replayed() {
    let server = FixtureServer::new(vec![
        home_page(),
        Reply::html("<!DOCTYPE html><html><body>not the booking answer</body></html>"),
    ]);
    let adapter = adapter(&server);
    let plan = adapter.book_seat_request(701, 9001, 2).unwrap();
    assert_eq!(
        adapter.book_seat(&plan, USER_ID).await.unwrap(),
        LibraryWriteOutcome::Unrecognized
    );
    // Exactly the token read and the one write: an unknown outcome is not retried.
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_library_booking_session_expiry_is_reported_as_login() {
    let server = FixtureServer::new(vec![
        home_page(),
        Reply::html("<!DOCTYPE html><html><body>用户登陆超时或访问内容不存在</body></html>"),
    ]);
    let adapter = adapter(&server);
    let plan = adapter.book_seat_request(701, 9001, 2).unwrap();
    assert_eq!(
        adapter.book_seat(&plan, USER_ID).await.unwrap(),
        LibraryWriteOutcome::LoginRequired
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_library_cancel_dispatches_one_delete_form() {
    let server = FixtureServer::new(vec![
        home_page(),
        Reply::json("{\"status\":1,\"msg\":\"success\"}"),
    ]);
    let adapter = adapter(&server);
    let plan = adapter.cancel_booking_request("202009111837").unwrap();
    assert_eq!(
        adapter.cancel_booking(&plan, USER_ID).await.unwrap(),
        LibraryWriteOutcome::Accepted
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("POST /api.php/profile/books/202009111837 HTTP/1.1"));
    assert!(requests[1].contains("_method=delete"));
    assert!(requests[1].contains("id=202009111837"));
    assert!(requests[1].contains("access_token=fa-9b7c"));
    assert!(requests[1].contains(&format!("userid={USER_ID}")));
}

#[tokio::test]
async fn backend_repair_library_write_refuses_a_non_student_account_before_any_request() {
    let server = FixtureServer::new(vec![home_page()]);
    let adapter = adapter(&server);
    let plan = adapter.book_seat_request(701, 9001, 2).unwrap();
    // A username that is not an all-digit student id is refused locally, so no
    // token read and no write are dispatched at all.
    assert!(matches!(
        adapter.book_seat(&plan, "fixture-user").await,
        Err(LibraryAdapterError::WriteOperation)
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_library_write_rejects_a_mismatched_plan() {
    let server = FixtureServer::new(vec![home_page()]);
    let adapter = adapter(&server);
    let booking = adapter.book_seat_request(701, 9001, 2).unwrap();
    // A booking plan can never be dispatched through the cancellation entry point
    // and the other way round.
    assert!(matches!(
        adapter.cancel_booking(&booking, USER_ID).await,
        Err(LibraryAdapterError::WriteOperation)
    ));
    let cancel = adapter.cancel_booking_request("202009111837").unwrap();
    assert!(matches!(
        adapter.book_seat(&cancel, USER_ID).await,
        Err(LibraryAdapterError::WriteOperation)
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_library_write_requires_a_token_the_plan_cannot_carry() {
    // The home page carries no token: the write must refuse rather than send an
    // empty one, and it must not have dispatched anything else.
    let server = FixtureServer::new(vec![Reply::html(
        "<!DOCTYPE html><html><body><p>welcome</p></body></html>",
    )]);
    let adapter = adapter(&server);
    let plan = adapter.book_seat_request(701, 9001, 2).unwrap();
    let error = adapter.book_seat(&plan, USER_ID).await.unwrap_err();
    assert!(matches!(
        error,
        LibraryAdapterError::WriteParse(LibraryWriteParseError::MissingAccessToken)
    ));
    assert_eq!(error.diagnostic_code(), "library_booking_token");
    assert!(!format!("{error}").contains("access_token"));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn backend_repair_library_records_reject_a_shifted_column_layout() {
    // The observed layout has 16 columns.  A page built with a different count
    // must fail rather than report neighbouring columns as the reservation.
    let body = "<!DOCTYPE html><html><body><table><tbody><tr><td>文科图书馆-四层-C区:F4C083</td><td>2020-09-11 12:15:52</td><td>已使用</td></tr></tbody></table></body></html>";
    assert!(matches!(
        parse_booking_records(body),
        Err(LibraryWriteParseError::InvalidRecord { index: 0, .. })
    ));
}
