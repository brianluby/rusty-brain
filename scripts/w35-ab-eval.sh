#!/usr/bin/env bash
# W3.5 A/B outcome eval (plan §6 / W3.5; F56). Does memory actually make the
# agent DO BETTER — not just use the tools? For each scenario
# (crates/rb-eval/scorecard/w35_ab_scenarios.json) it runs the same WORK task
# under THREE arms and judges the outcome deterministically:
#
#   memory-on   — rusty-brain hooks+MCP installed; a PLANT session stores the
#                 project decision, the WORK session (fresh context, same
#                 namespace) recalls it.
#   memory-off  — no rusty-brain, no decision anywhere: the baseline floor.
#   claude-md   — no rusty-brain, but the decision is written into the project
#                 CLAUDE.md: the NATIVE baseline the value-over-native proof
#                 (F56) must beat.
#
# Each WORK session runs `claude -p --output-format json`, so num_turns and
# total_cost_usd come straight from the CLI. The judge is deterministic: the
# WORK output (final answer + files written) must contain the scenario's
# `expect` token and not its `forbid` token (case-insensitive substring). For
# `stale_trap` scenarios the memory-on arm also checks `stale_token` — its
# presence means the model acted on a stale/wrong memory: a MEMORY-INDUCED
# ERROR, enumerated per run, NEVER averaged into a rate.
#
# PASS (plan §6: "memory-on must win on success/turns within token budget"):
#   memory-on success_rate >= BOTH baselines, AND beats each baseline on
#   success-rate OR mean-turns, AND zero memory-induced errors.
#
# STATUS: harness landed; the measured run is DEFERRED — it needs `claude` +
# ANTHROPIC_API_KEY + a small spend budget (the W3.5 nightly infra). Run it via:
#   cargo build --release -p rusty-brain -p rb-hooks -p rb-install
#   scripts/w35-ab-eval.sh --bin-dir target/release
# `--self-test` validates the judge + scoring math with NO API and is CI-safe.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCENARIOS="$REPO_ROOT/crates/rb-eval/scorecard/w35_ab_scenarios.json"
MODEL="${RB_W35_MODEL:-haiku}"
MAX_BUDGET_USD="${RB_W35_MAX_BUDGET_USD:-0.50}"

usage() { sed -n '2,31p' "$0"; exit 2; }

# --- judge (pure; exercised by --self-test) ----------------------------------
# judge_text <textfile> <expect> <forbid> <stale_token> <arm>
# Echoes "<success> <mie>" (each 0/1). Case-insensitive substring matching.
#   success = expect present AND forbid absent (forbid empty => no forbid check)
#   mie     = a MEMORY-INDUCED ERROR: only on the memory-on arm, only when the
#             task FAILED (success==0) AND the stale token is present. Keying mie
#             on failure is deliberate: a substring grep cannot tell "acted on
#             the stale memory" from "mentioned the superseded value while
#             correctly choosing the new one" — so a CORRECT answer that merely
#             names the old value (success==1) is NOT a memory-induced error.
#             Stale-trap scenarios therefore set NO `forbid` (the old value
#             appearing in passing must not fail success); `stale_token` + the
#             success==0 gate carry the error signal instead.
judge_text() {
  local file="$1" expect="$2" forbid="$3" stale="$4" arm="$5"
  local success=0 mie=0
  if grep -iqF -- "$expect" "$file" 2>/dev/null; then
    success=1
  fi
  if [ -n "$forbid" ] && grep -iqF -- "$forbid" "$file" 2>/dev/null; then
    success=0
  fi
  if [ "$arm" = "memory-on" ] && [ "$success" -eq 0 ] && [ -n "$stale" ] \
     && grep -iqF -- "$stale" "$file" 2>/dev/null; then
    mie=1
  fi
  echo "$success $mie"
}

