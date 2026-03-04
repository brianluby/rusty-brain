use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::HookInput;
use types::{MindConfig, RustyBrainError};

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

/// Resolve the canonical memory file path for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if platform path resolution fails.
pub fn resolve_memory_path(cwd: &Path) -> Result<std::path::PathBuf, RustyBrainError> {
    let resolved = platforms::resolve_memory_path(cwd, "opencode", platform_opt_in())?;
    Ok(resolved.path)
}

/// Build a `MindConfig` with the resolved memory path for the opencode platform.
///
/// Uses `MindConfig::from_env()` to honour env-driven config. Only overrides
/// `memory_path` when `MEMVID_PLATFORM_MEMORY_PATH` is not explicitly set,
/// preserving the documented precedence: explicit env override > platform
/// policy > default.
///
/// # Errors
///
/// Returns `RustyBrainError` if path resolution or env-based config loading fails.
pub fn mind_config(cwd: &Path) -> Result<MindConfig, RustyBrainError> {
    let mut config = MindConfig::from_env()?;
    // Only override with platform-resolved path when no explicit env override
    if std::env::var("MEMVID_PLATFORM_MEMORY_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .is_none()
    {
        config.memory_path = resolve_memory_path(cwd)?;
    }
    Ok(config)
}

/// Open a read-write `Mind` instance for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if config resolution or `Mind::open` fails.
pub fn open_mind_read_write(cwd: &Path) -> Result<Mind, RustyBrainError> {
    Mind::open(mind_config(cwd)?)
}

/// Open a read-only `Mind` instance for the opencode platform.
///
/// # Errors
///
/// Returns `RustyBrainError` if config resolution or `Mind::open_read_only` fails.
pub fn open_mind_read_only(cwd: &Path) -> Result<Mind, RustyBrainError> {
    Mind::open_read_only(mind_config(cwd)?)
}
