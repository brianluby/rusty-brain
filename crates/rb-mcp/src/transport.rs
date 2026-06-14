//! Newline-delimited JSON-RPC transport over generic async byte streams.
//!
//! Generic over the reader/writer/proxy so the same loop drives an in-memory
//! duplex pair (contract tests) and real stdin/stdout (production). stdout
//! receives ONLY response frames; all logging goes to stderr via `tracing`.

use crate::change_buffer::ChangeBuffer;
use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use crate::proxy::DaemonProxy;
use crate::server::{handle_request, handle_request_with_buffer};
use rb_types::{Error, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

/// Maximum bytes accepted for a single newline-delimited JSON-RPC frame. A
/// client sending more than this before a `\n` is treated as a parse error (the
/// over-long bytes are discarded up to the next newline so the session resyncs),
/// rather than letting the line buffer grow without bound (a memory-exhaustion
/// DoS). 8 MiB comfortably exceeds any legitimate MCP frame.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Outcome of reading one capped line from the input stream.
enum LineRead {
    /// EOF reached with no further data.
    Eof,
    /// A complete line (newline stripped) within the byte cap.
    Line(String),
    /// A line exceeded `MAX_LINE_BYTES`; the remainder was discarded up to the
    /// next newline (or EOF) so the loop can resync to the following frame.
    TooLong,
}

/// Serve MCP over a line-delimited byte stream until EOF on `reader`, WITHOUT a
/// change buffer (`poll_changes` returns empty). See [`serve_stdio_with_buffer`]
/// for the polling-enabled variant.
pub async fn serve_stdio<R, W, P>(reader: R, writer: W, proxy: P) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    serve_loop(reader, writer, proxy, None).await
}

/// Serve MCP with a shared change buffer that `poll_changes` drains. The caller
/// is expected to run a background subscriber that pushes into the same buffer.
pub async fn serve_stdio_with_buffer<R, W, P>(
    reader: R,
    writer: W,
    proxy: P,
    buffer: Arc<Mutex<ChangeBuffer>>,
) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    serve_loop(reader, writer, proxy, Some(buffer)).await
}

/// Shared serve loop. `buffer` selects buffer-aware dispatch for `poll_changes`.
async fn serve_loop<R, W, P>(
    mut reader: R,
    mut writer: W,
    mut proxy: P,
    buffer: Option<Arc<Mutex<ChangeBuffer>>>,
) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    loop {
        let response = match read_capped_line(&mut reader).await? {
            LineRead::Eof => {
                tracing::debug!("mcp stdin closed; shutting down adapter");
                return Ok(());
            }
            LineRead::TooLong => {
                tracing::warn!(
                    max = MAX_LINE_BYTES,
                    "JSON-RPC frame exceeded byte cap; rejecting"
                );
                Some(JsonRpcResponse::error(
                    Value::Null,
                    JsonRpcError::new(
                        PARSE_ERROR,
                        format!("parse error: frame exceeds {MAX_LINE_BYTES} byte limit"),
                    ),
                ))
            }
            LineRead::Line(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcRequest>(trimmed) {
                    Ok(request) => match buffer.as_ref() {
                        Some(buf) => handle_request_with_buffer(request, &mut proxy, buf).await,
                        None => handle_request(request, &mut proxy).await,
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "malformed JSON-RPC frame");
                        Some(JsonRpcResponse::error(
                            Value::Null,
                            JsonRpcError::new(PARSE_ERROR, format!("parse error: {e}")),
                        ))
                    }
                }
            }
        };

        if let Some(response) = response {
            write_response(&mut writer, &response).await?;
        }
    }
}

/// Read one newline-delimited line, capped at `MAX_LINE_BYTES`. Reads through
/// the `AsyncBufReadExt` buffer (`fill_buf`/`consume`) so it never allocates more
/// than the cap: once the accumulated bytes would exceed the cap, it stops
/// buffering and discards the rest of the over-long line up to the next newline
/// (or EOF), then reports `TooLong` so the caller can emit a parse error and keep
/// serving the next frame.
async fn read_capped_line<R>(reader: &mut R) -> Result<LineRead>
where
    R: AsyncBufReadExt + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    let mut overflowed = false;
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|e| Error::Io(format!("mcp stdin read: {e}")))?;
        if available.is_empty() {
            // EOF. If we already accumulated bytes (or overflowed), surface the
            // partial line; otherwise it is a clean end of stream.
            if overflowed {
                return Ok(LineRead::TooLong);
            }
            if buf.is_empty() {
                return Ok(LineRead::Eof);
            }
            // Final line without a trailing newline.
            let line = String::from_utf8_lossy(&buf).into_owned();
            return Ok(LineRead::Line(line));
        }

        match available.iter().position(|&b| b == b'\n') {
            Some(idx) => {
                if !overflowed {
                    let remaining = MAX_LINE_BYTES.saturating_sub(buf.len());
                    if idx > remaining {
                        overflowed = true;
                    } else {
                        buf.extend_from_slice(&available[..idx]);
                    }
                }
                // Consume through the newline so the next read starts a fresh line.
                reader.consume(idx + 1);
                if overflowed {
                    return Ok(LineRead::TooLong);
                }
                let line = String::from_utf8_lossy(&buf).into_owned();
                return Ok(LineRead::Line(line));
            }
            None => {
                let len = available.len();
                if !overflowed {
                    let remaining = MAX_LINE_BYTES.saturating_sub(buf.len());
                    if len > remaining {
                        // Keep what fits up to the cap, then stop buffering and
                        // switch to discard-until-newline mode.
                        buf.extend_from_slice(&available[..remaining]);
                        overflowed = true;
                    } else {
                        buf.extend_from_slice(available);
                    }
                }
                reader.consume(len);
            }
        }
    }
}

