# Research: OpenCode Plugin Adapter

**Feature**: 008-opencode-plugin | **Date**: 2026-03-03

This document resolves all NEEDS CLARIFICATION items from the Technical Context and documents research findings for key design decisions.

---

## R1: OpenCode Plugin Protocol

**Decision**: Subprocess-per-event with JSON stdin/stdout protocol, matching the Claude Code hook pattern.

**Rationale**: The PRD assumes OpenCode supports binary plugins invoked as subprocesses (A-4). The AR selected CLI subcommands as the entry point (`rusty-brain opencode chat-hook`, etc.). This is the simplest model that works regardless of OpenCode's exact plugin protocol — if the protocol differs, only the manifest and CLI argument mapping change, not the handler logic.

**Alternatives Considered**:
- Long-lived daemon process: Rejected — complex lifecycle management, crash recovery; unnecessary when sidecar file handles session state (AR Option 2 analysis)
- Shared library / FFI: Rejected — platform-specific, harder to test, more complex build pipeline
- WebSocket server: Rejected — adds network dependency, violates Constitution IX (local-only default)

**Open Spike**: PRD Spike-1 — validate OpenCode's actual manifest format and invocation mechanism. The handler library is protocol-agnostic; only the CLI entry point and manifest file need adaptation.

---

## R2: Sidecar File Management

**Decision**: JSON sidecar file at `.opencode/session-<id>.json` with atomic writes (temp file + rename) and 0600 permissions.

**Rationale**: Each subprocess invocation is stateless; session state (dedup cache, observation count) must persist across invocations within a session. A sidecar file is the simplest approach that works with any invocation model. Atomic writes prevent corruption if the process crashes mid-write. 0600 permissions match the memory file security posture (SEC-2).

**Implementation Details**:
- **Write path**: Serialize to temp file in same directory (`.opencode/.tmp-<random>`), then `std::fs::rename()` to target path. Rename is atomic on POSIX filesystems.
- **Read path**: `std::fs::read_to_string()` → `serde_json::from_str()`. On deserialization failure, delete corrupt file and create fresh state (WARN trace).
- **Directory creation**: `std::fs::create_dir_all(".opencode/")` with permissions set via `std::fs::set_permissions()`.
- **Temp file**: Use `tempfile::NamedTempFile` in the `.opencode/` directory for safe atomic write.

**Alternatives Considered**:
- SQLite: Rejected — heavyweight dependency for simple key-value state
- Memory-mapped file: Rejected — complex, no benefit for <100KB files
- Environment variables: Rejected — can't persist structured state across subprocess invocations
- Embed in .mv2 file: Rejected — couples session state to memory storage; different lifecycle

---

## R3: Fail-Open Pattern Implementation

**Decision**: `handle_with_failopen<F>` wrapper that catches both `Result::Err` and panics, returning a valid default `HookOutput` and emitting `tracing::warn!`.

**Rationale**: PRD M-5 requires that no plugin operation ever blocks the developer's workflow. The wrapper uses `std::panic::catch_unwind(AssertUnwindSafe(..))` to catch panics and standard `Result` matching for errors. This guarantees valid JSON output on stdout regardless of internal failures.

**Implementation Details**:
```rust
fn handle_with_failopen<F>(handler: F) -> HookOutput
where F: FnOnce() -> Result<HookOutput, RustyBrainError>
{
    match std::panic::catch_unwind(AssertUnwindSafe(|| handler())) {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "handler failed, fail-open");
            HookOutput::default()
        }
        Err(_panic) => {
            tracing::warn!("handler panicked, fail-open");
            HookOutput::default()
        }
    }
}
```

**Key Considerations**:
- `HookOutput::default()` serializes to `{}` (all fields `None`), which is a valid no-op response
- For `MindToolOutput`, fail-open returns `{ success: false, error: "internal error" }` — structured error, not silent
- WARN traces go to stderr only, never interfering with stdout JSON protocol
- `AssertUnwindSafe` is required because handler closures capture mutable references; this is safe because we don't continue using the state after a panic

**Alternatives Considered**:
- Let panics propagate (crash): Rejected — violates M-5; crashes block OpenCode
- Return exit code instead of JSON: Rejected — OpenCode expects structured output; non-zero exit without JSON is worse than fail-open JSON
- Log to file instead of stderr: Rejected — adds file I/O; stderr is the convention (matches 004-tool-output-compression)

---

## R4: Deduplication Hash Strategy

**Decision**: `std::collections::hash_map::DefaultHasher` with `tool_name + summary` as the hash key, stored as 16-byte hex string in the sidecar.

**Rationale**: The dedup cache prevents storing the same tool+summary observation twice within a session (M-4). `DefaultHasher` is fast, available in std (no new dependency), and sufficient for session-scoped collision avoidance. The hash does not need to be cryptographic — it's an optimization, not a security boundary.

**Implementation Details**:
```rust
fn compute_dedup_hash(tool_name: &str, summary: &str) -> String {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    tool_name.hash(&mut hasher);
    summary.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
```

