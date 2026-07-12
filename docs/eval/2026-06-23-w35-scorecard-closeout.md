# W3.5 memory-value scorecard closeout

- **Date:** 2026-06-23.
- **Current branch:** `codex/w35-scorecard-closeout`.
- **Current commit:** `19a3a6692631b84d9444d6f11e353512244fe655`.
- **Scenario file:** `crates/rb-eval/scorecard/memory_scorecard_scenarios.json`.
- **Scenario count:** 13 total: 4 freshness, 3 retrieval_scale, 3 capture, 3 reach.
- **Variance config:** `min_runs=5`, `runs_per_scenario=5`, `tie_margin=0.10`.

This closes W3.5 as a scorecard closeout, not as proof that every dimension has
passed. The only live, raw, gating-grade scorecard evidence currently recorded is
Class C / Freshness. Classes A, B, and R have landed in the unified scorecard
harness and scenario file, but their live N>=5 measured reads are intentionally
deferred because this environment has no `ANTHROPIC_API_KEY` and no new scorecard
spend was authorized for this closeout.

This is proxy scorecard evidence from `claude -p` one-shots. It is not Phase 5
two-user / two-machine pilot proof.

## Commands run for closeout

```bash
scripts/memory-scorecard.sh --self-test
jq empty crates/rb-eval/scorecard/memory_scorecard_scenarios.json
```

Both commands passed locally on `codex/w35-scorecard-closeout` at
`19a3a6692631b84d9444d6f11e353512244fe655`.

No new live scorecard run was created in this closeout. Local `claude` exists,
but `ANTHROPIC_API_KEY` is unset; A/B/R measurement therefore remains a tracked
follow-up instead of being inferred from landed code.

## Recorded measured artifact

| field | value |
|---|---|
| Dimension measured | C. Freshness |
| Workflow | `.github/workflows/memory-scorecard.yml` |
| Run URL | https://github.com/brianluby/rusty-brain/actions/runs/27917957530 |
| Source commit | `b601fea` on branch `feat/validate-class-c-freshness` |
| Raw TSV | `docs/eval/2026-06-21-w35-class-c-freshness-n10.tsv` |
| Writeup | `docs/eval/2026-06-21-w35-class-c-freshness-measured.md` |
| Scope | 4 freshness scenarios, 4 arms, N=10 runs per scenario/arm |
| Sessions | 160 total; 40 memory-on observations |
| Safety | 0 memory-induced errors; result `SAFE` |

Measured result:

```text
freshness memory-on          100% [91.2-100.0]
freshness realistic-baseline   0% [0.0-8.8]
freshness steelman-baseline  100% [91.2-100.0]
freshness memory-off          25% [14.2-40.2]
-> freshness: beats_realistic=yes ties_steelman=yes => PASS
SAFETY - memory-induced errors: 0 (allowed 0)
result: SAFE
```

## Dimension status

State values are intentionally restricted to `measured`, `landed, unmeasured`,
`intentionally deferred`, and `not landed`.

