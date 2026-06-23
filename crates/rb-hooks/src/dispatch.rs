//! Route a `HookContext` event to the matching capture flow. `Other` events are
//! a no-op (continue). Every flow returns `continue_execution: true`.

use rb_agents::DaemonClient;
use rb_agents::{HookContext, HookEvent, HookResult};

use crate::capture;
use crate::scratch::{self, Scratch};

/// Dispatch one parsed hook context to its capture flow. `client` is the
/// (best-effort) daemon connection — `None` means degraded — and `scratch` is
/// the per-session capture buffer, `None` when the event carries no session id.
/// Every flow returns `continue_execution: true`.
pub async fn dispatch(
    mut client: Option<&mut DaemonClient>,
    scratch: Option<&Scratch>,
    ctx: &HookContext,
) -> HookResult {
    match &ctx.event {
        HookEvent::SessionStart { source } => {
            // Opportunistic hygiene: reclaim scratch files from abandoned /
            // crashed sessions whose SessionEnd never fired (cheap; once per
            // session, off the daemon path).
            scratch::prune_stale();
            // W3.3: the injection is source-aware (startup vs resume vs compact).
            capture::session_start(client.take(), source.as_deref()).await
        }
        HookEvent::PostToolUse {
            tool_name,
            tool_input,
            tool_response,
        } => capture::post_tool_use(scratch, tool_name, tool_input, tool_response).await,
        // W3.1: a per-turn Stop stores nothing; the session fold is at SessionEnd.
        HookEvent::Stop {
            stop_hook_active, ..
        } => capture::stop(*stop_hook_active),
        HookEvent::SessionCheckpoint { .. } => {
            capture::session_checkpoint(
                client.take(),
                scratch,
                &ctx.cwd,
                ctx.transcript_path.as_deref(),
            )
            .await
        }
        HookEvent::SessionEnd { .. } => {
            capture::session_end(
                client.take(),
                scratch,
                &ctx.cwd,
                ctx.transcript_path.as_deref(),
            )
            .await
        }
        HookEvent::PreCompact {
            custom_instructions,
        } => {
            capture::pre_compact(
                client.take(),
                custom_instructions.as_deref(),
                ctx.transcript_path.as_deref(),
            )
            .await
        }
        // W3.2(a): recall memories relevant to the user's prompt and inject them
        // as additionalContext (read-only; deterministic — no model election).
        HookEvent::UserPromptSubmit { prompt } => {
            capture::user_prompt_submit(client.take(), prompt.as_deref()).await
        }
        HookEvent::Other(_) => HookResult::default(),
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
        let result = dispatch(None, None, &ctx(HookEvent::Other("Notification".into()))).await;
        assert!(result.continue_execution);
        assert!(result.system_message.is_none());
    }

    #[tokio::test]
    async fn post_tool_use_event_appends_to_scratch_and_writes_no_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = Scratch::at(tmp.path().join("scratch.json"));
        let event = HookEvent::PostToolUse {
            tool_name: "Edit".to_string(),
            tool_input: serde_json::json!({"file_path": "/src/main.rs"}),
            tool_response: serde_json::json!("ok"),
        };
        // No client passed: PostToolUse never needs the daemon (zero memories).
        let result = dispatch(None, Some(&scratch), &ctx(event)).await;
        assert!(result.continue_execution);
        // Routed to post_tool_use: the file landed in the scratch (not a memory).
        assert_eq!(scratch.read().files, vec!["/src/main.rs"]);
    }

    #[tokio::test]
    async fn stop_event_continues_and_stores_nothing() {
        let event = HookEvent::Stop {
            last_assistant_message: Some("done".to_string()),
            stop_hook_active: false,
        };
        let result = dispatch(None, None, &ctx(event)).await;
        assert!(result.continue_execution);
    }

    #[tokio::test]
    async fn session_end_event_routes_and_preserves_scratch_when_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = Scratch::at(tmp.path().join("scratch.json"));
        scratch.append(scratch::Kind::File, "src/lib.rs");
        // No client (degraded): session_end folds but cannot store, so it
        // PRESERVES the buffer for a retry/resume rather than losing the turn.
        let result = dispatch(
            None,
            Some(&scratch),
            &ctx(HookEvent::SessionEnd { reason: None }),
        )
        .await;
        assert!(result.continue_execution);
        assert_eq!(
            scratch.read().files,
            vec!["src/lib.rs"],
            "a degraded SessionEnd preserves the scratch"
        );
    }

    #[tokio::test]
    async fn session_checkpoint_event_routes_and_preserves_scratch_when_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let scratch = Scratch::at(tmp.path().join("scratch.json"));
        scratch.append(scratch::Kind::File, "src/lib.rs");
        let result = dispatch(
            None,
            Some(&scratch),
            &ctx(HookEvent::SessionCheckpoint {
                reason: Some("Stop".to_string()),
            }),
        )
        .await;
        assert!(result.continue_execution);
        assert_eq!(
            scratch.read().files,
            vec!["src/lib.rs"],
            "a degraded checkpoint preserves the scratch"
        );
    }

    #[tokio::test]
    async fn session_checkpoint_with_none_reason_routes_and_continues() {
        // A checkpoint whose reason is None (the adapter did not provide a
        // diagnostic label) must still route to the checkpoint flow and
        // continue, never block or panic.
        let tmp = tempfile::tempdir().unwrap();
        let scratch = Scratch::at(tmp.path().join("scratch.json"));
        scratch.append(scratch::Kind::Command, "cargo test");
        let result = dispatch(
            None,
            Some(&scratch),
            &ctx(HookEvent::SessionCheckpoint { reason: None }),
        )
        .await;
        assert!(result.continue_execution);
        assert_eq!(
            scratch.read().commands,
            vec!["cargo test"],
            "a degraded None-reason checkpoint preserves the scratch"
        );
    }

    #[tokio::test]
    async fn session_checkpoint_without_scratch_continues() {
        // No scratch means no session id — dispatch must still continue.
        let result = dispatch(
            None,
            None,
            &ctx(HookEvent::SessionCheckpoint {
                reason: Some("Stop".to_string()),
            }),
        )
        .await;
        assert!(result.continue_execution);
        assert!(result.system_message.is_none());
    }
}
