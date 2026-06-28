# Next-Work Sequencing & Parallelization

- **Date:** 2026-06-23
- **Scope:** ordering and parallelization for the work after the W3.5 scorecard track and the cross-agent capture/parity track.
- **Related PRDs:** [W3.5 scorecard closeout](2026-06-23-w35-scorecard-closeout.md), [cross-agentic agent parity](2026-06-23-cross-agentic-agent-parity.md), [cross-CLI capture inversion](2026-06-23-cross-cli-capture-inversion.md), [Codex apply_patch capture](2026-06-23-codex-apply-patch-capture.md), [road-to-tens roadmap](../plans/2026-06-11-rusty-brain-road-to-tens.md).

## Likely next work (ordered)

1. **Finish W3.5 closeout first.**
   - Run or dispatch the full scorecard if A/B/R should be measured.
   - Otherwise explicitly mark A/B/R as landed-but-unmeasured or deferred.
   - Create the closeout artifact and update docs/board/changelog.
   - This lets us say W3.5 is closed without overstating it.

2. **Then do cross-agent capture/parity.**
   - Record real lifecycle fixtures for Codex/OpenCode. (Gemini descoped 2026-06-27.)
   - Decide safe `SessionEnd`/`SessionCheckpoint` mappings.
   - Update the capability matrix and docs for Codex/OpenCode/Hermes (the matrix and Hermes discovery note already landed in PR #43) based on the recorded fixture evidence.
   - Only then expand scorecard targeting by agent.

3. **After that, move to Phase 4 / validation hardening.**
   - Per road-to-tens, the next broad phase is proving and hardening: eval gates, perf gates, fuzzing, docs-truth audit, and failure drills.
   - This is where W3.5 becomes one input into a wider release-quality gate.

4. **Then Phase 5 team mode.**
   - Shared/team memory substrate, hub/promote flow, trust controls, pilot users.
   - The current scorecard is proxy evidence; Phase 5 pilot evidence is the real product proof.

**Immediate next concrete thing:** start the W3.5 closeout worktree and decide whether to spend API on one full measured scorecard run or close with A/B/R explicitly marked as landed-but-unmeasured.

## Parallelization

Yes, work can overlap — but keep the branches separate.

### Safe to parallelize

- **W3.5 closeout** can run in parallel with **cross-agent capture/parity**. They touch different concerns. W3.5 should only reference cross-agent work as a follow-up, not depend on it.
- **Within W3.5**, one person can gather scorecard evidence while another drafts the closeout doc — as long as the doc waits for final artifact paths / run URLs before claiming measured status.
- **Within cross-agent**, fixture recording can be parallelized by CLI:
  - Codex lifecycle fixture
  - OpenCode lifecycle fixture
  - ~~Gemini lifecycle fixture~~ (descoped 2026-06-27)

  Each should produce its own sanitized fixture notes before shared code mappings are changed.

### Do sequentially

- In W3.5, do not finalize `docs/eval/2026-06-23-w35-scorecard-closeout.md` until the measured/deferred status for A/B/R is decided.
- In cross-agent, do not change adapter mappings until fixture evidence is reviewed. Fixture collection comes first; mapping/code changes come second.
- Do not implement Codex `apply_patch` capture until a real current Codex fixture proves `apply_patch` emits a usable `PostToolUse` payload.

### Practical split

- **Worker A:** W3.5 closeout branch.
- **Worker B:** Codex/OpenCode lifecycle fixtures (Gemini descoped 2026-06-27).
- **Worker C** (after B has evidence): mapping / capability-matrix / agent-scorecard work.

The main merge dependency is low: W3.5 closeout can merge first, and cross-agent parity can continue independently.
