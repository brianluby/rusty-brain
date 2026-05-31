# rusty-brain — Architecture Design Spec

- **Status:** Draft (approved in brainstorming; pending written-spec review)
- **Date:** 2026-05-31
- **Author:** Brian Luby
- **Supersedes:** N/A (greenfield)
- **Origin:** Derived from an architecture review of `mnemosyne`. This spec deliberately keeps that system's good ideas and discards its structural mistakes.

---

## 1. Context & Motivation

`mnemosyne` set out to be an agentic memory system but became a ~100K-LOC single crate (988 transitive deps) bundling a CRDT text editor, a `ractor`/`iroh` actor cluster, a gRPC server, a DSPy/Python ML bridge, a TUI dashboard, and a self-updater. The actual "memory" core was a minority of the code and carried **critical correctness bugs**:

- **Embedding-dimension contradiction** — the default model emitted 768-dim vectors into an `F32_BLOB(384)` column (a third value, 1536, lived in dead code). Semantic search was unreliable out of the box.
- **Non-reproducible schema** — columns/tables the code queried (`archived_at`, `memory_vectors`) were never created by the committed migrations ("ghost migrations"); fresh installs hit runtime "no such column" errors.
- **Dual SQLite stacks** (`libsql` + `rusqlite`/`sqlite-vec`) storing memories and vectors in *separate* databases with no shared transaction → desync.
- **Security theater** — an unauthenticated, permissive-CORS HTTP API; cross-process HMAC auth keyed on a guessable `mnemosyne-secret-$USER` default; a "self-update" that ran an untrusted build script and "verified" the binary *after* execution.

`rusty-brain` is the focused rebuild: **a shared memory substrate that many agents read and write concurrently — and nothing more.**

## 2. Goals

1. Persistent, project-aware semantic memory for AI agents, accessed primarily via MCP.
2. **Concurrent multi-agent access** as a first-class, baseline requirement.
3. Hybrid retrieval: full-text (FTS5) + vector similarity (sqlite-vec) + graph links.
4. Local-first, zero-ops: one binary, one database file, no external services required.
5. Correct and reproducible by construction: one source of truth, schema rebuildable from git, CI-enforced.
6. Lean dependency closure; modular workspace with compiler-enforced boundaries.

## 3. Non-Goals (explicit)

- **Not an orchestrator / task manager.** No work items, no agent lifecycle, no supervision, no scheduling of what agents *do*. Coordinating concurrent *access to memory* is in scope; coordinating *work* is not.
- **No collaborative text editor, no p2p networking, no TUI dashboard, no gRPC, no Python/ML bridge, no self-update.**
- No networked/multi-host server in v1 (single-machine, per-user). Reserved as a future option behind a clean seam.

## 4. Locked Decisions

| Decision | Choice | Rationale |
|---|---|---|
| v1 scope | **Lean shared-memory core** | Smallest correct thing that delivers the value prop; intelligence features deferred behind seams. |
| Access model | **Local memory daemon (single writer)** | Natural serialization of writes for many concurrent agents; one place to enforce isolation and broadcast change-events. |
| Storage | **SQLite + sqlite-vec, one DB** | Single source of truth; memories + FTS + vectors in one transaction. Directly fixes the dual-DB desync and dimension bugs. |
| Embeddings | **Pluggable `EmbeddingProvider` trait, remote default = Voyage** | Keeps the core's dependency closure small; local ONNX optional behind a feature. |
| Daemon transport | **Length-delimited JSON over Unix domain socket** | Debuggable/observable; robust, universally supported. (TOON evaluated and rejected — see §20.) |

## 5. Guiding Principles

1. **Workspace of focused crates**, never a monolith. Boundaries are a compiler-checked DAG.
2. **One database, one transaction, one source of truth.**
3. **Embedding dimension is a single configured value, enforced at init, fail-closed.**
4. **Migrations are file-discovered, checksummed, reproducible-from-git**, with a CI test that builds a fresh DB and exercises every query path.
5. **Memory is a substrate, not an orchestrator.**
6. **Security boundaries fail closed**; isolation enforced server-side, never trusted from the client.
7. **Lean by default**: heavy/optional things live behind features or separate crates. `cargo-deny` + `cargo-audit` in CI from commit one.
8. **No `unwrap()`/`expect()`/`panic!` on request paths** (clippy-denied workspace-wide; allowed in tests).

## 6. Architecture Overview

