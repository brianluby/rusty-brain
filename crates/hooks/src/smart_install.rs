use std::path::Path;

use crate::error::HookError;
use types::hooks::{HookInput, HookOutput};

const VERSION_FILENAME: &str = ".install-version";

/// Handle the smart-install hook event.
///
/// Reads `.install-version`, compares with the current binary version,
/// and writes the current version if missing or mismatched.
///
/// # Errors
///
/// Returns `HookError::Io` on file I/O failure.
pub fn handle_smart_install(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);
    let version_path = cwd.join(VERSION_FILENAME);
    let current_version = env!("CARGO_PKG_VERSION");

    let needs_write = match std::fs::read_to_string(&version_path) {
        Ok(stored) => stored.trim() != current_version,
        Err(_) => true, // Missing file → fresh install
    };

    if needs_write {
        // Atomic write: temp file + rename (unique name avoids collisions)
        let tmp_path = version_path.with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp_path, current_version)?;
        if let Err(e) = std::fs::rename(&tmp_path, &version_path) {
            let _ = std::fs::remove_file(&tmp_path); // Best-effort cleanup
            return Err(e.into());
        }
    }

    Ok(HookOutput {
        continue_execution: Some(true),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_input(cwd: &str) -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "test-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": cwd,
            "permission_mode": "default",
            "hook_event_name": "Notification"
        }))
        .expect("valid HookInput JSON")
    }

    // -----------------------------------------------------------------------
    // handle_smart_install — writes version file
    // -----------------------------------------------------------------------

    #[test]
    fn handle_smart_install_writes_version_file() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());

        let result = handle_smart_install(&input);
        assert!(
            result.is_ok(),
            "handle_smart_install should succeed: {result:?}"
        );

        let version_path = tmp.path().join(VERSION_FILENAME);
        assert!(version_path.exists(), ".install-version file should exist");

        let content = std::fs::read_to_string(&version_path).expect("should read version file");
        assert_eq!(
            content,
            env!("CARGO_PKG_VERSION"),
            "version file should contain current version"
        );
    }

    #[test]
    fn handle_smart_install_returns_continue_true() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());

        let output = handle_smart_install(&input).expect("should succeed");
        assert_eq!(output.continue_execution, Some(true));
    }

    #[test]
    fn handle_smart_install_skips_write_when_version_matches() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let version_path = tmp.path().join(VERSION_FILENAME);

        // Pre-write the current version
        std::fs::write(&version_path, env!("CARGO_PKG_VERSION")).expect("pre-write should succeed");

        let input = make_input(tmp.path().to_str().unwrap());
        let result = handle_smart_install(&input);
        assert!(result.is_ok(), "should succeed when version matches");
    }

    #[test]
    fn handle_smart_install_overwrites_stale_version() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let version_path = tmp.path().join(VERSION_FILENAME);

        // Pre-write an old version
        std::fs::write(&version_path, "0.0.0-old").expect("pre-write should succeed");

        let input = make_input(tmp.path().to_str().unwrap());
        let result = handle_smart_install(&input);
        assert!(result.is_ok(), "should succeed when updating stale version");

        let content = std::fs::read_to_string(&version_path).expect("should read version file");
        assert_eq!(content, env!("CARGO_PKG_VERSION"));
    }
}
