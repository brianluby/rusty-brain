# Tasks: Tool-Output Compression

**Input**: Design documents from `/specs/004-tool-output-compression/`
**Prerequisites**: plan.md (required), spec.md (required), prd.md, ar.md, data-model.md, contracts/compression.rs, research.md, quickstart.md

**Tests**: TDD is mandated by project conventions and constitution (Principle V). Test tasks are included for each phase.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Workspace crate**: `crates/compression/src/` for source, `crates/compression/src/*.rs` in-module tests
- **Root workspace**: `Cargo.toml` for workspace dependency additions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization — add dependencies, configure Cargo.toml, establish module layout

- [x] T001 Add `regex = "1"` to `[workspace.dependencies]` in root `Cargo.toml`
- [x] T002 Add `regex = { workspace = true }`, `tracing = { workspace = true }`, and `serde_json = { workspace = true }` to `[dependencies]` in `crates/compression/Cargo.toml` (serde_json needed for C-3 glob JSON array parsing)
- [x] T003 Replace placeholder `lib.rs` with module declarations and public re-exports in `crates/compression/src/lib.rs` — declare modules: `config`, `types`, `truncate`, `generic`, `read`, `lang`, `bash`, `grep`, `glob`, `edit`; re-export `compress`, `CompressionConfig`, `CompressedResult`, `CompressionStatistics`, `ToolType`; mark `Language` enum as `pub(crate)` in lang.rs (internal to compression crate); create stub `pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>) -> String { output.to_string() }` in each compressor module (read, bash, grep, glob, generic) and `pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>, is_write: bool) -> String { output.to_string() }` in edit.rs so the dispatcher compiles from Phase 3 onward
- [x] T004 Verify workspace builds cleanly: `cargo build -p compression` succeeds with no errors

**Checkpoint**: Crate skeleton compiles with all module stubs; no logic yet

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types, configuration, budget enforcer, and generic fallback that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

### Tests for Foundational Phase

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T005 [P] Write unit tests for `CompressionConfig` in `crates/compression/src/config.rs` — test `Default` impl (threshold=3000, budget=2000), test `validate()` rejects budget >= threshold, rejects zero values, accepts valid custom values
- [x] T006 [P] Write unit tests for `ToolType` in `crates/compression/src/types.rs` — test `From<&str>` case-insensitive matching: "read"→Read, "READ"→Read, "Bash"→Bash, "CustomTool"→Other("customtool"), "edit"→Edit, "write"→Write
- [x] T007 [P] Write unit tests for `enforce_budget()` in `crates/compression/src/truncate.rs` — test: text within budget returns unchanged, text exceeding budget is truncated with `[...truncated to N chars]` marker, empty string, Unicode multi-byte chars counted correctly, marker itself fits within budget, budget of 0 edge case
- [x] T008 [P] Write unit tests for generic compressor in `crates/compression/src/generic.rs` — test: head/tail preservation (first 15 + last 10 lines), `[...N lines omitted...]` indicator present, total line count stated, short input returned as-is, single-line input

### Implementation for Foundational Phase

- [x] T009 [P] Implement `CompressionConfig` struct with `Default` and `validate()` in `crates/compression/src/config.rs` — derive Debug, Clone, PartialEq; default threshold=3000, budget=2000; validate returns `Result<(), String>` checking budget < threshold and both > 0
- [x] T010 [P] Implement `ToolType`, `CompressedResult`, `CompressionStatistics` in `crates/compression/src/types.rs` — derive Debug, Clone, PartialEq; implement `From<&str>` for ToolType with `to_ascii_lowercase()` matching; ToolType::Other stores the lowercased name
- [x] T011 Implement `enforce_budget()` in `crates/compression/src/truncate.rs` — use `.chars().count()` for length, truncate from end preserving head, append `[...truncated to N chars]` marker that itself counts toward budget
- [x] T012 Implement generic fallback compressor `compress()` in `crates/compression/src/generic.rs` — head/tail truncation: first 15 lines + `[...N lines omitted...]` + last 10 lines + total line count summary; call `enforce_budget()` as final pass
- [x] T013 Verify foundational tests pass: `cargo test -p compression`

**Checkpoint**: Foundation ready — types compile, budget enforcer works, generic fallback compresses text. User story implementation can now begin.

---

## Phase 3: User Story 3 — Route Compression by Tool Type (Priority: P1) 🎯 MVP Core

