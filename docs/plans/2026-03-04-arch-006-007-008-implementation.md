# RB-ARCH-006/007/008 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Cap CLI over-fetch, decompose Mind into focused modules, and remove the unused singleton API.

**Architecture:** Three sequential changes: (1) Replace `usize::MAX` with bounded over-fetch in CLI filtered queries, (2) Split `mind.rs` into a `mind/` directory with 5 submodules while preserving the public API, (3) Remove the dead `get_mind`/`reset_mind` singleton from `lib.rs`.

**Tech Stack:** Rust stable (edition 2024), existing workspace crates (core, types, cli).

---

### Task 1: RB-ARCH-006 — Cap Over-Fetch in Filtered CLI Queries

**Files:**
- Modify: `crates/cli/src/commands.rs:22-26` (run_find)
- Modify: `crates/cli/src/commands.rs:93-97` (run_timeline)

**Step 1: Run existing tests to establish baseline**

Run: `cargo test --package rusty-brain-cli`
Expected: All tests PASS

**Step 2: Update run_find fetch_limit**

In `crates/cli/src/commands.rs`, replace:

```rust
    let fetch_limit = if type_filter.is_some() {
        usize::MAX
    } else {
        limit
    };
```

(in `run_find`, lines 22-26) with:

```rust
    let fetch_limit = if type_filter.is_some() {
        limit.saturating_mul(10)
    } else {
        limit
    };
```

**Step 3: Update run_timeline fetch_limit**

In `crates/cli/src/commands.rs`, replace the same pattern in `run_timeline` (lines 93-97):

```rust
    let fetch_limit = if type_filter.is_some() {
        usize::MAX
    } else {
        limit
    };
```

with:

```rust
    let fetch_limit = if type_filter.is_some() {
        limit.saturating_mul(10)
    } else {
        limit
    };
```

**Step 4: Run tests to verify no regressions**

Run: `cargo test --package rusty-brain-cli`
Expected: All tests PASS (including `test_find_type_filter`, `test_timeline_type_filter`, and the `*_applies_before_final_limit` tests)

**Step 5: Commit**

```bash
git add crates/cli/src/commands.rs
git commit -m "perf: cap over-fetch at limit*10 for type-filtered CLI queries (RB-ARCH-006)"
```

---

### Task 2: RB-ARCH-007 — Create mind/ directory and mod.rs

**Files:**
- Create: `crates/core/src/mind/mod.rs`

**Step 1: Run full core test suite to establish baseline**

Run: `cargo test --package rusty-brain-core`
Expected: All tests PASS

**Step 2: Create the mind/ directory**

Run: `mkdir -p crates/core/src/mind`

**Step 3: Create mod.rs with Mind struct, types, accessors, and test reference**

Create `crates/core/src/mind/mod.rs`:

```rust
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
```

Note: This won't compile yet — the submodule files don't exist. That's OK, we create them in Tasks 3-7.

---

### Task 3: RB-ARCH-007 — Create mind/lifecycle.rs

**Files:**
- Create: `crates/core/src/mind/lifecycle.rs`

**Step 1: Create lifecycle.rs with open, open_read_only, open_with_backend, recovery helpers, and permissions**

Create `crates/core/src/mind/lifecycle.rs`:

