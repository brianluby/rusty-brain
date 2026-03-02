# Feature Specification: Core Memory Engine

**Feature Branch**: `003-core-memory-engine`
**Created**: 2026-03-01
**Status**: Draft
**Input**: User description: "Review RUST_ROADMAP.md and let's create a spec for phase 2."

## User Scenarios & Testing

### User Story 1 - Store and Retrieve Observations (Priority: P1)

An AI coding agent (Claude Code, OpenCode) is working on a software project. As it uses tools (reads files, runs commands, edits code), each tool interaction generates an observation. The agent needs to store these observations persistently so they survive across sessions. In a later session, the agent needs to search past observations to recall decisions, solutions, and patterns discovered previously.

**Why this priority**: Without the ability to store and retrieve memories, no other feature in the system has value. This is the foundational capability that everything else builds upon.

**Independent Test**: Can be fully tested by storing a set of observations and then retrieving them via search queries, verifying that stored content is returned accurately with correct metadata.

**Acceptance Scenarios**:

1. **Given** a new project with no existing memory file, **When** the engine is initialized and an observation is stored, **Then** a new memory file is created at the configured path and the observation is persisted with its type, summary, content, metadata, and timestamp.
2. **Given** a memory file containing previously stored observations, **When** a search query is executed, **Then** matching observations are returned ranked by relevance, each including the observation type, summary, timestamp, and originating tool.
3. **Given** a memory file with observations across multiple sessions, **When** a question is asked against the memory store, **Then** a synthesized answer is returned based on relevant stored observations, or a clear "no relevant memories found" message if nothing matches.
4. **Given** an observation with all optional metadata fields populated (files, platform, session ID, tags), **When** the observation is retrieved, **Then** all metadata fields are preserved exactly as stored.

---

### User Story 2 - Provide Session Context on Startup (Priority: P1)

When an AI agent starts a new coding session, the memory engine automatically provides relevant context from past sessions. This includes recent observations, relevant memories matching the current project, and summaries of previous sessions. This context helps the agent resume work without losing track of prior decisions and discoveries.

**Why this priority**: Context injection at session start is the primary consumer-facing value proposition. Without it, stored memories are useless to the agent.

**Independent Test**: Can be fully tested by populating a memory store with known observations and session summaries, then requesting context and verifying the returned payload contains the expected recent observations, relevant memories, and session summaries within the configured token budget.

**Acceptance Scenarios**:

1. **Given** a memory file with recent observations, **When** session context is requested, **Then** the most recent observations (up to the configured maximum) are included in the context payload, ordered from newest to oldest.
2. **Given** a memory file with observations and a specific query, **When** context is requested with that query, **Then** both recent observations and query-relevant memories are included, staying within the configured token budget.
3. **Given** a memory file containing session summaries, **When** context is requested, **Then** up to 5 of the most recent session summaries are included in the context payload.
4. **Given** a token budget of N tokens, **When** context is assembled, **Then** the total context payload does not exceed N tokens (estimated at 1 token per 4 characters).

---

### User Story 3 - Save Session Summaries (Priority: P2)

When an AI agent finishes a coding session, it generates a summary capturing the key decisions made, files modified, and overall accomplishments. This summary is stored as a special observation that can be retrieved in future sessions to provide high-level continuity.

**Why this priority**: Session summaries provide compressed, high-signal context that makes future session startups more useful. They depend on the core store/retrieve capability being in place.

**Independent Test**: Can be fully tested by saving a session summary with known decisions and file modifications, then searching for it and verifying it appears in session context.

**Acceptance Scenarios**:

1. **Given** an active session with a known session ID, **When** a session summary is saved with decisions, modified files, and a summary text, **Then** the summary is stored as a searchable observation tagged with the session ID.
2. **Given** multiple saved session summaries, **When** session context is requested, **Then** the most recent summaries are returned in reverse chronological order.

---

### User Story 4 - Report Memory Statistics (Priority: P2)

A developer or agent needs to understand the state of the memory store: how many observations are stored, how many sessions have been recorded, the time range of stored memories, file size, and the distribution of observation types. This helps diagnose issues and understand memory growth over time.

**Why this priority**: Statistics are essential for monitoring and debugging the memory system, but they don't block the core store/retrieve/context workflow.

**Independent Test**: Can be fully tested by populating a memory store with known observations across multiple sessions and types, then requesting stats and verifying all computed values match expectations.

**Acceptance Scenarios**:

1. **Given** a memory file with observations, **When** stats are requested, **Then** the response includes total observation count, total session count, oldest memory timestamp, newest memory timestamp, and file size.
2. **Given** observations of various types, **When** stats are requested, **Then** a breakdown of observation counts by type is included.
3. **Given** stats have been computed once, **When** stats are requested again without new observations, **Then** the cached result is returned without recomputation.

---

### User Story 5 - Handle Corrupted Memory Files Gracefully (Priority: P2)

