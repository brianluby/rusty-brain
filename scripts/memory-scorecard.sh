#!/usr/bin/env bash
# shellcheck disable=SC2030,SC2031
# Memory-value scorecard harness (W3.5 criterion redesign; see
# docs/eval/2026-06-16-w35-criterion-redesign.md). The successor to the retired
# W3.5 A/B single-fact gate. Instead of "memory-on beats one
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
# Session-log retention (Vikunja #502): the ephemeral workroot (and with it
# every per-session stream-json log) is deleted on exit, which made the
# 2026-07-12 N=5 safety-gate MIEs undiagnosable. Opt in to retention with
# `--log-dir DIR` (retain there) or `RB_SCORECARD_KEEP_LOGS=1` (retain under
# `<dir of --out>/scorecard-session-logs`). Retained files are log-shaped only
# and carry no key material (see preserve_session_logs).
#
# Usage:
#   memory-scorecard.sh --self-test                       # judge + aggregation math, no API
#   memory-scorecard.sh [--agent claude-code|codex|opencode|gemini|hermes|all] [--bin-dir DIR] [--runs N] [--min-runs N] [--out FILE] [--log-dir DIR] [--scenarios-file F]
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

scorecard_agent_supported() { # agent
  [ "$1" = "claude-code" ]
}

scorecard_skip_reason() { # agent
  case "$1" in
    codex)    printf 'scorecard_unsupported_codex_fixture_gated' ;;
    opencode) printf 'scorecard_unsupported_opencode_plugin_deferred' ;;
    gemini)   printf 'scorecard_unsupported_gemini_not_first_priority' ;;
    hermes)   printf 'scorecard_unsupported_hermes_discovery_gated' ;;
    *)        printf 'scorecard_unsupported_unknown_agent' ;;
  esac
}

scorecard_skip_detail() { # agent
  case "$1" in
    codex)
      printf 'Codex scorecard is blocked until capture lifecycle, prompt retrieval, and apply_patch fixture gates are resolved.'
      ;;
    opencode)
      printf 'OpenCode scorecard is blocked until the JS/TS plugin config path and lifecycle fixtures are implemented.'
      ;;
    gemini)
      printf 'Gemini has an adapter, but the cross-agentic scorecard currently supports only Claude Code; Gemini scorecard support is not yet implemented.'
      ;;
    hermes)
      printf 'Hermes is discovery-gated; no hook names, config paths, or lifecycle semantics are verified.'
      ;;
    *)
      printf 'Unknown scorecard agent target.'
      ;;
  esac
}

# The earliest blocked pipeline stage (capture|config|scoring) for an
# unsupported agent, so machine-readable skips triage to the real gate.
# `retrieval` gaps never appear here: a missing injection channel does not stop
# the harness from running; capture/config gaps do.
#   codex    -> capture (terminus/apply_patch fixture-gated; see capability.rs)
#   opencode -> config  (rb-install JS/TS plugin path deferred; adapter works)
#   hermes   -> config  (discovery-gated; no verified config path to install)
#   gemini   -> scoring (adapter/capture exist; scorecard support unimplemented)
scorecard_skip_phase() { # agent
  case "$1" in
    codex)           printf 'capture' ;;
    opencode|hermes) printf 'config' ;;
    *)               printf 'scoring' ;;
  esac
}

scorecard_skip_line() { # agent
  local agent="$1"
  printf 'agent=%s\tdimension=all\tscenario=all\tphase=%s\tstatus=skip\treason=%s\tdetail=%s\n' \
    "$agent" "$(scorecard_skip_phase "$agent")" "$(scorecard_skip_reason "$agent")" "$(scorecard_skip_detail "$agent")"
}

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
# (Pattern carried from the retired P4a cache-trace prototype.)
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
# A PARSEABLE result that is itself an error (is_error=true — e.g. budget
# exceeded) also gets the worst-case turns sentinel, so a failure never lowers an
# arm's med_turns regardless of how it failed. (Cache-token extraction was first
# prototyped in the retired P4a cache-trace harness; the scorecard adds the
# parseable-error turns sentinel.)
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
    # A parseable but errored result must not report a low turn count (absent/0
    # num_turns) that undercuts the sentinel contract — force the sentinel.
    [ "$is_err" = "true" ] && turns="$TURNS_FAIL_SENTINEL"
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
  # Explicit if-blocks, not trailing `[ -n ... ] && ...` guards: a false guard
  # as the last command makes the group (and under `set -e`, the whole script)
  # fail silently whenever a scenario has no distractors.
  {
    if [ -n "$body" ]; then printf '# Project conventions\n\n%s\n' "$body"; fi
    if [ -n "$distractors" ]; then
      printf '\n## Other recorded decisions\n\n'
      printf '%s\n' "$distractors"
    fi
  } > "$file"
}

