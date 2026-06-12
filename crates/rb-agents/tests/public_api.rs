//! Locks the rb-agents public API surface so Parts W/X/Y compile against exactly
//! these re-exported names. Pure integration test: imports through the crate
//! root only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rb_agents::{
    agent_for, detect_namespace, AgentCli, AgentId, AgentInstaller, AutoStart, ClaudeCodeCli,
    CodexCli, DaemonClient, GeminiCli, HookContext, HookEvent, HookFragment, HookResult,
    InstallScope, OpenCodeCli, SENTINEL,
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
    let fragment = HookFragment {
        config_path: PathBuf::from("/proj/.claude/settings.json"),
        merge: serde_json::json!({ SENTINEL: {} }),
    };
    assert_eq!(fragment.merge[SENTINEL], serde_json::json!({}));
    // AgentInstaller is referenced as a trait bound to lock its name.
    fn _accepts_installer<T: AgentInstaller>(_t: &T) {}
}
