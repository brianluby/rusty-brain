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

fn spawn_daemon(exe: &Path, socket: &Path, db: &Path, config_home: &Path) -> Reap {
    Reap(
        Command::new(exe)
            .arg("serve")
            .env("RUSTY_BRAIN_SOCKET", socket)
            .env("RUSTY_BRAIN_DB", db)
            .env("XDG_CONFIG_HOME", config_home)
            .env_remove("VOYAGE_API_KEY")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    )
}

fn cli(exe: &Path, socket: &Path, db: &Path, config_home: &Path, namespace: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.env("RUSTY_BRAIN_SOCKET", socket)
        .env("RUSTY_BRAIN_DB", db)
        .env("XDG_CONFIG_HOME", config_home)
        .env("RUSTY_BRAIN_NAMESPACE", namespace)
        .env_remove("VOYAGE_API_KEY");
    cmd
}

fn db_family_contains(db: &Path, needle: &[u8]) -> bool {
    let Some(parent) = db.parent() else {
        return false;
    };
    let Some(file_name) = db.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.starts_with(file_name) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.windows(needle.len()).any(|w| w == needle) {
            return true;
        }
    }
    false
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

#[test]
fn init_seeds_deduplicates_redacts_and_undoes_project_context() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join("docs")).unwrap();
    let secret = "AKIAABCDEFGHIJKLMNOP";
    std::fs::write(
        project.join("README.md"),
        format!("# Demo\n\nThe durable storage decision is sqlite vec. Secret {secret}\n"),
    )
    .unwrap();
    std::fs::write(
        project.join("CLAUDE.md"),
        "# Policy\nNever commit secrets.\n",
    )
    .unwrap();
    std::fs::write(
        project.join("docs").join("adr-storage.md"),
        "# Storage ADR\nWe adopted sqlite-vec for local-first memory retrieval.\n",
    )
    .unwrap();
    std::fs::write(
        project.join("docs").join("usage.md"),
        "# Usage\nRun rusty-brain init before the first session.\n",
    )
    .unwrap();
    std::fs::write(
        project.join("docs").join("constraints.md"),
        "# Constraints\nThe daemon remains local-first and per-user.\n",
    )
    .unwrap();

    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let _reap = spawn_daemon(&exe, &socket, &db, dir.path());
    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "daemon socket never appeared at {}",
        socket.display()
    );

    let init = cli(&exe, &socket, &db, dir.path(), "init-e2e")
        .current_dir(&project)
        .args(["--json", "init", "--yes"])
        .output()
        .expect("run init");
    assert!(
        init.status.success(),
        "init failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    let first: serde_json::Value = serde_json::from_slice(&init.stdout).unwrap();
    let batch = first["batch"].as_str().unwrap().to_string();
    assert!(
        first["new"].as_u64().unwrap_or(0) >= 5,
        "init should seed well-known files + docs: {}",
        String::from_utf8_lossy(&init.stdout)
    );
    assert!(
        !db_family_contains(&db, secret.as_bytes()),
        "planted secret must be redacted before reaching sqlite files"
    );

    let recall = cli(&exe, &socket, &db, dir.path(), "init-e2e")
        .current_dir(&project)
        .args(["recall", "durable storage decision", "--limit", "10"])
        .output()
        .expect("run recall");
    assert!(recall.status.success());
    let stdout = String::from_utf8_lossy(&recall.stdout);
    assert!(
        stdout.contains("sqlite vec") || stdout.contains("sqlite-vec"),
        "recall should surface seeded context; got: {stdout}"
    );

    let second = cli(&exe, &socket, &db, dir.path(), "init-e2e")
        .current_dir(&project)
        .args(["--json", "init", "--yes"])
        .output()
        .expect("run init second time");
    assert!(second.status.success());
    let second_report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        second_report["new"].as_u64(),
        Some(0),
        "second init should be idempotent: {}",
        String::from_utf8_lossy(&second.stdout)
    );

    let undo = cli(&exe, &socket, &db, dir.path(), "init-e2e")
        .current_dir(&project)
        .args(["--json", "init", "--undo", &batch])
        .output()
        .expect("undo init batch");
    assert!(
        undo.status.success(),
        "undo failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&undo.stdout),
        String::from_utf8_lossy(&undo.stderr)
    );

    let recall_after_undo = cli(&exe, &socket, &db, dir.path(), "init-e2e")
        .current_dir(&project)
        .args(["recall", "durable storage decision", "--limit", "10"])
        .output()
        .expect("recall after undo");
    assert!(recall_after_undo.status.success());
    let stdout = String::from_utf8_lossy(&recall_after_undo.stdout);
    assert!(
        !stdout.contains("sqlite vec") && !stdout.contains("sqlite-vec"),
        "undo should remove the seeded batch from recall; got: {stdout}"
    );
}

#[test]
fn import_dry_run_prints_plan_and_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let source = project.join("seed.md");
    std::fs::write(
        &source,
        "# Dry Run Seed\nThe dry-run-only sentinel is never stored.\n",
    )
    .unwrap();

    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let _reap = spawn_daemon(&exe, &socket, &db, dir.path());
    assert!(wait_for_socket(&socket, Duration::from_secs(10)));

    let dry_run = cli(&exe, &socket, &db, dir.path(), "import-dry-run-e2e")
        .current_dir(&project)
        .args(["--json", "import", source.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("run import dry-run");
    assert!(
        dry_run.status.success(),
        "dry-run failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&dry_run.stdout),
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(plan["planned"].as_u64(), Some(1));

    let recall = cli(&exe, &socket, &db, dir.path(), "import-dry-run-e2e")
        .current_dir(&project)
        .args(["recall", "dry-run-only sentinel", "--limit", "10"])
        .output()
        .expect("recall dry-run sentinel");
    assert!(recall.status.success());
    let stdout = String::from_utf8_lossy(&recall.stdout);
    assert!(
        stdout.contains("No stored memories match"),
        "dry-run must not store the imported text; got: {stdout}"
    );
}
