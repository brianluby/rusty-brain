# Research: Type System & Configuration

**Branch**: `002-type-system-config` | **Date**: 2026-03-01

## R-1: Claude Code Hook JSON Protocol

**Decision**: Model hook input as a flat struct with common required fields plus optional event-specific fields. Model hook output as a struct with universal fields plus optional `hookSpecificOutput` keyed by event type.

**Rationale**: The Claude Code hook protocol has a common set of fields (`session_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name`) sent to every hook, with event-specific fields varying by event type. The output similarly has universal fields (`continue`, `stopReason`, `suppressOutput`, `systemMessage`) plus event-specific decision control via `hookSpecificOutput`. A flat struct with optional fields was chosen for simplicity and forward compatibility — unknown fields are silently ignored via serde defaults.

**Alternatives considered**:
- Enum for event-specific payloads — more type-safe but over-constrains deserialization; rusty-brain only handles 3 event types so full enum modeling has low ROI
- Separate struct per event type — no shared base, duplicates common fields
- Trait-based polymorphism — over-engineered for deserialization from JSON

### Hook Input Common Fields

All events carry:

| Field | Type | Required |
|-------|------|----------|
| `session_id` | String | yes |
| `transcript_path` | String | yes |
| `cwd` | String | yes |
| `permission_mode` | String | yes |
| `hook_event_name` | String | yes |

### Hook Input Event-Specific Fields

| Event | Additional Fields |
|-------|-------------------|
| PreToolUse | `tool_name`, `tool_input` (object), `tool_use_id` |
| PostToolUse | `tool_name`, `tool_input` (object), `tool_response` (object), `tool_use_id` |
| PostToolUseFailure | `tool_name`, `tool_input` (object), `tool_use_id`, `error` (string), `is_interrupt` (bool, optional) |
| Stop | `stop_hook_active` (bool), `last_assistant_message` (string) |
| SubagentStop | `stop_hook_active`, `last_assistant_message`, `agent_id`, `agent_type`, `agent_transcript_path` |
| SessionStart | `source` (startup/resume/clear/compact), `model` (string), `agent_type` (optional) |
| SessionEnd | `source` (clear/logout/prompt_input_exit/bypass_permissions_disabled/other) |
| UserPromptSubmit | `prompt` (string) |
| Notification | `message`, `notification_type`, `title` (optional) |
| PermissionRequest | `tool_name`, `tool_input`, `permission_suggestions` (optional array) |
| SubagentStart | `agent_id`, `agent_type` |
| TeammateIdle | `teammate_name`, `team_name` |
| TaskCompleted | `task_id`, `task_subject`, `task_description` (optional), `teammate_name` (optional), `team_name` (optional) |
| ConfigChange | `source`, `file_path` (optional) |
| PreCompact | (no additional fields documented) |

### Hook Output Universal Fields

| Field | Type | Default |
|-------|------|---------|
| `continue` | bool | true |
| `stopReason` | String (optional) | none |
| `suppressOutput` | bool | false |
| `systemMessage` | String (optional) | none |

### Hook Output Event-Specific Decision Control

- **PreToolUse**: `hookSpecificOutput.permissionDecision` (allow/deny/ask), `permissionDecisionReason`, `updatedInput`, `additionalContext`
- **PostToolUse**: `decision` (block), `reason`, `hookSpecificOutput.additionalContext`, `updatedMCPToolOutput`
- **Stop/SubagentStop**: `decision` (block), `reason`
- **UserPromptSubmit**: `decision` (block), `reason`, `hookSpecificOutput.additionalContext`
- **SessionStart**: `hookSpecificOutput.additionalContext`

### Scope Decision for Phase 1

For the types crate, we model the **input types for the three hooks rusty-brain actually uses** (SessionStart, PostToolUse/PreToolUse, Stop) plus a generic fallback. We also model the output types. The full event catalog is documented here for completeness but only the hooks rusty-brain implements need concrete types.

**Source**: https://code.claude.com/docs/en/hooks

---

## R-2: TypeScript agent-brain Defaults and Type Mapping

