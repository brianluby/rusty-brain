# Tasks: Platform Adapter System

**Input**: Design documents from `/specs/005-platform-adapter-system/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: REQUIRED — constitution mandates test-first development (non-negotiable). Tests are co-located in same file per existing Rust convention.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing. User stories are ordered by dependency graph (not strictly by priority) to ensure prerequisites are met.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2)
- Include exact file paths in descriptions

## Path Conventions

- **Types crate**: `crates/types/src/`
- **Platforms crate**: `crates/platforms/src/`
- Tests co-located in same file (`#[cfg(test)] mod tests`)

---

## Phase 1: Setup

**Purpose**: Add dependencies and create module skeleton for the platforms crate

- [x] T001 Add workspace dependencies (types, serde, serde_json, uuid, chrono, semver, thiserror) to `crates/platforms/Cargo.toml` and add `temp-env` as dev-dependency
- [x] T002 Create module skeleton in `crates/platforms/src/lib.rs` — declare modules: `adapter`, `registry`, `detection`, `identity`, `path_policy`, `pipeline`, `contract`, `adapters` (with `adapters/mod.rs`, `adapters/claude.rs`, `adapters/opencode.rs`) — empty files with doc comments only

---

## Phase 2: Foundational Types (Blocking Prerequisites)

**Purpose**: Core type definitions in `types` crate that ALL user stories depend on. Follows existing pattern (validated constructors, serde derives, co-located tests).

**CRITICAL**: No user story work can begin until this phase is complete.

- [x] T003 [P] Add `E_PLATFORM_*` error codes (`E_PLATFORM_INCOMPATIBLE_CONTRACT`, `E_PLATFORM_INVALID_CONTRACT_VERSION`, `E_PLATFORM_MISSING_SESSION_ID`, `E_PLATFORM_MISSING_PROJECT_IDENTITY`, `E_PLATFORM_PATH_TRAVERSAL`, `E_PLATFORM_ADAPTER_NOT_FOUND`) to `crates/types/src/error.rs` — add constants to `error_codes` module, add `Platform` variant to `AgentBrainError` enum, update `code()` match arm
- [x] T004 [P] Write tests then implement `EventKind` enum (SessionStart, ToolObservation, SessionStop) and `PlatformEvent` struct in `crates/types/src/platform_event.rs` — include serde round-trip tests, construction tests, and verify all derives (Debug, Clone, PartialEq, Serialize, Deserialize)
- [x] T005 [P] Write tests then implement `ProjectContext` struct, `ProjectIdentity` struct, and `IdentitySource` enum in `crates/types/src/project_context.rs` — include serde round-trip tests, Default impl for ProjectContext, all IdentitySource variants constructable
- [x] T006 [P] Write tests then implement `ContractValidationResult` struct in `crates/types/src/contract_version.rs` — include serde round-trip tests for compatible and incompatible results
- [x] T007 [P] Write tests then implement `DiagnosticRecord` struct, `DiagnosticSeverity` enum, and `DiagnosticRecord::new()` constructor in `crates/types/src/diagnostic.rs` — tests MUST cover: field deduplication (FR-020), cap at 20 fields (FR-020), `redacted` always true (FR-022), 30-day retention (FR-021), `expires_at` computation (FR-021), UUID auto-generation, timestamp auto-generation
- [x] T008 Update `crates/types/src/lib.rs` — add module declarations (`platform_event`, `project_context`, `contract_version`, `diagnostic`) and public re-exports for all new types

**Checkpoint**: All foundational types compile, all type-level tests pass (`cargo test -p types`)

---

## Phase 3: User Story 2 — Detect Which Platform Is Running (Priority: P1)

**Goal**: Automatically detect whether running inside Claude Code, OpenCode, or a custom platform using environment variables and hook input fields.

**Independent Test**: Set environment variables and pass hook input with various platform fields, verify detected platform name matches expected priority chain.

### Tests for User Story 2

- [x] T009 [US2] Write failing tests for `detect_platform()` in `crates/platforms/src/detection.rs` — test cases: (1) explicit platform field in hook input wins, (2) MEMVID_PLATFORM env var used when no explicit field, (3) OPENCODE=1 indicator detected, (4) default "claude" when nothing set, (5) case-normalization to lowercase, (6) whitespace trimming, (7) whitespace-only MEMVID_PLATFORM treated as absent, (8) empty platform field in input treated as absent. Use `temp_env::with_vars` for env var tests.

