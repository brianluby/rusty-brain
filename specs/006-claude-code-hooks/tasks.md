# Tasks: Claude Code Hooks

**Input**: Design documents from `/specs/006-claude-code-hooks/`
**Prerequisites**: plan.md (required), spec.md (required), prd.md, ar.md, sec.md, data-model.md, contracts/hooks-api.rs, research.md, quickstart.md

**Tests**: Included — constitution mandates test-first development (Principle V, non-negotiable).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1–US5)
- Paths relative to repository root

## Path Conventions

```
crates/hooks/
├── Cargo.toml
├── src/
│   ├── main.rs, lib.rs, io.rs, dispatch.rs, error.rs
│   ├── session_start.rs, post_tool_use.rs, stop.rs, smart_install.rs
│   ├── dedup.rs, truncate.rs, git.rs, manifest.rs, context.rs
└── tests/
    ├── common/mod.rs
    ├── io_test.rs, truncate_test.rs, dedup_test.rs, git_test.rs
    ├── session_start_test.rs, post_tool_use_test.rs, stop_test.rs
    ├── smart_install_test.rs, manifest_test.rs, e2e_test.rs
```

---

## Phase 1: Setup

**Purpose**: Configure the hooks crate with dependencies and foundational types

- [x] T001 Configure crates/hooks/Cargo.toml with workspace dependencies (types, core, platforms, serde, serde_json, clap, tracing, tracing-subscriber, chrono, thiserror) and dev-dependencies (tempfile, assert_cmd, predicates)
- [x] T002 Create crates/hooks/src/lib.rs with module declarations (io, dispatch, error, session_start, post_tool_use, stop, smart_install, dedup, truncate, git, manifest, context) and public re-exports for testing
- [x] T003 [P] Implement HookError enum with stable error codes (E_HOOK_IO, E_HOOK_PARSE, E_HOOK_MIND, E_HOOK_PLATFORM, E_HOOK_GIT, E_HOOK_DEDUP) and From impls in crates/hooks/src/error.rs per contracts/hooks-api.rs

---

## Phase 2: Foundational (Core Infrastructure)

**Purpose**: I/O layer, dispatch, and utility modules that ALL handlers depend on

**CRITICAL**: No handler implementation can begin until this phase is complete

### Tests (write FIRST — must FAIL before implementation)

- [x] T004 [P] Write unit tests for read_input (valid JSON, empty stdin, malformed JSON, unknown fields), write_output (valid serialization), and fail_open (Ok passthrough, Err→continue:true) in crates/hooks/tests/io_test.rs
- [x] T005 [P] Write unit tests for head_tail_truncate (under limit returns as-is, over limit preserves head 60%/tail 40% with truncation marker, empty string, exact boundary, single-char content) in crates/hooks/tests/truncate_test.rs
- [x] T006 [P] Write unit tests for DedupCache (is_duplicate returns false for new entry, true within 60s window, false after expiry; record creates file with atomic write; prune removes expired entries; corrupt file treated as empty; concurrent access safety) in crates/hooks/tests/dedup_test.rs
- [x] T007 [P] Write tests for detect_modified_files (returns file list from real git repo tempdir, returns empty Vec when git not available, returns empty Vec on timeout, returns empty Vec for non-git directory) in crates/hooks/tests/git_test.rs

### Implementation

- [x] T008 [P] Implement read_input (serde_json::from_reader on stdin), write_output (serde_json::to_writer on stdout with newline), and fail_open (Result→HookOutput conversion with tracing::warn on error) in crates/hooks/src/io.rs
- [x] T009 [P] Implement Cli struct (clap::Parser derive) and Subcommand enum (SessionStart, PostToolUse, Stop, SmartInstall) with kebab-case subcommand names in crates/hooks/src/dispatch.rs
- [x] T010 [P] Implement head_tail_truncate with chars/4 token estimation, 60/40 head/tail split, and "[...truncated...]" marker in crates/hooks/src/truncate.rs
- [x] T011 [P] Implement DedupCache with file-based JSON storage at .agent-brain/.dedup-cache.json, std::hash::DefaultHasher for keys, 60s TTL, auto-prune on read, atomic write via temp+rename in crates/hooks/src/dedup.rs
- [x] T012 [P] Implement detect_modified_files using Command::new("git") with hardcoded args ["diff", "--name-only", "HEAD"], 5-second timeout via child.wait_with_output, returns empty Vec on any error in crates/hooks/src/git.rs
- [x] T013 [P] Implement format_system_message that formats InjectedContext and MindStats into markdown system message with recent observations, session summaries, stats, and available commands (/mind:search, /mind:ask, /mind:recent, /mind:stats) in crates/hooks/src/context.rs
- [x] T014 Implement main.rs entry point: clap parse, conditional tracing-subscriber init gated on RUSTY_BRAIN_LOG env var, catch_unwind wrapping read_input+dispatch, fail_open conversion, write_output with "{}" fallback, always exit(0) in crates/hooks/src/main.rs
- [x] T015 [P] Create shared test helpers (sample HookInput builders for each subcommand, temp Mind setup with .mv2 file, temp project directory with .agent-brain/) in crates/hooks/tests/common/mod.rs

