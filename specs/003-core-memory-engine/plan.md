# Implementation Plan: Core Memory Engine

**Branch**: `003-core-memory-engine` | **Date**: 2026-03-01 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-core-memory-engine/spec.md`

## Summary

Implement the `Mind` struct — the core memory engine for rusty-brain — within `crates/core` using a trait-layered architecture. The `MemvidBackend` trait abstracts `memvid-core` operations, `FileGuard` handles corruption detection and backup management, and `ContextBuilder` assembles token-budgeted context payloads. All types flow from `crates/types`; all errors are `RustyBrainError` variants with stable codes.

## Technical Context

**Language/Version**: Rust (stable), edition 2024, MSRV 1.85.0
**Primary Dependencies**: memvid-core (pinned git rev `fbddef4`), ulid, fs2
**Storage**: memvid `.mv2` files on local filesystem
**Testing**: cargo test (unit + integration), criterion benchmarks for perf targets
**Target Platform**: macOS (darwin), Linux — local-only, no network
**Project Type**: Single Rust crate within workspace (`crates/core`)
**Performance Goals**: Store <500ms, Search <500ms, Context assembly <2s, Stats <2s at 10K observations
**Constraints**: No `unsafe` (workspace-level `forbid`), no network, no memory content in logs at INFO+, `Send + Sync` for Mind
**Scale/Scope**: 10K observations baseline, 100MB file size guard

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First Architecture | ✅ Pass | All work within existing `crates/core`; no new crates |
| II. Rust-First Implementation | ✅ Pass | Stable Rust; memvid isolated behind `MemvidBackend` trait |
| III. Agent-Friendly Interface | ✅ Pass | All output structured; no interactive prompts |
| IV. Contract-First Development | ✅ Pass | Contracts defined in this plan before implementation |
| V. Test-First Development | ✅ Pass | Tests authored before implementation per plan |
| VI. Complete Requirement Delivery | ✅ Pass | All Must Have requirements covered by executable tasks |
| VII. Memory Integrity | ✅ Pass | Atomic writes via memvid `commit()`; corruption detection; validated on load |
| VIII. Performance Discipline | ✅ Pass | Measurable targets defined at 10K observations |
| IX. Security-First | ✅ Pass | No content logging at INFO+; local-only; SEC-1..SEC-9 mapped |
| X. Error Handling Standards | ✅ Pass | `RustyBrainError` with stable codes for all operations |
| XI. Observability | ✅ Pass | `tracing` spans per public method; structured diagnostics |
| XII. Simplicity | ✅ Pass | Single crate, 6 modules, well-understood Rust patterns |
| XIII. Dependency Policy | ✅ Pass | New deps: `ulid` (required by clarification), `fs2` (required by FR-014) |

## Project Structure

### Documentation (this feature)

```text
specs/003-core-memory-engine/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── mind-api.rs      # Rust trait/struct contract definitions
├── ar.md                # Architecture Review
├── sec.md               # Security Review
├── prd.md               # Product Requirements Document
├── spec.md              # Feature specification
└── tasks.md             # Implementation tasks
```

### Source Code (repository root)

```text
crates/core/
├── Cargo.toml
└── src/
    ├── lib.rs              # Public re-exports: Mind, estimate_tokens, get_mind, reset_mind
    ├── mind.rs             # Mind struct — public API facade
    ├── backend.rs          # MemvidBackend trait + internal types (SearchHit, TimelineEntry, etc.)
    ├── memvid_store.rs     # MemvidStore — production MemvidBackend impl wrapping memvid-core
    ├── file_guard.rs       # FileGuard — pre-open validation, backup, size guard
    ├── context_builder.rs  # ContextBuilder — token-budgeted context assembly
    └── token.rs            # estimate_tokens() pure function

crates/core/tests/
├── integration/
│   ├── mind_roundtrip.rs   # Store → search → verify all fields
│   ├── mind_context.rs     # Context assembly with real .mv2
│   ├── mind_recovery.rs    # Corruption detection and recovery
│   └── mind_concurrent.rs  # Multi-process locking
└── common/
    └── mod.rs              # Shared test helpers (temp dirs, fixtures)
```

**Structure Decision**: Single crate (Option 1 from AR) with internal modules using `pub(crate)` visibility. Aligns with Constitution I (Crate-First) and XII (Simplicity).

## Complexity Tracking

No constitution violations requiring justification. All complexity is directly traceable to PRD requirements:

| Added Complexity | Why Needed | PRD Requirement |
|-----------------|------------|-----------------|
| `MemvidBackend` trait | Memvid isolation + mock testing | Constitution II, V |
| `FileGuard` module | Pre-open validation pipeline | S-4, S-5, S-6 |
| `ContextBuilder` module | Multi-query + token budgeting logic | M-5 |
| `ulid` dependency | Observation IDs sortable by time | Clarification session |
| `fs2` dependency | Cross-process file locking | C-1 (FR-014) |
