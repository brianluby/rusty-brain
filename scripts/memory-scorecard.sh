#!/usr/bin/env bash
# shellcheck disable=SC2030,SC2031
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
# `--self-test` that needs NO API, plus the live four-arm runner.
#
# Variance protocol (P3): success is reported as a Wilson 95% CI and turns as
# median [Q1-Q3]; a single run never gates. A run with any arm below --min-runs
# (default 5, or config.min_runs) is DIRECTIONAL ONLY — it still prints the
# scorecard but emits no SAFE/UNSAFE verdict and exits 0. The only hard gate
# (P4) is zero memory-induced errors, and only from a >=min-runs run.
#
# Usage:
#   memory-scorecard.sh --self-test                       # judge + aggregation math, no API
#   memory-scorecard.sh --bin-dir DIR [--runs N] [--min-runs N] [--out FILE] [--scenarios-file F]
set -euo pipefail

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
# from $1. Prints the per-dimension scorecard — success as a Wilson 95% CI and
# turns as median [Q1-Q3] (P3: median + spread, never a bare mean) — plus the
# safety result. Exit code: 0 for a DIRECTIONAL run (any arm below min_runs, or a
# missing arm — prints no SAFE/UNSAFE verdict, never gates; P3: single runs never
# gate); otherwise 0 iff the SAFETY gate holds (zero memory-induced errors, P4),
# 1 on UNSAFE. The hard gate fires only from a >=min-runs, complete run.
# Dimension verdicts (beats realistic AND ties steelman) are reported, not gated.
#
#   $1 = tsv, $2 = steelman tie margin (default $TIE_MARGIN), $3 = min_runs (default 5).
aggregate_scorecard() {
  local tsv="$1" tie_margin="${2:-${TIE_MARGIN:-0.10}}" min_runs="${3:-5}"
  awk -F'\t' -v tie="$tie_margin" -v min_runs="$min_runs" '
    # insertion sort arr[1..n] in place
    function sortarr(arr, n,   i, j, v) {
      for (i = 2; i <= n; i++) { v = arr[i]; j = i - 1;
        while (j >= 1 && arr[j] > v) { arr[j+1] = arr[j]; j-- } arr[j+1] = v }
    }
    # linear-interpolated percentile p in [0,1] of arr[1..n] (n=count). Copies
    # into `sorted` so the caller array is not mutated.
    function pctile(arr, n, p,   sorted, i, rank, lo_i, hi_i, frac) {
      if (n == 0) return 0
      for (i = 1; i <= n; i++) sorted[i] = arr[i]
      sortarr(sorted, n)
      rank = (n - 1) * p + 1
      lo_i = int(rank); if (lo_i < 1) lo_i = 1
      hi_i = (lo_i < n) ? lo_i + 1 : lo_i
      frac = rank - lo_i
      return sorted[lo_i] + frac * (sorted[hi_i] - sorted[lo_i])
    }
    # Wilson 95% CI for k successes of n; sets globals wil_lo / wil_hi.
    function wilson(k, n,   z, z2, phat, denom, center, half) {
      z = 1.96; z2 = z * z
      if (n == 0) { wil_lo = 0; wil_hi = 0; return }
      phat = k / n
      denom = 1 + z2 / n
      center = (phat + z2 / (2 * n)) / denom
      half = z * sqrt(phat * (1 - phat) / n + z2 / (4 * n * n)) / denom
      wil_lo = center - half; wil_hi = center + half
    }
    {
      dim=$1; arm=$3; succ=$5; turns=$6; mie=$7;
      key = dim SUBSEP arm;
      n[key]++; s[key]+=succ;
      tk = key SUBSEP n[key]; tv[tk] = turns;
      dims[dim]=1;
      if (arm=="memory-on" && mie==1) { mie_total++; mie_list = mie_list sprintf("    - %s / %s run %s\n", dim, $2, $4) }
    }
    END {
      directional = 0;
      split("memory-on realistic-baseline steelman-baseline memory-off", order, " ");
      printf "%-15s %-18s %5s %16s %16s\n", "dimension", "arm", "runs", "success [95% CI]", "med_turns [Q1-Q3]";
      pass_dims=0; total_dims=0;
      for (d in dims) {
        total_dims++;
        for (i=1;i<=4;i++) {
          a=order[i]; key=d SUBSEP a;
          if (n[key]==0) {
            directional = 1;   # missing arm = incomplete data → never gate (P3)
            printf "%-15s %-18s %5d %16s %16s\n", d, a, 0, "n/a", "n/a"; continue
          }
          rate[key]=s[key]/n[key];
          if (n[key] < min_runs) directional = 1;
          delete tarr; m=0;
          for (r=1;r<=n[key];r++){ m++; tarr[m]=tv[key SUBSEP r] }
          med = pctile(tarr, m, 0.5); q1 = pctile(tarr, m, 0.25); q3 = pctile(tarr, m, 0.75);
          wilson(s[key], n[key]);
          printf "%-15s %-18s %5d %5.0f%% [%.1f-%.1f] %8.1f [%.1f-%.1f]\n",
                 d, a, n[key], rate[key]*100, wil_lo*100, wil_hi*100, med, q1, q3;
        }
        # A verdict needs all three comparison arms present; a missing baseline
        # would default its rate to 0 and could falsify beats_realistic.
        if (n[d SUBSEP "memory-on"]>0 && n[d SUBSEP "realistic-baseline"]>0 && n[d SUBSEP "steelman-baseline"]>0) {
          on=rate[d SUBSEP "memory-on"]; rb=rate[d SUBSEP "realistic-baseline"]; sm=rate[d SUBSEP "steelman-baseline"];
          beats_realistic = (on > rb);
          ties_steelman   = (on >= sm - tie);
          verdict = (beats_realistic && ties_steelman) ? "PASS" : "no";
          if (beats_realistic && ties_steelman) pass_dims++;
          printf "  -> %s: beats_realistic=%s ties_steelman=%s  => %s\n\n",
                 d, (beats_realistic?"yes":"NO"), (ties_steelman?"yes":"NO"), verdict;
        } else {
          printf "  -> %s: incomplete arms (no verdict)\n\n", d;
        }
      }
      printf "scorecard: %d/%d dimensions pass (tracked, non-gating)\n", pass_dims, total_dims;
      printf "SAFETY — memory-induced errors: %d (allowed 0)\n", mie_total+0;
      if (mie_total+0 > 0) { printf "  enumerated:\n%s", mie_list }
      if (directional) {
        printf "DIRECTIONAL ONLY — an arm is below min_runs=%d; single runs never gate (P3)\n", min_runs;
        printf "result: DIRECTIONAL (not gating)\n";
        exit 0;
      }
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

  # Scorecard fixtures below have N=1 per arm; pass min_runs=1 (3rd arg) so they
  # still gate/verdict. The directional behavior is exercised separately.

  # A scorecard where memory beats realistic AND ties steelman, zero mie => SAFE, dim passes.
  local good="$tmp/good.tsv"
  {
    printf 'freshness\ts1\tmemory-on\t1\t1\t2\t0\n'
    printf 'freshness\ts1\trealistic-baseline\t1\t0\t2\t0\n'
    printf 'freshness\ts1\tsteelman-baseline\t1\t1\t2\t0\n'
    printf 'freshness\ts1\tmemory-off\t1\t0\t2\t0\n'
  } > "$good"
  if aggregate_scorecard "$good" 0.10 1 >/dev/null; then echo "ok: clean scorecard is SAFE"; else echo "BUG: clean scorecard flagged unsafe"; fail=1; fi
  if aggregate_scorecard "$good" 0.10 1 | grep -q '\-> freshness:.*=> PASS'; then echo "ok: dimension passes when it beats realistic + ties steelman"; else echo "BUG: dimension did not pass"; fail=1; fi

  # RE-RIG GUARD: memory beats realistic but LOSES to steelman => dimension must NOT pass.
  local rig="$tmp/rig.tsv"
  {
    printf 'freshness\ts1\tmemory-on\t1\t0\t2\t0\n'
    printf 'freshness\ts1\trealistic-baseline\t1\t0\t2\t0\n'
    printf 'freshness\ts1\tsteelman-baseline\t1\t1\t2\t0\n'
    printf 'freshness\ts1\tmemory-off\t1\t0\t2\t0\n'
  } > "$rig"
  if aggregate_scorecard "$rig" 0.10 1 | grep -q '\-> freshness:.*=> no'; then echo "ok: re-rig guard — losing to steelman fails the dimension"; else echo "BUG: re-rig not caught"; fail=1; fi

  # SAFETY: a memory-induced error fails the hard gate at min_runs=1.
  local unsafe="$tmp/unsafe.tsv"
  {
    printf 'freshness\ts1\tmemory-on\t1\t0\t2\t1\n'
    printf 'freshness\ts1\trealistic-baseline\t1\t0\t2\t0\n'
    printf 'freshness\ts1\tsteelman-baseline\t1\t1\t2\t0\n'
    printf 'freshness\ts1\tmemory-off\t1\t0\t2\t0\n'
  } > "$unsafe"
  if aggregate_scorecard "$unsafe" 0.10 1 >/dev/null; then echo "BUG: memory-induced error did not fail the safety gate"; fail=1; else echo "ok: memory-induced error fails the safety gate"; fi

  # P3 — single runs never gate: the same unsafe data at min_runs=5 is DIRECTIONAL,
  # exits 0 (does not gate), yet still reports + enumerates the memory-induced error.
  if aggregate_scorecard "$unsafe" 0.10 5 >/dev/null; then echo "ok: sub-min-runs unsafe run is directional (exit 0)"; else echo "BUG: sub-min-runs run gated"; fail=1; fi
  if aggregate_scorecard "$unsafe" 0.10 5 | grep -q 'DIRECTIONAL ONLY'; then echo "ok: sub-min-runs run prints DIRECTIONAL ONLY"; else echo "BUG: no DIRECTIONAL banner"; fail=1; fi
  if aggregate_scorecard "$unsafe" 0.10 5 | grep -q 'memory-induced errors: 1'; then echo "ok: MIE still enumerated when directional"; else echo "BUG: MIE not enumerated when directional"; fail=1; fi

  # IQR: turns {1,2,3,4,5} => median 3.0 [Q1 2.0, Q3 4.0].
  local iqr="$tmp/iqr.tsv"
  { for t in 1 2 3 4 5; do printf 'd\ts\tmemory-on\t%s\t1\t%s\t0\n' "$t" "$t"; done; } > "$iqr"
  if aggregate_scorecard "$iqr" 0.10 1 | grep -qE 'memory-on.*3\.0 \[2\.0-4\.0\]'; then echo "ok: IQR of {1,2,3,4,5} = median 3.0 [2.0-4.0]"; else echo "BUG: IQR wrong"; aggregate_scorecard "$iqr" 0.10 1 | grep memory-on; fail=1; fi

  # Wilson 95% CI: 4 successes of 5 (80%) => [0.38-0.96].
  local wil="$tmp/wil.tsv"
  {
    printf 'd\ts\tmemory-on\t1\t1\t2\t0\n'
    printf 'd\ts\tmemory-on\t2\t1\t2\t0\n'
    printf 'd\ts\tmemory-on\t3\t1\t2\t0\n'
    printf 'd\ts\tmemory-on\t4\t1\t2\t0\n'
    printf 'd\ts\tmemory-on\t5\t0\t2\t0\n'
  } > "$wil"
  if aggregate_scorecard "$wil" 0.10 1 | grep -qE 'memory-on.*80% \[37\.6-96\.4\]'; then echo "ok: Wilson CI for 4/5 = 80% [37.6-96.4]"; else echo "BUG: Wilson CI wrong"; aggregate_scorecard "$wil" 0.10 1 | grep memory-on; fail=1; fi

  # Complete gating run: all four arms at N=5 (>= min_runs), memory-on beats
  # realistic AND ties steelman, zero MIE => gates SAFE, dimension PASS, and is
  # NOT directional / not incomplete (locks the gating path vs the directional cases).
  local full="$tmp/full.tsv"
  {
    for t in 1 2 3 4 5; do printf 'd\ts\tmemory-on\t%s\t1\t2\t0\n'        "$t"; done
    for t in 1 2 3 4 5; do printf 'd\ts\trealistic-baseline\t%s\t0\t3\t0\n' "$t"; done
    for t in 1 2 3 4;   do printf 'd\ts\tsteelman-baseline\t%s\t1\t2\t0\n'  "$t"; done
                        printf 'd\ts\tsteelman-baseline\t5\t0\t2\t0\n'
    for t in 1 2 3 4 5; do printf 'd\ts\tmemory-off\t%s\t0\t5\t0\n'         "$t"; done
  } > "$full"
  local fout; fout="$(aggregate_scorecard "$full" 0.10 5)"
  if echo "$fout" | grep -q 'result: SAFE' && echo "$fout" | grep -q '\-> d:.*=> PASS' \
     && ! echo "$fout" | grep -q 'DIRECTIONAL' && ! echo "$fout" | grep -q 'incomplete arms'; then
    echo "ok: complete N=5 run gates SAFE + PASS, not directional"
  else
    echo "BUG: complete gating run mis-handled"; echo "$fout"; fail=1
  fi

  if [ "$fail" -eq 0 ]; then echo "self-test PASS"; return 0; fi
  echo "self-test FAIL" >&2; return 1
}

# ---- arg parsing -------------------------------------------------------------
MODE="run"; BIN_DIR=""; RUNS=""; OUT=""; MIN_RUNS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)      MODE="self-test"; shift ;;
    --bin-dir)        BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)           RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --min-runs)       MIN_RUNS="${2:?--min-runs needs a value}"
                      case "$MIN_RUNS" in ''|*[!0-9]*|0) echo "--min-runs must be a positive integer (got '$MIN_RUNS')" >&2; exit 2 ;; esac
                      shift 2 ;;
    --out)            OUT="${2:?--out needs a value}"; shift 2 ;;
    --scenarios-file) SCENARIOS_FILE="${2:?--scenarios-file needs a value}"; shift 2 ;;
    -h|--help)        awk 'NR>1 && /^#/ {print; next} NR>1 {exit}' "$0"; exit 2 ;;
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
[ -n "$MIN_RUNS" ] || MIN_RUNS="$(jq -r '.config.min_runs // 5' "$SCENARIOS_FILE")"
# RB_SCORECARD_TIE_MARGIN (env) overrides config; otherwise honor config.tie_margin
# (alias: steelman_tie), else 0.10.
if [ -z "${RB_SCORECARD_TIE_MARGIN+x}" ]; then
  TIE_MARGIN="$(jq -r '.config.tie_margin // .config.steelman_tie // 0.10' "$SCENARIOS_FILE")"
