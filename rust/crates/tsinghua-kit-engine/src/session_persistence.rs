#[path = "session_authority.rs"]
mod authority;
#[cfg(test)]
pub(crate) use authority::begin_explicit_authority;
pub(crate) use authority::{
    SessionLease, begin_explicit_authority_with_opt_in, load_authorized_state, revoke_authority,
};

use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    io::Write,
    path::{Path, PathBuf},
};

const SCHEMA: u32 = 2;
// Version 2 marks the SDK's Identity-only checkpoint semantics. Older
// snapshots may have been refreshed from the shared jar after SelfService or
// service handoffs, so they are deliberately rejected rather than migrated.
const MAX_AGE: Duration = Duration::days(30);
// Account metadata contains no authentication material, but it can locate a
// matching explicitly saved private-vault record and account-scoped cache. Keep
// that recovery context bounded independently of the shorter Cookie lifetime;
// every successful login/service proof refreshes the timestamp.
const ACCOUNT_METADATA_MAX_AGE: Duration = Duration::days(365);
const MAX_PAYLOAD_BYTES: usize = 512 * 1024;
const SESSION_FILE: &str = "campus-session-v3.json";
const LEGACY_SESSION_FILES: [&str; 3] = [
    "campus-session-v2.bin",
    "campus-session-v1.json",
    "campus-session-key-v1.bin",
];
const SESSION_LOCK_FILE: &str = "campus-session-store-v1.lock";
const ACCOUNT_METADATA_SCHEMA: u32 = 1;
const ACCOUNT_METADATA_FILE: &str = "campus-account-v1.json";
const RESUME_DIAGNOSTIC_FILE: &str = "backend-session-resume.json";
const STORAGE_DIR_OVERRIDE: &str = "THYOU_SESSION_DIR";

/// A resumable authenticated browser session.
///
/// This intentionally contains no account password. The serialized cookie
/// store is still an authentication credential and therefore lives only in
/// the application's private data directory with owner-only Unix permissions.
/// The file backend is deliberately the same Rust-owned persistence boundary
/// on every platform; it does not call an operating-system credential or
/// secret-service API.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResumeSnapshot {
    schema: u32,
    pub(crate) username: String,
    pub(crate) saved_at: DateTime<Utc>,
    /// A completed bootstrap is a navigation checkpoint, not a service proof.
    /// Restored services still validate their own responses/CSRF/account.
    #[serde(default)]
    pub(crate) portal_bootstrap_completed: bool,
    /// Stable trusted-device binding, not a password, ticket or CSRF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_fingerprint: Option<String>,
    cookie_store_json: String,
}

/// Non-sensitive account context used to locate an explicitly saved private
/// credential after the Cookie snapshot has expired.  This file is not an
/// authentication proof: it contains no password, Cookie, ticket, CSRF token,
/// response body, or service-session flag.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ResumeAccountMetadata {
    schema: u32,
    pub(crate) username: String,
    pub(crate) academic_stage: Option<crate::protocol::AcademicStage>,
    pub(crate) last_successful_login: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_fingerprint: Option<String>,
    #[serde(default)]
    pub(crate) credentials_saved: bool,
    /// A manual LoginPage stage choice is a routing preference, not live
    /// service proof.  Keep its provenance so a later automatic recovery can
    /// preserve the user's explicit override instead of re-running the
    /// student-id heuristic and silently changing the selected endpoint.
    #[serde(default)]
    pub(crate) stage_selection_explicit: bool,
}

impl fmt::Debug for ResumeAccountMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumeAccountMetadata")
            .field("schema", &self.schema)
            .field("username", &"[redacted]")
            .field("academic_stage", &self.academic_stage)
            .field("last_successful_login", &self.last_successful_login)
            .field("credentials_saved", &self.credentials_saved)
            .field("stage_selection_explicit", &self.stage_selection_explicit)
            .finish()
    }
}

impl ResumeAccountMetadata {
    pub(crate) fn new(
        username: &str,
        academic_stage: Option<crate::protocol::AcademicStage>,
        device_fingerprint: Option<&str>,
        credentials_saved: bool,
    ) -> Result<Self, String> {
        Self::new_with_stage_selection(
            username,
            academic_stage,
            device_fingerprint,
            credentials_saved,
            false,
        )
    }

