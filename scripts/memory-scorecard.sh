#!/usr/bin/env bash
# Memory-value scorecard harness (W3.5 criterion redesign; see
# docs/eval/2026-06-16-w35-criterion-redesign.md). The successor to
# scripts/w35-ab-eval.sh's single-fact gate. Instead of "memory-on beats one
# CLAUDE.md baseline on one fact", it scores memory on the axes where it
# STRUCTURALLY differs from a hand-maintained file, against TWO baselines so a
# win cannot come from re-rigging the eval toward memory:
#
#   arms per scenario:
#     memory-on          rusty-brain has the fact (planted per the scenario's mode)
#     realistic-baseline the CLAUDE.md a real team would actually have (stale/partial/large)
#     steelman-baseline  a diligent human's CLAUDE.md (clean/current/complete)
#     memory-off         nothing (the floor)
#
#   a dimension's claim (P1): memory-on BEATS realistic AND at least TIES steelman.
#   the ONLY hard gate (P4): zero memory-induced errors (the safety property).
#
# Plant modes (P2): `explicit` (rusty-brain remember — isolates retrieval) vs
# `auto-capture` (a real SessionEnd fold — exercises capture). Retrieval/freshness
# dimensions plant explicitly; capture is its own dimension.
#
# This file ships the PURE scoring core (judge + scorecard aggregation) with a
# `--self-test` that needs NO API, plus the live four-arm runner. Per P3 a single
# run never gates; pass --runs >= 5 for a real read.
#
# Usage:
#   memory-scorecard.sh --self-test                       # judge + aggregation math, no API
#   memory-scorecard.sh --bin-dir DIR [--runs N] [--out FILE] [--scenarios-file F]
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS_FILE="$REPO_ROOT/crates/rb-eval/scorecard/memory_scorecard_scenarios.json"
MODEL="${RB_SCORECARD_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_SCORECARD_MAX_BUDGET_USD:-0.50}"
# Steelman tie margin (P1): memory-on must come within this of the diligent-human
# baseline to count as a tie. A small margin absorbs haiku noise without letting a
# real regression pass.
TIE_MARGIN="${RB_SCORECARD_TIE_MARGIN:-0.10}"

# ---- pure judge (P: gate's substring judge; exercised by --self-test) --------
# judge_text <textfile> <expect> <forbid> <stale> <arm> -> "<success> <mie>"
#   success = expect present AND forbid absent (case-insensitive substring)
#   mie     = MEMORY-INDUCED ERROR: memory-on arm only, only when the task FAILED
#             (success==0) AND the stale token is present (acted on a stale memory).
judge_text() {
  local file="$1" expect="$2" forbid="$3" stale="$4" arm="$5"
  local success=0 mie=0
  grep -iqF -- "$expect" "$file" 2>/dev/null && success=1
  [ -n "$forbid" ] && grep -iqF -- "$forbid" "$file" 2>/dev/null && success=0
  if [ "$arm" = "memory-on" ] && [ "$success" -eq 0 ] && [ -n "$stale" ] \
     && grep -iqF -- "$stale" "$file" 2>/dev/null; then
    mie=1
  fi
  echo "$success $mie"
}

