//! Integration tests for full event normalization flow through the pipeline.
//!
//! Tests the end-to-end path: HookInput -> adapter.normalize() -> EventPipeline.process()
//! verifying that events produced by adapters pass pipeline validation.

use chrono::Utc;
use platforms::{EventPipeline, PipelineResult, claude_adapter, opencode_adapter};
use types::{EventKind, HookInput, IdentitySource, PlatformEvent, ProjectContext};
use uuid::Uuid;

// -------------------------------------------------------------------------
// Helper: build a HookInput via JSON (works around #[non_exhaustive])
// -------------------------------------------------------------------------

fn make_hook_input(
    hook_event_name: &str,
    tool_name: Option<&str>,
    platform: &str,
    cwd: &str,
) -> HookInput {
    let tool_name_json = match tool_name {
        Some(name) => format!(r#""tool_name": "{name}","#),
        None => String::new(),
    };
    let json = format!(
        r#"{{
            "session_id": "integration-session-001",
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
// T033: Full pipeline integration — adapter normalize + pipeline process
// -------------------------------------------------------------------------

#[test]
fn claude_session_start_passes_pipeline() {
    let adapter = claude_adapter();
    let input = make_hook_input("SessionStart", None, "claude", "/home/user/project");
    let event = adapter
        .normalize(&input, "SessionStart")
        .expect("SessionStart must normalize");

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(
        !result.skipped,
        "valid claude SessionStart must not be skipped"
    );
    assert!(result.identity.is_some());
    assert_eq!(result.reason, None);
    assert_eq!(result.diagnostic, None);
}

#[test]
fn claude_tool_observation_passes_pipeline() {
    let adapter = claude_adapter();
    let input = make_hook_input("PostToolUse", Some("Read"), "claude", "/home/user/project");
    let event = adapter
        .normalize(&input, "PostToolUse")
        .expect("PostToolUse must normalize");

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(
        !result.skipped,
        "valid claude PostToolUse must not be skipped"
    );
    let identity = result.identity.expect("identity must be present");
    assert_eq!(identity.key, Some("/home/user/project".to_string()));
    assert_eq!(identity.source, IdentitySource::Cwd);
}

#[test]
fn claude_session_stop_passes_pipeline() {
    let adapter = claude_adapter();
    let input = make_hook_input("Stop", None, "claude", "/tmp/work");
    let event = adapter
        .normalize(&input, "Stop")
        .expect("Stop must normalize");

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(!result.skipped, "valid claude Stop must not be skipped");
}

#[test]
fn opencode_session_start_passes_pipeline() {
    let adapter = opencode_adapter();
    let input = make_hook_input("session_start", None, "opencode", "/home/user/project");
    let event = adapter
        .normalize(&input, "session_start")
        .expect("session_start must normalize");

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(
        !result.skipped,
        "valid opencode session_start must not be skipped"
    );
    assert!(result.identity.is_some());
}

#[test]
fn opencode_tool_observation_passes_pipeline() {
    let adapter = opencode_adapter();
    let input = make_hook_input("tool_observation", Some("Write"), "opencode", "/tmp/code");
    let event = adapter
        .normalize(&input, "tool_observation")
        .expect("tool_observation must normalize");

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(
        !result.skipped,
        "valid opencode tool_observation must not be skipped"
    );
    let identity = result.identity.expect("identity must be present");
    assert_eq!(identity.key, Some("/tmp/code".to_string()));
}

#[test]
fn adapter_events_carry_valid_contract_version() {
    let claude = claude_adapter();
    let opencode = opencode_adapter();

    let input = make_hook_input("SessionStart", None, "claude", "/tmp");
    let claude_event = claude.normalize(&input, "SessionStart").unwrap();

    let input = make_hook_input("SessionStart", None, "opencode", "/tmp");
    let oc_event = opencode.normalize(&input, "SessionStart").unwrap();

    // Both must carry "1.0.0"
    assert_eq!(claude_event.contract_version, "1.0.0");
    assert_eq!(oc_event.contract_version, "1.0.0");

    // Both must pass contract validation in the pipeline
    let pipeline = EventPipeline::new();
    assert!(!pipeline.process(&claude_event).skipped);
    assert!(!pipeline.process(&oc_event).skipped);
}

#[test]
fn pipeline_skips_event_with_altered_contract_version() {
    let adapter = claude_adapter();
    let input = make_hook_input("SessionStart", None, "claude", "/tmp/project");
    let mut event = adapter.normalize(&input, "SessionStart").unwrap();

    // Tamper with the contract version
    event.contract_version = "2.0.0".to_string();

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(result.skipped, "altered contract version must be skipped");
    assert_eq!(
        result.reason.as_deref(),
        Some("incompatible_contract_major")
    );
}

#[test]
fn pipeline_skips_event_without_cwd_identity() {
    // Manually build an event with no identity fields
    let event = PlatformEvent {
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        platform: "claude".to_string(),
        contract_version: "1.0.0".to_string(),
        session_id: "test-session".to_string(),
        project_context: ProjectContext {
            platform_project_id: None,
            canonical_path: None,
            cwd: None,
        },
        kind: EventKind::SessionStart,
    };

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(result.skipped, "event without identity must be skipped");
    assert_eq!(result.reason.as_deref(), Some("missing_project_identity"));
}

#[test]
fn pipeline_result_fields_are_consistent_on_success() {
    let adapter = claude_adapter();
    let input = make_hook_input("SessionStart", None, "claude", "/home/dev");
    let event = adapter.normalize(&input, "SessionStart").unwrap();

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    assert!(!result.skipped);
    assert!(result.identity.is_some());
    assert!(result.reason.is_none());
    assert!(result.diagnostic.is_none());
}

#[test]
fn pipeline_result_serde_round_trip_for_adapter_event() {
    let adapter = claude_adapter();
    let input = make_hook_input("SessionStart", None, "claude", "/home/dev/project");
    let event = adapter.normalize(&input, "SessionStart").unwrap();

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);

    let json = serde_json::to_string(&result).expect("serialization must succeed");
    let deserialized: PipelineResult =
        serde_json::from_str(&json).expect("deserialization must succeed");

    assert_eq!(result, deserialized);
}

#[test]
fn multiple_events_processed_independently() {
    let adapter = claude_adapter();
    let pipeline = EventPipeline::new();

    // Event 1: valid
    let input1 = make_hook_input("SessionStart", None, "claude", "/project1");
    let event1 = adapter.normalize(&input1, "SessionStart").unwrap();
    let result1 = pipeline.process(&event1);

    // Event 2: valid, different cwd
    let input2 = make_hook_input("PostToolUse", Some("Bash"), "claude", "/project2");
    let event2 = adapter.normalize(&input2, "PostToolUse").unwrap();
    let result2 = pipeline.process(&event2);

    assert!(!result1.skipped);
    assert!(!result2.skipped);
    assert_ne!(
        result1.identity.unwrap().key,
        result2.identity.unwrap().key,
        "different cwds must resolve to different identities"
    );
}
