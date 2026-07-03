# Cross-CLI Terminus → SessionCheckpoint Capture-Recovery Mapping

- **Date:** 2026-06-26
- **Scope:** Resolve the post-W3.1 capture regression for the non-Claude CLIs
  (Codex / OpenCode; Gemini descoped 2026-06-27) by mapping each CLI's terminal
  native event onto a canonical event that actually folds the session scratch
  into a memory.
- **Status:** OpenCode capture **restored** (2026-07-02): Gap A (`session.idle` →
  `SessionCheckpoint`) and Gap B (`apply_patch` → `Edit` + V4A `patchText` path
  extraction in `edited_path`) both landed, with fixture-backed lifecycle tests;
  the `opencode` capability row graduated capture `Partial` → `Supported`.
  Scorecard enablement is still a separate, larger task than a flag flip — do NOT
  flip it from the capture work alone. Codex's decision is fixture-gated
  (decision tree below). **Gemini is descoped** (2026-06-27) — removed from
  active scope; it stays on canonical `Stop`. This is "Worker C" per the
  sequencing PRD.
- **Related:**
  [cross-cli-capture-inversion follow-up](../follow-ups/2026-06-13-cross-cli-capture-inversion.md),
  [cross-agent fixture-recording spec](../specs/2026-06-23-cross-agent-fixture-recording.md),
  [next-work sequencing](../prds/2026-06-23-next-work-sequencing.md),
  [codex apply_patch follow-up](../follow-ups/2026-06-02-codex-apply-patch-capture.md).

## Problem (recap)

W3.1 inverted capture: canonical `Stop` now stores **nothing**; the
once-per-session fold happens at canonical `SessionEnd`. Only the **Claude Code**
adapter emits canonical `SessionEnd` (fixture-verified). The other three adapters
map their terminal native event onto canonical `Stop`:

| CLI | terminal native event | current canonical mapping | adapter |
|---|---|---|---|
| Gemini | `SessionEnd` | `Stop` | `crates/rb-agents/src/gemini.rs:68` |
| Codex | `Stop` | `Stop` | `crates/rb-agents/src/codex.rs` |
| OpenCode | `session.idle` | `Stop` | `crates/rb-agents/src/opencode.rs:92` |

Net effect vs pre-W3.1 for those three CLIs: `PostToolUse` still appends to the
per-session scratch, but no canonical fold event ever fires, so the scratch is
never folded into a summary — it ages out via `scratch::prune_stale` (24h).
They went from per-event capture to **no new capture**.

## Key finding: the recovery mechanism already exists and is dormant

The fix is **not** new machinery. The `SessionCheckpoint` canonical event and the
`FoldMode::Checkpoint` path are already built, wired, and unit-tested — but **no
adapter emits `SessionCheckpoint`**, so the path is dead code in practice:

- `rb_agents::HookEvent::SessionCheckpoint { reason }` — `crates/rb-agents/src/event.rs:42`
  (with `session_checkpoint_is_distinct_from_session_end_and_stop`).
- Dispatch routes it: `crates/rb-hooks/src/dispatch.rs:37` → `capture::session_checkpoint(...)`.
- Fold semantics: `capture::fold_session_summary(.., FoldMode::Checkpoint)` folds
  the scratch into ONE summary and **supersedes** any prior summary for the
  session, but **retains** the scratch buffer (`scratch.mark_checkpointed(id)`),
  so a later checkpoint re-folds early+late turns into a new superseding summary.
  Covered by `scratch.rs` tests `mark_checkpointed_*`.
- `event_needs_daemon` already includes `SessionCheckpoint` — `crates/rb-hooks/src/main.rs:212`.

So `SessionCheckpoint` is exactly the "terminus-less CLI" fallback the
capture-inversion follow-up asked for ("an idle-debounced fold"). The only work
is to point each non-Claude terminus at it (or at `SessionEnd` where a CLI proves
a clean once-per-session terminus).

## Mapping decision framework

The rule, stated once: **per CLI, exactly ONE native event carries the session
fold (the "fold event"); every other lifecycle event maps to canonical `Stop`
(stores nothing).** Mapping more than one event to a fold would double-fold the
same session. So the decision is two steps:

**Step 1 — choose the single fold event.** Prefer a clean once-per-session
terminus if the CLI has one; otherwise use the most-terminal multi-fire signal
(a per-turn stop or an idle boundary).
The recorded lifecycle fixture answers which case applies:

