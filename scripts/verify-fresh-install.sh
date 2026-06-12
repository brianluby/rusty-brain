#!/usr/bin/env bash
# Phase 0 gate tier (c): scripted fresh-machine checklist (plan §3, W0.6).
#
# Consumes a W0.6 release artifact, installs it into an isolated prefix with a
# fresh HOME, and proves the out-of-the-box loop:
#   checksum-verified install -> auto-started daemon -> remember -> recall,
# against ONE daemon and ONE owner-only (0600) database. Also plants a fake
# secret through the real hook capture path and greps the raw DB + WAL for it:
# zero plaintext hits, while the redaction marker must be present.
#
# Usage:
#   scripts/verify-fresh-install.sh --tag v0.2.0      # download from the GitHub release (needs gh)
#   scripts/verify-fresh-install.sh --tarball PATH    # use a local artifact (checksum skipped
#                                                     # unless SHA256SUMS sits next to it)
set -euo pipefail

usage() { sed -n '2,16p' "$0"; exit 2; }

TAG=""
TARBALL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
    --tarball) TARBALL="${2:?--tarball needs a value}"; shift 2 ;;
    *) usage ;;
  esac
done
[ -n "$TAG" ] || [ -n "$TARBALL" ] || usage

REPO="brianluby/rusty-brain"
SENTINEL="fresh-install gate sentinel $(date +%s)"
PLANTED="gate-planted-secret-3f9c2b"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET="aarch64-apple-darwin" ;;
  Darwin-x86_64) TARGET="x86_64-apple-darwin" ;;
  Linux-x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

# Portable octal-mode and sha256 helpers (macOS vs GNU).
mode_of() { stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"; }
sha_check() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum --check --ignore-missing SHA256SUMS
  else shasum -a 256 --check --ignore-missing SHA256SUMS; fi
}

WORKDIR="$(mktemp -d)"
DAEMON_PID=""
cleanup() {
  status=$?
  if [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill "$DAEMON_PID" 2>/dev/null || true
  fi
  if [ "$status" -eq 0 ]; then
    rm -rf "$WORKDIR"
    echo "PASS: fresh-install checklist"
  else
    echo "FAIL: fresh-install checklist (workdir kept: $WORKDIR)" >&2
  fi
}
trap cleanup EXIT

# 1. Fetch + verify + unpack the artifact.
cd "$WORKDIR"
if [ -n "$TAG" ]; then
  gh release download "$TAG" --repo "$REPO" \
    --pattern "*-${TARGET}.tar.gz" --pattern SHA256SUMS
  sha_check
  TARBALL="$(ls rusty-brain-*-"${TARGET}".tar.gz)"
else
  TARBALL="$(cd "$(dirname "$TARBALL")" && pwd)/$(basename "$TARBALL")"
  if [ -f "$(dirname "$TARBALL")/SHA256SUMS" ]; then
    cp "$(dirname "$TARBALL")/SHA256SUMS" .
    cp "$TARBALL" .
    sha_check
  else
    echo "note: no SHA256SUMS next to the tarball; checksum skipped (local mode)"
  fi
fi
BIN_DIR="$WORKDIR/bin"
mkdir -p "$BIN_DIR"
tar -xzf "$TARBALL" --strip-components=1 -C "$BIN_DIR"
for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary in tarball: $bin" >&2; exit 1; }
done

# 2. Fresh-machine environment: empty HOME, no XDG dirs, no overrides except a
# SHORT socket path (default cache-dir sockets can exceed the 104-byte macOS
# sun_path limit under a temp HOME). The DB stays on default resolution so the
# real path + permission code runs.
#
# Hermetic: unset every config-bearing var the auto-started daemon could
# inherit — rb_config::FORWARD_ENV secrets plus the frozen LEGACY_KNOB_ENV
# compat knobs (crates/rb-config/src/lib.rs) — plus the path/namespace
# overrides, BEFORE re-exporting
# our own. A developer's VOYAGE_API_KEY or RB_* settings must not leak into the
# fresh-machine simulation.
export HOME="$WORKDIR/home"
mkdir -p "$HOME"
unset XDG_RUNTIME_DIR XDG_CACHE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME
unset VOYAGE_API_KEY RB_EMBED_BACKEND RB_LOCAL_MODEL \
      RB_ENRICH_BASE_URL RB_ENRICH_MODEL RB_ENRICH_API_KEY \
      RB_JOBS_CONFIG RB_ACCEPT_MODEL_CHANGE RUSTY_BRAIN_IDLE_TIMEOUT_SECS
unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE
export RUSTY_BRAIN_SOCKET="$WORKDIR/rb/sock"
export PATH="$BIN_DIR:$PATH"
cd "$HOME"

# 3. First client command auto-starts the daemon (the path a fresh user hits).
rusty-brain remember "$SENTINEL" --type insight --importance 7
rusty-brain status >/dev/null

# 4. Recall round-trips the sentinel.
rusty-brain recall "gate sentinel" | grep -F "fresh-install gate sentinel" >/dev/null \
  || { echo "recall did not return the sentinel" >&2; exit 1; }

# 5. Plant a fake secret through the REAL hook capture path; the daemon must
# store the redacted summary, never the plaintext value.
printf '%s' "{\"hook_event_name\":\"PostToolUse\",\"session_id\":\"gate\",\"cwd\":\"$HOME\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"export API_TOKEN=${PLANTED}\"},\"tool_response\":\"ok\"}" \
  | rusty-brain-hooks --agent claude-code >/dev/null

# 6. Exactly one DB, owner-only, with the capture landed and zero plaintext hits.
DB_COUNT="$(find "$HOME" -name memory.db | wc -l | tr -d ' ')"
[ "$DB_COUNT" = "1" ] || { echo "expected exactly 1 memory.db under HOME, found $DB_COUNT" >&2; exit 1; }
DB="$(find "$HOME" -name memory.db)"
MODE="$(mode_of "$DB")"
[ "$MODE" = "600" ] || { echo "db must be 0600, got $MODE ($DB)" >&2; exit 1; }
# Grep only the siblings that exist: under `set -euo pipefail`, grep's exit 2
# on a missing $DB-wal would fail the marker gate spuriously.
FILES=("$DB")
for sibling in "$DB-wal" "$DB-shm"; do
  if [ -e "$sibling" ]; then
    MODE="$(mode_of "$sibling")"
    [ "$MODE" = "600" ] || { echo "${sibling##*/} must be 0600, got $MODE" >&2; exit 1; }
    FILES+=("$sibling")
  fi
done
grep -aq "REDACTED:credential" "${FILES[@]}" \
  || { echo "hook capture did not land in the DB (no redaction marker found)" >&2; exit 1; }
if grep -aq "$PLANTED" "${FILES[@]}"; then
  echo "planted secret found in plaintext in the DB" >&2
  exit 1
fi

# 7. Exactly one daemon: the auto-started one, identified by its pidfile.
PIDFILE="$WORKDIR/rb/sock.pid"
[ -f "$PIDFILE" ] || { echo "daemon pidfile missing: $PIDFILE" >&2; exit 1; }
DAEMON_PID="$(cat "$PIDFILE")"
kill -0 "$DAEMON_PID" || { echo "daemon (pid $DAEMON_PID) is not running" >&2; exit 1; }
