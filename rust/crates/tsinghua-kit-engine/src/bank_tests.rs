//! Loopback fixtures and shape tests for the bank payroll and graduate-income
//! read slices.

use std::time::Duration;

use reqwest::Url;

use crate::bank_read::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

/// The payroll mapping root the runtime installs for this service.
const BANK_MAPPING: &str =
    "/http/77726476706e69737468656265737421e9ff459a69247b59700f81b9991b26317dbd36ae";

/// The graduate-income mapping root.
const GRADUATE_MAPPING: &str = "/http/77726476706e69737468656265737421eaed4b9069377a517a1d88b89d1b37269c624d2b1c6925f37faea82b8d";

fn transport() -> CampusHttpTransport {
    CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap()
}

fn bank_adapter(server: &FixtureServer) -> BankPaymentAdapter {
    BankPaymentAdapter::try_with_transport(
        mapped_url(server, BANK_MAPPING),
        BankLedger::Main,
        transport(),
    )
    .unwrap()
}

fn graduate_adapter(server: &FixtureServer) -> GraduateIncomeAdapter {
    GraduateIncomeAdapter::try_with_transport(mapped_url(server, GRADUATE_MAPPING), transport())
        .unwrap()
}

fn mapped_url(server: &FixtureServer, mapping: &str) -> Url {
    let mut url = Url::parse(server.base()).unwrap();
    url.set_path(mapping);
    url
}

/// The year form the service answers with: the years this account has receipts
/// for, as an option set.
fn years_body() -> String {
    "<html><body><form><select name=\"year\">\
     <option value=\"2021\">2021年</option>\
     <option value=\"2020\">2020年</option>\
     </select></form></body></html>"
        .to_owned()
}

/// One payroll receipt row in the shape the service renders: eleven columns, the
/// four money columns nested one level deeper, and a totals row at the end that
/// carries no receipt columns.
fn receipt_table(month: &str, rows: &[(&str, &str, &str)]) -> String {
    let mut body = format!(
        "<div><strong>{month}银行代发结果</strong></div><table><tbody>\
         <tr><th>序</th><th>代发部门</th><th>代发项目</th><th>代发用途</th><th>代发说明</th>\
         <th>开户银行</th><th>计税时间</th><th>应发金额</th><th>扣税金额</th>\
         <th>实发金额</th><th>存折金额</th><th>现金金额</th></tr>"
    );
    for (usage, total, actual) in rows {
        body.push_str(&format!(
            "<tr><td>1</td><td>024 计算机系</td><td>1234567890 元宇宙项目开发</td><td>{usage}</td>\
             <td></td><td>中国银行</td><td>20771225 15:07:37</td>\
             <td><strong>{total}</strong></td><td><strong>0.00</strong></td>\
             <td><strong>{actual}</strong></td><td><strong>{actual}</strong></td>\
             <td><strong>0.00</strong></td></tr>"
        ));
    }
    body.push_str("<tr><td colspan=\"12\">合计</td></tr></tbody></table>");
    body
}

fn receipts_body() -> String {
    format!(
        "<html><body>{}{}</body></html>",
        receipt_table("2021年12月", &[("勤工俭学", "500.00", "500.00")]),
        receipt_table("2021年10月", &[("医疗费", "120.00", "120.00")])
    )
}

#[test]
fn the_year_form_offers_the_years_the_service_lists() {
    let years = parse_bank_years_html(&years_body()).expect("years parse");
    assert_eq!(years, vec!["2021".to_owned(), "2020".to_owned()]);
}

#[test]
fn a_year_form_without_options_is_a_failure_not_an_empty_result() {
    let empty = "<html><body><form><select name=\"year\"></select></form></body></html>";
    assert_eq!(
        parse_bank_years_html(empty).unwrap_err(),
        BankPaymentParseError::NoYears
    );
}

#[test]
fn receipts_are_located_by_their_header_labels() {
    let ledger = parse_bank_receipts_html(&receipts_body()).expect("receipts parse");
    assert_eq!(ledger.months.len(), 2);
    assert_eq!(ledger.months[0].month, "2021年12月");
    assert_eq!(ledger.months[1].month, "2021年10月");
    let receipt = &ledger.months[0].receipts[0];
    assert_eq!(receipt.department, "024 计算机系");
    assert_eq!(receipt.project, "1234567890 元宇宙项目开发");
    assert_eq!(receipt.usage, "勤工俭学");
    assert_eq!(receipt.description, "");
    assert_eq!(receipt.bank, "中国银行");
    assert_eq!(receipt.time, "20771225 15:07:37");
    assert_eq!(ledger.receipt_count(), 2);
    assert!(!ledger.is_empty());
}

