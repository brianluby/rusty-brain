#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end harness tests: drive the built `rusty-brain-hooks` binary via
//! assert_cmd, feeding Claude Code JSON on stdin. The binary MUST always exit 0
//! and emit a JSON object with `"continue": true`, even against a dead socket.
//!
//! Lifecycle tests feed the REAL recorded Claude Code payloads from
//! `tests/fixtures/claude_code/` (W0.7 carryover — see the fixtures README for
//! provenance and sanitization). Hand-authored payloads are retained ONLY for
//! synthetic edge cases (garbage stdin, planted secrets) and are marked as such.

use std::io::Write;
use std::path::PathBuf;

use assert_cmd::cargo::CommandCargoExt;

// ---- REAL Claude Code hook payloads (recorded 2026-06-12, claude 2.1.175) ----

const REAL_SESSION_START: &str = include_str!("fixtures/claude_code/session_start.json");
const REAL_USER_PROMPT_SUBMIT: &str = include_str!("fixtures/claude_code/user_prompt_submit.json");
const REAL_POST_TOOL_USE_WRITE: &str =
    include_str!("fixtures/claude_code/post_tool_use_write.json");
const REAL_STOP: &str = include_str!("fixtures/claude_code/stop.json");
const REAL_SESSION_END: &str = include_str!("fixtures/claude_code/session_end.json");

// ---- REAL recorded OpenCode hook payloads (PR #46, see fixtures/opencode/) ----
// Both share session id `ses_107a97…` so a PostToolUse scratch aligns into the
// following `session.idle` fold. `tool_execute_after` is a BASH event (command
// `echo hi`): it pins bash-command capture only, NOT file-edit capture (Gap B /
// `apply_patch`, tracked separately).
const OPENCODE_TOOL_EXECUTE_AFTER: &str = include_str!("fixtures/opencode/tool_execute_after.json");
const OPENCODE_TOOL_EXECUTE_AFTER_APPLY_PATCH: &str =
    include_str!("fixtures/opencode/tool_execute_after_apply_patch.json");
const OPENCODE_SESSION_IDLE: &str = include_str!("fixtures/opencode/session_idle.json");

fn hooks_command() -> std::process::Command {
    std::process::Command::cargo_bin("rusty-brain-hooks").expect("binary builds")
}

fn run_with_stdin(socket: &str, agent: &str, stdin_json: &str) -> std::process::Output {
    let mut child = hooks_command()
        .args(["--agent", agent])
        .env("RUSTY_BRAIN_SOCKET", socket)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hooks binary");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin_json.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for output")
}

#[test]
fn session_start_against_dead_socket_fails_open() {
    // A socket path that does not exist and cannot auto-start anything useful in
    // the test environment: the harness must still exit 0 + {"continue":true}.
    // Stdin is the REAL recorded SessionStart payload.
    let dead = "/nonexistent/dir/rb-hooks-test.sock";
    let output = run_with_stdin(dead, "claude-code", REAL_SESSION_START);

    assert!(
        output.status.success(),
        "must exit 0 (fail-open); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(
        value.get("continue").and_then(|v| v.as_bool()),
        Some(true),
        "continue must be true, got {stdout}"
    );
}

