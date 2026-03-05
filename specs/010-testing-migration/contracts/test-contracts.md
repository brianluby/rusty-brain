# Test & Migration Contracts

**Feature Branch**: `010-testing-migration`
**Date**: 2026-03-04

## Contract 1: Compatibility Test Interface

Tests that verify Rust produces identical results to TypeScript when reading the same `.mv2` file.

```rust
/// Load a TypeScript-generated .mv2 fixture and verify search results match expected output.
///
/// # Contract
/// - Input: fixture path (PathBuf), query (String)
/// - Output: Vec<SearchResult> with same ordering as TypeScript
/// - Tolerance: |rust_score - ts_score| <= 0.01
/// - Failure: returns structured error if .mv2 is unreadable or results diverge
#[cfg(test)]
fn assert_compatible_search(fixture_path: &Path, query: &str, expected: &[ExpectedHit]) {
    // Load mind from fixture
    // Execute search with query
    // Compare result set, ordering, and scores within tolerance
}
```

## Contract 2: Benchmark Regression Gate

Benchmarks that compare Rust performance against TypeScript baselines.

```rust
/// Benchmark contract: each metric must be >= 2× faster than TypeScript baseline.
///
/// Metrics:
/// - query_latency_ms: time to execute a standard search query
/// - compression_throughput_mb_s: MB/s of tool output compression
/// - startup_time_ms: cold start to first command ready
///
/// Baseline source: tests/fixtures/ts_baselines.json
/// Threshold: rust_metric <= ts_metric / 2.0 (for latency/time)
///            rust_metric >= ts_metric * 2.0 (for throughput)
```

## Contract 3: Fuzz Harness Interface

Each fuzz harness must handle arbitrary byte input without panics.

```rust
/// Fuzz harness contract:
/// - Input: arbitrary &[u8]
/// - Output: Ok(result) or Err(structured_error) — NEVER panic
/// - Duration: minimum 60 seconds per harness in CI
/// - Crash inputs: saved to fuzz/artifacts/ and added as regression tests
///
/// Required harnesses:
/// 1. compression_fuzz — feeds bytes to compression engine
/// 2. hook_json_fuzz — feeds bytes to hook JSON parser
/// 3. search_query_fuzz — feeds bytes to search query parser
```

## Contract 4: Legacy Path Detection

```rust
/// Detect legacy .claude/mind.mv2 path and return diagnostic.
///
/// # Contract
/// - Input: project_root (PathBuf)
/// - Behavior:
///   - If .claude/mind.mv2 exists AND .agent-brain/mind.mv2 does NOT exist:
///     Return Diagnostic::Warning with migration suggestion
///   - If BOTH exist:
///     Use .agent-brain/mind.mv2, return Diagnostic::Warning about duplicate
///   - If only .agent-brain/mind.mv2 exists:
///     No diagnostic (normal path)
/// - Output: Option<Diagnostic>
fn detect_legacy_path(project_root: &Path) -> Option<Diagnostic>;
```

## Contract 5: Concurrency Safety

```rust
/// Concurrent writer test contract:
/// - Spawn N writers (N >= 4) targeting the same .mv2 file
/// - Each writer stores a unique observation
/// - After all writers complete:
///   - All N observations MUST be retrievable (no data loss)
///   - File MUST not be corrupted (Mind::open succeeds)
/// - Stale lock recovery MUST complete within 5 seconds
///
/// Test configurations: N = 4, 8, 16
```

## Contract 6: Environment Variable Compatibility

```rust
/// All environment variables from TypeScript version must be honored identically.
///
/// Required variables:
/// - MEMVID_PLATFORM: platform override (claude, opencode, auto)
/// - MEMVID_MIND_DEBUG: enable debug output (true/false)
/// - MEMVID_PLATFORM_MEMORY_PATH: custom .mv2 file path
/// - MEMVID_PLATFORM_PATH_OPT_IN: enable path-based features (true/false)
/// - CLAUDE_PROJECT_DIR: Claude project directory override
/// - OPENCODE_PROJECT_DIR: OpenCode project directory override
///
/// Each variable must be tested with: valid value, invalid value, unset.
```
