# Bounded Phase 5 dogfood pilot report

- **Status:** **NOT STARTED — BLOCKED ON ADMISSION AND SCHEDULING**
- **Protocol:**
  [`2026-07-12-phase5-dogfood-pilot-preregistration.md`](2026-07-12-phase5-dogfood-pilot-preregistration.md)
- **Pilot ID:** UNCONFIRMED
- **Frozen at:** UNCONFIRMED
- **Period:** UNCONFIRMED (14 consecutive days required)
- **Paired runs per repository:** UNCONFIRMED (N≥5 required)
- **Operator / independent reviewer:** UNCONFIRMED / UNCONFIRMED
- **Final verdict:** UNMEASURED

This file is a report template, not pilot evidence. Replace UNCONFIRMED and
UNMEASURED only from the frozen manifest and sanitized aggregate output. Do not
start treatment from this document.

## Admission record

| Gate | State | Immutable evidence | Frozen constraint |
|---|---|---|---|
| #56 semantic quality | **NO-GO** (`overall_pilot_go=false`) | independent reproduction: instruction-shaped poison is the second production recall result; immutable reviewed fix evidence UNCONFIRMED | blocker set; qualified treatment arm: NONE |
| #57 scale/resources | **NOT COMPLETE** (`complete=false`) | partial evidence exists; reviewed completion evidence UNCONFIRMED | max active memories: UNCONFIRMED |
| repositories | PENDING | frozen full commit SHAs: UNCONFIRMED | subset of rusty-brain / threatmitigator / vikunja-rust-mcp |
| paired plan | PENDING | frozen manifest: UNCONFIRMED | pair IDs, scenarios, common snapshots, and arm order |

Confirm that unqualified experimental behavior remained disabled: UNMEASURED.

## Execution conditions

| Repository | Commit | Baseline store / namespace | Treatment store / namespace | Planned / observed pairs |
|---|---|---|---|---:|
| UNCONFIRMED | UNCONFIRMED | UNCONFIRMED | UNCONFIRMED | UNCONFIRMED / UNMEASURED |

Agents and attributed surfaces:

| Agent | Claude-native pairs | HTTP pairs | MCP pairs |
|---|---:|---:|---:|
| UNCONFIRMED | UNMEASURED | UNMEASURED | UNMEASURED |

## Safety and label preservation

| Metric | Frozen threshold | Result | Verdict |
|---|---:|---:|---|
| memory-induced errors | 0 | UNMEASURED | UNMEASURED |
| contested-answer regression | ≤1.0 pp | UNMEASURED | UNMEASURED |
| multi-answer regression | ≤1.0 pp | UNMEASURED | UNMEASURED |
| provenance-label coverage | 100% | UNMEASURED | UNMEASURED |
| contested-label coverage | 100% | UNMEASURED | UNMEASURED |
| stale/wrong exact injections | report each | UNMEASURED | each injection is report-only unless an MIE occurs; aggregate stale/wrong ratios remain verdict-gated |

If STOP fired, enumerate only sanitized incident IDs and classifications here.
Do not paste prompts, responses, transcripts, or memory content.

## Activation and value

### Per repository

| Repository | Time to first useful recall | Corpus rows / bytes growth | Review-backlog growth | Result |
|---|---:|---:|---:|---|
| UNCONFIRMED | UNMEASURED | UNMEASURED / UNMEASURED | UNMEASURED | UNMEASURED |

### Aggregate

