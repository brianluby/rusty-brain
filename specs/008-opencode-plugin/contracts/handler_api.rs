// Contract: OpenCode Plugin Handler API
// Feature: 008-opencode-plugin
// Date: 2026-03-03
//
// These are the public function signatures that crates/opencode exposes.
// The CLI (crates/cli) calls these functions; they accept parsed input and
// return output structs — no stdin/stdout I/O in the library.
//
// All handlers fail-open: errors and panics are caught by the fail-open
// wrapper and converted to valid default output.

use std::path::Path;
use std::time::Duration;

use types::{HookInput, HookOutput, RustyBrainError};

use crate::types::{MindToolInput, MindToolOutput};

// ---------------------------------------------------------------------------
// Chat Hook (M-1, S-3)
// ---------------------------------------------------------------------------

/// Process a chat message event from OpenCode.
///
/// Resolves the memory path (LegacyFirst), opens the Mind, retrieves context
/// via `Mind::get_context(query)`, and returns a HookOutput with:
/// - `system_message`: formatted memory context (human-readable)
/// - `hook_specific_output`: structured InjectedContext JSON
///
/// If no memory file exists, `Mind::open()` auto-creates it (AC-2).
/// The handler then returns a welcome message indicating the memory system
/// is active.
///
/// On error: returns `HookOutput::default()` (fail-open).
///
/// Performance target: <200ms p95 (SC-001).
pub fn handle_chat_hook(input: &HookInput, cwd: &Path) -> Result<HookOutput, RustyBrainError>;

// ---------------------------------------------------------------------------
// Tool Hook (M-2, M-4)
// ---------------------------------------------------------------------------

/// Process a tool execution event from OpenCode.
///
/// 1. Loads sidecar state (or creates fresh state on first invocation)
/// 2. Compresses tool output via `compression::compress()`
/// 3. Computes dedup hash from tool_name + compressed summary
/// 4. If duplicate: skips storage, returns success
/// 5. If new: calls `Mind::remember()`, updates sidecar with new hash
///
/// On error: returns `HookOutput { continue_execution: Some(true), .. }` (fail-open).
///
/// Performance target: <100ms p95 including sidecar I/O (SC-002).
pub fn handle_tool_hook(input: &HookInput, cwd: &Path) -> Result<HookOutput, RustyBrainError>;

// ---------------------------------------------------------------------------
// Mind Tool (M-3)
// ---------------------------------------------------------------------------

/// Process a mind tool invocation from OpenCode.
///
/// Dispatches by mode:
/// - `search`: calls `Mind::search(query, limit)` → Vec<SearchResult>
/// - `ask`: calls `Mind::ask(question)` → Option<String>
/// - `recent`: calls `Mind::timeline(limit, true)` → Vec<TimelineEntry>
/// - `stats`: calls `Mind::stats()` → MindStats
/// - `remember`: calls `Mind::remember(Discovery, "user", content, None, None)` → ULID
///
/// Invalid mode: returns MindToolOutput with error listing valid modes (SEC-8).
///
/// On error: returns MindToolOutput { success: false, error: Some(msg) }.
pub fn handle_mind_tool(input: &MindToolInput, cwd: &Path) -> Result<MindToolOutput, RustyBrainError>;

// ---------------------------------------------------------------------------
// Session Cleanup (S-1)
// ---------------------------------------------------------------------------

/// Process a session deletion event from OpenCode.
///
/// 1. Loads sidecar state to get observation count and session metadata
/// 2. Generates session summary text
/// 3. Calls `Mind::save_session_summary(decisions, files, summary)`
/// 4. Deletes the sidecar file
///
/// On error: returns `HookOutput::default()` (fail-open).
pub fn handle_session_cleanup(session_id: &str, cwd: &Path) -> Result<HookOutput, RustyBrainError>;

// ---------------------------------------------------------------------------
// Orphan Cleanup (S-2)
// ---------------------------------------------------------------------------

/// Scan for and delete stale sidecar files older than max_age.
///
/// Scans `sidecar_dir` for files matching pattern `session-*.json`.
/// Deletes files with mtime older than `max_age` (default: 24 hours).
/// Does NOT recurse into subdirectories (SEC-12).
///
/// On any individual file error: logs WARN and continues scanning.
/// Never panics.
pub fn cleanup_stale_sidecars(sidecar_dir: &Path, max_age: Duration);

// ---------------------------------------------------------------------------
// Fail-Open Wrapper (M-5)
// ---------------------------------------------------------------------------

/// Execute a handler function with fail-open error and panic recovery.
///
/// Catches both `Result::Err` and panics, returning a valid default output.
/// Emits `tracing::warn!` for all caught errors and panics (SEC-10).
///
/// Used by the CLI layer to wrap each handler invocation.
pub fn handle_with_failopen<F>(handler: F) -> HookOutput
where
    F: FnOnce() -> Result<HookOutput, RustyBrainError>;

/// Fail-open wrapper for MindToolOutput (mind tool handlers).
pub fn mind_tool_with_failopen<F>(handler: F) -> MindToolOutput
where
    F: FnOnce() -> Result<MindToolOutput, RustyBrainError>;