    pub(crate) fn new_with_stage_selection(
        username: &str,
        academic_stage: Option<crate::protocol::AcademicStage>,
        device_fingerprint: Option<&str>,
        credentials_saved: bool,
        stage_selection_explicit: bool,
    ) -> Result<Self, String> {
        let username = username.trim();
        if username.is_empty() || username.len() > 128 || username.chars().any(char::is_control) {
            return Err(String::from("resume account metadata is invalid"));
        }
        let device_fingerprint = match device_fingerprint {
            Some(value) if is_valid_device_fingerprint(value) => Some(value.to_owned()),
            Some(_) => return Err(String::from("resume account metadata is invalid")),
            None => None,
        };
        if stage_selection_explicit && academic_stage.is_none() {
            return Err(String::from("resume account metadata is invalid"));
        }
        Ok(Self {
            schema: ACCOUNT_METADATA_SCHEMA,
            username: username.to_owned(),
            academic_stage,
            last_successful_login: Utc::now(),
            device_fingerprint,
            credentials_saved,
            stage_selection_explicit,
        })
    }

    pub(crate) fn with_credentials_saved(mut self, credentials_saved: bool) -> Self {
        self.credentials_saved = credentials_saved;
        self
    }

    pub(crate) fn device_fingerprint(&self) -> Option<&str> {
        self.device_fingerprint.as_deref()
    }

    fn is_well_formed(&self, now: DateTime<Utc>) -> bool {
        self.schema == ACCOUNT_METADATA_SCHEMA
            && !self.username.trim().is_empty()
            && self.username.len() <= 128
            && !self.username.chars().any(char::is_control)
            && self.last_successful_login <= now + Duration::minutes(5)
            && self
                .device_fingerprint
                .as_deref()
                .is_none_or(is_valid_device_fingerprint)
            && (!self.stage_selection_explicit || self.academic_stage.is_some())
    }

    fn is_current(&self, now: DateTime<Utc>) -> bool {
        self.is_well_formed(now)
            // A user who explicitly opted into the private credential files
            // has chosen a durable recovery boundary. Keep that non-secret
            // locator until logout/revocation; unsaved account-only cache
            // shells remain bounded to avoid retaining abandoned accounts.
            && (self.credentials_saved
                || now - self.last_successful_login <= ACCOUNT_METADATA_MAX_AGE)
    }
}

impl fmt::Debug for ResumeSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumeSnapshot")
            .field("schema", &self.schema)
            .field("username", &"[redacted]")
            .field("saved_at", &self.saved_at)
            .field("cookie_payload_len", &self.cookie_store_json.len())
            .finish()
    }
}

impl ResumeSnapshot {
    pub(crate) fn new(username: &str, cookie_store: &[u8]) -> Result<Self, String> {
        let username = username.trim();
        if username.is_empty()
            || username.len() > 128
            || username.chars().any(char::is_control)
            || cookie_store.is_empty()
            || cookie_store.len() > MAX_PAYLOAD_BYTES
        {
            return Err(String::from("resume session input is invalid"));
        }
        Ok(Self {
            schema: SCHEMA,
            username: username.to_owned(),
            saved_at: Utc::now(),
            portal_bootstrap_completed: false,
            cookie_store_json: {
                let value = std::str::from_utf8(cookie_store).map_err(|_| {
                    String::from("resume session cookie payload must be UTF-8 JSON")
                })?;
                serde_json::from_str::<serde_json::Value>(value).map_err(|_| {
                    String::from("resume session cookie payload must be valid JSON")
                })?;
                value.to_owned()
            },
            device_fingerprint: None,
        })
    }

    pub(crate) fn device_fingerprint(&self) -> Option<&str> {
        self.device_fingerprint
            .as_deref()
            .filter(|value| is_valid_device_fingerprint(value))
    }

    pub(crate) fn with_device_fingerprint(mut self, value: &str) -> Result<Self, String> {
        if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("resume device metadata is invalid".to_owned());
        }
        self.device_fingerprint = Some(value.to_owned());
        Ok(self)
    }

    pub(crate) fn cookie_store(&self) -> Result<Vec<u8>, String> {
        let bytes = self.cookie_store_json.as_bytes().to_vec();
        if bytes.is_empty()
            || bytes.len() > MAX_PAYLOAD_BYTES
            || serde_json::from_slice::<serde_json::Value>(&bytes).is_err()
        {
            return Err(String::from("resume session cookie payload is invalid"));
        }
        Ok(bytes)
    }

    pub(crate) fn expires_at(&self) -> DateTime<Utc> {
        self.saved_at + MAX_AGE
    }

    fn is_well_formed(&self, now: DateTime<Utc>) -> bool {
        self.schema == SCHEMA
            && !self.username.trim().is_empty()
            && self.username.len() <= 128
            && !self.username.chars().any(char::is_control)
            && self.saved_at <= now + Duration::minutes(5)
            && self
                .device_fingerprint
                .as_deref()
                .is_none_or(is_valid_device_fingerprint)
            && self.cookie_store().is_ok()
    }

    fn is_current(&self, now: DateTime<Utc>) -> bool {
        self.is_well_formed(now) && now < self.expires_at()
    }
}

