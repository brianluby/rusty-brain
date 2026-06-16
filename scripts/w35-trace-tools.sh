#!/usr/bin/env bash
# W3.5 turn-overhead TRACE (diagnostic, not a gate). Companion to
# scripts/w35-ab-eval.sh. The gate run found memory-on ties claude-md on success
# but LOSES on turns; code analysis blamed the live rusty-brain MCP server's
# "recall BEFORE starting a task" instructions + auto-approved recall/context/get
# tools (present ONLY in the memory-on arm, NOT removed by the eval's confound
# strip, which deletes only CLAUDE.md + the skill). This script CONFIRMS or
# REFUTES that from real transcripts: for each scenario it runs the WORK session
# under memory-on and claude-md with `--output-format stream-json --verbose` and
# tallies the per-session tool_use calls. If memory-on emits
# mcp__rusty-brain__{recall,context,get} turns that claude-md cannot, the
# hypothesis holds.
#
# Output: a structured TSV (scenario, arm, run, is_error, num_turns, cost,
# n_tool_calls, n_rusty_brain_calls, tool_sequence) — TOOL NAMES ONLY, no model
# free-text, so it is safe to upload as a CI artifact (mirrors w35-ab-eval.sh).
#
# Usage: w35-trace-tools.sh --bin-dir DIR [--runs N] [--out FILE] [--scenarios "id id ..."]
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS_JSON="$REPO_ROOT/crates/rb-eval/scorecard/w35_ab_scenarios.json"
MODEL="${RB_W35_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_W35_MAX_BUDGET_USD:-0.50}"

BIN_DIR=""; RUNS="2"; OUT=""; SCENARIOS="time-unix-millis config-settings-toml arch-single-writer"
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir)   BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)      RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --out)       OUT="${2:?--out needs a value}"; shift 2 ;;
    --scenarios) SCENARIOS="${2:?--scenarios needs a value}"; shift 2 ;;
    -h|--help)   sed -n '2,20p' "$0"; exit 2 ;;
    *)           echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

case "$RUNS" in ''|*[!0-9]*|0) echo "--runs must be a positive integer" >&2; exit 2 ;; esac
# Whitelist the scenario list charset: ids are [a-z0-9-], space-separated. Reject
# anything else so a dispatch value can never inject shell.
case "$SCENARIOS" in *[!a-z0-9\ _-]*) echo "--scenarios may contain only [a-z0-9 _-]" >&2; exit 2 ;; esac

