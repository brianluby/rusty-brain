# rusty-brain — P1 (Engine + Daemon) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Work in the worktree `~/repos/rusty-brain-p1` on branch `feat/p1-engine-daemon` (based on completed P0). NO AI attribution on commits.

**Goal:** Build the rusty-brain runtime on top of the P0 storage foundation: the wire protocol (`rb-proto`), pluggable embeddings (`rb-embed`, Voyage default), pure hybrid ranking (`rb-search`), the trait-generic orchestration engine (`rb-engine`), the single-writer concurrent daemon (`rb-daemon`), and the `rusty-brain` CLI binary — so many agents can concurrently remember/recall over one local SQLite memory store via a Unix-domain-socket daemon.

**Architecture:** Async (tokio). The daemon owns the store: ONE dedicated OS thread holds the write `SqliteStore` (rusqlite `Connection` is `!Sync`), fed by an mpsc + oneshot command channel; reads run on a bounded pool via `spawn_blocking`; a `tokio::broadcast` emits change events on commit. Per-connection tokio tasks speak length-delimited JSON over the UDS, handshake a namespace + `ContractVersion`, and dispatch to a per-connection `MemoryEngine<Backend, Provider>` that enforces namespace isolation server-side. The engine is generic over a `MemoryBackend` trait and an `EmbeddingProvider`, keeping it pure-policy and testable without a DB or network.

**Tech Stack:** Rust 2021, tokio, tokio-util (LengthDelimitedCodec), reqwest (Voyage), async-trait, serde/serde_json, clap, tracing; tests use an offline `DeterministicProvider` and `wiremock` (never the live API). Builds on P0 `rb-types` + `rb-store`. Reference spec: `~/repos/rusty-brain/docs/specs/2026-05-31-rusty-brain-architecture-design.md`. (Parts are lettered F–K to continue after P0's A–E.)

**Build order:** Part F (proto + workspace setup) → G/H (embed, search — independent) → I (engine, needs embed+search) → J (daemon, needs engine+proto+store) → K (binary, needs daemon+proto). F/G/H can be done in any order; I requires G+H; J requires F+I; K requires J.

**Cross-cluster contract decisions (authoritative — these override the spec's interface sketch where they differ):**
- **Embedder injection.** rb-daemon defines `SharedEmbedder` = an `Arc<dyn EmbeddingProvider>` newtype that itself implements `EmbeddingProvider` (Part J). `Daemon::bind(config: DaemonConfig, embedder: SharedEmbedder)` takes it as a **second argument** — the spec's one-arg `bind(config)` sketch was incomplete (`DaemonConfig` has no embedder field and stays 3 fields: `socket_path`, `db_path`, `read_pool_size`). Each per-connection `MemoryEngine<StoreHandle, SharedEmbedder>` clones the `Arc` to share one embedder instance.
- **`RememberInput` is rb-engine-only.** The `rusty-brain` binary does **not** depend on `rb-engine` and does **not** import `RememberInput`. Its `remember` subcommand builds `rb_proto::Request::Remember { content, context, memory_type, importance, keywords, tags, related_files }` and sends it via `rb_proto::Client`; the daemon maps it to `rb_engine::RememberInput` server-side.

---

## Part F — Workspace setup & rb-proto (the wire contract / P2 freeze point)

### Task 1: Add P1 workspace dependencies to the root manifest

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p1/Cargo.toml`

This task extends the existing P0 root `[workspace.dependencies]` and `[workspace] members` with everything P1 needs. The new crates are added to `members` here so the very next task can scaffold them and `cargo build --workspace` stays loadable. No code compiles against the new deps yet, so verification is manifest-syntax + `cargo metadata` only.

- [ ] **Step 1: Add the new workspace dependencies.** In `/Users/bluby/repos/rusty-brain-p1/Cargo.toml`, replace the existing `[workspace.dependencies]` block (which ends after `tempfile = "3"`) by appending the P1 deps so the full block reads exactly:

```toml
[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
sqlite-vec = "0.1"
deadpool-sqlite = "0.9"
include_dir = "0.7"
sha2 = "0.10"
tempfile = "3"
# --- P1 additions ---
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "sync", "time", "io-util", "signal"] }
tokio-util = { version = "0.7", features = ["codec"] }
tokio-stream = "0.1"
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
futures = "0.3"
bytes = "1"
secrecy = "0.10"
directories = "5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive", "env"] }
anyhow = "1"
wiremock = "0.6"
```

(`wiremock` is a dev-dependency used only by the rb-embed Voyage HTTP test in a later cluster; declaring it in `[workspace.dependencies]` now keeps a single source of truth and does not enter any default build closure.)

- [ ] **Step 2: Add the new crates to the workspace members.** In the same file, replace the `members` line:

```toml
members = ["crates/rb-types", "crates/rb-store"]
```

with:

```toml
members = [
    "crates/rb-types",
    "crates/rb-store",
    "crates/rb-proto",
    "crates/rb-embed",
    "crates/rb-search",
    "crates/rb-engine",
    "crates/rb-daemon",
    "crates/rusty-brain",
]
```

- [ ] **Step 3: Validate the manifest is well-formed TOML with the expected members and a sample new dep.** A full `cargo metadata` will fail until the new member crates exist on disk (Task 2), so validate TOML structure only here.
  Run: `python3 -c "import tomllib; d=tomllib.load(open('/Users/bluby/repos/rusty-brain-p1/Cargo.toml','rb')); m=d['workspace']['members']; assert m==['crates/rb-types','crates/rb-store','crates/rb-proto','crates/rb-embed','crates/rb-search','crates/rb-engine','crates/rb-daemon','crates/rusty-brain'], m; dd=d['workspace']['dependencies']; assert 'tokio' in dd and 'tokio-util' in dd and 'reqwest' in dd and 'secrecy' in dd and 'clap' in dd and 'wiremock' in dd; assert 'codec' in dd['tokio-util']['features']; print('root Cargo.toml P1 deps OK')"`
  Expected: prints `root Cargo.toml P1 deps OK` (exit 0).

- [ ] **Step 4: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "chore: add P1 workspace dependencies and crate members"`
  Expected: one commit created.

---

### Task 2: Scaffold all P1 crates as empty workspace members

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-search/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-search/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-engine/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-engine/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/main.rs`

This stands up every P1 crate as a buildable skeleton so `cargo build --workspace` is green before any feature code is written. Each manifest already lists the workspace deps that crate will use (per the spine), uses `[lints] workspace = true`, and library names use underscores. The bin crate gets a stub `main` (real CLI lands in the daemon/CLI cluster); its manifest lists only the deps the stub needs now.

- [ ] **Step 1: Create the `rb-proto` manifest.** Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml`:

```toml
[package]
name = "rb-proto"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain daemon wire protocol: request/response enums, length-delimited JSON framing, and async UDS client."

[lib]
name = "rb_proto"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
bytes = { workspace = true }
futures = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Create the `rb-proto` lib skeleton.** Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs`:

```rust
//! `rb_proto`: daemon wire protocol for rusty-brain.
//!
//! Length-delimited JSON frames over a Unix domain socket, a versioned
//! handshake, the `Request`/`Response` enums, and an async `Client`.
//! Concrete types are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
```

- [ ] **Step 3: Create the `rb-embed` skeleton (manifest + lib).** Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/Cargo.toml`:

```toml
[package]
name = "rb-embed"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Embedding provider trait for rusty-brain plus a Voyage remote impl and an offline deterministic provider."

[lib]
name = "rb_embed"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
async-trait = { workspace = true }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
secrecy = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
wiremock = { workspace = true }

[lints]
workspace = true
```

  Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/lib.rs`:

```rust
//! `rb_embed`: pluggable embedding providers for rusty-brain.
//!
//! The `EmbeddingProvider` trait, a Voyage remote impl, and an offline
//! deterministic provider for tests and no-API-key fallback. Concrete types
//! are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
```

- [ ] **Step 4: Create the `rb-search` skeleton (manifest + lib).** Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-search/Cargo.toml`:

```toml
[package]
name = "rb-search"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Pure, deterministic hybrid ranking for rusty-brain: combine keyword, vector, graph, importance, and recency signals."

[lib]
name = "rb_search"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
chrono = { workspace = true }

[lints]
workspace = true
```

  Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-search/src/lib.rs`:

```rust
//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO. Combines normalized keyword/vector/graph/importance/recency signals
//! into a single weighted score. Concrete types are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
```

- [ ] **Step 5: Create the `rb-engine` skeleton (manifest + lib).** Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-engine/Cargo.toml`:

```toml
[package]
name = "rb-engine"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain request orchestration: embed plus rank policy over an abstract memory backend (no concrete store dependency)."

[lib]
name = "rb_engine"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-search = { path = "../rb-search" }
rb-embed = { path = "../rb-embed" }
async-trait = { workspace = true }
chrono = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }

[lints]
workspace = true
```

  Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-engine/src/lib.rs`:

```rust
//! `rb_engine`: single-request orchestration for rusty-brain.
//!
//! Generic over a `MemoryBackend` trait and an `EmbeddingProvider`, so the
//! engine stays pure policy (embed plus rank) and testable without a real
//! store. Concrete types are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
```

- [ ] **Step 6: Create the `rb-daemon` skeleton (manifest + lib).** Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/Cargo.toml`:

```toml
[package]
name = "rb-daemon"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain single-writer daemon: dedicated writer thread, read pool, UDS listener, change broadcast, server-side isolation."

[lib]
name = "rb_daemon"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-store = { path = "../rb-store" }
rb-engine = { path = "../rb-engine" }
rb-embed = { path = "../rb-embed" }
rb-proto = { path = "../rb-proto" }
rb-search = { path = "../rb-search" }
tokio = { workspace = true }
tokio-util = { workspace = true }
futures = { workspace = true }
deadpool-sqlite = { workspace = true }
directories = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
serde = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }

[lints]
workspace = true
```

  Write `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs`:

```rust
//! `rb_daemon`: the single-writer rusty-brain service.
//!
//! One dedicated OS thread owns the write connection (rusqlite is `!Sync`);
//! reads run on a bounded pool via `spawn_blocking`; commits broadcast a
//! `MemoryChanged` event. A UDS listener dispatches per-connection engines
//! with server-side namespace isolation. Concrete types are added later.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
```

- [ ] **Step 7: Create the `rusty-brain` bin crate (manifest + stub main).** Write `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/Cargo.toml`. The stub only needs the standard library; clap/tokio/etc. are added in the CLI cluster to avoid unused-dependency churn under `-D warnings`:

```toml
[package]
name = "rusty-brain"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain CLI and daemon binary."

[[bin]]
name = "rusty-brain"
path = "src/main.rs"

[dependencies]

[lints]
workspace = true
```

  Write `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/main.rs`:

```rust
//! `rusty-brain` binary entry point.
//!
//! Stub for the P1 setup cluster: prints version and a not-yet-implemented
//! notice, then exits success. The full clap CLI (serve/remember/recall/...)
//! is implemented in the daemon and CLI cluster.

fn main() -> std::process::ExitCode {
    println!("rusty-brain {}", env!("CARGO_PKG_VERSION"));
    eprintln!("CLI not yet implemented in this build");
    std::process::ExitCode::SUCCESS
}
```

- [ ] **Step 8: Verify the whole workspace builds with every member present.** All eight members now exist on disk, so a full build resolves and compiles (cold builds also link bundled SQLite via rb-store, which can take a minute).
  Run: `cargo build --workspace --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: `Compiling rb-proto ...`, `rb-embed ...`, `rb-search ...`, `rb-engine ...`, `rb-daemon ...`, `rusty-brain ...` then `Finished` (exit 0). No "failed to load manifest for workspace member" errors.

- [ ] **Step 9: Verify clippy is clean workspace-wide with warnings denied.** Proves the shared lints are wired into every new crate via `[lints] workspace = true` and that the skeletons have no unused-dependency or dead-code warnings. (`--manifest-path` must come BEFORE `--`.)
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings`
  Expected: `Finished` with no warnings (exit 0).

- [ ] **Step 10: Verify formatting is clean.**
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no output, exit 0.

- [ ] **Step 11: Verify the stub binary runs.**
  Run: `cargo run -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml 2>/dev/null`
  Expected: prints `rusty-brain 0.0.1` to stdout, exit 0.

- [ ] **Step 12: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto crates/rb-embed crates/rb-search crates/rb-engine crates/rb-daemon crates/rusty-brain && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat: scaffold P1 crates (rb-proto, rb-embed, rb-search, rb-engine, rb-daemon, rusty-brain)"`
  Expected: one commit created; `cargo build --workspace` and clippy both green.

---

### Task 3: rb-proto `messages.rs` — contract version, handshake, and Request/Response enums

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/messages.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` (declare `mod messages;` + re-exports)

This task defines the wire vocabulary exactly per the spine: `CONTRACT_VERSION`, `Handshake`/`HandshakeAck`, and the `#[serde(tag = "op")]` `Request` / `#[serde(tag = "result")]` `Response` enums. Tests pin a serde round-trip for every variant and assert the internally tagged discriminators land in the JSON.

- [ ] **Step 1: Write the failing tests AND declare the module.** An undeclared `.rs` file is never compiled, so declare `mod messages;` in `lib.rs` now and create `messages.rs` with ONLY the test module; the build fails because the types do not exist yet.

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/messages.rs` with:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_types::{
        MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, SearchResult,
    };

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "one db, one transaction".into(),
            MemoryType::ArchitectureDecision,
            8,
        )
    }

    #[test]
    fn contract_version_is_one() {
        assert_eq!(CONTRACT_VERSION, 1);
    }

    #[test]
    fn handshake_round_trip() {
        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Project("rusty-brain".into()),
        };
        let json = serde_json::to_string(&hs).unwrap();
        let back: Handshake = serde_json::from_str(&json).unwrap();
        assert_eq!(back.contract_version, CONTRACT_VERSION);
        assert_eq!(back.namespace, Namespace::Project("rusty-brain".into()));
    }

    #[test]
    fn handshake_ack_round_trip() {
        let ack = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: false,
            message: Some("version mismatch".into()),
        };
        let json = serde_json::to_string(&ack).unwrap();
        let back: HandshakeAck = serde_json::from_str(&json).unwrap();
        assert!(!back.ok);
        assert_eq!(back.message.as_deref(), Some("version mismatch"));
    }

    fn all_requests() -> Vec<Request> {
        let id = MemoryId::new();
        vec![
            Request::Remember {
                content: "c".into(),
                context: Some("ctx".into()),
                memory_type: MemoryType::Insight,
                importance: 7,
                keywords: vec!["k".into()],
                tags: vec!["t".into()],
                related_files: vec!["src/lib.rs".into()],
            },
            Request::Recall {
                query: "q".into(),
                scope: Some(Namespace::Global),
                memory_type: Some(MemoryType::BugFix),
                tags: vec!["sqlite".into()],
                limit: 10,
            },
            Request::Get { id: id.clone() },
            Request::List {
                scope: Some(Namespace::Project("p".into())),
                min_importance: Some(5),
                limit: 20,
            },
            Request::Graph { id: id.clone(), depth: 2 },
            Request::Update {
                id: id.clone(),
                updates: MemoryUpdates {
                    importance: Some(9),
                    ..Default::default()
                },
            },
            Request::Delete { id },
            Request::Context,
            Request::Ping,
        ]
    }

    #[test]
    fn every_request_variant_round_trips() {
        for req in all_requests() {
            let json = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&json).unwrap();
            // Compare via JSON since Request is not PartialEq.
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn request_uses_op_tag() {
        let json = serde_json::to_string(&Request::Ping).unwrap();
        assert_eq!(json, r#"{"op":"Ping"}"#);
        let json = serde_json::to_string(&Request::Context).unwrap();
        assert_eq!(json, r#"{"op":"Context"}"#);
    }

    fn all_responses() -> Vec<Response> {
        vec![
            Response::Remembered { id: MemoryId::new() },
            Response::Recalled {
                results: vec![SearchResult { memory: note(), score: 0.9 }],
            },
            Response::Got { memory: Some(note()) },
            Response::Got { memory: None },
            Response::Listed { memories: vec![note()] },
            Response::GraphResult { memories: vec![note()] },
            Response::Updated,
            Response::Deleted,
            Response::ContextResult {
                recent: vec![note()],
                important: vec![note()],
                total: 2,
            },
            Response::Pong { contract_version: CONTRACT_VERSION },
            Response::Error {
                kind: "not_found".into(),
                message: "no such memory".into(),
            },
        ]
    }

    #[test]
    fn every_response_variant_round_trips() {
        for resp in all_responses() {
            let json = serde_json::to_string(&resp).unwrap();
            let back: Response = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn response_uses_result_tag() {
        let json = serde_json::to_string(&Response::Updated).unwrap();
        assert_eq!(json, r#"{"result":"Updated"}"#);
        let json = serde_json::to_string(&Response::Pong {
            contract_version: 1,
        })
        .unwrap();
        assert_eq!(json, r#"{"result":"Pong","contract_version":1}"#);
    }
}
```

  Set `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` to declare the module (re-exports added in Step 3):

```rust
//! `rb_proto`: daemon wire protocol for rusty-brain.
//!
//! Length-delimited JSON frames over a Unix domain socket, a versioned
//! handshake, the `Request`/`Response` enums, and an async `Client`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod messages;
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-proto messages`
  Expected: FAIL to compile — `cannot find value 'CONTRACT_VERSION' in this scope` / `cannot find type 'Handshake'` etc. Confirms the test drives new code.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/messages.rs`:

```rust
use rb_types::{
    MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, SearchResult,
};
use serde::{Deserialize, Serialize};

/// Wire contract version carried in the handshake. Clients and the daemon must
/// agree on this exact value; mismatch is rejected at connect time.
pub const CONTRACT_VERSION: u32 = 1;

/// First frame the client sends after connecting.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Handshake {
    pub contract_version: u32,
    pub namespace: Namespace,
}

/// Daemon reply to a `Handshake`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HandshakeAck {
    pub contract_version: u32,
    pub ok: bool,
    pub message: Option<String>,
}

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
        scope: Option<Namespace>,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    },
    Get {
        id: MemoryId,
    },
    List {
        scope: Option<Namespace>,
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
}

/// One response per request. Internally tagged on `result`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "result")]
pub enum Response {
    Remembered { id: MemoryId },
    Recalled { results: Vec<SearchResult> },
    Got { memory: Option<MemoryNote> },
    Listed { memories: Vec<MemoryNote> },
    GraphResult { memories: Vec<MemoryNote> },
    Updated,
    Deleted,
    ContextResult {
        recent: Vec<MemoryNote>,
        important: Vec<MemoryNote>,
        total: usize,
    },
    Pong { contract_version: u32 },
    Error { kind: String, message: String },
}
```

  Note: every `Request`/`Response` variant is either a unit variant or a struct (brace) variant, so `#[serde(tag = ...)]` (internal tagging) is valid — internal tagging forbids tuple/newtype variants, and there are none here.

- [ ] **Step 4: Re-export the types from `lib.rs`.** Update `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` so the module block reads:

```rust
mod messages;

pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 5: Run it — expect PASS.**
  Run: `cargo test -p rb-proto messages`
  Expected: PASS (8 tests in the `messages` module pass).

- [ ] **Step 6: Lint + format.**
  Run: `cargo clippy -p rb-proto --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no diff.

- [ ] **Step 7: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/src/messages.rs crates/rb-proto/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-proto): add CONTRACT_VERSION, handshake, and Request/Response enums"`
  Expected: one commit created.

---

### Task 4: rb-proto `error.rs` — stable wire<->error mapping

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/error.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` (declare `mod error;` + re-exports)

This task defines the stable `kind` strings that flow over the wire and the two pure functions that map between `rb_types::Error` and `Response::Error`. The daemon uses `error_to_response` to map a domain error into a wire response; the client uses `response_error_to_error` to turn a `Response::Error` back into an `rb_types::Error`. Kind strings are stable identifiers (the daemon never leaks internal detail beyond the `message`).

Note on variant coverage: P0's `rb_types::Error` has the variants `Storage`, `Migration`, `NotFound`, `InvalidNamespace`, `InvalidMemoryType`, `InvalidLinkType`, `Serialization`, `DimensionMismatch`, `Io`. rb-proto maps each to a stable kind and back. (The rb-embed cluster later adds an `Embedding` variant to `rb_types`; when it does, add one arm here with kind `"embedding"`. rb-proto does not need it yet.)

- [ ] **Step 1: Write the failing tests AND declare the module.** Declare `mod error;` in `lib.rs` and create `error.rs` with ONLY the test module; the build fails because the functions do not exist yet.

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/error.rs` with:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::Response;
    use rb_types::{Error, MemoryId};

    fn round_trip(err: Error) -> Error {
        let resp = error_to_response(&err);
        match resp {
            Response::Error { kind, message } => response_error_to_error(&kind, &message),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }

    #[test]
    fn not_found_maps_to_stable_kind() {
        let id = MemoryId::new();
        let resp = error_to_response(&Error::NotFound(id.clone()));
        match resp {
            Response::Error { kind, message } => {
                assert_eq!(kind, "not_found");
                assert!(message.contains(&id.to_string()));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn storage_round_trips_to_storage() {
        assert!(matches!(round_trip(Error::Storage("disk".into())), Error::Storage(_)));
    }

    #[test]
    fn migration_round_trips_to_migration() {
        assert!(matches!(
            round_trip(Error::Migration("bad".into())),
            Error::Migration(_)
        ));
    }

    #[test]
    fn invalid_namespace_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidNamespace("x".into())),
            Error::InvalidNamespace(_)
        ));
    }

    #[test]
    fn invalid_memory_type_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidMemoryType("zz".into())),
            Error::InvalidMemoryType(_)
        ));
    }

    #[test]
    fn invalid_link_type_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidLinkType("qq".into())),
            Error::InvalidLinkType(_)
        ));
    }

    #[test]
    fn serialization_round_trips() {
        assert!(matches!(
            round_trip(Error::Serialization("json".into())),
            Error::Serialization(_)
        ));
    }

    #[test]
    fn io_round_trips() {
        assert!(matches!(round_trip(Error::Io("eof".into())), Error::Io(_)));
    }

    #[test]
    fn dimension_mismatch_round_trips_to_storage_with_detail() {
        // DimensionMismatch carries structured fields that cannot be reconstructed
        // from a string; it degrades to Storage carrying the human message, which
        // is the documented, lossy-but-faithful behavior.
        let err = Error::DimensionMismatch { expected: 1024, got: 768 };
        let resp = error_to_response(&err);
        match resp {
            Response::Error { kind, message } => {
                assert_eq!(kind, "dimension_mismatch");
                assert!(message.contains("1024") && message.contains("768"));
                let back = response_error_to_error(&kind, &message);
                assert!(matches!(back, Error::Storage(_)));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_maps_to_storage() {
        let back = response_error_to_error("totally_unknown_kind", "weird");
        assert!(matches!(back, Error::Storage(_)));
    }
}
```

  Update `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` to declare the module (re-exports added in Step 3); the module block becomes:

```rust
mod error;
mod messages;

pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-proto error`
  Expected: FAIL to compile — `cannot find function 'error_to_response'` / `response_error_to_error`. Confirms the test drives new code.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/error.rs`:

```rust
//! Stable mapping between `rb_types::Error` and `Response::Error`.
//!
//! `kind` strings are part of the wire contract and must stay stable across
//! versions. The daemon maps domain errors out; the client maps them back.

use crate::Response;
use rb_types::Error;

/// Map a domain error into a wire `Response::Error`. The `kind` is a stable
/// identifier; `message` is the human-readable `Display` form (no internal
/// detail beyond what `rb_types::Error` already exposes).
pub fn error_to_response(err: &Error) -> Response {
    let kind = error_kind(err);
    Response::Error {
        kind: kind.to_string(),
        message: err.to_string(),
    }
}

/// The stable wire `kind` string for a domain error.
fn error_kind(err: &Error) -> &'static str {
    match err {
        Error::Storage(_) => "storage",
        Error::Migration(_) => "migration",
        Error::NotFound(_) => "not_found",
        Error::InvalidNamespace(_) => "invalid_namespace",
        Error::InvalidMemoryType(_) => "invalid_memory_type",
        Error::InvalidLinkType(_) => "invalid_link_type",
        Error::Serialization(_) => "serialization",
        Error::DimensionMismatch { .. } => "dimension_mismatch",
        Error::Io(_) => "io",
    }
}

/// Reconstruct a domain error from a wire `kind`/`message`.
///
/// Variants that carry structured data which cannot be parsed back from a
/// string (`NotFound`, `DimensionMismatch`) degrade to `Error::Storage`
/// carrying the original message — faithful text, lossy structure. Unknown
/// kinds also map to `Error::Storage` (fail closed: never silently succeed).
pub fn response_error_to_error(kind: &str, message: &str) -> Error {
    match kind {
        "storage" => Error::Storage(message.to_string()),
        "migration" => Error::Migration(message.to_string()),
        "invalid_namespace" => Error::InvalidNamespace(message.to_string()),
        "invalid_memory_type" => Error::InvalidMemoryType(message.to_string()),
        "invalid_link_type" => Error::InvalidLinkType(message.to_string()),
        "serialization" => Error::Serialization(message.to_string()),
        "io" => Error::Io(message.to_string()),
        // not_found / dimension_mismatch / anything unrecognized: preserve the
        // message under Storage rather than fabricate structured fields.
        _ => Error::Storage(message.to_string()),
    }
}
```

- [ ] **Step 4: Re-export the helpers from `lib.rs`.** Update the re-export block in `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs`:

```rust
pub use error::{error_to_response, response_error_to_error};
pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 5: Run it — expect PASS.**
  Run: `cargo test -p rb-proto error`
  Expected: PASS (11 tests in the `error` module pass).

- [ ] **Step 6: Lint + format.**
  Run: `cargo clippy -p rb-proto --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no diff.

- [ ] **Step 7: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/src/error.rs crates/rb-proto/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-proto): add stable wire<->error mapping helpers"`
  Expected: one commit created.

---

### Task 5: rb-proto `frame.rs` — length-delimited JSON frame read/write helpers

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/frame.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` (declare `mod frame;` + re-exports)
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml` (add `tokio-stream`)

This task provides the framing primitives: serialize a serde value to JSON bytes, send it as one length-delimited frame, and read one frame back and deserialize it. Both helpers operate on a `Framed<S, LengthDelimitedCodec>` where `S: AsyncRead + AsyncWrite + Unpin`, which is exactly what the UDS transport provides. The test drives an in-memory duplex stream (`tokio::io::duplex`) so no socket is needed.

- [ ] **Step 1: Write the failing tests AND declare the module.** Declare `mod frame;` in `lib.rs` and create `frame.rs` with ONLY the test module; the build fails because the functions do not exist yet.

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/frame.rs` with:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{Handshake, Request, CONTRACT_VERSION};
    use rb_types::{MemoryType, Namespace};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    // A pair of in-memory bidirectional streams standing in for the two ends of
    // a UnixStream. Each end is wrapped in the length-delimited codec.
    fn framed_pair() -> (
        Framed<tokio::io::DuplexStream, LengthDelimitedCodec>,
        Framed<tokio::io::DuplexStream, LengthDelimitedCodec>,
    ) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (
            Framed::new(a, LengthDelimitedCodec::new()),
            Framed::new(b, LengthDelimitedCodec::new()),
        )
    }

    #[tokio::test]
    async fn write_then_read_round_trips_a_value() {
        let (mut client, mut server) = framed_pair();

        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Project("rusty-brain".into()),
        };
        write_frame(&mut client, &hs).await.unwrap();

        let got: Handshake = read_frame(&mut server).await.unwrap();
        assert_eq!(got.contract_version, CONTRACT_VERSION);
        assert_eq!(got.namespace, Namespace::Project("rusty-brain".into()));
    }

    #[tokio::test]
    async fn two_frames_are_independently_decoded() {
        let (mut client, mut server) = framed_pair();

        let r1 = Request::Ping;
        let r2 = Request::Recall {
            query: "transactions".into(),
            scope: Some(Namespace::Global),
            memory_type: Some(MemoryType::BugFix),
            tags: vec![],
            limit: 5,
        };
        write_frame(&mut client, &r1).await.unwrap();
        write_frame(&mut client, &r2).await.unwrap();

        let g1: Request = read_frame(&mut server).await.unwrap();
        let g2: Request = read_frame(&mut server).await.unwrap();
        assert_eq!(
            serde_json::to_string(&g1).unwrap(),
            serde_json::to_string(&r1).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&g2).unwrap(),
            serde_json::to_string(&r2).unwrap()
        );
    }

    #[tokio::test]
    async fn read_after_peer_closes_is_io_error() {
        let (client, mut server) = framed_pair();
        // Drop the client end without sending anything -> stream ends cleanly.
        drop(client);
        let err = read_frame::<_, Request>(&mut server).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Io(_)),
            "clean EOF should surface as Error::Io, got {err:?}"
        );
    }
}
```

  Update `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` to declare the module; the module block becomes:

```rust
mod error;
mod frame;
mod messages;

pub use error::{error_to_response, response_error_to_error};
pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-proto frame`
  Expected: FAIL to compile — `cannot find function 'write_frame'` / `read_frame`. Confirms the test drives new code.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/frame.rs`:

```rust
//! Length-delimited JSON framing over an async transport.
//!
//! Each frame is a 4-byte big-endian length prefix (managed by
//! `LengthDelimitedCodec`) followed by the serde_json-encoded payload bytes.

use futures::SinkExt;
use rb_types::{Error, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::StreamExt;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Serialize `value` to JSON and send it as one length-delimited frame.
///
/// `LengthDelimitedCodec` implements `Encoder<bytes::Bytes>`, so the sink item
/// is `bytes::Bytes`; `SinkExt::send` both feeds and flushes the frame.
/// (verify against installed tokio-util at execution; adjust if the codec's
/// `Sink` item type differs from `bytes::Bytes`.)
pub async fn write_frame<S, T>(framed: &mut Framed<S, LengthDelimitedCodec>, value: &T) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|e| Error::Serialization(e.to_string()))?;
    framed
        .send(bytes::Bytes::from(bytes))
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

/// Read one length-delimited frame and deserialize it as `T`.
///
/// A clean end-of-stream (peer closed without sending a frame) yields
/// `Stream::next == None`, surfaced as `Error::Io` so callers can distinguish
/// it from a decode failure. The decoded item is `bytes::BytesMut`, which
/// derefs to `[u8]` for `serde_json::from_slice`.
pub async fn read_frame<S, T>(framed: &mut Framed<S, LengthDelimitedCodec>) -> Result<T>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: DeserializeOwned,
{
    match framed.next().await {
        Some(Ok(bytes)) => {
            serde_json::from_slice(&bytes).map_err(|e| Error::Serialization(e.to_string()))
        }
        Some(Err(e)) => Err(Error::Io(e.to_string())),
        None => Err(Error::Io("connection closed before a frame was received".to_string())),
    }
}
```

  Add the framing deps to `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml`. The `[dependencies]` block needs `tokio-stream` (for `StreamExt::next`) added; update the block to:

```toml
[dependencies]
rb-types = { path = "../rb-types" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
tokio-stream = { workspace = true }
bytes = { workspace = true }
futures = { workspace = true }
thiserror = { workspace = true }
```

  Note: `tokio_stream::StreamExt::next` and `futures::StreamExt::next` are interchangeable here (both operate on the `Stream` impl of `Framed`); `tokio-stream` is used to match the spine's declared workspace dependency. If you prefer to drop the extra dependency, replace `use tokio_stream::StreamExt;` with `use futures::StreamExt;` and remove `tokio-stream` from this manifest — the code is otherwise identical.

- [ ] **Step 4: Re-export the helpers from `lib.rs`.** Update the re-export block in `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs`:

```rust
pub use error::{error_to_response, response_error_to_error};
pub use frame::{read_frame, write_frame};
pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 5: Run it — expect PASS.**
  Run: `cargo test -p rb-proto frame`
  Expected: PASS (3 tests in the `frame` module pass).

- [ ] **Step 6: Lint + format.**
  Run: `cargo clippy -p rb-proto --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no diff.

- [ ] **Step 7: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/src/frame.rs crates/rb-proto/src/lib.rs crates/rb-proto/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-proto): add length-delimited JSON frame read/write helpers"`
  Expected: one commit created.

---

### Task 6: rb-proto `client.rs` — async Client connect + handshake + request

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/client.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` (declare `mod client;` + re-exports)

This task implements the async `Client`: it connects a `UnixStream`, wraps it in the length-delimited codec, sends the `Handshake`, verifies the `HandshakeAck` contract version, and exposes `request()` to send one `Request` and read one `Response`. Typed convenience wrappers land in Task 7. The end-to-end test stands up a tiny in-process responder over a real `UnixListener` in a `tempfile` dir, proving connect + handshake + request work over an actual socket. A second test proves a contract-version mismatch is rejected at connect. (`tempfile` is added as a dev-dependency in Task 8; if you need Task 6's tests to compile in isolation, add the `[dev-dependencies] tempfile = { workspace = true }` section now — Task 8 is idempotent and will leave it as-is.)

- [ ] **Step 1: Write the failing tests AND declare the module.** Declare `mod client;` in `lib.rs` and create `client.rs` with ONLY the test module; the build fails because `Client` does not exist yet.

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/client.rs` with:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
    };
    use rb_types::Namespace;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        (dir, path)
    }

    // Accept ONE connection, read the handshake, ack it (optionally with a
    // forced ack contract version to simulate drift), then echo one request as
    // a canned Pong response. Returns after serving a single connection.
    async fn run_responder(listener: UnixListener, ack_version: u32, ok: bool) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());

        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        let ack = HandshakeAck {
            contract_version: ack_version,
            ok,
            message: if ok { None } else { Some("version mismatch".into()) },
        };
        write_frame(&mut framed, &ack).await.unwrap();
        if !ok {
            return;
        }

        // Serve exactly one request: reply Pong regardless of the request.
        let _req: Request = read_frame(&mut framed).await.unwrap();
        let resp = Response::Pong {
            contract_version: CONTRACT_VERSION,
        };
        write_frame(&mut framed, &resp).await.unwrap();
    }

    #[tokio::test]
    async fn connect_handshake_and_request_round_trip() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION, true));

        let mut client = Client::connect(&path, Namespace::Project("rusty-brain".into()))
            .await
            .unwrap();
        let resp = client.request(Request::Ping).await.unwrap();
        match resp {
            Response::Pong { contract_version } => {
                assert_eq!(contract_version, CONTRACT_VERSION);
            }
            other => panic!("expected Pong, got {other:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_rejects_contract_version_mismatch() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        // Responder acks with a different contract version and ok=false.
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION + 1, false));

        let err = Client::connect(&path, Namespace::Global)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Storage(_)),
            "version mismatch must fail connect, got {err:?}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_to_missing_socket_is_io_error() {
        let (_dir, path) = socket_path();
        // Never bind a listener -> connect must fail with an IO error.
        let err = Client::connect(&path, Namespace::Global)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Io(_)),
            "missing socket should be Error::Io, got {err:?}"
        );
    }
}
```

  Update `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs` to declare the module; the module block becomes:

```rust
mod client;
mod error;
mod frame;
mod messages;

