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
# n_tool_calls, n_rusty_brain_calls, tok_in, tok_cache_create, tok_cache_read,
# tok_out, tool_sequence) — TOOL NAMES ONLY, no model free-text, so it is safe
# to upload as a CI artifact (mirrors w35-ab-eval.sh).
#
# The four tok_* columns (summed from each assistant event's .message.usage)
# are the prompt-caching study instrumentation (ADR-3,
# docs/eval/2026-06-19-w35-cache-study.md): claude-md's CLAUDE.md caches after
# turn 1 (cache_read, cheap), while memory-on's per-turn UserPromptSubmit
# injection is a cache_creation each turn. A naive raw-input-token comparison
# makes memory look worse; cost is already cache-adjusted, so the buckets are
# surfaced DIAGNOSTICALLY (per-arm cache aggregate appended to the report).
#
# Usage: w35-trace-tools.sh --bin-dir DIR [--runs N] [--out FILE] [--scenarios "id id ..."]
#        w35-trace-tools.sh --self-test   # CI-safe: token math + aggregate, NO API
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS_JSON="$REPO_ROOT/crates/rb-eval/scorecard/w35_ab_scenarios.json"
MODEL="${RB_W35_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_W35_MAX_BUDGET_USD:-0.50}"

BIN_DIR=""; RUNS="2"; OUT=""; SCENARIOS="time-unix-millis config-settings-toml arch-single-writer"; MODE="run"
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) MODE="self-test"; shift ;;
    --bin-dir)   BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)      RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --out)       OUT="${2:?--out needs a value}"; shift 2 ;;
    --scenarios) SCENARIOS="${2:?--scenarios needs a value}"; shift 2 ;;
    -h|--help)   sed -n '2,28p' "$0"; exit 2 ;;
    *)           echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

case "$RUNS" in ''|*[!0-9]*|0) echo "--runs must be a positive integer" >&2; exit 2 ;; esac
# Whitelist the scenario list charset: ids are [a-z0-9-], space-separated. Reject
# anything else so a dispatch value can never inject shell.
case "$SCENARIOS" in *[!a-z0-9\ _-]*) echo "--scenarios may contain only [a-z0-9 _-]" >&2; exit 2 ;; esac

command -v jq >/dev/null 2>&1 || { echo "jq not on PATH" >&2; exit 1; }

