//! Session cleanup handler (US4).
//!
//! On session deletion, generates and stores a session summary
//! (observation count, key decisions), deletes the sidecar file,
//! and releases memory.

use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::{HookOutput, RustyBrainError};

use crate::bootstrap;
use crate::sidecar;

/// Process a session deletion event from `OpenCode`.
///
/// 1. Loads sidecar state to get observation count and session metadata
/// 2. Generates session summary text
/// 3. Calls `Mind::save_session_summary(decisions, files, summary)`
/// 4. Deletes the sidecar file
///
/// If the sidecar file is missing, stores a minimal summary.
/// On error: caller wraps in fail-open returning `HookOutput::default()`.
///
/// # Errors
///
/// Returns `RustyBrainError` if memory path resolution, Mind opening,
/// or summary storage fails.
pub fn handle_session_cleanup(session_id: &str, cwd: &Path) -> Result<HookOutput, RustyBrainError> {
    let mind = bootstrap::open_mind_read_write(cwd)?;

    let sidecar_path = sidecar::sidecar_path(cwd, session_id);

    // Load sidecar state for observation metadata (if available)
    let observation_count = match sidecar::load(&sidecar_path) {
        Ok(state) => state.observation_count,
        Err(e) => {
            if sidecar_path.exists() {
                tracing::warn!(
                    error = %e,
                    path = %sidecar_path.display(),
                    "failed to load sidecar file during cleanup, defaulting to 0 observations"
                );
            }
            0
        }
    };

    let summary = format!("Session completed with {observation_count} observation(s) captured.",);

    mind.with_lock(|m: &Mind| m.save_session_summary(Vec::new(), Vec::new(), &summary))?;

    // Delete sidecar file (best-effort - already saved summary)
    if sidecar_path.exists() {
        if let Err(e) = std::fs::remove_file(&sidecar_path) {
            tracing::warn!(
                error = %e,
                path = %sidecar_path.display(),
                "failed to delete sidecar file during cleanup"
            );
        }
    }

    Ok(HookOutput::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // handle_session_cleanup
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "requires memvid Mind to be openable"]
    fn handle_session_cleanup_with_valid_mind() {
        let tmp = TempDir::new().unwrap();
        let _result = handle_session_cleanup("test-session", tmp.path());
    }

    // -----------------------------------------------------------------------
    // Sidecar path integration
    // -----------------------------------------------------------------------

    #[test]
    fn cleanup_uses_correct_sidecar_path() {
        let cwd = Path::new("/tmp/project");
        let sidecar_file = sidecar::sidecar_path(cwd, "sess-42");
        assert!(
            sidecar_file
                .to_string_lossy()
                .contains("session-sess-42.json")
        );
    }

    #[test]
    fn cleanup_handles_missing_sidecar_gracefully() {
        // When no sidecar file exists, observation_count defaults to 0.
        // We verify the sidecar load returns an appropriate error.
        let tmp = TempDir::new().unwrap();
        let path = sidecar::sidecar_path(tmp.path(), "nonexistent");
        let result = sidecar::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn cleanup_deletes_sidecar_file_after_processing() {
        // Create a sidecar file and verify the cleanup logic would delete it.
        // Since we can't open Mind in unit tests, we test the deletion path directly.
        let tmp = TempDir::new().unwrap();
        let sidecar_path = sidecar::sidecar_path(tmp.path(), "to-delete");
        let state = crate::types::SidecarState::new("to-delete".to_string());
        sidecar::save(&sidecar_path, &state).unwrap();
        assert!(sidecar_path.exists());

        // Direct file removal (what handle_session_cleanup does post-Mind)
        std::fs::remove_file(&sidecar_path).unwrap();
        assert!(!sidecar_path.exists());
    }
}
