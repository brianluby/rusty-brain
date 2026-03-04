use crate::error::HookError;
use types::hooks::{HookInput, HookOutput};

/// Read a single `HookInput` JSON object from stdin.
///
/// # Errors
///
/// Returns `HookError` on empty stdin, invalid JSON, or I/O failure.
pub fn read_input() -> Result<HookInput, HookError> {
    let stdin = std::io::stdin();
    let input: HookInput = serde_json::from_reader(stdin.lock())?;
    Ok(input)
}

/// Write a `HookOutput` as JSON to stdout, followed by a newline.
///
/// # Errors
///
/// Returns `HookError` on I/O or serialization failure.
pub fn write_output(output: &HookOutput) -> Result<(), HookError> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, output)?;
    std::io::Write::write_all(&mut handle, b"\n")?;
    Ok(())
}

/// Convert a handler result into a guaranteed-valid `HookOutput`.
///
/// - `Ok(output)` -> output as-is
/// - `Err(error)` -> `HookOutput { continue: true, ..default }` (fail-open)
#[must_use]
pub fn fail_open(result: Result<HookOutput, HookError>) -> HookOutput {
    match result {
        Ok(output) => output,
        Err(e) => {
            tracing::warn!("Hook error (fail-open): {e}");
            HookOutput {
                continue_execution: Some(true),
                ..Default::default()
            }
        }
    }
}
