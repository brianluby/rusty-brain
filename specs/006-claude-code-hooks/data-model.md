# Data Model: Claude Code Hooks

**Feature Branch**: `006-claude-code-hooks`
**Date**: 2026-03-03

## Entities

All entities in the hooks crate are either re-used from existing crates or are internal to hook processing. No new domain entities are introduced — hooks are a thin integration layer.

### Existing Entities (from crates/types, crates/core)

| Entity | Crate | Role in Hooks |
|--------|-------|---------------|
| `HookInput` | types | Deserialized from stdin; input to all handlers |
| `HookOutput` | types | Serialized to stdout; output of all handlers |
| `ObservationType` | types | Classifies observations stored by post-tool-use |
| `Observation` | types | Created via Mind::remember; returned in InjectedContext |
| `ObservationMetadata` | types | Attached to observations for platform/project context |
| `SessionSummary` | types | Created via Mind::save_session_summary; returned in InjectedContext |
| `InjectedContext` | types | Returned by Mind::get_context; formatted into systemMessage |
| `MindConfig` | types | Configures Mind::open for each handler |
| `MindStats` | types | Returned by Mind::stats; used in systemMessage formatting |
| `ProjectContext` | types | Extracted from HookInput for identity resolution |
| `ProjectIdentity` | types | Resolved from ProjectContext; used for memory path |
| `ResolvedMemoryPath` | platforms | Result of resolve_memory_path; contains PathBuf + PathMode |
| `Mind` | core | Memory engine; opened per handler invocation |
| `RustyBrainError` | types | Error type from core/platforms; converted to HookError |

### New Entities (crates/hooks internal)

#### HookError

Internal error type for hook processing. Converted to fail-open `HookOutput` at the boundary.

```rust
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("[E_HOOK_IO] I/O error: {message}")]
    Io { message: String, source: Option<std::io::Error> },

    #[error("[E_HOOK_PARSE] Parse error: {message}")]
    Parse { message: String },

    #[error("[E_HOOK_MIND] Mind error: {message}")]
    Mind { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> },

    #[error("[E_HOOK_PLATFORM] Platform error: {message}")]
    Platform { message: String },

    #[error("[E_HOOK_GIT] Git error: {message}")]
    Git { message: String },

    #[error("[E_HOOK_DEDUP] Dedup cache error: {message}")]
    Dedup { message: String },
}
```

**Stable error codes**: `E_HOOK_IO`, `E_HOOK_PARSE`, `E_HOOK_MIND`, `E_HOOK_PLATFORM`, `E_HOOK_GIT`, `E_HOOK_DEDUP`.

#### DedupEntry

Internal representation of a dedup cache entry (not a domain entity).

```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct DedupCache {
    entries: HashMap<String, i64>,  // hash(tool+summary) -> unix_timestamp
}
```

**Fields**:
- `entries`: Map of hash keys (string representation of u64) to Unix timestamps (seconds)
- Auto-pruned: entries older than 60 seconds removed on every read

**Validation rules**:
- Hash key must be non-empty string
- Timestamp must be positive
- On corrupt/unreadable file: treated as empty cache (fail-open)

#### Cli (clap derive)

```rust
#[derive(clap::Parser)]
#[command(name = "rusty-brain", about = "Memory hooks for Claude Code")]
struct Cli {
    #[command(subcommand)]
    command: Subcommand,
}

#[derive(clap::Subcommand)]
enum Subcommand {
    /// Initialize memory and inject context at session start
    SessionStart,
    /// Capture tool observations
    PostToolUse,
    /// Generate session summary and shutdown
    Stop,
    /// Track installation version
    SmartInstall,
}
```

### Relationships

```
HookInput (stdin) ──deserialize──> Handler Function
   │
   ├── session-start:
   │     HookInput ──detect_platform──> platform_name
   │     HookInput ──resolve_project_identity──> ProjectIdentity
   │     (platform_name, cwd) ──resolve_memory_path──> ResolvedMemoryPath
   │     ResolvedMemoryPath ──Mind::open──> Mind
   │     Mind ──get_context──> InjectedContext
   │     Mind ──stats──> MindStats
   │     (InjectedContext, MindStats) ──format──> systemMessage
   │     systemMessage ──> HookOutput
   │
   ├── post-tool-use:
   │     HookInput.tool_name ──classify──> ObservationType
   │     HookInput.tool_response ──truncate──> truncated_content
   │     (tool_name, summary) ──DedupCache::is_duplicate──> bool
   │     (obs_type, tool_name, summary, content) ──Mind::remember──> ULID
   │     ──> HookOutput { continue: true }
   │
   ├── stop:
   │     HookInput.cwd ──detect_modified_files──> Vec<String>
   │     each file ──Mind::remember──> ULID (per-file observation)
   │     (decisions, files, summary) ──Mind::save_session_summary──> ULID
   │     ──> HookOutput { systemMessage: summary }
   │
   └── smart-install:
         binary_version ──compare──> .install-version file
         ──> HookOutput { continue: true }
```

### State Transitions

**Mind Lifecycle (per invocation)**:
```
Not Initialized ──Mind::open(config)──> Initialized ──handler operations──> Done
```
Each hook invocation is a fresh process. No state persists in memory across invocations.

**Dedup Cache Entry Lifecycle**:
```
(not exists) ──record()──> Active (timestamp) ──60s elapsed──> Expired ──prune()──> Removed
```

**Version Marker Lifecycle**:
```
(not exists) ──smart-install──> Created(version)
Created(v1) ──smart-install(v1)──> No-op
Created(v1) ──smart-install(v2)──> Updated(v2)
```

### Tool Name to ObservationType Mapping

| Tool Name | ObservationType | Summary Template |
|-----------|----------------|------------------|
| `Read` | `Discovery` | "Read {file_path}" |
| `Edit` | `Feature` | "Edited {file_path}" |
| `Write` | `Feature` | "Wrote {file_path}" |
| `Bash` | `Discovery` | "Ran command: {truncated_command}" |
| `Grep` | `Discovery` | "Searched for {pattern}" |
| `Glob` | `Discovery` | "Searched files: {pattern}" |
| `WebFetch` | `Discovery` | "Fetched {url}" |
| `WebSearch` | `Discovery` | "Searched web: {query}" |
| (unknown) | `Discovery` | "Used {tool_name}" |
