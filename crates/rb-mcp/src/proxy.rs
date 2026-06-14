//! `DaemonProxy`: the seam between the MCP adapter and the daemon. The real
//! `rb_proto::Client` implements it (in the bin); tests inject an in-memory fake.
//! Plus the pure tool-call router (`build_request`) and response mapper.

use crate::jsonrpc::{JsonRpcError, INVALID_PARAMS, METHOD_NOT_FOUND};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rb_proto::{Request, Response};
use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates};
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

/// Optional confidence argument: a JSON number validated against the
/// canonical 0.0..=1.0 range (`rb_types::validate_confidence` is the single
/// source of truth, mirroring `opt_importance`).
fn opt_confidence(args: &Value, key: &str) -> Result<Option<f32>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let v = n
                .as_f64()
                .ok_or_else(|| invalid(format!("'{key}' must be a number")))?
                as f32;
            rb_types::validate_confidence(v)
                .map_err(|_| invalid(format!("'{key}' must be in the range 0.0..=1.0")))?;
            Ok(Some(v))
        }
        Some(_) => Err(invalid(format!("'{key}' must be a number"))),
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
                // MCP-tool writes declare no explicit prior (W0.5: only hook
                // captures declare a lower one). `None` keeps the full-trust
                // 1.0 baseline and lets an enricher fill it (fix #4).
                confidence: None,
                // The MCP `remember` tool stores plainly; update-as-supersede
                // (W3.1) is driven by the hook capture path, not this tool.
                supersedes: None,
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
                confidence: opt_confidence(args, "confidence")?,
            };
            Ok(Request::Update { id, updates })
        }
        "link" => {
            let from_raw = require_str(args, "from")?;
            let from = MemoryId::from_str(from_raw)
                .map_err(|e| invalid(format!("invalid memory id '{from_raw}': {e}")))?;
            let to_raw = require_str(args, "to")?;
            let to = MemoryId::from_str(to_raw)
                .map_err(|e| invalid(format!("invalid memory id '{to_raw}': {e}")))?;
            let link_type_raw = require_str(args, "type")?;
            let link_type = rb_types::LinkType::parse(link_type_raw)
                .map_err(|e| invalid(format!("invalid link type '{link_type_raw}': {e}")))?;
            // Reject the one type the engine rejects (and which the schema enum
            // already omits) at the boundary, so a hand-crafted call fails with
            // INVALID_PARAMS instead of a round-trip error.
            if link_type == rb_types::LinkType::Supersedes {
                return Err(invalid(
                    "supersedes links are created by storing a replacement memory, not by linking"
                        .to_string(),
                ));
            }
            Ok(Request::Link {
                from,
                to,
                link_type,
                reason: opt_string(args, "reason")?,
            })
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

/// A projected MCP tool result (W3.3 token economy): compact markdown `text`
/// the model reads, the full `structured` JSON exposed separately as optional
/// `structuredContent`, and the daemon-error flag. Recall/list/context render as
/// one compact line per memory; everything else keeps the full JSON as `text`.
pub struct ToolContent {
    pub text: String,
    pub structured: Value,
    pub is_error: bool,
}

impl ToolContent {
    /// A result whose model-facing `text` is just the structured JSON
    /// stringified — for tools with no compact markdown projection (e.g.
    /// `poll_changes`, `get`, acks).
    #[must_use]
    pub fn json(structured: Value, is_error: bool) -> Self {
        let text = serde_json::to_string(&structured)
            .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));
        Self {
            text,
            structured,
            is_error,
        }
    }
}

/// Compact relative age for a memory line, e.g. `just now` / `5m ago` / `3d ago`
/// / `2w ago` / `4mo ago` / `1y ago`. Negative deltas (clock skew) read as
/// `just now`.
fn relative_age(created_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - created_at).num_seconds().max(0);
    let (n, unit) = if secs < 60 {
        return "just now".to_string();
    } else if secs < 3600 {
        (secs / 60, "m")
    } else if secs < 86_400 {
        (secs / 3600, "h")
    } else if secs < 604_800 {
        (secs / 86_400, "d")
    } else if secs < 2_592_000 {
        (secs / 604_800, "w")
    } else if secs < 31_536_000 {
        (secs / 2_592_000, "mo")
    } else {
        (secs / 31_536_000, "y")
    };
    format!("{n}{unit} ago")
}

