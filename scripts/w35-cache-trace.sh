#!/usr/bin/env bash
# P4a prompt-caching study (companion to scripts/w35-ab-eval.sh and
# scripts/w35-trace-tools.sh; records ADR-3 in
# docs/eval/2026-06-19-p4a-caching-study.md).
#
# The W3.5 scorecard's dimension A (Retrieval at scale) is gated on an open
# question (redesign §A): a large CLAUDE.md sits in Claude Code's CACHED prefix
# (marginal per-turn cost ~0 after turn 1), while memory re-injects each turn,
# so a naive token comparison makes memory look worse. This harness resolves it
# by measuring, per arm and across a corpus ladder, the cached-vs-uncached input
# token breakdown that `claude -p --output-format stream-json --verbose` emits.
#
# Four arms (redesign §P1 dual baselines):
#   memory-on          — rusty-brain hooks+MCP; the target + N distractors are
#                        EXPLICITLY PLANTED via `rusty-brain remember` (redesign
#                        §P2 explicit-plant mode: bypasses the lossy auto-summary
#                        and isolates a clean retrieval + cache signal); the WORK
#                        session recalls via the deterministic UserPromptSubmit
#                        injection.
#   steelman-baseline  — no rusty-brain; the target + N distractors are written
#                        inline into CLAUDE.md (clean/current/complete). The
#                        honest best case for the cached prefix; the arm ADR-3's
#                        thresholds compare against.
#   realistic-baseline — no rusty-brain; N distractors in CLAUDE.md but the
#                        TARGET IS OMITTED (incomplete docs — the common reality).
#   memory-off         — nothing; the floor.
#
# Corpus ladder (default 0 50 200 500 1000): N DISTRACTORS plus one shared
# target, so corpus=0 is the single-fact case (ADR-1's parity baseline). At
# large N a buried target is where memory's retrieval edge should bite and a
# giant CLAUDE.md should start paying for its cache.
#
# Per WORK session the harness records: num_turns, total_cost_usd, AND the
# session-aggregate usage.{input_tokens, cache_creation_input_tokens,
# cache_read_input_tokens, output_tokens} — the cache economics ADR-3 needs.
# The judge is the same deterministic expect-substring as w35-ab-eval.sh (the
# model's OUTPUT only: final .result + files written this session, never the
# seeded CLAUDE.md). The TSV carries ONLY numbers + the success flag — no model
# free-text — so it is safe to upload as a CI artifact (same posture as the
# other w35 harnesses).
#
# PASS verdicts are computed per corpus size against ADR-3's pre-registered
# thresholds (memory-on vs steelman-baseline): see aggregate() below.
#
# STATUS: harness + pure --self-test landed; the measured run is DEFERRED — it
# needs `claude` auth + a small spend budget + a release build. Run it via:
#   cargo build --release -p rusty-brain -p rb-hooks -p rb-install
#   scripts/w35-cache-trace.sh --bin-dir target/release
# `--self-test` validates the usage-extraction + scoring math with NO API and is
# CI-safe.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS_JSON="$REPO_ROOT/crates/rb-eval/scorecard/w35_ab_scenarios.json"
MODEL="${RB_W35_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_W35_MAX_BUDGET_USD:-0.50}"
# A failed session scores this many turns — a worst-case sentinel, well above a
# normal task, so a failure can never make an arm look faster on the turns/cache
# tiebreakers. Defined early: extract_usage (exercised by --self-test) reads it.
TURNS_FAIL_SENTINEL="${RB_W35_TURNS_FAIL_SENTINEL:-99}"

usage() { sed -n '2,46p' "$0"; exit 2; }

