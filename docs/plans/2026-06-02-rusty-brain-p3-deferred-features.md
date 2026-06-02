# rusty-brain — P3 (Deferred Features Behind Existing Seams) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Implement Parts strictly in build order **Q → R → S → T → U**; each Part ends with a gate that must be green before the next Part starts.

**Goal:** Ship spec §17's deferred features — a `subscribe` change-stream, opt-in memory-evolution jobs (link decay, consolidation, importance recalibration), and on-device `local` embeddings — by wiring the seams that P0–P2 already built, adding no new write-path machinery and no new default dependencies.

**Architecture:** P3 is additive and behind existing seams. `subscribe` exposes the daemon's existing `tokio::broadcast` of `MemoryChanged` as a streaming `Request::Subscribe` (consumed by a `rusty-brain subscribe` CLI stream and an rb-mcp `poll_changes` ring buffer), namespace-scoped server-side. The three evolution jobs live in a new `rb-daemon::jobs` module sharing one `run_once()` core, driven by **both** an opt-in interval scheduler and a `rusty-brain evolve <job>` CLI trigger (a new `Request::RunJob`); every mutation funnels through the existing **single writer** (`StoreHandle` `WriteCommand`s) — jobs never open a parallel write path. Consolidation wires the already-built-but-unwired `SqliteStore::supersede` primitive. `local` embeddings add a third `ProviderKind` arm via the `fastembed` crate, strictly behind a `local` cargo feature excluded from the default build closure.

**Tech Stack:** Rust 2021 (stable, pinned). Workspace crates: rb-types, rb-store (rusqlite + sqlite-vec), rb-proto (length-delimited JSON over a Unix socket), rb-engine, rb-search, rb-embed, rb-enrich, rb-daemon, rb-mcp, rusty-brain. New deps: `toml` (job config; MIT/Apache, default closure) and `fastembed` (`local` feature only, Apache-2.0, **excluded** from default closure). Async via tokio. Tests are TDD, in-process, offline (DeterministicProvider; real-model embedding tests `#[ignore]`).

**Reference spec:** `docs/specs/2026-05-31-rusty-brain-architecture-design.md` — §17 (P3), §8 (concurrency/broadcast), §9 (data model), §10 (embeddings + local), §11 (ranking), §7 (dep budget), §15 (testing). Prior plans: `docs/plans/2026-05-31-rusty-brain-p0-foundation.md` (style template).

---

## Hard rules (carry forward from P0–P2; apply to every task)

- **TDD:** failing test first (RED), minimal implementation (GREEN), then clippy + fmt, then commit. One logical change per commit.
- **Conventional commits**, lowercase, crate-scoped, one line, **NO AI attribution** (no "Generated with…", no `Co-Authored-By`).
- **Single-writer discipline:** ALL store mutations go through the daemon's single writer thread (`StoreHandle` `WriteCommand`s); reads go via the read pool. Never share `SqliteStore` across tasks. The `rusty-brain evolve` CLI sends a `Request::RunJob` to the running daemon — it never writes the DB directly.
- **Namespace isolation stays enforced server-side and fails closed:** a subscriber sees only its handshake namespace's events; consolidation never merges across namespaces.
- **No-panic in non-test code:** workspace lints deny `unwrap_used`/`expect_used`/`panic`. Return `rb_types::Error` instead. Test modules opt out with `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- **Error plumbing:** prefer reusing existing `rb_types::Error` variants (`Storage`, `InvalidArgument`, `Embedding`). Adding a variant requires arms in `rb-proto::error_kind`, `rb-proto::response_error_to_error`, and `rb-daemon::error_to_response` plus their tests — this plan avoids new variants.
- **Evolution jobs are OFF by default**, bounded (a `batch_limit` per pass), idempotent (a second pass over unchanged data writes nothing), and fail-safe (reuse the writer's `catch_unwind` + reopen-on-panic path; a job failure is logged, never fatal).
- **No live network in CI:** subscribe/evolution tested in-process; the `local` model download is runtime-only — real-model tests are `#[ignore]`; unit tests use offline fixtures / `DeterministicProvider`.
- **Per-Part gate** (final task of each Part): `cargo test --workspace`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo fmt --all --check`. Parts that add deps (R, U) also run `cargo deny check`.
- **Commands run from the worktree root** `/Volumes/raid1/repos/rusty-brain-p3` (so commands are plain `cargo test -p <crate>`).

## Seam map (verified against `origin/main`; the exact code each Part builds on)

| Seam | Location | Used by |
|---|---|---|
| `MemoryChanged { id, namespace, kind }`, `ChangeKind {Created,Updated,Archived}` | `rb-daemon/src/change.rs` (Part Q moves these into `rb-types`, re-exports from rb-daemon) | Q, S |
| `broadcast::channel(BROADCAST_CAPACITY=256)`, `publish_change()` after Insert/Update/Archive commits, `DROPPED_BROADCASTS` counter, `StoreHandle::subscribe()` | `rb-daemon/src/store_handle.rs` | Q |
| Length-delimited JSON wire protocol (`Request`/`Response` enums, round-trip tests, `MAX_FRAME_BYTES=1 MiB`) | `rb-proto/src/{messages.rs,frame.rs,codec.rs,client.rs}` | Q, R |
| Per-connection handshake binds one namespace; `dispatch(&engine, req)` loop | `rb-daemon/src/server.rs` | Q, R |
| `SqliteStore::supersede(old,new)` — transactional, **unwired** (only tests call it) | `rb-store/src/store.rs` | S |
| `memory_links` (`strength REAL CHECK 0..1`, `created_at`), `add_link`/`load_links` | `rb-store/src/store.rs` | R |
| `access_count`/`last_accessed_at` recorded by `record_access(es)` — **write-only today** | `rb-store/src/store.rs` | T |
| `validate_importance(u8)` enforcing `1..=10` | `rb-types/src/validate.rs` | T |
| Single writer: `WriteCommand` enum, `writer_loop`, `run_store_op` (catch_unwind + reopen) | `rb-daemon/src/store_handle.rs` | R, S, T |
| `EmbeddingProvider` trait, `DeterministicProvider`, `VoyageProvider` | `rb-embed/src/{provider.rs,deterministic.rs,voyage.rs}` | U |
| `ProviderKind {Voyage,Deterministic}` + `select_provider_kind()` (the third arm goes here) | `rusty-brain/src/serve.rs` | U |
| `SharedEmbedder` (Arc + Semaphore=4); dim contract `seed_or_verify_dim()` → `DimensionMismatch`, refuse-to-run | `rb-daemon/src/{shared_embedder.rs}`, `rb-store/src/store.rs` | U |

## Build order & dependencies

```text
Part Q  subscribe change-stream            (independent; validates the broadcast seam)
Part R  link decay + shared job scaffolding (brings up run_once/config/scheduler/RunJob; S and T reuse it)
Part S  consolidation                       (depends on R's scaffolding; wires supersede)
Part T  importance recalibration            (depends on R's scaffolding; reuses the update writer path)
Part U  local ONNX embeddings               (independent; behind the `local` feature)
```

Parts R, S, and T share one contract introduced in Part R: `JobKind` (in `rb-types`), `JobSummary`, `run_once(kind, &StoreHandle, &JobsConfig)`, the `JobsConfig` TOML structs (every job disabled by default), the in-daemon scheduler, and the `Request::RunJob` path. Parts S and T **consume** these names verbatim and only add their own job arm + read/write helpers — they never redefine the contract.

---

## Part Q — subscribe change-stream (CLI stream + MCP poll)

This Part adds a live change-notification stream on top of the daemon's existing best-effort `MemoryChanged` broadcast. The `MemoryChanged`/`ChangeKind` types move from `rb-daemon` into `rb-types` so `rb-proto::Response` can name them, then `rb-proto` gains a `Request::Subscribe` plus streamed `Response::Change`/`Response::Lagged` frames. The daemon's per-connection loop grows a streaming branch that drains the namespace-scoped broadcast without ever blocking the writer, the `rb-proto` client gains a `recv_change` stream consumer, the CLI gains `rusty-brain subscribe`, and the MCP adapter gains a background subscriber feeding a bounded ring that a new `poll_changes` tool drains. All commands run from the worktree root `/Volumes/raid1/repos/rusty-brain-p3`.

HARD RULES honored throughout: a lagging subscriber NEVER blocks the writer or other clients (the broadcast channel already drops oldest on lag and reports `RecvError::Lagged`); namespace scoping is filtered server-side and fails closed; the MCP ring is bounded and lossy with an explicit dropped counter; no `.unwrap()`/`.expect()`/`panic!` in non-test code.

---

### Task Q1: rb-types `change.rs` — move change types

Move `MemoryChanged` + `ChangeKind` into the leaf crate `rb-types` so both `rb-proto` and `rb-daemon` can name them without a dependency cycle. Add `PartialEq` to both so wire round-trip and streaming tests can assert equality directly.

**Files:**
- Create: crates/rb-types/src/change.rs
- Modify: crates/rb-types/src/lib.rs

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-types/src/change.rs` with this exact content (test module included — the impl arrives in Step 3):

```rust
//! Change-notification vocabulary: what happened to a memory, broadcast after a
//! successful write. Notification only — never coordination. Lives in `rb-types`
//! (the leaf crate) so both `rb-proto` (wire `Response`) and `rb-daemon` (the
//! broadcast channel) can name it without a dependency cycle.

use crate::{MemoryId, Namespace};
use serde::{Deserialize, Serialize};

/// What happened to a memory. Published after a successful commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Created,
    Updated,
    Archived,
}

/// Change-notification event broadcast on every successful write (spec §8).
///
/// Notification only — never coordination. Enables the `subscribe` feature with
/// no new machinery: the daemon already publishes one of these per committed
/// write on a `tokio::sync::broadcast` channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChanged {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub kind: ChangeKind,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::{MemoryId, Namespace};

    #[test]
    fn change_kind_round_trips_all_variants() {
        for kind in [
            ChangeKind::Created,
            ChangeKind::Updated,
            ChangeKind::Archived,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: ChangeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn memory_changed_round_trips_clones_and_eq() {
        let evt = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: MemoryChanged = serde_json::from_str(&json).unwrap();
        // PartialEq is required by streaming/wire tests downstream.
        assert_eq!(evt, back);
        // Clone is required so broadcast subscribers each get an owned copy.
        assert_eq!(evt.clone(), evt);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-types change` — Expected: FAIL — `change.rs` is not yet declared in `lib.rs`, so the module is unreachable and the new tests do not compile/run (`error[E0432]`/unresolved module). (The file compiles standalone but is not wired in.)

- [ ] **Step 3 GREEN: wire the module + re-export.** Edit `crates/rb-types/src/lib.rs` to declare and export `change`. Replace the module list and re-export list as follows.

Add the module declaration (after `mod memory_type;` is fine; keep alphabetical-ish ordering):

```rust
mod change;
mod error;
mod link;
mod link_type;
mod memory;
mod memory_id;
mod memory_type;
mod namespace;
mod query;
mod validate;

pub use change::{ChangeKind, MemoryChanged};
pub use error::{Error, Result};
pub use link::MemoryLink;
pub use link_type::LinkType;
pub use memory::MemoryNote;
pub use memory_id::MemoryId;
pub use memory_type::MemoryType;
pub use namespace::Namespace;
pub use query::{MemoryUpdates, SearchQuery, SearchResult};
pub use validate::validate_importance;
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-types change` — Expected: PASS (2 tests: `change_kind_round_trips_all_variants`, `memory_changed_round_trips_clones_and_eq`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-types/src/change.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add MemoryChanged/ChangeKind change vocabulary"` — Expected: one commit.

---

### Task Q2: rb-daemon `change.rs` — re-export from rb-types

Delete the local `MemoryChanged`/`ChangeKind` definitions and re-export them from `rb-types` so all existing daemon code (`store_handle.rs` imports `crate::change::{ChangeKind, MemoryChanged}`) keeps compiling unchanged, and `pub use change::{ChangeKind, MemoryChanged}` in `rb-daemon/src/lib.rs` continues to point at the single canonical definition.

**Files:**
- Modify: crates/rb-daemon/src/change.rs
- Test: crates/rb-daemon/src/change.rs (re-export guard test)

- [ ] **Step 1 RED: write the failing test.** Replace the entire contents of `crates/rb-daemon/src/change.rs` with the re-export plus a guard test. This file currently OWNS the types; the new content removes them and re-exports from `rb-types`:

```rust
//! Change-notification re-export. The canonical `MemoryChanged`/`ChangeKind`
//! types live in `rb-types` (the leaf crate) so both `rb-proto` (wire
//! `Response`) and this daemon (the broadcast channel) can name them without a
//! dependency cycle. This module re-exports them so existing intra-crate paths
//! (`crate::change::{ChangeKind, MemoryChanged}`) keep working verbatim.

pub use rb_types::{ChangeKind, MemoryChanged};

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, Namespace};

    #[test]
    fn reexported_change_types_are_the_rb_types_definitions() {
        // A value constructed via the rb-daemon path is byte-identical to one
        // constructed via the rb-types path — proving there is exactly ONE
        // definition, re-exported, not a divergent copy.
        let via_daemon = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Global,
            kind: ChangeKind::Updated,
        };
        let direct: rb_types::MemoryChanged = via_daemon.clone();
        assert_eq!(via_daemon, direct);
        assert_eq!(via_daemon.kind, rb_types::ChangeKind::Updated);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon change` — Expected: PASS for the new guard test once the file compiles; if `rb-types` re-export from Q1 is missing it FAILs with `unresolved import rb_types::ChangeKind`. (With Q1 committed this compiles and the single guard test passes.)

- [ ] **Step 3 GREEN: no further impl needed.** The re-export in Step 1 is the implementation. Confirm `crates/rb-daemon/src/store_handle.rs` still imports `use crate::change::{ChangeKind, MemoryChanged};` (unchanged) and `crates/rb-daemon/src/lib.rs` still has `pub use change::{ChangeKind, MemoryChanged};` (unchanged) — both resolve through the re-export. Make no edits to those two files.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon` — Expected: PASS (all existing daemon tests plus the one new `reexported_change_types_are_the_rb_types_definitions`; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/change.rs && git commit -m "refactor(rb-daemon): re-export change types from rb-types"` — Expected: one commit.

---

### Task Q3: rb-proto `messages.rs` — Subscribe + streamed frames

Add `Request::Subscribe` (no fields — the namespace comes from the handshake) and two streamed `Response` variants: `Response::Change(MemoryChanged)` and `Response::Lagged { dropped: u64 }`. Extend the round-trip tests so every variant is still covered. `build_request` (Task in rb-mcp) has a wildcard, so `Subscribe` is intentionally NOT auto-exposed as an MCP tool.

**Files:**
- Modify: crates/rb-proto/src/messages.rs
- Test: crates/rb-proto/src/messages.rs (round-trip tests)

- [ ] **Step 1 RED: write the failing test.** Add these two tests to the existing `#[cfg(test)] mod tests` in `crates/rb-proto/src/messages.rs` (place them after `response_uses_result_tag`, inside the module):

```rust
    #[test]
    fn subscribe_request_round_trips_and_uses_op_tag() {
        let json = serde_json::to_string(&Request::Subscribe).unwrap();
        assert_eq!(json, r#"{"op":"Subscribe"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn change_and_lagged_responses_round_trip() {
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let change = Response::Change(MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
        });
        let json = serde_json::to_string(&change).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        // The streamed Change frame carries `result: "Change"`.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "Change");

        let lagged = Response::Lagged { dropped: 7 };
        let json = serde_json::to_string(&lagged).unwrap();
        assert_eq!(json, r#"{"result":"Lagged","dropped":7}"#);
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-proto subscribe_request_round_trips_and_uses_op_tag` — Expected: FAIL — `Request::Subscribe`, `Response::Change`, and `Response::Lagged` do not exist (`error[E0599]`/`no variant`).

- [ ] **Step 3 GREEN: add the variants + import.** Edit `crates/rb-proto/src/messages.rs`. First extend the top import to bring in the change types:

```rust
use rb_types::{
    ChangeKind, MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    SearchResult,
};
```

Note: `ChangeKind` is imported so the round-trip helper below can name it; if clippy flags it as unused in non-test code, keep only `MemoryChanged` in the top `use` and import `ChangeKind` inside the test module instead. To stay safe, use exactly this top import (only the types referenced by the enums + helpers):

```rust
use rb_types::{
    MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, SearchResult,
};
```

Add the `Subscribe` variant to the `Request` enum — insert it after `Ping` (last variant), keeping the existing nine:

```rust
/// One request per engine operation. Internally tagged on `op`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op")]
pub enum Request {
    Remember {
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
    },
    Recall {
        query: String,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    },
    Get {
        id: MemoryId,
    },
    List {
        min_importance: Option<u8>,
        limit: usize,
    },
    Graph {
        id: MemoryId,
        depth: u8,
    },
    Update {
        id: MemoryId,
        updates: MemoryUpdates,
    },
    Delete {
        id: MemoryId,
    },
    Context,
    Ping,
    /// Open a live change-notification stream. The daemon stops the
    /// request/response cadence for this connection and streams `Response::Change`
    /// (and `Response::Lagged` on broadcast overflow) until the client disconnects.
    /// The stream is scoped to the connection's handshake namespace, filtered
    /// server-side.
    Subscribe,
}
```

Add the two streamed variants to the `Response` enum — insert `Change` and `Lagged` after `Error` (last variant), keeping all existing ten:

```rust
/// One response per request. Internally tagged on `result`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "result")]
pub enum Response {
    Remembered {
        id: MemoryId,
    },
    Recalled {
        results: Vec<SearchResult>,
    },
    Got {
        memory: Option<MemoryNote>,
    },
    Listed {
        memories: Vec<MemoryNote>,
    },
    GraphResult {
        memories: Vec<MemoryNote>,
    },
    Updated,
    Deleted,
    ContextResult {
        recent: Vec<MemoryNote>,
        important: Vec<MemoryNote>,
        total: usize,
    },
    Pong {
        contract_version: u32,
    },
    Error {
        kind: String,
        message: String,
    },
    /// A streamed change event (only emitted on a `Subscribe` connection).
    Change(MemoryChanged),
    /// The subscriber fell behind and the broadcast channel dropped `dropped`
    /// events for it. Observability only; the stream continues.
    Lagged {
        dropped: u64,
    },
}
```

Also extend the existing `all_responses()` test helper so the variant-coverage test exercises the new frames. Replace the `all_responses` function body's returned vec by appending the two new variants before the closing `]`:

```rust
    fn all_responses() -> Vec<Response> {
        vec![
            Response::Remembered {
                id: MemoryId::new(),
            },
            Response::Recalled {
                results: vec![SearchResult {
                    memory: note(),
                    score: 0.9,
                }],
            },
            Response::Got {
                memory: Some(note()),
            },
            Response::Got { memory: None },
            Response::Listed {
                memories: vec![note()],
            },
            Response::GraphResult {
                memories: vec![note()],
            },
            Response::Updated,
            Response::Deleted,
            Response::ContextResult {
                recent: vec![note()],
                important: vec![note()],
                total: 2,
            },
            Response::Pong {
                contract_version: CONTRACT_VERSION,
            },
            Response::Error {
                kind: "not_found".into(),
                message: "no such memory".into(),
            },
            Response::Change(rb_types::MemoryChanged {
                id: MemoryId::new(),
                namespace: Namespace::Project("rusty-brain".into()),
                kind: rb_types::ChangeKind::Created,
            }),
            Response::Lagged { dropped: 3 },
        ]
    }
```

And extend `all_requests()` similarly so `every_request_variant_round_trips` covers `Subscribe`; append before the closing `]`:

```rust
            Request::Delete { id },
            Request::Context,
            Request::Ping,
            Request::Subscribe,
        ]
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-proto` — Expected: PASS (all existing messages/frame/codec/client tests plus the two new ones; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-proto --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-proto/src/messages.rs && git commit -m "feat(rb-proto): add Subscribe request and Change/Lagged response frames"` — Expected: one commit.

---

### Task Q4: rb-proto `client.rs` — recv_change stream consumer

Add a streaming consumer to `Client`: `subscribe()` sends `Request::Subscribe`, and `recv_change()` reads the next streamed frame as a `SubscribeItem` (`Change(MemoryChanged)` or `Lagged(u64)`). Any non-streamed response is a protocol violation mapped to `Error`. Also fix the now-non-exhaustive `Request` match in the existing `wrapper_tests::serve` responder.

**Files:**
- Modify: crates/rb-proto/src/client.rs
- Modify: crates/rb-proto/src/lib.rs
- Test: crates/rb-proto/src/client.rs (subscribe stream test)

- [ ] **Step 1 RED: write the failing test.** Add a new test module at the end of `crates/rb-proto/src/client.rs` (after `mod wrapper_tests`). It drives a hand-rolled responder that acks the handshake, reads one `Request::Subscribe`, then streams a `Change` and a `Lagged` frame:

```rust
#[cfg(test)]
mod subscribe_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
    };
    use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub.sock");
        (dir, path)
    }

    // Accept one connection, handshake, read exactly one Subscribe request, then
    // stream a Change frame followed by a Lagged frame, then close.
    async fn serve_stream(listener: UnixListener, change_id: MemoryId) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());
        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
            },
        )
        .await
        .unwrap();

        let req: Request = read_frame(&mut framed).await.unwrap();
        assert!(matches!(req, Request::Subscribe), "expected Subscribe");

        write_frame(
            &mut framed,
            &Response::Change(MemoryChanged {
                id: change_id,
                namespace: Namespace::Global,
                kind: ChangeKind::Created,
            }),
        )
        .await
        .unwrap();
        write_frame(&mut framed, &Response::Lagged { dropped: 5 })
            .await
            .unwrap();
        // Drop closes the stream; the client's next recv_change returns Err(Io).
    }

    #[tokio::test]
    async fn subscribe_then_recv_change_and_lagged() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let change_id = MemoryId::new();
        let server = tokio::spawn(serve_stream(listener, change_id.clone()));

        let mut client = Client::connect(&path, Namespace::Global).await.unwrap();
        client.subscribe().await.unwrap();

        match client.recv_change().await.unwrap() {
            SubscribeItem::Change(evt) => {
                assert_eq!(evt.id, change_id);
                assert_eq!(evt.kind, ChangeKind::Created);
            }
            other => panic!("expected Change, got {other:?}"),
        }
        match client.recv_change().await.unwrap() {
            SubscribeItem::Lagged(n) => assert_eq!(n, 5),
            other => panic!("expected Lagged, got {other:?}"),
        }
        // Stream closed -> the next recv is a transport error, not a hang.
        let err = client.recv_change().await.unwrap_err();
        assert!(matches!(err, Error::Io(_)), "closed stream -> Io: {err:?}");

        server.await.unwrap();
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-proto subscribe_then_recv_change_and_lagged` — Expected: FAIL — `Client::subscribe`, `Client::recv_change`, and `SubscribeItem` do not exist (`error[E0599]`/`cannot find type`).

- [ ] **Step 3 GREEN: add `SubscribeItem`, `subscribe`, `recv_change`.** Edit `crates/rb-proto/src/client.rs`. First extend the top `use rb_types::{...}` import to add `MemoryChanged`:

```rust
use rb_types::{
    Error, MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, Result,
    SearchResult,
};
```

Add the `SubscribeItem` type just below the `Client` struct definition (after the closing `}` of `pub struct Client`):

```rust
/// One item yielded by a live subscribe stream: either a change event or a
/// notice that the broadcast dropped `n` events for this slow subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeItem {
    /// A committed change in the connection's namespace.
    Change(MemoryChanged),
    /// The subscriber fell behind; `n` events were dropped by the broadcast.
    Lagged(u64),
}
```

Add the two methods inside the FIRST `impl Client { ... }` block (the one containing `connect` and `request`), placed after `request`:

```rust
    /// Open a live change-notification stream on this connection. After this
    /// returns `Ok`, the connection no longer follows request/response cadence;
    /// call [`recv_change`](Self::recv_change) in a loop to read streamed frames.
    /// The stream is scoped server-side to the handshake namespace.
    pub async fn subscribe(&mut self) -> Result<()> {
        write_frame(&mut self.framed, &Request::Subscribe).await
    }

    /// Read the next streamed item from a subscribe stream. Blocks until the
    /// daemon emits the next `Change`/`Lagged` frame; a closed stream surfaces as
    /// `Error::Io`. Any non-streamed response is a protocol violation.
    pub async fn recv_change(&mut self) -> Result<SubscribeItem> {
        let resp: Response = read_frame(&mut self.framed).await?;
        match resp {
            Response::Change(evt) => Ok(SubscribeItem::Change(evt)),
            Response::Lagged { dropped } => Ok(SubscribeItem::Lagged(dropped)),
            Resp::Error { kind, message } => Err(response_error_to_error(&kind, &message)),
            other => Err(Error::Storage(format!(
                "unexpected frame on subscribe stream: {other:?}"
            ))),
        }
    }
```

Export `SubscribeItem` from the crate. Edit `crates/rb-proto/src/lib.rs`, changing the client re-export line:

```rust
pub use client::{Client, SubscribeItem};
```

Now fix the already-existing exhaustive `Request` match in `wrapper_tests::serve` (it will fail to compile because `Request::Subscribe` is unhandled). Add a `Subscribe` arm — the wrapper responder never receives Subscribe in its test, so return a canned `Pong` to keep the match exhaustive:

```rust
                Request::Ping => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                },
                Request::Subscribe => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                },
            };
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-proto` — Expected: PASS (all client/messages/frame/codec tests plus the new `subscribe_then_recv_change_and_lagged`; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-proto --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-proto/src/client.rs crates/rb-proto/src/lib.rs && git commit -m "feat(rb-proto): add Client subscribe/recv_change stream consumer"` — Expected: one commit.

---

### Task Q5: rb-daemon `server.rs` — streaming Subscribe handler

Add a `Subscribe` branch to the per-connection loop: when a connection sends `Request::Subscribe`, stop the request/response cadence and enter a streaming loop over the `StoreHandle` broadcast receiver, writing `Response::Change` for events in the connection's namespace, `Response::Lagged` on `RecvError::Lagged`, and breaking on `RecvError::Closed` or any write error (client gone). A lagging or disconnected subscriber must never block the writer — the broadcast channel already drops oldest and reports `Lagged`, and we never `.await` on the writer here.

**Files:**
- Modify: crates/rb-daemon/src/server.rs
- Test: crates/rb-daemon/tests/daemon_e2e.rs (subscribe e2e)

