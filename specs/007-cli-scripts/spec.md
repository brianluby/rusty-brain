# Feature Specification: CLI Scripts

**Feature Branch**: `007-cli-scripts`
**Created**: 2026-03-02
**Status**: Draft
**Input**: User description: "Provide developer-facing CLI tools for interacting with the memory system: find, ask, stats, timeline (Phase 6 from RUST_ROADMAP.md)"

## User Scenarios & Testing

### User Story 1 - Search Memories by Pattern (Priority: P1)

A developer wants to find specific memories from past sessions — for example, all observations related to "authentication" or a specific file path. They run a search command from the terminal and get a list of matching memories with their type, timestamp, and content excerpt.

**Why this priority**: Search is the most fundamental way developers interact with their memory outside of the agent. It enables debugging, auditing, and knowledge retrieval without starting a full agent session.

**Independent Test**: Can be fully tested by running the find command against a memory file with known observations and verifying matching results appear with correct formatting.

**Acceptance Scenarios**:

1. **Given** a memory file with 50+ observations spanning multiple types and sessions, **When** the developer runs `rusty-brain find "authentication"`, **Then** matching observations are displayed with observation type, timestamp, summary, and a content excerpt, ordered by relevance.
2. **Given** a search pattern that matches no observations, **When** the developer runs `rusty-brain find "xyznonexistent"`, **Then** a clear "no results found" message is displayed.
3. **Given** the developer wants to limit results, **When** they run `rusty-brain find "error" --limit 5`, **Then** at most 5 results are returned.
4. **Given** the developer wants machine-readable output, **When** they run `rusty-brain find "auth" --json`, **Then** results are output as a JSON array suitable for piping to other tools.
5. **Given** the developer wants only decisions, **When** they run `rusty-brain find "auth" --type decision`, **Then** only observations with type `decision` are returned.

---

### User Story 2 - Ask Questions About Memory (Priority: P1)

A developer wants to ask a natural language question about their project history — for example, "What decisions were made about the database schema?" The system searches memory and returns a synthesized answer drawing from relevant observations.

**Why this priority**: Question-answering provides the highest-value interaction for developers. Instead of manually sifting through search results, they get a direct answer. This is the "killer feature" for developer adoption.

**Independent Test**: Can be fully tested by running the ask command with a question and verifying a coherent answer is returned that draws from stored observations.

**Acceptance Scenarios**:

1. **Given** a memory file with observations about database decisions, **When** the developer runs `rusty-brain ask "What database schema changes were made?"`, **Then** a synthesized answer is displayed that references relevant observations.
2. **Given** a question with no relevant memories, **When** the developer runs `rusty-brain ask "What about quantum computing?"`, **Then** a clear message indicates no relevant memories were found.
3. **Given** the developer wants machine-readable output, **When** they run `rusty-brain ask "..." --json`, **Then** the answer is output as a JSON object.

---

### User Story 3 - View Memory Statistics (Priority: P2)

A developer wants to understand the state of their memory file — how many observations are stored, how many sessions, the time range covered, file size, and breakdown by observation type. This helps with monitoring and maintenance.

**Why this priority**: Statistics provide essential visibility into the memory system. Developers need to know their memory is working, growing, and not corrupted. Less critical than search/ask since it doesn't retrieve specific knowledge.

**Independent Test**: Can be fully tested by running the stats command against a memory file and verifying the displayed counts, dates, and breakdowns are accurate.

**Acceptance Scenarios**:

1. **Given** a memory file with observations from multiple sessions, **When** the developer runs `rusty-brain stats`, **Then** a summary is displayed showing total observations, total sessions, oldest and newest memory timestamps, file size, and a breakdown of observation counts by type.
2. **Given** an empty or newly created memory file, **When** the developer runs `rusty-brain stats`, **Then** it shows zero counts with appropriate messaging rather than an error.
3. **Given** the developer wants machine-readable output, **When** they run `rusty-brain stats --json`, **Then** statistics are output as a JSON object.

---

### User Story 4 - View Chronological Timeline (Priority: P2)

