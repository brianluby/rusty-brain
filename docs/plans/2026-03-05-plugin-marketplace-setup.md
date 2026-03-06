# Plugin Marketplace Setup — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Move plugin files from `packaging/claude-code/` to repo root and set up a proper Claude Code marketplace so users install via `/plugin marketplace add brianluby/rusty-brain`.

**Architecture:** The rusty-brain repo itself becomes the marketplace. `.claude-plugin/marketplace.json` lists one plugin (`rusty-brain`) with source `"./"`. The plugin files (commands/, skills/, hooks/) live at the repo root. The install scripts are simplified to only install the binary + print instructions to add the marketplace.

**Tech Stack:** Claude Code plugin system, POSIX sh, PowerShell 5.1+

---

### Task 1: Move plugin files to repo root

**Files:**
- Move: `packaging/claude-code/.claude-plugin/plugin.json` → `.claude-plugin/plugin.json`
- Move: `packaging/claude-code/commands/` → `commands/`
- Move: `packaging/claude-code/skills/` → `skills/`
- Move: `packaging/claude-code/hooks/` → `hooks/`
- Delete: `packaging/claude-code/marketplace.json` (will be recreated at correct path)
- Delete: `packaging/claude-code/` (empty after moves)

**Step 1: Move files with git mv**

```bash
cd /Volumes/external/repos/rusty-brain
git mv packaging/claude-code/.claude-plugin .claude-plugin
git mv packaging/claude-code/commands commands
git mv packaging/claude-code/skills skills
git mv packaging/claude-code/hooks hooks
rm packaging/claude-code/marketplace.json
rmdir packaging/claude-code
```

**Step 2: Verify structure**

Run: `ls -la .claude-plugin/ commands/ skills/ hooks/`
Expected: All files present at repo root.

**Step 3: Commit**

```bash
git add -A
git commit -m "refactor: move plugin files from packaging/claude-code/ to repo root"
```

---

### Task 2: Create marketplace.json at correct location

The marketplace manifest must live at `.claude-plugin/marketplace.json` (not at plugin root).

**Files:**
- Create: `.claude-plugin/marketplace.json`
- Modify: `.claude-plugin/plugin.json` (remove `skills` and `commands` arrays — let directory convention handle it)

**Step 1: Create `.claude-plugin/marketplace.json`**

```json
{
  "name": "rusty-brain",
  "owner": {
    "name": "Brian Luby",
    "email": "brian@luby.info"
  },
  "metadata": {
    "description": "Persistent AI memory system using memvid video-encoded storage"
  },
  "plugins": [
    {
      "name": "rusty-brain",
      "description": "Persistent AI memory system using memvid video-encoded storage",
      "version": "0.1.0",
      "source": "./",
      "author": {
        "name": "Brian Luby"
      },
      "repository": "https://github.com/brianluby/rusty-brain",
      "license": "Apache-2.0",
      "keywords": ["memory", "ai", "memvid", "persistent-memory"]
    }
  ]
}
```

**Step 2: Simplify `.claude-plugin/plugin.json`**

Per the docs, commands in `commands/`, skills in `skills/`, and hooks in `hooks/hooks.json` are discovered by convention. Remove the explicit arrays and keep only metadata:

```json
{
  "name": "rusty-brain",
  "version": "0.1.0",
  "description": "Persistent AI memory system using memvid video-encoded storage",
  "author": {
    "name": "Brian Luby",
    "url": "https://github.com/brianluby"
  },
  "repository": "https://github.com/brianluby/rusty-brain",
  "license": "Apache-2.0",
  "keywords": ["memory", "ai", "memvid", "persistent-memory"]
}
```

**Step 3: Verify marketplace JSON is valid**

Run: `cat .claude-plugin/marketplace.json | python3 -m json.tool`
Expected: Valid JSON, no errors.

**Step 4: Commit**

```bash
git add .claude-plugin/marketplace.json .claude-plugin/plugin.json
git commit -m "feat: add marketplace manifest for plugin distribution"
```

---

### Task 3: Update install.sh — binary only + marketplace instructions

The install script should only install the binary. Plugin installation should be done via the marketplace. Remove the entire `install_plugin_files` function and the plugin directory logic.

**Files:**
- Modify: `install.sh`

**Step 1: Remove plugin installation from install.sh**

Remove:
- The `install_plugin_files()` function (lines 186-435)
- The `plugin_dir` variable in `main()` (line 444)
- The call to `install_plugin_files` (line 553)
- The "Installing plugin" / "Updating plugin" messages

Replace the plugin section with a post-install message:

```bash
# After the binary install success message, add:
printf '\nTo install the Claude Code plugin:\n'
printf '  1. Start Claude Code\n'
printf '  2. Run: /plugin marketplace add brianluby/rusty-brain\n'
printf '  3. Run: /plugin install rusty-brain@rusty-brain\n'
```

**Step 2: Verify install.sh syntax**

Run: `bash -n install.sh`
Expected: No syntax errors.

**Step 3: Commit**

```bash
git add install.sh
git commit -m "refactor: simplify install.sh to binary-only, use marketplace for plugin"
```

---

