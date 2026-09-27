# External benchmark results — rusty-brain vs MemPalace (Vikunja #60)

- **Date:** 2026-09-26
- **Protocol:** [`2026-09-26-external-bench-preregistration.md`](2026-09-26-external-bench-preregistration.md)
  (base plus addenda 1 and 2). Every rusty-brain run below came from a clean
  checkout of the commit that froze its protocol: `0fa0022` (LongMemEval
  preregistered), `a17c72c` (LongMemEval 256-token parity and LoCoMo),
  `5879f68` (ConvoMem and MemBench).
- **Pins:** `crates/rb-eval/external/manifest.json` and
  `convomem_selection.json`. MemPalace is at `5b1c32e`.
- **Hardware:** Darwin arm64, one machine. Latency and memory were measured
  while other runs shared the machine, so they are diagnostic only.

## What the numbers mean

Every benchmark here measures **retrieval**. Each question comes with a
haystack of past conversation. The system stores it, receives the question,
and returns a ranked list. The score asks whether the gold evidence came back
near the top. No language model answers anything, so none of these numbers
is question-answering accuracy. All metrics run 0–1, and higher is better.

Columns:

| Column | Meaning |
|---|---|
| **MemPalace raw** | MemPalace's raw mode on the pinned commit, re-run locally (Stage A). |
| **rb parity-256** | rusty-brain's vector index over MemPalace-identical documents, embedding truncated at 256 tokens like Chroma. An **adapter-equivalence check**, not a product mode. |
| **rb parity-512** | The same, at rusty-brain's production truncation (fastembed's default of 512). |
| **rb native** | The **production path**: `remember` then `recall` with heuristic enrichment, keyword + vector Linear fusion, the score floor, abstention and the confidence floor. This is the column that describes the product. |

## Headline

| Benchmark (metric) | MemPalace raw | rb parity-256 | rb parity-512 | **rb native** |
|---|---:|---:|---:|---:|
| LongMemEval-S, 500 q (R@5) | 0.966 | 0.966 | 0.960 | **0.980** |
| LongMemEval-S (R@1) | 0.806 | 0.806 | 0.806 | **0.876** |
| LoCoMo, 1,986 q (evidence recall@10) | 0.603 | 0.603 | 0.726 | **0.917** |
| ConvoMem, 250 items (recall@10) | 0.929 | 0.929 | 0.929 | **0.913** |
| MemBench, 8,500 items (hit@5) | 0.786 | 0.787 | 0.787 | **0.876** |

1. **The adapter is exact.** parity-256 reproduces MemPalace raw to three
   decimals, overall and per category, on LongMemEval, LoCoMo and ConvoMem,
   and to one item in 8,500 on MemBench. Every difference in the other
   columns comes from rusty-brain.
2. **Production recall beats MemPalace raw on three of four benchmarks:**
   LongMemEval (R@1 +7.0 pp), LoCoMo (+31.4 pp) and MemBench (+9.0 pp,
   including +27–28 pp on its hardest conditional and post-processing
   categories). It is slightly worse on ConvoMem (−1.6 pp, 4 items net).
3. **Longer embedding input** (512 vs 256 tokens) is worth +12.3 pp on
   LoCoMo's long sessions and nothing on ConvoMem's or MemBench's short
   messages. It is slightly negative on LongMemEval R@5 (−0.6 pp).
4. **One consistent weakness:** indirectly phrased questions (ConvoMem
   implicit connections, LongMemEval preference R@1, MemBench
   recommendations). See "Where native loses".

## LongMemEval-S (cleaned)

R@5 / R@1 by question type:

| Type | n | MemPalace raw | parity-256 | parity-512 | native |
|---|---:|---|---|---|---|
| knowledge-update | 78 | 1.000 / 0.897 | 1.000 / 0.897 | 1.000 / 0.872 | 1.000 / **0.962** |
| multi-session | 133 | 0.992 / 0.857 | 0.992 / 0.857 | 0.992 / 0.850 | 0.985 / **0.932** |
| single-session-assistant | 56 | 0.964 / 0.946 | 0.964 / 0.946 | 0.964 / 0.946 | 0.946 / 0.929 |
| single-session-preference | 30 | 0.967 / 0.700 | 0.967 / 0.700 | 0.933 / 0.700 | 0.933 / **0.433** |
| single-session-user | 70 | 0.914 / 0.714 | 0.914 / 0.714 | 0.914 / 0.686 | **0.986 / 0.929** |
| temporal-reasoning | 133 | 0.947 / 0.714 | 0.947 / 0.714 | 0.932 / 0.752 | **0.985 / 0.820** |

