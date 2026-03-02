# Tasks: Core Memory Engine

**Input**: Design documents from `/specs/003-core-memory-engine/`
**Prerequisites**: plan.md (required), spec.md (required), prd.md, ar.md, sec.md, research.md, data-model.md, contracts/mind-api.rs, quickstart.md

**Tests**: Tests are included per Constitution V (Test-First Development) and project testing requirements. Tests MUST be written first and verified to FAIL before implementation.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Workspace dependency additions, Cargo.toml updates, and project structure creation

- [x] T001 Add `ulid`, `fs2`, and `tempfile` workspace dependencies to `Cargo.toml` (root)
- [x] T002 Enable `lex` feature on `memvid-core` workspace dependency in `Cargo.toml` (root)
- [x] T003 Update `crates/core/Cargo.toml` with all dependencies per quickstart.md: `rusty-brain-types`, `ulid`, `fs2`, `serde`, `serde_json`, `chrono`, `tracing` (deps) and `tempfile`, `tokio` (dev-deps)
- [x] T004 Create module files for project structure in `crates/core/src/`: `mind.rs`, `backend.rs`, `memvid_store.rs`, `file_guard.rs`, `context_builder.rs`, `token.rs` — each as empty modules with doc comments
- [x] T005 Update `crates/core/src/lib.rs` with module declarations and public re-exports per contracts/mind-api.rs: `Mind`, `MemorySearchResult`, `estimate_tokens`, `get_mind`, `reset_mind`

**Checkpoint**: Project compiles (`cargo check -p rusty-brain-core`) with empty modules

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Types crate prerequisites, core abstractions, and internal infrastructure that MUST be complete before ANY user story

**CRITICAL**: No user story work can begin until this phase is complete

### Types Crate Prerequisites (research.md Section 4)

- [x] T006 [P] Rename `AgentBrainError` to `RustyBrainError` across `crates/types/src/error.rs` and all references in `crates/types/src/` — add `pub type AgentBrainError = RustyBrainError;` type alias for backwards compatibility
- [x] T007 [P] Add `ulid` dependency to `crates/types/Cargo.toml` and change `Observation.id` from `Uuid` to `Ulid` in `crates/types/src/observation.rs` — update `Observation::new` to use `ulid::Ulid::new()`, update serialization, update all tests
- [x] T008 Change `Observation.content` from `String` (required) to `Option<String>` (optional) in `crates/types/src/observation.rs` — update `Observation::new` signature, remove non-empty validation for content, update serde attrs (`skip_serializing_if = "Option::is_none"`), update all tests *(depends on T007 — both modify observation.rs)*
- [x] T009 Add `RustyBrainError` variants needed by core crate: `Storage` (wraps memvid errors), `CorruptedFile`, `FileTooLarge`, `LockTimeout` in `crates/types/src/error.rs` — each with stable error code from `error_codes` module
- [x] T010 Update `crates/types/src/lib.rs` re-exports to include `RustyBrainError` (alongside `AgentBrainError` alias) and verify `cargo test -p types` passes

### Core Abstractions

