//! MCP method dispatch: one decoded JSON-RPC request -> optional response.

use crate::jsonrpc::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use crate::proxy::{build_request, response_to_content, DaemonProxy};
use crate::tools::tool_definitions;
use serde_json::{json, Value};

/// The MCP protocol revision this adapter targets when the client omits one.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Handle one decoded JSON-RPC request. Returns `Some(response)` for requests and
/// `None` for notifications (which JSON-RPC forbids answering).
pub async fn handle_request(
    request: JsonRpcRequest,
    proxy: &mut dyn DaemonProxy,
) -> Option<JsonRpcResponse> {
    // Notifications (no id) are acknowledged silently with no response frame.
    if request.is_notification() {
        // `notifications/initialized` and any other notification: nothing to send.
        tracing::debug!(method = %request.method, "notification (no response)");
        return None;
    }

    // Safe: non-notification means id is Some.
    let id = request.id.clone().unwrap_or(Value::Null);
    let params = request.params.clone().unwrap_or_else(|| json!({}));

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::success(id, initialize_result(&params)),
        "tools/list" => JsonRpcResponse::success(id, tools_list_result()),
        "tools/call" => handle_tools_call(id, &params, proxy).await,
        other => JsonRpcResponse::error(
            id,
            JsonRpcError::new(METHOD_NOT_FOUND, format!("unknown method '{other}'")),
        ),
    };
    Some(response)
}

/// Build the `initialize` result: echo the client's protocolVersion (or the
/// default), advertise the tools capability, and surface server identity +
/// the rusty-brain wire contract version.
fn initialize_result(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "rusty-brain",
            "version": env!("CARGO_PKG_VERSION"),
            "contractVersion": rb_proto::CONTRACT_VERSION
        }
    })
}

/// Build the `tools/list` result from the static tool definitions.
fn tools_list_result() -> Value {
    json!({ "tools": tool_definitions() })
}

/// Handle `tools/call`: route name+arguments to a `Request`, forward via the
/// proxy, and wrap the response. Routing errors (unknown tool, bad args) become
/// JSON-RPC errors; daemon-reported errors become `isError` tool results.
async fn handle_tools_call(
    id: Value,
    params: &Value,
    proxy: &mut dyn DaemonProxy,
) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::error(
            id,
            JsonRpcError::new(INVALID_PARAMS, "tools/call requires a 'name'".into()),
        );
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let request = match build_request(name, &arguments) {
        Ok(r) => r,
        Err(err) => return JsonRpcResponse::error(id, err),
    };

    match proxy.call(request).await {
        Ok(resp) => {
            let content = response_to_content(resp);
            // A daemon-side error surfaces as a tool result with isError=true so
            // the agent sees the message instead of a transport failure.
            let is_error = content.get("error").is_some();
            JsonRpcResponse::success(id, tool_result(content, is_error))
        }
        Err(e) => JsonRpcResponse::error(
            id,
            // A transport/daemon failure (socket dropped, etc.) is a real
            // JSON-RPC error. The message is the sanitized domain error string.
            JsonRpcError::new(INTERNAL_ERROR, format!("daemon call failed: {e}")),
        ),
    }
}

