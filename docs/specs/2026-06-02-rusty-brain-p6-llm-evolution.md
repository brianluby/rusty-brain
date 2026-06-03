# rusty-brain — P6: LLM-Assisted Evolution — Design Spec

- **Status:** Draft (brainstormed and approved; pending written-spec review)
- **Date:** 2026-06-02
- **Author:** Brian Luby
- **Depends on:** P0–P4, **and P5** (reuses the `Reembed` primitive, composite `embedding_input`, the `rb-eval` harness, and complements P5's confidence/contradiction surfacing)
- **References:** `docs/specs/2026-05-31-rusty-brain-architecture-design.md` (§8 broadcast, §9 data model, §11 ranking, §17 P3 evolution jobs), `docs/specs/2026-06-02-rusty-brain-p5-retrieval-quality.md`. Derived from the verified research survey (sources in §9).

---

## 1. Context & Motivation

P3 shipped LLM-free evolution jobs (link decay, cosine-threshold consolidation, importance recalibration) behind the daemon's job scaffolding. The research survey shows the next tier of value in the field is **LLM-assisted** evolution — used by A-MEM, Mem0, and Generative Agents — which rusty-brain can adopt *without* compromising its LLM-free, low-latency default path, because the machinery already exists:

- `rb-enrich` already speaks an OpenAI-compatible endpoint (`crates/rb-enrich/src/openai_compat.rs`) and already proposes links via an LLM (`linker.rs`).
- The jobs scaffolding (`run_once`, `JobKind`, `JobsConfig` TOML, the in-daemon scheduler, `Request::RunJob`) already drives opt-in, off-by-default, bounded, fail-safe jobs through the single writer.
- The change-broadcast (`MemoryChanged` on `tokio::broadcast`, architecture spec §8) already carries every committed write — an untapped trigger source.
- P5 adds the `Reembed` primitive these jobs need when they mutate a memory's text.

Three gaps the current jobs cannot address:

1. **Consolidation can only merge, never reconcile.** Today's job clusters by cosine ≥ a threshold (`crates/rb-store/src/store.rs:282` `near_duplicates`) and picks a survivor by importance/recency (`jobs/consolidation.rs::pick_survivor`). It cannot decide *update vs supersede vs keep-both*, and cannot detect that two close memories actually *contradict*.
2. **Consolidation removes redundancy but never creates higher-level knowledge.** Generative Agents periodically *reflect*: cluster recent memories and synthesize an insight. rusty-brain already has an `insight` memory type and a `references` link type — the data model is ready, the job is missing.
3. **The graph signal is crude.** Ranking uses `1/(1+hops)` over a fixed-depth CTE. HippoRAG shows graph-centric retrieval (Personalized PageRank seeded by vector hits) surfaces structurally-central memories that hop-distance misses.

## 2. Goals

1. An opt-in **`reconcile`** job: LLM-decided ADD/UPDATE/SUPERSEDE/NOOP over near-duplicate clusters.
2. An opt-in **`reflect`** job: synthesize higher-level `insight` memories from clusters of recent high-importance memories, with an importance-accumulation trigger.
3. A **Personalized PageRank** graph signal for ranking, computed over the existing SQLite link graph.
4. Everything off by default, bounded, idempotent, fail-safe, single-writer; **no new default runtime dependencies** (PPR is pure; the LLM jobs reuse `rb-enrich`).

## 3. Non-Goals

- No LLM on the default read or write path. Reconcile/reflect run only when explicitly enabled.
- No external graph database (PPR runs over `memory_links` in SQLite — Zep/Graphiti's Neo4j model is rejected as non-local-first).
- No replacement of the LLM-free P3 jobs — the LLM jobs *complement* them; the operator chooses.
- No cross-namespace operations — every job stays namespace-isolated and fails closed, exactly as P3.

## 4. Locked Decisions

| Decision | Choice | Rationale |
|---|---|---|
| LLM jobs default state | **Off by default; opt-in via `JobsConfig` TOML** | Preserves the LLM-free, low-latency default; respects the fail-open capture rule and the dependency budget. |
| LLM transport | **Reuse `rb-enrich` OpenAI-compatible client** | No new dependency; one place for endpoint/key/timeout/masking. |
| `reconcile` vs `consolidation` | **Complementary, not a replacement** | Operator enables one; cosine consolidation stays the LLM-free option. |
| `reflect` trigger | **Interval OR importance-accumulation off the change-broadcast** | Mirrors Generative Agents' cumulative-importance reflection; reuses existing broadcast, no new write path. |
| Content-mutating writes | **Go through the writer + P5 `Reembed`** | Single-writer discipline; mutated text stays consistent with its vector. |
| Graph signal | **Personalized PageRank, pure, over the bounded subgraph** | Local-first; no new deps; documented scale bound like sqlite-vec. |
| Idempotency under LLM non-determinism | **Watermarks + supersede/state markers bound re-processing** | A second pass over unchanged data does no work despite a non-deterministic model. |

## 5. Build Order

```text
F  Personalized PageRank graph signal   (pure rb-search; independent; eval-validated first)
D  LLM reconcile job                     (jobs + rb-enrich + P5 Reembed)
E  LLM reflect/synthesis job             (jobs + rb-enrich + broadcast trigger)
```

F is sequenced first because it is pure, dependency-free, and immediately measurable via `rb-eval`; D and E are independent of each other and both reuse P5's `Reembed`.

---

## 6. Feature F — Personalized PageRank graph signal

**Crate:** `rb-search` (pure; no new deps), with a bounded subgraph read from `rb-store`.

**Design:** replace/augment the `1/(1+hops)` graph term with a Personalized PageRank score:
- **Personalization vector:** the recall seeds (FTS + vector hits) weighted by their pre-graph scores — PPR restarts toward the seeds, so structurally-central memories *near the seeds* score highest.
- **Graph:** the `memory_links` adjacency, edges weighted by `strength` (the `0..1` column already on every link). Damping `d = 0.85` (documented, configurable).
- **Computation:** power iteration to a fixed iteration cap or convergence epsilon (documented), over a **bounded subgraph** — nodes reachable within N hops of the seeds, capped at a configured node budget. Stable node ordering ⇒ deterministic output.
- **Output:** a normalized PPR score per candidate, fed into ranking as the graph signal (the Stage-2 graph prior under P5's `Rrf`, or the graph term under `Linear`).

**Data flow:** recall gathers seeds → load the bounded link subgraph around them → run PPR → attach `graph_ppr` to `Signals` → rank.

**Scale honesty (documented):** PPR is bounded by the subgraph node budget; beyond it the graph signal degrades gracefully to the seed neighborhood. Stated as a known limit, like the sqlite-vec brute-force ceiling (architecture spec §11).

**Error handling:** pure computation; a missing/empty graph yields a zero graph signal (no penalty), matching today's "missing signal = 0".

**Testing:** PPR unit tests on hand-computed small graphs (known stationary distributions), convergence, determinism, strength-weighting, and seed sensitivity; `rb-eval` graph-recall scenario comparing `1/(1+hops)` vs PPR (deterministic vectors are fine here — graph structure, not embeddings, drives the result).

## 7. Feature D — LLM `reconcile` job

**Crates:** `rb-daemon/src/jobs` (new job arm), `rb-enrich` (reconcile decision call), reusing P5 `Reembed`.

**Design:** a new `JobKind::Reconcile` with its own `run_once` arm and `ReconcileConfig` (in `JobsConfig`, `enabled = false`). For each near-duplicate cluster (reusing `near_duplicates`, namespace-scoped), call an `rb-enrich` function that returns one decision per cluster:
- **MERGE** — synthesize merged content; insert a new memory; supersede the members into it.
- **UPDATE** — rewrite the survivor's content to subsume the rest; supersede the others.
- **SUPERSEDE** — one member supersedes another (staleness/contradiction), keep the survivor.
- **NOOP** — distinct despite vector closeness; record a "reconciled" marker so the pair is not re-examined.

All writes funnel through the single writer: `Insert`/`Update`/`Supersede` + `Reembed` (content changed ⇒ vector must follow). Reuses `pick_survivor`-style determinism for any non-LLM tiebreak.

**Config:** `ReconcileConfig { enabled: bool, batch_limit: usize, similarity_floor: f32, model/endpoint via rb-enrich env }`. Default disabled. The cosine `consolidation` job is unchanged and remains the LLM-free option; operators enable at most one.

**Idempotency under LLM non-determinism:** only clusters above `similarity_floor` are considered; a processed pair is marked (reconciled watermark / superseded state) so a second pass over unchanged data does nothing, even though the model itself is non-deterministic. Documented as a caveat.

**Error handling:** fail-safe — an LLM/network error logs and skips the cluster, never fatal (reuse the writer's `catch_unwind` + reopen path). Bounded `batch_limit` per pass. API key never logged (existing `rb-enrich` masking).

**Testing:** in-process with `wiremock` (already a dep) serving canned decisions; assert the exact writer commands per decision (MERGE/UPDATE/SUPERSEDE/NOOP); idempotency (second pass writes nothing); namespace isolation (never merges across namespaces); fail-safe (HTTP error → cluster skipped, job summary records it). Real-model test `#[ignore]`.

## 8. Feature E — LLM `reflect` / synthesis job

**Crates:** `rb-daemon/src/jobs` (new job arm + trigger), `rb-enrich` (synthesis call), `rb-daemon` scheduler (accumulation trigger), reusing composite embedding from P5.

**Design:** a new `JobKind::Reflect` with `ReflectConfig` (`enabled = false`). Per namespace, cluster recent high-importance memories (since a `last_reflected_at` watermark, bounded by `batch_limit`), call `rb-enrich` to synthesize an `insight`-type memory, insert it (importance/confidence from the synthesis), and add `references` links from the insight to its source memories. The new insight is embedded via P5's composite `embedding_input`. **Non-destructive** — it only adds a memory and links.

**Trigger:** two modes, both opt-in:
- **Interval** — the existing scheduler, like the P3 jobs.
- **Importance-accumulation** — the daemon subscribes to its own `MemoryChanged` broadcast and sums the importance of `Created` events per namespace; when the running sum crosses `importance_threshold` (default ~150, à la Generative Agents' reflection trigger), it enqueues a `Reflect` run and resets the counter. This adds *only* a subscriber + counter to the scheduler — no new write path.

**Config:** `ReflectConfig { enabled: bool, importance_threshold: u32, batch_limit: usize, window, model/endpoint via rb-enrich env }`.

**Idempotency / convergence:** a `last_reflected_at` watermark (in `meta`, per namespace) prevents re-reflecting the same window; reflecting a window with nothing new is a no-op. The synthesized insight is ordinary memory, so it is itself subject to recall, decay, and consolidation.

**Error handling:** fail-safe (synthesis error → log, skip, never fatal); bounded batch; key masked.

**Testing:** in-process with `wiremock` returning a canned synthesis; assert one `insight` memory inserted with `references` links to the correct sources; trigger unit test (accumulated importance crosses the threshold → exactly one run enqueued, counter resets); watermark test (second run over the same window does nothing). Real-model test `#[ignore]`.

---

## 9. Sources (verified research)

- A-MEM (memory evolution: LLM updates neighbors' context/keywords/tags on insert, then re-embeds) — <https://arxiv.org/abs/2502.12110>
- Mem0 / Mem0g (LLM ADD/UPDATE/DELETE/NOOP over top-s; directed labeled graph; recency-favoring conflict resolution) — <https://arxiv.org/html/2504.19413v1>
- Generative Agents (reflection synthesizes higher-level insights; triggered when cumulative importance exceeds a threshold) — <https://ar5iv.labs.arxiv.org/html/2304.03442>
- HippoRAG (LLMs + KG + Personalized PageRank; vectors only seed) — <https://arxiv.org/abs/2405.14831>
- Zep / Graphiti (n-hop traversal + reranking; bi-temporal supersede) — <https://arxiv.org/html/2501.13956v1>

## 10. Testing Strategy (phase-wide)

- TDD; workspace `deny` lints unchanged.
- LLM jobs tested in-process against `wiremock`; **no live network in CI**; real-model tests `#[ignore]`.
- All jobs: off-by-default, bounded, idempotent, fail-safe, namespace-isolated — asserted per the P3 jobs test pattern.
- F validated by `rb-eval` (P5) graph-recall comparison.
- Per-phase gate: `cargo test --workspace`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo fmt --all --check`; `cargo deny check` (no new default deps expected — verify).

## 11. Risks & Mitigations

| Risk | Mitigation |
|---|---|
| LLM non-determinism breaks idempotency | Watermarks + supersede/reconciled markers; act only above `similarity_floor`; documented caveat. |
| `reconcile` destroys information via a bad MERGE | Supersede (soft, reversible) not hard-delete; namespace-isolated; bounded batch; real-model review before enabling. |
| Reflection floods the store with low-value insights | Importance-accumulation trigger gates frequency; insights are normal memories subject to decay/consolidation; `batch_limit`. |
| PPR cost on large graphs | Bounded subgraph + iteration cap; documented scale limit; measured via `rb-eval` latency. |
| Cost/latency of LLM jobs | Off by default; bounded batches; timeouts via `rb-enrich`; operator opts in knowingly. |

## 12. Traceability

| Research finding | P6 feature |
|---|---|
| Mem0 LLM ADD/UPDATE/DELETE/NOOP over similar memories | D — reconcile job |
| A-MEM evolves + re-embeds neighbors on insert | D — reconcile + P5 `Reembed` |
| Generative Agents reflection (synthesize insights; cumulative-importance trigger) | E — reflect job + accumulation trigger |
| HippoRAG Personalized PageRank seeded by vector hits | F — PPR graph signal |
| Zep no-LLM-at-query-time default | F runs at query time and is LLM-free; LLM stays in opt-in write-time jobs (D/E) |
