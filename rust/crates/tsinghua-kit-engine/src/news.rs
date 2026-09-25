//! Curated, context-bound INFO news types for the public SDK.

use std::{fmt, num::NonZeroU32};

use uuid::Uuid;

use crate::error::{Error, ErrorCode, Service};

const MAX_PAGE: u32 = 100_000;
const MAX_PAGE_SIZE: u32 = 100;
const MAX_FILTER_LENGTH: usize = 256;

/// A validated list or search request for the INFO news service.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsQuery {
    page: NonZeroU32,
    kind: NewsQueryKind,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum NewsQueryKind {
    List {
        page_size: NonZeroU32,
        source: Option<NewsSourceRef>,
        channel: Option<NewsChannelRef>,
    },
    Search {
        keyword: String,
        channel: Option<NewsChannelRef>,
        exact_match: bool,
    },
}

impl fmt::Debug for NewsQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.kind {
            NewsQueryKind::List { .. } => "list",
            NewsQueryKind::Search { .. } => "search",
        };
        formatter
            .debug_struct("NewsQuery")
            .field("kind", &kind)
            .field("page", &self.page)
            .finish_non_exhaustive()
    }
}

impl NewsQuery {
    /// Creates a validated list request. Page numbers start at one and the
    /// service page size is bounded to prevent accidental unbounded reads.
    pub fn list(page: u32, page_size: u32) -> Result<Self, Error> {
        let page = validate_page(page)?;
        let page_size = NonZeroU32::new(page_size)
            .filter(|value| value.get() <= MAX_PAGE_SIZE)
            .ok_or_else(invalid_news_input)?;
        Ok(Self {
            page,
            kind: NewsQueryKind::List {
                page_size,
                source: None,
                channel: None,
            },
        })
    }

    /// Creates a validated search request. Search terms are not printed by
    /// this type's `Debug` implementation.
    pub fn search(page: u32, keyword: impl Into<String>) -> Result<Self, Error> {
        let page = validate_page(page)?;
        let keyword = validate_filter(keyword.into())?;
        Ok(Self {
            page,
            kind: NewsQueryKind::Search {
                keyword,
                channel: None,
                exact_match: false,
            },
        })
    }

    /// Restricts a list query to a source selected from this client's latest
    /// INFO news catalog. The client checks the reference before making a
    /// request.
    pub fn with_source(mut self, source: &NewsSourceRef) -> Result<Self, Error> {
        match &mut self.kind {
            NewsQueryKind::List { source: slot, .. } => *slot = Some(source.clone()),
            NewsQueryKind::Search { .. } => return Err(invalid_news_input()),
        }
        Ok(self)
    }

    /// Restricts a list query to a channel selected from this client's latest
    /// INFO news catalog. The client checks the reference before making a
    /// request.
    pub fn with_channel(mut self, channel: &NewsChannelRef) -> Result<Self, Error> {
        match &mut self.kind {
            NewsQueryKind::List { channel: slot, .. } => *slot = Some(channel.clone()),
            NewsQueryKind::Search { .. } => return Err(invalid_news_input()),
        }
        Ok(self)
    }

    /// Restricts a search to the label of a channel returned by this client's
    /// latest INFO catalog read.
    pub fn with_search_channel(mut self, channel: &NewsChannelRef) -> Result<Self, Error> {
        match &mut self.kind {
            NewsQueryKind::Search { channel: slot, .. } => *slot = Some(channel.clone()),
            NewsQueryKind::List { .. } => return Err(invalid_news_input()),
        }
        Ok(self)
    }

    /// Enables the INFO search endpoint's exact-match option.
    pub fn exact_match(mut self, exact_match: bool) -> Result<Self, Error> {
        match &mut self.kind {
            NewsQueryKind::Search {
                exact_match: value, ..
            } => *value = exact_match,
            NewsQueryKind::List { .. } => return Err(invalid_news_input()),
        }
        Ok(self)
    }

    pub(crate) fn page(&self) -> u32 {
        self.page.get()
    }

    pub(crate) fn kind(&self) -> &NewsQueryKind {
        &self.kind
    }
}

