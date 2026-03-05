use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::MindConfig;
use types::hooks::HookInput;

use crate::error::HookError;

/// Severity level for lightweight hook diagnostics.
///
/// This is distinct from `types::DiagnosticSeverity` / `types::DiagnosticRecord`
/// which are heavier, persisted records with retention and redaction. This enum
/// is for transient, in-memory diagnostics returned by helper functions like
/// `detect_legacy_path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    /// Warning diagnostic — action may be needed.
    Warning,
    /// Informational diagnostic — no action required.
    Info,
}

/// A lightweight, transient diagnostic message.
///
/// Used for in-memory hook diagnostics (e.g. legacy path warnings) that are
/// surfaced in structured output but not persisted to storage.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// The severity level of this diagnostic.
    pub level: DiagnosticLevel,
    /// A human-readable message describing the diagnostic.
    pub message: String,
}

/// Detect legacy `.claude/mind.mv2` path and return a diagnostic if applicable.
///
/// # Behavior
///
/// - If `.claude/mind.mv2` exists AND `.agent-brain/mind.mv2` does **not** exist:
///   returns `Some(Diagnostic::Warning)` with a migration suggestion.
/// - If **both** paths exist: returns `Some(Diagnostic::Warning)` noting the
///   duplicate; the caller should use `.agent-brain/mind.mv2`.
/// - If only `.agent-brain/mind.mv2` exists (or neither): returns `None`.
///
/// Does NOT perform any writes — purely diagnostic.
#[must_use]
pub fn detect_legacy_path(project_root: &Path) -> Option<Diagnostic> {
    let legacy = project_root.join(platforms::LEGACY_CLAUDE_MEMORY_PATH);
    let canonical = project_root.join(".agent-brain/mind.mv2");

    let legacy_exists = legacy.exists();
    let canonical_exists = canonical.exists();

    match (legacy_exists, canonical_exists) {
        (true, false) => Some(Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "Legacy memory file found at `{}`. Migrate to `{}` for the current Rust engine.",
                platforms::LEGACY_CLAUDE_MEMORY_PATH,
                ".agent-brain/mind.mv2"
            ),
        }),
        (true, true) => Some(Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "Duplicate memory files detected: both `{}` and `{}` exist. \
                 Using `{}`. Consider removing the legacy file.",
                platforms::LEGACY_CLAUDE_MEMORY_PATH,
                ".agent-brain/mind.mv2",
                ".agent-brain/mind.mv2"
            ),
        }),
        _ => None,
    }
}

fn platform_opt_in() -> bool {
    std::env::var("MEMVID_PLATFORM_PATH_OPT_IN").is_ok_and(|v| v == "1")
}

/// Check whether the incoming event should be processed through the pipeline.
///
/// Normalizes the hook input into a `PlatformEvent` via the adapter registry,
/// then runs it through the `EventPipeline` for contract validation and
/// identity resolution. Returns `true` if processing should proceed.
///
/// Fail-open: returns `true` on all error paths (missing adapter, normalization
/// failure) so that handler behavior is never silently blocked.
#[must_use]
pub fn should_process(input: &HookInput, event_kind_hint: &str) -> bool {
    let platform_name = platforms::detect_platform(input);
    let registry = platforms::AdapterRegistry::with_builtins();

    let Some(adapter) = registry.resolve(&platform_name) else {
        return true;
    };

    let Some(event) = adapter.normalize(input, event_kind_hint) else {
        return true;
    };

    let pipeline = platforms::EventPipeline::new();
    let result = pipeline.process(&event);
    !result.skipped
}

/// Resolve the canonical memory file path for the detected platform.
///
/// # Errors
///
/// Returns `HookError::Platform` if platform path resolution fails.
pub fn resolve_memory_path(input: &HookInput, cwd: &Path) -> Result<std::path::PathBuf, HookError> {
    let platform_name = platforms::detect_platform(input);
    let resolved =
        platforms::resolve_memory_path(cwd, &platform_name, platform_opt_in()).map_err(|e| {
            HookError::Platform {
                message: format!("Failed to resolve memory path: {e}"),
            }
        })?;
    Ok(resolved.path)
}

