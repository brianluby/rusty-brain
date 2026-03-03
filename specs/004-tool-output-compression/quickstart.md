# Quickstart: Tool-Output Compression

**Branch**: `004-tool-output-compression`

## Prerequisites

- Rust stable (≥ 1.85.0, edition 2024)
- Workspace builds: `cargo build` succeeds from repo root

## Build

```bash
# Build just the compression crate
cargo build -p compression

# Build with all checks
cargo clippy -p compression -- -D warnings
cargo fmt -p compression --check
```

## Test

```bash
# Run all compression tests
cargo test -p compression

# Run a specific test
cargo test -p compression -- test_name

# Run with output visible
cargo test -p compression -- --nocapture
```

## Usage (from other crates)

```rust
use compression::{compress, CompressionConfig};

let config = CompressionConfig::default();
// config.compression_threshold == 3_000
// config.target_budget == 2_000

let result = compress(
    &config,
    "Read",                          // tool name (case-insensitive)
    &large_file_content,             // raw tool output
    Some("/path/to/file.rs"),        // optional context
);

if result.compression_applied {
    println!("Compressed {} → {} chars", result.original_size, result.text.chars().count());
    if let Some(stats) = &result.statistics {
        println!("Ratio: {:.1}×, saved {:.0}%", stats.ratio, stats.percentage_saved);
    }
} else {
    println!("Below threshold, returned unchanged");
}
```

## Custom Configuration

```rust
let config = CompressionConfig {
    compression_threshold: 5_000,  // trigger at 5K chars
    target_budget: 3_000,          // allow up to 3K chars output
};
```

## Supported Tool Types

| Tool Name | Compressor | Strategy |
|-----------|------------|----------|
| Read | `read.rs` | Extract language constructs (imports, functions, classes) |
| Bash | `bash.rs` | Preserve errors, warnings, success indicators |
| Grep | `grep.rs` | Group by file, show match counts |
| Glob | `glob.rs` | Group by directory, show file counts |
| Edit | `edit.rs` | File path + change summary |
| Write | `edit.rs` | File path + creation indicator |
| (other) | `generic.rs` | Head/tail truncation with omission marker |

## Key Behaviors

- **Infallible**: `compress()` never panics or returns errors
- **Budget guarantee**: Output ≤ `target_budget` characters when compression applied
- **Pass-through**: Empty, whitespace-only, or below-threshold inputs returned unchanged
- **Fallback**: If a specialized compressor fails, generic compressor is used automatically
