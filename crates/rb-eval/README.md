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

### Absolute semantic quality: the optional real-model mode

To spot-check *actual* semantic recall, run the optional, `#[ignore]`d real-model
mode against a live embedding model (no CI, no live network in CI):

```bash
VOYAGE_API_KEY=... cargo test -p rb-eval --test real_model -- --ignored --nocapture
```

It runs the same fixtures through `VoyageProvider`, prints semantic metrics for a
human to judge, and makes no assertion against the deterministic baselines.

The `composite_embedding_semantic_recall` case in `tests/real_model.rs` is the
P5 Feature A check: the engine embeds the composite document representation
(content + keywords + tags + context) rather than content alone. Deterministic
vectors cannot show that lift (their values are non-semantic noise — the
composite change only re-shuffles them, which is why the committed deterministic
baselines were re-captured for it), so the comparison is real-model-only. Run it,
read the printed recall/MRR, and compare against a content-only run to judge the
semantic gain.

## Components

- `fixtures/corpus.json` — committed, hand-authored coding-memory corpus. Each
  memory has stable string **keys** (not UUIDs; the runner maps each key to the
  engine-minted `MemoryId` after ingestion), enrichment fields
  (summary/keywords/tags/context), golden queries mapping to expected-relevant
  keys, and near-duplicate clusters for dedup scoring. Authored to exercise the
  FTS, vector, and graph retrieval paths.
- `corpus.rs` — fixture loader + validation; fails fast on any malformed fixture
  (unknown memory type, out-of-range importance/confidence, duplicate keys,
  queries/clusters referencing unknown keys).
- `metrics.rs` — pure, unit-tested functions: `recall_at_k`, `mrr`,
  `dedup_precision`, and latency percentiles (`p50`/`p99`).
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

## Error handling

This is a test-only crate. Failures are assertion failures with readable diffs
(the runner's per-query detail reports which fixture keys recall surfaced, in
order). The crate opts out of the workspace `unwrap_used`/`expect_used` denials
at its root as test code; because nothing shipped depends on `rb-eval`, no panic
can leak into a production binary.
