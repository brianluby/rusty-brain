# Implementation Plan: CLI Scripts

**Branch**: `007-cli-scripts` | **Date**: 2026-03-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/007-cli-scripts/spec.md`

## Summary

Provide developer-facing CLI tools (`find`, `ask`, `stats`, `timeline`) for read-only interaction with the rusty-brain memory system. The CLI is implemented as a thin orchestration layer in `crates/cli` using clap derive macros, delegating all data operations to the `Mind` public API from `crates/core`, with a small API extension for timeline queries. Output formatting switches between human-readable tables and JSON serialization via simple function dispatch.

## Technical Context

**Language/Version**: Rust stable, edition 2024, MSRV 1.85.0
**Primary Dependencies**: clap 4 (derive), serde/serde_json 1.0, tracing 0.1, chrono 0.4, memvid-core (pinned git rev `fbddef4`)
**New Dependencies**: tracing-subscriber 0.3 (CLI log init), comfy-table (human-readable table output)
**Storage**: `.mv2` files on local filesystem (read-only access via `Mind` API)
**Testing**: `cargo test` (unit tests per module, integration tests via `std::process::Command`)
**Target Platform**: macOS, Linux (Windows best-effort)
**Project Type**: Rust workspace, crate-per-concern (existing `crates/cli` skeleton)
**Performance Goals**: <500ms p95 CLI startup + operation (typical files <1K observations); <1s `find` at 10K observations
**Constraints**: Read-only, no interactive prompts, no network, no `unsafe`, no logging of memory content at INFO+
**Scale/Scope**: Single binary (`rusty-brain`) with 4 subcommands, ~600 lines of production code across 4 files

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Evidence |
|-----------|--------|----------|
| I. Crate-First | PASS | All code in existing `crates/cli` crate; one small addition to `crates/core` (public `Mind::timeline()`) |
| II. Rust-First | PASS | Stable Rust only; no `unsafe`; memvid isolated behind `Mind` API — CLI never touches backend directly |
| III. Agent-Friendly | PASS | `--json` on all subcommands; no interactive prompts; machine-readable exit codes (0, 1, 2) |
| IV. Contract-First | PASS | CLI argument schema + JSON output contracts defined in `contracts/`; `Mind::timeline()` contract defined before implementation |
| V. Test-First | PASS | Plan mandates tests before implementation for each module |
| VI. Complete Requirement Delivery | PASS | All M-1 through M-12 Must-Have requirements covered by planned implementation |
| VII. Memory Integrity | PASS | CLI is read-only; uses `Mind::with_lock()` for safe concurrent access |
| VIII. Performance | PASS | <500ms target measurable via integration test benchmarks |
| IX. Security-First | PASS | SEC-2 through SEC-10 mapped to implementation; no content logging at INFO+; local-only |
| X. Error Handling | PASS | Stable exit codes (0/1/2); user-friendly error messages; machine-parseable when `--json` |
| XI. Observability | PASS | `--verbose` flag enables DEBUG tracing to stderr |
| XII. Simplicity | PASS | Functions not traits; 4 small files; no premature abstractions |
| XIII. Dependency Policy | PASS | Reuse workspace deps (clap, serde, tracing); new deps (tracing-subscriber, comfy-table) justified by requirements |

**Gate result: PASS** — No violations. No complexity tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/007-cli-scripts/
├── plan.md              # This file
├── research.md          # Phase 0: Technical unknowns resolved
├── data-model.md        # Phase 1: CLI output types and upstream mapping
├── quickstart.md        # Phase 1: Build/test/run commands
├── contracts/           # Phase 1: API contracts
│   ├── cli-args.md      # Clap argument definitions
│   ├── json-output.md   # JSON output schemas for all subcommands
│   └── mind-timeline.md # Mind::timeline() public API extension
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
crates/cli/
├── Cargo.toml                 # MODIFIED: add core, types, serde_json, tracing, tracing-subscriber, comfy-table
└── src/
    ├── main.rs                # Orchestration: parse → resolve → open → dispatch → exit
    ├── args.rs                # Clap derive structs (Cli struct, Command enum)
    ├── commands.rs            # Subcommand logic (run_find, run_ask, run_stats, run_timeline)
    └── output.rs              # Output formatting (JSON via serde_json, tables via comfy-table)

crates/core/src/
└── mind.rs                    # MODIFIED: Add pub Mind::timeline() method + pub TimelineEntry struct

Cargo.toml                     # MODIFIED: Add tracing-subscriber and comfy-table to workspace deps
```

**Structure Decision**: Layered module architecture (AR Option 2). Four files in `crates/cli/src/`, each under 200 lines with a single responsibility. One new public method added to `crates/core::Mind`. No new crates created.

## Complexity Tracking

> No constitution violations — no entries needed.
