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
        // Codex's file-edit tool is `apply_patch`, carrying the raw V4A patch
        // under `tool_input.command` (NO `file_path`). This mirrors the REAL
        // payload live-captured from codex-cli 0.144.1 (see
        // rb-hooks/tests/fixtures/codex/post_tool_use_apply_patch.json) —
        // Codex fires PostToolUse for apply_patch since 0.123.0
        // (openai/codex#16732).
        AgentId::Codex => serde_json::json!({
            "hook_event_name": "PostToolUse",
            "cwd": "/proj",
            "session_id": "s",
            "tool_name": "apply_patch",
            "tool_input": {"command": "*** Begin Patch\n*** Add File: notes.txt\n+recorded.\n*** End Patch\n"},
            "tool_response": "Exit code: 0\nOutput:\nSuccess. Updated the following files:\nA notes.txt\n"
        }),
    }
}

#[test]
fn all_four_adapters_normalize_post_tool_use_event_with_cli_native_tool_name() {
    // Every dialect maps its post-tool event to canonical `PostToolUse`. The
    // adapter preserves the CLI's *native* tool-name spelling verbatim
    // (Claude/Gemini capitalize `Write`; OpenCode reports lowercase `write`;
    // Codex reports `apply_patch` for file edits); normalizing to a single
    // canonical name is the capture layer's job.
    let cases = [
        (AgentId::ClaudeCode, "Write"),
        (AgentId::OpenCode, "write"),
        (AgentId::Gemini, "Write"),
        (AgentId::Codex, "apply_patch"),
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

#[test]
fn fixture_gated_boundary_events_stay_stop_until_verified() {
    // Codex (`Stop`) and Gemini (`SessionEnd`) have no recorded lifecycle fixture
    // proving their terminus cadence yet, so their boundary stays canonical Stop
    // (stores nothing) rather than guessing a checkpoint/terminus mapping.
    // OpenCode is NO LONGER here: its `session.idle` FOLD is fixture-backed by a
    // green lifecycle test (see `opencode_session_idle_maps_to_checkpoint_not_stop`).
    let cases = [
        (
            AgentId::Gemini,
            serde_json::json!({
                "hook_event_name": "SessionEnd",
                "cwd": "/proj",
                "session_id": "s"
            }),
            "SessionEnd",
        ),
        (
            AgentId::Codex,
            serde_json::json!({
                "hook_event_name": "Stop",
                "cwd": "/proj",
                "session_id": "s"
            }),
            "Stop",
        ),
    ];
    for (id, raw, reason) in cases {
        let cli = agent_for(id);
        let ctx = cli.parse_input(&raw);
        assert_eq!(
            ctx.event,
            HookEvent::Stop {
                last_assistant_message: None,
                stop_hook_active: false,
            },
            "{id:?} boundary {reason:?} must not checkpoint until a real fixture verifies it"
        );
    }
}

#[test]
fn opencode_session_idle_maps_to_checkpoint_not_stop() {
    // Gap A: OpenCode's `session.idle` is the fold event. Its FOLD behavior is
    // fixture-backed (a green lifecycle test); its firing cadence is ambiguous
    // (one-run terminus.json: fired=2, verdict `ambiguous`), and SessionCheckpoint
    // is multi-fire-safe for ANY count >= 1 — so it maps to canonical
    // SessionCheckpoint, NOT Stop, restoring capture that Stop dropped. This is the
    // worked example the Codex / Gemini terminus decisions follow once their
    // fixtures land.
    let cli = agent_for(AgentId::OpenCode);
    let ctx = cli.parse_input(&serde_json::json!({
        "type": "session.idle",
        "directory": "/proj",
        "sessionID": "s"
    }));
    assert_eq!(ctx.event, HookEvent::SessionCheckpoint { reason: None });
}
