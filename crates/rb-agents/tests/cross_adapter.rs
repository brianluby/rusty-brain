#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Cross-adapter normalization: all four CLI dialects map their
//! post-tool-execution event to the canonical `HookEvent::PostToolUse` with the
//! same `tool_name`.

use rb_agents::{agent_for, AgentId, HookEvent};

/// The representative post-tool payload for one CLI, keyed by `AgentId`.
fn post_tool_payload(id: AgentId) -> serde_json::Value {
    match id {
        AgentId::ClaudeCode => serde_json::json!({
            "session_id": "s",
            "transcript_path": "/t.jsonl",
            "cwd": "/proj",
            "permission_mode": "default",
            "hook_event_name": "PostToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/x"},
            "tool_response": {"ok": true}
        }),
        AgentId::OpenCode => serde_json::json!({
            "type": "tool.execute.after",
            "directory": "/proj",
            "sessionID": "s",
            "tool": "Write",
            "args": {"file_path": "/x"},
            "output": {"ok": true}
        }),
        AgentId::Gemini => serde_json::json!({
            "hook_event_name": "AfterTool",
            "cwd": "/proj",
            "session_id": "s",
            "tool_name": "Write",
            "tool_input": {"file_path": "/x"},
            "tool_response": {"ok": true}
        }),
        AgentId::Codex => serde_json::json!({
            "hook_event_name": "PostToolUse",
            "cwd": "/proj",
            "session_id": "s",
            "tool_name": "Write",
            "tool_input": {"file_path": "/x"},
            "tool_response": {"ok": true}
        }),
    }
}

#[test]
fn all_four_adapters_normalize_post_tool_use_to_same_tool_name() {
    let ids = [
        AgentId::ClaudeCode,
        AgentId::OpenCode,
        AgentId::Gemini,
        AgentId::Codex,
    ];
    for id in ids {
        let cli = agent_for(id);
        let raw = post_tool_payload(id);
        let ctx = cli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse { tool_name, .. } => {
                assert_eq!(tool_name, "Write", "tool_name mismatch for {:?}", id);
            }
            other => panic!("expected PostToolUse for {id:?}, got {other:?}"),
        }
    }
}

#[test]
fn render_output_continue_is_true_for_all_four() {
    use rb_agents::HookResult;
    for id in [
        AgentId::ClaudeCode,
        AgentId::OpenCode,
        AgentId::Gemini,
        AgentId::Codex,
    ] {
        let cli = agent_for(id);
        let out = cli.render_output(&HookResult {
            system_message: None,
            continue_execution: true,
        });
        assert_eq!(
            out["continue"],
            serde_json::json!(true),
            "continue flag missing/false for {id:?}"
        );
    }
}