# --- aggregate + score (pure; exercised by --self-test) ----------------------
# Reads a results TSV (scenario<TAB>arm<TAB>run<TAB>success<TAB>turns<TAB>cost<TAB>mie)
# from $1, prints the per-arm report + PASS/FAIL, returns 0 on PASS else 1.
# `mie_allowed` ($2) caps tolerated memory-induced errors (default 0).
aggregate() {
  local tsv="$1" mie_allowed="${2:-0}"
  awk -F'\t' -v mie_allowed="$mie_allowed" '
    {
      arm=$2; succ=$4; turns=$5; cost=$6; mie=$7;
      n[arm]++; s[arm]+=succ; t[arm]+=turns; c[arm]+=cost;
      if (arm=="memory-on" && mie==1) { mie_total++; mie_list = mie_list sprintf("    - %s run %s\n", $1, $3); }
    }
    END {
      split("memory-on memory-off claude-md", order, " ");
      printf "%-12s %6s %8s %10s %12s\n", "arm", "runs", "success", "mean_turns", "mean_cost$";
      for (i=1;i<=3;i++) {
        a=order[i];
        if (n[a]==0) { printf "%-12s %6s %8s %10s %12s\n", a, 0, "n/a", "n/a", "n/a"; continue }
        rate[a]=s[a]/n[a]; mt[a]=t[a]/n[a]; mc[a]=c[a]/n[a];
        printf "%-12s %6d %7.2f%% %10.2f %12.4f\n", a, n[a], rate[a]*100, mt[a], mc[a];
      }
      print "";
      on=rate["memory-on"]; off=rate["memory-off"]; cm=rate["claude-md"];
      ont=mt["memory-on"]; offt=mt["memory-off"]; cmt=mt["claude-md"];
      if (n["memory-on"]==0 || n["memory-off"]==0 || n["claude-md"]==0) {
        print "score: FAIL (an arm produced no runs)"; exit 1;
      }
      success_ok = (on >= off && on >= cm);
      beats_off  = (on > off || ont < offt);
      beats_cm   = (on > cm  || ont < cmt);
      mie_ok     = (mie_total+0 <= mie_allowed+0);
      printf "memory-on >= both baselines on success: %s (on %.2f vs off %.2f, claude-md %.2f)\n", (success_ok?"yes":"NO"), on, off, cm;
      printf "memory-on beats memory-off (success or turns): %s\n", (beats_off?"yes":"NO");
      printf "memory-on beats claude-md (success or turns):  %s\n", (beats_cm?"yes":"NO");
      printf "memory-induced errors: %d (allowed %d)\n", mie_total+0, mie_allowed+0;
      if (mie_total+0 > 0) { printf "  enumerated (never averaged away):\n%s", mie_list; }
      pass = (success_ok && beats_off && beats_cm && mie_ok);
      printf "w35 a/b eval: %s\n", (pass?"PASS":"FAIL");
      exit (pass?0:1);
    }
  ' "$tsv"
}

