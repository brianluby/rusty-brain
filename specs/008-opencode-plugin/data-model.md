# Data Model: OpenCode Plugin Adapter

**Feature**: 008-opencode-plugin | **Date**: 2026-03-03

---

## Entities

### MindToolInput

The structured input for the native mind tool, received as JSON on stdin.

| Field | Type | Required | Description | Validation |
|-------|------|----------|-------------|------------|
| mode | String | Yes | Operation mode: `search`, `ask`, `recent`, `stats`, `remember` | Validated against fixed whitelist (SEC-8) |
| query | Option\<String\> | Conditional | Search query or question text | Required for `search` and `ask` modes; ignored for others |
| content | Option\<String\> | Conditional | Content to store as observation | Required for `remember` mode; ignored for others |
| limit | Option\<usize\> | No | Maximum number of results to return | Applies to `search` and `recent` modes; defaults to 10 |

**Serde**: No `deny_unknown_fields` — unknown fields are silently ignored for forward compatibility (M-7, SEC-7).

**State Transitions**: N/A — stateless request/response.

---

### MindToolOutput

The structured output from the native mind tool, written as JSON to stdout.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| success | bool | Yes | Whether the operation completed successfully |
| data | Option\<serde_json::Value\> | No | Mode-specific result data (see mode schemas below) |
| error | Option\<String\> | No | Machine-readable error message (present when success=false) |

**Mode-Specific `data` Schemas**:

- **search**: `Vec<SearchResult>` where SearchResult = `{ obs_type, summary, content_excerpt, timestamp, score, tool_name }`
- **ask**: `{ answer: Option<String> }` — synthesized answer or null if no relevant memories
- **recent**: `Vec<TimelineEntry>` where TimelineEntry = `{ obs_type, summary, timestamp, tool_name }`
- **stats**: `{ total_observations, total_sessions, date_range, file_size_bytes, type_breakdown }`
- **remember**: `{ observation_id: String }` — ULID of the stored observation

---

### SidecarState

Session-scoped state persisted as a JSON sidecar file (`.opencode/session-<id>.json`).

| Field | Type | Required | Description | Validation |
|-------|------|----------|-------------|------------|
| session_id | String | Yes | Unique session identifier (from HookInput or generated) | Non-empty after sanitization |
| created_at | DateTime\<Utc\> | Yes | When the session started | ISO 8601 format |
| last_updated | DateTime\<Utc\> | Yes | Last modification timestamp | ISO 8601 format; updated on every write |
| observation_count | u32 | Yes | Number of observations stored in this session | Incremented on each new (non-duplicate) observation |
| dedup_hashes | Vec\<String\> | Yes | LRU-bounded dedup cache (max 1024) | Each entry is a 16-char hex string from DefaultHasher |

**Lifecycle**:
- Created on first tool hook invocation in a session
- Updated on each tool hook invocation (observation stored or dedup check performed)
- Deleted on session cleanup (S-1)
- Orphaned files cleaned up after 24 hours (S-2)

**File Permissions**: 0600 (SEC-2)

**Corruption Recovery**: If deserialization fails, delete corrupt file and create fresh state with WARN trace.

---

### Plugin Manifest

Static configuration file for OpenCode plugin discovery (M-8).

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | String | Yes | Plugin identifier: `"rusty-brain"` |
| version | String | Yes | Plugin version (matches crate version) |
| description | String | Yes | Human-readable description |
| binary_path | String | Yes | Path to the `rusty-brain` binary |
| capabilities | Vec\<String\> | Yes | Declared capabilities: `["chat_hook", "tool_hook", "mind_tool"]` |

**Note**: Exact format depends on OpenCode's plugin protocol (PRD Spike-1). This schema represents the minimum information needed. Additional fields (command templates, event subscriptions) may be required.

---

## Entity Relationships

```mermaid
erDiagram
    HOOK_INPUT ||--|| HOOK_OUTPUT : "produces"
    HOOK_INPUT ||--o| SIDECAR_STATE : "reads/updates (tool hook)"
    MIND_TOOL_INPUT ||--|| MIND_TOOL_OUTPUT : "produces"
    SIDECAR_STATE ||--o{ DEDUP_HASH : "contains (max 1024)"
    PLUGIN_MANIFEST ||--o{ CAPABILITY : "declares"

    HOOK_INPUT {
        string session_id PK
        string cwd
        string hook_event_name
        string tool_name "optional"
        json tool_input "optional"
        json tool_response "optional"
    }

    HOOK_OUTPUT {
        bool continue_execution "Option~bool~ - serde rename: continue"
        string stop_reason "Option - serde rename: stopReason"
        bool suppress_output "Option~bool~ - serde rename: suppressOutput"
        string system_message "Option - serde rename: systemMessage (context injection)"
        string decision "Option - PreToolUse permission"
        string reason "Option - human-readable reason"
        json hook_specific_output "Option - serde rename: hookSpecificOutput (structured InjectedContext)"
    }

    SIDECAR_STATE {
        string session_id PK
        datetime created_at
        datetime last_updated
        u32 observation_count
    }

    DEDUP_HASH {
        string hash "16-char hex"
    }

    MIND_TOOL_INPUT {
        string mode "search|ask|recent|stats|remember"
        string query "optional"
        string content "optional"
        usize limit "optional"
    }

    MIND_TOOL_OUTPUT {
        bool success
        json data "optional, mode-specific"
        string error "optional"
    }

    PLUGIN_MANIFEST {
        string name
        string version
        string description
        string binary_path
    }

    CAPABILITY {
        string name "chat_hook|tool_hook|mind_tool"
    }
```

## Existing Types Used (from crates/types, DO NOT MODIFY)

| Type | Source | Usage |
|------|--------|-------|
| `HookInput` | `crates/types/src/hooks.rs` | Input for chat hook and tool hook handlers |
| `HookOutput` | `crates/types/src/hooks.rs` | Output from chat hook, tool hook, session cleanup handlers |
| `InjectedContext` | `crates/types/src/context.rs` | Returned by `Mind::get_context()`; serialized into `system_message` |
| `ObservationType` | `crates/types/src/observation.rs` | Used for `remember` operations (default: `Discovery`) |
| `Observation` | `crates/types/src/observation.rs` | Elements within `InjectedContext.recent_observations` and `relevant_memories` |
| `SessionSummary` | `crates/types/src/session.rs` | Elements within `InjectedContext.session_summaries` |
| `MindConfig` | `crates/types/src/config.rs` | Configuration for `Mind::open()` |
| `MindStats` | `crates/types/src/stats.rs` | Returned by `Mind::stats()` |
| `RustyBrainError` | `crates/types/src/error.rs` | Error type for all handler operations |

## Existing Types Used (from crates/core, DO NOT MODIFY)

| Type | Source | Usage |
|------|--------|-------|
| `Mind` | `crates/core/src/mind.rs` | Primary API for memory operations |
| `MemorySearchResult` | `crates/core/src/mind.rs` | Returned by `Mind::search()` |
| `TimelineEntry` | `crates/core/src/mind.rs` | Returned by `Mind::timeline()` |
