//! Pre-open file validation, backup management, and size guards.
//!
//! [`validate_and_open`] checks a memory file path before any I/O, returning
//! an [`OpenAction`](super::backend::OpenAction) indicating whether to create
//! or open the file. [`backup_and_prune`] creates timestamped backups and
//! removes old ones.

use crate::backend::OpenAction;
use std::path::Path;
use types::{RustyBrainError, error_codes};

/// Maximum allowed file size (100 MB).
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// System paths that are rejected during validation (Unix-only;
/// harmlessly unmatched on Windows where paths use different prefixes).
const FORBIDDEN_PREFIXES: &[&str] = &["/dev/", "/proc/", "/sys/"];

/// Validate a memory file path before attempting to open.
///
/// - Path resolving to system locations (`/dev/`, `/proc/`, `/sys/`) is rejected.
/// - Missing file returns `OpenAction::Create` (parent directories are created).
/// - Existing file >100 MB returns `Err(FileTooLarge)`.
/// - Existing file within size guard returns `OpenAction::Open`.
pub(crate) fn validate_and_open(path: &Path) -> Result<OpenAction, RustyBrainError> {
    let path_str = path.to_string_lossy();
    for prefix in FORBIDDEN_PREFIXES {
        if path_str.starts_with(prefix) {
            return Err(RustyBrainError::FileSystem {
                code: error_codes::E_FS_PERMISSION_DENIED,
                message: format!("system path rejected: {path_str}"),
                source: None,
            });
        }
    }

    if !path.exists() {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| RustyBrainError::FileSystem {
                    code: error_codes::E_FS_IO_ERROR,
                    message: format!("failed to create parent directories: {}", parent.display()),
                    source: Some(e),
                })?;
            }
        }
        return Ok(OpenAction::Create);
    }

    let metadata = std::fs::metadata(path).map_err(|e| RustyBrainError::FileSystem {
        code: error_codes::E_FS_IO_ERROR,
        message: format!("failed to read file metadata: {path_str}"),
        source: Some(e),
    })?;

    if !metadata.is_file() {
        return Err(RustyBrainError::FileSystem {
            code: error_codes::E_FS_IO_ERROR,
            message: format!("path is not a regular file: {path_str}"),
            source: None,
        });
    }

    if metadata.len() > MAX_FILE_SIZE {
        return Err(RustyBrainError::FileTooLarge {
            code: error_codes::E_STORAGE_FILE_TOO_LARGE,
            message: format!(
                "file size {} bytes exceeds maximum {} bytes: {path_str}",
                metadata.len(),
                MAX_FILE_SIZE,
            ),
        });
    }

    Ok(OpenAction::Open)
}

/// Create a timestamped backup and prune old backups.
///
/// Renames `path` to `{path}.backup-{YYYYMMDD-HHMMSS}`. Sets backup file
/// permissions to 0600. Deletes oldest backups beyond `max_backups`.
pub(crate) fn backup_and_prune(path: &Path, max_backups: usize) -> Result<(), RustyBrainError> {
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%f");
    let backup_name = format!("{}.backup-{timestamp}", path.display());
    let backup_path = Path::new(&backup_name);

    std::fs::rename(path, backup_path).map_err(|e| RustyBrainError::FileSystem {
        code: error_codes::E_FS_IO_ERROR,
        message: format!("failed to create backup: {backup_name}"),
        source: Some(e),
    })?;

    // Set backup permissions to 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(backup_path, perms).map_err(|e| RustyBrainError::FileSystem {
            code: error_codes::E_FS_IO_ERROR,
            message: format!("failed to set backup permissions: {backup_name}"),
            source: Some(e),
        })?;
    }

    // Prune old backups
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let prefix = format!("{stem}.backup-");

    let mut backups: Vec<std::path::PathBuf> = std::fs::read_dir(parent)
        .map_err(|e| RustyBrainError::FileSystem {
            code: error_codes::E_FS_IO_ERROR,
            message: format!("failed to read backup directory: {}", parent.display()),
            source: Some(e),
        })?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect();

    // Sort descending (newest first) by filename (timestamp in name)
    backups.sort();
    backups.reverse();

    // Delete beyond max_backups
    for old_backup in backups.iter().skip(max_backups) {
        let _ = std::fs::remove_file(old_backup);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_missing_file_returns_create() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.mv2");
        let result = validate_and_open(&path).unwrap();
        assert!(matches!(result, OpenAction::Create));
    }

    #[test]
    fn validate_existing_valid_file_returns_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mv2");
        std::fs::write(&path, b"valid data").unwrap();
        let result = validate_and_open(&path).unwrap();
        assert!(matches!(result, OpenAction::Open));
    }

    #[test]
    fn validate_system_path_rejected() {
        let path = Path::new("/dev/null.mv2");
        let result = validate_and_open(path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), error_codes::E_FS_PERMISSION_DENIED);
    }

    #[test]
    fn validate_proc_path_rejected() {
        let path = Path::new("/proc/self/test.mv2");
        let result = validate_and_open(path);
        assert!(result.is_err());
    }

    #[test]
    fn validate_sys_path_rejected() {
        let path = Path::new("/sys/test.mv2");
        let result = validate_and_open(path);
        assert!(result.is_err());
    }

    #[test]
    fn validate_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("test.mv2");
        let result = validate_and_open(&path).unwrap();
        assert!(matches!(result, OpenAction::Create));
        assert!(dir.path().join("nested").join("deep").exists());
    }

    #[test]
    fn validate_oversized_file_returns_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.mv2");
        // Create a sparse file that reports >100MB without consuming disk.
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_SIZE + 1).unwrap();

        let result = validate_and_open(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), error_codes::E_STORAGE_FILE_TOO_LARGE);
    }

    #[test]
    fn backup_and_prune_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mv2");
        std::fs::write(&path, b"original data").unwrap();

        backup_and_prune(&path, 3).unwrap();

        // Original should be gone
        assert!(!path.exists());

        // Backup should exist
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("test.mv2.backup-")
            })
            .collect();
        assert_eq!(entries.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn backup_and_prune_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mv2");
        std::fs::write(&path, b"original data").unwrap();

        backup_and_prune(&path, 3).unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("test.mv2.backup-")
            })
            .collect();
        assert_eq!(entries.len(), 1);

        let perms = std::fs::metadata(entries[0].path()).unwrap().permissions();
        assert_eq!(
            perms.mode() & 0o777,
            0o600,
            "backup should have 0600 permissions"
        );
    }

    #[test]
    fn backup_and_prune_keeps_max_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mv2");

        // Create 4 backups manually
        for i in 0..4 {
            let backup_name = format!("test.mv2.backup-20260301-00000{i}");
            std::fs::write(dir.path().join(&backup_name), format!("backup {i}")).unwrap();
        }

        // Create current file and backup it (5th total)
        std::fs::write(&path, b"current").unwrap();
        backup_and_prune(&path, 3).unwrap();

        // Should have at most 3 backups
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("test.mv2.backup-")
            })
            .collect();
        assert!(entries.len() <= 3);
    }
}