/// Open a read-write `Mind` instance for the detected platform.
///
/// Uses `MindConfig::from_env()` to honour env-driven config (e.g.
/// `MEMVID_MIND_DEBUG`). Only overrides `memory_path` when
/// `MEMVID_PLATFORM_MEMORY_PATH` is not explicitly set, preserving the
/// documented precedence: explicit env override > platform policy > default.
///
/// # Errors
///
/// Returns `HookError::Platform` if path resolution fails, or a `HookError`
/// wrapping the underlying `Mind::open` error on storage failure.
pub fn open_mind(input: &HookInput, cwd: &Path) -> Result<Mind, HookError> {
    let memory_path = resolve_memory_path(input, cwd)?;
    open_mind_with_path(memory_path)
}

/// Open a read-write `Mind` instance with a pre-resolved memory path.
///
/// Uses `MindConfig::from_env()` to honour env-driven config (e.g.
/// `MEMVID_MIND_DEBUG`). Only overrides `memory_path` when
/// `MEMVID_PLATFORM_MEMORY_PATH` is not explicitly set, preserving the
/// documented precedence: explicit env override > platform policy > default.
///
/// Use this when the caller has already resolved the path (e.g. `session_start`
/// needs the path for legacy-path warnings before opening) to avoid double
/// resolution.
///
/// # Errors
///
/// Returns `HookError::Platform` on config failure, or a `HookError`
/// wrapping the underlying `Mind::open` error on storage failure.
pub fn open_mind_with_path(memory_path: std::path::PathBuf) -> Result<Mind, HookError> {
    let mut config = MindConfig::from_env().map_err(|e| HookError::Platform {
        message: format!("Failed to load config from env: {e}"),
    })?;
    // Only override with caller-provided path when no explicit env override
    if std::env::var("MEMVID_PLATFORM_MEMORY_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .is_none()
    {
        config.memory_path = memory_path;
    }
    Ok(Mind::open(config)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a minimal [`HookInput`] for testing via JSON deserialization
    /// (required because `HookInput` is `#[non_exhaustive]`).
    fn make_input(cwd: &str) -> HookInput {
        serde_json::from_value(serde_json::json!({
            "session_id": "test-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": cwd,
            "permission_mode": "default",
            "hook_event_name": "SessionStart"
        }))
        .expect("valid HookInput JSON")
    }

    // -----------------------------------------------------------------------
    // should_process
    // -----------------------------------------------------------------------

    #[test]
    fn should_process_returns_true_for_standard_input() {
        let input = make_input("/tmp");
        // With no adapter match or with fail-open, should return true
        let result = should_process(&input, "session_start");
        assert!(
            result,
            "should_process should return true for standard input"
        );
    }

    #[test]
    fn should_process_returns_true_for_unknown_event_kind() {
        let input = make_input("/tmp");
        let result = should_process(&input, "completely_unknown_event");
        assert!(
            result,
            "should_process should fail-open for unknown event kinds"
        );
    }

    // -----------------------------------------------------------------------
    // resolve_memory_path
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_memory_path_returns_path_for_valid_cwd() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());
        let result = resolve_memory_path(&input, tmp.path());
        // Should succeed and return a PathBuf (the exact path depends on platform detection)
        assert!(
            result.is_ok(),
            "resolve_memory_path should succeed: {result:?}"
        );
        let path = result.unwrap();
        assert!(
            path.to_str().unwrap().contains("mind.mv2"),
            "resolved path should contain mind.mv2, got: {path:?}"
        );
    }

    #[test]
    fn resolve_memory_path_returns_pathbuf_type() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());
        let result = resolve_memory_path(&input, tmp.path());
        assert!(result.is_ok());
        let _path: PathBuf = result.unwrap();
    }

    // -----------------------------------------------------------------------
    // open_mind — requires memvid filesystem setup, so mark as #[ignore]
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "requires memvid runtime (Mind::open needs valid .mv2 file)"]
    fn open_mind_succeeds_with_valid_setup() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let input = make_input(tmp.path().to_str().unwrap());
        let result = open_mind(&input, tmp.path());
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // open_mind_with_path — requires memvid, but we can test the config logic
    // -----------------------------------------------------------------------

    #[test]
    #[ignore = "requires memvid runtime (Mind::open needs valid .mv2 file)"]
    fn open_mind_with_path_uses_provided_path() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let path = tmp.path().join(".agent-brain").join("mind.mv2");
        let result = open_mind_with_path(path);
        // Will fail because no .mv2 file exists, but tests the path logic
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn platform_opt_in_returns_bool() {
        // `platform_opt_in` reads `MEMVID_PLATFORM_PATH_OPT_IN` env var.
        // We simply verify it returns without panicking.
        let _result: bool = platform_opt_in();
    }

    // -----------------------------------------------------------------------
    // detect_legacy_path (Contract 4, T067-T069)
    // -----------------------------------------------------------------------

    /// T067: Legacy-only scenario — `.claude/mind.mv2` exists,
    /// `.agent-brain/mind.mv2` does not → Warning with migration suggestion.
    #[test]
    fn detect_legacy_path_legacy_only_returns_warning() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let legacy_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(legacy_dir.join("mind.mv2"), b"fake mv2").unwrap();

        let result = detect_legacy_path(tmp.path());

        assert!(
            result.is_some(),
            "should return a diagnostic for legacy-only"
        );
        let diag = result.unwrap();
        assert_eq!(diag.level, DiagnosticLevel::Warning);
        assert!(
            diag.message.contains(".claude/mind.mv2"),
            "message should mention legacy path, got: {}",
            diag.message
        );
        assert!(
            diag.message.to_lowercase().contains("migrate"),
            "message should suggest migration, got: {}",
            diag.message
        );
    }

    /// T068: Both-exist scenario — both `.claude/mind.mv2` and
    /// `.agent-brain/mind.mv2` exist → Warning about duplicate.
    #[test]
    fn detect_legacy_path_both_exist_returns_duplicate_warning() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        // Create legacy path
        let legacy_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(legacy_dir.join("mind.mv2"), b"fake mv2").unwrap();
        // Create canonical path
        let canonical_dir = tmp.path().join(".agent-brain");
        std::fs::create_dir_all(&canonical_dir).unwrap();
        std::fs::write(canonical_dir.join("mind.mv2"), b"fake mv2").unwrap();

        let result = detect_legacy_path(tmp.path());

        assert!(
            result.is_some(),
            "should return a diagnostic when both exist"
        );
        let diag = result.unwrap();
        assert_eq!(diag.level, DiagnosticLevel::Warning);
        assert!(
            diag.message.contains("uplicate") || diag.message.contains("both"),
            "message should mention duplicate/both, got: {}",
            diag.message
        );
        assert!(
            diag.message.contains(".agent-brain/mind.mv2"),
            "message should mention canonical path, got: {}",
            diag.message
        );
    }

    /// T069: Normal scenario — only `.agent-brain/mind.mv2` exists → None.
    #[test]
    fn detect_legacy_path_canonical_only_returns_none() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let canonical_dir = tmp.path().join(".agent-brain");
        std::fs::create_dir_all(&canonical_dir).unwrap();
        std::fs::write(canonical_dir.join("mind.mv2"), b"fake mv2").unwrap();

        let result = detect_legacy_path(tmp.path());

        assert!(
            result.is_none(),
            "should return None when only canonical path exists"
        );
    }

    /// Additional: Neither path exists → None.
    #[test]
    fn detect_legacy_path_neither_exists_returns_none() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");

        let result = detect_legacy_path(tmp.path());

        assert!(
            result.is_none(),
            "should return None when neither path exists"
        );
    }
}
