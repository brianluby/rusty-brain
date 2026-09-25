//! Bounded git subprocess helpers.
//!
//! Shared by the agent adapters, the hooks, and the daemon: every path that
//! shells out to git goes through the same kill-bounded, fail-open helpers so
//! a hung git can never wedge a hook, a connection handshake, or the runtime
//! (Vikunja #63 moved them here so the daemon can read repo state without a
//! cycle through rb-agents).
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

/// Run `git -C <dir> <args...>` under a wall-clock `timeout` and report its
/// EXIT STATUS only (Vikunja #63). For git's `--quiet` probes (`diff-index
/// --quiet HEAD`) the exit code IS the answer: 0 = clean, non-zero = dirty.
/// Returns `Some(success)` when git ran to completion within the deadline
/// (the status' success flag), or `None` on timeout / spawn / wait failure —
/// the caller fails open on `None`. Same stdio posture as
/// [`run_git_bounded`]. NEVER panics.
#[must_use]
pub fn run_git_status_bounded(dir: &Path, args: &[&str], timeout: Duration) -> Option<bool> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}


