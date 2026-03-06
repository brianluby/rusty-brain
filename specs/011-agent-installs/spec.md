# Feature Specification: Agentic Agent Installs

**Feature Branch**: `011-agent-installs`
**Created**: 2026-03-05
**Status**: Draft
**Input**: User description: "Agentic agent installs for opencode, github copilot-cli, codex and gemini."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - OpenCode Plugin Installation (Priority: P1)

An OpenCode user wants to install rusty-brain as a plugin so their agent has persistent memory across sessions. They run a single install command that detects their platform, places configuration files in the right locations, and registers slash commands (`/ask`, `/search`, `/recent`, `/stats`) so the agent can immediately invoke memory operations.

**Why this priority**: OpenCode already has platform adapter support (008-opencode-plugin) and command definitions. This story completes the end-to-end install experience for an already-supported platform.

**Independent Test**: Can be fully tested by running the install command in an OpenCode project directory and verifying that all slash commands appear and route to the rusty-brain binary.

**Acceptance Scenarios**:

1. **Given** a machine with OpenCode installed but no rusty-brain configured, **When** the user runs the agent install command for OpenCode, **Then** configuration files are placed in the OpenCode plugin directory and all four slash commands are registered.
2. **Given** rusty-brain is already configured for OpenCode, **When** the user runs the install command again, **Then** the configuration is upgraded without losing existing memory files.
3. **Given** the install completes, **When** the user starts an OpenCode session, **Then** the agent discovers rusty-brain commands and can invoke `/ask "what did I work on?"` successfully.

---

### User Story 2 - GitHub Copilot CLI Agent Installation (Priority: P1)

A GitHub Copilot CLI user wants rusty-brain integrated into their Copilot agent workflow. The install command sets up the necessary extension configuration so Copilot CLI can invoke rusty-brain for memory operations, providing persistent context across coding sessions.

**Why this priority**: Copilot CLI has a large user base and supports agent extensions. Enabling memory for Copilot users significantly expands rusty-brain's reach.

**Independent Test**: Can be fully tested by running the install command and verifying that Copilot CLI discovers and can invoke rusty-brain memory operations.

**Acceptance Scenarios**:

1. **Given** a machine with GitHub Copilot CLI installed, **When** the user runs the agent install command for Copilot, **Then** the appropriate Copilot agent/extension configuration files are created.
2. **Given** the install completes, **When** the user invokes memory operations through Copilot CLI, **Then** queries are routed to rusty-brain and results are returned in the expected format.
3. **Given** Copilot CLI's extension mechanism changes between versions, **When** the install command runs, **Then** it detects the installed Copilot CLI version and generates compatible configuration.

---

### User Story 3 - OpenAI Codex CLI Agent Installation (Priority: P1)

A Codex CLI user wants persistent memory across their coding sessions. The install command configures rusty-brain as a Codex agent extension, enabling the agent to store and retrieve observations, search past work, and maintain continuity.

**Why this priority**: Codex CLI is a direct competitor to Claude Code and supports agent extensions. Providing cross-agent memory is a key differentiator for rusty-brain.

**Independent Test**: Can be fully tested by running the install command and verifying Codex CLI can invoke rusty-brain's memory operations.

**Acceptance Scenarios**:

1. **Given** a machine with Codex CLI installed, **When** the user runs the agent install command for Codex, **Then** Codex-specific configuration files (agent definitions, tool registrations) are created.
2. **Given** the install completes, **When** the Codex agent invokes a memory search, **Then** the query is routed to rusty-brain and results are returned in Codex's expected output format.
3. **Given** memories were previously stored via a different agent (e.g., Claude Code), **When** the Codex agent searches memory, **Then** it can access the same shared memory store.

---

### User Story 4 - Google Gemini CLI Agent Installation (Priority: P2)

A Gemini CLI user wants persistent memory for their agent sessions. The install command sets up rusty-brain as an extension for Gemini CLI, enabling memory operations across sessions.

**Why this priority**: Gemini CLI is newer and its extension mechanism may be less mature. This is important for cross-agent coverage but lower priority than the more established platforms.

**Independent Test**: Can be fully tested by running the install command and verifying Gemini CLI can invoke rusty-brain memory operations.

**Acceptance Scenarios**:

1. **Given** a machine with Gemini CLI installed, **When** the user runs the agent install command for Gemini, **Then** Gemini-specific configuration files are created.
2. **Given** the install completes, **When** the Gemini agent invokes a memory operation, **Then** the query is routed to rusty-brain and results are returned.

