#!/usr/bin/env bash
# Run against a native release tarball containing the three real built binaries:
#   bash scripts/verify-fresh-install.test.sh --tarball dist/rusty-brain-VERSION-TARGET.tar.gz
# Checksum-race coverage instruments real tar; packaged executables are never stubbed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ "$#" -ne 2 ] || [ "$1" != "--tarball" ]; then
  echo "usage: bash scripts/verify-fresh-install.test.sh --tarball PATH" >&2
  exit 2
fi
[ -f "$2" ] && [ -r "$2" ] \
  || { echo "tarball is not a readable file: $2" >&2; exit 1; }
ARTIFACT_DIR="$(cd "$(dirname "$2")" && pwd)"
ARTIFACT="$ARTIFACT_DIR/$(basename "$2")"
# Keep nested fresh-install socket paths below the macOS sun_path limit.
WORKDIR="$(mktemp -d /tmp/rb-fi.XXXXXX)"
cleanup() {
  status=$?
  if [ "$status" -eq 0 ]; then rm -rf "$WORKDIR"
  else echo "artifact self-test failed; fixtures kept: $WORKDIR" >&2; fi
}
trap cleanup EXIT
mkdir -p "$WORKDIR/tmp" "$WORKDIR/dist" "$WORKDIR/artifacts with spaces"
export TMPDIR="$WORKDIR/tmp"
cp "$ARTIFACT" "$WORKDIR/dist/release.tar.gz"
cp "$ARTIFACT" "$WORKDIR/artifacts with spaces/release artifact.tar.gz"
cd "$WORKDIR"

write_checksum() {
  local directory="$1" archive="$2"
  (
    cd "$directory"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum "$archive" > SHA256SUMS
    else
      shasum -a 256 "$archive" > SHA256SUMS
    fi
  )
}

expect_success() {
  local label="$1" path="$2"
  if ! bash "$ROOT/scripts/verify-fresh-install.sh" --tarball "$path" > "$WORKDIR/output" 2>&1; then
    echo "FAIL: $label" >&2
    cat "$WORKDIR/output" >&2
    exit 1
  fi
  if ! grep -qF "PASS: fresh-install checklist" "$WORKDIR/output"; then
    echo "FAIL: $label did not finish the real fresh-install checklist" >&2
    cat "$WORKDIR/output" >&2
    exit 1
  fi
  echo "PASS: $label"
}

expect_failure() {
  local label="$1" expected="$2" path="$3"
  if bash "$ROOT/scripts/verify-fresh-install.sh" --tarball "$path" > "$WORKDIR/output" 2>&1; then
    echo "FAIL: expected failure for $label" >&2
    exit 1
  fi
  if ! grep -qF "$expected" "$WORKDIR/output" \
      || grep -qF "PASS: fresh-install checklist" "$WORKDIR/output"; then
    echo "FAIL: $label failed for the wrong reason" >&2
    cat "$WORKDIR/output" >&2
    exit 1
  fi
  echo "PASS: rejected $label"
}

expect_failure missing-directory "tarball is not a readable file" "missing/release.tar.gz"
expect_failure missing-file "tarball is not a readable file" "dist/missing.tar.gz"

# Each success runs the complete real CLI/daemon/hooks checklist, not just tar.
expect_success relative-without-checksum "dist/release.tar.gz"
write_checksum "artifacts with spaces" "release artifact.tar.gz"
expect_success relative-spaces-with-checksum "artifacts with spaces/release artifact.tar.gz"
expect_success absolute-spaces-with-checksum "$WORKDIR/artifacts with spaces/release artifact.tar.gz"
expect_failure absolute-missing-file "tarball is not a readable file" "$WORKDIR/artifacts with spaces/missing.tar.gz"

# Modify the real archive after signing its checksum: rejection must precede unpack/startup.
printf '\nchecksum regression\n' >> "artifacts with spaces/release artifact.tar.gz"
expect_failure relative-bad-checksum "FAILED" "artifacts with spaces/release artifact.tar.gz"
expect_failure absolute-bad-checksum "FAILED" "$WORKDIR/artifacts with spaces/release artifact.tar.gz"

