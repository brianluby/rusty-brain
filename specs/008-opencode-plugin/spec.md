# Feature Specification: OpenCode Plugin Adapter

**Feature Branch**: `008-opencode-plugin`
**Created**: 2026-03-03
**Status**: Draft
**Input**: User description: "Port the OpenCode plugin integration: chat hook, tool hook, native mind tool, session cleanup, deduplication (Phase 7 from RUST_ROADMAP.md)"

## User Scenarios & Testing

### User Story 1 - Context Injection in Chat (Priority: P1)

A developer using OpenCode starts a conversation with the AI agent. The chat message hook intercepts the conversation, retrieves relevant context from the memory system (recent observations, session summaries, related memories), and injects it into the conversation so the agent has continuity with previous work.

**Why this priority**: Context injection is the core value proposition of the memory system. Without it, the agent starts every conversation from scratch with no awareness of past sessions. This is the primary read path that delivers stored knowledge back to the developer.

**Independent Test**: Can be fully tested by triggering a chat hook with a test message and verifying that the response includes injected context from a memory file with known observations.

**Acceptance Scenarios**:

1. **Given** a project with an existing memory file containing observations from previous sessions, **When** the developer starts a new chat in OpenCode, **Then** the chat hook injects recent observations and session summaries into the conversation context.
2. **Given** a project with no existing memory file, **When** the developer starts a chat, **Then** the hook creates a new memory file and injects a welcome message indicating the memory system is active.
3. **Given** any error occurs during context retrieval, **When** the chat hook runs, **Then** it fails open — the conversation proceeds normally without injected context, and no error is surfaced to the developer.
4. **Given** a conversation about a specific topic (e.g., "authentication"), **When** the chat hook processes the message, **Then** it includes topic-relevant memories in addition to recent observations.

---

### User Story 2 - Tool Observation Capture (Priority: P1)

When the AI agent in OpenCode executes a tool (reads files, runs commands, edits code), the tool execution hook captures a compressed observation and stores it in memory. This builds the project's knowledge base throughout the session.

**Why this priority**: This is the primary write path. Without tool observation capture, the memory system never accumulates new knowledge and becomes stale after the initial session.

**Independent Test**: Can be fully tested by triggering a tool hook with a simulated tool execution and verifying an observation is stored in the memory file.

**Acceptance Scenarios**:

1. **Given** the agent executes a file read tool, **When** the tool hook receives the execution result, **Then** it stores a compressed observation with the correct observation type, tool name, and summary.
2. **Given** the same tool+summary combination was already captured in this session, **When** the tool hook receives a duplicate execution, **Then** it skips storage (deduplication) and does not create a redundant observation.
3. **Given** any error occurs during observation capture, **When** the tool hook runs, **Then** it fails open — the tool execution completes normally and no error is surfaced to the developer.
4. **Given** a large tool output (e.g., a file with thousands of lines), **When** the tool hook processes it, **Then** the stored observation content is compressed to approximately 500 tokens (tolerance inherited from `crates/compression::compress()` behavior).

---

### User Story 3 - Native Mind Tool (Priority: P1)

The developer (or the AI agent on behalf of the developer) can interact with the memory system directly through a native `mind` tool integrated into OpenCode. This tool supports multiple modes: searching memories, asking questions, viewing recent activity, checking statistics, and manually storing observations.

**Why this priority**: The native tool gives developers and agents direct, on-demand access to the memory system. While hooks provide automatic background capture and injection, the native tool enables intentional, explicit interactions — the developer actively choosing to search, ask, or remember something.

**Independent Test**: Can be fully tested by invoking the mind tool with each mode (search, ask, recent, stats, remember) and verifying correct results are returned.

**Acceptance Scenarios**:

1. **Given** a memory file with stored observations, **When** the mind tool is invoked in `search` mode with a query, **Then** matching observations are returned with type, timestamp, summary, and content excerpt.
2. **Given** a memory file with stored observations, **When** the mind tool is invoked in `ask` mode with a question, **Then** a synthesized answer is returned drawing from relevant memories.
3. **Given** a memory file with recent activity, **When** the mind tool is invoked in `recent` mode, **Then** the most recent observations are returned in reverse chronological order.
4. **Given** a memory file, **When** the mind tool is invoked in `stats` mode, **Then** memory statistics are returned (total observations, sessions, date range, file size, type breakdown).
5. **Given** a user wants to manually store a note, **When** the mind tool is invoked in `remember` mode with content, **Then** a new observation is stored in memory and confirmation is returned.

