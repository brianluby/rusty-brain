# Implementation Plan: OpenCode Plugin Adapter

**Branch**: `008-opencode-plugin` | **Date**: 2026-03-03 | **Spec**: [specs/008-opencode-plugin/spec.md](spec.md)
**Input**: Feature specification from `/specs/008-opencode-plugin/spec.md`

## Summary

Implement an OpenCode plugin adapter as a library crate (`crates/opencode`) with handler modules for chat hooks (context injection), tool hooks (observation capture with deduplication), a native mind tool (5 modes), and session lifecycle management. The existing `crates/cli` binary gains an `opencode` subcommand group that reads JSON from stdin and writes JSON to stdout. Session state (dedup cache) persists via a file-backed sidecar (`.opencode/session-<id>.json`). All errors fail-open with WARN-level tracing to stderr.

**Architecture**: Option 1 from AR — Library + CLI Subcommands (single binary, library/binary split for testability).

## Technical Context

**Language/Version**: Rust stable, edition 2024, MSRV 1.85.0
**Primary Dependencies**: Workspace crates (`core`, `platforms`, `compression`, `types`), `serde`, `serde_json`, `tracing`, `chrono`
**Storage**: `.agent-brain/mind.mv2` (via `crates/core` Mind API), `.opencode/session-<id>.json` (sidecar files on local filesystem)
**Testing**: `cargo test` (unit + integration); `cargo clippy -- -D warnings`; `cargo fmt --check`
**Target Platform**: Local CLI (macOS, Linux) — subprocess invoked by OpenCode editor
**Project Type**: Rust workspace — library crate + CLI binary extension
**Performance Goals**: Chat hook context injection <200ms p95, Tool observation capture <100ms p95 (including sidecar I/O)
**Constraints**: No new external crates (Constitution XIII), stdin/stdout JSON protocol with stderr for tracing, fail-open on all errors (M-5), no `deny_unknown_fields` on input types (M-7), no memory content logged at INFO+ (Constitution IX)
**Scale/Scope**: Single-user local tool, 1024-entry bounded LRU dedup cache, session-scoped sidecar files with 24h orphan TTL

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Evidence |
|---|-----------|--------|----------|
| I | Crate-First Architecture | PASS | Uses existing `crates/opencode` scaffold; extends existing `crates/cli` binary. No new crates created. |
| II | Rust-First Implementation | PASS | Stable Rust only; no `unsafe` code. memvid isolated behind `crates/core` Mind API. |
| III | Agent-Friendly Interface Design | PASS | JSON stdin/stdout protocol; no interactive prompts; machine-readable errors with stable codes. |
| IV | Contract-First Development | PASS | Handler function signatures, `MindToolInput`/`MindToolOutput` types, and sidecar state schema defined before implementation (see `contracts/`). |
| V | Test-First Development | PASS | Tests authored before implementation; test strategy covers sidecar (95%), handlers (90%), fail-open (100%). Non-negotiable. |
| VI | Complete Requirement Delivery | PASS | All M-1..M-8 Must Have requirements have acceptance criteria (AC-1..AC-18) and will have executable tasks. |
| VII | Memory Integrity and Data Safety | PASS | Atomic writes (temp + rename) for sidecar files; `Mind::with_lock` for .mv2 cross-process locking; corrupted sidecar detected and recreated. |
| VIII | Performance and Scope Discipline | PASS | Measurable targets: chat hook <200ms, tool hook <100ms. Integration tests with timer assertions. |
| IX | Security-First Design | PASS | SEC-2..SEC-12 mapped from sec.md. No memory content logged at INFO+. Local-only, no network. Sidecar files 0600 permissions. |
| X | Error Handling Standards | PASS | Fail-open wrapper returns valid JSON on any error/panic; errors include stable codes from `RustyBrainError`; WARN traces to stderr. |
| XI | Observability and Debuggability | PASS | `tracing::warn!` for all fail-open errors; no silent failures. `--verbose` / `RUSTY_BRAIN_DEBUG=1` for DEBUG level is deferred (PRD C-2, Could Have). |
| XII | Simplicity and Pragmatism | PASS | Extends existing CLI patterns (clap, tracing, JSON output); Vec-based LRU instead of external `lru` crate; 5 focused handler modules. |
| XIII | Dependency Policy | PASS | No new external crates. All dependencies (`serde`, `serde_json`, `tracing`, `chrono`, `tempfile`) already in workspace. |

**Gate Result**: ALL PASS — no violations to justify. Proceed to Phase 0.

## Project Structure

### Documentation (this feature)