A developer's memory file has become corrupted (disk error, interrupted write, manual tampering). When the engine attempts to open the file, it detects the corruption, creates a timestamped backup of the corrupted file, and initializes a fresh memory store so the agent can continue working without manual intervention.

**Why this priority**: Data resilience is critical for a system that accumulates long-term knowledge. Users must never be permanently blocked by a corrupted file.

**Independent Test**: Can be fully tested by providing a corrupted memory file, attempting to open the engine, and verifying a backup is created and a fresh store is initialized.

**Acceptance Scenarios**:

1. **Given** a corrupted memory file, **When** the engine attempts to open it, **Then** the corrupted file is renamed to `{filename}.backup-{timestamp}` and a fresh memory file is created.
2. **Given** 4 existing backup files from previous corruptions, **When** a new backup is created, **Then** only the 3 most recent backups are retained and the oldest is deleted.
3. **Given** a memory file larger than 100MB, **When** the engine attempts to open it, **Then** the file is rejected as likely corrupted before attempting to parse it.

---

### User Story 6 - Support Concurrent Access (Priority: P3)

Multiple agent processes may attempt to access the same memory file simultaneously (e.g., parallel worktrees, multiple terminal sessions). The engine must ensure data integrity through cross-process file locking with graceful retry behavior, preventing corruption from concurrent writes.

**Why this priority**: Concurrent access is an advanced scenario that only arises in power-user workflows. The core single-process flow must work first.

**Independent Test**: Can be fully tested by spawning multiple processes that attempt simultaneous writes and reads, verifying no data corruption occurs and all operations eventually succeed.

**Acceptance Scenarios**:

1. **Given** two processes attempting to write to the same memory file simultaneously, **When** one acquires the lock, **Then** the other retries with exponential backoff until the lock is released.
2. **Given** a lock is held by a process that has crashed, **When** another process attempts to acquire the lock, **Then** the OS releases the advisory flock on process exit and acquisition eventually succeeds (stale lock recovery is OS-provided via `flock` semantics, not application-level detection).
3. **Given** the engine is used in a multi-threaded application, **When** multiple threads access the engine concurrently, **Then** all operations are safe and no data races occur.

---

### Edge Cases

- What happens when the configured memory path's parent directory does not exist? The engine creates it automatically.
- What happens when the memory file is on a read-only filesystem? The engine returns a clear error with a stable error code, not a panic.
- What happens when a search query returns no results? An empty result set is returned, not an error.
- What happens when an observation's content is empty? The engine accepts it (summary is the minimum required field).
- What happens when the token budget is exceeded by a single observation? That observation is truncated to fit within the budget.
- What happens when the memory file is deleted between operations? The engine detects the absence and recreates it on the next write operation.

## Clarifications

### Session 2026-03-01

- Q: How should observations be serialized into `.mv2` frames to maintain cross-implementation compatibility? → A: Rust-only schema; drop TypeScript compatibility. The Rust types crate defines the canonical serialization format. No obligation to match the TypeScript agent-brain JSON schema.
- Q: What identifier format should be used for observation IDs? → A: ULID (lexicographically sortable by creation time, 26-char string, 128-bit unique).
- Q: How should "query-relevant memories" be selected for context injection? → A: Use memvid `find` results directly, capped at a configurable max (default 10). No additional re-ranking or score thresholding.
- Q: How should memvid operation failures propagate to callers? → A: Wrap in `RustyBrainError` with stable error codes, preserving the original memvid error as the error source. No direct exposure of memvid errors to callers.
- Q: What latency targets for core operations at 10K observations? → A: Store: 500ms, Search: 500ms, Context assembly: 2s.

## Requirements

### Functional Requirements

- **FR-001**: System MUST create a new memory file when none exists at the configured path, including creating any missing parent directories.
- **FR-002**: System MUST open an existing memory file and make its contents available for all operations (store, search, ask, context, stats).
- **FR-003**: System MUST store observations with required fields (type, tool_name, summary) and optional fields (content, metadata including files, platform, session ID, and arbitrary key-value pairs).
- **FR-004**: System MUST assign a ULID (Universally Unique Lexicographically Sortable Identifier) and timestamp to each stored observation automatically. ULIDs encode creation time and are naturally sortable.
- **FR-005**: System MUST support searching stored observations by text query, returning results ranked by relevance with a configurable result limit.
- **FR-006**: System MUST support question-answering against stored observations, returning a synthesized answer or a "no relevant memories found" fallback.
- **FR-007**: System MUST assemble session context containing recent observations, query-relevant memories (selected via memvid `find`, capped at a configurable maximum defaulting to 10), and session summaries, all within a configurable token budget.
- **FR-008**: System MUST store session summaries as tagged, searchable observations containing session ID, decisions, modified files, and summary text.
- **FR-009**: System MUST compute and return memory statistics including total observations, total sessions, timestamp range, file size, and per-type observation counts.
- **FR-010**: System MUST cache computed statistics and invalidate the cache when new observations are stored.
- **FR-011**: System MUST detect corrupted memory files on open, create timestamped backups, and initialize fresh stores without user intervention.
- **FR-012**: System MUST retain at most 3 backup files, pruning older ones automatically.
- **FR-013**: System MUST reject memory files exceeding 100MB as a corruption safeguard before attempting to parse them.
- **FR-014**: System MUST provide cross-process file locking with retry and exponential backoff to prevent concurrent write corruption.
- **FR-015**: System MUST expose a session identifier for the current session that is consistent across all operations within that session.
- **FR-016**: System MUST expose the resolved memory file path for diagnostic purposes.
- **FR-017**: System MUST provide an initialization status check indicating whether the engine has been successfully opened.
- **FR-018**: System MUST provide a token estimation utility (character count divided by 4) for context budget enforcement.
- **FR-019**: System MUST support a singleton access pattern allowing multiple callers within the same process to share one engine instance.
- **FR-020**: System MUST support resetting the singleton instance for testing purposes.
- **FR-021**: All errors produced by the engine MUST carry stable, machine-parseable error codes from the existing error code system. Memvid errors are wrapped in `RustyBrainError` variants with the original error preserved as the source.

