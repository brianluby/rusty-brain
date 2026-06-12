//! P4 marquee acceptance test: install the Claude Code hook block, fire a real
//! `PostToolUse` Edit through the built `rusty-brain-hooks` binary, prove the
//! observation reached the in-process daemon, then uninstall and prove the
//! sentinel block is removed. Offline: the daemon uses DeterministicProvider so
//! no embedding API is contacted (VOYAGE_API_KEY is cleared for the child).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use assert_cmd::cargo::cargo_bin;
use rb_daemon::{Daemon, DaemonConfig, JobsConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::Client;
use rb_types::Namespace;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;

/// Owns the in-process daemon: a temp dir, the bound socket path, a shutdown
/// channel, and the run task. Started off a fixed temp dir so the socket path
/// is short enough for the AF_UNIX sun_path limit.
struct RunningDaemon {
    socket: PathBuf,
    db: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    async fn start() -> RunningDaemon {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("memory.db");
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db.clone(),
            read_pool_size: 2,
            jobs_config: JobsConfig::default(),
            request_idle_timeout: None,
            enrich: None,
        };
        let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
        let daemon = Daemon::bind(cfg, embedder).await.unwrap();

        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        let mut ready = false;
        for _ in 0..400 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            ready,
            "daemon socket was not reachable within startup timeout at {}",
            socket.display()
        );

        RunningDaemon {
            socket,
            db,
            shutdown: Some(tx),
            task: Some(task),
            _dir: dir,
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

/// Resolve the `rusty-brain-hooks` binary, building it on demand.
///
/// `rusty-brain-hooks` is owned by the `rb-hooks` package, not `rb-install`, so a
/// focused `cargo test -p rb-install` run neither builds it nor sets
/// `CARGO_BIN_EXE_rusty-brain-hooks`. `assert_cmd::cargo::cargo_bin` would then
/// fall back to a `target/<profile>/rusty-brain-hooks` path that this run never
/// builds, and the spawn below would fail with "No such file". escargot builds
/// the cross-package binary explicitly (a near-instant no-op when it is already
/// built, e.g. in CI's multi-package run) and returns its true artifact path, so
/// the test passes whether run alone (`-p rb-install`) or alongside `rb-hooks`.
fn hooks_bin() -> PathBuf {
    escargot::CargoBuild::new()
        .package("rb-hooks")
        .bin("rusty-brain-hooks")
        .run()
        .expect("build rusty-brain-hooks for the e2e test")
        .path()
        .to_path_buf()
}

/// Read `.claude/settings.json` under `project` as JSON, or `Value::Null` if it
/// does not exist.
fn read_settings(project: &Path) -> serde_json::Value {
    let path = project.join(".claude").join("settings.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

/// True if the settings JSON contains an entry whose serialized form mentions
/// both the `rusty-brain` sentinel and the `rusty-brain-hooks` command, i.e.
/// our injected hook block is present.
fn has_sentinel_block(settings: &serde_json::Value) -> bool {
    let text = settings.to_string();
    text.contains("rusty-brain") && text.contains("rusty-brain-hooks")
}

/// Create a fake `claude` executable inside `dir` and return a `PATH` string
/// that prepends `dir`, so `ClaudeCodeInstaller::detect()` resolves `claude` on
/// `PATH`. Without this, CI — where `claude` is absent — would short-circuit the
/// installer to `NotFound`, write nothing, and the `has_sentinel_block`
/// assertion below would fail (the merge would never genuinely run).
#[cfg(unix)]
fn fake_claude_path(dir: &Path) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let bin_path = dir.join("claude");
    std::fs::write(&bin_path, "#!/bin/sh\necho claude 1.0.0\n").unwrap();
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), existing)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_capture_uninstall_round_trip() {
    // --- fixture project -----------------------------------------------------
    let proj_dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let project = proj_dir.path().to_path_buf();
    // Pin the namespace explicitly (env on the hook invocation below): the
    // fixture dir is not a git repo, and since W0.3 hooks never honor unpinned
    // CLAUDE.md frontmatter, so the env override is the stable identity here.
    let namespace = Namespace::Project("rb-e2e-fixture".to_string());

    // --- in-process daemon ---------------------------------------------------
    let daemon = RunningDaemon::start().await;

    // --- 1) install the Claude Code hook block -------------------------------
    let install_bin = cargo_bin("rusty-brain-install");
    // A fake `claude` on PATH so detect() succeeds and the installer genuinely
    // runs the merge in CI (where `claude` is absent). Without it the installer
    // short-circuits to NotFound, writes nothing, and `has_sentinel_block` below
    // fails. The temp dir is kept alive for the duration of the test.
    #[cfg(unix)]
    let path_dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    #[cfg(unix)]
    let test_path = fake_claude_path(path_dir.path());
    // NOTE (Part Z deviation): the Part Y CLI resolves the project scope from the
    // current working directory (no `--project <dir>` flag exists), so we run the
    // installer with `current_dir(&project)` to target the fixture project — the
    // same scoping the plan's `--project <project>` was intended to express.
    let mut install_cmd = Command::new(&install_bin);
    install_cmd
        .args(["install", "--agents", "claude-code"])
        .current_dir(&project);
    #[cfg(unix)]
    install_cmd.env("PATH", &test_path);
    let install_out = install_cmd
        .output()
        .expect("run rusty-brain-install install");
    assert!(
        install_out.status.success(),
        "install failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&install_out.stdout),
        String::from_utf8_lossy(&install_out.stderr)
    );

    let after_install = read_settings(&project);
    assert!(
        has_sentinel_block(&after_install),
        "settings.json must contain the rusty-brain sentinel hook block after install; got: {after_install}"
    );

    // --- 2) fire a PostToolUse Edit through the built hooks binary ------------
    // Built via escargot (see `hooks_bin`): it is owned by `rb-hooks`, so a
    // focused `cargo test -p rb-install` run would not otherwise build it.
    let hooks_bin = hooks_bin();
    let unique = "rb-e2e marker edit to src/zztest.rs at unique-token-9f3a";
    let event = serde_json::json!({
        "session_id": "rb-e2e-session",
        "transcript_path": "/dev/null",
        "cwd": project.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": project.join("src").join("zztest.rs").to_string_lossy(),
            "old_string": "old body",
            "new_string": unique
        },
        "tool_response": { "success": true }
    })
    .to_string();

    let hook_out = Command::new(&hooks_bin)
        .args(["--agent", "claude-code"])
        .env("RUSTY_BRAIN_SOCKET", &daemon.socket)
        .env("RUSTY_BRAIN_DB", &daemon.db)
        // Explicit namespace (W0.3 rule 1) so capture and the recall below
        // agree on identity without a git repo in the fixture.
        .env("RUSTY_BRAIN_NAMESPACE", "rb-e2e-fixture")
        // Isolate the dedup cache to the fixture tempdir so this test never reads
        // or writes the real ~/.cache and a prior run cannot suppress this edit as
        // a duplicate (mirrors crates/rb-hooks/tests/integration.rs).
        .env("XDG_CACHE_HOME", &project)
        .env_remove("VOYAGE_API_KEY")
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(event.as_bytes())?;
            }
            child.wait_with_output()
        })
        .expect("run rusty-brain-hooks post-tool-use");

    // (a) FAIL-OPEN: the hook binary must always exit 0.
    assert!(
        hook_out.status.success(),
        "rusty-brain-hooks must exit 0 (fail-open); status={:?} stdout={:?} stderr={:?}",
        hook_out.status.code(),
        String::from_utf8_lossy(&hook_out.stdout),
        String::from_utf8_lossy(&hook_out.stderr)
    );
    // The Claude Code adapter always renders a continue:true envelope.
    let stdout = String::from_utf8_lossy(&hook_out.stdout);
    assert!(
        stdout.contains("\"continue\""),
        "hook stdout must be a Claude Code envelope with a continue field; got: {stdout}"
    );

    // (b) the observation reached the daemon: recall finds the captured edit.
    // NOTE (Part Z deviation): the Part W capture flow stores the human-readable
    // summary `"Edited <file_path>"` as the memory content (the `new_string` body
    // is not persisted), so the recallable token is the unique file path
    // `src/zztest.rs` rather than the in-body `unique-token-9f3a` marker. This
    // still proves the PostToolUse Edit observation reached the daemon.
    let mut client = Client::connect(&daemon.socket, namespace.clone())
        .await
        .expect("connect to in-process daemon for recall");
    let mut found = false;
    for _ in 0..40 {
        let results = client
            .recall("Edited zztest.rs marker edit".to_string(), None, vec![], 10)
            .await
            .expect("recall");
        if results
            .iter()
            .any(|r| r.memory.content.contains("zztest.rs"))
        {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        found,
        "the PostToolUse Edit observation must be stored in the daemon and recallable"
    );

    // --- 3) uninstall removes ONLY the sentinel block ------------------------
    let mut uninstall_cmd = Command::new(&install_bin);
    uninstall_cmd
        .args(["uninstall", "--agents", "claude-code"])
        .current_dir(&project);
    #[cfg(unix)]
    uninstall_cmd.env("PATH", &test_path);
    let uninstall_out = uninstall_cmd
        .output()
        .expect("run rusty-brain-install uninstall");
    assert!(
        uninstall_out.status.success(),
        "uninstall failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&uninstall_out.stdout),
        String::from_utf8_lossy(&uninstall_out.stderr)
    );

    let after_uninstall = read_settings(&project);
    assert!(
        !has_sentinel_block(&after_uninstall),
        "the rusty-brain sentinel hook block must be gone after uninstall; got: {after_uninstall}"
    );

    daemon.stop().await;
    drop(proj_dir);
}
