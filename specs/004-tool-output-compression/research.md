# Research: Tool-Output Compression

**Branch**: `004-tool-output-compression` | **Date**: 2026-03-02

## Research Tasks & Findings

### 1. Regex Crate Selection

**Decision**: Use `regex` crate (not `regex-lite`)

**Rationale**: The full `regex` crate provides Unicode-aware matching needed for construct extraction in source files that may contain Unicode identifiers. `regex-lite` lacks some Unicode features and offers minimal size savings irrelevant to a non-WASM library crate.

**Alternatives considered**:
- `regex-lite` — smaller binary, but lacks Unicode character classes; insufficient for M-9
- `fancy-regex` — supports lookahead/lookbehind, but adds complexity and not needed for our patterns
- Manual string parsing — no external dep, but fragile and hard to maintain for 3+ languages

### 2. Panic Recovery Strategy

**Decision**: Use `std::panic::catch_unwind` to wrap specialized compressor calls

**Rationale**: `catch_unwind` is the idiomatic Rust way to catch panics at a boundary. Since compressor functions are pure (no `&mut` aliasing, no FFI), they satisfy the `UnwindSafe` requirement. This provides the safety net required by M-13 without changing the function signature to return `Result`.

**Alternatives considered**:
- Return `Result` from each compressor — forces error propagation into the public API, violating the infallible contract
- Use `std::panic::set_hook` — global state, not composable, overkill for this use case
- Trust compressors not to panic — risky; unexpected regex input or index errors could surface

**Note**: `catch_unwind` does NOT catch stack overflows or `abort()` calls. These are fatal regardless and acceptable per the AR risk analysis.

### 3. Logging Strategy

**Decision**: Use workspace `tracing` crate (already a workspace dependency)

**Rationale**: `tracing` is already declared in `[workspace.dependencies]`. Using it avoids adding a new dependency. Structured logging at DEBUG (dispatch decisions, compression ratios) and WARN (fallback triggered) levels satisfies constitution XI (Observability) without logging content at INFO+ (constitution IX).

**Alternatives considered**:
- `log` crate — simpler API but `tracing` is already in workspace; no reason to add another
- No logging — violates constitution XI; diagnostics would be invisible
- Custom statistics-only (no tracing) — insufficient for diagnosing fallback triggers

### 4. Character Counting Approach

**Decision**: Use `.chars().count()` consistently for all budget comparisons

**Rationale**: PRD assumption A-6 and spec clarification explicitly require Unicode `char` count, not byte count. This matches the TypeScript implementation's `string.length` behavior. While `.chars().count()` is O(n), it's called at most twice per compression (threshold check + budget enforcement), well within the 5ms budget.

**Alternatives considered**:
- `.len()` (byte count) — faster (O(1)) but would break budget guarantees for multi-byte content; violates PRD
- Custom char iterator with early exit — premature optimization; `.chars().count()` is sufficient

### 5. Construct Extraction Patterns (TypeScript → Rust Regex)

**Decision**: Port TypeScript regex patterns with Rust syntax adjustments

**Rationale**: The TypeScript implementation's patterns are production-proven. Key syntax differences:
- Rust `regex` uses `(?m)` for multiline mode (same as JS `m` flag)
- No lookbehind in Rust `regex` — rewrite patterns that used JS `(?<=...)` as capturing groups
- Named groups: `(?P<name>...)` in Rust vs. `(?<name>...)` in JS
- Rust regex has no `\d` shorthand for Unicode digits by default — use `[0-9]` for ASCII-only digit matching

**Languages and key patterns**:

| Language | Imports | Functions | Classes/Structs | Error Markers |
|----------|---------|-----------|-----------------|---------------|
| JS/TS | `import`, `require`, `from` | `function`, `=>`, `async function` | `class`, `interface` | TODO, FIXME, HACK, XXX, BUG |
| Python | `import`, `from...import` | `def`, `async def` | `class` | Same markers |
| Rust | `use`, `mod` | `fn`, `pub fn`, `async fn` | `struct`, `enum`, `trait`, `impl` | Same markers |

**Alternatives considered**:
- Tree-sitter — AST-level accuracy but heavy dependency (PRD Tool/Approach: deferred to C-1)
- syn crate (Rust only) — only parses Rust, doesn't help with JS/TS/Python

### 6. Workspace Dependency Integration

**Decision**: Add `regex` to `[workspace.dependencies]` in root `Cargo.toml`; reference from `crates/compression/Cargo.toml` with `workspace = true`

**Rationale**: Follows existing workspace dependency pattern (serde, thiserror, tracing all use this pattern). Ensures version consistency across crates if other crates ever need regex.

**Changes required**:
1. Root `Cargo.toml`: Add `regex = "1"` to `[workspace.dependencies]`
2. `crates/compression/Cargo.toml`: Add `regex = { workspace = true }` and `tracing = { workspace = true }` to `[dependencies]`

### 7. Relationship to Types Crate

**Decision**: Compression types (`CompressionConfig`, `CompressedResult`, etc.) live in the compression crate, not the types crate

**Rationale**: The types crate houses cross-crate shared types (Observation, MindConfig, HookInput). Compression types are internal to the compression pipeline and consumed only by the future integration layer. Moving them to types would couple unrelated crates to compression concerns. If integration needs shared types later, specific types can be promoted at that time.

**Alternatives considered**:
- Put all types in the types crate — over-couples; violates crate-first principle for compression-only types
- Create a shared `compression-types` crate — unnecessary crate proliferation for 3 small structs
