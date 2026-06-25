# Spec: Cross-Agent Fixture-Recording Harness (codex + opencode)

- **Date:** 2026-06-23
- **Status:** Design, ready for implementation of the recording harness. The scorecard runner work it unblocks is explicitly out of scope here.
- **Relationship:** prerequisite ("Worker B evidence") for the agent-scorecard slice of [cross-agentic agent parity](../prds/2026-06-23-cross-agentic-agent-parity.md); ordered per [next-work sequencing](../prds/2026-06-23-next-work-sequencing.md) ("fixture collection comes first; mapping/code changes come second").

## Problem

The memory-value scorecard (`scripts/memory-scorecard.sh`) and the rb-hooks lifecycle tests are both grounded in **Claude Code**:

- the live runner shells out to `claude -p ... --output-format stream-json --verbose` and parses Claude's `result` record (`num_turns`, `total_cost_usd`, `usage.*` cache buckets) in `extract_usage`;
- install uses `.claude/settings.json` + `.mcp.json`;
- rb-hooks tests parse real recorded payloads under `crates/rb-hooks/tests/fixtures/claude_code/`.

For codex and opencode the equivalent ground truth does not exist. The three non-Claude fixture READMEs (`codex/`, `opencode/`, `gemini/`) are placeholders that state recording was **blocked in-worktree** (restricted network/auth, and live runs would mutate user/global agent state). Concretely missing today:

- no real hook-lifecycle payloads for any non-Claude agent;
- no verified session **terminus** (whether codex `Stop` / opencode `session.idle` is a true end-of-session or a per-turn boundary) — this blocks the SessionEnd-equivalent capture fold;
- no recorded **headless result schema** for `codex exec` / `opencode run`, which the future runner's `extract_usage`-equivalent must parse;
- `rb-install` opencode support is deferred (`E_INSTALL_AGENT_DEFERRED`, needs a JS/TS plugin), so recording cannot rely on the installer.

Building a "real" codex/opencode scorecard before this evidence exists would require inventing result schemas and asserting capture/retrieval parity the capability matrix marks `unsupported`/`unknown` — exactly what the parity PRD forbids ("do not pretend parity exists"; "fixture-gated mappings").

## Goals

- Provide a locally-runnable harness that records **real** codex + opencode hook-lifecycle payloads and headless-result schemas, sanitized, into `crates/rb-hooks/tests/fixtures/<agent>/`, matching the `claude_code/` fixture format.
- Determine the session **terminus** for each agent from a multi-turn run (fired-once vs per-turn).
- Capture each agent's headless-result output **verbatim** (the cost/token-axis decision is deferred until the real schema is in hand — see Non-Goals).
- Author a standalone OpenCode logging plugin so recording does not wait on deferred `rb-install` support.
- Be verifiable offline (no auth/network) via a dry-run/self-test, so the harness itself is tested without recording.
- Document provenance + sanitization so the recorded fixtures are committable as ground truth, replacing the "blocked" placeholder READMEs.

## Non-Goals

These stay gated on the fixtures this harness produces; none are implemented here:

- Scorecard runner changes (agent dispatch for `run_session`/install/`extract_usage`).
- Flipping any capability-matrix status in `crates/rb-agents/src/capability.rs` from `partial`/`unsupported`/`unknown`.
- `rb-install` OpenCode plugin support (production install path; the harness ships only a *recording* plugin).
- Codex `apply_patch` capture (its own PRD; record the event shape opportunistically if it fires, but implement nothing).
- The cost/token-axis policy for non-Claude agents. The harness records the full result schema verbatim; how the future runner treats a missing cost axis is decided once we see what codex/opencode emit.
- Gemini (covered by its own placeholder; not requested in this slice).

## Design

### Component 1 — `scripts/record-agent-fixtures.sh`

`record-agent-fixtures.sh [--self-test] [--setup-trust codex|opencode] [--agent codex|opencode|all] [--out-dir DIR] [--dry-run]`

For each agent, operate against a **stable per-agent recorder home OUTSIDE the repo** at `${XDG_CACHE_HOME:-$HOME/.cache}/rusty-brain/fixture-record/<agent>/` so persisted trust survives across runs. The isolation level differs by agent — codex is fully isolated; opencode is a documented exception (see the per-agent bullets below) — so the "no real-home mutation" guarantee is **codex-specific, not blanket**. The earlier "bare throwaway HOME" approach is the precise blocker the prior recording attempt hit: codex ran in an empty `$HOME/.codex` with no auth and an untrusted directory/hooks, so it captured zero events. The corrected isolation model:

