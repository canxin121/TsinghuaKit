//! Terminal-only, non-secret device identity. Never stores a login session.
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
};

/// Fail before login if the device identity cannot be retained. Silently
/// generating another fingerprint would cause repeated MFA/device entries.
pub(super) fn resolve(cache: &Path) -> Result<String, String> {
    resolve_inner(cache).map_err(|_| "terminal_device_metadata_unavailable".to_owned())
}

/// Resolves the device marker used by a persisted application runtime.
///
/// Older THYou builds created this non-secret marker with the process umask,
/// which commonly resulted in `0644` on macOS.  The current runtime requires
/// owner-only permissions because the marker is sent as part of the trusted
/// device protocol.  Migrate only that known legacy shape in the already
/// private application directory; all other unsafe shapes remain fail-closed.
pub(super) fn resolve_persistent(cache: &Path) -> Result<String, String> {
    resolve_inner_with_legacy_permission_migration(cache)
        .map_err(|_| "terminal_device_metadata_unavailable".to_owned())
}

fn resolve_inner(cache: &Path) -> io::Result<String> {
    resolve_inner_with_policy(cache, false)
}

fn resolve_inner_with_legacy_permission_migration(cache: &Path) -> io::Result<String> {
    resolve_inner_with_policy(cache, true)
}

fn resolve_inner_with_policy(cache: &Path, migrate_legacy_permissions: bool) -> io::Result<String> {
    if !cache.is_absolute() {
        return Err(io::Error::other("absolute_device_path_required"));
    }
    let parent = cache
        .parent()
        .ok_or_else(|| io::Error::other("device_parent_missing"))?;
    crate::telemetry::private_dir(parent)?;
    let path = parent.join(super::DEVICE_FINGERPRINT_FILE);
    match crate::telemetry::new_private_file(&path) {
        Ok(mut file) => {
            let fingerprint = uuid::Uuid::new_v4().simple().to_string();
            file.write_all(fingerprint.as_bytes())?;
            file.sync_all()?;
            Ok(fingerprint)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let before = fs::symlink_metadata(&path)?;
            if !before.is_file() || before.file_type().is_symlink() || before.len() > 128 {
                return Err(io::Error::other("unsafe_device_file"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if before.nlink() != 1 {
                    return Err(io::Error::other("unsafe_device_permissions"));
                }

                let mode = before.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    // `resolve_fingerprint` in pre-vault THYou releases used
                    // `fs::write`, so a normal macOS umask left this exact
                    // marker at 0644.  Do not accept arbitrary broad modes:
                    // only the one legacy mode is eligible for in-place
                    // tightening, and only after the file is validated below.
                    if !migrate_legacy_permissions || mode != 0o644 {
                        return Err(io::Error::other("unsafe_device_permissions"));
                    }
                    let legacy = fs::File::open(&path)?;
                    let opened = legacy.metadata()?;
                    if before.dev() != opened.dev() || before.ino() != opened.ino() {
                        return Err(io::Error::other("device_file_changed"));
                    }
                    let mut value = String::new();
                    legacy.take(128).read_to_string(&mut value)?;
                    if !super::is_device_fingerprint(value.trim()) {
                        return Err(io::Error::other("invalid_device_identity"));
                    }
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                    let migrated = fs::symlink_metadata(&path)?;
                    if migrated.file_type().is_symlink()
                        || !migrated.is_file()
                        || migrated.nlink() != 1
                        || migrated.permissions().mode() & 0o777 != 0o600
                        || migrated.dev() != before.dev()
                        || migrated.ino() != before.ino()
                    {
                        return Err(io::Error::other("device_file_changed"));
                    }
                    tracing::info!(
                        target: "tsinghua_kit::storage",
                        event = "legacy_device_marker_permissions_migrated"
                    );
                }
            }
            let file = fs::File::open(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let opened = file.metadata()?;
                if before.dev() != opened.dev() || before.ino() != opened.ino() {
                    return Err(io::Error::other("device_file_changed"));
                }
            }
            let mut value = String::new();
            file.take(128).read_to_string(&mut value)?;
            let value = value.trim();
            if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(io::Error::other("invalid_device_identity"));
            }
            Ok(value.to_owned())
        }
        Err(error) => Err(error),
    }
}