### Key Entities

- **Mind**: The central memory engine that owns the connection to the underlying storage and provides all memory operations. One instance per memory file.
- **Observation**: A single recorded memory entry with a type classification (Discovery, Decision, Problem, Solution, Pattern, Warning, Success, Refactor, Bugfix, Feature), summary, optional content, and extensible metadata.
- **Session Summary**: An aggregated record of one coding session including key decisions, modified files, and a human-readable summary.
- **Injected Context**: The payload assembled for session startup containing recent observations, relevant memories, and session summaries, bounded by a token budget.
- **Memory Search Result**: A search hit containing the matched observation's type, summary, content excerpt, timestamp, relevance score, and originating tool.
- **Mind Statistics**: A snapshot of the memory store's state including counts, timestamps, file size, and type distribution.

## Success Criteria

### Measurable Outcomes

- **SC-001**: An observation can be stored and immediately retrieved via search within the same session, with 100% fidelity on all stored fields.
- **SC-002**: Session context assembly completes and respects the configured token budget, never exceeding it by more than 5%.
- **SC-003**: Corrupted file recovery (detect, backup, recreate) completes without user intervention and the new store is immediately usable.
- **SC-004**: Concurrent access from 2+ processes results in zero data corruption across 100 sequential write operations.
- **SC-005**: Statistics computation for a memory store with 10,000 observations completes in under 2 seconds.
- **SC-008**: Store operation completes in under 500ms for a memory file with 10,000 observations.
- **SC-009**: Search operation completes in under 500ms for a memory file with 10,000 observations.
- **SC-010**: Context assembly completes in under 2 seconds for a memory file with 10,000 observations.
- **SC-006**: All engine operations produce structured errors with stable error codes — no panics or unstructured error messages escape to callers.
- **SC-007**: The observation serialization schema is defined exclusively by the Rust types crate. No cross-implementation compatibility with the TypeScript agent-brain is required.

## Assumptions

- The `memvid` Rust crate (pinned in `Cargo.toml`) provides the native `.mv2` file operations: `create`, `open`, `put`, `commit`, `find`, `ask`, `timeline`, `frame_by_id`, and `stats` (verified in research.md Section 4).
- The types crate (`crates/types`) is complete and provides all shared types: `Observation`, `ObservationType`, `ObservationMetadata`, `SessionSummary`, `InjectedContext`, `MindConfig`, `MindStats`, and `RustyBrainError`.
- The `memvid` crate's `find` operation performs lexical search by default (no mode parameter required in the Rust API; verified in research.md Section 4).
- File locking will use OS-level advisory locks (not mandatory locks).
- Token estimation uses the simple heuristic of character count divided by 4, not a tokenizer model.
- The 100MB file size guard is a heuristic for corruption detection, not a hard architectural limit.

## Dependencies

- **Phase 1 (Type System)**: All shared types must be finalized and passing tests before core engine implementation begins.
- **memvid Rust crate**: The engine directly depends on `memvid-core` (pinned at a specific git revision in `Cargo.toml`).
- **File locking crate**: A cross-platform advisory file locking crate (e.g., `fs2` or `fd-lock`) is needed for concurrent access support.

## Scope Boundaries

### In Scope

- `Mind` struct implementation with all memory operations (remember, search, ask, get_context, save_session_summary, stats)
- Memvid Rust crate integration for native `.mv2` file I/O
- Corrupted file detection, backup, and recovery
- Cross-process file locking with retry/backoff
- Singleton access pattern for multi-caller processes
- Token estimation utility
- Unit and integration tests for all operations

### Out of Scope

- Tool-output compression (Phase 3)
- Platform adapter system (Phase 4)
- Hook binaries that invoke the engine (Phase 5)
- CLI commands (Phase 6)
- Network operations of any kind
- Logging of memory contents at INFO level or above (per project constitution)