#[test]
fn amounts_are_exact_integer_cents_and_the_totals_row_is_not_a_receipt() {
    let ledger = parse_bank_receipts_html(&receipts_body()).expect("receipts parse");
    let receipt = &ledger.months[0].receipts[0];
    assert_eq!(receipt.total_cents, Some(50_000));
    assert_eq!(receipt.deduction_cents, Some(0));
    assert_eq!(receipt.actual_cents, Some(50_000));
    assert_eq!(receipt.deposit_cents, Some(50_000));
    assert_eq!(receipt.cash_cents, Some(0));
    // The totals row has no receipt columns, so it is dropped rather than
    // reported as a receipt with empty money fields.
    assert_eq!(ledger.months[0].receipts.len(), 1);
}

#[test]
fn a_table_whose_columns_moved_is_read_by_its_labels_not_by_position() {
    // 代发部门 and 开户银行 swapped: the label set is still complete, so a
    // positional reader would report the bank as the department.  This module
    // locates each column by its label, so the values track the labels.
    let moved = "<html><body><div><strong>2021年12月银行代发结果</strong></div><table><tbody>\
        <tr><th>开户银行</th><th>代发项目</th><th>代发用途</th><th>代发说明</th>\
        <th>代发部门</th><th>计税时间</th><th>应发金额</th><th>扣税金额</th>\
        <th>实发金额</th><th>存折金额</th><th>现金金额</th></tr>\
        <tr><td>中国银行</td><td>项目</td><td>用途</td><td></td>\
        <td>024 计算机系</td><td>20771225 15:07:37</td>\
        <td>500.00</td><td>0.00</td><td>500.00</td><td>500.00</td><td>0.00</td></tr>\
        </tbody></table></body></html>";
    let ledger = parse_bank_receipts_html(moved).expect("receipts parse");
    let receipt = &ledger.months[0].receipts[0];
    assert_eq!(receipt.department, "024 计算机系");
    assert_eq!(receipt.bank, "中国银行");
}

#[test]
fn a_table_missing_one_of_the_expected_columns_is_not_a_receipt_table() {
    let missing = "<html><body><div><strong>2021年12月银行代发结果</strong></div><table><tbody>\
        <tr><th>代发部门</th><th>代发项目</th><th>代发用途</th><th>代发说明</th>\
        <th>开户银行</th><th>计税时间</th><th>应发金额</th><th>扣税金额</th>\
        <th>实发金额</th><th>存折金额</th></tr>\
        <tr><td>a</td><td>b</td><td>c</td><td></td><td>d</td><td>e</td>\
        <td>1.00</td><td>0.00</td><td>1.00</td><td>1.00</td></tr>\
        </tbody></table></body></html>";
    // The table is not a receipt table, so the document has a heading and no
    // table: a changed deployment rather than an empty ledger.
    assert_eq!(
        parse_bank_receipts_html(missing).unwrap_err(),
        BankPaymentParseError::SectionCountMismatch {
            headings: 1,
            tables: 0
        }
    );
}

#[test]
fn a_second_table_under_one_heading_is_a_section_mismatch() {
    let mismatched = format!(
        "<html><body>{}</body></html>",
        receipt_table("2021年12月", &[("勤工俭学", "500.00", "500.00")]).replace(
            "</tbody></table>",
            "</tbody></table><table><tbody><tr><th>代发部门</th><th>代发项目</th><th>代发用途</th>\
             <th>代发说明</th><th>开户银行</th><th>计税时间</th><th>应发金额</th><th>扣税金额</th>\
             <th>实发金额</th><th>存折金额</th><th>现金金额</th></tr></tbody></table>"
        )
    );
    assert_eq!(
        parse_bank_receipts_html(&mismatched).unwrap_err(),
        BankPaymentParseError::SectionCountMismatch {
            headings: 1,
            tables: 2
        }
    );
}

