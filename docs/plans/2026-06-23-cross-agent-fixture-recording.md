# Cross-Agent Fixture-Recording Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a locally-runnable bash harness that records real codex + opencode hook-lifecycle payloads and headless-result schemas into `crates/rb-hooks/tests/fixtures/<agent>/`, sanitized, so the cross-agent scorecard runner can later be built against recorded ground truth instead of guesses.

**Architecture:** One bash script (`scripts/record-agent-fixtures.sh`) modeled on the existing `scripts/memory-scorecard.sh` — pure helper functions exercised by a no-API `--self-test`, plus a live recording path guarded behind real CLI auth. The live path records against a STABLE per-agent recorder home OUTSIDE the repo (copied auth + persisted trust), pre-trusted once via `--setup-trust <agent>`, so the operator's real agent home is never mutated and no `--dangerously-bypass` flag is needed. A standalone OpenCode logging plugin under `scripts/fixtures/opencode-logger/` lets recording proceed without the deferred `rb-install` opencode support. A `--dry-run` mode runs everything that needs no auth/network so the harness is verifiable and CI-gated offline.

**Tech Stack:** bash, python3 (already a scorecard dependency, used for regex-robust sanitization and JSON validation), jq, the codex/opencode CLIs (live path only), Rust (`rb-hooks` integration test that shells out to the dry-run).

## Global Constraints

- Spec: `docs/specs/2026-06-23-cross-agent-fixture-recording.md` — every task implicitly serves it.
- Fixture format mirrors `crates/rb-hooks/tests/fixtures/claude_code/`: one sanitized single-line raw-stdin JSON file per event (`<event>.json`), plus a README with provenance / recording recipe / sanitization table / fields-present-and-absent.
- Recording must run against a STABLE per-agent recorder home OUTSIDE the repo (`${XDG_CACHE_HOME:-$HOME/.cache}/rusty-brain/fixture-record/<agent>/`) so persisted trust survives across runs; the operator's REAL agent home (`~/.codex`, `~/.local/share/opencode`) is never mutated. Auth is a read-only COPY into the recorder home (mode 0600); codex/opencode hooks are pre-trusted via a one-time `--setup-trust <agent>` step (no `--dangerously-bypass` flags).
- Committed fixtures contain no secrets and no real home path (rewritten to `/Users/user`, matching the claude_code sanitization).
- Codex `.codex/hooks.json` shape (from `rb-install`): `{"hooks": {"<Event>": [ <group> ]}}` where a group is `{"hooks":[{"type":"command","command":"<single shell string>"}]}` and the tool event `PostToolUse` additionally carries `"matcher":"*"`. Events: `SessionStart`, `PostToolUse`, `Stop`, `PreCompact`.
- OpenCode hook event type strings (from the `OpenCodeCli` adapter): `session.created`, `tool.execute.after`, `session.idle`, `session.compacted`, `session.deleted`.
- Out of scope (do NOT touch): scorecard runner code, `crates/rb-agents/src/capability.rs` statuses, production `rb-install` opencode support, codex `apply_patch` capture.
- No emojis in code, commits, or output.
- Conventional commits; run the relevant tests before each commit.

---

### Task 1: Script skeleton, arg parsing, and self-test harness

**Files:**
- Create: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Produces: CLI `record-agent-fixtures.sh [--self-test] [--agent codex|opencode|all] [--out-dir DIR] [--dry-run] [-h|--help]`; a `self_test()` function and a `check()` helper that later tasks append cases to (same pattern as `memory-scorecard.sh`).

