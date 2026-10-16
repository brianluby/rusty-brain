# Bounded Phase 5 dogfood pilot preregistration

- **Status:** **CANDIDATE PREPARED — NOT ADMITTED; no treatment has started.**
- **User decision (2026-10-16):** prepare only; do not admit the pilot, assign
  an operator, or start treatment. Passing technical gates does not override it.
- **Admission blockers:** replacement #57 resource evidence must pass on the
  clean integrated treatment revision, then #56 must be rechecked on that same
  revision. The original PR #70 N>=5 rerun must record zero memory-induced
  errors before treatment; prior semantic diagnostics alone do not replace it.
  Final freeze and operator/reviewer execution remain separate steps.
- **Selected candidates:** `rusty-brain` and `threatmitigator`, six balanced
  pairs each. `vikunja-rust-mcp` was not found locally or by authenticated GitHub
  repository search and is excluded, rather than assigned an invented revision.
- **Operator / independent reviewer:** not yet assigned. Execution needs a
  human to measure active time, adjudicate injections, and attest sanitization;
  neither a scheduler nor this preparation substitutes for those observations.
- **Ground truth:** this pilot measures bounded single-user dogfood value. It
  is not the Phase 5c two-user/two-machine team-mode gate.

The manifest is
[`2026-07-12-phase5-dogfood-pilot-manifest.json`](2026-07-12-phase5-dogfood-pilot-manifest.json).
It remains intentionally non-executable (`status=candidate`, resource evidence
unavailable, no freeze timestamp). `scripts/dogfood-pilot.py validate --candidate`
checks all prepared fields and actual scenario hashes without admitting a run.

The manifest records `task_56.state=go` and `overall_pilot_go=true`, pinned to
`commit:ac4bf4c3cfa951035f7992e027309a6f281ce87e`. On 2026-10-13, all eight
semantic safety tests, the production Linear gate, the five-seed fusion
diagnostic, and the controlled-arms diagnostic passed. The qualified arm is
`linear-with-recall-confidence-floor`; exact-evidence and surprise-aware
experiments remain **NO-GO** and must not be enabled.

Task #57 is marked Done in the tracker. Maintainer comment 182 approved a
bounded single-user trial at 25,000 active memories on 2026-07-15, explicitly
deferring physical filesystem exhaustion rather than claiming a full production
envelope. Its benchmark code (`be6e0c4faa727df546dd81aa79037cec53482860`),
simulated SQLITE_FULL regression (`121d3d7d2e660b0be8ad4a85967a7670be9f3660`),
and pilot handoff (`2c99f13`) were not recovered from local reflogs, unreachable
Git objects, registered worktrees, or retained scale-artifact locations. The
previous GitHub commit-API absence is not a new check. The referenced
`target/scale-benchmark-v4-full.json` remains absent; its recorded SHA-256 is
`c24756340dfaad111637a415a5f8b434ef6e227088997cc8ba9b8c1405b0ab06`.
The replacement extends the existing real-local SQLite/store/transport load
harness with retained actual SQLite page-limit refusal, rollback, recovery and
reopen evidence. See [the resource reproduction contract](../benchmarks/scale-concurrency-harness.md).
No replacement benchmark has been run during preparation. `max_active_memories`
is the **proposed**, not adopted, 25,000 ceiling; `complete=false` continues to
block admission. Only a passing immutable replacement report can adopt it.

## 1. What is frozen now

The pilot lasts exactly **14 consecutive days**. Each selected repository has
one explicit plan with **at least five paired observations**. The exact run
count, start/end timestamps, repository subset, commits, scenarios, and arm
order must be filled in and frozen before the first treatment observation.
They cannot be selected after outcomes are visible.

### Prepared schedule and task snapshots (2026-10-16)

The proposed interval is **2026-10-19T16:00:00Z through
2026-11-02T16:00:00Z**, exactly fourteen days, end-exclusive. Each repository
has six pairs on October 19, 20, 22, 25, 28 and 31: rusty-brain at 16:00 UTC,
threatmitigator at 17:00 UTC. Each plan has three baseline-first and three
treatment-first pairs, with two pairs per delivery surface. The first two
slots permit measuring the unchanged 72-hour activation threshold.

