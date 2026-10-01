//! Runtime-level fixtures for the bank payroll and graduate-income read
//! slices.
//!
//! These use only loopback responses.  They verify that each read happens
//! inside the existing INFO/WebVPN session, that one payroll ledger's proof
//! cannot satisfy the other ledger's read, that the two payroll ledgers are
//! addressed as two path families on one mapping, and that a service answer
//! which is not the expected document is a failure rather than an empty
//! statement.

use super::*;

use crate::bank_read::{BANK_SEARCH_PATH, FOUNDATION_BANK_SEARCH_PATH, GRADUATE_INCOME_PATH};
use crate::info::OpaqueUrl;
use crate::info_session::{InfoSessionAdapter, InfoWebVpnConfig};
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

/// The payroll host's mapping token, as the allowlist binds it.
const BANK_MAPPING: &str =
    "77726476706e69737468656265737421e9ff459a69247b59700f81b9991b26317dbd36ae";

/// The graduate-income host's mapping token.
const GRADUATE_MAPPING: &str =
    "77726476706e69737468656265737421eaed4b9069377a517a1d88b89d1b37269c624d2b1c6925f37faea82b8d";

fn bank_user() -> UserIdentity {
    UserIdentity {
        username: "fixture-bank-owner".to_owned(),
        display_name: None,
    }
}

/// Installs a proven Identity + INFO session whose adapter talks to `server`.
fn bank_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime =
        CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap();
    let user = bank_user();
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            runtime
                .coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-bank-csrf").unwrap())
        });
        runtime
            .coordinator
            .mark_authenticated(service, user.clone(), None, csrf, None)
            .unwrap();
    }
    runtime.portal_bootstrapped = true;
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/info/").unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    runtime.info_roaming_url =
        Some(OpaqueUrl::new(format!("{}info/f/info/index", server.base())).unwrap());
    runtime
}

/// The year form the service answers with.
fn years_body() -> String {
    "<html><body><form><select name=\"year\">\
     <option value=\"2021\">2021年</option>\
     <option value=\"2020\">2020年</option>\
     </select></form></body></html>"
        .to_owned()
}

/// One month's receipt table in the service's shape.
fn receipt_table(month: &str, usage: &str, amount: &str) -> String {
    format!(
        "<div><strong>{month}银行代发结果</strong></div><table><tbody>\
         <tr><th>序</th><th>代发部门</th><th>代发项目</th><th>代发用途</th><th>代发说明</th>\
         <th>开户银行</th><th>计税时间</th><th>应发金额</th><th>扣税金额</th>\
         <th>实发金额</th><th>存折金额</th><th>现金金额</th></tr>\
         <tr><td>1</td><td>024 计算机系</td><td>元宇宙项目开发</td><td>{usage}</td>\
         <td></td><td>中国银行</td><td>20771225 15:07:37</td>\
         <td><strong>{amount}</strong></td><td><strong>0.00</strong></td>\
         <td><strong>{amount}</strong></td><td><strong>{amount}</strong></td>\
         <td><strong>0.00</strong></td></tr>\
         <tr><td colspan=\"12\">合计</td></tr></tbody></table>"
    )
}

fn receipts_body() -> String {
    format!(
        "<html><body>{}</body></html>",
        receipt_table("2021年12月", "勤工俭学", "500.00")
    )
}

