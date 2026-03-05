# Tasks: Plugin Packaging & Distribution

**Input**: Design documents from `/specs/009-plugin-packaging/`
**Prerequisites**: plan.md (required), spec.md (required), prd.md, ar.md, sec.md, data-model.md, contracts/, research.md, quickstart.md

**Tests**: Included — plan.md specifies ShellCheck, bats-core, and Pester testing; constitution mandates test-first development.

**Organization**: Tasks grouped by user story for independent implementation and testing. Execution order differs from PRD priority order due to dependencies (US3 must complete before US1).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

All new files are outside `crates/` (distribution overlay):
- `packaging/claude-code/` — Claude Code plugin manifests, skills, hooks, commands
- `packaging/opencode/` — OpenCode command definitions
- `.github/workflows/` — CI/CD release pipeline
- `install.sh`, `install.ps1` — Install scripts at repo root
- `tests/` — Install script tests

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create packaging directory structure for all downstream tasks

- [x] T001 Create `packaging/` directory structure per plan.md: `packaging/claude-code/.claude-plugin/`, `packaging/claude-code/hooks/`, `packaging/claude-code/skills/mind/`, `packaging/claude-code/skills/memory/`, `packaging/claude-code/commands/`, `packaging/opencode/commands/`

---

## Phase 2: User Story 3 — Cross-Platform Release Binaries (Priority: P1) :dart: MVP

**Goal**: GitHub Actions CI pipeline builds and publishes pre-compiled binaries for all 5 supported platforms on each tagged release, packaged as `.tar.gz` with `.sha256` sidecar files.

**Independent Test**: Push a release tag (`v0.0.1-test`) and verify all 5 platform binaries plus checksums are published as GitHub Release assets.

**Why first**: Release assets must exist before install scripts (US1) can download them. This is the foundational pipeline.

**PRD Requirements**: M-3, M-4, M-9, S-2

### Implementation for User Story 3

- [x] T002 [US3] Create `.github/workflows/release.yml` — `create-release` job: trigger on `v[0-9]+.[0-9]+.[0-9]+` tags, extract version from `github.ref_name`, validate tag matches `Cargo.toml` workspace version, create draft GitHub Release via `gh release create --draft`
- [x] T003 [US3] Add `build-release` matrix job to `.github/workflows/release.yml` — 5-target build matrix using `houseabsolute/actions-rust-cross` (pinned SHA), set `MACOSX_DEPLOYMENT_TARGET=11.0` for macOS, use cross-rs for Linux musl targets, native cargo for macOS/Windows. Matrix: `{x86_64-unknown-linux-musl: ubuntu-24.04, aarch64-unknown-linux-musl: ubuntu-24.04, x86_64-apple-darwin: macos-13, aarch64-apple-darwin: macos-14, x86_64-pc-windows-msvc: windows-latest}`
- [x] T004 [US3] Add archive packaging step to `build-release` job in `.github/workflows/release.yml` — create staging directory `rusty-brain-v{version}-{target}/` containing both `rusty-brain` and `rusty-brain-hooks` binaries plus LICENSE and README.md, create `rusty-brain-v{version}-{target}.tar.gz`, generate `.sha256` sidecar file per `contracts/release-asset-naming.md` contract
- [x] T005 [US3] Add asset upload and `publish-release` job to `.github/workflows/release.yml` — upload `.tar.gz` + `.sha256` assets to draft release in `build-release` job, add final `publish-release` job (needs: build-release) that undrafts the release via `gh release edit --draft=false` after all builds succeed. Use pinned SHAs for all third-party actions (SEC-9)

**Checkpoint**: At this point, pushing a version tag produces a complete GitHub Release with 10 assets (5 archives + 5 checksums).

---

## Phase 3: User Story 2 — Claude Code Plugin Registration (Priority: P1)

**Goal**: Create all static plugin manifests, skill definitions, hook registrations, and slash commands that register rusty-brain as a Claude Code plugin.

**Independent Test**: Copy the `packaging/claude-code/` contents to `~/.claude/plugins/rusty-brain/` and verify Claude Code discovers the plugin and lists all skills.

**PRD Requirements**: M-1, M-2, M-8

### Implementation for User Story 2

