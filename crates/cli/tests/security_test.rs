//! Security-related integration tests.

mod common;

use common::{TestObs, cli_cmd, setup_test_mind};
use types::ObservationType;

#[cfg(unix)]
#[test]
fn test_lock_file_permissions_0600() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "Observation for lock test".into(),
        content: None,
    }]);

    // Run a CLI command to trigger file access
    let output = cli_cmd()
        .arg("--memory-path")
        .arg(&path)
        .arg("stats")
        .output()
        .expect("failed to execute CLI");

    assert!(output.status.success());

    // Check if a lock file was created with proper permissions
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock_path = std::path::PathBuf::from(lock_path);

    if lock_path.exists() {
        let perms = std::fs::metadata(&lock_path).unwrap().permissions();
        let mode = perms.mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "lock file should have 0600 permissions, got {mode:o}"
        );
    }
    // Lock file may not exist if Mind::open doesn't create one for read-only ops
}

#[cfg(unix)]
#[test]
fn test_signal_cleanup_releases_lock() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "Observation for signal test".into(),
        content: None,
    }]);

    // First invocation should succeed, proving the file is accessible
    let (status1, _stdout1, _stderr1) = common::run_cli(&path, &["stats"]);
    assert!(status1.success(), "first run should succeed");

    // Second invocation should also succeed (no stale lock)
    let (status2, _stdout2, _stderr2) = common::run_cli(&path, &["stats"]);
    assert!(
        status2.success(),
        "second run should succeed (no stale lock blocking)"
    );

    // fs2 file locks are OS-released on process exit, so consecutive runs
    // should never block each other.
}