### Task 4: Update install.ps1 — binary only + marketplace instructions

Same changes as Task 3 but for the PowerShell installer.

**Files:**
- Modify: `install.ps1`

**Step 1: Remove plugin installation from install.ps1**

Remove:
- All the plugin directory creation logic (lines 285-499)
- The `$pluginDir` variable

Replace with a post-install message:

```powershell
Write-Host ""
Write-Host "To install the Claude Code plugin:"
Write-Host "  1. Start Claude Code"
Write-Host "  2. Run: /plugin marketplace add brianluby/rusty-brain"
Write-Host "  3. Run: /plugin install rusty-brain@rusty-brain"
```

**Step 2: Verify install.ps1 syntax**

Run: `pwsh -c "Get-Content install.ps1 | Out-Null"` (or just review manually)

**Step 3: Commit**

```bash
git add install.ps1
git commit -m "refactor: simplify install.ps1 to binary-only, use marketplace for plugin"
```

---

### Task 5: Update .gitignore for plugin binary

The `rusty-brain-hooks` binary referenced by hooks.json needs to be in the plugin directory at install time. Since the marketplace clones the repo, the binary won't be there. The hooks command uses `${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks` — this binary must be present.

**Decision needed:** The hooks binary is built from Rust source. Options:
1. Commit a pre-built binary to the repo (bad — platform-specific)
2. Change hooks to reference the binary from `~/.local/bin/` instead of `${CLAUDE_PLUGIN_ROOT}`
3. Make the install script copy the hooks binary into the cached plugin directory

**Step 1: Update hooks.json to use PATH-based binary**

Change `${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks` → `rusty-brain-hooks` (relies on the binary being in PATH, which install.sh already ensures for `rusty-brain`).

But wait — the release archive bundles `rusty-brain-hooks` as a separate binary. Check if it's the same binary as `rusty-brain` or different.

Actually, looking at install.sh, `rusty-brain-hooks` is extracted from the release archive and copied to the plugin dir. With the marketplace approach, the binary won't be in the plugin cache. So:

**Update `hooks/hooks.json`** to reference the binary by name (on PATH) rather than `${CLAUDE_PLUGIN_ROOT}`:

```json
{
  "description": "rusty-brain hook registrations for Claude Code lifecycle events",
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "rusty-brain-hooks session-start",
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
            "command": "rusty-brain-hooks post-tool-use",
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
            "command": "rusty-brain-hooks stop",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

**Step 2: Update install.sh to also install rusty-brain-hooks binary to PATH**

Add after the `rusty-brain` binary copy:

```bash
if [ -f "$_extract_dir/rusty-brain-hooks" ]; then
  cp "$_extract_dir/rusty-brain-hooks" "${install_dir}/rusty-brain-hooks"
  chmod +x "${install_dir}/rusty-brain-hooks"
fi
```

**Step 3: Update install.ps1 similarly**

```powershell
$hooksBin = Join-Path $extractDir "rusty-brain-hooks.exe"
if (Test-Path $hooksBin) {
    Copy-Item -Path $hooksBin -Destination (Join-Path $installDir "rusty-brain-hooks.exe") -Force
}
```

**Step 4: Commit**

```bash
git add hooks/hooks.json install.sh install.ps1
git commit -m "fix: reference hooks binary from PATH instead of plugin root"
```

---

### Task 6: Clean up old plugin-manifest.json

**Files:**
- Delete: `plugin-manifest.json` (legacy, replaced by `.claude-plugin/plugin.json`)

**Step 1: Remove legacy manifest**

```bash
git rm plugin-manifest.json
git commit -m "chore: remove legacy plugin-manifest.json"
```

---

### Task 7: Clean up previously installed (broken) plugin

**Files:**
- Delete: `~/.claude/plugins/rusty-brain/` (the manually installed copy)

**Step 1: Remove the old manual install**

```bash
rm -rf ~/.claude/plugins/rusty-brain
```

This is a local-only cleanup, not committed.

---

### Task 8: Test the marketplace locally

**Step 1: Validate the plugin structure**

Run: `claude plugin validate /Volumes/external/repos/rusty-brain`
Expected: Valid plugin structure.

**Step 2: Test with --plugin-dir flag**

Run: `claude --plugin-dir /Volumes/external/repos/rusty-brain`
Then try: `/rusty-brain:ask test query`
Expected: Commands appear and are namespaced under `rusty-brain:`.

**Step 3: Test marketplace add from local path**

In Claude Code, run:
```
/plugin marketplace add /Volumes/external/repos/rusty-brain
/plugin install rusty-brain@rusty-brain
```
Expected: Plugin installs and commands become available.

---

### Task 9: Update README with new install instructions

**Files:**
- Modify: `README.md` (install section only)

**Step 1: Update install instructions**

Replace the plugin installation section with:

```markdown
## Installation

### Binary
```bash
curl -sSf https://raw.githubusercontent.com/brianluby/rusty-brain/main/install.sh | sh
```

### Claude Code Plugin
```
/plugin marketplace add brianluby/rusty-brain
/plugin install rusty-brain@rusty-brain
```
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: update install instructions for marketplace-based plugin"
```
