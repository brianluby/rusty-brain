# Tasks: OpenCode Plugin Adapter

**Input**: Design documents from `/specs/008-opencode-plugin/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), prd.md (optional), ar.md (optional), sec.md (optional), research.md, data-model.md, contracts/

**Tests**: Included — Constitution V mandates test-first development (non-negotiable).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1–US6)
- Exact file paths included in all descriptions

## Path Conventions

- **Library crate**: `crates/opencode/src/` (handler logic, no I/O)
- **CLI binary**: `crates/cli/src/` (stdin/stdout I/O, subcommand dispatch)
- **Tests**: `crates/opencode/tests/` (unit + integration)
- **Manifest**: `plugin-manifest.json` (repository root)

---

## Phase 1: Setup

**Purpose**: Configure workspace dependencies and module structure

- [x] T001 Configure `crates/opencode/Cargo.toml` with workspace dependencies (core, types, platforms, compression, serde, serde_json, tracing, chrono, tempfile; dev: tempfile also used for test scaffolding) and set up module declarations in `crates/opencode/src/lib.rs`. Note: `tempfile` is a regular dependency because the sidecar `save()` function uses `NamedTempFile` for atomic writes (research.md R2).
- [x] T002 [P] Add `opencode = { path = "../opencode" }` dependency to `crates/cli/Cargo.toml`

---

## Phase 2: Foundational (Types + Sidecar + Fail-Open)

**Purpose**: Shared types, sidecar state management, and fail-open infrastructure used by ALL user stories

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T003 Implement `MindToolInput`, `MindToolOutput`, `SidecarState`, `VALID_MODES`, and `MAX_DEDUP_ENTRIES` per contracts/types.rs in `crates/opencode/src/types.rs`
- [x] T004 Write sidecar unit tests covering: load/save roundtrip, LRU eviction at 1024 boundary, hash computation determinism, `sidecar_path` sanitization, atomic write (temp+rename), corrupt file recovery (delete and recreate with WARN), `is_duplicate` true/false, `add_hash` LRU refresh, and 0600 file permissions (SEC-2) in `crates/opencode/tests/sidecar_test.rs`
- [x] T005 Write fail-open unit tests covering: `handle_with_failopen` error→valid default `HookOutput` JSON, panic→valid default `HookOutput` JSON, `mind_tool_with_failopen` error→`MindToolOutput { success: false }`, panic→`MindToolOutput { success: false }`, and WARN trace emitted for all caught errors/panics (SEC-10) in `crates/opencode/tests/failopen_test.rs`
- [x] T006 Implement sidecar module per contracts/sidecar_api.rs: `load`, `save` (atomic temp+rename with 0600 permissions), `sidecar_path` (session ID sanitization), `is_duplicate`, `add_hash` (LRU eviction at 1024), `compute_dedup_hash` (DefaultHasher on tool_name+summary → 16-char hex) in `crates/opencode/src/sidecar.rs`
- [x] T007 Implement fail-open wrappers (`handle_with_failopen` using `catch_unwind(AssertUnwindSafe(..))`, `mind_tool_with_failopen`) and public module re-exports per contracts/handler_api.rs in `crates/opencode/src/lib.rs`

**Note**: T004 and T005 depend on T003 (types must compile before tests can reference them). Write T003 first, then T004/T005 in parallel.

**Checkpoint**: Types compile, sidecar tests pass, fail-open tests pass. All user story handlers can now be implemented.

---

## Phase 3: User Story 1 — Context Injection in Chat (Priority: P1) :dart: MVP

**Goal**: Chat hook intercepts OpenCode conversations, retrieves relevant context from memory (recent observations, session summaries, topic-relevant memories via `Mind::get_context`), and injects it as `system_message` + structured `hook_specific_output`.

**Independent Test**: Trigger chat hook with test message → verify response includes injected context from a memory file with known observations.

**Requirements**: M-1, M-5, M-6, M-7, S-3 | **ACs**: AC-1, AC-2, AC-3, AC-4, AC-18

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T008 [US1] Write chat hook unit tests covering: context injection with known memory file (AC-1), empty/new memory file with welcome message (AC-2), error path returns `HookOutput::default()` with WARN trace (AC-3, M-5), topic-relevant query passes to `Mind::get_context(Some(query))` (AC-4, S-3), memory path resolved via `resolve_memory_path(cwd, "opencode", false)` (AC-18, M-6), and `system_message` format includes recent observations + session summaries per research.md R6 in `crates/opencode/tests/chat_hook_test.rs`

### Implementation for User Story 1

- [x] T009 [US1] Implement `handle_chat_hook` per contracts/handler_api.rs: resolve memory path (LegacyFirst), open Mind, call `get_context(query)`, format `system_message` (human-readable) and `hook_specific_output` (structured `InjectedContext` JSON), return `HookOutput` in `crates/opencode/src/chat_hook.rs`

**Checkpoint**: Chat hook returns injected context for known memory; fails-open on errors. US1 independently testable.

---

## Phase 4: User Story 2 — Tool Observation Capture (Priority: P1)

**Goal**: Tool hook captures compressed observations after tool executions, with session-scoped deduplication via sidecar file.

**Independent Test**: Trigger tool hook with simulated tool execution → verify observation stored in memory; trigger again with same input → verify dedup skips storage.

**Requirements**: M-2, M-4, M-5, M-6, M-7 | **ACs**: AC-5, AC-6, AC-7, AC-8, AC-18

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T010 [US2] Write tool hook unit tests covering: new observation stored with correct obs_type, tool_name, compressed summary (AC-5), duplicate detected via sidecar hash and skipped (AC-6, M-4), large output compressed to ~500 tokens (AC-8), sidecar updated with new hash and incremented observation_count, error path returns `HookOutput { continue_execution: Some(true) }` with WARN trace (AC-7, M-5), and sidecar created on first invocation in `crates/opencode/tests/tool_hook_test.rs`

### Implementation for User Story 2

- [x] T011 [US2] Implement `handle_tool_hook` per contracts/handler_api.rs: load or create sidecar state, compress tool output via `compression::compress()`, compute dedup hash, check `is_duplicate`, if new call `Mind::remember()` and `add_hash`, save sidecar (atomic), return `HookOutput` in `crates/opencode/src/tool_hook.rs`

**Checkpoint**: Tool hook captures observations, deduplicates correctly, and fails-open. US2 independently testable.

---

## Phase 5: User Story 3 — Native Mind Tool (Priority: P1)

**Goal**: Native `mind` tool with 5 modes (search, ask, recent, stats, remember) dispatching to Mind API.

**Independent Test**: Invoke mind tool with each mode → verify correct structured results returned.

**Requirements**: M-3, M-5, M-6 | **ACs**: AC-9, AC-10, AC-11, AC-12, AC-13 | **SEC**: SEC-8

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T012 [US3] Write mind tool unit tests covering: search mode returns matching observations with type/timestamp/summary/excerpt (AC-9), ask mode returns synthesized answer (AC-10), recent mode returns reverse-chronological timeline (AC-11), stats mode returns total observations/sessions/date range/size/type breakdown (AC-12), remember mode stores observation as Discovery type and returns ULID (AC-13), invalid mode returns `MindToolOutput::error` listing valid modes (SEC-8, EC-3), missing required fields (query for search/ask, content for remember) return error, and empty results handled gracefully in `crates/opencode/tests/mind_tool_test.rs`

### Implementation for User Story 3

- [x] T013 [US3] Implement `handle_mind_tool` per contracts/handler_api.rs: validate mode against `VALID_MODES` whitelist (SEC-8), dispatch to `Mind::search`/`ask`/`timeline`/`stats`/`remember`, convert results to `MindToolOutput` with mode-specific `data` schemas per data-model.md, default limit to 10 for search/recent in `crates/opencode/src/mind_tool.rs`

**Checkpoint**: All 5 mind tool modes return correct results; invalid mode returns structured error. US3 independently testable.

---

## Phase 6: User Story 4 — Session Cleanup (Priority: P2)

**Goal**: On session deletion, generate and store session summary (observation count, key decisions), delete sidecar file, release memory.

**Independent Test**: Trigger session deletion → verify summary stored in memory and sidecar file deleted.

**Requirements**: S-1, M-5 | **ACs**: AC-14, AC-15

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T014 [US4] Write session cleanup unit tests covering: summary generated and stored with observation count from sidecar (AC-14), sidecar file deleted after summary storage, empty session (no observations) stores minimal summary (AC-15), error path returns `HookOutput::default()` with WARN trace (M-5), and missing sidecar file handled gracefully in `crates/opencode/tests/session_cleanup_test.rs`

### Implementation for User Story 4

- [x] T015 [US4] Implement `handle_session_cleanup` per contracts/handler_api.rs: load sidecar state for observation metadata, generate summary text with observation count, call `Mind::save_session_summary`, delete sidecar file, return `HookOutput` in `crates/opencode/src/session_cleanup.rs`

**Checkpoint**: Session cleanup generates summary and removes sidecar. US4 independently testable.

---

## Phase 7: User Story 5 — Plugin Registration and Discovery (Priority: P2)

**Goal**: OpenCode discovers rusty-brain plugin through a manifest file declaring capabilities and binary path.

**Independent Test**: Validate manifest file structure contains required fields (name, version, binary_path, capabilities).

**Requirements**: M-8 | **ACs**: AC-16

- [x] T016 [US5] Create plugin manifest file declaring name (`rusty-brain`), version, description, binary_path, and capabilities (`chat_hook`, `tool_hook`, `mind_tool`) per data-model.md Plugin Manifest schema at `plugin-manifest.json`

- [ ] T016a [US5] Verify and update `plugin-manifest.json` to match final OpenCode protocol from Spike-1 (fields: name `rusty-brain`, version, description, binary_path, capabilities `chat_hook`, `tool_hook`, `mind_tool`)

**Checkpoint**: Plugin manifest exists with valid structure. T016a tracks revalidation after Spike-1 confirms protocol.

---

## Phase 8: User Story 6 — Orphaned Sidecar Cleanup (Priority: P3)

**Goal**: On session start, scan `.opencode/` for stale sidecar files (>24h old) and delete them. Self-healing, no background process.

**Independent Test**: Create sidecar files with >24h timestamps, trigger session start → verify stale files deleted, fresh files preserved.

**Requirements**: S-2 | **ACs**: AC-17 | **SEC**: SEC-12

### Tests for User Story 6

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T017 [US6] Write orphan cleanup unit tests covering: stale files >24h deleted (AC-17), fresh files <24h preserved, only `session-*.json` pattern matched (SEC-12), no recursive deletion into subdirectories (SEC-12), error during scan/delete logs WARN and continues (EC-7), and empty directory handled gracefully in `crates/opencode/tests/sidecar_test.rs`

### Implementation for User Story 6

- [x] T018 [US6] Implement `cleanup_stale` per contracts/sidecar_api.rs: scan directory for `session-*.json` files, check `metadata.modified()` against 24h threshold, delete stale files, WARN on individual errors, never panic, no recursion in `crates/opencode/src/sidecar.rs`

**Checkpoint**: Orphan cleanup deletes stale sidecars, preserves fresh ones, fails-open on errors. US6 independently testable.

---

## Phase 9: CLI Integration & Polish

**Purpose**: Wire all handler modules into the CLI binary via subcommands; final quality gates.

- [x] T019 [P] Add `Opencode` variant with subcommands (`ChatHook`, `ToolHook`, `Mind`, `SessionCleanup`, `SessionStart`) to the Command enum in `crates/cli/src/args.rs`
- [x] T020 Create OpenCode subcommand handlers: read JSON from stdin, deserialize to typed input, dispatch to library handlers wrapped in fail-open, serialize output to stdout JSON, tracing to stderr in `crates/cli/src/opencode_cmd.rs`. Note: `SessionStart` reads `HookInput` from stdin (for `cwd` and `session_id`), resolves sidecar directory from `cwd`, then calls `cleanup_stale`. All other subcommands also read `HookInput` or `MindToolInput` from stdin.
- [x] T021 Extend main dispatch to route `Opencode` subcommands to handlers in `crates/cli/src/main.rs`
- [x] T022 [P] Write SEC-3 logging audit tests verifying no memory content (observations, search results, context) is logged at INFO level or above; verify WARN traces contain only error context, not memory payloads (SEC-3) in `crates/opencode/tests/logging_test.rs`
- [x] T023 [P] Write performance benchmark tests with timer assertions: chat hook context injection completes within 200ms (SC-001), tool hook observation capture completes within 750ms including Mind::open() (SC-002 handler-only target: <100ms p95 excluding Mind::open()); use known-size memory files for reproducibility in `crates/opencode/tests/perf_test.rs`
- [x] T024 Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T025 Run `cargo fmt --check` and fix any formatting issues
- [x] T026 Run quickstart.md manual verification commands (chat-hook, tool-hook, mind modes, session-cleanup, session-start) and verify structured JSON output

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1, US2, US3 (Phases 3–5)**: All depend on Foundational (Phase 2)
  - These three P1 stories CAN proceed in parallel (different files, independent handlers)
  - Or sequentially: US1 → US2 → US3
- **US4, US5 (Phases 6–7)**: Depend on Foundational (Phase 2) — can run in parallel with US1–US3
- **US6 (Phase 8)**: Depends on Foundational (Phase 2) — can run in parallel with US1–US5
- **CLI Integration & Polish (Phase 9)**: Depends on ALL user story phases completing (T009, T011, T013, T015, T018 minimum). T022 and T023 can run in parallel with CLI wiring (T019–T021). T024–T026 run after all tasks complete.

### User Story Dependencies

- **US1 (Chat Hook)**: Foundational only — no dependencies on other stories
- **US2 (Tool Hook)**: Foundational only — uses sidecar module (built in Foundational)
- **US3 (Mind Tool)**: Foundational only — no dependencies on other stories
- **US4 (Session Cleanup)**: Foundational only — uses sidecar module (load + delete)
- **US5 (Plugin Manifest)**: Foundational only — standalone static file
- **US6 (Orphan Cleanup)**: Foundational only — extends sidecar module

### Within Each User Story

1. Tests MUST be written and FAIL before implementation (Constitution V, non-negotiable)
2. Implementation makes tests pass
3. Verify story checkpoint before moving to next

### Implementation Guardrails (from AR — verify at each checkpoint)

- [x] No `deny_unknown_fields` on input structs (M-7)
- [x] No `get_mind()` singleton — `Mind::open(config)` per invocation
- [x] No memory content logged at INFO+ (Constitution IX)
- [x] No interactive prompts or `eprintln!` in library code (Constitution III)
- [x] No new external crates (Constitution XIII) — tracing-subscriber added as dev-dep only
- [x] No stdin/stdout I/O in `crates/opencode` — I/O in `crates/cli` only
- [x] All handler entry points wrapped in fail-open (M-5)
- [x] Atomic writes for sidecar (Constitution VII)
- [x] `resolve_memory_path(cwd, "opencode", false)` for LegacyFirst (M-6)
- [x] `tracing::warn!` for all fail-open errors (Constitution XI)
- [x] Sidecar files with 0600 permissions (SEC-2)
- [x] Mind tool mode validated against `VALID_MODES` whitelist (SEC-8)
- [x] No memory content logged at INFO+ verified by test (SEC-3, T022)
- [x] Performance benchmarks pass: chat hook <200ms, tool hook <750ms end-to-end including Mind::open() (handler-only <100ms p95 excluding Mind::open()) (SC-001, SC-002, T023)

---

## Parallel Opportunities

### Foundational Phase (Phase 2)

```text
# T003 first (types must compile before tests can reference them):
Task T003: Types in crates/opencode/src/types.rs