- [x] T006 [P] [US2] Create `packaging/claude-code/.claude-plugin/plugin.json` — plugin identity manifest per `contracts/plugin-json.schema.json`: name `rusty-brain`, skills pointing to `skills/mind/SKILL.md` and `skills/memory/SKILL.md`, hooks pointing to `hooks/hooks.json`, commands referencing `commands/` directory
- [x] T007 [P] [US2] Create `packaging/claude-code/marketplace.json` — marketplace registry per `contracts/marketplace-json.schema.json`: owner info, plugin entry with name, description, version, source URL
- [x] T008 [P] [US2] Create `packaging/claude-code/hooks/hooks.json` — hook registration per `contracts/hooks-json.schema.json` and `data-model.md`: SessionStart runs `${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks session-start`, PostToolUse (matcher: `*`) runs `${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks post-tool-use`, Stop runs `${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks stop`
- [x] T009 [P] [US2] Create `packaging/claude-code/skills/mind/SKILL.md` — mind skill definition with YAML frontmatter (name: mind, description), instructions for search/ask/recent/stats operations, invocation commands referencing `rusty-brain` binary
- [x] T010 [P] [US2] Create `packaging/claude-code/skills/memory/SKILL.md` — memory skill definition with YAML frontmatter (name: memory, description), instructions for memory capture and storage operations
- [x] T011 [P] [US2] Create Claude Code slash command definitions: `packaging/claude-code/commands/ask.md`, `packaging/claude-code/commands/search.md`, `packaging/claude-code/commands/recent.md`, `packaging/claude-code/commands/stats.md` — each with appropriate description and invocation instructions

**Checkpoint**: Plugin manifests and skill definitions are complete. Can be manually tested by copying to `~/.claude/plugins/rusty-brain/`.

---

## Phase 4: User Story 4 — OpenCode Slash Command Integration (Priority: P2)

**Goal**: OpenCode users get slash commands (`/ask`, `/search`, `/recent`, `/stats`) that invoke rusty-brain memory operations.

**Independent Test**: Place command definitions in OpenCode's commands directory and verify all 4 commands appear and route to rusty-brain.

**PRD Requirements**: S-1

### Implementation for User Story 4

- [x] T016 [P] [US4] Create OpenCode command definitions: `packaging/opencode/commands/mind-ask.md`, `packaging/opencode/commands/mind-search.md`, `packaging/opencode/commands/mind-recent.md`, `packaging/opencode/commands/mind-stats.md` — each with YAML frontmatter (description, argument-hint) and body referencing the `mind` native tool with `mode` and `query` parameters per research.md R-10

**Checkpoint**: OpenCode command definitions are complete. Can be tested by placing in OpenCode's command discovery directory.

---

## Phase 4a: Install Script Tests (Test-First per Constitution V)

**Purpose**: Write install script tests BEFORE implementation, per Constitution Principle V (Test-First Development — Non-Negotiable)

**Depends on**: Setup (Phase 1) only — tests define expected behavior independent of implementation

- [x] T017 [P] [US1] Create `tests/install_script_test.bats` — bats-core unit tests for `install.sh` functions: platform detection (all 5 targets + unsupported), SHA-256 verification (valid + corrupted + missing tool), error messages (network failure, permission denied, unsupported platform), temp cleanup on failure (SEC-10), non-zero file size check (SEC-11), version string injection rejection (SEC-5), malformed JSON handling (SEC-4)
- [x] T018 [P] [US1] Create `tests/install_script_test.ps1` — Pester unit tests for `install.ps1` functions: platform detection, download, SHA-256 verification, error handling, PATH detection

---

## Phase 4b: User Story 1 — One-Command Installation (Priority: P1)

**Goal**: Users install rusty-brain on macOS, Linux, or Windows with a single command. The script detects platform, downloads the correct binary, verifies SHA-256, installs the binary and plugin manifests, and provides PATH guidance.

**Independent Test**: Run `sh install.sh` on a clean machine; verify binary at `~/.local/bin/rusty-brain`, plugin at `~/.claude/plugins/rusty-brain/`, and `rusty-brain --version` returns valid version.

**Depends on**: US3 (release assets to download), US2 (manifest content to embed as heredocs), **Phase 4a (tests written first)**

**PRD Requirements**: M-5, M-6, M-7, M-8, M-10, M-11

**Security Requirements**: SEC-1 through SEC-12 (see sec.md)

### Implementation for User Story 1

