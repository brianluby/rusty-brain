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

`record-agent-fixtures.sh --agent codex|opencode|all [--out-dir DIR] [--dry-run]`

For each agent, operate inside a throwaway `HOME` and temp project (mirroring the scorecard's `seed_home` + `HOME`-override discipline) so no global agent state is mutated — the precise blocker the prior recording attempt hit:

1. **Register logging hooks** that append each event's raw stdin JSON (plus a trailing newline, matching the claude_code recipe) to `raw/<event>.json`:
   - **codex** → project `.codex/hooks.json`, `type: command` hooks for `SessionStart`, `PostToolUse` (matcher `*`), `Stop`, `PreCompact`.
   - **opencode** → the Component 2 plugin, registering `session.created`, `tool.execute.after`, `session.idle`, `session.compacted`, `session.deleted`.
2. **Drive a multi-turn headless session** that performs one Bash command and one file write (so `PostToolUse` / `tool.execute.after` fire), capturing the full machine-readable result stream to `result.jsonl` (or `result.json` if the agent emits a single object):
   - codex → `codex exec "<prompt>"` with the agent's JSON/stream output flag (discovered at record time; the harness logs the exact invocation used).
   - opencode → `opencode run "<prompt>"` likewise.
3. **Terminus determination**: count occurrences of the candidate terminus event (`Stop` / `session.idle`) across the multi-turn run; emit a `terminus.json` note recording the count and the inferred verdict (`true-terminus` if fired once at end, `per-turn` otherwise). This is the evidence that resolves the mapping question the fixture READMEs flag.
4. **Sanitize** before writing committed fixtures: replace the recording user's home dir with `/Users/user`; scrub the same secret classes the hook-capture redaction covers (bearer tokens, `key=value`, AWS-style keys, PEM blocks). The harness is bash, so it reimplements those patterns rather than calling the Rust redactor; the sanitizer is a pure shell function exercised by the dry-run, and a dry-run case pins parity with a known secret of each class.
5. **Emit** per-event fixture files into `crates/rb-hooks/tests/fixtures/<agent>/` and regenerate that agent's README provenance + sanitization + terminus + result-schema sections, replacing the "blocked" placeholder.

### Component 2 — OpenCode logging plugin (`scripts/fixtures/opencode-logger/`)

A minimal standalone JS/TS plugin opencode loads from the throwaway project. Its only job: write each hook event's payload to `raw/<event>.json`. It is a recording aid, not the production integration — that remains deferred per Non-Goals.

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

- one sanitized raw payload per lifecycle event listed in Component 1.1 that actually fired (absences documented in the README, as claude_code does for PreCompact);
- `terminus.json` with the multi-turn terminus verdict + the observed event counts;
- `result.jsonl`/`result.json`: the verbatim headless-result output, from which the runner build will derive the judge text and whatever turns/cost/token fields exist;
- a regenerated README with provenance (CLI version, OS, date), the exact recording invocation, the sanitization table, and a "fields present / absent" list mirroring the claude_code README.

## Acceptance Criteria

- `scripts/record-agent-fixtures.sh --dry-run --agent all` passes offline with no auth/network, asserting hook-config validity, sanitization, and fixture layout.
- The harness, run with auth, produces a complete fixture set (per above) for codex and for opencode without mutating global agent state.
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
bash scripts/record-agent-fixtures.sh --agent codex
bash scripts/record-agent-fixtures.sh --agent opencode
# review the diff under crates/rb-hooks/tests/fixtures/{codex,opencode}/ before committing
```

## Risks

- **Headless flag/format differs from assumptions.** The exact `codex exec` / `opencode run` machine-readable flag is discovered at record time, not assumed; the harness logs the invocation it used so the README is accurate. Mitigated by recording verbatim and deferring schema interpretation.
- **Terminus is ambiguous from one run.** Multi-turn count is evidence, not proof; the README states the run shape so a later run can corroborate. Mapping changes stay gated until corroborated.
- **OpenCode plugin API drift.** Pin the recorded OpenCode version in the README; the plugin is minimal to reduce surface.
- **Accidental secret/state leak.** Sanitizer is fail-closed and dry-run-tested; throwaway `HOME` contains global writes.

## Implementation Checklist

- [ ] `scripts/record-agent-fixtures.sh` skeleton with arg parsing (`--agent`, `--out-dir`, `--dry-run`) and throwaway HOME/project setup.
- [ ] codex hook-config generation + `codex exec` capture path.
- [ ] OpenCode logging plugin + `opencode run` capture path.
- [ ] Pure sanitizer (home rewrite + secret scrub) with dry-run tests.
- [ ] Terminus counter + `terminus.json` emitter.
- [ ] Fixture/README emitter matching the claude_code template.
- [ ] `--dry-run` self-test asserting config validity, sanitization, and layout.
- [ ] Wire dry-run into the rb-hooks / scripts test surface so CI runs it offline.
- [ ] Operator runs live recording for codex + opencode; review + commit fixtures.
