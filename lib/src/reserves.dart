part of '../tsinghua_kit.dart';

/// One catalogue record in the course-reserve textbook collection
/// (`馆藏教参`).
class ReservesBook {
  const ReservesBook({
    required this.title,
    required this.imageUrl,
    required this.isbn,
    required this.author,
    required this.publisher,
    required this.referenceId,
  });

  /// The title the catalogue advertises.
  final String title;

  /// The cover image address, already rewritten onto this API's own mapping
  /// origin in Rust.
  ///
  /// It is a link, not a payload: this client never fetches it. A row the
  /// service did not give an image for carries an empty string.
  final String imageUrl;

  /// The printed ISBN.
  final String isbn;

  /// The printed author line.
  final String author;

  /// The printed publication line.
  final String publisher;

  /// An opaque handle for this record's detail page, or null when the caller
  /// must not read one.
  ///
  /// Pass it back to [ReservesClient.detail] unchanged. It is only valid for
  /// the search that produced it, and the service's own book identifier is
  /// never exposed in its place.
  final String? referenceId;
}

/// One validated page of course-reserve search results.
class ReservesSearch {
  ReservesSearch({
    required List<ReservesBook> books,
    required this.total,
    required this.pageCount,
    required this.page,
  }) : books = List.unmodifiable(books);

  final List<ReservesBook> books;

  /// The service's own match count for this query.
  final BigInt total;

  /// The service's own page count for this query.
  final BigInt pageCount;

  /// The page this result came from.
  final int page;
}

/// One chapter link on a book's detail page.
class ReservesChapter {
  const ReservesChapter({required this.title, required this.url});

  final String title;

  /// The chapter address, already rewritten onto this API's own mapping origin
  /// in Rust. Like [ReservesBook.imageUrl], it is a link this client never
  /// fetches.
  final String url;
}

/// One book's bibliographic detail and chapter list.
class ReservesBookDetail {
  ReservesBookDetail({
    required this.title,
    required this.imageUrl,
    required this.author,
    required this.publisher,
    required this.isbn,
    required this.version,
    required this.volume,
    required List<ReservesChapter> chapters,
  }) : chapters = List.unmodifiable(chapters);

  final String title;
  final String imageUrl;
  final String author;
  final String publisher;
  final String isbn;

  /// The printed edition line.
  final String version;

  /// The printed volume line.
  final String volume;

  final List<ReservesChapter> chapters;
}

/// The course-reserve textbook collection (`馆藏教参`).
///
/// Both reads are live and account-bound: they ride the INFO/WebVPN session the
/// client already holds, and nothing here is served from cache, because a
/// holding can change between requests.
///
/// This API is read-only by construction. The service's own full-text reader
/// needs a campus identity login this client deliberately does not perform, so
/// an expired session is reported as an authentication failure rather than
/// silently answered with an empty catalogue.
class ReservesClient {
  ReservesClient._(this._handle);

  final native.ClientHandle _handle;

  /// Searches the catalogue by book name.
  ///
  /// [bookName] is the caller's own text. It is escaped with the service's own
  /// private convention inside Rust, so a name that cannot be represented is
  /// refused before any request; [page] is one-based and bounded by
  /// [maxPage].
  ///
  /// A search the service itself says matched nothing is an empty
  /// [ReservesSearch] with a zero `total`. A page this client could not read is
  /// an error, never an empty catalogue.
  Future<ReadResult<ReservesSearch>> search({
    required String bookName,
    required int page,
  }) =>
      _sdkCall(
        () async => _reservesSearch(
          await _handle.reservesSearchResult(bookName: bookName, page: page),
        ),
      );

  /// Reads one record's bibliographic detail and chapter list.
  ///
  /// [referenceId] must come from the most recent successful [search]; one
  /// from a superseded search or from another client does not resolve, and the
  /// call fails rather than reading a different book.
  Future<ReadResult<ReservesBookDetail>> detail({
    required String referenceId,
  }) =>
      _sdkCall(
        () async => _reservesDetail(
          await _handle.reservesDetailResult(referenceId: referenceId),
        ),
      );

  /// The highest one-based page the service accepts through this client.
  static const int maxPage = 1000;
}

ReadResult<ReservesSearch> _reservesSearch(
  native.ReservesSearchResultDto value,
) =>
    ReadResult(
      data: ReservesSearch(
        books: value.data.books
            .map(
              (book) => ReservesBook(
                title: book.title,
                imageUrl: book.imageUrl,
                isbn: book.isbn,
                author: book.author,
                publisher: book.publisher,
                referenceId: book.referenceId,
              ),
            )
            .toList(growable: false),
        total: value.data.total,
        pageCount: value.data.pageCount,
        page: value.data.page,
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<ReservesBookDetail> _reservesDetail(
  native.ReservesDetailResultDto value,
) =>
    ReadResult(
      data: ReservesBookDetail(
        title: value.data.title,
        imageUrl: value.data.imageUrl,
        author: value.data.author,
        publisher: value.data.publisher,
        isbn: value.data.isbn,
        version: value.data.version,
        volume: value.data.volume,
        chapters: value.data.chapters
            .map(
              (chapter) => ReservesChapter(
                title: chapter.title,
                url: chapter.url,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
