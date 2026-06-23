#!/usr/bin/env bash
# Cross-agent fixture-recording harness (see
# docs/specs/2026-06-23-cross-agent-fixture-recording.md). Records real codex /
# opencode hook-lifecycle payloads + headless-result schema into
# crates/rb-hooks/tests/fixtures/<agent>/. The PURE helpers run under
# `--self-test` with NO API; the live path is guarded behind real CLI auth.
#
# Modes:
#   --self-test                 pure helpers, no API/network (CI gate)
#   --dry-run --agent <a>       generate the recording layout, no CLI call
#   --setup-trust <agent>       ONE-TIME trust step (run once per agent). codex:
#                               launches the TUI in a STABLE recorder home to
#                               persist directory + hook trust; opencode: prepares
#                               the recorder home (no trust gate). Never uses a
#                               --dangerously-bypass flag.
#   --agent <a> [--out-dir D]   live record (needs CLI auth + prior --setup-trust)
#
# Isolation: a STABLE per-agent recorder home OUTSIDE the repo at
# ${XDG_CACHE_HOME:-$HOME/.cache}/rusty-brain/fixture-record/<agent>/ holds the
# persisted trust and a READ-ONLY copy of the operator's auth (mode 0600). The
# real ~/.codex / opencode config is never mutated.
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
  # Agent-specific trust + record-command wording (codex has a hook-trust gate;
  # opencode does not).
  local trust_line record_line
  case "$agent" in
    codex)
      trust_line='codex computes a per-hook `trusted_hash` that cannot be hand-written, so the operator pre-trusts the hooks ONCE via `--setup-trust codex` (interactive TUI); persisted `[hooks.state]` trust then lets recording run with NO `--dangerously-bypass-hook-trust` flag.'
      record_line='`CODEX_HOME=<rec> codex exec --json -C <rec proj> -s workspace-write --skip-git-repo-check -c approval_policy="never" "<prompt>"`' ;;
    opencode)
      trust_line='opencode has NO directory/plugin trust gate, so `--setup-trust opencode` only prepares the recorder home (redirected XDG dirs + copied `auth.json`/`account.json`) and verifies auth; no interactive trust step and no `--dangerously` flag.'
      record_line='`opencode run --format json --dir <rec proj> "<prompt>"` with all XDG dirs redirected to the recorder home' ;;
    *)
      trust_line='Hooks are pre-trusted via `--setup-trust '"$agent"'`; no `--dangerously` bypass flag is used.'
      record_line='the headless CLI invocation logged at record time' ;;
  esac
  {
    printf '# %s Fixture Status\n\n' "$agent"
    printf '## Provenance\n\n- **CLI:** %s\n- **Captured:** %s, %s\n- **Captured by:** scripts/record-agent-fixtures.sh\n- **Events:** %s\n\n' "$ver" "$capture_date" "$os_info" "$events"
    printf '## Recording Recipe\n\nGenerated by `scripts/record-agent-fixtures.sh --agent %s`. The harness uses a\n**stable per-agent recorder home OUTSIDE the repo** at\n`${XDG_CACHE_HOME:-$HOME/.cache}/rusty-brain/fixture-record/%s/` so persisted\ntrust survives across runs. The operator'"'"'s real auth is COPIED in (read-only on\nthe real side, mode 0600), so the real agent home (`~/.codex` /\n`~/.local/share/opencode`) is never mutated.\n\n%s\n\nRecord command: %s. One multi-turn headless session (one Bash + one file write)\nis captured to `result.jsonl`, then sanitized.\n\n' "$agent" "$agent" "$trust_line" "$record_line"
    printf '## Sanitization\n\n| What | Recorded value | Committed value |\n|---|---|---|\n| Home dir (recorder + real) | `/Users/<real>` | `/Users/user` |\n| Secrets (bearer/sk-/AKIA/key=value/`"key":"value"`/PEM) | real | `<redacted>` |\n\nThe recorder home holds the copied auth and session history; it lives outside\nthe repo and is never committed. Only sanitized per-event payloads + the\nsanitized `result.jsonl` are written under `crates/rb-hooks/tests/fixtures/%s/`.\n\n' "$agent"
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
      # `.mjs` so the plugin is unambiguous ESM regardless of package.json (see
      # setup_opencode_home).
      opencode_plugin_src "$out/raw" > "$out/.opencode/plugin/fixture-logger.mjs"
      ;;
    *) echo "dry_run_agent: unknown agent $agent" >&2; return 1 ;;
  esac
  echo '{"verdict":"ambiguous","fired":0,"turns":0}' > "$out/terminus.json"
  emit_readme "$agent" "$out" "dry-run (not recorded)" "ambiguous" "dry-run"
}

