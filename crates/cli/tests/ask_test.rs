//! Integration tests for `rusty-brain ask`.

mod common;

use common::{TestObs, run_cli, setup_test_mind};
use types::ObservationType;

#[test]
fn test_ask_returns_answer() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "caching is done via LRU in the service layer".into(),
            content: Some("The LRU cache has a 5-minute TTL".into()),
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Chose PostgreSQL for user storage".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) = run_cli(&path, &["ask", "How is caching implemented?"]);
    assert!(status.success(), "ask should succeed");
    assert!(!stdout.trim().is_empty(), "should return non-empty answer");
}

#[test]
fn test_ask_no_relevant_memories() {
    // Use an empty memory file to guarantee no results.
    let (_dir, path) = setup_test_mind(&[]);

    let (status, stdout, _stderr) = run_cli(
        &path,
        &["ask", "What quantum computing algorithms are used?"],
    );
    assert!(status.success(), "ask with no results should still succeed");
    assert!(
        stdout.contains("No relevant memories found"),
        "should show no results message, got: {stdout}"
    );
}

#[test]
fn test_ask_json_output_valid() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Decision,
        tool_name: "Write".into(),
        summary: "Chose async runtime for performance".into(),
        content: Some("Tokio selected for async I/O".into()),
    }]);

    let (status, stdout, _stderr) = run_cli(&path, &["ask", "async runtime", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["answer"].is_string());
    assert!(json["has_results"].is_boolean());
}
