# Quickstart: Platform Adapter System

## Build & Test

```bash
# Build workspace (includes platforms crate)
cargo build

# Run all tests
cargo test

# Run only platforms crate tests
cargo test -p platforms

# Run only types crate tests (new platform types)
cargo test -p types

# Lint check
cargo clippy -- -D warnings

# Format check
cargo fmt --check
```

## Usage Flow (from hook process)

```rust
use types::hooks::HookInput;
use platforms::{
    detect_platform,
    AdapterRegistry,
    EventPipeline,
};

// 1. Deserialize hook input from stdin
let input: HookInput = serde_json::from_reader(std::io::stdin())?;

// 2. Detect platform
let platform_name = detect_platform(&input);

// 3. Resolve adapter from registry
let registry = AdapterRegistry::with_builtins();
let adapter = registry.resolve(&platform_name)
    .expect("built-in adapter must exist for detected platform");

// 4. Normalize into a typed event
let event = adapter.normalize(&input, &input.hook_event_name);
let Some(event) = event else {
    // Input cannot be normalized (missing session ID, etc.)
    // Fail-open: skip silently
    return Ok(());
};

// 5. Run through pipeline (contract validation + identity resolution)
let pipeline = EventPipeline::new();
let result = pipeline.process(&event);

if result.skipped {
    // Event was skipped — diagnostic available in result.diagnostic
    eprintln!("Skipped: {:?}", result.reason);
    return Ok(());
}

// 6. Use the resolved identity for memory isolation
let identity = result.identity.unwrap();
println!("Project key: {:?} (source: {:?})", identity.key, identity.source);
```

## Adding a Custom Platform Adapter

```rust
use platforms::adapter::PlatformAdapter;
use platforms::AdapterRegistry;
use types::hooks::HookInput;
use types::platform_event::PlatformEvent;

struct CursorAdapter;

impl PlatformAdapter for CursorAdapter {
    fn platform_name(&self) -> &str { "cursor" }
    fn contract_version(&self) -> &str { "1.0.0" }

    fn normalize(&self, input: &HookInput, event_kind_hint: &str) -> Option<PlatformEvent> {
        // Custom normalization logic for Cursor IDE
        // ...
    }
}

// Register it
let mut registry = AdapterRegistry::with_builtins();
registry.register(Box::new(CursorAdapter));
```

## Key Design Decisions

- **Fail-open**: All validation failures produce diagnostics but never block the agent
- **Trait objects**: Adapters use `Box<dyn PlatformAdapter>` for open-world extensibility
- **Two-crate split**: Types in `types`, behavior in `platforms`
- **No I/O in hot path**: Path resolution returns paths, doesn't touch the filesystem
- **Last-registered wins**: Duplicate adapter registration silently overwrites
