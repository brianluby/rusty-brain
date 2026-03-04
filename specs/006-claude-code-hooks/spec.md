# Feature Specification: Claude Code Hooks

**Feature Branch**: `006-claude-code-hooks`
**Created**: 2026-03-02
**Status**: Draft
**Input**: User description: "Build the four hook binaries that Claude Code invokes as subprocess commands (Phase 5 from RUST_ROADMAP.md)"

## Clarifications

### Session 2026-03-03

- Q: How should the four hooks be packaged — single binary with subcommands, four separate binaries, or single binary auto-detecting from stdin? → A: Single binary with subcommands (e.g., `rusty-brain session-start`).
- Q: How should debug/diagnostic logging work given FR-009's no-stderr-in-normal-operation constraint? → A: Env-var controlled logging to stderr (`RUSTY_BRAIN_LOG=debug`), silent by default.
- Q: What level of data-at-rest protection should apply to stored memory content? → A: Use memvid's built-in encryption for `.mv2` files.

## User Scenarios & Testing

### User Story 1 - Session Context Injection (Priority: P1)

A developer starts a new Claude Code session in a project that has existing memory. The session-start hook initializes the memory system, detects the platform, resolves the correct memory file for this project, and injects recent observations and session summaries into the agent's system prompt so the agent has continuity with previous work.

**Why this priority**: Without context injection, the memory system has no way to deliver value. This is the primary interface between stored memories and the agent — it's the entire reason the system exists.

**Independent Test**: Can be fully tested by invoking the session-start binary with a valid JSON payload on stdin and verifying that structured context appears in the JSON output on stdout.

**Acceptance Scenarios**:

1. **Given** a project with an existing `.mv2` memory file containing 10+ observations, **When** the session-start hook receives a valid hook input on stdin, **Then** it returns a `HookOutput` with a `systemMessage` containing recent observations, session summaries, and available commands.
2. **Given** a project with no existing memory file, **When** the session-start hook runs, **Then** it creates a new memory file, returns a welcome message in `systemMessage`, and exits with code 0.
3. **Given** any error occurs during initialization, **When** the session-start hook runs, **Then** it returns a valid `HookOutput` with `continue` set to `true` (fail-open) and exits with code 0.
4. **Given** a project with a legacy memory path (`.claude/mind.mv2`), **When** the session-start hook runs, **Then** the `systemMessage` includes a migration suggestion to move to `.agent-brain/mind.mv2`.

---

### User Story 2 - Tool Observation Capture (Priority: P1)

After the agent executes a tool (e.g., reads a file, runs a command, edits code), the post-tool-use hook captures a compressed observation of what happened and stores it in memory. This builds the agent's knowledge base over the course of a session.

**Why this priority**: This is the primary write path for the memory system. Without tool observation capture, no new memories are created and the system becomes read-only.

**Independent Test**: Can be fully tested by invoking the post-tool-use binary with tool execution JSON on stdin and verifying an observation was stored (by subsequently querying the memory file).

**Acceptance Scenarios**:

1. **Given** the agent just ran a Read tool on a source file, **When** the post-tool-use hook receives the tool input and response on stdin, **Then** it stores a compressed observation with the correct observation type, tool name, and summary, and returns a `HookOutput` with `continue` set to `true`.
2. **Given** the same tool+summary combination was captured within the last 60 seconds, **When** the post-tool-use hook receives a duplicate, **Then** it skips storage (deduplication) and returns a valid `HookOutput`.
3. **Given** any error occurs during observation capture, **When** the post-tool-use hook runs, **Then** it returns a valid `HookOutput` with `continue` set to `true` (fail-open) and never blocks the agent's tool execution.
4. **Given** a tool output exceeding 2000 characters, **When** the post-tool-use hook processes it, **Then** the stored observation content is truncated to approximately 500 tokens using head/tail truncation.

---

### User Story 3 - Session Summary and Shutdown (Priority: P2)

When the agent session ends, the stop hook captures a summary of the session including files modified, key decisions made, and total observations. This summary is stored for future sessions to reference.

> **MVP Note**: For MVP, the "decisions" field is an empty list (`Vec::new()`). The stop hook focuses on file modifications and observation count; decision extraction from the conversation transcript is deferred to a future phase.