### Implementation for User Story 2

- [x] T010 [US2] Implement `detect_platform(input: &HookInput) -> String` in `crates/platforms/src/detection.rs` — priority chain per FR-006: explicit input field > MEMVID_PLATFORM env > OPENCODE=1 > default "claude". Normalize to lowercase and trim (FR-007). All T009 tests must pass.

**Checkpoint**: `cargo test -p platforms -- detection` — all platform detection tests green

---

## Phase 4: User Story 1 — Normalize Raw Hook Input into Typed Platform Events (Priority: P1) MVP

**Goal**: Convert raw hook JSON from Claude Code and OpenCode into typed PlatformEvent with consistent fields, unique event ID, and timestamp.

**Independent Test**: Pass representative raw hook JSON for each platform and verify normalized event output has all required fields.

### Tests for User Story 1

- [x] T011 [US1] Write failing tests for `PlatformAdapter` trait and `create_builtin_adapter()` factory in `crates/platforms/src/adapter.rs` — test cases: (1) factory creates adapter with correct platform name, (2) factory creates adapter with contract version "1.0.0", (3) platform_name() returns lowercase string

- [x] T012 [P] [US1] Write failing tests for Claude adapter normalization in `crates/platforms/src/adapters/claude.rs` — test cases: (1) SessionStart event from valid hook input, (2) ToolObservation event with tool_name, (3) SessionStop event, (4) returns None when session_id is empty string (note: truly absent session_id fails at JSON deserialization before reaching the adapter; the adapter checks for empty), (5) returns None when session_id is whitespace-only, (6) returns None for ToolObservation without tool_name, (7) event has auto-generated UUID, (8) event has auto-generated timestamp, (9) project_context.cwd populated from hook input, (10) platform field is "claude", (11) contract_version is "1.0.0"

- [x] T013 [P] [US1] Write failing tests for OpenCode adapter normalization in `crates/platforms/src/adapters/opencode.rs` — test cases: (1) SessionStart event from valid hook input, (2) platform field is "opencode", (3) same field structure as Claude-normalized event (matching acceptance scenario 2)

### Implementation for User Story 1

- [x] T014 [US1] Implement `PlatformAdapter` trait (platform_name, contract_version, normalize methods) and `create_builtin_adapter()` factory in `crates/platforms/src/adapter.rs` — trait is `Send + Sync`, factory returns `Box<dyn PlatformAdapter>`. Define `ADAPTER_CONTRACT_VERSION = "1.0.0"` constant. (`SUPPORTED_CONTRACT_MAJOR` is in `contract.rs` with `validate_contract()`.)

- [x] T015 [US1] Implement `BuiltinAdapter` struct and `PlatformAdapter` impl in `crates/platforms/src/adapters/claude.rs` — `normalize()` extracts session_id, cwd, hook_event_name, tool_name from HookInput; maps hook_event_name to EventKind; returns None for missing session_id or missing tool_name on ToolObservation; auto-generates event_id and timestamp; populates ProjectContext from input. Re-export from `crates/platforms/src/adapters/mod.rs`.

- [x] T016 [US1] Implement OpenCode adapter in `crates/platforms/src/adapters/opencode.rs` — uses same `create_builtin_adapter("opencode")` factory (shared normalization logic per research R4). Re-export from `crates/platforms/src/adapters/mod.rs`. All T013 tests must pass.

**Checkpoint**: `cargo test -p platforms -- adapters` — all normalization tests green for both Claude and OpenCode

---

## Phase 5: User Story 3 — Validate Adapter Contract Compatibility (Priority: P1)

**Goal**: Check event contract version against supported major version using semver. Incompatible events skipped with diagnostic rather than error.

**Independent Test**: Pass events with various version strings and verify compatible/incompatible results with correct reasons.

### Tests for User Story 3

- [x] T017 [US3] Write failing tests for `validate_contract()` in `crates/platforms/src/contract.rs` — test cases: (1) "1.2.3" is compatible with supported major 1, (2) "1.0.0" is compatible, (3) "2.0.0" is incompatible with reason "incompatible_contract_major", (4) "0.9.0" is incompatible, (5) "not-a-version" is incompatible with reason "invalid_contract_version", (6) empty string is incompatible, (7) "1.0.0-beta.1+build.42" is compatible (metadata stripped per clarification), (8) "1.0.0-rc.1" is compatible

### Implementation for User Story 3

