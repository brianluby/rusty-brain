# Architecture Remediation Plan (ADR-Style)

## Scope and Intent
- This plan operationalizes findings from `arch_review_findings.md` into architectural decisions and implementation tickets.
- Goal: reduce architectural drift, remove over-engineering where not justified, and improve scalability/maintainability without breaking existing plugin behavior.

## Assumptions
- Rust stable, edition 2024, MSRV 1.85.0 remains unchanged.
- Existing quality gates remain mandatory (`cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`).
- Fail-open behavior in hooks/opencode is non-negotiable.

---

## ADR-001: Canonical Memory Path Policy
**Status:** Proposed

### Context
- Path policy is currently split between `types::MindConfig::from_env` and `platforms::resolve_memory_path`, and they resolve different opt-in paths.
- Runtime entrypoints (CLI vs hooks/opencode) can read/write different files for the same project.

### Decision
- Make `platforms::resolve_memory_path(...)` the single source of truth for all runtime path resolution.
- Remove path-construction behavior from `MindConfig::from_env`; retain only non-path config parsing there.

### Consequences
- Positive: consistency across all entrypoints and no hidden memory splits.
- Negative: requires call-site refactor and migration notes for existing users.

### Validation Plan
- Add integration tests proving CLI and hooks/opencode resolve the same target path under identical env/project conditions.

---

## ADR-002: Right-Size Platform Abstraction (Use It or Remove It)
**Status:** Proposed

### Context
- `EventPipeline` and `AdapterRegistry` exist but are not in runtime ingress flow.
- Direct helper usage dominates runtime code paths.

### Decision
- Adopt pipeline-based ingress for runtime hooks/opencode rather than deleting now.
- If adoption cannot be completed in one milestone, explicitly mark unused components as deferred and block new abstractions until integrated.

### Consequences
- Positive: architecture intent matches implementation; better extension path for additional platforms.
- Negative: short-term implementation overhead.

### Validation Plan
- Runtime tests asserting normalized event -> pipeline process -> handler execution path for at least Claude and OpenCode.

---

## ADR-003: Shared Runtime Bootstrap for Handlers
**Status:** Proposed

### Context
- Platform opt-in/env parsing and Mind open/resolve logic are duplicated across handlers.

### Decision
- Introduce a shared bootstrap module for:
  - platform policy resolution
  - path resolution
  - read-only/read-write `Mind` open helpers
- Replace duplicated per-handler utility functions.

### Consequences
- Positive: fewer drift points and easier policy changes.
- Negative: one additional shared module boundary.

### Validation Plan
- Unit tests for bootstrap helpers + regression tests for all handler entrypoints.

---

## ADR-004: Decompose `Mind` Responsibilities
**Status:** Proposed

### Context
- `crates/core/src/mind.rs` currently mixes lifecycle/recovery, locking, read/write APIs, stats and mapping logic.

### Decision
- Split into focused modules/services while preserving public `Mind` API surface where practical.

### Consequences
- Positive: lower cognitive load, safer changes, clearer ownership.
- Negative: refactor churn and temporary code movement overhead.

### Validation Plan
- No behavior changes expected; enforce through existing core integration tests plus added module-level tests.

---

## ADR-005: Keep Query Paths Lightweight (Decouple from Full Stats Scan)
**Status:** Proposed

### Context
- CLI type-filtered find/timeline currently call `stats()` to compute fetch sizing; `stats()` is heavy for large stores.

### Decision
- Remove `stats()` dependency from interactive query paths.
- Prefer bounded over-fetch + local filter, and consider backend-level typed filtering as a follow-up optimization.

### Consequences
- Positive: lower latency and improved scalability.
- Negative: may return slightly less-than-optimal filtered result density without backend filter support.

### Validation Plan
- Add perf regression tests and compare P95 for `find --type` / `timeline --type` before and after.

---

## ADR-006: Test Placement Hygiene
**Status:** Proposed

### Context
- Several production files are large enough to hinder reviewability due to extensive inline tests.

### Decision
- Move broad scenario/contract tests to `tests/` where possible.
- Keep only tight unit tests near complex implementation details.

### Consequences
- Positive: production code becomes easier to navigate/review.
- Negative: more files to traverse during test debugging.

### Validation Plan
- Ensure test coverage parity by tracking moved test counts and preserving assertions.

---

## Implementation Tickets

## Phase 1: Correctness-Critical Alignment

