//! Read-only issued-invoice records and their documents (`电子发票`).
//!
//! The invoice service is a legacy application on its own campus host, reached
//! through the WebVPN mapping bound to selector [`INVOICE_WEBVPN_TARGET`].  Its
//! handoff is the only one in this engine that needs a *second* step: the first
//! response is an HTML page carrying a one-time ticket in an inline script, and
//! that ticket must be posted back to [`INVOICE_ROAM_AUTH_PATH`] before the
//! application session exists.  [`follow_invoice_handoff`] performs both steps
//! and refuses to continue when either the page or the ticket is missing.
//!
//! Two reads are exposed:
//!
//! * The paginated invoice list.  `page` is a caller-supplied value with a
//!   module-owned upper bound; the page size is fixed here, because it is the
//!   service's own page size rather than a preference.
//! * One invoice's PDF document, addressed by an opaque [`InvoiceRef`].  The
//!   service's `uuid` never leaves this module, so a caller cannot point the
//!   document read at a record it did not receive from the list.
//!
//! Invoice records carry billing identifiers for the payer (`cust_name`,
//! `cust_mob`, `cust_tax_no`, …).  Those fields are deliberately not part of the
//! public projection: the account owner already knows who they are, and an
//! unrelated payer's details have no business crossing the bridge.
//!
//! Amounts are converted to integer cents from the exact JSON token rather than
//! through `f64`, so a value the service wrote as a decimal cannot come back
//! rounded.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{
    fmt,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use serde_json::value::RawValue;
use thiserror::Error;

use crate::campus_html::{self, PageClass};
use crate::telemetry::timing::{BoundedBodyError, read_bounded_bytes};
use crate::transport::{CampusHttpTransport, TransportError};

/// The invoice host's WebVPN roaming selector.
pub const INVOICE_WEBVPN_TARGET: &str = "625B81A7A9D148B01DA59185CC4074E1";

/// The paginated invoice-list endpoint behind the selector above.
pub const INVOICE_LIST_PATH: &str = "/invoiceSys/getList.do";

/// One invoice's PDF document.  The record's opaque reference supplies the
/// `uuid` query value; the path itself is a module constant.
pub const INVOICE_DOCUMENT_PATH: &str = "/invoice/showInvPdf.do";

/// The second handoff step: the ticket from the roam page is posted here.
pub const INVOICE_ROAM_AUTH_PATH: &str = "/roam/roamAuth.do";

/// The WebVPN mapping this service lives behind.  It is the only mapping the
/// handoff will follow, so a rewritten `roamingurl` cannot move the exchange to
/// another application.
pub(crate) const INVOICE_MAPPING: &str =
    "/https/77726476706e69737468656265737421f4ed519669247b59700f81b9991b2631aee63c51";

/// The opaque deployment token inside [`INVOICE_MAPPING`].
pub(crate) const INVOICE_MAPPING_TOKEN: &str =
    "77726476706e69737468656265737421f4ed519669247b59700f81b9991b2631aee63c51";

/// The page size this module requests.  It is the service's own page size, not
/// a caller preference, so it stays a constant instead of a parameter.
pub const INVOICE_PAGE_SIZE: u32 = 20;

/// The largest page number a caller may ask for.  The bound exists so a caller
/// cannot use the adapter to walk the service indefinitely.
pub const MAX_INVOICE_PAGE: u32 = 1000;

/// The column the list is ordered by, and the direction.  Both are the
/// service's own field name and its documented ordering.
const ORDER_COLUMN: &str = "inv_date";
const ORDER_DIRECTION: &str = "desc";

/// The inline script assignment that carries the one-time handoff ticket.
const TICKET_CALL: &str = "(\"ticket\").value";

/// The shortest ticket the handoff could plausibly issue; anything shorter is a
/// mis-sliced fragment rather than a value.
const MIN_TICKET_CHARS: usize = 8;
const MAX_TICKET_CHARS: usize = 512;

const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_ROWS: usize = 4096;
const MAX_TEXT_CHARS: usize = 512;
const MAX_RECORD_FIELDS: usize = 64;
const MAX_PATH_CHARS: usize = 512;

/// The document read is a PDF; 12 MiB matches the other binary read in this
/// engine and is far above any real invoice.
const MAX_DOCUMENT_BYTES: usize = 12 * 1024 * 1024;
/// The magic number every PDF begins with.
const PDF_MAGIC: &[u8] = b"%PDF-";

static NEXT_INVOICE_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// This profile only issues GETs and one fixed POST body, so no write route can
/// be smuggled into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceMethod {
    Get,
    PostForm,
}