**Goal**: Implement the dispatcher entry point in `lib.rs` that gates on threshold, dispatches by tool name, wraps compressors in `catch_unwind`, falls back to generic on failure, and builds `CompressedResult` with statistics.

**Independent Test**: Call `compress()` with each supported tool name and unknown names; verify correct routing, threshold gate, empty/whitespace pass-through, fallback on panic, and statistics calculation.

**Why US3 first**: The dispatcher is the entry point for ALL compression. US1 and US2 implement specialized compressors but need the dispatcher to be callable. Building the dispatcher first with the generic fallback means the full API works end-to-end immediately.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T014 [US3] Write integration tests for `compress()` dispatcher in `crates/compression/src/lib.rs` — test: below-threshold input returns unchanged with `compression_applied: false`; empty input returns unchanged; whitespace-only returns unchanged; unknown tool type routes to generic compressor; result includes `text`, `compression_applied`, `original_size`; statistics present when compressed; case-insensitive tool name matching ("read", "Read", "READ" all dispatch to Read)

### Implementation for User Story 3

- [x] T015 [US3] Implement `compress()` entry point in `crates/compression/src/lib.rs` — empty/whitespace check → threshold gate → `ToolType::from(tool_name)` → match dispatch to compressor modules (initially stubs pass through to `enforce_budget()` which hard-truncates; specialized logic replaces stubs in Phases 4–7) → wrap each specialized call in `std::panic::catch_unwind` with WARN-level `tracing::warn!` on fallback → call `truncate::enforce_budget()` → build `CompressedResult` with `CompressionStatistics` (ratio, chars_saved, percentage_saved)
- [x] T016 [US3] Verify US3 tests pass: `cargo test -p compression`

**Checkpoint**: The full `compress()` API works end-to-end. Any tool name dispatches correctly (specialized compressors will be stubs falling through to generic). All downstream stories can now plug in their compressor logic.

---

## Phase 4: User Story 1 — Compress Large File Reads for Memory Storage (Priority: P1)

**Goal**: Implement the file-read compressor that extracts language-specific constructs (imports, function signatures, class/struct names, error markers) from source files in JS/TS, Python, and Rust.

**Independent Test**: Pass representative source files in each supported language through `compress()` with tool_name="Read" and verify constructs are preserved within budget.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T017 [P] [US1] Write unit tests for language detection in `crates/compression/src/lang.rs` — test `detect_language()`: ".js"→JavaScript, ".ts"→JavaScript, ".tsx"→JavaScript, ".py"→Python, ".rs"→Rust, ".txt"→Unknown, None path→Unknown, content-based heuristics for ambiguous cases
- [x] T018 [P] [US1] Write unit tests for construct extraction in `crates/compression/src/lang.rs` — test `extract_constructs()` for JavaScript: ES6 imports, require(), export default, export { }, export function/class/const, module.exports, function declarations, arrow functions, async functions, class declarations, interface declarations, TODO/FIXME/HACK/XXX/BUG markers; for Python: import, from...import, def, async def, class, markers; for Rust: use, mod, fn, pub fn, async fn, struct, enum, trait, impl, markers; for Unknown: returns empty vec
- [x] T019 [US1] Write unit tests for read compressor in `crates/compression/src/read.rs` — test: 10K-char JS file compressed to ≤ budget with imports and function signatures preserved; file below threshold returned unchanged (via dispatcher); file with no recognizable constructs falls through to generic; input_context file path used for language detection; multiple languages produce correct extractions

### Implementation for User Story 1

- [x] T020 [P] [US1] Implement `detect_language()` in `crates/compression/src/lang.rs` — parse file extension from `input_context` using `rsplit_once('.')`, match extensions case-insensitively; fall back to content heuristics (e.g., `#!/usr/bin/env python`, `fn main()`, `import React`)
- [x] T021 [P] [US1] Implement `extract_constructs()` in `crates/compression/src/lang.rs` — use `LazyLock<Regex>` for all patterns; define pattern sets for JS/TS (imports, require, export default, export { }, export function/class/const, module.exports, function, class, interface, arrow fns, async), Python (import, from, def, async def, class), Rust (use, mod, fn, pub fn, struct, enum, trait, impl); shared error marker pattern (TODO, FIXME, HACK, XXX, BUG); return `Vec<String>` preserving source order; deduplicate
- [x] T022 [US1] Implement read compressor `compress()` in `crates/compression/src/read.rs` — detect language from input_context → extract constructs → if empty, return output for generic fallback → join constructs with newlines → prepend file path header if input_context provided → respect budget via `enforce_budget()`
- [x] T023 [US1] Wire read compressor into dispatcher match arm in `crates/compression/src/lib.rs` — `ToolType::Read => read::compress(config, output, input_context)` (already wired in Phase 1)
- [x] T024 [US1] Verify US1 tests pass: `cargo test -p compression`

