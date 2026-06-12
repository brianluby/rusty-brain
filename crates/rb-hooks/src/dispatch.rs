//! Route a `HookContext` event to the matching capture flow. `Other` events are
//! a no-op (continue). Every flow returns `continue_execution: true`.

use rb_agents::DaemonClient;
use rb_agents::{HookContext, HookEvent, HookResult};

use crate::capture;
use crate::dedup::DedupCache;

/// Dispatch one parsed hook context to its capture flow. The optional client is
/// the (best-effort) daemon connection; `None` means degraded — flows still
/// return `continue_execution: true`.
pub async fn dispatch(
    mut client: Option<&mut DaemonClient>,
    dedup: &DedupCache,
    ctx: &HookContext,
) -> HookResult {
    match &ctx.event {
        HookEvent::SessionStart { .. } => capture::session_start(client.take()).await,
        HookEvent::PostToolUse {
            tool_name,
            tool_input,
            tool_response,
        } => {
            capture::post_tool_use(client.take(), dedup, tool_name, tool_input, tool_response).await
        }
        HookEvent::Stop { .. } => capture::stop(client.take(), &ctx.cwd).await,
        HookEvent::PreCompact {
            custom_instructions,
        } => capture::pre_compact(client.take(), custom_instructions.as_deref()).await,
        HookEvent::Other(_) => HookResult {
            system_message: None,
            continue_execution: true,
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::{HookContext, HookEvent};
    use std::path::PathBuf;

    fn ctx(event: HookEvent) -> HookContext {
        HookContext {
            event,
            cwd: PathBuf::from("/tmp"),
            session_id: Some("s1".to_string()),
            transcript_path: None,
        }
    }

    #[tokio::test]
    async fn other_event_is_noop_continue() {
        let tmp = tempfile::tempdir().unwrap();
        let dedup = DedupCache::at(tmp.path().join("d.json"));
        let result = dispatch(None, &dedup, &ctx(HookEvent::Other("Notification".into()))).await;
        assert!(result.continue_execution);
        assert!(result.system_message.is_none());
    }

    #[tokio::test]
    async fn post_tool_use_event_routes_and_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let dedup = DedupCache::at(tmp.path().join("d.json"));
        let event = HookEvent::PostToolUse {
            tool_name: "Edit".to_string(),
            tool_input: serde_json::json!({"file_path": "/src/main.rs"}),
            tool_response: serde_json::json!("ok"),
        };
        let result = dispatch(None, &dedup, &ctx(event)).await;
        assert!(result.continue_execution);
        // Routed to post_tool_use: the mutation must have been deduped.
        assert!(dedup.is_duplicate("Edit", "Edited /src/main.rs"));
    }

    #[tokio::test]
    async fn stop_event_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let dedup = DedupCache::at(tmp.path().join("d.json"));
        let event = HookEvent::Stop {
            last_assistant_message: Some("done".to_string()),
            stop_hook_active: false,
        };
        let result = dispatch(None, &dedup, &ctx(event)).await;
        assert!(result.continue_execution);
    }
}
