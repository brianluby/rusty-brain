#!/usr/bin/env bash
# W3.2 elicitation scorecard (plan §6 / W3.2). Measures whether the three
# elicitation channels cause the MODEL to use memory unprompted:
#
#   (a) deterministic recall  — UserPromptSubmit hook injects recalled memories
#   (b) policy                — CLAUDE.md memory-policy block + memory skill
#   (c) tool surface          — trigger-condition tool descriptions + MCP
#                               instructions + permissions.allow allowlisting
#
# For each scenario (crates/rb-eval/scorecard/w32_elicitation_scenarios.json) it
# runs TWO real headless `claude -p` sessions in a fresh namespace:
#   1. PLANT: the user states a decision/preference/constraint, NOT mentioning
#      memory. Measured: `remember_fired` = the session made a model-initiated
#      mcp__rusty-brain__remember tool call (channels b+c worked).
#   2. WORK : a fresh-context session of the SAME namespace performs a task where
#      the decision matters. Measured: `recall_before_work` = recalled context
#      reached the model before its first edit/command — via the deterministic
#      UserPromptSubmit injection (channel a) and/or a model-initiated recall.
#
# PASS (tracked per release): remember_fired in >=70% of scenarios,
# recall_before_work in >=50%.
#
# STATUS: harness landed; the measured >=10-session run is DEFERRED — it needs
# `claude` + ANTHROPIC_API_KEY + a small spend budget (the same infra plan §6
# scopes to the W3.5 nightly eval). Run it where that infra exists:
#   cargo build --release -p rusty-brain -p rb-hooks -p rb-install
#   scripts/w32-elicitation-scorecard.sh --bin-dir target/release
# `--self-test` validates the scoring math with NO API and is safe in CI.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS="$REPO_ROOT/crates/rb-eval/scorecard/w32_elicitation_scenarios.json"
MODEL="${RB_SCORECARD_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_SCORECARD_MAX_BUDGET_USD:-1}"

usage() { sed -n '2,33p' "$0"; exit 2; }

# --- scoring (pure; exercised by --self-test) --------------------------------
# Given counts, print the rates + PASS/FAIL against the thresholds and return
# 0 on pass, 1 on fail. Uses awk for float math (portable, no bc dependency).
score() {
  local total="$1" remembered="$2" recalled="$3"
  local rt_thresh="$4" rc_thresh="$5"
  awk -v t="$total" -v rem="$remembered" -v rec="$recalled" \
      -v rtt="$rt_thresh" -v rct="$rc_thresh" 'BEGIN {
    if (t == 0) { print "no scenarios"; exit 2 }
    rr = rem / t; cr = rec / t;
    printf "remember_fired:      %d/%d = %.2f  (threshold %.2f)\n", rem, t, rr, rtt;
    printf "recall_before_work:  %d/%d = %.2f  (threshold %.2f)\n", rec, t, cr, rct;
    pass = (rr >= rtt && cr >= rct);
    printf "scorecard: %s\n", (pass ? "PASS" : "FAIL");
    exit (pass ? 0 : 1);
  }'
}

self_test() {
  echo "== scorecard self-test (scoring logic only; no API) =="
  local fail=0
  # 8/10 remember (0.80 >= 0.70), 6/10 recall (0.60 >= 0.50) -> PASS.
  if score 10 8 6 0.70 0.50 >/dev/null; then echo "ok: passing case passes"; else echo "BUG: passing case failed"; fail=1; fi
  # 6/10 remember (0.60 < 0.70) -> FAIL on remember.
  if score 10 6 9 0.70 0.50 >/dev/null; then echo "BUG: sub-threshold remember passed"; fail=1; else echo "ok: sub-threshold remember fails"; fi
  # 9/10 remember but 4/10 recall (0.40 < 0.50) -> FAIL on recall.
  if score 10 9 4 0.70 0.50 >/dev/null; then echo "BUG: sub-threshold recall passed"; fail=1; else echo "ok: sub-threshold recall fails"; fi
  # exact-threshold boundary (0.70 and 0.50) -> PASS (>=).
  if score 10 7 5 0.70 0.50 >/dev/null; then echo "ok: exact threshold passes"; else echo "BUG: exact threshold failed"; fail=1; fi
  [ "$fail" -eq 0 ] && { echo "self-test PASS"; return 0; } || { echo "self-test FAIL" >&2; return 1; }
}

# --- arg parsing -------------------------------------------------------------
BIN_DIR=""
MODE="run"
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) MODE="self-test"; shift ;;
    --bin-dir) BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi

