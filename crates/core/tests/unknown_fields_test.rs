// Unknown fields resilience tests (T044).
//
// Verifies that existing .mv2 fixtures load successfully even if future
// versions add fields. Also tests that corrupted fixtures fail gracefully.

#[path = "compatibility/mod.rs"]
mod compat;

use compat::fixtures;
use rusty_brain_core::mind::Mind;
use types::MindConfig;

/// Loading an existing fixture should succeed — the reader must tolerate
/// any extra fields that may have been added by newer writers.
#[test]
fn existing_fixture_loads_and_searches() {
    let fixture_path = fixtures::fixture_mv2_path("small_10obs");
    assert!(fixture_path.exists(), "small_10obs.mv2 missing");

    let config = MindConfig {
        memory_path: fixture_path,
        min_confidence: 0.0,
        ..MindConfig::default()
    };

    let mind = Mind::open(config).expect("should open fixture without error");

    let results = mind
        .search("caching pattern", Some(5))
        .expect("search should succeed on valid fixture");
    assert!(
        !results.is_empty(),
        "should return results from valid fixture"
    );

    let stats = mind.stats().expect("stats should succeed on valid fixture");
    assert_eq!(stats.total_observations, 10);
}

/// Edge-cases fixture with unicode and special content should also load fine.
#[test]
fn edge_cases_fixture_loads_and_searches() {
    let fixture_path = fixtures::fixture_mv2_path("edge_cases");
    assert!(fixture_path.exists(), "edge_cases.mv2 missing");

    let config = MindConfig {
        memory_path: fixture_path,
        min_confidence: 0.0,
        ..MindConfig::default()
    };

    let mind = Mind::open(config).expect("should open edge_cases fixture");

    let results = mind
        .search("unicode emoji", Some(5))
        .expect("search should succeed on edge_cases fixture");
    assert!(
        !results.is_empty(),
        "should find unicode content in edge_cases fixture"
    );

    let stats = mind.stats().expect("stats should succeed");
    assert_eq!(stats.total_observations, 11);
}

/// A corrupted / invalid `.mv2` file should fail gracefully (no panic).
/// `Mind::open` may succeed lazily, but operations on the corrupted data
/// should return errors or empty results rather than panicking.
#[test]
fn corrupted_fixture_fails_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let bad_path = dir.path().join("corrupted.mv2");

    // Write garbage data
    std::fs::write(&bad_path, b"this is not a valid mv2 file").unwrap();

    let config = MindConfig {
        memory_path: bad_path,
        ..MindConfig::default()
    };

    // Mind::open may succeed (lazy initialization). The key requirement
    // is that no operation panics — errors are returned gracefully.
    match Mind::open(config) {
        Err(_) => {
            // Open failed — this is acceptable behavior.
        }
        Ok(mind) => {
            // Open succeeded lazily. Operations should either error or
            // return empty results, but must not panic.
            let search_result = mind.search("test query", Some(5));
            assert!(
                search_result.is_err() || search_result.unwrap().is_empty(),
                "search on corrupted fixture should error or return empty"
            );
        }
    }
}

/// Opening a non-existent file should either create a new empty mind or
/// return an error — either way, it must not panic.
#[test]
fn nonexistent_file_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let missing_path = dir.path().join("does_not_exist.mv2");

    let config = MindConfig {
        memory_path: missing_path,
        ..MindConfig::default()
    };

    // This should either succeed (creating a new file) or fail with an error.
    // The key requirement is that it does not panic.
    let result = Mind::open(config);
    match result {
        Ok(mind) => {
            // If it creates a new empty mind, stats should show 0 observations
            let stats = mind.stats().expect("stats on empty mind should work");
            assert_eq!(stats.total_observations, 0);
        }
        Err(e) => {
            // Acceptable — just verify the error is descriptive
            let msg = format!("{e}");
            assert!(!msg.is_empty(), "error message should be non-empty");
        }
    }
}
