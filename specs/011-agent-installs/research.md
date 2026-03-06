# Research: Agent Installs

**Feature**: 011-agent-installs | **Date**: 2026-03-05

## Research Tasks

### R1: Agent Extension Mechanisms

**Status**: Partially resolved — OpenCode confirmed, others require spike research

#### OpenCode Extension Mechanism

**Decision**: OpenCode uses a file-based plugin system with JSON configuration in the project's `.opencode/` directory. rusty-brain already has a working OpenCode adapter (008-opencode-plugin).

**Rationale**: The existing `crates/opencode/` crate and `crates/cli/src/opencode_cmd.rs` demonstrate the working pattern. The install command generates the `.opencode/plugins/rusty-brain.json` manifest and registers slash commands.

**Alternatives considered**: None — OpenCode's mechanism is well-documented and already implemented.

**Config location**: `.opencode/plugins/rusty-brain.json` (project-scoped) or `~/.config/opencode/plugins/rusty-brain.json` (global)

#### GitHub Copilot CLI Extension Mechanism

**Decision**: NEEDS SPIKE RESEARCH (PRD Spike-1)

**What we know**: Copilot CLI supports agent extensions via configuration files. The exact format, directory location, and registration mechanism need to be researched against the current Copilot CLI version.

**Research plan**: Install Copilot CLI, inspect extension documentation, identify config file format and location, create example config, document in this file.

#### OpenAI Codex CLI Extension Mechanism

**Decision**: NEEDS SPIKE RESEARCH (PRD Spike-2)

**What we know**: Codex CLI is an open-source agent that supports external tool invocation. The exact plugin/extension mechanism needs research.

**Research plan**: Review Codex CLI source code and documentation, identify how external tools are registered, document config format and location.

#### Google Gemini CLI Extension Mechanism

**Decision**: NEEDS SPIKE RESEARCH (PRD Spike-3)

**What we know**: Gemini CLI is newer and its extension mechanism may be less mature. Need to determine if a plugin system exists.

**Research plan**: Review Gemini CLI documentation and source, determine if extension support exists, document findings or flag as "stub only" if no mechanism exists.

### R2: Atomic File Writes in Rust

**Decision**: Use `tempfile::NamedTempFile` for atomic writes. Write content to temp file in same directory, then `persist()` (which calls `rename` on POSIX, `MoveFileEx` on Windows).

**Rationale**: `tempfile` is already a workspace dependency (dev-dep). The `persist()` method handles cross-platform atomic rename. Same-directory temp files ensure same-filesystem rename (required for atomic POSIX rename).

**Alternatives considered**:
- Manual `std::fs::write` to temp + `std::fs::rename`: Works but requires manual temp file naming and cleanup on error. `tempfile` handles this automatically.
- `fs2::FileExt` advisory locking: Overkill for one-time install; advisory locks aren't needed for config file writes.

### R3: Cross-Platform Binary Detection on PATH

**Decision**: Use `std::env::split_paths(&std::env::var_os("PATH"))` to iterate PATH entries, then `Path::join(entry, binary_name)` and check existence. On Windows, also check with `.exe`, `.cmd`, `.bat` extensions.

**Rationale**: Safe (no shell execution), cross-platform, handles edge cases (empty PATH, missing env var). Avoids `which`/`where` subprocess call which is platform-specific and could be a command injection vector.

**Alternatives considered**:
- `which` crate: External dependency, 500+ lines for something achievable in ~20 lines. Not justified per dependency policy.
- `std::process::Command::new("which")`: Platform-specific (no `which` on Windows without Git Bash), potential command injection if binary name is user-supplied (it's not in our case, but the pattern is unsafe).

### R4: Cross-Platform Config Directory Resolution

**Decision**: Use `std::env::var("HOME")` on Unix and `std::env::var("APPDATA")` or `std::env::var("USERPROFILE")` on Windows for global scope. For project scope, use the current working directory. Avoid adding `dirs` crate dependency.

**Rationale**: The install command needs only two config locations per scope: the agent's config directory and the rusty-brain binary path. These follow simple platform conventions (`~/.config/<agent>/` on Linux, `~/Library/Application Support/<agent>/` on macOS, `%APPDATA%/<agent>/` on Windows). Direct env var access is sufficient and avoids a new dependency.

**Alternatives considered**:
- `dirs` crate: Well-maintained but adds a dependency for something achievable with env vars + conditional compilation. Not justified unless we need complex XDG resolution.
- Hardcoded paths: Fragile, breaks on non-standard setups. Using env vars is more robust.

### R5: Subprocess Timeout for Agent Version Detection

**Decision**: Use `std::process::Command` with `std::thread::spawn` + `std::sync::mpsc::channel` for timeout, or `tokio::process::Command` if async is available. Since the CLI is sync, use the thread-based approach with a 2-second timeout.

**Rationale**: Agent `--version` commands should return instantly, but a hung process could block the install command indefinitely. A 2-second timeout balances reliability with user experience.

**Alternatives considered**:
- No timeout: Risk of indefinite hang if agent binary is malformed or prompts for input.
- `wait_timeout` crate: External dependency for a simple pattern. Not justified.
- `Command::output()` with `kill` on timer: The thread-based approach is simpler and doesn't require unsafe signal handling.

### R6: File Permission Setting on Config Files

**Decision**: On Unix, use `std::os::unix::fs::PermissionsExt` to set mode `0o644` (owner read/write, group/other read-only). On Windows, rely on default ACLs (no additional permission setting needed as Windows uses ACL-based permissions).

**Rationale**: SEC-1 requires config files not be world-writable. `0o644` is the standard permission for non-sensitive config files.

**Alternatives considered**:
- `0o600` (owner-only): Too restrictive for config files that other processes (agents) need to read.
- No explicit permission setting: Inherits umask, which could be overly permissive on some systems.

### R7: `tempfile` Dependency Promotion

**Decision**: Promote `tempfile` from `[dev-dependencies]` to `[dependencies]` in the `platforms` crate's `Cargo.toml`. It remains a workspace-level dependency.

**Rationale**: `ConfigWriter` uses `NamedTempFile` for atomic writes at runtime, not just in tests. This is a minimal change — the crate is already trusted in the workspace.

**Alternatives considered**:
- Implement temp file creation manually: Reinvents what `tempfile` does safely, with edge cases around cleanup on error.
