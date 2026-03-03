// Shared test helpers for `rusty-brain-core` integration tests.
//
// Provides temp directory creation, observation builders, and assertion
// helpers for `MemorySearchResult` field verification.

use std::path::{Path, PathBuf};
use tempfile::TempDir;
use types::{MindConfig, ObservationType};

/// Create a `MindConfig` pointing at a temp directory `.mv2` file.
///
/// Returns `(TempDir, MindConfig)`. The caller must hold `TempDir` alive
/// for the duration of the test to prevent premature cleanup.
#[allow(dead_code)]
pub fn temp_mind_config() -> (TempDir, MindConfig) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let memory_path = dir.path().join("test-brain.mv2");
    let config = MindConfig {
        memory_path,
        ..MindConfig::default()
    };
    (dir, config)
}

/// Return the `.mv2` path from a `MindConfig`.
#[allow(dead_code)]
pub fn mv2_path(config: &MindConfig) -> &Path {
    &config.memory_path
}

/// Sample observations for populating a Mind in tests.
#[allow(dead_code)]
pub struct SampleObs {
    pub obs_type: ObservationType,
    pub tool_name: &'static str,
    pub summary: &'static str,
    pub content: Option<&'static str>,
}

/// Return a set of diverse sample observations for testing.
#[allow(dead_code)]
pub fn sample_observations() -> Vec<SampleObs> {
    vec![
        SampleObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read",
            summary: "Found caching pattern in service layer",
            content: Some("The UserService uses an LRU cache with 5-minute TTL for user lookups"),
        },
        SampleObs {
            obs_type: ObservationType::Decision,
            tool_name: "Write",
            summary: "Chose async over sync for database operations",
            content: Some(
                "Using tokio::spawn for background DB writes to avoid blocking the main thread",
            ),
        },
        SampleObs {
            obs_type: ObservationType::Success,
            tool_name: "Bash",
            summary: "Completed Phase 1 setup tasks",
            content: None,
        },
        SampleObs {
            obs_type: ObservationType::Problem,
            tool_name: "Read",
            summary: "Race condition in session cleanup",
            content: Some("Multiple agents writing to the same .mv2 file without file locking"),
        },
    ]
}

/// Assert that a memory path exists and has non-zero size.
#[allow(dead_code)]
pub fn assert_mv2_exists(path: &Path) {
    assert!(path.exists(), "expected .mv2 file at {}", path.display());
    let meta = std::fs::metadata(path).expect("failed to read .mv2 metadata");
    assert!(meta.len() > 0, ".mv2 file should have non-zero size");
}

/// Assert that a path does NOT exist (for testing cleanup / deletion scenarios).
#[allow(dead_code)]
pub fn assert_path_missing(path: &Path) {
    assert!(
        !path.exists(),
        "expected path to not exist: {}",
        path.display()
    );
}

/// Return a unique `.mv2` path inside a given temp directory.
#[allow(dead_code)]
pub fn unique_mv2_path(dir: &Path) -> PathBuf {
    let id = ulid::Ulid::new().to_string().to_lowercase();
    dir.join(format!("test-{id}.mv2"))
}