**Decision**: Match TypeScript defaults exactly for behavioral compatibility. Adapt field names to Rust conventions (snake_case) while preserving JSON serialization names via serde rename attributes.

**Rationale**: The Rust port must be a drop-in replacement. Any deviation in defaults changes agent behavior silently.

### Configuration Defaults (Confirmed Match)

| Field (TS) | Field (Rust) | Default | Confirmed |
|------------|-------------|---------|-----------|
| `memoryPath` | `memory_path` | `.agent-brain/mind.mv2` | yes |
| `maxContextObservations` | `max_context_observations` | 20 | yes |
| `maxContextTokens` | `max_context_tokens` | 2000 | yes |
| `autoCompress` | `auto_compress` | true | yes |
| `minConfidence` | `min_confidence` | 0.6 | yes |
| `debug` | `debug` | false | yes |

### Environment Variables (Confirmed)

| Variable | Controls | Usage |
|----------|----------|-------|
| `MEMVID_PLATFORM` | Explicit platform override | Trim + lowercase, highest priority for platform detection |
| `MEMVID_MIND_DEBUG` | Debug logging | `"1"` enables |
| `MEMVID_PLATFORM_MEMORY_PATH` | Memory file path override | Used when path opt-in enabled |
| `MEMVID_PLATFORM_PATH_OPT_IN` | Per-platform path mode | `"1"` enables `mind-{platform}.mv2` naming |
| `CLAUDE_PROJECT_DIR` | Project root fallback | Priority: cwd > CLAUDE_PROJECT_DIR > OPENCODE_PROJECT_DIR > process.cwd() |
| `OPENCODE_PROJECT_DIR` | Secondary project root fallback | Also used for git root in stop hook |

Additional env vars found in TS but not in spec:
- `OPENCODE` — secondary platform signal (`"1"` → opencode platform)
- `REPO_ROOT` — git root fallback in stop hook only

### TypeScript ↔ Rust Type Mapping Discrepancies

**IMPORTANT**: The spec's ObservationMetadata differs from the TypeScript implementation:

| Spec Field | TS Equivalent | Notes |
|------------|---------------|-------|
| `files` | `files` | Match |
| `platform` | *(not in TS metadata)* | In TS, platform is on HookInput, not metadata |
| `project_key` | *(not in TS metadata)* | In TS, project_id is on HookInput |
| `compressed` | *(not in TS metadata)* | Not present in TS ObservationMetadata |
| `session_id` | `sessionId` | Match (different casing) |
| `extra` (HashMap) | `[key: string]: unknown` (index signature) | Functionally equivalent |

TS ObservationMetadata has fields NOT in spec:
- `functions: string[]`
- `error: string`
- `confidence: number`
- `tags: string[]`

**Resolution**: The spec intentionally redesigned ObservationMetadata for the Rust port. The spec adds `platform`, `project_key`, and `compressed` fields that were previously scattered across other types. The spec removes `functions`, `error`, `confidence`, and `tags` that were TS-specific. Follow the spec, not the TS field list, since the spec represents the intended Rust design.

### Observation Field Mapping

| TS Field | Rust Field | Type Change |
|----------|-----------|-------------|
| `id` (string) | `id` (Uuid) | Parse string → UUID |
| `timestamp` (number, epoch ms) | `timestamp` (DateTime<Utc>) | Parse epoch ms → chrono DateTime |
| `type` (string union) | `obs_type` (ObservationType enum) | `type` is reserved in Rust; rename to `obs_type`, serde rename to `"type"` |
| `tool` (optional string) | `tool_name` (String) | Spec makes it required and renames |
| `summary` (string) | `summary` (String) | Match |
| `content` (string) | `content` (String) | Match |
| `metadata` (optional) | `metadata` (Option) | Match |

### MindStats Field Mapping

