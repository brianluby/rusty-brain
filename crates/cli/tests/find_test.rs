//! Integration tests for `rusty-brain find`.

mod common;

use common::{TestObs, run_cli, setup_test_mind};
use types::ObservationType;

#[test]
fn test_find_returns_matching_results() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "Found authentication pattern in middleware".into(),
            content: Some("JWT tokens validated in auth layer".into()),
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Chose PostgreSQL for user data".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) = run_cli(&path, &["find", "authentication"]);
    assert!(status.success(), "find should succeed");
    assert!(
        stdout.contains("authentication"),
        "output should contain matching result"
    );
}

#[test]
fn test_find_no_results_message() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "Found caching pattern".into(),
        content: None,
    }]);

    let (status, stdout, _stderr) = run_cli(&path, &["find", "nonexistent_xyz_query"]);
    assert!(
        status.success(),
        "find with no results should still succeed"
    );
    assert!(
        stdout.contains("No results found"),
        "should show no results message, got: {stdout}"
    );
}

#[test]
fn test_find_respects_limit() {
    let observations: Vec<TestObs> = (0..10)
        .map(|i| TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: format!("pattern discovery number {i}"),
            content: None,
        })
        .collect();

    let (_dir, path) = setup_test_mind(&observations);

    let (status, stdout, _stderr) = run_cli(
        &path,
        &["find", "pattern discovery", "--limit", "3", "--json"],
    );
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let count = json["count"].as_u64().unwrap();
    assert!(count <= 3, "limit=3 should cap results, got {count}");
}

#[test]
fn test_find_json_output_valid() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Decision,
        tool_name: "Write".into(),
        summary: "Chose asynchronous runtime approach for request handling".into(),
        content: Some("After evaluating synchronous versus asynchronous options we selected the asynchronous runtime".into()),
    }]);

    let (status, stdout, _stderr) = run_cli(&path, &["find", "asynchronous runtime approach", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("should be valid JSON");
    assert!(json["results"].is_array());
    assert!(json["count"].is_number());

    let results = json["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "find should return at least one result"
    );
    let result = &results[0];
    assert!(result["obs_type"].is_string());
    assert!(result["summary"].is_string());
    assert!(result["timestamp"].is_string());
    assert!(result["score"].is_number());
    assert!(result["tool_name"].is_string());
}

#[test]
fn test_find_type_filter() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "Found pattern in codebase".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "Decided on pattern approach".into(),
            content: None,
        },
    ]);

    let (status, stdout, _stderr) =
        run_cli(&path, &["find", "pattern", "--type", "decision", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let results = json["results"].as_array().unwrap();
    for r in results {
        assert_eq!(r["obs_type"].as_str().unwrap(), "decision");
    }
}

#[test]
fn test_find_default_limit_is_10() {
    let observations: Vec<TestObs> = (0..15)
        .map(|i| TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: format!("searchable item number {i}"),
            content: None,
        })
        .collect();

    let (_dir, path) = setup_test_mind(&observations);

    let (status, stdout, _stderr) = run_cli(&path, &["find", "searchable item", "--json"]);
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let count = json["count"].as_u64().unwrap();
    assert!(count <= 10, "default limit should be 10, got {count}");
}

#[test]
fn test_find_empty_pattern_error() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "Some observation".into(),
        content: None,
    }]);

    let (status, _stdout, stderr) = run_cli(&path, &["find", ""]);
    assert!(!status.success(), "empty pattern should fail");
    assert!(
        stderr.contains("empty") || stderr.contains("pattern"),
        "error should mention empty pattern, got: {stderr}"
    );
}

#[test]
fn test_find_type_filter_applies_before_final_limit() {
    let (_dir, path) = setup_test_mind(&[
        TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: "authentication middleware validates bearer credentials in request pipeline".into(),
            content: None,
        },
        TestObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write".into(),
            summary: "authentication middleware decision for bearer credentials validation".into(),
            content: None,
        },
    ]);

    // Even with --limit=1, the type-filtered result should not be dropped.
    let (status, stdout, _stderr) = run_cli(
        &path,
        &[
            "find", "authentication middleware bearer credentials", "--limit", "1", "--type", "decision", "--json",
        ],
    );
    assert!(status.success());

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["obs_type"].as_str().unwrap(), "decision");
}
