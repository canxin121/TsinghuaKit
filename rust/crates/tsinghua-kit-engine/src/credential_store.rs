//! Rust-owned opt-in credential storage for cross-process session recovery.
//!
//! Credentials are kept in a THYou-private file vault. This module never
//! calls an operating-system credential API and never exposes the password to
//! Flutter, JSON caches, logs, command-line arguments, or session snapshots.
//! The vault is an application-managed encrypted file store: it protects
//! against ordinary accidental file reads, while its security boundary is the
//! owner-only THYou application directory. It is intentionally not presented
//! as equivalent to an OS-managed credential service against a local
//! administrator or a process that can inspect this application's files or
//! memory.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL},
};
use fs2::FileExt;
use ring::{
    aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    digest::{SHA256, digest},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use thiserror::Error;

use crate::protocol::AcademicStage;

const VAULT_DIRECTORY: &str = "credentials";
const VAULT_KEY_FILE: &str = "vault-key-v1.bin";
const VAULT_LOCK_FILE: &str = "vault.lock";
const RECORD_FILE_PREFIX: &str = "credential-v1-";
const RECORD_FILE_SUFFIX: &str = ".bin";
const RECORD_SCHEMA: u32 = 1;
const VAULT_KEY_BYTES: usize = 32;
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
}

