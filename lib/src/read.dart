part of '../tsinghua_kit.dart';

/// Cache behavior for a cacheable service read.
enum ReadPolicy { cacheOnly, preferFreshCache, refresh, refreshOrCached }

/// The source from which a successful result was obtained.
enum ReadSource { live, memoryCache, clientCache, persistentCache, unknown }

/// Whether cached data met its service-specific freshness rule.
enum CacheFreshness { fresh, stale, notApplicable, unknown }

/// Whether the requested collection was fully verified.
enum ReadCoverage {
  complete,
  partialReadLimitReached,
  partialSourceChanged,
  partialCompletionUnconfirmed,
  unknown,
}

/// Source, freshness, observation time, and completeness for one read.
class ReadMetadata {
  const ReadMetadata({
    required this.source,
    required this.freshness,
    required this.observedAt,
    required this.coverage,
    this.refreshFailureCode,
  });

  final ReadSource source;
  final CacheFreshness freshness;

  /// The original observation time, normalized to UTC.
  final DateTime observedAt;

  final ReadCoverage coverage;

  /// Stable SDK error code that caused an eligible stale fallback, if any.
  final String? refreshFailureCode;
}

/// A successful SDK value and the evidence describing how it was read.
class ReadResult<T> {
  const ReadResult({required this.data, required this.metadata});

  final T data;
  final ReadMetadata metadata;
}