> Does the chosen event fire **exactly once per session**, or **once per turn**
> (i.e. multiple times per session)?

**Step 2 — map the fold event by its cadence:**

- **Fires once per session (clean terminus):** → canonical `SessionEnd`.
  Fold + clear scratch. One summary per session, matching Claude.
- **Fires multiple times per session (no clean terminus — e.g. a per-turn stop
  or an idle boundary):** → canonical `SessionCheckpoint`. Each firing folds the
  accumulated scratch into one superseding summary and retains the buffer.
  Converges to a complete, single live summary; never multiplies memories
  (supersede keeps exactly one).

**Everything else → `Stop`.** Every lifecycle event that is NOT the chosen fold
event maps to canonical `Stop` and stores nothing. This is the rule that
prevents double-folding: when a CLI exposes both a multi-fire signal and a
clean per-session terminus, the terminus is the fold event (Step 2) and the
multi-fire signal stays `Stop`; when a CLI exposes only multi-fire signals
(per-turn stops or idle boundaries), exactly one of them is promoted to the
`SessionCheckpoint` fold event and the rest stay `Stop`.

**No fold event at all** (no usable terminal or per-turn signal): accept the
gap, document per-CLI, rely on `prune_stale`. Not expected for any current CLI.

## Per-CLI decisions

### OpenCode — terminus mapping decidable now; full capture restoration is NOT

OpenCode has **two independent gaps**; the terminus mapping fixes only the first.
Do not conflate them, and do not flip OpenCode's capability status until BOTH are
closed.

**Gap A — fold event (terminus mapping).** Recorded evidence (PR #46,
`crates/rb-hooks/tests/fixtures/opencode/terminus.json`):
`{"verdict":"ambiguous","fired":2,"turns":14}` — `session.idle` fired **twice** in
a single non-interactive `opencode run`. Note the recorder's verdict is literally
`ambiguous`, and the fixture README calls this "evidence, not proof"; it is a
ONE-run observation, not a verified count.

Crucially, the checkpoint decision does **not** require a clean-terminus proof —
that is the point. `SessionCheckpoint` is **multi-fire-safe**: for ANY fire count
≥ 1, each firing folds the accumulated scratch into one *superseding* summary, so
the exact cadence (1, 2, or N) does not change correctness. The "ambiguous"
verdict resolves *against* a clean once-per-session terminus, and checkpoint is
precisely the mapping that is robust to that ambiguity. So:

**Decision (acceptance criterion: "multi-fire checkpoint-safe", not "clean
terminus"):** map `session.idle` → canonical **`SessionCheckpoint`** (today `Stop`,
`opencode.rs:92`). This restores the *fold* — captured observations actually
become a memory instead of ageing out. OpenCode exposes no cleaner terminus
(`session.deleted` is not emitted on a normal headless run).

**Gap B — tool coverage.** [LANDED 2026-07-02] `normalize_tool` now maps
`apply_patch` → `Edit`, and `edited_path` parses the V4A `patchText`
`*** Add|Update|Delete File: <path>` directive; a fixture-backed lifecycle test
(`opencode_apply_patch_file_edit_folds_into_checkpoint_summary`) replays the real
`result.jsonl:10` payload. With Gap A and Gap B both landed, OpenCode capture is
restored and the `opencode` capability row graduated `Partial → Supported`.

> Historical pre-fix rationale (retained for the decision record; superseded by
> the LANDED note above):
>
> The terminus mapping only folds what the scratch already contains, and OpenCode's
> file edits are NOT being captured into the scratch. The recorded run created
> `notes.txt` via the **`apply_patch`** tool
> (`crates/rb-hooks/tests/fixtures/opencode/result.jsonl:10`,
> `"tool":"apply_patch"` with a `patchText`), but `normalize_tool`
> (`crates/rb-hooks/src/capture.rs:127`) has no `apply_patch` arm — it falls
> through to `""` (not captured). The committed `tool_execute_after.json` fixture
> captured the **`bash`** event, not the edit, so a lifecycle test that replays
> only that fixture would pass while every OpenCode file edit silently drops on
> the floor.
>
> This is the same `apply_patch` family as the deferred Codex gap, but it
> manifests **live** for OpenCode+gpt-5.5 (Codex's is upstream-blocked from even
> firing). The existing `"patch" => "Edit"` arm does NOT cover it — the recorded
> tool name is `apply_patch`, and the payload carries `patchText` (a raw patch),
> not a `file_path`, so a bare arm would summarize as "Edited unknown".
>
> **Therefore "OpenCode capture is restored" was FALSE until BOTH landed:**
> 1. `session.idle` → `SessionCheckpoint` (Gap A), AND
> 2. an `apply_patch` arm in `normalize_tool` + edited-path extraction from
>    `patchText`, proven by a real OpenCode `apply_patch` `tool.execute.after`
>    fixture.
>
> Gap A is the worked example the Codex terminus decision follows (Gemini
> descoped). Gap B is tracked alongside the Codex `apply_patch` follow-up.

