#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

cd "$WORKDIR"
git init -q
git config user.name release-preflight-test
git config user.email release-preflight-test@example.invalid
cp "$ROOT/scripts/release-preflight.sh" .

write_valid_manifest() {
  printf '%s\n' \
    '[workspace]' \
    'members = []' \
    '' \
    '[workspace.package]' \
    'version = "1.2.3"' > Cargo.toml
}

write_valid_changelog() {
  printf '%s\n' \
    '# Changelog' \
    '' \
    '## [Unreleased]' \
    '' \
    '## [1.2.3] - 2026-07-12' > CHANGELOG.md
}

expect_failure() {
  label="$1"
  expected="$2"
  shift 2
  if "$@" >failure.out 2>&1; then
    echo "FAIL: expected failure for $label" >&2
    exit 1
  fi
  if ! grep -qF "$expected" failure.out; then
    echo "FAIL: $label failed for the wrong reason:" >&2
    cat failure.out >&2
    exit 1
  fi
}

write_valid_manifest
write_valid_changelog
git add Cargo.toml CHANGELOG.md release-preflight.sh
git commit -qm baseline
git branch -M main
git update-ref refs/remotes/origin/main HEAD

./release-preflight.sh --tag v1.2.3 --commit HEAD --main-ref origin/main
expect_failure version-mismatch \
  "does not match workspace version" \
  ./release-preflight.sh --tag v1.2.4 --commit HEAD --main-ref origin/main

printf '%s\n' '# Changelog' '## [Unreleased]' > CHANGELOG.md
expect_failure missing-release-notes \
  "has no released ## [1.2.3] section" \
  ./release-preflight.sh --tag v1.2.3 --commit HEAD --main-ref origin/main
git restore CHANGELOG.md

git switch -qc not-main
printf '%s\n' '# side commit' >> CHANGELOG.md
git add CHANGELOG.md
git commit -qm side
expect_failure not-on-main \
  "is not reachable from origin/main" \
  ./release-preflight.sh --tag v1.2.3 --commit HEAD --main-ref origin/main

echo "PASS: release preflight self-test"
