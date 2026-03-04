# Research: Claude Code Hooks

**Feature Branch**: `006-claude-code-hooks`
**Date**: 2026-03-03

## R-1: Logging Strategy — tracing-subscriber for RUSTY_BRAIN_LOG

**Decision**: Use `tracing-subscriber` with `EnvFilter` gated on `RUSTY_BRAIN_LOG` environment variable, outputting to stderr.

**Rationale**: `tracing` is already a workspace dependency. `tracing-subscriber` is its standard companion for log output. The `EnvFilter` component reads env vars natively, making `RUSTY_BRAIN_LOG=debug` work out of the box. When unset, no subscriber is installed and all tracing macros become no-ops (zero overhead).

**Alternatives considered**:
- `env_logger`: Simpler but doesn't integrate with `tracing` macros already used in `crates/core`. Would require two logging systems.
- Custom stderr writer: More code to maintain, no structured logging, no level filtering.
- No logging: Rejected — violates Constitution XI (Observability/Debuggability).

**Implementation**: In `main.rs`, conditionally initialize:
```rust
if std::env::var("RUSTY_BRAIN_LOG").is_ok() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("RUSTY_BRAIN_LOG"))
        .with_writer(std::io::stderr)
        .init();
}
```

---

## R-2: Dedup Cache — File-based with std::hash::DefaultHasher

**Decision**: Use a sidecar JSON file (`.agent-brain/.dedup-cache.json`) with `std::hash::DefaultHasher` for generating dedup keys.

**Rationale**: Each hook invocation is a separate process — no shared in-memory state. A file-based cache is the only option for cross-invocation dedup. `DefaultHasher` is sufficient because:
1. The dedup cache is ephemeral (60s TTL, auto-pruned on every read)
2. This is not a security hash — it's a fast equality check
3. `DefaultHasher` is available in `std` with no extra dependencies
4. Hash instability across Rust versions is harmless given the 60s TTL

**Alternatives considered**:
- `blake3` or `sha256`: Cryptographic strength unnecessary; adds dependency (violates XIII)
- `FNV` / `SipHash`: SipHash is actually what `DefaultHasher` uses on most platforms; FNV adds a dependency
- In-memory HashMap: Lost between process invocations — doesn't work for subprocess model
- No dedup: Would store redundant observations for repeated tool calls (violates M-6)

**Cache format**:
```json
{
  "entries": {
    "14823947291": 1709490000,
    "98234729341": 1709490005
  }
}
```
Keys are `DefaultHasher` u64 hashes (as strings); values are Unix timestamps (seconds).

**Concurrency**: Atomic writes via temp file + rename. Worst case on race: a duplicate observation is stored (harmless).

---

## R-3: Git Subprocess Integration — Command with Timeout

**Decision**: Shell out to `git diff --name-only HEAD` using `std::process::Command` with a 5-second timeout implemented via a spawned child process and `wait_with_timeout` pattern.

**Rationale**: Git detection is only needed in the `stop` hook. Using the git CLI directly is the simplest approach — no library dependency needed. The 5-second timeout prevents hanging if git is slow or stuck.

**Alternatives considered**:
- `git2` (libgit2 bindings): Significant dependency, complex build (C library), overkill for a single `diff --name-only`
- `gix` (gitoxide): Pure Rust but heavy; not in workspace; not justified for one command
- No timeout: Risk of the hook hanging indefinitely if git is stuck (violates M-3 fail-open)

**Implementation pattern**:
```rust
fn detect_modified_files(cwd: &Path) -> Vec<String> {
    let child = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    // Wait with 5s timeout, parse stdout lines, return empty Vec on any error
}
```

**Hardcoded arguments** (SEC-9): `"diff"`, `"--name-only"`, `"HEAD"` are string literals. `cwd` from HookInput is used as working directory, not as a command argument. The platform layer already validates paths don't contain traversal components.

---

## R-4: Content Truncation — Head/Tail Strategy

**Decision**: Implement head/tail truncation targeting ~500 tokens using a chars/4 approximation.

**Rationale**: LLM-based compression is deferred (W-1). Head/tail preserves the beginning (context/intent) and end (result/conclusion) of tool output, which are typically the most useful parts. The middle (verbose output) is replaced with a `[...truncated...]` marker.

**Alternatives considered**:
- Simple `chars::take(n)`: Loses the end of output, which often contains the result
- Line-based truncation: More complex, same quality trade-off
- No truncation: Memory bloat; long observations waste storage and retrieval budget

**Token estimation**: `chars / 4` is a widely-used rough approximation for English text and code. Precise tokenization would require a tokenizer dependency. The approximation is acceptable because:
1. The 500-token target is itself approximate
2. Stored observations are later retrieved by the Mind API which has its own token budget
3. Slightly over/under is harmless

**Split ratio**: 60% head / 40% tail. Rationale: the beginning of tool output typically contains the command/file path (critical context), while the end contains results. The middle is most likely to be verbose/repetitive.

