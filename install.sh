#!/bin/sh
# install.sh — POSIX sh installer for rusty-brain
# Usage: curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh | sh
#
# Environment variables:
#   RUSTY_BRAIN_VERSION     — version to install (default: latest)
#   RUSTY_BRAIN_INSTALL_DIR — binary directory (default: ~/.local/bin)
#   GITHUB_TOKEN            — optional GitHub API token for rate-limited environments
set -eu

GITHUB_REPO="brianluby/rusty-brain"

# ---------- helpers ----------------------------------------------------------

err() {
  printf 'ERROR: %s\n' "$1" >&2
  return 1
}

# ---------- detect_platform --------------------------------------------------

detect_platform() {
  os="$(uname -s)"
  case "$os" in
    Linux)  os_part="unknown-linux-musl" ;;
    Darwin)
      os_part="apple-darwin"
      ;;
    *)
      printf 'ERROR: Unsupported platform: %s-%s\n\n' "$(uname -m)" "$os" >&2
      printf 'Supported platforms:\n' >&2
      printf '  - x86_64-unknown-linux-musl (Linux x86_64)\n' >&2
      printf '  - aarch64-unknown-linux-musl (Linux ARM64)\n' >&2
      printf '  - x86_64-apple-darwin (macOS Intel)\n' >&2
      printf '  - aarch64-apple-darwin (macOS Apple Silicon)\n' >&2
      printf '  - x86_64-pc-windows-msvc (Windows x86_64)\n\n' >&2
      printf 'For Windows, use install.ps1 instead.\n' >&2
      return 1
      ;;
  esac

  # On Darwin, prefer /usr/bin/uname -m for accurate Rosetta detection.
  # In test mode, always use the (potentially mocked) uname for testability.
  if [ "$os" = "Darwin" ] && [ "${INSTALL_SH_TESTING:-0}" != "1" ] && [ -x /usr/bin/uname ]; then
    arch="$(/usr/bin/uname -m)"
  else
    arch="$(uname -m)"
  fi

  # Normalize arm64 → aarch64
  case "$arch" in
    arm64) arch="aarch64" ;;
    x86_64|aarch64) ;;  # already canonical
    *)
      printf 'ERROR: Unsupported platform: %s-%s\n\n' "$arch" "$os" >&2
      printf 'Supported platforms:\n' >&2
      printf '  - x86_64-unknown-linux-musl (Linux x86_64)\n' >&2
      printf '  - aarch64-unknown-linux-musl (Linux ARM64)\n' >&2
      printf '  - x86_64-apple-darwin (macOS Intel)\n' >&2
      printf '  - aarch64-apple-darwin (macOS Apple Silicon)\n' >&2
      printf '  - x86_64-pc-windows-msvc (Windows x86_64)\n\n' >&2
      printf 'For Windows, use install.ps1 instead.\n' >&2
      return 1
      ;;
  esac

  printf '%s-%s\n' "$arch" "$os_part"
}

# ---------- validate_version -------------------------------------------------

