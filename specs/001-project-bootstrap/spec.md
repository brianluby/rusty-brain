# Feature Specification: Project Bootstrap

**Feature Branch**: `001-project-bootstrap`
**Created**: 2026-03-01
**Status**: Draft
**Input**: User description: "Phase 0 from RUST_ROADMAP.md — Establish the Rust workspace, CI, and foundational crates for the agent-brain Rust rewrite."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Developer Builds the Project (Priority: P1)

A developer clones the repository and runs a single build command. The project compiles successfully with all crates resolving their workspace dependencies. The developer can immediately begin working on any crate in the workspace.

**Why this priority**: Without a buildable workspace, no other development work can proceed. This is the foundational deliverable that unblocks all subsequent phases.

**Independent Test**: Can be fully tested by running the build command on a fresh clone and verifying zero errors. Delivers a compilable workspace that unblocks all subsequent phases.

**Acceptance Scenarios**:

1. **Given** a fresh clone of the repository, **When** the developer runs the build command, **Then** all workspace crates compile without errors.
2. **Given** the workspace is built, **When** the developer runs tests, **Then** all placeholder tests pass and the test harness is functional.
3. **Given** any individual crate in the workspace, **When** the developer builds or tests that crate in isolation, **Then** it compiles and tests independently without requiring the full workspace build first.

---

### User Story 2 - CI Validates Every Push (Priority: P2)

A contributor pushes a commit or opens a pull request. The CI pipeline automatically runs formatting checks, linting, tests, and a release build. The contributor receives clear pass/fail feedback before code review begins.

**Why this priority**: CI enforcement ensures quality from the start and prevents regressions as the codebase grows. Establishing it in Phase 0 means every subsequent phase benefits automatically.

**Independent Test**: Can be tested by pushing a commit to a feature branch and verifying that the CI pipeline runs all four quality gates and reports results on the pull request.

**Acceptance Scenarios**:

1. **Given** a commit is pushed to any branch, **When** CI runs, **Then** formatting, linting, testing, and release build steps all execute.
2. **Given** a commit with a formatting violation, **When** CI runs, **Then** the pipeline fails with a clear error identifying the violation.
3. **Given** a commit with a lint warning, **When** CI runs, **Then** the pipeline fails and reports the specific warning.
4. **Given** all checks pass, **When** CI completes, **Then** the contributor sees a green status within 10 minutes.

---

### User Story 3 - Developer Adds a Shared Dependency (Priority: P3)

A developer working on a future phase needs to add a dependency used by multiple crates. They add it once at the workspace level and all crates that need it reference it without version duplication. Workspace-level dependency management ensures consistency across crates.

**Why this priority**: Proper workspace dependency setup prevents version drift and simplifies future phases, but is lower priority because it's exercised naturally as Phase 1+ begins.

**Independent Test**: Can be tested by verifying that workspace dependencies are defined at the root level and that individual crates inherit them without specifying their own version.

**Acceptance Scenarios**:

1. **Given** a dependency is declared at the workspace level, **When** a crate references it with workspace inheritance, **Then** it resolves to the workspace-specified version.
2. **Given** the workspace dependency manifest, **When** a developer inspects it, **Then** all foundational dependencies listed in the roadmap are present with pinned versions.

---

### Edge Cases

