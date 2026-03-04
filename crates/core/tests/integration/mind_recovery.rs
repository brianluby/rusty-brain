// Integration tests for corruption detection and recovery.
//
// Uses real `MemvidStore` backend against temp `.mv2` files.

mod common {
    include!("../common/mod.rs");
}

use rusty_brain_core::mind::Mind;
use types::ObservationType;

// =========================================================================
// T051: Corruption recovery (SC-003)
// =========================================================================

#[test]
fn corrupted_file_is_backed_up_and_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupted.mv2");
    std::fs::write(&path, b"garbage bytes invalid mv2 format").unwrap();

    let config = types::MindConfig {
        memory_path: path.clone(),
        ..types::MindConfig::default()
    };

    let mind = Mind::open(config).unwrap();

    // Verify backup exists.
    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".backup-"))
        .collect();
    assert!(!backups.is_empty(), "backup should exist after recovery");

    // Verify mind is functional.
    mind.remember(
        ObservationType::Discovery,
        "Read",
        "test after recovery",
        None,
        None,
    )
    .unwrap();
    let results = mind.search("recovery", None).unwrap();
    assert!(!results.is_empty(), "search should work after recovery");
}

#[test]
fn backup_count_limited_after_multiple_corruptions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.mv2");

    // Create 4 backups manually with distinct timestamps.
    for i in 0..4 {
        let backup_name = format!("multi.mv2.backup-20260301-00000{i}");
        std::fs::write(dir.path().join(&backup_name), format!("backup {i}")).unwrap();
    }

    // Write garbage and open — creates 5th backup + fresh store.
    std::fs::write(&path, b"corrupt data").unwrap();
    let config = types::MindConfig {
        memory_path: path.clone(),
        ..types::MindConfig::default()
    };
    let _mind = Mind::open(config).unwrap();

    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".backup-"))
        .collect();

    assert!(
        backups.len() <= 3,
        "should keep at most 3 backups, got {}",
        backups.len()
    );
}

#[cfg(unix)]
#[test]
fn permission_errors_do_not_trigger_recovery_backup() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permission-denied.mv2");
    std::fs::write(&path, b"valid content irrelevant").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let config = types::MindConfig {
        memory_path: path.clone(),
        ..types::MindConfig::default()
    };

    let result = Mind::open(config);
    assert!(
        result.is_err(),
        "permission-denied open should fail without recovery"
    );

    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".backup-"))
        .collect();
    assert!(
        backups.is_empty(),
        "non-corruption open failures must not create recovery backups"
    );

    // Restore permissions so tempfile cleanup can remove the file.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
}
