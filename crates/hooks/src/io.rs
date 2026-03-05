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
    std::io::Write::flush(&mut handle)?;
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
            tracing::warn!(
                error = &e as &dyn std::error::Error,
                "Hook error (fail-open)"
            );
            HookOutput {
                continue_execution: Some(true),
                ..Default::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // fail_open
    // -----------------------------------------------------------------------

    #[test]
    fn fail_open_passes_through_ok_output() {
        let output = HookOutput {
            system_message: Some("hello".to_string()),
            continue_execution: Some(false),
            ..Default::default()
        };
        let result = fail_open(Ok(output.clone()));
        assert_eq!(result.system_message, Some("hello".to_string()));
        assert_eq!(result.continue_execution, Some(false));
    }

    #[test]
    fn fail_open_returns_continue_true_on_error() {
        let err = HookError::Io {
            message: "test error".to_string(),
            source: None,
        };
        let result = fail_open(Err(err));
        assert_eq!(
            result.continue_execution,
            Some(true),
            "fail_open should set continue to true on error"
        );
        assert!(
            result.system_message.is_none(),
            "fail_open should not set system_message on error"
        );
    }

    #[test]
    fn fail_open_returns_default_fields_on_error() {
        let err = HookError::Platform {
            message: "platform error".to_string(),
        };
        let result = fail_open(Err(err));
        assert!(result.stop_reason.is_none());
        assert!(result.suppress_output.is_none());
        assert!(result.decision.is_none());
        assert!(result.reason.is_none());
        assert!(result.hook_specific_output.is_none());
    }

    // -----------------------------------------------------------------------
    // write_output — verifies JSON serialization to a buffer
    // We can't easily test stdout, but we verify the serialization logic
    // -----------------------------------------------------------------------

    #[test]
    fn hook_output_default_serializes_to_empty_json() {
        let output = HookOutput::default();
        let json = serde_json::to_string(&output).expect("serialization should succeed");
        assert_eq!(json, "{}");
    }

    #[test]
    fn hook_output_with_continue_serializes_correctly() {
        let output = HookOutput {
            continue_execution: Some(true),
            ..Default::default()
        };
        let json = serde_json::to_string(&output).expect("serialization should succeed");
        assert!(json.contains("\"continue\":true") || json.contains("\"continue\": true"));
    }

    // -----------------------------------------------------------------------
    // read_input — can't easily test stdin, but verify deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn hook_input_deserializes_from_valid_json() {
        let json = r#"{
            "session_id": "s1",
            "transcript_path": "/t.jsonl",
            "cwd": "/tmp",
            "permission_mode": "default",
            "hook_event_name": "SessionStart"
        }"#;
        let input: Result<HookInput, _> = serde_json::from_str(json);
        assert!(input.is_ok());
        assert_eq!(input.unwrap().session_id, "s1");
    }

    #[test]
    fn hook_input_fails_on_missing_required_fields() {
        let json = r#"{"session_id": "s1"}"#;
        let input: Result<HookInput, _> = serde_json::from_str(json);
        assert!(
            input.is_err(),
            "should fail when required fields are missing"
        );
    }
}
