# W4.1 production-embedding semantic gate preregistration

Status: **frozen before the first W4.1 holdout gate run** on 2026-07-12.

This document fixes the inputs, metrics, floors, comparisons, and decision rules
before inspecting the W4.1 holdout result. Later result artifacts must link this
file and may report outcomes; they must not revise it. A floor change requires a
new dated preregistration and a fresh, untouched holdout.

## Frozen primary evaluation

- Corpus: `crates/rb-eval/fixtures/corpus.json`, SHA-256
  `7d33087001142b95097551e564fe18d6ae9cd6e474e9d7c5779c2358acc9102e`.
- Corpus shape: exactly 205 memories, 72 tuning/golden queries, and 8 authored
  near-duplicate clusters.
- Holdout: `crates/rb-eval/fixtures/holdout_queries.json`, SHA-256
  `81fe9057a4e91ececfb0cb50f0c001c19229e377f3f621648c9ea47e79df37d0`.
- Holdout shape: exactly 20 graded queries. The file and its relevance grades
  are immutable for this gate. W4.1 reports aggregates only; no per-query
  holdout ranking may be emitted during tuning.
- Primary production-capable model: the supported local
  `all-MiniLM-L6-v2` provider, dimension 384. Its fixture is recorded once from
  the frozen inputs; its SHA-256 is added to the machine-readable gate manifest
  immediately after recording and before executing retrieval metrics.
- Secondary comparison: Voyage, only when an existing `VOYAGE_API_KEY` is
  available. Absence of a credential is reported, never replaced by an
  unbounded or user-funded call. Voyage cannot become the default without its
  own frozen replay fixture passing the same floors.

### Embedding input contract

- Every memory uses the production composite document representation and
  `EmbedKind::Document`.
- Every golden and holdout query uses the verbatim authored query and
  `EmbedKind::Query`.
- Replay must find the exact `(model_id, input_kind, sha256(text))` entry.
  Query-to-document fallback, duplicate fixture keys, unknown input kinds,
  wrong dimensions, non-finite vector values, or any missing vector are hard
  failures.
- Offline replay is the CI/scheduled gate. Live recording is manual and
  bounded to the frozen input set; CI never uses network credentials.

## Preregistered primary floors

The golden and untouched holdout aggregates must each satisfy every floor:

| Metric | Floor |
|---|---:|
| mean recall@5 | 0.80 |
| MRR | 0.70 |
| mean NDCG@5 | 0.75 |
| dedup precision@5 | 0.90 |
| FTS query contribution rate | 0.80 |
| vector query contribution rate | 0.95 |
| graph query contribution rate | 0.00 (reported, not quality-gating) |
| archived/superseded/secret/poison exposure | exactly 0 |
| replay misses or input-kind fallbacks | exactly 0 |

Recall latency p50/p99, fixture bytes, returned rows, approximate context
tokens, recording request count, and provider cost are diagnostics. They are
not portable enough for a blocking floor until the scale benchmark establishes
a pinned runner envelope.

## Arms and decision rules

The primary comparison is current `Linear` versus current `Rrf` over identical
frozen inputs. Exact-keyword/operational-fact, pure vector, recency-only, and
importance-only lanes are diagnostic baselines. A bounded exact-evidence lane
and shadow novelty/importance/confidence arms stay evaluation-only; this task
does not change production storage, retention, or default ranking.

The five fixed chronological evaluation instants are:

1. `2026-01-01T00:00:00Z`
2. `2026-02-01T00:00:00Z`
3. `2026-03-01T00:00:00Z`
4. `2026-04-01T00:00:00Z`
5. `2026-05-01T00:00:00Z`

The first-release dogfood decision is:

- **Go with Linear unchanged** only if the primary local-model golden and
  holdout gates pass with zero safety exposure.
- **Flip the default to RRF** only in a separate reviewed change if RRF passes
  every floor at all five instants, causes no aggregate recall/MRR/NDCG/dedup
  regression larger than 0.01 versus Linear, improves at least one of those
  metrics by at least 0.01, and introduces no safety exposure.
- Otherwise **no-go for a default change**; retain Linear and use the failed
  stratum/channel diagnostics to plan the next retrieval change. The holdout
  is retired and replaced before any tuning loop informed by its result.

## Offline-only robustness strata

The gate reports, without mutating production behavior:

1. exact operational facts and literal evidence spans;
2. semantic questions requiring one or several memories;
3. superseded, archived, contested, and near-duplicate lifecycle cases;
4. irrelevant noise plus secret-shaped and instruction/poison-shaped content.

Per stratum it records recall@5, MRR, NDCG@5, exact-span/answer success, safety
exposure, channel contribution, returned rows, context bytes/tokens, and
latency p50/p99. Any absent stratum remains explicitly `not_measured`; it is
never silently treated as a pass.
