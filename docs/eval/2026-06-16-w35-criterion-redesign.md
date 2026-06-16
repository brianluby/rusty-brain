# W3.5 A/B criterion redesign — from single-fact parity to a memory-value scorecard

Status: **proposed** (decision record; supersedes the gate criterion in
`docs/eval/2026-06-14-w35-ab-eval.md`, not the harness).
Date: 2026-06-16.

## Why

The W3.5 A/B gate (12 single-fact scenarios; pass = memory-on beats a
CLAUDE.md baseline on success-or-turns with zero memory-induced errors) ran
green-path for the first time on 2026-06-16 and **failed**. Investigation
(adversarial code trace + a correctness-judging tool trace) established two
things:

1. **The product is not broken on the axis the gate claims to measure.** With a
   single, freshly-written fact, memory-on ties the CLAUDE.md baseline on
   success and is at parity-or-better on turns once a confirmed double-recall
   bug is set aside. Zero memory-induced errors held throughout, including the
   stale-traps.
2. **The criterion mismeasures memory.** The eval hands the CLAUDE.md baseline
   the very thing memory exists to provide — free, perfect human diligence — and
   then concludes memory adds nothing.

The current scenarios pick **memory's worst case and CLAUDE.md's best case on
every axis**:

| axis      | what the eval gives the CLAUDE.md baseline | what memory actually targets                 |
|-----------|--------------------------------------------|----------------------------------------------|
| count     | one fact                                   | hundreds of decisions across a codebase/history |
| scale     | trivially fits one file                    | a file that bloats context or buries the fact |
| authoring | pre-written, clean                         | nobody wrote it — memory auto-captured it    |
| freshness | already-current (stale-traps hand it the *updated* file) | docs go stale; memory auto-supersedes |
| reach     | one machine, one developer                 | A's decision reaches B on another machine    |

This document records the decisions for a redesigned eval. It does **not** change
the harness yet; the build sequence is in the last section.

## Decisions (ADRs)

### ADR-1: single-fact parity with a freshly-written file is a NON-GOAL

For one current, cleanly-authored fact, a CLAUDE.md file is genuinely as good as
memory. Memory's value is not "beat a perfect one-line file on one fact"; it is
retrieval at scale, automatic capture, freshness under change, and cross-machine
reach. The eval must state this so a tie on single-fact recall is never again
read as a product failure.

### ADR-2: the A/B is a cheap PROXY; the Phase-5 pilot is GROUND TRUTH

This eval runs `claude -p` one-shots against a small model (haiku). "Does shared
memory help a team" is barely observable through that lens. The A/B is a
**regression guard and progress tracker**, not the final arbiter. The
2-user / 2-machine / 2-week pilot (plan §Phase 5) is the ground truth. This
bounds how much to invest in the proxy and what a green/red board licenses.

## Design principles (apply to EVERY scenario)

### P1: dual baselines — never compare against a single baseline

Every scenario runs **four** arms:

- `memory-on` — rusty-brain installed; the fact reaches the work session via the
  store.
- `realistic-baseline` — the CLAUDE.md a real team would actually have for this
  scenario (incomplete / stale / large), reflecting reality, not diligence.
- `steelman-baseline` — a diligent human's CLAUDE.md (clean, current, complete).
- `memory-off` — nothing; the floor.

The claim for a dimension is **"memory-on beats the realistic baseline AND at
least ties the steelman baseline."** A win that only appears against the
realistic (strawman) baseline is **not** a win — it just means we re-rigged the
eval in memory's favor, the mirror image of the bug we are fixing. This rule is
the single most important guardrail in the redesign.

### P2: separate the capture axis from the retrieval axis

Memory's value chain is **capture** (auto-summarize a session → store) **+
retrieval** (recall it next session). They are separable and fail
independently — capture is currently lossy (a summary-pollution regression was
observed). A scenario must declare how the fact enters the store:

- **explicit plant** (`rusty-brain remember`) — bypasses the lossy summary;
  isolates a clean retrieval/freshness signal.
- **auto-capture** (a real SessionEnd fold) — exercises the full loop including
  capture.

**Retrieval/freshness dimensions plant explicitly. Capture is its own dimension,
the only one that uses auto-capture, and it reports a capture-fidelity rate as a
first-class metric.** Mixing the two is what let the last confound hide.

### P3: variance protocol — single runs do not gate

A single run misled this project once already (runs=1 reported a regression that
was partly noise). Therefore:

- N ≥ 5 runs per (scenario, arm); larger for the gate decision.
- Report **median + spread** (IQR or a bootstrap CI), never a bare mean.
- Pre-register pass thresholds **before** running.
- A single-run dispatch is **directional only** and never gates.