**LRU Eviction**:
- Vec-based: `dedup_hashes: Vec<String>` with max 1024 entries
- New entry appended to end (most recent)
- On capacity: `remove(0)` evicts oldest entry (front of Vec)
- Duplicate check: `contains()` is O(n) but acceptable at n=1024 (~16KB of hex strings)
- On duplicate hit, move existing entry to end (refresh LRU position)

**Alternatives Considered**:
- `lru` crate: Rejected — adds dependency for trivial data structure at n=1024 (Constitution XIII)
- SHA-256: Rejected — overkill; `DefaultHasher` provides sufficient collision resistance for 1024 entries
- Store full tool_name+summary: Rejected — increases sidecar size and sensitivity (SEC-4)
- No dedup: Rejected — violates M-4; would create redundant observations

---

## R5: Mind API Integration Patterns

**Decision**: Each handler creates its own `Mind` instance via `Mind::open(config)` or `Mind::open_read_only(config)`, using `MindConfig` resolved from environment and path resolution.

**Rationale**: Each subprocess invocation is independent — no shared state between invocations. The `get_mind()` singleton is designed for long-lived processes (like the hooks crate) and would require process-level lifecycle management that doesn't apply here.

**Usage by Handler**:

| Handler | Mind Access | Locking | APIs Used |
|---------|------------|---------|-----------|
| chat_hook | `Mind::open(config)` | `mind.with_lock()` | `get_context(query)` |
| tool_hook | `Mind::open(config)` | `mind.with_lock()` | `remember(obs_type, tool_name, summary, content, metadata)` |
| mind_tool (search) | `Mind::open_read_only(config)` | `mind.with_lock()` | `search(query, limit)` |
| mind_tool (ask) | `Mind::open_read_only(config)` | `mind.with_lock()` | `ask(question)` |
| mind_tool (recent) | `Mind::open_read_only(config)` | `mind.with_lock()` | `timeline(limit, true)` |
| mind_tool (stats) | `Mind::open_read_only(config)` | `mind.with_lock()` | `stats()` |
| mind_tool (remember) | `Mind::open(config)` | `mind.with_lock()` | `remember(obs_type, tool_name, summary, content, metadata)` |
| session_cleanup | `Mind::open(config)` | `mind.with_lock()` | `save_session_summary(decisions, files, summary)` |

**MindConfig Resolution**:
1. `MindConfig::from_env()` — reads env vars (`MEMVID_MIND_DEBUG`, etc.)
2. Override `memory_path` with result from `resolve_memory_path(cwd, "opencode", false)`
3. Result: config pointing to `.agent-brain/mind.mv2` in the project directory

---

## R6: Context Injection Format

**Decision**: Chat hook returns `HookOutput` with `system_message` containing formatted memory context (human-readable text) and `hook_specific_output` containing the structured `InjectedContext` JSON.

**Rationale**: The `HookOutput.system_message` field is designed for injecting text into the AI agent's system prompt. Formatted text is immediately useful to any agent. The `hook_specific_output` carries the raw structured data for agents that want to parse it programmatically.

**Format of `system_message`**:
```
# 🧠 Memory Context

## Recent Observations
- [Discovery] (2026-03-03 10:15) tool_name: summary text
- [Decision] (2026-03-03 09:30) tool_name: summary text

## Relevant Memories
- [Pattern] (2026-03-02 14:20) tool_name: summary text (score: 0.85)

## Session Summaries
- Session 2026-03-02: "Implemented authentication module" (12 observations)

📁 Project: **project-name**
💾 Memory: `.agent-brain/mind.mv2` (1234 KB)
```

**Alternatives Considered**:
- JSON-only in `system_message`: Rejected — less readable for the AI agent
- Structured data only in `hook_specific_output`: Rejected — many agents only read `system_message`
- Custom field on HookOutput: Rejected — AR says DO NOT MODIFY `crates/types`

---

## R7: Session ID Handling

**Decision**: Use `session_id` from `HookInput` if provided. If empty or missing, generate a UUID-based session ID and use it for the sidecar filename.

**Rationale**: PRD Q2 is unresolved — we don't know if OpenCode passes a session ID. Handling both cases ensures the plugin works regardless of OpenCode's behavior. The sidecar file is named `session-<id>.json` where `<id>` is the session ID (sanitized for filesystem safety).

**Sanitization**: Replace non-alphanumeric characters (except `-` and `_`) with `-` to prevent path traversal or filesystem issues.

---

## R8: Orphan Cleanup Strategy

**Decision**: On session start, scan `.opencode/` for `session-*.json` files with mtime > 24 hours and delete them. Fail-open on any error.

**Rationale**: Sessions that terminate abnormally (crash, kill) leave orphaned sidecar files. Scanning on session start is self-healing with zero operational overhead (no background process, no cron job). The 24-hour TTL ensures active sessions aren't accidentally cleaned up even during long-running work.

**Implementation Details**:
- Use `std::fs::read_dir(".opencode/")` to list files
- Filter by pattern: filename starts with `session-` and ends with `.json`
- Check `metadata.modified()` — if older than 24 hours, delete
- On any I/O error (read_dir, metadata, delete): `tracing::warn!` and continue scanning
- Do NOT recurse into subdirectories (SEC-12)