- [x] T011 Implement `estimate_tokens()` pure function in `crates/core/src/token.rs` — `pub fn estimate_tokens(text: &str) -> usize` returning `text.len() / 4` (M-9, FR-018)
- [x] T012 Write unit tests for `estimate_tokens()` in `crates/core/src/token.rs` — test empty string (0), short string, long string, unicode characters
- [x] T013 Define `MemvidBackend` trait and internal types (`SearchHit`, `TimelineEntry`, `FrameInfo`, `BackendStats`, `OpenAction`) in `crates/core/src/backend.rs` per contracts/mind-api.rs
- [x] T014 Implement `MockBackend` (test-only `MemvidBackend` impl using in-memory `Vec`) in `crates/core/src/backend.rs` — supports `put`, `find`, `timeline`, `frame_by_id`, `stats`, `commit`, `ask`
- [x] T015 Write unit tests for `MockBackend` in `crates/core/src/backend.rs` — verify put/find round-trip, timeline ordering, frame_by_id retrieval, stats computation
- [x] T016 Implement `FileGuard::validate_and_open()` in `crates/core/src/file_guard.rs` — path validation (reject `/dev/`, `/proc/`, `/sys/` per SEC-6), missing file → `OpenAction::Create` (create parent dirs), existing file size check (>100MB → `Err(FileTooLarge)` per S-6), valid file → `OpenAction::Open` (SEC-1: set 0600 permissions on creation)
- [x] T017 Write unit tests for `FileGuard::validate_and_open()` in `crates/core/src/file_guard.rs` — test missing file, existing valid file, oversized file (mock), system path rejection, parent dir creation
- [x] T018 Implement `MemvidStore` (production `MemvidBackend` impl wrapping `memvid-core`) in `crates/core/src/memvid_store.rs` — wrap `Memvid` handle in `Mutex`, implement all trait methods with error conversion to `RustyBrainError` (M-7), map `put_bytes`/`PutOptions` per research.md Section 6
- [x] T019 Write integration test for `MemvidStore` in `crates/core/src/memvid_store.rs` (moved from external integration test since types are `pub(crate)`) — create temp `.mv2` file, `put` → `find` → verify text match, `timeline` → verify frame order, `commit` → reopen → verify persistence

### Test Infrastructure

- [x] T073 [P] Create shared test helpers in `crates/core/tests/common/mod.rs` — temp dir creation, fixture builders (pre-populated MockBackend with N observations), assertion helpers for `MemorySearchResult` field verification

**Checkpoint**: Foundation ready — `cargo test -p types && cargo test -p rusty-brain-core` passes. User story implementation can now begin.

---

## Phase 3: User Story 1 — Store and Retrieve Observations (Priority: P1) MVP

**Goal**: An AI agent can store observations and retrieve them via search/ask, with all metadata preserved across store/retrieve cycles.

**PRD Requirements**: M-1, M-2, M-3, M-4, M-6, M-7, M-8

**Independent Test**: Store a set of observations, search by query, verify content/metadata fidelity, ask a question and verify answer or fallback.

### Tests for User Story 1

> **Write these tests FIRST, ensure they FAIL before implementation**

- [x] T020 [P] [US1] Write unit test for `Mind::open` (create new file) in `crates/core/src/mind.rs` — test with MockBackend: new file path → creates `.mv2`, returns initialized Mind with session_id, memory_path, is_initialized=true (AC-1, M-1)
- [x] T021 [P] [US1] Write unit test for `Mind::open` (open existing file) in `crates/core/src/mind.rs` — test with MockBackend: existing file → opens successfully, all operations available (AC-7, M-6)
- [x] T022 [P] [US1] Write unit test for `Mind::remember` in `crates/core/src/mind.rs` — test with MockBackend: store observation with all fields (including tool_name) → returns ULID string, verify ULID format, verify empty summary rejected (SEC-5), verify empty tool_name rejected, verify content optional (EC-4) (AC-2, M-2)
- [x] T023 [P] [US1] Write unit test for `Mind::search` in `crates/core/src/mind.rs` — test with MockBackend: store obs → search → verify results contain obs_type, summary, timestamp, score, tool_name; test empty results (EC-3); test limit param (AC-3, M-3)
- [x] T024 [P] [US1] Write unit test for `Mind::ask` in `crates/core/src/mind.rs` — test with MockBackend: store obs → ask question → verify answer returned; test no matches → "No relevant memories found" fallback (AC-4, M-4)
- [x] T025 [P] [US1] Write unit test for error wrapping in `crates/core/src/mind.rs` — verify all errors are `RustyBrainError` with stable error codes; verify memvid errors wrapped with source preserved (AC-8, M-7)
- [x] T026 [P] [US1] Write unit test for `Mind` accessors in `crates/core/src/mind.rs` — verify `session_id()` returns valid ULID, `memory_path()` matches config, `is_initialized()` returns true after open (AC-9, M-8)
- [x] T027 [US1] Write integration test for store→search round-trip in `crates/core/tests/integration/mind_roundtrip.rs` — use real MemvidStore: store observation with all metadata fields → search → verify 100% field fidelity (SC-001)
- [ ] T070 [US1] Write integration test for file-deleted-between-operations (EC-6) in `crates/core/tests/integration/mind_roundtrip.rs` — DEFERRED: requires Mind to detect and recreate .mv2 on operations (Phase 7 prerequisite)
- [x] T071 [US1] Write integration test for read-only filesystem (EC-2) in `crates/core/tests/integration/mind_roundtrip.rs` — set directory permissions to read-only, call `Mind::open` → verify returns `RustyBrainError` (not panic), verify error has stable error code

