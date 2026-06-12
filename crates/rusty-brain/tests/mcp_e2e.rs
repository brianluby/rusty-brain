//! End-to-end: drive the real `rusty-brain mcp` adapter over its stdin/stdout
//! against a real auto-started daemon (tempdir socket+DB, offline
//! DeterministicProvider). Asserts a remembered memory is recalled through MCP.
//! VOYAGE_API_KEY is cleared so CI never contacts a live embedding API.
//!
//! The adapter (`mcp`) child is reaped by `Reap`. The daemon it auto-starts is a
//! detached grandchild on the tempdir socket; it is left to its idle timeout and
//! is not owned here (matches `end_to_end.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::cargo::cargo_bin;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Hard upper bound on waiting for any single response. Daemon auto-start +
/// offline embedding is sub-second locally; this just prevents a wedged adapter
/// from hanging CI forever (precedent: `end_to_end.rs` `wait_for_socket` uses
/// a ~10s cap).
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

/// Owns the spawned `mcp` child and reaps it on drop, even if an assertion
/// panics and unwinds the test.
struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Write one JSON-RPC frame as a single `\n`-terminated line to the child stdin.
fn send(stdin: &mut std::process::ChildStdin, frame: &Value) {
    let line = format!("{}\n", serde_json::to_string(frame).unwrap());
    stdin.write_all(line.as_bytes()).expect("write frame");
    stdin.flush().expect("flush frame");
}

/// Spawn a thread draining the child's stdout into a channel of parsed JSON
/// lines, so the test side can wait with a deadline instead of blocking forever
/// on a wedged adapter. Returns the receiver; the thread ends at EOF.
fn spawn_line_reader(stdout: std::process::ChildStdout) -> Receiver<Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break, // EOF or read error: stop draining.
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
                        if tx.send(value).is_err() {
                            break; // Receiver dropped (test finished).
                        }
                    }
                    // Non-JSON lines are ignored (no log noise expected on stdout).
                }
            }
        }
    });
    rx
}

/// Receive response frames until one with the given `id` is found, bounded by
/// `RESPONSE_TIMEOUT` so a deadlock cannot hang CI. Panics on timeout or stream
/// end before the match.
fn read_until_id(rx: &Receiver<Value>, id: i64) -> Value {
    loop {
        match rx.recv_timeout(RESPONSE_TIMEOUT) {
            Ok(value) => {
                if value.get("id").and_then(Value::as_i64) == Some(id) {
                    return value;
                }
                // Otherwise a notification/frame we don't expect; ignore.
            }
            Err(RecvTimeoutError::Timeout) => {
                panic!("timed out after {RESPONSE_TIMEOUT:?} waiting for response id {id}")
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("stream ended before response id {id} arrived")
            }
        }
    }
}

/// Issue one `poll_changes` and return the parsed payload JSON.
fn poll_changes(stdin: &mut ChildStdin, rx: &Receiver<Value>, id: i64) -> Value {
    send(
        stdin,
        &json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
            "name":"poll_changes","arguments":{}
        }}),
    );
    let resp = read_until_id(rx, id);
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("poll_changes tool text");
    serde_json::from_str(text).expect("poll_changes payload is JSON")
}

/// Poll until the subscriber status equals `want`, returning the matching
/// payload. Panics (with the last payload) if `deadline` passes first.
fn wait_for_subscriber_status(
    stdin: &mut ChildStdin,
    rx: &Receiver<Value>,
    next_id: &mut i64,
    want: &str,
    deadline: Duration,
) -> Value {
    let end = Instant::now() + deadline;
    let mut last = Value::Null;
    while Instant::now() < end {
        let id = *next_id;
        *next_id += 1;
        last = poll_changes(stdin, rx, id);
        if last["subscriber"]["status"] == want {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("subscriber never reached status {want:?}; last payload: {last}");
}

#[test]
fn mcp_remember_then_recall_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    // Use a subdirectory for the socket so the daemon's `prepare_socket_dir`
    // can create it with 0700; `tempfile::tempdir()` creates dirs with 0755,
    // which the daemon rejects for security (same pattern as end_to_end.rs).
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    // Launch the MCP adapter; it auto-starts the daemon on the temp socket+DB.
    let mut child = Command::new(&exe)
        .arg("mcp")
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env_remove("VOYAGE_API_KEY") // force offline DeterministicProvider
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp adapter");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let _reap = Reap(child);
    let rx = spawn_line_reader(stdout);

    // 1) initialize
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2024-11-05"}}),
    );
    let init = read_until_id(&rx, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "rusty-brain");
    assert_eq!(
        init["result"]["serverInfo"]["contractVersion"],
        rb_proto::CONTRACT_VERSION
    );

    // 2) initialized notification (no response expected)
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );

    // 3) tools/call remember
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"remember",
            "arguments":{
                "content":"always use one database and one transaction",
                "type":"architecture_decision",
                "importance":9
            }
        }}),
    );
    let remembered = read_until_id(&rx, 2);
    assert_ne!(
        remembered["result"]["isError"],
        json!(true),
        "remember failed: {remembered}"
    );
    let remember_text = remembered["result"]["content"][0]["text"]
        .as_str()
        .expect("remember tool text");
    // The result text is JSON: {"id":"<uuid>"}.
    let remember_payload: Value = serde_json::from_str(remember_text).unwrap();
    assert!(
        remember_payload["id"].is_string(),
        "remember returned an id: {remember_text}"
    );

    // 4) tools/call recall — the stored content must come back.
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"recall",
            "arguments":{ "query":"one database transaction", "limit":10 }
        }}),
    );
    let recalled = read_until_id(&rx, 3);
    assert_ne!(
        recalled["result"]["isError"],
        json!(true),
        "recall errored: {recalled}"
    );
    let recall_text = recalled["result"]["content"][0]["text"]
        .as_str()
        .expect("recall tool text");
    assert!(
        recall_text.contains("one database and one transaction"),
        "recalled content missing from MCP result; got: {recall_text}"
    );

    // Close stdin so the adapter shuts down; the Reap guard kills/reaps the mcp
    // child regardless. The detached daemon grandchild times out on its own.
    drop(stdin);
}

