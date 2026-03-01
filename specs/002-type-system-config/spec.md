# Feature Specification: Type System & Configuration

**Feature Branch**: `002-type-system-config`
**Created**: 2026-03-01
**Status**: Draft
**Input**: User description: "Port Phase 1 type system and configuration from RUST_ROADMAP.md"

## User Scenarios & Testing *(mandatory)*

*Note: prd.md is the authoritative source for user stories and acceptance criteria. Stories below are retained for context but prd.md takes precedence.*

### User Story 1 - Downstream Crate Consumes Shared Types (Priority: P1)

A developer working on any downstream crate (core engine, hooks, CLI, platform adapters) imports shared data types from the types crate to model observations, sessions, configuration, and errors. Every type they need is available, well-documented, and enforces valid states at compile time.

**Why this priority**: Every other crate in the workspace depends on these shared types. Without them, no further development can proceed. This is the foundational building block for all of Phase 2+.

**Independent Test**: Import each public type from the types crate, construct valid instances, and verify they compile and behave correctly. Attempt to construct invalid states and confirm they are rejected at compile time or at construction.

**Acceptance Scenarios**:

1. **Given** a developer adds the types crate as a dependency, **When** they import observation, session, configuration, and error types, **Then** all types are available and well-documented with module-level documentation.
2. **Given** a developer constructs an observation with all required fields, **When** they inspect the resulting value, **Then** all 10 observation type variants are available and each observation carries an ID, timestamp, type, summary, content, and optional metadata.
3. **Given** a developer constructs a configuration without specifying optional fields, **When** they use the configuration, **Then** sensible defaults are applied (memory path: `.agent-brain/mind.mv2`, max context observations: 20, max context tokens: 2000, auto-compress: enabled, minimum confidence: 0.6, debug: disabled).

---

### User Story 2 - Data Round-Trips Through Serialization (Priority: P1)

An AI agent or CLI tool serializes observations, session summaries, and configuration to JSON for storage or inter-process communication, then deserializes them back. The round-trip preserves all data exactly, with no information loss or corruption.

**Why this priority**: The memory system stores and retrieves structured data constantly. If serialization is lossy or inconsistent, memories become corrupted. This is a data-integrity prerequisite.

**Independent Test**: Construct each type, serialize to JSON, deserialize back, and verify equality with the original. Include edge cases like empty strings, maximum-length fields, special characters, and missing optional fields.

**Acceptance Scenarios**:

1. **Given** an observation with all fields populated (including metadata with files, platform, and extra key-value data), **When** it is serialized to JSON and deserialized back, **Then** the result is identical to the original.
2. **Given** a session summary with all aggregation fields, **When** round-tripped through JSON, **Then** all fields including lists of decisions and modified files are preserved exactly.
3. **Given** a configuration with only default values, **When** serialized to JSON, **Then** all default values appear in the output. **When** deserialized from an empty or partial JSON object, **Then** defaults are applied for missing fields.
4. **Given** observation metadata with an extensible extra data map containing nested values, **When** round-tripped, **Then** the nested structure is preserved without flattening or loss.

---

### User Story 3 - Error Handling Provides Actionable Diagnostics (Priority: P2)

When an operation fails (file not found, invalid configuration, corrupted data, lock contention), the system produces a structured error with a stable error code, a human-readable message, and enough context for an AI agent to diagnose and recover without manual intervention.

**Why this priority**: AI agents consume errors programmatically. Unstructured or vague errors cause agents to retry blindly or give up. Structured errors enable smart recovery strategies.

**Independent Test**: Trigger each error variant and verify it produces a stable error code, a descriptive message, and retains the original cause chain. Verify errors serialize to a structured format suitable for machine parsing.

**Acceptance Scenarios**:

1. **Given** an operation encounters a file-system error, **When** the error is reported, **Then** it includes a stable category code, a human-readable description, and the underlying OS error details.
2. **Given** an invalid configuration value is provided, **When** the configuration is validated, **Then** the error identifies which field is invalid, what value was provided, and what values are acceptable.
3. **Given** a chain of errors (e.g., file read fails causing memory load to fail), **When** the top-level error is inspected, **Then** the full cause chain is accessible for diagnostic purposes.

