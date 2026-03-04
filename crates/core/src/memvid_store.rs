//! Production [`MemvidBackend`](super::backend::MemvidBackend) implementation
//! wrapping `memvid-core`.
//!
//! All memvid types are consumed here and never cross the trait boundary.

use crate::backend::{BackendStats, FrameInfo, MemvidBackend, SearchHit, TimelineEntry};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Mutex;
use types::{RustyBrainError, error_codes};

/// Production backend wrapping a `memvid_core::Memvid` handle behind a `Mutex`.
///
/// The `Mutex` is necessary because most memvid operations require `&mut self`.
/// This enables `MemvidStore` to satisfy the `Send + Sync` bounds on
/// [`MemvidBackend`].
pub(crate) struct MemvidStore {
    inner: Mutex<Option<memvid_core::Memvid>>,
}

impl MemvidStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

/// Convert a [`memvid_core::MemvidError`] into a [`RustyBrainError::Storage`].
#[allow(clippy::needless_pass_by_value)] // Required by map_err signature.
fn wrap_memvid_error(err: memvid_core::MemvidError) -> RustyBrainError {
    RustyBrainError::Storage {
        code: error_codes::E_STORAGE_BACKEND,
        message: format!("memvid operation failed: {err}"),
        source: Some(types::StorageSource(format!("{err}"))),
    }
}

fn classify_open_error(err: &memvid_core::MemvidError, path: &Path) -> RustyBrainError {
    let msg = err.to_string();
    let lower = msg.to_lowercase();
    // High-confidence corruption indicators only. Generic tokens like "invalid",
    // "decode", "deserialize", "header" are excluded to avoid mislabelling
    // non-corruption errors (e.g. enable_lex failures) as CorruptedFile, which
    // would trigger destructive backup-and-recreate recovery.
    let corruption_markers = [
        "corrupt",
        "malformed",
        "checksum",
        "truncated",
        "unexpected eof",
        "bad magic",
        "not a memvid",
    ];

    if corruption_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        RustyBrainError::CorruptedFile {
            code: error_codes::E_STORAGE_CORRUPTED_FILE,
            message: format!("memory file appears corrupted: {} ({msg})", path.display()),
        }
    } else {
        RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: format!("failed to open memory file {}: {msg}", path.display()),
            source: Some(types::StorageSource(msg)),
        }
    }
}

/// Acquire the mutex guard, returning a storage error on poison.
fn lock_inner(
    inner: &Mutex<Option<memvid_core::Memvid>>,
) -> Result<std::sync::MutexGuard<'_, Option<memvid_core::Memvid>>, RustyBrainError> {
    inner.lock().map_err(|_| RustyBrainError::Storage {
        code: error_codes::E_STORAGE_BACKEND,
        message: "memvid mutex poisoned".to_string(),
        source: None,
    })
}

impl MemvidBackend for MemvidStore {
    fn create(&self, path: &Path) -> Result<(), RustyBrainError> {
        let mut mv = memvid_core::Memvid::create(path).map_err(wrap_memvid_error)?;
        // Ensure lexical indexing is enabled for search/ask support.
        mv.enable_lex().map_err(wrap_memvid_error)?;
        let mut guard = lock_inner(&self.inner)?;
        *guard = Some(mv);
        Ok(())
    }

    fn open(&self, path: &Path) -> Result<(), RustyBrainError> {
        let mut mv = memvid_core::Memvid::open(path).map_err(|e| classify_open_error(&e, path))?;
        // Ensure lexical indexing is enabled for search/ask support.
        // Uses wrap_memvid_error (not classify_open_error) because enable_lex
        // failures on an already-open handle are not corruption and must not
        // trigger destructive recovery via CorruptedFile.
        mv.enable_lex().map_err(wrap_memvid_error)?;
        let mut guard = lock_inner(&self.inner)?;
        *guard = Some(mv);
        Ok(())
    }