# ---- pure scorecard aggregation (exercised by --self-test) -------------------
# Reads a results TSV (dimension<TAB>scenario<TAB>arm<TAB>run<TAB>success<TAB>turns<TAB>mie)
# from $1; prints the per-dimension scorecard + the safety gate; returns 0 iff the
# SAFETY gate holds (zero memory-induced errors) — per P4 only safety is hard.
# Dimension verdicts (beats realistic AND ties steelman) are reported, not gated.
aggregate_scorecard() {
  local tsv="$1" tie_margin="${2:-$TIE_MARGIN}"
  awk -F'\t' -v tie="$tie_margin" '
    function median(arr, n,   i, tmp, c) {
      c = 0; for (i in arr) { tmp[++c] = arr[i] }
      if (c == 0) return 0
      # insertion sort (small n)
      for (i = 2; i <= c; i++) { v = tmp[i]; j = i - 1;
        while (j >= 1 && tmp[j] > v) { tmp[j+1] = tmp[j]; j-- } tmp[j+1] = v }
      if (c % 2) return tmp[(c+1)/2]
      return (tmp[c/2] + tmp[c/2+1]) / 2.0
    }
    {
      dim=$1; arm=$3; succ=$5; turns=$6; mie=$7;
      key = dim SUBSEP arm;
      n[key]++; s[key]+=succ;
      tk = key SUBSEP n[key]; tv[tk] = turns;     # per-run turns for median
      dims[dim]=1;
      if (arm=="memory-on" && mie==1) { mie_total++; mie_list = mie_list sprintf("    - %s / %s run %s\n", dim, $2, $4) }
    }
    END {
      printf "%-16s %-18s %5s %9s %11s\n", "dimension", "arm", "runs", "success", "med_turns";
      pass_dims=0; total_dims=0;
      for (d in dims) {
        total_dims++;
        split("memory-on realistic-baseline steelman-baseline memory-off", order, " ");
        for (i=1;i<=4;i++) {
          a=order[i]; key=d SUBSEP a;
          if (n[key]==0) { printf "%-16s %-18s %5s %9s %11s\n", d, a, 0, "n/a", "n/a"; continue }
          rate[key]=s[key]/n[key];
          # gather this arm-dim turns into a contiguous array for median()
          delete tarr; m=0;
          for (r=1;r<=n[key];r++){ m++; tarr[m]=tv[key SUBSEP r] }
          printf "%-16s %-18s %5d %8.0f%% %11.1f\n", d, a, n[key], rate[key]*100, median(tarr, m);
        }
        on=rate[d SUBSEP "memory-on"]; rb=rate[d SUBSEP "realistic-baseline"]; sm=rate[d SUBSEP "steelman-baseline"];
        beats_realistic = (on > rb);
        ties_steelman   = (on >= sm - tie);
        verdict = (beats_realistic && ties_steelman) ? "PASS" : "no";
        if (beats_realistic && ties_steelman) pass_dims++;
        printf "  -> %s: beats_realistic=%s ties_steelman=%s  => %s\n\n",
               d, (beats_realistic?"yes":"NO"), (ties_steelman?"yes":"NO"), verdict;
      }
      printf "scorecard: %d/%d dimensions pass (tracked, non-gating)\n", pass_dims, total_dims;
      printf "SAFETY GATE — memory-induced errors: %d (allowed 0)\n", mie_total+0;
      if (mie_total+0 > 0) { printf "  enumerated:\n%s", mie_list }
      printf "result: %s\n", (mie_total+0 == 0 ? "SAFE" : "UNSAFE");
      exit (mie_total+0 == 0 ? 0 : 1);
    }
  ' "$tsv"
}