#[test]
fn an_amount_that_is_not_exact_is_refused_rather_than_rounded() {
    let body = receipt_table("2021年12月", &[("勤工俭学", "500.005", "500.00")]);
    let error = parse_bank_receipts_html(&body).unwrap_err();
    assert!(
        matches!(error, BankPaymentParseError::InvalidAmount { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn a_login_page_and_an_expiry_page_are_session_failures() {
    let login = "<html><body>net_Default_LoginCtrl1_txtUserName 请登录</body></html>";
    assert!(
        parse_bank_years_html(login)
            .unwrap_err()
            .is_session_expired()
    );
    let expired = "<html><body>time out用户登陆超时或访问内容不存在。请重试</body></html>";
    assert!(
        parse_bank_receipts_html(expired)
            .unwrap_err()
            .is_session_expired()
    );
}

#[test]
fn the_year_batch_body_is_re_encoded_from_the_service_values() {
    let profile = BankPaymentProfile::standard();
    let plan = profile.receipts_request(BankLedger::Main, &["2021".to_owned(), "2020".to_owned()]);
    assert_eq!(plan.form_body(), "year=2021&year=2020");
    assert_eq!(plan.path, BANK_SEARCH_PATH);
    // No caller text can reach the body: a value with a separator in it is
    // percent-encoded rather than spliced into the form.
    let hostile = profile.receipts_request(BankLedger::Main, &["2&year=9".to_owned()]);
    assert_eq!(hostile.form_body(), "year=2%26year%3D9");
}

#[test]
fn the_foundation_ledger_uses_its_own_path_and_selector() {
    let profile = BankPaymentProfile::standard();
    let plan = profile.years_request(BankLedger::Foundation);
    assert_eq!(plan.path, FOUNDATION_BANK_SEARCH_PATH);
    assert_eq!(plan.webvpn_target, FOUNDATION_BANK_WEBVPN_TARGET);
    assert_eq!(BankLedger::Main.webvpn_target(), BANK_WEBVPN_TARGET);
}

#[test]
fn the_income_date_range_must_be_two_eight_digit_bounds() {
    let profile = GraduateIncomeProfile::standard();
    assert_eq!(profile.roaming_selector(), GRADUATE_INCOME_WEBVPN_TARGET);
    assert!(matches!(
        profile.list_request("2026-01-01", "20261231").unwrap_err(),
        GraduateIncomeAdapterError::InvalidDateRange
    ));
    assert!(matches!(
        profile.list_request("20260101", "2026").unwrap_err(),
        GraduateIncomeAdapterError::InvalidDateRange
    ));
    let plan = profile
        .list_request("20260101", "20261231")
        .expect("range accepted");
    assert_eq!(plan.path, GRADUATE_INCOME_PATH);
    assert_eq!(plan.webvpn_target, GRADUATE_INCOME_WEBVPN_TARGET);
    assert_eq!(plan.method, BankPaymentMethod::Get);
}

#[test]
fn an_adapter_debug_output_carries_no_mapping_token() {
    let bank = BankPaymentAdapter::try_with_transport(
        Url::parse("https://webvpn.tsinghua.edu.cn/http/aaa/bbb/").unwrap(),
        BankLedger::Main,
        transport(),
    )
    .unwrap();
    let debug = format!("{bank:?}");
    assert!(
        !debug.contains("aaa"),
        "adapter debug leaked the mapping: {debug}"
    );
    assert!(
        !debug.contains("bbb"),
        "adapter debug leaked the mapping: {debug}"
    );

    let graduate = GraduateIncomeAdapter::try_with_transport(
        Url::parse("https://webvpn.tsinghua.edu.cn/http/aaa/bbb/").unwrap(),
        transport(),
    )
    .unwrap();
    let debug = format!("{graduate:?}");
    assert!(
        !debug.contains("aaa"),
        "adapter debug leaked the mapping: {debug}"
    );
}

#[test]
fn a_base_url_outside_a_mapping_root_is_refused() {
    assert!(
        GraduateIncomeAdapter::try_with_transport(
            Url::parse("ftp://webvpn.tsinghua.edu.cn/http/abc/def/").unwrap(),
            transport()
        )
        .is_err()
    );
    assert!(
        GraduateIncomeAdapter::try_with_transport(
            Url::parse("https://user:pass@webvpn.tsinghua.edu.cn/http/abc/def/").unwrap(),
            transport()
        )
        .is_err()
    );
}

#[tokio::test]
async fn the_payroll_read_batches_the_years_sequentially_through_the_transport() {
    let server = FixtureServer::new(vec![
        Reply::html(&years_body()),
        Reply::html(&receipts_body()),
    ]);
    let adapter = bank_adapter(&server);

    let ledger = adapter.read_ledger().await.expect("ledger reads");
    assert_eq!(ledger.months.len(), 2);
    assert_eq!(ledger.receipt_count(), 2);

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].starts_with(&format!("GET {BANK_MAPPING}/yhdfcx/search.do ")),
        "unexpected year request: {}",
        requests[0]
    );
    assert!(
        requests[1].starts_with(&format!("POST {BANK_MAPPING}/yhdfcx/search.do ")),
        "unexpected receipts request: {}",
        requests[1]
    );
    assert!(requests[1].ends_with("year=2021&year=2020"));
}

#[tokio::test]
async fn the_graduate_income_list_reads_its_rows_and_exact_amounts() {
    let body = r#"{"object":{"rows":[
        {"id":"1","ffnf":"2026","ffyf":"03","ffrq":"20260310","ffrqChs":"2026年3月",
         "dfytmc":"国家助学金","xmssbmmc":"研究生院","yfje":3000.00,"sfje":2700.00,
         "ksje":300.00},
        {"id":"2","ffnf":"2026","ffyf":"01","ffrq":"20260110","ffrqChs":"2026年1月",
         "dfytmc":"助研津贴","xmssbmmc":"计算机系","yfje":"1500.50","sfje":"1500.50",
         "ksje":"0.00"}],"total":2}}"#;
    let server = FixtureServer::new(vec![Reply::json(body)]);
    let adapter = graduate_adapter(&server);

    let page = adapter
        .read_list("20260101", "20261231")
        .await
        .expect("income reads");
    assert_eq!(page.len(), 2);
    assert_eq!(page.total, Some(2));
    assert_eq!(page.records[0].id, "1");
    assert_eq!(page.records[0].year_month, "2026年3月");
    assert_eq!(page.records[0].name, "国家助学金");
    assert_eq!(page.records[0].department, "研究生院");
    assert_eq!(page.records[0].before_tax_cents, Some(300_000));
    assert_eq!(page.records[0].after_tax_cents, Some(270_000));
    assert_eq!(page.records[0].tax_cents, Some(30_000));
    assert_eq!(page.records[1].before_tax_cents, Some(150_050));

    let request = &server.requests()[0];
    assert!(
        request.starts_with(&format!(
            "GET {GRADUATE_MAPPING}/b/yjsjzxt/v_yjszzjl_yjscwdfmx_cx/pageList?"
        )),
        "unexpected income request: {request}"
    );
    assert!(request.contains("ffkssj=20260101&ffjssj=20261231"));
    assert!(request.contains("rows=1000&page=1&sidx=id&sord=asc"));
}

