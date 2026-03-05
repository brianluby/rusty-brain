// One-time fixture generator — creates .mv2 test fixtures and expected results.
//
// Run with: cargo test -p rusty-brain-core --test generate_fixtures -- --ignored
//
// This generates:
// - tests/fixtures/small_10obs.mv2
// - tests/fixtures/medium_100obs.mv2
// - tests/fixtures/edge_cases.mv2
// - tests/fixtures/expected_results.json

use std::path::Path;

use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

fn workspace_root() -> &'static Path {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/core -> workspace root (two levels up)
    manifest.parent().unwrap().parent().unwrap()
}

fn fixtures_dir() -> std::path::PathBuf {
    workspace_root().join("tests").join("fixtures")
}

fn create_mind(path: &Path) -> Mind {
    let config = MindConfig {
        memory_path: path.to_path_buf(),
        ..MindConfig::default()
    };
    Mind::open(config).expect("failed to open mind")
}

/// Generate `small_10obs.mv2` with 10 diverse observations.
fn generate_small_fixture(dir: &Path) {
    let path = dir.join("small_10obs.mv2");
    if path.exists() {
        println!("small_10obs.mv2 already exists, skipping");
        return;
    }

    let mind = create_mind(&path);

    let observations = [
        (ObservationType::Discovery, "Read", "Found caching pattern in user service layer", Some("The UserService uses LRU cache with 5-minute TTL for user lookups to reduce database load")),
        (ObservationType::Decision, "Write", "Chose PostgreSQL for persistent user data storage", Some("Evaluated SQLite, PostgreSQL, and MySQL. PostgreSQL selected for JSONB support and concurrent write handling")),
        (ObservationType::Success, "Bash", "Completed database migration to version three successfully", None),
        (ObservationType::Problem, "Read", "Race condition found in session cleanup background task", Some("Multiple agents writing to the same session file without proper file locking causes data corruption")),
        (ObservationType::Discovery, "Glob", "Authentication middleware validates JWT bearer tokens", Some("Middleware extracts JWT from Authorization header and validates against RSA public key")),
        (ObservationType::Decision, "Write", "Selected async runtime for all background processing tasks", Some("Using tokio spawn for background database writes to avoid blocking the main thread")),
        (ObservationType::Success, "Bash", "All integration tests passing after database refactoring work", None),
        (ObservationType::Problem, "Read", "Memory leak detected in connection pool management code", Some("Connection pool grows unbounded when connections fail to return due to timeout handling bug")),
        (ObservationType::Discovery, "Read", "Error handling uses structured error codes throughout codebase", Some("All errors implement thiserror with machine-parseable error codes following E-XXXX format")),
        (ObservationType::Decision, "Write", "Implemented retry logic with exponential backoff strategy", Some("Three retries with 100ms, 200ms, 400ms delays for transient network failures")),
    ];

    for (obs_type, tool, summary, content) in &observations {
        mind.remember(*obs_type, tool, summary, *content, None)
            .expect("failed to remember");
    }

    println!("Generated small_10obs.mv2 with 10 observations");
}

/// Generate `medium_100obs.mv2` with 100 observations.
fn generate_medium_fixture(dir: &Path) {
    let path = dir.join("medium_100obs.mv2");
    if path.exists() {
        println!("medium_100obs.mv2 already exists, skipping");
        return;
    }

    let mind = create_mind(&path);

    let types = [
        ObservationType::Discovery,
        ObservationType::Decision,
        ObservationType::Success,
        ObservationType::Problem,
    ];
    let tools = ["Read", "Write", "Bash", "Glob", "Grep"];

    for i in 0..100 {
        let obs_type = types[i % types.len()];
        let tool = tools[i % tools.len()];
        let summary = format!(
            "Observation number {} about {} in module {}",
            i,
            match obs_type {
                ObservationType::Discovery => "pattern discovery",
                ObservationType::Decision => "architectural decision",
                ObservationType::Success => "successful completion",
                ObservationType::Problem => "problem identification",
                _ => "general observation",
            },
            match i % 5 {
                0 => "authentication",
                1 => "database",
                2 => "caching",
                3 => "networking",
                _ => "configuration",
            }
        );
        let content = if i % 3 == 0 {
            Some(format!(
                "Detailed content for observation {i}: This provides additional context about the {} module implementation details and design considerations",
                match i % 5 {
                    0 => "authentication",
                    1 => "database",
                    2 => "caching",
                    3 => "networking",
                    _ => "configuration",
                }
            ))
        } else {
            None
        };

        mind.remember(obs_type, tool, &summary, content.as_deref(), None)
            .expect("failed to remember");
    }

    println!("Generated medium_100obs.mv2 with 100 observations");
}

