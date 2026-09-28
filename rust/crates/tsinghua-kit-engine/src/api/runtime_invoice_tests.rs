//! Runtime-level fixtures for the e-invoice read slice.
//!
//! These use only loopback responses.  They verify that the list and document
//! reads happen inside the existing INFO/WebVPN session, that the one-time
//! handoff ticket never becomes the adapter's base URL or a caller-visible
//! value, that a page beyond the adapter's bound is refused before any
//! request, and that a superseded list leaves no resolvable document
//! reference behind.

use super::*;

use crate::info::OpaqueUrl;
use crate::info_session::{InfoSessionAdapter, InfoWebVpnConfig};
use crate::invoice_read::{INVOICE_LIST_PATH, MAX_INVOICE_PAGE};
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

/// The mapping token of the e-invoice host, as the allowlist binds it.
const INVOICE_MAPPING: &str =
    "77726476706e69737468656265737421f4ed519669247b59700f81b9991b2631aee63c51";

fn invoice_user() -> UserIdentity {
    UserIdentity {
        username: "fixture-invoice-owner".to_owned(),
        display_name: None,
    }
}

/// Installs a proven Identity + INFO session whose adapter talks to `server`.
fn invoice_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime =
        CampusRuntime::new_with_persistence("auto".into(), false, String::new(), false).unwrap();
    let user = invoice_user();
    for service in [ServiceId::Identity, ServiceId::Info] {
        runtime.coordinator.begin_authentication(service).unwrap();
        let csrf = (service == ServiceId::Info).then(|| {
            runtime
                .coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-invoice-csrf").unwrap())
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

/// One invoice row shaped like the service's own answer: amounts are decimal
/// strings, identifiers are mixed string/number, flags are `"1"`/`"0"`, and
/// the record identifier the adapter addresses documents by is a `uuid`.
fn invoice_row(bus_no: &str, uuid: &str) -> String {
    format!(
        r#"{{"bill_amount":"120.50","bmdm":"BM01","bus_no":"{bus_no}",
            "cust_email":"somebody@example.invalid","cust_mob":"13800000000",
            "cust_name":"某老师","cust_tax_no":"TAX01","cust_type":"1",
            "file_name":"f.pdf","financial_dept_name":"财务处",
            "financial_item_name":"场地费","inv_amount":"120.50","inv_code":"C01",
            "inv_crc":"crc","inv_data_id":991,"inv_date":"2026-03-01","inv_isred":"0",
            "inv_no":"INV-{bus_no}","inv_note":"备注","inv_type":"D101",
            "inv_typeStr":"清华大学校内结算凭证（电子）","is_allow_reimbursement":"1",
            "ists":"1","payment_item_type_name":"场地费","red_bus_no":"",
            "tax_amount":"6.83","uuid":"{uuid}"}}"#
    )
}

fn list_body() -> String {
    format!(
        r#"{{"data":[{},{}],"count":2}}"#,
        invoice_row("B001", "u-001"),
        invoice_row("B002", "u-002")
    )
}

/// The invoice handoff: the portal answers with the target host's own portal
/// entry point, the roam page carries a one-time ticket, and the exchange
/// answers with the application page.
fn invoice_handoff_replies(page: &str) -> Vec<Reply> {
    vec![
        Reply::html("XSRF-TOKEN=fixture-invoice-csrf;"),
        Reply::json(
            r#"{"object":{"roamingurl":"https://dzpj.tsinghua.edu.cn/roam/index.do?yyfwid=FIXTURE"}}"#,
        ),
        Reply::html("<html><script>(\"ticket\").value = 'AAABBBCCC111';</script></html>"),
        Reply::html("<html><body>发票查询</body></html>"),
        Reply::json(page),
    ]
}

#[tokio::test]
async fn backend_repair_invoice_list_reads_inside_the_proven_info_session() {
    let server = FixtureServer::new(invoice_handoff_replies(&list_body()));
    let mut runtime = invoice_runtime(&server);

    let result = runtime
        .load_invoice_list_result(1)
        .await
        .expect("invoice list reads");

    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert_eq!(result.page, 1);
    assert_eq!(result.total, 2);
    assert!(result.error.is_none());
    assert_eq!(result.records.len(), 2);
    assert_eq!(result.records[0].business_no, "B001");
    assert_eq!(result.records[0].title, "场地费");
    assert_eq!(result.records[0].bill_amount_cents, 12_050);
    assert_eq!(result.records[1].business_no, "B002");
    // The bridge row carries only the position, never the service's record id.
    assert_eq!(result.records[0].reference.index(), 0);
    assert_eq!(result.records[1].reference.index(), 1);
    assert!(runtime.invoice_service_is_proven());
    assert!(runtime.service_session_is_proven(ServiceId::Info));

    // The handoff's one-time ticket must never become the adapter's base URL:
    // the last request carries only the mapping root and the list's own path.
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    let read = &requests[4];
    assert!(
        read.starts_with(&format!(
            "POST /https/{INVOICE_MAPPING}{INVOICE_LIST_PATH} "
        )),
        "unexpected read request: {read}"
    );
    assert!(!read.contains("AAABBBCCC111"));
    assert!(!read.contains("FIXTURE"));
    assert!(read.ends_with("page=1&limit=20&columnName=inv_date&sort=desc"));
}

#[tokio::test]
async fn backend_repair_invoice_page_beyond_the_bound_is_refused_before_any_list_request() {
    // The reader session is established first, as it is for every INFO-hosted
    // read; what the bound forbids is the list request itself.  No page outside
    // the adapter's range may reach the service.
    let server = FixtureServer::new(invoice_handoff_replies(&list_body()));
    let mut runtime = invoice_runtime(&server);

    for page in [0, MAX_INVOICE_PAGE + 1] {
        let error = runtime
            .load_invoice_list_result(page)
            .await
            .expect_err("a page beyond the bound is refused");
        assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    }
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .all(|request| !request.contains(INVOICE_LIST_PATH)),
        "a refused page must not reach the list endpoint: {requests:?}"
    );
}

