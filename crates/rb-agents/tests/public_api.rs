//! Locks the rb-agents public API surface so Parts W/X/Y compile against exactly
//! these re-exported names. Pure integration test: imports through the crate
//! root only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rb_agents::{
    agent_capabilities, agent_for, capability_for_agent, detect_namespace, AgentCli, AgentId,
    AgentInstaller, AutoStart, ClaudeCodeCli, CodexCli, DaemonClient, GeminiCli, HookContext,
    HookEvent, HookFragment, HookResult, InstallScope, OpenCodeCli, SupportLevel, SENTINEL,
};

#[test]
fn event_model_types_are_reexported() {
    let ctx = HookContext {
        event: HookEvent::SessionStart { source: None },
        cwd: PathBuf::from("."),
        session_id: None,
        transcript_path: None,
    };
    assert_eq!(ctx.event, HookEvent::SessionStart { source: None });
    let result = HookResult {
        system_message: Some("x".to_string()),
        continue_execution: true,
        ..HookResult::default()
    };
    assert!(result.continue_execution);
}

#[test]
fn registry_and_adapters_are_reexported() {
    let cli: Box<dyn AgentCli> = agent_for(AgentId::ClaudeCode);
    assert_eq!(cli.id(), AgentId::ClaudeCode);
    let gemini: Box<dyn AgentCli> = agent_for(AgentId::Gemini);
    assert_eq!(gemini.binary_name(), "gemini");
    // The four real adapters are part of the public surface.
    let _ = std::any::type_name::<ClaudeCodeCli>();
    let _ = std::any::type_name::<OpenCodeCli>();
    let _ = std::any::type_name::<GeminiCli>();
    let _ = std::any::type_name::<CodexCli>();
}

#[test]
fn capability_matrix_is_reexported() {
    assert!(
        agent_capabilities()
            .iter()
            .any(|capability| capability.agent == "claude-code"),
        "claude-code capability row must be public"
    );
    let codex = capability_for_agent("codex").expect("codex capability row");
    assert_eq!(codex.scorecard, SupportLevel::Unsupported);
}

#[test]
fn adapter_status_variants_are_reexported() {
    use rb_agents::AdapterStatus;
    // Each variant must be constructable through the public path.
    let _stable = AdapterStatus::Stable;
    let _experimental = AdapterStatus::Experimental;
    let _discovery = AdapterStatus::Discovery;
    let _unsupported = AdapterStatus::Unsupported;
    assert_eq!(AdapterStatus::Stable, AdapterStatus::Stable);
    assert_ne!(AdapterStatus::Stable, AdapterStatus::Discovery);
}

#[test]
fn support_level_variants_are_reexported() {
    assert_eq!(SupportLevel::Supported, SupportLevel::Supported);
    assert_ne!(SupportLevel::Supported, SupportLevel::Partial);
    // All four variants accessible through the crate root.
    let _partial = SupportLevel::Partial;
    let _unsupported = SupportLevel::Unsupported;
    let _unknown = SupportLevel::Unknown;
}

#[test]
fn all_five_agents_are_present_in_public_matrix() {
    let agents: Vec<_> = agent_capabilities().iter().map(|c| c.agent).collect();
    for expected in ["claude-code", "codex", "opencode", "gemini", "hermes"] {
        assert!(
            agents.contains(&expected),
            "agent '{}' missing from public capability matrix",
            expected
        );
    }
}

#[test]
fn agent_capability_struct_fields_accessible_via_public_api() {
    use rb_agents::{AdapterStatus, AgentCapability};
    let cap: &AgentCapability = capability_for_agent("hermes").expect("hermes row");
    // Verify all public fields are accessible by touching them.
    let _agent: &str = cap.agent;
    let _adapter: AdapterStatus = cap.adapter_status;
    let _capture: SupportLevel = cap.capture;
    let _retrieval: SupportLevel = cap.retrieval;
    let _config: SupportLevel = cap.config;
    let _scorecard: SupportLevel = cap.scorecard;
    let _source: &str = cap.verified_lifecycle_source;
    let _limitations: &[&str] = cap.limitations;
    assert!(
        !_source.is_empty(),
        "verified_lifecycle_source must not be empty"
    );
    assert!(!_limitations.is_empty(), "limitations must not be empty");
}

#[test]
fn capability_for_agent_returns_none_for_unknown_name() {
    assert!(capability_for_agent("nonexistent-agent").is_none());
    assert!(capability_for_agent("").is_none());
}

#[test]
fn capability_for_agent_is_case_sensitive_via_public_api() {
    // The stable agent ids are lower-case; no fuzzy matching should occur.
    assert!(capability_for_agent("Claude-Code").is_none());
    assert!(capability_for_agent("CODEX").is_none());
}

#[test]
fn namespace_detection_is_reexported() {
    // Detecting on the current dir never panics and yields a namespace.
    let _ns = detect_namespace(Path::new("."));
}

#[test]
fn daemon_and_autostart_types_are_reexported() {
    let auto = AutoStart {
        self_exe: PathBuf::from("/bin/true"),
        db: PathBuf::from("/tmp/rb.db"),
    };
    assert_eq!(auto.self_exe, PathBuf::from("/bin/true"));
    // DaemonClient::connect is the entrypoint Parts W reuse; lock its argument
    // shape at compile time by binding the call to an explicitly-typed future.
    // The future is never awaited (no daemon is running): constructing it is
    // enough to verify the `(&Path, Namespace, Duration, Option<AutoStart>,
    // Option<ClientIdentity>)` signature against the public re-export.
    let socket = Path::new("/tmp/rb-agents-public-api.sock");
    // Explicitly typed so the test locks the identity PARAMETER type, not just
    // the call's arity (a bare `None` would infer whatever connect takes).
    let identity: Option<rb_proto::ClientIdentity> = None;
    let _connect: std::pin::Pin<Box<dyn std::future::Future<Output = Option<DaemonClient>>>> =
        Box::pin(DaemonClient::connect(
            socket,
            rb_types::Namespace::Global,
            Duration::from_millis(1),
            Some(auto.clone()),
            identity,
        ));
}

#[test]
fn install_contract_is_reexported() {
    assert_eq!(SENTINEL, "rusty-brain");
    let scope = InstallScope::Project(PathBuf::from("/proj"));
    assert_eq!(scope, InstallScope::Project(PathBuf::from("/proj")));
    let fragment = HookFragment::new(
        PathBuf::from("/proj/.claude/settings.json"),
        serde_json::json!({ SENTINEL: {} }),
    );
    assert_eq!(fragment.merge[SENTINEL], serde_json::json!({}));
    // The W3.2 managed side-effects default to empty for a hooks-only fragment.
    assert!(fragment.allow_entries.is_empty());
    assert!(fragment.managed_files.is_empty());
    assert!(fragment.text_blocks.is_empty());
    // AgentInstaller is referenced as a trait bound to lock its name.
    fn _accepts_installer<T: AgentInstaller>(_t: &T) {}
}
