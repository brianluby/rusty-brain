# rusty-brain

Rust rewrite of [agent-brain](https://github.com/brianluby/agent-brain/) — a memory system for AI coding agents (Claude Code, OpenCode, etc.). Uses [memvid](https://github.com/brianluby/memvid) for video-encoded memory storage and retrieval.

## Installation

### Binary

```bash
# macOS / Linux
curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh | sh

# Windows (PowerShell)
irm https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.ps1 | iex
```

### Claude Code Plugin

```
/plugin marketplace add brianluby/rusty-brain
/plugin install rusty-brain@rusty-brain
```

## Prerequisites

- Rust 1.85.0 or later (`rustup update stable`)
- Git

## Build & Test

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests
cargo fmt --check                # Check formatting
cargo clippy --workspace -- -D warnings  # Lint (must pass with zero warnings)
```

## Quality Gates

All of the following must pass before merge:

1. `cargo fmt --check` — formatting compliant
2. `cargo clippy --workspace -- -D warnings` — no lint violations
3. `cargo test --workspace` — all tests green
4. `cargo build --workspace --release` — release build succeeds

## Memory Path Policy

Canonical runtime memory-path resolution is owned by `platforms::resolve_memory_path(...)`.

- Default mode (`MEMVID_PLATFORM_PATH_OPT_IN` unset): `.agent-brain/mind.mv2`
- Platform opt-in mode (`MEMVID_PLATFORM_PATH_OPT_IN=1`): `.{platform}/mind-{platform}.mv2`
- Explicit override (`MEMVID_PLATFORM_MEMORY_PATH`) takes precedence over policy resolution
- CLI override (`--memory-path`) takes precedence over all environment-based resolution

### Migration Notes

- Older builds could resolve platform paths differently across entrypoints; current behavior is unified across CLI, hooks, and OpenCode.
- Legacy Claude path `.claude/mind.mv2` is still detected and surfaced in startup messaging so existing data can be migrated intentionally.
- If you previously stored memories in legacy or pre-unification locations, copy/merge into the currently resolved canonical path shown by startup output (or set `MEMVID_PLATFORM_MEMORY_PATH` during transition).

## Crate Layout

| Crate | Type | Description |
|-------|------|-------------|
| `core` | Library | Memory engine (Mind) |
| `types` | Library | Shared types and errors |
| `platforms` | Library | Platform adapter system |
| `compression` | Library | Tool-output compression |
| `hooks` | Binary | Claude Code hook binaries |
| `cli` | Binary | CLI scripts (find, ask, stats, timeline) |
| `opencode` | Library | `OpenCode` editor adapter |

## MSRV Policy

- **Edition**: 2024
- **Minimum Supported Rust Version**: 1.85.0
- Enforced via `rust-version` in `Cargo.toml` and `rust-toolchain.toml`

## Adding a New Dependency

1. Add the dependency to `[workspace.dependencies]` in the root `Cargo.toml`
2. In the crate that needs it, add `dependency_name = { workspace = true }` under `[dependencies]`
3. Run `cargo build --workspace` to verify

## Adding a New Crate

1. Create a directory under `crates/`: `mkdir -p crates/new-crate/src`
2. Create `crates/new-crate/Cargo.toml` with workspace inheritance:
   ```toml
   [package]
   name = "new-crate"
   version = "0.1.0"
   edition.workspace = true
   rust-version.workspace = true
   license.workspace = true

   [dependencies]

   [lints]
   workspace = true
   ```
3. Create the entry point: `crates/new-crate/src/lib.rs` (or `main.rs` for a binary)
4. The workspace auto-discovers it via `members = ["crates/*"]`
5. Run `cargo build --workspace` to verify

## License

Apache-2.0
