pub use platforms::bootstrap::{
    Diagnostic, DiagnosticLevel, detect_legacy_path, platform_opt_in, should_process,
};

use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::hooks::HookInput;

use crate::error::HookError;

/// Resolve the canonical memory file path for the detected platform.
///
/// # Errors
///
/// Returns `HookError::Platform` if platform path resolution fails.
pub fn resolve_memory_path(input: &HookInput, cwd: &Path) -> Result<std::path::PathBuf, HookError> {
    let platform_name = platforms::detect_platform(input);
    let config = platforms::bootstrap::build_mind_config(cwd, &platform_name).map_err(|e| {
        HookError::Platform {
            message: format!("Failed to build mind config: {e}"),
        }
    })?;
    Ok(config.memory_path)
}

/// Open a read-write `Mind` instance for the detected platform.
///
/// # Errors
///
/// Returns `HookError::Platform` if path resolution fails, or a `HookError`
/// wrapping the underlying `Mind::open` error on storage failure.
pub fn open_mind(input: &HookInput, cwd: &Path) -> Result<Mind, HookError> {
    let platform_name = platforms::detect_platform(input);
    let config = platforms::bootstrap::build_mind_config(cwd, &platform_name).map_err(|e| {
        HookError::Platform {
            message: format!("Failed to build mind config: {e}"),
        }
    })?;
    Ok(Mind::open(config)?)
}

/// Open a read-write `Mind` instance with a pre-resolved memory path.
///
/// # Errors
///
/// Returns `HookError::Platform` on config failure, or a `HookError`
/// wrapping the underlying `Mind::open` error on storage failure.
pub fn open_mind_with_path(memory_path: std::path::PathBuf) -> Result<Mind, HookError> {
    let mut config = types::MindConfig::from_env().map_err(|e| HookError::Platform {
        message: format!("Failed to load config from env: {e}"),
    })?;
    // Only override with caller-provided path when no explicit env override
    if std::env::var("MEMVID_PLATFORM_MEMORY_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .is_none()
    {
        config.memory_path = memory_path;
    }
    Ok(Mind::open(config)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_input(cwd: &str) -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "test-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": cwd,
            "permission_mode": "default",
            "hook_event_name": "SessionStart"
        }))
        .unwrap()
    }

    #[test]
    fn should_process_returns_true_for_standard_input() {
        let input = make_input("/tmp");
        let result = should_process(&input, "session_start");
        assert!(result);
    }
}