# Run-mode-only prerequisites (--self-test needs only jq, above).
if [ "$MODE" != "self-test" ]; then
  [ -n "$BIN_DIR" ] || { echo "need --bin-dir" >&2; exit 2; }
  BIN_DIR="$(cd "$BIN_DIR" && pwd)"
  for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
    [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin" >&2; exit 1; }
  done
  command -v claude  >/dev/null 2>&1 || { echo "claude not on PATH" >&2; exit 1; }
  command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH" >&2; exit 1; }
  [ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "ANTHROPIC_API_KEY is not set" >&2; exit 1; }
  [ -f "$SCENARIOS_JSON" ] || { echo "scenarios file not found: $SCENARIOS_JSON" >&2; exit 1; }
fi

SESSION_TIMEOUT=""
if [ "$MODE" != "self-test" ]; then
  if command -v timeout >/dev/null 2>&1; then SESSION_TIMEOUT="timeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"
  elif command -v gtimeout >/dev/null 2>&1; then SESSION_TIMEOUT="gtimeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"; fi

  unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE RUSTY_BRAIN_IDLE_TIMEOUT_SECS

  WORKROOT="$(mktemp -d "${TMPDIR:-/tmp}/rb-w35-trace.XXXXXX")"
  RESULTS="${OUT:-$WORKROOT/trace.tsv}"; : > "$RESULTS"
  cleanup() { rm -rf "$WORKROOT" 2>/dev/null || true; }
  trap cleanup EXIT
fi

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

# Sum per-session token usage from a stream-json --verbose log. Echoes one
# tab-separated line: tok_in<TAB>tok_cache_create<TAB>tok_cache_read<TAB>tok_out.
# Pure over the log (no daemon, no API); each assistant event's .message.usage is
# summed; missing fields read as 0 (defensive). The .message.usage path matches the
# Anthropic API usage shape — live verification is on the deferred measured run, and
# --self-test (run by CI before every dispatch) guards it: a broken path yields
# all-zeros, which the fixture's 2250/5000/5000/90 would fail on. All-zeros is ALSO
# legitimate for an errored session (no assistant events); at runtime such a row is
# already flagged by is_error=true/num_turns=0 (see emit_row), so it is not confused
# with a path bug (which would show is_error=false, num_turns>0, tok=0).
sum_usage() { # log
  jq -r 'select(.type=="assistant") | .message.usage // empty |
         [ (.input_tokens // 0),
           (.cache_creation_input_tokens // 0),
           (.cache_read_input_tokens // 0),
           (.output_tokens // 0) ] | @tsv' "$1" 2>/dev/null \
    | awk -F'\t' '{ i+=$1; cc+=$2; cr+=$3; o+=$4 }
                   END { printf "%d\t%d\t%d\t%d\n", i+0, cc+0, cr+0, o+0 }'
}

# Cache-token aggregate (pure awk over the results TSV). Per arm reports the mean
# of each token bucket plus two derived figures: mean_ctx_vol (in+cc+cr — what the
# model consumed as input context) and mean_eff_in (in+1.25*cc+0.1*cr — cache-weighted
# full-price input equivalents, a sanity check against total_cost_usd; 1.25 = Anthropic's
# 5-min cache-write premium, 0.1 = cache-read fraction). total_cost_usd is ALREADY
# cache-adjusted by the API, so it stays the honest cost axis; this table explains WHY
# costs differ (see ADR-3, docs/eval/2026-06-19-w35-cache-study.md).
aggregate_cache() { # results_tsv
  awk -F'\t' '
    { arm=$2; n[arm]++;
      tin[arm]+=$9; tcc[arm]+=$10; tcr[arm]+=$11; tout[arm]+=$12 }
    END {
      printf "%-10s %5s %10s %10s %10s %10s %12s %12s\n",
        "arm","runs","mean_in","mean_cc","mean_cr","mean_out","mean_ctx_vol","mean_eff_in";
      split("memory-on claude-md", order, " ");
      for (i=1;i<=2;i++){ a=order[i]; if (n[a]) {
        mi=tin[a]/n[a]; mcc=tcc[a]/n[a]; mcr=tcr[a]/n[a]; mo=tout[a]/n[a];
        printf "%-10s %5d %10.0f %10.0f %10.0f %10.0f %12.0f %12.0f\n",
          a, n[a], mi, mcc, mcr, mo, (mi+mcc+mcr), (mi+1.25*mcc+0.1*mcr);
      }}
    }' "$1"
}

# Parse a stream-json --verbose work log -> append one TSV row + echo a summary.
emit_row() { # id arm run log
  local id="$1" arm="$2" run="$3" log="$4"
  local is_err turns cost names n_tool n_rb seq
  local tin tcc tcr tou
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
  IFS=$'\t' read -r tin tcc tcr tou < <(sum_usage "$log")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$arm" "$run" "$is_err" "$turns" "$cost" "$n_tool" "$n_rb" \
    "$tin" "$tcc" "$tcr" "$tou" "$seq" >> "$RESULTS"
  printf '   [%-9s] is_error=%s turns=%s cost=%s tool_calls=%s rusty_brain=%s tok(in/cc/cr/out)=%s/%s/%s/%s seq=%s\n' \
    "$arm r$run" "$is_err" "$turns" "$cost" "$n_tool" "$n_rb" "$tin" "$tcc" "$tcr" "$tou" "$seq"
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

self_test() {
  echo "== W3.5 cache-trace self-test (token math + aggregate; no API) =="
  local tmp fail=0 log row sums
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/rb-w35-cache.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN
  log="$tmp/fixture.jsonl"
  # Three assistant events carry usage: msg_a writes cache, msg_b reads it, msg_c
  # OMITS the cache fields to exercise the // 0 fallback. The result event's
  # num_turns is a SEMANTIC CLI count and is deliberately != the assistant-event
  # count (3) — emit_row reads it straight from .result.num_turns, so the
  # turns/cost wiring (cols 4-6 below) is checked independently of how many usage
  # events precede it (as in real stream-json, where num_turns != assistant count).
  cat >"$log" <<'JSON'
{"type":"system","subtype":"init","session_id":"s1"}
{"type":"assistant","message":{"id":"msg_a","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1000,"cache_creation_input_tokens":5000,"cache_read_input_tokens":0,"output_tokens":50}}}
{"type":"assistant","message":{"id":"msg_b","type":"message","role":"assistant","content":[{"type":"text","text":"yo"}],"usage":{"input_tokens":1200,"cache_creation_input_tokens":0,"cache_read_input_tokens":5000,"output_tokens":30}}}
{"type":"assistant","message":{"id":"msg_c","type":"message","role":"assistant","content":[{"type":"text","text":"hm"}],"usage":{"input_tokens":50,"output_tokens":10}}}
{"type":"result","subtype":"success","is_error":false,"num_turns":2,"total_cost_usd":0.0123,"session_id":"s1","result":"done"}
JSON

  check() { if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "BUG: $1 (want '$2' got '$3')" >&2; fail=1; fi; }

  # sum_usage: in=1000+1200+50=2250; cc=5000; cr=5000; out=50+30+10=90.
  sums="$(sum_usage "$log")"
  check "sum_usage -> in/cc/cr/out" "$(printf '2250\t5000\t5000\t90')" "$sums"

  # emit_row wires the parsed buckets into columns 9-12 and cost/turns into 4-6.
  RESULTS="$tmp/emit.tsv"; : > "$RESULTS"
  emit_row "sid" "memory-on" "1" "$log"
  row="$(tail -1 "$RESULTS")"
  check "emit_row token columns (cols 9-12)" "$(printf '2250\t5000\t5000\t90')" \
        "$(printf '%s\n' "$row" | cut -f9-12)"
  check "emit_row is_error/turns/cost (cols 4-6)" "$(printf 'false\t2\t0.0123')" \
        "$(printf '%s\n' "$row" | cut -f4-6)"

  # aggregate_cache arithmetic (1 run/arm => means == the single values).
  # eff_in = in + 1.25*cc + 0.1*cr  (1.25 = cache-write premium, 0.1 = cache-read frac):
  #   memory-on eff_in = 2000+1.25*6000+0.1*500 = 9550; ctx_vol = 2000+6000+500 = 8500
  #   claude-md  eff_in = 1500+1.25*1000+0.1*8000 = 3550
  local agg_tsv out meff_on meff_cmd mcr_cmd mctx_on
  agg_tsv="$tmp/agg.tsv"
  printf 'scenario\tarm\trun\tis_error\tnum_turns\tcost\tn_tool_calls\tn_rusty_brain_calls\ttok_in\ttok_cache_create\ttok_cache_read\ttok_out\ttool_sequence\n' >"$agg_tsv"
  printf 's\tmemory-on\t1\tfalse\t2\t0.01\t0\t0\t2000\t6000\t500\t100\t-\n' >>"$agg_tsv"
  printf 's\tclaude-md\t1\tfalse\t2\t0.01\t0\t0\t1500\t1000\t8000\t80\t-\n' >>"$agg_tsv"
  out="$(aggregate_cache "$agg_tsv")"
  meff_on="$(printf '%s\n' "$out" | awk '$1=="memory-on"{print $8}')"
  meff_cmd="$(printf '%s\n' "$out" | awk '$1=="claude-md"{print $8}')"
  mcr_cmd="$(printf '%s\n' "$out" | awk '$1=="claude-md"{print $5}')"
  mctx_on="$(printf '%s\n' "$out" | awk '$1=="memory-on"{print $7}')"
  check "aggregate eff_in memory-on (2000+1.25*6000+0.1*500=9550)" "9550" "$meff_on"
  check "aggregate eff_in claude-md (1500+1.25*1000+0.1*8000=3550)" "3550" "$meff_cmd"
  check "aggregate mean_cache_read claude-md (8000)" "8000" "$mcr_cmd"
  check "aggregate mean_ctx_vol memory-on (2000+6000+500=8500)" "8500" "$mctx_on"

  if [ "$fail" -eq 0 ]; then echo "self-test PASS"; return 0; fi
  echo "self-test FAIL" >&2; return 1
}

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi

echo "== W3.5 tool-trace (model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
echo "   scenarios: $SCENARIOS"
printf 'scenario\tarm\trun\tis_error\tnum_turns\tcost\tn_tool_calls\tn_rusty_brain_calls\ttok_in\ttok_cache_create\ttok_cache_read\ttok_out\ttool_sequence\n'
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
echo "== cache-token aggregate (cached vs uncached input; see ADR-3) =="
aggregate_cache "$RESULTS"
echo
echo "report: $RESULTS"
