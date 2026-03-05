# Data Model: Testing & Quality + Migration & Backwards Compatibility

**Feature Branch**: `010-testing-migration`
**Date**: 2026-03-04

## Entities

### Test Fixture (new)

Represents a pre-generated TypeScript `.mv2` file used for compatibility testing.

| Field | Type | Description |
|-------|------|-------------|
| name | String | Fixture identifier (e.g., "small_10obs", "medium_100obs", "edge_cases") |
| mv2_path | PathBuf | Path to `.mv2` file in `tests/fixtures/` |
| expected_results_path | PathBuf | Path to `expected_results.json` with reference outputs |
| ts_version | String | TypeScript agent-brain version that generated the fixture |
| observation_count | usize | Number of observations in the fixture |

**Location**: `tests/fixtures/` (committed binary + JSON files, not a Rust struct)

### TypeScript Baseline (new)

Represents pre-captured performance measurements from the TypeScript version.

| Field | Type | Description |
|-------|------|-------------|
| metric | String | One of: "query_latency_ms", "compression_throughput_mb_s", "startup_time_ms" |
| value | f64 | Measured value |
| workload | String | Description of test workload |
| ts_version | String | TypeScript version measured |
| hardware | String | Machine spec identifier |

**Location**: `tests/fixtures/ts_baselines.json` (JSON file, not a Rust struct)

### Expected Search Result (new)

Reference output for compatibility comparison.

| Field | Type | Description |
|-------|------|-------------|
| query | String | Search query used |
| results | Vec<ExpectedHit> | Ordered list of expected results |
| total_count | usize | Total number of results |

### ExpectedHit (new)

| Field | Type | Description |
|-------|------|-------------|
| content | String | Observation content text |
| rank | usize | Expected position in results (1-based) |
| score_min | f64 | Minimum acceptable similarity score |
| score_max | f64 | Maximum acceptable similarity score (±0.01 tolerance) |

**Location**: Embedded in `expected_results.json` per fixture

### Fuzz Corpus Entry (new)

Seed inputs for fuzz testing harnesses.

| Field | Type | Description |
|-------|------|-------------|
| harness | String | Target harness: "compression", "hook_json", "search_query" |
| input | Vec<u8> | Raw bytes for the fuzz input |
| description | String | What this seed exercises (optional, for documentation) |

**Location**: `crates/<name>/fuzz/corpus/<harness_name>/` (raw files, not a Rust struct)

## Relationships

```text
Test Fixture 1──* Expected Search Result 1──* ExpectedHit
TypeScript Baseline (standalone, no FK)
Fuzz Corpus Entry (standalone, per-harness)
```

## Existing Entities Referenced (no changes)

- `Observation` (`crates/types`) — Written to/read from `.mv2` files; compatibility tests verify round-trip
- `Mind` (`crates/core`) — Primary API exercised by integration and compatibility tests
- `Diagnostic` (`crates/types`) — Used for legacy path detection warnings
- `CompressionConfig` / `CompressedResult` (`crates/compression`) — Fuzz targets

## State Transitions

No new runtime state transitions. Test fixtures are static data. Fuzz corpora grow monotonically (new crash inputs are appended as regression tests).

## Validation Rules

- Fixture `.mv2` files MUST be readable by `memvid-core` without errors
- `expected_results.json` MUST parse as valid JSON matching the `ExpectedSearchResult` schema
- Baseline values MUST be positive numbers
- Score tolerance: `|rust_score - ts_score| <= 0.01` (from clarification)
- Benchmark threshold: `ts_baseline / rust_result >= 2.0` (from clarification)