/// An opaque selector for one source returned by an INFO catalog read. Its
/// wire identifier, client identity, and catalog generation stay private.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsSourceRef {
    client_id: Uuid,
    catalog_generation: u64,
    id: String,
}

impl NewsSourceRef {
    pub(crate) fn new(client_id: Uuid, catalog_generation: u64, id: String) -> Self {
        Self {
            client_id,
            catalog_generation,
            id,
        }
    }

    pub(crate) fn belongs_to(&self, client_id: Uuid, generation: u64) -> bool {
        self.client_id == client_id && self.catalog_generation == generation
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Debug for NewsSourceRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsSourceRef")
            .field("selector", &"[opaque]")
            .finish()
    }
}

/// An opaque selector for one channel returned by an INFO catalog read.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsChannelRef {
    client_id: Uuid,
    catalog_generation: u64,
    id: String,
    label: String,
}

impl NewsChannelRef {
    pub(crate) fn new(client_id: Uuid, catalog_generation: u64, id: String, label: String) -> Self {
        Self {
            client_id,
            catalog_generation,
            id,
            label,
        }
    }

    pub(crate) fn belongs_to(&self, client_id: Uuid, generation: u64) -> bool {
        self.client_id == client_id && self.catalog_generation == generation
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

impl fmt::Debug for NewsChannelRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsChannelRef")
            .field("selector", &"[opaque]")
            .finish()
    }
}

/// Whether the INFO service proved a full channel directory or only supplied
/// candidates observed on the latest news page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NewsCatalogCoverage {
    /// Both source and channel directories were read successfully.
    Complete,
    /// The source options are complete, but the channel directory could not
    /// be confirmed as complete. It may contain recent-page candidates or no
    /// candidates when that fallback read was unavailable.
    Partial,
}

/// One source option from the INFO news directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSource {
    reference: NewsSourceRef,
    name: String,
}

impl NewsSource {
    pub(crate) fn new(reference: NewsSourceRef, name: String) -> Self {
        Self { reference, name }
    }

    pub fn reference(&self) -> &NewsSourceRef {
        &self.reference
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// One channel option from the INFO news directory or a recent news page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsChannel {
    reference: NewsChannelRef,
    title: String,
}

impl NewsChannel {
    pub(crate) fn new(reference: NewsChannelRef, title: String) -> Self {
        Self { reference, title }
    }

    pub fn reference(&self) -> &NewsChannelRef {
        &self.reference
    }

    pub fn title(&self) -> &str {
        &self.title
    }
}

/// Validated source and channel options from one INFO catalog observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsCatalog {
    sources: Vec<NewsSource>,
    channels: Vec<NewsChannel>,
    channel_coverage: NewsCatalogCoverage,
}

impl NewsCatalog {
    pub(crate) fn new(
        sources: Vec<NewsSource>,
        channels: Vec<NewsChannel>,
        channel_coverage: NewsCatalogCoverage,
    ) -> Self {
        Self {
            sources,
            channels,
            channel_coverage,
        }
    }

    pub fn sources(&self) -> &[NewsSource] {
        &self.sources
    }

    pub fn channels(&self) -> &[NewsChannel] {
        &self.channels
    }

    pub fn channel_coverage(&self) -> NewsCatalogCoverage {
        self.channel_coverage
    }
}

/// An opaque selector for one subscription rule from this client's latest
/// subscription read. It carries only the runtime-owned selector, never the
/// upstream subscription ID.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsSubscriptionRef {
    client_id: Uuid,
    generation: u64,
    selector: String,
}

impl NewsSubscriptionRef {
    pub(crate) fn new(client_id: Uuid, generation: u64, selector: String) -> Self {
        Self {
            client_id,
            generation,
            selector,
        }
    }

    pub(crate) fn belongs_to(&self, client_id: Uuid, generation: u64) -> bool {
        self.client_id == client_id && self.generation == generation
    }

    pub(crate) fn selector(&self) -> &str {
        &self.selector
    }
}

impl fmt::Debug for NewsSubscriptionRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsSubscriptionRef")
            .field("selector", &"[opaque]")
            .finish()
    }
}

