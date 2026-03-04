//! Tool hook handler for observation capture (US2).
//!
//! Captures compressed observations after tool executions, with session-scoped
//! deduplication via sidecar file.

use std::path::Path;

use compression::CompressionConfig;
use rusty_brain_core::mind::Mind;
use types::{HookInput, HookOutput, MindConfig, ObservationType, RustyBrainError};

use crate::sidecar;

/// Process a tool execution event from `OpenCode`.
///
/// 1. Loads sidecar state (or creates fresh state on first invocation)
/// 2. Compresses tool output via `compression::compress()`
/// 3. Computes dedup hash from `tool_name` + compressed summary
/// 4. If duplicate: skips storage, returns success
/// 5. If new: calls `Mind::remember()`, updates sidecar with new hash
///
/// On error: caller wraps in fail-open returning `HookOutput { continue_execution: Some(true) }`.
///
/// # Errors
///
/// Returns `RustyBrainError` if memory path resolution, Mind opening,
/// observation storage, or sidecar persistence fails.
pub fn handle_tool_hook(input: &HookInput, cwd: &Path) -> Result<HookOutput, RustyBrainError> {
    let tool_name = input.tool_name.as_deref().unwrap_or("unknown");
    let tool_response = input
        .tool_response
        .as_ref()
        .map(std::string::ToString::to_string)
        .unwrap_or_default();

    if tool_response.is_empty() {
        return Ok(HookOutput::default());
    }

    // Compress tool output
    let config = CompressionConfig::default();
    let compressed = compression::compress(&config, tool_name, &tool_response, None);
    let summary = &compressed.text;

    if summary.trim().is_empty() {
        return Ok(HookOutput::default());
    }

    // Load or create sidecar state
    let session_id = &input.session_id;
    let sidecar_file = sidecar::sidecar_path(cwd, session_id);

    let state = match sidecar::load(&sidecar_file) {
        Ok(s) => s,
        Err(e) => {
            if !sidecar_file.exists() {
                // First invocation — no sidecar yet
                crate::types::SidecarState::new(session_id.clone())
            } else if matches!(e, RustyBrainError::Serialization { .. }) {
                // Corrupt file — recreate
                tracing::warn!(
                    path = %sidecar_file.display(),
                    "corrupt sidecar file, recreating"
                );
                let _ = std::fs::remove_file(&sidecar_file);
                crate::types::SidecarState::new(session_id.clone())
            } else {
                // I/O or permission error — propagate
                return Err(e);
            }
        }
    };

    // Compute dedup hash
    let hash = sidecar::compute_dedup_hash(tool_name, summary);

    // Check for duplicate
    if sidecar::is_duplicate(&state, &hash) {
        return Ok(HookOutput::default());
    }

    // Store new observation
    let resolved = platforms::resolve_memory_path(cwd, "opencode", false)?;
    let mut mind_config = MindConfig::from_env()?;
    mind_config.memory_path = resolved.path;

    let mind = Mind::open(mind_config)?;
    mind.with_lock(|m: &Mind| {
        m.remember(
            ObservationType::Discovery,
            tool_name,
            summary,
            Some(&tool_response),
            None,
        )
    })?;

    // Update sidecar with new hash (immutable — returns new state)
    let state = sidecar::with_hash(&state, hash);
    sidecar::save(&sidecar_file, &state)?;

    Ok(HookOutput::default())
}
