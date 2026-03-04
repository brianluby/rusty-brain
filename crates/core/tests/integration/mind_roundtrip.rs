// Integration tests for Mind store→search→ask round-trip.
//
// Uses real `MemvidStore` backend against temp `.mv2` files.

mod common {
    include!("../common/mod.rs");
}

use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

// =========================================================================
// T027: Store→search round-trip with real MemvidStore (SC-001)
// =========================================================================

#[test]
fn store_search_round_trip_with_real_backend() {
    let (dir, config) = common::temp_mind_config();

    let mind = Mind::open(config).unwrap();
    assert!(mind.is_initialized());

    // Store observations with all metadata fields.
    let samples = common::sample_observations();
    let mut stored_ids = Vec::new();
    for s in &samples {
        let id = mind
            .remember(s.obs_type, s.tool_name, s.summary, s.content, None)
            .unwrap();
        stored_ids.push(id);
    }

    // Verify all IDs are unique ULIDs.
    for id in &stored_ids {
        assert_eq!(id.len(), 26, "should be 26-char ULID");
    }
    let unique_count = {
        let mut s = stored_ids.clone();
        s.sort();
        s.dedup();
        s.len()
    };
    assert_eq!(unique_count, stored_ids.len(), "all IDs should be unique");

    // Search by known content.
    let results = mind.search("caching pattern", None).unwrap();
    assert!(
        !results.is_empty(),
        "search should find the caching observation"
    );
    let r = &results[0];
    assert_eq!(r.obs_type, ObservationType::Discovery);
    assert_eq!(r.summary, "Found caching pattern in service layer");
    assert_eq!(r.tool_name, "Read");
    assert!(r.score > 0.0);

    // Verify .mv2 file exists and has non-zero size.
    common::assert_mv2_exists(mind.memory_path());

    drop(mind);
    drop(dir);
}

#[test]
fn ask_returns_relevant_content_with_real_backend() {
    let (_dir, config) = common::temp_mind_config();

    let mind = Mind::open(config).unwrap();
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "caching is done via LRU in the service layer",
        Some("Uses an in-memory LRU with 5-minute TTL"),
        None,
    )
    .unwrap();

    let answer = mind.ask("caching").unwrap();
    let text = answer.expect("ask should return Some for matching content");
    assert!(
        text.contains("caching") || text.contains("LRU"),
        "ask should return relevant content, got: {text}"
    );
}

#[test]
fn search_empty_store_returns_empty() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();
    let results = mind.search("anything", None).unwrap();
    assert!(results.is_empty());
}

#[test]
fn stats_reflect_stored_observations() {
    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();

    let stats = mind.stats().unwrap();
    assert_eq!(stats.total_observations, 0);

    mind.remember(
        ObservationType::Decision,
        "Write",
        "Chose async approach",
        None,
        None,
    )
    .unwrap();
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "Found pattern",
        None,
        None,
    )
    .unwrap();

    let stats = mind.stats().unwrap();
    assert_eq!(stats.total_observations, 2);
    assert!(stats.file_size_bytes > 0);
}

// =========================================================================
// SEC-1: .mv2 file has 0600 permissions on creation
// =========================================================================

#[cfg(unix)]
#[test]
fn created_mv2_file_has_0600_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, config) = common::temp_mind_config();
    let mind = Mind::open(config).unwrap();
    let path = mind.memory_path();

    assert!(path.exists(), ".mv2 file should exist after open");
    let perms = std::fs::metadata(path).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o777,
        0o600,
        ".mv2 file should have 0600 permissions (SEC-1)"
    );
}

#[cfg(unix)]
#[test]
fn recovered_mv2_file_has_0600_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupted.mv2");
    // Write garbage to trigger corruption recovery path.
    std::fs::write(&path, b"this is not a valid mv2 file").unwrap();

    let config = MindConfig {
        memory_path: path.clone(),
        ..MindConfig::default()
    };
    let mind = Mind::open(config).unwrap();

    let perms = std::fs::metadata(mind.memory_path()).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o777,
        0o600,
        "recovered .mv2 file should have 0600 permissions (SEC-1)"
    );
}

#[cfg(unix)]
#[test]
fn existing_mv2_file_permissions_hardened_to_0600_on_open() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing-perms.mv2");

    {
        let config = MindConfig {
            memory_path: path.clone(),
            ..MindConfig::default()
        };
        let _mind = Mind::open(config).unwrap();
    }

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let config = MindConfig {
        memory_path: path.clone(),
        ..MindConfig::default()
    };
    let _mind = Mind::open(config).unwrap();

    let perms = std::fs::metadata(&path).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o777,
        0o600,
        "existing store permissions should be hardened to 0600"
    );
}

// =========================================================================
// T071: Read-only filesystem (EC-2)
// =========================================================================

#[cfg(unix)]
#[test]
fn open_read_only_dir_returns_error() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let read_only_dir = dir.path().join("readonly");
    std::fs::create_dir(&read_only_dir).unwrap();
    // Set read-only permissions.
    std::fs::set_permissions(&read_only_dir, std::fs::Permissions::from_mode(0o444)).unwrap();

    let config = MindConfig {
        memory_path: read_only_dir.join("mind.mv2"),
        ..MindConfig::default()
    };

    let result = Mind::open(config);
    // Should fail because we can't create in a read-only directory.
    assert!(
        result.is_err(),
        "opening in read-only dir should return error"
    );

    // Restore permissions for cleanup.
    let _ = std::fs::set_permissions(&read_only_dir, std::fs::Permissions::from_mode(0o755));
}

// T070 (file-deleted-between-operations) is deferred — it requires
// Mind to detect and recreate the .mv2 file, which is not yet implemented.
// The current Mind design doesn't re-validate the file on each operation.