# ---- pure scorecard aggregation (exercised by --self-test) -------------------
# Reads a results TSV. The first 13 fields are stable:
#   dimension scenario arm run success turns mie cost tok_in tok_cc tok_cr tok_out is_error
# (the first seven are unchanged; the trailing six are ADR-3 token/cost, appended
# AFTER mie so the existing column indices are stable). Class B appends four more
# fields after those stable fields:
#   cap_fidelity cap_reason cap_summary_count cap_mcp_bypass_count
# Prints the per-dimension
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
    /^agent=/ { next }
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
      cap_fid=$14; cap_reason=$15; cap_summary=$16; cap_mcp=$17;
      key = dim SUBSEP arm;
      n[key]++; s[key]+=succ;
      co[key]+=cost; ci_in[key]+=inp; ci_cc[key]+=cc; ci_cr[key]+=cr;
      if (is_err == "true") err[key]++;
      # Track per-RUN cost collapse: a non-errored run with zero/absent
      # total_cost_usd. The mean can stay positive while one run collapsed the
      # cost axis, so the ADR-3 verdict must fail closed on ANY such run, not
      # just an all-zero mean.
      if (is_err != "true" && cost + 0 <= 0) bad_cost[key]++;
      tk = key SUBSEP n[key]; tv[tk] = turns;
      dims[dim]=1;
      if (arm=="memory-on" && mie==1) { mie_total++; mie_list = mie_list sprintf("    - %s / %s run %s\n", dim, $2, $4) }
      if (dim=="capture" && arm=="memory-on" && cap_fid != "" && cap_fid != "na") {
        csc=$2;
        cap_scenarios[csc]=1;
        cap_n[csc]++; cap_s[csc]+=cap_fid+0;
        cap_summary_total[csc]+=cap_summary+0; cap_mcp_total[csc]+=cap_mcp+0;
        cap_reason_count[csc SUBSEP cap_reason]++;
        cap_total_n++; cap_total_s+=cap_fid+0;
        cap_total_summary+=cap_summary+0; cap_total_mcp+=cap_mcp+0;
        cap_reason_total[cap_reason]++;
      }
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
          if (d == "capture") {
            cap_target_met = (cap_total_n > 0 && cap_total_s / cap_total_n >= 0.80);
            verdict = (beats_realistic && ties_steelman && cap_target_met) ? "PASS" : "no";
            if (beats_realistic && ties_steelman && cap_target_met) pass_dims++;
            printf "  -> %s: beats_realistic=%s ties_steelman=%s capture_fidelity_target=%s  => %s\n\n",
                   d, (beats_realistic?"yes":"NO"), (ties_steelman?"yes":"NO"), (cap_target_met?"yes":"NO"), verdict;
          } else {
            verdict = (beats_realistic && ties_steelman) ? "PASS" : "no";
            if (beats_realistic && ties_steelman) pass_dims++;
            printf "  -> %s: beats_realistic=%s ties_steelman=%s  => %s\n\n",
                   d, (beats_realistic?"yes":"NO"), (ties_steelman?"yes":"NO"), verdict;
          }
        } else {
          printf "  -> %s: incomplete arms (no verdict)\n\n", d;
        }
      }
      printf "scorecard: %d/%d dimensions pass (tracked, non-gating)\n", pass_dims, total_dims;

      if (cap_total_n > 0) {
        split("cap_ok cap_no_session_summary cap_summary_missing_fact cap_forbidden_token cap_list_error cap_timeout cap_mcp_bypass_detected", reason_order, " ");
        printf "\n== Class B capture fidelity (direct hook-origin session-summary; report-only) ==\n";
        printf "%-28s %5s %18s %9s %10s %s\n", "scenario", "runs", "fidelity [95% CI]", "summaries", "mcp_bypass", "reasons";
        for (csc in cap_scenarios) {
          wilson(cap_s[csc], cap_n[csc]);
          reasons = ""; sep = "";
          for (ri=1; ri<=7; ri++) {
            rr=reason_order[ri]; rc=cap_reason_count[csc SUBSEP rr]+0;
            if (rc > 0) { reasons = reasons sep rr "=" rc; sep = "," }
          }
          if (reasons == "") reasons = "none";
          printf "%-28s %5d %5.0f%% [%.1f-%.1f] %9d %10d %s\n",
                 csc, cap_n[csc], 100*cap_s[csc]/cap_n[csc], wil_lo*100, wil_hi*100,
                 cap_summary_total[csc], cap_mcp_total[csc], reasons;
        }
        wilson(cap_total_s, cap_total_n);
        target_met = (cap_total_s / cap_total_n >= 0.80);
        reasons = ""; sep = "";
        for (ri=1; ri<=7; ri++) {
          rr=reason_order[ri]; rc=cap_reason_total[rr]+0;
          if (rc > 0) { reasons = reasons sep rr "=" rc; sep = "," }
        }
        if (reasons == "") reasons = "none";
        printf "capture fidelity: %.0f%% [%.1f-%.1f] (%d/%d) target>=80%%=%s summaries=%d mcp_bypass=%d reasons=%s\n",
               100*cap_total_s/cap_total_n, wil_lo*100, wil_hi*100, cap_total_s, cap_total_n,
               (target_met?"yes":"NO"), cap_total_summary, cap_total_mcp, reasons;
      }

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
        } else if (bad_cost[onk] + bad_cost[stl] > 0) {
          # FAIL CLOSED: total_cost_usd is the load-bearing ADR-3 cost axis. ANY
          # non-errored run with zero/absent cost (a claude build that omits
          # total_cost_usd, or the `// 0` parse fallback firing) collapses the
          # axis — and a positive mean from the other runs would still let it
          # RATIFY. Skip the verdict whenever either arm has even one such run.
          printf "  -> retrieval_scale: SKIP cost verdict (zero/absent total_cost_usd on a non-errored run — verify the result-record usage path)\n";
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

capture_fidelity_from_json() { # expect forbid json_file -> cap_fidelity<TAB>reason<TAB>summary_count<TAB>mcp_bypass_count
  python3 - "$1" "$2" "$3" <<'PY'
import json, sys

expect, forbid, path = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    data = json.load(open(path))
except Exception:
    print("0\tcap_list_error\t0\t0")
    raise SystemExit(0)

notes = data.get("memories", data) if isinstance(data, dict) else data
if not isinstance(notes, list):
    print("0\tcap_list_error\t0\t0")
    raise SystemExit(0)

def live(note):
    return isinstance(note, dict) and note.get("archived_at") is None

def hay(note):
    return f"{note.get('content') or ''}\n{note.get('summary') or ''}".lower()

def contains(note, token):
    return bool(token) and token.lower() in hay(note)

live_notes = [n for n in notes if live(n)]
summaries = [
    n for n in live_notes
    if n.get("origin_source") == "hook" and "session-summary" in (n.get("tags") or [])
]
mcp_bypass = [
    n for n in live_notes
    if n.get("origin_source") == "mcp" and contains(n, expect)
]

has_expect = any(contains(n, expect) for n in summaries)
has_forbid = bool(forbid) and any(contains(n, forbid) for n in summaries)

reason = "cap_summary_missing_fact"
fidelity = 0
if mcp_bypass:
    reason = "cap_mcp_bypass_detected"
elif not summaries:
    reason = "cap_no_session_summary"
elif has_expect:
    # The decided value WAS captured. A correct summary that names the rejected
    # alternative in passing ("use ureq, not reqwest") must not false-fail, so
    # expect-present wins over forbid-present -- the same rationale the freshness
    # dimension uses to omit `forbid` entirely.
    reason = "cap_ok"
    fidelity = 1
elif has_forbid:
    # No summary captured the decided value AND one recorded the rejected
    # alternative instead: the hook captured the WRONG decision.
    reason = "cap_forbidden_token"

print(f"{fidelity}\t{reason}\t{len(summaries)}\t{len(mcp_bypass)}")
PY
}

reach_identity_paths() { # memory_on_base -> ha<TAB>pa<TAB>hb<TAB>pb
  local mb="$1"
  printf '%s\t%s\t%s\t%s\n' "$mb/ha" "$mb/pa" "$mb/hb" "$mb/pb"
}

# ---- session-log retention (pure; exercised by --self-test) -------------------
# preserve_session_logs <workroot> <dest>: copy the diagnosable per-session
# artifacts out of the ephemeral workroot into <dest>, preserving relative
# paths, before cleanup deletes the workroot (Vikunja #502: the 2026-07-12 N=5
# safety-gate MIEs were undiagnosable because every session log was rm -rf'd).
# Copies ONLY log-shaped files — the claude stream-json session logs
# (work.jsonl / plant.jsonl), the judged model output (judge.txt), and the
# memory-on daemon log (daemon.log) — never store DBs or seeded CLAUDE.md
# files, so the retained tree stays small. SECRET-SAFE: none of these carry key
# material — `claude -p --output-format stream-json` never echoes
# ANTHROPIC_API_KEY, and the rusty-brain daemon never receives it.
preserve_session_logs() { # workroot dest
  local workroot="$1" dest="$2"
  mkdir -p "$dest"
  local f rel
  while IFS= read -r -d '' f; do
    rel="${f#"$workroot"/}"
    mkdir -p "$dest/$(dirname "$rel")"
    cp "$f" "$dest/$rel"
  done < <(find "$workroot" -type f \
             \( -name '*.jsonl' -o -name 'judge.txt' -o -name 'daemon.log' \) -print0)
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

  # Emit one results row; the trailing ADR-3 and Class-B capture fields default
  # to a cost/cache-free, no-error, non-capture session so fixtures stay terse.
  # sc_row dim scenario arm run success turns mie [cost in cc cr out is_err cap_fid cap_reason cap_summary cap_mcp]
  sc_row() {
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$1" "$2" "$3" "$4" "$5" "$6" "$7" "${8:-0}" "${9:-0}" "${10:-0}" "${11:-0}" "${12:-0}" "${13:-false}" \
      "${14:-na}" "${15:-na}" "${16:-0}" "${17:-0}"
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

  # Class B direct capture parser: only live hook-origin session summaries count;
  # MCP-origin target memories are reported as bypasses, never a capture pass.
  local capj="$tmp/capture.json"
  printf '%s\n' '[{"content":"Decision: use ureq for HTTP","summary":"","tags":["hook","session-summary"],"origin_source":"hook","archived_at":null}]' > "$capj"
  check "capture parser accepts hook session-summary" "$(printf '1\tcap_ok\t1\t0')" "$(capture_fidelity_from_json ureq "" "$capj")"
  printf '%s\n' '[{"content":"Decision: use ureq for HTTP","summary":"","tags":["session-summary"],"origin_source":"mcp","archived_at":null}]' > "$capj"
  check "capture parser detects MCP bypass" "$(printf '0\tcap_mcp_bypass_detected\t0\t1')" "$(capture_fidelity_from_json ureq "" "$capj")"
  printf '%s\n' '[{"content":"Decision: use reqwest for HTTP","summary":"","tags":["hook","session-summary"],"origin_source":"hook","archived_at":null}]' > "$capj"
  check "capture parser catches missing expected fact" "$(printf '0\tcap_summary_missing_fact\t1\t0')" "$(capture_fidelity_from_json ureq "" "$capj")"
  # Forbidden token fires only when the decided value was NOT captured and the
  # rejected alternative was: content names reqwest (forbid) but not ureq (expect).
  check "capture parser catches forbidden token" "$(printf '0\tcap_forbidden_token\t1\t0')" "$(capture_fidelity_from_json ureq reqwest "$capj")"
  # A correct summary that names the rejected alternative in passing must PASS:
  # expect present wins over forbid present, so this is cap_ok, not a false-fail.
  printf '%s\n' '[{"content":"Decision: use ureq, not reqwest","summary":"","tags":["hook","session-summary"],"origin_source":"hook","archived_at":null}]' > "$capj"
  check "capture parser passes correct fact that names the alternative" "$(printf '1\tcap_ok\t1\t0')" "$(capture_fidelity_from_json ureq reqwest "$capj")"
  printf '%s\n' '[{"content":"Decision: use ureq for HTTP","summary":"","tags":["hook","session-summary"],"origin_source":"hook","archived_at":"2026-06-23T00:00:00Z"}]' > "$capj"
  check "capture parser ignores archived summaries" "$(printf '0\tcap_no_session_summary\t0\t0')" "$(capture_fidelity_from_json ureq "" "$capj")"

  # Class R pure layout invariant: identity A and B get distinct homes/projects.
  local ha pa hb pb
  IFS=$'\t' read -r ha pa hb pb < <(reach_identity_paths "$tmp/on")
  if [ "$ha" = "$tmp/on/ha" ] && [ "$pa" = "$tmp/on/pa" ] && [ "$hb" = "$tmp/on/hb" ] && [ "$pb" = "$tmp/on/pb" ] \
     && [ "$ha" != "$hb" ] && [ "$pa" != "$pb" ]; then
    echo "ok: reach identity layout uses separate A/B homes and projects"
  else
    echo "BUG: reach identity layout"; fail=1
  fi

  # Skip lines are a machine contract: phase names the earliest blocked pipeline
  # stage (capture/config/scoring), not a blanket phase=scoring.
  local skip_line
  skip_line="$(scorecard_skip_line codex)"
  if printf '%s' "$skip_line" | grep -qF $'agent=codex\tdimension=all\tscenario=all\tphase=capture\tstatus=skip\treason=scorecard_unsupported_codex_fixture_gated'; then
    echo "ok: scorecard codex skip is machine-readable and names the capture gate"
  else
    echo "BUG: scorecard codex skip line"; printf '%s\n' "$skip_line"; fail=1
  fi
  skip_line="$(scorecard_skip_line opencode)"
  if printf '%s' "$skip_line" | grep -qF $'agent=opencode\tdimension=all\tscenario=all\tphase=config\tstatus=skip\treason=scorecard_unsupported_opencode_plugin_deferred'; then
    echo "ok: scorecard opencode skip is machine-readable and names the config gate"
  else
    echo "BUG: scorecard opencode skip line"; printf '%s\n' "$skip_line"; fail=1
  fi
  skip_line="$(scorecard_skip_line hermes)"
  if printf '%s' "$skip_line" | grep -qF $'agent=hermes\tdimension=all\tscenario=all\tphase=config\tstatus=skip\treason=scorecard_unsupported_hermes_discovery_gated'; then
    echo "ok: scorecard hermes skip is machine-readable and names the config gate"
  else
    echo "BUG: scorecard hermes skip line"; printf '%s\n' "$skip_line"; fail=1
  fi
  skip_line="$(scorecard_skip_line gemini)"
  if printf '%s' "$skip_line" | grep -qF $'agent=gemini\tdimension=all\tscenario=all\tphase=scoring\tstatus=skip\treason=scorecard_unsupported_gemini_not_first_priority'; then
    echo "ok: scorecard gemini skip is machine-readable (scorecard itself unimplemented)"
  else
    echo "BUG: scorecard gemini skip line"; printf '%s\n' "$skip_line"; fail=1
  fi
  if scorecard_agent_supported claude-code && ! scorecard_agent_supported codex; then
    echo "ok: only claude-code is currently scorecard-supported"
  else
    echo "BUG: scorecard supported-agent predicate"; fail=1
  fi

  # Scenario-file contracts for the B/R additions. This is intentionally no-API:
  # it catches schema drift, answer leakage in realistic reach baselines, and
  # explicit-plant bypasses in auto-capture rows. Unlike the rest of the self-test
  # this reads the on-disk scenarios file, so guard its presence with a clear
  # message instead of letting python die on an unactionable traceback.
  if [ ! -f "$SCENARIOS_FILE" ]; then
    echo "BUG: scenarios file not found at '$SCENARIOS_FILE' (set --scenarios-file)"; fail=1
  elif python3 - "$SCENARIOS_FILE" <<'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path))
scenarios = data.get("scenarios", [])
errors = []

captures = [s for s in scenarios if s.get("dimension") == "capture"]
if len(captures) < 3:
    errors.append(f"need >=3 capture scenarios, got {len(captures)}")
for s in captures:
    sid = s.get("id", "<missing>")
    if s.get("plant_mode") != "auto-capture":
        errors.append(f"{sid}: capture scenario must use plant_mode=auto-capture")
    if not s.get("plant_session"):
        errors.append(f"{sid}: auto-capture scenario needs plant_session")
    if s.get("plant") not in (None, "", []):
        errors.append(f"{sid}: auto-capture scenario must not have explicit plant")
    if not (s.get("capture_expect") or s.get("expect")):
        errors.append(f"{sid}: capture scenario needs capture_expect or expect")

reaches = [s for s in scenarios if s.get("dimension") == "reach"]
if len(reaches) < 3:
    errors.append(f"need >=3 reach scenarios, got {len(reaches)}")
for s in reaches:
    sid = s.get("id", "<missing>")
    expect = str(s.get("expect") or "").lower()
    realistic = str(s.get("realistic_claude_md") or "").lower()
    steelman = str(s.get("steelman_claude_md") or "").lower()
    if s.get("plant_mode") != "explicit":
        errors.append(f"{sid}: reach scenario must use explicit plant")
    plant = s.get("plant")
    if not isinstance(plant, list) or not plant:
        errors.append(f"{sid}: reach scenario needs non-empty plant array")
    if expect and expect in realistic:
        errors.append(f"{sid}: realistic B CLAUDE.md leaks expect token")
    if expect and expect not in steelman:
        errors.append(f"{sid}: steelman B CLAUDE.md must contain expect token")

if errors:
    for e in errors:
        print("BUG:", e)
    raise SystemExit(1)
print("ok: scenario contracts cover Class B capture and Class R reach")
PY
  then
    :
  else
    fail=1
  fi

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
  # A PARSEABLE result that is itself an error gets the worst-case turns sentinel
  # (not its own low/absent num_turns), so a failure never lowers med_turns.
  printf '{"type":"result","is_error":true,"num_turns":1,"total_cost_usd":0,"usage":{}}\n' > "$tmp/error-result.jsonl"
  check "extract_usage sentinel on parseable error result" "$(printf 'true\t%s\t0\t0\t0\t0\t0' "$TURNS_FAIL_SENTINEL")" "$(extract_usage "$tmp/error-result.jsonl")"

  # Scorecard fixtures below have N=1 per arm; pass min_runs=1 (3rd arg) so they
  # still gate/verdict. The directional behavior is exercised separately.

  # A scorecard where memory beats realistic AND ties steelman, zero mie => SAFE, dim passes.
  local good="$tmp/good.tsv"
  {
    scorecard_skip_line codex
    sc_row freshness s1 memory-on          1 1 2 0
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 0 2 0
  } > "$good"
  if aggregate_scorecard "$good" 0.10 1 >/dev/null; then echo "ok: clean scorecard is SAFE"; else echo "BUG: clean scorecard flagged unsafe"; fail=1; fi
  if aggregate_scorecard "$good" 0.10 1 | grep -q '\-> freshness:.*=> PASS'; then echo "ok: dimension passes when it beats realistic + ties steelman"; else echo "BUG: dimension did not pass"; fail=1; fi
  # A Class-C-only scorecard must NOT print the ADR-3 retrieval@scale block.
  if aggregate_scorecard "$good" 0.10 1 | grep -q 'ADR-3 retrieval@scale'; then echo "BUG: ADR-3 block printed without retrieval_scale data"; fail=1; else echo "ok: ADR-3 block omitted when no retrieval_scale dimension"; fi

  # Class B capture aggregation: capture fields are appended after the stable
  # 13 scorecard fields and roll up independently from end-to-end success.
  local capagg="$tmp/capagg.tsv"
  {
    sc_row capture cap-one memory-on          1 1 2 0 0 0 0 0 0 false 1 cap_ok 1 0
    sc_row capture cap-one realistic-baseline 1 0 2 0
    sc_row capture cap-one steelman-baseline  1 1 2 0
    sc_row capture cap-one memory-off         1 0 2 0
    sc_row capture cap-two memory-on          1 0 2 0 0 0 0 0 0 false 0 cap_summary_missing_fact 1 0
    sc_row capture cap-two realistic-baseline 1 0 2 0
    sc_row capture cap-two steelman-baseline  1 1 2 0
    sc_row capture cap-two memory-off         1 0 2 0
  } > "$capagg"
  local capout; capout="$(aggregate_scorecard "$capagg" 0.10 1)"
  if printf '%s' "$capout" | grep -qF 'Class B capture fidelity'; then echo "ok: capture aggregation block prints"; else echo "BUG: capture aggregation block missing"; printf '%s\n' "$capout"; fail=1; fi
  if printf '%s' "$capout" | grep -qF 'capture fidelity: 50%'; then echo "ok: capture aggregation reports aggregate rate"; else echo "BUG: capture aggregate rate"; printf '%s\n' "$capout"; fail=1; fi

  # Class B verdict: downstream recall can beat/tie baselines while direct
  # capture fidelity is still below the tracked >=80% target; that must NOT mark
  # the capture dimension as passing.
  local caplow="$tmp/caplow.tsv"
  {
    sc_row capture cap-one memory-on          1 1 2 0 0 0 0 0 0 false 1 cap_ok 1 0
    sc_row capture cap-one realistic-baseline 1 0 2 0
    sc_row capture cap-one steelman-baseline  1 1 2 0
    sc_row capture cap-one memory-off         1 0 2 0
    sc_row capture cap-two memory-on          1 1 2 0 0 0 0 0 0 false 0 cap_summary_missing_fact 1 0
    sc_row capture cap-two realistic-baseline 1 0 2 0
    sc_row capture cap-two steelman-baseline  1 1 2 0
    sc_row capture cap-two memory-off         1 0 2 0
  } > "$caplow"
  local caplow_out; caplow_out="$(aggregate_scorecard "$caplow" 0.10 1)"
  if printf '%s' "$caplow_out" | grep -q '\-> capture:.*capture_fidelity_target=NO.*=> no'; then echo "ok: capture dimension fails when direct fidelity target is missed"; else echo "BUG: capture dimension passed despite low direct fidelity"; printf '%s\n' "$caplow_out"; fail=1; fi

  local caphigh="$tmp/caphigh.tsv"
  {
    sc_row capture cap-one memory-on          1 1 2 0 0 0 0 0 0 false 1 cap_ok 1 0
    sc_row capture cap-one realistic-baseline 1 0 2 0
    sc_row capture cap-one steelman-baseline  1 1 2 0
    sc_row capture cap-one memory-off         1 0 2 0
  } > "$caphigh"
  if aggregate_scorecard "$caphigh" 0.10 1 | grep -q '\-> capture:.*capture_fidelity_target=yes.*=> PASS'; then echo "ok: capture dimension passes when downstream and direct fidelity targets pass"; else echo "BUG: capture dimension did not pass with high direct fidelity"; aggregate_scorecard "$caphigh" 0.10 1; fail=1; fi

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

  # ADR-3 fail-closed must catch a PARTIAL collapse too: one non-errored run with
  # cost=0 among positive runs keeps the MEAN positive, yet the per-run guard must
  # still SKIP (the mean-only check would have wrongly RATIFIED here).
  local pcost="$tmp/pcost.tsv"
  {
    sc_row retrieval_scale a1 memory-on          1 1 3 0 0.02 600  4000 4400 100 false
    sc_row retrieval_scale a1 memory-on          2 1 3 0 0    600  4000 4400 100 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline  2 1 3 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0.01 6000 0    0    100 false
  } > "$pcost"
  local pout; pout="$(aggregate_scorecard "$pcost" 0.10 1)"
  if printf '%s' "$pout" | grep -qF 'SKIP cost verdict (zero/absent total_cost_usd'; then echo "ok: ADR-3 fails closed on a PARTIAL zero-cost run (mean stays positive)"; else echo "BUG: ADR-3 partial zero-cost skip"; printf '%s\n' "$pout"; fail=1; fi
  if printf '%s' "$pout" | grep -qF 'RATIFY Opt 3'; then echo "BUG: ADR-3 ratified despite a partial zero-cost run"; fail=1; else echo "ok: ADR-3 does not ratify with a partial zero-cost run"; fi

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

  # Session-log retention (Vikunja #502): preserve_session_logs copies the
  # diagnosable per-session artifacts (claude stream-json logs, judged text,
  # daemon logs) out of a workroot into a destination, preserving relative
  # paths — and copies NOTHING else (no store DBs, no seeded CLAUDE.md), so
  # the retained artifact stays small and log-shaped.
  local lw="$tmp/logs-workroot" ld="$tmp/logs-dest"
  mkdir -p "$lw/fresh-r1/on" "$lw/fresh-r1/realistic/p"
  printf '{"type":"result"}\n' > "$lw/fresh-r1/on/work.jsonl"
  printf 'plant\n'             > "$lw/fresh-r1/on/plant.jsonl"
  printf 'daemon\n'            > "$lw/fresh-r1/on/daemon.log"
  printf 'judged\n'            > "$lw/fresh-r1/realistic/judge.txt"
  printf 'db\n'                > "$lw/fresh-r1/on/memory.db"
  printf 'seeded\n'            > "$lw/fresh-r1/realistic/p/CLAUDE.md"
  if preserve_session_logs "$lw" "$ld"; then
    echo "ok: preserve_session_logs succeeds on a populated workroot"
  else
    echo "BUG: preserve_session_logs failed on a populated workroot"; fail=1
  fi
  for kept in fresh-r1/on/work.jsonl fresh-r1/on/plant.jsonl fresh-r1/on/daemon.log fresh-r1/realistic/judge.txt; do
    if [ -f "$ld/$kept" ]; then echo "ok: preserve_session_logs kept $kept"; else echo "BUG: preserve_session_logs lost $kept"; fail=1; fi
  done
  for dropped in fresh-r1/on/memory.db fresh-r1/realistic/p/CLAUDE.md; do
    if [ ! -e "$ld/$dropped" ]; then echo "ok: preserve_session_logs drops $dropped"; else echo "BUG: preserve_session_logs copied non-log $dropped"; fail=1; fi
  done
  # An empty workroot is not an error: retention must never break the run.
  local lwe="$tmp/logs-empty" lde="$tmp/logs-empty-dest"
  mkdir -p "$lwe"
  if preserve_session_logs "$lwe" "$lde" && [ -d "$lde" ]; then
    echo "ok: preserve_session_logs is a no-op on an empty workroot"
  else
    echo "BUG: preserve_session_logs failed on an empty workroot"; fail=1
  fi

  if [ "$fail" -eq 0 ]; then echo "self-test PASS"; return 0; fi
  echo "self-test FAIL" >&2; return 1
}

# ---- arg parsing -------------------------------------------------------------
MODE="run"; BIN_DIR=""; RUNS=""; OUT=""; MIN_RUNS=""; AGENT="claude-code"; PRE_SCORECARD_ROWS=""; LOG_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)      MODE="self-test"; shift ;;
    --agent)          AGENT="${2:?--agent needs a value}"
                      case "$AGENT" in claude-code|codex|opencode|gemini|hermes|all) ;; *) echo "--agent must be one of: claude-code, codex, opencode, gemini, hermes, all (got '$AGENT')" >&2; exit 2 ;; esac
                      shift 2 ;;
    --bin-dir)        BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)           RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --min-runs)       MIN_RUNS="${2:?--min-runs needs a value}"
                      case "$MIN_RUNS" in ''|*[!0-9]*|0) echo "--min-runs must be a positive integer (got '$MIN_RUNS')" >&2; exit 2 ;; esac
                      shift 2 ;;
    --out)            OUT="${2:?--out needs a value}"; shift 2 ;;
    --log-dir)        LOG_DIR="${2:?--log-dir needs a value}"; shift 2 ;;
    --scenarios-file) SCENARIOS_FILE="${2:?--scenarios-file needs a value}"; shift 2 ;;
    -h|--help)        awk 'NR>1 && /^#/ {print; next} NR>1 {exit}' "$0"; exit 2 ;;
    *)                echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi

