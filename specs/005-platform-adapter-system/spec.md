# Feature Specification: Platform Adapter System

**Feature Branch**: `005-platform-adapter-system`
**Created**: 2026-03-01
**Status**: Draft
**Input**: User description: "Port the multi-platform abstraction layer with adapter trait, contract validation, event pipeline, Claude/OpenCode adapters, project identity, path policy, platform detection, and diagnostics"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Normalize Raw Hook Input into Typed Platform Events (Priority: P1)

An AI coding platform (Claude Code or OpenCode) invokes a hook process, passing raw JSON input with platform-specific fields. The system normalizes this raw input into a well-typed platform event (session start, tool observation, or session stop) with consistent field names, a unique event ID, and a timestamp — regardless of which platform produced the input. This allows all downstream systems (memory storage, compression, context injection) to work with a single unified event format.

**Why this priority**: Event normalization is the core purpose of the adapter system. Without it, every downstream consumer would need platform-specific parsing logic, making the system brittle and hard to extend.

**Independent Test**: Can be fully tested by passing representative raw hook JSON for each platform and verifying the normalized event output has all required fields with correct values.

**Acceptance Scenarios**:

1. **Given** raw hook input from Claude Code containing a session ID, working directory, and tool name, **When** the Claude adapter normalizes this as a tool observation, **Then** the result is a typed event with a unique event ID, the correct platform name, a timestamp, the tool name in the payload, and a project context derived from the working directory
2. **Given** raw hook input from OpenCode containing session start data, **When** the OpenCode adapter normalizes this, **Then** the result is a typed session start event with the same field structure as a Claude-normalized event
3. **Given** raw hook input missing a tool name, **When** the adapter attempts to normalize a tool observation, **Then** the result indicates the event could not be created (null/none response) rather than producing an incomplete event

---

### User Story 2 - Detect Which Platform Is Running (Priority: P1)

The system automatically detects whether it is running inside Claude Code, OpenCode, or a custom platform — using environment variables and hook input fields. This detection determines which adapter to use for event normalization. Platform detection supports explicit override (via environment variable), implicit detection (via platform-specific indicators), and a sensible default (Claude Code).

**Why this priority**: Platform detection must happen before any adapter can be selected. It is the entry point for the entire adapter system and is called on every hook invocation.

**Independent Test**: Can be fully tested by setting environment variables and passing hook input with various platform fields, then verifying the detected platform name.

**Acceptance Scenarios**:

1. **Given** an explicit platform name in the hook input, **When** the detector runs, **Then** the explicit name is used (case-normalized to lowercase)
2. **Given** no explicit platform but the `MEMVID_PLATFORM` environment variable is set, **When** the detector runs, **Then** the environment variable value is used
3. **Given** no explicit platform and no `MEMVID_PLATFORM` but `OPENCODE=1` is set, **When** the detector runs, **Then** "opencode" is returned
4. **Given** no explicit platform and no platform-specific environment variables, **When** the detector runs, **Then** "claude" is returned as the default

---

### User Story 3 - Validate Adapter Contract Compatibility (Priority: P1)

When a platform event arrives, the system checks that the event's contract version is compatible with the system's supported version using semantic versioning rules. If the major version does not match, the event is flagged as incompatible. Incompatible events are skipped with a diagnostic record rather than causing errors — ensuring the system never blocks a coding session due to a version mismatch (fail-open semantics).

**Why this priority**: Contract validation prevents silent data corruption from processing events in an incompatible format. Fail-open behavior is critical because blocking an agent's session would be worse than skipping one observation.

**Independent Test**: Can be fully tested by passing events with various contract version strings and verifying compatible/incompatible results and diagnostic generation.

**Acceptance Scenarios**:

1. **Given** a platform event with contract version "1.2.3" and the system supports major version 1, **When** the contract is validated, **Then** the result is compatible
2. **Given** a platform event with contract version "2.0.0" and the system supports major version 1, **When** the contract is validated, **Then** the result is incompatible with reason "incompatible_contract_major"
3. **Given** a platform event with a malformed contract version string (e.g., "not-a-version"), **When** the contract is validated, **Then** the result is incompatible with reason "invalid_contract_version"
4. **Given** an incompatible event, **When** the pipeline processes it, **Then** the event is skipped and a diagnostic record is created, but no error is raised

---

### User Story 4 - Resolve Project Identity for Memory Isolation (Priority: P1)

Each project must have a unique identity key so that its memories are stored separately from other projects. The system resolves this identity from the platform-provided project context — preferring an explicit platform project ID, falling back to the canonical project path, and reporting "unresolved" if neither is available. This prevents memory cross-contamination between projects.