### Implementation for User Story 1

- [x] T028 [US1] Implement `Mind` struct with internal fields in `crates/core/src/mind.rs` — backend (`Box<dyn MemvidBackend>`), config (`MindConfig`), session_id (`String`), memory_path (`PathBuf`), initialized (`bool`), cached_stats (`Mutex<Option<MindStats>>`); ensure `Send + Sync` (compile-time assertion)
- [x] T029 [US1] Implement `Mind::open` in `crates/core/src/mind.rs` — call `FileGuard::validate_and_open`, on `Create` → `backend.create` + create parent dirs, on `Open` → `backend.open`, generate session ULID, set initialized=true, return `Mind` (M-1, M-6)
- [x] T030 [US1] Implement `Mind::remember` in `crates/core/src/mind.rs` — validate summary non-empty (SEC-5), validate tool_name non-empty, generate ULID + timestamp, serialize observation to JSON bytes for `put_bytes` payload (summary + content concatenated), set `PutOptions` (labels=[obs_type], tags=[tool_name, session_id], metadata=full observation JSON), call `backend.put` + `backend.commit`, invalidate stats cache, return ULID string (M-2, FR-003, FR-004)
- [x] T031 [US1] Implement `Mind::search` in `crates/core/src/mind.rs` — call `backend.find(query, limit.unwrap_or(10))`, parse `SearchHit` metadata JSON to extract `MemorySearchResult` fields (obs_type, summary, content_excerpt, timestamp, score, tool_name), return `Vec<MemorySearchResult>` (M-3, FR-005)
- [x] T032 [US1] Implement `Mind::ask` in `crates/core/src/mind.rs` — call `backend.ask(question, 10)`, return answer string or "No relevant memories found." if empty (M-4, FR-006)
- [x] T033 [US1] Implement `Mind` accessors (`session_id`, `memory_path`, `is_initialized`) in `crates/core/src/mind.rs` (M-8, FR-015, FR-016, FR-017)
- [x] T034 [US1] Implement `MemorySearchResult` struct in `crates/core/src/mind.rs` per contracts/mind-api.rs
- [x] T035 [US1] Add `tracing` spans to all public `Mind` methods in `crates/core/src/mind.rs` — `info!` for lifecycle events, `debug!` for operation summaries (query, result count), never log content at INFO+ (SEC-3, Constitution IX, XI)

**Checkpoint**: User Story 1 complete — store, search, ask operations work end-to-end. Run `cargo test -p rusty-brain-core` to verify.

---

## Phase 4: User Story 2 — Provide Session Context on Startup (Priority: P1)

**Goal**: Assemble a token-budgeted context payload containing recent observations, query-relevant memories, and session summaries for agent session startup.

**PRD Requirements**: M-5, FR-007

**Independent Test**: Populate store with known observations and summaries, request context with/without query, verify payload contains expected items within token budget.

### Tests for User Story 2

> **Write these tests FIRST, ensure they FAIL before implementation**

