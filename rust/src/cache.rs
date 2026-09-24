use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    marker::PhantomData,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Cache payloads are normalized Rust values, never HTML or authentication
/// material.  Keep a hard upper bound nevertheless so a damaged or malicious
/// file cannot cause an unbounded allocation during application startup.
const MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024;
const CACHE_LOCK_SUFFIX: &str = ".lock";

/// The JSON envelope stored by a local cache file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonCacheEnvelope<T> {
    pub schema_version: u32,
    pub service: String,
    pub saved_at: String,
    pub payload: T,
}

/// Errors returned while reading or writing a local JSON cache.
#[derive(Debug)]
pub enum CacheError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    InvalidEnvelope {
        message: &'static str,
    },
    SchemaVersionMismatch {
        expected: u32,
        found: u32,
    },
    ServiceMismatch {
        expected: String,
        found: String,
    },
    EmptyPayload,
    TemporaryFileExhausted {
        directory: PathBuf,
    },
}

impl fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "cache {operation} failed for {}: {source}",
                path.display()
            ),
            Self::Serialize(error) => write!(formatter, "cache JSON serialization failed: {error}"),
            Self::Deserialize(error) => {
                write!(formatter, "cache JSON deserialization failed: {error}")
            }
            Self::InvalidEnvelope { message } => {
                write!(formatter, "invalid cache envelope: {message}")
            }
            Self::SchemaVersionMismatch { expected, found } => write!(
                formatter,
                "cache schema version mismatch: expected {expected}, found {found}"
            ),
            Self::ServiceMismatch { expected, found } => write!(
                formatter,
                "cache service mismatch: expected {expected:?}, found {found:?}"
            ),
            Self::EmptyPayload => formatter.write_str("cache payload must not be empty"),
            Self::TemporaryFileExhausted { directory } => write!(
                formatter,
                "could not allocate a unique cache temporary file in {}",
                directory.display()
            ),
        }
    }
}

impl Error for CacheError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize(error) | Self::Deserialize(error) => Some(error),
            Self::InvalidEnvelope { .. }
            | Self::SchemaVersionMismatch { .. }
            | Self::ServiceMismatch { .. }
            | Self::EmptyPayload
            | Self::TemporaryFileExhausted { .. } => None,
        }
    }
}

/// A typed JSON cache whose path is supplied by the caller.
pub struct JsonFileCache<T = Value> {
    path: PathBuf,
    schema_version: u32,
    service: String,
    marker: PhantomData<fn() -> T>,
}

impl<T> JsonFileCache<T> {
    pub fn new(path: impl AsRef<Path>, schema_version: u32, service: impl Into<String>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            schema_version,
            service: service.into(),
            marker: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// Removes the cache file. Clearing an already absent cache succeeds.
    pub fn clear(&self) -> Result<(), CacheError> {
        with_cache_lock(&self.path, || {
            reject_non_regular_target(&self.path)?;
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(CacheError::io("remove", &self.path, error)),
            }
        })
    }

    /// Writes a payload without persisting authentication material implicitly.
    pub fn write<P>(&self, payload: P) -> Result<(), CacheError>
    where
        P: Serialize,
    {
        crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::CacheWrite, || {
            self.write_inner(payload)
        })
    }

    fn write_inner<P: Serialize>(&self, payload: P) -> Result<(), CacheError> {
        let payload = serde_json::to_value(payload).map_err(CacheError::Serialize)?;
        if is_empty_payload(&payload) {
            return Err(CacheError::EmptyPayload);
        }

        let envelope = JsonCacheEnvelope {
            schema_version: self.schema_version,
            service: self.service.clone(),
            saved_at: saved_at(),
            payload: &payload,
        };
        let contents = serde_json::to_vec_pretty(&envelope).map_err(CacheError::Serialize)?;
        // Every reader, writer, and corrupt-file cleanup for one cache path
        // shares the same lock. Atomic rename protects readers from a partial
        // file, while the lock also prevents an old reader from deleting a
        // newer file after a concurrent refresh.
        let result = with_cache_lock(&self.path, || write_atomically(&self.path, &contents));
        if result.is_ok() {
            tracing::debug!(target:"tsinghua_kit::storage",event="cache_write_completed",bytes=contents.len() as u64);
        } else {
            tracing::warn!(target:"tsinghua_kit::storage",event="cache_write_failed",reason="storage_io");
        }
        result
    }
}

