# rusty-brain — P5: Retrieval Quality & Measurement — Design Spec

- **Status:** Draft (brainstormed and approved; pending written-spec review)
- **Date:** 2026-06-02
- **Author:** Brian Luby
- **Depends on:** P0–P4 (workspace, store, engine, daemon, MCP, enrichment, evolution jobs, agent surface)
- **References:** `docs/specs/2026-05-31-rusty-brain-architecture-design.md` (§9 data model, §10 embeddings, §11 search/ranking, §15 testing). Derived from a verified deep-research survey of A-MEM, Mem0/Mem0g, Zep/Graphiti, HippoRAG, and Generative Agents (sources in §10).

---

## 1. Context & Motivation

A research survey of the agentic-memory field (24/25 extracted claims verified unanimously against primary papers) confirmed rusty-brain's design is on solid ground — its recency+importance+relevance ranking mirrors Generative Agents, and its evolution jobs mirror the field's first-class treatment of consolidation/forgetting. The survey also surfaced four concrete, low-risk improvements that the current code does not yet implement, and one meta-lesson: **the popular memory benchmark (LOCOMO) is unreliable** (an audit found 64% of its answer key wrong), so quality changes must be validated against our *own* measurement, not borrowed leaderboards.

This phase delivers that measurement harness first, then three retrieval-quality improvements gated by it. Concrete gaps in today's code:

- **Vectors are built from raw `content` alone.** `rb-engine` computes a summary, keywords, tags, and context during enrichment, then embeds only `note.content` (`crates/rb-engine/src/engine.rs:147`), discarding query-aligned signal. A-MEM embeds the *composite* (content + keywords + tags + context) and retrieves better for it.
- **Ranking is a weighted linear sum of normalized scores** (`crates/rb-search/src/rank.rs`), which assumes cosine-similarity, reciprocal-keyword-rank, and reciprocal-graph-hops are on comparable, calibrated scales. They are not. Reciprocal Rank Fusion (RRF) — the de-facto hybrid-search standard, used by Zep — fuses *rank positions* and is scale-free.
- **`confidence` is stored but never used at query time.** The `Signals` struct in `rank.rs` has no confidence field. A single wrong, high-matching memory can dominate recall — the **context-poisoning** failure mode.
- **Contradictions are invisible at recall.** `LinkType::Contradicts` exists and is writable, but recall/get never tell the agent that a returned memory is contested.

## 2. Goals

1. A reproducible, offline, CI-gated **regression harness** (`rb-eval`) that measures recall quality and guards against ranking regressions.
2. **RRF fusion** available as a configurable, eval-comparable alternative to linear ranking.
3. **Composite embeddings** (enriched representation) with a safe, idempotent re-embed transition.
4. **Confidence-weighted ranking** and **contradiction surfacing** as context-poisoning mitigations, using data already in the schema.
5. Zero new default runtime dependencies. No change to the single-writer discipline, namespace isolation, or fail-closed/ fail-open rules.

## 3. Non-Goals

- No chasing of external leaderboards (LOCOMO and similar are explicitly distrusted; we build our own fixtures).
- No cross-encoder / neural reranker in the default build (local-first; reserved behind a feature if ever).
- No LLM on the default read or write path (LLM-assisted evolution is P6).
- No multi-host or networked surface.

## 4. Locked Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Eval harness scope | **Offline regression harness** (committed fixtures + `DeterministicProvider`, CI gate; optional `#[ignore]` real-model mode) | Lean, fast, deterministic; guards regressions and relative ordering without network. Absolute semantic quality is the real-model mode's job. |
| RRF rollout | **Opt-in; `Linear` stays default; eval-gated flip** | Backward-compatible and data-driven — flip the default only when `rb-eval` shows RRF wins. |
| RRF design | **Two-stage hybrid** (RRF fuses FTS/vector/graph rank lists → importance/recency/confidence applied as priors) | Compares linear against a *real* RRF design, not a strawman; keeps the metadata priors that already work. |
| Composite embedding transition | **`embedding_input_version` stamp + `Reembed` write command + `rusty-brain reembed` batch command** | One idempotent mechanism covers the corpus transition, future model swaps, and P6's per-memory re-embed. Matches A-MEM/Mem0 (re-embed on mutation) + vector-DB practice (batch re-index). |
| Confidence in ranking | **Multiplicative dampener with a floor** | A low-confidence memory is suppressed regardless of match quality (the poisoning goal), but never fully zeroed. |
| Contradiction surfacing | **Read-side annotation, fail-open** | Uses the existing `contradicts` link; a lookup failure degrades to unflagged results, never fails recall. |

