# Assertion-grade retrieval precision (task #1218)

**Required exact-ID gate met.** The gate has three fixed cases:
`supersede-chain`, `five-slot-budget`, and `namespace-selection`. All three pass
through the production retrieval path. The unchanged budget fixture is the sole
demonstrated baseline-red/post-fix-green case; supersede and namespace were
already green and remain regressions rather than manufactured baseline defects.

The broader capability score remains honestly **3/6** (`score = 0.5`):
hard-negative admission and topic-drift cases are still red, are emitted in the
same report, and set `all_cases_passed = false`. They are not averaged into the
required gate and are not silently called passing.

**No LLM or model judge is used.** No answer generation, prose/file substring
matching, remote API, model download, or external service participates in the
gate. Expected memory ID sets are fixed, locally authored labels, not inferred
from retrieval output. This is a bounded retrieval assertion, not semantic
quality or agent task-success evidence.

## Run offline

From the repository root (with the locked Rust dependencies already cached):

```bash
# JSON to stdout on either a passing or an unmet required gate.
cargo run --offline --locked --quiet -p rb-eval --bin assertion-precision \
  > /tmp/assertion-precision.json
# Exit: 0 = all required cases pass; 1 = a required case fails; 2 = execution/report error.

# Instrument correctness, deliberately independent of capability success:
cargo test --offline --locked -p rb-eval --test assertion_precision

# Required exact-ID regressions (budget plus supersede/namespace invariants):
cargo test --offline --locked -p rb-eval --test assertion_precision_baseline

# Opt-in unresolved hard-negative and topic-drift probes:
cargo test --offline --locked -p rb-eval --test assertion_precision_baseline \
  -- --ignored --nocapture --test-threads=1

# Entire eval crate, not the entire workspace:
cargo test --offline --locked -p rb-eval
```

The executable accepts no configuration for expectations or scoring thresholds.
Do not swallow exit 1 or call a successful instrument test a passing capability
score. Dependency caching is a build prerequisite; the resulting executable has
no network/model/provider requirement. It does not start the daemon or touch the
user's memory database.

## Frozen corpus and criterion

[`assertion_precision.json`](../../crates/rb-eval/fixtures/assertion_precision.json)
contains 27 fictional beliefs, six independently seeded cases, and seven queries.
Each belief has an explicit UUID, namespace, content, importance, and optional
session provenance. Each query has an explicit expected UUID set and five-result
limit. Cases reset storage; sequential queries inside the topic-drift case share
one store. The clock is pinned to `2026-01-01T00:00:00Z`.

Each case declares `gate_required`. The required set is fixed to the revised
ticket gate (budget eviction plus the supersede and namespace regressions);
hard-negative and topic-drift cases remain scored, visible investigation cases.

- A query passes only when **returned ID set == expected ID set**, with no duplicate
  returned IDs. One extra memory fails even when every expected memory is present.
- Empty expected sets require empty output. Missing expected IDs fail too.
- Returned IDs are taken directly from engine results, with no fixture-key lookup
  that might silently discard an unknown/extra ID.
- A case passes only when **every** query in that case passes.
- The all-case score is `fully passing cases / total cases`, not mean per-query
  recall. The required gate is separately `passed required cases == required
  cases`, with at least one required case. Neither calculation grants partial
  credit, judge interpretation, tolerance, or threshold calibration.
- The report includes ranked returned IDs, missing/extra/duplicate IDs, actual
  production scores and channel attribution, per-query/per-case verdicts, fixture
  SHA-256, explicit no-judge statement, and measurement limits.

Beliefs, labels and queries were frozen before the baseline was run. The only
pre-run correction was the session namespace's required syntax; no expectation
was weakened or changed after seeing output. Afterward, the `authorship` metadata
was clarified to say **no runtime model**: this coding-agent-authored fixture does
not claim human-only authorship. That metadata-only edit changes the file hash,
not any belief, query, expectation or outcome.

## Actual production path

`MemoryEngine::compose_note` performs production heuristic enrichment and
`DeterministicProvider(dim=32)` document embedding. The harness replaces only the
fixture UUID, namespace and session metadata, then calls the existing
`SqliteBackend::write` -> `SqliteStore::insert_memory`. Links are authored reference
edges through `add_link`; supersede chains use `SqliteStore::supersede` through the
existing `supersede_for_eval` bridge.

