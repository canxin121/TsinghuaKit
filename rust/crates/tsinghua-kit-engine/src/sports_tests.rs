//! Loopback fixtures and shape tests for the sports-venue read slice.

use std::time::Duration;

use crate::reference_test_support::{FixtureServer, Reply};
use crate::sports_read::*;
use crate::transport::CampusHttpTransport;

use reqwest::{StatusCode, Url};

/// The two limits the venue page writes into its inline script.
fn limits_page(count: &str, init: &str) -> String {
    format!(
        r#"<html><head><script>
var limitBookCount = '{count}';
var limitBookInit = '{init}';
</script></head><body></body></html>"#
    )
}

/// One slot-detail page: three slots, one of which has no matching hash
/// statement, plus one whose hash statement names a different slot.
fn detail_page() -> String {
    r#"<html><body><script>
resourceArray.push({id:'14567229',time_session:'20:00-21:00',field_name:'台1',overlaySize:'2',can_net_book:'1'});
resourcesm.put('14567229', 'F3681513C5BC25CEDDF5FA7C3E8429F1769DBA48B7BD42CC');
addCost('14567229','0');
markResStatus('', '14567229', '0');
markStatusColor('14567229','','0','');
resourceArray.push({id:'14567231',time_session:'19:00-20:00',field_name:'台1',overlaySize:'2',can_net_book:'1'});
resourcesm.put('14567231', '85998D74214C74FC2D404EB53A976C866A206F2954865477');
addCost('14567231','25');
markResStatus('15674139', '14567231', '1');
markStatusColor('14567231','学生','1','');
resourceArray.push({id:'14567233',time_session:'18:00-19:00',field_name:'台1',overlaySize:'2',can_net_book:'1'});
resourcesm.put('99999999', 'DEADBEEFDEADBEEFDEADBEEFDEADBEEFDEADBEEFDEADBEEF');
resourceArray.push({id:'14567235',time_session:'17:00-18:00',field_name:'台1',overlaySize:'2',can_net_book:'0'});
</script></body></html>"#
        .to_owned()
}

/// The unpaid reservation table, with two rows and two different methods.
fn unpaid_page() -> String {
    r#"<html><body>
<table><tbody>
<tr>
<td>1</td>
<td>气膜馆羽毛球场</td>
<td>--</td>
<td>1号场</td>
<td>--</td>
<td>20:00-21:00</td>
<td>--</td>
<td>25</td>
<td>--</td>
<td>网上支付</td>
<td>--</td>
<td><span time="1695000000"></span><button onclick="payNow('PAY-1')"></button><button onclick="unsubscribeOnline('BOOK-1')"></button></td>
</tr>
<tr>
<td>2</td>
<td>综体篮球场</td>
<td>--</td>
<td>2号场</td>
<td>--</td>
<td>18:00-19:00</td>
<td>--</td>
<td>0</td>
<td>--</td>
<td>现场支付</td>
<td>--</td>
<td><span time="1695000001"></span><button onclick="unsubscribe('BOOK-2')"></button></td>
</tr>
</tbody></table>
</body></html>"#
        .to_owned()
}

/// The paid reservation table: one hidden carrier holding a nested row.
fn paid_page() -> String {
    r#"<html><body>
<table><tbody>
<tr style="display:none">
<td colspan="8">
<table><tbody>
<tr>
<td>1</td><td>--</td>
<td>气膜馆羽毛球场</td>
<td>3号场</td>
<td>21:00-22:00</td>
<td>25</td>
<td>--</td><td>--</td>
</tr>
</tbody></table>
</td>
</tr>
</tbody></table>
</body></html>"#
        .to_owned()
}

fn adapter(server: &FixtureServer) -> SportsAdapter {
    let base = Url::parse(server.base()).expect("fixture base");
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10))
        .expect("transport");
    SportsAdapter::try_with_transport(base, transport).expect("adapter")
}

