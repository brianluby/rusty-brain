#!/bin/sh
# tests/check_manifest_drift.sh — Verify install.sh embedded manifests match packaging/ canonical copies.
# Exits non-zero if any manifest has drifted.
set -eu

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMPDIR_DRIFT="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_DRIFT"' EXIT

# Source install.sh in test mode to get install_plugin_files function
export INSTALL_SH_TESTING=1
# shellcheck source=../install.sh
. "$REPO_ROOT/install.sh"

# Run install_plugin_files with a dummy version and extract dir
DUMMY_VERSION="v0.1.0"
EXTRACT_DIR="$TMPDIR_DRIFT/extract"
PLUGIN_DIR="$TMPDIR_DRIFT/plugin"
mkdir -p "$EXTRACT_DIR" "$PLUGIN_DIR"

# Create a dummy rusty-brain-hooks binary so install_plugin_files succeeds
touch "$EXTRACT_DIR/rusty-brain-hooks"
chmod +x "$EXTRACT_DIR/rusty-brain-hooks"

install_plugin_files "$DUMMY_VERSION" "$PLUGIN_DIR" "$EXTRACT_DIR"

CANONICAL="$REPO_ROOT/packaging/claude-code"
DRIFT=0

compare_file() {
  _canonical="$1"
  _generated="$2"
  _label="$3"

  if [ ! -f "$_canonical" ]; then
    printf 'MISSING canonical: %s\n' "$_label" >&2
    DRIFT=1
    return
  fi
  if [ ! -f "$_generated" ]; then
    printf 'MISSING generated: %s\n' "$_label" >&2
    DRIFT=1
    return
  fi

  if ! diff -u "$_canonical" "$_generated" >/dev/null 2>&1; then
    printf 'DRIFT detected: %s\n' "$_label" >&2
    diff -u "$_canonical" "$_generated" >&2 || true
    DRIFT=1
  fi
}

# hooks.json — exact match expected
compare_file "$CANONICAL/hooks/hooks.json" "$PLUGIN_DIR/hooks/hooks.json" "hooks/hooks.json"

# Skills — exact match expected
compare_file "$CANONICAL/skills/mind/SKILL.md" "$PLUGIN_DIR/skills/mind/SKILL.md" "skills/mind/SKILL.md"
compare_file "$CANONICAL/skills/memory/SKILL.md" "$PLUGIN_DIR/skills/memory/SKILL.md" "skills/memory/SKILL.md"

# Commands — exact match expected
for cmd in ask search recent stats; do
  compare_file "$CANONICAL/commands/${cmd}.md" "$PLUGIN_DIR/commands/${cmd}.md" "commands/${cmd}.md"
done

# plugin.json — normalize version field before comparison so differing versions don't cause false drift
# The canonical file has a real version; the generated file has the install-time version.
# Replace any semver "version": "X.Y.Z" with the canonical version from packaging/.
_canonical_version="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)".*/\1/p' "$CANONICAL/.claude-plugin/plugin.json" | head -n 1)"
sed "s/\"version\"[[:space:]]*:[[:space:]]*\"[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\"/\"version\": \"${_canonical_version}\"/" \
  "$PLUGIN_DIR/.claude-plugin/plugin.json" > "$TMPDIR_DRIFT/plugin_normalized.json"
compare_file "$CANONICAL/.claude-plugin/plugin.json" "$TMPDIR_DRIFT/plugin_normalized.json" ".claude-plugin/plugin.json"

# marketplace.json
sed "s/\"version\"[[:space:]]*:[[:space:]]*\"[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\"/\"version\": \"${_canonical_version}\"/" \
  "$PLUGIN_DIR/marketplace.json" > "$TMPDIR_DRIFT/marketplace_normalized.json"
compare_file "$CANONICAL/marketplace.json" "$TMPDIR_DRIFT/marketplace_normalized.json" "marketplace.json"

if [ "$DRIFT" -ne 0 ]; then
  printf '\nManifest drift detected! Update install.sh inline copies to match packaging/ canonical files.\n' >&2
  exit 1
fi

printf 'All manifests in sync.\n'
