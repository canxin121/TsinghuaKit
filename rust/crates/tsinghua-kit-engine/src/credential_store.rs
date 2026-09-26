//! Rust-owned opt-in credential storage for cross-process session recovery.
//!
//! Credentials are kept in readable JSON files under the host-selected app
//! data directory. This module never calls an operating-system credential API
//! and never exposes the password to Flutter, logs, command-line arguments, or
//! session snapshots. On Unix, Rust creates directories with owner-only
//! permissions; the files are not encrypted.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use thiserror::Error;

use crate::protocol::AcademicStage;

const VAULT_DIRECTORY: &str = "credentials";
const VAULT_LOCK_FILE: &str = "vault.lock";
const RECORD_FILE_PREFIX: &str = "credential-v2-";
const RECORD_FILE_SUFFIX: &str = ".json";
const LEGACY_RECORD_FILE_PREFIX: &str = "credential-v1-";
const LEGACY_RECORD_FILE_SUFFIX: &str = ".bin";
const RECORD_SCHEMA: u32 = 2;
const MAX_USERNAME_BYTES: usize = 128;
const MAX_PASSWORD_BYTES: usize = 4096;
const MAX_DEVICE_FINGERPRINT_BYTES: usize = 32;
const MAX_RECORD_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum CredentialStoreError {
    #[error("credential storage rejected the account key")]
    InvalidAccount,
    #[error("credential storage rejected the device binding")]
    InvalidDevice,
    #[error("credential storage operation failed")]
    Backend,
    #[error("credential storage record is invalid")]
    InvalidRecord,
    #[error("credential storage record belongs to another device")]
    BindingMismatch,
    #[error("credential record uses an unsupported encrypted file format")]
    LegacyEncryptedRecord,
}

/// A credential loaded into Rust memory for one bounded recovery attempt.
/// Its manual `Debug` implementation redacts the password so diagnostics can
/// safely describe the in-memory record without exposing the secret.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredCredential {
    pub(crate) password: String,
    pub(crate) stage: Option<AcademicStage>,
    /// Whether `stage` came from the user's explicit LoginPage override.
    /// Older records omit this field and deserialize as `false`.
    pub(crate) stage_selection_explicit: bool,
}

impl std::fmt::Debug for StoredCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredCredential")
            .field("password", &"[redacted]")
            .field("stage", &self.stage)
            .field("stage_selection_explicit", &self.stage_selection_explicit)
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialPayload {
    schema: u32,
    account: String,
    device_fingerprint: String,
    password: String,
    stage: Option<AcademicStage>,
    #[serde(default)]
    stage_selection_explicit: bool,
}

fn validate_account(account: &str) -> Result<&str, CredentialStoreError> {
    let account = account.trim();
    if account.is_empty()
        || account.len() > MAX_USERNAME_BYTES
        || account.chars().any(char::is_control)
    {
        return Err(CredentialStoreError::InvalidAccount);
    }
    Ok(account)
}

fn validate_device(device_fingerprint: &str) -> Result<&str, CredentialStoreError> {
    if device_fingerprint.len() != MAX_DEVICE_FINGERPRINT_BYTES
        || !device_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CredentialStoreError::InvalidDevice);
    }
    Ok(device_fingerprint)
}

fn validate_password(password: &str) -> Result<(), CredentialStoreError> {
    if password.is_empty()
        || password.len() > MAX_PASSWORD_BYTES
        || password.chars().any(char::is_control)
    {
        return Err(CredentialStoreError::InvalidRecord);
    }
    Ok(())
}

fn vault_directory(root: &Path) -> PathBuf {
    root.join(VAULT_DIRECTORY)
}

fn vault_lock_path(vault: &Path) -> PathBuf {
    vault.join(VAULT_LOCK_FILE)
}

fn record_path(vault: &Path, account: &str) -> PathBuf {
    vault.join(format!(
        "{RECORD_FILE_PREFIX}{}{RECORD_FILE_SUFFIX}",
        account_file_id(account)
    ))
}