A developer wants to see a chronological view of recent memory entries — a timeline of what happened across recent sessions. This is useful for reviewing what the agent did, debugging unexpected behavior, or recapping past work.

**Why this priority**: Timeline provides temporal context that search alone cannot. Seeing what happened in order helps developers understand the narrative of their sessions. Slightly less critical than stats since it overlaps somewhat with search.

**Independent Test**: Can be fully tested by running the timeline command and verifying entries appear in reverse chronological order with correct timestamps and types.

**Acceptance Scenarios**:

1. **Given** a memory file with observations from 3 sessions, **When** the developer runs `rusty-brain timeline`, **Then** observations are displayed in reverse chronological order (most recent first) with timestamp, observation type, and summary.
2. **Given** the developer wants to see more or fewer entries, **When** they run `rusty-brain timeline --limit 50`, **Then** at most 50 entries are displayed.
3. **Given** the developer wants the oldest entries first, **When** they run `rusty-brain timeline --oldest-first`, **Then** entries are displayed in chronological order (oldest first).
4. **Given** the developer wants machine-readable output, **When** they run `rusty-brain timeline --json`, **Then** entries are output as a JSON array.
5. **Given** the developer wants only discoveries, **When** they run `rusty-brain timeline --type discovery`, **Then** only observations with type `discovery` are displayed.

---

### User Story 5 - CLI Error Handling & Help (Priority: P2)

A developer encounters an error condition (missing file, invalid arguments, locked file) or needs to discover available commands. The CLI provides clear, actionable feedback in all cases.

**Why this priority**: Good error handling and discoverability are essential for developer adoption. A CLI that produces cryptic errors or stack traces will be abandoned.

**Independent Test**: Can be fully tested by triggering each error condition and verifying the output is a clear message (not a stack trace), and by running `--help` and verifying usage information is displayed.

**Acceptance Scenarios**:

1. **Given** the developer runs `rusty-brain` with no arguments, **When** the CLI starts, **Then** help text is displayed showing available subcommands and usage examples.
2. **Given** the memory file does not exist at the resolved path, **When** any subcommand is run, **Then** a clear error message indicates no memory file was found with a suggestion for where to create one.
3. **Given** the memory file is locked by an active agent session, **When** any subcommand is run, **Then** the CLI waits with exponential backoff (100ms base, 5 retries, 2x multiplier) and displays a clear error after retries are exhausted.
4. **Given** the developer passes `--limit 0` or `--limit -1`, **When** the subcommand processes arguments, **Then** a clear error indicates the limit must be a positive integer.

---

### Edge Cases

