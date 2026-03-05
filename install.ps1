#Requires -Version 5.1
<#
.SYNOPSIS
    Install rusty-brain on Windows.
.DESCRIPTION
    Downloads and installs the rusty-brain binary and Claude Code plugin manifests.
    Supports RUSTY_BRAIN_VERSION, RUSTY_BRAIN_INSTALL_DIR, and GITHUB_TOKEN env vars.
.LINK
    https://github.com/brianluby/rusty-brain
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$REPO = "brianluby/rusty-brain"
$DEFAULT_INSTALL_DIR = Join-Path $env:LOCALAPPDATA "rusty-brain\bin"
$TARGET = "x86_64-pc-windows-msvc"

# ---------------------------------------------------------------------------
# Functions
# ---------------------------------------------------------------------------

function Get-Architecture {
    [CmdletBinding()]
    param()

    try {
        return [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    } catch {
        return $env:PROCESSOR_ARCHITECTURE
    }
}

function Get-Platform {
    [CmdletBinding()]
    param()

    $arch = Get-Architecture

    switch ($arch) {
        { $_ -in "X64", "AMD64", "x86_64" } { return "x86_64-pc-windows-msvc" }
        default {
            throw "ERROR: Unsupported architecture: $arch`n`nSupported platforms:`n  - x86_64-pc-windows-msvc (Windows x86_64)`n`nFor macOS/Linux, use install.sh instead."
        }
    }
}

function Test-Version {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Version
    )

    # Reject shell metacharacters
    if ($Version -match '[;`$\(\)\|&<>{}!]') {
        throw "ERROR: Version contains invalid characters: $Version"
    }

    if ($Version -notmatch '^\d+\.\d+\.\d+$' -and $Version -notmatch '^v\d+\.\d+\.\d+$') {
        throw "ERROR: Invalid version format: $Version (expected vX.Y.Z or X.Y.Z)"
    }
}

function Test-FileSize {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$FilePath
    )

    if (-not (Test-Path $FilePath)) {
        throw "ERROR: File not found: $FilePath"
    }

    $size = (Get-Item $FilePath).Length
    if ($size -eq 0) {
        throw "ERROR: Downloaded file is empty: $FilePath"
    }
}

function Test-Sha256 {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string]$ChecksumPath
    )

    $checksumContent = Get-Content $ChecksumPath -Raw
    $expected = ($checksumContent.Trim() -split '\s+')[0]

    $actual = (Get-FileHash -Path $FilePath -Algorithm SHA256).Hash

    if ($expected -ieq $actual) {
        Write-Host "Verifying SHA-256 checksum... OK"
    } else {
        throw @"
ERROR: SHA-256 checksum verification failed!
  Expected: $expected
  Actual:   $actual
The downloaded file may be corrupted. Please try again.
If the problem persists, file an issue at https://github.com/$REPO/issues
"@
    }
}

function Test-InstallDirInPath {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$InstallDir
    )

    $normalised = $InstallDir.TrimEnd('\')
    foreach ($entry in $env:PATH -split ';') {
        if ($entry.TrimEnd('\') -ieq $normalised) {
            return $true
        }
    }
    return $false
}

function Invoke-Download {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Url,
        [Parameter(Mandatory)]
        [string]$OutFile
    )

    $headers = @{}
    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }

    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -Headers $headers -UseBasicParsing
    } finally {
        $ProgressPreference = 'Continue'
    }
}

function Get-ReleaseAssetUrl {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$JsonResponse,
        [Parameter(Mandatory)]
        [string]$Platform
    )

    if ([string]::IsNullOrWhiteSpace($JsonResponse)) {
        throw "ERROR: Empty JSON response from GitHub API"
    }

    try {
        $release = $JsonResponse | ConvertFrom-Json
    } catch {
        throw "ERROR: Malformed JSON response from GitHub API"
    }

    $pattern = "rusty-brain-.*-$Platform\."
    $asset = $release.assets | Where-Object { $_.name -match $pattern } | Select-Object -First 1

    if (-not $asset) {
        throw "ERROR: No asset found for platform: $Platform"
    }

    return $asset.browser_download_url
}

