//! Integration tests for install JSON output (T064-T066).
//!
//! Tests the CLI binary's install subcommand JSON output format,
//! non-TTY auto-detection, and error JSON structure.

use assert_cmd::Command;

fn cli_cmd() -> Command {
    Command::cargo_bin("rusty-brain").expect("binary should exist")
}

// T064 / AC-8: --json output matches InstallReport schema.
#[test]
fn install_json_output_is_valid_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = cli_cmd()
        .args(["install", "--project", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output should be valid JSON");

    // Verify InstallReport structure.
    assert!(parsed["status"].is_string(), "should have status field");
    assert!(parsed["results"].is_array(), "should have results array");
    assert!(
        parsed["memory_store"].is_string(),
        "should have memory_store"
    );
    assert!(parsed["scope"].is_string(), "should have scope");
}

// T064: Each result has expected fields.
#[test]
fn install_json_results_have_expected_fields() {
    let dir = tempfile::tempdir().unwrap();

    let output = cli_cmd()
        .args(["install", "--project", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let results = parsed["results"].as_array().unwrap();
    for result in results {
        assert!(
            result["agent_name"].is_string(),
            "each result should have agent_name"
        );
        assert!(
            result["status"].is_string(),
            "each result should have status"
        );
    }
}

// T066 / AC-11, SEC-10: Error JSON has code and message fields.
// Error JSON is written to stderr by print_error_json().
#[test]
fn install_error_json_has_code_and_message() {
    let output = cli_cmd()
        .args(["install", "--json"])
        .output()
        .expect("failed to execute CLI");

    assert!(!output.status.success(), "should fail without scope");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(&stderr).expect("error output should be valid JSON on stderr");

    assert!(parsed["code"].is_string(), "error should have code field");
    assert!(
        parsed["message"].is_string(),
        "error should have message field"
    );

    let code = parsed["code"].as_str().unwrap();
    assert!(
        code.starts_with("E_"),
        "error code should start with E_: {code}"
    );
}

// T066: Invalid agent error JSON on stderr.
#[test]
fn install_invalid_agent_error_json() {
    let dir = tempfile::tempdir().unwrap();

    let output = cli_cmd()
        .args(["install", "--agents", "nonexistent", "--project", "--json"])
        .current_dir(dir.path())
        .output()
        .expect("failed to execute CLI");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(&stderr).expect("error JSON should be on stderr");

    assert_eq!(parsed["code"], "E_CLI_INSTALL");
    let message = parsed["message"].as_str().unwrap();
    assert!(
        message.contains("E_INSTALL_INVALID_AGENT"),
        "should contain error code: {message}"
    );
}
