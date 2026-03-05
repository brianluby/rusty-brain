# Research: Plugin Packaging & Distribution

**Feature**: 009-plugin-packaging | **Date**: 2026-03-04

## R-1: Claude Code Plugin Directory Structure

**Decision**: Use `.claude-plugin/plugin.json` at plugin root for discovery. Skills in `skills/`, hooks in `hooks/hooks.json`, commands in `commands/`. All paths relative to plugin root.

**Rationale**: This matches the official Claude Code plugins reference (code.claude.com/docs/en/plugins-reference). The existing agent-brain Node.js plugin uses this exact structure and is successfully discovered by Claude Code.

**Alternatives Considered**:
- Flat `plugin.json` at root (without `.claude-plugin/` directory): Works but `.claude-plugin/` is the documented convention for metadata separation.
- Custom manifest format with `binary` field: Not supported by Claude Code's standard discovery. Binary invocation goes through `hooks/hooks.json` commands, not a manifest `binary` field.

**Key Finding**: Claude Code discovers plugins through `~/.claude/plugins/cache/{marketplace}/{name}/{version}/` for marketplace installs, but direct installation at `~/.claude/plugins/rusty-brain/` also works when registered via `installed_plugins.json`. The `${CLAUDE_PLUGIN_ROOT}` environment variable is injected at runtime and resolves to the plugin's install path.

## R-2: Cross-Compilation Strategy for 5 Targets

**Decision**: Use `houseabsolute/actions-rust-cross@v1` (pinned SHA) for the build matrix. Linux targets use cross-rs (Docker-based musl compilation). macOS and Windows use native compilation on GitHub-provided runners.