fi
# Validate min_runs regardless of source (CLI or config): a non-numeric/0 value
# coerces to 0 in awk, silently disabling the directional guard.
case "$MIN_RUNS" in ''|*[!0-9]*|0) echo "min_runs must be a positive integer (got '$MIN_RUNS')" >&2; exit 2 ;; esac

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
    export PATH="$BIN_DIR:$PATH"; cd "$proj" || return 1
    # `</dev/null`: claude -p reads stdin; without this it drains whatever fd 0
    # is (e.g. a caller's `while read` source), silently truncating the run.
    ${SESSION_TIMEOUT} claude -p "$prompt" \
      --setting-sources project --model "$MODEL" --max-budget-usd "$MAX_BUDGET_USD" \
      --permission-mode acceptEdits --allowedTools "Bash Edit Write" "$@" \
      </dev/null >"$log" 2>&1 || true
  )
}

install_rusty_brain() { # home proj
  local home="$1" proj="$2"
  (
    export HOME="$home"; export PATH="$BIN_DIR:$PATH"; cd "$proj" || return 1
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
        out="$(rusty-brain --json remember "$content" --type insight --supersedes "$last_id")"
      else
        out="$(rusty-brain --json remember "$content" --type insight)"
      fi
      last_id="$(jq -r '.id // empty' <<<"$out" 2>/dev/null)"
      [ -n "$last_id" ] || { echo "ERROR: explicit plant did not return an id for fact $i" >&2; return 1; }
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

    # memory-on. NOTE: split the `local`s — a single `local a=.. b=$a` does NOT
    # see the sibling under `set -u` (b errors as unbound).
    local mb="$base/on"
    local db="$mb/memory.db" ns="rb-sc-$id-r$run"
    local wh="$mb/hw" wp="$mb/wp"; seed_home "$wh"; mkdir -p "$wp"
    install_rusty_brain "$wh" "$wp"; rm -f "$wp/CLAUDE.md"; rm -rf "$wp/.claude/skills"
    local sockdir; sockdir="$(mktemp -d /tmp/rbsc.XXXXXX)"; local sock="$sockdir/s"
    (
      # Put the built binary FIRST on PATH (as run_session/install/plant_explicit
      # do): a stray `rusty-brain` elsewhere on PATH would otherwise shadow it and
      # the daemon launch fails with "unrecognized subcommand 'serve'".
      export PATH="$BIN_DIR:$PATH"
      export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
      # Start a daemon for the store and WAIT for the socket to bind before
      # planting (a fixed sleep races the bind — observed flaky). Capture the
      # daemon's output: a fail-closed bind gate with the output discarded turns
      # every failure into an unactionable "did not bind". It carries no secrets
      # (the daemon never receives ANTHROPIC_API_KEY) and is printed only on a
      # bind failure.
      derr="$sockdir/daemon.log"
      rusty-brain serve >"$derr" 2>&1 &
      dpid=$!
      # shellcheck disable=SC2329 # Invoked by the EXIT trap below.
      cleanup_memory_on() {
        kill "$dpid" 2>/dev/null || true
        rm -rf "$sockdir" 2>/dev/null || true
      }
      trap cleanup_memory_on EXIT
      # Poll up to 10s for the bind (normally <1s), but break the instant the
      # daemon process dies so a startup crash (e.g. a wrong binary that lacks
      # the `serve` subcommand) fails fast with its captured output, instead of
      # waiting out the whole budget on a process that already exited.
      bound=""
      for _ in $(seq 1 50); do
        if [ -S "$sock" ]; then bound=1; break; fi
        kill -0 "$dpid" 2>/dev/null || break
        sleep 0.2
      done
      if [ -z "$bound" ]; then
        echo "ERROR: memory-on daemon did not bind for $id run $run (daemon alive=$(kill -0 "$dpid" 2>/dev/null && echo yes || echo no))" >&2
        echo "----- daemon output (rusty-brain serve) -----" >&2
        cat "$derr" >&2 2>/dev/null || true
        echo "----- end daemon output -----" >&2
        exit 1
      fi
      if [ "$plant_mode" = "explicit" ]; then plant_explicit "$wh" "$facts"; fi
      # (auto-capture mode: a plant session would run here — wired with dimension B.)
      score_session "$dim" "$id" "memory-on" "$run" "$wp" "$wh" "$work" "$expect" "$forbid" "$stale"
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
# Read ALL scenarios into an array BEFORE running any (mirrors w35-ab-eval.sh): a
# streaming `while read < <(jq)` shares fd 0 with the loop body, and a body
# command that consumes stdin (claude -p) would eat the remaining scenario lines.
scenario_rows=()
while IFS= read -r row; do scenario_rows+=("$row"); done < <(jq -c '.scenarios[]' "$SCENARIOS_FILE")
for row in "${scenario_rows[@]}"; do run_scenario "$row"; done
echo
echo "== scorecard =="
aggregate_scorecard "$RESULTS" "$TIE_MARGIN" "$MIN_RUNS"