### Codex — FIXTURE-GATED

Codex native events: `SessionStart`, `PostToolUse`, `Stop`, `PreCompact`
(`crates/rb-agents/src/codex.rs`). Codex exposes **no** `SessionEnd` native event,
and `stop_hook_active` is hardcoded `false` (`codex.rs:67`).

**Open question the fixture must answer:** does Codex `Stop` fire once per session
or once per turn? Record via `bash scripts/record-agent-fixtures.sh --agent codex`
(needs the one-time interactive `--setup-trust codex` first) and inspect
`terminus.json` (`fired` vs `turns`).

**Decision tree:**
- `Stop` fires **once per session** → map Codex `Stop` → `SessionEnd`.
- `Stop` fires **per turn** → map Codex `Stop` → `SessionCheckpoint` (same as
  OpenCode). This is the more likely outcome given Codex's event model.

Unrelated and still deferred: Codex `apply_patch` capture remains blocked upstream
(openai/codex#16732); see the
[apply_patch follow-up](../follow-ups/2026-06-02-codex-apply-patch-capture.md).
Codex shell capture already works (`tool_name: "Bash"`).

### Gemini — DESCOPED (2026-06-27)

Gemini is removed from the active cross-CLI terminus/capture work. Its adapter
(`crates/rb-agents/src/gemini.rs`) stays as-is: native `SessionEnd` continues to
map to canonical `Stop` (`gemini.rs:68`), so Gemini neither folds nor
double-folds — its capture stays `Partial`, no worse than today. No Gemini
lifecycle fixture will be recorded and no mapping change will be made under this
plan.

If Gemini is reprioritized later, the work is unchanged from the Codex template:
record a lifecycle fixture, inspect `terminus.json`, and map its `SessionEnd`
once-per-session → canonical `SessionEnd` / per-turn → `SessionCheckpoint`.

## Scorecard targeting — a SEPARATE, larger task (do not couple to terminus mapping)

Terminus mapping and scorecard enablement are **independent**. Landing the
terminus mapping does NOT make an agent scorecard-able, and flipping
`scorecard_agent_supported()` after terminus mapping alone would be a **false
success path**.

The reason: `scripts/memory-scorecard.sh` is Claude-specific end to end, not just
in its `scorecard_agent_supported` gate (line ~50). Its `run_session` hardcodes
`claude -p "$prompt" --setting-sources project --model … --permission-mode
acceptEdits --allowedTools …` (line ~895), and `install_claude_hooks` (line ~910)
installs Claude's hook config into a seeded Claude home. There is no `--agent`
indirection in the runner — enabling OpenCode/Codex means writing agent-specific
**execution** (the headless CLI invocation per agent), **config/install** (each
CLI's hook registration), and the seeded-home shape, then verifying the captured
memories flow end-to-end.

So scorecard enablement is its own task, gated on (in addition to the terminus +
tool-coverage work below):

1. Real lifecycle fixtures committed under `crates/rb-hooks/tests/fixtures/<cli>/`
   (SessionStart, the captured tool events INCLUDING file edits, and the terminus
   event).
2. A green lifecycle test proving the terminus folds exactly one summary AND that
   file-edit capture works (see TDD tasks — for OpenCode this requires the
   `apply_patch` arm, Gap B).
3. The adapter terminus mapping decided + landed (this doc).
4. Agent-specific `run_session` execution + hook install in `memory-scorecard.sh`
   (the real work this finding surfaces — NOT a flag flip).
5. `capability.rs` row updated (capture `Partial`→`Supported` only once 1–3 hold;
   `scorecard: Unsupported→Supported` only once 4 holds; do not overstate before).

Until 4 exists, keep each non-Claude agent's `scorecard_skip_detail` clause —
update its wording to point at this plan rather than removing it.

## Capability matrix transitions (`crates/rb-agents/src/capability.rs`)

Per agent, on completing its terminus mapping + lifecycle test:
- `capture: Partial → Supported`
- `verified_lifecycle_source`: point at the committed fixtures (not the README).
- Update the limitation string (e.g. OpenCode: "session.idle remains canonical
  Stop …" → "session.idle maps to SessionCheckpoint (multi-fire checkpoint-safe;
  one-run observation fired=2, verdict ambiguous)"), and only drop the
  capture-gap caveat once the `apply_patch` arm (Gap B) lands.
- `scorecard: Unsupported → Supported` only after the scorecard criteria above.

Do NOT change these statuses until the fixture + test land — the capability tests
(`capability.rs` tests) assert claude-code is the only `scorecard: Supported`
agent and non-Claude agents are `capture: Partial`. Those tests update in lockstep
with each agent's landing.

## TDD task breakdown (Worker C)

OpenCode terminus mapping — Gap A (decidable now):
1. Generalize the lifecycle harness helper
   `observe_sequence_against_mock_daemon` (`crates/rb-hooks/tests/integration.rs:207`)
   to take an `agent` param (today hardcoded `--agent claude-code`, line 238).
2. RED: add an opencode lifecycle test replaying
   `[tool_execute_after.json, session_idle.json]` (both share
   `sessionID ses_107a97…`, so the scratch aligns) and asserting the terminus
   folds ONE summary Remember. Fails today (`session.idle` → `Stop` → nothing).
   NOTE: the committed `tool_execute_after.json` is a `bash` event, so this test
   proves only that bash-command capture folds — it does NOT prove file-edit
   capture (that is Gap B). Do not let a green here read as "capture restored".
3. GREEN: remap `session.idle` → `SessionCheckpoint` in `opencode.rs`.
4. Add a second-idle case asserting the second checkpoint **supersedes** the
   first (one live summary, not two).

OpenCode tool coverage — Gap B (blocks "capture restored"):
5. Record a real OpenCode `apply_patch` `tool.execute.after` fixture (the
   committed set lacks one; `result.jsonl:10` shows the tool name is
   `apply_patch` with a `patchText` payload).
6. RED: lifecycle test replaying the `apply_patch` event asserts the edited path
   is captured into the scratch. Fails today (`normalize_tool` drops
   `apply_patch`).
7. GREEN: add an `apply_patch` arm to `normalize_tool` + edited-path extraction
   from `patchText` in `summarize_post_tool_use` (`capture.rs`). Mirror the
   guidance in the Codex `apply_patch` follow-up.
8. Update `capability.rs` opencode row + limitation string — only after Gaps A
   AND B both land.

OpenCode scorecard — separate task (see "Scorecard targeting" above):
9. Add agent-specific `run_session` execution + hook install for OpenCode in
   `memory-scorecard.sh`, then flip `scorecard_agent_supported`. This is the bulk
   of the work and is NOT unlocked by terminus mapping alone.

Codex: record fixture → inspect `terminus.json` → apply the terminus decision
tree (Gap A analog) → assess its file-edit tool coverage (Gap B analog) → then
scorecard. Codex needs the operator's one-time `--setup-trust codex` before
recording; Codex `apply_patch` stays upstream-blocked (openai/codex#16732).
(Gemini is descoped — see the Gemini section.)

## Risks / open questions

- **Helper coupling:** the lifecycle harness is Claude-only (hardcoded agent +
  fixture session-id alignment). Generalizing it is a prerequisite for every
  non-Claude lifecycle test; do it once, cleanly.
- **Double-fold safety:** if a CLI has BOTH a per-turn and a per-session signal,
  only the per-session one maps to a fold event; the per-turn one stays `Stop`.
  No current CLI is known to expose both, but verify per fixture.
- **OpenCode `session.deleted`:** not emitted on a normal headless run, so it
  stays `Other`. If a future fixture shows it as a reliable terminus, prefer it
  (`SessionEnd`, clean) over the `session.idle` checkpoint policy.
- **Checkpoint cost:** each checkpoint is a daemon round-trip + store. For a
  many-idle session this is more writes than a single SessionEnd, but supersede
  keeps exactly one live memory. Acceptable; revisit only if a CLI fires the
  terminus pathologically often.
