# rb-eval — offline retrieval-quality regression harness

`rb-eval` is a **dev/measurement crate** for rusty-brain. It ingests a committed
fixture corpus through `rb-engine` with a deterministic embedding provider, runs
golden queries through `engine.recall`, computes ranking metrics, and asserts
each metric meets a committed baseline. It is the CI regression gate for ranking
changes (P5 Features B, A, C).

It is **deliberately excluded from the `rusty-brain` binary's dependency
closure** — nothing shipped depends on it. The CI agent/closure gate asserts it
never appears in `cargo tree -e no-dev -p rusty-brain`.

Run it:

```bash
cargo test -p rb-eval
```

## What this measures — and what it does NOT

This harness uses `rb_embed::DeterministicProvider`: a SHA-256-based provider
that yields **reproducible but non-semantic** unit vectors with no network. That
choice has a precise, honest consequence:

- **It guards:** ranking **determinism**, **regression detection**, and
  **relative-ordering invariants**. The concrete question it answers is *"did a
  change reorder results versus the committed baseline?"* The quality metrics
  reproduce bit-for-bit across runs (only latency varies), so any drift in the
  ranking pipeline is caught immediately.

- **It does NOT measure absolute semantic quality.** Because the deterministic
  vectors carry no meaning, the *absolute* recall/MRR values are modest (the
  default ranking weights are vector-heavy, so deterministic vector noise dilutes
  the keyword signal). Those absolute numbers are **only meaningful as a
  baseline to detect change**, not as a ground-truth quality score.

This mirrors the architecture spec's sqlite-vec scale honesty (§11): state the
limit explicitly rather than overclaim.

### Absolute semantic quality: strict record/replay gate (W4.1)

CI's semantic-measurement path is **replay**: real model vectors recorded once
(manually, with network/credentials) into a committed fixture, then served
offline with zero network and zero keys. Each vector is keyed on
`(model_id, input_kind, sha256(text))`; since W1.4 (query-kind embeddings),
recordings capture document inputs as `"document"` and query strings as
`"query"`. The W4.1 fixture has been re-recorded with 205 document entries and
92 query entries. The production gate uses strict replay: legacy query-to-document
fallback is disabled and a missing/wrong-kind vector fails closed.

- `fixtures/embeddings/all-MiniLM-L6-v2.json` — the committed default fixture:
  real all-MiniLM-L6-v2 (384-dim) vectors for every corpus document (composite
  embedding input), golden query, and held-out query.
- `semantic_gate.json` + `tests/semantic_gate.rs` — lock the corpus, untouched
  holdout, fixture digest, exact input kinds, five chronological instants, and
  preregistered recall/MRR/NDCG/dedup/channel floors. The Linear gate runs in
  ordinary CI; a weekly/manual workflow also reports the five-instant RRF arm.
- `tests/semantic_safety.rs` — offline exact-answer, multi-memory,
  archive/supersede, contested, and instruction-shaped poison strata. The
  explicit ignored pilot gate currently fails closed because the poison remains
  exposed below the correct fact.
- `controlled.rs` + `tests/controlled_arms.rs` — five fixed chronological
  streams for equal-budget Linear/RRF/exact-evidence/recency/importance
  retrieval arms and novelty/importance-confidence/combined online shadow
  admission arms. The scheduled/manual report emits every seed and enforces
  the frozen no-go for the exact lane and the combined admission arm's
  shadow-only qualification; the overall poison-exposure NO-GO blocks pilot
  admission and no production behavior changes.

Re-record after any corpus change (single command per provider):

```bash
# local ONNX model (downloads ~90MB of weights on first use)
cargo test -p rb-eval --features record-local --test record_embeddings \
  -- --ignored record_local_model_fixture --nocapture

# Voyage (preferred fixture source when a key is available)
VOYAGE_API_KEY=... cargo test -p rb-eval --test record_embeddings \
  -- --ignored record_voyage_fixture --nocapture
```

After recording a different model's fixture, point
`rb_eval::replay::COMMITTED_FIXTURE_PATH` (and the `include_str!` in
`ReplayProvider::committed`) at it and re-capture the baseline artifact.

To spot-check semantic recall against the LIVE API instead, the older
`#[ignore]`d real-model mode remains:

```bash
VOYAGE_API_KEY=... cargo test -p rb-eval --test real_model -- --ignored --nocapture
```

It runs the same fixtures through `VoyageProvider`, prints semantic metrics for a
human to judge, and makes no assertion against the deterministic baselines. Voyage
was not recorded for the 2026-07-12 W4.1 result because no existing credential was
available; the local-model gate made no network calls and cost $0.

Run the aggregate-only controlled report locally with:

```bash
cargo test -p rb-eval --test controlled_arms \
  controlled_retrieval_and_admission_arms_report_every_seed \
  -- --ignored --nocapture
```

Its 2026-07-12 result keeps the exact-evidence lane off. The combined admission
product meets its shadow-arm criteria, but the overall zero-poison-exposure
NO-GO blocks a bounded pilot. See the
[frozen preregistration](../../docs/eval/2026-07-12-w41-controlled-arms-preregistration.md)
and [dated results](../../docs/eval/2026-07-12-w41-semantic-gate-results.md).

### The pre-Phase-1 baseline artifact

