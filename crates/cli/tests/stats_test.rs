//! Integration tests for `rusty-brain stats`.

mod common;

use common::{TestObs, run_cli, setup_test_mind};
use types::ObservationType;

#[test]
fn test_stats_displays_summary() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "Found pattern".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Made decision".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) = run_cli(&path, &["stats", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(json["total_observations"].as_u64().unwrap(), 2);
    assert!(json["file_size_bytes"].is_number());
    assert!(json["type_counts"].is_object());

    // Verify snake_case keys (not camelCase)
    assert!(json.get("total_observations").is_some());
    assert!(json.get("total_sessions").is_some());
    assert!(json.get("file_size_bytes").is_some());
    assert!(json.get("type_counts").is_some());
    // Ensure camelCase keys are NOT present
    assert!(json.get("totalObservations").is_none());
    assert!(json.get("fileSize").is_none());
}

#[test]
fn test_stats_empty_memory_file() {
    let (_dir, path) = setup_test_mind(&[]);

    let (status, stdout, _stderr) = run_cli(&path, &["stats", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(json["total_observations"].as_u64().unwrap(), 0);
    assert_eq!(json["total_sessions"].as_u64().unwrap(), 0);
    // oldest_memory and newest_memory should be absent/null
    assert!(
        json.get("oldest_memory").is_none() || json["oldest_memory"].is_null(),
        "oldest_memory should be absent for empty store"
    );
}

#[test]
fn test_stats_json_output_valid() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Bugfix,
        tool_name: "Bash".into(),
        summary: "Fixed the build error".into(),
        content: None,
    }]);

    let (status, stdout, _stderr) = run_cli(&path, &["stats", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["total_observations"].is_number());
    assert!(json["total_sessions"].is_number());
    assert!(json["file_size_bytes"].is_number());
    assert!(json["type_counts"].is_object());
}