# --- usage extraction (pure; exercised by --self-test) -----------------------
# extract_usage <stream-json-log>
# Echoes one TSV row (7 fields), reading the session-AGGREGATE usage from the
# final `result` record (the same record that carries num_turns/total_cost_usd;
# it sums every turn's usage, so it is the session-total cache economics):
#   is_error  num_turns  cost  input_tok  cache_create_tok  cache_read_tok  output_tok
# A non-parseable log (timeout/error/budget) yields a worst-case sentinel row so
# a failure can never make an arm look artificially cheap on the cache metrics:
# turns=TURNS_FAIL_SENTINEL, every token field 0, is_error=true.
extract_usage() {
  local log="$1"
  local r is_err turns cost inp cc cr out
  r="$(jq -c 'select(.type=="result")' "$log" 2>/dev/null | tail -1)"
  if [ -n "$r" ]; then
    # `// true` would collapse an explicit false (jq's alt treats false as
    # empty), so gate on field presence: absent is_error => assume error.
    is_err="$(jq -r 'if has("is_error") then .is_error else true end' <<<"$r")"
    turns="$(jq -r '.num_turns // 0'           <<<"$r")"
    cost="$(jq -r '.total_cost_usd // 0'       <<<"$r")"
    inp="$(jq -r '.usage.input_tokens // 0'                <<<"$r")"
    cc="$(jq -r  '.usage.cache_creation_input_tokens // 0' <<<"$r")"
    cr="$(jq -r  '.usage.cache_read_input_tokens // 0'     <<<"$r")"
    out="$(jq -r '.usage.output_tokens // 0'   <<<"$r")"
  else
    is_err=true; turns="$TURNS_FAIL_SENTINEL"; cost=0; inp=0; cc=0; cr=0; out=0
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$is_err" "$turns" "$cost" "$inp" "$cc" "$cr" "$out"
}

# --- success judge (pure; exercised by --self-test) --------------------------
# judge_text <textfile> <expect> <forbid> <stale_token> <arm> -> echoes "<success> <mie>".
# Verbatim from scripts/w35-ab-eval.sh: expect-substring success (forbid absent
# => no forbid check); a memory-induced error keys ONLY on the memory-on arm
# failing AND the stale token being present. See w35-ab-eval.sh for rationale.
judge_text() {
  local file="$1" expect="$2" forbid="$3" stale="$4" arm="$5"
  local success=0 mie=0
  if grep -iqF -- "$expect" "$file" 2>/dev/null; then success=1; fi
  if [ -n "$forbid" ] && grep -iqF -- "$forbid" "$file" 2>/dev/null; then success=0; fi
  if [ "$arm" = "memory-on" ] && [ "$success" -eq 0 ] && [ -n "$stale" ] \
     && grep -iqF -- "$stale" "$file" 2>/dev/null; then mie=1; fi
  echo "$success $mie"
}