---

### User Story 4 - Configuration Resolves from Environment (Priority: P2)

An operator or AI agent configures rusty-brain behavior through environment variables without modifying any files. Environment variables override file-based or default configuration values, following a clear precedence order.

**Why this priority**: AI coding agents run in diverse environments (CI, local dev, containers) and need to configure behavior without touching project files. Environment-based configuration is the standard mechanism for this.

**Independent Test**: Set specific environment variables, construct a configuration, and verify the environment values take precedence over defaults. Unset them and verify defaults are restored.

**Acceptance Scenarios**:

1. **Given** the environment variable for platform is set, **When** configuration is resolved, **Then** the platform value from the environment is used instead of the default.
2. **Given** the environment variable for debug mode is set to a truthy value, **When** configuration is resolved, **Then** debug mode is enabled regardless of the default.
3. **Given** the environment variable for memory path is set, **When** configuration is resolved, **Then** the specified path is used for the memory file.
4. **Given** no environment variables are set, **When** configuration is resolved, **Then** all default values are applied.

---

### User Story 5 - Hook Protocol Types Enable Agent Communication (Priority: P3)

The hook binaries (session-start, post-tool-use, stop) exchange structured JSON messages with the host agent (Claude Code, OpenCode). The types crate provides the input and output message types that match the host agent's hook protocol, ensuring correct communication.

**Why this priority**: Hook communication is critical for the agent integration, but the hooks themselves are built in a later phase. Defining the protocol types early ensures the hooks crate can be developed against a stable contract.

**Independent Test**: Construct hook input and output messages, serialize them, and verify they match the expected JSON structure defined by the host agent protocol. Deserialize sample messages from the host agent and verify they parse correctly.

**Acceptance Scenarios**:

1. **Given** a hook input JSON message from Claude Code, **When** deserialized, **Then** it parses into a typed structure with event type, tool name, and payload.
2. **Given** a hook output with injected context, **When** serialized to JSON, **Then** it matches the structure the host agent expects.
3. **Given** an unknown or future field in a hook input message, **When** deserialized, **Then** the unknown fields are ignored without error (forward-compatible parsing).

---

### Edge Cases

- What happens when a timestamp field contains a value in seconds instead of milliseconds (or vice versa)?
- How does the system handle observation metadata with extremely large extra data maps (e.g., thousands of key-value pairs)?
- What happens when a configuration file contains unknown fields (forward compatibility)?
- How does the system handle empty or whitespace-only strings for required text fields like observation summary?
- What happens when environment variables contain invalid values (e.g., non-numeric string for a numeric config)?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST define an observation type classification with exactly 10 variants: Discovery, Decision, Problem, Solution, Pattern, Warning, Success, Refactor, Bugfix, Feature.
- **FR-002**: System MUST define an observation data structure carrying: unique ID, timestamp, observation type, tool name, summary, content, and optional metadata.
- **FR-003**: System MUST define observation metadata carrying: list of affected files, platform identifier, project identity key, compression flag, session ID, and an extensible map for arbitrary additional data.
- **FR-004**: System MUST define a session summary structure carrying: session ID, start time, end time, observation count, list of key decisions, list of modified files, and narrative summary.
- **FR-005**: System MUST define an injected context structure carrying: recent observations, relevant memories, session summaries, and token count. *(Note: the field name is `token_count`, matching the TypeScript `tokenCount` field.)*
- **FR-006**: System MUST define a configuration structure with these defaults: memory path `.agent-brain/mind.mv2`, max context observations 20, max context tokens 2000, auto-compress enabled, minimum confidence 0.6, debug disabled.
- **FR-007**: System MUST define a statistics structure carrying: total observation count, total session count, oldest memory timestamp, newest memory timestamp, file size, and observation type frequency breakdown.
- **FR-008**: System MUST define hook input and hook output structures matching the JSON protocol used by Claude Code hooks.
- **FR-009**: System MUST define a unified error type hierarchy with stable, machine-parseable error codes for: file-system errors, configuration errors, serialization errors, lock errors, memory corruption, and invalid input.
- **FR-010**: System MUST resolve configuration from environment variables, with environment values taking precedence over defaults. Supported variables: `MEMVID_PLATFORM`, `MEMVID_MIND_DEBUG`, `MEMVID_PLATFORM_MEMORY_PATH`, `MEMVID_PLATFORM_PATH_OPT_IN`, `CLAUDE_PROJECT_DIR`, `OPENCODE_PROJECT_DIR`.
- **FR-011**: All public types MUST serialize to JSON and deserialize from JSON without data loss (round-trip fidelity).
- **FR-012**: All public types MUST enforce valid states — invalid combinations of field values MUST be rejected at construction time, not discovered at runtime.
- **FR-013**: Configuration deserialization MUST apply default values for any missing fields, allowing partial configuration input.
- **FR-014**: Hook input deserialization MUST tolerate unknown fields without error to support forward compatibility with future host agent versions.

