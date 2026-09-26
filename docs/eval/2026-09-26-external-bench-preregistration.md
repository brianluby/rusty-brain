# External benchmark preregistration — LongMemEval (Vikunja #60)

- **Frozen:** 2026-09-26, by the commit that adds this file. It predates
  every full rusty-brain run.
- **Scope of this freeze:** LongMemEval-S (cleaned), 500 questions, parity
  and native profiles. LoCoMo, ConvoMem and MemBench (stages D–E) get their
  own preregistration before their first full run.
- **Pins:** `crates/rb-eval/external/manifest.json` (MemPalace commit
  `5b1c32e`, dataset HF revision `98d7416`, sha256 `d6f21ea9…c3a442`, MIT).

## Stage A — reproduced before this freeze

MemPalace raw mode on the pinned commit, run locally (Darwin arm64,
Python 3.12, `uv sync --frozen`), took 287 s:

| Metric | Published | Reproduced |
|---|---:|---:|
| recall_any@5 | 0.966 | **0.966** |
| recall_any@10 | 0.982 | 0.982 |
| NDCG@10 (MemPalace definition) | 0.889 | 0.889 |

This is within the ±0.5 pp gate, so rusty-brain differences can be
interpreted.

## What was seen before freezing

- A 5-question parity smoke (`--limit 5`) was checked only in aggregate
  against MemPalace's first 5 (R@5 0.8 vs 0.8), to validate the adapter. No
  per-question output was inspected, and no rusty-brain code or config
  changed as a result.
- The native profile has not been run on real data.

## Profiles

| Profile | Ingestion | Ranking |
|---|---|---|
| **parity** | MemPalace raw: one document per session, user turns joined by `\n`, sessions without user turns skipped, `MemoryNote` written directly with all-MiniLM-L6-v2 vectors | rusty-brain's sqlite-vec cosine index only (top 50) |
| **native** | `engine.remember` for each session document: heuristic enrichment, CLI provenance, confidence default | `engine.recall_with_status` with production defaults (Linear fusion, score floor, abstention, confidence floor), limit 50 |

Both profiles use a fresh in-memory store per question. Session ids never
enter stored text; they round-trip through an adapter-side map. The
`has_answer` field is never read. Timestamps are fixed, so recency carries
no signal.

## Metrics, frozen

Primary: **recall_any@5**, overall and for each of the six question types.

Also reported: recall_any and recall_all @1/3/5/10, MRR, standard NDCG@k,
MemPalace-definition NDCG@k (the only NDCG comparable with published
numbers), failure class counts, abstentions, query p50/p95/p99, total ingest
time and peak RSS.

## Interpretation rules, frozen

1. The full-500 run of each profile is **one-shot**. There is no tuning
   after reading per-question output. A later tuning effort needs a new
   development protocol and a sealed final set, declared before any
   per-question output is read.
2. **Parity validity.** If parity recall_any@5 is within ±1.0 pp of
   MemPalace's 0.966, the store and embedding path are equivalent for this
   benchmark. Outside that band, the difference is attributed to
   adapter/embedding runtime (fastembed vs Chroma ONNX) and investigated as
   such, never "fixed" by ranking changes.
3. **Native** is reported in its own column and never merged with parity.
   A higher native score may come from enrichment/fusion; a lower one may
   come from the score floor or abstention returning fewer than k results.
   Both are recorded, not adjusted.
4. **Decision gate.** No ranking, retention or schema change is justified by
   this benchmark alone. Any proposal must also pass #56's semantic gate and
   the zero-memory-induced-error scorecard.
5. **Limitations** are stated in the report: public synthetic chat data, a
   fresh store per question, no lifecycle behavior (feedback, supersede,
   consolidation), a retrieval-only headline, and MemPalace's hybrid-v4
   held-out contamination (questions d6233ab6, 4dfccbf8 and ceb54acb). That
   last one is why only raw mode is used as the comparator.

## Commands

```bash
B=~/.cache/rusty-brain-bench
cargo build --release -p rb-eval --features record-local --bin external-bench
for p in parity native; do
  ./target/release/external-bench longmemeval \
    --data $B/data/longmemeval/longmemeval_s_cleaned.json --profile $p --out $B/runs
done
```

---

## Addendum 1 — embedding truncation and LoCoMo (frozen before any full LoCoMo run)

### Finding: parity was not embedding-equivalent

`rb_embed::LocalProvider` loads fastembed's all-MiniLM-L6-v2 with fastembed's
default **512-token** truncation. MemPalace, through Chroma's ONNX MiniLM,
truncates at **256** (sentence-transformers' `max_seq_length`). This came to
light on a one-conversation LoCoMo adapter check, compared in aggregate only:

| LoCoMo conv 1 (199 questions), parity, top-10 | Evidence recall |
|---|---:|
| MemPalace raw (Stage A output) | 0.6265 |
| rusty-brain parity, 512 tokens (production default) | 0.7446 |
| rusty-brain parity, 256 tokens | **0.6265** |

At 256 tokens the adapter matches MemPalace exactly, so the gap is
entirely embedding truncation. `LocalProvider::load_with_max_tokens` and the
runner flag `--embed-max-tokens` were added. The production default is
unchanged.

### Consequences for LongMemEval

- The preregistered LongMemEval parity and native runs (frozen commit
  `0fa0022`) use production truncation (512). They are reported as
  preregistered.
- The true MemPalace-equivalent parity is a separate, clearly labelled run:
  **parity at 256 tokens**. It measures adapter equivalence only. It is not
  a tuning step, and no ranking changes follow from it.
- Interpretation rule 2 (parity within ±1.0 pp of 0.966) applies to the
  256-token run. The 512-token parity result is reported as the
  production-embedding diagnostic.

### LoCoMo protocol

- Dataset: `locomo10.json` at snap-research/locomo `3eb6f2c`, sha256
  `79fa87e9…98ff4`, CC BY-NC 4.0 (never committed).
- Stage A reproduced MemPalace raw session top-10 at **0.603** average
  evidence recall (published 0.603).
- Documents: one per session, `{speaker} said, "{text}"` lines. Evidence
  `D<n>:<m>` maps to `session_<n>`. Top-k = 10. The runner refuses a top-k
  larger than a conversation's session count (19–32).
- Isolation: a fresh store per **question**, unlike MemPalace's one
  collection per conversation, so that access counts can't leak. Memoized
  embeddings keep vectors identical across rebuilds.
- Runs: parity-256, parity-512 (production embedding) and native (production
  defaults). One shot each, with no tuning after per-question output.
- Primary metric: MemPalace-definition evidence recall@10, overall and per
  numeric category. MemPalace's category names are reported as MemPalace's;
  LoCoMo upstream does not name them.
- The 4 questions with empty evidence score 1.0 under MemPalace's
  definition. `evidence_recall_excl_empty` is reported alongside.
- Interpretation: parity-256 within ±1.0 pp of 0.603 validates the adapter
  on LoCoMo. Native and parity-512 are separate columns. The decision gate
  (rule 4) and limitations (rule 5) apply unchanged.