---

### User Story 4 - Session Cleanup (Priority: P2)

When a developer deletes a session or conversation in OpenCode, the plugin performs cleanup: generating a session summary, storing final observations, and gracefully releasing the memory file.

**Why this priority**: Session cleanup ensures data integrity and creates high-value session summaries for future reference. Less critical than the core read/write/tool paths, but important for long-term memory quality.

**Independent Test**: Can be fully tested by triggering a session deletion event and verifying a session summary is stored and the memory file is properly released.

**Acceptance Scenarios**:

1. **Given** an active session with captured observations, **When** the session is deleted in OpenCode, **Then** a session summary is generated and stored including observation count, key decisions, and files modified.
2. **Given** an active session with no observations, **When** the session is deleted, **Then** a minimal session summary is stored and the memory file is gracefully released.
3. **Given** any error during cleanup, **When** the session deletion occurs, **Then** the deletion completes normally (fail-open) and the error is logged for diagnostics.

---

### User Story 5 - Plugin Registration and Discovery (Priority: P2)

OpenCode discovers and loads the rusty-brain plugin through a manifest file. The manifest declares the plugin's capabilities (chat hook, tool hook, native tool), the binary paths, and metadata needed for OpenCode to integrate the plugin.

**Why this priority**: Without registration, OpenCode cannot discover the plugin. This is a prerequisite for all other functionality, but it's lower priority for specification because the format is dictated by OpenCode's plugin system.

**Independent Test**: Can be tested by validating the manifest file against OpenCode's plugin schema and verifying OpenCode loads the plugin without errors.

**Acceptance Scenarios**:

1. **Given** a correctly installed rusty-brain plugin, **When** OpenCode loads its plugin registry, **Then** it discovers the rusty-brain plugin with its declared capabilities (chat hook, tool hook, mind tool).
2. **Given** the plugin binary is missing or inaccessible, **When** OpenCode attempts to load the plugin, **Then** OpenCode receives a clear error indicating the binary was not found.

---

### Edge Cases

- What happens when the memory file is locked by a concurrent Claude Code session? The plugin must retry with backoff or fail-open, sharing the same cross-process locking mechanism as the hooks system.
- What happens when OpenCode sends an unrecognized event type? The plugin must ignore it gracefully without crashing.
- What happens when the mind tool receives an invalid mode? It must return a clear error listing the valid modes (search, ask, recent, stats, remember).
- What happens when deduplication cache grows during a very long session? The cache is bounded to 1024 entries with LRU eviction; oldest entries are evicted first, which may allow rare re-observation of very old tool outputs.
- What happens when the plugin is invoked outside of a project directory? It must resolve the memory path using the current working directory as fallback.
- What happens when OpenCode's plugin protocol changes? The plugin must handle unknown fields in input gracefully (forward compatibility).

## Requirements

### Functional Requirements

- **FR-001**: The plugin MUST provide a chat message hook that injects relevant context (recent observations, session summaries, topic-relevant memories) into conversations.
- **FR-002**: The plugin MUST provide a tool execution hook that captures compressed observations after each tool execution.
- **FR-003**: The plugin MUST provide a native `mind` tool with five modes: `search`, `ask`, `recent`, `stats`, and `remember`.
- **FR-004**: The `mind` tool `search` mode MUST accept a query string and return matching observations with type, timestamp, summary, and content excerpt.
- **FR-005**: The `mind` tool `ask` mode MUST accept a natural language question and return a synthesized answer from memory.
- **FR-006**: The `mind` tool `recent` mode MUST return the most recent observations in reverse chronological order.
- **FR-007**: The `mind` tool `stats` mode MUST return memory statistics: total observations, total sessions, date range, file size, and type breakdown.
- **FR-008**: The `mind` tool `remember` mode MUST accept content and store it as a new observation in memory.
- **FR-009**: The tool execution hook MUST deduplicate observations within a session using a bounded, session-scoped sidecar file (e.g., `.opencode/session-<id>.json`) keyed on tool name and summary hash, persisted across subprocess invocations.
- **FR-010**: The plugin MUST perform session cleanup on deletion: generate and store a session summary, then gracefully release the memory file.
- **FR-011**: All hooks MUST fail-open — any internal error must not block the developer's workflow in OpenCode. Errors MUST be emitted via `tracing` at WARN level to stderr for diagnostics.
- **FR-012**: The plugin MUST provide a manifest file for OpenCode plugin discovery and registration.
- **FR-013**: The plugin MUST resolve the correct memory file for the current project using LegacyFirst mode (`.agent-brain/mind.mv2`), sharing memory across all platforms.
- **FR-014**: The plugin MUST handle unknown fields in input gracefully for forward compatibility with future OpenCode protocol changes.
- **FR-015**: On session start, the plugin MUST scan for and delete orphaned sidecar session files older than 24 hours to prevent accumulation from abnormal terminations.