/// A credential loaded into Rust memory for one bounded recovery attempt.
/// Its manual `Debug` implementation redacts the password so diagnostics can
/// safely describe the in-memory record without exposing the secret.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StoredCredential {
    pub(crate) password: String,
    pub(crate) stage: Option<AcademicStage>,
    /// Whether `stage` came from the user's explicit LoginPage override.
    /// Older encrypted records omit this field and deserialize as `false`.
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialEnvelope {
    schema: u32,
    account_binding: String,
    device_binding: String,
    nonce: String,
    ciphertext: String,
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

fn vault_key_path(vault: &Path) -> PathBuf {
    vault.join(VAULT_KEY_FILE)
}

fn vault_lock_path(vault: &Path) -> PathBuf {
    vault.join(VAULT_LOCK_FILE)
}

fn account_binding(account: &str) -> String {
    BASE64.encode(digest(&SHA256, account.as_bytes()).as_ref())
}

fn record_path(vault: &Path, account: &str) -> PathBuf {
    vault.join(format!(
        "{RECORD_FILE_PREFIX}{}{RECORD_FILE_SUFFIX}",
        BASE64_URL.encode(digest(&SHA256, account.as_bytes()).as_ref())
    ))
}

fn device_binding(device_fingerprint: &str) -> String {
    BASE64.encode(digest(&SHA256, device_fingerprint.as_bytes()).as_ref())
}

fn associated_data(account: &str, device_fingerprint: &str) -> Vec<u8> {
    let mut data = b"THYou credential vault v1\0".to_vec();
    data.extend_from_slice(account.as_bytes());
    data.push(0);
    data.extend_from_slice(device_fingerprint.as_bytes());
    data
}

fn key_from_bytes(bytes: &[u8]) -> Result<LessSafeKey, CredentialStoreError> {
    if bytes.len() != VAULT_KEY_BYTES {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let key = UnboundKey::new(&CHACHA20_POLY1305, bytes)
        .map_err(|_| CredentialStoreError::InvalidRecord)?;
    Ok(LessSafeKey::new(key))
}

fn encode_envelope(
    account: &str,
    device_fingerprint: &str,
    password: &str,
    stage: Option<AcademicStage>,
    stage_selection_explicit: bool,
    key_bytes: &[u8],
) -> Result<Vec<u8>, CredentialStoreError> {
    validate_password(password)?;
    if stage_selection_explicit && stage.is_none() {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let payload = serde_json::to_vec(&CredentialPayload {
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

    let mut nonce_bytes = [0_u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| CredentialStoreError::Backend)?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let cipher = key_from_bytes(key_bytes)?;
    let mut ciphertext = payload;
    cipher
        .seal_in_place_append_tag(
            nonce,
            Aad::from(associated_data(account, device_fingerprint)),
            &mut ciphertext,
        )
        .map_err(|_| CredentialStoreError::Backend)?;

    serde_json::to_vec(&CredentialEnvelope {
        schema: RECORD_SCHEMA,
        account_binding: account_binding(account),
        device_binding: device_binding(device_fingerprint),
        nonce: BASE64.encode(nonce_bytes),
        ciphertext: BASE64.encode(ciphertext),
    })
    .map_err(|_| CredentialStoreError::InvalidRecord)
}

fn decode_envelope(
    bytes: &[u8],
    account: &str,
    device_fingerprint: &str,
    key_bytes: &[u8],
) -> Result<StoredCredential, CredentialStoreError> {
    if bytes.is_empty() || bytes.len() > MAX_RECORD_BYTES {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let envelope: CredentialEnvelope =
        serde_json::from_slice(bytes).map_err(|_| CredentialStoreError::InvalidRecord)?;
    if envelope.schema != RECORD_SCHEMA {
        return Err(CredentialStoreError::InvalidRecord);
    }
    if envelope.account_binding != account_binding(account) {
        return Err(CredentialStoreError::InvalidRecord);
    }
    if envelope.device_binding != device_binding(device_fingerprint) {
        return Err(CredentialStoreError::BindingMismatch);
    }
    let nonce_bytes = BASE64
        .decode(envelope.nonce.as_bytes())
        .map_err(|_| CredentialStoreError::InvalidRecord)?;
    let nonce_bytes: [u8; NONCE_LEN] = nonce_bytes
        .try_into()
        .map_err(|_| CredentialStoreError::InvalidRecord)?;
    let mut ciphertext = BASE64
        .decode(envelope.ciphertext.as_bytes())
        .map_err(|_| CredentialStoreError::InvalidRecord)?;
    if ciphertext.len() < 16 || ciphertext.len() > MAX_RECORD_BYTES {
        return Err(CredentialStoreError::InvalidRecord);
    }

    let cipher = key_from_bytes(key_bytes)?;
    let plaintext = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(associated_data(account, device_fingerprint)),
            &mut ciphertext,
        )
        .map_err(|_| CredentialStoreError::InvalidRecord)?;
    let payload: CredentialPayload =
        serde_json::from_slice(plaintext).map_err(|_| CredentialStoreError::InvalidRecord)?;
    if payload.schema != RECORD_SCHEMA
        || payload.account != account
        || payload.device_fingerprint != device_fingerprint
    {
        return Err(CredentialStoreError::InvalidRecord);
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

fn read_vault_key(vault: &Path) -> Result<Option<[u8; VAULT_KEY_BYTES]>, CredentialStoreError> {
    let path = vault_key_path(vault);
    if !reject_non_regular_file(&path)? {
        return Ok(None);
    }
    let metadata = fs::metadata(&path).map_err(|_| CredentialStoreError::Backend)?;
    if metadata.len() != VAULT_KEY_BYTES as u64 {
        return Err(CredentialStoreError::InvalidRecord);
    }
    let bytes = fs::read(&path).map_err(|_| CredentialStoreError::Backend)?;
    let key = bytes
        .try_into()
        .map_err(|_| CredentialStoreError::InvalidRecord)?;
    Ok(Some(key))
}

fn create_vault_key(vault: &Path) -> Result<[u8; VAULT_KEY_BYTES], CredentialStoreError> {
    let mut key = [0_u8; VAULT_KEY_BYTES];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| CredentialStoreError::Backend)?;
    write_atomically(&vault_key_path(vault), &key)?;
    Ok(key)
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
        let key = match read_vault_key(vault)? {
            Some(key) => key,
            None => create_vault_key(vault)?,
        };
        let envelope = encode_envelope(
            account,
            device_fingerprint,
            password,
            stage,
            stage_selection_explicit,
            &key,
        )?;
        write_atomically(&record_path(vault, account), &envelope)
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
        let path = record_path(vault, account);
        if !reject_non_regular_file(&path)? {
            return Ok(None);
        }
        let metadata = fs::metadata(&path).map_err(|_| CredentialStoreError::Backend)?;
        if metadata.len() > MAX_RECORD_BYTES as u64 {
            let _ = remove_record(&path);
            return Err(CredentialStoreError::InvalidRecord);
        }
        let bytes = fs::read(&path).map_err(|_| CredentialStoreError::Backend)?;
        let envelope: CredentialEnvelope = match serde_json::from_slice(&bytes) {
            Ok(envelope) => envelope,
            Err(_) => {
                let _ = remove_record(&path);
                return Err(CredentialStoreError::InvalidRecord);
            }
        };
        if envelope.schema != RECORD_SCHEMA || envelope.account_binding != account_binding(account)
        {
            let _ = remove_record(&path);
            return Err(CredentialStoreError::InvalidRecord);
        }
        // A record from the same account but another device is not corruption;
        // keep it intact so a copied application directory cannot destroy the
        // original device's opt-in record merely by probing it.
        if envelope.device_binding != device_binding(device_fingerprint) {
            return Err(CredentialStoreError::BindingMismatch);
        }
        let Some(key) = read_vault_key(vault)? else {
            let _ = remove_record(&path);
            return Err(CredentialStoreError::InvalidRecord);
        };
        match decode_envelope(&bytes, account, device_fingerprint, &key) {
            Ok(credential) => Ok(Some(credential)),
            Err(CredentialStoreError::BindingMismatch) => {
                Err(CredentialStoreError::BindingMismatch)
            }
            Err(error) => {
                let _ = remove_record(&path);
                Err(error)
            }
        }
    })
}

fn clear_at(root: &Path, account: &str) -> Result<(), CredentialStoreError> {
    let account = validate_account(account)?;
    let Some(vault) = existing_vault_directory(root)? else {
        return Ok(());
    };
    with_locked_vault(&vault, |vault| remove_record(&record_path(vault, account)))
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
    fn backend_repair_private_credential_file_round_trip_redacts_password() {
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
        let bytes = fs::read(record_path(&vault_directory(&root), "2026000000"))
            .expect("encrypted record reads");
        assert!(!String::from_utf8_lossy(&bytes).contains(password));
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
        assert_eq!(
            load_at(&root, "student", fixture_device()),
            Err(CredentialStoreError::InvalidRecord)
        );
        assert!(!path.exists());
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
