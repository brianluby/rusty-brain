//! `Mind` — the core memory engine for rusty-brain.
//!
//! Provides the public API for storing, searching, and retrieving observations
//! from a memvid-backed `.mv2` memory file.

mod lifecycle;
mod locking;
mod read;
mod stats;
mod write;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::backend::MemvidBackend;
#[cfg(test)]
use types::error_codes;
use types::{MindConfig, MindStats, ObservationType};

/// A single timeline entry representing a stored observation.
///
/// Returned by [`Mind::timeline()`]. Contains parsed metadata from the
/// underlying backend frame.
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    /// The observation type (discovery, decision, etc.)
    pub obs_type: ObservationType,
    /// Human-readable summary of the observation
    pub summary: String,
    /// When the observation was recorded
    pub timestamp: DateTime<Utc>,
    /// The tool that generated this observation
    pub tool_name: String,
}

/// Search result from [`Mind::search`].
#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub obs_type: ObservationType,
    pub summary: String,
    pub content_excerpt: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub score: f64,
    pub tool_name: String,
}

/// Central memory engine. One instance per `.mv2` file.
///
/// `Send + Sync` safe via internal `Mutex` on the backend. All mutating
/// operations (`remember`, `save_session_summary`) take `&self` — interior
/// mutability is provided by the backend's own `Mutex`.
pub struct Mind {
    backend: Box<dyn MemvidBackend>,
    config: MindConfig,
    session_id: String,
    memory_path: PathBuf,
    initialized: bool,
    cached_stats: Mutex<Option<MindStats>>,
}

// Compile-time assertion: Mind must be Send + Sync.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Mind>();
};

impl Mind {
    /// Current session identifier (ULID).
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Resolved path to the `.mv2` memory file.
    pub fn memory_path(&self) -> &Path {
        &self.memory_path
    }

    /// Whether the engine has been successfully opened.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Effective runtime config used by this mind instance.
    pub(crate) fn config(&self) -> &MindConfig {
        &self.config
    }
}

#[cfg(test)]
#[path = "../mind_tests.rs"]
mod tests;
