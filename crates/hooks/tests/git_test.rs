use hooks::git::detect_modified_files;

#[test]
fn returns_files_from_real_git_repo() {
    let dir = tempfile::tempdir().unwrap();
    // Initialize a git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Configure git user for commits
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
    // Create and commit a file
    std::fs::write(dir.path().join("file.txt"), "initial").unwrap();
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
    // Modify the file (unstaged change)
    std::fs::write(dir.path().join("file.txt"), "modified").unwrap();

    let files = detect_modified_files(dir.path());
    assert_eq!(files, vec!["file.txt"]);
}

#[test]
fn returns_empty_for_non_git_directory() {
    let dir = tempfile::tempdir().unwrap();
    let files = detect_modified_files(dir.path());
    assert!(files.is_empty());
}

#[test]
fn returns_empty_when_no_changes() {
    let dir = tempfile::tempdir().unwrap();
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
    std::fs::write(dir.path().join("file.txt"), "initial").unwrap();
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

    let files = detect_modified_files(dir.path());
    assert!(files.is_empty());
}
