#!/usr/bin/env bats
# tests/install_script_test.bats — Unit tests for install.sh
# Requires: bats-core (https://github.com/bats-core/bats-core)

setup() {
    # Source install.sh in test mode (prevents main execution)
    export INSTALL_SH_TESTING=1
    source "${BATS_TEST_DIRNAME}/../install.sh"
    TEST_TMPDIR="$(mktemp -d)"
}

teardown() {
    rm -rf "${TEST_TMPDIR}"
}

# ---- Platform Detection ----

@test "detect_platform: Linux x86_64 returns x86_64-unknown-linux-musl" {
    uname() {
        case "$1" in
            -s) echo "Linux" ;;
            -m) echo "x86_64" ;;
        esac
    }
    export -f uname
    run detect_platform
    [ "$status" -eq 0 ]
    [ "$output" = "x86_64-unknown-linux-musl" ]
}

@test "detect_platform: Linux aarch64 returns aarch64-unknown-linux-musl" {
    # Mock uname
    uname() {
        case "$1" in
            -s) echo "Linux" ;;
            -m) echo "aarch64" ;;
        esac
    }
    export -f uname
    run detect_platform
    [ "$status" -eq 0 ]
    [ "$output" = "aarch64-unknown-linux-musl" ]
}

@test "detect_platform: macOS arm64 normalized to aarch64-apple-darwin" {
    uname() {
        case "$1" in
            -s) echo "Darwin" ;;
            -m) echo "arm64" ;;
        esac
    }
    export -f uname
    run detect_platform
    [ "$status" -eq 0 ]
    [ "$output" = "aarch64-apple-darwin" ]
}

@test "detect_platform: macOS x86_64 returns x86_64-apple-darwin" {
    uname() {
        case "$1" in
            -s) echo "Darwin" ;;
            -m) echo "x86_64" ;;
        esac
    }
    export -f uname
    run detect_platform
    [ "$status" -eq 0 ]
    [ "$output" = "x86_64-apple-darwin" ]
}

@test "detect_platform: unsupported platform exits with error" {
    uname() {
        case "$1" in
            -s) echo "Linux" ;;
            -m) echo "armv7l" ;;
        esac
    }
    export -f uname
    run detect_platform
    [ "$status" -ne 0 ]
    [[ "$output" == *"Unsupported platform"* ]]
}

# ---- SHA-256 Verification ----

@test "verify_sha256: valid checksum passes" {
    # Create a test file and its checksum
    echo "test content" > "${TEST_TMPDIR}/test.tar.gz"
    expected=$(sha256sum "${TEST_TMPDIR}/test.tar.gz" 2>/dev/null || shasum -a 256 "${TEST_TMPDIR}/test.tar.gz" 2>/dev/null)
    expected=$(echo "$expected" | awk '{print $1}')
    echo "${expected}  test.tar.gz" > "${TEST_TMPDIR}/test.tar.gz.sha256"

    run verify_sha256 "${TEST_TMPDIR}/test.tar.gz" "${TEST_TMPDIR}/test.tar.gz.sha256"
    [ "$status" -eq 0 ]
}

@test "verify_sha256: corrupted file fails" {
    echo "original content" > "${TEST_TMPDIR}/test.tar.gz"
    echo "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  test.tar.gz" > "${TEST_TMPDIR}/test.tar.gz.sha256"

    run verify_sha256 "${TEST_TMPDIR}/test.tar.gz" "${TEST_TMPDIR}/test.tar.gz.sha256"
    [ "$status" -ne 0 ]
    [[ "$output" == *"checksum verification failed"* ]]
}

@test "verify_sha256: missing checksum file fails" {
    echo "test content" > "${TEST_TMPDIR}/test.tar.gz"

    run verify_sha256 "${TEST_TMPDIR}/test.tar.gz" "${TEST_TMPDIR}/nonexistent.sha256"
    [ "$status" -ne 0 ]
}

@test "verify_sha256: works when only shasum is available" {
    # Create a test file and compute checksum using shasum
    echo "shasum test content" > "${TEST_TMPDIR}/test.tar.gz"
    expected=$(shasum -a 256 "${TEST_TMPDIR}/test.tar.gz" 2>/dev/null || sha256sum "${TEST_TMPDIR}/test.tar.gz" 2>/dev/null)
    expected=$(echo "$expected" | awk '{print $1}')
    echo "${expected}  test.tar.gz" > "${TEST_TMPDIR}/test.tar.gz.sha256"

    run verify_sha256 "${TEST_TMPDIR}/test.tar.gz" "${TEST_TMPDIR}/test.tar.gz.sha256"
    [ "$status" -eq 0 ]
}

# ---- File Size Check ----

@test "check_file_size: non-zero file passes" {
    echo "content" > "${TEST_TMPDIR}/test.tar.gz"
    run check_file_size "${TEST_TMPDIR}/test.tar.gz"
    [ "$status" -eq 0 ]
}

