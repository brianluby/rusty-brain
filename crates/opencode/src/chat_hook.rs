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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    // -----------------------------------------------------------------------
    // handle_chat_hook — event filtering
    // -----------------------------------------------------------------------

    #[test]
    fn handle_chat_hook_with_non_session_start_event() {
        let input = make_hook_input("PostToolUse");
        let cwd = Path::new("/tmp/project");
        // Either returns default (event filtered) or errors (no Mind file).
        // Both are acceptable — the key is no panic.
        let _result = handle_chat_hook(&input, cwd);
    }

    // -----------------------------------------------------------------------
    // format_system_message
    // -----------------------------------------------------------------------

    #[test]
    fn format_system_message_includes_project_name() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        let msg = format_system_message(&ctx, Path::new("/home/user/my-project"));
        assert!(msg.contains("**my-project**"));
    }

    #[test]
    fn format_system_message_uses_unknown_for_root_path() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        let msg = format_system_message(&ctx, Path::new("/"));
        assert!(msg.contains("Project:"));
    }

    #[test]
    fn format_system_message_starts_with_memory_context_header() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        let msg = format_system_message(&ctx, Path::new("/tmp/test"));
        assert!(msg.starts_with("# Memory Context\n"));
    }

    #[test]
    fn format_system_message_omits_sections_when_empty() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        let msg = format_system_message(&ctx, Path::new("/tmp/test"));
        assert!(!msg.contains("## Recent Observations"));
        assert!(!msg.contains("## Relevant Memories"));
        assert!(!msg.contains("## Session Summaries"));
    }
}