---

### User Story 5 - Unified Multi-Agent Install (Priority: P1)

A developer using multiple AI coding agents wants to install rusty-brain for all their agents in one step. A single command with a `--agents` flag (or auto-detection) configures rusty-brain for all detected agents on the system.

**Why this priority**: Users working with multiple agents should not need to run separate install commands for each one. A unified experience reduces friction and ensures consistent configuration.

**Independent Test**: Can be tested by running the unified install on a machine with multiple agents installed and verifying each agent has working memory operations.

**Acceptance Scenarios**:

1. **Given** a machine with OpenCode and Codex CLI installed, **When** the user runs the install command without specifying agents, **Then** rusty-brain auto-detects both agents and configures itself for each.
2. **Given** the user specifies `--agents opencode,copilot`, **When** the install runs, **Then** only OpenCode and Copilot configurations are created.
3. **Given** an agent is not found on the system, **When** the install attempts to configure it, **Then** a clear message indicates the agent was not found and the install continues for other detected agents.
4. **Given** the install completes for multiple agents, **When** each agent is started, **Then** all agents share the same memory store (`.rusty-brain/mind.mv2`).

---

### User Story 6 - Agent Self-Installation (Priority: P2)

An AI coding agent is working in a project and discovers rusty-brain is available but not configured. The agent can invoke the install command itself, configuring rusty-brain for its own platform without requiring the user to leave the agent session.

**Why this priority**: True "agentic" installation means the agents themselves can trigger setup. This removes the last manual step from the adoption flow.

**Independent Test**: Can be tested by simulating an agent invoking the install command via its tool execution mechanism and verifying the plugin is registered without manual intervention.

**Acceptance Scenarios**:

1. **Given** an agent session where rusty-brain binary is available but not configured, **When** the agent runs the install command targeting its own platform, **Then** the configuration is created and the agent can immediately use memory operations.
2. **Given** the agent invokes the install command, **When** no interactive prompts are required, **Then** the install completes with only structured output (JSON status messages suitable for agent consumption).
3. **Given** the install fails (e.g., missing permissions), **When** the agent reads the error output, **Then** it receives a machine-parseable error with a suggested remediation.

---

### Edge Cases

- What happens when an agent's extension/plugin mechanism is not yet supported by rusty-brain?
  - The install command reports a clear error listing supported agents and their minimum versions.
- What happens when the agent's config directory does not exist (first-time agent installation)?
  - The install command creates the necessary directory structure.
- What happens when two agents have conflicting configuration file formats?
  - Each agent gets its own configuration files in agent-specific directories; the shared binary and memory store remain unified.
- What happens when the binary is installed but the agent configuration becomes stale after an agent upgrade?
  - A `--reconfigure` flag regenerates agent configuration files without re-downloading the binary.
- What happens when the install is run in a CI/CD environment with no agents installed?
  - The install skips agent configuration and only places the binary, exiting with a warning.
- What happens when an agent is detected but its version cannot be determined?
  - Proceed with install using the latest known config format and emit a warning that version could not be confirmed.
- What happens when the user has a custom agent config directory (non-default location)?
  - Deferred (S-4): A `--config-dir` override was planned but deferred from this PR. Agents use platform-standard config directories.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide an `install` subcommand (e.g., `rusty-brain install`) that configures rusty-brain for one or more AI coding agents.
- **FR-002**: System MUST support configuration for four agent platforms: OpenCode, GitHub Copilot CLI, OpenAI Codex CLI, and Google Gemini CLI.
- **FR-003**: System MUST auto-detect which agents are installed on the system by checking standard binary paths and configuration directories.
- **FR-004**: System MUST accept an `--agents` flag to explicitly specify which agents to configure (comma-separated list). Canonical agent names: `opencode`, `copilot`, `codex`, `gemini`.
- **FR-005**: System MUST generate agent-specific configuration files (plugin manifests, command definitions, tool registrations) appropriate to each agent's extension mechanism.
- **FR-006**: System MUST share a single memory store (`.rusty-brain/mind.mv2`) across all configured agents so memories are accessible regardless of which agent stored them.
- **FR-007**: System MUST produce only structured output (JSON) when invoked programmatically (via `--json` flag or when stdin is not a TTY), enabling agents to invoke the install command themselves.
- **FR-008**: System MUST support upgrading agent configurations without data loss when re-run on an already-configured system.
- **FR-009**: System MUST validate that the target agent is actually installed before attempting to configure it, providing a clear error if the agent is not found.
- **FR-010**: System MUST support a `--reconfigure` flag that regenerates agent configuration files without re-downloading or replacing the binary. Before overwriting existing config files, the system MUST create a `.bak` copy of each file being replaced.
- **FR-011**: System MUST provide clear, machine-parseable error messages for all failure modes (missing agent, permission denied, unsupported version).
- **FR-012**: System MUST NOT require interactive prompts during installation when invoked by an agent (non-TTY mode).
- **FR-013**: System MUST create any necessary directory structures (agent config directories) if they do not already exist.
- **FR-014**: System MUST log installation actions and results for diagnostic purposes via the standard `RUSTY_BRAIN_LOG` environment variable.
- **FR-015**: System MUST require the user to specify installation scope explicitly: `--project` (config placed relative to current working directory) or `--global` (config placed in user-level directories such as `~/.config/`). The command MUST NOT default to either scope silently.

