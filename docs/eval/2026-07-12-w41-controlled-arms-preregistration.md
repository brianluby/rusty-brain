# W4.1 controlled offline arms preregistration

Status: **frozen before executing the controlled retrieval/admission arms** on
2026-07-12. This supplements, and does not revise, the production-embedding
preregistration.

## Equal inputs and budgets

- Retrieval arms use the same frozen 205 memories, 72 goldens, untouched
  20-query holdout, relevance grades, and dedup clusters as the primary gate.
- Every retrieval arm emits at most 5 rows and at most 800 estimated prompt
  tokens per query. The estimator is `ceil(UTF-8 content bytes / 4)`; rows are
  accepted in rank order until the next whole row would cross the budget.
- Holdout output remains aggregate-only.
- Shadow admission arms see the identical 205-row corpus plus the same five
  labeled instruction-shaped poison probes. They share a 128-row and 32,768
  content-byte active-set cap.
- Nothing writes production storage, changes retention, or changes ranking
  weights/defaults.

## Retrieval arms

1. **Linear** and **RRF**: current engine rankings, truncated only by the common
   row/token budget.
2. **Bounded exact-evidence lane**: begin with Linear's top 5. Normalize
   lowercase alphanumeric tokens across content, summary, keywords, tags, and
   context; discard the fixed stoplist in code. A corpus-wide candidate is
   eligible with at least 2 distinct query-token matches. At most 2 candidates
   may replace selected rows, and only when its lexical count exceeds the
   displaced row by at least 1. An exact typed-anchor match would independently
   qualify with margin 1; the frozen `FixtureMemory` schema has no anchor field,
   so anchor contribution is structurally 0 and no anchor is invented.
3. **Recency-only**: newest-first according to each fixed chronological stream
   order, then stable key. Corpus rows have no authored timestamps and the
   production harness deliberately stamps all rows at one fixed instant, so a
   shadow stream position is the smallest faithful way to measure this
   baseline.
4. **Importance-only**: authored importance descending, then confidence
   descending, then stable key.

Per arm/set/seed report: recall@5, MRR, NDCG@5, dedup precision@5,
exact-span coverage, top-1 answer accuracy, stale/wrong/poison exposure,
contested disclosure, injected rows/tokens/bytes, and rank p50/p99.

An exact span is counted when a returned expected row contains at least two
distinct normalized non-stop query tokens. Wrong exposure is a top-1 key not
in the authored expected set. Stale keys are targets named by an authored
`supersedes <key>` context. Contested keys are both endpoints named by an
authored `contradicts <key>` context.

## Online shadow admission arms

For each arriving row, novelty is computed only against the previously
retained active rows: `1 - max(token-set Jaccard)`, with novelty 1 for an empty
active set. The three preregistered scores are:

- `novelty_only = novelty`;
- `importance_confidence = (importance / 10) * confidence`;
- `combined = novelty * (importance / 10) * confidence`.

The candidate is inserted, authored supersede targets already active are
removed, then the lowest admission score (stable key tie-break) is evicted
until both caps hold. This is an online bounded policy: it never consults a
future row or a future query. After the stream, every arm is queried with the
same exact-evidence ranker and 800-token/5-row budget.

The five poison probes are low-confidence strings shaped like instructions to
ignore policy, reveal a deployment token, run a command, fetch a URL, or copy a
secret. They are evaluation data only and are labeled, never executed.

Admission output reports the retrieval metrics above plus retained relevant
key recall, rows/bytes, dedup precision, stale/poison rows retained, and
per-decision latency p50/p99.

## Five fixed chronological permutations

Seeds are `20260101`, `20260201`, `20260301`, `20260401`, and `20260501`.
For each seed, a Kahn topological stream order uses SHA-256 of `(seed, key)` as
the ready-node priority. Authored supersede targets must precede their
replacement. Every reported seed must contain every corpus/probe key exactly
once; a cycle or missing key fails closed.

## Preregistered go/no-go

- **Exact lane GO** only if holdout exact-span coverage or answer accuracy
  improves by at least 0.01 over Linear, no holdout recall/MRR/NDCG/dedup metric
  regresses by more than 0.01, and stale/poison exposure does not increase.
- **Combined admission GO for a later pilot experiment** only if, averaged over
  all five seeds, its holdout recall/MRR/NDCG is within 0.01 of the better
  component arm, poison retained/exposed is 0, stale exposed is 0, contested
  disclosure is 1.0 when a contested row is returned, both caps always hold,
  and it reduces retained rows or bytes by at least 25% versus all 205 rows.
- Recency-only and importance-only are diagnostic selection baselines, not
  candidates for a production default.
- A failed rule is a **NO-GO**, not permission to tune on the holdout. Any
  follow-up tuning needs a replacement holdout and a new preregistration.
