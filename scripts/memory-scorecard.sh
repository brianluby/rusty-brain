#!/usr/bin/env bash
# shellcheck disable=SC2030,SC2031
# Memory-value scorecard harness. It compares rusty-brain against the
# information channels where semantic memory differs from a hand-maintained
# `AGENTS.md`, using OMP's native extension surface:
#
#   arms per scenario:
#     memory-on          rusty-brain has the fact (planted per the scenario's mode)
#     realistic-baseline the AGENTS.md a real team would actually have (stale/partial/large)
#     steelman-baseline  a diligent human's AGENTS.md (clean/current/complete)
#     memory-off         nothing, no extension (the floor)
#     length-matched-placebo inert text through the same OMP prompt-time channel
#   a dimension's claim (P1): memory-on BEATS realistic AND at least TIES steelman.
#   the hard safety gate is the paired stale-injection causal proxy described below.
#
# Plant modes (P2): `explicit` (rusty-brain remember — isolates retrieval) vs
# `auto-capture` (a real SessionEnd fold — exercises capture). Retrieval/freshness
# dimensions plant explicitly; capture is its own dimension.
#
# This file ships the PURE scoring core (artifact assertion + paired scorecard
# aggregation) with a `--self-test` that needs NO API, plus the live five-arm runner.
# Injected sizes use a documented UTF-8 estimator, NOT exact model tokens.
# See docs/eval/scorecard-controls.md for pairing, artifacts, and limitations.
#
# Variance protocol (P3): success is reported as a Wilson 95% CI and turns as
# median [Q1-Q3]; a single run never gates. A run with any arm below --min-runs
# (default 5, or config.min_runs) is DIRECTIONAL ONLY — it still prints the
# scorecard but emits no SAFE/UNSAFE verdict and exits 0. The hard gate requires
# zero attributed MIEs and zero unassessable causal pairs in a >=min-runs run.
#
# Session-log retention keeps OMP JSON event streams plus harness-authored
# recall diagnostics. Retain them with `--log-dir DIR` or
# `RB_SCORECARD_KEEP_LOGS=1`.
#
# Usage:
#   memory-scorecard.sh --self-test                       # judge + aggregation math, no API
#   memory-scorecard.sh [--agent omp|codex|opencode|gemini|hermes|all] [--model OMP_MODEL] [--bin-dir DIR] [--runs N] [--min-runs N] [--out FILE] [--log-dir DIR] [--scenarios-file F]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS_FILE="$REPO_ROOT/crates/rb-eval/scorecard/memory_scorecard_scenarios.json"
MODEL="${RB_SCORECARD_OMP_MODEL:-@slow}"
MAX_SESSION_SECONDS="${RB_SCORECARD_SESSION_TIMEOUT_SECS:-300}"
# Steelman tie margin (P1): memory-on must come within this of the diligent-human
# baseline to count as a tie. A small margin absorbs haiku noise without letting a
# real regression pass.
TIE_MARGIN="${RB_SCORECARD_TIE_MARGIN:-0.10}"
# A failed/unparseable session scores this many turns — a worst-case sentinel,
# well above a normal task, so a failure can never make an arm look faster on the
# turns tiebreaker. Used by extract_usage (exercised by --self-test).
TURNS_FAIL_SENTINEL="${RB_SCORECARD_TURNS_FAIL_SENTINEL:-99}"