```text
specs/008-opencode-plugin/
├── spec.md              # Feature specification (existing)
├── prd.md               # Product requirements document (existing)
├── ar.md                # Architecture review (existing)
├── sec.md               # Security review (existing)
├── plan.md              # This file (/speckit.plan command output)
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── handler_api.rs   # Public handler function signatures
│   ├── types.rs         # MindToolInput, MindToolOutput, SidecarState
│   └── sidecar_api.rs   # Sidecar module public API
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
crates/opencode/
├── Cargo.toml                    # Dependencies: core, types, platforms, compression, serde, serde_json, tracing, chrono
├── src/
│   ├── lib.rs                    # Public API re-exports, fail-open wrapper utility
│   ├── types.rs                  # MindToolInput, MindToolOutput (OpenCode-specific types)
│   ├── sidecar.rs                # SidecarState, load/save (atomic), LRU hash management, stale cleanup
│   ├── chat_hook.rs              # handle_chat_hook — context injection via Mind::get_context
│   ├── tool_hook.rs              # handle_tool_hook — observation capture with dedup via sidecar
│   ├── mind_tool.rs              # handle_mind_tool — mode dispatch to Mind search/ask/timeline/stats/remember
│   └── session_cleanup.rs        # handle_session_cleanup — summary generation, sidecar deletion
└── tests/
    ├── sidecar_test.rs           # Load/save, LRU eviction, hash, stale cleanup, atomic writes, corrupt file
    ├── chat_hook_test.rs         # Context injection, empty memory, error paths
    ├── tool_hook_test.rs         # Observation capture, dedup, compression, sidecar update
    ├── mind_tool_test.rs         # All 5 modes, invalid mode, empty results
    ├── session_cleanup_test.rs   # Summary generation, sidecar deletion, empty session
    └── failopen_test.rs          # Error recovery, panic recovery, valid JSON guaranteed

crates/cli/
├── Cargo.toml                    # Add dependency: opencode = { path = "../opencode" }
├── src/
│   ├── main.rs                   # Extended: add opencode subcommand dispatch
│   ├── args.rs                   # Extended: add Opencode variant to Command enum
│   ├── opencode_cmd.rs            # NEW: OpenCode subcommand handlers (stdin/stdout I/O, fail-open catch-all)
│   └── commands.rs               # Existing (unchanged)

# Static file (location TBD by Spike-1)
plugin-manifest.json              # OpenCode plugin manifest for discovery/registration
```

**Structure Decision**: Follows AR Option 1 (Library + CLI Subcommands). The `crates/opencode` library contains pure handler logic accepting parsed structs and returning output structs — no stdin/stdout I/O. The `crates/cli` binary handles all I/O and dispatches to the library. This enables comprehensive unit testing of handler logic without I/O mocking.

## Complexity Tracking

No constitution violations to justify. All implementation choices are the minimum needed for PRD requirements:

- 5 handler modules map 1:1 to PRD capabilities (M-1, M-2, M-3, M-4/S-2, S-1)
- Library/binary split is the minimum for testability (Constitution V)
- Sidecar LRU + orphan cleanup is the minimum for PRD M-4 + S-2
- Fail-open wrapper is the minimum for PRD M-5

## Key Design Decisions (from AR)

| Decision | Rationale | AR Reference |
|----------|-----------|--------------|
| Library + CLI subcommands (not dedicated binary) | Single binary, reuses CLI patterns, same testability | Option 1 selected |
| `MindToolInput`/`MindToolOutput` in `crates/opencode` (not `crates/types`) | OpenCode-specific types; don't pollute shared types crate | Technical Constraints |
| Vec-based LRU (not `lru` crate) | Trivial at n=1024; avoids new dependency | Anti-patterns |
| `system_message` field for context injection | `HookOutput.system_message` carries formatted memory context; `hook_specific_output` carries structured `InjectedContext` JSON | Component Overview |
| `Mind::open()` per invocation (not `get_mind()` singleton) | Each subprocess invocation is independent | Implementation Guardrails |
| Atomic writes (temp + rename) for sidecar | Prevents corruption on crash; Constitution VII | Key Algorithms |
| DefaultHasher for dedup hashes | Stable within process; sufficient for session-scoped dedup | Key Algorithms |

## Implementation Guardrails (from AR)

> These are non-negotiable constraints for the implementation phase:

