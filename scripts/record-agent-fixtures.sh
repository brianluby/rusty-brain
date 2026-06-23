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

# scrub <real_home> [extra_home ...]: stdin -> stdout. Rewrites every supplied
# home dir to /Users/user and redacts the secret classes the hook-capture
# redaction covers (bearer tokens, sk-/AKIA keys, key=value AND "key":"value"
# JSON secrets, PEM private keys). Hook payloads are JSON, so the keyword
# redaction matches both shell `key=value` and JSON `"key":"value"` forms; the
# JSON form catches AWS_SECRET_ACCESS_KEY and friends by field name (no prefix).
# Multiple home args let the caller scrub both the throwaway HOME and the
# operator's real HOME (paths can leak from inherited env / config). python3
# (already a harness dep) gives multiline-safe, idempotent regexes. A fixed
# fixpoint: redacted placeholders contain chars outside each pattern's class, so
# re-running is a no-op.
scrub() { # real_home [extra_home ...]
  # The Python program is passed as a file via process substitution (not on
  # stdin) so the function's piped payload stays connected to sys.stdin.
  python3 <(cat <<'PY'
import sys, re
homes = [h for h in sys.argv[1:] if h]
data = sys.stdin.read()
for h in homes:
    data = data.replace(h, "/Users/user")
subs = [
    (re.compile(r'Bearer\s+[A-Za-z0-9._\-]{8,}'), 'Bearer <redacted>'),
    # sk- keys are multi-segment in the wild (sk-ant-api03-..., sk-proj-...);
    # match the full hyphen-delimited token, not just a contiguous first run.
    (re.compile(r'\bsk-[A-Za-z0-9][A-Za-z0-9\-]{7,}\b'), 'sk-<redacted>'),
    (re.compile(r'\bAKIA[A-Z0-9]{12,}\b'), 'AKIA<redacted>'),
    (re.compile(r'(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----'), '<redacted-pem>'),
    # JSON form: "<...key/token/secret/password...>": "value" (value in quotes).
    # Negative lookahead skips already-redacted values to stay idempotent.
    (re.compile(r'(?i)("[A-Za-z0-9_]*(?:key|token|secret|password)[A-Za-z0-9_]*"\s*:\s*)"(?!<redacted)[^"]{4,}"'), r'\1"<redacted>"'),
    # Shell form: key=value (value stops at whitespace/quote/&).
    (re.compile(r'(?i)\b([A-Za-z0-9_]*(?:key|token|secret|password)[A-Za-z0-9_]*)\s*=\s*(?!<redacted|sk-<|AKIA<|Bearer <)[^\s"&]+'), r'\1=<redacted>'),
]
for rx, repl in subs:
    data = rx.sub(repl, data)
sys.stdout.write(data)
PY
) "$@"
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

# emit_readme <agent> <out_dir> <cli_version> <terminus_verdict> <events_csv> [os_info] [capture_date]
emit_readme() {
  local agent="$1" out="$2" ver="$3" term="$4" events="$5"
  local os_info="${6:-not recorded}" capture_date="${7:-dry-run}"
  {
    printf '# %s Fixture Status\n\n' "$agent"
    printf '## Provenance\n\n- **CLI:** %s\n- **Captured:** %s, %s\n- **Captured by:** scripts/record-agent-fixtures.sh\n- **Events:** %s\n\n' "$ver" "$capture_date" "$os_info" "$events"
    printf '## Recording Recipe\n\nGenerated by `scripts/record-agent-fixtures.sh --agent %s`: a throwaway HOME + project with logging hooks, one multi-turn headless session (one Bash + one file write), then sanitize.\n\n' "$agent"
    printf '## Sanitization\n\n| What | Recorded value | Committed value |\n|---|---|---|\n| Home dir (throwaway + real) | `/Users/<real>` | `/Users/user` |\n| Secrets (bearer/sk-/AKIA/key=value/`"key":"value"`/PEM) | real | `<redacted>` |\n\n'
    printf '## Session Terminus\n\nMulti-turn verdict: **%s** (see the run shape in the recipe; evidence, not proof). The machine-readable counts are in `terminus.json`.\n\n' "$term"
    printf '## Headless Result Schema\n\n`result.jsonl` is the verbatim headless-CLI result output. Cost/token-axis interpretation is deferred to the scorecard-runner build.\n\n'
    printf '## Files\n\n| File | Event stem |\n|---|---|\n'
    if [ -n "$events" ]; then
      local IFS=','; local s
      for s in $events; do
        s="${s# }"
        printf '| `%s.json` | `%s` |\n' "$s" "$s"
      done
    else
      printf '| _(none recorded)_ | |\n'
    fi
    printf '\n'
    printf '## Fields present / absent\n\nFields present in real payloads that the adapter intentionally drops are listed here after the operator reviews the recorded fixtures (mirrors the claude_code README). Populate from the committed `*.json` files once recorded.\n\n'
    printf '## Known Absences\n\nEvents that did not fire in the recording session are listed here by the recorder.\n'
  } > "$out/README.md"
}

# dry_run_agent <agent> <out_dir>: generate config + a placeholder fixture layout
# with NO CLI invocation, so the harness is verifiable offline. Asserts the
# emitted layout matches the claude_code template (config + README present).
dry_run_agent() { # agent out_dir
  local agent="$1" out="$2"
  mkdir -p "$out"
  case "$agent" in
    codex)
      mkdir -p "$out/.codex" "$out/raw"
      codex_hooks_json "$out/raw" > "$out/.codex/hooks.json"
      ;;
    opencode)
      mkdir -p "$out/.opencode/plugin" "$out/raw"
      opencode_plugin_src "$out/raw" > "$out/.opencode/plugin/fixture-logger.js"
      ;;
    *) echo "dry_run_agent: unknown agent $agent" >&2; return 1 ;;
  esac
  echo '{"verdict":"ambiguous","fired":0,"turns":0}' > "$out/terminus.json"
  emit_readme "$agent" "$out" "dry-run (not recorded)" "ambiguous" "dry-run"
}

