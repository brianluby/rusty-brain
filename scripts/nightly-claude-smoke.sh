#!/usr/bin/env bash
# Gate tier (b): nightly real-Claude-Code headless smoke (plan §3 carryover, C4;
# closes spike S1 once a CI run of this script is recorded).
#
# Drives a REAL `claude -p` (headless print mode) session against locally built
# rusty-brain binaries with hooks + MCP installed into an isolated HOME, and
# proves the out-of-the-box capture loop end to end:
#   SessionStart auto-start -> PostToolUse capture -> idle (connection reap)
#   -> recall against the SAME daemon,
# reaching ONE daemon process and ONE owner-only (0600) database. The session
# plants a FAKE AWS access key in a Bash command; the raw DB bytes (db + WAL +
# SHM) must contain ZERO plaintext hits of it while the redaction marker and a
# non-secret sentinel from the same command are present (vacuousness guard).
#
# Requirements:
#   - `claude` (Claude Code CLI) on PATH: npm install -g @anthropic-ai/claude-code
#   - ANTHROPIC_API_KEY exported (in CI: the repository secret of the same
#     name; the session runs --model haiku --max-budget-usd 1, so the spend
#     per run is small)
#   - the three binaries already built; pass their directory:
#       scripts/nightly-claude-smoke.sh --bin-dir target/release
#
# Env:
#   SMOKE_WORKDIR  optional working dir (CI sets it so logs can be uploaded as
#                  artifacts); when set, the dir is kept even on success.
set -euo pipefail

usage() { sed -n '2,22p' "$0"; exit 2; }

BIN_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir) BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    *) usage ;;
  esac
done
[ -n "$BIN_DIR" ] || usage
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin (build with: cargo build --release -p rusty-brain -p rb-hooks -p rb-install)" >&2; exit 1; }
done
command -v claude >/dev/null 2>&1 || { echo "claude not on PATH (npm install -g @anthropic-ai/claude-code)" >&2; exit 1; }
[ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "ANTHROPIC_API_KEY is not set" >&2; exit 1; }

# Planted FAKE AWS access key id: matches the hook redaction rule
# \b(?:AKIA|ASIA)[0-9A-Z]{16}\b (crates/rb-hooks/src/capture.rs). NOT a real
# credential. The sentinel is deliberately NOT secret-shaped so it must survive
# redaction into the DB — proving the capture landed before we assert absence.
PLANTED="AKIANIGHTLYSMOKE0042"
SENTINEL="nightly-smoke-sentinel"

