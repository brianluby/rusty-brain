//! Integration tests for CLI error handling.

mod common;

use common::{TestObs, cli_cmd, run_cli, setup_test_mind};
use types::ObservationType;

#[test]
fn test_no_args_shows_help() {
    let output = cli_cmd().output().expect("failed to execute CLI");

    // clap shows help and exits 2 when no subcommand provided
    // (arg_required_else_help = true)
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("Usage:") || combined.contains("USAGE:"),
        "should show usage help, got: {combined}"
    );
}

#[test]
fn test_missing_memory_file_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("nonexistent.mv2");
    // Path doesn't exist since we didn't create it

    let (status, _stdout, stderr) = run_cli(&path, &["stats"]);
    assert!(!status.success(), "should fail with missing file");
    assert!(
        stderr.contains("not found") || stderr.contains("Memory file not found"),
        "should mention file not found, got: {stderr}"
    );
    assert!(
        stderr.contains("nonexistent.mv2"),
        "should include the path in error, got: {stderr}"
    );
}

#[test]
fn test_invalid_limit_zero() {
    let (_dir, path) = setup_test_mind(&[]);

    let (status, _stdout, stderr) = run_cli(&path, &["find", "test", "--limit", "0"]);
    assert!(!status.success(), "limit=0 should fail");
    assert!(
        stderr.contains("0") || stderr.contains("limit") || stderr.contains("at least 1"),
        "should mention invalid limit, got: {stderr}"
    );
}

#[test]
fn test_invalid_type_lists_valid() {
    let (_dir, path) = setup_test_mind(&[]);

    let (status, _stdout, stderr) = run_cli(&path, &["find", "test", "--type", "invalid_type"]);
    assert!(!status.success(), "invalid type should fail");
    assert!(
        stderr.contains("discovery") && stderr.contains("decision"),
        "should list valid types, got: {stderr}"
    );
}

#[test]
fn test_memory_path_not_a_file() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let dir_path = dir.path().to_path_buf();

    let (status, _stdout, stderr) = run_cli(&dir_path, &["stats"]);
    assert!(!status.success(), "directory path should fail");
    assert!(
        stderr.contains("directory") || stderr.contains("not a file"),
        "should mention path is not a file, got: {stderr}"
    );
}

#[test]
fn test_exit_code_zero_on_success() {
    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "A test observation".into(),
        content: None,
    }]);

    let (status, _stdout, _stderr) = run_cli(&path, &["stats"]);
    assert!(status.success(), "stats should succeed with exit 0");

    #[cfg(unix)]
    {
        assert_eq!(status.code(), Some(0));
    }
}

#[test]
fn test_exit_code_nonzero_on_failure() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("nonexistent.mv2");

    let (status, _stdout, _stderr) = run_cli(&path, &["stats"]);
    assert!(!status.success(), "should fail");

    #[cfg(unix)]
    {
        assert_eq!(status.code(), Some(1));
    }
}

#[test]
fn test_lock_timeout_exit_code_2() {
    use fs2::FileExt;

    let (_dir, path) = setup_test_mind(&[TestObs {
        obs_type: ObservationType::Discovery,
        tool_name: "Read".into(),
        summary: "Observation for lock timeout test".into(),
        content: None,
    }]);

    // Create the lock file path (same convention as Mind::with_lock)
    let mut lock_os = path.as_os_str().to_os_string();
    lock_os.push(".lock");
    let lock_path = std::path::PathBuf::from(lock_os);

    // Acquire exclusive lock from the test process to block the CLI
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .expect("failed to open lock file");
    lock_file.lock_exclusive().expect("failed to acquire lock");

    // Run CLI — it should fail with lock timeout (exit code 2)
    let (status, _stdout, stderr) = run_cli(&path, &["stats"]);
    assert!(!status.success(), "should fail when lock is held");

    #[cfg(unix)]
    {
        assert_eq!(
            status.code(),
            Some(2),
            "lock timeout should exit with code 2, stderr: {stderr}"
        );
    }

    assert!(
        stderr.contains("in use") || stderr.contains("lock") || stderr.contains("Try again"),
        "should mention lock/in use, got: {stderr}"
    );

    // Release the lock
    lock_file.unlock().expect("failed to release lock");
}

#[test]
fn test_corrupted_memory_file_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("corrupted.mv2");
    // Write garbage bytes that are not a valid .mv2 file
    std::fs::write(&path, b"THIS IS NOT A VALID MV2 FILE - GARBAGE DATA").unwrap();

    let (status, _stdout, stderr) = run_cli(&path, &["stats"]);
    // The CLI should either handle the corrupted file gracefully or show an error.
    // Mind::open recovers from corruption (creates backup + fresh file), so this
    // should actually succeed after recovery.
    if !status.success() {
        // If it fails, the error should be user-friendly (no stack traces)
        assert!(
            !stderr.contains("panicked") && !stderr.contains("RUST_BACKTRACE"),
            "error should be user-friendly, not a panic: {stderr}"
        );
    }
}
