//! Loopback fixtures and shape tests for the course-reserve catalogue slice.

use std::time::Duration;

use reqwest::Url;

use crate::info_session::{InfoSessionAdapter, InfoWebVpnConfig};
use crate::reference_test_support::{FixtureServer, Reply};
use crate::reserves_read::*;
use crate::transport::CampusHttpTransport;

/// The mapping root the runtime installs for this service.
const MAPPING: &str = "/http/77726476706e69737468656265737421e2f2529935266d43300480aed641303c455d43259619a3eaf6eebb99";

fn adapter(server: &FixtureServer) -> ReservesAdapter {
    ReservesAdapter::try_with_transport(
        mapped_url(server, ""),
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .unwrap()
}

/// Builds a URL inside this service's WebVPN mapping on the fixture origin.
fn mapped_url(server: &FixtureServer, suffix: &str) -> Url {
    let mut url = Url::parse(server.base()).unwrap();
    url.set_path(&format!("{MAPPING}{suffix}"));
    url
}

/// One result block in the shape the service returns.
fn result_block(book_id: &str, title: &str) -> String {
    format!(
        r#"<div class="p-fbox">
             <a href="/Search/BookDetail?bookId={book_id}"><img src="/res/cover/{book_id}.jpg"></a>
             <strong>{title}</strong>
             <p>责任者：张三</p>
             <p>出版项：清华大学出版社&nbsp;2020</p>
             <p>ISBN：978-7-302-00000-1</p>
           </div>"#
    )
}

/// A search page holding two matches.
fn search_body() -> String {
    format!(
        r#"<html><body>
             <div class="s-list"><div class="btns">共 2 条结果,1 页</div></div>
             {}
             {}
           </body></html>"#,
        result_block("BK001", "高等数学"),
        result_block("BK002", "线性代数")
    )
}

/// A search page the service printed for a query that matched nothing.
fn empty_search_body() -> String {
    r#"<html><body>
         <div class="s-list"><div class="btns">共 0 条结果,0 页</div></div>
       </body></html>"#
        .to_owned()
}

/// One detail field row: a label cell and a value cell, the way the page prints
/// them.
fn detail_row(label: &str, value: &str) -> String {
    format!(r#"<tr><td><font>{label}</font></td><td><font><b>{value}</b></font></td></tr>"#)
}

/// A detail page for one book, with two chapter links.
fn detail_body() -> String {
    format!(
        r#"<html><body>
             <div class="p-result">
               <img src="/res/cover/BK001.jpg">
               <p>共 2 章</p>
               <p><a href="/chapter/1.pdf">第一章 极限</a><a href="/chapter/2.pdf">第二章 导数</a></p>
             </div>
             <table><tbody>
               {}
               {}
               {}
               {}
               {}
               {}
             </tbody></table>
           </body></html>"#,
        detail_row("书名", "高等数学"),
        detail_row("著者", "张三"),
        detail_row("出版社", "清华大学出版社"),
        detail_row("ISBN", "978-7-302-00000-1"),
        detail_row("版次", "第7版"),
        detail_row("卷册", "上册")
    )
}

#[test]
fn result_rows_are_read_from_the_exact_response_shape() {
    let parsed = parse_reserves_search_html(&search_body()).expect("search parses");
    assert_eq!(parsed.total, 2);
    assert_eq!(parsed.page_count, 1);
    assert_eq!(parsed.rows.len(), 2);
    assert_eq!(parsed.rows[0].title, "高等数学");
    assert_eq!(parsed.rows[0].author, "张三");
    assert_eq!(parsed.rows[0].publisher, "清华大学出版社 2020");
    assert_eq!(parsed.rows[0].isbn, "978-7-302-00000-1");
    assert_eq!(parsed.rows[0].book_id(), "BK001");
    assert_eq!(parsed.rows[1].title, "线性代数");
    assert_eq!(parsed.rows[1].book_id(), "BK002");
}

#[test]
fn a_printed_asset_path_is_rewritten_onto_the_mapping_origin() {
    let parsed = parse_reserves_search_html(&search_body()).expect("search parses");
    assert_eq!(
        parsed.rows[0].image_url,
        "https://webvpn.tsinghua.edu.cn/res/cover/BK001.jpg"
    );
}

#[test]
fn an_already_absolute_mapping_asset_is_not_prefixed_twice() {
    let body = search_body().replace(
        r#"src="/res/cover/BK001.jpg""#,
        r#"src="https://webvpn.tsinghua.edu.cn/res/cover/BK001.jpg""#,
    );
    let parsed = parse_reserves_search_html(&body).expect("search parses");
    assert_eq!(
        parsed.rows[0].image_url,
        "https://webvpn.tsinghua.edu.cn/res/cover/BK001.jpg"
    );
}

#[test]
fn an_asset_on_another_host_is_a_failure_not_a_followed_link() {
    let body = search_body().replace(
        r#"src="/res/cover/BK001.jpg""#,
        r#"src="https://elsewhere.invalid/tracker.gif""#,
    );
    assert_eq!(
        parse_reserves_search_html(&body).unwrap_err(),
        ReservesParseError::ForeignLink { row: 0 }
    );
}

#[test]
fn a_network_path_asset_is_refused_rather_than_rewritten() {
    let body = search_body().replace(
        r#"src="/res/cover/BK001.jpg""#,
        r#"src="//elsewhere.invalid/tracker.gif""#,
    );
    assert_eq!(
        parse_reserves_search_html(&body).unwrap_err(),
        ReservesParseError::ForeignLink { row: 0 }
    );
}

#[test]
fn an_empty_catalogue_is_reported_only_when_the_service_printed_a_zero_count() {
    let parsed = parse_reserves_search_html(&empty_search_body()).expect("empty search parses");
    assert!(parsed.rows.is_empty());
    assert_eq!(parsed.total, 0);
    assert_eq!(parsed.page_count, 0);
}

#[test]
fn a_missing_result_counter_is_a_failure_not_an_empty_page() {
    // The reference answers this shape from a built-in mock, which is
    // indistinguishable from a page that changed.  Here it is an error.
    let body = r#"<html><body><div class="s-list"></div></body></html>"#;
    assert_eq!(
        parse_reserves_search_html(body).unwrap_err(),
        ReservesParseError::MissingCount
    );
}

#[test]
fn a_page_with_no_results_but_a_nonzero_count_is_a_failure() {
    let body = r#"<html><body>
                    <div class="s-list"><div class="btns">共 7 条结果,1 页</div></div>
                  </body></html>"#;
    assert_eq!(
        parse_reserves_search_html(body).unwrap_err(),
        ReservesParseError::MissingResults
    );
}

#[test]
fn a_record_missing_a_printed_field_is_a_failure_not_a_blank_record() {
    let body = search_body().replace("<p>ISBN：978-7-302-00000-1</p>", "");
    assert_eq!(
        parse_reserves_search_html(&body).unwrap_err(),
        ReservesParseError::MissingField { row: 0 }
    );
}

#[test]
fn a_record_without_a_book_identifier_is_a_failure() {
    let body = search_body().replace("?bookId=BK001", "");
    assert_eq!(
        parse_reserves_search_html(&body).unwrap_err(),
        ReservesParseError::MissingBookId { row: 0 }
    );
}

#[test]
fn a_record_without_a_title_is_a_failure() {
    let body = search_body().replace("<strong>高等数学</strong>", "<strong></strong>");
    assert_eq!(
        parse_reserves_search_html(&body).unwrap_err(),
        ReservesParseError::MissingTitle { row: 0 }
    );
}

#[test]
fn a_webvpn_portal_page_is_a_login_failure() {
    let body = "<html><head><title>清华大学WebVPN</title></head><body></body></html>";
    let error = parse_reserves_search_html(body).unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error, ReservesParseError::LoginPage);
}

#[test]
fn a_timed_out_page_is_an_expiry_failure() {
    let body = "<html><body>用户登陆超时或访问内容不存在</body></html>";
    assert_eq!(
        parse_reserves_search_html(body).unwrap_err(),
        ReservesParseError::ExpiredPage
    );
}

#[test]
fn the_services_own_not_signed_in_notice_is_a_login_failure() {
    let body = format!("<html><body><p>{RESERVES_LOGIN_MARKER}</p></body></html>");
    let error = parse_reserves_search_html(&body).unwrap_err();
    assert!(error.is_session_expired());
}

#[test]
fn an_empty_body_is_a_failure() {
    assert_eq!(
        parse_reserves_search_html("   ").unwrap_err(),
        ReservesParseError::EmptyBody
    );
}

#[test]
fn detail_fields_are_read_from_the_rows_the_page_prints() {
    let parsed = parse_reserves_detail_html(&detail_body()).expect("detail parses");
    assert_eq!(parsed.title, "高等数学");
    assert_eq!(parsed.author, "张三");
    assert_eq!(parsed.publisher, "清华大学出版社");
    assert_eq!(parsed.isbn, "978-7-302-00000-1");
    assert_eq!(parsed.version, "第7版");
    assert_eq!(parsed.volume, "上册");
    assert_eq!(
        parsed.image_url,
        "https://webvpn.tsinghua.edu.cn/res/cover/BK001.jpg"
    );
    assert_eq!(parsed.chapters.len(), 2);
    assert_eq!(parsed.chapters[0].title, "第一章 极限");
    assert_eq!(
        parsed.chapters[0].url,
        "https://webvpn.tsinghua.edu.cn/chapter/1.pdf"
    );
    assert_eq!(parsed.chapters[1].title, "第二章 导数");
}

#[test]
fn a_detail_page_missing_a_row_is_a_failure() {
    let body = detail_body().replace(&detail_row("卷册", "上册"), "");
    assert_eq!(
        parse_reserves_detail_html(&body).unwrap_err(),
        ReservesParseError::MissingDetailField
    );
}

#[test]
fn a_detail_page_whose_labels_moved_still_reads_the_service_layout() {
    // A page that printed only part of the labels must not have one field
    // shifted onto another row; the layout order is used instead.
    let body = detail_body()
        .replace("版次", "版本说明")
        .replace("卷册", "分册说明");
    let parsed = parse_reserves_detail_html(&body).expect("detail parses");
    assert_eq!(parsed.title, "高等数学");
    assert_eq!(parsed.version, "第7版");
    assert_eq!(parsed.volume, "上册");
}

#[test]
fn a_detail_page_behind_the_not_signed_in_notice_is_a_login_failure() {
    let body = format!("<html><body>{RESERVES_LOGIN_MARKER}</body></html>");
    let error = parse_reserves_detail_html(&body).unwrap_err();
    assert!(error.is_session_expired());
}

#[test]
fn book_names_use_the_services_private_escape_scheme() {
    assert_eq!(encode_book_name("math 101").unwrap(), "math 101");
    assert_eq!(
        encode_book_name("高等数学").unwrap(),
        "%u9AD8%u7B49%u6570%u5B66"
    );
    // A character outside the BMP is sent as its two surrogate escapes, which
    // is what this service's decoder was built for.
    assert_eq!(encode_book_name("\u{1D11E}").unwrap(), "%uD834%uDD1E");
}

#[test]
fn unusable_book_names_are_refused_before_any_request() {
    for name in ["", "   ", "a&b", "a=b", "a#b", "50%", "a+b", "a?b"] {
        assert!(
            matches!(
                encode_book_name(name),
                Err(ReservesAdapterError::InvalidSearchText)
            ),
            "{name:?} should be refused"
        );
    }
    assert!(matches!(
        encode_book_name(&"x".repeat(65)),
        Err(ReservesAdapterError::InvalidSearchText)
    ));
}

#[tokio::test]
async fn the_adapter_reads_the_catalogue_through_the_cookie_aware_transport() {
    let server = FixtureServer::new(vec![Reply::html(&search_body())]);
    let read = adapter(&server)
        .search_with_proof("高等数学", 1)
        .await
        .expect("search succeeds");
    assert_eq!(read.value.total, 2);
    assert_eq!(read.value.page, 1);
    assert_eq!(read.value.len(), 2);
    assert_eq!(read.value.books[0].title, "高等数学");
    assert_eq!(read.value.books[0].reference.index(), 0);
    assert_eq!(read.value.books[1].reference.index(), 1);
    assert_eq!(read.proof.operation, ReservesOperation::Search);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET "));
    assert!(
        requests[0].contains(&format!("{MAPPING}{RESERVES_SEARCH_PATH}")),
        "request path should be the mapped search endpoint: {}",
        requests[0]
    );
    // The private escape must survive the URL layer byte for byte: a
    // re-encoding step would send `%25u9AD8…` and ask the service to decode
    // text it never encoded.
    assert!(
        requests[0].contains("?bookName=%u9AD8%u7B49%u6570%u5B66"),
        "query should carry the service's own escape form: {}",
        requests[0]
    );
    // The reference's first page carries no `page` parameter at all.
    assert!(
        !requests[0].contains("page="),
        "the first page must not add a page parameter: {}",
        requests[0]
    );
}

#[tokio::test]
async fn a_later_page_is_spelled_the_way_the_reference_spells_it() {
    let server = FixtureServer::new(vec![Reply::html(&search_body())]);
    let read = adapter(&server)
        .search("math", 3)
        .await
        .expect("search succeeds");
    assert_eq!(read.page, 3);

    let requests = server.requests();
    assert!(
        requests[0].contains("?bookName=math&page=3"),
        "request should carry the page parameter: {}",
        requests[0]
    );
}

#[tokio::test]
async fn the_detail_read_uses_the_reference_kept_inside_the_adapter() {
    let server = FixtureServer::new(vec![
        Reply::html(&search_body()),
        Reply::html(&detail_body()),
    ]);
    let adapter = adapter(&server);
    let search = adapter
        .search("高等数学", 1)
        .await
        .expect("search succeeds");
    let detail = adapter
        .detail(&search.books[0].reference)
        .await
        .expect("detail succeeds");
    assert_eq!(detail.title, "高等数学");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    // The service's own identifier is what reaches the wire; it never appears
    // in a caller-visible value.
    assert!(
        requests[1].contains(&format!("{RESERVES_DETAIL_PATH}?bookId=BK001")),
        "detail path should be the mapped endpoint: {}",
        requests[1]
    );
}

#[tokio::test]
async fn a_reference_only_resolves_inside_the_adapter_and_search_that_made_it() {
    let server = FixtureServer::new(vec![
        Reply::html(&search_body()),
        Reply::html(&search_body()),
    ]);
    let first = adapter(&server);
    let search = first.search("math", 1).await.expect("search succeeds");
    let reference = search.books[0].reference.clone();
    assert!(first.book_id(&reference).is_some());

    let other = adapter(&server);
    assert!(other.book_id(&reference).is_none());

    let superseded = first.search("math", 1).await.expect("search succeeds");
    assert!(first.book_id(&reference).is_none());
    assert!(first.book_id(&superseded.books[0].reference).is_some());
}

#[tokio::test]
async fn a_page_beyond_the_bound_or_a_bad_name_is_refused_before_any_request() {
    let server = FixtureServer::new(vec![]);
    let adapter = adapter(&server);
    for page in [0, MAX_RESERVES_PAGE + 1] {
        assert!(matches!(
            adapter.search("math", page).await.unwrap_err(),
            ReservesAdapterError::PageOutOfRange
        ));
    }
    assert!(matches!(
        adapter.search("a&b", 1).await.unwrap_err(),
        ReservesAdapterError::InvalidSearchText
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_login_answer_reaches_the_caller_as_a_session_failure() {
    let server = FixtureServer::new(vec![Reply::html(
        "<html><head><title>清华大学WebVPN</title></head><body></body></html>",
    )]);
    let error = adapter(&server).search("math", 1).await.unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "reserves_auth_required");
}

#[tokio::test]
async fn the_services_not_signed_in_notice_reaches_the_caller_as_a_session_failure() {
    let server = FixtureServer::new(vec![Reply::html(&format!(
        "<html><body>{RESERVES_LOGIN_MARKER}</body></html>"
    ))]);
    let error = adapter(&server).search("math", 1).await.unwrap_err();
    assert!(error.is_session_expired());
}

#[tokio::test]
async fn a_page_without_a_counter_is_reported_rather_than_an_empty_catalogue() {
    let server = FixtureServer::new(vec![Reply::html(
        "<html><body><div class=\"s-list\"></div></body></html>",
    )]);
    let error = adapter(&server).search("math", 1).await.unwrap_err();
    assert_eq!(error.diagnostic_code(), "reserves_count_missing");
}

#[tokio::test]
async fn a_non_html_answer_is_not_parsed_as_a_catalogue() {
    let server = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/pdf\r\n".into(),
        body: "%PDF-1.7".into(),
    }]);
    let error = adapter(&server).search("math", 1).await.unwrap_err();
    assert!(matches!(error, ReservesAdapterError::UnexpectedContentType));
}

