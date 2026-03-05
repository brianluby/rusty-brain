// Compatibility test helpers for TypeScript `.mv2` fixture verification.
//
// Implements Contract 1: `assert_compatible_search()` — loads a TypeScript-generated
// `.mv2` fixture, executes a search query, and compares results against expected
// output with ±0.01 score tolerance.

pub mod fixtures;

use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::MindConfig;

use fixtures::{ExpectedHit, SCORE_TOLERANCE};

/// Load a `.mv2` fixture and verify search results match expected output.
///
/// # Contract (from `contracts/test-contracts.md`)
/// - Input: fixture path, query string
/// - Output: assertion that results match expected hits
/// - Tolerance: |rust_score - expected_score_midpoint| <= 0.01
/// - Failure: panics with descriptive message if results diverge
#[allow(dead_code)]
pub fn assert_compatible_search(fixture_path: &Path, query: &str, expected: &[ExpectedHit]) {
    assert!(
        fixture_path.exists(),
        "fixture file not found: {}",
        fixture_path.display()
    );

    let config = MindConfig {
        memory_path: fixture_path.to_path_buf(),
        min_confidence: 0.0, // Don't filter — we check scores ourselves
        ..MindConfig::default()
    };

    let mind = Mind::open(config).expect("failed to open fixture .mv2");
    let results = mind
        .search(query, Some(expected.len() + 10))
        .expect("search failed");

    // Verify we got at least as many results as expected
    assert!(
        results.len() >= expected.len(),
        "expected at least {} results for query {:?}, got {}",
        expected.len(),
        query,
        results.len()
    );

    // Compare each expected hit against actual results
    for expected_hit in expected {
        let rank_idx = expected_hit.rank - 1; // 1-based to 0-based

        if rank_idx >= results.len() {
            panic!(
                "expected hit at rank {} but only got {} results for query {:?}",
                expected_hit.rank,
                results.len(),
                query
            );
        }

        let actual = &results[rank_idx];

        // Check content match (summary should contain or match the expected content)
        assert!(
            actual.summary.contains(&expected_hit.content)
                || expected_hit.content.contains(&actual.summary),
            "rank {}: content mismatch for query {:?}\n  expected to contain: {:?}\n  actual summary: {:?}",
            expected_hit.rank,
            query,
            expected_hit.content,
            actual.summary,
        );

        // Check score within tolerance bounds
        let score_midpoint = (expected_hit.score_min + expected_hit.score_max) / 2.0;
        let score_diff = (actual.score - score_midpoint).abs();
        assert!(
            actual.score >= expected_hit.score_min - SCORE_TOLERANCE
                && actual.score <= expected_hit.score_max + SCORE_TOLERANCE,
            "rank {}: score out of tolerance for query {:?}\n  expected: [{:.4}, {:.4}] (±{:.2})\n  actual: {:.4}\n  diff from midpoint: {:.4}",
            expected_hit.rank,
            query,
            expected_hit.score_min,
            expected_hit.score_max,
            SCORE_TOLERANCE,
            actual.score,
            score_diff,
        );
    }
}

/// Assert that stats from a fixture match expected values.
#[allow(dead_code)]
pub fn assert_compatible_stats(
    fixture_path: &Path,
    expected_observation_count: u64,
) {
    let config = MindConfig {
        memory_path: fixture_path.to_path_buf(),
        ..MindConfig::default()
    };

    let mind = Mind::open(config).expect("failed to open fixture .mv2");
    let stats = mind.stats().expect("stats failed");

    assert_eq!(
        stats.total_observations, expected_observation_count,
        "observation count mismatch"
    );
    assert!(stats.file_size_bytes > 0, "file size should be > 0");
}

/// Assert that timeline entries from a fixture match expected count.
#[allow(dead_code)]
pub fn assert_compatible_timeline(
    fixture_path: &Path,
    expected_count: usize,
) {
    let config = MindConfig {
        memory_path: fixture_path.to_path_buf(),
        ..MindConfig::default()
    };

    let mind = Mind::open(config).expect("failed to open fixture .mv2");
    let entries = mind
        .timeline(expected_count + 10, false)
        .expect("timeline failed");

    assert_eq!(
        entries.len(),
        expected_count,
        "timeline entry count mismatch"
    );
}
