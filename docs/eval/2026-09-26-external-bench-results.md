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

| Run | Wall | Ingest total | Query p50 / p95 / p99 | Peak RSS |
|---|---:|---:|---:|---:|
| LongMemEval parity-256 | 246.094 s | 242.530 s | 3.499 / 5.980 / 15.031 ms | 2.3 GB |
| LongMemEval parity-512 | 469.771 s | 466.090 s | 3.265 / 9.569 / 18.381 ms | 5.2 GB |
| LongMemEval native | 293.313 s | 289.814 s | 4.335 / 6.743 / 8.296 ms | 0.86 GB |
| LoCoMo native | 38.412 s | 26.184 s | 3.808 / 5.277 / 6.263 ms | 0.31 GB |
| ConvoMem native | 49.387 s | 47.628 s | 4.558 / 6.741 / 7.810 ms | 0.22 GB |
| MemBench native | 5,792.838 s | 5,734.835 s | 4.093 / 8.082 / 9.971 ms | 0.81 GB |

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

## Historical protocol commands

These identify the frozen runs; **do not rerun them into the frozen `runs/`
directory**. For new instrumentation evidence use the separately labeled
diagnostic commands below, not another one-shot quality evaluation.

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

Frozen per-question JSONL contains gold ids, ranked ids, final scores and
channel booleans. Only LongMemEval originally recorded an explicit failure
class. It does **not** contain native numeric per-channel scores; those
cannot be recovered uniquely from a fused score and booleans. Artifacts stay
outside git (including derived per-item records); only aggregate metadata
and checksums are committed here.

## 2026-10-16 artifact closeout: frozen quality versus new diagnostics

The headline and all category quality figures above remain the **2026-09-26
frozen results**. No benchmark was rerun, tuned, re-embedded or reranked to
prepare this addendum. The committed
[`frozen metadata inventory`](artifacts/2026-10-16-external-bench-frozen-metadata.json)
copies metadata from all 12 existing summaries and hashes both summaries and
their per-question JSONL. It includes every LongMemEval R@1/3/5/10,
recall_any/recall_all, MRR and both NDCG definitions overall and for all six
categories, plus all other benchmarks' category breakdowns. Original files
are at `~/.cache/rusty-brain-bench/runs/` with the names in that inventory;
they are local reproducibility artifacts, not remotely hosted downloads.

The four native summaries, in that local cache directory
(`~/.cache/rusty-brain-bench/runs/`), are `longmemeval-native.summary.json`,
`locomo-top10-native.summary.json`, `convomem-top10-native.summary.json`, and
`membench-movie-top5-native.summary.json`.
The ingest and p95 columns above were already recorded there; they are not
new performance measurements.

| Requested evidence | Frozen evidence / reconstruction | New diagnostic execution |
|---|---|---|
| Protocol, data/order pins, code, provider, granularity, top-k, quality/category metrics | Existing preregistration, manifests and hashed summaries; unchanged | Same pinned inputs, explicitly a different current-code execution |
| Numeric fusion weights | `Weights::default()` inspected at `0fa0022`, `a17c72c`, `5879f68`: vector 0.45, keyword 0.30, graph 0.10, importance 0.10, recency 0.05; summaries specify default Linear. Parity has no fusion | Captured from current `Weights::default()`, not copied from this table |
| Explicit failure classification per question | `complete-artifacts` deterministically adds missing outcome classes using existing gold/ranked ids or `found`/`hit`; original LongMemEval classes stay unchanged | Emitted by all four adapters |
| Numeric channel scores per returned question/hit | Native scores **unrecoverable** from frozen fused scores/booleans; parity cosine distance was recorded | Source-backed FTS reciprocal rank (not BM25), clamped cosine similarity and reciprocal graph hops; absent channels are `null`. These are unweighted Linear scores **before** priors, confidence and source adjustment, not final contributions |
| DB size | **Unmeasured**: old databases were ephemeral and no snapshots/page counts remain | Per-question `logical_db_bytes` after recall and maximum in summary: SQLite `page_count * page_size`, including free pages/indexes; not physical disk bytes, WAL or RSS |
| OS/hardware | Only `macos aarch64` recorded. Historical CPU model, RAM and full OS/kernel version are **unavailable**; today's machine cannot establish them | Current timestamp, kernel, CPU model, RAM and available logical CPUs; unsupported/unavailable fields remain `null` |
| Ingest, query p50/p95/p99, RSS, wall time, API cost | Recorded separately from quality in all 12 summaries; $0 local-model API cost | Captured again, diagnostic only; shared-machine caveat remains |

