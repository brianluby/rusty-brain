//! `Mind` — the core memory engine for rusty-brain.
//!
//! Provides the public API for storing, searching, and retrieving observations
//! from a memvid-backed `.mv2` memory file.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::backend::{MemvidBackend, OpenAction};
use crate::file_guard;
use crate::memvid_store::MemvidStore;
use types::{
    InjectedContext, MindConfig, MindStats, ObservationMetadata, ObservationType, RustyBrainError,
    SessionSummary, error_codes,
};

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
    /// Open or create a memory file based on config.
    ///
    /// Flow: `FileGuard` validates → `MemvidStore` creates/opens → Mind initialized.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError` if file validation fails, the backend cannot
    /// create/open the file, or parent directory creation fails.
    #[tracing::instrument(skip(config), fields(path = %config.memory_path.display()))]
    pub fn open(config: MindConfig) -> Result<Mind, RustyBrainError> {
        config.validate()?;
        let memory_path = config.memory_path.clone();
        let action = file_guard::validate_and_open(&memory_path)?;

        let backend = Box::new(MemvidStore::new());
        match action {
            OpenAction::Create => {
                backend.create(&memory_path)?;
                set_file_permissions_0600(&memory_path);
            }
            OpenAction::Open => {
                if let Err(e) = backend.open(&memory_path) {
                    tracing::info!(error = %e, path = %memory_path.display(), "corrupted memory file detected, recovering");
                    file_guard::backup_and_prune(&memory_path, 3)?;
                    backend.create(&memory_path)?;
                    set_file_permissions_0600(&memory_path);
                }
            }
        }

        let session_id = ulid::Ulid::new().to_string().to_lowercase();

        tracing::info!(session_id = %session_id, "mind opened");

        Ok(Mind {
            backend,
            config,
            session_id,
            memory_path,
            initialized: true,
            cached_stats: Mutex::new(None),
        })
    }

    /// Open with a custom backend (for testing with `MockBackend`).
    #[cfg(test)]
    pub(crate) fn open_with_backend(
        config: MindConfig,
        backend: Box<dyn MemvidBackend>,
    ) -> Result<Mind, RustyBrainError> {
        let memory_path = config.memory_path.clone();

        // For test backend, call create if file doesn't exist, open if it does.
        if memory_path.exists() {
            backend.open(&memory_path)?;
        } else {
            backend.create(&memory_path)?;
        }

        let session_id = ulid::Ulid::new().to_string().to_lowercase();

        Ok(Mind {
            backend,
            config,
            session_id,
            memory_path,
            initialized: true,
            cached_stats: Mutex::new(None),
        })
    }

    /// Store an observation. Returns the observation's ULID string.
    ///
    /// Required: `obs_type`, `summary` (non-empty), `tool_name` (non-empty).
    /// Optional: `content`, `metadata`.
    /// Auto-generates: ULID id, UTC timestamp.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::InvalidInput` if `summary` or `tool_name` is
    /// empty or whitespace-only. Returns `RustyBrainError::Storage` if the
    /// backend write or commit fails.
    #[tracing::instrument(skip(self, content, metadata), fields(obs_type = %obs_type, tool = tool_name))]
    pub fn remember(
        &self,
        obs_type: ObservationType,
        tool_name: &str,
        summary: &str,
        content: Option<&str>,
        metadata: Option<&ObservationMetadata>,
    ) -> Result<String, RustyBrainError> {
        // Validate inputs.
        if summary.trim().is_empty() {
            return Err(RustyBrainError::InvalidInput {
                code: error_codes::E_INPUT_EMPTY_FIELD,
                message: "summary must not be empty".to_string(),
            });
        }
        if tool_name.trim().is_empty() {
            return Err(RustyBrainError::InvalidInput {
                code: error_codes::E_INPUT_EMPTY_FIELD,
                message: "tool_name must not be empty".to_string(),
            });
        }

        let id = ulid::Ulid::new().to_string().to_lowercase();
        let timestamp = Utc::now();

        // Build the text payload: summary + optional content.
        let payload = match content {
            Some(c) => format!("{summary}\n\n{c}"),
            None => summary.to_string(),
        };

        // Labels: observation type.
        let labels = vec![obs_type.to_string()];

        // Tags: tool_name + session_id.
        let tags = vec![tool_name.to_string(), self.session_id.clone()];

        // Metadata: full observation JSON for round-trip fidelity.
        let meta_json = serde_json::json!({
            "id": id,
            "obs_type": obs_type.to_string(),
            "tool_name": tool_name,
            "summary": summary,
            "content": content,
            "timestamp": timestamp.to_rfc3339(),
            "session_id": &self.session_id,
            "metadata": metadata,
        });

        self.backend
            .put(payload.as_bytes(), &labels, &tags, &meta_json)?;
        self.backend.commit()?;

        // Invalidate cached stats.
        if let Ok(mut cache) = self.cached_stats.lock() {
            *cache = None;
        }

        tracing::debug!(id = %id, "observation stored");

        Ok(id)
    }

    /// Search observations by text query. Returns results ranked by relevance.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend search fails.
    #[tracing::instrument(skip(self), fields(query_len = query.len()))]
    pub fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MemorySearchResult>, RustyBrainError> {
        let hits = self.backend.find(query, limit.unwrap_or(10))?;
        let results: Vec<_> = hits.iter().map(parse_search_hit).collect();
        tracing::debug!(result_count = results.len(), "search complete");
        Ok(results)
    }

    /// Ask a question against stored observations.
    ///
    /// Returns synthesized answer or "No relevant memories found."
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend ask fails.
    #[tracing::instrument(skip(self), fields(question_len = question.len()))]
    pub fn ask(&self, question: &str) -> Result<String, RustyBrainError> {
        let answer = self.backend.ask(question, 10)?;
        if answer.trim().is_empty() {
            Ok("No relevant memories found.".to_string())
        } else {
            Ok(answer)
        }
    }

    /// Store a session summary as a tagged, searchable observation.
    ///
    /// Returns the observation's ULID string. The summary is stored with
    /// `obs_type=Decision` and tagged as `"session_summary"` so it appears
    /// in future context injections via [`Mind::get_context`].
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::InvalidInput` if `summary` is empty.
    /// Returns `RustyBrainError::Storage` if the backend write fails.
    #[tracing::instrument(skip(self, decisions, files_modified))]
    pub fn save_session_summary(
        &self,
        decisions: Vec<String>,
        files_modified: Vec<String>,
        summary: &str,
    ) -> Result<String, RustyBrainError> {
        let now = Utc::now();

        // observation_count is 0: the summary captures decisions and files, not
        // individual observation counts which are tracked separately by stats().
        let session_summary = SessionSummary::new(
            self.session_id.clone(),
            now,
            now,
            0,
            decisions,
            files_modified,
            summary.to_string(),
        )?;

        let content_json =
            serde_json::to_string(&session_summary).map_err(|e| RustyBrainError::Storage {
                code: error_codes::E_STORAGE_BACKEND,
                message: "failed to serialize session summary".to_string(),
                source: Some(types::StorageSource(e.to_string())),
            })?;

        // Use "session_summary" as tool_name so it becomes a tag.
        // Prefix the display summary with "session_summary:" for text searchability.
        let store_summary = format!("session_summary: {summary}");

        self.remember(
            ObservationType::Decision,
            "session_summary",
            &store_summary,
            Some(&content_json),
            None,
        )
    }

    /// Compute memory statistics.
    ///
    /// Iterates all frames via timeline + `frame_by_id` to compute type counts,
    /// session counts, and timestamp range. Results are cached and returned on
    /// subsequent calls if the frame count has not changed.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend stats query fails.
    #[tracing::instrument(skip(self))]
    pub fn stats(&self) -> Result<MindStats, RustyBrainError> {
        let backend_stats = self.backend.stats()?;

        // Return cached stats if frame_count matches (no new observations).
        if let Ok(cache) = self.cached_stats.lock() {
            if let Some(ref s) = *cache {
                if s.total_observations == backend_stats.frame_count {
                    return Ok(s.clone());
                }
            }
        }

        // Compute enriched stats from timeline + frame_by_id.
        let timeline = self.backend.timeline(
            usize::try_from(backend_stats.frame_count).unwrap_or(usize::MAX),
            false,
        )?;

        let mut total_sessions: u64 = 0;
        let mut oldest_memory: Option<DateTime<Utc>> = None;
        let mut newest_memory: Option<DateTime<Utc>> = None;
        let mut type_counts: std::collections::HashMap<ObservationType, u64> =
            std::collections::HashMap::new();

        for entry in &timeline {
            let frame = self.backend.frame_by_id(entry.frame_id)?;

            // Parse obs_type from metadata.
            if let Some(obs_type_str) = frame.metadata.get("obs_type").and_then(|v| v.as_str()) {
                let obs_type: ObservationType =
                    obs_type_str.parse().unwrap_or(ObservationType::Discovery);
                *type_counts.entry(obs_type).or_insert(0) += 1;
            }

            // Count session summaries by tag.
            if frame.tags.iter().any(|t| t == "session_summary") {
                total_sessions += 1;
            }

            // Track timestamps for oldest/newest.
            if let Some(ts_str) = frame.metadata.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
                    let ts = dt.with_timezone(&Utc);
                    match oldest_memory {
                        None => oldest_memory = Some(ts),
                        Some(old) if ts < old => oldest_memory = Some(ts),
                        _ => {}
                    }
                    match newest_memory {
                        None => newest_memory = Some(ts),
                        Some(new) if ts > new => newest_memory = Some(ts),
                        _ => {}
                    }
                }
            }
        }

        let stats = MindStats {
            total_observations: backend_stats.frame_count,
            total_sessions,
            oldest_memory,
            newest_memory,
            file_size_bytes: backend_stats.file_size,
            type_counts,
        };

        if let Ok(mut cache) = self.cached_stats.lock() {
            *cache = Some(stats.clone());
        }

        tracing::debug!(
            observations = stats.total_observations,
            sessions = stats.total_sessions,
            "stats computed"
        );

        Ok(stats)
    }

    /// Assemble session context for agent startup.
    ///
    /// Combines: recent observations (timeline), relevant memories (find),
    /// session summaries (find). Bounded by token budget (chars / 4).
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if any backend operation fails.
    #[tracing::instrument(skip(self))]
    pub fn get_context(&self, query: Option<&str>) -> Result<InjectedContext, RustyBrainError> {
        let ctx = crate::context_builder::build(self.backend.as_ref(), &self.config, query)?;
        tracing::debug!(
            recent = ctx.recent_observations.len(),
            relevant = ctx.relevant_memories.len(),
            summaries = ctx.session_summaries.len(),
            tokens = ctx.token_count,
            "context assembled"
        );
        Ok(ctx)
    }

    /// Execute a closure while holding an exclusive file lock on the `.mv2` file.
    ///
    /// Uses `fs2::FileExt::try_lock_exclusive` with exponential backoff
    /// (100ms base, 5 retries, 2x multiplier). The lock file is created
    /// adjacent to the memory file with `.lock` extension and 0600 permissions.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::LockTimeout` if the lock cannot be acquired
    /// after all retries. Propagates any error from the closure.
    pub fn with_lock<F, T>(&self, f: F) -> Result<T, RustyBrainError>
    where
        F: FnOnce(&Self) -> Result<T, RustyBrainError>,
    {
        use fs2::FileExt;

        let mut lock_os = self.memory_path.as_os_str().to_os_string();
        lock_os.push(".lock");
        let lock_path = PathBuf::from(lock_os);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| RustyBrainError::FileSystem {
                code: error_codes::E_FS_IO_ERROR,
                message: format!("failed to open lock file: {}", lock_path.display()),
                source: Some(e),
            })?;

        // Set lock file permissions to 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&lock_path, perms);
        }

        // Exponential backoff: 100ms base, 5 retries, 2x multiplier.
        let max_retries: u32 = 5;
        for attempt in 0..=max_retries {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    let result = f(self);
                    // Dropping the file closes the fd, releasing the flock.
                    drop(lock_file);
                    return result;
                }
                Err(_) if attempt < max_retries => {
                    let delay = std::time::Duration::from_millis(100) * 2u32.pow(attempt);
                    std::thread::sleep(delay);
                }
                Err(_) => {
                    return Err(RustyBrainError::LockTimeout {
                        code: error_codes::E_LOCK_TIMEOUT,
                        message: format!(
                            "failed to acquire lock after {max_retries} retries: {}",
                            lock_path.display()
                        ),
                    });
                }
            }
        }

        unreachable!()
    }

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
}

