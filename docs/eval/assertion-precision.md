# Assertion-grade retrieval precision (task #1218)

**Instrument implemented; capability gate NOT met.** On production revision
`d98de4c4f093860ea16a827c83445fed25267748`, **2/6 cases pass**
(`score = 0.3333333333333333`). The executable exits **1**, not a success/skip.
No production engine, store, search, embedding, or type code was changed.

**No LLM or model judge is used.** No answer generation, prose substring matching,
remote API, model download, or external service participates in the evaluation.
Expected memory ID sets are fixed, locally authored labels, not inferred from
retrieval output. This is a bounded retrieval assertion, not semantic quality or
agent task-success evidence.

## Run offline

From the repository root (with the locked Rust dependencies already cached):

```bash
# JSON to stdout on either a passing or an unmet precision gate.
cargo run --offline --locked --quiet -p rb-eval --bin assertion-precision \
  > /tmp/assertion-precision.json
# Exit: 0 = all cases pass; 1 = precision gate unmet; 2 = execution/report error.

# Instrument correctness, deliberately independent of capability success:
cargo test --offline --locked -p rb-eval --test assertion_precision

# Direct capability assertions, including failures on the current engine:
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

- A query passes only when **returned ID set == expected ID set**, with no duplicate
  returned IDs. One extra memory fails even when every expected memory is present.
- Empty expected sets require empty output. Missing expected IDs fail too.
- Returned IDs are taken directly from engine results, with no fixture-key lookup
  that might silently discard an unknown/extra ID.
- A case passes only when **every** query in that case passes.
- The score is `fully passing cases / total cases`, not mean per-query recall.
  No partial credit, judge interpretation, tolerance, or threshold calibration.
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

The task's stronger gate asks for demonstrated baseline failures and subsequent
fixes for **supersede resurfacing, budget eviction, and namespace leakage**.
That gate is **not satisfied**: only budget eviction reproduces among those three;
supersede resurfacing and namespace leakage do not reproduce in these cases.
No corresponding production fixes were made or claimed. Existing passing checks
remain regression probes, not evidence of a red-to-green fix. Do not manufacture a
failure or change a label to make the desired story true.

### Fixes still needed / investigation targets

- **Precision/abstention:** the default retrieval path admits hard negatives and
  an unrelated memory even for the empty-evidence query. Investigate query-aware
  relevance/admission and an evidence-aware abstention gate, not corpus-specific
  ID/text exclusions. A score-floor change alone needs separate recall validation.
- **Budget eviction:** same-topic, higher-importance session chatter consumes
  slots required by operational facts. Investigate source/topic-sensitive ranking
  or denoising before the fixed top-five selection; do not enlarge the budget or
  suppress returned extras in the evaluator.
- **Topic drift:** session provenance does not stop previous-topic evidence and
  session chatter from contaminating later recall. Validate a general topic/noise
  policy against the same frozen queries and independent holdouts.
- **Supersede and namespace:** no defect/fix is established by these passing
  probes. Broader independently motivated cases may be added, but the existing
  successful results must not be relabeled as failures.

These are capability investigation targets, not verified fixes. Passing this small,
nonsemantic corpus would still not establish semantic generalization.

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

`report-d98de4c.json` is the executable's actual JSON output, exit **1**.
`provenance.json` records the revision, host, fixture/report hashes and production
source comparison. The report has no hardcoded claim about the running Git SHA;
use the provenance sidecar for this archived run, and capture provenance again
for future runs.

`crate-tests.log`: **109 passed, 0 failed, 14 ignored** (five new opt-in capability
class tests are among the ignored tests). These are instrument/regression results,
not a green precision gate. The initial `clippy.log` found a test-only explicit
panic lint; `clippy-green.log` records the corrected final strict check.

## Legacy scorecard mapping: conceptual overlap only

Do **not** replace `scripts/memory-scorecard.sh` or its substring proxy on the
strength of this fixture. Equivalent legacy coverage is not achieved:

- The four `fresh-*` scenarios share the supersede/freshness concept with
  `supersede-chain`, but their HTTP/config/ID/test-runner facts and downstream
  agent tasks are not replayed here.
- `scale-http-buried`, `scale-id-type-buried`, and `scale-wire-format-buried` have
  500/1,000-distractor scale arms. The eleven-belief budget case is not scale
  coverage or an equivalent agent/context-budget experiment.
- The three `cap-*` scenarios exercise auto-capture; this fixture seeds beliefs
  directly and has no capture coverage.
- The three `reach-*` scenarios exercise agent/tool reach and downstream tasks;
  direct engine recall is not equivalent coverage.

The two instruments answer different questions. This one provides exact-ID
retrieval evidence with an honest failing gate; the legacy script remains a
prose-token proxy with its existing limitations.
