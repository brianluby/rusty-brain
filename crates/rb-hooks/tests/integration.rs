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
#[derive(Debug, Default, Clone)]
struct Observed {
    saw_remember: bool,
    identity_source: Option<String>,
    identity_agent: Option<String>,
    confidence: Option<f32>,
    content: Option<String>,
    context: Option<String>,
}

// Accept one connection, handshake-ack, and answer the first Remember with a
// canned id. Signals back via the channel what was observed.
async fn serve_one_remember(listener: UnixListener, tx: tokio::sync::oneshot::Sender<Observed>) {
    let mut observed = Observed::default();
    let Ok((stream, _addr)) = listener.accept().await else {
        let _ = tx.send(observed);
        return;
    };
    let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
        Framed::new(stream, LengthDelimitedCodec::new());
    let hs: Handshake = match read_frame(&mut framed).await {
        Ok(h) => h,
        Err(_) => {
            let _ = tx.send(observed);
            return;
        }
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
        let resp = match req {
            Request::Remember {
                confidence,
                content,
                context,
                ..
            } => {
                observed.saw_remember = true;
                // `confidence` is now Option<f32> on the wire (None = no prior);
                // hook captures send Some(0.7), asserted below.
                observed.confidence = confidence;
                observed.content = Some(content);
                observed.context = context;
                Response::Remembered {
                    id: MemoryId::new(),
                }
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
        };
        if write_frame(&mut framed, &resp).await.is_err() {
            break;
        }
    }
    let _ = tx.send(observed);
}

/// Run the hooks binary against an in-process mock daemon, feeding `stdin`,
/// and return what the daemon observed plus the hook's stdout. The dedup cache
/// is isolated to a fresh tempdir so a previously-persisted entry within the
/// 60s TTL cannot suppress the Remember under test as a duplicate.
fn observe_against_mock_daemon(stdin: &str) -> (Observed, String) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("live.sock");
    let socket_str = socket.to_string_lossy().to_string();

    let (tx, rx) = std::sync::mpsc::channel::<Observed>();
    let socket_for_thread = socket.clone();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async move {
            let listener = UnixListener::bind(&socket_for_thread).expect("bind");
            let (otx, orx) = tokio::sync::oneshot::channel::<Observed>();
            let accept = tokio::spawn(serve_one_remember(listener, otx));
            let observed = orx.await.unwrap_or_default();
            let _ = accept.await;
            let _ = tx.send(observed);
        });
    });

    // Give the listener a moment to bind.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut child = hooks_command()
        .args(["--agent", "claude-code"])
        .env("RUSTY_BRAIN_SOCKET", &socket_str)
        .env("XDG_CACHE_HOME", dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn hooks binary");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait for output");
    assert!(output.status.success(), "must exit 0");

    let observed = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap_or_default();
    let _ = server.join();
    let _: PathBuf = socket; // keep tempdir alive until here
    (
        observed,
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
}

#[test]
fn post_tool_use_against_live_daemon_remembers() {
    // The REAL recorded Write payload: tool_response is an OBJECT
    // ({"type":"create","filePath":...}), not the string a hand-authored
    // payload would guess — this pins the object-handling path end-to-end.
    let (observed, _stdout) = observe_against_mock_daemon(REAL_POST_TOOL_USE_WRITE);
    assert!(
        observed.saw_remember,
        "the daemon should have observed a Remember"
    );
    // W0.5: a hook-written memory declares source=hook + the driving agent on
    // the handshake identity, and carries confidence 0.7 on the Remember.
    assert_eq!(observed.identity_source.as_deref(), Some("hook"));
    assert_eq!(observed.identity_agent.as_deref(), Some("claude-code"));
    let confidence = observed.confidence.expect("Remember carries confidence");
    assert!(
        (confidence - 0.7).abs() < f32::EPSILON,
        "hook captures must send confidence 0.7, got {confidence}"
    );
    // The stored summary comes from the real tool_input.file_path...
    assert_eq!(
        observed.content.as_deref(),
        Some("Wrote /private/tmp/rb-w07-capture/proj/hello.txt"),
        "summary must be built from the real payload's tool_input"
    );
    // ...and the context from the real OBJECT tool_response, serialized.
    let context = observed.context.as_deref().expect("context from response");
    assert!(
        context.contains("filePath") && context.contains("create"),
        "object tool_response must be serialized into context, got {context}"
    );
}

