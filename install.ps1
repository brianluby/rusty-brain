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
        Test-Version -Version $version
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

    # Install hooks binary to PATH
    $hooksBin = Join-Path $extractDir "rusty-brain-hooks.exe"
    if (Test-Path $hooksBin) {
        Copy-Item -Path $hooksBin -Destination (Join-Path $installDir "rusty-brain-hooks.exe") -Force
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

    Write-Host ""
    Write-Host "To install the Claude Code plugin:"
    Write-Host "  1. Start Claude Code"
    Write-Host "  2. Run: /plugin marketplace add brianluby/rusty-brain"
    Write-Host "  3. Run: /plugin install rusty-brain@rusty-brain"

} finally {
    # Clean up temp directory (SEC-10)
    if ($tempDir -and (Test-Path $tempDir)) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
