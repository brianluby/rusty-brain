// Fixture loading utilities for compatibility tests.
//
// Provides JSON parsers for `expected_results.json` and `ts_baselines.json`,
// matching the schemas defined in `data-model.md`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Root structure of `expected_results.json`.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ExpectedResults {
    pub fixture: String,
    pub queries: Vec<ExpectedQuery>,
}

/// A single query with expected results.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ExpectedQuery {
    pub query: String,
    pub total_count: usize,
    pub results: Vec<ExpectedHit>,
}

/// A single expected search hit with score tolerance bounds.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ExpectedHit {
    pub content: String,
    pub rank: usize,
    pub score_min: f64,
    pub score_max: f64,
}

/// Root structure of `ts_baselines.json`.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct TypeScriptBaselines {
    pub baselines: Vec<BaselineEntry>,
}

/// A single TypeScript performance baseline measurement.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BaselineEntry {
    pub metric: String,
    pub value: f64,
    pub workload: String,
    pub ts_version: String,
    pub hardware: String,
}

/// Score tolerance for compatibility comparison (±0.01).
pub const SCORE_TOLERANCE: f64 = 0.01;

/// Benchmark speedup threshold (Rust must be ≥2× faster).
pub const BENCHMARK_THRESHOLD: f64 = 2.0;

/// Return the path to the workspace-level `tests/fixtures/` directory.
///
/// Traverses up from the manifest dir to find the workspace root.
pub fn fixtures_dir() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // From crates/core/ go up two levels to workspace root
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("failed to find workspace root")
        .join("tests")
        .join("fixtures")
}

/// Load and parse `expected_results.json` for a given fixture name.
///
/// Returns `None` if the file doesn't exist (fixture not yet generated).
#[allow(dead_code)]
pub fn load_expected_results(fixture_name: &str) -> Option<ExpectedResults> {
    let path = fixtures_dir().join("expected_results.json");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).expect("failed to read expected_results.json");
    let all: Vec<ExpectedResults> =
        serde_json::from_str(&content).expect("failed to parse expected_results.json");
    all.into_iter().find(|r| r.fixture == fixture_name)
}

/// Load and parse `ts_baselines.json`.
///
/// Returns `None` if the file doesn't exist (baselines not yet captured).
#[allow(dead_code)]
pub fn load_ts_baselines() -> Option<TypeScriptBaselines> {
    let path = fixtures_dir().join("ts_baselines.json");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).expect("failed to read ts_baselines.json");
    Some(serde_json::from_str(&content).expect("failed to parse ts_baselines.json"))
}

/// Return the path to a `.mv2` fixture file.
#[allow(dead_code)]
pub fn fixture_mv2_path(fixture_name: &str) -> PathBuf {
    fixtures_dir().join(format!("{fixture_name}.mv2"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_dir_exists() {
        let dir = fixtures_dir();
        assert!(
            dir.exists(),
            "tests/fixtures/ directory should exist at {}",
            dir.display()
        );
    }

    #[test]
    fn score_tolerance_is_correct() {
        assert!((SCORE_TOLERANCE - 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn benchmark_threshold_is_correct() {
        assert!((BENCHMARK_THRESHOLD - 2.0).abs() < f64::EPSILON);
    }
}