## 5. Build Order

`H → B → A → C`, then a phase gate. H is first so B/A/C are measurable; the others are independent of each other but all consume H.

```text
H  rb-eval offline regression harness        (gates everything below)
B  RRF two-stage fusion mode                  (pure rb-search; eval-compared)
A  composite embedding + reembed primitive    (engine + store + meta + CLI)
C  confidence dampener + contradiction flag   (rb-search + engine + proto/mcp)
```

---

## 6. Feature H — `rb-eval` offline regression harness

**Crate:** new `crates/rb-eval` — a dev/measurement crate, **excluded from the `rusty-brain` binary's dependency closure** (not a workspace default-member of the binary build path; built and run in CI and locally). Depends on `rb-engine`, `rb-store`, `rb-search`, `rb-embed` (`DeterministicProvider`).

**Components:**
- `fixtures/` — committed JSON: a small coding-memory corpus (typed notes with content + enrichment fields), a set of golden queries each with expected-relevant `memory_id`s, and near-duplicate clusters for dedup scoring. Authored to exercise FTS, vector, and graph paths.
- `corpus.rs` — fixture loader and validation (fail fast on malformed fixtures).
- `metrics.rs` — pure functions: `recall_at_k`, `mrr`, `dedup_precision`, latency percentiles (p50/p99). Unit-tested independently.
- `runner.rs` — builds an in-memory store, ingests fixtures through `rb-engine` with `DeterministicProvider`, runs each golden query through recall, computes metrics, and asserts each metric ≥ its committed baseline.
- `baselines.json` — committed metric baselines; CI fails on regression. Updating a baseline is a reviewed, intentional commit.

**Data flow:** load fixtures → `engine.remember` each (deterministic vectors) → for each golden query, `engine.recall` → compute metrics vs expected → assert ≥ baseline.

**Honest framing (documented in the crate README):** offline + `DeterministicProvider` guards **ranking determinism, regression detection, and relative-ordering invariants** ("did change X reorder results vs the committed baseline?"). It does **not** measure absolute semantic quality — that requires the optional `#[ignore]` real-model mode (Voyage or `local`), run manually for spot checks. This limit is stated explicitly, in the spirit of the architecture spec's sqlite-vec scale honesty (§11).

**Error handling:** test-only crate; failures are assertion failures with readable diffs (expected vs actual ranking). The crate may `#![allow(clippy::unwrap_used, clippy::expect_used)]` as test code; no panics leak into shipped crates.

**Testing:** the harness *is* the test; plus unit tests for each metric function on hand-checked inputs.

## 7. Feature B — RRF two-stage hybrid fusion

**Crate:** `rb-search` only (pure functions; no new deps).

**Design:** add `FusionMode { Linear, Rrf }`. `Linear` remains the default and is the existing `rank()`. `Rrf` is a new path:
- **Stage 1 — rank fusion.** Each retrieval path (FTS, vector, graph) contributes a ranked candidate list. RRF score per candidate = `Σ_paths 1 / (k + rank_path)` with a documented default `k = 60`. Missing from a list ⇒ that path contributes nothing (no penalty), matching today's "missing signal = 0" rule. Rank for the vector path derives from ascending distance; for graph, from ascending hops.
- **Stage 2 — priors.** Apply `importance`, `recency` (the existing 30-day-half-life exponential), and `confidence` (Feature C) as multiplicative priors on the fused score. Documented, configurable weights; pure and deterministic (stable sort, `total_cmp`).

**Configuration:** `FusionMode` selected via the existing weights/config plumbing read by the engine on recall. Default `Linear` preserves current behavior byte-for-byte.

