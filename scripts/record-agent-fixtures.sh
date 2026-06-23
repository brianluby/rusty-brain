#!/usr/bin/env bash
# Cross-agent fixture-recording harness (see
# docs/specs/2026-06-23-cross-agent-fixture-recording.md). Records real codex /
# opencode hook-lifecycle payloads + headless-result schema into
# crates/rb-hooks/tests/fixtures/<agent>/. The PURE helpers run under
# `--self-test` with NO API; the live path is guarded behind real CLI auth.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURE_ROOT="$REPO_ROOT/crates/rb-hooks/tests/fixtures"

fail=0
check() { if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "BUG: $1 (want '$2' got '$3')"; fail=1; fi; }

agent_supported() { case "$1" in codex|opencode) return 0 ;; *) return 1 ;; esac; }

# scrub <real_home>: stdin -> stdout. Rewrites the recording user's home dir to
# /Users/user and redacts the secret classes the hook-capture redaction covers
# (bearer tokens, sk-/AKIA keys, key=value secrets, PEM private keys). python3
# (already a harness dep) gives multiline-safe, idempotent regexes. A fixed
# fixpoint: redacted placeholders contain chars outside each pattern's class, so
# re-running is a no-op.
scrub() { # real_home
  # The Python program is passed as a file via process substitution (not on
  # stdin) so the function's piped payload stays connected to sys.stdin.
  python3 <(cat <<'PY'
import sys, re
real_home = sys.argv[1]
data = sys.stdin.read()
if real_home:
    data = data.replace(real_home, "/Users/user")
subs = [
    (re.compile(r'Bearer\s+[A-Za-z0-9._\-]{8,}'), 'Bearer <redacted>'),
    (re.compile(r'\bsk-[A-Za-z0-9]{8,}\b'), 'sk-<redacted>'),
    (re.compile(r'\bAKIA[A-Z0-9]{12,}\b'), 'AKIA<redacted>'),
    (re.compile(r'(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----'), '<redacted-pem>'),
    (re.compile(r'(?i)\b([A-Za-z0-9_]*(?:key|token|secret|password)[A-Za-z0-9_]*)\s*=\s*(?!<redacted|sk-<|AKIA<|Bearer <)[^\s"&]+'), r'\1=<redacted>'),
]
for rx, repl in subs:
    data = rx.sub(repl, data)
sys.stdout.write(data)
PY
) "$1"
}

# infer_terminus <fired_count> <total_turns>: classify whether the candidate
# terminus event is a true end-of-session (fired exactly once across a
# multi-turn run) or a per-turn boundary. A single-turn run cannot distinguish
# the two, so it is ambiguous. Evidence, not proof — the README records the run
# shape so a later run can corroborate.
infer_terminus() { # fired_count total_turns
  local count="$1" turns="$2"
  if [ "$turns" -le 1 ]; then echo "ambiguous"
  elif [ "$count" -eq 1 ]; then echo "true-terminus"
  elif [ "$count" -eq "$turns" ]; then echo "per-turn"
  else echo "ambiguous"; fi
}

# codex_hooks_json <log_dir>: emit a .codex/hooks.json whose every event command
# appends that event's raw stdin JSON (+ trailing newline) to a per-event file,
# matching the rb-install codex schema: { "hooks": { "<Event>": [ <group> ] } }
# where a group is { "hooks": [ { "type":"command", "command":"<shell string>" } ] }
# and the tool event (PostToolUse) additionally carries "matcher":"*".
codex_hooks_json() { # log_dir
  local d="$1"
  python3 - "$d" <<'PY'
import json, sys
d = sys.argv[1]
events = {"SessionStart": "session_start", "PostToolUse": "post_tool_use",
          "Stop": "stop", "PreCompact": "pre_compact"}
def cmd(stem):
    f = f"{d}/{stem}.json"
    return f"cat >> '{f}'; printf '\\n' >> '{f}'"
hooks = {}
for event, stem in events.items():
    group = {"hooks": [{"type": "command", "command": cmd(stem)}]}
    if event == "PostToolUse":
        group = {"matcher": "*", **group}
    hooks[event] = [group]
print(json.dumps({"hooks": hooks}, indent=2))
PY
}

# opencode_plugin_src <log_dir>: emit the recording plugin with the log dir baked
# in (the committed copy under scripts/fixtures/opencode-logger/ reads the dir
# from RB_FIXTURE_LOG_DIR; here we inline it so a throwaway run needs no env).
opencode_plugin_src() { # log_dir
  local d="$1"
  sed "s#process.env.RB_FIXTURE_LOG_DIR || \".\"#\"$d\"#" \
    "$REPO_ROOT/scripts/fixtures/opencode-logger/plugin.js"
}

