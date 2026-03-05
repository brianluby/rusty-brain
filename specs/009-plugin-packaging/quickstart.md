# Quickstart: Plugin Packaging & Distribution

**Feature**: 009-plugin-packaging | **Date**: 2026-03-04

## Prerequisites

- GitHub repository with `crates/cli` and `crates/hooks` producing `rusty-brain` and `rusty-brain-hooks` binaries
- GitHub Actions enabled on the repository
- Write access to create releases

## Development Workflow

### 1. Create Packaging Files

All static manifests and skill definitions live in `packaging/`:

```sh
# From repo root
ls packaging/claude-code/
# .claude-plugin/plugin.json
# marketplace.json
# hooks/hooks.json
# skills/mind/SKILL.md
# skills/memory/SKILL.md
# commands/ask.md, search.md, recent.md, stats.md

ls packaging/opencode/commands/
# mind-ask.md, mind-search.md, mind-recent.md, mind-stats.md
```

### 2. Test Install Script Locally

```sh
# Lint the install script
shellcheck install.sh

# Run unit tests (requires bats-core)
bats tests/install_script_test.bats

# Dry run (downloads but doesn't install)
RUSTY_BRAIN_VERSION=v0.1.0 sh install.sh --dry-run
```

### 3. Create a Release

```sh
# Ensure Cargo.toml version matches your intended tag
grep '^version' crates/cli/Cargo.toml

# Tag and push
git tag v0.1.0
git push origin v0.1.0

# GitHub Actions will:
# 1. Build binaries for 5 platforms
# 2. Package as .tar.gz with .sha256 sidecars
# 3. Publish a GitHub Release
```

### 4. Verify Release

```sh
# Check release assets
gh release view v0.1.0

# Test install on your machine
curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh | sh

# Verify
rusty-brain --version
ls ~/.claude/plugins/rusty-brain/
```

### 5. Verify Plugin Discovery

Open Claude Code in any project. The `mind` and `memory` skills should appear in the available skills list. Test with:
- `/mind:search test query`
- `/mind:stats`

## File Layout Summary

```text
repo-root/
├── .github/workflows/
│   ├── ci.yml                 # Existing CI (unchanged)
│   └── release.yml            # NEW: Release pipeline
├── packaging/
│   ├── claude-code/           # Claude Code plugin files
│   │   ├── .claude-plugin/plugin.json
│   │   ├── marketplace.json
│   │   ├── hooks/hooks.json
│   │   ├── skills/{mind,memory}/SKILL.md
│   │   └── commands/{ask,search,recent,stats}.md
│   └── opencode/
│       └── commands/{mind-ask,mind-search,mind-recent,mind-stats}.md
├── install.sh                 # POSIX sh installer (macOS/Linux)
├── install.ps1                # PowerShell installer (Windows)
└── tests/
    ├── install_script_test.bats    # install.sh tests
    └── install_script_test.ps1     # install.ps1 tests
```

## Key Conventions

- **No Rust code changes**: This feature only adds scripts, workflows, and manifests
- **Plugin manifests are embedded**: The install script writes manifests directly (no separate download)
- **`${CLAUDE_PLUGIN_ROOT}`**: Use this env var in hooks.json for plugin-relative binary paths (skills use direct `rusty-brain` commands via PATH)
- **Version source of truth**: `Cargo.toml` workspace version; git tag must match
- **Binary install path**: `~/.local/bin/` (macOS/Linux), `$env:LOCALAPPDATA\rusty-brain\bin\` (Windows)
- **Plugin install path**: `~/.claude/plugins/rusty-brain/`
