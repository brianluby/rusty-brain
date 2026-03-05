//! Agent integration smoke test (T076).
//!
//! Verifies the full CLI workflow that an AI coding agent would perform:
//! store observations via the library API, then query them through the CLI
//! binary with structured JSON output.

mod common;

use common::{TestObs, run_cli, setup_test_mind};
use types::ObservationType;

/// Full agent workflow: remember → stats → find → timeline.
/// Validates every subcommand produces valid, machine-parseable JSON.
#[test]
fn full_agent_workflow_produces_structured_json() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "Found caching middleware in service layer".into(),
            content: Some("LRU cache with 5-minute TTL".into()),
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Selected PostgreSQL for persistence".into(),
            content: Some("Evaluated SQLite vs PG; chose PG for JSONB".into()),
        },
        TestObs {
            obs_type: ObservationType::Bugfix,
            tool_name: "Bash".into(),
            summary: "Fixed race condition in connection pool".into(),
            content: None,
        },
    ]);

    // Step 1: stats --json → valid JSON with correct counts
    let (status, stdout, _) = run_cli(&path, &["stats", "--json"]);
    assert!(status.success(), "stats should succeed");
    let stats: serde_json::Value = serde_json::from_str(&stdout).expect("stats: valid JSON");
    assert_eq!(stats["total_observations"].as_u64().unwrap(), 3);
    assert!(stats["file_size_bytes"].as_u64().unwrap() > 0);

    // Step 2: find --json → valid JSON with results array
    let (status, stdout, _) = run_cli(
        &path,
        &["find", "caching middleware service layer", "--json"],
    );
    assert!(status.success(), "find should succeed");
    let find: serde_json::Value = serde_json::from_str(&stdout).expect("find: valid JSON");
    assert!(find["results"].is_array());
    assert!(find["count"].as_u64().unwrap() >= 1);
    let r = &find["results"][0];
    assert!(r["summary"].is_string());
    assert!(r["score"].is_number());
    assert!(r["timestamp"].is_string());

    // Step 3: timeline --json → valid JSON with entries array
    let (status, stdout, _) = run_cli(&path, &["timeline", "--json"]);
    assert!(status.success(), "timeline should succeed");
    let tl: serde_json::Value = serde_json::from_str(&stdout).expect("timeline: valid JSON");
    assert!(tl["entries"].is_array());
    let entries = tl["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "timeline should have 3 entries");
}

/// Verifies error output is also structured and agent-friendly.
#[test]
fn error_output_is_agent_friendly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing_path = dir.path().join("does-not-exist.mv2");

    let (status, _stdout, stderr) = run_cli(&missing_path, &["stats"]);
    assert!(!status.success());
    // Error should be human-readable (no panics, no raw backtraces)
    assert!(
        !stderr.contains("panicked"),
        "error should not be a panic: {stderr}"
    );
    assert!(
        stderr.contains("not found") || stderr.contains("Memory file"),
        "error should be descriptive: {stderr}"
    );
}

/// Verifies that exit codes follow the documented convention:
/// 0 = success, 1 = user error, 2 = lock timeout.
#[test]
fn exit_codes_follow_convention() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "test observation for exit code verification".into(),
        content: None,
    }]);

    // Success → exit 0
    let (status, _, _) = run_cli(&path, &["stats"]);
    assert_eq!(status.code(), Some(0));

    // Missing file → exit 1
    let dir2 = tempfile::tempdir().expect("tempdir");
    let missing = dir2.path().join("missing.mv2");
    let (status, _, _) = run_cli(&missing, &["stats"]);
    assert_eq!(status.code(), Some(1));
}