Native overall: R@10 0.990, MRR 0.922, NDCG@10 0.924 (MemPalace definition
0.932 against MemPalace's 0.889). There were no abstentions. 45 of 500
questions returned fewer than 10 results because of the score floor, and
those count as misses.

## LoCoMo (session granularity, top-10)

Evidence recall@10 by numeric category (MemPalace's labels in brackets;
LoCoMo upstream does not name them):

| Cat | n | MemPalace raw | parity-256 | parity-512 | native |
|---|---:|---:|---:|---:|---:|
| 1 [single-hop] | 282 | 0.590 | 0.590 | 0.644 | **0.754** |
| 2 [temporal] | 321 | 0.692 | 0.692 | 0.765 | **0.912** |
| 3 [temporal-inference] | 96 | 0.460 | 0.460 | 0.546 | **0.699** |
| 4 [open-domain] | 841 | 0.581 | 0.581 | 0.743 | **0.967** |
| 5 [adversarial] | 446 | 0.619 | 0.619 | 0.756 | **0.975** |

Native MRR (excluding the 4 empty-evidence questions): 0.732, against 0.367
for parity-256.

## ConvoMem (MemPalace's published sample: 50 per category, top-10)

| Category | MemPalace raw | parity-256 | parity-512 | native |
|---|---:|---:|---:|---:|
| user facts | 0.980 | 0.980 | 0.980 | 0.980 |
| assistant facts | 1.000 | 1.000 | 1.000 | 1.000 |
| abstention | 0.910 | 0.910 | 0.910 | 0.910 |
| preferences | 0.860 | 0.860 | 0.860 | 0.860 |
| implicit connections | 0.893 | 0.893 | 0.893 | **0.813** |
| changing facts | not run (see below) | — | — | — |

**The published "all categories" ConvoMem figure covers 5 of 6.**
`changing_evidence` has no `1_evidence/` folder, MemPalace's lister 404s,
and the category is silently skipped. The adapter mirrors the skip so the
comparison is exact.

## MemBench (FirstAgent, 8,500 items, hit@5)

| Category | n | MemPalace raw | parity-256 | native |
|---|---:|---:|---:|---:|
| aggregative | 1000 | 0.996 | 0.996 | 0.994 |
| comparative | 1000 | 0.985 | 0.985 | **0.995** |
| conditional | 1000 | 0.512 | 0.512 | **0.778** |
| highlevel | 500 | 0.978 | 0.978 | **0.992** |
| highlevel_rec | 500 | 0.802 | 0.802 | 0.774 |
| knowledge_update | 1000 | 0.951 | 0.952 | **0.967** |
| lowlevel_rec | 500 | 1.000 | 1.000 | 0.990 |
| noisy | 1000 | 0.414 | 0.414 | **0.574** |
| post_processing | 1000 | 0.502 | 0.502 | **0.781** |
| simple | 1000 | 0.935 | 0.935 | **0.981** |

parity-512 is identical to parity-256 in every category (turns are short).
The dual-id hit rule changes the totals by 3–5 items (sid-only: parity
0.786, native 0.876).

MemPalace's published MemBench figure (0.803) is its tuned **hybrid** mode.
It is not a comparator here. The hybrid data-version check was stopped
unfinished after 2.6 hours to free CPU for the raw comparator. That is
acceptable because parity already matches raw on the same pinned data to
one item.

## Where native loses, and why

Diagnosed read-only from per-item output (ConvoMem, native vs parity-512
with the same embedding):

- Native lost 8 items and gained 4. In **every** lost item, parity had the
  evidence at rank 5–10, so native only reshuffles borderline cases.
- **7 of 8 losses: keyword fusion reweighting.** Almost every native result
  was found by both channels (`FV`); keyword-only results entered the top 10
  in 1 of 80 slots. The OR-of-tokens keyword query matches nearly every
  message in a short conversation. The Linear blend then favors messages that
  share more words with the question, and implicit or paraphrased evidence
  shares few by design.
- **1 of 8: the score floor.** An 8-message conversation returned 6 results,
  and the evidence (parity rank 8) fell below 0.18.
- Enrichment's effect on the vector is not isolated. That would need a
  keyword-off ablation, which was not preregistered.

The same pattern shows up in LongMemEval's single-session-preference R@1
(0.433 vs 0.700) and MemBench's recommendation categories (highlevel_rec
0.774 vs 0.802, lowlevel_rec 0.990 vs 1.000), where questions are phrased
indirectly. On LoCoMo the
mechanism helps a great deal, because questions name the entities in the
evidence.

