use std::path::Path;

use crate::error::HookError;
use crate::git::detect_modified_files;
use types::hooks::{HookInput, HookOutput};
use types::{MindConfig, ObservationType};

/// Handle the stop hook event.
///
/// Detects modified files via git, stores each as a separate observation,
/// generates and stores a session summary, and returns a system message.
///
/// # Errors
///
/// Returns `HookError::Mind` or `HookError::Git` on failure.
pub fn handle_stop(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    // Detect modified files (returns empty Vec on any error)
    let modified_files = detect_modified_files(cwd);

    // Resolve memory path and open Mind
    let platform_name = platforms::detect_platform(input);
    let resolved = platforms::resolve_memory_path(cwd, &platform_name, false).map_err(|e| {
        HookError::Platform {
            message: format!("Failed to resolve memory path: {e}"),
        }
    })?;

    let config = MindConfig {
        memory_path: resolved.path.clone(),
        ..MindConfig::default()
    };

    let mind = rusty_brain_core::mind::Mind::open(config)?;

    // Store each modified file as a separate Feature observation
    for file in &modified_files {
        let summary = format!("Modified file: {file}");
        let _ = mind.remember(
            ObservationType::Feature,
            "session_stop",
            &summary,
            None,
            None,
        );
    }

    // Build session summary text
    let summary_text = if modified_files.is_empty() {
        "Session ended with no file modifications.".to_string()
    } else {
        format!(
            "Session ended. Modified {} file(s): {}",
            modified_files.len(),
            modified_files.join(", ")
        )
    };

    // Decisions are empty for MVP (decision extraction deferred)
    let decisions: Vec<String> = Vec::new();

    // Save session summary
    mind.save_session_summary(decisions, modified_files.clone(), &summary_text)?;

    Ok(HookOutput {
        system_message: Some(summary_text),
        ..Default::default()
    })
}
