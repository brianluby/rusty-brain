//! Cross-platform integration tests: Claude and OpenCode adapters produce
//! structurally equivalent events that both pass pipeline validation.

use platforms::{EventPipeline, claude_adapter, opencode_adapter};
use types::{EventKind, HookInput, IdentitySource};

// -------------------------------------------------------------------------
// Helper: build a HookInput via JSON (works around #[non_exhaustive])
// -------------------------------------------------------------------------

fn make_hook_input(
    hook_event_name: &str,
    tool_name: Option<&str>,
    platform: &str,
    cwd: &str,
    session_id: &str,
) -> HookInput {
    let tool_name_json = match tool_name {
        Some(name) => format!(r#""tool_name": "{name}","#),
        None => String::new(),
    };
    let json = format!(
        r#"{{
            "session_id": "{session_id}",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "{cwd}",
            "permission_mode": "default",
            "hook_event_name": "{hook_event_name}",
            {tool_name_json}
            "platform": "{platform}"
        }}"#
    );
    serde_json::from_str(&json).expect("test HookInput JSON must parse")
}

// -------------------------------------------------------------------------
// T034: Claude -> normalize -> pipeline vs OpenCode -> normalize -> pipeline
// -------------------------------------------------------------------------

#[test]
fn both_platforms_session_start_pass_pipeline() {
    let pipeline = EventPipeline::new();

    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("SessionStart", None, "claude", "/home/user/proj", "sess-1");
    let o_input = make_hook_input(
        "session_start",
        None,
        "opencode",
        "/home/user/proj",
        "sess-1",
    );

    let c_event = claude.normalize(&c_input, "SessionStart").unwrap();
    let o_event = opencode.normalize(&o_input, "session_start").unwrap();

    let c_result = pipeline.process(&c_event);
    let o_result = pipeline.process(&o_event);

    assert!(!c_result.skipped, "claude SessionStart must pass");
    assert!(!o_result.skipped, "opencode session_start must pass");
}

#[test]
fn both_platforms_tool_observation_pass_pipeline() {
    let pipeline = EventPipeline::new();

    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("PostToolUse", Some("Read"), "claude", "/project", "sess-2");
    let o_input = make_hook_input(
        "tool_observation",
        Some("Read"),
        "opencode",
        "/project",
        "sess-2",
    );

    let c_event = claude.normalize(&c_input, "PostToolUse").unwrap();
    let o_event = opencode.normalize(&o_input, "tool_observation").unwrap();

    let c_result = pipeline.process(&c_event);
    let o_result = pipeline.process(&o_event);

    assert!(!c_result.skipped, "claude PostToolUse must pass");
    assert!(!o_result.skipped, "opencode tool_observation must pass");
}

#[test]
fn both_platforms_produce_same_event_kind_for_session_start() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("SessionStart", None, "claude", "/tmp", "sess-3");
    let o_input = make_hook_input("session_start", None, "opencode", "/tmp", "sess-3");

    let c_event = claude.normalize(&c_input, "SessionStart").unwrap();
    let o_event = opencode.normalize(&o_input, "session_start").unwrap();

    assert_eq!(c_event.kind, EventKind::SessionStart);
    assert_eq!(o_event.kind, EventKind::SessionStart);
}

#[test]
fn both_platforms_produce_same_event_kind_for_tool_observation() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("PostToolUse", Some("Bash"), "claude", "/tmp", "sess-4");
    let o_input = make_hook_input(
        "tool_observation",
        Some("Bash"),
        "opencode",
        "/tmp",
        "sess-4",
    );

    let c_event = claude.normalize(&c_input, "PostToolUse").unwrap();
    let o_event = opencode.normalize(&o_input, "tool_observation").unwrap();

    assert_eq!(
        c_event.kind,
        EventKind::ToolObservation {
            tool_name: "Bash".to_string()
        }
    );
    assert_eq!(c_event.kind, o_event.kind);
}

#[test]
fn both_platforms_produce_same_event_kind_for_session_stop() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("Stop", None, "claude", "/tmp", "sess-5");
    let o_input = make_hook_input("session_stop", None, "opencode", "/tmp", "sess-5");

    let c_event = claude.normalize(&c_input, "Stop").unwrap();
    let o_event = opencode.normalize(&o_input, "session_stop").unwrap();

    assert_eq!(c_event.kind, EventKind::SessionStop);
    assert_eq!(o_event.kind, EventKind::SessionStop);
}

#[test]
fn both_platforms_share_same_contract_version() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    assert_eq!(claude.contract_version(), opencode.contract_version());
    assert_eq!(claude.contract_version(), "1.0.0");
}

