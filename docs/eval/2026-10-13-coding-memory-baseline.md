# Coding-memory development baseline — #85

## Scope and evidence

Six authored development cases extend existing rb-eval; retrieval correctness is
separate from answer correctness. The real local `all-MiniLM-L6-v2` recorder
produced 19 document/query vectors. The strict offline baseline executed
successfully and wrote [the complete categorized results](2026-10-13-coding-memory-results.json).
Execution success does not mean every capability passed.

- Fixture: `crates/rb-eval/fixtures/coding_memory.json`.
- Fixture SHA-256: `42320615cbd5f781d0a7ffb531526b88813c89332a143928890414d910c7de80`.
- Recorded vectors: `crates/rb-eval/fixtures/embeddings/coding-memory-all-MiniLM-L6-v2.json`.
- Vector-file SHA-256: `489511cb2abf0a3ee9d8107a805efb86209340b765be6e7ea40fedebc2f73e79`.
- Answer quality: not evaluated; no generated answer or model judge.
- No paid provider, production memory, frozen threshold, or sealed holdout was used for these cases.

## Existing coverage inventory

#61's `assertion_precision.json` already covers exact evidence sets, duplicate and
extra-ID rejection, payroll-topic abstention, current-only supersession,
five-slot evidence, namespace selection and topic drift. #62 already provides
production abstention/reason codes. Existing `semantic_safety.rs` checks directly
available multiple facts, supersession and active conflicts with deterministic
vectors; it is not evidence of real-vector multihop or explicit prior history.
These contracts are reused, not replaced by another benchmark framework.

## Observed categorized results

| Case | Category | Result | Discriminating observation |
|---|---|---|---|
| Unanswered deployment region | Abstention | **Fail** | A forbidden deployment-adjacent note returned; no abstention reason. |
| Fully indexed decision + incident | Direct multiple facts | Pass | Both exact IDs returned; both available directly. Not multihop. |
| Controlled one-hop incident | Graph expansion | Pass | Incident absent from keyword/vector candidates, returned graph-only at distance one. |
| Controlled two-hop incident | Unsupported two-hop | **Fail** | Seed and bridge returned; distance-two incident missing. |
| Current versus prior decision | Temporal history | Pass | Active recall excludes ancestor; explicit history retains both with correct current/archived flags. |
| Near-identical opposing instructions | Conflict | Pass | Both active opposing IDs returned as contested; obsolete instruction excluded. |

The deployment-region case returned
`12470000-0000-4000-8000-000000000001` at score `0.455442`, although no source
states a region. This is a retrieval/abstention failure under the predeclared
contract, not an observed generated-answer hallucination. The two-hop case missed
`12470000-0000-4000-8000-000000000008`. Neither label nor threshold was changed to
turn these failures into passes.

Graph-only cases deliberately omit selected stored vectors while recording the
real document inputs. Independent direct-channel absence, graph distance and
returned-channel assertions establish a controlled expansion capability, **not**
a naturally occurring semantic candidate-cutoff result. A fully indexed example
is reported separately. History is the retained supersede timeline, not
point-in-time reconstruction.

## Reproduction

The new local-only recording and strict replay commands are bounded to these
inputs. Model initialization may download missing local model artifacts; the
baseline itself has no live-provider fallback.

```sh
cargo test --locked -p rb-eval --features record-local --test coding_memory record_coding_memory_local_vectors -- --ignored --exact --nocapture
cargo run --locked -p rb-eval --bin coding-memory -- --output docs/eval/2026-10-13-coding-memory-results.json
cargo test --locked -p rb-eval --test coding_memory
cargo test --locked -p rb-eval --test coding_memory replay_coding_memory_recorded_vectors -- --ignored --exact --nocapture
```

Recording and the baseline executable were actually run from an isolated
HOME/config/cache/working directory using their compiled binaries. Recording
passed; baseline exit zero means execution/report completion. The manual real
replay acceptance probe **failed** on the forbidden deployment-region evidence,
matching the published result. It remains an ignored capability probe rather
than silently becoming a passing quality gate. Default deterministic
ID/history/topology/conflict regressions passed in the integrated workspace suite.

## Report interpretation and limits

The JSON retains per-query expected/returned/missing/extra/duplicate and forbidden
IDs, exact-set verdicts, raw direct candidates, authored graph topology, returned
channel flags, abstention reasons and current/archived/contested history labels.
Category counts preserve unsupported behavior rather than averaging it away.

This is six-case development evidence, not LoCoMo compliance, production-scale
validation, answer quality, or a new release admission gate. No hooks, daemon
transport or background consolidation are exercised by this evaluator; those
paths have separate tranche verification. The deployment-region gap and
unsupported two-hop behavior remain visible for subsequent prioritization.