#[tokio::test]
async fn a_server_error_is_reported_with_its_status() {
    let server = FixtureServer::new(vec![Reply {
        status: 500,
        headers: "Content-Type: text/html\r\n".into(),
        body: "<html><body>error</body></html>".into(),
    }]);
    let error = adapter(&server).search("math", 1).await.unwrap_err();
    assert!(matches!(error, ReservesAdapterError::HttpStatus { .. }));
}

#[test]
fn the_reference_recovery_payload_is_never_registered_as_a_roam_selector() {
    // The reference recovers this application by performing a campus identity
    // login.  This engine does not implement a second campus login, so the
    // payload must not be a selector the roaming allow list answers.  The
    // assertion is made against the allow list's own answer rather than a
    // hand-written copy of it: a copy keeps passing after an arm is added.
    let adapter = InfoSessionAdapter::new(
        InfoWebVpnConfig::new("https://vpn.fixture.invalid/", "/target").expect("config"),
        CampusHttpTransport::new("THYou/reserves-selector-fixture").unwrap(),
    )
    .unwrap();
    // A raw campus target is exactly what a registered selector would turn
    // into this module's mapping; refusing it is what "not registered" means.
    let raw = crate::info::OpaqueUrl::new(
        "http://reserves.lib.tsinghua.edu.cn/Search/ResBooks?bookName=math",
    )
    .unwrap();
    assert!(
        adapter
            .map_additional_roaming(RESERVES_WEBVPN_TARGET, &raw)
            .is_err(),
        "the identity-login payload must not be an answerable roam selector"
    );
    // The read is addressed at this module's own mapping constant instead, and
    // that mapping is what the adapter accepts directly.
    let mut base = Url::parse("https://vpn.fixture.invalid/").unwrap();
    base.set_path(&format!("{MAPPING}/"));
    ReservesAdapter::try_with_transport(
        base,
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap(),
    )
    .expect("the module's own mapping root configures the adapter");
}
