//! Read-only INFO news request profiles and strict JSON mapping.
//!
//! The INFO portal handoff and its cookie-aware transport live in the
//! existing INFO modules. This file deliberately stops at a relative request
//! plan and a cleaned news record: it does not own cookies, CSRF values,
//! absolute URLs, or HTML documents.
//!
//! The route family and field names are behavior evidence from the local
//! campus-services audit. Deployments may change the WebVPN prefix, source
//! identifiers, channel identifiers, or search labels, so those values are
//! either relative profile inputs or request inputs rather than hidden
//! constants.

use std::fmt;

/// The WebVPN mapping for the `ghxt` legacy news source. A read-only live
/// redirect identified this exact host, and the pinned THU Info policy has a
/// `ghxt` article selector. It is not a general WebVPN mapping permission.
pub(crate) const GHXT_MAPPING_ID: &str =
    "77726476706e69737468656265737421f7ff598869336153301c9aa596522b20a9a1d8152fdfaea2";

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::Url;
use serde_json::{Map, Value};
use thiserror::Error;

const DEFAULT_LIST_PATH: &str = "/b/info/xxfb_fg/xnzx/template/more";
const DEFAULT_SEARCH_PATH: &str = "/b/xnzx/search/info/xxfb_fg/teacher/getMobilePageList";
const DEFAULT_DETAIL_PATH: &str = "/b/info/xxfb_fg/xnzx/template/detail";
const DEFAULT_CSRF_FIELD: &str = "_csrf";
const DEFAULT_LIST_OPERATION_FIELD: &str = "oType";
const DEFAULT_LIST_OPERATION_VALUE: &str = "xs";
const DEFAULT_LIST_SOURCE_FIELD: &str = "lydw";
const DEFAULT_LIST_CHANNEL_FIELD: &str = "lmid";
const DEFAULT_LIST_PAGE_FIELD: &str = "currentPage";
const DEFAULT_LIST_PAGE_SIZE_FIELD: &str = "length";
const DEFAULT_ALL_CHANNEL: &str = "all";
const DEFAULT_SEARCH_FORM_FIELD: &str = "esParamClass";
const DEFAULT_DETAIL_ID_FIELD: &str = "xxid";
const DEFAULT_DETAIL_PREVIEW_FIELD: &str = "preview";
const DEFAULT_MAX_PAGE_SIZE: u32 = 100;

/// The observed HTTP methods for the read-only news operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsHttpMethod {
    Get,
    Post,
}

/// The response wrapper selected by a news request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsFeedKind {
    List,
    Search,
    Favorite,
    Subscription,
}

impl NewsFeedKind {
    fn collection_field(self) -> &'static str {
        match self {
            Self::List => "dataList",
            Self::Search | Self::Favorite | Self::Subscription => "resultsList",
        }
    }
}

/// Identifies the read operation without carrying a URL or session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsOperation {
    List,
    Search,
    Detail,
}

/// Where the caller must add the CSRF value when it turns a plan into a
/// transport request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsParameterPlacement {
    Query,
    Form,
}

/// A CSRF requirement contains only the wire field name and placement.
///
/// The value is intentionally absent. The shared INFO transport or its
/// caller owns the cookie-derived token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsCsrfRequirement {
    pub field: String,
    pub placement: NewsParameterPlacement,
}

/// A request plan with a relative path and non-secret parameters only.
///
/// The query and form collections do not contain a CSRF pair. A caller must
/// inject the value described by csrf at the final transport boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsRequestPlan {
    operation: NewsOperation,
    method: NewsHttpMethod,
    path: String,
    query: Vec<(String, String)>,
    form: Vec<(String, String)>,
    csrf: NewsCsrfRequirement,
}

impl NewsRequestPlan {
    pub fn operation(&self) -> NewsOperation {
        self.operation
    }

    pub fn method(&self) -> NewsHttpMethod {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query_parameters(&self) -> &[(String, String)] {
        &self.query
    }

    pub fn form_parameters(&self) -> &[(String, String)] {
        &self.form
    }

    pub fn csrf_requirement(&self) -> &NewsCsrfRequirement {
        &self.csrf
    }
}

impl fmt::Debug for NewsRequestPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsRequestPlan")
            .field("operation", &self.operation)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &RedactedParameters(&self.query))
            .field("form", &RedactedParameters(&self.form))
            .field("csrf", &self.csrf)
            .finish()
    }
}

/// Relative-path and wire-field configuration for the list endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsListRouteProfile {
    pub path: String,
    pub operation_field: String,
    pub operation_value: String,
    pub source_field: String,
    pub channel_field: String,
    pub page_field: String,
    pub page_size_field: String,
    pub csrf_field: String,
    pub csrf_placement: NewsParameterPlacement,
}

/// Relative-path and wire-field configuration for the search endpoint.
///
/// The nested search object names are configurable because the endpoint is a
/// deployment detail. Their standard values are the names observed in the
/// audited profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSearchRouteProfile {
    pub path: String,
    pub csrf_field: String,
    pub csrf_placement: NewsParameterPlacement,
    pub form_field: String,
    pub params_object: String,
    pub filter_object: String,
    pub order_object: String,
    pub keyword_field: String,
    pub tag_field: String,
    pub category_field: String,
    pub channel_filter_field: String,
    pub sort_field: String,
    pub exact_match_field: String,
    pub page_field: String,
}

/// Relative-path and wire-field configuration for the detail handoff.
///
/// The detail response is intentionally not parsed here. The source evidence
/// includes HTML and linked documents, so a document policy must be designed
/// before it can become a stable DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsDetailRouteProfile {
    pub path: String,
    pub id_field: String,
    pub preview_field: String,
    pub csrf_field: String,
    pub csrf_placement: NewsParameterPlacement,
}

/// The independently configurable INFO news profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsProfile {
    pub list: NewsListRouteProfile,
    pub search: NewsSearchRouteProfile,
    pub detail: NewsDetailRouteProfile,
}

impl Default for NewsProfile {
    fn default() -> Self {
        Self::standard()
    }
}

impl NewsProfile {
    /// Returns the relative paths and field names supported by the static
    /// INFO/news evidence.
    pub fn standard() -> Self {
        Self {
            list: NewsListRouteProfile {
                path: DEFAULT_LIST_PATH.to_owned(),
                operation_field: DEFAULT_LIST_OPERATION_FIELD.to_owned(),
                operation_value: DEFAULT_LIST_OPERATION_VALUE.to_owned(),
                source_field: DEFAULT_LIST_SOURCE_FIELD.to_owned(),
                channel_field: DEFAULT_LIST_CHANNEL_FIELD.to_owned(),
                page_field: DEFAULT_LIST_PAGE_FIELD.to_owned(),
                page_size_field: DEFAULT_LIST_PAGE_SIZE_FIELD.to_owned(),
                csrf_field: DEFAULT_CSRF_FIELD.to_owned(),
                csrf_placement: NewsParameterPlacement::Query,
            },
            search: NewsSearchRouteProfile {
                path: DEFAULT_SEARCH_PATH.to_owned(),
                csrf_field: DEFAULT_CSRF_FIELD.to_owned(),
                csrf_placement: NewsParameterPlacement::Query,
                form_field: DEFAULT_SEARCH_FORM_FIELD.to_owned(),
                params_object: "params".to_owned(),
                filter_object: "filterParams".to_owned(),
                order_object: "orderMap".to_owned(),
                keyword_field: "bt".to_owned(),
                tag_field: "tag".to_owned(),
                category_field: "xxfl".to_owned(),
                channel_filter_field: "lmmcgroup".to_owned(),
                sort_field: "sort".to_owned(),
                exact_match_field: "matchExact".to_owned(),
                page_field: "currentPage".to_owned(),
            },
            detail: NewsDetailRouteProfile {
                path: DEFAULT_DETAIL_PATH.to_owned(),
                id_field: DEFAULT_DETAIL_ID_FIELD.to_owned(),
                preview_field: DEFAULT_DETAIL_PREVIEW_FIELD.to_owned(),
                csrf_field: DEFAULT_CSRF_FIELD.to_owned(),
                csrf_placement: NewsParameterPlacement::Query,
            },
        }
    }

    pub fn validate(&self) -> Result<(), NewsProfileError> {
        validate_path("list.path", &self.list.path)?;
        validate_path("search.path", &self.search.path)?;
        validate_path("detail.path", &self.detail.path)?;

        for (field, value) in [
            ("list.operation_field", self.list.operation_field.as_str()),
            ("list.source_field", self.list.source_field.as_str()),
            ("list.channel_field", self.list.channel_field.as_str()),
            ("list.page_field", self.list.page_field.as_str()),
            ("list.page_size_field", self.list.page_size_field.as_str()),
            ("list.csrf_field", self.list.csrf_field.as_str()),
            ("search.csrf_field", self.search.csrf_field.as_str()),
            ("search.form_field", self.search.form_field.as_str()),
            ("search.params_object", self.search.params_object.as_str()),
            ("search.filter_object", self.search.filter_object.as_str()),
            ("search.order_object", self.search.order_object.as_str()),
            ("search.keyword_field", self.search.keyword_field.as_str()),
            ("search.tag_field", self.search.tag_field.as_str()),
            ("search.category_field", self.search.category_field.as_str()),
            (
                "search.channel_filter_field",
                self.search.channel_filter_field.as_str(),
            ),
            ("search.sort_field", self.search.sort_field.as_str()),
            (
                "search.exact_match_field",
                self.search.exact_match_field.as_str(),
            ),
            ("search.page_field", self.search.page_field.as_str()),
            ("detail.id_field", self.detail.id_field.as_str()),
            ("detail.preview_field", self.detail.preview_field.as_str()),
            ("detail.csrf_field", self.detail.csrf_field.as_str()),
        ] {
            validate_wire_name(field, value)?;
        }

        if self.list.operation_value.trim().is_empty() {
            return Err(NewsProfileError::InvalidValue {
                field: "list.operation_value".to_owned(),
            });
        }
        validate_wire_value("list.operation_value", &self.list.operation_value)?;
        Ok(())
    }