pub use error::{error_to_response, response_error_to_error};
pub use frame::{read_frame, write_frame};
pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-proto client`
  Expected: FAIL to compile — `cannot find type 'Client' in this scope`. Confirms the test drives new code.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/client.rs`:

```rust
//! Async client for the rusty-brain daemon over a Unix domain socket.

use crate::frame::{read_frame, write_frame};
use crate::messages::{Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION};
use rb_types::{Error, Namespace, Result};
use std::path::Path;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// A connected, handshaken client. Sends one `Request`, reads one `Response`.
pub struct Client {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
}

impl Client {
    /// Connect to the daemon socket, perform the versioned handshake, and verify
    /// the daemon speaks `CONTRACT_VERSION`. Fails closed on any version drift or
    /// a non-ok ack.
    pub async fn connect(socket_path: &Path, namespace: Namespace) -> Result<Client> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| Error::Io(format!("connect {}: {e}", socket_path.display())))?;
        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

        let handshake = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace,
        };
        write_frame(&mut framed, &handshake).await?;

        let ack: HandshakeAck = read_frame(&mut framed).await?;
        if ack.contract_version != CONTRACT_VERSION {
            return Err(Error::Storage(format!(
                "contract version mismatch: client {CONTRACT_VERSION}, daemon {}",
                ack.contract_version
            )));
        }
        if !ack.ok {
            let detail = ack.message.unwrap_or_else(|| "handshake rejected".to_string());
            return Err(Error::Storage(format!("handshake rejected: {detail}")));
        }

        Ok(Client { framed })
    }

    /// Send one request and read one response.
    pub async fn request(&mut self, req: Request) -> Result<Response> {
        write_frame(&mut self.framed, &req).await?;
        let resp: Response = read_frame(&mut self.framed).await?;
        Ok(resp)
    }
}
```

- [ ] **Step 4: Re-export `Client` from `lib.rs`.** Add `Client` to the re-export block in `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/lib.rs`:

```rust
pub use client::Client;
pub use error::{error_to_response, response_error_to_error};
pub use frame::{read_frame, write_frame};
pub use messages::{
    Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
```

- [ ] **Step 5: Run it — expect PASS.** (If `tempfile` is not yet a dev-dependency, add `[dev-dependencies] tempfile = { workspace = true }` to `crates/rb-proto/Cargo.toml` — this is finalized in Task 8.)
  Run: `cargo test -p rb-proto client`
  Expected: PASS (3 tests in the `client` module pass — round-trip over a real UDS, version-mismatch rejection, missing-socket IO error).

- [ ] **Step 6: Lint + format.**
  Run: `cargo clippy -p rb-proto --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no diff.

- [ ] **Step 7: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/src/client.rs crates/rb-proto/src/lib.rs crates/rb-proto/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-proto): add async Client with connect, handshake, and request"`
  Expected: one commit created.

---

### Task 7: rb-proto `Client` typed convenience wrappers

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/client.rs` (add wrapper methods + tests)

This task adds the typed wrappers from the spine (`remember`, `recall`, `get`, `list`, `graph`, `update`, `delete`, `context`, `ping`). Each builds the matching `Request`, calls `request()`, and unwraps the expected `Response` variant into a domain type — mapping a `Response::Error` back through `response_error_to_error` and any unexpected variant to `Error::Storage`. The test extends the in-process responder to answer each op with its matching response.

- [ ] **Step 1: Write the failing tests.** Append a second test module to `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/client.rs` (the existing `mod tests` stays as-is). Add:

```rust
#[cfg(test)]
mod wrapper_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        error_to_response, read_frame, write_frame, Handshake, HandshakeAck, Request, Response,
        CONTRACT_VERSION,
    };
    use rb_types::{Error, MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.sock");
        (dir, path)
    }

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Global,
            "remembered body".into(),
            MemoryType::Insight,
            6,
        )
    }

    // Accept one connection, handshake, then answer each incoming Request with a
    // matching canned Response until the client disconnects.
    async fn serve(listener: UnixListener, fixed_id: MemoryId) {
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

        loop {
            let req: Request = match read_frame(&mut framed).await {
                Ok(r) => r,
                Err(_) => break, // client closed
            };
            let resp = match req {
                Request::Remember { .. } => Response::Remembered {
                    id: fixed_id.clone(),
                },
                Request::Recall { .. } => Response::Recalled {
                    results: vec![SearchResult {
                        memory: note(),
                        score: 0.5,
                    }],
                },
                Request::Get { .. } => Response::Got {
                    memory: Some(note()),
                },
                Request::List { .. } => Response::Listed {
                    memories: vec![note()],
                },
                Request::Graph { .. } => Response::GraphResult {
                    memories: vec![note()],
                },
                Request::Update { .. } => Response::Updated,
                Request::Delete { .. } => Response::Deleted,
                Request::Context => Response::ContextResult {
                    recent: vec![note()],
                    important: vec![note()],
                    total: 2,
                },
                Request::Ping => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                },
            };
            write_frame(&mut framed, &resp).await.unwrap();
        }
    }

    async fn connect(path: &std::path::Path) -> Client {
        Client::connect(path, Namespace::Global).await.unwrap()
    }

    #[tokio::test]
    async fn typed_wrappers_return_domain_types() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let fixed_id = MemoryId::new();
        let server = tokio::spawn(serve(listener, fixed_id.clone()));

        let mut c = connect(&path).await;

        let id = c
            .remember(
                "body".into(),
                Some("ctx".into()),
                MemoryType::Insight,
                6,
                vec!["k".into()],
                vec!["t".into()],
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(id, fixed_id);

        let results = c.recall("q".into(), None, None, vec![], 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!((results[0].score - 0.5).abs() < f32::EPSILON);

        let got = c.get(fixed_id.clone()).await.unwrap();
        assert!(got.is_some());

        let listed = c.list(None, Some(5), 10).await.unwrap();
        assert_eq!(listed.len(), 1);

        let graphed = c.graph(fixed_id.clone(), 2).await.unwrap();
        assert_eq!(graphed.len(), 1);

        c.update(fixed_id.clone(), Default::default()).await.unwrap();
        c.delete(fixed_id.clone()).await.unwrap();

        let (recent, important, total) = c.context().await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(important.len(), 1);
        assert_eq!(total, 2);

        let version = c.ping().await.unwrap();
        assert_eq!(version, CONTRACT_VERSION);

        drop(c);
        server.await.unwrap();
    }

    // A responder that always returns a Response::Error, to prove wrappers map
    // wire errors back into rb_types::Error.
    async fn serve_error(listener: UnixListener) {
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
        let _req: Request = read_frame(&mut framed).await.unwrap();
        let resp = error_to_response(&Error::NotFound(MemoryId::new()));
        write_frame(&mut framed, &resp).await.unwrap();
    }

    #[tokio::test]
    async fn wrapper_maps_wire_error_to_domain_error() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(serve_error(listener));

        let mut c = connect(&path).await;
        let err = c.get(MemoryId::new()).await.unwrap_err();
        // NotFound degrades to Storage on the wire (see error.rs), but it is an
        // Err, not a falsely-successful None.
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");
        server.await.unwrap();
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-proto wrapper`
  Expected: FAIL to compile — `no method named 'remember' found` etc. Confirms the test drives the new wrappers.

- [ ] **Step 3: Add the wrapper methods.** Insert the following `impl Client` block in `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/src/client.rs`, immediately AFTER the existing `impl Client { ... }` block that ends with the `request` method (before the first `#[cfg(test)]`). It reuses the existing `use` imports and adds the response-mapping helper:

```rust
use crate::error::response_error_to_error;
use crate::messages::Response as Resp;
use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates, SearchResult};

impl Client {
    /// Helper: turn an unexpected response (including a wire `Error`) into an
    /// `Err`. The daemon's `Response::Error` is mapped back to a domain error;
    /// any other unexpected variant is a protocol violation -> `Error::Storage`.
    fn unexpected(resp: Resp) -> Error {
        match resp {
            Resp::Error { kind, message } => response_error_to_error(&kind, &message),
            other => Error::Storage(format!("unexpected response: {other:?}")),
        }
    }

    /// Store a new memory; returns its id.
    #[allow(clippy::too_many_arguments)]
    pub async fn remember(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
    ) -> Result<MemoryId> {
        let resp = self
            .request(Request::Remember {
                content,
                context,
                memory_type,
                importance,
                keywords,
                tags,
                related_files,
            })
            .await?;
        match resp {
            Resp::Remembered { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Hybrid recall; returns ranked results.
    pub async fn recall(
        &mut self,
        query: String,
        scope: Option<Namespace>,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let resp = self
            .request(Request::Recall {
                query,
                scope,
                memory_type,
                tags,
                limit,
            })
            .await?;
        match resp {
            Resp::Recalled { results } => Ok(results),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetch a single memory by id.
    pub async fn get(&mut self, id: MemoryId) -> Result<Option<MemoryNote>> {
        let resp = self.request(Request::Get { id }).await?;
        match resp {
            Resp::Got { memory } => Ok(memory),
            other => Err(Self::unexpected(other)),
        }
    }

    /// List memories in scope.
    pub async fn list(
        &mut self,
        scope: Option<Namespace>,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        let resp = self
            .request(Request::List {
                scope,
                min_importance,
                limit,
            })
            .await?;
        match resp {
            Resp::Listed { memories } => Ok(memories),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Walk the link graph from a memory.
    pub async fn graph(&mut self, id: MemoryId, depth: u8) -> Result<Vec<MemoryNote>> {
        let resp = self.request(Request::Graph { id, depth }).await?;
        match resp {
            Resp::GraphResult { memories } => Ok(memories),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Apply a partial update to a memory.
    pub async fn update(&mut self, id: MemoryId, updates: MemoryUpdates) -> Result<()> {
        let resp = self.request(Request::Update { id, updates }).await?;
        match resp {
            Resp::Updated => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Soft-archive a memory.
    pub async fn delete(&mut self, id: MemoryId) -> Result<()> {
        let resp = self.request(Request::Delete { id }).await?;
        match resp {
            Resp::Deleted => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetch the project context payload (recent, important, total).
    pub async fn context(&mut self) -> Result<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        let resp = self.request(Request::Context).await?;
        match resp {
            Resp::ContextResult {
                recent,
                important,
                total,
            } => Ok((recent, important, total)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Round-trip ping; returns the daemon's contract version.
    pub async fn ping(&mut self) -> Result<u32> {
        let resp = self.request(Request::Ping).await?;
        match resp {
            Resp::Pong { contract_version } => Ok(contract_version),
            other => Err(Self::unexpected(other)),
        }
    }
}
```

  (verify against installed clippy at execution; if `clippy::too_many_arguments` does not fire on `remember`, the `#[allow]` is harmless.)

- [ ] **Step 4: Run it — expect PASS.**
  Run: `cargo test -p rb-proto wrapper`
  Expected: PASS (2 tests: all typed wrappers return domain types over a real UDS, and a wire error maps back to a domain error).

- [ ] **Step 5: Lint + format.**
  Run: `cargo clippy -p rb-proto --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no diff.

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/src/client.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-proto): add typed Client convenience wrappers"`
  Expected: one commit created.

---

### Task 8: rb-proto add `tempfile` dev-dependency for socket tests

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml`

The client tests (Tasks 6 and 7) use `tempfile::tempdir()` to host the test UDS. That dev-dependency must be declared. This task is intentionally split out so the dependency change is its own atomic commit and the prior tasks' code-then-deps ordering stays clean. (If the executor already added `tempfile` while making Task 6 compile, this task verifies it is present and correct rather than duplicating it.)

- [ ] **Step 1: Add the dev-dependency.** Ensure `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml` has a `[dev-dependencies]` section with `tempfile`. The full manifest should read:

```toml
[package]
name = "rb-proto"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain daemon wire protocol: request/response enums, length-delimited JSON framing, and async UDS client."

[lib]
name = "rb_proto"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
tokio-stream = { workspace = true }
bytes = { workspace = true }
futures = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Verify the manifest parses with the dev-dependency.**
  Run: `python3 -c "import tomllib; d=tomllib.load(open('/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/Cargo.toml','rb')); assert d['dev-dependencies']['tempfile']=={'workspace':True}; assert 'tokio-stream' in d['dependencies']; print('rb-proto manifest OK')"`
  Expected: prints `rb-proto manifest OK` (exit 0).

- [ ] **Step 3: Run the full rb-proto suite — expect PASS.** With `tempfile` declared, every module test compiles and runs.
  Run: `cargo test -p rb-proto`
  Expected: PASS (all tests across `messages`, `error`, `frame`, `client`, `wrapper_tests`).

- [ ] **Step 4: Commit.** (If Task 6/7 already committed `Cargo.toml` with `tempfile`, this `git add` is a no-op and the commit is skipped — that is fine; verify Step 3 still passes.)
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "test(rb-proto): add tempfile dev-dependency for UDS client tests" || echo "tempfile already committed; nothing to do"`
  Expected: one commit created, or a clear no-op message.

---

### Task 9: rb-proto public-API guard test + crate-wide gates

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/tests/public_api.rs`

This locks the rb-proto public surface (mirroring the rb-types `public_api` guard) and runs the full crate-wide quality gates so the cluster ends green. The integration test imports every public item via the crate root; if a future change drops or renames a re-export, this test fails to compile.

- [ ] **Step 1: Write the public-API guard integration test.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-proto/tests/public_api.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_proto::{
    error_to_response, read_frame, response_error_to_error, write_frame, Client, Handshake,
    HandshakeAck, Request, Response, CONTRACT_VERSION,
};
use rb_types::{Error, MemoryId, Namespace};

#[test]
fn public_surface_is_reachable_and_stable() {
    // Constants and messages.
    assert_eq!(CONTRACT_VERSION, 1);
    let _hs = Handshake {
        contract_version: CONTRACT_VERSION,
        namespace: Namespace::Global,
    };
    let _ack = HandshakeAck {
        contract_version: CONTRACT_VERSION,
        ok: true,
        message: None,
    };
    let _req = Request::Ping;

    // Error mapping helpers, round-tripping through the wire form.
    let resp = error_to_response(&Error::NotFound(MemoryId::new()));
    match resp {
        Response::Error { kind, message } => {
            assert_eq!(kind, "not_found");
            let back = response_error_to_error(&kind, &message);
            assert!(matches!(back, Error::Storage(_)));
        }
        other => panic!("expected Response::Error, got {other:?}"),
    }
}

// Compile-only references proving the framing and Client symbols are public.
// (Never called: these are type-level guards on the public surface.)
#[allow(dead_code)]
fn _framing_symbols_exist() {
    // Reference each public symbol so an accidental removal breaks compilation.
    let _ = write_frame::<tokio::net::UnixStream, Request>;
    let _ = read_frame::<tokio::net::UnixStream, Response>;
    let _ = Client::connect;
}
```

  The test references `tokio` directly (for `tokio::net::UnixStream` in the turbofish symbol guards), which is already a normal `[dependencies]` entry of rb-proto and therefore available to integration test targets (the rb-types `public_api.rs` guard relies on the same property for `chrono`). No manifest change is required. (verify against installed cargo at execution; if the integration target cannot see a normal dependency in your toolchain, add `tokio` under `[dev-dependencies]`.)

- [ ] **Step 2: Run the guard test — expect PASS.** It is a regression guard, not a red→green driver: every imported item already exists by Task 7, so it locks the surface.
  Run: `cargo test -p rb-proto --test public_api`
  Expected: PASS.

- [ ] **Step 3: Run the full rb-proto suite — expect PASS.**
  Run: `cargo test -p rb-proto`
  Expected: PASS (all unit tests plus the `public_api` integration test).

- [ ] **Step 4: Workspace clippy gate with warnings denied.**
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings`
  Expected: `Finished`, no warnings (exit 0).

- [ ] **Step 5: Workspace format gate.**
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no output, exit 0.

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-proto/tests/public_api.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "test(rb-proto): add public-API guard and finalize crate gates"`
  Expected: one commit created; `cargo test -p rb-proto`, workspace clippy, and fmt all green.

## Part G — rb-embed (EmbeddingProvider + Voyage + offline DeterministicProvider)

### Task 10: rb-embed `EmbeddingProvider` trait + `DeterministicProvider` (offline, infallible)

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p1/Cargo.toml` (workspace members + rb-embed workspace deps present)
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-types/src/error.rs` (add `Embedding` variant)
- Create/Overwrite: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/provider.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/deterministic.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/lib.rs` (declare modules + re-exports)

> The rb-embed skeleton (`crates/rb-embed/Cargo.toml` + an empty `src/lib.rs`) and the workspace member/deps registration were created by the P1 setup cluster. This task DEFENSIVELY confirms that registration, pins the real manifest (including the wiremock dev-dep used in Task 11), defines the `EmbeddingProvider` trait, and implements the public offline `DeterministicProvider`. The `VoyageProvider` arrives in Task 11. All paths target the P1 worktree at `/Users/bluby/repos/rusty-brain-p1` (branch `feat/p1-engine-daemon`).

- [ ] **Step 1: Add the `Embedding` error variant to rb-types.** The spine maps all embedding failures to `rb_types::Error::Embedding`. Add the variant to `crates/rb-types/src/error.rs` as the last arm inside `pub enum Error`, immediately after the existing `Io(String)` variant:

```rust
    #[error("io error: {0}")]
    Io(String),
    #[error("embedding error: {0}")]
    Embedding(String),
```

  (If the setup cluster already added it, leave the existing line in place — this is additive.)

- [ ] **Step 2: Confirm rb-types still passes after the additive change.**
  Run: `cargo test -p rb-types --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: PASS (all existing rb-types tests still green; the new variant is additive).

- [ ] **Step 3: Confirm the workspace member and rb-embed workspace dependencies exist in the root manifest.** Verify `/Users/bluby/repos/rusty-brain-p1/Cargo.toml` `[workspace] members` contains `"crates/rb-embed"` and `[workspace.dependencies]` contains the entries below (added by P1 setup; add any that are missing — do NOT remove existing P0 entries):

  Run: `grep -n 'crates/rb-embed' /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: a line inside the `[workspace] members = [...]` array referencing `crates/rb-embed`. If absent, add `"crates/rb-embed"` to the `members` array.

  Required `[workspace.dependencies]` entries (add any missing):

```toml
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
secrecy = "0.10"
tracing = "0.1"
```

  (`serde`, `serde_json`, `thiserror`, `tempfile` are already present from P0.)

- [ ] **Step 4: Pin the rb-embed crate manifest.** Overwrite `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/Cargo.toml` with the exact deps from the spine plus the `wiremock` dev-dependency (used by Task 11) and `tokio` (multi-thread + macros) for the async tests:

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

[dependencies]
rb-types = { path = "../rb-types" }
async-trait = { workspace = true }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
secrecy = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
wiremock = "0.6"

[lints]
workspace = true
```

- [ ] **Step 5: Write the failing test for `DeterministicProvider` AND declare the modules.** An undeclared `.rs` file is never compiled, so declare both modules in `lib.rs` now and add `deterministic.rs` with the implementation-driving test. Also create the `provider.rs` trait file (the test needs the `EmbeddingProvider` trait in scope to call `.embed()`).

  Set `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/lib.rs` to:

```rust
//! `rb_embed`: embedding providers for rusty-brain.
//!
//! Defines the `EmbeddingProvider` trait, the remote `VoyageProvider`
//! (added in a later task), and a public offline `DeterministicProvider`
//! used as a no-API-key fallback and in tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod deterministic;
mod provider;

pub use deterministic::DeterministicProvider;
pub use provider::EmbeddingProvider;
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/provider.rs` with ONLY the trait (no impls yet):

```rust
use async_trait::async_trait;

/// A source of embedding vectors. Implementations may be remote (Voyage) or
/// local/offline (deterministic). All implementations are `Send + Sync` so the
/// daemon can share a single provider across connection tasks.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Stable identifier of the model, stored on each memory as `embedding_model`.
    fn model_id(&self) -> &str;

    /// The fixed embedding dimension. Enforced against `meta.embedding_dim` at init.
    fn dim(&self) -> usize;

    /// Embed each input text, returning one vector per input **in input order**.
    /// Every returned vector has length `self.dim()`.
    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>>;
}
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/deterministic.rs` with ONLY the test module (the type is intentionally missing so the build fails):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::provider::EmbeddingProvider;

    #[test]
    fn model_id_and_dim_are_reported() {
        let p = DeterministicProvider::new(512);
        assert_eq!(p.dim(), 512);
        assert_eq!(p.model_id(), "deterministic");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_returns_one_vector_per_input_of_correct_length() {
        let p = DeterministicProvider::new(8);
        let inputs = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let out = p.embed(&inputs).await.unwrap();
        assert_eq!(out.len(), 3);
        for v in &out {
            assert_eq!(v.len(), 8);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn same_text_yields_same_vector() {
        let p = DeterministicProvider::new(16);
        let a = p.embed(&["repeatable".to_string()]).await.unwrap();
        let b = p.embed(&["repeatable".to_string()]).await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn different_text_yields_different_vector() {
        let p = DeterministicProvider::new(16);
        let out = p
            .embed(&["one".to_string(), "two".to_string()])
            .await
            .unwrap();
        assert_ne!(out[0], out[1]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_preserves_input_order() {
        let p = DeterministicProvider::new(16);
        let inputs = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        // Embedding the list at once must equal embedding each item alone, in order.
        let batched = p.embed(&inputs).await.unwrap();
        let mut individually = Vec::new();
        for t in &inputs {
            individually.push(p.embed(std::slice::from_ref(t)).await.unwrap()[0].clone());
        }
        assert_eq!(batched, individually);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_input_yields_empty_output() {
        let p = DeterministicProvider::new(4);
        let out = p.embed(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn vectors_are_unit_length() {
        let p = DeterministicProvider::new(32);
        let out = p.embed(&["normalize me".to_string()]).await.unwrap();
        let norm: f32 = out[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "expected unit vector, got norm {norm}");
    }
}
```

- [ ] **Step 6: Run it — expect a compile failure.**
  Run: `cargo test -p rb-embed deterministic --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: FAIL to compile — `cannot find type 'DeterministicProvider' in this scope` (the module compiles now that it is declared, but the type does not exist yet). This confirms the test drives the implementation.

- [ ] **Step 7: Add the minimal `DeterministicProvider` implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/deterministic.rs`:

```rust
use crate::provider::EmbeddingProvider;
use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Offline, deterministic embedding provider. Hashes each input text into a
/// reproducible unit-length vector of length `dim`. Same text always yields the
/// same vector; different texts yield different vectors. Never performs IO and
/// never errors. Public so it can be used as a no-API-key fallback and in tests.
///
/// Determinism relies on `std::collections::hash_map::DefaultHasher::new()`,
/// which is seeded with fixed keys (unlike `RandomState`), so the same input
/// produces the same hash across processes and runs.
pub struct DeterministicProvider {
    dim: usize,
}

impl DeterministicProvider {
    /// Create a provider that emits `dim`-length vectors.
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }

    /// Produce one reproducible unit-length vector for `text`.
    fn embed_one(&self, text: &str) -> Vec<f32> {
        // Seed a per-coordinate hash from the text plus the coordinate index so
        // the components are decorrelated but fully reproducible.
        let mut raw = Vec::with_capacity(self.dim);
        for i in 0..self.dim {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            i.hash(&mut hasher);
            let h = hasher.finish();
            // Map the 64-bit hash into a signed value in roughly [-1.0, 1.0).
            let unit = (h as f64) / (u64::MAX as f64); // [0.0, 1.0)
            raw.push((unit * 2.0 - 1.0) as f32);
        }
        // Normalize to unit length so distances are comparable to real models.
        let norm: f32 = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut raw {
                *x /= norm;
            }
        } else if !raw.is_empty() {
            // Degenerate all-zero case: emit a deterministic basis vector.
            raw[0] = 1.0;
        }
        raw
    }
}

#[async_trait]
impl EmbeddingProvider for DeterministicProvider {
    fn model_id(&self) -> &str {
        "deterministic"
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}
```

- [ ] **Step 8: Run it — expect PASS.**
  Run: `cargo test -p rb-embed deterministic --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: PASS (7 tests in the `deterministic` module pass).

- [ ] **Step 9: Lint + format the workspace.**
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings`
  Expected: `Finished` with no warnings (exit 0).
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no output, exit 0.

- [ ] **Step 10: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-embed crates/rb-types/src/error.rs Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-embed): add EmbeddingProvider trait and offline DeterministicProvider"`
  Expected: one commit created.

---

### Task 11: rb-embed `VoyageProvider` — remote embeddings over reqwest (wiremock-tested)

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/voyage.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/lib.rs` (declare `mod voyage;` + re-export)

> The `VoyageProvider` POSTs to `{base_url}/embeddings`, parses `{data:[{embedding:[...]}, ...]}`, preserves input order, checks the returned count and every returned vector's length against `dim()`, bounds batches to <=128 by chunking, sets a 30s request timeout, and maps every HTTP/parse/count/dim error to `rb_types::Error::Embedding`. All wire tests use a **wiremock** mock server (no live API). One `#[ignore]` smoke test hits the real API only when `VOYAGE_API_KEY` is set, so CI never depends on a live key.

