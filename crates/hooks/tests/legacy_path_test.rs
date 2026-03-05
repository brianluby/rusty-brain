mod common;

use hooks::bootstrap::{Diagnostic, DiagnosticLevel, detect_legacy_path};

/// T072: Integration test — create temp directory with `.claude/mind.mv2`,
/// run detection logic, verify structured diagnostic in output.
#[test]
fn legacy_only_returns_migration_warning() {
    let dir = tempfile::tempdir().unwrap();

    // Set up legacy path only
    let legacy_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("mind.mv2"), b"fake mv2 data").unwrap();

    let result = detect_legacy_path(dir.path());

    assert!(result.is_some(), "should detect legacy path");
    let diag = result.unwrap();
    assert_eq!(diag.level, DiagnosticLevel::Warning);
    assert!(
        diag.message.contains(".claude/mind.mv2"),
        "diagnostic should reference legacy path: {}",
        diag.message
    );
    assert!(
        diag.message.contains("igrat"),
        "diagnostic should suggest migration: {}",
        diag.message
    );
}

#[test]
fn both_paths_returns_duplicate_warning() {
    let dir = tempfile::tempdir().unwrap();

    // Set up both paths
    let legacy_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("mind.mv2"), b"legacy data").unwrap();

    let canonical_dir = dir.path().join(".agent-brain");
    std::fs::create_dir_all(&canonical_dir).unwrap();
    std::fs::write(canonical_dir.join("mind.mv2"), b"canonical data").unwrap();

    let result = detect_legacy_path(dir.path());

    assert!(result.is_some(), "should detect duplicate paths");
    let diag = result.unwrap();
    assert_eq!(diag.level, DiagnosticLevel::Warning);
    assert!(
        diag.message.contains(".agent-brain/mind.mv2"),
        "diagnostic should reference canonical path: {}",
        diag.message
    );
}

#[test]
fn canonical_only_returns_none() {
    let dir = tempfile::tempdir().unwrap();

    // Set up only canonical path
    let canonical_dir = dir.path().join(".agent-brain");
    std::fs::create_dir_all(&canonical_dir).unwrap();
    std::fs::write(canonical_dir.join("mind.mv2"), b"canonical data").unwrap();

    let result = detect_legacy_path(dir.path());
    assert!(result.is_none(), "no diagnostic for canonical-only setup");
}

#[test]
fn neither_path_returns_none() {
    let dir = tempfile::tempdir().unwrap();

    let result = detect_legacy_path(dir.path());
    assert!(result.is_none(), "no diagnostic when neither path exists");
}

/// Verify diagnostic wiring in session_start output.
#[test]
fn session_start_includes_legacy_diagnostic_in_system_message() {
    let dir = tempfile::tempdir().unwrap();

    // Create legacy path
    let legacy_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("mind.mv2"), b"fake mv2").unwrap();

    let input = common::session_start_input_with_cwd(dir.path().to_str().unwrap());
    let output = hooks::session_start::handle_session_start(&input).unwrap();

    let msg = output
        .system_message
        .expect("session_start should return system_message");
    assert!(
        msg.contains("Warning"),
        "system message should contain warning label: {msg}"
    );
    assert!(
        msg.contains(".claude/mind.mv2"),
        "system message should mention legacy path: {msg}"
    );
}