    /// Builds a list GET plan. Source and channel values are caller-provided
    /// because their deployment-specific identifiers are not stable evidence.
    pub fn list_request(
        &self,
        page: u32,
        page_size: u32,
        source_id: Option<&str>,
        channel_id: Option<&str>,
    ) -> Result<NewsRequestPlan, NewsProfileError> {
        self.validate()?;
        validate_page(page)?;
        validate_page_size(page_size)?;
        let source = optional_wire_value("source_id", source_id)?;
        let channel = optional_wire_value("channel_id", channel_id)?;

        let query = vec![
            (
                self.list.operation_field.clone(),
                self.list.operation_value.clone(),
            ),
            (self.list.source_field.clone(), source.unwrap_or_default()),
            (
                self.list.channel_field.clone(),
                channel.unwrap_or_else(|| DEFAULT_ALL_CHANNEL.to_owned()),
            ),
            (self.list.page_field.clone(), page.to_string()),
            (self.list.page_size_field.clone(), page_size.to_string()),
        ];

        Ok(NewsRequestPlan {
            operation: NewsOperation::List,
            method: NewsHttpMethod::Get,
            path: self.list.path.clone(),
            query,
            form: Vec::new(),
            csrf: NewsCsrfRequirement {
                field: self.list.csrf_field.clone(),
                placement: self.list.csrf_placement,
            },
        })
    }

    /// Builds a search POST plan. The localized channel label, when present,
    /// is supplied by the caller from a parsed channel list or another
    /// deployment-owned mapping; this module does not hard-code one.
    pub fn search_request(
        &self,
        input: &NewsSearchInput,
    ) -> Result<NewsRequestPlan, NewsProfileError> {
        self.validate()?;
        validate_page(input.page)?;
        validate_wire_value("search.keyword", &input.keyword)?;
        let channel_filter = optional_wire_value(
            "search.channel_filter_label",
            input.channel_filter_label.as_deref(),
        )?;

        let mut params = Map::new();
        params.insert(
            self.search.keyword_field.clone(),
            Value::String(input.keyword.clone()),
        );
        params.insert(
            self.search.tag_field.clone(),
            Value::String(input.keyword.clone()),
        );
        params.insert(
            self.search.category_field.clone(),
            Value::String(input.keyword.clone()),
        );

        let mut filters = Map::new();
        if let Some(channel_filter) = channel_filter {
            filters.insert(
                self.search.channel_filter_field.clone(),
                Value::String(channel_filter),
            );
        }

        let mut ordering = Map::new();
        ordering.insert(
            self.search.sort_field.clone(),
            Value::String("time".to_owned()),
        );

        let mut payload = Map::new();
        payload.insert(self.search.params_object.clone(), Value::Object(params));
        payload.insert(self.search.filter_object.clone(), Value::Object(filters));
        payload.insert(self.search.order_object.clone(), Value::Object(ordering));
        payload.insert(
            self.search.exact_match_field.clone(),
            Value::String(if input.exact_match { "是" } else { "否" }.to_owned()),
        );
        payload.insert(
            self.search.page_field.clone(),
            Value::Number(input.page.into()),
        );
        let serialized = serde_json::to_string(&Value::Object(payload)).map_err(|error| {
            NewsProfileError::SearchParametersSerialization {
                message: error.to_string(),
            }
        })?;

        Ok(NewsRequestPlan {
            operation: NewsOperation::Search,
            method: NewsHttpMethod::Post,
            path: self.search.path.clone(),
            query: Vec::new(),
            form: vec![(self.search.form_field.clone(), serialized)],
            csrf: NewsCsrfRequirement {
                field: self.search.csrf_field.clone(),
                placement: self.search.csrf_placement,
            },
        })
    }

    /// Builds the observed detail GET plan. The response must be handled by a
    /// document/redirect policy outside this module.
    pub fn detail_request(&self, article_id: &str) -> Result<NewsRequestPlan, NewsProfileError> {
        self.validate()?;
        let article_id = required_article_id(article_id)?;
        let query = vec![
            (self.detail.id_field.clone(), article_id),
            (self.detail.preview_field.clone(), String::new()),
        ];

        Ok(NewsRequestPlan {
            operation: NewsOperation::Detail,
            method: NewsHttpMethod::Get,
            path: self.detail.path.clone(),
            query,
            form: Vec::new(),
            csrf: NewsCsrfRequirement {
                field: self.detail.csrf_field.clone(),
                placement: self.detail.csrf_placement,
            },
        })
    }
}

/// Input for the observed search form. It contains no session or transport
/// values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSearchInput {
    pub keyword: String,
    pub channel_filter_label: Option<String>,
    pub exact_match: bool,
    pub page: u32,
}

impl NewsSearchInput {
    pub fn new(keyword: impl Into<String>, page: u32) -> Self {
        Self {
            keyword: keyword.into(),
            channel_filter_label: None,
            exact_match: false,
            page,
        }
    }

    pub fn with_channel_filter(mut self, label: impl Into<String>) -> Self {
        self.channel_filter_label = Some(label.into());
        self
    }

    pub fn with_exact_match(mut self, exact_match: bool) -> Self {
        self.exact_match = exact_match;
        self
    }
}

/// Errors found while validating a relative INFO/news profile or request
/// input.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NewsProfileError {
    #[error("{field} must be a relative path without a query or fragment: {value}")]
    InvalidPath { field: String, value: String },

    #[error("{field} must be a non-empty wire field name: {value}")]
    InvalidWireName { field: String, value: String },

    #[error("{field} must not be empty or contain control characters")]
    InvalidValue { field: String },

    #[error("page must be at least 1")]
    InvalidPage,

    #[error("page size must be between 1 and {max}")]
    InvalidPageSize { max: u32 },

    #[error("search parameters could not be serialized: {message}")]
    SearchParametersSerialization { message: String },
}

fn validate_path(field: &str, path: &str) -> Result<(), NewsProfileError> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['?', '#'])
        || path.contains("..")
        || path.contains("://")
        || path.contains('\\')
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
        || path.chars().any(char::is_control)
    {
        return Err(NewsProfileError::InvalidPath {
            field: field.to_owned(),
            value: path.to_owned(),
        });
    }
    Ok(())
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
    // Validate the original escape spelling separately. Here we only look
    // for dangerous bytes introduced by one percent-decoding pass, then
    // repeat for nested escapes such as `%252e%252e`. This preserves ordinary
    // encoded punctuation while still rejecting an encoded traversal path.
    let mut candidate = path.to_owned();
    loop {
        if !candidate.as_bytes().contains(&b'%') {
            return false;
        }
        if invalid_percent_encoding(&candidate) {
            // A malformed escape at this later decoding level is a literal
            // result of a valid outer `%25` escape. The original caller has
            // already rejected malformed escapes in the supplied value.
            return false;
        }
        let bytes = candidate.as_bytes();
        if bytes.windows(3).any(|window| {
            window[0] == b'%'
                && hex_value(window[1])
                    .zip(hex_value(window[2]))
                    .is_some_and(|(high, low)| {
                        matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                    })
        }) {
            return true;
        }
        let Ok(decoded) = percent_decode(&candidate) else {
            return true;
        };
        if decoded == candidate {
            return false;
        }
        candidate = decoded;
    }
}

fn validate_wire_name(field: &str, value: &str) -> Result<(), NewsProfileError> {
    if value.trim().is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '&' | '=' | '?' | '#'))
    {
        return Err(NewsProfileError::InvalidWireName {
            field: field.to_owned(),
            value: value.to_owned(),
        });
    }
    Ok(())
}

fn validate_wire_value(field: &str, value: &str) -> Result<(), NewsProfileError> {
    if value.chars().any(char::is_control) {
        return Err(NewsProfileError::InvalidValue {
            field: field.to_owned(),
        });
    }
    Ok(())
}

fn required_wire_value(field: &str, value: &str) -> Result<String, NewsProfileError> {
    if value.trim().is_empty() {
        return Err(NewsProfileError::InvalidValue {
            field: field.to_owned(),
        });
    }
    validate_wire_value(field, value)?;
    Ok(value.trim().to_owned())
}

fn required_article_id(value: &str) -> Result<String, NewsProfileError> {
    let value = required_wire_value("article_id", value)?;
    if value.chars().any(|character| {
        character.is_whitespace()
            || matches!(
                character,
                '/' | '\\' | '?' | '#' | '&' | '=' | ':' | '<' | '>'
            )
    }) || invalid_percent_encoding(&value)
        || path_contains_encoded_escape(&value)
    {
        return Err(NewsProfileError::InvalidValue {
            field: "article_id".to_owned(),
        });
    }
    Ok(value)
}

fn optional_wire_value(
    field: &str,
    value: Option<&str>,
) -> Result<Option<String>, NewsProfileError> {
    value
        .map(|value| required_wire_value(field, value))
        .transpose()
}

fn validate_page(page: u32) -> Result<(), NewsProfileError> {
    if page == 0 {
        Err(NewsProfileError::InvalidPage)
    } else {
        Ok(())
    }
}

fn validate_page_size(page_size: u32) -> Result<(), NewsProfileError> {
    if page_size == 0 || page_size > DEFAULT_MAX_PAGE_SIZE {
        Err(NewsProfileError::InvalidPageSize {
            max: DEFAULT_MAX_PAGE_SIZE,
        })
    } else {
        Ok(())
    }
}

/// A cleaned article link. It remains opaque: callers must apply their own
/// redirect and allowed-origin policy before fetching it.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsLink(String);

impl NewsLink {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NewsLink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("NewsLink")
            .field(&"[opaque]")
            .finish()
    }
}

/// A channel identifier supplied by the INFO response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsChannelId(String);

impl NewsChannelId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The stable news record exposed by the parser.
///
/// Unknown response fields are discarded. The article link is opaque and
/// redacted in Debug output; this module does not resolve or fetch it.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub link: NewsLink,
    pub published_at: String,
    pub source: String,
    pub topped: bool,
    pub channel: NewsChannelId,
    pub favorited: bool,
}