#[tokio::test]
async fn backend_repair_bank_payroll_reads_inside_the_proven_info_session() {
    let server = FixtureServer::new(vec![
        Reply::html(&years_body()),
        Reply::html(&receipts_body()),
    ]);
    let mut runtime = bank_runtime(&server);

    let result = runtime
        .load_bank_payment_ledger_result(BankLedger::Main)
        .await
        .expect("payroll ledger reads");

    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert_eq!(result.receipt_count, 1);
    assert!(result.error.is_none());
    assert_eq!(result.months.len(), 1);
    assert_eq!(result.months[0].month, "2021年12月");
    let receipt = &result.months[0].receipts[0];
    assert_eq!(receipt.department, "024 计算机系");
    assert_eq!(receipt.usage, "勤工俭学");
    assert_eq!(receipt.total_cents, Some(50_000));
    assert_eq!(receipt.actual_cents, Some(50_000));
    assert!(runtime.bank_payment_service_is_proven());

    // The year and receipt reads both land inside this host's mapping, and the
    // batch names the year set the service itself offered.  They are the only
    // two requests of a ledger read: the proven INFO session is the
    // precondition, not a portal handoff in front of the read.
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let year_request = &requests[0];
    assert!(
        year_request.starts_with(&format!("GET /http/{BANK_MAPPING}{BANK_SEARCH_PATH} ")),
        "unexpected year request: {year_request}"
    );
    let receipt_request = &requests[1];
    assert!(
        receipt_request.starts_with(&format!("POST /http/{BANK_MAPPING}{BANK_SEARCH_PATH} ")),
        "unexpected receipt request: {receipt_request}"
    );
    assert!(receipt_request.ends_with("year=2021&year=2020"));
}

#[tokio::test]
async fn backend_repair_bank_foundation_ledger_is_a_separate_path_on_the_same_mapping() {
    let server = FixtureServer::new(vec![
        Reply::html(&years_body()),
        Reply::html(&receipts_body()),
    ]);
    let mut runtime = bank_runtime(&server);

    runtime
        .load_bank_payment_ledger_result(BankLedger::Foundation)
        .await
        .expect("foundation ledger reads");

    let requests = server.requests();
    assert!(
        requests[0].starts_with(&format!(
            "GET /http/{BANK_MAPPING}{FOUNDATION_BANK_SEARCH_PATH} "
        )),
        "unexpected foundation year request: {}",
        requests[0]
    );
    assert!(
        requests[1].starts_with(&format!(
            "POST /http/{BANK_MAPPING}{FOUNDATION_BANK_SEARCH_PATH} "
        )),
        "unexpected foundation receipt request: {}",
        requests[1]
    );
}

#[tokio::test]
async fn backend_repair_bank_ledger_switch_reproves_before_reading_the_other_ledger() {
    // One adapter serves one ledger.  Asking for the other ledger must prepare
    // that ledger's own adapter rather than reuse the first one's proof.
    let server = FixtureServer::new(vec![
        Reply::html(&years_body()),
        Reply::html(&receipts_body()),
        Reply::html(&years_body()),
        Reply::html(&receipts_body()),
    ]);
    let mut runtime = bank_runtime(&server);

    runtime
        .load_bank_payment_ledger_result(BankLedger::Main)
        .await
        .expect("main ledger reads");
    runtime
        .load_bank_payment_ledger_result(BankLedger::Foundation)
        .await
        .expect("foundation ledger reads");

    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(
        requests[2].contains(FOUNDATION_BANK_SEARCH_PATH),
        "the second ledger must address its own path: {}",
        requests[2]
    );
}

