# Implementation Plan: Type System & Configuration

**Branch**: `002-type-system-config` | **Date**: 2026-03-01 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/002-type-system-config/spec.md`

## Summary

Define all shared data structures, error types, and configuration for the rusty-brain memory system in `crates/types`. This is Phase 1 of the Rust roadmap — the foundational type layer that every other crate depends on. Includes 10 entity types (Observation, ObservationType, ObservationMetadata, SessionSummary, InjectedContext, MindConfig, MindStats, HookInput, HookOutput, AgentBrainError), JSON round-trip serialization, environment variable configuration resolution, stable error codes, and comprehensive test coverage.

## Technical Context

**Language/Version**: Rust (edition 2024, MSRV 1.85.0, stable toolchain)
**Primary Dependencies**: serde 1.0 (derive), serde_json 1.0, thiserror 2.0, chrono 0.4 (serde), uuid 1 (v4, serde) — all pinned in workspace Cargo.toml
**Storage**: N/A (types only, no I/O in this phase)
**Testing**: `cargo test` (unit tests in each module, integration test for cross-module round-trips)
**Target Platform**: Cross-platform (Linux, macOS, Windows) — no platform-specific code
**Project Type**: Rust workspace member (`crates/types`)
**Performance Goals**: N/A (data structures only, no runtime behavior)
**Constraints**: No `unsafe` (workspace lint: `forbid`), no new dependencies without plan approval, all types must round-trip through JSON
**Scale/Scope**: 10 entity types, ~8 source modules, ~14 functional requirements, target >90% test coverage

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Evidence |
|-----------|--------|----------|
| I. Crate-First Architecture | PASS | All types in existing `crates/types` — no new crates |
| II. Rust-First Implementation | PASS | Stable Rust only; `unsafe` forbidden by workspace lint |
| III. Agent-Friendly Interface Design | PASS | All types serialize to JSON; errors carry stable machine-parseable codes |
| IV. Contract-First Development | PASS | API contract defined in `contracts/types-api.rs` before implementation |
| V. Test-First Development | PASS (planned) | Each module will have tests written before implementation |
| VI. Complete Requirement Delivery | PASS (planned) | All 11 Must Have and 5 Should Have requirements mapped to implementation |
| VII. Memory Integrity and Data Safety | PASS | Round-trip fidelity tests cover all storage-path types |
| VIII. Performance and Scope Discipline | PASS | No performance targets in scope; no speculative benchmark work |
| IX. Security-First Design | PASS | No I/O, no network, no secret handling; memory content logging restricted |
| X. Error Handling Standards | PASS | Unified error type with stable codes, machine-parseable, cause chains |
| XI. Observability and Debuggability | PASS | Debug derives on all types; structured error output |
| XII. Simplicity and Pragmatism | PASS | Standard Rust patterns (enums, Result, derives); no clever abstractions |
| XIII. Dependency Policy | PASS | All deps already in workspace; no new deps added |

**Post-Phase-1 re-check**: All gates still pass. Contract is defined, data model is documented, no violations found.

## Project Structure

### Documentation (this feature)

```text
specs/002-type-system-config/
├── spec.md              # Feature specification
├── prd.md               # Product requirements document
├── ar.md                # Architecture review
├── sec.md               # Security review
├── plan.md              # This file
├── research.md          # Phase 0 research findings
├── data-model.md        # Phase 1 entity definitions
├── quickstart.md        # Phase 1 usage guide
├── contracts/
│   └── types-api.rs     # Phase 1 API contract
├── checklists/
│   └── requirements.md  # Spec quality checklist
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (repository root)

```text
crates/types/
├── Cargo.toml           # Dependencies: serde, serde_json, thiserror, chrono, uuid
└── src/
    ├── lib.rs           # Crate root: module declarations, re-exports
    ├── error.rs         # AgentBrainError, error_codes module
    ├── observation.rs   # ObservationType, Observation, ObservationMetadata
    ├── session.rs       # SessionSummary
    ├── context.rs       # InjectedContext
    ├── config.rs        # MindConfig (Default, from_env)
    ├── stats.rs         # MindStats
    └── hooks.rs         # HookInput, HookOutput
```

**Structure Decision**: Single crate (`crates/types`) with one module per logical domain. This follows the existing workspace layout from 001-project-bootstrap. No new crates needed — types is the designated home for shared data structures.

## Complexity Tracking

No constitution violations to justify. All implementation uses standard Rust patterns within the existing crate layout.

## Implementation Strategy

### Module Dependency Order

```text
error.rs           (no internal deps)
    ↓
observation.rs     (uses error.rs for validation)
    ↓
session.rs         (standalone, conceptual dep on observation)
    ↓
context.rs         (uses observation + session types)
    ↓
config.rs          (uses error.rs for validation)
    ↓
stats.rs           (uses observation::ObservationType)
    ↓
hooks.rs           (uses context::InjectedContext)
    ↓
lib.rs             (re-exports everything)
```

### Implementation Phases

#### Phase A: Foundation (error.rs, observation.rs)

1. **error.rs** — Error types first since other modules depend on `AgentBrainError`
   - Define `AgentBrainError` enum with 6 variants (non_exhaustive)
   - Define `error_codes` module with stable string constants
   - Implement `code()` method
   - Derive `thiserror::Error` for Display and Error traits
   - Tests: construct each variant, verify code(), verify Display, verify Error::source() chaining