function Get-LatestVersion {
    [CmdletBinding()]
    param()

    $uri = "https://api.github.com/repos/$REPO/releases/latest"
    $headers = @{ "Accept" = "application/vnd.github+json" }

    if ($env:GITHUB_TOKEN) {
        $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN"
    }

    $release = Invoke-RestMethod -Uri $uri -Headers $headers -UseBasicParsing
    return $release.tag_name
}

# ---------------------------------------------------------------------------
# Test guard: when dot-sourced by Pester, only export functions
# ---------------------------------------------------------------------------
if ($env:INSTALL_PS1_TESTING -eq "1") {
    return
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
$tempDir = $null

try {
    $platform = Get-Platform
    Write-Host "Detected platform: $platform"

    # Determine install directory
    $installDir = if ($env:RUSTY_BRAIN_INSTALL_DIR) { $env:RUSTY_BRAIN_INSTALL_DIR } else { $DEFAULT_INSTALL_DIR }

    # Determine version
    if ($env:RUSTY_BRAIN_VERSION) {
        Test-Version -Version $env:RUSTY_BRAIN_VERSION
        $version = $env:RUSTY_BRAIN_VERSION
    } else {
        $version = Get-LatestVersion
    }

    # Normalise version tag (ensure v prefix for download URL)
    $versionTag = if ($version.StartsWith("v")) { $version } else { "v$version" }
    $versionBare = $versionTag.TrimStart("v")

    # Check for existing installation
    $binaryPath = Join-Path $installDir "rusty-brain.exe"
    $existingVersion = $null
    if (Test-Path $binaryPath) {
        try {
            $existingOutput = & $binaryPath --version 2>&1
            if ($existingOutput -match '(\d+\.\d+\.\d+)') {
                $existingVersion = $Matches[1]
                Write-Host "Existing installation found: rusty-brain v$existingVersion"
            }
        } catch {
            # Ignore errors reading existing version
        }
    }

    # Build download URLs
    $archiveName = "rusty-brain-$versionTag-$TARGET.tar.gz"
    $checksumName = "$archiveName.sha256"
    $baseUrl = "https://github.com/$REPO/releases/download/$versionTag"
    $archiveUrl = "$baseUrl/$archiveName"
    $checksumUrl = "$baseUrl/$checksumName"

    # Create temp directory
    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) "rusty-brain-install-$([System.IO.Path]::GetRandomFileName())"
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

    $archivePath = Join-Path $tempDir $archiveName
    $checksumPath = Join-Path $tempDir $checksumName

    # Download archive and checksum
    Write-Host "Downloading rusty-brain $versionTag..."

    Invoke-Download -Url $archiveUrl -OutFile $archivePath
    Invoke-Download -Url $checksumUrl -OutFile $checksumPath

    # Verify file size (SEC-11)
    Test-FileSize -FilePath $archivePath

    # Verify SHA-256 checksum (mandatory)
    Test-Sha256 -FilePath $archivePath -ChecksumPath $checksumPath

    # Extract archive
    $extractDir = Join-Path $tempDir "extracted"
    New-Item -ItemType Directory -Path $extractDir -Force | Out-Null
    tar xzf $archivePath -C $extractDir --strip-components=1
    if ($LASTEXITCODE -ne 0) {
        throw "ERROR: Failed to extract archive"
    }

    # Create install directory and copy binary
    if (-not (Test-Path $installDir)) {
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    }

    $action = if ($existingVersion) { "Upgrading" } else { "Installing to" }
    Write-Host "$action $binaryPath"

    Copy-Item -Path (Join-Path $extractDir "rusty-brain.exe") -Destination $binaryPath -Force

    # Create plugin directory structure
    $pluginDir = Join-Path $env:APPDATA ".claude\plugins\rusty-brain"
    $action2 = if ($existingVersion) { "Updating plugin at" } else { "Installing plugin to" }
    Write-Host "$action2 $pluginDir\"

    $dirs = @(
        (Join-Path $pluginDir ".claude-plugin")
        (Join-Path $pluginDir "hooks")
        (Join-Path $pluginDir "skills\mind")
        (Join-Path $pluginDir "skills\memory")
        (Join-Path $pluginDir "commands")
    )

    foreach ($d in $dirs) {
        if (-not (Test-Path $d)) {
            New-Item -ItemType Directory -Path $d -Force | Out-Null
        }
    }

    # Write plugin.json
    @"
{
  "name": "rusty-brain",
  "version": "$versionBare",
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
"@ | Set-Content -Path (Join-Path $pluginDir ".claude-plugin\plugin.json") -Encoding UTF8

    # Write marketplace.json
    @"
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
      "version": "$versionBare",
      "source": "./"
    }
  ]
}
"@ | Set-Content -Path (Join-Path $pluginDir "marketplace.json") -Encoding UTF8

    # Write hooks.json
    @'
{
  "description": "rusty-brain hook registrations for Claude Code lifecycle events",
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks session-start", "timeout": 30}]}],
    "PostToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks post-tool-use", "timeout": 30}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/rusty-brain-hooks stop", "timeout": 30}]}]
  }
}
'@ | Set-Content -Path (Join-Path $pluginDir "hooks\hooks.json") -Encoding UTF8

    # Write skills/mind/SKILL.md
    @'
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
'@ | Set-Content -Path (Join-Path $pluginDir "skills\mind\SKILL.md") -Encoding UTF8

    # Write skills/memory/SKILL.md
    @'
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
'@ | Set-Content -Path (Join-Path $pluginDir "skills\memory\SKILL.md") -Encoding UTF8

    # Write commands/ask.md
    @'