impl<T> JsonFileCache<T>
where
    T: DeserializeOwned,
{
    pub fn read(&self) -> Result<Option<JsonCacheEnvelope<T>>, CacheError> {
        let result = crate::telemetry::timing::measure_sync(
            crate::telemetry::timing::Phase::CacheRead,
            || with_cache_lock(&self.path, || self.read_unlocked()),
        );
        crate::telemetry::timing::cache_result(matches!(result, Ok(Some(_))), result.is_err());
        result
    }

    fn read_unlocked(&self) -> Result<Option<JsonCacheEnvelope<T>>, CacheError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                tracing::debug!(target:"tsinghua_kit::storage",event="cache_miss");
                return Ok(None);
            }
            Err(error) => {
                tracing::warn!(target:"tsinghua_kit::storage",event="cache_read_failed",reason="storage_metadata");
                return Err(CacheError::io("stat", &self.path, error));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CacheError::InvalidEnvelope {
                message: "cache target is not a regular file",
            });
        }
        ensure_private_file_permissions(&self.path)?;
        if metadata.len() == 0 || metadata.len() > MAX_CACHE_BYTES {
            let error = CacheError::InvalidEnvelope {
                message: "cache file size is outside the supported limit",
            };
            remove_corrupt_file(&self.path);
            return Err(error);
        }

        let contents = match fs::read(&self.path) {
            Ok(contents) => contents,
            // A file can disappear between stat and read.  Treat that as a
            // normal miss; permissions and other I/O failures remain errors
            // and are never deleted as if they were corrupt JSON.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                tracing::warn!(target:"tsinghua_kit::storage",event="cache_read_failed",reason="storage_io");
                return Err(CacheError::io("read", &self.path, error));
            }
        };

        let value: Value = match serde_json::from_slice(&contents) {
            Ok(value) => value,
            Err(error) => {
                let error = CacheError::Deserialize(error);
                remove_corrupt_file(&self.path);
                return Err(error);
            }
        };
        let metadata: JsonCacheEnvelope<Value> = match serde_json::from_value(value) {
            Ok(metadata) => metadata,
            Err(error) => {
                let error = CacheError::Deserialize(error);
                remove_corrupt_file(&self.path);
                return Err(error);
            }
        };

        if metadata.schema_version != self.schema_version {
            let error = CacheError::SchemaVersionMismatch {
                expected: self.schema_version,
                found: metadata.schema_version,
            };
            remove_corrupt_file(&self.path);
            return Err(error);
        }
        if metadata.service != self.service {
            let error = CacheError::ServiceMismatch {
                expected: self.service.clone(),
                found: metadata.service,
            };
            remove_corrupt_file(&self.path);
            return Err(error);
        }
        if metadata.saved_at.parse::<u128>().is_err() {
            let error = CacheError::InvalidEnvelope {
                message: "cache saved_at is not a Unix millisecond timestamp",
            };
            remove_corrupt_file(&self.path);
            return Err(error);
        }
        if is_empty_payload(&metadata.payload) {
            let error = CacheError::EmptyPayload;
            remove_corrupt_file(&self.path);
            return Err(error);
        }

        let JsonCacheEnvelope {
            schema_version,
            service,
            saved_at,
            payload,
        } = metadata;
        let payload = match serde_json::from_value(payload) {
            Ok(payload) => payload,
            Err(error) => {
                let error = CacheError::Deserialize(error);
                remove_corrupt_file(&self.path);
                return Err(error);
            }
        };
        tracing::debug!(target:"tsinghua_kit::storage",event="cache_read_completed",bytes=contents.len() as u64);

        Ok(Some(JsonCacheEnvelope {
            schema_version,
            service,
            saved_at,
            payload,
        }))
    }
}