`docs/eval/2026-06-12-pre-phase1-baseline.json` freezes the pre-Phase-1
measurement: overall metrics (recall@k, MRR, NDCG over the authored grades,
dedup precision) AND per-channel (FTS/vector/graph) hit-contribution stats,
over both the golden and held-out query sets, under the committed real-vector
replay fixture (plus the deterministic reference). It is **frozen** — later
Phase 1 workstreams report before/after against it; never edit it. Regenerate
under a new dated filename only if the corpus or fixture is re-recorded:

```bash
cargo run -p rb-eval --example capture_baseline > docs/eval/<date>-pre-phase1-baseline.json
```

Notable starting line it records: the FTS channel contributes hits on only
~4% of natural-language goldens (`channels.fts_query_rate`) and the graph
channel on 0% — the headroom W1.2 (FTS tokenization) and W1.5 (real graph
hops) exist to close (Phase 1 gate: FTS ≥ 80%).

The `composite_embedding_semantic_recall` case in `tests/real_model.rs` is the
P5 Feature A check: the engine embeds the composite document representation
(content + keywords + tags + context) rather than content alone. Deterministic
vectors cannot show that lift (their values are non-semantic noise — the
composite change only re-shuffles them, which is why the committed deterministic
baselines were re-captured for it), so the comparison is real-model-only. Run it,
read the printed recall/MRR, and compare against a content-only run to judge the
semantic gain.

## Components

- `fixtures/corpus.json` — committed, hand-authored coding-memory corpus
  (Phase-1 scale: **205 memories, 72 graded golden queries, 8 near-duplicate
  clusters**). Each memory has stable string **keys** (not UUIDs; the runner
  maps each key to the engine-minted `MemoryId` after ingestion) and enrichment
  fields (summary/keywords/tags/context). The corpus simulates a dev team's
  shared memory across four fictional projects (`mer_*` payments backend,
  `sky_*` TypeScript dashboard, `pk_*` Python build cache, `ops_*` platform)
  plus rusty-brain itself: decisions, constraints, gotchas, incidents, review
  outcomes, preferences, and hook-captured session summaries (`*_session_*`
  keys, `confidence < 1.0`). It deliberately contains near-duplicate
  restatements (the dedup clusters) and six contradiction pairs (decision
  reversals). Golden queries are natural-language developer questions with
  **relevance grades** (`grades` aligns with `expected`; 3 = primary, 2 =
  relevant, 1 = marginal — consumed by `ndcg_at_k`, W1.0b). The README
  quickstart query (`how is writing serialized?`) is committed verbatim with
  its verbatim target memory (`readme_quickstart`).
- `fixtures/holdout_queries.json` — the **held-out** graded query set (20
  queries). It must **never** be used for weight tuning, threshold selection,
  or tokenizer choice; it is reserved for the Phase 4 (W4.1) CI gate so that
  gate runs on queries no tuning loop has seen. `corpus.rs` validates it
  against the committed corpus, and the harness asserts it stays disjoint from
  the tuning goldens. **Measurement cadence:** holdout aggregates are computed
  only at frozen-artifact captures (`examples/capture_baseline.rs`) and by the
  aggregate-only W4.1 gate — never as per-query tuning feedback. The legacy
  holdout replay diagnostic stays `#[ignore]`d; the strict gate is the only
  default-CI consumer. Iterating ranking changes while watching per-query
  holdout behavior is itself a tuning loop.
- `corpus.rs` — fixture loader + validation; fails fast on any malformed fixture
  (unknown memory type, out-of-range importance/confidence, duplicate keys,
  queries/clusters referencing unknown keys).
- `metrics.rs` — pure, unit-tested functions: `recall_at_k`, `mrr`,
  `dedup_precision`, `ndcg_at_k` (graded relevance, W1.0b), and latency
  percentiles (`p50`/`p99`).
- `replay.rs` — record/replay of real embedding vectors as committed fixtures
  (see above); `RecordingProvider` wraps a real provider, `ReplayProvider`
  serves the committed vectors offline and fails closed on misses.
- `backend.rs` — a `MemoryBackend` over an in-memory `SqliteStore`, mirroring the
  daemon's namespace-scoping/active-only semantics, so the real FTS + sqlite-vec
  + graph paths run (not a mock map).
- `runner.rs` — builds the engine, ingests fixtures, runs golden queries,
  computes the aggregate report, and compares against baselines.
- `baselines.json` — committed metric baselines; the harness fails on regression.

## Updating baselines

`baselines.json` is captured from an initial run and reproduces bit-for-bit.
**Raising a baseline** (after an intentional ranking improvement) or **lowering
one** (for a justified trade-off) is a deliberate, reviewed commit — never an
incidental change. Latency p50/p99 are reported but **not gated** (they are
machine-dependent).

The Phase-1 plan's ground rule applies: every retrieval-semantics change lands
with a re-captured `baselines.json` in the same commit, with a one-line
justification of which metric moved and why. The W1.0a corpus expansion reset
the absolute numbers downward (natural-language goldens against today's FTS
tokenization and non-semantic deterministic vectors); that low starting line is
the measurement Phase 1's retrieval workstreams are expected to raise.

## Error handling

This is a test-only crate. Failures are assertion failures with readable diffs
(the runner's per-query detail reports which fixture keys recall surfaced, in
order). The crate opts out of the workspace `unwrap_used`/`expect_used` denials
at its root as test code; because nothing shipped depends on `rb-eval`, no panic
can leak into a production binary.
