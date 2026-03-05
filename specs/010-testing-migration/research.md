# Research: Testing & Quality + Migration & Backwards Compatibility

**Feature Branch**: `010-testing-migration`
**Date**: 2026-03-04

## Decision 1: Fuzz Testing Framework

**Decision**: Use `cargo-fuzz` with `libFuzzer` backend.
**Rationale**: `cargo-fuzz` is the de facto standard for Rust fuzzing, integrates with Rust's `#[no_mangle]` FFI and `libFuzzer`. It supports corpus persistence, crash minimization, and coverage-guided mutations. Already listed in workspace dependencies context (`criterion` for benchmarks suggests familiarity with Rust tooling).
**Alternatives considered**:
- `afl.rs` — Requires separate AFL installation, more complex setup, less CI-friendly.
- `proptest` — Property-based testing, not true fuzzing; better for structured input exploration but doesn't provide crash-driven mutation.
- `honggfuzz-rs` — Capable but less ecosystem support than `cargo-fuzz`.

## Decision 2: TypeScript Compatibility Fixture Strategy

**Decision**: Pre-generate `.mv2` fixture files from the TypeScript agent-brain and commit them to `tests/fixtures/` with accompanying `expected_results.json` files containing reference search results, timeline output, and stats.
**Rationale**: Clarification session confirmed committed fixtures for deterministic, reproducible tests. No TypeScript build dependency in CI. Fixtures should include: (1) a small `.mv2` with ~10 observations, (2) a medium `.mv2` with ~100 observations, (3) an `.mv2` with edge-case data (empty queries, unicode, long strings).
**Alternatives considered**:
- Generate on-the-fly in CI — Rejected (adds TypeScript/Node.js dependency).
- Download from artifact store — Rejected (adds network dependency and flakiness).

## Decision 3: Performance Benchmark Baseline Collection

**Decision**: Capture TypeScript baselines by running the TypeScript agent-brain against the same fixture workloads on the same hardware, recording results in `tests/fixtures/ts_baselines.json`. CI compares Rust results against these baselines using the 2× threshold from clarification.
**Rationale**: Apples-to-apples comparison requires identical workloads and hardware. Pre-captured baselines in JSON make CI comparison straightforward without requiring TypeScript in the pipeline.
**Alternatives considered**:
- Run both versions in CI — Rejected (too complex, requires Node.js + Rust in same workflow).
- Use published benchmarks — Rejected (hardware-dependent, not reproducible).

## Decision 4: Concurrency Test Approach

**Decision**: Use `std::thread` with `Arc<Barrier>` for synchronized parallel writer tests. Test with 4, 8, and 16 concurrent writers. Use `tempfile` for isolated test directories. Validate with SHA-256 checksums on written data.
**Rationale**: Existing `crates/core/tests/integration/mind_concurrent.rs` already has concurrency test infrastructure. Extend it with more writers and add stale lock recovery scenarios (5-second timeout per clarification). `std::thread` is simpler than tokio for CPU-bound lock-contention tests.
**Alternatives considered**:
- tokio tasks — Overkill for lock-contention testing; introduces async complexity unnecessarily.
- `loom` — Excellent for atomic correctness but too specialized for file-lock scenarios.

## Decision 5: Legacy Path Detection Implementation

**Decision**: Add a `detect_legacy_path()` function to `crates/hooks/src/bootstrap.rs` that checks for `.claude/mind.mv2` relative to the project root. Return a structured warning (not an error) via the existing diagnostic system. Detection happens at startup in both hook entry points.
**Rationale**: Minimal code change. Uses existing diagnostic infrastructure (`types::Diagnostic`). Non-blocking — users see a suggestion but the system continues working.
**Alternatives considered**:
- Automatic migration — Explicitly out of scope per spec.
- Separate CLI command — Over-engineered for a deprecation notice.

## Decision 6: CI Pipeline Enhancement

**Decision**: Extend existing `.github/workflows/ci.yml` with benchmark regression gate and fuzz smoke test step. Benchmarks use `criterion` (already in workspace). Fuzz tests run for 60 seconds per harness. Use GitHub Actions `continue-on-error: false` for benchmark regression.
**Rationale**: Existing CI already runs `cargo test`, `clippy`, `fmt`. Adding benchmark and fuzz steps is incremental. 60-second fuzz duration per clarification keeps CI under 10 minutes total.
**Alternatives considered**:
- Separate workflow for benchmarks — Unnecessary fragmentation; single CI file is simpler.
- Nightly-only fuzz — Deferred; 60-second smoke in CI is the minimum per spec.

## Decision 7: Test Organization Pattern

**Decision**: Follow the existing crate-local pattern: unit tests in `src/*.rs` modules (via `#[cfg(test)] mod tests`), integration tests in `crates/<name>/tests/*.rs`. New compatibility tests go in `crates/core/tests/compatibility/`. New fuzz harnesses go in `crates/<name>/fuzz/`.
**Rationale**: Consistent with existing project structure. Constitution Principle XII (Simplicity) says to extend existing harnesses before introducing new frameworks.
**Alternatives considered**:
- Top-level `tests/` directory — Would break crate-local convention established across all 7 crates.
- Separate test crate — Over-engineered for this scope.
