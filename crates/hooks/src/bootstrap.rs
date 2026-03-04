use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::MindConfig;
use types::hooks::HookInput;

use crate::error::HookError;

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
/// # Errors
///
/// Returns `HookError::Platform` if path resolution fails, or a `HookError`
/// wrapping the underlying `Mind::open` error on storage failure.
pub fn open_mind(input: &HookInput, cwd: &Path) -> Result<Mind, HookError> {
    let memory_path = resolve_memory_path(input, cwd)?;
    let config = MindConfig {
        memory_path,
        ..MindConfig::default()
    };
    Ok(Mind::open(config)?)
}
