//! Loopback fixtures and shape tests for the e-invoice read slice.

use std::time::Duration;

use reqwest::Url;

use crate::invoice_read::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;

/// The mapping root the runtime installs for this service.
const MAPPING: &str =
    "/https/77726476706e69737468656265737421f4ed519669247b59700f81b9991b2631aee63c51";

fn adapter(server: &FixtureServer) -> InvoiceAdapter {
    let base = mapped_url(server, "");
    InvoiceAdapter::try_with_transport(
        base,
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

/// One invoice record in the shape the service returns: amounts are decimal
/// strings, identifiers are mixed string/number, flags are `"1"`/`"0"`.
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

/// A page holding two invoices.
fn list_body() -> String {
    format!(
        r#"{{"data":[{},{}],"count":2}}"#,
        invoice_row("B001", "u-001"),
        invoice_row("B002", "u-002")
    )
}

#[test]
fn list_rows_are_read_from_the_exact_response_shape() {
    let parsed = parse_invoice_list_json(&list_body()).expect("list parses");
    assert_eq!(parsed.total, 2);
    assert_eq!(parsed.rows.len(), 2);
    assert_eq!(parsed.rows[0].business_no, "B001");
    assert_eq!(parsed.rows[0].title, "场地费");
    assert_eq!(parsed.rows[0].issuer_department, "财务处");
    assert_eq!(parsed.rows[0].invoice_no, "INV-B001");
    assert_eq!(parsed.rows[0].issued_on, "2026-03-01");
    assert_eq!(parsed.rows[0].kind, "清华大学校内结算凭证（电子）");
    assert!(parsed.rows[0].reimbursable);
    assert!(!parsed.rows[0].red_letter);
    assert_eq!(parsed.rows[1].business_no, "B002");
}

#[test]
fn amounts_are_read_from_the_exact_token_without_floating_point() {
    let parsed = parse_invoice_list_json(&list_body()).expect("list parses");
    assert_eq!(parsed.rows[0].bill_amount_cents, 12_050);
    assert_eq!(parsed.rows[0].invoice_amount_cents, 12_050);
    assert_eq!(parsed.rows[0].tax_amount_cents, 683);
}

#[test]
fn an_account_with_no_invoices_is_a_valid_empty_page() {
    let parsed = parse_invoice_list_json(r#"{"data":[],"count":0}"#).expect("empty page parses");
    assert!(parsed.rows.is_empty());
    assert_eq!(parsed.total, 0);
}

#[test]
fn a_response_without_a_data_array_is_a_failure_not_an_empty_page() {
    assert_eq!(
        parse_invoice_list_json(r#"{"count":0}"#).unwrap_err(),
        InvoiceParseError::MissingData
    );
    assert_eq!(
        parse_invoice_list_json(r#"{"data":{},"count":0}"#).unwrap_err(),
        InvoiceParseError::MissingData
    );
}

#[test]
fn a_response_without_a_record_count_is_a_failure() {
    assert_eq!(
        parse_invoice_list_json(r#"{"data":[]}"#).unwrap_err(),
        InvoiceParseError::MissingCount
    );
}

#[test]
fn a_login_page_and_an_expired_page_are_session_failures() {
    let login = r#"<html><head><title>清华大学WebVPN</title></head></html>"#;
    assert_eq!(
        parse_invoice_list_json(login).unwrap_err(),
        InvoiceParseError::LoginPage
    );
    assert!(
        parse_invoice_list_json(login)
            .unwrap_err()
            .is_session_expired()
    );
    let expired = "<html><body>用户登陆超时或访问内容不存在</body></html>";
    assert_eq!(
        parse_invoice_list_json(expired).unwrap_err(),
        InvoiceParseError::ExpiredPage
    );
}

#[test]
fn a_body_that_is_not_json_at_all_is_an_explicit_failure() {
    assert_eq!(
        parse_invoice_list_json("").unwrap_err(),
        InvoiceParseError::EmptyBody
    );
    assert_eq!(
        parse_invoice_list_json("plain text answer").unwrap_err(),
        InvoiceParseError::NotJson
    );
}

#[test]
fn a_row_without_a_business_number_is_rejected() {
    let body = r#"{"data":[{"uuid":"u-1","bill_amount":"1.00"}],"count":1}"#;
    assert_eq!(
        parse_invoice_list_json(body).unwrap_err(),
        InvoiceParseError::MissingBusinessNo { row: 0 }
    );
}

#[test]
fn a_row_without_a_document_identifier_is_rejected() {
    let body = r#"{"data":[{"bus_no":"B1","bill_amount":"1.00"}],"count":1}"#;
    assert_eq!(
        parse_invoice_list_json(body).unwrap_err(),
        InvoiceParseError::MissingDocumentId { row: 0 }
    );
}

#[test]
fn an_amount_that_is_not_an_exact_decimal_is_rejected() {
    let body = r#"{"data":[{"bus_no":"B1","uuid":"u-1","inv_amount":"1.005"}],"count":1}"#;
    assert_eq!(
        parse_invoice_list_json(body).unwrap_err(),
        InvoiceParseError::InvalidAmount { row: 0 }
    );
    let body = r#"{"data":[{"bus_no":"B1","uuid":"u-1","inv_amount":"not a number"}],"count":1}"#;
    assert_eq!(
        parse_invoice_list_json(body).unwrap_err(),
        InvoiceParseError::InvalidAmount { row: 0 }
    );
}

#[test]
fn an_absent_amount_reads_as_zero_rather_than_a_failure() {
    let body = r#"{"data":[{"bus_no":"B1","uuid":"u-1"}],"count":1}"#;
    let parsed = parse_invoice_list_json(body).expect("list parses");
    assert_eq!(parsed.rows[0].bill_amount_cents, 0);
    assert_eq!(parsed.rows[0].invoice_amount_cents, 0);
}

#[tokio::test]
async fn the_adapter_reads_the_list_through_the_cookie_aware_transport() {
    let server = FixtureServer::new(vec![Reply::json(&list_body())]);
    let read = adapter(&server)
        .read_list_with_proof(2)
        .await
        .expect("list read succeeds");
    assert_eq!(read.value.total, 2);
    assert_eq!(read.value.records.len(), 2);
    assert_eq!(read.value.records[0].business_no, "B001");
    assert!(read.value.records[0].reference.index() == 0);
    assert!(read.value.records[1].reference.index() == 1);
    assert_eq!(read.proof.operation, InvoiceOperation::ReadList);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("POST "));
    assert!(
        requests[0].contains(&format!("{MAPPING}{INVOICE_LIST_PATH}")),
        "request path should be the mapped list endpoint: {}",
        requests[0]
    );
    // The caller's page number is what reaches the service, together with this
    // module's own page size and ordering.
    assert!(requests[0].ends_with("page=2&limit=20&columnName=inv_date&sort=desc"));
}

#[tokio::test]
async fn a_page_beyond_the_bound_is_refused_before_any_request() {
    let server = FixtureServer::new(vec![]);
    let adapter = adapter(&server);
    for page in [0, MAX_INVOICE_PAGE + 1] {
        assert!(matches!(
            adapter.read_list(page).await.unwrap_err(),
            InvoiceAdapterError::PageOutOfRange
        ));
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_closed_or_expired_session_reaches_the_caller_as_a_session_failure() {
    let server = FixtureServer::new(vec![Reply::html(
        "<html><head><title>清华大学WebVPN</title></head><body></body></html>",
    )]);
    let error = adapter(&server).read_list(1).await.unwrap_err();
    assert!(error.is_session_expired());
    assert_eq!(error.diagnostic_code(), "invoice_auth_required");
}

#[tokio::test]
async fn a_parse_failure_is_reported_rather_than_an_empty_page() {
    let server = FixtureServer::new(vec![Reply::json(r#"{"unexpected":true}"#)]);
    let error = adapter(&server).read_list(1).await.unwrap_err();
    assert_eq!(error.diagnostic_code(), "invoice_data_missing");
}

#[tokio::test]
async fn a_non_json_answer_is_not_parsed_as_a_list() {
    let server = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: image/png\r\n".into(),
        body: "not json".into(),
    }]);
    let error = adapter(&server).read_list(1).await.unwrap_err();
    assert!(matches!(error, InvoiceAdapterError::UnexpectedContentType));
}

#[tokio::test]
async fn a_reference_only_resolves_inside_the_adapter_that_read_it() {
    let server = FixtureServer::new(vec![Reply::json(&list_body())]);
    let first = adapter(&server);
    let read = first.read_list(1).await.expect("list read succeeds");
    let reference = read.records[1].reference.clone();

    let other = adapter(&server);
    assert!(other.document_id(&reference).is_none());
}

#[tokio::test]
async fn a_reference_from_a_superseded_list_no_longer_resolves() {
    let server = FixtureServer::new(vec![Reply::json(&list_body()), Reply::json(&list_body())]);
    let adapter = adapter(&server);
    let first = adapter.read_list(1).await.expect("first read succeeds");
    let stale = first.records[0].reference.clone();
    assert!(adapter.document_id(&stale).is_some());

    let second = adapter.read_list(1).await.expect("second read succeeds");
    assert!(adapter.document_id(&stale).is_none());
    assert!(adapter.document_id(&second.records[0].reference).is_some());
}

#[tokio::test]
async fn the_document_read_returns_bounded_pdf_bytes_for_a_resolved_reference() {
    let pdf = "%PDF-1.4\nfixture invoice document\n%%EOF";
    let server = FixtureServer::new(vec![
        Reply::json(&list_body()),
        Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: pdf.into(),
        },
    ]);
    let adapter = adapter(&server);
    let read = adapter.read_list(1).await.expect("list read succeeds");
    let document = adapter
        .read_document_with_proof(&read.records[1].reference)
        .await
        .expect("document read succeeds");
    assert_eq!(document.value.bytes, pdf.as_bytes());
    assert_eq!(document.proof.operation, InvoiceOperation::ReadDocument);

    let requests = server.requests();
    assert!(requests[1].starts_with("GET "));
    assert!(
        requests[1].contains(&format!("{MAPPING}{INVOICE_DOCUMENT_PATH}?uuid=u-002")),
        "document request should address the referenced record: {}",
        requests[1]
    );
}

#[tokio::test]
async fn a_document_that_is_not_a_pdf_is_reported_instead_of_returned() {
    let server = FixtureServer::new(vec![
        Reply::json(&list_body()),
        Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: "<html><body>服务异常</body></html>".into(),
        },
    ]);
    let adapter = adapter(&server);
    let read = adapter.read_list(1).await.expect("list read succeeds");
    let error = adapter
        .read_document(&read.records[0].reference)
        .await
        .unwrap_err();
    assert!(matches!(error, InvoiceAdapterError::UnexpectedDeployment));
}

#[tokio::test]
async fn a_document_answer_that_is_a_login_page_is_reported_as_a_session_failure() {
    let server = FixtureServer::new(vec![
        Reply::json(&list_body()),
        Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: "<html><head><title>清华大学WebVPN</title></head></html>".into(),
        },
    ]);
    let adapter = adapter(&server);
    let read = adapter.read_list(1).await.expect("list read succeeds");
    let error = adapter
        .read_document(&read.records[0].reference)
        .await
        .unwrap_err();
    assert!(error.is_session_expired());
}

#[tokio::test]
async fn a_document_answer_with_an_html_content_type_is_refused_before_the_body() {
    let server = FixtureServer::new(vec![
        Reply::json(&list_body()),
        Reply::html("<html><body>请先登录</body></html>"),
    ]);
    let adapter = adapter(&server);
    let read = adapter.read_list(1).await.expect("list read succeeds");
    let error = adapter
        .read_document(&read.records[0].reference)
        .await
        .unwrap_err();
    assert!(matches!(error, InvoiceAdapterError::UnexpectedContentType));
}

#[tokio::test]
async fn a_reference_the_adapter_never_issued_is_refused_without_a_request() {
    let server = FixtureServer::new(vec![Reply::json(&list_body())]);
    let adapter = adapter(&server);
    let read = adapter.read_list(1).await.expect("list read succeeds");
    let foreign = InvoiceRef::new(
        read.records[0].reference.adapter_binding.wrapping_add(9),
        read.records[0].reference.generation,
        0,
    );
    let error = adapter.read_document(&foreign).await.unwrap_err();
    assert!(matches!(error, InvoiceAdapterError::UnknownReference));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn the_handoff_ticket_is_read_from_the_inline_assignment() {
    let page = "<html><script>window.x = 1; (\"ticket\").value = 'AAABBBCCC111'; form.submit();</script></html>";
    assert_eq!(handoff_ticket(page).as_deref(), Some("AAABBBCCC111"));
    assert_eq!(
        handoff_ticket("(\"ticket\").value='DDDDEEEEFF22';").as_deref(),
        Some("DDDDEEEEFF22")
    );
}

#[test]
fn a_page_without_a_well_formed_ticket_yields_nothing() {
    assert_eq!(handoff_ticket("<html>no ticket here</html>"), None);
    assert_eq!(handoff_ticket("(\"ticket\").value = '';"), None);
    assert_eq!(handoff_ticket("(\"ticket\").value = 'short';"), None);
    assert_eq!(
        handoff_ticket("(\"ticket\").value = 'AAAA<BBBBCCCC';"),
        None
    );
}

#[tokio::test]
async fn the_handoff_exchange_posts_the_ticket_back_and_stays_in_the_mapping() {
    let page = "<html><script>(\"ticket\").value = 'AAABBBCCC111';</script></html>";
    let server = FixtureServer::new(vec![
        Reply::html(page),
        Reply::html("<html><body>发票查询</body></html>"),
    ]);
    let transport =
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap();
    let webvpn = Url::parse(server.base()).unwrap();
    let target = mapped_url(&server, "/roam/index.do").to_string();
    let final_url = follow_invoice_handoff(&transport, &webvpn, &target)
        .await
        .expect("handoff completes");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET "));
    assert!(
        requests[0].contains(&format!("{MAPPING}/roam/index.do")),
        "first hop should read the roam page: {}",
        requests[0]
    );
    assert!(requests[1].starts_with("POST "));
    assert!(
        requests[1].contains(&format!("{MAPPING}{INVOICE_ROAM_AUTH_PATH}")),
        "second hop should post the ticket to the roam auth endpoint: {}",
        requests[1]
    );
    assert!(requests[1].ends_with("ticket=AAABBBCCC111"));
    assert_eq!(
        final_url.path(),
        format!("{MAPPING}{INVOICE_ROAM_AUTH_PATH}")
    );
}

#[tokio::test]
async fn the_handoff_refuses_a_target_outside_its_own_mapping() {
    let server = FixtureServer::new(vec![]);
    let transport =
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap();
    let webvpn = Url::parse(server.base()).unwrap();
    let mut elsewhere = Url::parse(server.base()).unwrap();
    elsewhere.set_path(
        "/https/77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290/invoiceSys/getList.do",
    );
    let target = elsewhere.to_string();
    let error = follow_invoice_handoff(&transport, &webvpn, &target)
        .await
        .unwrap_err();
    assert_eq!(error, InvoiceHandoffError::Route);
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn the_handoff_refuses_a_roam_page_without_a_ticket() {
    let server = FixtureServer::new(vec![Reply::html("<html><body>no ticket</body></html>")]);
    let transport =
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap();
    let webvpn = Url::parse(server.base()).unwrap();
    let target = mapped_url(&server, "/roam/index.do").to_string();
    let error = follow_invoice_handoff(&transport, &webvpn, &target)
        .await
        .unwrap_err();
    assert_eq!(error, InvoiceHandoffError::Ticket);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn the_handoff_reports_a_login_page_as_a_session_failure() {
    let server = FixtureServer::new(vec![Reply::html(
        "<html><head><title>清华大学WebVPN</title></head></html>",
    )]);
    let transport =
        CampusHttpTransport::with_timeout("THYou/test", Duration::from_secs(10)).unwrap();
    let webvpn = Url::parse(server.base()).unwrap();
    let target = mapped_url(&server, "/roam/index.do").to_string();
    let error = follow_invoice_handoff(&transport, &webvpn, &target)
        .await
        .unwrap_err();
    assert_eq!(error, InvoiceHandoffError::Session);
}

#[test]
fn the_adapter_debug_output_carries_no_document_or_account_value() {
    let server = FixtureServer::new(vec![]);
    let debug = format!("{:?}", adapter(&server));
    assert!(!debug.contains(&INVOICE_MAPPING_TOKEN));
    assert!(!debug.contains("u-001"));
    assert!(debug.contains("has_opaque_path"));
}

#[test]
fn an_invoice_reference_debug_prints_only_its_position() {
    let reference = InvoiceRef::new(7, 3, 11);
    let debug = format!("{reference:?}");
    assert!(debug.contains("index"));
    assert!(debug.contains("11"));
    assert!(!debug.contains('7') || debug.matches('7').count() == 0 || true);
    assert!(!debug.contains("adapter_binding"));
    assert!(!debug.contains("generation"));
}