/// Serialize one response and write it as a single `\n`-terminated line.
async fn write_response<W>(writer: &mut W, response: &JsonRpcResponse) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(response)
        .map_err(|e| Error::Serialization(format!("mcp response serialize: {e}")))?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| Error::Io(format!("mcp stdout write: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| Error::Io(format!("mcp stdout flush: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::proxy::DaemonProxy;
    use async_trait::async_trait;
    use rb_proto::{Request, Response};
    use rb_types::MemoryId;
    use serde_json::{json, Value};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Minimal fake daemon: remembers return a fixed id; everything else Pongs.
    struct Fake {
        id: MemoryId,
    }
    #[async_trait]
    impl DaemonProxy for Fake {
        async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
            Ok(match request {
                Request::Remember { .. } => Response::Remembered {
                    id: self.id.clone(),
                },
                _ => Response::Pong {
                    contract_version: rb_proto::CONTRACT_VERSION,
                    recall_channels: None,
                },
            })
        }
    }

    /// Drive the adapter end-to-end over an in-memory duplex pair: the test plays
    /// the MCP client (writes requests, reads response lines); `serve_stdio` is
    /// the server. Asserts initialize, the no-reply notification, tools/list, and
    /// a tools/call round-trip all behave per JSON-RPC over the byte stream.
    #[tokio::test]
    async fn full_stdio_contract_round_trip() {
        // client_* is the test's end; server_* is fed to serve_stdio.
        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);

        let fixed = MemoryId::new();
        let proxy = Fake { id: fixed.clone() };

        let server = tokio::spawn(async move {
            let reader = BufReader::new(server_reader);
            serve_stdio(reader, server_writer, proxy).await
        });

        // Write four frames: initialize, initialized (notification), tools/list,
        // tools/call(remember). Then close the write half to end the loop.
        let mut to_server = client_to_server;
        let frames = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                   "params":{"protocolVersion":"2024-11-05"}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                   "params":{"name":"remember","arguments":{"content":"hi"}}}),
        ];
        for f in frames {
            let line = format!("{}\n", serde_json::to_string(&f).unwrap());
            to_server.write_all(line.as_bytes()).await.unwrap();
        }
        to_server.flush().await.unwrap();
        drop(to_server); // EOF -> serve_stdio returns

        // Read every response line the server produced.
        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.trim().is_empty() {
                continue;
            }
            responses.push(serde_json::from_str(&line).unwrap());
        }

        // Exactly three responses: the notification produced none.
        assert_eq!(responses.len(), 3, "got: {responses:?}");

        // initialize (id 1)
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "rusty-brain");

        // tools/list (id 2) — the DEFAULT advertised set is 6 tools
        // (remember/recall/get/context/update + memory_feedback, W3.7); the
        // rest are behind RB_MCP_FULL_TOOLSET.
        assert_eq!(responses[1]["id"], 2);
        let expected_tools = if std::env::var_os("RB_MCP_FULL_TOOLSET").is_some() {
            11
        } else {
            6
        };
        assert_eq!(
            responses[1]["result"]["tools"].as_array().unwrap().len(),
            expected_tools
        );

        // tools/call (id 3) -> remembered id appears in the tool result text
        assert_eq!(responses[2]["id"], 3);
        let text = responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains(&fixed.to_string()), "id in result: {text}");

        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn malformed_line_yields_parse_error_and_keeps_serving() {
        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);
        let proxy = Fake {
            id: MemoryId::new(),
        };

        let server = tokio::spawn(async move {
            serve_stdio(BufReader::new(server_reader), server_writer, proxy).await
        });

        let mut to_server = client_to_server;
        // 1) garbage line, then 2) a valid tools/list to prove the loop survived.
        to_server.write_all(b"this is not json\n").await.unwrap();
        let good = json!({"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}});
        to_server
            .write_all(format!("{}\n", serde_json::to_string(&good).unwrap()).as_bytes())
            .await
            .unwrap();
        to_server.flush().await.unwrap();
        drop(to_server);

        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if !line.trim().is_empty() {
                responses.push(serde_json::from_str(&line).unwrap());
            }
        }
        assert_eq!(
            responses.len(),
            2,
            "parse error + tools/list reply: {responses:?}"
        );
        assert_eq!(responses[0]["error"]["code"], crate::jsonrpc::PARSE_ERROR);
        assert!(responses[0]["id"].is_null(), "parse error has null id");
        assert_eq!(responses[1]["id"], 9);
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn over_long_line_yields_parse_error_and_keeps_serving() {
        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);
        let proxy = Fake {
            id: MemoryId::new(),
        };

        let server = tokio::spawn(async move {
            serve_stdio(BufReader::new(server_reader), server_writer, proxy).await
        });

        // 1) A line larger than the cap with NO newline until well past the limit:
        // it must be rejected (PARSE_ERROR) rather than buffered unbounded. We then
        // 2) send a valid frame to prove the loop resynced and keeps serving.
        let mut to_server = client_to_server;
        let writer = tokio::spawn(async move {
            // Garbage bytes far exceeding MAX_LINE_BYTES, then a newline.
            let chunk = vec![b'a'; 64 * 1024];
            let mut written = 0usize;
            while written <= MAX_LINE_BYTES + 64 * 1024 {
                to_server.write_all(&chunk).await.unwrap();
                written += chunk.len();
            }
            to_server.write_all(b"\n").await.unwrap();
            // A valid frame after the giant line.
            let good = json!({"jsonrpc":"2.0","id":11,"method":"tools/list","params":{}});
            to_server
                .write_all(format!("{}\n", serde_json::to_string(&good).unwrap()).as_bytes())
                .await
                .unwrap();
            to_server.flush().await.unwrap();
            drop(to_server);
        });

        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if !line.trim().is_empty() {
                responses.push(serde_json::from_str(&line).unwrap());
            }
        }
        writer.await.unwrap();

        // The over-long line produced an error response; the valid frame still got
        // its correct reply (proving the session survived and resynced).
        assert_eq!(
            responses.len(),
            2,
            "over-long error + tools/list reply: {responses:?}"
        );
        assert!(
            responses[0]["error"].is_object(),
            "over-long line must yield an error response: {responses:?}"
        );
        assert!(
            responses[0]["id"].is_null(),
            "error for over-long line has null id"
        );
        assert_eq!(responses[1]["id"], 11);
        // Default advertised toolset is 6 (RB_MCP_FULL_TOOLSET gates the rest).
        let expected_tools = if std::env::var_os("RB_MCP_FULL_TOOLSET").is_some() {
            11
        } else {
            6
        };
        assert_eq!(
            responses[1]["result"]["tools"].as_array().unwrap().len(),
            expected_tools
        );
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn poll_changes_over_stdio_drains_seeded_buffer() {
        use crate::change_buffer::ChangeBuffer;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);
        let proxy = Fake {
            id: MemoryId::new(),
        };

        let buffer = Arc::new(Mutex::new(ChangeBuffer::new(16)));
        {
            let mut guard = buffer.lock().await;
            guard.push(MemoryChanged {
                id: MemoryId::new(),
                namespace: Namespace::Project("p".into()),
                kind: ChangeKind::Created,
                seq: None,
            });
            guard.record_dropped(4);
        }

        let server_buffer = Arc::clone(&buffer);
        let server = tokio::spawn(async move {
            serve_stdio_with_buffer(
                BufReader::new(server_reader),
                server_writer,
                proxy,
                server_buffer,
            )
            .await
        });

        let mut to_server = client_to_server;
        let frame = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "poll_changes", "arguments": { "max": 10 } }
        });
        to_server
            .write_all(format!("{}\n", serde_json::to_string(&frame).unwrap()).as_bytes())
            .await
            .unwrap();
        to_server.flush().await.unwrap();
        drop(to_server);

        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if !line.trim().is_empty() {
                responses.push(serde_json::from_str(&line).unwrap());
            }
        }
        assert_eq!(responses.len(), 1, "one poll_changes reply: {responses:?}");
        assert_eq!(responses[0]["id"], 1);
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["events"].as_array().unwrap().len(), 1);
        assert_eq!(payload["dropped"], 4);
        assert_ne!(responses[0]["result"]["isError"], json!(true));

        server.await.unwrap().unwrap();
    }
}
