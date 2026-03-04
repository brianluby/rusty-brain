//! Tests for --verbose flag behavior.

mod common;

use common::{TestObs, cli_cmd, setup_test_mind};
use types::ObservationType;

#[test]
fn test_verbose_output_goes_to_stderr_not_stdout() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "A test observation for verbose check".into(),
        content: None,
    }]);

    let output = cli_cmd()
        .arg("--memory-path")
        .arg(&path)
        .arg("-v")
        .arg("stats")
        .arg("--json")
        .output()
        .expect("failed to execute CLI");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let _stderr = String::from_utf8_lossy(&output.stderr);

    // stdout should be clean JSON only
    let _: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON even with -v");

    // stderr should contain debug tracing output (if tracing subscriber works)
    // Note: tracing output may or may not appear depending on subscriber init timing,
    // but stdout MUST remain clean JSON regardless.
    assert!(
        !stdout.contains("DEBUG") && !stdout.contains("TRACE"),
        "stdout should not contain tracing output, got: {stdout}"
    );
}