# --- aggregate + ADR-3 verdicts (pure; exercised by --self-test) -------------
# Reads the results TSV:
#   scenario<TAB>arm<TAB>corpus<TAB>run<TAB>success<TAB>turns<TAB>cost<TAB>input_tok<TAB>cache_create_tok<TAB>cache_read_tok<TAB>output_tok<TAB>mie
# Prints a per-(arm,corpus) cache-economics table, then a per-corpus verdict
# applying ADR-3's pre-registered thresholds (memory-on vs steelman-baseline).
aggregate() {
  local tsv="$1" mie_allowed="${2:-0}"
  awk -F'\t' -v mie_allowed="$mie_allowed" '
    function ratio(cr, inp) { if (cr+inp == 0) return 0; return cr/(cr+inp) }
    {
      arm=$2; c=$3; succ=$5; turns=$6; cost=$7; inp=$8; cc=$9; cr=$10; mie=$12;
      key=arm SUBSEP c;
      n[key]++; s[key]+=succ; t[key]+=turns; co[key]+=cost;
      ti[key]+=inp; tcc[key]+=cc; tcr[key]+=cr;
      if (arm=="memory-on" && mie==1) { mie_total++; mie_list = mie_list sprintf("    - %s corpus %s run %s\n", $1, c, $4); }
      corpora[c]=1;
    }
    END {
      narms = split("memory-on steelman-baseline realistic-baseline memory-off", arms, " ");
      # sort corpus sizes ascending
      ncorp=0; for (c in corpora) { ncorp++; corp[ncorp]=c }
      # insertion sort (small N)
      for (i=2;i<=ncorp;i++) { v=corp[i]; j=i-1; while (j>=1 && corp[j]+0 > v+0) { corp[j+1]=corp[j]; j-- } corp[j+1]=v }
      printf "%-20s %6s %5s %8s %9s %10s %10s %10s %9s\n", "arm/corpus", "runs", "succ", "mturns", "mcost$", "m_input", "m_ccrea", "m_cread", "cache%";
      for (ci=1; ci<=ncorp; ci++) {
        c=corp[ci];
        for (ai=1; ai<=narms; ai++) {
          a=arms[ai]; key=a SUBSEP c;
          if (n[key]==0) continue;
          printf "%-12s/%-6s %6d %4.0f%% %9.2f %9.4f %10.0f %10.0f %10.0f %8.1f\n", \
            a, c, n[key], 100*s[key]/n[key], t[key]/n[key], co[key]/n[key], \
            ti[key]/n[key], tcc[key]/n[key], tcr[key]/n[key], \
            100*ratio(tcr[key], ti[key]);
        }
      }
      print "";
      # ADR-3 verdicts: memory-on vs steelman-baseline, per corpus.
      print "== ADR-3 verdicts (memory-on vs steelman-baseline; thresholds pre-registered) ==";
      for (ci=1; ci<=ncorp; ci++) {
        c=corp[ci];
        onk ="memory-on" SUBSEP c;          stl="steelman-baseline" SUBSEP c;
        if (n[onk]==0 || n[stl]==0) { printf "  corpus %-6s SKIP (an arm produced no runs)\n", c; continue }
        on_acc = s[onk]/n[onk];             stl_acc = s[stl]/n[stl];
        on_cr  = ratio(tcr[onk], ti[onk]);  stl_cr  = ratio(tcr[stl], ti[stl]);
        acc_ok = (on_acc >= stl_acc);
        # threshold 2 "within 20%" = memory-on cache% >= 0.8 * steelman cache%
        cache_ok = (stl_cr == 0 ? (on_cr == 0) : (on_cr >= 0.8 * stl_cr));
        printf "  corpus %-6s acc on %.2f vs stl %.2f [%s] | cache%% on %.1f vs stl %.1f [%s]\n", \
          c, on_acc, stl_acc, (acc_ok?"ok":"NO"), 100*on_cr, 100*stl_cr, (cache_ok?"ok":"NO");
        if (acc_ok && cache_ok) {
          verdict="RATIFY Opt 3 (accuracy + cache within 20% of steelman)"
        } else if (acc_ok && !cache_ok) {
          verdict="Opt 2 candidate (accuracy wins; cache ratio > 20% worse — consider caching the SessionStart digest)"
        } else if (!acc_ok && cache_ok) {
          verdict="accuracy loses at scale (cache is fine) — investigate retrieval, not caching"
        } else {
          verdict="descope dimension A token-cost axis (value is capture/freshness/reach per ADR-1)"
        }
        printf "             -> %s\n", verdict;
      }
      print "";
      mie_ok = (mie_total+0 <= mie_allowed+0);
      printf "memory-induced errors: %d (allowed %d)\n", mie_total+0, mie_allowed+0;
      if (mie_total+0 > 0) { printf "  enumerated (never averaged away):\n%s", mie_list; }
      # The hard gate is still zero memory-induced errors (the safety property).
      printf "p4a cache study: %s\n", (mie_ok ? "cache-economics recorded (see verdicts above)" : "FAIL (memory-induced errors)");
      exit (mie_ok ? 0 : 1);
    }
  ' "$tsv"
}

# --- corpus generator (pure; deterministic; exercised indirectly) ------------
# gen_corpus <scenario_id> <n>  -> prints n markdown-bullet distractor lines.
# Deterministic (seeded by scenario_id) so re-runs are reproducible. Distractors
# are deliberately OFF-TOPIC from any scenario target (port numbers for fictional
# services) so they never accidentally contain an `expect`/`stale` token.
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