# --- self-test (no API) ------------------------------------------------------
self_test() {
  echo "== W3.5 self-test (judge + scoring; no API) =="
  local tmp fail=0
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/rb-w35-selftest.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  # judge_text cases.
  printf 'use the ureq crate for HTTP\n' > "$tmp/new-only.txt"     # uses new, no old
  printf 'use reqwest for HTTP\n'        > "$tmp/old-only.txt"     # uses old, not new
  printf 'use ureq, never reqwest\n'     > "$tmp/new-names-old.txt" # uses new, names old in passing

  check() { # <desc> <expected "succ mie"> <actual>
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "BUG: $1 (want '$2' got '$3')"; fail=1; fi
  }
  # Stale-trap scenarios set NO forbid; success is expect-only, mie keys on failure.
  check "trap: new only => success, no mie"                 "1 0" "$(judge_text "$tmp/new-only.txt"      ureq '' reqwest memory-on)"
  check "trap: correct but names superseded value => success, NO mie" "1 0" "$(judge_text "$tmp/new-names-old.txt" ureq '' reqwest memory-on)"
  check "trap: acted on stale (old, not new) => fail + mie" "0 1" "$(judge_text "$tmp/old-only.txt"      ureq '' reqwest memory-on)"
  check "trap: acted-on-stale in claude-md => fail, no mie" "0 0" "$(judge_text "$tmp/old-only.txt"      ureq '' reqwest claude-md)"
  # Non-trap scenarios set forbid; no stale token => never an mie.
  check "non-trap: forbid present => fail, no mie"          "0 0" "$(judge_text "$tmp/old-only.txt"      ureq reqwest '' memory-on)"
  check "non-trap: honored => success, no mie"              "1 0" "$(judge_text "$tmp/new-only.txt"      ureq reqwest '' memory-on)"

  # aggregate: a PASSING result set (memory-on best, no MIE).
  pass_tsv="$tmp/pass.tsv"
  {
    printf 's1\tmemory-on\t1\t1\t3\t0.02\t0\n'
    printf 's1\tmemory-off\t1\t0\t5\t0.01\t0\n'
    printf 's1\tclaude-md\t1\t1\t4\t0.02\t0\n'
    printf 's2\tmemory-on\t1\t1\t3\t0.02\t0\n'
    printf 's2\tmemory-off\t1\t0\t5\t0.01\t0\n'
    printf 's2\tclaude-md\t1\t0\t4\t0.02\t0\n'
  } > "$pass_tsv"
  if aggregate "$pass_tsv" 0 >/dev/null; then echo "ok: passing result set passes"; else echo "BUG: passing result set failed"; fail=1; fi

  # aggregate: memory-on tied on success AND no fewer turns => FAIL (no win).
  nowin_tsv="$tmp/nowin.tsv"
  {
    printf 's1\tmemory-on\t1\t1\t5\t0.02\t0\n'
    printf 's1\tmemory-off\t1\t1\t5\t0.01\t0\n'
    printf 's1\tclaude-md\t1\t1\t5\t0.02\t0\n'
  } > "$nowin_tsv"
  if aggregate "$nowin_tsv" 0 >/dev/null; then echo "BUG: no-win set passed"; fail=1; else echo "ok: no-win set fails"; fi

  # aggregate: a single memory-induced error fails the run even though memory-on
  # otherwise wins on success-rate and turns (mie implies a failed run, so it
  # rides on the run2 row with success=0; memory-on still beats both baselines).
  mie_tsv="$tmp/mie.tsv"
  {
    printf 's1\tmemory-on\t1\t1\t2\t0.02\t0\n'
    printf 's1\tmemory-on\t2\t0\t2\t0.02\t1\n'
    printf 's1\tmemory-off\t1\t0\t5\t0.01\t0\n'
    printf 's1\tmemory-off\t2\t0\t5\t0.01\t0\n'
    printf 's1\tclaude-md\t1\t0\t5\t0.02\t0\n'
    printf 's1\tclaude-md\t2\t0\t5\t0.02\t0\n'
  } > "$mie_tsv"
  if aggregate "$mie_tsv" 0 >/dev/null; then echo "BUG: memory-induced error did not fail the run"; fail=1; else echo "ok: memory-induced error fails the run"; fi

  [ "$fail" -eq 0 ] && { echo "self-test PASS"; return 0; } || { echo "self-test FAIL" >&2; return 1; }
}