/// The observed invoice operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceOperation {
    Handoff,
    ReadList,
    ReadDocument,
}

/// An invoice read requires an already established INFO/WebVPN session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path, query and form fields only, never an
/// absolute WebVPN mapping, Cookie, or account value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceRequestPlan {
    pub operation: InvoiceOperation,
    pub method: InvoiceMethod,
    pub path: &'static str,
    pub query: &'static str,
    pub webvpn_target: &'static str,
    pub session_prerequisite: InvoiceSessionPrerequisite,
}

impl InvoiceRequestPlan {
    /// Returns the serialized query without the leading `?`.
    pub fn query_string(&self) -> &str {
        self.query
    }
}

/// Fixed invoice route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InvoiceProfile;

impl InvoiceProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        INVOICE_WEBVPN_TARGET
    }

    pub fn list_request(self) -> InvoiceRequestPlan {
        InvoiceRequestPlan {
            operation: InvoiceOperation::ReadList,
            method: InvoiceMethod::PostForm,
            path: INVOICE_LIST_PATH,
            query: "",
            webvpn_target: INVOICE_WEBVPN_TARGET,
            session_prerequisite: InvoiceSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    pub fn document_request(self) -> InvoiceRequestPlan {
        InvoiceRequestPlan {
            operation: InvoiceOperation::ReadDocument,
            method: InvoiceMethod::Get,
            path: INVOICE_DOCUMENT_PATH,
            query: "",
            webvpn_target: INVOICE_WEBVPN_TARGET,
            session_prerequisite: InvoiceSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// One parsed invoice record, before the adapter attaches its opaque reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceRow {
    /// The service's business number, used as the row key.
    pub business_no: String,
    /// The billed item's human name.
    pub title: String,
    /// The issuing department's name.
    pub issuer_department: String,
    /// The payment item's name.
    pub payment_item: String,
    /// The invoice number as the service rendered it.
    pub invoice_no: String,
    /// The issue date as the service rendered it.
    pub issued_on: String,
    /// The service's free-form note for this record.
    pub note: String,
    /// The service's own document-kind label.
    pub kind: String,
    /// True when the service marked this record reimbursable.
    pub reimbursable: bool,
    /// True when the service marked this record as a red-letter (reversal) one.
    pub red_letter: bool,
    /// The billed amount in cents.
    pub bill_amount_cents: i64,
    /// The invoiced amount in cents.
    pub invoice_amount_cents: i64,
    /// The tax amount in cents.
    pub tax_amount_cents: i64,
    /// The service's record identifier.  It is retained only so the adapter can
    /// address this record's document, and never leaves the module.
    uuid: String,
}

/// An opaque reference to one invoice's document.
///
/// The `uuid` stays inside the adapter that produced it.  A reference from a
/// different adapter instance, from a superseded list, or beyond the range of
/// the list it came from does not resolve at all.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct InvoiceRef {
    pub(crate) adapter_binding: u64,
    pub(crate) generation: u64,
    index: u32,
}

impl InvoiceRef {
    pub(crate) fn new(adapter_binding: u64, generation: u64, index: u32) -> Self {
        Self {
            adapter_binding,
            generation,
            index,
        }
    }

    /// The position of this record inside the list that produced it.
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl fmt::Debug for InvoiceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvoiceRef")
            .field("index", &self.index)
            .finish()
    }
}

/// One issued invoice as exposed to a caller.
#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceRecord {
    /// The service's business number, used as the row key.
    pub business_no: String,
    /// The billed item's human name.
    pub title: String,
    /// The issuing department's name.
    pub issuer_department: String,
    /// The payment item's name.
    pub payment_item: String,
    /// The invoice number as the service rendered it.
    pub invoice_no: String,
    /// The issue date as the service rendered it.
    pub issued_on: String,
    /// The service's free-form note for this record.
    pub note: String,
    /// The service's own document-kind label.
    pub kind: String,
    /// True when the service marked this record reimbursable.
    pub reimbursable: bool,
    /// True when the service marked this record as a red-letter (reversal) one.
    pub red_letter: bool,
    /// The billed amount in cents.
    pub bill_amount_cents: i64,
    /// The invoiced amount in cents.
    pub invoice_amount_cents: i64,
    /// The tax amount in cents.
    pub tax_amount_cents: i64,
    /// Opaque handle for this record's document.
    pub reference: InvoiceRef,
}

