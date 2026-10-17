# Bounded diversity selection — results (Vikunja #86)

- **Date:** 2026-10-17
- **Protocol:** [`2026-10-17-diversity-selection-preregistration.md`](2026-10-17-diversity-selection-preregistration.md),
  committed (`b0a7885`) before the arm was measured.
- **Code:** clean commit `049412b`, release build (first measured at `52a841b`; the review fixes changed no quality metric). The artifact is
  [`2026-10-17-diversity-selection-results.json`](2026-10-17-diversity-selection-results.json).
- **Inputs:** committed corpus (72 golden and 20 held-out queries, 8
  near-duplicate clusters, 6 contradiction pairs) and the strict committed
  real-vector replay fixture (`all-MiniLM-L6-v2`); five registered seeds,
  averaged. No model calls; $0 API cost.
- **Reproduce:** `cargo test --release -p rb-eval --test diversity_selection -- --ignored --nocapture`

## Decision: defer

MMR (λ = 0.7) over Linear's own top-10 candidates fails two of the six
preregistered held-out conditions: it gives **no** duplicate-coverage gain and
it **lowers** NDCG@5 by more than the allowed 0.01. Recall@5 and MRR are
unchanged on the held-out set, nothing relevant or contradictory is silently
dropped there, and the step is cheap. The arm stays eval-only; nothing in
production changes.

## Held-out set (decision set, 20 queries)

| Metric | Linear | MMR | Δ | Rule | Pass |
|---|---:|---:|---:|---|:---:|
| Dedup precision@5 | 0.970 | 0.970 | 0.000 | ≥ +0.02 | no |
| Recall@5 | 0.975 | 0.975 | 0.000 | ≥ −0.01 | yes |
| NDCG@5 | 0.950 | 0.935 | −0.015 | ≥ −0.01 | no |
| MRR | 0.950 | 0.950 | 0.000 | reported | — |
| Injected tokens (sum over queries) | 5,272 | 5,230 | −42 | reported | — |
| Relevant drops (5 seeds) | — | 0 | — | = 0 | yes |
| Contradiction drops (5 seeds) | — | 0 | — | = 0 | yes |
| MMR step p99 (worst seed) | — | 57 µs | — | ≤ 1 ms | yes |

## Golden set (72 queries, reported, not the decision set)

| Metric | Linear | MMR | Δ |
|---|---:|---:|---:|
| Dedup precision@5 | 0.947 | 0.961 | +0.014 |
| Recall@5 | 0.963 | 0.956 | −0.007 |
| NDCG@5 | 0.954 | 0.940 | −0.013 |
| MRR | 0.984 | 0.983 | −0.001 |
| Injected tokens (sum over queries) | 19,684 | 19,639 | −45 |
| Relevant drops (5 seeds) | — | 5 | — |
| Contradiction drops (5 seeds) | — | 0 | — |

## What this means

1. **There is little redundancy left to remove.** Linear already places at
   most one near-duplicate cluster member in nearly every budgeted top-5
   (dedup precision 0.97 held-out, 0.95 golden). Write-time near-duplicate
   suppression and consolidation already do most of this work.
2. **When MMR does act, it mostly reorders.** The NDCG loss comes from moving
   a relevant row below a less relevant but dissimilar one. On the golden set
   it also pushed 5 expected rows (over 5 seeds × 72 queries) out of the
   5-row budget: related-but-distinct supporting facts that look similar to a
   row already chosen. That is the silent-loss mode #65 must avoid.
3. **Token savings are negligible** (under 1%), because rows are similar in
   size; diversity is not a lever for #65's token-budget goal.
4. **Contradictions are safe either way.** Neither arm splits an authored
   contradiction pair; #65's conflict resolution remains a separate concern.

## Guidance for #65

Do not add a read-time MMR stage. If #65 needs redundancy control, prefer
something that cannot demote a relevant row: for example, drop only rows inside
the same authored or consolidation cluster, or pack by marginal utility under
the token budget. Either change would need its own preregistered gate. A
different λ was not tried and should not be: λ was fixed in advance, and
choosing it after seeing these results would be tuning on the held-out set.

## Caveats

- Latency is in-process on one shared machine (Apple M4 Pro) while the #60
  benchmark replay was running. MMR-arm p50/p99 are percentiles of each
  query's own recall time plus its MMR step (nearest-rank, as for Linear).
- PR review found two implementation issues, fixed before this artifact: the
  redundancy max clamped all-negative similarities to 0 instead of using the
  preregistered true max, and arm latency summed independent percentiles.
  Neither changed any quality metric or loss count.
- Relevance inside MMR is Linear's rank, not its fused score, because fused
  scores are not exposed per key. That proxy was declared in advance.
- The corpus is small (205 memories). The result says MMR does not pay off at
  this redundancy level, not that diversity can never help at larger scale.
