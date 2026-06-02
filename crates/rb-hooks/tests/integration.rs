#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end harness tests: drive the built `rusty-brain-hooks` binary via
//! assert_cmd, feeding Claude Code JSON on stdin. The binary MUST always exit 0
//! and emit a JSON object with `"continue": true`, even against a dead socket.

use std::io::Write;
use std::path::PathBuf;

use assert_cmd::cargo::CommandCargoExt;

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
    let dead = "/nonexistent/dir/rb-hooks-test.sock";
    let stdin =
        r#"{"hook_event_name":"SessionStart","cwd":"/tmp","session_id":"s1","source":"startup"}"#;
    let output = run_with_stdin(dead, "claude-code", stdin);

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
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"{}")
        .expect("write");
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

// Accept one connection, handshake-ack, and answer the first Remember with a
// canned id. Signals back via the channel that a Remember was observed.
async fn serve_one_remember(listener: UnixListener, tx: tokio::sync::oneshot::Sender<bool>) {
    let Ok((stream, _addr)) = listener.accept().await else {
        let _ = tx.send(false);
        return;
    };
    let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
        Framed::new(stream, LengthDelimitedCodec::new());
    let _hs: Handshake = match read_frame(&mut framed).await {
        Ok(h) => h,
        Err(_) => {
            let _ = tx.send(false);
            return;
        }
    };
    let _ = write_frame(
        &mut framed,
        &HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: true,
            message: None,
        },
    )
    .await;
    let mut saw_remember = false;
    while let Ok(req) = read_frame::<_, Request>(&mut framed).await {
        let resp = match req {
            Request::Remember { .. } => {
                saw_remember = true;
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
            },
            _ => Response::Pong {
                contract_version: CONTRACT_VERSION,
            },
        };
        if write_frame(&mut framed, &resp).await.is_err() {
            break;
        }
    }
    let _ = tx.send(saw_remember);
}

#[test]
fn post_tool_use_against_live_daemon_remembers() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("live.sock");
    let socket_str = socket.to_string_lossy().to_string();

    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let socket_for_thread = socket.clone();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async move {
            let listener = UnixListener::bind(&socket_for_thread).expect("bind");
            let (otx, orx) = tokio::sync::oneshot::channel::<bool>();
            let accept = tokio::spawn(serve_one_remember(listener, otx));
            let saw = orx.await.unwrap_or(false);
            let _ = accept.await;
            let _ = tx.send(saw);
        });
    });

    // Give the listener a moment to bind.
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Isolate the dedup cache to this tempdir so a previously-persisted entry from
    // an earlier run within the 60s TTL cannot suppress this observation as a
    // duplicate (which would skip the Remember the test asserts on).
    let stdin = r#"{"hook_event_name":"PostToolUse","cwd":"/tmp","session_id":"s1","tool_name":"Edit","tool_input":{"file_path":"/src/uniqueW9.rs"},"tool_response":"ok"}"#;
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

    let saw_remember = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap_or(false);
    assert!(saw_remember, "the daemon should have observed a Remember");
    let _ = server.join();
    let _: PathBuf = socket; // keep tempdir alive until here
}
