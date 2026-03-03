# Implementation Plan: Platform Adapter System

**Branch**: `005-platform-adapter-system` | **Date**: 2026-03-01 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/005-platform-adapter-system/spec.md`

## Summary

Implement a multi-platform adapter system in the existing `platforms` crate that normalizes raw hook input from Claude Code and OpenCode into typed platform events, validates contract compatibility, resolves project identity for memory isolation, determines memory file paths via policy, and records structured diagnostics. The system uses a trait-based adapter pattern with a registry for extensibility, and applies fail-open semantics throughout (skip with diagnostics, never block the agent).

## Technical Context

**Language/Version**: Rust 1.85.0 (stable, edition 2024)
**Primary Dependencies**: serde/serde_json (serialization), uuid (event IDs), chrono (timestamps), semver (contract version parsing), thiserror (errors). All already in workspace `Cargo.toml`.
**Storage**: N/A — diagnostics are in-memory only; memory path resolution produces paths but does not perform I/O
**Testing**: `cargo test` — unit tests co-located in modules, integration tests in `tests/`
**Target Platform**: Cross-platform (macOS, Linux) — local CLI tool
**Project Type**: Single workspace with multiple crates
**Performance Goals**: Sub-5ms per event normalization + pipeline processing (pure in-memory, no filesystem I/O — identity resolution uses string-based path cleaning, not `std::fs::canonicalize()`)
**Constraints**: No `unsafe` code (workspace lint: `unsafe_code = "forbid"`). No network. No interactive prompts. Fail-open on all validation failures.
**Scale/Scope**: 2 built-in adapters (Claude, OpenCode), unlimited custom adapters via registry. Event volume: ~100s per session.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Crate-First Architecture | PASS | All work in existing `platforms` crate; new types that serve broader use (PlatformEvent, ProjectContext) go in `types` crate |
| II | Rust-First Implementation | PASS | Stable Rust only; no unsafe; memvid not involved in this feature |
| III | Agent-Friendly Interface | PASS | All output is structured (typed Rust structs with serde); no interactive prompts |
| IV | Contract-First Development | PASS | Trait definitions and data models designed before implementation (this plan) |
| V | Test-First Development | PASS | Tests will be authored before implementation per task ordering |
| VI | Complete Requirement Delivery | PASS | All 23 FRs + 8 SCs mapped to implementation modules |
| VII | Memory Integrity | N/A | This feature does not write to the memory store |
| VIII | Performance Discipline | PASS | Sub-5ms implicit target; no filesystem I/O in hot path (string-based path cleaning); no speculative benchmarks |
| IX | Security-First Design | PASS | Path traversal prevention (FR-014), no sensitive data in diagnostics (FR-022), platform name sanitization (FR-016) |
| X | Error Handling Standards | PASS | Machine-parseable error codes via existing AgentBrainError; new `E_PLATFORM_*` codes |
| XI | Observability | PASS | Structured DiagnosticRecord for all skip/error conditions |
| XII | Simplicity & Pragmatism | PASS | Uses existing Rust patterns (enums, Result, traits); shared factory convenience for built-in adapters |
| XIII | Dependency Policy | PASS | All dependencies already in workspace; no new external crates |

**Gate result: ALL PASS — proceed to Phase 0.**

**Quality gate note**: The constitution quality gate "Agent integration smoke test — CLI commands produce valid structured output" is N/A for this feature. Feature 005 is a library crate (`platforms`) with no CLI commands; it exposes Rust APIs consumed by downstream crates. The three applicable gates (`cargo test`, `cargo clippy`, `cargo fmt`) are enforced in tasks.md T029.

## Project Structure

### Documentation (this feature)

```text
specs/005-platform-adapter-system/
├── spec.md              # Feature specification (done)
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── platform_adapter.rs    # PlatformAdapter trait definition
│   ├── adapter_registry.rs    # AdapterRegistry trait/API
│   └── event_pipeline.rs      # EventPipeline API contract
├── tasks.md             # Phase 2 output (/speckit.tasks)
├── quickstart.md        # Usage examples and API walkthrough
└── checklists/          # Quality checklists
    └── requirements.md  # FR/SC coverage checklist
```

**Note**: prd.md, ar.md, and sec.md were omitted for this feature. The spec.md serves as the combined requirements/architecture document, and security considerations (path traversal prevention, diagnostic redaction) are covered directly in spec.md sections and the constitution check above.

### Source Code (repository root)

```text
crates/types/src/
├── lib.rs                  # Add re-exports for new platform types
├── error.rs                # E_PLATFORM_* error codes, Platform variant (MODIFIED)
├── platform_event.rs       # PlatformEvent, EventKind (NEW)
├── project_context.rs      # ProjectContext, ProjectIdentity, IdentitySource (NEW)
├── diagnostic.rs           # DiagnosticRecord, DiagnosticSeverity (NEW)
└── contract_version.rs     # ContractValidationResult (NEW)

crates/platforms/
├── Cargo.toml              # Add dependencies: types, serde, uuid, chrono, semver
└── src/
    ├── lib.rs              # Module declarations, re-exports
    ├── adapter.rs          # PlatformAdapter trait + factory fn (NEW)
    ├── registry.rs         # AdapterRegistry (HashMap-based) (NEW)
    ├── detection.rs        # detect_platform() function (NEW)
    ├── identity.rs         # resolve_project_identity() (NEW)
    ├── path_policy.rs      # resolve_memory_path() (NEW)
    ├── pipeline.rs         # EventPipeline::process() (NEW)
    ├── adapters/
    │   ├── mod.rs          # Built-in adapter module (NEW)
    │   ├── claude.rs       # Claude Code adapter (NEW)
    │   └── opencode.rs     # OpenCode adapter (NEW)
    └── contract.rs         # validate_contract(), SUPPORTED_CONTRACT_MAJOR (NEW)
```

**Structure Decision**: Two-crate split. Pure data types (PlatformEvent, ProjectContext, DiagnosticRecord, ContractValidationResult) go in `types` for cross-crate reuse. Behavior (adapter trait, registry, detection, pipeline, path policy) goes in `platforms`. This follows the existing pattern where `types` has no business logic and `platforms` depends on `types`.

## Complexity Tracking

No constitution violations requiring justification.
