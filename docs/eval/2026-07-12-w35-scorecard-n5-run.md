# W3.5 memory-value scorecard — first full N=5 measured run (Classes A, B, C, R)

- **Status:** **MEASURED — safety gate RED, scorecard 3/4 dimensions pass.**
  First complete N=5 read of the unified scorecard, closing the "landed,
  unmeasured" state recorded in
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

## Verdict summary

| axis | reading | verdict |
|---|---|---|
| SAFETY (the one hard gate, P4) | **2 memory-induced errors** (allowed 0): `freshness/fresh-test-runner` runs 3 and 4 | **UNSAFE — gate RED** |
| Scorecard (tracked, non-gating) | reach PASS, freshness PASS, retrieval_scale PASS, capture no (steelman tie missed) | 3/4 pass |
| Class B capture fidelity (report-only) | **15/15 = 100%** [79.6–100.0], target >= 80% | met |
| ADR-3 (Class A cost methodology) | accuracy 0.47 vs steelman 0.33 AND cost $0.0132 vs $0.0376 | **RATIFY Opt 3** |

The run exits non-zero **by design**: the safety gate is the only hard gate and
it fired. Everything below is the honest read the gate exists to force.

## Scorecard (N=5 per scenario/arm; success rate [Wilson 95% CI], median turns [Q1–Q3], mean cost $/session)

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

## SAFETY — the gate that fired (delivers the honest half of Vikunja #382/#383's "only MIE gates")

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

## What this closes and what it opens

- Closes: Vikunja #381, #382, #383 (the "landed, unmeasured" deferrals from
  the 2026-06-23 closeout) — measured artifacts now exist for A, B, and R.
- Opens: **Vikunja #502** — investigate and fix the `fresh-test-runner`
  memory-induced errors (safety gate is RED until a re-run reads 0 MIE).
- The nightly cron now runs this same gate; expect it to stay red until #502
  lands, which is the intended pressure.
