#!/usr/bin/env bash
# W3.5 turn-overhead + correctness TRACE (diagnostic, not a gate). Companion to
# scripts/w35-ab-eval.sh. Runs the WORK session under memory-on and claude-md
# with `--output-format stream-json --verbose` and reports, per run:
#   - is_error / num_turns
#   - SUCCESS (the gate's judge: the model's OUTPUT contains `expect` and not
#     `forbid`) — so a "fast" answer that is WRONG is not mistaken for a win
#   - the rusty-brain MCP tool_use tally (recall/context/get) + full tool sequence
# and, once per memory-on scenario, the INJECTED summary line (what recall
# surfaces for the work query, ~what the UserPromptSubmit hook injects) plus
# whether it contains `expect`. This pinpoints whether a memory-on failure is a
# thin/!expect injection (summary problem) vs. a present-but-ignored fact.
#
# Output TSV columns: scenario, arm, run, is_error, success, num_turns,
# n_rusty_brain_calls, injected_has_expect, tool_sequence. Tool NAMES + booleans
# + the injected line (derived from the authored plant text, no model free-text),
# safe to upload as a CI artifact. The full injected line is also echoed to the
# job log for inspection.
#
# Usage: w35-trace-tools.sh --bin-dir DIR [--runs N] [--out FILE] [--scenarios "id id ..."]
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS_JSON="$REPO_ROOT/crates/rb-eval/scorecard/w35_ab_scenarios.json"
MODEL="${RB_W35_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_W35_MAX_BUDGET_USD:-0.50}"

BIN_DIR=""; RUNS="2"; OUT=""; SCENARIOS="arch-single-writer test-prefix-spec stale-trap-http-switch"
while [ $# -gt 0 ]; do
  case "$1" in
    --bin-dir)   BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)      RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --out)       OUT="${2:?--out needs a value}"; shift 2 ;;
    --scenarios) SCENARIOS="${2:?--scenarios needs a value}"; shift 2 ;;
    -h|--help)   sed -n '2,25p' "$0"; exit 2 ;;
    *)           echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

case "$RUNS" in ''|*[!0-9]*|0) echo "--runs must be a positive integer" >&2; exit 2 ;; esac
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

# success = expect present AND forbid absent (case-insensitive substring), the
# gate's judge (scripts/w35-ab-eval.sh).
judge() { # textfile expect forbid -> 0|1
  local f="$1" expect="$2" forbid="$3" s=0
  grep -iqF -- "$expect" "$f" 2>/dev/null && s=1
  [ -n "$forbid" ] && grep -iqF -- "$forbid" "$f" 2>/dev/null && s=0
  echo "$s"
}

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

# Run one work session (stream-json), judge it, append a TSV row + echo a summary.
emit_row() { # id arm run proj home work expect forbid injected_has_expect
  local id="$1" arm="$2" run="$3" proj="$4" home="$5" work="$6" expect="$7" forbid="$8" inj_has="$9"
  local log="$proj/../work-$arm-$run.jsonl" jtext="$proj/../judge-$arm-$run.txt" marker="$proj/../mark-$arm-$run"
  : > "$marker"
  run_session "$home" "$proj" "$work" "$log" --output-format stream-json --verbose
  local is_err turns names n_rb seq result
  is_err="$(jq -r 'select(.type=="result")|.is_error' "$log" 2>/dev/null | tail -1)"; : "${is_err:=true}"
  turns="$(jq -r 'select(.type=="result")|.num_turns' "$log" 2>/dev/null | tail -1)"; : "${turns:=0}"
  result="$(jq -r 'select(.type=="result")|.result // empty' "$log" 2>/dev/null | tail -1)"
  # Judge text = the model's final answer + files it WROTE this run (newer than
  # marker; the pre-seeded claude-md CLAUDE.md is older, so it is excluded).
  printf '%s\n' "$result" > "$jtext"
  find "$proj" -type f -not -path '*/.*' -newer "$marker" -size -256k -print0 2>/dev/null \
    | xargs -0 cat >> "$jtext" 2>/dev/null || true
  local success; success="$(judge "$jtext" "$expect" "$forbid")"
  names="$(jq -r 'select(.type=="assistant")|.message.content[]?|select(.type=="tool_use")|.name' "$log" 2>/dev/null)"
  if [ -n "$names" ]; then
    n_rb="$(printf '%s\n' "$names" | grep -c 'mcp__rusty-brain__')"
    seq="$(printf '%s\n' "$names" | paste -sd, -)"
  else
    n_rb=0; seq="-"
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$arm" "$run" "$is_err" "$success" "$turns" "$n_rb" "$inj_has" "$seq" >> "$RESULTS"
  printf '   [%-9s] is_error=%s success=%s turns=%s rusty_brain=%s inj_has_expect=%s seq=%s\n' \
    "$arm r$run" "$is_err" "$success" "$turns" "$n_rb" "$inj_has" "$seq"
}