#[test]
fn invalid_stdin_fails_open() {
    // SYNTHETIC edge case (no real payload can be garbage): hand-authored stdin.
    let dead = "/nonexistent/dir/rb-hooks-test2.sock";
    let output = run_with_stdin(dead, "claude-code", "not json at all {{{");
    assert!(output.status.success(), "invalid stdin must still exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn unknown_agent_fails_open_with_literal_continue() {
    // Unknown agent => arg parse error => last-resort literal {"continue":true}.
    let mut child = hooks_command()
        .args(["--agent", "bogus"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    // The bogus-agent binary fails open and may exit BEFORE reading stdin, closing
    // the read end of the pipe; the resulting broken-pipe write error is EXPECTED,
    // not a failure. The contract under test is exit-0 + a literal continue,
    // independent of whether stdin was consumed. Asserting the write succeeded
    // raced on fast CI runners (the child exited before the parent's write).
    let _ = child.stdin.take().expect("stdin").write_all(b"{}");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "unknown agent must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"continue\":true") || stdout.contains("\"continue\": true"),
        "must emit continue:true, got {stdout}"
    );
}

// ---- Live in-process daemon: assert a PostToolUse remember happens ----

use rb_proto::{
    read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
use rb_types::MemoryId;
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// What the mock daemon observed from the hook: the handshake identity and
/// the first Remember's payload (W0.5 provenance + confidence assertions).
///
/// The scalar `content`/`context`/`confidence` fields mirror the FIRST Remember
/// (back-compat with the single-fold lifecycle tests). `remembers` records EVERY
/// Remember in order — needed to assert multi-checkpoint supersede chains, where
/// the second checkpoint's `supersedes` must point at the first's issued id.
#[derive(Debug, Default, Clone)]
struct Observed {
    saw_remember: bool,
    identity_source: Option<String>,
    identity_agent: Option<String>,
    confidence: Option<f32>,
    content: Option<String>,
    context: Option<String>,
    remembers: Vec<RememberObs>,
}

/// One Remember the mock observed: its folded `content`, the `supersedes` id it
/// carried (the prior summary it replaces, if any), and the canned `issued_id`
/// the mock answered with (so a later checkpoint's `supersedes` can be matched
/// against it).
#[derive(Debug, Clone)]
struct RememberObs {
    content: String,
    supersedes: Option<String>,
    issued_id: String,
}

/// Record one request into `observed` and build the mock's response. A Remember
/// is answered with a fresh canned id (recorded on the `RememberObs` so a later
/// checkpoint's `supersedes` can be matched against it); Context/Ping/other get
/// benign answers. The scalar fields track the FIRST Remember for back-compat.
fn record_and_respond(req: Request, observed: &mut Observed) -> Response {
    match req {
        Request::Remember {
            confidence,
            content,
            context,
            supersedes,
            ..
        } => {
            let id = MemoryId::new();
            observed.saw_remember = true;
            if observed.remembers.is_empty() {
                observed.confidence = confidence;
                observed.content = Some(content.clone());
                observed.context = context;
            }
            observed.remembers.push(RememberObs {
                content,
                supersedes: supersedes.map(|m| m.to_string()),
                issued_id: id.to_string(),
            });
            Response::Remembered { id }
        }
        Request::Context => Response::ContextResult {
            recent: vec![],
            important: vec![],
            total: 0,
        },
        Request::Ping => Response::Pong {
            contract_version: CONTRACT_VERSION,
            recall_channels: None,
        },
        _ => Response::Pong {
            contract_version: CONTRACT_VERSION,
            recall_channels: None,
        },
    }
}

// Serve the mock daemon. When `single` is true, serve exactly one connection
// (the single-fold lifecycle tests); when false, keep accepting a fresh
// connection per fold (a multi-checkpoint sequence). The driver signals
// completion explicitly via `shutdown_rx` once it has issued every hook
// invocation, so the accept loop never uses a wall-clock timeout as a
// sequence-completion signal — that previously made multi-checkpoint runs
// timing-sensitive (a slow CI spawn could close the listener before the next
// checkpoint connected). The whole call stays bounded by the driver's
// `recv_timeout`. Signals back what was observed.
async fn serve_remembers(
    listener: UnixListener,
    tx: tokio::sync::oneshot::Sender<Observed>,
    single: bool,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let mut observed = Observed::default();
    loop {
        // Accept the next connection, but stop the moment the driver signals it
        // has finished issuing invocations (or its thread dies and drops the
        // sender, which resolves the recv to Err). `biased` prefers a ready
        // connection so a genuine fold is never dropped for a pending shutdown.
        let accepted = tokio::select! {
            biased;
            res = listener.accept() => res,
            _ = &mut shutdown_rx => break,
        };
        let Ok((stream, _addr)) = accepted else {
            // accept errored: stop and report what was observed so far.
            break;
        };
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());
        let hs: Handshake = match read_frame(&mut framed).await {
            Ok(h) => h,
            // A bad handshake aborts this connection; in single mode we are done,
            // otherwise wait for the next checkpoint's connection.
            Err(_) if single => break,
            Err(_) => continue,
        };
        if let Some(identity) = hs.identity {
            observed.identity_source = identity.source;
            observed.identity_agent = identity.agent;
        }
        let _ = write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
            },
        )
        .await;
        while let Ok(req) = read_frame::<_, Request>(&mut framed).await {
            let resp = record_and_respond(req, &mut observed);
            if write_frame(&mut framed, &resp).await.is_err() {
                break;
            }
        }
        if single {
            break;
        }
    }
    let _ = tx.send(observed);
}