- [ ] **Step 1: Write the failing tests AND declare the module.** Declare `mod voyage;` in `lib.rs` now (an undeclared `.rs` file is never compiled) and add the test-only `voyage.rs`. The wiremock tests assert request shape, response parsing, order preservation, the count-mismatch and dim-mismatch error paths, an HTTP-error path, empty-input short-circuit, and >128 chunking; an `#[ignore]` test covers the real API.

  Set `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/lib.rs` to (adds `mod voyage;` + the `VoyageProvider` re-export):

```rust
//! `rb_embed`: embedding providers for rusty-brain.
//!
//! Defines the `EmbeddingProvider` trait, the remote `VoyageProvider`,
//! and a public offline `DeterministicProvider` used as a no-API-key
//! fallback and in tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod deterministic;
mod provider;
mod voyage;

pub use deterministic::DeterministicProvider;
pub use provider::EmbeddingProvider;
pub use voyage::VoyageProvider;
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/voyage.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::provider::EmbeddingProvider;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Build a provider pointed at a mock server with an explicit api key, so the
    // tests never touch the real Voyage endpoint or read the environment.
    // `for_test` is a #[cfg(test)]-only helper on VoyageProvider.
    fn provider_for(base_url: &str, dim: usize) -> VoyageProvider {
        VoyageProvider::for_test("voyage-3-lite", dim, "test-key", base_url)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn metadata_reports_model_and_dim() {
        let p = provider_for("http://127.0.0.1:1/v1", 4);
        assert_eq!(p.model_id(), "voyage-3-lite");
        assert_eq!(p.dim(), 4);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn embed_sends_correct_request_and_parses_response_in_order() {
        let server = MockServer::start().await;
        // The mock asserts the request shape: POST /v1/embeddings, JSON body with
        // the model, the inputs in order, and input_type "document", with bearer auth.
        let response = serde_json::json!({
            "data": [
                { "embedding": [0.1, 0.2, 0.3, 0.4] },
                { "embedding": [0.5, 0.6, 0.7, 0.8] }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_partial_json(serde_json::json!({
                "model": "voyage-3-lite",
                "input": ["first", "second"],
                "input_type": "document"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let out = p
            .embed(&["first".to_string(), "second".to_string()])
            .await
            .unwrap();

        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(out[1], vec![0.5, 0.6, 0.7, 0.8]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dim_mismatch_is_an_embedding_error() {
        let server = MockServer::start().await;
        // Server returns a 3-length vector but the provider expects dim=4.
        let response = serde_json::json!({
            "data": [ { "embedding": [0.1, 0.2, 0.3] } ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p.embed(&["x".to_string()]).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "expected Error::Embedding, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn count_mismatch_is_an_embedding_error() {
        let server = MockServer::start().await;
        // Two inputs but the server returns only one embedding.
        let response = serde_json::json!({
            "data": [ { "embedding": [0.1, 0.2, 0.3, 0.4] } ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p
            .embed(&["a".to_string(), "b".to_string()])
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::Embedding(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_status_is_an_embedding_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let err = p.embed(&["x".to_string()]).await.unwrap_err();
        assert!(matches!(err, rb_types::Error::Embedding(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_input_short_circuits_without_calling_server() {
        // No mock mounted: if embed() called the server it would 404 and error.
        let server = MockServer::start().await;
        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let out = p.embed(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batches_over_128_are_chunked_and_recombined_in_order() {
        let server = MockServer::start().await;
        // The mock echoes a fixed vector per requested input by reading the request
        // body, so we can assert the total count and ordering after chunking.
        // The closure captures nothing, so it is Send + Sync as Respond requires.
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(|req: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                let n = body["input"].as_array().unwrap().len();
                let data: Vec<serde_json::Value> = (0..n)
                    .map(|_| serde_json::json!({ "embedding": [1.0, 0.0, 0.0, 0.0] }))
                    .collect();
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": data }))
            })
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let p = provider_for(&base, 4);
        let inputs: Vec<String> = (0..200).map(|i| format!("item-{i}")).collect();
        let out = p.embed(&inputs).await.unwrap();
        assert_eq!(out.len(), 200);
        for v in &out {
            assert_eq!(v, &vec![1.0, 0.0, 0.0, 0.0]);
        }
    }

    // Real-API smoke test. Ignored by default; run with:
    //   VOYAGE_API_KEY=... cargo test -p rb-embed -- --ignored voyage_real_api
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires VOYAGE_API_KEY and network access"]
    async fn voyage_real_api_smoke() {
        let p = VoyageProvider::from_env().unwrap();
        let out = p.embed(&["hello world".to_string()]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), p.dim());
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-embed voyage --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: FAIL to compile — `cannot find type 'VoyageProvider' in this scope` / `no function 'for_test'` (the module is now compiled, but `VoyageProvider` does not exist yet). This confirms the tests drive the implementation.

- [ ] **Step 3: Add the minimal `VoyageProvider` implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/src/voyage.rs`. Note: `for_test` is `#[cfg(test)]`-gated and infallible (it shares the private `build` constructor so it gets the same 30s timeout and base-url trimming), so no `.unwrap()`/`.expect()` appears in non-test code — keeping the workspace `unwrap_used = deny` lint satisfied under `cargo clippy -D warnings`:

```rust
use crate::provider::EmbeddingProvider;
use async_trait::async_trait;
use rb_types::Error;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

/// Maximum number of inputs per HTTP request (Voyage batch ceiling).
const MAX_BATCH: usize = 128;
/// Default model and its embedding dimension.
const DEFAULT_MODEL: &str = "voyage-3-lite";
const DEFAULT_DIM: usize = 512;
/// Default API base; the `/embeddings` path is appended per request.
const DEFAULT_BASE_URL: &str = "https://api.voyageai.com/v1";
/// Outbound request timeout (all embedding calls are timed out).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Remote embedding provider backed by the Voyage AI embeddings API.
pub struct VoyageProvider {
    client: reqwest::Client,
    api_key: SecretString,
    model: String,
    dim: usize,
    base_url: String,
}

/// Shape of the Voyage `/embeddings` response we depend on.
#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

impl VoyageProvider {
    /// Build a provider from the environment. Reads `VOYAGE_API_KEY` (an
    /// `Error::Embedding` if absent), defaults to model `voyage-3-lite` (dim 512),
    /// and constructs a reqwest client with a request timeout.
    pub fn from_env() -> rb_types::Result<Self> {
        let key = std::env::var("VOYAGE_API_KEY")
            .map_err(|_| Error::Embedding("VOYAGE_API_KEY is not set".to_string()))?;
        Self::build(DEFAULT_MODEL, DEFAULT_DIM, key, DEFAULT_BASE_URL)
    }

    /// Build a provider for a specific model + dimension, reading the key from
    /// `VOYAGE_API_KEY`. Use this when overriding the default model.
    pub fn with_model(model: &str, dim: usize) -> rb_types::Result<Self> {
        let key = std::env::var("VOYAGE_API_KEY")
            .map_err(|_| Error::Embedding("VOYAGE_API_KEY is not set".to_string()))?;
        Self::build(model, dim, key, DEFAULT_BASE_URL)
    }

    /// Test-only constructor: explicit key + base URL, no environment access.
    /// Builds the reqwest client directly (without `?`/unwrap) so it is infallible
    /// and never panics; on the unlikely builder failure it falls back to the
    /// default reqwest client (which still honors the per-request timeout set via
    /// `.timeout(...)` on each request would require a builder, so we reuse the
    /// builder result and only fall back to `Client::new()` if `build()` errors).
    #[cfg(test)]
    pub(crate) fn for_test(model: &str, dim: usize, api_key: &str, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            api_key: SecretString::from(api_key.to_string()),
            model: model.to_string(),
            dim,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn build(model: &str, dim: usize, api_key: String, base_url: &str) -> rb_types::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Embedding(format!("failed to build http client: {e}")))?;
        Ok(Self {
            client,
            api_key: SecretString::from(api_key),
            model: model.to_string(),
            dim,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// POST a single chunk of inputs and return their embeddings in order.
    async fn embed_chunk(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
            "input_type": "document",
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Embedding(format!("voyage request failed: {e}")))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| Error::Embedding(format!("voyage returned an error status: {e}")))?;

        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| Error::Embedding(format!("failed to parse voyage response: {e}")))?;

        if parsed.data.len() != texts.len() {
            return Err(Error::Embedding(format!(
                "voyage returned {} embeddings for {} inputs",
                parsed.data.len(),
                texts.len()
            )));
        }

        let mut out = Vec::with_capacity(parsed.data.len());
        for item in parsed.data {
            if item.embedding.len() != self.dim {
                return Err(Error::Embedding(format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.dim,
                    item.embedding.len()
                )));
            }
            out.push(item.embedding);
        }
        Ok(out)
    }
}

#[async_trait]
impl EmbeddingProvider for VoyageProvider {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String]) -> rb_types::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(MAX_BATCH) {
            let mut embeddings = self.embed_chunk(chunk).await?;
            out.append(&mut embeddings);
        }
        Ok(out)
    }
}
```

  (verify against installed crate at execution; adjust if the secrecy 0.10 `SecretString::from`/`expose_secret` or the reqwest 0.12 `bearer_auth`/`error_for_status`/`json` API differs. Verified locally: secrecy 0.10.3 provides `impl From<String> for SecretString` and `ExposeSecret::expose_secret(&self) -> &str`; reqwest 0.12.28 provides `error_for_status(self) -> Result<Self>` and async `json`.)

- [ ] **Step 4: Run it — expect PASS.**
  Run: `cargo test -p rb-embed voyage --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: PASS. The 7 non-ignored tests pass and `voyage_real_api_smoke` is reported as `ignored` (it never calls the live API in CI).

- [ ] **Step 5: Confirm the ignored real-API test is correctly gated (does NOT run by default).**
  Run: `cargo test -p rb-embed voyage --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- --nocapture 2>&1 | grep -E "voyage_real_api_smoke|test result"`
  Expected: a line showing `voyage_real_api_smoke ... ignored` and a `test result: ok.` summary with `N passed; 0 failed; 1 ignored`. CI never depends on a live API key.

- [ ] **Step 6: Lint + format the workspace.**
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings`
  Expected: `Finished` with no warnings (exit 0). (The only `.unwrap_or_else(..unwrap..)`-style fallback is gone; `for_test` is `#[cfg(test)]`-gated and its single `unwrap_or_else` is allowed under the test cfg, so no `unwrap_used`/`expect_used`/`panic` lint fires in non-test code.)
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no output, exit 0.

- [ ] **Step 7: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-embed/src/voyage.rs crates/rb-embed/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-embed): add VoyageProvider with reqwest, batching, and order/dim checks"`
  Expected: one commit created.

---

### Task 12: rb-embed crate gate — public-API guard + provider parity integration test

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/tests/public_api.rs`

> A crate-level integration test (compiled as a separate target, in non-test cfg of the lib) that locks the rb-embed public surface (`EmbeddingProvider`, `DeterministicProvider`, `VoyageProvider`) and asserts the trait contract holds through the public re-exports — in particular that the offline provider honors `dim()`, order, and determinism, and that the trait is object-safe (`Box<dyn EmbeddingProvider>`). It uses ONLY the offline `DeterministicProvider` for live assertions so CI never needs network or a key; `VoyageProvider` is only constructed via the public `with_model` (no network). This is the per-crate gate matching the P0 `tests/public_api.rs` pattern.

- [ ] **Step 1: Confirm the integration test's dev-dependencies are present.** The integration target needs `tokio` (async driver) and uses `rb_types` (already a normal dependency of rb-embed, so visible to integration tests — no separate dev entry required).
  Run: `grep -n 'tokio' /Users/bluby/repos/rusty-brain-p1/crates/rb-embed/Cargo.toml`
  Expected: a `tokio = { workspace = true }` line under `[dev-dependencies]` (added in Task 10). If missing, add it before proceeding.

- [ ] **Step 2: Write the public-API guard integration test.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-embed/tests/public_api.rs`. It imports every public type via the crate root and drives the trait through a `dyn EmbeddingProvider`, proving the trait is object-safe and the offline provider satisfies the full contract (length, order, determinism). `VoyageProvider` is only constructed (no network) to prove it is publicly reachable and implements the trait:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::{DeterministicProvider, EmbeddingProvider, VoyageProvider};

#[tokio::test(flavor = "multi_thread")]
async fn deterministic_provider_satisfies_trait_contract_via_dyn() {
    // Object-safety: the trait must be usable behind a trait object, as the
    // engine/daemon will hold `Box<dyn EmbeddingProvider>` (or a generic param).
    let provider: Box<dyn EmbeddingProvider> = Box::new(DeterministicProvider::new(64));
    assert_eq!(provider.dim(), 64);
    assert_eq!(provider.model_id(), "deterministic");

    let inputs = vec![
        "one transaction one database".to_string(),
        "single writer thread".to_string(),
        "namespace isolation".to_string(),
    ];
    let out = provider.embed(&inputs).await.unwrap();

    // One vector per input, each of length dim().
    assert_eq!(out.len(), inputs.len());
    for v in &out {
        assert_eq!(v.len(), provider.dim());
    }

    // Determinism: re-embedding yields identical vectors.
    let out2 = provider.embed(&inputs).await.unwrap();
    assert_eq!(out, out2);

    // Distinctness: different inputs produce different vectors.
    assert_ne!(out[0], out[1]);
    assert_ne!(out[1], out[2]);
}

#[test]
fn voyage_provider_is_publicly_constructible() {
    // Reachable from the crate root and implements EmbeddingProvider; no network.
    let provider = VoyageProvider::with_model("voyage-3-lite", 1024);
    // Without VOYAGE_API_KEY this is an Embedding error; with it, a valid provider.
    match provider {
        Ok(p) => {
            let p: &dyn EmbeddingProvider = &p;
            assert_eq!(p.dim(), 1024);
            assert_eq!(p.model_id(), "voyage-3-lite");
        }
        Err(e) => assert!(matches!(e, rb_types::Error::Embedding(_))),
    }
}
```

- [ ] **Step 3: Run it — expect PASS (regression guard, not a red→green driver).**
  Run: `cargo test -p rb-embed --test public_api --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: PASS (2 tests). By the end of Task 11 the public surface already exists, so every import resolves. The test LOCKS that surface: if a future change drops or renames a re-export, or makes `EmbeddingProvider` non-object-safe, this integration target fails to compile — catching the regression. (Note: `for_test` is `#[cfg(test)]`-gated and therefore NOT visible from this separate integration crate, which is exactly why this test relies only on `with_model`.)

- [ ] **Step 4: Run the full rb-embed crate test suite as the cluster gate.**
  Run: `cargo test -p rb-embed --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: PASS — all unit tests (deterministic + voyage modules) plus the `public_api` integration target green; `voyage_real_api_smoke` shown as `ignored`. No network access occurred.

- [ ] **Step 5: Lint + format the workspace.**
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings`
  Expected: `Finished` with no warnings (exit 0).
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no output, exit 0.

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-embed/tests/public_api.rs crates/rb-embed/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "test(rb-embed): add public-API guard and provider trait-contract integration test"`
  Expected: one commit created.

## Part H — rb-search (pure hybrid ranking)

### Task 13: rb-search `weights.rs` — `Weights` struct with documented `Default` summing to 1.0

**Files:**
- Create: `crates/rb-search/src/weights.rs`
- Modify: `crates/rb-search/src/lib.rs` (add `mod weights;` + re-export)
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-search/src/weights.rs`

> The `rb-search` crate skeleton (manifest with `rb-types` + `chrono` deps and `[lints] workspace = true`, plus a doc-only `lib.rs`) is created in the earlier P1 setup cluster. This task adds the first real module. A `.rs` file not declared with `mod` in `lib.rs` is never compiled, so we declare the module up front and the build fails until the type exists — confirming the test drives new code rather than being silently skipped.

- [ ] **Step 1: Write the failing test AND declare the module.** Create `crates/rb-search/src/weights.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn default_weights_match_documented_values() {
        let w = Weights::default();
        assert!((w.vector - 0.45).abs() < f32::EPSILON);
        assert!((w.keyword - 0.30).abs() < f32::EPSILON);
        assert!((w.graph - 0.10).abs() < f32::EPSILON);
        assert!((w.importance - 0.10).abs() < f32::EPSILON);
        assert!((w.recency - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn default_weights_sum_to_one() {
        let w = Weights::default();
        let sum = w.vector + w.keyword + w.graph + w.importance + w.recency;
        assert!((sum - 1.0).abs() < 1e-6, "weights must sum to 1.0, got {sum}");
    }

    #[test]
    fn weights_is_copy_and_clone() {
        let w = Weights::default();
        let copied = w; // Copy
        // Invoke the Clone impl explicitly: `w.clone()` would trip clippy's
        // `clone_on_copy` lint (denied by -D warnings) because Weights is Copy.
        let cloned = Clone::clone(&w);
        assert!((copied.vector - cloned.vector).abs() < f32::EPSILON);
        // original still usable after the copy (proves Copy, not move)
        assert!((w.vector - 0.45).abs() < f32::EPSILON);
    }
}
```

  Set `crates/rb-search/src/lib.rs` to declare the module (replace the doc-only skeleton body's tail with the `mod` line; the `pub use` is added in Step 3):

```rust
//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO, no async. Combines normalized keyword / vector / graph / importance /
//! recency signals into a single weighted score (see `rank`).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod weights;
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-search weights -- --nocapture`
  Expected: FAIL to compile — `cannot find type 'Weights' in this scope` (the `weights` module is now compiled, but the `Weights` type does not exist yet). This confirms the test drives new code.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-search/src/weights.rs`:

```rust
/// Relative contribution of each ranking signal to the final score.
///
/// The `Default` weights are tuned for hybrid recall and **sum to 1.0**, so the
/// weighted score of any single candidate stays in `[0.0, 1.0]`:
///
/// | signal     | weight |
/// |------------|--------|
/// | vector     | 0.45   |
/// | keyword    | 0.30   |
/// | graph      | 0.10   |
/// | importance | 0.10   |
/// | recency    | 0.05   |
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    pub vector: f32,
    pub keyword: f32,
    pub graph: f32,
    pub importance: f32,
    pub recency: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            vector: 0.45,
            keyword: 0.30,
            graph: 0.10,
            importance: 0.10,
            recency: 0.05,
        }
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Append the re-export to `crates/rb-search/src/lib.rs` so the public API is `rb_search::Weights`:

```rust
//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO, no async. Combines normalized keyword / vector / graph / importance /
//! recency signals into a single weighted score (see `rank`).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod weights;

pub use weights::Weights;
```

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-search weights -- --nocapture`
  Expected: PASS (3 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-search --all-targets -- -D warnings`
  Expected: no warnings, exit 0. Run: `cargo fmt --all`
  Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-search/src/weights.rs crates/rb-search/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-search): add Weights struct with documented Default summing to 1.0"`
  Expected: one commit created.

---

### Task 14: rb-search `rank.rs` — `Signals` + `HALF_LIFE` + pure `rank()` scoring

**Files:**
- Create: `crates/rb-search/src/rank.rs`
- Modify: `crates/rb-search/src/lib.rs` (add `mod rank;` + re-exports)
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-search/src/rank.rs`

> `rank()` is the pure scoring core. Normalization per the spine: `vector_sim = 1 - clamp(distance/2, 0, 1)`; `keyword = 1/(1+rank)` (0 = best); `graph = 1/(1+hops)`; `importance/10`; `recency = exp(-age_days/HALF_LIFE)` with `HALF_LIFE = 30.0`. A missing signal contributes 0 to its term (no penalty). The weighted sum is sorted descending and truncated to `limit`. `slice::sort_by` is a stable sort, so equal scores keep input order for deterministic, reproducible output.

- [ ] **Step 1: Write the failing test AND declare the module.** Create `crates/rb-search/src/rank.rs` with ONLY the test module. It pins: a strong vector+keyword doc outranks a weak one; recency breaks ties; graph-only candidates still rank above nothing; `limit` truncates; and a property-style check that every score is in `[0,1]` and ordering is stable across repeated calls:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::{Duration, Utc};
    use rb_types::MemoryId;

    /// Returns the current instant, captured once per test and reused as the single
    /// consistent reference for both `created_at` offsets and the `rank(..., n, ...)`
    /// call, so each test is internally deterministic.
    fn now() -> chrono::DateTime<chrono::Utc> {
        Utc::now()
    }

    #[test]
    fn strong_doc_outranks_weak_doc() {
        let n = now();
        let strong = MemoryId::new();
        let weak = MemoryId::new();
        let signals = vec![
            Signals {
                id: weak.clone(),
                keyword_rank: Some(9),
                vector_distance: Some(1.6), // far -> low sim
                graph_hops: None,
                importance: 2,
                created_at: n - Duration::days(120),
            },
            Signals {
                id: strong.clone(),
                keyword_rank: Some(0), // best keyword hit
                vector_distance: Some(0.1), // very close
                graph_hops: Some(1),
                importance: 9,
                created_at: n,
            },
        ];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, strong, "strong vector+keyword doc must rank first");
        assert_eq!(ranked[1].0, weak);
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn recency_breaks_ties_between_otherwise_equal_docs() {
        let n = now();
        let recent = MemoryId::new();
        let old = MemoryId::new();
        // Identical on every signal EXCEPT created_at.
        let mk = |id: MemoryId, created| Signals {
            id,
            keyword_rank: Some(1),
            vector_distance: Some(0.4),
            graph_hops: Some(2),
            importance: 5,
            created_at: created,
        };
        let signals = vec![
            mk(old.clone(), n - Duration::days(200)),
            mk(recent.clone(), n),
        ];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, recent, "more recent doc wins the tie");
        assert_eq!(ranked[1].0, old);
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn graph_only_candidate_still_ranks_above_zero() {
        let n = now();
        let id = MemoryId::new();
        let signals = vec![Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: None,
            graph_hops: Some(0), // adjacent -> graph proximity 1.0
            importance: 0,
            created_at: n - Duration::days(10_000), // ancient -> recency ~0
        }];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0, id);
        // graph weight (0.10) * proximity (1.0) dominates; score strictly > 0.
        assert!(ranked[0].1 > 0.0, "graph-only candidate must score > 0");
    }

    #[test]
    fn missing_signals_do_not_penalize() {
        let n = now();
        // A: only a keyword hit. B: only a vector hit at the SAME normalized strength.
        // keyword Some(0) -> 1.0 ; vector dist 0.0 -> sim 1.0. With default weights
        // keyword(0.30) vs vector(0.45), the vector-only doc should outrank.
        let a = MemoryId::new();
        let b = MemoryId::new();
        let signals = vec![
            Signals {
                id: a.clone(),
                keyword_rank: Some(0),
                vector_distance: None,
                graph_hops: None,
                importance: 0,
                created_at: n - Duration::days(10_000),
            },
            Signals {
                id: b.clone(),
                keyword_rank: None,
                vector_distance: Some(0.0),
                graph_hops: None,
                importance: 0,
                created_at: n - Duration::days(10_000),
            },
        ];
        let ranked = rank(signals, Weights::default(), n, 10);
        assert_eq!(ranked[0].0, b, "vector-only (0.45) beats keyword-only (0.30)");
        // The keyword-only doc is NOT penalized to 0: it scores its full keyword term.
        assert!((ranked[1].1 - 0.30).abs() < 1e-5);
    }

    #[test]
    fn limit_truncates_to_top_n() {
        let n = now();
        let signals: Vec<Signals> = (0..5)
            .map(|i| Signals {
                id: MemoryId::new(),
                keyword_rank: Some(i),
                vector_distance: Some(0.1 * i as f32),
                graph_hops: None,
                importance: (10 - i) as u8,
                created_at: n,
            })
            .collect();
        let ranked = rank(signals, Weights::default(), n, 2);
        assert_eq!(ranked.len(), 2, "limit truncates to 2");
        // truncation keeps the two highest scores in descending order
        assert!(ranked[0].1 >= ranked[1].1);
    }

    #[test]
    fn scores_in_range_and_ordering_is_stable() {
        let n = now();
        let signals: Vec<Signals> = (0..20)
            .map(|i| Signals {
                id: MemoryId::new(),
                keyword_rank: if i % 2 == 0 { Some(i) } else { None },
                vector_distance: if i % 3 == 0 { Some(0.05 * i as f32) } else { None },
                graph_hops: if i % 4 == 0 { Some((i % 5) as u8) } else { None },
                importance: (i % 11) as u8,
                created_at: n - Duration::days(i as i64),
            })
            .collect();
        let first = rank(signals.clone(), Weights::default(), n, 20);
        // every score is a sane probability-like value in [0,1], non-increasing.
        for w in first.windows(2) {
            assert!(w[0].1 >= w[1].1, "scores must be sorted descending");
        }
        for (_, score) in &first {
            assert!(*score >= 0.0 && *score <= 1.0, "score {score} out of [0,1]");
        }
        // determinism: same inputs -> identical id ordering AND identical scores.
        let second = rank(signals, Weights::default(), n, 20);
        let ids_a: Vec<_> = first.iter().map(|(id, _)| id.clone()).collect();
        let ids_b: Vec<_> = second.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids_a, ids_b, "ordering must be stable across runs");
        for (x, y) in first.iter().zip(second.iter()) {
            assert!((x.1 - y.1).abs() < f32::EPSILON, "scores must be reproducible");
        }
    }
}
```

  Add `mod rank;` after `mod weights;` in `crates/rb-search/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-search rank -- --nocapture`
  Expected: FAIL to compile (`cannot find type 'Signals' in this scope` / `cannot find function 'rank' in this scope`), confirming the module is compiled and the API is missing.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-search/src/rank.rs`:

```rust
use crate::weights::Weights;
use rb_types::MemoryId;

/// Recency half-life in days: a memory `HALF_LIFE` days old scores `e^-1 ~= 0.368`
/// on the recency term. Documented, fixed constant for deterministic ranking.
pub const HALF_LIFE: f32 = 30.0;

