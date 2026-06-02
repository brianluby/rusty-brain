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
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

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