if [ "$AGENT" = "all" ]; then
  echo "== memory-value scorecard agent target: all =="
  PRE_SCORECARD_ROWS="$(scorecard_skip_line codex; scorecard_skip_line opencode; scorecard_skip_line gemini; scorecard_skip_line hermes)"
  printf '%s\n' "$PRE_SCORECARD_ROWS"
  if [ -n "$OUT" ]; then
    printf '%s\n' "$PRE_SCORECARD_ROWS" > "$OUT"
  fi
  AGENT="claude-code"
elif ! scorecard_agent_supported "$AGENT"; then
  skip_line="$(scorecard_skip_line "$AGENT")"
  echo "== memory-value scorecard agent target: $AGENT =="
  printf '%s\n' "$skip_line"
  if [ -n "$OUT" ]; then
    printf '%s\n' "$skip_line" > "$OUT"
  fi
  exit 0
fi

# ---- live run prerequisites (the four-arm runner; needs API) -----------------
[ -n "$BIN_DIR" ] || BIN_DIR="$REPO_ROOT/target/release"
[ -d "$BIN_DIR" ] || { echo "bin dir not found: $BIN_DIR (build first with 'cargo build --release', or pass --bin-dir DIR)" >&2; exit 1; }
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
if [ -n "$PRE_SCORECARD_ROWS" ]; then
  printf '%s\n' "$PRE_SCORECARD_ROWS" >> "$RESULTS"
