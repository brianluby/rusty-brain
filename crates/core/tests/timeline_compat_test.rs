// Timeline compatibility tests (T038).
//
// Loads each fixture and verifies the timeline output has the correct
// observation count.

#[path = "compatibility/mod.rs"]
mod compat;

use compat::fixtures;

#[test]
fn small_fixture_timeline_count() {
    let fixture_path = fixtures::fixture_mv2_path("small_10obs");
    assert!(fixture_path.exists(), "small_10obs.mv2 missing");

    compat::assert_compatible_timeline(&fixture_path, 10);
}

#[test]
fn medium_fixture_timeline_count() {
    let fixture_path = fixtures::fixture_mv2_path("medium_100obs");
    assert!(fixture_path.exists(), "medium_100obs.mv2 missing");

    compat::assert_compatible_timeline(&fixture_path, 100);
}

#[test]
fn edge_cases_fixture_timeline_count() {
    let fixture_path = fixtures::fixture_mv2_path("edge_cases");
    assert!(fixture_path.exists(), "edge_cases.mv2 missing");

    // The edge_cases fixture has 6 logical observations, but the long 5000-char
    // summary is chunked into multiple frames by memvid. Timeline returns the
    // deduplicated logical entry count.
    compat::assert_compatible_timeline(&fixture_path, 6);
}
