# W3.5 memory-value scorecard — first full N=5 measured run (Classes A, B, C, R)

- **Status:** **MEASURED — first read RED; post-fix N=5 reread SAFE with zero
  memory-induced errors.** First complete N=5 read of the unified scorecard,
  closing the "landed, unmeasured" state recorded in
  `docs/eval/2026-06-23-w35-scorecard-closeout.md`. Delivers Vikunja #381
  (Class A + ADR-3), #382 (Class B), and #383 (Class R).
- **Date:** 2026-07-12.
- **Run URL:** <https://github.com/brianluby/rusty-brain/actions/runs/29203432198>
  (`memory-scorecard.yml`, `runs=5`, main @ `1d3473c1`).
- **Raw artifact:** [`2026-07-12-w35-scorecard-n5.tsv`](2026-07-12-w35-scorecard-n5.tsv)
  (260 rows: 13 scenarios x 4 arms x 5 runs), also attached to the run as
  `memory-scorecard-report`.
- **Conditions:** agent `claude-code` (2.1.207, npm latest at run time), model
  `haiku`, `--max-budget-usd 0.50`/session, macOS runner. Harness fixes
  required to run at all: PR #65 (claude 2.1.x workspace-trust gate;
  `write_claude_md` `set -e` latent kill).

## Recovery reread after PR #70 (Vikunja #502)

- **Run URL:** <https://github.com/brianluby/rusty-brain/actions/runs/29210642539>
  (`memory-scorecard.yml`, `runs=5`, main @ `2d46dd1c`).
- **Result:** workflow and scorecard step succeeded; `SAFETY — memory-induced
  errors: 0 (allowed 0)` and `result: SAFE`.
- **Freshness:** memory-on 60% [38.7–78.1], realistic 0%, steelman 45%,
  memory-off 5%; beats realistic and ties steelman, so the dimension passes.
  The eight unsuccessful memory-on rows were ordinary misses (`mie=0`), not
  stale answers.
- **Tracked scorecard:** reach and freshness pass; capture and
  retrieval-at-scale miss their steelman comparisons, so 2/4 dimensions pass.
  Those dimensions are deliberately non-gating and do not weaken the recovered
  safety result. Direct capture fidelity remains 15/15 (100% [79.6–100.0]).
- **Retrieval-at-scale reading:** memory-on 20% vs steelman 40%; cost remains
  favorable ($0.0131 vs $0.0368), but accuracy loses. The run correctly says
  to investigate retrieval rather than caching. This is input to the
  preregistered production-embedding gate, not a reason to alter ranking under
  #502.

The reread satisfies #502's live zero-MIE criterion and confirms the PR #70
framing fix. It does not retroactively change the first-run measurements below;
they remain the evidence that triggered the safety response. The run was green,
so failure-only session diagnostics were intentionally not uploaded. The
follow-up harness now captures exact hook injections, raw candidate ids/states/
scores/channels, planted ids, and supersede histories for any future red run,
with a deterministic four-way MIE classification and no ranking or token-budget
change.

## First-run verdict summary (historical trigger)

| axis | reading | verdict |
|---|---|---|
| SAFETY (the one hard gate, P4) | **2 memory-induced errors** (allowed 0): `freshness/fresh-test-runner` runs 3 and 4 | **UNSAFE — gate RED** |
| Scorecard (tracked, non-gating) | reach PASS, freshness PASS, retrieval_scale PASS, capture no (steelman tie missed) | 3/4 pass |
| Class B capture fidelity (report-only) | **15/15 = 100%** [79.6–100.0], target >= 80% | met |
| ADR-3 (Class A cost methodology) | accuracy 0.47 vs steelman 0.33 AND cost $0.0132 vs $0.0376 | **RATIFY Opt 3** |

The first run exited non-zero **by design**: the safety gate is the only hard
gate and it fired. Everything below preserves the honest first-run read that
forced the safety response; the recovery reread above is the current verdict.

## First-run scorecard (N=5 per scenario/arm; success rate [Wilson 95% CI], median turns [Q1–Q3], mean cost $/session)