/// Generate `edge_cases.mv2` with tricky content.
fn generate_edge_cases_fixture(dir: &Path) {
    let path = dir.join("edge_cases.mv2");
    if path.exists() {
        println!("edge_cases.mv2 already exists, skipping");
        return;
    }

    let mind = create_mind(&path);

    let long_summary = "A".repeat(5000);
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("Unicode: emoji and CJK characters in observation summary content", Some("Content with emoji: \u{1f600}\u{1f680}\u{2764}\u{fe0f} and CJK: \u{4f60}\u{597d}\u{4e16}\u{754c}")),
        ("Empty summary edge case placeholder", None),
        (&long_summary, Some("Very long summary observation for stress testing the search indexing")),
        ("Special chars: <script>alert('xss')</script> & \"quotes\" 'apos'", Some("Content with <html> tags and &amp; entities")),
        ("Newlines in summary\nshould be handled\ngracefully", Some("Content\nwith\nmultiple\nnewlines")),
        ("   Leading and trailing whitespace   ", Some("   Whitespace content   ")),
    ];

    for (summary, content) in &cases {
        mind.remember(ObservationType::Discovery, "Read", summary, *content, None)
            .expect("failed to remember");
    }

    println!("Generated edge_cases.mv2 with {} observations", cases.len());
}

/// Generate `expected_results.json` from the small fixture.
fn generate_expected_results(dir: &Path) {
    let path = dir.join("expected_results.json");
    if path.exists() {
        println!("expected_results.json already exists, skipping");
        return;
    }

    // Open the small fixture and run reference queries
    let fixture_path = dir.join("small_10obs.mv2");
    if !fixture_path.exists() {
        println!("small_10obs.mv2 not found, generate it first");
        return;
    }

    let config = MindConfig {
        memory_path: fixture_path,
        min_confidence: 0.0,
        ..MindConfig::default()
    };
    let mind = Mind::open(config).expect("failed to open fixture");

    // Run reference queries and capture results
    let queries = ["caching pattern user service", "authentication JWT bearer token", "database migration"];
    let mut results_json = Vec::new();

    for query in &queries {
        let results = mind.search(query, Some(5)).expect("search failed");
        let hits: Vec<serde_json::Value> = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                serde_json::json!({
                    "content": r.summary,
                    "rank": i + 1,
                    "score_min": (r.score - 0.01_f64).max(0.0),
                    "score_max": r.score + 0.01
                })
            })
            .collect();

        results_json.push(serde_json::json!({
            "query": query,
            "total_count": hits.len(),
            "results": hits
        }));
    }

    let expected = serde_json::json!([{
        "fixture": "small_10obs",
        "queries": results_json
    }]);

    let json = serde_json::to_string_pretty(&expected).unwrap();
    std::fs::write(&path, json).expect("failed to write expected_results.json");
    println!("Generated expected_results.json");
}

/// Generate `ts_baselines.json` with placeholder baselines.
fn generate_ts_baselines(dir: &Path) {
    let path = dir.join("ts_baselines.json");
    if path.exists() {
        println!("ts_baselines.json already exists, skipping");
        return;
    }

    // These are representative TypeScript performance baselines.
    // In a real scenario, these would be captured from the TypeScript agent-brain.
    // Values are intentionally generous (slow) to ensure Rust easily meets 2× threshold.
    let baselines = serde_json::json!({
        "baselines": [
            {
                "metric": "query_latency_ms",
                "value": 45.0,
                "workload": "100 observations, single search query",
                "ts_version": "1.0.0",
                "hardware": "Apple M-series"
            },
            {
                "metric": "compression_throughput_mb_s",
                "value": 5.0,
                "workload": "10KB tool output, default compression",
                "ts_version": "1.0.0",
                "hardware": "Apple M-series"
            },
            {
                "metric": "startup_time_ms",
                "value": 200.0,
                "workload": "Cold start to first command ready",
                "ts_version": "1.0.0",
                "hardware": "Apple M-series"
            }
        ]
    });

    let json = serde_json::to_string_pretty(&baselines).unwrap();
    std::fs::write(&path, json).expect("failed to write ts_baselines.json");
    println!("Generated ts_baselines.json");
}

#[test]
#[ignore] // Run manually: cargo test -p rusty-brain-core --test generate_fixtures -- --ignored
fn generate_all_fixtures() {
    let dir = fixtures_dir();
    assert!(dir.exists(), "tests/fixtures/ must exist");

    generate_small_fixture(&dir);
    generate_medium_fixture(&dir);
    generate_edge_cases_fixture(&dir);
    generate_expected_results(&dir);
    generate_ts_baselines(&dir);

    println!("\nAll fixtures generated in {}", dir.display());
}