2. **observation.rs** — Core data model
   - Define `ObservationType` enum (10 variants, non_exhaustive)
   - Define `Observation` struct with all fields
   - Define `ObservationMetadata` with flattened extra map
   - Serde: `rename_all = "camelCase"`, `rename = "type"` on obs_type
   - Validation: reject empty/whitespace-only summary and content at construction (return `AgentBrainError::InvalidInput`)
   - Tests: construction, validation rejection, JSON round-trip, Unicode content, deeply nested extra map

#### Phase B: Data Containers (session.rs, context.rs, stats.rs)

3. **session.rs** — Session summary
   - Define `SessionSummary` struct
   - Serde: `rename_all = "camelCase"`, explicit `rename = "filesModified"` for modified_files
   - Validation: end_time >= start_time, non-empty id, non-empty summary
   - Tests: construction, validation, JSON round-trip

4. **context.rs** — Injected context
   - Define `InjectedContext` struct
   - All fields default to empty/zero
   - Tests: construction with defaults, JSON round-trip with nested observations

5. **stats.rs** — Memory statistics
   - Define `MindStats` struct
   - Serde: `rename = "fileSize"` for file_size_bytes, `rename = "topTypes"` for type_counts
   - Tests: construction, JSON round-trip, empty store (None timestamps)

#### Phase C: Configuration (config.rs)

6. **config.rs** — Configuration with defaults and env resolution
   - Define `MindConfig` struct with `#[serde(default)]`
   - Implement `Default` with documented values
   - Implement `from_env()` reading 6 environment variables (algorithm below)
   - Implement `validate()` checking min_confidence in 0.0..=1.0, max_context_observations > 0, max_context_tokens > 0
   - Error: return `AgentBrainError::Configuration` for invalid env values
   - Tests: default values match spec, env var override for each variable, invalid env var rejection, partial JSON deserialization

   **`from_env()` resolution algorithm:**

   1. Start with `Default::default()`
   2. `MEMVID_MIND_DEBUG`: if `"1"` or `"true"` (case-insensitive) → `debug = true`; other non-empty values → `AgentBrainError::Configuration` with code `E_CONFIG_INVALID_VALUE`
   3. `MEMVID_PLATFORM_MEMORY_PATH`: if set → `memory_path = PathBuf::from(value)`
   4. `MEMVID_PLATFORM_PATH_OPT_IN`: if `"1"` AND `MEMVID_PLATFORM_MEMORY_PATH` not set:
      - Read `MEMVID_PLATFORM` for platform name (trim + lowercase)
      - If not set, check `CLAUDE_PROJECT_DIR` presence → `"claude"`
      - If not set, check `OPENCODE_PROJECT_DIR` presence → `"opencode"`
      - If platform resolved → `memory_path = .agent-brain/mind-{platform}.mv2`
   5. Call `self.validate()` and return

#### Phase D: Hook Protocol (hooks.rs)

7. **hooks.rs** — Hook input/output types
   - Define `HookInput` flat struct with optional event-specific fields
   - Define `HookOutput` with universal fields + hookSpecificOutput
   - Serde: snake_case for HookInput, mixed case for HookOutput with explicit renames
   - NO `deny_unknown_fields` on HookInput (forward compatibility)
   - Tests: parse real Claude Code hook JSON samples, unknown field tolerance (10+ unknown fields), round-trip

#### Phase E: Integration (lib.rs)

8. **lib.rs** — Crate root
   - Module declarations for all 7 modules
   - Public re-exports of all types
   - Crate-level documentation
   - Integration test: downstream crate can import all types, construct instances, serialize/deserialize

### Cargo.toml Updates

The types crate `Cargo.toml` needs these workspace dependencies added:

```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

### Key Design Decisions

| Decision | Rationale | Reference |
|----------|-----------|-----------|
| `#[non_exhaustive]` on ObservationType and AgentBrainError | Allow additive evolution without breaking downstream match | research.md R-5 |
| Flat HookInput struct (not enum) | Forward compatibility; unknown fields ignored; simpler deserialization | research.md R-1 |
| camelCase JSON for app types, snake_case for hook types | Match TypeScript impl for app types; Claude Code protocol uses snake_case | research.md R-2, R-3 |
| String error codes (`E_FS_*`, `E_CONFIG_*`, etc.) | Stable, human-readable, agent-parseable | research.md R-4 |
| ObservationMetadata redesigned from TS version | Spec intentionally adds platform/project_key/compressed, removes TS-specific fields | research.md R-2 |
| `token_count` (not `token_count_estimate`) | Match TypeScript field name for compatibility | data-model.md |
| `continue_execution` (not `continue`) | `continue` is a Rust reserved word; serde renames to `"continue"` | data-model.md |

### Quality Gates (per constitution)

All must pass before merge:

1. `cargo test -p types` — all tests green
2. `cargo clippy -p types -- -D warnings` — no warnings
3. `cargo fmt -p types --check` — formatted
4. `cargo doc -p types --no-deps` — no doc warnings
5. JSON round-trip tests pass for all 10 public types
6. Error code stability verified (codes are string constants)
7. Default values match TypeScript implementation (6/6 fields)
8. Environment variable resolution works for all 6 variables
9. HookInput tolerates 10+ unknown fields
