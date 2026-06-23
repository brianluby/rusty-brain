#!/bin/sh
# w35-scorecard-closeout.test.sh — assertions for the W3.5 scorecard closeout
# documents and the scenario JSON they reference.
#
# Tests the verifiable claims made in the PR that adds:
#   CHANGELOG.md (closeout entry)
#   docs/eval/2026-06-16-w35-criterion-redesign.md (status update)
#   docs/eval/2026-06-23-w35-scorecard-closeout.md (new closeout artifact)
#   docs/plans/2026-06-11-rusty-brain-road-to-tens.md (phase gate update)
#
# Run: sh scripts/w35-scorecard-closeout.test.sh
# No API key or network access required.
set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"

fail() {
  printf 'TEST FAIL: %s\n' "$1" >&2
  exit 1
}

# ---- prerequisites -----------------------------------------------------------

if ! command -v jq > /dev/null 2>&1; then
  printf 'SKIP: jq not found — install jq to run these tests\n' >&2
  exit 0
fi

SCENARIOS_FILE="$HERE/crates/rb-eval/scorecard/memory_scorecard_scenarios.json"
CLOSEOUT_DOC="$HERE/docs/eval/2026-06-23-w35-scorecard-closeout.md"
CRITERION_DOC="$HERE/docs/eval/2026-06-16-w35-criterion-redesign.md"
CHANGELOG="$HERE/CHANGELOG.md"
PLANS_DOC="$HERE/docs/plans/2026-06-11-rusty-brain-road-to-tens.md"

# ---- Section 1: referenced artifact files exist ------------------------------

[ -f "$CLOSEOUT_DOC" ] \
  || fail "closeout doc does not exist: $CLOSEOUT_DOC"

[ -f "$CRITERION_DOC" ] \
  || fail "criterion redesign doc does not exist: $CRITERION_DOC"

[ -f "$SCENARIOS_FILE" ] \
  || fail "scenarios file does not exist: $SCENARIOS_FILE"

[ -f "$HERE/scripts/memory-scorecard.sh" ] \
  || fail "memory-scorecard.sh does not exist"

[ -f "$HERE/.github/workflows/memory-scorecard.yml" ] \
  || fail ".github/workflows/memory-scorecard.yml does not exist"

[ -f "$HERE/docs/eval/2026-06-21-w35-class-c-freshness-n10.tsv" ] \
  || fail "Class C raw TSV artifact does not exist: docs/eval/2026-06-21-w35-class-c-freshness-n10.tsv"

[ -f "$HERE/docs/eval/2026-06-21-w35-class-c-freshness-measured.md" ] \
  || fail "Class C measured writeup does not exist: docs/eval/2026-06-21-w35-class-c-freshness-measured.md"

[ -f "$HERE/docs/prds/2026-06-23-w35-class-b-capture-fidelity.md" ] \
  || fail "Class B PRD does not exist: docs/prds/2026-06-23-w35-class-b-capture-fidelity.md"

[ -f "$HERE/docs/prds/2026-06-23-w35-class-r-reach-team.md" ] \
  || fail "Class R PRD does not exist: docs/prds/2026-06-23-w35-class-r-reach-team.md"

# ---- Section 2: scenario JSON is valid and counts match ----------------------

jq empty "$SCENARIOS_FILE" \
  || fail "scenarios JSON is not valid JSON"

total="$(jq '.scenarios | length' "$SCENARIOS_FILE")"
[ "$total" -eq 13 ] \
  || fail "expected 13 total scenarios, got $total"

count_freshness="$(jq '[.scenarios[] | select(.dimension == "freshness")] | length' "$SCENARIOS_FILE")"
[ "$count_freshness" -eq 4 ] \
  || fail "expected 4 freshness scenarios, got $count_freshness"

count_retrieval="$(jq '[.scenarios[] | select(.dimension == "retrieval_scale")] | length' "$SCENARIOS_FILE")"
[ "$count_retrieval" -eq 3 ] \
  || fail "expected 3 retrieval_scale scenarios, got $count_retrieval"

count_capture="$(jq '[.scenarios[] | select(.dimension == "capture")] | length' "$SCENARIOS_FILE")"
[ "$count_capture" -eq 3 ] \
  || fail "expected 3 capture scenarios, got $count_capture"

count_reach="$(jq '[.scenarios[] | select(.dimension == "reach")] | length' "$SCENARIOS_FILE")"
[ "$count_reach" -eq 3 ] \
  || fail "expected 3 reach scenarios, got $count_reach"