- [ ] **Step 1 RED: write the failing test.** Add this test to `crates/rb-daemon/tests/daemon_e2e.rs` (append at the end of the file). It connects a subscriber under namespace `a`, then a writer under `a` remembers a memory (a `Change(Created)` must arrive), then a writer under `b` remembers a memory (which must NOT be delivered to the `a` subscriber):

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_streams_only_own_namespace_changes() {
    use rb_proto::SubscribeItem;
    use rb_types::ChangeKind;

    let daemon = RunningDaemon::start(4).await;
    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());

    // Subscriber on namespace A.
    let mut sub = Client::connect(&daemon.socket, ns_a.clone()).await.unwrap();
    sub.subscribe().await.unwrap();

    // Writer on namespace A: a Created event must reach the subscriber.
    let mut writer_a = Client::connect(&daemon.socket, ns_a.clone()).await.unwrap();
    let id_a = writer_a
        .remember(
            "a memory".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    // The subscriber receives the A change (skip any Lagged notices).
    let got = loop {
        match tokio::time::timeout(Duration::from_secs(5), sub.recv_change())
            .await
            .expect("subscribe stream timed out waiting for the A change")
            .unwrap()
        {
            SubscribeItem::Change(evt) => break evt,
            SubscribeItem::Lagged(_) => continue,
        }
    };
    assert_eq!(got.id, id_a, "subscriber must receive its namespace's change");
    assert_eq!(got.namespace, ns_a);
    assert_eq!(got.kind, ChangeKind::Created);

    // Writer on namespace B: this change must NOT be delivered to the A subscriber.
    let mut writer_b = Client::connect(&daemon.socket, ns_b).await.unwrap();
    writer_b
        .remember(
            "b memory".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    // Do a second A write so there IS a frame to read; the B write must have been
    // filtered out server-side, so the very next Change is the second A event.
    let id_a2 = writer_a
        .remember(
            "a memory 2".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    let next = loop {
        match tokio::time::timeout(Duration::from_secs(5), sub.recv_change())
            .await
            .expect("subscribe stream timed out waiting for the second A change")
            .unwrap()
        {
            SubscribeItem::Change(evt) => break evt,
            SubscribeItem::Lagged(_) => continue,
        }
    };
    assert_eq!(
        next.id, id_a2,
        "the B-namespace change must be filtered out; next frame is the 2nd A change"
    );
    assert_eq!(next.namespace, ns_a, "no cross-namespace leakage");

    daemon.stop().await;
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon --test daemon_e2e subscribe_streams_only_own_namespace_changes` — Expected: FAIL — the daemon does not handle `Request::Subscribe` (the `dispatch` match is non-exhaustive once Q3 added the variant, so `rb-daemon` will not even compile until the branch exists; before the new handler, `subscribe()` writes a frame the server treats as an unknown request and the stream never delivers).

- [ ] **Step 3 GREEN: add the Subscribe branch + streaming loop.** Edit `crates/rb-daemon/src/server.rs`. Add the needed imports to the top `rb_proto` use and a broadcast error import:

```rust
use rb_proto::{
    bounded_framed, read_frame, write_frame, Handshake, HandshakeAck, Request, Response,
    CONTRACT_VERSION,
};
use rb_types::{Error, Result};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{info, warn};
```

In `handle_connection`, the per-request loop currently calls `dispatch` for every frame. Intercept `Request::Subscribe` BEFORE dispatch and hand off to the streaming loop. Replace the existing request loop (the `loop { ... }` block near the end of `handle_connection`, starting at `loop {` and ending at its closing `}` before `Ok(())`) with this version. Note the `engine` move issue: the streaming loop needs the `store` handle but `engine` consumed it via `MemoryEngine::new(store, ...)`. Clone the store BEFORE building the engine. Replace the engine-construction + loop region:

```rust
    let store_for_stream = store.clone();
    let engine = {
        let base = MemoryEngine::new(store, embedder, namespace.clone());
        match enricher {
            Some(e) => base.with_enricher(e),
            None => base,
        }
    };
    loop {
        // Break the loop if the client is idle for too long between requests.
        let req: Request = match timeout(REQUEST_IDLE_TIMEOUT, read_frame(&mut framed)).await {
            Ok(Ok(req)) => req,
            Ok(Err(_)) => break, // parse error or clean close
            Err(_) => {
                warn!("client idle timeout; closing connection");
                break;
            }
        };
        // Subscribe converts this connection into a one-way change stream. It
        // never returns to request/response cadence; it runs until the client
        // disconnects or the broadcast closes.
        if matches!(req, Request::Subscribe) {
            stream_changes(&mut framed, &store_for_stream, &namespace).await;
            break;
        }
        let resp = dispatch(&engine, req).await;
        write_frame(&mut framed, &resp).await?;
    }

    Ok(())
}

/// Stream namespace-scoped change events to a subscriber until the client
/// disconnects or the broadcast closes.
///
/// HARD RULE: this must NEVER block the writer. It only reads from the broadcast
/// receiver (which drops oldest and reports `Lagged` for slow consumers) and
/// writes to this one connection's socket; a write error means the client is
/// gone, so we stop. Events outside `namespace` are filtered server-side (fail
/// closed: only exact-namespace events are forwarded).
async fn stream_changes(
    framed: &mut Framed<UnixStream, LengthDelimitedCodec>,
    store: &StoreHandle,
    namespace: &rb_types::Namespace,
) {
    let mut rx = store.subscribe();
    loop {
        match rx.recv().await {
            Ok(evt) => {
                if &evt.namespace != namespace {
                    continue; // cross-namespace event: never leak it
                }
                if write_frame(framed, &Response::Change(evt)).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(RecvError::Lagged(dropped)) => {
                // The subscriber fell behind; the broadcast dropped `dropped`
                // events for it. Surface the count and keep streaming.
                if write_frame(framed, &Response::Lagged { dropped })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(RecvError::Closed) => break, // daemon shutting down
        }
    }
}
```

Add the `Subscribe` arm to the `dispatch` match so it stays exhaustive. `dispatch` is only reached for non-Subscribe frames now (the loop intercepts Subscribe before calling it), but the match must still compile — return an Error response (defensive; unreachable in practice because the loop handles it):

```rust
        Request::Context => match engine.context().await {
            Ok((recent, important, total)) => Response::ContextResult {
                recent,
                important,
                total,
            },
            Err(e) => error_to_response(e),
        },
        // Subscribe is handled by the streaming branch in `handle_connection`
        // before `dispatch` is called; reaching here is a protocol misuse.
        Request::Subscribe => error_to_response(Error::InvalidArgument(
            "Subscribe is a streaming op, not a single request".to_string(),
        )),
    }
}
```

Confirm `MemoryEngine::new` takes the namespace by value and that cloning it for `MemoryEngine::new(store, embedder, namespace.clone())` plus passing `&namespace` to `stream_changes` compiles (the original code moved `namespace` into the engine; now it is cloned so the streaming branch can borrow the binding).

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon --test daemon_e2e subscribe_streams_only_own_namespace_changes` — Expected: PASS (1 test). Then run the whole e2e suite: `cargo test -p rb-daemon` — Expected: PASS (0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/server.rs crates/rb-daemon/tests/daemon_e2e.rs && git commit -m "feat(rb-daemon): stream namespace-scoped change events on Subscribe"` — Expected: one commit.

---

### Task Q6: rusty-brain `output.rs` — render subscribe items

Add a pure renderer for a streamed `SubscribeItem` (human one-line and `--json` object forms) so the CLI `subscribe` loop prints each event without inline formatting. Pure, no IO, fully unit-testable.

**Files:**
- Modify: crates/rusty-brain/src/output.rs
- Test: crates/rusty-brain/src/output.rs (render_change tests)

- [ ] **Step 1 RED: write the failing test.** Add these tests to the existing `#[cfg(test)] mod tests` in `crates/rusty-brain/src/output.rs` (append inside the module):

```rust
    #[test]
    fn human_change_shows_kind_namespace_and_id() {
        use rb_proto::SubscribeItem;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let id = MemoryId::new();
        let item = SubscribeItem::Change(MemoryChanged {
            id: id.clone(),
            namespace: Namespace::Project("p".into()),
            kind: ChangeKind::Created,
        });
        let out = render_change(&item, false);
        assert!(out.contains("created"), "kind shown: {out}");
        assert!(out.contains("project:p"), "namespace shown: {out}");
        assert!(out.contains(&id.to_string()), "id shown: {out}");
    }

    #[test]
    fn human_lagged_shows_dropped_count() {
        use rb_proto::SubscribeItem;
        let out = render_change(&SubscribeItem::Lagged(9), false);
        assert!(out.to_lowercase().contains("lagged"), "lagged shown: {out}");
        assert!(out.contains('9'), "dropped count shown: {out}");
    }

    #[test]
    fn json_change_is_parseable_object() {
        use rb_proto::SubscribeItem;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let id = MemoryId::new();
        let item = SubscribeItem::Change(MemoryChanged {
            id: id.clone(),
            namespace: Namespace::Global,
            kind: ChangeKind::Archived,
        });
        let out = render_change(&item, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["kind"], "Archived");
        assert_eq!(parsed["namespace"], "global");
        assert_eq!(parsed["id"], id.to_string());
    }

    #[test]
    fn json_lagged_is_parseable_object() {
        use rb_proto::SubscribeItem;
        let out = render_change(&SubscribeItem::Lagged(4), true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["lagged"], true);
        assert_eq!(parsed["dropped"], 4);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib output::tests::human_change_shows_kind_namespace_and_id` — Expected: FAIL — `render_change` does not exist (`error[E0425]`).

- [ ] **Step 3 GREEN: add `render_change`.** Edit `crates/rusty-brain/src/output.rs`. Add this function (place it after `render_context`):

```rust
/// Render one streamed subscribe item (a change event or a lagged notice).
/// JSON: a flat object. Human: a single line.
pub fn render_change(item: &rb_proto::SubscribeItem, json: bool) -> String {
    use rb_proto::SubscribeItem;
    match item {
        SubscribeItem::Change(evt) => {
            let kind = match evt.kind {
                rb_types::ChangeKind::Created => "Created",
                rb_types::ChangeKind::Updated => "Updated",
                rb_types::ChangeKind::Archived => "Archived",
            };
            if json {
                let value = serde_json::json!({
                    "kind": kind,
                    "namespace": evt.namespace.as_db_string(),
                    "id": evt.id.to_string(),
                });
                serde_json::to_string(&value).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to render change JSON");
                    "{}".to_string()
                })
            } else {
                format!(
                    "{} {} {}",
                    kind.to_lowercase(),
                    evt.namespace.as_db_string(),
                    evt.id
                )
            }
        }
        SubscribeItem::Lagged(dropped) => {
            if json {
                format!("{{\"lagged\":true,\"dropped\":{dropped}}}")
            } else {
                format!("lagged: {dropped} change event(s) dropped (subscriber fell behind)")
            }
        }
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib output` — Expected: PASS (existing output tests plus the four new ones; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/output.rs && git commit -m "feat(rusty-brain): render subscribe change/lagged items"` — Expected: one commit.

---

### Task Q7: rusty-brain `cli.rs` + `run.rs` — subscribe command

Add `Command::Subscribe` to the clap surface and wire it in `run.rs`: open a dedicated connection, send `Subscribe`, then loop printing each `SubscribeItem` until the stream closes (EOF / Ctrl-C), exiting cleanly. The existing `run_client` matches every `Command` arm exhaustively, so the new arm is required to compile.

**Files:**
- Modify: crates/rusty-brain/src/cli.rs
- Modify: crates/rusty-brain/src/run.rs
- Test: crates/rusty-brain/src/cli.rs (parse test) and crates/rusty-brain/src/run.rs (match-arm guard)

- [ ] **Step 1 RED: write the failing test.** Add a parse test to `crates/rusty-brain/src/cli.rs`. The file has no test module yet, so add one at the end:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_subscribe_subcommand() {
        let cli = Cli::parse_from(["rusty-brain", "subscribe"]);
        assert!(
            matches!(cli.command, Command::Subscribe),
            "`rusty-brain subscribe` must parse to Command::Subscribe"
        );
    }

    #[test]
    fn parses_subscribe_with_global_json_flag() {
        let cli = Cli::parse_from(["rusty-brain", "--json", "subscribe"]);
        assert!(cli.json, "--json is a global flag and applies to subscribe");
        assert!(matches!(cli.command, Command::Subscribe));
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib cli::tests::parses_subscribe_subcommand` — Expected: FAIL — `Command::Subscribe` does not exist (`error[E0599]`/`no variant`).

- [ ] **Step 3 GREEN: add the variant + run wiring.** Edit `crates/rusty-brain/src/cli.rs`. Add the `Subscribe` variant to the `Command` enum (place after `Context`, before `Status`):

```rust
    /// Show the project context payload (recent + important).
    Context,

    /// Stream live change notifications for the current namespace until Ctrl-C.
    Subscribe,

    /// Ping the daemon and report its contract version.
    Status,
```

Now edit `crates/rusty-brain/src/run.rs` to handle the new arm in `run_client`. Add this arm to the `match command { ... }` block (place after the `Command::Context` arm, before `Command::Status`):

```rust
        Command::Subscribe => {
            client.subscribe().await.context("subscribe failed")?;
            // Stream until the daemon closes the connection (or the process is
            // interrupted). recv_change returns Err(Io) on a clean close, which
            // we treat as a normal end-of-stream exit (not a failure).
            loop {
                tokio::select! {
                    biased;
                    _ = tokio::signal::ctrl_c() => {
                        break;
                    }
                    item = client.recv_change() => {
                        match item {
                            Ok(item) => {
                                println!("{}", output::render_change(&item, json));
                            }
                            Err(_) => break, // stream closed: clean exit
                        }
                    }
                }
            }
        }
```

The `client` binding in `run_client` is `let mut client = ...`, so `client.subscribe()` / `client.recv_change()` (both `&mut self`) compile. The `Resp`/`SubscribeItem` types come from `rb_proto` via the typed wrappers; no new `use` is needed because `output::render_change` takes `&rb_proto::SubscribeItem` and `run.rs` only names it through the `client` return type.

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib cli` then `cargo test -p rusty-brain --lib run` — Expected: PASS (the new cli parse tests and the existing run guard tests; 0 failures). Then `cargo build -p rusty-brain` — Expected: success (the exhaustive `run_client` match now covers `Command::Subscribe`).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rusty-brain/src/cli.rs crates/rusty-brain/src/run.rs && git commit -m "feat(rusty-brain): add subscribe command streaming change events"` — Expected: one commit.

---

### Task Q8: rb-mcp `change_buffer.rs` — bounded change ring

Add a bounded, lossy ring buffer for change events plus a dropped counter. A background subscriber pushes events in; `poll_changes` drains them out. When the ring is at capacity a push evicts the oldest and increments `dropped`; a `Lagged(n)` from the broadcast adds `n` to `dropped`. This is the core data structure (no IO), fully unit-tested offline.

**Files:**
- Create: crates/rb-mcp/src/change_buffer.rs
- Modify: crates/rb-mcp/src/lib.rs
- Test: crates/rb-mcp/src/change_buffer.rs (ring tests)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-mcp/src/change_buffer.rs` with this exact content (impl + tests together):

```rust
//! A bounded, lossy ring of change events shared between a background daemon
//! subscriber and the `poll_changes` tool. Bounded so a flood of writes (or a
//! client that never polls) can never grow memory without limit; lossy so the
//! newest events win and the count of dropped events is reported on each drain.

use rb_types::MemoryChanged;
use std::collections::VecDeque;

/// A bounded ring of buffered change events with a since-last-drain dropped
/// counter. Cheap to clone via `Arc<Mutex<ChangeBuffer>>` at the call site.
#[derive(Debug)]
pub struct ChangeBuffer {
    events: VecDeque<MemoryChanged>,
    capacity: usize,
    /// Events dropped (evicted on overflow, or reported by broadcast `Lagged`)
    /// since the last `drain`. Reset to 0 by `drain`.
    dropped: u64,
}

/// The result of draining the ring: up to `max` events plus the number of
/// events dropped since the previous drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drained {
    pub events: Vec<MemoryChanged>,
    pub dropped: u64,
}

impl ChangeBuffer {
    /// Create an empty ring holding at most `capacity` events (min 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::new(),
            capacity: capacity.max(1),
            dropped: 0,
        }
    }

    /// Push one event, evicting (and counting) the oldest if at capacity.
    pub fn push(&mut self, evt: MemoryChanged) {
        if self.events.len() >= self.capacity {
            // Evict oldest: newest events are the most useful to a poller.
            let _ = self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(evt);
    }

    /// Record that the broadcast dropped `n` events for this subscriber.
    pub fn record_dropped(&mut self, n: u64) {
        self.dropped = self.dropped.saturating_add(n);
    }

    /// Drain up to `max` of the oldest buffered events, returning them plus the
    /// dropped count accumulated since the previous drain (then reset to 0).
    pub fn drain(&mut self, max: usize) -> Drained {
        let take = max.min(self.events.len());
        let events: Vec<MemoryChanged> = self.events.drain(..take).collect();
        let dropped = self.dropped;
        self.dropped = 0;
        Drained { events, dropped }
    }

    /// Number of buffered events currently held.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the ring currently holds no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};

    fn evt(kind: ChangeKind) -> MemoryChanged {
        MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Global,
            kind,
        }
    }

    #[test]
    fn push_then_drain_returns_events_in_order_no_drops() {
        let mut b = ChangeBuffer::new(8);
        let a = evt(ChangeKind::Created);
        let c = evt(ChangeKind::Updated);
        b.push(a.clone());
        b.push(c.clone());
        assert_eq!(b.len(), 2);
        let drained = b.drain(10);
        assert_eq!(drained.events, vec![a, c]);
        assert_eq!(drained.dropped, 0);
        assert!(b.is_empty(), "drain empties what it took");
    }

    #[test]
    fn drain_respects_max_and_leaves_remainder() {
        let mut b = ChangeBuffer::new(8);
        for _ in 0..5 {
            b.push(evt(ChangeKind::Created));
        }
        let first = b.drain(2);
        assert_eq!(first.events.len(), 2);
        assert_eq!(b.len(), 3, "remainder stays buffered");
        let second = b.drain(100);
        assert_eq!(second.events.len(), 3);
        assert_eq!(second.dropped, 0);
    }

    #[test]
    fn overflow_evicts_oldest_and_counts_drops() {
        let mut b = ChangeBuffer::new(2);
        let e1 = evt(ChangeKind::Created);
        let e2 = evt(ChangeKind::Updated);
        let e3 = evt(ChangeKind::Archived);
        b.push(e1);
        b.push(e2.clone());
        b.push(e3.clone()); // evicts e1
        assert_eq!(b.len(), 2, "capacity is never exceeded");
        let drained = b.drain(10);
        assert_eq!(
            drained.events,
            vec![e2, e3],
            "oldest evicted; newest retained"
        );
        assert_eq!(drained.dropped, 1, "one eviction counted as a drop");
    }

    #[test]
    fn record_dropped_accumulates_and_resets_on_drain() {
        let mut b = ChangeBuffer::new(4);
        b.record_dropped(3);
        b.push(evt(ChangeKind::Created));
        b.record_dropped(2);
        let drained = b.drain(10);
        assert_eq!(drained.events.len(), 1);
        assert_eq!(drained.dropped, 5, "3 + 2 reported once");
        // Dropped resets after a drain.
        let next = b.drain(10);
        assert_eq!(next.dropped, 0);
        assert!(next.events.is_empty());
    }

    #[test]
    fn zero_capacity_is_clamped_to_one() {
        let mut b = ChangeBuffer::new(0);
        b.push(evt(ChangeKind::Created));
        b.push(evt(ChangeKind::Updated)); // evicts the first
        assert_eq!(b.len(), 1);
        let drained = b.drain(10);
        assert_eq!(drained.events.len(), 1);
        assert_eq!(drained.dropped, 1);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-mcp change_buffer` — Expected: FAIL — `change_buffer` is not declared in `lib.rs`, so the module is unreachable (`error[E0583]`/unresolved module / tests do not run).

- [ ] **Step 3 GREEN: declare the module.** Edit `crates/rb-mcp/src/lib.rs` to add the module and re-export. Replace the module/re-export block:

```rust
pub mod change_buffer;
pub mod jsonrpc;
pub mod proxy;
pub mod server;
pub mod tools;
pub mod transport;

pub use change_buffer::{ChangeBuffer, Drained};
pub use jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use proxy::{build_request, response_to_content, DaemonProxy};
pub use server::handle_request;
pub use tools::{tool_definitions, ToolDef};
pub use transport::serve_stdio;
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-mcp change_buffer` — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-mcp/src/change_buffer.rs crates/rb-mcp/src/lib.rs && git commit -m "feat(rb-mcp): add bounded lossy change-event ring buffer"` — Expected: one commit.

---

### Task Q9: rb-mcp `tools.rs` + `proxy.rs` — poll_changes tool

Add a 9th MCP tool `poll_changes` (optional `max` integer) and route it. Because `poll_changes` reads from the adapter's local ring rather than forwarding a `Request` to the daemon, it is handled specially in the server (Task Q10), but the JSON-Schema descriptor lives here and the tool-count tests must be updated. No `Request` mapping is added in `build_request` (it keeps its wildcard) — `poll_changes` is intentionally not a daemon round-trip.

**Files:**
- Modify: crates/rb-mcp/src/tools.rs
- Test: crates/rb-mcp/src/tools.rs (tool-count + schema tests)

- [ ] **Step 1 RED: write the failing test.** In `crates/rb-mcp/src/tools.rs`, update the two count-based tests and add one for the new tool. First, modify `exposes_exactly_the_eight_spine_tools` to expect nine and include `poll_changes`, and add `poll_changes_takes_optional_max`. Replace the body of `exposes_exactly_the_eight_spine_tools` and append a new test:

```rust
    #[test]
    fn exposes_exactly_the_nine_tools() {
        let names: BTreeSet<&str> = tool_definitions().iter().map(|t| t.name).collect();
        let expected: BTreeSet<&str> = [
            "remember",
            "recall",
            "get",
            "list",
            "graph",
            "update",
            "delete",
            "context",
            "poll_changes",
        ]
        .into_iter()
        .collect();
        assert_eq!(names, expected, "tool set must be the 8 spine tools + poll_changes");
        assert_eq!(tool_definitions().len(), 9);
    }

    #[test]
    fn poll_changes_takes_optional_max() {
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "poll_changes")
            .unwrap();
        assert_eq!(t.input_schema["type"], "object");
        let props = t.input_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("max"), "poll_changes accepts optional max");
        // No required fields: polling with no args is valid.
        let required_empty = match t.input_schema.get("required") {
            None => true,
            Some(v) => v.as_array().map(|a| a.is_empty()).unwrap_or(false),
        };
        assert!(required_empty, "poll_changes must require no input");
    }
```

Delete the now-obsolete `exposes_exactly_the_eight_spine_tools` test (it is replaced by `exposes_exactly_the_nine_tools`).

- [ ] **Step 2: run it.** Run: `cargo test -p rb-mcp tools::tests::exposes_exactly_the_nine_tools` — Expected: FAIL — `tool_definitions()` still returns 8 and has no `poll_changes` (length/`find` assertions fail).

- [ ] **Step 3 GREEN: add the descriptor.** Edit `crates/rb-mcp/src/tools.rs`. In `tool_definitions()`, add a ninth `ToolDef` to the returned `vec![...]` (append after the `context` tool, before the closing `]`):

```rust
        ToolDef {
            name: "poll_changes",
            description: "Drain buffered change notifications for the current \
                          namespace since the last poll. Returns up to `max` events \
                          plus a count of events dropped (the buffer is bounded and \
                          lossy under heavy write load).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "max": { "type": "integer", "minimum": 1,
                             "description": "Maximum events to return this poll (default: 100)." }
                }
            }),
        },
    ]
}
```

The existing `every_tool_has_object_input_schema_and_nonempty_description` test iterates all tools and already passes for `poll_changes` (object schema, non-empty description, has `properties`). The `tool_list_serializes_with_camelcase_input_schema` test uses index 0 (`remember`) and is unaffected.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-mcp tools` — Expected: PASS (all tools tests, now nine tools).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-mcp/src/tools.rs && git commit -m "feat(rb-mcp): add poll_changes tool descriptor"` — Expected: one commit.

---

### Task Q10: rb-mcp `server.rs` — poll_changes dispatch over the ring

Wire `poll_changes` into the MCP dispatcher: it does NOT go through `DaemonProxy`; instead it drains the shared `ChangeBuffer` and returns `{ events: [...], dropped: <n> }`. Thread an `Arc<Mutex<ChangeBuffer>>` through `handle_request`/`handle_tools_call`, and update the two existing `tools/list` tests that assert exactly 8 tools to expect 9.

**Files:**
- Modify: crates/rb-mcp/src/server.rs
- Test: crates/rb-mcp/src/server.rs (poll_changes dispatch test)

- [ ] **Step 1 RED: write the failing test.** Add a test to `crates/rb-mcp/src/server.rs`'s `#[cfg(test)] mod tests` that pre-loads the ring and asserts `poll_changes` drains it. Also update the two existing assertions that expect 8 tools. First add the new test (append inside the module):

```rust
    #[tokio::test]
    async fn poll_changes_drains_the_ring_buffer() {
        use crate::change_buffer::ChangeBuffer;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let mut proxy = fake();
        let buffer = Arc::new(Mutex::new(ChangeBuffer::new(16)));
        {
            let mut guard = buffer.lock().await;
            guard.push(MemoryChanged {
                id: MemoryId::new(),
                namespace: Namespace::Project("p".into()),
                kind: ChangeKind::Created,
            });
            guard.record_dropped(2);
        }

        let r = req(
            "tools/call",
            Some(20),
            json!({ "name": "poll_changes", "arguments": { "max": 10 } }),
        );
        let resp = handle_request_with_buffer(r, &mut proxy, &buffer)
            .await
            .unwrap();
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let payload: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["events"].as_array().unwrap().len(), 1);
        assert_eq!(payload["dropped"], 2);
        assert_ne!(result["isError"], json!(true));

        // A second poll returns nothing new and zero drops (the ring was drained).
        let r2 = req(
            "tools/call",
            Some(21),
            json!({ "name": "poll_changes", "arguments": {} }),
        );
        let resp2 = handle_request_with_buffer(r2, &mut proxy, &buffer)
            .await
            .unwrap();
        let text2 = resp2.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        let payload2: serde_json::Value = serde_json::from_str(&text2).unwrap();
        assert_eq!(payload2["events"].as_array().unwrap().len(), 0);
        assert_eq!(payload2["dropped"], 0);
    }
```

Update the existing `tools_list_returns_eight_tools` test: rename it and change the count to nine:

```rust
    #[tokio::test]
    async fn tools_list_returns_nine_tools() {
        let mut proxy = fake();
        let r = req("tools/list", Some(2), json!({}));
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 9);
        assert!(tools.iter().any(|t| t["name"] == "remember"));
        assert!(tools.iter().any(|t| t["name"] == "poll_changes"));
        assert!(tools[0]["inputSchema"]["type"] == "object");
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-mcp server::tests::poll_changes_drains_the_ring_buffer` — Expected: FAIL — `handle_request_with_buffer` does not exist (`error[E0425]`), and `tools_list_returns_nine_tools` fails on the count until Q9 + this task land.

- [ ] **Step 3 GREEN: add buffer-aware dispatch.** Edit `crates/rb-mcp/src/server.rs`. Add imports at the top:

```rust
use crate::change_buffer::ChangeBuffer;
use crate::jsonrpc::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use crate::proxy::{build_request, response_to_content, DaemonProxy};
use crate::tools::tool_definitions;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
```

Keep the existing public `handle_request` as a thin wrapper that has no buffer (used by the existing stdio transport path where polling is not wired), and add the buffer-aware variant. Replace the existing `handle_request` with these two functions:

```rust
/// Handle one decoded JSON-RPC request WITHOUT a change buffer. `poll_changes`
/// in this mode returns an empty result with a note, since no subscriber is
/// running. Retained for the plain stdio transport that does not poll.
pub async fn handle_request(
    request: JsonRpcRequest,
    proxy: &mut dyn DaemonProxy,
) -> Option<JsonRpcResponse> {
    dispatch(request, proxy, None).await
}

/// Handle one decoded JSON-RPC request with a shared change buffer that
/// `poll_changes` drains. Used by the daemon-backed MCP server that runs a
/// background subscriber.
pub async fn handle_request_with_buffer(
    request: JsonRpcRequest,
    proxy: &mut dyn DaemonProxy,
    buffer: &Arc<Mutex<ChangeBuffer>>,
) -> Option<JsonRpcResponse> {
    dispatch(request, proxy, Some(buffer)).await
}

/// Shared dispatch core. `buffer` is `Some` when a background subscriber feeds
/// the `poll_changes` ring; `None` when polling is unavailable.
async fn dispatch(
    request: JsonRpcRequest,
    proxy: &mut dyn DaemonProxy,
    buffer: Option<&Arc<Mutex<ChangeBuffer>>>,
) -> Option<JsonRpcResponse> {
    // Notifications (no id) are acknowledged silently with no response frame.
    if request.is_notification() {
        tracing::debug!(method = %request.method, "notification (no response)");
        return None;
    }

    // Safe: non-notification means id is Some.
    let id = request.id.clone().unwrap_or(Value::Null);
    let params = request.params.clone().unwrap_or_else(|| json!({}));

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::success(id, initialize_result(&params)),
        "tools/list" => JsonRpcResponse::success(id, tools_list_result()),
        "tools/call" => handle_tools_call(id, &params, proxy, buffer).await,
        other => JsonRpcResponse::error(
            id,
            JsonRpcError::new(METHOD_NOT_FOUND, format!("unknown method '{other}'")),
        ),
    };
    Some(response)
}
```

Update `handle_tools_call` to take the buffer and special-case `poll_changes` before `build_request`. Replace the existing `handle_tools_call`:

```rust
/// Handle `tools/call`: `poll_changes` drains the local change ring; every other
/// tool routes name+arguments to a `Request` forwarded via the proxy. Routing
/// errors become JSON-RPC errors; daemon-reported errors become `isError` tool
/// results.
async fn handle_tools_call(
    id: Value,
    params: &Value,
    proxy: &mut dyn DaemonProxy,
    buffer: Option<&Arc<Mutex<ChangeBuffer>>>,
) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::error(
            id,
            JsonRpcError::new(INVALID_PARAMS, "tools/call requires a 'name'".into()),
        );
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if name == "poll_changes" {
        return handle_poll_changes(id, &arguments, buffer).await;
    }

    let request = match build_request(name, &arguments) {
        Ok(r) => r,
        Err(err) => return JsonRpcResponse::error(id, err),
    };

    match proxy.call(request).await {
        Ok(resp) => {
            let content = response_to_content(resp);
            let is_error = content.get("error").is_some();
            JsonRpcResponse::success(id, tool_result(content, is_error))
        }
        Err(e) => JsonRpcResponse::error(
            id,
            JsonRpcError::new(INTERNAL_ERROR, format!("daemon call failed: {e}")),
        ),
    }
}

/// Default and cap for the number of events a single `poll_changes` returns.
const POLL_DEFAULT_MAX: usize = 100;
const POLL_HARD_CAP: usize = 1000;

/// Drain the change ring for `poll_changes`. Returns `{ events, dropped }`. When
/// no buffer is wired (plain stdio mode) returns an empty, never-erroring result.
async fn handle_poll_changes(
    id: Value,
    arguments: &Value,
    buffer: Option<&Arc<Mutex<ChangeBuffer>>>,
) -> JsonRpcResponse {
    let max = match arguments.get("max") {
        None | Some(Value::Null) => POLL_DEFAULT_MAX,
        Some(v) => match v.as_u64().and_then(|n| usize::try_from(n).ok()) {
            Some(n) if n >= 1 => n.min(POLL_HARD_CAP),
            _ => {
                return JsonRpcResponse::error(
                    id,
                    JsonRpcError::new(INVALID_PARAMS, "'max' must be a positive integer".into()),
                );
            }
        },
    };

    let Some(buffer) = buffer else {
        // No background subscriber is running: nothing to drain, but this is not
        // an error — the client can keep polling.
        let content = json!({ "events": [], "dropped": 0 });
        return JsonRpcResponse::success(id, tool_result(content, false));
    };

    let drained = {
        let mut guard = buffer.lock().await;
        guard.drain(max)
    };
    let content = json!({ "events": drained.events, "dropped": drained.dropped });
    JsonRpcResponse::success(id, tool_result(content, false))
}
```

Leave `initialize_result`, `tools_list_result`, and `tool_result` unchanged.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-mcp server` — Expected: PASS (existing server tests plus `poll_changes_drains_the_ring_buffer` and the renamed `tools_list_returns_nine_tools`; 0 failures). Then `cargo test -p rb-mcp` — Expected: PASS (0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-mcp/src/server.rs && git commit -m "feat(rb-mcp): dispatch poll_changes by draining the change ring"` — Expected: one commit.

---

### Task Q11: rusty-brain `mcp.rs` — background subscriber wiring

Wire the MCP adapter binary so that, alongside the request/response `ClientProxy`, a background task opens a SECOND daemon connection in the same namespace, sends `Subscribe`, and pushes streamed events into a shared `ChangeBuffer` that `poll_changes` drains. Drive the transport through `handle_request_with_buffer`. A second connection is used because the proxy connection is busy serving `tools/call` round-trips; the subscriber connection is read-only.

**Files:**
- Modify: crates/rusty-brain/src/mcp.rs
- Modify: crates/rusty-brain/src/run.rs
- Modify: crates/rb-mcp/src/transport.rs
- Test: crates/rusty-brain/src/mcp.rs (compile-time wiring guard)

- [ ] **Step 1 RED: write the failing test.** Add a compile-time guard to `crates/rusty-brain/src/mcp.rs`'s test module asserting `run_mcp` still takes a pre-resolved namespace and that the new `serve_stdio_with_buffer` symbol is callable. Append inside the existing `#[cfg(test)] mod tests`:

```rust
    // Compile-time guard: the buffer-aware stdio entrypoint must exist and be
    // importable from rb_mcp. If `serve_stdio_with_buffer` is removed or its
    // signature changes incompatibly, this fails to compile.
    #[test]
    fn buffer_aware_serve_symbol_is_available() {
        fn _assert_symbol_exists() {
            // Reference the function path without calling it.
            let _f = rb_mcp::serve_stdio_with_buffer::<
                tokio::io::BufReader<tokio::io::Stdin>,
                tokio::io::Stdout,
                crate::mcp::ClientProxy,
            >;
        }
        let _ = _assert_symbol_exists;
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain --lib mcp::tests::buffer_aware_serve_symbol_is_available` — Expected: FAIL — `rb_mcp::serve_stdio_with_buffer` does not exist (`error[E0425]`/unresolved).

- [ ] **Step 3 GREEN: add the buffer-aware transport + subscriber wiring.** First, add a buffer-aware transport entrypoint to `crates/rb-mcp/src/transport.rs`. Add these imports to the top `use` block:

```rust
use crate::change_buffer::ChangeBuffer;
use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use crate::proxy::DaemonProxy;
use crate::server::{handle_request, handle_request_with_buffer};
use rb_types::{Error, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
```

Refactor `serve_stdio` to delegate to a shared core, and add the buffer-aware variant. Replace the `serve_stdio` function with this trio (the loop body is identical except which dispatch it calls):

```rust
/// Serve MCP over a line-delimited byte stream until EOF on `reader`, WITHOUT a
/// change buffer (`poll_changes` returns empty). See [`serve_stdio_with_buffer`]
/// for the polling-enabled variant.
pub async fn serve_stdio<R, W, P>(reader: R, writer: W, proxy: P) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    serve_loop(reader, writer, proxy, None).await
}

/// Serve MCP with a shared change buffer that `poll_changes` drains. The caller
/// is expected to run a background subscriber that pushes into the same buffer.
pub async fn serve_stdio_with_buffer<R, W, P>(
    reader: R,
    writer: W,
    proxy: P,
    buffer: Arc<Mutex<ChangeBuffer>>,
) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    serve_loop(reader, writer, proxy, Some(buffer)).await
}

/// Shared serve loop. `buffer` selects buffer-aware dispatch for `poll_changes`.
async fn serve_loop<R, W, P>(
    mut reader: R,
    mut writer: W,
    mut proxy: P,
    buffer: Option<Arc<Mutex<ChangeBuffer>>>,
) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    loop {
        let response = match read_capped_line(&mut reader).await? {
            LineRead::Eof => {
                tracing::debug!("mcp stdin closed; shutting down adapter");
                return Ok(());
            }
            LineRead::TooLong => {
                tracing::warn!(
                    max = MAX_LINE_BYTES,
                    "JSON-RPC frame exceeded byte cap; rejecting"
                );
                Some(JsonRpcResponse::error(
                    Value::Null,
                    JsonRpcError::new(
                        PARSE_ERROR,
                        format!("parse error: frame exceeds {MAX_LINE_BYTES} byte limit"),
                    ),
                ))
            }
            LineRead::Line(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonRpcRequest>(trimmed) {
                    Ok(request) => match buffer.as_ref() {
                        Some(buf) => handle_request_with_buffer(request, &mut proxy, buf).await,
                        None => handle_request(request, &mut proxy).await,
                    },
                    Err(e) => {
                        tracing::warn!(error = %e, "malformed JSON-RPC frame");
                        Some(JsonRpcResponse::error(
                            Value::Null,
                            JsonRpcError::new(PARSE_ERROR, format!("parse error: {e}")),
                        ))
                    }
                }
            }
        };

        if let Some(response) = response {
            write_response(&mut writer, &response).await?;
        }
    }
}
```

Export the new symbol. Edit `crates/rb-mcp/src/lib.rs`, updating the transport re-export:

```rust
pub use transport::{serve_stdio, serve_stdio_with_buffer};
```

Now edit `crates/rusty-brain/src/mcp.rs` to run the background subscriber and use the buffer-aware transport. Replace the imports and `run_mcp`:

```rust
use crate::client::connect_or_start;
use async_trait::async_trait;
use rb_mcp::{serve_stdio_with_buffer, ChangeBuffer, DaemonProxy};
use rb_proto::{Client, Request, Response, SubscribeItem};
use std::path::Path;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::sync::Mutex;
```

Keep `ClientProxy` and its `DaemonProxy` impl exactly as they are. Replace `run_mcp` with:

```rust
/// Run the MCP adapter: connect (auto-start) a proxy connection, spawn a
/// background subscriber on a SECOND connection that feeds a bounded change
/// ring, then serve stdio with `poll_changes` draining that ring.
///
/// The `namespace` is resolved off the async runtime in `main.rs` (shells out to
/// git / reads files), consistent with the CLI path.
pub async fn run_mcp(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
) -> anyhow::Result<()> {
    let self_exe =
        std::env::current_exe().map_err(|e| anyhow::anyhow!("locating own executable: {e}"))?;
    let client = connect_or_start(socket_path, db_path, namespace.clone(), self_exe.clone())
        .await
        .map_err(|e| anyhow::anyhow!("connecting to daemon: {e}"))?;

    // Bounded ring shared between the background subscriber and poll_changes.
    let buffer = Arc::new(Mutex::new(ChangeBuffer::new(1024)));

    // Background subscriber on a dedicated, read-only connection. If it cannot
    // connect or subscribe, poll_changes simply returns empty — the adapter must
    // still serve tools, so a subscriber failure is logged, not fatal.
    let sub_buffer = Arc::clone(&buffer);
    let sub_socket = socket_path.to_path_buf();
    let sub_db = db_path.to_path_buf();
    let sub_ns = namespace.clone();
    tokio::spawn(async move {
        run_subscriber(&sub_socket, &sub_db, sub_ns, self_exe, sub_buffer).await;
    });

    let proxy = ClientProxy::new(client);
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_buffer(stdin, stdout, proxy, buffer)
        .await
        .map_err(|e| anyhow::anyhow!("mcp adapter failed: {e}"))?;
    Ok(())
}

/// Background loop: open a subscriber connection and push namespace-scoped change
/// events into the shared ring until the stream closes. Best-effort: connection
/// or stream errors end the loop quietly (logged to stderr), leaving poll_changes
/// to return whatever was already buffered.
async fn run_subscriber(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
    self_exe: std::path::PathBuf,
    buffer: Arc<Mutex<ChangeBuffer>>,
) {
    let mut client = match connect_or_start(socket_path, db_path, namespace, self_exe).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "mcp change subscriber could not connect; poll_changes will be empty");
            return;
        }
    };
    if let Err(e) = client.subscribe().await {
        tracing::warn!(error = %e, "mcp change subscriber could not subscribe; poll_changes will be empty");
        return;
    }
    loop {
        match client.recv_change().await {
            Ok(SubscribeItem::Change(evt)) => {
                buffer.lock().await.push(evt);
            }
            Ok(SubscribeItem::Lagged(n)) => {
                buffer.lock().await.record_dropped(n);
            }
            Err(e) => {
                tracing::debug!(error = %e, "mcp change subscriber stream ended");
                break;
            }
        }
    }
}
```

`run.rs` already routes `Command::Mcp` to `crate::mcp::run_mcp(&socket_path, &db_path, namespace)`; no change to `run.rs` is required for this task (the modify entry is listed defensively — confirm line 32 of `crates/rusty-brain/src/run.rs` still reads `Command::Mcp => crate::mcp::run_mcp(&socket_path, &db_path, namespace)` and leave it unchanged).

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain --lib mcp` — Expected: PASS (the existing mcp guards plus `buffer_aware_serve_symbol_is_available`). Then `cargo test -p rb-mcp` — Expected: PASS (transport tests still green via the `serve_stdio` -> `serve_loop` delegation). Then `cargo build -p rusty-brain` — Expected: success.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings` and `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings on either) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-mcp/src/transport.rs crates/rb-mcp/src/lib.rs crates/rusty-brain/src/mcp.rs && git commit -m "feat(rusty-brain): run background change subscriber feeding poll_changes ring"` — Expected: one commit.

---

### Task Q12: rb-mcp `transport.rs` — poll_changes stdio contract test

Add an end-to-end stdio contract test proving `tools/call` for `poll_changes` drains a pre-seeded buffer over the real newline-delimited transport (not just the dispatcher). This closes the loop on the MCP surface: a client calling `poll_changes` over stdio gets `{ events, dropped }`.

**Files:**
- Modify: crates/rb-mcp/src/transport.rs
- Test: crates/rb-mcp/src/transport.rs (poll_changes over stdio)

- [ ] **Step 1 RED: write the failing test.** Add this test to `crates/rb-mcp/src/transport.rs`'s `#[cfg(test)] mod tests`. It seeds a buffer, runs `serve_stdio_with_buffer`, and sends one `poll_changes` frame:

```rust
    #[tokio::test]
    async fn poll_changes_over_stdio_drains_seeded_buffer() {
        use crate::change_buffer::ChangeBuffer;
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);
        let proxy = Fake {
            id: MemoryId::new(),
        };

        let buffer = Arc::new(Mutex::new(ChangeBuffer::new(16)));
        {
            let mut guard = buffer.lock().await;
            guard.push(MemoryChanged {
                id: MemoryId::new(),
                namespace: Namespace::Project("p".into()),
                kind: ChangeKind::Created,
            });
            guard.record_dropped(4);
        }

        let server_buffer = Arc::clone(&buffer);
        let server = tokio::spawn(async move {
            serve_stdio_with_buffer(
                BufReader::new(server_reader),
                server_writer,
                proxy,
                server_buffer,
            )
            .await
        });

        let mut to_server = client_to_server;
        let frame = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "poll_changes", "arguments": { "max": 10 } }
        });
        to_server
            .write_all(format!("{}\n", serde_json::to_string(&frame).unwrap()).as_bytes())
            .await
            .unwrap();
        to_server.flush().await.unwrap();
        drop(to_server);

        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if !line.trim().is_empty() {
                responses.push(serde_json::from_str(&line).unwrap());
            }
        }
        assert_eq!(responses.len(), 1, "one poll_changes reply: {responses:?}");
        assert_eq!(responses[0]["id"], 1);
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["events"].as_array().unwrap().len(), 1);
        assert_eq!(payload["dropped"], 4);
        assert_ne!(responses[0]["result"]["isError"], json!(true));

        server.await.unwrap().unwrap();
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-mcp transport::tests::poll_changes_over_stdio_drains_seeded_buffer` — Expected: FAIL before Q11's `serve_stdio_with_buffer` is present (`error[E0425]`); with Q11 committed it compiles, so write this test AFTER Q11. If run in isolation before the impl, it FAILs to compile on the missing `serve_stdio_with_buffer`/`ChangeBuffer` paths. (Order: Q11 first, then this test — both impl pieces already exist by Step 1 here, so the failure mode is a clean RED only if the test is added before any code; since transport already has the symbol from Q11, this RED is a pre-impl placeholder. To force a real RED, run this test on the exact transport from before Q11 — Expected: FAIL with unresolved `serve_stdio_with_buffer`.)

- [ ] **Step 3 GREEN: no new impl.** `serve_stdio_with_buffer` and buffer-aware dispatch already exist (Tasks Q10–Q11). This task only adds the cross-cutting stdio contract test; the assertion above passes against the existing code. Make no source edits beyond the test.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-mcp transport` — Expected: PASS (all four transport tests including the new `poll_changes_over_stdio_drains_seeded_buffer`; 0 failures). Then `cargo test -p rb-mcp` — Expected: PASS (0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-mcp/src/transport.rs && git commit -m "test(rb-mcp): poll_changes drains the buffer over the stdio transport"` — Expected: one commit.

---

### Part Q gate

Run the full workspace gates from the worktree root and expect green. Part Q adds NO new third-party dependencies (only intra-workspace wiring and existing tokio/serde features), so `cargo deny check` is not required for this Part.

- [ ] **Step 1: workspace tests.** Run: `cargo test --workspace` — Expected: PASS, 0 failures (includes the new `rb-types` change tests, the `rb-proto` Subscribe/Change/Lagged round-trips and subscribe stream test, the `rb-daemon` namespace-scoped subscribe e2e, the `rb-mcp` ring + poll_changes dispatch + stdio contract tests, and the `rusty-brain` cli/output/mcp tests).

- [ ] **Step 2: workspace clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 3: workspace format.** Run: `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 4: gate commit (only if any formatting touch-ups were needed).** Run: `git add -A && git commit -m "chore: part Q gate green (subscribe change-stream)"` — Expected: one commit, or nothing to commit if Steps 1-3 produced no changes.


## Part R — link decay + shared evolution-job scaffolding (rb-daemon `jobs` module)

This Part brings up the shared, opt-in **evolution-job framework** that Parts S (consolidation) and T (importance recalibration) reuse, and proves it end-to-end with the lowest-risk job: **link decay**. The framework is OFF by default: a TOML config (every job disabled), a single `run_once(kind, store, config)` core, an in-daemon interval scheduler that only ticks enabled jobs, and a `rusty-brain evolve <job>` CLI trigger that sends a new `Request::RunJob` to the running daemon so every mutation still funnels through the single writer (`StoreHandle` `WriteCommand`s). `JobKind` lives in `rb-types` so `rb-proto` and `rb-daemon` share it with no dependency cycle. Tasks are ordered so each layer compiles green before the next consumes it: types → dep → config → dispatch core → store seam → algorithm → scheduler → proto+CLI → gate.

---

### Task R1: rb-types `job.rs` — JobKind

**Files:**
- Create: `crates/rb-types/src/job.rs`
- Modify: `crates/rb-types/src/lib.rs`
- Test: `crates/rb-types/src/job.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** Create `crates/rb-types/src/job.rs` containing ONLY the test module first (the type does not exist yet, so it fails to compile):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const ALL: [JobKind; 3] = [
        JobKind::LinkDecay,
        JobKind::Consolidation,
        JobKind::ImportanceRecalibration,
    ];

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&JobKind::LinkDecay).unwrap(),
            r#""link_decay""#
        );
        assert_eq!(
            serde_json::to_string(&JobKind::Consolidation).unwrap(),
            r#""consolidation""#
        );
        assert_eq!(
            serde_json::to_string(&JobKind::ImportanceRecalibration).unwrap(),
            r#""importance_recalibration""#
        );
    }

    #[test]
    fn serde_round_trips_all_variants() {
        for kind in ALL {
            let json = serde_json::to_string(&kind).unwrap();
            let back: JobKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn parse_is_inverse_of_as_str() {
        for kind in ALL {
            assert_eq!(JobKind::parse(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = JobKind::parse("garbage").unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)));
    }

    #[test]
    fn copy_and_eq_hold() {
        let k = JobKind::LinkDecay;
        let copied = k; // Copy
        assert_eq!(k, copied);
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-types job` — Expected: FAIL — compile error `cannot find type JobKind in this scope` (the type is not yet defined).

- [ ] **Step 3 GREEN: minimal implementation.** Prepend the type to `crates/rb-types/src/job.rs`, above the test module:

```rust
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Which background maintenance ("evolution") job to run. Shared by `rb-proto`
/// (wire) and `rb-daemon` (`jobs` module) so neither needs to depend on the
/// other. Serializes in `snake_case` to match the TOML config section names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    LinkDecay,
    Consolidation,
    ImportanceRecalibration,
}

impl JobKind {
    /// Stable string form used by the CLI `evolve <job>` argument and logs.
    /// MUST stay in lockstep with the `serde(rename_all = "snake_case")` form.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::LinkDecay => "link_decay",
            JobKind::Consolidation => "consolidation",
            JobKind::ImportanceRecalibration => "importance_recalibration",
        }
    }

    /// Parse the CLI/db string into a `JobKind`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "link_decay" => Ok(JobKind::LinkDecay),
            "consolidation" => Ok(JobKind::Consolidation),
            "importance_recalibration" => Ok(JobKind::ImportanceRecalibration),
            other => Err(Error::InvalidArgument(format!("unknown job: {other}"))),
        }
    }
}
```

Then wire it into `crates/rb-types/src/lib.rs`. Add the module declaration alongside the others:

```rust
mod job;
```

and the re-export alongside the others:

```rust
pub use job::JobKind;
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-types job` — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-types/src/job.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add JobKind shared by proto and daemon"` — Expected: one commit.