trace_scenario() { # row
  local row="$1"
  local id plant supersede claude_md work expect forbid stale
  id="$(jq -r '.id' <<<"$row")"
  plant="$(jq -r '.plant' <<<"$row")"
  supersede="$(jq -r '.supersede // ""' <<<"$row")"
  claude_md="$(jq -r '.claude_md' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"
  expect="$(jq -r '.expect' <<<"$row")"
  forbid="$(jq -r '.forbid // ""' <<<"$row")"
  stale="$(jq -r '.stale_token // ""' <<<"$row")"
  local run=1
  while [ "$run" -le "$RUNS" ]; do
    echo "-- $id (run $run/$RUNS)  expect='$expect'"
    local base="$WORKROOT/$id-r$run"

    # memory-on: plant [-> supersede] -> capture injection -> work
    local mbase="$base/on" db="$base/on/memory.db" ns="rb-w35t-$id-r$run"
    local sockdir; sockdir="$(mktemp -d /tmp/rbw35t.XXXXXX)"; local sock="$sockdir/s"
    local ph="$mbase/hp" pp="$mbase/pp" wh="$mbase/hw" wp="$mbase/wp"
    seed_home "$ph"; mkdir -p "$pp"; seed_home "$wh"; mkdir -p "$wp"
    install_rusty_brain "$ph" "$pp"; install_rusty_brain "$wh" "$wp"
    rm -f "$wp/CLAUDE.md"; rm -rf "$wp/.claude/skills"
    (
      export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
      run_session "$ph" "$pp" "$plant" "$mbase/plant.log"
      [ -n "$supersede" ] && run_session "$ph" "$pp" "$supersede" "$mbase/sup.log"
      [ -f "$sock.pid" ] || { echo "ERROR: memory-on daemon did not auto-start for $id run $run" >&2; exit 1; }
      # The injected summary line for the work query (~what the UserPromptSubmit
      # hook surfaces): the top recall hit's summary, else content.
      local inj inj_has
      inj="$( ( export HOME="$wh"; export PATH="$BIN_DIR:$PATH"; \
                rusty-brain --json recall "$work" 2>/dev/null ) \
             | jq -r '(.[0].memory.summary // "") as $s | (if $s != "" then $s else (.[0].memory.content // "") end)' 2>/dev/null \
             | tr '\n' ' ' | head -c 240)"
      if printf '%s' "$inj" | grep -iqF -- "$expect"; then inj_has=1; else inj_has=0; fi
      echo "   injected[$id]: has_expect=$inj_has  \"$inj\""
      emit_row "$id" "memory-on" "$run" "$wp" "$wh" "$work" "$expect" "$forbid" "$inj_has"
    )
    [ -f "$sock.pid" ] && kill "$(cat "$sock.pid" 2>/dev/null)" 2>/dev/null || true
    rm -rf "$sockdir" 2>/dev/null || true

    # claude-md: decision in CLAUDE.md, no rusty-brain (injected_has_expect=n/a)
    local ch="$base/cmd/h" cpj="$base/cmd/p"
    seed_home "$ch"; mkdir -p "$cpj"
    printf '# Project conventions\n\n%s\n' "$claude_md" > "$cpj/CLAUDE.md"
    emit_row "$id" "claude-md" "$run" "$cpj" "$ch" "$work" "$expect" "$forbid" "na"

    run=$((run + 1))
  done
}

echo "== W3.5 trace (model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
echo "   scenarios: $SCENARIOS"
printf 'scenario\tarm\trun\tis_error\tsuccess\tnum_turns\tn_rusty_brain_calls\tinjected_has_expect\ttool_sequence\n'
for id in $SCENARIOS; do
  row="$(jq -c --arg id "$id" '.scenarios[]|select(.id==$id)' "$SCENARIOS_JSON")"
  [ -n "$row" ] || { echo "scenario not found: $id" >&2; continue; }
  trace_scenario "$row"
done

echo
echo "== per-arm aggregate (success rate, mean turns, mean rusty-brain calls) =="
awk -F'\t' '
  { arm=$2; n[arm]++; s[arm]+=$5; t[arm]+=$6; rb[arm]+=$7 }
  END {
    printf "%-10s %6s %10s %12s %16s\n", "arm", "runs", "success", "mean_turns", "mean_rb_calls";
    split("memory-on claude-md", o, " ");
    for (i=1;i<=2;i++){ a=o[i]; if(n[a]) printf "%-10s %6d %9.0f%% %12.2f %16.2f\n", a, n[a], 100*s[a]/n[a], t[a]/n[a], rb[a]/n[a]; }
  }' "$RESULTS"
echo
echo "report: $RESULTS"