/// Run a SEQUENCE of hook invocations for `agent` against ONE mock daemon,
/// sharing the dedup + scratch cache dir (so a PostToolUse scratch survives into
/// the following fold) and an optional pinned namespace. Returns what the mock
/// observed plus the LAST invocation's stdout. The cache is isolated to a fresh
/// tempdir so a prior run can never interfere. When `single`, the mock serves
/// exactly one connection; otherwise it serves every fold's connection (for
/// multi-checkpoint supersede assertions).
fn observe_sequence_impl(
    events: &[&str],
    namespace: Option<&str>,
    agent: &str,
    single: bool,
) -> (Observed, String) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("live.sock");
    let socket_str = socket.to_string_lossy().to_string();

    let (tx, rx) = std::sync::mpsc::channel::<Observed>();
    // Driver -> mock "all invocations issued" signal (see `serve_remembers`).
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let socket_for_thread = socket.clone();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async move {
            let listener = UnixListener::bind(&socket_for_thread).expect("bind");
            let (otx, orx) = tokio::sync::oneshot::channel::<Observed>();
            let accept = tokio::spawn(serve_remembers(listener, otx, single, shutdown_rx));
            let observed = orx.await.unwrap_or_default();
            let _ = accept.await;
            let _ = tx.send(observed);
        });
    });

    // Give the listener a moment to bind before the first hook connects.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut last_stdout = String::new();
    for event in events {
        let mut cmd = hooks_command();
        cmd.args(["--agent", agent])
            .env("RUSTY_BRAIN_SOCKET", &socket_str)
            .env("XDG_CACHE_HOME", dir.path());
        if let Some(ns) = namespace {
            cmd.env("RUSTY_BRAIN_NAMESPACE", ns);
        }
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn hooks binary");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(event.as_bytes())
            .expect("write stdin");
        let output = child.wait_with_output().expect("wait for output");
        assert!(output.status.success(), "must exit 0");
        last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
    }

    // Every hook invocation has run: tell the mock to stop accepting. Dropping
    // the sender on an early panic also resolves the mock's recv to Err, so it
    // never hangs.
    let _ = shutdown_tx.send(());

    let observed = rx
        .recv_timeout(std::time::Duration::from_secs(12))
        .unwrap_or_default();
    let _ = server.join();
    let _: PathBuf = socket; // keep tempdir alive until here
    (observed, last_stdout)
}

/// Single-fold lifecycle sequence: the mock serves exactly one daemon
/// connection (the one fold a SessionEnd/SessionCheckpoint sends, if any).
fn observe_sequence_against_mock_daemon(
    events: &[&str],
    namespace: Option<&str>,
    agent: &str,
) -> (Observed, String) {
    observe_sequence_impl(events, namespace, agent, true)
}

/// Multi-checkpoint sequence: the mock serves EVERY fold's connection and
/// records each Remember, so a supersede chain across repeated checkpoints can
/// be asserted. Costs one idle-accept timeout at the end of the run.
fn observe_sequence_collecting_remembers(
    events: &[&str],
    namespace: Option<&str>,
    agent: &str,
) -> (Observed, String) {
    observe_sequence_impl(events, namespace, agent, false)
}

/// Single-event convenience over [`observe_sequence_against_mock_daemon`].
fn observe_against_mock_daemon(stdin: &str, agent: &str) -> (Observed, String) {
    observe_sequence_against_mock_daemon(&[stdin], None, agent)
}

