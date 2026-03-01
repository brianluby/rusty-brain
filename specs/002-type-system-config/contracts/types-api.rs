// ============================================================================
// Contract: crates/types public API surface
// Branch: 002-type-system-config
// Date: 2026-03-01
//
// This file defines the PUBLIC API contract for the types crate.
// It is a specification artifact, NOT compilable source code.
// Implementation must match these signatures.
// ============================================================================

// ---------------------------------------------------------------------------
// Module: observation
// ---------------------------------------------------------------------------

/// Classification of what kind of event an observation represents.
/// 10 variants covering discoveries, decisions, problems, solutions,
/// patterns, warnings, successes, and code changes.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservationType {
    Discovery,
    Decision,
    Problem,
    Solution,
    Pattern,
    Warning,
    Success,
    Refactor,
    Bugfix,
    Feature,
}

/// A single memory entry recorded during an agent work session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    #[serde(rename = "type")]
    pub obs_type: ObservationType,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub summary: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ObservationMetadata>,
}

/// Extensible metadata attached to an observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationMetadata {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub project_key: String,
    #[serde(default)]
    pub compressed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Module: session
// ---------------------------------------------------------------------------

/// Aggregated summary of an entire agent work session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub observation_count: u64,
    #[serde(default)]
    pub key_decisions: Vec<String>,
    #[serde(default, rename = "filesModified")]
    pub modified_files: Vec<String>,
    pub summary: String,
}

// ---------------------------------------------------------------------------
// Module: context
// ---------------------------------------------------------------------------

/// Bundle of recent memories and session context for injection
/// into an agent's conversation at session start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectedContext {
    #[serde(default)]
    pub recent_observations: Vec<Observation>,
    #[serde(default)]
    pub relevant_memories: Vec<Observation>,
    #[serde(default)]
    pub session_summaries: Vec<SessionSummary>,
    #[serde(default)]
    pub token_count: u64,
}

// ---------------------------------------------------------------------------
// Module: config
// ---------------------------------------------------------------------------

/// Configuration controlling the memory engine's behavior.
///
/// Supports three resolution sources with precedence:
/// environment variable > JSON file > programmatic default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct MindConfig {
    pub memory_path: PathBuf,           // default: ".agent-brain/mind.mv2"
    pub max_context_observations: u32,  // default: 20
    pub max_context_tokens: u32,        // default: 2000
    pub auto_compress: bool,            // default: true
    pub min_confidence: f64,            // default: 0.6
    pub debug: bool,                    // default: false
}

impl Default for MindConfig { /* documented defaults */ }

impl MindConfig {
    /// Resolve configuration with environment variable overrides.
    ///
    /// Reads supported environment variables and applies them
    /// over the default configuration. Invalid env var values
    /// produce a Configuration error identifying the field and value.
    pub fn from_env() -> Result<Self, AgentBrainError>;

    /// Validate this configuration's invariants.
    ///
    /// Checks:
    /// - `min_confidence` is in `0.0..=1.0`
    /// - `max_context_observations` is > 0
    /// - `max_context_tokens` is > 0
    ///
    /// Returns `AgentBrainError::Configuration` on violation.
    pub fn validate(&self) -> Result<(), AgentBrainError>;
}

// ---------------------------------------------------------------------------
// Validated constructors
// ---------------------------------------------------------------------------

impl Observation {
    /// Construct a validated `Observation`.
    ///
    /// Auto-generates `id` (Uuid::new_v4) and `timestamp` (Utc::now).
    /// Validates that `summary` and `content` are non-empty and not
    /// whitespace-only. Returns `AgentBrainError::InvalidInput` on failure.
    ///
    /// Fields remain `pub` for reading. `new()` is the recommended
    /// constructor for validated construction.
    pub fn new(
        obs_type: ObservationType,
        tool_name: String,
        summary: String,
        content: String,
        metadata: Option<ObservationMetadata>,
    ) -> Result<Self, AgentBrainError>;
}

impl SessionSummary {
    /// Construct a validated `SessionSummary`.
    ///
    /// Validates:
    /// - `id` is non-empty
    /// - `summary` is non-empty
    /// - `end_time` >= `start_time`
    ///
    /// Returns `AgentBrainError::InvalidInput` on failure.
    ///
    /// Fields remain `pub` for reading. `new()` is the recommended
    /// constructor for validated construction.
    pub fn new(
        id: String,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        observation_count: u64,
        key_decisions: Vec<String>,
        modified_files: Vec<String>,
        summary: String,
    ) -> Result<Self, AgentBrainError>;
}

