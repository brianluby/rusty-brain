# Quickstart: Testing & Quality + Migration & Backwards Compatibility

**Feature Branch**: `010-testing-migration`
**Date**: 2026-03-04

## Prerequisites

- Rust 1.85.0+ (stable)
- `cargo-fuzz` installed: `cargo install cargo-fuzz`
- TypeScript agent-brain `.mv2` fixtures committed to `tests/fixtures/`

## Verify Current State

```bash
# Run existing tests (expect 2 CLI test failures — pre-existing)
cargo test --workspace

# Run existing benchmarks
cargo bench --workspace

# Check lint and format
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Implementation Order

### Phase 1: Fix Existing Failures + Unit Test Gaps

```bash
# Fix the 2 failing CLI tests first
cargo test --test find_test

# Add unit tests to hooks crate (currently 0 unit tests)
# Add unit tests to opencode crate (currently 0 unit tests)
cargo test -p rusty-brain-hooks --lib
cargo test -p rusty-brain-opencode --lib
```

### Phase 2: Compatibility & Migration Tests

```bash
# Generate TypeScript fixtures (one-time, outside Rust)
# Place in tests/fixtures/small_10obs.mv2, etc.
# Create tests/fixtures/expected_results.json

# Run compatibility tests
cargo test --test compatibility
```

### Phase 3: Concurrency Tests

```bash
# Run concurrency tests with multiple writer counts
cargo test mind_concurrent -- --nocapture
```

### Phase 4: Performance Benchmarks

```bash
# Capture TypeScript baselines first (one-time)
# Place in tests/fixtures/ts_baselines.json

# Run Rust benchmarks and compare
cargo bench --workspace
```

### Phase 5: Fuzz Testing

```bash
# Initialize fuzz targets
cd crates/compression && cargo fuzz init
cargo fuzz add compression_fuzz
cargo fuzz run compression_fuzz -- -max_total_time=60

cd crates/hooks && cargo fuzz init
cargo fuzz add hook_json_fuzz
cargo fuzz run hook_json_fuzz -- -max_total_time=60

cd crates/core && cargo fuzz init
cargo fuzz add search_query_fuzz
cargo fuzz run search_query_fuzz -- -max_total_time=60
```

### Phase 6: CI Enhancement

```bash
# Update .github/workflows/ci.yml
# Add benchmark regression step
# Add fuzz smoke test step
# Verify full pipeline
gh workflow run ci.yml
```

## Validation

```bash
# Full quality gate check
cargo fmt --check && \
cargo clippy --workspace -- -D warnings && \
cargo test --workspace && \
cargo bench --workspace
```
