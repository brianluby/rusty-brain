# Tasks: Project Bootstrap

**Input**: Design documents from `specs/001-project-bootstrap/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Placeholder tests are part of FR-003 (each crate must have at least one passing test). They are included inline with crate creation tasks, not as separate test phases.

**Organization**: Tasks grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Workspace Root)

**Purpose**: Create the workspace manifest that all crates depend on. Maps to plan Phase A and FR-001, FR-006, FR-007.

- [x] T001 Create virtual workspace manifest in `Cargo.toml` with `[workspace]`, `members = ["crates/*"]`, and `resolver = "3"`
- [x] T002 Configure `[workspace.package]` in `Cargo.toml` with `edition = "2024"`, `rust-version = "1.85.0"`, license, repository, and authors
- [x] T003 Configure `[workspace.lints.clippy]` and `[workspace.lints.rust]` in `Cargo.toml` per contracts/workspace.toml (clippy all=warn, pedantic=warn, correctness=deny; rust unsafe_code=forbid)
- [x] T003a Create `rust-toolchain.toml` at repository root with `[toolchain]` channel = "stable" and minimum version hint, providing early MSRV feedback before Cargo resolution

---

## Phase 2: Foundational (Workspace Dependencies)

**Purpose**: Declare all shared dependencies so crate creation can inherit them. Maps to plan Phase A and FR-004, FR-005, FR-011. MUST complete before crate scaffolding.

**CRITICAL**: No crate creation can begin until dependencies are declared at the workspace level.

- [x] T004 Declare all foundational dependencies in `[workspace.dependencies]` section of `Cargo.toml`: serde (with derive), serde_json, thiserror, tokio (with full), tracing, chrono (with serde), uuid (with v4+serde), clap (with derive), semver — per contracts/workspace.toml
- [x] T005 Add memvid-core as git dependency in `[workspace.dependencies]` of `Cargo.toml`: `memvid-core = { git = "https://github.com/brianluby/memvid/", rev = "fbddef4bff6ac756f91724681234243e98d5ba04" }`

**Checkpoint**: Workspace root is fully configured. `cargo metadata` should parse without errors (no member crates yet, but manifest is valid).

---

## Phase 3: User Story 1 — Developer Builds the Project (Priority: P1) MVP

**Goal**: A developer clones the repo, runs `cargo build --workspace`, and all 7 crates compile with passing placeholder tests.

**Independent Test**: Run `cargo build --workspace && cargo test --workspace` on a fresh clone. Expect zero errors and 7+ passing tests. Then build each crate individually with `cargo build -p <name>` and `cargo test -p <name>`.

### Implementation for User Story 1

- [x] T006 [P] [US1] Create `crates/types/Cargo.toml` with workspace inheritance (name = "types", edition.workspace, rust-version.workspace, lints workspace = true) and `crates/types/src/lib.rs` with placeholder module doc and one passing test
- [x] T007 [P] [US1] Create `crates/core/Cargo.toml` with workspace inheritance (name = "core") and `crates/core/src/lib.rs` with placeholder module doc and one passing test
- [x] T008 [P] [US1] Create `crates/platforms/Cargo.toml` with workspace inheritance (name = "platforms") and `crates/platforms/src/lib.rs` with placeholder module doc and one passing test
- [x] T009 [P] [US1] Create `crates/compression/Cargo.toml` with workspace inheritance (name = "compression") and `crates/compression/src/lib.rs` with placeholder module doc and one passing test
- [x] T010 [P] [US1] Create `crates/hooks/Cargo.toml` with workspace inheritance (name = "hooks") and `crates/hooks/src/main.rs` with placeholder main function and one passing test
- [x] T011 [P] [US1] Create `crates/cli/Cargo.toml` with workspace inheritance (name = "cli") and `crates/cli/src/main.rs` with placeholder main function and one passing test
- [x] T012 [P] [US1] Create `crates/opencode/Cargo.toml` with workspace inheritance (name = "opencode") and `crates/opencode/src/lib.rs` with placeholder module doc and one passing test
- [x] T013 [US1] Verify full workspace build: run `cargo build --workspace` and confirm zero errors
- [x] T014 [US1] Verify full test suite: run `cargo test --workspace` and confirm 7+ passing tests (at least one per crate)
- [x] T015 [US1] Verify individual crate isolation: run `cargo build -p <name>` and `cargo test -p <name>` for each of the 7 crates independently

**Checkpoint**: User Story 1 complete. Fresh clone → build → test succeeds. All 7 crates compile independently and together.

---

## Phase 4: User Story 2 — CI Validates Every Push (Priority: P2)

**Goal**: A CI pipeline on GitHub Actions runs 4 quality gates (fmt, clippy, test, release build) on every push and PR, with Cargo caching, completing within 10 minutes.

**Independent Test**: Push a commit to a feature branch and verify the CI workflow runs all 4 gates. Push a commit with a formatting violation and verify CI fails with a clear error.

### Implementation for User Story 2

- [x] T016 [US2] Create `.github/workflows/ci.yml` (and parent directories) per contracts/ci.yml: triggers on push (all branches) and pull_request; steps: checkout, dtolnay/rust-toolchain@stable, Swatinem/rust-cache@v2, then 4 gates: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo build --workspace --release`
- [x] T017 [US2] Verify CI pipeline runs locally: execute all 4 quality gate commands sequentially and confirm they pass

