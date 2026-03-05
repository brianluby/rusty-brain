// Stats compatibility tests (T039).
//
// Loads each fixture and verifies stats match expected observation counts
// and file size is non-zero.

#[path = "compatibility/mod.rs"]
mod compat;

use compat::fixtures;

#[test]
fn small_fixture_stats() {
    let fixture_path = fixtures::fixture_mv2_path("small_10obs");
    assert!(fixture_path.exists(), "small_10obs.mv2 missing");

    compat::assert_compatible_stats(&fixture_path, 10);
}

#[test]
fn medium_fixture_stats() {
    let fixture_path = fixtures::fixture_mv2_path("medium_100obs");
    assert!(fixture_path.exists(), "medium_100obs.mv2 missing");

    compat::assert_compatible_stats(&fixture_path, 100);
}

#[test]
fn edge_cases_fixture_stats() {
    let fixture_path = fixtures::fixture_mv2_path("edge_cases");
    assert!(fixture_path.exists(), "edge_cases.mv2 missing");

    compat::assert_compatible_stats(&fixture_path, 11);
}
