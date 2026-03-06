# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

rusty-brain is a Rust rewrite of [agent-brain](https://github.com/brianluby/agent-brain/) — a memory system for AI coding agents (claude-code, opencode, etc.). It uses [memvid](https://github.com/brianluby/memvid) for video-encoded memory storage and retrieval. The project is currently in the specification phase with no Rust source code yet.

## Build & Quality Commands

```bash
cargo build                         # Build the project
cargo test                          # Run all tests
cargo test <test_name>              # Run a single test by name
cargo test --lib                    # Unit tests only
cargo test --test '*'               # Integration tests only
cargo clippy -- -D warnings         # Lint (must pass with zero warnings)
cargo fmt --check                   # Check formatting
cargo fmt                           # Auto-format
```

**Quality gates (all must pass before merge):**
1. `cargo test` — all green
2. `cargo clippy -- -D warnings` — no warnings
3. `cargo fmt --check` — formatted
4. Agent integration smoke test — CLI commands produce valid structured output

## Architecture

### Spec-Driven Development (Specify Workflow)

All features follow this mandatory pipeline before code is written:

1. **Specify** (`/speckit.specify`) — Feature spec from natural language description
2. **PRD** (`/speckit.prd`) — MoSCoW requirements and prioritized user stories
3. **Architecture** (`/speckit.architecture`) — Options analysis and technical approach
4. **Security** (`/speckit.security`) — Attack surface, CIA impact, data classification
5. **Plan** (`/speckit.plan`) — Tech stack, data models, API contracts
6. **Tasks** (`/speckit.tasks`) — Dependency-ordered implementation tasks
7. **Analyze** (`/speckit.analyze`) — Cross-artifact consistency check
8. **Implement** (`/speckit.implement`) — Test-first execution of tasks

Feature artifacts live in `specs/<NNN>-<feature-name>/` with: `spec.md`, `prd.md`, `plan.md`, `ar.md`, `sec.md`, `tasks.md`, `research.md`, `data-model.md`, `contracts/`, `checklists/`.

### Key Directories

- `.specify/` — Workflow engine: config, bash scripts, markdown templates, constitution
- `.specify/memory/constitution.md` — 13 core principles governing all development (non-negotiable)
- `.claude/commands/` — Speckit slash commands for Claude Code
- `.rusty-brain/mind.mv2` — Memvid-encoded persistent memory database

### Git Mode

Configured for **worktree** mode (`.specify/config.json`): features are developed in parallel using separate `.claude/worktrees/` directories. Feature branches follow the naming pattern `NNN-feature-name`.

## Constitution Highlights

The constitution (`.specify/memory/constitution.md` v2.0.0) governs all implementation. Key principles:

- **Crate-First**: New features go in existing crate layout unless explicitly justified
- **Rust-First**: Stable Rust only; `unsafe` requires architecture justification and sign-off
- **Agent-Friendly**: All output must be structured (JSON/TOML), no interactive prompts in agent code paths
- **Contract-First**: Define trait/API contracts before implementation
- **Test-First (Non-Negotiable)**: Tests authored before implementation; memory round-trip tests required for any storage path
- **Memory Integrity**: Atomic writes, no silent corruption, validated indices on load
- **Memvid Pinning**: memvid version must be pinned in `Cargo.toml`; upgrades require round-trip correctness testing

## Rust Patterns to Follow

- Enums for state, `Result<T, E>` for errors, traits for extension points
- Machine-parseable errors with stable error codes
- Isolate memvid behind clean Rust abstractions (traits/wrappers) so upstream changes don't ripple
- No network by default — any remote capability must be opt-in
- No logging of memory contents at INFO or above without explicit opt-in

## Active Technologies
- Rust stable, edition 2024, MSRV 1.85.0
- memvid-core (pinned git rev `fbddef4`); upgrades require round-trip correctness testing
- memvid `.mv2` files on local filesystem (read-only access via `Mind` API in CLI)
- ulid, fs2 (003-core-memory-engine)
- serde, serde_json, uuid, chrono, semver, thiserror (005-platform-adapter-system)
- Rust stable, edition 2024, MSRV 1.85.0 + clap 4 (subcommand dispatch), serde/serde_json (JSON protocol), tracing (diagnostics), existing workspace crates (core, types, platforms) (006-claude-code-hooks)
- `.rusty-brain/mind.mv2` (memvid-encrypted observations), `.rusty-brain/.dedup-cache.json` (hash-based dedup), `.rusty-brain/.install-version` (version marker) (006-claude-code-hooks)
- clap 4 (derive), tracing 0.1 (007-cli-scripts)
- `regex` crate (new), workspace `tracing` (for WARN-level fallback logging) (004-tool-output-compression)
- Workspace crates (`core`, `platforms`, `compression`, `types`), `serde`, `serde_json`, `tracing`, `chrono`, `tempfile` (regular dep for atomic sidecar writes) (008-opencode-plugin)
- `.rusty-brain/mind.mv2` (via `crates/core` Mind API), `.opencode/session-<id>.json` (sidecar files on local filesystem) (008-opencode-plugin)
- Rust stable, edition 2024, MSRV 1.85.0 (binary already built by workspace). Shell scripts: POSIX sh + PowerShell 5.1+. + cross-rs (CI only, for Linux musl cross-compilation), `houseabsolute/actions-rust-cross` GitHub Action. No new Rust crate dependencies. (009-plugin-packaging)
- N/A (no new storage; existing `.mv2` files are preserved, never touched). (009-plugin-packaging)
- Rust stable, edition 2024, MSRV 1.85.0 + memvid-core (pinned rev `fbddef4`), criterion 0.5 (benchmarks), cargo-fuzz/libFuzzer (fuzz testing), serde/serde_json (fixture parsing), tempfile (test isolation), assert_cmd/predicates (CLI tests) (010-testing-migration)
- `.mv2` files via memvid-core (read/write compatibility), `tests/fixtures/` (committed test data) (010-testing-migration)
- Rust stable, edition 2024, MSRV 1.85.0 + serde, serde_json, fs2, chrono (all existing workspace deps) (012-default-memory-path)
- `.mv2` files on local filesystem, `.dedup-cache.json`, `.install-version` (012-default-memory-path)

All crates already present in workspace `Cargo.toml`. Diagnostics are in-memory only; memory path resolution produces paths but does not perform I/O.

## Recent Changes
- 003-core-memory-engine: Added memvid-core (pinned git rev `fbddef4`), ulid, fs2
- 005-platform-adapter-system: Wired the `platforms` crate to use `types`, serde/serde_json, uuid, chrono, semver, and thiserror already present in the workspace `Cargo.toml`, and added `temp-env` as a dev-dependency.
- 004-tool-output-compression: Added `regex` crate, workspace `tracing` for WARN-level fallback logging