**Why this priority**: Without project identity resolution, all projects would share a single memory file, making search results irrelevant and memory storage chaotic. This is foundational for multi-project support.

**Independent Test**: Can be fully tested by passing project context objects with various combinations of project ID, canonical path, and working directory, then verifying the resolved identity key and source.

**Acceptance Scenarios**:

1. **Given** a project context with an explicit platform project ID, **When** the identity is resolved, **Then** the platform project ID is used as the key, with source "platform_project_id"
2. **Given** a project context with no platform project ID but a canonical path, **When** the identity is resolved, **Then** the resolved absolute path is used as the key, with source "canonical_path"
3. **Given** a project context with no platform project ID but a working directory (cwd), **When** the identity is resolved, **Then** the resolved cwd is used as the canonical path and key
4. **Given** a project context with no project ID, no canonical path, and no cwd, **When** the identity is resolved, **Then** the result has a null key with source "unresolved"

---

### User Story 5 - Process Events Through the Pipeline (Priority: P2)

The event pipeline is the central coordination point that takes a normalized platform event, validates its contract version, resolves the project identity, and returns whether the event should be processed or skipped. If any step fails, the pipeline skips the event with a diagnostic rather than raising an error. This ensures that a single malformed event never disrupts the agent's workflow.

**Why this priority**: The pipeline composes contract validation and identity resolution into a single entry point. While each component can be tested independently (P1 stories above), the pipeline is what downstream hooks actually call.

**Independent Test**: Can be fully tested by passing complete platform events and verifying the pipeline returns the correct process/skip decision along with the resolved project identity key.

**Acceptance Scenarios**:

1. **Given** a valid platform event with a compatible contract version and resolvable project identity, **When** the pipeline processes the event, **Then** the result indicates not skipped and includes the project identity key
2. **Given** a platform event with an incompatible contract version, **When** the pipeline processes the event, **Then** the result indicates skipped with reason "incompatible_contract_major" and includes a diagnostic
3. **Given** a platform event with a compatible contract but unresolvable project identity, **When** the pipeline processes the event, **Then** the result indicates skipped with reason "missing_project_identity" and lists the missing context fields

---

### User Story 6 - Resolve Memory File Path with Policy Rules (Priority: P2)

The system determines where a project's memory file should be stored based on a path policy. By default, it uses a legacy path (e.g., `.agent-brain/mind.mv2`). When the platform opts in to platform-specific paths, the system uses a platform-namespaced path instead (e.g., `.claude/mind-claude.mv2`). The resolved path is always validated to stay within the project directory to prevent path traversal.

**Why this priority**: Correct path resolution ensures memory files are stored in predictable, safe locations. It enables the future migration from legacy to platform-specific paths and prevents security issues from path traversal.

**Independent Test**: Can be fully tested by passing various path policy inputs and verifying the resolved path and mode.

**Acceptance Scenarios**:

1. **Given** a project directory and no platform opt-in, **When** the path policy is resolved, **Then** the memory path uses the legacy relative path within the project directory, with mode "legacy_first"
2. **Given** a project directory with platform opt-in enabled, **When** the path policy is resolved, **Then** the memory path uses a platform-namespaced path, with mode "platform_opt_in"
3. **Given** a relative path that would resolve outside the project directory (e.g., `../../etc/secrets`), **When** the path policy is resolved, **Then** an error is raised indicating the path must stay inside the project directory
4. **Given** a platform name containing special characters, **When** the default platform-specific path is generated, **Then** special characters are sanitized to prevent filesystem issues

---

### User Story 7 - Register and Resolve Platform Adapters (Priority: P2)

The system maintains a registry of available platform adapters. Adapters are registered by platform name and can be looked up (resolved) by name. The registry also provides a list of all registered platform names. This allows new platforms to be added without modifying existing code.

**Why this priority**: The registry is the extensibility mechanism. While only Claude and OpenCode adapters exist today, the registry pattern enables future platforms to be added as plug-ins.

**Independent Test**: Can be fully tested by registering adapters, resolving them by name, and listing available platforms.

**Acceptance Scenarios**:

1. **Given** an adapter registered for platform "claude", **When** resolving platform "claude", **Then** the registered adapter is returned
2. **Given** no adapter registered for platform "unknown", **When** resolving platform "unknown", **Then** null/none is returned
3. **Given** adapters registered for "claude" and "opencode", **When** listing platforms, **Then** both names are returned in sorted order

---

### User Story 8 - Record Diagnostic Information for Debugging (Priority: P3)

When events are skipped or errors occur in the adapter pipeline, the system creates structured diagnostic records that capture the platform name, error type, affected fields, severity level, and a retention expiration. These records intentionally omit sensitive data (redacted by default) to prevent leaking project content. Diagnostics support a 30-day retention window for debugging stale issues.