/// Set file permissions to 0600 (owner read/write only) per SEC-1.
#[cfg(unix)]
fn set_file_permissions_0600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(path, perms);
}

#[cfg(not(unix))]
fn set_file_permissions_0600(_path: &Path) {}

/// Parse a backend `SearchHit` into a `MemorySearchResult`.
fn parse_search_hit(hit: &crate::backend::SearchHit) -> MemorySearchResult {
    let meta = &hit.metadata;

    let obs_type_str = meta["obs_type"].as_str().unwrap_or("discovery");
    let obs_type: ObservationType = obs_type_str.parse().unwrap_or(ObservationType::Discovery);

    let summary = meta["summary"].as_str().unwrap_or(&hit.text).to_string();

    let content_excerpt = meta["content"].as_str().map(|s| {
        if s.len() > 200 {
            // Find the nearest char boundary at or before byte 200.
            let mut end = 200;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &s[..end])
        } else {
            s.to_string()
        }
    });

    let timestamp = meta["timestamp"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));

    let tool_name = meta["tool_name"].as_str().unwrap_or("").to_string();

    MemorySearchResult {
        obs_type,
        summary,
        content_excerpt,
        timestamp,
        score: hit.score,
        tool_name,
    }
}

#[cfg(test)]
#[path = "mind_tests.rs"]
mod tests;