**Checkpoint**: Foundation ready — all utility modules tested and working, binary compiles and handles stdin/stdout/fail-open. Handler implementation can begin.

---

## Phase 3: User Story 1 — Session Context Injection (Priority: P1) MVP

**Goal**: Developer starts a Claude Code session and the agent receives context from previous sessions in the system prompt.

**Independent Test**: Invoke `rusty-brain session-start` with valid HookInput JSON on stdin → verify HookOutput contains systemMessage with recent observations, summaries, and commands.

**Requirements**: M-1, M-2, M-3, M-4, M-10, M-11, S-1, S-4, S-5 | SEC-1, SEC-4, SEC-5, SEC-8

### Tests

- [x] T016 [US1] Write integration tests for handle_session_start in crates/hooks/tests/session_start_test.rs: (1) existing .mv2 with 10+ observations returns systemMessage with context, (2) no memory file creates new encrypted .mv2 and returns welcome message, (3) error during init returns fail-open HookOutput with continue:true, (4) legacy .claude/mind.mv2 path detected includes migration suggestion in systemMessage, (5) systemMessage includes available commands

### Implementation

- [x] T017 [US1] Implement handle_session_start in crates/hooks/src/session_start.rs: detect_platform from HookInput, resolve_project_identity from cwd, resolve_memory_path, Mind::open with encryption config, Mind::get_context, Mind::stats, check legacy path (.claude/mind.mv2), format_system_message, return HookOutput with systemMessage

**Checkpoint**: Session-start hook fully functional — `echo '{"session_id":"test","transcript_path":"/tmp/t","cwd":".","permission_mode":"default","hook_event_name":"SessionStart"}' | cargo run -p hooks -- session-start` returns context JSON.

---

## Phase 4: User Story 2 — Tool Observation Capture (Priority: P1)

**Goal**: After each tool execution, the hook captures a compressed observation and stores it in memory, building a knowledge base throughout the session.

**Independent Test**: Invoke `rusty-brain post-tool-use` with tool JSON on stdin → verify observation stored in .mv2 (queryable via Mind::search). Invoke again within 60s with same content → verify dedup skips storage.

**Requirements**: M-2, M-3, M-5, M-6, M-10, S-2, S-6 | SEC-1, SEC-2, SEC-5, SEC-8, SEC-10

### Tests

- [x] T018 [US2] Write integration tests for handle_post_tool_use in crates/hooks/tests/post_tool_use_test.rs: (1) Read tool stores observation with Discovery type and "Read {path}" summary, (2) Edit/Write tools store with Feature type, (3) Bash tool stores with truncated command summary, (4) duplicate within 60s is skipped, (5) tool output >2000 chars is truncated to ~500 tokens, (6) error during storage returns fail-open, (7) unknown tool type uses generic Discovery fallback, (8) dedup cache contains only hashes not content (SEC-2)

### Implementation

- [x] T019 [US2] Implement handle_post_tool_use in crates/hooks/src/post_tool_use.rs: extract tool_name/tool_input/tool_response from HookInput, classify tool_name→ObservationType per data-model.md mapping, generate summary from tool_input, DedupCache::is_duplicate check, head_tail_truncate tool_response to 500 tokens, Mind::remember with obs_type/tool/summary/content, DedupCache::record, return HookOutput with continue:true

**Checkpoint**: Post-tool-use hook captures and deduplicates observations — verified by storing an observation then querying the Mind.

---

## Phase 5: User Story 3 — Session Summary and Shutdown (Priority: P2)