# --- arg parsing -------------------------------------------------------------
BIN_DIR=""
MODE="run"
RUNS=""
REPORT_TSV=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) MODE="self-test"; shift ;;
    --bin-dir)   BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
    --runs)      RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --out)       REPORT_TSV="${2:?--out needs a value}"; shift 2 ;;
    -h|--help)   usage ;;
    *)           usage ;;
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
command -v jq      >/dev/null 2>&1 || { echo "jq not on PATH (needed to read scenarios + parse --output-format json)" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 not on PATH (needed to edit .claude/settings.json)" >&2; exit 1; }
[ -n "${ANTHROPIC_API_KEY:-}" ] || { echo "ANTHROPIC_API_KEY is not set — the measured run is DEFERRED until it is" >&2; exit 1; }
[ -f "$SCENARIOS" ] || { echo "scenarios file not found: $SCENARIOS" >&2; exit 1; }

[ -n "$RUNS" ] || RUNS="$(jq -r '.config.runs_per_scenario // 3' "$SCENARIOS")"
MIE_ALLOWED="$(jq -r '.config.memory_induced_errors_allowed // 0' "$SCENARIOS")"

# Per-session wall-clock cap (defense against a hung session stalling the run).
# `timeout` is GNU coreutils (not in macOS base); use it or `gtimeout` if present,
# else run uncapped (the job-level timeout is the backstop). Empty -> no prefix.
SESSION_TIMEOUT=""
if command -v timeout >/dev/null 2>&1; then
  SESSION_TIMEOUT="timeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"
elif command -v gtimeout >/dev/null 2>&1; then
  SESSION_TIMEOUT="gtimeout ${RB_W35_SESSION_TIMEOUT_SECS:-300}"
fi

# Hermeticity (mirrors nightly-claude-smoke.sh): clear any rusty-brain path/
# namespace overrides inherited from the environment. The memory-on arm RE-sets
# these inside its own subshell per (scenario, run); clearing them here keeps the
# memory-off and claude-md baselines from inheriting a CI-set store and silently
# gaining memory. ANTHROPIC_API_KEY stays exported — claude needs it.
unset RUSTY_BRAIN_DB RUSTY_BRAIN_SOCKET RUSTY_BRAIN_NAMESPACE RUSTY_BRAIN_IDLE_TIMEOUT_SECS

WORKROOT="$(mktemp -d "${TMPDIR:-/tmp}/rb-w35-abeval.XXXXXX")"
RESULTS="${REPORT_TSV:-$WORKROOT/results.tsv}"
: > "$RESULTS"
cleanup() { rm -rf "$WORKROOT" 2>/dev/null || true; }
trap cleanup EXIT

# Seed Claude Code's onboarding flag in a fresh HOME so headless mode never
# stalls on first-run setup.
seed_home() { mkdir -p "$1"; printf '{"hasCompletedOnboarding": true}\n' > "$1/.claude.json"; }

# Run ONE headless session whose stdout is captured to $log. Extra args ($@)
# tune output format. The session runs with a cheap model + hard spend cap, and
# (when a `timeout` binary is available) a per-session wall-clock cap so one hung
# session cannot stall the whole multi-session run — the spend cap bounds cost,
# not wall-clock. A timed-out/failed session leaves a non-JSON log, which scores
# as a failure (never silently as success).
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

# Install rusty-brain hooks + project MCP into a memory-on session's project.
# Runs under the session's isolated HOME so a project install can never read or
# write the caller's real ~/.claude.
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

# Build the judge text for a WORK session: the final `.result` from the JSON
# output, plus any non-dotfile the model wrote in the project (the "output/diff"
# the plan's judge inspects). Robust to a non-JSON log (budget/error): falls
# back to the raw log so a failed parse never silently scores as success.
collect_judge_text() {
  local jsonlog="$1" proj="$2" out="$3"
  if jq -e '.result' "$jsonlog" >/dev/null 2>&1; then
    jq -r '.result' "$jsonlog" > "$out"
  else
    cp "$jsonlog" "$out"
  fi
  # Append files the model created/edited (skip dotdirs: .claude/.mcp.json/.git).
  find "$proj" -type f -not -path '*/.*' -print0 2>/dev/null \
    | xargs -0 cat >> "$out" 2>/dev/null || true
}

# Parse num_turns / total_cost_usd from a JSON-output log (0 when absent).
json_turns() { jq -r '.num_turns // 0'      "$1" 2>/dev/null || echo 0; }
json_cost()  { jq -r '.total_cost_usd // 0' "$1" 2>/dev/null || echo 0; }

# Run the WORK session for one arm and append its result row to $RESULTS.
score_work() {
  local id="$1" arm="$2" run="$3" proj="$4" home="$5" work="$6" \
        expect="$7" forbid="$8" stale="$9"
  local jlog="$proj/../work.json" jtext="$proj/../judge.txt"
  run_session "$home" "$proj" "$work" "$jlog" --output-format json
  collect_judge_text "$jlog" "$proj" "$jtext"
  local turns cost res
  turns="$(json_turns "$jlog")"; cost="$(json_cost "$jlog")"
  res="$(judge_text "$jtext" "$expect" "$forbid" "$stale" "$arm")"
  local success="${res% *}" mie="${res#* }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$id" "$arm" "$run" "$success" "$turns" "$cost" "$mie" >> "$RESULTS"
  echo "   [$arm] success=$success turns=$turns cost=$cost mie=$mie"
}

run_scenario_run() {
  local id="$1" run="$2" plant="$3" supersede="$4" claude_md="$5" work="$6" \
        expect="$7" forbid="$8" stale="$9"
  local base="$WORKROOT/$id-r$run"

  # --- arm: memory-on (plant -> [supersede] -> work, shared store) -----------
  local mbase="$base/on" sock="$base/on/sock" db="$base/on/memory.db" ns="rb-w35-$id-r$run"
  mkdir -p "$mbase"
  local plant_home="$mbase/home_plant" plant_proj="$mbase/proj_plant"
  local work_home="$mbase/home_work" work_proj="$mbase/proj_work"
  seed_home "$plant_home"; mkdir -p "$plant_proj"
  seed_home "$work_home";  mkdir -p "$work_proj"
  install_rusty_brain "$plant_home" "$plant_proj"
  install_rusty_brain "$work_home" "$work_proj"
  # Fairness (avoid a confound): the installer appends a "recall before work"
  # memory-policy block to CLAUDE.md + ships a skill (the W3.2(b) elicitation
  # channel). Strip BOTH from the WORK project so the memory-on work session
  # gets the decision ONLY through the deterministic UserPromptSubmit recall
  # injection (channel a — the actual store mechanism), not through extra prose
  # guidance the baselines lack. The PLANT project keeps the block (we want
  # capture to fire there). Project install writes <proj>/CLAUDE.md +
  # <proj>/.claude/skills (verified in rb-install claude_code.rs).
  rm -f "$work_proj/CLAUDE.md"
  rm -rf "$work_proj/.claude/skills"
  (
    export RUSTY_BRAIN_SOCKET="$sock" RUSTY_BRAIN_DB="$db" RUSTY_BRAIN_NAMESPACE="$ns"
    run_session "$plant_home" "$plant_proj" "$plant" "$mbase/plant.log"
    if [ -n "$supersede" ]; then
      run_session "$plant_home" "$plant_proj" "$supersede" "$mbase/supersede.log"
    fi
    # The PLANT session's SessionStart hook must have auto-started the daemon
    # (pidfile = <socket>.pid). If it did not, the memory-on arm would silently
    # run WITHOUT memory and the whole comparison would be invalid — fail loudly
    # (the nightly job's alert issue surfaces it), matching the smoke's contract.
    [ -f "$sock.pid" ] || {
      echo "ERROR: memory-on daemon did not auto-start for $id run $run (SessionStart hook?)" >&2
      exit 1
    }
    score_work "$id" "memory-on" "$run" "$work_proj" "$work_home" "$work" "$expect" "$forbid" "$stale"
  )
  # Reap the session-started daemon for this run's store.
  if [ -f "$sock.pid" ]; then kill "$(cat "$sock.pid" 2>/dev/null)" 2>/dev/null || true; fi

  # --- arm: claude-md-only (decision in CLAUDE.md, no rusty-brain) -----------
  local chome="$base/cmd/home" cproj="$base/cmd/proj"
  seed_home "$chome"; mkdir -p "$cproj"
  printf '# Project conventions\n\n%s\n' "$claude_md" > "$cproj/CLAUDE.md"
  score_work "$id" "claude-md" "$run" "$cproj" "$chome" "$work" "$expect" "$forbid" "$stale"

  # --- arm: memory-off (nothing) ---------------------------------------------
  local ohome="$base/off/home" oproj="$base/off/proj"
  seed_home "$ohome"; mkdir -p "$oproj"
  score_work "$id" "memory-off" "$run" "$oproj" "$ohome" "$work" "$expect" "$forbid" "$stale"
}

echo "== W3.5 A/B outcome eval (model=$MODEL, budget=\$$MAX_BUDGET_USD/session, runs=$RUNS) =="
rows=()
while IFS= read -r row; do rows+=("$row"); done < <(jq -c '.scenarios[]' "$SCENARIOS")
for row in "${rows[@]}"; do
  id="$(jq -r '.id' <<<"$row")"
  plant="$(jq -r '.plant' <<<"$row")"
  supersede="$(jq -r '.supersede // ""' <<<"$row")"
  claude_md="$(jq -r '.claude_md' <<<"$row")"
  work="$(jq -r '.work' <<<"$row")"
  expect="$(jq -r '.expect' <<<"$row")"
  forbid="$(jq -r '.forbid // ""' <<<"$row")"
  stale="$(jq -r '.stale_token // ""' <<<"$row")"
  run=1
  while [ "$run" -le "$RUNS" ]; do
    echo "-- scenario: $id (run $run/$RUNS)"
    run_scenario_run "$id" "$run" "$plant" "$supersede" "$claude_md" "$work" "$expect" "$forbid" "$stale"
    run=$((run + 1))
  done
done

echo
echo "== results =="
aggregate "$RESULTS" "$MIE_ALLOWED"
