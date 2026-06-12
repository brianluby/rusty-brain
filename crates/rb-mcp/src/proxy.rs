//! `DaemonProxy`: the seam between the MCP adapter and the daemon. The real
//! `rb_proto::Client` implements it (in the bin); tests inject an in-memory fake.
//! Plus the pure tool-call router (`build_request`) and response mapper.

use crate::jsonrpc::{JsonRpcError, INVALID_PARAMS, METHOD_NOT_FOUND};
use async_trait::async_trait;
use rb_proto::{Request, Response};
use rb_types::{MemoryId, MemoryType, MemoryUpdates};
use serde_json::{json, Value};
use std::str::FromStr;

/// The daemon-facing capability the adapter needs: send one `Request`, get one
/// `Response`. Implemented by `rb_proto::Client` (via a thin wrapper in the bin)
/// and by an in-memory fake in tests. Mirrors `Client::request`: a daemon-side
/// error is `Ok(Response::Error { .. })`; only transport failures are `Err`.
#[async_trait]
pub trait DaemonProxy: Send {
    /// Forward one request to the daemon and return its response.
    async fn call(&mut self, request: Request) -> rb_types::Result<Response>;
}

/// A tool-routing error already shaped as a JSON-RPC error (code + message).
pub type ToolError = JsonRpcError;

fn invalid(msg: impl Into<String>) -> ToolError {
    JsonRpcError::new(INVALID_PARAMS, msg.into())
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("missing required string argument '{key}'")))
}

fn opt_string(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(format!("'{key}' must be a string"))),
    }
}

/// Parse an optional array-of-strings argument. Absent or null yields an empty
/// vec; an array with any non-string element fails closed with `INVALID_PARAMS`
/// (the schema declares `items: { type: string }`, so partial coercion would be
/// silently lossy).
fn opt_string_vec(args: &Value, key: &str) -> Result<Vec<String>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(elems)) => elems
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(format!("'{key}' must be an array of strings")))
            })
            .collect(),
        Some(_) => Err(invalid(format!("'{key}' must be an array of strings"))),
    }
}

fn opt_u8(args: &Value, key: &str) -> Result<Option<u8>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| invalid(format!("'{key}' must be an integer in 0..=255"))),
    }
}

/// Parse an optional importance field (1..=10). Reuses `rb_types::validate_importance`
/// as the single source of truth for the valid range. `depth` must NOT use this;
/// use `opt_u8` instead (depth is a traversal depth, not an importance value).
fn opt_importance(args: &Value, key: &str) -> Result<Option<u8>, ToolError> {
    match opt_u8(args, key)? {
        None => Ok(None),
        Some(v) => {
            rb_types::validate_importance(v)
                .map_err(|_| invalid(format!("'{key}' must be in the range 1..=10")))?;
            Ok(Some(v))
        }
    }
}

const MAX_LIMIT: usize = 100;

fn opt_usize(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => match v.as_u64().and_then(|n| usize::try_from(n).ok()) {
            Some(n) if n <= MAX_LIMIT => Ok(n),
            Some(_) => Err(invalid(format!("'{key}' must be <= {MAX_LIMIT}"))),
            None => Err(invalid(format!("'{key}' must be a non-negative integer"))),
        },
    }
}

fn parse_type(args: &Value, key: &str) -> Result<Option<MemoryType>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => MemoryType::parse(s)
            .map(Some)
            .map_err(|e| invalid(format!("invalid '{key}': {e}"))),
        Some(_) => Err(invalid(format!("'{key}' must be a string"))),
    }
}

fn parse_id(args: &Value) -> Result<MemoryId, ToolError> {
    let raw = require_str(args, "id")?;
    MemoryId::from_str(raw).map_err(|e| invalid(format!("invalid memory id '{raw}': {e}")))
}

