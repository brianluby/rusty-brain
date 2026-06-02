//! JSON-RPC 2.0 envelope types for the MCP adapter.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// One incoming JSON-RPC request or notification. A request carries an `id`; a
/// notification omits it (and gets no response). A JSON `null` id deserializes
/// to `None`, so it is also treated as a notification.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    pub method: String,
    // serde maps an explicit JSON `null` to `None` for `Option<Value>`, so
    // `id: null` collapses to the same `None` as an absent `id`. This is safe
    // for MCP: per the MCP/JSON-RPC spec notifications OMIT `id`, and no
    // conformant MCP client sends a request with `id: null`, so treating both
    // as "notification" cannot misclassify a real request.
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// A request with no `id` is a notification and must NOT be answered.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// One outgoing JSON-RPC response: exactly one of `result` / `error` is set.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// A successful response carrying `result`.
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failed response carrying `error`.
    pub fn error(id: Value, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// Build an error with a code and message (no extra data).
    pub fn new(code: i64, message: String) -> Self {
        Self {
            code,
            message,
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use serde_json::json;

    #[test]
    fn request_with_id_round_trips() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(1)));
        assert!(req.params.is_some());
    }

    #[test]
    fn notification_has_no_id() {
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.method, "notifications/initialized");
        assert!(req.id.is_none(), "notification must have no id");
        assert!(req.is_notification());
    }

    #[test]
    fn explicit_null_id_is_treated_as_no_id() {
        // serde maps a JSON `null` for an `Option<Value>` field to `None`, so a
        // frame with `"id":null` is a notification, not a request with a null id.
        let raw = r#"{"jsonrpc":"2.0","id":null,"method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert!(req.id.is_none());
        assert!(req.is_notification());
    }

    #[test]
    fn success_response_serializes_result_not_error() {
        let resp = JsonRpcResponse::success(json!(7), json!({"ok": true}));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""), "{s}");
        assert!(s.contains("\"result\""), "{s}");
        assert!(!s.contains("\"error\""), "success must omit error: {s}");
        assert!(s.contains("\"id\":7"), "{s}");
    }

    #[test]
    fn error_response_serializes_error_not_result() {
        let resp = JsonRpcResponse::error(
            json!(7),
            JsonRpcError::new(METHOD_NOT_FOUND, "no such method".into()),
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"error\""), "{s}");
        assert!(!s.contains("\"result\""), "error must omit result: {s}");
        assert!(s.contains("-32601"), "method-not-found code present: {s}");
    }

    #[test]
    fn error_codes_match_jsonrpc_spec() {
        assert_eq!(PARSE_ERROR, -32700);
        assert_eq!(INVALID_REQUEST, -32600);
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(INVALID_PARAMS, -32602);
        assert_eq!(INTERNAL_ERROR, -32603);
    }
}