**Data flow:** candidates (already carrying `keyword_rank`, `vector_distance`, `graph_hops` in `Signals`) → derive per-path ranks → RRF fuse → apply priors → sort → truncate to `limit`.

**Testing:** `rank_rrf` unit tests (RRF arithmetic on known lists, determinism, prior application, non-finite-input sanitization mirroring `rank()`); `rb-eval` runs the full fixture set under both modes and reports the metric delta. The default flips to `Rrf` only in a later commit if and when the eval shows a win.

## 8. Feature A — composite embedding input + re-embed primitive

**Crates:** `rb-engine` (embed input), `rb-store` + `rb-daemon` (re-embed write path + stamp), `rusty-brain` (CLI).

**Embed input:** a pure `embedding_input(note) -> String` (in `rb-engine`) composes `content` + keywords + tags + context (newline-joined, documented order). It replaces `note.content` at `engine.rs:147`. Enrichment already runs before the embed call, so the fields are populated. **The query stays embedded raw** — only the *document* representation changes (A-MEM does the same; query symmetry is not required).

**Schema / invariants (additive migration):**
- New `meta` key `embedding_input_version` (e.g. `"v2-composite"`), seeded at init, sitting alongside the existing `embedding_model` / `embedding_dim` invariants (architecture spec §9).
- New `memories.embedding_input_version` column (TEXT) stamped per row at write, alongside the existing `embedding_model` stamp. Additive, file-discovered, checksummed migration — must pass the existing CI reproducibility gate (§15).

**Re-embed primitive:**
- New `WriteCommand::Reembed { id }` in `crates/rb-daemon/src/store_handle.rs` and a store path that recomputes a single memory's vector and `UPDATE`s `memory_vectors` (today vectors are write-once via `INSERT` at `store.rs:818`; this adds the update path the system has so far avoided by making content immutable, `engine.rs:386`).
- `rusty-brain reembed` CLI → a `Request::RunJob`-style daemon request that batch-scans active memories whose stamped `(embedding_model, embedding_input_version)` ≠ current and re-embeds them through the single writer. **Bounded and idempotent** (a row already at current stamps is skipped; a second run over unchanged data writes nothing) — same discipline as the evolution jobs.

**Transition semantics:** after upgrading, the corpus is mixed (old vectors built from content-only, new from composite) until `reembed` runs. This is tolerated and documented; recall still functions (cosine space is unchanged; only document text representation shifts). `reembed` is the explicit, user-triggered convergence step.

**Rationale (from research):** comparable systems re-embed *on mutation, per-memory* (A-MEM on evolution, Mem0 on UPDATE) and *batch re-index* on a global model/template change (general vector-DB practice). There is no "lazy on access" pattern in this class; it is unnecessary here because content is immutable today. The same `Reembed` primitive is what P6's content-mutating jobs (reconcile/reflection) will call.

**Error handling:** embedding failures surface via the existing `Error::Embedding` path; `reembed` batches fail-safe (a failed row is logged and retried next run, never fatal). Dim contract (`seed_or_verify_dim`) is unchanged and still fail-closed.

