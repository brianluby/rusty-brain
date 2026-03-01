# Implementation Plan: Project Bootstrap

**Branch**: `001-project-bootstrap` | **Date**: 2026-03-01 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `specs/001-project-bootstrap/spec.md`

## Summary

Establish the Rust workspace, CI pipeline, and foundational crates for the agent-brain Rust rewrite. This is Phase 0 from the roadmap — a buildable, testable, CI-enforced scaffold with 7 crates (`core`, `types`, `platforms`, `compression`, `hooks`, `cli`, `opencode`) and workspace-level dependency management. No runtime functionality; placeholder tests only.

## Technical Context

**Language/Version**: Rust, edition 2024, MSRV 1.85.0 (current stable: 1.93.1)
**Primary Dependencies**: memvid-core (git), serde/serde_json, thiserror, tokio, tracing, chrono, uuid, clap, semver
**Storage**: N/A (Phase 0 — no runtime storage)
**Testing**: `cargo test` (built-in test harness, placeholder tests per crate)
**Target Platform**: Linux x86_64, macOS x86_64/aarch64 (CI: ubuntu-latest)
**Project Type**: Cargo workspace with 7 member crates
**Performance Goals**: CI pipeline completes all 4 quality gates within 10 minutes (with caching)
**Constraints**: Clean build from fresh clone must succeed; individual crate isolation required
**Scale/Scope**: 7 crates, ~50 files total (scaffolding only)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Crate-First Architecture | PASS | 7 crates defined per roadmap layout |
| II. Rust-First Implementation | PASS | Stable Rust, edition 2024, no unsafe |
| III. Agent-Friendly Interface Design | N/A | No runtime interfaces in Phase 0 |
| IV. Contract-First Development | PASS | Workspace structure is the contract; contracts/ generated below |
| V. Test-First Development | PASS | Placeholder tests per crate required before Phase 0 is complete |
| VI. Complete Requirement Delivery | PASS | All FR-001 through FR-011 mapped to implementation tasks |
| VII. Memory Integrity and Data Safety | N/A | No runtime storage in Phase 0 |
| VIII. Performance and Scope Discipline | PASS | CI timing target measurable; no speculative perf work |
| IX. Security-First Design | PASS | No secrets, no network, no sensitive data in Phase 0 |
| X. Error Handling Standards | N/A | No runtime error handling in Phase 0 |
| XI. Observability and Debuggability | N/A | No runtime behavior in Phase 0 |
| XII. Simplicity and Pragmatism | PASS | Minimal scaffold — no unnecessary abstractions |
| XIII. Dependency Policy | PASS | memvid pinned to git rev; all deps from roadmap, workspace-managed |

**Gate result**: PASS — no violations.

## Project Structure

### Documentation (this feature)

```text
specs/001-project-bootstrap/
├── spec.md              # Feature specification (completed)
├── plan.md              # This file
├── research.md          # Phase 0 research output (completed)
├── data-model.md        # Phase 1 data model (minimal for scaffolding)
├── quickstart.md        # Developer quickstart guide
├── contracts/           # Workspace and CI contracts
│   ├── workspace.toml   # Root Cargo.toml contract
│   └── ci.yml           # CI pipeline contract
└── checklists/
    └── requirements.md  # Requirements checklist (completed)
```

### Source Code (repository root)

```text
Cargo.toml                    # Virtual workspace manifest
Cargo.lock                    # Lockfile (auto-generated)
rust-toolchain.toml           # Toolchain pinning for MSRV enforcement
README.md                     # Project overview, build instructions, crate layout
.github/
└── workflows/
    └── ci.yml                # CI pipeline: fmt, clippy, test, release build

crates/
├── core/                     # Memory engine (library crate)
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs            # Placeholder with one test
├── types/                    # Shared types & errors (library crate)
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs
├── platforms/                # Platform adapter system (library crate)
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs
├── compression/              # Tool-output compression (library crate)
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs
├── hooks/                    # Claude Code hook binaries (binary crate)
│   ├── Cargo.toml
│   └── src/
│       └── main.rs           # Placeholder with one test
├── cli/                      # CLI scripts (binary crate)
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
└── opencode/                 # OpenCode editor adapter (library crate)
    ├── Cargo.toml
    └── src/
        └── lib.rs
```

**Structure Decision**: Cargo virtual workspace with all crates under `crates/`. Binary crates (`hooks`, `cli`) use `main.rs`; library crates use `lib.rs`. Matches roadmap layout exactly.