**Checkpoint**: File-read compression works for JS/TS, Python, and Rust source files. The most complex compressor is complete.

---

## Phase 5: User Story 2 — Compress Bash Command Output (Priority: P1)

**Goal**: Implement the bash output compressor that preserves error lines, warning lines, and success indicators while discarding intermediate noise.

**Independent Test**: Pass representative bash outputs (build logs with errors, test results, npm install output) through `compress()` with tool_name="Bash" and verify errors and success indicators are preserved.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T025 [US2] Write unit tests for bash compressor in `crates/compression/src/bash.rs` — test: 20K-char build log with 3 error lines all preserved; success indicators ("Build successful", "All tests passed", "0 errors") preserved; warning lines preserved; bash output below threshold returned unchanged (via dispatcher); mixed output with errors, warnings, and noise correctly filtered; input_context command string included in header; empty output handled

### Implementation for User Story 2

- [x] T026 [US2] Implement bash compressor `compress()` in `crates/compression/src/bash.rs` — classify lines using `LazyLock<Regex>` patterns: error patterns (error:, Error:, ERR, FAILED, panic, fatal), warning patterns (warning:, warn:, WARN), success patterns (success, passed, ok, complete, done); collect errors first, then warnings, then success indicators; prepend command header from input_context; add summary line count; respect budget via `enforce_budget()`
- [x] T027 [US2] Wire bash compressor into dispatcher match arm in `crates/compression/src/lib.rs` — `ToolType::Bash => bash::compress(config, output, input_context)` (already wired in Phase 1)
- [x] T028 [US2] Verify US2 tests pass: `cargo test -p compression`

**Checkpoint**: Bash output compression preserves 100% of error lines (SC-004). Both P1 compressors (Read, Bash) are complete.

---

## Phase 6: User Story 4 — Compress Search Results (Priority: P2)

**Goal**: Implement grep and glob compressors that group results by file/directory and show counts.

**Independent Test**: Pass representative grep/glob outputs through `compress()` and verify grouping, counting, and truncation behavior.

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T029 [P] [US4] Write unit tests for grep compressor in `crates/compression/src/grep.rs` — test: 200 matches across 40 files grouped by file with match counts; top 10 individual matches shown; output within budget; input_context query included in header; grep output with no file paths (piped) falls through to generic; empty grep output
- [x] T030 [P] [US4] Write unit tests for glob compressor in `crates/compression/src/glob.rs` — test: 500 files grouped by directory with top 5 directories and file counts; sample filenames per group; JSON array format parsed correctly (C-3); line-delimited paths grouped correctly; neither-format falls through to generic; input_context query included in header

### Implementation for User Story 4

- [x] T031 [P] [US4] Implement grep compressor `compress()` in `crates/compression/src/grep.rs` — parse `file:line:content` format; group by file path; count matches per file; sort files by match count descending; show top 10 individual matches; prepend query header from input_context; add summary (total files, total matches); respect budget via `enforce_budget()`
- [x] T032 [P] [US4] Implement glob compressor `compress()` in `crates/compression/src/glob.rs` — try JSON array parse first (C-3), fall back to line-delimited paths; extract directory component from each path; group by directory; sort by file count descending; show top 5 directories with counts and sample filenames; prepend query header from input_context; add summary (total files, total dirs); respect budget via `enforce_budget()`
- [x] T033 [US4] Wire grep and glob compressors into dispatcher match arms in `crates/compression/src/lib.rs` — `ToolType::Grep => grep::compress(...)`, `ToolType::Glob => glob::compress(...)` (already wired in Phase 1)
- [x] T034 [US4] Verify US4 tests pass: `cargo test -p compression`

**Checkpoint**: Search result compression works with grouping and summarization. All P2 search compressors complete.

---

## Phase 7: User Story 5 — Compress Edit and Write Operations (Priority: P2)

**Goal**: Implement the edit/write compressor that extracts file path and change summary.

**Independent Test**: Pass representative edit/write tool outputs through `compress()` and verify file path and summary are preserved.

