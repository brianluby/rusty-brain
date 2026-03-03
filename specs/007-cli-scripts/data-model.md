# Data Model: CLI Scripts (007)

**Date**: 2026-03-02 | **Branch**: `007-cli-scripts`

## Overview

The CLI introduces no new persistent data models. It consumes existing types from `crates/core` and `crates/types`, defines CLI-local serializable output types for JSON rendering, and adds one new public type (`TimelineEntry`) to `crates/core` for the timeline API extension.

## Upstream Types (consumed, not modified)

### ObservationType (crates/types)

```rust
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservationType {
    Discovery, Decision, Problem, Solution, Pattern,
    Warning, Success, Refactor, Bugfix, Feature,
}
```

- Already has `Serialize`, `Deserialize`, `Display`, `FromStr`
- `FromStr` is case-insensitive — used by `--type` flag parsing
- `Display` returns lowercase — used in human-readable output

### MindStats (crates/types)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MindStats {
    pub total_observations: u64,
    pub total_sessions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_memory: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_memory: Option<DateTime<Utc>>,
    #[serde(rename = "fileSize")]
    pub file_size_bytes: u64,
    #[serde(rename = "topTypes")]
    pub type_counts: HashMap<ObservationType, u64>,
}
```

- **Caution**: Serializes to camelCase JSON keys (`totalObservations`, `fileSize`, `topTypes`)
- CLI mirror type `StatsOutput` overrides to snake_case keys per PRD contract

### MemorySearchResult (crates/core::mind)

```rust
#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub obs_type: ObservationType,
    pub summary: String,
    pub content_excerpt: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub score: f64,
    pub tool_name: String,
}
```

- Does NOT have `Serialize` — intentional (research R3)
- CLI maps to `SearchResultJson` for serialization

### MindConfig (crates/types)

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct MindConfig {
    pub memory_path: PathBuf,
    pub max_context_observations: u32,
    pub max_context_tokens: u32,
    pub auto_compress: bool,
    pub min_confidence: f64,
    pub debug: bool,
}
```

- `MindConfig::from_env()` handles platform path resolution
- CLI overrides `memory_path` if `--memory-path` is provided

## New Upstream Type (added to crates/core)

### TimelineEntry (crates/core::mind)

```rust
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub obs_type: ObservationType,
    pub summary: String,
    pub timestamp: DateTime<Utc>,
    pub tool_name: String,
}
```

- New public type returned by `Mind::timeline()`
- Parsed from backend `FrameInfo.metadata` JSON (same pattern as `Mind::stats()`)
- Does NOT have `Serialize` — CLI maps to `TimelineEntryJson` for serialization

## CLI-Local Output Types (crates/cli only)

These types exist solely for JSON serialization in the CLI. They are NOT added to `crates/types`.

### FindOutput

```rust
#[derive(Debug, Serialize)]
pub struct FindOutput {
    pub results: Vec<SearchResultJson>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchResultJson {
    pub obs_type: String,              // ObservationType::to_string()
    pub summary: String,
    pub content_excerpt: Option<String>,
    pub timestamp: String,             // RFC 3339
    pub score: f64,
    pub tool_name: String,
}
```

**Mapping from MemorySearchResult**:
| Source field | Target field | Transform |
|-------------|-------------|-----------|
| `obs_type` | `obs_type` | `.to_string()` (lowercase) |
| `summary` | `summary` | direct |
| `content_excerpt` | `content_excerpt` | direct |
| `timestamp` | `timestamp` | `.to_rfc3339()` |
| `score` | `score` | direct |
| `tool_name` | `tool_name` | direct |

### AskOutput

```rust
#[derive(Debug, Serialize)]
pub struct AskOutput {
    pub answer: String,
    pub has_results: bool,
}
```

**Mapping from Mind::ask() return**:
| Source | Target field | Transform |
|--------|-------------|-----------|
| `String` return value | `answer` | direct |
| Answer content check | `has_results` | `!answer.is_empty() && !is_no_results_message(&answer)` |

### StatsOutput

```rust
#[derive(Debug, Serialize)]
pub struct StatsOutput {
    pub total_observations: u64,
    pub total_sessions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_memory: Option<String>,  // RFC 3339
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_memory: Option<String>,  // RFC 3339
    pub file_size_bytes: u64,
    pub type_counts: HashMap<String, u64>,  // obs_type as lowercase string keys
}
```

**Mapping from MindStats**:
| Source field | Target field | Transform |
|-------------|-------------|-----------|
| `total_observations` | `total_observations` | direct |
| `total_sessions` | `total_sessions` | direct |
| `oldest_memory` | `oldest_memory` | `.map(\|dt\| dt.to_rfc3339())` |
| `newest_memory` | `newest_memory` | `.map(\|dt\| dt.to_rfc3339())` |
| `file_size_bytes` | `file_size_bytes` | direct |
| `type_counts` | `type_counts` | Map `ObservationType` keys to `.to_string()` |

### TimelineOutput

```rust
#[derive(Debug, Serialize)]
pub struct TimelineOutput {
    pub entries: Vec<TimelineEntryJson>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct TimelineEntryJson {
    pub obs_type: String,     // ObservationType::to_string()
    pub summary: String,
    pub timestamp: String,    // RFC 3339
}
```

**Mapping from core::TimelineEntry**:
| Source field | Target field | Transform |
|-------------|-------------|-----------|
| `obs_type` | `obs_type` | `.to_string()` (lowercase) |
| `summary` | `summary` | direct |
| `timestamp` | `timestamp` | `.to_rfc3339()` |

## Entity Relationship

```text
┌─────────────────────────────────────────────────┐
│                  crates/types                     │
│  ObservationType  MindStats  MindConfig           │
└──────────────┬──────────────────┬────────────────┘
               │                  │
┌──────────────▼──────────────────▼────────────────┐
│                  crates/core                       │
│  Mind  MemorySearchResult  TimelineEntry (NEW)     │
└──────────────┬──────────────────┬────────────────┘
               │                  │
               │   delegates to   │
               ▼                  ▼
┌──────────────────────────────────────────────────┐
│                  crates/cli                        │
│  FindOutput  AskOutput  StatsOutput  TimelineOutput│
│  SearchResultJson  TimelineEntryJson               │
│                                                    │
│  (maps upstream types → CLI output types)          │
└──────────────────────────────────────────────────┘
```

## Validation Rules

| Field | Rule | Source |
|-------|------|--------|
| `--limit` | Must be positive integer (>0) | SEC-5, EC-4 |
| `--type` | Must be valid `ObservationType` variant (case-insensitive) | SEC-6, EC-5 |
| `--memory-path` | Must be existing file (not directory) | SEC-7, F1 |
| `obs_type` (output) | Always lowercase string | `ObservationType::Display` impl |
| `timestamp` (output) | Always RFC 3339 format | `DateTime::to_rfc3339()` |
| `score` (output) | Range 0.0 to 1.0 | Core Mind::search() contract |

## State Transitions

Not applicable — the CLI is stateless. Each invocation is a single-shot process: parse → resolve → open → query → format → exit. No state is maintained between invocations.