### Key Entities

- **Plugin Manifest**: A configuration file declaring the plugin's capabilities, binary paths, and metadata for OpenCode to discover and load the plugin.
- **Chat Hook**: An integration point that intercepts conversations to inject memory context.
- **Tool Hook**: An integration point that captures observations after tool executions.
- **Mind Tool**: A native tool exposed to OpenCode's AI agent with five operational modes (search, ask, recent, stats, remember).
- **Session-Scoped Deduplication Cache**: A bounded, file-backed sidecar (e.g., `.opencode/session-<id>.json`) that prevents duplicate observations within a single session, keyed on tool name + summary hash. Persists across subprocess invocations and is cleaned up on session end.

## Success Criteria

### Measurable Outcomes

- **SC-001**: Context injection adds relevant memories to conversations within 200ms, causing no perceptible delay to the developer.
- **SC-002**: Tool observation capture completes within 100ms per tool execution, never blocking the agent's workflow.
- **SC-003**: All five `mind` tool modes (search, ask, recent, stats, remember) return correct results when invoked against a memory file with known contents.
- **SC-004**: Deduplication prevents 100% of duplicate observations within a session — the same tool+summary combination produces at most one stored observation per session.
- **SC-005**: No plugin operation ever causes an error visible to the developer or blocks OpenCode's normal operation (fail-open verified across all error paths).
- **SC-006**: Session cleanup produces a session summary that includes observation count and is retrievable by future sessions.
- **SC-007**: The plugin manifest is valid and results in successful plugin loading by OpenCode.

## Clarifications

### Session 2026-03-03

- Q: How should the plugin handle state persistence (dedup cache, session state) across hook invocations within a session? → A: Sidecar file per session (e.g., `.opencode/session-<id>.json`) that persists dedup state and session metadata across subprocess invocations; cleaned up on session end.
- Q: Should OpenCode use shared memory (legacy path) or platform-isolated memory? → A: Shared memory (`.agent-brain/mind.mv2`, LegacyFirst mode) so observations are available across all platforms (Claude Code, OpenCode, etc.).
- Q: Where should fail-open errors be reported? → A: Via `tracing` at WARN level to stderr, matching existing project convention (004-tool-output-compression). Stderr does not interfere with stdout-based JSON protocol.
- Q: What should the maximum dedup cache size be? → A: 1024 entries with LRU eviction.
- Q: How should orphaned sidecar session files be handled? → A: Stale cleanup on session start — scan for sidecar files older than 24 hours and delete them. Self-healing, no background process needed.

## Assumptions

- The `crates/core` memory engine (`Mind`, `get_mind`) is complete and provides search, ask, get_context, remember, save_session_summary, and stats APIs.
- The `crates/platforms` adapter system is complete and provides the OpenCode platform adapter, detection, identity resolution, and path policy.
- The `crates/compression` crate is complete and provides tool output compression.
- OpenCode's plugin system supports binary plugins invoked as subprocesses with structured (JSON) input/output. If OpenCode requires a different integration pattern (e.g., shared library, WebSocket), the manifest and invocation layer will be adapted but the core logic remains the same.
- The deduplication cache is session-scoped (reset on session start), bounded to 1024 entries with LRU eviction, and persisted as a sidecar file to survive across subprocess invocations within a session.
- The `mind` tool's `remember` mode stores observations with type `Discovery`. An optional type override is deferred (PRD C-1, Could Have).
