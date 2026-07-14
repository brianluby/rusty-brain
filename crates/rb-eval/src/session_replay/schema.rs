//! Normalized, source-independent session replay schema.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Version stamped on every normalized artifact.
pub const SESSION_REPLAY_SCHEMA_VERSION: &str = "rusty-brain-session-replay-v1";

/// Local transcript store that produced a normalized record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Claude Code JSONL under its per-project transcript root.
    ClaudeCode,
    /// OpenCode's local SQLite session store.
    OpenCode,
    /// A generated record that never came from a transcript.
    Synthetic,
}

/// Event role after source-specific message shapes are normalized.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventRole {
    User,
    Assistant,
    Tool,
    System,
}

/// Events are deliberately separated into dialogue and non-dialogue kinds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Dialogue,
    ToolCall,
    ToolResult,
    RepositoryEvidence,
    Lifecycle,
}

/// Authority is explicit so assistant prose cannot silently become truth.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAuthority {
    UserStatement,
    AssistantUncorroborated,
    ToolEvidence,
    CommittedRepositoryState,
    SystemMetadata,
}

impl EvidenceAuthority {
    /// Whether this authority class may become an evaluation candidate memory.
    pub fn candidate_eligible(self) -> bool {
        matches!(
            self,
            Self::UserStatement | Self::ToolEvidence | Self::CommittedRepositoryState
        )
    }
}

/// Redaction/de-identification category recorded without the matched text.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionCategory {
    Credential,
    HighEntropy,
    Email,
    Phone,
    IpAddress,
    Hostname,
    HomePath,
    AbsolutePath,
    PersonalName,
    UserIdentifier,
    Organization,
    Url,
}

/// Rejection reasons are aggregate-safe and never contain source text.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCategory {
    MalformedJson,
    MissingSession,
    MissingProject,
    MissingTimestamp,
    InvalidTimestamp,
    MissingRole,
    MissingContent,
    UnsupportedRecord,
    PrivateReasoning,
    OversizedContent,
    RedactionUnavailable,
    ResidualSensitiveData,
    EmptySession,
    CrossesTimeBoundary,
}

/// Source pointers are pseudonymous but retain record ordering and adapter provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub source: SourceKind,
    pub source_locator_id: String,
    pub source_record_id: String,
    pub source_record_index: u64,
    pub adapter_version: String,
}

/// Normalized details for a non-dialogue tool event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolEvent {
    pub name: String,
    pub status: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
}

/// One normalized, de-identified source event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NormalizedEvent {
    pub schema_version: String,
    pub event_id: String,
    pub session_id: String,
    pub project_id: String,
    pub timestamp: DateTime<Utc>,
    pub ordinal: u64,
    pub role: EventRole,
    pub kind: EventKind,
    pub authority: EvidenceAuthority,
    pub provenance: Provenance,
    pub content: Option<String>,
    pub tool: Option<ToolEvent>,
    pub redactions: Vec<RedactionCategory>,
}

impl NormalizedEvent {
    /// True only for user/assistant text. Tool content never enters this lane.
    pub fn is_dialogue(&self) -> bool {
        self.kind == EventKind::Dialogue
            && matches!(self.role, EventRole::User | EventRole::Assistant)
    }
}

/// A whole session is the indivisible unit used for temporal splitting.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NormalizedSession {
    pub schema_version: String,
    pub session_id: String,
    pub project_id: String,
    pub source: SourceKind,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub events: Vec<NormalizedEvent>,
}

/// Output lane selection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayLane {
    DialogueOnly,
    FullEvent,
}

/// Whole-session split assigned using a single time boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Development,
    Holdout,
}

/// Extracted text is never evaluation truth until a human explicitly reviews it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Unreviewed,
    ReviewedSanitized,
}

/// Earlier authoritative evidence made available to a later natural query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CandidateMemory {
    pub schema_version: String,
    pub candidate_id: String,
    pub session_id: String,
    pub project_id: String,
    pub timestamp: DateTime<Utc>,
    pub split: DatasetSplit,
    pub authority: EvidenceAuthority,
    pub review_status: ReviewStatus,
    pub semantic_ground_truth: bool,
    pub text: String,
    pub provenance: Provenance,
}

/// A later user turn that refers back to eligible earlier evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NaturalQuery {
    pub schema_version: String,
    pub query_id: String,
    pub session_id: String,
    pub project_id: String,
    pub timestamp: DateTime<Utc>,
    pub split: DatasetSplit,
    pub review_status: ReviewStatus,
    pub semantic_ground_truth: bool,
    pub text: String,
    pub candidate_pool_ids: Vec<String>,
    pub provenance: Provenance,
}

/// Aggregate adapter inventory. Maps are ordered for deterministic JSON.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImportStats {
    pub source: SourceKind,
    pub source_containers: u64,
    pub source_sessions: u64,
    pub source_records: u64,
    pub accepted_sessions: u64,
    pub accepted_events: u64,
    pub dialogue_events: u64,
    pub tool_events: u64,
    pub lifecycle_events: u64,
    pub events_with_redactions: u64,
    pub redactions: BTreeMap<RedactionCategory, u64>,
    pub rejections: BTreeMap<RejectionCategory, u64>,
}

impl ImportStats {
    pub(crate) fn new(source: SourceKind) -> Self {
        Self {
            source,
            source_containers: 0,
            source_sessions: 0,
            source_records: 0,
            accepted_sessions: 0,
            accepted_events: 0,
            dialogue_events: 0,
            tool_events: 0,
            lifecycle_events: 0,
            events_with_redactions: 0,
            redactions: BTreeMap::new(),
            rejections: BTreeMap::new(),
        }
    }

    pub(crate) fn reject(&mut self, category: RejectionCategory) {
        *self.rejections.entry(category).or_default() += 1;
    }

    pub(crate) fn accept_event(&mut self, event: &NormalizedEvent) {
        self.accepted_events += 1;
        if event.is_dialogue() {
            self.dialogue_events += 1;
        } else if matches!(
            event.kind,
            EventKind::ToolCall | EventKind::ToolResult | EventKind::RepositoryEvidence
        ) {
            self.tool_events += 1;
        } else {
            self.lifecycle_events += 1;
        }
        if !event.redactions.is_empty() {
            self.events_with_redactions += 1;
        }
        for category in &event.redactions {
            *self.redactions.entry(*category).or_default() += 1;
        }
    }
}

/// Result returned by either local-store adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdapterOutput {
    pub sessions: Vec<NormalizedSession>,
    pub stats: ImportStats,
}

/// Candidate/query construction outcome with leakage rejections reported separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CandidateDataset {
    pub candidates: Vec<CandidateMemory>,
    pub queries: Vec<NaturalQuery>,
    pub development_sessions: u64,
    pub holdout_sessions: u64,
    pub crossing_sessions_rejected: u64,
}