| Metric | Frozen threshold | Result | Verdict |
|---|---:|---:|---|
| repository activation coverage | 100% | UNMEASURED | UNMEASURED |
| helpful ratio | ≥80% | UNMEASURED | UNMEASURED |
| wrong ratio | ≤10% | UNMEASURED | UNMEASURED |
| stale ratio | ≤10% | UNMEASURED | UNMEASURED |
| ignored injections | report | UNMEASURED | report-only |
| baseline / treatment corrections | report | UNMEASURED / UNMEASURED | UNMEASURED |
| correction-incidence regression | ≤1.0 pp | UNMEASURED | UNMEASURED |
| baseline / treatment task success | report | UNMEASURED / UNMEASURED | UNMEASURED |
| task-success regression | ≤1.0 pp | UNMEASURED | UNMEASURED |
| median turns saved [Q1–Q3] | ≥0 median | UNMEASURED | UNMEASURED |
| median active seconds saved [Q1–Q3] | ≥0 median | UNMEASURED | UNMEASURED |
| either paired efficiency metric improves | required | UNMEASURED | UNMEASURED |
| exact recoveries | ≥1 | UNMEASURED | UNMEASURED |
| semantic helpful recalls | ≥1 | UNMEASURED | UNMEASURED |

## Injected-token cost

| Surface | Pairs | Injections | Helpful ratio | Injected tokens | Exact attributed USD |
|---|---:|---:|---:|---:|---:|
| Claude-native | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED |
| HTTP | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED |
| MCP | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED |
| total | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED |

If exact marginal USD cannot be attributed, leave it UNMEASURED. Do not derive
it from a guessed model price or convert missing usage to zero.

## Retention, backup, and review friction

| Metric | Frozen threshold | Result | Verdict |
|---|---:|---:|---|
| retention actions | ≥1 | UNMEASURED | UNMEASURED |
| retention failures | 0 | UNMEASURED | UNMEASURED |
| retention operator-active seconds | report | UNMEASURED | report-only |
| backup attempts | ≥1 | UNMEASURED | UNMEASURED |
| backup failures | 0 | UNMEASURED | UNMEASURED |
| backup operator-active seconds | report | UNMEASURED | report-only |
| final review backlog growth per repository | ≤0 | UNMEASURED | UNMEASURED |

## Sanitized qualitative evidence

Use one row per deliberately selected example. The reviewer must confirm the
source JSONL row carries `sanitization_attested=true`.

| Pair ID | Category | Sanitized outcome summary | Provenance label | Contested | Reviewer attested |
|---|---|---|---|---|---|
| UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED | UNMEASURED |

## Explicitly unmeasured

Until execution, all fields below are unmeasured:

- actual repository selection, commits, start/end timestamps, and run count;
- time to first useful recall and activation coverage;
- helpful, wrong, stale, ignored, correction, turn, active-time, exact-recovery,
  semantic-recall, corpus-growth, review-backlog, retention, and backup metrics;
- MIE count, contested/multi-answer regression, and label preservation;
- Claude-native versus HTTP/MCP attribution;
- injected tokens and exactly attributable injection USD;
- qualitative evidence and reviewer attestation.

The bounded pilot does not measure multi-user/multi-machine adoption, team-mode
sync/curation, unselected repositories, behavior outside the frozen commits,
corpora above the #57 ceiling, disk-full recovery, unbounded production scale,
or any #56-unqualified experimental arm.

## Artifacts and reproduction

| Artifact | Path / immutable reference | SHA-256 or commit |
|---|---|---|
| frozen manifest | UNCONFIRMED | UNCONFIRMED |
| sanitized pair JSONL | UNMEASURED | UNMEASURED |
| aggregate JSON | UNMEASURED | UNMEASURED |
| #56 evidence | UNCONFIRMED | UNCONFIRMED |
| #57 evidence | UNCONFIRMED | UNCONFIRMED |

Commands used:

```bash
python3 scripts/dogfood-pilot.py --self-test
python3 scripts/dogfood-pilot.py validate --manifest MANIFEST.json
python3 scripts/dogfood-pilot.py aggregate \
  --manifest MANIFEST.json --events EVENTS.jsonl --output REPORT.json
```

## Final decision

- **Verdict:** UNMEASURED
- **Stop reasons:** UNMEASURED
- **No-go reasons:** UNMEASURED
- **Allowed next scope:** UNMEASURED
- **Reviewer attestation:** UNMEASURED