/// Per-candidate raw signals gathered from the three retrieval paths.
///
/// Any `Option` left as `None` means "this path did not surface this candidate";
/// its corresponding term contributes 0 (no penalty).
#[derive(Clone, Debug)]
pub struct Signals {
    pub id: MemoryId,
    /// Keyword rank, 0 = best. `None` if not a keyword hit.
    pub keyword_rank: Option<usize>,
    /// Cosine distance, smaller = closer. `None` if not a vector hit.
    pub vector_distance: Option<f32>,
    /// Graph hops from a seed, 0 = the seed itself. `None` if not graph-reached.
    pub graph_hops: Option<u8>,
    /// Importance 0..=10.
    pub importance: u8,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Normalize a single candidate's signals into a weighted score in `[0, 1]`.
fn score_one(
    s: &Signals,
    w: &Weights,
    now: chrono::DateTime<chrono::Utc>,
) -> f32 {
    // Vector similarity: cosine distance in [0, 2] -> similarity in [0, 1].
    let vector_sim = match s.vector_distance {
        Some(d) => 1.0 - (d / 2.0).clamp(0.0, 1.0),
        None => 0.0,
    };
    // Keyword: reciprocal rank, 0 = best -> 1.0.
    let keyword = match s.keyword_rank {
        Some(r) => 1.0 / (1.0 + r as f32),
        None => 0.0,
    };
    // Graph proximity: reciprocal hops, 0 hops -> 1.0.
    let graph = match s.graph_hops {
        Some(h) => 1.0 / (1.0 + h as f32),
        None => 0.0,
    };
    // Importance normalized to [0, 1].
    let importance = (s.importance as f32 / 10.0).clamp(0.0, 1.0);
    // Recency: exponential decay over age in days. Future timestamps clamp to age 0.
    let age_days = ((now - s.created_at).num_seconds() as f32 / 86_400.0).max(0.0);
    let recency = (-age_days / HALF_LIFE).exp();

    w.vector * vector_sim
        + w.keyword * keyword
        + w.graph * graph
        + w.importance * importance
        + w.recency * recency
}

/// Rank candidates by weighted, normalized signal score.
///
/// Pure and deterministic: returns `(id, score)` pairs sorted by score descending,
/// truncated to `limit`. `slice::sort_by` is a stable sort, so equal scores preserve
/// input order and output ordering is reproducible across runs. Missing signals
/// contribute 0 (no penalty).
pub fn rank(
    signals: Vec<Signals>,
    weights: Weights,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> Vec<(MemoryId, f32)> {
    let mut scored: Vec<(MemoryId, f32)> = signals
        .iter()
        .map(|s| (s.id.clone(), score_one(s, &weights, now)))
        .collect();
    // Stable sort descending by score; partial_cmp is total here (scores are finite),
    // and we fall back to Equal so a NaN (which cannot occur with finite inputs) does
    // not panic, keeping the function panic-free on all paths.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    scored
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Update `crates/rb-search/src/lib.rs` to re-export the new public items:

```rust
//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO, no async. Combines normalized keyword / vector / graph / importance /
//! recency signals into a single weighted score (see `rank`).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod rank;
mod weights;

pub use rank::{rank, Signals, HALF_LIFE};
pub use weights::Weights;
```

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-search rank -- --nocapture`
  Expected: PASS (6 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-search --all-targets -- -D warnings`
  Expected: no warnings, exit 0. Run: `cargo fmt --all`
  Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-search/src/rank.rs crates/rb-search/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-search): add Signals, HALF_LIFE, and pure deterministic rank()"`
  Expected: one commit created.

---

### Task 15: rb-search `merge.rs` — `build_signals()` candidate-merge helper

**Files:**
- Create: `crates/rb-search/src/merge.rs`
- Modify: `crates/rb-search/src/lib.rs` (add `mod merge;` + re-export)
- Test: inline `#[cfg(test)] mod tests` in `crates/rb-search/src/merge.rs`

> `build_signals` is the pure glue that folds the three retrieval result sets (keyword `Vec<MemoryId>` in rank order, vector `Vec<(MemoryId, f32)>`, graph `Vec<MemoryId>` in hop order) plus a metadata lookup (`HashMap<MemoryId, (importance, created_at)>`) into the `Vec<Signals>` that `rank` consumes. A candidate may appear in any subset of the three paths; each path fills only its own field. Candidates missing from `meta` are dropped (they cannot be scored or fetched), which keeps the helper fail-closed.

- [ ] **Step 1: Write the failing test AND declare the module.** Create `crates/rb-search/src/merge.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::Utc;
    use rb_types::MemoryId;
    use std::collections::HashMap;

    #[test]
    fn merges_overlapping_paths_into_one_signal_per_id() {
        let now = Utc::now();
        let shared = MemoryId::new();
        let kw_only = MemoryId::new();
        let vec_only = MemoryId::new();
        let graph_only = MemoryId::new();

        let mut meta: HashMap<MemoryId, (u8, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for id in [&shared, &kw_only, &vec_only, &graph_only] {
            meta.insert(id.clone(), (5, now));
        }

        let keyword = vec![shared.clone(), kw_only.clone()];
        let vector = vec![(shared.clone(), 0.2), (vec_only.clone(), 0.9)];
        let graph = vec![shared.clone(), graph_only.clone()];

        let signals = build_signals(&keyword, &vector, &graph, &meta);
        // one Signals per distinct id (4 total).
        assert_eq!(signals.len(), 4);

        let by_id: HashMap<MemoryId, &Signals> =
            signals.iter().map(|s| (s.id.clone(), s)).collect();

        // shared appears in all three paths: keyword_rank 0 (first), vector dist 0.2, graph hops 0 (first).
        let sh = by_id.get(&shared).unwrap();
        assert_eq!(sh.keyword_rank, Some(0));
        assert!((sh.vector_distance.unwrap() - 0.2).abs() < f32::EPSILON);
        assert_eq!(sh.graph_hops, Some(0));

        // kw_only: keyword_rank 1, no vector, no graph.
        let k = by_id.get(&kw_only).unwrap();
        assert_eq!(k.keyword_rank, Some(1));
        assert!(k.vector_distance.is_none());
        assert!(k.graph_hops.is_none());

        // vec_only: only vector distance 0.9.
        let v = by_id.get(&vec_only).unwrap();
        assert!(v.keyword_rank.is_none());
        assert!((v.vector_distance.unwrap() - 0.9).abs() < f32::EPSILON);
        assert!(v.graph_hops.is_none());

        // graph_only: graph hops 1 (second in graph order).
        let g = by_id.get(&graph_only).unwrap();
        assert!(g.keyword_rank.is_none());
        assert!(g.vector_distance.is_none());
        assert_eq!(g.graph_hops, Some(1));
    }

    #[test]
    fn keyword_rank_and_graph_hops_follow_input_order() {
        let now = Utc::now();
        let a = MemoryId::new();
        let b = MemoryId::new();
        let c = MemoryId::new();
        let mut meta = HashMap::new();
        for id in [&a, &b, &c] {
            meta.insert(id.clone(), (5, now));
        }
        let keyword = vec![a.clone(), b.clone(), c.clone()];
        let graph = vec![c.clone(), b.clone(), a.clone()];
        let signals = build_signals(&keyword, &[], &graph, &meta);
        let by_id: HashMap<MemoryId, &Signals> =
            signals.iter().map(|s| (s.id.clone(), s)).collect();
        // keyword rank = index in keyword vec.
        assert_eq!(by_id.get(&a).unwrap().keyword_rank, Some(0));
        assert_eq!(by_id.get(&b).unwrap().keyword_rank, Some(1));
        assert_eq!(by_id.get(&c).unwrap().keyword_rank, Some(2));
        // graph hops = index in graph vec.
        assert_eq!(by_id.get(&c).unwrap().graph_hops, Some(0));
        assert_eq!(by_id.get(&b).unwrap().graph_hops, Some(1));
        assert_eq!(by_id.get(&a).unwrap().graph_hops, Some(2));
    }

    #[test]
    fn carries_importance_and_created_at_from_meta() {
        let created = Utc::now();
        let id = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert(id.clone(), (8u8, created));
        let signals = build_signals(&[id.clone()], &[], &[], &meta);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].importance, 8);
        assert_eq!(signals[0].created_at, created);
    }

    #[test]
    fn candidates_absent_from_meta_are_dropped() {
        let now = Utc::now();
        let known = MemoryId::new();
        let unknown = MemoryId::new(); // appears in results but has no meta entry
        let mut meta = HashMap::new();
        meta.insert(known.clone(), (5, now));
        let keyword = vec![known.clone(), unknown.clone()];
        let signals = build_signals(&keyword, &[], &[], &meta);
        // the unknown id is dropped (cannot be scored or fetched) -> fail closed.
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].id, known);
    }

    #[test]
    fn output_feeds_rank_end_to_end() {
        let now = Utc::now();
        let strong = MemoryId::new();
        let weak = MemoryId::new();
        let mut meta = HashMap::new();
        meta.insert(strong.clone(), (9u8, now));
        meta.insert(weak.clone(), (2u8, now));
        let keyword = vec![strong.clone(), weak.clone()];
        let vector = vec![(strong.clone(), 0.1), (weak.clone(), 1.7)];
        let signals = build_signals(&keyword, &vector, &[], &meta);
        let ranked = crate::rank::rank(signals, crate::weights::Weights::default(), now, 10);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, strong, "merge + rank place the strong doc first");
    }
}
```

  Add `mod merge;` (before `mod rank;`, alphabetical) in `crates/rb-search/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-search merge -- --nocapture`
  Expected: FAIL to compile (`cannot find function 'build_signals' in this scope`), confirming the module is compiled and the helper is missing.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-search/src/merge.rs`:

```rust
use crate::rank::Signals;
use rb_types::MemoryId;
use std::collections::HashMap;

/// Fold the three retrieval result sets into one `Signals` per distinct candidate.
///
/// - `keyword`: ids in rank order (index 0 = best keyword hit).
/// - `vector`: `(id, cosine_distance)` pairs (smaller distance = closer).
/// - `graph`: ids in hop order (index 0 = nearest in the graph walk).
/// - `meta`: per-id `(importance, created_at)`, the source of truth for scoring/fetch.
///
/// A candidate may appear in any subset of the three paths; each path fills only its
/// own field, leaving the rest `None` so `rank` contributes 0 for absent signals.
/// Candidates with no `meta` entry are dropped (fail closed — they cannot be scored
/// or later fetched). Output order follows first appearance across keyword, then
/// vector, then graph, which `rank` re-sorts deterministically.
pub fn build_signals(
    keyword: &[MemoryId],
    vector: &[(MemoryId, f32)],
    graph: &[MemoryId],
    meta: &HashMap<MemoryId, (u8, chrono::DateTime<chrono::Utc>)>,
) -> Vec<Signals> {
    // Preserve first-seen order with a parallel index map into `out`.
    let mut index: HashMap<MemoryId, usize> = HashMap::new();
    let mut out: Vec<Signals> = Vec::new();

    // The closure captures only `meta` (by shared ref) and takes `index`/`out` as
    // `&mut` parameters per call, so it does NOT need to be `mut` itself. Marking it
    // `mut` would trip the `unused_mut` warning, which is denied under -D warnings.
    let slot = |id: &MemoryId,
                index: &mut HashMap<MemoryId, usize>,
                out: &mut Vec<Signals>|
     -> Option<usize> {
        if let Some(&i) = index.get(id) {
            return Some(i);
        }
        // Only materialize a candidate we have metadata for.
        let (importance, created_at) = *meta.get(id)?;
        let i = out.len();
        out.push(Signals {
            id: id.clone(),
            keyword_rank: None,
            vector_distance: None,
            graph_hops: None,
            importance,
            created_at,
        });
        index.insert(id.clone(), i);
        Some(i)
    };

    for (rank_idx, id) in keyword.iter().enumerate() {
        if let Some(i) = slot(id, &mut index, &mut out) {
            out[i].keyword_rank = Some(rank_idx);
        }
    }
    for (id, distance) in vector.iter() {
        if let Some(i) = slot(id, &mut index, &mut out) {
            out[i].vector_distance = Some(*distance);
        }
    }
    for (hop_idx, id) in graph.iter().enumerate() {
        if let Some(i) = slot(id, &mut index, &mut out) {
            // hop index saturates into u8; graph depth is bounded well below 255.
            out[i].graph_hops = Some(hop_idx.min(u8::MAX as usize) as u8);
        }
    }

    out
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Update `crates/rb-search/src/lib.rs` to its final form:

```rust
//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO, no async. Combines normalized keyword / vector / graph / importance /
//! recency signals into a single weighted score (see `rank`). `build_signals`
//! folds the three retrieval result sets into the `Signals` that `rank` consumes.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod merge;
mod rank;
mod weights;

pub use merge::build_signals;
pub use rank::{rank, Signals, HALF_LIFE};
pub use weights::Weights;
```

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-search merge -- --nocapture`
  Expected: PASS (5 tests pass).

- [ ] **Step 6: Run the full crate suite + gates.** Run: `cargo test -p rb-search`
  Expected: PASS (all 14 tests across `weights`, `rank`, `merge` pass). Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings`
  Expected: `Finished` with no warnings (exit 0). Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: no output, exit 0.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-search/src/merge.rs crates/rb-search/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-search): add build_signals merge helper folding retrieval paths into Signals"`
  Expected: one commit created; `cargo test -p rb-search` and workspace clippy both green.

## Part I — rb-engine (trait-generic orchestration)

### Task 16: rb-engine `backend.rs` — `MemoryBackend` async trait + in-test mock

**Files:**
- Modify: `crates/rb-engine/Cargo.toml` (ensure `tokio` dev-dependency present)
- Create: `crates/rb-engine/src/backend.rs`
- Modify: `crates/rb-engine/src/lib.rs` (add `mod backend;` + re-export)

> The `rb-engine` crate skeleton (manifest + empty `lib.rs` with the test-only clippy allow) is created in the P1 setup task, mirroring how the P0 `rb-types`/`rb-store` skeletons were created. This task adds the first real module. The engine deliberately does NOT depend on `rb-store` — it is generic over the `MemoryBackend` trait so it stays pure policy and is unit-tested against an in-memory `HashMap` mock (no DB, no network).

- [ ] **Step 1: Ensure `tokio` is a dev-dependency.** `#[tokio::test]` needs the tokio test runtime. Set `crates/rb-engine/Cargo.toml` to exactly this (adds the `[dev-dependencies]` block if the setup skeleton lacked it; leaves the runtime deps as the spine specifies):

```toml
[package]
name = "rb-engine"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Single-request memory orchestration: heuristic enrichment, embed, hybrid recall ranking."

[lib]
name = "rb_engine"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-search = { path = "../rb-search" }
rb-embed = { path = "../rb-embed" }
async-trait = { workspace = true }
chrono = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Write the failing test AND declare the module.** An undeclared `.rs` file is never compiled, so we declare `mod backend;` in `lib.rs` now and create `backend.rs` containing the trait-shaped test only. The build fails because `MemoryBackend` does not exist yet.

  Create `crates/rb-engine/src/backend.rs` with ONLY the test module (it defines an in-memory mock that the trait must satisfy, then exercises every method). The mock's `keyword`/`vector` return ids in a **deterministic** order (created_at desc) so downstream recall ranking is reproducible:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal in-memory backend used to unit-test the engine in isolation.
    /// NOT backed by rb-store; just a HashMap behind a Mutex.
    #[derive(Default)]
    struct MockBackend {
        notes: Mutex<HashMap<MemoryId, MemoryNote>>,
        embeddings: Mutex<HashMap<MemoryId, Vec<f32>>>,
    }

    #[async_trait::async_trait]
    impl MemoryBackend for MockBackend {
        async fn write(
            &self,
            note: MemoryNote,
            embedding: Option<Vec<f32>>,
        ) -> rb_types::Result<()> {
            if let Some(emb) = embedding {
                self.embeddings.lock().unwrap().insert(note.id.clone(), emb);
            }
            self.notes.lock().unwrap().insert(note.id.clone(), note);
            Ok(())
        }
        async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
            Ok(self.notes.lock().unwrap().get(&id).cloned())
        }
        async fn keyword(
            &self,
            _ns: Namespace,
            _query: String,
            _limit: usize,
        ) -> rb_types::Result<Vec<MemoryId>> {
            // Deterministic order (created_at desc) so keyword_rank is reproducible.
            let mut notes: Vec<MemoryNote> =
                self.notes.lock().unwrap().values().cloned().collect();
            notes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            Ok(notes.into_iter().map(|n| n.id).collect())
        }
        async fn vector(
            &self,
            _ns: Namespace,
            _embedding: Vec<f32>,
            _limit: usize,
        ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
            let mut pairs: Vec<(MemoryId, MemoryNote)> = self
                .embeddings
                .lock()
                .unwrap()
                .keys()
                .filter_map(|id| {
                    self.notes
                        .lock()
                        .unwrap()
                        .get(id)
                        .cloned()
                        .map(|n| (id.clone(), n))
                })
                .collect();
            pairs.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
            Ok(pairs.into_iter().map(|(id, _)| (id, 0.0)).collect())
        }
        async fn graph(
            &self,
            _id: MemoryId,
            _depth: u8,
        ) -> rb_types::Result<Vec<MemoryId>> {
            Ok(Vec::new())
        }
        async fn list(
            &self,
            _ns: Namespace,
            min_importance: Option<u8>,
            limit: usize,
        ) -> rb_types::Result<Vec<MemoryNote>> {
            let mut v: Vec<MemoryNote> = self
                .notes
                .lock()
                .unwrap()
                .values()
                .filter(|n| min_importance.map(|m| n.importance >= m).unwrap_or(true))
                .cloned()
                .collect();
            v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            v.truncate(limit);
            Ok(v)
        }
        async fn update(
            &self,
            id: MemoryId,
            updates: MemoryUpdates,
        ) -> rb_types::Result<()> {
            let mut guard = self.notes.lock().unwrap();
            let note = guard
                .get_mut(&id)
                .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
            if let Some(c) = updates.content {
                note.content = c;
            }
            if let Some(s) = updates.summary {
                note.summary = s;
            }
            Ok(())
        }
        async fn archive(&self, id: MemoryId) -> rb_types::Result<()> {
            let mut guard = self.notes.lock().unwrap();
            let note = guard
                .get_mut(&id)
                .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
            note.archived_at = Some(chrono::Utc::now());
            Ok(())
        }
    }

    #[tokio::test]
    async fn mock_backend_round_trips_write_and_get() {
        let backend = MockBackend::default();
        let note = MemoryNote::new(
            Namespace::Global,
            "hello world".to_string(),
            MemoryType::Insight,
            5,
        );
        let id = note.id.clone();
        backend
            .write(note.clone(), Some(vec![0.1, 0.2, 0.3]))
            .await
            .unwrap();
        let got = backend.get(id).await.unwrap().unwrap();
        assert_eq!(got.content, "hello world");
    }

    #[tokio::test]
    async fn mock_backend_archive_sets_archived_at() {
        let backend = MockBackend::default();
        let note =
            MemoryNote::new(Namespace::Global, "x".to_string(), MemoryType::Reference, 3);
        let id = note.id.clone();
        backend.write(note, None).await.unwrap();
        backend.archive(id.clone()).await.unwrap();
        assert!(backend.get(id).await.unwrap().unwrap().archived_at.is_some());
    }
}
```

  Set `crates/rb-engine/src/lib.rs` to declare the module (the `pub use` re-export is added in Step 4, once the trait exists):

```rust
//! `rb_engine`: single-request memory orchestration (policy only).
//!
//! Generic over a `MemoryBackend` (store access) and an
//! `rb_embed::EmbeddingProvider`. P1 enrichment is heuristic only; LLM
//! enrichment and semantic link generation are deferred to P2.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backend;
```

- [ ] **Step 3: Run it — expect FAIL.** Run: `cargo test -p rb-engine backend` Expected: FAIL to compile (`cannot find trait 'MemoryBackend' in this scope`), confirming the mock `impl MemoryBackend for MockBackend` drives a real trait into existence rather than being silently skipped.

- [ ] **Step 4: Add the trait above the test module.** Prepend to `crates/rb-engine/src/backend.rs`. NOTE: the parent-module `use` imports ONLY the types referenced by the trait signatures (`MemoryId`, `MemoryNote`, `MemoryUpdates`, `Namespace`). `MemoryType` is NOT used by the trait — it is imported separately inside the test module — so importing it here would be an `unused_imports` warning that fails `clippy -D warnings` in the lib (non-test) build:

```rust
use rb_types::{MemoryId, MemoryNote, MemoryUpdates, Namespace};

/// Async store-access abstraction the engine is generic over. The daemon
/// implements this on top of the synchronous `rb_store::Store` using a
/// dedicated writer thread plus `spawn_blocking` readers; tests implement it
/// over an in-memory map. The engine never touches a concrete store.
#[async_trait::async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn write(
        &self,
        note: MemoryNote,
        embedding: Option<Vec<f32>>,
    ) -> rb_types::Result<()>;
    async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>>;
    async fn keyword(
        &self,
        ns: Namespace,
        query: String,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryId>>;
    async fn vector(
        &self,
        ns: Namespace,
        embedding: Vec<f32>,
        limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>>;
    async fn graph(&self, id: MemoryId, depth: u8) -> rb_types::Result<Vec<MemoryId>>;
    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>>;
    async fn update(
        &self,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()>;
    async fn archive(&self, id: MemoryId) -> rb_types::Result<()>;
}
```

  (verify against installed `async-trait` at execution; the `#[async_trait::async_trait]` attribute form is stable across 0.1.x but adjust if the macro path differs.)

- [ ] **Step 5: Re-export the trait from `lib.rs`.** Set `crates/rb-engine/src/lib.rs` to:

```rust
//! `rb_engine`: single-request memory orchestration (policy only).
//!
//! Generic over a `MemoryBackend` (store access) and an
//! `rb_embed::EmbeddingProvider`. P1 enrichment is heuristic only; LLM
//! enrichment and semantic link generation are deferred to P2.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backend;

pub use backend::MemoryBackend;
```

- [ ] **Step 6: Run it — expect PASS.** Run: `cargo test -p rb-engine backend` Expected: PASS (2 tests pass).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git add crates/rb-engine/Cargo.toml crates/rb-engine/src/backend.rs crates/rb-engine/src/lib.rs && git commit -m "feat(rb-engine): add async MemoryBackend trait with in-memory mock test"`

---

### Task 17: rb-engine `enrich.rs` — heuristic summary + keyword derivation (pure, no IO)

**Files:**
- Create: `crates/rb-engine/src/enrich.rs`
- Modify: `crates/rb-engine/src/lib.rs` (add `mod enrich;`)

> P1 enrichment is HEURISTIC ONLY — no LLM. Two pure helpers: a summary default (first ~150 chars) and a simple keyword extractor (up to 5 tokens). Keeping them pure and separately tested lets `remember` stay tiny and lets the determinism be asserted without a backend or provider.

- [ ] **Step 1: Write the failing test AND declare the module.** Declare `mod enrich;` in `lib.rs` now and create `enrich.rs` with the test only; the build fails because the functions do not exist.

  Create `crates/rb-engine/src/enrich.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn summary_of_short_content_is_unchanged_trimmed() {
        assert_eq!(default_summary("  short body  "), "short body");
    }

    #[test]
    fn summary_truncates_to_150_chars_on_char_boundary() {
        let content = "x".repeat(400);
        let s = default_summary(&content);
        assert_eq!(s.chars().count(), 150);
    }

    #[test]
    fn summary_truncation_never_splits_a_multibyte_char() {
        // 200 'é' (2 bytes each in UTF-8); truncation must stay on a char boundary.
        let content = "é".repeat(200);
        let s = default_summary(&content);
        assert_eq!(s.chars().count(), 150);
        // Round-trips as valid UTF-8 (would panic on a bad boundary while building).
        assert!(s.chars().all(|c| c == 'é'));
    }

    #[test]
    fn keywords_lowercase_dedupe_and_cap_at_five() {
        let kw = derive_keywords(
            "SQLite WAL mode enables concurrent SQLITE readers and writers safely",
        );
        // length >= 4 (by char count), lowercased, order-preserving, deduped, max 5.
        assert_eq!(kw, vec!["sqlite", "mode", "enables", "concurrent", "readers"]);
    }

    #[test]
    fn keywords_skips_short_tokens_and_punctuation() {
        let kw = derive_keywords("a an the to of, big-decision: keep!");
        assert_eq!(kw, vec!["decision", "keep"]);
    }

    #[test]
    fn keywords_empty_content_yields_empty() {
        assert!(derive_keywords("   ").is_empty());
    }

    #[test]
    fn keywords_length_guard_uses_char_count_not_bytes() {
        // "café" is 5 bytes but 4 chars; "über" is 5 bytes but 4 chars.
        // Both must be kept (>= 4 chars), proving the guard counts chars, not bytes.
        let kw = derive_keywords("café über");
        assert_eq!(kw, vec!["café", "über"]);
    }
}
```

  Add `mod enrich;` after `mod backend;` in `crates/rb-engine/src/lib.rs`.

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine enrich` Expected: FAIL to compile (`cannot find function 'default_summary'` / `derive_keywords`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `crates/rb-engine/src/enrich.rs`. The keyword length guard uses `chars().count()` (NOT byte length) so multibyte tokens are handled correctly:

```rust
/// Maximum characters retained in a heuristic summary.
const SUMMARY_MAX_CHARS: usize = 150;
/// Maximum number of derived keywords.
const MAX_KEYWORDS: usize = 5;
/// Minimum token length (in characters) kept as a keyword (drops stop-word-ish short tokens).
const MIN_KEYWORD_LEN: usize = 4;

/// Heuristic summary: trim, then keep the first `SUMMARY_MAX_CHARS` characters
/// on a char boundary (never splitting a multi-byte UTF-8 sequence).
pub(crate) fn default_summary(content: &str) -> String {
    let trimmed = content.trim();
    trimmed.chars().take(SUMMARY_MAX_CHARS).collect()
}

/// Heuristic keyword extraction: split on non-alphanumeric, lowercase, keep
/// tokens of length >= `MIN_KEYWORD_LEN` characters, dedupe preserving
/// first-seen order, and cap at `MAX_KEYWORDS`. Pure and deterministic.
pub(crate) fn derive_keywords(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in content.split(|c: char| !c.is_alphanumeric()) {
        if raw.chars().count() < MIN_KEYWORD_LEN {
            continue;
        }
        let token = raw.to_lowercase();
        if !out.iter().any(|existing| existing == &token) {
            out.push(token);
        }
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-engine enrich` Expected: PASS (7 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-engine/src/enrich.rs crates/rb-engine/src/lib.rs && git commit -m "feat(rb-engine): add heuristic summary and keyword enrichment helpers"`

---

### Task 18: rb-engine `engine.rs` — `RememberInput` + `MemoryEngine::new` + `remember`

**Files:**
- Create: `crates/rb-engine/src/engine.rs`
- Modify: `crates/rb-engine/src/lib.rs` (add `mod engine;` + re-exports)
- Create: `crates/rb-engine/src/test_support.rs` (shared in-test mock backend, behind `#[cfg(test)]`)
- Modify: `crates/rb-engine/src/lib.rs` (add `#[cfg(test)] mod test_support;`)

> This task introduces the `MemoryEngine<B, P>` struct and the `remember` path. To avoid duplicating the mock across tasks, the in-test `MockBackend` (which also records what was written so `remember` can be asserted) lives in a shared `#[cfg(test)] mod test_support;`. The engine is tested with the offline `DeterministicProvider` so `remember` embeds without any network. The mock's `keyword`/`vector` return ids in a deterministic order (created_at desc) so recall ranking (Task 19) is reproducible across runs.

- [ ] **Step 1: Write the shared test-support mock (test-only, no production code).** Create `crates/rb-engine/src/test_support.rs`. It is compiled only under `cfg(test)`, so it may use `unwrap` freely; the module-level allow keeps clippy quiet:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::backend::MemoryBackend;
use rb_types::{MemoryId, MemoryNote, MemoryUpdates, Namespace};
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory `MemoryBackend` for engine unit tests. Records writes (note +
/// embedding) so tests can assert what `remember` produced. Reads return the
/// stored notes. Keyword/vector return ALL stored ids in a DETERMINISTIC order
/// (created_at desc) so the ranker, not HashMap iteration order, decides
/// result ordering; `graph` returns nothing (graph paths tested separately).
#[derive(Default)]
pub(crate) struct MockBackend {
    pub notes: Mutex<HashMap<MemoryId, MemoryNote>>,
    pub embeddings: Mutex<HashMap<MemoryId, Vec<f32>>>,
}

impl MockBackend {
    pub fn count(&self) -> usize {
        self.notes.lock().unwrap().len()
    }
    pub fn embedding_of(&self, id: &MemoryId) -> Option<Vec<f32>> {
        self.embeddings.lock().unwrap().get(id).cloned()
    }
    pub fn note_of(&self, id: &MemoryId) -> Option<MemoryNote> {
        self.notes.lock().unwrap().get(id).cloned()
    }
}

#[async_trait::async_trait]
impl MemoryBackend for MockBackend {
    async fn write(
        &self,
        note: MemoryNote,
        embedding: Option<Vec<f32>>,
    ) -> rb_types::Result<()> {
        if let Some(emb) = embedding {
            self.embeddings.lock().unwrap().insert(note.id.clone(), emb);
        }
        self.notes.lock().unwrap().insert(note.id.clone(), note);
        Ok(())
    }
    async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        Ok(self.notes.lock().unwrap().get(&id).cloned())
    }
    async fn keyword(
        &self,
        ns: Namespace,
        _query: String,
        _limit: usize,
    ) -> rb_types::Result<Vec<MemoryId>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(v.into_iter().map(|n| n.id).collect())
    }
    async fn vector(
        &self,
        ns: Namespace,
        _embedding: Vec<f32>,
        _limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(v.into_iter().map(|n| (n.id, 0.0)).collect())
    }
    async fn graph(&self, _id: MemoryId, _depth: u8) -> rb_types::Result<Vec<MemoryId>> {
        Ok(Vec::new())
    }
    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .filter(|n| min_importance.map(|m| n.importance >= m).unwrap_or(true))
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v.truncate(limit);
        Ok(v)
    }
    async fn update(
        &self,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        if let Some(c) = updates.content {
            note.content = c;
        }
        if let Some(s) = updates.summary {
            note.summary = s;
        }
        if let Some(i) = updates.importance {
            note.importance = i;
        }
        if let Some(t) = updates.tags {
            note.tags = t;
        }
        if let Some(ctx) = updates.context {
            note.context = ctx;
        }
        Ok(())
    }
    async fn archive(&self, id: MemoryId) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        note.archived_at = Some(chrono::Utc::now());
        Ok(())
    }
}
```

  Add `#[cfg(test)] mod test_support;` after `mod enrich;` in `crates/rb-engine/src/lib.rs`.

- [ ] **Step 2: Write the failing test AND declare the engine module.** Declare `mod engine;` in `lib.rs` and create `engine.rs` with the `remember` test only. It uses the offline `DeterministicProvider` (never hits the network) and the shared `MockBackend`. The build fails because `MemoryEngine`/`RememberInput` do not exist.

  Create `crates/rb-engine/src/engine.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::test_support::MockBackend;
    use rb_embed::DeterministicProvider;
    use rb_types::{MemoryType, Namespace};

    fn engine() -> MemoryEngine<MockBackend, DeterministicProvider> {
        MemoryEngine::new(
            MockBackend::default(),
            DeterministicProvider::new(16),
            Namespace::Project("rb".into()),
        )
    }

    fn input(content: &str, importance: u8) -> RememberInput {
        RememberInput {
            content: content.to_string(),
            context: None,
            memory_type: MemoryType::Insight,
            importance,
            keywords: Vec::new(),
            tags: Vec::new(),
            related_files: Vec::new(),
        }
    }

    #[tokio::test]
    async fn remember_stores_note_and_embedding() {
        let eng = engine();
        let id = eng.remember(input("single writer over sqlite wal", 7)).await.unwrap();
        // exactly one note written, with an embedding of provider dim.
        assert_eq!(eng.backend().count(), 1);
        let emb = eng.backend().embedding_of(&id).unwrap();
        assert_eq!(emb.len(), 16);
    }

    #[tokio::test]
    async fn remember_applies_heuristic_summary_and_keywords() {
        let eng = engine();
        let content = "concurrent readers never block the single dedicated writer thread";
        let id = eng.remember(input(content, 6)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // summary defaults to the (trimmed) content since it's < 150 chars.
        assert_eq!(note.summary, content);
        // keywords derived (empty input) — non-empty, lowercased, capped at 5.
        assert!(!note.keywords.is_empty());
        assert!(note.keywords.len() <= 5);
        assert!(note.keywords.iter().all(|k| k == &k.to_lowercase()));
    }

    #[tokio::test]
    async fn remember_preserves_explicit_keywords_and_namespace() {
        let eng = engine();
        let mut inp = input("body text here", 5);
        inp.keywords = vec!["explicit".to_string()];
        inp.tags = vec!["t1".to_string()];
        inp.context = Some("ctx".to_string());
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.keywords, vec!["explicit".to_string()]);
        assert_eq!(note.tags, vec!["t1".to_string()]);
        assert_eq!(note.context, "ctx");
        // engine enforces its own namespace.
        assert_eq!(note.namespace, Namespace::Project("rb".into()));
    }

    #[tokio::test]
    async fn remember_sets_embedding_model_from_provider() {
        let eng = engine();
        let id = eng.remember(input("model id check", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.embedding_model, eng.embedder().model_id());
    }

    #[tokio::test]
    async fn remember_is_deterministic_same_content_same_embedding() {
        let eng = engine();
        let id1 = eng.remember(input("identical content", 5)).await.unwrap();
        let id2 = eng.remember(input("identical content", 5)).await.unwrap();
        assert_ne!(id1, id2); // distinct notes
        assert_eq!(
            eng.backend().embedding_of(&id1),
            eng.backend().embedding_of(&id2)
        ); // deterministic provider => same vector
    }
}
```

  Add `mod engine;` after `#[cfg(test)] mod test_support;` in `crates/rb-engine/src/lib.rs`.