```rust
//! Mind lifecycle: open, create, recovery, and permissions.

use std::path::Path;
use std::sync::Mutex;

use crate::backend::{MemvidBackend, OpenAction};
use crate::file_guard;
use crate::memvid_store::MemvidStore;
use types::{MindConfig, RustyBrainError, error_codes};

use super::Mind;

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

        if config.debug {
            tracing::debug!(path = %memory_path.display(), "mind debug mode enabled");
        }

        let backend = Box::new(MemvidStore::new());
        match action {
            OpenAction::Create => {
                backend.create(&memory_path)?;
                set_file_permissions_0600(&memory_path)?;
            }
            OpenAction::Open => {
                if let Err(e) = backend.open(&memory_path) {
                    if should_recover_from_open_error(&e) {
                        tracing::info!(error = %e, path = %memory_path.display(), "corrupted memory file detected, recovering");
                        file_guard::backup_and_prune(&memory_path, 3)?;
                        backend.create(&memory_path)?;
                        set_file_permissions_0600(&memory_path)?;
                    } else {
                        return Err(e);
                    }
                } else {
                    // Harden pre-existing stores on normal open as well.
                    set_file_permissions_0600(&memory_path)?;
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

    /// Open an existing memory file in read-only mode (no recovery).
    ///
    /// Unlike [`Mind::open`], this method never creates files, never backs up
    /// corrupted files, and never performs destructive recovery. If the file
    /// cannot be opened (e.g. corrupted), the backend error is returned
    /// directly. Intended for read-only CLI access.
    ///
    /// The caller is expected to pre-validate that the file exists and is a
    /// regular file before calling this method.
    ///
    /// # Errors
    ///
    /// Returns backend-open errors directly (including corruption, permission,
    /// and version errors) plus config/file validation errors.
    #[tracing::instrument(skip(config), fields(path = %config.memory_path.display()))]
    pub fn open_read_only(config: MindConfig) -> Result<Mind, RustyBrainError> {
        config.validate()?;
        let memory_path = config.memory_path.clone();
        file_guard::validate_existing(&memory_path)?;

        let backend = Box::new(MemvidStore::new());
        backend.open(&memory_path)?;

        let session_id = ulid::Ulid::new().to_string().to_lowercase();

        tracing::info!(session_id = %session_id, "mind opened (read-only)");

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
}

fn should_recover_from_open_error(err: &RustyBrainError) -> bool {
    matches!(
        err,
        RustyBrainError::CorruptedFile { .. } | RustyBrainError::MemoryCorruption { .. }
    )
}

/// Set file permissions to 0600 (owner read/write only) per SEC-1.
///
/// # Errors
///
/// Returns `RustyBrainError::FileSystem` if the permission change fails.
#[cfg(unix)]
fn set_file_permissions_0600(path: &Path) -> Result<(), RustyBrainError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms).map_err(|e| RustyBrainError::FileSystem {
        code: error_codes::E_FS_IO_ERROR,
        message: format!("failed to set file permissions: {}", path.display()),
        source: Some(e),
    })
}

#[cfg(not(unix))]
fn set_file_permissions_0600(_path: &Path) -> Result<(), RustyBrainError> {
    Ok(())
}
```

---

### Task 4: RB-ARCH-007 — Create mind/locking.rs

**Files:**
- Create: `crates/core/src/mind/locking.rs`

**Step 1: Create locking.rs with with_lock, ReentrantLockGuard, and IN_FILE_LOCK**

Create `crates/core/src/mind/locking.rs`:

```rust
//! Cross-process file locking for Mind.

use std::cell::Cell;
use std::path::PathBuf;

use types::{RustyBrainError, error_codes};

use super::Mind;

thread_local! {
    static IN_FILE_LOCK: Cell<bool> = const { Cell::new(false) };
}

impl Mind {
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

        if IN_FILE_LOCK.with(Cell::get) {
            return f(self);
        }

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
            std::fs::set_permissions(&lock_path, perms).map_err(|e| {
                RustyBrainError::FileSystem {
                    code: error_codes::E_FS_IO_ERROR,
                    message: format!(
                        "failed to set lock file permissions: {}",
                        lock_path.display()
                    ),
                    source: Some(e),
                }
            })?;
        }

        // Exponential backoff: 100ms base, 5 retries, 2x multiplier.
        let max_retries: u32 = 5;
        for attempt in 0..=max_retries {
            match lock_file.try_lock_exclusive() {
                Ok(()) => {
                    let _reentrant_guard = ReentrantLockGuard::enter();
                    let result = f(self);
                    // Dropping the file closes the fd, releasing the flock.
                    drop(lock_file);
                    return result;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && attempt < max_retries => {
                    let delay = std::time::Duration::from_millis(100) * 2u32.pow(attempt);
                    std::thread::sleep(delay);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(RustyBrainError::LockTimeout {
                        code: error_codes::E_LOCK_TIMEOUT,
                        message: format!(
                            "failed to acquire lock after {max_retries} retries: {}",
                            lock_path.display()
                        ),
                    });
                }
                Err(e) => {
                    return Err(RustyBrainError::FileSystem {
                        code: error_codes::E_FS_IO_ERROR,
                        message: format!("failed to acquire lock: {}", lock_path.display()),
                        source: Some(e),
                    });
                }
            }
        }

        unreachable!()
    }
}

struct ReentrantLockGuard {
    previous: bool,
}

impl ReentrantLockGuard {
    fn enter() -> Self {
        let previous = IN_FILE_LOCK.with(|locked| {
            let prev = locked.get();
            locked.set(true);
            prev
        });
        Self { previous }
    }
}

impl Drop for ReentrantLockGuard {
    fn drop(&mut self) {
        IN_FILE_LOCK.with(|locked| locked.set(self.previous));
    }
}
```

---

### Task 5: RB-ARCH-007 — Create mind/read.rs

**Files:**
- Create: `crates/core/src/mind/read.rs`

**Step 1: Create read.rs with search, ask, timeline, get_context, and parse_search_hit**

Create `crates/core/src/mind/read.rs`:

