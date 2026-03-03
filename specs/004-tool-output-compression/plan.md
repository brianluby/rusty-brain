# Implementation Plan: Tool-Output Compression

**Branch**: `004-tool-output-compression` | **Date**: 2026-03-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/004-tool-output-compression/spec.md`

## Summary

Port the intelligent tool-output compression system from the TypeScript agent-brain implementation to Rust, implementing it within the existing `crates/compression` crate. The system uses a function-based dispatcher to route tool outputs to specialized compressors (Read, Bash, Grep, Glob, Edit/Write) or a generic fallback, producing budget-compliant compressed text using regex-based pattern matching for language construct extraction.

Architecture decision: **Option 1 — Function-Based Dispatcher** (from AR). Plain functions with `match`-based routing, no traits or dynamic dispatch.

## Technical Context

**Language/Version**: Rust (stable, edition 2024, rust-version 1.85.0)
**Primary Dependencies**: `regex` crate (new), workspace `tracing` (for WARN-level fallback logging)
**Storage**: N/A — pure text transformation library, no persistence
**Testing**: `cargo test` (unit + integration); property-based tests for budget guarantee
**Target Platform**: Cross-platform (Linux, macOS, Windows) — no platform-specific code
**Project Type**: Workspace crate within existing multi-crate Rust project
**Performance Goals**: < 5ms for 10,000-character input (SC-006)
**Constraints**: Synchronous only, `unsafe_code = "forbid"`, Unicode char counting, no content logging at INFO+
**Scale/Scope**: 6 specialized compressors + 1 generic fallback, ~10 source modules, ~2,000 lines total

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Evidence |
|-----------|--------|----------|
| I. Crate-First Architecture | ✅ Pass | Implementation in existing `crates/compression` skeleton; no new crate needed |
| II. Rust-First Implementation | ✅ Pass | Stable Rust only; `unsafe_code = "forbid"` at workspace level; no memvid boundary (no memvid dependency) |
| III. Agent-Friendly Interface | ✅ Pass | Library crate with structured `CompressedResult` return type; no interactive prompts; no CLI surface |
| IV. Contract-First Development | ✅ Pass | Interface contract defined in PRD and AR (CompressionConfig, CompressedResult, compress() signature) |
| V. Test-First Development | ✅ Pass | Testing strategy in AR; TDD workflow mandated per project conventions |
| VI. Complete Requirement Delivery | ✅ Pass | All 13 Must-Have + 5 Should-Have requirements traced in AR traceability matrix |
| VII. Memory Integrity | ✅ N/A | Compression crate has no storage/persistence; integrity is the pipeline's concern |
| VIII. Performance Discipline | ✅ Pass | SC-006: < 5ms target; measurable via `cargo bench` with criterion |
| IX. Security-First Design | ✅ Pass | No network, no secret storage; content not logged at INFO+ per constitution IX |
| X. Error Handling Standards | ✅ Pass | Infallible public API (M-13); internal errors caught + fallback; WARN-level log with context |
| XI. Observability | ✅ Pass | `CompressionStatistics` returned; DEBUG logging for dispatch; WARN for fallback triggers |
| XII. Simplicity | ✅ Pass | Function-based dispatcher (AR Option 1); no traits, no dynamic dispatch, no over-engineering |
| XIII. Dependency Policy | ✅ Pass | Single new dep: `regex` (MIT/Apache-2.0, >100M downloads); justified by M-8, M-9 |

**Gate result: PASS** — No violations. No complexity tracking entries needed.

## Project Structure

### Documentation (this feature)

```text
specs/004-tool-output-compression/
├── spec.md              # Feature specification (with clarifications)
├── prd.md               # Product Requirements Document
├── ar.md                # Architecture Review
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── compression.rs   # Rust trait/type contract definitions
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
crates/compression/
├── Cargo.toml           # Add regex + tracing deps
└── src/
    ├── lib.rs           # Entry point: compress(), re-exports, threshold gate, dispatch
    ├── config.rs        # CompressionConfig with Default impl
    ├── types.rs         # CompressedResult, CompressionStatistics, ToolType enum
    ├── truncate.rs      # enforce_budget() — shared final truncation
    ├── generic.rs       # Generic fallback compressor (head/tail)
    ├── read.rs          # File-read compressor
    ├── lang.rs          # Per-language regex patterns, construct extraction
    ├── bash.rs          # Bash output compressor
    ├── grep.rs          # Grep result compressor
    ├── glob.rs          # Glob result compressor
    └── edit.rs          # Edit/Write compressor