```
   agent A (Claude)        agent B (Claude)        human (terminal)
        | stdio                 | stdio                 | argv
   rusty-brain mcp         rusty-brain mcp        rusty-brain recall
        |                       |                       |
        +------- Unix domain socket (0600, $XDG_RUNTIME_DIR) -------+
                                                                    v
                       +--------------  rb-daemon  --------------+
   per-conn tokio task |  Readers: deadpool read-pool (WAL)  ----+--> SQLite (WAL)
   (bounded, framed)   |  Writer:  ONE dedicated thread + mpsc --+--> (single writer)
                       |  Events:  tokio::broadcast on commit    |
                       |  Namespace isolation enforced here      |
                       +-----------------------------------------+
```

One binary (`rusty-brain`) with subcommands; the engine is a set of library crates.

## 7. Crate Decomposition (workspace)

| Crate | Responsibility | Intended direct deps | Internal deps |
|---|---|---|---|
| `rb-types` | Domain vocabulary: `MemoryId`, `Namespace`, `MemoryNote`, `MemoryType`, `LinkType`, `MemoryLink`, `SearchQuery`, `SearchResult`, `Error` | serde, uuid, chrono, thiserror | — (leaf) |
| `rb-store` | SQLite+sqlite-vec engine: schema, migrations, FTS, vector KNN, graph CTE; `Store` trait + `SqliteStore` | rusqlite (bundled), sqlite-vec, deadpool-sqlite | rb-types |
| `rb-embed` | `EmbeddingProvider` trait + Voyage impl; local ONNX behind `local` feature | reqwest (·fastembed w/ feature) | rb-types |
| `rb-search` | Hybrid ranking as pure, unit-tested functions over candidates | — | rb-types |
| `rb-engine` | Single-request orchestration: namespace resolve → embed → store → link → search; namespace detection | — | store, embed, search, types |
| `rb-proto` | Daemon wire protocol (request/response enums) + UDS client + length-delimited JSON framing | serde, serde_json, tokio, tokio-util | rb-types |
| `rb-daemon` | Single-writer service logic: writer thread, read pool, UDS listener, change broadcast, isolation, shutdown | tokio | engine, proto, types |
| `rusty-brain` (bin) | One binary; subcommands `serve`, `mcp`, and client ops (`remember`/`recall`/…) | clap, tokio, anyhow | daemon, proto, engine |

**Dependency budget:** the default build's transitive closure is tracked in CI; ONNX (`local`) and any future networking server are opt-in and excluded by default. Target: low hundreds of transitive crates, not ~1000.

## 8. Concurrency & Access Model