/// Route a `tools/call` (tool name + JSON arguments) to an `rb_proto::Request`.
/// Unknown tools fail with `METHOD_NOT_FOUND`; bad arguments with `INVALID_PARAMS`.
pub fn build_request(name: &str, args: &Value) -> Result<Request, ToolError> {
    match name {
        "remember" => {
            let content = require_str(args, "content")?.to_owned();
            let memory_type = parse_type(args, "type")?.unwrap_or(MemoryType::Insight);
            let importance = opt_importance(args, "importance")?.unwrap_or(5);
            Ok(Request::Remember {
                content,
                context: opt_string(args, "context")?,
                memory_type,
                importance,
                keywords: Vec::new(),
                tags: opt_string_vec(args, "tags")?,
                related_files: Vec::new(),
                // MCP-tool writes keep full trust (W0.5: only hook captures
                // declare a lower prior).
                confidence: 1.0,
            })
        }
        "recall" => Ok(Request::Recall {
            query: require_str(args, "query")?.to_owned(),
            memory_type: parse_type(args, "type")?,
            tags: opt_string_vec(args, "tags")?,
            limit: opt_usize(args, "limit", 10)?,
        }),
        "get" => Ok(Request::Get {
            id: parse_id(args)?,
        }),
        "list" => Ok(Request::List {
            min_importance: opt_importance(args, "min_importance")?,
            limit: opt_usize(args, "limit", 20)?,
        }),
        "graph" => {
            let depth = opt_u8(args, "depth")?.unwrap_or(1);
            Ok(Request::Graph {
                id: parse_id(args)?,
                depth,
            })
        }
        "update" => {
            let id = parse_id(args)?;
            // Tags absent -> leave unchanged (None); tags present -> validate the
            // whole array, failing closed on any non-string element.
            let tags = match args.get("tags") {
                None | Some(Value::Null) => None,
                Some(_) => Some(opt_string_vec(args, "tags")?),
            };
            let updates = MemoryUpdates {
                content: opt_string(args, "content")?,
                summary: opt_string(args, "summary")?,
                importance: opt_importance(args, "importance")?,
                tags,
                context: opt_string(args, "context")?,
            };
            Ok(Request::Update { id, updates })
        }
        "delete" => Ok(Request::Delete {
            id: parse_id(args)?,
        }),
        "context" => Ok(Request::Context),
        other => Err(JsonRpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown tool '{other}'"),
        )),
    }
}