---

### Task R2: workspace + rb-daemon `Cargo.toml` — toml dep

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/rb-daemon/Cargo.toml`

- [ ] **Step 1 RED: declare the dependency the config module will need.** This is a `chore` (manifest) task; the "test" is the build closure resolving cleanly. Add to the workspace `[workspace.dependencies]` table in `Cargo.toml`, immediately after the `# --- P1 additions ---` block (keep one entry per line, alphabetic intent not required — match the existing append style):

```toml
toml = "0.8"
```

So the relevant section of `Cargo.toml` reads:

```toml
clap = { version = "4", features = ["derive", "env"] }
anyhow = "1"
wiremock = "0.6"
libc = "0.2"
assert_cmd = "2"
predicates = "3"
toml = "0.8"
```

Then add it to `crates/rb-daemon/Cargo.toml` under `[dependencies]`, after `libc`. **Also promote `chrono` to a real dependency here:** Part R's `jobs/link_decay.rs` calls `chrono::Utc::now()` and `JobsConfig`/`LinkRow` use `chrono::DateTime` at runtime, so `chrono` must move from `[dev-dependencies]` into `[dependencies]` (and be removed from `[dev-dependencies]` if it was dev-only — a `[dependencies]` entry is available to tests too). Parts S and T also rely on runtime `chrono` and assume it is already present from here.

```toml
chrono = { workspace = true }
toml = { workspace = true }
```

So the daemon `[dependencies]` block reads:

```toml
[dependencies]
rb-types = { path = "../rb-types" }
rb-store = { path = "../rb-store" }
rb-engine = { path = "../rb-engine" }
rb-embed = { path = "../rb-embed" }
rb-enrich = { path = "../rb-enrich" }
rb-proto = { path = "../rb-proto" }
tokio = { workspace = true }
tokio-util = { workspace = true }
tracing = { workspace = true }
serde = { workspace = true }
async-trait = { workspace = true }
chrono = { workspace = true }
libc = { workspace = true }
toml = { workspace = true }
```

If `chrono` still appears under `crates/rb-daemon/Cargo.toml`'s `[dev-dependencies]`, delete that line now that it is a normal dependency.

- [ ] **Step 2: run it.** Run: `cargo build -p rb-daemon` — Expected: PASS — the `toml` crate resolves and downloads; build succeeds with no new warnings.

- [ ] **Step 3: license check.** Run: `cargo deny check` — Expected: `ok` — `toml` (and its deps `serde`, `serde_spanned`, `toml_datetime`, `toml_edit`, `winnow`) are all `MIT OR Apache-2.0`, already covered by the `deny.toml` allowlist; no new advisories.

- [ ] **Step 4: format.** Run: `cargo fmt --all` (Expected: no diff).

- [ ] **Step 5: commit.** Run: `git add Cargo.toml crates/rb-daemon/Cargo.toml Cargo.lock && git commit -m "chore: add toml dep and promote chrono for evolution jobs"` — Expected: one commit.

---

### Task R3: rb-daemon `jobs/config.rs` — JobsConfig

**Files:**
- Create: `crates/rb-daemon/src/jobs/config.rs`
- Create: `crates/rb-daemon/src/jobs/mod.rs`
- Modify: `crates/rb-daemon/src/lib.rs`
- Test: `crates/rb-daemon/src/jobs/config.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** First create the module wiring so the new tree is part of the crate. Create `crates/rb-daemon/src/jobs/mod.rs` with ONLY the submodule declaration and re-export for config (the rest of `mod.rs` arrives in R4):

```rust
//! Opt-in background "evolution" jobs: bounded, idempotent maintenance passes
//! that read via the read pool and mutate ONLY through the single writer.

mod config;

pub use config::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
```

Add `mod jobs;` to `crates/rb-daemon/src/lib.rs` after `mod change;`:

```rust
mod jobs;
```

and the public re-export after the existing `pub use` lines:

```rust
pub use jobs::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
```

Now create `crates/rb-daemon/src/jobs/config.rs` containing ONLY the test module first (the types do not exist yet, so it fails to compile):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn default_is_all_disabled_with_documented_values() {
        let cfg = JobsConfig::default();

        assert!(!cfg.link_decay.enabled);
        assert_eq!(cfg.link_decay.interval_secs, 86_400);
        assert!((cfg.link_decay.half_life_days - 30.0).abs() < f64::EPSILON);
        assert!((cfg.link_decay.floor - 0.05).abs() < f64::EPSILON);
        assert!(!cfg.link_decay.prune_below_floor);
        assert_eq!(cfg.link_decay.batch_limit, 1000);

        assert!(!cfg.consolidation.enabled);
        assert_eq!(cfg.consolidation.interval_secs, 86_400);
        assert!((cfg.consolidation.similarity_threshold - 0.95).abs() < f32::EPSILON);
        assert_eq!(cfg.consolidation.batch_limit, 200);

        assert!(!cfg.importance.enabled);
        assert_eq!(cfg.importance.interval_secs, 86_400);
        assert!((cfg.importance.access_weight - 0.5).abs() < f64::EPSILON);
        assert!((cfg.importance.recency_weight - 0.5).abs() < f64::EPSILON);
        assert!((cfg.importance.half_life_days - 30.0).abs() < f64::EPSILON);
        assert_eq!(cfg.importance.batch_limit, 1000);
    }

    #[test]
    fn partial_toml_overrides_only_named_fields() {
        let toml_src = r#"
[link_decay]
enabled = true
half_life_days = 7.0
prune_below_floor = true
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.toml");
        std::fs::write(&path, toml_src).unwrap();

        let cfg = JobsConfig::load(Some(path.as_path())).unwrap();
        // Overridden:
        assert!(cfg.link_decay.enabled);
        assert!((cfg.link_decay.half_life_days - 7.0).abs() < f64::EPSILON);
        assert!(cfg.link_decay.prune_below_floor);
        // Defaulted (serde(default) per field):
        assert_eq!(cfg.link_decay.interval_secs, 86_400);
        assert!((cfg.link_decay.floor - 0.05).abs() < f64::EPSILON);
        assert_eq!(cfg.link_decay.batch_limit, 1000);
        // Untouched sections still disabled:
        assert!(!cfg.consolidation.enabled);
        assert!(!cfg.importance.enabled);
    }

    #[test]
    fn missing_path_yields_default() {
        let cfg = JobsConfig::load(None).unwrap();
        assert!(!cfg.link_decay.enabled);
        assert_eq!(cfg.link_decay.batch_limit, 1000);
    }

    #[test]
    fn nonexistent_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let cfg = JobsConfig::load(Some(path.as_path())).unwrap();
        assert!(!cfg.link_decay.enabled);
    }

    #[test]
    fn malformed_toml_is_invalid_argument() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is = = not toml [[[").unwrap();
        let err = JobsConfig::load(Some(path.as_path())).unwrap_err();
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)), "got {err:?}");
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon config` — Expected: FAIL — compile error `cannot find type JobsConfig` / `LinkDecayConfig` (types undefined).

- [ ] **Step 3 GREEN: minimal implementation.** Prepend to `crates/rb-daemon/src/jobs/config.rs`, above the test module:

```rust
//! TOML configuration for the evolution jobs. Every job is disabled by default;
//! a missing or absent config file yields `JobsConfig::default()`. All fields
//! are `serde(default)` so a partial file overrides only the keys it names.

use serde::Deserialize;
use std::path::Path;

/// Top-level config: one section per job. All sections default-construct, so an
/// empty file (or no `[link_decay]` table at all) means "everything disabled".
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct JobsConfig {
    pub link_decay: LinkDecayConfig,
    pub consolidation: ConsolidationConfig,
    pub importance: ImportanceConfig,
}

/// Link-decay tuning. Exponential decay of link `strength` by age, floored.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LinkDecayConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub half_life_days: f64,
    pub floor: f64,
    pub prune_below_floor: bool,
    pub batch_limit: usize,
}

impl Default for LinkDecayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 86_400,
            half_life_days: 30.0,
            floor: 0.05,
            prune_below_floor: false,
            batch_limit: 1000,
        }
    }
}

/// Consolidation tuning (used by Part S). Declared here so the config file
/// schema is stable from the first release; the job itself lands in Part S.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ConsolidationConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub similarity_threshold: f32,
    pub batch_limit: usize,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 86_400,
            similarity_threshold: 0.95,
            batch_limit: 200,
        }
    }
}

/// Importance-recalibration tuning (used by Part T). Declared here for a stable
/// config schema; the job itself lands in Part T.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ImportanceConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub access_weight: f64,
    pub recency_weight: f64,
    pub half_life_days: f64,
    pub batch_limit: usize,
}

impl Default for ImportanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 86_400,
            access_weight: 0.5,
            recency_weight: 0.5,
            half_life_days: 30.0,
            batch_limit: 1000,
        }
    }
}

impl JobsConfig {
    /// Load config from `path`. `None`, or a path that does not exist, yields the
    /// all-disabled default (jobs are opt-in). A parse error is surfaced as
    /// `Error::InvalidArgument` so a typo in the file fails loudly, not silently.
    pub fn load(path: Option<&Path>) -> rb_types::Result<JobsConfig> {
        let Some(path) = path else {
            return Ok(JobsConfig::default());
        };
        if !path.exists() {
            return Ok(JobsConfig::default());
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            rb_types::Error::InvalidArgument(format!(
                "read jobs config {}: {e}",
                path.display()
            ))
        })?;
        toml::from_str(&text).map_err(|e| {
            rb_types::Error::InvalidArgument(format!(
                "parse jobs config {}: {e}",
                path.display()
            ))
        })
    }
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon config` — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/jobs/config.rs crates/rb-daemon/src/jobs/mod.rs crates/rb-daemon/src/lib.rs && git commit -m "feat(rb-daemon): add JobsConfig with all-disabled defaults"` — Expected: one commit.

---

### Task R4: rb-daemon `jobs/mod.rs` — run_once core

**Files:**
- Modify: `crates/rb-daemon/src/jobs/mod.rs`
- Create: `crates/rb-daemon/src/jobs/link_decay.rs`
- Modify: `crates/rb-daemon/src/lib.rs`
- Test: `crates/rb-daemon/src/jobs/mod.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** Replace the body of `crates/rb-daemon/src/jobs/mod.rs` so it declares the new submodule and types and carries this test module. (Keep the `mod config;` / `pub use config::...` lines.) Add this test module at the bottom of `mod.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn job_summary_default_is_all_zero() {
        let s = JobSummary::default();
        assert_eq!(s.scanned, 0);
        assert_eq!(s.changed, 0);
        assert_eq!(s.skipped, 0);
    }

    #[test]
    fn job_summary_round_trips_json() {
        let s = JobSummary {
            scanned: 9,
            changed: 4,
            skipped: 5,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: JobSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_link_decay_on_empty_store_scans_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, 8, 1).unwrap();
        let config = JobsConfig::default();

        let summary = run_once(JobKind::LinkDecay, &store, &config)
            .await
            .unwrap();
        assert_eq!(summary, JobSummary::default());

        store.shutdown().await;
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon jobs::tests` — Expected: FAIL — compile error `cannot find function run_once` / `cannot find type JobSummary` (not yet defined).

- [ ] **Step 3 GREEN: minimal implementation.** Replace `crates/rb-daemon/src/jobs/mod.rs` entirely with:

```rust
//! Opt-in background "evolution" jobs: bounded, idempotent maintenance passes
//! that read via the read pool and mutate ONLY through the single writer.

mod config;
mod link_decay;
pub mod scheduler;

pub use config::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
pub use rb_types::JobKind;

use crate::StoreHandle;
use serde::{Deserialize, Serialize};

/// What a single job pass touched. Returned by `run_once`, logged by the
/// scheduler, and surfaced to the CLI via `Response::JobRan`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    pub scanned: u64,
    pub changed: u64,
    pub skipped: u64,
}

/// Run ONE bounded, idempotent pass of `kind`. Reads via the read pool; every
/// mutation goes through `store` (the single writer). Fail-safe: returns `Err`
/// on failure without leaving partial state (each write is its own txn); never
/// panics. Dispatches to the per-job `run` with the matching sub-config.
pub async fn run_once(
    kind: JobKind,
    store: &StoreHandle,
    config: &JobsConfig,
) -> rb_types::Result<JobSummary> {
    match kind {
        JobKind::LinkDecay => link_decay::run(store, &config.link_decay).await,
        JobKind::Consolidation => Err(rb_types::Error::InvalidArgument(
            "consolidation job is not implemented yet".to_string(),
        )),
        JobKind::ImportanceRecalibration => Err(rb_types::Error::InvalidArgument(
            "importance recalibration job is not implemented yet".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn job_summary_default_is_all_zero() {
        let s = JobSummary::default();
        assert_eq!(s.scanned, 0);
        assert_eq!(s.changed, 0);
        assert_eq!(s.skipped, 0);
    }

    #[test]
    fn job_summary_round_trips_json() {
        let s = JobSummary {
            scanned: 9,
            changed: 4,
            skipped: 5,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: JobSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_link_decay_on_empty_store_scans_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, 8, 1).unwrap();
        let config = JobsConfig::default();

        let summary = run_once(JobKind::LinkDecay, &store, &config)
            .await
            .unwrap();
        assert_eq!(summary, JobSummary::default());

        store.shutdown().await;
    }
}
```

Note: this file references `mod scheduler;` and `link_decay::run`. To keep this task compiling, create a temporary stub `crates/rb-daemon/src/jobs/scheduler.rs` with just a doc comment (the real scheduler arrives in R7), and create `crates/rb-daemon/src/jobs/link_decay.rs` with a minimal `run` that returns an empty summary for the empty store (the real algorithm + store seam arrive in R5/R6). Create `crates/rb-daemon/src/jobs/scheduler.rs`:

```rust
//! Interval scheduler for enabled evolution jobs. Implemented in Part R, Task R7.
```

Create `crates/rb-daemon/src/jobs/link_decay.rs`:

```rust
//! Link-decay job: exponentially decay link `strength` by age, floored. Reads
//! candidate links via the read pool; writes via the single writer.

use crate::jobs::config::LinkDecayConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;

/// Run one bounded link-decay pass. (R4 stub: no candidate source yet, so an
/// empty store scans nothing. Real algorithm + store seam land in R5/R6.)
pub async fn run(
    _store: &StoreHandle,
    _config: &LinkDecayConfig,
) -> rb_types::Result<JobSummary> {
    Ok(JobSummary::default())
}
```

Finally, update the re-export in `crates/rb-daemon/src/lib.rs` to add `JobKind`, `JobSummary`, and `run_once` to the existing jobs re-export line, replacing it with:

```rust
pub use jobs::{
    run_once, ConsolidationConfig, ImportanceConfig, JobKind, JobSummary, JobsConfig,
    LinkDecayConfig,
};
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon jobs::tests` — Expected: PASS (3 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/jobs/mod.rs crates/rb-daemon/src/jobs/link_decay.rs crates/rb-daemon/src/jobs/scheduler.rs crates/rb-daemon/src/lib.rs && git commit -m "feat(rb-daemon): add JobSummary and run_once dispatch core"` — Expected: one commit.

---

### Task R5: rb-store `store.rs` — link decay read/write seam

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1 RED: write the failing test.** Add these tests to the existing `#[cfg(test)] mod tests` block in `crates/rb-store/src/store.rs` (the module already starts with `#![allow(clippy::panic)]` and `use super::*;`; add a local `#![allow(clippy::unwrap_used, clippy::expect_used)]` is unnecessary because the crate's `lib.rs` already sets `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`). Append:

```rust
    #[test]
    fn links_for_decay_returns_link_rows_bounded_by_limit() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("decay".to_string());

        // Two real memories to satisfy the FK on memory_links.
        let a = MemoryNote::new(ns.clone(), "source".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "target".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();

        let created = chrono::Utc::now();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: created,
            })
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.source, a.id);
        assert_eq!(row.target, b.id);
        assert_eq!(row.link_type, rb_types::LinkType::References);
        assert!((row.strength - 0.8).abs() < f32::EPSILON);
        assert_eq!(row.created_at.timestamp(), created.timestamp());

        // The limit is honoured.
        let none = store.links_for_decay(0).unwrap();
        assert!(none.is_empty(), "limit 0 returns no rows");
    }

    #[test]
    fn set_link_strength_updates_only_the_matching_edge() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("setstr".to_string());
        let a = MemoryNote::new(ns.clone(), "s".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "t".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        store
            .set_link_strength(&a.id, &b.id, rb_types::LinkType::References, 0.25)
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].strength - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn delete_link_removes_only_the_matching_edge() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("dellink".to_string());
        let a = MemoryNote::new(ns.clone(), "s".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "t".to_string(), MemoryType::Insight, 5);
        store.insert_memory(&a, Some(&[0.1f32; 8])).unwrap();
        store.insert_memory(&b, Some(&[0.2f32; 8])).unwrap();
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "r".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        // A second, differently-typed edge between the same nodes must survive.
        store
            .add_link(&MemoryLink {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                link_type: rb_types::LinkType::Extends,
                strength: 0.4,
                reason: "r2".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        store
            .delete_link(&a.id, &b.id, rb_types::LinkType::References)
            .unwrap();

        let rows = store.links_for_decay(10).unwrap();
        assert_eq!(rows.len(), 1, "only the References edge was deleted");
        assert_eq!(rows[0].link_type, rb_types::LinkType::Extends);
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-store links_for_decay` — Expected: FAIL — compile error `no method named links_for_decay` / `set_link_strength` / `delete_link` / `cannot find type LinkRow`.

- [ ] **Step 3 GREEN: minimal implementation.** Add a public `LinkRow` struct and three inherent methods to the `impl SqliteStore { ... }` block (the one that ends at line 150 with `checkpoint_truncate`). Insert the new methods immediately before the closing `}` of that block, right after `checkpoint_truncate`:

```rust
    /// One link edge selected for decay. `created_at` is decoded fail-closed.
    /// Defined here (not as a `Store` trait method) because the decay job calls
    /// it directly through the read pool, outside the engine's namespace scope.

    /// Read up to `limit` link edges for the decay job, newest-irrelevant order
    /// (PK order is fine; decay is per-row and idempotent). One query, no joins.
    pub fn links_for_decay(&self, limit: usize) -> Result<Vec<LinkRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT source_id, target_id, link_type, strength, created_at
                 FROM memory_links
                 LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            let (src, tgt, lt, strength, created) =
                r.map_err(|e| Error::Storage(e.to_string()))?;
            out.push(LinkRow {
                source: src.parse::<MemoryId>()?,
                target: tgt.parse::<MemoryId>()?,
                link_type: rb_types::LinkType::parse(&lt)?,
                // strength is SQLite REAL (f64) narrowed to f32, matching load_links.
                strength: strength as f32,
                created_at: from_ts(created)?,
            });
        }
        Ok(out)
    }

    /// Set the `strength` of a single link edge identified by its full PK.
    /// A missing edge is a no-op (0 rows updated); decay is best-effort.
    pub fn set_link_strength(
        &self,
        source: &MemoryId,
        target: &MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE memory_links SET strength = ?1
                 WHERE source_id = ?2 AND target_id = ?3 AND link_type = ?4",
                rusqlite::params![
                    strength as f64,
                    source.to_string(),
                    target.to_string(),
                    link_type.as_str(),
                ],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    /// Delete a single link edge identified by its full PK. A missing edge is a
    /// no-op. Used by the decay job's `prune_below_floor` policy.
    pub fn delete_link(
        &self,
        source: &MemoryId,
        target: &MemoryId,
        link_type: rb_types::LinkType,
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM memory_links
                 WHERE source_id = ?1 AND target_id = ?2 AND link_type = ?3",
                rusqlite::params![
                    source.to_string(),
                    target.to_string(),
                    link_type.as_str(),
                ],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }
```

Then define the `LinkRow` struct at module scope. Add it immediately after the `use` block at the top of `crates/rb-store/src/store.rs` (after line 8, the `use std::path::Path;` line):

```rust
/// One link edge as read by the link-decay job. Public so the daemon's job code
/// can consume it via the read pool. Not part of the `Store` trait: it is a
/// cross-namespace maintenance read, not an engine operation.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRow {
    pub source: MemoryId,
    pub target: MemoryId,
    pub link_type: MemoryType,
    pub strength: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

Wait — `link_type` must be `rb_types::LinkType`, not `MemoryType`. Use this exact struct instead:

```rust
/// One link edge as read by the link-decay job. Public so the daemon's job code
/// can consume it via the read pool. Not part of the `Store` trait: it is a
/// cross-namespace maintenance read, not an engine operation.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRow {
    pub source: MemoryId,
    pub target: MemoryId,
    pub link_type: rb_types::LinkType,
    pub strength: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

Finally export `LinkRow` from the crate. Update `crates/rb-store/src/lib.rs` `pub use` line to:

```rust
pub use store::{LinkRow, SqliteStore, Store};
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-store link` — Expected: PASS (the 3 new tests plus existing link tests; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-store/src/store.rs crates/rb-store/src/lib.rs && git commit -m "feat(rb-store): add links_for_decay, set_link_strength, delete_link"` — Expected: one commit.

---

### Task R6: rb-daemon `store_handle.rs` — writer commands + read helper

**Files:**
- Modify: `crates/rb-daemon/src/store_handle.rs`
- Test: `crates/rb-daemon/src/store_handle.rs` (inline `#[cfg(test)]`)

This task adds two new `WriteCommand`s (`SetLinkStrength`, `DeleteLink`), their writer-loop arms (each routed through `run_store_op` so panics are contained), the `StoreHandle` async methods, and a `links_for_decay` read helper on `StoreHandle` that the job code calls via the read pool.

- [ ] **Step 1 RED: write the failing test.** Add to the `#[cfg(test)] mod tests` block in `crates/rb-daemon/src/store_handle.rs` (it already starts with `#![allow(clippy::unwrap_used, clippy::expect_used)]` and `use super::*;`):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_link_decay_read_write_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("decay-handle".to_string());

        let a = note(&ns, "source for decay");
        let b = note(&ns, "target for decay");
        let (aid, bid) = (a.id.clone(), b.id.clone());
        handle.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        handle
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.9,
                reason: "seed".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // Read candidates via the pool.
        let rows = handle.links_for_decay(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].strength - 0.9).abs() < f32::EPSILON);

        // Set strength via the single writer.
        handle
            .set_link_strength(aid.clone(), bid.clone(), rb_types::LinkType::References, 0.3)
            .await
            .unwrap();
        let rows = handle.links_for_decay(10).await.unwrap();
        assert!((rows[0].strength - 0.3).abs() < f32::EPSILON);

        // Delete via the single writer.
        handle
            .delete_link(aid, bid, rb_types::LinkType::References)
            .await
            .unwrap();
        let rows = handle.links_for_decay(10).await.unwrap();
        assert!(rows.is_empty(), "link removed");

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon store_handle_link_decay` — Expected: FAIL — compile error `no method named links_for_decay` / `set_link_strength` / `delete_link` on `StoreHandle`.

- [ ] **Step 3 GREEN: minimal implementation.** Three edits to `crates/rb-daemon/src/store_handle.rs`.

(a) Add the two variants to the `enum WriteCommand { ... }`, immediately after the `RecordAccesses { ... }` variant and before the `#[cfg(test)] PanicForTest` variant:

```rust
    SetLinkStrength {
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
        reply: oneshot::Sender<Result<()>>,
    },
    DeleteLink {
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        reply: oneshot::Sender<Result<()>>,
    },
```

(b) Add the matching writer-loop arms inside `writer_loop`'s `match cmd { ... }`, immediately after the `WriteCommand::RecordAccesses { .. } => { ... }` arm and before the `#[cfg(test)] WriteCommand::PanicForTest` arm. No `MemoryChanged` event: link strength/deletion is maintenance, not a memory mutation.

```rust
            WriteCommand::SetLinkStrength {
                source,
                target,
                link_type,
                strength,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.set_link_strength(&source, &target, link_type, strength)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::DeleteLink {
                source,
                target,
                link_type,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.delete_link(&source, &target, link_type)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
```

(c) Add three public methods to `StoreHandle`. Place them inside the `impl StoreHandle { ... }` block that holds `start`/`subscribe`/`shutdown` — append them just before the closing `}` of that block (after `read_pool_len_for_test`). These are NOT part of the `MemoryBackend` trait; they are inherent helpers the jobs call directly:

```rust
    /// Read up to `limit` link edges (cross-namespace) via the read pool, for
    /// the link-decay job. Reads never go through the writer.
    pub async fn links_for_decay(&self, limit: usize) -> Result<Vec<rb_store::LinkRow>> {
        self.with_read(move |store| store.links_for_decay(limit))
            .await
    }

    /// Set the strength of a single link edge through the single writer.
    pub async fn set_link_strength(
        &self,
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::SetLinkStrength {
            source,
            target,
            link_type,
            strength,
            reply,
        };
        self.send_write(cmd, rx).await
    }

    /// Delete a single link edge through the single writer.
    pub async fn delete_link(
        &self,
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::DeleteLink {
            source,
            target,
            link_type,
            reply,
        };
        self.send_write(cmd, rx).await
    }
```

`rb_store::LinkRow` is already a dependency of `rb-daemon` (the crate depends on `rb-store`); add `use rb_store::LinkRow;` is unnecessary because the method uses the fully qualified path `rb_store::LinkRow`.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon store_handle_link_decay` — Expected: PASS (1 test).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/store_handle.rs && git commit -m "feat(rb-daemon): add SetLinkStrength/DeleteLink writer commands and links_for_decay read"` — Expected: one commit.

---

### Task R7: rb-daemon `jobs/link_decay.rs` — decay algorithm

**Files:**
- Modify: `crates/rb-daemon/src/jobs/link_decay.rs`
- Test: `crates/rb-daemon/src/jobs/link_decay.rs` (inline `#[cfg(test)]`)

The pure `decayed_strength` helper takes `now` as `age_days` (already computed) so tests are deterministic. `run` computes `age_days` from a passed-in `now`, then issues writes via the single writer.

- [ ] **Step 1 RED: write the failing test.** Replace `crates/rb-daemon/src/jobs/link_decay.rs` so it carries this test module (the real `run`/`decayed_strength` are added in Step 3):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::jobs::config::LinkDecayConfig;
    use rb_types::Namespace;

    const DIM: usize = 8;

    #[test]
    fn decayed_strength_is_monotonic_non_increasing_in_age() {
        let (hl, floor) = (30.0_f64, 0.05_f64);
        let mut prev = decayed_strength(0.9, 0.0, hl, floor);
        for age in [1.0, 5.0, 30.0, 60.0, 365.0] {
            let cur = decayed_strength(0.9, age, hl, floor);
            assert!(cur <= prev, "decay must not increase with age");
            prev = cur;
        }
    }

    #[test]
    fn decayed_strength_never_below_floor_and_never_above_input() {
        let (hl, floor) = (30.0_f64, 0.05_f64);
        for age in [0.0, 10.0, 100.0, 10_000.0] {
            let s = decayed_strength(0.9, age, hl, floor);
            assert!(s >= floor as f32 - f32::EPSILON, "never below floor");
            assert!(s <= 0.9 + f32::EPSILON, "never above input");
        }
    }

    #[test]
    fn decayed_strength_halves_at_one_half_life() {
        let s = decayed_strength(0.8, 30.0, 30.0, 0.0);
        assert!((s - 0.4).abs() < 1e-5, "one half-life halves strength, got {s}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_decays_an_old_link_via_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("decay-run".to_string());

        let a = rb_types::MemoryNote::new(
            ns.clone(),
            "source".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let b = rb_types::MemoryNote::new(
            ns.clone(),
            "target".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        store.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        // Link created 60 days ago (two half-lives at hl=30).
        let created = chrono::Utc::now() - chrono::Duration::days(60);
        store
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "seed".to_string(),
                created_at: created,
            })
            .await
            .unwrap();

        let cfg = LinkDecayConfig {
            enabled: true,
            interval_secs: 86_400,
            half_life_days: 30.0,
            floor: 0.05,
            prune_below_floor: false,
            batch_limit: 1000,
        };
        let now = chrono::Utc::now();
        let summary = run_at(&store, &cfg, now).await.unwrap();

        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.skipped, 0);

        // 0.8 over two half-lives ≈ 0.2, comfortably above the 0.05 floor.
        let rows = store.links_for_decay(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].strength - 0.2).abs() < 1e-3, "got {}", rows[0].strength);

        store.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_prunes_below_floor_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = crate::StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("decay-prune".to_string());

        let a = rb_types::MemoryNote::new(
            ns.clone(),
            "source".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let b = rb_types::MemoryNote::new(
            ns.clone(),
            "target".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        store.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        // Very old + weak: 0.1 over ~10 half-lives -> well under the 0.05 floor.
        let created = chrono::Utc::now() - chrono::Duration::days(300);
        store
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.1,
                reason: "seed".to_string(),
                created_at: created,
            })
            .await
            .unwrap();

        let cfg = LinkDecayConfig {
            enabled: true,
            interval_secs: 86_400,
            half_life_days: 30.0,
            floor: 0.05,
            prune_below_floor: true,
            batch_limit: 1000,
        };
        let summary = run_at(&store, &cfg, chrono::Utc::now()).await.unwrap();
        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.changed, 1);

        let rows = store.links_for_decay(10).await.unwrap();
        assert!(rows.is_empty(), "weak old link pruned below floor");

        store.shutdown().await;
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon link_decay` — Expected: FAIL — compile error `cannot find function decayed_strength` / `cannot find function run_at`.

- [ ] **Step 3 GREEN: minimal implementation.** Replace the top of `crates/rb-daemon/src/jobs/link_decay.rs` (everything above the `#[cfg(test)] mod tests` block) with:

```rust
//! Link-decay job: exponentially decay link `strength` by age, floored. Reads
//! candidate links via the read pool; writes via the single writer. Bounded by
//! `batch_limit`; idempotent (re-running on already-decayed links no-ops once
//! the value stops moving by more than `EPSILON`).

use crate::jobs::config::LinkDecayConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;

/// Minimum strength delta that counts as a change. Below this the row is left
/// untouched so the pass is idempotent and avoids pointless writes.
const EPSILON: f32 = 1e-6;

/// Pure decay function. `age_days` is the link's age; `half_life_days` the decay
/// constant. The result is `strength * 0.5^(age/half_life)`, floored at `floor`,
/// never exceeding the input. Deterministic; the unit tests pin its invariants.
pub fn decayed_strength(strength: f32, age_days: f64, half_life_days: f64, floor: f64) -> f32 {
    // A non-positive half-life is meaningless; treat it as "no decay" rather
    // than dividing by zero (fail-safe: never panics, never NaN).
    if half_life_days <= 0.0 || age_days <= 0.0 {
        return strength.max(floor as f32);
    }
    let factor = 0.5_f64.powf(age_days / half_life_days);
    let decayed = (strength as f64) * factor;
    decayed.max(floor) as f32
}

/// Run one bounded link-decay pass using `chrono::Utc::now()` as the clock.
pub async fn run(store: &StoreHandle, config: &LinkDecayConfig) -> rb_types::Result<JobSummary> {
    run_at(store, config, chrono::Utc::now()).await
}

/// Run one bounded pass with an injected `now` (deterministic in tests).
pub async fn run_at(
    store: &StoreHandle,
    config: &LinkDecayConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> rb_types::Result<JobSummary> {
    let rows = store.links_for_decay(config.batch_limit).await?;
    let mut summary = JobSummary::default();

    for row in rows {
        summary.scanned += 1;

        let age_secs = (now - row.created_at).num_seconds();
        let age_days = (age_secs as f64) / 86_400.0;
        let new_strength = decayed_strength(
            row.strength,
            age_days,
            config.half_life_days,
            config.floor,
        );

        let floor = config.floor as f32;
        if config.prune_below_floor && new_strength <= floor {
            store
                .delete_link(row.source.clone(), row.target.clone(), row.link_type)
                .await?;
            summary.changed += 1;
        } else if (new_strength - row.strength).abs() > EPSILON {
            store
                .set_link_strength(
                    row.source.clone(),
                    row.target.clone(),
                    row.link_type,
                    new_strength,
                )
                .await?;
            summary.changed += 1;
        } else {
            summary.skipped += 1;
        }
    }

    Ok(summary)
}
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon link_decay` — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/jobs/link_decay.rs && git commit -m "feat(rb-daemon): implement bounded idempotent link decay algorithm"` — Expected: one commit.

---

### Task R8: rb-daemon `jobs/scheduler.rs` — interval scheduler

**Files:**
- Modify: `crates/rb-daemon/src/jobs/scheduler.rs`
- Modify: `crates/rb-daemon/src/server.rs`
- Test: `crates/rb-daemon/src/jobs/scheduler.rs` (inline `#[cfg(test)]`)

The scheduler spawns one tokio task per *enabled* job; each ticks on its `interval_secs` and calls `run_once`, logging the `JobSummary` at info and any error at warn (never fatal). It returns a `JoinHandle<()>` that `Daemon::run` aborts on shutdown. Disabled jobs are never scheduled.

- [ ] **Step 1 RED: write the failing test.** Replace `crates/rb-daemon/src/jobs/scheduler.rs` so it carries this test module (the real `spawn` is added in Step 3). The test drives the scheduler with a tiny interval against a real `StoreHandle` and asserts the enabled job actually ran (an old link decays):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::jobs::{JobsConfig, LinkDecayConfig};
    use crate::StoreHandle;
    use rb_types::Namespace;

    const DIM: usize = 8;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enabled_link_decay_job_runs_on_its_interval() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("sched".to_string());

        let a = rb_types::MemoryNote::new(
            ns.clone(),
            "s".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let b = rb_types::MemoryNote::new(
            ns.clone(),
            "t".to_string(),
            rb_types::MemoryType::Insight,
            5,
        );
        let (aid, bid) = (a.id.clone(), b.id.clone());
        store.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        store.write(b, Some(vec![0.2f32; DIM])).await.unwrap();
        let created = chrono::Utc::now() - chrono::Duration::days(60);
        store
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "seed".to_string(),
                created_at: created,
            })
            .await
            .unwrap();

        // Tiny interval so the first tick fires almost immediately.
        let config = JobsConfig {
            link_decay: LinkDecayConfig {
                enabled: true,
                interval_secs: 0,
                half_life_days: 30.0,
                floor: 0.05,
                prune_below_floor: false,
                batch_limit: 1000,
            },
            ..Default::default()
        };

        let handle = spawn(store.clone(), config);

        // Poll until the strength has been reduced by the running job.
        let mut decayed = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let rows = store.links_for_decay(10).await.unwrap();
            if !rows.is_empty() && rows[0].strength < 0.79 {
                decayed = true;
                break;
            }
        }
        assert!(decayed, "enabled link-decay job must run and reduce strength");

        handle.abort();
        store.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn all_disabled_config_spawns_no_work() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = StoreHandle::start(db, DIM, 1).unwrap();

        // Default config: every job disabled -> the join handle finishes promptly
        // (no jobs scheduled, the supervisor returns immediately).
        let handle = spawn(store.clone(), JobsConfig::default());
        let joined = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "disabled config must not spawn any ticking job");

        store.shutdown().await;
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon scheduler` — Expected: FAIL — compile error `cannot find function spawn`.

- [ ] **Step 3 GREEN: minimal implementation.** Replace the top of `crates/rb-daemon/src/jobs/scheduler.rs` (everything above the `#[cfg(test)] mod tests` block) with:

```rust
//! Interval scheduler for enabled evolution jobs. Spawns one tokio task per
//! enabled job; each ticks on its `interval_secs` and calls `run_once`, logging
//! the summary at info and errors at warn. A job error is logged and the loop
//! continues (never fatal, never unwraps). Disabled jobs are never scheduled.
//! The returned `JoinHandle` is aborted by `Daemon::run` on shutdown.

use crate::jobs::{run_once, JobKind, JobsConfig};
use crate::StoreHandle;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Spawn the job supervisor. Returns a single `JoinHandle` owning all per-job
/// tasks; aborting it aborts the whole tree (each child is spawned detached and
/// the supervisor holds their handles, aborting them when it is itself aborted).
pub fn spawn(store: StoreHandle, config: JobsConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut children: Vec<JoinHandle<()>> = Vec::new();

        if config.link_decay.enabled {
            children.push(spawn_job(
                JobKind::LinkDecay,
                config.link_decay.interval_secs,
                store.clone(),
                config.clone(),
            ));
        }
        if config.consolidation.enabled {
            children.push(spawn_job(
                JobKind::Consolidation,
                config.consolidation.interval_secs,
                store.clone(),
                config.clone(),
            ));
        }
        if config.importance.enabled {
            children.push(spawn_job(
                JobKind::ImportanceRecalibration,
                config.importance.interval_secs,
                store.clone(),
                config.clone(),
            ));
        }

        if children.is_empty() {
            // Nothing enabled: return immediately so a disabled config spawns no
            // long-lived task.
            return;
        }

        // Hold the children alive until aborted. Awaiting a child only resolves
        // if that child panics (its tick loop never returns otherwise); on abort
        // of this supervisor, the children are dropped, which aborts them too.
        for child in children {
            let _ = child.await;
        }
    })
}

/// Spawn a single job's tick loop. The first tick fires immediately, then every
/// `max(interval_secs, 1)` seconds. Each tick is fail-safe: an error is logged
/// at warn and the loop continues.
fn spawn_job(
    kind: JobKind,
    interval_secs: u64,
    store: StoreHandle,
    config: JobsConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let period = Duration::from_secs(interval_secs.max(1));
        let mut ticker = tokio::time::interval(period);
        // Default MissedTickBehavior::Burst is fine: we never want to skip work,
        // and ticks are seconds apart at minimum.
        loop {
            ticker.tick().await;
            match run_once(kind, &store, &config).await {
                Ok(summary) => info!(
                    job = kind.as_str(),
                    scanned = summary.scanned,
                    changed = summary.changed,
                    skipped = summary.skipped,
                    "evolution job completed"
                ),
                Err(e) => warn!(job = kind.as_str(), error = %e, "evolution job failed"),
            }
        }
    })
}
```

Now wire the scheduler into the daemon. Two edits to `crates/rb-daemon/src/server.rs`.

(i) Add `jobs_config` to `DaemonConfig` and a `jobs` import. Change the import line:

```rust
use crate::error_map::error_to_response;
use crate::jobs::{self, JobsConfig};
use crate::{SharedEmbedder, StoreHandle};
```

Change the `DaemonConfig` struct to:

```rust
/// Static configuration for a daemon instance.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub read_pool_size: usize,
    pub jobs_config: JobsConfig,
}
```

(ii) Carry the `JobsConfig` from `bind` into `run`, and spawn/abort the scheduler. Add a `jobs_config` field to the `Daemon` struct:

```rust
/// A bound, ready-to-run daemon.
pub struct Daemon {
    listener: UnixListener,
    store: StoreHandle,
    embedder: SharedEmbedder,
    enricher: Option<Arc<dyn Enricher>>,
    socket_path: PathBuf,
    pidfile_path: PathBuf,
    bind_guard: BindGuard,
    jobs_config: JobsConfig,
}
```

In `Daemon::bind`, store the config into the returned struct. Change the final `Ok(Self { ... })` to include `jobs_config`:

```rust
        info!(socket = %config.socket_path.display(), "daemon bound");
        Ok(Self {
            listener,
            store,
            embedder,
            enricher,
            socket_path: config.socket_path,
            pidfile_path,
            bind_guard,
            jobs_config: config.jobs_config,
        })
```

In `Daemon::run`, destructure the new field, spawn the scheduler after building the accept loop locals, and abort it on shutdown. Change the destructure at the top of `run`:

```rust
        let Daemon {
            listener,
            store,
            embedder,
            enricher,
            socket_path: _socket_path,
            pidfile_path: _pidfile_path,
            mut bind_guard,
            jobs_config,
        } = self;
        tokio::pin!(shutdown);
        let scheduler = jobs::scheduler::spawn(store.clone(), jobs_config);
        let mut conns: JoinSet<()> = JoinSet::new();
        let conn_sem = Arc::new(Semaphore::new(MAX_CONNECTIONS));
```

After the accept loop's closing `}` (the `loop { ... }`), abort the scheduler before draining connections. Change the post-loop block to:

```rust
        drop(listener);
        scheduler.abort();
        conns.shutdown().await;
        store.shutdown().await;

        bind_guard.cleanup();
        info!("daemon shut down cleanly");
        Ok(())
```

The existing `server.rs` tests do not construct `DaemonConfig` directly (they only test `prepare_socket_dir` / `validate_namespace`), so no test edit is required there — but the `daemon_e2e.rs` integration test and `serve.rs` DO construct `DaemonConfig`; those are updated in R10 (`run_serve`) and the gate. Since this task changes `DaemonConfig`, also update `crates/rusty-brain/src/serve.rs` `run_with_embedder` now so the workspace stays buildable:

In `crates/rusty-brain/src/serve.rs`, change the `DaemonConfig { ... }` construction in `run_with_embedder` to include the new field (defaulting to all-disabled for now; the real wiring of `--jobs-config` lands in R10):

```rust
    let config = DaemonConfig {
        socket_path,
        db_path,
        read_pool_size,
        jobs_config: rb_daemon::JobsConfig::default(),
    };
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon scheduler` — Expected: PASS (2 tests). Then `cargo build --workspace` — Expected: PASS (the `DaemonConfig` field addition compiles everywhere).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/jobs/scheduler.rs crates/rb-daemon/src/server.rs crates/rusty-brain/src/serve.rs && git commit -m "feat(rb-daemon): schedule enabled evolution jobs on their interval"` — Expected: one commit.

---

### Task R9: rb-proto `messages.rs` + `client.rs` — RunJob wire op

**Files:**
- Modify: `crates/rb-proto/src/messages.rs`
- Modify: `crates/rb-proto/src/client.rs`
- Modify: `crates/rb-mcp/src/server.rs`
- Modify: `crates/rb-mcp/src/proxy.rs`
- Test: `crates/rb-proto/src/messages.rs` and `crates/rb-proto/src/client.rs` (inline `#[cfg(test)]`)

Adds `Request::RunJob { job: JobKind }` and `Response::JobRan { scanned, changed, skipped }`, updates the exhaustive round-trip tests + matches, adds `Client::run_job`, and threads the new variant through every exhaustive match in the workspace (`rb-mcp` `FakeProxy::call`, `rb-mcp` `response_to_content`, and `rb-proto` `client.rs` `serve` test responder).

- [ ] **Step 1 RED: write the failing test.** In `crates/rb-proto/src/messages.rs`, the test module has its own `use rb_types::{ ... }` line (it does not yet import `JobKind`); add `JobKind` to it so the new tests can name the variant unqualified. Change the test module's import line to:

```rust
    use rb_types::{
        JobKind, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, SearchResult,
    };
```

Then add `RunJob` to `all_requests()` (in the test module) and `JobRan` to `all_responses()`. Append to the `vec![ ... ]` returned by `all_requests()` (just before the closing `]`, after `Request::Ping`):

```rust
            Request::RunJob {
                job: JobKind::LinkDecay,
            },
```

Append to the `vec![ ... ]` returned by `all_responses()` (just before the closing `]`, after `Response::Error { ... }`):

```rust
            Response::JobRan {
                scanned: 10,
                changed: 3,
                skipped: 7,
            },
```

Also add a focused tag test to the `messages.rs` test module:

```rust
    #[test]
    fn run_job_uses_op_tag_with_snake_case_job() {
        let json = serde_json::to_string(&Request::RunJob {
            job: JobKind::LinkDecay,
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"RunJob","job":"link_decay"}"#);
    }

    #[test]
    fn job_ran_uses_result_tag() {
        let json = serde_json::to_string(&Response::JobRan {
            scanned: 1,
            changed: 0,
            skipped: 1,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"result":"JobRan","scanned":1,"changed":0,"skipped":1}"#
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-proto messages` — Expected: FAIL — compile error `no variant RunJob on Request` / `no variant JobRan on Response`.

- [ ] **Step 3 GREEN: minimal implementation.** Add the variants. First change the TOP import line of `messages.rs` (the production `use rb_types::{...}` at the head of the file) so `JobKind` is in scope for the enum:

```rust
use rb_types::{JobKind, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, SearchResult};
```

Add `RunJob` to the `Request` enum (after `Context,` and before `Ping,`):

```rust
    RunJob {
        job: JobKind,
    },
```

Add `JobRan` to the `Response` enum (after `Pong { ... }` and before `Error { ... }`):

```rust
    JobRan {
        scanned: u64,
        changed: u64,
        skipped: u64,
    },
```

- [ ] **Step 4 (part 1): add the client wrapper + its test.** In `crates/rb-proto/src/client.rs`, add to the second `impl Client { ... }` block (the one with `remember`/`recall`/...), after `ping`:

```rust
    /// Trigger one bounded evolution-job pass on the daemon; returns the summary
    /// `(scanned, changed, skipped)`. The daemon performs all mutations through
    /// its single writer; this is just the wire trigger.
    pub async fn run_job(&mut self, job: rb_types::JobKind) -> Result<(u64, u64, u64)> {
        let resp = self.request(Request::RunJob { job }).await?;
        match resp {
            Resp::JobRan {
                scanned,
                changed,
                skipped,
            } => Ok((scanned, changed, skipped)),
            other => Err(Self::unexpected(other)),
        }
    }
```

Update the `serve` test responder in the `wrapper_tests` module of `client.rs` so its exhaustive `match req { ... }` covers the new variant. Add this arm to the `match req` inside `serve`, after the `Request::Ping => ...` arm:

```rust
                Request::RunJob { .. } => Response::JobRan {
                    scanned: 4,
                    changed: 2,
                    skipped: 2,
                },
```

Add a wrapper test to the `wrapper_tests` module (inside `typed_wrappers_return_domain_types`, after the `ping` assertion and before `drop(c)`):

```rust
        let (scanned, changed, skipped) =
            c.run_job(rb_types::JobKind::LinkDecay).await.unwrap();
        assert_eq!((scanned, changed, skipped), (4, 2, 2));
```

- [ ] **Step 4 (part 2): thread the new variant through rb-mcp exhaustive matches.** In `crates/rb-mcp/src/server.rs`, add an arm to `FakeProxy::call`'s `match request { ... }` (after `Request::Ping => ...`):

```rust
                Request::RunJob { .. } => Response::JobRan {
                    scanned: 0,
                    changed: 0,
                    skipped: 0,
                },
```

In `crates/rb-mcp/src/proxy.rs`, add an arm to `response_to_content`'s `match resp { ... }` (after `Response::Pong { .. } => ...` and before `Response::Error { .. } => ...`):

```rust
        Response::JobRan {
            scanned,
            changed,
            skipped,
        } => json!({ "scanned": scanned, "changed": changed, "skipped": skipped }),
```

- [ ] **Step 4 (part 3): run it.** Run: `cargo test -p rb-proto messages run_job` then `cargo test -p rb-proto` then `cargo build -p rb-mcp` — Expected: PASS for rb-proto (all round-trip + tag + wrapper tests), and rb-mcp builds with the new exhaustive arms.

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-proto --all-targets -- -D warnings && cargo clippy -p rb-mcp --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-proto/src/messages.rs crates/rb-proto/src/client.rs crates/rb-mcp/src/server.rs crates/rb-mcp/src/proxy.rs && git commit -m "feat(rb-proto): add RunJob request and JobRan response with client wrapper"` — Expected: one commit.

---

### Task R10: rb-daemon dispatch + rusty-brain `evolve` CLI

**Files:**
- Modify: `crates/rb-daemon/src/server.rs`
- Modify: `crates/rusty-brain/src/cli.rs`
- Modify: `crates/rusty-brain/src/run.rs`
- Modify: `crates/rusty-brain/src/serve.rs`
- Modify: `crates/rusty-brain/src/paths.rs`
- Test: `crates/rusty-brain/src/cli.rs` and `crates/rusty-brain/src/paths.rs` (inline `#[cfg(test)]`)

The daemon dispatch `RunJob` arm calls `jobs::run_once` using a `StoreHandle` clone (NOT the namespace-bound engine — jobs are cross-namespace maintenance). The CLI gains `evolve <job>` which maps a string to `JobKind`, sends `Request::RunJob`, and prints the summary. A `--jobs-config` flag / `RB_JOBS_CONFIG` env resolves a `JobsConfig` path that `run_serve` loads at startup.

- [ ] **Step 1 RED: write the failing test.** Add a CLI parse test to `crates/rusty-brain/src/cli.rs`. Since the file currently has no test module, add one at the bottom:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use clap::Parser;

    #[test]
    fn evolve_parses_link_decay_job() {
        let cli = Cli::parse_from(["rusty-brain", "evolve", "link_decay"]);
        match cli.command {
            Command::Evolve { job } => assert_eq!(job, "link_decay"),
            other => panic!("expected Evolve, got {other:?}"),
        }
    }

    #[test]
    fn serve_accepts_jobs_config_flag() {
        let cli = Cli::parse_from([
            "rusty-brain",
            "serve",
            "--jobs-config",
            "/tmp/jobs.toml",
        ]);
        match cli.command {
            Command::Serve { jobs_config } => {
                assert_eq!(jobs_config.as_deref(), Some("/tmp/jobs.toml"));
            }
            other => panic!("expected Serve, got {other:?}"),
        }
    }
}
```

Also add a path-resolution test to `crates/rusty-brain/src/paths.rs` (its test module already exists):

```rust
    #[test]
    fn jobs_config_prefers_override_then_env_then_none() {
        // Explicit override wins.
        assert_eq!(
            resolve_jobs_config_path(Some("/tmp/a.toml".to_string()), None),
            Some(PathBuf::from("/tmp/a.toml"))
        );
        // Env used when no override.
        assert_eq!(
            resolve_jobs_config_path(None, Some("/tmp/b.toml".to_string())),
            Some(PathBuf::from("/tmp/b.toml"))
        );
        // Neither -> None (all jobs disabled by default).
        assert_eq!(resolve_jobs_config_path(None, None), None);
        // Blank override falls through to env.
        assert_eq!(
            resolve_jobs_config_path(Some("  ".to_string()), Some("/tmp/c.toml".to_string())),
            Some(PathBuf::from("/tmp/c.toml"))
        );
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rusty-brain evolve` then `cargo test -p rusty-brain jobs_config` — Expected: FAIL — compile errors: no `Command::Evolve`, `Command::Serve` has no `jobs_config` field, `resolve_jobs_config_path` not found.

- [ ] **Step 3 GREEN: minimal implementation.** Four edits.

(a) `crates/rusty-brain/src/cli.rs`: change the `Serve` variant to carry the flag and add an `Evolve` variant. Replace the `Serve` variant:

```rust
    /// Run the memory daemon in the foreground until Ctrl-C.
    Serve {
        /// Path to the evolution-jobs TOML config (else `RB_JOBS_CONFIG`, else
        /// all jobs disabled).
        #[arg(long = "jobs-config", env = "RB_JOBS_CONFIG")]
        jobs_config: Option<String>,
    },
```

Add the `Evolve` variant after `Status,` (the last variant), and before the closing `}` of the enum:

```rust
    /// Trigger one bounded evolution-job pass on the running daemon.
    Evolve {
        /// Which job to run: `link_decay`, `consolidation`, or
        /// `importance_recalibration`.
        job: String,
    },
