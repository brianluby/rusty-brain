use std::collections::HashMap;
use std::fs::OpenOptions;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::HookError;

const TTL_SECONDS: i64 = 60;
const CACHE_FILENAME: &str = ".dedup-cache.json";

/// File-based deduplication cache for post-tool-use observations.
///
/// Entries expire after 60 seconds and are pruned on every write (in `record`).
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

    fn lock_path(&self) -> PathBuf {
        let mut lock_os = self.cache_path.as_os_str().to_os_string();
        lock_os.push(".lock");
        PathBuf::from(lock_os)
    }

    fn with_lock<T>(&self, f: impl FnOnce() -> Result<T, HookError>) -> Result<T, HookError> {
        let lock_path = self.lock_path();
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HookError::Dedup {
                message: format!("Failed to create cache directory: {e}"),
            })?;
        }

        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| HookError::Dedup {
                message: format!("Failed to open cache lock file: {e}"),
            })?;

        // Non-blocking lock with bounded retries (fail-open on timeout).
        // Prevents indefinite stalls from stuck lock holders.
        let max_retries: u32 = 3;
        let mut acquired = false;
        for attempt in 0..=max_retries {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    acquired = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && attempt < max_retries => {
                    std::thread::sleep(std::time::Duration::from_millis(
                        50 * u64::from(attempt + 1),
                    ));
                }
                Err(_) => break,
            }
        }

        if !acquired {
            return Err(HookError::Dedup {
                message: "Failed to acquire cache lock (timeout)".to_string(),
            });
        }

        let result = f();
        drop(lock_file); // Releases the exclusive lock by closing the fd
        result
    }

    /// Check if the given tool+summary combination was recorded within the last 60 seconds.
    ///
    /// Returns `true` if duplicate (should skip storage).
    /// On any error: returns `false` (fail-open).
    #[must_use]
    pub fn is_duplicate(&self, tool_name: &str, summary: &str) -> bool {
        self.with_lock(|| {
            let data = self.read_cache().unwrap_or_default();
            let key = Self::hash_key(tool_name, summary);
            let now = chrono::Utc::now().timestamp();

            Ok(data
                .entries
                .get(&key)
                .is_some_and(|&ts| (now - ts) < TTL_SECONDS))
        })
        .unwrap_or(false)
    }

    /// Record a new tool+summary entry with the current timestamp.
    ///
    /// Prunes expired entries before writing. Uses atomic write (temp+rename).
    ///
    /// # Errors
    ///
    /// Returns `HookError::Dedup` on I/O or serialization failure.
    pub fn record(&self, tool_name: &str, summary: &str) -> Result<(), HookError> {
        self.with_lock(|| {
            let mut data = self.read_cache().unwrap_or_default();
            let now = chrono::Utc::now().timestamp();

            // Prune expired entries
            data.entries.retain(|_, ts| (now - *ts) < TTL_SECONDS);

            // Record new entry
            let key = Self::hash_key(tool_name, summary);
            data.entries.insert(key, now);

            self.write_cache(&data)
        })
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

        // Atomic write: write to temp file then rename (unique name avoids collisions)
        let tmp_path = self
            .cache_path
            .with_extension(format!("tmp.{}", std::process::id()));
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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // DedupCache::new
    // -----------------------------------------------------------------------

    #[test]
    fn new_sets_cache_path_under_agent_brain_dir() {
        let cache = DedupCache::new(Path::new("/tmp/project"));
        assert_eq!(
            cache.cache_path,
            PathBuf::from("/tmp/project/.agent-brain/.dedup-cache.json")
        );
    }

    // -----------------------------------------------------------------------
    // hash_key — deterministic
    // -----------------------------------------------------------------------

    #[test]
    fn hash_key_is_deterministic() {
        let key1 = DedupCache::hash_key("Read", "Read /src/main.rs");
        let key2 = DedupCache::hash_key("Read", "Read /src/main.rs");
        assert_eq!(key1, key2, "same inputs should produce same hash");
    }

    #[test]
    fn hash_key_differs_for_different_tools() {
        let key1 = DedupCache::hash_key("Read", "same summary");
        let key2 = DedupCache::hash_key("Write", "same summary");
        assert_ne!(
            key1, key2,
            "different tools should produce different hashes"
        );
    }

    #[test]
    fn hash_key_differs_for_different_summaries() {
        let key1 = DedupCache::hash_key("Read", "summary A");
        let key2 = DedupCache::hash_key("Read", "summary B");
        assert_ne!(
            key1, key2,
            "different summaries should produce different hashes"
        );
    }

    // -----------------------------------------------------------------------
    // is_duplicate — fresh cache
    // -----------------------------------------------------------------------

    #[test]
    fn is_duplicate_returns_false_for_fresh_cache() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let cache = DedupCache::new(tmp.path());
        assert!(
            !cache.is_duplicate("Read", "Read /src/main.rs"),
            "fresh cache should not report duplicates"
        );
    }

    // -----------------------------------------------------------------------
    // record + is_duplicate
    // -----------------------------------------------------------------------

    #[test]
    fn record_and_is_duplicate_detects_recent_entry() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let cache = DedupCache::new(tmp.path());

        cache
            .record("Read", "Read /src/main.rs")
            .expect("record should succeed");
        assert!(
            cache.is_duplicate("Read", "Read /src/main.rs"),
            "recently recorded entry should be duplicate"
        );
    }

    #[test]
    fn record_does_not_mark_different_entry_as_duplicate() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let cache = DedupCache::new(tmp.path());

        cache
            .record("Read", "Read /src/main.rs")
            .expect("record should succeed");
        assert!(
            !cache.is_duplicate("Write", "Wrote /src/main.rs"),
            "different entry should not be duplicate"
        );
    }

    // -----------------------------------------------------------------------
    // write_cache / read_cache round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn write_and_read_cache_round_trips() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let cache = DedupCache::new(tmp.path());

        let mut data = CacheData::default();
        let now = chrono::Utc::now().timestamp();
        data.entries.insert("test_key".to_string(), now);

        cache
            .write_cache(&data)
            .expect("write_cache should succeed");
        let read_back = cache.read_cache().expect("read_cache should succeed");
        assert_eq!(read_back.entries.get("test_key"), Some(&now));
    }

    // -----------------------------------------------------------------------
    // read_cache — missing file
    // -----------------------------------------------------------------------

    #[test]
    fn read_cache_returns_error_for_missing_file() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let cache = DedupCache::new(tmp.path());
        let result = cache.read_cache();
        assert!(
            result.is_err(),
            "read_cache should error when file is missing"
        );
    }

    // -----------------------------------------------------------------------
    // lock_path
    // -----------------------------------------------------------------------

    #[test]
    fn lock_path_appends_lock_extension() {
        let cache = DedupCache::new(Path::new("/tmp/project"));
        let lock = cache.lock_path();
        assert_eq!(
            lock.extension().and_then(|e| e.to_str()),
            Some("lock"),
            "lock path should have .lock extension"
        );
    }
}