- [x] T036 [P] [US2] Write unit test for `ContextBuilder::build` (recent observations) in `crates/core/src/context_builder.rs` — test with MockBackend: populate store → build context → verify recent observations present, ordered newest-first, capped at `max_context_observations` (AC-5)
- [x] T037 [P] [US2] Write unit test for `ContextBuilder::build` (relevant memories with query) in `crates/core/src/context_builder.rs` — test with MockBackend: populate store → build context with query → verify relevant memories from `find` included, capped at `max_relevant_memories` (AC-5)
- [x] T038 [P] [US2] Write unit test for `ContextBuilder::build` (session summaries) in `crates/core/src/context_builder.rs` — test with MockBackend: populate store with session summaries → build context → verify up to `max_session_summaries` summaries included (AC-5)
- [x] T039 [P] [US2] Write unit test for `ContextBuilder::build` (token budget enforcement) in `crates/core/src/context_builder.rs` — test with MockBackend: populate store with large content → build context with small token budget → verify total payload ≤ budget tokens, verify truncation of oversized single observation (AC-6, EC-5, SC-002)
- [x] T040 [US2] Write unit test for `Mind::get_context` in `crates/core/src/mind.rs` — verify delegation to ContextBuilder, verify with and without query parameter

### Implementation for User Story 2

- [x] T041 [US2] Implement `ContextBuilder::build` in `crates/core/src/context_builder.rs` — (1) get recent via `backend.timeline(max_context_observations, reverse=true)`, enrich with `backend.frame_by_id` to get full metadata, parse to `Observation`; (2) if query: get relevant via `backend.find(query, max_relevant_memories)`, parse to `MemorySearchResult`; (3) get summaries via `backend.find("session_summary", max_session_summaries)`, parse to `SessionSummary`; (4) apply token budget (chars/4), truncate if needed; (5) return `InjectedContext` with `token_count` (M-5, FR-007)
- [x] T042 [US2] Implement `Mind::get_context` in `crates/core/src/mind.rs` — delegate to `context_builder::build(backend, config, query)` (M-5)

**Checkpoint**: User Story 2 complete — context assembly works with token budgeting. Verify with `cargo test -p rusty-brain-core`.

---

## Phase 5: User Story 3 — Save Session Summaries (Priority: P2)

**Goal**: Store session summaries as tagged, searchable observations that appear in future context injections.

**PRD Requirements**: S-1, FR-008

**Independent Test**: Save a session summary with known decisions/files, search for it, verify it appears in context payload.

### Tests for User Story 3

> **Write these tests FIRST, ensure they FAIL before implementation**

- [x] T043 [P] [US3] Write unit test for `Mind::save_session_summary` in `crates/core/src/mind.rs` — test with MockBackend: save summary → verify stored as observation with `obs_type=Decision`, tagged with session_id and "session_summary", verify decisions/files/summary serialized in content (AC-10, S-1)
- [x] T044 [US3] Write integration test for session summary round-trip in `crates/core/tests/integration/mind_context.rs` — use real MemvidStore: save summary → get_context → verify summary appears in `session_summaries`, verify reverse chronological order with multiple summaries (AC-10)

### Implementation for User Story 3

- [x] T045 [US3] Implement `Mind::save_session_summary` in `crates/core/src/mind.rs` — serialize `SessionSummary` to JSON, store via `remember` with `obs_type=Decision`, tags=["session_summary", session_id], content=serialized summary JSON, invalidate stats cache, return ULID (S-1, FR-008)

**Checkpoint**: User Story 3 complete — session summaries stored and retrievable in context. Verify with `cargo test -p rusty-brain-core`.

---

## Phase 6: User Story 4 — Report Memory Statistics (Priority: P2)

**Goal**: Compute and return statistics about the memory store with caching for repeated calls.

**PRD Requirements**: S-2, S-3, FR-009, FR-010

**Independent Test**: Populate store with known observations, request stats, verify all computed values match expectations, verify caching behavior.

### Tests for User Story 4

> **Write these tests FIRST, ensure they FAIL before implementation**

- [x] T046 [P] [US4] Write unit test for `Mind::stats` (computation) in `crates/core/src/mind.rs` — test with MockBackend: populate store → stats → verify total_observations, total_sessions, oldest_memory, newest_memory, file_size_bytes, observation_type_counts (AC-11, S-2)
- [x] T047 [P] [US4] Write unit test for `Mind::stats` (caching) in `crates/core/src/mind.rs` — test with MockBackend: stats → stats again (no new obs) → verify cached result returned (same computation); store new obs → stats → verify recomputed (AC-12, S-3)