Retrieval calls `MemoryEngine::recall_with_status` with the default metadata filter,
linear fusion weights and score floor. It executes real SQLite FTS5, sqlite-vec,
scoped graph expansion, ranking and truncation. Degraded retrieval is an execution
error, not silently accepted evidence. There is no shadow retrieval implementation,
precomputed result list, custom vector table, or altered production ranking.

The deterministic embedding provider is **not semantic**. Its normal production
implementation is used unchanged; these results must not be generalized to a
semantic model. Ingestion deliberately uses compose + store rather than remember's
implicit similarity-link generation so graph topology is explicitly authored.
This does not test auto-linking, daemon writer concurrency, transport, capture,
consolidation, authorization, or agent response quality. Five returned slots are
exercised; the hooks' 200-character rendering projection is not.

## Observed baseline, not requested failures invented after the fact

| Case | Expected evidence | Observed result |
|---|---|---|
| `hard-negative-environment` | Production health endpoint only | **FAIL:** also returns development endpoint (`...002`). |
| `hard-negative-unknown` | Empty: no payroll-retention belief exists | **FAIL:** returns coffee-grinder note (`...004`). |
| `supersede-chain` | Latest timeout (`...007`) only | **PASS:** neither ancestor resurfaces, including authored graph back-links. |
| `five-slot-budget` | All five deploy requirements (`...008`–`...012`) | **FAIL:** migration check (`...010`) and backup snapshot (`...011`) are evicted by dashboard and desk-lamp session notes (`...014`, `...018`). |
| `namespace-selection` | Bound project's endpoint (`...019`) only | **PASS:** excludes matching other-project, global and session rows, including cross-scope reference links. |
| `session-topic-drift` | Auth policy, then storage policy, each alone | **FAIL:** both queries include other-topic/session notes. |

Namespaces here are **selection scopes, not authorization boundaries**. The
cross-scope links are seeded through the store to test defensive retrieval, not
claimed to be creatable through the namespace-bound engine link API.

The revised task gate requires demonstrated **budget eviction** on the baseline
and the unchanged fixture to pass after the retrieval fix. Supersede-chain
exclusion and namespace isolation remain required passing regression invariants;
no missing trace is relabeled as a baseline failure. The corresponding task
comment records the evidence audit and decision.

The archived baseline has the two missing requirement IDs and two extra session
IDs; the current exact-ID regression returns only `...008`–`...012`. The
permanent `budget_eviction` test records this red-to-green behavior. Supersede
and namespace are also permanent exact-ID regressions.


### Production-shaped wire probes

`crates/rb-daemon/tests/daemon_e2e.rs` separately runs two deterministic,
offline daemon probes through the public Unix-socket protocol:

- three `Remember` operations linked through the actual `supersedes` request
  field, followed by `Recall`, must return only the current decision;
- two clients with identical records in different handshake namespaces cannot
  link across that boundary, and local `Recall` must return only the local ID.

These exercise the daemon's writer, handshakes, `Request::Remember`,
`Request::Recall`, and namespace checks instead of the evaluator bridge. They
are regression probes, not a manufactured baseline: a pass means the tested
production path did not reproduce that defect class and therefore cannot count
as the task's required red-to-green evidence.

### Trace availability

The only identified real operational antecedent is Vikunja task `#50`: two
memory-on scorecard runs answered an obsolete command despite the store holding
the supersede chain. Its retained record explicitly says session logs were
deleted and the mechanism was not established: archived recall, a missed
current head, ignored current evidence, and no recalled evidence remain
distinct possibilities. It cannot serve as a red baseline for *superseded
value resurfacing*.

The committed `session_replay` fixtures are invented/sanitized parser examples,
not corresponding production retrieval traces. No retained trace demonstrates
a namespace leak. A qualifying gate baseline must preserve a reviewed,
sanitized request sequence and store state sufficient to replay the observed
wrong ID set; a current engine or wire-probe pass cannot be relabeled as such
a failure.


### Fixes still needed / investigation targets

- **Precision/abstention:** the default retrieval path admits hard negatives and
  an unrelated memory even for the empty-evidence query. Investigate query-aware
  relevance/admission and an evidence-aware abstention gate, not corpus-specific
  ID/text exclusions. A score-floor change alone needs separate recall validation.
- **Budget eviction:** fixed by a source-quality prior for `session_id` memories
  plus a bounded lower floor for durable FTS hits. The source prior changes
  ordering, not relevance eligibility: session admission floors scale with the
  same multiplier as their scores. This prevents ordinary captured decisions
  from disappearing while retaining the prior-only rejection gate. The separate
  durable-FTS floor remains at the prior-only ceiling.
