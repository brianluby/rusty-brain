use std::path::Path;

use crate::context::format_system_message;
use crate::error::HookError;
use types::hooks::{HookInput, HookOutput};
use types::{MindConfig, ProjectContext};

const LEGACY_MEMORY_PATH: &str = ".claude/mind.mv2";

/// Handle the session-start hook event.
///
/// Detects the platform, resolves project identity and memory path,
/// opens or creates the Mind, fetches context, and returns a system message.
///
/// # Errors
///
/// Returns `HookError::Mind` or `HookError::Platform` on failure.
pub fn handle_session_start(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    // Detect platform
    let platform_name = platforms::detect_platform(input);

    // Resolve project identity
    let context = ProjectContext {
        platform_project_id: None,
        canonical_path: None,
        cwd: Some(input.cwd.clone()),
    };
    let _identity = platforms::resolve_project_identity(&context);

    // Resolve memory path
    let resolved = platforms::resolve_memory_path(cwd, &platform_name, false).map_err(|e| {
        HookError::Platform {
            message: format!("Failed to resolve memory path: {e}"),
        }
    })?;

    // Build MindConfig
    let config = MindConfig {
        memory_path: resolved.path.clone(),
        ..MindConfig::default()
    };

    // Open Mind (creates new .mv2 if missing)
    let mind = rusty_brain_core::mind::Mind::open(config)?;

    // Get context and stats
    let ctx = mind.get_context(None)?;
    let stats = mind.stats()?;

    // Format system message
    let mut message = format_system_message(&ctx, &stats, &resolved.path);

    // Check for legacy memory path
    let legacy_path = cwd.join(LEGACY_MEMORY_PATH);
    if legacy_path.exists() {
        use std::fmt::Write;
        let _ = write!(
            message,
            "\n**Note:** Legacy memory file detected at `{LEGACY_MEMORY_PATH}`. \
             Consider migrating to `.agent-brain/mind.mv2`.\n"
        );
    }

    Ok(HookOutput {
        system_message: Some(message),
        ..Default::default()
    })
}