# ---- self-test (no API) ------------------------------------------------------
self_test() {
  echo "== memory-scorecard self-test (judge + scorecard math; no API) =="
  local tmp fail=0
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/rb-scorecard-selftest.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  printf 'use the ureq crate\n'      > "$tmp/new.txt"
  printf 'use reqwest\n'             > "$tmp/old.txt"
  printf 'use ureq, never reqwest\n' > "$tmp/names-old.txt"

  check() { if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "BUG: $1 (want '$2' got '$3')"; fail=1; fi; }
  check "current answer => success, no mie"                 "1 0" "$(judge_text "$tmp/new.txt"       ureq '' reqwest memory-on)"
  check "names superseded value but correct => success"     "1 0" "$(judge_text "$tmp/names-old.txt" ureq '' reqwest memory-on)"
  check "acted on stale (memory-on) => fail + mie"          "0 1" "$(judge_text "$tmp/old.txt"       ureq '' reqwest memory-on)"
  check "acted on stale (baseline) => fail, no mie"         "0 0" "$(judge_text "$tmp/old.txt"       ureq '' reqwest steelman-baseline)"

  # A scorecard where memory beats realistic AND ties steelman, zero mie => SAFE, dim passes.
  local good="$tmp/good.tsv"
  {
    printf 'freshness\ts1\tmemory-on\t1\t1\t2\t0\n'
    printf 'freshness\ts1\trealistic-baseline\t1\t0\t2\t0\n'
    printf 'freshness\ts1\tsteelman-baseline\t1\t1\t2\t0\n'
    printf 'freshness\ts1\tmemory-off\t1\t0\t2\t0\n'
  } > "$good"
  if aggregate_scorecard "$good" >/dev/null; then echo "ok: clean scorecard is SAFE"; else echo "BUG: clean scorecard flagged unsafe"; fail=1; fi
  if aggregate_scorecard "$good" | grep -q '\-> freshness:.*=> PASS'; then echo "ok: dimension passes when it beats realistic + ties steelman"; else echo "BUG: dimension did not pass"; fail=1; fi

  # RE-RIG GUARD: memory beats realistic but LOSES to steelman => dimension must NOT pass.
  local rig="$tmp/rig.tsv"
  {
    printf 'freshness\ts1\tmemory-on\t1\t0\t2\t0\n'
    printf 'freshness\ts1\trealistic-baseline\t1\t0\t2\t0\n'
    printf 'freshness\ts1\tsteelman-baseline\t1\t1\t2\t0\n'
    printf 'freshness\ts1\tmemory-off\t1\t0\t2\t0\n'
  } > "$rig"
  if aggregate_scorecard "$rig" | grep -q '\-> freshness:.*=> no'; then echo "ok: re-rig guard — losing to steelman fails the dimension"; else echo "BUG: re-rig not caught"; fail=1; fi

  # SAFETY: a memory-induced error fails the hard gate even if dimensions look fine.
  local unsafe="$tmp/unsafe.tsv"
  {
    printf 'freshness\ts1\tmemory-on\t1\t0\t2\t1\n'
    printf 'freshness\ts1\trealistic-baseline\t1\t0\t2\t0\n'
    printf 'freshness\ts1\tsteelman-baseline\t1\t1\t2\t0\n'
    printf 'freshness\ts1\tmemory-off\t1\t0\t2\t0\n'
  } > "$unsafe"
  if aggregate_scorecard "$unsafe" >/dev/null; then echo "BUG: memory-induced error did not fail the safety gate"; fail=1; else echo "ok: memory-induced error fails the safety gate"; fi

  # median: even count averages the two middles.
  local med="$tmp/med.tsv"
  {
    printf 'd\ts\tmemory-on\t1\t1\t1\t0\n'
    printf 'd\ts\tmemory-on\t2\t1\t3\t0\n'
    printf 'd\ts\tmemory-on\t3\t1\t5\t0\n'
    printf 'd\ts\tmemory-on\t4\t1\t9\t0\n'
  } > "$med"
  if aggregate_scorecard "$med" | grep -qE 'memory-on .* 4 .* 4\.0'; then echo "ok: median turns of {1,3,5,9} = 4.0"; else echo "BUG: median wrong"; aggregate_scorecard "$med" | grep memory-on; fail=1; fi

  [ "$fail" -eq 0 ] && { echo "self-test PASS"; return 0; } || { echo "self-test FAIL" >&2; return 1; }
}

# ---- arg parsing -------------------------------------------------------------
MODE="run"; BIN_DIR=""; RUNS=""; OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)      MODE="self-test"; shift ;;
    --bin-dir)        BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)           RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --out)            OUT="${2:?--out needs a value}"; shift 2 ;;
    --scenarios-file) SCENARIOS_FILE="${2:?--scenarios-file needs a value}"; shift 2 ;;
    -h|--help)        sed -n '2,30p' "$0"; exit 2 ;;
    *)                echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi

