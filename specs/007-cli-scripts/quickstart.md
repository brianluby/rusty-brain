# Quickstart: CLI Scripts (007)

**Date**: 2026-03-02 | **Branch**: `007-cli-scripts`

## Prerequisites

- Rust stable (MSRV 1.85.0)
- Existing workspace builds: `cargo build --workspace`

## Build

```bash
# Build the CLI binary
cargo build -p cli

# Build in release mode (stripped, optimized)
cargo build -p cli --release
```

The binary is produced at `target/debug/rusty-brain` (or `target/release/rusty-brain`).

## Run

```bash
# Show help
cargo run -p cli -- --help

# Search memories
cargo run -p cli -- find "authentication"
cargo run -p cli -- find "error" --limit 5 --type decision --json

# Ask a question
cargo run -p cli -- ask "What database changes were made?"
cargo run -p cli -- ask "What patterns were discovered?" --json

# View statistics
cargo run -p cli -- stats
cargo run -p cli -- stats --json

# View timeline
cargo run -p cli -- timeline
cargo run -p cli -- timeline --limit 20 --oldest-first --type discovery --json

# Override memory path
cargo run -p cli -- --memory-path /path/to/memory.mv2 find "auth"

# Verbose mode (debug tracing to stderr)
cargo run -p cli -- -v find "auth"
```

## Test

```bash
# Run all tests (workspace-wide)
cargo test

# Run CLI tests only
cargo test -p cli

# Run core tests only (includes Mind::timeline() tests)
cargo test -p rusty-brain-core

# Run a specific test
cargo test -p cli test_find_json_output
```

## Quality Gates

All must pass before merge:

```bash
cargo test                              # All tests green
cargo clippy --workspace -- -D warnings # No lint warnings
cargo fmt --check                       # Formatting compliant
```

## Development Workflow

1. **Core API extension first**: Add `Mind::timeline()` to `crates/core/src/mind.rs`
2. **Args module**: Define clap structs in `crates/cli/src/args.rs`
3. **Output module**: Implement formatting in `crates/cli/src/output.rs`
4. **Commands module**: Wire subcommand logic in `crates/cli/src/commands.rs`
5. **Main orchestrator**: Connect everything in `crates/cli/src/main.rs`

## Key File Locations

| File | Purpose |
|------|---------|
| `crates/cli/Cargo.toml` | CLI crate dependencies |
| `crates/cli/src/main.rs` | Binary entry point, orchestration |
| `crates/cli/src/args.rs` | Clap argument definitions |
| `crates/cli/src/commands.rs` | Subcommand execution logic |
| `crates/cli/src/output.rs` | JSON and table formatting |
| `crates/core/src/mind.rs` | Mind API (search, ask, stats, timeline) |
| `crates/types/src/observation.rs` | ObservationType enum |
| `crates/types/src/stats.rs` | MindStats struct |
| `crates/types/src/config.rs` | MindConfig struct |