```rust
//! Read-only Mind operations: search, ask, timeline, get_context.

use chrono::{DateTime, Utc};

use crate::backend::SearchHit;
use types::{InjectedContext, ObservationType, RustyBrainError};

use super::{MemorySearchResult, Mind, TimelineEntry};

impl Mind {
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
        let requested = limit.unwrap_or(10);
        let fetch_limit = if self.config.min_confidence > 0.0 {
            requested.saturating_mul(3)
        } else {
            requested
        };
        let hits = self.backend.find(query, fetch_limit)?;
        let results: Vec<_> = hits
            .iter()
            .map(parse_search_hit)
            .filter(|r| r.score >= self.config.min_confidence)
            .take(requested)
            .collect();
        tracing::debug!(result_count = results.len(), "search complete");
        Ok(results)
    }

    /// Ask a question against stored observations.
    ///
    /// Returns `Some(answer)` when relevant memories are found, or `None`
    /// when the backend returns no results. Callers decide how to present
    /// the "no results" case instead of comparing against a sentinel string.
    ///
    /// # Errors
    ///
    /// Returns `RustyBrainError::Storage` if the backend ask fails.
    #[tracing::instrument(skip(self), fields(question_len = question.len()))]
    pub fn ask(&self, question: &str) -> Result<Option<String>, RustyBrainError> {
        let answer = self.backend.ask(question, 10)?;
        if answer.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(answer))
        }
    }

    /// Query timeline entries.
    ///
    /// Returns observations parsed from backend frames. When `reverse` is
    /// `true`, entries are ordered most-recent-first (default CLI behavior).
    /// When `false`, entries are ordered oldest-first.
    ///
    /// The `limit` parameter controls the maximum number of entries returned.
    ///
    /// # Errors
    ///
    /// Returns [`RustyBrainError::Storage`] if the backend timeline or
    /// frame lookup fails.
    #[tracing::instrument(skip(self))]
    pub fn timeline(
        &self,
        limit: usize,
        reverse: bool,
    ) -> Result<Vec<TimelineEntry>, RustyBrainError> {
        let entries = self.backend.timeline(limit, reverse)?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in &entries {
            let frame = self.backend.frame_by_id(entry.frame_id)?;
            let obs_type = frame
                .metadata
                .get("obs_type")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<ObservationType>().ok())
                .unwrap_or(ObservationType::Discovery);
            let summary = frame
                .metadata
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or(&entry.preview)
                .to_string();
            let timestamp = frame
                .metadata
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map_or_else(Utc::now, |dt| dt.with_timezone(&Utc));
            let tool_name = frame
                .metadata
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            result.push(TimelineEntry {
                obs_type,
                summary,
                timestamp,
                tool_name,
            });
        }
        tracing::debug!(entry_count = result.len(), "timeline query complete");
        Ok(result)
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
}

/// Parse a backend `SearchHit` into a `MemorySearchResult`.
fn parse_search_hit(hit: &SearchHit) -> MemorySearchResult {
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
```

---

### Task 6: RB-ARCH-007 — Create mind/write.rs

**Files:**
- Create: `crates/core/src/mind/write.rs`

**Step 1: Create write.rs with remember, remember_unlocked, and save_session_summary**

Create `crates/core/src/mind/write.rs`:

```rust
//! Write operations: remember, save_session_summary.

use chrono::Utc;

use types::{
    ObservationMetadata, ObservationType, RustyBrainError, SessionSummary, StorageSource,
    error_codes,
};

use super::Mind;

impl Mind {
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
    ///
    /// This method acquires the cross-process file lock internally before
    /// mutating storage.
    #[tracing::instrument(skip(self, content, metadata), fields(obs_type = %obs_type, tool = tool_name))]
    pub fn remember(
        &self,
        obs_type: ObservationType,
        tool_name: &str,
        summary: &str,
        content: Option<&str>,
        metadata: Option<&ObservationMetadata>,
    ) -> Result<String, RustyBrainError> {
        self.with_lock(|mind| {
            mind.remember_unlocked(obs_type, tool_name, summary, content, metadata)
        })
    }

    fn remember_unlocked(
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
    #[tracing::instrument(skip(self, decisions, modified_files))]
    pub fn save_session_summary(
        &self,
        decisions: Vec<String>,
        modified_files: Vec<String>,
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
            modified_files,
            summary.to_string(),
        )?;

        let content_json =
            serde_json::to_string(&session_summary).map_err(|e| RustyBrainError::Storage {
                code: error_codes::E_STORAGE_BACKEND,
                message: "failed to serialize session summary".to_string(),
                source: Some(StorageSource(e.to_string())),
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
}
```

---

### Task 7: RB-ARCH-007 — Create mind/stats.rs

**Files:**
- Create: `crates/core/src/mind/stats.rs`

**Step 1: Create stats.rs with stats computation and caching**

Create `crates/core/src/mind/stats.rs`:

