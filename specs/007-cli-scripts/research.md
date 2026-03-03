# Research: CLI Scripts (007)

**Date**: 2026-03-02 | **Branch**: `007-cli-scripts`

## R1: Mind::timeline() Implementation Strategy

**Decision**: Add a public `Mind::timeline(limit, reverse)` method that delegates to `self.backend.timeline()` + `self.backend.frame_by_id()`, returning `Vec<TimelineEntry>` with parsed metadata.

**Rationale**: This follows the identical pattern used by `Mind::stats()` (lines 292-369 in `mind.rs`), which already iterates `backend.timeline()` entries and calls `frame_by_id()` to extract metadata JSON. The new method returns individual parsed entries instead of aggregating them into statistics.

**Alternatives considered**:
- Copy timeline logic into CLI: Rejected — violates constitution II (duplicating core logic) and accesses `pub(crate)` backend types.
- Expose `backend.timeline()` as public: Rejected — leaks internal backend abstraction, violates memvid isolation principle.
- Add timeline data to `get_context()` return type: Rejected — `get_context()` serves a different purpose (agent startup context) and has token budgeting constraints.

**Implementation sketch**:
```rust
// New public type in crates/core::mind
pub struct TimelineEntry {
    pub obs_type: ObservationType,
    pub summary: String,
    pub timestamp: DateTime<Utc>,
    pub tool_name: String,
}

// New public method on Mind
pub fn timeline(&self, limit: usize, reverse: bool) -> Result<Vec<TimelineEntry>, RustyBrainError> {
    let entries = self.backend.timeline(limit, reverse)?;
    let mut result = Vec::with_capacity(entries.len());
    for entry in &entries {
        let frame = self.backend.frame_by_id(entry.frame_id)?;
        // Parse metadata JSON (same pattern as stats())
        let obs_type = frame.metadata.get("obs_type")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<ObservationType>().ok())
            .unwrap_or(ObservationType::Discovery);
        let summary = frame.metadata.get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or(&entry.preview)
            .to_string();
        let timestamp = frame.metadata.get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let tool_name = frame.metadata.get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        result.push(TimelineEntry { obs_type, summary, timestamp, tool_name });
    }
    Ok(result)
}
```

---

## R2: MindStats Serialization — camelCase vs snake_case

**Decision**: CLI defines a mirror `StatsOutput` struct with `#[serde(rename_all = "snake_case")]` keys matching the PRD JSON contract. Map from `MindStats` (camelCase) in the CLI layer.

**Rationale**: `MindStats` (in `crates/types`) uses `#[serde(rename_all = "camelCase")]` producing keys like `totalObservations`, `fileSize`, `topTypes`. The PRD specifies snake_case keys: `total_observations`, `file_size_bytes`, `type_counts`. Adding a CLI-local mirror type is consistent with the AR decision to keep CLI-specific serialization types in `crates/cli`.

**Alternatives considered**:
- Use MindStats directly with camelCase output: Rejected — violates PRD JSON contract (AC-5, AC-8).
- Change MindStats to snake_case: Rejected — would break existing consumers and the types crate's convention.
- Use `#[serde(alias)]`: Rejected — aliases apply to deserialization, not serialization.

**Key field mappings**:
| MindStats field | StatsOutput field | JSON key |
|-----------------|-------------------|----------|
| `total_observations` | `total_observations` | `total_observations` |
| `total_sessions` | `total_sessions` | `total_sessions` |
| `oldest_memory` | `oldest_memory` | `oldest_memory` |
| `newest_memory` | `newest_memory` | `newest_memory` |
| `file_size_bytes` | `file_size_bytes` | `file_size_bytes` |
| `type_counts` | `type_counts` | `type_counts` |

---

## R3: MemorySearchResult Serialization Strategy

**Decision**: CLI defines a mirror `SearchResultJson` struct for JSON serialization. Do NOT add `#[derive(Serialize)]` to `MemorySearchResult` in `crates/core`.

**Rationale**: The AR explicitly decided this (Open Questions Q1): "CLI defines serializable mirror types (`SearchResultJson`, `TimelineEntryJson`) to avoid forcing serialization on core consumers." This keeps the core crate free from serialization concerns and allows the CLI to control field naming and formatting (e.g., RFC 3339 timestamp strings, `obs_type` as lowercase string).

**Alternatives considered**:
- Add `Serialize` to `MemorySearchResult`: Rejected — forces serde dependency on all core consumers; locks serialization format.
- Use `serde_json::to_value()` with manual mapping: Rejected — less type-safe than a dedicated struct.

---

## R4: New Workspace Dependencies

**Decision**: Add `tracing-subscriber` and `comfy-table` to the workspace `Cargo.toml`.

**Rationale**:
- `tracing-subscriber` (MIT, tokio team): Required for the `--verbose` flag (S-3) to initialize a tracing subscriber that outputs to stderr. The workspace already uses `tracing` for instrumentation but has no subscriber — binary crates need one.
- `comfy-table` (MIT, ~450 GitHub stars, actively maintained): Required for human-readable table output with column alignment and terminal width detection. Lighter than `tabled`, more feature-complete than manual formatting.