```

**Structure Decision**: Single crate with flat module layout per AR Option 1. Each compressor is a separate module for testability and the 400-line module limit. No sub-directories — all modules at `src/` level.

## Complexity Tracking

No violations to justify. Architecture is the simplest option that satisfies all Must-Have requirements (see AR Simplest Implementation Comparison).

## Phase 0: Research Findings

All unknowns resolved. See [research.md](research.md) for details.

Key decisions:
1. **Regex crate**: Use `regex` (not `regex-lite`) for full Unicode support and `LazyLock` compatibility
2. **Panic recovery**: Use `std::panic::catch_unwind` for compressor error boundaries
3. **Logging**: Use workspace `tracing` crate for structured logging at WARN/DEBUG levels
4. **Character counting**: `.chars().count()` consistently (not `.len()`)
5. **Construct patterns**: Port TypeScript regex patterns with Rust regex syntax adjustments

## Phase 1: Design Artifacts

### Data Model

See [data-model.md](data-model.md) for full entity definitions.

Core types:
- `CompressionConfig` — threshold + budget, `Default` impl, validation
- `ToolType` — enum with `From<&str>` for case-insensitive matching
- `CompressedResult` — text + flag + original_size + optional statistics
- `CompressionStatistics` — ratio, chars_saved, percentage_saved

### Contracts

See [contracts/compression.rs](contracts/compression.rs) for Rust type definitions.

Public API surface:
```rust
pub fn compress(
    config: &CompressionConfig,
    tool_name: &str,
    output: &str,
    input_context: Option<&str>,
) -> CompressedResult;
```

### Implementation Order (from AR)

1. `config.rs` + `types.rs` — data structures
2. `truncate.rs` — budget enforcer
3. `generic.rs` — fallback compressor
4. `lib.rs` — dispatcher with threshold gate + error boundary
5. `lang.rs` + `read.rs` — file-read compressor (P1, most complex)
6. `bash.rs` — bash compressor (P1)
7. `grep.rs` — grep compressor (P2)
8. `glob.rs` — glob compressor (P2)
9. `edit.rs` — edit/write compressor (P2)

### Testing Approach

- TDD workflow: write test → verify fail → implement → verify pass
- Unit tests per module (in-module `#[cfg(test)]`)
- Integration tests in `lib.rs` (end-to-end dispatch)
- Property test: no output exceeds `config.target_budget` for any input
- Benchmark: criterion bench for 10K-char inputs against 5ms target

## Constitution Re-check (Post Phase 1 Design)

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First | ✅ | All work in `crates/compression` |
| II. Rust-First | ✅ | No `unsafe`; stable Rust only |
| III. Agent-Friendly | ✅ | Structured types, no prompts |
| IV. Contract-First | ✅ | contracts/compression.rs produced |
| V. Test-First | ✅ | Testing strategy documented |
| VI. Complete Delivery | ✅ | All M-* and S-* requirements mapped |
| VII. Memory Integrity | N/A | No storage |
| VIII. Performance | ✅ | 5ms target; benchmark planned |
| IX. Security-First | ✅ | No network; no content logging |
| X. Error Handling | ✅ | Infallible API; structured fallback |
| XI. Observability | ✅ | Statistics + tracing |
| XII. Simplicity | ✅ | Function-based, no over-engineering |
| XIII. Dependencies | ✅ | Only `regex` + existing `tracing` |

**Post-design gate: PASS**
