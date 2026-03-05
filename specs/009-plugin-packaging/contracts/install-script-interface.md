# Contract: Install Script Interface

**Feature**: 009-plugin-packaging | **Date**: 2026-03-04

## install.sh (macOS/Linux)

### Invocation

```sh
curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh | sh
```

Or download-then-run (for security-conscious users):
```sh
curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh -o install.sh
less install.sh  # inspect
sh install.sh
```

### Environment Variables (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `RUSTY_BRAIN_VERSION` | latest | Specific version to install (e.g., `v0.1.0`) |
| `RUSTY_BRAIN_INSTALL_DIR` | `~/.local/bin` | Binary install directory |
| `GITHUB_TOKEN` | (none) | GitHub API token for rate-limited environments |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error (unsupported platform, network failure, checksum mismatch, permission denied) |

### Output Contract

**Success**:
```
Detected platform: aarch64-apple-darwin
Downloading rusty-brain v0.1.0...
Verifying SHA-256 checksum... OK
Installing to /Users/alice/.local/bin/rusty-brain
Installing plugin to /Users/alice/.claude/plugins/rusty-brain/
rusty-brain v0.1.0 installed successfully!
```

If PATH missing:
```
NOTE: /Users/alice/.local/bin is not in your PATH.
Add it by running:
  export PATH="$HOME/.local/bin:$PATH"
Add the above to your shell profile (~/.zshrc or ~/.bashrc) to make it permanent.
```

**Upgrade** (existing install detected):
```
Detected platform: aarch64-apple-darwin
Existing installation found: rusty-brain v0.0.9
Downloading rusty-brain v0.1.0...
Verifying SHA-256 checksum... OK
Upgrading /Users/alice/.local/bin/rusty-brain
Updating plugin at /Users/alice/.claude/plugins/rusty-brain/
rusty-brain upgraded: v0.0.9 -> v0.1.0
```

**Error** (unsupported platform):
```
ERROR: Unsupported platform: armv7l-linux

Supported platforms:
  - x86_64-unknown-linux-musl (Linux x86_64)
  - aarch64-unknown-linux-musl (Linux ARM64)
  - x86_64-apple-darwin (macOS Intel)
  - aarch64-apple-darwin (macOS Apple Silicon)
  - x86_64-pc-windows-msvc (Windows x86_64)

For Windows, use install.ps1 instead.
```

**Error** (checksum mismatch):
```
ERROR: SHA-256 checksum verification failed!
  Expected: a1b2c3d4...
  Actual:   e5f6g7h8...
The downloaded file may be corrupted. Please try again.
If the problem persists, file an issue at https://github.com/brianluby/rusty-brain/issues
```

### Filesystem Effects

| Path | Action | Condition |
|------|--------|-----------|
| `~/.local/bin/rusty-brain` | Create/replace | Always |
| `~/.claude/plugins/rusty-brain/` | Create directory tree | Always |
| `~/.claude/plugins/rusty-brain/.claude-plugin/plugin.json` | Create/replace | Always |
| `~/.claude/plugins/rusty-brain/marketplace.json` | Create/replace | Always |
| `~/.claude/plugins/rusty-brain/hooks/hooks.json` | Create/replace | Always |
| `~/.claude/plugins/rusty-brain/rusty-brain-hooks` | Create/replace (binary) | Always |
| `~/.claude/plugins/rusty-brain/skills/mind/SKILL.md` | Create/replace | Always |
| `~/.claude/plugins/rusty-brain/skills/memory/SKILL.md` | Create/replace | Always |
| `~/.agent-brain/` | **NEVER TOUCHED** | N/A |
| `~/.bashrc`, `~/.zshrc` etc. | **NEVER TOUCHED** | N/A |

### Security Properties

- All downloads over HTTPS with TLS 1.2+ enforced
- SHA-256 checksum verified before any binary placement
- No `eval`, no backtick substitution, no indirect execution
- Temp directory cleaned up on both success and failure (via `trap`)
- Non-zero file size verified before checksum comparison
- No root/sudo required

## install.ps1 (Windows)

### Invocation

```powershell
irm https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.ps1 | iex
```

### Environment Variables (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `RUSTY_BRAIN_VERSION` | latest | Specific version to install |
| `RUSTY_BRAIN_INSTALL_DIR` | `$env:LOCALAPPDATA\rusty-brain\bin` | Binary install directory |

### Filesystem Effects

| Path | Action |
|------|--------|
| `$env:LOCALAPPDATA\rusty-brain\bin\rusty-brain.exe` | Create/replace |
| `$env:APPDATA\.claude\plugins\rusty-brain\` | Create directory tree + manifests |
| `$env:APPDATA\.claude\plugins\rusty-brain\rusty-brain-hooks.exe` | Create/replace |

### PATH Handling

Detects if install directory is in PATH. If not, prints instructions for adding it via System Properties or `$env:PATH`.