**Goal**: When the session ends, the hook captures a session summary with modified files and stores individual file edits as separate observations for granular searchability. Note: for MVP, the "decisions" field is an empty `Vec` — decision extraction from the transcript is deferred.

**Independent Test**: Invoke `rusty-brain stop` in a git repo with modifications → verify session summary stored and individual file edits stored as separate observations.

**Requirements**: M-2, M-3, M-7, M-10, C-1 | SEC-1, SEC-5, SEC-8, SEC-9

### Tests

- [x] T020 [US3] Write integration tests for handle_stop in crates/hooks/tests/stop_test.rs: (1) session with git modifications detects files and stores summary, (2) each modified file stored as separate observation, (3) session with no changes stores summary noting no modifications, (4) git not available returns empty file list and still stores summary, (5) error during summary generation fails open with graceful mind shutdown, (6) git subprocess arguments are hardcoded string literals (SEC-9 code review assertion)

### Implementation

- [x] T021 [US3] Implement handle_stop in crates/hooks/src/stop.rs: detect_modified_files from cwd, store each file as separate Feature observation via Mind::remember, collect session decisions, Mind::save_session_summary with files/decisions/summary, format summary for HookOutput systemMessage, graceful mind shutdown

**Checkpoint**: Stop hook captures session summary with git-detected file modifications.

---

## Phase 6: User Story 5 — Hook Registration Manifest (Priority: P2)

**Goal**: Generate a hooks.json manifest that tells Claude Code which hooks exist and how to invoke them.

**Independent Test**: Validate generated hooks.json contains entries for all 4 event types (SessionStart, PostToolUse, Stop, Notification) with correct binary+subcommand commands.

**Requirements**: M-9 | SEC-5

### Tests

- [x] T022 [US5] Write unit tests for generate_manifest in crates/hooks/tests/manifest_test.rs: (1) generated JSON contains SessionStart, PostToolUse, Stop, Notification entries, (2) each entry has type "command" with correct "rusty-brain <subcommand>" command string, (3) JSON is valid and parseable, (4) binary name is configurable

### Implementation

- [x] T023 [US5] Implement generate_manifest in crates/hooks/src/manifest.rs: build hooks.json structure per research.md R-6 format with SessionStart→"rusty-brain session-start", PostToolUse→"rusty-brain post-tool-use", Stop→"rusty-brain stop", Notification→"rusty-brain smart-install", serialize to JSON string

**Checkpoint**: hooks.json manifest generated and validated against expected schema.

---

## Phase 7: User Story 4 — Installation and Version Management (Priority: P3)

**Goal**: Track binary installation state via a version marker file for update detection.

**Independent Test**: Invoke `rusty-brain smart-install` → verify .install-version file written. Invoke again → verify no-op fast path.

**Requirements**: M-2, M-3, M-8, M-10 | SEC-5, SEC-8, SEC-10

### Tests

- [x] T024 [US4] Write unit tests for handle_smart_install in crates/hooks/tests/smart_install_test.rs: (1) fresh install writes current version to .install-version and exits 0, (2) matching version is no-op, (3) mismatched version updates .install-version, (4) error during I/O fails open, (5) version file uses atomic write (SEC-10)

### Implementation

- [x] T025 [US4] Implement handle_smart_install in crates/hooks/src/smart_install.rs: read .install-version file (or treat missing as fresh install), compare with current binary version from env!("CARGO_PKG_VERSION"), if match→no-op, if mismatch/missing→atomic write current version, return HookOutput with continue:true

**Checkpoint**: Smart-install tracks version state correctly. All five user stories now implemented.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: End-to-end validation, quality gates, and cross-story verification

