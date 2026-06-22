# Decision-grade capture rubric (W3.1)

- **Status:** Active (Phase 3, W3.1 capture inversion)
- **Date:** 2026-06-13
- **Gate clause (§6):** "≥80% of a 50-memory sample per release passes a written
  rubric, human-graded."

## What W3.1 changed

PostToolUse now writes **zero** memories — it appends a redacted observation
(file touched / command run / failure) to a per-session scratch file. **One**
summary memory is written per session at **SessionEnd** (folding the scratch +
transcript + working-tree diff); PreCompact writes at most one decision
snapshot. So the corpus this rubric grades is dominated by *session summaries*
and *decision snapshots*, not per-event noise.

## The bar

A captured memory is **decision-grade** iff a future session, reading only that
memory, could **act on it** — it states a **decision**, a **constraint**, or an
**outcome**, scoped enough to be actionable. It is *not* decision-grade if it is
a bare, contextless event log ("ran a command", "edited a file") that tells a
future session nothing it could not re-derive faster by looking at the repo.

## Grading procedure (per release)

1. Sample **50** memories from a dogfood corpus produced under W3.1 capture
   (prefer `origin_source = hook`, tags `session-summary` / `pre-compact`).
   Sample across sessions, not all from one.
2. Grade each **pass/fail** against the checklist below. One grader; record the
   grader and date alongside the score.
3. **Pass the gate** iff ≥ 40 / 50 (80%) are decision-grade. Record the number,
   the failure taxonomy (below), and a one-line note per failed item.

## Pass checklist (a memory passes if it satisfies ≥1)

- **States a decision:** "Chose cosine distance over L2 because thresholds
  recalibrate in distance units." / "Going with porter over unicode61."
- **States a constraint:** "Namespace is not an auth boundary." / "The engine
  rejects content updates — supersede instead."
- **States an outcome a future session would act on:** "Re-embed pass left 142
  rows stale; rerun until changed == 0." / "cargo test green after the WAL fix;
  the flaky `idle_*` test was a timeout, now injectable."
- **A scoped session summary** whose *Goal* + *Decisions* sections give a future
  session a head start (the bare *Files touched* / *Commands run* sections alone
  do **not** earn a pass — they are supporting detail, not the decision).

## Failure taxonomy (record which on each fail)

- **F-noise** — pure event log, no decision/constraint/outcome (e.g. a summary
  whose only content is a files/commands list).
- **F-stale** — was actionable when written but is now contradicted by the repo
  (acceptable in moderation; flags where supersede/dedup should have fired).
- **F-vague** — gestures at a decision without the specifics to act on it
  ("changed the approach" with no *what* or *why*).
- **F-redaction-scar** — a redaction marker swallowed the actionable token
  (over-redaction); the decision is unrecoverable.

## Notes

- This is **self-grading against a known rubric**, which overfits; §13 requires
  Phase-3 scorecards to also carry external evidence (the W3.5 memory-value
  scorecard, fresh-eyes installs). Treat the 80% bar as necessary, not sufficient.
- The heuristic summary builder (`rb-hooks/src/capture.rs::build_session_summary`)
  leads with **Goal** and **Decisions**; the daemon enricher (heuristic by
  default, LLM when configured) refines the stored `summary` field from that
  content. When the LLM enricher is configured, re-grade — the bar is the same.