fn is_valid_device_fingerprint(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(unix)]
fn private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)
        .map_err(|_| String::from("resume session file metadata is unavailable"))?
        .permissions()
        .mode();
    if mode & 0o777 != 0o600 {
        return Err(String::from("resume session file is not private storage"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn encode_snapshot(snapshot: &ResumeSnapshot) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|_| String::from("resume session could not be encoded"))?;
    if bytes.is_empty() || bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(String::from("resume session payload is too large"));
    }
    Ok(bytes)
}

fn decode_snapshot(bytes: &[u8]) -> Result<ResumeSnapshot, String> {
    if bytes.is_empty() || bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(String::from("resume session JSON is invalid"));
    }
    serde_json::from_slice(bytes).map_err(|_| String::from("resume session JSON is invalid"))
}

fn storage_dir() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(STORAGE_DIR_OVERRIDE) {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err(String::from(
            "resume session directory override must be absolute",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| String::from("resume session home directory is unavailable"))?;
        const CONTAINER_MARKER: &str = "/Library/Containers/com.thyou.thyou/Data";
        let container = if home.to_string_lossy().contains(CONTAINER_MARKER) {
            home
        } else {
            home.join("Library/Containers/com.thyou.thyou/Data")
        };
        return Ok(container.join("Library/Application Support/THYou"));
    }

    #[cfg(target_os = "ios")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| String::from("resume session home directory is unavailable"))?;
        return Ok(home.join("Library/Application Support/THYou"));
    }

    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .ok_or_else(|| String::from("resume session app data directory is unavailable"))?;
        return Ok(root.join("THYou"));
    }

    #[cfg(target_os = "android")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| String::from("resume session app home directory is unavailable"))?;
        return Ok(home.join("files/THYou"));
    }

    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        if let Some(root) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            && root.is_absolute()
        {
            return Ok(root.join("THYou"));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| String::from("resume session home directory is unavailable"))?;
        return Ok(home.join(".local/share/THYou"));
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| String::from("resume session home directory is unavailable"))?;
        Ok(home.join(".thyou"))
    }
}

/// Returns the private application-data root used by both resumable sessions
/// and Rust-owned slow-changing caches.  Callers only receive the directory
/// location; credentials, cookies, tickets, and CSRF values never leave the
/// persistence module through this helper.
pub(crate) fn application_data_dir() -> Result<PathBuf, String> {
    storage_dir()
}

fn session_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join(SESSION_FILE))
}

fn account_metadata_path() -> Result<PathBuf, String> {
    Ok(storage_dir()?.join(ACCOUNT_METADATA_FILE))
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path)
        .map_err(|_| String::from("resume session directory could not be created"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| String::from("resume session directory metadata is unavailable"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(String::from(
            "resume session directory is not private storage",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| String::from("resume session directory permissions could not be set"))?;
    }
    Ok(())
}

fn private_writer(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| String::from("resume session file could not be opened"))
}

fn session_lock_path(parent: &Path) -> PathBuf {
    parent.join(SESSION_LOCK_FILE)
}

/// All session snapshots and account recovery metadata share one lock.  The
/// two files form one recovery boundary: a reader must not observe metadata
/// from one login together with a Cookie snapshot from another login, and a
/// writer must not let an atomic rename race with a constructor's cleanup.
fn open_session_store_lock(parent: &Path) -> Result<File, String> {
    let path = session_lock_path(parent);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(String::from(
                "resume session lock is not regular private storage",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return Err(String::from("resume session lock metadata is unavailable")),
    }

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|_| String::from("resume session lock could not be opened"))?;
    if private_file_permissions(&path).is_err() {
        return Err(String::from("resume session lock is not private storage"));
    }
    file.try_lock_exclusive()
        .map_err(|_| String::from("resume session lock could not be acquired"))?;
    Ok(file)
}

fn with_session_store_lock<T>(
    parent: &Path,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    ensure_private_directory(parent)?;
    let lock = open_session_store_lock(parent)?;
    let result = operation();
    let unlock = lock.unlock();
    if result.is_ok() && unlock.is_err() {
        return Err(String::from("resume session lock could not be released"));
    }
    result
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            String::from("resume session file is not regular private storage"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(String::from("resume session file metadata is unavailable")),
    }
}

fn commit_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let backup = destination.with_extension("json.bak");
        let _ = fs::remove_file(&backup);
        if destination.exists() {
            fs::rename(destination, &backup)
                .map_err(|_| String::from("resume session previous file could not be staged"))?;
        }
        if fs::rename(temporary, destination).is_err() {
            let _ = fs::rename(&backup, destination);
            return Err(String::from("resume session file could not be committed"));
        }
        let _ = fs::remove_file(&backup);
    }
    #[cfg(not(target_os = "windows"))]
    {
        fs::rename(temporary, destination)
            .map_err(|_| String::from("resume session file could not be committed"))?;
    }
    Ok(())
}

