use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::bootstrap;
use crate::context::format_system_message;
use crate::error::HookError;
use types::hooks::{HookInput, HookOutput};

/// Determine the effective memory path that `open_mind_with_path` will use.
///
/// When `MEMVID_PLATFORM_MEMORY_PATH` is set and non-empty, that value takes
/// precedence over the platform-resolved path. This mirrors the precedence
/// logic inside `bootstrap::open_mind_with_path`.
fn effective_memory_path(resolved: PathBuf) -> PathBuf {
    std::env::var("MEMVID_PLATFORM_MEMORY_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .map_or(resolved, PathBuf::from)
}

/// Handle the session-start hook event.
///
/// Runs the event through the adapter + pipeline for contract validation and
/// identity resolution. If the pipeline skips the event, returns a default
/// (no-op) output. Otherwise opens the Mind, fetches context, and returns
/// a system message.
///
/// # Errors
///
/// Returns `HookError::Mind` or `HookError::Platform` on failure.
#[tracing::instrument(skip(input))]
pub fn handle_session_start(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    if !bootstrap::should_process(input, "session_start") {
        return Ok(HookOutput::default());
    }

    let resolved_path = bootstrap::resolve_memory_path(input, cwd)?;
    let mind = bootstrap::open_mind_with_path(resolved_path.clone())?;
    let memory_path = effective_memory_path(resolved_path);

    // Get context and stats
    let ctx = mind.get_context(None)?;
    let stats = mind.stats()?;

    // Format system message
    let mut message = format_system_message(&ctx, &stats, &memory_path);

    // Check for legacy memory paths and emit diagnostics
    for diag in bootstrap::detect_legacy_paths(cwd) {
        let label = match diag.level {
            bootstrap::DiagnosticLevel::Warning => "Warning",
            bootstrap::DiagnosticLevel::Info => "Info",
        };
        let _ = write!(message, "\n**{label}:** {}\n", diag.message);
    }

    Ok(HookOutput {
        system_message: Some(message),
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
            "hook_event_name": "SessionStart"
        }))
        .expect("valid HookInput JSON")
    }

    // -----------------------------------------------------------------------
    // effective_memory_path
    // -----------------------------------------------------------------------

    #[test]
    fn effective_memory_path_returns_resolved_when_env_not_set() {
        let resolved = PathBuf::from("/tmp/resolved/mind.mv2");
        temp_env::with_var("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>, || {
            let result = effective_memory_path(resolved.clone());
            assert_eq!(
                result, resolved,
                "should return resolved path when env is not set"
            );
        });
    }

    #[test]
    fn effective_memory_path_returns_env_override_when_set() {
        let resolved = PathBuf::from("/tmp/resolved/mind.mv2");
        let override_path = "/custom/override/mind.mv2";
        temp_env::with_var("MEMVID_PLATFORM_MEMORY_PATH", Some(override_path), || {
            let result = effective_memory_path(resolved);
            assert_eq!(
                result,
                PathBuf::from(override_path),
                "should return env override path when set"
            );
        });
    }

    // -----------------------------------------------------------------------
    // handle_session_start — requires Mind (memvid), so #[ignore]
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "requires memvid runtime (Mind::open needs valid .mv2 file)"]
    fn handle_session_start_returns_system_message() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());
        let result = handle_session_start(&input);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.system_message.is_some());
    }

    #[test]
    fn handle_session_start_errors_for_nonexistent_path() {
        let input = make_input("/nonexistent/path");
        let result = handle_session_start(&input);
        // Mind::open fails for nonexistent paths, so expect an error
        assert!(
            result.is_err(),
            "should error for nonexistent cwd: {result:?}"
        );
    }
}