| dimension | state | evidence | closeout reading |
|---|---|---|---|
| C. Freshness | measured | Run `27917957530`; raw TSV `docs/eval/2026-06-21-w35-class-c-freshness-n10.tsv`; writeup `docs/eval/2026-06-21-w35-class-c-freshness-measured.md` | Green. Memory-on beat realistic stale docs, tied the steelman, and produced 0 memory-induced errors over the Class C measured run. |
| A. Retrieval@scale | landed, unmeasured | `0d93195`; scenarios `scale-http-buried`, `scale-id-type-buried`, `scale-wire-format-buried`; ADR-3 cost/cache reporting in `scripts/memory-scorecard.sh`; self-test coverage | Harness and scenarios landed. No raw N>=5 artifact or run URL exists, so A is not claimed measured or green. |
| B. Capture fidelity | landed, unmeasured | PR #40 merge `ec3e857`; scenarios `cap-http-ureq`, `cap-error-apperror`, `cap-id-ulid`; PRD `docs/prds/2026-06-23-w35-class-b-capture-fidelity.md`; self-test coverage | Harness and scenarios landed. No raw N>=5 artifact or run URL exists, so B is not claimed measured. A future red B read is a tracked capture-quality signal, not a release blocker unless it creates memory-induced errors. |
| R. Reach/team | landed, unmeasured | PR #40 merge `ec3e857`; scenarios `reach-plugin-manifest-path`, `reach-socket-serializer`, `reach-daemon-e2e-command`; PRD `docs/prds/2026-06-23-w35-class-r-reach-team.md`; self-test coverage | Harness and scenarios landed. No raw N>=5 artifact or run URL exists, so R is not claimed measured and does not prove Phase 5 team adoption. |
| Safety | measured | Class C run `27917957530` and raw TSV; closeout self-test | The recorded Class C measured run is `SAFE` with 0 memory-induced errors. Full-scorecard safety across A/B/R is not measured until those dimensions get raw runs. |

## Deferrals and follow-up ownership

| follow-up | owner | reason | required artifact |
|---|---|---|---|
| Vikunja `#13`: measure Class A retrieval@scale at N>=5 and finalize ADR-3 from live cost/cache data | Evaluation owner / W3.5 board parent `#2` | Code and scenarios landed, but live `total_cost_usd` and cache buckets require API key + spend | GitHub Actions run URL or bounded TSV under `docs/eval/`, plus any ADR-3 decision update |
| Vikunja `#14`: measure Class B capture fidelity at N>=5 | Evaluation owner / W3.5 board parent `#2` | Code and scenarios landed, but direct hook-origin capture fidelity needs real Claude Code plant sessions | GitHub Actions run URL or bounded TSV under `docs/eval/`, plus a measured Class B writeup even if red |
| Vikunja `#15`: measure Class R reach/team at N>=5 | Evaluation owner / W3.5 board parent `#2` | Code and scenarios landed, but identity-A-to-identity-B reach needs a live run | GitHub Actions run URL or bounded TSV under `docs/eval/`, clearly labelled as proxy evidence, not Phase 5 proof |
| Phase 5 pilot validation | Phase 5 owner | W3.5 scorecard is a proxy and cannot prove two-user / two-machine adoption | Pilot metrics and evidence from the Phase 5 plan |

## What W3.5 can claim now

- The old single-fact A/B gate is retired and replaced by the four-arm
  memory-value scorecard.
- The unified scorecard harness and scenario inventory cover C, A, B, R, and the
  safety gate.
- Class C / Freshness has raw measured evidence and is green.
- The hard safety gate held for the recorded Class C measured run.
- A/B/R are landed in code and self-tested, but not measured.

## What W3.5 cannot claim now

- It cannot claim A/B/R passed, because no raw A/B/R N>=5 artifact or run URL is
  recorded.
- It cannot claim full-scorecard safety across A/B/R, because those dimensions
  have not been live-measured.
- It cannot claim Phase 5 pilot validation, team adoption, or two-machine proof.
- It cannot treat a future Class B red result as a release blocker unless the
  run shows memory-induced errors; capture fidelity is tracked evidence.

## Addendum (2026-07-12): the deferred N>=5 read has been taken

The "landed, unmeasured" state above is closed:
`docs/eval/2026-07-12-w35-scorecard-n5-run.md` records the first full N=5
measured run (Classes A, B, C, R; run 29203432198). Headlines: ADR-3
**ratified** (Opt 3), capture fidelity 100% (the expected-red that wasn't),
reach 33% vs 0% realistic, and the safety gate **fired** (2 memory-induced
errors, `fresh-test-runner`) — tracked as Vikunja #502. Vikunja #381/#382/#383
(the follow-ups filed by this closeout as #13/#14/#15) are delivered by that
artifact.
