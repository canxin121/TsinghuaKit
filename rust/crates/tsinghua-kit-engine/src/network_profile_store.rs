//! Client-owned local connection-profile storage.
//!
//! Persistence is explicit and independent from Auth/session persistence.
//! Rust writes readable JSON files under the host-selected app data directory.
//! Unix directories and files use owner-only permissions; the data is not
//! encrypted and no operating-system credential service is used.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use fs2::FileExt;
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    error::{Error, ErrorCode, Service},
    network::{
        NetworkAccessMethod, NetworkProfileId, NetworkProfileStoragePolicy, StoredNetworkProfile,
    },
};

const SCHEMA: u32 = 2;
const MAX_PROFILES: usize = 256;
const MAX_STORE_BYTES: usize = 2 * 1024 * 1024;
const DATA_FILE: &str = "profiles-v2.json";
const LEGACY_DATA_FILE: &str = "profiles-v1.bin";
const LEGACY_KEY_FILE: &str = "profiles-key-v1.bin";
const LOCK_FILE: &str = "profiles.lock";
const PRIVATE_DIR: &str = "tsinghua-kit-network-profiles";

pub(crate) struct NetworkProfileStore {
    pub(crate) profiles: HashMap<NetworkProfileId, StoredNetworkProfile>,
    generation: u64,
    backend: Option<JsonDirectoryStore>,
}

impl NetworkProfileStore {
    pub(crate) fn open(policy: NetworkProfileStoragePolicy) -> Result<Self, Error> {
        match policy {
            NetworkProfileStoragePolicy::MemoryOnly => Ok(Self {
                profiles: HashMap::new(),
                generation: 0,
                backend: None,
            }),
            NetworkProfileStoragePolicy::JsonDirectory { root, namespace } => {
                let backend = JsonDirectoryStore::open(root, namespace)?;
                let (generation, profiles) = backend.load()?;
                Ok(Self {
                    profiles,
                    generation,
                    backend: Some(backend),
                })
            }
        }
    }

    pub(crate) fn commit(
        &mut self,
        profiles: HashMap<NetworkProfileId, StoredNetworkProfile>,
    ) -> Result<(), Error> {
        if profiles.len() > MAX_PROFILES {
            return Err(storage_error());
        }
        let generation = self.generation.checked_add(1).ok_or_else(storage_error)?;
        if let Some(backend) = &self.backend {
            backend.persist(generation, &profiles)?;
        }
        self.profiles = profiles;
        self.generation = generation;
        Ok(())
    }
}

struct JsonDirectoryStore {
    directory: PathBuf,
    namespace: String,
    _lock: File,
}

impl JsonDirectoryStore {
    fn open(root: PathBuf, namespace: String) -> Result<Self, Error> {
        if !root.is_absolute() || !valid_namespace(&namespace) {
            return Err(Error::new(Service::Network, ErrorCode::InvalidInput));
        }
        ensure_private_directory(&root)?;
        let app_root = root.join(PRIVATE_DIR);
        ensure_private_directory(&app_root)?;
        let namespace_hash = BASE64.encode(digest(&SHA256, namespace.as_bytes()).as_ref());
        let directory = app_root.join(namespace_hash);
        ensure_private_directory(&directory)?;

        let lock_path = directory.join(LOCK_FILE);
        reject_symlink(&lock_path)?;
        let mut lock_options = OpenOptions::new();
        lock_options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            lock_options.mode(0o600);
        }
        let lock = lock_options.open(&lock_path).map_err(|_| storage_error())?;
        set_private_file_permissions(&lock_path)?;
        lock.try_lock_exclusive().map_err(|_| storage_error())?;

