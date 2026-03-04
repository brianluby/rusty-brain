use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::error::HookError;

const TTL_SECONDS: i64 = 60;
const CACHE_FILENAME: &str = ".dedup-cache.json";

/// File-based deduplication cache for post-tool-use observations.
///
/// Entries expire after 60 seconds and are pruned on every read.
/// Stores only hashes (not content) for security (SEC-2).
pub struct DedupCache {
    cache_path: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct CacheData {
    entries: HashMap<String, i64>,
}

impl DedupCache {
    /// Create a new `DedupCache` for the given project directory.
    #[must_use]
    pub fn new(project_dir: &Path) -> Self {
        Self {
            cache_path: project_dir.join(".agent-brain").join(CACHE_FILENAME),
        }
    }

    /// Check if the given tool+summary combination was recorded within the last 60 seconds.
    ///
    /// Returns `true` if duplicate (should skip storage).
    /// On any error: returns `false` (fail-open).
    #[must_use]
    pub fn is_duplicate(&self, tool_name: &str, summary: &str) -> bool {
        let Ok(data) = self.read_cache() else {
            return false;
        };

        let key = Self::hash_key(tool_name, summary);
        let now = chrono::Utc::now().timestamp();

        data.entries
            .get(&key)
            .is_some_and(|&ts| (now - ts) < TTL_SECONDS)
    }

    /// Record a new tool+summary entry with the current timestamp.
    ///
    /// Prunes expired entries before writing. Uses atomic write (temp+rename).
    ///
    /// # Errors
    ///
    /// Returns `HookError::Dedup` on I/O or serialization failure.
    pub fn record(&self, tool_name: &str, summary: &str) -> Result<(), HookError> {
        let mut data = self.read_cache().unwrap_or_default();
        let now = chrono::Utc::now().timestamp();

        // Prune expired entries
        data.entries.retain(|_, ts| (now - *ts) < TTL_SECONDS);

        // Record new entry
        let key = Self::hash_key(tool_name, summary);
        data.entries.insert(key, now);

        self.write_cache(&data)
    }

    fn hash_key(tool_name: &str, summary: &str) -> String {
        let mut hasher = DefaultHasher::new();
        tool_name.hash(&mut hasher);
        summary.hash(&mut hasher);
        hasher.finish().to_string()
    }

    fn read_cache(&self) -> Result<CacheData, HookError> {
        let content = std::fs::read_to_string(&self.cache_path).map_err(|e| HookError::Dedup {
            message: format!("Failed to read cache: {e}"),
        })?;
        serde_json::from_str(&content).map_err(|e| HookError::Dedup {
            message: format!("Failed to parse cache: {e}"),
        })
    }

    fn write_cache(&self, data: &CacheData) -> Result<(), HookError> {
        // Ensure parent directory exists
        if let Some(parent) = self.cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HookError::Dedup {
                message: format!("Failed to create cache directory: {e}"),
            })?;
        }

        // Atomic write: write to temp file then rename
        let tmp_path = self.cache_path.with_extension("tmp");
        let json = serde_json::to_string(data).map_err(|e| HookError::Dedup {
            message: format!("Failed to serialize cache: {e}"),
        })?;
        std::fs::write(&tmp_path, &json).map_err(|e| HookError::Dedup {
            message: format!("Failed to write temp cache: {e}"),
        })?;
        if let Err(e) = std::fs::rename(&tmp_path, &self.cache_path) {
            let _ = std::fs::remove_file(&tmp_path); // Best-effort cleanup
            return Err(HookError::Dedup {
                message: format!("Failed to rename cache: {e}"),
            });
        }

        Ok(())
    }
}