    fn put(
        &self,
        payload: &[u8],
        labels: &[String],
        tags: &[String],
        metadata: &serde_json::Value,
    ) -> Result<u64, RustyBrainError> {
        let mut guard = lock_inner(&self.inner)?;
        let mv = guard.as_mut().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized — call create or open first".to_string(),
            source: None,
        })?;

        let mut builder = memvid_core::PutOptionsBuilder::default();
        for label in labels {
            builder = builder.label(label);
        }
        for tag in tags {
            builder = builder.push_tag(tag);
        }
        // Store full observation metadata as a DocMetadata JSON value.
        // We serialize the serde_json::Value into DocMetadata via metadata_entry
        // to preserve all fields for round-trip fidelity.
        if let serde_json::Value::Object(map) = metadata {
            for (key, value) in map {
                builder = builder.metadata_entry(key, value.clone());
            }
        }
        // Disable heavy extraction features for agent memory (not documents).
        builder = builder
            .auto_tag(false)
            .extract_dates(false)
            .extract_triplets(false);

        let options = builder.build();
        let frame_id = mv
            .put_bytes_with_options(payload, options)
            .map_err(wrap_memvid_error)?;
        Ok(frame_id)
    }

    fn commit(&self) -> Result<(), RustyBrainError> {
        let mut guard = lock_inner(&self.inner)?;
        let mv = guard.as_mut().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized".to_string(),
            source: None,
        })?;
        mv.commit().map_err(wrap_memvid_error)
    }

    fn find(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, RustyBrainError> {
        let mut guard = lock_inner(&self.inner)?;
        let mv = guard.as_mut().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized".to_string(),
            source: None,
        })?;

        // Use the full `search()` API (Tantivy-backed) rather than `find()` / `search_lex()`
        // because the latter requires the legacy lex_index which isn't populated for
        // newly created files — only the Tantivy engine is active after `enable_lex()`.
        let request = memvid_core::SearchRequest {
            query: query.to_string(),
            top_k: limit,
            snippet_chars: 200,
            uri: None,
            scope: None,
            cursor: None,
            as_of_frame: None,
            as_of_ts: None,
            no_sketch: true,
            acl_context: None,
            acl_enforcement_mode: memvid_core::AclEnforcementMode::default(),
        };
        let response = mv.search(request).map_err(wrap_memvid_error)?;

        // Convert SearchHit (memvid) → our internal SearchHit.
        let mut results = Vec::with_capacity(response.hits.len());
        for hit in response.hits {
            let frame = mv.frame_by_id(hit.frame_id).map_err(wrap_memvid_error)?;
            let metadata = frame_extra_metadata_to_json(&frame);
            results.push(SearchHit {
                text: hit.text,
                score: hit.score.map_or(1.0, f64::from),
                metadata,
                labels: frame.labels.clone(),
                tags: frame.tags.clone(),
            });
        }
        Ok(results)
    }

    fn ask(&self, question: &str, limit: usize) -> Result<String, RustyBrainError> {
        let mut guard = lock_inner(&self.inner)?;
        let mv = guard.as_mut().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized".to_string(),
            source: None,
        })?;

        let request = memvid_core::AskRequest {
            question: question.to_string(),
            top_k: limit,
            snippet_chars: 500,
            uri: None,
            scope: None,
            cursor: None,
            start: None,
            end: None,
            context_only: true,
            mode: memvid_core::AskMode::Lex,
            as_of_frame: None,
            as_of_ts: None,
            adaptive: None,
            acl_context: None,
            acl_enforcement_mode: memvid_core::AclEnforcementMode::default(),
        };

        let response = mv
            .ask(request, None::<&dyn memvid_core::VecEmbedder>)
            .map_err(wrap_memvid_error)?;

        // Return the assembled context from context_fragments.
        // When context_only=true, answer is None and context is in fragments.
        let answer = response
            .context_fragments
            .iter()
            .map(|f| f.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(answer)
    }

    fn timeline(&self, limit: usize, reverse: bool) -> Result<Vec<TimelineEntry>, RustyBrainError> {
        let mut guard = lock_inner(&self.inner)?;
        let mv = guard.as_mut().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized".to_string(),
            source: None,
        })?;

        let Some(limit_nz) = NonZeroU64::new(u64::try_from(limit).unwrap_or(u64::MAX)) else {
            return Ok(Vec::new());
        };
        let query = memvid_core::TimelineQueryBuilder::default()
            .limit(limit_nz)
            .reverse(reverse)
            .build();

        let entries = mv.timeline(query).map_err(wrap_memvid_error)?;

        Ok(entries
            .into_iter()
            .map(|e| TimelineEntry {
                frame_id: e.frame_id,
                preview: e.preview,
                timestamp: Some(e.timestamp),
            })
            .collect())
    }

    fn frame_by_id(&self, frame_id: u64) -> Result<FrameInfo, RustyBrainError> {
        let guard = lock_inner(&self.inner)?;
        let mv = guard.as_ref().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized".to_string(),
            source: None,
        })?;

        let frame = mv.frame_by_id(frame_id).map_err(wrap_memvid_error)?;
        let metadata = frame_extra_metadata_to_json(&frame);

        Ok(FrameInfo {
            labels: frame.labels.clone(),
            tags: frame.tags.clone(),
            metadata,
            timestamp: Some(frame.timestamp),
        })
    }

    fn stats(&self) -> Result<BackendStats, RustyBrainError> {
        let guard = lock_inner(&self.inner)?;
        let mv = guard.as_ref().ok_or_else(|| RustyBrainError::Storage {
            code: error_codes::E_STORAGE_BACKEND,
            message: "memvid handle not initialized".to_string(),
            source: None,
        })?;

        let s = mv.stats().map_err(wrap_memvid_error)?;
        Ok(BackendStats {
            frame_count: s.frame_count,
            file_size: s.size_bytes,
        })
    }
}

