//! Shared provenance and completeness types for SDK reads.
//!
//! A successful empty collection means the source was read completely and
//! contained no items. A failed read has no `ReadResult`; stale fallback data
//! must carry both its cache provenance and the refresh failure that caused
//! the fallback.

use std::fmt;

use chrono::{DateTime, Utc};

use crate::error::ErrorCode;

/// Policy requested by the caller for a cacheable read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReadPolicy {
    /// Read only a valid cache entry; never contact the service.
    CacheOnly,
    /// Use a valid fresh cache entry, otherwise perform a live read.
    PreferFreshCache,
    /// Require a live read and return its failure directly.
    Refresh,
    /// Try a live read and permit an eligible stale cache fallback.
    RefreshOrCached,
}

/// Where the returned data came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReadSource {
    /// Data was obtained from a successful current service read.
    Live,
    /// Data came from this client's short-lived memory cache.
    MemoryCache,
    /// Data came from a cache owned by this Client. It may use temporary
    /// files and is not promised to survive Client release.
    ClientCache,
    /// Data came from a host-configured persistent cache.
    PersistentCache,
}

/// Whether a cached value met the owning service's freshness rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheFreshness {
    /// The value met the service-specific freshness rule when returned.
    Fresh,
    /// The value was allowed only as an explicit stale fallback.
    Stale,
    /// The service does not define cache freshness for this operation.
    NotApplicable,
}

/// Why the returned collection is not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IncompleteReason {
    /// A safe request/page limit was reached before completion was proven.
    ReadLimitReached,
    /// The source changed during the read or returned inconsistent totals.
    SourceChanged,
    /// The source did not provide enough evidence to establish completion.
    CompletionUnconfirmed,
}

/// The completeness proof for a collection read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReadCoverage {
    /// The query's documented range has been read completely.
    Complete,
    /// Some values were read, but the documented query is not fully covered.
    Partial(IncompleteReason),
}

/// Metadata attached to one successful business result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMetadata {
    source: ReadSource,
    freshness: CacheFreshness,
    observed_at: DateTime<Utc>,
    coverage: ReadCoverage,
    refresh_failure: Option<ErrorCode>,
}

impl ReadMetadata {
    fn construct(
        source: ReadSource,
        freshness: CacheFreshness,
        observed_at: DateTime<Utc>,
        coverage: ReadCoverage,
        refresh_failure: Option<ErrorCode>,
    ) -> Self {
        Self {
            source,
            freshness,
            observed_at,
            coverage,
            refresh_failure,
        }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn new(
        source: ReadSource,
        freshness: CacheFreshness,
        observed_at: DateTime<Utc>,
        coverage: ReadCoverage,
        refresh_failure: Option<ErrorCode>,
    ) -> Self {
        Self::construct(source, freshness, observed_at, coverage, refresh_failure)
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn new(
        source: ReadSource,
        freshness: CacheFreshness,
        observed_at: DateTime<Utc>,
        coverage: ReadCoverage,
        refresh_failure: Option<ErrorCode>,
    ) -> Self {
        Self::construct(source, freshness, observed_at, coverage, refresh_failure)
    }

    /// Returns the actual source used for the data.
    pub fn source(&self) -> ReadSource {
        self.source
    }

    /// Returns the cache freshness classification.
    pub fn freshness(&self) -> CacheFreshness {
        self.freshness
    }

    /// Returns the time of the original live observation in UTC.
    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }

    /// Returns whether the service confirmed the requested range completely.
    pub fn coverage(&self) -> ReadCoverage {
        self.coverage
    }

    /// Returns the stable refresh failure when stale data was used as a
    /// permitted fallback.
    pub fn refresh_failure(&self) -> Option<ErrorCode> {
        self.refresh_failure
    }
}

/// A domain value together with source, freshness, and completeness evidence.
#[derive(Clone, PartialEq, Eq)]
pub struct ReadResult<T> {
    data: T,
    metadata: ReadMetadata,
}

impl<T> fmt::Debug for ReadResult<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadResult")
            .field("data_type", &std::any::type_name::<T>())
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl<T> ReadResult<T> {
    fn construct(data: T, metadata: ReadMetadata) -> Self {
        Self { data, metadata }
    }

    #[cfg(feature = "ffi-compat")]
    #[doc(hidden)]
    pub fn new(data: T, metadata: ReadMetadata) -> Self {
        Self::construct(data, metadata)
    }

    #[cfg(not(feature = "ffi-compat"))]
    pub(crate) fn new(data: T, metadata: ReadMetadata) -> Self {
        Self::construct(data, metadata)
    }

    /// Borrows the returned business data.
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Consumes the result and returns its business data.
    pub fn into_data(self) -> T {
        self.data
    }

    /// Returns provenance and completeness information for this value.
    pub fn metadata(&self) -> &ReadMetadata {
        &self.metadata
    }

    /// Consumes the result and returns both the value and its metadata.
    pub fn into_parts(self) -> (T, ReadMetadata) {
        (self.data, self.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_refactor_empty_data_requires_complete_metadata() {
        let complete = ReadResult::new(
            Vec::<u8>::new(),
            ReadMetadata::new(
                ReadSource::Live,
                CacheFreshness::NotApplicable,
                Utc::now(),
                ReadCoverage::Complete,
                None,
            ),
        );
        assert!(complete.data().is_empty());
        assert_eq!(complete.metadata().coverage(), ReadCoverage::Complete);

        let partial = ReadMetadata::new(
            ReadSource::Live,
            CacheFreshness::NotApplicable,
            Utc::now(),
            ReadCoverage::Partial(IncompleteReason::CompletionUnconfirmed),
            None,
        );
        assert_ne!(partial.coverage(), ReadCoverage::Complete);
    }

    #[test]
    fn backend_refactor_read_debug_does_not_print_business_payload() {
        let result = ReadResult::new(
            "private-business-content".to_owned(),
            ReadMetadata::new(
                ReadSource::Live,
                CacheFreshness::NotApplicable,
                Utc::now(),
                ReadCoverage::Complete,
                None,
            ),
        );

        assert!(!format!("{result:?}").contains("private-business-content"));
    }
}
