# Contract: Mind::timeline() API Extension

**Date**: 2026-03-02 | **Source**: AR §Core API Extension

## Scope

Add one new public method and one new public type to `crates/core::mind`. No trait changes. No changes to `MemvidBackend`.

## New Public Type: TimelineEntry

**Location**: `crates/core/src/mind.rs`

```rust
/// A single timeline entry representing a stored observation.
///
/// Returned by [`Mind::timeline()`]. Contains parsed metadata from the
/// underlying backend frame.
#[derive(Debug, Clone)]
pub struct TimelineEntry {
    /// The observation type (discovery, decision, etc.)
    pub obs_type: ObservationType,
    /// Human-readable summary of the observation
    pub summary: String,
    /// When the observation was recorded
    pub timestamp: DateTime<Utc>,
    /// The tool that generated this observation
    pub tool_name: String,
}
```

### Design decisions

- No `Serialize` derive: CLI defines its own serializable mirror type
- No `frame_id` field: internal backend detail, not exposed
- No `content_excerpt` field: timeline entries show summary only (content available via `find`)
- `tool_name` included: useful context for developers reviewing timeline

## New Public Method: Mind::timeline()

**Location**: `crates/core/src/mind.rs`

```rust
impl Mind {
    /// Query timeline entries.
    ///
    /// Returns observations parsed from backend frames. When `reverse` is
    /// `true`, entries are ordered most-recent-first (default CLI behavior).
    /// When `false`, entries are ordered oldest-first.
    ///
    /// The `limit` parameter controls the maximum number of entries returned.
    ///
    /// # Errors
    ///
    /// Returns [`RustyBrainError::Storage`] if the backend timeline or
    /// frame lookup fails.
    #[tracing::instrument(skip(self))]
    pub fn timeline(
        &self,
        limit: usize,
        reverse: bool,
    ) -> Result<Vec<TimelineEntry>, RustyBrainError> {
        // Implementation delegates to:
        //   self.backend.timeline(limit, reverse)
        //   self.backend.frame_by_id(entry.frame_id)
        // Same iteration pattern as Mind::stats()
    }
}
```

### Behavior contract

| Input | Behavior |
|-------|----------|
| `limit = 10, reverse = true` | Returns up to 10 entries, most recent first |
| `limit = 10, reverse = false` | Returns up to 10 entries, oldest first |
| `limit = 0` | Returns empty vec (caller validates limit > 0) |
| Empty memory file | Returns empty vec, no error |
| Corrupted frame metadata | Skips frame with best-effort defaults (obs_type = Discovery, summary = preview text, timestamp = now) |

### Error cases

| Condition | Error |
|-----------|-------|
| Backend timeline query fails | `RustyBrainError::Storage` |
| Backend frame_by_id fails | `RustyBrainError::Storage` |

### Implementation pattern

Follows `Mind::stats()` (lines 292-369):
1. Call `self.backend.timeline(limit, reverse)` to get `Vec<backend::TimelineEntry>`
2. For each entry, call `self.backend.frame_by_id(entry.frame_id)` to get `FrameInfo`
3. Parse `obs_type`, `summary`, `timestamp`, `tool_name` from `FrameInfo.metadata` JSON
4. Return `Vec<TimelineEntry>`

### Metadata JSON structure (from Mind::remember)

```json
{
  "id": "<ulid>",
  "obs_type": "<lowercase string>",
  "tool_name": "<string>",
  "summary": "<string>",
  "content": "<string|null>",
  "timestamp": "<RFC 3339 string>",
  "session_id": "<ulid>",
  "metadata": "<ObservationMetadata|null>"
}
```

### Fallback behavior for missing/malformed metadata

| Field | Fallback |
|-------|----------|
| `obs_type` | `ObservationType::Discovery` |
| `summary` | `backend::TimelineEntry.preview` (100-char truncation) |
| `timestamp` | `Utc::now()` |
| `tool_name` | `"unknown"` |

## Testing requirements

| Test | Description |
|------|-------------|
| Unit: empty memory | `Mind::timeline(10, true)` returns empty vec |
| Unit: reverse order | Entries returned most-recent-first when `reverse = true` |
| Unit: chronological order | Entries returned oldest-first when `reverse = false` |
| Unit: limit respected | With 20 entries, `timeline(5, true)` returns exactly 5 |
| Unit: metadata parsing | All fields correctly extracted from backend metadata JSON |
| Unit: malformed metadata | Fallback values used when metadata fields are missing |
| Integration: round-trip | `remember()` → `timeline()` → verify entry matches original observation |

## Impact on existing code

- No changes to `MemvidBackend` trait
- No changes to `MockBackend` (already implements `timeline()` and `frame_by_id()`)
- No changes to existing `Mind` methods
- New method added after `stats()` in `mind.rs`
