//! `rb-types` — pure domain vocabulary for rusty-brain.

mod error;
mod memory_id;
mod namespace;

pub use error::{Error, Result};
pub use memory_id::MemoryId;
pub use namespace::Namespace;