/// Truncate `s` to at most `max` chars on a char boundary, appending `…` when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// One compact markdown line for a memory (W3.3):
/// `[type, imp N, Xd ago] summary-or-first-200-chars (id <uuid>)` + ` ⚠ contested`.
/// Newlines/control chars in the memory text are flattened to spaces so stored
/// content cannot inject fake markdown structure into the result. The full id is
/// kept (not a 6-char prefix) so the model can `get`/`update`/`link` the result;
/// `structuredContent` carries it too.
fn memory_md_line(m: &MemoryNote, now: DateTime<Utc>) -> String {
    let raw = if m.summary.trim().is_empty() {
        m.content.trim()
    } else {
        m.summary.trim()
    };
    let body: String = truncate_chars(raw, 200)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let contested = if m.contested { " ⚠ contested" } else { "" };
    format!(
        "[{}, imp {}, {}] {} (id {}){}",
        m.memory_type.as_str(),
        m.importance,
        relative_age(m.created_at, now),
        body,
        m.id,
        contested,
    )
}

/// Render memories as a numbered compact markdown list (one [`memory_md_line`]
/// per entry).
fn render_indexed<'a>(
    memories: impl IntoIterator<Item = &'a MemoryNote>,
    now: DateTime<Utc>,
) -> String {
    let mut out = String::new();
    for (i, m) in memories.into_iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, memory_md_line(m, now)));
    }
    out
}

/// Compact markdown for the `context` projection: a total count plus Important
/// and Recent sections (each a numbered list).
fn render_context_md(
    recent: &[MemoryNote],
    important: &[MemoryNote],
    total: usize,
    now: DateTime<Utc>,
) -> String {
    if total == 0 && recent.is_empty() && important.is_empty() {
        return "no stored memories".to_string();
    }
    let mut out = format!("{total} memories in scope.\n");
    if !important.is_empty() {
        out.push_str("\nImportant:\n");
        out.push_str(&render_indexed(important.iter(), now));
    }
    if !recent.is_empty() {
        out.push_str("\nRecent:\n");
        out.push_str(&render_indexed(recent.iter(), now));
    }
    out
}