fn save_at_unlocked(path: &Path, snapshot: &ResumeSnapshot) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("resume session path is invalid"))?;
    ensure_private_directory(parent)?;
    ensure_no_legacy_session_files(parent)?;
    reject_symlink(path)?;
    let payload = encode_snapshot(snapshot)?;
    let suffix = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let temporary = parent.join(format!(
        ".{SESSION_FILE}.tmp-{}-{suffix}",
        std::process::id()
    ));
    let mut file = private_writer(&temporary)?;
    let write_result = file
        .write_all(&payload)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all());
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(String::from("resume session file could not be written"));
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(String::from(
                "resume session file permissions could not be set",
            ));
        }
    }
    if let Err(error) = commit_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| String::from("resume session file permissions could not be set"))?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

fn save_at(path: &Path, snapshot: &ResumeSnapshot) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("resume session path is invalid"))?;
    with_session_store_lock(parent, || save_at_unlocked(path, snapshot))
}

fn clear_at_unlocked(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(String::from("resume session file could not be removed")),
    }
}

fn clear_legacy_session_files_unlocked(parent: &Path) -> Result<(), String> {
    for name in LEGACY_SESSION_FILES {
        clear_at_unlocked(&parent.join(name))?;
    }
    Ok(())
}

pub(crate) fn ensure_no_legacy_session_files(parent: &Path) -> Result<(), String> {
    for name in LEGACY_SESSION_FILES {
        match fs::symlink_metadata(parent.join(name)) {
            Ok(_) => {
                return Err(String::from(
                    "legacy session storage exists; preserve it and remove it explicitly before using JSON persistence",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(String::from("legacy session storage is unavailable")),
        }
    }
    Ok(())
}

fn clear_at(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("resume session path is invalid"))?;
    with_session_store_lock(parent, || clear_at_unlocked(path))
}

fn load_at_unlocked(path: &Path) -> Result<Option<ResumeSnapshot>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(String::from("resume session file metadata is unavailable")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(String::from(
            "resume session file is not a regular file; the file was preserved",
        ));
    }
    if private_file_permissions(path).is_err() {
        return Err(String::from(
            "resume session file permissions are invalid; the file was preserved",
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_PAYLOAD_BYTES as u64 {
        return Err(String::from(
            "resume session file size is invalid; the file was preserved",
        ));
    }
    let payload =
        fs::read(path).map_err(|_| String::from("resume session file could not be read"))?;
    let snapshot = decode_snapshot(&payload)
        .map_err(|_| String::from("resume session JSON is invalid; the file was preserved"))?;
    let now = Utc::now();
    if !snapshot.is_well_formed(now) {
        return Err(String::from(
            "resume session data is invalid; the file was preserved",
        ));
    }
    if !snapshot.is_current(now) {
        clear_at_unlocked(path)?;
        return Ok(None);
    }
    Ok(Some(snapshot))
}

fn load_at(path: &Path) -> Result<Option<ResumeSnapshot>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("resume session path is invalid"))?;
    with_session_store_lock(parent, || load_at_unlocked(path))
}

fn load_from_root(root: &Path) -> Result<Option<ResumeSnapshot>, String> {
    with_session_store_lock(root, || {
        ensure_no_legacy_session_files(root)?;
        load_at_unlocked(&root.join(SESSION_FILE))
    })
}

pub(crate) fn save_at_root(root: &Path, snapshot: &ResumeSnapshot) -> Result<(), String> {
    if snapshot.device_fingerprint().is_none() {
        return Err(String::from("resume session device binding is required"));
    }
    with_session_store_lock(root, || {
        save_at_unlocked(&root.join(SESSION_FILE), snapshot)
    })
}

pub(crate) fn save(snapshot: &ResumeSnapshot) -> Result<(), String> {
    save_at_root(&storage_dir()?, snapshot)
}

fn load_account_metadata_at_unlocked(path: &Path) -> Result<Option<ResumeAccountMetadata>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(String::from("resume account metadata is unavailable")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(String::from(
            "resume account metadata is not a regular file; the file was preserved",
        ));
    }
    if metadata.len() == 0 || metadata.len() > 16 * 1024 {
        return Err(String::from(
            "resume account metadata size is invalid; the file was preserved",
        ));
    }
    private_file_permissions(path).map_err(|_| {
        String::from("resume account metadata permissions are invalid; the file was preserved")
    })?;
    let payload =
        fs::read(path).map_err(|_| String::from("resume account metadata could not be read"))?;
    let metadata: ResumeAccountMetadata = serde_json::from_slice(&payload).map_err(|_| {
        String::from("resume account metadata JSON is invalid; the file was preserved")
    })?;
    if !metadata.is_well_formed(Utc::now()) {
        return Err(String::from(
            "resume account metadata is invalid; the file was preserved",
        ));
    }
    if !metadata.is_current(Utc::now()) {
        clear_at_unlocked(path)?;
        return Ok(None);
    }
    Ok(Some(metadata))
}

fn load_account_metadata_at(path: &Path) -> Result<Option<ResumeAccountMetadata>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("resume account metadata path is invalid"))?;
    with_session_store_lock(parent, || load_account_metadata_at_unlocked(path))
}

