# Bounded diversity selection — preregistration (Vikunja #86)

- **Date:** 2026-10-17, committed before any measurement of the arm below.
- **Supports:** #65 (candidate denoise and token-budget packing). This is a
  shadow experiment, not a competing pipeline: no production ranking, weight,
  gate or default changes.
- **Base:** `main` at `a1ec930`.

## Question

Given the **same candidate set and budget** that production Linear recall
already produces, does a minimal deterministic Maximal Marginal Relevance
(MMR) reselection step put more distinct, useful information into the
5-row/800-token prompt budget, without dropping relevant facts or one side of
a contradiction?

## Current implementation (checked first)

- Recall is one Linear fusion pass (`rb-search`), with `RECALL_LIMIT = 10`
  candidates in the eval runner. The prompt budget is whole-row:
  `PROMPT_ROW_BUDGET = 5` and `PROMPT_TOKEN_BUDGET = 800` (`controlled.rs`).
- There is **no** read-time diversity or MMR step. Write-time near-duplicate
  suppression (W3.1) and the consolidation job exist but do not run at recall.
- Existing evidence is reused, not added: the committed corpus's 8
  near-duplicate clusters and 6 contradiction pairs, graded golden queries, the
  held-out set, and the strict committed real-vector replay fixture
  (`all-MiniLM-L6-v2`). No new diversity case is added.

## Arms

| Arm | Definition |
|---|---|
| `linear` | Production Linear recall under strict real-vector replay, top-10 candidates, then the shared whole-row budget. Unchanged. |
| `mmr` | The **same** top-10 Linear candidates per query, reselected greedily by MMR, then the same shared budget. |

MMR is fixed here and not tuned:

- The score for candidate `d`, given the already-selected set `S`, is
  `λ · rel(d) − (1 − λ) · max_{s ∈ S} cos(d, s)`. The `max` over an empty `S`
  is `0`.
- `λ = 0.7`.
- `rel(d) = (n − r) / n`, where `r` is `d`'s zero-based Linear rank and `n` is
  the candidate count. This keeps Linear's fused keyword + vector + graph order
  as the relevance signal, so the arm isolates diversity instead of switching
  to vector-only relevance. Fused scores are not exposed per key, so rank is
  the declared proxy.
- `cos` is cosine similarity between the stored composite document vectors
  (`embedding_input` of each stored note), served from the strict replay
  fixture. A missing vector is a hard error, never a fallback.
- Ties break by Linear rank. Selection is fully deterministic.

## Metrics (all reported, aggregate only)

Recall@5, MRR, NDCG@5, dedup precision@5 (duplicate coverage), injected
rows/bytes/tokens, and p50/p99 latency (Linear recall plus the MMR step,
measured in-process on a shared machine, so diagnostic only). Two silent-loss
counts are also reported:

- **Relevant drops:** expected keys present in Linear's budgeted rows but
  absent from MMR's, summed over queries.
- **Contradiction drops:** queries where Linear's budgeted rows hold both
  members of an authored contradiction pair and MMR's hold only one.

Both sets (golden and held-out) and all five registered `STREAM_SEEDS` are
run, and values are averaged across seeds. Held-out results are aggregate-only
and are never used to change `λ`, the relevance proxy, the pool or the budget.

## Decision rule (fixed)

**Adopt** MMR as the diversity component for #65 only if all of these hold on
the held-out set:

1. Dedup precision@5 improves by at least 0.02 absolute.
2. Recall@5 and NDCG@5 each fall by no more than 0.01 absolute.
3. Relevant drops and contradiction drops are both 0.
4. The MMR step adds no more than 1 ms at p99.

Otherwise **defer**, recording which condition failed. Either way the arm stays
eval-only in this experiment. Any production adoption is #65's change, behind
its own gate.