fi
# Session-log retention (Vikunja #502): --log-dir wins; RB_SCORECARD_KEEP_LOGS=1
# derives the dir from --out (fail fast without an anchor — logs written into
# the about-to-be-deleted workroot would be silently lost, the exact defect
# this fixes).
if [ -z "$LOG_DIR" ] && [ "${RB_SCORECARD_KEEP_LOGS:-0}" = "1" ]; then
  if [ -n "$OUT" ]; then
    LOG_DIR="$(cd "$(dirname "$OUT")" && pwd)/scorecard-session-logs"
  else
    echo "RB_SCORECARD_KEEP_LOGS=1 needs --out (to anchor the log dir) or an explicit --log-dir" >&2
    exit 2
  fi
fi
cleanup() {
  # Retention is best-effort by design: a copy failure must never mask the
  # run's own exit code (the safety gate) — but it runs BEFORE the rm.
  if [ -n "$LOG_DIR" ]; then
    preserve_session_logs "$WORKROOT" "$LOG_DIR" \
      || echo "WARN: session-log retention to $LOG_DIR failed (continuing cleanup)" >&2
  fi
  rm -rf "$WORKROOT" 2>/dev/null || true
}
trap cleanup EXIT

seed_home() { # home [proj]
  # claude >= 2.1.x ignores project settings (hooks, permissions.allow, MCP)
  # in a workspace with no recorded trust decision, and `-p` mode cannot show
  # the trust dialog — so a generated project must be pre-trusted in the
  # home's .claude.json (keyed by the project's REAL path; macOS tmpdirs
  # resolve /var -> /private/var).
  mkdir -p "$1"
  if [ -n "${2:-}" ]; then
    mkdir -p "$2"
    python3 - "$1/.claude.json" "$2" <<'PY'
import json, os, sys
path, proj = sys.argv[1], os.path.realpath(sys.argv[2])
json.dump(
    {"hasCompletedOnboarding": True,
     "projects": {proj: {"hasTrustDialogAccepted": True}}},
    open(path, "w"), indent=2)
PY
  else
    printf '{"hasCompletedOnboarding": true}\n' > "$1/.claude.json"
  fi
}

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