/// Map a daemon `Response` to the JSON value embedded in an MCP tool result.
/// Domain types already derive `Serialize`, so this is a structural projection.
pub fn response_to_content(resp: Response) -> Value {
    match resp {
        Response::Remembered { id } => json!({ "id": id.to_string() }),
        Response::Recalled { results } => json!({ "results": results }),
        Response::Got { memory } => json!({ "memory": memory }),
        Response::Listed { memories } => json!({ "memories": memories }),
        Response::GraphResult { memories } => json!({ "memories": memories }),
        Response::Updated => json!({ "ok": true }),
        Response::Deleted => json!({ "ok": true }),
        Response::ContextResult {
            recent,
            important,
            total,
        } => json!({ "recent": recent, "important": important, "total": total }),
        Response::Pong { contract_version } => json!({ "contract_version": contract_version }),
        Response::JobRan {
            scanned,
            changed,
            skipped,
        } => json!({ "scanned": scanned, "changed": changed, "skipped": skipped }),
        Response::Error { kind, message } => {
            json!({ "error": { "kind": kind, "message": message } })
        }
        // Streamed subscribe frames (and the subscribe ack) never reach the
        // request/response proxy path; map them defensively to an error content
        // (unreachable in practice).
        Response::Change(_) | Response::Lagged { .. } | Response::SubscribeAck => json!({
            "error": {
                "kind": "protocol",
                "message": "unexpected streamed frame on a request/response call",
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_proto::Request;
    use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use serde_json::json;

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "one db one transaction".into(),
            MemoryType::ArchitectureDecision,
            8,
        )
    }

    #[test]
    fn build_remember_request_with_defaults() {
        let req = build_request("remember", &json!({ "content": "hello" })).unwrap();
        match req {
            Request::Remember {
                content,
                context,
                memory_type,
                importance,
                tags,
                ..
            } => {
                assert_eq!(content, "hello");
                assert!(context.is_none());
                assert_eq!(memory_type, MemoryType::Insight, "default type is insight");
                assert_eq!(importance, 5, "default importance is 5");
                assert!(tags.is_empty());
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn build_remember_request_with_all_fields() {
        let req = build_request(
            "remember",
            &json!({
                "content": "c",
                "context": "ctx",
                "type": "bug_fix",
                "importance": 9,
                "tags": ["a", "b"]
            }),
        )
        .unwrap();
        match req {
            Request::Remember {
                memory_type,
                importance,
                tags,
                context,
                ..
            } => {
                assert_eq!(memory_type, MemoryType::BugFix);
                assert_eq!(importance, 9);
                assert_eq!(tags, vec!["a".to_string(), "b".to_string()]);
                assert_eq!(context.as_deref(), Some("ctx"));
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn build_recall_request_maps_query_limit_type_tags() {
        let req = build_request(
            "recall",
            &json!({ "query": "q", "limit": 3, "type": "insight", "tags": ["t"] }),
        )
        .unwrap();
        match req {
            Request::Recall {
                query,
                memory_type,
                tags,
                limit,
            } => {
                assert_eq!(query, "q");
                assert_eq!(limit, 3);
                assert_eq!(memory_type, Some(MemoryType::Insight));
                assert_eq!(tags, vec!["t".to_string()]);
            }
            other => panic!("expected Recall, got {other:?}"),
        }
    }

    #[test]
    fn build_get_graph_delete_parse_ids() {
        let id = MemoryId::new();
        let g = build_request("get", &json!({ "id": id.to_string() })).unwrap();
        assert!(matches!(g, Request::Get { .. }));
        let gr = build_request("graph", &json!({ "id": id.to_string(), "depth": 2 })).unwrap();
        match gr {
            Request::Graph { depth, .. } => assert_eq!(depth, 2),
            other => panic!("expected Graph, got {other:?}"),
        }
        let d = build_request("delete", &json!({ "id": id.to_string() })).unwrap();
        assert!(matches!(d, Request::Delete { .. }));
    }

    #[test]
    fn build_update_maps_partial_fields() {
        let id = MemoryId::new();
        let u = build_request(
            "update",
            &json!({ "id": id.to_string(), "importance": 7, "tags": ["x"] }),
        )
        .unwrap();
        match u {
            Request::Update { updates, .. } => {
                assert_eq!(updates.importance, Some(7));
                assert_eq!(updates.tags, Some(vec!["x".to_string()]));
                assert!(updates.content.is_none());
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn build_context_and_list_defaults() {
        assert!(matches!(
            build_request("context", &json!({})).unwrap(),
            Request::Context
        ));
        match build_request("list", &json!({})).unwrap() {
            Request::List {
                min_importance,
                limit,
            } => {
                assert!(min_importance.is_none());
                assert_eq!(limit, 20, "default list limit is 20");
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_arg_is_invalid_params() {
        let err = build_request("remember", &json!({})).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
        let err = build_request("get", &json!({})).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn bad_id_and_bad_type_are_invalid_params() {
        let err = build_request("get", &json!({ "id": "not-a-uuid" })).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
        let err =
            build_request("remember", &json!({ "content": "c", "type": "nope" })).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn non_string_tag_element_is_invalid_params() {
        // The schema declares `tags: { items: { type: string } }`; an array with
        // any non-string element is malformed and must fail closed (no partial
        // store), across every tool that accepts tags.
        for tool in ["remember", "recall", "update"] {
            let args = match tool {
                "remember" => json!({ "content": "c", "tags": [1, "x"] }),
                "recall" => json!({ "query": "q", "tags": [1, "x"] }),
                _ => json!({ "id": MemoryId::new().to_string(), "tags": [1, "x"] }),
            };
            let err = build_request(tool, &args).unwrap_err();
            assert_eq!(
                err.code,
                crate::jsonrpc::INVALID_PARAMS,
                "{tool} must reject non-string tag elements"
            );
        }
        // A valid all-string array still works.
        let req = build_request("recall", &json!({ "query": "q", "tags": ["a", "b"] })).unwrap();
        match req {
            Request::Recall { tags, .. } => {
                assert_eq!(tags, vec!["a".to_string(), "b".to_string()]);
            }
            other => panic!("expected Recall, got {other:?}"),
        }
    }

    #[test]
    fn non_string_type_arg_is_invalid_params() {
        // A present-but-non-string `type` (e.g. an integer or boolean) must fail
        // closed with INVALID_PARAMS, not silently degrade to `None` / a default.
        for (tool, args) in [
            ("remember", json!({ "content": "c", "type": 123 })),
            ("remember", json!({ "content": "c", "type": true })),
            ("recall", json!({ "query": "q", "type": 123 })),
        ] {
            let err = build_request(tool, &args).unwrap_err();
            assert_eq!(
                err.code,
                crate::jsonrpc::INVALID_PARAMS,
                "{tool} with non-string 'type' must be INVALID_PARAMS (args={args})"
            );
        }
        // A valid string type still succeeds.
        let ok = build_request("remember", &json!({ "content": "c", "type": "insight" }));
        assert!(ok.is_ok(), "valid string type must succeed");
        // Absent type yields Ok (defaults applied in callers).
        let ok = build_request("remember", &json!({ "content": "c" }));
        assert!(ok.is_ok(), "absent type must succeed");
    }

    #[test]
    fn non_string_context_or_summary_is_invalid_params() {
        // opt_string must fail closed on a non-string value, not silently drop it.
        for (tool, args) in [
            ("remember", json!({ "content": "c", "context": false })),
            ("remember", json!({ "content": "c", "context": {} })),
            (
                "update",
                json!({ "id": MemoryId::new().to_string(), "summary": 123 }),
            ),
            (
                "update",
                json!({ "id": MemoryId::new().to_string(), "context": [] }),
            ),
        ] {
            let err = build_request(tool, &args).unwrap_err();
            assert_eq!(
                err.code,
                crate::jsonrpc::INVALID_PARAMS,
                "{tool} with non-string optional string field must be INVALID_PARAMS (args={args})"
            );
        }
        // A valid string context still works.
        let ok = build_request(
            "remember",
            &json!({ "content": "c", "context": "some context" }),
        );
        assert!(ok.is_ok(), "valid string context must succeed");
        // Absent context is fine.
        let ok = build_request("remember", &json!({ "content": "c" }));
        assert!(ok.is_ok(), "absent context must succeed");
    }

    #[test]
    fn out_of_range_importance_is_invalid_params() {
        // importance must be 1..=10; 0, 42, 255 must all fail with INVALID_PARAMS.
        for (tool, args) in [
            ("remember", json!({ "content": "c", "importance": 0 })),
            ("remember", json!({ "content": "c", "importance": 42 })),
            (
                "update",
                json!({ "id": MemoryId::new().to_string(), "importance": 255 }),
            ),
            ("list", json!({ "min_importance": 0 })),
            ("list", json!({ "min_importance": 255 })),
        ] {
            let err = build_request(tool, &args).unwrap_err();
            assert_eq!(
                err.code,
                crate::jsonrpc::INVALID_PARAMS,
                "{tool} with out-of-range importance must be INVALID_PARAMS (args={args})"
            );
        }
        // Valid importance is fine.
        let ok = build_request("remember", &json!({ "content": "c", "importance": 5 }));
        assert!(ok.is_ok(), "valid importance 5 must succeed");
        let ok = build_request("remember", &json!({ "content": "c", "importance": 1 }));
        assert!(ok.is_ok(), "valid importance 1 must succeed");
        let ok = build_request("remember", &json!({ "content": "c", "importance": 10 }));
        assert!(ok.is_ok(), "valid importance 10 must succeed");
        // graph{depth:3} must still work (depth uses opt_u8, not opt_importance).
        let ok = build_request(
            "graph",
            &json!({ "id": MemoryId::new().to_string(), "depth": 3 }),
        );
        assert!(
            ok.is_ok(),
            "graph depth=3 must succeed (not clamped to 1..=10)"
        );
    }

    #[test]
    fn limit_above_max_is_invalid_params() {
        // A hostile client requesting >100 results must be rejected.
        for (tool, args) in [
            ("recall", json!({ "query": "q", "limit": 1_000_000u64 })),
            ("list", json!({ "limit": 101 })),
        ] {
            let err = build_request(tool, &args).unwrap_err();
            assert_eq!(
                err.code,
                crate::jsonrpc::INVALID_PARAMS,
                "{tool} with limit > MAX_LIMIT must be INVALID_PARAMS (args={args})"
            );
        }
        // limit at the boundary is fine.
        let ok = build_request("recall", &json!({ "query": "q", "limit": 50 }));
        assert!(ok.is_ok(), "limit=50 must succeed");
        let ok = build_request("recall", &json!({ "query": "q", "limit": 100 }));
        assert!(ok.is_ok(), "limit=100 (MAX_LIMIT) must succeed");
        // absent limit uses the default.
        let ok = build_request("recall", &json!({ "query": "q" }));
        assert!(ok.is_ok(), "absent limit must use default");
    }

    #[test]
    fn unknown_tool_is_method_not_found() {
        let err = build_request("frobnicate", &json!({})).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::METHOD_NOT_FOUND);
    }

    #[test]
    fn response_to_content_renders_each_variant_as_json() {
        use rb_proto::Response;
        let id = MemoryId::new();
        let remembered = response_to_content(Response::Remembered { id: id.clone() });
        assert_eq!(remembered["id"], id.to_string());

        let recalled = response_to_content(Response::Recalled {
            results: vec![SearchResult {
                memory: note(),
                score: 0.5,
            }],
        });
        assert!(recalled["results"].is_array());
        assert_eq!(recalled["results"][0]["score"], 0.5);

        let got = response_to_content(Response::Got {
            memory: Some(note()),
        });
        assert!(got["memory"]["content"].is_string());

        let none = response_to_content(Response::Got { memory: None });
        assert!(none["memory"].is_null());

        let ctx = response_to_content(Response::ContextResult {
            recent: vec![note()],
            important: vec![note()],
            total: 2,
        });
        assert_eq!(ctx["total"], 2);

        let err = response_to_content(Response::Error {
            kind: "not_found".into(),
            message: "nope".into(),
        });
        assert!(
            err.get("error").is_some(),
            "error variant carries an `error` key"
        );
    }

    #[test]
    fn recall_result_carries_contested_flag() {
        use rb_proto::Response;
        // Feature C: the additive `contested` boolean surfaces in the MCP result
        // schema for each recall row.
        let mut contested = note();
        contested.contested = true;
        let recalled = response_to_content(Response::Recalled {
            results: vec![SearchResult {
                memory: contested,
                score: 0.9,
            }],
        });
        assert_eq!(recalled["results"][0]["memory"]["contested"], true);
    }
}