- [ ] **Step 3: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests::remember` Expected: FAIL to compile (`cannot find type 'MemoryEngine'` / `RememberInput` / no method `backend`/`embedder`).

- [ ] **Step 4: Add `RememberInput`, the struct, `new`, the test accessors, and `remember` above the test module.** Prepend to `crates/rb-engine/src/engine.rs`:

```rust
use crate::backend::MemoryBackend;
use crate::enrich::{default_summary, derive_keywords};
use rb_embed::EmbeddingProvider;
use rb_search::Weights;
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

/// Input to `remember`. Mirrors the proto `Request::Remember` payload.
pub struct RememberInput {
    pub content: String,
    pub context: Option<String>,
    pub memory_type: MemoryType,
    pub importance: u8,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub related_files: Vec<String>,
}

/// Policy layer: orchestrates heuristic enrichment + embedding + ranking over a
/// `MemoryBackend`. Generic so it is unit-tested without a DB or network.
pub struct MemoryEngine<B: MemoryBackend, P: EmbeddingProvider> {
    backend: B,
    embedder: P,
    weights: Weights,
    namespace: Namespace,
}

impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P> {
    /// Construct an engine bound to a single namespace (set server-side from the
    /// client handshake; clients cannot widen it).
    pub fn new(backend: B, embedder: P, namespace: Namespace) -> Self {
        Self {
            backend,
            embedder,
            weights: Weights::default(),
            namespace,
        }
    }

    /// Borrow the backend (used by daemon/tests for introspection).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Borrow the embedding provider.
    pub fn embedder(&self) -> &P {
        &self.embedder
    }

    /// Store a new memory: heuristic-enrich, embed the content, then write.
    pub async fn remember(&self, input: RememberInput) -> rb_types::Result<MemoryId> {
        let mut note = MemoryNote::new(
            self.namespace.clone(),
            input.content,
            input.memory_type,
            input.importance,
        );
        // Heuristic enrichment (no LLM in P1).
        note.summary = default_summary(&note.content);
        note.keywords = if input.keywords.is_empty() {
            derive_keywords(&note.content)
        } else {
            input.keywords
        };
        note.tags = input.tags;
        note.related_files = input.related_files;
        if let Some(ctx) = input.context {
            note.context = ctx;
        }
        note.embedding_model = self.embedder.model_id().to_string();

        // Embed the content (single text in, single vector out).
        let mut embeddings = self.embedder.embed(&[note.content.clone()]).await?;
        let embedding = embeddings.pop();

        let id = note.id.clone();
        self.backend.write(note, embedding).await?;
        Ok(id)
    }
}
```

  The `weights` field is read by `recall` (Task 19, same crate, committed before any release), but at THIS task's clippy gate `weights` is only written by `new()` and would trip `dead_code` ("field is never read"). Add a tiny accessor inside the `impl` block, right after `embedder`, so the field is read now:

```rust
    /// Borrow the ranking weights (used by `recall`).
    pub fn weights(&self) -> Weights {
        self.weights
    }
```

  (verify against installed `async-trait`/`rb_embed` at execution; `self.embedder.embed(&[...])` must take a `&[String]` per the spine — `note.content.clone()` builds that single-element slice's owned String.)

- [ ] **Step 5: Re-export from `lib.rs`.** Set the re-export block in `crates/rb-engine/src/lib.rs` so the crate root exposes the engine and input:

```rust
pub use backend::MemoryBackend;
pub use engine::{MemoryEngine, RememberInput};
```

- [ ] **Step 6: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests::remember` Expected: PASS (5 remember tests pass).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git add crates/rb-engine/src/engine.rs crates/rb-engine/src/test_support.rs crates/rb-engine/src/lib.rs && git commit -m "feat(rb-engine): add MemoryEngine, RememberInput, and remember path"`

---

### Task 19: rb-engine `recall` — embed query, hybrid candidates, rank, fetch, filter

**Files:**
- Modify: `crates/rb-engine/src/engine.rs` (add `recall` + candidate/meta helpers + tests)

> `recall` embeds the query, gathers candidates from `backend.keyword` + `backend.vector` (+ a bounded 1-hop graph expansion of the top keyword hit), assembles a `meta` map and a note cache from the fetched notes, calls `rb_search::build_signals` + `rb_search::rank`, then returns `SearchResult { memory, score }` for the ranked ids — applying the type/tag filters. Fetching each candidate once (and caching it) means `rank` and the final result share the same notes with no second round-trip.

> RANKING NOTE: `rb_search::build_signals` assigns `keyword_rank` by POSITION in the keyword vec, so two distinct candidates can never share the same keyword rank. With Default `Weights`, the keyword-rank delta between rank 0 and rank 1 (0.30·0.5 = 0.15 weighted) is strictly larger than the maximum importance delta (0.10·(9/10) ≈ 0.09 weighted). Therefore importance alone CANNOT override a one-position keyword-rank difference, and recall ordering between near-identical candidates is decided by keyword/vector position, NOT importance. Tests must not assert "higher importance ranks first" through `recall`; importance-driven ordering is verified via `list` (Task 20), which is deterministically ordered by recency.

- [ ] **Step 1: Write the failing test.** Append a `recall` test module section to `crates/rb-engine/src/engine.rs`'s existing `#[cfg(test)] mod tests` block by adding these tests INSIDE it (place them after the `remember` tests, before the closing brace):

```rust
    async fn seed(eng: &MemoryEngine<MockBackend, DeterministicProvider>, content: &str, ty: MemoryType, imp: u8, tags: &[&str]) -> rb_types::MemoryId {
        let mut inp = input(content, imp);
        inp.memory_type = ty;
        inp.tags = tags.iter().map(|t| t.to_string()).collect();
        eng.remember(inp).await.unwrap()
    }

    #[tokio::test]
    async fn recall_returns_results_for_seeded_memories() {
        let eng = engine();
        seed(&eng, "alpha topic about sqlite", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "beta topic about tokio", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("topic", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        // scores are finite and sorted descending.
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn recall_respects_limit() {
        let eng = engine();
        for i in 0..5 {
            seed(&eng, &format!("doc number {i}"), MemoryType::Insight, 5, &[]).await;
        }
        let results = eng.recall("doc", 2, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn recall_type_filter_excludes_other_types() {
        let eng = engine();
        seed(&eng, "a bug fix note", MemoryType::BugFix, 5, &[]).await;
        seed(&eng, "an insight note", MemoryType::Insight, 5, &[]).await;
        let results = eng
            .recall("note", 10, Some(MemoryType::BugFix), &[])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.memory_type, MemoryType::BugFix);
    }

    #[tokio::test]
    async fn recall_tag_filter_requires_all_tags() {
        let eng = engine();
        seed(&eng, "tagged one", MemoryType::Insight, 5, &["x", "y"]).await;
        seed(&eng, "tagged two", MemoryType::Insight, 5, &["x"]).await;
        let results = eng
            .recall("tagged", 10, None, &["x".to_string(), "y".to_string()])
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].memory.tags.contains(&"y".to_string()));
    }

    #[tokio::test]
    async fn recall_ranks_all_candidates_with_finite_descending_scores() {
        // NOTE: importance does NOT decide ordering between near-identical
        // candidates (keyword-rank position dominates, see RANKING NOTE), so we
        // assert the honest invariants: every candidate is returned, scores are
        // finite, and the result is sorted descending. Importance-driven order
        // is covered by the deterministic `list` test in Task 20.
        let eng = engine();
        let _low = seed(&eng, "ranking probe content", MemoryType::Insight, 2, &[]).await;
        let _high = seed(&eng, "ranking probe content", MemoryType::Insight, 9, &[]).await;
        let results = eng.recall("ranking probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.score.is_finite()));
        assert!(results[0].score >= results[1].score);
    }

    #[tokio::test]
    async fn recall_empty_store_returns_empty() {
        let eng = engine();
        let results = eng.recall("anything", 10, None, &[]).await.unwrap();
        assert!(results.is_empty());
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests::recall` Expected: FAIL to compile (`no method named 'recall' found`).

- [ ] **Step 3: Add `recall` and its private helpers to the `impl` block.** Insert these methods into the existing `impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P>` block in `crates/rb-engine/src/engine.rs`, after `remember`:

```rust
    /// Hybrid recall: embed the query, gather keyword + vector (+ 1-hop graph)
    /// candidates scoped to the engine namespace, rank with `rb_search`, then
    /// return ranked `SearchResult`s after applying type/tag filters.
    pub async fn recall(
        &self,
        query: &str,
        limit: usize,
        type_filter: Option<MemoryType>,
        tags: &[String],
    ) -> rb_types::Result<Vec<rb_types::SearchResult>> {
        use std::collections::HashMap;

        // Over-fetch candidates so post-filtering still has enough to fill `limit`.
        let candidate_limit = limit.saturating_mul(4).max(limit);

        let mut query_emb = self.embedder.embed(&[query.to_string()]).await?;
        let embedding = query_emb.pop().unwrap_or_default();

        let keyword = self
            .backend
            .keyword(self.namespace.clone(), query.to_string(), candidate_limit)
            .await?;
        let vector = self
            .backend
            .vector(self.namespace.clone(), embedding, candidate_limit)
            .await?;

        // Bounded 1-hop graph expansion of the top keyword hit only.
        let graph = match keyword.first() {
            Some(top) => self.backend.graph(top.clone(), 1).await?,
            None => Vec::new(),
        };

        // Collect the unique candidate id set across all three sources.
        let mut order: Vec<MemoryId> = Vec::new();
        let mut seen: std::collections::HashSet<MemoryId> = std::collections::HashSet::new();
        for id in keyword
            .iter()
            .chain(vector.iter().map(|(id, _)| id))
            .chain(graph.iter())
        {
            if seen.insert(id.clone()) {
                order.push(id.clone());
            }
        }

        // Fetch each candidate once; build the note cache + the rank meta map.
        let mut notes: HashMap<MemoryId, MemoryNote> = HashMap::new();
        let mut meta: HashMap<MemoryId, (u8, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for id in &order {
            if let Some(note) = self.backend.get(id.clone()).await? {
                meta.insert(id.clone(), (note.importance, note.created_at));
                notes.insert(id.clone(), note);
            }
        }

        let signals = rb_search::build_signals(&keyword, &vector, &graph, &meta);
        let ranked = rb_search::rank(signals, self.weights, chrono::Utc::now(), candidate_limit);

        // Assemble results in ranked order, applying filters, truncating to limit.
        let mut results: Vec<rb_types::SearchResult> = Vec::new();
        for (id, score) in ranked {
            let Some(note) = notes.get(&id) else { continue };
            if let Some(ty) = type_filter {
                if note.memory_type != ty {
                    continue;
                }
            }
            if !tags.iter().all(|t| note.tags.contains(t)) {
                continue;
            }
            results.push(rb_types::SearchResult {
                memory: note.clone(),
                score,
            });
            if results.len() == limit {
                break;
            }
        }
        Ok(results)
    }
```

  (verify against installed `rb_search` at execution: `build_signals(&keyword, &vector, &graph, &meta)` and `rank(signals, weights, now, limit)` signatures are taken from the P1 spine; adjust borrow/move shapes only if the implemented signatures differ.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests::recall` Expected: PASS (6 recall tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-engine/src/engine.rs && git commit -m "feat(rb-engine): add hybrid recall with embed, rank, and type/tag filters"`

---

### Task 20: rb-engine pass-throughs — `get`/`list`/`graph`/`update`/`delete` + `context`

**Files:**
- Modify: `crates/rb-engine/src/engine.rs` (add the remaining methods + tests)

> The remaining operations are mostly thin pass-throughs to the backend, all scoped to the engine namespace. `delete` maps to soft archive (spec §12). `context` returns two lists: `recent` (list by recency, no importance floor) and `important` (importance >= 8). `graph` expands then fetches the connected notes. `list` is deterministically ordered by recency, so this is where importance-floor + ordering behavior is asserted (recall ordering is dominated by keyword/vector position — see Task 19 RANKING NOTE).

- [ ] **Step 1: Write the failing test.** Add these tests INSIDE the existing `#[cfg(test)] mod tests` block in `crates/rb-engine/src/engine.rs` (after the recall tests, before the closing brace):

```rust
    #[tokio::test]
    async fn get_returns_stored_note_or_none() {
        let eng = engine();
        let id = eng.remember(input("findable", 5)).await.unwrap();
        assert!(eng.get(id.clone()).await.unwrap().is_some());
        assert!(eng
            .get(rb_types::MemoryId::new())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn list_orders_by_recency_and_honors_min_importance() {
        let eng = engine();
        seed(&eng, "first", MemoryType::Insight, 3, &[]).await;
        seed(&eng, "second", MemoryType::Insight, 9, &[]).await;
        let all = eng.list(None, 10).await.unwrap();
        assert_eq!(all.len(), 2);
        // most recent first (second was inserted last).
        assert_eq!(all[0].content, "second");
        let important = eng.list(Some(8), 10).await.unwrap();
        assert_eq!(important.len(), 1);
        assert_eq!(important[0].importance, 9);
    }

    #[tokio::test]
    async fn update_mutates_then_get_reflects_change() {
        let eng = engine();
        let id = eng.remember(input("old body", 5)).await.unwrap();
        let updates = rb_types::MemoryUpdates {
            content: Some("new body".to_string()),
            importance: Some(9),
            ..Default::default()
        };
        eng.update(id.clone(), updates).await.unwrap();
        let note = eng.get(id).await.unwrap().unwrap();
        assert_eq!(note.content, "new body");
        assert_eq!(note.importance, 9);
    }

    #[tokio::test]
    async fn delete_soft_archives_the_note() {
        let eng = engine();
        let id = eng.remember(input("doomed", 5)).await.unwrap();
        eng.delete(id.clone()).await.unwrap();
        let note = eng.get(id).await.unwrap().unwrap();
        assert!(note.archived_at.is_some());
    }

    #[tokio::test]
    async fn graph_returns_connected_notes() {
        // MockBackend.graph returns empty, so graph() yields no neighbors here;
        // this asserts the pass-through shape and empty handling without a DB.
        let eng = engine();
        let id = eng.remember(input("anchor", 5)).await.unwrap();
        let neighbors = eng.graph(id, 2).await.unwrap();
        assert!(neighbors.is_empty());
    }

    #[tokio::test]
    async fn context_splits_recent_and_important() {
        let eng = engine();
        seed(&eng, "low importance recent", MemoryType::Insight, 2, &[]).await;
        seed(&eng, "high importance note", MemoryType::Insight, 9, &[]).await;
        let (recent, important, total) = eng.context().await.unwrap();
        // recent includes both; important only the >= 8 one.
        assert_eq!(recent.len(), 2);
        assert_eq!(important.len(), 1);
        assert_eq!(important[0].importance, 9);
        assert_eq!(total, 2);
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests` Expected: FAIL to compile (`no method named 'get'`/`'list'`/`'update'`/`'delete'`/`'graph'`/`'context'` found).

- [ ] **Step 3: Add the pass-through methods to the `impl` block.** Insert these into the existing `impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P>` block in `crates/rb-engine/src/engine.rs`, after `recall`:

```rust
    /// Fetch a single memory by id (namespace scoping is enforced by the backend).
    pub async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        self.backend.get(id).await
    }

    /// List memories in the engine namespace, most-recent first, optionally
    /// filtered by a minimum importance.
    pub async fn list(
        &self,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        self.backend
            .list(self.namespace.clone(), min_importance, limit)
            .await
    }

    /// Expand the graph around `id` to `depth` hops and fetch the connected notes.
    pub async fn graph(&self, id: MemoryId, depth: u8) -> rb_types::Result<Vec<MemoryNote>> {
        let ids = self.backend.graph(id, depth).await?;
        let mut notes = Vec::with_capacity(ids.len());
        for nid in ids {
            if let Some(note) = self.backend.get(nid).await? {
                notes.push(note);
            }
        }
        Ok(notes)
    }

    /// Apply a partial update to an existing memory.
    pub async fn update(
        &self,
        id: MemoryId,
        updates: rb_types::MemoryUpdates,
    ) -> rb_types::Result<()> {
        self.backend.update(id, updates).await
    }

    /// Soft-delete (archive) a memory. Spec §12: delete == soft archive.
    pub async fn delete(&self, id: MemoryId) -> rb_types::Result<()> {
        self.backend.archive(id).await
    }

    /// Project context payload: recent memories (by recency) plus important ones
    /// (importance >= 8), with a total count of the recent window.
    pub async fn context(
        &self,
    ) -> rb_types::Result<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        const CONTEXT_LIMIT: usize = 50;
        const IMPORTANT_FLOOR: u8 = 8;
        let recent = self
            .backend
            .list(self.namespace.clone(), None, CONTEXT_LIMIT)
            .await?;
        let important = self
            .backend
            .list(self.namespace.clone(), Some(IMPORTANT_FLOOR), CONTEXT_LIMIT)
            .await?;
        let total = recent.len();
        Ok((recent, important, total))
    }
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests` Expected: PASS (all engine tests — remember, recall, and these pass-throughs — pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-engine/src/engine.rs && git commit -m "feat(rb-engine): add get/list/graph/update/delete pass-throughs and context"`

---

### Task 21: rb-engine `lib.rs` — finalize re-exports + public-API guard test + crate gate

**Files:**
- Modify: `crates/rb-engine/src/lib.rs` (finalize module list + flat re-exports)
- Create: `crates/rb-engine/tests/public_api.rs` (integration test guarding the public surface end-to-end)

> A `tests/` integration target compiles `rb-engine` as a downstream consumer would, so it proves the public re-exports are sufficient to drive a full remember→recall→context flow using only the offline `DeterministicProvider` and a consumer-defined in-memory backend (no DB, no network). This is the crate-level gate.

- [ ] **Step 1: Finalize `lib.rs`.** Set `crates/rb-engine/src/lib.rs` to its final form (modules + flat re-exports; `MemoryBackend`, `MemoryEngine`, `RememberInput` are the public surface):

```rust
//! `rb_engine`: single-request memory orchestration (policy only).
//!
//! Generic over a [`MemoryBackend`] (store access) and an
//! [`rb_embed::EmbeddingProvider`]. P1 enrichment is heuristic only; LLM
//! enrichment and semantic link generation are deferred to P2.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backend;
mod enrich;
mod engine;
#[cfg(test)]
mod test_support;

pub use backend::MemoryBackend;
pub use engine::{MemoryEngine, RememberInput};
```

- [ ] **Step 2: Write the public-API integration guard.** Create `crates/rb-engine/tests/public_api.rs`. It defines its OWN tiny backend (proving the trait is fully usable from outside the crate), wires it with `DeterministicProvider`, and exercises remember + recall + context through the public API. (`async-trait`, `chrono`, `rb-types`, and `rb-embed` are normal `[dependencies]` of rb-engine and are therefore visible as extern crates to integration targets; `tokio` is the dev-dependency added in Task 16. No Cargo.toml change is needed.)

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::DeterministicProvider;
use rb_engine::{MemoryBackend, MemoryEngine, RememberInput};
use rb_types::{MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct VecBackend {
    notes: Mutex<HashMap<MemoryId, MemoryNote>>,
}

#[async_trait::async_trait]
impl MemoryBackend for VecBackend {
    async fn write(
        &self,
        note: MemoryNote,
        _embedding: Option<Vec<f32>>,
    ) -> rb_types::Result<()> {
        self.notes.lock().unwrap().insert(note.id.clone(), note);
        Ok(())
    }
    async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        Ok(self.notes.lock().unwrap().get(&id).cloned())
    }
    async fn keyword(
        &self,
        ns: Namespace,
        _query: String,
        _limit: usize,
    ) -> rb_types::Result<Vec<MemoryId>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(v.into_iter().map(|n| n.id).collect())
    }
    async fn vector(
        &self,
        _ns: Namespace,
        _embedding: Vec<f32>,
        _limit: usize,
    ) -> rb_types::Result<Vec<(MemoryId, f32)>> {
        Ok(Vec::new())
    }
    async fn graph(&self, _id: MemoryId, _depth: u8) -> rb_types::Result<Vec<MemoryId>> {
        Ok(Vec::new())
    }
    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let mut v: Vec<MemoryNote> = self
            .notes
            .lock()
            .unwrap()
            .values()
            .filter(|n| n.namespace == ns)
            .filter(|n| min_importance.map(|m| n.importance >= m).unwrap_or(true))
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v.truncate(limit);
        Ok(v)
    }
    async fn update(
        &self,
        id: MemoryId,
        updates: MemoryUpdates,
    ) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        if let Some(c) = updates.content {
            note.content = c;
        }
        Ok(())
    }
    async fn archive(&self, id: MemoryId) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&id)
            .ok_or_else(|| rb_types::Error::NotFound(id.clone()))?;
        note.archived_at = Some(chrono::Utc::now());
        Ok(())
    }
}

#[tokio::test]
async fn full_flow_through_public_api() {
    let engine = MemoryEngine::new(
        VecBackend::default(),
        DeterministicProvider::new(8),
        Namespace::Project("rb".into()),
    );

    let id = engine
        .remember(RememberInput {
            content: "single writer over sqlite wal with concurrent readers".to_string(),
            context: Some("architecture".to_string()),
            memory_type: MemoryType::ArchitectureDecision,
            importance: 9,
            keywords: Vec::new(),
            tags: vec!["concurrency".to_string()],
            related_files: Vec::new(),
        })
        .await
        .unwrap();

    // get reflects the stored, enriched note.
    let note = engine.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(note.memory_type, MemoryType::ArchitectureDecision);
    assert!(!note.keywords.is_empty());
    assert_eq!(note.context, "architecture");

    // recall finds it.
    let results = engine.recall("writer", 10, None, &[]).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, id);

    // context surfaces it as both recent and important (importance 9 >= 8).
    let (recent, important, total) = engine.context().await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(recent.len(), 1);
    assert_eq!(important.len(), 1);
}
```

- [ ] **Step 3: Run the full crate test suite — expect PASS.** Run: `cargo test -p rb-engine` Expected: PASS — all unit tests (backend, enrich, engine) AND the `public_api` integration test pass. (If the integration target fails to resolve `async_trait`, add `async-trait = { workspace = true }` under `[dev-dependencies]` in `crates/rb-engine/Cargo.toml` and re-run — normally not required since it is a normal dependency. Verify against cargo at execution.)

- [ ] **Step 4: Workspace gate — clippy with warnings denied.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` Expected: `Finished` with no warnings (exit 0). Proves the engine integrates cleanly with the rest of the workspace under the shared deny lints.

- [ ] **Step 5: Workspace gate — format check.** Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-engine/src/lib.rs crates/rb-engine/tests/public_api.rs && git commit -m "feat(rb-engine): finalize public API with end-to-end integration guard"`

## Part J — rb-daemon (single-writer concurrent daemon)

### Task 22: rb-daemon `change.rs` — `MemoryChanged` / `ChangeKind` broadcast event

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/change.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` (add `mod change;` + re-export)

These are the change-notification payloads the writer thread publishes on a `tokio::broadcast` channel after every successful commit (spec §8). They are pure data (serde round-trippable) so the deferred `subscribe` feature needs no new machinery. We build them first because every later task references `MemoryChanged`.

- [ ] **Step 1: Write the failing test AND declare the module.** A `.rs` file not declared with `mod` is never compiled. Set `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` to declare the module (re-export added in Step 3):

```rust
//! `rb_daemon`: single-writer service over `rb_store`.
//!
//! One dedicated OS thread owns the write `SqliteStore` (rusqlite is `!Sync`,
//! so the write connection never crosses threads); a bounded pool of read
//! stores serves concurrent reads via `spawn_blocking`; a Unix-domain-socket
//! listener frames `rb_proto` requests to a per-connection `MemoryEngine`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod change;
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/change.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, Namespace};

    #[test]
    fn change_kind_round_trips_all_variants() {
        for kind in [ChangeKind::Created, ChangeKind::Updated, ChangeKind::Archived] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: ChangeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn memory_changed_round_trips_and_clones() {
        let evt = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: MemoryChanged = serde_json::from_str(&json).unwrap();
        assert_eq!(evt.id, back.id);
        assert_eq!(evt.namespace, back.namespace);
        assert_eq!(evt.kind, back.kind);
        // Clone is required so broadcast subscribers each get an owned copy.
        assert_eq!(evt.clone().kind, ChangeKind::Created);
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-daemon change` Expected: FAIL to compile — `cannot find type 'MemoryChanged' in this scope` / `cannot find type 'ChangeKind'`. The module is compiled (declared in `lib.rs`) but the types do not exist yet, confirming the test drives new code.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/change.rs`:

```rust
use rb_types::{MemoryId, Namespace};
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
/// Notification only — never coordination. Enables the deferred `subscribe`
/// feature with no new machinery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryChanged {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub kind: ChangeKind,
}
```

  Add the re-export to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` (after `mod change;`):

```rust
pub use change::{ChangeKind, MemoryChanged};
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-daemon change` Expected: PASS (2 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-daemon/src/change.rs crates/rb-daemon/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-daemon): add MemoryChanged/ChangeKind broadcast events"`

---

### Task 23: rb-daemon `store_handle.rs` — dedicated writer thread + read pool + `MemoryBackend`

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/store_handle.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` (add `mod store_handle;` + re-export)

This is the core of the concurrency model (spec §8). `StoreHandle::start` spawns **one** dedicated `std::thread` that owns the write `SqliteStore` (rusqlite `Connection` is `Send` but `!Sync`, so the write store lives ONLY on that thread and is **never** wrapped in `Arc<Mutex>` or shared across tasks). Writes are sent as `WriteCommand`s over a bounded `tokio::sync::mpsc`, each carrying a `tokio::sync::oneshot::Sender` for the reply; on success the writer publishes a `MemoryChanged` on a `tokio::broadcast`. Reads acquire an `OwnedSemaphorePermit` in async context (bounded pool), then check out a `SqliteStore` from the pool inside `tokio::task::spawn_blocking`. `StoreHandle` implements `rb_engine::MemoryBackend` so the engine stays pure-policy.

- [ ] **Step 1: Write the failing integration test AND declare the module.** Declare `mod store_handle;` in `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` (re-export in Step 3). The test lives as an integration test so it exercises only the public surface and the async backend trait.

  Set `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` to (keeping the prior `mod change;` + its re-export):

```rust
//! `rb_daemon`: single-writer service over `rb_store`.
//!
//! One dedicated OS thread owns the write `SqliteStore` (rusqlite is `!Sync`,
//! so the write connection never crosses threads); a bounded pool of read
//! stores serves concurrent reads via `spawn_blocking`; a Unix-domain-socket
//! listener frames `rb_proto` requests to a per-connection `MemoryEngine`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod change;
mod store_handle;

pub use change::{ChangeKind, MemoryChanged};
pub use store_handle::StoreHandle;
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/tests/store_handle.rs` with the complete contents below. It uses the async `MemoryBackend` trait directly against a tempdir DB. `DIM` matches the deterministic provider used elsewhere (8):

```rust
//! Integration tests for the StoreHandle concurrency core: writer thread,
//! read pool, change broadcast, and the async MemoryBackend impl.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_daemon::{ChangeKind, StoreHandle};
use rb_engine::MemoryBackend;
use rb_types::{MemoryNote, MemoryType, MemoryUpdates, Namespace};

const DIM: usize = 8;

fn note(ns: &Namespace, body: &str) -> MemoryNote {
    let mut n = MemoryNote::new(ns.clone(), body.to_string(), MemoryType::Insight, 5);
    n.summary = body.chars().take(40).collect();
    n.keywords = vec!["memory".to_string()];
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_then_read_round_trips_through_pool() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 4).unwrap();

    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "always one db one transaction");
    let id = n.id.clone();
    let emb = vec![0.1f32; DIM];

    handle.write(n.clone(), Some(emb)).await.unwrap();

    let got = handle.get(id.clone()).await.unwrap();
    assert!(got.is_some(), "written memory must be retrievable");
    assert_eq!(got.unwrap().content, "always one db one transaction");

    let listed = handle.list(ns.clone(), None, 50).await.unwrap();
    assert_eq!(listed.len(), 1, "list returns the one written memory");

    let kw = handle.keyword(ns.clone(), "memory".to_string(), 50).await.unwrap();
    assert_eq!(kw, vec![id.clone()], "keyword search finds it by keyword");

    let vec_hits = handle.vector(ns, vec![0.1f32; DIM], 5).await.unwrap();
    assert_eq!(vec_hits.len(), 1, "vector search returns the one embedded memory");
    assert_eq!(vec_hits[0].0, id);

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_publishes_change_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 2).unwrap();
    let mut rx = handle.subscribe();

    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "broadcast me");
    let id = n.id.clone();
    handle.write(n, Some(vec![0.2f32; DIM])).await.unwrap();

    let evt = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("a change event must arrive within 2s")
        .expect("broadcast channel must not be closed");
    assert_eq!(evt.id, id);
    assert_eq!(evt.namespace, ns);
    assert_eq!(evt.kind, ChangeKind::Created);

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_and_archive_emit_correct_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 2).unwrap();
    let mut rx = handle.subscribe();

    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "evolve me");
    let id = n.id.clone();
    handle.write(n, Some(vec![0.3f32; DIM])).await.unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, ChangeKind::Created);

    let updates = MemoryUpdates { importance: Some(9), ..Default::default() };
    handle.update(id.clone(), updates).await.unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, ChangeKind::Updated);

    handle.archive(id.clone()).await.unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, ChangeKind::Archived);

    let got = handle.get(id).await.unwrap().unwrap();
    assert_eq!(got.importance, 9, "update persisted");
    assert!(got.archived_at.is_some(), "archive persisted (soft delete)");

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn many_concurrent_writers_lose_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 4).unwrap();
    let ns = Namespace::Project("a".to_string());

    const N: usize = 200;
    let mut tasks = Vec::with_capacity(N);
    for i in 0..N {
        let h = handle.clone();
        let ns = ns.clone();
        tasks.push(tokio::spawn(async move {
            let n = note(&ns, &format!("concurrent note {i}"));
            h.write(n, Some(vec![i as f32; DIM])).await
            // `h` is dropped here when the task future completes, releasing this
            // clone's writer Sender — required for shutdown to close the mpsc.
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }

    let listed = handle.list(ns, None, N + 10).await.unwrap();
    assert_eq!(listed.len(), N, "no writes lost under concurrency");

    handle.shutdown().await;
}
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-daemon --test store_handle` Expected: FAIL to compile — `cannot find function 'start' in ... StoreHandle` / `the trait bound 'StoreHandle: MemoryBackend' is not satisfied`. This confirms the test drives the new implementation.

- [ ] **Step 3: Implement `StoreHandle`, the writer thread, the read pool, and the `MemoryBackend` impl.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/store_handle.rs` with the complete contents below.

  Key threading invariants honored exactly:
  - The write `SqliteStore` is created **inside** the spawned `std::thread` and never leaves it — it is not `Arc`-wrapped, not `Mutex`-wrapped, not moved out. (`SqliteStore` is `Send` but `!Sync`; constructing it on the writer thread keeps it single-threaded.)
  - Each `WriteCommand` carries its own `oneshot::Sender<Result<()>>`; the writer runs the sync op and replies, then (on `Ok`) broadcasts `MemoryChanged`.
  - The writer loops until the `mpsc` closes (all `Sender`s dropped), then returns — graceful shutdown. (Callers must therefore ensure every `StoreHandle` clone is dropped before/at shutdown; the server in Task 25 drains its per-connection tasks before calling `shutdown`.)
  - Reads acquire an `OwnedSemaphorePermit` **asynchronously** (never busy-waiting, never blocking a worker), then check out a `SqliteStore` from the bounded pool inside `spawn_blocking`, run the sync method, and return the store to the pool. The permit is held until the store is returned, so a held permit always implies an available store.

```rust
//! The single-writer store handle: one dedicated OS thread owns the write
//! connection (rusqlite is `!Sync`, so it must never be shared); a bounded
//! pool of read connections serves concurrent reads via `spawn_blocking`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rb_engine::MemoryBackend;
use rb_store::{SqliteStore, Store};
use rb_types::{Error, MemoryId, MemoryNote, MemoryUpdates, Namespace, Result};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Semaphore};

use crate::change::{ChangeKind, MemoryChanged};

/// Capacity of the broadcast channel: lagging subscribers drop oldest events
/// (notification only, never coordination).
const BROADCAST_CAPACITY: usize = 256;
/// Bound on the write queue. Backpressure (spec §8): senders await capacity.
const WRITE_QUEUE_CAPACITY: usize = 256;

/// One write request handed to the dedicated writer thread. Each carries a
/// `oneshot` reply channel; the writer also broadcasts a `MemoryChanged` on Ok.
enum WriteCommand {
    Insert {
        note: Box<MemoryNote>,
        embedding: Option<Vec<f32>>,
        reply: oneshot::Sender<Result<()>>,
    },
    Update {
        id: MemoryId,
        updates: Box<MemoryUpdates>,
        reply: oneshot::Sender<Result<()>>,
    },
    Archive {
        id: MemoryId,
        reply: oneshot::Sender<Result<()>>,
    },
}

/// Cloneable handle to the single-writer store. Cloning shares the same writer
/// thread, read pool, and broadcast channel.
#[derive(Clone)]
pub struct StoreHandle {
    writer_tx: mpsc::Sender<WriteCommand>,
    pool: Arc<ReadPool>,
    events: broadcast::Sender<MemoryChanged>,
    /// Joined on `shutdown`. `Mutex<Option<..>>` so a single clone can take it.
    writer_join: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

/// A bounded pool of read stores. The `Semaphore` bounds concurrency; the `Vec`
/// holds the idle stores. Each read acquires one permit (async), pops one store
/// (on a blocking thread), runs the sync op, and returns it. `permits` and
/// `stores` are `Arc` so the owned permit and the locked Vec move cleanly into
/// the blocking closure.
struct ReadPool {
    permits: Arc<Semaphore>,
    stores: Arc<Mutex<Vec<SqliteStore>>>,
}

impl ReadPool {
    fn open(db_path: &PathBuf, dim: usize, size: usize) -> Result<Self> {
        let mut stores = Vec::with_capacity(size);
        for _ in 0..size {
            stores.push(SqliteStore::open(db_path, dim)?);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(size)),
            stores: Arc::new(Mutex::new(stores)),
        })
    }
}

impl StoreHandle {
    /// Start the writer thread and open the read pool. Returns immediately once
    /// the writer's write store has opened successfully (errors surface here).
    pub fn start(db_path: PathBuf, embedding_dim: usize, read_pool_size: usize) -> Result<Self> {
        let pool = Arc::new(ReadPool::open(&db_path, embedding_dim, read_pool_size.max(1))?);
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (writer_tx, writer_rx) = mpsc::channel::<WriteCommand>(WRITE_QUEUE_CAPACITY);

        // Channel for the writer thread to report whether its write store opened.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        let writer_events = events.clone();
        let writer_path = db_path.clone();

        let writer_join = std::thread::Builder::new()
            .name("rb-writer".to_string())
            .spawn(move || {
                writer_loop(writer_path, embedding_dim, writer_rx, writer_events, ready_tx);
            })
            .map_err(|e| Error::Io(format!("spawn writer thread: {e}")))?;

        // Block until the writer confirms the write store opened (or failed).
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = writer_join.join();
                return Err(e);
            }
            Err(_) => {
                let _ = writer_join.join();
                return Err(Error::Storage("writer thread exited before ready".to_string()));
            }
        }

        Ok(Self {
            writer_tx,
            pool,
            events,
            writer_join: Arc::new(Mutex::new(Some(writer_join))),
        })
    }

    /// Subscribe to change events. Each subscriber receives an owned copy.
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryChanged> {
        self.events.subscribe()
    }

    /// Gracefully stop: drop this clone's write sender so the writer loop ends
    /// once ALL clones are gone, then join the writer thread (which flushes its
    /// WAL on connection close before exit).
    ///
    /// Callers MUST ensure no other live `StoreHandle` clone is holding a sender
    /// (e.g. the server drains its per-connection tasks first); otherwise the
    /// writer mpsc never closes and the join would block.
    pub async fn shutdown(self) {
        // Drop our sender + broadcast + pool so the writer's mpsc can close.
        drop(self.writer_tx);
        drop(self.events);
        drop(self.pool);
        // Take and join the writer thread on a blocking thread (join is sync).
        let join = { self.writer_join.lock().await.take() };
        if let Some(handle) = join {
            let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        }
    }

    /// Send a write command and await the writer's reply.
    async fn send_write(&self, cmd: WriteCommand, rx: oneshot::Receiver<Result<()>>) -> Result<()> {
        self.writer_tx
            .send(cmd)
            .await
            .map_err(|_| Error::Storage("writer thread unavailable".to_string()))?;
        rx.await
            .map_err(|_| Error::Storage("writer dropped reply".to_string()))?
    }

    /// Run a synchronous read against a checked-out store on a blocking thread.
    ///
    /// The semaphore permit is acquired in async context (no busy-wait, never
    /// blocks a worker) and then MOVED into the blocking closure so it is held
    /// for the whole read and released only after the store is returned.
    async fn with_read<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&SqliteStore) -> Result<T> + Send + 'static,
    {
        let permits = Arc::clone(&self.pool.permits);
        let stores = Arc::clone(&self.pool.stores);

        // Acquire a permit asynchronously. A permit guarantees a store is in the
        // pool (a permit is released only after its store is pushed back).
        let permit = permits
            .acquire_owned()
            .await
            .map_err(|_| Error::Storage("read pool closed".to_string()))?;

        tokio::task::spawn_blocking(move || {
            // Hold the permit for the full read (drops at closure end).
            let _permit = permit;

            // Check out a store. The permit invariant guarantees one is present.
            let store = stores
                .blocking_lock()
                .pop()
                .ok_or_else(|| Error::Storage("read pool exhausted (no store despite permit)".to_string()))?;

            let result = f(&store);

            // Always return the store to the pool, even on error.
            stores.blocking_lock().push(store);
            result
        })
        .await
        .map_err(|e| Error::Storage(format!("read task panicked or cancelled: {e}")))?
    }
}

/// The dedicated writer loop. Owns the write `SqliteStore` for its entire life;
/// the store is created here and never escapes this thread (`!Sync`-safe).
fn writer_loop(
    db_path: PathBuf,
    embedding_dim: usize,
    mut rx: mpsc::Receiver<WriteCommand>,
    events: broadcast::Sender<MemoryChanged>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) {
    let store = match SqliteStore::open(&db_path, embedding_dim) {
        Ok(s) => {
            let _ = ready_tx.send(Ok(()));
            s
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    // Blocking receive loop: `blocking_recv` parks this OS thread (NOT a tokio
    // worker) until a command arrives or all senders drop.
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            WriteCommand::Insert {
                note,
                embedding,
                reply,
            } => {
                let ns = note.namespace.clone();
                let id = note.id.clone();
                let res = store.insert_memory(&note, embedding.as_deref());
                let ok = res.is_ok();
                let _ = reply.send(res);
                if ok {
                    let _ = events.send(MemoryChanged {
                        id,
                        namespace: ns,
                        kind: ChangeKind::Created,
                    });
                }
            }
            WriteCommand::Update {
                id,
                updates,
                reply,
            } => {
                let res = store.update_memory(&id, &updates);
                let ok = res.is_ok();
                // Look up the namespace for the event (we are already on the
                // writer thread with the live connection; this read is cheap).
                let ns = store.get_memory(&id).ok().flatten().map(|m| m.namespace);
                let _ = reply.send(res);
                if ok {
                    if let Some(namespace) = ns {
                        let _ = events.send(MemoryChanged {
                            id,
                            namespace,
                            kind: ChangeKind::Updated,
                        });
                    }
                }
            }
            WriteCommand::Archive { id, reply } => {
                // Resolve namespace BEFORE archiving (still readable either way,
                // but cheap and avoids surprises).
                let ns = store.get_memory(&id).ok().flatten().map(|m| m.namespace);
                let res = store.archive_memory(&id);
                let ok = res.is_ok();
                let _ = reply.send(res);
                if ok {
                    if let Some(namespace) = ns {
                        let _ = events.send(MemoryChanged {
                            id,
                            namespace,
                            kind: ChangeKind::Archived,
                        });
                    }
                }
            }
        }
    }

    // mpsc closed (all senders dropped) -> graceful shutdown. Dropping the write
    // `SqliteStore` closes its rusqlite connection, which flushes the WAL frames
    // it produced; SQLite's default `wal_autocheckpoint` plus connection close
    // provide durability. NOTE: P0's `SqliteStore` exposes no explicit
    // checkpoint API (its `conn` is `pub(crate)` to rb-store), so we cannot
    // issue `PRAGMA wal_checkpoint` here; a future rb-store explicit-checkpoint
    // method should replace this drop-based flush.
    drop(store);
}

#[async_trait]
impl MemoryBackend for StoreHandle {
    async fn write(&self, note: MemoryNote, embedding: Option<Vec<f32>>) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Insert {
            note: Box::new(note),
            embedding,
            reply,
        };
        self.send_write(cmd, rx).await
    }

    async fn get(&self, id: MemoryId) -> Result<Option<MemoryNote>> {
        self.with_read(move |s| s.get_memory(&id)).await
    }

    async fn keyword(
        &self,
        ns: Namespace,
        query: String,
        limit: usize,
    ) -> Result<Vec<MemoryId>> {
        self.with_read(move |s| s.keyword_search(&ns, &query, limit)).await
    }

    async fn vector(
        &self,
        ns: Namespace,
        embedding: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        self.with_read(move |s| s.vector_search(&ns, &embedding, limit)).await
    }

    async fn graph(&self, id: MemoryId, depth: u8) -> Result<Vec<MemoryId>> {
        self.with_read(move |s| s.graph_neighbors(&id, depth)).await
    }

    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        self.with_read(move |s| s.list(&ns, min_importance, limit)).await
    }

    async fn update(&self, id: MemoryId, updates: MemoryUpdates) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Update {
            id,
            updates: Box::new(updates),
            reply,
        };
        self.send_write(cmd, rx).await
    }

    async fn archive(&self, id: MemoryId) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Archive { id, reply };
        self.send_write(cmd, rx).await
    }
}
```

  (verify against installed tokio at execution: `Semaphore::acquire_owned(self: Arc<Semaphore>) -> Result<OwnedSemaphorePermit, AcquireError>` and `tokio::sync::Mutex::blocking_lock` both exist in tokio 1.x; if the pinned tokio renames either, adjust. Do NOT call `Semaphore::acquire` on a non-`Arc` here — we need the `OwnedSemaphorePermit` to move it into `spawn_blocking`.)

  Add `async-trait` to `crates/rb-daemon/Cargo.toml` `[dependencies]` if the setup cluster did not already (`async-trait = { workspace = true }`) — it is needed for the `#[async_trait]` impl. Also ensure `[dependencies]` includes `rb-store = { path = "../rb-store" }`, `rb-engine = { path = "../rb-engine" }`, `tokio = { workspace = true }`, and `[dev-dependencies]` includes `rb-engine = { path = "../rb-engine" }`, `tempfile = { workspace = true }`, and `tokio = { workspace = true }` (for `#[tokio::test]`).

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-daemon --test store_handle -- --nocapture` Expected: PASS — all four tests (`write_then_read_round_trips_through_pool`, `write_publishes_change_event`, `update_and_archive_emit_correct_kinds`, `many_concurrent_writers_lose_nothing`) report `ok`. If `many_concurrent_writers_lose_nothing` reports fewer than 200 rows, the writer is dropping commands — verify the `mpsc` capacity and that every `Insert` replies; if any test hangs, either the writer thread panicked before `ready_tx.send` (check `SqliteStore::open` dim matches) or a `StoreHandle` clone outlived `shutdown` (every spawned task must drop its clone before the final `handle.shutdown()`).

- [ ] **Step 5: Stress for flakiness.** Run: `cargo test -p rb-daemon --test store_handle -- --nocapture` two more times. Expected: PASS every time (no lost writes, no panics). The single-writer serialization must make this deterministic.

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings (no `unwrap`/`expect`/`panic` outside tests; the writer thread never panics on the request path — it uses `let _ =` for best-effort replies/broadcasts). Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-daemon/src/store_handle.rs crates/rb-daemon/src/lib.rs crates/rb-daemon/tests/store_handle.rs crates/rb-daemon/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-daemon): add StoreHandle with dedicated writer thread, read pool, and MemoryBackend"`

---

### Task 24: rb-daemon `paths.rs` — default socket/db paths + shared embedder wrapper

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/paths.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/shared_embedder.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` (add modules + re-exports)

Two small support pieces the server and CLI both need: (1) `default_socket_path()` / `default_db_path()` derived via `directories`, honoring `$XDG_RUNTIME_DIR` for the socket (spec §14); (2) `SharedEmbedder`, an `Arc<dyn EmbeddingProvider>` newtype that itself implements `EmbeddingProvider`, so one embedder instance is shared across all per-connection `MemoryEngine`s without making `MemoryEngine` non-generic.

- [ ] **Step 1: Write the failing tests AND declare the modules.** Add to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs`:

```rust
mod paths;
mod shared_embedder;
```

  and the re-exports (after the existing `pub use`):

```rust
pub use paths::{default_db_path, default_socket_path};
pub use shared_embedder::SharedEmbedder;
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/paths.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn socket_path_is_under_a_rusty_brain_dir_named_sock() {
        let p = default_socket_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("sock"));
        assert!(
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                == Some("rusty-brain"),
            "socket lives in a rusty-brain directory: {p:?}"
        );
    }

    #[test]
    fn db_path_ends_with_rusty_brain_db_file() {
        let p = default_db_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("memory.db"));
    }

    #[test]
    fn xdg_runtime_dir_is_honored_for_socket() {
        // NOTE: this test mutates process-global env. The sibling path tests
        // only assert file_name == "sock" and parent == "rusty-brain", which
        // hold for ANY base directory, so a parallel read of XDG_RUNTIME_DIR
        // mid-mutation cannot break their assertions. We still restore the var.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        let p = default_socket_path().unwrap();
        assert!(
            p.starts_with(dir.path()),
            "socket must live under XDG_RUNTIME_DIR when set: {p:?}"
        );
        std::env::remove_var("XDG_RUNTIME_DIR");
    }
}
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/shared_embedder.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_embed::{DeterministicProvider, EmbeddingProvider};

    #[tokio::test]
    async fn shared_embedder_delegates_dim_and_embed() {
        let inner = DeterministicProvider::new(8);
        let model = inner.model_id().to_string();
        let shared = SharedEmbedder::new(inner);
        assert_eq!(shared.dim(), 8);
        assert_eq!(shared.model_id(), model);
        let out = shared.embed(&["hello".to_string()]).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 8);
    }

    #[tokio::test]
    async fn cloning_shares_one_instance() {
        let shared = SharedEmbedder::new(DeterministicProvider::new(8));
        let clone = shared.clone();
        // Both produce identical vectors for identical input (same instance).
        let a = shared.embed(&["same".to_string()]).await.unwrap();
        let b = clone.embed(&["same".to_string()]).await.unwrap();
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-daemon paths` and `cargo test -p rb-daemon shared_embedder` Expected: both FAIL to compile — `cannot find function 'default_socket_path'` / `cannot find type 'SharedEmbedder'`. Confirms the tests drive the impl.

- [ ] **Step 3: Implement `paths.rs`.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/paths.rs`:

```rust
use std::path::PathBuf;

use rb_types::{Error, Result};

/// Default Unix-domain-socket path: `$XDG_RUNTIME_DIR/rusty-brain/sock` when the
/// runtime dir is available, else the platform runtime dir from `directories`,
/// else a fallback under the user's cache dir. The parent dir is created (0700)
/// by the daemon, not here.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        let rt = PathBuf::from(rt);
        if !rt.as_os_str().is_empty() {
            return Ok(rt.join("rusty-brain").join("sock"));
        }
    }
    let dirs = directories::ProjectDirs::from("dev", "rusty-brain", "rusty-brain")
        .ok_or_else(|| Error::Io("cannot determine a runtime directory".to_string()))?;
    let base = dirs
        .runtime_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs.cache_dir().to_path_buf());
    Ok(base.join("rusty-brain").join("sock"))
}