[ -n "$BIN_DIR" ] || { echo "need --bin-dir" >&2; exit 2; }
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin" >&2; exit 1; }
done
command -v claude  >/dev/null 2>&1 || { echo "claude not on PATH" >&2; exit 1; }
command -v jq      >/dev/null 2>&1 || { echo "jq not on PATH" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH" >&2; exit 1; }
[ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "ANTHROPIC_API_KEY is not set" >&2; exit 1; }
[ -f "$SCENARIOS_JSON" ] || { echo "scenarios file not found: $SCENARIOS_JSON" >&2; exit 1; }

SESSION_TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then SESSION_TIMEOUT="timeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"
elif command -v gtimeout >/dev/null 2>&1; then SESSION_TIMEOUT="gtimeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"; fi

unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE RUSTY_BRAIN_IDLE_TIMEOUT_SECS

WORKROOT="$(mktemp -d "${TMPDIR:-/tmp}/rb-w35-trace.XXXXXX")"
RESULTS="${OUT:-$WORKROOT/trace.tsv}"; : > "$RESULTS"
cleanup() { rm -rf "$WORKROOT" 2>/dev/null || true; }
trap cleanup EXIT

seed_home() { mkdir -p "$1"; printf '{"hasCompletedOnboarding": true}\n' > "$1/.claude.json"; }

run_session() { # home proj prompt log [extra args...]
  local home="$1" proj="$2" prompt="$3" log="$4"; shift 4
  (
    export HOME="$home"
    unset XDG_RUNTIME_DIR XDG_CACHE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME
    export PATH="$BIN_DIR:$PATH"
    cd "$proj"
    ${SESSION_TIMEOUT} claude -p "$prompt" \
      --setting-sources project --model "$MODEL" --max-budget-usd "$MAX_BUDGET_USD" \
      --permission-mode acceptEdits --allowedTools "Bash Edit Write" "$@" \
      >"$log" 2>&1 || true
  )
}

install_rusty_brain() { # home proj
  local home="$1" proj="$2"
  (
    export HOME="$home"; export PATH="$BIN_DIR:$PATH"; cd "$proj"
    rusty-brain-install install --agents claude-code >/dev/null
    python3 - "$proj/.claude/settings.json" "$proj/.mcp.json" "$BIN_DIR/rusty-brain" <<'PY'
import json, sys
sp, mp, cmd = sys.argv[1], sys.argv[2], sys.argv[3]
json.dump({"mcpServers": {"rusty-brain": {"command": cmd, "args": ["mcp"]}}}, open(mp, "w"), indent=2)
s = json.load(open(sp)); s["enableAllProjectMcpServers"] = True
json.dump(s, open(sp, "w"), indent=2)
PY
  )
}

# Parse a stream-json --verbose work log -> append one TSV row + echo a summary.
emit_row() { # id arm run log
  local id="$1" arm="$2" run="$3" log="$4"
  local is_err turns cost names n_tool n_rb seq
  is_err="$(jq -r 'select(.type=="result")|.is_error' "$log" 2>/dev/null | tail -1)"; : "${is_err:=true}"
  turns="$(jq -r 'select(.type=="result")|.num_turns'  "$log" 2>/dev/null | tail -1)"; : "${turns:=0}"
  cost="$(jq -r 'select(.type=="result")|.total_cost_usd' "$log" 2>/dev/null | tail -1)"; : "${cost:=0}"
  names="$(jq -r 'select(.type=="assistant")|.message.content[]?|select(.type=="tool_use")|.name' "$log" 2>/dev/null)"
  if [ -n "$names" ]; then
    n_tool="$(printf '%s\n' "$names" | grep -c .)"
    n_rb="$(printf '%s\n' "$names" | grep -c 'mcp__rusty-brain__')"
    seq="$(printf '%s\n' "$names" | paste -sd, -)"
  else
    n_tool=0; n_rb=0; seq="-"
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$arm" "$run" "$is_err" "$turns" "$cost" "$n_tool" "$n_rb" "$seq" >> "$RESULTS"
  printf '   [%-9s] is_error=%s turns=%s cost=%s tool_calls=%s rusty_brain=%s seq=%s\n' \
    "$arm r$run" "$is_err" "$turns" "$cost" "$n_tool" "$n_rb" "$seq"
}

trace_scenario() { # row
  local row="$1"
  local id plant supersede claude_md work
  id="$(jq -r '.id' <<<"$row")"
  plant="$(jq -r '.plant' <<<"$row")"
  supersede="$(jq -r '.supersede // ""' <<<"$row")"
  claude_md="$(jq -r '.claude_md' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"
  local run=1
  while [ "$run" -le "$RUNS" ]; do
    echo "-- $id (run $run/$RUNS)"
    local base="$WORKROOT/$id-r$run"

    # memory-on: plant [-> supersede] -> work (shared store, short /tmp socket)
    local mbase="$base/on" db="$base/on/memory.db" ns="rb-w35t-$id-r$run"
    local sockdir; sockdir="$(mktemp -d /tmp/rbw35t.XXXXXX)"; local sock="$sockdir/s"
    local ph="$mbase/hp" pp="$mbase/pp" wh="$mbase/hw" wp="$mbase/wp"
    seed_home "$ph"; mkdir -p "$pp"; seed_home "$wh"; mkdir -p "$wp"
    install_rusty_brain "$ph" "$pp"; install_rusty_brain "$wh" "$wp"
    rm -f "$wp/CLAUDE.md"; rm -rf "$wp/.claude/skills"   # same confound strip as the gate
    (
      export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
      run_session "$ph" "$pp" "$plant" "$mbase/plant.log"
      [ -n "$supersede" ] && run_session "$ph" "$pp" "$supersede" "$mbase/sup.log"
      [ -f "$sock.pid" ] || { echo "ERROR: memory-on daemon did not auto-start for $id run $run" >&2; exit 1; }
      run_session "$wh" "$wp" "$work" "$mbase/work.jsonl" --output-format stream-json --verbose
      emit_row "$id" "memory-on" "$run" "$mbase/work.jsonl"
    )
    [ -f "$sock.pid" ] && kill "$(cat "$sock.pid" 2>/dev/null)" 2>/dev/null || true
    rm -rf "$sockdir" 2>/dev/null || true

    # claude-md: decision in CLAUDE.md, no rusty-brain
    local ch="$base/cmd/h" cpj="$base/cmd/p"
    seed_home "$ch"; mkdir -p "$cpj"
    printf '# Project conventions\n\n%s\n' "$claude_md" > "$cpj/CLAUDE.md"
    run_session "$ch" "$cpj" "$work" "$base/cmd/work.jsonl" --output-format stream-json --verbose
    emit_row "$id" "claude-md" "$run" "$base/cmd/work.jsonl"

    run=$((run + 1))
  done
}

echo "== W3.5 tool-trace (model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
echo "   scenarios: $SCENARIOS"
printf 'scenario\tarm\trun\tis_error\tnum_turns\tcost\tn_tool_calls\tn_rusty_brain_calls\ttool_sequence\n'
for id in $SCENARIOS; do
  row="$(jq -c --arg id "$id" '.scenarios[]|select(.id==$id)' "$SCENARIOS_JSON")"
  [ -n "$row" ] || { echo "scenario not found: $id" >&2; continue; }
  trace_scenario "$row"
done

echo
echo "== per-arm aggregate (mean turns, mean rusty-brain calls) =="
awk -F'\t' '
  { arm=$2; n[arm]++; t[arm]+=$5; rb[arm]+=$8 }
  END {
    printf "%-10s %6s %12s %18s\n", "arm", "runs", "mean_turns", "mean_rb_calls";
    split("memory-on claude-md", o, " ");
    for (i=1;i<=2;i++){ a=o[i]; if(n[a]) printf "%-10s %6d %12.2f %18.2f\n", a, n[a], t[a]/n[a], rb[a]/n[a]; }
  }' "$RESULTS"
echo
echo "report: $RESULTS"
