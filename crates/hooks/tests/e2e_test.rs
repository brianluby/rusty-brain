use assert_cmd::Command;

fn rusty_brain_cmd() -> Command {
    assert_cmd::cargo::cargo_bin_cmd!("rusty-brain")
}

fn valid_session_start_json(cwd: &str) -> String {
    serde_json::json!({
        "session_id": "e2e-test-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "source": "startup",
        "platform": "claude"
    })
    .to_string()
}

fn valid_post_tool_use_json(cwd: &str) -> String {
    serde_json::json!({
        "session_id": "e2e-test-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": "/tmp/test.rs"},
        "tool_response": "fn main() {}",
        "tool_use_id": "toolu_01E2E"
    })
    .to_string()
}

fn valid_stop_json(cwd: &str) -> String {
    serde_json::json!({
        "session_id": "e2e-test-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "Stop",
        "stop_hook_active": true,
        "last_assistant_message": "Done."
    })
    .to_string()
}

fn valid_smart_install_json(cwd: &str) -> String {
    serde_json::json!({
        "session_id": "e2e-test-001",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "Notification"
    })
    .to_string()
}

// --- (1) Each subcommand with valid input produces valid HookOutput JSON ---

#[test]
fn session_start_valid_input_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    let json_input = valid_session_start_json(dir.path().to_str().unwrap());

    let output = rusty_brain_cmd()
        .arg("session-start")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

#[test]
fn post_tool_use_valid_input_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    let json_input = valid_post_tool_use_json(dir.path().to_str().unwrap());

    let output = rusty_brain_cmd()
        .arg("post-tool-use")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

#[test]
fn stop_valid_input_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    let json_input = valid_stop_json(dir.path().to_str().unwrap());

    let output = rusty_brain_cmd()
        .arg("stop")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

#[test]
fn smart_install_valid_input_produces_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    let json_input = valid_smart_install_json(dir.path().to_str().unwrap());

    let output = rusty_brain_cmd()
        .arg("smart-install")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

// --- (2) Empty stdin produces fail-open JSON ---

#[test]
fn empty_stdin_produces_fail_open_json() {
    let output = rusty_brain_cmd()
        .arg("session-start")
        .write_stdin("")
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    // Should output valid JSON (either fail-open or empty object)
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

// --- (3) Malformed JSON produces fail-open JSON ---

#[test]
fn malformed_json_produces_fail_open_json() {
    let output = rusty_brain_cmd()
        .arg("post-tool-use")
        .write_stdin("{ this is not valid json }")
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

// --- (4) Unknown subcommand exits 0 with valid JSON ---

#[test]
fn unknown_subcommand_produces_valid_json() {
    // main.rs uses Cli::try_parse() — unknown subcommands return Err,
    // which our handler catches, writes HookOutput::default() {}, and exits 0.
    let output = rusty_brain_cmd()
        .arg("nonexistent-command")
        .write_stdin("{}")
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_object(), "output must be a JSON object");
}

// --- (5) SEC-3: Logging does not leak observation content ---

#[test]
fn sec3_logging_does_not_leak_observation_content() {
    let dir = tempfile::tempdir().unwrap();
    let json_input = valid_post_tool_use_json(dir.path().to_str().unwrap());

    let output = rusty_brain_cmd()
        .env("RUSTY_BRAIN_LOG", "info")
        .arg("post-tool-use")
        .write_stdin(json_input)
        .assert()
        .success();

    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    // SEC-3: tool_response content ("fn main() {}") must not appear in info-level logs
    assert!(
        !stderr.contains("fn main()"),
        "SEC-3 violation: observation content leaked to logs at INFO level"
    );
}

// --- (7) Quickstart validation (C3/T028) ---
// These tests use payloads matching quickstart.md to validate documented examples work.

#[test]
fn quickstart_session_start() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    // Quickstart payload: minimal fields, no source/platform
    let json_input = serde_json::json!({
        "session_id": "test-123",
        "transcript_path": "/tmp/t",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart"
    })
    .to_string();

    let output = rusty_brain_cmd()
        .arg("session-start")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(
        parsed.is_object(),
        "quickstart session-start must produce valid JSON"
    );
}

#[test]
fn quickstart_post_tool_use() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    // Quickstart payload: tool_response is an object, not a string
    let json_input = serde_json::json!({
        "session_id": "test-123",
        "transcript_path": "/tmp/t",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": "src/main.rs"},
        "tool_response": {"content": "fn main() {}"}
    })
    .to_string();

    let output = rusty_brain_cmd()
        .arg("post-tool-use")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(
        parsed.is_object(),
        "quickstart post-tool-use must produce valid JSON"
    );
}

#[test]
fn quickstart_stop() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    // Quickstart payload: no stop_hook_active or last_assistant_message
    let json_input = serde_json::json!({
        "session_id": "test-123",
        "transcript_path": "/tmp/t",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "Stop"
    })
    .to_string();

    let output = rusty_brain_cmd()
        .arg("stop")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(
        parsed.is_object(),
        "quickstart stop must produce valid JSON"
    );
}

#[test]
fn quickstart_smart_install() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();
    // Quickstart payload: minimal notification
    let json_input = serde_json::json!({
        "session_id": "test-123",
        "transcript_path": "/tmp/t",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "Notification"
    })
    .to_string();

    let output = rusty_brain_cmd()
        .arg("smart-install")
        .write_stdin(json_input)
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(
        parsed.is_object(),
        "quickstart smart-install must produce valid JSON"
    );
}

// --- (8) Binary exits 0 for every valid scenario ---

#[test]
fn all_subcommands_exit_zero_with_valid_input() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap();

    for (subcmd, json_fn) in &[
        ("session-start", valid_session_start_json(cwd)),
        ("post-tool-use", valid_post_tool_use_json(cwd)),
        ("stop", valid_stop_json(cwd)),
        ("smart-install", valid_smart_install_json(cwd)),
    ] {
        rusty_brain_cmd()
            .arg(subcmd)
            .write_stdin(json_fn.as_str())
            .assert()
            .success();
    }
}
