//! `rb-types` — pure domain vocabulary for rusty-brain.
//!
//! Leaf crate: no dependencies on other workspace crates. Defines the shared
//! types (`MemoryNote`, `MemoryId`, `Namespace`, `MemoryType`, `LinkType`,
//! `MemoryLink`, `SearchQuery`, `SearchResult`, `MemoryUpdates`, `Error`) used
//! across the engine, store, daemon, and binary.

mod change;
mod error;
mod link;
mod link_type;
mod memory;
mod memory_id;
mod memory_type;
mod namespace;
mod query;
mod validate;

pub use change::{ChangeKind, MemoryChanged};
pub use error::{Error, Result};
pub use link::MemoryLink;
pub use link_type::LinkType;
pub use memory::MemoryNote;
pub use memory_id::MemoryId;
pub use memory_type::MemoryType;
pub use namespace::Namespace;
pub use query::{MemoryUpdates, SearchQuery, SearchResult};
pub use validate::validate_importance;