/// One validated page of invoice records.
///
/// An empty `records` list with a zero `total` is a valid answer: the service
/// really does report that an account has no invoices.  It is only produced
/// when the response carried a well-formed `data` array, so "no invoices" can
/// never be manufactured from a response that failed to parse.
#[derive(Debug, Clone, PartialEq)]
pub struct InvoicePage {
    pub records: Vec<InvoiceRecord>,
    /// The service's total record count across all pages.
    pub total: u64,
}

impl InvoicePage {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// One invoice document's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceDocument {
    pub bytes: Vec<u8>,
}

impl InvoiceDocument {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// The records parsed from one list response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceListRows {
    pub rows: Vec<InvoiceRow>,
    pub total: u64,
}

/// Parser failures retain only stable names.  They never keep response bytes,
/// Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvoiceParseError {
    #[error("invoice response body is empty")]
    EmptyBody,

    #[error("invoice response is an HTML login page")]
    LoginPage,

    #[error("invoice response is an expired or timed-out page")]
    ExpiredPage,

    #[error("invoice response is not the expected JSON document")]
    NotJson,

    #[error("the invoice list response has no data array")]
    MissingData,

    #[error("the invoice list response has no record count")]
    MissingCount,

    #[error("the invoice list response exceeded the bounded record limit")]
    TooLarge,

    #[error("invoice record {row} is not a JSON object")]
    UnrecognizedRow { row: usize },

    #[error("invoice record {row} is missing its business number")]
    MissingBusinessNo { row: usize },

    #[error("invoice record {row} is missing its document identifier")]
    MissingDocumentId { row: usize },

    #[error("invoice record {row} carried an amount that is not an exact decimal")]
    InvalidAmount { row: usize },
}

impl InvoiceParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired INFO session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }
}

/// Adapter failures are body-free so a login page or a document read cannot
/// leak through a debug or bridge DTO.
#[derive(Debug, Error)]
pub enum InvoiceAdapterError {
    #[error("invoice base URL is invalid")]
    InvalidBaseUrl,

    #[error("invoice transport failed")]
    Transport(#[source] TransportError),

    #[error("invoice request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("invoice response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("invoice response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("invoice response is not the expected content type")]
    UnexpectedContentType,

    #[error("invoice INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("the invoice handoff page did not carry a usable ticket")]
    HandoffTicket,

    #[error("the invoice handoff page was not the expected deployment")]
    UnexpectedDeployment,

    #[error("invoice response could not be parsed: {0}")]
    Parse(#[source] InvoiceParseError),

    #[error("the invoice page request is outside the allowed bound")]
    PageOutOfRange,

    #[error("the referenced invoice document is no longer available")]
    UnknownReference,

    #[error("the invoice document exceeded the bounded read limit")]
    DocumentTooLarge,
}

impl InvoiceAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl | Self::PageOutOfRange => "invoice_config",
            Self::Transport(_) => "invoice_network",
            Self::HttpStatus { .. } => "invoice_http",
            Self::UnexpectedOrigin => "invoice_origin",
            Self::UnexpectedPath => "invoice_path",
            Self::UnexpectedContentType => "invoice_content_type",
            Self::SessionExpired => "invoice_auth_required",
            Self::HandoffTicket => "invoice_handoff_ticket",
            Self::UnexpectedDeployment => "invoice_template",
            Self::Parse(InvoiceParseError::EmptyBody) => "invoice_body_empty",
            Self::Parse(InvoiceParseError::LoginPage) => "invoice_auth_required",
            Self::Parse(InvoiceParseError::ExpiredPage) => "invoice_auth_required",
            Self::Parse(InvoiceParseError::NotJson) => "invoice_not_json",
            Self::Parse(InvoiceParseError::MissingData) => "invoice_data_missing",
            Self::Parse(InvoiceParseError::MissingCount) => "invoice_count_missing",
            Self::Parse(InvoiceParseError::TooLarge) => "invoice_size",
            Self::Parse(InvoiceParseError::UnrecognizedRow { .. }) => "invoice_row_shape",
            Self::Parse(InvoiceParseError::MissingBusinessNo { .. }) => "invoice_row_key",
            Self::Parse(InvoiceParseError::MissingDocumentId { .. }) => "invoice_row_document",
            Self::Parse(InvoiceParseError::InvalidAmount { .. }) => "invoice_row_amount",
            Self::UnknownReference => "invoice_reference",
            Self::DocumentTooLarge => "invoice_size",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(InvoiceParseError::LoginPage)
                | Self::Parse(InvoiceParseError::ExpiredPage)
        )
    }
}