**Checkpoint**: User Story 2 complete. CI pipeline defined and verified locally. Will be fully validated on first push to GitHub.

---

## Phase 5: User Story 3 — Developer Adds a Shared Dependency (Priority: P3)

**Goal**: Workspace dependency inheritance works correctly — deps declared once at root, crates reference with `{ workspace = true }`, no version duplication.

**Independent Test**: Inspect workspace `Cargo.toml` for all foundational dependencies. Verify at least one crate's `Cargo.toml` uses `{ workspace = true }` inheritance. Run `cargo tree` and confirm no version duplication.

### Implementation for User Story 3

- [x] T018 [US3] Add workspace dependency references to crate Cargo.toml files where applicable: add `serde = { workspace = true }` to `crates/types/Cargo.toml`, add `clap = { workspace = true }` to `crates/cli/Cargo.toml`, add `clap = { workspace = true }` to `crates/hooks/Cargo.toml` (at minimum — demonstrating the inheritance pattern)
- [x] T019 [US3] Verify dependency inheritance: run `cargo tree --workspace` and confirm shared dependencies resolve to a single version with no duplication
- [x] T020 [US3] Verify memvid-core dependency resolves: run `cargo build -p core` after adding `memvid-core = { workspace = true }` to `crates/core/Cargo.toml` and confirm git dependency fetches and compiles

**Checkpoint**: User Story 3 complete. Workspace dependency management is proven and documented.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation and final verification across all user stories.

- [x] T021 Create `README.md` at repository root: project purpose (Rust rewrite of agent-brain), build instructions (`cargo build --workspace`, `cargo test --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`), crate layout table (7 crates with types and descriptions), MSRV policy (1.85.0+, edition 2024), and how to add new crates/dependencies
- [x] T022 Validate quickstart flow: follow `specs/001-project-bootstrap/quickstart.md` step by step on the built workspace and confirm all commands succeed
- [x] T023 Final verification against all success criteria: SC-001 (clone→build), SC-002 (7 crates independent), SC-003 (7+ tests), SC-004 (CI timing — estimated from local run), SC-005 (clone→test under 5 minutes with README)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 (T001-T003 must complete before T004-T005)
- **User Story 1 (Phase 3)**: Depends on Phase 2 (workspace deps must exist before crates can inherit)
- **User Story 2 (Phase 4)**: Depends on Phase 3 (CI needs crates to exist to run gates)
- **User Story 3 (Phase 5)**: Depends on Phase 3 (needs crates to exist to add dep references)
- **Polish (Phase 6)**: Depends on Phases 4-5

