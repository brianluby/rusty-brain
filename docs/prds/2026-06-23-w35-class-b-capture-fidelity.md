# PRD: W3.5 Class B Capture Fidelity Scorecard

## Status

Draft, implementation-ready.

## Owner Area

Primary: W3.5 memory-value scorecard and evaluation tooling.

Touchpoints:

- `scripts/memory-scorecard.sh`
- `crates/rb-eval/scorecard/memory_scorecard_scenarios.json`
- `.github/workflows/memory-scorecard.yml`
- `crates/rb-hooks/src/capture.rs` only if the harness exposes a hook bug
- `docs/eval/` for measured results after a live run

## Problem

The W3.5 scorecard can measure memory value after facts are explicitly planted, but it does not yet prove that facts emerging during real agent work are captured with enough fidelity to be useful later.

The product loop is:

1. A decision emerges during a session.
2. A hook captures the session.
3. `SessionEnd` folds transcript, scratch, and working-tree state into one memory.
4. A later session recalls the memory.
5. The agent acts on it.

Current explicit-plant scorecard paths mostly exercise steps 4 and 5. Class B must isolate steps 1 through 3 so a later failure can be attributed to capture loss, retrieval loss, or model behavior instead of being opaque.

## Goals

- Add Class B capture scenarios to the W3.5 scorecard.
- Implement `plant_mode: "auto-capture"` for memory-on rows.
- Run a real Claude Code plant session and rely on the real `SessionEnd` hook fold.
- Measure direct capture fidelity from the store before downstream recall.
- Preserve the existing four-arm design: `memory-on`, `realistic-baseline`, `steelman-baseline`, and `memory-off`.
- Report Class B as tracked evidence without changing the hard gate, which remains zero memory-induced errors.

## Non-Goals

- Do not fix capture quality as part of this PRD.
- Do not use explicit `rusty-brain remember`, MCP `remember`, or hand-written `CLAUDE.md` content as the memory-on plant.
- Do not score capture fidelity through recall ranking.
- Do not evaluate non-Claude adapters in this PRD.
- Do not make Class B a PR-blocking gate.

## Functional Requirements

### B1. Scenario Schema

Add at least three scenarios with `dimension: "capture"` and `plant_mode: "auto-capture"`.

Required fields:

```json
{
  "id": "cap-http-ureq",
  "dimension": "capture",
  "plant_mode": "auto-capture",
  "plant_session": "Decision: outbound HTTP uses ureq.",
  "realistic_claude_md": "",
  "steelman_claude_md": "Outbound HTTP uses ureq.",
  "work": "Choose the HTTP client for a new outbound call.",
  "expect": "ureq",
  "capture_expect": "ureq"
}
```

Rules:

- `plant_session` is required for auto-capture scenarios.
- `plant` must be absent or empty for auto-capture scenarios.
- `capture_expect` defaults to `expect` when omitted.
- `capture_forbid` is optional.
- Initial scenario IDs should cover HTTP client, error handling, and ID strategy, such as `cap-http-ureq`, `cap-error-apperror`, and `cap-id-ulid`.

### B2. Auto-Capture Plant

For each memory-on capture scenario, the runner must:

- Start an isolated daemon with the same DB, socket, and namespace used by the later work session.
- Install Claude Code hooks into the plant project.
- Run the plant prompt through Claude Code.
- Allow the real `SessionEnd` hook to fire naturally.
- Avoid explicit remember paths entirely.
- Prevent MCP remember from masking SessionEnd loss, either by disabling project MCP for the plant session or by filtering fidelity strictly to hook-origin session summaries.

### B3. Direct Fidelity Measurement

After the plant session and before the downstream work session, compute capture fidelity from store contents.

`capture_fidelity = 1` only when a live memory exists where:

- `origin_source == "hook"`
- `tags` contains `session-summary`
- `archived_at == null`
- `content + summary` contains `capture_expect`, case-insensitively
- `capture_forbid`, when present, is absent

Use direct listing, not recall:

```bash
rusty-brain --json list --limit 1000
```

The helper may poll briefly, up to about 10 seconds, to account for process cleanup timing.

Capture reasons to emit:

- `cap_ok`
- `cap_no_session_summary`
- `cap_summary_missing_fact`
- `cap_forbidden_token`
- `cap_list_error`
- `cap_timeout`
- `cap_mcp_bypass_detected`

### B4. Downstream Recall

After direct fidelity is measured, the runner must still run the downstream memory-on work session and score the usual output. This produces two metrics:

- Direct capture fidelity: did a hook-origin session summary preserve the decision?
- End-to-end recall: did a later session act on the decision?

The downstream memory-on project must not contain a `CLAUDE.md` answer leak.

### B5. Aggregation and Artifacts

Keep the existing scorecard rows stable. If adding TSV columns, append them after the existing fields:

```text
cap_fidelity cap_reason cap_summary_count cap_mcp_bypass_count
```

For non-capture rows, use a consistent neutral value such as `na` or `0`.

Report:

- Capture fidelity by scenario.
- Aggregate Class B fidelity rate.
- Wilson 95 percent CI.
- Existing end-to-end success by arm.
- Existing turns, token, and cost metrics.

Class B should be considered green when:

- `capture_fidelity_rate >= 0.80`
- memory-on beats realistic baseline
- memory-on ties steelman within the existing margin
- memory-induced errors remain zero

Only memory-induced errors should fail the scorecard as a hard gate.

## Acceptance Criteria

- `memory_scorecard_scenarios.json` contains at least three capture scenarios.
- `auto-capture` runs a real Claude Code plant session.
- Direct fidelity is measured before downstream work starts.
- Fidelity only counts live hook-origin `session-summary` memories.
- Explicit remember and MCP remember cannot create a false capture pass.
- Existing freshness and retrieval-scale scenarios still run.
- `scripts/memory-scorecard.sh --self-test` covers fidelity parsing and aggregation without API calls.
- A live `runs=1` dispatch prints Class B fidelity output.
- A measured `runs>=5` artifact is recorded under `docs/eval/`, even if Class B is red.

## Verification

Run:

```bash
jq empty crates/rb-eval/scorecard/memory_scorecard_scenarios.json
scripts/memory-scorecard.sh --self-test
```

Then run a manual `memory-scorecard.yml` dispatch with `runs=1`. Promote to `runs>=5` only after logs show:

- plant session executed
- direct fidelity measured
- no explicit plant path used
- no `CLAUDE.md` answer leak in memory-on

## Risks

- MCP remember masks SessionEnd capture loss. Mitigate with strict hook-origin filtering and bypass counters.
- Claude Code does not fire `SessionEnd` in CI. Classify as `cap_no_session_summary` and compare with `scripts/nightly-claude-smoke.sh`.
- Class B increases runtime and cost. Use `runs=1` for plumbing and `runs>=5` only for measured reads.
- A red Class B result is mistaken for a release blocker. Keep output explicit that it is tracked and non-gating except for safety.

## Implementation Checklist

- [ ] Add capture scenarios.
- [ ] Parse `plant_session`, `capture_expect`, and `capture_forbid`.
- [ ] Implement `run_auto_capture_plant`.
- [ ] Implement direct hook-origin fidelity filtering.
- [ ] Record fidelity before `score_session`.
- [ ] Append or companion-write capture fields.
- [ ] Extend scorecard aggregation.
- [ ] Extend `--self-test`.
- [ ] Update workflow comments/artifact schema.
- [ ] Run live `runs=1`.
- [ ] Run measured `runs>=5`.
- [ ] Publish measured Class B artifact.
