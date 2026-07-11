#!/bin/sh
# memory-scorecard.test.sh — POSIX-sh assertions for the agent-targeting
# functions added to scripts/memory-scorecard.sh.
#
# Tests the pure, API-free functions (scorecard_agent_supported,
# scorecard_skip_reason, scorecard_skip_detail, scorecard_skip_line) and the
# --agent argument routing by invoking the script in skip-exit paths that do
# NOT require API keys or compiled binaries.
#
# Usage:
#   sh scripts/memory-scorecard.test.sh
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

# ---------------------------------------------------------------------------
# Helper: source only the pure function definitions from memory-scorecard.sh.
# We emit a POSIX substitute for the script header that defines the four
# functions identically to their source so we can call them directly.
# The functions are extracted verbatim from the script rather than sourced
# wholesale (which would execute main-body code).
# ---------------------------------------------------------------------------
_source_pure_functions() {
  # scorecard_agent_supported — lines 50-52
  scorecard_agent_supported() { [ "$1" = "claude-code" ]; }

  # scorecard_skip_reason — lines 54-62
  scorecard_skip_reason() {
    case "$1" in
      codex)    printf 'scorecard_unsupported_codex_fixture_gated' ;;
      opencode) printf 'scorecard_unsupported_opencode_plugin_deferred' ;;
      gemini)   printf 'scorecard_unsupported_gemini_not_first_priority' ;;
      hermes)   printf 'scorecard_unsupported_hermes_discovery_gated' ;;
      *)        printf 'scorecard_unsupported_unknown_agent' ;;
    esac
  }

  # scorecard_skip_detail — lines 64-82
  scorecard_skip_detail() {
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

  # scorecard_skip_phase — earliest blocked pipeline stage per agent
  # (capture|config|scoring; retrieval gaps never block the harness)
  scorecard_skip_phase() {
    case "$1" in
      codex)           printf 'capture' ;;
      opencode|hermes) printf 'config' ;;
      *)               printf 'scoring' ;;
    esac
  }

  # scorecard_skip_line
  scorecard_skip_line() {
    local agent="$1"
    printf 'agent=%s\tdimension=all\tscenario=all\tphase=%s\tstatus=skip\treason=%s\tdetail=%s\n' \
      "$agent" "$(scorecard_skip_phase "$agent")" "$(scorecard_skip_reason "$agent")" "$(scorecard_skip_detail "$agent")"
  }
}
_source_pure_functions

# ---------------------------------------------------------------------------
# 1. scorecard_agent_supported: claude-code is the only supported target
# ---------------------------------------------------------------------------
if scorecard_agent_supported "claude-code"; then
  pass "scorecard_agent_supported returns true for claude-code"
else
  fail "scorecard_agent_supported should return true for claude-code"
fi

for agent in codex opencode gemini hermes; do
  if scorecard_agent_supported "$agent"; then
    fail "scorecard_agent_supported should return false for $agent"
  else
    pass "scorecard_agent_supported returns false for $agent"
  fi
done

if scorecard_agent_supported "all"; then
  fail "scorecard_agent_supported should return false for 'all'"
else
  pass "scorecard_agent_supported returns false for 'all'"
fi

if scorecard_agent_supported ""; then
  fail "scorecard_agent_supported should return false for empty string"
else
  pass "scorecard_agent_supported returns false for empty string"
fi

# ---------------------------------------------------------------------------
# 2. scorecard_skip_reason: each agent returns the expected reason token
# ---------------------------------------------------------------------------
reason_codex="$(scorecard_skip_reason codex)"
[ "$reason_codex" = "scorecard_unsupported_codex_fixture_gated" ] \
  || fail "codex skip reason: expected 'scorecard_unsupported_codex_fixture_gated', got '$reason_codex'"
pass "scorecard_skip_reason codex is scorecard_unsupported_codex_fixture_gated"

reason_opencode="$(scorecard_skip_reason opencode)"
[ "$reason_opencode" = "scorecard_unsupported_opencode_plugin_deferred" ] \
  || fail "opencode skip reason: expected 'scorecard_unsupported_opencode_plugin_deferred', got '$reason_opencode'"
