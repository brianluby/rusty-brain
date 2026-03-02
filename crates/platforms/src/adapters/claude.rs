//! Claude Code platform adapter implementation.
//!
//! Provides a factory function that returns a [`PlatformAdapter`](crate::adapter::PlatformAdapter)
//! configured for Claude Code's hook protocol.

use crate::adapter::create_builtin_adapter;

/// Create the Claude Code platform adapter.
#[must_use]
pub fn claude_adapter() -> Box<dyn crate::adapter::PlatformAdapter> {
    create_builtin_adapter("claude")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::test_helpers::{make_input, make_input_with_session_id};
    use types::EventKind;

    // -------------------------------------------------------------------------
    // T012: Claude adapter normalization tests
    // -------------------------------------------------------------------------

    #[test]
    fn session_start_event() {
        let adapter = claude_adapter();
        let input = make_input("SessionStart", None, "claude");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("SessionStart must produce Some");
        assert_eq!(event.kind, EventKind::SessionStart);
    }

    #[test]
    fn tool_observation_event() {
        let adapter = claude_adapter();
        let input = make_input("PostToolUse", Some("Write"), "claude");
        let event = adapter
            .normalize(&input, "PostToolUse")
            .expect("PostToolUse with tool_name must produce Some");
        assert_eq!(
            event.kind,
            EventKind::ToolObservation {
                tool_name: "Write".to_string()
            }
        );
    }

    #[test]
    fn session_stop_event() {
        let adapter = claude_adapter();
        let input = make_input("Stop", None, "claude");
        let event = adapter
            .normalize(&input, "Stop")
            .expect("Stop must produce Some");
        assert_eq!(event.kind, EventKind::SessionStop);
    }

    #[test]
    fn empty_session_id_returns_none() {
        let adapter = claude_adapter();
        let input = make_input_with_session_id("", "claude");
        let result = adapter.normalize(&input, "SessionStart");
        assert!(result.is_none(), "empty session_id must return None");
    }

    #[test]
    fn whitespace_session_id_returns_none() {
        let adapter = claude_adapter();
        let input = make_input_with_session_id("   ", "claude");
        let result = adapter.normalize(&input, "SessionStart");
        assert!(result.is_none(), "whitespace session_id must return None");
    }

    #[test]
    fn tool_observation_without_tool_name_returns_none() {
        let adapter = claude_adapter();
        let input = make_input("PostToolUse", None, "claude");
        let result = adapter.normalize(&input, "PostToolUse");
        assert!(
            result.is_none(),
            "PostToolUse without tool_name must return None"
        );
    }

    #[test]
    fn event_has_uuid() {
        let adapter = claude_adapter();
        let input = make_input("SessionStart", None, "claude");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("must produce event");
        assert!(
            !event.event_id.is_nil(),
            "event_id must be a valid (non-nil) UUID"
        );
    }

    #[test]
    fn event_has_timestamp() {
        let adapter = claude_adapter();
        let input = make_input("SessionStart", None, "claude");
        let before = chrono::Utc::now();
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("must produce event");
        let after = chrono::Utc::now();
        assert!(
            event.timestamp >= before && event.timestamp <= after,
            "timestamp must be between before and after normalization"
        );
    }

    #[test]
    fn project_context_cwd_populated() {
        let adapter = claude_adapter();
        let input = make_input("SessionStart", None, "claude");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("must produce event");
        assert_eq!(
            event.project_context.cwd,
            Some("/home/user/project".to_string()),
            "project_context.cwd must be populated from input"
        );
    }

    #[test]
    fn platform_is_claude() {
        let adapter = claude_adapter();
        let input = make_input("SessionStart", None, "claude");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("must produce event");
        assert_eq!(event.platform, "claude", "platform must be 'claude'");
    }

    #[test]
    fn contract_version_is_1_0_0() {
        let adapter = claude_adapter();
        let input = make_input("SessionStart", None, "claude");
        let event = adapter
            .normalize(&input, "SessionStart")
            .expect("must produce event");
        assert_eq!(
            event.contract_version, "1.0.0",
            "contract_version must be '1.0.0'"
        );
    }
}