record_cli_for() { case "$1" in codex) echo codex ;; opencode) echo opencode ;; *) echo "" ;; esac; }

# recorder_home <agent>: the STABLE per-agent recorder home OUTSIDE the repo.
# Persisted trust (codex [projects]/[hooks.state]) and copied auth live here so
# they survive across runs. The operator's REAL ~/.codex / opencode config is
# never written to; only a read-only COPY of auth lands in this dir. A pure
# function (no side effects) so the self-test can pin its path contract.
recorder_home() { # agent
  printf '%s/rusty-brain/fixture-record/%s\n' "${XDG_CACHE_HOME:-$HOME/.cache}" "$1"
}

# count_lines <file>: emit the line count of <file>, or 0 if it does not exist.
# Guards the `< $file` redirect, which fails (and prints a shell "No such file
# or directory") BEFORE `2>/dev/null` can suppress it when the file is absent.
# Used for both the terminus count and the turn count.
count_lines() { # file
  if [ -f "$1" ]; then wc -l < "$1" | tr -d ' '; else echo 0; fi
}

# codex_trust_config <abs_proj_dir>: emit the recorder CODEX_HOME/config.toml
# body. (a) DIRECTORY trust is hand-writeable: a top-level table keyed by the
# absolute project dir with trust_level="trusted" (validated by `codex doctor`).
# (b) approval_policy="never" keeps the non-interactive `codex exec` run from
# stalling on an approval prompt. HOOK trust (trusted_hash under [hooks.state])
# is NOT hand-written: codex computes the sha256 over the hook entry internally,
# so it is populated by the one-time interactive `--setup-trust` step instead.
codex_trust_config() { # abs_proj_dir
  local proj="$1"
  printf '# Recorder CODEX_HOME config.toml (written by record-agent-fixtures.sh).\n'
  printf '# Directory trust is hand-written; hook trust under [hooks.state] is\n'
  printf '# populated by `--setup-trust codex` (codex computes its hash itself).\n'
  printf '[projects."%s"]\n' "$proj"
  printf 'trust_level = "trusted"\n\n'
  printf '# Non-interactive recording must never block on an approval prompt.\n'
  printf 'approval_policy = "never"\n'
}

# copy_ro <src> <dst>: copy a real-side auth file into the recorder home with
# 0600 perms. The real file is only READ (never modified). Missing source is a
# soft failure (returns 1) so optional files do not abort the recorder.
copy_ro() { # src dst
  [ -f "$1" ] || return 1
  cp -p "$1" "$2" || return 1
  chmod 600 "$2"
}

# setup_codex_home <recorder_home>: prepare a stable CODEX_HOME for recording.
# Copies the operator's real ~/.codex/auth.json (REQUIRED) and the optional
# identity files, seeds config.toml directory trust, and drops the recording
# hooks.json into the recorder project. The real ~/.codex is never written.
# Echoes the recorder project dir on stdout. Hook trust still requires the
# one-time `--setup-trust codex` step (codex writes the trusted_hash).
setup_codex_home() { # recorder_home
  local rec="$1"
  local proj="$rec/proj" raw="$rec/raw"
  mkdir -p "$rec" "$proj/.codex" "$raw"
  chmod 700 "$rec"
  # Required auth (read-only copy of the real credential).
  copy_ro "$HOME/.codex/auth.json" "$rec/auth.json" \
    || { echo "ERROR: missing $HOME/.codex/auth.json; run \`codex login\` first" >&2; return 1; }
  # Optional identity/telemetry files (harmless; not required for auth).
  copy_ro "$HOME/.codex/installation_id" "$rec/installation_id" 2>/dev/null || true
  copy_ro "$HOME/.codex/version.json" "$rec/version.json" 2>/dev/null || true
  # Directory-trust config (hook trust comes from --setup-trust).
  codex_trust_config "$proj" > "$rec/config.toml"
  chmod 600 "$rec/config.toml"
  # The logging hooks the recorder fires through (PostToolUse appends raw stdin).
  codex_hooks_json "$raw" > "$proj/.codex/hooks.json"
  echo "$proj"
}

