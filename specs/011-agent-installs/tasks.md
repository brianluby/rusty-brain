# Tasks: Agent Installs

**Input**: Design documents from `/specs/011-agent-installs/`
**Prerequisites**: plan.md, spec.md, prd.md, ar.md, sec.md, data-model.md, contracts/

**Tests**: Included — Constitution V mandates test-first development (non-negotiable).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Project initialization, dependency promotion, module scaffolding

- [X] T001 Promote `tempfile` from dev-dependency to regular dependency in `crates/platforms/Cargo.toml`
- [X] T002 [P] Create installer module directory structure: `crates/platforms/src/installer/mod.rs`, `crates/platforms/src/installer/agents/mod.rs`
- [X] T003 [P] Add `pub mod installer;` to `crates/platforms/src/lib.rs`
- [X] T004 [P] Create `crates/types/src/install.rs` with module declaration and add `pub mod install;` to `crates/types/src/lib.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types, traits, and infrastructure that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

### Types (types crate)

- [X] T005 Define `InstallError` enum with stable error codes (AgentNotFound, PermissionDenied, UnsupportedVersion, ConfigCorrupted, IoError, ScopeRequired, InvalidAgent, PathTraversal) in `crates/types/src/install.rs` — implement `Display` via `thiserror` with `[E_INSTALL_*]` prefixed messages
- [X] T006 [P] Define `InstallScope` enum (Project { root: PathBuf }, Global) in `crates/types/src/install.rs`
- [X] T007 [P] Define `AgentInfo` struct (name, binary_path, version) in `crates/types/src/install.rs`
- [X] T008 [P] Define `ConfigFile` struct (target_path, content, description) in `crates/types/src/install.rs`
- [X] T009 [P] Define `InstallConfig` struct (agents, scope, json, reconfigure, config_dir) in `crates/types/src/install.rs`
- [X] T010 [P] Define `InstallStatus` enum (Configured, Upgraded, Skipped, Failed, NotFound) with Serialize in `crates/types/src/install.rs`
- [X] T011 [P] Define `AgentInstallResult` struct (agent_name, status, config_path, version_detected, error) with Serialize in `crates/types/src/install.rs`
- [X] T012 Define `InstallReport` struct (status, results, memory_store, scope) with Serialize in `crates/types/src/install.rs`

### AgentInstaller Trait

- [X] T013 Write unit tests for `AgentInstaller` trait contract expectations in `crates/platforms/src/installer/mod.rs` (test module) — verify trait is object-safe and can be stored in Box
- [X] T014 Define `AgentInstaller` trait with methods: `agent_name()`, `detect()`, `generate_config()`, `validate()` in `crates/platforms/src/installer/mod.rs` per contracts/agent-installer-trait.rs

### Binary Detection Utility

- [X] T015 Write unit tests for `find_binary_on_path()` in `crates/platforms/src/installer/mod.rs` — test: binary found, binary not found, Windows .exe extension, empty PATH
- [X] T016 Implement `find_binary_on_path(name: &str) -> Option<PathBuf>` using `std::env::split_paths` + `Path::join` in `crates/platforms/src/installer/mod.rs` — no shell execution (SEC-7)

### Path Validation (SEC-4)

- [X] T017 Write unit tests for `validate_config_path()` in `crates/platforms/src/installer/mod.rs` — test: valid path, path with `..` traversal, symlink outside boundary, absolute path
- [X] T018 Implement `validate_config_path(path: &Path) -> Result<PathBuf, InstallError>` that canonicalizes and rejects traversal sequences in `crates/platforms/src/installer/mod.rs`

### Global Scope Path Resolution

- [X] T018a [P] Implement `resolve_global_config_dir(agent_name: &str) -> PathBuf` in `crates/platforms/src/installer/mod.rs` — resolve per-platform global config directories (macOS: `~/Library/Application Support/<agent>/`, Linux: `~/.config/<agent>/`, Windows: `%APPDATA%/<agent>/`) using `cfg!(target_os)` and env vars
- [X] T018b [P] Write unit tests for `resolve_global_config_dir()` in `crates/platforms/src/installer/mod.rs` — test: each platform variant, missing HOME/APPDATA env var returns error

### ConfigWriter

- [X] T019 Write unit tests for `ConfigWriter` in `crates/platforms/src/installer/writer.rs` — test: write new file (atomic), write with backup, backup when no existing file, directory creation, file permissions (0o644 on Unix)
- [X] T020 Implement `ConfigWriter` struct with `write(config: &ConfigFile, backup: bool) -> Result<(), InstallError>` using `tempfile::NamedTempFile` for atomic writes in `crates/platforms/src/installer/writer.rs` — set 0o644 permissions on Unix (SEC-1), create parent dirs (M-12)
- [X] T021 Implement `ConfigWriter::backup(path: &Path) -> Result<(), InstallError>` that copies existing file to `.bak` in `crates/platforms/src/installer/writer.rs` (SEC-8, S-1)

### InstallerRegistry

- [X] T022 Write unit tests for `InstallerRegistry` in `crates/platforms/src/installer/registry.rs` — test: register, resolve case-insensitive, list sorted agents, resolve unknown returns None (mirror AdapterRegistry test pattern)
- [X] T023 Implement `InstallerRegistry` with `register()`, `resolve()`, `agents()` methods in `crates/platforms/src/installer/registry.rs`

### InstallOrchestrator

- [X] T024 Write unit tests for `InstallOrchestrator` in `crates/platforms/src/installer/orchestrator.rs` — test: auto-detect flow, explicit agent list, agent not found continues, scope required error, per-agent failure isolation
- [X] T025 Implement `InstallOrchestrator` with `run(config: InstallConfig) -> Result<InstallReport, InstallError>` in `crates/platforms/src/installer/orchestrator.rs` — fail per-agent not per-command, use ConfigWriter for all writes, tracing for logging (S-2)
- [X] T026 Add agent name validation against hardcoded allowlist (opencode, copilot, codex, gemini) in `InstallOrchestrator::run()` in `crates/platforms/src/installer/orchestrator.rs` (SEC-5)

### CLI Integration

- [X] T027 Write CLI arg parsing tests for `Install` subcommand in `crates/cli/src/args.rs` — test: --agents parsing, --project/--global mutual exclusion, --json flag, --reconfigure flag, --config-dir, scope required error (M-13)
- [X] T028 Add `Install` variant to `Command` enum in `crates/cli/src/args.rs` with all flags: --agents (comma-delimited), --project, --global (required group "scope"), --json, --reconfigure, --config-dir
- [X] T029 Update `Command::json()` method to include Install variant in `crates/cli/src/args.rs`
- [X] T030 Create `crates/cli/src/install_cmd.rs` with `run_install()` function that builds `InstallConfig` from CLI args and calls `InstallOrchestrator::run()`, then formats output as JSON or human-readable table
- [X] T031 Add install dispatch to `crates/cli/src/commands.rs` routing `Command::Install` to `install_cmd::run_install()`

**Checkpoint**: Foundation ready — all types, traits, writer, registry, orchestrator, and CLI routing in place. User story implementation can begin.

---

## Phase 3: User Story 1 — OpenCode Plugin Installation (Priority: P1) MVP

**Goal**: Users can run `rusty-brain install --agents opencode --project` to configure rusty-brain for OpenCode with all slash commands registered.

**Independent Test**: Run install in a tempdir with mocked OpenCode binary, verify config files created with correct content.

### Tests for User Story 1

- [X] T032 [P] [US1] Write unit tests for `OpenCodeInstaller::detect()` in `crates/platforms/src/installer/agents/opencode.rs` — test: binary found with version, binary not found, version parse failure
- [X] T033 [P] [US1] Write unit tests for `OpenCodeInstaller::generate_config()` in `crates/platforms/src/installer/agents/opencode.rs` — test: project scope config content, global scope config content, config references correct binary path and memory store path, slash commands registered (/ask, /search, /recent, /stats)
- [X] T034 [P] [US1] Write integration test for full OpenCode install flow in `crates/platforms/tests/install_opencode_test.rs` — test: install to tempdir, verify files exist with correct content, upgrade preserves data (AC-1, AC-2, AC-9)

### Implementation for User Story 1

- [X] T035 [US1] Implement `OpenCodeInstaller` struct with `AgentInstaller` trait in `crates/platforms/src/installer/agents/opencode.rs` — detect via `find_binary_on_path("opencode")`, version via subprocess with 2s timeout (SEC-6)
- [X] T036 [US1] Implement `OpenCodeInstaller::generate_config()` returning `Vec<ConfigFile>` with OpenCode plugin manifest JSON and slash command registrations in `crates/platforms/src/installer/agents/opencode.rs` — pure function, no I/O
- [X] T037 [US1] Implement `OpenCodeInstaller::validate()` checking config files exist and are valid JSON in `crates/platforms/src/installer/agents/opencode.rs`
- [X] T038 [US1] Register `OpenCodeInstaller` in `InstallerRegistry::with_builtins()` in `crates/platforms/src/installer/registry.rs`
- [X] T039 [US1] Export `opencode_installer()` factory function from `crates/platforms/src/installer/agents/mod.rs`

**Checkpoint**: `rusty-brain install --agents opencode --project` works end-to-end. All tests pass.

---

## Phase 4: User Story 2 — GitHub Copilot CLI Installation (Priority: P1)

**Goal**: Users can run `rusty-brain install --agents copilot --project` to configure rusty-brain for Copilot CLI.

**Independent Test**: Run install in a tempdir with mocked Copilot binary, verify config files created.

**NOTE**: Requires Spike-1 research (PRD). If Copilot extension mechanism is unconfirmed, implement as a stub installer that reports "not yet supported" with clear messaging.

### Tests for User Story 2

- [X] T040 [P] [US2] Write unit tests for `CopilotInstaller::detect()` in `crates/platforms/src/installer/agents/copilot.rs` — test: binary found, binary not found
- [X] T041 [P] [US2] Write unit tests for `CopilotInstaller::generate_config()` in `crates/platforms/src/installer/agents/copilot.rs` — test: config content matches Copilot extension format (or stub error if unconfirmed)

### Implementation for User Story 2

- [X] T042 [US2] Implement `CopilotInstaller` struct with `AgentInstaller` trait in `crates/platforms/src/installer/agents/copilot.rs` — detect via `find_binary_on_path("gh")` + check for copilot extension
- [X] T043 [US2] Implement `CopilotInstaller::generate_config()` with Copilot-specific config templates in `crates/platforms/src/installer/agents/copilot.rs`
- [X] T044 [US2] Register `CopilotInstaller` in `InstallerRegistry::with_builtins()` in `crates/platforms/src/installer/registry.rs`
- [X] T045 [US2] Export `copilot_installer()` factory from `crates/platforms/src/installer/agents/mod.rs`

**Checkpoint**: `rusty-brain install --agents copilot --project` works (or returns clear "not yet supported" stub).

---

## Phase 5: User Story 3 — OpenAI Codex CLI Installation (Priority: P1)

**Goal**: Users can run `rusty-brain install --agents codex --project` to configure rusty-brain for Codex CLI.

**Independent Test**: Run install in a tempdir with mocked Codex binary, verify config files created.

**NOTE**: Requires Spike-2 research (PRD). If Codex extension mechanism is unconfirmed, implement as stub.

### Tests for User Story 3

- [X] T046 [P] [US3] Write unit tests for `CodexInstaller::detect()` in `crates/platforms/src/installer/agents/codex.rs`
- [X] T047 [P] [US3] Write unit tests for `CodexInstaller::generate_config()` in `crates/platforms/src/installer/agents/codex.rs`

### Implementation for User Story 3

- [X] T048 [US3] Implement `CodexInstaller` struct with `AgentInstaller` trait in `crates/platforms/src/installer/agents/codex.rs` — detect via `find_binary_on_path("codex")`
- [X] T049 [US3] Implement `CodexInstaller::generate_config()` with Codex-specific config templates in `crates/platforms/src/installer/agents/codex.rs`
- [X] T050 [US3] Register `CodexInstaller` in `InstallerRegistry::with_builtins()` in `crates/platforms/src/installer/registry.rs`
- [X] T051 [US3] Export `codex_installer()` factory from `crates/platforms/src/installer/agents/mod.rs`

**Checkpoint**: `rusty-brain install --agents codex --project` works (or returns clear stub).

---

## Phase 6: User Story 5 — Unified Multi-Agent Install (Priority: P1)

**Goal**: Users can run `rusty-brain install --project` to auto-detect and configure all installed agents in one command.

**Independent Test**: Run install in a tempdir with multiple mocked agent binaries, verify all detected agents configured and sharing same memory store path.

### Tests for User Story 5

- [X] T052 [P] [US5] Write integration test for auto-detection flow in `crates/platforms/tests/install_multi_agent_test.rs` — test: multiple agents detected, single memory store path shared (AC-5, AC-7)
- [X] T053 [P] [US5] Write integration test for explicit `--agents` filtering in `crates/platforms/tests/install_multi_agent_test.rs` — test: only specified agents configured (AC-6)
- [X] T054 [P] [US5] Write integration test for agent-not-found handling in `crates/platforms/tests/install_multi_agent_test.rs` — test: missing agent reports warning, continues for others (AC-10)

### Implementation for User Story 5

- [X] T055 [US5] Verify `InstallOrchestrator::run()` auto-detection iterates all registered installers when `config.agents` is None in `crates/platforms/src/installer/orchestrator.rs` — add missing logic if needed
- [X] T056 [US5] Verify `InstallOrchestrator::run()` filters to specified agents when `config.agents` is Some in `crates/platforms/src/installer/orchestrator.rs` — add missing logic if needed
- [X] T057 [P] [US5] Write integration test verifying all agent configs reference the same shared memory store path (`.rusty-brain/mind.mv2`) in `crates/platforms/tests/install_multi_agent_test.rs` — install multiple agents, parse each config, assert identical memory_store path (AC-7)
- [X] T057a [US5] Verify orchestrator populates `InstallReport.memory_store` with the correct shared path in `crates/platforms/src/installer/orchestrator.rs`

**Checkpoint**: `rusty-brain install --project` auto-detects and configures all agents. Explicit `--agents` filtering works.

---

## Phase 7: User Story 4 — Google Gemini CLI Installation (Priority: P2)

**Goal**: Users can run `rusty-brain install --agents gemini --project` to configure rusty-brain for Gemini CLI.

**Independent Test**: Run install in a tempdir with mocked Gemini binary, verify config files created.

**NOTE**: Requires Spike-3 research (PRD). If Gemini extension mechanism is unconfirmed, implement as stub.

### Tests for User Story 4

- [X] T058 [P] [US4] Write unit tests for `GeminiInstaller::detect()` in `crates/platforms/src/installer/agents/gemini.rs`
- [X] T059 [P] [US4] Write unit tests for `GeminiInstaller::generate_config()` in `crates/platforms/src/installer/agents/gemini.rs`

### Implementation for User Story 4

- [X] T060 [US4] Implement `GeminiInstaller` struct with `AgentInstaller` trait in `crates/platforms/src/installer/agents/gemini.rs` — detect via `find_binary_on_path("gemini")`
- [X] T061 [US4] Implement `GeminiInstaller::generate_config()` with Gemini-specific config templates in `crates/platforms/src/installer/agents/gemini.rs`
- [X] T062 [US4] Register `GeminiInstaller` in `InstallerRegistry::with_builtins()` in `crates/platforms/src/installer/registry.rs`
- [X] T063 [US4] Export `gemini_installer()` factory from `crates/platforms/src/installer/agents/mod.rs`

**Checkpoint**: `rusty-brain install --agents gemini --project` works (or returns clear stub).

---

## Phase 8: User Story 6 — Agent Self-Installation (Priority: P2)

**Goal**: AI agents can invoke `rusty-brain install --agents <self> --project --json` programmatically with no interactive prompts and receive structured JSON output.

**Independent Test**: Invoke install command with non-TTY stdin, verify JSON output is valid and parseable, no prompts issued.

### Tests for User Story 6

- [X] T064 [P] [US6] Write CLI integration test for `--json` output format in `crates/cli/tests/install_json_test.rs` using `assert_cmd` — verify valid JSON structure matches `InstallReport` schema (AC-8)
- [X] T065 [P] [US6] Write CLI integration test for non-TTY auto-JSON detection in `crates/cli/tests/install_json_test.rs` — verify JSON output when stdin is not a TTY (AC-12)
- [X] T066 [P] [US6] Write CLI integration test for error JSON output in `crates/cli/tests/install_json_test.rs` — verify error JSON has error_code and suggestion fields (AC-11, SEC-10)

### Implementation for User Story 6

- [X] T067 [US6] Add non-TTY detection to `install_cmd::run_install()` that auto-enables JSON output when stdin is not a TTY in `crates/cli/src/install_cmd.rs` (M-7)
- [X] T068 [US6] Verify no interactive prompts exist in install code path — audit all install modules for stdin reads in `crates/platforms/src/installer/` (M-11)
- [X] T069 [US6] Verify error output includes machine-parseable error codes and remediation suggestions in JSON format in `crates/cli/src/install_cmd.rs` (M-10, SEC-10)

**Checkpoint**: Agents can invoke install programmatically with structured JSON responses.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Security hardening, quality gates, documentation

- [X] T070 [P] Run `cargo clippy --workspace -- -D warnings` and fix all warnings
- [X] T071 [P] Run `cargo fmt --check` and fix all formatting issues
- [X] T072 [P] Verify all error messages do not leak internal filesystem structure beyond relevant config path (SEC-10) — review error Display impls in `crates/types/src/install.rs`
- [X] T073 [P] Verify memory contents are never logged during install operations (SEC-2) and install manifest contains only paths, not memory data (SEC-3) — grep install modules for memory content access and audit InstallReport/AgentInstallResult serialization
- [X] T075 Run quickstart.md validation — execute all commands from `specs/011-agent-installs/quickstart.md` and verify expected output
- [X] T076 Update `crates/platforms/src/lib.rs` public re-exports to include key installer types for ergonomic imports

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 OpenCode (Phase 3)**: Depends on Foundational — MVP target
- **US2 Copilot (Phase 4)**: Depends on Foundational — can parallel with US1
- **US3 Codex (Phase 5)**: Depends on Foundational — can parallel with US1/US2
- **US5 Multi-Agent (Phase 6)**: Depends on at least US1 being complete (needs 1+ installer to test)
- **US4 Gemini (Phase 7)**: Depends on Foundational — can parallel with US1-US3
- **US6 Self-Install (Phase 8)**: Depends on Foundational + at least US1 (needs working installer for JSON output testing)
- **Polish (Phase 9)**: Depends on all desired user stories being complete

### User Story Dependencies

- **US1 (OpenCode, P1)**: Independent — start after Foundational
- **US2 (Copilot, P1)**: Independent — can parallel with US1 (requires Spike-1 research)
- **US3 (Codex, P1)**: Independent — can parallel with US1 (requires Spike-2 research)
- **US5 (Multi-Agent, P1)**: Needs at least US1 complete for meaningful integration testing
- **US4 (Gemini, P2)**: Independent — can parallel with P1 stories (requires Spike-3 research)
- **US6 (Self-Install, P2)**: Needs at least US1 complete for end-to-end JSON testing

### Within Each User Story

- Tests MUST be written and FAIL before implementation (Constitution V)
- Detect method before generate_config
- Generate_config before validate
- Register in registry after implementation complete
- Story complete before moving to next priority (or parallel if staffed)

### Parallel Opportunities

**Within Phase 2 (Foundational)**:
- T006, T007, T008, T009, T010, T011 can all run in parallel (independent type definitions)
- T015+T017 (utility tests) can parallel with T019+T022 (writer+registry tests)

**Across User Stories (after Foundational)**:
- US1, US2, US3, US4 can all run in parallel (different files, no shared state)
- Each agent installer is in its own file with no cross-dependencies

**Within Each User Story**:
- Test tasks (detect tests, generate_config tests) can run in parallel
- Implementation follows: detect -> generate_config -> validate -> register

---

## Parallel Example: User Story 1 (OpenCode)

```bash
# Launch tests in parallel (all different files):
Task: "Write unit tests for OpenCodeInstaller::detect() in crates/platforms/src/installer/agents/opencode.rs"
Task: "Write unit tests for OpenCodeInstaller::generate_config() in crates/platforms/src/installer/agents/opencode.rs"
Task: "Write integration test for full OpenCode install flow in crates/platforms/tests/install_opencode_test.rs"