### Tests for User Story 5

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T035 [US5] Write unit tests for edit compressor in `crates/compression/src/edit.rs` — test: Edit (`is_write: false`) output with file path and large diff → result contains file path, "Changes applied" indicator, at most first 500 chars of original; Write (`is_write: true`) output for new file → result contains file path and "File created" indicator; input_context file path used as header; short edit output below threshold unchanged

### Implementation for User Story 5

- [x] T036 [US5] Implement edit/write compressor in `crates/compression/src/edit.rs` — expose `pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>, is_write: bool) -> String`; when `is_write` is true: file path + "File created" indicator + first 500 chars; when false: file path + "Changes applied" + first 500 chars of diff content; extract file path from input_context or first line of output; respect budget via `enforce_budget()`
- [x] T037 [US5] Wire edit compressor into dispatcher match arms in `crates/compression/src/lib.rs` — `ToolType::Edit => edit::compress(config, output, input_context, false)`, `ToolType::Write => edit::compress(config, output, input_context, true)` (already wired in Phase 1)
- [x] T038 [US5] Verify US5 tests pass: `cargo test -p compression`

**Checkpoint**: Edit/Write compression complete. All P2 compressors done.

---

## Phase 8: User Story 6 — Generic Fallback Compression (Priority: P3)

**Goal**: The generic fallback compressor was already implemented in Phase 2 (T012). This phase validates it meets all US6 acceptance criteria and handles edge cases.

**Independent Test**: Pass large text from unsupported tool types through `compress()` and verify head/tail preservation with omission indicators.

### Tests for User Story 6

- [x] T039 [US6] Write acceptance tests for generic fallback in `crates/compression/src/generic.rs` — test: 15K-char output from unsupported tool type → first 15 lines + last 10 lines + `[...N lines omitted...]` indicator; total line count stated; verify through full dispatcher with tool_name="CustomTool"; verify through full dispatcher with tool_name="WebFetch"

### Implementation for User Story 6

- [x] T040 [US6] Review and adjust generic compressor if needed to match AC-12 exactly (first 15 lines, last 10 lines, omission indicator with line count) in `crates/compression/src/generic.rs` — already meets spec
- [x] T041 [US6] Verify US6 tests pass: `cargo test -p compression`

**Checkpoint**: Generic fallback validated. All 6 user stories complete.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Integration tests, quality gates, performance validation, and cleanup

- [x] T042 [P] Write property-based test in `crates/compression/src/lib.rs` — for any arbitrary input string and any tool name, `compress()` output satisfies: `result.text.chars().count() <= config.target_budget` when `compression_applied` is `true` (SC-001 budget guarantee)
- [x] T043 [P] Write integration test for error recovery in `crates/compression/src/lib.rs` — simulate specialized compressor panic via a test-only path; verify fallback to generic compressor; verify WARN log emitted; verify no panic propagated to caller (M-13, AC-15)
- [x] T044 [P] Write integration test for custom `CompressionConfig` in `crates/compression/src/lib.rs` — test with threshold=5000, budget=3000; verify compression triggers at 5000 chars, budget respected at 3000 chars (AC-14)
- [x] T045 [P] Write edge case tests in `crates/compression/src/lib.rs` — EC-1: empty output; EC-2: whitespace-only; EC-3: no-construct file read → generic; EC-4: grep with no file paths; EC-5: glob neither line nor JSON; EC-6: case-insensitive tool names; EC-7: multi-byte Unicode char counting
- [x] T046 Run full quality gate: `cargo test -p compression && cargo clippy --workspace -- -D warnings && cargo fmt -p compression --check`
- [x] T047 Review all modules for guardrail compliance: no `unwrap()` in non-test code, no `unsafe`, no `.len()` for budget checks, no content logged at INFO+, all paths through `enforce_budget()`, each module ≤ 400 lines
- [x] T048 Run quickstart.md smoke test: verify the usage example from `specs/004-tool-output-compression/quickstart.md` compiles and produces expected output
- [x] T049 Add `criterion = { version = "0.5", features = ["html_reports"] }` to `[dev-dependencies]` in `crates/compression/Cargo.toml` and create `crates/compression/benches/compress_bench.rs` with `[[bench]]` entry in Cargo.toml
- [x] T050 Write criterion benchmark in `crates/compression/benches/compress_bench.rs` — benchmark `compress()` with 10K-char inputs for each tool type (Read/JS, Bash, Grep, Glob, Edit, Generic); assert median < 5ms per call (SC-006); run via `cargo bench -p compression`
- [x] T051 [P] Write SC-002 verification test in `crates/compression/tests/success_criteria.rs` — feed 20,000+ character inputs through each compressor (Read/JS, Bash, Grep, Glob, Edit, Generic) and assert `statistics.unwrap().ratio >= 10.0` (SC-002: ≥ 10× compression ratio on large inputs)
- [x] T052 [P] Write SC-003 quantitative construct preservation test in `crates/compression/tests/success_criteria.rs` — create a JS/TS source file with known constructs (N imports, M function signatures, K class names), compress with Read compressor, count how many constructs appear in the output, assert preservation rate ≥ 80% by count of unique constructs (SC-003)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US3 Dispatcher (Phase 3)**: Depends on Foundational — BLOCKS US1, US2, US4, US5 (they need the dispatcher to wire into)
- **US1 Read (Phase 4)**: Depends on US3 dispatcher
- **US2 Bash (Phase 5)**: Depends on US3 dispatcher; can run in parallel with US1
- **US4 Search (Phase 6)**: Depends on US3 dispatcher; can run in parallel with US1/US2
- **US5 Edit (Phase 7)**: Depends on US3 dispatcher; can run in parallel with US1/US2/US4
- **US6 Generic (Phase 8)**: Depends on Phase 2 (already implemented); validation only
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US3 (Dispatcher)**: Must be first — all other stories plug into it
- **US1 (Read)**: Independent of US2, US4, US5, US6
- **US2 (Bash)**: Independent of US1, US4, US5, US6
- **US4 (Search)**: Independent of US1, US2, US5, US6
- **US5 (Edit)**: Independent of US1, US2, US4, US6
- **US6 (Generic)**: Already implemented in Phase 2; Phase 8 is validation only

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Implementation tasks build incrementally
- Wire into dispatcher as final implementation step
- Verify all story tests pass before moving on

