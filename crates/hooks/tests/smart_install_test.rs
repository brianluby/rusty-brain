mod common;

use hooks::smart_install::handle_smart_install;

#[test]
fn fresh_install_writes_version_file() {
    let dir = tempfile::tempdir().unwrap();
    let input = common::smart_install_input(dir.path().to_str().unwrap());

    let output = handle_smart_install(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));

    let version_path = dir.path().join(".install-version");
    assert!(version_path.exists(), ".install-version should be created");
    let version = std::fs::read_to_string(&version_path).unwrap();
    assert!(
        !version.is_empty(),
        "version file should contain a version string"
    );
}

#[test]
fn matching_version_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let version = env!("CARGO_PKG_VERSION");
    let version_path = dir.path().join(".install-version");
    std::fs::write(&version_path, version).unwrap();
    let modified_before = std::fs::metadata(&version_path)
        .unwrap()
        .modified()
        .unwrap();

    let input = common::smart_install_input(dir.path().to_str().unwrap());
    let output = handle_smart_install(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));

    // File should not have been rewritten
    let modified_after = std::fs::metadata(&version_path)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        modified_before, modified_after,
        "matching version should not rewrite file"
    );
}

#[test]
fn mismatched_version_updates_file() {
    let dir = tempfile::tempdir().unwrap();
    let version_path = dir.path().join(".install-version");
    std::fs::write(&version_path, "0.0.0-old").unwrap();

    let input = common::smart_install_input(dir.path().to_str().unwrap());
    let output = handle_smart_install(&input).unwrap();
    assert_eq!(output.continue_execution, Some(true));

    let version = std::fs::read_to_string(&version_path).unwrap();
    assert_ne!(version, "0.0.0-old", "version should have been updated");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn error_during_io_fails_open() {
    // Use a regular file as cwd — cannot create children under a file (cross-platform)
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("not-a-dir");
    std::fs::write(&file_path, "blocker").unwrap();
    let input = common::smart_install_input(file_path.to_str().unwrap());
    // The handler should return Err; fail-open conversion happens at the I/O boundary
    let result = handle_smart_install(&input);
    assert!(
        result.is_err(),
        "handle_smart_install should return Err for invalid cwd"
    );
}