# --- self-test (no API, no binaries) -----------------------------------------
self_test() {
  echo "== P4a self-test (usage extraction + ADR-3 math; no API) =="
  local tmp fail=0
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/rb-p4a-selftest.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  # --- extract_usage on a well-formed stream-json log. ---
  local log="$tmp/work.jsonl"
  # A result record with realistic cache numbers (turn1 writes the cache,
  # turn2+ reads it) plus a couple of unrelated records that must be ignored.
  {
    printf '{"type":"system","subtype":"init"}\n'
    printf '{"type":"assistant","message":{"usage":{"input_tokens":1500,"cache_read_input_tokens":0,"cache_creation_input_tokens":4000,"output_tokens":120}}}\n'
    printf '{"type":"assistant","message":{"usage":{"input_tokens":60,"cache_read_input_tokens":5400,"cache_creation_input_tokens":0,"output_tokens":40}}}\n'
    printf '{"type":"result","subtype":"success","is_error":false,"num_turns":2,"total_cost_usd":0.0123,"usage":{"input_tokens":1560,"cache_creation_input_tokens":4000,"cache_read_input_tokens":5400,"output_tokens":160},"result":"use ureq for HTTP"}\n'
  } > "$log"
  local got want
  got="$(extract_usage "$log")"
  want=$'false\t2\t0.0123\t1560\t4000\t5400\t160'
  if [ "$got" = "$want" ]; then echo "ok: extract_usage reads aggregate cache fields"; else echo "BUG: extract_usage (want [$want] got [$got])"; fail=1; fi

  # --- extract_usage on a non-parseable log (timeout/error) -> sentinel row. ---
  : > "$tmp/empty.jsonl"
  got="$(extract_usage "$tmp/empty.jsonl")"
  want=$'true\t'"$TURNS_FAIL_SENTINEL"$'\t0\t0\t0\t0\t0'
  if [ "$got" = "$want" ]; then echo "ok: extract_usage sentinel on empty log"; else echo "BUG: extract_usage empty (want [$want] got [$got])"; fail=1; fi

  # --- judge_text (parity smoke with w35-ab-eval; one case per branch). ---
  printf 'use the ureq crate\n' > "$tmp/n.txt"
  printf 'use reqwest\n'        > "$tmp/old.txt"
  check() { if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "BUG: $1 (want [$2] got [$3])"; fail=1; fi; }
  check "judge success"             "1 0" "$(judge_text "$tmp/n.txt"   ureq ''   ''      memory-on)"
  check "judge mie on fail+stale"   "0 1" "$(judge_text "$tmp/old.txt" ureq ''   reqwest memory-on)"
  check "judge no mie off-arm"      "0 0" "$(judge_text "$tmp/old.txt" ureq ''   reqwest steelman-baseline)"

  # --- aggregate: ADR-3 threshold 1 (RATIFY Opt 3) at a corpus size. ---
  # memory-on ties steelman on accuracy AND cache% within 20% -> ratify.
  local tsv="$tmp/ratify.tsv"
  {
    printf 's1\tmemory-on\t50\t1\t1\t3\t0.02\t600\t4000\t4400\t100\t0\n'
    printf 's1\tsteelman-baseline\t50\t1\t1\t3\t0.02\t300\t4000\t8700\t100\t0\n'   # cache% 96.7
    printf 's1\trealistic-baseline\t50\t1\t0\t5\t0.02\t300\t4000\t8700\t100\t0\n'
    printf 's1\tmemory-off\t50\t1\t0\t5\t0.01\t6000\t0\t0\t100\t0\n'
  } > "$tsv"
  local verdict
  verdict="$(aggregate "$tsv" 0 2>/dev/null | grep -F -A1 -- 'corpus 50' | tail -1)"
  if printf '%s' "$verdict" | grep -qF "RATIFY Opt 3"; then echo "ok: ratify Opt 3 when acc+cache within bounds"; else echo "BUG: ratify verdict (got [$verdict])"; fail=1; fi

  # --- aggregate: ADR-3 threshold 2 (Opt 2 candidate) — cache ratio > 20% worse. ---
  # memory-on cache% 88, steelman cache% 99 -> 88 < 0.8*99=79.2? no, 88>=79.2 so still ok.
  # Push memory-on cache% down to 60 to force the Opt-2 branch: steelman 99, mem 60 -> 60<79.2 -> candidate.
  local tsv2="$tmp/opt2.tsv"
  {
    printf 's1\tmemory-on\t500\t1\t1\t3\t0.02\t4000\t4000\t6000\t100\t0\n'          # cache% 60
    printf 's1\tsteelman-baseline\t500\t1\t1\t3\t0.02\t100\t4000\t9900\t100\t0\n'   # cache% 99
    printf 's1\trealistic-baseline\t500\t1\t0\t5\t0.02\t100\t4000\t9900\t100\t0\n'
    printf 's1\tmemory-off\t500\t1\t0\t5\t0.01\t10000\t0\t0\t100\t0\n'
  } > "$tsv2"
  verdict="$(aggregate "$tsv2" 0 2>/dev/null | grep -F -A1 -- 'corpus 500' | tail -1)"
  if printf '%s' "$verdict" | grep -qF "Opt 2 candidate"; then echo "ok: Opt 2 candidate when cache ratio > 20% worse"; else echo "BUG: opt2 verdict (got [$verdict])"; fail=1; fi

  # --- aggregate: a memory-induced error fails the hard gate. ---
  local tsv3="$tmp/mie.tsv"
  {
    printf 's1\tmemory-on\t50\t1\t0\t3\t0.02\t600\t4000\t4400\t100\t1\n'
    printf 's1\tsteelman-baseline\t50\t1\t1\t3\t0.02\t300\t4000\t8700\t100\t0\n'
    printf 's1\tmemory-off\t50\t1\t0\t5\t0.01\t6000\t0\t0\t100\t0\n'
  } > "$tsv3"
  if aggregate "$tsv3" 0 >/dev/null 2>&1; then echo "BUG: mie did not fail the hard gate"; fail=1; else echo "ok: mie fails the hard gate"; fi

  # --- gen_corpus is deterministic + off-topic. ---
  local a b
  a="$(gen_corpus dep-http-ureq 3)"
  b="$(gen_corpus dep-http-ureq 3)"
  if [ "$a" = "$b" ]; then echo "ok: gen_corpus deterministic"; else echo "BUG: gen_corpus not deterministic"; fail=1; fi
  if printf '%s' "$a" | grep -qiwF "ureq"; then echo "BUG: gen_corpus leaked the target token"; fail=1; else echo "ok: gen_corpus off-topic"; fi

  [ "$fail" -eq 0 ] && { echo "self-test PASS"; return 0; } || { echo "self-test FAIL" >&2; return 1; }
}

