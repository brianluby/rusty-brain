# Follow-up: Extend the W3.1 capture inversion to non-Claude CLIs

- **Date:** 2026-06-13
- **Area:** `rb-agents` adapters (Gemini / Codex / OpenCode) + `rb-hooks` capture
- **Status:** OpenCode capture **restored** — both gaps landed. The recovery
  mechanism (`SessionCheckpoint` + `FoldMode::Checkpoint`) already existed but
  was dormant; OpenCode's terminus mapping (Gap A) landed first (`session.idle`
  → `SessionCheckpoint`, fixture-backed lifecycle test), and the `apply_patch`
  tool-coverage fix (Gap B) followed: `normalize_tool` maps `apply_patch` →
  `Edit` and `edited_path` parses the V4A `patchText` `*** <op> File: <path>`
  directive. The `opencode` capability row graduated capture `Partial` →
  `Supported`. Codex remains fixture-gated; Gemini is descoped (2026-06-27).
  Scorecard enablement for OpenCode is still a separate, larger task.
- **Severity:** Medium — a capture **regression** for non-Claude CLIs until resolved

## Summary

W3.1 inverted capture: canonical `Stop` now stores **nothing**, and the
once-per-session fold happens at canonical `SessionEnd`. Only the **Claude Code**
adapter emits canonical `SessionEnd` (verified against recorded fixtures). Of the
other three adapters, Gemini and Codex still map their terminal event onto
canonical `Stop`, while OpenCode now maps `session.idle` to canonical
`SessionCheckpoint` (Gap A):

| CLI | terminal native event | canonical mapping |
|---|---|---|
| Gemini | `SessionEnd` | `Stop` (`crates/rb-agents/src/gemini.rs`) — descoped |
| Codex | `Stop` | `Stop` (`crates/rb-agents/src/codex.rs`) — fixture-gated |
| OpenCode | `session.idle` | `SessionCheckpoint` (`crates/rb-agents/src/opencode.rs`) — **Gap A landed** |

OpenCode's `session.idle` now folds via canonical `SessionCheckpoint` (Gap A,
multi-fire checkpoint-safe), so its bash-tool scratch is captured again. Codex
remains on `Stop` until a recorded lifecycle fixture proves its terminus cadence.
Gemini is **descoped** (2026-06-27): it stays on `Stop`, no worse than today, and
no fixture/mapping work is planned for it under this track. For any CLI still on `Stop`: `PostToolUse` appends to the
per-session scratch as designed, but no canonical fold ever fires, so the scratch
is never folded into a summary — it simply ages out via `scratch::prune_stale`
(24h). Net effect vs pre-W3.1 (while unmapped): per-event capture → **no new
capture** (existing memories still recall/inject fine).

## Why it was not fixed in W3.1

- Phase 3's stated goal is **Claude Code value**; the other adapters' terminal
  cadence (per-turn vs per-session) is not fixture-verified the way Claude's
  `Stop` (per-turn) vs `SessionEnd` (per-session) split is.
- Folding on canonical `Stop` is **unsafe** for Claude: Claude's `Stop` fires
  once per turn, so folding there would write many summaries per session and
  break the ≤5-memories gate.
- Mapping e.g. Gemini's `SessionEnd` onto canonical `SessionEnd` blindly risks
  over-capture if that event is actually per-turn — the adapter comment calls it
  "the stop event," which is ambiguous.

## What "done" looks like

For each non-Claude adapter, determine (from upstream docs / recorded fixtures)
which native event is the true **session terminus** vs the **per-turn stop**, then:

- map the session terminus to canonical `SessionEnd` (it gets the fold), and
- map the per-turn stop to canonical `Stop` (stores nothing),

mirroring the Claude split. If a CLI has only a per-turn signal and no session
terminus, decide a policy (e.g. an idle-debounced fold, or accept the gap and
document it per-CLI). Add recorded-payload fixtures under
`rb-hooks/tests/fixtures/<cli>/` and a lifecycle test per the W3.4 harness.

## Related

- `crates/rb-hooks/src/capture.rs` — the `stop` doc comment points here.
- `docs/plans/2026-06-11-rusty-brain-road-to-tens.md` §6 — W3.1 progress note
  records the same cross-CLI caveat.
