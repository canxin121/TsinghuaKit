//! Durable, root-scoped revocation. This file contains no account or secret.
//! Lock order is authority -> session/vault. Never hold a session/vault lock
//! while acquiring this lock. A captured lease may be checked, never refreshed
//! by a response writer; only a new explicit login creates a new generation.
use super::*;

const AUTHORITY_FILE: &str = "campus-auth-authority-v1.json";
const AUTHORITY_LOCK: &str = "campus-auth-authority-v1.lock";
const LEGACY_GENERATION: &str = "legacy-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    schema: u32,
    generation: String,
    revoked: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SessionLease {
    root: PathBuf,
    generation: String,
}

impl fmt::Debug for SessionLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionLease([private authority])")
    }
}

fn with_lock<T>(root: &Path, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    ensure_private_directory(root)?;
    let path = root.join(AUTHORITY_LOCK);
    reject_symlink(&path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|_| "session authority unavailable".to_owned())?;
    private_file_permissions(&path)?;
    // Do not freeze the UI indefinitely on an externally held filesystem lock.
    file.try_lock_exclusive()
        .map_err(|_| "session authority busy".to_owned())?;
    let result = action();
    let unlock = file.unlock();
    if result.is_ok() && unlock.is_err() {
        return Err("session authority unavailable".to_owned());
    }
    result
}

fn read_unlocked(root: &Path) -> Result<Authority, String> {
    let path = root.join(AUTHORITY_FILE);
    reject_symlink(&path)?;
    match fs::metadata(&path) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(Authority {
                schema: 1,
                generation: LEGACY_GENERATION.to_owned(),
                revoked: false,
            });
        }
        Err(_) => return Err("session authority unavailable".to_owned()),
        Ok(m) if m.len() > 1024 || m.len() == 0 => {
            return Err("session authority invalid".to_owned());
        }
        Ok(_) => {}
    }
    private_file_permissions(&path)?;
    let value: Authority = serde_json::from_slice(
        &fs::read(path).map_err(|_| "session authority unavailable".to_owned())?,
    )
    .map_err(|_| "session authority invalid".to_owned())?;
    if value.schema != 1
        || (value.generation != LEGACY_GENERATION
            && uuid::Uuid::parse_str(&value.generation).is_err())
    {
        return Err("session authority invalid".to_owned());
    }
    Ok(value)
}