# --- real run prerequisites --------------------------------------------------
[ -n "$BIN_DIR" ] || usage
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin (cargo build --release -p rusty-brain -p rb-hooks -p rb-install)" >&2; exit 1; }
done
command -v claude  >/dev/null 2>&1 || { echo "claude not on PATH (npm install -g @anthropic-ai/claude-code)" >&2; exit 1; }
command -v jq      >/dev/null 2>&1 || { echo "jq not on PATH (needed to read the scenarios JSON)" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH (needed to edit .claude/settings.json)" >&2; exit 1; }
[ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "ANTHROPIC_API_KEY is not set — the measured run is DEFERRED until it is" >&2; exit 1; }
[ -f "$SCENARIOS" ] || { echo "scenarios file not found: $SCENARIOS" >&2; exit 1; }

RT_THRESH="$(jq -r '.pass_thresholds.remember_fired_rate' "$SCENARIOS")"
RC_THRESH="$(jq -r '.pass_thresholds.recall_before_work_rate' "$SCENARIOS")"

WORKROOT="$(mktemp -d)"
cleanup() { rm -rf "$WORKROOT" 2>/dev/null || true; }
trap cleanup EXIT

# True if the JSONL transcript under $1 records a model-initiated call to the
# named tool ($2). Claude Code writes tool_use blocks naming MCP tools as
# mcp__<server>__<tool>; a substring match is sufficient and robust to format.
transcript_has_tool() {
  local proj_transcripts_dir="$1" tool="$2"
  grep -rqlF "$tool" "$proj_transcripts_dir" 2>/dev/null
}

run_scenario() {
  # Returns "<remember_fired> <recall_before_work>" as 0/1 pair on stdout.
  local id="$1" plant="$2" work="$3"
  local home="$WORKROOT/$id/home" proj="$WORKROOT/$id/proj"
  mkdir -p "$home" "$proj"
  (
    export HOME="$home"
    unset XDG_RUNTIME_DIR XDG_CACHE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME
    unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET
    export RUSTY_BRAIN_SOCKET="$WORKROOT/$id/sock"
    export RUSTY_BRAIN_NAMESPACE="rb-scorecard-$id"
    export PATH="$BIN_DIR:$PATH"
    printf '{"hasCompletedOnboarding": true}\n' > "$HOME/.claude.json"
    cd "$proj"

    # Install hooks + the W3.2 channels (the installer now writes the
    # permissions.allow mcp__rusty-brain__* entry — channel c). MCP server config
    # is project-scoped (.mcp.json + enableAllProjectMcpServers approves it
    # non-interactively; W3.6 will commit these natively).
    rusty-brain-install install --agents claude-code >/dev/null
    cat > "$proj/.mcp.json" <<EOF
{ "mcpServers": { "rusty-brain": { "command": "$BIN_DIR/rusty-brain", "args": ["mcp"] } } }
EOF
    python3 - "$proj/.claude/settings.json" <<'PY'
import json, sys
p = sys.argv[1]
with open(p) as f: s = json.load(f)
s["enableAllProjectMcpServers"] = True
with open(p, "w") as f: json.dump(s, f, indent=2); f.write("\n")
PY

    common=(--setting-sources project --model "$MODEL" --max-budget-usd "$MAX_BUDGET_USD"
            --permission-mode acceptEdits --allowedTools "Bash Edit Write")

    # 1) PLANT session: state the decision; let the model elect to remember.
    claude -p "$plant" "${common[@]}" >"$WORKROOT/$id/plant.log" 2>&1 || true
    # 2) WORK session (fresh context, same namespace): UserPromptSubmit recall
    #    fires deterministically before the model acts.
    claude -p "$work" "${common[@]}" >"$WORKROOT/$id/work.log" 2>&1 || true

    local transcripts="$HOME/.claude/projects"
    local remembered=0 recalled=0
    transcript_has_tool "$transcripts" "mcp__rusty-brain__remember" && remembered=1
    # recall_before_work: the deterministic UserPromptSubmit injection reached
    # the model (its block header is recorded in the work turn's context) OR the
    # model made its own recall call.
    if grep -rqlF "Memories relevant to this prompt" "$transcripts" 2>/dev/null \
       || transcript_has_tool "$transcripts" "mcp__rusty-brain__recall"; then
      recalled=1
    fi
    echo "$remembered $recalled"
  )
}

echo "== W3.2 elicitation scorecard (model=$MODEL, budget=\$$MAX_BUDGET_USD/session) =="
total=0; remembered=0; recalled=0
mapfile -t rows < <(jq -c '.scenarios[]' "$SCENARIOS")
for row in "${rows[@]}"; do
  id="$(jq -r '.id' <<<"$row")"
  plant="$(jq -r '.plant' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"
  echo "-- scenario: $id"
  read -r rem rec < <(run_scenario "$id" "$plant" "$work")
  total=$((total + 1)); remembered=$((remembered + rem)); recalled=$((recalled + rec))
  echo "   remember_fired=$rem recall_before_work=$rec"
done

echo
score "$total" "$remembered" "$recalled" "$RT_THRESH" "$RC_THRESH"