impl fmt::Debug for NewsItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsItem")
            .field("id", &"[redacted]")
            .field("title", &self.title)
            .field("link", &self.link)
            .field("published_at", &self.published_at)
            .field("source", &self.source)
            .field("topped", &self.topped)
            .field("channel", &self.channel)
            .field("favorited", &self.favorited)
            .finish()
    }
}

/// A successful page with its operation kind and cleaned records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsPage {
    pub feed: NewsFeedKind,
    pub items: Vec<NewsItem>,
}

/// A normalized read-only INFO article detail.
///
/// The INFO API returns the article body as encoded HTML.  The body is
/// decoded here, while attachment identifiers are kept as data so a caller
/// cannot accidentally manufacture a URL containing the session CSRF token.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsDetail {
    pub id: String,
    pub title: String,
    pub content_html: String,
    pub summary: String,
    pub attachments: Vec<NewsAttachment>,
}

impl fmt::Debug for NewsDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsDetail")
            .field("id", &"[redacted]")
            .field("title", &self.title)
            .field("content_html", &"[redacted]")
            .field("summary", &"[redacted]")
            .field("attachments", &self.attachments)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct NewsAttachment {
    pub id: String,
    pub name: String,
}

impl fmt::Debug for NewsAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsAttachment")
            .field("id", &"[redacted]")
            .field("name", &self.name)
            .finish()
    }
}

impl NewsPage {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn classification(&self) -> NewsPageClassification {
        if self.items.is_empty() {
            NewsPageClassification::EmptyList
        } else {
            NewsPageClassification::Data
        }
    }
}

/// Explicit success classification for a page with zero or more records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsPageClassification {
    Data,
    EmptyList,
}

/// The parser keeps an empty valid list distinct from a malformed response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewsParseOutcome {
    Data(NewsPage),
    EmptyList(NewsPage),
}

impl NewsParseOutcome {
    pub fn page(&self) -> &NewsPage {
        match self {
            Self::Data(page) | Self::EmptyList(page) => page,
        }
    }

    pub fn classification(&self) -> NewsPageClassification {
        match self {
            Self::Data(_) => NewsPageClassification::Data,
            Self::EmptyList(_) => NewsPageClassification::EmptyList,
        }
    }

    pub fn into_page(self) -> NewsPage {
        match self {
            Self::Data(page) | Self::EmptyList(page) => page,
        }
    }
}

/// HTML response classification without retaining the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsHtmlClassification {
    LoginPage,
    OtherHtml,
}

/// Structural errors in an otherwise JSON-shaped response.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NewsPayloadError {
    #[error("response root must be an object")]
    RootNotObject,

    #[error("response object is missing")]
    MissingObject,

    #[error("response object must be an object")]
    ObjectNotObject,

    #[error("response is missing {field}")]
    MissingCollection { field: String },

    #[error("{field} must be an array")]
    CollectionNotArray { field: String },

    #[error("record at index {index} must be an object")]
    RecordNotObject { index: usize },

    #[error("record at index {index} is missing {field}")]
    MissingField { index: usize, field: String },

    #[error("record at index {index} field {field} must be {expected}")]
    WrongFieldType {
        index: usize,
        field: String,
        expected: &'static str,
    },

    #[error("record at index {index} field {field} is invalid: {reason}")]
    InvalidField {
        index: usize,
        field: String,
        reason: String,
    },

    #[error("result must be a string when present")]
    ResultNotString,

    #[error("success must be a boolean when present")]
    SuccessNotBoolean,
}

/// Errors at the news response boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NewsParseError {
    #[error("news response is an HTML login page")]
    HtmlLoginPage,

    #[error("news response indicates that the INFO login or session is required")]
    LoginRequired,

    #[error("news response is HTML and has no supported JSON record shape")]
    HtmlPage,

    #[error("news response JSON is malformed: {message}")]
    MalformedJson { message: String },

    #[error("news response payload is malformed: {source}")]
    MalformedPayload {
        #[source]
        source: NewsPayloadError,
    },

    #[error("news service returned a business failure: {message:?}")]
    BusinessFailure { message: Option<String> },

    #[error("news detail JSON is malformed: {message}")]
    DetailMalformedJson { message: String },

    #[error("news detail payload is malformed: {field}: {reason}")]
    DetailMalformedPayload { field: String, reason: String },

    #[error("news detail service returned a business failure: {message:?}")]
    DetailBusinessFailure { message: Option<String> },

    #[error("news catalog payload is malformed: {kind}")]
    CatalogMalformedPayload { kind: &'static str },

    #[error("news article link did not contain a usable xxid")]
    DetailLinkMissingId,

    #[error("news article file link did not contain a usable file id")]
    DetailLinkMissingFileId,
}

/// Parses a list response with object.dataList.
pub fn parse_news_list(body: &str) -> Result<NewsParseOutcome, NewsParseError> {
    parse_news_page(body, NewsFeedKind::List)
}

/// Parses a search response with object.resultsList.
pub fn parse_news_search(body: &str) -> Result<NewsParseOutcome, NewsParseError> {
    parse_news_page(body, NewsFeedKind::Search)
}

/// Parses the JSON returned by the INFO detail endpoint.
///
/// The public reference client treats the detail endpoint as an API response
/// containing `object.xxDto`.  A 200 login document is rejected before JSON
/// parsing, so it can never be mistaken for an article with empty content.
pub fn parse_news_detail(body: &str) -> Result<NewsDetail, NewsParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        if let Some(classification) = classify_html(body) {
            return match classification {
                NewsHtmlClassification::LoginPage => Err(NewsParseError::HtmlLoginPage),
                NewsHtmlClassification::OtherHtml => Err(NewsParseError::HtmlPage),
            };
        }

        let json_body = body.trim_start_matches('\u{feff}');
        let root: Value = serde_json::from_str(json_body).map_err(|error| {
            NewsParseError::DetailMalformedJson {
                message: error.to_string(),
            }
        })?;
        let root_object = root
            .as_object()
            .ok_or_else(|| detail_malformed("root", "response root must be an object"))?;
        if let Err(error) = inspect_news_envelope(root_object) {
            return Err(match error {
                NewsEnvelopeError::Malformed(source) => match source {
                    NewsPayloadError::ResultNotString => {
                        detail_malformed("result", "result must be a string when present")
                    }
                    NewsPayloadError::SuccessNotBoolean => {
                        detail_malformed("success", "success must be a boolean when present")
                    }
                    source => detail_malformed("response", &source.to_string()),
                },
                NewsEnvelopeError::LoginRequired => NewsParseError::LoginRequired,
                NewsEnvelopeError::BusinessFailure { message } => {
                    NewsParseError::DetailBusinessFailure { message }
                }
            });
        }

        let object = root_object
            .get("object")
            .and_then(Value::as_object)
            .ok_or_else(|| detail_malformed("object", "object must be an object"))?;
        let dto = object
            .get("xxDto")
            .and_then(Value::as_object)
            .ok_or_else(|| detail_malformed("object.xxDto", "xxDto must be an object"))?;
        let title = detail_required_string(dto, "bt")?;
        let content = detail_required_string(dto, "nr")?;
        // The reference uses HTML entity decoding, not decodeURIComponent.
        // A literal '%' in prose and percent escapes inside embedded URLs must
        // remain untouched; neither field is an encoded request parameter.
        let title = decode_html_entities_only(title)
            .and_then(|title| strip_html_markup(&title))
            .map(|title| collapse_whitespace(&title))
            .map_err(|reason| detail_malformed("object.xxDto.bt", &reason))?;
        if title.is_empty() || title.chars().any(char::is_control) {
            return Err(detail_malformed("object.xxDto.bt", "invalid title"));
        }
        let content_html = decode_html_entities_only(content)
            .map_err(|reason| detail_malformed("object.xxDto.nr", &reason))?;
        if content_html.trim().is_empty() {
            return Err(detail_malformed("object.xxDto.nr", "content is empty"));
        }
        validate_detail_html(&content_html)
            .map_err(|reason| detail_malformed("object.xxDto.nr", &reason))?;
        let summary = strip_html_markup(&content_html)
            .map(|value| collapse_whitespace(&value))
            .map_err(|reason| detail_malformed("object.xxDto.nr", &reason))?;
        // Some valid detail responses omit xxid. Keep the parser's id empty in
        // that case so the caller can fill it with the request's article_id. If
        // xxid is present, it must still be a non-empty normalized identifier.
        let id = if dto.contains_key("xxid") {
            let id = detail_required_string(dto, "xxid")?;
            normalize_identifier(id, "xxid")
                .map_err(|reason| detail_malformed("object.xxDto.xxid", &reason))?
        } else {
            String::new()
        };

        let attachments = match dto.get("fjs_template") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .enumerate()
                .map(|(index, value)| parse_attachment(value, index))
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => {
                return Err(detail_malformed(
                    "object.xxDto.fjs_template",
                    "attachments must be an array or null",
                ));
            }
        };

        Ok(NewsDetail {
            id,
            title,
            content_html,
            summary,
            attachments,
        })
    })
}