fn legacy_record_path(vault: &Path, account: &str) -> PathBuf {
    vault.join(format!(
        "{LEGACY_RECORD_FILE_PREFIX}{}{LEGACY_RECORD_FILE_SUFFIX}",
        account_file_id(account)
    ))
}

fn account_file_id(account: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ring::digest::{SHA256, digest};
    URL_SAFE_NO_PAD.encode(digest(&SHA256, account.as_bytes()).as_ref())
}

fn encode_record(
    account: &str,
    device_fingerprint: &str,
    password: &str,
    stage: Option<AcademicStage>,
    stage_selection_explicit: bool,
) -> Result<Vec<u8>, CredentialStoreError> {
    validate_password(password)?;
    if stage_selection_explicit && stage.is_none() {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let payload = serde_json::to_vec_pretty(&CredentialPayload {
        schema: RECORD_SCHEMA,
        account: account.to_owned(),
        device_fingerprint: device_fingerprint.to_owned(),
        password: password.to_owned(),
        stage,
        stage_selection_explicit,
    })
    .map_err(|_| CredentialStoreError::InvalidRecord)?;
    if payload.is_empty() || payload.len() > MAX_RECORD_BYTES {
        return Err(CredentialStoreError::InvalidRecord);
    }

    Ok(payload)
}

fn decode_record(
    bytes: &[u8],
    account: &str,
    device_fingerprint: &str,
) -> Result<StoredCredential, CredentialStoreError> {
    if bytes.is_empty() || bytes.len() > MAX_RECORD_BYTES {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let payload: CredentialPayload =
        serde_json::from_slice(bytes).map_err(|_| CredentialStoreError::InvalidRecord)?;
    if payload.schema != RECORD_SCHEMA || payload.account != account {
        return Err(CredentialStoreError::InvalidRecord);
    }
    if payload.device_fingerprint != device_fingerprint {
        return Err(CredentialStoreError::BindingMismatch);
    }
    validate_password(&payload.password)?;
    Ok(StoredCredential {
        password: payload.password,
        stage: payload.stage,
        stage_selection_explicit: payload.stage_selection_explicit,
    })
}

#[cfg(unix)]
fn private_directory_permissions(path: &Path) -> Result<(), CredentialStoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)
        .map_err(|_| CredentialStoreError::Backend)?
        .permissions()
        .mode();
    if mode & 0o777 != 0o700 {
        return Err(CredentialStoreError::Backend);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_directory_permissions(_path: &Path) -> Result<(), CredentialStoreError> {
    Ok(())
}

#[cfg(unix)]
fn private_file_permissions(path: &Path) -> Result<(), CredentialStoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)
        .map_err(|_| CredentialStoreError::Backend)?
        .permissions()
        .mode();
    if mode & 0o777 != 0o600 {
        return Err(CredentialStoreError::Backend);
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_file_permissions(_path: &Path) -> Result<(), CredentialStoreError> {
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), CredentialStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CredentialStoreError::Backend);
            }
            private_directory_permissions(path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| CredentialStoreError::Backend)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| CredentialStoreError::Backend)?;
            }
            private_directory_permissions(path)
        }
        Err(_) => Err(CredentialStoreError::Backend),
    }
}

fn ensure_vault_directory(root: &Path) -> Result<PathBuf, CredentialStoreError> {
    create_private_directory(root)?;
    let vault = vault_directory(root);
    create_private_directory(&vault)?;
    Ok(vault)
}

fn existing_vault_directory(root: &Path) -> Result<Option<PathBuf>, CredentialStoreError> {
    let root_metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CredentialStoreError::Backend),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(CredentialStoreError::Backend);
    }
    private_directory_permissions(root)?;
    let vault = vault_directory(root);
    match fs::symlink_metadata(&vault) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CredentialStoreError::Backend);
            }
            private_directory_permissions(&vault)?;
            Ok(Some(vault))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(CredentialStoreError::Backend),
    }
}