#[test]
fn stop_against_live_daemon_remembers_session_summary() {
    // The REAL recorded Stop payload (stop_hook_active=false, object-free).
    // Its cwd is the recording dir, which is not a git repo wherever the test
    // runs, so git detection fails open to "no file modifications".
    let (observed, stdout) = observe_against_mock_daemon(REAL_STOP);
    assert!(
        observed.saw_remember,
        "Stop must store a session-summary memory"
    );
    let confidence = observed.confidence.expect("Remember carries confidence");
    assert!(
        (confidence - 0.7).abs() < f32::EPSILON,
        "hook captures must send confidence 0.7, got {confidence}"
    );
    let content = observed.content.as_deref().unwrap_or_default();
    assert!(
        content.contains("Session ended"),
        "session summary expected, got {content}"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn user_prompt_submit_is_a_no_op_continue() {
    // The REAL recorded UserPromptSubmit payload. The event is intentionally
    // unmodeled today (parses as HookEvent::Other) — W3.2 wires deterministic
    // recall onto it. This pins the current no-op contract: continue, no
    // injection, and ZERO memories written.
    let (observed, stdout) = observe_against_mock_daemon(REAL_USER_PROMPT_SUBMIT);
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
fn session_end_is_a_no_op_continue() {
    // The REAL recorded SessionEnd payload (reason:"other" in -p mode). The
    // event is intentionally unmodeled today (parses as HookEvent::Other) —
    // W3.1's capture inversion makes it the one-summary-per-session writer.
    let (observed, stdout) = observe_against_mock_daemon(REAL_SESSION_END);
    assert!(
        !observed.saw_remember,
        "SessionEnd must not write memories yet (W3.1 owns its consumption)"
    );
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout must be valid JSON");
    assert_eq!(value.get("continue").and_then(|v| v.as_bool()), Some(true));
    assert!(value.get("hookSpecificOutput").is_none());
}

#[test]
fn planted_secrets_in_tool_response_never_reach_the_remember_payload() {
    // SYNTHETIC edge case: the recorded real session contains no secrets, so
    // this payload is hand-authored (real-shaped fields, planted values).
    // W0.5 minimal redaction: a tool response carrying every supported secret
    // shape must arrive at the daemon with markers instead of plaintext.
    let stdin = concat!(
        r#"{"hook_event_name":"PostToolUse","cwd":"/tmp","session_id":"s1","#,
        r#""tool_name":"Bash","tool_input":{"command":"deploy --password=hunter2"},"#,
        r#""tool_response":"key AKIAABCDEFGHIJKLMNOP\nAuthorization: Bearer sk-live-deadbeef\n"#,
        r#"GITHUB_TOKEN=ghp_secret123\n-----BEGIN RSA PRIVATE KEY-----\nMIIfakekeymaterial\n"#,
        r#"-----END RSA PRIVATE KEY-----\ndone"}"#
    );
    let (observed, _stdout) = observe_against_mock_daemon(stdin);
    assert!(observed.saw_remember, "the Remember must reach the daemon");

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
            "planted secret {secret:?} leaked into the remember payload: {payload}"
        );
    }
    assert!(
        payload.contains("[REDACTED:"),
        "redaction markers must be present: {payload}"
    );
    assert!(
        payload.contains("done"),
        "non-secret response text must survive: {payload}"
    );
}

#[test]
fn session_start_on_empty_corpus_injects_zero_tokens() {
    // W1.3 / F30 first-session scenario: the mock daemon answers Context with
    // ZERO memories, so SessionStart must inject literally nothing — no
    // `hookSpecificOutput.additionalContext`, no `systemMessage`, not even a
    // header. The hook still continues. (The full Claude Code session-lifecycle
    // version of this scenario lands in the W3.4 fixture harness.) Stdin is the
    // REAL recorded SessionStart payload.
    let (_observed, stdout) = observe_against_mock_daemon(REAL_SESSION_START);
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
fn real_user_prompt_submit_fixture_parses_as_other() {
    // Unmodeled today BY DESIGN: W3.2 (deterministic recall) consumes the
    // `prompt` field; until then the canonical event is Other with the raw
    // name preserved. The common context still parses.
    let ctx = parse_fixture(REAL_USER_PROMPT_SUBMIT);
    assert_common_context(&ctx);
    assert_eq!(ctx.event, HookEvent::Other("UserPromptSubmit".to_string()));
}

#[test]
fn real_session_end_fixture_parses_as_other() {
    // Unmodeled today BY DESIGN: W3.1 (capture inversion) adds SessionEnd to
    // CLAUDE_EVENTS and the adapter; until then it is Other, name preserved.
    let ctx = parse_fixture(REAL_SESSION_END);
    assert_common_context(&ctx);
    assert_eq!(ctx.event, HookEvent::Other("SessionEnd".to_string()));
}