self_test() {
  echo "== record-agent-fixtures self-test (pure; no API) =="
  if agent_supported codex && agent_supported opencode && ! agent_supported gemini; then
    echo "ok: agent allowlist is codex + opencode"
  else
    echo "BUG: agent allowlist"; fail=1
  fi
  # scrub: home rewrite + each secret class, and idempotence.
  local s1 s2
  s1="$(printf 'path=/Users/realuser/.codex tok=Bearer abcd1234efgh key=sk-ABCDEFGH12345678 aws=AKIAABCDEFGH1234 api_key=supersecretvalue' | scrub /Users/realuser)"
  check "scrub rewrites home"        "1" "$(printf '%s' "$s1" | grep -cF '/Users/user/.codex')"
  check "scrub keeps no real home"   "0" "$(printf '%s' "$s1" | grep -cF '/Users/realuser')"
  check "scrub redacts bearer"       "1" "$(printf '%s' "$s1" | grep -cF 'Bearer <redacted>')"
  check "scrub redacts sk- key"      "1" "$(printf '%s' "$s1" | grep -cF 'sk-<redacted>')"
  check "scrub redacts aws key"      "1" "$(printf '%s' "$s1" | grep -cF 'AKIA<redacted>')"
  check "scrub redacts key=value"    "1" "$(printf '%s' "$s1" | grep -cF 'api_key=<redacted>')"
  s2="$(printf '%s' "$s1" | scrub /Users/realuser)"
  check "scrub is idempotent"        "$s1" "$s2"
  local pem
  pem="$(printf -- '-----BEGIN PRIVATE KEY-----\nMIIabc\n-----END PRIVATE KEY-----' | scrub '')"
  check "scrub redacts PEM block"    "<redacted-pem>" "$pem"
  check "terminus fired-once => true-terminus" "true-terminus" "$(infer_terminus 1 3)"
  check "terminus once-per-turn => per-turn"   "per-turn"      "$(infer_terminus 3 3)"
  check "terminus single-turn run => ambiguous" "ambiguous"    "$(infer_terminus 1 1)"
  check "terminus mismatch => ambiguous"       "ambiguous"     "$(infer_terminus 2 3)"
  local ch; ch="$(codex_hooks_json /tmp/rec/raw)"
  check "codex hooks.json is valid json" "0" "$(printf '%s' "$ch" | python3 -c 'import json,sys; json.load(sys.stdin)'; echo $?)"
  check "codex registers all four events" "4" "$(printf '%s' "$ch" | python3 -c 'import json,sys; h=json.load(sys.stdin)["hooks"]; print(sum(k in h for k in ("SessionStart","PostToolUse","Stop","PreCompact")))')"
  check "codex PostToolUse carries matcher" "*" "$(printf '%s' "$ch" | python3 -c 'import json,sys; print(json.load(sys.stdin)["hooks"]["PostToolUse"][0]["matcher"])')"
  check "codex Stop omits matcher" "no-matcher" "$(printf '%s' "$ch" | python3 -c 'import json,sys; g=json.load(sys.stdin)["hooks"]["Stop"][0]; print("no-matcher" if "matcher" not in g else "has-matcher")')"
  check "codex command appends to per-event log" "1" "$(printf '%s' "$ch" | python3 -c 'import json,sys; c=json.load(sys.stdin)["hooks"]["Stop"][0]["hooks"][0]["command"]; print(int("/tmp/rec/raw/stop.json" in c))')"
  local op; op="$(opencode_plugin_src /tmp/rec/raw)"
  check "opencode plugin references log dir" "1" "$(printf '%s' "$op" | grep -cF '/tmp/rec/raw')"
  for ev in session.created tool.execute.after session.idle session.compacted session.deleted; do
    check "opencode plugin handles $ev" "1" "$(printf '%s' "$op" | grep -cF "$ev")"
  done
  check "opencode-logger plugin file exists" "1" "$( [ -f "$REPO_ROOT/scripts/fixtures/opencode-logger/plugin.js" ] && echo 1 || echo 0 )"
  if [ "$fail" -eq 0 ]; then echo "self-test PASS"; return 0; fi
  echo "self-test FAIL" >&2; return 1
}

MODE="record"; AGENT=""; OUT_DIR=""; DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) MODE="self-test"; shift ;;
    --agent)     AGENT="${2:?--agent needs a value}"
                 case "$AGENT" in codex|opencode|all) ;; *) echo "--agent must be codex, opencode, or all (got '$AGENT')" >&2; exit 2 ;; esac
                 shift 2 ;;
    --out-dir)   OUT_DIR="${2:?--out-dir needs a value}"; shift 2 ;;
    --dry-run)   DRY_RUN=1; shift ;;
    -h|--help)   awk 'NR>1 && /^#/ {print; next} NR>1 {exit}' "$0"; exit 2 ;;
    *)           echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi
echo "record mode not yet implemented" >&2; exit 1
