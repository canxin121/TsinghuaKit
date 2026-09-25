part of '../tsinghua_kit.dart';

/// Completeness of the channel choices supplied by INFO.
enum NewsCatalogCoverage { complete, partial, unknown }

/// Opaque source selection returned by this Client's latest news catalog.
class NewsSourceReference {
  const NewsSourceReference._(this._id);

  final String _id;
}

/// One source option from the current INFO catalog.
class NewsSource {
  const NewsSource({required this.reference, required this.name});

  final NewsSourceReference reference;
  final String name;
}

/// Opaque channel selection returned by this Client's latest news catalog.
class NewsChannelReference {
  const NewsChannelReference._(this._id);

  final String _id;
}

/// One channel option from the current INFO catalog.
class NewsChannel {
  const NewsChannel({required this.reference, required this.title});

  final NewsChannelReference reference;
  final String title;
}

/// The source and channel choices confirmed by INFO.
class NewsCatalog {
  NewsCatalog({
    required List<NewsSource> sources,
    required List<NewsChannel> channels,
    required this.channelCoverage,
  })  : sources = List.unmodifiable(sources),
        channels = List.unmodifiable(channels);

  final List<NewsSource> sources;
  final List<NewsChannel> channels;
  final NewsCatalogCoverage channelCoverage;
}

/// Opaque article selector returned by a page from this Client.
class NewsArticleReference {
  const NewsArticleReference._(this._id);

  final String _id;
}

/// One cleaned article returned by INFO.
class NewsArticle {
  const NewsArticle({
    required this.reference,
    required this.title,
    required this.publishedAt,
    required this.source,
    required this.topped,
    required this.channel,
    required this.favorited,
  });

  final NewsArticleReference reference;
  final String title;
  final String publishedAt;
  final String source;
  final bool topped;
  final String channel;
  final bool favorited;
}

/// One validated INFO page. Later pages have not necessarily been loaded.
class NewsPage {
  NewsPage({required this.page, required List<NewsArticle> articles})
      : articles = List.unmodifiable(articles);

  final int page;
  final List<NewsArticle> articles;
}

/// Opaque selector for a rule from this Client's latest subscription read.
class NewsSubscriptionReference {
  const NewsSubscriptionReference._(this._id);

  final String _id;
}

/// One read-only INFO subscription rule.
class NewsSubscription {
  NewsSubscription({
    required this.reference,
    required this.title,
    required this.order,
    required List<String> sources,
    required List<String> channels,
    required this.keyword,
  })  : sources = List.unmodifiable(sources),
        channels = List.unmodifiable(channels);

  final NewsSubscriptionReference reference;
  final String title;
  final BigInt order;
  final List<String> sources;
  final List<String> channels;
  final String keyword;
}

/// The current account's read-only INFO subscription rules.
class NewsSubscriptions {
  NewsSubscriptions({required List<NewsSubscription> rules})
      : rules = List.unmodifiable(rules);

  final List<NewsSubscription> rules;
}

/// The complete bounded set of current-account favorite articles.
class NewsFavorites {
  NewsFavorites({required List<NewsArticle> articles})
      : articles = List.unmodifiable(articles);

  final List<NewsArticle> articles;
}

/// One attachment name without an upstream URL.
class NewsAttachment {
  const NewsAttachment({required this.name});

  final String name;
}

/// Read-only INFO article detail. Content remains HTML supplied by the source.
class NewsArticleDetail {
  NewsArticleDetail({
    required this.title,
    required this.contentHtml,
    required this.summary,
    required List<NewsAttachment> attachments,
  }) : attachments = List.unmodifiable(attachments);

  final String title;
  final String contentHtml;
  final String summary;
  final List<NewsAttachment> attachments;
}

/// Read-only INFO operations using this Client's shared Runtime and transport.
class NewsClient {
  NewsClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads current source and channel choices. A successful refresh invalidates
  /// references returned by the previous catalog.
  Future<ReadResult<NewsCatalog>> catalog() => _sdkCall(
        () async => _newsCatalogResult(await _handle.newsCatalog()),
      );

  /// Reads one validated list page with optional selections from [catalog].
  Future<ReadResult<NewsPage>> articles({
    required int page,
    int pageSize = 20,
    NewsSourceReference? source,
    NewsChannelReference? channel,
    required ReadPolicy policy,
  }) =>
      _sdkCall(() async => _newsPageResult(
            await _handle.newsArticles(
              page: page,
              pageSize: pageSize,
              sourceReferenceId: source?._id,
              channelReferenceId: channel?._id,
              policy: _readPolicyDto(policy),
            ),
          ));

