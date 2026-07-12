//! `rb-types` — pure domain vocabulary for rusty-brain.
//!
//! Leaf crate: no dependencies on other workspace crates. Defines the shared
//! types (`MemoryNote`, `MemoryId`, `Namespace`, `MemoryType`, `LinkType`,
//! `MemoryLink`, `SearchQuery`, `SearchResult`, `MemoryUpdates`, `Error`) used
//! across the engine, store, daemon, and binary.

mod anchor;
mod change;
mod error;
mod feedback_kind;
mod job;
mod link;
mod link_type;
mod memory;
mod memory_id;
mod memory_type;
mod namespace;
mod query;
mod stats;
mod validate;

pub use anchor::{
    kind_str as anchor_kind_str, normalize_anchor_value, parse_file_filter,
    parse_kind as parse_anchor_kind, MemoryAnchor,
};
pub use change::{ChangeKind, MemoryChanged};
pub use error::{Error, Result};
pub use feedback_kind::FeedbackKind;
pub use job::JobKind;
pub use link::{MemoryLink, SIMILARITY_LINK_MAX_COSINE_DISTANCE};
pub use link_type::LinkType;
pub use memory::MemoryNote;
pub use memory_id::MemoryId;
pub use memory_type::MemoryType;
pub use namespace::Namespace;
pub use query::{
    AnchorFilter, AnchorKind, ChannelHits, MemoryState, MemoryUpdates, RecallFilter, SearchQuery,
    SearchResult,
};
pub use stats::{FeedbackTotals, GrowthBucket, MemoryStats, TopRecalled};
pub use validate::{validate_confidence, validate_importance};