/// Parses the HTML document returned by a legacy INFO news link.
///
/// The current INFO application normally exposes `var xxid` and the JSON
/// detail endpoint.  Older entries still point at department JSP/CMS pages,
/// however.  Those pages do not share one DOM shape, so this parser selects
/// an article heading and a content-bearing element from structural HTML
/// evidence.  It deliberately rejects a generic HTML document when it cannot
/// prove both pieces of article content.
pub fn parse_news_legacy_detail(body: &str) -> Result<NewsDetail, NewsParseError> {
    match classify_html(body) {
        Some(NewsHtmlClassification::LoginPage) => return Err(NewsParseError::HtmlLoginPage),
        Some(NewsHtmlClassification::OtherHtml) => {}
        None => return Err(NewsParseError::HtmlPage),
    }

    let elements =
        collect_legacy_elements(body).map_err(|reason| detail_malformed("legacy.html", &reason))?;
    let title = select_legacy_title(&elements)
        .ok_or_else(|| detail_malformed("legacy.title", "article title was not found"))?;
    let content = select_legacy_content(&elements, &title.text)
        .ok_or_else(|| detail_malformed("legacy.content", "article content was not found"))?;
    let content_html = sanitize_legacy_fragment(&content.html)
        .map_err(|reason| detail_malformed("legacy.content", &reason))?;
    if content_html.trim().is_empty() {
        return Err(detail_malformed(
            "legacy.content",
            "article content is empty",
        ));
    }
    validate_detail_html(&content_html)
        .map_err(|reason| detail_malformed("legacy.content", &reason))?;
    let summary =
        legacy_text(&content_html).map_err(|reason| detail_malformed("legacy.content", &reason))?;
    if summary.is_empty() || summary == title.text {
        return Err(detail_malformed(
            "legacy.content",
            "article content does not contain a distinct body",
        ));
    }

    Ok(NewsDetail {
        id: String::new(),
        title: title.text,
        content_html,
        summary,
        attachments: Vec::new(),
    })
}

/// Parses a PDF response from the INFO file stream route.
///
/// The bridge currently carries text fields, so a verified PDF is represented
/// by its base64 payload in `content_html`.  A PDF magic header and a terminal
/// `%%EOF` marker are both required; an arbitrary binary or an empty response
/// cannot become a successful detail.
pub fn parse_news_pdf(body: &[u8]) -> Result<NewsDetail, NewsParseError> {
    if let Ok(text) = std::str::from_utf8(body) {
        if let Some(classification) = classify_html(text) {
            return match classification {
                NewsHtmlClassification::LoginPage => Err(NewsParseError::HtmlLoginPage),
                NewsHtmlClassification::OtherHtml => Err(NewsParseError::HtmlPage),
            };
        }
    }
    if body.len() < 12
        || !body.starts_with(b"%PDF-")
        || !body
            .windows(b"%%EOF".len())
            .any(|window| window == b"%%EOF")
    {
        return Err(detail_malformed(
            "pdf",
            "response is not a complete PDF document",
        ));
    }

    Ok(NewsDetail {
        id: String::new(),
        title: "PdF".to_owned(),
        content_html: BASE64_STANDARD.encode(body),
        summary: "PdF".to_owned(),
        attachments: Vec::new(),
    })
}

/// Parses the JSON envelope used by the older system publication PDF route.
///
/// That route returns a non-empty `content` string instead of raw PDF bytes;
/// it is kept separate from the raw PDF parser so an arbitrary JSON object
/// cannot be promoted to a successful article.
pub fn parse_news_system_pdf(body: &str) -> Result<NewsDetail, NewsParseError> {
    if let Some(classification) = classify_html(body) {
        return match classification {
            NewsHtmlClassification::LoginPage => Err(NewsParseError::HtmlLoginPage),
            NewsHtmlClassification::OtherHtml => Err(NewsParseError::HtmlPage),
        };
    }
    let json_body = body.trim_start_matches('\u{feff}');
    let root: Value =
        serde_json::from_str(json_body).map_err(|error| NewsParseError::DetailMalformedJson {
            message: error.to_string(),
        })?;
    let root_object = root
        .as_object()
        .ok_or_else(|| detail_malformed("root", "response root must be an object"))?;
    if let Err(error) = inspect_news_envelope(root_object) {
        return Err(match error {
            NewsEnvelopeError::Malformed(source) => {
                detail_malformed("response", &source.to_string())
            }
            NewsEnvelopeError::LoginRequired => NewsParseError::LoginRequired,
            NewsEnvelopeError::BusinessFailure { message } => {
                NewsParseError::DetailBusinessFailure { message }
            }
        });
    }

    let content = root_object
        .get("content")
        .or_else(|| {
            root_object
                .get("object")
                .and_then(|value| value.get("content"))
        })
        .ok_or_else(|| detail_malformed("content", "content field is missing"))?
        .as_str()
        .ok_or_else(|| detail_malformed("content", "content field must be a string"))?;
    if content.trim().is_empty() {
        return Err(detail_malformed("content", "content is empty"));
    }

    let content_html = if content.trim_start().starts_with('<') {
        sanitize_legacy_fragment(content).map_err(|reason| detail_malformed("content", &reason))?
    } else {
        content.trim().to_owned()
    };
    if content_html.trim().is_empty() {
        return Err(detail_malformed("content", "content is empty"));
    }
    let summary = if content_html.trim_start().starts_with('<') {
        legacy_text(&content_html).map_err(|reason| detail_malformed("content", &reason))?
    } else {
        "PdF".to_owned()
    };

    Ok(NewsDetail {
        id: String::new(),
        title: "PdF".to_owned(),
        content_html,
        summary: if summary.is_empty() {
            "PdF".to_owned()
        } else {
            summary
        },
        attachments: Vec::new(),
    })
}

#[derive(Debug, Clone)]
struct LegacyElement {
    tag: String,
    attrs: String,
    html: String,
    text: String,
    order: usize,
}

struct OpenLegacyElement {
    tag: String,
    attrs: String,
    content_start: usize,
    order: usize,
}

fn select_legacy_title(elements: &[LegacyElement]) -> Option<LegacyElement> {
    elements
        .iter()
        .filter(|element| !element.text.is_empty() && element.text.chars().count() <= 512)
        .filter_map(|element| {
            let tag_score = match element.tag.as_str() {
                "h1" => 240,
                "h2" => 225,
                "h3" => 210,
                "h4" | "h5" | "h6" => 190,
                "title" => 70,
                _ => 0,
            };
            let attr_score = score_legacy_attributes(
                &element.attrs,
                &[
                    "title", "headline", "subject", "biaoti", "bt", "td1", "td4", "title_b",
                ],
            );
            let score = tag_score + attr_score;
            (score > 0).then(|| (score, std::cmp::Reverse(element.order), element.clone()))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
        .map(|(_, _, element)| element)
}

fn select_legacy_content(elements: &[LegacyElement], title: &str) -> Option<LegacyElement> {
    elements
        .iter()
        .filter(|element| !element.text.is_empty() && element.text != title)
        .filter_map(|element| {
            let tag_score = match element.tag.as_str() {
                "article" => 260,
                "main" => 245,
                "section" => 175,
                "p" => 95,
                "td" => 65,
                "div" => 20,
                _ => 0,
            };
            let attr_score = score_legacy_attributes(
                &element.attrs,
                &[
                    "content",
                    "article",
                    "detail",
                    "concon",
                    "xqbox",
                    "field-item",
                    "wordsection",
                    "r_cont",
                    "sideleft",
                    "td1",
                    "td4",
                    "style1",
                ],
            );
            let table_score =
                if element.tag == "td" && element.attrs.to_ascii_lowercase().contains("colspan") {
                    100
                } else {
                    0
                };
            let length_score = element.text.chars().count().min(240) as i32 / 8;
            let score = tag_score + attr_score + table_score + length_score;
            (score > 0).then(|| (score, std::cmp::Reverse(element.order), element.clone()))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
        .map(|(_, _, element)| element)
}

fn score_legacy_attributes(attrs: &str, markers: &[&str]) -> i32 {
    let lower = attrs.to_ascii_lowercase();
    markers
        .iter()
        .filter(|marker| lower.contains(**marker))
        .map(|marker| {
            if matches!(*marker, "content" | "article" | "detail" | "title") {
                150
            } else {
                90
            }
        })
        .sum()
}

fn collect_legacy_elements(body: &str) -> Result<Vec<LegacyElement>, String> {
    let mut elements = Vec::new();
    let mut stack = Vec::<OpenLegacyElement>::new();
    let mut cursor = 0;
    let mut order = 0;
    while let Some(relative_start) = body[cursor..].find('<') {
        let start = cursor + relative_start;
        if body[start..].starts_with("<!--") {
            let Some(relative_end) = body[start + 4..].find("-->") else {
                break;
            };
            cursor = start + 4 + relative_end + 3;
            continue;
        }
        let end = find_html_tag_end(body, start + 1)?;
        let raw = body[start + 1..end].trim();
        cursor = end + 1;
        if raw.is_empty() || raw.starts_with('!') || raw.starts_with('?') {
            continue;
        }
        let closing = raw.starts_with('/');
        let raw = raw.strip_prefix('/').unwrap_or(raw).trim_start();
        let Some(name_end) = raw
            .char_indices()
            .find(|(_, character)| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | ':')
            })
            .map(|(index, _)| index)
            .unwrap_or_else(|| raw.len())
            .checked_sub(0)
        else {
            continue;
        };
        if name_end == 0 {
            continue;
        }
        let tag = raw[..name_end].to_ascii_lowercase();
        if closing {
            let Some(position) = stack.iter().rposition(|open| open.tag == tag) else {
                continue;
            };
            while stack.len() > position + 1 {
                stack.pop();
            }
            let Some(open) = stack.pop() else {
                continue;
            };
            if open.content_start <= start {
                let html = body[open.content_start..start].to_owned();
                let text = legacy_text(&html)?;
                elements.push(LegacyElement {
                    tag: open.tag,
                    attrs: open.attrs,
                    html,
                    text,
                    order: open.order,
                });
            }
            continue;
        }
        if is_void_html_tag(&tag) || raw.trim_end().ends_with('/') {
            continue;
        }
        stack.push(OpenLegacyElement {
            tag,
            attrs: raw[name_end..]
                .trim()
                .trim_end_matches('/')
                .trim()
                .to_owned(),
            content_start: end + 1,
            order,
        });
        order += 1;
    }
    Ok(elements)
}

fn find_html_tag_end(body: &str, start: usize) -> Result<usize, String> {
    let bytes = body.as_bytes();
    let mut quote = None;
    for index in start..bytes.len() {
        match (quote, bytes[index]) {
            (None, b'\'' | b'"') => quote = Some(bytes[index]),
            (Some(value), byte) if value == byte => quote = None,
            (None, b'>') => return Ok(index),
            _ => {}
        }
    }
    Err("HTML tag is not terminated".to_owned())
}

fn is_void_html_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn legacy_text(value: &str) -> Result<String, String> {
    let value = drop_legacy_blocks(value);
    let value = strip_html_markup(&value)?;
    let value = decode_html_entities(&value)?;
    Ok(collapse_whitespace(&value))
}

fn drop_legacy_blocks(value: &str) -> String {
    let mut output = value.to_owned();
    for tag in [
        "script", "style", "noscript", "iframe", "object", "embed", "form",
    ] {
        loop {
            let lower = output.to_ascii_lowercase();
            let marker = format!("<{tag}");
            let Some(start) = lower.find(&marker) else {
                break;
            };
            let after_name = start + marker.len();
            if lower
                .as_bytes()
                .get(after_name)
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            {
                let next = after_name;
                if next >= output.len() {
                    break;
                }
                output.replace_range(start..next, "");
                continue;
            }
            let open_end = match lower[after_name..].find('>') {
                Some(relative) => after_name + relative + 1,
                None => {
                    output.truncate(start);
                    break;
                }
            };
            let close_marker = format!("</{tag}>");
            let end = lower[open_end..]
                .find(&close_marker)
                .map(|relative| open_end + relative + close_marker.len())
                .unwrap_or(output.len());
            output.replace_range(start..end, "");
        }
    }
    output
}

fn sanitize_legacy_fragment(value: &str) -> Result<String, String> {
    let value = drop_legacy_blocks(value);
    let allowed = [
        "a",
        "b",
        "blockquote",
        "br",
        "caption",
        "code",
        "em",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "i",
        "li",
        "ol",
        "p",
        "pre",
        "section",
        "strong",
        "sub",
        "sup",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "tr",
        "u",
        "ul",
    ];
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative_start) = value[cursor..].find('<') {
        let start = cursor + relative_start;
        output.push_str(&value[cursor..start]);
        let end = find_html_tag_end(&value, start + 1)?;
        let raw = value[start + 1..end].trim();
        if raw.starts_with("!--") {
            cursor = end + 1;
            continue;
        }
        let closing = raw.starts_with('/');
        let raw = raw.strip_prefix('/').unwrap_or(raw).trim_start();
        let Some(name_end) = raw
            .char_indices()
            .find(|(_, character)| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | ':')
            })
            .map(|(index, _)| index)
            .unwrap_or_else(|| raw.len())
            .checked_sub(0)
        else {
            cursor = end + 1;
            continue;
        };
        let tag = raw[..name_end].to_ascii_lowercase();
        if allowed.contains(&tag.as_str()) {
            if closing {
                output.push_str("</");
                output.push_str(&tag);
                output.push('>');
            } else if is_void_html_tag(&tag) {
                output.push('<');
                output.push_str(&tag);
                output.push('>');
            } else {
                output.push('<');
                output.push_str(&tag);
                output.push('>');
            }
        }
        cursor = end + 1;
    }
    output.push_str(&value[cursor..]);
    Ok(output)
}