#[tokio::test]
async fn backend_repair_bank_parse_failure_is_not_an_empty_statement() {
    // The service answers 200 with a year form carrying no options.  The caller
    // must see a failure, not a validated empty ledger.
    let server = FixtureServer::new(vec![Reply::html(
        "<html><body><form><select name=\"year\"></select></form></body></html>",
    )]);
    let mut runtime = bank_runtime(&server);

    runtime
        .load_bank_payment_ledger_result(BankLedger::Main)
        .await
        .expect_err("a year form with no options is a failure");
    assert_eq!(
        runtime.last_bank_payment_failure_code(),
        Some("bank_years_empty")
    );
    assert!(!runtime.bank_payment_service_is_proven());
    // No receipt request may follow a year form that offered nothing.
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_bank_expiry_page_is_a_session_failure() {
    let server = FixtureServer::new(vec![Reply::html(
        "time out用户登陆超时或访问内容不存在。请重试",
    )]);
    let mut runtime = bank_runtime(&server);

    runtime
        .load_bank_payment_ledger_result(BankLedger::Main)
        .await
        .expect_err("an expiry page is a failure");
    assert!(!runtime.bank_payment_service_is_proven());
}

#[tokio::test]
async fn backend_repair_bank_requires_a_proven_account() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = bank_runtime(&server);
    runtime.invalidate_service_session(ServiceId::Identity);
    runtime.invalidate_service_session(ServiceId::Info);

    assert!(
        runtime
            .load_bank_payment_ledger_result(BankLedger::Main)
            .await
            .is_err(),
        "an unproven account must not read a ledger"
    );
    assert!(
        runtime
            .load_graduate_income_result("20260101", "20261231")
            .await
            .is_err(),
        "an unproven account must not read income"
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_graduate_income_reads_its_own_mapping() {
    let income = r#"{"object":{"rows":[
        {"id":"1","ffnf":"2026","ffyf":"03","ffrq":"20260310","ffrqChs":"2026年3月",
         "dfytmc":"国家助学金","xmssbmmc":"研究生院","yfje":3000.00,"sfje":2700.00,
         "ksje":300.00}],"total":1}}"#;
    let mut replies: Vec<Reply> = vec![
        Reply::html("XSRF-TOKEN=fixture-bank-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"http://zzjl.graduate.tsinghua.edu.cn/b/yjsjzxt/v_yjszzjl_yjscwdfmx_cx/pageList"}}"#,
        ),
        Reply::html("<html><body>研究生收入</body></html>"),
    ];
    replies.push(Reply::json(income));
    let server = FixtureServer::new(replies);
    let mut runtime = bank_runtime(&server);

    let result = runtime
        .load_graduate_income_result("20260101", "20261231")
        .await
        .expect("income reads");
    assert_eq!(result.source, "live");
    assert_eq!(result.total, Some(1));
    assert_eq!(result.records[0].name, "国家助学金");
    assert_eq!(result.records[0].before_tax_cents, Some(300_000));
    assert!(runtime.graduate_income_service_is_proven());
    // The payroll adapter sits on a different mapping, so this read proves only
    // the income session.
    assert!(!runtime.bank_payment_service_is_proven());

    let read = &server.requests()[3];
    assert!(
        read.starts_with(&format!(
            "GET /http/{GRADUATE_MAPPING}{GRADUATE_INCOME_PATH}?"
        )),
        "unexpected income request: {read}"
    );
    assert!(read.contains("ffkssj=20260101&ffjssj=20261231"));
}

#[tokio::test]
async fn backend_repair_graduate_income_rejects_a_non_digit_range_before_any_list_request() {
    let mut replies: Vec<Reply> = vec![
        Reply::html("XSRF-TOKEN=fixture-bank-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"http://zzjl.graduate.tsinghua.edu.cn/b/yjsjzxt/v_yjszzjl_yjscwdfmx_cx/pageList"}}"#,
        ),
        Reply::html("<html><body>研究生收入</body></html>"),
    ];
    replies.push(Reply::json(r#"{"object":{"rows":[],"total":0}}"#));
    let server = FixtureServer::new(replies);
    let mut runtime = bank_runtime(&server);

    // The reader session is established first, as it is for every INFO-hosted
    // read; what the bound forbids is the list request itself.  A caller's free
    // text must never reach the service as a date filter.
    runtime
        .load_graduate_income_result("2026-01-01", "20261231")
        .await
        .expect_err("a non-digit bound is refused");
    assert_eq!(
        runtime.last_graduate_income_failure_code(),
        Some("graduate_income_range")
    );
    let requests = server.requests();
    assert!(
        requests.iter().all(|request| !request.contains("ffkssj=")),
        "a refused range must not reach the list endpoint: {requests:?}"
    );

    // The same session-backed reader must not have been damaged by the
    // refusal: the range check is the argument's, not the connection's.
    assert!(runtime.graduate_income_adapter.is_none());
    assert!(
        runtime.service_session_is_proven(ServiceId::Info),
        "a refused argument must not degrade the INFO session"
    );
}