scorecard_agent_supported() { # agent
  [ "$1" = "omp" ]
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
      printf 'Gemini has an adapter, but the cross-agentic scorecard currently supports only OMP; Gemini scorecard support is not yet implemented.'
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

# ---- pure outcome scoring (exercised by --self-test) -------------------------
# judge_text <textfile> <expect> -> success
#   Compatibility-only fallback for scenarios without an artifact assertion.
judge_text() {
  local file="$1" expect="$2"
  local success=0
  grep -iqF -- "$expect" "$file" 2>/dev/null && success=1
  echo "$success"
}

# judge_outcome <project> <judged-text> <expect> <artifact-path> <exact> -> success
# An asserted scenario is judged solely by the exact delivered artifact. Assistant
# prose cannot rescue a missing or obsolete artifact. The legacy substring scorer
# remains only for non-safety scenarios that have not declared an artifact contract.
judge_outcome() {
  local project="$1" textfile="$2" expect="$3" artifact="$4" exact="$5"
  if [ -z "$artifact" ]; then judge_text "$textfile" "$expect"; return 0; fi
  python3 - "$project" "$artifact" "$exact" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
relative = Path(sys.argv[2])
expected = sys.argv[3]
if relative.is_absolute() or ".." in relative.parts:
    print(0)
    raise SystemExit
candidate = root / relative
try:
    if candidate.is_symlink() or not candidate.is_file():
        print(0)
        raise SystemExit
    resolved = candidate.resolve()
    resolved.relative_to(root)
    actual = candidate.read_text(encoding="utf-8").replace("\r\n", "\n")
except (OSError, UnicodeError, ValueError):
    print(0)
    raise SystemExit
if actual.endswith("\n"):
    actual = actual[:-1]
print(1 if actual == expected else 0)
PY
}

# ---- corpus generator (pure; deterministic; exercised by --self-test) --------
# gen_corpus <scenario_id> <n> [collision-guards-json] [target-content-json]
# prints n markdown-bullet distractor lines. Each Class A scenario has its own
# deterministic, same-domain candidate pool: HTTP crates, identifier types, or
# configuration/wire-format conventions. Expected/stale/forbidden tokens and
# explicit collision tokens are rejected case-insensitively; target facts are
# rejected both verbatim and by their backtick-delimited identifiers.
gen_corpus() {
  python3 - "$1" "$2" "${3:-[]}" "${4:-[]}" <<'PY'
import hashlib
import json
import random
import re
import sys

sid, n = sys.argv[1], int(sys.argv[2])
guards = [value for value in json.loads(sys.argv[3]) if isinstance(value, str) and value]
targets = [value for value in json.loads(sys.argv[4]) if isinstance(value, str) and value]
for target in targets:
    guards.extend(re.findall(r"`([^`]+)`", target))

adjectives = (
    "account", "admin", "audit", "billing", "catalog", "checkout", "customer",
    "delivery", "event", "gateway", "inventory", "ledger", "metrics", "order",
    "partner", "profile", "reporting", "search", "shipping", "worker",
)
nouns = ("adapter", "api", "client", "consumer", "daemon", "job", "service", "worker")
components = [f"{adjective}-{noun}" for adjective in adjectives for noun in nouns]

if sid == "scale-http-buried":
    alternatives = (
        ("hyper", "an async connection pool"),
        ("isahc", "libcurl-backed requests"),
        ("surf", "an async client facade"),
        ("awc", "Actix-native requests"),
        ("minreq", "minimal synchronous requests"),
        ("curl", "direct libcurl bindings"),
        ("attohttpc", "small synchronous requests"),
        ("http-client", "runtime-neutral requests"),
    )
    candidates = [
        f"Outbound HTTP convention: the `{component}` uses the `{crate_name}` crate for {purpose}."
        for component in components
        for crate_name, purpose in alternatives
    ]
elif sid == "scale-id-type-buried":
    alternatives = (
        ("Ksuid", "roughly time-sortable identifiers"),
        ("Cuid2", "collision-resistant application ids"),
        ("Nanoid", "compact URL-safe identifiers"),
        ("SnowflakeId", "distributed sortable identifiers"),
        ("ObjectId", "timestamp-prefixed document ids"),
        ("Xid", "compact globally unique identifiers"),
        ("TypeId", "typed UUID-compatible identifiers"),
        ("i64", "database-assigned numeric identifiers"),
    )
    candidates = [
        f"Identifier convention: the `{component}` record uses `{id_type}` for {reason}."
        for component in components
        for id_type, reason in alternatives
    ]
elif sid == "scale-wire-format-buried":
    alternatives = (
        ("CBOR", "ciborium"),
        ("Bincode", "bincode"),
        ("Protocol Buffers", "prost"),
        ("Apache Avro", "apache-avro"),
        ("Cap'n Proto", "capnp"),
        ("FlatBuffers", "flatbuffers"),
        ("RON", "ron"),
        ("Postcard", "postcard"),
    )
    config_names = (
        "codec.toml", "transport.toml", "protocol.toml", "serialization.toml",
        "socket.toml", "envelope.toml", "messages.toml", "ipc.toml",
    )
    candidates = [
        f"Wire-format convention: `{component}` uses {format_name} via `{library}`; its codec settings live in `config/{config_names[index % len(config_names)]}`."
        for component in components
        for index, (format_name, library) in enumerate(alternatives)
    ]
else:
    raise SystemExit(f"unsupported retrieval-scale scenario: {sid}")

blocked = tuple(value.casefold() for value in guards)
target_text = {value.casefold().strip() for value in targets}
eligible = [
    candidate for candidate in candidates
    if candidate.casefold().strip() not in target_text
    and not any(token in candidate.casefold() for token in blocked)
]
random.Random(int(hashlib.sha256(sid.encode()).hexdigest()[:12], 16)).shuffle(eligible)
if len(eligible) < n:
    raise SystemExit(
        f"{sid}: only {len(eligible)} collision-free same-domain candidates for requested corpus {n}"
    )
for candidate in eligible[:n]:
    print(f"- {candidate}")
PY
}

# A scale scenario is valid only when every explicitly planted target has the
# same importance as the generated competitor corpus.
scale_importance_valid() { # scenario-json
  jq -e '
    (.corpus_size // 0) == 0 or
    (((.plant // []) | length) > 0 and
      ((.corpus_importance // 5) as $importance |
        all(.plant[]; (.importance // 5) == $importance)))
  ' >/dev/null <<<"$1"
}

# ---- usage extraction (pure; ADR-3; exercised by --self-test) -----------------
# extract_usage <omp-json-log> -> is_error turns cost input cache_write cache_read output
# OMP's terminal `agent_end` contains every message produced by the run, not
# aggregate usage on its last assistant message. Sum all assistant calls; an
# incomplete terminal event, deadline, or provider error is a worst-case failure.
extract_usage() {
  local log="$1"
  local terminal is_err turns cost inp cc cr out
  terminal="$(jq -sc '[.[] | select(.type=="agent_end")][-1] // empty' "$log" 2>/dev/null || true)"
  if [ -n "$terminal" ]; then
    is_err="$(jq -r '
      [.messages[]? | select(.role=="assistant")] as $calls |
      ($calls | length) == 0 or
      ($calls[-1].stopReason != "stop" and $calls[-1].stopReason != "length") or
      any($calls[]; .stopReason == "error" or .stopReason == "aborted" or .usage == null)
    ' <<<"$terminal")"
    turns="$(jq -s '[.[] | select(.type=="turn_start")] | length' "$log" 2>/dev/null || printf 0)"
    IFS=$'\t' read -r cost inp cc cr out < <(jq -r '
      [.messages[]? | select(.role=="assistant") | .usage] as $usage |
      [([$usage[].cost.total // 0] | add // 0),
       ([$usage[].input // 0] | add // 0),
       ([$usage[].cacheWrite // 0] | add // 0),
       ([$usage[].cacheRead // 0] | add // 0),
       ([$usage[].output // 0] | add // 0)] | @tsv
    ' <<<"$terminal")
    [ "$is_err" = true ] && turns="$TURNS_FAIL_SENTINEL"
  else
    is_err=true; turns="$TURNS_FAIL_SENTINEL"; cost=0; inp=0; cc=0; cr=0; out=0
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$is_err" "$turns" "$cost" "$inp" "$cc" "$cr" "$out"
}

# ---- baseline AGENTS.md writer (pure; exercised by --self-test) --------------
# Write a baseline's AGENTS.md from a body (the realistic/steelman text) and an
# optional distractor corpus. Writes nothing when both are empty (so a Class A
# realistic arm with no body still gets distractors).
write_agents_md() { # file body distractors
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
# Reads an attributed results TSV. The first 13 fields are stable:
#   dimension scenario arm run success turns mie cost tok_in tok_cc tok_cr tok_out is_error
# Fields 14-17 are Class B capture diagnostics. Fields 18-22 are injection-size
# estimates. Fields 23-24 append answer-evidence booleans for SessionStart and
# prompt-time/query hook output. The causal postprocessor appends:
#   25 injected_stale_evidence (0|1|unknown|na)
#   26 injection_evidence_reason
#   27 causal_attribution (memory_induced|not_memory_induced|unassessable|na)
#   28 causal_reason
# Legacy rows still parse, but they cannot support query-source claims and a
# memory-on row without causal fields is unassessable and can never produce SAFE.
# Prints the per-dimension scorecard — success as a Wilson 95% CI, turns as
# median [Q1-Q3] (P3: median + spread, never a bare mean), and mean
# total_cost_usd — plus query-source attribution, cost diagnostics, and safety.
# Exit code: 0 for a DIRECTIONAL run (any arm below min_runs, or a missing arm —
# prints no SAFE/UNSAFE verdict, never gates); otherwise 0 iff the causal safety
# proxy has zero attributed MIEs and zero unassessable required pairs.
#
#   $1 = tsv, $2 = steelman tie margin (default $TIE_MARGIN), $3 = min_runs (default 5).
aggregate_scorecard() {
  local tsv="$1" tie_margin="${2:-${TIE_MARGIN:-0.10}}" min_runs="${3:-5}"
  echo "scoring: paired-stale-injection-causal-proxy; task outcome scorer is recorded per row; independent no-judge assertion fixture is reported separately."
  echo "causal_rule: memory_induced iff memory-on failed AND the same scenario/run memory-off succeeded AND stale evidence appears in the memory-on prompt-time receipt."
  echo "SAFE: every required pair and injection receipt is assessable and zero pairs satisfy causal_rule. UNSAFE: at least one attributed pair or one unassessable required pair."
  echo "limitations: before_agent_start receipts prove message preparation, not provider delivery/attention; paired nondeterministic runs are a causal proxy, not randomized causal proof."
  echo "injected_est: per-arm mean [start,prompt,file,total], utf8-bytes-div4-ceil-v1; not exact model tokens; excludes prompts/tool traffic/plant sessions. Legacy sizes are unknown."
  awk -F'\t' -v tie="$tie_margin" -v min_runs="$min_runs" '
    /^agent=/ { next }
    # cache-read fraction cr/(cr+in) — 0 when there is no input (ADR-3 diag).
    function ratio(cr, inp) { if (cr + inp == 0) return 0; return cr / (cr + inp) }
    # Never coerce absent historical measurements to zero.
    function add_inj(k) {
      inj_n[k]++;
      if (NF < 22 || $22 != "utf8-bytes-div4-ceil-v1") { inj_unknown[k]=1; return }
      inj_start[k]+=$18; inj_prompt[k]+=$19; inj_file[k]+=$20; inj_total[k]+=$21;
    }
    function injected(k) {
      if (inj_unknown[k] || inj_n[k]==0) return "injected_est[start=?,prompt=?,file=?,total=?]";
      return sprintf("injected_est[start=%.1f,prompt=%.1f,file=%.1f,total=%.1f]",
        inj_start[k]/inj_n[k], inj_prompt[k]/inj_n[k], inj_file[k]/inj_n[k], inj_total[k]/inj_n[k]);
    }
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
      start_answer=(NF >= 24 ? $23 : "na"); prompt_answer=(NF >= 24 ? $24 : "na");
      causal_attr=(NF >= 27 ? $27 : "unassessable");
      causal_reason=(NF >= 28 ? $28 : "missing_causal_fields");
      key = dim SUBSEP arm;
      n[key]++; s[key]+=succ; add_inj(key);
      co[key]+=cost; ci_in[key]+=inp; ci_cc[key]+=cc; ci_cr[key]+=cr;
      if (is_err == "true") err[key]++;
      # Track per-RUN cost collapse: a non-errored run with zero/absent
      # total_cost_usd. The mean can stay positive while one run collapsed the
      # cost axis, so the ADR-3 verdict must fail closed on ANY such run, not
      # just an all-zero mean.
      if (is_err != "true" && cost + 0 <= 0) bad_cost[key]++;
      tk = key SUBSEP n[key]; tv[tk] = turns;
      dims[dim]=1;
      if (arm=="memory-on") {
        causal_total++;
        causal_lines = causal_lines sprintf(
          "causal_attribution\tdimension=%s\tscenario=%s\trun=%s\tattribution=%s\treason=%s\tmie=%d\n",
          dim, $2, $4, causal_attr, causal_reason, mie+0);
        if (causal_attr=="memory_induced" && mie+0==1) {
          mie_total++;
          mie_list = mie_list sprintf("    - %s / %s run %s (%s)\n", dim, $2, $4, causal_reason);
        } else if (causal_attr=="unassessable" || causal_attr=="" ||
                   (causal_attr!="memory_induced" && causal_attr!="not_memory_induced") ||
                   (causal_attr=="memory_induced" && mie+0!=1) ||
                   (causal_attr=="not_memory_induced" && mie+0!=0)) {
          unassessable_total++;
          unassessable_list = unassessable_list sprintf(
            "    - %s / %s run %s (%s)\n", dim, $2, $4, causal_reason);
        }
      }
      if (dim=="retrieval_scale" && arm=="memory-on") {
        if (NF < 24 || (start_answer != "0" && start_answer != "1") ||
            (prompt_answer != "0" && prompt_answer != "1")) {
          scale_source_unknown++;
        } else {
          scale_source_n++;
          scale_start_present += start_answer;
          scale_prompt_present += prompt_answer;
          if (start_answer == 0 && prompt_answer == 1) {
            scale_query_only++;
            if (succ == 1) scale_query_success++;
          } else if (start_answer == 1 && succ == 1) {
            scale_startup_success++;
          } else if (succ == 1) {
            scale_unattributed_success++;
          }
        }
      }
      if (dim=="capture" && arm=="memory-on" && cap_fid != "" && cap_fid != "na") {
        csc=$2;
        cap_scenarios[csc]=1;
        cap_n[csc]++; cap_s[csc]+=cap_fid+0;
        add_inj("capture-scenario" SUBSEP csc); add_inj("capture-total");
        cap_summary_total[csc]+=cap_summary+0; cap_mcp_total[csc]+=cap_mcp+0;
        cap_reason_count[csc SUBSEP cap_reason]++;
        cap_total_n++; cap_total_s+=cap_fid+0;
        cap_total_summary+=cap_summary+0; cap_total_mcp+=cap_mcp+0;
        cap_reason_total[cap_reason]++;
      }
    }
    END {
      directional = 0;
      split("memory-on realistic-baseline steelman-baseline memory-off length-matched-placebo", order, " ");
      printf "%-15s %-18s %5s %16s %16s %9s\n", "dimension", "arm", "runs", "success [95% CI]", "med_turns [Q1-Q3]", "mcost$";
      pass_dims=0; total_dims=0;
      for (d in dims) {
        total_dims++;
        for (i=1;i<=5;i++) {
          a=order[i]; key=d SUBSEP a;
          if (n[key]==0) {
            directional = 1;   # missing arm = incomplete data → never gate (P3)
            printf "%-15s %-18s %5d %16s %16s %9s %s\n", d, a, 0, "n/a", "n/a", "n/a", injected(key); continue
          }
          rate[key]=s[key]/n[key];
          if (n[key] < min_runs) directional = 1;
          delete tarr; m=0;
          for (r=1;r<=n[key];r++){ m++; tarr[m]=tv[key SUBSEP r] }
          med = pctile(tarr, m, 0.5); q1 = pctile(tarr, m, 0.25); q3 = pctile(tarr, m, 0.75);
          wilson(s[key], n[key]);
          printf "%-15s %-18s %5d %5.0f%% [%.1f-%.1f] %8.1f [%.1f-%.1f] %9.4f %s\n",
                 d, a, n[key], rate[key]*100, wil_lo*100, wil_hi*100, med, q1, q3, co[key]/n[key], injected(key);
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
          } else if (d == "retrieval_scale") {
            source_complete = (scale_source_n == n[d SUBSEP "memory-on"]);
            query_evidenced = (source_complete && scale_query_success > 0);
            verdict = (beats_realistic && ties_steelman && query_evidenced) ? "PASS" : "no";
            if (beats_realistic && ties_steelman && query_evidenced) pass_dims++;
            printf "  -> %s: beats_realistic=%s ties_steelman=%s query_retrieval_evidenced=%s  => %s\n\n",
                   d, (beats_realistic?"yes":"NO"), (ties_steelman?"yes":"NO"), (query_evidenced?"yes":"NO"), verdict;
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
      printf "scorecard: dimensions_passed=%d dimensions_evaluated=%d (tracked, non-gating)\n", pass_dims, total_dims;

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
          printf "%-28s %5d %5.0f%% [%.1f-%.1f] %9d %10d %s memory-on %s\n",
                 csc, cap_n[csc], 100*cap_s[csc]/cap_n[csc], wil_lo*100, wil_hi*100,
                 cap_summary_total[csc], cap_mcp_total[csc], reasons, injected("capture-scenario" SUBSEP csc);
        }
        wilson(cap_total_s, cap_total_n);
        target_met = (cap_total_s / cap_total_n >= 0.80);
        reasons = ""; sep = "";
        for (ri=1; ri<=7; ri++) {
          rr=reason_order[ri]; rc=cap_reason_total[rr]+0;
          if (rc > 0) { reasons = reasons sep rr "=" rc; sep = "," }
        }
        if (reasons == "") reasons = "none";
        printf "capture fidelity: %.0f%% [%.1f-%.1f] (%d/%d) target>=80%%=%s summaries=%d mcp_bypass=%d reasons=%s memory-on %s\n",
               100*cap_total_s/cap_total_n, wil_lo*100, wil_hi*100, cap_total_s, cap_total_n,
               (target_met?"yes":"NO"), cap_total_summary, cap_total_mcp, reasons, injected("capture-total");
      }

      # --- ADR-3 retrieval@scale token/cost (docs/eval/2026-06-19-*) ----------
      # Query-attributed accuracy is the PRIMARY axis (the dimension verdict
      # above); response accuracy without source evidence is diagnostic only.
      # Cost (total_cost_usd, cache-adjusted by construction) is secondary; cache
      # buckets are diagnostic and never a pass/fail metric. This block remains
      # informational; the hard gate is the paired stale-injection causal proxy.
      has_scale = 0; for (d in dims) if (d=="retrieval_scale") has_scale=1;
      if (has_scale) {
        rk = "retrieval_scale";
        printf "\n== ADR-3 retrieval@scale token/cost (query-specific evidence required; memory-on vs steelman-baseline) ==\n";
        printf "%-20s %5s %6s %9s %10s %10s %10s %7s %9s %9s\n",
               "arm", "runs", "succ", "mcost$", "m_input", "m_ccrea", "m_cread", "cache%", "ctx_vol", "eff_in";
        for (i=1;i<=5;i++) {
          a=order[i]; key=rk SUBSEP a;
          if (n[key]==0) continue;
          mc=co[key]/n[key]; mi=ci_in[key]/n[key]; mcc=ci_cc[key]/n[key]; mcr=ci_cr[key]/n[key];
          # ctx_vol = in+cc+cr (context-window pressure); eff_in = in+1.25*cc+0.1*cr
          # (cache-weighted full-price input equivalents) — both diagnostic.
          printf "%-20s %5d %5.0f%% %9.4f %10.0f %10.0f %10.0f %6.1f%% %9.0f %9.0f %s\n",
                 a, n[key], 100*rate[key], mc, mi, mcc, mcr,
                 100*ratio(ci_cr[key], ci_in[key]), mi+mcc+mcr, mi+1.25*mcc+0.1*mcr, injected(key);
        }
        if (scale_source_n == n[rk SUBSEP "memory-on"]) {
          printf "  -> query-source evidence: session_start_present=%d/%d prompt_recall_present=%d/%d query_only=%d/%d\n",
                 scale_start_present, scale_source_n, scale_prompt_present, scale_source_n,
                 scale_query_only, scale_source_n;
          printf "     query-retrieval-specific successes=%d/%d startup-context successes=%d/%d unattributed successes=%d/%d\n",
                 scale_query_success, scale_source_n, scale_startup_success, scale_source_n,
                 scale_unattributed_success, scale_source_n;
        } else {
          printf "  -> query-source evidence: UNMEASURED (%d/%d memory-on rows have source fields; legacy rows cannot support a retrieval claim)\n",
                 scale_source_n, n[rk SUBSEP "memory-on"];
        }
        onk=rk SUBSEP "memory-on"; stl=rk SUBSEP "steelman-baseline";
        on_cost=(n[onk]>0 ? co[onk]/n[onk] : 0); stl_cost=(n[stl]>0 ? co[stl]/n[stl] : 0);
        if (n[onk]==0 || n[stl]==0) {
          printf "  -> retrieval_scale: SKIP cost verdict (memory-on or steelman-baseline produced no runs)\n";
        } else if (scale_source_n != n[onk]) {
          printf "  -> retrieval_scale: SKIP cost verdict (query-source evidence incomplete; response accuracy is not proof of retrieval)\n";
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
          # A scale accuracy claim needs both observed answers and at least one
          # successful query-only row; startup-provided answers do not prove
          # discriminating retrieval among the competitor corpus.
          acc_ok = (on_acc >= stl_acc) && (s[onk] > 0 && s[stl] > 0) && (scale_query_success > 0);
          # ADR-3 cost axis is total_cost_usd; "within 20%" => on <= 1.2*stl
          # (both costs are guaranteed > 0 by the fail-closed guard above).
          cost_ok = (on_cost <= 1.2 * stl_cost);
          if (acc_ok && cost_ok)        v="RATIFY Opt 3 (query-specific accuracy >= steelman AND total_cost_usd within 20%)";
          else if (acc_ok && !cost_ok)  v="Opt 2 candidate (query-specific accuracy wins; cost > 20% worse)";
          else if (!acc_ok && cost_ok)  v="query retrieval loses / is not evidenced at scale (cost fine)";
          else                          v="descope token-cost axis (value is capture/freshness/reach per ADR-1)";
          printf "  -> retrieval_scale: response acc on %.2f (%s) vs stl %.2f (%s) | query-specific successes %d/%d [%s] | cost$ on %.4f vs stl %.4f [%s]\n",
                 on_acc, injected(onk), stl_acc, injected(stl), scale_query_success, scale_source_n,
                 (acc_ok?"ok":"NO"), on_cost, stl_cost, (cost_ok?"ok":"NO");
          printf "     => %s\n", v;
        }
      }

      printf "%s", causal_lines;
      printf "SAFETY — paired causal proxy: memory_induced=%d unassessable=%d (allowed 0/0)\n",
             mie_total+0, unassessable_total+0;
      if (mie_total+0 > 0) { printf "  attributed:\n%s", mie_list }
      if (unassessable_total+0 > 0) { printf "  unassessable (fails closed):\n%s", unassessable_list }
      if (directional) {
        printf "DIRECTIONAL ONLY — an arm is below min_runs=%d; single runs never gate (P3)\n", min_runs;
        printf "result: DIRECTIONAL (not gating)\n";
        exit 0;
      }
      safety_fail = (mie_total+0 > 0 || unassessable_total+0 > 0);
      printf "result: %s\n", (safety_fail ? "UNSAFE" : "SAFE");
      exit (safety_fail ? 1 : 0);
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
    # The decided value was captured. A correct summary may name the rejected
    # alternative explicitly, so expect-present wins for capture fidelity.
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


# Capture the exact read-side evidence a scorecard work session is about to see.
# SessionStart and prompt-time hook outputs are retained separately so Class A
# can distinguish an answer already present at startup from one surfaced by the
# query-specific recall path. These probes are read-only and separate from the
# extension's actual per-session receipt.
capture_memory_diagnostics() { # home project query diagnostics_dir planted_jsonl
  local home="$1" project="$2" query="$3" dir="$4" planted="$5"
  mkdir -p "$dir"
  [ -f "$planted" ] || : > "$planted"
  (
    export HOME="$home" PATH="$BIN_DIR:$PATH"
    # These read-only probes never call a model or expose provider credentials.
    unset ANTHROPIC_API_KEY OPENAI_API_KEY GEMINI_API_KEY
    # Probe startup before any query path can refresh a target's access time.
    rusty-brain --json context > "$dir/context.json"
    local sid="scorecard-diagnostic"
    jq -cn --arg sid "$sid" --arg cwd "$project" \
      '{type:"session_start",source:"startup",session_id:$sid,cwd:$cwd}' \
      > "$dir/session-start-input.json"
    rusty-brain-hooks --agent omp \
      < "$dir/session-start-input.json" > "$dir/session-start-injection.json"

    jq -cn --arg sid "$sid" --arg cwd "$project" --arg prompt "$query" \
      '{type:"prompt",session_id:$sid,cwd:$cwd,prompt:$prompt}' \
      > "$dir/prompt-time-input.json"
    rusty-brain-hooks --agent omp \
      < "$dir/prompt-time-input.json" > "$dir/prompt-time-injection.json"
    # Candidate snapshots follow the hook probes so they cannot influence either
    # source classification through access-time refresh.
    rusty-brain --json recall "$query" --limit 5 > "$dir/recall-active.json"
    rusty-brain --json recall "$query" --limit 5 --archived > "$dir/recall-archived.json"

    # History every planted or ranked candidate. Anchoring on both an archived
    # predecessor and its active head makes the full supersede-chain ids
    # durable even if one side did not rank for this exact query.
    {
      jq -r '.id // empty' "$planted"
      jq -r '.[].memory.id // empty' "$dir/recall-active.json"
      jq -r '.[].memory.id // empty' "$dir/recall-archived.json"
    } | sort -u | while IFS= read -r memory_id; do
      [ -n "$memory_id" ] || continue
      rusty-brain --json history "$memory_id" > "$dir/history-$memory_id.json"
    done

    jq -n --arg query "$query" --arg namespace "${RUSTY_BRAIN_NAMESPACE:-}" \
      '{schema_version:3,query:$query,namespace:$namespace,recall_limit:5,
        evidence:{context:"context.json",active_candidates:"recall-active.json",
        archived_candidates:"recall-archived.json",planted_chain:"planted.jsonl",
        session_start_input:"session-start-input.json",
        session_start_injection:"session-start-injection.json",
        prompt_time_input:"prompt-time-input.json",
        prompt_time_injection:"prompt-time-injection.json",
        histories:"history-<memory-id>.json"}}' > "$dir/index.json"
  )
}

# Add scored outcome, source attribution, and receipt evidence to retained
# diagnostics. `scorecard-controls.py attribute-results` adds the paired
# memory-off outcome and final causal attribution after every arm completes.
finalize_memory_diagnostics() { # dir success stale_evidence evidence_reason expect stale -> start<TAB>prompt
  local dir="$1" success="$2" stale_evidence="$3" evidence_reason="$4" expect="$5" stale="$6"
  [ -d "$dir" ] || { printf 'na\tna\n'; return 0; }
  local startup prompt probe_stale=0 probe_current=0 probe_any=0
  local startup_current_present=0 prompt_current_present=0
  startup="$(jq -r '.message // ""' "$dir/session-start-injection.json" 2>/dev/null || true)"
  prompt="$(jq -r '.message // ""' "$dir/prompt-time-injection.json" 2>/dev/null || true)"
  [ -z "$prompt" ] || probe_any=1
  if [ -n "$stale" ] && grep -qiF -- "$stale" <<<"$prompt"; then probe_stale=1; fi
  if [ -n "$expect" ] && grep -qiF -- "$expect" <<<"$prompt"; then probe_current=1; fi
  if [ -n "$expect" ] && grep -qiF -- "$expect" <<<"$startup"; then startup_current_present=1; fi
  if [ -n "$expect" ] && grep -qiF -- "$expect" <<<"$prompt"; then prompt_current_present=1; fi
  jq -n --argjson success "$success" --arg stale_evidence "$stale_evidence" \
    --arg evidence_reason "$evidence_reason" \
    --argjson probe_stale "$probe_stale" --argjson probe_current "$probe_current" \
    --argjson probe_any "$probe_any" \
    --argjson startup_current_present "$startup_current_present" \
    --argjson prompt_current_present "$prompt_current_present" \
    '{success:$success,memory_induced_error:false,
      causal_attribution:"unassessable",causal_reason:"pending_memory_off_pair",
      injected_stale_evidence:
        (if $stale_evidence == "1" then true
         elif $stale_evidence == "0" then false else null end),
      injection_evidence_reason:$evidence_reason,
      diagnostic_probe_stale_present:($probe_stale == 1),
      diagnostic_probe_current_present:($probe_current == 1),
      diagnostic_probe_any_memory:($probe_any == 1),
      session_start_answer_present:($startup_current_present == 1),
      prompt_recall_answer_present:($prompt_current_present == 1),
      query_only_answer_evidence:($startup_current_present == 0 and $prompt_current_present == 1)}' \
    > "$dir/outcome.json"
  printf '%s\t%s\n' "$startup_current_present" "$prompt_current_present"
}

# Strict allowlist for retained scorecard artifacts. Diagnostic names are
# accepted only below an `on/diagnostics` directory, preventing a model-created
# arbitrary file elsewhere in the work project from entering the CI artifact.
retainable_scorecard_artifact() { # relative_path
  local rel="$1"
  # Fixed-depth regexes matter: shell case globs let `*` match `/`, so a path
  # like `fresh-r1/on/wp/on/diagnostics/index.json` (inside the model-writable
  # project) would otherwise masquerade as a harness-owned artifact.
  if [[ "$rel" =~ ^[^/]+/(on|realistic|steelman|off|placebo)/(work\.jsonl|judge\.txt)$ ]]; then
    return 0
  fi
  if [[ "$rel" =~ ^[^/]+/(on|placebo)/injections/(prompt-time\.jsonl|pair-check\.json|control-error\.json)$ ]]; then
    return 0
  fi
  if [[ "$rel" =~ ^[^/]+/on/(plant\.jsonl|daemon\.log)$ ]]; then
    return 0
  fi
  if [[ "$rel" =~ ^[^/]+/on/diagnostics/(index\.json|outcome\.json|context\.json|recall-active\.json|recall-archived\.json|planted\.jsonl|session-start-input\.json|session-start-injection\.json|prompt-time-input\.json|prompt-time-injection\.json)$ ]]; then
    return 0
  fi
  if [[ "$rel" =~ ^[^/]+/on/diagnostics/history-[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}\.json$ ]]; then
    return 0
  fi
  return 1
}

# ---- session-log retention (pure; exercised by --self-test) -------------------
# preserve_session_logs <workroot> <dest>: copy the diagnosable per-session
# artifacts out of the ephemeral workroot into <dest>, preserving relative
# paths, before cleanup deletes the workroot (Vikunja #502: the 2026-07-12 N=5
# safety-gate MIEs were undiagnosable because every session log was rm -rf'd).
# Copies only the harness-authored OMP JSON logs (work.jsonl / plant.jsonl),
# judged model output, memory-on daemon log, and read-only diagnostics — never
# stores DBs, seeded AGENTS.md files, or arbitrary model-created JSONL. Provider
# credentials are neither copied nor passed to the memory daemon.
preserve_session_logs() { # workroot dest
  local workroot="$1" dest="$2" failed=0
  mkdir -p "$dest"
  local f rel
  while IFS= read -r -d '' f; do
    rel="${f#"$workroot"/}"
    retainable_scorecard_artifact "$rel" || continue
    if ! { mkdir -p "$dest/$(dirname "$rel")" && cp "$f" "$dest/$rel"; } 2>/dev/null; then
      echo "WARN: session-log retention failed to copy $rel" >&2
      failed=1
    fi
  done < <(find "$workroot" -type f -print0)
  return "$failed"
}

# resolve_log_dir <log_dir_flag> <keep_logs_env> <out> -> echoes the retention
# dir ('' = retention off). Pure so --self-test covers the wiring. --log-dir
# wins; RB_SCORECARD_KEEP_LOGS=1 derives <dir of --out>/scorecard-session-logs.
# FAILS CLOSED (PR #70 CodeRabbit) when keep-logs has no --out anchor or the
# --out directory does not exist: a silent empty `cd` substitution would have
# resolved to /scorecard-session-logs at the filesystem root.
resolve_log_dir() { # log_dir_flag keep_logs_env out
  local flag="$1" keep="$2" out="$3"
  if [ -n "$flag" ]; then printf '%s\n' "$flag"; return 0; fi
  if [ "$keep" != "1" ]; then printf '\n'; return 0; fi
  if [ -z "$out" ]; then
    echo "RB_SCORECARD_KEEP_LOGS=1 needs --out (to anchor the log dir) or an explicit --log-dir" >&2
    return 2
  fi
  local out_dir
  out_dir="$(dirname "$out")"
  if [ ! -d "$out_dir" ]; then
    echo "RB_SCORECARD_KEEP_LOGS=1: --out directory does not exist: $out_dir" >&2
    return 2
  fi
  printf '%s/scorecard-session-logs\n' "$(cd "$out_dir" && pwd)"
}

# ---- self-test (no API) ------------------------------------------------------
self_test() {
  echo "== memory-scorecard self-test (judge + scorecard math; no API) =="
  local tmp fail=0
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/rb-scorecard-selftest.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  mkdir -p "$tmp/task"
  printf 'I used the current ureq value in prose.\n' > "$tmp/task/judge.txt"
  printf 'reqwest\n' > "$tmp/task/answer.txt"

  check() { if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "BUG: $1 (want '$2' got '$3')"; fail=1; fi; }
  check "exact artifact rejects current-value prose plus obsolete delivery" "0" \
    "$(judge_outcome "$tmp/task" "$tmp/task/judge.txt" ureq answer.txt ureq)"
  printf 'ureq\n' > "$tmp/task/answer.txt"
  check "exact artifact accepts the delivered current value" "1" \
    "$(judge_outcome "$tmp/task" "$tmp/task/judge.txt" ureq answer.txt ureq)"
  check "legacy non-safety fallback remains parse-compatible" "1" \
    "$(judge_outcome "$tmp/task" "$tmp/task/judge.txt" current "" "")"

  # Emit one attributed results row. The first 13 fields remain stable; capture
  # fields are 14-17, injection sizes 18-22, and causal evidence/attribution 23-26.
  # sc_row dim scenario arm run success turns mie [cost in cc cr out is_err cap_fid cap_reason cap_summary cap_mcp ...]
  sc_row() {
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$1" "$2" "$3" "$4" "$5" "$6" "$7" "${8:-0}" "${9:-0}" "${10:-0}" "${11:-0}" "${12:-0}" "${13:-false}" \
      "${14:-na}" "${15:-na}" "${16:-0}" "${17:-0}" \
      "${18:-0}" "${19:-0}" "${20:-0}" "${21:-0}" "${22:-utf8-bytes-div4-ceil-v1}" \
      "${23:-na}" "${24:-no_stale_token}" "${25:-not_memory_induced}" "${26:-no_stale_candidate}"
  }
  sc_causal_row() { # first 7 stable fields, then stale evidence/reason + attribution/reason
    sc_row "$1" "$2" "$3" "$4" "$5" "$6" "$7" \
      0 0 0 0 0 false na na 0 0 0 0 0 0 utf8-bytes-div4-ceil-v1 \
      "$8" "$9" "${10}" "${11}"
  }
  legacy_row() { # original 13-field schema
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t0\t0\t0\t0\t0\tfalse\n' \
      "$1" "$2" "$3" "$4" "$5" "$6" "$7"
  }
  # source_row start-answer prompt-answer <sc_row args...>
  source_row() {
    local start_answer="$1" prompt_answer="$2"
    shift 2
    printf '%s\t0\t0\t0\t0\tunknown\t%s\t%s\n' \
      "$(sc_row "$@")" "$start_answer" "$prompt_answer"
  }

  # Class A corpora are deterministic, domain-plausible, collision-free, and
  # keep target/distractor importance equal in every committed scale scenario.
  local ga gb scale_rows=0
  local scenarios="$REPO_ROOT/crates/rb-eval/scorecard/memory_scorecard_scenarios.json"
  while IFS= read -r scale_row; do
    scale_rows=$((scale_rows + 1))
    local scale_id guards targets marker
    scale_id="$(jq -r '.id' <<<"$scale_row")"
    guards="$(jq -c '[.expect?, .stale_token?, .forbid?] + (.distractor_collision_tokens // []) | map(select(type == "string" and length > 0))' <<<"$scale_row")"
    targets="$(jq -c '[.plant[]?.content]' <<<"$scale_row")"
    case "$scale_id" in
      scale-http-buried)        marker="Outbound HTTP convention:" ;;
      scale-id-type-buried)     marker="Identifier convention:" ;;
      scale-wire-format-buried) marker="Wire-format convention:" ;;
      *) echo "BUG: unknown scale scenario $scale_id"; fail=1; continue ;;
    esac
    ga="$(gen_corpus "$scale_id" 12 "$guards" "$targets")"
    gb="$(gen_corpus "$scale_id" 12 "$guards" "$targets")"
    check "$scale_id corpus deterministic" "$ga" "$gb"
    check "$scale_id corpus emits N lines" "12" "$(grep -c . <<<"$ga")"
    if grep -qF "$marker" <<<"$ga"; then echo "ok: $scale_id competitors are same-domain"; else echo "BUG: $scale_id competitors are not domain-plausible"; fail=1; fi
    local collision=0 token
    while IFS= read -r token; do
      if [ -n "$token" ] && grep -qiF -- "$token" <<<"$ga"; then collision=1; fi
    done < <(jq -r '.[]' <<<"$guards")
    if [ "$collision" -eq 0 ]; then echo "ok: $scale_id guards expected/stale/forbidden/target tokens"; else echo "BUG: $scale_id corpus collision"; fail=1; fi
    if scale_importance_valid "$scale_row"; then echo "ok: $scale_id target and distractors have equal importance"; else echo "BUG: $scale_id importance mismatch"; fail=1; fi
  done < <(jq -c '.scenarios[] | select(.dimension == "retrieval_scale")' "$scenarios")
  check "exactly three retrieval-scale scenarios" "3" "$scale_rows"
  if scale_importance_valid '{"corpus_size":1,"corpus_importance":5,"plant":[{"content":"target","importance":8}]}'; then
    echo "BUG: unequal scale importance accepted"; fail=1
  else
    echo "ok: unequal scale importance rejected"
  fi

  # A Class A realistic arm gets competitors only; the steelman gets target +
  # competitors; both-empty writes no file.
  local wd="$tmp/wcm"; mkdir -p "$wd"
  local http_guards='["ureq","reqwest"]'
  write_agents_md "$wd/realistic.md" "" "$(gen_corpus scale-http-buried 2 "$http_guards" '[]')"
  write_agents_md "$wd/steelman.md" "HTTP: use the \`ureq\` crate." "$(gen_corpus scale-http-buried 2 "$http_guards" '[]')"
  write_agents_md "$wd/none.md" "" ""
  if grep -qF 'Outbound HTTP convention:' "$wd/realistic.md" && ! grep -qiF ureq "$wd/realistic.md"; then echo "ok: write_agents_md realistic = competitors only (target omitted)"; else echo "BUG: realistic AGENTS.md"; fail=1; fi
  if grep -qiF ureq "$wd/steelman.md" && grep -qF 'Outbound HTTP convention:' "$wd/steelman.md"; then echo "ok: write_agents_md steelman = target + competitors"; else echo "BUG: steelman AGENTS.md"; fail=1; fi
  if [ ! -f "$wd/none.md" ]; then echo "ok: write_agents_md writes nothing when body + distractors empty"; else echo "BUG: empty write_agents_md created a file"; fail=1; fi

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
  if scorecard_agent_supported omp && ! scorecard_agent_supported codex; then
    echo "ok: only omp is currently scorecard-supported"
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

expected_dimension_counts = {
    "freshness": 4,
    "retrieval_scale": 3,
    "capture": 3,
    "reach": 3,
}
actual_dimension_counts = {
    dimension: sum(1 for s in scenarios if s.get("dimension") == dimension)
    for dimension in expected_dimension_counts
}
if actual_dimension_counts != expected_dimension_counts:
    errors.append(
        f"legacy scenario parity mismatch: expected {expected_dimension_counts}, "
        f"got {actual_dimension_counts}"
    )

freshness = [s for s in scenarios if s.get("dimension") == "freshness"]
for s in freshness:
    sid = s.get("id", "<missing>")
    assertion = s.get("outcome_assertion")
    if not isinstance(assertion, dict):
        errors.append(f"{sid}: stale scenario needs an exact artifact assertion")
        continue
    artifact = assertion.get("path")
    exact = assertion.get("exact")
    if not isinstance(artifact, str) or not artifact or artifact.startswith("/") or ".." in artifact.split("/"):
        errors.append(f"{sid}: assertion path must be a safe relative path")
    if not isinstance(exact, str) or not exact:
        errors.append(f"{sid}: assertion exact value must be non-empty")
    if artifact and artifact not in str(s.get("work") or ""):
        errors.append(f"{sid}: work prompt must name the asserted artifact")

for s in scenarios:
    if "forbid" in s:
        errors.append(f"{s.get('id', '<missing>')}: obsolete outcome forbid predicate must not be present")

captures = [s for s in scenarios if s.get("dimension") == "capture"]
if len(captures) != 3:
    errors.append(f"need exactly 3 capture scenarios, got {len(captures)}")
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
if len(reaches) != 3:
    errors.append(f"need exactly 3 reach scenarios, got {len(reaches)}")
for s in reaches:
    sid = s.get("id", "<missing>")
    expect = str(s.get("expect") or "").lower()
    realistic = str(s.get("realistic_agents_md") or "").lower()
    steelman = str(s.get("steelman_agents_md") or "").lower()
    if s.get("plant_mode") != "explicit":
        errors.append(f"{sid}: reach scenario must use explicit plant")
    plant = s.get("plant")
    if not isinstance(plant, list) or not plant:
        errors.append(f"{sid}: reach scenario needs non-empty plant array")
    if expect and expect in realistic:
        errors.append(f"{sid}: realistic AGENTS.md leaks expect token")
    if expect and expect not in steelman:
        errors.append(f"{sid}: steelman AGENTS.md must contain expect token")

if errors:
    for e in errors:
        print("BUG:", e)
    raise SystemExit(1)
print("ok: scenario contracts preserve all legacy OMP scorecard dimensions")
PY
  then
    :
  else
    fail=1
  fi

  # extract_usage reads OMP's terminal agent_end usage and counts turn_start.
  local elog="$tmp/work.jsonl"
  {
    printf '{"type":"turn_start"}\n'
    printf '{"type":"turn_start"}\n'
    printf '{"type":"agent_end","messages":[{"role":"assistant","stopReason":"toolUse","usage":{"input":1000,"cacheWrite":4000,"cacheRead":2000,"output":100,"cost":{"total":0.01}}},{"role":"toolResult","content":[]},{"role":"assistant","stopReason":"stop","usage":{"input":560,"cacheWrite":0,"cacheRead":3400,"output":60,"cost":{"total":0.0023}}}]}\n'
  } > "$elog"
  check "extract_usage sums every assistant call" "$(printf 'false\t2\t0.0123\t1560\t4000\t5400\t160')" "$(extract_usage "$elog")"
  : > "$tmp/empty.jsonl"
  check "extract_usage sentinel on empty log" "$(printf 'true\t%s\t0\t0\t0\t0\t0' "$TURNS_FAIL_SENTINEL")" "$(extract_usage "$tmp/empty.jsonl")"
  # A terminal OMP provider error gets the worst-case turn sentinel.
  printf '{"type":"agent_end","messages":[{"role":"assistant","stopReason":"error","usage":{"input":0,"cacheWrite":0,"cacheRead":0,"output":0,"cost":{"total":0}}}]}\n' > "$tmp/error-result.jsonl"
  check "extract_usage sentinel on terminal OMP error" "$(printf 'true\t%s\t0\t0\t0\t0\t0' "$TURNS_FAIL_SENTINEL")" "$(extract_usage "$tmp/error-result.jsonl")"

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
    sc_row freshness s1 length-matched-placebo         1 0 2 0
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
    sc_row capture cap-one length-matched-placebo         1 0 2 0
    sc_row capture cap-two memory-on          1 0 2 0 0 0 0 0 0 false 0 cap_summary_missing_fact 1 0
    sc_row capture cap-two realistic-baseline 1 0 2 0
    sc_row capture cap-two steelman-baseline  1 1 2 0
    sc_row capture cap-two memory-off         1 0 2 0
    sc_row capture cap-two length-matched-placebo         1 0 2 0
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
    sc_row capture cap-one length-matched-placebo         1 0 2 0
    sc_row capture cap-two memory-on          1 1 2 0 0 0 0 0 0 false 0 cap_summary_missing_fact 1 0
    sc_row capture cap-two realistic-baseline 1 0 2 0
    sc_row capture cap-two steelman-baseline  1 1 2 0
    sc_row capture cap-two memory-off         1 0 2 0
    sc_row capture cap-two length-matched-placebo         1 0 2 0
  } > "$caplow"
  local caplow_out; caplow_out="$(aggregate_scorecard "$caplow" 0.10 1)"
  if printf '%s' "$caplow_out" | grep -q '\-> capture:.*capture_fidelity_target=NO.*=> no'; then echo "ok: capture dimension fails when direct fidelity target is missed"; else echo "BUG: capture dimension passed despite low direct fidelity"; printf '%s\n' "$caplow_out"; fail=1; fi

  local caphigh="$tmp/caphigh.tsv"
  {
    sc_row capture cap-one memory-on          1 1 2 0 0 0 0 0 0 false 1 cap_ok 1 0
    sc_row capture cap-one realistic-baseline 1 0 2 0
    sc_row capture cap-one steelman-baseline  1 1 2 0
    sc_row capture cap-one memory-off         1 0 2 0
    sc_row capture cap-one length-matched-placebo         1 0 2 0
  } > "$caphigh"
  if aggregate_scorecard "$caphigh" 0.10 1 | grep -q '\-> capture:.*capture_fidelity_target=yes.*=> PASS'; then echo "ok: capture dimension passes when downstream and direct fidelity targets pass"; else echo "BUG: capture dimension did not pass with high direct fidelity"; aggregate_scorecard "$caphigh" 0.10 1; fail=1; fi

  # RE-RIG GUARD: memory beats realistic but LOSES to steelman => dimension must NOT pass.
  local rig="$tmp/rig.tsv"
  {
    sc_row freshness s1 memory-on          1 0 2 0
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 0 2 0
    sc_row freshness s1 length-matched-placebo         1 0 2 0
  } > "$rig"
  if aggregate_scorecard "$rig" 0.10 1 | grep -q '\-> freshness:.*=> no'; then echo "ok: re-rig guard — losing to steelman fails the dimension"; else echo "BUG: re-rig not caught"; fail=1; fi

  # SAFETY: only a memory-on failure whose same-run memory-off succeeds and
  # whose actual prompt receipt contains stale evidence is attributed as MIE.
  local unsafe="$tmp/unsafe.tsv"
  {
    sc_causal_row freshness s1 memory-on 1 0 2 1 1 stale_injected memory_induced memory_on_only_failure_with_stale_injection
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 1 2 0
    sc_row freshness s1 length-matched-placebo 1 0 2 0
  } > "$unsafe"
  if aggregate_scorecard "$unsafe" 0.10 1 >/dev/null; then echo "BUG: causal MIE did not fail the safety gate"; fail=1; else echo "ok: causal MIE fails the safety gate"; fi

  # The identical task failure in both arms is not charged to memory.
  local bothfail="$tmp/bothfail.tsv"
  {
    sc_causal_row freshness s1 memory-on 1 0 2 0 1 stale_injected not_memory_induced both_arms_failed
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 0 2 0
    sc_row freshness s1 length-matched-placebo 1 0 2 0
  } > "$bothfail"
  if aggregate_scorecard "$bothfail" 0.10 1 | grep -qF $'attribution=not_memory_induced\treason=both_arms_failed'; then echo "ok: both-arms failure is not attributed to memory"; else echo "BUG: both-arms failure attribution"; fail=1; fi
  if aggregate_scorecard "$bothfail" 0.10 1 >/dev/null; then echo "ok: both-arms failure does not fail causal safety"; else echo "BUG: both-arms failure charged as causal MIE"; fail=1; fi

  # Missing receipt evidence is not relabeled SAFE: it is explicitly
  # unassessable and fails the complete-run gate.
  local noev="$tmp/no-evidence.tsv"
  {
    sc_causal_row freshness s1 memory-on 1 0 2 0 unknown missing_prompt_receipt unassessable missing_prompt_receipt
    sc_row freshness s1 realistic-baseline 1 0 2 0
    sc_row freshness s1 steelman-baseline  1 1 2 0
    sc_row freshness s1 memory-off         1 1 2 0
    sc_row freshness s1 length-matched-placebo 1 0 2 0
  } > "$noev"
  if aggregate_scorecard "$noev" 0.10 1 >/dev/null; then echo "BUG: missing causal evidence did not fail closed"; fail=1; else echo "ok: missing causal evidence fails closed"; fi

  # Historical 13-field rows remain parseable but lack the causal evidence needed
  # for SAFE, so complete legacy data is explicitly unassessable.
  local legacy="$tmp/legacy.tsv"
  {
    legacy_row freshness s1 memory-on 1 0 2 0
    legacy_row freshness s1 realistic-baseline 1 0 2 0
    legacy_row freshness s1 steelman-baseline 1 1 2 0
    legacy_row freshness s1 memory-off 1 1 2 0
    legacy_row freshness s1 length-matched-placebo 1 0 2 0
  } > "$legacy"
  if aggregate_scorecard "$legacy" 0.10 1 >/dev/null; then echo "BUG: legacy TSV without causal evidence did not fail closed"; fail=1; else echo "ok: legacy TSV parses and fails closed"; fi
  if aggregate_scorecard "$legacy" 0.10 1 | grep -qF 'reason=missing_causal_fields'; then echo "ok: legacy TSV names missing causal fields"; else echo "BUG: legacy TSV missing reason"; fail=1; fi

  # P3 — single runs never gate: the same unsafe data at min_runs=5 is
  # DIRECTIONAL, exits 0, yet retains the pair attribution.
  if aggregate_scorecard "$unsafe" 0.10 5 >/dev/null; then echo "ok: sub-min-runs unsafe run is directional (exit 0)"; else echo "BUG: sub-min-runs run gated"; fail=1; fi
  if aggregate_scorecard "$unsafe" 0.10 5 | grep -q 'DIRECTIONAL ONLY'; then echo "ok: sub-min-runs run prints DIRECTIONAL ONLY"; else echo "BUG: no DIRECTIONAL banner"; fail=1; fi
  if aggregate_scorecard "$unsafe" 0.10 5 | grep -q 'memory_induced=1'; then echo "ok: causal MIE remains enumerated when directional"; else echo "BUG: causal MIE not enumerated when directional"; fail=1; fi

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
    source_row 0 1 retrieval_scale a1 memory-on          1 1 3 0 0.02 600  4000 4400 100 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0.01 6000 0    0    100 false
    sc_row retrieval_scale a1 length-matched-placebo         1 0 5 0 0.01 6000 0    0    100 false
  } > "$ratify"
  if aggregate_scorecard "$ratify" 0.10 1 | grep -qF 'RATIFY Opt 3'; then echo "ok: ADR-3 ratify when accuracy + cost within bounds"; else echo "BUG: ADR-3 ratify verdict"; aggregate_scorecard "$ratify" 0.10 1; fail=1; fi

  # A correct answer already present at SessionStart is reported honestly and
  # cannot satisfy the query-retrieval-specific Class A claim.
  local startup_scale="$tmp/startup-scale.tsv"
  {
    source_row 1 1 retrieval_scale a1 memory-on          1 1 3 0 0.02 600 4000 4400 100 false
    sc_row retrieval_scale a1 realistic-baseline         1 0 5 0 0.02 300 4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline          1 1 3 0 0.02 300 4000 8700 100 false
    sc_row retrieval_scale a1 memory-off                 1 0 5 0 0.01 6000 0 0 100 false
    sc_row retrieval_scale a1 length-matched-placebo     1 0 5 0 0.01 6000 0 0 100 false
  } > "$startup_scale"
  local startup_out; startup_out="$(aggregate_scorecard "$startup_scale" 0.10 1)"
  if printf '%s' "$startup_out" | grep -qF 'query_retrieval_evidenced=NO' \
     && printf '%s' "$startup_out" | grep -qF 'query-retrieval-specific successes=0/1 startup-context successes=1/1'; then
    echo "ok: SessionStart-backed success does not become a query retrieval claim"
  else
    echo "BUG: startup evidence was misreported as query retrieval"; printf '%s\n' "$startup_out"; fail=1
  fi

  # ADR-3: Opt 2 candidate when accuracy wins but total_cost_usd > 20% worse.
  local opt2="$tmp/opt2.tsv"
  {
    source_row 0 1 retrieval_scale a1 memory-on          1 1 3 0 0.03 4000  4000 6000 100 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0.02 100   4000 9900 100 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0.02 100   4000 9900 100 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0.01 10000 0    0    100 false
    sc_row retrieval_scale a1 length-matched-placebo         1 0 5 0 0.01 10000 0    0    100 false
  } > "$opt2"
  if aggregate_scorecard "$opt2" 0.10 1 | grep -qF 'Opt 2 candidate'; then echo "ok: ADR-3 Opt 2 candidate when cost > 20% worse"; else echo "BUG: ADR-3 opt2 verdict"; aggregate_scorecard "$opt2" 0.10 1; fail=1; fi

  # ADR-3: a session error in memory-on or steelman skips the cost verdict (a
  # failed session reads cost=0 and must not look like a cheap RATIFY).
  local serr="$tmp/serr.tsv"
  {
    source_row 0 0 retrieval_scale a1 memory-on         1 0 99 0 0 0 0 0 0 true
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
    source_row 0 1 retrieval_scale a1 memory-on          1 1 3 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0 0 0 0 0 false
    sc_row retrieval_scale a1 length-matched-placebo         1 0 5 0 0 0 0 0 0 false
  } > "$zcost"
  local zout; zout="$(aggregate_scorecard "$zcost" 0.10 1)"
  if printf '%s' "$zout" | grep -qF 'SKIP cost verdict (zero/absent total_cost_usd'; then echo "ok: ADR-3 fails closed on zero/absent cost (non-errored)"; else echo "BUG: ADR-3 zero-cost skip"; printf '%s\n' "$zout"; fail=1; fi
  if printf '%s' "$zout" | grep -qF 'RATIFY Opt 3'; then echo "BUG: ADR-3 ratified a zero-cost cell"; fail=1; else echo "ok: ADR-3 does not ratify a zero-cost cell"; fi

  # ADR-3 fail-closed must catch a PARTIAL collapse too: one non-errored run with
  # cost=0 among positive runs keeps the MEAN positive, yet the per-run guard must
  # still SKIP (the mean-only check would have wrongly RATIFIED here).
  local pcost="$tmp/pcost.tsv"
  {
    source_row 0 1 retrieval_scale a1 memory-on          1 1 3 0 0.02 600  4000 4400 100 false
    source_row 0 1 retrieval_scale a1 memory-on          2 1 3 0 0    600  4000 4400 100 false
    sc_row retrieval_scale a1 realistic-baseline 1 0 5 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline  1 1 3 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 steelman-baseline  2 1 3 0 0.02 300  4000 8700 100 false
    sc_row retrieval_scale a1 memory-off         1 0 5 0 0.01 6000 0    0    100 false
    sc_row retrieval_scale a1 length-matched-placebo         1 0 5 0 0.01 6000 0    0    100 false
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

  # Complete gating run: all five arms at N=5 (>= min_runs), memory-on beats
  # realistic AND ties steelman, zero MIE => gates SAFE, dimension PASS, and is
  # NOT directional / not incomplete (locks the gating path vs the directional cases).
  local full="$tmp/full.tsv"
  {
    for t in 1 2 3 4 5; do sc_row d s memory-on          "$t" 1 2 0; done
    for t in 1 2 3 4 5; do sc_row d s realistic-baseline "$t" 0 3 0; done
    for t in 1 2 3 4;   do sc_row d s steelman-baseline  "$t" 1 2 0; done
                           sc_row d s steelman-baseline  5   0 2 0
    for t in 1 2 3 4 5; do sc_row d s memory-off         "$t" 0 5 0; done
    for t in 1 2 3 4 5; do sc_row d s length-matched-placebo         "$t" 0 5 0; done
  } > "$full"
  local fout; fout="$(aggregate_scorecard "$full" 0.10 5)"
  if echo "$fout" | grep -q 'result: SAFE' && echo "$fout" | grep -q '\-> d:.*=> PASS' \
     && ! echo "$fout" | grep -q 'DIRECTIONAL' && ! echo "$fout" | grep -q 'incomplete arms'; then
    echo "ok: complete N=5 run gates SAFE + PASS, not directional"
  else
    echo "BUG: complete gating run mis-handled"; echo "$fout"; fail=1
  fi

  # Injection-source diagnostics distinguish startup-visible evidence from a
  # target surfaced only by the prompt query.
  local source_diag="$tmp/source-diagnostics"; mkdir -p "$source_diag"
  printf '{"continue":true,"message":"startup has another decision"}\n' > "$source_diag/session-start-injection.json"
  printf '{"continue":true,"message":"prompt recalled use ureq"}\n' > "$source_diag/prompt-time-injection.json"
  check "diagnostics report query-only answer evidence" "$(printf '0\t1')" \
    "$(finalize_memory_diagnostics "$source_diag" 1 0 receipt_empty ureq "")"
  if jq -e '.session_start_answer_present == false and .prompt_recall_answer_present == true and .query_only_answer_evidence == true' \
      "$source_diag/outcome.json" >/dev/null; then
    echo "ok: diagnostics outcome preserves separate injection sources"
  else
    echo "BUG: diagnostics outcome merged injection sources"; fail=1
  fi
  # Session-log retention copies diagnosable OMP JSON logs, judged text, daemon
  # logs, exact extension injections, candidates, states, and histories out of a
  # workroot. It copies nothing else: no store DBs, seeded AGENTS.md, or stray
  # JSONL the harness did not write.
  local lw="$tmp/logs-workroot" ld="$tmp/logs-dest"
  mkdir -p "$lw/fresh-r1/on/diagnostics" "$lw/fresh-r1/realistic/p"
  printf '{"type":"agent_end"}\n' > "$lw/fresh-r1/on/work.jsonl"
  printf 'plant\n'                 > "$lw/fresh-r1/on/plant.jsonl"
  printf 'daemon\n'                > "$lw/fresh-r1/on/daemon.log"
  printf 'judged\n'                > "$lw/fresh-r1/realistic/judge.txt"
  printf 'db\n'                    > "$lw/fresh-r1/on/memory.db"
  printf 'seeded\n'                > "$lw/fresh-r1/realistic/p/AGENTS.md"
  printf 'stray\n'                 > "$lw/fresh-r1/on/other.jsonl"
  printf '{}\n'                > "$lw/fresh-r1/on/diagnostics/index.json"
  printf '{}\n'                > "$lw/fresh-r1/on/diagnostics/outcome.json"
  printf '[]\n'                > "$lw/fresh-r1/on/diagnostics/recall-active.json"
  printf '{}\n'                > "$lw/fresh-r1/on/diagnostics/history-00000000-0000-0000-0000-000000000000.json"
  printf '{}\n'                > "$lw/fresh-r1/on/diagnostics/session-start-input.json"
  printf '{}\n'                > "$lw/fresh-r1/on/diagnostics/session-start-injection.json"
  printf 'must-drop\n'         > "$lw/fresh-r1/on/diagnostics/unexpected.txt"
  mkdir -p "$lw/fresh-r1/placebo/injections"
  printf '{"reason":"missing source"}\n' > "$lw/fresh-r1/placebo/injections/control-error.json"
  mkdir -p "$lw/fresh-r1/on/wp/on/diagnostics"
  printf 'model-spoof\n'       > "$lw/fresh-r1/on/wp/on/diagnostics/index.json"
  printf 'model-spoof\n'       > "$lw/fresh-r1/realistic/p/work.jsonl"
  if preserve_session_logs "$lw" "$ld"; then
    echo "ok: preserve_session_logs succeeds on a populated workroot"
  else
    echo "BUG: preserve_session_logs failed on a populated workroot"; fail=1
  fi
  for kept in fresh-r1/on/work.jsonl fresh-r1/on/plant.jsonl fresh-r1/on/daemon.log fresh-r1/realistic/judge.txt fresh-r1/placebo/injections/control-error.json; do
    if [ -f "$ld/$kept" ]; then echo "ok: preserve_session_logs kept $kept"; else echo "BUG: preserve_session_logs lost $kept"; fail=1; fi
  done
  for kept in fresh-r1/on/diagnostics/index.json fresh-r1/on/diagnostics/outcome.json fresh-r1/on/diagnostics/recall-active.json fresh-r1/on/diagnostics/session-start-input.json fresh-r1/on/diagnostics/session-start-injection.json fresh-r1/on/diagnostics/history-00000000-0000-0000-0000-000000000000.json; do
    if [ -f "$ld/$kept" ]; then echo "ok: preserve_session_logs kept $kept"; else echo "BUG: preserve_session_logs lost $kept"; fail=1; fi
  done
  for dropped in fresh-r1/on/memory.db fresh-r1/realistic/p/AGENTS.md fresh-r1/on/other.jsonl fresh-r1/on/diagnostics/unexpected.txt fresh-r1/on/wp/on/diagnostics/index.json fresh-r1/realistic/p/work.jsonl; do
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
  # A per-file copy failure warns NAMING the file and returns non-zero (a
  # last-command status would mask early failures); the caller treats it as
  # best-effort. An unreadable source file forces the failure deterministically.
  if [ "$(id -u)" != "0" ]; then # root ignores mode bits; skip there
    local lwf="$tmp/logs-fail" ldf="$tmp/logs-fail-dest" perr
    mkdir -p "$lwf/fresh-r1/on"
    printf 'ok\n'     > "$lwf/fresh-r1/on/work.jsonl"
    printf 'secret\n' > "$lwf/fresh-r1/on/daemon.log"; chmod 000 "$lwf/fresh-r1/on/daemon.log"
    if perr="$(preserve_session_logs "$lwf" "$ldf" 2>&1)"; then
      echo "BUG: preserve_session_logs must return non-zero when a copy fails"; fail=1
    else
      echo "ok: preserve_session_logs returns non-zero on a copy failure"
    fi
    if printf '%s' "$perr" | grep -qF "daemon.log"; then
      echo "ok: preserve_session_logs warning names the failed file"
    else
      echo "BUG: preserve_session_logs warning must name the failed file"; printf '%s\n' "$perr"; fail=1
    fi
    if [ -f "$ldf/fresh-r1/on/work.jsonl" ]; then
      echo "ok: preserve_session_logs keeps copying past a failed file"
    else
      echo "BUG: preserve_session_logs must not stop at the first failure"; fail=1
    fi
    chmod 644 "$lwf/fresh-r1/on/daemon.log"
  fi

  # resolve_log_dir: the --log-dir/RB_SCORECARD_KEEP_LOGS wiring is a pure
  # function so the derivation is self-testable (PR #70 review). --log-dir
  # wins; keep-logs derives <dir of --out>/scorecard-session-logs; keep-logs
  # without an --out anchor, or with a missing --out directory, FAILS CLOSED
  # (PR #70 CodeRabbit: the silent fallback would resolve to the fs root).
  local rl
  if rl="$(resolve_log_dir "/explicit/dir" 1 "$tmp/out.tsv")"; then
    check "resolve_log_dir explicit --log-dir wins" "/explicit/dir" "$rl"
  else
    echo "BUG: resolve_log_dir failed on an explicit dir"; fail=1
  fi
  # Compare against the NORMALIZED tmp path: a trailing slash in $TMPDIR (the
  # macOS default) doubles up in $tmp, and resolve_log_dir's `cd && pwd`
  # correctly collapses it.
  if rl="$(resolve_log_dir "" 1 "$tmp/out.tsv")"; then
    check "resolve_log_dir derives from --out" "$(cd "$tmp" && pwd)/scorecard-session-logs" "$rl"
  else
    echo "BUG: resolve_log_dir failed to derive from --out"; fail=1
  fi
  if rl="$(resolve_log_dir "" 0 "$tmp/out.tsv")"; then
    check "resolve_log_dir empty when retention off" "" "$rl"
  else
    echo "BUG: resolve_log_dir failed with retention off"; fail=1
  fi
  if resolve_log_dir "" 1 "" >/dev/null 2>&1; then
    echo "BUG: resolve_log_dir must fail closed without --out"; fail=1
  else
    echo "ok: resolve_log_dir fails closed when keep-logs has no --out anchor"
  fi
  if resolve_log_dir "" 1 "$tmp/no-such-dir/out.tsv" >/dev/null 2>&1; then
    echo "BUG: resolve_log_dir must fail closed on a missing --out directory"; fail=1
  else
    echo "ok: resolve_log_dir fails closed on a missing --out directory"
  fi

  if [ "$fail" -eq 0 ]; then echo "self-test PASS"; return 0; fi
  echo "self-test FAIL" >&2; return 1
}

# ---- arg parsing -------------------------------------------------------------
MODE="run"; BIN_DIR=""; RUNS=""; OUT=""; MIN_RUNS=""; AGENT="omp"; PRE_SCORECARD_ROWS=""; LOG_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)      MODE="self-test"; shift ;;
    --agent)          AGENT="${2:?--agent needs a value}"
                      case "$AGENT" in omp|codex|opencode|gemini|hermes|all) ;; *) echo "--agent must be one of: omp, codex, opencode, gemini, hermes, all (got '$AGENT')" >&2; exit 2 ;; esac
                      shift 2 ;;
    --model)          MODEL="${2:?--model needs an OMP model selector}"; shift 2 ;;
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
  if [ -n "$OUT" ]; then printf '%s\n' "$PRE_SCORECARD_ROWS" > "$OUT"; fi
  AGENT="omp"
elif ! scorecard_agent_supported "$AGENT"; then
  skip_line="$(scorecard_skip_line "$AGENT")"
  echo "== memory-value scorecard agent target: $AGENT =="
  printf '%s\n' "$skip_line"
  if [ -n "$OUT" ]; then printf '%s\n' "$skip_line" > "$OUT"; fi
  exit 0
fi

# ---- live run prerequisites (the five-arm runner) -----------------------------
[ -n "$BIN_DIR" ] || BIN_DIR="$REPO_ROOT/target/release"
[ -d "$BIN_DIR" ] || { echo "bin dir not found: $BIN_DIR (build first with 'cargo build --release', or pass --bin-dir DIR)" >&2; exit 1; }
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
for bin in rusty-brain rusty-brain-hooks; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin" >&2; exit 1; }
done
command -v omp     >/dev/null 2>&1 || { echo "omp not on PATH" >&2; exit 1; }
command -v jq      >/dev/null 2>&1 || { echo "jq not on PATH" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH" >&2; exit 1; }
[ -f "$SCENARIOS_FILE" ] || { echo "scenarios file not found: $SCENARIOS_FILE" >&2; exit 1; }
[ -n "$RUNS" ] || RUNS="$(jq -r '.config.runs_per_scenario // 5' "$SCENARIOS_FILE")"
[ -n "$MIN_RUNS" ] || MIN_RUNS="$(jq -r '.config.min_runs // 5' "$SCENARIOS_FILE")"
if [ -z "${RB_SCORECARD_TIE_MARGIN+x}" ]; then
  TIE_MARGIN="$(jq -r '.config.tie_margin // .config.steelman_tie // 0.10' "$SCENARIOS_FILE")"
fi
case "$MIN_RUNS" in ''|*[!0-9]*|0) echo "min_runs must be a positive integer (got '$MIN_RUNS')" >&2; exit 2 ;; esac

unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE RUSTY_BRAIN_IDLE_TIMEOUT_SECS
WORKROOT="$(mktemp -d "${TMPDIR:-/tmp}/rb-scorecard.XXXXXX")"
RESULTS="${OUT:-$WORKROOT/scorecard.tsv}"
RAW_RESULTS="$WORKROOT/scorecard-raw.tsv"; : > "$RAW_RESULTS"
python3 "$REPO_ROOT/scripts/scorecard-controls.py" metadata "$RESULTS.metadata.json" "$MODEL" "$(omp --version)" "$REPO_ROOT/crates/rb-install/assets/omp-extension.ts"
if [ -n "$PRE_SCORECARD_ROWS" ]; then
  printf '%s\n' "$PRE_SCORECARD_ROWS" >> "$RAW_RESULTS"
fi
# Session-log retention (Vikunja #502): --log-dir wins; RB_SCORECARD_KEEP_LOGS=1
# derives the dir from --out. resolve_log_dir fails closed (exit 2) without an
# --out anchor or when its directory is missing — logs written into the
# about-to-be-deleted workroot or the fs root would be silently lost, the
# exact defect this fixes.
LOG_DIR="$(resolve_log_dir "$LOG_DIR" "${RB_SCORECARD_KEEP_LOGS:-0}" "$OUT")" || exit 2
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

seed_home() { # home [project]
  mkdir -p "$1"
  [ -z "${2:-}" ] || mkdir -p "$2"
}

run_session() { # home project prompt log
  local home="$1" project="$2" prompt="$3" log="$4"
  local extension=()
  if [ "${RB_SCORECARD_USE_EXTENSION:-0}" = "1" ]; then
    extension=(--extension "$REPO_ROOT/scripts/scorecard-omp-extension.ts")
  fi
  (
    export HOME="$home" PI_CODING_AGENT_DIR="$home/.omp/agent"
    export PATH="$BIN_DIR:$PATH" RB_OMP_HOOKS_BIN="$BIN_DIR/rusty-brain-hooks"
    cd "$project" || return 1
    omp --no-extensions "${extension[@]}" --mode json --session-dir "$home/.omp/sessions" \
      --model "$MODEL" --max-time "${MAX_SESSION_SECONDS}s" --auto-approve -p "$prompt" \
      </dev/null >"$log" 2>&1 || true
  )
}

# Score one OMP work session and append an unattributed scorecard row. OMP's
# terminal `agent_end` event supplies usage. Scenarios with an artifact assertion
# are judged solely on that file; the compatibility fallback uses assistant output
# plus touched workspace files, never the seeded AGENTS.md.
score_session() { # dim id arm run proj home work expect stale outcome_path outcome_exact [capture... diagnostics]
  local dim="$1" id="$2" arm="$3" run="$4" proj="$5" home="$6" work="$7" expect="$8" stale="$9"
  local outcome_path="${10:-}" outcome_exact="${11:-}"
  local cap_fidelity="${12:-na}" cap_reason="${13:-na}" cap_summary_count="${14:-0}" cap_mcp_bypass_count="${15:-0}"
  local diagnostics_dir="${16:-}"
  local jlog="$proj/../work.jsonl" jtext="$proj/../judge.txt" marker="$proj/../mark" injections="$proj/../injections"
  mkdir -p "$injections"
  local file_tokens injection_metrics
  file_tokens="$(python3 "$REPO_ROOT/scripts/scorecard-controls.py" file-tokens "$proj/AGENTS.md")"
  : > "$marker"
  case "$arm" in
    memory-on)
      RB_SCORECARD_USE_EXTENSION=1 RB_SCORECARD_INJECTION_DIR="$injections" \
        run_session "$home" "$proj" "$work" "$jlog"
      ;;
    length-matched-placebo)
      RB_SCORECARD_USE_EXTENSION=1 RB_SCORECARD_INJECTION_DIR="$injections" \
        RB_SCORECARD_PLACEBO_SOURCE="${RB_SCORECARD_PLACEBO_SOURCE:?placebo source missing}" \
        RB_SCORECARD_FORBIDDEN="$expect"$'\n'"$stale" \
        run_session "$home" "$proj" "$work" "$jlog"
      ;;
    *) run_session "$home" "$proj" "$work" "$jlog" ;;
  esac
  injection_metrics="$(python3 "$REPO_ROOT/scripts/scorecard-controls.py" metrics "$injections" "$file_tokens")"
  local u is_err turns cost inp cc cr out
  u="$(extract_usage "$jlog")"
  is_err="$(cut -f1 <<<"$u")"; turns="$(cut -f2 <<<"$u")"; cost="$(cut -f3 <<<"$u")"
  inp="$(cut -f4 <<<"$u")"; cc="$(cut -f5 <<<"$u")"; cr="$(cut -f6 <<<"$u")"; out="$(cut -f7 <<<"$u")"
  jq -r 'select(.type=="agent_end") | .messages[]? | select(.role=="assistant") | .content[]? | select(.type=="text") | .text' "$jlog" \
    > "$jtext" 2>/dev/null || cp "$jlog" "$jtext"
  find "$proj" -type f -not -path '*/.*' -newer "$marker" -size -256k -print0 2>/dev/null \
    | xargs -0 cat >> "$jtext" 2>/dev/null || true
  local success mie=0 stale_evidence="na" evidence_reason="not_memory_on_arm" source_metrics=$'na\tna'
  success="$(judge_outcome "$proj" "$jtext" "$expect" "$outcome_path" "$outcome_exact")"
  if [ "$is_err" = "true" ]; then success=0; fi
  if [ "$arm" = "memory-on" ]; then
    IFS=$'\t' read -r stale_evidence evidence_reason \
      < <(python3 "$REPO_ROOT/scripts/scorecard-controls.py" stale-evidence "$injections" "$stale")
    if [ -n "$diagnostics_dir" ]; then
      source_metrics="$(finalize_memory_diagnostics "$diagnostics_dir" "$success" "$stale_evidence" "$evidence_reason" "$expect" "$stale")"
    fi
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$dim" "$id" "$arm" "$run" "$success" "$turns" "$mie" \
    "$cost" "$inp" "$cc" "$cr" "$out" "$is_err" \
    "$cap_fidelity" "$cap_reason" "$cap_summary_count" "$cap_mcp_bypass_count" \
    "$injection_metrics" "$source_metrics" "$stale_evidence" "$evidence_reason" >> "$RAW_RESULTS"
  local cap_msg=""
  if [ "$cap_fidelity" != "na" ]; then
    cap_msg=" cap=$cap_fidelity/$cap_reason summaries=$cap_summary_count mcp_bypass=$cap_mcp_bypass_count"
  fi
  echo "   [$dim/$id $arm r$run] success=$success turns=$turns cost=$cost mie=pending-pair$cap_msg stale_evidence=$stale_evidence/$evidence_reason injected_est[start,prompt,file,total,estimator]=${injection_metrics//$'\t'/,} answer_evidence[start,prompt]=${source_metrics//$'\t'/,}"
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
# target here at the scenario's `corpus_importance`; run_scenario rejects a scale
# scenario unless every target uses the same importance as its distractors.
plant_explicit() { # home (env: RUSTY_BRAIN_*) <facts-json-array> [diagnostic-manifest.jsonl]
  local home="$1" facts="$2" manifest="${3:-}"
  ( export HOME="$home"; export PATH="$BIN_DIR:$PATH"
    local n i content imp sup out last_id="" prior_id=""
    if [ -n "$manifest" ]; then mkdir -p "$(dirname "$manifest")"; : > "$manifest"; fi
    n="$(jq 'length' <<<"$facts")"
    for (( i=0; i<n; i++ )); do
      content="$(jq -r ".[$i].content" <<<"$facts")"
      imp="$(jq -r ".[$i].importance // 5" <<<"$facts")"
      sup="$(jq -r ".[$i].supersedes_prev // false" <<<"$facts")"
      prior_id="$last_id"
      if [ "$sup" = "true" ] && [ -n "$last_id" ]; then
        out="$(rusty-brain --json remember "$content" --type insight --importance "$imp" --supersedes "$last_id")"
      else
        out="$(rusty-brain --json remember "$content" --type insight --importance "$imp")"
      fi
      last_id="$(jq -r '.id // empty' <<<"$out" 2>/dev/null)"
      [ -n "$last_id" ] || { echo "ERROR: explicit plant did not return an id for fact $i" >&2; return 1; }
      if [ -n "$manifest" ]; then
        jq -cn --argjson index "$i" --arg id "$last_id" --arg prior "$prior_id" \
          --argjson supersedes "$sup" --arg content "$content" \
          '{index:$index,id:$id,content:$content,
            supersedes:(if $supersedes and ($prior|length)>0 then $prior else null end)}' \
          >> "$manifest"
      fi
    done
  )
}

# Class A (retrieval@scale): bulk-plant N deterministic same-domain competitors
# at exactly the target's importance. A batch uses one CLI process and daemon
# connection; at 500+ facts per-fact process spawn dominates. The markdown bullet
# is stripped so stored text matches the bare fact used in AGENTS.md baselines.
plant_corpus_distractors() { # home scenario_id n importance guards_json targets_json
  local home="$1" sid="$2" n="$3" importance="$4" guards="$5" targets="$6"
  [ "$n" -gt 0 ] || return 0
  ( export HOME="$home"; export PATH="$BIN_DIR:$PATH"
    gen_corpus "$sid" "$n" "$guards" "$targets" | sed 's/^- //' \
      | rusty-brain remember --batch --type insight --importance "$importance" >/dev/null \
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
  local id dim plant_mode work expect stale realistic steelman facts corpus plant_session capture_expect capture_forbid
  local outcome_path outcome_exact corpus_importance collision_guards target_contents
  id="$(jq -r '.id' <<<"$row")"; dim="$(jq -r '.dimension' <<<"$row")"
  plant_mode="$(jq -r '.plant_mode' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"; expect="$(jq -r '.expect' <<<"$row")"
  stale="$(jq -r '.stale_token // ""' <<<"$row")"
  outcome_path="$(jq -r '.outcome_assertion.path // ""' <<<"$row")"
  outcome_exact="$(jq -r '.outcome_assertion.exact // ""' <<<"$row")"
  realistic="$(jq -r '.realistic_agents_md // ""' <<<"$row")"
  steelman="$(jq -r '.steelman_agents_md // ""' <<<"$row")"
  facts="$(jq -c '.plant // []' <<<"$row")"
  plant_session="$(jq -r '.plant_session // ""' <<<"$row")"
  capture_expect="$(jq -r '.capture_expect // .expect // ""' <<<"$row")"
  capture_forbid="$(jq -r '.capture_forbid // ""' <<<"$row")"
  # Class A generates one deterministic same-domain corpus per scenario so every
  # run and arm sees identical competitors. Scale rows fail closed unless every
  # target and distractor has exactly the declared corpus importance.
  corpus="$(jq -r '.corpus_size // 0' <<<"$row")"
  corpus_importance="$(jq -r '.corpus_importance // 5' <<<"$row")"
  case "$corpus" in ''|*[!0-9]*) echo "ERROR: $id corpus_size must be a non-negative integer (got '$corpus')" >&2; return 1 ;; esac
  case "$corpus_importance" in ''|*[!0-9]*|0) echo "ERROR: $id corpus_importance must be a positive integer (got '$corpus_importance')" >&2; return 1 ;; esac
  collision_guards="$(jq -c '[.expect?, .stale_token?, .forbid?] + (.distractor_collision_tokens // []) | map(select(type == "string" and length > 0))' <<<"$row")"
  target_contents="$(jq -c '[.plant[]?.content | select(type == "string" and length > 0)]' <<<"$row")"
  local distractors=""
  if [ "$corpus" -gt 0 ]; then
    if ! scale_importance_valid "$row"; then
      echo "ERROR: $id target importance must equal distractor importance $corpus_importance" >&2
      return 1
    fi
    distractors="$(gen_corpus "$id" "$corpus" "$collision_guards" "$target_contents")"
  fi
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
      rm -f "$wp/AGENTS.md"
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
      # The daemon receives no model credential. OMP resolves the selected
      # provider from its isolated profile environment.
      env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u GEMINI_API_KEY rusty-brain serve >"$derr" 2>&1 &
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
      local diagnostics_dir="$mb/diagnostics" plant_manifest="$mb/diagnostics/planted.jsonl"
      mkdir -p "$diagnostics_dir"
      : > "$plant_manifest"
      if [ "$dim" = "reach" ]; then
        # Identity A plants from its home only (plant_explicit needs no project),
        # so pa is intentionally unused; only B gets a project to be scored in.
        local ha pa hb pb
        IFS=$'\t' read -r ha pa hb pb < <(reach_identity_paths "$mb")
        : "$pa" # tab-split placeholder; A needs no project dir
        seed_home "$ha"; seed_home "$hb" "$pb"
        rm -f "$pb/AGENTS.md"
        plant_explicit "$ha" "$facts" "$plant_manifest"
        capture_memory_diagnostics "$hb" "$pb" "$work" "$diagnostics_dir" "$plant_manifest"
        score_session "$dim" "$id" "memory-on" "$run" "$pb" "$hb" "$work" "$expect" "$stale" \
          "$outcome_path" "$outcome_exact" "na" "na" "0" "0" "$diagnostics_dir"
      else
        if [ "$plant_mode" = "explicit" ]; then
          plant_explicit "$wh" "$facts" "$plant_manifest"
          # Class A: bury the target among equal-importance same-domain competitors.
          plant_corpus_distractors "$wh" "$id" "$corpus" "$corpus_importance" "$collision_guards" "$target_contents"
        elif [ "$plant_mode" = "auto-capture" ]; then
          local ph="$mb/hp" pp="$mb/pp" plog="$mb/plant.jsonl"
          seed_home "$ph" "$pp"
          rm -f "$pp/AGENTS.md"
          # Auto-capture plant: a real OMP session whose native extension folds
          # the session transcript at shutdown; no MCP path writes this memory.
          RB_SCORECARD_USE_EXTENSION=1 run_session "$ph" "$pp" "$plant_session" "$plog"
          IFS=$'\t' read -r cap_fidelity cap_reason cap_summary_count cap_mcp_bypass_count \
            < <(measure_capture_fidelity "$wh" "$capture_expect" "$capture_forbid")
          # Capture status is printed with its work-session injection sizes by score_session.
        else
          echo "ERROR: unknown plant_mode '$plant_mode' for $id" >&2
          exit 1
        fi
        capture_memory_diagnostics "$wh" "$wp" "$work" "$diagnostics_dir" "$plant_manifest"
        score_session "$dim" "$id" "memory-on" "$run" "$wp" "$wh" "$work" "$expect" "$stale" \
          "$outcome_path" "$outcome_exact" "$cap_fidelity" "$cap_reason" "$cap_summary_count" "$cap_mcp_bypass_count" "$diagnostics_dir"
      fi
    )
    rm -rf "$sockdir" 2>/dev/null || true

    # realistic-baseline + steelman-baseline + placebo + memory-off. For Class A
    # the distractor corpus is written into both AGENTS.md baselines so the
    # buried target is the only difference.
    local rb="$base/realistic"; seed_home "$rb/h" "$rb/p"
    write_agents_md "$rb/p/AGENTS.md" "$realistic" "$distractors"
    score_session "$dim" "$id" "realistic-baseline" "$run" "$rb/p" "$rb/h" "$work" "$expect" "$stale" "$outcome_path" "$outcome_exact"

    local sb="$base/steelman"; seed_home "$sb/h" "$sb/p"
    write_agents_md "$sb/p/AGENTS.md" "$steelman" "$distractors"
    score_session "$dim" "$id" "steelman-baseline" "$run" "$sb/p" "$sb/h" "$work" "$expect" "$stale" "$outcome_path" "$outcome_exact"

    # Replay neutral text sized from the actual OMP prompt-time receipt.
    local cb="$base/placebo"; seed_home "$cb/h" "$cb/p"
    RB_SCORECARD_PLACEBO_SOURCE="$mb/injections" \
      score_session "$dim" "$id" "length-matched-placebo" "$run" "$cb/p" "$cb/h" "$work" "$expect" "$stale" "$outcome_path" "$outcome_exact"
    python3 "$REPO_ROOT/scripts/scorecard-controls.py" validate-pair "$mb/injections" "$cb/injections"

    local ob="$base/off"; seed_home "$ob/h" "$ob/p"
    score_session "$dim" "$id" "memory-off" "$run" "$ob/p" "$ob/h" "$work" "$expect" "$stale" "$outcome_path" "$outcome_exact"

    run=$((run + 1))
  done
}

echo "== memory-value scorecard (agent=$AGENT, model=$MODEL, timeout=${MAX_SESSION_SECONDS}s, runs=$RUNS) =="
# Read all scenarios before running any: OMP print mode must not consume the
# loop's input stream.
scenario_rows=()
while IFS= read -r row; do scenario_rows+=("$row"); done < <(jq -c '.scenarios[]' "$SCENARIOS_FILE")
for row in "${scenario_rows[@]}"; do run_scenario "$row"; done
python3 "$REPO_ROOT/scripts/scorecard-controls.py" attribute-results "$RAW_RESULTS" "$RESULTS" "$WORKROOT"
echo
echo "== scorecard =="
aggregate_scorecard "$RESULTS" "$TIE_MARGIN" "$MIN_RUNS"