# --- arg parsing -------------------------------------------------------------
BIN_DIR=""
MODE="run"
RUNS=""
REPORT_TSV=""
CORPUS_SIZES="0 50 200 500 1000"
SCENARIOS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)    MODE="self-test"; shift ;;
    --aggregate)    MODE="aggregate"; REPORT_TSV="${2:?--aggregate needs a value}"; shift 2 ;;
    --bin-dir)      BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)         RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --out)          REPORT_TSV="${2:?--out needs a value}"; shift 2 ;;
    --corpus-sizes) CORPUS_SIZES="${2:?--corpus-sizes needs a value}"; shift 2 ;;
    --scenarios)    SCENARIOS="${2:?--scenarios needs a value}"; shift 2 ;;
    -h|--help)      usage ;;
    *)              usage ;;
  esac
done

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi

# Re-score a saved results TSV (e.g. an uploaded nightly artifact) without
# re-running any sessions. Reads MIE_ALLOWED from the scenarios file when present.
if [ "$MODE" = "aggregate" ]; then
  [ -f "$REPORT_TSV" ] || { echo "tsv not found: $REPORT_TSV" >&2; exit 1; }
  mie_default=0
  [ -f "$SCENARIOS_JSON" ] && mie_default="$(jq -r '.config.memory_induced_errors_allowed // 0' "$SCENARIOS_JSON" 2>/dev/null || echo 0)"
  aggregate "$REPORT_TSV" "$mie_default"
  exit $?
fi

# --- real-run prerequisites --------------------------------------------------
[ -n "$BIN_DIR" ] || usage
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
for bin in rusty-brain rusty-brain-hooks rusty-brain-install; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing binary: $BIN_DIR/$bin (cargo build --release -p rusty-brain -p rb-hooks -p rb-install)" >&2; exit 1; }
done
command -v claude  >/dev/null 2>&1 || { echo "claude not on PATH (npm install -g @anthropic-ai/claude-code)" >&2; exit 1; }
command -v jq      >/dev/null 2>&1 || { echo "jq not on PATH (needed to parse stream-json usage)" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH (corpus generator + settings.json edit)" >&2; exit 1; }
[ -n "${ANTHROPIC_API_KEY:-${CLAUDE_CODE_API_KEY:-${CLAUDE_API_KEY:-}}}" ] \
  || { echo "no Anthropic API credential in env (ANTHROPIC_API_KEY / CLAUDE_CODE_API_KEY) — the measured run is DEFERRED until one is set" >&2; exit 1; }
[ -f "$SCENARIOS_JSON" ] || { echo "scenarios file not found: $SCENARIOS_JSON" >&2; exit 1; }

# Validate free-form inputs (never interpolate a dispatch value into a shell).
case "$CORPUS_SIZES" in *[!0-9\ ]*) echo "--corpus-sizes may contain only digits and spaces" >&2; exit 2 ;; esac
for cs in $CORPUS_SIZES; do
  case "$cs" in ''|*[!0-9]*) echo "--corpus-sizes has a non-numeric entry: '$cs'" >&2; exit 2 ;; esac
