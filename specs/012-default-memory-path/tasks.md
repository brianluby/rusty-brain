# Tasks: Default Memory Path Change

**Input**: Design documents from `/specs/012-default-memory-path/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Included — constitution mandates test-first development (Principle V).

**Organization**: Tasks grouped by user story. US1/US2/US4 are P1, US3 is P2.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: No new project setup needed — existing workspace. Verify build baseline.

- [X] T001 Verify clean build baseline: run `cargo test && cargo clippy -- -D warnings && cargo fmt --check` and record any pre-existing failures

**Checkpoint**: Build is green, ready to proceed.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Update core constants and default values that ALL user stories depend on.

**CRITICAL**: No user story work can begin until this phase is complete.

- [X] T002 Rename `DEFAULT_LEGACY_PATH` to `DEFAULT_MEMORY_PATH` and change value from `".agent-brain/mind.mv2"` to `".rusty-brain/mind.mv2"` in `crates/platforms/src/path_policy.rs`
- [X] T003 Add `LEGACY_AGENT_BRAIN_PATH` constant with value `".agent-brain/mind.mv2"` in `crates/platforms/src/path_policy.rs`
- [X] T004 Add `DEFAULT_MEMORY_DIR` constant with value `".rusty-brain"` in `crates/platforms/src/path_policy.rs`
- [X] T005 Update `MindConfig::default()` to use `PathBuf::from(".rusty-brain/mind.mv2")` and update the doc comment in `crates/types/src/config.rs`
- [X] T006 Update unit test `default_memory_path_is_agent_brain_mind_mv2` (rename to `default_memory_path_is_rusty_brain_mind_mv2` and fix assertion) in `crates/types/tests/config_test.rs`
- [X] T007 Update all other tests in `crates/types/tests/config_test.rs` that assert `.agent-brain/mind.mv2` as the default (lines 200-201, 223-225, 418, 424+)
- [X] T008 Update `legacy_mode_no_opt_in` test to assert `.rusty-brain/mind.mv2` in `crates/platforms/src/path_policy.rs`
- [X] T009 Update `legacy_mode_ignores_platform_name` test to assert `.rusty-brain/mind.mv2` in `crates/platforms/src/path_policy.rs`
- [X] T010 Update `format_legacy_path_warning_contains_both_paths` test to assert `.rusty-brain/mind.mv2` in `crates/platforms/src/path_policy.rs`
- [X] T011 Verify build passes: `cargo test && cargo clippy -- -D warnings`

**Checkpoint**: Foundation ready — all default paths point to `.rusty-brain/`, existing tests updated. User story implementation can begin.

---

## Phase 3: User Story 1 — New Installation Uses .rusty-brain Directory (Priority: P1) MVP

**Goal**: New installations create memory files at `.rusty-brain/mind.mv2`. The `resolve_memory_path()` function returns the new canonical path. Custom env var paths remain unaffected.

**Independent Test**: Initialize rusty-brain in a fresh temp directory and verify the resolved path is `.rusty-brain/mind.mv2`.

### Implementation for User Story 1

- [X] T012 [P] [US1] Update `from_env_ignores_platform_detection_for_memory_path` test assertion to `.rusty-brain/mind.mv2` in `crates/types/tests/config_test.rs`
- [X] T013 [P] [US1] Update `from_env_ignores_claude_project_dir_for_memory_path` test assertion to `.rusty-brain/mind.mv2` in `crates/types/tests/config_test.rs`
- [X] T014 [US1] Run `cargo test -p platforms -p types` to verify US1 tests pass

**Checkpoint**: New installations resolve to `.rusty-brain/mind.mv2`. Custom paths unaffected. US1 independently testable.

---

## Phase 4: User Story 2 — Migration from .agent-brain Directory (Priority: P1)

**Goal**: Existing `.agent-brain/` installations are detected, used as fallback, and migration guidance is provided. All callers of the renamed detection function are updated.

**Independent Test**: Create temp dir with `.agent-brain/mind.mv2`, run detection, verify fallback path is used and migration diagnostic is produced with actionable `mv` commands.

### Tests for User Story 2

- [X] T015 [US2] Write test `detect_legacy_paths_agent_brain_only_suggests_migration` — `.agent-brain/` exists, no `.rusty-brain/` → returns Info diagnostic with actionable `mkdir -p .rusty-brain && mv .agent-brain/mind.mv2 .rusty-brain/mind.mv2` command in `crates/platforms/src/bootstrap.rs`
- [X] T016 [US2] Write test `detect_legacy_paths_both_dirs_warns_duplicate` — both `.agent-brain/` and `.rusty-brain/` exist → returns Warning about duplicate in `crates/platforms/src/bootstrap.rs`
- [X] T017 [US2] Write test `detect_legacy_paths_rusty_brain_only_returns_empty` — only `.rusty-brain/` exists → returns empty Vec in `crates/platforms/src/bootstrap.rs`
- [X] T018 [US2] Write test `resolve_effective_path_falls_back_to_agent_brain` — `.agent-brain/mind.mv2` exists, `.rusty-brain/` doesn't → returns `.agent-brain/mind.mv2` in `crates/platforms/src/bootstrap.rs`
- [X] T019 [US2] Write test `resolve_effective_path_prefers_rusty_brain` — both exist → returns `.rusty-brain/mind.mv2` in `crates/platforms/src/bootstrap.rs`
- [X] T020 [US2] Write test `resolve_effective_path_new_install_returns_rusty_brain` — neither exists → returns `.rusty-brain/mind.mv2` in `crates/platforms/src/bootstrap.rs`

### Implementation for User Story 2

- [X] T021 [US2] Refactor `detect_legacy_path` → `detect_legacy_paths` returning `Vec<Diagnostic>` with `.agent-brain/` detection (Info level with actionable `mv` command when only legacy exists, Warning when duplicate) and update canonical path references from `.agent-brain/mind.mv2` to `.rusty-brain/mind.mv2` in `crates/platforms/src/bootstrap.rs`
- [X] T022 [US2] Implement `resolve_effective_path(project_root: &Path) -> PathBuf` — checks if `.rusty-brain/mind.mv2` exists on disk (returns it); else checks if `.agent-brain/mind.mv2` exists (returns it as fallback); else returns `.rusty-brain/mind.mv2` as the new-install default. This function replaces `resolve_memory_path` for the non-opt-in case in `crates/platforms/src/bootstrap.rs`
- [X] T023 [US2] Wire `resolve_effective_path` into `build_mind_config` — when no explicit `MEMVID_PLATFORM_MEMORY_PATH` and not platform opt-in, call `resolve_effective_path(project_dir)` to get the memory path (replacing the current `resolve_memory_path` call for the default case) in `crates/platforms/src/bootstrap.rs`
- [X] T024 [US2] Update `format_legacy_path_warning` to include actionable `mv` commands targeting `.rusty-brain/` (FR-009) in `crates/platforms/src/path_policy.rs`
- [X] T025 [US2] Update `crates/hooks/src/bootstrap.rs` re-export — change import from `detect_legacy_path` to `detect_legacy_paths` in `crates/hooks/src/bootstrap.rs`
- [X] T026 [US2] Update `handle_session_start` in `crates/hooks/src/session_start.rs` — change `detect_legacy_path()` call to `detect_legacy_paths()` and handle `Vec<Diagnostic>` return type (iterate diagnostics instead of Option)
- [X] T027 [US2] Update `handle_post_tool_use` in `crates/hooks/src/post_tool_use.rs` — change `detect_legacy_path()` call to `detect_legacy_paths()` and handle `Vec<Diagnostic>` return type
- [X] T028 [P] [US2] Update integration tests in `crates/hooks/tests/legacy_path_test.rs` — change canonical path assertions from `.agent-brain/` to `.rusty-brain/`, update function name from `detect_legacy_path` to `detect_legacy_paths`
- [X] T029 [P] [US2] Update `crates/hooks/tests/env_compat_test.rs` — change `.agent-brain/mind.mv2` assertions to `.rusty-brain/mind.mv2` (lines 166, 238, 287)
- [X] T030 [P] [US2] Update `crates/hooks/tests/layout_compat_test.rs` — change `.agent-brain` references to `.rusty-brain`
- [X] T031 [P] [US2] Update `crates/opencode/tests/env_compat_test.rs` — change `.agent-brain/mind.mv2` assertions to `.rusty-brain/mind.mv2` (lines 166, 238, 287)
- [X] T032 [P] [US2] Update `crates/opencode/tests/chat_hook_test.rs` — change `.agent-brain` path setup to `.rusty-brain` (line 125)
- [X] T033 [US2] Run `cargo test -p platforms -p hooks -p opencode` to verify US2 tests pass

**Checkpoint**: Legacy `.agent-brain/` installations fall back gracefully, migration guidance with actionable commands is shown, all callers updated. US2 independently testable.

---

## Phase 5: User Story 4 — Supporting Files in .rusty-brain Directory (Priority: P1)

**Goal**: Dedup cache and install version marker live in `.rusty-brain/` directory.

**Independent Test**: Create a fresh temp dir, run dedup and smart-install operations, verify files are created under `.rusty-brain/`.

### Tests for User Story 4

- [X] T034 [US4] Write test `new_sets_cache_path_under_rusty_brain_dir` verifying `DedupCache::new()` uses `.rusty-brain/.dedup-cache.json` in `crates/hooks/src/dedup.rs`
- [X] T035 [US4] Write test `smart_install_writes_version_to_rusty_brain_dir` verifying `.install-version` is created at `.rusty-brain/.install-version` in `crates/hooks/src/smart_install.rs`

### Implementation for User Story 4

- [X] T036 [US4] Update `DedupCache::new()` to use `project_dir.join(".rusty-brain").join(CACHE_FILENAME)` in `crates/hooks/src/dedup.rs`
- [X] T037 [US4] Update `new_sets_cache_path_under_agent_brain_dir` test (rename to `new_sets_cache_path_under_rusty_brain_dir` and fix assertion to `.rusty-brain/.dedup-cache.json`) in `crates/hooks/src/dedup.rs`
- [X] T038 [US4] Update `handle_smart_install()` to write version file at `cwd.join(".rusty-brain").join(VERSION_FILENAME)` instead of `cwd.join(VERSION_FILENAME)` in `crates/hooks/src/smart_install.rs`
- [X] T039 [US4] Update all `smart_install.rs` tests to expect `.rusty-brain/.install-version` path in `crates/hooks/src/smart_install.rs`
- [X] T040 [P] [US4] Update `crates/hooks/tests/dedup_test.rs` — change all 7 `.agent-brain` directory references to `.rusty-brain`
- [X] T041 [P] [US4] Update `crates/hooks/tests/permissions_test.rs` — change 6 `.agent-brain` directory references to `.rusty-brain` (lines 77-78, 80, 94, 111-112)
- [X] T042 [US4] Run `cargo test -p hooks` to verify US4 tests pass

**Checkpoint**: All supporting files created under `.rusty-brain/`. US4 independently testable.

---

## Phase 6: User Story 3 — Legacy .claude Path Detection (Priority: P2)

**Goal**: `.claude/mind.mv2` detection now suggests migration to `.rusty-brain/mind.mv2` (not `.agent-brain/`). Three-tier detection chain complete. Diagnostic messages include actionable `mv` commands (FR-009).

**Independent Test**: Create temp dir with `.claude/mind.mv2`, run detection, verify migration suggestion points to `.rusty-brain/mind.mv2` with exact move command.

### Tests for User Story 3

- [X] T043 [US3] Write test `detect_legacy_paths_claude_only_suggests_rusty_brain` — `.claude/mind.mv2` exists alone → diagnostic suggests migration to `.rusty-brain/mind.mv2` with actionable `mv` command in `crates/platforms/src/bootstrap.rs`
- [X] T044 [US3] Write test `detect_legacy_paths_all_three_dirs` — all three exist → uses `.rusty-brain/`, warns about both `.agent-brain/` and `.claude/` in `crates/platforms/src/bootstrap.rs`
- [X] T045 [US3] Write test `detect_legacy_paths_claude_and_agent_brain_no_rusty` — `.claude/` and `.agent-brain/` exist but no `.rusty-brain/` → suggests migration to `.rusty-brain/`, warns about `.claude/` in `crates/platforms/src/bootstrap.rs`

### Implementation for User Story 3

- [X] T046 [US3] Extend `detect_legacy_paths()` to check `.claude/mind.mv2` and produce diagnostic targeting `.rusty-brain/mind.mv2` (not `.agent-brain/`) with actionable move commands in `crates/platforms/src/bootstrap.rs`
- [X] T047 [US3] Update `session_start_includes_legacy_diagnostic_in_system_message` integration test in `crates/hooks/tests/legacy_path_test.rs` to verify `.rusty-brain` appears in migration message
- [X] T048 [US3] Run `cargo test -p platforms -p hooks` to verify US3 tests pass

**Checkpoint**: Three-tier legacy detection complete. All legacy paths point to `.rusty-brain/` as migration target with actionable commands.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Documentation updates and final validation across all stories.

- [X] T049 [P] Update `CLAUDE.md` — replace all `.agent-brain/` references with `.rusty-brain/` in project documentation sections
- [X] T050 [P] Update `crates/hooks/src/context.rs` test `format_system_message_contains_memory_path` to use `.rusty-brain` path (line 108)
- [X] T051 [P] Update `skills/mind/SKILL.md` — change `.agent-brain/mind.mv2` reference to `.rusty-brain/mind.mv2` (line 19)
- [X] T052 [P] Update `skills/memory/SKILL.md` — change `.agent-brain/mind.mv2` reference to `.rusty-brain/mind.mv2` (line 19)
- [X] T053 [P] Update `README.md` — change `.agent-brain/mind.mv2` reference to `.rusty-brain/mind.mv2` (line 51)
- [X] T054 [P] Update `RUST_ROADMAP.md` — change `.agent-brain` references to `.rusty-brain` (lines 117, 365-366)
- [X] T055 [P] Update `.gitignore` — add `.rusty-brain/` entry alongside existing `.agent-brain/` (keep both during migration period) (line 24)
- [X] T056 [P] Search for any remaining `.agent-brain` string literals across the workspace using `rg '\.agent-brain' --type rust --type md` and update as needed
- [X] T057 Run full quality gate: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
- [X] T058 Run quickstart.md validation: verify migration instructions work on a temp directory with `.agent-brain/mind.mv2`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — verify baseline
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 — can start immediately after
- **US2 (Phase 4)**: Depends on Phase 2 — can run in parallel with US1
- **US4 (Phase 5)**: Depends on Phase 2 — can run in parallel with US1 and US2
- **US3 (Phase 6)**: Depends on Phase 4 (US2) — needs `detect_legacy_paths` framework
- **Polish (Phase 7)**: Depends on all user stories complete

### User Story Dependencies

- **US1 (P1)**: Independent — only needs foundational constants
- **US2 (P1)**: Independent — only needs foundational constants
- **US4 (P1)**: Independent — only needs foundational constants
- **US3 (P2)**: Depends on US2 — extends the `detect_legacy_paths` function built in US2

### Within Each User Story

- Tests written FIRST and verified to FAIL
- Implementation follows to make tests pass
- Verification step at end of each phase

### Parallel Opportunities

- **Phase 2**: T002–T004 can run in parallel (different constants, same file — coordinate edits)
- **Phase 3 + Phase 4 + Phase 5**: US1, US2, and US4 can all run in parallel after Phase 2
- **Phase 4**: T028–T032 are independent test file updates, all parallelizable
- **Phase 7**: T049–T056 can all run in parallel (different files)

---

## Parallel Example: After Phase 2 Completion

```text
# Agent 1: US1 (new installation path)
Task: T012–T014 in crates/types/tests/config_test.rs