        Ok(Self {
            directory,
            namespace,
            _lock: lock,
        })
    }

    fn load(&self) -> Result<(u64, HashMap<NetworkProfileId, StoredNetworkProfile>), Error> {
        if self.legacy_format_present()? {
            return Err(storage_error());
        }
        let path = self.directory.join(DATA_FILE);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((0, HashMap::new()));
            }
            Err(_) => return Err(storage_error()),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(storage_error());
            }
            Ok(_) => {}
        }
        verify_private_file(&path)?;
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|file| {
                file.take((MAX_STORE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|_| storage_error())?;
        if bytes.is_empty() || bytes.len() > MAX_STORE_BYTES {
            return Err(storage_error());
        }
        let payload: StorePayload = serde_json::from_slice(&bytes).map_err(|_| storage_error())?;
        if payload.schema != SCHEMA
            || payload.namespace != self.namespace
            || payload.profiles.len() > MAX_PROFILES
        {
            return Err(storage_error());
        }
        let mut profiles = HashMap::with_capacity(payload.profiles.len());
        for record in payload.profiles {
            let id = NetworkProfileId::parse(&record.id).map_err(|_| storage_error())?;
            let label = record.label.trim();
            let username = record.username.trim();
            if !valid_profile_text(label, 80)
                || !valid_profile_text(username, 128)
                || record.revision == 0
            {
                return Err(storage_error());
            }
            let method =
                NetworkAccessMethod::from_storage_name(&record.method).ok_or_else(storage_error)?;
            let password = record.password.map(|secret| secret.0);
            if profiles
                .insert(
                    id,
                    StoredNetworkProfile {
                        id,
                        label: label.to_owned(),
                        username: username.to_owned(),
                        method,
                        password,
                        revision: record.revision,
                    },
                )
                .is_some()
            {
                return Err(storage_error());
            }
        }
        Ok((payload.generation, profiles))
    }

    fn legacy_format_present(&self) -> Result<bool, Error> {
        for name in [LEGACY_DATA_FILE, LEGACY_KEY_FILE] {
            let path = self.directory.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    return Err(storage_error());
                }
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(storage_error()),
            }
        }
        Ok(false)
    }

    fn persist(
        &self,
        generation: u64,
        profiles: &HashMap<NetworkProfileId, StoredNetworkProfile>,
    ) -> Result<(), Error> {
        if profiles.len() > MAX_PROFILES {
            return Err(storage_error());
        }
        let mut records = profiles.values().collect::<Vec<_>>();
        records.sort_by_key(|profile| profile.id.as_str());
        let payload = PersistedStorePayload {
            schema: SCHEMA,
            namespace: &self.namespace,
            generation,
            profiles: records
                .into_iter()
                .map(|profile| PersistedProfile {
                    id: profile.id.as_str(),
                    label: &profile.label,
                    username: &profile.username,
                    method: profile.method.storage_name(),
                    password: profile.password.as_ref().map(|value| value.as_str()),
                    revision: profile.revision,
                })
                .collect(),
        };
        let mut bytes =
            Zeroizing::new(serde_json::to_vec_pretty(&payload).map_err(|_| storage_error())?);
        if bytes.is_empty() || bytes.len() > MAX_STORE_BYTES {
            return Err(storage_error());
        }
        atomic_write(&self.directory, &self.directory.join(DATA_FILE), &bytes)?;
        bytes.zeroize();
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StorePayload {
    schema: u32,
    namespace: String,
    generation: u64,
    profiles: Vec<StoredProfileRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProfileRecord {
    id: String,
    label: String,
    username: String,
    method: String,
    password: Option<StoredSecret>,
    revision: u64,
}

struct StoredSecret(Zeroizing<String>);

impl<'de> Deserialize<'de> for StoredSecret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            return Err(serde::de::Error::custom("invalid secret field"));
        }
        Ok(Self(Zeroizing::new(value)))
    }
}

#[derive(Serialize)]
struct PersistedStorePayload<'a> {
    schema: u32,
    namespace: &'a str,
    generation: u64,
    profiles: Vec<PersistedProfile<'a>>,
}

#[derive(Serialize)]
struct PersistedProfile<'a> {
    id: String,
    label: &'a str,
    username: &'a str,
    method: &'static str,
    password: Option<&'a str>,
    revision: u64,
}

fn valid_namespace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
}

fn valid_profile_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && value.chars().all(|ch| !ch.is_control())
}

fn storage_error() -> Error {
    Error::new(Service::Network, ErrorCode::StorageUnavailable)
}

fn ensure_private_directory(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).map_err(|_| storage_error())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| storage_error())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(storage_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| storage_error())?;
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| storage_error())?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn verify_private_file(path: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path).map_err(|_| storage_error())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(storage_error());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(storage_error());
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(storage_error())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(storage_error()),
    }
}

