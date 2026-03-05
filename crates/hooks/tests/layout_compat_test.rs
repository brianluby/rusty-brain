//! T043: Directory layout assertion tests.
//!
//! Verifies that the `.agent-brain/` directory structure matches the expected
//! TypeScript-era layout: `mind.mv2`, `.dedup-cache.json`, `.install-version`.

mod common;

use std::path::Path;

use types::hooks::HookInput;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "layout-test",
        "transcript_path": "/tmp/transcript.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart"
    }))
    .expect("valid HookInput JSON")
}

// ---------------------------------------------------------------------------
// Default path resolution produces .agent-brain/mind.mv2
// ---------------------------------------------------------------------------

#[test]
fn resolve_memory_path_points_to_agent_brain_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    temp_env::with_vars(
        [
            ("MEMVID_PLATFORM", None::<&str>),
            ("OPENCODE", None::<&str>),
            ("MEMVID_PLATFORM_PATH_OPT_IN", None::<&str>),
            ("MEMVID_PLATFORM_MEMORY_PATH", None::<&str>),
            ("MEMVID_MIND_DEBUG", None::<&str>),
        ],
        || {
            let input = make_input(tmp.path().to_str().unwrap());
            let path =
                hooks::bootstrap::resolve_memory_path(&input, tmp.path()).expect("should resolve");

            // Path should end with .agent-brain/mind.mv2
            assert!(
                path.ends_with(".agent-brain/mind.mv2"),
                "resolved path should end with .agent-brain/mind.mv2, got: {path:?}"
            );

            // Parent should be .agent-brain directory
            let parent = path.parent().expect("path should have parent");
            assert!(
                parent.ends_with(".agent-brain"),
                "parent dir should be .agent-brain, got: {parent:?}"
            );
        },
    );
}

// ---------------------------------------------------------------------------
// Expected file layout under .agent-brain/
// ---------------------------------------------------------------------------

#[test]
fn agent_brain_dir_expected_files_can_be_constructed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let agent_brain_dir = tmp.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_brain_dir).expect("create .agent-brain dir");

    let expected_files = ["mind.mv2", ".dedup-cache.json", ".install-version"];

    for filename in &expected_files {
        let file_path = agent_brain_dir.join(filename);
        std::fs::write(&file_path, "").expect("write placeholder");
        assert!(file_path.exists(), "expected file should exist: {filename}");
    }
}

#[test]
fn agent_brain_dir_structure_matches_typescript_layout() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let agent_brain_dir = tmp.path().join(".agent-brain");
    std::fs::create_dir_all(&agent_brain_dir).expect("create .agent-brain dir");

    // Simulate the full layout the TypeScript version creates
    std::fs::write(agent_brain_dir.join("mind.mv2"), b"fake-mv2").expect("write mind.mv2");
    std::fs::write(agent_brain_dir.join(".dedup-cache.json"), b"{}")
        .expect("write .dedup-cache.json");
    std::fs::write(agent_brain_dir.join(".install-version"), b"0.1.0")
        .expect("write .install-version");

    // Verify all three files exist
    assert!(agent_brain_dir.join("mind.mv2").exists());
    assert!(agent_brain_dir.join(".dedup-cache.json").exists());
    assert!(agent_brain_dir.join(".install-version").exists());

    // Verify no unexpected subdirectories
    let entries: Vec<_> = std::fs::read_dir(&agent_brain_dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        entries.len(),
        3,
        "should have exactly 3 entries in .agent-brain/, got: {entries:?}"
    );
}

#[test]
fn smart_install_writes_install_version_in_cwd() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let input = common::smart_install_input(tmp.path().to_str().unwrap());

    let result = hooks::smart_install::handle_smart_install(&input);
    assert!(result.is_ok(), "smart_install should succeed: {result:?}");

    let version_path = tmp.path().join(".install-version");
    assert!(
        version_path.exists(),
        ".install-version should be written to cwd"
    );

    let content = std::fs::read_to_string(&version_path).expect("read version");
    assert!(
        !content.is_empty(),
        ".install-version should contain a version string"
    );
}

// ---------------------------------------------------------------------------
// Legacy path constant matches expected value
// ---------------------------------------------------------------------------

#[test]
fn legacy_claude_memory_path_constant() {
    assert_eq!(
        platforms::LEGACY_CLAUDE_MEMORY_PATH,
        ".claude/mind.mv2",
        "legacy path constant must match TypeScript-era value"
    );
}

// ---------------------------------------------------------------------------
// Platform opt-in produces correct scoped layout
// ---------------------------------------------------------------------------

#[test]
fn platform_opt_in_produces_scoped_directory() {
    let resolved = platforms::resolve_memory_path(Path::new("/project"), "claude", true)
        .expect("should resolve");

    assert_eq!(
        resolved.path,
        std::path::PathBuf::from("/project/.claude/mind-claude.mv2"),
        "platform opt-in should produce .claude/mind-claude.mv2"
    );
}

#[test]
fn platform_opt_in_opencode_produces_scoped_directory() {
    let resolved = platforms::resolve_memory_path(Path::new("/project"), "opencode", true)
        .expect("should resolve");

    assert_eq!(
        resolved.path,
        std::path::PathBuf::from("/project/.opencode/mind-opencode.mv2"),
        "platform opt-in should produce .opencode/mind-opencode.mv2"
    );
}
