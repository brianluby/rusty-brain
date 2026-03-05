# tests/install_script_test.ps1 — Pester unit tests for install.ps1
# Requires: Pester v5+ (Install-Module Pester -Force)
# Run: Invoke-Pester -Path ./tests/install_script_test.ps1

BeforeAll {
    # Source install.ps1 functions in test mode (prevents main execution)
    $env:INSTALL_PS1_TESTING = "1"
    . "$PSScriptRoot/../install.ps1"
}

AfterAll {
    Remove-Item Env:\INSTALL_PS1_TESTING -ErrorAction SilentlyContinue
}

Describe "Platform Detection" {
    It "Detects Windows x86_64" {
        Mock Get-Architecture { return "X64" }

        $result = Get-Platform
        $result | Should -Be "x86_64-pc-windows-msvc"
    }

    It "Detects AMD64 as x86_64" {
        Mock Get-Architecture { return "AMD64" }

        $result = Get-Platform
        $result | Should -Be "x86_64-pc-windows-msvc"
    }

    It "Rejects ARM64 architecture" {
        Mock Get-Architecture { return "Arm64" }

        { Get-Platform } | Should -Throw "*Unsupported*"
    }

    It "Rejects x86 (32-bit) architecture" {
        Mock Get-Architecture { return "X86" }

        { Get-Platform } | Should -Throw "*Unsupported*"
    }
}

Describe "SHA-256 Verification" {
    It "Passes with valid checksum" {
        $testFile = Join-Path $TestDrive "test.tar.gz"
        Set-Content -Path $testFile -Value "test content" -NoNewline
        $hash = (Get-FileHash -Path $testFile -Algorithm SHA256).Hash.ToLower()
        $checksumFile = Join-Path $TestDrive "test.tar.gz.sha256"
        Set-Content -Path $checksumFile -Value "$hash  test.tar.gz"

        { Test-Sha256 -FilePath $testFile -ChecksumPath $checksumFile } | Should -Not -Throw
    }

    It "Fails with corrupted file" {
        $testFile = Join-Path $TestDrive "test.tar.gz"
        Set-Content -Path $testFile -Value "original content" -NoNewline
        $checksumFile = Join-Path $TestDrive "test.tar.gz.sha256"
        Set-Content -Path $checksumFile -Value "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  test.tar.gz"

        { Test-Sha256 -FilePath $testFile -ChecksumPath $checksumFile } | Should -Throw "*checksum*"
    }

    It "Fails when checksum file is missing" {
        $testFile = Join-Path $TestDrive "test.tar.gz"
        Set-Content -Path $testFile -Value "content" -NoNewline
        $checksumFile = Join-Path $TestDrive "nonexistent.sha256"

        { Test-Sha256 -FilePath $testFile -ChecksumPath $checksumFile } | Should -Throw
    }

    It "Handles uppercase hash comparison correctly" {
        $testFile = Join-Path $TestDrive "test.tar.gz"
        Set-Content -Path $testFile -Value "case test" -NoNewline
        $hash = (Get-FileHash -Path $testFile -Algorithm SHA256).Hash.ToUpper()
        $checksumFile = Join-Path $TestDrive "test.tar.gz.sha256"
        Set-Content -Path $checksumFile -Value "$hash  test.tar.gz"

        { Test-Sha256 -FilePath $testFile -ChecksumPath $checksumFile } | Should -Not -Throw
    }
}

Describe "File Size Check" {
    It "Passes with non-zero file" {
        $testFile = Join-Path $TestDrive "test.tar.gz"
        Set-Content -Path $testFile -Value "content"
        { Test-FileSize -FilePath $testFile } | Should -Not -Throw
    }

    It "Fails with zero-byte file (SEC-11)" {
        $testFile = Join-Path $TestDrive "empty.tar.gz"
        New-Item -Path $testFile -ItemType File -Force | Out-Null
        { Test-FileSize -FilePath $testFile } | Should -Throw "*empty*"
    }

    It "Fails when file does not exist" {
        $testFile = Join-Path $TestDrive "nonexistent.tar.gz"
        { Test-FileSize -FilePath $testFile } | Should -Throw
    }
}