/// Reads only the common JSON envelope while leaving schema/service/payload
/// interpretation to the caller. This is used by directory scanners whose
/// candidate path may contain a valid cache for another service: unlike
/// `JsonFileCache::read`, an envelope mismatch must never delete that file.
pub(crate) fn read_untyped_json_envelope(
    path: impl AsRef<Path>,
) -> Result<Option<JsonCacheEnvelope<Value>>, CacheError> {
    let path = path.as_ref().to_path_buf();
    with_cache_lock(&path, || {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CacheError::io("stat", &path, error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CacheError::InvalidEnvelope {
                message: "cache target is not a regular file",
            });
        }
        ensure_private_file_permissions(&path)?;
        if metadata.len() == 0 || metadata.len() > MAX_CACHE_BYTES {
            return Err(CacheError::InvalidEnvelope {
                message: "cache file size is outside the supported limit",
            });
        }
        let contents = fs::read(&path).map_err(|error| CacheError::io("read", &path, error))?;
        serde_json::from_slice(&contents)
            .map(Some)
            .map_err(|error| {
                // This helper deliberately does not remove malformed input: the
                // directory scanner cannot know whether the file belongs to a
                // different valid cache family.
                CacheError::Deserialize(error)
            })
    })
}

impl CacheError {
    fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

fn is_empty_payload(payload: &Value) -> bool {
    match payload {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => values.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

fn saved_at() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
        .to_string()
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), CacheError> {
    if contents.len() as u64 > MAX_CACHE_BYTES {
        return Err(CacheError::InvalidEnvelope {
            message: "cache payload exceeds the supported size",
        });
    }
    let directory = parent_directory(path);
    ensure_private_directory(directory)?;
    reject_non_regular_target(path)?;

    let (temporary_path, mut temporary_file) = create_temporary_file(path, directory)?;
    let mut cleanup = TemporaryPath::new(temporary_path.clone());
    let result = (|| {
        temporary_file
            .write_all(contents)
            .map_err(|error| CacheError::io("write temporary cache", &temporary_path, error))?;
        temporary_file
            .sync_all()
            .map_err(|error| CacheError::io("sync temporary cache", &temporary_path, error))?;
        drop(temporary_file);

        fs::rename(&temporary_path, path)
            .map_err(|error| CacheError::io("rename temporary cache", path, error))?;
        set_private_file_permissions(path)?;
        sync_directory(directory)?;
        Ok(())
    })();

    if result.is_ok() {
        cleanup.disarm();
    }
    result
}

fn cache_lock_path(path: &Path) -> PathBuf {
    let base_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "cache".into());
    parent_directory(path).join(format!(".{base_name}{CACHE_LOCK_SUFFIX}"))
}

/// Serializes all operations on one cache path across Runtime instances and
/// processes. The lock is deliberately a private sibling file rather than a
/// lock held on the cache itself: atomic replacement changes the cache inode,
/// so locking the destination would not protect a reader from a concurrent
/// rename.
fn with_cache_lock<T>(
    path: &Path,
    operation: impl FnOnce() -> Result<T, CacheError>,
) -> Result<T, CacheError> {
    let directory = parent_directory(path);
    ensure_private_directory(directory)?;
    let lock_path = cache_lock_path(path);
    reject_non_regular_target(&lock_path)?;

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|error| CacheError::io("open cache lock", &lock_path, error))?;
    // Tighten an old lock file created by an earlier version before acquiring
    // it. The cache directory is owner-only, but the lock itself must remain
    // owner-only after a permission migration as well.
    ensure_private_file_permissions(&lock_path)?;
    lock.lock_exclusive()
        .map_err(|error| CacheError::io("lock cache", &lock_path, error))?;

