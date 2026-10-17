# Bounded Phase 5 dogfood pilot report

- **Status:** **NOT STARTED — CANDIDATE PREPARED; ADMISSION STILL BLOCKED**
- **User decision (2026-10-16):** preparation only, no admission or operator
  assignment. Technical evidence can be completed without starting treatment.
- **Protocol:**
  [`2026-07-12-phase5-dogfood-pilot-preregistration.md`](2026-07-12-phase5-dogfood-pilot-preregistration.md)
- **Candidate pilot ID:** `phase5-20261019`
- **Frozen at:** NOT FROZEN
- **Candidate period:** 2026-10-19T16:00:00Z–2026-11-02T16:00:00Z (14 days)
- **Planned pairs per repository:** 6 (three per arm order); **observed:** none
- **Operator / independent reviewer:** NOT ASSIGNED / NOT ASSIGNED
- **Final verdict:** UNMEASURED

This is a prospective report, not observed pilot evidence. The manifest and
sanitized source-derived tasks are prepared; passing candidate validation is
not resource admission, a final freeze, or fourteen days of measured value.
Do not start treatment from this document. Task #59 remains incomplete.

Four distinct private store roots and local-model configurations are reserved in
[the store preparation record](2026-10-16-pilot-store-plan.json). They contain no
initialized databases, copied credentials or treatment observations.

## Admission record

| Gate | State | Immutable evidence | Frozen constraint |
|---|---|---|---|
| #56 semantic quality | **GO for the production arm only** (`overall_pilot_go=true`) | `commit:ac4bf4c3cfa951035f7992e027309a6f281ce87e`; PR #90 merged; all four gate/diagnostic commands passed on 2026-10-13 | blocker cleared; arm: `linear-with-recall-confidence-floor`; no experimental behavior |
| #57 scale/resources | **EVIDENCE UNAVAILABLE** (`complete=false`) | original named code/artifact not recovered; replacement real-local scale matrix and SQLite page-limit recovery evidence must be run and retained on the clean integration revision | proposed 25,000-memory ceiling is not adopted; physical-full deferral is limited to maintainer comment 182's bounded trial |
| repositories | CANDIDATE PREPARED | rusty-brain `a33ea3ba0fef50b94b1d65884118384d7886a24d`; threatmitigator `580cfff8d0fca117171b9ac40e05e3fc5486ac78` | rusty-brain may be repinned to final integration commit before any treatment; task SHA is separate from treatment-binary SHA |
| paired plan | CANDIDATE PREPARED, NOT FROZEN | manifest plus private local scenario bundle; hash in the store plan | twelve hashed synthetic, real-code-derived task inputs; six pairs/repository; no pair observations |

Confirm that unqualified experimental behavior remained disabled: UNMEASURED.

## Execution conditions

| Repository | Commit | Baseline store / namespace | Treatment store / namespace | Planned / observed pairs |
|---|---|---|---|---:|
| rusty-brain | `a33ea3ba0fef50b94b1d65884118384d7886a24d` | `phase5-20261019-rusty-brain-baseline` / same namespace ID | `phase5-20261019-rusty-brain-treatment` / same namespace ID | 6 / UNMEASURED |
| threatmitigator | `580cfff8d0fca117171b9ac40e05e3fc5486ac78` | `phase5-20261019-threatmitigator-baseline` / same namespace ID | `phase5-20261019-threatmitigator-treatment` / same namespace ID | 6 / UNMEASURED |

Agents and attributed surfaces:

| Agent | Claude-native pairs | HTTP pairs | MCP pairs |
|---|---:|---:|---:|
| UNCONFIRMED | UNMEASURED | UNMEASURED | UNMEASURED |

Model route candidate: `litellm/deepseek-v4.1-flash`; provider backend version is
**unexposed**, not pinned. Both arms must record identical route/version marker
and an 8,000-token common prompt budget. Planned surface coverage is two
Claude-native, two HTTP and two MCP pairs per repository, not observed usage.
The study stores do not exist yet; identifiers must map to separate disposable
DB/runtime/config paths without touching the installed production dogfood.


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

- final admission, integrated treatment revision, freeze, actual execution
  timestamps and completion of the prepared schedule;
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
python3 scripts/dogfood-pilot.py validate --candidate \
  --manifest MANIFEST.json --snapshots SCENARIOS.json
# After real integrated gates and final freeze, omit --candidate:
python3 scripts/dogfood-pilot.py validate \
  --manifest MANIFEST.json --snapshots SCENARIOS.json
python3 scripts/dogfood-pilot.py aggregate \
  --manifest MANIFEST.json --snapshots SCENARIOS.json \
  --events EVENTS.jsonl --output REPORT.json
```

## Final decision

- **Verdict:** UNMEASURED
- **Stop reasons:** UNMEASURED
- **No-go reasons:** UNMEASURED
- **Allowed next scope:** UNMEASURED
- **Reviewer attestation:** UNMEASURED