- What happens when the memory file does not exist at the expected path? Clear error message indicating no memory file found, with suggestion on where to create one.
- What happens when the memory file is locked by an active agent session? The CLI MUST wait using exponential backoff (reusing the core engine's `with_lock` pattern: 100ms base, 5 retries, 2x multiplier) and display a clear error message after all retries are exhausted.
- What happens when the memory file is corrupted? The core engine handles recovery; the CLI should surface a user-friendly error and suggest running the system to trigger auto-recovery.
- What happens when no subcommand is provided? Display help text showing available commands and usage examples.
- What happens when the `--limit` value is zero or negative? Treat as invalid input with a clear error message.
- What happens when the terminal does not support color output? Detect non-interactive terminals and disable color/formatting automatically (e.g., when piped to another command).

## Requirements

### Functional Requirements

- **FR-001**: The CLI MUST provide a `find` subcommand that searches memories by text pattern and displays matching results with observation type, timestamp, summary, and content excerpt.
- **FR-002**: The CLI MUST provide an `ask` subcommand that accepts a natural language question and returns a synthesized answer from memory.
- **FR-003**: The CLI MUST provide a `stats` subcommand that displays memory statistics: total observations, total sessions, oldest/newest timestamps, file size, and type breakdown.
- **FR-004**: The CLI MUST provide a `timeline` subcommand that displays observations in chronological order with configurable direction and limit.
- **FR-005**: All subcommands MUST support a `--json` flag for machine-readable structured output suitable for piping to other tools.
- **FR-006**: All subcommands MUST support a `--limit <N>` flag to control the number of results returned (where applicable). The default limit when not specified MUST be 10 for both `find` and `timeline`. The `ask` subcommand is exempt (`--limit` not applicable) since it returns a single synthesized answer.
- **FR-007**: The CLI MUST automatically detect the correct memory file path for the current project using the same resolution logic as the hook and platform systems.
- **FR-008**: The CLI MUST provide a `--memory-path <path>` flag to override automatic memory file detection.
- **FR-009**: The CLI MUST display a help message with usage examples when invoked with no arguments or with `--help`.
- **FR-010**: The CLI MUST provide clear, user-friendly error messages for all failure modes (missing file, invalid arguments, corrupted memory).
- **FR-011**: The CLI MUST automatically disable color and formatting when output is piped to another command (non-interactive terminal detection).
- **FR-012**: All subcommands MUST exit with code 0 on success and a non-zero exit code on failure, following standard CLI conventions.
- **FR-013**: The `find` and `timeline` subcommands MUST support a `--type <obs_type>` flag to filter results by observation type (e.g., `decision`, `discovery`, `preference`). When specified, only observations matching the given type are returned.
- **FR-014**: The CLI MUST support a global `--verbose` / `-v` flag that enables DEBUG-level `tracing` output to stderr. This surfaces memory path resolution, search timing, and backend operations for diagnostic purposes. When not specified, only data output and errors are displayed.

### Key Entities

- **CLI Binary**: A single executable (`rusty-brain`) with subcommands for different operations (find, ask, stats, timeline).
- **Search Result**: A memory match containing observation type, timestamp, summary, content excerpt, and relevance score.
- **Memory Statistics**: An aggregate view of the memory file including counts, date ranges, file size, and type distribution.
- **Timeline Entry**: A chronologically ordered observation record with timestamp, type, and summary.

## Success Criteria

### Measurable Outcomes

- **SC-001**: All four subcommands (find, ask, stats, timeline) return correct results when invoked against a memory file with known contents.
- **SC-002**: The `find` command returns results within 1 second for memory files with up to 10,000 observations.
- **SC-003**: The `--json` output for every subcommand is valid JSON that can be parsed by standard tools (e.g., `jq`).
- **SC-004**: The CLI displays a helpful error message (not a stack trace or cryptic error) for every invalid input or missing resource scenario.
- **SC-005**: All subcommands correctly resolve the memory file path without the user needing to specify `--memory-path` when run from within a project directory.
- **SC-006**: The CLI binary starts and returns results in under 500ms for typical operations, providing a responsive interactive experience.

## Assumptions

- The `crates/core` memory engine (`Mind`) is complete and provides the search, ask, stats, and timeline/get_context APIs needed by the CLI.
- The `crates/platforms` path resolution logic is available for automatic memory file detection.
- The binary name will be `rusty-brain` (matching the project name), with subcommands as the interface pattern.
- Color and table formatting will be used for human-readable output; the `--json` flag switches to structured output for automation.
- The CLI is a read-only interface to the memory system — it does not write or modify observations. Writing is handled by the hooks system.
- No authentication or access control is needed — the CLI operates on local files owned by the current user.

## Clarifications

### Session 2026-03-02

- Q: When the memory file is locked, should the CLI wait with backoff or immediately error? → A: Wait with exponential backoff (reuse core's `with_lock` pattern), fail with clear message after retries exhausted.
- Q: What is the default `--limit` when not specified for `find` and `timeline`? → A: Default to 10 results for both commands (matches core search default).
- Q: Does `--limit` apply to the `ask` subcommand? → A: No, `ask` is exempt since it returns a single synthesized answer.
- Q: Should `find` and `timeline` support filtering by observation type? → A: Yes, add `--type <obs_type>` flag to both commands.
- Q: Should the CLI support a verbosity/debug flag? → A: Yes, add `--verbose` / `-v` global flag enabling DEBUG-level tracing to stderr.
