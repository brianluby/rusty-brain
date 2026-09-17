#!/bin/sh
# Offline routing assertions for the OMP-native scorecard.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SCORECARD_SH="${HERE}/memory-scorecard.sh"

fail() {
  printf 'TEST FAIL: %s\n' "$1" >&2
  exit 1
}

pass() {
  printf 'ok: %s\n' "$1"
}

# The pure self-test covers scoring, OMP event-log parsing, scenario schema,
# retention, and the supported-agent predicate without starting an OMP model.
bash "$SCORECARD_SH" --self-test
pass "OMP scorecard self-test"

for agent in codex opencode gemini hermes; do
  output="$(bash "$SCORECARD_SH" --agent "$agent" 2>/dev/null)"
  printf '%s' "$output" | grep -qF "agent=$agent" \
    || fail "--agent $agent output lacks machine-readable agent field"
  printf '%s' "$output" | grep -qF "status=skip" \
    || fail "--agent $agent must take the documented skip path"
  pass "--agent $agent skips with a machine-readable reason"
done

invalid_exit=0
bash "$SCORECARD_SH" --agent invalid-agent >/dev/null 2>&1 || invalid_exit=$?
[ "$invalid_exit" = 2 ] || fail "invalid --agent must exit 2, got $invalid_exit"
pass "invalid --agent exits 2"

all_output="$(PATH=/usr/bin:/bin /bin/bash "$SCORECARD_SH" --agent all 2>/dev/null || true)"
for agent in codex opencode gemini hermes; do
  printf '%s' "$all_output" | grep -qF "agent=$agent" \
    || fail "--agent all must report $agent as skipped"
done
pass "--agent all reports non-OMP skips"

omp_exit=0
PATH=/usr/bin:/bin /bin/bash "$SCORECARD_SH" --agent omp --model @slow >/dev/null 2>&1 || omp_exit=$?
[ "$omp_exit" != 0 ] || fail "OMP path unexpectedly passed without required binaries"
pass "--agent omp selects the live path without starting a model"

python3 -m unittest discover -s "$HERE/tests" -p test_scorecard_controls.py -v
printf '\nTEST PASS: OMP scorecard routing and receipt controls behave correctly\n'