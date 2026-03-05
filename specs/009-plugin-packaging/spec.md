# Feature Specification: Plugin Packaging & Distribution

**Feature Branch**: `009-plugin-packaging`
**Created**: 2026-03-04
**Status**: Draft
**Input**: User description: "Phase 8 — Plugin Packaging & Distribution. Make the Rust version installable and usable in the same way as the Node.js original."

## Clarifications

### Session 2026-03-04

- Q: Where should the Claude Code plugin be installed/registered? → A: Global plugin at `~/.claude/plugins/rusty-brain/`
- Q: What naming convention for release binary assets? → A: `rusty-brain-v{version}-{target-triple}.tar.gz` (e.g., `rusty-brain-v0.1.0-x86_64-unknown-linux-musl.tar.gz`)
- Q: What checksum algorithm for binary integrity verification? → A: SHA-256 with a single `.sha256` file per asset
- Q: Where should the install script be hosted? → A: GitHub repo raw URL from default branch
- Q: Should the install script add `~/.local/bin` to PATH if missing? → A: Detect if PATH is missing and print manual instructions (no automatic shell config modification)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - One-Command Installation (Priority: P1)

A developer discovers rusty-brain and wants to install it on their machine. They run a single install command (shell script on macOS/Linux, PowerShell script on Windows) which detects their platform, downloads the correct binary, and configures it for immediate use with their coding agent (Claude Code or OpenCode).

**Why this priority**: Without a frictionless install path, no other distribution features matter. This is the gateway to adoption.

**Independent Test**: Can be fully tested by running the install script on a fresh machine and verifying the binary is placed correctly, is executable, and responds to `rusty-brain --version`.

**Acceptance Scenarios**:

1. **Given** a macOS ARM machine with no rusty-brain installed, **When** the user runs `curl ... | sh`, **Then** the correct `aarch64-apple-darwin` binary is downloaded, placed in a standard location (e.g., `~/.local/bin`), and `rusty-brain --version` returns a valid version string.
2. **Given** a Linux x86_64 machine, **When** the user runs the install script, **Then** the correct `x86_64-unknown-linux-musl` binary is installed and functional.
3. **Given** a Windows x86_64 machine, **When** the user runs `install.ps1`, **Then** the correct `.exe` binary is downloaded, placed in a discoverable location, and added to PATH.
4. **Given** rusty-brain is already installed, **When** the user runs the install script again, **Then** it upgrades to the latest version without losing existing configuration or memory files.

---

### User Story 2 - Claude Code Plugin Registration (Priority: P1)

A Claude Code user wants rusty-brain to appear as a registered plugin with working skills (mind, memory) and slash commands. After installation, the plugin manifests (`plugin.json`, `marketplace.json`) point to the Rust binary, and SKILL.md files are available so the coding agent can invoke memory operations.

**Why this priority**: Claude Code is the primary target platform. The plugin must register correctly for the agent to discover and use rusty-brain's capabilities.

**Independent Test**: Can be tested by installing the plugin and verifying that Claude Code recognizes the skills and can invoke `mind:search`, `mind:ask`, `mind:recent`, `mind:stats`, and `mind:memory` commands.

**Acceptance Scenarios**:

1. **Given** rusty-brain is installed, **When** the user opens Claude Code in a project directory, **Then** the plugin is discovered via `plugin.json` and skills appear in the available skills list.
2. **Given** the plugin is registered, **When** the agent invokes the `mind:search` skill, **Then** rusty-brain processes the search and returns results in the expected format.
3. **Given** the plugin is registered, **When** the agent invokes the `mind:memory` skill, **Then** memories are captured and stored using the Rust binary (not the Node.js original).

---

### User Story 3 - OpenCode Slash Command Integration (Priority: P2)

An OpenCode user wants slash commands (`/ask`, `/search`, `/recent`, `/stats`) that invoke rusty-brain operations. The `commands/` directory contains command definitions that OpenCode discovers and routes to the Rust binary.

**Why this priority**: OpenCode is a secondary but important platform. Command definitions enable the same memory workflow in a different agent environment.

**Independent Test**: Can be tested by placing command definitions in the `commands/` directory and verifying OpenCode lists them and routes invocations to the rusty-brain binary.

**Acceptance Scenarios**:

