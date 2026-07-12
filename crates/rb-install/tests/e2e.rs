//! P4 marquee acceptance test: install the Claude Code hook block, fire a real
//! `PostToolUse` Edit through the built `rusty-brain-hooks` binary, prove the
//! observation reached the in-process daemon, then uninstall and prove the
//! sentinel block is removed. Offline: the daemon uses DeterministicProvider so
//! no embedding API is contacted (VOYAGE_API_KEY is cleared for the child).
//!
//! Also home to the C4 planted-secret DB-grep test: the per-PR proof that a
//! secret planted through the real hook path is absent from the raw DB FILE
//! bytes (not just the wire payload) — see
//! `planted_secrets_never_reach_the_db_file_bytes`.
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
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_daemon::FusionMode::Linear,
            http: None,
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

    /// Shut the daemon down and hand back the temp-dir guard so callers can
    /// inspect on-disk artifacts (DB/WAL bytes) after the close; dropping the
    /// returned guard deletes the directory.
    async fn stop(mut self) -> tempfile::TempDir {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
        self._dir
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

/// Fire one hook event through the built `rusty-brain-hooks` binary against the
/// running daemon, with the dedup + scratch caches isolated to `project`
/// (`XDG_CACHE_HOME`) and the namespace pinned. Asserts fail-open exit 0 and
/// returns the child output.
fn fire_hook(
    hooks_bin: &Path,
    daemon: &RunningDaemon,
    namespace: &str,
    project: &Path,
    event: &str,
) -> std::process::Output {
    let out = Command::new(hooks_bin)
        .args(["--agent", "claude-code"])
        .env("RUSTY_BRAIN_SOCKET", &daemon.socket)
        .env("RUSTY_BRAIN_DB", &daemon.db)
        // Explicit namespace (W0.3 rule 1) so capture and recall agree on
        // identity without a git repo in the fixture.
        .env("RUSTY_BRAIN_NAMESPACE", namespace)
        // Isolate the dedup + scratch caches to the fixture tempdir so this run
        // never touches the real ~/.cache and a prior run cannot interfere.
        .env("XDG_CACHE_HOME", project)
        .env_remove("VOYAGE_API_KEY")
        .current_dir(project)
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
        .expect("run rusty-brain-hooks");
    assert!(
        out.status.success(),
        "rusty-brain-hooks must exit 0 (fail-open); stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// A Claude Code `SessionEnd` event JSON for `session_id` in `project` — the
/// W3.1 capture point that folds the per-session scratch into one summary.
fn session_end_event(project: &Path, session_id: &str) -> String {
    serde_json::json!({
        "session_id": session_id,
        // No transcript here (the fold draws on the scratch + working tree).
        "transcript_path": "/dev/null",
        "cwd": project.to_string_lossy(),
        "hook_event_name": "SessionEnd",
        "reason": "other",
    })
    .to_string()
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
    // W3.2(c): the MCP permission allowlist entry is written so headless
    // model-initiated remember/recall never stall on an approval prompt (S1).
    let allow_has_mcp = |s: &serde_json::Value| {
        s.get("permissions")
            .and_then(|p| p.get("allow"))
            .and_then(|a| a.as_array())
            .is_some_and(|a| {
                a.iter()
                    .any(|e| e.as_str() == Some("mcp__rusty-brain__remember"))
            })
    };
    assert!(
        allow_has_mcp(&after_install),
        "settings.json must allowlist the rusty-brain MCP tools after install; got: {after_install}"
    );
    // W3.2(b): the memory-policy block + skill are written on install.
    let claude_md = project.join("CLAUDE.md");
    let skill = project
        .join(".claude")
        .join("skills")
        .join("rusty-brain-memory")
        .join("SKILL.md");
    assert!(
        std::fs::read_to_string(&claude_md)
            .unwrap_or_default()
            .contains("BEGIN rusty-brain:memory-policy"),
        "CLAUDE.md must carry the memory-policy block after install"
    );
    assert!(
        skill.exists(),
        "the memory skill must be written after install"
    );

    // --- 2) fire a PostToolUse Edit, then SessionEnd, through the hooks binary -
    // W3.1 capture inversion: PostToolUse appends the touched file to the
    // per-session scratch (ZERO memories); SessionEnd folds the scratch into ONE
    // recallable summary. Both fire through the REAL binary against the daemon.
    // The hooks binary is built via escargot (see `hooks_bin`): it is owned by
    // `rb-hooks`, so a focused `cargo test -p rb-install` run would not build it.
    let hooks_bin = hooks_bin();
    let session_id = "rb-e2e-session";
    let zztest = project.join("src").join("zztest.rs");
    let post_tool_use = serde_json::json!({
        "session_id": session_id,
        "transcript_path": "/dev/null",
        "cwd": project.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": {
            "file_path": zztest.to_string_lossy(),
            "old_string": "old body",
            "new_string": "rb-e2e marker edit"
        },
        "tool_response": { "success": true }
    })
    .to_string();
    let hook_out = fire_hook(
        &hooks_bin,
        &daemon,
        "rb-e2e-fixture",
        &project,
        &post_tool_use,
    );
    // (a) The Claude Code adapter always renders a continue:true envelope.
    let stdout = String::from_utf8_lossy(&hook_out.stdout);
    assert!(
        stdout.contains("\"continue\""),
        "hook stdout must be a Claude Code envelope with a continue field; got: {stdout}"
    );

    // SessionEnd folds the per-session scratch into ONE summary memory.
    fire_hook(
        &hooks_bin,
        &daemon,
        "rb-e2e-fixture",
        &project,
        &session_end_event(&project, session_id),
    );

    // (b) the folded summary reached the daemon and is recallable: it lists the
    // touched file, so the recallable token is the `zztest.rs` path.
    let mut client = Client::connect(&daemon.socket, namespace.clone())
        .await
        .expect("connect to in-process daemon for recall");
    let mut found = false;
    for _ in 0..40 {
        let results = client
            .recall(
                "session summary files touched zztest".to_string(),
                None,
                vec![],
                10,
            )
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
        "the SessionEnd summary must fold the PostToolUse file edit and be recallable"
    );

    // --- 2.5) W3.2(a): a UserPromptSubmit deterministically recalls + injects -
    // Fire a UserPromptSubmit whose prompt matches the stored summary through the
    // REAL binary against the daemon, and prove the hook injects the relevant
    // memory as `hookSpecificOutput.additionalContext` tagged "UserPromptSubmit"
    // — recall reaching the model WITHOUT it electing to call a tool.
    let user_prompt = serde_json::json!({
        "session_id": session_id,
        "transcript_path": "/dev/null",
        "cwd": project.to_string_lossy(),
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "session summary files touched zztest"
    })
    .to_string();
    let prompt_out = fire_hook(
        &hooks_bin,
        &daemon,
        "rb-e2e-fixture",
        &project,
        &user_prompt,
    );
    let prompt_stdout = String::from_utf8_lossy(&prompt_out.stdout);
    let envelope: serde_json::Value =
        serde_json::from_str(&prompt_stdout).expect("UserPromptSubmit hook stdout is JSON");
    assert_eq!(
        envelope["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit",
        "W3.2(a): the injection must be tagged with the firing event; got {prompt_stdout}"
    );
    let injected = envelope["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(
        injected.contains("zztest.rs"),
        "W3.2(a): the prompt-relevant memory must be injected as additionalContext; got {injected}"
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
    // W3.2(c): uninstall also removes our MCP permission allowlist entry.
    assert!(
        !allow_has_mcp(&after_uninstall),
        "the MCP permission allowlist entry must be gone after uninstall; got: {after_uninstall}"
    );
    // W3.2(b): the policy block is stripped and the skill file removed.
    assert!(
        !std::fs::read_to_string(&claude_md)
            .unwrap_or_default()
            .contains("rusty-brain:memory-policy"),
        "the CLAUDE.md policy block must be gone after uninstall"
    );
    assert!(
        !skill.exists(),
        "the memory skill must be removed after uninstall"
    );

    daemon.stop().await;
    drop(proj_dir);
}

/// True if `needle` occurs as a contiguous byte substring of `haystack`.
/// SQLite stores small TEXT payloads contiguously in cell bodies (and WAL
/// frames are raw page images), so a byte-level grep is a faithful
/// plaintext-at-rest check for short strings like the planted secrets below.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// C4 / gate tier (b) per-PR complement: the planted-secret proof at the DB
/// FILE, not the wire. `rb-hooks/tests/integration.rs` already asserts the
/// Remember payload is redacted in flight; this test drives the same secret
/// shapes through the REAL `rusty-brain-hooks` binary into a REAL daemon
/// writing a REAL SQLite file, then greps the raw db/WAL/SHM bytes at rest:
/// zero plaintext hits, redaction markers present. A unique NON-secret
/// sentinel in the same captured command must be recallable AND present in
/// the DB bytes, so the zero-hit greps can never pass vacuously (e.g. because
/// the capture silently failed to land).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planted_secrets_never_reach_the_db_file_bytes() {
    let proj_dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let project = proj_dir.path().to_path_buf();
    let namespace = Namespace::Project("rb-c4-db-grep".to_string());

    let daemon = RunningDaemon::start().await;
    let hooks_bin = hooks_bin();

    // The same planted shapes rb-hooks' wire-level test uses (every rule of
    // the W0.5 minimal redaction set), plus the non-secret sentinel.
    const SENTINEL: &str = "db-grep-sentinel-7c4f";
    const PLANTED: [&str; 5] = [
        "hunter2",
        "AKIAABCDEFGHIJKLMNOP",
        "sk-live-deadbeef",
        "ghp_secret123",
        "MIIfakekeymaterial",
    ];
    // W3.1 capture inversion: PostToolUse appends the (redacted) command to the
    // scratch; a FAILING tool response additionally records its (redacted)
    // stderr. SessionEnd folds both into the stored summary. Planting secrets in
    // BOTH the command and the failure stderr exercises every W0.5 minimal
    // redaction rule through the real capture path; the non-secret SENTINEL rides
    // in the command so the DB greps below can never pass vacuously.
    let post_tool_use = serde_json::json!({
        "session_id": "rb-c4-db-grep",
        "transcript_path": "/dev/null",
        "cwd": project.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": format!("deploy --password={} && echo {SENTINEL}", PLANTED[0]),
        },
        // A FAILING response: its stderr is captured (redacted) as a failure.
        "tool_response": {
            "is_error": true,
            "stderr": format!(
                "key {}\nAuthorization: Bearer {}\nGITHUB_TOKEN={}\n\
                 -----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----\ndone",
                PLANTED[1], PLANTED[2], PLANTED[3], PLANTED[4]
            ),
        },
    })
    .to_string();
    fire_hook(
        &hooks_bin,
        &daemon,
        "rb-c4-db-grep",
        &project,
        &post_tool_use,
    );
    // SessionEnd folds the scratch (redacted command + failure) into the summary.
    fire_hook(
        &hooks_bin,
        &daemon,
        "rb-c4-db-grep",
        &project,
        &session_end_event(&project, "rb-c4-db-grep"),
    );

    // Vacuousness guard: the capture must actually land before we assert
    // anything is ABSENT from the DB.
    let mut client = Client::connect(&daemon.socket, namespace)
        .await
        .expect("connect to in-process daemon for recall");
    let mut found = false;
    for _ in 0..80 {
        let results = client
            .recall(
                "session summary commands deploy sentinel".to_string(),
                None,
                vec![],
                10,
            )
            .await
            .expect("recall");
        if results.iter().any(|r| r.memory.content.contains(SENTINEL)) {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        found,
        "the planted-secret capture must be folded into the SessionEnd summary and recallable (sentinel {SENTINEL})"
    );

    // Stop the daemon FIRST: closing the last SQLite connection checkpoints
    // the WAL into the main file, so the at-rest grep sees the final state.
    // Any straggler -wal/-shm files are grepped too. `stop` hands back the
    // temp-dir guard so the DB files outlive the daemon for the read below.
    let db_path = daemon.db.clone();
    let db_dir = daemon.stop().await;

    let base = db_path.to_string_lossy().to_string();
    let mut at_rest: Vec<u8> = Vec::new();
    let mut read_files: Vec<String> = Vec::new();
    for path in [base.clone(), format!("{base}-wal"), format!("{base}-shm")] {
        if let Ok(bytes) = std::fs::read(&path) {
            read_files.push(path);
            at_rest.extend_from_slice(&bytes);
            // Separator so a needle can never falsely match across files.
            at_rest.push(0);
        }
    }
    assert!(
        read_files.contains(&base),
        "the main DB file must exist and be readable at {base}"
    );

    assert!(
        contains_bytes(&at_rest, SENTINEL.as_bytes()),
        "the non-secret sentinel must be present in the DB bytes (files: {read_files:?})"
    );
    assert!(
        contains_bytes(&at_rest, b"[REDACTED:"),
        "redaction markers must be present in the DB bytes (files: {read_files:?})"
    );
    for secret in PLANTED {
        assert!(
            !contains_bytes(&at_rest, secret.as_bytes()),
            "planted secret {secret:?} found in PLAINTEXT in the DB bytes (files: {read_files:?})"
        );
    }

    drop(db_dir);
    drop(proj_dir);
}

/// Phase-3 gate (W3.1): a 40-turn simulated session must produce ≤ 5 memories
/// (vs ~40 under the pre-inversion per-event capture). Drives 40 PostToolUse
/// mutations + one SessionEnd through the REAL hooks binary into a REAL daemon,
/// then counts the live memories: exactly one session summary, well under the
/// gate's ceiling.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forty_turn_session_produces_at_most_five_memories() {
    let proj_dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
    let project = proj_dir.path().to_path_buf();
    let namespace = Namespace::Project("rb-gate-40turn".to_string());

    let daemon = RunningDaemon::start().await;
    let hooks_bin = hooks_bin();
    let session_id = "rb-gate-40turn-session";

    // 40 turns of distinct mutations (Edit / Write / Bash), the kind that under
    // the OLD capture flow would each have written their own memory.
    for turn in 0..40u32 {
        let event = match turn % 3 {
            0 => serde_json::json!({
                "session_id": session_id,
                "transcript_path": "/dev/null",
                "cwd": project.to_string_lossy(),
                "hook_event_name": "PostToolUse",
                "tool_name": "Edit",
                "tool_input": { "file_path": format!("src/mod_{turn}.rs") },
                "tool_response": { "success": true }
            }),
            1 => serde_json::json!({
                "session_id": session_id,
                "transcript_path": "/dev/null",
                "cwd": project.to_string_lossy(),
                "hook_event_name": "PostToolUse",
                "tool_name": "Write",
                "tool_input": { "file_path": format!("docs/note_{turn}.md") },
                "tool_response": { "success": true }
            }),
            _ => serde_json::json!({
                "session_id": session_id,
                "transcript_path": "/dev/null",
                "cwd": project.to_string_lossy(),
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_input": { "command": format!("cargo test turn_{turn}") },
                "tool_response": { "success": true }
            }),
        }
        .to_string();
        fire_hook(&hooks_bin, &daemon, "rb-gate-40turn", &project, &event);
    }

    // The single SessionEnd folds the whole turn's scratch into ONE summary.
    fire_hook(
        &hooks_bin,
        &daemon,
        "rb-gate-40turn",
        &project,
        &session_end_event(&project, session_id),
    );

    let mut client = Client::connect(&daemon.socket, namespace)
        .await
        .expect("connect to in-process daemon");
    // Poll briefly for the SessionEnd write to land.
    let mut memories = Vec::new();
    for _ in 0..40 {
        memories = client.list(None, 1000).await.expect("list");
        if !memories.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert!(
        !memories.is_empty(),
        "the 40-turn session must capture SOMETHING (the gate must not pass vacuously)"
    );
    assert!(
        memories.len() <= 5,
        "Phase-3 gate: a 40-turn session must produce <=5 memories, got {} ({:?})",
        memories.len(),
        memories
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
    );
    // The one memory is the folded session summary, listing folded mutations.
    let summary = &memories[0];
    assert!(
        summary.content.contains("Session summary"),
        "the captured memory is the SessionEnd summary, got: {}",
        summary.content
    );

    daemon.stop().await;
    drop(proj_dir);
}