install_claude_hooks() { # home proj
  local home="$1" proj="$2"
  (
    export HOME="$home"; export PATH="$BIN_DIR:$PATH"; cd "$proj" || return 1
    rusty-brain-install install --agents claude-code >/dev/null
  )
}

install_rusty_brain() { # home proj
  local home="$1" proj="$2"
  install_claude_hooks "$home" "$proj"
  (
    export HOME="$home"; export PATH="$BIN_DIR:$PATH"; cd "$proj" || return 1
    python3 - "$proj/.claude/settings.json" "$proj/.mcp.json" "$BIN_DIR/rusty-brain" <<'PY'
import json, sys
sp, mp, cmd = sys.argv[1], sys.argv[2], sys.argv[3]
json.dump({"mcpServers": {"rusty-brain": {"command": cmd, "args": ["mcp"]}}}, open(mp, "w"), indent=2)
s = json.load(open(sp)); s["enableAllProjectMcpServers"] = True
json.dump(s, open(sp, "w"), indent=2)
PY
  )
}

install_rusty_brain_hooks_only() { # home proj
  local home="$1" proj="$2"
  install_claude_hooks "$home" "$proj"
  rm -f "$proj/.mcp.json"
  python3 - "$proj/.claude/settings.json" <<'PY'
import json, os, sys
p = sys.argv[1]
if not os.path.exists(p):
    raise SystemExit(0)
s = json.load(open(p))
s.pop("enableAllProjectMcpServers", None)
json.dump(s, open(p, "w"), indent=2)
PY
}

