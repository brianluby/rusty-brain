use std::path::Path;

use crate::{AdapterRegistry, EventPipeline, detect_platform, resolve_memory_path};
use types::{HookInput, MindConfig, RustyBrainError};

/// Memory database filename, shared across path construction.
const MIND_FILENAME: &str = "mind.mv2";

/// Diagnostic severity level for legacy path warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

/// Detect legacy memory paths and produce diagnostics.
///
/// Checks for `.agent-brain/mind.mv2` and `.claude/mind.mv2` relative to
/// `project_root`. Returns diagnostics with migration instructions pointing
/// to `.rusty-brain/mind.mv2`.
#[must_use]
pub fn detect_legacy_paths(project_root: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    let canonical = format!("{}/{MIND_FILENAME}", crate::DEFAULT_MEMORY_DIR);
    let rusty_brain = project_root.join(&canonical);
    let agent_brain = project_root.join(crate::LEGACY_AGENT_BRAIN_PATH);
    let claude_legacy = project_root.join(crate::LEGACY_CLAUDE_MEMORY_PATH);

    let rusty_exists = rusty_brain.exists();
    let agent_exists = agent_brain.exists();
    let claude_exists = claude_legacy.exists();

    // .agent-brain detection
    if agent_exists && !rusty_exists {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            message: format!(
                "Using legacy memory file at `{}`. Migrate to `{canonical}`: \
                 `mkdir -p {} && mv .agent-brain/mind.mv2 {canonical}`",
                crate::LEGACY_AGENT_BRAIN_PATH,
                crate::DEFAULT_MEMORY_DIR
            ),
        });
    } else if agent_exists && rusty_exists {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "Duplicate memory files: using `{canonical}`. Consider removing `.agent-brain/`."
            ),
        });
    }

    // .claude detection
    if claude_exists {
        diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "Legacy memory file at `.claude/mind.mv2`. Migrate to `{canonical}`: \
                 `mkdir -p {} && mv .claude/mind.mv2 {canonical}`",
                crate::DEFAULT_MEMORY_DIR
            ),
        });
    }

    diagnostics
}

/// Resolve the effective memory path, falling back to `.agent-brain/` if
/// `.rusty-brain/mind.mv2` doesn't exist yet.
///
/// Resolution order:
/// 1. `.rusty-brain/mind.mv2` — used if the file exists
/// 2. `.agent-brain/mind.mv2` — used if file exists and `.rusty-brain/mind.mv2` doesn't
/// 3. `.rusty-brain/mind.mv2` — returned as default for new installations
#[must_use]
pub fn resolve_effective_path(project_root: &Path) -> std::path::PathBuf {
    let rusty_brain = project_root.join(crate::DEFAULT_MEMORY_DIR).join(MIND_FILENAME);
    if rusty_brain.exists() {
        return rusty_brain;
    }

    let agent_brain = project_root.join(crate::LEGACY_AGENT_BRAIN_PATH);
    if agent_brain.exists() {
        return agent_brain;
    }

    // New installation default
    rusty_brain
}

#[must_use]
pub fn platform_opt_in() -> bool {
    std::env::var("MEMVID_PLATFORM_PATH_OPT_IN").is_ok_and(|v| v == "1")
}

/// Run an event through the pipeline to check if it should be processed.
/// Returns true if it should be processed (fail-open on error).
#[must_use]
pub fn should_process(input: &HookInput, event_kind_hint: &str) -> bool {
    let platform_name = detect_platform(input);
    let registry = AdapterRegistry::with_builtins();

    let Some(adapter) = registry.resolve(&platform_name) else {
        return true; // fail-open
    };

    let Some(event) = adapter.normalize(input, event_kind_hint) else {
        return true; // fail-open
    };

    let pipeline = EventPipeline::new();
    let result = pipeline.process(&event);
    !result.skipped
}