#[test]
fn platform_names_differ() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    assert_eq!(claude.platform_name(), "claude");
    assert_eq!(opencode.platform_name(), "opencode");
    assert_ne!(claude.platform_name(), opencode.platform_name());
}

#[test]
fn both_platforms_resolve_same_cwd_identity() {
    let pipeline = EventPipeline::new();

    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let cwd = "/home/shared/project";
    let c_input = make_hook_input("SessionStart", None, "claude", cwd, "sess-6");
    let o_input = make_hook_input("session_start", None, "opencode", cwd, "sess-6");

    let c_event = claude.normalize(&c_input, "SessionStart").unwrap();
    let o_event = opencode.normalize(&o_input, "session_start").unwrap();

    let c_result = pipeline.process(&c_event);
    let o_result = pipeline.process(&o_event);

    let c_identity = c_result.identity.expect("claude identity");
    let o_identity = o_result.identity.expect("opencode identity");

    assert_eq!(
        c_identity.key, o_identity.key,
        "same cwd must resolve to same key"
    );
    assert_eq!(c_identity.source, IdentitySource::Cwd);
    assert_eq!(o_identity.source, IdentitySource::Cwd);
}

#[test]
fn both_platforms_carry_same_session_id() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("SessionStart", None, "claude", "/tmp", "shared-session");
    let o_input = make_hook_input("session_start", None, "opencode", "/tmp", "shared-session");

    let c_event = claude.normalize(&c_input, "SessionStart").unwrap();
    let o_event = opencode.normalize(&o_input, "session_start").unwrap();

    assert_eq!(c_event.session_id, o_event.session_id);
    assert_eq!(c_event.session_id, "shared-session");
}

#[test]
fn both_platforms_populate_project_context_cwd() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("SessionStart", None, "claude", "/workspace", "sess-7");
    let o_input = make_hook_input("session_start", None, "opencode", "/workspace", "sess-7");

    let c_event = claude.normalize(&c_input, "SessionStart").unwrap();
    let o_event = opencode.normalize(&o_input, "session_start").unwrap();

    assert_eq!(c_event.project_context.cwd, Some("/workspace".to_string()));
    assert_eq!(c_event.project_context.cwd, o_event.project_context.cwd);
}

#[test]
fn both_platforms_reject_empty_session_id() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_json = r#"{
        "session_id": "",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/tmp",
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "platform": "claude"
    }"#;
    let o_json = r#"{
        "session_id": "",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/tmp",
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "platform": "opencode"
    }"#;

    let c_input: HookInput = serde_json::from_str(c_json).unwrap();
    let o_input: HookInput = serde_json::from_str(o_json).unwrap();

    assert!(
        claude.normalize(&c_input, "SessionStart").is_none(),
        "claude must reject empty session_id"
    );
    assert!(
        opencode.normalize(&o_input, "session_start").is_none(),
        "opencode must reject empty session_id"
    );
}

#[test]
fn both_platforms_reject_tool_event_without_tool_name() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("PostToolUse", None, "claude", "/tmp", "sess-8");
    let o_input = make_hook_input("tool_observation", None, "opencode", "/tmp", "sess-8");

    assert!(
        claude.normalize(&c_input, "PostToolUse").is_none(),
        "claude must reject PostToolUse without tool_name"
    );
    assert!(
        opencode.normalize(&o_input, "tool_observation").is_none(),
        "opencode must reject tool_observation without tool_name"
    );
}

#[test]
fn events_have_different_platforms_but_same_structure() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let c_input = make_hook_input("PostToolUse", Some("Grep"), "claude", "/home/dev", "sess-9");
    let o_input = make_hook_input(
        "PostToolUse",
        Some("Grep"),
        "opencode",
        "/home/dev",
        "sess-9",
    );

    let c_event = claude.normalize(&c_input, "PostToolUse").unwrap();
    let o_event = opencode.normalize(&o_input, "PostToolUse").unwrap();

    // Platform names differ
    assert_eq!(c_event.platform, "claude");
    assert_eq!(o_event.platform, "opencode");

    // Structural fields match
    assert_eq!(c_event.contract_version, o_event.contract_version);
    assert_eq!(c_event.session_id, o_event.session_id);
    assert_eq!(c_event.kind, o_event.kind);
    assert_eq!(c_event.project_context.cwd, o_event.project_context.cwd);
    assert_eq!(
        c_event.project_context.platform_project_id,
        o_event.project_context.platform_project_id
    );
    assert_eq!(
        c_event.project_context.canonical_path,
        o_event.project_context.canonical_path
    );
}