fn save_account_metadata_at_unlocked(
    path: &Path,
    metadata: &ResumeAccountMetadata,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| String::from("resume account metadata path is invalid"))?;
    ensure_private_directory(parent)?;
    reject_symlink(&path)?;
    let payload = serde_json::to_vec(metadata)
        .map_err(|_| String::from("resume account metadata could not be encoded"))?;
    if payload.is_empty() || payload.len() > 16 * 1024 {
        return Err(String::from("resume account metadata is too large"));
    }
    let temporary = parent.join(format!(
        ".{ACCOUNT_METADATA_FILE}.tmp-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = private_writer(&temporary)?;
    if file
        .write_all(&payload)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        return Err(String::from("resume account metadata could not be written"));
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(String::from(
                "resume account metadata permissions could not be set",
            ));
        }
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "resume account metadata could not be committed: {error}"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| String::from("resume account metadata permissions could not be set"))?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

pub(crate) fn save_account_metadata_at_root(
    root: &Path,
    metadata: &ResumeAccountMetadata,
) -> Result<(), String> {
    with_session_store_lock(root, || {
        save_account_metadata_at_unlocked(&root.join(ACCOUNT_METADATA_FILE), metadata)
    })
}

/// Commits the readable Cookie snapshot and its non-secret account locator
/// as one recovery boundary. Each file is still written through its own
/// atomic temporary-file replacement, but the shared store lock prevents a
/// constructor or another Runtime from observing the pair between commits.
/// If the second commit fails, remove the session snapshot so a half-written
/// pair cannot authorize or scope a later restore.
pub(crate) fn save_resume_state_at_root(
    root: &Path,
    snapshot: &ResumeSnapshot,
    metadata: &ResumeAccountMetadata,
) -> Result<(), String> {
    if snapshot.device_fingerprint().is_none()
        || snapshot.username != metadata.username
        || metadata.device_fingerprint() != snapshot.device_fingerprint()
    {
        return Err(String::from("resume state account binding is invalid"));
    }
    with_session_store_lock(root, || {
        ensure_no_legacy_session_files(root)?;
        let session_path = root.join(SESSION_FILE);
        if let Err(error) = save_at_unlocked(&session_path, snapshot) {
            let _ = clear_at_unlocked(&session_path);
            return Err(error);
        }
        if let Err(error) =
            save_account_metadata_at_unlocked(&root.join(ACCOUNT_METADATA_FILE), metadata)
        {
            // The metadata may have failed before or after its rename. In
            // both cases removing the snapshot is the safe outcome: the
            // remaining metadata can at most scope a cache shell or a later
            // explicit private-vault recovery, never authorize Cookie use.
            let _ = clear_at_unlocked(&session_path);
            return Err(error);
        }
        Ok(())
    })
}

pub(crate) fn save_account_metadata(metadata: &ResumeAccountMetadata) -> Result<(), String> {
    save_account_metadata_at_root(&storage_dir()?, metadata)
}

pub(crate) fn load_account_metadata_at_root(
    root: &Path,
) -> Result<Option<ResumeAccountMetadata>, String> {
    with_session_store_lock(root, || {
        load_account_metadata_at_unlocked(&root.join(ACCOUNT_METADATA_FILE))
    })
}

pub(crate) fn load_account_metadata() -> Result<Option<ResumeAccountMetadata>, String> {
    load_account_metadata_at_root(&storage_dir()?)
}

pub(crate) fn clear_session_at_root(root: &Path) -> Result<(), String> {
    with_session_store_lock(root, || {
        clear_at_unlocked(&root.join(SESSION_FILE))?;
        clear_legacy_session_files_unlocked(root)
    })
}