/// Wrap a JSON payload as an MCP tool result (a single JSON text content item).
fn tool_result(content: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&content)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::jsonrpc::JsonRpcRequest;
    use crate::proxy::DaemonProxy;
    use async_trait::async_trait;
    use rb_proto::{Request, Response};
    use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use serde_json::json;

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("p".into()),
            "remembered body".into(),
            MemoryType::Insight,
            6,
        )
    }

    /// A fake proxy that records the last request and returns a canned response
    /// per request kind, so the dispatcher is tested without a daemon.
    struct FakeProxy {
        id: MemoryId,
        last: Option<Request>,
        force_error: bool,
    }

    #[async_trait]
    impl DaemonProxy for FakeProxy {
        async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
            self.last = Some(request.clone());
            if self.force_error {
                return Ok(Response::Error {
                    kind: "not_found".into(),
                    message: "no such memory".into(),
                });
            }
            Ok(match request {
                Request::Remember { .. } => Response::Remembered {
                    id: self.id.clone(),
                },
                Request::Recall { .. } => Response::Recalled {
                    results: vec![SearchResult {
                        memory: note(),
                        score: 0.9,
                    }],
                },
                Request::Get { .. } => Response::Got {
                    memory: Some(note()),
                },
                Request::List { .. } => Response::Listed {
                    memories: vec![note()],
                },
                Request::Graph { .. } => Response::GraphResult {
                    memories: vec![note()],
                },
                Request::Update { .. } => Response::Updated,
                Request::Delete { .. } => Response::Deleted,
                Request::Context => Response::ContextResult {
                    recent: vec![note()],
                    important: vec![note()],
                    total: 1,
                },
                Request::Ping => Response::Pong {
                    contract_version: 1,
                },
            })
        }
    }

    fn fake() -> FakeProxy {
        FakeProxy {
            id: MemoryId::new(),
            last: None,
            force_error: false,
        }
    }

    fn req(method: &str, id: Option<i64>, params: serde_json::Value) -> JsonRpcRequest {
        let raw = json!({
            "jsonrpc": "2.0",
            "method": method,
            "id": id,
            "params": params
        });
        serde_json::from_value(raw).unwrap()
    }

    #[tokio::test]
    async fn initialize_returns_server_info_and_capabilities() {
        let mut proxy = fake();
        let r = req(
            "initialize",
            Some(1),
            json!({ "protocolVersion": "2024-11-05" }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "rusty-brain");
        assert!(result["serverInfo"]["version"].is_string());
        assert!(result["capabilities"]["tools"].is_object());
        // Echoes the client's requested protocol version.
        assert_eq!(result["protocolVersion"], "2024-11-05");
        // Surfaces the rusty-brain wire contract version (a u32; serde_json
        // Value implements PartialEq<u32>).
        assert_eq!(
            result["serverInfo"]["contractVersion"],
            rb_proto::CONTRACT_VERSION
        );
    }

    #[tokio::test]
    async fn initialized_notification_gets_no_response() {
        let mut proxy = fake();
        let r = req("notifications/initialized", None, json!({}));
        let resp = handle_request(r, &mut proxy).await;
        assert!(resp.is_none(), "notifications must not be answered");
    }

    #[tokio::test]
    async fn tools_list_returns_eight_tools() {
        let mut proxy = fake();
        let r = req("tools/list", Some(2), json!({}));
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 8);
        assert!(tools.iter().any(|t| t["name"] == "remember"));
        assert!(tools[0]["inputSchema"]["type"] == "object");
    }

    #[tokio::test]
    async fn tools_call_remember_forwards_and_wraps_result() {
        let mut proxy = fake();
        let want_id = proxy.id.clone();
        let r = req(
            "tools/call",
            Some(3),
            json!({ "name": "remember", "arguments": { "content": "hi" } }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let result = resp.result.unwrap();
        // MCP tool result: content array with a text item, isError false/absent.
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains(&want_id.to_string()),
            "id in result text: {text}"
        );
        assert_ne!(result["isError"], json!(true));
        assert!(matches!(proxy.last, Some(Request::Remember { .. })));
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_is_method_not_found() {
        let mut proxy = fake();
        let r = req(
            "tools/call",
            Some(4),
            json!({ "name": "frobnicate", "arguments": {} }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, crate::jsonrpc::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn tools_call_bad_arguments_is_invalid_params() {
        let mut proxy = fake();
        let r = req(
            "tools/call",
            Some(5),
            json!({ "name": "get", "arguments": { "id": "not-a-uuid" } }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        assert_eq!(resp.error.unwrap().code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn tools_call_daemon_error_becomes_iserror_tool_result() {
        let mut proxy = fake();
        proxy.force_error = true;
        let r = req(
            "tools/call",
            Some(6),
            json!({ "name": "get", "arguments": { "id": MemoryId::new().to_string() } }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(
            result["isError"],
            json!(true),
            "daemon error -> isError result"
        );
        // The transport itself stays successful (result, not error).
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let mut proxy = fake();
        let r = req("does/not/exist", Some(7), json!({}));
        let resp = handle_request(r, &mut proxy).await.unwrap();
        assert_eq!(resp.error.unwrap().code, crate::jsonrpc::METHOD_NOT_FOUND);
    }
}