### Key Entities

- **Observation**: A single memory entry recorded during an agent's work session. Classified by type, linked to a tool invocation, and carrying structured metadata.
- **ObservationType**: Classification of what kind of event the observation represents (10 variants covering discoveries, decisions, problems, solutions, patterns, warnings, successes, and code changes).
- **ObservationMetadata**: Extensible metadata attached to an observation — files touched, platform, session association, and arbitrary extra data.
- **SessionSummary**: Aggregated summary of an entire agent work session — what happened, what decisions were made, what files changed.
- **InjectedContext**: A bundle of recent memories and session context prepared for injection into an agent's conversation at session start.
- **MindConfig**: Configuration controlling the memory engine's behavior — file locations, context limits, compression, and debug settings.
- **MindStats**: Statistical snapshot of the memory store — counts, timestamps, sizes, and type distributions.
- **HookInput / HookOutput**: The structured messages exchanged between the memory system's hook binaries and the host AI agent.
- **AgentBrainError**: The unified error hierarchy for all failure modes in the system.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every downstream crate in the workspace can import and use all shared types without compilation errors or ambiguity.
- **SC-002**: 100% of public types pass JSON serialization round-trip tests — serialize then deserialize produces an identical value.
- **SC-003**: All 10 observation type variants are representable and distinguishable in serialized output.
- **SC-004**: Configuration constructed without explicit values matches documented defaults for all 6 configurable fields.
- **SC-005**: Configuration respects environment variable overrides for all 6 supported environment variables.
- **SC-006**: Every error variant includes a stable error code that does not change across versions, enabling agents to match on error codes programmatically.
- **SC-007**: Invalid type construction (e.g., empty required fields, out-of-range values) is rejected before runtime use — either at compile time or at the point of construction.
- **SC-008**: Hook input parsing tolerates at least 5 unknown fields without error, demonstrating forward compatibility.
- **SC-009**: All type definitions include documentation sufficient for a developer to understand purpose, constraints, and usage without reading source code of other crates.

## Assumptions

- The 10 observation type variants listed in the roadmap are complete and final for this phase. New variants may be added in future phases.
- The hook JSON protocol follows the Claude Code hooks specification. If the protocol changes upstream, the types will need updating.
- Environment variable names match those used by the existing TypeScript implementation for backwards compatibility.
- "Token count" in InjectedContext uses the same character-based heuristic as the TypeScript version (characters / 4), not a real tokenizer.
- Configuration defaults match the TypeScript implementation's defaults exactly to ensure behavioral compatibility.
- The extensible extra data map in ObservationMetadata uses a string-to-JSON-value mapping, matching the TypeScript `Record<string, unknown>` pattern.

## Dependencies

- Phase 0 workspace (complete) — provides the crate structure and workspace dependencies.
- No external service dependencies — this phase is entirely local data structure definitions and tests.

## Scope Boundaries

**In scope**: All types, errors, configuration, environment resolution, serialization, and unit tests defined in Phase 1 of the roadmap.

**Out of scope**: Runtime behavior (memory engine, file I/O, hooks, CLI), platform adapters, compression logic, and any network functionality. These belong to Phase 2+.
