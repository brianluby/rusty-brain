mod common;

use hooks::error::HookError;
use hooks::io::{fail_open, read_input, write_output};
use types::hooks::{HookInput, HookOutput};

#[test]
fn read_input_valid_json_returns_hook_input() {
    let json = r#"{"session_id":"s1","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"SessionStart"}"#;
    let cursor = std::io::Cursor::new(json.as_bytes());
    let input: HookInput = serde_json::from_reader(cursor).unwrap();
    assert_eq!(input.session_id, "s1");
    assert_eq!(input.hook_event_name, "SessionStart");
}

#[test]
fn read_input_empty_stdin_returns_error() {
    let cursor = std::io::Cursor::new(b"");
    let result: Result<HookInput, _> = serde_json::from_reader(cursor);
    assert!(result.is_err());
}

#[test]
fn read_input_malformed_json_returns_error() {
    let cursor = std::io::Cursor::new(b"not json at all");
    let result: Result<HookInput, _> = serde_json::from_reader(cursor);
    assert!(result.is_err());
}

#[test]
fn read_input_unknown_fields_are_ignored() {
    let json = r#"{"session_id":"s1","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"Stop","future_field":"hello"}"#;
    let cursor = std::io::Cursor::new(json.as_bytes());
    let input: HookInput = serde_json::from_reader(cursor).unwrap();
    assert_eq!(input.session_id, "s1");
}

#[test]
fn write_output_produces_valid_json() {
    let output = HookOutput {
        continue_execution: Some(true),
        ..Default::default()
    };
    let mut buf = Vec::new();
    serde_json::to_writer(&mut buf, &output).unwrap();
    let json_str = String::from_utf8(buf).unwrap();
    assert!(json_str.contains("\"continue\":true"));
}

#[test]
fn fail_open_ok_returns_output_unchanged() {
    let expected = HookOutput {
        system_message: Some("hello".to_string()),
        ..Default::default()
    };
    let result = fail_open(Ok(expected.clone()));
    assert_eq!(result, expected);
}

#[test]
fn fail_open_err_returns_continue_true() {
    let err = HookError::Io {
        message: "test error".to_string(),
        source: None,
    };
    let result = fail_open(Err(err));
    assert_eq!(result.continue_execution, Some(true));
    assert!(result.system_message.is_none());
}