- **codex** needs isolation because it has auth + trust gates: the recorder home (mode 0700) holds a **read-only COPY** of `~/.codex/auth.json` (mode 0600) under `CODEX_HOME=<recorder home>`, plus a hand-written `[projects."<rec proj>"] trust_level = "trusted"` + `approval_policy = "never"` in `config.toml`. The real `~/.codex` is never written.
- **opencode** has no trust gate and authenticated fine from its real config, so it records under the operator's **REAL** `~/.config/opencode` / `~/.local/share/opencode` (the working model + permissions + auth). Only a project-local plugin + `RB_FIXTURE_LOG_DIR=<rec>/raw` are recorder-specific. (An earlier attempt redirected all XDG dirs into a fresh recorder home; that fresh config defaulted to a model the operator's plan excluded and retry-looped forever — so opencode deliberately uses the real config. The one accepted side effect is a single session entry in the real opencode history.)
- **One-time trust** (`--setup-trust <agent>`): codex computes a per-hook `trusted_hash` over `.codex/hooks.json` that cannot be hand-written, so the operator runs the interactive `codex` TUI ONCE in the recorder home to persist `[hooks.state]` trust; later `codex exec` then fires the hooks with **NO** `--dangerously-bypass-hook-trust` flag. opencode has no trust gate, so its `--setup-trust` just prepares the recorder project and verifies auth.
- **Hard timeout**: every live agent run is wrapped in `timeout`/`gtimeout` (`RB_REC_TIMEOUT_SECS`, default 180s) with `</dev/null`, so a stuck or retry-looping session can never hang the recorder (the exact opencode failure above).

1. **Register logging hooks** that append each event's raw stdin JSON (plus a trailing newline, matching the claude_code recipe) to `raw/<event>.json`:
   - **codex** → recorder-project `.codex/hooks.json`, `type: command` hooks for `SessionStart`, `PostToolUse` (matcher `*`), `Stop`, `PreCompact`.
   - **opencode** → the Component 2 plugin, registering `session.created`, `tool.execute.after`, `session.idle`, `session.compacted`, `session.deleted`.
2. **Drive a multi-turn headless session** that performs one Bash command and one file write (so `PostToolUse` / `tool.execute.after` fire), capturing the full machine-readable result stream to `result.jsonl` (or `result.json` if the agent emits a single object). No `--dangerously` bypass flag is used; trust is pre-persisted in the recorder home instead:
   - codex → `CODEX_HOME=<rec> timeout 180 codex exec --json -C <rec proj> -s workspace-write --skip-git-repo-check -c approval_policy="never" "<prompt>" </dev/null`.
   - opencode → `RB_FIXTURE_LOG_DIR=<rec>/raw timeout 180 opencode run --format json --dir <rec proj> "<prompt>" </dev/null`, using the operator's real opencode config/auth (no XDG redirect).
3. **Terminus determination**: count occurrences of the candidate terminus event (`Stop` / `session.idle`) across the multi-turn run; emit a `terminus.json` note recording the count and the inferred verdict (`true-terminus` if fired exactly once, `per-turn` if it fires once per turn, else `ambiguous`). The "turns" figure is a **line count of the result stream**, an approximation — not an authoritative `num_turns` field (the result schema is captured verbatim and deferred, per the Non-Goals) — so the verdict is intentionally `ambiguous` whenever the terminus event does not fire exactly once; the raw `fired`/`turns` counts are recorded for inspection rather than trusted as proof. **Empirical result (opencode, gpt-5.5):** `session.idle` fired **twice** in a single non-interactive `opencode run`, so it is NOT a clean once-per-session terminus — the follow-on runner must not treat `session.idle` as end-of-session. This is exactly the mapping question the fixture READMEs flag, now answered with evidence.
4. **Sanitize** before writing committed fixtures: replace the recording user's home dir with `/Users/user`; scrub the same secret classes the hook-capture redaction covers (bearer tokens, `key=value`, AWS-style keys, PEM blocks). The harness is bash, so it reimplements those patterns rather than calling the Rust redactor; the sanitizer is a shell function that shells out to `python3` (already a harness dependency) for multiline-safe, idempotent regexes, exercised by the dry-run, and a dry-run case pins parity with a known secret of each class.
5. **Emit** per-event fixture files into `crates/rb-hooks/tests/fixtures/<agent>/` and regenerate that agent's README provenance + sanitization + terminus + result-schema sections, replacing the "blocked" placeholder.

### Component 2 — OpenCode logging plugin (`scripts/fixtures/opencode-logger/`)

