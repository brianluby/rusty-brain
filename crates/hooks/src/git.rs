use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Detect files modified in the working directory using `git diff --name-only HEAD`.
///
/// Uses `spawn()` + `try_wait()` with a 5-second timeout to prevent hanging
/// if git is slow or stuck.
///
/// Returns empty `Vec` on any error (git not found, timeout, non-zero exit, non-git directory).
/// Arguments are hardcoded string literals (SEC-9).
pub fn detect_modified_files(cwd: &Path) -> Vec<String> {
    let Ok(mut child) = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };

    // Read stdout in a separate thread to avoid pipe buffer deadlock.
    // If git produces more output than the OS pipe buffer, it blocks on write
    // and never exits — reading concurrently prevents this.
    let stdout_handle = child.stdout.take();
    let reader_thread = std::thread::spawn(move || {
        let Some(mut stdout) = stdout_handle else {
            return String::new();
        };
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });

    // Poll for completion with timeout
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= GIT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Vec::new();
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return Vec::new(),
        }
    };

    if !status.success() {
        return Vec::new();
    }

    let Ok(output) = reader_thread.join() else {
        return Vec::new();
    };

    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // detect_modified_files — in a real git repo
    // -----------------------------------------------------------------------

    #[test]
    fn detect_modified_files_returns_vec_for_git_repo() {
        // Use the project root itself (known git repo)
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("should find workspace root");

        let files = detect_modified_files(project_root);
        // We can't assert specific files, but the function should not panic
        // and should return a Vec<String>
        let _: Vec<String> = files;
    }

    #[test]
    fn detect_modified_files_returns_empty_for_nonexistent_dir() {
        let files = detect_modified_files(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(
            files.is_empty(),
            "should return empty vec for nonexistent directory"
        );
    }

    #[test]
    fn detect_modified_files_returns_empty_for_non_git_dir() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let files = detect_modified_files(tmp.path());
        assert!(
            files.is_empty(),
            "should return empty vec for non-git directory"
        );
    }

    #[test]
    fn detect_modified_files_detects_new_file_in_git_repo() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");

        // Initialize a git repo, create a commit, then modify a file
        let init = std::process::Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output();

        if init.is_err() || !init.unwrap().status.success() {
            // git not available, skip
            return;
        }

        // Configure git user for commit
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output();

        // Create initial file and commit
        std::fs::write(tmp.path().join("file.txt"), "initial").expect("write file");
        let _ = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(tmp.path())
            .output();

        // Modify the file
        std::fs::write(tmp.path().join("file.txt"), "modified").expect("write modified file");

        let files = detect_modified_files(tmp.path());
        assert!(
            files.contains(&"file.txt".to_string()),
            "should detect modified file, got: {files:?}"
        );
    }
}