**Alternatives considered**:
- `tabled` for table output: Rejected — heavier dependency, more features than needed.
- `colored` for color output: Not needed separately — `comfy-table` supports color attributes. Can add later if needed for non-table colored text.
- Manual formatting with `format!()`: Rejected — reinventing column alignment and terminal width detection.
- `env_logger` instead of `tracing-subscriber`: Rejected — workspace already uses `tracing`, not `log`.

**License compliance**: Both MIT-licensed, compatible with existing workspace licenses.

---

## R5: Memory Path Resolution Strategy

**Decision**: Use `MindConfig::from_env()` as the primary path resolution mechanism. If `--memory-path` is provided, override `config.memory_path` before calling `Mind::open()`.

**Rationale**: `MindConfig::from_env()` already encapsulates the platform detection logic by reading environment variables (`MEMVID_PLATFORM_MEMORY_PATH`, `MEMVID_PLATFORM_PATH_OPT_IN`, `MEMVID_PLATFORM`, `CLAUDE_PROJECT_DIR`, etc.) and calling `resolve_memory_path()` internally. The CLI doesn't need to call `resolve_memory_path()` directly.

**Implementation**:
```rust
let mut config = MindConfig::from_env();
if let Some(path) = cli.memory_path {
    config.memory_path = path;
}
let mind = Mind::open(config)?;
```

**Alternatives considered**:
- Call `resolve_memory_path()` directly in CLI: Rejected — duplicates logic already in `MindConfig::from_env()`.
- Create a new `MindConfig::for_cli()` constructor: Rejected — over-engineering; `from_env()` + field override is sufficient.

**Edge case**: If neither `--memory-path` nor environment variables provide a path, `MindConfig::default()` uses `.agent-brain/mind.mv2` relative to the current directory. The CLI should check if this file exists before calling `Mind::open()` and provide a clear error if not (SEC-7, F1).

---

## R6: Mind::open() Behavior with Missing Files

**Decision**: Validate file existence before calling `Mind::open()`. Display a user-friendly error if the resolved path does not exist or is not a regular file.

**Rationale**: `Mind::open()` creates the backend, which may attempt to create a new memory file (it's designed for agent sessions that write). For the read-only CLI, opening a non-existent file should produce a clear error, not silently create one. Pre-validation also allows us to provide a more specific error message with the resolved path and a hint about `--memory-path`.

**Implementation**:
```rust
let path = &config.memory_path;
if !path.exists() {
    return Err(CliError::MemoryFileNotFound { path: path.clone() });
}
if !path.is_file() {
    return Err(CliError::NotAFile { path: path.clone() });
}
```

**Alternatives considered**:
- Rely on `Mind::open()` error: Rejected — error message would come from memvid, not user-friendly.
- Check in `Mind::open()`: Rejected — `Mind::open()` intentionally supports creating new files for agent sessions.

---

## R7: --limit Validation Approach

**Decision**: Use clap's `value_parser` with a range constraint to validate `--limit` is a positive integer at parse time.

**Rationale**: clap 4 supports `value_parser = clap::value_parser!(usize).range(1..)` which produces a clear error message automatically: `error: invalid value '0' for '--limit <LIMIT>': 0 is not in 1..`. This is simpler than manual validation and provides consistent error formatting.

**Implementation**:
```rust
#[arg(long, default_value_t = 10, value_parser = clap::value_parser!(usize).range(1..))]
limit: usize,
```

**Alternatives considered**:
- Manual validation after parse: Rejected — more code, inconsistent error formatting vs. other clap errors.
- Custom value parser: Rejected — range constraint is sufficient.

---

## R8: Error Type Strategy

**Decision**: Define a CLI-local `CliError` enum that wraps `RustyBrainError` and adds CLI-specific variants (file not found, not a file). Map to exit codes and user-friendly messages in `main.rs`.

**Rationale**: `RustyBrainError` from `crates/core` covers storage and backend errors but doesn't include CLI-specific concerns like "memory file not found" with path hints. A thin wrapper enum in CLI keeps error mapping clean and testable.

**Implementation**:
```rust
enum CliError {
    Core(RustyBrainError),
    MemoryFileNotFound { path: PathBuf },
    NotAFile { path: PathBuf },
}

impl CliError {
    fn exit_code(&self) -> i32 {
        match self {
            CliError::Core(RustyBrainError::LockTimeout { .. }) => 2,
            _ => 1,
        }
    }
}
```

**Alternatives considered**:
- Use `RustyBrainError` directly with `eprintln!()`: Rejected — no structured exit code mapping; can't distinguish lock timeout (exit 2) from other errors (exit 1).
- Use `anyhow`: Rejected — adds dependency; loses typed error matching for exit codes.