fn detail_malformed(field: &str, reason: &str) -> NewsParseError {
    NewsParseError::DetailMalformedPayload {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}

fn detail_required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, NewsParseError> {
    object
        .get(field)
        .ok_or_else(|| detail_malformed(field, "field is missing"))?
        .as_str()
        .ok_or_else(|| detail_malformed(field, "field must be a string"))
}

fn parse_attachment(value: &Value, index: usize) -> Result<NewsAttachment, NewsParseError> {
    let object = value.as_object().ok_or_else(|| {
        detail_malformed(
            &format!("object.xxDto.fjs_template[{index}]"),
            "attachment must be an object",
        )
    })?;
    let id = detail_required_string(object, "wjid")?;
    let name = detail_required_string(object, "wjmc")?;
    let id = normalize_identifier(id, "wjid")
        .map_err(|reason| detail_malformed("attachment.wjid", &reason))?;
    let name = normalize_plain_text(name, "wjmc")
        .map_err(|reason| detail_malformed("attachment.wjmc", &reason))?;
    Ok(NewsAttachment { id, name })
}

fn validate_detail_html(value: &str) -> Result<(), String> {
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err("content contains a control character".to_owned());
    }
    Ok(())
}

/// Parses one strict list or search JSON response.
///
/// A successful empty collection returns NewsParseOutcome::EmptyList. A
/// non-success result, login document, malformed JSON, missing collection, or
/// malformed record is never converted into an empty page.
pub fn parse_news_page(body: &str, feed: NewsFeedKind) -> Result<NewsParseOutcome, NewsParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        if let Some(classification) = classify_html(body) {
            return match classification {
                NewsHtmlClassification::LoginPage => Err(NewsParseError::HtmlLoginPage),
                NewsHtmlClassification::OtherHtml => Err(NewsParseError::HtmlPage),
            };
        }

        let json_body = body.trim_start_matches('\u{feff}');
        let root: Value =
            serde_json::from_str(json_body).map_err(|error| NewsParseError::MalformedJson {
                message: error.to_string(),
            })?;
        let root_object = root
            .as_object()
            .ok_or_else(|| malformed(NewsPayloadError::RootNotObject))?;

        if let Err(error) = inspect_news_envelope(root_object) {
            return Err(match error {
                NewsEnvelopeError::Malformed(source) => malformed(source),
                NewsEnvelopeError::LoginRequired => NewsParseError::LoginRequired,
                NewsEnvelopeError::BusinessFailure { message } => {
                    NewsParseError::BusinessFailure { message }
                }
            });
        }

        let object = root_object
            .get("object")
            .ok_or_else(|| malformed(NewsPayloadError::MissingObject))?
            .as_object()
            .ok_or_else(|| malformed(NewsPayloadError::ObjectNotObject))?;
        let collection_field = feed.collection_field();
        let collection = object
            .get(collection_field)
            .ok_or_else(|| {
                malformed(NewsPayloadError::MissingCollection {
                    field: collection_field.to_owned(),
                })
            })?
            .as_array()
            .ok_or_else(|| {
                malformed(NewsPayloadError::CollectionNotArray {
                    field: collection_field.to_owned(),
                })
            })?;

        let items = collection
            .iter()
            .enumerate()
            .map(|(index, value)| parse_news_item(value, index, feed))
            .collect::<Result<Vec<_>, _>>()
            .map_err(malformed)?;
        let page = NewsPage { feed, items };
        if page.is_empty() {
            Ok(NewsParseOutcome::EmptyList(page))
        } else {
            Ok(NewsParseOutcome::Data(page))
        }
    })
}

fn malformed(source: NewsPayloadError) -> NewsParseError {
    NewsParseError::MalformedPayload { source }
}

enum NewsEnvelopeError {
    Malformed(NewsPayloadError),
    LoginRequired,
    BusinessFailure { message: Option<String> },
}

fn inspect_news_envelope(root: &Map<String, Value>) -> Result<(), NewsEnvelopeError> {
    if let Some(result) = root.get("result") {
        let result = result
            .as_str()
            .ok_or_else(|| NewsEnvelopeError::Malformed(NewsPayloadError::ResultNotString))?;
        if !result.eq_ignore_ascii_case("success") {
            return Err(classify_business_failure(root, result));
        }
    }

    if let Some(success) = root.get("success") {
        match success {
            Value::Bool(true) => {}
            Value::Bool(false) => return Err(classify_business_failure(root, "success=false")),
            _ => {
                return Err(NewsEnvelopeError::Malformed(
                    NewsPayloadError::SuccessNotBoolean,
                ));
            }
        }
    }

    // Some deployments use an explicit `error` field instead of `result`.
    // Null and false mean that the field carries no failure; every other
    // value is an explicit failure signal and must never be ignored in front
    // of an otherwise empty collection.
    if let Some(error) = root.get("error") {
        match error {
            Value::Null | Value::Bool(false) => {}
            Value::Bool(true) => return Err(classify_business_failure(root, "error")),
            Value::String(value) if value.trim().is_empty() => {}
            Value::String(value) => return Err(classify_business_failure(root, value)),
            _ => return Err(classify_business_failure(root, "error")),
        }
    }

    // A few INFO deployments return a failure envelope with only `message`
    // or `msg`. The collection wrapper may still be present (and may even be
    // empty), so checking only the wrapper would turn an expired session into
    // a valid empty page. Ordinary informational text remains acceptable;
    // only explicit login or failure wording changes the state.
    for field in ["message", "msg"] {
        let Some(message) = root.get(field).and_then(Value::as_str) else {
            continue;
        };
        if is_login_marker(message) {
            return Err(NewsEnvelopeError::LoginRequired);
        }
        if is_failure_marker(message) {
            return Err(NewsEnvelopeError::BusinessFailure {
                message: business_message(root, message),
            });
        }
    }

    Ok(())
}

fn classify_business_failure(root: &Map<String, Value>, fallback: &str) -> NewsEnvelopeError {
    let message = business_message(root, fallback);
    let has_login_message = ["msg", "message", "error"].iter().any(|field| {
        root.get(*field)
            .and_then(Value::as_str)
            .is_some_and(is_login_marker)
    });
    if is_login_marker(fallback)
        || has_login_message
        || message.as_deref().is_some_and(is_login_marker)
    {
        NewsEnvelopeError::LoginRequired
    } else {
        NewsEnvelopeError::BusinessFailure { message }
    }
}