# Score one work session and append a scorecard row. The first 13 fields are
# stable; Class B capture metrics append four fields after them. The session runs
# under `--output-format stream-json --verbose` so the final result record
# carries num_turns, total_cost_usd, AND the session-aggregate cache buckets
# (ADR-3, docs/eval/2026-06-19-*); extract_usage reads them. The judged text is
# the model OUTPUT only: the final `.result` plus files written this session
# (mtime newer than a marker, size-capped) — never the seeded CLAUDE.md, so the
# baselines' planted distractors/target are not self-graded.
score_session() { # dim id arm run proj home work expect forbid stale [cap_fidelity cap_reason cap_summary_count cap_mcp_bypass_count]
  local dim="$1" id="$2" arm="$3" run="$4" proj="$5" home="$6" work="$7" expect="$8" forbid="$9" stale="${10}"
  local cap_fidelity="${11:-na}" cap_reason="${12:-na}" cap_summary_count="${13:-0}" cap_mcp_bypass_count="${14:-0}"
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
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$dim" "$id" "$arm" "$run" "$success" "$turns" "$mie" \
    "$cost" "$inp" "$cc" "$cr" "$out" "$is_err" \
    "$cap_fidelity" "$cap_reason" "$cap_summary_count" "$cap_mcp_bypass_count" >> "$RESULTS"
  local cap_msg=""
  if [ "$cap_fidelity" != "na" ]; then
    cap_msg=" cap=$cap_fidelity/$cap_reason summaries=$cap_summary_count mcp_bypass=$cap_mcp_bypass_count"
  fi
  echo "   [$dim/$id $arm r$run] success=$success turns=$turns cost=$cost mie=$mie$cap_msg"
}

