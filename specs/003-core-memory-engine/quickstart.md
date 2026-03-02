# Quickstart: Core Memory Engine

**Branch**: `003-core-memory-engine` | **Date**: 2026-03-01

## Prerequisites

- Rust 1.85.0+ (stable)
- Phase 1 types crate complete (with ULID migration and `RustyBrainError` rename)
- memvid-core available at pinned git revision

## Dependencies (Cargo.toml)

```toml
# crates/core/Cargo.toml
[package]
name = "rusty-brain-core"
version = "0.1.0"
edition.workspace = true

[dependencies]
memvid-core = { workspace = true }
types = { path = "../types" }
ulid = { version = "1", features = ["serde"] }
fs2 = "0.4"
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = "3"
tokio = { workspace = true, features = ["test-util"] }

[lints]
workspace = true
```

```toml
# Workspace Cargo.toml additions
[workspace.dependencies]
# Add to existing deps:
ulid = { version = "1", features = ["serde"] }
fs2 = "0.4"
tempfile = "3"

# Update memvid-core to enable "lex" feature:
memvid-core = { git = "https://github.com/brianluby/memvid/", rev = "fbddef4bff6ac756f91724681234243e98d5ba04", features = ["lex"] }
```

## Basic Usage

```rust
use rusty_brain_core::{Mind, estimate_tokens};
use types::{MindConfig, ObservationType, ObservationMetadata};

// 1. Open (or create) a memory file
let config = MindConfig::new("/path/to/project/.agent-brain/mind.mv2");
let mind = Mind::open(config)?;

// 2. Store an observation
let obs_id = mind.remember(
    ObservationType::Discovery,
    "Read",                  // tool_name — which tool generated this observation
    "Found caching pattern in service layer",
    Some("The UserService caches DB queries using an LRU cache with 5min TTL"),
    None, // no extra metadata
)?;
println!("Stored observation: {obs_id}");

// 3. Search past observations
let results = mind.search("caching pattern", Some(5))?;
for result in &results {
    println!("[{:?}] {} (score: {:.2})", result.obs_type, result.summary, result.score);
}

// 4. Ask a question
let answer = mind.ask("What caching strategies have been used?")?;
println!("Answer: {answer}");

// 5. Get session context (for agent startup injection)
let context = mind.get_context(Some("authentication module"))?;
println!("Context: {} recent, {} relevant, {} summaries ({} tokens)",
    context.recent_observations.len(),
    context.relevant_memories.len(),
    context.session_summaries.len(),
    context.token_count,
);

// 6. Save session summary at session end
let summary_id = mind.save_session_summary(
    vec!["Adopted LRU caching for user queries".into()],
    vec!["src/services/user.rs".into(), "src/cache/mod.rs".into()],
    "Implemented caching layer for UserService to reduce DB load",
)?;

// 7. Check stats
let stats = mind.stats()?;
println!("Total observations: {}, file size: {} bytes",
    stats.total_observations, stats.file_size_bytes);

// 8. Token estimation utility
let tokens = estimate_tokens("Hello world");
assert_eq!(tokens, 2); // 11 chars / 4 = 2 (truncated)
```

## Singleton Pattern

```rust
use rusty_brain_core::{get_mind, reset_mind};
use types::MindConfig;

// First call creates the instance
let mind = get_mind(MindConfig::new("/path/to/mind.mv2"))?;

// Subsequent calls return the same Arc<Mind>
let mind2 = get_mind(MindConfig::new("/path/to/mind.mv2"))?;
// mind and mind2 point to the same instance

// For testing: reset to allow fresh initialization
reset_mind();
```

## Error Handling

```rust
use types::{RustyBrainError, error_codes};

match mind.remember(ObservationType::Discovery, "Read", "", None, None) {
    Ok(id) => println!("Stored: {id}"),
    Err(e) => {
        // Machine-parseable error code
        println!("Error code: {}", e.code());
        // Human-readable message
        println!("Error: {e}");

        // Pattern match on specific codes
        match e.code() {
            error_codes::E_INPUT_EMPTY_FIELD => println!("Summary is required"),
            error_codes::E_MEM_CORRUPTED_INDEX => println!("Memory file corrupted"),
            _ => println!("Unexpected error"),
        }
    }
}
```

## Build & Test

```bash
# Build
cargo build -p rusty-brain-core

# Run all tests
cargo test -p rusty-brain-core

# Run with specific test
cargo test -p rusty-brain-core -- mind_roundtrip

# Lint
cargo clippy -p rusty-brain-core -- -D warnings

# Format check
cargo fmt -p rusty-brain-core -- --check
```

## Module Structure

```text
crates/core/src/
├── lib.rs              # pub: Mind, MemorySearchResult, estimate_tokens, get_mind, reset_mind
├── mind.rs             # Mind struct implementation
├── backend.rs          # pub(crate): MemvidBackend trait, SearchHit, TimelineEntry, FrameInfo, BackendStats
├── memvid_store.rs     # pub(crate): MemvidStore (production backend)
├── file_guard.rs       # pub(crate): validate_and_open, backup_and_prune
├── context_builder.rs  # pub(crate): build()
└── token.rs            # pub: estimate_tokens()
```

## Key Constraints

- **No `unsafe`** — workspace lints forbid it
- **No network** — local filesystem only
- **No content logging at INFO+** — use DEBUG/TRACE only, guarded by `MEMVID_MIND_DEBUG`
- **All errors are `RustyBrainError`** — no panics escape to callers
- **`Mind` is `Send + Sync`** — safe for multi-threaded consumers
- **memvid-core pinned** — rev bumps require round-trip testing