- **Single writer, serialized.** All mutations flow through a bounded `mpsc` to **one dedicated OS thread** owning the write connection (`rusqlite` is synchronous; the writer must not run on the tokio runtime). No `SQLITE_BUSY`, no corruption, no lost writes.
- **Concurrent readers.** WAL mode + a small read-connection pool (`deadpool-sqlite`); reads run via the blocking pool and never block the writer or each other.
- **Change events.** After each successful commit the writer publishes `MemoryChanged { id, namespace, kind }` to a `tokio::broadcast` channel. (Enables the deferred `subscribe` feature with no new machinery — notifications, not coordination.)
- **Deterministic single-instance.** The daemon binds the UDS plus a pidfile; clients (`mcp`, CLI) auto-start it if absent and connect otherwise. Stale sockets are reclaimed. (Replaces mnemosyne's port-scanning owner/client hack.)
- **Isolation is the real boundary.** Each client handshake establishes a namespace context (from cwd / git root / `CLAUDE.md`). The daemon scopes **every** query server-side; a client cannot read outside its granted scope. Fail closed. Local trust derives from the **0600 socket permissions**, not a guessable secret.
- **Backpressure** via bounded channels on both the connection and writer queues.

## 9. Data Model

One database file. All tables co-located; a `remember` writes the memory + FTS row + vector + links in **one transaction**.

```sql
-- meta: single source of truth for invariants
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- seeded at init: schema_version, embedding_model, embedding_dim

CREATE TABLE _migrations (
  version    INTEGER PRIMARY KEY,
  name       TEXT NOT NULL,
  checksum   TEXT NOT NULL,
  applied_at INTEGER NOT NULL
);

CREATE TABLE memories (
  memory_id        TEXT PRIMARY KEY,
  namespace        TEXT NOT NULL,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  content          TEXT NOT NULL,
  summary          TEXT NOT NULL,
  keywords         TEXT NOT NULL,   -- JSON array
  tags             TEXT NOT NULL,   -- JSON array
  context          TEXT NOT NULL DEFAULT '',
  memory_type      TEXT NOT NULL CHECK (memory_type IN (
                     'architecture_decision','code_pattern','bug_fix','configuration',
                     'constraint','entity','insight','reference','preference')),
  importance       INTEGER NOT NULL CHECK (importance BETWEEN 1 AND 10),
  confidence       REAL NOT NULL CHECK (confidence BETWEEN 0.0 AND 1.0),
  related_files    TEXT NOT NULL DEFAULT '[]',
  access_count     INTEGER NOT NULL DEFAULT 0,
  last_accessed_at INTEGER,
  archived_at      INTEGER,          -- NULL = active (soft delete, in BASE schema)
  superseded_by    TEXT REFERENCES memories(memory_id),
  embedding_model  TEXT NOT NULL
);
CREATE INDEX idx_mem_ns        ON memories(namespace);
CREATE INDEX idx_mem_created   ON memories(created_at);
CREATE INDEX idx_mem_importance ON memories(importance);
CREATE INDEX idx_mem_active    ON memories(archived_at) WHERE archived_at IS NULL;

CREATE TABLE memory_links (
  source_id  TEXT NOT NULL REFERENCES memories(memory_id),
  target_id  TEXT NOT NULL REFERENCES memories(memory_id),
  link_type  TEXT NOT NULL CHECK (link_type IN
               ('extends','contradicts','implements','references','supersedes')),
  strength   REAL NOT NULL CHECK (strength BETWEEN 0.0 AND 1.0),
  reason     TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  PRIMARY KEY (source_id, target_id, link_type)
);

CREATE VIRTUAL TABLE memories_fts USING fts5(
  content, summary, keywords, tags,
  content='memories', content_rowid='rowid'
);

-- DIM is substituted from meta.embedding_dim at init; NOT hardcoded.
CREATE VIRTUAL TABLE memory_vectors USING vec0(
  memory_id TEXT PRIMARY KEY,
  embedding float[DIM]
);
```

**Invariants enforced in code:**
- On startup the daemon asserts `meta.embedding_dim == provider.dim()` and **refuses to run** on mismatch.
- Rows are decoded by **explicit column names** (never positional `SELECT *`).
- FTS kept in sync via triggers, validated by integration tests.

**Migrations:** ordered SQL files, discovered at runtime, each applied transactionally with its checksum recorded. No hardcoded-in-Rust migration list. No divergent schema trees. A CI test builds a fresh DB from committed files and runs the full query surface (see §15).

## 10. Embeddings

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn dim(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error>;
}
```

- Default impl: **Voyage** (`rb-embed`, via `reqwest`), with a request timeout and bounded concurrency. API key via env / OS keychain (never logged, never on disk in plaintext).
- `local` feature: ONNX/`fastembed` impl, isolated so `onnxruntime` is absent from the default closure.
- The provider's `dim()` is the contract checked against `meta.embedding_dim`. Changing models is a deliberate, migration-gated operation.

## 11. Search & Ranking

- **Candidate generation:** FTS5 keyword query ∪ vector KNN (sqlite-vec) ∪ optional bounded graph expansion (recursive CTE, max depth configurable).
- **Ranking:** `rb-search` combines normalized component scores in a **pure, unit-tested** function. Default weights (configurable, documented honestly): vector 0.5, keyword 0.3, graph/importance/recency 0.2.
- **Scale honesty:** sqlite-vec brute-force KNN is appropriate at small/medium scale. The scale ceiling and the absence of an ANN index are documented as known limits; ANN is a future option, not a hidden surprise.

## 12. Interfaces

**MCP tools** (agent-facing, JSON results — no TOON):

| Tool | Inputs | Output |
|---|---|---|
| `remember` | content, context?, importance?, namespace? | memory_id |
| `recall` | query, scope?, type?, tags?, limit? | uniform rows: {id, summary, type, importance, tags, score} |
| `get` | memory_id | full `MemoryNote` (content + links) |
| `list` | scope, since?, min_importance?, limit? | uniform summary rows |
| `graph` | memory_id, depth? | connected memories |
| `update` | memory_id, updates | ok |
| `delete` | memory_id | ok (soft archive) |
| `context` | — | project context payload (recent + important + graph overview) |

`recall`/`list`/`context` return lean uniform rows (summary + metadata); full bodies are fetched on demand via `get`. All inputs validated against JSON Schema at the boundary.

**CLI:** mirrors the tools, plus `serve` (run daemon), `status`, and daemon control.

## 13. Error Handling & Resilience

- `rb-types::Error` (thiserror) domain enum; library crates return `Result<T, Error>`. Binaries use `anyhow` at the top level only.
- Boundary mapping: MCP → JSON-RPC error codes; daemon proto → typed error responses. No internal-detail leakage.
- Timeouts on all outbound embedding calls (`tokio::time::timeout`); bounded channels everywhere; `spawn_blocking`/dedicated thread for synchronous SQLite work.
- Graceful shutdown: drain the write queue, checkpoint WAL, release socket + pidfile.

## 14. Security Model

- **Transport trust = filesystem.** UDS at `$XDG_RUNTIME_DIR/rusty-brain/sock`, mode `0600`, in a `0700` dir. Only the user's processes can connect. No guessable shared secret.
- **No network surface by default.** No HTTP/gRPC/p2p. (A networked server, if ever added, is a separate opt-in crate with real auth.)
- **Namespace isolation enforced server-side, fail closed.**
- **Secrets:** env var first, OS keychain optional (feature); masked in logs; never plaintext on disk.
- **No self-update.** Distribution via normal release artifacts / package manager.

## 15. Testing Strategy

- **TDD**, 80%+ coverage target; workspace clippy/rust lint gates (deny `unwrap_used`/`expect_used` outside tests).
- **Per-crate unit tests:** type invariants; ranking determinism (property tests); proto round-trip.
- **Migration reproducibility gate (CI):** build a DB from committed migrations only, then exercise **every** query path (store, recall, get, list, graph, update, archive, embeddings). Fails if any column/table is missing — the direct guard against ghost migrations.
- **Concurrency test:** N client tasks hammering the daemon concurrently; assert no `SQLITE_BUSY`, no lost writes, and namespace isolation holds across clients.
- **MCP contract tests** for each tool's schema and error mapping.
- `cargo-deny` + `cargo-audit` in CI.

## 16. What We Deliberately Drop from mnemosyne

CRDT text editor · `ractor`/`iroh` actor cluster & p2p · gRPC server · DSPy/Python bridge · TUI dashboard · self-update-via-git · permissive-CORS HTTP API · dual SQLite stacks · all task/work-item/orchestration concepts · HMAC-with-guessable-secret cross-process auth.

## 17. Phasing / Roadmap

- **P0 — Foundation:** workspace skeleton, `rb-types`, `rb-store` + reproducible migrations + the CI reproducibility gate, lints + deny/audit.
- **P1 — Core engine + daemon:** `rb-engine` (remember/recall/get/list), `rb-embed` (Voyage) + dim contract, `rb-search` hybrid ranking, `rb-daemon` (writer thread + read pool + UDS), `rb-proto`, CLI client.
- **P2 — Agent surface:** `mcp` adapter + daemon auto-start, namespace detection (git/`CLAUDE.md`), graph links + traversal.
- **P3 — Deferred (behind existing seams):** `subscribe` change-stream (cross-agent awareness), memory evolution (consolidation / link decay / importance recalibration) as opt-in daemon jobs, `local` embedding feature.

## 18. Open Questions

- LLM-based enrichment (summary/keyword/type/importance, semantic link generation) — keep mnemosyne's idea, but decide whether it's a P1 `rb-engine` step or a P2 enhancement. Default: minimal heuristic enrichment in P1, LLM enrichment opt-in in P2.
- Namespace scope-resolution policy (session→project→global widening) — confirm defaults during P1.

## 19. Future Options (seamed, not built)

- Networked multi-host server (separate crate, real auth) if memory ever needs to span machines.
- ANN vector index if scale exceeds sqlite-vec brute force.
- Additional embedding providers via the trait.

## 20. Rejected Alternatives

- **TOON for transport or MCP responses.** TOON (~40% fewer LLM tokens for *uniform arrays*) is wrong for the UDS transport (not LLM-facing; its truncation-blindness is a liability for a wire protocol) and only marginally useful for MCP responses (it compacts metadata, not the freeform `content` that dominates large recalls). Rejected for v1 to avoid unproven dependency/risk; revisit only with empirical token measurements on real payloads.
- **libsql/Turso, Postgres/pgvector** — heavier or non-local-first; reserved as future options.
- **Embedded-lib shared-file access** — simpler but suffers write contention and complicates cross-agent notifications; the single-writer daemon is cleaner for concurrent multi-agent use.

## Appendix A — Lesson → Design Traceability

| mnemosyne finding | rusty-brain mitigation |
|---|---|
| Single 100K-LOC crate, 988 deps | Workspace of focused crates; dep budget tracked in CI (§7) |
| Dual SQLite DB desync | One DB, one transaction (§9) |
| 384/768/1536 dimension contradiction | `meta.embedding_dim`, single value, fail-closed startup check (§9, §10) |
| Ghost migrations / non-reproducible schema | File-discovered, checksummed migrations + CI reproducibility gate (§9, §15) |
| Unauthenticated permissive-CORS HTTP API | No network by default; UDS + 0600 perms (§14) |
| HMAC keyed on guessable `secret-$USER` | Filesystem-based local trust; server-side isolation (§8, §14) |
| Self-update runs untrusted build, verifies after | No self-update; package-manager distribution (§14, §16) |
| Scope creep (editor, actors, gRPC, ML) | Explicit non-goals; memory-substrate-only (§3, §16) |
| 929 `unwrap()` on crash-exposed paths | Lint-denied outside tests; explicit error types (§5, §13) |