/// Configuration for a read-only invoice adapter.
#[derive(Clone)]
pub struct InvoiceAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl InvoiceAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, InvoiceAdapterError> {
        Self::with_user_agent_and_timeout(base_url, "THYou/invoice", Duration::from_secs(30))
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, InvoiceAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, InvoiceAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| InvoiceAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(InvoiceAdapterError::InvalidBaseUrl);
        }
        Ok(Self {
            base_url,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, InvoiceAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(InvoiceAdapterError::Transport)
    }
}

impl fmt::Debug for InvoiceAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvoiceAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one invoice response.  It is opaque: no
/// Cookie, URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct InvoiceBusinessProof {
    adapter_binding: u64,
    generation: u64,
    pub(crate) operation: InvoiceOperation,
}

impl fmt::Debug for InvoiceBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvoiceBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated page together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct InvoicePageRead {
    pub value: InvoicePage,
    pub proof: InvoiceBusinessProof,
}

/// A validated document together with its business proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceDocumentRead {
    pub value: InvoiceDocument,
    pub proof: InvoiceBusinessProof,
}

/// The document identifiers named by the most recent successful list read.
#[derive(Default)]
struct InvoiceDocuments {
    generation: u64,
    uuids: Vec<String>,
}

/// Read-only invoice client.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct InvoiceAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: InvoiceProfile,
    binding: u64,
    documents: Mutex<InvoiceDocuments>,
}

impl InvoiceAdapter {
    pub fn new(config: InvoiceAdapterConfig) -> Result<Self, InvoiceAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, InvoiceAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: InvoiceProfile::standard(),
            binding: NEXT_INVOICE_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            documents: Mutex::new(InvoiceDocuments::default()),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> InvoiceProfile {
        self.profile
    }

    /// Reads one page of invoice records.
    ///
    /// `page` is one-based and bounded by [`MAX_INVOICE_PAGE`] so the caller
    /// cannot use this adapter to walk the service without limit.
    pub async fn read_list(&self, page: u32) -> Result<InvoicePage, InvoiceAdapterError> {
        self.read_list_with_proof(page).await.map(|read| read.value)
    }