## Design Decisions

### D-001: Virtual Workspace Manifest

The root `Cargo.toml` uses `[workspace]` without `[package]` (virtual manifest). This avoids polluting the root with `src/` and clearly signals the root is configuration-only.

- `resolver = "3"` is explicitly set (required for virtual workspaces in edition 2024).
- `members = ["crates/*"]` uses glob to auto-discover crates.

### D-002: Workspace Dependency Inheritance

All foundational dependencies declared in `[workspace.dependencies]`. Member crates inherit via `{ workspace = true }` syntax. This ensures:
- Single version source for all shared deps
- No version drift between crates
- Easy upgrades (one place to change)

### D-003: Workspace Metadata & Lints Inheritance

Shared metadata (`edition`, `rust-version`, `license`, `repository`) in `[workspace.package]`. Shared lints in `[workspace.lints.clippy]` and `[workspace.lints.rust]`. Member crates inherit both.

### D-004: memvid Git Dependency Pinning

```toml
[workspace.dependencies]
memvid-core = { git = "https://github.com/brianluby/memvid/", rev = "fbddef4bff6ac756f91724681234243e98d5ba04" }
```

Pinned to commit SHA for reproducibility. No tags available in the repository. The crate name is `memvid-core` (not `memvid`).

### D-005: CI Pipeline with Caching

GitHub Actions workflow using:
- `dtolnay/rust-toolchain@stable` for toolchain setup
- `Swatinem/rust-cache@v2` for Cargo registry and build artifact caching
- 4 quality gates as separate steps: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo build --workspace --release`
- Triggered on push to any branch + pull requests

### D-006: Placeholder Test Strategy

Each crate gets exactly one test in its entry point file:
- Library crates: `#[cfg(test)] mod tests { #[test] fn it_works() { ... } }`
- Binary crates: test in `main.rs` or a `tests/` directory

This satisfies SC-003 (minimum 7 passing tests). Constitution Principle V (Test-First) requires tests before implementation for *new behavior*; Phase 0 scaffolding has no runtime behavior, so placeholder tests created alongside code are sufficient. Behavioral test-first discipline applies starting in Phase 1.

## Implementation Phases

### Phase A: Workspace Root (FR-001, FR-004, FR-006, FR-007, FR-011)

1. Create root `Cargo.toml` with virtual workspace manifest
2. Configure `[workspace.package]` with shared metadata
3. Configure `[workspace.dependencies]` with all foundational deps
4. Configure `[workspace.lints]` with clippy/rust rules
5. Create `rust-toolchain.toml` for MSRV enforcement

### Phase B: Crate Scaffolding (FR-002, FR-003, FR-005)

1. Create all 7 crate directories under `crates/`
2. Create each crate's `Cargo.toml` with workspace inheritance
3. Create entry point files (`lib.rs` or `main.rs`) with placeholder code and test
4. Verify `cargo build --workspace` succeeds
5. Verify `cargo test --workspace` passes (7+ tests)

### Phase C: CI Pipeline (FR-008, FR-009)

1. Create `.github/workflows/ci.yml`
2. Configure 4 quality gates
3. Configure Cargo caching with `Swatinem/rust-cache@v2`
4. Configure triggers (push + pull_request)

### Phase D: Documentation (FR-010)

1. Create `README.md` with project purpose
2. Document build and test instructions
3. Document crate layout and responsibilities
4. Document MSRV and edition policy

### Phase E: Verification (SC-001 through SC-005)

1. Verify fresh clone → build succeeds (SC-001)
2. Verify all 7 crates compile independently (SC-002)
3. Verify 7+ tests pass (SC-003)
4. Push and verify CI completes within 10 minutes (SC-004)
5. Verify README instructions are sufficient for clone-to-test (SC-005)

## Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|------------|
| memvid-core git dependency has breaking API changes | Build fails | Pinned to specific commit SHA; upgrade is explicit |
| memvid-core Cargo.toml has dependency conflicts | Build fails | Research confirmed compatible versions; verify with `cargo tree` |
| CI caching misses → exceeds 10-minute target | SC-004 fails | Swatinem/rust-cache handles this reliably; fallback: split into parallel jobs |
| Edition 2024 breaks unsafe patterns | Build fails | No unsafe code in Phase 0; only placeholder code |

## Complexity Tracking

No constitution violations to justify. All decisions are straightforward and align with documented requirements and roadmap.