#[tokio::test]
async fn a_resource_read_reports_limits_phone_and_slots() {
    let server = FixtureServer::new(vec![
        Reply::html(&limits_page("5", "2")),
        Reply::html(&detail_page()),
        Reply::html("13800138000"),
    ]);
    let adapter = adapter(&server);
    let resources = adapter
        .read_resources("3998000", "4045681", "2026-09-29")
        .await
        .expect("resources");
    assert_eq!(resources.count, 5);
    assert_eq!(resources.init, 2);
    assert_eq!(resources.phone.as_deref(), Some("13800138000"));
    // Two slots carry a hash statement naming them; the third names another
    // slot and the fourth carries none, so neither is reported.
    assert_eq!(resources.data.len(), 2);
    let first = &resources.data[0];
    assert_eq!(first.res_id, "14567229");
    assert_eq!(first.time_session, "20:00-21:00");
    assert_eq!(first.field_name, "台1");
    assert_eq!(first.overlay_size, Some(2));
    assert!(first.can_net_book);
    assert_eq!(first.cost.as_deref(), Some("0"));
    assert_eq!(first.book_id.as_deref(), Some(""));
    assert_eq!(first.locked, Some(false));
    assert_eq!(first.payment_status, Some(false));
    let second = &resources.data[1];
    assert_eq!(second.book_id.as_deref(), Some("15674139"));
    assert_eq!(second.locked, Some(true));
    assert_eq!(second.user_type.as_deref(), Some("学生"));
    assert_eq!(second.payment_status, Some(true));
    assert_eq!(second.cost.as_deref(), Some("25"));
    // The booking hash is a single-purpose token, so Debug never prints it.
    let rendered = format!("{resources:?}");
    assert!(!rendered.contains("F3681513C5BC25CEDDF5FA7C3E8429F1769DBA48B7BD42CC"));
    assert!(!rendered.contains("13800138000"));
    assert!(rendered.contains("phone_present"));
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("GET /gymbook/gymBookAction.do?ms=viewGymBook&viewType=m&gymnasium_id=3998000&item_id=4045681&time_date=2026-09-29 "));
    assert!(requests[1].starts_with("GET /gymsite/cacheAction.do?ms=viewBook&userType=1&"));
    assert!(requests[2].starts_with("GET /gymbook/gymBookAction.do?ms=hadContactOrNot "));
}

