// Contract: crates/hooks API — Claude Code Hook Handlers
// Feature: 006-claude-code-hooks
// Date: 2026-03-03
//
// This file defines the public interface contracts for the hooks crate.
// Implementation MUST match these signatures exactly.
// Types referenced here (HookInput, HookOutput, etc.) are defined in crates/types.

// ---------------------------------------------------------------------------
// Module: error
// ---------------------------------------------------------------------------

/// Hook-specific error type. All variants include a stable error code prefix.
/// Converted to fail-open HookOutput at the I/O boundary — never exposed to
/// the caller (Claude Code).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HookError {
    #[error("[E_HOOK_IO] {message}")]
    Io {
        message: String,
        #[source]
        source: Option<std::io::Error>,
    },

    #[error("[E_HOOK_PARSE] {message}")]
    Parse { message: String },

    #[error("[E_HOOK_MIND] {message}")]
    Mind {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("[E_HOOK_PLATFORM] {message}")]
    Platform { message: String },

    #[error("[E_HOOK_GIT] {message}")]
    Git { message: String },

    #[error("[E_HOOK_DEDUP] {message}")]
    Dedup { message: String },
}

// ---------------------------------------------------------------------------
// Module: io
// ---------------------------------------------------------------------------

/// Read a single HookInput JSON object from stdin.
/// Returns Err on empty stdin, invalid JSON, or I/O failure.
pub fn read_input() -> Result<HookInput, HookError> { todo!() }

/// Write a HookOutput as JSON to stdout, followed by a newline.
/// Returns Err on I/O failure.
pub fn write_output(output: &HookOutput) -> Result<(), HookError> { todo!() }

