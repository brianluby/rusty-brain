//! Tests for pipe detection and JSON output cleanliness.

mod common;

use common::{TestObs, cli_cmd, setup_test_mind};
use types::ObservationType;

#[test]
fn test_json_output_has_no_ansi_escape_codes() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "A test observation for pipe check".into(),
        content: None,
    }]);

    let output = cli_cmd()
        .arg("--memory-path")
        .arg(&path)
        .arg("find")
        .arg("test")
        .arg("--json")
        .output()
        .expect("failed to execute CLI");

    assert!(
        output.status.success(),
        "CLI should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // ANSI escape codes start with \x1b[ (ESC[)
    assert!(
        !stdout.contains('\x1b'),
        "JSON output should not contain ANSI escape codes, got: {stdout}"
    );

    // Also verify it's valid JSON
    let _: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
}
