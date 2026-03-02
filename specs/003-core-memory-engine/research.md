# Research: Core Memory Engine

**Branch**: `003-core-memory-engine` | **Date**: 2026-03-01

## 1. memvid-core Rust API Surface Verification

**Decision**: memvid-core API at rev `fbddef4` differs from the spec assumptions in several areas. The `MemvidBackend` trait must be adapted to the actual API.

**Rationale**: Direct verification of the pinned memvid-core source reveals the following API mapping:

| Spec Assumption | Actual memvid-core API | Adaptation Required |
|-----------------|----------------------|---------------------|
| `create(path, type)` | `Memvid::create(path) -> Result<Self>` | No type parameter; engine type inferred from file |
| `open(path)` | `Memvid::open(path) -> Result<Self>` | Direct match |
| `put(title, label, text, metadata, tags)` | `put_bytes(payload: &[u8]) -> Result<u64>` with `PutOptions` | Serialize observation to bytes; use `PutOptions` for metadata |
| `find(query, k, mode)` | `find(query: &str, limit: usize) -> Result<Vec<LexSearchHit>>` | No mode parameter (always lexical); direct match otherwise |
| `ask(question, k, mode)` | `ask(request: AskRequest, embedder: Option<&E>) -> Result<AskResponse>` | Feature-gated (`lex` feature); construct `AskRequest` struct |
| `timeline(limit, reverse)` | `timeline(query: TimelineQuery) -> Result<Vec<TimelineEntry>>` | Construct `TimelineQuery` with limit/reverse fields |
| `getFrameInfo(frame_id)` | `frame_by_id(frame_id: FrameId) -> Result<Frame>` | Use `Frame` struct instead of custom FrameInfo |
| `stats()` | `stats() -> Result<Stats>` | Direct match; map `Stats` fields to `MindStats` |
| `seal()` | `commit() -> Result<()>` | Renamed; use `commit()` for persistence |

**Key Findings**:
1. `put_bytes` accepts raw `&[u8]` — observations must be serialized to JSON bytes before storage
2. `PutOptions` controls metadata, labels, tags at the memvid level — observation metadata maps here
3. `find()` is always lexical search (no `mode` parameter) — simplifies the trait
4. `ask()` is feature-gated behind `#[cfg(feature = "lex")]` — must enable this feature in Cargo.toml
5. `commit()` replaces the assumed `seal()` — must be called after write operations to persist
6. `frame_by_id()` returns a full `Frame` struct — extract needed fields for our `FrameInfo` internal type
7. All operations take `&mut self` — the `Memvid` handle is inherently mutable

**Alternatives considered**: None — memvid-core is the mandated storage backend per constitution XIII.

## 2. ULID Crate Selection

**Decision**: Use `ulid` crate (latest stable) for observation ID generation.

**Rationale**: ULID was chosen over UUID v4 per clarification session (2026-03-01). ULIDs are lexicographically sortable by creation time, 26-char string representation, 128-bit unique. The `ulid` crate is the most widely used Rust implementation with MIT/Apache dual license.

**Integration notes**:
- `ulid::Ulid::new()` generates a ULID with current timestamp
- `ulid::Ulid::to_string()` produces the 26-char canonical string
- Serde support via `ulid` crate's `serde` feature
- Replaces workspace `uuid` dependency for observation IDs (uuid remains for other crates if needed)

**Alternatives considered**:
- `uuid` v4 — not time-sortable; rejected per clarification
- Sequential `u64` — not globally unique; rejected per clarification

## 3. File Locking Crate Selection

**Decision**: Use `fs2` crate for cross-process advisory file locking.

**Rationale**: `fs2` provides `FileExt::lock_exclusive()` and `FileExt::try_lock_exclusive()` for advisory locking on macOS and Linux. It's widely used, MIT/Apache licensed, and wraps OS-level `flock()` on Unix.

