//! Newline-delimited JSON-RPC transport over generic async byte streams.
//!
//! Generic over the reader/writer/proxy so the same loop drives an in-memory
//! duplex pair (contract tests) and real stdin/stdout (production). stdout
//! receives ONLY response frames; all logging goes to stderr via `tracing`.

use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use crate::proxy::DaemonProxy;
use crate::server::handle_request;
use rb_types::{Error, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Serve MCP over a line-delimited byte stream until EOF on `reader`.
///
/// Each input line is parsed as a `JsonRpcRequest`; requests are dispatched and
/// their responses written as one `\n`-terminated JSON line. Notifications get
/// no output. A line that fails to parse yields a JSON-RPC parse error (null id)
/// and the loop continues — one bad frame never tears down the session.
pub async fn serve_stdio<R, W, P>(mut reader: R, mut writer: W, mut proxy: P) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| Error::Io(format!("mcp stdin read: {e}")))?;
        if n == 0 {
            // EOF: the client closed stdin; shut the adapter down cleanly.
            tracing::debug!("mcp stdin closed; shutting down adapter");
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(request) => handle_request(request, &mut proxy).await,
            Err(e) => {
                tracing::warn!(error = %e, "malformed JSON-RPC frame");
                Some(JsonRpcResponse::error(
                    Value::Null,
                    JsonRpcError::new(PARSE_ERROR, format!("parse error: {e}")),
                ))
            }
        };

        if let Some(response) = response {
            write_response(&mut writer, &response).await?;
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

        // tools/list (id 2)
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 8);

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
}