- What happens when a developer has a toolchain older than the minimum supported version? The build should fail with a clear error indicating the required version. *Enforced by `rust-version = "1.85.0"` in workspace Cargo.toml (Cargo rejects builds on older toolchains) and `rust-toolchain.toml` (rustup auto-installs the correct channel).*
- What happens when the `memvid-core` crate is unavailable or its pinned revision is unreachable? The build should fail at dependency resolution with an identifiable error, not at compilation. *This is standard Cargo behavior for git dependencies — `cargo build` emits a clear network/revision error during dependency fetch, before any compilation begins.*
- What happens when a crate is added to the `crates/` directory but not listed in the workspace members? *Not applicable — `members = ["crates/*"]` uses glob auto-discovery, so any crate added under `crates/` is automatically included.* Documentation in README.md explains this pattern.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The project MUST use a Cargo workspace with a root `Cargo.toml` containing all member crates.
- **FR-002**: The workspace MUST include the following crates as members: `core`, `types`, `platforms`, `compression`, `hooks`, `cli`, and `opencode`, each under a `crates/` directory.
- **FR-003**: Each crate MUST contain a valid `Cargo.toml` and entry point (`lib.rs` for libraries, `main.rs` for binaries) with at least one passing placeholder test.
- **FR-004**: The workspace MUST declare shared dependencies at the root level so individual crates inherit them via workspace inheritance syntax.
- **FR-005**: The workspace MUST pin the `memvid-core` crate as a git dependency (`https://github.com/brianluby/memvid/`) to a specific revision or tag in the root `Cargo.toml`.
- **FR-006**: The project MUST use Rust edition 2024.
- **FR-007**: The project MUST define and enforce a minimum supported Rust version (MSRV) via `rust-version` in `Cargo.toml`.
- **FR-008**: The project MUST include a CI pipeline that runs four quality gates: format check, lint check (warnings denied), test suite, and release build. The pipeline MUST cache Cargo registry and build artifacts to meet timing targets.
- **FR-009**: The CI pipeline MUST run on every push to any branch and on every pull request.
- **FR-010**: The project MUST include a `README.md` describing the project purpose, how to build and test, and the crate layout.
- **FR-011**: The foundational dependencies from the roadmap MUST be declared at the workspace level: `memvid-core`, `serde`/`serde_json`, `thiserror`, `tokio`, `tracing`, `chrono`, `uuid`, `clap`, and `semver`.

### Key Entities

- **Workspace**: The root-level configuration that ties all crates together, manages shared dependencies, and defines build profiles.
- **Crate**: An individual package within the workspace, responsible for a specific domain (types, core engine, platform adapters, compression, hooks, CLI, OpenCode editor adapter).
- **CI Pipeline**: The automated quality gate that validates every code change against formatting, linting, testing, and build criteria.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can clone the repository and successfully build the entire workspace in a single command with zero errors.
- **SC-002**: All 7 workspace crates compile independently and as part of the full workspace build.
- **SC-003**: The test suite runs and passes with at least one test per crate (minimum 7 passing tests).
- **SC-004**: The CI pipeline completes all four quality gates within 10 minutes on a GitHub Actions ubuntu-latest runner.
- **SC-005**: A new contributor can go from clone to passing tests in under 5 minutes using only the README instructions.

## Clarifications

### Session 2026-03-01

- Q: How should the workspace source the `memvid` dependency? → A: Git dependency from `https://github.com/brianluby/memvid/` (rev/tag pinned).
- Q: What is the purpose of the `opencode` crate? → A: Adapter library for OpenCode editor integration.
- Q: Should the CI pipeline use Cargo caching to meet the 10-minute target? → A: Yes, cache Cargo registry and build artifacts.

## Assumptions

- The `memvid-core` crate is sourced as a git dependency from `https://github.com/brianluby/memvid/`, pinned to a specific revision or tag.
- GitHub Actions is the CI platform, consistent with the roadmap. Cargo registry and build artifact caching will be used to meet the 10-minute completion target.
- The MSRV will be the first stable Rust release supporting edition 2024 (1.85+).
- Binary crates (`hooks`, `cli`) will use `main.rs`; library crates (`core`, `types`, `platforms`, `compression`, `opencode`) will use `lib.rs`. The `opencode` crate serves as an adapter library for OpenCode editor integration.
- Placeholder tests are sufficient for Phase 0; comprehensive tests are added in subsequent phases.

## Scope Boundaries

### In Scope

- Cargo workspace initialization with all 7 crates
- Workspace-level dependency declarations for all foundational dependencies
- CI pipeline setup with four quality gates
- Rust edition 2024 and MSRV policy
- Project README with build instructions and crate layout

### Out of Scope

- Implementation of any crate's actual functionality (starts in Phase 1)
- Cross-platform release builds or binary distribution (Phase 8)
- Benchmarking or performance testing infrastructure (Phase 9)
- Any runtime behavior beyond placeholder tests passing
