# Quickstart: OpenCode Plugin Adapter

**Feature**: 008-opencode-plugin | **Date**: 2026-03-03

---

## Prerequisites

- Rust stable (edition 2024, MSRV 1.85.0)
- Existing workspace builds: `cargo build --workspace`
- Existing tests pass: `cargo test --workspace`

## Build

```bash
# Build the full workspace (includes crates/opencode and crates/cli)
cargo build --workspace

# Build just the opencode crate (for faster iteration)
cargo build -p opencode

# Build the CLI binary (includes opencode subcommands)
cargo build -p rusty-brain-cli
```

## Test

```bash
# Run all tests
cargo test --workspace

# Run only opencode crate tests
cargo test -p opencode

# Run a specific test
cargo test -p opencode sidecar_test

# Run with verbose output (shows tracing)
RUST_LOG=debug cargo test -p opencode -- --nocapture

# Lint
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --check
```

## Manual Verification

### Chat Hook

```bash
# Simulate a chat hook event (requires a .agent-brain/mind.mv2 file)
echo '{"session_id":"test-001","transcript_path":"","cwd":"/path/to/project","permission_mode":"","hook_event_name":"chat_start"}' \
  | cargo run -p rusty-brain-cli -- opencode chat-hook
```

Expected output: JSON with `system_message` containing memory context (or empty `{}` if no memory file).

### Tool Hook

```bash
# Simulate a tool execution event
echo '{"session_id":"test-001","transcript_path":"","cwd":"/path/to/project","permission_mode":"","hook_event_name":"post_tool_use","tool_name":"read","tool_response":{"content":"file contents here"}}' \
  | cargo run -p rusty-brain-cli -- opencode tool-hook
```

Expected output: `{}` (success, continue execution). A sidecar file appears at `/path/to/project/.opencode/session-test-001.json`.

### Mind Tool

```bash
# Search memories
echo '{"mode":"search","query":"authentication"}' \
  | cargo run -p rusty-brain-cli -- opencode mind

# Get stats
echo '{"mode":"stats"}' \
  | cargo run -p rusty-brain-cli -- opencode mind

# Store a memory
echo '{"mode":"remember","content":"Important project decision: use JWT for auth"}' \
  | cargo run -p rusty-brain-cli -- opencode mind

# Invalid mode (should return structured error)
echo '{"mode":"invalid"}' \
  | cargo run -p rusty-brain-cli -- opencode mind
```

### Session Cleanup

```bash
# Simulate session deletion (reads HookInput from stdin)
echo '{"session_id":"test-001","transcript_path":"","cwd":"/path/to/project","permission_mode":"","hook_event_name":"session_end"}' \
  | cargo run -p rusty-brain-cli -- opencode session-cleanup
```

### Orphan Cleanup

```bash
# Simulate session start (triggers stale file cleanup, reads HookInput from stdin)
echo '{"session_id":"new-session","transcript_path":"","cwd":"/path/to/project","permission_mode":"","hook_event_name":"session_start"}' \
  | cargo run -p rusty-brain-cli -- opencode session-start
```

## Project Structure

```text
crates/opencode/src/
├── lib.rs                 # Public API, fail-open wrapper
├── types.rs               # MindToolInput, MindToolOutput
├── sidecar.rs             # Session state persistence, LRU dedup
├── chat_hook.rs           # Context injection handler
├── tool_hook.rs           # Observation capture handler
├── mind_tool.rs           # Mind tool mode dispatcher
└── session_cleanup.rs     # Session cleanup handler

crates/cli/src/
├── args.rs                # Extended with Opencode subcommand
├── main.rs                # Extended with opencode dispatch
└── opencode_cmd.rs        # NEW: stdin/stdout I/O for opencode subcommands
```

## Key Conventions

1. **No stdin/stdout I/O in crates/opencode** — handlers accept parsed structs, return output structs
2. **Fail-open** — all errors caught and converted to valid JSON output; WARN traces to stderr
3. **No `deny_unknown_fields`** — input types accept unknown JSON fields for forward compatibility
4. **No `get_mind()` singleton** — each handler creates its own `Mind::open(config)` instance
5. **Atomic sidecar writes** — temp file + rename; 0600 permissions
6. **No memory content at INFO+ log level** — Constitution IX compliance

## Dependencies Added

In `crates/opencode/Cargo.toml`:
```toml
[dependencies]
rusty-brain-core = { path = "../core" }
types = { path = "../types" }
platforms = { path = "../platforms" }
compression = { path = "../compression" }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
chrono = { workspace = true }
tempfile = { workspace = true }

[dev-dependencies]
tracing-subscriber = { workspace = true }
```

In `crates/cli/Cargo.toml`:
```toml
[dependencies]
# Add:
opencode = { path = "../opencode" }
```