    pub async fn read_list_with_proof(
        &self,
        page: u32,
    ) -> Result<InvoicePageRead, InvoiceAdapterError> {
        if page == 0 || page > MAX_INVOICE_PAGE {
            return Err(InvoiceAdapterError::PageOutOfRange);
        }
        let plan = self.profile.list_request();
        let form = vec![
            ("page".to_owned(), page.to_string()),
            ("limit".to_owned(), INVOICE_PAGE_SIZE.to_string()),
            ("columnName".to_owned(), ORDER_COLUMN.to_owned()),
            ("sort".to_owned(), ORDER_DIRECTION.to_owned()),
        ];
        let response = self.execute(&plan, Some(&form)).await?;
        let body = self.accept_json(response).await?;
        let parsed = parse_invoice_list_json(&body).map_err(InvoiceAdapter::map_parse_error)?;
        // The generation advances only after the page parsed, so a rejected
        // read leaves the previous documents resolvable instead of pointing
        // them at nothing.
        let generation = self
            .documents
            .lock()
            .map(|documents| documents.generation)
            .unwrap_or_default()
            .wrapping_add(1);
        let records = parsed
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| InvoiceRecord {
                business_no: row.business_no.clone(),
                title: row.title.clone(),
                issuer_department: row.issuer_department.clone(),
                payment_item: row.payment_item.clone(),
                invoice_no: row.invoice_no.clone(),
                issued_on: row.issued_on.clone(),
                note: row.note.clone(),
                kind: row.kind.clone(),
                reimbursable: row.reimbursable,
                red_letter: row.red_letter,
                bill_amount_cents: row.bill_amount_cents,
                invoice_amount_cents: row.invoice_amount_cents,
                tax_amount_cents: row.tax_amount_cents,
                reference: InvoiceRef::new(self.binding, generation, index as u32),
            })
            .collect();
        if let Ok(mut documents) = self.documents.lock() {
            *documents = InvoiceDocuments {
                generation,
                uuids: parsed.rows.iter().map(|row| row.uuid.clone()).collect(),
            };
        }
        Ok(InvoicePageRead {
            value: InvoicePage {
                records,
                total: parsed.total,
            },
            proof: InvoiceBusinessProof {
                adapter_binding: self.binding,
                generation,
                operation: InvoiceOperation::ReadList,
            },
        })
    }

    /// Reads one invoice's document, addressed by a reference this adapter
    /// produced.
    pub async fn read_document(
        &self,
        reference: &InvoiceRef,
    ) -> Result<InvoiceDocument, InvoiceAdapterError> {
        self.read_document_with_proof(reference)
            .await
            .map(|read| read.value)
    }

    /// Reads one invoice's document, addressed by a reference this adapter
    /// produced.
    pub async fn read_document_with_proof(
        &self,
        reference: &InvoiceRef,
    ) -> Result<InvoiceDocumentRead, InvoiceAdapterError> {
        let uuid = self
            .document_id(reference)
            .ok_or(InvoiceAdapterError::UnknownReference)?;
        let plan = self.profile.document_request();
        let query = format!("uuid={}", percent_encode_query_value(&uuid));
        let response = self.execute_query(&plan, &query).await?;
        let bytes = self.accept_document(response).await?;
        Ok(InvoiceDocumentRead {
            value: InvoiceDocument { bytes },
            proof: InvoiceBusinessProof {
                adapter_binding: self.binding,
                generation: self.current_generation(),
                operation: InvoiceOperation::ReadDocument,
            },
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &InvoiceBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    /// Resolves a record reference to its document identifier, if the reference
    /// still belongs to the list this adapter most recently read.
    pub(crate) fn document_id(&self, reference: &InvoiceRef) -> Option<String> {
        if reference.adapter_binding != self.binding {
            return None;
        }
        let documents = self.documents.lock().ok()?;
        if documents.generation != reference.generation {
            return None;
        }
        documents
            .uuids
            .get(usize::try_from(reference.index).ok()?)
            .cloned()
    }

    /// The current document generation.  It advances once per accepted read.
    pub fn document_generation(&self) -> u64 {
        self.current_generation()
    }

    fn current_generation(&self) -> u64 {
        self.documents
            .lock()
            .map(|documents| documents.generation)
            .unwrap_or_default()
    }

    /// Issues one request and applies every guard that must precede a parse.
    async fn execute(
        &self,
        plan: &InvoiceRequestPlan,
        form: Option<&[(String, String)]>,
    ) -> Result<InvoiceResponse, InvoiceAdapterError> {
        self.execute_inner(plan, "", form).await
    }

    async fn execute_query(
        &self,
        plan: &InvoiceRequestPlan,
        query: &str,
    ) -> Result<InvoiceResponse, InvoiceAdapterError> {
        self.execute_inner(plan, query, None).await
    }

    async fn execute_inner(
        &self,
        plan: &InvoiceRequestPlan,
        query: &str,
        form: Option<&[(String, String)]>,
    ) -> Result<InvoiceResponse, InvoiceAdapterError> {
        let endpoint = self.endpoint(plan, query)?;
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let builder = match (plan.method, form) {
            (InvoiceMethod::Get, _) => self.transport.client().get(endpoint),
            (InvoiceMethod::PostForm, Some(fields)) => {
                self.transport.client().post(endpoint).form(fields)
            }
            (InvoiceMethod::PostForm, None) => return Err(InvoiceAdapterError::InvalidBaseUrl),
        };
        let response = self
            .transport
            .send(builder)
            .await
            .map_err(|error| InvoiceAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| InvoiceAdapterError::UnexpectedOrigin)?;
        let location_target = location
            .as_deref()
            .map(|value| resolve_location(&final_url, value))
            .transpose()?;
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || location_target.as_ref().is_some_and(|target| {
                same_origin(&self.base_url, target) && looks_like_login_url(target)
            })
        {
            return Err(InvoiceAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(InvoiceAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(InvoiceAdapterError::UnexpectedPath);
        }
        if final_url.path() != expected_path
            || final_url.query().map(str::to_owned) != expected_query
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
            || status.is_redirection()
        {
            return Err(InvoiceAdapterError::UnexpectedPath);
        }
        if status != StatusCode::OK {
            return Err(InvoiceAdapterError::HttpStatus { status });
        }
        Ok(InvoiceResponse {
            response,
            content_type,
        })
    }

    /// Reads a text body and classifies the two service states that arrive as a
    /// 200 page before any content-type check can turn them into a format
    /// failure.
    async fn accept_json(&self, response: InvoiceResponse) -> Result<String, InvoiceAdapterError> {
        let body = crate::telemetry::timing::read_text(response.response)
            .await
            .map_err(|error| InvoiceAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_JSON_BYTES {
            return Err(InvoiceAdapterError::UnexpectedDeployment);
        }
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(InvoiceAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        if !is_json_content_type(response.content_type.as_deref()) {
            return Err(InvoiceAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    /// Reads a document body under a hard byte bound and proves it is a PDF by
    /// its own magic number rather than by the server's content type alone.
    async fn accept_document(
        &self,
        response: InvoiceResponse,
    ) -> Result<Vec<u8>, InvoiceAdapterError> {
        if !is_document_content_type(response.content_type.as_deref()) {
            return Err(InvoiceAdapterError::UnexpectedContentType);
        }
        if response
            .response
            .content_length()
            .is_some_and(|length| length > MAX_DOCUMENT_BYTES as u64)
        {
            return Err(InvoiceAdapterError::DocumentTooLarge);
        }
        let bytes = read_bounded_bytes(response.response, MAX_DOCUMENT_BYTES)
            .await
            .map_err(|error| match error {
                BoundedBodyError::Request(error) => {
                    InvoiceAdapterError::Transport(TransportError::Request(error))
                }
                BoundedBodyError::TooLarge => InvoiceAdapterError::DocumentTooLarge,
            })?;
        // A well-formed HTML login page can arrive with a binary content type;
        // classify it as the session failure it is instead of reporting a
        // format problem.
        if let Ok(text) = std::str::from_utf8(&bytes) {
            match campus_html::classify_page(text) {
                PageClass::Login | PageClass::Expired => {
                    return Err(InvoiceAdapterError::SessionExpired);
                }
                PageClass::Unknown => {}
            }
        }
        let first = bytes
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace());
        if first.is_none() || !bytes.starts_with(PDF_MAGIC) {
            return Err(InvoiceAdapterError::UnexpectedDeployment);
        }
        Ok(bytes)
    }

    fn endpoint(&self, plan: &InvoiceRequestPlan, query: &str) -> Result<Url, InvoiceAdapterError> {
        if !valid_relative_path(plan.path)
            || query.chars().any(char::is_control)
            || invalid_percent_encoding(query)
            || path_contains_encoded_escape(query)
        {
            return Err(InvoiceAdapterError::InvalidBaseUrl);
        }
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{}", plan.path));
        if query.is_empty() {
            endpoint.set_query(None);
        } else {
            endpoint.set_query(Some(query));
        }
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    fn map_parse_error(error: InvoiceParseError) -> InvoiceAdapterError {
        match error {
            InvoiceParseError::LoginPage | InvoiceParseError::ExpiredPage => {
                InvoiceAdapterError::SessionExpired
            }
            other => InvoiceAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for InvoiceAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvoiceAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("document_generation", &self.current_generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// A response whose transport-level guards have already passed.
struct InvoiceResponse {
    response: reqwest::Response,
    content_type: Option<String>,
}

/// Performs the invoice handoff's second step.
///
/// The roaming page carries a one-time ticket in an inline script assignment,
/// and the application session only exists after that ticket is posted back to
/// the roam-auth endpoint.  Both steps stay inside this module so the ticket is
/// never handed to a caller, and both use this module's own mapping constant:
/// a rewritten `roamingurl` cannot redirect the exchange to another
/// application.
///
/// The ticket is a one-time credential.  When either step returns an
/// ambiguous result the exchange is reported as unconfirmed and is never
/// replayed.
pub(crate) async fn follow_invoice_handoff(
    transport: &CampusHttpTransport,
    webvpn: &Url,
    target: &str,
) -> Result<Url, InvoiceHandoffError> {
    let mapping = INVOICE_MAPPING;
    let mut url = Url::parse(target).map_err(|_| InvoiceHandoffError::Route)?;
    let prefix = format!("{mapping}/");
    if !crate::webvpn_url::same_origin(webvpn, &url)
        || !url.path().starts_with(&prefix)
        || url.path().contains(['%', '\\'])
        || !crate::webvpn_url::redirect_path_stays_in_mapping(&url, "https", INVOICE_MAPPING_TOKEN)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(InvoiceHandoffError::Route);
    }
    let request = transport
        .client()
        .get(url.clone())
        .build()
        .map_err(|_| InvoiceHandoffError::Route)?;
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| InvoiceHandoffError::Unconfirmed)?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(InvoiceHandoffError::Session);
    }
    let final_url = response.url().clone();
    if !crate::webvpn_url::same_origin(webvpn, &final_url)
        || !final_url.path().starts_with(&prefix)
        || final_url.fragment().is_some()
    {
        return Err(InvoiceHandoffError::Route);
    }
    if !status.is_success() {
        return Err(InvoiceHandoffError::Http);
    }
    let body = crate::telemetry::timing::read_text(response)
        .await
        .map_err(|_| InvoiceHandoffError::Unconfirmed)?;
    if body.len() > MAX_HTML_BYTES {
        return Err(InvoiceHandoffError::UnexpectedDeployment);
    }
    match campus_html::classify_page(&body) {
        PageClass::Login | PageClass::Expired => return Err(InvoiceHandoffError::Session),
        PageClass::Unknown => {}
    }
    let ticket = handoff_ticket(&body).ok_or(InvoiceHandoffError::Ticket)?;
    url.set_path(&format!("{mapping}{INVOICE_ROAM_AUTH_PATH}"));
    url.set_query(None);
    url.set_fragment(None);
    let request = transport
        .client()
        .post(url.clone())
        .form(&[("ticket", ticket.as_str())])
        .build()
        .map_err(|_| InvoiceHandoffError::Route)?;
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| InvoiceHandoffError::Unconfirmed)?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(InvoiceHandoffError::Session);
    }
    let final_url = response.url().clone();
    if !crate::webvpn_url::same_origin(webvpn, &final_url)
        || !final_url.path().starts_with(&prefix)
        || final_url.fragment().is_some()
    {
        return Err(InvoiceHandoffError::Route);
    }
    if !status.is_success() {
        return Err(InvoiceHandoffError::Http);
    }
    Ok(final_url)
}

/// The invoice handoff's own failure vocabulary.  It carries no URL, ticket, or
/// body: the caller only needs to know which stage stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum InvoiceHandoffError {
    #[error("the invoice handoff target is outside the allowed mapping")]
    Route,

    #[error("the invoice handoff did not report whether the ticket was consumed")]
    Unconfirmed,

    #[error("the invoice INFO/WebVPN session is not established")]
    Session,

    #[error("the invoice handoff returned an HTTP failure")]
    Http,

    #[error("the invoice handoff page carried no usable ticket")]
    Ticket,

    #[error("the invoice handoff page was not the expected deployment")]
    UnexpectedDeployment,
}

/// Extracts the one-time ticket from a roam page.
///
/// The page writes `("ticket").value = '…';`.  The scan requires the anchor,
/// the assignment, and both quotes in order, and bounds the value, so a
/// reworded or truncated assignment is reported as a missing ticket rather than
/// being sliced out of unrelated text.
pub fn handoff_ticket(html: &str) -> Option<String> {
    let index = html.find(TICKET_CALL)? + TICKET_CALL.len();
    let rest = html[index..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('\'')?;
    let end = rest.find('\'')?;
    let ticket = rest[..end].trim();
    if ticket.len() < MIN_TICKET_CHARS
        || ticket.len() > MAX_TICKET_CHARS
        || ticket.chars().any(char::is_control)
        || ticket.contains(['<', '>', '\\', '"', '\''])
    {
        return None;
    }
    Some(ticket.to_owned())
}

/// Parses one invoice list response.
///
/// A well-formed response with an empty `data` array is a valid page: the
/// service really does answer "no invoices" for an account that has none.  A
/// response that is missing `data` or `count`, or that carries an amount which
/// is not an exact decimal, is an error — never an empty page.
pub fn parse_invoice_list_json(body: &str) -> Result<InvoiceListRows, InvoiceParseError> {
    let trimmed = body.strip_prefix('\u{feff}').unwrap_or(body).trim();
    if trimmed.is_empty() {
        return Err(InvoiceParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(InvoiceParseError::LoginPage),
        PageClass::Expired => return Err(InvoiceParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    let raw: &RawValue = serde_json::from_str(trimmed).map_err(|_| InvoiceParseError::NotJson)?;
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(raw.get()).map_err(|_| InvoiceParseError::NotJson)?;
    let data = object
        .get("data")
        .ok_or(InvoiceParseError::MissingData)?
        .as_array()
        .ok_or(InvoiceParseError::MissingData)?;
    let total = object
        .get("count")
        .and_then(|value| match value {
            serde_json::Value::Number(number) => number
                .as_u64()
                .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok())),
            serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
            _ => None,
        })
        .ok_or(InvoiceParseError::MissingCount)?;
    if data.len() > MAX_ROWS {
        return Err(InvoiceParseError::TooLarge);
    }
    let mut rows = Vec::with_capacity(data.len());
    for (index, entry) in data.iter().enumerate() {
        rows.push(parse_invoice_row(entry, index)?);
    }
    Ok(InvoiceListRows { rows, total })
}

fn parse_invoice_row(
    entry: &serde_json::Value,
    index: usize,
) -> Result<InvoiceRow, InvoiceParseError> {
    let fields = entry
        .as_object()
        .filter(|fields| fields.len() <= MAX_RECORD_FIELDS)
        .ok_or(InvoiceParseError::UnrecognizedRow { row: index })?;
    let business_no = required_text(fields, "bus_no")
        .ok_or(InvoiceParseError::MissingBusinessNo { row: index })?;
    let uuid =
        required_text(fields, "uuid").ok_or(InvoiceParseError::MissingDocumentId { row: index })?;
    let bill_amount_cents = amount_cents(fields, "bill_amount", index)?;
    let invoice_amount_cents = amount_cents(fields, "inv_amount", index)?;
    let tax_amount_cents = amount_cents(fields, "tax_amount", index)?;
    Ok(InvoiceRow {
        business_no,
        title: text(fields, "financial_item_name"),
        issuer_department: text(fields, "financial_dept_name"),
        payment_item: text(fields, "payment_item_type_name"),
        invoice_no: text(fields, "inv_no"),
        issued_on: text(fields, "inv_date"),
        note: text(fields, "inv_note"),
        kind: text(fields, "inv_typeStr"),
        reimbursable: text(fields, "is_allow_reimbursement") == "1",
        red_letter: text(fields, "inv_isred") == "1",
        bill_amount_cents,
        invoice_amount_cents,
        tax_amount_cents,
        uuid,
    })
}

/// Reads one optional string field, bounded and stripped.  A field the service
/// sent as a number is read as its own token so a numeric identifier is not
/// turned into a parse failure.
fn text(fields: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    let value = match fields.get(key) {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::Bool(value)) => value.to_string(),
        _ => return String::new(),
    };
    let value = value.trim();
    if value.chars().any(char::is_control) {
        return String::new();
    }
    value.chars().take(MAX_TEXT_CHARS).collect()
}

/// Reads a field this module cannot do without.  An absent, blank, or
/// over-long value is a missing value rather than an empty one.
///
/// The document identifier is allowed to hold any printable characters: it is
/// percent-encoded before it reaches a query string, so the URL stays well
/// formed without this module narrowing what the service may issue.
fn required_text(fields: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    let value = match fields.get(key) {
        Some(serde_json::Value::String(value)) => value.trim().to_owned(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        _ => return None,
    };
    if value.is_empty() || value.len() > MAX_TEXT_CHARS || value.chars().any(char::is_control) {
        return None;
    }
    Some(value)
}

/// Reads one amount field as exact integer cents.
///
/// The JSON token is converted arithmetically rather than through `f64`, so a
/// value the service wrote with two decimals comes back with both of them.
fn amount_cents(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    row: usize,
) -> Result<i64, InvoiceParseError> {
    let Some(value) = fields.get(key) else {
        return Ok(0);
    };
    let token = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.trim().to_owned(),
        serde_json::Value::Null => return Ok(0),
        _ => return Err(InvoiceParseError::InvalidAmount { row }),
    };
    crate::money::exact_cents(&token).ok_or(InvoiceParseError::InvalidAmount { row })
}

/// The list endpoint is a legacy `*.do` route: it answers JSON, but some
/// deployments label it `text/html`.  The JSON parse is the real proof, so this
/// only rejects a content type that could not carry a JSON document at all.
fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("application/json")
            || mime.eq_ignore_ascii_case("text/json")
            || mime.eq_ignore_ascii_case("text/plain")
            || mime.eq_ignore_ascii_case("text/html")
    })
}

fn is_document_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("application/pdf")
            || mime.eq_ignore_ascii_case("application/octet-stream")
    })
}

/// Percent-encodes one query value.  The document identifier is a service
/// value, so it is encoded rather than concatenated.
fn percent_encode_query_value(value: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if UNRESERVED.contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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

fn normalize_base_url(mut base_url: Url) -> Result<Url, InvoiceAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(InvoiceAdapterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = opaque_mapping_root(path).unwrap_or_else(|| {
        if path.is_empty() {
            "/".to_owned()
        } else {
            format!("{path}/")
        }
    });
    base_url.set_path(&path);
    Ok(base_url)
}

fn opaque_mapping_root(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let scheme = segments.next()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let token = segments.next()?;
    Some(format!("/{scheme}/{token}/"))
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, InvoiceAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(InvoiceAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| InvoiceAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(InvoiceAdapterError::UnexpectedOrigin);
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
        && path.len() <= MAX_PATH_CHARS
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
    let bytes = path.as_bytes();
    bytes.windows(3).any(|window| {
        window[0] == b'%'
            && hex_value(window[1])
                .zip(hex_value(window[2]))
                .is_some_and(|(high, low)| {
                    matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                })
    })
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