- [x] T012 [US1] Create `install.sh` — POSIX sh core structure: shebang (`#!/bin/sh`), `set -eu`, environment variable handling (`RUSTY_BRAIN_VERSION`, `RUSTY_BRAIN_INSTALL_DIR`, `GITHUB_TOKEN`), platform detection via `uname -s`/`uname -m` with `arm64`→`aarch64` normalization and Rosetta detection (`/usr/bin/uname -m` on macOS), target triple mapping, temp directory creation with `trap` cleanup on EXIT, error helper functions with actionable messages per `contracts/install-script-interface.md` output contract
- [x] T013 [US1] Add download and SHA-256 verification to `install.sh` — query GitHub Releases API for latest (or `RUSTY_BRAIN_VERSION`), extract asset URL matching target triple, download `.tar.gz` archive and `.sha256` sidecar to temp dir, verify non-zero file size (SEC-11), SHA-256 verification using `sha256sum`/`shasum -a 256`/`openssl dgst -sha256` fallback chain (research.md R-4), no `eval` (SEC-7), HTTPS-only (SEC-8), handle rate limiting with `GITHUB_TOKEN` hint
- [x] T014 [US1] Add installation logic to `install.sh` — extract binary with `tar` using `--strip-components=1` for path traversal protection (SEC-12), place `rusty-brain` binary in install dir (`~/.local/bin/`), `chmod +x`, detect existing installation and print upgrade summary (old→new version), embed plugin manifests as heredocs (plugin.json, marketplace.json, hooks.json, SKILL.md files from Phase 3) written to `~/.claude/plugins/rusty-brain/` directory tree, copy `rusty-brain-hooks` binary to plugin dir, PATH detection (`~/.local/bin` in `$PATH`?) with print-only instructions (no shell config modification per M-11), never touch `~/.agent-brain/` (SEC-1)
- [x] T015 [P] [US1] Create `install.ps1` — PowerShell 5.1+ install script: detect Windows x86_64, query GitHub Releases API, download via `Invoke-WebRequest`, SHA-256 verification via `Get-FileHash`, extract `.tar.gz` via `tar`, place binary in `$env:LOCALAPPDATA\rusty-brain\bin\`, create plugin dir at `$env:APPDATA\.claude\plugins\rusty-brain\` with manifest files, copy hooks binary, PATH detection with instructions per `contracts/install-script-interface.md`
- [x] T019 [US1] Lint `install.sh` with ShellCheck and verify POSIX sh compliance — no bashisms, no `eval`, no backtick substitution (SEC-7), all variables quoted

**Checkpoint**: Both install scripts are complete and pass their test suites.

---

## Phase 5: Validation & Cross-Cutting Concerns

**Purpose**: Schema validation, security hardening review, and quickstart verification

- [x] T020 Validate all manifests against contract schemas — verify `packaging/claude-code/.claude-plugin/plugin.json` matches `contracts/plugin-json.schema.json`, `marketplace.json` matches `contracts/marketplace-json.schema.json`, `hooks.json` matches `contracts/hooks-json.schema.json`, release asset naming follows `contracts/release-asset-naming.md`
- [x] T021 Security hardening review — verify SEC-1 through SEC-12 are satisfied: no `.mv2` access (SEC-1), no memory content logging (SEC-2), no secrets in manifests (SEC-3), HTTPS enforcement (SEC-8), pinned action SHAs (SEC-9), trap cleanup (SEC-10), tar path traversal protection (SEC-12)
- [x] T022 Run quickstart.md validation — verify documented workflow steps match actual file layout and commands

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **US3 — Release Binaries (Phase 2)**: Depends on Setup — creates CI pipeline
- **US2 — Plugin Registration (Phase 3)**: Depends on Setup — creates static manifest files. **Can run in parallel with US3**
- **US4 — OpenCode Commands (Phase 4)**: Depends on Setup only — **can run in parallel with US2/US3**
- **Install Script Tests (Phase 4a)**: Depends on Setup only — **tests written before implementation per Constitution V**
- **US1 — Install Scripts (Phase 4b)**: Depends on **US3** (release assets to download), **US2** (manifest content to embed as heredocs), and **Phase 4a** (tests first)
- **Validation (Phase 6)**: Depends on all user stories being complete

### User Story Dependencies

```
Phase 1: Setup
     │
     ├──────────────┬──────────────┬──────────────┐
     ▼              ▼              ▼              ▼
Phase 2: US3   Phase 3: US2  Phase 4: US4  Phase 4a: Tests
(Release Bins) (Plugin Reg)  (OpenCode)    (install script
     │              │              │         tests first)
     └──────┬───────┘              │              │
            ▼                      │              │
      Phase 4b: US1  ◄────────────────────────────┘
     (Install Scripts)             │
            │                      │
            └──────────┬───────────┘
                       ▼
                 Phase 6: Validation