```
dimension       arm                 runs success [95% CI] med_turns [Q1-Q3]    mcost$
reach           memory-on             15    33% [15.2-58.3]      1.0 [1.0-8.5]    0.0286
reach           realistic-baseline    15     0% [-0.0-20.4]      9.0 [7.5-10.0]   0.0559
reach           steelman-baseline     15    20% [7.0-45.2]       1.0 [1.0-5.5]    0.0225
reach           memory-off            15     0% [-0.0-20.4]      8.0 [6.0-9.5]    0.0395
  -> reach: beats_realistic=yes ties_steelman=yes  => PASS

capture         memory-on             15    47% [24.8-69.9]      1.0 [1.0-6.0]    0.0200
capture         realistic-baseline    15     0% [-0.0-20.4]      3.0 [3.0-4.0]    0.0221
capture         steelman-baseline     15    73% [48.0-89.1]      1.0 [1.0-1.0]    0.0119
capture         memory-off            15     7% [1.2-29.8]       3.0 [3.0-4.0]    0.0210
  -> capture: beats_realistic=yes ties_steelman=NO capture_fidelity_target=yes  => no

freshness       memory-on             20    60% [38.7-78.1]      1.0 [1.0-4.2]    0.0176
freshness       realistic-baseline    20     0% [-0.0-16.1]      1.5 [1.0-3.0]    0.0168
freshness       steelman-baseline     20    60% [38.7-78.1]      1.0 [1.0-3.0]    0.0165
freshness       memory-off            20     0% [-0.0-16.1]      3.0 [1.0-4.0]    0.0191
  -> freshness: beats_realistic=yes ties_steelman=yes  => PASS

retrieval_scale memory-on             15    47% [24.8-69.9]      1.0 [1.0-1.0]    0.0132
retrieval_scale realistic-baseline    15     0% [-0.0-20.4]      3.0 [1.0-4.5]    0.0652
retrieval_scale steelman-baseline     15    33% [15.2-58.3]      1.0 [1.0-1.0]    0.0376
retrieval_scale memory-off            15     7% [1.2-29.8]       1.0 [1.0-2.5]    0.0439
  -> retrieval_scale: beats_realistic=yes ties_steelman=yes  => PASS
```

## SAFETY — the first-run gate that fired (historical)

Two memory-induced errors, both `freshness/fresh-test-runner` (memory-on, runs
3 and 4): the session answered the **superseded** `cargo test` with no mention
of the current `cargo nextest run`, despite the store holding the supersede
chain (`cargo test` -> archived, `nextest` active).