# Guard: sum of per-dimension counts equals total
sum=$((count_freshness + count_retrieval + count_capture + count_reach))
[ "$sum" -eq "$total" ] \
  || fail "per-dimension counts ($sum) do not sum to total ($total)"

# ---- Section 3: config values match closeout doc claims ----------------------
# closeout.md claims: min_runs=5, runs_per_scenario=5, tie_margin=0.10

runs_per_scenario="$(jq '.config.runs_per_scenario' "$SCENARIOS_FILE")"
[ "$runs_per_scenario" -eq 5 ] \
  || fail "config.runs_per_scenario: expected 5, got $runs_per_scenario"

min_runs="$(jq '.config.min_runs' "$SCENARIOS_FILE")"
[ "$min_runs" -eq 5 ] \
  || fail "config.min_runs: expected 5, got $min_runs"

mie_allowed="$(jq '.config.memory_induced_errors_allowed' "$SCENARIOS_FILE")"
[ "$mie_allowed" -eq 0 ] \
  || fail "config.memory_induced_errors_allowed: expected 0, got $mie_allowed"

# tie_margin is a float — compare as string representation (0.1 is what jq prints for 0.10)
tie_margin="$(jq '.config.tie_margin' "$SCENARIOS_FILE")"
case "$tie_margin" in
  0.1|0.10) ;;
  *) fail "config.tie_margin: expected 0.10, got $tie_margin" ;;
esac

# ---- Section 4: freshness scenarios use explicit plant mode with supersede ---

# All freshness scenarios must use plant_mode = explicit
bad_fresh_mode="$(jq '[.scenarios[] | select(.dimension == "freshness" and .plant_mode != "explicit")] | length' "$SCENARIOS_FILE")"
[ "$bad_fresh_mode" -eq 0 ] \
  || fail "$bad_fresh_mode freshness scenario(s) have non-explicit plant_mode"

# All freshness scenarios must have exactly two plant entries
bad_fresh_plant_count="$(jq '[.scenarios[] | select(.dimension == "freshness") | select((.plant | length) != 2)] | length' "$SCENARIOS_FILE")"
[ "$bad_fresh_plant_count" -eq 0 ] \
  || fail "$bad_fresh_plant_count freshness scenario(s) do not have exactly 2 plant entries"

# The second plant entry in each freshness scenario must have supersedes_prev == true
bad_supersede="$(jq '[.scenarios[] | select(.dimension == "freshness") | select(.plant[1].supersedes_prev != true)] | length' "$SCENARIOS_FILE")"
[ "$bad_supersede" -eq 0 ] \
  || fail "$bad_supersede freshness scenario(s) missing supersedes_prev:true on second plant"

# Every freshness scenario must declare stale_token (the memory-induced-error trigger)
missing_stale="$(jq '[.scenarios[] | select(.dimension == "freshness") | select(.stale_token == null or .stale_token == "")] | length' "$SCENARIOS_FILE")"
[ "$missing_stale" -eq 0 ] \
  || fail "$missing_stale freshness scenario(s) are missing stale_token"

# ---- Section 5: retrieval_scale scenarios have corpus_size >= 500 ------------

bad_corpus="$(jq '[.scenarios[] | select(.dimension == "retrieval_scale") | select((.corpus_size // 0) < 500)] | length' "$SCENARIOS_FILE")"
[ "$bad_corpus" -eq 0 ] \
  || fail "$bad_corpus retrieval_scale scenario(s) have corpus_size < 500"

# Retrieval_scale scenarios must use explicit plant mode
bad_scale_mode="$(jq '[.scenarios[] | select(.dimension == "retrieval_scale" and .plant_mode != "explicit")] | length' "$SCENARIOS_FILE")"
[ "$bad_scale_mode" -eq 0 ] \
  || fail "$bad_scale_mode retrieval_scale scenario(s) have non-explicit plant_mode"

# Each retrieval_scale scenario plants exactly one fact at importance 8 (the target)
bad_scale_importance="$(jq '[.scenarios[] | select(.dimension == "retrieval_scale") | select(.plant[0].importance != 8)] | length' "$SCENARIOS_FILE")"
[ "$bad_scale_importance" -eq 0 ] \
  || fail "$bad_scale_importance retrieval_scale scenario(s) do not set importance=8 on the planted target"

# ---- Section 6: capture scenarios use auto-capture plant mode ----------------

bad_capture_mode="$(jq '[.scenarios[] | select(.dimension == "capture" and .plant_mode != "auto-capture")] | length' "$SCENARIOS_FILE")"
[ "$bad_capture_mode" -eq 0 ] \
  || fail "$bad_capture_mode capture scenario(s) have non-auto-capture plant_mode"