/// One read-only news subscription rule owned by the current Identity account.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsSubscription {
    reference: NewsSubscriptionRef,
    title: String,
    order: i64,
    sources: Vec<String>,
    channels: Vec<String>,
    keyword: String,
}

impl NewsSubscription {
    pub(crate) fn new(
        reference: NewsSubscriptionRef,
        title: String,
        order: i64,
        sources: Vec<String>,
        channels: Vec<String>,
        keyword: String,
    ) -> Self {
        Self {
            reference,
            title,
            order,
            sources,
            channels,
            keyword,
        }
    }

    pub fn reference(&self) -> &NewsSubscriptionRef {
        &self.reference
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn order(&self) -> i64 {
        self.order
    }

    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    pub fn keyword(&self) -> &str {
        &self.keyword
    }
}

impl fmt::Debug for NewsSubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsSubscription")
            .field("reference", &self.reference)
            .field("title", &self.title)
            .field("order", &self.order)
            .field("source_count", &self.sources.len())
            .field("channel_count", &self.channels.len())
            .field("keyword", &"[redacted]")
            .finish()
    }
}

/// The verified subscription-rule list returned by one live INFO read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsSubscriptions {
    rules: Vec<NewsSubscription>,
}

impl NewsSubscriptions {
    pub(crate) fn new(rules: Vec<NewsSubscription>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[NewsSubscription] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// All favorite news items from the complete, bounded pagination read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsFavorites {
    items: Vec<NewsArticle>,
}

impl NewsFavorites {
    pub(crate) fn new(items: Vec<NewsArticle>) -> Self {
        Self { items }
    }

    pub fn items(&self) -> &[NewsArticle] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

fn validate_page(page: u32) -> Result<NonZeroU32, Error> {
    NonZeroU32::new(page)
        .filter(|value| value.get() <= MAX_PAGE)
        .ok_or_else(invalid_news_input)
}

fn validate_filter(value: String) -> Result<String, Error> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_FILTER_LENGTH
        || trimmed.chars().any(char::is_control)
    {
        return Err(invalid_news_input());
    }
    Ok(trimmed.to_owned())
}

fn invalid_news_input() -> Error {
    Error::new(Service::News, ErrorCode::InvalidInput)
}

/// An opaque selector for one article from a specific Client and news-link
/// snapshot. Its identifier and provenance are never printed or mutable.
#[derive(Clone, PartialEq, Eq)]
pub struct ArticleRef {
    client_id: Uuid,
    generation: u64,
    article_id: String,
}

impl ArticleRef {
    pub(crate) fn new(client_id: Uuid, generation: u64, article_id: String) -> Self {
        Self {
            client_id,
            generation,
            article_id,
        }
    }

    pub(crate) fn belongs_to(&self, client_id: Uuid) -> bool {
        self.client_id == client_id
    }

    pub(crate) fn selector(&self) -> (u64, &str) {
        (self.generation, &self.article_id)
    }
}

impl fmt::Debug for ArticleRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArticleRef")
            .field("selector", &"[opaque]")
            .finish()
    }
}

/// One cleaned news record. Detail access requires its opaque `ArticleRef`.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsArticle {
    reference: ArticleRef,
    title: String,
    published_at: String,
    source: String,
    topped: bool,
    channel: String,
    favorited: bool,
}

impl NewsArticle {
    pub(crate) fn new(
        reference: ArticleRef,
        title: String,
        published_at: String,
        source: String,
        topped: bool,
        channel: String,
        favorited: bool,
    ) -> Self {
        Self {
            reference,
            title,
            published_at,
            source,
            topped,
            channel,
            favorited,
        }
    }

    /// Returns the opaque selector required to load this article's detail.
    pub fn reference(&self) -> &ArticleRef {
        &self.reference
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn published_at(&self) -> &str {
        &self.published_at
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn is_topped(&self) -> bool {
        self.topped
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }

    pub fn is_favorited(&self) -> bool {
        self.favorited
    }
}

impl fmt::Debug for NewsArticle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsArticle")
            .field("reference", &self.reference)
            .field("title", &self.title)
            .field("published_at", &self.published_at)
            .field("source", &self.source)
            .field("topped", &self.topped)
            .field("channel", &self.channel)
            .field("favorited", &self.favorited)
            .finish()
    }
}

