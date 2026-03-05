# Tasks: Testing & Quality + Migration & Backwards Compatibility

**Input**: Design documents from `/specs/010-testing-migration/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Tests ARE the primary deliverable for this feature. All test tasks are required.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **Rust workspace**: `crates/<crate-name>/src/`, `crates/<crate-name>/tests/`
- **Test fixtures**: `tests/fixtures/`
- **Fuzz harnesses**: `crates/<crate-name>/fuzz/`
- **CI**: `.github/workflows/ci.yml`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Fix pre-existing failures and create shared test infrastructure

- [x] T001 Diagnose and fix `test_find_json_output_valid` failure in `crates/cli/tests/find_test.rs`
- [x] T002 Diagnose and fix `test_find_type_filter_applies_before_final_limit` failure in `crates/cli/tests/find_test.rs`
- [x] T003 Create `tests/fixtures/` directory and add `README.md` documenting fixture naming conventions and the data model schema (Test Fixture, ExpectedSearchResult, ExpectedHit, TypeScript Baseline) from `data-model.md`
- [x] T004 [P] Install `cargo-fuzz` tooling and verify it works with the workspace (`cargo install cargo-fuzz`)

**Checkpoint**: `cargo test --workspace` passes with 0 failures. Fixture directory exists. cargo-fuzz available.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared test helpers and fixture loading utilities needed by multiple user stories

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 Create shared compatibility test helper module with `assert_compatible_search()` per Contract 1 in `crates/core/tests/compatibility/mod.rs` — loads `.mv2` fixture, executes search, compares results with ±0.01 score tolerance
- [x] T006 [P] Create fixture loading utilities: `load_expected_results()` and `load_ts_baselines()` JSON parsers in `crates/core/tests/compatibility/fixtures.rs` matching data-model.md schemas (ExpectedSearchResult, ExpectedHit, TypeScriptBaseline)
- [x] T007 [P] Generate and commit TypeScript `.mv2` test fixtures using the TypeScript agent-brain: `tests/fixtures/small_10obs.mv2` (~10 observations), `tests/fixtures/medium_100obs.mv2` (~100 observations), `tests/fixtures/edge_cases.mv2` (unicode, empty strings, long text)
- [x] T008 [P] Generate and commit `tests/fixtures/expected_results.json` with reference search results, timeline output, and stats from TypeScript agent-brain for each fixture file
- [x] T009 [P] Capture and commit TypeScript performance baselines in `tests/fixtures/ts_baselines.json` with metrics: query_latency_ms, compression_throughput_mb_s, startup_time_ms (measured on same hardware used for Rust benchmarks)

**Checkpoint**: Foundation ready — fixture files committed, helper modules compile, user story implementation can begin

---

## Phase 3: User Story 1 — Developer Validates Crate Quality via Test Suite (Priority: P1) 🎯 MVP

**Goal**: Every public API function, trait method, and type constructor across all 7 crates has at least one unit test. Cross-crate workflows are validated end-to-end. `cargo test` passes green.

**Independent Test**: Run `cargo test --workspace` and verify all public APIs have corresponding test cases. Cross-crate remember → search → getContext cycle passes.

**Covers**: FR-001, FR-002, FR-003, SC-001, SC-002

### hooks crate unit tests (currently 0 unit tests, 18 pub fns)

- [x] T010 [US1] Add unit tests for `should_process()` in `crates/hooks/src/bootstrap.rs` — test with valid/invalid platform configs, missing env vars
- [x] T011 [US1] Add unit tests for `resolve_memory_path()` in `crates/hooks/src/bootstrap.rs` — test default path, custom path via env var, missing directory
- [x] T012 [US1] Add unit tests for `open_mind()` and `open_mind_with_path()` in `crates/hooks/src/bootstrap.rs` — test successful open, missing file, corrupted file
- [x] T013 [P] [US1] Add unit tests for `format_system_message()` in `crates/hooks/src/context.rs` — test message formatting with various observation counts and platform types
- [x] T014 [P] [US1] Add unit tests for `handle_session_start()` in `crates/hooks/src/session_start.rs` — test successful start, dedup check, error paths
- [x] T015 [P] [US1] Add unit tests for `handle_post_tool_use()` in `crates/hooks/src/post_tool_use.rs` — test with various tool types, compression integration
- [x] T016 [P] [US1] Add unit tests for `handle_stop()` in `crates/hooks/src/stop.rs` — test graceful shutdown, observation persistence
- [x] T017 [P] [US1] Add unit tests for dedup functions (`check_duplicate()`, `record_hash()`, `load_cache()`) in `crates/hooks/src/dedup.rs`
- [x] T018 [P] [US1] Add unit tests for `handle_smart_install()` in `crates/hooks/src/smart_install.rs` — test install detection, version checking
- [x] T019 [P] [US1] Add unit tests for IO functions (`read_input()`, `write_output()`) in `crates/hooks/src/io.rs` — test JSON serialization/deserialization
- [x] T020 [P] [US1] Add unit tests for `read_manifest()` in `crates/hooks/src/manifest.rs` — test valid manifest, missing file, malformed JSON
- [x] T021 [P] [US1] Add unit tests for `detect_git_context()` in `crates/hooks/src/git.rs` — test inside/outside git repo, branch detection

### opencode crate unit tests (currently 0 unit tests, 22 pub fns)

- [x] T022 [P] [US1] Add unit tests for `handle_with_failopen()` in `crates/opencode/src/lib.rs` — test success path, error recovery, fail-open behavior
- [x] T023 [P] [US1] Add unit tests for `mind_tool_with_failopen()` in `crates/opencode/src/mind_tool.rs` — test search, remember, stats operations with failopen
- [x] T024 [P] [US1] Add unit tests for sidecar functions (`load()`, `save()`, `sidecar_path()`, `ensure_dir()`) in `crates/opencode/src/sidecar.rs`
- [x] T025 [P] [US1] Add unit tests for `handle_chat_hook()` in `crates/opencode/src/chat_hook.rs` — test observation extraction from chat messages
- [x] T026 [P] [US1] Add unit tests for `handle_tool_hook()` in `crates/opencode/src/tool_hook.rs` — test tool output processing, compression integration
- [x] T027 [P] [US1] Add unit tests for bootstrap functions in `crates/opencode/src/bootstrap.rs` — test initialization, config resolution, platform detection
- [x] T028 [P] [US1] Add unit tests for `cleanup_sessions()` in `crates/opencode/src/session_cleanup.rs` — test stale session cleanup, active session preservation
- [x] T029 [P] [US1] Add unit tests for type constructors and serialization in `crates/opencode/src/types.rs`

### cli crate — expand unit test coverage (currently 4 unit tests, 12 pub fns)

- [x] T030 [P] [US1] Add unit tests for CLI argument parsing and command dispatch functions in `crates/cli/src/args.rs` and `crates/cli/src/commands.rs` — test subcommand routing, flag validation, output format selection
- [x] T031 [P] [US1] Add unit tests for output formatting functions in `crates/cli/src/output.rs` — test JSON and table output rendering with various input data

### compression crate — enhance compress() coverage

- [x] T032 [P] [US1] Add unit tests for `compress()` entry point in `crates/compression/src/lib.rs` — test all tool types (Read, Write, Bash, Glob, Grep, LS), various input sizes, config combinations beyond existing panic recovery tests

### platforms crate — expand integration tests

- [x] T033 [P] [US1] Add integration test for EventPipeline composite API in `crates/platforms/tests/pipeline_integration_test.rs` — test full event normalization flow for Claude platform
- [x] T034 [P] [US1] Add integration test for cross-platform round-trips in `crates/platforms/tests/cross_platform_test.rs` — test Claude → normalize → verify and OpenCode → normalize → verify

### cross-crate integration

- [x] T035 [US1] Add or verify integration test for full remember → search → getContext cycle in `crates/core/tests/integration/mind_roundtrip.rs` — ensure the cross-crate workflow per FR-002 is exercised end-to-end
- [x] T036 [US1] Run `cargo test --workspace` and verify all public APIs across 7 crates have ≥1 test; document final test count (target: ≥400 total tests, SC-001 parity ≥48)

**Checkpoint**: US1 complete — `cargo test --workspace` green, every pub fn has ≥1 test, cross-crate workflow validated

---

## Phase 4: User Story 2 — Drop-in Replacement for TypeScript Version (Priority: P1)

**Goal**: Rusty-brain reads TypeScript `.mv2` files and produces identical search results, timeline, and stats. All env vars honored. Directory layout matches.

**Independent Test**: Point rusty-brain at TypeScript-generated `.mv2` fixture, run search/timeline/stats, compare against expected_results.json.

**Covers**: FR-006, FR-009, FR-010, FR-011, FR-013, FR-014, SC-005, SC-008

### Compatibility tests

- [ ] T037 [P] [US2] Create search compatibility test in `crates/core/tests/compatibility/search_compat_test.rs` — load each fixture (small, medium, edge_cases), run queries from expected_results.json, assert same result set and ordering with ±0.01 score tolerance per FR-013
- [ ] T038 [P] [US2] Create timeline compatibility test in `crates/core/tests/compatibility/timeline_compat_test.rs` — load fixtures, verify timeline output matches TypeScript reference
- [ ] T039 [P] [US2] Create stats compatibility test in `crates/core/tests/compatibility/stats_compat_test.rs` — load fixtures, verify stats (observation count, memory size, etc.) match TypeScript reference
- [ ] T040 [P] [US2] Create `.mv2` format read/write round-trip test in `crates/core/tests/compatibility/format_compat_test.rs` — write observations with Rust, read with Rust, verify identical to TypeScript-written data per FR-009

### Environment variable tests

- [ ] T041 [P] [US2] Create env var compatibility tests in `crates/hooks/tests/env_compat_test.rs` — test all 6 env vars (MEMVID_PLATFORM, MEMVID_MIND_DEBUG, MEMVID_PLATFORM_MEMORY_PATH, MEMVID_PLATFORM_PATH_OPT_IN, CLAUDE_PROJECT_DIR, OPENCODE_PROJECT_DIR) with valid, invalid, and unset values per Contract 6
- [ ] T042 [P] [US2] Create env var compatibility tests for opencode in `crates/opencode/tests/env_compat_test.rs` — test OPENCODE_PROJECT_DIR and MEMVID_PLATFORM with opencode-specific behavior

### Directory layout test

- [ ] T043 [US2] Create directory layout assertion test in `crates/hooks/tests/layout_compat_test.rs` — verify `.agent-brain/` structure matches TypeScript: `mind.mv2`, `.dedup-cache.json`, `.install-version` per FR-011

### Edge case tests

- [ ] T044 [P] [US2] Create unknown fields resilience test in `crates/core/tests/compatibility/unknown_fields_test.rs` — test reading `.mv2` with extra unknown fields (simulating newer TS version), verify known fields read correctly and unknowns ignored gracefully
- [ ] T045 [P] [US2] Create invalid env var test in `crates/hooks/tests/invalid_env_test.rs` — test `MEMVID_PLATFORM=nonexistent` returns clear structured error
- [ ] T046 [P] [US2] Create permissions error test in `crates/hooks/tests/permissions_test.rs` — test `.agent-brain/` with restricted permissions returns meaningful error

### Plugin.json update

- [ ] T047 [US2] Update `plugin.json` hook paths from `dist/hooks/*.js` to Rust binary paths per FR-014; verify structure in a test

**Checkpoint**: US2 complete — TypeScript `.mv2` fixtures produce identical results, all env vars honored, directory layout matches, plugin.json updated

---

## Phase 5: User Story 3 — Concurrent Access Safety (Priority: P2)

**Goal**: Parallel writers, lock contention, and stale lock recovery work without data corruption or silent failures. 5-second recovery timeout.

**Independent Test**: Spawn multiple threads writing to shared `.mv2` file, verify all writes succeed and data is intact.

**Covers**: FR-004, SC-003

- [ ] T048 [US3] Add 4-writer concurrent test in `crates/core/tests/integration/mind_concurrent.rs` — spawn 4 threads with Arc<Barrier>, each writes unique observation, verify all 4 retrievable after completion
- [ ] T049 [US3] Add 8-writer concurrent test in `crates/core/tests/integration/mind_concurrent.rs` — same pattern with 8 writers, verify no data loss
- [ ] T050 [US3] Add 16-writer concurrent test in `crates/core/tests/integration/mind_concurrent.rs` — stress test with 16 writers, verify no corruption
- [ ] T051 [US3] Add stale lock recovery test in `crates/core/tests/integration/mind_concurrent.rs` — create stale lock file (simulating crashed process), start new Mind instance, verify recovery completes within 5 seconds per clarification
- [ ] T052 [US3] Add reader-during-write test in `crates/core/tests/integration/mind_concurrent.rs` — one writer holds lock, concurrent reader must either wait or get clean error, never read partial data
- [ ] T053 [US3] Add data integrity verification in `crates/core/tests/integration/mind_concurrent.rs` — after all concurrent writes complete, open Mind and verify SHA-256 checksums of all stored observations match originals

**Checkpoint**: US3 complete — 4, 8, 16 writer tests pass 100%, stale lock recovery ≤5s, no partial reads

---

## Phase 6: User Story 4 — Performance Meets or Exceeds TypeScript Baseline (Priority: P2)

**Goal**: Benchmarks demonstrate ≥2× performance advantage on query latency, compression throughput, and startup time. CI gates on regression.

**Independent Test**: Run `cargo bench`, compare against ts_baselines.json, verify all metrics ≥2× faster.

**Covers**: FR-005, FR-008, SC-004

- [ ] T054 [P] [US4] Enhance query latency benchmark in `crates/core/benches/search.rs` — add baseline comparison logic that loads ts_baselines.json and asserts Rust latency ≤ TypeScript / 2.0
- [ ] T055 [P] [US4] Enhance compression throughput benchmark in `crates/compression/benches/compress_bench.rs` — add baseline comparison logic that asserts Rust throughput ≥ TypeScript × 2.0
- [ ] T056 [P] [US4] Create startup time benchmark in `crates/cli/benches/startup_bench.rs` — measure cold start to first command ready, compare against ts_baselines.json startup_time_ms
- [ ] T057 [P] [US4] Enhance store benchmark in `crates/core/benches/store.rs` — add write throughput baseline comparison
- [ ] T058 [P] [US4] Enhance context assembly benchmark in `crates/core/benches/context.rs` — add context build time baseline comparison
- [ ] T059 [US4] Create benchmark summary report script or test that loads all criterion results and ts_baselines.json, outputs pass/fail for each metric with actual speedup factors, in `crates/cli/tests/bench_regression_test.rs`

**Checkpoint**: US4 complete — all benchmarks show ≥2× speedup, regression gate logic in place

---

## Phase 7: User Story 5 — Fuzz Testing Catches Malformed Inputs (Priority: P3)

**Goal**: Fuzz harnesses exercise compression, hook JSON, and search queries with random/malformed inputs. No panics or crashes in 60-second runs.

**Independent Test**: Run each fuzz harness for 60 seconds, verify no panics.

**Covers**: FR-007, SC-006

- [ ] T060 [P] [US5] Initialize cargo-fuzz for compression crate and create compression fuzz harness in `crates/compression/fuzz/fuzz_targets/compression_fuzz.rs` — feeds arbitrary bytes to `compress()`, verifies Ok or structured Err, never panic per Contract 3
- [ ] T061 [P] [US5] Create seed corpus for compression fuzzer in `crates/compression/fuzz/corpus/compression_fuzz/` — include valid tool output samples (Read, Bash, Glob results), empty input, very large input, binary data
- [ ] T062 [P] [US5] Initialize cargo-fuzz for hooks crate and create hook JSON fuzz harness in `crates/hooks/fuzz/fuzz_targets/hook_json_fuzz.rs` — feeds arbitrary bytes to hook JSON parser, verifies no panic
- [ ] T063 [P] [US5] Create seed corpus for hook JSON fuzzer in `crates/hooks/fuzz/corpus/hook_json_fuzz/` — include valid hook JSON payloads (session_start, post_tool_use, stop), malformed JSON, truncated input
- [ ] T064 [P] [US5] Initialize cargo-fuzz for core crate and create search query fuzz harness in `crates/core/fuzz/fuzz_targets/search_query_fuzz.rs` — feeds arbitrary bytes as search query strings, verifies empty results or structured error, never crash
- [ ] T065 [P] [US5] Create seed corpus for search query fuzzer in `crates/core/fuzz/corpus/search_query_fuzz/` — include valid queries, unicode, SQL injection patterns, very long strings, null bytes
- [ ] T066 [US5] Run all 3 fuzz harnesses for 60 seconds each and verify 0 panics; document any crash artifacts in `crates/<name>/fuzz/artifacts/` and add as regression tests

**Checkpoint**: US5 complete — 3 harnesses run 60s each with no panics, seed corpora committed

---

## Phase 8: User Story 6 — Legacy Path Detection and Migration Guidance (Priority: P3)

**Goal**: Detect `.claude/mind.mv2` and suggest migration to `.agent-brain/mind.mv2`. Non-blocking structured diagnostic.

**Independent Test**: Create `.claude/mind.mv2`, run rusty-brain, verify diagnostic warning appears.

**Covers**: FR-012, SC-009

- [ ] T067 [US6] Add unit test for legacy-only scenario in `crates/hooks/src/bootstrap.rs` — `.claude/mind.mv2` exists, `.agent-brain/mind.mv2` does not → returns Diagnostic::Warning with migration suggestion (test-first: write failing test)
- [ ] T068 [US6] Add unit test for both-exist scenario in `crates/hooks/src/bootstrap.rs` — both paths exist → uses `.agent-brain/mind.mv2`, returns Diagnostic::Warning about duplicate (test-first)
- [ ] T069 [US6] Add unit test for normal scenario in `crates/hooks/src/bootstrap.rs` — only `.agent-brain/mind.mv2` exists → returns None (no diagnostic) (test-first)
- [ ] T070 [US6] Implement `detect_legacy_path()` function in `crates/hooks/src/bootstrap.rs` — check for `.claude/mind.mv2` relative to project root, return `Option<Diagnostic>` per Contract 4 (make T067-T069 pass)
- [ ] T071 [US6] Wire `detect_legacy_path()` into hook startup paths (session_start, post_tool_use) so diagnostic is emitted at startup in `crates/hooks/src/session_start.rs` and `crates/hooks/src/post_tool_use.rs`
- [ ] T072 [US6] Add integration test for legacy detection in `crates/hooks/tests/legacy_path_test.rs` — create temp directory with `.claude/mind.mv2`, run hook entry point, verify structured diagnostic in output

**Checkpoint**: US6 complete — legacy path detected, structured diagnostic emitted, all 3 scenarios tested

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: CI enhancement, final validation, cross-cutting improvements

- [ ] T073 Update `.github/workflows/ci.yml` to add benchmark regression gate step — run `cargo bench` and fail if any metric is <2× vs ts_baselines.json (use custom comparison script parsing criterion JSON output)
- [ ] T074 [P] Update `.github/workflows/ci.yml` to add fuzz smoke test step — install cargo-fuzz, run each harness for 60 seconds, fail on any panic
- [ ] T075 [P] Add variance/deviation metrics to benchmark output — flag unreliable runs when system is under heavy load (edge case from spec)
- [ ] T076 Run agent integration smoke test — verify CLI commands (`rusty-brain search`, `rusty-brain stats`, `rusty-brain timeline`) produce valid structured JSON output per constitution Quality Gates
- [ ] T077 Run full quality gate validation: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace && cargo bench --workspace`
- [ ] T078 Run quickstart.md validation — execute all commands from quickstart.md and verify they work as documented
- [ ] T079 Final test count audit — verify total test count ≥400, every pub fn has ≥1 test, all SC-001 through SC-009 success criteria met

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on T001-T002 (green baseline) — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 completion — no dependencies on other stories
- **US2 (Phase 4)**: Depends on Phase 2 completion (fixtures) — no dependencies on other stories
- **US3 (Phase 5)**: Depends on Phase 2 completion — no dependencies on other stories
- **US4 (Phase 6)**: Depends on Phase 2 completion (baselines) — no dependencies on other stories
- **US5 (Phase 7)**: Depends on Phase 1 (cargo-fuzz installed) — no dependencies on other stories
- **US6 (Phase 8)**: Depends on Phase 2 completion — no dependencies on other stories
- **Polish (Phase 9)**: Depends on US4 (benchmarks) and US5 (fuzz) for CI gate steps

### User Story Dependencies

- **US1 (P1)**: Independent — can start after Phase 2
- **US2 (P1)**: Independent — can start after Phase 2 (needs fixtures from T007-T008)
- **US3 (P2)**: Independent — can start after Phase 2
- **US4 (P2)**: Independent — can start after Phase 2 (needs baselines from T009)
- **US5 (P3)**: Independent — can start after Phase 1 T004 (cargo-fuzz)
- **US6 (P3)**: Independent — can start after Phase 2

### Within Each User Story

- Tests written first (test-first per constitution Principle V)
- Implementation code where needed (US6 only)
- Integration verification last

### Parallel Opportunities

- **Phase 1**: T001 and T002 sequential (same file); T003 and T004 parallel
- **Phase 2**: T005-T006 parallel (different files); T007-T009 parallel (independent fixture generation)
- **Phase 3**: T010-T012 sequential (same file: bootstrap.rs); T013-T034 parallelizable (different files/crates)
- **Phase 4**: T037-T046 parallelizable (different test files); T047 independent
- **Phase 5**: T048-T053 sequential (same file: mind_concurrent.rs)
- **Phase 6**: T054-T058 parallelizable (different bench files); T059 depends on benchmarks
- **Phase 7**: T060-T065 parallelizable (different crates); T066 depends on harnesses
- **Phase 8**: T067-T069 sequential (same file: bootstrap.rs); T070 depends on T067-T069; T071-T072 after T070
- **Phase 9**: T073-T075 parallelizable; T076-T079 sequential (validation)

---

## Parallel Example: User Story 1

```bash
# Launch hooks unit tests — bootstrap.rs tasks sequentially, others in parallel:
# Sequential: T010, T011, T012 (same file: bootstrap.rs)
# Parallel with above: T013-T021 (different files)

# Launch all opencode unit tests in parallel (8 tasks, different files):
Task: "Add unit tests for handle_with_failopen() in crates/opencode/src/lib.rs"
Task: "Add unit tests for mind_tool_with_failopen() in crates/opencode/src/mind_tool.rs"
Task: "Add unit tests for sidecar functions in crates/opencode/src/sidecar.rs"
# ... etc

# Launch CLI + compression + platforms in parallel with hooks/opencode:
Task: "Add CLI unit tests in crates/cli/src/args.rs"
Task: "Add compress() tests in crates/compression/src/lib.rs"
Task: "Add pipeline tests in crates/platforms/tests/"
```

## Parallel Example: User Story 2

```bash
# Launch all compatibility tests in parallel (4 tasks, different test files):
Task: "Create search compatibility test in crates/core/tests/compatibility/search_compat_test.rs"
Task: "Create timeline compatibility test in crates/core/tests/compatibility/timeline_compat_test.rs"
Task: "Create stats compatibility test in crates/core/tests/compatibility/stats_compat_test.rs"
Task: "Create .mv2 format round-trip test in crates/core/tests/compatibility/format_compat_test.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (fix failing tests, install cargo-fuzz)
2. Complete Phase 2: Foundational (fixture infrastructure)
3. Complete Phase 3: User Story 1 (comprehensive unit test coverage)
4. **STOP and VALIDATE**: `cargo test --workspace` green, every pub fn covered
5. This alone delivers SC-001, SC-002 — the foundation for all other stories

### Incremental Delivery

1. Setup + Foundational → Green baseline, fixtures ready
2. Add US1 → Test coverage across all crates (MVP!)
3. Add US2 → TypeScript compatibility verified
4. Add US3 → Concurrency safety proven
5. Add US4 → Performance benchmarks with baselines
6. Add US5 → Fuzz testing for robustness
7. Add US6 → Legacy path detection
8. Polish → CI gates, final validation

### Parallel Team Strategy

With multiple developers after Phase 2 completes:

- **Developer A**: US1 (hooks unit tests) + US1 (opencode unit tests)
- **Developer B**: US2 (compatibility tests) + US6 (legacy detection)
- **Developer C**: US3 (concurrency tests) + US4 (benchmarks)
- **Developer D**: US5 (fuzz testing)

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- This feature's primary deliverable IS tests — every task produces test code
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- Avoid: vague tasks, same file conflicts, cross-story dependencies