### Implementation for User Story 4

- [x] T048 [US4] Implement `Mind::stats` in `crates/core/src/mind.rs` — check cached_stats: if `Some` and `frame_count` matches `backend.stats().frame_count`, return cached; otherwise compute from `backend.timeline` iteration + `backend.frame_by_id` to count types/sessions/timestamps, get `file_size` from `backend.stats`, cache result, return `MindStats` (S-2, S-3, FR-009, FR-010)

**Checkpoint**: User Story 4 complete — statistics with caching work. Verify with `cargo test -p rusty-brain-core`.

---

## Phase 7: User Story 5 — Handle Corrupted Memory Files Gracefully (Priority: P2)

**Goal**: Detect corrupted files on open, back them up, and initialize fresh stores automatically.

**PRD Requirements**: S-4, S-5, S-6, FR-011, FR-012, FR-013

**Independent Test**: Provide a corrupted file, attempt open, verify backup created and fresh store initialized.

### Tests for User Story 5

> **Write these tests FIRST, ensure they FAIL before implementation**

- [x] T049 [P] [US5] Write unit test for `FileGuard::backup_and_prune` in `crates/core/src/file_guard.rs` — test backup creation with timestamped name `{path}.backup-{YYYYMMDD-HHMMSS}`, test backup file permissions (0600 per SEC-2), test pruning: 4 existing backups → only 3 retained after new backup (AC-13, AC-14, S-5)
- [x] T050 [P] [US5] Write unit test for corruption detection in `crates/core/src/mind.rs` — test with real MemvidStore: create file with invalid content → `Mind::open` → verify corrupted file backed up and fresh store created (AC-13, S-4)
- [x] T051 [US5] Write integration test for corruption recovery in `crates/core/tests/integration/mind_recovery.rs` — write garbage bytes to `.mv2` file → `Mind::open` → verify backup file exists, verify new Mind is functional (remember + search work), verify backup count ≤ 3 after multiple corruptions (SC-003)

### Implementation for User Story 5

- [x] T052 [US5] Implement `FileGuard::backup_and_prune` in `crates/core/src/file_guard.rs` — rename file to `{path}.backup-{YYYYMMDD-HHMMSS}`, set backup permissions to 0600 (SEC-2), glob for `{path}.backup-*`, sort by timestamp descending, delete beyond `max_backups` (default 3) (S-5, FR-012)
- [x] T053 [US5] Update `Mind::open` corruption recovery flow in `crates/core/src/mind.rs` — when `backend.open` returns corruption error: call `FileGuard::backup_and_prune`, then `backend.create` to initialize fresh store, log recovery at `info!` level (S-4, FR-011)

**Checkpoint**: User Story 5 complete — corruption detection, backup, and recovery work. Verify with `cargo test -p rusty-brain-core`.

---

## Phase 8: User Story 6 — Support Concurrent Access (Priority: P3)

**Goal**: Cross-process file locking with retry/backoff to prevent concurrent write corruption.

**PRD Requirements**: C-1, C-2, C-3, FR-014, FR-019, FR-020

**Independent Test**: Spawn multiple processes writing simultaneously, verify no data corruption.

### Tests for User Story 6

> **Write these tests FIRST, ensure they FAIL before implementation**

- [x] T054 [P] [US6] Write unit test for `with_lock` wrapper in `crates/core/src/mind.rs` — test lock acquisition, test retry with exponential backoff on contention, test lock file permissions (0600 per SEC-9) (AC-16, C-1)
- [x] T055 [P] [US6] Write unit test for `get_mind`/`reset_mind` singleton in `crates/core/src/lib.rs` — test first call creates instance, subsequent calls return same `Arc<Mind>`, `reset_mind` clears instance, next call creates new (C-2, C-3)
- [x] T056 [US6] Write integration test for concurrent access in `crates/core/tests/integration/mind_concurrent.rs` — spawn 2+ threads writing via locked Mind, verify no data corruption across 100 sequential writes (SC-004)

### Implementation for User Story 6

