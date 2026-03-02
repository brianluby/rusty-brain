//! Platform adapter trait and built-in adapter factory.
//!
//! Defines the [`PlatformAdapter`] trait that all platform adapters must implement,
//! and provides [`BuiltinAdapter`] — a shared implementation that handles event
//! normalization for built-in platforms (Claude Code, OpenCode).

use chrono::Utc;
use types::{EventKind, HookInput, PlatformEvent, ProjectContext};
use uuid::Uuid;

/// Contract version declared by all built-in adapters.
pub const ADAPTER_CONTRACT_VERSION: &str = "1.0.0";

/// A platform adapter normalizes raw hook input into typed platform events.
pub trait PlatformAdapter: Send + Sync {
    /// Returns the lowercase platform name.
    fn platform_name(&self) -> &str;
    /// Returns the adapter's contract version as a semver string.
    fn contract_version(&self) -> &str;
    /// Normalize raw hook input into a typed platform event.
    /// Returns `None` if the input cannot be normalized.
    fn normalize(&self, input: &HookInput, event_kind_hint: &str) -> Option<PlatformEvent>;
}

/// Built-in adapter that uses shared normalization logic.
///
/// All built-in adapters (Claude Code, `OpenCode`) share identical normalization
/// behavior. This struct stores only the lowercase platform name; all logic
/// lives in the [`PlatformAdapter`] impl.
struct BuiltinAdapter {
    platform: String,
}

impl BuiltinAdapter {
    fn new(platform_name: &str) -> Self {
        Self {
            platform: platform_name.to_lowercase(),
        }
    }
}

impl PlatformAdapter for BuiltinAdapter {
    fn platform_name(&self) -> &str {
        &self.platform
    }

    fn contract_version(&self) -> &str {
        ADAPTER_CONTRACT_VERSION
    }

    fn normalize(&self, input: &HookInput, event_kind_hint: &str) -> Option<PlatformEvent> {
        // Session ID is required (FR-005a).
        if input.session_id.trim().is_empty() {
            return None;
        }

        // Map event kind hint to EventKind.
        let kind = match event_kind_hint.to_lowercase().as_str() {
            "sessionstart" | "session_start" => EventKind::SessionStart,
            "posttooluse" | "pretooluse" | "tool_observation" => {
                // Tool name required for tool observations (FR-005).
                let tool_name = input.tool_name.as_ref()?;
                let trimmed = tool_name.trim();
                if trimmed.is_empty() {
                    return None;
                }
                EventKind::ToolObservation {
                    tool_name: trimmed.to_string(),
                }
            }
            "stop" | "sessionstop" | "session_stop" => EventKind::SessionStop,
            _ => return None,
        };

        let project_context = ProjectContext {
            platform_project_id: None,
            canonical_path: None,
            cwd: Some(input.cwd.clone()),
        };

        Some(PlatformEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            platform: self.platform.clone(),
            contract_version: ADAPTER_CONTRACT_VERSION.to_string(),
            session_id: input.session_id.clone(),
            project_context,
            kind,
        })
    }
}

/// Create a built-in adapter for the given platform name.
///
/// The platform name is lowercased and stored. All built-in adapters share
/// the same normalization logic via [`BuiltinAdapter`].
#[must_use]
pub fn create_builtin_adapter(platform_name: &str) -> Box<dyn PlatformAdapter> {
    Box::new(BuiltinAdapter::new(platform_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // T011: PlatformAdapter trait + factory tests
    // -------------------------------------------------------------------------

    #[test]
    fn factory_creates_adapter_with_correct_name() {
        let adapter = create_builtin_adapter("claude");
        assert_eq!(
            adapter.platform_name(),
            "claude",
            "factory must create adapter with correct platform name"
        );
    }

    #[test]
    fn factory_creates_adapter_with_contract_version() {
        let adapter = create_builtin_adapter("claude");
        assert_eq!(
            adapter.contract_version(),
            "1.0.0",
            "factory must create adapter with contract version 1.0.0"
        );
    }

    #[test]
    fn whitespace_only_tool_name_returns_none() {
        let adapter = create_builtin_adapter("claude");
        let json = r#"{
            "session_id": "test-session",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": "/tmp",
            "permission_mode": "default",
            "hook_event_name": "PostToolUse",
            "tool_name": "   "
        }"#;
        let input: types::HookInput = serde_json::from_str(json).expect("valid HookInput JSON");
        let result = adapter.normalize(&input, "PostToolUse");
        assert!(
            result.is_none(),
            "whitespace-only tool_name must return None"
        );
    }

    #[test]
    fn platform_name_returns_lowercase() {
        let adapter = create_builtin_adapter("CLAUDE");
        assert_eq!(
            adapter.platform_name(),
            "claude",
            "platform name must be lowercased"
        );
    }
}