### Parallel Opportunities

- **Phase 2**: T005, T006, T007, T008 (all test stubs) can run in parallel; T009, T010 can run in parallel
- **Phases 4–7**: US1, US2, US4, US5 can all run in parallel after US3 is complete (different files, independent logic)
- **Within US4**: T029/T030 (tests) in parallel; T031/T032 (implementation) in parallel
- **Phase 9**: T042, T043, T044, T045 can all run in parallel; T049, T050 (benchmark) can run in parallel with other Phase 9 tasks

---

## Parallel Example: After US3 Dispatcher (Phase 3) is Complete

```text
# Launch all user story implementations in parallel:
Agent 1: US1 — Read compressor (Phase 4: T017–T024)
Agent 2: US2 — Bash compressor (Phase 5: T025–T028)
Agent 3: US4 — Grep + Glob compressors (Phase 6: T029–T034)
Agent 4: US5 — Edit/Write compressor (Phase 7: T035–T038)

# Each agent works independently in different files:
Agent 1: lang.rs, read.rs
Agent 2: bash.rs
Agent 3: grep.rs, glob.rs
Agent 4: edit.rs
```

---

## Implementation Strategy

### MVP First (US3 + Generic Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (types, config, truncate, generic)
3. Complete Phase 3: US3 Dispatcher
4. **STOP and VALIDATE**: The full `compress()` API works — all tool types route to generic fallback
5. This is a functional MVP that compresses any tool output

### Incremental Delivery

1. Setup + Foundational + US3 → Functional API with generic compression (MVP!)
2. Add US1 (Read) → Semantic file-read compression → Test independently
3. Add US2 (Bash) → Error-preserving bash compression → Test independently
4. Add US4 (Search) → Grouped search results → Test independently
5. Add US5 (Edit) → Lightweight edit records → Test independently
6. Validate US6 (Generic) → Confirm fallback meets acceptance criteria
7. Polish → Property tests, edge cases, quality gates

### Parallel Team Strategy

With multiple agents after Phase 3:
- Agent A: US1 (Read + Lang — most complex, start first)
- Agent B: US2 (Bash)
- Agent C: US4 (Grep + Glob)
- Agent D: US5 (Edit/Write — simplest, fastest to complete)

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- TDD workflow: write test → verify fail → implement → verify pass
- Commit after each phase or logical group
- Stop at any checkpoint to validate story independently
- All modules must stay ≤ 400 lines per project convention
- All regex patterns must use `LazyLock<Regex>` (no per-call compilation)
- Never use `unwrap()` in non-test code; never use `unsafe`; never use `.len()` for budget checks