**Why this priority**: Session summaries provide high-value, condensed context for future sessions. Less critical than the core read/write paths (US1/US2) but essential for long-term memory quality.

**Independent Test**: Can be fully tested by invoking the stop binary with a session-end JSON payload and verifying the session summary is stored and includes file modifications detected from git.

**Acceptance Scenarios**:

1. **Given** a session where the agent made code changes, **When** the stop hook runs, **Then** it detects modified files (via git diff), generates a session summary, stores it in memory, and returns a `HookOutput` with a summary message.
2. **Given** a session with no code changes, **When** the stop hook runs, **Then** it still stores a session summary (possibly noting no files were changed) and gracefully shuts down the mind instance.
3. **Given** a session where individual file edits were captured, **When** the stop hook runs, **Then** each edited file is stored as a separate observation for fine-grained searchability.
4. **Given** any error during summary generation, **When** the stop hook runs, **Then** it fails open, performs graceful mind shutdown, and exits with code 0.

---

### User Story 4 - Installation and Version Management (Priority: P3)

When Claude Code invokes the smart-install hook (typically at first use or after updates), the system verifies the binary is up-to-date and performs any necessary setup. For the Rust version, there is no `npm install` step — this is a lightweight version-check and marker-file operation.

**Why this priority**: This is largely a no-op for the Rust binary distribution but needed for completeness and self-update scenarios.

**Independent Test**: Can be fully tested by invoking the smart-install binary and verifying it writes a version marker file and exits cleanly.

**Acceptance Scenarios**:

1. **Given** a fresh installation (no `.install-version` file exists), **When** the smart-install hook runs, **Then** it writes the current binary version to `.install-version` and exits with code 0.
2. **Given** `.install-version` matches the current binary version, **When** the smart-install hook runs, **Then** it exits immediately with no changes (no-op fast path).
3. **Given** any error occurs, **When** the smart-install hook runs, **Then** it fails open and never blocks session startup.

---

### User Story 5 - Hook Registration Manifest (Priority: P2)

The system provides a `hooks.json` manifest file that tells Claude Code which hooks exist, what events they respond to, and where the binaries are located. This is how Claude Code discovers and invokes the hooks.

**Why this priority**: Without registration, Claude Code cannot discover and invoke the hooks. This is a prerequisite for any hook to function in production, though hooks can be tested independently without it.

**Independent Test**: Can be tested by validating the generated `hooks.json` against the Claude Code hook registration schema.

**Acceptance Scenarios**:

1. **Given** a correctly installed rusty-brain, **When** Claude Code reads `hooks.json`, **Then** it finds entries for `session-start`, `post-tool-use`, `stop`, and `smart-install` with correct binary paths and event types.
2. **Given** the binary location changes (e.g., different install directory), **When** a generation command is run, **Then** `hooks.json` is updated with the correct paths.

---

### Edge Cases

- What happens when stdin is empty or contains invalid JSON? Hook must fail-open with a valid HookOutput.
- What happens when the memory file is locked by another process? Hook must retry with backoff or fail-open.
- What happens when the memory file is corrupted? The core `Mind::open` handles recovery; hooks must handle the error propagation gracefully.
- What happens when the hook binary is invoked with an unknown `hook_event_name`? It must return a valid HookOutput and not crash.
- What happens when git is not available in the PATH during the stop hook? It must handle missing git gracefully and skip file-modification detection.
- What happens when stdin JSON contains unknown fields? HookInput already supports forward-compatible deserialization.
- What happens when multiple Claude Code sessions run concurrently in the same project? Cross-process file locking (already in core) handles this; hooks must not assume exclusive access.

## Requirements

### Functional Requirements