# Explicit plant (P2): each fact is stored via `rusty-brain remember`, in array
# order, isolating retrieval from the lossy auto-capture path. A SUPERSEDED state
# (Class C: X later replaced by X') is expressed with `rusty-brain remember
# --supersedes <prior-id>` (cli.rs -> client.remember_superseding -> the engine's
# atomic supersede — the same path the SessionEnd hook uses); plant_explicit below
# threads the prior memory's id to drive it.
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

measure_capture_fidelity() { # home expect forbid
  local home="$1" expect="$2" forbid="$3"
  local out err result reason last=""
  # The SessionEnd summary is written asynchronously by the hook, so poll the
  # store until a definite verdict lands (CAP_POLL_MAX * CAP_POLL_SECS budget).
  local CAP_POLL_MAX=50 CAP_POLL_SECS=0.2
  out="$(mktemp "${TMPDIR:-/tmp}/rb-scorecard-list.XXXXXX.json")"
  err="$(mktemp "${TMPDIR:-/tmp}/rb-scorecard-list.XXXXXX.err")"
  for _ in $(seq 1 "$CAP_POLL_MAX"); do
    if ( export HOME="$home"; export PATH="$BIN_DIR:$PATH"; rusty-brain --json list --limit 1000 >"$out" 2>"$err" ); then
      result="$(capture_fidelity_from_json "$expect" "$forbid" "$out")"
      last="$result"
      reason="$(cut -f2 <<<"$result")"
      case "$reason" in
        cap_ok|cap_forbidden_token|cap_summary_missing_fact|cap_mcp_bypass_detected)
          rm -f "$out" "$err"
          printf '%s\n' "$result"
          return 0
          ;;
      esac
    else
      last="$(printf '0\tcap_list_error\t0\t0')"
    fi
    sleep "$CAP_POLL_SECS"
  done
  rm -f "$out" "$err"
  # Budget exhausted without a terminal verdict. If the store was readable but the
  # async SessionEnd summary never landed (cap_no_session_summary, or no poll ever
  # recorded a state), that is a real timeout; a persistent list failure keeps its
  # own cap_list_error so the report distinguishes "slow hook" from "broken store".
  case "$(cut -f2 <<<"$last")" in
    cap_no_session_summary|"") printf '0\tcap_timeout\t0\t0\n' ;;
    *)                         printf '%s\n' "$last" ;;
  esac
}