---

## R-5: Fail-Open Boundary — catch_unwind + Result Conversion

**Decision**: Wrap all handler dispatch in `std::panic::catch_unwind` and convert any `Result::Err` or panic into a valid `HookOutput { continue: true }`.

**Rationale**: M-3 requires that no hook invocation ever blocks the host agent. The fail-open boundary is a single point in `main.rs` that guarantees:
1. Panics in handler code don't produce non-JSON output
2. Errors in handler code produce valid `HookOutput`
3. Exit code is always 0

**Implementation**:
```
main():
  1. Parse clap (if fails → exit 0 with empty JSON)
  2. Init logging (if RUSTY_BRAIN_LOG set)
  3. result = catch_unwind(|| read_input + dispatch)
  4. output = match result {
       Ok(Ok(output)) => output,
       Ok(Err(e))     => { log error; HookOutput::fail_open() }
       Err(panic)     => { log panic; HookOutput::fail_open() }
     }
  5. write_output(&output) // if this fails, write "{}" as last resort
  6. exit(0)
```

**Alternatives considered**:
- Per-handler try/catch: Duplicates the boundary; risk of missing a path
- `process::exit(1)` on error: Violates M-3 (exit code must always be 0)
- No catch_unwind: Panics would print to stderr and produce non-JSON output

---

## R-6: Hooks Manifest Format — hooks.json

**Decision**: Generate a static `hooks.json` that maps Claude Code hook event types to the `rusty-brain` binary with subcommand arguments.

**Rationale**: Claude Code discovers hooks via a JSON manifest file. The manifest maps event types (`SessionStart`, `PostToolUse`, `Stop`, `Notification`) to commands.

**Format** (based on Claude Code hook registration schema):
```json
{
  "hooks": {
    "SessionStart": [
      {
        "type": "command",
        "command": "rusty-brain session-start"
      }
    ],
    "PostToolUse": [
      {
        "type": "command",
        "command": "rusty-brain post-tool-use"
      }
    ],
    "Stop": [
      {
        "type": "command",
        "command": "rusty-brain stop"
      }
    ],
    "Notification": [
      {
        "type": "command",
        "command": "rusty-brain smart-install",
        "matcher": "smart-install"
      }
    ]
  }
}
```

**Note**: The exact format depends on Claude Code's hook registration schema. The `manifest.rs` module will generate this. Binary path can be made configurable for different install locations.

---

## R-7: Dependencies to Add to crates/hooks/Cargo.toml

**Decision**: Add the following workspace dependencies to `crates/hooks`:

| Dependency | Source | Justification |
|-----------|--------|---------------|
| `types` (path) | Workspace crate | HookInput, HookOutput, ObservationType, MindConfig, InjectedContext |
| `core` (path) | Workspace crate | Mind API (get_mind, reset_mind) |
| `platforms` (path) | Workspace crate | detect_platform, resolve_memory_path, resolve_project_identity |
| `serde` | Workspace | Derive for dedup cache types |
| `serde_json` | Workspace | JSON read/write for stdin/stdout and dedup cache |
| `tracing` | Workspace | Diagnostic logging macros |
| `tracing-subscriber` (new) | crates.io | EnvFilter for RUSTY_BRAIN_LOG |
| `chrono` | Workspace | Timestamps in dedup cache |
| `thiserror` | Workspace | HookError derive |

**Dev dependencies**:
| Dependency | Source | Justification |
|-----------|--------|---------------|
| `tempfile` | Workspace | Temporary directories for integration tests |
| `assert_cmd` (new) | crates.io | Binary subprocess testing for E2E tests |
| `predicates` (new) | crates.io | Assertion helpers for subprocess output |

**Alternatives considered**:
- Fewer deps: Could skip `assert_cmd`/`predicates` and use raw `Command` for E2E tests, but these crates are standard for Rust CLI testing and improve test readability significantly.
- `tracing-subscriber` already used by `crates/core` implicitly via `tracing` — adding it as direct dependency is justified by M-10.

---

## R-8: System Message Formatting

**Decision**: Format the `systemMessage` returned by `session-start` as a structured markdown-like string containing recent observations, session summaries, stats, and available commands.

**Rationale**: The `InjectedContext` from `Mind::get_context()` returns structured data (observations, summaries, token count). This needs to be formatted into a single string for the `systemMessage` field of `HookOutput`. The format should be human-readable (for debugging) but primarily consumed by the AI agent.

**Format template**:
```
# [brain] Claude Mind Active

[project_path] | [observation_count] memories | [session_count] sessions

## Recent Context
[formatted observations from InjectedContext.recent_observations]

## Previous Sessions
[formatted summaries from InjectedContext.session_summaries]

## Commands
- /mind:search <query> - Search memories
- /mind:ask <question> - Ask your memory
- /mind:recent - View timeline
- /mind:stats - View statistics
```

This matches the existing memvid-mind-context format already used by the JavaScript agent-brain implementation.
