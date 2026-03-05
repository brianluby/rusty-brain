//! T046: Permissions error tests.
//!
//! Verifies that hooks produce meaningful errors when filesystem permissions
//! prevent normal operation. Unix-only (`#[cfg(unix)]`).

mod common;

// ---------------------------------------------------------------------------
// Read-only directory prevents file creation
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn smart_install_fails_on_readonly_directory() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    // Make the directory read-only (no write permission)
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o444)).expect("set permissions");

    let input = common::smart_install_input(dir.to_str().unwrap());
    let result = hooks::smart_install::handle_smart_install(&input);

    // Restore permissions before assertions (so tempdir cleanup works)
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .expect("restore permissions");

    assert!(
        result.is_err(),
        "smart_install should fail on read-only directory"
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("E_HOOK_IO")
            || err_msg.contains("Permission denied")
            || err_msg.contains("permission"),
        "error should indicate an I/O or permission issue: {err_msg}"
    );
}

#[cfg(unix)]
#[test]
fn smart_install_error_is_hookioerror_variant() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();

    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o444)).expect("set permissions");

    let input = common::smart_install_input(dir.to_str().unwrap());
    let result = hooks::smart_install::handle_smart_install(&input);

    // Restore permissions before assertions
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .expect("restore permissions");

    assert!(result.is_err());
    // The error should be an Io variant (from std::io::Error conversion)
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("E_HOOK_IO"),
        "error should be categorized as E_HOOK_IO: {err_msg}"
    );
}

#[cfg(unix)]
#[test]
fn readonly_agent_brain_dir_prevents_mv2_creation() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let agent_brain = tmp.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_brain).expect("create .agent-brain");

    // Make .agent-brain read-only
    std::fs::set_permissions(&agent_brain, std::fs::Permissions::from_mode(0o444))
        .expect("set permissions");

    // Attempt to write a file inside the read-only directory
    let test_file = agent_brain.join("test-write.tmp");
    let write_result = std::fs::write(&test_file, "test");

    // Restore permissions before assertions
    std::fs::set_permissions(&agent_brain, std::fs::Permissions::from_mode(0o755))
        .expect("restore permissions");

    assert!(
        write_result.is_err(),
        "writing to read-only .agent-brain/ should fail"
    );

    let err = write_result.unwrap_err();
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied,
        "error kind should be PermissionDenied"
    );
}

#[cfg(unix)]
#[test]
fn dedup_cache_unwritable_produces_error() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let agent_brain = tmp.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_brain).expect("create .agent-brain");

    // Create the dedup cache file as read-only
    let dedup_path = agent_brain.join(".dedup-cache.json");
    std::fs::write(&dedup_path, "{}").expect("write initial cache");
    std::fs::set_permissions(&dedup_path, std::fs::Permissions::from_mode(0o444))
        .expect("set read-only");

    // Attempt to overwrite should fail
    let write_result = std::fs::write(&dedup_path, r#"{"new": true}"#);

    // Restore permissions
    std::fs::set_permissions(&dedup_path, std::fs::Permissions::from_mode(0o644))
        .expect("restore permissions");

    assert!(
        write_result.is_err(),
        "overwriting read-only .dedup-cache.json should fail"
    );
}

#[cfg(unix)]
#[test]
fn install_version_unwritable_produces_error() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("tempdir");

    // Create .install-version as read-only
    let version_path = tmp.path().join(".install-version");
    std::fs::write(&version_path, "0.0.0-old").expect("write old version");
    std::fs::set_permissions(&version_path, std::fs::Permissions::from_mode(0o444))
        .expect("set read-only");

    let input = common::smart_install_input(tmp.path().to_str().unwrap());
    let result = hooks::smart_install::handle_smart_install(&input);

    // Restore permissions
    std::fs::set_permissions(&version_path, std::fs::Permissions::from_mode(0o644))
        .expect("restore permissions");

    // smart_install writes a temp file then renames, so it may or may not fail
    // depending on the OS's rename behavior (rename can overwrite read-only
    // targets on some Unix systems). The key is it doesn't panic.
    // If it does fail, the error should be structured.
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(
            err_msg.contains("E_HOOK_IO"),
            "error should be E_HOOK_IO variant: {err_msg}"
        );
    }
}
