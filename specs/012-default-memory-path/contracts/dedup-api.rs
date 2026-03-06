// Contract: Updated DedupCache API
// Feature: 012-default-memory-path

use std::path::{Path, PathBuf};

/// File-based deduplication cache for post-tool-use observations.
///
/// Cache path updated from `.agent-brain/.dedup-cache.json` to
/// `.rusty-brain/.dedup-cache.json`.
pub struct DedupCache {
    cache_path: PathBuf,
}

impl DedupCache {
    /// Create a new `DedupCache` for the given project directory.
    ///
    /// Cache file is stored at `<project_dir>/.rusty-brain/.dedup-cache.json`.
    #[must_use]
    pub fn new(project_dir: &Path) -> Self {
        Self {
            cache_path: project_dir.join(".rusty-brain").join(".dedup-cache.json"),
        }
    }
}