---
description: Ask questions about memories and get context-aware answers
argument-hint: "<question>"
allowed-tools: ["Bash"]
---

Ask a question about stored memories:

```bash
rusty-brain ask "$ARGUMENTS"
```
'@ | Set-Content -Path (Join-Path $pluginDir "commands\ask.md") -Encoding UTF8

    # Write commands/search.md
    @'
---
description: Search memories for specific content or patterns
argument-hint: "<query>"
allowed-tools: ["Bash"]
---

Search memories for matching content:

```bash
rusty-brain find "$ARGUMENTS"
```
'@ | Set-Content -Path (Join-Path $pluginDir "commands\search.md") -Encoding UTF8

    # Write commands/recent.md
    @'
---
description: Show recent memories and activity timeline
allowed-tools: ["Bash"]
---

Show recent memory activity:

```bash
rusty-brain timeline
```
'@ | Set-Content -Path (Join-Path $pluginDir "commands\recent.md") -Encoding UTF8

    # Write commands/stats.md
    @'
---
description: Show memory statistics and storage information
allowed-tools: ["Bash"]
---

Show memory statistics:

```bash
rusty-brain stats
```
'@ | Set-Content -Path (Join-Path $pluginDir "commands\stats.md") -Encoding UTF8

    # Copy hooks binary to plugin directory
    $hooksSrc = Join-Path $extractDir "rusty-brain-hooks.exe"
    if (Test-Path $hooksSrc) {
        Copy-Item -Path $hooksSrc -Destination (Join-Path $pluginDir "rusty-brain-hooks.exe") -Force
    }

    # Check PATH and print result
    if (-not (Test-InstallDirInPath -InstallDir $installDir)) {
        Write-Host ""
        Write-Host "NOTE: $installDir is not in your PATH."
        Write-Host "Add it by running:"
        Write-Host "  `$env:PATH = `"$installDir;`$env:PATH`""
        Write-Host "To make it permanent, add the directory via System Properties > Environment Variables."
    }

    # Print success
    Write-Host ""
    if ($existingVersion) {
        Write-Host "rusty-brain upgraded: v$existingVersion -> $versionTag"
    } else {
        Write-Host "rusty-brain $versionTag installed successfully!"
    }

} finally {
    # Clean up temp directory (SEC-10)
    if ($tempDir -and (Test-Path $tempDir)) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