# Agent 2: US2 (migration from .agent-brain)
Task: T015–T033 in crates/platforms/src/bootstrap.rs + hooks/ + opencode/

# Agent 3: US4 (supporting files)
Task: T034–T042 in crates/hooks/src/dedup.rs + crates/hooks/src/smart_install.rs + hooks/tests/
```

---

## Implementation Strategy

### MVP First (US1 Only)

1. Complete Phase 1: Setup (baseline verification)
2. Complete Phase 2: Foundational (constant updates)
3. Complete Phase 3: US1 (new installation path works)
4. **STOP and VALIDATE**: `cargo test` passes, new installations resolve to `.rusty-brain/`
5. Existing `.agent-brain/` users temporarily broken until US2

### Recommended: US1 + US2 Together

Since US2 provides the fallback that prevents breaking existing users, ship US1 and US2 together as the minimum viable change:

1. Phase 1 + Phase 2: Setup + Foundational
2. Phase 3 + Phase 4: US1 + US2 (parallel or sequential)
3. **STOP and VALIDATE**: New installations work, existing installations fall back gracefully
4. Phase 5: US4 (supporting files)
5. Phase 6: US3 (legacy .claude detection)
6. Phase 7: Polish

### Incremental Delivery

1. Setup + Foundational → Constants ready
2. US1 + US2 → Core path change complete, no data loss
3. US4 → Supporting files migrated
4. US3 → Full three-tier legacy detection
5. Polish → Documentation, final validation

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires test-first: write tests, verify they fail, then implement
- FR-008: Never auto-migrate files — all migration is user-initiated
- FR-009: All diagnostic messages must include actionable `mv` commands (not just descriptions)
- The `detect_legacy_path` → `detect_legacy_paths` rename is a breaking internal API change; all callers (session_start.rs, post_tool_use.rs, hooks/bootstrap.rs re-export) must be updated in the same phase (T025–T027)
- `smart_install.rs` version file path change means fresh installs write to `.rusty-brain/.install-version` but existing `.install-version` at project root won't be auto-detected — acceptable per spec (new installations only)
- `.gitignore` gets both `.agent-brain/` and `.rusty-brain/` during migration period