### Key Entities

- **Agent Platform**: A supported AI coding agent (OpenCode, Copilot CLI, Codex CLI, Gemini CLI). Key attributes: name, detection method (binary path, config directory), configuration format, minimum supported version.
- **Agent Configuration**: The set of files needed to register rusty-brain with a specific agent. Key attributes: target directory, file format, template, binary path reference.
- **Install Manifest**: A structured record of what was installed and configured. Key attributes: agent name, configuration path, binary path, install timestamp, version.
- **Shared Memory Store**: The unified `.rusty-brain/mind.mv2` file accessed by all agents. Key attributes: file path, access mode (read/write via Mind API).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Users can configure rusty-brain for any single supported agent in under 30 seconds using one command.
- **SC-002**: Users can configure rusty-brain for all detected agents in under 60 seconds using one command.
- **SC-003**: After installation, 100% of configured agents can invoke at least one memory operation (search, ask, recent, stats) without additional manual setup.
- **SC-004**: Auto-detection correctly identifies installed agents on 95%+ of standard development environments (default installation paths).
- **SC-005**: Memories stored by one agent are retrievable by any other configured agent within the same project.
- **SC-006**: Agent-invoked installation (non-interactive mode) completes successfully with structured output parseable by the invoking agent.
- **SC-007**: Upgrading an existing installation preserves all memory data and agent configurations with zero data loss.

## Clarifications

### Session 2026-03-05

- Q: Is `rusty-brain install` project-scoped or system-wide? → A: User must explicitly specify scope; no default. Use `--project` for project-scoped or `--global` for system-wide installation.
- Q: How to handle agents whose extension mechanism is undocumented or doesn't exist? → A: Research each agent's actual extension mechanism first; only build adapters for agents with confirmed plugin support; stub the rest.
- Q: What happens when an agent is detected but its version cannot be determined? → A: Proceed with install using latest known config format, emit a warning that version could not be confirmed.
- Q: Should the install command back up existing config files before overwriting? → A: Yes, create a `.bak` copy of existing config files before overwriting.
- Q: What are the canonical short names for the `--agents` flag? → A: `opencode`, `copilot`, `codex`, `gemini`.

## Assumptions

- Each target agent's extension/plugin mechanism MUST be researched and confirmed before building its adapter. Only agents with documented, stable plugin support get full adapters; others get stubs until their mechanisms are confirmed.
- All agents with confirmed plugin support are expected to receive tool results as structured text (JSON or plain text) from external processes.
- The rusty-brain binary is already installed on the system (via `install.sh`/`install.ps1` from 009-plugin-packaging) before `rusty-brain install` is run for agent configuration.
- Agent detection relies on checking `$PATH` for known binary names and standard config directory locations.
- The existing platform adapter system (005) can be extended to support new agent platforms without major refactoring.

## Scope Boundaries

### In Scope

- Install subcommand for agent-specific configuration
- Auto-detection of installed agents
- Configuration file generation for OpenCode, Copilot CLI, Codex CLI, and Gemini CLI
- Unified multi-agent install in one command
- Non-interactive (agentic) install mode with JSON output
- Reconfiguration support for existing installations
- Shared memory store across all agents

### Out of Scope

- Binary download and placement (handled by 009-plugin-packaging install scripts)
- Claude Code plugin installation (already handled by 009-plugin-packaging)
- Auto-update or version management of the agents themselves
- Agent-specific memory formats (all agents use the same `.mv2` format)
- GUI or web-based installation wizard
- Agent marketplace/store submissions (future enhancement)
- Custom agent platform support (only the four named agents)