Reconstruction adds **outcome**, not causal failure analysis: `hit`,
`partial_hit`, `miss`, `abstained`, or `unscorable_empty_gold`. Empty evidence
can retain the inherited published metric of 1.0 without being mislabeled a
successful retrieval. ConvoMem degenerate-gold/empty-evidence flags and
MemBench's sid-only alternative remain separately visible.

### Commands for integration owner

After the combined build/check, run artifact reconstruction in a **new**
directory; it reads all 12 frozen outputs and does no inference. Create-new
output files prevent overwriting the frozen inputs. The resulting manifest
contains input/output hashes and failure counts; completed per-question
JSONL stays outside git.

```bash
B=~/.cache/rusty-brain-bench
X=./target/release/external-bench
# Build once after integration, not while sibling implementations are in flight:
cargo build --release -p rb-eval --features record-local --bin external-bench
C=$(mktemp -d "$B/artifact-completion-20261016.XXXXXX")
$X complete-artifacts --runs "$B/runs" --out "$C"
```

For **full all-four per-question numeric-channel/DB evidence**, the following
is a separately declared current diagnostic execution, **not** a replacement
quality run. It includes native-512, parity-512 and parity-256. No frozen
vector/store cache was found in the benchmark cache (`*.npy`, `*.npz`,
`*.pkl`, `*.sqlite`, `*.db`; only dependency test data matched), so native
channels and DB allocation require rebuilding each question's temporary
store and running the pinned local model. There is no external API charge;
the old native wall times total about 6,174 seconds (MemBench alone about
5,793 seconds), not a runtime guarantee for this execution. Do not inspect
individual failures to change weights, gates, token limits or selection.

```bash
D=$(mktemp -d "$B/diagnostics-20261016.XXXXXX")
for mode in native parity parity256; do
  profile=$mode
  tokens=()
  if [ "$mode" = parity256 ]; then
    profile=parity
    tokens=(--embed-max-tokens 256)
  fi
  $X --diagnostic longmemeval --data "$B/data/longmemeval/longmemeval_s_cleaned.json" --profile "$profile" --out "$D" "${tokens[@]}"
  $X --diagnostic locomo --data "$B/data/locomo/data/locomo10.json" --profile "$profile" --top-k 10 --out "$D" "${tokens[@]}"
  $X --diagnostic convomem --cache "$B/data/convomem" --profile "$profile" --per-category 50 --top-k 10 --out "$D" "${tokens[@]}"
  $X --diagnostic membench --data "$B/data/Membench/MemData/FirstAgent" --profile "$profile" --topic movie --top-k 5 --out "$D" "${tokens[@]}"
done
```

All new files are named `diagnostic-*`; summary `artifact_role` is
`current_diagnostic_not_frozen_quality`. Current code/config SHA and dirty
state are recorded independently. These numbers explain **current**
retrieval, not the historical run's missing channel values. A limited smoke
is instrumentation validation only, never the all-question audit.

**Closeout boundary:** instrumentation and reconstructable frozen metadata
are supplied here. The full diagnostic execution follows below. Missing
historical hardware and native channel values remain explicitly unavailable,
not implicitly waived or retrospectively invented.

## Diagnostic replay execution (2026-10-17)

The full all-four diagnostic matrix above was executed once: 12 runs
(native-512, parity-512 and parity-256 for each benchmark) on clean merged
`main` `a1ec930`, from a detached worktree, with the commands exactly as
written. This is a **current diagnostic**, not a replacement quality run: the
September headline above stays the frozen result. Hardware: Apple M4 Pro,
12 logical CPUs, Darwin 27.0.0. Only the pinned local embedding model runs;
there are no external API calls and $0 external API cost.

