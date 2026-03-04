mod common;

use hooks::session_start::handle_session_start;

#[test]
fn no_memory_file_returns_welcome_message() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::session_start_input_with_cwd(dir.path().to_str().unwrap());

    let output = handle_session_start(&input).unwrap();
    assert!(
        output.system_message.is_some(),
        "should return a system message"
    );
    let msg = output.system_message.unwrap();
    assert!(
        msg.contains("Mind Active") || msg.contains("Commands"),
        "should contain context header or commands section"
    );
}

#[test]
fn returns_system_message_with_commands() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::session_start_input_with_cwd(dir.path().to_str().unwrap());

    let output = handle_session_start(&input).unwrap();
    let msg = output
        .system_message
        .expect("expected system_message to be present");
    assert!(msg.contains("/mind:search"), "should list search command");
    assert!(msg.contains("/mind:ask"), "should list ask command");
    assert!(msg.contains("/mind:recent"), "should list recent command");
    assert!(msg.contains("/mind:stats"), "should list stats command");
}

#[test]
fn error_during_init_returns_err() {
    // Use a regular file as cwd — cannot create memory dir under a file (cross-platform)
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("not-a-dir");
    std::fs::write(&file_path, "blocker").unwrap();
    let input = common::session_start_input_with_cwd(file_path.to_str().unwrap());

    let result = handle_session_start(&input);
    // Handler returns Err; fail-open conversion happens at the I/O boundary in main.rs
    assert!(
        result.is_err(),
        "handle_session_start should return Err for invalid cwd"
    );
}

#[test]
fn legacy_path_detected_includes_migration_suggestion() {
    let dir = tempfile::tempdir().unwrap();
    // Create a legacy .claude/mind.mv2 path
    let legacy_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("mind.mv2"), "dummy").unwrap();

    let input = common::session_start_input_with_cwd(dir.path().to_str().unwrap());
    let output = handle_session_start(&input).unwrap();
    let msg = output.system_message.unwrap();
    assert!(
        msg.contains("legacy") || msg.contains(".claude/mind.mv2") || msg.contains("migration"),
        "should mention legacy path: {msg}"
    );
}
