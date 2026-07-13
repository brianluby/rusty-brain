# W4.1 production-embedding semantic gate results

Preregistrations:
[`2026-07-12-w41-semantic-gate-preregistration.md`](2026-07-12-w41-semantic-gate-preregistration.md)
and
[`2026-07-12-w41-controlled-arms-preregistration.md`](2026-07-12-w41-controlled-arms-preregistration.md).

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
distractors; dedup meets its floor. The ≥5k noise/resource variant and p50/p99
resource envelopes belong to the separate scale/concurrency task.

## Controlled retrieval arms

The controlled report evaluated every arm over five preregistered chronological
streams. All values below are five-seed means over the untouched holdout, with
the same 5-row/800-token whole-row prompt budget.

| Arm | recall@5 | MRR | NDCG@5 | dedup@5 | exact span | answer | stale exposure | wrong answer | tokens |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Linear | 0.9750 | 0.9500 | 0.9498 | 0.9700 | 0.9000 | 0.9000 | 0.0600 | 0.1000 | 5237.0 |
| RRF | 0.9500 | 0.8583 | 0.8493 | 0.9700 | 0.9000 | 0.7500 | 0.0400 | 0.2500 | 5325.0 |
| exact evidence | 0.9500 | 0.9500 | 0.9413 | 0.9700 | 0.9000 | 0.9000 | 0.0500 | 0.1000 | 5525.0 |
| recency only | 0.0433 | 0.0252 | 0.0235 | 1.0000 | 0.0300 | 0.0100 | 0.0800 | 0.9900 | 5296.0 |
| importance only | 0.0167 | 0.0167 | 0.0058 | 1.0000 | 0.0000 | 0.0000 | 0.0000 | 1.0000 | 5575.0 |

Every arm had zero poison exposure and 1.0 contested disclosure. The bounded
exact-evidence lane is **NO-GO**: it did not improve exact-span coverage or
answer accuracy, and recall regressed by 0.025, beyond the allowed 0.01. The
small stale-exposure improvement does not override that failure. Recency-only
and importance-only remain diagnostic baselines, not default candidates.

The controlled stale label is deliberately stricter than the engine state. It
derives authored truth from fixture contexts that declare a current reversal,
even though those predecessor rows remain active in the frozen corpus. Thus the
0.06 Linear value exposes a fixture-state gap; it does not contradict the
engine safety test showing zero exposure for rows actually archived or
superseded in storage.

## Online shadow-admission arms

Each arm saw the same 205 rows plus five labeled instruction-poison probes in
each stream, with 128-row/32,768-byte caps. Holdout values are five-seed means.

| Arm | recall@5 | MRR | NDCG@5 | dedup@5 | exact span | answer | relevant retained | rows | bytes | poison rows |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| novelty only | 0.5917 | 0.7150 | 0.6207 | 1.0000 | 0.6500 | 0.6900 | 0.5310 | 128 | 26440.2 | 1 |
| importance-confidence | 0.7917 | 0.9000 | 0.8101 | 0.9800 | 0.8000 | 0.8500 | 0.8621 | 128 | 27197.0 | 0 |
| combined product | 0.8217 | 0.9300 | 0.8456 | 0.9900 | 0.8600 | 0.8800 | 0.8759 | 128 | 27107.0 | 0 |

All three arms retained zero authored-stale rows, had zero stale/poison query
exposure, and disclosed every returned contested row. Novelty-only nevertheless
retained one poison probe in every seed. The combined product is **GO for a
later bounded pilot shadow experiment**: it is within 0.01 of (and in this run
exceeds) the better component on recall/MRR/NDCG, retains/exposes no poison or
stale row, preserves contested disclosure, respects both caps, and reduces the
205-row corpus to 128 rows (37.6%). This is evidence for an experiment only;
it does not authorize a production retention or admission change.

## Reproduction

```bash
cargo test -p rb-eval --test semantic_gate \
  production_embedding_linear_gate_passes_goldens_and_untouched_holdout \
  -- --nocapture

cargo test -p rb-eval --test semantic_gate \
  five_seed_linear_rrf_diagnostic -- --ignored --nocapture

cargo test -p rb-eval --test semantic_safety

cargo test -p rb-eval --test controlled_arms \
  controlled_retrieval_and_admission_arms_report_every_seed \
  -- --ignored --nocapture
```
