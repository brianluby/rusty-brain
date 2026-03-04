//! Chat hook handler for context injection (US1).
//!
//! Intercepts `OpenCode` conversations, retrieves relevant context from memory
//! (recent observations, session summaries, topic-relevant memories via
//! `Mind::get_context`), and injects it as `system_message` + structured
//! `hook_specific_output`.

use std::fmt::Write as _;
use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::{HookInput, HookOutput, InjectedContext, RustyBrainError};

use crate::bootstrap;

/// Process a chat message event from `OpenCode`.
///
/// Resolves the memory path, opens the Mind, retrieves context
/// via `Mind::get_context(query)`, and returns a `HookOutput` with:
/// - `system_message`: formatted memory context (human-readable)
/// - `hook_specific_output`: structured `InjectedContext` JSON
///
/// If no memory file exists, `Mind::open()` auto-creates it (AC-2).
/// On error: caller wraps in fail-open returning `HookOutput::default()`.
///
/// # Errors
///
/// Returns `RustyBrainError` if memory path resolution, Mind opening,
/// or context retrieval fails.
pub fn handle_chat_hook(input: &HookInput, cwd: &Path) -> Result<HookOutput, RustyBrainError> {
    if !bootstrap::should_process(input, "session_start") {
        return Ok(HookOutput::default());
    }

    let mind = bootstrap::open_mind_read_write(cwd)?;
    let query = input.prompt.as_deref();

    let ctx = mind.with_lock(|m: &Mind| m.get_context(query))?;

    let system_message = format_system_message(&ctx, cwd);
    let hook_specific = serde_json::to_value(&ctx).ok();

    Ok(HookOutput {
        system_message: Some(system_message),
        hook_specific_output: hook_specific,
        ..Default::default()
    })
}

/// Format the `InjectedContext` into a human-readable system message.
///
/// Format per research.md R6.
fn format_system_message(ctx: &InjectedContext, cwd: &Path) -> String {
    let mut msg = String::from("# Memory Context\n");

    if !ctx.recent_observations.is_empty() {
        msg.push_str("\n## Recent Observations\n");
        for obs in &ctx.recent_observations {
            let ts = obs.timestamp.format("%Y-%m-%d %H:%M");
            let _ = writeln!(
                msg,
                "- [{}] ({ts}) {}: {}",
                obs.obs_type, obs.tool_name, obs.summary
            );
        }
    }

    if !ctx.relevant_memories.is_empty() {
        msg.push_str("\n## Relevant Memories\n");
        for obs in &ctx.relevant_memories {
            let ts = obs.timestamp.format("%Y-%m-%d %H:%M");
            let _ = writeln!(
                msg,
                "- [{}] ({ts}) {}: {}",
                obs.obs_type, obs.tool_name, obs.summary
            );
        }
    }

    if !ctx.session_summaries.is_empty() {
        msg.push_str("\n## Session Summaries\n");
        for summary in &ctx.session_summaries {
            let date = summary.start_time.format("%Y-%m-%d");
            let _ = writeln!(
                msg,
                "- Session {date}: \"{}\" ({} observations)",
                summary.summary, summary.observation_count
            );
        }
    }

    let project_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let _ = write!(msg, "\nProject: **{project_name}**\n");

    msg
}
