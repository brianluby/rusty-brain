# Contract: JSON Output Schemas

**Date**: 2026-03-02 | **Source**: PRD §Interface Contract, AR §Interface Definitions

All JSON output is written to stdout via `serde_json::to_string_pretty()`.
All timestamps are RFC 3339 format.
All `obs_type` values are lowercase strings matching `ObservationType::Display`.

## find

```json
{
  "results": [
    {
      "obs_type": "decision",
      "summary": "Chose PostgreSQL for user data",
      "content_excerpt": "After evaluating options...",
      "timestamp": "2026-02-28T14:30:00+00:00",
      "score": 0.92,
      "tool_name": "read_file"
    }
  ],
  "count": 1
}
```

### Field specifications

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `results` | array | No | Array of matching observations |
| `results[].obs_type` | string | No | Lowercase observation type |
| `results[].summary` | string | No | Observation summary text |
| `results[].content_excerpt` | string | Yes | Truncated content (up to 200 chars), null if no content |
| `results[].timestamp` | string | No | RFC 3339 timestamp |
| `results[].score` | number | No | Relevance score (0.0 to 1.0) |
| `results[].tool_name` | string | No | Tool that generated the observation |
| `count` | integer | No | Number of results in array |

### Edge cases

- No results: `{ "results": [], "count": 0 }`
- With `--type` filter: results are pre-filtered; `count` reflects filtered count

---

## ask

```json
{
  "answer": "The database schema was changed to add a users table with...",
  "has_results": true
}
```

### Field specifications

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `answer` | string | No | Synthesized answer from memory |
| `has_results` | boolean | No | Whether relevant memories were found |

### Edge cases

- No relevant memories: `{ "answer": "No relevant memories found for your question.", "has_results": false }`

---

## stats

```json
{
  "total_observations": 247,
  "total_sessions": 12,
  "oldest_memory": "2026-01-15T09:00:00+00:00",
  "newest_memory": "2026-03-01T18:45:00+00:00",
  "file_size_bytes": 524288,
  "type_counts": {
    "discovery": 89,
    "decision": 45,
    "problem": 32,
    "solution": 28,
    "pattern": 15,
    "warning": 12,
    "success": 10,
    "refactor": 8,
    "bugfix": 5,
    "feature": 3
  }
}
```

### Field specifications

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `total_observations` | integer | No | Total number of stored observations |
| `total_sessions` | integer | No | Number of distinct agent sessions |
| `oldest_memory` | string | Yes | RFC 3339 timestamp of oldest observation; key omitted when no observations |
| `newest_memory` | string | Yes | RFC 3339 timestamp of newest observation; key omitted when no observations |
| `file_size_bytes` | integer | No | Size of .mv2 file in bytes |
| `type_counts` | object | No | Map of observation type (lowercase) to count |

### Edge cases

- Empty memory file: `{ "total_observations": 0, "total_sessions": 0, "file_size_bytes": 0, "type_counts": {} }` — `oldest_memory` and `newest_memory` keys are omitted (not present as null)

---

## timeline

```json
{
  "entries": [
    {
      "obs_type": "discovery",
      "summary": "Found unused import in auth module",
      "timestamp": "2026-03-01T18:45:00+00:00"
    }
  ],
  "count": 1
}
```

### Field specifications

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `entries` | array | No | Array of timeline entries |
| `entries[].obs_type` | string | No | Lowercase observation type |
| `entries[].summary` | string | No | Observation summary text |
| `entries[].timestamp` | string | No | RFC 3339 timestamp |
| `count` | integer | No | Number of entries in array |

### Edge cases

- No entries: `{ "entries": [], "count": 0 }`
- With `--type` filter: entries are pre-filtered; `count` reflects filtered count
- `--oldest-first`: same schema, entries ordered chronologically (oldest first)

---

## Error Output (non-JSON mode)

Errors are written to stderr as plain text, not JSON. Exit code indicates error type.

When `--json` is active, errors are NOT output as JSON — they remain on stderr as plain text. This keeps stdout clean for piping (a failed `--json` command produces no stdout output, only stderr + non-zero exit).