/// Build a configured [`MindConfig`], applying platform path policies.
///
/// # Errors
///
/// Returns `RustyBrainError::Configuration` if `MindConfig::from_env` fails
/// or if platform path resolution fails.
pub fn build_mind_config(
    project_dir: &Path,
    platform_name: &str,
) -> Result<MindConfig, RustyBrainError> {
    let mut config = MindConfig::from_env().map_err(|e| RustyBrainError::Configuration {
        code: types::error_codes::E_CONFIG_INVALID_VALUE,
        message: e.to_string(),
    })?;

    // If user explicitly provided MEMVID_PLATFORM_MEMORY_PATH, MindConfig::from_env already set it.
    // Otherwise, we use platform path resolution.
    if std::env::var("MEMVID_PLATFORM_MEMORY_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .is_none()
    {
        if platform_opt_in() {
            let resolved = resolve_memory_path(project_dir, platform_name, true).map_err(|e| {
                RustyBrainError::Configuration {
                    code: types::error_codes::E_CONFIG_INVALID_VALUE,
                    message: e.to_string(),
                }
            })?;
            config.memory_path = resolved.path;
        } else {
            config.memory_path = resolve_effective_path(project_dir);
        }
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // detect_legacy_paths
    // -----------------------------------------------------------------------

    #[test]
    fn detect_legacy_paths_agent_brain_only_suggests_migration() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".agent-brain");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("mind.mv2"), b"data").unwrap();

        let result = detect_legacy_paths(dir.path());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].level, DiagnosticLevel::Info);
        assert!(result[0].message.contains("mkdir -p .rusty-brain && mv .agent-brain/mind.mv2 .rusty-brain/mind.mv2"));
    }

    #[test]
    fn detect_legacy_paths_both_dirs_warns_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".agent-brain");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("mind.mv2"), b"data").unwrap();
        let rusty_dir = dir.path().join(".rusty-brain");
        std::fs::create_dir_all(&rusty_dir).unwrap();
        std::fs::write(rusty_dir.join("mind.mv2"), b"data").unwrap();

        let result = detect_legacy_paths(dir.path());
        assert!(
            result
                .iter()
                .any(|d| d.level == DiagnosticLevel::Warning && d.message.contains("Duplicate"))
        );
    }

    #[test]
    fn detect_legacy_paths_rusty_brain_only_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let rusty_dir = dir.path().join(".rusty-brain");
        std::fs::create_dir_all(&rusty_dir).unwrap();
        std::fs::write(rusty_dir.join("mind.mv2"), b"data").unwrap();

        let result = detect_legacy_paths(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn detect_legacy_paths_claude_only_suggests_rusty_brain() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("mind.mv2"), b"data").unwrap();

        let result = detect_legacy_paths(dir.path());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].level, DiagnosticLevel::Warning);
        assert!(result[0].message.contains(".rusty-brain"));
    }

    #[test]
    fn detect_legacy_paths_all_three_dirs() {
        let dir = tempfile::tempdir().unwrap();
        for d in [".claude", ".agent-brain", ".rusty-brain"] {
            let p = dir.path().join(d);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("mind.mv2"), b"data").unwrap();
        }

        let result = detect_legacy_paths(dir.path());
        // Should warn about .agent-brain duplicate + .claude legacy
        assert!(result.len() >= 2);
    }

    #[test]
    fn detect_legacy_paths_claude_and_agent_brain_no_rusty() {
        let dir = tempfile::tempdir().unwrap();
        for d in [".claude", ".agent-brain"] {
            let p = dir.path().join(d);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("mind.mv2"), b"data").unwrap();
        }

        let result = detect_legacy_paths(dir.path());
        assert!(result.len() >= 2);
        // .agent-brain → Info (migration), .claude → Warning
        assert!(result.iter().any(|d| d.level == DiagnosticLevel::Info));
        assert!(result.iter().any(|d| d.level == DiagnosticLevel::Warning));
    }

    // -----------------------------------------------------------------------
    // resolve_effective_path
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_effective_path_falls_back_to_agent_brain() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".agent-brain");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("mind.mv2"), b"data").unwrap();

        let result = resolve_effective_path(dir.path());
        assert_eq!(result, dir.path().join(".agent-brain/mind.mv2"));
    }

    #[test]
    fn resolve_effective_path_ignores_rusty_brain_dir_without_mv2() {
        let dir = tempfile::tempdir().unwrap();
        // .rusty-brain/ exists with only metadata (e.g. from smart_install)
        let rusty_dir = dir.path().join(".rusty-brain");
        std::fs::create_dir_all(&rusty_dir).unwrap();
        std::fs::write(rusty_dir.join(".install-version"), b"0.1.0").unwrap();
        // .agent-brain/mind.mv2 exists with actual memory data
        let agent_dir = dir.path().join(".agent-brain");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("mind.mv2"), b"data").unwrap();

        let result = resolve_effective_path(dir.path());
        assert_eq!(
            result,
            dir.path().join(".agent-brain/mind.mv2"),
            "should fall back to .agent-brain when .rusty-brain has no mind.mv2"
        );
    }

    #[test]
    fn resolve_effective_path_prefers_rusty_brain() {
        let dir = tempfile::tempdir().unwrap();
        for d in [".agent-brain", ".rusty-brain"] {
            let p = dir.path().join(d);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("mind.mv2"), b"data").unwrap();
        }

        let result = resolve_effective_path(dir.path());
        assert_eq!(result, dir.path().join(".rusty-brain/mind.mv2"));
    }

    #[test]
    fn resolve_effective_path_new_install_returns_rusty_brain() {
        let dir = tempfile::tempdir().unwrap();

        let result = resolve_effective_path(dir.path());
        assert_eq!(result, dir.path().join(".rusty-brain/mind.mv2"));
    }
}