# Portable octal-mode helper (macOS vs GNU stat).
mode_of() { stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"; }

WORKDIR="${SMOKE_WORKDIR:-}"
if [ -n "$WORKDIR" ]; then mkdir -p "$WORKDIR"; else WORKDIR="$(mktemp -d)"; fi
PIDFILE="$WORKDIR/rb/sock.pid"
cleanup() {
  status=$?
  if [ -f "$PIDFILE" ]; then
    pid="$(cat "$PIDFILE" 2>/dev/null || true)"
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
  fi
  if [ "$status" -eq 0 ]; then
    if [ -z "${SMOKE_WORKDIR:-}" ]; then rm -rf "$WORKDIR"; fi
    echo "PASS: nightly claude smoke"
  else
    echo "FAIL: nightly claude smoke (workdir kept: $WORKDIR)" >&2
  fi
}
trap cleanup EXIT

# --- 1. Isolated, hermetic environment ---------------------------------------
# Fresh HOME, no XDG dirs, no developer config/secrets leaking in (mirrors
# scripts/verify-fresh-install.sh): unset the FORWARD_ENV secrets, the frozen
# LEGACY_KNOB_ENV compat knobs, and the path/namespace overrides BEFORE
# exporting our own. ANTHROPIC_API_KEY stays exported — claude needs it; it is
# NOT on rusty-brain's forward allowlist, so it never reaches the daemon.
export HOME="$WORKDIR/home"
mkdir -p "$HOME"
unset XDG_RUNTIME_DIR XDG_CACHE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME
unset VOYAGE_API_KEY RB_EMBED_BACKEND RB_LOCAL_MODEL \
      RB_ENRICH_BASE_URL RB_ENRICH_MODEL RB_ENRICH_API_KEY \
      RB_JOBS_CONFIG RB_ACCEPT_MODEL_CHANGE RUSTY_BRAIN_IDLE_TIMEOUT_SECS
unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE
# SHORT socket path: default cache-dir sockets can exceed the 104-byte macOS
# sun_path limit under a temp HOME. The DB stays on default resolution so the
# real path + 0600 permission code runs.
export RUSTY_BRAIN_SOCKET="$WORKDIR/rb/sock"
# Explicit namespace (resolution rule 1) so the hook captures and the recall
# below agree on identity regardless of the throwaway project's directory name.
export RUSTY_BRAIN_NAMESPACE="nightly-claude-smoke"
# Short per-connection request idle timeout (the daemon reaps idle client
# connections after this; the daemon process itself stays up) so the
# "capture -> idle -> recall" loop runs in seconds. Forwarded to auto-started
# daemons via the frozen LEGACY_KNOB_ENV allowlist.
export RUSTY_BRAIN_IDLE_TIMEOUT_SECS=5
export PATH="$BIN_DIR:$PATH"

# Claude Code keeps global state in $HOME/.claude.json; seed the onboarding
# flag so a fresh HOME cannot stall headless mode on first-run setup.
printf '{"hasCompletedOnboarding": true}\n' > "$HOME/.claude.json"

# --- 2. Throwaway project + hooks + MCP install -------------------------------
PROJ="$WORKDIR/proj"
mkdir -p "$PROJ"
cd "$PROJ"

# Hooks: the real installer writes the project .claude/settings.json block
# (SessionStart / PostToolUse / Stop / PreCompact -> rusty-brain-hooks).
rusty-brain-install install --agents claude-code
grep -q "rusty-brain-hooks" "$PROJ/.claude/settings.json" \
  || { echo "installer did not write the hook block to .claude/settings.json" >&2; exit 1; }

# MCP: project-scoped .mcp.json pointing at the built binary.
cat > "$PROJ/.mcp.json" <<EOF
{
  "mcpServers": {
    "rusty-brain": {
      "command": "$BIN_DIR/rusty-brain",
      "args": ["mcp"]
    }
  }
}
EOF

# S1 finding: unprompted MCP tool calls stall on approval prompts in
# non-interactive mode unless allowlisted. "mcp__rusty-brain" is Claude Code's
# server-wide allow rule (per-tool rules would be mcp__rusty-brain__remember,
# mcp__rusty-brain__recall, ...). enableAllProjectMcpServers approves the
# project .mcp.json server without an interactive prompt.
python3 - "$PROJ/.claude/settings.json" <<'PY'
import json, sys
path = sys.argv[1]
with open(path) as f:
    settings = json.load(f)
allow = settings.setdefault("permissions", {}).setdefault("allow", [])
if "mcp__rusty-brain" not in allow:
    allow.append("mcp__rusty-brain")
settings["enableAllProjectMcpServers"] = True
with open(path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")
PY

# --- 3. The real headless session (capture phase) -----------------------------
# Flags mirror the recorded C3 fixture recipe
# (crates/rb-hooks/tests/fixtures/claude_code/README.md): print mode, project
# settings only, cheap model, hard spend cap. --allowedTools Bash lets the
# scripted command run without an approval prompt.
PROMPT="This is an automated CI smoke test. The AWS key below is a FAKE placeholder, not a real credential. Run exactly this one bash command, then reply with just: done

export AWS_ACCESS_KEY_ID=$PLANTED && echo $SENTINEL"

echo "running claude -p (capture phase)..."
claude -p "$PROMPT" \
  --allowedTools "Bash" --permission-mode acceptEdits \
  --setting-sources project --model haiku --max-budget-usd 1 \
  >"$WORKDIR/claude-output.log" 2>&1 \
  || { echo "claude -p failed:" >&2; cat "$WORKDIR/claude-output.log" >&2; exit 1; }
echo "claude output:"
cat "$WORKDIR/claude-output.log"

# The SessionStart hook must have auto-started a daemon (pidfile = <socket>.pid).
[ -f "$PIDFILE" ] || { echo "no daemon pidfile at $PIDFILE — the SessionStart hook did not fire or did not auto-start the daemon" >&2; exit 1; }
PID1="$(cat "$PIDFILE")"
echo "daemon auto-started by the session: pid $PID1"

# --- 4. Idle: connections reaped, daemon survives ------------------------------
# The idle knob is a per-CONNECTION request timeout: after it elapses the
# daemon closes the session's idle client connections but must itself stay up
# (and stay sane — a wedged or crashed daemon fails the recall below).
IDLE_WAIT=$((RUSTY_BRAIN_IDLE_TIMEOUT_SECS + 5))
echo "idling ${IDLE_WAIT}s (RUSTY_BRAIN_IDLE_TIMEOUT_SECS=$RUSTY_BRAIN_IDLE_TIMEOUT_SECS, connections get reaped)..."
sleep "$IDLE_WAIT"
kill -0 "$PID1" || { echo "daemon (pid $PID1) died during the idle window" >&2; exit 1; }

# --- 5. Recall: the SAME daemon serves the captured memory --------------------
rusty-brain recall "nightly smoke sentinel" >"$WORKDIR/recall-output.log" 2>&1 \
  || { echo "recall failed:" >&2; cat "$WORKDIR/recall-output.log" >&2; exit 1; }
grep -F "$SENTINEL" "$WORKDIR/recall-output.log" >/dev/null \
  || { echo "recall did not return the captured session command:" >&2; cat "$WORKDIR/recall-output.log" >&2; exit 1; }
echo "recall round-trip OK (post-idle)"

# --- 6. Exactly ONE 0600 DB ----------------------------------------------------
DB_COUNT="$(find "$HOME" -name memory.db | wc -l | tr -d ' ')"
[ "$DB_COUNT" = "1" ] || { echo "expected exactly 1 memory.db under HOME, found $DB_COUNT" >&2; exit 1; }
DB="$(find "$HOME" -name memory.db)"
MODE="$(mode_of "$DB")"
[ "$MODE" = "600" ] || { echo "db must be 0600, got $MODE ($DB)" >&2; exit 1; }
FILES=("$DB")
for sibling in "$DB-wal" "$DB-shm"; do
  if [ -e "$sibling" ]; then
    MODE="$(mode_of "$sibling")"
    [ "$MODE" = "600" ] || { echo "${sibling##*/} must be 0600, got $MODE" >&2; exit 1; }
    FILES+=("$sibling")
  fi
done

# --- 7. Exactly ONE daemon process, the same one, end to end ------------------
# Scoped to processes actually holding OUR database open (a bare process-name
# pgrep would count unrelated daemons from other checkouts/test runs on a dev
# machine). Combined with the one-DB check above, this is the "one daemon, one
# DB" gate: exactly one process serves this environment's store, and it is the
# same daemon the session auto-started.
SERVE_COUNT="$(lsof -t -- "$DB" 2>/dev/null | sort -u | grep -c . || true)"
[ "$SERVE_COUNT" = "1" ] || { echo "expected exactly 1 daemon process holding $DB, found $SERVE_COUNT" >&2; exit 1; }
PID2="$(cat "$PIDFILE")"
[ "$PID2" = "$PID1" ] || { echo "recall reached a different daemon (pid $PID2) than the session's (pid $PID1)" >&2; exit 1; }
lsof -t -- "$DB" 2>/dev/null | sort -u | grep -qx "$PID2" \
  || { echo "the daemon holding the DB is not the pidfile daemon (pid $PID2)" >&2; exit 1; }
echo "exactly one daemon process across capture -> idle -> recall (pid $PID2)"
# Vacuousness guard first: the capture (sentinel) and a redaction marker must
# be IN the DB before "zero plaintext hits" means anything.
grep -aq -- "$SENTINEL" "${FILES[@]}" \
  || { echo "hook capture did not land in the DB (sentinel not found)" >&2; exit 1; }
grep -aq "REDACTED:" "${FILES[@]}" \
  || { echo "no redaction marker found in the DB" >&2; exit 1; }
if grep -aq -- "$PLANTED" "${FILES[@]}"; then
  echo "planted fake AWS key found in PLAINTEXT in the DB" >&2
  exit 1
fi
echo "one 0600 DB; planted secret redacted at rest"