#[tokio::test]
async fn a_records_read_reports_both_tables() {
    let server = FixtureServer::new(vec![Reply::html(&unpaid_page()), Reply::html(&paid_page())]);
    let adapter = adapter(&server);
    let records = adapter.read_records().await.expect("records");
    assert_eq!(records.len(), 3);
    let online = &records[0];
    assert_eq!(online.name, "气膜馆羽毛球场");
    assert_eq!(online.field, "1号场");
    assert_eq!(online.time, "20:00-21:00");
    assert_eq!(online.price, "25");
    assert_eq!(online.method, "网上支付");
    assert_eq!(online.book_timestamp, Some(1_695_000_000));
    assert_eq!(online.book_id.as_deref(), Some("BOOK-1"));
    assert_eq!(online.pay_id.as_deref(), Some("PAY-1"));
    let onsite = &records[1];
    assert_eq!(onsite.method, "现场支付");
    assert_eq!(onsite.book_id.as_deref(), Some("BOOK-2"));
    // An on-site row offers no payment, so it carries no payment identifier.
    assert_eq!(onsite.pay_id, None);
    assert_eq!(onsite.book_timestamp, Some(1_695_000_001));
    let paid = &records[2];
    assert_eq!(paid.name, "气膜馆羽毛球场");
    assert_eq!(paid.field, "3号场");
    assert_eq!(paid.time, "21:00-22:00");
    assert_eq!(paid.price, "25");
    assert_eq!(paid.method, PAID_METHOD);
    assert_eq!(paid.book_timestamp, None);
    assert_eq!(paid.book_id, None);
    assert_eq!(paid.pay_id, None);
    let rendered = format!("{records:?}");
    assert!(!rendered.contains("BOOK-1"));
    assert!(!rendered.contains("PAY-1"));
    assert!(rendered.contains("has_book_id"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /pay/payAction.do?ms=getOrdersForNopay "));
    assert!(requests[1].starts_with("GET /pay/payAction.do?ms=getOrdersForUnpay "));
}

#[tokio::test]
async fn a_configured_phone_absence_is_a_validated_absence() {
    let server = FixtureServer::new(vec![Reply::html(&limits_page("1", "0"))]);
    let adapter = adapter(&server);
    assert_eq!(parse_sports_phone_body("do_not").expect("phone"), None);
    // Anything that is not the service's own no-phone flag and is not a phone
    // token is an error, never a silent "no phone configured".
    for rejected in [
        "",
        "   ",
        "<html><body>系统维护中</body></html>",
        "1380013800x",
    ] {
        assert!(
            parse_sports_phone_body(rejected).is_err(),
            "{rejected} must not read as absent"
        );
    }
    assert_eq!(
        parse_sports_phone_body(" 13800138000 ").expect("phone"),
        Some("13800138000".to_owned())
    );
    let _ = adapter;
}

#[test]
fn a_slot_whose_hash_statement_misses_it_is_not_given_a_neighbour() {
    let resources = parse_sports_resources_html(&detail_page(), "3998000", "4045681", "2024-09-20")
        .expect("resources");
    assert_eq!(resources.len(), 2);
    assert!(
        resources
            .iter()
            .all(|slot| slot.res_hash.starts_with(|c: char| c.is_ascii_hexdigit()))
    );
    assert!(
        resources
            .iter()
            .all(|slot| !slot.res_hash.contains("DEADBEEF"))
    );
}

#[test]
fn an_unreadable_slot_entry_is_an_error_not_a_shorter_list() {
    let page = r#"<html><body><script>
resourceArray.push({time_session:'20:00-21:00',field_name:'台1'});
</script></body></html>"#;
    assert!(matches!(
        parse_sports_resources_html(page, "3998000", "4045681", "2024-09-20"),
        Err(SportsParseError::UnrecognizedSlot { index: 0 })
    ));
}

#[test]
fn a_page_without_limits_is_not_an_empty_venue() {
    assert!(matches!(
        parse_sports_limits_html("<html><body>no script here</body></html>"),
        Err(SportsParseError::MissingLimit)
    ));
    assert!(matches!(
        parse_sports_limits_html(&limits_page("5", "not-a-number")),
        Err(SportsParseError::MissingLimit)
    ));
    assert_eq!(
        parse_sports_limits_html(&limits_page("0", "3")).expect("limits"),
        SportsLimits { count: 0, init: 3 }
    );
}

#[test]
fn an_ambiguous_or_absent_method_landmark_is_unrecognized() {
    // Two method cells: choosing one would shift every value on the row.
    let ambiguous = r#"<html><body><table><tbody><tr>
<td>1</td><td>a</td><td>--</td><td>b</td><td>--</td><td>20:00-21:00</td><td>--</td><td>25</td><td>--</td>
<td>网上支付</td><td>现场支付</td><td>x</td>
</tr></tbody></table></body></html>"#;
    assert!(matches!(
        parse_sports_unpaid_records_html(ambiguous),
        Err(SportsParseError::UnrecognizedRow { row: 0 })
    ));
    // No method cell at all.
    let landmarkless = r#"<html><body><table><tbody><tr>
<td>1</td><td>a</td><td>b</td>
</tr></tbody></table></body></html>"#;
    assert!(matches!(
        parse_sports_unpaid_records_html(landmarkless),
        Err(SportsParseError::UnrecognizedRow { row: 0 })
    ));
}

#[test]
fn a_response_without_a_table_is_never_an_empty_reservation_list() {
    let empty_page = "<html><body>暂无预约</body></html>";
    assert!(matches!(
        parse_sports_unpaid_records_html(empty_page),
        Err(SportsParseError::MissingTable)
    ));
    assert!(matches!(
        parse_sports_paid_records_html(empty_page),
        Err(SportsParseError::MissingTable)
    ));
    // A table with no rows really is an empty list.
    let empty_table = "<html><body><table><tbody></tbody></table></body></html>";
    assert!(
        parse_sports_unpaid_records_html(empty_table)
            .expect("unpaid")
            .is_empty()
    );
    assert!(
        parse_sports_paid_records_html(empty_table)
            .expect("paid")
            .is_empty()
    );
}

#[test]
fn a_paid_carrier_that_does_not_match_is_unrecognized() {
    // Hidden, but with no nested `tbody`, so the carrier is not the shape this
    // client reads.
    let broken = r#"<html><body><table><tbody>
<tr style="display:none"><td>nothing nested here</td></tr>
</tbody></table></body></html>"#;
    assert!(matches!(
        parse_sports_paid_records_html(broken),
        Err(SportsParseError::UnrecognizedRow { row: 0 })
    ));
}

#[test]
fn a_login_or_expired_page_is_a_session_failure() {
    for page in [
        "<html><head><title>清华大学WebVPN</title></head><body></body></html>",
        "<html><body>用户登陆超时或访问内容不存在</body></html>",
    ] {
        assert!(
            parse_sports_limits_html(page)
                .expect_err("must fail")
                .is_session_expired()
        );
        assert!(
            parse_sports_unpaid_records_html(page)
                .expect_err("must fail")
                .is_session_expired()
        );
        assert!(
            parse_sports_resources_html(page, "3998000", "4045681", "2024-09-20")
                .expect_err("must fail")
                .is_session_expired()
        );
        assert!(
            parse_sports_phone_body(page)
                .expect_err("must fail")
                .is_session_expired()
        );
    }
    assert!(matches!(
        parse_sports_limits_html("   "),
        Err(SportsParseError::EmptyBody)
    ));
}

#[tokio::test]
async fn a_refused_venue_value_never_reaches_the_network() {
    let server = FixtureServer::new(vec![]);
    let adapter = adapter(&server);
    for (gym, item, date) in [
        ("", "4045681", "2026-09-29"),
        ("3998000", "4045681x", "2026-09-29"),
        ("3998000", "4045681", "2026-13-29"),
        ("3998000", "4045681", "2026-9-29"),
        ("3998000", "4045681", "2026-02-30"),
        ("3998000", "4045681", ""),
        ("3998000", "4045681&x=1", "2026-09-29"),
    ] {
        assert!(
            matches!(
                adapter.read_resources(gym, item, date).await,
                Err(SportsAdapterError::InvalidInput)
            ),
            "{gym}/{item}/{date} must be refused"
        );
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_non_success_status_and_a_foreign_origin_are_never_data() {
    let server = FixtureServer::new(vec![Reply {
        status: 502,
        headers: "Content-Type: text/html\r\n".into(),
        body: limits_page("5", "2").into_bytes(),
    }]);
    let adapter = adapter(&server);
    assert!(matches!(
        adapter
            .read_resources("3998000", "4045681", "2026-09-29")
            .await,
        Err(SportsAdapterError::HttpStatus {
            status: StatusCode::BAD_GATEWAY
        })
    ));
}

#[tokio::test]
async fn a_redirect_off_the_mapping_is_refused() {
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: https://example.test/gymbook\r\n".into(),
        body: Vec::new(),
    }]);
    let adapter = adapter(&server);
    assert!(matches!(
        adapter.read_records().await,
        Err(SportsAdapterError::UnexpectedOrigin)
    ));
}

#[test]
fn a_base_url_is_never_accepted_with_credentials_or_a_foreign_shape() {
    for rejected in [
        "ftp://50.tsinghua.edu.cn/",
        "https://user:secret@50.tsinghua.edu.cn/",
        "https://50.tsinghua.edu.cn/?token=1",
        "https://50.tsinghua.edu.cn/#fragment",
        "not a url",
    ] {
        assert!(
            matches!(
                SportsAdapterConfig::new(rejected),
                Err(SportsAdapterError::InvalidBaseUrl)
            ),
            "{rejected} must be refused"
        );
    }
    let accepted = SportsAdapterConfig::new("http://50.tsinghua.edu.cn/gymbook/gymBookAction.do")
        .expect("config")
        .base_url()
        .clone();
    // A non-mapping deployment path is normalized to its own root, not
    // truncated onto one.
    assert_eq!(
        accepted.as_str(),
        "http://50.tsinghua.edu.cn/gymbook/gymBookAction.do/"
    );
    let mapped = SportsAdapterConfig::new(&format!(
        "https://webvpn.tsinghua.edu.cn/http/{SPORTS_MAPPING_TOKEN}/gymbook/x.do"
    ))
    .expect("config")
    .base_url()
    .clone();
    assert_eq!(mapped.path(), format!("/http/{SPORTS_MAPPING_TOKEN}/"));
}

#[test]
fn diagnostic_codes_are_distinct_per_failure_class() {
    let codes = [
        SportsAdapterError::InvalidBaseUrl.diagnostic_code(),
        SportsAdapterError::InvalidInput.diagnostic_code(),
        SportsAdapterError::UnexpectedOrigin.diagnostic_code(),
        SportsAdapterError::UnexpectedPath.diagnostic_code(),
        SportsAdapterError::SessionExpired.diagnostic_code(),
        SportsAdapterError::UnexpectedDeployment.diagnostic_code(),
        SportsAdapterError::Parse(SportsParseError::MissingTable).diagnostic_code(),
        SportsAdapterError::Parse(SportsParseError::UnrecognizedPhone).diagnostic_code(),
        SportsAdapterError::Parse(SportsParseError::TooLarge).diagnostic_code(),
    ];
    let mut unique = codes.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), codes.len());
    assert!(codes.iter().all(|code| code.starts_with("sports_")));
}

#[test]
fn debug_output_never_carries_a_token_or_a_credential() {
    let config = SportsAdapterConfig::new("http://50.tsinghua.edu.cn/").expect("config");
    let rendered = format!("{config:?}");
    assert!(rendered.contains("50.tsinghua.edu.cn"));
    assert!(!rendered.contains("token"));
    let transport = CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10))
        .expect("transport");
    let adapter = SportsAdapter::try_with_transport(
        Url::parse("http://50.tsinghua.edu.cn/").unwrap(),
        transport,
    )
    .expect("adapter");
    let rendered = format!("{adapter:?}");
    assert!(!rendered.contains("Cookie"));
    assert!(rendered.contains("cookie-aware transport"));
    assert_eq!(adapter.profile().roaming_selector(), SPORTS_WEBVPN_TARGET);
    assert_eq!(
        SportsOperation::ReadResources.as_str(),
        "read_resource_list"
    );
    assert_eq!(SportsOperation::ReadRecords.as_str(), "read_records");
}