    let result = operation();
    let unlock = lock.unlock();
    if result.is_ok() && unlock.is_err() {
        return Err(CacheError::io(
            "unlock cache",
            &lock_path,
            unlock.expect_err("unlock error must be present"),
        ));
    }
    result
}

fn ensure_private_directory(directory: &Path) -> Result<(), CacheError> {
    fs::create_dir_all(directory)
        .map_err(|error| CacheError::io("create cache directory", directory, error))?;
    let metadata = fs::symlink_metadata(directory)
        .map_err(|error| CacheError::io("stat cache directory", directory, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CacheError::InvalidEnvelope {
            message: "cache directory is not a regular directory",
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| CacheError::io("chmod cache directory", directory, error))?;
    }
    Ok(())
}

fn reject_non_regular_target(path: &Path) -> Result<(), CacheError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CacheError::InvalidEnvelope {
                message: "cache target is not a regular file",
            })
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CacheError::io("stat cache target", path, error)),
    }
}

fn remove_corrupt_file(path: &Path) {
    // Only remove a regular file.  Never follow or replace a symlink while
    // cleaning malformed cache input, and never turn a permission/I/O failure
    // into a destructive directory operation.
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        let _ = fs::remove_file(path);
    }
}

fn set_private_file_permissions(path: &Path) -> Result<(), CacheError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| CacheError::io("chmod cache file", path, error))?;
    }
    Ok(())
}

pub(crate) fn ensure_private_file_permissions(path: &Path) -> Result<(), CacheError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| CacheError::io("stat cache file", path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CacheError::InvalidEnvelope {
                message: "cache target is not a regular file",
            });
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|error| CacheError::io("chmod cache file", path, error))?;
            let updated = fs::symlink_metadata(path)
                .map_err(|error| CacheError::io("stat cache file", path, error))?;
            if updated.file_type().is_symlink()
                || !updated.is_file()
                || updated.permissions().mode() & 0o777 != 0o600
            {
                return Err(CacheError::InvalidEnvelope {
                    message: "cache file could not be made private",
                });
            }
        }
    }
    Ok(())
}

fn sync_directory(directory: &Path) -> Result<(), CacheError> {
    let file = File::open(directory)
        .map_err(|error| CacheError::io("open cache directory", directory, error))?;
    file.sync_all()
        .map_err(|error| CacheError::io("sync cache directory", directory, error))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_temporary_file(path: &Path, directory: &Path) -> Result<(PathBuf, File), CacheError> {
    let base_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "cache".into());
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());

    for attempt in 0..64 {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary_path = directory.join(format!(
            ".{base_name}.tmp-{}-{timestamp}-{counter}-{attempt}",
            process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary_path) {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(CacheError::io(
                    "create temporary cache",
                    &temporary_path,
                    error,
                ));
            }
        }
    }

    Err(CacheError::TemporaryFileExhausted {
        directory: directory.to_path_buf(),
    })
}

struct TemporaryPath {
    path: PathBuf,
    armed: bool,
}

impl TemporaryPath {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct TestPayload {
        value: String,
    }

    struct TestPath(PathBuf);

    impl TestPath {
        fn new(label: &str) -> Self {
            let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let path = env::temp_dir().join(format!(
                "thyou-cache-{label}-{}-{timestamp}-{counter}",
                process::id()
            ));
            fs::create_dir_all(&path).expect("test cache directory should be creatable");
            Self(path)
        }

        fn path(&self) -> PathBuf {
            self.0.join("cache.json")
        }
    }

    impl Drop for TestPath {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.path());
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn round_trips_payload_and_metadata() {
        let path = TestPath::new("round-trip");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 3, "learn");
        let payload = TestPayload {
            value: "assignment".to_owned(),
        };