/// One requested INFO news page. `ReadResult` metadata describes this page;
/// it does not claim that every page matching the query has been fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsPage {
    page: u32,
    items: Vec<NewsArticle>,
}

impl NewsPage {
    pub(crate) fn new(page: u32, items: Vec<NewsArticle>) -> Self {
        Self { page, items }
    }

    pub fn page(&self) -> u32 {
        self.page
    }

    pub fn items(&self) -> &[NewsArticle] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// A cleaned attachment without a server URL or session token.
#[derive(Clone, PartialEq, Eq)]
pub struct NewsAttachment {
    name: String,
}

impl NewsAttachment {
    pub(crate) fn new(name: String) -> Self {
        Self { name }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for NewsAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewsAttachment")
            .field("name", &self.name)
            .finish()
    }
}

/// Read-only article detail obtained from a current `ArticleRef`.
#[derive(Clone, PartialEq, Eq)]
pub struct ArticleDetail {
    title: String,
    content_html: String,
    summary: String,
    attachments: Vec<NewsAttachment>,
}

impl ArticleDetail {
    pub(crate) fn new(
        title: String,
        content_html: String,
        summary: String,
        attachments: Vec<NewsAttachment>,
    ) -> Self {
        Self {
            title,
            content_html,
            summary,
            attachments,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn content_html(&self) -> &str {
        &self.content_html
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn attachments(&self) -> &[NewsAttachment] {
        &self.attachments
    }
}

impl fmt::Debug for ArticleDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArticleDetail")
            .field("title", &self.title)
            .field("content_html", &"[redacted]")
            .field("summary", &"[redacted]")
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_refactor_news_query_validates_scope_and_bounds() {
        assert!(NewsQuery::list(0, 20).is_err());
        assert!(NewsQuery::list(1, MAX_PAGE_SIZE + 1).is_err());
        assert!(NewsQuery::search(1, " \n ").is_err());
        assert!(NewsQuery::search(1, "hello\nworld").is_err());
        assert!(
            NewsQuery::list(1, 20)
                .unwrap()
                .with_search_channel(&NewsChannelRef::new(
                    Uuid::new_v4(),
                    1,
                    "LM_NEWS".to_owned(),
                    "News".to_owned(),
                ))
                .is_err()
        );
        assert!(
            NewsQuery::search(1, "hello")
                .unwrap()
                .with_source(&NewsSourceRef::new(Uuid::new_v4(), 1, "1".to_owned()))
                .is_err()
        );
    }

    #[test]
    fn backend_refactor_news_filter_references_hide_server_ids() {
        let source = NewsSourceRef::new(Uuid::new_v4(), 3, "private-source-id".to_owned());
        let channel = NewsChannelRef::new(
            Uuid::new_v4(),
            4,
            "private-channel-id".to_owned(),
            "private-channel-label".to_owned(),
        );
        assert!(!format!("{source:?}").contains("private-source-id"));
        assert!(!format!("{channel:?}").contains("private-channel-id"));
        let query = NewsQuery::list(1, 20)
            .unwrap()
            .with_source(&source)
            .unwrap();
        assert!(!format!("{query:?}").contains("private-source-id"));
    }

    #[test]
    fn backend_refactor_news_subscription_debug_hides_selector_and_keyword() {
        let rule = NewsSubscription::new(
            NewsSubscriptionRef::new(Uuid::new_v4(), 8, "private-runtime-selector".to_owned()),
            "My updates".to_owned(),
            1,
            vec!["Registrar".to_owned()],
            vec!["Announcements".to_owned()],
            "private-search-term".to_owned(),
        );
        let debug = format!("{rule:?}");
        assert!(!debug.contains("private-runtime-selector"));
        assert!(!debug.contains("private-search-term"));
        assert!(debug.contains("My updates"));
    }

    #[test]
    fn backend_refactor_article_reference_debug_is_opaque() {
        let reference = ArticleRef::new(Uuid::new_v4(), 7, "private-article-id".to_owned());
        assert!(!format!("{reference:?}").contains("private-article-id"));
    }
}
