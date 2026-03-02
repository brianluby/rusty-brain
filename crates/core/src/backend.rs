//! Storage abstraction hiding memvid-core behind a clean trait boundary.
//!
//! The [`MemvidBackend`] trait defines all storage operations. Concrete
//! implementations ([`super::memvid_store::MemvidStore`] for production,
//! [`MockBackend`] for tests) live behind this interface so upstream memvid
//! changes never ripple into the public API.

use std::path::Path;
use types::RustyBrainError;

/// Internal search hit from backend.
pub(crate) struct SearchHit {
    pub text: String,
    pub score: f64,
    pub metadata: serde_json::Value,
    #[allow(dead_code)] // Populated by backend for round-trip fidelity; not yet consumed.
    pub labels: Vec<String>,
    pub tags: Vec<String>,
}

/// Internal timeline entry from backend.
pub(crate) struct TimelineEntry {
    pub frame_id: u64,
    #[allow(dead_code)] // Populated by backend; reserved for future timeline display.
    pub preview: String,
    #[allow(dead_code)] // Populated by backend; reserved for future timeline display.
    pub timestamp: Option<i64>,
}

/// Internal frame metadata from backend.
pub(crate) struct FrameInfo {
    #[allow(dead_code)] // Populated by backend for round-trip fidelity; not yet consumed.
    pub labels: Vec<String>,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    #[allow(dead_code)] // Populated by backend; stats uses metadata timestamps instead.
    pub timestamp: Option<i64>,
}

/// Internal storage statistics from backend.
pub(crate) struct BackendStats {
    pub frame_count: u64,
    pub file_size: u64,
}

/// Actions recommended by `FileGuard` after pre-open validation.
#[derive(Debug)]
pub(crate) enum OpenAction {
    /// No file exists; create new.
    Create,
    /// File exists and passes all guards.
    Open,
}

/// Storage abstraction hiding memvid-core behind a clean boundary.
/// All memvid types stay behind this trait — never cross into public API.
pub(crate) trait MemvidBackend: Send + Sync {
    /// Create a new .mv2 file at path.
    fn create(&self, path: &Path) -> Result<(), RustyBrainError>;

    /// Open an existing .mv2 file at path.
    fn open(&self, path: &Path) -> Result<(), RustyBrainError>;

    /// Store data as a frame. Returns frame ID.
    fn put(
        &self,
        payload: &[u8],
        labels: &[String],
        tags: &[String],
        metadata: &serde_json::Value,
    ) -> Result<u64, RustyBrainError>;

    /// Commit pending writes to disk.
    fn commit(&self) -> Result<(), RustyBrainError>;

