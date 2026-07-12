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

// PRD 2026-07-02 search-filter parity: the new recall/list filter flags flow
// through the built binary to the daemon and change which rows come back.
#[test]
fn filter_flags_flow_through_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let config_home = dir.path();

    let _reap = spawn_daemon(&exe, &socket, &db, config_home);
    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "daemon socket never appeared at {}",
        socket.display()
    );

    let run = |args: &[&str]| {
        let out = cli(&exe, &socket, &db, config_home, "filter-e2e")
            .args(args)
            .output()
            .expect("run cli");
        assert!(
            out.status.success(),
            "{args:?} failed: stderr={:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    run(&[
        "remember",
        "the flagship decision content",
        "--importance",
        "9",
        "--tags",
        "core",
    ]);
    let low_out = run(&[
        "--json",
        "remember",
        "the low priority scratch content",
        "--importance",
        "3",
        "--tags",
        "scratch",
    ]);
    // Parse the machine-readable output as JSON (the `--json` contract is
    // `{"id":"<uuid>"}`) instead of scanning for a quoted UUID shape.
    let low_id = serde_json::from_str::<serde_json::Value>(low_out.trim())
        .expect("remember --json must print valid JSON")["id"]
        .as_str()
        .expect("remember --json output must carry a string `id`")
        .to_string();

    // Importance range on list.
    let stdout = run(&["list", "--min-importance", "8"]);
    assert!(stdout.contains("flagship decision"), "got: {stdout}");
    assert!(!stdout.contains("low priority"), "got: {stdout}");

    // Source filter: both rows were written by the CLI surface.
    let stdout = run(&["list", "--source", "cli"]);
    assert!(stdout.contains("flagship decision") && stdout.contains("low priority"));
    let stdout = run(&["list", "--source", "hook"]);
    assert!(
        !stdout.contains("flagship decision") && !stdout.contains("low priority"),
        "no row was written by a hook; got: {stdout}"
    );

    // Date window: an --until far in the past excludes everything.
    let stdout = run(&["list", "--until", "2000-01-01"]);
    assert!(!stdout.contains("content"), "got: {stdout}");

    // Recall accepts the same flags (importance range narrows the hits).
    let stdout = run(&["recall", "content", "--min-importance", "8"]);
    assert!(stdout.contains("flagship decision"), "got: {stdout}");
    assert!(!stdout.contains("low priority"), "got: {stdout}");

    // Composition: source + importance + since.
    let stdout = run(&[
        "list",
        "--source",
        "cli",
        "--min-importance",
        "8",
        "--since",
        "2000-01-01",
    ]);
    assert!(stdout.contains("flagship decision") && !stdout.contains("low priority"));

    // Archived state: delete one, then reach it only via --archived.
    run(&["delete", &low_id]);
    let stdout = run(&["list"]);
    assert!(!stdout.contains("low priority"), "got: {stdout}");
    let stdout = run(&["list", "--archived"]);
    assert!(
        stdout.contains("low priority") && !stdout.contains("flagship decision"),
        "--archived must list ONLY archived rows; got: {stdout}"
    );
    let stdout = run(&["recall", "content", "--archived"]);
    assert!(
        stdout.contains("low priority") && !stdout.contains("flagship decision"),
        "recall --archived must reach archived rows via keyword; got: {stdout}"
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

#[test]
fn export_then_restore_round_trips_and_redacts() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let _reap = spawn_daemon(&exe, &socket, &db, dir.path());
    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "daemon socket never appeared at {}",
        socket.display()
    );

    // Plant a secret in memory content via the import path (which redacts
    // client-side before the wire/store, so the DB has the redacted form).
    let secret = "AKIAABCDEFGHIJKLMNOP";
    let mut import_child = cli(&exe, &socket, &db, dir.path(), "export-e2e")
        .args([
            "import",
            "-",
            "--type",
            "architecture_decision",
            "--importance",
            "9",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn import");
    use std::io::Write as _;
    import_child
        .stdin
        .take()
        .expect("import stdin")
        .write_all(
            format!(
                "# Storage Decision\nThe durable storage decision is sqlite-wal. Key {secret}\n"
            )
            .as_bytes(),
        )
        .expect("write import stdin");
    let import_out = import_child.wait_with_output().expect("wait import");
    assert!(
        import_out.status.success(),
        "import failed: stderr={:?}",
        String::from_utf8_lossy(&import_out.stderr)
    );

    // Export as JSON.
    let export = cli(&exe, &socket, &db, dir.path(), "export-e2e")
        .args(["--json", "export", "--format", "json"])
        .output()
        .expect("run export");
    assert!(
        export.status.success(),
        "export failed: stderr={:?}",
        String::from_utf8_lossy(&export.stderr)
    );
    let export_text = String::from_utf8_lossy(&export.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&export_text).expect("export is JSON");
    assert!(
        parsed["count"].as_u64().unwrap_or(0) >= 1,
        "export should have at least one memory: {export_text}"
    );
    // The planted secret must be redacted at rest, so the export must not
    // contain it.
    assert!(
        !export_text.contains(secret),
        "export leaked secret: {export_text}"
    );

    // Restore (into the same namespace). Dedup should skip all items since
    // the content is already stored.
    let restore = cli(&exe, &socket, &db, dir.path(), "export-e2e")
        .args(["--json", "restore", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn restore");
    let mut child = restore;
    child
        .stdin
        .take()
        .expect("restore stdin")
        .write_all(export_text.as_bytes())
        .expect("write export to restore stdin");
    let restore_out = child.wait_with_output().expect("wait restore");
    assert!(
        restore_out.status.success(),
        "restore failed: stderr={:?}",
        String::from_utf8_lossy(&restore_out.stderr)
    );
    let restore_report: serde_json::Value =
        serde_json::from_slice(&restore_out.stdout).expect("restore --json stdout is an object");
    assert_eq!(
        restore_report["skipped_duplicate"].as_u64(),
        Some(parsed["count"].as_u64().unwrap()),
        "restore should skip all as duplicates (idempotent): {}",
        String::from_utf8_lossy(&restore_out.stdout)
    );

    // Recall still works.
    let recall = cli(&exe, &socket, &db, dir.path(), "export-e2e")
        .args(["recall", "durable storage decision", "--limit", "10"])
        .output()
        .expect("run recall");
    assert!(recall.status.success());
    let stdout = String::from_utf8_lossy(&recall.stdout);
    assert!(
        stdout.contains("sqlite-wal") || stdout.contains("sqlite"),
        "recall should surface the exported/restored memory; got: {stdout}"
    );
    assert!(
        !stdout.contains(secret),
        "recall must not show the unredacted secret: {stdout}"
    );

    // Backup writes a timestamped file under the data dir.
    let backup = cli(&exe, &socket, &db, dir.path(), "export-e2e")
        .args(["--json", "backup"])
        .output()
        .expect("run backup");
    assert!(
        backup.status.success(),
        "backup failed: stderr={:?}",
        String::from_utf8_lossy(&backup.stderr)
    );
    let backup_report: serde_json::Value =
        serde_json::from_slice(&backup.stdout).expect("backup --json stdout is an object");
    let backup_path = backup_report["path"].as_str().expect("backup path in JSON");
    assert!(Path::new(backup_path).exists(), "backup file exists");
    let backup_content = std::fs::read_to_string(backup_path).unwrap();
    assert!(
        !backup_content.contains(secret),
        "backup file must not contain the secret"
    );
}

#[test]
fn stats_and_status_report_value_signals_through_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let _reap = spawn_daemon(&exe, &socket, &db, dir.path());
    assert!(wait_for_socket(&socket, Duration::from_secs(10)));

    // Seed two memories and one helpful-feedback event.
    let remembered = cli(&exe, &socket, &db, dir.path(), "stats-e2e")
        .args(["--json", "remember", "alpha decision"])
        .output()
        .expect("run remember");
    assert!(remembered.status.success());
    let id = serde_json::from_slice::<serde_json::Value>(&remembered.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(cli(&exe, &socket, &db, dir.path(), "stats-e2e")
        .args(["remember", "beta pattern"])
        .output()
        .expect("run remember")
        .status
        .success());
    assert!(cli(&exe, &socket, &db, dir.path(), "stats-e2e")
        .args(["feedback", &id, "--kind", "helpful"])
        .output()
        .expect("run feedback")
        .status
        .success());

    // `stats --json`: the seeded distribution comes back, counts + ids only.
    let stats = cli(&exe, &socket, &db, dir.path(), "stats-e2e")
        .args(["--json", "stats", "--window-days", "7"])
        .output()
        .expect("run stats");
    assert!(
        stats.status.success(),
        "stats failed: stderr={:?}",
        String::from_utf8_lossy(&stats.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(parsed["stats"]["live"].as_u64(), Some(2));
    assert_eq!(parsed["stats"]["window_days"].as_u64(), Some(7));
    assert_eq!(parsed["stats"]["feedback"]["helpful"].as_u64(), Some(1));
    assert_eq!(parsed["stats"]["feedback"]["net"].as_i64(), Some(1));
    assert_eq!(parsed["provider_model"].as_str(), Some("deterministic"));
    assert_eq!(parsed["writer_alive"].as_bool(), Some(true));
    let text = String::from_utf8_lossy(&stats.stdout);
    assert!(
        !text.contains("alpha decision"),
        "stats must never leak memory content: {text}"
    );

    // Extended `status --json` (DOC-1): one health payload.
    let status = cli(&exe, &socket, &db, dir.path(), "stats-e2e")
        .args(["--json", "status"])
        .output()
        .expect("run status");
    assert!(status.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(parsed["ok"].as_bool(), Some(true));
    assert_eq!(parsed["writer_alive"].as_bool(), Some(true));
    assert_eq!(parsed["provider_model"].as_str(), Some("deterministic"));
    assert_eq!(parsed["db"]["file_mode"].as_str(), Some("0600"));
    assert_eq!(parsed["memories"]["live"].as_u64(), Some(2));
    assert_eq!(parsed["namespace"].as_str(), Some("project:stats-e2e"));
}

#[test]
fn doctor_healthy_system_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let _reap = spawn_daemon(&exe, &socket, &db, dir.path());
    assert!(wait_for_socket(&socket, Duration::from_secs(10)));

    let doctor = cli(&exe, &socket, &db, dir.path(), "doctor-e2e")
        .arg("doctor")
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        doctor.status.success(),
        "doctor must exit 0 on a healthy system; stdout={stdout} stderr={:?}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    assert!(stdout.contains("no problems"), "{stdout}");
    // The offline fallback is active in this test env: warned, not failed.
    assert!(stdout.to_lowercase().contains("deterministic"), "{stdout}");
}

#[test]
fn doctor_fails_with_guidance_on_wrong_db_mode() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");
    let _reap = spawn_daemon(&exe, &socket, &db, dir.path());
    assert!(wait_for_socket(&socket, Duration::from_secs(10)));

    // A completed round trip proves the daemon is fully initialized (writer
    // AND read pool opened — each open re-tightens the DB to 0600), so the
    // chmod below cannot race a late open that would undo it.
    assert!(cli(&exe, &socket, &db, dir.path(), "doctor-e2e")
        .args(["remember", "mode probe"])
        .output()
        .expect("run remember")
        .status
        .success());

    // Loosen the DB file mode behind the daemon's back.
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();

    let doctor = cli(&exe, &socket, &db, dir.path(), "doctor-e2e")
        .arg("doctor")
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        !doctor.status.success(),
        "doctor must exit non-zero on a world-readable DB; stdout={stdout}"
    );
    assert!(stdout.contains("db-file-mode"), "{stdout}");
    assert!(stdout.contains("0644"), "observed mode shown: {stdout}");
    assert!(
        stdout.contains(&format!("chmod 600 {}", db.display())),
        "actionable guidance shown: {stdout}"
    );
}

#[test]
fn doctor_fails_with_guidance_on_model_mismatch_against_db_meta() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    // A DB whose meta records a real embedding model, with NO daemon running:
    // serve would fail closed (W0.2), and doctor must diagnose exactly that.
    // (This env has no VOYAGE_API_KEY, so the expected provider is the
    // deterministic fallback — a mismatch with the recorded 'voyage-3'.)
    drop(rb_store::SqliteStore::open_with_model(&db, 8, "voyage-3").unwrap());

    let doctor = cli(&exe, &socket, &db, dir.path(), "doctor-e2e")
        .arg("doctor")
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        !doctor.status.success(),
        "doctor must exit non-zero on a model mismatch; stdout={stdout}"
    );
    assert!(stdout.contains("embedding-model"), "{stdout}");
    assert!(
        stdout.contains("voyage-3"),
        "the recorded model is named: {stdout}"
    );
    assert!(
        stdout.contains("--accept-model-change"),
        "guidance names the opt-in: {stdout}"
    );
}