// ---------------------------------------------------------------------------
// Module: stats
// ---------------------------------------------------------------------------

/// Statistical snapshot of the memory store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MindStats {
    pub total_observations: u64,
    pub total_sessions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_memory: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_memory: Option<DateTime<Utc>>,
    #[serde(rename = "fileSize")]
    pub file_size_bytes: u64,
    #[serde(rename = "topTypes")]
    #[serde(default)]
    pub type_counts: HashMap<ObservationType, u64>,
}

// ---------------------------------------------------------------------------
// Module: hooks
// ---------------------------------------------------------------------------

/// Structured input received from the host AI agent via stdin.
///
/// Uses a flat struct with optional event-specific fields.
/// Unknown fields are silently ignored for forward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    pub permission_mode: String,
    pub hook_event_name: String,

    // Event-specific fields (optional, presence depends on event type)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_hook_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Structured output sent back to the host AI agent via stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookOutput {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "continue")]
    pub continue_execution: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "stopReason")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "suppressOutput")]
    pub suppress_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "systemMessage")]
    pub system_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "hookSpecificOutput")]
    pub hook_specific_output: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Module: error
// ---------------------------------------------------------------------------

/// Unified error hierarchy for all failure modes.
///
/// Each variant carries a stable error code (&'static str) that does not
/// change across versions, enabling agents to match programmatically.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AgentBrainError {
    #[error("[{code}] {message}")]
    FileSystem {
        code: &'static str,
        message: String,
        #[source]
        source: Option<std::io::Error>,
    },

    #[error("[{code}] {message}")]
    Configuration {
        code: &'static str,
        message: String,
    },

    #[error("[{code}] {message}")]
    Serialization {
        code: &'static str,
        message: String,
        #[source]
        source: Option<serde_json::Error>,
    },

    #[error("[{code}] {message}")]
    Lock {
        code: &'static str,
        message: String,
    },

    #[error("[{code}] {message}")]
    MemoryCorruption {
        code: &'static str,
        message: String,
    },

    #[error("[{code}] {message}")]
    InvalidInput {
        code: &'static str,
        message: String,
    },
}

impl AgentBrainError {
    /// Returns the stable error code for this error.
    pub fn code(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Error code constants
// ---------------------------------------------------------------------------

pub mod error_codes {
    // FileSystem
    pub const E_FS_NOT_FOUND: &str = "E_FS_NOT_FOUND";
    pub const E_FS_PERMISSION_DENIED: &str = "E_FS_PERMISSION_DENIED";
    pub const E_FS_IO_ERROR: &str = "E_FS_IO_ERROR";

    // Configuration
    pub const E_CONFIG_INVALID_VALUE: &str = "E_CONFIG_INVALID_VALUE";
    pub const E_CONFIG_MISSING_FIELD: &str = "E_CONFIG_MISSING_FIELD";
    pub const E_CONFIG_PARSE_ERROR: &str = "E_CONFIG_PARSE_ERROR";

    // Serialization
    pub const E_SER_SERIALIZE_FAILED: &str = "E_SER_SERIALIZE_FAILED";
    pub const E_SER_DESERIALIZE_FAILED: &str = "E_SER_DESERIALIZE_FAILED";

    // Lock
    pub const E_LOCK_ACQUISITION_FAILED: &str = "E_LOCK_ACQUISITION_FAILED";
    pub const E_LOCK_TIMEOUT: &str = "E_LOCK_TIMEOUT";

    // MemoryCorruption
    pub const E_MEM_CORRUPTED_INDEX: &str = "E_MEM_CORRUPTED_INDEX";
    pub const E_MEM_INVALID_CHECKSUM: &str = "E_MEM_INVALID_CHECKSUM";

    // InvalidInput
    pub const E_INPUT_EMPTY_FIELD: &str = "E_INPUT_EMPTY_FIELD";
    pub const E_INPUT_OUT_OF_RANGE: &str = "E_INPUT_OUT_OF_RANGE";
    pub const E_INPUT_INVALID_FORMAT: &str = "E_INPUT_INVALID_FORMAT";
}