The summary, hashes and audit counts are committed as
[`artifacts/2026-10-17-external-bench-diagnostic-replay.json`](artifacts/2026-10-17-external-bench-diagnostic-replay.json).
The per-question JSONL stays outside git (LoCoMo and ConvoMem are
CC BY-NC) in the local cache as `diagnostics-20261017-a1ec930.*`, and the
artifact records each file's SHA-256.

### Per-question audit: complete

| Run | Questions | Failure class on every record | Returned hits with numeric channel scores | Records with `logical_db_bytes` | Max DB bytes |
|---|---:|:---:|---:|:---:|---:|
| LongMemEval native | 500 | yes | 8,333 / 8,333 | 500 | 2,043,904 |
| LongMemEval parity | 500 | yes | 23,551 / 23,551 | 500 | 1,982,464 |
| LoCoMo native | 1,986 | yes | 19,855 / 19,855 | 1,986 | 2,007,040 |
| LoCoMo parity | 1,986 | yes | 19,860 / 19,860 | 1,986 | 1,990,656 |
| ConvoMem native | 250 | yes | 2,464 / 2,464 | 250 | 1,830,912 |
| ConvoMem parity | 250 | yes | 2,487 / 2,487 | 250 | 1,814,528 |
| MemBench native | 8,500 | yes | 42,500 / 42,500 | 8,500 | 2,080,768 |
| MemBench parity | 8,500 | yes | 42,500 / 42,500 | 8,500 | 1,945,600 |

Parity-256 rows are identical to parity rows (see the next section). Channel
scores keep the semantics stated above: unweighted, pre-prior, `null` for an
absent channel. DB bytes are logical SQLite allocation per fresh in-memory
store, not disk, WAL or RSS.

### Finding: the effective embedding length is now 256 tokens

Today the "512-token" profiles are not 512 tokens. On every benchmark,
parity with the default (512 requested) produces exactly the same results as
parity-256. The only model snapshot available
(`Qdrant/all-MiniLM-L6-v2-onnx@d13954661f83`, downloaded 2026-10-16)
declares `model_max_length: 256` and tokenizer truncation 256, and fastembed
5.15 appears to cap the requested length at that value. Rebuilding the
September code (`a17c72c`) gives the same LoCoMo parity result today, both by
default and with an explicit `--embed-max-tokens 512`, so the change is in
the model files, not in rusty-brain. The September snapshot evidently allowed
512 tokens and no longer exists, so **the September 512-token columns cannot
be reproduced today**. The run metadata label
`512 (fastembed default, production)` overstates the effective length; that,
model-revision pinning and the production 256-vs-512 choice are tracked in
kanban #100. Nothing was tuned or changed in response.

### Current versus frozen headline

| Benchmark (metric) | Frozen parity-256 | Current parity (effective 256) | Frozen parity-512 | Frozen native | **Current native** |
|---|---:|---:|---:|---:|---:|
| LongMemEval-S (R@5) | 0.966 | 0.966 | 0.960 | 0.980 | **0.978** |
| LongMemEval-S (R@1) | 0.806 | 0.806 | 0.806 | 0.876 | **0.876** |
| LoCoMo (evidence recall@10) | 0.603 | 0.603 | 0.726 | 0.917 | **0.900** |
| ConvoMem (recall@10) | 0.929 | 0.929 | 0.929 | 0.913 | **0.913** |
| MemBench (hit@5) | 0.787 | 0.787 | 0.787 | 0.876 | **0.876** |

1. Current parity reproduces the frozen **256-token** parity column exactly,
   and so still reproduces MemPalace raw. The adapter-equivalence result holds.
2. ConvoMem and MemBench, whose messages are short, are unchanged in every
   column, as truncation predicts.
3. Current native is lower only where long inputs matter: LoCoMo −1.7 pp
   (0.917 → 0.900) and LongMemEval R@5 −0.2 pp. It still leads MemPalace raw
   on LongMemEval, LoCoMo and MemBench, and still trails slightly on
   ConvoMem.