// W0.1 (F01/F11/F48): the daemon closes idle connections; with the timeout
// injected down to 1s, a call after a 2s gap proves the proxy reconnects
// instead of failing every subsequent tool call.
#[test]
fn mcp_call_succeeds_after_daemon_idle_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    // RUSTY_BRAIN_IDLE_TIMEOUT_SECS is on FORWARD_ENV, so the auto-started
    // daemon inherits the 1s idle timeout.
    let mut child = Command::new(&exe)
        .arg("mcp")
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env("RUSTY_BRAIN_IDLE_TIMEOUT_SECS", "1")
        .env_remove("VOYAGE_API_KEY") // force offline DeterministicProvider
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp adapter");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let _reap = Reap(child);
    let rx = spawn_line_reader(stdout);

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2024-11-05"}}),
    );
    read_until_id(&rx, 1);

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"remember",
            "arguments":{ "content":"idle survivors reconnect", "importance":7 }
        }}),
    );
    let remembered = read_until_id(&rx, 2);
    assert_ne!(
        remembered["result"]["isError"],
        json!(true),
        "remember failed: {remembered}"
    );

    // Exceed the injected idle timeout so the daemon closes the proxy's
    // connection between request frames.
    std::thread::sleep(Duration::from_secs(2));

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"recall",
            "arguments":{ "query":"idle survivors", "limit":10 }
        }}),
    );
    let recalled = read_until_id(&rx, 3);
    assert_ne!(
        recalled["result"]["isError"],
        json!(true),
        "recall after idle must succeed via reconnect: {recalled}"
    );
    let recall_text = recalled["result"]["content"][0]["text"]
        .as_str()
        .expect("recall tool text");
    assert!(
        recall_text.contains("idle survivors reconnect"),
        "recalled content missing after reconnect; got: {recall_text}"
    );

    drop(stdin);
}

// W0.1 (F57): kill the daemon under a live adapter. The change subscriber must
// report `disconnected` while down, reconnect with backoff (auto-starting a
// fresh daemon), report `connected` again, and then deliver change events for
// new writes — proving poll_changes distinguishes "quiet" from "deaf" and
// recovers without an adapter restart.
#[test]
fn subscriber_recovers_after_daemon_restart_and_reports_status() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    let mut child = Command::new(&exe)
        .arg("mcp")
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env_remove("VOYAGE_API_KEY") // force offline DeterministicProvider
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp adapter");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let _reap = Reap(child);
    let rx = spawn_line_reader(stdout);
    let mut next_id = 100i64;

    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2024-11-05"}}),
    );
    read_until_id(&rx, 1);

    // The subscriber connects in the background; wait until it reports in
    // BEFORE writing, so the change event below cannot race the subscription.
    wait_for_subscriber_status(
        &mut stdin,
        &rx,
        &mut next_id,
        "connected",
        Duration::from_secs(10),
    );

    // SIGKILL the auto-started daemon via its pidfile (written next to the
    // socket by the daemon's BindGuard).
    let pidfile = socket.with_extension("pid");
    let pid = std::fs::read_to_string(&pidfile)
        .expect("daemon pidfile")
        .trim()
        .to_string();
    let killed = Command::new("kill")
        .args(["-9", &pid])
        .status()
        .expect("run kill");
    assert!(killed.success(), "kill -9 {pid} failed");

    // The subscriber notices the dead stream and sits in its >=500ms backoff
    // window with the buffer marked disconnected (50ms polls cannot miss it).
    let down = wait_for_subscriber_status(
        &mut stdin,
        &rx,
        &mut next_id,
        "disconnected",
        Duration::from_secs(5),
    );
    assert!(
        down["subscriber"]["for_secs"].is_u64(),
        "disconnected status must carry an outage duration: {down}"
    );

    // The reconnect loop auto-starts a fresh daemon and resubscribes.
    wait_for_subscriber_status(
        &mut stdin,
        &rx,
        &mut next_id,
        "connected",
        Duration::from_secs(10),
    );

    // A write through the proxy (whose own connection also died with the old
    // daemon) must succeed and produce a change event in the recovered stream.
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"remember",
            "arguments":{ "content":"post-restart write", "importance":6 }
        }}),
    );
    let remembered = read_until_id(&rx, 2);
    assert_ne!(
        remembered["result"]["isError"],
        json!(true),
        "remember after daemon restart failed: {remembered}"
    );
    let remembered_id = {
        let text = remembered["result"]["content"][0]["text"]
            .as_str()
            .expect("remember tool text");
        let payload: Value = serde_json::from_str(text).unwrap();
        payload["id"].as_str().expect("remember id").to_string()
    };

    // The event for the post-restart write must arrive through poll_changes.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let id = next_id;
        next_id += 1;
        let payload = poll_changes(&mut stdin, &rx, id);
        let found = payload["events"]
            .as_array()
            .expect("events array")
            .iter()
            .any(|e| e["id"] == json!(remembered_id.as_str()) && e["kind"] == json!("Created"));
        if found {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "change event for {remembered_id} never arrived; last payload: {payload}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(stdin);
}
