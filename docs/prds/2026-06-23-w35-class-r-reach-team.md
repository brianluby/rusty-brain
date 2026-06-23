# PRD: W3.5 Class R Reach/Team Scorecard Simulation

## Status

Draft, implementation-ready. Class R is stretch/report-only and must not become a hard release gate beyond the existing zero memory-induced-errors safety gate.

## Owner Area

Primary: W3.5 memory-value scorecard.

Touchpoints:

- `scripts/memory-scorecard.sh`
- `crates/rb-eval/scorecard/memory_scorecard_scenarios.json`
- `.github/workflows/memory-scorecard.yml`
- `docs/eval/` for measured results

## Problem

The current scorecard measures freshness and retrieval, but it does not exercise the main team-value thesis: one teammate records a project decision and another teammate can use it before that decision appears in committed project docs.

Class R must simulate two identities with separate local agent state while sharing exactly one rusty-brain store and namespace. The question is whether identity B can answer or act correctly using memory written by identity A.

## Goals

- Add a `reach` scorecard dimension.
- Simulate identity A and identity B with separate homes, project directories, and agent state.
- Share one `RUSTY_BRAIN_DB`, one socket, and one namespace for memory-on.
- Plant facts explicitly from identity A so reach is not confounded by Class B auto-capture quality.
- Score only identity B's work.
- Preserve four scorecard arms.
- Keep reach report-only.

## Non-Goals

- Do not build real team sync, remote transport, trust weighting, user auth, or memory promotion.
- Do not depend on Class B auto-capture.
- Do not require separate OS users or machines.
- Do not add LLM judging.
- Do not claim this proves the later two-user/two-machine pilot.

## Functional Requirements

### R1. Scenario Rows

Add at least two, preferably three, rows with:

```json
{
  "id": "reach-plugin-manifest-path",
  "dimension": "reach",
  "plant_mode": "explicit",
  "plant": "Decision: plugin metadata lives at plugins/rusty-brain/.claude-plugin/plugin.json.",
  "realistic_claude_md": "",
  "steelman_claude_md": "Plugin metadata lives at plugins/rusty-brain/.claude-plugin/plugin.json.",
  "work": "Update plugin metadata and name the manifest path.",
  "expect": "plugins/rusty-brain/.claude-plugin/plugin.json"
}
```

Candidate scenarios:

- `reach-plugin-manifest-path`
- `reach-socket-serializer`
- `reach-daemon-e2e-command`

### R2. Identity Isolation

For memory-on reach scenarios, create two identities:

```text
$base/on/ha  # A home
$base/on/pa  # A project
$base/on/hb  # B home
$base/on/pb  # B project
```

A and B must share:

```text
RUSTY_BRAIN_DB="$base/on/memory.db"
RUSTY_BRAIN_SOCKET="$base/on/daemon.sock"
RUSTY_BRAIN_NAMESPACE="rb-sc-$id-r$run"
```

Identity A plants the fact with `plant_explicit`. Identity B runs `score_session`; only B output and B-written files are judged.

### R3. Baseline Semantics

Realistic baseline:

- B has no memory.
- B has no target-bearing `CLAUDE.md`.
- A may have an audit-only project state, but it must never be read by B.

Steelman baseline:

- B has no memory.
- B's `CLAUDE.md` contains the exact target fact.

Memory-off:

- B has no memory.
- B has no target-bearing `CLAUDE.md`.

### R4. Scoring

Success is B correctness:

- `expect` is present in B judged text.
- `forbid`, if configured, is absent.
- Reach scenarios should generally omit `stale_token`; freshness safety belongs to Class C.

Reach dimension verdict:

- memory-on beats realistic baseline
- memory-on ties steelman within the existing `tie_margin`
- verdict is reported but not release-gating

## Acceptance Criteria

- Scenario file contains at least two `dimension: "reach"` rows.
- `scripts/memory-scorecard.sh --self-test` includes reach setup invariants.
- A `runs=1` reach smoke emits four rows per reach scenario.
- Memory-on reach rows are scored from B, not A.
- Realistic B has no target fact in `CLAUDE.md`.
- Steelman B has the target fact in `CLAUDE.md`.
- A full `runs>=5` read reports reach success with confidence interval, turns, cost, and verdict.
- Reach failure does not fail the job unless a memory-induced error occurs.

## Verification

Run:

```bash
jq empty crates/rb-eval/scorecard/memory_scorecard_scenarios.json
scripts/memory-scorecard.sh --self-test
```

Then run a reach-only smoke if a dimension filter exists. If no filter exists, run the full scorecard at `runs=1`, inspect rows for `dimension=reach`, and promote to a measured run only after baseline leakage checks pass.

## Risks

- Baseline leakage. Mitigate with separate A/B homes and projects, and judge only B artifacts.
- Namespace mismatch. Force one explicit namespace for A plant and B work.
- Capture confound. Use explicit plant, not auto-capture.
- Cost growth. Start with two reach scenarios or a `runs=1` smoke before `runs>=5`.
- Overclaiming. Label Class R as a proxy, not the final two-user/two-machine proof.

## Implementation Checklist

- [ ] Add reach scenarios.
- [ ] Add reach branch in `run_scenario`.
- [ ] Create separate A/B homes and projects.
- [ ] Share one DB, socket, and namespace.
- [ ] Plant facts from A.
- [ ] Score B only.
- [ ] Ensure B memory-on has no target-bearing docs.
- [ ] Ensure realistic B omits target.
- [ ] Ensure steelman B includes target.
- [ ] Extend `--self-test`.
- [ ] Run live `runs=1`.
- [ ] Run measured `runs>=5`.
- [ ] Publish measured reach result as report-only.
