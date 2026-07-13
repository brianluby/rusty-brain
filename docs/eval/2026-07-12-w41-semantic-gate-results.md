# W4.1 production-embedding semantic gate results

Preregistration:
[`2026-07-12-w41-semantic-gate-preregistration.md`](2026-07-12-w41-semantic-gate-preregistration.md)

Outcome: **GO for bounded dogfood with the current Linear fusion; NO-GO for an
RRF default flip.** This gate does not waive the separate scale/resource gate.

## Frozen inputs

- 205-memory authored corpus, 72 graded golden queries, 8 dedup clusters.
- Untouched 20-query graded holdout; aggregates only.
- Supported local `all-MiniLM-L6-v2`, dimension 384.
- Re-recorded fixture SHA-256:
  `716dd5edeb275d73d553ebbfdd59a4404ac7db03dae4f214765e40fa1e2ba818`.
- 297 exact-kind vectors: 205 `document`, 92 `query`.
- Zero query/document fallbacks and zero network/provider requests in replay.
- `VOYAGE_API_KEY` was unavailable in the execution environment, so the
  optional Voyage comparison was not run and incurred no spend.

## Primary gate

| Set | recall@5 | MRR | NDCG@5 | dedup@5 | FTS query rate | vector query rate | graph query rate |
|---|---:|---:|---:|---:|---:|---:|---:|
| floor | 0.8000 | 0.7000 | 0.7500 | 0.9000 | 0.8000 | 0.9500 | 0.0000 |
| golden | 0.9630 | 0.9838 | 0.9537 | 0.9472 | 1.0000 | 1.0000 | 0.0278 |
| holdout | 0.9750 | 0.9500 | 0.9498 | 0.9700 | 1.0000 | 1.0000 | 0.1000 |

Strict replay failed on any missing `(model, input_kind, text hash)` entry and
the run observed zero fallbacks. The committed non-ignored test now enforces
these floors in ordinary CI; the weekly/manual workflow repeats it offline.

Diagnostics from the local run (not portable gates): golden p50/p99 recall
latency was about 22/54 ms and holdout about 21/37 ms; 711/200 rows and about
155/43 KB of result content were returned across the respective query sets.
Fixture size was about 988 KB. Offline replay provider cost was $0.

## Five-instant Linear versus RRF decision

Quality metrics were bit-identical at the five preregistered monthly instants;
only machine-dependent latency moved.

| Mode/set | recall@5 | MRR | NDCG@5 | dedup@5 |
|---|---:|---:|---:|---:|
| Linear golden | 0.9630 | 0.9838 | 0.9537 | 0.9472 |
| RRF golden | 0.9167 | 0.8449 | 0.8309 | 0.9389 |
| Linear holdout | 0.9750 | 0.9500 | 0.9498 | 0.9700 |
| RRF holdout | 0.9500 | 0.8583 | 0.8493 | 0.9700 |

RRF clears the absolute floors, but violates the preregistered no-regression
rule: golden MRR/NDCG fall by about 0.139/0.123 and holdout MRR/NDCG by about
0.092/0.100. Therefore Linear remains the dogfood/default fusion. No weights,
thresholds, or production behavior changed after seeing the holdout.

## Offline robustness strata

Dedicated offline tests additionally established:

- exact operational fact retrieval returns the literal answer span;
- one semantic question returns both required memories;
- archived and superseded memories have exactly zero default recall exposure;
- both sides of an active contradiction are returned with `contested=true`;
- a low-confidence instruction-shaped poison does not outrank the correct fact.

The 205-memory primary corpus supplies authored near-duplicates and ordinary
distractors; dedup meets its floor. Exact-evidence, recency-only,
importance-only, novelty, and alternate-confidence arms are not separately
calibrated production modes in this result and remain `not_measured`, not pass.
The ≥5k noise/resource variant and p50/p99 resource envelopes belong to the
separate scale/concurrency task.

## Reproduction

```bash
cargo test -p rb-eval --test semantic_gate \
  production_embedding_linear_gate_passes_goldens_and_untouched_holdout \
  -- --nocapture

cargo test -p rb-eval --test semantic_gate \
  five_seed_linear_rrf_diagnostic -- --ignored --nocapture

cargo test -p rb-eval --test semantic_safety
```