done
# Reject whitespace-only input (e.g. "   "): it passes the charset check above
# but yields zero cells — a silent no-op run.
case "$CORPUS_SIZES" in *[0-9]*) ;; *) echo "--corpus-sizes must contain at least one number" >&2; exit 2 ;; esac

# Default scenarios: one stale-trap + two plain (cheap, exercises both judge paths).
[ -n "$SCENARIOS" ] || SCENARIOS="dep-http-ureq config-settings-toml stale-trap-http-switch"
case "$SCENARIOS" in *[!a-z0-9\ _-]*) echo "--scenarios may contain only [a-z0-9 _-]" >&2; exit 2 ;; esac
case "$SCENARIOS" in *[a-z0-9]*) ;; *) echo "--scenarios must contain at least one id" >&2; exit 2 ;; esac

[ -n "$RUNS" ] || RUNS="$(jq -r '.config.runs_per_scenario // 3' "$SCENARIOS_JSON")"
MIE_ALLOWED="$(jq -r '.config.memory_induced_errors_allowed // 0' "$SCENARIOS_JSON")"

# Per-session wall-clock cap (defense against a hung session stalling the run).
SESSION_TIMEOUT=""
if command -v timeout  >/dev/null 2>&1; then SESSION_TIMEOUT="timeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"
elif command -v gtimeout >/dev/null 2>&1; then SESSION_TIMEOUT="gtimeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"; fi

# Hermeticity (mirrors w35-ab-eval.sh): clear any rusty-brain path/namespace
# overrides inherited from the environment so the baselines cannot silently gain
# memory. The memory-on arm RE-sets these in its own subshell per (scenario, run).
unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE RUSTY_BRAIN_IDLE_TIMEOUT_SECS

WORKROOT="$(mktemp -d "${TMPDIR:-/tmp}/rb-p4a-cache.XXXXXX")"
RESULTS="${REPORT_TSV:-$WORKROOT/results.tsv}"
: > "$RESULTS"
cleanup() { rm -rf "$WORKROOT" 2>/dev/null || true; }
trap cleanup EXIT

# Seed Claude Code's onboarding flag in a fresh HOME so headless mode never
# stalls on first-run setup.
seed_home() { mkdir -p "$1"; printf '{"hasCompletedOnboarding": true}\n' > "$1/.claude.json"; }