- [x] T026 [P] Write E2E tests that invoke the compiled rusty-brain binary as a subprocess with stdin piping and stdout capture in crates/hooks/tests/e2e_test.rs: (1) each subcommand with valid input produces valid HookOutput JSON, (2) empty stdin produces fail-open JSON, (3) malformed JSON produces fail-open JSON, (4) unknown subcommand exits 0, (5) binary exits 0 for every scenario, (6) with RUSTY_BRAIN_LOG=info, stderr output does not contain memory content (SEC-3 verification)
- [x] T027 Run quality gates: cargo test --workspace (all green), cargo clippy --workspace -- -D warnings (zero warnings), cargo fmt --check (formatted), verify no `unwrap()`/`expect()` in handler code, no `std::process::exit()` in handlers, no tokio dependency in crates/hooks
- [ ] T028 Validate all quickstart.md scenarios: build binary, invoke each hook with sample JSON, verify expected outputs, verify debug logging with RUSTY_BRAIN_LOG=debug
- [ ] T029 [P] Write benchmark tests verifying performance targets: session-start <200ms with 1K observations, post-tool-use <100ms with typical tool output (S-5, S-6)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all handler phases
- **US1 (Phase 3)**: Depends on Phase 2 — first handler, validates pipeline end-to-end
- **US2 (Phase 4)**: Depends on Phase 2 — can run in parallel with US1 (different files)
- **US3 (Phase 5)**: Depends on Phase 2 — can run in parallel with US1/US2
- **US5 (Phase 6)**: Depends on Phase 2 — can run in parallel with all other stories
- **US4 (Phase 7)**: Depends on Phase 2 — can run in parallel with all other stories
- **Polish (Phase 8)**: Depends on all story phases completing

### User Story Dependencies

- **US1 (P1)**: No dependencies on other stories. Uses: io, dispatch, error, context, main
- **US2 (P1)**: No dependencies on other stories. Uses: io, dispatch, error, truncate, dedup, main
- **US3 (P2)**: No dependencies on other stories. Uses: io, dispatch, error, git, main
- **US5 (P2)**: No dependencies on other stories. Uses: manifest (standalone module)
- **US4 (P3)**: No dependencies on other stories. Uses: io, dispatch, error, main

### Within Each User Story

1. Tests written FIRST — must FAIL before implementation
2. Implementation to make tests pass
3. Verify all tests GREEN before moving to next story

### Parallel Opportunities

**Phase 2 (Foundational)**:
- T004, T005, T006, T007 can all run in parallel (test files for different modules)
- T008, T009, T010, T011, T012, T013, T015 can all run in parallel (implementation files)
- T014 (main.rs) depends on T008 (io.rs) and T009 (dispatch.rs)

**Phases 3–7 (User Stories)**:
- ALL five user stories can be implemented in parallel after Phase 2 completes
- Each story touches only its own handler file + test file (no cross-story file conflicts)

---

## Parallel Example: All User Stories After Phase 2

```
# After Phase 2 Foundation completes, launch all stories in parallel:

Agent 1: US1 — session_start.rs + session_start_test.rs
Agent 2: US2 — post_tool_use.rs + post_tool_use_test.rs
Agent 3: US3 — stop.rs + stop_test.rs
Agent 4: US5 — manifest.rs + manifest_test.rs
Agent 5: US4 — smart_install.rs + smart_install_test.rs
```

---

## Implementation Strategy

### MVP First (US1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: US1 — Session Context Injection
4. **STOP and VALIDATE**: `echo '...' | rusty-brain session-start` produces context JSON
5. This alone delivers the core value: persistent memory across sessions

### Recommended Full Delivery Order

> **Note**: This delivery order supersedes phase numbers for execution sequence. Phases are numbered by user story grouping, not implementation order.

1. Phase 1 → Phase 2 → **Foundation ready**
2. Phase 7 (US4 — smart-install) → Simplest handler, validates full I/O pipeline end-to-end per AR suggestion
3. Phase 3 (US1 — session-start) → Core read path, MVP value
4. Phase 4 (US2 — post-tool-use) → Core write path, completes read+write loop
5. Phase 5 (US3 — stop) → Session summaries, high-value for continuity
6. Phase 6 (US5 — manifest) → Registration, needed for production deployment
7. Phase 8 → Polish, E2E validation, quality gates

### Incremental Delivery

Each story adds value without breaking previous stories:
- After US1: Agent gets context at session start
- After US2: Agent's actions are recorded as observations
- After US3: Session summaries provide condensed context for future sessions
- After US5: Claude Code can auto-discover and invoke hooks
- After US4: Version tracking for update detection

---

## Notes

- All code paths must fail-open (M-3): valid JSON + exit 0 always
- No stderr unless RUSTY_BRAIN_LOG set (M-10)
- No `unwrap()`/`expect()` in handler code — propagate via Result
- No `std::process::exit()` inside handlers — only in main()
- No tokio/async — synchronous subprocess I/O only
- Atomic writes for dedup cache and version marker (SEC-10)
- Git args hardcoded as string literals (SEC-9)
- Memory files use memvid encryption (M-11)
- Dedup cache stores only hashes, not content (SEC-2)