**Integration notes**:
- Open a `.lock` file adjacent to the `.mv2` file
- Use `try_lock_exclusive()` for non-blocking attempt
- Implement exponential backoff on `WouldBlock` errors
- Advisory locks auto-release on process exit (handles stale lock case)
- Lock file permissions should be 0600 per SEC-9

**Alternatives considered**:
- `fd-lock` — similar functionality but less widely adopted; `fs2` preferred for ecosystem familiarity
- In-process `Mutex` only — doesn't protect cross-process access; rejected

## 4. Types Crate Discrepancies

**Decision**: The types crate (`crates/types`) has three discrepancies with the clarification decisions that must be resolved as prerequisites before core engine implementation.

**Findings**:

| Discrepancy | Current State | Required State | Impact |
|-------------|---------------|----------------|--------|
| Error type name | `AgentBrainError` | `RustyBrainError` | All consumers referencing the error type need update. Type alias can ease migration. |
| Observation ID type | `Uuid` (v4) | `Ulid` (from `ulid` crate) | Changes the `id` field type; affects serialization format; tests need update |
| Observation `content` field | `String` (required, non-empty validated) | `Option<String>` (optional per FR-003, EC-4) | Spec says "summary is the minimum required field" and EC-4 says empty content is accepted |

**Resolution approach**: These are prerequisite changes to the types crate that should be completed before core engine implementation. They are tracked as separate tasks in the implementation plan.

## 5. memvid-core Feature Flags

**Decision**: Enable the `lex` feature for `memvid-core` in `Cargo.toml`.

**Rationale**: The `ask()` operation is feature-gated behind `#[cfg(feature = "lex")]`. Since `Mind::ask` is a Must Have requirement (M-4), this feature must be enabled.

**Integration notes**:
- Update `Cargo.toml` workspace dependency: `memvid-core = { git = "...", rev = "...", features = ["lex"] }`
- Verify `ask()` compiles and works with the `lex` feature enabled
- If the `lex` feature doesn't exist or doesn't work as expected, fall back to lexical search only (`find()`) and document the limitation

## 6. Observation Serialization for memvid put

**Decision**: Observations are serialized to JSON bytes for `put_bytes()`. Metadata is conveyed through `PutOptions`.

**Rationale**: memvid's `put_bytes` accepts raw bytes. The observation's human-readable content (summary + content) is the primary payload. Structured metadata (type, ID, timestamp, tool_name, files, platform, etc.) is stored in `PutOptions` metadata field as a JSON value.

**Mapping**:
```
memvid put_bytes payload:  observation.summary + "\n\n" + observation.content (UTF-8 bytes)
PutOptions.labels:         [observation.obs_type as string]
PutOptions.tags:           [observation.tool_name, observation.metadata.session_id, ...]
PutOptions.metadata:       JSON object with all observation fields for faithful round-trip
```

The full observation JSON is stored in `PutOptions.metadata` to enable lossless round-trip retrieval. The text payload enables memvid's lexical search to find observations by content.

**Alternatives considered**:
- Store entire observation JSON as payload — makes search less effective since memvid searches payload text
- Store only summary as payload — loses content for search; less useful
- Selected hybrid approach gives best search + round-trip fidelity

## 7. Concurrency Model

**Decision**: `Mind` is `Send + Sync` via `Arc<Mutex<Memvid>>` for the internal memvid handle. File locking is external to `Mind`.

**Rationale**: memvid-core's `Memvid` handle uses `&mut self` for most operations, so it's inherently not `Sync`. Wrapping in `Mutex` provides thread safety. `Arc` enables sharing across threads. Cross-process locking via `fs2` is layered on top by consumers (hooks, CLI), not inside `Mind` — per AR decision that locking is a consumer concern.

**Alternatives considered**:
- `RwLock` instead of `Mutex` — memvid `find` and `timeline` also take `&mut self`, so read/write differentiation doesn't help
- Locking inside Mind — conflates concerns; single-process usage pays unnecessary overhead
