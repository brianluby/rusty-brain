//! Mind lifecycle: open, recovery, and file permissions.

use std::path::Path;
use std::sync::Mutex;

use crate::backend::{MemvidBackend, OpenAction};
use crate::file_guard;
use crate::memvid_store::MemvidStore;
use types::{MindConfig, RustyBrainError, error_codes};

use super::Mind;

impl Mind {
    /// Open or create a memory file based on config.
    ///
    /// Flow: `FileGuard` validates → `MemvidStore` creates/opens → Mind initialized.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError` if file validation fails, the backend cannot
    /// create/open the file, or parent directory creation fails.
    #[tracing::instrument(skip(config), fields(path = %config.memory_path.display()))]
    pub fn open(config: MindConfig) -> Result<Mind, RustyBrainError> {
        config.validate()?;
        let memory_path = config.memory_path.clone();
        let action = file_guard::validate_and_open(&memory_path)?;

        if config.debug {
            tracing::debug!(path = %memory_path.display(), "mind debug mode enabled");
        }

        let backend = Box::new(MemvidStore::new());
        match action {
            OpenAction::Create => {
                backend.create(&memory_path)?;
                set_file_permissions_0600(&memory_path)?;
            }
            OpenAction::Open => {
                if let Err(e) = backend.open(&memory_path) {
                    if should_recover_from_open_error(&e) {
                        tracing::info!(error = %e, path = %memory_path.display(), "corrupted memory file detected, recovering");
                        file_guard::backup_and_prune(&memory_path, 3)?;
                        backend.create(&memory_path)?;
                        set_file_permissions_0600(&memory_path)?;
                    } else {
                        return Err(e);
                    }
                } else {
                    // Harden pre-existing stores on normal open as well.
                    set_file_permissions_0600(&memory_path)?;
                }
            }
        }

        let session_id = ulid::Ulid::new().to_string().to_lowercase();

        tracing::info!(session_id = %session_id, "mind opened");

        Ok(Mind {
            backend,
            config,
            session_id,
            memory_path,
            initialized: true,
            cached_stats: Mutex::new(None),
        })
    }

    /// Open an existing memory file in read-only mode (no recovery).
    ///
    /// Unlike [`Mind::open`], this method never creates files, never backs up
    /// corrupted files, and never performs destructive recovery. If the file
    /// cannot be opened (e.g. corrupted), the backend error is returned
    /// directly. Intended for read-only CLI access.
    ///
    /// The caller is expected to pre-validate that the file exists and is a
    /// regular file before calling this method.
    ///
    /// # Errors
    ///
    /// Returns backend-open errors directly (including corruption, permission,
    /// and version errors) plus config/file validation errors.
    #[tracing::instrument(skip(config), fields(path = %config.memory_path.display()))]
    pub fn open_read_only(config: MindConfig) -> Result<Mind, RustyBrainError> {
        config.validate()?;
        let memory_path = config.memory_path.clone();
        file_guard::validate_existing(&memory_path)?;

        let backend = Box::new(MemvidStore::new());
        backend.open(&memory_path)?;

        let session_id = ulid::Ulid::new().to_string().to_lowercase();

        tracing::info!(session_id = %session_id, "mind opened (read-only)");

        Ok(Mind {
            backend,
            config,
            session_id,
            memory_path,
            initialized: true,
            cached_stats: Mutex::new(None),
        })
    }

    /// Open with a custom backend (for testing with `MockBackend`).
    #[cfg(test)]
    pub(crate) fn open_with_backend(
        config: MindConfig,
        backend: Box<dyn MemvidBackend>,
    ) -> Result<Mind, RustyBrainError> {
        let memory_path = config.memory_path.clone();

        // For test backend, call create if file doesn't exist, open if it does.
        if memory_path.exists() {
            backend.open(&memory_path)?;
        } else {
            backend.create(&memory_path)?;
        }

        let session_id = ulid::Ulid::new().to_string().to_lowercase();

        Ok(Mind {
            backend,
            config,
            session_id,
            memory_path,
            initialized: true,
            cached_stats: Mutex::new(None),
        })
    }
}

fn should_recover_from_open_error(err: &RustyBrainError) -> bool {
    match err {
        RustyBrainError::CorruptedFile { .. } | RustyBrainError::MemoryCorruption { .. } => true,
        // Storage backend errors from memvid-core: only recover for explicit
        // corruption indicators. Non-corruption conditions (version mismatch,
        // transient I/O, permission errors) should NOT trigger destructive
        // backup-and-recreate recovery.
        RustyBrainError::Storage { message, .. } => {
            let msg = message.to_lowercase();
            msg.contains("corrupt")
                || msg.contains("invalid")
                || msg.contains("malformed")
                || msg.contains("cannot be opened")
                || msg.contains("failed to fill whole buffer")
        }
        _ => false,
    }
}

/// Set file permissions to 0600 (owner read/write only) per SEC-1.
///
/// # Errors
///
/// Returns `RustyBrainError::FileSystem` if the permission change fails.
#[cfg(unix)]
fn set_file_permissions_0600(path: &Path) -> Result<(), RustyBrainError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms).map_err(|e| RustyBrainError::FileSystem {
        code: error_codes::E_FS_IO_ERROR,
        message: format!("failed to set file permissions: {}", path.display()),
        source: Some(e),
    })
}

#[cfg(not(unix))]
fn set_file_permissions_0600(_path: &Path) -> Result<(), RustyBrainError> {
    Ok(())
}