# ---- live run prerequisites (the four-arm runner; needs API) -----------------
[ -n "$BIN_DIR" ] || { echo "need --bin-dir (or --self-test)" >&2; exit 2; }
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin" >&2; exit 1; }
done
command -v claude  >/dev/null 2>&1 || { echo "claude not on PATH" >&2; exit 1; }
command -v jq      >/dev/null 2>&1 || { echo "jq not on PATH" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH" >&2; exit 1; }
[ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "ANTHROPIC_API_KEY is not set" >&2; exit 1; }
[ -f "$SCENARIOS_FILE" ] || { echo "scenarios file not found: $SCENARIOS_FILE" >&2; exit 1; }
[ -n "$RUNS" ] || RUNS="$(jq -r '.config.runs_per_scenario // 5' "$SCENARIOS_FILE")"

SESSION_TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then SESSION_TIMEOUT="timeout ${RB_SCORECARD_SESSION_TIMEOUT_SECS:-300}"
elif command -v gtimeout >/dev/null 2>&1; then SESSION_TIMEOUT="gtimeout ${RB_SCORECARD_SESSION_TIMEOUT_SECS:-300}"; fi

unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE RUSTY_BRAIN_IDLE_TIMEOUT_SECS
WORKROOT="$(mktemp -d "${TMPDIR:-/tmp}/rb-scorecard.XXXXXX")"
RESULTS="${OUT:-$WORKROOT/scorecard.tsv}"; : > "$RESULTS"
cleanup() { rm -rf "$WORKROOT" 2>/dev/null || true; }
trap cleanup EXIT

seed_home() { mkdir -p "$1"; printf '{"hasCompletedOnboarding": true}\n' > "$1/.claude.json"; }

run_session() { # home proj prompt log [extra args...]
  local home="$1" proj="$2" prompt="$3" log="$4"; shift 4
  (
    export HOME="$home"
    unset XDG_RUNTIME_DIR XDG_CACHE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_STATE_HOME
    export PATH="$BIN_DIR:$PATH"; cd "$proj"
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

# Score one work session (stream-json) and append a scorecard row.
score_session() { # dim id arm run proj home work expect forbid stale
  local dim="$1" id="$2" arm="$3" run="$4" proj="$5" home="$6" work="$7" expect="$8" forbid="$9" stale="${10}"
  local jlog="$proj/../work.json" jtext="$proj/../judge.txt" marker="$proj/../mark"
  : > "$marker"
  run_session "$home" "$proj" "$work" "$jlog" --output-format json
  local turns
  if jq -e '.result' "$jlog" >/dev/null 2>&1; then
    jq -r '.result' "$jlog" > "$jtext"
    turns="$(jq -r '.num_turns // 99' "$jlog" 2>/dev/null || echo 99)"
  else
    cp "$jlog" "$jtext"; turns=99
  fi
  find "$proj" -type f -not -path '*/.*' -newer "$marker" -size -256k -print0 2>/dev/null \
    | xargs -0 cat >> "$jtext" 2>/dev/null || true
  local res success mie
  res="$(judge_text "$jtext" "$expect" "$forbid" "$stale" "$arm")"
  success="${res% *}"; mie="${res#* }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$dim" "$id" "$arm" "$run" "$success" "$turns" "$mie" >> "$RESULTS"
  echo "   [$dim/$id $arm r$run] success=$success turns=$turns mie=$mie"
}

# Explicit plant (P2): each fact is stored via `rusty-brain remember`, in array
# order, isolating retrieval from the lossy auto-capture path. NOTE: storing a
# SUPERSEDED state (Class C: X later replaced by X') is NOT yet expressible here —
# the CLI has no `remember --supersedes` (supersede lives only in the engine's
# remember_superseding, exercised by the hook). Class C must add that CLI flag, or
# construct the superseded state via `remember --supersedes`.
#
# Each fact: { content, supersedes_prev? }. Facts store in array order; a fact with
# `supersedes_prev: true` is stored as a SUPERSEDING update of the immediately
# prior planted fact (Class C: X then X' replacing X), archiving the predecessor so
# recall returns only the current value — explicit, no lossy auto-capture (P2).
plant_explicit() { # home (env: RUSTY_BRAIN_*) <facts-json-array>
  local home="$1" facts="$2"
  ( export HOME="$home"; export PATH="$BIN_DIR:$PATH"
    local n i content sup out last_id=""
    n="$(jq 'length' <<<"$facts")"
    for (( i=0; i<n; i++ )); do
      content="$(jq -r ".[$i].content" <<<"$facts")"
      sup="$(jq -r ".[$i].supersedes_prev // false" <<<"$facts")"
      if [ "$sup" = "true" ] && [ -n "$last_id" ]; then
        out="$(rusty-brain --json remember "$content" --type insight --supersedes "$last_id" 2>/dev/null)"
      else
        out="$(rusty-brain --json remember "$content" --type insight 2>/dev/null)"
      fi
      last_id="$(jq -r '.id // empty' <<<"$out" 2>/dev/null)"
    done
  )
}

run_scenario() { # row
  local row="$1"
  local id dim plant_mode work expect forbid stale realistic steelman facts
  id="$(jq -r '.id' <<<"$row")"; dim="$(jq -r '.dimension' <<<"$row")"
  plant_mode="$(jq -r '.plant_mode' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"; expect="$(jq -r '.expect' <<<"$row")"
  forbid="$(jq -r '.forbid // ""' <<<"$row")"; stale="$(jq -r '.stale_token // ""' <<<"$row")"
  realistic="$(jq -r '.realistic_claude_md // ""' <<<"$row")"
  steelman="$(jq -r '.steelman_claude_md // ""' <<<"$row")"
  facts="$(jq -c '.plant // []' <<<"$row")"
  local run=1
  while [ "$run" -le "$RUNS" ]; do
    local base="$WORKROOT/$id-r$run"
    echo "-- $dim/$id (run $run/$RUNS) [$plant_mode]"

    # memory-on
    local mb="$base/on" db="$mb/memory.db" ns="rb-sc-$id-r$run"
    local sockdir; sockdir="$(mktemp -d /tmp/rbsc.XXXXXX)"; local sock="$sockdir/s"
    local wh="$mb/hw" wp="$mb/wp"; seed_home "$wh"; mkdir -p "$wp"
    install_rusty_brain "$wh" "$wp"; rm -f "$wp/CLAUDE.md"; rm -rf "$wp/.claude/skills"
    (
      export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
      # Start a daemon for the store and WAIT for the socket to bind before
      # planting (a fixed sleep races the bind — observed flaky).
      rusty-brain serve >/dev/null 2>&1 &
      local dpid=$!
      for _ in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.2; done
      if [ "$plant_mode" = "explicit" ]; then plant_explicit "$wh" "$facts"; fi
      # (auto-capture mode: a plant session would run here — wired with dimension B.)
      score_session "$dim" "$id" "memory-on" "$run" "$wp" "$wh" "$work" "$expect" "$forbid" "$stale"
      kill "$dpid" 2>/dev/null || true
    )
    rm -rf "$sockdir" 2>/dev/null || true

    # realistic-baseline + steelman-baseline + memory-off
    local rb="$base/realistic"; seed_home "$rb/h"; mkdir -p "$rb/p"
    [ -n "$realistic" ] && printf '# Project conventions\n\n%s\n' "$realistic" > "$rb/p/CLAUDE.md"
    score_session "$dim" "$id" "realistic-baseline" "$run" "$rb/p" "$rb/h" "$work" "$expect" "$forbid" "$stale"

    local sb="$base/steelman"; seed_home "$sb/h"; mkdir -p "$sb/p"
    [ -n "$steelman" ] && printf '# Project conventions\n\n%s\n' "$steelman" > "$sb/p/CLAUDE.md"
    score_session "$dim" "$id" "steelman-baseline" "$run" "$sb/p" "$sb/h" "$work" "$expect" "$forbid" "$stale"

    local ob="$base/off"; seed_home "$ob/h"; mkdir -p "$ob/p"
    score_session "$dim" "$id" "memory-off" "$run" "$ob/p" "$ob/h" "$work" "$expect" "$forbid" "$stale"

    run=$((run + 1))
  done
}

echo "== memory-value scorecard (model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
while IFS= read -r row; do run_scenario "$row"; done < <(jq -c '.scenarios[]' "$SCENARIOS_FILE")
echo
echo "== scorecard =="
aggregate_scorecard "$RESULTS"