fn reject_non_regular_file(path: &Path) -> Result<bool, CredentialStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CredentialStoreError::InvalidRecord);
            }
            private_file_permissions(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(CredentialStoreError::Backend),
    }
}

fn private_writer(path: &Path) -> Result<File, CredentialStoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| CredentialStoreError::Backend)
}

fn temporary_path(destination: &Path) -> Result<PathBuf, CredentialStoreError> {
    let parent = destination.parent().ok_or(CredentialStoreError::Backend)?;
    let name = destination
        .file_name()
        .ok_or(CredentialStoreError::Backend)?
        .to_string_lossy();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| CredentialStoreError::Backend)?
        .as_nanos();
    Ok(parent.join(format!(".{name}.tmp-{}-{timestamp}", std::process::id())))
}

fn write_atomically(destination: &Path, payload: &[u8]) -> Result<(), CredentialStoreError> {
    if payload.is_empty() || payload.len() > MAX_RECORD_BYTES {
        return Err(CredentialStoreError::InvalidRecord);
    }
    if let Ok(true) = reject_non_regular_file(destination) {
        // The destination will be permission-checked again immediately before
        // the commit. This check only rejects a pre-existing special path.
    } else if destination.exists() {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let temporary = temporary_path(destination)?;
    let mut file = private_writer(&temporary)?;
    let result = file.write_all(payload).and_then(|_| file.sync_all());
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(CredentialStoreError::Backend);
    }
    drop(file);
    private_file_permissions(&temporary)?;

    // A destination symlink is never replaced. A regular destination is
    // replaced atomically on Unix; Windows needs the existing file removed
    // first because rename does not replace it there.
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            let _ = fs::remove_file(&temporary);
            return Err(CredentialStoreError::InvalidRecord);
        }
        Ok(_) => {
            private_file_permissions(destination)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            return Err(CredentialStoreError::Backend);
        }
    }
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination).map_err(|_| CredentialStoreError::Backend)?;
    }
    if fs::rename(&temporary, destination).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(CredentialStoreError::Backend);
    }
    private_file_permissions(destination)
}

fn remove_record(path: &Path) -> Result<(), CredentialStoreError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CredentialStoreError::Backend),
    }
}

fn open_vault_lock(vault: &Path) -> Result<File, CredentialStoreError> {
    let path = vault_lock_path(vault);
    let _ = reject_non_regular_file(&path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|_| CredentialStoreError::Backend)?;
    private_file_permissions(&path)?;
    file.try_lock_exclusive()
        .map_err(|_| CredentialStoreError::Backend)?;
    Ok(file)
}

fn with_locked_vault<T>(
    vault: &Path,
    operation: impl FnOnce(&Path) -> Result<T, CredentialStoreError>,
) -> Result<T, CredentialStoreError> {
    let lock = open_vault_lock(vault)?;
    let result = operation(vault);
    let unlock_result = lock.unlock();
    if result.is_ok() && unlock_result.is_err() {
        return Err(CredentialStoreError::Backend);
    }
    result
}

fn save_at(
    root: &Path,
    account: &str,
    password: &str,
    stage: Option<AcademicStage>,
    device_fingerprint: &str,
) -> Result<(), CredentialStoreError> {
    save_at_with_stage_selection(root, account, password, stage, device_fingerprint, false)
}

fn save_at_with_stage_selection(
    root: &Path,
    account: &str,
    password: &str,
    stage: Option<AcademicStage>,
    device_fingerprint: &str,
    stage_selection_explicit: bool,
) -> Result<(), CredentialStoreError> {
    let account = validate_account(account)?;
    let device_fingerprint = validate_device(device_fingerprint)?;
    validate_password(password)?;
    let vault = ensure_vault_directory(root)?;
    with_locked_vault(&vault, |vault| {
        if reject_non_regular_file(&legacy_record_path(vault, account))? {
            return Err(CredentialStoreError::LegacyEncryptedRecord);
        }
        let record = encode_record(
            account,
            device_fingerprint,
            password,
            stage,
            stage_selection_explicit,
        )?;
        write_atomically(&record_path(vault, account), &record)
    })
}

