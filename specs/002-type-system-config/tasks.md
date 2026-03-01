# Tasks: Type System & Configuration

**Input**: Design documents from `specs/002-type-system-config/`
**Prerequisites**: plan.md (required), spec.md (required), prd.md, data-model.md, contracts/types-api.rs, research.md, quickstart.md

**Tests**: Included per constitution V (Test-First Development is non-negotiable). Each module follows RED-GREEN-REFACTOR: write tests first, verify they fail, then implement.

**Organization**: Tasks grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1-US5)
- Exact file paths included in all descriptions

## Path Conventions

- **Workspace member**: `crates/types/src/` (source), `crates/types/tests/` (integration tests)
- **Workspace root**: `Cargo.toml` (workspace deps already pinned from 001-project-bootstrap)

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Update dependencies and create module file stubs for the types crate

- [x] T001 Update crates/types/Cargo.toml to add workspace dependencies: serde_json, thiserror, chrono, uuid; add serde_json to dev-dependencies
- [x] T002 Create empty module files (error.rs, observation.rs, session.rs, context.rs, config.rs, stats.rs, hooks.rs) in crates/types/src/ and add module declarations to crates/types/src/lib.rs

---

## Phase 2: Foundational — Error Types (Blocking Prerequisites)

**Purpose**: AgentBrainError and error_codes are used by observation, config, and other modules for validation errors. Must complete before any user story work.

**CRITICAL**: No user story work can begin until this phase is complete.

### Tests

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T003 Write unit tests for AgentBrainError: construct each of 6 variants (FileSystem, Configuration, Serialization, Lock, MemoryCorruption, InvalidInput), verify code() returns expected &'static str, verify Display format "[code] message" in crates/types/src/error.rs

### Implementation