validate_version() {
  _ver="${1:-}"
  if [ -z "$_ver" ]; then
    err "Version string is empty"
    return 1
  fi

  # Reject shell metacharacters (SEC-7)
  case "$_ver" in
    *[\ \	\;\'\"'`'\$\(\)\|\&\>\<\{\}\!]*)
      err "Version contains invalid characters: $_ver"
      return 1
      ;;
  esac

  # Must match v[0-9]+.[0-9]+.[0-9]+  (v prefix optional)
  _stripped="$(printf '%s' "$_ver" | sed 's/^v//')"
  if ! printf '%s' "$_stripped" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    err "Invalid version format: $_ver (expected vX.Y.Z)"
    return 1
  fi

  return 0
}

# ---------- check_file_size --------------------------------------------------

check_file_size() {
  _file="${1:-}"
  if [ ! -f "$_file" ]; then
    err "File does not exist: $_file"
    return 1
  fi

  _size="$(wc -c < "$_file" | tr -d ' ')"
  if [ "$_size" -le 0 ]; then
    err "Downloaded file is empty (zero bytes): $_file"
    return 1
  fi

  return 0
}

# ---------- verify_sha256 ----------------------------------------------------

verify_sha256() {
  _archive="${1:-}"
  _checksum_file="${2:-}"

  _expected="$(awk '{print $1}' < "$_checksum_file")"

  # Compute actual hash using fallback chain
  if command -v sha256sum >/dev/null 2>&1; then
    _actual="$(sha256sum "$_archive" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    _actual="$(shasum -a 256 "$_archive" | awk '{print $1}')"
  elif command -v openssl >/dev/null 2>&1; then
    _actual="$(openssl dgst -sha256 "$_archive" | awk '{print $NF}')"
  else
    err "No SHA-256 tool found (need sha256sum, shasum, or openssl)"
    return 1
  fi

  # Case-insensitive comparison
  _expected_lower="$(printf '%s' "$_expected" | tr 'A-F' 'a-f')"
  _actual_lower="$(printf '%s' "$_actual" | tr 'A-F' 'a-f')"

  if [ "$_expected_lower" != "$_actual_lower" ]; then
    printf 'ERROR: SHA-256 checksum verification failed!\n' >&2
    printf '  Expected: %s\n' "$_expected" >&2
    printf '  Actual:   %s\n' "$_actual" >&2
    printf 'The downloaded file may be corrupted. Please try again.\n' >&2
    printf 'If the problem persists, file an issue at https://github.com/%s/issues\n' "$GITHUB_REPO" >&2
    return 1
  fi

  printf 'Verifying SHA-256 checksum... OK\n'
}

# ---------- cleanup ----------------------------------------------------------

cleanup() {
  if [ -n "${1:-}" ] && [ -d "$1" ]; then
    rm -rf "$1"
  fi
}

# ---------- parse_release_json -----------------------------------------------

parse_release_json() {
  _json="${1:-}"
  _triple="${2:-}"

  if [ -z "$_json" ]; then
    err "Empty JSON response from GitHub API"
    return 1
  fi

  _pattern="rusty-brain-.*-${_triple}\\.tar\\.gz\""
  _url="$(printf '%s' "$_json" \
    | grep -o "\"browser_download_url\"[[:space:]]*:[[:space:]]*\"[^\"]*${_pattern}" \
    | head -n 1 \
    | sed 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"//;s/"$//')"

  if [ -z "$_url" ]; then
    err "No asset found for target triple: $_triple"
    return 1
  fi

  printf '%s\n' "$_url"
}

# ---------- install_plugin_files ---------------------------------------------

install_plugin_files() {
  _version="${1:-}"
  _plugin_dir="${2:-}"
  _extract_dir="${3:-}"

  # Strip v prefix for semver in manifests
  _semver="$(printf '%s' "$_version" | sed 's/^v//')"

  printf 'Installing plugin to %s\n' "$_plugin_dir"

  mkdir -p "$_plugin_dir/.claude-plugin"
  mkdir -p "$_plugin_dir/hooks"
  mkdir -p "$_plugin_dir/skills/mind"
  mkdir -p "$_plugin_dir/skills/memory"
  mkdir -p "$_plugin_dir/commands"

  # plugin.json
  cat > "$_plugin_dir/.claude-plugin/plugin.json" <<'PLUGIN_EOF'
{
  "name": "rusty-brain",
  "version": "VERSION_PLACEHOLDER",
  "description": "Persistent AI memory system using memvid video-encoded storage",
  "author": {
    "name": "Brian Luby",
    "url": "https://github.com/brianluby"
  },
  "repository": "https://github.com/brianluby/rusty-brain",
  "license": "Apache-2.0",
  "keywords": ["memory", "ai", "memvid", "persistent-memory"],
  "skills": ["./skills/mind/", "./skills/memory/"],
  "hooks": "./hooks/hooks.json",
  "commands": [
    "./commands/ask.md",
    "./commands/search.md",
    "./commands/recent.md",
    "./commands/stats.md"
  ]
}
PLUGIN_EOF
  sed -i.bak "s/VERSION_PLACEHOLDER/$_semver/" "$_plugin_dir/.claude-plugin/plugin.json"
  rm -f "$_plugin_dir/.claude-plugin/plugin.json.bak"

  # marketplace.json
  cat > "$_plugin_dir/marketplace.json" <<'MARKET_EOF'
{
  "name": "rusty-brain-marketplace",
  "description": "rusty-brain plugin marketplace manifest",
  "owner": {
    "name": "Brian Luby",
    "url": "https://github.com/brianluby"
  },
  "plugins": [
    {
      "name": "rusty-brain",
      "description": "Persistent AI memory system using memvid video-encoded storage",
      "version": "VERSION_PLACEHOLDER",
      "source": "./"
    }
  ]
}
MARKET_EOF
  sed -i.bak "s/VERSION_PLACEHOLDER/$_semver/" "$_plugin_dir/marketplace.json"
  rm -f "$_plugin_dir/marketplace.json.bak"

  # hooks.json
  cat > "$_plugin_dir/hooks/hooks.json" <<'HOOKS_EOF'
{
  "description": "rusty-brain hook registrations for Claude Code lifecycle events",
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks session-start",
            "timeout": 30
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks post-tool-use",
            "timeout": 30
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks stop",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
HOOKS_EOF

  # skills/mind/SKILL.md
  cat > "$_plugin_dir/skills/mind/SKILL.md" <<'MIND_EOF'
---
name: mind
description: Search and manage Claude's persistent memory stored in a single portable .mv2 file
---

# Claude Mind

Search and manage Claude's persistent memory.

## Commands

- `/mind:search <query>` - Search memories for specific content or patterns
- `/mind:ask <question>` - Ask questions about memories and get context-aware answers
- `/mind:recent` - Show recent memories and activity timeline
- `/mind:stats` - Show memory statistics and storage information

## Usage

All memory operations use the `rusty-brain` CLI binary. Memories are stored in `.agent-brain/mind.mv2` and persist across conversations.

### Search memories
```bash
rusty-brain find "<query>"
```

### Ask a question
```bash
rusty-brain ask "<question>"
```

### View recent activity
```bash
rusty-brain timeline
```

### View statistics
```bash
rusty-brain stats
```

_Memories are captured automatically from your tool use via hooks._
MIND_EOF

  # skills/memory/SKILL.md
  cat > "$_plugin_dir/skills/memory/SKILL.md" <<'MEMORY_EOF'
---
name: memory
description: Claude Mind - Search and manage Claude's persistent memory stored in a single portable .mv2 file
---

# Claude Memory

Capture and store memories for persistent context across conversations.

## How It Works

Memory capture happens automatically through Claude Code hooks:
- **SessionStart**: Loads existing memory context
- **PostToolUse**: Captures relevant observations from tool interactions
- **Stop**: Persists captured memories to the `.mv2` file

## Storage

Memories are stored in `.agent-brain/mind.mv2` using memvid video-encoded format. This file is portable and persists across sessions.

## Manual Memory Operations

Use the `mind` skill for manual memory operations:
- `/mind:search <query>` - Search existing memories
- `/mind:ask <question>` - Ask questions about stored context
- `/mind:recent` - View recent activity
- `/mind:stats` - View storage statistics
MEMORY_EOF

  # commands/ask.md
  cat > "$_plugin_dir/commands/ask.md" <<'ASK_EOF'
---
description: Ask questions about memories and get context-aware answers
argument-hint: "<question>"
allowed-tools: ["Bash"]
---

Ask a question about stored memories:

```bash
rusty-brain ask "$ARGUMENTS"
```
ASK_EOF

  # commands/search.md
  cat > "$_plugin_dir/commands/search.md" <<'SEARCH_EOF'
---
description: Search memories for specific content or patterns
argument-hint: "<query>"
allowed-tools: ["Bash"]
---

Search memories for matching content:

```bash
rusty-brain find "$ARGUMENTS"
```
SEARCH_EOF

  # commands/recent.md
  cat > "$_plugin_dir/commands/recent.md" <<'RECENT_EOF'
---
description: Show recent memories and activity timeline
allowed-tools: ["Bash"]
---

Show recent memory activity:

```bash
rusty-brain timeline
```
RECENT_EOF

  # commands/stats.md
  cat > "$_plugin_dir/commands/stats.md" <<'STATS_EOF'
---
description: Show memory statistics and storage information
allowed-tools: ["Bash"]
---

Show memory statistics:

```bash
rusty-brain stats
```
STATS_EOF

  # Copy hooks binary from extracted archive (required for hooks.json)
  if [ -f "$_extract_dir/rusty-brain-hooks" ]; then
    cp "$_extract_dir/rusty-brain-hooks" "$_plugin_dir/rusty-brain-hooks"
    chmod +x "$_plugin_dir/rusty-brain-hooks"
  else
    err "rusty-brain-hooks binary missing from release archive (expected at $_extract_dir/rusty-brain-hooks). Aborting plugin installation."
    return 1
  fi
}

# ---------- main -------------------------------------------------------------

main() {
  target="$(detect_platform)"
  printf 'Detected platform: %s\n' "$target"

  install_dir="${RUSTY_BRAIN_INSTALL_DIR:-$HOME/.local/bin}"
  plugin_dir="$HOME/.claude/plugins/rusty-brain"

  # Validate version if provided
  version="${RUSTY_BRAIN_VERSION:-}"
  if [ -n "$version" ]; then
    validate_version "$version"
    # Ensure v prefix
    case "$version" in
      v*) ;;
      *)  version="v${version}" ;;
    esac
  fi

  # Create temp directory with cleanup trap (SEC-10)
  tmpdir="$(mktemp -d)"
  trap 'cleanup "$tmpdir"' EXIT

  # Determine version from GitHub API if not specified
  if [ -z "$version" ]; then
    _auth_header=""
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      _auth_header="Authorization: token ${GITHUB_TOKEN}"
    fi

    if [ -n "$_auth_header" ]; then
      _release_json="$(curl -sSfL -H "$_auth_header" \
        "https://api.github.com/repos/${GITHUB_REPO}/releases/latest")"
    else
      _release_json="$(curl -sSfL \
        "https://api.github.com/repos/${GITHUB_REPO}/releases/latest")"
    fi

    version="$(printf '%s' "$_release_json" \
      | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
      | head -n 1 \
      | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"//;s/"$//')"

    if [ -z "$version" ]; then
      err "Failed to determine latest version from GitHub API"
      return 1
    fi

    # Validate API-sourced version for defense-in-depth (SEC-5)
    _bare_version="${version#v}"
    validate_version "$_bare_version" || {
      err "GitHub API returned invalid version: $version"
      return 1
    }
  fi

  printf 'Downloading rusty-brain %s...\n' "$version"

  # Build download URLs (HTTPS only, SEC-8)
  _base_url="https://github.com/${GITHUB_REPO}/releases/download/${version}"
  _archive_name="rusty-brain-${version}-${target}.tar.gz"
  _archive_url="${_base_url}/${_archive_name}"
  _checksum_url="${_archive_url}.sha256"

  _archive_path="${tmpdir}/${_archive_name}"
  _checksum_path="${tmpdir}/${_archive_name}.sha256"

  # Download archive and checksum
  _curl_opts="-sSfL"
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl "$_curl_opts" -H "Authorization: token ${GITHUB_TOKEN}" \
      -o "$_archive_path" "$_archive_url"
    curl "$_curl_opts" -H "Authorization: token ${GITHUB_TOKEN}" \
      -o "$_checksum_path" "$_checksum_url"
  else
    curl "$_curl_opts" -o "$_archive_path" "$_archive_url"
    curl "$_curl_opts" -o "$_checksum_path" "$_checksum_url"
  fi

  # Verify file size (SEC-11)
  check_file_size "$_archive_path"

  # Verify SHA-256 checksum (mandatory)
  verify_sha256 "$_archive_path" "$_checksum_path"

  # Extract into fresh empty directory (SEC-12)
  # Note: GNU tar rejects '../' paths by default; --strip-components is for
  # directory prefix convenience, not security. The actual protections are:
  # (1) extraction into a clean temp directory, (2) tar's default path sanitization.
  _extract_dir="${tmpdir}/extract"
  mkdir -p "$_extract_dir"
  tar xzf "$_archive_path" -C "$_extract_dir" --strip-components=1

  # Detect existing installation for upgrade messaging
  _existing_version=""
  if [ -x "${install_dir}/rusty-brain" ]; then
    _existing_version="$("${install_dir}/rusty-brain" --version 2>/dev/null \
      | awk '{print $NF}' || true)"
    printf 'Existing installation found: rusty-brain %s\n' "$_existing_version"
  fi

  # Install binary
  mkdir -p "$install_dir"
  if [ -n "$_existing_version" ]; then
    printf 'Upgrading %s/rusty-brain\n' "$install_dir"
  else
    printf 'Installing to %s/rusty-brain\n' "$install_dir"
  fi
  cp "$_extract_dir/rusty-brain" "${install_dir}/rusty-brain"
  chmod +x "${install_dir}/rusty-brain"

  # Install plugin files — NEVER touch ~/.agent-brain/ (SEC-1)
  if [ -n "$_existing_version" ]; then
    printf 'Updating plugin at %s\n' "$plugin_dir"
  fi
  install_plugin_files "$version" "$plugin_dir" "$_extract_dir"

  # Check if install dir is in PATH (informational only, never modify shell config M-11)
  case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
      printf '\nNOTE: %s is not in your PATH.\n' "$install_dir"
      printf 'Add it by running:\n'
      # shellcheck disable=SC2016
      printf '  export PATH="%s:$PATH"\n' "$install_dir"
      printf 'Add the above to your shell profile (~/.zshrc or ~/.bashrc) to make it permanent.\n'
      ;;
  esac

  # Success message
  if [ -n "$_existing_version" ]; then
    printf 'rusty-brain upgraded: %s -> %s\n' "$_existing_version" "$version"
  else
    printf 'rusty-brain %s installed successfully!\n' "$version"
  fi
}

# ---------- entry point ------------------------------------------------------

# Test guard: when sourced by bats tests, only define functions
if [ "${INSTALL_SH_TESTING:-0}" != "1" ]; then
  main
fi