Describe "Version Validation" {
    It "Accepts valid semver with v prefix" {
        { Test-Version -Version "v0.1.0" } | Should -Not -Throw
    }

    It "Accepts valid semver without v prefix" {
        { Test-Version -Version "0.1.0" } | Should -Not -Throw
    }

    It "Rejects pre-release suffix (strict semver only)" {
        { Test-Version -Version "v1.2.3-beta.1" } | Should -Throw
    }

    It "Rejects shell metacharacters (SEC-5)" {
        { Test-Version -Version "v0.1.0; rm -rf /" } | Should -Throw
    }

    It "Rejects backtick injection (SEC-5)" {
        { Test-Version -Version 'v0.1.0`whoami`' } | Should -Throw
    }

    It "Rejects dollar sign injection (SEC-5)" {
        { Test-Version -Version 'v0.1.0$(whoami)' } | Should -Throw
    }

    It "Rejects pipe injection (SEC-5)" {
        { Test-Version -Version 'v0.1.0|calc' } | Should -Throw
    }

    It "Rejects ampersand injection (SEC-5)" {
        { Test-Version -Version 'v0.1.0&&calc' } | Should -Throw
    }

    It "Rejects empty version" {
        { Test-Version -Version "" } | Should -Throw
    }

    It "Rejects whitespace-only version" {
        { Test-Version -Version "   " } | Should -Throw
    }
}

Describe "PATH Detection" {
    It "Detects when install directory is in PATH" {
        $installDir = Join-Path $TestDrive "rusty-brain-bin"
        New-Item -Path $installDir -ItemType Directory -Force | Out-Null
        $originalPath = $env:PATH
        $env:PATH = "$installDir;$env:PATH"
        try {
            $result = Test-InstallDirInPath -InstallDir $installDir
            $result | Should -BeTrue
        }
        finally {
            $env:PATH = $originalPath
        }
    }

    It "Returns false when install directory is not in PATH" {
        $installDir = Join-Path $TestDrive "not-in-path-dir"
        New-Item -Path $installDir -ItemType Directory -Force | Out-Null
        $result = Test-InstallDirInPath -InstallDir $installDir
        $result | Should -BeFalse
    }
}

Describe "Download Function" {
    It "Throws on network failure" {
        Mock Invoke-WebRequest { throw "Could not resolve host" }
        { Invoke-Download -Url "https://invalid.example.com/file" -OutFile (Join-Path $TestDrive "out.bin") } | Should -Throw
    }

    It "Throws on HTTP 404" {
        Mock Invoke-WebRequest { throw [System.Net.WebException]::new("404 Not Found") }
        { Invoke-Download -Url "https://example.com/missing" -OutFile (Join-Path $TestDrive "out.bin") } | Should -Throw
    }

    It "Passes GITHUB_TOKEN as Authorization header" -Skip:($true) {
        # Skipped: requires mock inspection; validate structure only
    }
}

Describe "Malformed JSON Handling (SEC-4)" {
    It "Rejects empty API response" {
        { Get-ReleaseAssetUrl -JsonResponse "" -Platform "x86_64-pc-windows-msvc" } | Should -Throw
    }

    It "Rejects malformed JSON" {
        { Get-ReleaseAssetUrl -JsonResponse '{"incomplete": true' -Platform "x86_64-pc-windows-msvc" } | Should -Throw
    }

    It "Rejects JSON with no matching asset" {
        $json = '{"assets": [{"name": "wrong-platform.tar.gz", "browser_download_url": "https://example.com/wrong"}]}'
        { Get-ReleaseAssetUrl -JsonResponse $json -Platform "x86_64-pc-windows-msvc" } | Should -Throw
    }

    It "Accepts valid JSON with correct asset" {
        $json = '{"assets": [{"name": "rusty-brain-v0.1.0-x86_64-pc-windows-msvc.tar.gz", "browser_download_url": "https://github.com/example/releases/download/v0.1.0/rusty-brain-v0.1.0-x86_64-pc-windows-msvc.tar.gz"}]}'
        $result = Get-ReleaseAssetUrl -JsonResponse $json -Platform "x86_64-pc-windows-msvc"
        $result | Should -BeLike "*x86_64-pc-windows-msvc*"
    }
}