### User Story Dependencies

- **User Story 1 (P1)**: Depends on Foundational only — no other story dependencies
- **User Story 2 (P2)**: Depends on US1 completion (CI validates the built workspace)
- **User Story 3 (P3)**: Depends on US1 completion (needs crates to demonstrate inheritance)
- **US2 and US3 are independent of each other** — can run in parallel after US1

### Within Each User Story

- Crate creation tasks (T006-T012) are all parallel — different directories, no shared files
- Verification tasks (T013-T015) must follow crate creation
- CI creation (T016-T017) must follow workspace build verification
- Dependency inheritance (T018-T020) must follow crate creation

### Parallel Opportunities

- **Phase 1**: T001-T003 are sequential (all modify `Cargo.toml`)
- **Phase 2**: T004-T005 are sequential (both modify `Cargo.toml`)
- **Phase 3**: T006-T012 are ALL parallel (7 crates, 7 different directories)
- **Phase 4+5**: US2 and US3 can run in parallel after US1 completes
- **Phase 6**: T021 can run in parallel with T022-T023 (README vs. verification)

---

## Parallel Example: User Story 1

```text
# Launch all 7 crate scaffolding tasks together (all [P]):
Agent 1: T006 "Create crates/types/ with Cargo.toml and src/lib.rs"
Agent 2: T007 "Create crates/core/ with Cargo.toml and src/lib.rs"
Agent 3: T008 "Create crates/platforms/ with Cargo.toml and src/lib.rs"
Agent 4: T009 "Create crates/compression/ with Cargo.toml and src/lib.rs"
Agent 5: T010 "Create crates/hooks/ with Cargo.toml and src/main.rs"
Agent 6: T011 "Create crates/cli/ with Cargo.toml and src/main.rs"
Agent 7: T012 "Create crates/opencode/ with Cargo.toml and src/lib.rs"

# Then verify sequentially:
T013: cargo build --workspace
T014: cargo test --workspace
T015: cargo build/test -p <each crate>
```

## Parallel Example: User Stories 2 & 3

```text
# After US1 is complete, run US2 and US3 in parallel:
Agent A: T016-T017 "CI pipeline setup and verification"
Agent B: T018-T020 "Dependency inheritance setup and verification"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T003a)
2. Complete Phase 2: Foundational (T004-T005)
3. Complete Phase 3: User Story 1 (T006-T015)
4. **STOP and VALIDATE**: `cargo build --workspace && cargo test --workspace` passes
5. Workspace is buildable — all subsequent work is unblocked

### Incremental Delivery

1. Setup + Foundational → Workspace manifest ready
2. Add User Story 1 → 7 crates build and test (MVP!)
3. Add User Story 2 → CI enforces quality on every push
4. Add User Story 3 → Dependency inheritance proven
5. Polish → README, quickstart validation, success criteria verification

### Single Developer Strategy

1. T001-T005 sequentially (all modify Cargo.toml)
2. T006-T012 in parallel (7 independent crates — use Agent tool)
3. T013-T015 sequentially (verification)
4. T016-T017 then T018-T020 (or parallel if using agents)
5. T021-T023 to finish

---

## Notes

- [P] tasks = different files, no dependencies on incomplete tasks
- [Story] label maps task to specific user story for traceability
- All crate Cargo.toml files inherit from workspace (edition, rust-version, lints)
- Placeholder tests: `#[test] fn it_works() { assert!(true); }` for libraries, similar for binaries
- Binary crates (hooks, cli) use `main.rs`; library crates use `lib.rs`
- memvid-core is the crate name (not memvid) — from research.md R-001
- Constitution requires: test-first, no unsafe, no silent failures, contract-first