```

Note: the `Serve` variant now has a field, and `clap`'s `env` requires the `env` feature, which is already enabled (`clap = { ..., features = ["derive", "env"] }`).

(b) `crates/rusty-brain/src/paths.rs`: add the env constant and resolver. After the `DB_ENV` constant:

```rust
/// Env var that points at the evolution-jobs TOML config.
pub const JOBS_CONFIG_ENV: &str = "RB_JOBS_CONFIG";
```

After `resolve_db_path`:

```rust
/// Resolve the jobs-config path: explicit override wins, else the env value,
/// else `None` (meaning: load the all-disabled default). Blank strings are
/// treated as absent.
pub fn resolve_jobs_config_path(
    override_value: Option<String>,
    env_value: Option<String>,
) -> Option<PathBuf> {
    override_value
        .or(env_value)
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
}
```

(c) `crates/rusty-brain/src/serve.rs`: thread a `JobsConfig` through `run_serve` and `run_with_embedder`. Change `run_serve`'s signature and body to accept the resolved config path, load it, and build the `DaemonConfig` with it:

```rust
/// Run the daemon at the given paths until `shutdown` resolves.
/// Picks the embedding provider from the environment (`VOYAGE_API_KEY`).
pub async fn run_serve(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    jobs_config_path: Option<PathBuf>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let jobs_config = rb_daemon::JobsConfig::load(jobs_config_path.as_deref())?;
    let api_key = std::env::var("VOYAGE_API_KEY").ok();
    match select_provider_kind(api_key) {
        ProviderKind::Voyage => {
            let embedder = VoyageProvider::from_env()?;
            run_with_embedder(
                socket_path,
                db_path,
                read_pool_size,
                jobs_config,
                embedder,
                shutdown,
            )
            .await
        }
        ProviderKind::Deterministic => {
            tracing::warn!(
                "VOYAGE_API_KEY not set; using offline DeterministicProvider \
                 (dim {DEFAULT_DIM}). Recall quality is reduced and embeddings \
                 are not portable to a real model."
            );
            let embedder = DeterministicProvider::new(DEFAULT_DIM);
            run_with_embedder(
                socket_path,
                db_path,
                read_pool_size,
                jobs_config,
                embedder,
                shutdown,
            )
            .await
        }
    }
}
```

and change `run_with_embedder`:

```rust
/// Bind a daemon for a concrete embedder and run it to shutdown.
async fn run_with_embedder<P>(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    jobs_config: rb_daemon::JobsConfig,
    embedder: P,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()>
where
    P: EmbeddingProvider + 'static,
{
    let config = DaemonConfig {
        socket_path,
        db_path,
        read_pool_size,
        jobs_config,
    };
    let daemon = Daemon::bind(config, SharedEmbedder::new(embedder)).await?;
    daemon.run(shutdown).await
}
```

(d) `crates/rusty-brain/src/run.rs`: update the `Serve` dispatch to pass the resolved config path, and add an `Evolve` dispatch. Change the `Command::Serve` arm in `run`:

```rust
        Command::Serve { jobs_config } => {
            let jobs_config_path =
                paths::resolve_jobs_config_path(jobs_config, std::env::var("RB_JOBS_CONFIG").ok());
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(socket_path, db_path, 4, jobs_config_path, shutdown)
                .await
                .context("daemon failed")?;
            Ok(())
        }
```

Add `Evolve` to the `match cli.command` in `run` (it is a client command but maps a string to a job; handle it in `run_client`). The simplest is to route it through `run_client` like the others, so add it to the `other => run_client(...)` path by NOT special-casing it in `run`. Then add the `Command::Evolve` arm to `run_client`'s `match command { ... }`, after `Command::Status => { ... }`:

```rust
        Command::Evolve { job } => {
            let kind = rb_types::JobKind::parse(&job)
                .map_err(|e| anyhow::anyhow!("invalid job '{job}': {e}"))?;
            let (scanned, changed, skipped) =
                client.run_job(kind).await.context("evolve failed")?;
            if json {
                println!(
                    "{{\"scanned\":{scanned},\"changed\":{changed},\"skipped\":{skipped}}}"
                );
            } else {
                println!(
                    "evolve {job}: scanned={scanned} changed={changed} skipped={skipped}"
                );
            }
        }
```

`run_client`'s `match command` also has `Command::Serve` and `Command::Mcp` bail arms; `Command::Serve` now has a field, so its bail arm must match the new shape. Change the `Command::Serve => ...` arm in `run_client` to:

```rust
        Command::Serve { .. } => anyhow::bail!("internal: serve must be handled before run_client"),
```

(e) `crates/rb-daemon/src/server.rs`: add the daemon-side `RunJob` dispatch. The dispatch currently runs `dispatch(&engine, req)`; jobs are cross-namespace so they must use the `StoreHandle` (and `JobsConfig`), not the namespace-bound engine. Thread a `StoreHandle` clone and the `JobsConfig` into `handle_connection` and `dispatch`.

Change `handle_connection`'s signature to also take the store + jobs config:

```rust
async fn handle_connection(
    stream: UnixStream,
    store: StoreHandle,
    embedder: SharedEmbedder,
    enricher: Option<Arc<dyn Enricher>>,
    jobs_config: JobsConfig,
) -> Result<()> {
```

Inside `handle_connection`, the engine consumes `store` by value (`MemoryEngine::new(store, ...)`). Clone it for the job path before building the engine. Replace the `let engine = { ... };` block with:

```rust
    let job_store = store.clone();
    let engine = {
        let base = MemoryEngine::new(store, embedder, namespace);
        match enricher {
            Some(e) => base.with_enricher(e),
            None => base,
        }
    };
```

and replace the dispatch call inside the request loop:

```rust
        let resp = dispatch(&engine, &job_store, &jobs_config, req).await;
        write_frame(&mut framed, &resp).await?;
```

Change the `dispatch` signature and add the `RunJob` arm. Replace the `async fn dispatch<P>(...)` signature line and add the new match arm (after the `Request::Context => ...` arm, before the closing `}` of the match):

```rust
async fn dispatch<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    jobs_config: &JobsConfig,
    req: Request,
) -> Response
where
    P: EmbeddingProvider,
{
```

and the new arm:

```rust
        Request::RunJob { job } => match jobs::run_once(job, job_store, jobs_config).await {
            Ok(summary) => Response::JobRan {
                scanned: summary.scanned,
                changed: summary.changed,
                skipped: summary.skipped,
            },
            Err(e) => error_to_response(e),
        },
```

The per-connection spawn in `Daemon::run` must clone the store again for `handle_connection` (it already clones `store` for the engine; that clone is moved into the engine) and pass the `jobs_config`. The `jobs_config` is already destructured in `run` (from R8) but was moved into `scheduler::spawn`. Clone it before spawning the scheduler so the accept loop can hand a clone to each connection. Change the scheduler spawn line in `run` to:

```rust
        let scheduler = jobs::scheduler::spawn(store.clone(), jobs_config.clone());
```

and in the accept arm, where the per-connection locals are cloned, add a `jobs_config` clone and pass it into `handle_connection`. Change the accept block body:

```rust
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            let store = store.clone();
                            let embedder = embedder.clone();
                            let enricher = enricher.clone();
                            let jobs_config = jobs_config.clone();
                            let permit = match conn_sem.clone().try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => {
                                    warn!("connection cap ({MAX_CONNECTIONS}) reached; dropping connection");
                                    drop(stream);
                                    continue;
                                }
                            };
                            conns.spawn(async move {
                                let _permit = permit; // released when task completes
                                if let Err(e) =
                                    handle_connection(stream, store, embedder, enricher, jobs_config).await
                                {
                                    warn!(error = %e, "connection ended with error");
                                }
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "accept failed");
                        }
                    }
                }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rusty-brain evolve && cargo test -p rusty-brain jobs_config` (Expected: PASS) then `cargo build --workspace` (Expected: PASS — every `DaemonConfig` / `run_serve` / `dispatch` caller compiles).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings && cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/server.rs crates/rusty-brain/src/cli.rs crates/rusty-brain/src/run.rs crates/rusty-brain/src/serve.rs crates/rusty-brain/src/paths.rs && git commit -m "feat(rusty-brain): add evolve CLI trigger and daemon RunJob dispatch"` — Expected: one commit.

---

### Task R11: rb-daemon `tests/daemon_e2e.rs` — evolve through the live daemon

**Files:**
- Modify: `crates/rb-daemon/tests/daemon_e2e.rs`
- Test: `crates/rb-daemon/tests/daemon_e2e.rs`

A black-box test that proves the WIRE path end-to-end: `Client::run_job` → daemon `RunJob` dispatch → `Response::JobRan`. The existing harness is `RunningDaemon::start(pool_size) -> RunningDaemon` (with a `.socket: PathBuf` field and an async `.stop(self)`); clients connect via `Client::connect(&daemon.socket, ns)`. The widening of `DaemonConfig` in R8 also requires adding the new `jobs_config` field to every `DaemonConfig { ... }` literal in this file (there are four: in `RunningDaemon::start` and three more inside the bind-conflict tests).

- [ ] **Step 1 RED: write the failing test.** Append this test to `crates/rb-daemon/tests/daemon_e2e.rs`. An empty store yields an all-zero `JobRan`, which still proves the round-trip:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_job_link_decay_round_trips_over_the_wire() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("evolve-e2e".to_string());
    let mut client = Client::connect(&daemon.socket, ns).await.unwrap();

    let (scanned, changed, skipped) = client
        .run_job(rb_types::JobKind::LinkDecay)
        .await
        .unwrap();
    // Empty store: nothing to scan, but the wire op resolves to JobRan.
    assert_eq!((scanned, changed, skipped), (0, 0, 0));

    daemon.stop().await;
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon --test daemon_e2e run_job_link_decay` — Expected: FAIL — compile error: every `DaemonConfig { ... }` literal in this file is missing the `jobs_config` field added in R8.

- [ ] **Step 3 GREEN: minimal implementation.** Add the new field to all four `DaemonConfig { ... }` literals in `crates/rb-daemon/tests/daemon_e2e.rs`. The first is in `RunningDaemon::start`:

```rust
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: pool_size,
            jobs_config: rb_daemon::JobsConfig::default(),
        };
```

Apply the same `jobs_config: rb_daemon::JobsConfig::default(),` final field to the other three `DaemonConfig` literals in the file (the `cfg` / `cfg2` constructions in the socket-conflict and pidfile-lock tests near lines 296, 315, and 324). No production code change is needed — the wire path already works from R9/R10; this task closes e2e coverage and proves the harness compiles against the widened `DaemonConfig`.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon --test daemon_e2e` — Expected: PASS (all existing e2e tests plus the new `run_job` one; 0 failures).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/tests/daemon_e2e.rs && git commit -m "test(rb-daemon): cover evolve RunJob round-trip over the live daemon"` — Expected: one commit.

---

### Part R gate

**Files:** none (verification only).

- [ ] **Step 1: full workspace test.** Run: `cargo test --workspace` — Expected: PASS, 0 failures (all Part R unit + integration tests plus every pre-existing test, including the widened exhaustive round-trip tests in `rb-proto` and the `rb-mcp` match arms).

- [ ] **Step 2: workspace clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings (no `.unwrap()`/`.expect()`/`panic!`/`unreachable!` in non-test code; jobs code returns `rb_types::Error`).

- [ ] **Step 3: format check.** Run: `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 4: dependency policy (Part R adds `toml`).** Run: `cargo deny check` — Expected: `ok` (licenses + advisories + sources all clean; `toml` and its transitive deps are `MIT OR Apache-2.0`, already in the allowlist).


## Part S — consolidation (wire `supersede`; similarity-gated, namespace-isolated)