fn parse_news_item(
    value: &Value,
    index: usize,
    feed: NewsFeedKind,
) -> Result<NewsItem, NewsPayloadError> {
    let object = value
        .as_object()
        .ok_or(NewsPayloadError::RecordNotObject { index })?;
    let title = required_string(object, index, "bt")?;
    let link = required_string(object, index, "url")?;
    let id = required_string(object, index, "xxid")?;
    let published_at = required_string(object, index, "time")?;
    let source = required_string(object, index, "dwmc_show")?;
    let channel = required_string(object, index, "lmid")?;

    let title = normalize_title(title).map_err(|reason| NewsPayloadError::InvalidField {
        index,
        field: "bt".to_owned(),
        reason,
    })?;
    let link = normalize_link(link).map_err(|reason| NewsPayloadError::InvalidField {
        index,
        field: "url".to_owned(),
        reason,
    })?;
    let id = normalize_identifier(id, "xxid").map_err(|reason| NewsPayloadError::InvalidField {
        index,
        field: "xxid".to_owned(),
        reason,
    })?;
    let published_at =
        normalize_date(published_at).map_err(|reason| NewsPayloadError::InvalidField {
            index,
            field: "time".to_owned(),
            reason,
        })?;
    let source = normalize_plain_text(source, "dwmc_show").map_err(|reason| {
        NewsPayloadError::InvalidField {
            index,
            field: "dwmc_show".to_owned(),
            reason,
        }
    })?;
    let channel =
        normalize_identifier(channel, "lmid").map_err(|reason| NewsPayloadError::InvalidField {
            index,
            field: "lmid".to_owned(),
            reason,
        })?;

    let topped = parse_topped(object, index, feed)?;
    let favorited = object
        .get("sfsc")
        .ok_or_else(|| missing_field(index, "sfsc"))?
        .as_bool()
        .ok_or_else(|| wrong_type(index, "sfsc", "boolean"))?;

    Ok(NewsItem {
        id,
        title,
        link: NewsLink(link),
        published_at,
        source,
        topped,
        channel: NewsChannelId(channel),
        favorited,
    })
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    index: usize,
    field: &str,
) -> Result<&'a str, NewsPayloadError> {
    object
        .get(field)
        .ok_or_else(|| missing_field(index, field))?
        .as_str()
        .ok_or_else(|| wrong_type(index, field, "string"))
}

fn parse_topped(
    object: &Map<String, Value>,
    index: usize,
    feed: NewsFeedKind,
) -> Result<bool, NewsPayloadError> {
    if matches!(feed, NewsFeedKind::Favorite | NewsFeedKind::Subscription) {
        return Ok(false);
    }
    let value = object
        .get("yxzd")
        .ok_or_else(|| missing_field(index, "yxzd"))?;
    match (feed, value) {
        (NewsFeedKind::List, Value::String(value)) => Ok(value.contains("1-")),
        (NewsFeedKind::Search, Value::String(value)) => Ok(value.contains("1-")),
        (NewsFeedKind::Search, Value::Null) => Ok(false),
        (NewsFeedKind::Favorite | NewsFeedKind::Subscription, _) => Ok(false),
        (NewsFeedKind::List, _) => Err(wrong_type(index, "yxzd", "string")),
        (NewsFeedKind::Search, _) => Err(wrong_type(index, "yxzd", "string or null")),
    }
}

fn missing_field(index: usize, field: &str) -> NewsPayloadError {
    NewsPayloadError::MissingField {
        index,
        field: field.to_owned(),
    }
}

fn wrong_type(index: usize, field: &str, expected: &'static str) -> NewsPayloadError {
    NewsPayloadError::WrongFieldType {
        index,
        field: field.to_owned(),
        expected,
    }
}

fn business_message(root: &Map<String, Value>, result: &str) -> Option<String> {
    let message = ["msg", "message", "error"]
        .iter()
        .find_map(|field| root.get(*field).and_then(scalar_text));
    message
        .or_else(|| {
            let result = result.trim();
            (!result.is_empty()).then(|| result.to_owned())
        })
        .and_then(|message| sanitize_diagnostic(&message))
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn sanitize_diagnostic(value: &str) -> Option<String> {
    let stripped = strip_html_markup(value).ok()?;
    let normalized = collapse_whitespace(&stripped);
    if normalized.is_empty() {
        return None;
    }
    let lowercase = normalized.to_ascii_lowercase();
    if [
        "csrf",
        "cookie",
        "password",
        "token",
        "secret",
        "ticket",
        "authorization",
        "sessionid",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
    {
        return Some("[redacted diagnostic]".to_owned());
    }
    // Business messages are safe diagnostics, not a second response channel.
    // Refuse URL/query-like text and assignment syntax so a server cannot
    // echo a ticket, Cookie, CSRF value, or an entire request body through
    // the public error enum.
    if normalized.chars().any(|character| {
        matches!(
            character,
            '=' | '&' | '?' | '#' | '/' | '\\' | '%' | ':' | '`'
        )
    }) {
        return Some("[redacted diagnostic]".to_owned());
    }
    Some(normalized.chars().take(160).collect())
}

fn is_login_marker(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if ["login success", "登录成功", "认证成功", "会话有效"]
        .iter()
        .any(|marker| value == *marker)
        || value == "logged in"
    {
        return false;
    }
    [
        "not logged in",
        "unauthenticated",
        "login required",
        "authentication required",
        "please login",
        "please log in",
        "need login",
        "login timeout",
        "login failed",
        "authentication failed",
        "session expired",
        "session invalid",
        "unauthorized",
        "未登录",
        "请登录",
        "请先登录",
        "请重新登录",
        "需要登录",
        "登录失效",
        "登录超时",
        "登录失败",
        "未认证",
        "认证过期",
        "认证失效",
        "认证失败",
        "会话过期",
        "会话已过期",
        "会话失效",
        "会话已失效",
        "会话超时",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn is_failure_marker(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || [
            "success",
            "successful",
            "ok",
            "done",
            "completed",
            "成功",
            "完成",
            "正常",
        ]
        .iter()
        .any(|marker| value == *marker)
    {
        return false;
    }
    [
        "error",
        "failed",
        "failure",
        "fail",
        "forbidden",
        "denied",
        "invalid",
        "bad request",
        "错误",
        "失败",
        "异常",
        "拒绝",
        "禁止",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn classify_html(body: &str) -> Option<NewsHtmlClassification> {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    // A JSON response may contain a login marker in its diagnostic message.
    // Leave JSON-shaped documents to the JSON parser so business failures and
    // session failures retain their distinct classifications.
    if matches!(trimmed.as_bytes().first(), Some(b'{' | b'[' | b'"')) {
        return None;
    }
    if !trimmed.starts_with('<') {
        return has_plain_login_marker(trimmed).then_some(NewsHtmlClassification::LoginPage);
    }
    let lowercase = trimmed.to_ascii_lowercase();
    let compact = lowercase
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    let identity_form = contains_input_name(&compact, "i_user")
        && contains_input_name(&compact, "i_pass")
        && (compact.contains("password") || compact.contains("i_pass") || compact.contains("登录"));
    let generic_login_form = lowercase.contains("<form")
        && (lowercase.contains("password")
            || lowercase.contains("username")
            || lowercase.contains("登录"));
    let login_marker = identity_form || generic_login_form || has_plain_login_marker(&lowercase);
    Some(if login_marker {
        NewsHtmlClassification::LoginPage
    } else {
        NewsHtmlClassification::OtherHtml
    })
}

fn has_plain_login_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "/do/off/ui/auth/login",
        "登录失效",
        "登录超时",
        "会话已过期",
        "会话已失效",
        "请先登录",
        "请重新登录",
        "未登录",
        "请登录",
        "session expired",
        "login required",
        "authentication required",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

fn contains_input_name(body: &str, name: &str) -> bool {
    [
        format!("name=\"{name}\""),
        format!("name='{name}'"),
        format!("name={name}"),
    ]
    .iter()
    .any(|candidate| body.contains(candidate))
}

fn normalize_identifier(value: &str, field: &str) -> Result<String, String> {
    let value = decode_encoded_text(value)?;
    let value = collapse_whitespace(&value);
    if value.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(value)
}

fn normalize_plain_text(value: &str, field: &str) -> Result<String, String> {
    let value = decode_encoded_text(value)?;
    let value = collapse_whitespace(&value);
    if value.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(value)
}

fn normalize_title(value: &str) -> Result<String, String> {
    let value = decode_encoded_text(value)?;
    let value = strip_html_markup(&value)?;
    let value = collapse_whitespace(&value);
    if value.is_empty() {
        return Err("title is empty".to_owned());
    }
    if value.chars().any(char::is_control) {
        return Err("title contains a control character".to_owned());
    }
    Ok(value)
}

fn normalize_link(value: &str) -> Result<String, String> {
    // The news list URL is an HTML-escaped opaque link. Decode entities so
    // `&amp;` becomes a query separator, but retain percent escapes such as
    // `%26` and `%2F`; decoding those here changes the URL's query/path
    // boundaries before the transport gets to resolve the link.
    let value = decode_html_entities_only(value)?;
    let value = value.trim();
    if value.is_empty() {
        return Err("link is empty".to_owned());
    }
    if value.chars().any(char::is_control) {
        return Err("link contains a control character".to_owned());
    }
    if invalid_percent_encoding(value) {
        return Err("link contains an invalid percent escape".to_owned());
    }

    // The parser does not choose the final allowed origin; that remains the
    // responsibility of info_session. It does, however, establish a safe URL
    // boundary so a later layer never receives a javascript/data link,
    // network-path reference, userinfo, fragment, or traversal path as if it
    // were an ordinary article link.
    if let Ok(url) = Url::parse(value) {
        if !matches!(url.scheme(), "http" | "https") {
            return Err("link scheme is not http or https".to_owned());
        }
        if url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url
                .fragment()
                .is_some_and(|fragment| !is_safe_publication_fragment(fragment))
        {
            return Err("link absolute URL has an unsafe origin or fragment".to_owned());
        }
        if unsafe_link_path(url.path()) {
            return Err("link path is unsafe".to_owned());
        }
    } else {
        if !value.starts_with('/') || value.starts_with("//") {
            return Err("link must be an origin-relative path or http(s) URL".to_owned());
        }
        let (without_fragment, fragment) = match value.split_once('#') {
            Some((without_fragment, fragment))
                if is_safe_publication_fragment(fragment) && !fragment.contains('#') =>
            {
                (without_fragment, Some(fragment))
            }
            Some(_) => return Err("link contains an unsafe fragment".to_owned()),
            None => (value, None),
        };
        let path = without_fragment
            .split_once('?')
            .map_or(without_fragment, |(path, _)| path);
        if unsafe_link_path(path) {
            return Err("link path is unsafe".to_owned());
        }
        if let Some(fragment) = fragment {
            if fragment.is_empty() {
                return Err("link fragment is empty".to_owned());
            }
        }
    }
    Ok(value.to_owned())
}

fn is_safe_publication_fragment(fragment: &str) -> bool {
    let Some(identifier) = fragment.strip_prefix("/publish/") else {
        return false;
    };
    !identifier.is_empty()
        && !identifier.contains('/')
        && !identifier.contains(['?', '#', '&', '='])
        && !identifier.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '\\'
        })
        && !invalid_percent_encoding(identifier)
        && !path_contains_encoded_escape(identifier)
}

fn unsafe_link_path(path: &str) -> bool {
    path.contains(['\\', '?', '#'])
        || path.chars().any(char::is_control)
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
}

fn normalize_date(value: &str) -> Result<String, String> {
    let value = decode_encoded_text(value)?;
    let mut value = collapse_whitespace(&value)
        .replace('：', ":")
        .replace('／', "/");
    if value.len() > 11
        && matches!(value.as_bytes().get(10), Some(b'T' | b't'))
        && value.as_bytes().get(9).is_some_and(u8::is_ascii_digit)
        && value.as_bytes().get(11).is_some_and(u8::is_ascii_digit)
    {
        value.replace_range(10..11, " ");
    }
    let value = collapse_whitespace(&value);
    if value.is_empty() {
        return Err("published date is empty".to_owned());
    }
    if value.chars().any(char::is_control) {
        return Err("published date contains a control character".to_owned());
    }
    Ok(value)
}

fn decode_encoded_text(value: &str) -> Result<String, String> {
    let value = percent_decode(value)?;
    decode_html_entities(&value)
}

fn decode_html_entities_only(value: &str) -> Result<String, String> {
    decode_html_entities(value)
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err("incomplete percent escape".to_owned());
        }
        let high =
            hex_value(bytes[index + 1]).ok_or_else(|| "invalid percent escape".to_owned())?;
        let low = hex_value(bytes[index + 2]).ok_or_else(|| "invalid percent escape".to_owned())?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| "percent-decoded value is not UTF-8".to_owned())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn decode_html_entities(value: &str) -> Result<String, String> {
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value.as_bytes()[index] != b'&' {
            let character = value[index..]
                .chars()
                .next()
                .ok_or_else(|| "invalid UTF-8 boundary".to_owned())?;
            output.push(character);
            index += character.len_utf8();
            continue;
        }

        let Some(relative_end) = value[index + 1..].find(';') else {
            output.push('&');
            index += 1;
            continue;
        };
        let end = index + 1 + relative_end;
        let entity = &value[index + 1..end];
        if entity.is_empty() || entity.len() > 32 {
            output.push('&');
            index += 1;
            continue;
        }
        if let Some(character) = decode_entity(entity)? {
            output.push(character);
        } else {
            output.push_str(&value[index..=end]);
        }
        index = end + 1;
    }
    Ok(output)
}

fn decode_entity(entity: &str) -> Result<Option<char>, String> {
    let character = match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            let value = u32::from_str_radix(&entity[2..], 16)
                .map_err(|_| "invalid hexadecimal HTML entity".to_owned())?;
            char::from_u32(value)
                .ok_or_else(|| "invalid HTML entity code point".to_owned())?
                .into()
        }
        _ if entity.starts_with('#') => {
            let value = entity[1..]
                .parse::<u32>()
                .map_err(|_| "invalid decimal HTML entity".to_owned())?;
            char::from_u32(value)
                .ok_or_else(|| "invalid HTML entity code point".to_owned())?
                .into()
        }
        _ => None,
    };
    Ok(character)
}