4. The frozen September headline is not rewritten. It remains the record of
   what that configuration measured; this section is the record of what the
   current configuration measures.

### Remaining unavailable, not waived

- Native numeric channel scores and DB sizes for the **September** runs:
  unrecoverable, as stated above. The new values describe current code only.
- September CPU model, RAM and full OS version: unavailable.
- A reproducible 512-token configuration: not available until kanban #100 pins a
  model revision that allows it. (Resolved the same day; see the next section.)

## Embedding model pin (2026-10-17, kanban #100)

The 256-token finding above has a confirmed cause and a fix.

- **Cause.** Upstream commit `944c7f6` of `Qdrant/all-MiniLM-L6-v2-onnx`
  (2026-09-27) changed `tokenizer_config.json` `model_max_length` from 512
  to 256. `model.onnx` is byte-identical across revisions. fastembed 5.15
  truncates at `min(requested, model_max_length)` without any warning, and
  hf-hub reuses whatever revision is cached. So older installs kept 512 and
  fresh installs silently got 256. The September runs (2026-09-26) used
  revision `5f1b8cd`, the last one allowing 512.
- **Fix.** The local provider now loads revision
  `5f1b8cd78bc4fb444dd171e59b18f3a3af89a079` only, checks the SHA-256 of all
  five files, and fails closed on a mismatch. A token request above the
  tokenizer's cap is an error instead of a silent clamp. Run metadata records
  the revision, the file hashes, and the requested and effective token limits
  (read back from the loaded tokenizer). The default stays 512, the
  documented setting.
- **Committed fixtures are unaffected.** Re-recording the semantic-gate
  replay fixture with the pin gives all 297 vectors byte-identical to the
  committed fixture at both 512 and 256: every corpus text fits in 256
  tokens. Only long-input benchmarks tell the settings apart.
- **Existing stores.** The stored model id stays `all-MiniLM-L6-v2`. A local
  store first filled between 2026-09-27 and this change may hold rows over 256
  tokens embedded from their first 256 tokens. The weights are identical, so
  those vectors share the vector space with new ones (as every row longer than
  the limit always has); they are a truncated representation, not a different
  model. Changing the id would key away the committed replay fixtures and make
  every local store refuse to start until re-embedded, so it was not done.

### Pinned rerun against the frozen September runs

Diagnostic reruns on the pin branch. "Exact" means per-question ranking order,
scores and evidence recall match the frozen September JSONL on every question;
the only added field is `channel_scores`.

| Benchmark (metric) | Run | Frozen (Sept) | Pinned rerun | Per-question |
|---|---|---:|---:|---|
| LoCoMo (evidence recall@10) | parity, 512 | 0.726 | 0.726 | exact, 1,986/1,986 |
| LoCoMo (evidence recall@10) | parity, 256 | 0.603 | 0.603 | exact, 1,986/1,986 |
| LoCoMo (evidence recall@10) | native, 512 | 0.917 | 0.917 | exact, 1,986/1,986 |
| LongMemEval-S (R@5) | parity, 512 | 0.960 | 0.960 | exact, 500/500 |
| LongMemEval-S (R@5) | native, 512 | 0.980 | 0.980 | exact, 500/500 |
| ConvoMem (recall@10) | parity, 512 | 0.929 | 0.929 | exact, 250/250 |
| ConvoMem (recall@10) | native, 512 | 0.913 | 0.913 | exact, 250/250 |

MemBench was not rerun (about an hour per run); its messages are short, and it
was unchanged across every column above.

### Production setting: keep 512

The measured evidence favors keeping 512 tokens. It is the documented default
behind every committed baseline. It is better wherever inputs are long
(LoCoMo native 0.917 versus 0.900 at 256, parity 0.726 versus 0.603;
LongMemEval native R@5 0.980 versus 0.978), and
equal wherever they are short (ConvoMem, MemBench, and the semantic-gate
corpus). 256 remains available explicitly (`--embed-max-tokens 256`) as the
MemPalace-parity setting. Nothing was tuned; the frozen September artifacts
are unchanged.
