# Bounded Phase 5 dogfood pilot preregistration

- **Status:** **BLOCKED — protocol only; no treatment has started.**
- **Admission blockers:** task #56 is currently **NO-GO** after independent
  reproduction found the low-confidence instruction-shaped poison in the
  second production recall result; task #57 admission, the confirmed repository
  subset, exact UTC period, and the frozen paired-scenario plan also remain
  unresolved.
- **Candidate repositories:** `rusty-brain`, `threatmitigator`, and
  `vikunja-rust-mcp`. This list is not a selection.
- **Operator:** a maintainer running the pilot and an independent reviewer
  confirming the sanitized evidence.
- **Ground truth:** this pilot measures bounded single-user dogfood value. It
  is not the Phase 5c two-user/two-machine team-mode gate.

The manifest is
[`2026-07-12-phase5-dogfood-pilot-manifest.json`](2026-07-12-phase5-dogfood-pilot-manifest.json).
It is intentionally invalid for execution while fields are pending. The
aggregator is `scripts/dogfood-pilot.py`.

The manifest records `task_56.overall_pilot_go=false` and the poison-exposure
blocker. It also records `task_57.complete=false`; partial benchmark evidence
does not supply a pilot envelope. The corrected #56 decision leaves both exact
evidence and surprise-aware combined admission at **NO-GO**, so no treatment
arm is production-qualified. Only a separately reviewed production fix plus a
repeated #56 gate may change `task_56.state` from `no-go` to `go` and
`task_56.overall_pilot_go` to `true`.

## 1. What is frozen now

The pilot lasts exactly **14 consecutive days**. Each selected repository has
one explicit plan with **at least five paired observations**. The exact run
count, start/end timestamps, repository subset, commits, scenarios, and arm
order must be filled in and frozen before the first treatment observation.
They cannot be selected after outcomes are visible.

The thresholds below are compiled into the aggregator. A manifest with a
different threshold object is rejected; changing a threshold requires a new
preregistration and a new pilot, not an edit to an observed run.

| Decision | Frozen threshold |
|---|---:|
| memory-induced errors | 0 |
| contested-answer regression | at most 1.0 percentage point |
| multi-answer regression | at most 1.0 percentage point |
| provenance labels retained | 100% of injections |
| contested labels retained | 100% of contested injections |
| time to first useful recall | at most 72 hours in every selected repository |
| repository activation coverage | 100% |
| helpful share of rated injections | at least 80% |
| wrong share of rated injections | at most 10% |
| stale share of rated injections | at most 10% |
| paired task-success regression | at most 1.0 percentage point |
| paired correction-incidence regression | at most 1.0 percentage point |
| median turns saved | at least 0 |
| median operator-active seconds saved | at least 0 |
| positive paired efficiency | turns or active time must improve, not merely tie |
| exact recoveries | at least 1 |
| semantic helpful recalls | at least 1 |
| review-backlog growth at close | at most 0 per repository |
| backup drill | at least 1 attempt and 0 failures |
| retention drill | at least 1 action and 0 failures |

Corpus growth, ignored injections, stale/wrong exact injections, injected
tokens, injected-token USD attribution, review effort, and retention/backup
active time are reported even where they do not have a value threshold.

## 2. Admission: all fields must be green

Do not install or enable a treatment from this protocol. Admission requires:

1. Task #56 records `state=go` and `overall_pilot_go=true`, clears `blocker`,
   supplies an immutable `commit:`, `sha256:`, or `github-run:` evidence
   reference, and names exactly one qualified treatment
   arm. The treatment uses only that arm. Experimental
   ranking, retention, admission, or embedding behavior that did not qualify
   remains shadow-only and cannot graduate through this pilot.
2. Task #57 records `state=go` and `complete=true`, supplies the same kind of
   immutable reviewed evidence reference, and sets a positive maximum
   active-memory count. Every treatment observation must remain under
   that ceiling. Known resource limits remain operating constraints, not
   waived findings.
3. The repository subset, full commit SHAs, globally unique store/namespace
   tuples, pair IDs, scenario IDs, sanitized fixture SHA-256 values, and
   alternating arm order are frozen in the manifest.
4. The exact UTC start/end timestamps span 14 days. `frozen_at_utc` precedes
   the start. The exact per-repository pair count is at least five and matches
   every repository's plan.
