# Developer Guide

This guide covers building, testing, and extending rusty-brain. For end-user setup, see [README.md](../README.md).

## Build & Test

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests
cargo fmt --check                # Check formatting
cargo clippy --workspace -- -D warnings  # Lint (must pass with zero warnings)
```

## Quality Gates

All of the following must pass before merge:

1. `cargo test --workspace` — all tests green
2. `cargo clippy --workspace -- -D warnings` — no lint violations
3. `cargo fmt --check` — formatting compliant
4. Agent integration smoke test — CLI commands produce valid structured output

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

## Memory Path Policy

Canonical runtime memory-path resolution is owned by `platforms::resolve_memory_path(...)`.

- Default mode (`MEMVID_PLATFORM_PATH_OPT_IN` unset): `.rusty-brain/mind.mv2`
- Platform opt-in mode (`MEMVID_PLATFORM_PATH_OPT_IN=1`): `.{platform}/mind-{platform}.mv2`
- Explicit override (`MEMVID_PLATFORM_MEMORY_PATH`) takes precedence over policy resolution
- CLI override (`--memory-path`) takes precedence over all environment-based resolution

### Migration Notes

- Older builds could resolve platform paths differently across entrypoints; current behavior is unified across CLI, hooks, and OpenCode.
- Legacy Claude path `.claude/mind.mv2` is still detected and surfaced in startup messaging so existing data can be migrated intentionally.
- If you previously stored memories in legacy or pre-unification locations, copy/merge into the currently resolved canonical path shown by startup output (or set `MEMVID_PLATFORM_MEMORY_PATH` during transition).

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

## CLI JSON Output Contract

All CLI subcommands (`find`, `ask`, `stats`, `timeline`) accept a `--json` flag that emits machine-parseable output on stdout. When adding a new subcommand:

- Implement a `--json` flag via clap
- Output a single JSON object or array to stdout; never mix prose and JSON
- All structured types live in `crates/types/src/`
- Test JSON output in `crates/cli/src/` using `assert_cmd` + `serde_json`

## Local Development Tips

Run a subset of tests to speed up the feedback loop:

```bash
cargo test --lib                  # Unit tests only
cargo test --test '*'             # Integration tests only
cargo test -p rusty-brain-core    # Single crate
cargo test <test_name>            # Single test by name
```

Override the memory path during development without touching your real memory file:

```bash
MEMVID_PLATFORM_MEMORY_PATH=/tmp/test.mv2 cargo run -p cli -- find "auth"
```

## Spec-Driven Development

All features follow a mandatory pipeline before code is written:

1. **Specify** — feature spec from natural language description
2. **PRD** — MoSCoW requirements and prioritized user stories
3. **Architecture** — options analysis and technical approach
4. **Security** — attack surface and CIA impact
5. **Plan** — tech stack, data models, API contracts
6. **Tasks** — dependency-ordered implementation tasks
7. **Analyze** — cross-artifact consistency check
8. **Implement** — test-first execution of tasks

Feature artifacts live in `specs/<NNN>-<feature-name>/`. See `CLAUDE.md` for slash commands that drive each stage.
