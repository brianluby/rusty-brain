# W3.5 memory-value scorecard — Class C (Freshness), MEASURED

- **Status:** **MEASURED — green.** First measured run of the redesigned
  memory-value scorecard (the scaffold + Class C / Freshness). This is
  build-sequence step 3 ("Validate") from
  `docs/eval/2026-06-16-w35-criterion-redesign.md`, which left the measured C run
  *deferred*.
- **Date:** 2026-06-21.

## What this measures

Class C / **Freshness** from the criterion redesign: a decision X is later
superseded by X'. When a later task needs the **current** value, who supplies it?
Four arms (P1, dual baselines):

| arm | setup | what it represents |
|---|---|---|
| **memory-on** | both facts planted explicitly; the 2nd supersedes the 1st, so recall returns X' | the product |
| **realistic-baseline** | the CLAUDE.md a real team actually has: **stale**, still records X | reality (nobody updated the docs) |
| **steelman-baseline** | a diligent human's CLAUDE.md, updated to X' | the best a human file gets |
| **memory-off** | nothing | the floor |

The dimension's claim (P1): memory-on **BEATS** the realistic baseline **AND TIES**
the steelman. The only hard gate (P4): **zero memory-induced errors** (never serve
a superseded value as current).

## Provenance

| field | value |
|---|---|
| Workflow | `.github/workflows/memory-scorecard.yml` ("memory scorecard") |
| Run | [27917957530](https://github.com/brianluby/rusty-brain/actions/runs/27917957530) — `workflow_dispatch`, `macos-latest`, conclusion **success** |
| Commit | `b601fea` (branch `feat/validate-class-c-freshness`) |
| N | 10 runs per (scenario, arm) → 40 obs/arm, **160 sessions** |
| Model / budget | `haiku`, `--max-budget-usd 0.50` per session (hard per-session cap) |
| Scenarios | `crates/rb-eval/scorecard/memory_scorecard_scenarios.json` (4 freshness) |
| Raw data | [`2026-06-21-w35-class-c-freshness-n10.tsv`](./2026-06-21-w35-class-c-freshness-n10.tsv) (columns: `dimension  scenario  arm  run  success  turns  mie`) |

Validation also ran the smokes the build sequence calls for, all green: the local
`--self-test` (judge + aggregation math, no API); a GitHub-dispatch **N=1**
plumbing smoke ([run 27917826435](https://github.com/brianluby/rusty-brain/actions/runs/27917826435));
and a local live N=1.

## Result (N=10)

```
dimension   arm                 runs  success [95% CI]     med_turns [Q1-Q3]
freshness   memory-on             40   100% [91.2-100.0]    1.0 [1.0-4.0]
freshness   realistic-baseline    40     0% [0.0-8.8]       1.0 [1.0-3.0]
freshness   steelman-baseline     40   100% [91.2-100.0]    1.0 [1.0-3.0]
freshness   memory-off            40    25% [14.2-40.2]     3.0 [1.8-3.2]
  -> freshness: beats_realistic=yes ties_steelman=yes  => PASS
scorecard: 1/1 dimensions pass (tracked, non-gating)
SAFETY — memory-induced errors: 0 (allowed 0)
result: SAFE
```

(95% CI = Wilson; turns = median [Q1–Q3]. N=10 ≥ `min_runs=5`, so this is a
gating-grade read, not directional.) Per-scenario, memory-on was **10/10 on all
four scenarios** (`fresh-http-switch`, `fresh-config-rename`, `fresh-id-type`,
`fresh-test-runner`) — the headline is not carried by one easy scenario.

## Reading

- **memory-on beats the realistic baseline decisively: 100% vs 0%.** The stale
  CLAUDE.md does not merely fail to help — at **0%** it is *worse than the
  memory-off floor (25%)*. A confidently-stated obsolete decision actively steers
  the model to the wrong (superseded) value, whereas no docs at least lets the
  model sometimes guess the current one. This is the core Class C thesis: stale
  documentation is a liability, and memory's auto-supersede removes it.
- **memory-on ties the steelman: 100% vs 100%.** Against a perfectly-maintained
  file, memory is at parity — consistent with ADR-1 (single-fact parity with a
  *fresh* file is a non-goal). Memory's edge is that it *stays* fresh without the
  human diligence the steelman arm assumes.
- **Safety gate holds: 0 memory-induced errors over 40 memory-on sessions.** The
  supersede path returned the current value every time and never served the
  archived predecessor as current.

## Caveats (what this does and does not license)

- **Proxy, not ground truth (ADR-2).** These are `claude -p` one-shots against
  `haiku`. The scorecard is a regression guard and progress tracker; the Phase-5
  pilot is the arbiter. A green C does not by itself close the broader gate.
- **One dimension of four.** Build order is C → A → B → R; only **C (Freshness)**
  is instantiated here. A (retrieval@scale), B (capture), R (reach/team) remain.
- **Non-gating.** Per P4 the only PR-blocking signal is zero memory-induced
  errors; the per-dimension verdict is tracked, not gated.

## Harness fix landed during validation

The first dispatch smoke failed with an unactionable `daemon did not bind`. Root
cause: the memory-on daemon subshell launched bare `rusty-brain serve` without
putting `$BIN_DIR` first on `PATH` (unlike `run_session` / `install` /
`plant_explicit`), so on a host with another `rusty-brain` earlier on `PATH` it
ran the wrong binary (no `serve` subcommand) and the bind gate fired with the
daemon's stderr discarded to `/dev/null`. Fixed in this branch: PATH-prefix the
daemon subshell; capture the daemon output and print it (plus daemon-alive state)
on a bind failure; and break the wait the instant the daemon process dies. The
captured-output change is what surfaced the true cause on the next run.

## Schedule

With C green, `memory-scorecard.yml` is moved from manual-dispatch-only to
**weekly** (Mondays, N=5) plus manual dispatch, matching the `w35-ab-eval.yml`
posture: **ALLOWED TO FAIL, WITH ALERTING** (opens/comments/closes a pinned alert
issue), and it **NEVER gates PRs**.