#[tokio::test]
async fn backend_repair_invoice_document_read_stays_bounded_and_referenced() {
    let pdf = "%PDF-1.4\nfixture invoice document\n%%EOF";
    let mut replies = invoice_handoff_replies(&list_body());
    replies.push(Reply {
        status: 200,
        headers: "Content-Type: application/pdf\r\n".into(),
        body: pdf.into(),
    });
    let server = FixtureServer::new(replies);
    let mut runtime = invoice_runtime(&server);

    let list = runtime
        .load_invoice_list_result(1)
        .await
        .expect("invoice list reads");
    let document = runtime
        .load_invoice_document_result(&list.records[1].reference)
        .await
        .expect("document read succeeds");
    assert_eq!(document.bytes, pdf.as_bytes());
    assert_eq!(document.source, "live");

    let requests = server.requests();
    let read = &requests[5];
    assert!(
        read.starts_with(&format!(
            "GET /https/{INVOICE_MAPPING}/invoice/showInvPdf.do?uuid=u-002 "
        )),
        "unexpected document request: {read}"
    );
}

#[tokio::test]
async fn backend_repair_invoice_document_answer_that_is_not_the_document_fails() {
    let mut replies = invoice_handoff_replies(&list_body());
    replies.push(Reply {
        status: 200,
        headers: "Content-Type: application/pdf\r\n".into(),
        body: "<html><body>服务异常</body></html>".into(),
    });
    let server = FixtureServer::new(replies);
    let mut runtime = invoice_runtime(&server);

    let list = runtime
        .load_invoice_list_result(1)
        .await
        .expect("invoice list reads");
    let error = runtime
        .load_invoice_document_result(&list.records[0].reference)
        .await
        .expect_err("an answer that is not the document is a failure");
    assert!(
        !error.contains("u-001"),
        "error echoed a record id: {error}"
    );
    assert_eq!(
        runtime.last_invoice_failure_code(),
        Some("invoice_template")
    );
}

#[tokio::test]
async fn backend_repair_invoice_parse_failure_is_not_an_empty_page() {
    // The service answers 200 with a JSON document that has no `data` array.
    // The caller must see a failure, not a validated empty page.
    let server = FixtureServer::new(invoice_handoff_replies(r#"{"unexpected":true}"#));
    let mut runtime = invoice_runtime(&server);

    let error = runtime
        .load_invoice_list_result(1)
        .await
        .expect_err("a body without a data array is a failure");
    assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    assert_eq!(
        runtime.last_invoice_failure_code(),
        Some("invoice_data_missing")
    );
    assert!(!runtime.invoice_service_is_proven());
}

#[tokio::test]
async fn backend_repair_invoice_expiry_page_is_a_session_failure() {
    // The handoff succeeds but the list itself is an expiry page.  The session
    // is dropped rather than replayed, and a list from an earlier read must
    // not be presented as the current one.
    let server = FixtureServer::new(invoice_handoff_replies(
        "time out用户登陆超时或访问内容不存在。请重试",
    ));
    let mut runtime = invoice_runtime(&server);

    let error = runtime
        .load_invoice_list_result(1)
        .await
        .expect_err("an expiry page is a failure");
    assert!(!error.contains("ticket"), "error echoed a ticket: {error}");
    assert!(!runtime.invoice_service_is_proven());
}

#[tokio::test]
async fn backend_repair_invoice_requires_a_proven_account() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = invoice_runtime(&server);
    runtime.invalidate_service_session(ServiceId::Identity);
    runtime.invalidate_service_session(ServiceId::Info);

    assert!(
        runtime.load_invoice_list_result(1).await.is_err(),
        "an unproven account must not read a list"
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_invoice_reference_does_not_survive_the_session() {
    let server = FixtureServer::new(invoice_handoff_replies(&list_body()));
    let mut runtime = invoice_runtime(&server);

    let list = runtime
        .load_invoice_list_result(1)
        .await
        .expect("invoice list reads");
    let stale = list.records[0].reference.clone();

    // Invalidating the INFO session drops the adapter, and with it the
    // document identifiers: no reference handed out earlier can outlive the
    // session it came from.
    runtime.invalidate_service_session(ServiceId::Info);
    assert!(runtime.invoice_adapter.is_none());

    let error = runtime
        .load_invoice_document_result(&stale)
        .await
        .expect_err("a reference from a dropped session must not resolve");
    assert!(
        !error.contains("u-001"),
        "error echoed a record id: {error}"
    );
}