pub(crate) fn clear_session() -> Result<(), String> {
    clear_session_at_root(&storage_dir()?)
}

pub(crate) fn clear_account_metadata_at_root(root: &Path) -> Result<(), String> {
    with_session_store_lock(root, || {
        clear_at_unlocked(&root.join(ACCOUNT_METADATA_FILE))
    })
}

pub(crate) fn clear_account_metadata() -> Result<(), String> {
    clear_account_metadata_at_root(&storage_dir()?)
}

pub(crate) fn load() -> Result<Option<ResumeSnapshot>, String> {
    load_from_root(&storage_dir()?)
}

pub(crate) fn load_at_root(root: &Path) -> Result<Option<ResumeSnapshot>, String> {
    load_from_root(root)
}

pub(crate) fn clear_at_root(root: &Path) -> Result<(), String> {
    with_session_store_lock(root, || {
        clear_at_unlocked(&root.join(SESSION_FILE))?;
        clear_legacy_session_files_unlocked(root)?;
        clear_at_unlocked(&root.join(ACCOUNT_METADATA_FILE))
    })
}

pub(crate) fn clear() -> Result<(), String> {
    clear_at_root(&storage_dir()?)
}

#[cfg(all(debug_assertions, not(test)))]
pub(crate) fn record_resume_diagnostic_at(root: &Path, status: &str) {
    tracing::info!(target:"tsinghua_kit::storage",event="session_restore",outcome=status);
    if ensure_private_directory(root).is_err() {
        return;
    }
    let path = root.join(RESUME_DIAGNOSTIC_FILE);
    let encoded = serde_json::json!({
        "schema": 1,
        "storage": "private_file",
        "status": status,
        "updated_at": Utc::now(),
    });
    let Ok(bytes) = serde_json::to_vec_pretty(&encoded) else {
        return;
    };
    let temporary = root.join(format!(".{RESUME_DIAGNOSTIC_FILE}.tmp"));
    let Ok(mut file) = private_writer(&temporary) else {
        return;
    };
    if file
        .write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        return;
    }
    drop(file);
    if commit_file(&temporary, &path).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(all(debug_assertions, not(test)))]
pub(crate) fn record_resume_diagnostic(status: &str) {
    if let Ok(root) = storage_dir() {
        record_resume_diagnostic_at(&root, status);
    }
}

#[cfg(any(not(debug_assertions), test))]
pub(crate) fn record_resume_diagnostic(_status: &str) {}

