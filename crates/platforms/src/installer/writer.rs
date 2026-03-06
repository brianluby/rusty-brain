//! Atomic config file writer with backup support.

use std::fs;
use std::path::Path;

use tempfile::NamedTempFile;
use types::install::{ConfigFile, InstallError};

use super::validate_config_path;

/// Writes configuration files atomically using temp-file-then-rename.
pub struct ConfigWriter;

impl ConfigWriter {
    /// Write a config file atomically.
    ///
    /// If `backup` is true and the target file exists, creates a `.bak` copy first.
    /// Creates parent directories if they don't exist (M-12).
    /// Sets file permissions to 0o644 on Unix (SEC-1).
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::IoError`] if directory creation, file writing,
    /// permission setting, or atomic rename fails.
    pub fn write(config: &ConfigFile, backup: bool) -> Result<(), InstallError> {
        let target = &config.target_path;

        // Validate path against traversal attacks (SEC-4).
        validate_config_path(target)?;

        // Create parent directories if needed.
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| InstallError::IoError {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        // Backup existing file if requested.
        if backup && target.exists() {
            Self::backup(target)?;
        }

        // Write to temp file in same directory (same filesystem for atomic rename).
        let parent = target.parent().unwrap_or(Path::new("."));
        let temp = NamedTempFile::new_in(parent).map_err(|source| InstallError::IoError {
            path: parent.to_path_buf(),
            source,
        })?;

        fs::write(temp.path(), &config.content).map_err(|source| InstallError::IoError {
            path: temp.path().to_path_buf(),
            source,
        })?;

        // Set permissions on Unix (SEC-1).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o644);
            fs::set_permissions(temp.path(), perms).map_err(|source| InstallError::IoError {
                path: temp.path().to_path_buf(),
                source,
            })?;
        }

        // Atomic rename (persist consumes the NamedTempFile).
        temp.persist(target).map_err(|e| InstallError::IoError {
            path: target.clone(),
            source: e.error,
        })?;

        Ok(())
    }

    /// Create a `.bak` backup of an existing file (SEC-8, S-1).
    ///
    /// # Errors
    ///
    /// Returns [`InstallError::IoError`] if the file copy fails.
    pub fn backup(path: &Path) -> Result<(), InstallError> {
        if !path.exists() {
            return Ok(());
        }
        let backup_path = path.with_extension(match path.extension() {
            Some(ext) => format!("{}.bak", ext.to_string_lossy()),
            None => "bak".to_string(),
        });
        fs::copy(path, &backup_path).map_err(|source| InstallError::IoError {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_config(dir: &Path, filename: &str, content: &str) -> ConfigFile {
        ConfigFile {
            target_path: dir.join(filename),
            content: content.to_string(),
            description: "test config".to_string(),
        }
    }

    // T019: ConfigWriter tests

    #[test]
    fn write_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let config = make_config(dir.path(), "test.json", r#"{"key": "value"}"#);

        ConfigWriter::write(&config, false).unwrap();

        assert!(config.target_path.exists());
        let content = fs::read_to_string(&config.target_path).unwrap();
        assert_eq!(content, r#"{"key": "value"}"#);
    }

    #[test]
    fn write_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let nested_path = dir.path().join("a").join("b").join("c").join("test.json");
        let config = ConfigFile {
            target_path: nested_path.clone(),
            content: "test".to_string(),
            description: "test".to_string(),
        };

        ConfigWriter::write(&config, false).unwrap();

        assert!(nested_path.exists());
    }

    #[test]
    fn write_with_backup_creates_bak_file() {
        let dir = TempDir::new().unwrap();
        let config = make_config(dir.path(), "config.json", "original");
        ConfigWriter::write(&config, false).unwrap();

        // Overwrite with backup
        let updated = make_config(dir.path(), "config.json", "updated");
        ConfigWriter::write(&updated, true).unwrap();

        // Check original was backed up
        let bak_path = dir.path().join("config.json.bak");
        assert!(bak_path.exists());
        let bak_content = fs::read_to_string(&bak_path).unwrap();
        assert_eq!(bak_content, "original");

        // Check new content
        let new_content = fs::read_to_string(&config.target_path).unwrap();
        assert_eq!(new_content, "updated");
    }

    #[test]
    fn backup_no_existing_file_is_ok() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        assert!(ConfigWriter::backup(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn write_sets_permissions_0o644() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let config = make_config(dir.path(), "perms.json", "content");
        ConfigWriter::write(&config, false).unwrap();

        let metadata = fs::metadata(&config.target_path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "file permissions should be 0o644");
    }

    #[test]
    fn write_overwrites_without_backup_when_not_requested() {
        let dir = TempDir::new().unwrap();
        let config = make_config(dir.path(), "test.json", "original");
        ConfigWriter::write(&config, false).unwrap();

        let updated = make_config(dir.path(), "test.json", "updated");
        ConfigWriter::write(&updated, false).unwrap();

        let bak_path = dir.path().join("test.json.bak");
        assert!(
            !bak_path.exists(),
            "no .bak file should exist without backup flag"
        );
    }
}