- **FR-001**: Each hook binary MUST read a single JSON object from stdin, process it, and write a single JSON object to stdout.
- **FR-002**: All hooks MUST fail-open — any internal error must result in a valid `HookOutput` with `continue` set to `true` and exit code 0.
- **FR-003**: The session-start hook MUST initialize the memory system, detect the platform, resolve the correct memory file, and return recent context in `systemMessage`.
- **FR-004**: The post-tool-use hook MUST capture tool observations with compressed content and store them in memory.
- **FR-005**: The post-tool-use hook MUST deduplicate observations within a 60-second window based on a hash of tool name and summary.
- **FR-006**: The stop hook MUST detect file modifications, generate a session summary, store individual edits, and gracefully shut down the mind instance.
- **FR-007**: The smart-install hook MUST track installation state via a `.install-version` marker file.
- **FR-008**: The system MUST provide a `hooks.json` manifest for Claude Code hook discovery and registration.
- **FR-009**: All hooks MUST produce structured, machine-parseable output — no interactive prompts, no unstructured text to stderr in normal operation.
- **FR-010**: All hooks MUST complete within a reasonable time bound — hooks must not cause noticeable delay to the agent's workflow.
- **FR-011**: The post-tool-use hook MUST support all standard tool types: Read, Edit, Write, Bash, Grep, Glob, WebFetch, and any unknown tool type via a generic fallback.
- **FR-012**: The session-start hook MUST include available commands and skills in its context injection.
- **FR-013**: The stop hook MUST store each file modification as a separate observation for granular searchability.
- **FR-014**: Diagnostic logging MUST be controlled via the `RUSTY_BRAIN_LOG` environment variable (e.g., `RUSTY_BRAIN_LOG=debug`), outputting to stderr. Logging MUST be silent by default (no output when the variable is unset).
- **FR-015**: Memory files MUST use memvid's built-in encryption for data-at-rest protection of stored observations and session summaries.

### Key Entities

- **Hook Binary**: A single `rusty-brain` executable with subcommands (`session-start`, `post-tool-use`, `stop`, `smart-install`) that dispatches to the appropriate hook handler based on the subcommand.
- **Event Type → Subcommand Mapping**: Claude Code event names map to binary subcommands as follows:

  | Claude Code Event | Subcommand |
  |-------------------|------------|
  | SessionStart | `session-start` |
  | PostToolUse | `post-tool-use` |
  | Stop | `stop` |
  | Notification | `smart-install` |
- **Hook Manifest** (`hooks.json`): A JSON configuration file mapping event types to binary paths, enabling Claude Code to discover and invoke hooks.
- **Deduplication Window**: A time-based (60-second) cache keyed on tool+summary hash that prevents storing duplicate observations from repeated tool calls.
- **Version Marker** (`.install-version`): A file tracking the installed binary version for update detection.

## Success Criteria

### Measurable Outcomes

- **SC-001**: All four hooks accept valid `HookInput` JSON on stdin and produce valid `HookOutput` JSON on stdout for every input, including malformed or empty input.
- **SC-002**: Session-start hook delivers context (recent observations and session summaries) to the agent within 200ms of invocation for a memory file with up to 1,000 observations.
- **SC-003**: Post-tool-use hook completes observation storage within 100ms for typical tool outputs, causing no perceptible delay to the developer's workflow.
- **SC-004**: No hook invocation ever returns a non-zero exit code or produces output that would block the host agent from continuing.
- **SC-005**: Deduplication correctly prevents redundant observations — the same tool+summary combination within 60 seconds produces at most one stored observation.
- **SC-006**: Stop hook captures all modified files detected by git diff and stores each as a searchable observation.
- **SC-007**: `hooks.json` is valid JSON and contains correct entries for all four hook event types with accurate binary paths.

## Assumptions

- The `crates/core` memory engine (`Mind`, `get_mind`, `reset_mind`) is complete and provides the full API needed by hooks (remember, search, get_context, save_session_summary, stats).
- The `crates/platforms` adapter system is complete and provides platform detection, identity resolution, and memory path resolution.
- The `crates/types` hook protocol types (`HookInput`, `HookOutput`) are complete and match the Claude Code JSON protocol.
- Tool-output compression will be available from a compression module (either within hooks or as a dependency). If the compression crate (Phase 3) is not yet implemented, hooks will use a basic head/tail truncation fallback.
- Git is the assumed version control system for file-modification detection in the stop hook. Non-git projects fall back to no file-modification tracking.
- The smart-install hook is intentionally minimal for the Rust binary distribution — it does not perform package installation, only version tracking.
