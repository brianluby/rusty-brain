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

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut buf = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read as _;
                    if out.read_to_end(&mut buf).is_err() {
                        return None;
                    }
                }
                return Some(buf);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
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