# setup_opencode_home <recorder_home>: prepare a stable opencode home for
# recording by redirecting every XDG dir under the recorder root and copying the
# operator's real auth.json + account.json (read-only on the real side, 0600).
# opencode has NO directory/plugin trust gate, so no pre-trust step is needed —
# a plugin referenced from opencode.json loads unconditionally. Echoes the
# recorder project dir on stdout. Caller exports the XDG vars from recorder_home.
setup_opencode_home() { # recorder_home
  local rec="$1"
  local proj="$rec/proj" raw="$rec/raw"
  mkdir -p "$rec/data/opencode" "$rec/config/opencode" "$rec/state/opencode" \
           "$rec/cache/opencode" "$proj/.opencode/plugin" "$raw"
  chmod 700 "$rec"
  copy_ro "$HOME/.local/share/opencode/auth.json" "$rec/data/opencode/auth.json" \
    || { echo "ERROR: missing ~/.local/share/opencode/auth.json; run \`opencode auth login\` first" >&2; return 1; }
  # account.json (active-provider index) is recommended so the provider list matches.
  copy_ro "$HOME/.local/share/opencode/account.json" "$rec/data/opencode/account.json" 2>/dev/null || true
  # The recording plugin (registered via opencode.json's `plugin` array). The
  # output uses the `.mjs` extension so it is an unambiguous ESM module under
  # any Bun version opencode bundles (the recorder project has no package.json
  # `"type":"module"`, and a plain `.js` would be CJS by default in stricter
  # runtimes — `.mjs` removes that ambiguity).
  opencode_plugin_src "$raw" > "$proj/.opencode/plugin/fixture-logger.mjs"
  printf '{"plugin":["./.opencode/plugin/fixture-logger.mjs"]}\n' > "$proj/opencode.json"
  echo "$proj"
}

# setup_trust <agent>: the ONE-TIME interactive trust step the operator runs
# once per agent. codex: launches the interactive TUI in the recorder project so
# the operator can approve the directory (if not pre-seeded) AND trust the hooks,
# which persists [hooks.state].trusted_hash into the recorder config.toml; later
# `codex exec` then fires the hooks with NO --dangerously flag. opencode: no
# trust gate exists, so this only verifies auth resolved against the recorder
# home. No --dangerously-bypass flag is ever used.
setup_trust() { # agent
  local agent="$1"
  local cli; cli="$(record_cli_for "$agent")"
  command -v "$cli" >/dev/null 2>&1 || { echo "ERROR: $cli not on PATH; cannot set up trust for $agent" >&2; return 1; }
  local rec; rec="$(recorder_home "$agent")"
  case "$agent" in
    codex)
      local proj; proj="$(setup_codex_home "$rec")" || return 1
      echo "Launching the codex TUI in the recorder home for the ONE-TIME trust step."
      echo "  Recorder CODEX_HOME: $rec"
      echo "  Recorder project:    $proj"
      echo "In the TUI: accept 'trust this directory' (if prompted) and TRUST the hooks, then quit."
      echo "This persists [projects].trust_level + [hooks.state].trusted_hash; recording then needs no --dangerously flag."
      CODEX_HOME="$rec" "$cli" -C "$proj" || true
      echo "Verifying trust persisted (read-only):"
      CODEX_HOME="$rec" "$cli" doctor 2>&1 | grep -Ei 'config|auth' || true
      if grep -q '\[hooks.state\.' "$rec/config.toml" 2>/dev/null; then
        echo "ok: [hooks.state] trusted_hash entries present in $rec/config.toml"
      else
        echo "WARNING: no [hooks.state] entries found yet; re-run --setup-trust codex and trust the hooks in the TUI." >&2
      fi
      ;;
    opencode)
      local rec_data="$rec/data" rec_config="$rec/config" rec_state="$rec/state" rec_cache="$rec/cache"
      setup_opencode_home "$rec" >/dev/null || return 1
      echo "opencode has no directory/plugin trust gate; verifying auth resolved against the recorder home."
      echo "  Recorder home: $rec"
      env XDG_DATA_HOME="$rec_data" XDG_CONFIG_HOME="$rec_config" \
          XDG_STATE_HOME="$rec_state" XDG_CACHE_HOME="$rec_cache" \
          OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 \
          "$cli" auth list || true
      echo "ok: opencode recorder home prepared (no interactive trust step required)."
      ;;
    *) echo "setup_trust: unknown agent $agent" >&2; return 1 ;;
  esac
}