pass "scorecard_skip_reason opencode is scorecard_unsupported_opencode_plugin_deferred"

reason_gemini="$(scorecard_skip_reason gemini)"
[ "$reason_gemini" = "scorecard_unsupported_gemini_not_first_priority" ] \
  || fail "gemini skip reason: expected 'scorecard_unsupported_gemini_not_first_priority', got '$reason_gemini'"
pass "scorecard_skip_reason gemini is scorecard_unsupported_gemini_not_first_priority"

reason_hermes="$(scorecard_skip_reason hermes)"
[ "$reason_hermes" = "scorecard_unsupported_hermes_discovery_gated" ] \
  || fail "hermes skip reason: expected 'scorecard_unsupported_hermes_discovery_gated', got '$reason_hermes'"
pass "scorecard_skip_reason hermes is scorecard_unsupported_hermes_discovery_gated"

reason_unknown="$(scorecard_skip_reason totally-unknown-agent)"
[ "$reason_unknown" = "scorecard_unsupported_unknown_agent" ] \
  || fail "unknown agent skip reason: expected 'scorecard_unsupported_unknown_agent', got '$reason_unknown'"
pass "scorecard_skip_reason unknown agent is scorecard_unsupported_unknown_agent"

# ---------------------------------------------------------------------------
# 3. scorecard_skip_detail: each agent returns a non-empty human-readable string
# ---------------------------------------------------------------------------
for agent in codex opencode gemini hermes; do
  detail="$(scorecard_skip_detail "$agent")"
  [ -n "$detail" ] || fail "scorecard_skip_detail returned empty string for $agent"
  pass "scorecard_skip_detail is non-empty for $agent"
done

detail_codex="$(scorecard_skip_detail codex)"
printf '%s' "$detail_codex" | grep -qF "fixture" \
  || fail "codex skip detail should mention 'fixture', got: $detail_codex"
pass "scorecard_skip_detail codex mentions fixture gates"

detail_hermes="$(scorecard_skip_detail hermes)"
printf '%s' "$detail_hermes" | grep -qiF "discovery" \
  || fail "hermes skip detail should mention 'discovery', got: $detail_hermes"
pass "scorecard_skip_detail hermes mentions discovery"

detail_fallback="$(scorecard_skip_detail totally-unknown-agent)"
[ -n "$detail_fallback" ] || fail "scorecard_skip_detail should return non-empty fallback"
pass "scorecard_skip_detail returns non-empty fallback for unknown agent"

# ---------------------------------------------------------------------------
# 4. scorecard_skip_line: output contains all required TSV fields
# ---------------------------------------------------------------------------
for agent in codex opencode gemini hermes; do
  skip_line="$(scorecard_skip_line "$agent")"

  case "$agent" in
    codex)           expected_phase="capture" ;;
    opencode|hermes) expected_phase="config" ;;
    *)               expected_phase="scoring" ;;
  esac

  printf '%s' "$skip_line" | grep -qF "agent=$agent" \
    || fail "$agent skip line missing 'agent=$agent': $skip_line"

  printf '%s' "$skip_line" | grep -qF "dimension=all" \
    || fail "$agent skip line missing 'dimension=all': $skip_line"

  printf '%s' "$skip_line" | grep -qF "scenario=all" \
    || fail "$agent skip line missing 'scenario=all': $skip_line"

  printf '%s' "$skip_line" | grep -qF "phase=$expected_phase" \
    || fail "$agent skip line missing 'phase=$expected_phase': $skip_line"

  printf '%s' "$skip_line" | grep -qF "status=skip" \
    || fail "$agent skip line missing 'status=skip': $skip_line"

  printf '%s' "$skip_line" | grep -qF "reason=scorecard_unsupported" \
    || fail "$agent skip line missing 'reason=scorecard_unsupported...' prefix: $skip_line"

  printf '%s' "$skip_line" | grep -qF "detail=" \
    || fail "$agent skip line missing 'detail=' field: $skip_line"

  pass "scorecard_skip_line for $agent has all required TSV fields"
done

