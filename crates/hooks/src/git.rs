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