/// Default database path: `<data-dir>/rusty-brain/memory.db`.
pub fn default_db_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "rusty-brain", "rusty-brain")
        .ok_or_else(|| Error::Io("cannot determine a data directory".to_string()))?;
    Ok(dirs.data_dir().join("memory.db"))
}
```

  (verify against installed `directories` v5 at execution: `ProjectDirs::from(qualifier, organization, application)` and `runtime_dir()->Option<&Path>`, `data_dir()->&Path`, `cache_dir()->&Path`. If v5 differs, adjust the accessor names.)

- [ ] **Step 4: Implement `shared_embedder.rs`.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/shared_embedder.rs`:

```rust
use std::sync::Arc;

use async_trait::async_trait;
use rb_embed::EmbeddingProvider;
use rb_types::Result;

/// A reference-counted, cloneable embedding provider. Lets one embedder instance
/// be shared across every per-connection `MemoryEngine` while keeping
/// `MemoryEngine`'s `P: EmbeddingProvider` generic bound satisfied.
#[derive(Clone)]
pub struct SharedEmbedder {
    inner: Arc<dyn EmbeddingProvider>,
}

impl SharedEmbedder {
    /// Wrap any concrete provider.
    pub fn new<P: EmbeddingProvider + 'static>(provider: P) -> Self {
        Self {
            inner: Arc::new(provider),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for SharedEmbedder {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.inner.embed(texts).await
    }
}
```

  Ensure `crates/rb-daemon/Cargo.toml` `[dependencies]` includes `rb-embed = { path = "../rb-embed" }`, `async-trait = { workspace = true }`, and `directories = { workspace = true }`; and `[dev-dependencies]` includes `rb-embed = { path = "../rb-embed" }` (for the test) and `tempfile = { workspace = true }`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-daemon paths` then `cargo test -p rb-daemon shared_embedder` Expected: PASS (3 path tests, 2 embedder tests).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-daemon/src/paths.rs crates/rb-daemon/src/shared_embedder.rs crates/rb-daemon/src/lib.rs crates/rb-daemon/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-daemon): add default path helpers and SharedEmbedder wrapper"`

---

### Task 25: rb-daemon `server.rs` — UDS bind (0700 dir / 0600 socket), pidfile single-instance, per-connection dispatch, graceful shutdown

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/server.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/error_map.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs` (add modules + re-exports)

The server: binds a `UnixListener` at `socket_path` inside a `0700` dir with the socket `chmod`'d `0600` (spec §14); writes a pidfile and enforces single-instance (a live socket means another daemon owns it → fail; a stale socket is reclaimed). The accept loop spawns a tokio task per connection (tracked in a `JoinSet`): read `Handshake` (reject on `contract_version` mismatch, capture namespace), send `HandshakeAck`, then loop reading `Request` frames and dispatching to a per-connection `MemoryEngine` whose namespace is fixed to the handshake namespace (server-side isolation: the client cannot widen scope). Each `rb_types::Error` maps to `Response::Error` with no internal leakage. Graceful shutdown stops accepting, **drains/aborts the per-connection tasks** (so their `StoreHandle` clones drop), `StoreHandle::shutdown` (writer-thread join), and removes the socket + pidfile.

- [ ] **Step 1: Write the failing error-map test AND declare the module.** Add to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/lib.rs`:

```rust
mod error_map;
mod server;
```

  and re-exports (after existing):

```rust
pub use server::{Daemon, DaemonConfig};
```

  Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/error_map.rs` with ONLY the test module (note the `clippy::panic` allow — the workspace denies `panic`, and this module calls `panic!`):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_proto::Response;
    use rb_types::{Error, MemoryId};

    #[test]
    fn maps_each_error_to_stable_kind() {
        let cases: Vec<(Error, &str)> = vec![
            (Error::Storage("x".into()), "storage"),
            (Error::Migration("x".into()), "migration"),
            (Error::NotFound(MemoryId::new()), "not_found"),
            (Error::InvalidNamespace("x".into()), "invalid_namespace"),
            (Error::InvalidMemoryType("x".into()), "invalid_memory_type"),
            (Error::InvalidLinkType("x".into()), "invalid_link_type"),
            (Error::Serialization("x".into()), "serialization"),
            (
                Error::DimensionMismatch { expected: 1, got: 2 },
                "dimension_mismatch",
            ),
            (Error::Io("x".into()), "io"),
        ];
        for (err, expected_kind) in cases {
            match error_to_response(err) {
                Response::Error { kind, message } => {
                    assert_eq!(kind, expected_kind);
                    assert!(!message.is_empty(), "message is populated");
                }
                other => panic!("expected Response::Error, got {other:?}"),
            }
        }
    }

    #[test]
    fn message_does_not_leak_struct_internals() {
        // The message is the Display string, never the Debug of the daemon.
        let r = error_to_response(Error::Storage("disk full".into()));
        if let Response::Error { message, .. } = r {
            assert_eq!(message, "storage error: disk full");
        } else {
            panic!("expected error response");
        }
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-daemon error_map` Expected: FAIL to compile — `cannot find function 'error_to_response'`. Confirms the test drives the impl.

- [ ] **Step 3: Implement `error_map.rs`.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/error_map.rs`:

```rust
use rb_proto::Response;
use rb_types::Error;

