//! `OpenCode` platform adapter implementation.
//!
//! Provides a factory function that returns a [`PlatformAdapter`](crate::adapter::PlatformAdapter)
//! configured for the `OpenCode` hook protocol.

use crate::adapter::create_builtin_adapter;

/// Create the `OpenCode` platform adapter.
#[must_use]
pub fn opencode_adapter() -> Box<dyn crate::adapter::PlatformAdapter> {
    create_builtin_adapter("opencode")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::claude::claude_adapter;
    use crate::adapters::test_helpers::make_input;
    use types::{EventKind, HookInput};

    // -------------------------------------------------------------------------
    // T013: OpenCode adapter normalization tests
    // -------------------------------------------------------------------------

    #[test]
    fn session_start_event() {
        let adapter = opencode_adapter();
        let input = make_input("SessionStart", None, "opencode");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("SessionStart must produce Some");
        assert_eq!(event.kind, EventKind::SessionStart);
    }

    #[test]
    fn platform_is_opencode() {
        let adapter = opencode_adapter();
        let input = make_input("SessionStart", None, "opencode");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("must produce event");
        assert_eq!(event.platform, "opencode", "platform must be 'opencode'");
    }

    #[test]
    fn same_field_structure_as_claude() {
        let oc_adapter = opencode_adapter();
        let cl_adapter = claude_adapter();

        let oc_input = make_input("SessionStart", None, "opencode");
        // Build a Claude input with the same fields but platform="claude".
        let cl_json = r#"{
            "session_id": "test-session-123",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": "/home/user/project",
            "permission_mode": "default",
            "hook_event_name": "SessionStart",
            "platform": "claude"
        }"#;
        let cl_input: HookInput =
            serde_json::from_str(cl_json).expect("test HookInput JSON must parse");

        let oc_event = oc_adapter
            .normalize(&oc_input, "SessionStart")
            .expect("opencode must produce event");
        let cl_event = cl_adapter
            .normalize(&cl_input, "SessionStart")
            .expect("claude must produce event");

        // Both events must have the same field structure (differ only in
        // platform, event_id, and timestamp).
        assert_eq!(oc_event.contract_version, cl_event.contract_version);
        assert_eq!(oc_event.session_id, cl_event.session_id);
        assert_eq!(oc_event.project_context.cwd, cl_event.project_context.cwd);
        assert_eq!(
            oc_event.project_context.platform_project_id,
            cl_event.project_context.platform_project_id
        );
        assert_eq!(
            oc_event.project_context.canonical_path,
            cl_event.project_context.canonical_path
        );
        assert_eq!(oc_event.kind, cl_event.kind);

        // Platform names must differ.
        assert_ne!(
            oc_event.platform, cl_event.platform,
            "platforms should differ"
        );
    }
}