#[test]
fn post_tool_use_writes_no_memory() {
    // W3.1 capture inversion: PostToolUse appends to the per-session scratch and
    // writes ZERO memories — it does not even connect to the daemon (the mock
    // observes nothing). The REAL recorded Write payload pins this end-to-end.
    let (observed, stdout) = observe_against_mock_daemon(REAL_POST_TOOL_USE_WRITE, "claude-code");
    assert!(
        !observed.saw_remember,
        "PostToolUse must write no memory (W3.1 capture inversion)"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn stop_writes_no_memory() {
    // W3.1: a per-turn Stop stores nothing; the session fold is at SessionEnd.
    let (observed, stdout) = observe_against_mock_daemon(REAL_STOP, "claude-code");
    assert!(
        !observed.saw_remember,
        "Stop must store nothing (W3.1); the fold is at SessionEnd"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn user_prompt_submit_is_a_no_op_continue() {
    // The REAL recorded UserPromptSubmit payload. Unmodeled today (parses as
    // HookEvent::Other) — W3.2 wires deterministic recall onto it. Pins the
    // current no-op contract: continue, no injection, ZERO memories.
    let (observed, stdout) = observe_against_mock_daemon(REAL_USER_PROMPT_SUBMIT, "claude-code");
    assert!(
        !observed.saw_remember,
        "UserPromptSubmit must not write memories (W3.2 owns its consumption)"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
    assert!(
        value.get("hookSpecificOutput").is_none(),
        "no context injection on an unmodeled event, got {stdout}"
    );
}

#[test]
fn session_end_with_empty_scratch_writes_nothing() {
    // SessionEnd connects, but a fresh cache dir (empty scratch), a non-existent
    // cwd (no git changes), and a non-existent transcript leave nothing to fold
    // → no memory, continue. The REAL recorded SessionEnd payload drives it.
    let (observed, stdout) = observe_against_mock_daemon(REAL_SESSION_END, "claude-code");
    assert!(
        !observed.saw_remember,
        "an empty-scratch SessionEnd has nothing to fold and writes nothing"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
    assert!(value.get("hookSpecificOutput").is_none());
}

#[test]
fn session_end_folds_post_tool_use_scratch_into_one_summary() {
    // The W3.1 lifecycle end-to-end against the mock: a PostToolUse appends the
    // touched file to the per-session scratch (no memory, no connect); the
    // following SessionEnd folds it into ONE summary Remember. Both share the
    // cache dir + pinned namespace, and the REAL fixtures share a session id, so
    // the scratch aligns across the two invocations.
    let (observed, _stdout) = observe_sequence_against_mock_daemon(
        &[REAL_POST_TOOL_USE_WRITE, REAL_SESSION_END],
        Some("rb-it-fold"),
        "claude-code",
    );
    assert!(
        observed.saw_remember,
        "SessionEnd must fold the PostToolUse scratch into one summary"
    );
    // W0.5 provenance: the fold is a hook write at confidence 0.7.
    assert_eq!(observed.identity_source.as_deref(), Some("hook"));
    assert_eq!(observed.identity_agent.as_deref(), Some("claude-code"));
    let confidence = observed.confidence.expect("Remember carries confidence");
    assert!(
        (confidence - 0.7).abs() < f32::EPSILON,
        "hook captures must send confidence 0.7, got {confidence}"
    );
    // The summary lists the file the PostToolUse touched (from the REAL payload).
    let content = observed.content.as_deref().unwrap_or_default();
    assert!(
        content.contains("hello.txt"),
        "the summary must fold the touched file: {content}"
    );
    assert!(
        content.contains("Files touched"),
        "the summary must carry a files section: {content}"
    );
}

#[test]
fn opencode_session_idle_folds_tool_scratch_into_one_checkpoint_summary() {
    // Gap A end-to-end: OpenCode's `session.idle` is the fold event. A
    // `tool.execute.after` (bash) appends the command to the per-session scratch
    // (no memory, no connect); the following `session.idle` folds it into ONE
    // SessionCheckpoint summary Remember. Both REAL fixtures share the cache dir,
    // pinned namespace, and recorded session id, so the scratch aligns.
    //
    // FAILS before Gap A: `session.idle` maps to canonical Stop, which stores
    // nothing, so the mock observes no Remember.
    //
    // SCOPE: the fixture is a BASH event, so this proves bash-command capture
    // folds — NOT file-edit capture (Gap B / `apply_patch`).
    let (observed, _stdout) = observe_sequence_against_mock_daemon(
        &[OPENCODE_TOOL_EXECUTE_AFTER, OPENCODE_SESSION_IDLE],
        Some("rb-it-opencode-fold"),
        "opencode",
    );
    assert!(
        observed.saw_remember,
        "OpenCode session.idle must fold the tool scratch into one checkpoint summary"
    );
    // Same W0.5 provenance as Claude, but stamped with the OpenCode agent id.
    assert_eq!(observed.identity_source.as_deref(), Some("hook"));
    assert_eq!(observed.identity_agent.as_deref(), Some("opencode"));
    let confidence = observed.confidence.expect("Remember carries confidence");
    assert!(
        (confidence - 0.7).abs() < f32::EPSILON,
        "hook captures must send confidence 0.7, got {confidence}"
    );
    // The summary folds the recorded bash command under its commands section.
    let content = observed.content.as_deref().unwrap_or_default();
    assert!(
        content.contains("echo hi"),
        "the checkpoint summary must fold the recorded bash command: {content}"
    );
    assert!(
        content.contains("Commands run"),
        "the summary must carry a commands section: {content}"
    );
}

#[test]
fn opencode_apply_patch_file_edit_folds_into_checkpoint_summary() {
    // Gap B end-to-end: OpenCode's file edits go through the `apply_patch` tool,
    // whose payload carries a V4A `patchText` (no `file_path`). The edited path
    // must be captured into the scratch and folded by the following session.idle
    // checkpoint — otherwise every OpenCode file edit silently drops on the
    // floor. Replays the REAL recorded apply_patch payload (result.jsonl:10,
    // re-shaped to the per-event hook format); it shares the recorded session id
    // with session_idle.json so the scratch aligns.
    let (observed, _stdout) = observe_sequence_against_mock_daemon(
        &[
            OPENCODE_TOOL_EXECUTE_AFTER_APPLY_PATCH,
            OPENCODE_SESSION_IDLE,
        ],
        Some("rb-it-opencode-apply-patch"),
        "opencode",
    );
    assert!(
        observed.saw_remember,
        "OpenCode session.idle must fold the apply_patch file edit into a checkpoint summary"
    );
    assert_eq!(observed.identity_agent.as_deref(), Some("opencode"));
    let content = observed.content.as_deref().unwrap_or_default();
    assert!(
        content.contains("notes.txt"),
        "the checkpoint summary must fold the apply_patch-ed file: {content}"
    );
    assert!(
        content.contains("Files touched"),
        "the summary must carry a files section: {content}"
    );
}

#[test]
fn opencode_second_session_idle_supersedes_the_first_checkpoint() {
    // Gap A multi-fire safety: two `session.idle` events => two SessionCheckpoint
    // folds. The second must SUPERSEDE the first (one live summary, not two):
    // checkpoint #1 RETAINS the scratch, checkpoint #2 re-folds it and points
    // `supersedes` at #1's issued id. This is why `session.idle` maps to a
    // (multi-fire-safe) checkpoint rather than a once-per-session SessionEnd.
    let (observed, _stdout) = observe_sequence_collecting_remembers(
        &[
            OPENCODE_TOOL_EXECUTE_AFTER,
            OPENCODE_SESSION_IDLE,
            OPENCODE_SESSION_IDLE,
        ],
        Some("rb-it-opencode-supersede"),
        "opencode",
    );
    assert_eq!(
        observed.remembers.len(),
        2,
        "two session.idle checkpoints must produce exactly two Remembers, got {:?}",
        observed.remembers
    );
    assert_eq!(
        observed.remembers[0].supersedes, None,
        "the first checkpoint supersedes nothing"
    );
    assert_eq!(
        observed.remembers[1].supersedes.as_deref(),
        Some(observed.remembers[0].issued_id.as_str()),
        "the second checkpoint must supersede the first checkpoint's summary id"
    );
    // The retained scratch feeds the second fold too: the command persists into
    // the superseding summary (one progressively-complete memory, not a reset).
    assert!(
        observed.remembers[1].content.contains("echo hi"),
        "the superseding summary must still carry the folded command: {}",
        observed.remembers[1].content
    );
}

#[test]
fn planted_secrets_never_reach_the_folded_remember_payload() {
    // W0.5 minimal redaction at the WIRE level (the DB-bytes complement lives in
    // rb-install/tests/e2e.rs): a Bash command + a FAILING tool response carrying
    // every supported secret shape are folded by SessionEnd into the Remember,
    // which must arrive with markers instead of plaintext. SYNTHETIC payloads
    // (the recorded session contains no secrets).
    let post = concat!(
        r#"{"hook_event_name":"PostToolUse","cwd":"/tmp/rb-it-secret-xyz","session_id":"s1","#,
        r#""tool_name":"Bash","tool_input":{"command":"deploy --password=hunter2 && echo keepme"},"#,
        r#""tool_response":{"is_error":true,"stderr":"key AKIAABCDEFGHIJKLMNOP\nAuthorization: Bearer sk-live-deadbeef\n"#,
        r#"GITHUB_TOKEN=ghp_secret123\n-----BEGIN RSA PRIVATE KEY-----\nMIIfakekeymaterial\n-----END RSA PRIVATE KEY-----\ndone"}}"#
    );
    let session_end = r#"{"hook_event_name":"SessionEnd","cwd":"/tmp/rb-it-secret-xyz","session_id":"s1","reason":"other"}"#;
    let (observed, _stdout) = observe_sequence_against_mock_daemon(
        &[post, session_end],
        Some("rb-it-secret"),
        "claude-code",
    );
    assert!(
        observed.saw_remember,
        "the SessionEnd fold of the scratch must reach the daemon"
    );
    let payload = format!(
        "{}\n{}",
        observed.content.as_deref().unwrap_or_default(),
        observed.context.as_deref().unwrap_or_default()
    );
    for secret in [
        "hunter2",
        "AKIAABCDEFGHIJKLMNOP",
        "sk-live-deadbeef",
        "ghp_secret123",
        "MIIfakekeymaterial",
    ] {
        assert!(
            !payload.contains(secret),
            "planted secret {secret:?} leaked into the folded payload: {payload}"
        );
    }
    assert!(
        payload.contains("[REDACTED:"),
        "redaction markers must be present: {payload}"
    );
    assert!(
        payload.contains("keepme"),
        "non-secret command text must survive: {payload}"
    );
}

#[test]
fn session_start_on_empty_corpus_injects_zero_tokens() {
    // W1.3 / F30 first-session scenario: the mock daemon answers Context with
    // ZERO memories, so SessionStart must inject literally nothing — no
    // `hookSpecificOutput.additionalContext`, no `systemMessage`, not even a
    // header. The hook still continues. Stdin is the REAL recorded SessionStart.
    let (_observed, stdout) = observe_against_mock_daemon(REAL_SESSION_START, "claude-code");
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(
        value.get("continue").and_then(|v| v.as_bool()),
        Some(true),
        "must continue: {stdout}"
    );
    assert!(
        value.get("hookSpecificOutput").is_none(),
        "empty corpus must inject zero context tokens, got {stdout}"
    );
    assert!(
        value.get("systemMessage").is_none(),
        "empty corpus must not emit a user-facing message either, got {stdout}"
    );
}

// ---- Adapter parsing pinned against the REAL fixtures (W0.7 carryover) ----
//
// These assert the EXACT HookContext the ClaudeCodeCli adapter produces for
// every recorded payload, so any drift in Claude Code's wire format (or an
// adapter regression) fails loudly against ground truth, not approximations.

use rb_agents::{AgentCli, ClaudeCodeCli, HookEvent};

/// The transcript path shared by every event of the recorded session
/// (sanitized: recording user's home dir -> `/Users/user`).
const REAL_TRANSCRIPT_PATH: &str = "/Users/user/.claude/projects/-private-tmp-rb-w07-capture-proj/8f3433c5-d4d5-4c67-abfa-78175bef9b64.jsonl";
const REAL_CWD: &str = "/private/tmp/rb-w07-capture/proj";
const REAL_SESSION_ID: &str = "8f3433c5-d4d5-4c67-abfa-78175bef9b64";

fn parse_fixture(fixture: &str) -> rb_agents::HookContext {
    let raw: serde_json::Value = serde_json::from_str(fixture.trim()).expect("fixture is JSON");
    ClaudeCodeCli.parse_input(&raw)
}

/// Every recorded event carries the same session id, cwd, and transcript path.
fn assert_common_context(ctx: &rb_agents::HookContext) {
    assert_eq!(ctx.cwd, PathBuf::from(REAL_CWD));
    assert_eq!(ctx.session_id.as_deref(), Some(REAL_SESSION_ID));
    assert_eq!(
        ctx.transcript_path,
        Some(PathBuf::from(REAL_TRANSCRIPT_PATH)),
        "transcript_path is sent on EVERY Claude Code event and must be parsed"
    );
}

#[test]
fn real_session_start_fixture_parses_exactly() {
    let ctx = parse_fixture(REAL_SESSION_START);
    assert_common_context(&ctx);
    assert_eq!(
        ctx.event,
        HookEvent::SessionStart {
            source: Some("startup".to_string())
        }
    );
}

#[test]
fn real_post_tool_use_write_fixture_parses_exactly() {
    let ctx = parse_fixture(REAL_POST_TOOL_USE_WRITE);
    assert_common_context(&ctx);
    match ctx.event {
        HookEvent::PostToolUse {
            tool_name,
            tool_input,
            tool_response,
        } => {
            assert_eq!(tool_name, "Write");
            assert_eq!(
                tool_input,
                serde_json::json!({
                    "file_path": "/private/tmp/rb-w07-capture/proj/hello.txt",
                    "content": "hi"
                })
            );
            // The REAL tool_response is an object — the shape hand-authored
            // payloads got wrong (they guessed a plain string).
            assert_eq!(
                tool_response,
                serde_json::json!({
                    "type": "create",
                    "filePath": "/private/tmp/rb-w07-capture/proj/hello.txt",
                    "content": "hi",
                    "structuredPatch": [],
                    "originalFile": null,
                    "userModified": false
                })
            );
        }
        other => unreachable!("expected PostToolUse, got {other:?}"),
    }
}

#[test]
fn real_stop_fixture_parses_exactly() {
    let ctx = parse_fixture(REAL_STOP);
    assert_common_context(&ctx);
    assert_eq!(
        ctx.event,
        HookEvent::Stop {
            last_assistant_message: Some(
                "Done. Created `/private/tmp/rb-w07-capture/proj/hello.txt` with content \"hi\"."
                    .to_string()
            ),
            stop_hook_active: false,
        }
    );
}

#[test]
fn real_user_prompt_submit_fixture_parses_as_user_prompt_submit() {
    // W3.2(a) landed: the adapter models UserPromptSubmit and parses the
    // recorded payload's `prompt` field — the deterministic-recall query.
    let ctx = parse_fixture(REAL_USER_PROMPT_SUBMIT);
    assert_common_context(&ctx);
    assert_eq!(
        ctx.event,
        HookEvent::UserPromptSubmit {
            prompt: Some("Create a file named hello.txt containing exactly: hi".to_string())
        }
    );
}

#[test]
fn real_session_end_fixture_parses_as_session_end() {
    // W3.1 landed: the adapter now models SessionEnd (the capture point) and
    // parses the recorded payload's `reason` ("other" in -p mode).
    let ctx = parse_fixture(REAL_SESSION_END);
    assert_common_context(&ctx);
    assert_eq!(
        ctx.event,
        HookEvent::SessionEnd {
            reason: Some("other".to_string())
        }
    );
}