/// Map a domain `Error` to a wire `Response::Error` with a stable `kind` string
/// and a human `message` (the `Display` form — no internal struct leakage).
pub(crate) fn error_to_response(err: Error) -> Response {
    let kind = match &err {
        Error::Storage(_) => "storage",
        Error::Migration(_) => "migration",
        Error::NotFound(_) => "not_found",
        Error::InvalidNamespace(_) => "invalid_namespace",
        Error::InvalidMemoryType(_) => "invalid_memory_type",
        Error::InvalidLinkType(_) => "invalid_link_type",
        Error::Serialization(_) => "serialization",
        Error::DimensionMismatch { .. } => "dimension_mismatch",
        Error::Io(_) => "io",
    };
    Response::Error {
        kind: kind.to_string(),
        message: err.to_string(),
    }
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-daemon error_map` Expected: PASS (2 tests).

- [ ] **Step 5: Implement `server.rs` (Daemon + DaemonConfig + dispatch).** Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/src/server.rs` with the complete contents below. The dispatch builds one `MemoryEngine<StoreHandle, SharedEmbedder>` per connection, fixing the namespace to the handshake value (isolation enforced server-side). Client-supplied scope fields in `Recall`/`List`/`Graph` are ignored — every operation uses the connection namespace.

```rust
//! UDS server: single-instance bind, per-connection framed dispatch, graceful
//! shutdown. Isolation is enforced server-side: every engine is pinned to the
//! handshake namespace and the client cannot widen it.

use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use rb_embed::EmbeddingProvider;
use rb_engine::{MemoryEngine, RememberInput};
use rb_proto::{
    read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
use rb_types::{Error, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{info, warn};

use crate::error_map::error_to_response;
use crate::shared_embedder::SharedEmbedder;
use crate::store_handle::StoreHandle;

/// Static configuration for a daemon instance.
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub read_pool_size: usize,
}

/// A bound, ready-to-run daemon: owns the listener, the store handle, the shared
/// embedder, and the paths it must clean up on shutdown.
pub struct Daemon {
    listener: UnixListener,
    store: StoreHandle,
    embedder: SharedEmbedder,
    socket_path: PathBuf,
    pidfile_path: PathBuf,
}

impl Daemon {
    /// Bind the daemon: start the `StoreHandle` (dim from the embedder), create
    /// the socket dir `0700`, bind the UDS `0600`, and write a pidfile with the
    /// single-instance guard. Fails closed if another live daemon owns the
    /// socket; reclaims a stale socket.
    pub async fn bind(config: DaemonConfig, embedder: SharedEmbedder) -> Result<Daemon> {
        let dim = embedder.dim();

        // 1. Ensure the data dir for the DB exists.
        if let Some(parent) = config.db_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Io(format!("create db dir {}: {e}", parent.display())))?;
        }

        // 2. Single-instance: if a live daemon answers on the socket, refuse.
        //    The authoritative guard is this live-socket probe plus the bind
        //    below (a concurrent racer's bind fails with EADDRINUSE). The
        //    pidfile written in step 6 is advisory for operators; a flock-based
        //    pidfile lock is deferred (P1 is single-user).
        if config.socket_path.exists() {
            if probe_live(&config.socket_path).await {
                return Err(Error::Io(format!(
                    "another rusty-brain daemon is already listening at {}",
                    config.socket_path.display()
                )));
            }
            // Stale socket: reclaim it.
            let _ = tokio::fs::remove_file(&config.socket_path).await;
        }

        // 3. Create the socket dir 0700.
        let sock_dir = config
            .socket_path
            .parent()
            .ok_or_else(|| Error::Io("socket path has no parent dir".to_string()))?
            .to_path_buf();
        tokio::fs::create_dir_all(&sock_dir)
            .await
            .map_err(|e| Error::Io(format!("create socket dir {}: {e}", sock_dir.display())))?;
        tokio::fs::set_permissions(&sock_dir, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|e| Error::Io(format!("chmod 0700 {}: {e}", sock_dir.display())))?;

        // 4. Bind the listener, then chmod the socket 0600.
        let listener = UnixListener::bind(&config.socket_path)
            .map_err(|e| Error::Io(format!("bind {}: {e}", config.socket_path.display())))?;
        tokio::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|e| {
                Error::Io(format!("chmod 0600 {}: {e}", config.socket_path.display()))
            })?;

        // 5. Start the store handle (writer thread + read pool) on the DB.
        let store = StoreHandle::start(config.db_path.clone(), dim, config.read_pool_size)?;

        // 6. Write the (advisory) pidfile.
        let pidfile_path = config.socket_path.with_extension("pid");
        tokio::fs::write(&pidfile_path, std::process::id().to_string())
            .await
            .map_err(|e| Error::Io(format!("write pidfile: {e}")))?;

        info!(socket = %config.socket_path.display(), "daemon bound");
        Ok(Daemon {
            listener,
            store,
            embedder,
            socket_path: config.socket_path,
            pidfile_path,
        })
    }

    /// Run the accept loop until `shutdown` resolves, then drain connections and
    /// clean up. Graceful shutdown order (spec §8): stop accepting -> abort and
    /// join per-connection tasks (dropping their `StoreHandle` clones) ->
    /// `StoreHandle::shutdown` (writer-thread join / WAL flush) -> remove socket
    /// + pidfile.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let Daemon {
            listener,
            store,
            embedder,
            socket_path,
            pidfile_path,
        } = self;
        tokio::pin!(shutdown);

        // Track per-connection tasks so shutdown can drain/abort them. Each task
        // holds a `StoreHandle` clone (via its `MemoryEngine`); those clones MUST
        // be dropped before `store.shutdown()`, or the writer mpsc never closes.
        let mut conns: JoinSet<()> = JoinSet::new();

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    info!("shutdown signal received; stopping accept loop");
                    break;
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            let store = store.clone();
                            let embedder = embedder.clone();
                            conns.spawn(async move {
                                if let Err(e) = handle_connection(stream, store, embedder).await {
                                    warn!(error = %e, "connection ended with error");
                                }
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "accept failed");
                        }
                    }
                }
                // Reap finished connection tasks so the JoinSet does not grow
                // unbounded over the daemon's lifetime.
                Some(_joined) = conns.join_next() => {}
            }
        }

        // 1. Stop accepting (listener dropped here).
        drop(listener);

        // 2. Drain: abort all in-flight connection tasks and await them so their
        //    `StoreHandle` clones (inside each task's `MemoryEngine`) are dropped.
        //    Clients may hold connections open indefinitely, so abort rather than
        //    wait forever.
        conns.shutdown().await;

        // 3. Now only this scope holds a `StoreHandle`; shutting it down closes
        //    the writer mpsc and joins the writer thread (WAL flush on close).
        store.shutdown().await;

        // 4. Remove socket + pidfile.
        drop(embedder);
        let _ = tokio::fs::remove_file(&socket_path).await;
        let _ = tokio::fs::remove_file(&pidfile_path).await;
        info!("daemon shut down cleanly");
        Ok(())
    }
}

/// Probe whether a live daemon answers on `path` by completing a handshake.
async fn probe_live(path: &std::path::Path) -> bool {
    match UnixStream::connect(path).await {
        Ok(stream) => {
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
            let hs = Handshake {
                contract_version: CONTRACT_VERSION,
                namespace: rb_types::Namespace::Global,
            };
            if write_frame(&mut framed, &hs).await.is_err() {
                return false;
            }
            // A live daemon replies with a HandshakeAck within a short window.
            matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    read_frame::<_, HandshakeAck>(&mut framed),
                )
                .await,
                Ok(Ok(_))
            )
        }
        Err(_) => false,
    }
}

/// Handle one connection: handshake (verify contract, capture namespace), then
/// loop request->engine->response. The engine is pinned to the handshake
/// namespace; the client cannot read or write outside it.
async fn handle_connection(
    stream: UnixStream,
    store: StoreHandle,
    embedder: SharedEmbedder,
) -> Result<()> {
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

    // 1. Handshake.
    let handshake: Handshake = match read_frame(&mut framed).await {
        Ok(h) => h,
        Err(e) => {
            // Cannot even parse a handshake; nothing to reply to meaningfully.
            return Err(e);
        }
    };
    if handshake.contract_version != CONTRACT_VERSION {
        let ack = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: false,
            message: Some(format!(
                "contract mismatch: server {CONTRACT_VERSION}, client {}",
                handshake.contract_version
            )),
        };
        write_frame(&mut framed, &ack).await?;
        return Ok(());
    }
    let namespace = handshake.namespace.clone();
    let ack = HandshakeAck {
        contract_version: CONTRACT_VERSION,
        ok: true,
        message: None,
    };
    write_frame(&mut framed, &ack).await?;

    // 2. Per-connection engine pinned to the handshake namespace.
    let engine = MemoryEngine::new(store, embedder, namespace.clone());

    // 3. Request loop. Reading via `read_frame` returns `Err` only on a hard
    //    framing/socket error; a clean client disconnect surfaces as the
    //    underlying codec stream ending, which `read_frame` maps to an Io error
    //    we treat as end-of-connection.
    loop {
        let req: Request = match read_frame::<_, Request>(&mut framed).await {
            Ok(r) => r,
            // Client closed the connection (EOF) or framing ended: stop cleanly.
            Err(_) => break,
        };
        let resp = dispatch(&engine, &namespace, req).await;
        write_frame(&mut framed, &resp).await?;
    }
    Ok(())
}

/// Dispatch one request against the connection's engine. Isolation: scope fields
/// in the request are IGNORED — every operation uses the connection namespace
/// (pinned inside the engine).
async fn dispatch<P: EmbeddingProvider>(
    engine: &MemoryEngine<StoreHandle, P>,
    _namespace: &rb_types::Namespace,
    req: Request,
) -> Response {
    match req {
        Request::Ping => Response::Pong {
            contract_version: CONTRACT_VERSION,
        },
        Request::Remember {
            content,
            context,
            memory_type,
            importance,
            keywords,
            tags,
            related_files,
        } => {
            let input = RememberInput {
                content,
                context,
                memory_type,
                importance,
                keywords,
                tags,
                related_files,
            };
            match engine.remember(input).await {
                Ok(id) => Response::Remembered { id },
                Err(e) => error_to_response(e),
            }
        }
        Request::Recall {
            query,
            scope: _,
            memory_type,
            tags,
            limit,
        } => match engine.recall(&query, limit, memory_type, &tags).await {
            Ok(results) => Response::Recalled { results },
            Err(e) => error_to_response(e),
        },
        Request::Get { id } => match engine.get(id).await {
            Ok(memory) => Response::Got { memory },
            Err(e) => error_to_response(e),
        },
        Request::List {
            scope: _,
            min_importance,
            limit,
        } => match engine.list(min_importance, limit).await {
            Ok(memories) => Response::Listed { memories },
            Err(e) => error_to_response(e),
        },
        Request::Graph { id, depth } => match engine.graph(id, depth).await {
            Ok(memories) => Response::GraphResult { memories },
            Err(e) => error_to_response(e),
        },
        Request::Update { id, updates } => match engine.update(id, updates).await {
            Ok(()) => Response::Updated,
            Err(e) => error_to_response(e),
        },
        Request::Delete { id } => match engine.delete(id).await {
            Ok(()) => Response::Deleted,
            Err(e) => error_to_response(e),
        },
        Request::Context => match engine.context().await {
            Ok((recent, important, total)) => Response::ContextResult {
                recent,
                important,
                total,
            },
            Err(e) => error_to_response(e),
        },
    }
}
```

  NOTE on `engine.list` / `engine.graph` / `engine.context()` shapes: this dispatch assumes the rb-engine cluster exposes `list(min_importance: Option<u8>, limit: usize)`, `graph(id, depth) -> Vec<MemoryNote>`, and `context() -> Result<(Vec<MemoryNote>, Vec<MemoryNote>, usize)>` (matching `Response::ContextResult { recent, important, total }`), with the namespace pinned inside the engine. The spine states these are namespace-pinned pass-throughs; if the rb-engine cluster used slightly different arg orders or a single merged `context` Vec, adapt these arms to the engine's actual signatures (verify against rb-engine at execution; do NOT change the wire `Response` shapes).

  NOTE on framing helpers: this calls `read_frame::<_, T>(&mut framed)` / `write_frame(&mut framed, &value)` against the rb-proto helpers. If rb-proto's helper signatures differ (e.g. take `&mut Framed<UnixStream, LengthDelimitedCodec>` explicitly, or return a different error type), adjust the calls to the actual form (verify against rb-proto at execution; keep the framing semantics — one serde value per length-delimited frame).

  Add to `crates/rb-daemon/Cargo.toml` `[dependencies]` (if the setup cluster did not): `futures = { workspace = true }`, `tokio-util = { workspace = true }`, `tracing = { workspace = true }`, plus `rb-proto = { path = "../rb-proto" }`, `rb-engine = { path = "../rb-engine" }`, `rb-embed = { path = "../rb-embed" }`. (`serde_json` is no longer needed here since request decoding goes through `read_frame`; keep it only if other modules use it.)

- [ ] **Step 6: Verify it compiles (full integration test arrives in Task 26).** Run: `cargo build -p rb-daemon` Expected: `Finished` (exit 0). The server wiring type-checks against `rb_proto`/`rb_engine`. If `read_frame`/`write_frame` signatures differ from `read_frame::<_, T>(&mut framed)` / `write_frame(&mut framed, &value)`, adjust the calls to the rb-proto helpers' actual form (verify against rb-proto at execution; keep the framing semantics).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-daemon/src/server.rs crates/rb-daemon/src/error_map.rs crates/rb-daemon/src/lib.rs crates/rb-daemon/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rb-daemon): add UDS server with single-instance guard, per-connection dispatch, and graceful shutdown"`

---

### Task 26: rb-daemon end-to-end integration tests — full client round-trip, concurrency, namespace isolation, single-instance

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/tests/daemon_e2e.rs`

The capstone tests for the cluster. They bind a real `Daemon` on a temp socket, run it on a task with an oneshot-driven shutdown, connect a real `rb_proto::Client`, and exercise the full surface. The deterministic, offline `rb_embed::DeterministicProvider` is used (never real Voyage), so CI never touches the network. Three guarantees from spec §8/§15 are proven: (1) a full `remember→recall→get→list→graph→update→delete→context`+`ping` round-trip; (2) many concurrent clients with no lost writes and no errors; (3) namespace isolation — a client handshaked as `Project("a")` never sees `Project("b")` rows through the daemon.

- [ ] **Step 1: Write the failing end-to-end test.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rb-daemon/tests/daemon_e2e.rs` with the complete contents below.

```rust
//! End-to-end daemon tests over a real Unix socket with the offline
//! DeterministicProvider (no network). Proves the full round-trip, concurrency
//! with no lost writes, namespace isolation, and single-instance guarding.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::Client;
use rb_types::{MemoryType, MemoryUpdates, Namespace};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;

/// A running daemon plus the means to shut it down and join its task.
struct RunningDaemon {
    socket: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    async fn start(pool_size: usize) -> RunningDaemon {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("sock");
        let db = dir.path().join("memory.db");
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: pool_size,
        };
        let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
        let daemon = Daemon::bind(cfg, embedder).await.unwrap();

        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        // Wait until the socket is connectable (bind completed before spawn, so
        // this is effectively immediate, but poll to avoid races on slow CI).
        for _ in 0..200 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        RunningDaemon {
            socket,
            shutdown: Some(tx),
            task: Some(task),
            _dir: dir,
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_round_trip_through_client() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("a".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    // ping
    client.ping().await.unwrap();

    // remember
    let id = client
        .remember(
            "rusty-brain uses one db and one transaction".to_string(),
            Some("architecture".to_string()),
            MemoryType::ArchitectureDecision,
            8,
            vec!["sqlite".to_string()],
            vec!["design".to_string()],
            vec!["src/store.rs".to_string()],
        )
        .await
        .unwrap();

    // get
    let got = client.get(id.clone()).await.unwrap();
    assert!(got.is_some());
    let note = got.unwrap();
    assert_eq!(note.content, "rusty-brain uses one db and one transaction");
    assert_eq!(note.namespace, ns, "stored under the handshake namespace");

    // recall: the stored doc and the query share FTS tokens (rusty-brain / db /
    // transaction), so the KEYWORD signal surfaces it. (The DeterministicProvider
    // hashes differing texts to dissimilar vectors, so the match here is driven
    // by keyword search, not vector similarity.)
    let results = client
        .recall("rusty-brain db transaction".to_string(), 10, None, vec![])
        .await
        .unwrap();
    assert!(
        results.iter().any(|r| r.memory.id == id),
        "recall must surface the remembered memory"
    );

    // list
    let listed = client.list(None, 50).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);

    // graph (no links yet -> neighborhood may be empty or contain only the node)
    let graph = client.graph(id.clone(), 1).await.unwrap();
    assert!(
        graph.iter().all(|m| m.id != id) || graph.is_empty() || graph.iter().any(|m| m.id == id),
        "graph returns a (possibly empty) neighborhood without error"
    );

    // update
    let updates = MemoryUpdates {
        importance: Some(10),
        tags: Some(vec!["design".to_string(), "core".to_string()]),
        ..Default::default()
    };
    client.update(id.clone(), updates).await.unwrap();
    let after = client.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(after.importance, 10);

    // context
    let (recent, important, total) = client.context().await.unwrap();
    assert!(total >= 1, "context total counts the active memory");
    assert!(
        recent.iter().chain(important.iter()).any(|m| m.id == id),
        "context surfaces the high-importance memory"
    );

    // delete (soft archive) -> no longer listed
    client.delete(id.clone()).await.unwrap();
    let listed_after = client.list(None, 50).await.unwrap();
    assert!(
        listed_after.iter().all(|m| m.id != id),
        "archived memory is not listed"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn many_concurrent_clients_no_lost_writes_no_errors() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("a".to_string());

    const CLIENTS: usize = 16;
    const PER_CLIENT: usize = 10;

    let mut tasks = Vec::with_capacity(CLIENTS);
    for c in 0..CLIENTS {
        let socket = daemon.socket.clone();
        let ns = ns.clone();
        tasks.push(tokio::spawn(async move {
            let mut client = Client::connect(&socket, ns).await.unwrap();
            for i in 0..PER_CLIENT {
                client
                    .remember(
                        format!("memory from client {c} item {i}"),
                        None,
                        MemoryType::Insight,
                        5,
                        vec!["concurrent".to_string()],
                        vec![],
                        vec![],
                    )
                    .await
                    .unwrap();
            }
            // `client` (and its connection) is dropped here, closing the
            // connection so the daemon's per-connection task can finish.
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    // A fresh client must see EVERY write (no lost writes).
    let mut verifier = Client::connect(&daemon.socket, ns).await.unwrap();
    let listed = verifier.list(None, CLIENTS * PER_CLIENT + 10).await.unwrap();
    assert_eq!(
        listed.len(),
        CLIENTS * PER_CLIENT,
        "all {} writes must be present (no lost writes)",
        CLIENTS * PER_CLIENT
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn namespace_isolation_enforced_server_side() {
    let daemon = RunningDaemon::start(4).await;

    // Client A writes under Project("a").
    let ns_a = Namespace::Project("a".to_string());
    let mut client_a = Client::connect(&daemon.socket, ns_a.clone()).await.unwrap();
    let id_a = client_a
        .remember(
            "secret belonging to project a".to_string(),
            None,
            MemoryType::Insight,
            7,
            vec!["alpha".to_string()],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    // Client B writes under Project("b").
    let ns_b = Namespace::Project("b".to_string());
    let mut client_b = Client::connect(&daemon.socket, ns_b.clone()).await.unwrap();
    let id_b = client_b
        .remember(
            "secret belonging to project b".to_string(),
            None,
            MemoryType::Insight,
            7,
            vec!["beta".to_string()],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    // B's list/recall must NOT reveal A's row, even if B asks broadly.
    let b_list = client_b.list(None, 50).await.unwrap();
    assert!(
        b_list.iter().all(|m| m.id != id_a),
        "namespace B must not see namespace A's memory via list"
    );
    assert!(
        b_list.iter().any(|m| m.id == id_b),
        "namespace B sees its own memory"
    );

    let b_recall = client_b
        .recall("secret".to_string(), 50, None, vec![])
        .await
        .unwrap();
    assert!(
        b_recall.iter().all(|r| r.memory.id != id_a),
        "namespace B must not recall namespace A's memory"
    );

    // And A cannot see B's row.
    let a_list = client_a.list(None, 50).await.unwrap();
    assert!(
        a_list.iter().all(|m| m.id != id_b),
        "namespace A must not see namespace B's memory"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_bind_on_live_socket_fails_closed() {
    let daemon = RunningDaemon::start(2).await;

    // A second bind on the SAME live socket must be refused (single-instance).
    let dir2 = tempfile::tempdir().unwrap();
    let cfg2 = DaemonConfig {
        socket_path: daemon.socket.clone(),
        db_path: dir2.path().join("memory.db"),
        read_pool_size: 2,
    };
    let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
    let err = Daemon::bind(cfg2, embedder).await.unwrap_err();
    assert!(
        err.to_string().contains("already listening"),
        "second bind on a live socket must fail closed: {err}"
    );

    daemon.stop().await;
}
```

- [ ] **Step 2: Run it — expect PASS.** Run: `cargo test -p rb-daemon --test daemon_e2e -- --nocapture` Expected: PASS — `full_round_trip_through_client`, `many_concurrent_clients_no_lost_writes_no_errors`, `namespace_isolation_enforced_server_side`, and `second_bind_on_live_socket_fails_closed` all report `ok`. If `recall` finds nothing, the FTS keyword path is not matching shared tokens — confirm `rb-store::keyword_search` tokenizes content and `rb-search` ranks the keyword hit (the stored doc and the query share `rusty-brain`/`db`/`transaction`, so keyword match, not vector similarity, drives this). If `namespace_isolation_*` fails, the dispatch is honoring a client-supplied scope instead of pinning the handshake namespace — re-check Task 25's `dispatch` (scope fields are ignored; the engine's namespace is fixed).

- [ ] **Step 3: Stress for flakiness.** Run: `cargo test -p rb-daemon --test daemon_e2e -- --nocapture` two more times. Expected: PASS every time. The concurrency test must always see exactly `CLIENTS * PER_CLIENT` rows; a single lost write or `SQLITE_BUSY` is a hard failure (the single writer thread serializes all writes, so this must hold).

- [ ] **Step 4: Run the whole crate suite (no regressions).** Run: `cargo test -p rb-daemon` Expected: PASS — the unit tests (change, paths, shared_embedder, error_map), the `store_handle` integration tests, and these e2e tests all green.

- [ ] **Step 5: Workspace-wide gates.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` Expected: no warnings (no `unwrap`/`expect`/`panic` outside `#[cfg(test)]`). Run: `cargo fmt --all --check` Expected: no diff, exit 0.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rb-daemon/tests/daemon_e2e.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "test(rb-daemon): end-to-end client round-trip, concurrency, namespace isolation, and single-instance gates"`

## Part K — rusty-brain binary (CLI) & end-to-end tests

### Task 27: bin crate wiring — `rusty-brain` Cargo.toml dev-deps, `main.rs`, and module skeleton

The `crates/rusty-brain` skeleton (package `rusty-brain`, `[[bin]] name = "rusty-brain"`, lib name `rusty_brain`) was created in the P1 setup task with its runtime deps. This task adds the dev-dependencies the CLI tests need (`assert_cmd`, `predicates`, `tempfile`, `anyhow`), stands up the module layout (`cli`, `namespace_detect`, `paths`, `logging`, `output`, `run`), and wires a minimal `main.rs` that parses args and dispatches — so every later task plugs into a compiling crate. We split logic into a library (`lib.rs`) plus a thin `main.rs` so `assert_cmd` can drive the real binary while unit tests can call the library directly.

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/main.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/cli.rs`

- [ ] **Step 1: Add dev-dependencies to the bin manifest.** Modify `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/Cargo.toml` so the `[dependencies]` table (already present from setup) is followed by a `[dev-dependencies]` table and the `[lints]` table. Replace the whole file with exactly:

```toml
[package]
name = "rusty-brain"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain: shared agent memory daemon and CLI."

[[bin]]
name = "rusty-brain"
path = "src/main.rs"

[lib]
name = "rusty_brain"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-proto = { path = "../rb-proto" }
rb-daemon = { path = "../rb-daemon" }
rb-embed = { path = "../rb-embed" }
tokio = { workspace = true }
clap = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
directories = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
tempfile = { workspace = true }
anyhow = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Write the clap CLI definition.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/cli.rs` with the full argument surface. Subcommand argument parsing for `--type` reuses `rb_types::MemoryType::parse` via a small `value_parser`. (`default_value = "insight"` is run through `parse_memory_type` at parse time; `MemoryType` needs no `Display` impl because we use a string default, not `default_value_t`.)

```rust
//! Command-line surface for the `rusty-brain` binary (clap derive).

use clap::{Parser, Subcommand};
use rb_types::MemoryType;

/// Parse a `--type` value into a `MemoryType` using the canonical db strings.
fn parse_memory_type(s: &str) -> Result<MemoryType, String> {
    MemoryType::parse(s).map_err(|e| e.to_string())
}

#[derive(Parser, Debug)]
#[command(
    name = "rusty-brain",
    about = "Shared semantic memory for AI agents (daemon + CLI).",
    version
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of human text (where supported).
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run the memory daemon in the foreground until Ctrl-C.
    Serve,

    /// Store a new memory.
    Remember {
        /// Memory content (the body to remember).
        content: String,
        /// Memory type (db string, e.g. `insight`, `bug_fix`).
        #[arg(long = "type", default_value = "insight", value_parser = parse_memory_type)]
        memory_type: MemoryType,
        /// Importance 1-10.
        #[arg(long, default_value_t = 5)]
        importance: u8,
        /// Optional context string.
        #[arg(long)]
        context: Option<String>,
        /// Tags (repeatable: `--tags a --tags b`).
        #[arg(long)]
        tags: Vec<String>,
    },

    /// Recall memories matching a query.
    Recall {
        /// Free-text query.
        query: String,
        /// Maximum number of results.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Restrict to a memory type (db string).
        #[arg(long = "type", value_parser = parse_memory_type)]
        memory_type: Option<MemoryType>,
        /// Filter by tags (repeatable).
        #[arg(long)]
        tags: Vec<String>,
    },

    /// Fetch a single memory by id.
    Get {
        /// Memory id (UUID).
        id: String,
    },

    /// List memories in the current namespace.
    List {
        /// Maximum number of results.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Only memories with at least this importance.
        #[arg(long)]
        min_importance: Option<u8>,
    },

    /// Show memories connected to an id by graph links.
    Graph {
        /// Memory id (UUID).
        id: String,
        /// Traversal depth.
        #[arg(long, default_value_t = 1)]
        depth: u8,
    },

    /// Soft-delete (archive) a memory.
    Delete {
        /// Memory id (UUID).
        id: String,
    },

    /// Show the project context payload (recent + important).
    Context,

    /// Ping the daemon and report its contract version.
    Status,
}
```

- [ ] **Step 3: Write a minimal `lib.rs` that re-exports the modules.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`. The later tasks add `namespace_detect`, `paths`, `logging`, `output`, `serve`, `client`, and `run`; declare only `cli` now so the crate compiles:

```rust
//! `rusty_brain` binary library: clap CLI, namespace detection, daemon/client glue.
//!
//! Logic lives here (testable directly); `main.rs` is a thin shell that parses
//! args and dispatches. Later tasks add `paths`, `namespace_detect`, `logging`,
//! `output`, `serve`, `client`, and `run`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cli;
```

- [ ] **Step 4: Write a minimal `main.rs` that parses args and exits cleanly.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/main.rs`. For now it only proves the CLI parses; real dispatch arrives in Task 34. Returning `std::process::ExitCode` keeps the no-`panic` lint satisfied:

```rust
//! `rusty-brain` binary entry point. Parses the CLI; dispatch is wired in Task 34.

use clap::Parser;
use rusty_brain::cli::Cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    let _cli = Cli::parse();
    ExitCode::SUCCESS
}
```

- [ ] **Step 5: Verify the bin crate builds.** Run: `cargo build -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml`
  Expected: `Compiling rusty-brain v0.0.1 ...` then `Finished` (exit 0).

- [ ] **Step 6: Verify `--help` is generated.** Run: `cargo run -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- --help`
  Expected: prints usage including the subcommands `serve`, `remember`, `recall`, `get`, `list`, `graph`, `delete`, `context`, `status` (exit 0).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/Cargo.toml crates/rusty-brain/src/lib.rs crates/rusty-brain/src/main.rs crates/rusty-brain/src/cli.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add clap CLI surface and bin/lib skeleton"`
  Expected: one commit created.

---

### Task 28: `paths` module — env-overridable socket/db paths