  /// Searches INFO with one validated keyword and an optional catalog channel.
  Future<ReadResult<NewsPage>> search({
    required int page,
    required String keyword,
    NewsChannelReference? channel,
    bool exactMatch = false,
    required ReadPolicy policy,
  }) =>
      _sdkCall(() async => _newsPageResult(
            await _handle.newsSearch(
              page: page,
              keyword: keyword,
              channelReferenceId: channel?._id,
              exactMatch: exactMatch,
              policy: _readPolicyDto(policy),
            ),
          ));

  /// Reads detail for an article returned by this Client's current page.
  Future<ReadResult<NewsArticleDetail>> article({
    required NewsArticleReference reference,
    required ReadPolicy policy,
  }) =>
      _sdkCall(() async => _newsArticleDetailResult(
            await _handle.newsArticle(
              referenceId: reference._id,
              policy: _readPolicyDto(policy),
            ),
          ));

  /// Reads the complete bounded current-account favorites collection.
  Future<ReadResult<NewsFavorites>> favorites() => _sdkCall(
        () async => _newsFavoritesResult(await _handle.newsFavorites()),
      );

  /// Reads subscription rules for the current Identity account.
  Future<ReadResult<NewsSubscriptions>> subscriptions() => _sdkCall(
        () async => _newsSubscriptionsResult(await _handle.newsSubscriptions()),
      );

  /// Reads one page for a rule returned by this Client's latest subscription
  /// read. The returned page's article references can be used for details.
  Future<ReadResult<NewsPage>> subscriptionArticles({
    required NewsSubscriptionReference reference,
    required int page,
  }) =>
      _sdkCall(() async => _newsPageResult(
            await _handle.newsSubscriptionArticles(
              referenceId: reference._id,
              page: page,
            ),
          ));
}

native.ReadPolicyDto _readPolicyDto(ReadPolicy value) => switch (value) {
      ReadPolicy.cacheOnly => native.ReadPolicyDto.cacheOnly,
      ReadPolicy.preferFreshCache => native.ReadPolicyDto.preferFreshCache,
      ReadPolicy.refresh => native.ReadPolicyDto.refresh,
      ReadPolicy.refreshOrCached => native.ReadPolicyDto.refreshOrCached,
    };

ReadResult<NewsCatalog> _newsCatalogResult(native.NewsCatalogResultDto value) =>
    ReadResult(
      data: NewsCatalog(
        sources: value.data.sources
            .map(
              (source) => NewsSource(
                reference: NewsSourceReference._(source.referenceId),
                name: source.name,
              ),
            )
            .toList(growable: false),
        channels: value.data.channels
            .map(
              (channel) => NewsChannel(
                reference: NewsChannelReference._(channel.referenceId),
                title: channel.title,
              ),
            )
            .toList(growable: false),
        channelCoverage: _newsCatalogCoverage(value.data.channelCoverage),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<NewsPage> _newsPageResult(native.NewsPageResultDto value) =>
    ReadResult(
      data: NewsPage(
        page: value.data.page,
        articles: value.data.articles.map(_newsArticle).toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<NewsFavorites> _newsFavoritesResult(
  native.NewsFavoritesResultDto value,
) =>
    ReadResult(
      data: NewsFavorites(
        articles: value.data.articles.map(_newsArticle).toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<NewsSubscriptions> _newsSubscriptionsResult(
  native.NewsSubscriptionsResultDto value,
) =>
    ReadResult(
      data: NewsSubscriptions(
        rules: value.data.rules
            .map(
              (rule) => NewsSubscription(
                reference: NewsSubscriptionReference._(rule.referenceId),
                title: rule.title,
                order: _newsOrder(rule.order),
                sources: rule.sources,
                channels: rule.channels,
                keyword: rule.keyword,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<NewsArticleDetail> _newsArticleDetailResult(
  native.ArticleDetailResultDto value,
) =>
    ReadResult(
      data: NewsArticleDetail(
        title: value.data.title,
        contentHtml: value.data.contentHtml,
        summary: value.data.summary,
        attachments: value.data.attachments
            .map((attachment) => NewsAttachment(name: attachment.name))
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

NewsArticle _newsArticle(native.NewsArticleDto value) => NewsArticle(
      reference: NewsArticleReference._(value.referenceId),
      title: value.title,
      publishedAt: value.publishedAt,
      source: value.source,
      topped: value.topped,
      channel: value.channel,
      favorited: value.favorited,
    );

NewsCatalogCoverage _newsCatalogCoverage(
  native.NewsCatalogCoverageDto value,
) =>
    switch (value) {
      native.NewsCatalogCoverageDto.complete => NewsCatalogCoverage.complete,
      native.NewsCatalogCoverageDto.partial => NewsCatalogCoverage.partial,
      native.NewsCatalogCoverageDto.unknown => NewsCatalogCoverage.unknown,
    };

BigInt _newsOrder(Object value) =>
    value is BigInt ? value : BigInt.from(value as int);