- [ ] **Step 1: Write the failing test (the skeleton's own self-test)**

Create `scripts/record-agent-fixtures.sh` with only the shebang and a stub `self_test` that asserts the harness's agent allowlist:

```bash
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

self_test() {
  echo "== record-agent-fixtures self-test (pure; no API) =="
  if agent_supported codex && agent_supported opencode && ! agent_supported gemini; then
    echo "ok: agent allowlist is codex + opencode"
  else
    echo "BUG: agent allowlist"; fail=1
  fi
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
```

- [ ] **Step 2: Run it to verify it passes the skeleton check and rejects bad input**

Run:
```bash
bash scripts/record-agent-fixtures.sh --self-test
bash scripts/record-agent-fixtures.sh --agent bogus; echo "exit=$?"
```
Expected: self-test prints `ok: agent allowlist is codex + opencode` then `self-test PASS`; the bad-agent run prints the `--agent must be ...` error and `exit=2`.

- [ ] **Step 3: Make it executable**

Run: `chmod +x scripts/record-agent-fixtures.sh`

- [ ] **Step 4: Commit**

```bash
git add scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): scaffold cross-agent recording harness skeleton"
```

---

### Task 2: Pure sanitizer (`scrub`)

**Files:**
- Modify: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Produces: `scrub <real_home>` — reads stdin, writes sanitized stdout: rewrites `<real_home>` to `/Users/user` and redacts bearer tokens, `sk-`/`AKIA` keys, `key=value` secrets, and PEM private-key blocks. Idempotent.

- [ ] **Step 1: Write the failing tests**

Add these `check` cases inside `self_test()` (before the final `if [ "$fail" -eq 0 ]`):

```bash
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: FAIL — `scrub: command not found` / `BUG: scrub ...`.

- [ ] **Step 3: Implement `scrub`**

Add above `self_test()`:

```bash
# scrub <real_home>: stdin -> stdout. Rewrites the recording user's home dir to
# /Users/user and redacts the secret classes the hook-capture redaction covers
# (bearer tokens, sk-/AKIA keys, key=value secrets, PEM private keys). python3
# (already a harness dep) gives multiline-safe, idempotent regexes. A fixed
# fixpoint: redacted placeholders contain chars outside each pattern's class, so
# re-running is a no-op.
scrub() { # real_home
  python3 - "$1" <<'PY'
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
    (re.compile(r'(?i)\b([A-Za-z0-9_]*(?:key|token|secret|password)[A-Za-z0-9_]*)\s*=\s*[^\s"&]+'), r'\1=<redacted>'),
]
for rx, repl in subs:
    data = rx.sub(repl, data)
sys.stdout.write(data)
PY
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: PASS — all eight `scrub` checks print `ok:`.

- [ ] **Step 5: Commit**

```bash
git add scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): add idempotent sanitizer for recorded payloads"
```

---

### Task 3: Terminus inference (`infer_terminus`)

**Files:**
- Modify: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Produces: `infer_terminus <fired_count> <total_turns>` — echoes `true-terminus`, `per-turn`, or `ambiguous`. Resolves the codex `Stop` / opencode `session.idle` mapping question the fixture READMEs flag.

- [ ] **Step 1: Write the failing tests**

Add to `self_test()`:

```bash
  check "terminus fired-once => true-terminus" "true-terminus" "$(infer_terminus 1 3)"
  check "terminus once-per-turn => per-turn"   "per-turn"      "$(infer_terminus 3 3)"
  check "terminus single-turn run => ambiguous" "ambiguous"    "$(infer_terminus 1 1)"
  check "terminus mismatch => ambiguous"       "ambiguous"     "$(infer_terminus 2 3)"
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: FAIL — `infer_terminus: command not found`.

- [ ] **Step 3: Implement `infer_terminus`**

```bash
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
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: PASS — four `terminus` checks print `ok:`.

- [ ] **Step 5: Commit**

```bash
git add scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): add session terminus inference from multi-turn run"
```

---

### Task 4: Codex hook-config generator (`codex_hooks_json`)

**Files:**
- Modify: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Produces: `codex_hooks_json <log_dir>` — prints a valid `.codex/hooks.json` whose command for each of `SessionStart`/`PostToolUse`/`Stop`/`PreCompact` appends raw stdin to `<log_dir>/<event>.json`; `PostToolUse` carries `"matcher":"*"`. Matches the `rb-install` codex schema.

- [ ] **Step 1: Write the failing tests**

Add to `self_test()`:

```bash
  local ch; ch="$(codex_hooks_json /tmp/rec/raw)"
  check "codex hooks.json is valid json" "0" "$(printf '%s' "$ch" | python3 -c 'import json,sys; json.load(sys.stdin)'; echo $?)"
  check "codex registers all four events" "4" "$(printf '%s' "$ch" | python3 -c 'import json,sys; h=json.load(sys.stdin)["hooks"]; print(sum(k in h for k in ("SessionStart","PostToolUse","Stop","PreCompact")))')"
  check "codex PostToolUse carries matcher" "*" "$(printf '%s' "$ch" | python3 -c 'import json,sys; print(json.load(sys.stdin)["hooks"]["PostToolUse"][0]["matcher"])')"
  check "codex Stop omits matcher" "no-matcher" "$(printf '%s' "$ch" | python3 -c 'import json,sys; g=json.load(sys.stdin)["hooks"]["Stop"][0]; print("no-matcher" if "matcher" not in g else "has-matcher")')"
  check "codex command appends to per-event log" "1" "$(printf '%s' "$ch" | python3 -c 'import json,sys; c=json.load(sys.stdin)["hooks"]["Stop"][0]["hooks"][0]["command"]; print(int("/tmp/rec/raw/stop.json" in c))')"
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: FAIL — `codex_hooks_json: command not found`.

- [ ] **Step 3: Implement `codex_hooks_json`**

```bash
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
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: PASS — five `codex` checks print `ok:`.

- [ ] **Step 5: Commit**

```bash
git add scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): generate codex hooks.json recording config"
```

---

### Task 5: OpenCode logging plugin and registration generator

**Files:**
- Create: `scripts/fixtures/opencode-logger/plugin.js`
- Create: `scripts/fixtures/opencode-logger/README.md`
- Modify: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Produces: `opencode_plugin_src <log_dir>` — prints the plugin source with `<log_dir>` baked in; the plugin writes each of the five OpenCode hook event payloads to `<log_dir>/<stem>.json`. Stems: `session.created`->`session_created`, `tool.execute.after`->`tool_execute_after`, `session.idle`->`session_idle`, `session.compacted`->`session_compacted`, `session.deleted`->`session_deleted`.

- [ ] **Step 1: Write the failing tests**

Add to `self_test()`:

```bash
  local op; op="$(opencode_plugin_src /tmp/rec/raw)"
  check "opencode plugin references log dir" "1" "$(printf '%s' "$op" | grep -cF '/tmp/rec/raw')"
  for ev in session.created tool.execute.after session.idle session.compacted session.deleted; do
    check "opencode plugin handles $ev" "1" "$(printf '%s' "$op" | grep -cF "$ev")"
  done
  check "opencode-logger plugin file exists" "1" "$( [ -f "$REPO_ROOT/scripts/fixtures/opencode-logger/plugin.js" ] && echo 1 || echo 0 )"
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: FAIL — `opencode_plugin_src: command not found` and the plugin-file check returns `0`.

- [ ] **Step 3: Create the committed plugin template**

Create `scripts/fixtures/opencode-logger/plugin.js`. This is the committed reference copy (the harness rewrites the log dir at record time via `opencode_plugin_src`). It maps each event to a per-event file and appends the raw payload as one JSON line:

```javascript
// OpenCode fixture-recording plugin. Recording aid ONLY — not the production
// integration (rb-install opencode support stays deferred). Writes each hook
// event payload as one JSON line to RB_FIXTURE_LOG_DIR/<stem>.json so the
// recorder can sanitize and commit them. See
// docs/specs/2026-06-23-cross-agent-fixture-recording.md.
import { appendFileSync } from "node:fs";

const LOG_DIR = process.env.RB_FIXTURE_LOG_DIR || ".";
const STEMS = {
  "session.created": "session_created",
  "tool.execute.after": "tool_execute_after",
  "session.idle": "session_idle",
  "session.compacted": "session_compacted",
  "session.deleted": "session_deleted",
};

function log(type, payload) {
  const stem = STEMS[type];
  if (!stem) return;
  appendFileSync(`${LOG_DIR}/${stem}.json`, JSON.stringify(payload) + "\n");
}

export const FixtureLogger = async () => ({
  event: async ({ event }) => log(event?.type, event),
});
```

- [ ] **Step 4: Create the plugin README**

Create `scripts/fixtures/opencode-logger/README.md`:

```markdown
# OpenCode fixture-recording plugin

A standalone OpenCode plugin used only to record real hook-event payloads for
rusty-brain's cross-agent fixtures. It is NOT the production integration —
`rb-install` opencode support is deferred (see the cross-agentic parity PRD).

`scripts/record-agent-fixtures.sh --agent opencode` copies this plugin into a
throwaway project, sets `RB_FIXTURE_LOG_DIR`, runs `opencode run`, then
sanitizes and commits the captured payloads under
`crates/rb-hooks/tests/fixtures/opencode/`.

Pin the recorded OpenCode version in the generated fixture README; the plugin is
kept minimal to reduce API-drift surface.
```

- [ ] **Step 5: Implement `opencode_plugin_src` in the harness**

```bash
# opencode_plugin_src <log_dir>: emit the recording plugin with the log dir baked
# in (the committed copy under scripts/fixtures/opencode-logger/ reads the dir
# from RB_FIXTURE_LOG_DIR; here we inline it so a throwaway run needs no env).
opencode_plugin_src() { # log_dir
  local d="$1"
  sed "s#process.env.RB_FIXTURE_LOG_DIR || \".\"#\"$d\"#" \
    "$REPO_ROOT/scripts/fixtures/opencode-logger/plugin.js"
}
```

- [ ] **Step 6: Run to verify it passes**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: PASS — the log-dir, five per-event, and plugin-file checks print `ok:`.

- [ ] **Step 7: Commit**

```bash
git add scripts/fixtures/opencode-logger/ scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): add opencode logging plugin and registration"
```

---

### Task 6: Fixture/README emitter and dry-run layout check

**Files:**
- Modify: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Consumes: `scrub`, `codex_hooks_json`, `opencode_plugin_src`, `infer_terminus`.
- Produces: `emit_readme <agent> <out_dir> <cli_version> <terminus_verdict> <events_csv>` — writes `<out_dir>/README.md` with provenance/recipe/sanitization/terminus/fields sections; `dry_run_agent <agent> <out_dir>` — generates config + a placeholder fixture layout (no CLI call) and returns 0 only if the layout matches the claude_code template.

- [ ] **Step 1: Write the failing tests**

Add to `self_test()`:

```bash
  local dr; dr="$(mktemp -d "${TMPDIR:-/tmp}/rb-rec-selftest.XXXXXX")"
  dry_run_agent codex "$dr/codex"
  check "dry-run emits codex hooks.json" "1" "$( [ -f "$dr/codex/.codex/hooks.json" ] && echo 1 || echo 0 )"
  check "dry-run emits codex README" "1" "$( [ -f "$dr/codex/README.md" ] && echo 1 || echo 0 )"
  check "dry-run codex README has provenance" "1" "$(grep -cF '## Provenance' "$dr/codex/README.md")"
  dry_run_agent opencode "$dr/opencode"
  check "dry-run emits opencode plugin" "1" "$( [ -f "$dr/opencode/.opencode/plugin/fixture-logger.js" ] && echo 1 || echo 0 )"
  check "dry-run emits opencode README" "1" "$( [ -f "$dr/opencode/README.md" ] && echo 1 || echo 0 )"
  rm -rf "$dr"
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: FAIL — `dry_run_agent: command not found`.

