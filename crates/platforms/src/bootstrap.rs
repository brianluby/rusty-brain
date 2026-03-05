use std::path::Path;

use crate::{AdapterRegistry, EventPipeline, detect_platform, resolve_memory_path};
use types::{HookInput, MindConfig, RustyBrainError};

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

/// Detect legacy `.claude/mind.mv2` path.
#[must_use]
pub fn detect_legacy_path(project_root: &Path) -> Option<Diagnostic> {
    let legacy = project_root.join(crate::LEGACY_CLAUDE_MEMORY_PATH);
    let canonical = project_root.join(".agent-brain/mind.mv2");

    let legacy_exists = legacy.exists();
    let canonical_exists = canonical.exists();

    match (legacy_exists, canonical_exists) {
        (true, false) => Some(Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "Legacy memory file found at `{}`. Migrate to `{}` for the current Rust engine.",
                crate::LEGACY_CLAUDE_MEMORY_PATH,
                ".agent-brain/mind.mv2"
            ),
        }),
        (true, true) => Some(Diagnostic {
            level: DiagnosticLevel::Warning,
            message: format!(
                "Duplicate memory files detected: both `{}` and `{}` exist. Using `{}`. Consider removing the legacy file.",
                crate::LEGACY_CLAUDE_MEMORY_PATH,
                ".agent-brain/mind.mv2",
                ".agent-brain/mind.mv2"
            ),
        }),
        _ => None,
    }
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
        let resolved =
            resolve_memory_path(project_dir, platform_name, platform_opt_in()).map_err(|e| {
                RustyBrainError::Configuration {
                    code: types::error_codes::E_CONFIG_INVALID_VALUE,
                    message: e.to_string(),
                }
            })?;
        config.memory_path = resolved.path;
    }

    Ok(config)
}
