# Data Model: Tool-Output Compression

**Branch**: `004-tool-output-compression` | **Date**: 2026-03-02

## Entity Definitions

### CompressionConfig

Configuration struct controlling compression behavior. Implements `Default` for sensible production defaults.

| Field | Type | Default | Validation | Description |
|-------|------|---------|------------|-------------|
| `compression_threshold` | `usize` | `3_000` | Must be > 0 | Character count above which compression triggers |
| `target_budget` | `usize` | `2_000` | Must be > 0; must be < `compression_threshold` | Maximum character count for compressed output |

**Derives**: `Debug`, `Clone`, `PartialEq`
**Traits**: `Default`
**Validation**: `validate()` method returns `Result<(), String>` for invariant checking (budget < threshold)

### ToolType

Enum representing the tool that produced the output. Used for dispatch routing.

| Variant | Matches (case-insensitive) | Compressor Module |
|---------|---------------------------|-------------------|
| `Read` | "read" | `read.rs` |
| `Bash` | "bash" | `bash.rs` |
| `Grep` | "grep" | `grep.rs` |
| `Glob` | "glob" | `glob.rs` |
| `Edit` | "edit" | `edit.rs` |
| `Write` | "write" | `edit.rs` (shared) |
| `Other(String)` | anything else | `generic.rs` |

**Derives**: `Debug`, `Clone`, `PartialEq`
**Conversion**: `From<&str>` — case-insensitive matching via `to_ascii_lowercase()`

### CompressedResult

The output of a compression operation. Always returned (never an error).

| Field | Type | Description |
|-------|------|-------------|
| `text` | `String` | The (possibly compressed) output text |
| `compression_applied` | `bool` | `true` if compression was performed; `false` if pass-through |
| `original_size` | `usize` | Character count of the original input (`output.chars().count()`) |
| `statistics` | `Option<CompressionStatistics>` | Present when `compression_applied` is `true` |

**Derives**: `Debug`, `Clone`, `PartialEq`

### CompressionStatistics

Diagnostic data about a compression operation. Attached to `CompressedResult` when compression occurred.

| Field | Type | Description |
|-------|------|-------------|
| `ratio` | `f64` | `original_size / compressed_size` (e.g., 10.0 means 10× reduction) |
| `chars_saved` | `usize` | `original_size - compressed_size` |
| `percentage_saved` | `f64` | `(chars_saved / original_size) * 100.0` (0.0–100.0) |

**Derives**: `Debug`, `Clone`, `PartialEq`

## Relationships

```text
CompressionConfig ──configures──▶ compress() entry point
                                      │
                                      ▼
                              ToolType (from tool_name)
                                      │
                              ┌───────┼───────┐
                              ▼       ▼       ▼
                          Read    Bash    Generic  ... (other compressors)
                              │       │       │
                              └───────┼───────┘
                                      ▼
                              CompressedResult
                                      │
                                      ▼
                         CompressionStatistics (optional)
```

## State Transitions

None — all entities are immutable value types. Compression is a stateless, single-pass operation.

## Invariants

1. `CompressedResult.text.chars().count() ≤ config.target_budget` when `compression_applied` is `true`
2. `CompressedResult.original_size == output.chars().count()` always
3. `CompressionStatistics.ratio ≥ 1.0` when present (compressed is always ≤ original)
4. `CompressionConfig.target_budget < CompressionConfig.compression_threshold`
5. `ToolType::from("READ") == ToolType::Read` (case-insensitive)