/// Convert a handler result into a guaranteed-valid HookOutput.
/// - Ok(output) → output as-is
/// - Err(error) → HookOutput { continue_execution: Some(true), ..default }
/// Logs the error via tracing::warn if a subscriber is active.
pub fn fail_open(result: Result<HookOutput, HookError>) -> HookOutput {
    match result {
        Ok(output) => output,
        Err(_e) => HookOutput {
            continue_execution: Some(true),
            ..Default::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Module: dispatch (clap)
// ---------------------------------------------------------------------------

#[derive(clap::Parser)]
#[command(name = "rusty-brain", about = "Memory hooks for Claude Code")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Subcommand,
}

#[derive(clap::Subcommand)]
pub enum Subcommand {
    /// Initialize memory and inject context at session start
    SessionStart,
    /// Capture tool observations after tool execution
    PostToolUse,
    /// Generate session summary and gracefully shut down
    Stop,
    /// Track installation version state
    SmartInstall,
}

// ---------------------------------------------------------------------------
// Module: session_start
// ---------------------------------------------------------------------------

/// Handle the session-start hook event.
///
/// 1. Detect platform from HookInput
/// 2. Resolve project identity from cwd
/// 3. Resolve memory path
/// 4. Open Mind (or create new memory file)
/// 5. Get context (recent observations + session summaries)
/// 6. Get stats
/// 7. Format systemMessage
/// 8. Check for legacy memory path and add migration suggestion
///
/// Returns HookOutput with systemMessage containing injected context.
/// Fails: HookError::Mind, HookError::Platform
pub fn handle_session_start(input: &HookInput) -> Result<HookOutput, HookError> { todo!() }

// ---------------------------------------------------------------------------
// Module: post_tool_use
// ---------------------------------------------------------------------------

/// Handle the post-tool-use hook event.
///
/// 1. Extract tool_name and tool_response from HookInput
/// 2. Classify tool_name → ObservationType
/// 3. Generate summary from tool_input
/// 4. Check dedup cache (skip if duplicate within 60s)
/// 5. Truncate tool_response content to ~500 tokens
/// 6. Store observation via Mind::remember
/// 7. Record in dedup cache
///
/// Returns HookOutput with continue_execution: true.
/// Fails: HookError::Mind, HookError::Dedup
pub fn handle_post_tool_use(input: &HookInput) -> Result<HookOutput, HookError> { todo!() }

// ---------------------------------------------------------------------------
// Module: stop
// ---------------------------------------------------------------------------

/// Handle the stop hook event.
///
/// 1. Detect modified files via git diff (empty Vec on error/no git)
/// 2. Store each modified file as a separate observation (ObservationType::Feature)
/// 3. Collect decisions from session context
/// 4. Generate and store session summary via Mind::save_session_summary
/// 5. Format summary message for HookOutput
///
/// Returns HookOutput with systemMessage containing session summary.
/// Fails: HookError::Mind, HookError::Git
pub fn handle_stop(input: &HookInput) -> Result<HookOutput, HookError> { todo!() }

// ---------------------------------------------------------------------------
// Module: smart_install
// ---------------------------------------------------------------------------

/// Handle the smart-install hook event.
///
/// 1. Read .install-version file (or treat as fresh install if missing)
/// 2. Compare with current binary version
/// 3. If version matches → no-op
/// 4. If version differs or file missing → write current version
///
/// Returns HookOutput with continue_execution: true.
/// Fails: HookError::Io
pub fn handle_smart_install(input: &HookInput) -> Result<HookOutput, HookError> { todo!() }

// ---------------------------------------------------------------------------
// Module: dedup
// ---------------------------------------------------------------------------

/// File-based deduplication cache for post-tool-use observations.
/// Stored at `.agent-brain/.dedup-cache.json` adjacent to the .mv2 file.
/// Entries expire after 60 seconds and are pruned on every read.
pub struct DedupCache {
    cache_path: std::path::PathBuf,
}

impl DedupCache {
    /// Create a new DedupCache for the given project directory.
    /// The cache file is stored at `{project_dir}/.agent-brain/.dedup-cache.json`.
    pub fn new(project_dir: &std::path::Path) -> Self { todo!() }

    /// Check if the given tool+summary combination was recorded within the last 60 seconds.
    /// Returns true if duplicate (should skip storage).
    /// On any error (corrupt file, I/O failure): returns false (fail-open — not a duplicate).
    pub fn is_duplicate(&self, tool_name: &str, summary: &str) -> bool { todo!() }

    /// Record a new tool+summary entry with the current timestamp.
    /// Prunes expired entries before writing.
    /// Uses atomic write (temp file + rename) for concurrency safety.
    pub fn record(&self, tool_name: &str, summary: &str) -> Result<(), HookError> { todo!() }
}

// ---------------------------------------------------------------------------
// Module: truncate
// ---------------------------------------------------------------------------

/// Truncate content to approximately `max_tokens` using head/tail strategy.
///
/// - If content is under the token limit: returns as-is
/// - Otherwise: keeps first ~60% and last ~40%, inserting "[...truncated...]"
/// - Token estimation: chars / 4 (rough approximation)
///
/// # Arguments
/// - `content`: The text to truncate
/// - `max_tokens`: Target token count (default: 500)
///
/// # Returns
/// Truncated string, or original if under limit.
pub fn head_tail_truncate(content: &str, max_tokens: usize) -> String { todo!() }

// ---------------------------------------------------------------------------
// Module: git
// ---------------------------------------------------------------------------

/// Detect files modified in the current working directory using `git diff --name-only HEAD`.
///
/// - Spawns a git subprocess with a 5-second timeout
/// - On any error (git not found, timeout, non-zero exit): returns empty Vec
/// - Arguments are hardcoded string literals (SEC-9)
/// - cwd is used as working directory, not as command argument
///
/// # Arguments
/// - `cwd`: Working directory for the git command (from HookInput.cwd)
///
/// # Returns
/// Vec of modified file paths (relative to cwd), or empty Vec on error.
pub fn detect_modified_files(cwd: &std::path::Path) -> Vec<String> { todo!() }

// ---------------------------------------------------------------------------
// Module: manifest
// ---------------------------------------------------------------------------

/// Generate the hooks.json manifest content for Claude Code hook registration.
///
/// # Arguments
/// - `binary_name`: Name of the rusty-brain binary (default: "rusty-brain")
///
/// # Returns
/// JSON string containing the hooks manifest.
pub fn generate_manifest(binary_name: &str) -> String { todo!() }

// ---------------------------------------------------------------------------
// Module: context
// ---------------------------------------------------------------------------

/// Format an InjectedContext and MindStats into a systemMessage string
/// suitable for injection into the agent's prompt.
///
/// # Arguments
/// - `context`: InjectedContext from Mind::get_context
/// - `stats`: MindStats from Mind::stats
/// - `memory_path`: Path to the .mv2 file (for display)
///
/// # Returns
/// Formatted markdown-like string with recent observations, session summaries,
/// stats, and available commands.
pub fn format_system_message(
    context: &InjectedContext,
    stats: &MindStats,
    memory_path: &std::path::Path,
) -> String { todo!() }
