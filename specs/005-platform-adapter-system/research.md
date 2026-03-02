# Research: Platform Adapter System

**Feature**: 005-platform-adapter-system | **Date**: 2026-03-01

## R1: Type Placement — Platform Types in `types` vs `platforms`

**Decision**: Split. Pure data types in `types` crate; behavior in `platforms` crate.

**Rationale**: The `types` crate already houses all shared data structures (HookInput, Observation, SessionSummary, etc.) with no business logic. PlatformEvent, ProjectContext, and DiagnosticRecord are domain types that other crates (hooks, core, cli) will need to reference. Placing them in `types` maintains the existing pattern and avoids circular dependencies.

**Alternatives considered**:
- All in `platforms`: Would require `hooks` and `core` to depend on `platforms` just for types, creating unwanted coupling.
- New `platform-types` crate: Unnecessary given the existing `types` crate already serves this exact role.

## R2: Adapter Trait Design — Trait Object vs Enum Dispatch

**Decision**: Trait object (`Box<dyn PlatformAdapter>`) stored in the registry.

**Rationale**: The spec requires extensibility (SC-007: "A new platform adapter can be added by implementing the adapter interface and registering it, without modifying any existing code"). Trait objects provide open-world extensibility — new adapters can be added by external crates implementing the trait. Enum dispatch would require modifying the enum for each new adapter, violating the open/closed principle and SC-007.

**Alternatives considered**:
- Enum dispatch (`match` on platform name): Faster (no vtable), but closed-world — adding a platform requires modifying the enum. Violates SC-007.
- Generic type parameter: Would infect all callers with generic bounds. Trait objects are more ergonomic here since adapters are stored in a registry with heterogeneous types.

**Implementation note**: The trait will be `Send + Sync` to support potential future concurrent usage, following Rust best practices for trait objects in registries.

## R3: Semver Parsing Strategy

**Decision**: Use the `semver` crate (already in workspace) to parse contract versions. Extract only the major version for comparison. Pre-release and build metadata are parsed but ignored for compatibility checking.

**Rationale**: The `semver` crate is already a workspace dependency (version 1). It handles all parsing edge cases (pre-release, build metadata, malformed strings). `semver::Version::parse(&str)` returns a `Result<Version, semver::Error>`, yielding `Err` on malformed input — exactly what FR-008 needs for detecting unparseable versions.

**Alternatives considered**:
- Manual parsing with `split('.')`: Fragile, doesn't handle pre-release/build metadata correctly.
- Custom version type: No benefit over the battle-tested `semver` crate.

## R4: HookInput Reuse vs New Raw Input Type

**Decision**: Accept `&HookInput` as the raw input for both Claude and OpenCode adapters. The existing `HookInput` type (in `types::hooks`) already captures all fields needed for normalization.

**Rationale**: `HookInput` has `session_id`, `cwd`, `hook_event_name`, `tool_name`, `platform`, and uses `#[serde(default)]` for forward compatibility. Creating a separate raw input type would duplicate the schema with no benefit.

**Alternatives considered**:
- `serde_json::Value` as raw input: More flexible but loses type safety. The adapters would need to do their own field extraction, duplicating the deserialization logic already in `HookInput`.
- New `RawHookInput` type: No additional fields needed beyond what `HookInput` already provides.

**Note**: If OpenCode's hook protocol differs significantly, a separate `OpenCodeHookInput` type can be added to `types` later. For now, the spec says both adapters share the same normalization logic.

## R5: Event ID Generation Strategy

**Decision**: Use `uuid::Uuid::new_v4()` for event IDs (same as `Observation::new`).

**Rationale**: Consistent with existing codebase pattern. UUID v4 is already a workspace dependency and provides the uniqueness guarantee required by FR-002.

**Alternatives considered**:
- ULID (Universally Unique Lexicographically Sortable Identifier): Would need a new dependency. UUID v4 is sufficient since events already carry timestamps for ordering.
- Sequential counter: Not unique across sessions or processes.

## R6: Platform Detection Implementation

**Decision**: Pure function `detect_platform(input: &HookInput) -> String` that checks sources in priority order: explicit field > MEMVID_PLATFORM env var > platform indicators > default "claude".

**Rationale**: Stateless function matches the spec's priority chain (FR-006). Environment variable reading is safe and doesn't need sandboxing (spec assumption). The function normalizes to lowercase and trims whitespace (FR-007).

**Alternatives considered**:
- Struct with cached detection: Over-engineering for a pure function called once per hook invocation.
- Builder pattern: Adds complexity with no benefit for a simple priority chain.

## R7: Error Code Naming Convention

**Decision**: Add new error codes under existing `error_codes` module with `E_PLATFORM_*` prefix. Add a new `AgentBrainError::Platform` variant.

**Rationale**: Follows existing naming pattern (`E_FS_*`, `E_CONFIG_*`, etc.). New codes needed:
- `E_PLATFORM_INCOMPATIBLE_CONTRACT` — major version mismatch
- `E_PLATFORM_INVALID_CONTRACT_VERSION` — malformed version string
- `E_PLATFORM_MISSING_SESSION_ID` — normalization requires session ID
- `E_PLATFORM_MISSING_PROJECT_IDENTITY` — unresolvable project context
- `E_PLATFORM_PATH_TRAVERSAL` — resolved path escapes project directory

**Alternatives considered**:
- Reuse existing codes: `E_INPUT_INVALID_FORMAT` could work for version parsing, but platform-specific codes are more diagnostic-friendly and align with the spec's distinct error reasons.

## R8: Diagnostic Record — Struct vs Builder

**Decision**: Simple struct construction via `DiagnosticRecord::new()` validated constructor (same pattern as `Observation::new`).

**Rationale**: DiagnosticRecord has 9 fields but most are passed directly (no complex computation). A builder would add API surface for infrequent construction. The `new()` constructor validates field names (dedup, cap at 20) and computes expiration.

**Alternatives considered**:
- Builder pattern: More ergonomic for many optional fields, but DiagnosticRecord has no optional fields — all are required.
- Free function: Doesn't provide the same discoverability as `DiagnosticRecord::new()`.

## R9: Memory Path Policy — Where to Implement

**Decision**: In `platforms` crate as a `resolve_memory_path()` function that returns a `ResolvedMemoryPath` struct.

**Rationale**: Path policy depends on platform name and opt-in flags, which are platform-adapter concerns. The function is pure (takes inputs, returns a path) with path traversal validation. It does NOT perform filesystem I/O (no checking if directories exist).

**Alternatives considered**:
- In `types` crate: Types crate has no business logic by convention.
- In `config` module of types: `MindConfig::from_env()` already does some path resolution, but the new path policy is more sophisticated (platform-specific paths, mode tracking). Better as a dedicated function in `platforms`.

## R10: Registry Concurrency Model

**Decision**: `AdapterRegistry` uses a `HashMap<String, Box<dyn PlatformAdapter>>` with no interior mutability. Registration happens at startup (before event processing), and lookup is read-only during processing.

**Rationale**: The adapter system runs in a single-threaded hook process. There's no concurrent access pattern. Adding `Arc<RwLock<...>>` would be over-engineering for the current use case.

**Alternatives considered**:
- `Arc<RwLock<HashMap<...>>>`: Thread-safe but unnecessary overhead for single-threaded hook processes.
- `OnceCell`/`LazyLock`: Good for static registries but doesn't support dynamic registration.