A minimal standalone JS plugin opencode loads from the recorder project (referenced from `opencode.json`'s `plugin` array; opencode has no plugin-trust gate). Its only job: write each hook event's payload to `raw/<event>.json`. It taps both the generic `event` hook and the dedicated `tool.execute.after` hook, and exports the plugin under both a named and a default export. It is a recording aid, not the production integration — that remains deferred per Non-Goals.

### Component 3 — dry-run / self-test (offline-verifiable)

`--dry-run` performs everything that needs no auth/network and asserts:

- the generated `.codex/hooks.json` and the opencode plugin registration are structurally valid and reference the per-event log files;
- the sanitizer, given a sample payload containing a home path + a fake secret, produces output with the home rewritten and the secret scrubbed, and is idempotent;
- the emitted fixture directory layout matches the `claude_code/` template (one file per event + README sections present).

This is the surface the harness's tests cover, and the gate I run before claiming the harness works.

### Component 4 — this spec

Serves as the contract for the follow-on "then I build the runner" step: it enumerates the fixture set the runner will require so that step is a small, fixture-grounded change rather than a guess.

## Required fixture set (the contract for the runner build)

A fixture set for an agent is **complete** when `crates/rb-hooks/tests/fixtures/<agent>/` contains:

- one sanitized raw payload per lifecycle event listed in Component 1.1 that actually fired, named **generically by event** — `session_start.json`, `post_tool_use.json` (codex), `tool_execute_after.json` (opencode), etc. — one file per event type holding that event's first occurrence. This deliberately uses generic event names, NOT the tool-specific variant the `claude_code/` seed set happens to carry (`post_tool_use_write.json`); a single representative tool payload per event is sufficient for the runner build, and per-tool variants are out of scope. Absences are documented in the README, as claude_code does for PreCompact;
- `terminus.json` with the multi-turn terminus verdict + the observed event counts;
- `result.jsonl`/`result.json`: the verbatim headless-result output, from which the runner build will derive the judge text and whatever turns/cost/token fields exist;
- a regenerated README with provenance (CLI version, OS, date), the exact recording invocation, the sanitization table, and a "fields present / absent" list mirroring the claude_code README.

## Acceptance Criteria

- `scripts/record-agent-fixtures.sh --dry-run --agent all` passes offline with no auth/network, asserting hook-config validity, sanitization, and fixture layout.
- The harness, run with auth (after a one-time `--setup-trust <agent>`), produces a complete fixture set (per above) for codex and for opencode. **codex**: all writes land in the stable recorder home outside the repo; the real `~/.codex` is never mutated. **opencode** (documented exception): records under the operator's real `~/.config/opencode`, writing one session entry to real opencode history — no auth/config file is copied or modified, and committed fixtures are sanitized.
- The OpenCode logging plugin loads and records payloads in a real `opencode run`.
- No secrets or real home paths appear in committed fixtures.
- Existing Claude Code rb-hooks tests and the scorecard `--self-test` still pass unchanged.
- No capability-matrix status is changed and no scorecard runner code is modified by this work.

## Verification

Offline (what CI / I can run):

```bash
bash scripts/record-agent-fixtures.sh --dry-run --agent all
cargo test -p rb-hooks
scripts/memory-scorecard.sh --self-test
```

Live (what the operator runs locally, with CLI auth installed):

```bash
# One-time per agent: pre-trust the recorder home (codex: interactive TUI hook
# trust; opencode: no trust gate, just prepares the home). Re-run if tokens expire.
bash scripts/record-agent-fixtures.sh --setup-trust codex
bash scripts/record-agent-fixtures.sh --setup-trust opencode

# Then record (no --dangerously flag; trust is pre-persisted in the recorder home):
bash scripts/record-agent-fixtures.sh --agent codex
bash scripts/record-agent-fixtures.sh --agent opencode
# review the diff under crates/rb-hooks/tests/fixtures/{codex,opencode}/ before committing
```

## Risks

- **Headless flag/format differs from assumptions.** The exact `codex exec` / `opencode run` machine-readable flag is discovered at record time, not assumed; the harness logs the invocation it used so the README is accurate. Mitigated by recording verbatim and deferring schema interpretation.
- **Terminus is ambiguous from one run.** Multi-turn count is evidence, not proof; the README states the run shape so a later run can corroborate. Mapping changes stay gated until corroborated.
- **OpenCode plugin API drift.** Pin the recorded OpenCode version in the README; the plugin is minimal to reduce surface.
- **Accidental secret/state leak.** Sanitizer is fail-closed and dry-run-tested; auth is a read-only copy (mode 0600) into the recorder home (mode 0700) outside the repo, which `.gitignore` also covers as defense in depth. Only sanitized per-event payloads + the sanitized `result.jsonl` are written into the committed fixtures dir — never auth/config files.
- **Token expiry in the recorder home.** The copied `auth.json` can drift from the real one as refresh tokens rotate (the recorder copy mutates, the real one is untouched). Benign for isolation; the operator re-runs `--setup-trust <agent>` to re-copy when tokens expire.

## Implementation Checklist

- [ ] `scripts/record-agent-fixtures.sh` skeleton with arg parsing (`--agent`, `--setup-trust`, `--out-dir`, `--dry-run`) and stable recorder-home setup.
- [ ] codex hook-config generation + `codex exec --json` capture path against the recorder `CODEX_HOME`.
- [ ] OpenCode logging plugin + `opencode run` capture path.
- [ ] Pure sanitizer (home rewrite + secret scrub) with dry-run tests.
- [ ] Terminus counter + `terminus.json` emitter.
- [ ] Fixture/README emitter matching the claude_code template.
- [ ] `--dry-run` self-test asserting config validity, sanitization, and layout.
- [ ] Wire dry-run into the rb-hooks / scripts test surface so CI runs it offline.
- [ ] Operator runs live recording for codex + opencode; review + commit fixtures.