- [x] T057 [US6] Implement `with_lock` wrapper in `crates/core/src/mind.rs` — open `.lock` file adjacent to `.mv2`, use `fs2::FileExt::try_lock_exclusive`, implement exponential backoff (100ms base, 5 retries, 2x multiplier), timeout → `RustyBrainError::LockTimeout`, set lock file permissions to 0600 (SEC-9) (C-1, FR-014)
- [x] T058 [US6] Implement `get_mind` and `reset_mind` singleton in `crates/core/src/lib.rs` — use `std::sync::Mutex<Option<Arc<Mind>>>` for thread-safe lazy initialization, `reset_mind` clears for testing (C-2, C-3, FR-019, FR-020)

**Checkpoint**: User Story 6 complete — concurrent access is safe. Verify with `cargo test -p rusty-brain-core`.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Security hardening, compile-time assertions, quality gates, and final validation

- [x] T059 [P] Add compile-time `Send + Sync` assertions for `Mind` in `crates/core/src/mind.rs` — `const _: () = { fn assert_send_sync<T: Send + Sync>() {} fn _check() { assert_send_sync::<Mind>(); } };`
- [x] T060 [P] Verify no memory content logged at INFO+ in `crates/core/src/` — grep for `info!`, `warn!`, `error!` calls, ensure none contain observation content/summary/metadata fields (SEC-3)
- [x] T061 [P] Verify error messages do not leak observation content — review all `RustyBrainError` Display impls and error construction sites (SEC-4)
- [x] T062 [P] Run `cargo clippy -p rusty-brain-core -- -D warnings` and fix any warnings
- [x] T063 [P] Run `cargo fmt -p rusty-brain-core -- --check` and fix any formatting issues
- [x] T064 Run `cargo test` (full workspace) to verify no regressions from types crate changes
- [x] T065 Run quickstart.md validation — verify all code examples from `specs/003-core-memory-engine/quickstart.md` compile and behave as documented

### Benchmarks (Constitution VIII)

- [x] T066 [P] Add criterion benchmark for store operation in `crates/core/benches/store.rs` — measure `Mind::remember` latency at 10K observations, target <500ms p95 (SC-008)
- [x] T067 [P] Add criterion benchmark for search operation in `crates/core/benches/search.rs` — measure `Mind::search` latency at 10K observations, target <500ms p95 (SC-009)
- [x] T068 [P] Add criterion benchmark for context assembly in `crates/core/benches/context.rs` — measure `Mind::get_context` latency at 10K observations, target <2s p95 (SC-010)
- [x] T069 [P] Add criterion benchmark for stats computation in `crates/core/benches/stats.rs` — measure `Mind::stats` latency at 10K observations, target <2s p95 (SC-005)

### Benchmark Infrastructure

- [x] T074 Add `criterion` dev-dependency to workspace `Cargo.toml` and `crates/core/Cargo.toml`; configure `[[bench]]` targets for `store`, `search`, `context`, `stats` in `crates/core/Cargo.toml`

### Supply Chain Security

- [x] T072 Run `cargo audit` with zero critical vulnerabilities — verify all dependencies pass audit (SEC-7)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — **BLOCKS all user stories**
  - T006, T007 (types changes) can run in parallel; T008 depends on T007 (both modify observation.rs)
  - T009, T010 depend on T006 completion (error rename)
  - T011-T012 (token) can run in parallel with everything else in Phase 2
  - T013 (backend trait) must precede T014 (MockBackend) and T018 (MemvidStore)
  - T016-T017 (FileGuard) can run in parallel with T013-T015
  - T018-T019 (MemvidStore) depends on T013
- **User Story 1 (Phase 3)**: Depends on Phase 2 — **MVP story**
- **User Story 2 (Phase 4)**: Depends on Phase 3 (US1 must be working for context to use)
- **User Story 3 (Phase 5)**: Depends on Phase 3 (uses `remember` internally)
- **User Story 4 (Phase 6)**: Depends on Phase 3 (needs working backend for stats)
- **User Story 5 (Phase 7)**: Depends on Phase 2 only (FileGuard + Mind::open)
  - Can proceed in parallel with Phases 4-6 after Phase 3 checkpoint
