# Follow-up: Extend the W3.1 capture inversion to non-Claude CLIs

- **Date:** 2026-06-13
- **Area:** `rb-agents` adapters (Gemini / Codex / OpenCode) + `rb-hooks` capture
- **Status:** Resolution planned — see
  [cross-CLI terminus mapping plan](../plans/2026-06-26-cross-cli-terminus-mapping.md).
  The recovery mechanism (`SessionCheckpoint` + `FoldMode::Checkpoint`) already
  exists and is wired/tested but dormant; OpenCode's terminus mapping is
  fixture-backed (Gap A), but full capture restoration still needs the separate
  `apply_patch` tool-coverage fix (Gap B). Codex/Gemini remain fixture-gated.
- **Severity:** Medium — a capture **regression** for non-Claude CLIs until resolved

## Summary

W3.1 inverted capture: canonical `Stop` now stores **nothing**, and the
once-per-session fold happens at canonical `SessionEnd`. Only the **Claude Code**
adapter emits canonical `SessionEnd` (verified against recorded fixtures). The
other three adapters map their terminal event onto canonical `Stop`:

| CLI | terminal native event | canonical mapping |
|---|---|---|
| Gemini | `SessionEnd` | `Stop` (`crates/rb-agents/src/gemini.rs`) |
| Codex | `Stop` | `Stop` (`crates/rb-agents/src/codex.rs`) |
| OpenCode | `session.idle` | `Stop` (`crates/rb-agents/src/opencode.rs`) |

So for Gemini / Codex / OpenCode: `PostToolUse` appends to the per-session
scratch as designed, but no canonical `SessionEnd` ever fires, so the scratch is
never folded into a summary — it simply ages out via `scratch::prune_stale`
(24h). Net effect vs pre-W3.1: those CLIs go from per-event capture to **no new
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