1. **Given** rusty-brain is installed with OpenCode command definitions, **When** the user types `/ask "what did I work on yesterday?"` in OpenCode, **Then** the question is routed to rusty-brain and a response is returned.
2. **Given** command definitions are present, **When** OpenCode starts, **Then** all four commands (ask, search, recent, stats) appear in the available commands list.

---

### User Story 4 - Cross-Platform Release Binaries (Priority: P1)

A CI/CD pipeline builds and publishes pre-compiled binaries for all supported platforms (Linux x86_64, Linux aarch64, macOS x86_64, macOS aarch64, Windows x86_64) on each release. Users on any of these platforms can download a binary that works without compiling from source.

**Why this priority**: Pre-built binaries are a prerequisite for the install scripts and plugin distribution. Without them, users must have Rust toolchains installed.

**Independent Test**: Can be tested by downloading each platform's binary on the corresponding OS and verifying it executes correctly.

**Acceptance Scenarios**:

1. **Given** a new release tag is pushed, **When** CI completes, **Then** binaries for all five platform targets are published as release assets.
2. **Given** a published release, **When** a user downloads the binary for their platform, **Then** the binary runs without additional dependencies (statically linked or with minimal system dependencies).
3. **Given** a release with binaries, **When** the install script queries the latest release, **Then** it correctly identifies and downloads the binary matching the user's OS and architecture.

---

### User Story 5 - npm Wrapper Package (Priority: P3)

For ecosystems where npm is the standard package manager, an optional npm wrapper package (`npx rusty-brain`) downloads and invokes the correct native binary. This bridges the gap for users who expect npm-based installation.

**Why this priority**: This is an optional convenience for Node.js-centric workflows. The primary distribution path is direct binary installation.

**Independent Test**: Can be tested by running `npx rusty-brain --version` on a machine without rusty-brain pre-installed and verifying it downloads and runs the correct binary.

**Acceptance Scenarios**:

1. **Given** the npm package is published, **When** a user runs `npx rusty-brain --version`, **Then** the correct native binary is downloaded and the version is displayed.
2. **Given** the npm package is installed globally, **When** the user runs `rusty-brain search "query"`, **Then** the native binary is invoked with the correct arguments.

---

### User Story 6 - Cargo Crate Publication (Priority: P3)

For Rust developers, rusty-brain is optionally available as a crate on crates.io. Users can install it via `cargo install rusty-brain`. This is a secondary distribution channel.

**Why this priority**: This serves a niche audience (Rust developers) and is optional. The primary path is pre-built binaries.

**Independent Test**: Can be tested by running `cargo install rusty-brain` and verifying the binary compiles and runs correctly.

**Acceptance Scenarios**:

1. **Given** the crate is published on crates.io, **When** a user runs `cargo install rusty-brain`, **Then** the binary compiles and installs successfully.

---

### Edge Cases

- What happens when the user's platform/architecture is not in the supported list (e.g., Linux ARM 32-bit, FreeBSD)?
  - The install script reports a clear error message listing supported platforms.
- What happens when the network is unavailable during installation?
  - The install script fails with a clear error and does not leave partial files.
- What happens when the user lacks write permissions to the install directory?
  - The install script suggests alternative paths or using `sudo`, with a clear explanation.
- What happens when a newer version has breaking changes to the memory format?
  - The install script warns the user and points to migration documentation. Existing `.mv2` files are never modified in place.
- What happens when the `plugin.json` points to a binary path that doesn't exist?
  - The plugin system reports a clear error indicating the binary path is invalid and suggests re-running the install script.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide `plugin.json`, `marketplace.json`, and `hooks/hooks.json` manifests installed at `~/.claude/plugins/rusty-brain/` that reference the Rust binary path for Claude Code plugin discovery and hook registration.