# Then implement sequentially:
Task: "Implement OpenCodeInstaller::detect()"
Task: "Implement OpenCodeInstaller::generate_config()"
Task: "Implement OpenCodeInstaller::validate()"
Task: "Register in InstallerRegistry"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T031)
3. Complete Phase 3: US1 — OpenCode (T032-T039)
4. **STOP and VALIDATE**: `rusty-brain install --agents opencode --project` works end-to-end
5. All quality gates pass: `cargo test`, `cargo clippy`, `cargo fmt`

### Incremental Delivery

1. Setup + Foundational -> Foundation ready
2. Add US1 (OpenCode) -> Test independently -> MVP complete
3. Add US2 (Copilot) -> Test independently (after Spike-1)
4. Add US3 (Codex) -> Test independently (after Spike-2)
5. Add US5 (Multi-Agent) -> Test auto-detection with all installed agents
6. Add US4 (Gemini) -> Test independently (after Spike-3)
7. Add US6 (Self-Install) -> Test JSON output and non-TTY mode
8. Polish -> Quality gates, security hardening

### Spike Research Required

Before implementing US2, US3, US4:
- **Spike-1**: Research Copilot CLI extension mechanism (blocks US2)
- **Spike-2**: Research Codex CLI extension mechanism (blocks US3)
- **Spike-3**: Research Gemini CLI extension mechanism (blocks US4)

These spikes can run in parallel with US1 implementation.

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution V: Tests MUST be written and fail before implementation
- SEC requirements mapped to specific tasks: SEC-1 (T020), SEC-2 (T073), SEC-3 (T073), SEC-4 (T017-T018), SEC-5 (T026), SEC-6 (T035), SEC-7 (T016), SEC-8 (T021), SEC-9 (T020), SEC-10 (T072)
- Global scope path resolution (T018a-T018b) is foundational — each agent's `generate_config()` uses it for `--global` scope
- S-3 (version detection for compatible config): each agent installer's `detect()` should return version info; `generate_config()` should use version to select compatible template when multiple formats exist
- Agents with unconfirmed extension mechanisms (Copilot, Codex, Gemini) should be stubbed with clear "not yet supported" messaging if spike research is incomplete
