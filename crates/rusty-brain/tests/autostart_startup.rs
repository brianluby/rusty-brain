//! Exercise real CLI and MCP autostart against a populated model-conflict corpus.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::cargo::cargo_bin;
use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};

struct DaemonCleanup(PathBuf);

impl Drop for DaemonCleanup {
    fn drop(&mut self) {
        let pidfile = self.0.with_extension("pid");
        if let Ok(pid) = std::fs::read_to_string(&pidfile) {
            let _ = Command::new("kill").args(["-9", pid.trim()]).status();
        }
    }
}

struct ServeCleanup(Child);

impl Drop for ServeCleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_before_deadline(mut command: Command, timeout: Duration) -> (Output, Duration) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child: Child = command.spawn().expect("spawn CLI");
    let deadline = started + timeout;
    loop {
        match child.try_wait().expect("poll CLI") {
            Some(_) => {
                return (
                    child.wait_with_output().expect("collect CLI output"),
                    started.elapsed(),
                )
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("CLI exceeded startup test deadline of {timeout:?}");
            }
        }
    }
}

fn cli(executable: &Path, socket: &Path, database: &Path, config: &Path, cache: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .env("RUSTY_BRAIN_SOCKET", socket)
        .env("RUSTY_BRAIN_DB", database)
        .env("XDG_CONFIG_HOME", config)
        .env("XDG_CACHE_HOME", cache)
        .env("RUST_LOG", "warn")
        .env_remove("RUSTY_BRAIN_IDLE_TIMEOUT_SECS")
        .env_remove("RB_EMBED_BACKEND")
        .env_remove("RB_LOCAL_MODEL")
        .env_remove("RB_ACCEPT_MODEL_CHANGE")
        .env_remove("VOYAGE_API_KEY");
    command
}

fn model_conflict_database(database: &Path) -> rb_types::MemoryId {
    let store = SqliteStore::open_with_model(database, 512, "fixture-other-model").unwrap();
    let mut note = MemoryNote::new(
        Namespace::Project("startup-fixture".into()),
        "A populated model-conflict startup fixture.".into(),
        MemoryType::Insight,
        6,
    );
    note.embedding_model = "fixture-other-model".into();
    note.embedding_input_version = "v2-composite".into();
    store.insert_memory(&note, Some(&vec![0.125; 512])).unwrap();
    drop(store);
    note.id
}

#[test]
fn cli_and_mcp_report_real_model_conflict_before_the_startup_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("memory.db");
    model_conflict_database(&database);

    let executable = cargo_bin("rusty-brain");
    let config = directory.path().join("config");
    let cache = directory.path().join("cache");
    for (surface, arguments) in [("CLI", vec!["status"]), ("MCP", vec!["mcp"])] {
        let socket = directory
            .path()
            .join("runtime")
            .join(format!("{surface}.sock"));
        let _cleanup = DaemonCleanup(socket.clone());
        let (output, elapsed) = run_before_deadline(
            {
                let mut command = cli(&executable, &socket, &database, &config, &cache);
                command.args(arguments);
                command
            },
            Duration::from_secs(7),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{surface} unexpectedly succeeded");
        assert!(
            elapsed < Duration::from_secs(6),
            "{surface} took {elapsed:?}"
        );
        assert!(
            stderr.contains("fixture-other-model"),
            "{surface} did not surface the child model/config cause: {stderr}"
        );
        assert!(stderr.contains("--accept-model-change"), "{stderr}");
        assert!(stderr.contains("startup stderr log"), "{stderr}");
        assert!(stderr.contains("redacted stderr tail"), "{stderr}");
        assert!(
            output.stdout.is_empty(),
            "{surface} emitted stdout on startup failure"
        );
    }
}