pub fn response_to_content(resp: Response, now: DateTime<Utc>) -> ToolContent {
    match resp {
        Response::Remembered { id } => ToolContent::json(json!({ "id": id.to_string() }), false),
        // W3.3: recall renders as compact markdown (one line per hit) for the
        // model; the full results ride structuredContent. W1.3 empty state +
        // W1.6d degraded warning are preserved. Projection only — the wire
        // `Response` is unchanged (no CONTRACT_VERSION bump).
        Response::Recalled { results, degraded } => {
            let text = if results.is_empty() {
                let mut t = "no stored memories match".to_string();
                if degraded {
                    t.push_str(" (vector search unavailable — keyword + graph channels only)");
                }
                t
            } else {
                let mut t = render_indexed(results.iter().map(|r| &r.memory), now);
                if degraded {
                    t.push_str(
                        "⚠ vector search unavailable (embedding provider error); \
                         keyword + graph results only\n",
                    );
                }
                t
            };
            let mut structured = if results.is_empty() {
                json!({ "results": [], "hint": "no stored memories match" })
            } else {
                json!({ "results": results })
            };
            if degraded {
                if let Some(obj) = structured.as_object_mut() {
                    obj.insert(
                        "warning".to_string(),
                        json!(
                            "vector search unavailable (embedding provider error); \
                             results from keyword and graph channels only"
                        ),
                    );
                }
            }
            ToolContent {
                text,
                structured,
                is_error: false,
            }
        }
        // `get` returns full content (the model fetched it deliberately), so it
        // keeps the full JSON as text rather than a truncated projection.
        Response::Got { memory } => ToolContent::json(json!({ "memory": memory }), false),
        Response::Listed { memories } => {
            let text = if memories.is_empty() {
                "no stored memories".to_string()
            } else {
                render_indexed(memories.iter(), now)
            };
            ToolContent {
                text,
                structured: json!({ "memories": memories }),
                is_error: false,
            }
        }
        Response::GraphResult { memories } => {
            ToolContent::json(json!({ "memories": memories }), false)
        }
        Response::Updated => ToolContent::json(json!({ "ok": true }), false),
        Response::Linked => ToolContent::json(json!({ "ok": true }), false),
        Response::Deleted => ToolContent::json(json!({ "ok": true }), false),
        Response::ContextResult {
            recent,
            important,
            total,
        } => {
            let text = render_context_md(&recent, &important, total, now);
            ToolContent {
                text,
                structured: json!({ "recent": recent, "important": important, "total": total }),
                is_error: false,
            }
        }
        Response::Pong {
            contract_version, ..
        } => ToolContent::json(json!({ "contract_version": contract_version }), false),
        Response::JobRan {
            scanned,
            changed,
            skipped,
        } => ToolContent::json(
            json!({ "scanned": scanned, "changed": changed, "skipped": skipped }),
            false,
        ),
        // Namespace rename is a CLI admin op (W0.3 carryover) with no MCP tool
        // surface; projected anyway so the mapping stays total.
        Response::NamespaceRenamed { moved, vectors } => {
            ToolContent::json(json!({ "moved": moved, "vectors": vectors }), false)
        }
        // Scrub is a CLI admin op (W2.4) with no MCP tool surface; projected
        // anyway so the mapping stays total.
        Response::Scrubbed {
            scanned,
            redacted,
            reembed_pending,
        } => ToolContent::json(
            json!({
                "scanned": scanned,
                "redacted": redacted,
                "reembed_pending": reembed_pending,
            }),
            false,
        ),
        Response::Error { kind, message } => ToolContent::json(
            json!({ "error": { "kind": kind, "message": message } }),
            true,
        ),
        // Streamed subscribe frames (and the subscribe ack) never reach the
        // request/response proxy path; map them defensively to an error content
        // (unreachable in practice).
        Response::Change(_) | Response::Lagged { .. } | Response::SubscribeAck => {
            ToolContent::json(
                json!({
                    "error": {
                        "kind": "protocol",
                        "message": "unexpected streamed frame on a request/response call",
                    }
                }),
                true,
            )
        }
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
    fn build_update_maps_and_validates_confidence() {
        let id = MemoryId::new();
        let u = build_request(
            "update",
            &json!({ "id": id.to_string(), "confidence": 0.25 }),
        )
        .unwrap();
        match u {
            Request::Update { updates, .. } => {
                assert_eq!(updates.confidence, Some(0.25));
            }
            other => panic!("expected Update, got {other:?}"),
        }
        for bad in [json!(-0.1), json!(1.5), json!("high")] {
            let err = build_request(
                "update",
                &json!({ "id": id.to_string(), "confidence": bad }),
            )
            .unwrap_err();
            assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS, "{bad:?}");
        }
    }

    #[test]
    fn build_link_maps_ids_type_and_reason() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let l = build_request(
            "link",
            &json!({
                "from": a.to_string(),
                "to": b.to_string(),
                "type": "contradicts",
                "reason": "newer decision disagrees"
            }),
        )
        .unwrap();
        match l {
            Request::Link {
                from,
                to,
                link_type,
                reason,
            } => {
                assert_eq!(from, a);
                assert_eq!(to, b);
                assert_eq!(link_type, rb_types::LinkType::Contradicts);
                assert_eq!(reason.as_deref(), Some("newer decision disagrees"));
            }
            other => panic!("expected Link, got {other:?}"),
        }
        // Bad link type and missing required args are INVALID_PARAMS.
        let err = build_request(
            "link",
            &json!({ "from": a.to_string(), "to": b.to_string(), "type": "depends_on" }),
        )
        .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
        let err = build_request("link", &json!({ "from": a.to_string() })).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
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
    fn response_to_content_renders_structured_json_plus_compact_markdown() {
        use rb_proto::Response;
        let now = Utc::now();
        let id = MemoryId::new();
        let remembered = response_to_content(Response::Remembered { id: id.clone() }, now);
        assert_eq!(remembered.structured["id"], id.to_string());

        let recalled = response_to_content(
            Response::Recalled {
                results: vec![SearchResult {
                    memory: note(),
                    score: 0.5,
                    channels: rb_types::ChannelHits::default(),
                }],
                degraded: false,
            },
            now,
        );
        // Full shape rides structuredContent (the wire Response is unchanged).
        assert!(recalled.structured["results"].is_array());
        assert_eq!(recalled.structured["results"][0]["score"], 0.5);
        // W3.3: the model-facing text is a compact markdown line — type +
        // importance + age + id — NOT the full JSON.
        assert!(
            recalled
                .text
                .starts_with("1. [architecture_decision, imp 8, "),
            "compact line: {}",
            recalled.text
        );
        assert!(recalled.text.contains("just now"));
        assert!(recalled.text.contains("one db one transaction"));
        assert!(recalled.text.contains("(id "));
        assert!(
            !recalled.text.contains("\"score\""),
            "no raw JSON in the model text"
        );
        assert!(!recalled.is_error);

        let got = response_to_content(
            Response::Got {
                memory: Some(note()),
            },
            now,
        );
        assert!(got.structured["memory"]["content"].is_string());

        let none = response_to_content(Response::Got { memory: None }, now);
        assert!(none.structured["memory"].is_null());

        let ctx = response_to_content(
            Response::ContextResult {
                recent: vec![note()],
                important: vec![note()],
                total: 2,
            },
            now,
        );
        assert_eq!(ctx.structured["total"], 2);
        assert!(ctx.text.contains("2 memories in scope"));
        assert!(ctx.text.contains("Important:") && ctx.text.contains("Recent:"));

        let err = response_to_content(
            Response::Error {
                kind: "not_found".into(),
                message: "nope".into(),
            },
            now,
        );
        assert!(
            err.structured.get("error").is_some(),
            "error variant carries an `error` key"
        );
        assert!(err.is_error, "error variant sets the isError flag");
    }

    #[test]
    fn recall_result_carries_contested_flag() {
        use rb_proto::Response;
        // Feature C: the additive `contested` boolean surfaces both in the
        // structured schema and as a ⚠ marker on the compact line.
        let mut contested = note();
        contested.contested = true;
        let recalled = response_to_content(
            Response::Recalled {
                results: vec![SearchResult {
                    memory: contested,
                    score: 0.9,
                    channels: rb_types::ChannelHits::default(),
                }],
                degraded: false,
            },
            Utc::now(),
        );
        assert_eq!(
            recalled.structured["results"][0]["memory"]["contested"],
            true
        );
        assert!(
            recalled.text.contains("⚠ contested"),
            "contested marker on the compact line: {}",
            recalled.text
        );
    }

    #[test]
    fn empty_recall_returns_model_legible_empty_state() {
        use rb_proto::Response;
        // W1.3: below-floor/empty recall renders `{results: [], hint: ...}` so
        // the model can tell "nothing stored matches" from a tool failure.
        let recalled = response_to_content(
            Response::Recalled {
                results: Vec::new(),
                degraded: false,
            },
            Utc::now(),
        );
        assert!(recalled.structured["results"].is_array());
        assert_eq!(
            recalled.structured["results"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(recalled.structured["hint"], "no stored memories match");
        assert_eq!(recalled.text, "no stored memories match");
    }

    #[test]
    fn non_empty_recall_carries_no_hint() {
        use rb_proto::Response;
        // The hint is the EMPTY state only — a populated recall keeps its
        // pre-W1.3 structured shape.
        let recalled = response_to_content(
            Response::Recalled {
                results: vec![SearchResult {
                    memory: note(),
                    score: 0.9,
                    channels: rb_types::ChannelHits::default(),
                }],
                degraded: false,
            },
            Utc::now(),
        );
        assert!(
            recalled.structured.get("hint").is_none(),
            "hint only on empty results"
        );
    }

    #[test]
    fn degraded_recall_renders_a_one_line_warning() {
        use rb_proto::Response;
        // W1.6d: a degraded recall (embedder outage; keyword+graph only)
        // surfaces a warning in BOTH the structured payload and the compact text.
        let recalled = response_to_content(
            Response::Recalled {
                results: vec![SearchResult {
                    memory: note(),
                    score: 0.9,
                    channels: rb_types::ChannelHits::default(),
                }],
                degraded: true,
            },
            Utc::now(),
        );
        let warning = recalled.structured["warning"]
            .as_str()
            .expect("degraded recall must carry a warning line");
        assert!(
            warning.contains("vector search unavailable"),
            "warning names the degradation: {warning}"
        );
        assert!(
            recalled.text.contains("vector search unavailable"),
            "degraded warning also in the compact text: {}",
            recalled.text
        );

        let empty = response_to_content(
            Response::Recalled {
                results: Vec::new(),
                degraded: true,
            },
            Utc::now(),
        );
        assert_eq!(empty.structured["hint"], "no stored memories match");
        assert!(
            empty.structured["warning"].is_string(),
            "the empty state keeps the degradation warning too"
        );
        assert!(
            empty.text.contains("vector search unavailable"),
            "empty+degraded text still flags the outage: {}",
            empty.text
        );
    }

    #[test]
    fn non_degraded_recall_carries_no_warning() {
        use rb_proto::Response;
        let recalled = response_to_content(
            Response::Recalled {
                results: Vec::new(),
                degraded: false,
            },
            Utc::now(),
        );
        assert!(
            recalled.structured.get("warning").is_none(),
            "warning only when degraded"
        );
    }
}
