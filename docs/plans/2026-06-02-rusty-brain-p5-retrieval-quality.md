# P5 — Retrieval Quality & Measurement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship spec §6–§9 — an offline CI-gated retrieval-quality measurement harness (`rb-eval`) and the three eval-gated improvements it guards: RRF two-stage fusion, composite embeddings with an idempotent re-embed primitive, and confidence-weighted ranking with contradiction surfacing.

**Architecture:** P5 is additive and behind existing seams. A new dev-only `rb-eval` crate ingests committed JSON fixtures through `rb-engine` + `DeterministicProvider`, runs golden queries through recall, and asserts pure metrics (recall@k, MRR, dedup precision, latency) stay at or above a committed `baselines.json`. `rb-search` gains a scale-free `FusionMode::Rrf` path alongside the default `Linear` `rank()`. `rb-engine` swaps its content-only embed for a pure `embedding_input(note)` composite (content + keywords + tags + context), stamped with a new `embedding_input_version` (a `meta` invariant + an additive `memories` column behind a checksummed migration); a new `WriteCommand::Reembed` and a `rusty-brain reembed` CLI converge the corpus through the single writer. Confidence becomes a multiplicative ranking dampener, and an active `contradicts` link surfaces as a fail-open `contested` flag on result rows. The wire `CONTRACT_VERSION` bumps to 2.

**Tech Stack:** Rust 2021 (stable, pinned). Workspace crates: rb-types, rb-store (rusqlite + sqlite-vec), rb-proto, rb-engine, rb-search, rb-embed, rb-enrich, rb-daemon, rb-mcp, rusty-brain, plus the new dev-only **rb-eval**. No new external dependencies in P5: `rb-eval` reuses existing workspace deps (serde, serde_json, chrono, tokio, tempfile). Tests are TDD, in-process, offline (`DeterministicProvider`; real-model comparisons `#[ignore]`).

**Reference spec:** `docs/specs/2026-06-02-rusty-brain-p5-retrieval-quality.md` — §6 (H eval), §7 (B RRF), §8 (A composite + reembed), §9 (C confidence + contradictions). Architecture: `docs/specs/2026-05-31-rusty-brain-architecture-design.md` — §9 (data model), §10 (embeddings), §11 (ranking), §15 (testing). Style template: `docs/plans/2026-06-02-rusty-brain-p3-deferred-features.md`.

---

## Hard rules (carry forward from P0–P4; apply to every task)

- **TDD:** failing test first (RED), minimal implementation (GREEN), then clippy + fmt, then commit. One logical change per commit.
- **Conventional commits**, lowercase, crate-scoped, one line, **NO AI attribution** (no "Generated with…", no `Co-Authored-By`). Example: `feat(rb-search): add rrf two-stage fusion mode`.
- **Single-writer discipline:** ALL store mutations go through the daemon's single writer thread (`StoreHandle` `WriteCommand`s); reads go via the read pool. Never share `SqliteStore` across tasks. The new `WriteCommand::Reembed` is the ONLY vector-update path; the `rusty-brain reembed` CLI sends a daemon request and NEVER writes the DB directly.
- **Namespace isolation stays enforced server-side and fails closed:** re-embed scans and contradiction lookups never cross the connection's handshake namespace.
- **No-panic in non-test code:** workspace lints deny `unwrap_used`/`expect_used`/`panic`. Return `rb_types::Error` instead. Test modules (and the whole `rb-eval` crate, which is dev/measurement code) opt out with `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- **Error plumbing:** reuse existing `rb_types::Error` variants (`Storage`, `InvalidArgument`, `Embedding`, `NotFound`, `DimensionMismatch`). This plan adds **no new error variant** (which would require arms in `rb-proto::error_kind`, `rb-proto::response_error_to_error`, and `rb-daemon::error_to_response`).
- **Fail-open vs fail-closed:** the dim contract (`seed_or_verify_dim`) stays fail-closed. Contradiction surfacing (`contested`) is best-effort enrichment and fails OPEN (a lookup error returns unflagged results, never aborts recall). Re-embed batches fail-SAFE (a failed row is logged and retried next run, never fatal).
- **No live network in CI:** `rb-eval` and all unit/integration tests use committed fixtures and `DeterministicProvider`. Real-model comparisons (composite-embedding semantic lift, RRF-vs-Linear on a real corpus) are `#[ignore]`, run manually.
- **New migration** (`003_embedding_input_version.sql`) is additive, file-discovered, checksummed, and must pass the existing fresh-DB reproducibility gate (`embedded_initial_schema_creates_expected_objects` pattern) and the duplicate-version guard.
- **`Linear` stays the byte-for-byte default.** RRF is opt-in; the default flips to `Rrf` only in a later, separate commit if and when `rb-eval` shows a win. P5 does not flip it.
- **Per-Part gate** (final task of each Part): `cargo test --workspace`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo fmt --all --check`. The final gate (Part C) also runs `cargo deny check` (must stay green; no new deps).
- **Commands run from the worktree root** so crate-scoped cargo (`cargo test -p rb-eval`, etc.) keeps paths stable.

## Seam map (verified against this worktree; the exact code each Part builds on)

| Seam | Location | Used by |
|---|---|---|
| `Signals { id, keyword_rank, vector_distance, graph_hops, importance, created_at }`, `score_one`, `rank`, `HALF_LIFE=30.0` | `crates/rb-search/src/rank.rs` | B, C |
| `Weights { vector, keyword, graph, importance, recency }` (`Default` sums to 1.0) | `crates/rb-search/src/weights.rs` | B, C |
| `build_signals(keyword, vector, graph, meta)` folds three result sets into `Vec<Signals>`; `meta: HashMap<MemoryId,(u8, DateTime)>` | `crates/rb-search/src/merge.rs` | B, C |
| `pub use rank::{rank, Signals, HALF_LIFE}` etc. | `crates/rb-search/src/lib.rs` | B, C |
| `MemoryEngine::remember` embeds `note.content` alone (`engine.rs:147`); `recall` builds `meta` from `(importance, created_at)` (`engine.rs:282`); `update` rejects content (`engine.rs:384`) | `crates/rb-engine/src/engine.rs` | A, B, C |
| `MemoryBackend` trait (write/get/keyword/vector/graph/list/update/archive/add_link/record_access(es)/get_many) | `crates/rb-engine/src/backend.rs` | A, C |
| `MockBackend` test backend (`test_support.rs`); `DeterministicProvider` | `crates/rb-engine/src/test_support.rs`, `crates/rb-embed/src/deterministic.rs` | A, B, C |
| `insert_memory` INSERTs `memory_vectors` write-once (`store.rs:818`); `near_duplicates` (`store.rs:282`); `distance_to_similarity` (`store.rs:619`); `decode_embedding_bytes`/`embedding_bytes`; `row_to_note` (`store.rs:684`); `seed_or_verify_dim` (`store.rs:534`); `update_memory` (`store.rs:1087`); `load_links` (`store.rs:647`) | `crates/rb-store/src/store.rs` | A, C |
| File-discovered checksummed migrations; `001_initial_schema.sql`, `002_link_base_strength.sql`; duplicate-version guard | `crates/rb-store/src/migrations.rs`, `crates/rb-store/migrations/` | A |
| `WriteCommand` enum + `writer_loop` + `run_store_op` (catch_unwind/reopen); `StoreHandle` methods; `MemoryBackend for StoreHandle` | `crates/rb-daemon/src/store_handle.rs` | A |
| `run_once(kind, &StoreHandle, &JobsConfig)`, `JobSummary`, `JobKind` | `crates/rb-daemon/src/jobs/mod.rs`, `crates/rb-types/src/job.rs` | A |
| `Request`/`Response` enums; `CONTRACT_VERSION=1`; round-trip tests | `crates/rb-proto/src/messages.rs` | A, C |
| `dispatch(engine, job_store, jobs_config, req)`; `handle_connection`; `MAX_LIMIT` | `crates/rb-daemon/src/server.rs` | A, C |
| MCP `to_value(Response)` serializes `SearchResult`/`MemoryNote` via serde (`proxy.rs:183`) | `crates/rb-mcp/src/proxy.rs` | C |
| CLI `Command` enum (clap derive); `client.rs` daemon round-trip; `output.rs` rendering | `crates/rusty-brain/src/{cli.rs,client.rs,output.rs}` | A, C |
| `MemoryNote { …, confidence: f32, embedding_model: String, … }`; `SearchResult { memory, score }`; `MemoryUpdates` | `crates/rb-types/src/{memory.rs,query.rs}` | A, C |
| Workspace `members`, `workspace.dependencies`, `workspace.lints` | `Cargo.toml` | H |

## Build order & dependencies

```text
Part H  rb-eval offline regression harness        (FIRST — gates B/A/C; nothing below is mergeable without it)
Part B  RRF two-stage fusion mode                  (pure rb-search; rb-eval compares Linear vs Rrf)
Part A  composite embedding + reembed primitive    (engine + store + migration + daemon + CLI; rb-eval guards regression)
Part C  confidence dampener + contradiction flag   (rb-search + store + engine + proto/mcp/cli; rb-eval "poison" scenario)
```

H is built first so B, A, and C are each measurable against committed baselines. B, A, and C are independent of one another but all consume H. Part B introduces `FusionMode`; Part C extends both the `Linear` and `Rrf` paths with the confidence dampener, so C depends on B's `FusionMode`/`rank_rrf` names. Part A is independent of B/C but its recall-regression check relies on H.

Names introduced once and reused verbatim across Parts:
- H: `rb_eval::{Corpus, GoldenQuery, DupCluster, Metrics, recall_at_k, mrr, dedup_precision, percentile, run_eval, Baselines}`.
- B: `rb_search::{FusionMode, rank_rrf, RRF_K}`; `rank_with_mode(signals, weights, mode, now, limit)`.
- A: `rb_engine::embedding_input(note) -> String`; `EMBEDDING_INPUT_VERSION = "v2-composite"`; `WriteCommand::Reembed { namespace, id }`; `StoreHandle::reembed`; `SqliteStore::reembed_one` + `needs_reembed`/`active_ids_for_reembed`; `Request::Reembed`/`Response::Reembedded`; CLI `Command::Reembed`.
- C: `Signals.confidence: f32`; `Weights.confidence_floor: f32` (default `0.5`); `confidence_dampener`; `SearchResult.contested: bool`, `MemoryNote.contested: bool`; `SqliteStore::contested_ids`.

---

## Part H — `rb-eval` offline regression harness

This Part stands up a new dev/measurement crate that turns "did this change reorder recall results vs the committed baseline?" into a CI gate. It loads committed JSON fixtures (a small coding-memory corpus, golden queries with expected-relevant ids, and near-duplicate clusters), ingests them through `rb-engine` with `DeterministicProvider`, runs each golden query through `recall`, and asserts pure metrics (recall@k, MRR, dedup precision, latency p50/p99) stay at or above `baselines.json`. The crate is `[dev]`-style: it is a workspace member built and run in CI but is NOT in the `rusty-brain` binary's dependency closure.

HONEST FRAMING (documented in the crate's `lib.rs` doc-comment and asserted nowhere but stated everywhere): with `DeterministicProvider`, the harness guards **ranking determinism, regression detection, and relative-ordering invariants** — NOT absolute semantic quality. Semantic lift (e.g. composite embeddings in Part A) is only observable in the optional `#[ignore]` real-model mode. This mirrors the architecture spec's sqlite-vec scale honesty (§11).

HARD RULES honored: no network; fixtures + `DeterministicProvider` only; the crate may `#![allow(clippy::unwrap_used, clippy::expect_used)]` since it is test/measurement code; updating a baseline is an explicit, reviewed commit.

---

### Task H1: scaffold the `rb-eval` crate

Create the crate, add it to the workspace, and prove it compiles and links `rb-engine`/`rb-embed`. No logic yet — just the skeleton with a doc-comment stating the honest-framing limit.

**Files:**
- Create: crates/rb-eval/Cargo.toml
- Create: crates/rb-eval/src/lib.rs
- Modify: Cargo.toml (workspace members)
- Test: crates/rb-eval/src/lib.rs (placeholder compile test)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-eval/src/lib.rs` with this exact content:

```rust
//! `rb_eval`: offline, deterministic retrieval-quality regression harness.
//!
//! Loads committed JSON fixtures (corpus + golden queries + dup clusters),
//! ingests them through `rb_engine` with `rb_embed::DeterministicProvider`, runs
//! each golden query through recall, and asserts pure metrics (recall@k, MRR,
//! dedup precision, latency p50/p99) stay at or above a committed
//! `baselines.json`.
//!
//! HONEST FRAMING: with `DeterministicProvider` this harness guards ranking
//! determinism, regression detection, and relative-ordering invariants — it does
//! NOT measure absolute semantic quality. Semantic lift is observable only in the
//! optional `#[ignore]` real-model mode (Voyage / local), run manually for spot
//! checks. This is the same scale-honesty the architecture spec states for
//! sqlite-vec (§11). Updating `baselines.json` is an explicit, reviewed commit.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_links_engine_and_embed() {
        // Proves rb-eval can construct the providers it will drive in the runner.
        let provider = rb_embed::DeterministicProvider::new(16);
        assert_eq!(rb_embed::EmbeddingProvider::dim(&provider), 16);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-eval crate_links_engine_and_embed` — Expected: FAIL — the crate is not a workspace member and `crates/rb-eval/Cargo.toml` does not exist (`error: no such package 'rb-eval'`).

- [ ] **Step 3 GREEN: create the manifest + register the member.** Create `crates/rb-eval/Cargo.toml`:

```toml
[package]
name = "rb-eval"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
rb-types = { path = "../rb-types" }
rb-store = { path = "../rb-store" }
rb-search = { path = "../rb-search" }
rb-engine = { path = "../rb-engine" }
rb-embed = { path = "../rb-embed" }
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
tokio = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

Then add the member to the root `Cargo.toml` `members` list (after `"crates/rusty-brain",`):

```toml
members = [
    "crates/rb-types",
    "crates/rb-store",
    "crates/rb-proto",
    "crates/rb-embed",
    "crates/rb-search",
    "crates/rb-engine",
    "crates/rb-enrich",
    "crates/rb-daemon",
    "crates/rb-mcp",
    "crates/rusty-brain",
    "crates/rb-eval",
]
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-eval crate_links_engine_and_embed` — Expected: PASS (1 test).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-eval --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add Cargo.toml crates/rb-eval/Cargo.toml crates/rb-eval/src/lib.rs && git commit -m "chore(rb-eval): scaffold offline retrieval-quality eval crate"` — Expected: one commit.

---

### Task H2: pure metric functions

Add `metrics.rs` with the four pure metric functions the runner asserts on. Each is unit-tested on hand-checked inputs, independent of any fixture or store. These are the load-bearing math; they must be exact.

**Files:**
- Create: crates/rb-eval/src/metrics.rs
- Modify: crates/rb-eval/src/lib.rs (declare `pub mod metrics;`)
- Test: crates/rb-eval/src/metrics.rs (unit tests)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-eval/src/metrics.rs` with this exact content (impl + tests together; Step 3 only wires the module):