- [ ] **RB-ARCH-001 (P0, M)** Unify memory path resolution authority
  - **Goal:** route CLI path resolution through `platforms::resolve_memory_path`.
  - **Files:** `crates/cli/src/main.rs`, `crates/types/src/config.rs`, shared bootstrap module (new, location TBD).
  - **Acceptance:** same resolved path for CLI and hooks/opencode under equivalent env; regression tests added.
  - **Depends on:** none.

- [ ] **RB-ARCH-002 (P0, S)** Add migration and compatibility notes for path policy unification
  - **Goal:** avoid user confusion during transition.
  - **Files:** `README.md`, potentially hook startup messaging path guidance.
  - **Acceptance:** docs clearly state canonical path policy and legacy behavior.
  - **Depends on:** RB-ARCH-001.

## Phase 2: Remove Architectural Drift

- [ ] **RB-ARCH-003 (P1, M)** Introduce shared handler bootstrap utilities
  - **Goal:** replace duplicated `platform_opt_in()` and repeated resolve/open patterns.
  - **Files:** new shared module under `crates/hooks` and/or `crates/opencode`; all handler files in both crates.
  - **Acceptance:** duplicated bootstrap logic removed from all 7 current call sites.
  - **Depends on:** RB-ARCH-001.

- [ ] **RB-ARCH-004 (P1, M)** Integrate pipeline into runtime ingress
  - **Goal:** feed normalized events through adapter + pipeline before handler behavior.
  - **Files:** `crates/hooks/src/main.rs`, `crates/hooks/src/*`, `crates/opencode/src/*`, `crates/platforms/src/*` (if needed).
  - **Acceptance:** runtime tests prove pipeline decisions affect processing path.
  - **Depends on:** RB-ARCH-003.

- [ ] **RB-ARCH-005 (P1, S)** Remove dead identity resolution in session start
  - **Goal:** eliminate no-op `_identity` calculation unless wired into actual behavior.
  - **Files:** `crates/hooks/src/session_start.rs`.
  - **Acceptance:** no unused identity computation remains.
  - **Depends on:** RB-ARCH-004 (or can be done immediately if decision is to defer pipeline).

## Phase 3: Scalability and Maintainability

- [x] **RB-ARCH-006 (P1, M)** Remove `stats()` dependency from filtered CLI query flow
  - **Goal:** make `find --type` and `timeline --type` avoid full stats scan.
  - **Files:** `crates/cli/src/commands.rs`, possibly `crates/core/src/mind.rs` API extension.
  - **Acceptance:** no `stats()` call in filtered query flow; all tests pass.
  - **Depends on:** none.

- [x] **RB-ARCH-007 (P2, L)** Decompose `Mind` internals into focused modules
  - **Goal:** split lifecycle/locking/read/write/stats concerns.
  - **Files:** `crates/core/src/mind.rs` -> multiple modules.
  - **Acceptance:** API behavior preserved, integration tests unchanged/green.
  - **Depends on:** RB-ARCH-006 preferred first.

- [x] **RB-ARCH-008 (P2, M)** Evaluate and decide fate of `get_mind/reset_mind` singleton API
  - **Goal:** remove if internal-only and unused; otherwise document lifecycle contract.
  - **Files:** `crates/core/src/lib.rs`, docs.
  - **Acceptance:** explicit decision captured and code/docs aligned.
  - **Depends on:** RB-ARCH-007.

## Phase 4: Codebase Hygiene

- [x] **RB-ARCH-009 (P2, M)** Move large scenario tests out of source modules
  - **Goal:** reduce production-file size and mixed concerns.
  - **Files:** `crates/types/src/config.rs`, `crates/types/src/error.rs`, `crates/platforms/src/pipeline.rs`, others as needed.
  - **Acceptance:** source files materially reduced; coverage parity maintained.
  - **Depends on:** none.

- [x] **RB-ARCH-010 (P3, S)** Consolidate legacy path messaging
  - **Goal:** one source of truth for migration warnings.
  - **Files:** `crates/hooks/src/session_start.rs`, docs/policy module.
  - **Acceptance:** startup message aligns with canonical policy language.
  - **Depends on:** RB-ARCH-001.

---

## Milestone Validation Checklist
- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Smoke-check CLI + hooks/opencode memory path parity in same repo/env
- [ ] Regression-check fail-open behavior (error/panic paths still emit valid JSON outputs)

## Recommended Execution Order
1. RB-ARCH-001, RB-ARCH-002
2. RB-ARCH-003, RB-ARCH-004, RB-ARCH-005
3. RB-ARCH-006, RB-ARCH-007, RB-ARCH-008
4. RB-ARCH-009, RB-ARCH-010