- [x] T004 Implement AgentBrainError enum (#[non_exhaustive], thiserror derive), error_codes module (15 string constants: E_FS_*, E_CONFIG_*, E_SER_*, E_LOCK_*, E_MEM_*, E_INPUT_*), and code() method in crates/types/src/error.rs

**Checkpoint**: `cargo test -p types` passes for error module. All 6 variants constructable with stable codes.

---

## Phase 3: User Story 1 — Downstream Crate Consumes Shared Types (Priority: P1) MVP

**Goal**: All 10 entity types are defined, well-typed, and enforce valid states. A downstream crate can import every type from `types::` and construct valid instances.

**Independent Test**: Import each public type from the types crate, construct valid instances, and verify compile + runtime correctness. Attempt invalid construction and confirm rejection.

**Requirements covered**: M-1, M-2, M-3, M-4, M-5, M-6, M-7, M-11, S-5

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T005 [P] [US1] Write unit tests for ObservationType (all 10 variants constructable, Copy+Clone+Eq+Hash), Observation (construction with all fields, reject empty/whitespace-only summary, reject empty/whitespace-only content with AgentBrainError::InvalidInput), and ObservationMetadata (construction with defaults, flattened extra map accepts arbitrary keys) in crates/types/src/observation.rs
- [x] T006 [P] [US1] Write unit tests for SessionSummary (construction with all 7 fields, reject end_time < start_time, reject empty summary, reject empty id) in crates/types/src/session.rs
- [x] T007 [P] [US1] Write unit tests for InjectedContext (construction, all Vec fields default to empty, token_count defaults to 0) in crates/types/src/context.rs
- [x] T008 [P] [US1] Write unit tests for MindConfig Default (memory_path=".agent-brain/mind.mv2", max_context_observations=20, max_context_tokens=2000, auto_compress=true, min_confidence=0.6, debug=false) and validate() boundary cases (reject min_confidence < 0.0 or > 1.0, reject max_context_observations = 0, reject max_context_tokens = 0) in crates/types/src/config.rs
- [x] T009 [P] [US1] Write unit tests for MindStats (construction, oldest_memory/newest_memory are None for empty store, type_counts defaults to empty HashMap) in crates/types/src/stats.rs

### Implementation for User Story 1

- [x] T010 [P] [US1] Implement ObservationType enum (#[non_exhaustive], derive Debug/Clone/Copy/PartialEq/Eq/Hash/Serialize/Deserialize, serde rename_all="lowercase"), Observation struct (serde rename_all="camelCase", obs_type renamed to "type", validation rejects empty summary/content), and ObservationMetadata (serde rename_all="camelCase", flatten extra HashMap) in crates/types/src/observation.rs
- [x] T011 [P] [US1] Implement SessionSummary struct (serde rename_all="camelCase", modified_files renamed to "filesModified", validation rejects end_time < start_time and empty summary/id) in crates/types/src/session.rs
- [x] T012 [P] [US1] Implement InjectedContext struct (serde rename_all="camelCase", all fields with #[serde(default)], token_count field matching TS naming) in crates/types/src/context.rs
- [x] T013 [P] [US1] Implement MindConfig struct (serde rename_all="camelCase", #[serde(default)] on struct, Default trait with 6 documented values) in crates/types/src/config.rs
- [x] T014 [P] [US1] Implement MindStats struct (serde rename_all="camelCase", file_size_bytes renamed to "fileSize", type_counts renamed to "topTypes", optional timestamps with skip_serializing_if) in crates/types/src/stats.rs
- [x] T015 [US1] Update crates/types/src/lib.rs with public re-exports: pub use error, observation, session, context, config, stats modules and all public types

**Checkpoint**: `cargo test -p types` passes for all modules. `cargo build -p types` succeeds. All 8 entity types (excluding HookInput/HookOutput) constructable from downstream code.

---

## Phase 4: User Story 2 — Data Round-Trips Through Serialization (Priority: P1)

**Goal**: Every public type serializes to JSON and deserializes back without data loss. Edge cases (Unicode, nested data, missing optionals, partial config) are verified.

**Independent Test**: Construct each type with representative data, serialize to JSON string, deserialize back, assert equality with original.

**Requirements covered**: M-10, M-11, S-2

### Tests for User Story 2

> **NOTE: Write these tests FIRST. They should PASS since serde derives are in place from US1. If any fail, fix serde attributes.**

- [x] T016 [P] [US2] Write JSON round-trip tests for Observation (all fields populated, Unicode emoji/CJK/RTL in summary and content, special characters) and ObservationMetadata (deeply nested extra map 5+ levels, empty extra map) in crates/types/src/observation.rs
- [x] T017 [P] [US2] Write JSON round-trip tests for SessionSummary (all fields preserved, empty Vec<String> for key_decisions and modified_files, verify "filesModified" JSON key name) in crates/types/src/session.rs
- [x] T018 [P] [US2] Write JSON round-trip tests for InjectedContext (nested Observation and SessionSummary instances, empty context with all defaults) in crates/types/src/context.rs
- [x] T019 [P] [US2] Write JSON round-trip tests for MindConfig (default values appear in JSON output, partial JSON input with missing fields applies defaults via serde(default), empty JSON object {} produces all defaults) in crates/types/src/config.rs
- [x] T020 [P] [US2] Write JSON round-trip tests for MindStats (optional timestamps Some/None, HashMap<ObservationType, u64> type_counts with multiple entries, verify "fileSize" and "topTypes" JSON key names) in crates/types/src/stats.rs
- [x] T021 [US2] Write cross-module integration test: construct Observation with metadata, wrap in InjectedContext with SessionSummary, serialize entire structure to JSON, deserialize back, verify full equality in crates/types/tests/round_trip.rs

### Implementation for User Story 2

No new type implementation required. Round-trip correctness is verified by tests above. If any test fails, fix the corresponding serde attributes in the source module from US1.

**Checkpoint**: `cargo test -p types` passes all round-trip tests. JSON output matches expected key names (camelCase for app types). Partial deserialization with defaults works for MindConfig.

---

## Phase 5: User Story 3 — Error Handling Provides Actionable Diagnostics (Priority: P2)

**Goal**: Every error variant provides a stable code, human-readable message, and full cause chain. Errors are structured enough for AI agents to diagnose and recover programmatically.

**Independent Test**: Trigger each error variant, verify stable code, descriptive message, and cause chain traversal via Error::source().

**Requirements covered**: M-9, S-4

### Tests for User Story 3

- [x] T022 [P] [US3] Write tests verifying all 15 error code constants in error_codes module match their string values exactly (E_FS_NOT_FOUND, E_FS_PERMISSION_DENIED, E_FS_IO_ERROR, E_CONFIG_INVALID_VALUE, E_CONFIG_MISSING_FIELD, E_CONFIG_PARSE_ERROR, E_SER_SERIALIZE_FAILED, E_SER_DESERIALIZE_FAILED, E_LOCK_ACQUISITION_FAILED, E_LOCK_TIMEOUT, E_MEM_CORRUPTED_INDEX, E_MEM_INVALID_CHECKSUM, E_INPUT_EMPTY_FIELD, E_INPUT_OUT_OF_RANGE, E_INPUT_INVALID_FORMAT) in crates/types/src/error.rs
- [x] T023 [P] [US3] Write tests verifying Error::source() chaining: FileSystem variant wraps std::io::Error, Serialization variant wraps serde_json::Error, source() returns Some with original error in crates/types/src/error.rs
- [x] T024 [US3] Write test verifying 3-level deep cause chain traversal (io::Error → FileSystem → assert source().source() accessible) and Display output format "[code] message" for all 6 variants in crates/types/src/error.rs

### Implementation for User Story 3

Error types implemented in Phase 2. This phase verifies diagnostic quality through comprehensive tests. Fix error implementations if any test reveals insufficient cause chain support or incorrect Display format.

**Checkpoint**: All error code stability tests pass. Error::source() returns correct wrapped errors for FileSystem and Serialization variants. Display format is consistent "[code] message".

---

## Phase 6: User Story 4 — Configuration Resolves from Environment (Priority: P2)

**Goal**: `MindConfig::from_env()` reads 6 environment variables with precedence over defaults. Invalid env values produce actionable Configuration errors.

**Independent Test**: Set specific environment variables, call from_env(), verify env values override defaults. Unset them and verify defaults restored. Provide invalid values and verify Configuration error with field identification.

**Requirements covered**: S-1

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T025 [P] [US4] Write unit tests for MindConfig::from_env() with each env var override: MEMVID_PLATFORM_MEMORY_PATH overrides memory_path, MEMVID_MIND_DEBUG="1" enables debug, MEMVID_PLATFORM sets platform detection, no env vars returns all defaults in crates/types/src/config.rs
- [x] T026 [P] [US4] Write unit tests for invalid env var rejection: MEMVID_MIND_DEBUG="banana" returns AgentBrainError::Configuration identifying the field and invalid value, verify error code is E_CONFIG_INVALID_VALUE in crates/types/src/config.rs

### Implementation for User Story 4

- [x] T027 [US4] Implement MindConfig::from_env() reading 6 env vars (MEMVID_PLATFORM, MEMVID_MIND_DEBUG, MEMVID_PLATFORM_MEMORY_PATH, MEMVID_PLATFORM_PATH_OPT_IN, CLAUDE_PROJECT_DIR, OPENCODE_PROJECT_DIR) with validation and precedence: env > default in crates/types/src/config.rs

**Checkpoint**: `cargo test -p types` passes all env var tests. Each of 6 env vars overrides its corresponding default. Invalid env values rejected with Configuration error including field name and invalid value.

---

## Phase 7: User Story 5 — Hook Protocol Types Enable Agent Communication (Priority: P3)

**Goal**: HookInput and HookOutput types match the Claude Code hook JSON protocol. Unknown fields in HookInput are silently ignored for forward compatibility.

**Independent Test**: Parse real Claude Code hook JSON samples into HookInput. Serialize HookOutput and verify JSON key names match protocol. Add 10+ unknown fields and verify deserialization succeeds.

**Requirements covered**: M-8, S-3

### Tests for User Story 5

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T028 [P] [US5] Write unit tests for HookInput deserialization from real Claude Code JSON samples: SessionStart event (with source, model), PostToolUse event (with tool_name, tool_input, tool_response, tool_use_id), Stop event (with stop_hook_active, last_assistant_message) in crates/types/src/hooks.rs
- [x] T029 [P] [US5] Write unit tests for HookInput forward compatibility: deserialize JSON with 10+ unknown fields alongside known fields, verify known fields parse correctly and no error in crates/types/src/hooks.rs
- [x] T030 [P] [US5] Write unit tests for HookOutput serialization: continue_execution maps to "continue" JSON key, stopReason/suppressOutput/systemMessage/hookSpecificOutput use correct mixed-case keys, round-trip fidelity in crates/types/src/hooks.rs

### Implementation for User Story 5

- [x] T031 [US5] Implement HookInput struct (snake_case JSON keys, no deny_unknown_fields, 5 required + 10 optional fields per data-model.md) in crates/types/src/hooks.rs
- [x] T032 [US5] Implement HookOutput struct (mixed-case explicit serde renames: continue_execution→"continue", stop_reason→"stopReason", suppress_output→"suppressOutput", system_message→"systemMessage", hook_specific_output→"hookSpecificOutput") in crates/types/src/hooks.rs
- [x] T033 [US5] Update crates/types/src/lib.rs to add hooks module declaration and public re-exports for HookInput and HookOutput

**Checkpoint**: `cargo test -p types` passes all hook tests. HookInput parses real Claude Code JSON. 10+ unknown fields ignored without error. HookOutput serializes with correct protocol key names.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, quality gates, and final validation across all modules

- [x] T034 [P] Add crate-level doc comment and module-level doc comments for all public types, fields, and methods in crates/types/src/ (S-5: sufficient for developer understanding without reading other crates)
- [x] T035 Run cargo clippy -p types -- -D warnings and fix all warnings
- [x] T036 Run cargo fmt -p types --check and fix any formatting issues
- [x] T037 Validate quickstart.md code examples compile and run correctly as integration tests in crates/types/tests/quickstart_validation.rs
- [x] T038 Run cargo doc -p types --no-deps and fix any doc warnings

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories (AgentBrainError needed for validation)
- **US1 (Phase 3)**: Depends on Phase 2 — defines all type structures
- **US2 (Phase 4)**: Depends on Phase 3 — tests require types to be implemented
- **US3 (Phase 5)**: Depends on Phase 2 — tests comprehensive error behavior
- **US4 (Phase 6)**: Depends on Phase 3 — from_env() needs MindConfig struct
- **US5 (Phase 7)**: Depends on Phase 3 — HookOutput uses InjectedContext from context module
- **Polish (Phase 8)**: Depends on all desired user stories being complete

### User Story Dependencies

```
Phase 1 (Setup)
    └── Phase 2 (Foundational: error.rs)
         ├── Phase 3 (US1: all types) ← MVP
         │    ├── Phase 4 (US2: round-trip tests)
         │    ├── Phase 6 (US4: env resolution)
         │    └── Phase 7 (US5: hook types)
         └── Phase 5 (US3: error diagnostics)
              └── Phase 8 (Polish)
```

- **US1 (P1)**: Can start after Phase 2. No dependencies on other stories.
- **US2 (P1)**: Can start after US1. Tests verify serde attributes from US1.
- **US3 (P2)**: Can start after Phase 2. Independent of US1/US2. **Parallel with US1.**
- **US4 (P2)**: Can start after US1. Needs MindConfig struct.
- **US5 (P3)**: Can start after US1. Needs InjectedContext for HookOutput.

### Within Each User Story

1. Tests MUST be written and verified to FAIL before implementation (constitution V)
2. Within a module: tests → implementation (sequential)
3. Across modules: different files can be worked in parallel
4. Implementation → verify tests pass → commit

### Parallel Opportunities

**Phase 3 (US1) — Maximum parallelism across modules:**
- T005, T006, T007, T008, T009 (all test tasks in different files — fully parallel)
- T010, T011, T012, T013, T014 (implementation across different files — parallel, except context.rs needs observation+session types and stats.rs needs ObservationType)
- **True parallel groups**: {observation, session, config} then {context, stats} then {lib.rs}

**Phase 4 (US2) — All round-trip tests parallel:**
- T016, T017, T018, T019, T020 (different files — fully parallel)

**Phase 5+6 — Can run in parallel with each other:**
- US3 (error diagnostics) and US4 (env resolution) are independent

**Phase 7 (US5) — Test tasks parallel:**
- T028, T029, T030 (same file but logically independent test groups)

---

## Parallel Example: User Story 1 Implementation

```
# Wave 1: Write all tests in parallel (different files)
Agent A: T005 - observation tests (crates/types/src/observation.rs)
Agent B: T006 - session tests (crates/types/src/session.rs)
Agent C: T007 - context tests (crates/types/src/context.rs)
Agent D: T008 - config tests (crates/types/src/config.rs)
Agent E: T009 - stats tests (crates/types/src/stats.rs)

# Wave 2: Implement types in parallel (independent modules first)
Agent A: T010 - observation impl (crates/types/src/observation.rs)
Agent B: T011 - session impl (crates/types/src/session.rs)
Agent D: T013 - config impl (crates/types/src/config.rs)

# Wave 3: Implement dependent modules
Agent C: T012 - context impl (needs observation + session types)
Agent E: T014 - stats impl (needs ObservationType)

# Wave 4: Wire up re-exports
Agent A: T015 - lib.rs re-exports
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (Cargo.toml + module stubs)
2. Complete Phase 2: Foundational (AgentBrainError + error_codes)
3. Complete Phase 3: User Story 1 (all type definitions with validation)
4. **STOP and VALIDATE**: `cargo test -p types`, `cargo clippy -p types -- -D warnings`
5. All 8 entity types (excluding hooks) available from `types::` crate root

### Incremental Delivery

1. Setup + Foundational → error types ready
2. Add US1 → all types available → **MVP!**
3. Add US2 → round-trip fidelity verified → **data integrity confirmed**
4. Add US3 → error diagnostics verified → **agent-friendly errors**
5. Add US4 → env resolution → **deployable across environments**
6. Add US5 → hook protocol → **ready for Phase 2 hooks crate**
7. Polish → docs + quality gates → **merge-ready**

### Quality Gates (per constitution, all must pass before merge)

1. `cargo test -p types` — all green
2. `cargo clippy -p types -- -D warnings` — no warnings
3. `cargo fmt -p types --check` — formatted
4. `cargo doc -p types --no-deps` — no doc warnings
5. JSON round-trip tests pass for all 10 public types
6. Error code stability verified (15 string constants)
7. Default values match TypeScript implementation (6/6 fields)
8. Environment variable resolution works for all 6 variables
9. HookInput tolerates 10+ unknown fields

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks in same phase
- [Story] label maps tasks to user stories for traceability
- Each user story is independently completable and testable at its checkpoint
- Test-first is mandatory per constitution V — write tests, verify RED, implement GREEN
- Commit after each phase checkpoint
- All serde attributes must match contracts/types-api.rs exactly
- Refer to data-model.md for field types, JSON key names, and validation rules
- Refer to research.md for design decisions (R-1 through R-5)