#[tokio::test]
async fn a_rejected_date_range_issues_no_request_at_all() {
    let server = FixtureServer::new(vec![]);
    let adapter = graduate_adapter(&server);
    assert!(
        adapter.read_list("2026-01-01", "20261231").await.is_err(),
        "a non-digit bound must be refused"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_missing_rows_array_is_a_failure_not_an_empty_list() {
    assert_eq!(
        parse_graduate_income_json(r#"{"object":{"total":0}}"#).unwrap_err(),
        GraduateIncomeParseError::MissingRows
    );
    // An explicit empty array is the service's own "no income" answer.
    let empty = parse_graduate_income_json(r#"{"object":{"rows":[],"total":0}}"#).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.total, Some(0));
}

#[test]
fn a_graduate_income_row_without_its_identifier_is_refused() {
    let body = r#"{"object":{"rows":[{"dfytmc":"助学金","yfje":"1.00"}]}}"#;
    assert_eq!(
        parse_graduate_income_json(body).unwrap_err(),
        GraduateIncomeParseError::MissingField { row: 0 }
    );
}

#[test]
fn an_inexact_income_amount_is_refused_rather_than_rounded() {
    let body = r#"{"object":{"rows":[{"id":"1","dfytmc":"助学金","yfje":"1.005"}]}}"#;
    assert_eq!(
        parse_graduate_income_json(body).unwrap_err(),
        GraduateIncomeParseError::InvalidAmount { row: 0 }
    );
}