#[test]
fn cli_accept_model_change_preserves_the_corpus_and_reembeds_it() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("memory.db");
    let id = model_conflict_database(&database);
    let socket = directory.path().join("runtime").join("daemon.sock");
    let executable = cargo_bin("rusty-brain");
    let config = directory.path().join("config");
    let cache = directory.path().join("cache");
    let mut command = cli(&executable, &socket, &database, &config, &cache);
    command
        .args(["serve", "--accept-model-change"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let daemon = ServeCleanup(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(7);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "accepting daemon did not bind");
        thread::sleep(Duration::from_millis(10));
    }
    let run = |arguments: &[&str]| {
        let mut command = cli(&executable, &socket, &database, &config, &cache);
        command
            .env("RUSTY_BRAIN_NAMESPACE", "startup-fixture")
            .args(arguments);
        let (output, _) = run_before_deadline(command, Duration::from_secs(7));
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let status = run(&["--json", "status"]);
    assert_eq!(status["provider_model"], "deterministic");
    assert_eq!(status["db"]["embedding_model"], "deterministic");
    assert_eq!(status["memories"]["live"], 1);
    assert_eq!(status["vectors"], 0);
    let reembedded = run(&["--json", "reembed", "--limit", "10"]);
    assert_eq!(reembedded["changed"], 1);
    assert_eq!(reembedded["skipped"], 0);
    assert_eq!(run(&["--json", "status"])["vectors"], 1);
    drop(daemon);

    let reopened = SqliteStore::open_with_model(&database, 512, "deterministic").unwrap();
    let memory = reopened.get_memory(&id).unwrap().unwrap();
    assert_eq!(
        memory.content,
        "A populated model-conflict startup fixture."
    );
    assert_eq!(memory.embedding_model, "deterministic");
    assert_eq!(memory.embedding_input_version, "v2-composite");
}

#[test]
fn nine_overlapping_cli_starts_preserve_each_attempts_model_conflict() {
    let directory = tempfile::tempdir().unwrap();
    let executable = cargo_bin("rusty-brain");
    let config = directory.path().join("config");
    let cache = directory.path().join("cache");
    let mut cleanups = Vec::new();
    let commands: Vec<_> = (0..9)
        .map(|index| {
            let database = directory.path().join(format!("memory-{index}.db"));
            model_conflict_database(&database);
            let socket = directory
                .path()
                .join("runtime")
                .join(format!("daemon-{index}.sock"));
            cleanups.push(DaemonCleanup(socket.clone()));
            let mut command = cli(&executable, &socket, &database, &config, &cache);
            command.arg("status");
            command
        })
        .collect();
    let barrier = Barrier::new(commands.len());
    thread::scope(|scope| {
        let attempts: Vec<_> = commands
            .into_iter()
            .map(|command| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    run_before_deadline(command, Duration::from_secs(7))
                })
            })
            .collect();
        for (index, attempt) in attempts.into_iter().enumerate() {
            let (output, elapsed) = attempt.join().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success(),
                "attempt {index} unexpectedly succeeded"
            );
            assert!(
                elapsed < Duration::from_secs(6),
                "attempt {index} took {elapsed:?}"
            );
            assert!(
                stderr.contains("fixture-other-model"),
                "attempt {index} lost its actual startup cause: {stderr}"
            );
            assert!(stderr.contains("redacted stderr tail"), "{stderr}");
            assert!(output.stdout.is_empty(), "attempt {index} emitted stdout");
        }
    });
}

#[test]
fn autostarted_daemon_remains_usable_after_the_starting_cli_exits() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("runtime").join("daemon.sock");
    let database = directory.path().join("memory.db");
    let config = directory.path().join("config");
    let cache = directory.path().join("cache");
    let cleanup = DaemonCleanup(socket.clone());
    let executable = cargo_bin("rusty-brain");

    let (first, _) = run_before_deadline(
        {
            let mut command = cli(&executable, &socket, &database, &config, &cache);
            command.arg("status");
            command
        },
        Duration::from_secs(10),
    );
    assert!(
        first.status.success(),
        "first status failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let (second, _) = run_before_deadline(
        {
            let mut command = cli(&executable, &socket, &database, &config, &cache);
            command.arg("status");
            command
        },
        Duration::from_secs(5),
    );
    assert!(
        second.status.success(),
        "detached daemon did not survive: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    drop(cleanup);
}

#[cfg(unix)]
#[test]
fn internal_collector_keeps_the_bounded_true_tail_in_a_private_file() {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};

    const LIMIT: usize = 64 * 1024;
    let directory = tempfile::tempdir().unwrap();
    let logs = directory.path().join("rusty-brain").join("logs");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(&logs).unwrap();
    let log = logs.join("startup-integration.log");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    drop(options.open(&log).unwrap());

    let executable = cargo_bin("rusty-brain");
    let mut collector = Command::new(executable)
        .arg("--internal-startup-stderr-collector")
        .arg(&log)
        .arg("120000")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = collector.stdin.take().unwrap();
    thread::sleep(Duration::from_secs(6));
    let mut bytes = vec![b'x'; LIMIT + 16];
    bytes[..5].copy_from_slice(b"FIRST");
    bytes[15] = b'\n';
    let marker = b"LAST-END";
    let end = bytes.len();
    bytes[end - marker.len()..].copy_from_slice(marker);
    input.write_all(&bytes).unwrap();
    drop(input);
    assert!(collector.wait().unwrap().success());

    let stored = std::fs::read(&log).unwrap();
    assert_eq!(stored.len(), LIMIT);
    assert!(!stored.windows(5).any(|window| window == b"FIRST"));
    assert!(stored.ends_with(marker));
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
