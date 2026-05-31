//! `rb-types` — pure domain vocabulary for rusty-brain.

mod error;
mod memory_id;
mod memory_type;
mod namespace;

pub use error::{Error, Result};
pub use memory_id::MemoryId;
pub use memory_type::MemoryType;
pub use namespace::Namespace;