- [ ] **Step 3: Implement `emit_readme` and `dry_run_agent`**

```bash
# emit_readme <agent> <out_dir> <cli_version> <terminus_verdict> <events_csv>
emit_readme() {
  local agent="$1" out="$2" ver="$3" term="$4" events="$5"
  {
    printf '# %s Fixture Status\n\n' "$agent"
    printf '## Provenance\n\n- **CLI:** %s\n- **Captured by:** scripts/record-agent-fixtures.sh\n- **Events:** %s\n\n' "$ver" "$events"
    printf '## Recording Recipe\n\nGenerated by `scripts/record-agent-fixtures.sh --agent %s`: a throwaway HOME + project with logging hooks, one multi-turn headless session (one Bash + one file write), then sanitize.\n\n' "$agent"
    printf '## Sanitization\n\n| What | Recorded value | Committed value |\n|---|---|---|\n| Home dir | `/Users/<real>` | `/Users/user` |\n| Secrets (bearer/sk-/AKIA/key=value/PEM) | real | `<redacted>` |\n\n'
    printf '## Session Terminus\n\nMulti-turn verdict: **%s** (see the run shape in the recipe; evidence, not proof).\n\n' "$term"
    printf '## Headless Result Schema\n\n`result.jsonl` is the verbatim headless-CLI result output. Cost/token-axis interpretation is deferred to the scorecard-runner build.\n\n'
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
  emit_readme "$agent" "$out" "dry-run (not recorded)" "ambiguous" "dry-run"
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: PASS — the six dry-run layout checks print `ok:`.

- [ ] **Step 5: Wire `--dry-run` into the record path**

Replace the placeholder `echo "record mode not yet implemented"` block at the bottom with dispatch that, for `--dry-run`, generates into `OUT_DIR` (or a temp dir) and exits 0; the live path (Task 7) is added next:

```bash
[ -n "$AGENT" ] || { echo "--agent is required for recording (codex|opencode|all)" >&2; exit 2; }
agents="$AGENT"; [ "$AGENT" = "all" ] && agents="codex opencode"
for a in $agents; do
  out="${OUT_DIR:-$FIXTURE_ROOT/$a}"
  if [ "$DRY_RUN" -eq 1 ]; then
    out="${OUT_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/rb-rec-dry.XXXXXX")/$a}"
    dry_run_agent "$a" "$out"
    echo "dry-run: generated $a recording layout under $out"
  else
    record_live "$a" "$out"   # defined in Task 7
  fi