# Swap the mutable source only after checksum verification, at real extraction.
REAL_TAR="$(command -v tar)"
mkdir -p "$WORKDIR/payload" "$WORKDIR/race" "$WORKDIR/replacement" "$WORKDIR/instrumentation"
"$REAL_TAR" -xzf "dist/release.tar.gz" --strip-components=1 -C "$WORKDIR/payload"
printf 'unverified replacement\n' > "$WORKDIR/payload/unverified-marker"
"$REAL_TAR" -czf "$WORKDIR/replacement/replacement.tar.gz" -C "$WORKDIR" payload
cp "dist/release.tar.gz" "$WORKDIR/race/release.tar.gz"
write_checksum "$WORKDIR/race" "release.tar.gz"
cat > "$WORKDIR/instrumentation/tar" <<'TAR'
#!/usr/bin/env bash
cp "$FRESH_INSTALL_REPLACEMENT" "$FRESH_INSTALL_SOURCE"
"$FRESH_INSTALL_REAL_TAR" "$@"
status=$?
if [ "$status" -eq 0 ]; then
  for destination; do :; done
  if [ -f "$destination/unverified-marker" ]; then
    printf 'present\n' > "$FRESH_INSTALL_MARKER"
  else
    printf 'absent\n' > "$FRESH_INSTALL_MARKER"
  fi
fi
exit "$status"
TAR
chmod +x "$WORKDIR/instrumentation/tar"
FRESH_INSTALL_REAL_TAR="$REAL_TAR" \
FRESH_INSTALL_REPLACEMENT="$WORKDIR/replacement/replacement.tar.gz" \
FRESH_INSTALL_SOURCE="$WORKDIR/race/release.tar.gz" \
FRESH_INSTALL_MARKER="$WORKDIR/race-marker" \
PATH="$WORKDIR/instrumentation:$PATH" \
  expect_success checksum-source-replacement "$WORKDIR/race/release.tar.gz"
[ "$(cat "$WORKDIR/race-marker")" = "absent" ] \
  || { echo "FAIL: unpacked bytes other than the verified copy" >&2; exit 1; }

# A genuinely damaged packaged hook fails after auto-start; the daemon must stop.
dd if="$WORKDIR/payload/rusty-brain-hooks" of="$WORKDIR/damaged-hook" bs=4 count=1 2>/dev/null
cp "$WORKDIR/damaged-hook" "$WORKDIR/payload/rusty-brain-hooks"
rm "$WORKDIR/payload/unverified-marker"
"$REAL_TAR" -czf "$WORKDIR/replacement/damaged.tar.gz" -C "$WORKDIR" payload
expect_failure damaged-hook "FAIL: fresh-install checklist" "$WORKDIR/replacement/damaged.tar.gz"
failure="$(grep -F 'workdir kept:' "$WORKDIR/output")"
failed_dir="${failure#*workdir kept: }"
failed_dir="${failed_dir%)}"
pid="$(cat "$failed_dir/rb/sock.pid")"
deadline=$((SECONDS + 5))
while kill -0 "$pid" 2>/dev/null && [ "$SECONDS" -lt "$deadline" ]; do sleep 0.05; done
if kill -0 "$pid" 2>/dev/null; then
  echo "FAIL: damaged artifact left daemon $pid running (fixtures: $failed_dir)" >&2
  exit 1
fi

cp "dist/release.tar.gz" "$WORKDIR/unreadable.tar.gz"
chmod 000 "$WORKDIR/unreadable.tar.gz"
if [ ! -r "$WORKDIR/unreadable.tar.gz" ]; then
  expect_failure unreadable-file "tarball is not a readable file" "$WORKDIR/unreadable.tar.gz"
else
  echo "SKIP: unreadable-file rejection (caller can read mode-000 files)"
fi
chmod 600 "$WORKDIR/unreadable.tar.gz"

if [ "$(uname -s)" = "Darwin" ] && [ -x /opt/homebrew/opt/coreutils/libexec/gnubin/stat ]; then
  PATH="/opt/homebrew/opt/coreutils/libexec/gnubin:$PATH" \
    expect_success darwin-with-gnu-stat-on-path "dist/release.tar.gz"
fi

echo "PASS: fresh-install artifact self-test"
