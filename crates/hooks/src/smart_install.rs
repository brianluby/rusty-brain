use std::path::Path;

use crate::error::HookError;
use types::hooks::{HookInput, HookOutput};

const VERSION_FILENAME: &str = ".install-version";

/// Handle the smart-install hook event.
///
/// Reads `.install-version`, compares with the current binary version,
/// and writes the current version if missing or mismatched.
///
/// # Errors
///
/// Returns `HookError::Io` on file I/O failure.
pub fn handle_smart_install(input: &HookInput) -> Result<HookOutput, HookError> {
    let cwd = Path::new(&input.cwd);
    let version_path = cwd.join(VERSION_FILENAME);
    let current_version = env!("CARGO_PKG_VERSION");

    let needs_write = match std::fs::read_to_string(&version_path) {
        Ok(stored) => stored.trim() != current_version,
        Err(_) => true, // Missing file → fresh install
    };

    if needs_write {
        // Atomic write: temp file + rename (unique name avoids collisions)
        let tmp_path = version_path.with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp_path, current_version)?;
        if let Err(e) = std::fs::rename(&tmp_path, &version_path) {
            let _ = std::fs::remove_file(&tmp_path); // Best-effort cleanup
            return Err(e.into());
        }
    }

    Ok(HookOutput {
        continue_execution: Some(true),
        ..Default::default()
    })
}
