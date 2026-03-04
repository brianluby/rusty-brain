use std::path::Path;

use crate::bootstrap;
use crate::error::HookError;
use crate::git::detect_modified_files;
use types::ObservationType;
use types::hooks::{HookInput, HookOutput};

/// Handle the stop hook event.
///
/// Detects modified files via git, stores each as a separate observation,
/// generates and stores a session summary, and returns a system message.
///
/// # Errors
///
/// Returns `HookError::Platform` (memory-path resolution) or `HookError::Mind` on failure.
/// Git detection is fail-open and does not produce errors.
pub fn handle_stop(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);

    if !bootstrap::should_process(input, "stop") {
        return Ok(HookOutput::default());
    }

    // Detect modified files (returns empty Vec on any error)
    let modified_files = detect_modified_files(cwd);

    let mind = bootstrap::open_mind(input, cwd)?;

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

    // Store observations and summary under one lock
    mind.with_lock(|m| {
        for file in &modified_files {
            let summary = format!("Modified file: {file}");
            if let Err(e) = m.remember(
                ObservationType::Feature,
                "session_stop",
                &summary,
                None,
                None,
            ) {
                tracing::warn!("Failed to store file observation for '{file}': {e}");
            }
        }

        m.save_session_summary(decisions, modified_files.clone(), &summary_text)?;
        Ok(())
    })?;

    Ok(HookOutput {
        system_message: Some(summary_text),
        ..Default::default()
    })
}