`observed_at_utc` means completion of both arms in a pair, and must fall in
**[that scenario's scheduled UTC, the next scheduled slot in the same repository)**.
The final pair's upper bound is the pilot end. The next repository's slot does
not shorten this window. Both early completions (for example an October 31
scenario completed on October 19) and late completions are rejected; a missed
window is an incomplete planned pair, not permission to backdate or reschedule
after observing outcomes.

The sanitized scenario bundle is private local data at
`${HOME}/.local/share/rusty-brain-pilots/phase5-20261019/scenario-inputs.json`.
It contains source paths, exact task wording, synthetic seed notes, review
rubrics, opportunity counts, surface and UTC slot, but no private transcripts or
observed outputs. `threatmitigator` is private, so task bodies/source paths are
not committed. Public metadata uses opaque private-project scenario IDs and
canonical SHA-256 commitments; the store plan records the whole bundle hash.
Hashes use UTF-8 JSON with sorted keys, ASCII escaping and separators `,`/`:`.
Candidate validation recomputes every hash, rejects globally duplicate scenario
IDs before indexing plans, and requires coverage of the whole pair plan.
These are controlled source-derived tasks in real repositories,
not evidence of fourteen days of organic use; the eventual report must retain
that distinction.

The current task-source commits are rusty-brain
`a33ea3ba0fef50b94b1d65884118384d7886a24d` and threatmitigator
`580cfff8d0fca117171b9ac40e05e3fc5486ac78`. Task-source commits and the
memory-treatment binary's verified revision are distinct: the latter is the
integrated #56/#57 evidence revision. Before final freeze, the parent may pin
rusty-brain tasks to that clean integration commit and recompute their hashes;
no treatment observation may precede that freeze.

The candidate execution route is `litellm/deepseek-v4.1-flash`, with an
8,000-token common prompt budget, per the user's explicit provider switch.
The parent catalog confirms this configured route; catalog presence is not
an observed pilot invocation. The provider has not exposed an
immutable backend version: `model_version=provider-unexposed` records that
limitation, not a verified model revision. Both arms must use the same route,
version metadata, tool permissions and task budget; record any provider version
that becomes available before freeze. A requirement for an immutable backend
version would remain blocked on provider support, not operator attestation.

Store/namespace identifiers map to four separate private roots in the
[prepared store plan](2026-10-16-pilot-store-plan.json). The roots and configuration
files exist with owner-only permissions; databases are deliberately uninitialized
and no memories or credentials were copied. Runtime sockets have a short,
separate disposable root. Recreate that private runtime root before any future
admission if the OS removes it. Never point the study at installed dogfood data
or edit existing MCP/OMP configuration. Restore each task working directory from its
frozen commit and the same sanitized task fixture; retain treatment memories
within its own repository arm only. A treatment-first human may learn the task:
balanced order reports that residual crossover risk; separate fresh agent
sessions prevent machine-context leakage, not human unlearning.

If resource/provenance verification or operator availability misses the first
slot, stop before treatment, declare a new future fourteen-day interval and
rehash/re-freeze the scenario schedules. Do not backdate observations. Twelve
paired sessions, immediate interim aggregation after each, and one reviewed
backup/retention drill need the operator and independent reviewer during the
declared interval; no unattended future execution is installed by this change.

```bash
python3 scripts/dogfood-pilot.py validate --candidate \
  --manifest docs/eval/2026-07-12-phase5-dogfood-pilot-manifest.json \
  --snapshots "${HOME}/.local/share/rusty-brain-pilots/phase5-20261019/scenario-inputs.json"
```


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
     --manifest docs/eval/2026-07-12-phase5-dogfood-pilot-manifest.json \
     --snapshots "${HOME}/.local/share/rusty-brain-pilots/phase5-20261019/scenario-inputs.json"
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
  --snapshots "${HOME}/.local/share/rusty-brain-pilots/phase5-20261019/scenario-inputs.json" \
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
- schema version 2 model ID, reported version (or explicit provider-unexposed
  marker), and prompt-budget fields in both arms, equal to frozen execution;
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

Do not add raw production-content fields. Do not collect database rows, model
output, private prompts, transcripts, diffs, environment variables, API keys,
or production memory bodies. The deliberately synthetic preregistered task
wording and seed notes above are inputs, not captured private evidence. Do not
create a separate raw pilot artifact. Ordinary source-system data stays under
its existing local retention policy and is never copied into this study.

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