        cache.write(&payload).expect("cache write should succeed");
        let envelope = cache
            .read()
            .expect("cache read should succeed")
            .expect("cache file should exist");

        assert_eq!(envelope.schema_version, 3);
        assert_eq!(envelope.service, "learn");
        assert!(!envelope.saved_at.is_empty());
        assert_eq!(envelope.payload, payload);
    }

    #[test]
    fn missing_cache_and_clear_are_idempotent() {
        let path = TestPath::new("clear");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");

        assert!(cache.read().expect("missing read should succeed").is_none());
        cache
            .clear()
            .expect("clearing a missing cache should succeed");
        cache
            .write(TestPayload {
                value: "cached".to_owned(),
            })
            .expect("cache write should succeed");
        cache.clear().expect("cache clear should succeed");
        assert!(
            cache
                .read()
                .expect("read after clear should succeed")
                .is_none()
        );
    }

    #[test]
    fn rejects_schema_version_mismatch() {
        let path = TestPath::new("schema");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 2, "registrar");
        let envelope = serde_json::json!({
            "schema_version": 1,
            "service": "registrar",
            "saved_at": "123",
            "payload": {"value": "old"}
        });
        fs::write(
            path.path(),
            serde_json::to_vec(&envelope).expect("JSON should encode"),
        )
        .expect("fixture should write");

        assert!(matches!(
            cache.read(),
            Err(CacheError::SchemaVersionMismatch {
                expected: 2,
                found: 1
            })
        ));
        assert!(!path.path().exists());
    }

    #[test]
    fn rejects_empty_payload_on_write_and_read() {
        let write_path = TestPath::new("empty-write");
        let write_cache = JsonFileCache::<Value>::new(write_path.path(), 1, "overview");
        assert!(matches!(
            write_cache.write(Value::Null),
            Err(CacheError::EmptyPayload)
        ));

        let read_path = TestPath::new("empty-read");
        let read_cache = JsonFileCache::<Value>::new(read_path.path(), 1, "overview");
        let envelope = serde_json::json!({
            "schema_version": 1,
            "service": "overview",
            "saved_at": "123",
            "payload": []
        });
        fs::write(
            read_path.path(),
            serde_json::to_vec(&envelope).expect("JSON should encode"),
        )
        .expect("fixture should write");

        assert!(matches!(read_cache.read(), Err(CacheError::EmptyPayload)));
        assert!(!read_path.path().exists());
    }

    #[test]
    fn backend_repair_corrupt_json_is_removed_but_not_retried() {
        let path = TestPath::new("corrupt-json");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");
        fs::create_dir_all(path.path().parent().unwrap()).expect("cache directory should exist");
        fs::write(path.path(), b"{not-json").expect("corrupt fixture should write");

        assert!(matches!(cache.read(), Err(CacheError::Deserialize(_))));
        assert!(!path.path().exists());
        assert!(
            cache
                .read()
                .expect("removed corrupt cache is a miss")
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn backend_repair_cache_directory_and_file_are_private_and_symlinks_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let path = TestPath::new("permissions");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");
        cache
            .write(TestPayload {
                value: "private".to_owned(),
            })
            .expect("cache write should succeed");
        assert_eq!(
            fs::metadata(path.0.as_path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(path.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let symlink_path = path.0.join("symlink.json");
        symlink(path.path(), &symlink_path).expect("symlink fixture should be creatable");
        let symlink_cache = JsonFileCache::<TestPayload>::new(&symlink_path, 1, "overview");
        assert!(matches!(
            symlink_cache.read(),
            Err(CacheError::InvalidEnvelope { .. })
        ));
        assert!(symlink_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn backend_repair_read_tightens_legacy_cache_permissions_before_decoding() {
        use std::os::unix::fs::PermissionsExt;

        let path = TestPath::new("legacy-permissions");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");
        let envelope = serde_json::json!({
            "schema_version": 1,
            "service": "overview",
            "saved_at": "123",
            "payload": {"value": "legacy"}
        });
        fs::write(
            path.path(),
            serde_json::to_vec(&envelope).expect("JSON should encode"),
        )
        .expect("legacy cache fixture should write");
        fs::set_permissions(path.path(), fs::Permissions::from_mode(0o644))
            .expect("legacy cache fixture should be broad before read");

        let result = cache
            .read()
            .expect("legacy cache should remain readable")
            .expect("legacy cache should exist");
        assert_eq!(result.payload.value, "legacy");
        assert_eq!(
            fs::metadata(path.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn backend_repair_cache_lock_is_private_and_rejects_lock_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let path = TestPath::new("cache-lock");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");
        cache
            .write(TestPayload {
                value: "locked".to_owned(),
            })
            .expect("cache write should succeed");

        let lock_path = cache_lock_path(cache.path());
        assert_eq!(
            fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let target = path.0.join("lock-target");
        fs::write(&target, b"not a lock").expect("lock target should be writable");
        fs::remove_file(&lock_path).expect("fixture lock should be removable");
        symlink(&target, &lock_path).expect("lock symlink should be creatable");

        assert!(matches!(
            cache.read(),
            Err(CacheError::InvalidEnvelope { .. })
        ));
        assert!(target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn backend_repair_cache_reader_waits_for_writer_lock() {
        use std::{sync::mpsc, thread, time::Duration};

        let path = TestPath::new("cache-lock-serial");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");
        cache
            .write(TestPayload {
                value: "before".to_owned(),
            })
            .expect("initial cache write should succeed");

        // Re-open and hold the sibling lock explicitly so another thread must
        // wait. This exercises the same cross-process primitive used by the
        // public read/write methods without writing a test-only cache format.
        let lock_path = cache_lock_path(cache.path());
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("cache lock should exist");
        lock.lock_exclusive().expect("test lock should be acquired");

        let (sender, receiver) = mpsc::channel();
        let thread_cache = JsonFileCache::<TestPayload>::new(cache.path(), 1, "overview");
        let worker = thread::spawn(move || {
            let result = thread_cache.read().map(|value| value.is_some());
            sender.send(result).expect("reader result should be sent");
        });

        assert!(
            receiver.recv_timeout(Duration::from_millis(250)).is_err(),
            "cache reader must not pass the writer lock"
        );
        lock.unlock().expect("test lock should be released");
        let result = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("reader should continue after lock release")
            .expect("reader should succeed");
        assert!(result);
        worker.join().expect("reader thread should exit");
    }

    #[test]
    fn backend_repair_untyped_envelope_probe_keeps_foreign_cache_intact() {
        let path = TestPath::new("untyped-envelope");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 7, "other-service");
        cache
            .write(TestPayload {
                value: "belongs elsewhere".to_owned(),
            })
            .expect("foreign cache should be writable");

        let envelope = read_untyped_json_envelope(cache.path())
            .expect("generic envelope probe should succeed")
            .expect("foreign cache should exist");
        assert_eq!(envelope.schema_version, 7);
        assert_eq!(envelope.service, "other-service");
        assert!(path.path().exists());
    }

    #[test]
    fn failed_rename_removes_temporary_file() {
        let path = TestPath::new("rename-failure");
        fs::create_dir(path.path()).expect("target directory should be creatable");
        let cache = JsonFileCache::<TestPayload>::new(path.path(), 1, "overview");
        let payload = TestPayload {
            value: "will fail".to_owned(),
        };
        let prefix = format!(
            ".{}.tmp-",
            path.path()
                .file_name()
                .expect("test path should have a file name")
                .to_string_lossy()
        );

        assert!(cache.write(&payload).is_err());

        let temporary_files = fs::read_dir(env::temp_dir())
            .expect("temporary directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .count();
        assert_eq!(temporary_files, 0);
    }
}
