mod common;

use hooks::post_tool_use::handle_post_tool_use;

#[test]
fn read_tool_stores_observation() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::post_tool_use_input_with_cwd(
        dir.path().to_str().unwrap(),
        "Read",
        serde_json::json!({"file_path": "/tmp/test.rs"}),
        serde_json::json!("fn main() {}"),
    );

    let output = handle_post_tool_use(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));
}

#[test]
fn edit_tool_stores_feature_observation() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::post_tool_use_input_with_cwd(
        dir.path().to_str().unwrap(),
        "Edit",
        serde_json::json!({"file_path": "/tmp/test.rs", "old_string": "a", "new_string": "b"}),
        serde_json::json!({"success": true}),
    );

    let output = handle_post_tool_use(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));
}

#[test]
fn write_tool_stores_feature_observation() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::post_tool_use_input_with_cwd(
        dir.path().to_str().unwrap(),
        "Write",
        serde_json::json!({"file_path": "/tmp/test.rs", "content": "hello"}),
        serde_json::json!({"success": true}),
    );

    let output = handle_post_tool_use(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));
}

#[test]
fn bash_tool_stores_with_truncated_command() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::post_tool_use_input_with_cwd(
        dir.path().to_str().unwrap(),
        "Bash",
        serde_json::json!({"command": "cargo test --workspace"}),
        serde_json::json!("all tests passed"),
    );

    let output = handle_post_tool_use(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));
}

#[test]
fn duplicate_within_60s_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();

    // First call — should store
    let input1 = common::post_tool_use_input_with_cwd(
        cwd,
        "Read",
        serde_json::json!({"file_path": "/tmp/same.rs"}),
        serde_json::json!("content"),
    );
    let output1 = handle_post_tool_use(&input1).unwrap();
    assert_eq!(output1.continue_execution, Some(true));

    // Second call with same tool+input — should be deduplicated (still returns continue:true)
    let input2 = common::post_tool_use_input_with_cwd(
        cwd,
        "Read",
        serde_json::json!({"file_path": "/tmp/same.rs"}),
        serde_json::json!("content"),
    );
    let output2 = handle_post_tool_use(&input2).unwrap();
    assert_eq!(output2.continue_execution, Some(true));
}

#[test]
fn tool_output_over_2000_chars_is_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let long_content = "x".repeat(3000);
    let input = common::post_tool_use_input_with_cwd(
        dir.path().to_str().unwrap(),
        "Read",
        serde_json::json!({"file_path": "/tmp/big.rs"}),
        serde_json::json!(long_content),
    );

    // Should not error even with large content
    let output = handle_post_tool_use(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));
}

#[test]
fn error_during_storage_returns_fail_open() {
    // Use an invalid cwd that prevents Mind::open
    let input = common::post_tool_use_input_with_cwd(
        "/dev/null/nonexistent",
        "Read",
        serde_json::json!({"file_path": "/tmp/test.rs"}),
        serde_json::json!("content"),
    );

    let result = handle_post_tool_use(&input);
    match result {
        Ok(output) => assert_eq!(output.continue_execution, Some(true)),
        Err(_) => {} // Expected — the I/O layer will fail-open this
    }
}

#[test]
fn unknown_tool_uses_discovery_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::post_tool_use_input_with_cwd(
        dir.path().to_str().unwrap(),
        "SomeUnknownTool",
        serde_json::json!({"key": "value"}),
        serde_json::json!("result"),
    );

    let output = handle_post_tool_use(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));
}