**Rationale**: `actions-rust-cross` auto-selects `cross` vs native `cargo` per target, handles toolchain setup, and integrates with `rust-cache`. This is the pattern used by dozens of Rust CLI projects. Manual cross-rs installation (ripgrep's approach) works but requires more boilerplate.

**Alternatives Considered**:
- `cargo-dist`: Generates CI workflows and install scripts automatically but doesn't support custom post-install steps (plugin manifest copying). We'd need custom scripts anyway, negating the main value.
- `taiki-e/upload-rust-binary-action`: All-in-one action but less flexible for custom packaging (e.g., including manifests in archives).
- Manual cross-rs installation (ripgrep pattern): Works but more YAML boilerplate.

**Runner Assignment**:
| Target | Runner | Tool |
|--------|--------|------|
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | cross-rs (Docker) |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04` | cross-rs (Docker) |
| `x86_64-apple-darwin` | `macos-13` | native cargo |
| `aarch64-apple-darwin` | `macos-14` | native cargo |
| `x86_64-pc-windows-msvc` | `windows-latest` | native cargo |

**Key Finding**: macOS Intel (`x86_64-apple-darwin`) requires `macos-13` because `macos-latest` is now Apple Silicon only. `MACOSX_DEPLOYMENT_TARGET=11.0` must be set for Big Sur minimum compatibility.

## R-3: Install Script Platform Detection

**Decision**: Use `uname -s` for OS detection, `uname -m` for architecture. Normalize `arm64` to `aarch64`. On macOS, use `/usr/bin/uname -m` (the universal binary) to correctly detect Apple Silicon even under Rosetta.

**Rationale**: This is the POSIX-standard approach used by rustup, starship, and nvm. The Rosetta detection caveat is important because `uname -m` can incorrectly report `x86_64` on Apple Silicon when running under Rosetta 2.

**Alternatives Considered**:
- `dpkg --print-architecture` (Debian-specific, not portable)
- `arch` command (not available on all Linux distros)
- `sysctl hw.optional.arm64` (macOS-specific, overly complex)

## R-4: SHA-256 Verification Tool Portability

**Decision**: Try `sha256sum` first (Linux), then `shasum -a 256` (macOS), then `openssl dgst -sha256` (fallback). Exit with error if none available (do NOT skip verification).

**Rationale**: `sha256sum` is standard on Linux (GNU coreutils). macOS ships with `shasum` (Perl-based). `openssl dgst` is the universal fallback but its output format varies. The SEC review (SEC-6) requires verification — it must not be optional.

**Alternatives Considered**:
- Skip verification if no tool found (REJECTED: violates SEC-6 and M-4)
- Require `openssl` only (unnecessary constraint on systems with sha256sum)
- Download a verification tool (circular trust problem)

**Key Finding**: The `.sha256` sidecar file should contain only the hex hash string (no filename), extracted via `awk '{print $1}'`. This avoids format mismatches between `sha256sum` (GNU format: `hash  filename`) and `shasum` (BSD format: `hash  filename`).

## R-5: Release Workflow Architecture

**Decision**: Three-job pattern: (1) `create-release` creates a draft GitHub Release, (2) `build-release` matrix builds + packages + uploads per target, (3) `publish-release` undrafts the release after all builds complete.

**Rationale**: The draft-then-publish pattern prevents users from downloading a partial release (e.g., only Linux binaries available while macOS is still building). This is the exact pattern used by ripgrep, starship, and bat.

**Alternatives Considered**:
- Single job with all targets sequentially (too slow, ~45 min vs ~15 min parallel)
- Direct non-draft release (users might download incomplete set)
- Use `softprops/action-gh-release` instead of `gh` CLI (less control over draft lifecycle)

**Key Finding**: Tag-triggered workflow uses `on: push: tags: ['v[0-9]+.[0-9]+.[0-9]+']`. The version is extracted from the tag name (`github.ref_name`). A validation step ensures the tag matches `Cargo.toml` version.

## R-6: Plugin Manifest Embedding Strategy

**Decision**: Embed plugin manifests (plugin.json, marketplace.json, hooks.json, SKILL.md files) as heredocs in the install script. The install script creates the directory structure and writes files directly — no separate download needed.

**Rationale**: Avoids extra network requests during install. The manifest files are small (~2 KB total). Embedding them in the install script means a single `curl | sh` command handles everything. This is specified in the AR (constraints section).

**Alternatives Considered**:
- Include manifests in the binary archive (requires extracting and copying; archive grows)
- Download manifests separately from GitHub raw URL (extra network request, extra failure point)
- Ship manifests as part of the Rust binary and extract via `rusty-brain init-plugin` command (requires Rust code changes — violates AR guardrail)

**Key Finding**: The install script version must be injected at write time. Use a `VERSION` variable at the top of embedded JSON/YAML that gets set from the GitHub API response during install.

## R-7: Hook Binary Distribution

**Decision**: Both `rusty-brain` (CLI) and `rusty-brain-hooks` (hook handler) binaries must be included in the release archive. The hooks.json references `rusty-brain-hooks` via `${CLAUDE_PLUGIN_ROOT}` path. The install script copies the hooks binary to the plugin directory.

**Rationale**: The `crates/hooks` binary is separate from `crates/cli` and handles all Claude Code hook events (SessionStart, PostToolUse, Stop). The hooks.json must reference a binary inside the plugin directory (Claude Code's `${CLAUDE_PLUGIN_ROOT}` resolves to the plugin path). The CLI binary goes to `~/.local/bin` for user access; the hooks binary goes to the plugin directory for Claude Code access.

**Alternatives Considered**:
- Single binary with hooks as subcommands of the CLI binary: The existing architecture uses two separate binaries (`rusty-brain` and `rusty-brain-hooks`). Merging them would require Rust code changes (violates AR guardrail).
- Symlink hooks binary to `~/.local/bin` version: Claude Code's `${CLAUDE_PLUGIN_ROOT}` won't resolve symlinks outside the plugin directory.

**Key Finding**: The release archive must contain both binaries. The packaging step in CI must copy both `target/{target}/release/rusty-brain` and `target/{target}/release/rusty-brain-hooks` into the archive.

## R-8: musl Static Linking with memvid-core

**Decision**: Test musl cross-compilation with memvid-core in a spike (PRD Spike-1). memvid-core is pure Rust with the `lex` feature (text tokenization). If musl linking fails, fallback to glibc with a documented caveat.

**Rationale**: memvid-core uses no C FFI or system libraries (based on the pinned git revision). Pure Rust crates generally compile cleanly with musl. However, the `lex` feature may pull in dependencies that need investigation.

**Key Finding**: The `lex` feature in memvid-core adds text tokenization but is implemented in pure Rust (no C bindings). musl static linking should succeed without special configuration. A `Cross.toml` file at workspace root can be used to pass environment variables if needed.

## R-9: Archive Format and Naming

**Decision**: `.tar.gz` for Linux and macOS, `.tar.gz` for Windows too (not `.zip`). Asset naming: `rusty-brain-v{version}-{target-triple}.tar.gz`. SHA-256 sidecar: `rusty-brain-v{version}-{target-triple}.tar.gz.sha256`.

**Rationale**: The spec and PRD both specify `.tar.gz` for all platforms. While `.zip` is more native on Windows, using a single format simplifies the install script logic and CI packaging step. Windows PowerShell can extract `.tar.gz` via `tar` (available since Windows 10 build 17063).

**Alternatives Considered**:
- `.zip` for Windows (more native but adds conditional logic to CI and install scripts)
- Uncompressed binary (no extraction needed but larger download)

## R-10: OpenCode Command Format

**Decision**: Use the `mind` tool-based format (`mind-ask.md`, `mind-search.md`, etc.) for OpenCode commands. These reference the `mind` native tool with `mode` and `query` parameters.

**Rationale**: The existing OpenCode integration (`crates/opencode`) already implements a `mind` tool handler. The command files delegate to this tool rather than invoking shell commands directly. This matches the current `.opencode/command/` format used by the agent-brain Node.js version.

**Alternatives Considered**:
- Bash-invocation format (shell out to `rusty-brain` CLI): Works but bypasses the OpenCode tool protocol; less integrated.
- Claude Code-style commands (with allowed-tools/Bash): Not compatible with OpenCode's command discovery.