**Why this priority**: Diagnostics are essential for debugging production issues but are not critical for the core event processing flow. They enhance observability without affecting functionality.

**Independent Test**: Can be fully tested by creating diagnostic records with various inputs and verifying all fields are populated correctly, field names are deduplicated, and expiration is calculated accurately.

**Acceptance Scenarios**:

1. **Given** a diagnostic creation request with platform "claude", error type "missing_project_identity", and severity "warning", **When** the diagnostic is created, **Then** the record includes a unique ID, timestamp, the provided fields, `redacted: true`, and an expiration 30 days from creation
2. **Given** a list of field names with duplicates, **When** the diagnostic is created, **Then** the field names are deduplicated
3. **Given** a list of more than 20 field names, **When** the diagnostic is created, **Then** only the first 20 unique field names are retained

---

### Edge Cases

- When the hook input JSON is completely empty (no fields at all), the adapter returns None/null — normalization requires at minimum a session ID
- When the session ID is empty or whitespace-only, the adapter returns None/null — a session ID is required for normalization (truly absent session_id field fails JSON deserialization before reaching the adapter)
- When two adapters are registered for the same platform name, the second silently overwrites the first (last-registered wins)
- Contract versions with pre-release or build metadata (e.g., "1.0.0-beta.1+build.42") have metadata stripped before checking; only the major version number is compared
- When the project directory does not exist on the filesystem, identity resolution still succeeds — it uses the provided path string as-is without filesystem I/O. Path existence is the caller's concern, not the adapter system's.
- When the MEMVID_PLATFORM environment variable contains only whitespace, it is treated as absent and the detection falls through to the next priority level (platform-specific indicators, then default "claude")
- When the canonical path is a symlink, identity resolution uses the provided string value as-is — no symlink resolution is performed (the adapter system does no filesystem I/O). Symlink resolution, if needed, is the caller's responsibility.
- When multiple events arrive with different contract versions in the same session, each event is validated independently against the supported major version. There is no session-level version negotiation — each event stands alone.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST normalize raw hook input into one of three typed event kinds: session start, tool observation, or session stop
- **FR-002**: System MUST assign a unique event ID and timestamp to every normalized event
- **FR-003**: System MUST extract project context (platform project ID, canonical path, working directory) from raw hook input during normalization
- **FR-004**: System MUST include the platform name and contract version on every normalized event
- **FR-005**: System MUST return a null/none result when normalizing a tool observation from input that lacks a tool name
- **FR-005a**: System MUST return a null/none result when normalizing any event kind from input whose session ID is empty or whitespace-only (note: a truly absent session_id field causes JSON deserialization failure before the adapter is invoked; the adapter only needs to check for empty/whitespace)
- **FR-006**: System MUST detect the running platform using this priority: explicit hook input field > `MEMVID_PLATFORM` environment variable > platform-specific indicators (e.g., `OPENCODE=1`) > default "claude"
- **FR-007**: System MUST normalize detected platform names to lowercase and trim whitespace
- **FR-008**: System MUST validate event contract versions against the supported major version using semantic versioning rules; pre-release and build metadata are stripped before comparison (only major version matters)
- **FR-009**: System MUST skip events with incompatible or malformed contract versions rather than raising errors (fail-open)
- **FR-010**: System MUST resolve project identity using this priority: explicit platform project ID > canonical path > working directory > unresolved
- **FR-011**: System MUST report the identity resolution source (platform_project_id, canonical_path, or unresolved) alongside the key
- **FR-012**: System MUST skip events with unresolvable project identity rather than raising errors (fail-open)
- **FR-013**: System MUST resolve memory file paths relative to the project directory
- **FR-014**: System MUST reject resolved memory paths that escape the project directory (path traversal prevention)
- **FR-015**: System MUST support two memory path modes: legacy (default) and platform-specific (opt-in)
- **FR-016**: System MUST sanitize platform names used in file paths by replacing non-alphanumeric characters (except hyphens and underscores) with hyphens
- **FR-017**: System MUST provide an adapter registry that supports registration (last-registered wins on duplicate platform names), lookup by platform name, and listing of all registered platforms
- **FR-018**: System MUST provide a factory for creating adapters from a platform name
- **FR-019**: System MUST create structured diagnostic records for skipped events, including: unique ID, timestamp, platform, error type, affected field names, severity, redacted flag, retention period, and expiration timestamp
- **FR-020**: System MUST deduplicate and cap field names in diagnostic records (maximum 20 unique field names)
- **FR-021**: System MUST set diagnostic retention to 30 days from creation
- **FR-022**: System MUST mark all diagnostic records as redacted by default (no sensitive project data included)
- **FR-023**: System MUST compose contract validation and identity resolution into a single pipeline entry point that returns a process/skip decision

