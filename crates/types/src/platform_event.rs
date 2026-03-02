//! Platform event types representing normalized agent session events.
//!
//! A [`PlatformEvent`] is a normalized, typed record of something that happened
//! during an agent session. Each event is classified by an [`EventKind`] and
//! carries platform-specific context extracted during normalization.
//!
//! These types are produced by platform adapters (in the `platforms` crate)
//! when converting raw hook input into a consistent internal representation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project_context::ProjectContext;

/// The kind of platform event, with variant-specific payloads.
///
/// Serialized as `snake_case` tagged enum (e.g. `"session_start"`,
/// `"tool_observation"`). The enum is `#[non_exhaustive]` to allow new
/// event kinds in future releases without breaking downstream matches.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The agent session has started.
    SessionStart,
    /// A tool was invoked during the session.
    ToolObservation {
        /// Name of the tool that was observed (e.g. "bash", "edit", "read").
        tool_name: String,
    },
    /// The agent session has stopped.
    SessionStop,
}

/// A normalized, typed record of something that happened during an agent session.
///
/// Produced by a `PlatformAdapter::normalize()` call. Contains a unique event
/// ID, UTC timestamp, platform identification, and the event kind with any
/// variant-specific payload. Serialized with camelCase field names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformEvent {
    /// Unique identifier (UUID v4), auto-generated during normalization.
    pub event_id: Uuid,
    /// UTC timestamp of when the event was normalized.
    pub timestamp: DateTime<Utc>,
    /// Lowercase platform name (e.g. "claude", "opencode").
    pub platform: String,
    /// Semver string declared by the adapter (e.g. "1.0.0").
    pub contract_version: String,
    /// Session identifier extracted from hook input.
    pub session_id: String,
    /// Project context extracted from hook input.
    pub project_context: ProjectContext,
    /// Classification and payload of this event.
    pub kind: EventKind,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // T004: EventKind — serde round-trip tests
    // -------------------------------------------------------------------------

    #[test]
    fn event_kind_session_start_serde_round_trip() {
        let original = EventKind::SessionStart;
        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: EventKind =
            serde_json::from_str(&json).expect("deserialization must succeed");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn event_kind_tool_observation_serde_round_trip() {
        let original = EventKind::ToolObservation {
            tool_name: "bash".to_string(),
        };
        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: EventKind =
            serde_json::from_str(&json).expect("deserialization must succeed");
        assert_eq!(original, deserialized);

        // Verify the tool_name is preserved
        if let EventKind::ToolObservation { tool_name } = &deserialized {
            assert_eq!(tool_name, "bash");
        } else {
            panic!("expected ToolObservation variant after round-trip");
        }
    }

    #[test]
    fn event_kind_session_stop_serde_round_trip() {
        let original = EventKind::SessionStop;
        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: EventKind =
            serde_json::from_str(&json).expect("deserialization must succeed");
        assert_eq!(original, deserialized);
    }

    // -------------------------------------------------------------------------
    // T004: PlatformEvent — serde round-trip test
    // -------------------------------------------------------------------------

    #[test]
    fn platform_event_serde_round_trip() {
        let event_id = Uuid::new_v4();
        let timestamp = Utc::now();
        let original = PlatformEvent {
            event_id,
            timestamp,
            platform: "claude".to_string(),
            contract_version: "1.0.0".to_string(),
            session_id: "ses-abc-123".to_string(),
            project_context: ProjectContext {
                platform_project_id: None,
                canonical_path: None,
                cwd: Some("/test".to_string()),
            },
            kind: EventKind::ToolObservation {
                tool_name: "edit".to_string(),
            },
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: PlatformEvent =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve all PlatformEvent fields"
        );
        assert_eq!(deserialized.event_id, event_id);
        assert_eq!(deserialized.timestamp, timestamp);
        assert_eq!(deserialized.platform, "claude");
        assert_eq!(deserialized.contract_version, "1.0.0");
        assert_eq!(deserialized.session_id, "ses-abc-123");
        assert_eq!(deserialized.project_context.cwd, Some("/test".to_string()));
        assert!(deserialized.project_context.platform_project_id.is_none());
        assert!(deserialized.project_context.canonical_path.is_none());

        if let EventKind::ToolObservation { tool_name } = &deserialized.kind {
            assert_eq!(tool_name, "edit");
        } else {
            panic!("expected ToolObservation kind after round-trip");
        }
    }

    // -------------------------------------------------------------------------
    // T004: EventKind — Debug, Clone, PartialEq derives
    // -------------------------------------------------------------------------

    #[test]
    fn event_kind_derives_debug_clone_eq() {
        // Debug
        let kind = EventKind::SessionStart;
        let debug_str = format!("{:?}", kind);
        assert!(
            debug_str.contains("SessionStart"),
            "Debug output should contain variant name"
        );

        let kind_with_payload = EventKind::ToolObservation {
            tool_name: "grep".to_string(),
        };
        let debug_payload = format!("{:?}", kind_with_payload);
        assert!(
            debug_payload.contains("ToolObservation"),
            "Debug output should contain variant name"
        );
        assert!(
            debug_payload.contains("grep"),
            "Debug output should contain payload"
        );

        // Clone
        let original = EventKind::ToolObservation {
            tool_name: "bash".to_string(),
        };
        let cloned = original.clone();
        assert_eq!(original, cloned, "Clone must produce equal value");

        // PartialEq + Eq
        assert_eq!(EventKind::SessionStart, EventKind::SessionStart);
        assert_eq!(EventKind::SessionStop, EventKind::SessionStop);
        assert_ne!(EventKind::SessionStart, EventKind::SessionStop);
        assert_eq!(
            EventKind::ToolObservation {
                tool_name: "x".to_string()
            },
            EventKind::ToolObservation {
                tool_name: "x".to_string()
            },
        );
        assert_ne!(
            EventKind::ToolObservation {
                tool_name: "x".to_string()
            },
            EventKind::ToolObservation {
                tool_name: "y".to_string()
            },
        );
    }

    // -------------------------------------------------------------------------
    // T004: PlatformEvent — Debug, Clone derives
    // -------------------------------------------------------------------------

    #[test]
    fn platform_event_derives_debug_clone() {
        let event = PlatformEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            platform: "claude".to_string(),
            contract_version: "1.0.0".to_string(),
            session_id: "ses-001".to_string(),
            project_context: ProjectContext {
                platform_project_id: None,
                canonical_path: None,
                cwd: Some("/test".to_string()),
            },
            kind: EventKind::SessionStart,
        };

        // Debug
        let debug_str = format!("{:?}", event);
        assert!(
            debug_str.contains("PlatformEvent"),
            "Debug output should contain struct name"
        );
        assert!(
            debug_str.contains("claude"),
            "Debug output should contain platform value"
        );

        // Clone
        let cloned = event.clone();
        assert_eq!(event, cloned, "Clone must produce equal value");
        assert_eq!(cloned.event_id, event.event_id);
        assert_eq!(cloned.platform, "claude");
        assert_eq!(cloned.session_id, "ses-001");
    }
}