## Resources (diagnostic; shared machine)

| Run | Wall | Query p50 / p99 | Peak RSS |
|---|---:|---:|---:|
| LongMemEval parity-256 | 246 s | 3.5 / 15.0 ms | 2.3 GB |
| LongMemEval parity-512 | 470 s | 3.3 / 18.4 ms | 5.2 GB |
| LongMemEval native | 293 s | 4.3 / 8.3 ms | 0.86 GB |
| LoCoMo native | 38 s | 3.8 / 6.3 ms | 0.31 GB |
| ConvoMem native | 49 s | 4.6 / 7.8 ms | 0.22 GB |
| MemBench native | 5,793 s | 4.1 / 10.0 ms | 0.81 GB |

Parity's RSS comes from the adapter embedding a whole haystack in one batch
(padding to the longest document). It is not production behavior. Every
store was in memory, so on-disk DB size was not measured. External API cost
was $0 (local model).

## Limitations

- **Retrieval only.** None of this is end-to-end answer accuracy, which was
  not run (stage F).
- **Synthetic or public data with a fresh store per question.** No lifecycle
  behavior is exercised: feedback, supersede, consolidation and retention
  never run. rusty-brain's freshness and safety properties are measured by
  the scorecard, not here.
- **Protocol quirks are inherited and reported, not fixed:** MemPalace's
  NDCG ideal (reported alongside standard NDCG), LoCoMo's empty evidence
  scoring 1.0, ConvoMem's bidirectional substring gold (an empty evidence text would match any retrieval; none of the 250 pinned items has one, and `items_with_empty_evidence` reports it) and skipped category,
  and MemBench's dual-id hit rule.
- MemPalace's hybrid-v4 LongMemEval result is contaminated (tuning questions
  d6233ab6, 4dfccbf8 and ceb54acb are in its held-out split). Only raw mode
  is used as a comparator.
- Latency and RSS come from a shared machine.

## Proposed improvements (candidates only)

Per decision rule 4, nothing here justifies a change on its own. Each
candidate needs its own development protocol, a sealed final set, the #56
semantic/safety gate and the zero-memory-induced-error scorecard.

1. **Adaptive keyword weight.** Down-weight the FTS component when query
   terms overlap weakly with the top vector hits (paraphrase or implicit
   questions). Target: ConvoMem implicit connections, LongMemEval
   preference R@1 and MemBench recommendations, without giving back LoCoMo
   or MemBench's conditional/post-processing gains.
2. **Score-floor behavior on tiny corpora.** A fixed 0.18 floor cut gold
   evidence in an 8-message store. Consider a floor relative to the top
   score, or skip the floor below a corpus-size threshold.
3. **Embedding truncation.** Production uses 512 tokens with a model trained
   at up to 256. It helped on long sessions here (LoCoMo +12 pp), but
   chunking long memories may beat truncation either way.

## Reproduce

```bash
B=~/.cache/rusty-brain-bench   # data/, runs/, mempalace/ (outside git)
cargo build --release -p rb-eval --features record-local --bin external-bench
X=./target/release/external-bench
$X longmemeval --data $B/data/longmemeval/longmemeval_s_cleaned.json --profile native --out $B/runs
$X locomo      --data $B/data/locomo/data/locomo10.json              --profile native --out $B/runs
$X convomem    --cache $B/data/convomem                                --profile native --out $B/runs
$X membench    --data $B/data/Membench/MemData/FirstAgent              --profile native --out $B/runs
# add --profile parity [--embed-max-tokens 256] for the equivalence columns
```

Per-question JSONL (gold ids, ranked ids, per-hit channels and scores,
metrics, failure class) is written next to each summary. It stays outside
git because the datasets' licenses (CC BY-NC for LoCoMo and ConvoMem) do not
permit committing derived per-item content.
