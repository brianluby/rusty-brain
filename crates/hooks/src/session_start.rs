use std::path::Path;

use crate::bootstrap;
use crate::context::format_system_message;
use crate::error::HookError;
use types::hooks::{HookInput, HookOutput};

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
pub fn handle_session_start(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    if !bootstrap::should_process(input, "session_start") {
        return Ok(HookOutput::default());
    }

    let memory_path = bootstrap::resolve_memory_path(input, cwd)?;
    let mind = bootstrap::open_mind_with_path(memory_path.clone())?;

    // Get context and stats
    let ctx = mind.get_context(None)?;
    let stats = mind.stats()?;

    // Format system message
    let mut message = format_system_message(&ctx, &stats, &memory_path);

    // Check for legacy memory path (constant and message owned by path_policy)
    let legacy_path = cwd.join(platforms::LEGACY_CLAUDE_MEMORY_PATH);
    if legacy_path.exists() {
        message.push_str(&platforms::format_legacy_path_warning(&memory_path));
    }

    Ok(HookOutput {
        system_message: Some(message),
        ..Default::default()
    })
}