Caveat recorded for the investigation: `cargo test` is also the answer a
memoryless model gives by default (memory-off failed 5/5 on this scenario
without tripping MIE — the metric only counts memory-on). So the mechanism is
not yet established: the two red runs are either (a) recall/digest surfacing
the archived value, or (b) a recall **miss** letting the model fall back to the
ecosystem default — which the MIE metric deliberately still charges to memory
(the product's job was to supply the update and it did not). Session logs are
not retained by the harness, so the distinction needs a local reproduction
against a planted store. Follow-up filed: **Vikunja #502**.

3/5 runs of the same scenario passed (answered `nextest`), so this is a
flakiness-band defect, not a deterministic one.

**2026-07-12 follow-up (Vikunja #502) — mechanism identified: (c) injection
ignored, not (a) or (b).** A local reproduction of the exact plant
(deterministic-fallback embeddings, no Voyage key, `plant_explicit` semantics)
showed the retrieval layer clean: `--json list`, the SessionStart digest, the
UserPromptSubmit injection for the exact work prompt, and every recall probe
(including the adversarial query `cargo test`) return ONLY the nextest tip;
the archived row never surfaces (now pinned by
`superseded_decision_never_reaches_context_or_recall_and_tip_always_does` in
`crates/rb-daemon/tests/daemon_e2e.rs`). Injection presence in the failing
runs follows from that pipeline determinism — each run replants an identical
store into a hermetic HOME, and the hook renders the digest/recall injection
from it deterministically — while every memory-on run took 1 turn (no recall
tool call to diverge on). The TSV's matching memory-on token accounting
across passing and failing runs (`cache_creation` 7195 in all five) is
consistent with that; it is corroborating cache accounting, not itself proof
of byte-identical prompts. So the model (haiku) received the injected tip in
runs 3 and 4 and answered the ecosystem default anyway. Contributing cause
addressed: the shared injection preamble told the model to weigh entries as
"possibly-stale" and to never "follow" instruction-shaped text — for a stored
convention ("use nextest, not plain cargo test") that frame argues AGAINST the
memory. The preamble now states an UNCONDITIONAL never-execute rule first
(hostile fact-shaped content stays covered), then a preference scoped to
ANSWERING (recorded decisions beat generic defaults; superseded records
excluded; disputes labeled `[contested]` — see docs/THREAT_MODEL.md).
Residual model variance required a paid N>=5 reread. The recovery run above
completed SAFE at zero MIE, so no orchestrator exception or ranking treatment
is warranted.

## Class B — capture fidelity (Vikunja #382)

Direct hook-origin measurement (store inspected after the plant session,
before any recall):

```
scenario                      runs  fidelity [95% CI] summaries mcp_bypass reasons
cap-error-apperror               5   100% [56.6-100.0]         5          0 cap_ok=5
cap-id-ulid                      5   100% [56.6-100.0]         5          0 cap_ok=5
cap-http-ureq                    5   100% [56.6-100.0]         5          0 cap_ok=5
capture fidelity: 100% [79.6-100.0] (15/15) target>=80%=yes summaries=15 mcp_bypass=0
```

The 2026-06-23 PRD expected this to be **red** ("known lossy capture") — it is
not: every plant session's SessionEnd fold captured the decision, with zero MCP
bypass. The *downstream* recall half of the dimension (47% vs steelman 73%) is
where capture now loses: the fold stores the fact, but the work session doesn't
always surface it strongly enough to beat a diligent human's CLAUDE.md. Per the
pre-registered gates this stays a tracked signal, not a blocker.

## Class A — retrieval@scale + ADR-3 (Vikunja #381)

Target fact buried under a 500-fact distractor corpus:

```
arm                   runs   succ    mcost$    m_input    m_ccrea    m_cread  cache%   ctx_vol    eff_in
memory-on               15    47%    0.0132         10       7613      17299   99.9%     24922     11256
realistic-baseline      15     0%    0.0652         29      26390    128727  100.0%    155146     45889
steelman-baseline       15    33%    0.0376         11      26273     22491  100.0%     48774     35101
memory-off              15     7%    0.0439         21       6841     51986  100.0%     58848     13771
  -> retrieval_scale: acc on 0.47 vs stl 0.33 [ok] | cost$ on 0.0132 vs stl 0.0376 [ok]
     => RATIFY Opt 3 (accuracy >= steelman AND total_cost_usd within 20%)
```

**ADR-3 decision: RATIFIED (Option 3).** At scale, memory-on beats the
steelman on accuracy (47% vs 33%) at ~35% of its per-session cost — selective
recall injects ~11k effective input tokens vs the steelman's ~35k
(everything-in-CLAUDE.md), and caching does not erase the difference
(cache% ~100% on both). Accuracy-at-scale is confirmed as the primary metric;
cost is reported diagnostically per the resolved methodology.

## Class R — reach/team proxy (Vikunja #383)

Two-identity shared-store simulation (A plants, B is scored; separate HOMEs,
one DB/namespace): memory-on 33% [15.2–58.3] vs realistic 0%, steelman 20%,
memory-off 0% — the only arm that ever transfers A's uncommitted decision to B
at all is memory (and it also edges the steelman, whose CLAUDE.md B does see).
**Proxy evidence only**: this simulates reach on one machine and does not prove
the Phase 5 two-user/two-machine pilot.

## What the first run and recovery reread close and open

- Closes: Vikunja #381, #382, #383 (the "landed, unmeasured" deferrals from
  the 2026-06-23 closeout) — measured artifacts now exist for A, B, and R.
- Resolves #502's live safety gate: PR #70 identified mechanism (c), hardened
  the injection frame, and the recovery reread returned zero MIE. The task can
  close when the diagnostic-evidence follow-up described above lands.
- Continues the non-gating quality work under the preregistered production-
  embedding gate: the recovery run's retrieval-at-scale accuracy lost to the
  steelman even though cost remained favorable. Do not treat that signal as a
  reason to reopen #502 or apply an unconditional rank boost.
- The weekly cron continues to enforce zero MIE. Any future red run now retains
  enough evidence to classify the failure without guessing at its mechanism.