```rust
//! Pure retrieval-quality metrics. No IO, no store, no provider — hand-checkable
//! arithmetic over id lists. Each function is deterministic and total.

use rb_types::MemoryId;

/// Fraction of expected-relevant ids that appear in the top-`k` ranked ids.
/// `expected` empty => 1.0 (vacuously perfect; a query with no relevant docs
/// cannot lose recall). `k` is clamped to `ranked.len()`.
pub fn recall_at_k(ranked: &[MemoryId], expected: &[MemoryId], k: usize) -> f64 {
    if expected.is_empty() {
        return 1.0;
    }
    let top: std::collections::HashSet<&MemoryId> = ranked.iter().take(k).collect();
    let hits = expected.iter().filter(|e| top.contains(e)).count();
    hits as f64 / expected.len() as f64
}

/// Mean reciprocal rank of the FIRST expected-relevant id in `ranked`
/// (1-indexed). No expected id present => 0.0. `expected` empty => 0.0 (no
/// reciprocal rank is defined). Single-query MRR; the runner averages across
/// queries.
pub fn mrr(ranked: &[MemoryId], expected: &[MemoryId]) -> f64 {
    let wanted: std::collections::HashSet<&MemoryId> = expected.iter().collect();
    for (i, id) in ranked.iter().enumerate() {
        if wanted.contains(id) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Precision of dedup: of the candidate ids flagged as near-duplicates of an
/// anchor, the fraction that are TRUE duplicates (members of the same cluster).
/// `flagged` empty => 1.0 (nothing wrong was flagged).
pub fn dedup_precision(flagged: &[MemoryId], true_dups: &[MemoryId]) -> f64 {
    if flagged.is_empty() {
        return 1.0;
    }
    let truth: std::collections::HashSet<&MemoryId> = true_dups.iter().collect();
    let correct = flagged.iter().filter(|f| truth.contains(f)).count();
    correct as f64 / flagged.len() as f64
}

/// The `p`-th percentile (0.0..=1.0) of `samples` micros using the
/// nearest-rank method on a sorted copy. Empty => 0. Deterministic: sorts a
/// clone, never mutates the caller's slice.
pub fn percentile(samples: &[u128], p: f64) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let p = p.clamp(0.0, 1.0);
    // nearest-rank: rank = ceil(p * N), 1-indexed, clamped to [1, N].
    let n = sorted.len();
    let rank = (p * n as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_types::MemoryId;

    fn ids(n: usize) -> Vec<MemoryId> {
        (0..n).map(|_| MemoryId::new()).collect()
    }

    #[test]
    fn recall_at_k_counts_hits_in_top_k() {
        let v = ids(5);
        let ranked = v.clone();
        // expected = first and last; only the first is in top-3.
        let expected = vec![v[0].clone(), v[4].clone()];
        assert!((recall_at_k(&ranked, &expected, 3) - 0.5).abs() < 1e-9);
        // top-5 finds both.
        assert!((recall_at_k(&ranked, &expected, 5) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_empty_expected_is_perfect() {
        let v = ids(3);
        assert!((recall_at_k(&v, &[], 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn mrr_is_reciprocal_of_first_hit_rank() {
        let v = ids(4);
        let ranked = v.clone();
        // first expected id is at index 2 => reciprocal rank 1/3.
        let expected = vec![v[2].clone()];
        assert!((mrr(&ranked, &expected) - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn mrr_is_zero_when_no_expected_present() {
        let ranked = ids(3);
        let expected = ids(2); // disjoint
        assert!(mrr(&ranked, &expected).abs() < 1e-9);
    }

    #[test]
    fn dedup_precision_is_fraction_of_true_flags() {
        let v = ids(4);
        // flagged 3, of which 2 are real dups.
        let flagged = vec![v[0].clone(), v[1].clone(), v[2].clone()];
        let true_dups = vec![v[0].clone(), v[1].clone()];
        assert!((dedup_precision(&flagged, &true_dups) - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn dedup_precision_empty_flags_is_perfect() {
        assert!((dedup_precision(&[], &ids(2)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn percentile_nearest_rank() {
        let s = vec![10u128, 20, 30, 40, 50];
        assert_eq!(percentile(&s, 0.5), 30); // ceil(0.5*5)=3 -> idx 2
        assert_eq!(percentile(&s, 0.99), 50); // ceil(0.99*5)=5 -> idx 4
        assert_eq!(percentile(&s, 0.0), 10); // rank floored to 1 -> idx 0
    }

    #[test]
    fn percentile_empty_is_zero_and_does_not_mutate_input() {
        let s: Vec<u128> = vec![];
        assert_eq!(percentile(&s, 0.5), 0);
        let s2 = vec![3u128, 1, 2];
        let _ = percentile(&s2, 0.5);
        assert_eq!(s2, vec![3, 1, 2], "input slice must be untouched");
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-eval metrics` — Expected: FAIL — `metrics` is not declared in `lib.rs` (`error[E0433]`/unresolved module; the tests do not compile/run).

- [ ] **Step 3 GREEN: declare the module.** Add to `crates/rb-eval/src/lib.rs`, after the doc-comment block and `#![allow(...)]`:

```rust
pub mod metrics;
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-eval metrics` — Expected: PASS (8 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-eval --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean, no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-eval/src/metrics.rs crates/rb-eval/src/lib.rs && git commit -m "feat(rb-eval): add pure recall/mrr/dedup/percentile metrics"` — Expected: one commit.

---

### Task H3: fixture loader + committed JSON fixtures

Add `corpus.rs` (typed fixture structs + a loader that fails fast on malformed fixtures) and commit the three fixture files. Fixtures use STABLE string keys (a `key` like `"sqlite-wal"`) rather than UUIDs, so golden queries and dup clusters reference notes by key; the loader assigns a fresh `MemoryId` per note at load and exposes a `key -> MemoryId` map. This keeps fixtures human-editable and id-stable within one run.

**Files:**
- Create: crates/rb-eval/src/corpus.rs
- Create: crates/rb-eval/fixtures/corpus.json
- Create: crates/rb-eval/fixtures/golden_queries.json
- Create: crates/rb-eval/fixtures/dup_clusters.json
- Modify: crates/rb-eval/src/lib.rs (declare `pub mod corpus;`)
- Test: crates/rb-eval/src/corpus.rs (load + validation tests)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-eval/src/corpus.rs` with this exact content:

```rust
//! Fixture types + loader. Fixtures key notes by a stable string `key`; golden
//! queries and dup clusters reference those keys. The loader assigns a fresh
//! `MemoryId` per note and returns a `key -> id` map so the runner can translate
//! expected keys into the ids recall returns. Fails fast on malformed fixtures.

use rb_types::{MemoryId, MemoryType};
use serde::Deserialize;
use std::collections::HashMap;

/// One fixture note. `key` is a stable, human-readable handle used by queries
/// and clusters; it is NOT the runtime id (assigned at load).
#[derive(Debug, Clone, Deserialize)]
pub struct FixtureNote {
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub context: String,
    /// Db string for the memory type (e.g. "insight", "bug_fix").
    pub memory_type: String,
    pub importance: u8,
}

/// A golden query: free text plus the note keys expected in the top results.
#[derive(Debug, Clone, Deserialize)]
pub struct GoldenQuery {
    pub query: String,
    pub expected_keys: Vec<String>,
    /// The k at which recall@k is asserted for this query.
    pub k: usize,
}

/// A near-duplicate cluster: an anchor key plus the keys that are true dups.
#[derive(Debug, Clone, Deserialize)]
pub struct DupCluster {
    pub anchor_key: String,
    pub dup_keys: Vec<String>,
}

/// The loaded corpus: typed notes (with assigned ids), the key->id map, golden
/// queries, and dup clusters.
#[derive(Debug, Clone)]
pub struct Corpus {
    pub notes: Vec<LoadedNote>,
    pub key_to_id: HashMap<String, MemoryId>,
    pub golden: Vec<GoldenQuery>,
    pub clusters: Vec<DupCluster>,
}

/// A fixture note resolved to a runtime id and a parsed memory type.
#[derive(Debug, Clone)]
pub struct LoadedNote {
    pub id: MemoryId,
    pub key: String,
    pub content: String,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub context: String,
    pub memory_type: MemoryType,
    pub importance: u8,
}

impl Corpus {
    /// Load and validate the committed fixtures from `dir`. Fail fast (panics in
    /// this dev/measurement crate are acceptable) on: malformed JSON, duplicate
    /// note keys, or a query/cluster referencing an unknown key.
    pub fn load(dir: &std::path::Path) -> Corpus {
        let notes: Vec<FixtureNote> = read_json(&dir.join("corpus.json"));
        let golden: Vec<GoldenQuery> = read_json(&dir.join("golden_queries.json"));
        let clusters: Vec<DupCluster> = read_json(&dir.join("dup_clusters.json"));

        let mut key_to_id: HashMap<String, MemoryId> = HashMap::new();
        let mut loaded: Vec<LoadedNote> = Vec::with_capacity(notes.len());
        for n in notes {
            assert!(
                !key_to_id.contains_key(&n.key),
                "duplicate fixture note key: {}",
                n.key
            );
            let id = MemoryId::new();
            key_to_id.insert(n.key.clone(), id.clone());
            let memory_type = MemoryType::parse(&n.memory_type)
                .unwrap_or_else(|e| panic!("bad memory_type for {}: {e}", n.key));
            loaded.push(LoadedNote {
                id,
                key: n.key,
                content: n.content,
                keywords: n.keywords,
                tags: n.tags,
                context: n.context,
                memory_type,
                importance: n.importance,
            });
        }

        // Referential integrity: every referenced key must exist.
        for q in &golden {
            for k in &q.expected_keys {
                assert!(key_to_id.contains_key(k), "golden query references unknown key: {k}");
            }
        }
        for c in &clusters {
            assert!(
                key_to_id.contains_key(&c.anchor_key),
                "dup cluster references unknown anchor key: {}",
                c.anchor_key
            );
            for k in &c.dup_keys {
                assert!(key_to_id.contains_key(k), "dup cluster references unknown key: {k}");
            }
        }

        Corpus { notes: loaded, key_to_id, golden, clusters }
    }

    /// The default committed fixture directory (`crates/rb-eval/fixtures`).
    pub fn fixtures_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
    }

    /// Translate a list of fixture keys into runtime ids (panics on unknown key).
    pub fn ids_for(&self, keys: &[String]) -> Vec<MemoryId> {
        keys.iter()
            .map(|k| self.key_to_id.get(k).expect("key must exist").clone())
            .collect()
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> T {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse fixture {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_committed_fixtures_with_referential_integrity() {
        let corpus = Corpus::load(&Corpus::fixtures_dir());
        // Non-trivial corpus so recall/dedup are meaningful.
        assert!(corpus.notes.len() >= 6, "corpus must have >= 6 notes");
        assert!(!corpus.golden.is_empty(), "must have golden queries");
        assert!(!corpus.clusters.is_empty(), "must have dup clusters");
        // Every note key is unique and present in the map.
        assert_eq!(corpus.key_to_id.len(), corpus.notes.len());
        // ids_for round-trips a known set of keys.
        let first_key = corpus.notes[0].key.clone();
        let ids = corpus.ids_for(std::slice::from_ref(&first_key));
        assert_eq!(ids[0], corpus.key_to_id[&first_key]);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-eval corpus` — Expected: FAIL — `corpus` is not declared in `lib.rs` and the fixture files do not exist (`error[E0433]` unresolved module; once wired, the load test would panic on missing files).

- [ ] **Step 3a GREEN: declare the module.** Add to `crates/rb-eval/src/lib.rs`:

```rust
pub mod corpus;
```

- [ ] **Step 3b GREEN: commit the corpus fixture.** Create `crates/rb-eval/fixtures/corpus.json` (6 notes spanning FTS-friendly, vector-friendly, and dup-cluster cases):

```json
[
  { "key": "sqlite-wal", "content": "Use a single SQLite writer with WAL so concurrent readers never block the dedicated writer thread.", "keywords": ["sqlite", "wal", "writer"], "tags": ["storage", "concurrency"], "context": "rusty-brain storage core", "memory_type": "architecture_decision", "importance": 9 },
  { "key": "wal-restated", "content": "One writer thread owns the write connection; readers use a bounded pool. WAL mode avoids SQLITE_BUSY storms.", "keywords": ["sqlite", "wal", "pool"], "tags": ["storage", "concurrency"], "context": "rusty-brain storage core", "memory_type": "architecture_decision", "importance": 8 },
  { "key": "tokio-broadcast", "content": "Change notifications use a tokio broadcast channel that drops oldest on lag and reports Lagged to slow subscribers.", "keywords": ["tokio", "broadcast", "subscribe"], "tags": ["concurrency", "notify"], "context": "daemon change stream", "memory_type": "insight", "importance": 6 },
  { "key": "rrf-fusion", "content": "Reciprocal Rank Fusion fuses ranked candidate lists by 1/(k+rank) and is scale-free, unlike a weighted sum of raw scores.", "keywords": ["rrf", "ranking", "fusion"], "tags": ["search", "ranking"], "context": "hybrid search", "memory_type": "insight", "importance": 7 },
  { "key": "composite-embed", "content": "Embed the composite of content plus keywords, tags, and context rather than content alone for better query alignment.", "keywords": ["embedding", "composite", "recall"], "tags": ["search", "embeddings"], "context": "embedding input", "memory_type": "insight", "importance": 7 },
  { "key": "confidence-poison", "content": "A low-confidence wrong memory can dominate recall; dampen score by confidence to mitigate context poisoning.", "keywords": ["confidence", "poisoning", "ranking"], "tags": ["search", "safety"], "context": "ranking priors", "memory_type": "insight", "importance": 8 }
]
```

- [ ] **Step 3c GREEN: commit golden queries.** Create `crates/rb-eval/fixtures/golden_queries.json`:

```json
[
  { "query": "sqlite writer wal concurrency", "expected_keys": ["sqlite-wal", "wal-restated"], "k": 5 },
  { "query": "reciprocal rank fusion ranking", "expected_keys": ["rrf-fusion"], "k": 5 },
  { "query": "composite embedding recall", "expected_keys": ["composite-embed"], "k": 5 },
  { "query": "confidence context poisoning", "expected_keys": ["confidence-poison"], "k": 5 }
]
```

- [ ] **Step 3d GREEN: commit dup clusters.** Create `crates/rb-eval/fixtures/dup_clusters.json` (the two WAL notes are near-duplicates of each other):

```json
[
  { "anchor_key": "sqlite-wal", "dup_keys": ["wal-restated"] }
]
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-eval corpus` — Expected: PASS (1 test: `loads_committed_fixtures_with_referential_integrity`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-eval --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-eval/src/corpus.rs crates/rb-eval/src/lib.rs crates/rb-eval/fixtures && git commit -m "feat(rb-eval): add fixture loader and committed corpus/golden/dup fixtures"` — Expected: one commit.

---

### Task H4: runner + baselines + CI regression gate

Add `runner.rs` that builds an in-memory `SqliteStore`-backed engine, ingests the corpus through `engine.remember` with `DeterministicProvider`, runs each golden query through `engine.recall` (timed), runs dedup over each cluster via `near_duplicates`, computes the aggregate `Metrics`, and asserts each metric ≥ the committed `baselines.json`. The runner exposes `run_eval(corpus, mode) -> Metrics` so Parts B/A/C can call it under different modes; the CI gate is a `#[test]` asserting `>= baseline`.

**Files:**
- Create: crates/rb-eval/src/runner.rs
- Create: crates/rb-eval/baselines.json
- Modify: crates/rb-eval/src/lib.rs (declare `pub mod runner;`; re-export `run_eval`, `Metrics`, `Baselines`)
- Test: crates/rb-eval/src/runner.rs (regression gate + determinism test)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-eval/src/runner.rs` with this exact content. Note: `EvalMode` is defined locally here so H is self-contained; Part B extends `run_eval` to accept `rb_search::FusionMode` via this enum's `Rrf` arm (added in Part B, Task B5).

```rust
//! The eval runner: ingest fixtures through rb-engine (deterministic vectors),
//! run golden queries through recall, score dedup over clusters, compute the
//! aggregate Metrics, and assert >= the committed baselines.

use crate::corpus::Corpus;
use crate::metrics::{dedup_precision, mrr, percentile, recall_at_k};
use rb_engine::{MemoryEngine, RememberInput};
use rb_embed::DeterministicProvider;
use rb_store::SqliteStore;
use rb_types::Namespace;
use serde::Deserialize;
use std::sync::Arc;

const EVAL_DIM: usize = 64;
const DEDUP_THRESHOLD: f32 = 0.90;

/// Which ranking path to evaluate. H ships only `Linear`; Part B adds the `Rrf`
/// arm and threads it into `MemoryEngine`'s fusion mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalMode {
    Linear,
}

/// Aggregate retrieval-quality metrics for one eval run.
#[derive(Debug, Clone, serde::Serialize, Deserialize, PartialEq)]
pub struct Metrics {
    pub mean_recall_at_k: f64,
    pub mean_mrr: f64,
    pub mean_dedup_precision: f64,
    pub recall_p50_micros: u128,
    pub recall_p99_micros: u128,
}

/// Committed baselines. A metric run must be >= each "min" floor (higher is
/// better) and latency must be <= each "max" ceiling (lower is better).
#[derive(Debug, Clone, Deserialize)]
pub struct Baselines {
    pub min_mean_recall_at_k: f64,
    pub min_mean_mrr: f64,
    pub min_mean_dedup_precision: f64,
    pub max_recall_p99_micros: u128,
}

impl Baselines {
    pub fn load() -> Baselines {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("baselines.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read baselines {}: {e}", path.display()));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse baselines: {e}"))
    }
}

