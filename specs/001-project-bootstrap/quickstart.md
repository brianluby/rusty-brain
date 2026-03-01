# Quickstart: Project Bootstrap

**Feature**: 001-project-bootstrap
**Date**: 2026-03-01

## Prerequisites

- Rust toolchain 1.85.0 or later (`rustup update stable`)
- Git

## Build & Test

```bash
# Clone the repository
git clone <repo-url>
cd rusty-brain

# Build the entire workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Check formatting
cargo fmt --check

# Run linter
cargo clippy --workspace -- -D warnings

# Release build
cargo build --workspace --release
```

## Verify Individual Crates

Each crate can be built and tested in isolation:

```bash
cargo build -p core
cargo test -p core

cargo build -p types
cargo test -p types

cargo build -p platforms
cargo test -p platforms

cargo build -p compression
cargo test -p compression

cargo build -p hooks
cargo test -p hooks

cargo build -p cli
cargo test -p cli

cargo build -p opencode
cargo test -p opencode
```

## Crate Layout

| Crate | Type | Purpose |
|-------|------|---------|
| `core` | Library | Memory engine (Mind) |
| `types` | Library | Shared types and errors |
| `platforms` | Library | Platform adapter system |
| `compression` | Library | Tool-output compression |
| `hooks` | Binary | Claude Code hook binaries |
| `cli` | Binary | CLI scripts (find, ask, stats, timeline) |
| `opencode` | Library | OpenCode editor adapter |

## Adding a Dependency

1. Add the dependency to `[workspace.dependencies]` in the root `Cargo.toml`
2. In the crate that needs it, add `dependency_name = { workspace = true }` under `[dependencies]`
3. Run `cargo build --workspace` to verify

## Adding a New Crate

1. Create directory under `crates/`: `mkdir -p crates/new-crate/src`
2. Create `crates/new-crate/Cargo.toml` with workspace inheritance
3. Create entry point: `crates/new-crate/src/lib.rs` (or `main.rs` for binary)
4. The workspace auto-discovers it via `members = ["crates/*"]`
5. Run `cargo build --workspace` to verify
