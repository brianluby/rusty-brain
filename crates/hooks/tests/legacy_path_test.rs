mod common;

use hooks::bootstrap::{DiagnosticLevel, detect_legacy_paths};

/// Integration test — `.claude/mind.mv2` exists alone → suggests migration to .rusty-brain
#[test]
fn claude_only_suggests_rusty_brain_migration() {
    let dir = tempfile::tempdir().unwrap();

    let legacy_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("mind.mv2"), b"fake mv2 data").unwrap();

    let result = detect_legacy_paths(dir.path());

    assert!(!result.is_empty(), "should detect legacy path");
    let diag = &result[0];
    assert_eq!(diag.level, DiagnosticLevel::Warning);
    assert!(
        diag.message.contains(".claude/mind.mv2"),
        "diagnostic should reference legacy path: {}",
        diag.message
    );
    assert!(
        diag.message.contains(".rusty-brain"),
        "diagnostic should suggest migration to .rusty-brain: {}",
        diag.message
    );
}

/// .agent-brain only (no .rusty-brain) → Info with migration suggestion
#[test]
fn agent_brain_only_suggests_migration() {
    let dir = tempfile::tempdir().unwrap();

    let agent_dir = dir.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("mind.mv2"), b"agent data").unwrap();

    let result = detect_legacy_paths(dir.path());

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].level, DiagnosticLevel::Info);
    assert!(
        result[0].message.contains("mv .agent-brain .rusty-brain"),
        "diagnostic should contain actionable mv command: {}",
        result[0].message
    );
}

/// Both .agent-brain and .rusty-brain exist → Warning about duplicate
#[test]
fn both_agent_brain_and_rusty_brain_warns_duplicate() {
    let dir = tempfile::tempdir().unwrap();

    let agent_dir = dir.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("mind.mv2"), b"agent data").unwrap();

    let rusty_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_dir).unwrap();
    std::fs::write(rusty_dir.join("mind.mv2"), b"rusty data").unwrap();

    let result = detect_legacy_paths(dir.path());

    assert!(!result.is_empty());
    assert!(
        result
            .iter()
            .any(|d| d.level == DiagnosticLevel::Warning && d.message.contains("Duplicate"))
    );
}

/// Both .claude and .agent-brain exist (no .rusty-brain)
#[test]
fn claude_and_agent_brain_both_detected() {
    let dir = tempfile::tempdir().unwrap();

    let legacy_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("mind.mv2"), b"claude data").unwrap();

    let agent_dir = dir.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("mind.mv2"), b"agent data").unwrap();

    let result = detect_legacy_paths(dir.path());

    // Should have diagnostics for both .agent-brain (Info) and .claude (Warning)
    assert!(
        result.len() >= 2,
        "should detect both legacy paths, got {}",
        result.len()
    );
}

/// Only .rusty-brain exists → no diagnostics
#[test]
fn rusty_brain_only_returns_empty() {
    let dir = tempfile::tempdir().unwrap();

    let rusty_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_dir).unwrap();
    std::fs::write(rusty_dir.join("mind.mv2"), b"rusty data").unwrap();

    let result = detect_legacy_paths(dir.path());
    assert!(
        result.is_empty(),
        "no diagnostic for rusty-brain-only setup"
    );
}

/// Neither path exists → no diagnostics
#[test]
fn neither_path_returns_empty() {
    let dir = tempfile::tempdir().unwrap();

    let result = detect_legacy_paths(dir.path());
    assert!(result.is_empty(), "no diagnostic when neither path exists");
}

/// Verify diagnostic wiring in session_start output.
#[test]
fn session_start_includes_legacy_diagnostic_in_system_message() {
    let dir = tempfile::tempdir().unwrap();

    // Create legacy .claude path
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
        msg.contains(".rusty-brain"),
        "system message should mention .rusty-brain migration target: {msg}"
    );
}