# record_live <agent> <out_dir>: live recording path. Requires the agent CLI and
# real auth, plus a one-time `--setup-trust <agent>` having run (codex hooks must
# already be trusted in the recorder home). Uses the STABLE recorder home outside
# the repo so persisted trust survives; the operator's real agent home is never
# mutated. Captures raw per-event payloads + the headless result stream, counts
# the terminus event across a multi-turn run, sanitizes, and writes fixtures.
# NO --dangerously-bypass flag is used; trust is pre-persisted instead.
record_live() { # agent out_dir
  local agent="$1" out="$2"
  local cli; cli="$(record_cli_for "$agent")"
  command -v "$cli" >/dev/null 2>&1 || { echo "ERROR: $cli not on PATH; cannot record $agent fixtures" >&2; return 1; }
  local ver; ver="$("$cli" --version 2>/dev/null | head -1 || echo unknown)"
  # Provenance the README needs (spec: CLI version, OS, date).
  local os_info capture_date
  os_info="$(uname -s -r 2>/dev/null || echo unknown)"
  capture_date="$(date +%Y-%m-%d)"

  # Real HOME is scrubbed so any path leaked from the copied auth/config rewrites
  # to /Users/user alongside the recorder home path.
  local real_home="${HOME:-}"
  local rec; rec="$(recorder_home "$agent")"

  mkdir -p "$out"
  # A two-step prompt so the terminus event count can be compared against turns.
  local prompt="First run: echo hi via Bash. Then create a file notes.txt containing exactly: recorded. Do both."
  # Capture raw CLI output to a temp file INSIDE the recorder home (outside the
  # repo): if the harness is interrupted between CLI exit and the scrub pass,
  # unscrubbed output (recorder-home path, session details, any echoed token)
  # never lands in the working tree. The sanitized copy is written to
  # $out/result.jsonl only after scrub succeeds.
  local result="$out/result.jsonl"
  local raw_result="$rec/result.jsonl.raw"
  local proj raw

  case "$agent" in
    codex)
      proj="$(setup_codex_home "$rec")" || return 1
      raw="$rec/raw"
      # Refuse to record without the one-time hook trust having been done, so we
      # never silently produce empty fixtures (the exact prior failure mode).
      if ! grep -q '\[hooks.state\.' "$rec/config.toml" 2>/dev/null; then
        echo "ERROR: codex hooks are not trusted in the recorder home yet." >&2
        echo "       Run: bash scripts/record-agent-fixtures.sh --setup-trust codex" >&2
        return 1
      fi
      rm -f "$raw"/*.json 2>/dev/null || true
      # Stable CODEX_HOME (auth + trust pre-seeded). No --dangerously flag.
      #   --json                  -> JSONL event stream on stdout (captured)
      #   -C <proj>               -> working root = recorder project (auto writable_root)
      #   -s workspace-write      -> model may write files under cwd, no bypass
      #   --skip-git-repo-check   -> allow a non-git record dir (does NOT grant trust)
      #   -c approval_policy=never -> non-interactive, never block on approval
      CODEX_HOME="$rec" "$cli" exec \
          --json \
          -C "$proj" \
          -s workspace-write \
          --skip-git-repo-check \
          -c approval_policy="never" \
          -o "$proj/.last-message.txt" \
          "$prompt" >"$raw_result" 2>&1 || true
      ;;
    opencode)
      proj="$(setup_opencode_home "$rec")" || return 1
      raw="$rec/raw"
      rm -f "$raw"/*.json 2>/dev/null || true
      # Redirect ALL opencode dirs into the recorder home so auth resolves there
      # and no global opencode state is touched. RB_FIXTURE_LOG_DIR points the
      # plugin at the raw log dir. --format json streams parseable events. Do NOT
      # pass --pure (it would skip the plugin). No trust gate exists for opencode.
      ( cd "$proj" && env \
          XDG_DATA_HOME="$rec/data" XDG_CONFIG_HOME="$rec/config" \
          XDG_STATE_HOME="$rec/state" XDG_CACHE_HOME="$rec/cache" \
          OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_MODELS_FETCH=1 \
          RB_FIXTURE_LOG_DIR="$raw" \
          "$cli" run --format json --dir "$proj" "$prompt" ) >"$raw_result" 2>&1 || true
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
    # one verbatim line per event). Scrub BOTH the recorder home and the real HOME.
    head -1 "$f" | scrub "$rec" "$real_home" > "$out/$stem.json"
    present="$present${present:+, }$stem"
  done
  # Sanitize the raw result stream (it can echo paths/secrets) from the temp
  # file in the recorder home INTO the committed path. The unscrubbed raw never
  # touches the repo; it is removed once the sanitized copy is written. Two
  # simple commands (not `a && b`) so a scrub failure aborts under set -e
  # instead of leaving an unredacted file behind.
  if [ -f "$raw_result" ]; then
    scrub "$rec" "$real_home" < "$raw_result" > "$result"
    rm -f "$raw_result"
  fi

  # Terminus: count fired terminus events vs turns observed in the result.
  # count_lines guards the `< $file` redirect so an absent file yields 0 without
  # leaking a shell "No such file or directory" into the captured value.
  local fired turns verdict
  fired="$(count_lines "$raw/$terminus_stem.json")"
  turns="$(count_lines "$result")"
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
  # The emitted plugin uses the .mjs extension (unambiguous ESM; the recorder
  # project has no package.json "type":"module"). opencode.json must reference
  # the .mjs path, not a plain .js.
  check "dry-run emits opencode plugin (.mjs)" "1" "$( [ -f "$dr/opencode/.opencode/plugin/fixture-logger.mjs" ] && echo 1 || echo 0 )"
  check "dry-run emits no plain .js plugin" "0" "$( [ -f "$dr/opencode/.opencode/plugin/fixture-logger.js" ] && echo 1 || echo 0 )"
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
  # record_live must ALSO refuse when codex IS on PATH but the recorder
  # config.toml has no persisted [hooks.state.] hook trust (the realistic
  # operator error: --setup-trust codex never run / trust never approved). This
  # is the fail-closed guard against silently producing empty fixtures. Mock a
  # codex on PATH (answers --version), point HOME at a dir holding .codex/auth.json
  # so setup_codex_home's required auth copy succeeds, and point XDG_CACHE_HOME at
  # an isolated recorder root. setup_codex_home regenerates config.toml WITHOUT a
  # [hooks.state.] stanza, so the guard must trip (rc=1) and write no out dir.
  local tg_bin tg_home tg_cache tg_out tg_rc=0
  tg_bin="$(mktemp -d "${TMPDIR:-/tmp}/rb-tg-bin.XXXXXX")"
  tg_home="$(mktemp -d "${TMPDIR:-/tmp}/rb-tg-home.XXXXXX")"
  tg_cache="$(mktemp -d "${TMPDIR:-/tmp}/rb-tg-cache.XXXXXX")"
  tg_out="$(mktemp -d "${TMPDIR:-/tmp}/rb-tg-out.XXXXXX")/codex"
  printf '#!/bin/sh\necho "codex-cli 0.0.0-mock"\n' > "$tg_bin/codex"
  chmod +x "$tg_bin/codex"
  mkdir -p "$tg_home/.codex"
  printf '{"OPENAI_API_KEY":"sk-mock-not-real"}\n' > "$tg_home/.codex/auth.json"
  PATH="$tg_bin:$PATH" HOME="$tg_home" XDG_CACHE_HOME="$tg_cache" \
    record_live codex "$tg_out" >/dev/null 2>&1 || tg_rc=$?
  check "record_live refuses codex trust-not-done" "1" "$tg_rc"
  check "record_live wrote no fixtures when trust missing" "0" \
    "$( [ -f "$tg_out/README.md" ] || [ -f "$tg_out/result.jsonl" ] && echo 1 || echo 0 )"
  rm -rf "$tg_bin" "$tg_home" "$tg_cache" "$(dirname "$tg_out")"
  check "codex cli name" "codex" "$(record_cli_for codex)"
  check "opencode cli name" "opencode" "$(record_cli_for opencode)"
  # recorder_home: STABLE per-agent home OUTSIDE the repo under the XDG cache
  # (or ~/.cache). Path contract is pinned so the README/docs stay accurate.
  local rh_codex rh_oc
  rh_codex="$(XDG_CACHE_HOME=/c recorder_home codex)"
  rh_oc="$(XDG_CACHE_HOME=/c recorder_home opencode)"
  check "recorder_home codex path"   "/c/rusty-brain/fixture-record/codex"    "$rh_codex"
  check "recorder_home opencode path" "/c/rusty-brain/fixture-record/opencode" "$rh_oc"
  check "recorder_home falls back to ~/.cache" "1" "$( (unset XDG_CACHE_HOME; HOME=/h recorder_home codex) | grep -cF '/h/.cache/rusty-brain/fixture-record/codex' )"
  check "recorder_home is outside the repo" "0" "$(printf '%s' "$rh_codex" | grep -cF "$REPO_ROOT")"
  # count_lines: guards the `< file` redirect so an ABSENT file yields 0 with NO
  # leaked shell error (the line-235 bug); a present file yields its line count.
  local cl_tmp; cl_tmp="$(mktemp "${TMPDIR:-/tmp}/rb-cl.XXXXXX")"
  printf 'a\nb\nc\n' > "$cl_tmp"
  check "count_lines counts present file" "3" "$(count_lines "$cl_tmp")"
  check "count_lines absent file => 0"    "0" "$(count_lines /no/such/rb-file 2>&1)"
  rm -f "$cl_tmp"
  # codex_trust_config: hand-writeable DIRECTORY trust + approval_policy; must NOT
  # hand-write a trusted_hash (that is codex-computed via --setup-trust).
  local tc; tc="$(codex_trust_config /abs/rec/proj)"
  check "codex_trust_config writes projects stanza" "1" "$(printf '%s' "$tc" | grep -cF '[projects."/abs/rec/proj"]')"
  check "codex_trust_config sets trust_level"       "1" "$(printf '%s' "$tc" | grep -cF 'trust_level = "trusted"')"
  check "codex_trust_config sets approval_policy"    "1" "$(printf '%s' "$tc" | grep -cF 'approval_policy = "never"')"
  check "codex_trust_config omits trusted_hash"      "0" "$(printf '%s' "$tc" | grep -cF 'trusted_hash')"
  # record_live exposes a --setup-trust mode; opencode's setup is non-interactive
  # (no trust gate). Confirm the dispatcher knows both agents need a known CLI.
  local st_rc=0
  setup_trust nonexistent-agent >/dev/null 2>&1 || st_rc=$?
  check "setup_trust refuses absent CLI" "1" "$st_rc"
  if [ "$fail" -eq 0 ]; then echo "self-test PASS"; return 0; fi
  echo "self-test FAIL" >&2; return 1
}

MODE="record"; AGENT=""; OUT_DIR=""; DRY_RUN=0; TRUST_AGENT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) MODE="self-test"; shift ;;
    --agent)     AGENT="${2:?--agent needs a value}"
                 case "$AGENT" in codex|opencode|all) ;; *) echo "--agent must be codex, opencode, or all (got '$AGENT')" >&2; exit 2 ;; esac
                 shift 2 ;;
    --setup-trust)
                 # Accepts `--setup-trust <agent>`; or `--setup-trust` paired
                 # with `--agent <a>` (the latter resolved after the loop).
                 MODE="setup-trust"
                 case "${2:-}" in
                   codex|opencode) TRUST_AGENT="$2"; shift 2 ;;
                   *) shift ;;
                 esac ;;
    --out-dir)   OUT_DIR="${2:?--out-dir needs a value}"; shift 2 ;;
    --dry-run)   DRY_RUN=1; shift ;;
    -h|--help)   awk 'NR>1 && /^#/ {print; next} NR>1 {exit}' "$0"; exit 2 ;;
    *)           echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ "$MODE" = "self-test" ]; then self_test; exit $?; fi

if [ "$MODE" = "setup-trust" ]; then
  # Resolve the agent from --setup-trust <agent> or a paired --agent <a>.
  [ -n "$TRUST_AGENT" ] || TRUST_AGENT="$AGENT"
  case "$TRUST_AGENT" in
    codex|opencode) ;;
    *) echo "--setup-trust needs an agent: codex or opencode (got '${TRUST_AGENT:-}')" >&2; exit 2 ;;
  esac
  setup_trust "$TRUST_AGENT"; exit $?
fi

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
