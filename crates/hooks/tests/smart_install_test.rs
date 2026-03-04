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
    // Use a path that doesn't exist and can't be created (e.g., under /dev/null)
    let input = common::smart_install_input("/dev/null/nonexistent");
    // The handler should not panic
    let result = handle_smart_install(&input);
    // Either Ok with continue:true or Err that will be fail-opened by the I/O layer
    match result {
        Ok(output) => assert_eq!(output.continue_execution, Some(true)),
        Err(_) => {} // Expected — the I/O layer will fail-open this
    }
}