run_scenario() { # row
  local row="$1"
  local id dim plant_mode work expect forbid stale realistic steelman facts corpus plant_session capture_expect capture_forbid
  id="$(jq -r '.id' <<<"$row")"; dim="$(jq -r '.dimension' <<<"$row")"
  plant_mode="$(jq -r '.plant_mode' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"; expect="$(jq -r '.expect' <<<"$row")"
  forbid="$(jq -r '.forbid // ""' <<<"$row")"; stale="$(jq -r '.stale_token // ""' <<<"$row")"
  realistic="$(jq -r '.realistic_claude_md // ""' <<<"$row")"
  steelman="$(jq -r '.steelman_claude_md // ""' <<<"$row")"
  facts="$(jq -c '.plant // []' <<<"$row")"
  plant_session="$(jq -r '.plant_session // ""' <<<"$row")"
  capture_expect="$(jq -r '.capture_expect // .expect // ""' <<<"$row")"
  capture_forbid="$(jq -r '.capture_forbid // ""' <<<"$row")"
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
    # reach scores identity B (hb/pb) and uses the per-arm baseline homes, so the
    # shared work home/project here is only set up for the non-reach dimensions.
    local wh="$mb/hw" wp="$mb/wp"
    if [ "$dim" != "reach" ]; then
      seed_home "$wh" "$wp"
      install_rusty_brain "$wh" "$wp"; rm -f "$wp/CLAUDE.md"; rm -rf "$wp/.claude/skills"
    fi
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
      # bind failure. The log lives under the WORKROOT (not the short-lived
      # sockdir) so opt-in session-log retention (Vikunja #502) preserves it;
      # only the SOCKET needs the short /tmp path (unix socket length limits).
      mkdir -p "$mb"
      derr="$mb/daemon.log"
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
      local cap_fidelity="na" cap_reason="na" cap_summary_count="0" cap_mcp_bypass_count="0"
      if [ "$dim" = "reach" ]; then
        # Identity A plants from its home only (plant_explicit needs no project),
        # so pa is intentionally unused; only B gets a project to be scored in.
        local ha pa hb pb
        IFS=$'\t' read -r ha pa hb pb < <(reach_identity_paths "$mb")
        : "$pa" # tab-split placeholder; A needs no project dir
        seed_home "$ha"; seed_home "$hb" "$pb"
        install_rusty_brain "$hb" "$pb"; rm -f "$pb/CLAUDE.md"; rm -rf "$pb/.claude/skills"
        plant_explicit "$ha" "$facts"
        score_session "$dim" "$id" "memory-on" "$run" "$pb" "$hb" "$work" "$expect" "$forbid" "$stale"
      else
        if [ "$plant_mode" = "explicit" ]; then
          plant_explicit "$wh" "$facts"
          # Class A: bury the planted target under the distractor corpus (bulk-load).
          plant_corpus_distractors "$wh" "$id" "$corpus"
        elif [ "$plant_mode" = "auto-capture" ]; then
          local ph="$mb/hp" pp="$mb/pp" plog="$mb/plant.jsonl"
          seed_home "$ph" "$pp"
          install_rusty_brain_hooks_only "$ph" "$pp"; rm -f "$pp/CLAUDE.md"; rm -rf "$pp/.claude/skills"
          # Auto-capture plant: a real Claude Code session whose SessionEnd hook
          # (no MCP) is the only path that writes memory for this dimension.
          run_session "$ph" "$pp" "$plant_session" "$plog" --output-format stream-json --verbose
          IFS=$'\t' read -r cap_fidelity cap_reason cap_summary_count cap_mcp_bypass_count \
            < <(measure_capture_fidelity "$wh" "$capture_expect" "$capture_forbid")
          echo "   [$dim/$id memory-on r$run] capture_fidelity=$cap_fidelity reason=$cap_reason summaries=$cap_summary_count mcp_bypass=$cap_mcp_bypass_count"
        else
          echo "ERROR: unknown plant_mode '$plant_mode' for $id" >&2
          exit 1
        fi
        score_session "$dim" "$id" "memory-on" "$run" "$wp" "$wh" "$work" "$expect" "$forbid" "$stale" \
          "$cap_fidelity" "$cap_reason" "$cap_summary_count" "$cap_mcp_bypass_count"
      fi
    )
    rm -rf "$sockdir" 2>/dev/null || true

    # realistic-baseline + steelman-baseline + memory-off. For Class A the
    # distractor corpus is written into BOTH baselines' CLAUDE.md so the buried
    # target is the ONLY difference: steelman holds target + distractors (diligent
    # human), realistic holds distractors only (target omitted — the common
    # "nobody wrote it down" reality).
    local rb="$base/realistic"; seed_home "$rb/h" "$rb/p"
    write_claude_md "$rb/p/CLAUDE.md" "$realistic" "$distractors"
    score_session "$dim" "$id" "realistic-baseline" "$run" "$rb/p" "$rb/h" "$work" "$expect" "$forbid" "$stale"

    local sb="$base/steelman"; seed_home "$sb/h" "$sb/p"
    write_claude_md "$sb/p/CLAUDE.md" "$steelman" "$distractors"
    score_session "$dim" "$id" "steelman-baseline" "$run" "$sb/p" "$sb/h" "$work" "$expect" "$forbid" "$stale"

    local ob="$base/off"; seed_home "$ob/h" "$ob/p"
    score_session "$dim" "$id" "memory-off" "$run" "$ob/p" "$ob/h" "$work" "$expect" "$forbid" "$stale"

    run=$((run + 1))
  done
}

echo "== memory-value scorecard (agent=$AGENT, model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
# Read ALL scenarios into an array BEFORE running any (not streamed): a
# streaming `while read < <(jq)` shares fd 0 with the loop body, and a body
# command that consumes stdin (claude -p) would eat the remaining scenario lines.
scenario_rows=()
while IFS= read -r row; do scenario_rows+=("$row"); done < <(jq -c '.scenarios[]' "$SCENARIOS_FILE")
for row in "${scenario_rows[@]}"; do run_scenario "$row"; done
echo
echo "== scorecard =="
aggregate_scorecard "$RESULTS" "$TIE_MARGIN" "$MIN_RUNS"
