# Test Fixtures

Test data for compatibility and performance testing against the TypeScript agent-brain reference implementation.

## Naming Conventions

- `small_10obs.mv2` — ~10 observations, basic compatibility
- `medium_100obs.mv2` — ~100 observations, scale testing
- `edge_cases.mv2` — Unicode, empty strings, long text
- `expected_results.json` — Reference search/timeline/stats from TypeScript
- `ts_baselines.json` — TypeScript performance measurements

## Data Model Schemas

### Test Fixture

| Field | Type | Description |
|-------|------|-------------|
| name | String | Fixture identifier (e.g., "small_10obs") |
| mv2_path | PathBuf | Path to `.mv2` file |
| expected_results_path | PathBuf | Path to `expected_results.json` |
| ts_version | String | TypeScript agent-brain version |
| observation_count | usize | Number of observations |

### ExpectedSearchResult (in expected_results.json)

```json
{
  "fixture": "small_10obs",
  "queries": [
    {
      "query": "search term",
      "total_count": 3,
      "results": [
        {
          "content": "observation text",
          "rank": 1,
          "score_min": 0.85,
          "score_max": 0.87
        }
      ]
    }
  ]
}
```

### TypeScriptBaseline (in ts_baselines.json)

```json
{
  "baselines": [
    {
      "metric": "query_latency_ms",
      "value": 45.0,
      "workload": "100 observations, single query",
      "ts_version": "1.0.0",
      "hardware": "Apple M-series"
    }
  ]
}
```

## Validation Rules

- `.mv2` files must be readable by memvid-core without errors
- `expected_results.json` must parse as valid JSON matching the schema above
- Baseline values must be positive numbers
- Score tolerance: |rust_score - ts_score| <= 0.01
- Benchmark threshold: ts_baseline / rust_result >= 2.0
