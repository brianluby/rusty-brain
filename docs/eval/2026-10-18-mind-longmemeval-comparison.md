# rusty-brain vs `mind` on LongMemEval (oracle, turn level)

- **Date:** 2026-10-18. Local only, $0 API cost.
- **Report only.** The harness code used here is not kept in the tree. It is
  in this repository's history at commit `b160870` (the turn-level mode and
  the BM25 replica) and `11b1c7c` (the last revision with it), where the
  commands below can be rerun.
- **Question:** how does rusty-brain compare with the
  [`mind`](https://github.com/Da7-Tech/mind) project on the one benchmark both
  report, under mind's own setup?
- **Artifact:**
  [`artifacts/2026-10-18-mind-longmemeval-comparison.json`](artifacts/2026-10-18-mind-longmemeval-comparison.json)
  holds per-question outcomes for every run, file hashes and the validation
  numbers.

## Short answer

On mind's own 50 questions:

- **rusty-brain native vs mind:** matches or beats both mind variants on all
  three metrics.
- **rusty-brain native vs mind's own BM25 baseline:** statistically tied.

On all 470 non-abstention questions:

- **Rank-1 evidence and answer-string hits:** rusty-brain native ties BM25.
- **Evidence in the top 5:** native beats BM25, 0.928 vs 0.881, and the
  difference is significant (paired p = 0.004).
- **Vector-only (parity) profile:** worse than BM25 at rank 1.

mind itself was not run here; its numbers are its published 50-question
results.

## Setup: mind's harness, replicated

mind's three LongMemEval reports
(`bench/results/longmemeval-{bm25,offline,concept}-v7-dev.json`) all use:

- **Dataset:** `longmemeval_oracle.json` (`xiaowu0162/longmemeval-cleaned`,
  revision `98d7416`, SHA-256 `821a2034…`). Each question's history holds only
  its evidence sessions, about 22 turns.
- **Records:** one per turn, user and assistant, with text
  `[date] question <qid> session <sid> <role>: <content>`. Identical texts
  collapse into one record.
- **Evidence:** turns marked `has_answer`, or every turn of an answer session
  with no marked turn.
- **Query and store:** the question text, against a fresh store per question.
- **Metrics:** evidence at rank 1, evidence in the top 5, and the normalized
  answer string inside any top-5 record.
- **Subset:** 50 questions (seed 13), abstention excluded.

The replication follows `bench/longmemeval.py` and `mind.py` at commit
`91cbbf0`. Both match the SHA-256 recorded in mind's reports.

- **rusty-brain:**
  - Command: `external-bench longmemeval-turn` (`rb-eval`, at `b160870`).
  - Module: `crates/rb-eval/src/external/longmemeval_turn.rs` (at `b160870`).
  - Embedding model: pinned local `all-MiniLM-L6-v2` at 512 tokens (kanban
    #100).
  - Native profile: the production pipeline (heuristic enrichment plus Linear
    fusion of FTS, vector and graph).
  - Parity profile: vector index only.
- **BM25 reference:** `scripts/external/mind_bm25_replica.py` (at
  `b160870`), an independent reimplementation of mind's `BM25Baseline` and
  tokenizer. It does not run mind's code.
- **Record formats:**
  - `mind`: mind's exact text, used for the comparison.
  - `clean`: `role: content` only. mind's format embeds the session id, and
    oracle gold sessions are named `answer_*`, which is a potential label leak
    shared by every engine scored that way.

### Validation

- **Records:** mind's report counts 978 records over its 50 questions before
  collapsing duplicates. Our construction gives exactly 978, and per-question
  record and evidence counts match an independent Python rebuild.
- **BM25:** the BM25 replica reproduces mind's published BM25 numbers exactly
  (0.66 / 0.92 / 0.56).

## Results on mind's 50 questions (mind record format)

Wilson 95% intervals in brackets.

| System | Evidence@1 | Evidence@5 | Answer-string@5 |
|---|---:|---:|---:|
| mind offline (published) | 0.50 | 0.84 | **0.58** |
| mind + concept sidecar (published) | 0.56 | 0.84 | 0.52 |
| BM25 (mind published = our replica) | **0.66** [0.52, 0.78] | **0.92** [0.81, 0.97] | 0.56 [0.42, 0.69] |
| rusty-brain parity (vector only) | 0.52 [0.39, 0.65] | 0.88 [0.76, 0.94] | 0.50 [0.37, 0.63] |
| **rusty-brain native** | 0.64 [0.50, 0.76] | **0.92** [0.81, 0.97] | **0.58** [0.44, 0.71] |

- **Native vs BM25, paired:** native wins / loses on evidence@1 +6/−7,
  evidence@5 +2/−2 and answer@5 +1/−0, all with p = 1.0. That is a tie.
- **Native vs mind:** native's point estimates are at or above both mind
  variants on every metric. mind publishes no per-question outcomes, so this
  cannot be tested pairwise. At n = 50 one question is 2 points, and the
  intervals overlap.

## Results on all 470 non-abstention questions

| System | Evidence@1 | Evidence@5 | Answer-string@5 |
|---|---:|---:|---:|
| BM25 replica | 0.543 [0.50, 0.59] | 0.881 [0.85, 0.91] | 0.455 [0.41, 0.50] |
| rusty-brain parity | 0.496 [0.45, 0.54] | 0.883 [0.85, 0.91] | 0.430 [0.39, 0.47] |
| **rusty-brain native** | 0.543 [0.50, 0.59] | **0.928** [0.90, 0.95] | 0.453 [0.41, 0.50] |

Paired exact McNemar tests against BM25 (wins / losses):

| | Evidence@1 | Evidence@5 | Answer-string@5 |
|---|---|---|---|
| native, mind format | +47/−47, p = 1.0 | **+38/−16, p = 0.004** | +7/−8, p = 1.0 |
| native, clean format | +49/−46, p = 0.84 | **+40/−15, p = 0.001** | +13/−8, p = 0.38 |
| parity, mind format | +75/−97, p = 0.11 | +38/−37, p = 1.0 | **+8/−20, p = 0.036** (worse) |
| parity, clean format | **+70/−113, p = 0.002** (worse) | +33/−40, p = 0.48 | **+11/−29, p = 0.006** (worse) |

## What this means

1. **mind's 50 are easier than average.** BM25 scores 0.66 at rank 1 on them
   but 0.543 on all 470. Any conclusion from those 50 alone carries about ±13
   points of noise and a selection bias.
2. **Fusion is what helps.** rusty-brain native gets an evidence turn into the
   top 5 for 4.7 more questions per hundred than BM25. On which turn ranks
   first it only ties BM25. The vector-only profile is worse than BM25 at
   rank 1, as expected for exact-turn questions with lexical overlap. So the
   gain comes from combining channels, not from the embedding model.
3. **On mind's own setup, rusty-brain is at least as good as mind's best
   reported variant.** mind's published variants are below its own BM25
   baseline on evidence@1 and evidence@5. rusty-brain native ties that
   baseline at rank 1 and beats it at depth 5 on the full set.
4. **The record format barely matters.** Removing the date, question id and
   session id moves native by at most 1.4 points (evidence@1 0.543 → 0.557).
   The `answer_*` session-id leak does not drive these results.

## Caveats

- **mind was not run.** Its numbers are its published 50-question results,
  and the 470-question comparison has no mind row. Running mind would mean
  executing third-party code; the BM25 replica avoids that and is validated
  against mind's own BM25 numbers.
- **The oracle is an easy haystack** (about 22 turns, no distractor sessions).
  This complements, and does not replace, the LongMemEval-S session-level
  results in `2026-09-26-external-bench-results.md` (native R@5 0.980 over
  500 questions).
- **Configuration was not tuned.** These are production defaults and the
  pinned model. Nothing was tuned on these questions.
- **Latency is not compared.** mind reports recall latency only, while our
  wall times include ingest and embedding, on different hardware.

## Provenance

- **What the runs were built from:** the reported runs were built from an
  uncommitted working tree on `1694a38`, so their summaries record that
  commit with `dirty: true`. Those Rust changes were then committed with no
  further edits as `b160870`.
- **Check from a clean tree:** three of the eight rusty-brain runs (the
  parity profile: n = 50 in both formats, and n = 470 in mind format) were
  rerun from the clean, committed `11b1c7c`. Per-question outcomes and top-5
  rankings were identical to the reported runs.
- **Not repeated:** the native-profile runs.
- **BM25 replica:** its later edits (no MD5, ties ordered by text) left
  per-question outcomes identical on all four runs.

## Reproduce

From a checkout of `b160870`:

```bash
cargo build --release -p rb-eval --features record-local --bin external-bench
./target/release/external-bench --diagnostic longmemeval-turn \
  --data DATA/longmemeval_oracle.json --profile native --format mind \
  --question-ids mind/bench/results/longmemeval-bm25-v7-dev.json --out RUNS
python3 -I scripts/external/mind_bm25_replica.py DATA/longmemeval_oracle.json all mind
```

Omit `--question-ids` (and pass `all` to the replica) for all 470 questions.
Use `--profile parity` and `--format clean` for the other runs.