```

### Within Each Phase

- Tasks marked [P] within a phase can run in parallel
- T002→T003→T004→T005 are sequential (same file, building on each other)
- T006 through T011 are all [P] (different files)
- T012→T013→T014 are sequential (building the install script incrementally)
- T015 can run in parallel with T012-T014 (different file)

### Parallel Opportunities

- **Max parallelism (Phase 2+3+4+4a)**: US3 (release workflow), US2 (6 manifest files), US4 (command definitions), and install script tests can all proceed simultaneously after Setup
- **Within US2**: All 6 tasks (T006-T011) are independent files — full parallel execution
- **Install script tests**: T017 (bats) and T018 (Pester) can be written in parallel
- **Install scripts**: `install.sh` (T012-T014) and `install.ps1` (T015) can be written in parallel (after tests exist)
- **Validation phase**: T020, T021, T022 are all independent — full parallel execution

---

## Parallel Example: Phase 3 (US2 — Plugin Registration)

```
# Launch all 6 manifest/skill tasks in parallel:
Task: "Create plugin.json in packaging/claude-code/.claude-plugin/plugin.json"
Task: "Create marketplace.json in packaging/claude-code/marketplace.json"
Task: "Create hooks.json in packaging/claude-code/hooks/hooks.json"
Task: "Create mind SKILL.md in packaging/claude-code/skills/mind/SKILL.md"
Task: "Create memory SKILL.md in packaging/claude-code/skills/memory/SKILL.md"
Task: "Create slash commands in packaging/claude-code/commands/"
```

---

## Implementation Strategy

### MVP First (US3 + US2 + US1)

1. Complete Phase 1: Setup (T001)
2. Complete Phase 2: US3 — Release Binaries (T002-T005) + Phase 3: US2 — Plugin Registration (T006-T011) **in parallel**
3. Complete Phase 4: US1 — Install Scripts (T012-T015)
4. **STOP and VALIDATE**: Tag a test release (`v0.0.1-test`), run install script, verify binary + plugin discovery
5. MVP is complete — users can install and use rusty-brain

### Incremental Delivery

1. Setup → Foundation ready
2. US3 (Release Binaries) + US2 (Plugin Registration) → CI pipeline + manifests ready
3. US1 (Install Scripts) → Full install path works — **MVP!**
4. US4 (OpenCode Commands) → OpenCode users supported
5. Polish → Testing, linting, security validation

### Suggested MVP Scope

**US3 + US2 + US1** (all P1 stories) — This gives users a complete install-to-use path on all 5 platforms with Claude Code integration. US4 (P2) and Polish can follow.

---

## Requirement Traceability

| PRD Req | Task(s) | User Story |
|---------|---------|------------|
| M-1 | T006, T007, T014 | US2, US1 |
| M-2 | T009, T010, T014 | US2, US1 |
| M-3 | T003, T004 | US3 |
| M-4 | T004 | US3 |
| M-5 | T012, T013, T014 | US1 |
| M-6 | T015 | US1 |
| M-7 | T014 | US1 |
| M-8 | T006, T014 | US2, US1 |
| M-9 | T003 | US3 |
| M-10 | T012, T013, T014 | US1 |
| M-11 | T014 | US1 |
| S-1 | T016 | US4 |
| S-2 | T002, T003, T004, T005 | US3 |
| S-3 | T012 | US1 |

| SEC Req | Task(s) | Verification |
|---------|---------|--------------|
| SEC-1 | T014 | Never touch ~/.agent-brain/ |
| SEC-2 | T012-T014 | No memory content in output |
| SEC-3 | T006-T011 | No secrets in manifests |
| SEC-4 | T013, T017 | Validate API JSON |
| SEC-5 | T013, T017 | Reject shell metacharacters |
| SEC-6 | T013 | No eval in checksum |
| SEC-7 | T012-T014, T019 | No eval, no backticks |
| SEC-8 | T013 | HTTPS only |
| SEC-9 | T002-T005 | Pinned action SHAs |
| SEC-10 | T012, T017 | Trap cleanup |
| SEC-11 | T013, T017 | Non-zero file size |
| SEC-12 | T014 | Tar strip-components |

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- No Rust code changes — all tasks produce scripts, YAML, JSON, or Markdown files
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- AR guardrails: DO NOT modify crates/, ci.yml, Cargo.toml, shell config files, or ~/.agent-brain/
