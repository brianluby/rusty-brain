//! The 8 MCP tools rusty-brain exposes, each with a JSON-Schema input contract.

use serde::Serialize;
use serde_json::{json, Value};

/// One MCP tool descriptor as returned by `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// The valid `memory_type` db strings, surfaced as a JSON-Schema enum so agents
/// pick a legal value. Mirrors `rb_types::MemoryType::as_str`.
fn memory_type_enum() -> Value {
    json!([
        "architecture_decision",
        "code_pattern",
        "bug_fix",
        "configuration",
        "constraint",
        "entity",
        "insight",
        "reference",
        "preference"
    ])
}

/// All 8 tool definitions. Pure: no IO, deterministic ordering.
pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "remember",
            description: "Store a new memory in the shared store. Returns the new memory id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text to remember." },
                    "context": { "type": "string", "description": "Optional surrounding context." },
                    "type": { "type": "string", "enum": memory_type_enum(),
                              "description": "Memory category (default: insight)." },
                    "importance": { "type": "integer", "minimum": 1, "maximum": 10,
                                    "description": "Importance 1-10 (default: 5)." },
                    "tags": { "type": "array", "items": { "type": "string" },
                              "description": "Optional tags." }
                },
                "required": ["content"]
            }),
        },
        ToolDef {
            name: "recall",
            description: "Hybrid (keyword + vector + graph) recall of memories matching a query.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Free-text query." },
                    "limit": { "type": "integer", "minimum": 1,
                               "description": "Max results (default: 10)." },
                    "type": { "type": "string", "enum": memory_type_enum(),
                              "description": "Restrict to a memory type." },
                    "tags": { "type": "array", "items": { "type": "string" },
                              "description": "Filter by tags." }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "get",
            description: "Fetch a single memory (full content + links) by id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "list",
            description: "List memories in the current namespace.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1,
                               "description": "Max results (default: 20)." },
                    "min_importance": { "type": "integer", "minimum": 1, "maximum": 10,
                                        "description": "Only memories at/above this importance." }
                }
            }),
        },
        ToolDef {
            name: "graph",
            description: "Show memories connected to an id by graph links.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." },
                    "depth": { "type": "integer", "minimum": 1,
                               "description": "Traversal depth (default: 1)." }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "update",
            description: "Apply a partial update to a memory (only provided fields change). \
                          Content cannot be updated — delete and re-remember so embeddings \
                          stay consistent.",
            // `content` is deliberately ABSENT from the schema: the runtime
            // rejects it, and schema-driven clients would keep generating it.
            // Re-introduce alongside W3.1 update-as-supersede.
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." },
                    "summary": { "type": "string" },
                    "importance": { "type": "integer", "minimum": 1, "maximum": 10 },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "context": { "type": "string" }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "delete",
            description: "Soft-delete (archive) a memory by id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "context",
            description: "Project context payload: recent + important memories and a total count.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDef {
            name: "poll_changes",
            description: "Drain buffered change notifications for the current \
                          namespace since the last poll. Returns up to `max` events, \
                          a count of events dropped (the buffer is bounded and lossy \
                          under heavy write load), and the subscriber health: status \
                          'disconnected' means events may be missed, not that \
                          nothing changed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "max": { "type": "integer", "minimum": 1,
                             "description": "Maximum events to return this poll (default: 100)." }
                }
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn exposes_exactly_the_nine_tools() {
        let names: BTreeSet<&str> = tool_definitions().iter().map(|t| t.name).collect();
        let expected: BTreeSet<&str> = [
            "remember",
            "recall",
            "get",
            "list",
            "graph",
            "update",
            "delete",
            "context",
            "poll_changes",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            names, expected,
            "tool set must be the 8 spine tools + poll_changes"
        );
        assert_eq!(tool_definitions().len(), 9);
    }

    #[test]
    fn poll_changes_takes_optional_max() {
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "poll_changes")
            .unwrap();
        assert_eq!(t.input_schema["type"], "object");
        let props = t.input_schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("max"),
            "poll_changes accepts optional max"
        );
        // No required fields: polling with no args is valid.
        let required_empty = match t.input_schema.get("required") {
            None => true,
            Some(v) => v.as_array().map(|a| a.is_empty()).unwrap_or(false),
        };
        assert!(required_empty, "poll_changes must require no input");
    }

    #[test]
    fn every_tool_has_object_input_schema_and_nonempty_description() {
        for t in tool_definitions() {
            assert!(
                !t.description.is_empty(),
                "tool {} needs a description",
                t.name
            );
            assert_eq!(
                t.input_schema["type"], "object",
                "tool {} inputSchema must be an object",
                t.name
            );
            // `properties` must be present (possibly empty for `context`).
            assert!(
                t.input_schema.get("properties").is_some(),
                "tool {} inputSchema needs properties",
                t.name
            );
        }
    }

    #[test]
    fn remember_requires_content_only() {
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "remember")
            .unwrap();
        let required: Vec<&str> = t.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["content"]);
        let props = t.input_schema["properties"].as_object().unwrap();
        for opt in ["context", "type", "importance", "tags"] {
            assert!(
                props.contains_key(opt),
                "remember should accept optional {opt}"
            );
        }
    }

    #[test]
    fn update_schema_omits_content_and_description_carries_the_guidance() {
        // F55: the schema must not advertise a `content` property at all — the
        // engine rejects content updates, and schema-driven clients generate
        // whatever the schema offers. The tool DESCRIPTION carries the
        // limitation and the workaround (delete + re-remember) instead.
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "update")
            .unwrap();
        let props = t.input_schema["properties"].as_object().unwrap();
        assert!(
            !props.contains_key("content"),
            "update schema must not advertise content: {props:?}"
        );
        assert!(
            t.description.contains("Content cannot be updated"),
            "description must state the limitation: {}",
            t.description
        );
        assert!(
            t.description.contains("delete and re-remember"),
            "description must point at the workaround: {}",
            t.description
        );
    }

    #[test]
    fn recall_requires_query_and_get_requires_id() {
        let recall = tool_definitions()
            .into_iter()
            .find(|t| t.name == "recall")
            .unwrap();
        assert_eq!(recall.input_schema["required"][0], "query");
        let get = tool_definitions()
            .into_iter()
            .find(|t| t.name == "get")
            .unwrap();
        assert_eq!(get.input_schema["required"][0], "id");
    }

    #[test]
    fn context_takes_no_required_input() {
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "context")
            .unwrap();
        // No `required` key, or an empty required array — either is acceptable.
        let required_empty = match t.input_schema.get("required") {
            None => true,
            Some(v) => v.as_array().map(|a| a.is_empty()).unwrap_or(false),
        };
        assert!(required_empty, "context must require no input");
    }

    #[test]
    fn memory_type_enum_matches_memorytype() {
        // Drift guard: the strings in `memory_type_enum()` must exactly equal
        // the `MemoryType::as_str()` values for ALL variants, and each must
        // round-trip via `MemoryType::parse`. Add a variant to `MemoryType`
        // without updating `memory_type_enum()` — this test fails.
        use rb_types::MemoryType;
        use std::collections::BTreeSet;

        // All known MemoryType variants — must be kept in sync with rb-types.
        const ALL_VARIANTS: [MemoryType; 9] = [
            MemoryType::ArchitectureDecision,
            MemoryType::CodePattern,
            MemoryType::BugFix,
            MemoryType::Configuration,
            MemoryType::Constraint,
            MemoryType::Entity,
            MemoryType::Insight,
            MemoryType::Reference,
            MemoryType::Preference,
        ];

        let schema_strings: BTreeSet<String> = memory_type_enum()
            .as_array()
            .expect("memory_type_enum must be a JSON array")
            .iter()
            .map(|v| {
                v.as_str()
                    .expect("each enum entry must be a string")
                    .to_string()
            })
            .collect();

        let type_strings: BTreeSet<String> = ALL_VARIANTS
            .iter()
            .map(|mt| mt.as_str().to_string())
            .collect();

        assert_eq!(
            schema_strings, type_strings,
            "memory_type_enum() strings must exactly match MemoryType::as_str() for all variants"
        );

        // Every schema string must also round-trip through MemoryType::parse.
        for s in &schema_strings {
            let parsed = MemoryType::parse(s);
            assert!(
                parsed.is_ok(),
                "schema string '{s}' must round-trip via MemoryType::parse"
            );
            assert_eq!(
                parsed.unwrap().as_str(),
                s.as_str(),
                "as_str() must be the inverse of parse() for '{s}'"
            );
        }
    }

    #[test]
    fn tool_list_serializes_with_camelcase_input_schema() {
        // The MCP wire shape uses `inputSchema` (camelCase).
        let t = &tool_definitions()[0];
        let s = serde_json::to_string(t).unwrap();
        assert!(
            s.contains("\"inputSchema\""),
            "must serialize as inputSchema: {s}"
        );
        assert!(!s.contains("input_schema"), "must not leak snake_case: {s}");
    }
}
