//! Fail-open wrapper unit tests (T005).

use opencode::types::MindToolOutput;

#[test]
fn handle_with_failopen_success_passes_through() {
    let output = opencode::handle_with_failopen(|| {
        Ok(::types::HookOutput {
            system_message: Some("hello".to_string()),
            ..Default::default()
        })
    });

    assert_eq!(output.system_message, Some("hello".to_string()));
}

#[test]
fn handle_with_failopen_error_returns_default() {
    let output = opencode::handle_with_failopen(|| {
        Err(::types::RustyBrainError::InvalidInput {
            code: ::types::error_codes::E_INPUT_EMPTY_FIELD,
            message: "test error".to_string(),
        })
    });

    // Default HookOutput has all fields None
    assert_eq!(output, ::types::HookOutput::default());
}

#[test]
fn handle_with_failopen_panic_returns_default() {
    let output = opencode::handle_with_failopen(|| {
        panic!("deliberate test panic");
    });

    assert_eq!(output, ::types::HookOutput::default());
}

#[test]
fn mind_tool_with_failopen_success_passes_through() {
    let output = opencode::mind_tool_with_failopen(|| {
        Ok(MindToolOutput::success(serde_json::json!({"count": 5})))
    });

    assert!(output.success);
    assert_eq!(output.data, Some(serde_json::json!({"count": 5})));
}

#[test]
fn mind_tool_with_failopen_error_returns_failure() {
    let output = opencode::mind_tool_with_failopen(|| {
        Err(::types::RustyBrainError::InvalidInput {
            code: ::types::error_codes::E_INPUT_EMPTY_FIELD,
            message: "test error".to_string(),
        })
    });

    assert!(!output.success);
    assert_eq!(output.error, Some("internal error".to_string()));
    assert!(output.data.is_none());
}

#[test]
fn mind_tool_with_failopen_panic_returns_failure() {
    let output = opencode::mind_tool_with_failopen(|| {
        panic!("deliberate mind tool panic");
    });

    assert!(!output.success);
    assert_eq!(output.error, Some("internal error".to_string()));
    assert!(output.data.is_none());
}
