//! OpenCode-specific types (not in shared types crate).
//!
//! These types are specific to the `OpenCode` plugin adapter and should not
//! be added to `crates/types`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Mind Tool Input (M-3, SEC-6, SEC-7, SEC-8)
// ---------------------------------------------------------------------------

/// Structured input for the native mind tool.
///
/// Deserialized from JSON on stdin. Does NOT use `deny_unknown_fields`
/// to maintain forward compatibility with future `OpenCode` protocol
/// changes (M-7, SEC-7).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MindToolInput {
    /// Operation mode. Must be one of: "search", "ask", "recent", "stats", "remember".
    /// Validated against `VALID_MODES` whitelist (SEC-8).
    pub mode: String,

    /// Search query or question text.
    /// Required for "search" and "ask" modes.
    pub query: Option<String>,

    /// Content to store as an observation.
    /// Required for "remember" mode.
    pub content: Option<String>,

    /// Maximum number of results to return.
    /// Applies to "search" and "recent" modes. Defaults to 10.
    pub limit: Option<usize>,
}

/// Valid mind tool modes (SEC-8 whitelist).
pub const VALID_MODES: &[&str] = &["search", "ask", "recent", "stats", "remember"];

// ---------------------------------------------------------------------------
// Mind Tool Output (M-3)
// ---------------------------------------------------------------------------

/// Structured output from the native mind tool.
///
/// Serialized as JSON to stdout.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MindToolOutput {
    /// Whether the operation completed successfully.
    pub success: bool,

    /// Mode-specific result data. Present when `success=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,

    /// Stable machine-parseable error code (e.g. `"E_INPUT_INVALID_FORMAT"`).
    /// Present when `success=false`. Callers should branch on this field
    /// rather than parsing the free-text `error` message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,

    /// Human-readable error message. Present when `success=false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MindToolOutput {
    /// Create a successful output with data.
    #[must_use]
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error_code: None,
            error: None,
        }
    }

    /// Create a failed output with error message and stable error code.
    #[must_use]
    pub fn error_with_code(code: &str, message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error_code: Some(code.to_string()),
            error: Some(message.into()),
        }
    }

    /// Create a failed output with error message and default `E_UNKNOWN` code.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::error_with_code(types::error_codes::E_UNKNOWN, message)
    }
}

// ---------------------------------------------------------------------------
// Sidecar State (M-4, SEC-2, SEC-4, SEC-11)
// ---------------------------------------------------------------------------

/// Session-scoped state persisted as a JSON sidecar file.
///
/// File location: `.opencode/session-<id>.json`
/// File permissions: 0600 (SEC-2)
///
/// Contains only dedup hashes, NOT raw observation content (SEC-4).
/// Written via atomic temp-file + rename (SEC-11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidecarState {
    /// Unique session identifier.
    pub session_id: String,

    /// When the session started (ISO 8601).
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp (ISO 8601).
    pub last_updated: DateTime<Utc>,

    /// Number of observations stored in this session (non-duplicate).
    pub observation_count: u32,

    /// LRU-bounded dedup cache. Max `MAX_DEDUP_ENTRIES` entries.
    /// Each entry is a 16-char hex string from `DefaultHasher`.
    pub dedup_hashes: Vec<String>,
}

/// Maximum number of entries in the dedup cache (LRU eviction boundary).
pub const MAX_DEDUP_ENTRIES: usize = 1024;

impl SidecarState {
    /// Create a new sidecar state for a session.
    #[must_use]
    pub fn new(session_id: String) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            created_at: now,
            last_updated: now,
            observation_count: 0,
            dedup_hashes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // MindToolInput
    // -----------------------------------------------------------------------

    #[test]
    fn mind_tool_input_deserializes_search_mode() {
        let json = r#"{"mode":"search","query":"test query","limit":5}"#;
        let input: MindToolInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.mode, "search");
        assert_eq!(input.query, Some("test query".to_string()));
        assert_eq!(input.limit, Some(5));
        assert!(input.content.is_none());
    }

    #[test]
    fn mind_tool_input_deserializes_remember_mode() {
        let json = r#"{"mode":"remember","content":"important finding"}"#;
        let input: MindToolInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.mode, "remember");
        assert_eq!(input.content, Some("important finding".to_string()));
        assert!(input.query.is_none());
        assert!(input.limit.is_none());
    }

    #[test]
    fn mind_tool_input_ignores_unknown_fields() {
        let json = r#"{"mode":"stats","future_field":"ignored"}"#;
        let input: MindToolInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.mode, "stats");
    }

    #[test]
    fn valid_modes_contains_all_five() {
        assert_eq!(VALID_MODES.len(), 5);
        assert!(VALID_MODES.contains(&"search"));
        assert!(VALID_MODES.contains(&"ask"));
        assert!(VALID_MODES.contains(&"recent"));
        assert!(VALID_MODES.contains(&"stats"));
        assert!(VALID_MODES.contains(&"remember"));
    }

    // -----------------------------------------------------------------------
    // MindToolOutput
    // -----------------------------------------------------------------------

    #[test]
    fn mind_tool_output_success_sets_fields_correctly() {
        let data = serde_json::json!({"key": "value"});
        let output = MindToolOutput::success(data.clone());
        assert!(output.success);
        assert_eq!(output.data, Some(data));
        assert!(output.error_code.is_none());
        assert!(output.error.is_none());
    }

    #[test]
    fn mind_tool_output_error_sets_default_code() {
        let output = MindToolOutput::error("something broke");
        assert!(!output.success);
        assert!(output.data.is_none());
        assert_eq!(
            output.error_code.as_deref(),
            Some(types::error_codes::E_UNKNOWN)
        );
        assert_eq!(output.error.as_deref(), Some("something broke"));
    }

    #[test]
    fn mind_tool_output_error_with_code_sets_custom_code() {
        let output = MindToolOutput::error_with_code("E_CUSTOM", "custom error");
        assert!(!output.success);
        assert_eq!(output.error_code.as_deref(), Some("E_CUSTOM"));
        assert_eq!(output.error.as_deref(), Some("custom error"));
    }

    #[test]
    fn mind_tool_output_success_serializes_without_error_fields() {
        let output = MindToolOutput::success(serde_json::json!(42));
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"data\":42"));
        assert!(!json.contains("error_code"));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn mind_tool_output_error_serializes_without_data_field() {
        let output = MindToolOutput::error("fail");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(!json.contains("\"data\""));
        assert!(json.contains("\"error\":\"fail\""));
    }

    // -----------------------------------------------------------------------
    // SidecarState
    // -----------------------------------------------------------------------

    #[test]
    fn sidecar_state_new_initializes_correctly() {
        let state = SidecarState::new("sess-001".to_string());
        assert_eq!(state.session_id, "sess-001");
        assert_eq!(state.observation_count, 0);
        assert!(state.dedup_hashes.is_empty());
        assert!(state.created_at <= Utc::now());
        assert_eq!(state.created_at, state.last_updated);
    }

    #[test]
    fn sidecar_state_json_round_trip() {
        let state = SidecarState::new("sess-round-trip".to_string());
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: SidecarState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, deserialized);
    }

    #[test]
    fn max_dedup_entries_is_1024() {
        assert_eq!(MAX_DEDUP_ENTRIES, 1024);
    }
}