5. The operator runs:

   ```bash
   python3 scripts/dogfood-pilot.py validate \
     --manifest docs/eval/2026-07-12-phase5-dogfood-pilot-manifest.json
   ```

The command fails closed on a pending task, missing field, changed threshold,
unqualified arm, unfrozen pair, non-isolated store, or invalid period. A
successful validation means the protocol is executable; it does not start the
pilot.

## 3. Paired scenario method

Each pair uses the same repository commit, predeclared task, sanitized fixture
snapshot hash, and agent. Only the memory treatment differs.

- **Baseline:** memory is disabled and the task runs in its own environment,
  namespace, and store.
- **Treatment:** the task runs in a second environment, namespace, and store,
  using only the #56-qualified arm and staying below the #57 active-memory
  ceiling.
- **Isolation:** never copy a treatment database, injected context, generated
  answer, or learned memory into the baseline. Never reuse either store for the
  other arm. Restore both arms from the same sanitized task fixture.
- **Order:** `baseline_first` and `treatment_first` are frozen per pair. Balance
  them as evenly as the exact run count permits. The observation must record
  the planned order; the aggregator rejects a mismatch.
- **Scenarios:** use ordinary repository work that can be repeated safely and
  judged without inspecting raw model text. Include predeclared contested and
  multi-answer cases so their regressions are measurable rather than silently
  absent.
- **Attribution:** every treatment observation records both the agent and one
  delivery surface: `claude_native`, `http`, or `mcp`. Claude hook injection is
  not pooled with HTTP/MCP in the report.

Run the interim aggregation immediately after every completed pair. A nonzero
STOP exit means no further treatment work may run.

```bash
python3 scripts/dogfood-pilot.py aggregate \
  --manifest docs/eval/2026-07-12-phase5-dogfood-pilot-manifest.json \
  --events /path/to/sanitized-pilot-events.jsonl \
  --output /path/to/interim-report.json \
  --interim
```

## 4. Exact metric definitions

One JSONL row represents one completed pair. Counts are nonnegative integers;
durations are operator-active seconds, not wall-clock calendar time.

| Metric | Definition |
|---|---|
| time to first useful recall | hours from the frozen pilot start to the first pair with at least one treatment injection classified `helpful`, computed separately per repository |
| helpful/wrong/stale ratios | each count divided by `helpful + wrong + stale`; ignored injections are reported separately and excluded from this denominator |
| ignored injections | injected recall events presented to the agent but not used in the task outcome |
| corrections | operator interventions needed to repair an arm's answer or work product; final comparison uses the share of pairs with one or more corrections |
| turns saved | baseline turns minus treatment turns for each pair; report the median and inclusive Q1–Q3 |
| active time saved | baseline operator-active seconds minus treatment seconds for each pair; report the median and inclusive Q1–Q3 |
| corpus growth | last treatment `corpus_rows_after` minus first `corpus_rows_before`, and the equivalent byte delta, per repository |
| review backlog | final treatment backlog minus initial backlog, per repository |
| retention friction | action count, failure count, and operator-active seconds spent on the retention drill |
| backup friction | attempt count, failure count, and operator-active seconds spent on the backup drill |
| exact recovery | helpful injection that recovers the predeclared literal fact or decision span |
| semantic helpful recall | helpful injection that resolves the task without literal-span matching and passes the frozen deterministic or human rubric |
| stale/wrong exact injection | injection containing the predeclared exact stale or wrong answer span, whether or not the agent acts on it |
| memory-induced error (MIE) | treatment failure in which memory supplies, omits, or weakens the current evidence such that the agent acts on a stale/wrong answer; preserve the scorecard's fail-closed classification |
| contested regression | baseline contested correctness minus treatment contested correctness, in percentage points |
| multi-answer regression | baseline multi-answer correctness minus treatment multi-answer correctness, in percentage points |
| injected-token cost | exact tokens added by treatment injections; USD is recorded only when the run can attribute it exactly, otherwise it is `null` and explicitly unmeasured |

`injections_total` must equal helpful + wrong + stale + ignored. Exact
recoveries and semantic helpful recalls are disjoint subsets of helpful
injections; their sum cannot exceed the helpful count.
Every injection records provenance-label coverage; every contested injection
records contested-label coverage. The tool rejects inconsistent partitions or
counts.