- **Topic drift:** source ranking does not solve query-aware admission; the
  unchanged topic-drift case still fails. Validate admission changes against
  the frozen queries and independent
  holdouts.
- **Supersede and namespace:** no defect/fix is established by these passing
  probes. Broader independently motivated cases may be added, but the existing
  successful results must not be relabeled as failures.

These are capability investigation targets, not verified fixes. Their failures
remain in the report and keep `all_cases_passed` false. Passing the required
small, nonsemantic corpus still does not establish semantic generalization.

## TDD and retained evidence

All artifacts are under
[`crates/rb-eval/evidence/assertion-precision/`](../../crates/rb-eval/evidence/assertion-precision/).

1. Authored the fixture and direct real-engine capability assertions first.
   `baseline-d98de4c.log`: the explicit ignored-test run exited **101**, with
   **3 failing class tests and 2 passing class tests**. The hard-negative class has
   two cases; topic drift has two queries. This is the real behavioral RED before
   the evaluator existed, not a compilation failure or fake retrieval result.
2. Added a compiling executable-contract test before implementing the executable.
   `instrument-red.log`: runtime assertion failure because there was no executable
   target/JSON report. Implemented the evaluator; `instrument-green.log` passes.
3. Required production score/channel evidence in the report before implementing
   those fields. `channel-red.log` fails at the absent evidence assertion;
   `channel-green.log` passes after adding actual engine attribution.
4. Added independent set-comparison tests before exposing the scorer helper.
   `set-api-red.log` records the **missing-API compilation failure** (not claimed
   as capability evidence); extracted the existing comparison into the helper,
   then `set-api-green.log` passes. Tests include unknown extras, missing IDs,
   empty sets, order invariance and duplicate output.
5. Added fixture-label validity and repeated real-engine run checks. These validate
   existing behavior and pass without forced failures. The subprocess test now
   invokes Cargo's built binary directly, avoiding nested build jobs.
6. Promoted budget eviction, supersede exclusion, and namespace isolation to
   permanent regressions. Only budget has archived red-to-green evidence. The
   production fix applies a session source-quality prior and durable-FTS
   admission floor without changing fixture expectations or the five-result
   budget.

`report-d98de4c.json` is the executable's actual baseline JSON output, exit
**1** under the original all-case gate. `provenance.json` records its revision,
host, fixture/report hashes, and production-source comparison. Schema 2 adds
gate-scope metadata without rewriting that archived output; current runs emit
the required/all-case split directly.

`crate-tests.log` is historical instrument evidence from the original capture,
not current validation. The initial `clippy.log` found a test-only explicit
panic lint; `clippy-green.log` records the corrected historical check.

## Legacy scorecard mapping: shared OMP scenario source

`scripts/memory-scorecard.sh` now replays the legacy scorecard’s exact thirteen
scenario rows through OMP’s native extension:

| Legacy dimension | Rows | OMP scorecard treatment |
|---|---:|---|
| Freshness | 4 | Explicit supersede chains and stale/current baseline contrast. |
| Retrieval at scale | 3 | The original 500/1,000-distractor corpus arms. |
| Auto-capture | 3 | The original plant-session and shutdown-capture rows. |
| Reach | 3 | The original identity-A-to-B rows. |

The shared source is
[`memory_scorecard_scenarios.json`](../../crates/rb-eval/scorecard/memory_scorecard_scenarios.json);
the runner's deterministic self-test checks those dimension counts. Migration
renamed runner-specific baseline fields from `*_claude_md` to `*_agents_md`
while retaining the scenario rows. Shared inputs do not establish equivalent
host lifecycle, model behavior, or assertion coverage: full OMP scorecard
outcomes require a separate live run.

The fixture is deliberately **not** an agent-task judge. The live scorecard
executes `assertion-precision` once, writes `RESULTS.tsv.assertions.json`,
independently recomputes every exact set/case verdict, verifies the fixed three
required case IDs and `judge_used = false`, and uses that result as its hard
gate. A malformed or missing report fails closed.

The five live arms remain represented, including memory-off and the
length-matched placebo with their per-arm injection estimates. Their historical
substring metric is explicitly report-only, excludes assistant prose, and is
unsupported as task-correctness evidence; it cannot produce `ASSERTION-PASS`.
Auto-capture, model response, identity/reach outcomes, and causal MIE attribution
remain outside this fixture rather than being silently declared successful.