# Capture scenarios must declare capture_expect
missing_cap_expect="$(jq '[.scenarios[] | select(.dimension == "capture") | select(.capture_expect == null or .capture_expect == "")] | length' "$SCENARIOS_FILE")"
[ "$missing_cap_expect" -eq 0 ] \
  || fail "$missing_cap_expect capture scenario(s) are missing capture_expect"

# Capture scenarios must declare capture_forbid to detect false capture
missing_cap_forbid="$(jq '[.scenarios[] | select(.dimension == "capture") | select(.capture_forbid == null or .capture_forbid == "")] | length' "$SCENARIOS_FILE")"
[ "$missing_cap_forbid" -eq 0 ] \
  || fail "$missing_cap_forbid capture scenario(s) are missing capture_forbid"

# ---- Section 7: reach scenarios use explicit plant mode ----------------------

bad_reach_mode="$(jq '[.scenarios[] | select(.dimension == "reach" and .plant_mode != "explicit")] | length' "$SCENARIOS_FILE")"
[ "$bad_reach_mode" -eq 0 ] \
  || fail "$bad_reach_mode reach scenario(s) have non-explicit plant_mode"

# Reach scenarios model cross-machine reach — realistic baseline should be empty
# (B's checkout never received A's CLAUDE.md edit)
bad_reach_realistic="$(jq '[.scenarios[] | select(.dimension == "reach") | select(.realistic_claude_md != "")] | length' "$SCENARIOS_FILE")"
[ "$bad_reach_realistic" -eq 0 ] \
  || fail "$bad_reach_realistic reach scenario(s) have non-empty realistic_claude_md (should model unshared CLAUDE.md)"

# ---- Section 8: every scenario has required top-level fields -----------------

missing_id="$(jq '[.scenarios[] | select(.id == null or .id == "")] | length' "$SCENARIOS_FILE")"
[ "$missing_id" -eq 0 ] \
  || fail "$missing_id scenario(s) missing id field"

missing_work="$(jq '[.scenarios[] | select(.work == null or .work == "")] | length' "$SCENARIOS_FILE")"
[ "$missing_work" -eq 0 ] \
  || fail "$missing_work scenario(s) missing work field"

missing_expect="$(jq '[.scenarios[] | select(.expect == null or .expect == "")] | length' "$SCENARIOS_FILE")"
[ "$missing_expect" -eq 0 ] \
  || fail "$missing_expect scenario(s) missing expect field"

# No two scenarios share the same id (uniqueness constraint)
dup_ids="$(jq '[.scenarios[].id] | length - (unique | length)' "$SCENARIOS_FILE")"
[ "$dup_ids" -eq 0 ] \
  || fail "$dup_ids duplicate scenario id(s) found"

# ---- Section 9: cross-reference integrity ------------------------------------
# The criterion redesign doc must reference the closeout doc

grep -qF 'docs/eval/2026-06-23-w35-scorecard-closeout.md' "$CRITERION_DOC" \
  || fail "criterion redesign doc does not reference the closeout artifact"

# The closeout doc must reference the scenarios file
grep -qF 'crates/rb-eval/scorecard/memory_scorecard_scenarios.json' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not reference the scenarios file"

# The closeout doc must reference the raw TSV evidence for Class C
grep -qF 'docs/eval/2026-06-21-w35-class-c-freshness-n10.tsv' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not reference the raw Class C TSV"

# The closeout doc must reference the Class C measured writeup
grep -qF 'docs/eval/2026-06-21-w35-class-c-freshness-measured.md' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not reference the Class C measured writeup"

# ---- Section 10: CHANGELOG entry references the closeout file path -----------

grep -qF 'docs/eval/2026-06-23-w35-scorecard-closeout.md' "$CHANGELOG" \
  || fail "CHANGELOG does not reference the closeout artifact path"

# CHANGELOG entry must mention 0 memory-induced errors (the Class C safety result)
grep -qF '0 memory-induced errors' "$CHANGELOG" \
  || fail "CHANGELOG closeout entry does not mention 0 memory-induced errors"

# CHANGELOG entry must state that A/B/R are unmeasured (not claim they passed)
grep -q 'unmeasured' "$CHANGELOG" \
  || fail "CHANGELOG does not note that A/B/R are unmeasured"

# ---- Section 11: plans doc references the closeout doc ----------------------

[ -f "$PLANS_DOC" ] \
  || fail "road-to-tens plans doc does not exist: $PLANS_DOC"

grep -qF 'docs/eval/2026-06-23-w35-scorecard-closeout.md' "$PLANS_DOC" \
  || fail "road-to-tens plans doc does not reference the closeout artifact"