- **User Story 6 (Phase 8)**: Depends on Phase 3 (needs working Mind for locking)
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Depends on Foundational (Phase 2) — no other story dependencies
- **US2 (P1)**: Depends on US1 — context assembly reads from store populated by US1
- **US3 (P2)**: Depends on US1 — `save_session_summary` uses `remember` internally
- **US4 (P2)**: Depends on US1 — stats computed from stored observations
- **US5 (P2)**: Depends on Foundational only — can start after Phase 2
- **US6 (P3)**: Depends on US1 — locking wraps working Mind operations

### Within Each User Story

1. Tests MUST be written and FAIL before implementation
2. Tests marked [P] can run in parallel
3. Implementation tasks follow test tasks
4. Story complete before checkpoint validation

### Parallel Opportunities

**Phase 2 (max parallelism)**:
- Agent 1: T006, T009, T010 (error rename chain)
- Agent 2: T007 (ULID migration)
- Agent 3: T008 (content optionality)
- Agent 4: T011, T012 (token util)
- Agent 5: T013, T014, T015 (backend trait + mock)
- Agent 6: T016, T017 (FileGuard)
- Agent 7: T018, T019 (MemvidStore) — after T013

**Phase 3 (test parallelism)**:
- T020-T027 can all run in parallel (different test functions, same file)

**Phases 4-6 (after US1)**:
- US3 (Phase 5) and US4 (Phase 6) can run in parallel
- US5 (Phase 7) can run in parallel with Phases 4-6

---

## Parallel Example: Phase 2 Foundation

```
# Launch types crate changes in parallel:
Agent 1: "Rename AgentBrainError to RustyBrainError in crates/types/src/error.rs"
Agent 2: "Change Observation.id from Uuid to Ulid in crates/types/src/observation.rs"
# T008 runs AFTER T007 completes (both modify observation.rs):
Agent 3 (after Agent 2): "Change Observation.content to Option<String> in crates/types/src/observation.rs"

# After types changes merge, launch core abstractions in parallel:
Agent 4: "Implement estimate_tokens in crates/core/src/token.rs"
Agent 5: "Define MemvidBackend trait in crates/core/src/backend.rs"
Agent 6: "Implement FileGuard in crates/core/src/file_guard.rs"
```

## Parallel Example: User Story 1 Tests

```
# Launch all US1 tests in parallel:
Agent 1: "Write Mind::open tests in crates/core/src/mind.rs"
Agent 2: "Write Mind::remember tests in crates/core/src/mind.rs"
Agent 3: "Write Mind::search tests in crates/core/src/mind.rs"
Agent 4: "Write Mind::ask tests in crates/core/src/mind.rs"
Agent 5: "Write integration roundtrip test in crates/core/tests/integration/mind_roundtrip.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (Store + Retrieve + Search + Ask)
4. **STOP and VALIDATE**: `cargo test -p rusty-brain-core` — all green
5. Tag MVP milestone

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add US1 → Test independently → **MVP!** (store/retrieve/search/ask)
3. Add US2 → Test independently → Context injection works
4. Add US3 + US4 (parallel) → Session summaries + Stats
5. Add US5 → Corruption recovery hardened
6. Add US6 → Concurrent access safe
7. Polish → Quality gates pass → Ready for Phase 3 (Compression)

### Suggested MVP Scope

**MVP = Phase 1 + Phase 2 + Phase 3 (User Story 1)**

This delivers the foundational capability that everything else builds upon:
- Create/open memory files
- Store observations with full metadata
- Search past observations by query
- Ask questions against stored knowledge
- All errors structured with stable codes

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable at its checkpoint
- Constitution V requires tests before implementation (TDD)
- All memvid types must stay behind `MemvidBackend` trait boundary (Constitution II, AR guardrails)
- No `unsafe` code — workspace lint `unsafe_code = "forbid"` (Constitution II)
- No observation content in logs at INFO+ (SEC-3, Constitution IX)
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