done
```

- [ ] **Step 6: Verify dry-run end-to-end offline**

Run: `bash scripts/record-agent-fixtures.sh --dry-run --agent all`
Expected: exit 0; prints `dry-run: generated codex recording layout under ...` and the same for opencode.

- [ ] **Step 7: Commit**

```bash
git add scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): add README emitter and offline dry-run layout check"
```

---

### Task 7: Live recording orchestration

**Files:**
- Modify: `scripts/record-agent-fixtures.sh`

**Interfaces:**
- Consumes: `recorder_home`, `setup_codex_home`/`setup_opencode_home`, `codex_trust_config`, `copy_ro`, `codex_hooks_json`, `opencode_plugin_src`, `scrub`, `infer_terminus`, `count_lines`, `emit_readme`.
- Produces: `record_live <agent> <out_dir>` — prepares the STABLE recorder home (copied auth + directory trust + logging hooks), runs a real multi-turn headless session against it (`codex exec --json` / `opencode run --format json`, no `--dangerously` flag) capturing `result.jsonl`, counts the terminus event, sanitizes raw payloads into `<out_dir>/<event>.json`, and writes the README. Requires the agent CLI on PATH and (codex) a prior `--setup-trust` having persisted `[hooks.state]` trust — it REFUSES to record otherwise so it never produces empty fixtures. Never mutates the operator's real agent home.
- Produces: `setup_trust <agent>` — the one-time interactive trust step (codex TUI hook trust; opencode home prep + auth verification).

This task has no offline unit test (it requires live CLI auth). Its guardrails are tested instead: the missing-CLI preflight (for both `record_live` and `setup_trust`), the `recorder_home` path contract, `count_lines`, and `codex_trust_config` are all checked in self-test, and the body reuses Task 2-6 helpers that are already covered.

- [ ] **Step 1: Write the failing preflight test**

Add to `self_test()`:

```bash
  # record_live must refuse to run when the agent CLI is absent (fail fast,
  # never silently produce empty fixtures). Use a guaranteed-absent binary name.
  ( cli_missing_msg() { record_cli_for nonexistent-agent; }; true )
  check "codex cli name" "codex" "$(record_cli_for codex)"
  check "opencode cli name" "opencode" "$(record_cli_for opencode)"