- [ ] **DO NOT** use `deny_unknown_fields` on any input struct (M-7 forward compatibility)
- [ ] **DO NOT** use `get_mind()` singleton — create `Mind::open(config)` per invocation
- [ ] **DO NOT** log memory contents at INFO level or above (Constitution IX)
- [ ] **DO NOT** add interactive prompts or `eprintln!` in library code (Constitution III)
- [ ] **DO NOT** add new external crates without explicit justification (Constitution XIII)
- [ ] **DO NOT** read/write stdin/stdout in `crates/opencode` — I/O belongs in `crates/cli`
- [ ] **MUST** wrap all handler entry points in fail-open catch-all (M-5)
- [ ] **MUST** use atomic writes (temp + rename) for sidecar updates (Constitution VII)
- [ ] **MUST** use `resolve_memory_path(cwd, "opencode", false)` for LegacyFirst (M-6)
- [ ] **MUST** emit errors via `tracing::warn!` to stderr (M-5, Constitution XI)
- [ ] **MUST** write tests before implementation (Constitution V, non-negotiable)
- [ ] **MUST** create sidecar files with 0600 permissions (SEC-2)
- [ ] **MUST** validate mind tool mode against whitelist (SEC-8)

## Security Requirements (from sec.md)

| SEC ID | Requirement | Verification |
|--------|-------------|--------------|
| SEC-2 | Sidecar files created with 0600 permissions | Unit test |
| SEC-3 | No memory content logged at INFO+ | Unit test + code review |
| SEC-4 | Sidecar contains only dedup hashes, not raw content | Unit test |
| SEC-5 | No API keys/tokens/secrets stored or logged | Code review |
| SEC-6 | All stdin JSON validated via serde typed deserialization | Unit test |
| SEC-7 | No `deny_unknown_fields`; unknown fields don't influence logic | Unit test |
| SEC-8 | Mind tool mode validated against fixed whitelist | Unit test |
| SEC-9 | File paths validated via `resolve_memory_path` (no traversal) | Already enforced by platforms crate |
| SEC-10 | Fail-open emits WARN for all suppressed errors | Integration test |
| SEC-11 | Sidecar atomic writes via temp + rename | Unit test |
| SEC-12 | Orphan cleanup only deletes `session-*.json` in `.opencode/` | Unit test |

## Suggested Implementation Order (from AR)

1. **Sidecar module** — SidecarState, load/save atomic, LRU hash, stale cleanup, dedup hash
2. **Types** — MindToolInput, MindToolOutput
3. **Chat hook handler** — context injection via Mind::get_context
4. **Tool hook handler** — observation capture with dedup via sidecar
5. **Mind tool handler** — mode dispatch to Mind APIs
6. **Session cleanup handler** — summary generation, sidecar deletion
7. **Fail-open wrapper** — `handle_with_failopen` utility in lib.rs
8. **CLI subcommands** — clap subcommand definitions, stdin/stdout I/O, handler dispatch
9. **Plugin manifest** — static JSON file for OpenCode discovery

## Testing Strategy

| Layer | Test Type | Coverage | Key Scenarios |
|-------|-----------|----------|---------------|
| Sidecar | Unit | 95% | Load/save roundtrip, LRU eviction at boundary (1024), hash computation determinism, stale cleanup (>24h deleted, <24h preserved), atomic write survives concurrent access, corrupt file recovery |
| Chat hook | Unit + Integration | 90% | Context injection with known memory, empty/new memory file, error path fail-open, topic-relevant query |
| Tool hook | Unit + Integration | 90% | New observation stored, duplicate detected and skipped, large output compressed, sidecar updated, error path fail-open |
| Mind tool | Unit | 90% | All 5 modes with known data, invalid mode returns error with valid modes list, empty results |
| Session cleanup | Unit + Integration | 85% | Summary stored with observation count, sidecar deleted, empty session handled, error path fail-open |
| Fail-open | Unit | 100% | Error → valid JSON, panic → valid JSON, WARN trace emitted for both |
| CLI integration | Integration | Key paths | stdin/stdout roundtrip for each subcommand, invalid JSON input handled |

## Constitution XI: Observability Note

WARN-level tracing to stderr satisfies the core diagnostic output requirement (no silent failures). Explicit `--verbose` / `RUSTY_BRAIN_DEBUG=1` flags are deferred to PRD C-2 (Could Have) and should be added post-MVP. The spirit of Constitution XI is met: all fail-open errors emit WARN traces, and `tracing-subscriber` is available as a dev-dep for test verification.

## Open Questions (Spike-Level, Not Blocking)

- **Q1**: OpenCode's exact plugin manifest format (PRD Spike-1)
- **Q2**: Does OpenCode pass session ID or must the plugin generate one? (Design handles both)