### P4: one hard gate, the rest is a scorecard

The value is multi-dimensional; collapsing it into one boolean is what produced
the misleading RED. Structure the output as a **scorecard**: per-dimension
directional targets that track progress, plus exactly one **hard gate** —
**zero memory-induced errors** (the safety property). Everything else informs,
but does not block, a release.

## The scorecard

| dimension              | what it measures                                   | plant mode    | primary metric                          | hard gate? |
|------------------------|----------------------------------------------------|---------------|-----------------------------------------|------------|
| **C. Freshness**       | superseded decision: current vs stale answer       | explicit      | correctness (current answer)            | no (tracked) |
| **A. Retrieval@scale** | one relevant fact among a large corpus             | explicit      | correctness + measured token cost       | no (tracked) |
| **B. Capture**         | a decision that emerges in-session is recalled later | auto-capture | capture-fidelity rate, then end-to-end recall | no (tracked) |
| **R. Reach (team)**    | A captures; B (other identity/machine) recalls     | explicit      | correctness for B                       | no (stretch) |
| **Safety**             | memory never serves a superseded/wrong value as current | (all)     | memory-induced-error count              | **YES**      |

Build order is **C → A → B → R**, justified below. Reach (R) is a stretch but is
the actual product thesis and is feasible via a two-identity / shared-namespace
simulation (the harness already isolates HOME + namespace); it is in the
scorecard so the headline value is not silently dropped.

## Per-dimension notes (decisions + open questions)

### C — Freshness (build first; cheapest, strongest, least confounded)

- Decision X is later superseded by X'. memory-on: both planted explicitly,
  second supersedes the first → recall returns X'. Realistic baseline: a CLAUDE.md
  that still says X (nobody updated the docs). Steelman baseline: a CLAUDE.md
  updated to X'.
- Claim: memory-on returns X' (beats the stale realistic baseline) and ties the
  updated steelman baseline. This is rigging-proof precisely because of the
  steelman arm.
- Also stress-tests the supersede path end to end.

### A — Retrieval at scale (build second; resolve the caching question first)

- Open question that gates the build: **prompt caching.** A large CLAUDE.md is
  loaded once and cached, so its marginal per-turn cost is near-zero, while
  memory re-injects each turn. A naive token comparison can make memory look
  *worse*. Resolve before building: measure **cached vs uncached input tokens**,
  and crank the corpus past comfortable context (500+ facts) so the retrieval
  edge — accuracy on a buried fact — actually bites. Token cost is the secondary
  axis; accuracy-at-scale is primary.

### B — Capture fidelity (build third; expect it to be the hardest bar)

- This is the only auto-capture dimension. It will likely fail today — capture is
  known-lossy — and that is a real product gap, not a criterion artifact.
- Two metrics: (1) **capture-fidelity rate** — after a session where a decision
  emerges, does the stored summary actually contain the fact? Measured directly,
  not through a downstream session. (2) end-to-end recall, only meaningful once
  (1) is high.
- Open question: making "a decision emerges in real work" deterministic enough to
  score (the source of past fragility).

### R — Reach / team (stretch; the actual thesis)

- Two identities (distinct HOME/namespace), a shared store (or hub): identity A
  captures in session 1; identity B recalls in session 2. The realistic baseline
  is that B's checkout never received A's CLAUDE.md edit (not committed/pulled);
  the steelman is that it did.

## Build sequence (this is the execution plan)

0. **This doc** — lock the decisions (no code, no spend). ← current step.
1. **Shared scaffold** (code, no spend): a `plant(mode = explicit | auto-capture)`
   primitive, the four-arm dual-baseline runner, the N-run variance aggregation,
   the scorecard + safety-only hard gate, and a `--self-test` (no API) for the
   judge + aggregation math. Reuse `scripts/w35-ab-eval.sh`'s hermeticity.
2. **Class C** instantiated on the scaffold (no spend until run): explicit plant,
   both baselines, thresholds pre-registered here.
3. **Validate** (first spend, small): `--self-test` → a 1-scenario smoke dispatch
   → the real C run at N ≥ 5. Read the scorecard.
4. **A then B then R** (later, gated on the scaffold proving out in step 3), each
   reusing the scaffold; A only after the caching question is resolved.

The scaffold (step 1), not Class C, is the real first deliverable — C is its
first instance. Building C in isolation would bake a planting-mode and
baseline-pair decision that A, B, and R must inherit, so those decisions are made
here, at the shared layer, with all four dimensions in view.