#[cfg(any(not(debug_assertions), test))]
pub(crate) fn record_resume_diagnostic_at(_root: &Path, _status: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_resume_retains_trusted_device_binding_without_credentials_in_debug() {
        let device = "0123456789abcdef0123456789abcdef";
        let old = ResumeSnapshot::new("fixture-user", b"[]").unwrap();
        assert!(old.device_fingerprint().is_none());
        let snapshot = old.clone().with_device_fingerprint(device).unwrap();
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let restored: ResumeSnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.device_fingerprint(), Some(device));
        let debug = format!("{restored:?}");
        assert!(!debug.contains(device));
        assert!(!debug.contains("fixture-user"));
        assert!(old.with_device_fingerprint("invalid").is_err());
        let fields = serde_json::to_value(&snapshot).unwrap();
        for forbidden in ["password", "ticket", "csrf", "html"] {
            assert!(fields.get(forbidden).is_none());
        }
    }

    #[test]
    fn backend_repair_resume_checkpoint_is_metadata_not_service_proof() {
        let mut snapshot = ResumeSnapshot::new("fixture-private-user", b"[]").unwrap();
        assert!(!snapshot.portal_bootstrap_completed);
        assert!(!format!("{snapshot:?}").contains("fixture-private-user"));
        snapshot.portal_bootstrap_completed = true;
        let mut encoded = serde_json::to_value(&snapshot).unwrap();
        assert!(
            serde_json::from_value::<ResumeSnapshot>(encoded.clone())
                .unwrap()
                .portal_bootstrap_completed
        );
        encoded
            .as_object_mut()
            .unwrap()
            .remove("portal_bootstrap_completed");
        assert!(
            !serde_json::from_value::<ResumeSnapshot>(encoded)
                .unwrap()
                .portal_bootstrap_completed
        );
    }

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "thyou-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn backend_repair_resume_snapshot_round_trip_has_no_password_ticket_or_csrf_fields() {
        let source = br#"{"cookie":"fixture"}"#;
        let snapshot = ResumeSnapshot::new("fixture-user", source).unwrap();
        assert_eq!(snapshot.cookie_store().unwrap(), source);
        assert!(snapshot.is_current(Utc::now()));
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert!(!encoded.to_ascii_lowercase().contains("password"));
        assert!(!encoded.to_ascii_lowercase().contains("ticket"));
        assert!(!encoded.to_ascii_lowercase().contains("csrf"));
    }

    #[test]
    fn backend_repair_account_metadata_round_trip_has_only_safe_recovery_context() {
        let metadata = ResumeAccountMetadata::new(
            "fixture-user",
            Some(crate::protocol::AcademicStage::Graduate),
            Some("0123456789abcdef0123456789abcdef"),
            true,
        )
        .unwrap();
        assert!(metadata.is_current(Utc::now()));
        assert_eq!(
            metadata.device_fingerprint(),
            Some("0123456789abcdef0123456789abcdef")
        );
        let encoded = serde_json::to_string(&metadata).unwrap();
        for forbidden in ["password", "cookie", "ticket", "csrf", "response"] {
            assert!(!encoded.to_ascii_lowercase().contains(forbidden));
        }
        let debug = format!("{metadata:?}");
        assert!(!debug.contains("fixture-user"));
    }

    #[test]
    fn backend_repair_resume_snapshot_rejects_expired_or_far_future_state() {
        let mut snapshot = ResumeSnapshot::new("fixture-user", b"[]").unwrap();
        snapshot.saved_at = Utc::now() - Duration::days(31);
        assert!(!snapshot.is_current(Utc::now()));
        snapshot.saved_at = Utc::now() + Duration::hours(1);
        assert!(!snapshot.is_current(Utc::now()));
    }

    #[test]
    fn backend_repair_account_metadata_keeps_saved_credentials_until_revocation() {
        let metadata = ResumeAccountMetadata::new("fixture-user", None, None, true).unwrap();
        assert!(metadata.is_current(Utc::now()));

        let mut durable = metadata.clone();
        durable.last_successful_login = Utc::now() - Duration::days(366);
        assert!(durable.is_current(Utc::now()));

        let mut bounded = metadata.clone().with_credentials_saved(false);
        bounded.last_successful_login = Utc::now() - Duration::days(366);
        assert!(!bounded.is_current(Utc::now()));

        let mut future = metadata;
        future.last_successful_login = Utc::now() + Duration::hours(1);
        assert!(!future.is_current(Utc::now()));
    }

    #[test]
    fn backend_repair_account_metadata_round_trips_manual_stage_provenance() {
        let metadata = ResumeAccountMetadata::new_with_stage_selection(
            "fixture-user",
            Some(crate::protocol::AcademicStage::Undergraduate),
            Some("0123456789abcdef0123456789abcdef"),
            true,
            true,
        )
        .unwrap();
        let encoded = serde_json::to_vec(&metadata).unwrap();
        let restored: ResumeAccountMetadata = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored.academic_stage, metadata.academic_stage);
        assert!(restored.stage_selection_explicit);
        assert!(restored.credentials_saved);
    }

    #[test]
    fn backend_repair_corrupt_account_metadata_is_preserved_and_reported() {
        let root = test_dir("account-metadata-invalid");
        ensure_private_directory(&root).unwrap();
        let path = root.join(ACCOUNT_METADATA_FILE);
        let bytes = b"not metadata JSON";
        fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert!(load_account_metadata_at(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_file_session_store_round_trips_without_os_secret_service() {
        let root = test_dir("session-file-round-trip");
        let path = root.join(SESSION_FILE);
        let snapshot = ResumeSnapshot::new("fixture-user", b"[]").unwrap();
        save_at(&path, &snapshot).unwrap();
        assert_eq!(load_at(&path).unwrap(), Some(snapshot));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        clear_at(&path).unwrap();
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_session_snapshot_is_readable_json_without_a_key_file() {
        let root = test_dir("session-json");
        let path = root.join(SESSION_FILE);
        let snapshot = ResumeSnapshot::new(
            "fixture-user",
            br#"{"cookie":"cookie-secret-marker","ticket":"ticket-marker"}"#,
        )
        .unwrap()
        .with_device_fingerprint("0123456789abcdef0123456789abcdef")
        .unwrap();

        save_at(&path, &snapshot).unwrap();
        let raw = fs::read(&path).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(json["username"], "fixture-user");
        assert!(
            json["cookie_store_json"]
                .as_str()
                .unwrap()
                .contains("cookie-secret-marker")
        );
        assert!(!root.join("campus-session-key-v1.bin").exists());
        assert_eq!(load_at(&path).unwrap(), Some(snapshot));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_legacy_session_files_are_preserved_and_reported() {
        let root = test_dir("session-legacy-files");
        let encrypted_legacy = root.join(LEGACY_SESSION_FILES[0]);
        let legacy_bytes = b"legacy encrypted snapshot fixture";
        ensure_private_directory(&root).unwrap();
        fs::write(&encrypted_legacy, legacy_bytes).unwrap();

        let error = load_from_root(&root).expect_err("legacy encrypted file is unsupported");
        assert!(error.contains("legacy session storage exists"));
        assert_eq!(fs::read(&encrypted_legacy).unwrap(), legacy_bytes);
        assert!(!root.join(SESSION_FILE).exists());

        let plaintext_legacy = root.join(LEGACY_SESSION_FILES[1]);
        let legacy_json = br#"{"schema":1,"cookie_store":"old-format"}"#;
        fs::write(&plaintext_legacy, legacy_json).unwrap();
        assert!(load_from_root(&root).is_err());
        assert_eq!(fs::read(&plaintext_legacy).unwrap(), legacy_json);

        clear_session_at_root(&root).unwrap();
        assert!(!encrypted_legacy.exists());
        assert!(!plaintext_legacy.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_file_session_store_preserves_unreadable_and_discards_expired_files() {
        let root = test_dir("session-file-invalid");
        let path = root.join(SESSION_FILE);
        ensure_private_directory(&root).unwrap();
        let invalid_json = b"not a session snapshot";
        fs::write(&path, invalid_json).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(load_at(&path).is_err());
        assert_eq!(
            fs::read(&path).unwrap(),
            invalid_json,
            "invalid JSON is preserved"
        );

        let mut expired = ResumeSnapshot::new("fixture-user", b"[]").unwrap();
        expired.saved_at = Utc::now() - Duration::days(31);
        save_at(&path, &expired).unwrap();
        assert_eq!(load_at(&path).unwrap(), None);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_account_metadata_survives_cookie_expiry_until_logout() {
        let root = test_dir("account-metadata-expiry");
        let session_path = root.join(SESSION_FILE);
        let metadata_path = root.join(ACCOUNT_METADATA_FILE);
        let snapshot = ResumeSnapshot::new("fixture-user", b"[]").unwrap();
        save_at(&session_path, &snapshot).unwrap();
        let metadata = ResumeAccountMetadata::new(
            "fixture-user",
            Some(crate::protocol::AcademicStage::Graduate),
            None,
            true,
        )
        .unwrap();
        save_account_metadata_at_root(&root, &metadata).unwrap();
        let mut expired = snapshot;
        expired.saved_at = Utc::now() - Duration::days(31);
        save_at(&session_path, &expired).unwrap();

        assert_eq!(load_at(&session_path).unwrap(), None);
        assert!(!session_path.exists());
        assert_eq!(
            load_account_metadata_at(&metadata_path).unwrap(),
            Some(metadata)
        );
        assert!(metadata_path.exists());

        clear_at(&metadata_path).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_resume_state_clears_snapshot_when_metadata_commit_fails() {
        let root = test_dir("resume-state-transaction");
        let device = "0123456789abcdef0123456789abcdef";
        let snapshot = ResumeSnapshot::new("fixture-user", b"[]")
            .unwrap()
            .with_device_fingerprint(device)
            .unwrap();
        let metadata = ResumeAccountMetadata::new(
            "fixture-user",
            Some(crate::protocol::AcademicStage::Graduate),
            Some(device),
            true,
        )
        .unwrap();
        ensure_private_directory(&root).unwrap();
        fs::create_dir(root.join(ACCOUNT_METADATA_FILE)).unwrap();

        assert!(save_resume_state_at_root(&root, &snapshot, &metadata).is_err());
        assert!(!root.join(SESSION_FILE).exists());
        assert!(root.join(ACCOUNT_METADATA_FILE).is_dir());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_repair_resume_state_commits_snapshot_and_metadata_together() {
        let root = test_dir("resume-state-success");
        let device = "0123456789abcdef0123456789abcdef";
        let snapshot = ResumeSnapshot::new("fixture-user", b"[]")
            .unwrap()
            .with_device_fingerprint(device)
            .unwrap();
        let metadata = ResumeAccountMetadata::new(
            "fixture-user",
            Some(crate::protocol::AcademicStage::Graduate),
            Some(device),
            true,
        )
        .unwrap();

        save_resume_state_at_root(&root, &snapshot, &metadata).unwrap();
        assert_eq!(load_at_root(&root).unwrap(), Some(snapshot));
        assert_eq!(
            load_account_metadata_at_root(&root).unwrap(),
            Some(metadata)
        );
        let _ = fs::remove_dir_all(root);
    }
}