- [x] T018 [US3] Implement `validate_contract(version_str: &str) -> ContractValidationResult` in `crates/platforms/src/contract.rs` — use `semver::Version::parse()`, extract major version, compare against `SUPPORTED_CONTRACT_MAJOR`. Return reasons: "incompatible_contract_major" or "invalid_contract_version". Never panic. All T017 tests must pass.

**Checkpoint**: `cargo test -p platforms -- contract` — all contract validation tests green

---

## Phase 6: User Story 4 — Resolve Project Identity for Memory Isolation (Priority: P1)

**Goal**: Resolve unique project identity key from project context, preventing memory cross-contamination.

**Independent Test**: Pass ProjectContext objects with various field combinations and verify resolved key and source.

### Tests for User Story 4

- [x] T019 [US4] Write failing tests for `resolve_project_identity()` in `crates/platforms/src/identity.rs` — test cases: (1) platform_project_id present → key=id, source=PlatformProjectId, (2) no project_id but canonical_path → key=path, source=CanonicalPath, (3) no project_id but cwd → key=cwd, source=CanonicalPath, (4) nothing present → key=None, source=Unresolved, (5) platform_project_id takes priority over canonical_path, (6) canonical_path takes priority over cwd, (7) empty string project_id treated as absent, (8) whitespace-only project_id treated as absent, (9) cwd pointing to non-existent directory still used as-is (no filesystem I/O — string value is used directly)

### Implementation for User Story 4

- [x] T020 [US4] Implement `resolve_project_identity(context: &ProjectContext) -> ProjectIdentity` in `crates/platforms/src/identity.rs` — priority chain per FR-010: platform_project_id > canonical_path > cwd > unresolved. Use string values directly (no filesystem I/O — no `std::fs::canonicalize()`; the caller provides paths, identity resolution just selects which one to use as the key). Report source per FR-011. All T019 tests must pass.

**Checkpoint**: `cargo test -p platforms -- identity` — all identity resolution tests green

---

## Phase 7: User Story 7 — Register and Resolve Platform Adapters (Priority: P2)

**Goal**: Maintain a registry of available adapters with registration, lookup, and listing support.

**Independent Test**: Register adapters, resolve by name, list platforms — verify sorted order and duplicate-overwrite behavior.

### Tests for User Story 7

- [x] T021 [US7] Write failing tests for `AdapterRegistry` in `crates/platforms/src/registry.rs` — test cases: (1) resolve returns registered adapter for "claude", (2) resolve returns None for unregistered "unknown", (3) platforms() returns sorted list ["claude", "opencode"], (4) duplicate registration overwrites (last-registered wins), (5) resolve is case-insensitive, (6) with_builtins() pre-registers claude and opencode, (7) new() creates empty registry

### Implementation for User Story 7

- [x] T022 [US7] Implement `AdapterRegistry` struct (HashMap-backed) with `new()`, `register()`, `resolve()`, `platforms()`, and `with_builtins()` in `crates/platforms/src/registry.rs` — register normalizes platform name to lowercase; resolve normalizes lookup key; platforms() returns sorted Vec. All T021 tests must pass.

**Checkpoint**: `cargo test -p platforms -- registry` — all registry tests green

---

## Phase 8: User Story 5 — Process Events Through the Pipeline (Priority: P2)

**Goal**: Central coordination point composing contract validation + identity resolution into single entry point with process/skip decision.

**Independent Test**: Pass complete platform events, verify pipeline returns correct process/skip with identity key.

**Dependencies**: Requires US3 (contract validation) and US4 (identity resolution) to be complete.

### Tests for User Story 5

- [x] T023 [US5] Write failing tests for `EventPipeline` in `crates/platforms/src/pipeline.rs` — test cases: (1) valid event with compatible contract and resolvable identity → not skipped, identity present, (2) incompatible contract version → skipped with reason "incompatible_contract_major", diagnostic present, (3) compatible contract but unresolvable identity → skipped with reason "missing_project_identity", diagnostic present with missing field names, (4) malformed contract version → skipped with reason "invalid_contract_version", (5) pipeline never panics on any input

### Implementation for User Story 5

- [x] T024 [US5] Implement `EventPipeline` struct with `new()` and `process(&self, event: &PlatformEvent) -> PipelineResult` in `crates/platforms/src/pipeline.rs` — define `PipelineResult` struct (skipped, reason, identity, diagnostic). Compose `validate_contract()` then `resolve_project_identity()`. Create DiagnosticRecord for skip cases. All T023 tests must pass.

