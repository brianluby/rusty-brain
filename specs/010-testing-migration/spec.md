# Feature Specification: Testing & Quality + Migration & Backwards Compatibility

**Feature Branch**: `010-testing-migration`
**Created**: 2026-03-04
**Status**: Draft
**Input**: User description: "Phase 9 — Testing & Quality + Phase 10 — Migration & Backwards Compatibility"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Developer Validates Crate Quality via Test Suite (Priority: P1)

A developer working on rusty-brain runs the full test suite and receives comprehensive coverage results across all 7 workspace crates. Every public API function, trait, and type has at least one unit test. Cross-crate workflows (remember → search → getContext) are validated end-to-end. The developer has confidence that changes in one crate do not break others.

**Why this priority**: Without comprehensive test coverage, no other quality or migration guarantee can be trusted. This is the foundation for all subsequent stories.

**Independent Test**: Can be fully tested by running `cargo test` and verifying all public APIs have corresponding test cases. Delivers confidence in code correctness across all crates.

**Acceptance Scenarios**:

1. **Given** the rusty-brain workspace, **When** `cargo test` is run, **Then** all unit tests pass and every public API in each crate has at least one test exercising it
2. **Given** the core crate with a Mind instance, **When** the remember → search → getContext cycle is exercised in an integration test, **Then** stored observations are retrievable and context is correctly assembled
3. **Given** the platform adapters, **When** events from different platforms are normalized, **Then** cross-platform round-trip tests confirm data integrity through the full pipeline

---

### User Story 2 - Drop-in Replacement for TypeScript Version (Priority: P1)

A user currently using the TypeScript agent-brain switches to rusty-brain by updating `plugin.json` hook paths. Their existing `.mv2` memory files, `.agent-brain/` directory layout, and environment variable configurations continue to work without any migration steps. The Rust version reads existing `.mv2` files and produces identical search results, timeline entries, and stats.

**Why this priority**: Users cannot adopt rusty-brain unless it is a seamless replacement. Zero-migration compatibility is essential for adoption.

**Independent Test**: Can be fully tested by pointing rusty-brain at a `.mv2` file created by the TypeScript version and verifying search, timeline, and stats produce identical results. Delivers confident migration path.

**Acceptance Scenarios**:

1. **Given** a `.mv2` file written by the TypeScript agent-brain, **When** rusty-brain reads it, **Then** search queries return identical results to the TypeScript version
2. **Given** environment variables `MEMVID_PLATFORM`, `MEMVID_MIND_DEBUG`, `MEMVID_PLATFORM_MEMORY_PATH`, `MEMVID_PLATFORM_PATH_OPT_IN`, `CLAUDE_PROJECT_DIR`, and `OPENCODE_PROJECT_DIR`, **When** rusty-brain starts, **Then** it honors each variable identically to the TypeScript version
3. **Given** the existing `.agent-brain/` directory layout, **When** rusty-brain is installed, **Then** it uses the same directory structure and file locations without requiring changes

---

### User Story 3 - Concurrent Access Safety (Priority: P2)

Multiple AI agent sessions attempt to access the same `.mv2` memory file simultaneously. The system handles parallel writers, lock contention, and recovery from stale locks without data corruption or silent failures.

**Why this priority**: AI agents often run in parallel (worktree mode, multiple hooks firing). Concurrency safety is critical for real-world reliability.

**Independent Test**: Can be fully tested by spawning multiple threads/processes that simultaneously write to and read from a shared `.mv2` file, verifying no data loss or corruption.

**Acceptance Scenarios**:

1. **Given** two concurrent writers targeting the same `.mv2` file, **When** both attempt to write observations simultaneously, **Then** both writes succeed without data loss or corruption
2. **Given** a stale lock file left by a crashed process, **When** a new process starts, **Then** it detects and recovers from the stale lock within 5 seconds
3. **Given** a writer holding a lock, **When** a reader attempts concurrent access, **Then** the reader either waits or receives a clear error, never reading partially-written data

---

### User Story 4 - Performance Meets or Exceeds TypeScript Baseline (Priority: P2)

Performance benchmarks demonstrate that rusty-brain is faster than the TypeScript version on memory query latency, compression throughput, and startup time. Benchmarks are reproducible and integrated into CI.

**Why this priority**: Performance is a key value proposition of the Rust rewrite. Benchmarks prove the rewrite delivers on its promise and prevent regressions.