This Part wires the already-implemented-but-unused `SqliteStore::supersede` primitive into a bounded, idempotent, namespace-isolated consolidation job that merges near-duplicate memories. We add one read seam (`near_duplicates`, KNN over a memory's OWN stored vector, same-namespace, active, self-excluded, similarity-gated), one writer command (`Supersede`, publishing an `Archived` event for the absorbed duplicate so subscribers see the merge), and the `jobs/consolidation.rs` job that picks a deterministic survivor and supersedes every other member of a duplicate cluster. The job NEVER crosses namespaces (`near_duplicates` filters by namespace exactly like the proven `vector_search`) and is idempotent (a superseded/archived memory is skipped on the next pass, so a second run changes nothing). All tasks assume Part R's scaffolding (`JobKind`, `JobSummary`, `JobsConfig`, `ConsolidationConfig`, `run_once`, the scheduler, and the `RunJob` proto/daemon path) already exists; Part S adds ONLY the Consolidation arm and its store seams.

All commands run from the worktree root `/Volumes/raid1/repos/rusty-brain-p3`.

---

### Task S1: rb-store `store.rs` — near-duplicates read

Add a namespace-isolated near-duplicate read on `SqliteStore`. It loads the memory's own stored vector, runs the same vec0 KNN MATCH that `vector_search` already uses, and filters in Rust to the same namespace, active rows, excluding self, keeping only candidates whose cosine similarity (`1.0 - distance/2.0`, the exact convention `rb-search::rank` uses) is `>= threshold`. Results are sorted by similarity descending, then by id string for a deterministic tie-break, and truncated to `limit`.

**Files:**
- Modify: crates/rb-store/src/store.rs

- [ ] **Step 1 RED: add the failing test module.** Append this module to the end of `crates/rb-store/src/store.rs`:

```rust
#[cfg(test)]
mod near_duplicates_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_vec(
        store: &SqliteStore,
        ns: Namespace,
        content: &str,
        v: [f32; 8],
    ) -> rb_types::MemoryId {
        let m = MemoryNote::new(ns, content.into(), MemoryType::Insight, 5);
        let id = m.id.clone();
        store.insert_memory(&m, Some(&v)).unwrap();
        id
    }

    #[test]
    fn returns_same_namespace_twin_and_never_crosses_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj_a = Namespace::Project("a".into());
        let proj_b = Namespace::Project("b".into());

        // Anchor in A.
        let anchor = insert_vec(
            &store,
            proj_a.clone(),
            "anchor",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Near-identical twin in A (same direction => cosine distance ~0 => sim ~1).
        let twin = insert_vec(
            &store,
            proj_a.clone(),
            "twin",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // A clearly different vector in A (orthogonal => cosine distance ~1 => sim ~0.5).
        let _different = insert_vec(
            &store,
            proj_a.clone(),
            "different",
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Near-identical to anchor BUT in namespace B: must NEVER be returned.
        let foreign = insert_vec(
            &store,
            proj_b.clone(),
            "foreign twin",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let dups = store
            .near_duplicates(&proj_a, &anchor, 0.95, 10)
            .unwrap();
        let ids: Vec<rb_types::MemoryId> = dups.iter().map(|(id, _)| id.clone()).collect();

        assert!(ids.contains(&twin), "the same-namespace twin must be found");
        assert!(
            !ids.contains(&anchor),
            "the anchor itself must be excluded (self)"
        );
        assert!(
            !ids.contains(&foreign),
            "a near-identical memory in another namespace must NEVER be returned"
        );
        // The orthogonal vector has similarity ~0.5, well below the 0.95 threshold.
        assert_eq!(ids, vec![twin], "only the above-threshold twin is returned");
        // Reported similarity for an identical vector is at/near 1.0.
        assert!(
            dups[0].1 >= 0.95,
            "twin similarity must meet the threshold, got {}",
            dups[0].1
        );
    }

    #[test]
    fn missing_anchor_or_no_vector_returns_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("a".into());
        // Anchor id that does not exist at all.
        let ghost = rb_types::MemoryId::new();
        assert!(store
            .near_duplicates(&ns, &ghost, 0.95, 10)
            .unwrap()
            .is_empty());

        // A memory that exists but was inserted WITHOUT an embedding has no vector
        // row, so there is nothing to KNN against: empty, not an error.
        let no_vec = MemoryNote::new(ns.clone(), "no vector".into(), MemoryType::Insight, 5);
        let no_vec_id = no_vec.id.clone();
        store.insert_memory(&no_vec, None).unwrap();
        assert!(store
            .near_duplicates(&ns, &no_vec_id, 0.95, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn excludes_archived_candidates() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("a".into());
        let anchor = insert_vec(
            &store,
            ns.clone(),
            "anchor",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let twin = insert_vec(
            &store,
            ns.clone(),
            "twin",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        // Archive the twin: it must drop out of the candidate set.
        store.archive_memory(&twin).unwrap();
        let dups = store.near_duplicates(&ns, &anchor, 0.95, 10).unwrap();
        assert!(
            dups.is_empty(),
            "an archived candidate must not be returned, got {dups:?}"
        );
    }
}
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-store near_duplicates` — Expected: FAIL to COMPILE with `no method named near_duplicates found for struct SqliteStore`.

- [ ] **Step 3 GREEN: implement `near_duplicates` (and a byte-decoder helper).** First add this private helper immediately after the existing `embedding_bytes` function (after line `fn embedding_bytes(...) -> Vec<u8> { ... }`):

```rust
/// Decode a little-endian `f32` blob (the exact byte layout `embedding_bytes`
/// writes) back into a `Vec<f32>`. Fail closed if the length is not a multiple
/// of four bytes rather than silently truncating a corrupt vector.
fn decode_embedding_bytes(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % 4 != 0 {
        return Err(Error::Storage(format!(
            "stored embedding blob length {} is not a multiple of 4",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        // chunks_exact(4) yields slices of exactly 4 bytes; the conversion to a
        // [u8; 4] cannot fail, but we handle it explicitly to avoid unwrap.
        let arr: [u8; 4] = chunk
            .try_into()
            .map_err(|_| Error::Storage("embedding chunk was not 4 bytes".to_string()))?;
        out.push(f32::from_le_bytes(arr));
    }
    Ok(out)
}

/// Convert a vec0 cosine `distance` (range `[0, 2]`) into a similarity in
/// `[0, 1]`, matching the exact convention used by `rb-search::rank::score_one`
/// (`1.0 - (d / 2.0).clamp(0.0, 1.0)`). A non-finite distance yields `0.0`.
fn distance_to_similarity(distance: f32) -> f32 {
    if distance.is_finite() {
        1.0 - (distance / 2.0).clamp(0.0, 1.0)
    } else {
        0.0
    }
}
```

Then add the `near_duplicates` method inside the existing `impl SqliteStore { ... }` block (place it after `checkpoint_truncate`, before the closing brace of that `impl`):

```rust
    /// Find active memories in `ns` whose stored vector is near-identical to the
    /// vector of `id` (cosine similarity `>= threshold`), excluding `id` itself.
    ///
    /// Namespace-isolated by construction: candidates are filtered to `ns` and
    /// active (`archived_at IS NULL`) in Rust, exactly as `vector_search` does,
    /// so a near-identical memory in another namespace is NEVER returned. Reads
    /// the anchor's OWN stored embedding from `memory_vectors` and runs the same
    /// vec0 KNN MATCH the search path uses. A missing anchor or an anchor with no
    /// stored vector yields an empty result (not an error). Results are sorted by
    /// similarity descending, then id string ascending for a deterministic
    /// tie-break, and truncated to `limit`.
    pub fn near_duplicates(
        &self,
        ns: &Namespace,
        id: &MemoryId,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        // Load the anchor's stored embedding blob. No row => nothing to compare.
        let blob: Option<Vec<u8>> = match self.conn.query_row(
            "SELECT embedding FROM memory_vectors WHERE memory_id = ?1",
            rusqlite::params![id.to_string()],
            |row| row.get::<_, Vec<u8>>(0),
        ) {
            Ok(b) => Some(b),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(Error::Storage(e.to_string())),
        };
        let Some(blob) = blob else {
            return Ok(Vec::new());
        };
        let anchor_vec = decode_embedding_bytes(&blob)?;
        // Defense-in-depth: a stored vector must match the configured dimension.
        if anchor_vec.len() != self.embedding_dim {
            return Err(Error::DimensionMismatch {
                expected: self.embedding_dim,
                got: anchor_vec.len(),
            });
        }

        // sqlite-vec accepts the query vector as a JSON array string (same as
        // vector_search). vec0 cannot filter on namespace/active inside KNN, so
        // we over-fetch a candidate pool and filter in Rust.
        let query_json = serde_json::to_string(&anchor_vec)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        const VEC0_KNN_MAX: i64 = 4096;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        // +1 because the anchor itself is the nearest (distance 0) and is then
        // dropped by the self-exclusion filter below.
        let k_budget = limit_i64
            .saturating_add(1)
            .saturating_mul(10)
            .max(limit_i64)
            .min(VEC0_KNN_MAX);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, distance
                 FROM memory_vectors
                 WHERE embedding MATCH ?1
                 ORDER BY distance
                 LIMIT ?2",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![query_json, k_budget], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| Error::Storage(e.to_string()))?;

        let ns_str = ns.as_db_string();
        let self_str = id.to_string();
        let mut out: Vec<(MemoryId, f32)> = Vec::new();
        for r in rows {
            let (cand_str, dist) = r.map_err(|e| Error::Storage(e.to_string()))?;

            // Exclude self.
            if cand_str == self_str {
                continue;
            }

            // Namespace + active filter, fail closed on any non-"no rows" error.
            let active: bool = match self.conn.query_row(
                "SELECT 1 FROM memories WHERE memory_id = ?1 AND namespace = ?2 AND archived_at IS NULL",
                rusqlite::params![cand_str, ns_str],
                |_| Ok(true),
            ) {
                Ok(found) => found,
                Err(rusqlite::Error::QueryReturnedNoRows) => false,
                Err(e) => return Err(Error::Storage(e.to_string())),
            };
            if !active {
                continue;
            }

            let similarity = distance_to_similarity(dist as f32);
            if similarity >= threshold {
                out.push((parse_id(&cand_str)?, similarity));
            }
        }

        // Deterministic order: similarity descending, then id string ascending.
        out.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        out.truncate(limit);
        Ok(out)
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-store near_duplicates` — Expected: PASS (3 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-store/src/store.rs && git commit -m "feat(rb-store): add namespace-isolated near_duplicates knn read"` — Expected: one commit.

---

### Task S2: rb-daemon `store_handle.rs` — near-duplicates read path

Expose `near_duplicates` through the `StoreHandle` read pool so the consolidation job can read candidates the same way every other read flows. This is a thin async wrapper over `with_read`, mirroring the existing `vector`/`list` read methods.

**Files:**
- Modify: crates/rb-daemon/src/store_handle.rs

- [ ] **Step 1 RED: add the failing test.** Add this test to the existing `mod tests` block at the bottom of `crates/rb-daemon/src/store_handle.rs` (inside the `#[cfg(test)] mod tests { ... }`, after the last test):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_near_duplicates_is_namespace_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns_a = Namespace::Project("a".to_string());
        let ns_b = Namespace::Project("b".to_string());

        // Anchor + near-identical twin in A.
        let mut anchor = note(&ns_a, "anchor");
        anchor.id = rb_types::MemoryId::new();
        let anchor_id = anchor.id.clone();
        handle
            .write(anchor, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let twin = note(&ns_a, "twin");
        let twin_id = twin.id.clone();
        handle
            .write(twin, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // A near-identical memory in namespace B that must never be returned.
        let foreign = note(&ns_b, "foreign");
        let foreign_id = foreign.id.clone();
        handle
            .write(foreign, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let dups = handle
            .near_duplicates(ns_a.clone(), anchor_id.clone(), 0.95, 10)
            .await
            .unwrap();
        let ids: Vec<rb_types::MemoryId> = dups.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids, vec![twin_id], "only the same-namespace twin is returned");
        assert!(!ids.contains(&anchor_id), "anchor excluded (self)");
        assert!(!ids.contains(&foreign_id), "ns-B memory never crosses over");

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon near_duplicates` — Expected: FAIL to COMPILE with `no method named near_duplicates found for struct StoreHandle`.

- [ ] **Step 3 GREEN: add the read wrapper.** Add this method inside the `impl StoreHandle { ... }` block (place it after `read_pool_len_for_test`, before the closing brace of that `impl`):

```rust
    /// Read near-duplicate candidates for `id` within `ns` via the read pool.
    /// Namespace-isolated (see `SqliteStore::near_duplicates`); used by the
    /// consolidation job to find merge candidates without crossing namespaces.
    pub async fn near_duplicates(
        &self,
        ns: Namespace,
        id: MemoryId,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        self.with_read(move |store| store.near_duplicates(&ns, &id, threshold, limit))
            .await
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon near_duplicates` — Expected: PASS (1 test).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/store_handle.rs && git commit -m "feat(rb-daemon): expose near_duplicates through the read pool"` — Expected: one commit.

---

### Task S3: rb-daemon `store_handle.rs` — supersede writer command

Wire the existing `SqliteStore::supersede` into the single-writer path: add `WriteCommand::Supersede`, a `StoreHandle::supersede` method, and a writer-loop arm that runs through the panic-contained `run_store_op` and — mirroring the `Archive` arm — publishes a `MemoryChanged { kind: Archived }` event for the absorbed `old` memory so subscribers observe the consolidation.

**Files:**
- Modify: crates/rb-daemon/src/store_handle.rs

- [ ] **Step 1 RED: add the failing tests.** Add these three tests to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/rb-daemon/src/store_handle.rs` (after the test added in Task S2):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_archives_old_and_sets_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old fact");
        let new = note(&ns, "new fact");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        handle.supersede(old_id.clone(), new_id.clone()).await.unwrap();

        // `old` is archived and points at `new`; `new` is untouched.
        let got_old = handle.get(ns.clone(), old_id.clone()).await.unwrap().unwrap();
        assert_eq!(got_old.superseded_by.as_ref(), Some(&new_id));
        assert!(got_old.archived_at.is_some(), "old must be archived");
        let got_new = handle.get(ns.clone(), new_id.clone()).await.unwrap().unwrap();
        assert!(got_new.superseded_by.is_none());
        assert!(got_new.archived_at.is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_missing_new_target_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old only");
        let old_id = old.id.clone();
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();

        // `new` does not exist => FK violation => storage error; old unchanged.
        let err = handle
            .supersede(old_id.clone(), rb_types::MemoryId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");
        let got_old = handle.get(ns.clone(), old_id.clone()).await.unwrap().unwrap();
        assert!(got_old.superseded_by.is_none(), "rolled back: no pointer");
        assert!(got_old.archived_at.is_none(), "rolled back: not archived");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_publishes_archived_event_for_old() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old fact");
        let new = note(&ns, "new fact");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        // Subscribe BEFORE the supersede so we observe its event.
        let mut rx = handle.subscribe();
        handle.supersede(old_id.clone(), new_id.clone()).await.unwrap();

        // The next event delivered must be the Archived event for `old`.
        let evt = rx.recv().await.unwrap();
        assert_eq!(evt.id, old_id, "Archived event must target the old memory");
        assert_eq!(evt.namespace, ns);
        assert_eq!(
            evt.kind,
            crate::change::ChangeKind::Archived,
            "supersede must publish an Archived event for the absorbed memory"
        );

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon supersede` — Expected: FAIL to COMPILE with `no method named supersede found for struct StoreHandle`.

- [ ] **Step 3 GREEN: add the command, the arm, and the method.** Three edits in `crates/rb-daemon/src/store_handle.rs`.

(a) Add a `Supersede` variant to the `enum WriteCommand` (insert it immediately after the `RecordAccesses { ... }` variant, before the `#[cfg(test)] PanicForTest` variant):

```rust
    Supersede {
        namespace: Namespace,
        old: MemoryId,
        new: MemoryId,
        reply: oneshot::Sender<Result<()>>,
    },
```

(b) Add the writer-loop arm. In `fn writer_loop`, inside the `match cmd { ... }`, insert this arm immediately after the `WriteCommand::RecordAccesses { ... }` arm (before the `#[cfg(test)] WriteCommand::PanicForTest` arm):

```rust
            WriteCommand::Supersede {
                namespace,
                old,
                new,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.supersede(&old, &new)
                });
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                // supersede archives `old`; mirror the Archive arm so subscribers
                // observe the consolidation as an Archived event for `old`.
                if changed {
                    publish_change(
                        &events,
                        MemoryChanged {
                            id: old,
                            namespace,
                            kind: ChangeKind::Archived,
                        },
                    );
                }
                if !writer_usable {
                    break;
                }
            }
```

(c) Add the public method. Inside `impl StoreHandle { ... }`, place it immediately after the `near_duplicates` method added in Task S2 (before the closing brace of that `impl`):

```rust
    /// Supersede `old` with `new`: archive `old` and point it at `new`, through
    /// the single writer. The `namespace` is carried only for the published
    /// `Archived` change event; the FK-guarded SQL keys on ids. Fails closed
    /// (rolls back) if `new` does not exist.
    pub async fn supersede(&self, namespace: Namespace, old: MemoryId, new: MemoryId) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Supersede {
            namespace,
            old,
            new,
            reply,
        };
        self.send_write(cmd, rx).await
    }
```

NOTE: the three new supersede tests call `handle.supersede(old_id, new_id)` with TWO args, but the method takes three (`namespace, old, new`). Fix the three tests added in Step 1 to pass the namespace as the first argument — replace each `handle.supersede(old_id.clone(), new_id.clone())` with `handle.supersede(ns.clone(), old_id.clone(), new_id.clone())` and the missing-target test's `handle.supersede(old_id.clone(), rb_types::MemoryId::new())` with `handle.supersede(ns.clone(), old_id.clone(), rb_types::MemoryId::new())`. The corrected test bodies are:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_archives_old_and_sets_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old fact");
        let new = note(&ns, "new fact");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        handle
            .supersede(ns.clone(), old_id.clone(), new_id.clone())
            .await
            .unwrap();

        let got_old = handle.get(ns.clone(), old_id.clone()).await.unwrap().unwrap();
        assert_eq!(got_old.superseded_by.as_ref(), Some(&new_id));
        assert!(got_old.archived_at.is_some(), "old must be archived");
        let got_new = handle.get(ns.clone(), new_id.clone()).await.unwrap().unwrap();
        assert!(got_new.superseded_by.is_none());
        assert!(got_new.archived_at.is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_missing_new_target_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old only");
        let old_id = old.id.clone();
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();

        let err = handle
            .supersede(ns.clone(), old_id.clone(), rb_types::MemoryId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");
        let got_old = handle.get(ns.clone(), old_id.clone()).await.unwrap().unwrap();
        assert!(got_old.superseded_by.is_none(), "rolled back: no pointer");
        assert!(got_old.archived_at.is_none(), "rolled back: not archived");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_publishes_archived_event_for_old() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old fact");
        let new = note(&ns, "new fact");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        let mut rx = handle.subscribe();
        handle
            .supersede(ns.clone(), old_id.clone(), new_id.clone())
            .await
            .unwrap();

        let evt = rx.recv().await.unwrap();
        assert_eq!(evt.id, old_id, "Archived event must target the old memory");
        assert_eq!(evt.namespace, ns);
        assert_eq!(
            evt.kind,
            crate::change::ChangeKind::Archived,
            "supersede must publish an Archived event for the absorbed memory"
        );

        handle.shutdown().await;
    }
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon supersede` — Expected: PASS (3 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/store_handle.rs && git commit -m "feat(rb-daemon): wire supersede writer command with archived event"` — Expected: one commit.

---

### Task S4: rb-daemon `jobs/consolidation.rs` — survivor picker (pure)

Factor the deterministic survivor selection into a PURE function `pick_survivor(&[MemoryMeta]) -> MemoryId`, with unit tests for every tiebreak (importance, then newest `created_at`, then lexicographically-smallest id). This is the consolidation policy in isolation, fully testable without a store.

**Files:**
- Create: crates/rb-daemon/src/jobs/consolidation.rs (the `MemoryMeta` struct + `pick_survivor` half; the `run` job is added in Task S5)
- Modify: crates/rb-daemon/src/jobs/mod.rs (declare `mod consolidation;` — Part R created this file; add the line if absent)

- [ ] **Step 1 RED: create the file with `pick_survivor` tests but no implementation.** Create `crates/rb-daemon/src/jobs/consolidation.rs` with exactly this content:

```rust
//! Consolidation job: merge near-duplicate memories by superseding every
//! duplicate of a cluster into a single deterministically-chosen survivor.
//! Bounded, idempotent, and namespace-isolated (see `run`).

use rb_types::MemoryId;

/// The minimal metadata the survivor policy needs. Kept tiny so `pick_survivor`
/// is a pure function over plain data, independent of the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMeta {
    pub id: MemoryId,
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Choose the survivor of a duplicate cluster deterministically.
///
/// Order of preference (each later key only breaks ties of all earlier keys):
/// 1. highest `importance`,
/// 2. newest `created_at`,
/// 3. lexicographically-smallest id string (total, stable final tiebreak).
///
/// Returns the chosen id. `candidates` must be non-empty; an empty slice is a
/// caller bug and yields `None` so the caller can skip the cluster rather than
/// panic.
pub fn pick_survivor(candidates: &[MemoryMeta]) -> Option<MemoryId> {
    candidates
        .iter()
        .max_by(|a, b| {
            a.importance
                .cmp(&b.importance)
                .then_with(|| a.created_at.cmp(&b.created_at))
                // For the FINAL tiebreak we want the SMALLEST id to win. `max_by`
                // returns the greatest element, so invert the id comparison:
                // a "greater" element here is the one with the smaller id.
                .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
        })
        .map(|m| m.id.clone())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::MemoryId;

    fn meta(importance: u8, created_secs: i64) -> MemoryMeta {
        MemoryMeta {
            id: MemoryId::new(),
            importance,
            created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(created_secs, 0)
                .expect("valid timestamp"),
        }
    }

    #[test]
    fn empty_cluster_returns_none() {
        assert!(pick_survivor(&[]).is_none());
    }

    #[test]
    fn single_candidate_is_the_survivor() {
        let only = meta(5, 100);
        assert_eq!(pick_survivor(&[only.clone()]), Some(only.id));
    }

    #[test]
    fn higher_importance_wins() {
        // b has higher importance even though a is newer.
        let a = meta(3, 200);
        let b = meta(9, 100);
        assert_eq!(
            pick_survivor(&[a, b.clone()]),
            Some(b.id),
            "highest importance must win regardless of recency"
        );
    }

    #[test]
    fn equal_importance_newest_created_wins() {
        // Same importance; b is newer (larger created_at) -> b wins.
        let a = meta(7, 100);
        let b = meta(7, 500);
        assert_eq!(
            pick_survivor(&[a, b.clone()]),
            Some(b.id),
            "with equal importance the newest created_at wins"
        );
    }

    #[test]
    fn equal_importance_and_time_smallest_id_wins() {
        // Identical importance + created_at: the lexicographically-smallest id wins.
        let ts = 300;
        let one = meta(5, ts);
        let two = meta(5, ts);
        let mut both = vec![one.clone(), two.clone()];
        let expected = if one.id.to_string() < two.id.to_string() {
            one.id.clone()
        } else {
            two.id.clone()
        };
        assert_eq!(pick_survivor(&both), Some(expected.clone()));
        // Order-independence: reversing the input yields the SAME survivor.
        both.reverse();
        assert_eq!(
            pick_survivor(&both),
            Some(expected),
            "survivor must not depend on input order"
        );
    }
}
```

Then ensure `crates/rb-daemon/src/jobs/mod.rs` declares the module. Part R created `jobs/mod.rs`; confirm it contains the line `mod consolidation;` (Part R's contract lists `consolidation.rs` in the module tree). If it is not present, add it alongside the other job module declarations:

```rust
mod consolidation;
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon pick_survivor` — Expected: FAIL to COMPILE — the test file references `pick_survivor`/`MemoryMeta` which now exist, so this should COMPILE and PASS; if `mod consolidation;` was missing from `mod.rs`, the failure is `file not found for module` / unresolved import until the `mod consolidation;` line is added. After adding it: Expected PASS.

- [ ] **Step 3 GREEN: (implementation already written in Step 1).** The `pick_survivor` body above is the minimal real implementation — no placeholder. Confirm `jobs/mod.rs` has `mod consolidation;`.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon consolidation::tests` — Expected: PASS (5 tests).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/jobs/consolidation.rs crates/rb-daemon/src/jobs/mod.rs && git commit -m "feat(rb-daemon): add deterministic pick_survivor for consolidation"` — Expected: one commit.

---

### Task S5: rb-daemon `jobs/consolidation.rs` — bounded idempotent run + run_once arm

Implement `run(store, &ConsolidationConfig) -> JobSummary`: scan active, non-superseded memories per namespace up to `batch_limit`; for each not-yet-consumed memory find its near-duplicates within its OWN namespace; pick a survivor over the cluster; supersede every other member; mark members consumed so they are never revisited (idempotency). Wire it into `run_once`'s `JobKind::Consolidation` arm. The run NEVER merges across namespaces because every candidate and its `near_duplicates` lookup is namespace-scoped.

**Files:**
- Modify: crates/rb-daemon/src/jobs/consolidation.rs
- Modify: crates/rb-daemon/src/jobs/mod.rs (route `JobKind::Consolidation => consolidation::run(...)` inside `run_once`)
- Modify: crates/rb-daemon/src/store_handle.rs (add a read helper `candidates_for_consolidation` the job uses to enumerate active, non-superseded notes per batch)

- [ ] **Step 1 RED: add the run tests.** First add a read helper test to the `#[cfg(test)] mod tests` block in `crates/rb-daemon/src/store_handle.rs` (after the supersede tests):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn candidates_for_consolidation_lists_active_non_superseded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("scan".to_string());

        let keep = note(&ns, "active");
        let archived = note(&ns, "archived");
        let new = note(&ns, "survivor");
        let (keep_id, archived_id, new_id) = (keep.id.clone(), archived.id.clone(), new.id.clone());
        handle.write(keep, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(archived, Some(vec![0.2f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.3f32; DIM])).await.unwrap();

        // Supersede `archived` into `new`: it becomes archived + superseded and
        // must drop out of the candidate enumeration.
        handle
            .supersede(ns.clone(), archived_id.clone(), new_id.clone())
            .await
            .unwrap();

        let cands = handle.candidates_for_consolidation(100).await.unwrap();
        let ids: Vec<rb_types::MemoryId> = cands.iter().map(|c| c.id.clone()).collect();
        assert!(ids.contains(&keep_id), "active memory present");
        assert!(ids.contains(&new_id), "survivor present");
        assert!(
            !ids.contains(&archived_id),
            "archived/superseded memory must be excluded"
        );
        // Every returned candidate carries its namespace for per-ns grouping.
        assert!(cands.iter().all(|c| c.namespace == ns));

        handle.shutdown().await;
    }
```

Then add the consolidation `run` tests to `crates/rb-daemon/src/jobs/consolidation.rs` by REPLACING its `#[cfg(test)] mod tests { ... }` block's closing brace region — specifically, add these tests INSIDE the existing `mod tests` (after `equal_importance_and_time_smallest_id_wins`):

```rust
    use crate::jobs::config::ConsolidationConfig;
    use crate::StoreHandle;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    const DIM: usize = 8;

    fn vnote(ns: &Namespace, body: &str, importance: u8) -> MemoryNote {
        MemoryNote::new(ns.clone(), body.to_string(), MemoryType::Insight, importance)
    }

    fn cfg(threshold: f32, batch_limit: usize) -> ConsolidationConfig {
        ConsolidationConfig {
            enabled: true,
            interval_secs: 86_400,
            similarity_threshold: threshold,
            batch_limit,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn merges_twins_in_same_namespace_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns_a = Namespace::Project("a".to_string());
        let ns_b = Namespace::Project("b".to_string());

        // Two near-identical memories in A (the survivor has higher importance).
        let survivor = vnote(&ns_a, "twin survivor", 9);
        let dup = vnote(&ns_a, "twin dup", 3);
        let (survivor_id, dup_id) = (survivor.id.clone(), dup.id.clone());
        handle
            .write(survivor, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(dup, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // A near-identical memory in B: must NOT be merged with A's cluster.
        let foreign = vnote(&ns_b, "foreign twin", 9);
        let foreign_id = foreign.id.clone();
        handle
            .write(foreign, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let summary = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(summary.changed, 1, "exactly one duplicate superseded");

        // The lower-importance dup is archived and points at the survivor.
        let got_dup = handle.get(ns_a.clone(), dup_id.clone()).await.unwrap().unwrap();
        assert!(got_dup.archived_at.is_some(), "dup archived");
        assert_eq!(got_dup.superseded_by.as_ref(), Some(&survivor_id));

        // The survivor stays active and is NOT superseded.
        let got_survivor = handle.get(ns_a.clone(), survivor_id.clone()).await.unwrap().unwrap();
        assert!(got_survivor.archived_at.is_none());
        assert!(got_survivor.superseded_by.is_none());

        // The foreign memory in B is completely untouched (namespace isolation).
        let got_foreign = handle.get(ns_b.clone(), foreign_id.clone()).await.unwrap().unwrap();
        assert!(got_foreign.archived_at.is_none(), "ns-B memory never merged");
        assert!(got_foreign.superseded_by.is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn is_idempotent_second_run_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        let a = vnote(&ns, "twin a", 9);
        let b = vnote(&ns, "twin b", 3);
        handle
            .write(a, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(b, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let first = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(first.changed, 1, "first run merges the duplicate");

        // Second run: the duplicate is now archived/superseded and excluded, so
        // there is nothing left to merge.
        let second = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(second.changed, 0, "second run must be a no-op (idempotent)");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn distinct_memories_are_not_merged_and_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        // Two orthogonal vectors: similarity ~0.5, far below 0.95 threshold.
        let x = vnote(&ns, "topic x", 5);
        let y = vnote(&ns, "topic y", 5);
        let (x_id, y_id) = (x.id.clone(), y.id.clone());
        handle
            .write(x, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(y, Some(vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        let summary = run(&handle, &cfg(0.95, 200)).await.unwrap();
        assert_eq!(summary.changed, 0, "distinct memories are never merged");
        assert!(summary.skipped >= 2, "both memories counted as skipped (no dup)");

        // Both remain active.
        assert!(handle
            .get(ns.clone(), x_id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_none());
        assert!(handle
            .get(ns.clone(), y_id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_consolidation_arm_returns_summary() {
        use crate::jobs::{run_once, JobKind, JobsConfig};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        let a = vnote(&ns, "twin a", 9);
        let b = vnote(&ns, "twin b", 3);
        handle
            .write(a, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        handle
            .write(b, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // Drive through the shared run_once entry point with the Consolidation kind.
        let config = JobsConfig {
            consolidation: cfg(0.95, 200),
            ..Default::default()
        };
        let summary = run_once(JobKind::Consolidation, &handle, &config)
            .await
            .unwrap();
        assert_eq!(
            summary.changed, 1,
            "run_once(Consolidation) must merge via the same path"
        );

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon consolidation` — Expected: FAIL to COMPILE: `no function or associated item named run` and `no method named candidates_for_consolidation found for struct StoreHandle`.

- [ ] **Step 3 GREEN: implement the read helper, the `run` job, and the `run_once` arm.**

(a) Add the read helper to `crates/rb-daemon/src/store_handle.rs`. Inside `impl StoreHandle { ... }`, after the `supersede` method, add:

```rust
    /// Enumerate up to `limit` active, non-superseded memories across ALL
    /// namespaces, oldest first then by id, for the consolidation job to scan.
    /// Each candidate carries the id/namespace/importance/created_at the job and
    /// its survivor policy need. Reads via the pool.
    pub async fn candidates_for_consolidation(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::jobs::consolidation::Candidate>> {
        self.with_read(move |store| store.candidates_for_consolidation(limit))
            .await
    }
```

(b) Add the backing store read to `crates/rb-store/src/store.rs`. The job needs id, namespace, importance, created_at for active, non-superseded rows. Add a small row struct and a method on `SqliteStore`. Add this struct just above `impl SqliteStore` (near the other free items), and the method inside the `impl SqliteStore { ... }` block after `near_duplicates`:

```rust
/// A minimal projection of a memory row for the consolidation scan: only the
/// fields the job and its survivor policy need. Avoids loading full notes/links.
#[derive(Clone, Debug)]
pub struct ConsolidationCandidate {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

```rust
    /// Active (`archived_at IS NULL`), non-superseded (`superseded_by IS NULL`)
    /// memories, oldest first then by id, capped at `limit`. The deterministic
    /// ORDER BY makes a consolidation pass reproducible.
    pub fn candidates_for_consolidation(
        &self,
        limit: usize,
    ) -> Result<Vec<ConsolidationCandidate>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, namespace, importance, created_at
                 FROM memories
                 WHERE archived_at IS NULL AND superseded_by IS NULL
                 ORDER BY created_at ASC, memory_id ASC
                 LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(
                rusqlite::params![i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            let (id_str, ns_str, importance, created) =
                r.map_err(|e| Error::Storage(e.to_string()))?;
            out.push(ConsolidationCandidate {
                id: parse_id(&id_str)?,
                namespace: Namespace::parse_db_string(&ns_str)?,
                importance: u8::try_from(importance)
                    .map_err(|_| Error::Storage(format!("importance {importance} out of u8 range")))?,
                created_at: from_ts(created)?,
            });
        }
        Ok(out)
    }
```

Re-export the new struct from `crates/rb-store/src/lib.rs` by changing the store re-export line:

```rust
pub use store::{ConsolidationCandidate, SqliteStore, Store};
```

(c) In `crates/rb-daemon/src/jobs/consolidation.rs`, define the daemon-facing `Candidate` alias and the `run` job. The daemon read helper returns `crate::jobs::consolidation::Candidate`; make it a re-export of the store type so there is one shape. Add this near the top of `consolidation.rs`, after the `use rb_types::MemoryId;` line:

```rust
/// The consolidation scan row, re-exported from the store so the daemon read
/// helper and the job share exactly one type.
pub use rb_store::ConsolidationCandidate as Candidate;

use crate::StoreHandle;
use rb_types::Result;
use std::collections::HashSet;
```

Then add the `run` function (place it after `pick_survivor`, before the `#[cfg(test)]` module):

```rust
/// Run ONE bounded, idempotent, namespace-isolated consolidation pass.
///
/// Algorithm: read up to `batch_limit` active, non-superseded candidates
/// (deterministic order). For each candidate `m` not already consumed by an
/// earlier cluster, look up its near-duplicates WITHIN m's own namespace; if it
/// has none, it is `skipped`. Otherwise form the cluster `{m} ∪ dups`, pick a
/// deterministic survivor, and supersede every OTHER member into the survivor,
/// marking each consumed so it is never revisited. `scanned` counts candidates
/// examined; `changed` counts supersede writes; `skipped` counts candidates with
/// no duplicate.
///
/// Idempotency: a superseded/archived member is excluded from the candidate scan
/// AND from `near_duplicates` (both filter `archived_at IS NULL`), so a second
/// run over unchanged data performs zero writes. Never merges across namespaces:
/// `near_duplicates` is namespace-scoped, so a cluster only ever contains
/// same-namespace members.
pub async fn run(store: &StoreHandle, config: &ConsolidationConfig) -> Result<JobSummary> {
    let candidates = store
        .candidates_for_consolidation(config.batch_limit)
        .await?;

    let mut summary = JobSummary::default();
    let mut consumed: HashSet<String> = HashSet::new();

    for cand in &candidates {
        summary.scanned += 1;

        // Already absorbed into an earlier cluster this pass: skip silently.
        if consumed.contains(&cand.id.to_string()) {
            continue;
        }

        // Same-namespace near-duplicates only (namespace isolation guaranteed by
        // the store read). `limit` is the batch budget — a cluster cannot exceed
        // the scan size.
        let dups = store
            .near_duplicates(
                cand.namespace.clone(),
                cand.id.clone(),
                config.similarity_threshold,
                config.batch_limit,
            )
            .await?;

        // Drop any dup already consumed this pass (it belongs to an earlier
        // cluster); also drop the anchor if it somehow appears.
        let live_dups: Vec<Candidate> = dups
            .into_iter()
            .filter(|(dup_id, _)| {
                dup_id != &cand.id && !consumed.contains(&dup_id.to_string())
            })
            .filter_map(|(dup_id, _)| {
                candidates
                    .iter()
                    .find(|c| c.id == dup_id)
                    .cloned()
            })
            .collect();

        if live_dups.is_empty() {
            summary.skipped += 1;
            continue;
        }

        // Build the cluster metadata and choose the survivor deterministically.
        let mut cluster: Vec<MemoryMeta> = Vec::with_capacity(live_dups.len() + 1);
        cluster.push(MemoryMeta {
            id: cand.id.clone(),
            importance: cand.importance,
            created_at: cand.created_at,
        });
        for d in &live_dups {
            cluster.push(MemoryMeta {
                id: d.id.clone(),
                importance: d.importance,
                created_at: d.created_at,
            });
        }
        let Some(survivor_id) = pick_survivor(&cluster) else {
            // Cluster is non-empty here, so this is unreachable in practice; skip
            // defensively rather than panic.
            summary.skipped += 1;
            continue;
        };

        // Supersede every member that is not the survivor into the survivor.
        for member in &cluster {
            if member.id == survivor_id {
                continue;
            }
            store
                .supersede(
                    cand.namespace.clone(),
                    member.id.clone(),
                    survivor_id.clone(),
                )
                .await?;
            summary.changed += 1;
            consumed.insert(member.id.to_string());
        }
        // The survivor (and the anchor, if it survived) is consumed so it is not
        // re-clustered as a fresh anchor later in the same pass.
        consumed.insert(survivor_id.to_string());
        consumed.insert(cand.id.to_string());
    }

    Ok(summary)
}
```

Add the imports `run` needs to the top-of-file `use` block (merge with the `use` added above). The complete top-of-file use section of `consolidation.rs` must be:

```rust
use crate::jobs::config::ConsolidationConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;
use rb_types::{MemoryId, Result};
use std::collections::HashSet;

/// The consolidation scan row, re-exported from the store so the daemon read
/// helper and the job share exactly one type.
pub use rb_store::ConsolidationCandidate as Candidate;
```

(d) Wire the arm in `crates/rb-daemon/src/jobs/mod.rs`. Part R's `run_once` matches `kind`; add/confirm the Consolidation arm dispatches to `consolidation::run`. The relevant match must read (add the `JobKind::Consolidation` arm if Part R left it as a placeholder):

```rust
        JobKind::Consolidation => consolidation::run(store, &config.consolidation).await,
```

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon consolidation` then `cargo test -p rb-daemon candidates_for_consolidation` and `cargo test -p rb-store candidates_for_consolidation` — Expected: PASS (consolidation: 5 unit + 4 integration; store helper: 1; handle helper: 1).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` and `cargo clippy -p rb-store --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-store/src/store.rs crates/rb-store/src/lib.rs crates/rb-daemon/src/store_handle.rs crates/rb-daemon/src/jobs/consolidation.rs crates/rb-daemon/src/jobs/mod.rs && git commit -m "feat(rb-daemon): bounded idempotent consolidation run wired into run_once"` — Expected: one commit.

---

### Task S6: rb-daemon `server.rs` — RunJob consolidation reaches the daemon

Confirm `JobKind::Consolidation` flows end-to-end through the daemon's `RunJob` dispatch (added in Part R) with NO new wiring beyond the `run_once` arm shipped in Task S5. This test exercises the daemon-side dispatch arm directly via `jobs::run_once` against a real `StoreHandle`, proving the `evolve consolidation` / `Request::RunJob { job: JobKind::Consolidation }` path returns a populated `JobSummary` (the CLI/proto plumbing itself is Part R's tested surface; here we lock the Consolidation behavior at the dispatch seam Part S owns).

**Files:**
- Modify: crates/rb-daemon/src/server.rs

- [ ] **Step 1 RED: add the failing test.** Add this test to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/rb-daemon/src/server.rs` (after the last test):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_job_consolidation_merges_via_store_handle() {
        use crate::jobs::{run_once, JobKind, JobsConfig};
        use crate::StoreHandle;
        use rb_types::{MemoryNote, MemoryType, Namespace};

        const DIM: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        // The RunJob dispatch arm operates on a StoreHandle clone (jobs are
        // cross-namespace maintenance, not engine-bound); build one directly.
        let store = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        let mut a = MemoryNote::new(ns.clone(), "twin a".to_string(), MemoryType::Insight, 9);
        a.id = rb_types::MemoryId::new();
        let mut b = MemoryNote::new(ns.clone(), "twin b".to_string(), MemoryType::Insight, 3);
        b.id = rb_types::MemoryId::new();
        store
            .write(a, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        store
            .write(b, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // Defaults disable the job, but run_once runs ONE pass on demand
        // regardless of `enabled` (enabled only gates the scheduler). Provide an
        // explicit consolidation config so the threshold is the documented 0.95.
        let config = JobsConfig {
            consolidation: crate::jobs::ConsolidationConfig {
                enabled: true,
                interval_secs: 86_400,
                similarity_threshold: 0.95,
                batch_limit: 200,
            },
            ..Default::default()
        };

        let summary = run_once(JobKind::Consolidation, &store, &config)
            .await
            .unwrap();
        assert_eq!(
            summary.changed, 1,
            "the RunJob(Consolidation) path must merge the duplicate"
        );

        store.shutdown().await;
    }
```

NOTE: this test references `crate::jobs::ConsolidationConfig`; ensure `jobs/mod.rs` (Part R) re-exports it: the contract's `pub use jobs::{JobKind, JobSummary, JobsConfig, run_once};` in `lib.rs` plus `pub use config::{ConsolidationConfig, ...};` inside `jobs/mod.rs` makes `crate::jobs::ConsolidationConfig` resolvable. If `jobs/mod.rs` does not yet re-export `ConsolidationConfig`, add `pub use config::ConsolidationConfig;` to it.

- [ ] **Step 2: run it.** Run: `cargo test -p rb-daemon run_job_consolidation` — Expected: FAIL if `crate::jobs::ConsolidationConfig` is not re-exported (unresolved import); otherwise it COMPILES and PASSES once Tasks S1-S5 are merged. The intended RED state before S5 is "run_once Consolidation arm returns scanned/skipped only, changed=0" -> assertion fails. After S5: PASS.

- [ ] **Step 3 GREEN: ensure the re-export.** If absent, add to `crates/rb-daemon/src/jobs/mod.rs`:

```rust
pub use config::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
```

No other production change is needed: the Consolidation behavior was implemented in Task S5; this task only confirms it reaches the daemon dispatch seam.

- [ ] **Step 4: run it.** Run: `cargo test -p rb-daemon run_job_consolidation` — Expected: PASS (1 test).

- [ ] **Step 5: lint+format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit.** Run: `git add crates/rb-daemon/src/server.rs crates/rb-daemon/src/jobs/mod.rs && git commit -m "test(rb-daemon): confirm run_job consolidation merges through the dispatch seam"` — Expected: one commit.

---

### Part S gate

Run the full workspace gates and confirm green. Part S adds no new dependencies (it reuses `serde`, `chrono`, `rusqlite`, `tempfile` already present), so `cargo deny check` is not required for this Part.

- [ ] **Step 1: workspace tests.** Run: `cargo test --workspace` — Expected: PASS, 0 failures.
- [ ] **Step 2: workspace clippy.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.
- [ ] **Step 3: format check.** Run: `cargo fmt --all --check` — Expected: no diff.
- [ ] **Step 4: commit (only if any fix was needed to pass the gate).** Run: `git add -A && git commit -m "chore: part S gate green"` — Expected: one commit (skip if the working tree is already clean).


## Part T — importance recalibration (consume access_count / last_accessed_at)

This Part makes the long-write-only `access_count` / `last_accessed_at` columns finally *do* something: a bounded, idempotent maintenance pass recomputes each active memory's `importance` from how often and how recently it was accessed, clamped to the validated `1..=10` range. It consumes Part R's evolution scaffolding exactly — `run_once`, `JobKind::ImportanceRecalibration`, `JobsConfig.importance: ImportanceConfig`, `JobSummary`, the scheduler, and the `Request::RunJob` path — adding only the recalibration arm plus the two read seams it needs (`SqliteStore::memories_for_recalibration` and its `StoreHandle` wrapper). All mutations reuse the existing single-writer `StoreHandle::update(namespace, id, MemoryUpdates{ importance: Some(n), ..default })` path; no new write command is introduced. The recalibration function is pure and takes `now` by value so every test is deterministic.

**Recalibration formula (the documented contract, implemented in Task T3):**

Idempotency is the hard constraint: a memory's `importance` column is *both* the input and where the result is written, so the formula must be a **fixed point** — re-running over unchanged access data must produce the identical value. We get that by making the recalibrated value a pure function of the access signals alone (NOT an accumulator over the current importance) whenever there is any access signal, and falling back to the author's `base` only when the memory has never been touched:

```text
recency = match last_accessed_at {
    Some(t) => 0.5_f64.powf((((now - t).max(0)) as f64 / 86_400.0) / half_life_days),
    None    => 0.0,                          // never accessed contributes no recency
};
access  = (access_count.max(0) as f64).ln_1p(); // ln(1 + n): diminishing returns, ln_1p(0) == 0
// signal in [0, +inf): heavier with more/recent access; EXACTLY 0.0 when never accessed
// (access_count == 0 AND last_accessed_at is None).
signal  = access_weight * access + recency_weight * recency;

if signal <= 0.0 {
    // Untouched memory: keep the author's importance verbatim (still clamped for safety).
    new = (base as f64).round().clamp(1.0, 10.0) as u8
} else {
    // Touched memory: target is a PURE function of access data on the full 1..=10 band.
    // tanh squashes the unbounded signal into [0,1); FLOOR=1, SPAN=9 spread it across 1..=10.
    const FLOOR: f64 = 1.0;
    const SPAN: f64 = 9.0; // 10.0 - 1.0
    target = FLOOR + SPAN * signal.tanh();
    new = target.round().clamp(1.0, 10.0) as u8
}
```

Properties (all asserted in Task T3):
- **Idempotent / fixed point**: for a touched memory, `new` is derived from `(access_count, last_accessed_at, now, cfg)` ONLY — `base` (the current stored importance) never enters the touched branch — so writing `new` back and re-reading it yields the identical `new`. An untouched memory stays at `base` (also a fixed point). The `is_a_fixed_point` test feeds the output back as the next `base` and asserts equality.
- **Monotonic in access**: `signal`, hence `tanh(signal)`, is non-decreasing in `access_count`, so more access can only raise (never lower) the touched-branch `new`.
- **Recency decay**: a stale `last_accessed_at` drives `recency -> 0`; a never-accessed memory (`access_count == 0` and `last_accessed_at == None`) has `signal == 0.0` and falls into the untouched branch, leaving `new == base`.
- **Always valid**: every branch ends in `.clamp(1.0, 10.0) as u8`, so the output always passes `validate_importance`.

---

### Task T1: rb-store `store.rs` — recal read

**Files:**
- Modify: `crates/rb-store/src/store.rs`

- [ ] **Step 1 RED: add the failing test** — append this test to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/rb-store/src/store.rs` (the module already begins with `#![allow(clippy::unwrap_used, clippy::expect_used)]`; do NOT add a second attribute). It asserts the new `memories_for_recalibration` read carries the access fields, excludes archived rows, and is bounded by `limit`.

```rust
    #[test]
    fn memories_for_recalibration_carries_access_fields_and_excludes_archived() {
        let store = SqliteStore::open_in_memory(8).unwrap();

        // Two active rows in different namespaces; one archived row.
        let mut a = MemoryNote::new(
            Namespace::Global,
            "frequently accessed".into(),
            MemoryType::Insight,
            5,
        );
        a.access_count = 7;
        a.last_accessed_at = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0);
        store.insert_memory(&a, None).unwrap();

        let b = MemoryNote::new(
            Namespace::Project("rb".into()),
            "never accessed".into(),
            MemoryType::Reference,
            3,
        );
        store.insert_memory(&b, None).unwrap();

        let gone = MemoryNote::new(Namespace::Global, "archived".into(), MemoryType::Insight, 9);
        let gone_id = gone.id.clone();
        store.insert_memory(&gone, None).unwrap();
        store.archive_memory(&gone_id).unwrap();

        let rows = store.memories_for_recalibration(100).unwrap();

        // Archived row excluded; exactly the two active rows returned.
        assert_eq!(rows.len(), 2, "archived rows must be excluded");
        assert!(
            rows.iter().all(|r| r.id != gone_id),
            "archived id must not appear"
        );

        let row_a = rows
            .iter()
            .find(|r| r.id == a.id)
            .expect("active row a must be present");
        assert_eq!(row_a.namespace, Namespace::Global);
        assert_eq!(row_a.importance, 5);
        assert_eq!(row_a.access_count, 7);
        assert_eq!(row_a.last_accessed_at, Some(1_700_000_000));

        let row_b = rows
            .iter()
            .find(|r| r.id == b.id)
            .expect("active row b must be present");
        assert_eq!(row_b.namespace, Namespace::Project("rb".into()));
        assert_eq!(row_b.importance, 3);
        assert_eq!(row_b.access_count, 0);
        assert_eq!(row_b.last_accessed_at, None);
    }

    #[test]
    fn memories_for_recalibration_respects_limit() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        for i in 0..5 {
            let m = MemoryNote::new(
                Namespace::Global,
                format!("note {i}"),
                MemoryType::Insight,
                4,
            );
            store.insert_memory(&m, None).unwrap();
        }
        let rows = store.memories_for_recalibration(3).unwrap();
        assert_eq!(rows.len(), 3, "limit must bound the row count");
    }
```

- [ ] **Step 2: run it** — Run: `cargo test -p rb-store memories_for_recalibration` — Expected: FAIL to compile with `no method named memories_for_recalibration found for struct SqliteStore` and `cannot find type RecalRow` (the type and method do not exist yet).

- [ ] **Step 3 GREEN: implement the read** — add the public `RecalRow` type and the `memories_for_recalibration` inherent method. Insert the `RecalRow` struct definition immediately AFTER the `impl SqliteStore { ... }` block that ends at line 150 (i.e. directly before the `static VEC_REGISTERED` declaration), and add the method inside a NEW `impl SqliteStore` block placed in the same spot. Use explicit columns mirroring `list`, and the existing `parse_id` / `Namespace::parse_db_string` helpers.

```rust
/// One active memory's recalibration inputs: the spine fields the importance
/// job reads to recompute `importance`. `last_accessed_at` is the raw stored
/// unix-seconds value (`None` when the memory has never been accessed); the job
/// passes it straight into the pure `recalibrate` function with a single `now`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecalRow {
    pub namespace: Namespace,
    pub id: MemoryId,
    pub importance: u8,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
}

impl SqliteStore {
    /// Read up to `limit` ACTIVE (non-archived) memories with the fields the
    /// importance-recalibration job needs. Bounded by `limit` and ordered by
    /// `created_at DESC` for a stable, deterministic scan. Read-only: issues no
    /// writes (every mutation goes through the single writer via `StoreHandle`).
    pub fn memories_for_recalibration(&self, limit: usize) -> Result<Vec<RecalRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT namespace, memory_id, importance, access_count, last_accessed_at
                 FROM memories
                 WHERE archived_at IS NULL
                 ORDER BY created_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut rows = stmt
            .query(rusqlite::params![i64::try_from(limit).unwrap_or(i64::MAX)])
            .map_err(|e| Error::Storage(e.to_string()))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            let namespace = Namespace::parse_db_string(
                &row.get::<_, String>("namespace")
                    .map_err(|e| Error::Storage(e.to_string()))?,
            )?;
            let id = parse_id(
                &row.get::<_, String>("memory_id")
                    .map_err(|e| Error::Storage(e.to_string()))?,
            )?;
            let importance = row
                .get::<_, i64>("importance")
                .map_err(|e| Error::Storage(e.to_string()))? as u8;
            let access_count = row
                .get::<_, i64>("access_count")
                .map_err(|e| Error::Storage(e.to_string()))?;
            let last_accessed_at = row
                .get::<_, Option<i64>>("last_accessed_at")
                .map_err(|e| Error::Storage(e.to_string()))?;
            out.push(RecalRow {
                namespace,
                id,
                importance,
                access_count,
                last_accessed_at,
            });
        }
        Ok(out)
    }
}
```

  Then re-export `RecalRow` from the crate root — modify `crates/rb-store/src/lib.rs`:

```rust
pub use store::{RecalRow, SqliteStore, Store};
```

- [ ] **Step 4: run it** — Run: `cargo test -p rb-store memories_for_recalibration` — Expected: PASS (2 tests).

- [ ] **Step 5: lint+format** — Run: `cargo clippy -p rb-store --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit** — Run: `git add crates/rb-store/src/store.rs crates/rb-store/src/lib.rs && git commit -m "feat(rb-store): add memories_for_recalibration read for the importance job"` — Expected: one commit.

---

### Task T2: rb-daemon `store_handle.rs` — recal seam

**Files:**
- Modify: `crates/rb-daemon/src/store_handle.rs`

- [ ] **Step 1 RED: add the failing test** — add this test to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/rb-daemon/src/store_handle.rs` (the module already begins with `#![allow(clippy::unwrap_used, clippy::expect_used)]`; do NOT add a second attribute). It drives a write + record_access through the handle, then reads the recal rows back through the read pool.

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_memories_for_recalibration_reads_access_fields() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("recal".to_string());

        let m = note(&ns, "accessed twice");
        let id = m.id.clone();
        handle.write(m, Some(vec![0.1f32; DIM])).await.unwrap();

        // Two accesses bump access_count to 2 and stamp last_accessed_at.
        handle.record_access(id.clone()).await.unwrap();
        handle.record_access(id.clone()).await.unwrap();

        let rows = handle.memories_for_recalibration(100).await.unwrap();
        let row = rows
            .iter()
            .find(|r| r.id == id)
            .expect("recal row must be present");
        assert_eq!(row.namespace, ns);
        assert_eq!(row.access_count, 2, "two record_access calls => count 2");
        assert!(
            row.last_accessed_at.is_some(),
            "last_accessed_at must be stamped after record_access"
        );

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it** — Run: `cargo test -p rb-daemon store_handle_memories_for_recalibration` — Expected: FAIL to compile with `no method named memories_for_recalibration found for struct StoreHandle`.

- [ ] **Step 3 GREEN: implement the read-pool wrapper** — first import `RecalRow` by changing the `rb_store` use line near the top of `crates/rb-daemon/src/store_handle.rs` from:

```rust
use rb_store::{SqliteStore, Store};
```

  to:

```rust
use rb_store::{RecalRow, SqliteStore, Store};
```

  Then add this inherent method inside the existing `impl StoreHandle { ... }` block (place it directly after the `subscribe` method, before `shutdown`). It runs the read on the bounded read pool via the existing private `with_read` helper.

```rust
    /// Read up to `limit` active memories with the fields the importance job
    /// needs. Goes through the bounded read pool (never the writer). Used only by
    /// the cross-namespace maintenance jobs, which then issue any importance
    /// changes back through `update` (the single writer).
    pub async fn memories_for_recalibration(&self, limit: usize) -> Result<Vec<RecalRow>> {
        self.with_read(move |store| store.memories_for_recalibration(limit))
            .await
    }
```

- [ ] **Step 4: run it** — Run: `cargo test -p rb-daemon store_handle_memories_for_recalibration` — Expected: PASS (1 test).

- [ ] **Step 5: lint+format** — Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit** — Run: `git add crates/rb-daemon/src/store_handle.rs && git commit -m "feat(rb-daemon): expose memories_for_recalibration on the store handle"` — Expected: one commit.

---

### Task T3: rb-daemon `jobs/importance.rs` — recalibrate fn

**Files:**
- Create: `crates/rb-daemon/src/jobs/importance.rs`

> Part R already created `crates/rb-daemon/src/jobs/` with `mod.rs`, `config.rs`, `scheduler.rs`, `link_decay.rs`, `consolidation.rs`, and a placeholder `importance.rs`, and wired `mod jobs;` into `lib.rs`. This task fills in `importance.rs` with the pure recalibration function (the heart of the job) plus its unit tests. `ImportanceConfig` is defined in `jobs/config.rs` per the scaffolding contract.

- [ ] **Step 1 RED: write the failing unit tests** — create `crates/rb-daemon/src/jobs/importance.rs` containing ONLY the `recalibrate` function declaration (stub) and the test module, so the tests compile against a real signature but fail on behaviour. Write the file exactly:

```rust
//! Importance recalibration job: recompute `importance` from access_count and
//! last_accessed_at (recency), clamped to the validated 1..=10 range.

use crate::jobs::config::ImportanceConfig;

/// Recompute an importance value from access frequency and recency.
///
/// Deterministic, monotonic in access, and a FIXED POINT (idempotent): for a
/// touched memory the result depends only on the access signals, never on the
/// current importance, so re-running over unchanged access data is a no-op.
/// `now` and `last_accessed_at` are unix seconds.
///
/// Formula (documented contract):
///   recency = last_accessed_at.map(|t| 0.5^(((now - t).max(0)/86400)/half_life_days)).unwrap_or(0.0)
///   access  = ln(1 + access_count.max(0))
///   signal  = access_weight*access + recency_weight*recency
///   if signal <= 0 => new = clamp(round(base))                  // untouched: keep author's value
///   else           => new = clamp(round(1 + 9*tanh(signal)))    // touched: pure access target
pub fn recalibrate(
    _base: u8,
    _access_count: i64,
    _last_accessed_at: Option<i64>,
    _now: i64,
    _cfg: &ImportanceConfig,
) -> u8 {
    0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn cfg() -> ImportanceConfig {
        ImportanceConfig {
            enabled: true,
            interval_secs: 86_400,
            access_weight: 0.5,
            recency_weight: 0.5,
            half_life_days: 30.0,
            batch_limit: 1000,
        }
    }

    const NOW: i64 = 1_700_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn output_is_always_a_valid_importance() {
        // Every output must pass the 1..=10 validator, for a wide input sweep.
        for base in 1u8..=10 {
            for access in [0i64, 1, 10, 100, 10_000, -5] {
                for last in [None, Some(NOW), Some(NOW - 365 * DAY), Some(0), Some(NOW + DAY)] {
                    let out = recalibrate(base, access, last, NOW, &cfg());
                    assert!(
                        rb_types::validate_importance(out).is_ok(),
                        "recalibrate({base},{access},{last:?}) = {out} must be 1..=10"
                    );
                }
            }
        }
    }

    #[test]
    fn is_a_fixed_point_for_touched_and_untouched() {
        // Idempotency: feeding the output back in as `base` must yield the same
        // value, both for a touched memory (access-derived target) and an
        // untouched one (author's base preserved).
        for (access, last) in [
            (50i64, Some(NOW)),        // touched: heavy + fresh
            (3, Some(NOW - 10 * DAY)), // touched: light + slightly stale
            (0, None),                 // untouched
        ] {
            for base in 1u8..=10 {
                let once = recalibrate(base, access, last, NOW, &cfg());
                let twice = recalibrate(once, access, last, NOW, &cfg());
                assert_eq!(
                    once, twice,
                    "recalibrate must be a fixed point: base={base}, access={access}, \
                     last={last:?}, once={once}, twice={twice}"
                );
            }
        }
    }

    #[test]
    fn clamps_to_upper_bound_ten() {
        // Heavy access + fresh recency saturates the touched-branch target at 10.
        let out = recalibrate(10, 1_000_000, Some(NOW), NOW, &cfg());
        assert_eq!(out, 10, "must clamp at the upper bound");
    }

    #[test]
    fn clamps_to_lower_bound_one() {
        // Minimum base, never accessed: untouched branch keeps base, never below 1.
        let out = recalibrate(1, 0, None, NOW, &cfg());
        assert_eq!(out, 1, "must clamp at the lower bound");
    }

    #[test]
    fn more_access_never_lowers_importance() {
        // Monotonic in access_count: more accesses => importance >= fewer accesses.
        let few = recalibrate(5, 1, Some(NOW), NOW, &cfg());
        let many = recalibrate(5, 10_000, Some(NOW), NOW, &cfg());
        assert!(
            many >= few,
            "more access must not lower importance: few={few}, many={many}"
        );
    }

    #[test]
    fn stale_and_unaccessed_falls_back_to_base() {
        // A very old last_accessed_at with zero access decays the signal to 0,
        // so it lands in the untouched branch exactly like a never-accessed
        // memory: both keep `base`.
        let stale = recalibrate(5, 0, Some(NOW - 3650 * DAY), NOW, &cfg());
        let never = recalibrate(5, 0, None, NOW, &cfg());
        assert_eq!(stale, 5, "stale + unaccessed keeps base");
        assert_eq!(
            stale, never,
            "decayed-to-zero signal must match never-accessed: stale={stale}, never={never}"
        );
    }

    #[test]
    fn none_last_accessed_is_treated_as_never_accessed() {
        // None contributes zero recency; with zero access the signal is 0 and the
        // untouched branch keeps base verbatim.
        let out = recalibrate(6, 0, None, NOW, &cfg());
        assert_eq!(out, 6, "no access and no recency leaves base unchanged");
    }

    #[test]
    fn future_last_accessed_is_clamped_to_zero_age_not_negative() {
        // (now - t).max(0) guards a clock-skewed future timestamp: recency is the
        // maximum (1.0), never a NaN/negative blow-up. The recency alone makes the
        // signal positive => touched branch, identical to an exactly-now access.
        let future = recalibrate(5, 0, Some(NOW + 10 * DAY), NOW, &cfg());
        let now_exact = recalibrate(5, 0, Some(NOW), NOW, &cfg());
        assert_eq!(
            future, now_exact,
            "future timestamp clamps to age 0, same as now: future={future}, now={now_exact}"
        );
        assert!(rb_types::validate_importance(future).is_ok());
    }

    #[test]
    fn touched_target_matches_documented_formula() {
        // access_count=1, fresh: access=ln(2)=0.6931, recency=1.0,
        // signal = 0.5*0.6931 + 0.5*1.0 = 0.84657; tanh(0.84657)=0.68915;
        // target = 1 + 9*0.68915 = 7.2024 => round => 7. base is irrelevant here.
        let out = recalibrate(3, 1, Some(NOW), NOW, &cfg());
        assert_eq!(out, 7, "touched target is a pure function of access, not base");
        // Same access signal with a different base yields the SAME touched target
        // (proves base does not enter the touched branch — the fixed-point property).
        let out_other_base = recalibrate(9, 1, Some(NOW), NOW, &cfg());
        assert_eq!(
            out, out_other_base,
            "touched target ignores base: {out} vs {out_other_base}"
        );
    }
}
```

- [ ] **Step 2: run it** — Run: `cargo test -p rb-daemon recalibrate` — Expected: FAIL — the stub returns `0`, so `output_is_always_a_valid_importance` panics (`recalibrate(...) = 0 must be 1..=10`) and the fixed-point / target tests assert-fail.

- [ ] **Step 3 GREEN: implement `recalibrate`** — replace the stub body with the real, documented fixed-point formula. Replace the whole `pub fn recalibrate(...) -> u8 { 0 }` block with:

```rust
pub fn recalibrate(
    base: u8,
    access_count: i64,
    last_accessed_at: Option<i64>,
    now: i64,
    cfg: &ImportanceConfig,
) -> u8 {
    // Recency in [0,1]: exponential decay by elapsed days over the half-life.
    // A future (clock-skewed) timestamp clamps to age 0 => recency 1.0.
    // None (never accessed) contributes no recency.
    let recency = match last_accessed_at {
        Some(t) => {
            let age_days = (now - t).max(0) as f64 / 86_400.0;
            0.5_f64.powf(age_days / cfg.half_life_days.max(f64::MIN_POSITIVE))
        }
        None => 0.0,
    };

    // Access contribution: ln(1 + n) gives diminishing returns and ln_1p(0) == 0.
    let access = (access_count.max(0) as f64).ln_1p();

    // Combined access signal in [0, +inf); EXACTLY 0.0 only when never accessed.
    let signal = cfg.access_weight * access + cfg.recency_weight * recency;

    let value = if signal <= 0.0 {
        // Untouched memory: keep the author's importance (still clamped for safety).
        base as f64
    } else {
        // Touched memory: target is a PURE function of the access signal on the
        // full 1..=10 band, independent of `base`. This is what makes the job a
        // fixed point — re-running re-derives the same target, so nothing changes.
        const FLOOR: f64 = 1.0;
        const SPAN: f64 = 9.0; // 10.0 - 1.0
        FLOOR + SPAN * signal.tanh()
    };

    // Always a valid importance (1..=10): the clamp is the single source of truth
    // that keeps the output inside validate_importance's range.
    value.round().clamp(1.0, 10.0) as u8
}
```

- [ ] **Step 4: run it** — Run: `cargo test -p rb-daemon recalibrate` — Expected: PASS (9 tests).

- [ ] **Step 5: lint+format** — Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit** — Run: `git add crates/rb-daemon/src/jobs/importance.rs && git commit -m "feat(rb-daemon): add pure importance recalibration function"` — Expected: one commit.

---

### Task T4: rb-daemon `jobs/importance.rs` — run + arm

**Files:**
- Modify: `crates/rb-daemon/src/jobs/importance.rs`
- Modify: `crates/rb-daemon/src/jobs/mod.rs`

> `chrono` is already a regular `[dependencies]` entry in `crates/rb-daemon/Cargo.toml` (promoted in Part R Task R2, since `jobs/link_decay.rs` also calls `chrono::Utc::now()` at runtime). This task therefore makes NO `Cargo.toml` change — it only adds the `run` function and wires the `run_once` arm. The `run` function below calls `chrono::Utc::now()` in non-test code, which compiles because of that earlier promotion.

- [ ] **Step 1 RED: write the failing job test** — append this async test to the `#[cfg(test)] mod tests` block in `crates/rb-daemon/src/jobs/importance.rs`. It drives a real `StoreHandle`, accesses one memory, runs the job once, asserts the accessed memory's importance rose and the never-accessed one is unchanged, then runs the job a second time and asserts ZERO changes (idempotency), with the namespace preserved.

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_recalibrates_accessed_memories_and_is_idempotent() {
        use crate::StoreHandle;
        use rb_engine::MemoryBackend;
        use rb_types::{MemoryNote, MemoryType, Namespace};

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, 8, 2).unwrap();
        let ns = Namespace::Project("recal-job".to_string());

        // hot: low base importance, will be accessed many times.
        let hot = MemoryNote::new(ns.clone(), "hot memory".into(), MemoryType::Insight, 3);
        let hot_id = hot.id.clone();
        handle.write(hot, Some(vec![0.1f32; 8])).await.unwrap();

        // cold: never accessed, importance should not change (delta 0 => skipped).
        let cold = MemoryNote::new(ns.clone(), "cold memory".into(), MemoryType::Reference, 3);
        let cold_id = cold.id.clone();
        handle.write(cold, Some(vec![0.2f32; 8])).await.unwrap();

        for _ in 0..50 {
            handle.record_access(hot_id.clone()).await.unwrap();
        }

        let cfg = ImportanceConfig {
            enabled: true,
            interval_secs: 86_400,
            access_weight: 1.0,
            recency_weight: 1.0,
            half_life_days: 30.0,
            batch_limit: 1000,
        };

        // First pass: hot rises, cold unchanged.
        let summary = run(&handle, &cfg).await.unwrap();
        assert_eq!(summary.scanned, 2, "both active rows scanned");
        assert_eq!(summary.changed, 1, "only the hot memory changed");
        assert_eq!(summary.skipped, 1, "the cold memory was skipped");

        let hot_after = handle
            .get(ns.clone(), hot_id.clone())
            .await
            .unwrap()
            .expect("hot memory present");
        assert!(
            hot_after.importance > 3,
            "accessed memory's importance must rise above base 3, got {}",
            hot_after.importance
        );
        assert_eq!(
            hot_after.namespace, ns,
            "update must preserve the row's own namespace"
        );

        let cold_after = handle
            .get(ns.clone(), cold_id.clone())
            .await
            .unwrap()
            .expect("cold memory present");
        assert_eq!(
            cold_after.importance, 3,
            "never-accessed memory keeps its base importance"
        );

        // Second pass with unchanged access data: nothing changes (idempotent).
        let again = run(&handle, &cfg).await.unwrap();
        assert_eq!(again.scanned, 2, "second pass still scans both rows");
        assert_eq!(
            again.changed, 0,
            "idempotent: re-running with unchanged access data changes nothing"
        );
        assert_eq!(again.skipped, 2, "both rows already at their recalibrated value");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_once_dispatches_importance_arm() {
        use crate::jobs::{run_once, JobKind, JobsConfig};
        use crate::StoreHandle;
        use rb_engine::MemoryBackend;
        use rb_types::{MemoryNote, MemoryType, Namespace};

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, 8, 2).unwrap();
        let ns = Namespace::Global;

        let m = MemoryNote::new(ns.clone(), "dispatched".into(), MemoryType::Insight, 4);
        handle.write(m, Some(vec![0.1f32; 8])).await.unwrap();

        let config = JobsConfig::default();
        let summary = run_once(JobKind::ImportanceRecalibration, &handle, &config)
            .await
            .unwrap();
        assert_eq!(
            summary.scanned, 1,
            "run_once must route ImportanceRecalibration through importance::run"
        );

        handle.shutdown().await;
    }
```

- [ ] **Step 2: run it** — Run: `cargo test -p rb-daemon run_recalibrates_accessed_memories_and_is_idempotent run_once_dispatches_importance_arm` — Expected: FAIL to compile with `cannot find function run in this scope` (the `run` fn does not exist yet; the `run_once` import resolves to Part R's `run_once`, whose `ImportanceRecalibration` arm currently returns a default/empty summary).

- [ ] **Step 3 GREEN: implement `run` and wire the arm** — `chrono` is already a regular dependency (Part R Task R2), so no `Cargo.toml` change is needed here. Add the `run` function to `crates/rb-daemon/src/jobs/importance.rs`, immediately after the `recalibrate` function (before the `#[cfg(test)] mod tests`). Update the imports at the top of the file to bring in what `run` needs.

  Change the top-of-file `use` line from:

```rust
use crate::jobs::config::ImportanceConfig;
```

  to:

```rust
use crate::jobs::config::ImportanceConfig;
use crate::jobs::JobSummary;
use crate::StoreHandle;
use rb_engine::MemoryBackend;
use rb_types::{MemoryUpdates, Result};
```

  Add the `run` function:

```rust
/// Run one bounded, idempotent recalibration pass.
///
/// Reads up to `cfg.batch_limit` active memories via the read pool, recomputes
/// each importance with [`recalibrate`], and — only when the value actually
/// changes — writes it back through the single-writer `update` path using the
/// row's OWN namespace. Idempotent: a second pass over unchanged access data
/// recomputes the same values and writes nothing. Fail-safe: each update is its
/// own writer transaction; a single failed update aborts the pass with an error
/// rather than leaving a half-applied batch.
pub async fn run(store: &StoreHandle, cfg: &ImportanceConfig) -> Result<JobSummary> {
    let now = chrono::Utc::now().timestamp();
    let rows = store.memories_for_recalibration(cfg.batch_limit).await?;

    let mut summary = JobSummary::default();
    for row in rows {
        summary.scanned += 1;
        let new = recalibrate(
            row.importance,
            row.access_count,
            row.last_accessed_at,
            now,
            cfg,
        );
        if new == row.importance {
            summary.skipped += 1;
            continue;
        }
        store
            .update(
                row.namespace.clone(),
                row.id.clone(),
                MemoryUpdates {
                    importance: Some(new),
                    ..Default::default()
                },
            )
            .await?;
        summary.changed += 1;
    }
    Ok(summary)
}
```

  Then wire the `ImportanceRecalibration` arm in `run_once`. Open `crates/rb-daemon/src/jobs/mod.rs` and replace the `JobKind::ImportanceRecalibration` match arm inside `run_once` (Part R left it returning a default summary / `todo`-free placeholder) so it delegates to `importance::run`:

```rust
        JobKind::ImportanceRecalibration => importance::run(store, &config.importance).await,
```

  (The surrounding `pub async fn run_once(kind: JobKind, store: &StoreHandle, config: &JobsConfig) -> rb_types::Result<JobSummary>` signature, the `mod importance;` declaration, and the `LinkDecay`/`Consolidation` arms are all already present from Part R — change ONLY the `ImportanceRecalibration` arm.)

- [ ] **Step 4: run it** — Run: `cargo test -p rb-daemon run_recalibrates_accessed_memories_and_is_idempotent run_once_dispatches_importance_arm` — Expected: PASS (2 tests).

- [ ] **Step 5: lint+format** — Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit** — Run: `git add crates/rb-daemon/src/jobs/importance.rs crates/rb-daemon/src/jobs/mod.rs && git commit -m "feat(rb-daemon): wire importance recalibration job into run_once"` — Expected: one commit.

---

### Task T5: rb-daemon `tests/daemon_e2e.rs` — run-job flow

**Files:**
- Modify: `crates/rb-daemon/tests/daemon_e2e.rs` (test only)

> Part R already added the wire path: `Request::RunJob { job }` -> `Response::JobRan { scanned, changed, skipped }` (dispatched server-side against the `StoreHandle`, NOT the namespace-bound engine), the proto-shared `rb_proto::JobKind`, and the `Client::run_job(job)` wrapper that returns a `JobSummary`-shaped result. The scheduler also ticks `run_once` per the same arm. This task adds one end-to-end test over a real Unix socket (using the existing `RunningDaemon` harness in this file) confirming `JobKind::ImportanceRecalibration` flows through `Client::run_job` and yields a populated `JobSummary` — no new wiring beyond the Task T4 `run_once` arm.

- [ ] **Step 1 RED: write the failing e2e test** — append this test to `crates/rb-daemon/tests/daemon_e2e.rs` (the file already begins with `#![allow(clippy::unwrap_used, clippy::expect_used)]` and defines `DIM`, `RunningDaemon`, and imports `Client`, `MemoryType`, `Namespace`; reuse them). It seeds one memory via the typed `remember` helper, then triggers the importance job over the wire via the Part R `run_job` wrapper and asserts the returned summary scanned exactly that row.

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_job_importance_recalibration_flows_through_client() {
    use rb_proto::JobKind;

    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("recal-e2e".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    // Seed one memory in this namespace via the typed remember helper.
    let id = client
        .remember(
            "importance recalibration target".to_string(),
            Some("evolution".to_string()),
            MemoryType::Insight,
            4,
            vec!["evolution".to_string()],
            vec!["jobs".to_string()],
            vec![],
        )
        .await
        .unwrap();

    // Trigger the cross-namespace importance job over the wire (Part R wrapper).
    let summary = client.run_job(JobKind::ImportanceRecalibration).await.unwrap();
    assert_eq!(
        summary.scanned, 1,
        "the one seeded row must be scanned by the importance job"
    );
    // A freshly-remembered, never-accessed row recalibrates to its base:
    // delta is 0 => unchanged => skipped, not changed.
    assert_eq!(summary.changed, 0, "never-accessed row is unchanged");
    assert_eq!(summary.skipped, 1, "never-accessed row is skipped");

    // The seeded memory still resolves and kept its base importance.
    let got = client.get(id).await.unwrap().expect("seeded memory present");
    assert_eq!(got.importance, 4, "never-accessed memory keeps its base");

    daemon.stop().await;
}
```

- [ ] **Step 2: run it** — Run: `cargo test -p rb-daemon --test daemon_e2e run_job_importance_recalibration_flows_through_client` — Expected: PASS (1 test). The `Request::RunJob` wire path, `rb_proto::JobKind`, `Client::run_job`, and the `run_once` importance arm all already exist (Part R + Task T4), so this asserts the end-to-end flow rather than introducing new code. (If it FAILS to compile because the `remember` helper arity differs in your tree, mirror the exact signature used by `full_round_trip_through_client` in this same file — the `JobRan`/summary assertions are the load-bearing part.)

- [ ] **Step 3 GREEN: no implementation change** — this test exercises only the existing Part R wire path and the Task T4 `run_once` importance arm. If Step 2 already passed, make no code change and proceed. If Step 2 failed solely on a `remember` arity mismatch, the only edit is to the TEST call to match `full_round_trip_through_client` in this file — never weaken the `scanned`/`changed`/`skipped` assertions.

- [ ] **Step 4: run it** — Run: `cargo test -p rb-daemon --test daemon_e2e run_job_importance_recalibration_flows_through_client` — Expected: PASS (1 test).

- [ ] **Step 5: lint+format** — Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit** — Run: `git add crates/rb-daemon/tests/daemon_e2e.rs && git commit -m "test(rb-daemon): cover run-job dispatch for importance recalibration"` — Expected: one commit.

---

### Part T gate

**Files:** (none — verification only)

- [ ] **Step 1: workspace tests** — Run: `cargo test --workspace` — Expected: PASS, 0 failures.

- [ ] **Step 2: workspace clippy** — Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings.

- [ ] **Step 3: workspace format** — Run: `cargo fmt --all --check` — Expected: no diff.

> Part T adds no new dependencies, so `cargo deny check` is not required for this Part (it is run by Parts R and U, which touch deps).


## Part U — local ONNX embeddings via fastembed (behind `local` feature)

This Part adds a third `EmbeddingProvider`: an offline-capable local ONNX provider backed by the `fastembed` crate (v5.15.0, Apache-2.0), defaulting to the `all-MiniLM-L6-v2` model at **384 dimensions**. Every line of fastembed/ort/onnxruntime code is gated behind a new `local` cargo feature that is **excluded from the default build closure** — a plain `cargo build --workspace` never compiles or links it. The feature is forwarded `rusty-brain → rb-embed`, wired as a `#[cfg(feature = "local")] ProviderKind::Local` arm with `select_provider_kind` extended to take a `local_requested: bool` (precedence `local > voyage > deterministic`), and the variant is exercised by a dedicated `build-local` CI job so the dep is type/lint-checked without polluting the default test job. ONNX inference is CPU-bound and synchronous, so `embed` runs inside `tokio::task::spawn_blocking`; bounded concurrency is provided by the existing `SharedEmbedder` semaphore. All commands below run from the worktree root `/Volumes/raid1/repos/rusty-brain-p3`.

---

### Task U1: rb-embed `Cargo.toml` — optional dep

**Files:**
- Modify: crates/rb-embed/Cargo.toml

- [ ] **Step 1 RED: prove the default closure is fastembed-free and the feature is declared.**
  This task has no Rust test; the "RED" check is two shell assertions on the dependency tree. Before the edit, run them to capture the starting state:

  ```bash
  cargo tree -e no-dev -p rb-embed | grep -i fastembed
  ```
  Expected (before any edit): exits non-zero, prints nothing (fastembed is not a dependency yet).

- [ ] **Step 2: run it — Run:** `cargo tree -e no-dev -p rb-embed | grep -i fastembed` — Expected: FAIL (grep exits 1, no output) because no feature table exists and fastembed is not declared.

- [ ] **Step 3 GREEN: add fastembed as an OPTIONAL dependency and a `local` feature gate.**
  Edit `crates/rb-embed/Cargo.toml` so the full file reads exactly:

  ```toml
  [package]
  name = "rb-embed"
  version.workspace = true
  edition.workspace = true
  license.workspace = true
  authors.workspace = true
  repository.workspace = true
  description = "EmbeddingProvider trait, Voyage remote provider, and an offline deterministic provider for rusty-brain."

  [lib]
  name = "rb_embed"
  path = "src/lib.rs"

  [features]
  # Local ONNX embeddings via fastembed (all-MiniLM-L6-v2, 384-dim).
  # OFF by default: the default build closure MUST NOT compile or link
  # fastembed/ort/onnxruntime. Enabled in CI only by the `build-local` job
  # and at runtime via `--features local`.
  local = ["dep:fastembed"]

  [dependencies]
  rb-types = { path = "../rb-types" }
  async-trait = { workspace = true }
  reqwest = { workspace = true }
  serde = { workspace = true }
  serde_json = { workspace = true }
  secrecy = { workspace = true }
  sha2 = { workspace = true }
  # Optional: only compiled when the `local` feature is enabled.
  fastembed = { version = "5.15", optional = true }

  [dev-dependencies]
  tokio = { workspace = true }
  wiremock = { workspace = true }

  [lints]
  workspace = true
  ```

- [ ] **Step 4: run it — Run:** `cargo tree -e no-dev -p rb-embed | grep -i fastembed` (Expected: FAIL — grep exits 1, no output: fastembed is optional and the feature is OFF, so it stays out of the default closure) then **Run:** `cargo tree -e no-dev -p rb-embed --features local | grep -i fastembed` (Expected: PASS — prints a `fastembed v5.15.x` line, confirming the feature pulls it in). Also **Run:** `cargo build -p rb-embed` (Expected: PASS, no fastembed compiled).

- [ ] **Step 5: lint+format — Run:** `cargo clippy -p rb-embed --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit — Run:** `git add crates/rb-embed/Cargo.toml && git commit -m "chore(rb-embed): add optional fastembed dep behind local feature"` — Expected: one commit.

---

### Task U2: rb-embed `local.rs` — LocalProvider

**Files:**
- Create: crates/rb-embed/src/local.rs

- [ ] **Step 1 RED: write the failing test.**
  Create `crates/rb-embed/src/local.rs` containing ONLY the test module below for the RED step (the implementation is added in Step 3). The offline tests cover `model_id()`/`dim()` from the pure helpers and a fixture-constructed provider with no model loaded; the real-model `embed` test is `#[ignore]`. Full verbatim contents:

  ```rust
  #![cfg(feature = "local")]
  //! Local ONNX embedding provider backed by `fastembed`.
  //!
  //! Compiled only under the `local` cargo feature so the default build closure
  //! never links fastembed/ort/onnxruntime. The default model is
  //! `all-MiniLM-L6-v2` (384-dim). Model weights are downloaded at runtime on
  //! first use into fastembed's cache directory; there is no network access in
  //! unit tests (the real-embedding test is `#[ignore]`).

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;
      use crate::provider::EmbeddingProvider;

      #[test]
      fn default_model_name_resolves_to_all_minilm() {
          assert_eq!(resolve_model_name(""), "all-MiniLM-L6-v2");
          assert_eq!(resolve_model_name("all-MiniLM-L6-v2"), "all-MiniLM-L6-v2");
      }

      #[test]
      fn known_model_reports_384_dim() {
          assert_eq!(dim_for_model("all-MiniLM-L6-v2").unwrap(), 384);
      }

      #[test]
      fn unknown_model_is_an_embedding_error() {
          let err = dim_for_model("not-a-real-model").unwrap_err();
          assert!(
              matches!(err, rb_types::Error::Embedding(_)),
              "expected Error::Embedding for unknown model, got {err:?}"
          );
      }

      #[test]
      fn fixture_provider_reports_model_id_and_dim_without_loading_a_model() {
          // Build a provider WITHOUT a loaded model so the test is fully offline:
          // metadata must be available before (or without ever) downloading weights.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          assert_eq!(p.model_id(), "all-MiniLM-L6-v2");
          assert_eq!(p.dim(), 384);
      }

      #[tokio::test(flavor = "multi_thread")]
      async fn embed_without_loaded_model_is_an_embedding_error() {
          // The fixture provider has no model; calling embed must fail closed
          // with Error::Embedding rather than panicking.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          let err = p.embed(&["hello".to_string()]).await.unwrap_err();
          assert!(
              matches!(err, rb_types::Error::Embedding(_)),
              "expected Error::Embedding when no model is loaded, got {err:?}"
          );
      }

      #[tokio::test(flavor = "multi_thread")]
      async fn empty_input_yields_empty_output_without_loading_a_model() {
          // Empty input short-circuits: no model access, no error.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          let out = p.embed(&[]).await.unwrap();
          assert!(out.is_empty());
      }

      // Real-model smoke test. Ignored by default; downloads ~90MB of weights on
      // first run. Run with:
      //   cargo test -p rb-embed --features local -- --ignored local_real_model
      #[tokio::test(flavor = "multi_thread")]
      #[ignore = "downloads the all-MiniLM-L6-v2 model and runs ONNX inference"]
      async fn local_real_model_smoke() {
          let p = LocalProvider::load("all-MiniLM-L6-v2").unwrap();
          assert_eq!(p.dim(), 384);
          let out = p
              .embed(&["hello world".to_string(), "second".to_string()])
              .await
              .unwrap();
          assert_eq!(out.len(), 2);
          assert_eq!(out[0].len(), 384);
          assert_eq!(out[1].len(), 384);
      }
  }
  ```

- [ ] **Step 2: run it — Run:** `cargo test -p rb-embed --features local local::tests` — Expected: FAIL to **compile** with `cannot find function resolve_model_name` / `cannot find type LocalProvider in this scope` (no implementation yet).

- [ ] **Step 3 GREEN: minimal implementation.**
  Prepend the implementation ABOVE the `#[cfg(test)] mod tests` block so the full `crates/rb-embed/src/local.rs` reads exactly as below. `embed` runs fastembed's synchronous `embed` inside `tokio::task::spawn_blocking` (ONNX is CPU-bound); the model is held behind a `std::sync::Mutex` because `TextEmbedding::embed` takes `&mut self`. All fastembed/anyhow errors are mapped to `rb_types::Error::Embedding`. No `.unwrap()/.expect()/panic!` in non-test code — a poisoned mutex maps to `Error::Embedding`.

  ```rust
  #![cfg(feature = "local")]
  //! Local ONNX embedding provider backed by `fastembed`.
  //!
  //! Compiled only under the `local` cargo feature so the default build closure
  //! never links fastembed/ort/onnxruntime. The default model is
  //! `all-MiniLM-L6-v2` (384-dim). Model weights are downloaded at runtime on
  //! first use into fastembed's cache directory; there is no network access in
  //! unit tests (the real-embedding test is `#[ignore]`).

  use crate::provider::EmbeddingProvider;
  use async_trait::async_trait;
  use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
  use rb_types::Error;
  use std::sync::{Arc, Mutex};

  /// Default model when none is specified. 384-dimensional.
  pub const DEFAULT_MODEL: &str = "all-MiniLM-L6-v2";
  /// Embedding dimension of `all-MiniLM-L6-v2`.
  pub const DEFAULT_DIM: usize = 384;

  /// Map an (optionally empty) model name to the canonical name we support.
  /// An empty string selects the default model.
  pub fn resolve_model_name(name: &str) -> &str {
      if name.trim().is_empty() {
          DEFAULT_MODEL
      } else {
          name
      }
  }

  /// Map a model name to its fastembed enum variant, failing closed on unknown
  /// names rather than guessing a dimension.
  fn model_for_name(name: &str) -> rb_types::Result<EmbeddingModel> {
      match resolve_model_name(name) {
          "all-MiniLM-L6-v2" => Ok(EmbeddingModel::AllMiniLML6V2),
          other => Err(Error::Embedding(format!(
              "unsupported local embedding model: {other}"
          ))),
      }
  }

  /// Map a model name to its fixed embedding dimension. Unknown names are an
  /// `Error::Embedding` (the dim contract is sacred; never guess).
  pub fn dim_for_model(name: &str) -> rb_types::Result<usize> {
      match resolve_model_name(name) {
          "all-MiniLM-L6-v2" => Ok(DEFAULT_DIM),
          other => Err(Error::Embedding(format!(
              "unsupported local embedding model: {other}"
          ))),
      }
  }

  /// Offline-capable local embedding provider. Holds a loaded fastembed
  /// `TextEmbedding` behind a `Mutex` (its `embed` takes `&mut self`) and runs
  /// inference on a blocking thread pool. `model` is `None` only in tests built
  /// via [`LocalProvider::without_model`], which never load weights so metadata
  /// can be asserted offline.
  pub struct LocalProvider {
      model: Option<Arc<Mutex<TextEmbedding>>>,
      model_id: String,
      dim: usize,
  }

  impl LocalProvider {
      /// Load `model_name` (empty selects the default), downloading weights at
      /// runtime on first use. Maps any fastembed init failure to
      /// `Error::Embedding`.
      pub fn load(model_name: &str) -> rb_types::Result<Self> {
          let canonical = resolve_model_name(model_name).to_string();
          let model_enum = model_for_name(&canonical)?;
          let dim = dim_for_model(&canonical)?;
          let options = InitOptions::new(model_enum).with_show_download_progress(false);
          let model = TextEmbedding::try_new(options)
              .map_err(|e| Error::Embedding(format!("failed to load local model: {e}")))?;
          Ok(Self {
              model: Some(Arc::new(Mutex::new(model))),
              model_id: canonical,
              dim,
          })
      }

      /// Test/diagnostic constructor that records metadata WITHOUT loading any
      /// weights. `embed` on such a provider fails closed with `Error::Embedding`.
      pub fn without_model(model_id: &str, dim: usize) -> Self {
          Self {
              model: None,
              model_id: model_id.to_string(),
              dim,
          }
      }
  }

  #[async_trait]
  impl EmbeddingProvider for LocalProvider {
      fn model_id(&self) -> &str {
          &self.model_id
      }

      fn dim(&self) -> usize {
          self.dim
      }

      async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
          if texts.is_empty() {
              return Ok(Vec::new());
          }
          let model = self
              .model
              .as_ref()
              .ok_or_else(|| Error::Embedding("local model is not loaded".to_string()))?;
          let model = Arc::clone(model);
          let owned: Vec<String> = texts.to_vec();
          let expected_dim = self.dim;

          // ONNX inference is CPU-bound and synchronous; run it off the async
          // runtime. SharedEmbedder's semaphore bounds how many of these run.
          let vectors = tokio::task::spawn_blocking(move || -> rb_types::Result<Vec<Vec<f32>>> {
              let mut guard = model
                  .lock()
                  .map_err(|_| Error::Embedding("local model mutex poisoned".to_string()))?;
              let out = guard
                  .embed(&owned, None)
                  .map_err(|e| Error::Embedding(format!("local embedding failed: {e}")))?;
              if out.len() != owned.len() {
                  return Err(Error::Embedding(format!(
                      "local model returned {} embeddings for {} inputs",
                      out.len(),
                      owned.len()
                  )));
              }
              for v in &out {
                  if v.len() != expected_dim {
                      return Err(Error::Embedding(format!(
                          "local embedding dimension mismatch: expected {}, got {}",
                          expected_dim,
                          v.len()
                      )));
                  }
              }
              Ok(out)
          })
          .await
          .map_err(|e| Error::Embedding(format!("local embedding task failed to join: {e}")))??;

          Ok(vectors)
      }
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;
      use crate::provider::EmbeddingProvider;

      #[test]
      fn default_model_name_resolves_to_all_minilm() {
          assert_eq!(resolve_model_name(""), "all-MiniLM-L6-v2");
          assert_eq!(resolve_model_name("all-MiniLM-L6-v2"), "all-MiniLM-L6-v2");
      }

      #[test]
      fn known_model_reports_384_dim() {
          assert_eq!(dim_for_model("all-MiniLM-L6-v2").unwrap(), 384);
      }

      #[test]
      fn unknown_model_is_an_embedding_error() {
          let err = dim_for_model("not-a-real-model").unwrap_err();
          assert!(
              matches!(err, rb_types::Error::Embedding(_)),
              "expected Error::Embedding for unknown model, got {err:?}"
          );
      }

      #[test]
      fn fixture_provider_reports_model_id_and_dim_without_loading_a_model() {
          // Build a provider WITHOUT a loaded model so the test is fully offline:
          // metadata must be available before (or without ever) downloading weights.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          assert_eq!(p.model_id(), "all-MiniLM-L6-v2");
          assert_eq!(p.dim(), 384);
      }

      #[tokio::test(flavor = "multi_thread")]
      async fn embed_without_loaded_model_is_an_embedding_error() {
          // The fixture provider has no model; calling embed must fail closed
          // with Error::Embedding rather than panicking.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          let err = p.embed(&["hello".to_string()]).await.unwrap_err();
          assert!(
              matches!(err, rb_types::Error::Embedding(_)),
              "expected Error::Embedding when no model is loaded, got {err:?}"
          );
      }

      #[tokio::test(flavor = "multi_thread")]
      async fn empty_input_yields_empty_output_without_loading_a_model() {
          // Empty input short-circuits: no model access, no error.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          let out = p.embed(&[]).await.unwrap();
          assert!(out.is_empty());
      }

      // Real-model smoke test. Ignored by default; downloads ~90MB of weights on
      // first run. Run with:
      //   cargo test -p rb-embed --features local -- --ignored local_real_model
      #[tokio::test(flavor = "multi_thread")]
      #[ignore = "downloads the all-MiniLM-L6-v2 model and runs ONNX inference"]
      async fn local_real_model_smoke() {
          let p = LocalProvider::load("all-MiniLM-L6-v2").unwrap();
          assert_eq!(p.dim(), 384);
          let out = p
              .embed(&["hello world".to_string(), "second".to_string()])
              .await
              .unwrap();
          assert_eq!(out.len(), 2);
          assert_eq!(out[0].len(), 384);
          assert_eq!(out[1].len(), 384);
      }
  }
  ```

- [ ] **Step 4: run it — Run:** `cargo test -p rb-embed --features local local::tests` — Expected: PASS (6 tests run, the `#[ignore]`d `local_real_model_smoke` is skipped).

- [ ] **Step 5: lint+format — Run:** `cargo clippy -p rb-embed --features local --all-targets -- -D warnings` (Expected: no warnings) then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit — Run:** `git add crates/rb-embed/src/local.rs && git commit -m "feat(rb-embed): add LocalProvider for fastembed ONNX embeddings"` — Expected: one commit.

---

### Task U3: rb-embed `lib.rs` — module wiring

**Files:**
- Modify: crates/rb-embed/src/lib.rs

- [ ] **Step 1 RED: write the failing test.**
  Add this test to a new `#[cfg(test)] mod local_export_tests` at the end of `crates/rb-embed/src/lib.rs`. It asserts the crate re-exports `LocalProvider` under the feature. Full verbatim:

  ```rust
  #[cfg(all(test, feature = "local"))]
  mod local_export_tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use crate::{EmbeddingProvider, LocalProvider};

      #[test]
      fn local_provider_is_publicly_re_exported() {
          // Constructing via the crate-root path proves the `pub use` wiring.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          assert_eq!(p.dim(), 384);
          assert_eq!(p.model_id(), "all-MiniLM-L6-v2");
      }
  }
  ```

- [ ] **Step 2: run it — Run:** `cargo test -p rb-embed --features local local_export_tests` — Expected: FAIL to compile with `unresolved import crate::LocalProvider` and `cannot find module local` (the module is not declared or re-exported yet).

- [ ] **Step 3 GREEN: declare and re-export the gated module.**
  Edit `crates/rb-embed/src/lib.rs` so the full file reads exactly:

  ```rust
  //! `rb_embed`: embedding providers for rusty-brain.
  //!
  //! Defines the `EmbeddingProvider` trait, the remote `VoyageProvider`,
  //! a public offline `DeterministicProvider` used as a no-API-key fallback
  //! and in tests, and (under the `local` feature) the `LocalProvider` for
  //! offline ONNX embeddings via fastembed.
  #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

  mod deterministic;
  #[cfg(feature = "local")]
  mod local;
  mod provider;
  mod voyage;

  pub use deterministic::DeterministicProvider;
  #[cfg(feature = "local")]
  pub use local::LocalProvider;
  pub use provider::EmbeddingProvider;
  pub use voyage::VoyageProvider;

  #[cfg(all(test, feature = "local"))]
  mod local_export_tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use crate::{EmbeddingProvider, LocalProvider};

      #[test]
      fn local_provider_is_publicly_re_exported() {
          // Constructing via the crate-root path proves the `pub use` wiring.
          let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
          assert_eq!(p.dim(), 384);
          assert_eq!(p.model_id(), "all-MiniLM-L6-v2");
      }
  }
  ```

- [ ] **Step 4: run it — Run:** `cargo test -p rb-embed --features local local_export_tests` (Expected: PASS, 1 test) then **Run:** `cargo test -p rb-embed` (Expected: PASS — the default build ignores the gated module entirely and all existing tests still pass).

- [ ] **Step 5: lint+format — Run:** `cargo clippy -p rb-embed --features local --all-targets -- -D warnings` (Expected: no warnings), then `cargo clippy -p rb-embed --all-targets -- -D warnings` (Expected: no warnings — default build), then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit — Run:** `git add crates/rb-embed/src/lib.rs && git commit -m "feat(rb-embed): re-export LocalProvider under the local feature"` — Expected: one commit.

---

### Task U4: rusty-brain `Cargo.toml` + `serve.rs` — Local provider arm

**Files:**
- Modify: crates/rusty-brain/Cargo.toml
- Modify: crates/rusty-brain/src/serve.rs

- [ ] **Step 1 RED: write the failing test.**
  Replace the existing `#[cfg(test)] mod tests` block at the bottom of `crates/rusty-brain/src/serve.rs` with the block below. It updates every existing `select_provider_kind` call to the new two-argument signature and adds coverage for the `Local` precedence and the without-feature error path. Full verbatim:

  ```rust
  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn selects_voyage_when_key_present_and_local_not_requested() {
          let sel = select_provider_kind(Some("vk-123".to_string()), false);
          assert_eq!(sel, ProviderKind::Voyage);
      }

      #[test]
      fn selects_deterministic_when_key_absent_and_local_not_requested() {
          let sel = select_provider_kind(None, false);
          assert_eq!(sel, ProviderKind::Deterministic);
      }

      #[test]
      fn selects_deterministic_when_key_empty_and_local_not_requested() {
          let sel = select_provider_kind(Some(String::new()), false);
          assert_eq!(sel, ProviderKind::Deterministic);
      }

      #[test]
      fn selects_deterministic_when_key_is_whitespace_and_local_not_requested() {
          let sel = select_provider_kind(Some("   ".to_string()), false);
          assert_eq!(sel, ProviderKind::Deterministic);
      }

      #[test]
      fn local_requested_takes_precedence_over_voyage() {
          // Precedence is local > voyage > deterministic: even with a key,
          // an explicit local request wins.
          let sel = select_provider_kind(Some("vk-123".to_string()), true);
          assert_eq!(sel, ProviderKind::Local);
      }

      #[test]
      fn local_requested_takes_precedence_over_deterministic() {
          let sel = select_provider_kind(None, true);
          assert_eq!(sel, ProviderKind::Local);
      }

      #[cfg(not(feature = "local"))]
      #[tokio::test(flavor = "multi_thread")]
      async fn local_selected_without_feature_is_an_embedding_error() {
          // When `local` is requested but the crate was built WITHOUT the
          // feature, run_serve must fail closed with Error::Embedding rather
          // than silently falling back to another provider.
          let dir = tempfile::tempdir().unwrap();
          let socket = dir.path().join("rb.sock");
          let db = dir.path().join("rb.sqlite");
          let err = run_with_kind(
              ProviderKind::Local,
              socket,
              db,
              4,
              std::future::ready(()),
          )
          .await
          .unwrap_err();
          assert!(
              matches!(err, rb_types::Error::Embedding(_)),
              "expected Error::Embedding when local feature is absent, got {err:?}"
          );
      }
  }
  ```

- [ ] **Step 2: run it — Run:** `cargo test -p rusty-brain serve::tests` — Expected: FAIL to compile: `this function takes 1 argument but 2 arguments were supplied` (old `select_provider_kind`), `no variant named Local`, and `cannot find function run_with_kind`.

- [ ] **Step 3 GREEN: implement the new signature, the gated variant, and the dispatch helper.**
  Add `local` as a passthrough feature to `crates/rusty-brain/Cargo.toml`. Insert a `[features]` table immediately after the `[lib]` block so the relevant section reads:

  ```toml
  [lib]
  name = "rusty_brain"
  path = "src/lib.rs"

  [features]
  # Forward the local ONNX embedding backend to rb-embed. OFF by default;
  # enabled in CI by the `build-local` job and at runtime via `--features local`.
  local = ["rb-embed/local"]

  [dependencies]
  ```

  Then rewrite `crates/rusty-brain/src/serve.rs` so the full file reads exactly:

  ```rust
  //! `serve` subcommand: bind the daemon and run until Ctrl-C.

  use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
  use rb_embed::{DeterministicProvider, EmbeddingProvider, VoyageProvider};
  use rb_types::Result;
  use std::path::PathBuf;

  /// Default embedding dimension for the offline provider and Voyage's default model.
  pub const DEFAULT_DIM: usize = 512;

  /// Which embedding provider `serve` will use.
  ///
  /// `Local` is always present in the enum (so selection logic is uniform), but
  /// it can only be *constructed and run* when the crate is built with the
  /// `local` feature. Selecting `Local` without the feature is a fail-closed
  /// `Error::Embedding` in `run_with_kind` — never a silent fallback.
  #[derive(Debug, PartialEq, Eq, Clone, Copy)]
  pub enum ProviderKind {
      Local,
      Voyage,
      Deterministic,
  }

  /// Pure selection with precedence `local > voyage > deterministic`.
  /// `local_requested` comes from the environment (see [`run_serve`]); Voyage is
  /// chosen iff a non-empty API key is present, otherwise Deterministic.
  pub fn select_provider_kind(api_key: Option<String>, local_requested: bool) -> ProviderKind {
      if local_requested {
          return ProviderKind::Local;
      }
      match api_key {
          Some(k) if !k.trim().is_empty() => ProviderKind::Voyage,
          _ => ProviderKind::Deterministic,
      }
  }

  /// Read whether the local backend was requested via the environment.
  /// True when `RB_EMBED_BACKEND=local` (case-insensitive) or `RB_LOCAL_MODEL`
  /// is set to a non-empty value.
  fn local_requested_from_env() -> bool {
      let backend = std::env::var("RB_EMBED_BACKEND")
          .ok()
          .map(|v| v.trim().eq_ignore_ascii_case("local"))
          .unwrap_or(false);
      let model_set = std::env::var("RB_LOCAL_MODEL")
          .ok()
          .map(|v| !v.trim().is_empty())
          .unwrap_or(false);
      backend || model_set
  }

  /// Run the daemon at the given paths until `shutdown` resolves.
  /// Picks the embedding provider from the environment (`RB_EMBED_BACKEND` /
  /// `RB_LOCAL_MODEL` for local, `VOYAGE_API_KEY` for Voyage).
  pub async fn run_serve(
      socket_path: PathBuf,
      db_path: PathBuf,
      read_pool_size: usize,
      shutdown: impl std::future::Future<Output = ()>,
  ) -> Result<()> {
      let api_key = std::env::var("VOYAGE_API_KEY").ok();
      let kind = select_provider_kind(api_key, local_requested_from_env());
      run_with_kind(kind, socket_path, db_path, read_pool_size, shutdown).await
  }

  /// Construct the concrete provider for `kind` and run the daemon to shutdown.
  /// Selecting `Local` without the `local` feature is a fail-closed error.
  async fn run_with_kind(
      kind: ProviderKind,
      socket_path: PathBuf,
      db_path: PathBuf,
      read_pool_size: usize,
      shutdown: impl std::future::Future<Output = ()>,
  ) -> Result<()> {
      match kind {
          ProviderKind::Local => {
              #[cfg(feature = "local")]
              {
                  let model_name = std::env::var("RB_LOCAL_MODEL").unwrap_or_default();
                  tracing::info!(
                      "using local ONNX embeddings via fastembed (model '{}'); \
                       weights download at runtime on first use",
                      if model_name.trim().is_empty() {
                          "all-MiniLM-L6-v2"
                      } else {
                          model_name.as_str()
                      }
                  );
                  let embedder = rb_embed::LocalProvider::load(&model_name)?;
                  run_with_embedder(socket_path, db_path, read_pool_size, embedder, shutdown).await
              }
              #[cfg(not(feature = "local"))]
              {
                  let _ = (socket_path, db_path, read_pool_size, shutdown);
                  Err(rb_types::Error::Embedding(
                      "local embedding backend requested but this binary was built \
                       without the `local` feature; rebuild with `--features local`"
                          .to_string(),
                  ))
              }
          }
          ProviderKind::Voyage => {
              let embedder = VoyageProvider::from_env()?;
              run_with_embedder(socket_path, db_path, read_pool_size, embedder, shutdown).await
          }
          ProviderKind::Deterministic => {
              tracing::warn!(
                  "VOYAGE_API_KEY not set; using offline DeterministicProvider \
                   (dim {DEFAULT_DIM}). Recall quality is reduced and embeddings \
                   are not portable to a real model."
              );
              let embedder = DeterministicProvider::new(DEFAULT_DIM);
              run_with_embedder(socket_path, db_path, read_pool_size, embedder, shutdown).await
          }
      }
  }

  /// Bind a daemon for a concrete embedder and run it to shutdown.
  async fn run_with_embedder<P>(
      socket_path: PathBuf,
      db_path: PathBuf,
      read_pool_size: usize,
      embedder: P,
      shutdown: impl std::future::Future<Output = ()>,
  ) -> Result<()>
  where
      P: EmbeddingProvider + 'static,
  {
      let config = DaemonConfig {
          socket_path,
          db_path,
          read_pool_size,
      };
      let daemon = Daemon::bind(config, SharedEmbedder::new(embedder)).await?;
      daemon.run(shutdown).await
  }

  #[cfg(test)]
  mod tests {
      #![allow(clippy::unwrap_used, clippy::expect_used)]
      use super::*;

      #[test]
      fn selects_voyage_when_key_present_and_local_not_requested() {
          let sel = select_provider_kind(Some("vk-123".to_string()), false);
          assert_eq!(sel, ProviderKind::Voyage);
      }

      #[test]
      fn selects_deterministic_when_key_absent_and_local_not_requested() {
          let sel = select_provider_kind(None, false);
          assert_eq!(sel, ProviderKind::Deterministic);
      }

      #[test]
      fn selects_deterministic_when_key_empty_and_local_not_requested() {
          let sel = select_provider_kind(Some(String::new()), false);
          assert_eq!(sel, ProviderKind::Deterministic);
      }

      #[test]
      fn selects_deterministic_when_key_is_whitespace_and_local_not_requested() {
          let sel = select_provider_kind(Some("   ".to_string()), false);
          assert_eq!(sel, ProviderKind::Deterministic);
      }

      #[test]
      fn local_requested_takes_precedence_over_voyage() {
          // Precedence is local > voyage > deterministic: even with a key,
          // an explicit local request wins.
          let sel = select_provider_kind(Some("vk-123".to_string()), true);
          assert_eq!(sel, ProviderKind::Local);
      }

      #[test]
      fn local_requested_takes_precedence_over_deterministic() {
          let sel = select_provider_kind(None, true);
          assert_eq!(sel, ProviderKind::Local);
      }

      #[cfg(not(feature = "local"))]
      #[tokio::test(flavor = "multi_thread")]
      async fn local_selected_without_feature_is_an_embedding_error() {
          // When `local` is requested but the crate was built WITHOUT the
          // feature, run_serve must fail closed with Error::Embedding rather
          // than silently falling back to another provider.
          let dir = tempfile::tempdir().unwrap();
          let socket = dir.path().join("rb.sock");
          let db = dir.path().join("rb.sqlite");
          let err = run_with_kind(
              ProviderKind::Local,
              socket,
              db,
              4,
              std::future::ready(()),
          )
          .await
          .unwrap_err();
          assert!(
              matches!(err, rb_types::Error::Embedding(_)),
              "expected Error::Embedding when local feature is absent, got {err:?}"
          );
      }
  }
  ```

- [ ] **Step 4: run it — Run:** `cargo test -p rusty-brain serve::tests` (Expected: PASS, 7 tests — the without-feature error test is compiled because the default build has no `local` feature) then **Run:** `cargo test -p rusty-brain --features local serve::tests` (Expected: PASS, 6 tests — the `#[cfg(not(feature = "local"))]` error test is excluded, the 6 selection tests pass).

- [ ] **Step 5: lint+format — Run:** `cargo clippy -p rusty-brain --all-targets -- -D warnings` (Expected: no warnings), then `cargo clippy -p rusty-brain --features local --all-targets -- -D warnings` (Expected: no warnings), then `cargo fmt --all` (Expected: no diff).

- [ ] **Step 6: commit — Run:** `git add crates/rusty-brain/Cargo.toml crates/rusty-brain/src/serve.rs && git commit -m "feat(rusty-brain): wire Local provider arm behind local feature"` — Expected: one commit.

---

### Task U5: CI `ci.yml` — build-local job

**Files:**
- Modify: .github/workflows/ci.yml

- [ ] **Step 1 RED: assert the new job exists.**
  No Rust test applies; the RED check greps the workflow for the new job. **Run:** `grep -n "build-local" .github/workflows/ci.yml` — Expected: FAIL (grep exits 1, no output) because the job does not exist yet.

- [ ] **Step 2: run it — Run:** `grep -n "build-local" .github/workflows/ci.yml` — Expected: FAIL (no match).

- [ ] **Step 3 GREEN: add the `build-local` job.**
  Edit `.github/workflows/ci.yml` so the full file reads exactly (the existing `clippy-test`, `deny`, `audit`, and `fmt` jobs are unchanged; only the `build-local` job is appended):

  ```yaml
  name: CI

  on:
    push:
      branches: [main]
    pull_request:
      branches: [main]

  env:
    CARGO_TERM_COLOR: always
    RUSTFLAGS: "-D warnings"

  jobs:
    fmt:
      name: rustfmt
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - name: Check formatting
          run: cargo fmt --all --check

    clippy-test:
      name: clippy + test
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - uses: Swatinem/rust-cache@v2
        - name: Clippy
          run: cargo clippy --workspace --all-targets --all-features -- -D warnings
        - name: Test
          run: cargo test --workspace

    build-local:
      name: build + clippy (local feature)
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - uses: Swatinem/rust-cache@v2
        - name: Build with local feature
          run: cargo build -p rusty-brain --features local
        - name: Clippy with local feature
          run: cargo clippy -p rusty-brain --features local --all-targets -- -D warnings
        - name: Test rb-embed local (offline only; real-model tests are #[ignore])
          run: cargo test -p rb-embed --features local

    deny:
      name: cargo-deny
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - uses: EmbarkStudios/cargo-deny-action@v2
          with:
            command: check

    audit:
      name: cargo-audit
      runs-on: ubuntu-latest
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
        - uses: rustsec/audit-check@v2
          with:
            token: ${{ secrets.GITHUB_TOKEN }}
  ```

- [ ] **Step 4: run it — Run:** `grep -n "build-local" .github/workflows/ci.yml` (Expected: PASS — matches the job key) then validate the YAML parses and the default job is intact: **Run:** `python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); j=d['jobs']; assert 'build-local' in j; assert j['clippy-test']['steps'][-1]['run']=='cargo test --workspace'; print('ok', sorted(j))"` (Expected: prints `ok ['audit', 'build-local', 'clippy-test', 'deny', 'fmt']`, confirming the default `clippy-test` job's `cargo test --workspace` is unchanged and has NO `--all-features`).

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` — Expected: no diff (workflow edit touches no Rust). No clippy applies to YAML.

- [ ] **Step 6: commit — Run:** `git add .github/workflows/ci.yml && git commit -m "ci: add build-local job for the local embedding feature"` — Expected: one commit.

---

### Task U6: `deny.toml` — verify licenses for the local closure

**Files:**
- Modify: deny.toml (only if `cargo deny check` flags a missing license)

- [ ] **Step 1 RED: run cargo-deny against the all-features closure (which now includes fastembed).**
  Because `deny.toml` sets `all-features = true`, cargo-deny already sees the fastembed/ort/hf-hub/tokenizers tree. **Run:** `cargo deny check 2>&1 | tee /tmp/rb-deny-u6.log; echo "exit=${PIPESTATUS[0]}"` — Expected: this is the verification gate. fastembed and its known tree (ort, tokenizers, hf-hub) are Apache-2.0/MIT, all already in the allow-list, so this is EXPECTED to pass (`exit=0`). If it fails on a license, the failing crate + SPDX id is printed in the log.

- [ ] **Step 2: run it — Run:** `cargo deny check licenses 2>&1 | tail -40` — Expected: `licenses ok` (no `rejected` lines). If instead a license such as `Zlib`, `OpenSSL`, or `BSL-1.0` is reported as not in the allow-list, note the exact SPDX identifier from the output for Step 3.

- [ ] **Step 3 GREEN: add only the missing license(s), if any.**
  If Step 2 passed (`licenses ok`), make NO change to `deny.toml` and skip to Step 6 with nothing to commit (record "no deny.toml change required" and proceed to the Part gate). If a permissive license was flagged, add ONLY that exact SPDX id to the `allow` array in `deny.toml`. For example, were `Zlib` flagged, the `[licenses]` `allow` block would become:

  ```toml
  allow = [
      "MIT",
      "Apache-2.0",
      "Apache-2.0 WITH LLVM-exception",
      "BSD-2-Clause",
      "BSD-3-Clause",
      "ISC",
      "Unicode-3.0",
      "Unicode-DFS-2016",
      "CC0-1.0",
      "CDLA-Permissive-2.0",
      "Zlib",
  ]
  ```

  Only add a license that is genuinely permissive (no copyleft); if a copyleft license appears, STOP and flag it rather than allow-listing it.

- [ ] **Step 4: run it — Run:** `cargo deny check 2>&1 | tail -20; echo "exit=${PIPESTATUS[0]}"` — Expected: `exit=0`, `licenses ok`, `advisories ok`, `sources ok` (bans may emit `warn` for duplicate versions, which is non-fatal per `multiple-versions = "warn"`).

- [ ] **Step 5: lint+format — Run:** `cargo fmt --all --check` — Expected: no diff (TOML-only change touches no Rust).

- [ ] **Step 6: commit (only if `deny.toml` changed) — Run:** `git add deny.toml && git commit -m "chore: allow-list permissive license required by fastembed closure"` — Expected: one commit IF a license was added; otherwise no commit (record that no change was needed).

---

### Task U7: Part U gate

**Files:**
- (none — verification only)

- [ ] **Step 1: default workspace tests — Run:** `cargo test --workspace` — Expected: PASS, 0 failures. Confirms the default closure (NO `local` feature) still builds and tests cleanly and that fastembed was never compiled for this job.

- [ ] **Step 2: verify the default closure is fastembed-free — Run:** `cargo tree -e no-dev --workspace 2>/dev/null | grep -i fastembed; echo "exit=$?"` — Expected: `exit=1` (grep found nothing): the default `cargo build`/`cargo test` closure does NOT include fastembed/ort/onnxruntime.

- [ ] **Step 3: all-features clippy (this DOES pull fastembed, matching CI) — Run:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — Expected: no warnings (fastembed-backed `local` module compiles and lints clean under `--all-features`).

- [ ] **Step 4: formatting — Run:** `cargo fmt --all --check` — Expected: no diff.

- [ ] **Step 5: local feature build + offline tests (mirrors the `build-local` CI job) — Run:** `cargo build -p rusty-brain --features local` (Expected: PASS — links fastembed) then `cargo test -p rb-embed --features local` (Expected: PASS — offline tests pass; the real-model test stays `#[ignore]`).

- [ ] **Step 6: supply-chain gate (Part touches deps) — Run:** `cargo deny check` — Expected: ok (`licenses ok`, `advisories ok`, `sources ok`; `bans` may `warn` only). This confirms the fastembed/ort/hf-hub/tokenizers licenses are accepted.

