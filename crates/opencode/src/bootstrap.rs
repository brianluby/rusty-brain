pub use platforms::bootstrap::{platform_opt_in, should_process};

use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::{MindConfig, RustyBrainError};

/// Resolve the canonical memory file path for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if platform path resolution fails.
pub fn resolve_memory_path(cwd: &Path) -> Result<std::path::PathBuf, RustyBrainError> {
    let config = platforms::bootstrap::build_mind_config(cwd, "opencode")?;
    Ok(config.memory_path)
}

/// Build a `MindConfig` with the resolved memory path for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if path resolution or env-based config loading fails.
pub fn mind_config(cwd: &Path) -> Result<MindConfig, RustyBrainError> {
    platforms::bootstrap::build_mind_config(cwd, "opencode")
}

/// Open a read-write `Mind` instance for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if config resolution or `Mind::open` fails.
pub fn open_mind_read_write(cwd: &Path) -> Result<Mind, RustyBrainError> {
    Mind::open(mind_config(cwd)?)
}

/// Open a read-only `Mind` instance for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if config resolution or `Mind::open_read_only` fails.
pub fn open_mind_read_only(cwd: &Path) -> Result<Mind, RustyBrainError> {
    Mind::open_read_only(mind_config(cwd)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::HookInput;

    fn make_hook_input(event: &str) -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "test-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/tmp/project",
            "permission_mode": "default",
            "hook_event_name": event,
        }))
        .unwrap()
    }

    #[test]
    fn should_process_returns_bool() {
        let input = make_hook_input("PostToolUse");
        let _result = should_process(&input, "PostToolUse");
    }
}