**Testing:** `embedding_input` pure-fn unit tests (field composition, ordering, empty fields); `Reembed` idempotency + stamp-skip tests; migration reproducibility test (fresh DB exercises the new column); `rb-eval` recall comparison content-only vs composite under the real-model `#[ignore]` mode (deterministic vectors can't show semantic lift, so this comparison is real-model-only and documented as such).

## 9. Feature C — confidence-weighted ranking + contradiction surfacing

**Crates:** `rb-search` (confidence factor), `rb-store`/`rb-engine` (contradiction lookup + annotation), `rb-proto`/`rb-mcp`/`rusty-brain` (result-shape field + display).

**Confidence dampener:** add `confidence: f32` to the `Signals` struct and apply it as a multiplicative dampener on the final score: `score *= floor + (1 - floor) * confidence`, with a documented, configurable `floor` (e.g. `0.5`) so a low-confidence memory is meaningfully suppressed but never fully zeroed. Applies in both `Linear` and `Rrf` (as a Stage-2 prior). Pure and deterministic.

**Contradiction surfacing:** add a boolean `contested` to recall/list/context result rows and the `get` payload, set when the memory has at least one *active* `contradicts` link (inbound or outbound). Computed read-side over `memory_links` for the result set after ranking (reuse the existing link-loading path). **Fail-open:** if the contradiction lookup errors, results are returned unflagged rather than failing recall — surfacing contested state is best-effort enrichment, never a gate on retrieval.

**Wire/contract:** the new `contested` field is additive to the MCP result schema and CLI output; `ContractVersion` (architecture spec §12) bumps so clients can detect the richer shape. Defaulting absent → `false` keeps older clients correct.

**Data flow:** recall → rank (confidence-aware) → for the top `limit` results, batch-load `contradicts` links → set `contested` → return.

**Testing:** confidence-dampening unit tests (low-confidence memory ranks below an otherwise-equal high-confidence one; floor honored); a "poison" scenario in `rb-eval` (a low-confidence wrong memory must rank below the high-confidence correct one); a contradiction integration test (create A, create B, link `A contradicts B`, recall → both flagged `contested`); fail-open test (forced lookup error → results returned, unflagged).

---

## 10. Sources (verified research)

- A-MEM (atomic notes; composite embedding; evolve-and-re-embed neighbors on insert) — <https://arxiv.org/abs/2502.12110>
- Mem0 / Mem0g (ADD/UPDATE/DELETE/NOOP; re-embed on UPDATE) — <https://arxiv.org/html/2504.19413v1>
- Zep / Graphiti (hybrid retrieval reranked by RRF/MMR/cross-encoder) — <https://arxiv.org/html/2501.13956v1>, <https://neo4j.com/blog/developer/graphiti-knowledge-graph-memory/>
- HippoRAG (graph-centric retrieval) — <https://arxiv.org/abs/2405.14831>
- Generative Agents (recency·importance·relevance retrieval) — <https://ar5iv.labs.arxiv.org/html/2304.03442>
- LOCOMO benchmark audit (answer-key unreliability) — <https://dev.to/penfieldlabs/we-audited-locomo-64-of-the-answer-key-is-wrong-and-the-judge-accepts-up-to-63-of-intentionally-33lg>
- Context poisoning (failure mode motivating confidence/contradiction defenses) — <https://www.agentpatterns.tech/en/failures/context-poisoning>

## 11. Testing Strategy (phase-wide)

- TDD throughout; workspace lints stay `deny` for `unwrap_used`/`expect_used`/`panic` outside tests.
- `rb-eval` is the CI regression gate for ranking changes (B, A, C). Real-model comparisons are `#[ignore]`, run manually.
- New migration (A) must pass the existing fresh-DB reproducibility gate (architecture spec §15).
- No live network in CI; deterministic fixtures and `DeterministicProvider` only.
- Per-phase gate: `cargo test --workspace`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo fmt --all --check`. A-adds-a-dep? No new deps in P5 — `cargo deny check` still runs and must stay green.

## 12. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Deterministic vectors can't prove semantic lift for composite embeddings (A) | Real-model `#[ignore]` comparison; offline harness guards regressions, not absolute quality (documented). |
| RRF tuned to fixtures, not real corpora | `k` documented and configurable; flip-the-default decision requires real-model eval, not just fixtures. |
| `Reembed` introduces the first vector-update path (previously write-once) | Bounded, idempotent, single-writer; integration tests for idempotency and the dim contract. |
| `contested` flag adds recall latency | Batched single lookup over the result set only; fail-open; measured by `rb-eval` latency metric. |

## 13. Traceability

| Research finding | P5 feature |
|---|---|
| A-MEM embeds composite, not content alone | A — composite `embedding_input` |
| A-MEM/Mem0 re-embed on mutation; vector-DBs batch re-index | A — `Reembed` primitive + `reembed` command |
| Zep reranks with RRF | B — RRF two-stage fusion |
| Generative Agents = recency·importance·relevance | (already in `rank.rs`); B priors preserve it |
| Context poisoning; confidence unused at query time | C — confidence dampener |
| `contradicts` link writable but invisible at recall | C — contradiction surfacing |
| LOCOMO answer key 64% wrong; SOTA claims contested | H — own offline eval, not borrowed leaderboards |
