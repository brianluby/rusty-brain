// Smoke test verifying compatibility test infrastructure compiles and works.
// Actual .mv2 fixture tests are in separate files (US2 phase).

#[path = "compatibility/mod.rs"]
mod compat;

use compat::fixtures;

#[test]
fn fixtures_dir_exists() {
    let dir = fixtures::fixtures_dir();
    assert!(
        dir.exists(),
        "tests/fixtures/ should exist at {}",
        dir.display()
    );
}

#[test]
fn score_tolerance_constant_correct() {
    assert!((fixtures::SCORE_TOLERANCE - 0.01).abs() < f64::EPSILON);
}

#[test]
fn benchmark_threshold_constant_correct() {
    assert!((fixtures::BENCHMARK_THRESHOLD - 2.0).abs() < f64::EPSILON);
}

/// Verify `assert_compatible_search` works with a Rust-generated fixture.
#[test]
fn assert_compatible_search_with_rust_fixture() {
    use rusty_brain_core::mind::Mind;
    use types::{MindConfig, ObservationType};

    let dir = tempfile::tempdir().unwrap();
    let mv2_path = dir.path().join("compat-test.mv2");

    // Create a fixture with known content
    {
        let config = MindConfig {
            memory_path: mv2_path.clone(),
            ..MindConfig::default()
        };
        let mind = Mind::open(config).unwrap();
        mind.remember(
            ObservationType::Discovery,
            "Read",
            "Found authentication pattern in middleware layer",
            Some("JWT token validation happens in the auth middleware"),
            None,
        )
        .unwrap();
    }

    // Use the compatibility helper to verify search works
    let expected = vec![fixtures::ExpectedHit {
        content: "authentication pattern".to_string(),
        rank: 1,
        score_min: 0.0,
        score_max: 10.0, // Wide tolerance for smoke test
    }];

    compat::assert_compatible_search(&mv2_path, "authentication middleware", &expected);
}