**Checkpoint**: `cargo test -p platforms -- pipeline` — all pipeline tests green

---

## Phase 9: User Story 6 — Resolve Memory File Path with Policy Rules (Priority: P2)

**Goal**: Determine memory file storage path based on legacy/platform-specific policy with path traversal prevention.

**Independent Test**: Pass various path policy inputs and verify resolved path, mode, and path traversal rejection.

### Tests for User Story 6

- [x] T025 [US6] Write failing tests for `resolve_memory_path()` in `crates/platforms/src/path_policy.rs` — test cases: (1) no opt-in → legacy path `.agent-brain/mind.mv2`, mode LegacyFirst, (2) platform opt-in → platform-namespaced path e.g. `.claude/mind-claude.mv2`, mode PlatformOptIn, (3) path traversal attempt `../../etc/secrets` → error E_PLATFORM_PATH_TRAVERSAL, (4) platform name with special chars sanitized (e.g., "my.platform" → "my-platform"), (5) path stays within project directory, (6) define `ResolvedMemoryPath` and `PathMode` structs

### Implementation for User Story 6

- [x] T026 [US6] Implement `resolve_memory_path(project_dir: &Path, platform_name: &str, platform_opt_in: bool) -> Result<ResolvedMemoryPath, AgentBrainError>` in `crates/platforms/src/path_policy.rs` — define `ResolvedMemoryPath` {path, mode} and `PathMode` enum {LegacyFirst, PlatformOptIn}. Sanitize platform name per FR-016. Validate resolved path stays within project_dir per FR-014. All T025 tests must pass.

**Checkpoint**: `cargo test -p platforms -- path_policy` — all path policy tests green

---

## Phase 10: User Story 8 — Record Diagnostic Information for Debugging (Priority: P3)

**Goal**: Verify diagnostic record creation is correct across all integration points. (DiagnosticRecord type and constructor already implemented in Phase 2.)

**Independent Test**: Create diagnostic records with various inputs, verify all fields correctly populated.

- [x] T027 [US8] Write integration tests in `crates/platforms/src/pipeline.rs` verifying diagnostic records produced by pipeline skip cases — test cases: (1) diagnostic has correct platform name from event, (2) diagnostic has correct error_type matching skip reason, (3) diagnostic severity is "warning" for contract/identity skips, (4) diagnostic redacted is true, (5) diagnostic expires_at is 30 days from timestamp, (6) diagnostic affected_fields lists relevant field names

**Checkpoint**: `cargo test -p platforms` — all platform crate tests green including diagnostic integration

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: Final quality pass, public API cleanup, workspace-level validation