fn atomic_write(directory: &Path, target: &Path, bytes: &[u8]) -> Result<(), Error> {
    reject_symlink(target)?;
    let temporary = directory.join(format!("profiles-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary).map_err(|_| storage_error())?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| storage_error())?;
        set_private_file_permissions(&temporary)?;

        #[cfg(windows)]
        {
            let backup = target.with_extension("json.bak");
            reject_symlink(&backup)?;
            if backup.exists() {
                if target.exists() {
                    fs::remove_file(&backup).map_err(|_| storage_error())?;
                } else {
                    fs::rename(&backup, target).map_err(|_| storage_error())?;
                }
            }
            if target.exists() {
                fs::rename(target, &backup).map_err(|_| storage_error())?;
            }
            if let Err(error) = fs::rename(&temporary, target) {
                if backup.exists() {
                    let _ = fs::rename(&backup, target);
                }
                return Err(error);
            }
            if backup.exists() {
                fs::remove_file(&backup).map_err(|_| storage_error())?;
            }
        }
        #[cfg(not(windows))]
        fs::rename(&temporary, target).map_err(|_| storage_error())?;

        set_private_file_permissions(target)?;
        if let Ok(directory) = File::open(directory) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{NetworkProfileInput, NetworkProfileStoragePolicy};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tsinghua-kit-profile-test-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn backend_refactor_network_profile_store_round_trips_readable_json_records() {
        let temporary = TestDirectory::new();
        let policy = NetworkProfileStoragePolicy::json_directory(
            temporary.path(),
            "org.example.profile-test",
        )
        .unwrap();
        let mut store = NetworkProfileStore::open(policy.clone()).unwrap();
        let (id, password) = {
            let mut profiles = store.profiles.clone();
            let input = NetworkProfileInput::new(
                "Local portal",
                "portal-account",
                NetworkAccessMethod::Portal,
            )
            .unwrap()
            .save_password("synthetic-network-secret")
            .unwrap();
            let (label, username, method, password) = input.into_parts();
            let id = NetworkProfileId::new();
            let stored = StoredNetworkProfile {
                id,
                label,
                username,
                method,
                password,
                revision: 1,
            };
            let password = stored.password.as_ref().unwrap().to_string();
            profiles.insert(id, stored);
            store.commit(profiles).unwrap();
            (id, password)
        };
        let data_path = store.backend.as_ref().unwrap().directory.join(DATA_FILE);
        let raw = fs::read(&data_path).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(json["profiles"][0]["password"], password);
        assert!(!data_path.with_file_name(LEGACY_KEY_FILE).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&data_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(store);

        let restored = NetworkProfileStore::open(policy.clone()).unwrap();
        let profile = restored.profiles.get(&id).unwrap();
        assert_eq!(profile.username, "portal-account");
        assert_eq!(
            profile.password.as_deref().map(String::as_str),
            Some(password.as_str())
        );
        drop(restored);

        let mut tampered = fs::read(&data_path).unwrap();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        fs::write(&data_path, &tampered).unwrap();
        let corrupted = match NetworkProfileStore::open(policy) {
            Ok(_) => panic!("tampered profile store must not be treated as empty"),
            Err(error) => error,
        };
        assert_eq!(corrupted.code(), ErrorCode::StorageUnavailable);
        assert_eq!(fs::read(&data_path).unwrap(), tampered);
    }

    #[test]
    fn backend_refactor_network_profile_store_preserves_legacy_files_and_reports_error() {
        let temporary = TestDirectory::new();
        let policy = NetworkProfileStoragePolicy::json_directory(
            temporary.path(),
            "org.example.external-key-migration",
        )
        .unwrap();
        let store = NetworkProfileStore::open(policy.clone()).unwrap();
        let directory = store.backend.as_ref().unwrap().directory.clone();
        drop(store);

        let old_data = directory.join(LEGACY_DATA_FILE);
        let old_key = directory.join(LEGACY_KEY_FILE);
        let legacy_data = b"legacy encrypted profiles";
        let legacy_key = b"legacy key fixture";
        fs::write(&old_data, legacy_data).unwrap();
        fs::write(&old_key, legacy_key).unwrap();
        set_private_file_permissions(&old_data).unwrap();
        set_private_file_permissions(&old_key).unwrap();

        let error = match NetworkProfileStore::open(policy) {
            Ok(_) => panic!("an external-key envelope must not be read as empty data"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::StorageUnavailable);
        assert_eq!(fs::read(&old_data).unwrap(), legacy_data);
        assert_eq!(fs::read(&old_key).unwrap(), legacy_key);
    }

    #[test]
    fn backend_refactor_network_profile_vault_is_namespace_scoped_and_exclusive() {
        let temporary = TestDirectory::new();
        let policy =
            NetworkProfileStoragePolicy::json_directory(temporary.path(), "org.example.one")
                .unwrap();
        let store = NetworkProfileStore::open(policy.clone()).unwrap();
        let second = match NetworkProfileStore::open(policy) {
            Ok(_) => panic!("a second Client must not share the exclusive profile store"),
            Err(error) => error,
        };
        assert_eq!(second.service(), Service::Network);
        assert_eq!(second.code(), ErrorCode::StorageUnavailable);
        drop(store);

        let other_namespace =
            NetworkProfileStoragePolicy::json_directory(temporary.path(), "org.example.two")
                .unwrap();
        let other = NetworkProfileStore::open(other_namespace).unwrap();
        assert!(other.profiles.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn backend_refactor_network_profile_vault_rejects_symlink_root() {
        use std::os::unix::fs::symlink;

        let temporary = TestDirectory::new();
        let actual = temporary.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let alias = temporary.path().join("alias");
        symlink(&actual, &alias).unwrap();
        let policy =
            NetworkProfileStoragePolicy::json_directory(alias, "org.example.symlink-test").unwrap();
        let error = match NetworkProfileStore::open(policy) {
            Ok(_) => panic!("profile storage must not follow a symlink root"),
            Err(error) => error,
        };
        assert_eq!(error.code(), ErrorCode::StorageUnavailable);
    }
}