fn load_at(
    root: &Path,
    account: &str,
    device_fingerprint: &str,
) -> Result<Option<StoredCredential>, CredentialStoreError> {
    let account = validate_account(account)?;
    let device_fingerprint = validate_device(device_fingerprint)?;
    let Some(vault) = existing_vault_directory(root)? else {
        return Ok(None);
    };
    with_locked_vault(&vault, |vault| {
        if reject_non_regular_file(&legacy_record_path(vault, account))? {
            return Err(CredentialStoreError::LegacyEncryptedRecord);
        }
        let path = record_path(vault, account);
        if !reject_non_regular_file(&path)? {
            return Ok(None);
        }
        let metadata = fs::metadata(&path).map_err(|_| CredentialStoreError::Backend)?;
        if metadata.len() > MAX_RECORD_BYTES as u64 {
            return Err(CredentialStoreError::InvalidRecord);
        }
        let bytes = fs::read(&path).map_err(|_| CredentialStoreError::Backend)?;
        decode_record(&bytes, account, device_fingerprint).map(Some)
    })
}

fn clear_at(root: &Path, account: &str) -> Result<(), CredentialStoreError> {
    let account = validate_account(account)?;
    let Some(vault) = existing_vault_directory(root)? else {
        return Ok(());
    };
    with_locked_vault(&vault, |vault| {
        remove_record(&record_path(vault, account))?;
        remove_record(&legacy_record_path(vault, account))
    })
}

pub(crate) fn save(
    account: &str,
    password: &str,
    stage: Option<AcademicStage>,
    device_fingerprint: &str,
) -> Result<(), CredentialStoreError> {
    save_with_stage_selection(account, password, stage, device_fingerprint, false)
}

pub(crate) fn save_with_stage_selection(
    account: &str,
    password: &str,
    stage: Option<AcademicStage>,
    device_fingerprint: &str,
    stage_selection_explicit: bool,
) -> Result<(), CredentialStoreError> {
    let root = crate::session_persistence::application_data_dir()
        .map_err(|_| CredentialStoreError::Backend)?;
    save_at_root_with_stage_selection(
        &root,
        account,
        password,
        stage,
        device_fingerprint,
        stage_selection_explicit,
    )
}

pub(crate) fn save_at_root_with_stage_selection(
    root: &Path,
    account: &str,
    password: &str,
    stage: Option<AcademicStage>,
    device_fingerprint: &str,
    stage_selection_explicit: bool,
) -> Result<(), CredentialStoreError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::CredentialStore, || {
        save_at_with_stage_selection(
            root,
            account,
            password,
            stage,
            device_fingerprint,
            stage_selection_explicit,
        )
    })
}

pub(crate) fn load(
    account: &str,
    device_fingerprint: &str,
) -> Result<Option<StoredCredential>, CredentialStoreError> {
    let root = crate::session_persistence::application_data_dir()
        .map_err(|_| CredentialStoreError::Backend)?;
    load_at_root(&root, account, device_fingerprint)
}

pub(crate) fn load_at_root(
    root: &Path,
    account: &str,
    device_fingerprint: &str,
) -> Result<Option<StoredCredential>, CredentialStoreError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::CredentialStore, || {
        load_at(root, account, device_fingerprint)
    })
}

pub(crate) fn clear(account: &str) -> Result<(), CredentialStoreError> {
    let root = crate::session_persistence::application_data_dir()
        .map_err(|_| CredentialStoreError::Backend)?;
    clear_at_root(&root, account)
}