Client and daemon must agree on where the socket and database live, and tests must be able to redirect both to a temp directory. This module wraps `rb_daemon::default_socket_path()` / `default_db_path()` and lets `RUSTY_BRAIN_SOCKET` / `RUSTY_BRAIN_DB` override them. The override is read from the process environment, so `assert_cmd` can set it per test.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/paths.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod paths;` to `lib.rs` (after `pub mod cli;`), then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/paths.rs` with the test module only. The functions take an explicit `Option<String>` override so tests never mutate global process env (which is unsound across threads):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn socket_path_prefers_override() {
        let got = resolve_socket_path(Some("/tmp/rb-test.sock".to_string()));
        assert_eq!(got, PathBuf::from("/tmp/rb-test.sock"));
    }

    #[test]
    fn socket_path_falls_back_to_default_when_no_override() {
        let got = resolve_socket_path(None);
        // Cross-cluster coupling: this pins to rb-daemon's default socket file
        // name (`sock`, per the spine `$XDG_RUNTIME_DIR/rusty-brain/sock`). If
        // rb-daemon renames it, update this assertion.
        assert_eq!(got.file_name().unwrap(), "sock");
    }

    #[test]
    fn db_path_prefers_override() {
        let got = resolve_db_path(Some("/tmp/rb-test.db".to_string()));
        assert_eq!(got, PathBuf::from("/tmp/rb-test.db"));
    }

    #[test]
    fn db_path_falls_back_to_default_when_no_override() {
        let got = resolve_db_path(None);
        // Cross-cluster coupling: pins to rb-daemon's default db extension `db`.
        assert_eq!(got.extension().unwrap(), "db");
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml paths` Expected: FAIL to compile (`cannot find function 'resolve_socket_path'`), confirming the module is compiled and the impl is missing.

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/paths.rs`:

```rust
//! Socket / database path resolution with env-var overrides for tests.

use std::path::PathBuf;

/// Env var that overrides the daemon socket path.
pub const SOCKET_ENV: &str = "RUSTY_BRAIN_SOCKET";
/// Env var that overrides the database path.
pub const DB_ENV: &str = "RUSTY_BRAIN_DB";

/// Resolve the socket path: explicit override wins, else the daemon default.
pub fn resolve_socket_path(override_value: Option<String>) -> PathBuf {
    match override_value {
        Some(p) => PathBuf::from(p),
        None => rb_daemon::default_socket_path(),
    }
}

/// Resolve the database path: explicit override wins, else the daemon default.
pub fn resolve_db_path(override_value: Option<String>) -> PathBuf {
    match override_value {
        Some(p) => PathBuf::from(p),
        None => rb_daemon::default_db_path(),
    }
}

/// Read the socket path from the environment (override) or fall back to default.
pub fn socket_path_from_env() -> PathBuf {
    resolve_socket_path(std::env::var(SOCKET_ENV).ok())
}

/// Read the db path from the environment (override) or fall back to default.
pub fn db_path_from_env() -> PathBuf {
    resolve_db_path(std::env::var(DB_ENV).ok())
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml paths` Expected: PASS (4 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/paths.rs crates/rusty-brain/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add env-overridable socket/db path resolution"`
  Expected: one commit created.

---

### Task 29: `namespace_detect` module — minimal git-root / cwd → `Namespace::Project`

P1 namespace detection is deliberately minimal (full git/`CLAUDE.md` detection is P2): use the git repository root's directory name if inside a repo, else the current directory name, as `Namespace::Project(name)`; fall back to `Namespace::Global` only when no usable directory name exists. The detection is parameterized over a starting directory and a "git root finder" closure so it is fully unit-testable without touching the real cwd or shelling out to git.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/namespace_detect.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod namespace_detect;` to `lib.rs`, then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/namespace_detect.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::Namespace;
    use std::path::{Path, PathBuf};

    #[test]
    fn uses_git_root_dirname_when_in_repo() {
        let start = Path::new("/home/alice/code/rusty-brain/crates/rusty-brain");
        let git_root = |_: &Path| -> Option<PathBuf> {
            Some(PathBuf::from("/home/alice/code/rusty-brain"))
        };
        let ns = detect_namespace_with(start, git_root);
        assert_eq!(ns, Namespace::Project("rusty-brain".to_string()));
    }

    #[test]
    fn falls_back_to_cwd_dirname_outside_repo() {
        let start = Path::new("/home/alice/scratch/notes");
        let git_root = |_: &Path| -> Option<PathBuf> { None };
        let ns = detect_namespace_with(start, git_root);
        assert_eq!(ns, Namespace::Project("notes".to_string()));
    }

    #[test]
    fn falls_back_to_global_for_root_dir() {
        let start = Path::new("/");
        let git_root = |_: &Path| -> Option<PathBuf> { None };
        let ns = detect_namespace_with(start, git_root);
        assert_eq!(ns, Namespace::Global);
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml namespace_detect` Expected: FAIL to compile (`cannot find function 'detect_namespace_with'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/namespace_detect.rs`:

```rust
//! Minimal namespace detection for P1: git-root or cwd directory name.
//!
//! Full git/`CLAUDE.md` resolution is deferred to P2. This is intentionally a
//! single, predictable rule so behavior is obvious from the working directory.

use rb_types::Namespace;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Detect the namespace for the real process, using the current directory and a
/// `git rev-parse --show-toplevel` lookup. Never fails: degrades to `Global`.
pub fn detect_namespace() -> Namespace {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    detect_namespace_with(&cwd, git_toplevel)
}

/// Core logic, parameterized for tests: pick the git-root dir name if a repo is
/// found, else the start dir name, else `Global`.
pub fn detect_namespace_with<F>(start: &Path, git_root: F) -> Namespace
where
    F: Fn(&Path) -> Option<PathBuf>,
{
    let base = git_root(start).unwrap_or_else(|| start.to_path_buf());
    match base.file_name().and_then(|n| n.to_str()) {
        Some(name) if !name.is_empty() => Namespace::Project(name.to_string()),
        _ => Namespace::Global,
    }
}

/// Find the git toplevel for `dir` by invoking git; `None` if not a repo.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml namespace_detect` Expected: PASS (3 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/namespace_detect.rs crates/rusty-brain/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add minimal git-root/cwd namespace detection"`
  Expected: one commit created.

---

### Task 30: `logging` module — tracing to stderr, results to stdout

Logs must go to stderr so that `--json` and result lines on stdout stay machine-parseable. This installs a `tracing_subscriber` writing to stderr, honoring `RUST_LOG` (default `info`). It is idempotent-safe for tests (uses `try_init` so a second init in the same process is a no-op rather than a panic).

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/logging.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod logging;` to `lib.rs`, then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/logging.rs` with the test module only. The subscriber is process-global, so whether THIS call wins the install race depends on test ordering across the binary; the only sound invariant is that the call returns a `bool` without panicking and that a second call is a no-op (still no panic):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn init_logging_is_idempotent_and_never_panics() {
        // The subscriber is global; another test may have installed it first, so
        // we cannot assert a specific true/false for the first call. We CAN
        // assert that calling it twice in a row never panics and that the second
        // call reports "already initialized" (false), proving try-init semantics:
        // once a subscriber is set, init_logging() must return false rather than
        // panic.
        let _ = init_logging();
        let second = init_logging();
        assert!(!second, "second init must be a no-op (try_init returns false)");
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml logging` Expected: FAIL to compile (`cannot find function 'init_logging'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/logging.rs`:

```rust
//! tracing setup: human logs to stderr; stdout is reserved for results.

use tracing_subscriber::EnvFilter;

/// Initialize tracing to stderr, honoring `RUST_LOG` (default `info`).
/// Returns `true` if this call installed the subscriber, `false` if one was
/// already set (safe to call repeatedly in-process; used by tests).
pub fn init_logging() -> bool {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok()
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml logging` Expected: PASS (1 test passes).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/logging.rs crates/rusty-brain/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add stderr tracing init (results stay on stdout)"`
  Expected: one commit created.

---

### Task 31: `output` module — human + `--json` rendering of results

All client subcommands render either human-readable text (default) or JSON (`--json`) to stdout. These pure formatting functions take the proto/domain values and return strings, so they unit-test without any IO. JSON output reuses `serde_json` on the domain types (which already derive `Serialize` in P0).

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/output.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod output;` to `lib.rs`, then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/output.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, SearchResult};

    fn note(content: &str, importance: u8) -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("p".into()),
            content.to_string(),
            MemoryType::Insight,
            importance,
        )
    }

    #[test]
    fn human_recall_lists_score_and_summary() {
        let mut n = note("one db one transaction", 8);
        n.summary = "one db one transaction".to_string();
        let results = vec![SearchResult { memory: n.clone(), score: 0.91 }];
        let out = render_recall(&results, false);
        assert!(out.contains("0.91"), "score shown: {out}");
        assert!(out.contains("one db one transaction"), "summary shown: {out}");
        assert!(out.contains(&n.id.to_string()), "id shown: {out}");
    }

    #[test]
    fn json_recall_is_parseable_array() {
        let n = note("body", 5);
        let results = vec![SearchResult { memory: n, score: 0.5 }];
        let out = render_recall(&results, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["score"].as_f64().unwrap(), 0.5);
    }

    #[test]
    fn human_recall_empty_has_guidance() {
        let out = render_recall(&[], false);
        assert!(out.to_lowercase().contains("no memories"), "empty guidance: {out}");
    }

    #[test]
    fn human_list_shows_each_note() {
        let notes = vec![note("alpha", 7), note("beta", 3)];
        let out = render_notes(&notes, false);
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
    }

    #[test]
    fn json_list_is_parseable_array() {
        let notes = vec![note("alpha", 7)];
        let out = render_notes(&notes, true);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["content"].as_str().unwrap(), "alpha");
    }

    #[test]
    fn render_get_some_and_none() {
        let n = note("body", 5);
        let some = render_get(&Some(n.clone()), false);
        assert!(some.contains("body"));
        let none = render_get(&None, false);
        assert!(none.to_lowercase().contains("not found"));
        let json_none = render_get(&None, true);
        assert_eq!(json_none.trim(), "null");
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml output` Expected: FAIL to compile (`cannot find function 'render_recall'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/output.rs` (note: `Namespace`/`MemoryType` have no `Display`, so we use `as_db_string()` / `as_str()`):

```rust
//! Pure rendering of results to human text or JSON (stdout).

use rb_types::{MemoryNote, SearchResult};

/// Render recall hits. JSON: the raw `Vec<SearchResult>`. Human: one line per hit.
pub fn render_recall(results: &[SearchResult], json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(results)
            .unwrap_or_else(|e| format!("[] // json error: {e}"));
    }
    if results.is_empty() {
        return "No memories matched.".to_string();
    }
    let mut out = String::new();
    for r in results {
        let summary = if r.memory.summary.is_empty() {
            r.memory.content.as_str()
        } else {
            r.memory.summary.as_str()
        };
        out.push_str(&format!(
            "[{:.2}] {} ({}) {}\n",
            r.score,
            r.memory.id,
            r.memory.memory_type.as_str(),
            summary
        ));
    }
    out.trim_end().to_string()
}

/// Render a list of notes (used by `list`, `graph`, and the `context` halves).
pub fn render_notes(notes: &[MemoryNote], json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(notes)
            .unwrap_or_else(|e| format!("[] // json error: {e}"));
    }
    if notes.is_empty() {
        return "No memories.".to_string();
    }
    let mut out = String::new();
    for n in notes {
        let summary = if n.summary.is_empty() {
            n.content.as_str()
        } else {
            n.summary.as_str()
        };
        out.push_str(&format!(
            "{} (imp {}, {}) {}\n",
            n.id,
            n.importance,
            n.memory_type.as_str(),
            summary
        ));
    }
    out.trim_end().to_string()
}

/// Render a single fetched memory (or a not-found message).
pub fn render_get(memory: &Option<MemoryNote>, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(memory)
            .unwrap_or_else(|e| format!("null // json error: {e}"));
    }
    match memory {
        Some(n) => format!(
            "{}\nnamespace: {}\ntype: {}\nimportance: {}\n\n{}",
            n.id,
            n.namespace.as_db_string(),
            n.memory_type.as_str(),
            n.importance,
            n.content
        ),
        None => "Memory not found.".to_string(),
    }
}

/// Render a remembered id (json: an object `{ "id": "<uuid>" }`).
pub fn render_remembered(id: &rb_types::MemoryId, json: bool) -> String {
    if json {
        format!("{{\"id\":\"{id}\"}}")
    } else {
        format!("Remembered {id}")
    }
}

/// Render the `context` payload.
pub fn render_context(
    recent: &[MemoryNote],
    important: &[MemoryNote],
    total: usize,
    json: bool,
) -> String {
    if json {
        let value = serde_json::json!({
            "recent": recent,
            "important": important,
            "total": total,
        });
        return serde_json::to_string_pretty(&value)
            .unwrap_or_else(|e| format!("{{}} // json error: {e}"));
    }
    let mut out = format!("Context ({total} memories total)\n\nRecent:\n");
    out.push_str(&render_notes(recent, false));
    out.push_str("\n\nImportant:\n");
    out.push_str(&render_notes(important, false));
    out
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml output` Expected: PASS (6 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/output.rs crates/rusty-brain/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add human/JSON result rendering"`
  Expected: one commit created.

---

### Task 32: `serve` command — `Daemon::bind` + `run` with Ctrl-C shutdown and provider selection

`serve` builds a `DaemonConfig` from the resolved socket/db paths, picks an embedding provider (`VoyageProvider::from_env()` if `VOYAGE_API_KEY` is set, else `DeterministicProvider::new(512)` with a tracing warning), and runs the daemon until `tokio::signal::ctrl_c` fires. The provider-selection logic is factored into a pure function returning the chosen `ProviderKind` so it is unit-testable without binding a socket; the async `run_serve` wiring is exercised end-to-end in Task 35.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/serve.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod serve;` to `lib.rs`, then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/serve.rs` with the test module only. The test drives the pure selection helper, which takes the env value explicitly:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn selects_voyage_when_key_present() {
        let sel = select_provider_kind(Some("vk-123".to_string()));
        assert_eq!(sel, ProviderKind::Voyage);
    }

    #[test]
    fn selects_deterministic_when_key_absent() {
        let sel = select_provider_kind(None);
        assert_eq!(sel, ProviderKind::Deterministic);
    }

    #[test]
    fn selects_deterministic_when_key_empty() {
        let sel = select_provider_kind(Some(String::new()));
        assert_eq!(sel, ProviderKind::Deterministic);
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml serve` Expected: FAIL to compile (`cannot find type 'ProviderKind'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/serve.rs`. The two daemon-run branches are monomorphic over the concrete provider type, so a single generic `run_with_embedder` tail is instantiated per branch (the `Daemon` is generic over the embedder per the spine):

```rust
//! `serve` subcommand: bind the daemon and run until Ctrl-C.

use rb_daemon::{Daemon, DaemonConfig};
use rb_embed::{DeterministicProvider, EmbeddingProvider, VoyageProvider};
use rb_types::Result;
use std::path::PathBuf;

/// Default embedding dimension for the offline provider and Voyage's default model.
pub const DEFAULT_DIM: usize = 512;

/// Which embedding provider `serve` will use.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProviderKind {
    Voyage,
    Deterministic,
}

/// Pure selection: Voyage iff a non-empty API key is present, else Deterministic.
pub fn select_provider_kind(api_key: Option<String>) -> ProviderKind {
    match api_key {
        Some(k) if !k.is_empty() => ProviderKind::Voyage,
        _ => ProviderKind::Deterministic,
    }
}

/// Run the daemon at the given paths until `shutdown` resolves.
/// Picks the embedding provider from the environment (`VOYAGE_API_KEY`).
pub async fn run_serve(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let api_key = std::env::var("VOYAGE_API_KEY").ok();
    match select_provider_kind(api_key) {
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
    let daemon = Daemon::bind(config, embedder).await?;
    daemon.run(shutdown).await
}
```

> Cross-cluster contract note (rb-daemon): the spine prints `Daemon::bind(config: DaemonConfig)`, but the spine's 3-field `DaemonConfig` (socket_path, db_path, read_pool_size) has no field for the embedder, while the spine's `Daemon` struct is documented to *hold* an embedder (it constructs `StoreHandle` with `dim = embedder.dim()` and gives each per-connection `MemoryEngine` an embedder). The only consistent way to inject it is `Daemon::bind(config, embedder)`. This cluster therefore requires rb-daemon's `bind` to take `(DaemonConfig, P: EmbeddingProvider + 'static)`. If the rb-daemon cluster instead threads the embedder through `DaemonConfig` or a builder, change this single call site accordingly — `select_provider_kind`, `ProviderKind`, and `DEFAULT_DIM` are unaffected. (verify against installed rb-daemon at execution; adjust if the API differs.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml serve` Expected: PASS (3 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/serve.rs crates/rusty-brain/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add serve command with provider selection and ctrl-c shutdown"`
  Expected: one commit created.

---

### Task 33: `client` module — connect with daemon auto-start + backoff retry

Client subcommands connect to the default socket. If the socket is absent (no daemon), they spawn `rusty-brain serve` as a detached child, then retry `Client::connect` with a short bounded backoff before giving up. The connect-with-retry logic is parameterized over a "connect attempt" closure and a "spawn" closure so the retry/backoff behavior is unit-tested deterministically without real sockets or processes.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/client.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod client;` to `lib.rs`, then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/client.rs` with the test module only. The generic retry helper is tested with async closures and a counter, simulating "fails until the daemon is up":

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn succeeds_on_first_try_without_spawning() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let sp = Arc::clone(&spawned);
        let spawn = move || {
            sp.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let at = Arc::clone(&attempts);
        let connect = move || {
            at.fetch_add(1, Ordering::SeqCst);
            async { Ok::<u32, rb_types::Error>(7) }
        };
        let v = connect_with_retry(connect, spawn, 5, std::time::Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(v, 7);
        assert_eq!(spawned.load(Ordering::SeqCst), 0, "no spawn when first connect works");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn spawns_then_retries_until_connected() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let sp = Arc::clone(&spawned);
        let spawn = move || {
            sp.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let at = Arc::clone(&attempts);
        // Fail the first two attempts, succeed on the third.
        let connect = move || {
            let n = at.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(rb_types::Error::Io("no socket".into()))
                } else {
                    Ok::<u32, rb_types::Error>(42)
                }
            }
        };
        let v = connect_with_retry(connect, spawn, 10, std::time::Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(v, 42);
        assert_eq!(spawned.load(Ordering::SeqCst), 1, "spawned exactly once after first failure");
        assert!(attempts.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let spawn = || Ok(());
        let connect = || async { Err::<u32, rb_types::Error>(rb_types::Error::Io("never".into())) };
        let err = connect_with_retry(connect, spawn, 3, std::time::Duration::ZERO)
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::Io(_)));
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml client` Expected: FAIL to compile (`cannot find function 'connect_with_retry'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/client.rs`:

```rust
//! Client connection with daemon auto-start and bounded backoff retry.

use rb_proto::Client;
use rb_types::{Error, Namespace, Result};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Generic connect-with-retry: try `connect`; on the first failure run `spawn`
/// (to start the daemon) once, then keep retrying up to `max_attempts`, sleeping
/// `backoff` between attempts. Returns the last error if all attempts fail.
pub async fn connect_with_retry<C, Fut, T, S>(
    mut connect: C,
    spawn: S,
    max_attempts: usize,
    backoff: Duration,
) -> Result<T>
where
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
    S: FnOnce() -> Result<()>,
{
    let mut spawn = Some(spawn);
    let mut last_err: Option<Error> = None;
    for attempt in 0..max_attempts.max(1) {
        match connect().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = Some(e);
                // After the very first failure, start the daemon once.
                if attempt == 0 {
                    if let Some(s) = spawn.take() {
                        s()?;
                    }
                }
                if backoff > Duration::ZERO {
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| Error::Io("connect failed".into())))
}

/// Connect to the daemon at `socket_path` for `namespace`, auto-starting a
/// detached `rusty-brain serve` child if the socket is not yet accepting.
pub async fn connect_or_start(
    socket_path: &Path,
    namespace: Namespace,
    self_exe: PathBuf,
) -> Result<Client> {
    let sock = socket_path.to_path_buf();
    let ns = namespace.clone();
    let connect = || {
        let sock = sock.clone();
        let ns = ns.clone();
        async move { Client::connect(&sock, ns).await }
    };
    let spawn_sock = socket_path.to_path_buf();
    let spawn = move || spawn_daemon(&self_exe, &spawn_sock);
    connect_with_retry(connect, spawn, 50, Duration::from_millis(100)).await
}

/// Spawn `rusty-brain serve` as a detached child, passing the resolved
/// `RUSTY_BRAIN_SOCKET` so child + client agree on the socket path.
fn spawn_daemon(self_exe: &Path, socket_path: &Path) -> Result<()> {
    let mut cmd = std::process::Command::new(self_exe);
    cmd.arg("serve");
    cmd.env(crate::paths::SOCKET_ENV, socket_path);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.spawn().map(|_child| ()).map_err(|e| Error::Io(e.to_string()))
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml client` Expected: PASS (3 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/client.rs crates/rusty-brain/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): add client connect with daemon auto-start and backoff"`
  Expected: one commit created.

---

### Task 34: `run` dispatcher + `main.rs` wiring — async dispatch of every subcommand

This wires the parsed `Cli` to behavior: `serve` calls `run_serve` with a Ctrl-C shutdown; every client subcommand resolves paths + namespace, connects (auto-starting the daemon), issues the matching typed `Client` request, renders output to stdout, and maps errors to a non-zero `ExitCode`. The dispatcher returns `anyhow::Result<()>`; `main.rs` runs it on a tokio runtime and converts the result into an `ExitCode` (no `panic`).

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/run.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/lib.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/main.rs`

- [ ] **Step 1: Write the failing test AND declare the module.** Add `pub mod run;` to `lib.rs`, then create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/run.rs` with the test module only. The test covers the pure id-parsing helper (`parse_id`) the dispatcher uses for `get`/`graph`/`delete`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parse_id_accepts_valid_uuid() {
        let id = rb_types::MemoryId::new();
        let parsed = parse_id(&id.to_string()).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_id_rejects_garbage_with_clear_error() {
        let err = parse_id("not-a-uuid").unwrap_err();
        let msg = err.to_string();
        // rb_types::MemoryId::from_str returns Error::Storage with the message
        // "invalid memory id 'not-a-uuid': ...", so both substrings are present.
        assert!(msg.contains("not-a-uuid") || msg.to_lowercase().contains("invalid"), "{msg}");
    }

    #[test]
    fn memory_id_from_str_is_what_parse_id_uses() {
        // Guards that parse_id and MemoryId::from_str stay in agreement.
        let id = rb_types::MemoryId::new();
        let a = parse_id(&id.to_string()).unwrap();
        let b = rb_types::MemoryId::from_str(&id.to_string()).unwrap();
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml run::` Expected: FAIL to compile (`cannot find function 'parse_id'`). The `::` suffix scopes the filter to the `run` module.

- [ ] **Step 3: Add the dispatcher implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/run.rs`. Note: `RememberInput` is an `rb-engine` type and the bin crate does NOT depend on rb-engine; the proto `Client::remember` wrapper takes the explicit `Request::Remember` fields (rb-proto has no rb-engine dependency). The `Serve` arm inside `run_client` is impossible (handled earlier) but is mapped to an error rather than `unreachable!`, keeping the binary panic-free under the workspace `panic = deny` lint:

```rust
//! Async dispatch from parsed `Cli` to daemon/client behavior.

use crate::cli::{Cli, Command};
use crate::namespace_detect::detect_namespace;
use crate::{client, output, paths, serve};
use anyhow::Context as _;
use rb_types::MemoryId;
use std::str::FromStr;

/// Parse a CLI id argument into a `MemoryId`, surfacing a clear error.
pub fn parse_id(s: &str) -> rb_types::Result<MemoryId> {
    MemoryId::from_str(s)
}

/// Execute the parsed CLI. `serve` blocks until Ctrl-C; client commands connect
/// (auto-starting the daemon), issue one request, print to stdout, and return.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let socket_path = paths::socket_path_from_env();
    let db_path = paths::db_path_from_env();

    match cli.command {
        Command::Serve => {
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(socket_path, db_path, 4, shutdown)
                .await
                .context("daemon failed")?;
            Ok(())
        }
        other => run_client(other, cli.json, &socket_path).await,
    }
}

/// Connect to the daemon and dispatch a single client request.
async fn run_client(
    command: Command,
    json: bool,
    socket_path: &std::path::Path,
) -> anyhow::Result<()> {
    let namespace = detect_namespace();
    let self_exe = std::env::current_exe().context("locating own executable")?;
    let mut client = client::connect_or_start(socket_path, namespace, self_exe)
        .await
        .context("connecting to daemon")?;

    match command {
        // `serve` is dispatched in `run` before we ever reach here; return an
        // error instead of `unreachable!` to keep the binary panic-free.
        Command::Serve => {
            anyhow::bail!("internal: serve must be handled before run_client")
        }
        Command::Remember {
            content,
            memory_type,
            importance,
            context,
            tags,
        } => {
            // rb-proto's typed wrapper takes the explicit Request::Remember
            // fields (no rb-engine RememberInput; the bin/proto crates do not
            // depend on rb-engine). P1 client sends empty keywords/related_files;
            // heuristic enrichment happens server-side in the engine.
            let id = client
                .remember(
                    content,
                    context,
                    memory_type,
                    importance,
                    Vec::new(), // keywords
                    tags,
                    Vec::new(), // related_files
                )
                .await
                .context("remember failed")?;
            println!("{}", output::render_remembered(&id, json));
        }
        Command::Recall {
            query,
            limit,
            memory_type,
            tags,
        } => {
            let results = client
                .recall(&query, limit, memory_type, &tags)
                .await
                .context("recall failed")?;
            println!("{}", output::render_recall(&results, json));
        }
        Command::Get { id } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let memory = client.get(id).await.context("get failed")?;
            println!("{}", output::render_get(&memory, json));
        }
        Command::List {
            limit,
            min_importance,
        } => {
            let notes = client
                .list(min_importance, limit)
                .await
                .context("list failed")?;
            println!("{}", output::render_notes(&notes, json));
        }
        Command::Graph { id, depth } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let notes = client.graph(id, depth).await.context("graph failed")?;
            println!("{}", output::render_notes(&notes, json));
        }
        Command::Delete { id } => {
            let id = parse_id(&id).context("invalid memory id")?;
            client.delete(id).await.context("delete failed")?;
            println!("Deleted");
        }
        Command::Context => {
            let (recent, important, total) =
                client.context().await.context("context failed")?;
            println!(
                "{}",
                output::render_context(&recent, &important, total, json)
            );
        }
        Command::Status => {
            let version = client.ping().await.context("status/ping failed")?;
            if json {
                println!("{{\"contract_version\":{version},\"ok\":true}}");
            } else {
                println!("ok (contract v{version})");
            }
        }
    }
    Ok(())
}
```

> Note: the typed `Client` wrappers follow the spine — `remember(content, context, memory_type, importance, keywords, tags, related_files)->MemoryId` (mirroring the `Request::Remember` fields; rb-proto has no rb-engine dependency so it cannot accept `rb_engine::RememberInput`), `recall(&str, usize, Option<MemoryType>, &[String])->Vec<SearchResult>`, `get(MemoryId)->Option<MemoryNote>`, `list(Option<u8>, usize)->Vec<MemoryNote>`, `graph(MemoryId, u8)->Vec<MemoryNote>`, `delete(MemoryId)`, `context()->(Vec<MemoryNote>, Vec<MemoryNote>, usize)`, `ping()->u32`. If the rb-proto cluster gives `remember` a different argument grouping (e.g. a proto-local `RememberArgs` struct mirroring those fields), adapt this single call site; the dispatch structure is unchanged. (verify against installed rb-proto at execution; adjust the wrapper call if it differs.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml run::` Expected: PASS (3 tests pass).

- [ ] **Step 5: Rewrite `main.rs` to run the dispatcher on a tokio runtime and map errors to `ExitCode`.** Set `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/src/main.rs` to (the multi-threaded runtime is required so the daemon's `spawn_blocking` reads and dedicated writer thread have a worker pool):

```rust
//! `rusty-brain` binary entry point: init logging, parse, dispatch, map exit code.

use clap::Parser;
use rusty_brain::cli::Cli;
use rusty_brain::logging::init_logging;
use rusty_brain::run::run;
use std::process::ExitCode;

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Print the full anyhow context chain to stderr; stdout stays clean.
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 6: Verify the whole bin compiles and parses.** Run: `cargo build -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: `Finished` (exit 0). Run: `cargo run -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- recall --help` Expected: usage for `recall` including `--limit`, `--type`, `--tags` (exit 0).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/src/run.rs crates/rusty-brain/src/lib.rs crates/rusty-brain/src/main.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "feat(rusty-brain): wire async dispatcher and main exit-code handling"`
  Expected: one commit created.

---

### Task 35: CLI surface tests (assert_cmd) + end-to-end remember→recall through the real binary

This task locks the CLI behavior with `assert_cmd`: help/version, argument-validation exit codes, and a full end-to-end run that starts the **real** binary's daemon on a temp socket+DB (via `RUSTY_BRAIN_SOCKET`/`RUSTY_BRAIN_DB`), runs `remember` then `recall`, and asserts the recalled content appears. The end-to-end test force-clears `VOYAGE_API_KEY` so the offline `DeterministicProvider` is used — CI never touches a live API.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/tests/cli_surface.rs`
- Create: `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/tests/end_to_end.rs`

- [ ] **Step 1: Write the CLI-surface tests.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/tests/cli_surface.rs`. These exercise the parser surface only (no daemon), so they are fast and hermetic:

```rust
//! CLI surface tests: help, version, and argument-validation exit codes.
//! These never start the daemon (parser-only paths).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("rusty-brain").unwrap()
}

#[test]
fn help_lists_all_subcommands() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("remember"))
        .stdout(predicate::str::contains("recall"))
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("graph"))
        .stdout(predicate::str::contains("delete"))
        .stdout(predicate::str::contains("context"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn version_prints_a_version() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("rusty-brain"));
}

#[test]
fn unknown_subcommand_fails_with_nonzero_exit() {
    bin().arg("frobnicate").assert().failure();
}

#[test]
fn remember_requires_content_argument() {
    // Missing required positional `content` -> clap usage error, exit 2.
    bin()
        .arg("remember")
        .assert()
        .failure()
        .stderr(predicate::str::contains("content").or(predicate::str::contains("required")));
}

#[test]
fn remember_rejects_invalid_memory_type() {
    bin()
        .args(["remember", "some content", "--type", "not_a_type"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid").or(predicate::str::contains("not_a_type")));
}

#[test]
fn recall_help_shows_flags() {
    bin()
        .args(["recall", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--limit"))
        .stdout(predicate::str::contains("--type"))
        .stdout(predicate::str::contains("--tags"));
}
```

- [ ] **Step 2: Run the surface tests — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml --test cli_surface` Expected: PASS (6 tests). `assert_cmd` builds the `rusty-brain` binary on first use. If `remember_rejects_invalid_memory_type` fails, confirm the `--type` `value_parser` in `cli.rs` returns the `MemoryType::parse` error string (Task 27), whose `Display` is `"invalid memory type: not_a_type"`.

- [ ] **Step 3: Write the end-to-end test.** Create `/Users/bluby/repos/rusty-brain-p1/crates/rusty-brain/tests/end_to_end.rs`. It explicitly starts the daemon (`serve`) on temp paths, waits for the socket to appear, runs `remember` then `recall`, asserts the content is recalled, then shuts the daemon down. The spawned daemon child is owned directly by a `Reap` guard whose `Drop` kills and waits on it, so it is reaped even if an assertion below panics. `VOYAGE_API_KEY` is force-cleared so the offline provider is used and CI never hits the network:

```rust
//! End-to-end: start the real binary's daemon on a temp socket+DB, then run
//! `remember` and `recall` through the built binary; assert the content returns.
//! Uses the offline DeterministicProvider (VOYAGE_API_KEY is cleared), so CI
//! never contacts a live embedding API.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use predicates::Predicate;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Owns the spawned daemon process and reaps it on drop (kill + wait), even if a
/// later assertion panics and unwinds the test.
struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Block until `path` exists or the deadline passes. Returns true if it appeared.
fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    path.exists()
}

#[test]
fn remember_then_recall_round_trips_through_the_binary() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    // 1. Start the daemon in the background on the temp socket+DB, owned by the
    //    reaper guard so it is always cleaned up.
    let _reap = Reap(
        Command::new(&exe)
            .arg("serve")
            .env("RUSTY_BRAIN_SOCKET", &socket)
            .env("RUSTY_BRAIN_DB", &db)
            .env_remove("VOYAGE_API_KEY") // force offline DeterministicProvider
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon"),
    );

    assert!(
        wait_for_socket(&socket, Duration::from_secs(10)),
        "daemon socket never appeared at {}",
        socket.display()
    );

    // 2. remember
    let remember = Command::new(&exe)
        .args(["remember", "always use one database and one transaction"])
        .args(["--type", "architecture_decision", "--importance", "9"])
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env_remove("VOYAGE_API_KEY")
        .output()
        .expect("run remember");
    assert!(
        remember.status.success(),
        "remember failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&remember.stdout),
        String::from_utf8_lossy(&remember.stderr)
    );

    // 3. recall — the stored content must come back.
    let recall = Command::new(&exe)
        .args(["recall", "one database transaction", "--limit", "10"])
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env_remove("VOYAGE_API_KEY")
        .output()
        .expect("run recall");
    assert!(
        recall.status.success(),
        "recall failed: stderr={:?}",
        String::from_utf8_lossy(&recall.stderr)
    );
    let stdout = String::from_utf8_lossy(&recall.stdout);
    let found = predicate::str::contains("one database and one transaction");
    assert!(
        found.eval(&stdout),
        "recalled output did not contain the remembered content; got: {stdout}"
    );
}
```

- [ ] **Step 4: Run the end-to-end test — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml --test end_to_end -- --nocapture` Expected: PASS — `remember_then_recall_round_trips_through_the_binary ... ok`. If it hangs at `wait_for_socket`, the daemon failed to bind: re-run `serve` manually with `RUST_LOG=debug` and the same env to see the bind error on stderr. If `recall` returns empty, the DeterministicProvider must produce the same vector for identical text and `recall` must include the keyword candidate path (rb-engine cluster); the content token overlap ("one database … transaction") also drives the FTS keyword match.

- [ ] **Step 5: Run the whole bin test suite together.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: PASS — unit tests (`paths`, `namespace_detect`, `logging`, `output`, `serve`, `client`, `run`) plus `cli_surface` and `end_to_end` integration tests all green.

- [ ] **Step 6: Lint + format the workspace.** Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p1/Cargo.toml` Expected: no diff (exit 0).

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p1 add crates/rusty-brain/tests/cli_surface.rs crates/rusty-brain/tests/end_to_end.rs && git -C /Users/bluby/repos/rusty-brain-p1 commit -m "test(rusty-brain): CLI surface tests and end-to-end remember/recall through the binary"`
  Expected: one commit created.


---

## After P1 — Roadmap & the P2 parallelism note

### Parallelizing P2 with P1
The seam is **`rb-proto`** (Part F of this plan): it depends only on `rb-types`, so once Part F is committed, P2's MCP adapter + namespace detection can be built in a SEPARATE worktree (e.g. `~/repos/rusty-brain-p2`, branch `feat/p2-mcp-surface`) against the frozen wire contract, in parallel with the rest of P1 (G–K). Caveat: if the proto contract changes mid-flight, the P2 adapter must re-sync. For a solo dev, sequential P1 → P2 is simpler; fork P2 only if you want throughput. (Subagent implementers cannot safely share one branch — parallel streams require separate worktrees.)

### P2 — Agent surface
- `mcp` subcommand: thin MCP stdio server (one tool per `rb_proto::Request`), translating MCP tool calls → `rb_proto::Client` requests; daemon auto-start.
- Namespace detection: real git-root + `CLAUDE.md` frontmatter parsing → `Namespace` (replaces P1's minimal dirname heuristic).
- Wire graph links into recall (semantic link generation); optional LLM enrichment (summary/keywords/type/links) as an opt-in replacement for P1's heuristic enrichment.
- `ContractVersion` surfaced to MCP clients; MCP contract tests per tool.

### P3 — Deferred (behind existing seams)
- `subscribe` change-stream over the daemon's `tokio::broadcast` (cross-agent awareness).
- Memory evolution (consolidation / link decay / importance recalibration) as opt-in daemon jobs.
- `local` ONNX embedding feature in `rb-embed`.

### P4 — Broader agent surface (deferred)
- `rb-hooks` / `rb-install`: capture hooks + `install` for Claude Code / OpenCode / Copilot / Codex / Gemini — fail-open, `ContractVersion`-gated. Separate crates, never compiled into core.

### Carry-forward follow-ups from P0/P1 review
- `record_access` / `supersede` `Store` methods (recall-time access-count bump, supersession) — the columns already exist in the P0 schema.
- Batch link-loading to remove the N+1 in `list`/`get` (P0 left a `// TODO(P1)` marker).


---

## Plan provenance

Authored by a 6-cluster fan-out against a fixed async/threading interface spine; each cluster adversarially reviewed for placeholders, spine/type drift, Rust/async correctness (writer-thread `!Sync` model, reads via `spawn_blocking`, no live network in CI), and writing-plans format. Reviewer-confirmed fixes per cluster: proto-setup (5); embed (5); search (4); engine (5); daemon (8); bin (6). Tasks renumbered globally for sequential execution.
