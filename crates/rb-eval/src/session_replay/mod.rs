//! Privacy-preserving, local-only transcript normalization and replay datasets.
//!
//! The source adapters never write raw input. All strings pass through the
//! strict session-scoped redactor before they can enter a normalized event.
//! Assistant dialogue remains available for dialogue-shape evaluation but is
//! always marked [`EvidenceAuthority::AssistantUncorroborated`] and is never
//! eligible to become a candidate memory.

mod augmentation;
mod claude;
mod common;
mod dataset;
mod opencode;
mod privacy;
mod report;
mod schema;

pub use augmentation::{
    controlled_variants, generate_distractors, ControlledVariant, Distractor, VariantKind,
    DEFAULT_FAKER_SEED, DISTRACTOR_CORPUS_SIZES, FAKER_VERSION,
};
pub use claude::import_claude_jsonl;
pub use common::AdapterError;
pub use dataset::{build_candidate_dataset, derive_holdout_boundary, events_for_lane};
pub use opencode::import_opencode_db;
pub use privacy::{PrivacyError, SanitizedText, StrictRedactor};
pub use report::{build_inventory_report, write_local_artifacts, ArtifactError, InventoryReport};
pub use schema::{
    AdapterOutput, CandidateDataset, CandidateMemory, DatasetSplit, EventKind, EventRole,
    EvidenceAuthority, ImportStats, NaturalQuery, NormalizedEvent, NormalizedSession, Provenance,
    RedactionCategory, RejectionCategory, ReplayLane, ReviewStatus, SourceKind, ToolEvent,
    SESSION_REPLAY_SCHEMA_VERSION,
};
