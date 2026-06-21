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
# A failed/unparseable session scores this many turns — a worst-case sentinel,
# well above a normal task, so a failure can never make an arm look faster on the
# turns tiebreaker. Used by extract_usage (exercised by --self-test).
TURNS_FAIL_SENTINEL="${RB_SCORECARD_TURNS_FAIL_SENTINEL:-99}"

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

# ---- corpus generator (pure; deterministic; exercised by --self-test) --------
# gen_corpus <scenario_id> <n>  -> prints n markdown-bullet distractor lines.
# Deterministic (seeded by scenario_id) so re-runs are reproducible. Distractors
# are deliberately OFF-TOPIC from any scenario target (fictional service port
# numbers) so they never accidentally contain an `expect`/`stale` token. Used by
# the Class A (retrieval@scale) arms: planted into the memory-on store and written
# into both baselines' CLAUDE.md so a buried target is the only thing that differs.
# (Verbatim shape from scripts/w35-cache-trace.sh's gen_corpus.)
gen_corpus() {
  python3 - "$1" "$2" <<'PY'
import sys, hashlib, random
sid, n = sys.argv[1], int(sys.argv[2])
seed = int(hashlib.sha256(sid.encode()).hexdigest()[:12], 16)
random.seed(seed)
ports = random.sample(range(4000, 60000), n)
for i in range(n):
    print(f"- Service port convention: `service-svc-{i:04d}` listens on port {ports[i]}; do not reassign it.")
PY
}

# ---- usage extraction (pure; ADR-3; exercised by --self-test) -----------------
# extract_usage <stream-json-log>  -> echoes one TSV row (7 fields):
#   is_error  num_turns  cost  input_tok  cache_create_tok  cache_read_tok  output_tok
# read from the final `result` record of `claude -p --output-format stream-json
# --verbose` (it carries num_turns/total_cost_usd AND the session-aggregate
# usage.{input_tokens,cache_creation_input_tokens,cache_read_input_tokens,
# output_tokens} — the cache economics ADR-3 needs; see docs/eval/2026-06-19-*).
# A non-parseable log (timeout/error/budget) yields a worst-case sentinel row and
# is_error=true, so a failed session can never look like cheap successful caching.
# (Verbatim shape from scripts/w35-cache-trace.sh's extract_usage.)
extract_usage() {
  local log="$1"
  local r is_err turns cost inp cc cr out
  r="$(jq -c 'select(.type=="result")' "$log" 2>/dev/null | tail -1 || true)"
  if [ -n "$r" ]; then
    # `// true` would collapse an explicit false (jq's alt treats false as
    # empty), so gate on field presence: absent is_error => assume error.
    is_err="$(jq -r 'if has("is_error") then .is_error else true end' <<<"$r")"
    turns="$(jq -r '.num_turns // 0'           <<<"$r")"
    cost="$(jq -r  '.total_cost_usd // 0'      <<<"$r")"
    inp="$(jq -r   '.usage.input_tokens // 0'                <<<"$r")"
    cc="$(jq -r    '.usage.cache_creation_input_tokens // 0' <<<"$r")"
    cr="$(jq -r    '.usage.cache_read_input_tokens // 0'     <<<"$r")"
    out="$(jq -r   '.usage.output_tokens // 0' <<<"$r")"
  else
    is_err=true; turns="$TURNS_FAIL_SENTINEL"; cost=0; inp=0; cc=0; cr=0; out=0
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$is_err" "$turns" "$cost" "$inp" "$cc" "$cr" "$out"
}

# ---- baseline CLAUDE.md writer (pure; exercised by --self-test) --------------
# Write a baseline's CLAUDE.md from a body (the realistic/steelman text) and an
# optional distractor corpus (Class A). Writes nothing when both are empty (so a
# Class A realistic arm with no body still gets the distractors, and a Class C arm
# with an empty baseline gets no file at all — unchanged from before).
write_claude_md() { # file body distractors
  local file="$1" body="$2" distractors="$3"
  [ -n "$body" ] || [ -n "$distractors" ] || return 0
  {
    [ -n "$body" ] && printf '# Project conventions\n\n%s\n' "$body"
    [ -n "$distractors" ] && { printf '\n## Other recorded decisions\n\n'; printf '%s\n' "$distractors"; }
  } > "$file"
}