# Run ONE headless session whose stdout is captured to $log. Output format is
# stream-json --verbose so the per-turn + result usage records (with the cache
# token breakdown) are captured. Cheap model + hard spend cap + wall-clock cap.
run_session() {
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

# Install rusty-brain hooks + project MCP into a memory-on session's project
# (verbatim from w35-ab-eval.sh; proven hermetic install path).
install_rusty_brain() {
  local home="$1" proj="$2"
  (
    export HOME="$home"
    export PATH="$BIN_DIR:$PATH"
    cd "$proj"
    rusty-brain-install install --agents claude-code >/dev/null
    python3 - "$proj/.claude/settings.json" "$proj/.mcp.json" "$BIN_DIR/rusty-brain" <<'PY'
import json, sys
settings_path, mcp_path, server_cmd = sys.argv[1], sys.argv[2], sys.argv[3]
with open(mcp_path, "w") as f:
    json.dump({"mcpServers": {"rusty-brain": {"command": server_cmd, "args": ["mcp"]}}}, f, indent=2)
    f.write("\n")
with open(settings_path) as f:
    s = json.load(f)
s["enableAllProjectMcpServers"] = True
with open(settings_path, "w") as f:
    json.dump(s, f, indent=2)
    f.write("\n")
PY
  )
}

# Bulk-plant the corpus into the memory-on store via direct `rusty-brain remember`
# CLI calls (redesign §P2 explicit-plant mode). The caller (run_cell's memory-on
# subshell) exports RUSTY_BRAIN_SOCKET/DB/NAMESPACE, so the first call auto-starts
# the daemon on the shared socket and later calls + the WORK session's hooks all
# reach the SAME store. target at importance 8 so it surfaces in recall;
# distractors at importance 5.
plant_corpus() { # <target_text> <scenario_id> <n_distractors>
  local target="$1" sid="$2" n="$3"
  local cmd="$BIN_DIR/rusty-brain"
  "$cmd" remember "$target" --importance 8 >/dev/null 2>&1 || true
  if [ "$n" -gt 0 ]; then
    local facts
    facts="$(gen_corpus "$sid" "$n")"
    while IFS= read -r fact; do
      [ -n "$fact" ] || continue
      "$cmd" remember "${fact#- }" --importance 5 >/dev/null 2>&1 || true
    done <<<"$facts"
  fi
}

# Run the WORK session for one arm and append its result row to $RESULTS.
# Judge text = the model's OUTPUT only: the final `.result` from the stream-json
# result record + files the model WROTE DURING this session (mtime newer than a
# marker, size-capped). Pre-seeded files (notably the steelman CLAUDE.md, which
# holds the target) are EXCLUDED. Cache fields come from extract_usage.
score_work() { # <id> <arm> <corpus> <run> <proj> <home> <work> <expect> <forbid> <stale>
  local id="$1" arm="$2" corpus="$3" run="$4" proj="$5" home="$6" work="$7" \
        expect="$8" forbid="$9" stale="${10}"
  local log="$proj/../work.jsonl" text="$proj/../judge.txt" marker="$proj/../work.start"
  : > "$marker"
  run_session "$home" "$proj" "$work" "$log" --output-format stream-json --verbose
  # Judge text starts at the final .result (empty/absent => judge the raw log,
  # which fails success and is never silently skipped).
  if jq -e 'select(.type=="result")|.result' "$log" >/dev/null 2>&1; then
    jq -r 'select(.type=="result")|.result' "$log" | tail -1 > "$text"
  else
    cp "$log" "$text"
  fi
  find "$proj" -type f -not -path '*/.*' -newer "$marker" -size -256k -print0 2>/dev/null \
    | xargs -0 cat >> "$text" 2>/dev/null || true
  local u success mie
  u="$(extract_usage "$log")"           # is_err turns cost input cc cr out
  local is_err turns cost inp cc cr out
  is_err="$(cut -f1 <<<"$u")"; turns="$(cut -f2 <<<"$u")"; cost="$(cut -f3 <<<"$u")"
  inp="$(cut -f4 <<<"$u")"; cc="$(cut -f5 <<<"$u")"; cr="$(cut -f6 <<<"$u")"; out="$(cut -f7 <<<"$u")"
  # success = expect-judged correctness; a server-level is_error forces failure.
  local res; res="$(judge_text "$text" "$expect" "$forbid" "$stale" "$arm")"
  success="${res% *}"; mie="${res#* }"
  if [ "$is_err" = "true" ]; then success=0; fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$arm" "$corpus" "$run" "$success" "$turns" "$cost" \
    "$inp" "$cc" "$cr" "$out" "$mie" >> "$RESULTS"
  printf '   [%-18s c=%-4s] succ=%s turns=%s cost=%s cache%%=%.1f (cr=%s/%s)\n' \
    "$arm" "$corpus" "$success" "$turns" "$cost" \
    "$(awk -v cr="$cr" -v inp="$inp" 'BEGIN{ if(cr+inp==0) print 0; else printf "%.1f", 100*cr/(cr+inp) }')" \
    "$cr" "$((cr + inp))"
}

run_cell() { # <id> <run> <corpus> <plant> <claude_md> <work> <expect> <forbid> <stale>
  local id="$1" run="$2" corpus="$3" plant="$4" claude_md="$5" work="$6" \
        expect="$7" forbid="$8" stale="$9"
  local n=$((corpus))   # distractor count
  local base="$WORKROOT/$id-c${corpus}-r$run"
  local distractors=""
  if [ "$n" -gt 0 ]; then distractors="$(gen_corpus "$id" "$n")"; fi

  # --- arm: memory-on (explicit plant of target + N distractors) -------------
  # Short /tmp socket dir (a long $WORKROOT path can exceed macOS's ~104-byte
  # sun_path limit — the nightly-claude-smoke.sh socket-length note).
  local mbase="$base/on"
  local sockdir; sockdir="$(mktemp -d "/tmp/rbp4a.XXXXXX")"
  local sock="$sockdir/s" db="$base/on/memory.db" ns="rb-p4a-$id-c${corpus}-r$run"
  local wh="$mbase/home" wp="$mbase/proj"
  seed_home "$wh"; mkdir -p "$wp"
  install_rusty_brain "$wh" "$wp"
  # Confound strip (fairness, same as w35-ab-eval): remove the installer's
  # "recall before work" CLAUDE.md block + skill so the WORK session gets the
  # target ONLY through the deterministic UserPromptSubmit recall injection.
  rm -f "$wp/CLAUDE.md"
  rm -rf "$wp/.claude/skills"
  (
    # Scope the per-run store to this subshell (so the baselines below cannot
    # inherit it) AND reach the WORK session's hooks: run_session's nested
    # subshell inherits these, so the UserPromptSubmit recall hook connects to
    # the PLANTED store. Without this export, recall would hit the empty default
    # store and memory-on would silently run without memory — the exact
    # invalidity the redesign warns about. Mirrors w35-ab-eval.sh.
    export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
    plant_corpus "$plant" "$id" "$n"
    # The first remember must have auto-started the daemon (pidfile = socket.pid).
    # If it did not, the memory-on arm would silently run WITHOUT memory and the
    # whole comparison would be invalid — fail loudly.
    [ -f "$sock.pid" ] || { echo "ERROR: memory-on daemon did not auto-start for $id c=$corpus run $run" >&2; exit 1; }
    score_work "$id" "memory-on" "$corpus" "$run" "$wp" "$wh" "$work" "$expect" "$forbid" "$stale"
  )
  [ -f "$sock.pid" ] && kill "$(cat "$sock.pid" 2>/dev/null)" 2>/dev/null || true
  rm -rf "$sockdir" 2>/dev/null || true

  # --- arm: steelman-baseline (target + distractors in CLAUDE.md) ------------
  local sh="$base/stl/home" sp="$base/stl/proj"
  seed_home "$sh"; mkdir -p "$sp"
  { printf '# Project conventions\n\n%s\n\n## Other decisions\n' "$claude_md";
    [ -n "$distractors" ] && printf '%s\n' "$distractors"; } > "$sp/CLAUDE.md"
  score_work "$id" "steelman-baseline" "$corpus" "$run" "$sp" "$sh" "$work" "$expect" "$forbid" "$stale"

  # --- arm: realistic-baseline (distractors only; target OMITTED) ------------
  local rh="$base/real/home" rp="$base/real/proj"
  seed_home "$rh"; mkdir -p "$rp"
  { printf '# Project conventions\n\n## Other decisions\n';
    [ -n "$distractors" ] && printf '%s\n' "$distractors"; } > "$rp/CLAUDE.md"
  score_work "$id" "realistic-baseline" "$corpus" "$run" "$rp" "$rh" "$work" "$expect" "$forbid" "$stale"

  # --- arm: memory-off (nothing) ---------------------------------------------
  local oh="$base/off/home" op="$base/off/proj"
  seed_home "$oh"; mkdir -p "$op"
  score_work "$id" "memory-off" "$corpus" "$run" "$op" "$oh" "$work" "$expect" "$forbid" "$stale"
}

echo "== P4a cache study (model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
echo "   scenarios: $SCENARIOS"
echo "   corpus sizes (distractors): $CORPUS_SIZES"
for id in $SCENARIOS; do
  row="$(jq -c --arg id "$id" '.scenarios[]|select(.id==$id)' "$SCENARIOS_JSON")"
  [ -n "$row" ] || { echo "scenario not found: $id" >&2; continue; }
  plant="$(jq -r '.plant' <<<"$row")"
  claude_md="$(jq -r '.claude_md' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"
  expect="$(jq -r '.expect' <<<"$row")"
  forbid="$(jq -r '.forbid // ""' <<<"$row")"
  stale="$(jq -r '.stale_token // ""' <<<"$row")"
  for corpus in $CORPUS_SIZES; do
    run=1
    while [ "$run" -le "$RUNS" ]; do
      echo "-- $id (corpus $corpus, run $run/$RUNS)"
      run_cell "$id" "$run" "$corpus" "$plant" "$claude_md" "$work" "$expect" "$forbid" "$stale"
      run=$((run + 1))
    done
  done
done

echo
echo "== results =="
aggregate "$RESULTS" "$MIE_ALLOWED"
echo
echo "report: $RESULTS"