## 5. Observation and qualitative-evidence format

The full machine-readable schema is enforced by `scripts/dogfood-pilot.py`.
Each pair contains:

- opaque pair/scenario IDs, repository, commit, timestamp, frozen arm order,
  and SHA-256 of the sanitized common fixture;
- baseline and treatment environment/store/namespace IDs;
- task success, turns, active seconds, corrections, contested opportunities
  and correct answers, and multi-answer opportunities and correct answers for
  each arm;
- the treatment arm/surface plus the injection, safety, corpus, backlog,
  retention, backup, and token fields defined above;
- zero or more sanitized qualitative examples.

A qualitative example has exactly five fields:

```json
{
  "category": "helpful",
  "sanitized_summary": "Recovered the chosen test command without exposing stored text.",
  "provenance_label": "hook",
  "contested": false,
  "sanitization_attested": true
}
```

Allowed categories are `helpful`, `wrong`, `stale`, `correction`, and
`friction`. The summary is a single line of at most 280 characters. It describes
the outcome, never the prompt, transcript, response, memory body, secret,
username, or absolute path. The tool rejects unknown fields, common secret/key
patterns, and home-directory paths. Pattern rejection is only a backstop: the
operator and reviewer must attest that the example is sanitized.

Do not add raw-content fields. Do not collect database rows, model output,
prompts, transcripts, diffs, environment variables, API keys, or memory bodies.
Do not create a separate raw pilot artifact. Ordinary source-system data stays
under its existing local retention policy and is never copied into this study.

## 6. Stop, continue, go, and no-go

### Immediate STOP

Stop treatment and preserve only sanitized evidence when any interim report
finds:

- one or more MIEs;
- contested or multi-answer correctness regression greater than 1.0 percentage
  point;
- either regression remains unmeasured at final aggregation;
- provenance or contested labels below 100%; or
- an input/admission/isolation validation error.

Do not tune thresholds, scenarios, labels, ranking, prompts, or the qualified
arm and resume the same pilot. Diagnose under a separate task; any material
change requires a fresh preregistration and new observations.

An interim report may leave contested or multi-answer regression unmeasured
until its predeclared scenario runs. Final aggregation treats either missing
axis, or missing label coverage, as STOP rather than silently passing it.

### CONTINUE

An interim report says CONTINUE only when all observations match the frozen
plan and no stop condition has fired. It makes no value claim.

### Final GO

GO requires all planned pairs, no stop condition, and every frozen value and
operational threshold in section 1. Both paired medians must be nonnegative and
at least one must be positive. Exact and semantic value must each occur at
least once. Every selected repository must reach a helpful recall within 72
hours and close without review-backlog growth. At least one successful backup
and retention action must be measured.

### Final NO-GO

NO-GO means the bounded treatment was safe enough to finish but missed one or
more activation, value, or operational thresholds. It does not authorize wider
rollout. Missing final pairs or admission fields are validation errors, not a
NO-GO result.

Final aggregation uses the same command without `--interim`. Exit codes are 0
for GO/CONTINUE, 2 for invalid or unsafe input, 3 for STOP, and 4 for NO-GO.

## 7. Reproducibility and review

Before admission, and again at close:

```bash
python3 scripts/dogfood-pilot.py --self-test
python3 -m py_compile scripts/dogfood-pilot.py
```

The final report must use
[`2026-07-12-phase5-dogfood-pilot-report.md`](2026-07-12-phase5-dogfood-pilot-report.md).
Attach the frozen manifest, sanitized JSONL, aggregate JSON, repository commit
SHAs, #56/#57 evidence, and reviewer attestation. Record every unavailable
metric as unmeasured; never convert missing evidence to zero.

## 8. What this protocol cannot claim

Until the pilot actually runs, every outcome metric is unmeasured. Even a GO
would cover only the selected repositories, commits, agent/surfaces, qualified
arm, 14-day period, and #57 resource envelope. It would not prove multi-user or
multi-machine adoption, team hub/curation, production-wide semantic quality,
unbounded scale, disk-full recovery, or safe graduation of any shadow-only
experimental behavior.
