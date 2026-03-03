//! Integration tests for `rusty-brain timeline`.

mod common;

use common::{TestObs, run_cli, setup_test_mind};
use types::ObservationType;

#[test]
fn test_timeline_reverse_chronological() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "First observation ever".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Second observation made".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Bugfix,
            tool_name: "Bash".into(),
            summary: "Third observation latest".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) = run_cli(&path, &["timeline", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);

    // Default is most-recent-first
    assert!(
        entries[0]["summary"].as_str().unwrap().contains("Third"),
        "most recent should be first"
    );
    assert!(
        entries[2]["summary"].as_str().unwrap().contains("First"),
        "oldest should be last"
    );
}

#[test]
fn test_timeline_oldest_first() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "First observation".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Second observation".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) = run_cli(&path, &["timeline", "--oldest-first", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);

    assert!(
        entries[0]["summary"].as_str().unwrap().contains("First"),
        "oldest should be first with --oldest-first"
    );
}

#[test]
fn test_timeline_respects_limit() {
    let observations: Vec<TestObs> = (0..10)
        .map(|i| TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: format!("timeline entry number {i}"),
            content: None,
        })
        .collect();

    let (_dir, path) = setup_test_mind(&observations);

    let (status, stdout, _stderr) = run_cli(&path, &["timeline", "--limit", "3", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let count = json["count"].as_u64().unwrap();
    assert_eq!(count, 3);
}

#[test]
fn test_timeline_json_output_valid() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Pattern,
        tool_name: "Read".into(),
        summary: "Found repository pattern".into(),
        content: None,
    }]);

    let (status, stdout, _stderr) = run_cli(&path, &["timeline", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["entries"].is_array());
    assert!(json["count"].is_number());

    let entries = json["entries"].as_array().unwrap();
    assert!(!entries.is_empty(), "timeline should have at least one entry");
    let entry = &entries[0];
    assert!(entry["obs_type"].is_string());
    assert!(entry["summary"].is_string());
    assert!(entry["timestamp"].is_string());
    // Timeline entries should NOT have score or tool_name (per contract)
    assert!(entry.get("score").is_none());
    assert!(entry.get("tool_name").is_none());
}

#[test]
fn test_timeline_type_filter() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "A discovery entry".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "A decision entry".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "Another discovery entry".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) = run_cli(&path, &["timeline", "--type", "discovery", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for e in entries {
        assert_eq!(e["obs_type"].as_str().unwrap(), "discovery");
    }
}

#[test]
fn test_timeline_default_limit_is_10() {
    let observations: Vec<TestObs> = (0..15)
        .map(|i| TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: format!("timeline item {i}"),
            content: None,
        })
        .collect();

    let (_dir, path) = setup_test_mind(&observations);

    // Run without --limit flag
    let (status, stdout, _stderr) = run_cli(&path, &["timeline", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let count = json["count"].as_u64().unwrap();
    assert_eq!(count, 10, "default limit should be 10, got {count}");
}