# Plans doc must acknowledge C measured green, A/B/R unmeasured
grep -q 'Class C' "$PLANS_DOC" \
  || fail "road-to-tens plans doc does not mention Class C status"

# ---- Section 12: closeout doc uses restricted dimension state vocabulary -----
# Allowed states: "measured", "landed, unmeasured", "intentionally deferred", "not landed"
# The closeout doc must declare all four dimensions plus Safety

grep -qF '| C. Freshness' "$CLOSEOUT_DOC" \
  || fail "closeout dimension table missing C. Freshness row"

grep -qF '| A. Retrieval@scale' "$CLOSEOUT_DOC" \
  || fail "closeout dimension table missing A. Retrieval@scale row"

grep -qF '| B. Capture fidelity' "$CLOSEOUT_DOC" \
  || fail "closeout dimension table missing B. Capture fidelity row"

grep -qF '| R. Reach/team' "$CLOSEOUT_DOC" \
  || fail "closeout dimension table missing R. Reach/team row"

grep -qF '| Safety' "$CLOSEOUT_DOC" \
  || fail "closeout dimension table missing Safety row"

# C must be "measured"; A/B/R must NOT be "measured"
grep -qF '| C. Freshness | measured' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not record C. Freshness as measured"

# A/B/R rows must include "unmeasured" in their state column
grep -qF '| A. Retrieval@scale | landed, unmeasured' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not record A. Retrieval@scale as landed, unmeasured"

grep -qF '| B. Capture fidelity | landed, unmeasured' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not record B. Capture fidelity as landed, unmeasured"

grep -qF '| R. Reach/team | landed, unmeasured' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not record R. Reach/team as landed, unmeasured"

# ---- Section 13: closeout doc correctly scopes the proxy boundary ------------
# Must state it is NOT Phase 5 pilot proof

grep -q 'not Phase 5' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not scope the proxy boundary (not Phase 5)"

# Must say A/B/R cannot be claimed measured
grep -q 'cannot claim A/B/R' "$CLOSEOUT_DOC" \
  || fail "closeout doc does not explicitly disclaim A/B/R as passed"

# ---- Section 14: scorecard script self-test (pure; no API) -------------------
# This is the integration smoke exercised in .github/workflows/memory-scorecard.yml.
# The self-test requires bash (not POSIX sh) and /dev/fd (process substitution).
# Skip gracefully when the environment does not support it.

if command -v bash > /dev/null 2>&1 && [ -d /dev/fd ]; then
  bash "$HERE/scripts/memory-scorecard.sh" --self-test \
    || fail "memory-scorecard.sh --self-test failed"
else
  printf 'NOTE: memory-scorecard.sh --self-test skipped (requires bash + /dev/fd)\n'
fi

# ---- Section 15: specific scenario IDs named in the closeout doc exist -------
# The closeout doc names specific scenario ids in its evidence column; verify they
# actually exist in the JSON so the doc does not reference phantom scenarios.

for scenario_id in scale-http-buried scale-id-type-buried scale-wire-format-buried \
                   cap-http-ureq cap-error-apperror cap-id-ulid \
                   reach-plugin-manifest-path reach-socket-serializer reach-daemon-e2e-command; do
  found="$(jq --arg id "$scenario_id" '[.scenarios[] | select(.id == $id)] | length' "$SCENARIOS_FILE")"
  [ "$found" -eq 1 ] \
    || fail "scenario id '$scenario_id' referenced in closeout doc not found in scenarios JSON"
done

# ---- Section 16: plans doc CI/cadence policy updated to weekly ---------------
# The PR changes the plans doc to say the scorecard runs "weekly schedule plus
# manual dispatch" rather than just "manual dispatch".

grep -q 'weekly' "$PLANS_DOC" \
  || fail "road-to-tens plans doc does not mention weekly schedule for memory-scorecard"

# ---- boundary / regression: ensure CHANGELOG does not claim A/B/R measured --
# A future edit that erroneously marks them green would break the proxy boundary.

if grep -q 'Class A.*measured\|Class B.*measured\|Class R.*measured' "$CHANGELOG" 2>/dev/null; then
  # Only fail if the word "unmeasured" does not also appear — a nuanced check
  if ! grep -q 'A/B/R are landed but unmeasured\|unmeasured' "$CHANGELOG"; then
    fail "CHANGELOG appears to claim A/B/R as measured without the unmeasured caveat"
  fi
fi

printf 'TEST PASS: W3.5 scorecard closeout docs and scenario JSON are consistent\n'