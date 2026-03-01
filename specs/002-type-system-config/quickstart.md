# Quickstart: Type System & Configuration

**Branch**: `002-type-system-config` | **Date**: 2026-03-01

## Prerequisites

- Rust 1.85.0+ (stable)
- Workspace already bootstrapped (001-project-bootstrap complete)

## Module Layout

All types live in `crates/types/`. The recommended module structure:

```
crates/types/src/
├── lib.rs              # Re-exports all public types
├── observation.rs      # Observation, ObservationType, ObservationMetadata
├── session.rs          # SessionSummary
├── context.rs          # InjectedContext
├── config.rs           # MindConfig, Default, from_env()
├── stats.rs            # MindStats
├── hooks.rs            # HookInput, HookOutput
└── error.rs            # AgentBrainError, error_codes module
```

## Dependencies Required

Add to `crates/types/Cargo.toml`:

```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

## Build & Test

```bash
# Build the types crate
cargo build -p types

# Run types crate tests
cargo test -p types

# Check lints
cargo clippy -p types -- -D warnings

# Check formatting
cargo fmt -p types --check

# Generate docs
cargo doc -p types --no-deps --open
```

## Usage Examples

### Creating an Observation

```rust
use types::{Observation, ObservationType, ObservationMetadata};
use chrono::Utc;
use uuid::Uuid;

let obs = Observation {
    id: Uuid::new_v4(),
    timestamp: Utc::now(),
    obs_type: ObservationType::Discovery,
    tool_name: "Read".to_string(),
    summary: "Found config pattern in main.rs".to_string(),
    content: "The main.rs file uses a builder pattern for config...".to_string(),
    metadata: Some(ObservationMetadata {
        files: vec!["src/main.rs".to_string()],
        platform: "claude".to_string(),
        project_key: "my-project".to_string(),
        compressed: false,
        session_id: Some("session-123".to_string()),
        extra: Default::default(),
    }),
};
```

### Configuration with Defaults

```rust
use types::MindConfig;

// All defaults
let config = MindConfig::default();
assert_eq!(config.max_context_observations, 20);
assert_eq!(config.min_confidence, 0.6);

// From environment
let config = MindConfig::from_env().expect("valid config");
```

### JSON Round-Trip

```rust
use types::Observation;

let json = serde_json::to_string(&obs).unwrap();
let deserialized: Observation = serde_json::from_str(&json).unwrap();
assert_eq!(obs, deserialized);
```

### Error Handling

```rust
use types::{AgentBrainError, error_codes};

let err = AgentBrainError::InvalidInput {
    code: error_codes::E_INPUT_EMPTY_FIELD,
    message: "observation summary cannot be empty".to_string(),
};
assert_eq!(err.code(), "E_INPUT_EMPTY_FIELD");
```

### Hook Input Parsing

```rust
use types::HookInput;

let json = r#"{
    "session_id": "abc123",
    "transcript_path": "/path/to/transcript.jsonl",
    "cwd": "/home/user/project",
    "permission_mode": "default",
    "hook_event_name": "PostToolUse",
    "tool_name": "Write",
    "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
    "tool_response": {"success": true},
    "tool_use_id": "toolu_01ABC",
    "unknown_future_field": "ignored"
}"#;

let input: HookInput = serde_json::from_str(json).unwrap();
assert_eq!(input.hook_event_name, "PostToolUse");
assert_eq!(input.tool_name, Some("Write".to_string()));
// Unknown fields silently ignored ✓
```

## Implementation Order

1. `error.rs` — error types first (other modules depend on `AgentBrainError`)
2. `observation.rs` — core data model
3. `session.rs` — depends on observation types conceptually
4. `context.rs` — depends on observation + session
5. `config.rs` — standalone with env var resolution
6. `stats.rs` — depends on observation type enum
7. `hooks.rs` — depends on context for HookOutput
8. `lib.rs` — re-export everything

Each module should have tests written BEFORE implementation (test-first per constitution V).