record_cli_for() { case "$1" in codex) echo codex ;; opencode) echo opencode ;; *) echo "" ;; esac; }

seed_home() { mkdir -p "$1"; }

# record_live <agent> <out_dir>: live recording path. Requires the agent CLI and
# real auth. Runs entirely under a throwaway HOME so global agent state is never
# touched. Captures raw per-event payloads, the headless result stream, counts
# the terminus event across a multi-turn run, sanitizes, and writes fixtures.
record_live() { # agent out_dir
  local agent="$1" out="$2"
  local cli; cli="$(record_cli_for "$agent")"
  command -v "$cli" >/dev/null 2>&1 || { echo "ERROR: $cli not on PATH; cannot record $agent fixtures" >&2; return 1; }
  local ver; ver="$("$cli" --version 2>/dev/null | head -1 || echo unknown)"
  # Provenance the README needs (spec: CLI version, OS, date).
  local os_info capture_date
  os_info="$(uname -s -r 2>/dev/null || echo unknown)"
  capture_date="$(date +%Y-%m-%d)"

  # Remember the operator's real HOME so paths leaked from inherited env/config
  # are scrubbed too (the throwaway HOME below only covers paths under it).
  local real_home="${HOME:-}"

  local work; work="$(mktemp -d "${TMPDIR:-/tmp}/rb-rec.XXXXXX")"
  # Clean up the tempdir on ANY exit from here on, including a set -e abort.
  trap 'rm -rf "$work"' RETURN
  local home="$work/home" proj="$work/proj" raw="$work/raw"
  seed_home "$home"; mkdir -p "$proj" "$raw"
  mkdir -p "$out"

  # A two-step prompt so the terminus event count can be compared against turns.
  local prompt="First run: echo hi via Bash. Then create a file notes.txt containing exactly: recorded. Do both."
  local result="$out/result.jsonl"

  # Spawn the agent with a CLEAN environment (env -i) so inherited operator
  # secrets (OPENAI_API_KEY, ANTHROPIC_API_KEY, AWS_*, GH_TOKEN, ...) can never
  # land in a SessionStart env payload. Only the throwaway HOME, PATH, and the
  # per-agent log-dir var are passed through.
  case "$agent" in
    codex)
      mkdir -p "$proj/.codex"
      codex_hooks_json "$raw" > "$proj/.codex/hooks.json"
      # Use codex's non-interactive exec with its machine-readable output flag.
      # The exact flag is recorded in the README from `codex exec --help`.
      ( cd "$proj" && env -i HOME="$home" PATH="$PATH" \
          codex exec "$prompt" ) >"$result" 2>&1 || true
      ;;
    opencode)
      mkdir -p "$proj/.opencode/plugin"
      opencode_plugin_src "$raw" > "$proj/.opencode/plugin/fixture-logger.js"
      # Register the plugin so opencode actually loads it (a dropped file alone
      # is ignored; opencode.json's `plugin` array declares the local path).
      printf '{"plugin":["./.opencode/plugin/fixture-logger.js"]}\n' > "$proj/opencode.json"
      ( cd "$proj" && env -i HOME="$home" PATH="$PATH" RB_FIXTURE_LOG_DIR="$raw" \
          opencode run "$prompt" ) >"$result" 2>&1 || true
      ;;
  esac

  # Sanitize each captured raw event into a committed single-line fixture.
  local present="" stem terminus_stem
  case "$agent" in
    codex)    terminus_stem="stop" ;;
    opencode) terminus_stem="session_idle" ;;
  esac
  for f in "$raw"/*.json; do
    [ -e "$f" ] || continue
    stem="$(basename "$f" .json)"
    # Commit the FIRST captured line per event, sanitized (matches claude_code:
    # one verbatim line per event). Scrub BOTH the throwaway and the real HOME.
    head -1 "$f" | scrub "$home" "$real_home" > "$out/$stem.json"
    present="$present${present:+, }$stem"
  done
  # Sanitize the result stream too (it can echo paths/secrets). Two simple
  # commands (not `a && b`) so a scrub failure aborts under set -e instead of
  # leaving the file unredacted.
  if [ -f "$result" ]; then
    scrub "$home" "$real_home" < "$result" > "$result.tmp"
    mv "$result.tmp" "$result"
  fi

  # Terminus: count fired terminus events vs turns observed in the result.
  # wc -l always exits 0 and emits a single integer (grep -c exits 1 on zero
  # matches AND prints '0', so `grep -c || echo` double-captures two lines).
  local fired turns verdict
  fired="$(wc -l < "$raw/$terminus_stem.json" 2>/dev/null || echo 0)"
  turns="$(wc -l < "$result" 2>/dev/null || echo 0)"
  verdict="$(infer_terminus "$fired" "$turns")"

  # Spec-required machine-readable terminus artifact for the future scorecard.
  printf '{"verdict":"%s","fired":%s,"turns":%s}\n' "$verdict" "$fired" "$turns" > "$out/terminus.json"

  emit_readme "$agent" "$out" "$ver" "$verdict" "$present" "$os_info" "$capture_date"
  echo "recorded $agent fixtures under $out (events: ${present:-none}, terminus: $verdict)"
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
  # Multi-segment sk- keys (real Anthropic / OpenAI-project formats) must redact
  # the FULL hyphen-delimited token, leaving no key material behind.
  check "scrub redacts sk-ant key"   "1" "$(printf 'k=sk-ant-api03-XYZ12345ABCDEF' | scrub '' | grep -cF 'sk-<redacted>')"
  check "scrub leaks no sk-ant body"  "0" "$(printf 'k=sk-ant-api03-XYZ12345ABCDEF' | scrub '' | grep -cF 'api03')"
  check "scrub redacts sk-proj key"  "1" "$(printf 'tok=sk-proj-ABCDEFGH12345678' | scrub '' | grep -cF 'sk-<redacted>')"
  check "scrub leaks no sk-proj body" "0" "$(printf 'tok=sk-proj-ABCDEFGH12345678' | scrub '' | grep -cF '12345678')"
  # JSON `"key":"value"` form (the actual hook-payload shape) must redact too.
  local sj
  sj="$(printf '{"api_key":"supersecretvalue123","AWS_SECRET_ACCESS_KEY":"wJalrXUtnFEMI/K7MDENG/bPxRfiCYrealSECRET","GH_TOKEN":"ghp_ABCDEFGHIJKLMNOP01234"}' | scrub '')"
  check "scrub redacts json api_key"  "1" "$(printf '%s' "$sj" | grep -cF '"api_key":"<redacted>"')"
  check "scrub redacts json aws secret" "1" "$(printf '%s' "$sj" | grep -cF '"AWS_SECRET_ACCESS_KEY":"<redacted>"')"
  check "scrub redacts json gh token" "1" "$(printf '%s' "$sj" | grep -cF '"GH_TOKEN":"<redacted>"')"
  check "scrub leaks no json secret"  "0" "$(printf '%s' "$sj" | grep -cE 'supersecretvalue|realSECRET|ghp_ABCDEF')"
  check "scrub json is idempotent"    "$sj" "$(printf '%s' "$sj" | scrub '')"
  # Multiple home args: both the throwaway and the operator's real home rewrite.
  local sh
  sh="$(printf '{"cwd":"/Users/realuser/proj","tmp":"/tmp/rb-rec.X/home/x"}' | scrub /tmp/rb-rec.X/home /Users/realuser)"
  check "scrub rewrites both homes"   "0" "$(printf '%s' "$sh" | grep -cE '/Users/realuser|/tmp/rb-rec.X/home')"
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
  check "codex command appends to per-event log" "1" "$(printf '%s' "$ch" | python3 -c 'import json,sys; c=json.load(sys.stdin)["hooks"]["Stop"][0]["hooks"][0]["command"]; print(int("cat >>" in c and "/tmp/rec/raw/stop.json" in c))')"
  local op; op="$(opencode_plugin_src /tmp/rec/raw)"
  check "opencode plugin references log dir" "1" "$(printf '%s' "$op" | grep -cF '/tmp/rec/raw')"
  for ev in session.created tool.execute.after session.idle session.compacted session.deleted; do
    check "opencode plugin handles $ev" "present" "$(printf '%s' "$op" | grep -qF "$ev" && echo present || echo absent)"
  done
  # tool.execute.after must be a DEDICATED hook slot (a key in the returned
  # object), not only mentioned in the STEMS map — the generic `event` handler
  # never receives it (it is not part of the SDK Event union).
  check "opencode plugin registers tool.execute.after hook" "present" "$(printf '%s' "$op" | grep -qE '"tool\.execute\.after"\s*:' && echo present || echo absent)"
  check "opencode-logger plugin file exists" "1" "$( [ -f "$REPO_ROOT/scripts/fixtures/opencode-logger/plugin.js" ] && echo 1 || echo 0 )"
  local dr; dr="$(mktemp -d "${TMPDIR:-/tmp}/rb-rec-selftest.XXXXXX")"
  dry_run_agent codex "$dr/codex"
  check "dry-run emits codex hooks.json" "1" "$( [ -f "$dr/codex/.codex/hooks.json" ] && echo 1 || echo 0 )"
  check "dry-run emits codex README" "1" "$( [ -f "$dr/codex/README.md" ] && echo 1 || echo 0 )"
  check "dry-run codex README has provenance" "1" "$(grep -cF '## Provenance' "$dr/codex/README.md")"
  check "dry-run codex hooks.json is valid json" "0" "$(python3 -c 'import json,sys; json.load(sys.stdin)' < "$dr/codex/.codex/hooks.json"; echo $?)"
  check "dry-run emits codex terminus.json" "1" "$( [ -f "$dr/codex/terminus.json" ] && echo 1 || echo 0 )"
  check "dry-run codex terminus.json is valid json" "0" "$(python3 -c 'import json,sys; json.load(sys.stdin)' < "$dr/codex/terminus.json"; echo $?)"
  check "dry-run codex README has files section" "1" "$(grep -cF '## Files' "$dr/codex/README.md")"
  dry_run_agent opencode "$dr/opencode"
  check "dry-run emits opencode plugin" "1" "$( [ -f "$dr/opencode/.opencode/plugin/fixture-logger.js" ] && echo 1 || echo 0 )"
  check "dry-run emits opencode README" "1" "$( [ -f "$dr/opencode/README.md" ] && echo 1 || echo 0 )"
  check "dry-run emits opencode terminus.json" "1" "$( [ -f "$dr/opencode/terminus.json" ] && echo 1 || echo 0 )"
  rm -rf "$dr"
  # record_live must REFUSE to run when the agent CLI is absent (fail fast,
  # never silently produce empty fixtures). record_cli_for returns "" for an
  # unknown agent, so record_live's `command -v ""` guard must return nonzero.
  local guard_rc=0
  record_live nonexistent-agent "/tmp/rb-guard-should-not-exist" >/dev/null 2>&1 || guard_rc=$?
  check "record_live refuses absent CLI" "1" "$guard_rc"
  check "record_live wrote nothing for absent CLI" "0" "$( [ -d "/tmp/rb-guard-should-not-exist" ] && echo 1 || echo 0 )"
  check "codex cli name" "codex" "$(record_cli_for codex)"
  check "opencode cli name" "opencode" "$(record_cli_for opencode)"
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

[ -n "$AGENT" ] || { echo "--agent is required for recording (codex|opencode|all)" >&2; exit 2; }
agents="$AGENT"; [ "$AGENT" = "all" ] && agents="codex opencode"
for a in $agents; do
  # Always place each agent under its own <base>/<agent>/ subfolder (the fixture
  # layout contract), whether <base> is the explicit --out-dir or the default.
  # Without the per-agent suffix, `--agent all` clobbers the first agent's
  # README/fixtures with the second's.
  if [ "$DRY_RUN" -eq 1 ]; then
    base="${OUT_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/rb-rec-dry.XXXXXX")}"
    out="$base/$a"
    dry_run_agent "$a" "$out"
    echo "dry-run: generated $a recording layout under $out"
  else
    out="${OUT_DIR:+$OUT_DIR/$a}"
    out="${out:-$FIXTURE_ROOT/$a}"
    record_live "$a" "$out"
  fi
done
