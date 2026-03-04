# Quickstart: Claude Code Hooks

**Feature Branch**: `006-claude-code-hooks`

## Prerequisites

- Rust stable 1.85.0+
- Existing workspace builds: `cargo build --workspace`
- Git (optional, for stop hook file detection)

## Build

```bash
# Build the hooks binary
cargo build -p hooks

# Verify binary exists
./target/debug/rusty-brain --help
```

## Test Individual Hooks

### Session Start

```bash
echo '{"session_id":"test-123","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"SessionStart"}' | ./target/debug/rusty-brain session-start
```

Expected: JSON with `systemMessage` containing memory context (or welcome message if no `.mv2` file exists).

### Post Tool Use

```bash
echo '{"session_id":"test-123","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{"file_path":"src/main.rs"},"tool_response":{"content":"fn main() {}"}}' | ./target/debug/rusty-brain post-tool-use
```

Expected: JSON with `continue: true`.

### Stop

```bash
echo '{"session_id":"test-123","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"Stop"}' | ./target/debug/rusty-brain stop
```

Expected: JSON with session summary in `systemMessage`.

### Smart Install

```bash
echo '{"session_id":"test-123","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"Notification"}' | ./target/debug/rusty-brain smart-install
```

Expected: JSON with `continue: true`. Creates `.install-version` file.

## Enable Debug Logging

```bash
RUSTY_BRAIN_LOG=debug echo '...' | ./target/debug/rusty-brain session-start
```

Diagnostic output goes to stderr; JSON output remains clean on stdout.

## Run Tests

```bash
# All hooks tests
cargo test -p hooks

# Specific test
cargo test -p hooks -- test_session_start_with_existing_memory

# With debug output
RUSTY_BRAIN_LOG=debug cargo test -p hooks -- --nocapture
```

## Quality Gates

```bash
cargo test --workspace                    # All tests green
cargo clippy --workspace -- -D warnings   # No lint warnings
cargo fmt --check                         # Formatting compliant
```

## Hook Registration

Copy the generated `hooks.json` to your Claude Code settings directory:

```bash
# Generate manifest (after binary is built)
# The manifest points to the rusty-brain binary location
cp hooks.json ~/.claude/hooks.json
```

## File Layout After Installation

```
project-root/
├── .agent-brain/
│   ├── mind.mv2              # Encrypted memory file (created on first session-start)
│   └── .dedup-cache.json     # Dedup cache (auto-managed, 60s TTL)
├── .install-version          # Version marker (created by smart-install)
└── ...
```