**Independent Test**: Can be fully tested by running `cargo bench` against known workloads and comparing results to TypeScript baseline measurements. Delivers quantified performance data.

**Acceptance Scenarios**:

1. **Given** a standard memory workload, **When** memory query latency is benchmarked, **Then** rusty-brain is faster than the TypeScript version
2. **Given** a standard compression workload, **When** compression throughput is benchmarked, **Then** rusty-brain meets or exceeds TypeScript throughput
3. **Given** a cold start, **When** startup time is measured, **Then** rusty-brain starts faster than the TypeScript version

---

### User Story 5 - Fuzz Testing Catches Malformed Inputs (Priority: P3)

Fuzz tests exercise the compression engine, hook JSON parsing, and search query parsing with random/malformed inputs. The system handles all inputs gracefully without panics, crashes, or undefined behavior.

**Why this priority**: Robustness against malformed inputs is important for production reliability but is an enhancement beyond basic correctness.

**Independent Test**: Can be fully tested by running fuzz harnesses against each input boundary and verifying no panics or crashes occur.

**Acceptance Scenarios**:

1. **Given** malformed JSON input to hook parsers, **When** fuzz-generated payloads are processed, **Then** the system returns structured errors without panicking
2. **Given** malformed input to the compression engine, **When** fuzz-generated data is compressed/decompressed, **Then** the system returns errors or safely handles the input
3. **Given** adversarial search queries, **When** fuzz-generated queries are executed, **Then** the system returns empty results or structured errors, never crashing

---

### User Story 6 - Legacy Path Detection and Migration Guidance (Priority: P3)

A user who previously stored their mind file at `.claude/mind.mv2` (the old location) receives a clear suggestion to move it to `.agent-brain/mind.mv2`. The system detects the legacy path and provides actionable guidance without automatically moving files.

**Why this priority**: Supports users migrating from older configurations, but affects a small subset of users.

**Independent Test**: Can be fully tested by creating a `.claude/mind.mv2` file and verifying the system detects it and suggests the move.

**Acceptance Scenarios**:

1. **Given** a `.claude/mind.mv2` file exists and no `.agent-brain/mind.mv2` exists, **When** rusty-brain starts, **Then** it outputs a suggestion to move the file to `.agent-brain/mind.mv2`
2. **Given** both `.claude/mind.mv2` and `.agent-brain/mind.mv2` exist, **When** rusty-brain starts, **Then** it uses `.agent-brain/mind.mv2` and warns about the duplicate

---

### Edge Cases

- What happens when a `.mv2` file is written by a newer TypeScript version with unknown fields? The system should read known fields and ignore unknown ones gracefully.
- What happens when environment variables contain invalid values (e.g., `MEMVID_PLATFORM=nonexistent`)? The system should return a clear error.
- What happens when the `.agent-brain/` directory has unexpected permissions? The system should report a meaningful error about directory access.
- What happens when a benchmark runs on a heavily loaded system? Results should include variance/deviation metrics to flag unreliable runs.
- What happens when fuzz testing finds a genuine bug? The failing input should be captured as a regression test.

## Requirements *(mandatory)*

### Functional Requirements

**Testing & Quality**

