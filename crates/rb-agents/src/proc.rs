//! Bounded git subprocess helper.
//!
//! Detection paths shell out to `git`; a hung or runaway git must never block,
//! hang, or otherwise wedge the agent hook. [`run_git_bounded`] spawns `git`
//! with no stdin and a discarded stderr, polls for completion against a
//! wall-clock deadline, and on timeout (or any spawn/wait error) kills+reaps the
//! child and fails open (returns `None`). NEVER panics.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to sleep between non-blocking `try_wait` polls.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Run `git -C <dir> <args...>` under a wall-clock `timeout`, fail-open.
///
/// Returns `Some(stdout_bytes)` only on a clean, successful exit within the
/// deadline. On timeout, spawn failure, wait failure, or a non-success exit
/// status, the child is killed + reaped (best-effort) and `None` is returned.
/// `stdin` is `/dev/null` and `stderr` is discarded so git can never block on
/// input or pollute the hook channel. NEVER panics.
#[must_use]
pub fn run_git_bounded(dir: &Path, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return None,
    };

    // Drain stdout CONCURRENTLY with the wait loop. If we waited for exit before
    // reading, a large `git` stdout (more than the OS pipe buffer) would block
    // git's write while we block on exit — a deadlock that only resolves when the
    // timeout kills the child, so large outputs would never succeed. Moving the
    // pipe into a reader thread lets git keep writing while we poll for exit.
    let reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = Vec::new();
            // On read error, return whatever we have; the caller decides via the
            // exit status whether to trust it.
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });

    // Join the reader thread, returning its captured bytes. A panicked reader
    // thread (it never panics by construction) degrades to `None`.
    let join_reader = |reader: Option<std::thread::JoinHandle<Vec<u8>>>| -> Option<Vec<u8>> {
        match reader {
            Some(handle) => handle.join().ok(),
            None => Some(Vec::new()),
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let _ = join_reader(reader);
                    return None;
                }
                // Clean exit: the writer is done, so the reader thread will see
                // EOF and finish. Join it to collect the full output.
                return join_reader(reader);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Closing the child's stdout (on kill/reap) lets the reader
                    // hit EOF; join it so the thread never leaks.
                    let _ = join_reader(reader);
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_reader(reader);
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    /// Skip a test body if `git` is not available on PATH.
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn real_repo_returns_toplevel_bytes() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(tmp.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        };
        if !run(&["init"]).map(|s| s.success()).unwrap_or(false) {
            return; // git unavailable for init; skip
        }
        let out = run_git_bounded(
            tmp.path(),
            &["rev-parse", "--show-toplevel"],
            Duration::from_secs(5),
        );
        let bytes = out.expect("a real repo must return Some(toplevel bytes)");
        let text = String::from_utf8(bytes).expect("toplevel path is utf8");
        assert!(
            !text.trim().is_empty(),
            "toplevel output should be non-empty, got {text:?}"
        );
    }

    #[test]
    fn non_repo_dir_returns_none() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = run_git_bounded(
            tmp.path(),
            &["rev-parse", "--show-toplevel"],
            Duration::from_secs(5),
        );
        assert!(out.is_none(), "a non-repo dir must return None");
    }

    #[test]
    fn large_stdout_is_fully_captured_without_deadlock() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(tmp.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        };
        if !run(&["init"]).map(|s| s.success()).unwrap_or(false) {
            return; // git unavailable for init; skip
        }

        // Stage enough files that `git diff --cached --name-only` produces well
        // over the OS pipe buffer (commonly 64 KiB). Each filename is padded so a
        // few thousand entries comfortably exceed the buffer; if we did not drain
        // stdout concurrently, git would block on write and this would deadlock
        // until the timeout, returning None instead of the full listing.
        const COUNT: usize = 4000;
        for i in 0..COUNT {
            // ~40 bytes/name → ~160 KiB of `--name-only` output.
            let name = format!("file-{i:020}-padding-xxxxxxxx.txt");
            std::fs::write(tmp.path().join(&name), b"x").unwrap();
        }
        assert!(
            run(&["add", "-A"]).map(|s| s.success()).unwrap_or(false),
            "git add must succeed"
        );

        let out = run_git_bounded(
            tmp.path(),
            &["diff", "--cached", "--name-only"],
            Duration::from_secs(30),
        );
        let bytes = out.expect("large diff must be fully captured, not deadlocked");
        assert!(
            bytes.len() > 64 * 1024,
            "output should exceed the pipe buffer, got {} bytes",
            bytes.len()
        );
        let text = String::from_utf8(bytes).expect("name-only output is utf8");
        let lines = text.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(
            lines, COUNT,
            "every staged file must appear in the captured output"
        );
    }

    #[test]
    fn bogus_arg_returns_none_without_panic() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = run_git_bounded(
            tmp.path(),
            &["this-is-not-a-git-subcommand-xyz"],
            Duration::from_secs(5),
        );
        assert!(out.is_none(), "a bogus git arg must return None (no panic)");
    }
}