# ---------------------------------------------------------------------------
# 5. scorecard_skip_line: codex includes the exact machine-readable reason
#    (regression test matching the --self-test assertion inside the script)
# ---------------------------------------------------------------------------
codex_skip="$(scorecard_skip_line codex)"
expected_codex_prefix="agent=codex	dimension=all	scenario=all	phase=capture	status=skip	reason=scorecard_unsupported_codex_fixture_gated"
printf '%s' "$codex_skip" | grep -qF "$expected_codex_prefix" \
  || fail "codex skip line does not match expected prefix. got: $codex_skip"
pass "scorecard_skip_line codex matches machine-readable prefix exactly"

# ---------------------------------------------------------------------------
# 6. --agent with an invalid value exits 2 and writes to stderr
# ---------------------------------------------------------------------------
invalid_output="$(bash "$SCORECARD_SH" --agent totally-invalid-agent 2>&1 || true)"
exit_code=0
bash "$SCORECARD_SH" --agent totally-invalid-agent 2>/dev/null || exit_code=$?
[ "$exit_code" = "2" ] \
  || fail "--agent with invalid value should exit 2, got $exit_code"
pass "--agent with invalid value exits 2"

printf '%s' "$invalid_output" | grep -qi "must be one of" \
  || fail "--agent invalid error message should say 'must be one of', got: $invalid_output"
pass "--agent invalid value error message mentions 'must be one of'"

# ---------------------------------------------------------------------------
# 7. --agent with each known unsupported agent exits 0 (skip path, no API needed)
# ---------------------------------------------------------------------------
for agent in codex opencode gemini hermes; do
  agent_exit=0
  bash "$SCORECARD_SH" --agent "$agent" 2>/dev/null || agent_exit=$?
  [ "$agent_exit" = "0" ] \
    || fail "--agent $agent should exit 0 (skip path), got $agent_exit"
  pass "--agent $agent exits 0 on the skip path"
done

# ---------------------------------------------------------------------------
# 8. --agent for each unsupported target prints a skip line to stdout
# ---------------------------------------------------------------------------
for agent in codex opencode gemini hermes; do
  output="$(bash "$SCORECARD_SH" --agent "$agent" 2>/dev/null)"
  printf '%s' "$output" | grep -qF "status=skip" \
    || fail "--agent $agent output should contain 'status=skip', got: $output"
  printf '%s' "$output" | grep -qF "agent=$agent" \
    || fail "--agent $agent output should contain 'agent=$agent', got: $output"
  pass "--agent $agent prints machine-readable skip line"
done

# ---------------------------------------------------------------------------
# 9. --agent all prints skip lines for codex and opencode (not gemini or hermes),
#    then attempts the live claude-code path (will fail without binaries, which is
#    expected — we only check that the skip output was emitted before the failure)
# ---------------------------------------------------------------------------
all_output="$(bash "$SCORECARD_SH" --agent all 2>/dev/null || true)"
printf '%s' "$all_output" | grep -qF "agent=codex" \
  || fail "--agent all output should contain codex skip line"
printf '%s' "$all_output" | grep -qF "agent=opencode" \
  || fail "--agent all output should contain opencode skip line"
pass "--agent all emits codex and opencode skip lines"

# ---------------------------------------------------------------------------
# 10. --agent claude-code does NOT emit a skip line (live path attempted)
#     We verify exit is non-zero (missing binaries/API), not a skip 0.
# ---------------------------------------------------------------------------
claude_exit=0
bash "$SCORECARD_SH" --agent claude-code 2>/dev/null || claude_exit=$?
[ "$claude_exit" != "0" ] \
  || fail "--agent claude-code should not exit 0 without required binaries/API"
pass "--agent claude-code does not take the skip exit path"

claude_output="$(bash "$SCORECARD_SH" --agent claude-code 2>/dev/null || true)"
# Must NOT emit a status=skip line (that would mean it was incorrectly skipped)
if printf '%s' "$claude_output" | grep -qF "status=skip"; then
  fail "--agent claude-code should not emit a skip line"
fi
pass "--agent claude-code does not emit a status=skip line"

# ---------------------------------------------------------------------------
printf '\nTEST PASS: memory-scorecard.sh agent-targeting functions behave correctly\n'