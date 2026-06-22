//! End-to-end: start the real binary's daemon on a temp socket+DB, then run
//! `remember` and `recall` through the built binary; assert the content returns.
//! Uses the offline DeterministicProvider (VOYAGE_API_KEY is cleared), so CI
//! never contacts a live embedding API.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use predicates::Predicate;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Owns the spawned daemon process and reaps it on drop (kill + wait), even if a
/// later assertion panics and unwinds the test.
struct Reap(Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Block until `path` accepts connections or the deadline passes.
fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[test]
fn remember_then_recall_round_trips_through_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    let _reap = Reap(
        Command::new(&exe)
            .arg("serve")
            .env("RUSTY_BRAIN_SOCKET", &socket)
            .env("RUSTY_BRAIN_DB", &db)
            // Isolate from any real user config file (C1): empty tempdir => defaults.
            .env("XDG_CONFIG_HOME", dir.path())
            .env_remove("VOYAGE_API_KEY")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );

    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "daemon socket never appeared at {}",
        socket.display()
    );

    let remember = Command::new(&exe)
        .args(["remember", "always use one database and one transaction"])
        .args(["--type", "architecture_decision", "--importance", "9"])
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        // Isolate from any real user config file (C1): empty tempdir => defaults.
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("VOYAGE_API_KEY")
        .output()
        .expect("run remember");
    assert!(
        remember.status.success(),
        "remember failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&remember.stdout),
        String::from_utf8_lossy(&remember.stderr)
    );

    let recall = Command::new(&exe)
        .args(["recall", "one database transaction", "--limit", "10"])
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        // Isolate from any real user config file (C1): empty tempdir => defaults.
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("VOYAGE_API_KEY")
        .output()
        .expect("run recall");
    assert!(
        recall.status.success(),
        "recall failed: stderr={:?}",
        String::from_utf8_lossy(&recall.stderr)
    );
    let stdout = String::from_utf8_lossy(&recall.stdout);
    let found = predicate::str::contains("one database and one transaction");
    assert!(
        found.eval(&stdout),
        "recalled output did not contain the remembered content; got: {stdout}"
    );
}

#[test]
fn remember_batch_bulk_loads_lines_from_stdin() {
    use std::io::Write as _;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    let _reap = Reap(
        Command::new(&exe)
            .arg("serve")
            .env("RUSTY_BRAIN_SOCKET", &socket)
            .env("RUSTY_BRAIN_DB", &db)
            .env("XDG_CONFIG_HOME", dir.path())
            .env_remove("VOYAGE_API_KEY")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );
    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "daemon socket never appeared at {}",
        socket.display()
    );

    // Three facts plus a blank line (which the batch loop must skip), all stored
    // over a SINGLE process invocation / daemon connection.
    let facts = "the cache eviction policy is lru\n\nthe internal metrics port is 9847\nthe retry budget is three attempts\n";
    let mut child = Command::new(&exe)
        .args(["--json", "remember", "--batch", "--importance", "6"])
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("VOYAGE_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn batch remember");
    child
        .stdin
        .take()
        .expect("batch stdin")
        .write_all(facts.as_bytes())
        .expect("write batch facts");
    let batch = child.wait_with_output().expect("wait batch remember");
    assert!(
        batch.status.success(),
        "batch remember failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&batch.stdout),
        String::from_utf8_lossy(&batch.stderr)
    );
    // The blank line is skipped, so exactly three facts are stored.
    let report: serde_json::Value =
        serde_json::from_slice(&batch.stdout).expect("batch --json stdout is an object");
    assert_eq!(
        report["count"].as_u64(),
        Some(3),
        "batch should report 3 stored facts (blank line skipped); got {}",
        String::from_utf8_lossy(&batch.stdout)
    );

    // A fact buried in the batch is retrievable.
    let recall = Command::new(&exe)
        .args(["recall", "internal metrics port", "--limit", "10"])
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("VOYAGE_API_KEY")
        .output()
        .expect("run recall");
    assert!(
        recall.status.success(),
        "recall failed: stderr={:?}",
        String::from_utf8_lossy(&recall.stderr)
    );
    let stdout = String::from_utf8_lossy(&recall.stdout);
    assert!(
        predicate::str::contains("9847").eval(&stdout),
        "recall did not surface a batch-loaded fact; got: {stdout}"
    );
}