### Key Entities

- **Platform Event**: A normalized, typed record of something that happened during an agent session — one of three kinds (session start, tool observation, session stop). Contains an event ID, timestamp, platform name, contract version, session ID, project context, and a kind-specific payload
- **Platform Adapter**: A trait-based normalizer that converts raw, platform-specific hook input into typed platform events. Each adapter owns its own `normalize()` implementation, is associated with a platform name, and declares a contract version. Built-in adapters may share logic via a factory convenience, but future adapters implement normalization independently
- **Project Context**: Information about which project a session belongs to — includes an optional platform project ID, optional canonical path, and optional working directory
- **Project Identity**: The resolved unique key for a project, derived from project context. Used to isolate memory storage per project
- **Adapter Registry**: A collection of registered platform adapters, supporting lookup by platform name and discovery of available platforms
- **Memory Path Policy**: Rules for determining where a project's memory file is stored, supporting legacy and platform-specific path modes
- **Diagnostic Record**: A structured, redacted record of an error or warning encountered during event processing, with a retention window for debugging purposes
- **Contract Validation Result**: The outcome of checking an event's contract version against the supported version — compatible or incompatible, with a reason for incompatibility

## Clarifications

### Session 2026-03-01

- Q: When a future platform adapter needs different normalization logic, how should the adapter trait handle this? → A: Per-adapter `normalize()` method; shared factory is convenience only
- Q: What is the maximum acceptable latency for a single event normalization + pipeline processing call? → A: No explicit target; sub-5ms implicit (pure in-memory, no I/O)
- Q: When a second adapter is registered for an already-registered platform name, what should happen? → A: Silently overwrite (last-registered wins)
- Q: How should contract versions with pre-release or build metadata be handled during compatibility checking? → A: Strip metadata, compare major version only
- Q: When the hook input JSON is completely empty or the session ID is missing/empty, what should the adapter return? → A: Return None/null — cannot normalize without a session ID. Note: a truly absent session_id field causes JSON deserialization to fail before the adapter is called (session_id is a required String in HookInput); the adapter only checks for empty/whitespace.
- Q: When multiple events in the same session have different contract versions, how are they handled? → A: Each event is validated independently. There is no session-level version state or negotiation.
- Q: Does identity resolution perform filesystem I/O (e.g., canonicalize, symlink resolution)? → A: No. Identity resolution uses path strings as provided. The caller supplies resolved paths if needed.

## Assumptions

- The system supports exactly two built-in platform adapters (Claude Code and OpenCode) at launch; additional adapters can be registered via the registry
- Both built-in adapters use a shared factory for normalization as a convenience (since their input schemas are identical), but each adapter owns its own `normalize()` implementation via the adapter trait — future adapters with different input schemas implement their own normalization logic independently
- Contract version checking uses only the major version for compatibility; minor, patch, pre-release, and build metadata are stripped/ignored before comparison
- The current supported adapter contract major version is 1
- Fail-open semantics apply throughout: validation failures produce diagnostics but never block the agent's session or raise unrecoverable errors
- Diagnostic persistence (writing to disk) is a future enhancement; initial implementation creates in-memory diagnostic records only
- Environment variable reading is safe and does not require sandboxing
- Identity resolution uses path strings as provided — no filesystem I/O (no `canonicalize()`, no symlink resolution). The caller is responsible for providing resolved paths if needed.
- Platform names are case-insensitive; they are normalized to lowercase for storage and comparison
- Event normalization and pipeline processing are pure in-memory computation with no I/O; no explicit latency target is required, but sub-5ms per event is the implicit expectation

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: All three event kinds (session start, tool observation, session stop) are correctly normalized from raw hook input for both Claude Code and OpenCode platforms
- **SC-002**: Platform detection correctly identifies the running platform in 100% of test cases covering all priority levels (explicit input, env var, platform indicator, default)
- **SC-003**: Contract validation correctly accepts compatible versions and rejects incompatible versions with a 0% false positive/negative rate on well-formed version strings
- **SC-004**: Project identity resolution produces the correct key and source for all combinations of present/absent context fields
- **SC-005**: The event pipeline never raises an unrecoverable error, even when given malformed input — all failures produce skip results with diagnostics
- **SC-006**: Memory path resolution rejects 100% of path traversal attempts (paths escaping the project directory)
- **SC-007**: A new platform adapter can be added by implementing the adapter interface and registering it, without modifying any existing code
- **SC-008**: Diagnostic records contain no sensitive project data (all marked redacted) and have correct 30-day expiration timestamps