fn strip_html_markup(value: &str) -> Result<String, String> {
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value.as_bytes()[index] != b'<' {
            let character = value[index..]
                .chars()
                .next()
                .ok_or_else(|| "invalid UTF-8 boundary".to_owned())?;
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let Some(relative_end) = value[index + 1..].find('>') else {
            return Err("unterminated HTML markup".to_owned());
        };
        output.push(' ');
        index = index + 1 + relative_end + 1;
    }
    Ok(output)
}

fn collapse_whitespace(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() {
            pending_space = !output.is_empty();
            continue;
        }
        if pending_space {
            output.push(' ');
            pending_space = false;
        }
        output.push(character);
    }
    output.trim().to_owned()
}

struct RedactedParameters<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedParameters<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|(name, _value)| DebugParameter { name }))
            .finish()
    }
}

struct DebugParameter<'a> {
    name: &'a str,
}

impl fmt::Debug for DebugParameter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Parameter")
            .field("name", &self.name)
            .field("value", &"[redacted]")
            .finish()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn backend_repair_business_news_detail_percent_is_content_not_uri_encoding() {
        for content in [
            "<p>完成率 100%，政策 &amp; 通知。</p>",
            "<p>折扣50% off</p>",
            "<a href='/f/file?name=a%2Fb%25.pdf'>附件</a>",
        ] {
            let body=serde_json::json!({"result":"success","object":{"xxDto":{"xxid":"fixture","bt":"完成率100% &amp; 通知","nr":content}}}).to_string();
            let detail = parse_news_detail(&body).unwrap();
            assert_eq!(detail.title, "完成率100% & 通知");
            assert_eq!(detail.content_html, content.replace("&amp;", "&"));
        }
    }
    #[test]
    fn backend_repair_business_news_detail_preserves_literal_percent_sequences() {
        let body=serde_json::json!({"object":{"xxDto":{"bt":"配置%20说明","nr":"<p>参数%20不是空格，模板%AB是原文。</p>"}}}).to_string();
        let detail = parse_news_detail(&body).unwrap();
        assert_eq!(detail.title, "配置%20说明");
        assert!(detail.content_html.contains("%20"));
        assert!(detail.content_html.contains("%AB"));
    }
    use super::*;

    const LIST_FIXTURE: &str = r#"
        {
          "object": {
            "dataList": [
              {
                "bt": "%E6%95%99%E5%8A%A1%E9%80%9A%E7%9F%A5 &amp; <em>安排</em>",
                "url": "/b/info/xxfb_fg/xnzx/template/detail?xxid=news-1&amp;preview=",
                "xxid": " news-1 ",
                "time": " 2026-09-11T08:30:00 ",
                "dwmc_show": " 教务处 ",
                "yxzd": "1-0",
                "lmid": "LM_JWGG",
                "sfsc": false
              }
            ]
          }
        }
    "#;

    const SEARCH_FIXTURE: &str = r#"
        {
          "result": "success",
          "object": {
            "resultsList": [
              {
                "bt": "&lt;strong&gt;%E6%95%B0%E6%8D%AE&lt;/strong&gt;",
                "url": "https://news.example.invalid/article/news-2",
                "xxid": "news-2",
                "time": "2026/09/10 09:00",
                "dwmc_show": "信息化工作办公室",
                "yxzd": null,
                "lmid": "LM_BGTG",
                "sfsc": true
              }
            ]
          }
        }
    "#;

    const DETAIL_FIXTURE: &str = r#"
        {
          "result": "success",
          "object": {
            "xxDto": {
              "xxid": "news-1",
              "bt": "教务通知",
              "nr": "&lt;p&gt;请查看 &lt;strong&gt;通知&lt;/strong&gt;。&lt;/p&gt;",
              "fjs_template": [
                {"wjid":"file-1", "wjmc":"附件.pdf"}
              ]
            }
          }
        }
    "#;

    #[test]
    fn standard_profile_builds_relative_list_search_and_detail_plans() {
        let profile = NewsProfile::standard();
        let list = profile
            .list_request(1, 20, None, Some("LM_JWGG"))
            .expect("list plan");
        assert_eq!(list.operation(), NewsOperation::List);
        assert_eq!(list.method(), NewsHttpMethod::Get);
        assert_eq!(list.path(), DEFAULT_LIST_PATH);
        assert_eq!(
            list.query_parameters(),
            &[
                ("oType".to_owned(), "xs".to_owned()),
                ("lydw".to_owned(), String::new()),
                ("lmid".to_owned(), "LM_JWGG".to_owned()),
                ("currentPage".to_owned(), "1".to_owned()),
                ("length".to_owned(), "20".to_owned()),
            ]
        );
        assert!(
            list.query_parameters()
                .iter()
                .all(|(name, _)| name != "_csrf")
        );
        assert_eq!(list.csrf_requirement().field, "_csrf");
        let all = profile.list_request(1, 20, None, None).expect("all plan");
        assert_eq!(
            all.query_parameters()
                .iter()
                .find(|(name, _)| name == "lmid")
                .map(|(_, value)| value.as_str()),
            Some("all")
        );

        let search = profile
            .search_request(
                &NewsSearchInput::new("课程", 2)
                    .with_channel_filter("教务通知")
                    .with_exact_match(true),
            )
            .expect("search plan");
        assert_eq!(search.operation(), NewsOperation::Search);
        assert_eq!(search.method(), NewsHttpMethod::Post);
        assert_eq!(search.path(), DEFAULT_SEARCH_PATH);
        assert!(search.query_parameters().is_empty());
        assert_eq!(search.form_parameters().len(), 1);
        let search_payload: Value =
            serde_json::from_str(&search.form_parameters()[0].1).expect("search JSON");
        assert_eq!(search_payload["params"]["bt"], "课程");
        assert_eq!(search_payload["filterParams"]["lmmcgroup"], "教务通知");
        assert_eq!(search_payload["matchExact"], "是");
        assert_eq!(search_payload["currentPage"], 2);

        let detail = profile.detail_request("news-1").expect("detail plan");
        assert_eq!(detail.operation(), NewsOperation::Detail);
        assert_eq!(detail.path(), DEFAULT_DETAIL_PATH);
        assert_eq!(
            detail.query_parameters(),
            &[
                ("xxid".to_owned(), "news-1".to_owned()),
                ("preview".to_owned(), String::new()),
            ]
        );
        for invalid in ["javascript:alert(1)", "news/1", "news%ZZ", "news%2e%2e"] {
            assert!(matches!(
                profile.detail_request(invalid),
                Err(NewsProfileError::InvalidValue { field }) if field == "article_id"
            ));
        }
    }

    #[test]
    fn request_debug_does_not_expose_sensitive_like_values() {
        let profile = NewsProfile::standard();
        let plan = profile
            .detail_request("redacted-article-id")
            .expect("detail plan");
        let debug = format!("{plan:?}");
        assert!(!debug.contains("redacted-article-id"));
        assert!(!debug.contains("https://"));
        assert!(debug.contains("_csrf"));
    }

    #[test]
    fn parses_and_normalizes_list_fixture() {
        let outcome = parse_news_list(LIST_FIXTURE).expect("list fixture");
        let page = outcome.page();
        assert_eq!(outcome.classification(), NewsPageClassification::Data);
        assert_eq!(page.feed, NewsFeedKind::List);
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.id, "news-1");
        assert_eq!(item.title, "教务通知 & 安排");
        assert_eq!(
            item.link.as_str(),
            "/b/info/xxfb_fg/xnzx/template/detail?xxid=news-1&preview="
        );
        assert_eq!(item.published_at, "2026-09-11 08:30:00");
        assert_eq!(item.source, "教务处");
        assert!(item.topped);
        assert_eq!(item.channel.as_str(), "LM_JWGG");
        assert!(!item.favorited);
    }

    #[test]
    fn parses_search_fixture_and_allows_null_topped_marker() {
        let outcome = parse_news_search(SEARCH_FIXTURE).expect("search fixture");
        let item = &outcome.page().items[0];
        assert_eq!(item.title, "数据");
        assert_eq!(item.published_at, "2026/09/10 09:00");
        assert!(!item.topped);
        assert!(item.favorited);
    }

    #[test]
    fn parses_detail_fixture_and_decodes_body_and_attachments() {
        let detail = parse_news_detail(DETAIL_FIXTURE).expect("detail fixture");
        assert_eq!(detail.id, "news-1");
        assert_eq!(detail.title, "教务通知");
        assert_eq!(detail.content_html, "<p>请查看 <strong>通知</strong>。</p>");
        assert_eq!(detail.summary, "请查看 通知 。");
        assert_eq!(detail.attachments.len(), 1);
        assert_eq!(detail.attachments[0].id, "file-1");
        assert_eq!(detail.attachments[0].name, "附件.pdf");
    }

    #[test]
    fn does_not_promote_a_detail_with_empty_content() {
        let body = r#"{"result":"success","object":{"xxDto":{"bt":"标题","nr":""}}}"#;
        assert!(matches!(
            parse_news_detail(body),
            Err(NewsParseError::DetailMalformedPayload { field, .. })
                if field == "object.xxDto.nr"
        ));
    }

    #[test]
    fn allows_valid_detail_without_xxid_for_caller_id_fallback() {
        let body = r#"{"result":"success","object":{"xxDto":{"bt":"标题","nr":"<p>正文</p>"}}}"#;
        let detail = parse_news_detail(body).expect("detail without echoed xxid");
        assert!(detail.id.is_empty());
        assert_eq!(detail.title, "标题");
        assert_eq!(detail.content_html, "<p>正文</p>");
        assert_eq!(detail.summary, "正文");
    }

    #[test]
    fn still_rejects_missing_detail_envelope() {
        for body in [
            r#"{"result":"success"}"#,
            r#"{"result":"success","object":{}}"#,
        ] {
            assert!(matches!(
                parse_news_detail(body),
                Err(NewsParseError::DetailMalformedPayload { .. })
            ));
        }
    }

    #[test]
    fn rejects_http_200_login_documents_for_detail_and_plain_login_marker() {
        let login = "\u{feff}<!doctype html><html><form><input name=\"i_user\"><input name=\"i_pass\"></form></html>";
        assert!(matches!(
            parse_news_list(login),
            Err(NewsParseError::HtmlLoginPage)
        ));
        assert!(matches!(
            parse_news_detail("会话已过期，请先登录"),
            Err(NewsParseError::HtmlLoginPage)
        ));
        assert!(matches!(
            parse_news_detail(r#"{"result":"error","msg":"登录失效"}"#),
            Err(NewsParseError::LoginRequired)
        ));
    }

    #[test]
    fn distinguishes_empty_list_from_malformed_or_missing_data() {
        let empty = parse_news_list(r#"{"object":{"dataList":[]}}"#).expect("empty list");
        assert_eq!(empty.classification(), NewsPageClassification::EmptyList);
        assert!(empty.page().items.is_empty());

        assert!(matches!(
            parse_news_list(r#"{"object":{}}"#),
            Err(NewsParseError::MalformedPayload {
                source: NewsPayloadError::MissingCollection { field }
            }) if field == "dataList"
        ));
        assert!(matches!(
            parse_news_list(r#"{"object":{"dataList":{}}}"#),
            Err(NewsParseError::MalformedPayload {
                source: NewsPayloadError::CollectionNotArray { field }
            }) if field == "dataList"
        ));
    }

    #[test]
    fn classifies_login_business_failure_malformed_and_wrong_record() {
        assert!(matches!(
            parse_news_list("<!doctype html><html><form><input name=\"password\"></form></html>"),
            Err(NewsParseError::HtmlLoginPage)
        ));
        assert!(matches!(
            parse_news_list(r#"{"result":"error","msg":"permission denied"}"#),
            Err(NewsParseError::BusinessFailure { message: Some(message) })
                if message == "permission denied"
        ));
        assert!(matches!(
            parse_news_list(r#"{"result":"error","msg":"session expired"}"#),
            Err(NewsParseError::LoginRequired)
        ));
        assert!(matches!(
            parse_news_list(r#"{"object":{"dataList":["not-an-object"]}}"#),
            Err(NewsParseError::MalformedPayload {
                source: NewsPayloadError::RecordNotObject { index: 0 }
            })
        ));
        assert!(matches!(
            parse_news_list(r#"{"object":{"dataList":[{"bt":"title"}]}}"#),
            Err(NewsParseError::MalformedPayload {
                source: NewsPayloadError::MissingField { index: 0, field }
            }) if field == "url"
        ));
        assert!(matches!(
            parse_news_list("{not-json}"),
            Err(NewsParseError::MalformedJson { .. })
        ));
    }

    #[test]
    fn rejects_invalid_encoded_values_and_preserves_redaction() {
        let invalid_percent = r#"{
          "object":{"dataList":[{
            "bt":"title%ZZ",
            "url":"/news/1",
            "xxid":"news-1",
            "time":"2026-09-11",
            "dwmc_show":"source",
            "yxzd":"",
            "lmid":"LM_JWGG",
            "sfsc":false
          }]}
        }"#;
        assert!(matches!(
            parse_news_list(invalid_percent),
            Err(NewsParseError::MalformedPayload {
                source: NewsPayloadError::InvalidField { field, .. }
            }) if field == "bt"
        ));

        let outcome = parse_news_search(SEARCH_FIXTURE).expect("search fixture");
        let debug = format!("{:?}", outcome.page().items[0]);
        assert!(!debug.contains("https://news.example.invalid"));
    }

    #[test]
    fn enforces_a_safe_boundary_for_article_links() {
        for link in [
            "/b/info/article?xxid=fixture",
            "https://news.example.invalid/article/fixture?preview=",
        ] {
            assert!(
                normalize_link(link).is_ok(),
                "ordinary link should parse: {link}"
            );
        }

        for link in [
            "javascript:alert(1)",
            "data:text/html,login",
            "ftp://news.example.invalid/article",
            "https://user:password@news.example.invalid/article",
            "https://news.example.invalid/article#fragment",
            "/article#fragment",
            "/article%ZZ",
            "/article/../login",
            "/article/%2e%2e/login",
            "/article/%252e%252e/login",
            "//outside.example.invalid/article",
        ] {
            assert!(
                normalize_link(link).is_err(),
                "unsafe link was accepted: {link}"
            );
        }
    }

    #[test]
    fn profile_rejects_absolute_paths_and_unbounded_pages() {
        let mut profile = NewsProfile::standard();
        profile.list.path = "https://news.example.invalid/list".to_owned();
        assert!(matches!(
            profile.validate(),
            Err(NewsProfileError::InvalidPath { field, .. }) if field == "list.path"
        ));

        let profile = NewsProfile::standard();
        assert!(matches!(
            profile.list_request(0, 20, None, None),
            Err(NewsProfileError::InvalidPage)
        ));
        assert!(matches!(
            profile.list_request(1, DEFAULT_MAX_PAGE_SIZE + 1, None, None),
            Err(NewsProfileError::InvalidPageSize { .. })
        ));

        let mut profile = NewsProfile::standard();
        profile.list.path = "/b/../outside".to_owned();
        assert!(matches!(
            profile.validate(),
            Err(NewsProfileError::InvalidPath { field, .. }) if field == "list.path"
        ));
        profile.list.path = "/b/%2e%2e/outside".to_owned();
        assert!(matches!(
            profile.validate(),
            Err(NewsProfileError::InvalidPath { field, .. }) if field == "list.path"
        ));
    }
}

#[path = "info_news_legacy_policy.rs"]
mod legacy_policy;
pub(crate) use legacy_policy::parse_news_legacy_detail_for_url;

#[path = "info_news_catalog.rs"]
mod catalog;
pub(crate) use catalog::channels_observed_on_news_page;
pub use catalog::{NewsChannelOption, NewsSourceOption, parse_news_channels, parse_news_sources};

#[path = "info_news_personal.rs"]
mod personal;
pub use personal::{
    MAX_FAVORITE_PAGES, NewsFavoritePage, NewsSubscriptionRule, parse_news_favorite_page,
    parse_news_subscription_page, parse_news_subscriptions,
};