```rust
//! Mind statistics computation and caching.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use types::{MindStats, ObservationType, RustyBrainError};

use super::Mind;

impl Mind {
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
        let mut type_counts: HashMap<ObservationType, u64> = HashMap::new();

        for entry in &timeline {
            let frame = self.backend.frame_by_id(entry.frame_id)?;

            // Parse obs_type from metadata.
            if let Some(obs_type_str) = frame.metadata.get("obs_type").and_then(|v| v.as_str()) {
                if let Ok(obs_type) = obs_type_str.parse::<ObservationType>() {
                    *type_counts.entry(obs_type).or_insert(0) += 1;
                }
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
}
```

---

### Task 8: RB-ARCH-007 — Delete old mind.rs and verify

**Files:**
- Delete: `crates/core/src/mind.rs`

**Step 1: Delete the old mind.rs file**

Run: `rm crates/core/src/mind.rs`

Note: Rust resolves `pub mod mind;` to either `mind.rs` or `mind/mod.rs`. Since we created `mind/mod.rs` in Task 2, removing `mind.rs` makes the compiler use the directory-based module.

**Step 2: Verify compilation**

Run: `cargo build --package rusty-brain-core`
Expected: BUILD SUCCESS with no errors

**Step 3: Run full core test suite**

Run: `cargo test --package rusty-brain-core`
Expected: All tests PASS (identical results to Task 2 Step 1 baseline)

**Step 4: Run clippy**

Run: `cargo clippy --package rusty-brain-core -- -D warnings`
Expected: No warnings

**Step 5: Run workspace tests to verify no downstream breakage**

Run: `cargo test --workspace`
Expected: All tests PASS

**Step 6: Commit**

```bash
git add crates/core/src/mind/ crates/core/src/mind.rs
git commit -m "refactor: decompose Mind into focused submodules (RB-ARCH-007)

Split crates/core/src/mind.rs (747 lines) into:
- mind/mod.rs: struct, types, accessors
- mind/lifecycle.rs: open, recovery, permissions
- mind/locking.rs: cross-process file locking
- mind/read.rs: search, ask, timeline, get_context
- mind/write.rs: remember, save_session_summary
- mind/stats.rs: statistics computation and caching

Public API surface unchanged. All tests pass."
```

---

### Task 9: RB-ARCH-008 — Remove get_mind/reset_mind Singleton

**Files:**
- Modify: `crates/core/src/lib.rs`

**Step 1: Run baseline tests**

Run: `cargo test --package rusty-brain-core`
Expected: All tests PASS

**Step 2: Replace lib.rs contents**

Replace the entire contents of `crates/core/src/lib.rs` with:

```rust
//! Core memory engine (Mind) for rusty-brain.
//!
//! This crate provides the [`Mind`] struct — the central API for storing,
//! searching, and retrieving observations from a memvid-backed `.mv2` memory
//! file. It also exposes [`estimate_tokens`] for token budget estimation.

mod backend;
mod context_builder;
mod file_guard;
mod memvid_store;
pub mod mind;
pub mod token;
```

This removes:
- `MIND_INSTANCE` static
- `get_mind()` function
- `reset_mind()` function
- `singleton_get_and_reset` test
- `use std::sync::{Arc, Mutex}` import
- `use mind::Mind` import (no longer needed at this level)
- `use types::{MindConfig, RustyBrainError, error_codes}` import (no longer needed at this level)

**Step 3: Verify compilation**

Run: `cargo build --workspace`
Expected: BUILD SUCCESS

**Step 4: Run full workspace tests**

Run: `cargo test --workspace`
Expected: All tests PASS (the removed test was the only consumer)

**Step 5: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

**Step 6: Check formatting**

Run: `cargo fmt --check`
Expected: No formatting issues

**Step 7: Commit**

```bash
git add crates/core/src/lib.rs
git commit -m "refactor: remove unused get_mind/reset_mind singleton (RB-ARCH-008)

Zero runtime callers exist — both hooks and opencode use Mind::open()
per invocation. Removes global Mutex state and simplifies lib.rs."
```

---

### Task 10: Final Quality Gates

**Step 1: Run all quality gates**

Run these in sequence:
```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```
Expected: All three pass clean.

**Step 2: Verify module sizes**

Run: `wc -l crates/core/src/mind/*.rs`
Expected: Each file < 200 lines, total < 750 lines (roughly same as before, just split).

**Step 3: Update remediation plan checkboxes**

In `arch_review_remediation_plan.md`, mark RB-ARCH-006, RB-ARCH-007, and RB-ARCH-008 as done:
- `[x] **RB-ARCH-006 (P1, M)**`
- `[x] **RB-ARCH-007 (P2, L)**`
- `[x] **RB-ARCH-008 (P2, M)**`