pub(crate) fn clear_at_root(root: &Path, account: &str) -> Result<(), CredentialStoreError> {
    clear_at(root, account)
}

/// Stores an independent SelfService password below its own namespace. A
/// matching username in Identity can never address this record.
pub(crate) fn save_self_service_at_root(
    root: &Path,
    account: &str,
    password: &str,
    device_fingerprint: &str,
) -> Result<(), CredentialStoreError> {
    let domain_root = root.join("self-service");
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::CredentialStore, || {
        save_at_with_stage_selection(
            &domain_root,
            account,
            password,
            None,
            device_fingerprint,
            false,
        )
    })
}

pub(crate) fn load_self_service_at_root(
    root: &Path,
    account: &str,
    device_fingerprint: &str,
) -> Result<Option<StoredCredential>, CredentialStoreError> {
    let domain_root = root.join("self-service");
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::CredentialStore, || {
        load_at(&domain_root, account, device_fingerprint)
    })
}

pub(crate) fn clear_self_service_at_root(
    root: &Path,
    account: &str,
) -> Result<(), CredentialStoreError> {
    let domain_root = root.join("self-service");
    clear_at(&domain_root, account)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture_root() -> PathBuf {
        let serial = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "thyou-private-credential-fixture-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture root creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("fixture root is private");
        }
        root
    }

    fn fixture_device() -> &'static str {
        "0123456789abcdef0123456789abcdef"
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn credential_file_is_readable_json_and_debug_still_redacts_password() {
        let root = fixture_root();
        let password = "fixture-password";
        save_at(
            &root,
            "2026000000",
            password,
            Some(AcademicStage::Graduate),
            fixture_device(),
        )
        .expect("private credential saves");
        let vault = vault_directory(&root);
        let bytes = fs::read(record_path(&vault, "2026000000")).expect("JSON record reads");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("record is JSON");
        assert_eq!(json["password"], password);
        assert!(!vault.join("vault-key-v1.bin").exists());
        let decoded = load_at(&root, "2026000000", fixture_device())
            .expect("private credential loads")
            .expect("record exists");
        assert_eq!(decoded.password, password);
        assert_eq!(decoded.stage, Some(AcademicStage::Graduate));
        assert!(!decoded.stage_selection_explicit);
        let debug = format!("{decoded:?}");
        assert!(!debug.contains(password));
        cleanup(&root);
    }

    #[test]
    fn backend_repair_private_credential_file_round_trips_manual_stage_selection() {
        let root = fixture_root();
        save_at_with_stage_selection(
            &root,
            "2026000000",
            "fixture-password",
            Some(AcademicStage::Undergraduate),
            fixture_device(),
            true,
        )
        .expect("private credential saves");
        let decoded = load_at(&root, "2026000000", fixture_device())
            .expect("private credential loads")
            .expect("record exists");
        assert_eq!(decoded.stage, Some(AcademicStage::Undergraduate));
        assert!(decoded.stage_selection_explicit);
        cleanup(&root);
    }

    #[test]
    fn backend_repair_private_credential_file_rejects_corrupt_record() {
        let root = fixture_root();
        save_at(&root, "student", "fixture-password", None, fixture_device())
            .expect("private credential saves");
        let path = record_path(&vault_directory(&root), "student");
        fs::write(&path, b"not-a-credential-record").expect("corrupt fixture writes");
        let corrupt_bytes = fs::read(&path).expect("corrupt fixture remains readable");
        assert_eq!(
            load_at(&root, "student", fixture_device()),
            Err(CredentialStoreError::InvalidRecord)
        );
        assert_eq!(fs::read(&path).unwrap(), corrupt_bytes);
        cleanup(&root);
    }

    #[test]
    fn credential_store_reports_legacy_encrypted_record_without_removing_it() {
        let root = fixture_root();
        let vault = vault_directory(&root);
        create_private_directory(&vault).expect("credential directory creates");
        let record = legacy_record_path(&vault, "student");
        write_atomically(&record, b"legacy encrypted envelope").expect("legacy fixture writes");
        let saved_record = fs::read(&record).expect("legacy record reads");

        assert_eq!(
            load_at(&root, "student", fixture_device()),
            Err(CredentialStoreError::LegacyEncryptedRecord)
        );
        assert_eq!(
            save_at(
                &root,
                "student",
                "replacement-password",
                None,
                fixture_device()
            ),
            Err(CredentialStoreError::LegacyEncryptedRecord)
        );
        assert_eq!(fs::read(&record).unwrap(), saved_record);
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn backend_repair_private_credential_file_rejects_symlink_or_unsafe_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = fixture_root();
        save_at(&root, "student", "fixture-password", None, fixture_device())
            .expect("private credential saves");
        let path = record_path(&vault_directory(&root), "student");
        let target = root.join("outside-record");
        fs::rename(&path, &target).expect("record stages");
        symlink(&target, &path).expect("record symlink creates");
        assert!(load_at(&root, "student", fixture_device()).is_err());
        assert!(target.exists());
        fs::remove_file(&path).expect("record symlink removes");
        fs::rename(&target, &path).expect("record restores");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("unsafe permission fixture writes");
        assert!(load_at(&root, "student", fixture_device()).is_err());
        cleanup(&root);
    }

    #[test]
    fn backend_repair_private_credential_file_clear_removes_account_record() {
        let root = fixture_root();
        save_at(&root, "student", "fixture-password", None, fixture_device())
            .expect("private credential saves");
        clear_at(&root, "student").expect("private credential clears");
        assert_eq!(
            load_at(&root, "student", fixture_device()).expect("cleared record loads"),
            None
        );
        cleanup(&root);
    }

    #[test]
    fn backend_repair_private_credential_file_account_and_device_binding_is_strict() {
        let root = fixture_root();
        save_at(&root, "student", "fixture-password", None, fixture_device())
            .expect("private credential saves");
        let other_device = "fedcba9876543210fedcba9876543210";
        assert_eq!(
            load_at(&root, "student", other_device),
            Err(CredentialStoreError::BindingMismatch)
        );
        assert_eq!(
            load_at(&root, "another-student", fixture_device()).expect("other account loads"),
            None
        );
        assert!(
            load_at(&root, "student", fixture_device())
                .expect("bound record loads")
                .is_some()
        );
        cleanup(&root);
    }

    #[test]
    fn backend_auth_credential_domains_do_not_share_same_named_account_records() {
        let root = fixture_root();
        save_at(
            &root,
            "shared-name",
            "identity-password",
            Some(AcademicStage::Graduate),
            fixture_device(),
        )
        .expect("Identity record saves");
        save_self_service_at_root(
            &root,
            "shared-name",
            "self-service-password",
            fixture_device(),
        )
        .expect("SelfService record saves");

        let identity = load_at(&root, "shared-name", fixture_device())
            .expect("Identity record loads")
            .expect("Identity record exists");
        let self_service = load_self_service_at_root(&root, "shared-name", fixture_device())
            .expect("SelfService record loads")
            .expect("SelfService record exists");
        assert_eq!(identity.password, "identity-password");
        assert_eq!(identity.stage, Some(AcademicStage::Graduate));
        assert_eq!(self_service.password, "self-service-password");
        assert_eq!(self_service.stage, None);

        clear_self_service_at_root(&root, "shared-name")
            .expect("SelfService record can be forgotten independently");
        assert!(
            load_self_service_at_root(&root, "shared-name", fixture_device())
                .expect("forgotten SelfService record loads")
                .is_none()
        );
        assert_eq!(
            load_at(&root, "shared-name", fixture_device())
                .expect("Identity record remains")
                .expect("Identity record still exists")
                .password,
            "identity-password"
        );
        cleanup(&root);
    }
}