| TS Field | Rust Field | Notes |
|----------|-----------|-------|
| `totalObservations` | `total_observations` | Match |
| `totalSessions` | `total_sessions` | Match |
| `oldestMemory` (number) | `oldest_memory` (Option<DateTime>) | TS uses epoch; Rust uses Option<DateTime> for empty stores |
| `newestMemory` (number) | `newest_memory` (Option<DateTime>) | Same |
| `fileSize` (number) | `file_size_bytes` (u64) | Renamed for clarity |
| `topTypes` (Record) | `type_counts` (HashMap) | Renamed for clarity |

**Source**: https://github.com/brianluby/agent-brain/ `/src/types.ts`

---

## R-3: Serde Patterns for Rust Types

**Decision**: Use `serde` derive macros with `#[serde(rename_all = "camelCase")]` on types that need JSON compatibility with the TypeScript implementation. Use `#[serde(rename = "type")]` for the `obs_type` field. Use `#[serde(default)]` on MindConfig for partial deserialization. Use `#[serde(deny_unknown_fields)]` sparingly — only where forward compatibility is NOT needed.

**Rationale**: The TypeScript implementation uses camelCase JSON keys. Rust conventions use snake_case. Serde rename attributes bridge the gap without runtime cost.

**Key patterns**:
- `#[serde(rename_all = "camelCase")]` on Observation, SessionSummary, InjectedContext, MindConfig, MindStats
- `#[serde(rename = "type")]` on `obs_type` field
- `#[serde(default)]` on MindConfig struct for partial deserialization with defaults
- `#[serde(flatten)]` on ObservationMetadata's `extra` field to capture arbitrary keys
- `#[serde(rename_all = "snake_case")]` on HookInput (Claude Code protocol uses snake_case)
- HookOutput uses per-field `#[serde(rename = "...")]` with mixed-case keys matching the Claude Code output protocol
- NO `deny_unknown_fields` on HookInput (forward compatibility requirement S-3)

---

## R-4: Error Code Design

**Decision**: Use string constant error codes in the format `E_CATEGORY_DETAIL` (e.g., `E_FS_NOT_FOUND`, `E_CONFIG_INVALID_VALUE`). Codes are `&'static str` constants, not derived from enum variant names.

**Rationale**: String codes are human-readable, agent-parseable, and stable across refactors. Numeric codes are harder to remember and extend. Deriving from variant names couples the code to internal naming.

**Error categories and codes**:

| Category | Code Pattern | Examples |
|----------|-------------|----------|
| FileSystem | `E_FS_*` | `E_FS_NOT_FOUND`, `E_FS_PERMISSION_DENIED`, `E_FS_IO_ERROR` |
| Configuration | `E_CONFIG_*` | `E_CONFIG_INVALID_VALUE`, `E_CONFIG_MISSING_FIELD`, `E_CONFIG_PARSE_ERROR` |
| Serialization | `E_SER_*` | `E_SER_SERIALIZE_FAILED`, `E_SER_DESERIALIZE_FAILED` |
| Lock | `E_LOCK_*` | `E_LOCK_ACQUISITION_FAILED`, `E_LOCK_TIMEOUT` |
| MemoryCorruption | `E_MEM_*` | `E_MEM_CORRUPTED_INDEX`, `E_MEM_INVALID_CHECKSUM` |
| InvalidInput | `E_INPUT_*` | `E_INPUT_EMPTY_FIELD`, `E_INPUT_OUT_OF_RANGE`, `E_INPUT_INVALID_FORMAT` |

**Alternatives considered**:
- Numeric codes (HTTP-style 4xx/5xx) — less readable, harder to extend
- Derive from variant Display — couples code to formatting, fragile

---

## R-5: Non-Exhaustive Enum Strategy

**Decision**: Mark `ObservationType` and `AgentBrainError` as `#[non_exhaustive]`. This addresses open question Q1 from the PRD.

**Rationale**: Both enums are likely to gain variants in future phases. `#[non_exhaustive]` forces downstream `match` statements to include a wildcard arm, preventing breakage when new variants are added.

**Trade-off**: Downstream code cannot exhaustively match without `_`, which slightly reduces compile-time safety. However, the benefit of additive-only API evolution outweighs this cost.
