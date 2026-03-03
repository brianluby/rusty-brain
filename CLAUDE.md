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
- `.agent-brain/mind.mv2` — Memvid-encoded persistent memory database

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
- Rust (stable, edition 2024, rust-version 1.85.0) + `regex` crate (new), workspace `tracing` (for WARN-level fallback logging) (004-tool-output-compression)
- N/A — pure text transformation library, no persistence (004-tool-output-compression)

## Recent Changes
- 004-tool-output-compression: Added Rust (stable, edition 2024, rust-version 1.85.0) + `regex` crate (new), workspace `tracing` (for WARN-level fallback logging)
