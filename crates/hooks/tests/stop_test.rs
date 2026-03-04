mod common;

use hooks::stop::handle_stop;

#[test]
fn session_with_git_modifications_stores_summary() {
    let dir = tempfile::tempdir().unwrap();
    // Initialize a git repo with a committed file, then modify it
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let test_file = dir.path().join("test.rs");
    std::fs::write(&test_file, "fn main() {}").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Modify the file to create a diff
    std::fs::write(&test_file, "fn main() { println!(\"hello\"); }").unwrap();

    let input = common::stop_input(dir.path().to_str().unwrap());
    let output = handle_stop(&input).unwrap();

    // Should return a system message with summary
    assert!(
        output.system_message.is_some(),
        "should return a system message with session summary"
    );
}

#[test]
fn each_modified_file_stored_as_separate_observation() {
    let dir = tempfile::tempdir().unwrap();
    // Initialize a git repo with two committed files, then modify both
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("a.rs"), "a").unwrap();
    std::fs::write(dir.path().join("b.rs"), "b").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("a.rs"), "a_modified").unwrap();
    std::fs::write(dir.path().join("b.rs"), "b_modified").unwrap();

    let input = common::stop_input(dir.path().to_str().unwrap());
    let output = handle_stop(&input).unwrap();

    assert!(output.system_message.is_some());
    let msg = output.system_message.unwrap();
    assert!(
        msg.contains("Modified 2 file(s)"),
        "summary should reference file count: {msg}"
    );
    assert!(
        msg.contains("a.rs") && msg.contains("b.rs"),
        "summary should list modified files: {msg}"
    );
}

#[test]
fn session_with_no_changes_stores_summary_noting_no_modifications() {
    let dir = tempfile::tempdir().unwrap();
    // Initialize a git repo with no modifications
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let input = common::stop_input(dir.path().to_str().unwrap());
    let output = handle_stop(&input).unwrap();

    assert!(output.system_message.is_some());
    let msg = output.system_message.unwrap();
    assert!(
        msg.contains("no file modifications"),
        "should note no modifications: {msg}"
    );
}

#[test]
fn git_not_available_returns_empty_file_list_and_stores_summary() {
    let dir = tempfile::tempdir().unwrap();
    // Non-git directory — detect_modified_files returns empty Vec
    let input = common::stop_input(dir.path().to_str().unwrap());
    let output = handle_stop(&input).unwrap();

    // Should still produce a summary (with no file modifications)
    assert!(
        output.system_message.is_some(),
        "should still return summary even without git"
    );
}

#[test]
fn error_during_summary_generation_fails_open() {
    let input = common::stop_input("/dev/null/nonexistent");

    let result = handle_stop(&input);
    // Handler should return Err for invalid paths;
    // fail-open conversion happens at the I/O boundary in main.rs
    assert!(
        result.is_err(),
        "handle_stop should return Err for invalid path"
    );
}