# ---- pure scorecard aggregation (exercised by --self-test) -------------------
# Reads a 13-field results TSV:
#   dimension scenario arm run success turns mie cost tok_in tok_cc tok_cr tok_out is_error
# (the first seven are unchanged; the trailing six are ADR-3 token/cost, appended
# AFTER mie so the existing column indices are stable). Prints the per-dimension
# scorecard — success as a Wilson 95% CI, turns as median [Q1-Q3] (P3: median +
# spread, never a bare mean), and mean total_cost_usd — plus, for the
# `retrieval_scale` dimension, an ADR-3 token/cost block (cache buckets +
# RATIFY/Opt-2/descope verdict; docs/eval/2026-06-19-*), then the safety result.
# Exit code: 0 for a DIRECTIONAL run (any arm below min_runs, or a missing arm —
# prints no SAFE/UNSAFE verdict, never gates; P3: single runs never gate);
# otherwise 0 iff the SAFETY gate holds (zero memory-induced errors, P4), 1 on
# UNSAFE. The hard gate fires only from a >=min-runs, complete run. Dimension
# verdicts (beats realistic AND ties steelman; ADR-3 cost) are reported, not gated.
#
#   $1 = tsv, $2 = steelman tie margin (default $TIE_MARGIN), $3 = min_runs (default 5).
aggregate_scorecard() {
  local tsv="$1" tie_margin="${2:-${TIE_MARGIN:-0.10}}" min_runs="${3:-5}"
  awk -F'\t' -v tie="$tie_margin" -v min_runs="$min_runs" '
    # cache-read fraction cr/(cr+in) — 0 when there is no input (ADR-3 diag).
    function ratio(cr, inp) { if (cr + inp == 0) return 0; return cr / (cr + inp) }
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
      cost=$8; inp=$9; cc=$10; cr=$11; is_err=$13;
      key = dim SUBSEP arm;
      n[key]++; s[key]+=succ;
      co[key]+=cost; ci_in[key]+=inp; ci_cc[key]+=cc; ci_cr[key]+=cr;
      if (is_err == "true") err[key]++;
      tk = key SUBSEP n[key]; tv[tk] = turns;
      dims[dim]=1;
      if (arm=="memory-on" && mie==1) { mie_total++; mie_list = mie_list sprintf("    - %s / %s run %s\n", dim, $2, $4) }
    }
    END {
      directional = 0;
      split("memory-on realistic-baseline steelman-baseline memory-off", order, " ");
      printf "%-15s %-18s %5s %16s %16s %9s\n", "dimension", "arm", "runs", "success [95% CI]", "med_turns [Q1-Q3]", "mcost$";
      pass_dims=0; total_dims=0;
      for (d in dims) {
        total_dims++;
        for (i=1;i<=4;i++) {
          a=order[i]; key=d SUBSEP a;
          if (n[key]==0) {
            directional = 1;   # missing arm = incomplete data → never gate (P3)
            printf "%-15s %-18s %5d %16s %16s %9s\n", d, a, 0, "n/a", "n/a", "n/a"; continue
          }
          rate[key]=s[key]/n[key];
          if (n[key] < min_runs) directional = 1;
          delete tarr; m=0;
          for (r=1;r<=n[key];r++){ m++; tarr[m]=tv[key SUBSEP r] }
          med = pctile(tarr, m, 0.5); q1 = pctile(tarr, m, 0.25); q3 = pctile(tarr, m, 0.75);
          wilson(s[key], n[key]);
          printf "%-15s %-18s %5d %5.0f%% [%.1f-%.1f] %8.1f [%.1f-%.1f] %9.4f\n",
                 d, a, n[key], rate[key]*100, wil_lo*100, wil_hi*100, med, q1, q3, co[key]/n[key];
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

      # --- ADR-3 retrieval@scale token/cost (docs/eval/2026-06-19-*) ----------
      # Accuracy is the PRIMARY axis (the dimension verdict above); cost
      # (total_cost_usd, cache-adjusted by construction) is the SECONDARY axis;
      # the cache buckets are diagnostic, never a pass/fail metric (they
      # double-count cheap cache reads). This block is informational — the only
      # hard gate is still zero memory-induced errors.
      has_scale = 0; for (d in dims) if (d=="retrieval_scale") has_scale=1;
      if (has_scale) {
        rk = "retrieval_scale";
        printf "\n== ADR-3 retrieval@scale token/cost (memory-on vs steelman-baseline; accuracy primary, cost secondary) ==\n";
        printf "%-20s %5s %6s %9s %10s %10s %10s %7s %9s %9s\n",
               "arm", "runs", "succ", "mcost$", "m_input", "m_ccrea", "m_cread", "cache%", "ctx_vol", "eff_in";
        for (i=1;i<=4;i++) {
          a=order[i]; key=rk SUBSEP a;
          if (n[key]==0) continue;
          mc=co[key]/n[key]; mi=ci_in[key]/n[key]; mcc=ci_cc[key]/n[key]; mcr=ci_cr[key]/n[key];
          # ctx_vol = in+cc+cr (context-window pressure); eff_in = in+1.25*cc+0.1*cr
          # (cache-weighted full-price input equivalents) — both diagnostic.
          printf "%-20s %5d %5.0f%% %9.4f %10.0f %10.0f %10.0f %6.1f%% %9.0f %9.0f\n",
                 a, n[key], 100*rate[key], mc, mi, mcc, mcr,
                 100*ratio(ci_cr[key], ci_in[key]), mi+mcc+mcr, mi+1.25*mcc+0.1*mcr;
        }
        onk=rk SUBSEP "memory-on"; stl=rk SUBSEP "steelman-baseline";
        on_cost=(n[onk]>0 ? co[onk]/n[onk] : 0); stl_cost=(n[stl]>0 ? co[stl]/n[stl] : 0);
        if (n[onk]==0 || n[stl]==0) {
          printf "  -> retrieval_scale: SKIP cost verdict (memory-on or steelman-baseline produced no runs)\n";
        } else if (err[onk]+err[stl] > 0) {
          printf "  -> retrieval_scale: SKIP cost verdict (session errors in memory-on or steelman-baseline)\n";
        } else if (on_cost <= 0 || stl_cost <= 0) {
          # FAIL CLOSED: total_cost_usd is the load-bearing ADR-3 cost axis. A
          # zero/absent cost on a NON-errored session (e.g. a claude build that
          # omits total_cost_usd, or the `// 0` parse fallback firing) must never
          # read as a passing 0-vs-0 comparison — it would silently RATIFY off a
          # collapsed cost axis. Treat it as invalid data and skip the verdict.
          printf "  -> retrieval_scale: SKIP cost verdict (zero/absent total_cost_usd on a non-errored cell — verify the result-record usage path)\n";
        } else {
          on_acc=rate[onk]; stl_acc=rate[stl];
          # acc floor: both arms must land the buried fact at least once, else a
          # 0-vs-0 tie would falsely read as "accuracy ok".
          acc_ok = (on_acc >= stl_acc) && (s[onk] > 0 && s[stl] > 0);
          # ADR-3 cost axis is total_cost_usd; "within 20%" => on <= 1.2*stl
          # (both costs are guaranteed > 0 by the fail-closed guard above).
          cost_ok = (on_cost <= 1.2 * stl_cost);
          if (acc_ok && cost_ok)        v="RATIFY Opt 3 (accuracy >= steelman AND total_cost_usd within 20%)";
          else if (acc_ok && !cost_ok)  v="Opt 2 candidate (accuracy wins; cost > 20% worse — consider caching the SessionStart digest)";
          else if (!acc_ok && cost_ok)  v="accuracy loses / not meaningful at scale (cost fine) — investigate retrieval, not caching";
          else                          v="descope token-cost axis (value is capture/freshness/reach per ADR-1)";
          printf "  -> retrieval_scale: acc on %.2f vs stl %.2f [%s] | cost$ on %.4f vs stl %.4f [%s]\n",
                 on_acc, stl_acc, (acc_ok?"ok":"NO"), on_cost, stl_cost, (cost_ok?"ok":"NO");
          printf "     => %s\n", v;
        }
      }

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

  # Emit one 13-field results row; the trailing ADR-3 fields default to a
  # cost/cache-free, no-error session so the scorecard-math fixtures stay terse.
  # sc_row dim scenario arm run success turns mie [cost in cc cr out is_err]
  sc_row() {
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$1" "$2" "$3" "$4" "$5" "$6" "$7" "${8:-0}" "${9:-0}" "${10:-0}" "${11:-0}" "${12:-0}" "${13:-false}"
  }

  # gen_corpus: deterministic, off-topic, emits exactly N lines.
  local ga gb
  ga="$(gen_corpus scale-http 3)"; gb="$(gen_corpus scale-http 3)"
  check "gen_corpus deterministic" "$ga" "$gb"
  if printf '%s' "$ga" | grep -qiwF ureq; then echo "BUG: gen_corpus leaked a target token"; fail=1; else echo "ok: gen_corpus off-topic"; fi
  check "gen_corpus emits N lines" "7" "$(gen_corpus scale-http 7 | grep -c .)"

  # write_claude_md: a Class A realistic arm (empty body) gets distractors ONLY
  # (target omitted); the steelman arm gets target + distractors; both-empty
  # writes no file at all (a Class C arm with no baseline text).
  local wd="$tmp/wcm"; mkdir -p "$wd"
  write_claude_md "$wd/realistic.md" "" "$(gen_corpus scale-http 2)"
  write_claude_md "$wd/steelman.md" "HTTP: use the \`ureq\` crate." "$(gen_corpus scale-http 2)"
  write_claude_md "$wd/none.md" "" ""
  if grep -qF 'service-svc-' "$wd/realistic.md" && ! grep -qiF ureq "$wd/realistic.md"; then echo "ok: write_claude_md realistic = distractors only (target omitted)"; else echo "BUG: realistic CLAUDE.md"; fail=1; fi
  if grep -qiF ureq "$wd/steelman.md" && grep -qF 'service-svc-' "$wd/steelman.md"; then echo "ok: write_claude_md steelman = target + distractors"; else echo "BUG: steelman CLAUDE.md"; fail=1; fi
  if [ ! -f "$wd/none.md" ]; then echo "ok: write_claude_md writes nothing when body + distractors empty"; else echo "BUG: empty write_claude_md created a file"; fail=1; fi

  # extract_usage: reads the session-aggregate cache buckets from the result
  # record; a non-parseable log yields the worst-case sentinel + is_error=true.
  local elog="$tmp/work.jsonl"
  {
    printf '{"type":"system","subtype":"init"}\n'
    printf '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"total_cost_usd":0.0123,"usage":{"input_tokens":1560,"cache_creation_input_tokens":4000,"cache_read_input_tokens":5400,"output_tokens":160},"result":"use ureq for HTTP"}\n'
  } > "$elog"
  check "extract_usage reads aggregate cache fields" "$(printf 'false\t2\t0.0123\t1560\t4000\t5400\t160')" "$(extract_usage "$elog")"
  : > "$tmp/empty.jsonl"
  check "extract_usage sentinel on empty log" "$(printf 'true\t%s\t0\t0\t0\t0\t0' "$TURNS_FAIL_SENTINEL")" "$(extract_usage "$tmp/empty.jsonl")"

  # Scorecard fixtures below have N=1 per arm; pass min_runs=1 (3rd arg) so they
  # still gate/verdict. The directional behavior is exercised separately.

  # A scorecard where memory beats realistic AND ties steelman, zero mie => SAFE, dim passes.
  local good="$tmp/good.tsv"
  {
    sc_row freshness s1 memory-on          1 1 2 0
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 0 2 0
  } > "$good"
  if aggregate_scorecard "$good" 0.10 1 >/dev/null; then echo "ok: clean scorecard is SAFE"; else echo "BUG: clean scorecard flagged unsafe"; fail=1; fi
  if aggregate_scorecard "$good" 0.10 1 | grep -q '\-> freshness:.*=> PASS'; then echo "ok: dimension passes when it beats realistic + ties steelman"; else echo "BUG: dimension did not pass"; fail=1; fi
  # A Class-C-only scorecard must NOT print the ADR-3 retrieval@scale block.
  if aggregate_scorecard "$good" 0.10 1 | grep -q 'ADR-3 retrieval@scale'; then echo "BUG: ADR-3 block printed without retrieval_scale data"; fail=1; else echo "ok: ADR-3 block omitted when no retrieval_scale dimension"; fi

  # RE-RIG GUARD: memory beats realistic but LOSES to steelman => dimension must NOT pass.
  local rig="$tmp/rig.tsv"
  {
    sc_row freshness s1 memory-on          1 0 2 0
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 0 2 0
  } > "$rig"
  if aggregate_scorecard "$rig" 0.10 1 | grep -q '\-> freshness:.*=> no'; then echo "ok: re-rig guard — losing to steelman fails the dimension"; else echo "BUG: re-rig not caught"; fail=1; fi

  # SAFETY: a memory-induced error fails the hard gate at min_runs=1.
  local unsafe="$tmp/unsafe.tsv"
  {
    sc_row freshness s1 memory-on          1 0 2 1
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 0 2 0
  } > "$unsafe"
  if aggregate_scorecard "$unsafe" 0.10 1 >/dev/null; then echo "BUG: memory-induced error did not fail the safety gate"; fail=1; else echo "ok: memory-induced error fails the safety gate"; fi

  # P3 — single runs never gate: the same unsafe data at min_runs=5 is DIRECTIONAL,
  # exits 0 (does not gate), yet still reports + enumerates the memory-induced error.
  if aggregate_scorecard "$unsafe" 0.10 5 >/dev/null; then echo "ok: sub-min-runs unsafe run is directional (exit 0)"; else echo "BUG: sub-min-runs run gated"; fail=1; fi
  if aggregate_scorecard "$unsafe" 0.10 5 | grep -q 'DIRECTIONAL ONLY'; then echo "ok: sub-min-runs run prints DIRECTIONAL ONLY"; else echo "BUG: no DIRECTIONAL banner"; fail=1; fi
  if aggregate_scorecard "$unsafe" 0.10 5 | grep -q 'memory-induced errors: 1'; then echo "ok: MIE still enumerated when directional"; else echo "BUG: MIE not enumerated when directional"; fail=1; fi

  # mcost$ column reports the per-arm mean total_cost_usd (0.02, 0.04 => 0.0300).
  local costf="$tmp/cost.tsv"
  {
    sc_row freshness s1 memory-on 1 1 2 0 0.02 0 0 0 0 false
    sc_row freshness s1 memory-on 2 1 2 0 0.04 0 0 0 0 false
  } > "$costf"
  if aggregate_scorecard "$costf" 0.10 1 | grep -qE 'memory-on.*0\.0300'; then echo "ok: mcost\$ column reports mean total_cost_usd"; else echo "BUG: mcost column"; aggregate_scorecard "$costf" 0.10 1 | grep memory-on; fail=1; fi

  # ADR-3 (retrieval@scale): RATIFY Opt 3 when memory-on accuracy >= steelman AND
  # total_cost_usd within 20% of steelman.
  local ratify="$tmp/ratify.tsv"
  {
    sc_row retrieval_scale a1 memory-on          1 1 3 0 0.02 600  4000 4400 100 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0.01 6000 0    0    100 false
  } > "$ratify"
  if aggregate_scorecard "$ratify" 0.10 1 | grep -qF 'RATIFY Opt 3'; then echo "ok: ADR-3 ratify when accuracy + cost within bounds"; else echo "BUG: ADR-3 ratify verdict"; aggregate_scorecard "$ratify" 0.10 1; fail=1; fi

  # ADR-3: Opt 2 candidate when accuracy wins but total_cost_usd > 20% worse.
  local opt2="$tmp/opt2.tsv"
  {
    sc_row retrieval_scale a1 memory-on          1 1 3 0 0.03 4000  4000 6000 100 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0.02 100   4000 9900 100 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0.02 100   4000 9900 100 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0.01 10000 0    0    100 false
  } > "$opt2"
  if aggregate_scorecard "$opt2" 0.10 1 | grep -qF 'Opt 2 candidate'; then echo "ok: ADR-3 Opt 2 candidate when cost > 20% worse"; else echo "BUG: ADR-3 opt2 verdict"; aggregate_scorecard "$opt2" 0.10 1; fail=1; fi

  # ADR-3: a session error in memory-on or steelman skips the cost verdict (a
  # failed session reads cost=0 and must not look like a cheap RATIFY).
  local serr="$tmp/serr.tsv"
  {
    sc_row retrieval_scale a1 memory-on         1 0 99 0 0 0 0 0 0 true
    sc_row retrieval_scale a1 steelman-baseline 1 0 99 0 0 0 0 0 0 true
  } > "$serr"
  local serr_out; serr_out="$(aggregate_scorecard "$serr" 0.10 1)"
  if printf '%s' "$serr_out" | grep -qF 'SKIP cost verdict (session errors'; then echo "ok: ADR-3 skips cost verdict on session errors"; else echo "BUG: ADR-3 error skip"; printf '%s\n' "$serr_out"; fail=1; fi
  if printf '%s' "$serr_out" | grep -qF 'RATIFY Opt 3'; then echo "BUG: ADR-3 ratified an errored cell"; fail=1; else echo "ok: ADR-3 does not ratify an errored cell"; fi

  # ADR-3: a NON-errored cell with zero/absent total_cost_usd must FAIL CLOSED
  # (a 0-vs-0 cost is not a passing comparison — the live result-record cost path
  # is unverified, so a collapsed cost axis must never silently RATIFY).
  local zcost="$tmp/zcost.tsv"
  {
    sc_row retrieval_scale a1 memory-on          1 1 3 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0 0 0 0 0 false
  } > "$zcost"
  local zout; zout="$(aggregate_scorecard "$zcost" 0.10 1)"
  if printf '%s' "$zout" | grep -qF 'SKIP cost verdict (zero/absent total_cost_usd'; then echo "ok: ADR-3 fails closed on zero/absent cost (non-errored)"; else echo "BUG: ADR-3 zero-cost skip"; printf '%s\n' "$zout"; fail=1; fi
  if printf '%s' "$zout" | grep -qF 'RATIFY Opt 3'; then echo "BUG: ADR-3 ratified a zero-cost cell"; fail=1; else echo "ok: ADR-3 does not ratify a zero-cost cell"; fi

  # IQR: turns {1,2,3,4,5} => median 3.0 [Q1 2.0, Q3 4.0].
  local iqr="$tmp/iqr.tsv"
  { for t in 1 2 3 4 5; do sc_row d s memory-on "$t" 1 "$t" 0; done; } > "$iqr"
  if aggregate_scorecard "$iqr" 0.10 1 | grep -qE 'memory-on.*3\.0 \[2\.0-4\.0\]'; then echo "ok: IQR of {1,2,3,4,5} = median 3.0 [2.0-4.0]"; else echo "BUG: IQR wrong"; aggregate_scorecard "$iqr" 0.10 1 | grep memory-on; fail=1; fi

  # Wilson 95% CI: 4 successes of 5 (80%) => [0.38-0.96].
  local wil="$tmp/wil.tsv"
  {
    sc_row d s memory-on 1 1 2 0
    sc_row d s memory-on 2 1 2 0
    sc_row d s memory-on 3 1 2 0
    sc_row d s memory-on 4 1 2 0
    sc_row d s memory-on 5 0 2 0
  } > "$wil"
  if aggregate_scorecard "$wil" 0.10 1 | grep -qE 'memory-on.*80% \[37\.6-96\.4\]'; then echo "ok: Wilson CI for 4/5 = 80% [37.6-96.4]"; else echo "BUG: Wilson CI wrong"; aggregate_scorecard "$wil" 0.10 1 | grep memory-on; fail=1; fi

  # Complete gating run: all four arms at N=5 (>= min_runs), memory-on beats
  # realistic AND ties steelman, zero MIE => gates SAFE, dimension PASS, and is
  # NOT directional / not incomplete (locks the gating path vs the directional cases).
  local full="$tmp/full.tsv"
  {
    for t in 1 2 3 4 5; do sc_row d s memory-on          "$t" 1 2 0; done
    for t in 1 2 3 4 5; do sc_row d s realistic-baseline "$t" 0 3 0; done
    for t in 1 2 3 4;   do sc_row d s steelman-baseline  "$t" 1 2 0; done
                           sc_row d s steelman-baseline  5   0 2 0
    for t in 1 2 3 4 5; do sc_row d s memory-off         "$t" 0 5 0; done
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

# Score one work session and append a 13-field scorecard row. The session runs
# under `--output-format stream-json --verbose` so the final result record
# carries num_turns, total_cost_usd, AND the session-aggregate cache buckets
# (ADR-3, docs/eval/2026-06-19-*); extract_usage reads them. The judged text is
# the model OUTPUT only: the final `.result` plus files written this session
# (mtime newer than a marker, size-capped) — never the seeded CLAUDE.md, so the
# baselines' planted distractors/target are not self-graded.
score_session() { # dim id arm run proj home work expect forbid stale
  local dim="$1" id="$2" arm="$3" run="$4" proj="$5" home="$6" work="$7" expect="$8" forbid="$9" stale="${10}"
  local jlog="$proj/../work.jsonl" jtext="$proj/../judge.txt" marker="$proj/../mark"
  : > "$marker"
  run_session "$home" "$proj" "$work" "$jlog" --output-format stream-json --verbose
  local u is_err turns cost inp cc cr out
  u="$(extract_usage "$jlog")"   # is_err turns cost input cc cr out
  is_err="$(cut -f1 <<<"$u")"; turns="$(cut -f2 <<<"$u")"; cost="$(cut -f3 <<<"$u")"
  inp="$(cut -f4 <<<"$u")"; cc="$(cut -f5 <<<"$u")"; cr="$(cut -f6 <<<"$u")"; out="$(cut -f7 <<<"$u")"
  if jq -e 'select(.type=="result")|.result' "$jlog" >/dev/null 2>&1; then
    jq -r 'select(.type=="result")|.result' "$jlog" | tail -1 > "$jtext"
  else
    cp "$jlog" "$jtext"
  fi
  find "$proj" -type f -not -path '*/.*' -newer "$marker" -size -256k -print0 2>/dev/null \
    | xargs -0 cat >> "$jtext" 2>/dev/null || true
  local res success mie
  res="$(judge_text "$jtext" "$expect" "$forbid" "$stale" "$arm")"
  success="${res% *}"; mie="${res#* }"
  # A server-level error forces failure: it must never look like a cheap success.
  if [ "$is_err" = "true" ]; then success=0; fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$dim" "$id" "$arm" "$run" "$success" "$turns" "$mie" \
    "$cost" "$inp" "$cc" "$cr" "$out" "$is_err" >> "$RESULTS"
  echo "   [$dim/$id $arm r$run] success=$success turns=$turns cost=$cost mie=$mie"
}

# Explicit plant (P2): each fact is stored via `rusty-brain remember`, in array
# order, isolating retrieval from the lossy auto-capture path. The superseded
# state (Class C: X later replaced by X') is constructed via `rusty-brain remember
# --supersedes <id>`, which stores the replacement and archives the prior memory
# in one atomic op (the same `supersedes` edge the SessionEnd hook exercises).
#
# Each fact: { content, importance?, supersedes_prev? }. Facts store in array
# order at their `importance` (default 5); a fact with `supersedes_prev: true` is
# stored as a SUPERSEDING update of the immediately prior planted fact (Class C: X
# then X' replacing X), archiving the predecessor so recall returns only the
# current value — explicit, no lossy auto-capture (P2). Class A plants a single
# TARGET here (importance 8 so it surfaces over the importance-5 distractor corpus
# planted by plant_corpus_distractors).
plant_explicit() { # home (env: RUSTY_BRAIN_*) <facts-json-array>
  local home="$1" facts="$2"
  ( export HOME="$home"; export PATH="$BIN_DIR:$PATH"
    local n i content imp sup out last_id=""
    n="$(jq 'length' <<<"$facts")"
    for (( i=0; i<n; i++ )); do
      content="$(jq -r ".[$i].content" <<<"$facts")"
      imp="$(jq -r ".[$i].importance // 5" <<<"$facts")"
      sup="$(jq -r ".[$i].supersedes_prev // false" <<<"$facts")"
      if [ "$sup" = "true" ] && [ -n "$last_id" ]; then
        out="$(rusty-brain --json remember "$content" --type insight --importance "$imp" --supersedes "$last_id")"
      else
        out="$(rusty-brain --json remember "$content" --type insight --importance "$imp")"
      fi
      last_id="$(jq -r '.id // empty' <<<"$out" 2>/dev/null)"
      [ -n "$last_id" ] || { echo "ERROR: explicit plant did not return an id for fact $i" >&2; return 1; }
    done
  )
}

# Class A (retrieval@scale): bulk-plant N off-topic distractors (importance 5)
# into the memory-on store so the TARGET (planted by plant_explicit at importance
# 8) is buried. Uses `rusty-brain remember --batch` — ONE process + ONE daemon
# connection for all N facts — because at 500+ facts a per-fact CLI invocation is
# dominated by process spawn + handshake (embeds are the cheap deterministic
# fallback). The leading markdown bullet is stripped so the stored content matches
# the bare fact (the bullet is only for the baselines' CLAUDE.md rendering).
plant_corpus_distractors() { # home scenario_id n
  local home="$1" sid="$2" n="$3"
  [ "$n" -gt 0 ] || return 0
  ( export HOME="$home"; export PATH="$BIN_DIR:$PATH"
    gen_corpus "$sid" "$n" | sed 's/^- //' \
      | rusty-brain remember --batch --type insight --importance 5 >/dev/null \
      || { echo "ERROR: bulk corpus plant failed for $sid (n=$n)" >&2; return 1; }
  )
}

run_scenario() { # row
  local row="$1"
  local id dim plant_mode work expect forbid stale realistic steelman facts corpus
  id="$(jq -r '.id' <<<"$row")"; dim="$(jq -r '.dimension' <<<"$row")"
  plant_mode="$(jq -r '.plant_mode' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"; expect="$(jq -r '.expect' <<<"$row")"
  forbid="$(jq -r '.forbid // ""' <<<"$row")"; stale="$(jq -r '.stale_token // ""' <<<"$row")"
  realistic="$(jq -r '.realistic_claude_md // ""' <<<"$row")"
  steelman="$(jq -r '.steelman_claude_md // ""' <<<"$row")"
  facts="$(jq -c '.plant // []' <<<"$row")"
  # corpus_size (Class A): N off-topic distractors that bury the target. Generated
  # ONCE per scenario (deterministic, seeded by id) so every run + arm sees the
  # same corpus. 0 (default, e.g. Class C) => no distractors, behavior unchanged.
  corpus="$(jq -r '.corpus_size // 0' <<<"$row")"
  case "$corpus" in ''|*[!0-9]*) echo "ERROR: $id corpus_size must be a non-negative integer (got '$corpus')" >&2; return 1 ;; esac
  local distractors=""
  if [ "$corpus" -gt 0 ]; then distractors="$(gen_corpus "$id" "$corpus")"; fi
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
      export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
      # Start a daemon for the store and WAIT for the socket to bind before
      # planting (a fixed sleep races the bind — observed flaky).
      rusty-brain serve >/dev/null 2>&1 &
      dpid=$!
      # shellcheck disable=SC2329 # Invoked by the EXIT trap below.
      cleanup_memory_on() {
        kill "$dpid" 2>/dev/null || true
        rm -rf "$sockdir" 2>/dev/null || true
      }
      trap cleanup_memory_on EXIT
      for _ in $(seq 1 50); do [ -S "$sock" ] && break; sleep 0.2; done
      [ -S "$sock" ] || { echo "ERROR: memory-on daemon did not bind for $id run $run" >&2; exit 1; }
      if [ "$plant_mode" = "explicit" ]; then
        plant_explicit "$wh" "$facts"
        # Class A: bury the planted target under the distractor corpus (bulk-load).
        plant_corpus_distractors "$wh" "$id" "$corpus"
      fi
      # (auto-capture mode: a plant session would run here — wired with dimension B.)
      score_session "$dim" "$id" "memory-on" "$run" "$wp" "$wh" "$work" "$expect" "$forbid" "$stale"
    )
    rm -rf "$sockdir" 2>/dev/null || true

    # realistic-baseline + steelman-baseline + memory-off. For Class A the
    # distractor corpus is written into BOTH baselines' CLAUDE.md so the buried
    # target is the ONLY difference: steelman holds target + distractors (diligent
    # human), realistic holds distractors only (target omitted — the common
    # "nobody wrote it down" reality).
    local rb="$base/realistic"; seed_home "$rb/h"; mkdir -p "$rb/p"
    write_claude_md "$rb/p/CLAUDE.md" "$realistic" "$distractors"
    score_session "$dim" "$id" "realistic-baseline" "$run" "$rb/p" "$rb/h" "$work" "$expect" "$forbid" "$stale"

    local sb="$base/steelman"; seed_home "$sb/h"; mkdir -p "$sb/p"
    write_claude_md "$sb/p/CLAUDE.md" "$steelman" "$distractors"
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