- **FR-002**: System MUST include SKILL.md files for `mind` and `memory` skills in a `skills/` directory, matching the format expected by Claude Code.
- **FR-003**: System MUST include OpenCode slash command definitions for `ask`, `search`, `recent`, and `stats` in a `commands/` directory.
- **FR-004**: System MUST produce pre-compiled release binaries (both `rusty-brain` CLI and `rusty-brain-hooks` hook binary) for five targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`.
- **FR-005**: System MUST provide an `install.sh` script for macOS/Linux that detects OS and architecture, downloads the correct binary, and places it in a standard location.
- **FR-006**: System MUST provide an `install.ps1` script for Windows that downloads the correct binary and configures PATH.
- **FR-007**: Install scripts MUST support upgrading an existing installation without data loss (memory files, configuration preserved).
- **FR-008**: Install scripts MUST validate downloaded binary integrity via SHA-256 checksum verification against the corresponding `.sha256` sidecar file.
- **FR-009**: System MAY publish to crates.io as an optional distribution channel.
- **FR-010**: System MAY provide an npm wrapper package that downloads and invokes the correct native binary.
- **FR-011**: All manifests (`plugin.json`, `marketplace.json`, command definitions) MUST reference the Rust binary, not the Node.js original.
- **FR-012**: Release binaries MUST be statically linked or require only standard system libraries (no additional runtime dependencies).
- **FR-013**: Install scripts MUST provide clear, actionable error messages for unsupported platforms, network failures, and permission issues.
- **FR-014**: System MUST include Claude Code slash command definitions for `ask`, `search`, `recent`, and `stats` in the plugin `commands/` directory.

### Key Entities

- **Release Binary**: A platform-specific compiled executable. Key attributes: target triple, version, SHA-256 checksum (`.sha256` sidecar file per asset), download URL. Asset naming: `rusty-brain-v{version}-{target-triple}.tar.gz`.
- **Plugin Manifest**: JSON configuration file that registers the plugin with a coding agent. Key attributes: binary path, skill references, version.
- **Skill Definition**: A SKILL.md file describing a capability the agent can invoke. Key attributes: name, description, trigger pattern, invocation command.
- **Command Definition**: A configuration file that registers a slash command with OpenCode. Key attributes: command name, description, handler binary path, arguments.
- **Install Script**: A platform-specific script that automates binary download and configuration. Key attributes: target platform, download URL template, install directory (macOS/Linux: `~/.local/bin`, Windows: `$env:LOCALAPPDATA\rusty-brain\bin`). Hosted at GitHub repo raw URL from default branch. Does not modify shell config; prints PATH instructions if install directory is not in PATH.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Users can install rusty-brain on any of the five supported platforms in under 60 seconds using a single command.
- **SC-002**: After installation, all plugin skills (mind, memory) are discoverable by Claude Code without additional manual configuration.
- **SC-003**: After installation, all OpenCode slash commands (ask, search, recent, stats) are available without additional manual configuration.
- **SC-004**: Release binaries for all five platforms are automatically produced on every tagged release with zero manual steps.
- **SC-005**: The install script correctly detects and handles 100% of the five supported platform targets.
- **SC-006**: Upgrading an existing installation preserves all user data (memory files, configuration) with zero data loss.
- **SC-007**: Downloaded binaries pass checksum verification 100% of the time (no corrupted downloads accepted).

## Assumptions

- GitHub Releases is the primary hosting mechanism for release binaries.
- The install script downloads from GitHub Releases API endpoints.
- Claude Code plugin is installed globally at `~/.claude/plugins/rusty-brain/` with `plugin.json` at the root of that directory.
- OpenCode command discovery follows the `commands/` directory convention currently used by the Node.js version.
- The npm wrapper package (if implemented) follows the pattern used by projects like `esbuild` and `turbo` — a thin JS shim that downloads the native binary on first run.
- Static linking with musl is used for Linux targets to avoid glibc version dependencies.
- macOS binaries target the minimum supported macOS version (11.0 Big Sur or later).
- Windows binaries target the MSVC toolchain for broad compatibility.

## Scope Boundaries

### In Scope

- Plugin manifests for Claude Code
- Skill definitions (SKILL.md) for Claude Code
- Command definitions for OpenCode
- Cross-platform CI/CD release pipeline
- Install scripts for macOS, Linux, and Windows
- Binary checksum generation and verification
- Upgrade-in-place support

### Out of Scope

- Auto-update mechanism (users re-run install script manually)
- Package manager packages (Homebrew, apt, chocolatey) — future enhancement
- GUI installer for any platform
- Signing/notarization of macOS binaries (future enhancement, may be added later)
- Windows code signing (future enhancement)
- Docker image distribution