/// Convert a `Frame`'s `extra_metadata` (`BTreeMap<String, String>`) to a `serde_json::Value`.
fn frame_extra_metadata_to_json(frame: &memvid_core::Frame) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = frame
        .extra_metadata
        .iter()
        .map(|(k, v)| {
            // Try to parse the value as JSON first (for nested objects/arrays).
            let json_val =
                serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.clone()));
            (k.clone(), json_val)
        })
        .collect();
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: memvid-core `put_bytes` returns a WAL sequence number, not the
    // materialized frame index. After `commit()`, frames are assigned 0-based
    // IDs in `toc.frames`. Use `timeline()` to discover actual frame IDs.

    #[test]
    fn memvid_store_create_put_find_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.mv2");

        let store = MemvidStore::new();
        store.create(&path).unwrap();

        let payload = b"Found a caching pattern in the service layer";
        let labels = vec!["discovery".to_string()];
        let tags = vec!["Read".to_string(), "session-abc".to_string()];
        let metadata = serde_json::json!({"summary": "Found caching pattern"});

        store.put(payload, &labels, &tags, &metadata).unwrap();
        store.commit().unwrap();

        let hits = store.find("caching pattern", 10).unwrap();
        assert!(!hits.is_empty(), "expected at least one search hit");
        assert!(
            hits[0].text.contains("caching"),
            "hit text should contain query term"
        );
        assert_eq!(hits[0].labels, labels);
        assert_eq!(hits[0].tags, tags);
    }

    #[test]
    fn memvid_store_timeline_ordering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timeline.mv2");

        let store = MemvidStore::new();
        store.create(&path).unwrap();

        store
            .put(b"first entry", &[], &[], &serde_json::json!({}))
            .unwrap();
        store
            .put(b"second entry", &[], &[], &serde_json::json!({}))
            .unwrap();
        store
            .put(b"third entry", &[], &[], &serde_json::json!({}))
            .unwrap();
        store.commit().unwrap();

        // Forward order — 3 frames in insertion order
        let entries = store.timeline(10, false).unwrap();
        assert_eq!(entries.len(), 3);
        // Timeline frame_ids should be monotonically increasing
        assert!(entries[0].frame_id < entries[1].frame_id);
        assert!(entries[1].frame_id < entries[2].frame_id);

        // Reverse order
        let rev_entries = store.timeline(10, true).unwrap();
        assert_eq!(rev_entries.len(), 3);
        assert_eq!(rev_entries[0].frame_id, entries[2].frame_id);
        assert_eq!(rev_entries[2].frame_id, entries[0].frame_id);
    }

    #[test]
    fn memvid_store_frame_by_id_metadata_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame_meta.mv2");

        let store = MemvidStore::new();
        store.create(&path).unwrap();

        let labels = vec!["decision".to_string()];
        let tags = vec!["Write".to_string()];
        let metadata = serde_json::json!({"key": "value", "nested": {"a": 1}});

        store.put(b"test data", &labels, &tags, &metadata).unwrap();
        store.commit().unwrap();

        // Use timeline to discover the actual materialized frame ID.
        let entries = store.timeline(1, false).unwrap();
        assert_eq!(entries.len(), 1);
        let info = store.frame_by_id(entries[0].frame_id).unwrap();
        assert_eq!(info.labels, labels);
        assert_eq!(info.tags, tags);
        assert!(info.timestamp.is_some());
        // Verify metadata round-trip (stored as extra_metadata strings, parsed back)
        assert_eq!(info.metadata["key"], serde_json::json!("value"));
    }

    #[test]
    fn memvid_store_stats() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.mv2");

        let store = MemvidStore::new();
        store.create(&path).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.frame_count, 0);

        store
            .put(b"hello", &[], &[], &serde_json::json!({}))
            .unwrap();
        store
            .put(b"world!", &[], &[], &serde_json::json!({}))
            .unwrap();
        store.commit().unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.frame_count, 2);
        assert!(stats.file_size > 0);
    }

    #[test]
    fn memvid_store_commit_reopen_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("persist.mv2");

        // Create and populate
        {
            let store = MemvidStore::new();
            store.create(&path).unwrap();
            store
                .put(
                    b"persistent data across sessions",
                    &["discovery".to_string()],
                    &["session-1".to_string()],
                    &serde_json::json!({"key": "persisted"}),
                )
                .unwrap();
            store.commit().unwrap();
            // store is dropped here, releasing the file lock
        }

        // Reopen and verify
        {
            let store = MemvidStore::new();
            store.open(&path).unwrap();

            let stats = store.stats().unwrap();
            assert_eq!(stats.frame_count, 1, "frame should persist after reopen");

            let hits = store.find("persistent data", 10).unwrap();
            assert!(!hits.is_empty(), "data should be searchable after reopen");
            assert_eq!(hits[0].labels, vec!["discovery".to_string()]);
            assert_eq!(hits[0].tags, vec!["session-1".to_string()]);
        }
    }

    #[test]
    fn memvid_store_ask_returns_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ask.mv2");

        let store = MemvidStore::new();
        store.create(&path).unwrap();

        store
            .put(
                b"caching is done via LRU in the service layer",
                &[],
                &[],
                &serde_json::json!({}),
            )
            .unwrap();
        store.commit().unwrap();

        let answer = store.ask("caching", 10).unwrap();
        assert!(
            answer.contains("caching") || answer.contains("LRU"),
            "ask should return relevant content, got: {answer}"
        );
    }

    #[test]
    fn memvid_store_operations_on_uninitialized_return_error() {
        let store = MemvidStore::new();
        // All operations on an uninitialized store should return errors, not panic.
        assert!(store.commit().is_err());
        assert!(store.find("query", 10).is_err());
        assert!(store.ask("question", 10).is_err());
        assert!(store.timeline(10, false).is_err());
        assert!(store.frame_by_id(0).is_err());
        assert!(store.stats().is_err());
        assert!(
            store
                .put(b"data", &[], &[], &serde_json::json!({}))
                .is_err()
        );
    }
}