    /// Lexical search. Returns matching hits with scores.
    fn find(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, RustyBrainError>;

    /// Question-answering against stored content.
    fn ask(&self, question: &str, limit: usize) -> Result<String, RustyBrainError>;

    /// Timeline query: recent frames.
    fn timeline(&self, limit: usize, reverse: bool) -> Result<Vec<TimelineEntry>, RustyBrainError>;

    /// Get full frame metadata by ID.
    fn frame_by_id(&self, frame_id: u64) -> Result<FrameInfo, RustyBrainError>;

    /// Get storage statistics.
    fn stats(&self) -> Result<BackendStats, RustyBrainError>;
}

/// In-memory mock implementation of [`MemvidBackend`] for testing.
///
/// Stores frames in a `Vec` behind a `Mutex`, supporting put, find, timeline,
/// `frame_by_id`, stats, commit, and ask operations.
#[cfg(test)]
pub(crate) struct MockBackend {
    frames: std::sync::Mutex<Vec<MockFrame>>,
}

#[cfg(test)]
struct MockFrame {
    id: u64,
    payload: Vec<u8>,
    labels: Vec<String>,
    tags: Vec<String>,
    metadata: serde_json::Value,
    timestamp: i64,
}

#[cfg(test)]
impl MockBackend {
    pub fn new() -> Self {
        Self {
            frames: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl MemvidBackend for MockBackend {
    fn create(&self, _path: &Path) -> Result<(), RustyBrainError> {
        Ok(())
    }

    fn open(&self, _path: &Path) -> Result<(), RustyBrainError> {
        Ok(())
    }

    fn put(
        &self,
        payload: &[u8],
        labels: &[String],
        tags: &[String],
        metadata: &serde_json::Value,
    ) -> Result<u64, RustyBrainError> {
        let mut frames = self.frames.lock().unwrap();
        let id = u64::try_from(frames.len()).expect("frame count exceeds u64");
        frames.push(MockFrame {
            id,
            payload: payload.to_vec(),
            labels: labels.to_vec(),
            tags: tags.to_vec(),
            metadata: metadata.clone(),
            timestamp: chrono::Utc::now().timestamp(),
        });
        Ok(id)
    }

    fn commit(&self) -> Result<(), RustyBrainError> {
        Ok(())
    }

    fn find(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, RustyBrainError> {
        let frames = self.frames.lock().unwrap();
        let query_lower = query.to_lowercase();
        let mut hits: Vec<SearchHit> = frames
            .iter()
            .filter_map(|f| {
                let text = String::from_utf8_lossy(&f.payload);
                if text.to_lowercase().contains(&query_lower) {
                    Some(SearchHit {
                        text: text.into_owned(),
                        score: 1.0,
                        metadata: f.metadata.clone(),
                        labels: f.labels.clone(),
                        tags: f.tags.clone(),
                    })
                } else {
                    None
                }
            })
            .collect();
        hits.truncate(limit);
        Ok(hits)
    }

    fn ask(&self, question: &str, limit: usize) -> Result<String, RustyBrainError> {
        let results = self.find(question, limit)?;
        if results.is_empty() {
            Ok(String::new())
        } else {
            Ok(results
                .iter()
                .map(|h| h.text.clone())
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }

    fn timeline(&self, limit: usize, reverse: bool) -> Result<Vec<TimelineEntry>, RustyBrainError> {
        let frames = self.frames.lock().unwrap();
        let mut entries: Vec<TimelineEntry> = frames
            .iter()
            .map(|f| TimelineEntry {
                frame_id: f.id,
                preview: String::from_utf8_lossy(&f.payload)
                    .chars()
                    .take(100)
                    .collect(),
                timestamp: Some(f.timestamp),
            })
            .collect();
        if reverse {
            entries.reverse();
        }
        entries.truncate(limit);
        Ok(entries)
    }

    fn frame_by_id(&self, frame_id: u64) -> Result<FrameInfo, RustyBrainError> {
        let frames = self.frames.lock().unwrap();
        frames
            .iter()
            .find(|f| f.id == frame_id)
            .map(|f| FrameInfo {
                labels: f.labels.clone(),
                tags: f.tags.clone(),
                metadata: f.metadata.clone(),
                timestamp: Some(f.timestamp),
            })
            .ok_or_else(|| RustyBrainError::Storage {
                code: types::error_codes::E_STORAGE_BACKEND,
                message: format!("frame {frame_id} not found"),
                source: None,
            })
    }

    fn stats(&self) -> Result<BackendStats, RustyBrainError> {
        let frames = self.frames.lock().unwrap();
        Ok(BackendStats {
            frame_count: frames.len() as u64,
            file_size: frames.iter().map(|f| f.payload.len() as u64).sum(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_backend_put_find_round_trip() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        let payload = b"Found a caching pattern in the service layer";
        let labels = vec!["discovery".to_string()];
        let tags = vec!["Read".to_string()];
        let metadata = serde_json::json!({"summary": "Found caching pattern"});

        let frame_id = backend.put(payload, &labels, &tags, &metadata).unwrap();
        assert_eq!(frame_id, 0);

        let hits = backend.find("caching pattern", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("caching pattern"));
        assert_eq!(hits[0].labels, labels);
        assert_eq!(hits[0].tags, tags);
    }

    #[test]
    fn mock_backend_timeline_ordering() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        backend
            .put(b"first", &[], &[], &serde_json::json!({}))
            .unwrap();
        backend
            .put(b"second", &[], &[], &serde_json::json!({}))
            .unwrap();
        backend
            .put(b"third", &[], &[], &serde_json::json!({}))
            .unwrap();

        // Forward order
        let entries = backend.timeline(10, false).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].frame_id, 0);
        assert_eq!(entries[2].frame_id, 2);

        // Reverse order
        let entries = backend.timeline(10, true).unwrap();
        assert_eq!(entries[0].frame_id, 2);
        assert_eq!(entries[2].frame_id, 0);

        // Limit
        let entries = backend.timeline(2, false).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn mock_backend_frame_by_id_retrieval() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        let metadata = serde_json::json!({"key": "value"});
        let labels = vec!["decision".to_string()];
        let tags = vec!["Write".to_string(), "session-1".to_string()];
        backend
            .put(b"test data", &labels, &tags, &metadata)
            .unwrap();

        let info = backend.frame_by_id(0).unwrap();
        assert_eq!(info.labels, labels);
        assert_eq!(info.tags, tags);
        assert_eq!(info.metadata, metadata);
    }

    #[test]
    fn mock_backend_frame_by_id_missing_returns_error() {
        let backend = MockBackend::new();
        let result = backend.frame_by_id(999);
        assert!(result.is_err());
    }

    #[test]
    fn mock_backend_stats_computation() {
        let backend = MockBackend::new();
        backend.create(Path::new("/tmp/test.mv2")).unwrap();

        let stats = backend.stats().unwrap();
        assert_eq!(stats.frame_count, 0);
        assert_eq!(stats.file_size, 0);

        backend
            .put(b"hello", &[], &[], &serde_json::json!({}))
            .unwrap();
        backend
            .put(b"world!", &[], &[], &serde_json::json!({}))
            .unwrap();

        let stats = backend.stats().unwrap();
        assert_eq!(stats.frame_count, 2);
        assert_eq!(stats.file_size, 11); // 5 + 6
    }

    #[test]
    fn mock_backend_find_empty_results() {
        let backend = MockBackend::new();
        let hits = backend.find("nonexistent", 10).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn mock_backend_ask_returns_content() {
        let backend = MockBackend::new();
        backend
            .put(b"caching is done via LRU", &[], &[], &serde_json::json!({}))
            .unwrap();

        let answer = backend.ask("caching", 10).unwrap();
        assert!(answer.contains("caching"));
    }

    #[test]
    fn mock_backend_ask_no_matches_returns_empty() {
        let backend = MockBackend::new();
        let answer = backend.ask("nonexistent", 10).unwrap();
        assert!(answer.is_empty());
    }
}