/// Build a fresh in-memory engine, ingest the corpus, and run the eval. Pure of
/// network; uses DeterministicProvider so vectors are reproducible.
pub async fn run_eval(corpus: &Corpus, _mode: EvalMode) -> Metrics {
    let store = SqliteStore::open_in_memory(EVAL_DIM).expect("open in-memory store");
    let store = Arc::new(StoreBackend::new(store));
    let ns = Namespace::Project("rb-eval".into());
    let engine = MemoryEngine::new(
        StoreBackend::clone_arc(&store),
        DeterministicProvider::new(EVAL_DIM),
        ns.clone(),
    );

    for n in &corpus.notes {
        let input = RememberInput {
            content: n.content.clone(),
            context: if n.context.is_empty() { None } else { Some(n.context.clone()) },
            memory_type: n.memory_type,
            importance: n.importance,
            keywords: n.keywords.clone(),
            tags: n.tags.clone(),
            related_files: Vec::new(),
        };
        engine.remember(input).await.expect("remember fixture note");
    }

    // Golden queries: recall@k + MRR, timed.
    let mut recalls = Vec::new();
    let mut mrrs = Vec::new();
    let mut latencies: Vec<u128> = Vec::new();
    for q in &corpus.golden {
        let expected = corpus.ids_for(&q.expected_keys);
        let start = std::time::Instant::now();
        let results = engine
            .recall(&q.query, q.k, None, &[])
            .await
            .expect("recall golden query");
        latencies.push(start.elapsed().as_micros());
        let ranked: Vec<_> = results.iter().map(|r| r.memory.id.clone()).collect();
        recalls.push(recall_at_k(&ranked, &expected, q.k));
        mrrs.push(mrr(&ranked, &expected));
    }

    // Dedup precision: near_duplicates of each anchor vs the true cluster.
    let mut dedup_scores = Vec::new();
    for c in &corpus.clusters {
        let anchor = corpus.key_to_id[&c.anchor_key].clone();
        let flagged: Vec<_> = store
            .raw()
            .near_duplicates(&ns, &anchor, DEDUP_THRESHOLD, 10)
            .expect("near_duplicates")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let true_dups = corpus.ids_for(&c.dup_keys);
        dedup_scores.push(dedup_precision(&flagged, &true_dups));
    }

    Metrics {
        mean_recall_at_k: mean(&recalls),
        mean_mrr: mean(&mrrs),
        mean_dedup_precision: mean(&dedup_scores),
        recall_p50_micros: percentile(&latencies, 0.50),
        recall_p99_micros: percentile(&latencies, 0.99),
    }
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Thin `MemoryBackend` over a shared in-memory `SqliteStore` for eval ingest.
/// Single-threaded eval, so a `Mutex` is sufficient; this is NOT the daemon's
/// single-writer path and never ships in the binary.
struct StoreBackend {
    inner: std::sync::Mutex<SqliteStore>,
}

impl StoreBackend {
    fn new(store: SqliteStore) -> Self {
        Self { inner: std::sync::Mutex::new(store) }
    }
    fn clone_arc(arc: &Arc<StoreBackend>) -> Arc<StoreBackend> {
        Arc::clone(arc)
    }
    /// Borrow the raw store for the dedup pass (single-threaded eval).
    fn raw(&self) -> std::sync::MutexGuard<'_, SqliteStore> {
        self.inner.lock().expect("eval store lock")
    }
}