- [x] T028 Update `crates/platforms/src/lib.rs` with complete public re-exports — export all public types, traits, functions: `PlatformAdapter`, `AdapterRegistry`, `EventPipeline`, `PipelineResult`, `ResolvedMemoryPath`, `PathMode`, `detect_platform`, `validate_contract`, `resolve_project_identity`, `resolve_memory_path`, `create_builtin_adapter`, constants
- [x] T029 Run full quality gate — `cargo build` (compiles), `cargo test` (all green), `cargo clippy -- -D warnings` (zero warnings), `cargo fmt --check` (formatted). Fix any issues. Note: agent integration smoke test is N/A for this feature (library crate with no CLI commands; see plan.md quality gate note).
- [x] T030 Verify quickstart.md code examples compile against actual public API — check that the usage flow and custom adapter examples in `specs/005-platform-adapter-system/quickstart.md` match the implemented API signatures

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational Types (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US2 Detection (Phase 3)**: Depends on Phase 2 — independent of other stories
- **US1 Normalization (Phase 4)**: Depends on Phase 2 — independent of US2
- **US3 Contract Validation (Phase 5)**: Depends on Phase 2 — independent of US1/US2
- **US4 Identity Resolution (Phase 6)**: Depends on Phase 2 — independent of US1/US2/US3
- **US7 Registry (Phase 7)**: Depends on US1 (needs PlatformAdapter trait)
- **US5 Pipeline (Phase 8)**: Depends on US3 + US4 (composes both)
- **US6 Path Policy (Phase 9)**: Depends on Phase 2 — independent of other stories
- **US8 Diagnostics Integration (Phase 10)**: Depends on US5 (tests pipeline diagnostics)
- **Polish (Phase 11)**: Depends on all user stories complete

### User Story Dependencies

```text
Phase 1 (Setup)
    │
Phase 2 (Types) ─── BLOCKS ALL ───┐
    │                              │
    ├── US2 (Detection)   ─────────┤ (independent)
    ├── US1 (Normalization) ───────┤ (independent)
    ├── US3 (Contract Validation) ─┤ (independent)
    ├── US4 (Identity Resolution) ─┤ (independent)
    ├── US6 (Path Policy) ─────────┤ (independent)
    │                              │
    ├── US7 (Registry) ←── US1     │ (needs adapter trait)
    ├── US5 (Pipeline) ←── US3+US4 │ (composes both)
    └── US8 (Diagnostics) ←── US5  │ (integration tests)
                                   │
                          Phase 11 (Polish)
```

### Within Each User Story

- Tests MUST be written and FAIL before implementation (constitution Principle V)
- Types/structs before behavior
- Core implementation before integration
- Story complete before moving to dependent stories

### Parallel Opportunities

**After Phase 2 completes, these can run in parallel:**
- US1 (Normalization) + US2 (Detection) + US3 (Contract Validation) + US4 (Identity Resolution) + US6 (Path Policy)

**After US1 completes:**
- US7 (Registry) can start

**After US3 + US4 complete:**
- US5 (Pipeline) can start

---

## Parallel Example: After Phase 2

```text
Agent A: US1 — Normalization (T011-T016) in crates/platforms/src/adapter.rs + adapters/
Agent B: US2 — Detection (T009-T010) in crates/platforms/src/detection.rs
Agent C: US3 — Contract (T017-T018) in crates/platforms/src/contract.rs
Agent D: US4 — Identity (T019-T020) in crates/platforms/src/identity.rs
Agent E: US6 — Path Policy (T025-T026) in crates/platforms/src/path_policy.rs
```

All five are in different files with no cross-dependencies.

---

## Parallel Example: Phase 2 Foundational Types

```text
Agent A: T003 — Error codes in crates/types/src/error.rs
Agent B: T004 — PlatformEvent in crates/types/src/platform_event.rs
Agent C: T005 — ProjectContext in crates/types/src/project_context.rs
Agent D: T006 — ContractValidationResult in crates/types/src/contract_version.rs
Agent E: T007 — DiagnosticRecord in crates/types/src/diagnostic.rs
```

All five are in different files with no cross-dependencies (T008 runs after all complete).

---

## Implementation Strategy

### MVP First (User Story 1 + 2 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational Types (CRITICAL — blocks all stories)
3. Complete Phase 3: US2 — Platform Detection
4. Complete Phase 4: US1 — Event Normalization
5. **STOP and VALIDATE**: Both stories independently testable
6. Can demonstrate: raw hook input → detect platform → select adapter → normalize event

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add US2 (Detection) + US1 (Normalization) → **MVP: Hook input normalizes to typed events**
3. Add US3 (Contract Validation) → Events validated against version
4. Add US4 (Identity Resolution) → Projects have unique keys
5. Add US5 (Pipeline) → Full event processing pipeline
6. Add US7 (Registry) → Extensible adapter lookup
7. Add US6 (Path Policy) → Memory file paths resolved
8. Add US8 (Diagnostics Integration) → Observability verified
9. Polish → Production-ready

### Task Summary

| Phase | Story | Tasks | Parallel |
|-------|-------|-------|----------|
| 1 Setup | — | T001-T002 (2) | No |
| 2 Foundational | — | T003-T008 (6) | T003-T007 parallel |
| 3 US2 Detection | P1 | T009-T010 (2) | No |
| 4 US1 Normalization | P1 | T011-T016 (6) | T012+T013 parallel |
| 5 US3 Contract | P1 | T017-T018 (2) | No |
| 6 US4 Identity | P1 | T019-T020 (2) | No |
| 7 US7 Registry | P2 | T021-T022 (2) | No |
| 8 US5 Pipeline | P2 | T023-T024 (2) | No |
| 9 US6 Path Policy | P2 | T025-T026 (2) | No |
| 10 US8 Diagnostics | P3 | T027 (1) | No |
| 11 Polish | — | T028-T030 (3) | No |
| **Total** | | **30 tasks** | |

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable after Phase 2
- Tests MUST fail before implementation (constitution Principle V — non-negotiable)
- Commit after each phase checkpoint
- All paths are relative to repository root
