// Contract: OpenCode Plugin Types
// Feature: 008-opencode-plugin
// Date: 2026-03-03
//
// Types defined in crates/opencode (NOT in crates/types, since they are
// OpenCode-specific and should not pollute the shared types crate).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Mind Tool Input (M-3, SEC-6, SEC-7, SEC-8)
// ---------------------------------------------------------------------------

/// Structured input for the native mind tool.
///
/// Deserialized from JSON on stdin. Does NOT use `deny_unknown_fields`
/// to maintain forward compatibility with future OpenCode protocol
/// changes (M-7, SEC-7).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MindToolInput {
    /// Operation mode. Must be one of: "search", "ask", "recent", "stats", "remember".
    /// Validated against VALID_MODES whitelist (SEC-8).
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

    /// Mode-specific result data. Present when success=true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,

    /// Stable machine-parseable error code (e.g. `"E_INPUT_INVALID_FORMAT"`).
    /// Present when `success=false`. Callers should branch on this field
    /// rather than parsing the free-text `error` message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,

    /// Human-readable error message. Present when success=false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MindToolOutput {
    /// Create a successful output with data.
    pub fn success(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error_code: None,
            error: None,
        }
    }

    /// Create a failed output with error message and stable error code.
    pub fn error_with_code(code: &str, message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error_code: Some(code.to_string()),
            error: Some(message.into()),
        }
    }

    /// Create a failed output with error message (no structured code).
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error_code: None,
            error: Some(message.into()),
        }
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

    /// LRU-bounded dedup cache. Max 1024 entries.
    /// Each entry is a 16-char hex string from DefaultHasher.
    pub dedup_hashes: Vec<String>,
}

/// Maximum number of entries in the dedup cache (LRU eviction boundary).
pub const MAX_DEDUP_ENTRIES: usize = 1024;

impl SidecarState {
    /// Create a new sidecar state for a session.
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