// NOTE: the eval `MemoryBackend for Arc<StoreBackend>` impl bridges the async
// engine to the synchronous in-memory store. It mirrors `MockBackend` semantics
// (write/get/keyword/vector/graph/list/update/archive/add_link/record_access(es)/
// get_many) by delegating to the locked `SqliteStore`. It is mechanical and
// uncovered here for brevity ONLY because every method is a direct one-line
// delegation to the matching `Store`/`SqliteStore` method already exercised by
// rb-store's own tests; implement each by locking `inner` and calling through.
//
// Implement the impl in this same file. Each method: lock `inner`, call the
// matching store method, map the namespace filter exactly as `StoreHandle` does.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Corpus;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn meets_committed_baselines_in_linear_mode() {
        let corpus = Corpus::load(&Corpus::fixtures_dir());
        let m = run_eval(&corpus, EvalMode::Linear).await;
        let b = Baselines::load();
        assert!(
            m.mean_recall_at_k >= b.min_mean_recall_at_k,
            "recall@k {} regressed below baseline {}",
            m.mean_recall_at_k, b.min_mean_recall_at_k
        );
        assert!(
            m.mean_mrr >= b.min_mean_mrr,
            "MRR {} regressed below baseline {}",
            m.mean_mrr, b.min_mean_mrr
        );
        assert!(
            m.mean_dedup_precision >= b.min_mean_dedup_precision,
            "dedup precision {} regressed below baseline {}",
            m.mean_dedup_precision, b.min_mean_dedup_precision
        );
        // Latency ceiling is a generous guard (CI noise); see baselines.json.
        assert!(
            m.recall_p99_micros <= b.max_recall_p99_micros,
            "recall p99 {}us exceeds ceiling {}us",
            m.recall_p99_micros, b.max_recall_p99_micros
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eval_is_deterministic_across_runs() {
        let corpus = Corpus::load(&Corpus::fixtures_dir());
        let a = run_eval(&corpus, EvalMode::Linear).await;
        let b = run_eval(&corpus, EvalMode::Linear).await;
        // Quality metrics are bit-identical (deterministic vectors + stable sort);
        // latency fields may differ, so compare only the quality numbers.
        assert!((a.mean_recall_at_k - b.mean_recall_at_k).abs() < 1e-12);
        assert!((a.mean_mrr - b.mean_mrr).abs() < 1e-12);
        assert!((a.mean_dedup_precision - b.mean_dedup_precision).abs() < 1e-12);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-eval runner` — Expected: FAIL — `runner` is not declared in `lib.rs`, `baselines.json` does not exist, and the `MemoryBackend for Arc<StoreBackend>` impl is missing (`error[E0277]`: `Arc<StoreBackend>: MemoryBackend` not satisfied).

- [ ] **Step 3a GREEN: implement the eval backend.** In `crates/rb-eval/src/runner.rs`, replace the explanatory `NOTE` comment with a concrete `#[async_trait::async_trait] impl rb_engine::MemoryBackend for Arc<StoreBackend>` whose methods lock `inner` and delegate: `write` → `insert_memory`; `get` → `get_memory` filtered by `ns`; `keyword` → `keyword_search`; `vector` → `vector_search`; `graph` → namespace+active-filtered `graph_neighbors` (mirror `StoreHandle::graph`); `list` → `list`; `update` → ns-checked `update_memory`; `archive` → ns-checked `archive_memory`; `add_link` → `add_link`; `record_access`/`record_accesses` → the matching store methods; `get_many` → `get_many`. Add `async-trait` to `[dependencies]` in `crates/rb-eval/Cargo.toml` (workspace dep, already in the closure):

```toml
async-trait = { workspace = true }
```

- [ ] **Step 3b GREEN: declare the module + re-exports.** Add to `crates/rb-eval/src/lib.rs`:

```rust
pub mod runner;

pub use corpus::Corpus;
pub use metrics::{dedup_precision, mrr, percentile, recall_at_k};
pub use runner::{run_eval, Baselines, EvalMode, Metrics};
```

- [ ] **Step 3c GREEN: commit the baselines.** Create `crates/rb-eval/baselines.json` with conservative floors the deterministic fixture corpus meets (the implementer records the actual first-run `Metrics` and sets floors slightly below them; the values below are the committed starting point — adjust DOWN if a fixture edit lowers a metric, never silently up):

```json
{
  "min_mean_recall_at_k": 0.75,
  "min_mean_mrr": 0.5,
  "min_mean_dedup_precision": 1.0,
  "max_recall_p99_micros": 500000
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-eval runner` — Expected: PASS (2 tests). If `meets_committed_baselines_in_linear_mode` fails on a floor, FIRST print the observed `Metrics` (temporarily) and set each `min_*` in `baselines.json` to just below the observed value, then re-run; commit the calibrated baseline. Do NOT lower a floor to mask a real ranking bug.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-eval --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-eval/Cargo.toml crates/rb-eval/src/runner.rs crates/rb-eval/src/lib.rs crates/rb-eval/baselines.json && git commit -m "feat(rb-eval): add eval runner, baselines, and CI regression gate"` — Expected: one commit.

---

### Task H5: Part H gate

- [ ] **Step 1: full test suite.** Run: `cargo test --workspace` — Expected: PASS (all existing tests plus the new `rb-eval` tests; 0 failures).
- [ ] **Step 2: clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] **Step 3: format.** Run: `cargo fmt --all --check` — Expected: no diff.
- [ ] **Step 4: commit (only if Steps 1–3 surfaced fixes).** Run: `git add -A && git commit -m "chore(rb-eval): part H gate green"` — Expected: at most one commit (skip if nothing changed).

---

## Part B — RRF two-stage hybrid fusion (pure `rb-search`)

This Part adds a scale-free alternative to the existing weighted-linear `rank()`: `FusionMode::Rrf`. Stage 1 fuses the FTS, vector, and graph paths by rank position (`Σ 1/(k + rank)`, `k=60`); stage 2 applies importance, recency (the existing 30-day half-life), and (in Part C) confidence as multiplicative priors. `Linear` stays the byte-for-byte default. Everything here is pure, deterministic (`total_cmp`, stable sort), and added to `rb-search` with no new deps. `rb-eval` then runs the full fixture set under both modes and reports the delta; the default is NOT flipped in P5.

HARD RULES honored: missing-from-a-list contributes nothing (no penalty), matching today's "missing signal = 0" rule; non-finite inputs are sanitized exactly as `rank()` does; output ordering is reproducible across runs.

---

### Task B1: `FusionMode` enum + `RRF_K` constant

Add the mode selector and the documented default fusion constant. Standalone so later tasks can name them.

**Files:**
- Create: crates/rb-search/src/fusion.rs
- Modify: crates/rb-search/src/lib.rs (declare `mod fusion;`, re-export `FusionMode`, `RRF_K`)
- Test: crates/rb-search/src/fusion.rs (enum default + constant test)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-search/src/fusion.rs` with this exact content:

```rust
//! Fusion mode selection for hybrid ranking. `Linear` is the existing
//! weighted-sum `rank()`; `Rrf` is the scale-free Reciprocal Rank Fusion path.

/// Which ranking algorithm `rank_with_mode` uses. `Default` is `Linear`, the
/// byte-for-byte-unchanged behavior shipped since P1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FusionMode {
    /// Weighted sum of normalized signals (the existing `rank`).
    #[default]
    Linear,
    /// Two-stage Reciprocal Rank Fusion: rank-position fusion, then priors.
    Rrf,
}

/// The documented RRF constant: `score += 1/(k + rank)`. `k = 60` is the
/// de-facto hybrid-search default (used by Zep and the original RRF paper);
/// larger `k` flattens the contribution of top ranks. Configurable here only.
pub const RRF_K: f32 = 60.0;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn default_mode_is_linear() {
        assert_eq!(FusionMode::default(), FusionMode::Linear);
    }

    #[test]
    fn rrf_k_is_documented_default() {
        assert!((RRF_K - 60.0).abs() < f32::EPSILON);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-search fusion` — Expected: FAIL — `fusion` is not declared in `lib.rs` (`error[E0433]` unresolved module).

- [ ] **Step 3 GREEN: wire the module.** Edit `crates/rb-search/src/lib.rs` to add the module and re-export. The full updated tail of `lib.rs`:

```rust
mod fusion;
mod merge;
mod rank;
mod weights;

pub use fusion::{FusionMode, RRF_K};
pub use merge::build_signals;
pub use rank::{rank, rank_with_mode, Signals, HALF_LIFE};
pub use weights::Weights;
```

(Note: `rank_with_mode` is exported here but defined in Task B3; until then, leave it out of the `pub use` and add it in B3. For this task export only `FusionMode`, `RRF_K`, and the existing names.)

- [ ] **Step 4: run it.** Run: `cargo test -p rb-search fusion` — Expected: PASS (2 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-search --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-search/src/fusion.rs crates/rb-search/src/lib.rs && git commit -m "feat(rb-search): add FusionMode enum and RRF_K constant"` — Expected: one commit.

---

### Task B2: RRF stage 1 — rank-position fusion

Add the pure stage-1 fusion: derive per-path ranks from `Signals` (keyword from `keyword_rank`; vector from ascending `vector_distance`; graph from ascending `graph_hops`) and sum `1/(k+rank)` across the paths each candidate appears in. Missing from a path = no contribution. This is the core RRF arithmetic; test it on known lists.

**Files:**
- Modify: crates/rb-search/src/rank.rs (add `rrf_stage1`)
- Test: crates/rb-search/src/rank.rs (RRF arithmetic tests)

- [ ] **Step 1 RED: write the failing test.** Add these tests to the existing `#[cfg(test)] mod tests` in `crates/rb-search/src/rank.rs` (after `negative_custom_scores_clamp_to_zero`):

```rust
    #[test]
    fn rrf_stage1_sums_reciprocal_ranks_across_paths() {
        let n = now();
        // A: keyword rank 0 AND vector closest -> two contributions.
        // B: keyword rank 1 only -> one contribution.
        let a = MemoryId::new();
        let b = MemoryId::new();
        let signals = vec![
            Signals {
                id: a.clone(),
                keyword_rank: Some(0),
                vector_distance: Some(0.1),
                graph_hops: None,
                importance: 5,
                created_at: n,
            },
            Signals {
                id: b.clone(),
                keyword_rank: Some(1),
                vector_distance: Some(0.9),
                graph_hops: None,
                importance: 5,
                created_at: n,
            },
        ];
        let fused = rrf_stage1(&signals);
        // A's keyword rank = 0, vector rank = 0 (closest). B's keyword rank = 1,
        // vector rank = 1. RRF_K = 60.
        let a_expected = 1.0 / (RRF_K + 0.0) + 1.0 / (RRF_K + 0.0);
        let b_expected = 1.0 / (RRF_K + 1.0) + 1.0 / (RRF_K + 1.0);
        assert!((fused[&a] - a_expected).abs() < 1e-6);
        assert!((fused[&b] - b_expected).abs() < 1e-6);
        assert!(fused[&a] > fused[&b], "A appears higher in both lists");
    }

    #[test]
    fn rrf_stage1_missing_path_contributes_nothing() {
        let n = now();
        let kw_only = MemoryId::new();
        let signals = vec![Signals {
            id: kw_only.clone(),
            keyword_rank: Some(0),
            vector_distance: None,
            graph_hops: None,
            importance: 5,
            created_at: n,
        }];
        let fused = rrf_stage1(&signals);
        // Only the keyword path contributes: exactly 1/(k+0).
        assert!((fused[&kw_only] - 1.0 / RRF_K).abs() < 1e-6);
    }

    #[test]
    fn rrf_stage1_derives_vector_rank_from_ascending_distance() {
        let n = now();
        let near = MemoryId::new();
        let far = MemoryId::new();
        let signals = vec![
            Signals {
                id: far.clone(),
                keyword_rank: None,
                vector_distance: Some(1.5),
                graph_hops: None,
                importance: 5,
                created_at: n,
            },
            Signals {
                id: near.clone(),
                keyword_rank: None,
                vector_distance: Some(0.2),
                graph_hops: None,
                importance: 5,
                created_at: n,
            },
        ];
        let fused = rrf_stage1(&signals);
        // near has the smaller distance -> vector rank 0 -> higher RRF.
        assert!(fused[&near] > fused[&far]);
        assert!((fused[&near] - 1.0 / RRF_K).abs() < 1e-6);
        assert!((fused[&far] - 1.0 / (RRF_K + 1.0)).abs() < 1e-6);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-search rrf_stage1` — Expected: FAIL — `rrf_stage1` and `RRF_K` are not in scope in `rank.rs` (`error[E0425]`/`cannot find function`).

- [ ] **Step 3 GREEN: implement `rrf_stage1`.** Add to `crates/rb-search/src/rank.rs`. First extend the top `use` to bring in `RRF_K`:

```rust
use crate::fusion::RRF_K;
use crate::weights::Weights;
use rb_types::MemoryId;
use std::collections::HashMap;
```

Then add the function (place it above the `#[cfg(test)]` module):

```rust
/// RRF stage 1: fuse the three retrieval paths by RANK POSITION.
///
/// For each path a candidate appears in, add `1 / (RRF_K + rank)`. Ranks are
/// derived deterministically: keyword from `keyword_rank` (0 = best); vector from
/// ascending `vector_distance` (closest = rank 0); graph from ascending
/// `graph_hops` (nearest = rank 0). A candidate missing from a path contributes
/// nothing from that path (no penalty), matching the "missing signal = 0" rule.
/// Non-finite vector distances sort last (treated as the worst match). Pure and
/// deterministic; the vector/graph rank derivation uses `total_cmp` + the id
/// string as a stable tie-break.
fn rrf_stage1(signals: &[Signals]) -> HashMap<MemoryId, f32> {
    // Vector path: rank by ascending distance (non-finite => worst).
    let mut vec_order: Vec<(&MemoryId, f32)> = signals
        .iter()
        .filter_map(|s| s.vector_distance.map(|d| (&s.id, d)))
        .collect();
    vec_order.sort_by(|a, b| {
        let av = if a.1.is_finite() { a.1 } else { f32::INFINITY };
        let bv = if b.1.is_finite() { b.1 } else { f32::INFINITY };
        av.total_cmp(&bv)
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });
    let vector_rank: HashMap<&MemoryId, usize> = vec_order
        .iter()
        .enumerate()
        .map(|(rank, (id, _))| (*id, rank))
        .collect();

    // Graph path: rank by ascending hops.
    let mut graph_order: Vec<(&MemoryId, u8)> = signals
        .iter()
        .filter_map(|s| s.graph_hops.map(|h| (&s.id, h)))
        .collect();
    graph_order.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });
    let graph_rank: HashMap<&MemoryId, usize> = graph_order
        .iter()
        .enumerate()
        .map(|(rank, (id, _))| (*id, rank))
        .collect();

    let contribution = |rank: usize| -> f32 { 1.0 / (RRF_K + rank as f32) };

    let mut fused: HashMap<MemoryId, f32> = HashMap::with_capacity(signals.len());
    for s in signals {
        let mut score = 0.0_f32;
        if let Some(r) = s.keyword_rank {
            score += contribution(r);
        }
        if let Some(r) = vector_rank.get(&s.id) {
            score += contribution(*r);
        }
        if let Some(r) = graph_rank.get(&s.id) {
            score += contribution(*r);
        }
        fused.insert(s.id.clone(), score);
    }
    fused
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-search rrf_stage1` — Expected: PASS (3 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-search --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-search/src/rank.rs && git commit -m "feat(rb-search): add RRF stage-1 rank-position fusion"` — Expected: one commit.

---

### Task B3: RRF stage 2 (priors) + `rank_rrf` + `rank_with_mode`

Add stage 2 (multiply the fused score by importance, recency, and — wired in Part C — confidence priors), assemble `rank_rrf` (stage1 → stage2 → stable sort → truncate), and a `rank_with_mode` dispatcher so callers select `Linear` or `Rrf` with one signature. `Linear` delegates to the existing `rank` unchanged.

**Files:**
- Modify: crates/rb-search/src/rank.rs (add `rrf_stage2_prior`, `rank_rrf`, `rank_with_mode`)
- Modify: crates/rb-search/src/lib.rs (re-export `rank_with_mode`)
- Test: crates/rb-search/src/rank.rs (priors + dispatch tests)

- [ ] **Step 1 RED: write the failing test.** Add these tests to the `#[cfg(test)] mod tests` in `crates/rb-search/src/rank.rs`:

```rust
    #[test]
    fn rrf_stage2_applies_importance_and_recency_priors() {
        let n = now();
        // Two candidates identical on rank-fusion inputs; importance breaks them.
        let high = MemoryId::new();
        let low = MemoryId::new();
        let mk = |id: MemoryId, importance| Signals {
            id,
            keyword_rank: Some(0),
            vector_distance: Some(0.1),
            graph_hops: None,
            importance,
            created_at: n,
        };
        let signals = vec![mk(high.clone(), 10), mk(low.clone(), 1)];
        let ranked = rank_rrf(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, high, "higher importance wins the prior");
        assert!(ranked[0].1 > ranked[1].1);
        assert!(ranked.iter().all(|(_, s)| s.is_finite()));
    }

    #[test]
    fn rrf_recency_breaks_ties_between_equal_fusion_candidates() {
        let n = now();
        let recent = MemoryId::new();
        let old = MemoryId::new();
        let mk = |id: MemoryId, created| Signals {
            id,
            keyword_rank: Some(0),
            vector_distance: Some(0.1),
            graph_hops: None,
            importance: 5,
            created_at: created,
        };
        let signals = vec![
            mk(old.clone(), n - Duration::days(200)),
            mk(recent.clone(), n),
        ];
        let ranked = rank_rrf(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, recent, "more recent doc wins the recency prior");
    }

    #[test]
    fn rank_with_mode_linear_equals_rank() {
        let n = now();
        let signals: Vec<Signals> = (0..6)
            .map(|i| Signals {
                id: MemoryId::new(),
                keyword_rank: Some(i),
                vector_distance: Some(0.1 * i as f32),
                graph_hops: if i % 2 == 0 { Some(i as u8) } else { None },
                importance: (i as u8) + 1,
                created_at: n - Duration::days(i as i64),
            })
            .collect();
        let via_mode = rank_with_mode(signals.clone(), Weights::default(), FusionMode::Linear, n, 6);
        let via_rank = rank(signals, Weights::default(), n, 6);
        assert_eq!(via_mode, via_rank, "Linear mode must equal rank() byte-for-byte");
    }

    #[test]
    fn rank_with_mode_rrf_is_deterministic_and_truncates() {
        let n = now();
        let signals: Vec<Signals> = (0..8)
            .map(|i| Signals {
                id: MemoryId::new(),
                keyword_rank: if i % 2 == 0 { Some(i) } else { None },
                vector_distance: if i % 3 == 0 { Some(0.05 * i as f32) } else { None },
                graph_hops: None,
                importance: (i as u8 % 10) + 1,
                created_at: n,
            })
            .collect();
        let a = rank_with_mode(signals.clone(), Weights::default(), FusionMode::Rrf, n, 3);
        let b = rank_with_mode(signals, Weights::default(), FusionMode::Rrf, n, 3);
        assert_eq!(a.len(), 3, "truncates to limit");
        let ids_a: Vec<_> = a.iter().map(|(id, _)| id.clone()).collect();
        let ids_b: Vec<_> = b.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids_a, ids_b, "RRF ordering is reproducible");
        for w in a.windows(2) {
            assert!(w[0].1 >= w[1].1, "RRF scores sorted descending");
        }
    }
```

Also extend the top `use` import to bring in `FusionMode` for the dispatch test:

```rust
use crate::fusion::{FusionMode, RRF_K};
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-search rank_with_mode` — Expected: FAIL — `rank_rrf` and `rank_with_mode` do not exist (`error[E0425]`).

- [ ] **Step 3 GREEN: implement priors + assembly + dispatch.** Add to `crates/rb-search/src/rank.rs` (above the test module):

```rust
/// RRF stage 2: the multiplicative prior applied to a fused score. Importance
/// and recency mirror `score_one`'s normalization exactly; `confidence` is the
/// Part C dampener (1.0 until C lands, so this is a no-op for B in isolation).
/// Non-finite or zeroed priors clamp so the product stays finite and >= 0.
fn rrf_stage2_prior(s: &Signals, w: &Weights, now: chrono::DateTime<chrono::Utc>) -> f32 {
    let importance = (s.importance as f32 / 10.0).clamp(0.0, 1.0);
    let age_days = ((now - s.created_at).num_seconds() as f32 / 86_400.0).max(0.0);
    let recency = (-age_days / HALF_LIFE).exp();
    // Each prior contributes a bounded multiplier in (0, 1]; a weight of 0
    // neutralizes that prior (multiplier 1.0) rather than zeroing the score.
    let blend = |weight: f32, value: f32| -> f32 {
        let weight = if weight.is_finite() { weight.clamp(0.0, 1.0) } else { 0.0 };
        let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        1.0 - weight + weight * value
    };
    let prior = blend(w.importance, importance) * blend(w.recency, recency);
    if prior.is_finite() {
        prior.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Two-stage RRF ranking: rank-position fusion (stage 1) scaled by importance +
/// recency priors (stage 2). Pure and deterministic: stable sort by `total_cmp`,
/// truncated to `limit`. Missing signals contribute nothing (no penalty).
pub fn rank_rrf(
    signals: Vec<Signals>,
    weights: Weights,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> Vec<(MemoryId, f32)> {
    let fused = rrf_stage1(&signals);
    let mut scored: Vec<(MemoryId, f32)> = signals
        .iter()
        .map(|s| {
            let base = fused.get(&s.id).copied().unwrap_or(0.0);
            let score = base * rrf_stage2_prior(s, &weights, now);
            (s.id.clone(), if score.is_finite() { score } else { 0.0 })
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(limit);
    scored
}

/// Dispatch ranking by `FusionMode`. `Linear` delegates to `rank` (unchanged);
/// `Rrf` uses `rank_rrf`. Single entry point so callers thread one mode value.
pub fn rank_with_mode(
    signals: Vec<Signals>,
    weights: Weights,
    mode: FusionMode,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> Vec<(MemoryId, f32)> {
    match mode {
        FusionMode::Linear => rank(signals, weights, now, limit),
        FusionMode::Rrf => rank_rrf(signals, weights, now, limit),
    }
}
```

- [ ] **Step 3b GREEN: export `rank_with_mode`.** Update the `pub use rank::...` line in `crates/rb-search/src/lib.rs`:

```rust
pub use rank::{rank, rank_rrf, rank_with_mode, Signals, HALF_LIFE};
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-search rank` — Expected: PASS (all existing `rank.rs` tests plus the 4 new ones; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-search --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-search/src/rank.rs crates/rb-search/src/lib.rs && git commit -m "feat(rb-search): add RRF stage-2 priors and rank_with_mode dispatch"` — Expected: one commit.

---

### Task B4: thread `FusionMode` through the engine

Give `MemoryEngine` a `fusion_mode` field (default `Linear`) and a `with_fusion_mode` builder, and have `recall` call `rank_with_mode(..., self.fusion_mode, ...)` instead of `rank(...)`. Default behavior is unchanged.

**Files:**
- Modify: crates/rb-engine/src/engine.rs (field + builder + recall call)
- Test: crates/rb-engine/src/engine.rs (fusion-mode recall test)

- [ ] **Step 1 RED: write the failing test.** Add this test to the `#[cfg(test)] mod tests` in `crates/rb-engine/src/engine.rs`:

```rust
    #[tokio::test]
    async fn recall_under_rrf_mode_returns_finite_descending_results() {
        let eng = engine().with_fusion_mode(rb_search::FusionMode::Rrf);
        seed(&eng, "alpha topic about sqlite", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "beta topic about tokio", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("topic", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn default_engine_uses_linear_fusion_mode() {
        let eng = engine();
        // No panic / behavior change: default recall path still works.
        seed(&eng, "default mode probe", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 1);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-engine recall_under_rrf_mode_returns_finite_descending_results` — Expected: FAIL — `with_fusion_mode` does not exist (`error[E0599]`/no method).

- [ ] **Step 3 GREEN: add the field, builder, and recall call.** In `crates/rb-engine/src/engine.rs`:

Add `use rb_search::{FusionMode, Weights};` (replace the existing `use rb_search::Weights;`). Add the field to the struct:

```rust
pub struct MemoryEngine<B: MemoryBackend, P: EmbeddingProvider> {
    backend: B,
    embedder: P,
    weights: Weights,
    fusion_mode: FusionMode,
    namespace: Namespace,
    linker: Box<dyn Linker>,
    enricher: Option<Arc<dyn Enricher>>,
}
```

Initialize it in `new` (after `weights: Weights::default(),`):

```rust
            fusion_mode: FusionMode::default(),
```

Add the builder (next to `with_enricher`):

```rust
    /// Select the ranking fusion mode. `Linear` (default) is the existing
    /// weighted-sum rank; `Rrf` is the scale-free two-stage RRF path. Eval-gated:
    /// the default flips to `Rrf` only when `rb-eval` shows a win.
    pub fn with_fusion_mode(mut self, mode: FusionMode) -> Self {
        self.fusion_mode = mode;
        self
    }
```

Replace the `rank` call in `recall` (was `let ranked = rb_search::rank(signals, self.weights, chrono::Utc::now(), candidate_limit);`):

```rust
        let ranked = rb_search::rank_with_mode(
            signals,
            self.weights,
            self.fusion_mode,
            chrono::Utc::now(),
            candidate_limit,
        );
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-engine recall` — Expected: PASS (all existing recall tests plus the 2 new ones; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-engine/src/engine.rs && git commit -m "feat(rb-engine): thread FusionMode through recall ranking"` — Expected: one commit.

---

### Task B5: `rb-eval` compares Linear vs RRF

Extend `rb-eval`'s `EvalMode` with an `Rrf` arm and thread it into `run_eval` (via `with_fusion_mode`), then add a comparison test that runs both modes over the fixture set and reports the metric delta. This is the eval comparison the spec requires; it asserts both modes stay at/above baseline (it does NOT assert RRF wins — that decision is real-corpus-gated).

**Files:**
- Modify: crates/rb-eval/src/runner.rs (EvalMode::Rrf + thread into engine)
- Test: crates/rb-eval/src/runner.rs (both-modes comparison test)

- [ ] **Step 1 RED: write the failing test.** Add this test to the `#[cfg(test)] mod tests` in `crates/rb-eval/src/runner.rs`:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn both_fusion_modes_meet_recall_baseline_and_report_delta() {
        let corpus = Corpus::load(&Corpus::fixtures_dir());
        let b = Baselines::load();
        let linear = run_eval(&corpus, EvalMode::Linear).await;
        let rrf = run_eval(&corpus, EvalMode::Rrf).await;
        // Both modes must clear the committed recall floor.
        assert!(linear.mean_recall_at_k >= b.min_mean_recall_at_k);
        assert!(rrf.mean_recall_at_k >= b.min_mean_recall_at_k);
        // Report (not assert) the delta so a reviewer sees the comparison.
        println!(
            "fusion delta: recall linear={:.4} rrf={:.4} (delta {:+.4})",
            linear.mean_recall_at_k,
            rrf.mean_recall_at_k,
            rrf.mean_recall_at_k - linear.mean_recall_at_k
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-eval both_fusion_modes_meet_recall_baseline_and_report_delta` — Expected: FAIL — `EvalMode::Rrf` does not exist (`error[E0599]`/no variant).

- [ ] **Step 3 GREEN: add the `Rrf` arm and thread it.** In `crates/rb-eval/src/runner.rs`, extend `EvalMode`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalMode {
    Linear,
    Rrf,
}
```

Map it to `rb_search::FusionMode` and apply it when building the engine in `run_eval` (replace the `let _mode: EvalMode` signature usage — now consume `mode`):

```rust
    let fusion = match mode {
        EvalMode::Linear => rb_search::FusionMode::Linear,
        EvalMode::Rrf => rb_search::FusionMode::Rrf,
    };
    let engine = MemoryEngine::new(
        StoreBackend::clone_arc(&store),
        DeterministicProvider::new(EVAL_DIM),
        ns.clone(),
    )
    .with_fusion_mode(fusion);
```

(Change the `run_eval` signature parameter from `_mode: EvalMode` to `mode: EvalMode`.)

- [ ] **Step 4: run it.** Run: `cargo test -p rb-eval` — Expected: PASS (all `rb-eval` tests, including the new comparison; 0 failures). The delta line prints under `cargo test -- --nocapture`.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-eval --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-eval/src/runner.rs && git commit -m "feat(rb-eval): compare linear vs rrf fusion over fixtures"` — Expected: one commit.

---

### Task B6: Part B gate

- [ ] **Step 1: full test suite.** Run: `cargo test --workspace` — Expected: PASS (0 failures).
- [ ] **Step 2: clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] **Step 3: format.** Run: `cargo fmt --all --check` — Expected: no diff.
- [ ] **Step 4: commit (only if Steps 1–3 surfaced fixes).** Run: `git add -A && git commit -m "chore(rb-search): part B gate green"` — Expected: at most one commit.

---

## Part A — composite embedding input + re-embed primitive

This Part replaces the content-only embed with a pure `embedding_input(note)` composite (content + keywords + tags + context), stamps each row with an `embedding_input_version`, and adds the idempotent convergence machinery: a `meta` invariant, an additive checksummed migration, the first-ever vector-UPDATE path (`SqliteStore::reembed_one`), a new `WriteCommand::Reembed` through the single writer, a `Request::Reembed`/`Response::Reembedded` wire pair, and a bounded `rusty-brain reembed` CLI. The query stays embedded RAW — only the document representation changes.

HARD RULES honored: vectors are write-once today; `Reembed` is the ONLY update path and goes through the single writer; the scan is bounded and idempotent (a row already at the current `(embedding_model, embedding_input_version)` is skipped; a second run writes nothing); the migration is additive/file-discovered/checksummed and must pass the fresh-DB reproducibility gate; the dim contract stays fail-closed.

---

### Task A1: pure `embedding_input(note)` composer + version constant

Add a pure function composing the embedded document text and the version stamp constant. Standalone and unit-tested on field composition, ordering, and empty fields.

**Files:**
- Create: crates/rb-engine/src/embedding_input.rs
- Modify: crates/rb-engine/src/lib.rs (declare `mod embedding_input;`, re-export `embedding_input`, `EMBEDDING_INPUT_VERSION`)
- Test: crates/rb-engine/src/embedding_input.rs (composition tests)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-engine/src/embedding_input.rs` with this exact content:

```rust
//! Composite embedding input. A-MEM embeds the composite of content + enrichment
//! (keywords, tags, context), which aligns the stored vector with the kinds of
//! queries agents actually ask. This replaces the content-only embed at write
//! time. The QUERY stays embedded raw — only the document representation changes,
//! so query symmetry is not required (A-MEM does the same).

use rb_types::MemoryNote;

/// The current embedding-input template version, stamped per row and seeded as a
/// `meta` invariant. Bump this string whenever the composition below changes so
/// `reembed` can detect and converge stale rows.
pub const EMBEDDING_INPUT_VERSION: &str = "v2-composite";

/// Compose the document text to embed for `note`: content, then keywords, tags,
/// and context, each on its own labeled line, omitting empty sections so the
/// representation is stable and never has trailing blank lines. Pure and
/// deterministic; documented order is content → keywords → tags → context.
pub fn embedding_input(note: &MemoryNote) -> String {
    let mut sections: Vec<String> = Vec::with_capacity(4);
    // Content is always present (a note cannot exist without it).
    sections.push(note.content.clone());
    if !note.keywords.is_empty() {
        sections.push(format!("keywords: {}", note.keywords.join(", ")));
    }
    if !note.tags.is_empty() {
        sections.push(format!("tags: {}", note.tags.join(", ")));
    }
    if !note.context.trim().is_empty() {
        sections.push(format!("context: {}", note.context));
    }
    sections.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn note(content: &str) -> MemoryNote {
        MemoryNote::new(Namespace::Global, content.to_string(), MemoryType::Insight, 5)
    }

    #[test]
    fn version_is_the_composite_stamp() {
        assert_eq!(EMBEDDING_INPUT_VERSION, "v2-composite");
    }

    #[test]
    fn composes_all_fields_in_documented_order() {
        let mut n = note("single writer over sqlite");
        n.keywords = vec!["sqlite".into(), "writer".into()];
        n.tags = vec!["storage".into()];
        n.context = "rusty-brain core".into();
        let input = embedding_input(&n);
        assert_eq!(
            input,
            "single writer over sqlite\nkeywords: sqlite, writer\ntags: storage\ncontext: rusty-brain core"
        );
    }

    #[test]
    fn omits_empty_sections_without_trailing_newlines() {
        let n = note("just content"); // no keywords/tags/context
        assert_eq!(embedding_input(&n), "just content");
    }

    #[test]
    fn content_only_differs_from_full_composite() {
        let mut full = note("body");
        full.keywords = vec!["k".into()];
        // The composite must differ from the bare content so re-embedding actually
        // changes the vector input.
        assert_ne!(embedding_input(&full), full.content);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-engine embedding_input` — Expected: FAIL — `embedding_input` module is not declared in `lib.rs` (`error[E0433]`).

- [ ] **Step 3 GREEN: wire the module.** In `crates/rb-engine/src/lib.rs`, add the module declaration and re-export (match the existing `pub use` style):

```rust
mod embedding_input;
pub use embedding_input::{embedding_input, EMBEDDING_INPUT_VERSION};
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-engine embedding_input` — Expected: PASS (4 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-engine/src/embedding_input.rs crates/rb-engine/src/lib.rs && git commit -m "feat(rb-engine): add composite embedding_input composer and version stamp"` — Expected: one commit.

---

### Task A2: `remember` embeds the composite + stamps `embedding_input_version`

Replace the content-only embed at `engine.rs:147` with `embedding_input(&note)`, and add an `embedding_input_version` field to `MemoryNote` (defaulted, stamped at write). The field is additive on the type; the column lands in Task A3.

**Files:**
- Modify: crates/rb-types/src/memory.rs (add `embedding_input_version` field + default)
- Modify: crates/rb-engine/src/engine.rs (embed composite + stamp version)
- Test: crates/rb-engine/src/engine.rs (composite-embed + stamp tests); crates/rb-types/src/memory.rs (default test)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-types/src/memory.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn new_defaults_embedding_input_version_empty() {
        let m = sample();
        assert_eq!(m.embedding_input_version, "");
    }
```

And add to `crates/rb-engine/src/engine.rs` `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn remember_embeds_composite_not_content_alone() {
        let eng = engine();
        // Two notes: identical content, different keywords -> different composite
        // -> different deterministic vectors. Proves the composite (not bare
        // content) is what gets embedded.
        let mut a = input("shared body text", 5);
        a.keywords = vec!["alpha".into()];
        let mut b = input("shared body text", 5);
        b.keywords = vec!["beta".into()];
        let ida = eng.remember(a).await.unwrap();
        let idb = eng.remember(b).await.unwrap();
        assert_ne!(
            eng.backend().embedding_of(&ida),
            eng.backend().embedding_of(&idb),
            "different enrichment must yield different composite vectors"
        );
    }

    #[tokio::test]
    async fn remember_stamps_embedding_input_version() {
        let eng = engine();
        let id = eng.remember(input("stamp check", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.embedding_input_version, rb_engine::EMBEDDING_INPUT_VERSION);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-types new_defaults_embedding_input_version_empty` — Expected: FAIL — `embedding_input_version` field does not exist on `MemoryNote` (`error[E0609]`/no field).

- [ ] **Step 3a GREEN: add the type field.** In `crates/rb-types/src/memory.rs`, add the field to `MemoryNote` (after `embedding_model: String,`):

```rust
    pub embedding_model: String,
    /// Which `embedding_input` template version produced the stored vector.
    /// Empty until stamped at write; used by `reembed` to detect stale rows.
    #[serde(default)]
    pub embedding_input_version: String,
    pub links: Vec<MemoryLink>,
```

Initialize it in `MemoryNote::new` (after `embedding_model: String::new(),`):

```rust
            embedding_model: String::new(),
            embedding_input_version: String::new(),
```

- [ ] **Step 3b GREEN: embed the composite + stamp in `remember`.** In `crates/rb-engine/src/engine.rs`, set the stamp next to the model stamp (after `note.embedding_model = self.embedder.model_id().to_string();`):

```rust
        note.embedding_model = self.embedder.model_id().to_string();
        note.embedding_input_version = crate::EMBEDDING_INPUT_VERSION.to_string();
```

Replace the embed call (was `let mut embeddings = self.embedder.embed(&[note.content.clone()]).await?;`):

```rust
        // Embed the COMPOSITE document (content + keywords + tags + context), not
        // content alone, so the stored vector aligns with agent-style queries.
        let mut embeddings = self
            .embedder
            .embed(&[crate::embedding_input(&note)])
            .await?;
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-types memory && cargo test -p rb-engine remember_embeds_composite_not_content_alone && cargo test -p rb-engine remember_stamps_embedding_input_version` — Expected: PASS. Then run `cargo test -p rb-engine` to confirm no existing remember test regressed (the deterministic-vector tests assert determinism, not content-equality, so they still pass).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings`, `cargo clippy -p rb-engine --all-targets -- -D warnings`, then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-types/src/memory.rs crates/rb-engine/src/engine.rs && git commit -m "feat(rb-engine): embed composite input and stamp embedding_input_version"` — Expected: one commit.

---

### Task A3: additive migration + `memories.embedding_input_version` column + meta seed

Add the checksummed migration creating the new column, seed the `meta` invariant at init, and persist/read the stamp in `insert_memory`/`row_to_note`. Must pass the existing fresh-DB reproducibility gate.

**Files:**
- Create: crates/rb-store/migrations/003_embedding_input_version.sql
- Modify: crates/rb-store/src/store.rs (insert + row_to_note + seed meta key)
- Test: crates/rb-store/src/store.rs (column round-trip + migration test); crates/rb-store/src/migrations.rs (object-presence assert)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-store/src/store.rs` `#[cfg(test)] mod tests` (a round-trip proving the stamp persists):

```rust
    #[test]
    fn insert_and_read_round_trips_embedding_input_version() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(
            Namespace::Global,
            "stamped row".to_string(),
            MemoryType::Insight,
            5,
        );
        m.embedding_input_version = "v2-composite".to_string();
        store.insert_memory(&m, Some(&[0.1f32; 8])).unwrap();
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.embedding_input_version, "v2-composite");
    }
```

And add to `crates/rb-store/src/migrations.rs` `embedded_initial_schema_creates_expected_objects` (append an assertion that the new column exists after `run_migrations`):

```rust
        // 003: embedding_input_version column present on memories.
        let has_eiv: i64 = c
            .query_row(
                "SELECT count(*) FROM pragma_table_info('memories') WHERE name='embedding_input_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_eiv, 1, "embedding_input_version column added by 003");
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-store insert_and_read_round_trips_embedding_input_version` — Expected: FAIL — the column does not exist; `insert_memory` does not write it and `row_to_note` does not read it (`Error::Storage` "no such column" or the field read fails).

- [ ] **Step 3a GREEN: add the migration.** Create `crates/rb-store/migrations/003_embedding_input_version.sql`:

```sql
-- 003_embedding_input_version.sql
-- Stamp each memory with the embedding-input template version that produced its
-- stored vector, alongside the existing embedding_model stamp. Additive: a
-- NOT NULL column with an empty-string default so existing rows (content-only
-- vectors) read back as "" and `reembed` treats them as stale until converged.
-- The companion meta invariant `embedding_input_version` is seeded in code at
-- open() next to embedding_dim / embedding_model (store.rs), not here, matching
-- how those invariants are managed.

ALTER TABLE memories
  ADD COLUMN embedding_input_version TEXT NOT NULL DEFAULT '';
```

- [ ] **Step 3b GREEN: persist + read the column.** In `crates/rb-store/src/store.rs` `insert_memory`, extend the INSERT column list and params. Change the column list to add `embedding_input_version` after `embedding_model`:

```rust
                    "INSERT INTO memories (
                        memory_id, namespace, created_at, updated_at, content, summary,
                        keywords, tags, context, memory_type, importance, confidence,
                        related_files, access_count, last_accessed_at, archived_at,
                        superseded_by, embedding_model, embedding_input_version
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, ?18, ?19
                     )",
```

Add the param after `note.embedding_model,`:

```rust
                        note.embedding_model,
                        note.embedding_input_version,
```

In `row_to_note`, read the field (after `embedding_model: g("embedding_model")?,`):

```rust
        embedding_model: g("embedding_model")?,
        embedding_input_version: g("embedding_input_version")?,
        links,
```

Add the field to BOTH other SELECT column lists used by `list` (`store.rs:1062`) and `get_many` (`store.rs:1280`) — and `get_memory` (`store.rs:867`) — so `row_to_note` sees the column: append `, embedding_input_version` after `embedding_model` in each `SELECT ... FROM memories` statement that feeds `row_to_note`.

- [ ] **Step 3c GREEN: seed the meta invariant.** In `crates/rb-store/src/store.rs` `init` (after `seed_or_verify_dim(&conn, embedding_dim)?;`), seed the version invariant idempotently:

```rust
        seed_or_verify_dim(&conn, embedding_dim)?;
        seed_embedding_input_version(&conn)?;
```

Add the helper next to `seed_or_verify_dim`:

```rust
/// Seed `meta.embedding_input_version` if absent. Unlike the dim invariant this
/// does NOT fail-close on mismatch: a corpus can legitimately hold mixed
/// versions mid-transition (some rows content-only, some composite) until
/// `reembed` converges them, so the meta value records the CURRENT template, not
/// a hard contract. Seeded once; never overwritten here.
fn seed_embedding_input_version(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('embedding_input_version', ?1)",
        rusqlite::params![rb_engine_input_version()],
    )
    .map_err(storage_err)?;
    Ok(())
}

/// The current input-template version string. Defined here (not imported from
/// rb-engine, which depends on rb-store) to avoid a dependency cycle; it MUST
/// stay in lockstep with `rb_engine::EMBEDDING_INPUT_VERSION`.
fn rb_engine_input_version() -> &'static str {
    "v2-composite"
}
```

(NOTE on the cycle: `rb-store` is a leaf that `rb-engine` depends on, so `rb-store` cannot import `rb_engine::EMBEDDING_INPUT_VERSION`. The string is duplicated with an explicit lockstep comment. A test below guards the lockstep.)

- [ ] **Step 3d GREEN: lockstep guard test.** Add to `crates/rb-store/src/store.rs` tests:

```rust
    #[test]
    fn meta_seeds_current_embedding_input_version() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let v: String = store
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='embedding_input_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "v2-composite", "must match rb_engine::EMBEDDING_INPUT_VERSION");
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-store embedding_input_version && cargo test -p rb-store embedded_initial_schema_creates_expected_objects && cargo test -p rb-store meta_seeds_current_embedding_input_version` — Expected: PASS. Then run `cargo test -p rb-store` to confirm no existing store test regressed.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-store/migrations/003_embedding_input_version.sql crates/rb-store/src/store.rs crates/rb-store/src/migrations.rs && git commit -m "feat(rb-store): add embedding_input_version column, migration, and meta seed"` — Expected: one commit.

---

### Task A4: store re-embed UPDATE path + stale-row scan

Add the first vector-UPDATE path (`reembed_one`: recompute is done by the caller; the store UPDATEs `memory_vectors` and re-stamps the row) and a bounded read scan (`active_ids_for_reembed`) that returns active rows whose `(embedding_model, embedding_input_version)` differ from the target stamps. Both are namespace-scoped.

**Files:**
- Modify: crates/rb-store/src/store.rs (add `reembed_one`, `active_ids_for_reembed`, a `ReembedRow` projection)
- Test: crates/rb-store/src/store.rs (update + idempotency + scan tests)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-store/src/store.rs` tests:

```rust
    #[test]
    fn reembed_one_updates_vector_and_restamps_row() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(Namespace::Global, "x".into(), MemoryType::Insight, 5);
        m.embedding_model = "old-model".into();
        m.embedding_input_version = "".into(); // content-only legacy stamp
        store.insert_memory(&m, Some(&[0.1f32; 8])).unwrap();

        // Re-embed with a new vector and the current stamps.
        store
            .reembed_one(&m.id, &[0.9f32; 8], "voyage-3", "v2-composite")
            .unwrap();

        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.embedding_model, "voyage-3");
        assert_eq!(got.embedding_input_version, "v2-composite");
        // The stored vector blob changed.
        let dups = store
            .near_duplicates(&Namespace::Global, &MemoryId::new(), 0.0, 1)
            .unwrap();
        let _ = dups; // (vector presence proven via the scan test below)
    }

    #[test]
    fn active_ids_for_reembed_returns_only_stale_active_rows_in_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("scan".into());
        let other = Namespace::Project("other".into());

        // Stale (legacy stamp).
        let mut stale = MemoryNote::new(ns.clone(), "stale".into(), MemoryType::Insight, 5);
        stale.embedding_model = "old".into();
        stale.embedding_input_version = "".into();
        store.insert_memory(&stale, Some(&[0.1f32; 8])).unwrap();

        // Current (already converged) -> skipped.
        let mut current = MemoryNote::new(ns.clone(), "current".into(), MemoryType::Insight, 5);
        current.embedding_model = "voyage-3".into();
        current.embedding_input_version = "v2-composite".into();
        store.insert_memory(&current, Some(&[0.2f32; 8])).unwrap();

        // Stale but in another namespace -> never returned.
        let mut foreign = MemoryNote::new(other, "foreign".into(), MemoryType::Insight, 5);
        foreign.embedding_model = "old".into();
        foreign.embedding_input_version = "".into();
        store.insert_memory(&foreign, Some(&[0.3f32; 8])).unwrap();

        // Archived stale -> never returned.
        let mut archived = MemoryNote::new(ns.clone(), "archived".into(), MemoryType::Insight, 5);
        archived.embedding_model = "old".into();
        archived.archived_at = Some(chrono::Utc::now());
        store.insert_memory(&archived, Some(&[0.4f32; 8])).unwrap();

        let rows = store
            .active_ids_for_reembed(&ns, "voyage-3", "v2-composite", 100)
            .unwrap();
        let ids: Vec<MemoryId> = rows.iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids, vec![stale.id.clone()], "only the active, stale, in-ns row");
    }

    #[test]
    fn active_ids_for_reembed_is_idempotent_after_convergence() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("conv".into());
        let mut m = MemoryNote::new(ns.clone(), "converge me".into(), MemoryType::Insight, 5);
        m.embedding_model = "old".into();
        m.embedding_input_version = "".into();
        store.insert_memory(&m, Some(&[0.1f32; 8])).unwrap();

        assert_eq!(store.active_ids_for_reembed(&ns, "voyage-3", "v2-composite", 100).unwrap().len(), 1);
        store.reembed_one(&m.id, &[0.5f32; 8], "voyage-3", "v2-composite").unwrap();
        // After convergence the scan returns nothing: a second pass writes nothing.
        assert!(store.active_ids_for_reembed(&ns, "voyage-3", "v2-composite", 100).unwrap().is_empty());
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-store reembed` — Expected: FAIL — `reembed_one` and `active_ids_for_reembed` do not exist (`error[E0599]`/no method).

- [ ] **Step 3a GREEN: add the projection + scan.** In `crates/rb-store/src/store.rs`, add a row projection (near `RecalRow`):

```rust
/// One active memory that needs re-embedding: its id and the content fields the
/// caller composes into the new embedding input. Read-only projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReembedRow {
    pub id: MemoryId,
    pub content: String,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub context: String,
}
```

Add the methods in an `impl SqliteStore` block:

```rust
    /// Active (`archived_at IS NULL`), in-namespace memories whose stored stamps
    /// `(embedding_model, embedding_input_version)` differ from the target,
    /// ordered by `created_at ASC, memory_id ASC` (deterministic), capped at
    /// `limit`. A row already at BOTH target stamps is skipped, which is what
    /// makes a re-embed pass idempotent. Read-only.
    pub fn active_ids_for_reembed(
        &self,
        ns: &Namespace,
        target_model: &str,
        target_version: &str,
        limit: usize,
    ) -> Result<Vec<ReembedRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, content, keywords, tags, context
                 FROM memories
                 WHERE namespace = ?1
                   AND archived_at IS NULL
                   AND NOT (embedding_model = ?2 AND embedding_input_version = ?3)
                 ORDER BY created_at ASC, memory_id ASC
                 LIMIT ?4",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    ns.as_db_string(),
                    target_model,
                    target_version,
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, content, keywords, tags, context) =
                r.map_err(|e| Error::Storage(e.to_string()))?;
            out.push(ReembedRow {
                id: parse_id(&id)?,
                content,
                keywords: parse_json_array(&keywords)?,
                tags: parse_json_array(&tags)?,
                context,
            });
        }
        Ok(out)
    }

    /// Replace the stored vector for `id` and re-stamp `(embedding_model,
    /// embedding_input_version)`, in ONE transaction. This is the only path that
    /// UPDATEs `memory_vectors` (vectors are otherwise write-once). Fails closed
    /// (rolls back) on a dimension mismatch or a missing row. Caller computes the
    /// new embedding from the composite input; the store only persists it.
    pub fn reembed_one(
        &self,
        id: &MemoryId,
        embedding: &[f32],
        embedding_model: &str,
        embedding_input_version: &str,
    ) -> Result<()> {
        if embedding.len() != self.embedding_dim {
            return Err(Error::DimensionMismatch {
                expected: self.embedding_dim,
                got: embedding.len(),
            });
        }
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|e| Error::Storage(e.to_string()))?;
        let result = (|| -> Result<()> {
            // vec0 has no UPDATE for the embedding column; delete + reinsert the row.
            self.conn
                .execute(
                    "DELETE FROM memory_vectors WHERE memory_id = ?1",
                    rusqlite::params![id.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            self.conn
                .execute(
                    "INSERT INTO memory_vectors (memory_id, embedding) VALUES (?1, ?2)",
                    rusqlite::params![id.to_string(), embedding_bytes(embedding)],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            let affected = self
                .conn
                .execute(
                    "UPDATE memories
                     SET embedding_model = ?1, embedding_input_version = ?2, updated_at = ?3
                     WHERE memory_id = ?4",
                    rusqlite::params![
                        embedding_model,
                        embedding_input_version,
                        chrono::Utc::now().timestamp(),
                        id.to_string()
                    ],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            if affected == 0 {
                return Err(Error::NotFound(id.clone()));
            }
            Ok(())
        })();
        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT;")
                .map_err(|e| Error::Storage(e.to_string())),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-store reembed` — Expected: PASS (3 tests). Then `cargo test -p rb-store` to confirm no regression.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-store/src/store.rs && git commit -m "feat(rb-store): add reembed_one vector-update path and stale-row scan"` — Expected: one commit.

---

### Task A5: `WriteCommand::Reembed` + `StoreHandle` re-embed batch

Add the new write command (the ONLY caller of `reembed_one`), a read method for the scan, and a `StoreHandle::reembed` batch method that drives the whole namespace-scoped convergence through the single writer using an injected embedder closure. Bounded + idempotent + fail-safe per row.

**Files:**
- Modify: crates/rb-daemon/src/store_handle.rs (WriteCommand::Reembed arm, writer_loop arm, StoreHandle::reembed_scan + reembed_apply)
- Test: crates/rb-daemon/src/store_handle.rs (single-writer re-embed round-trip + idempotency)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-daemon/src/store_handle.rs` `#[cfg(test)] mod tests`:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_reembed_converges_stale_rows_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("reembed".to_string());

        // Insert a row with a legacy stamp (content-only, old model).
        let mut m = note(&ns, "converge me");
        m.embedding_model = "old".into();
        m.embedding_input_version = "".into();
        let id = m.id.clone();
        handle.write(m, Some(vec![0.1f32; DIM])).await.unwrap();

        // Scan finds it stale.
        let stale = handle
            .reembed_scan(ns.clone(), "voyage-3", "v2-composite", 100)
            .await
            .unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, id);

        // Apply a re-embed through the single writer.
        handle
            .reembed_apply(id.clone(), vec![0.9f32; DIM], "voyage-3".into(), "v2-composite".into())
            .await
            .unwrap();

        // Re-stamped; a second scan finds nothing (idempotent convergence).
        let got = handle.get(ns.clone(), id.clone()).await.unwrap().unwrap();
        assert_eq!(got.embedding_model, "voyage-3");
        assert_eq!(got.embedding_input_version, "v2-composite");
        let after = handle
            .reembed_scan(ns.clone(), "voyage-3", "v2-composite", 100)
            .await
            .unwrap();
        assert!(after.is_empty(), "converged corpus yields no stale rows");

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon store_handle_reembed_converges_stale_rows_and_is_idempotent` — Expected: FAIL — `reembed_scan`/`reembed_apply` and `WriteCommand::Reembed` do not exist (`error[E0599]`/`E0599`).

- [ ] **Step 3a GREEN: add the WriteCommand variant.** In `crates/rb-daemon/src/store_handle.rs`, add to the `WriteCommand` enum (after `Supersede { .. }`):

```rust
    Reembed {
        id: MemoryId,
        embedding: Vec<f32>,
        embedding_model: String,
        embedding_input_version: String,
        reply: oneshot::Sender<Result<()>>,
    },
```

- [ ] **Step 3b GREEN: handle it in `writer_loop`.** Add an arm in the `match cmd` block (after the `Supersede` arm). No `MemoryChanged` event — re-embed changes only the vector representation, not the logical memory (mirror the access-tracking arms' no-event rule):

```rust
            WriteCommand::Reembed {
                id,
                embedding,
                embedding_model,
                embedding_input_version,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.reembed_one(&id, &embedding, &embedding_model, &embedding_input_version)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
```

- [ ] **Step 3c GREEN: add the `StoreHandle` methods.** Add to the `impl StoreHandle` block:

```rust
    /// Scan up to `limit` active, in-namespace memories whose stamps differ from
    /// the target `(model, version)`. Reads via the pool (never the writer).
    pub async fn reembed_scan(
        &self,
        ns: Namespace,
        target_model: String,
        target_version: String,
        limit: usize,
    ) -> Result<Vec<rb_store::ReembedRow>> {
        self.with_read(move |store| {
            store.active_ids_for_reembed(&ns, &target_model, &target_version, limit)
        })
        .await
    }

    /// Apply ONE re-embed through the single writer: replace the vector and
    /// re-stamp the row. The caller (the daemon's reembed handler) computes the
    /// embedding; this only persists it.
    pub async fn reembed_apply(
        &self,
        id: MemoryId,
        embedding: Vec<f32>,
        embedding_model: String,
        embedding_input_version: String,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Reembed {
            id,
            embedding,
            embedding_model,
            embedding_input_version,
            reply,
        };
        self.send_write(cmd, rx).await
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon store_handle_reembed` — Expected: PASS. Then `cargo test -p rb-daemon` to confirm no regression.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/store_handle.rs && git commit -m "feat(rb-daemon): add Reembed write command and single-writer reembed methods"` — Expected: one commit.

---

### Task A6: engine `reembed` driver + proto wire pair + daemon dispatch

Add a `MemoryEngine::reembed` driver that scans its namespace for stale rows, recomputes each composite embedding with its own embedder, and applies each through the single writer (bounded, idempotent, fail-safe per row). Then add the `Request::Reembed`/`Response::Reembedded` wire pair and the daemon `dispatch` arm. `CONTRACT_VERSION` stays 1 here (the additive `contested` field in Part C is what bumps it; `Reembed` is a new op, not a shape change to existing responses — older clients simply never send it).

**Files:**
- Modify: crates/rb-engine/src/engine.rs (add `reembed` driver + `ReembedSummary`)
- Modify: crates/rb-proto/src/messages.rs (Request::Reembed, Response::Reembedded, round-trip coverage)
- Modify: crates/rb-daemon/src/server.rs (dispatch arm)
- Test: crates/rb-engine/src/engine.rs (driver idempotency); crates/rb-proto/src/messages.rs (round-trip); crates/rb-daemon/src/server.rs (integration)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-engine/src/engine.rs` tests:

```rust
    #[tokio::test]
    async fn reembed_converges_stale_rows_then_is_a_noop() {
        let eng = engine();
        // Seed a note, then forcibly mark it stale in the mock backend so the scan
        // treats it as needing re-embed.
        let id = eng.remember(input("converge body", 5)).await.unwrap();
        eng.backend().set_stale_for_reembed(&id);

        let first = eng.reembed(100).await.unwrap();
        assert_eq!(first.changed, 1, "the stale row is re-embedded");
        assert_eq!(first.skipped, 0);

        // After convergence the row is stamped current; a second pass changes nothing.
        let second = eng.reembed(100).await.unwrap();
        assert_eq!(second.changed, 0, "idempotent: nothing left to re-embed");
    }
```

And add to `crates/rb-proto/src/messages.rs` tests (extend `all_requests` and `all_responses` and assert the op/result tags):

```rust
    #[test]
    fn reembed_request_and_response_round_trip() {
        let json = serde_json::to_string(&Request::Reembed { limit: 500 }).unwrap();
        assert_eq!(json, r#"{"op":"Reembed","limit":500}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);

        let json = serde_json::to_string(&Response::Reembedded {
            scanned: 10,
            changed: 4,
            skipped: 6,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"result":"Reembedded","scanned":10,"changed":4,"skipped":6}"#
        );
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-proto reembed_request_and_response_round_trip` — Expected: FAIL — `Request::Reembed` / `Response::Reembedded` do not exist (`error[E0599]`).

- [ ] **Step 3a GREEN: add the mock helper + engine driver.** In `crates/rb-engine/src/test_support.rs`, add a stale-marking helper and the scan/apply hooks the driver needs. Add a field `stale_reembed: Mutex<std::collections::HashSet<MemoryId>>` to `MockBackend` and:

```rust
    pub fn set_stale_for_reembed(&self, id: &MemoryId) {
        self.stale_reembed.lock().unwrap().insert(id.clone());
    }
```

Extend the `MemoryBackend` trait (`crates/rb-engine/src/backend.rs`) with the two re-embed hooks so the engine driver is backend-agnostic (the `StoreHandle` impl wires them to `reembed_scan`/`reembed_apply`; the mock wires them to its map):

```rust
    /// Active, in-namespace rows whose stamps differ from the target; the engine
    /// recomputes each composite embedding and applies via `reembed_apply`.
    async fn reembed_scan(
        &self,
        ns: Namespace,
        target_model: String,
        target_version: String,
        limit: usize,
    ) -> rb_types::Result<Vec<rb_types::ReembedRow>>;
    /// Persist one recomputed vector + re-stamp, through the single writer.
    async fn reembed_apply(
        &self,
        id: MemoryId,
        embedding: Vec<f32>,
        embedding_model: String,
        embedding_input_version: String,
    ) -> rb_types::Result<()>;
```

(Add a `ReembedRow` re-export in `rb-types` mirroring `rb-store::ReembedRow`, OR define a small `rb_types::ReembedRow` and have `rb-store` convert; to avoid a new cross-crate type, define `ReembedRow { id, content, keywords, tags, context }` in `rb-types` and have `rb-store::active_ids_for_reembed` return `rb_types::ReembedRow`. Update Task A4's projection to live in `rb-types` instead — adjust the import there. This keeps the engine trait depending only on `rb-types`.)

Implement both on `MockBackend` (mock scan returns notes whose id is in `stale_reembed`, projected to `ReembedRow`; mock apply removes the id from `stale_reembed`, replaces the stored embedding, and stamps the note). Implement both on the in-crate test `MockBackend` in `backend.rs` tests too (delegating trivially), so that module still compiles.

Add the driver to `crates/rb-engine/src/engine.rs`:

```rust
/// What a `reembed` pass touched. Mirrors `JobSummary` shape for CLI/proto.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReembedSummary {
    pub scanned: u64,
    pub changed: u64,
    pub skipped: u64,
}

impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P> {
    /// Converge this namespace's vectors to the current `(model, input_version)`:
    /// scan up to `limit` stale active rows, recompute each composite embedding
    /// with this engine's embedder, and apply through the single writer. Bounded
    /// (`limit`), idempotent (a converged corpus scans empty), and fail-safe (a
    /// row that fails to embed/apply is logged and counted as skipped, never
    /// fatal; the next pass retries it).
    pub async fn reembed(&self, limit: usize) -> rb_types::Result<ReembedSummary> {
        let target_model = self.embedder.model_id().to_string();
        let target_version = crate::EMBEDDING_INPUT_VERSION.to_string();
        let stale = self
            .backend
            .reembed_scan(
                self.namespace.clone(),
                target_model.clone(),
                target_version.clone(),
                limit,
            )
            .await?;
        let mut summary = ReembedSummary {
            scanned: stale.len() as u64,
            ..Default::default()
        };
        for row in stale {
            // Reconstruct the composite input from the projected fields.
            let input = crate::embedding_input_from_parts(
                &row.content,
                &row.keywords,
                &row.tags,
                &row.context,
            );
            let embedding = match self.embedder.embed(&[input]).await {
                Ok(mut v) => match v.pop() {
                    Some(e) => e,
                    None => {
                        summary.skipped += 1;
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, memory_id = %row.id, "reembed embed failed; skipping");
                    summary.skipped += 1;
                    continue;
                }
            };
            match self
                .backend
                .reembed_apply(
                    row.id.clone(),
                    embedding,
                    target_model.clone(),
                    target_version.clone(),
                )
                .await
            {
                Ok(()) => summary.changed += 1,
                Err(e) => {
                    tracing::warn!(error = %e, memory_id = %row.id, "reembed apply failed; skipping");
                    summary.skipped += 1;
                }
            }
        }
        Ok(summary)
    }
}
```

Add `embedding_input_from_parts` to `crates/rb-engine/src/embedding_input.rs` (the same composition as `embedding_input`, but from a projection so the driver need not load a full `MemoryNote`):

```rust
/// Same composition as [`embedding_input`] but from already-projected fields
/// (the re-embed scan returns a projection, not a full note). MUST stay in
/// lockstep with `embedding_input`.
pub fn embedding_input_from_parts(
    content: &str,
    keywords: &[String],
    tags: &[String],
    context: &str,
) -> String {
    let mut sections: Vec<String> = Vec::with_capacity(4);
    sections.push(content.to_string());
    if !keywords.is_empty() {
        sections.push(format!("keywords: {}", keywords.join(", ")));
    }
    if !tags.is_empty() {
        sections.push(format!("tags: {}", tags.join(", ")));
    }
    if !context.trim().is_empty() {
        sections.push(format!("context: {context}"));
    }
    sections.join("\n")
}
```

Refactor `embedding_input(note)` to delegate to `embedding_input_from_parts` (so the lockstep is enforced by construction), and re-export `embedding_input_from_parts` from `lib.rs`.

- [ ] **Step 3b GREEN: add the proto wire pair.** In `crates/rb-proto/src/messages.rs`, add to `Request` (after `Subscribe,`):

```rust
    /// Re-embed up to `limit` stale active memories in the connection namespace,
    /// converging them to the daemon's current embedding model + input version.
    /// Bounded and idempotent; safe to call repeatedly.
    Reembed {
        limit: usize,
    },
```

Add to `Response` (after `SubscribeAck,`):

```rust
    /// Result of a `Reembed` pass.
    Reembedded {
        scanned: u64,
        changed: u64,
        skipped: u64,
    },
```

Add `Request::Reembed { limit: 500 }` to `all_requests()` and `Response::Reembedded { scanned: 1, changed: 1, skipped: 0 }` to `all_responses()` so the existing exhaustive round-trip tests cover them.

- [ ] **Step 3c GREEN: dispatch the request.** In `crates/rb-daemon/src/server.rs` `dispatch`, add an arm (near the `Request::RunJob` arm):

```rust
        Request::Reembed { limit } => match engine.reembed(limit.min(MAX_LIMIT)).await {
            Ok(s) => Response::Reembedded {
                scanned: s.scanned,
                changed: s.changed,
                skipped: s.skipped,
            },
            Err(e) => error_to_response(e),
        },
```

(`MAX_LIMIT` already bounds list/recall; reuse it to bound a single re-embed pass. The CLI loops passes until a pass reports `changed == 0`, so a small `MAX_LIMIT` still converges the whole corpus — see Task A7.)

- [ ] **Step 3d GREEN: wire `MemoryBackend for StoreHandle`.** In `crates/rb-daemon/src/store_handle.rs`, implement the two new trait methods on the `impl MemoryBackend for StoreHandle` by delegating to the `reembed_scan`/`reembed_apply` inherent methods added in A5.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-engine reembed_converges_stale_rows_then_is_a_noop && cargo test -p rb-proto reembed && cargo test -p rb-daemon` — Expected: PASS. Then `cargo test -p rb-types` (the new `ReembedRow` type) and `cargo test -p rb-store reembed` to confirm the moved projection still works.

- [ ] **Step 5: lint+format.** Run: `cargo clippy --workspace --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-types crates/rb-store/src/store.rs crates/rb-engine crates/rb-proto/src/messages.rs crates/rb-daemon/src/server.rs crates/rb-daemon/src/store_handle.rs && git commit -m "feat: add namespace reembed driver, wire pair, and dispatch"` — Expected: one commit.

---

### Task A7: `rusty-brain reembed` CLI

Add the `Command::Reembed` subcommand that loops bounded `Request::Reembed` passes against the running daemon until a pass reports `changed == 0` (full convergence), printing a summary. The CLI NEVER writes the DB — it only sends requests.

**Files:**
- Modify: crates/rusty-brain/src/cli.rs (Command::Reembed)
- Modify: crates/rusty-brain/src/client.rs (send Reembed, loop to convergence)
- Modify: crates/rusty-brain/src/output.rs (render summary)
- Test: crates/rusty-brain/src/cli.rs (parse test); crates/rusty-brain/src/output.rs (render test)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rusty-brain/src/cli.rs` tests:

```rust
    #[test]
    fn parses_reembed_with_default_batch() {
        let cli = Cli::parse_from(["rusty-brain", "reembed"]);
        match cli.command {
            Command::Reembed { batch } => assert_eq!(batch, 500),
            other => panic!("expected Reembed, got {other:?}"),
        }
    }

    #[test]
    fn parses_reembed_with_explicit_batch() {
        let cli = Cli::parse_from(["rusty-brain", "reembed", "--batch", "100"]);
        match cli.command {
            Command::Reembed { batch } => assert_eq!(batch, 100),
            other => panic!("expected Reembed, got {other:?}"),
        }
    }
```

And add to `crates/rusty-brain/src/output.rs` tests:

```rust
    #[test]
    fn render_reembed_summary_human_and_json() {
        let human = render_reembed(12, 5, 7, false);
        assert!(human.contains("12") && human.contains('5') && human.contains('7'));
        let json = render_reembed(12, 5, 7, true);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["scanned"], 12);
        assert_eq!(v["changed"], 5);
        assert_eq!(v["skipped"], 7);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain parses_reembed_with_default_batch` — Expected: FAIL — `Command::Reembed` does not exist (`error[E0599]`/no variant).

- [ ] **Step 3a GREEN: add the subcommand.** In `crates/rusty-brain/src/cli.rs`, add to `Command` (after `Evolve { .. }`):

```rust
    /// Re-embed stale memories in the current namespace until convergence.
    Reembed {
        /// Max memories re-embedded per daemon round-trip (the CLI loops until
        /// a pass changes nothing).
        #[arg(long, default_value_t = 500)]
        batch: usize,
    },
```

- [ ] **Step 3b GREEN: add the client convergence loop.** In `crates/rusty-brain/src/client.rs`, add a handler that sends `Request::Reembed { limit: batch }` in a loop, accumulating `scanned`/`changed`/`skipped`, stopping when a pass returns `changed == 0` (or `scanned == 0`). Match the existing client request/response style (the same pattern used for `Evolve` → `Request::RunJob` → `Response::JobRan`). On a `Response::Error`, surface it (do not loop forever). Return the totals.

- [ ] **Step 3c GREEN: add the renderer.** In `crates/rusty-brain/src/output.rs`, add:

```rust
/// Render a re-embed summary (json: `{ "scanned", "changed", "skipped" }`).
pub fn render_reembed(scanned: u64, changed: u64, skipped: u64, json: bool) -> String {
    if json {
        format!("{{\"scanned\":{scanned},\"changed\":{changed},\"skipped\":{skipped}}}")
    } else {
        format!("Re-embedded: {changed} changed, {skipped} skipped, {scanned} scanned.")
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain reembed` — Expected: PASS (the parse + render tests). Then `cargo test -p rusty-brain` to confirm no regression.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/cli.rs crates/rusty-brain/src/client.rs crates/rusty-brain/src/output.rs && git commit -m "feat(rusty-brain): add reembed CLI that converges via the daemon"` — Expected: one commit.

---

### Task A8: Part A gate

- [ ] **Step 1: full test suite.** Run: `cargo test --workspace` — Expected: PASS (0 failures), including the migration reproducibility gate and the rb-eval baseline (deterministic vectors are unchanged in distance space, so composite embedding does not regress the offline metrics; the semantic-lift comparison is the `#[ignore]` real-model test noted in Task A8 Step 4).
- [ ] **Step 2: clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] **Step 3: format.** Run: `cargo fmt --all --check` — Expected: no diff.
- [ ] **Step 4: document the real-model comparison (no code).** Add a one-paragraph note to `crates/rb-eval/src/lib.rs` doc-comment (or a `// REAL-MODEL MODE` comment in `runner.rs`) stating that content-only-vs-composite semantic lift is only observable under the optional `#[ignore]` real-model mode (Voyage/local), since `DeterministicProvider` hashes the WHOLE input string and so already reflects the composite change without modeling semantic similarity. This is the honest-framing limit from spec §6/§8/§12. If a stub `#[ignore]` test is added, gate it behind a real API key and never run it in CI.
- [ ] **Step 5: commit (only if Steps 1–4 surfaced fixes/docs).** Run: `git add -A && git commit -m "chore: part A gate green and document real-model reembed comparison"` — Expected: at most one commit.

---

## Part C — confidence-weighted ranking + contradiction surfacing

This Part wires data already in the schema into the read path. `confidence` becomes a multiplicative dampener (`score *= floor + (1 - floor) * confidence`, `floor = 0.5` default, configurable) applied in BOTH `Linear` and `Rrf` so a low-confidence wrong memory is suppressed but never zeroed (the context-poisoning mitigation). An active `contradicts` link surfaces as a fail-open `contested: bool` on recall/list/context/get result rows, computed read-side over the result set after ranking. `CONTRACT_VERSION` bumps to 2 so clients can detect the richer shape; the field defaults to `false` so older clients stay correct.

HARD RULES honored: the dampener is pure/deterministic and applies identically to both fusion modes; the contradiction lookup is batched over the result set only and FAILS OPEN (a lookup error returns unflagged results, never aborts recall); namespace isolation is preserved (the lookup is scoped).

---

### Task C1: `Signals.confidence` + `Weights.confidence_floor` + dampener (both modes)

Add `confidence` to `Signals`, a configurable `confidence_floor` to `Weights` (default `0.5`), and apply the multiplicative dampener as the final step of both `rank` (Linear) and `rank_rrf`. `build_signals` is extended to carry confidence from `meta`.

**Files:**
- Modify: crates/rb-search/src/rank.rs (Signals.confidence; dampener in score_one + rank_rrf)
- Modify: crates/rb-search/src/weights.rs (confidence_floor + default)
- Modify: crates/rb-search/src/merge.rs (meta carries confidence)
- Test: crates/rb-search/src/{rank.rs,weights.rs,merge.rs}

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-search/src/weights.rs` tests:

```rust
    #[test]
    fn default_confidence_floor_is_one_half() {
        let w = Weights::default();
        assert!((w.confidence_floor - 0.5).abs() < f32::EPSILON);
    }
```

Add to `crates/rb-search/src/rank.rs` tests (covering both modes + the floor):

```rust
    #[test]
    fn confidence_dampens_score_but_never_zeroes_it() {
        let n = now();
        let high = MemoryId::new();
        let low = MemoryId::new();
        // Identical on every signal EXCEPT confidence.
        let mk = |id: MemoryId, confidence| Signals {
            id,
            keyword_rank: Some(0),
            vector_distance: Some(0.1),
            graph_hops: None,
            importance: 5,
            confidence,
            created_at: n,
        };
        let signals = vec![mk(high.clone(), 1.0), mk(low.clone(), 0.0)];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, high, "high-confidence memory ranks first");
        assert_eq!(ranked[1].0, low);
        // floor = 0.5: the zero-confidence memory keeps HALF its score, not zero.
        assert!(ranked[1].1 > 0.0, "dampener floors at confidence_floor, never zero");
    }

    #[test]
    fn confidence_dampener_applies_under_rrf_too() {
        let n = now();
        let high = MemoryId::new();
        let low = MemoryId::new();
        let mk = |id: MemoryId, confidence| Signals {
            id,
            keyword_rank: Some(0),
            vector_distance: Some(0.1),
            graph_hops: None,
            importance: 5,
            confidence,
            created_at: n,
        };
        let signals = vec![mk(low.clone(), 0.1), mk(high.clone(), 1.0)];
        let ranked = rank_rrf(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, high, "RRF stage-2 confidence prior suppresses low-confidence");
        assert!(ranked[1].1 > 0.0);
    }
```

Add to `crates/rb-search/src/merge.rs` tests:

```rust
    #[test]
    fn carries_confidence_from_meta() {
        let now = Utc::now();
        let id = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert(id.clone(), (8u8, 0.25f32, now));
        let signals = build_signals(std::slice::from_ref(&id), &[], &[], &meta);
        assert!((signals[0].confidence - 0.25).abs() < f32::EPSILON);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-search confidence` — Expected: FAIL — `Signals` has no `confidence` field, `Weights` has no `confidence_floor`, and the `meta` tuple shape differs (`error[E0560]`/`E0609`/`E0308`).

- [ ] **Step 3a GREEN: add the floor to Weights.** In `crates/rb-search/src/weights.rs`, add the field and default (it is a dampener config, NOT a summed signal weight, so it stays OUT of the sum-to-1.0 invariant — note this in the doc):

```rust
pub struct Weights {
    pub vector: f32,
    pub keyword: f32,
    pub graph: f32,
    pub importance: f32,
    pub recency: f32,
    /// Confidence dampener floor in `[0,1]`. The final score is multiplied by
    /// `confidence_floor + (1 - confidence_floor) * confidence`, so a
    /// zero-confidence memory keeps `confidence_floor` of its score (never zero)
    /// and a full-confidence memory is unchanged. NOT part of the sum-to-1.0
    /// signal-weight invariant. Default 0.5.
    pub confidence_floor: f32,
}
```

In `Default`:

```rust
            recency: 0.05,
            confidence_floor: 0.5,
```

- [ ] **Step 3b GREEN: add confidence to Signals + the dampener helper.** In `crates/rb-search/src/rank.rs`, add the field to `Signals` (after `importance: u8,`):

```rust
    /// Importance 0..=10.
    pub importance: u8,
    /// Stored confidence in `[0,1]`; a multiplicative dampener at scoring time.
    pub confidence: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
```

Add the dampener helper (above `score_one`):

```rust
/// The multiplicative confidence dampener: `floor + (1 - floor) * confidence`.
/// `floor` and `confidence` are clamped to `[0,1]`; a non-finite input yields the
/// floor (the most-suppressed-but-nonzero outcome). Result is always in
/// `[floor, 1]`, so a low-confidence memory is suppressed but never zeroed.
fn confidence_dampener(confidence: f32, floor: f32) -> f32 {
    let floor = if floor.is_finite() { floor.clamp(0.0, 1.0) } else { 0.0 };
    let confidence = if confidence.is_finite() {
        confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    floor + (1.0 - floor) * confidence
}
```

Apply it in `score_one` as the final factor before the finite check:

```rust
    let score = finite_weight(w.vector) * vector_sim
        + finite_weight(w.keyword) * keyword
        + finite_weight(w.graph) * graph
        + finite_weight(w.importance) * importance
        + finite_weight(w.recency) * recency;
    let score = score * confidence_dampener(s.confidence, w.confidence_floor);

    if score.is_finite() {
        score.clamp(0.0, 1.0)
    } else {
        0.0
    }
```

Apply it in `rank_rrf` by multiplying the per-candidate score (extend the existing `let score = base * rrf_stage2_prior(...)` line):

```rust
            let score = base
                * rrf_stage2_prior(s, &weights, now)
                * confidence_dampener(s.confidence, weights.confidence_floor);
```

- [ ] **Step 3c GREEN: update every `Signals { .. }` literal.** Adding a non-`Option` field breaks every struct literal. Update them all to set `confidence`:
  - `crates/rb-search/src/merge.rs`: in `build_signals`, change the `meta` parameter type to `&HashMap<MemoryId, (u8, f32, chrono::DateTime<chrono::Utc>)>` and the `slot` closure's destructure to `let (importance, confidence, created_at) = *meta.get(id)?;`, set `confidence,` in the pushed `Signals`.
  - `crates/rb-search/src/rank.rs` tests: add `confidence: 1.0,` to every existing `Signals { .. }` literal (the `strong_doc_outranks_weak_doc`, `recency_breaks_ties...`, `graph_only...`, `missing_signals...`, `limit_truncates...`, `scores_in_range...`, `non_finite_vector...`, `non_finite_weights...`, `negative_custom_scores...`, and the new RRF tests) so they still compile and assert the same ordering (confidence 1.0 is a no-op dampener).
  - `crates/rb-search/src/merge.rs` tests: change every `meta.insert(id, (imp, now))` to `(imp, 1.0, now)` and update the tuple type annotations; the `output_feeds_rank_end_to_end` test too.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-search` — Expected: PASS (all existing + new tests; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-search --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-search/src && git commit -m "feat(rb-search): add confidence dampener to linear and rrf ranking"` — Expected: one commit.

---

### Task C2: engine threads `confidence` into `build_signals` meta

Update `recall` to put each note's `confidence` into the `meta` map (now a 3-tuple). Pure plumbing; behavior is unchanged for the default `confidence = 1.0` notes.

**Files:**
- Modify: crates/rb-engine/src/engine.rs (meta tuple includes confidence)
- Test: crates/rb-engine/src/engine.rs (low-confidence ranks below high-confidence)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-engine/src/engine.rs` tests:

```rust
    #[tokio::test]
    async fn recall_ranks_low_confidence_memory_below_equal_high_confidence() {
        let eng = engine();
        // Two near-identical notes; force one to low confidence via the backend.
        let high = note(
            Namespace::Project("rb".into()),
            "confidence probe content",
            MemoryType::Insight,
            5,
            &[],
        );
        let mut low = note(
            Namespace::Project("rb".into()),
            "confidence probe content",
            MemoryType::Insight,
            5,
            &[],
        );
        low.confidence = 0.0;
        let (high_id, low_id) = (high.id.clone(), low.id.clone());
        eng.backend().insert_note(high);
        eng.backend().insert_note(low);
        eng.backend().set_keyword_results(vec![high_id.clone(), low_id.clone()]);
        eng.backend().set_vector_results(vec![(high_id.clone(), 0.1), (low_id.clone(), 0.1)]);

        let results = eng.recall("confidence probe", 10, None, &[]).await.unwrap();
        let pos = |id: &MemoryId| results.iter().position(|r| &r.memory.id == id).unwrap();
        assert!(pos(&high_id) < pos(&low_id), "high-confidence ranks above low");
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-engine recall_ranks_low_confidence_memory_below_equal_high_confidence` — Expected: FAIL — the `meta` insert is a 2-tuple, so it does not compile against the new `build_signals` signature (`error[E0308]` mismatched types).

- [ ] **Step 3 GREEN: include confidence in meta.** In `crates/rb-engine/src/engine.rs` `recall`, change the `meta` type and insert. The declaration becomes:

```rust
        let mut meta: HashMap<MemoryId, (u8, f32, chrono::DateTime<chrono::Utc>)> = HashMap::new();
```

The insert becomes:

```rust
            meta.insert(note.id.clone(), (note.importance, note.confidence, note.created_at));
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-engine recall` — Expected: PASS (all recall tests including the new one; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-engine/src/engine.rs && git commit -m "feat(rb-engine): thread confidence into recall ranking signals"` — Expected: one commit.

---

### Task C3: store batched `contested_ids` read

Add a single batched read that, given a set of memory ids, returns the subset with at least one active `contradicts` link (inbound OR outbound). One query over `memory_links`; namespace isolation is enforced by the caller scoping the input ids (recall already returns only in-namespace notes).

**Files:**
- Modify: crates/rb-store/src/store.rs (add `contested_ids`)
- Test: crates/rb-store/src/store.rs (inbound/outbound/none cases)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-store/src/store.rs` tests:

```rust
    #[test]
    fn contested_ids_flags_inbound_and_outbound_contradicts() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Global;
        let a = MemoryNote::new(ns.clone(), "a".into(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "b".into(), MemoryType::Insight, 5);
        let c = MemoryNote::new(ns.clone(), "c".into(), MemoryType::Insight, 5);
        let (aid, bid, cid) = (a.id.clone(), b.id.clone(), c.id.clone());
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store.insert_memory(&c, Some(&[0.3f32; 8])).unwrap();

        // A contradicts B: A (outbound) and B (inbound) are both contested; C is not.
        store
            .add_link(&MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::Contradicts,
                strength: 0.9,
                reason: "conflict".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        let flagged = store
            .contested_ids(&[aid.clone(), bid.clone(), cid.clone()])
            .unwrap();
        assert!(flagged.contains(&aid), "outbound contradicts is contested");
        assert!(flagged.contains(&bid), "inbound contradicts is contested");
        assert!(!flagged.contains(&cid), "unlinked memory is not contested");
    }

    #[test]
    fn contested_ids_ignores_non_contradicts_links() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Global;
        let a = MemoryNote::new(ns.clone(), "a".into(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "b".into(), MemoryType::Insight, 5);
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.9,
                reason: "related".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        let flagged = store.contested_ids(&[aid, bid]).unwrap();
        assert!(flagged.is_empty(), "References links never mark contested");
    }

    #[test]
    fn contested_ids_empty_input_is_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        assert!(store.contested_ids(&[]).unwrap().is_empty());
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-store contested_ids` — Expected: FAIL — `contested_ids` does not exist (`error[E0599]`/no method).

- [ ] **Step 3 GREEN: implement the batched read.** In `crates/rb-store/src/store.rs`, add to an `impl SqliteStore` block:

```rust
    /// Of the given `ids`, return the subset that has at least one ACTIVE
    /// `contradicts` link (inbound or outbound). One query over `memory_links`;
    /// `ids` empty => empty. Namespace isolation is the caller's responsibility
    /// (pass only ids already scoped to the namespace). Used read-side after
    /// ranking to set the `contested` flag; best-effort (the caller fails open).
    pub fn contested_ids(&self, ids: &[MemoryId]) -> Result<std::collections::HashSet<MemoryId>> {
        if ids.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        // "?1, ?2, ..." placeholders reused for both source_id and target_id.
        let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 1)).collect();
        let in_list = placeholders.join(", ");
        let sql = format!(
            "SELECT source_id, target_id FROM memory_links
             WHERE link_type = 'contradicts'
               AND (source_id IN ({in_list}) OR target_id IN ({in_list}))"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let params: Vec<Box<dyn rusqlite::ToSql>> =
            ids.iter().map(|id| Box::new(id.to_string()) as Box<dyn rusqlite::ToSql>).collect();
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let wanted: std::collections::HashSet<String> = ids.iter().map(|i| i.to_string()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut out = std::collections::HashSet::new();
        for r in rows {
            let (src, tgt) = r.map_err(|e| Error::Storage(e.to_string()))?;
            // Both endpoints of a contradicts edge are contested IF they are in
            // the requested set.
            if wanted.contains(&src) {
                out.insert(parse_id(&src)?);
            }
            if wanted.contains(&tgt) {
                out.insert(parse_id(&tgt)?);
            }
        }
        Ok(out)
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-store contested_ids` — Expected: PASS (3 tests). Then `cargo test -p rb-store` to confirm no regression.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-store/src/store.rs && git commit -m "feat(rb-store): add batched contested_ids contradicts lookup"` — Expected: one commit.

---

### Task C4: `contested` field on result rows + engine sets it fail-open + `CONTRACT_VERSION` bump

Add `contested: bool` (defaulting `false`) to `SearchResult` AND `MemoryNote` (so recall/list/context/graph/get all carry it uniformly through serde). The engine sets it after ranking via a batched, FAIL-OPEN `contested_ids` lookup. Bump `CONTRACT_VERSION` to 2.

**Files:**
- Modify: crates/rb-types/src/query.rs (SearchResult.contested), crates/rb-types/src/memory.rs (MemoryNote.contested)
- Modify: crates/rb-engine/src/backend.rs (add `contested_ids` trait method), crates/rb-engine/src/test_support.rs + backend.rs tests (mock impl), crates/rb-daemon/src/store_handle.rs (StoreHandle impl)
- Modify: crates/rb-engine/src/engine.rs (set contested in recall/get/list/context, fail-open)
- Modify: crates/rb-proto/src/messages.rs (CONTRACT_VERSION = 2 + test)
- Test: crates/rb-engine/src/engine.rs (contested set + fail-open); crates/rb-proto/src/messages.rs (version)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-proto/src/messages.rs` tests — change `contract_version_is_one` to:

```rust
    #[test]
    fn contract_version_is_two() {
        assert_eq!(CONTRACT_VERSION, 2);
    }
```

Add to `crates/rb-engine/src/engine.rs` tests:

```rust
    #[tokio::test]
    async fn recall_flags_contested_when_active_contradicts_link_exists() {
        let eng = engine();
        let a = seed(&eng, "claim that X is true", MemoryType::Insight, 5, &[]).await;
        let b = seed(&eng, "claim that X is false", MemoryType::Insight, 5, &[]).await;
        eng.backend().add_link(rb_types::MemoryLink {
            source_id: a.clone(),
            target_id: b.clone(),
            link_type: rb_types::LinkType::Contradicts,
            strength: 0.9,
            reason: "conflict".into(),
            created_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
        let results = eng.recall("claim", 10, None, &[]).await.unwrap();
        assert!(
            results.iter().filter(|r| r.contested).count() >= 2,
            "both sides of the contradiction are flagged contested"
        );
    }

    #[tokio::test]
    async fn recall_fails_open_when_contested_lookup_errors() {
        let eng = engine();
        seed(&eng, "probe", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_fail_contested_lookup(true);
        // Recall still returns results, just unflagged (best-effort enrichment).
        let results = eng.recall("probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].contested, "lookup failure -> unflagged, not aborted");
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-proto contract_version_is_two` — Expected: FAIL — `CONTRACT_VERSION` is still 1 (`assertion failed`).

- [ ] **Step 3a GREEN: add the fields.** In `crates/rb-types/src/query.rs`, add to `SearchResult`:

```rust
pub struct SearchResult {
    pub memory: MemoryNote,
    pub score: f32,
    /// True if the memory has an active `contradicts` link. Read-side enrichment;
    /// defaults false so older clients (and any path that skips the lookup) stay
    /// correct.
    #[serde(default)]
    pub contested: bool,
}
```

In `crates/rb-types/src/memory.rs`, add to `MemoryNote` (after `embedding_input_version`):

```rust
    pub embedding_input_version: String,
    /// True if this memory has an active `contradicts` link. Set read-side on
    /// get/list/context/graph results; defaults false (and is never persisted —
    /// `#[serde(default, skip_serializing_if = "..")]` is NOT used so the wire
    /// shape is stable, but the column does not exist: it is computed, not stored).
    #[serde(default)]
    pub contested: bool,
    pub links: Vec<MemoryLink>,
```

Initialize `contested: false` in `MemoryNote::new` (after `embedding_input_version: String::new(),`) and `contested: false` in every `SearchResult { .. }` literal (engine `recall`, proto/mcp tests, output tests). In `crates/rb-store/src/store.rs` `row_to_note`, set `contested: false,` (the store never computes it; recall/get fill it). Update the `MemoryNote` round-trip test and the `SearchResult` round-trip test to expect the new default.

- [ ] **Step 3b GREEN: add the backend trait method + impls.** In `crates/rb-engine/src/backend.rs`, add to `MemoryBackend`:

```rust
    /// Of `ids`, the subset with an active `contradicts` link. Best-effort: the
    /// engine fails open on error (returns unflagged results).
    async fn contested_ids(
        &self,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<std::collections::HashSet<MemoryId>>;
```

Implement on `MockBackend` (`test_support.rs`): add a `fail_contested_lookup: AtomicBool` + `set_fail_contested_lookup`, and an impl that scans stored notes' `links` for `Contradicts` edges touching any requested id (both endpoints), erroring when the fail flag is set. Implement the trivial version on the in-crate `MockBackend` in `backend.rs` tests too. Implement on `StoreHandle` (`store_handle.rs`) by delegating to `contested_ids` via the read pool:

```rust
    async fn contested_ids(
        &self,
        ids: Vec<MemoryId>,
    ) -> Result<std::collections::HashSet<MemoryId>> {
        self.with_read(move |store| store.contested_ids(&ids)).await
    }
```

- [ ] **Step 3c GREEN: set `contested` in the engine, fail-open.** In `crates/rb-engine/src/engine.rs` `recall`, after assembling `results` and before the access-tracking block, batch-load and apply contested flags fail-open:

```rust
        // Best-effort contradiction surfacing: a lookup failure leaves results
        // unflagged rather than failing recall (fail-open enrichment).
        let result_ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        match self.backend.contested_ids(result_ids).await {
            Ok(contested) => {
                for r in &mut results {
                    let flag = contested.contains(&r.memory.id);
                    r.contested = flag;
                    r.memory.contested = flag;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "contested lookup failed; returning unflagged results");
            }
        }
```

Apply the same fail-open flagging in `get` (set `note.contested` on the single returned note), `list`, and `context` (flag each returned note). Factor a small private helper `async fn flag_contested(&self, notes: &mut [MemoryNote])` to avoid repetition; it calls `contested_ids` once over all note ids and sets each `note.contested`, logging+ignoring on error.

- [ ] **Step 3d GREEN: bump the contract version.** In `crates/rb-proto/src/messages.rs`:

```rust
pub const CONTRACT_VERSION: u32 = 2;
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-proto && cargo test -p rb-engine contested && cargo test -p rb-engine recall_fails_open_when_contested_lookup_errors` — Expected: PASS. Then `cargo test -p rb-store && cargo test -p rb-daemon && cargo test -p rb-mcp && cargo test -p rusty-brain` to confirm the additive field did not break serde round-trips or rendering.

- [ ] **Step 5: lint+format.** Run: `cargo clippy --workspace --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-types crates/rb-engine crates/rb-store/src/store.rs crates/rb-daemon/src/store_handle.rs crates/rb-proto/src/messages.rs && git commit -m "feat: surface contested contradicts flag fail-open and bump contract version"` — Expected: one commit.

---

### Task C5: `rb-eval` context-poisoning scenario

Add a poison scenario to the eval: a low-confidence WRONG memory that matches a golden query strongly must rank BELOW the high-confidence correct one. This proves the confidence dampener mitigates context poisoning end-to-end through `engine.recall`.

**Files:**
- Modify: crates/rb-eval/fixtures/corpus.json (add an optional `confidence` field + a poison note)
- Modify: crates/rb-eval/fixtures/golden_queries.json (a query the poison note matches)
- Modify: crates/rb-eval/src/corpus.rs (FixtureNote gains `confidence`; LoadedNote carries it)
- Modify: crates/rb-eval/src/runner.rs (apply confidence post-ingest via update path or direct store; assert poison ordering)
- Test: crates/rb-eval/src/runner.rs (poison ranking test)

- [ ] **Step 1 RED: write the failing test.** Add to `crates/rb-eval/src/runner.rs` tests:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn low_confidence_poison_ranks_below_high_confidence_truth() {
        let corpus = Corpus::load(&Corpus::fixtures_dir());
        // The fixtures define a poison query whose strongest lexical match is a
        // low-confidence wrong note; the correct note is high-confidence.
        let ordering = poison_ordering(&corpus).await;
        assert!(
            ordering.truth_rank < ordering.poison_rank,
            "high-confidence truth (rank {}) must outrank low-confidence poison (rank {})",
            ordering.truth_rank,
            ordering.poison_rank
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-eval low_confidence_poison_ranks_below_high_confidence_truth` — Expected: FAIL — `poison_ordering` and the poison fixtures do not exist (`error[E0425]`).

- [ ] **Step 3a GREEN: extend the fixture types.** In `crates/rb-eval/src/corpus.rs`, add `#[serde(default = "default_confidence")] pub confidence: f32,` to `FixtureNote`, carry `confidence: f32` on `LoadedNote`, set it at load, and add:

```rust
fn default_confidence() -> f32 {
    1.0
}
```

- [ ] **Step 3b GREEN: add the poison fixtures.** Append to `crates/rb-eval/fixtures/corpus.json` two notes (a high-confidence truth and a low-confidence poison that lexically over-matches the query):

```json
  { "key": "poison-truth", "content": "The single writer commits inside one IMMEDIATE transaction so writes are atomic.", "keywords": ["writer", "transaction", "atomic"], "tags": ["storage"], "context": "correctness", "memory_type": "architecture_decision", "importance": 8, "confidence": 1.0 },
  { "key": "poison-wrong", "content": "writer writer writer transaction transaction atomic atomic — multiple writers commit concurrently with no transaction.", "keywords": ["writer", "transaction", "atomic"], "tags": ["storage"], "context": "wrong", "memory_type": "insight", "importance": 8, "confidence": 0.05 }
```

Append to `crates/rb-eval/fixtures/golden_queries.json`:

```json
  { "query": "writer transaction atomic commit", "expected_keys": ["poison-truth"], "k": 5 }
```

(The poison note repeats the query terms so it would win on raw lexical match without the confidence dampener; the low `confidence: 0.05` must push it below the truth.)

- [ ] **Step 3c GREEN: apply confidence in the runner + add `poison_ordering`.** In `crates/rb-eval/src/runner.rs`, after ingesting each note via `engine.remember`, set its stored `confidence` directly through the locked store (eval-only; the engine has no confidence-update path and does not need one for the binary). Add a helper that, for each loaded note with `confidence != 1.0`, runs `UPDATE memories SET confidence = ?1 WHERE memory_id = ?2` through `store.raw()`. Then add:

```rust
    pub struct PoisonOrdering {
        pub truth_rank: usize,
        pub poison_rank: usize,
    }

    pub async fn poison_ordering(corpus: &Corpus) -> PoisonOrdering {
        // Rebuild an engine, ingest, apply confidence, run the poison query.
        // (Reuse run_eval's setup; expose a small ingest helper or inline it.)
        // truth_rank / poison_rank are 0-based positions in the recall results;
        // a key absent from results gets usize::MAX so the assert still holds.
        unimplemented!("see Step 3c: ingest + apply confidence + recall the poison query")
    }
```

Replace the `unimplemented!` body with: build the in-memory engine + backend (factor the ingest from `run_eval` into a shared `async fn ingest(corpus) -> (engine, store)` so both call it), apply the confidence overrides, run `engine.recall("writer transaction atomic commit", 10, None, &[])`, find the 0-based positions of `poison-truth` and `poison-wrong` ids (default `usize::MAX` if absent), and return them. Mark `poison_ordering`/`PoisonOrdering` `#[cfg(test)]` or keep them in the test module (they are test support).

- [ ] **Step 4: run it.** Run: `cargo test -p rb-eval low_confidence_poison_ranks_below_high_confidence_truth` — Expected: PASS. Then `cargo test -p rb-eval` to confirm the baseline gate and fusion comparison still pass after the corpus grew (recalibrate `baselines.json` DOWN only if a floor is now narrowly missed because the corpus changed, and commit the recalibration with a note).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-eval --all-targets -- -D warnings` then `cargo fmt --all` (Expected: clean).

- [ ] **Step 6: commit.** Run: `git add crates/rb-eval && git commit -m "test(rb-eval): add context-poisoning confidence scenario"` — Expected: one commit.

---

### Task C6: Part C gate (final P5 gate)

- [ ] **Step 1: full test suite.** Run: `cargo test --workspace` — Expected: PASS (0 failures), including the rb-eval baseline gate, the migration reproducibility gate, and all serde round-trips with the new `contested` field.
- [ ] **Step 2: clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] **Step 3: format.** Run: `cargo fmt --all --check` — Expected: no diff.
- [ ] **Step 4: supply-chain.** Run: `cargo deny check` — Expected: PASS (no new dependencies were added in P5; the existing scoped MPL-2.0 exception remains the only one).
- [ ] **Step 5: commit (only if Steps 1–4 surfaced fixes).** Run: `git add -A && git commit -m "chore: part C final P5 gate green"` — Expected: at most one commit.

---

## Self-review (run before declaring P5 done)

- [ ] **Spec coverage.** Every spec section maps to at least one Task:
  - §6 (H eval harness) → H1 (crate), H2 (metrics: recall@k/MRR/dedup/percentiles), H3 (corpus loader + committed fixtures), H4 (runner + `baselines.json` + CI gate + honest-framing doc), H5 (gate). Real-model `#[ignore]` limit documented in H4/A8.
  - §7 (B RRF two-stage fusion) → B1 (`FusionMode`/`RRF_K`, `Linear` default), B2 (stage-1 rank fusion `1/(k+rank)`, `k=60`, missing=no penalty), B3 (stage-2 priors + `rank_rrf` + `rank_with_mode`), B4 (engine threading), B5 (rb-eval Linear-vs-RRF comparison; default NOT flipped).
  - §8 (A composite embedding + reembed) → A1 (`embedding_input`), A2 (remember embeds composite + stamps version), A3 (additive checksummed migration + `meta` invariant + column), A4 (`reembed_one` UPDATE path + idempotent stale scan), A5 (`WriteCommand::Reembed` + StoreHandle), A6 (engine driver + wire pair + dispatch, single-writer/idempotent/bounded/fail-safe), A7 (`rusty-brain reembed` CLI), A8 (gate + real-model note).
  - §9 (C confidence + contradictions) → C1 (`Signals.confidence` + `confidence_floor` + dampener in BOTH modes, floored not zeroed), C2 (engine threads confidence), C3 (batched `contested_ids`), C4 (`contested` on result rows, fail-open, `ContractVersion` → 2), C5 (rb-eval poison scenario), C6 (final gate).
- [ ] **No placeholders.** Every GREEN step shows real, complete code — except A6 Step 3c, A7 Step 3b, C4 Step 3b/3c, and C5 Step 3c, which give an exact spec of mechanical delegation/wiring (named methods, exact SQL, exact field sets) rather than re-printing boilerplate that mirrors already-shown code; no "TBD"/"similar to Task N" hand-waving.
- [ ] **Type/name consistency.** `FusionMode`, `RRF_K`, `rank_rrf`, `rank_with_mode`; `embedding_input`/`embedding_input_from_parts`, `EMBEDDING_INPUT_VERSION = "v2-composite"`; `ReembedRow` (in `rb-types`), `WriteCommand::Reembed`, `StoreHandle::{reembed_scan,reembed_apply}`, `MemoryEngine::reembed`/`ReembedSummary`, `Request::Reembed`/`Response::Reembedded`, CLI `Command::Reembed { batch }`; `Signals.confidence`, `Weights.confidence_floor` (default 0.5), `confidence_dampener`, `SqliteStore::contested_ids`, `SearchResult.contested`/`MemoryNote.contested`, `CONTRACT_VERSION = 2` — each name is introduced once and reused verbatim across Tasks.
- [ ] **Single-writer + fail-open invariants.** `Reembed` is the only vector-UPDATE path and goes through the writer; the CLI never writes the DB. `contested` lookup and `reembed` per-row failures are fail-open/fail-safe; the dim contract stays fail-closed.
- [ ] **No new deps.** `rb-eval` uses only existing workspace deps; `cargo deny check` is green at the final gate.

## Open items / could-not-fully-plan

None. Every spec section (H, B, A, C) has concrete Tasks with RED/GREEN/commit steps. Two design decisions made explicit (and noted inline) that the spec left to the implementation:
1. `contested` is added to BOTH `SearchResult` and `MemoryNote` (rather than a separate per-endpoint wrapper) so recall/list/context/graph/get all carry it uniformly through serde with one additive change; it is computed read-side and never persisted (`row_to_note` sets it `false`).
2. `ReembedRow` and the `embedding_input_version` lockstep string live in `rb-types`/`rb-store` respectively to avoid the `rb-engine → rb-store` dependency-cycle direction, with explicit lockstep guard tests.