fn write_unlocked(root: &Path, value: &Authority) -> Result<(), String> {
    let destination = root.join(AUTHORITY_FILE);
    reject_symlink(&destination)?;
    let temporary = root.join(format!(".auth-authority-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = private_writer(&temporary)?;
        let bytes =
            serde_json::to_vec(value).map_err(|_| "session authority invalid".to_owned())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "session authority unavailable".to_owned())?;
        drop(file);
        commit_file(&temporary, &destination)?;
        #[cfg(unix)]
        File::open(root)
            .and_then(|f| f.sync_all())
            .map_err(|_| "session authority unavailable".to_owned())?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

impl SessionLease {
    pub(crate) fn belongs_to(&self, root: &Path) -> bool {
        self.root == root
    }

    pub(crate) fn current(root: &Path) -> Result<Option<Self>, String> {
        with_lock(root, || {
            let state = read_unlocked(root)?;
            Ok((!state.revoked).then(|| Self {
                root: root.to_owned(),
                generation: state.generation,
            }))
        })
    }

    pub(crate) fn check_current(&self) -> Result<bool, String> {
        with_lock(&self.root, || {
            let state = read_unlocked(&self.root)?;
            Ok(!state.revoked && state.generation == self.generation)
        })
    }

    pub(crate) fn is_current(&self) -> bool {
        self.check_current().unwrap_or(false)
    }

    pub(crate) fn with_current<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        with_lock(&self.root, || {
            let state = read_unlocked(&self.root)?;
            if state.revoked || state.generation != self.generation {
                return Err("session authority revoked".to_owned());
            }
            action()
        })
    }
}

/// Read both files under the same generation and session lock. A tombstone
/// suppresses all pre-logout metadata, even if physical deletion previously
/// failed. Metadata alone is still never live authentication proof.
pub(crate) fn load_authorized_state(
    root: &Path,
) -> Result<
    (
        Option<SessionLease>,
        Option<ResumeSnapshot>,
        Option<ResumeAccountMetadata>,
    ),
    String,
> {
    with_lock(root, || {
        let state = read_unlocked(root)?;
        if state.revoked {
            return Ok((None, None, None));
        }
        let lease = SessionLease {
            root: root.to_owned(),
            generation: state.generation,
        };
        with_session_store_lock(root, || {
            clear_at_unlocked(&root.join(LEGACY_SESSION_FILE))?;
            let metadata = load_account_metadata_at_unlocked(&root.join(ACCOUNT_METADATA_FILE))?;
            let snapshot = load_at_unlocked(&root.join(SESSION_FILE))?;
            Ok((Some(lease), snapshot, metadata))
        })
    })
}

/// Open only an explicit user login. Publish a tombstone first; failed cleanup
/// cannot accidentally make old files current under the new generation.
/// Same-account recovery metadata may survive a failed fresh login, but an
/// account switch must not restore the previous account's locator.
pub(crate) fn begin_explicit_authority(
    root: &Path,
    username: &str,
) -> Result<SessionLease, String> {
    begin_explicit_authority_with_opt_in(root, username, true)
}

pub(crate) fn begin_explicit_authority_with_opt_in(
    root: &Path,
    username: &str,
    remember_credentials: bool,
) -> Result<SessionLease, String> {
    with_lock(root, || {
        let previous = read_unlocked(root)?;
        let mut state = Authority {
            schema: 1,
            generation: uuid::Uuid::new_v4().to_string(),
            revoked: true,
        };
        write_unlocked(root, &state)?;
        with_session_store_lock(root, || {
            clear_at_unlocked(&root.join(SESSION_FILE))?;
            clear_at_unlocked(&root.join(LEGACY_SESSION_FILE))?;
            let metadata = load_account_metadata_at_unlocked(&root.join(ACCOUNT_METADATA_FILE))?;
            if previous.revoked || metadata.as_ref().is_some_and(|m| m.username != username) {
                clear_at_unlocked(&root.join(ACCOUNT_METADATA_FILE))?;
            } else if !remember_credentials {
                if let Some(metadata) = metadata {
                    save_account_metadata_at_unlocked(
                        &root.join(ACCOUNT_METADATA_FILE),
                        &metadata.with_credentials_saved(false),
                    )?;
                }
            }
            Ok(())
        })?;
        state.revoked = false;
        write_unlocked(root, &state)?;
        Ok(SessionLease {
            root: root.to_owned(),
            generation: state.generation,
        })
    })
}

/// Revoke before deletion. A stale runtime must not delete the newer account's
/// files. Return cleanup errors even when logical revocation succeeded.
pub(crate) fn revoke_authority(
    root: &Path,
    lease: Option<&SessionLease>,
    username: Option<&str>,
) -> Result<(), String> {
    with_lock(root, || {
        let state = read_unlocked(root)?;
        // An already-signed-out runtime owns no authority to revoke a later
        // login in another window. Never treat absence of a lease as a wildcard.
        if lease.is_none() && !state.revoked {
            return Ok(());
        }
        if let Some(lease) = lease {
            if !lease.belongs_to(root) || (!state.revoked && state.generation != lease.generation) {
                return Ok(()); // A newer explicit login already superseded us.
            }
        }
        write_unlocked(
            root,
            &Authority {
                schema: 1,
                generation: uuid::Uuid::new_v4().to_string(),
                revoked: true,
            },
        )?;
        let credential_result = username
            .map(|name| crate::credential_store::clear_at_root(root, name))
            .transpose();
        let session_result = clear_at_root(root);
        if credential_result.is_err() || session_result.is_err() {
            return Err("session revocation cleanup incomplete".to_owned());
        }
        Ok(())
    })
}
