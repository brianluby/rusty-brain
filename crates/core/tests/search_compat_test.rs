// Search compatibility tests (T037).
//
// Loads each fixture (.mv2), runs queries from expected_results.json,
// and asserts results match with +/-0.01 score tolerance.

#[path = "compatibility/mod.rs"]
mod compat;

use compat::fixtures;

#[test]
fn small_fixture_expected_queries() {
    let expected = fixtures::load_expected_results("small_10obs")
        .expect("expected_results.json should contain small_10obs fixture");

    let fixture_path = fixtures::fixture_mv2_path("small_10obs");
    assert!(
        fixture_path.exists(),
        "small_10obs.mv2 fixture missing at {}",
        fixture_path.display()
    );

    for eq in &expected.queries {
        compat::assert_compatible_search(&fixture_path, &eq.query, &eq.results);
    }
}

#[test]
fn medium_fixture_generic_queries() {
    let fixture_path = fixtures::fixture_mv2_path("medium_100obs");
    assert!(
        fixture_path.exists(),
        "medium_100obs.mv2 fixture missing at {}",
        fixture_path.display()
    );

    // Generic queries that should match observations in the medium fixture.
    // We verify that search returns at least one result for each query.
    let queries = [
        "authentication pattern discovery",
        "database architectural decision",
        "caching module observation",
    ];

    let config = types::MindConfig {
        memory_path: fixture_path.clone(),
        min_confidence: 0.0,
        ..types::MindConfig::default()
    };
    let mind = rusty_brain_core::mind::Mind::open(config).expect("failed to open medium fixture");

    for query in &queries {
        let results = mind
            .search(query, Some(10))
            .expect("search should not fail");
        assert!(
            !results.is_empty(),
            "expected at least one result for query {query:?} in medium fixture"
        );
    }
}

#[test]
fn edge_cases_fixture_unicode_queries() {
    let fixture_path = fixtures::fixture_mv2_path("edge_cases");
    assert!(
        fixture_path.exists(),
        "edge_cases.mv2 fixture missing at {}",
        fixture_path.display()
    );

    let config = types::MindConfig {
        memory_path: fixture_path.clone(),
        min_confidence: 0.0,
        ..types::MindConfig::default()
    };
    let mind =
        rusty_brain_core::mind::Mind::open(config).expect("failed to open edge_cases fixture");

    // Unicode query — should not panic or error
    let results = mind
        .search("emoji CJK characters", Some(10))
        .expect("unicode query should not fail");
    assert!(
        !results.is_empty(),
        "expected at least one result for unicode query in edge_cases fixture"
    );

    // Special characters query
    let results = mind
        .search("special chars script alert", Some(10))
        .expect("special chars query should not fail");
    assert!(
        !results.is_empty(),
        "expected at least one result for special chars query"
    );

    // Whitespace edge case
    let results = mind
        .search("leading trailing whitespace", Some(10))
        .expect("whitespace query should not fail");
    assert!(
        !results.is_empty(),
        "expected at least one result for whitespace query"
    );
}