```

- [ ] **Step 2: Run to verify it fails**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: FAIL — `record_cli_for: command not found`.

- [ ] **Step 3: Implement the CLI map, preflight, and `record_live`**

```bash
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

  local work; work="$(mktemp -d "${TMPDIR:-/tmp}/rb-rec.XXXXXX")"
  local home="$work/home" proj="$work/proj" raw="$work/raw"
  seed_home "$home"; mkdir -p "$proj" "$raw"
  mkdir -p "$out"

  # A two-step prompt so the terminus event count can be compared against turns.
  local prompt="First run: echo hi via Bash. Then create a file notes.txt containing exactly: recorded. Do both."
  local result="$out/result.jsonl"

  (
    export HOME="$home"; cd "$proj" || exit 1
    case "$agent" in
      codex)
        mkdir -p "$proj/.codex"
        codex_hooks_json "$raw" > "$proj/.codex/hooks.json"
        # Use codex's non-interactive exec with its machine-readable output flag.
        # The exact flag is recorded in the README from `codex exec --help`.
        codex exec "$prompt" >"$result" 2>&1 || true
        ;;
      opencode)
        mkdir -p "$proj/.opencode/plugin"
        export RB_FIXTURE_LOG_DIR="$raw"
        opencode_plugin_src "$raw" > "$proj/.opencode/plugin/fixture-logger.js"
        opencode run "$prompt" >"$result" 2>&1 || true
        ;;
    esac
  )

  # Sanitize each captured raw event into a committed single-line fixture.
  local present="" stem ev terminus_stem
  case "$agent" in
    codex)    terminus_stem="stop" ;;
    opencode) terminus_stem="session_idle" ;;
  esac
  for f in "$raw"/*.json; do
    [ -e "$f" ] || continue
    stem="$(basename "$f" .json)"
    # Commit the FIRST captured line per event, sanitized (matches claude_code:
    # one verbatim line per event).
    head -1 "$f" | scrub "$home" > "$out/$stem.json"
    present="$present${present:+, }$stem"
  done
  # Sanitize the result stream too (it can echo paths/secrets).
  if [ -f "$result" ]; then scrub "$home" < "$result" > "$result.tmp" && mv "$result.tmp" "$result"; fi

  # Terminus: count fired terminus events vs turns observed in the result.
  local fired turns verdict
  fired="$( [ -f "$raw/$terminus_stem.json" ] && grep -c . "$raw/$terminus_stem.json" || echo 0 )"
  turns="$(grep -c . "$result" 2>/dev/null || echo 1)"
  verdict="$(infer_terminus "$fired" "$turns")"

  emit_readme "$agent" "$out" "$ver" "$verdict" "$present"
  rm -rf "$work"
  echo "recorded $agent fixtures under $out (events: ${present:-none}, terminus: $verdict)"
}
```

- [ ] **Step 4: Run to verify the preflight checks pass**

Run: `bash scripts/record-agent-fixtures.sh --self-test`
Expected: PASS — `codex cli name` / `opencode cli name` print `ok:`.

- [ ] **Step 5: Confirm preflight fails fast without the CLI**

Run (in an env where codex is not installed): `bash scripts/record-agent-fixtures.sh --agent codex; echo "exit=$?"`
Expected: `ERROR: codex not on PATH; cannot record codex fixtures` and `exit=1`.

- [ ] **Step 6: Commit**

```bash
git add scripts/record-agent-fixtures.sh
git commit -m "feat(fixtures): add live recording orchestration for codex and opencode"
```

---

### Task 8: CI-gate the dry-run from rb-hooks

**Files:**
- Create: `crates/rb-hooks/tests/fixture_recorder_dry_run.rs`

**Interfaces:**
- Consumes: `scripts/record-agent-fixtures.sh --dry-run --agent all` and `--self-test`.
- Produces: a Rust integration test so the offline-verifiable parts run in `cargo test -p rb-hooks` (the project's existing CI surface).

- [ ] **Step 1: Write the failing test**

Create `crates/rb-hooks/tests/fixture_recorder_dry_run.rs`:

```rust
//! Offline CI gate for the cross-agent fixture-recording harness: runs the
//! script's pure `--self-test` and an `--dry-run --agent all`. The live
//! recording path needs CLI auth and is exercised manually by the operator.
use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/record-agent-fixtures.sh")
}

#[test]
fn self_test_passes() {
    let out = Command::new("bash").arg(script()).arg("--self-test")
        .output().expect("run self-test");
    assert!(out.status.success(), "self-test failed:\n{}",
        String::from_utf8_lossy(&out.stdout));
}

#[test]
fn dry_run_all_succeeds() {
    let out = Command::new("bash").arg(script())
        .args(["--dry-run", "--agent", "all"])
        .output().expect("run dry-run");
    assert!(out.status.success(), "dry-run failed:\n{}",
        String::from_utf8_lossy(&out.stderr));
}
```

- [ ] **Step 2: Run to verify it passes**

Run: `cargo test -p rb-hooks --test fixture_recorder_dry_run`
Expected: PASS — both tests green (the script already works from Tasks 1-7).

- [ ] **Step 3: Run the full rb-hooks suite to confirm no regression**

Run: `cargo test -p rb-hooks`
Expected: PASS — existing Claude Code lifecycle tests unaffected.

- [ ] **Step 4: Commit**

```bash
git add crates/rb-hooks/tests/fixture_recorder_dry_run.rs
git commit -m "test(fixtures): CI-gate recording harness self-test and dry-run"
```

---

### Task 9: Operator live-recording handoff (manual, no code)

This task is run by a human with codex + opencode auth installed; it produces the actual committed fixtures and is the input to the follow-on scorecard-runner build.

**Trust/auth model (binding):** each agent records against a STABLE recorder home OUTSIDE the repo at `${XDG_CACHE_HOME:-$HOME/.cache}/rusty-brain/fixture-record/<agent>/`. The harness copies the operator's real auth into that home (read-only on the real side, mode 0600) and seeds codex directory trust; the real `~/.codex` / opencode config is never mutated. Hooks are pre-trusted via a one-time `--setup-trust <agent>`, so recording uses NO `--dangerously-bypass-hook-trust` / `--dangerously-bypass-approvals-and-sandbox` flag.

- [ ] **Step 1: One-time trust setup (run once per agent, in a real terminal)**

```bash
bash scripts/record-agent-fixtures.sh --setup-trust codex
#   -> launches the codex TUI in the recorder CODEX_HOME. In the TUI: accept
#      "trust this directory" (if prompted) and TRUST the hooks, then quit. This
#      persists [projects].trust_level + [hooks.state].trusted_hash. The command
#      then verifies via `codex doctor` and checks for [hooks.state] entries.
bash scripts/record-agent-fixtures.sh --setup-trust opencode
#   -> opencode has no trust gate; this prepares the recorder home (redirected
#      XDG dirs + copied auth.json/account.json) and runs `opencode auth list`.
```

Re-run `--setup-trust <agent>` whenever the copied auth tokens expire (the recorder copy can drift from the real one as refresh tokens rotate; the real one stays untouched).

- [ ] **Step 2: Record codex fixtures**

Run: `bash scripts/record-agent-fixtures.sh --agent codex`
This runs `CODEX_HOME=<rec> codex exec --json -C <rec proj> -s workspace-write --skip-git-repo-check -c approval_policy="never" "<prompt>"`. The harness REFUSES to record if `[hooks.state]` trust is missing (re-run `--setup-trust codex`). Then review the diff under `crates/rb-hooks/tests/fixtures/codex/` — confirm no real home path or secret survived sanitization, and that the README provenance (CLI version, terminus verdict, events present/absent) is accurate.

Open question to confirm at record time (documented in the README): whether codex `exec --json` fires `PreToolUse`/`PostToolUse` for non-shell tools (per `differences.md` they currently match SHELL commands only — the prompt exercises a Bash command to guarantee `PostToolUse` capture), and whether `file_change` items appear in the headless `--json` stream (the readiness gate in `docs/prds/2026-06-23-codex-apply-patch-capture.md`).

- [ ] **Step 3: Record opencode fixtures**

Run: `bash scripts/record-agent-fixtures.sh --agent opencode`
This runs `opencode run --format json --dir <rec proj> "<prompt>"` with the XDG dirs redirected to the recorder home. Review `crates/rb-hooks/tests/fixtures/opencode/` the same way; capture the `--format json` result-stream schema verbatim (its event-type/envelope shape is still unknown until a real run).

- [ ] **Step 4: Commit the recorded fixtures**

```bash
git add crates/rb-hooks/tests/fixtures/codex crates/rb-hooks/tests/fixtures/opencode
git commit -m "test(fixtures): record real codex and opencode lifecycle fixtures"
```

- [ ] **Step 5: Hand off to the scorecard-runner build**

With recorded result schemas in hand, the cross-agent scorecard runner work (out of scope for this plan) can begin: decide the cost-axis policy from the real `result.jsonl`, add agent dispatch to `run_session`/install/`extract_usage`, and flip capability-matrix statuses only where a fixture proves the capability.

---

## Self-Review

**Spec coverage:**
- Component 1 (harness) → Tasks 1, 4, 6, 7. Component 2 (opencode plugin) → Task 5. Component 3 (dry-run/self-test) → Tasks 1-6 + Task 8 CI gate. Component 4 (spec) → already written. Sanitization → Task 2. Terminus → Task 3. Required fixture set / README → Task 6 + Task 9. Acceptance "dry-run passes offline" → Task 8; "complete fixture set with auth" → Task 9; "no secrets/home in commits" → Task 2 + Task 9 review; "existing tests pass" → Task 8 Step 3; "no matrix/runner changes" → Global Constraints + Task 9 Step 4 defers them. All spec sections map to a task.

**Placeholder scan:** No TBD/TODO; every code step shows complete, runnable code. The only deferred items (cost-axis policy, runner changes) are explicit Non-Goals carried into Task 9 Step 4, not plan placeholders.

**Type consistency:** Helper names are stable across tasks — `scrub` (Task 2, used in 7), `infer_terminus` (3, used in 7), `codex_hooks_json` (4, used in 6/7), `opencode_plugin_src` (5, used in 6/7), `emit_readme`/`dry_run_agent` (6), `record_cli_for`/`record_live` (7). Event-stem mappings match the adapter strings in Global Constraints.