# Then these two test tasks can run in parallel:
Task T004: Sidecar tests in crates/opencode/tests/sidecar_test.rs
Task T005: Fail-open tests in crates/opencode/tests/failopen_test.rs
```

### P1 User Stories (Phases 3–5) — After Foundational

```text
# These three story test tasks can run in parallel:
Task T008: [US1] Chat hook tests in crates/opencode/tests/chat_hook_test.rs
Task T010: [US2] Tool hook tests in crates/opencode/tests/tool_hook_test.rs
Task T012: [US3] Mind tool tests in crates/opencode/tests/mind_tool_test.rs

# Then these three implementations can run in parallel:
Task T009: [US1] Chat hook in crates/opencode/src/chat_hook.rs
Task T011: [US2] Tool hook in crates/opencode/src/tool_hook.rs
Task T013: [US3] Mind tool in crates/opencode/src/mind_tool.rs
```

### All User Stories (Phases 3–8) — Maximum Parallelism

With 6 developers, ALL user stories can proceed in parallel after Foundational completes. Each story is independently testable.

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001–T002)
2. Complete Phase 2: Foundational (T003–T007)
3. Complete Phase 3: User Story 1 — Chat Hook (T008–T009)
4. **STOP and VALIDATE**: Chat hook injects context from known memory, fails-open on errors
5. This delivers the core value proposition: memory context in conversations

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. US1 (Chat Hook) → Read path works → **MVP!**
3. US2 (Tool Hook) → Write path works → Memory accumulates
4. US3 (Mind Tool) → Direct access works → Full interaction model
5. US4 (Session Cleanup) → Summaries generated → Memory quality improves
6. US5 (Plugin Manifest) → OpenCode discovers plugin → End-to-end integration
7. US6 (Orphan Cleanup) → Housekeeping → Production-ready
8. CLI Integration → Binary wired up → Deployable

### Suggested MVP Scope

**US1 (Chat Hook) alone** delivers the core value proposition. A developer can experience memory-enhanced conversations with just the chat hook working. Add US2 next to enable the write path.

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- All tests must FAIL before implementation begins (Constitution V)
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- All handler logic lives in `crates/opencode` (library) — NO stdin/stdout I/O
- All I/O lives in `crates/cli/src/opencode_cmd.rs` (binary)
- Fail-open wrapper ensures valid JSON output for every handler invocation