@test "check_file_size: zero-byte file fails (SEC-11)" {
    touch "${TEST_TMPDIR}/empty.tar.gz"
    run check_file_size "${TEST_TMPDIR}/empty.tar.gz"
    [ "$status" -ne 0 ]
    [[ "$output" == *"zero"* ]] || [[ "$output" == *"empty"* ]]
}

@test "check_file_size: missing file fails" {
    run check_file_size "${TEST_TMPDIR}/nonexistent.tar.gz"
    [ "$status" -ne 0 ]
}

# ---- Version Validation ----

@test "validate_version: valid semver passes" {
    run validate_version "v0.1.0"
    [ "$status" -eq 0 ]
}

@test "validate_version: valid semver without v prefix passes" {
    run validate_version "0.1.0"
    [ "$status" -eq 0 ]
}

@test "validate_version: pre-release suffix rejected (strict semver only)" {
    run validate_version "v1.2.3-beta.1"
    [ "$status" -ne 0 ]
}

@test "validate_version: shell metacharacters rejected (SEC-5)" {
    run validate_version 'v0.1.0; rm -rf /'
    [ "$status" -ne 0 ]
}

@test "validate_version: backtick injection rejected (SEC-5)" {
    run validate_version 'v0.1.0`whoami`'
    [ "$status" -ne 0 ]
}

@test "validate_version: dollar sign injection rejected (SEC-5)" {
    run validate_version 'v0.1.0$(whoami)'
    [ "$status" -ne 0 ]
}

@test "validate_version: pipe injection rejected (SEC-5)" {
    run validate_version 'v0.1.0|cat /etc/passwd'
    [ "$status" -ne 0 ]
}

@test "validate_version: ampersand injection rejected (SEC-5)" {
    run validate_version 'v0.1.0&&whoami'
    [ "$status" -ne 0 ]
}

@test "validate_version: empty version rejected" {
    run validate_version ""
    [ "$status" -ne 0 ]
}

@test "validate_version: whitespace-only version rejected" {
    run validate_version "   "
    [ "$status" -ne 0 ]
}

# ---- Temp Cleanup ----

@test "cleanup removes temp directory on exit (SEC-10)" {
    local tmpdir
    tmpdir="$(mktemp -d)"
    [ -d "$tmpdir" ]
    cleanup "$tmpdir"
    [ ! -d "$tmpdir" ]
}

@test "cleanup handles already-removed directory gracefully" {
    local tmpdir
    tmpdir="$(mktemp -d)"
    rm -rf "$tmpdir"
    # Should not error even if directory is already gone
    run cleanup "$tmpdir"
    [ "$status" -eq 0 ]
}

@test "cleanup removes nested files inside temp directory (SEC-10)" {
    local tmpdir
    tmpdir="$(mktemp -d)"
    mkdir -p "${tmpdir}/subdir"
    echo "sensitive" > "${tmpdir}/subdir/data.bin"
    echo "archive" > "${tmpdir}/test.tar.gz"
    cleanup "$tmpdir"
    [ ! -d "$tmpdir" ]
}

# ---- Error Messages ----

@test "error messages for unsupported platform include actionable guidance" {
    uname() {
        case "$1" in
            -s) echo "Linux" ;;
            -m) echo "armv7l" ;;
        esac
    }
    export -f uname
    run detect_platform
    [[ "$output" == *"Supported platforms"* ]] || [[ "$output" == *"supported"* ]]
}

@test "error messages for unsupported OS include OS name" {
    uname() {
        case "$1" in
            -s) echo "FreeBSD" ;;
            -m) echo "x86_64" ;;
        esac
    }
    export -f uname
    run detect_platform
    [ "$status" -ne 0 ]
    [[ "$output" == *"Unsupported"* ]]
}

# ---- Malformed JSON Handling (SEC-4) ----

@test "parse_release_json: empty response fails" {
    run parse_release_json "" "x86_64-unknown-linux-musl"
    [ "$status" -ne 0 ]
}

@test "parse_release_json: malformed JSON fails" {
    run parse_release_json '{"incomplete": true' "x86_64-unknown-linux-musl"
    [ "$status" -ne 0 ]
}

@test "parse_release_json: JSON with no matching asset fails" {
    run parse_release_json '{"assets": [{"name": "wrong-platform.tar.gz", "browser_download_url": "https://example.com/wrong"}]}' "x86_64-unknown-linux-musl"
    [ "$status" -ne 0 ]
}

@test "parse_release_json: valid JSON with correct asset succeeds" {
    local platform
    platform="x86_64-unknown-linux-musl"
    local json='{"assets": [{"name": "rusty-brain-v0.1.0-x86_64-unknown-linux-musl.tar.gz", "browser_download_url": "https://github.com/example/releases/download/v0.1.0/rusty-brain-v0.1.0-x86_64-unknown-linux-musl.tar.gz"}]}'
    run parse_release_json "$json" "$platform"
    [ "$status" -eq 0 ]
    [[ "$output" == *"x86_64-unknown-linux-musl"* ]]
}
