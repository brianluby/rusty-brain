//! Short-TTL deduplication cache for PostToolUse observations.
//!
//! Each hook invocation is a fresh process, so dedup must be cross-process: the
//! cache is a small JSON file under the XDG cache dir, namespaced per project.
//! Entries expire after `TTL_SECONDS` and are pruned on every `record`. We store
//! only stable FNV-1a hashes of `(tool_name, summary)` — never raw content.
//!
//! Fail-open: every error (unreadable/corrupt/unwritable cache) degrades to
//! "not a duplicate" so capture never silently drops on cache trouble.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rb_types::Namespace;

const TTL_SECONDS: u64 = 60;

/// A file-backed, per-namespace dedup cache.
pub struct DedupCache {
    cache_path: PathBuf,
}

impl DedupCache {
    /// Resolve the per-namespace cache file under the XDG cache dir
    /// (`$XDG_CACHE_HOME/rusty-brain/` or `~/.cache/rusty-brain/`), falling back
    /// to the system temp dir if neither is set.
    pub fn for_namespace(namespace: &Namespace) -> Self {
        let dir = Self::cache_dir();
        let file = format!("dedup-{}.json", Self::namespace_slug(namespace));
        Self {
            cache_path: dir.join(file),
        }
    }

    /// Construct a cache at an explicit path (used by tests).
    pub fn at(cache_path: PathBuf) -> Self {
        Self { cache_path }
    }

    fn cache_dir() -> PathBuf {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("rusty-brain");
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            if !home.is_empty() {
                return PathBuf::from(home).join(".cache").join("rusty-brain");
            }
        }
        std::env::temp_dir().join("rusty-brain")
    }

    /// Turn a namespace into a filesystem-safe slug (alphanumerics kept, every
    /// other byte replaced by `_`). Keeps caches for distinct projects separate.
    fn namespace_slug(namespace: &Namespace) -> String {
        let raw = namespace.as_db_string();
        raw.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    }

    /// Stable FNV-1a 64-bit hash of `tool_name` + NUL + `summary`. Stable across
    /// processes and Rust versions (required: persisted to disk, read by a later
    /// invocation within the TTL window).
    fn hash_key(tool_name: &str, summary: &str) -> String {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = FNV_OFFSET;
        for byte in tool_name
            .as_bytes()
            .iter()
            .chain(b"\0")
            .chain(summary.as_bytes())
        {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash.to_string()
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// True if `(tool_name, summary)` was recorded within the last `TTL_SECONDS`.
    /// Any error (missing/corrupt cache) fails open to `false`.
    pub fn is_duplicate(&self, tool_name: &str, summary: &str) -> bool {
        let entries = self.read();
        let key = Self::hash_key(tool_name, summary);
        let now = Self::now_secs();
        entries
            .get(&key)
            .is_some_and(|&ts| now.saturating_sub(ts) < TTL_SECONDS)
    }

    /// Record `(tool_name, summary)` with the current timestamp, pruning expired
    /// entries first. Best-effort: write errors are swallowed (fail-open).
    pub fn record(&self, tool_name: &str, summary: &str) {
        let mut entries = self.read();
        let now = Self::now_secs();
        entries.retain(|_, ts| now.saturating_sub(*ts) < TTL_SECONDS);
        entries.insert(Self::hash_key(tool_name, summary), now);
        self.write(&entries);
    }

    /// Read the cache map; any error degrades to an empty map.
    fn read(&self) -> HashMap<String, u64> {
        match std::fs::read_to_string(&self.cache_path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    /// Atomically write the cache map (temp file + rename). Best-effort.
    fn write(&self, entries: &HashMap<String, u64>) {
        if let Some(parent) = self.cache_path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let Ok(json) = serde_json::to_string(entries) else {
            return;
        };
        let tmp = self
            .cache_path
            .with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, json.as_bytes()).is_err() {
            return;
        }
        if std::fs::rename(&tmp, &self.cache_path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Suppress the unused-import lint for `Path` in builds where only `PathBuf` is
/// referenced directly; `Path` is part of the documented public path API.
#[allow(dead_code)]
fn _path_marker(_: &Path) {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn hash_key_is_deterministic() {
        let a = DedupCache::hash_key("Edit", "Edited /src/main.rs");
        let b = DedupCache::hash_key("Edit", "Edited /src/main.rs");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_key_differs_by_tool_and_summary() {
        let base = DedupCache::hash_key("Edit", "summary");
        assert_ne!(base, DedupCache::hash_key("Write", "summary"));
        assert_ne!(base, DedupCache::hash_key("Edit", "other"));
    }

    #[test]
    fn fresh_cache_reports_no_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DedupCache::at(tmp.path().join("dedup.json"));
        assert!(!cache.is_duplicate("Edit", "Edited /src/main.rs"));
    }

    #[test]
    fn recorded_entry_is_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DedupCache::at(tmp.path().join("dedup.json"));
        cache.record("Edit", "Edited /src/main.rs");
        assert!(cache.is_duplicate("Edit", "Edited /src/main.rs"));
    }

    #[test]
    fn different_entry_is_not_duplicate_after_record() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DedupCache::at(tmp.path().join("dedup.json"));
        cache.record("Edit", "Edited /src/main.rs");
        assert!(!cache.is_duplicate("Write", "Wrote /src/lib.rs"));
    }

    #[test]
    fn expired_entry_is_not_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dedup.json");
        // Seed an entry whose timestamp is far in the past.
        let key = DedupCache::hash_key("Edit", "old");
        let mut entries = HashMap::new();
        entries.insert(key, 1_000u64); // ~1970, definitely expired
        let json = serde_json::to_string(&entries).unwrap();
        std::fs::write(&path, json).unwrap();

        let cache = DedupCache::at(path);
        assert!(!cache.is_duplicate("Edit", "old"));
    }

    #[test]
    fn corrupt_cache_fails_open_to_not_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dedup.json");
        std::fs::write(&path, b"{ not valid json").unwrap();
        let cache = DedupCache::at(path);
        assert!(!cache.is_duplicate("Edit", "anything"));
    }

    #[test]
    fn namespace_slug_is_filesystem_safe() {
        let slug = DedupCache::namespace_slug(&Namespace::Project("rusty/brain:1".into()));
        assert!(
            slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "slug must be fs-safe, got {slug}"
        );
    }
}