- **FR-001**: System MUST have unit tests for every public function, trait method, and type constructor across all 7 workspace crates (`cli`, `compression`, `core`, `hooks`, `opencode`, `platforms`, `types`)
- **FR-002**: System MUST have integration tests covering cross-crate workflows: remember → search → getContext cycle
- **FR-003**: System MUST have platform adapter tests verifying event normalization and cross-platform round-trips for all supported platforms
- **FR-004**: System MUST have concurrency tests exercising parallel writers, lock contention scenarios, and stale lock recovery
- **FR-005**: System MUST have performance benchmarks measuring memory query latency, compression throughput, and startup time
- **FR-006**: System MUST have compatibility tests that read `.mv2` files written by the TypeScript agent-brain and verify identical search results, timeline output, and stats
- **FR-007**: System MUST have fuzz test harnesses for compression input, hook JSON parsing, and search query parsing
- **FR-008**: CI pipeline MUST gate on: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo bench` (no regressions)

**Migration & Backwards Compatibility**

- **FR-009**: System MUST read and write `.mv2` files in the same format as the TypeScript version with zero conversion or migration steps
- **FR-010**: System MUST honor all existing environment variables: `MEMVID_PLATFORM`, `MEMVID_MIND_DEBUG`, `MEMVID_PLATFORM_MEMORY_PATH`, `MEMVID_PLATFORM_PATH_OPT_IN`, `CLAUDE_PROJECT_DIR`, `OPENCODE_PROJECT_DIR`
- **FR-011**: System MUST use the identical `.agent-brain/` directory layout as the TypeScript version
- **FR-012**: System MUST detect `.claude/mind.mv2` (legacy path) and suggest migration to `.agent-brain/mind.mv2`
- **FR-013**: System MUST produce identical search results as the TypeScript version when querying the same `.mv2` file with the same query. "Identical" means: same result set, same relevance ordering; similarity scores may differ within ±0.01 tolerance due to cross-language floating-point differences
- **FR-014**: Packaging artifacts MUST include updated `plugin.json` pointing hook paths to Rust binaries instead of `dist/hooks/*.js`

### Key Entities

- **Test Suite**: Collection of unit, integration, platform, concurrency, and compatibility tests organized per crate
- **Benchmark Suite**: Performance benchmarks with baseline comparisons against TypeScript performance data
- **Fuzz Corpus**: Collection of seed inputs and discovered crash cases for compression, JSON parsing, and search queries
- **Compatibility Fixture**: `.mv2` files generated by the TypeScript version used as golden test data
- **TypeScript Baseline**: Reference measurements (search results, performance timings) from the TypeScript agent-brain for comparison

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All public APIs across 7 crates have at least one corresponding test; overall test count meets or exceeds 48 (TypeScript parity)
- **SC-002**: Cross-crate integration tests cover the full remember → search → getContext cycle and pass reliably
- **SC-003**: Concurrency tests with 4+ parallel writers complete without data loss or corruption in 100% of runs
- **SC-004**: Performance benchmarks show rusty-brain is at least 2× faster than TypeScript on memory query latency, compression throughput, and startup time; CI gate fails below this threshold
- **SC-005**: Compatibility tests confirm identical search results, timeline output, and stats when reading TypeScript-generated `.mv2` files
- **SC-006**: Fuzz tests run for a minimum of 60 seconds per harness in CI without discovering panics or crashes; deeper overnight runs may be scheduled separately
- **SC-007**: CI gates (`fmt`, `clippy`, `test`, `bench`) all pass on every merge to main
- **SC-008**: A user can switch from TypeScript to Rust by only updating `plugin.json` hook paths — no `.mv2` file migration, no directory changes, no environment variable changes required
- **SC-009**: Legacy `.claude/mind.mv2` path is detected and migration guidance is displayed to the user

## Assumptions

- TypeScript agent-brain `.mv2` files are pre-generated and committed to `tests/fixtures/` in the repo for deterministic, reproducible tests with no TypeScript build dependency
- The TypeScript version's search/timeline/stats behavior is considered the reference specification for compatibility
- Performance baselines from the TypeScript version will be measured on the same hardware used for Rust benchmarks
- Fuzz testing uses `cargo-fuzz` or equivalent Rust fuzzing infrastructure
- CI runs on GitHub Actions (existing infrastructure from 009-plugin-packaging)
- The `.mv2` format is stable and will not change between the TypeScript version being replaced and this release

## Clarifications

### Session 2026-03-04

- Q: What level of equivalence defines "identical search results" (FR-013, SC-005)? → A: Same result set and ordering; scores may differ within ±0.01 tolerance
- Q: What minimum speedup factor should benchmarks enforce (SC-004)? → A: At least 2× faster on each metric; CI gate fails below this
- Q: What minimum fuzz duration should CI enforce per harness (SC-006)? → A: 60 seconds per harness
- Q: How should TypeScript `.mv2` test fixtures be sourced? → A: Pre-generated and committed to `tests/fixtures/` in the repo
- Q: What maximum timeout for stale lock recovery (SC-003, US3)? → A: 5 seconds

## Scope & Boundaries

**In scope**:
- Unit, integration, platform, concurrency, compatibility, fuzz tests
- Performance benchmarks with TypeScript baselines
- CI gate enforcement
- `.mv2` format compatibility (read/write)
- Environment variable compatibility
- Directory layout compatibility
- Legacy path detection
- `plugin.json` update for Rust binary paths

**Out of scope**:
- Automatic migration tools that move files for the user
- Support for `.mv2` format versions older than current TypeScript release
- Remote/networked memory storage
- GUI or interactive migration wizard
- Changes to the `.mv2` format itself
