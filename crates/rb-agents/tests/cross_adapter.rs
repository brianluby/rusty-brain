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
        // OpenCode reports lowercase tool names verbatim; the adapter preserves
        // the CLI-native casing (capture-time normalization handles the mapping).
        AgentId::OpenCode => serde_json::json!({
            "type": "tool.execute.after",
            "directory": "/proj",
            "sessionID": "s",
            "tool": "write",
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
fn all_four_adapters_normalize_post_tool_use_event_with_cli_native_tool_name() {
    // Every dialect maps its post-tool event to canonical `PostToolUse`. The
    // adapter preserves the CLI's *native* tool-name casing verbatim (Claude/
    // Gemini/Codex capitalize `Write`; OpenCode reports lowercase `write`);
    // case-folding to a single canonical name is the capture layer's job.
    let cases = [
        (AgentId::ClaudeCode, "Write"),
        (AgentId::OpenCode, "write"),
        (AgentId::Gemini, "Write"),
        (AgentId::Codex, "Write"),
    ];
    for (id, expected) in cases {
        let cli = agent_for(id);
        let raw = post_tool_payload(id);
        let ctx = cli.parse_input(&raw);
        match ctx.event {
            HookEvent::PostToolUse { tool_name, .. } => {
                assert_eq!(tool_name, expected, "tool_name mismatch for {id:?}");
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
            ..HookResult::default()
        });
        assert_eq!(
            out["continue"],
            serde_json::json!(true),
            "continue flag missing/false for {id:?}"
        );
    }
}
