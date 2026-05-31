# rusty-brain — P0 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the rusty-brain foundation — a Cargo workspace, the `rb-types` domain crate, and the `rb-store` SQLite+sqlite-vec storage engine with reproducible, checksummed migrations — all gated by CI (fmt, clippy-deny, tests, cargo-deny, cargo-audit) and three anti-regression guard tests (migration reproducibility, concurrency, namespace isolation).

**Architecture:** A workspace of focused crates with compiler-enforced boundaries. `rb-types` is the dependency-light domain vocabulary; `rb-store` owns one SQLite database (WAL) holding memories + FTS5 + sqlite-vec vectors written in single transactions, with the embedding dimension enforced fail-closed at init. P0 delivers a synchronous, well-tested storage layer; the single-writer daemon, embeddings, search ranking, MCP, and CLI arrive in P1+ (outlined at the end).

**Tech Stack:** Rust 2021, rusqlite (bundled SQLite), sqlite-vec, deadpool-sqlite, include_dir, sha2, serde/serde_json, uuid, chrono, thiserror; tempfile for tests. Reference spec: `docs/specs/2026-05-31-rusty-brain-architecture-design.md`.

---

## Part A — Workspace, toolchain & CI

### Task 1: Root workspace manifest

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/Cargo.toml`

- [ ] **Step 1: Write the root `Cargo.toml`.** Create `/Users/bluby/repos/rusty-brain/Cargo.toml` with exactly this content (workspace members are P0-only; `[workspace.dependencies]` and `[workspace.lints]` are shared by every member):

```toml
[workspace]
resolver = "2"
members = ["crates/rb-types", "crates/rb-store"]

[workspace.package]
version = "0.0.1"
edition = "2021"
license = "MIT"
authors = ["Brian Luby"]
repository = "https://github.com/brianluby/rusty-brain"

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

[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"

[workspace.lints.rust]
unsafe_code = "warn"
```

- [ ] **Step 2: Validate the manifest syntax.** The workspace declares two members that do not exist on disk yet. Any cargo command that loads the workspace (`cargo metadata`, `cargo build`, `cargo fmt`) will FAIL with exit 101 ("failed to load manifest for workspace member") until BOTH members exist — that is normal and is why those checks are deferred to Task 6. For now, validate only that the manifest is syntactically valid TOML with the expected top-level tables.
  Run: `python3 -c "import tomllib; d=tomllib.load(open('/Users/bluby/repos/rusty-brain/Cargo.toml','rb')); assert d['workspace']['members']==['crates/rb-types','crates/rb-store']; assert 'workspace.dependencies' in {k:1 for k in ['workspace.dependencies']} or 'dependencies' in d['workspace']; assert d['workspace']['lints']['clippy']['unwrap_used']=='deny'; print('Cargo.toml TOML OK')"`
  Expected: prints `Cargo.toml TOML OK` (exit 0). The manifest parses and has the expected members and lints.

- [ ] **Step 3: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain add Cargo.toml && git -C /Users/bluby/repos/rusty-brain commit -m "chore: add root workspace manifest with shared deps and lints"`
  Expected: one commit created.

---

### Task 2: Pin the toolchain

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/rust-toolchain.toml`

- [ ] **Step 1: Write `rust-toolchain.toml`.** Create `/Users/bluby/repos/rusty-brain/rust-toolchain.toml` so local builds and CI agree on the stable channel and have the components the CI workflow invokes (`rustfmt`, `clippy`):

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

- [ ] **Step 2: Verify the toolchain resolves.** Confirm rustup honors the pin and the stable toolchain is active.
  Run: `cargo --version`
  Expected: prints `cargo 1.x.y ...` (the installed stable; currently 1.95.0). No "toolchain not installed" error.

- [ ] **Step 3: Verify rustfmt and clippy components are present.**
  Run: `cargo fmt --version && cargo clippy --version`
  Expected: both print version strings (e.g. `rustfmt 1.9.0-stable` and `clippy 0.1.95`). No "not found" error.

- [ ] **Step 4: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain add rust-toolchain.toml && git -C /Users/bluby/repos/rusty-brain commit -m "chore: pin stable toolchain with rustfmt and clippy"`
  Expected: one commit created.

---

### Task 3: cargo-deny policy

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/deny.toml`

- [ ] **Step 1: Write `deny.toml`.** Create `/Users/bluby/repos/rusty-brain/deny.toml`. This is the supply-chain policy enforced in CI from commit one (spec §5.7, §15): deny known advisories, deny unknown/copyleft licenses via an allowlist, and warn on multiple-version duplicates. Use the current `cargo-deny` v2 config schema. NOTE: the bans key is `wildcards` (plural) — the singular `wildcard` is rejected by cargo-deny with an `unexpected-keys` error and would break CI.

```toml
# cargo-deny configuration for rusty-brain.
# Enforced in CI from commit one (spec sections 5.7 and 15).

[graph]
all-features = true

[advisories]
# Fail on any security advisory in the dependency graph.
# `version = 2` selects the modern schema where unmaintained/yanked
# default to deny-style behavior via the lint surface below.
version = 2
yanked = "deny"
ignore = []

[licenses]
version = 2
# Sane permissive allowlist covering the P0 dependency closure
# (serde/uuid/chrono/thiserror/rusqlite/sqlite-vec/deadpool/include_dir/sha2/tempfile).
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Zlib",
    "CC0-1.0",
    "MPL-2.0",
]
confidence-threshold = 0.9
# No copyleft-by-default; anything not in `allow` fails the build.

[bans]
# Surface accidental dependency bloat / duplicate trees.
multiple-versions = "warn"
wildcards = "warn"
highlight = "all"
deny = []
skip = []
skip-tree = []

[sources]
# Only crates.io is trusted by default.
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

- [ ] **Step 2: Verify the policy is well-formed.** Run cargo-deny against the workspace if it is installed; otherwise this is verified by CI in Task 4. The `check` subcommand with no argument runs all checks (advisories, bans, licenses, sources); the bans check is what would reject a mis-named `wildcard` key, so run at least `bans`.
  Run: `cargo deny --version >/dev/null 2>&1 && cargo deny --manifest-path /Users/bluby/repos/rusty-brain/Cargo.toml check bans 2>&1 | tail -5 || echo "cargo-deny not installed locally; CI will validate"`
  Expected: either `bans ok` in the summary with no `unexpected-keys` error, or the fallback message `cargo-deny not installed locally; CI will validate`. (verify against installed cargo-deny at execution; if the installed version renames a `[advisories]` or `[bans]` key, adjust to match it.)

- [ ] **Step 3: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain add deny.toml && git -C /Users/bluby/repos/rusty-brain commit -m "chore: add cargo-deny supply-chain policy"`
  Expected: one commit created.

---

### Task 4: CI workflow

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/.github/workflows/ci.yml`

- [ ] **Step 1: Write the CI workflow.** Create `/Users/bluby/repos/rusty-brain/.github/workflows/ci.yml`. It runs format check, clippy (deny warnings), tests, `cargo-deny check`, and `cargo-audit` (spec §15). The toolchain is taken from `rust-toolchain.toml` (committed in Task 2), so the workflow does not pin a separate channel:

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
      - name: Check formatting
        run: cargo fmt --all --check

  clippy-test:
    name: clippy + test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: Swatinem/rust-cache@v2
      - name: Clippy
        run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - name: Test
        run: cargo test --workspace

  deny:
    name: cargo-deny
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install cargo-deny
        run: cargo install --locked cargo-deny
      - name: cargo-deny check
        run: cargo deny check

  audit:
    name: cargo-audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install cargo-audit
        run: cargo install --locked cargo-audit
      - name: cargo-audit
        run: cargo audit
```

- [ ] **Step 2: Verify the workflow is valid YAML.** Confirm the file parses as YAML (CI itself runs on push; local validation just guards syntax).
  Run: `python3 -c "import yaml,sys; yaml.safe_load(open('/Users/bluby/repos/rusty-brain/.github/workflows/ci.yml')); print('YAML OK')"`
  Expected: prints `YAML OK`.

- [ ] **Step 3: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain add .github/workflows/ci.yml && git -C /Users/bluby/repos/rusty-brain commit -m "ci: add fmt/clippy/test/deny/audit workflow"`
  Expected: one commit created.

---

### Task 5: rb-types crate skeleton

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-types/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-types/src/lib.rs`

> Note: until Task 6 creates the `rb-store` member on disk, NO cargo command that loads the workspace (`cargo build`, `cargo clippy`, `cargo fmt`) can succeed — cargo fails with exit 101 ("failed to load manifest for workspace member") whenever a declared member is missing, and `-p rb-types` does not bypass this. Therefore this task validates the new files by syntax only and defers all compilation/clippy/fmt to Task 6's full-workspace gate.

- [ ] **Step 1: Write the `rb-types` manifest.** Create `/Users/bluby/repos/rusty-brain/crates/rb-types/Cargo.toml`. Package name uses a hyphen (`rb-types`); the library crate name uses an underscore (`rb_types`) via `[lib] name`. It consumes the shared deps and lints from the workspace. (Only the deps `rb-types` actually needs per spine §7: serde, serde_json, uuid, chrono, thiserror.)

```toml
[package]
name = "rb-types"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Domain vocabulary for rusty-brain: ids, namespaces, memory notes, errors."

[lib]
name = "rb_types"
path = "src/lib.rs"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }
chrono = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Write an empty `lib.rs`.** Create `/Users/bluby/repos/rusty-brain/crates/rb-types/src/lib.rs` with a crate-level doc comment and the test-only clippy allow so future test modules can use `unwrap`/`expect` despite the workspace `deny` lints (spine note on test code):

```rust
//! `rb_types`: pure domain vocabulary for rusty-brain.
//!
//! Leaf crate with no internal dependencies. Public types are added in
//! subsequent tasks (`MemoryId`, `Namespace`, `MemoryNote`, etc.).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
```

- [ ] **Step 3: Validate the manifest syntax.** A full `cargo build` cannot run yet (rb-store member still missing). Validate the manifest is well-formed TOML with the expected package/lib/lints wiring.
  Run: `python3 -c "import tomllib; d=tomllib.load(open('/Users/bluby/repos/rusty-brain/crates/rb-types/Cargo.toml','rb')); assert d['package']['name']=='rb-types'; assert d['lib']['name']=='rb_types'; assert d['lints']['workspace'] is True; print('rb-types Cargo.toml OK')"`
  Expected: prints `rb-types Cargo.toml OK` (exit 0).

- [ ] **Step 4: Validate `lib.rs` is present and non-empty.** Confirm the source file exists with the crate doc and the test-only allow attribute.
  Run: `grep -q "cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))" /Users/bluby/repos/rusty-brain/crates/rb-types/src/lib.rs && echo "rb-types lib.rs OK"`
  Expected: prints `rb-types lib.rs OK` (exit 0). Compilation, clippy, and fmt are verified in Task 6 once both members exist.

- [ ] **Step 5: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain add crates/rb-types && git -C /Users/bluby/repos/rusty-brain commit -m "feat: add rb-types crate skeleton"`
  Expected: one commit created.

---

### Task 6: rb-store crate skeleton

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-store/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-store/src/lib.rs`

> This is the first task where BOTH workspace members exist on disk, so it is the first point at which `cargo build --workspace`, `cargo clippy --workspace`, and `cargo fmt --all` can succeed. These gates here also retroactively verify the rb-types skeleton from Task 5.

- [ ] **Step 1: Write the `rb-store` manifest.** Create `/Users/bluby/repos/rusty-brain/crates/rb-store/Cargo.toml`. Package name `rb-store`, library name `rb_store`. It depends on `rb-types` (by path) plus the storage stack from the spine §7: rusqlite (bundled), sqlite-vec, deadpool-sqlite, include_dir, sha2; dev-dep tempfile.

```toml
[package]
name = "rb-store"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "SQLite + sqlite-vec storage engine for rusty-brain: schema, migrations, FTS, vector KNN, graph."

[lib]
name = "rb_store"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rusqlite = { workspace = true }
sqlite-vec = { workspace = true }
deadpool-sqlite = { workspace = true }
include_dir = { workspace = true }
sha2 = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Write an empty `lib.rs`.** Create `/Users/bluby/repos/rusty-brain/crates/rb-store/src/lib.rs` with a doc comment plus the test-only clippy allow (re-exporting `rb_types` is not needed yet):

```rust
//! `rb_store`: SQLite + sqlite-vec storage engine for rusty-brain.
//!
//! Provides the `Store` trait and `SqliteStore` implementation (added in
//! subsequent tasks): one database, one transaction, file-discovered
//! checksummed migrations, FTS5 keyword search, sqlite-vec KNN, and a
//! recursive-CTE graph walk.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
```

- [ ] **Step 3: Verify the whole workspace builds.** Both members now exist on disk, so a full workspace build should resolve and compile cleanly (this also pulls and links the bundled SQLite C source, which can take a minute on a cold build).
  Run: `cargo build --workspace --manifest-path /Users/bluby/repos/rusty-brain/Cargo.toml`
  Expected: `Compiling rb-types v0.0.1 ...`, `Compiling rb-store v0.0.1 ...`, then `Finished` (exit 0). No "member not found" errors remain.

- [ ] **Step 4: Verify clippy passes workspace-wide with warnings denied.** This proves the shared `[workspace.lints]` are wired into both crates via `[lints] workspace = true`. IMPORTANT: `--manifest-path` must come BEFORE the `--` separator — anything after `--` is forwarded to clippy-driver/rustc, which rejects `--manifest-path` with "Unrecognized option".
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain/Cargo.toml -- -D warnings`
  Expected: `Finished` with no warnings/errors (exit 0).

- [ ] **Step 5: Verify formatting is clean.**
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain/Cargo.toml`
  Expected: no output, exit 0 (already formatted).

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain add crates/rb-store && git -C /Users/bluby/repos/rusty-brain commit -m "feat: add rb-store crate skeleton"`
  Expected: one commit created; `cargo build --workspace` and `cargo clippy --workspace` both green.

## Part B — rb-types (domain vocabulary)

### Task 7: rb-types `error.rs` — domain error enum + Result alias

**Files:**
- Create: `crates/rb-types/src/error.rs`
- Create: `crates/rb-types/src/memory_id.rs` (temporary stub; fully built in Task 8)
- Modify: `crates/rb-types/src/lib.rs` (add `mod error;` + `mod memory_id;` + re-exports)

- [ ] **Step 1: Write the failing test AND wire the module so it actually compiles.** A `.rs` file that is not declared with `mod` in `lib.rs` is never compiled — `cargo test` would silently ignore it and report 0 tests instead of the compile failure we want. So we must (a) create `error.rs` with ONLY the test module, (b) create a minimal `memory_id` stub it can reference, and (c) declare both modules in `lib.rs`. The `Error` type is intentionally still missing, so the build fails.

  Create `crates/rb-types/src/error.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory_id::MemoryId;

    #[test]
    fn display_messages_match_spine() {
        assert_eq!(
            Error::Storage("disk".into()).to_string(),
            "storage error: disk"
        );
        assert_eq!(
            Error::Migration("bad".into()).to_string(),
            "migration error: bad"
        );
        assert_eq!(
            Error::InvalidNamespace("x".into()).to_string(),
            "invalid namespace: x"
        );
        assert_eq!(
            Error::InvalidMemoryType("zz".into()).to_string(),
            "invalid memory type: zz"
        );
        assert_eq!(
            Error::InvalidLinkType("qq".into()).to_string(),
            "invalid link type: qq"
        );
        assert_eq!(
            Error::Serialization("json".into()).to_string(),
            "serialization error: json"
        );
        assert_eq!(Error::Io("eof".into()).to_string(), "io error: eof");
    }

    #[test]
    fn dimension_mismatch_message() {
        let e = Error::DimensionMismatch { expected: 1024, got: 768 };
        assert_eq!(
            e.to_string(),
            "embedding dimension mismatch: expected 1024, got 768"
        );
    }

    #[test]
    fn not_found_message_uses_memory_id_display() {
        let id = MemoryId::new();
        let e = Error::NotFound(id.clone());
        assert_eq!(e.to_string(), format!("memory not found: {id}"));
    }

    #[test]
    fn result_alias_resolves() {
        let ok: Result<u8> = Ok(7);
        assert_eq!(ok.unwrap(), 7);
    }
}
```

  Create the temporary stub `crates/rb-types/src/memory_id.rs` (it will be fully built + tested in Task 8, but `Error::NotFound(MemoryId)` needs the type to exist now):

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(uuid::Uuid);

impl MemoryId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

  Set `crates/rb-types/src/lib.rs` to declare both modules and re-export `MemoryId` (the `error` re-exports come in Step 4, once the types exist):

```rust
//! `rb-types` — pure domain vocabulary for rusty-brain.

mod error;
mod memory_id;

pub use memory_id::MemoryId;
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-types error` Expected: FAIL to compile — `cannot find type 'Error' in this scope` (the `error` module is now compiled, but the `Error` type and `Result` alias do not exist yet). This confirms the test drives new code rather than being silently skipped.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/error.rs`:

```rust
use crate::memory_id::MemoryId;

/// Domain error type for rusty-brain. All library crates return `Result<T, Error>`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("memory not found: {0}")]
    NotFound(MemoryId),
    #[error("invalid namespace: {0}")]
    InvalidNamespace(String),
    #[error("invalid memory type: {0}")]
    InvalidMemoryType(String),
    #[error("invalid link type: {0}")]
    InvalidLinkType(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },
    #[error("io error: {0}")]
    Io(String),
}

/// Convenience alias used throughout rusty-brain.
pub type Result<T> = std::result::Result<T, Error>;
```

- [ ] **Step 4: Re-export `Error` and `Result` from `lib.rs`.** Set `crates/rb-types/src/lib.rs` to:

```rust
//! `rb-types` — pure domain vocabulary for rusty-brain.

mod error;
mod memory_id;

pub use error::{Error, Result};
pub use memory_id::MemoryId;
```

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types error` Expected: PASS (4 tests in the `error` module pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/error.rs crates/rb-types/src/memory_id.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add domain Error enum and Result alias"`

---

### Task 8: rb-types `memory_id.rs` — `MemoryId` newtype with `FromStr` + serde round-trip

**Files:**
- Modify: `crates/rb-types/src/memory_id.rs` (replace stub with full impl + tests)

- [ ] **Step 1: Write the failing test.** Append a test module to `crates/rb-types/src/memory_id.rs` that exercises `FromStr`, `Display`, `as_uuid`, default, and serde. The `memory_id` module is already declared in `lib.rs` (from Task 7), so this test compiles and fails because `FromStr` is not yet implemented:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;
    use std::str::FromStr;

    #[test]
    fn new_ids_are_unique() {
        assert_ne!(MemoryId::new(), MemoryId::new());
    }

    #[test]
    fn default_equals_new_shape() {
        let id = MemoryId::default();
        // round-trips through its own string form
        let parsed = MemoryId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn display_and_fromstr_round_trip() {
        let id = MemoryId::new();
        let s = id.to_string();
        let back = MemoryId::from_str(&s).unwrap();
        assert_eq!(id, back);
        assert_eq!(back.as_uuid(), id.as_uuid());
    }

    #[test]
    fn fromstr_rejects_bad_uuid() {
        let err = MemoryId::from_str("not-a-uuid").unwrap_err();
        assert!(matches!(err, Error::Storage(_)));
    }

    #[test]
    fn serde_json_round_trip_is_plain_uuid_string() {
        let id = MemoryId::new();
        let json = serde_json::to_string(&id).unwrap();
        // serde derive on a single-field tuple struct serializes as the inner value
        assert_eq!(json, format!("\"{}\"", id.as_uuid()));
        let back: MemoryId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types memory_id` Expected: FAIL to compile (`the trait bound 'MemoryId: FromStr' is not satisfied` / `no method from_str`), confirming the test drives the new impl.

- [ ] **Step 3: Replace the stub with the full implementation.** Set `crates/rb-types/src/memory_id.rs` (above the test module) to:

```rust
use crate::error::Error;
use serde::{Deserialize, Serialize};

/// Stable, unique identifier for a single `MemoryNote`. Wraps a v4 UUID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(uuid::Uuid);

impl MemoryId {
    /// Generate a fresh random identifier.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Return the underlying UUID by value.
    pub fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for MemoryId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(s)
            .map_err(|e| Error::Storage(format!("invalid memory id '{s}': {e}")))?;
        Ok(Self(uuid))
    }
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-types memory_id` Expected: PASS (5 tests pass).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-types/src/memory_id.rs && git commit -m "feat(rb-types): implement MemoryId FromStr, Display, and serde round-trip"`

---

### Task 9: rb-types `namespace.rs` — `Namespace` enum with db-string round-trip + priority

**Files:**
- Create: `crates/rb-types/src/namespace.rs`
- Modify: `crates/rb-types/src/lib.rs` (add `mod namespace;` + re-export)

- [ ] **Step 1: Write the failing test AND declare the module.** A `.rs` file not declared with `mod` is never compiled, so we declare `mod namespace;` in `lib.rs` now and add the test-only `namespace.rs`; the build then fails because `Namespace` does not exist yet.

  Create `crates/rb-types/src/namespace.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;

    #[test]
    fn priority_orders_session_project_global() {
        assert_eq!(Namespace::Global.priority(), 2);
        assert_eq!(Namespace::Project("p".into()).priority(), 1);
        assert_eq!(
            Namespace::Session {
                project: "p".into(),
                session_id: "s".into(),
            }
            .priority(),
            0
        );
        // session is highest priority (smallest number)
        let mut ns = vec![
            Namespace::Global,
            Namespace::Session {
                project: "p".into(),
                session_id: "s".into(),
            },
            Namespace::Project("p".into()),
        ];
        ns.sort_by_key(|n| n.priority());
        assert!(matches!(ns[0], Namespace::Session { .. }));
        assert!(matches!(ns[1], Namespace::Project(_)));
        assert!(matches!(ns[2], Namespace::Global));
    }

    #[test]
    fn db_strings_match_exact_forms() {
        assert_eq!(Namespace::Global.as_db_string(), "global");
        assert_eq!(
            Namespace::Project("rusty-brain".into()).as_db_string(),
            "project:rusty-brain"
        );
        assert_eq!(
            Namespace::Session {
                project: "rusty-brain".into(),
                session_id: "abc123".into(),
            }
            .as_db_string(),
            "session:rusty-brain:abc123"
        );
    }

    #[test]
    fn parse_db_string_round_trips_all_variants() {
        for ns in [
            Namespace::Global,
            Namespace::Project("rusty-brain".into()),
            Namespace::Session {
                project: "rusty-brain".into(),
                session_id: "abc123".into(),
            },
        ] {
            let s = ns.as_db_string();
            let back = Namespace::parse_db_string(&s).unwrap();
            assert_eq!(ns, back);
        }
    }

    #[test]
    fn parse_session_keeps_session_id_with_colons() {
        // session_id may itself contain colons; only the first two colons delimit.
        let ns = Namespace::parse_db_string("session:proj:sid:with:colons").unwrap();
        assert_eq!(
            ns,
            Namespace::Session {
                project: "proj".into(),
                session_id: "sid:with:colons".into(),
            }
        );
    }

    #[test]
    fn parse_rejects_unknown_and_empty() {
        assert!(matches!(
            Namespace::parse_db_string("bogus").unwrap_err(),
            Error::InvalidNamespace(_)
        ));
        assert!(matches!(
            Namespace::parse_db_string("project:").unwrap_err(),
            Error::InvalidNamespace(_)
        ));
        assert!(matches!(
            Namespace::parse_db_string("session:onlyproject").unwrap_err(),
            Error::InvalidNamespace(_)
        ));
    }

    #[test]
    fn serde_json_round_trip() {
        let ns = Namespace::Session {
            project: "p".into(),
            session_id: "s".into(),
        };
        let json = serde_json::to_string(&ns).unwrap();
        let back: Namespace = serde_json::from_str(&json).unwrap();
        assert_eq!(ns, back);
    }
}
```

  Add `mod namespace;` after `mod memory_id;` in `crates/rb-types/src/lib.rs` (the `pub use` for `Namespace` is added in Step 3, once the type exists).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types namespace` Expected: FAIL to compile (`cannot find type 'Namespace' in this scope`), confirming the module is compiled and the type is missing.

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/namespace.rs`:

```rust
use crate::error::Error;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Scope a memory belongs to. DB forms (exact):
/// `global` | `project:{name}` | `session:{project}:{session_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Namespace {
    Global,
    Project(String),
    Session { project: String, session_id: String },
}

impl Namespace {
    /// Lower value = narrower/higher precedence. Session=0, Project=1, Global=2.
    pub fn priority(&self) -> u8 {
        match self {
            Namespace::Session { .. } => 0,
            Namespace::Project(_) => 1,
            Namespace::Global => 2,
        }
    }

    /// Serialize to the exact string stored in the `namespace` column.
    pub fn as_db_string(&self) -> String {
        match self {
            Namespace::Global => "global".to_string(),
            Namespace::Project(name) => format!("project:{name}"),
            Namespace::Session {
                project,
                session_id,
            } => format!("session:{project}:{session_id}"),
        }
    }

    /// Parse a db string back into a `Namespace`. Fail closed on anything unrecognized.
    pub fn parse_db_string(s: &str) -> Result<Self> {
        if s == "global" {
            return Ok(Namespace::Global);
        }
        if let Some(name) = s.strip_prefix("project:") {
            if name.is_empty() {
                return Err(Error::InvalidNamespace(s.to_string()));
            }
            return Ok(Namespace::Project(name.to_string()));
        }
        if let Some(rest) = s.strip_prefix("session:") {
            // Split into project and session_id on the FIRST colon only;
            // session_id may itself contain colons.
            if let Some((project, session_id)) = rest.split_once(':') {
                if !project.is_empty() && !session_id.is_empty() {
                    return Ok(Namespace::Session {
                        project: project.to_string(),
                        session_id: session_id.to_string(),
                    });
                }
            }
            return Err(Error::InvalidNamespace(s.to_string()));
        }
        Err(Error::InvalidNamespace(s.to_string()))
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Add `pub use namespace::Namespace;` to the re-export block in `crates/rb-types/src/lib.rs`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types namespace` Expected: PASS (6 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/namespace.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add Namespace with db-string round-trip and priority"`

---

### Task 10: rb-types `memory_type.rs` — `MemoryType` enum matching exact SQL CHECK strings

**Files:**
- Create: `crates/rb-types/src/memory_type.rs`
- Modify: `crates/rb-types/src/lib.rs` (add `mod memory_type;` + re-export)

- [ ] **Step 1: Write the failing test AND declare the module.** Declare `mod memory_type;` in `lib.rs` now (an undeclared `.rs` file is never compiled) and add the test-only `memory_type.rs`. The `as_str` values MUST match the SQL CHECK in spec §9 exactly:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;

    const ALL: [MemoryType; 9] = [
        MemoryType::ArchitectureDecision,
        MemoryType::CodePattern,
        MemoryType::BugFix,
        MemoryType::Configuration,
        MemoryType::Constraint,
        MemoryType::Entity,
        MemoryType::Insight,
        MemoryType::Reference,
        MemoryType::Preference,
    ];

    #[test]
    fn as_str_matches_sql_check_values() {
        assert_eq!(
            MemoryType::ArchitectureDecision.as_str(),
            "architecture_decision"
        );
        assert_eq!(MemoryType::CodePattern.as_str(), "code_pattern");
        assert_eq!(MemoryType::BugFix.as_str(), "bug_fix");
        assert_eq!(MemoryType::Configuration.as_str(), "configuration");
        assert_eq!(MemoryType::Constraint.as_str(), "constraint");
        assert_eq!(MemoryType::Entity.as_str(), "entity");
        assert_eq!(MemoryType::Insight.as_str(), "insight");
        assert_eq!(MemoryType::Reference.as_str(), "reference");
        assert_eq!(MemoryType::Preference.as_str(), "preference");
    }

    #[test]
    fn parse_is_inverse_of_as_str_for_all_variants() {
        for mt in ALL {
            assert_eq!(MemoryType::parse(mt.as_str()).unwrap(), mt);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = MemoryType::parse("nonsense").unwrap_err();
        assert!(matches!(err, Error::InvalidMemoryType(_)));
    }

    #[test]
    fn serde_json_round_trip() {
        let mt = MemoryType::BugFix;
        let json = serde_json::to_string(&mt).unwrap();
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(mt, back);
    }
}
```

  Add `mod memory_type;` to `crates/rb-types/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types memory_type` Expected: FAIL to compile (`cannot find type 'MemoryType' in this scope`).

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/memory_type.rs`:

```rust
use crate::error::Error;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Category of a memory. `as_str` values match the `memory_type` SQL CHECK exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryType {
    ArchitectureDecision,
    CodePattern,
    BugFix,
    Configuration,
    Constraint,
    Entity,
    Insight,
    Reference,
    Preference,
}

impl MemoryType {
    /// Stable db string. MUST stay in lockstep with the SQL CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::ArchitectureDecision => "architecture_decision",
            MemoryType::CodePattern => "code_pattern",
            MemoryType::BugFix => "bug_fix",
            MemoryType::Configuration => "configuration",
            MemoryType::Constraint => "constraint",
            MemoryType::Entity => "entity",
            MemoryType::Insight => "insight",
            MemoryType::Reference => "reference",
            MemoryType::Preference => "preference",
        }
    }

    /// Parse a db string into a `MemoryType`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "architecture_decision" => Ok(MemoryType::ArchitectureDecision),
            "code_pattern" => Ok(MemoryType::CodePattern),
            "bug_fix" => Ok(MemoryType::BugFix),
            "configuration" => Ok(MemoryType::Configuration),
            "constraint" => Ok(MemoryType::Constraint),
            "entity" => Ok(MemoryType::Entity),
            "insight" => Ok(MemoryType::Insight),
            "reference" => Ok(MemoryType::Reference),
            "preference" => Ok(MemoryType::Preference),
            other => Err(Error::InvalidMemoryType(other.to_string())),
        }
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Add `pub use memory_type::MemoryType;` to `crates/rb-types/src/lib.rs`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types memory_type` Expected: PASS (4 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/memory_type.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add MemoryType with SQL-CHECK-matched as_str/parse"`

---

### Task 11: rb-types `link_type.rs` — `LinkType` enum matching exact SQL CHECK strings

**Files:**
- Create: `crates/rb-types/src/link_type.rs`
- Modify: `crates/rb-types/src/lib.rs` (add `mod link_type;` + re-export)

- [ ] **Step 1: Write the failing test AND declare the module.** Declare `mod link_type;` in `lib.rs` now (an undeclared `.rs` file is never compiled) and add the test-only `link_type.rs`. `as_str` values MUST match the `memory_links` SQL CHECK in spec §9:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;

    const ALL: [LinkType; 5] = [
        LinkType::Extends,
        LinkType::Contradicts,
        LinkType::Implements,
        LinkType::References,
        LinkType::Supersedes,
    ];

    #[test]
    fn as_str_matches_sql_check_values() {
        assert_eq!(LinkType::Extends.as_str(), "extends");
        assert_eq!(LinkType::Contradicts.as_str(), "contradicts");
        assert_eq!(LinkType::Implements.as_str(), "implements");
        assert_eq!(LinkType::References.as_str(), "references");
        assert_eq!(LinkType::Supersedes.as_str(), "supersedes");
    }

    #[test]
    fn parse_is_inverse_of_as_str_for_all_variants() {
        for lt in ALL {
            assert_eq!(LinkType::parse(lt.as_str()).unwrap(), lt);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = LinkType::parse("depends_on").unwrap_err();
        assert!(matches!(err, Error::InvalidLinkType(_)));
    }

    #[test]
    fn serde_json_round_trip() {
        let lt = LinkType::Supersedes;
        let json = serde_json::to_string(&lt).unwrap();
        let back: LinkType = serde_json::from_str(&json).unwrap();
        assert_eq!(lt, back);
    }
}
```

  Add `mod link_type;` to `crates/rb-types/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types link_type` Expected: FAIL to compile (`cannot find type 'LinkType' in this scope`).

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/link_type.rs`:

```rust
use crate::error::Error;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Relationship between two memories. `as_str` values match the SQL CHECK exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkType {
    Extends,
    Contradicts,
    Implements,
    References,
    Supersedes,
}

impl LinkType {
    /// Stable db string. MUST stay in lockstep with the SQL CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkType::Extends => "extends",
            LinkType::Contradicts => "contradicts",
            LinkType::Implements => "implements",
            LinkType::References => "references",
            LinkType::Supersedes => "supersedes",
        }
    }

    /// Parse a db string into a `LinkType`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "extends" => Ok(LinkType::Extends),
            "contradicts" => Ok(LinkType::Contradicts),
            "implements" => Ok(LinkType::Implements),
            "references" => Ok(LinkType::References),
            "supersedes" => Ok(LinkType::Supersedes),
            other => Err(Error::InvalidLinkType(other.to_string())),
        }
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Add `pub use link_type::LinkType;` to `crates/rb-types/src/lib.rs`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types link_type` Expected: PASS (4 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/link_type.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add LinkType with SQL-CHECK-matched as_str/parse"`

---

### Task 12: rb-types `link.rs` — `MemoryLink` struct + serde round-trip

**Files:**
- Create: `crates/rb-types/src/link.rs`
- Modify: `crates/rb-types/src/lib.rs` (add `mod link;` + re-export)

- [ ] **Step 1: Write the failing test AND declare the module.** Declare `mod link;` in `lib.rs` now (an undeclared `.rs` file is never compiled) and add the test-only `link.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::link_type::LinkType;
    use crate::memory_id::MemoryId;
    use chrono::Utc;

    fn sample() -> MemoryLink {
        MemoryLink {
            source_id: MemoryId::new(),
            target_id: MemoryId::new(),
            link_type: LinkType::Extends,
            strength: 0.75,
            reason: "builds on prior decision".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn fields_are_accessible() {
        let link = sample();
        assert_eq!(link.link_type, LinkType::Extends);
        assert_eq!(link.reason, "builds on prior decision");
        assert!((link.strength - 0.75).abs() < f32::EPSILON);
        assert_ne!(link.source_id, link.target_id);
    }

    #[test]
    fn serde_json_round_trip_preserves_all_fields() {
        let link = sample();
        let json = serde_json::to_string(&link).unwrap();
        let back: MemoryLink = serde_json::from_str(&json).unwrap();
        assert_eq!(link, back);
    }

    #[test]
    fn clone_equals_original() {
        let link = sample();
        assert_eq!(link.clone(), link);
    }
}
```

  Add `mod link;` to `crates/rb-types/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types link::` Expected: FAIL to compile (`cannot find type 'MemoryLink' in this scope`). The `::` suffix scopes the filter to the `link` module only (it does not match `link_type::tests`).

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/link.rs`:

```rust
use crate::link_type::LinkType;
use crate::memory_id::MemoryId;
use serde::{Deserialize, Serialize};

/// A directed, typed relationship between two memories with a confidence/strength.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryLink {
    pub source_id: MemoryId,
    pub target_id: MemoryId,
    pub link_type: LinkType,
    pub strength: f32,
    pub reason: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Add `pub use link::MemoryLink;` to `crates/rb-types/src/lib.rs`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types link::` Expected: PASS (3 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/link.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add MemoryLink struct with serde round-trip"`

---

### Task 13: rb-types `memory.rs` — `MemoryNote` struct + `new()` defaults

**Files:**
- Create: `crates/rb-types/src/memory.rs`
- Modify: `crates/rb-types/src/lib.rs` (add `mod memory;` + re-export)

- [ ] **Step 1: Write the failing test AND declare the module.** Declare `mod memory;` in `lib.rs` now (an undeclared `.rs` file is never compiled) and add the test-only `memory.rs`. It pins every default from the spine's `new()` contract:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory_type::MemoryType;
    use crate::namespace::Namespace;

    fn sample() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "always use one DB and one transaction".to_string(),
            MemoryType::ArchitectureDecision,
            8,
        )
    }

    #[test]
    fn new_sets_constructor_args() {
        let m = sample();
        assert_eq!(m.namespace, Namespace::Project("rusty-brain".into()));
        assert_eq!(m.content, "always use one DB and one transaction");
        assert_eq!(m.memory_type, MemoryType::ArchitectureDecision);
        assert_eq!(m.importance, 8);
    }

    #[test]
    fn new_applies_spine_defaults() {
        let m = sample();
        assert_eq!(m.summary, "");
        assert_eq!(m.context, "");
        assert!((m.confidence - 1.0).abs() < f32::EPSILON);
        assert_eq!(m.access_count, 0);
        assert_eq!(m.embedding_model, "");
        assert!(m.keywords.is_empty());
        assert!(m.tags.is_empty());
        assert!(m.related_files.is_empty());
        assert!(m.links.is_empty());
        assert!(m.last_accessed_at.is_none());
        assert!(m.archived_at.is_none());
        assert!(m.superseded_by.is_none());
    }

    #[test]
    fn new_sets_created_and_updated_equal() {
        let m = sample();
        assert_eq!(m.created_at, m.updated_at);
    }

    #[test]
    fn new_generates_unique_ids() {
        let a = sample();
        let b = sample();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn serde_json_round_trip_preserves_all_fields() {
        let m = sample();
        let json = serde_json::to_string(&m).unwrap();
        let back: MemoryNote = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
```

  Add `mod memory;` to `crates/rb-types/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types memory::` Expected: FAIL to compile (`cannot find type 'MemoryNote' in this scope`). The `::` suffix scopes the filter to the `memory` module only (it does not match `memory_id::` or `memory_type::`).

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/memory.rs`:

```rust
use crate::link::MemoryLink;
use crate::memory_id::MemoryId;
use crate::memory_type::MemoryType;
use crate::namespace::Namespace;
use serde::{Deserialize, Serialize};

/// A single unit of memory: content plus enrichment, metadata, and links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryNote {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub content: String,
    pub summary: String,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub context: String,
    pub memory_type: MemoryType,
    /// 1-10 (validated at storage boundary).
    pub importance: u8,
    /// 0.0..=1.0 (validated at storage boundary).
    pub confidence: f32,
    pub related_files: Vec<String>,
    pub access_count: u64,
    pub last_accessed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub superseded_by: Option<MemoryId>,
    pub embedding_model: String,
    pub links: Vec<MemoryLink>,
}

impl MemoryNote {
    /// Construct a fresh active memory. Generates an id, sets created_at == updated_at
    /// to now, empties all collections, and applies spine defaults
    /// (summary/context empty, confidence 1.0, access_count 0, embedding_model empty).
    pub fn new(
        namespace: Namespace,
        content: String,
        memory_type: MemoryType,
        importance: u8,
    ) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: MemoryId::new(),
            namespace,
            created_at: now,
            updated_at: now,
            content,
            summary: String::new(),
            keywords: Vec::new(),
            tags: Vec::new(),
            context: String::new(),
            memory_type,
            importance,
            confidence: 1.0,
            related_files: Vec::new(),
            access_count: 0,
            last_accessed_at: None,
            archived_at: None,
            superseded_by: None,
            embedding_model: String::new(),
            links: Vec::new(),
        }
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Add `pub use memory::MemoryNote;` to `crates/rb-types/src/lib.rs`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types memory::` Expected: PASS (5 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/memory.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add MemoryNote struct and new() with spine defaults"`

---

### Task 14: rb-types `query.rs` — `SearchQuery`, `SearchResult`, `MemoryUpdates`

**Files:**
- Create: `crates/rb-types/src/query.rs`
- Modify: `crates/rb-types/src/lib.rs` (add `mod query;` + re-exports)

- [ ] **Step 1: Write the failing test AND declare the module.** Declare `mod query;` in `lib.rs` now (an undeclared `.rs` file is never compiled) and add the test-only `query.rs`. It pins the `Default` derives and serde round-trips:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::memory::MemoryNote;
    use crate::memory_type::MemoryType;
    use crate::namespace::Namespace;

    #[test]
    fn search_query_default_is_empty() {
        let q = SearchQuery::default();
        assert_eq!(q.query, "");
        assert!(q.scope.is_none());
        assert!(q.memory_type.is_none());
        assert!(q.tags.is_empty());
        assert_eq!(q.limit, 0);
    }

    #[test]
    fn search_query_round_trip() {
        let q = SearchQuery {
            query: "transactions".to_string(),
            scope: Some(Namespace::Global),
            memory_type: Some(MemoryType::BugFix),
            tags: vec!["sqlite".to_string()],
            limit: 10,
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: SearchQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(back.query, "transactions");
        assert_eq!(back.scope, Some(Namespace::Global));
        assert_eq!(back.memory_type, Some(MemoryType::BugFix));
        assert_eq!(back.tags, vec!["sqlite".to_string()]);
        assert_eq!(back.limit, 10);
    }

    #[test]
    fn search_result_round_trip() {
        let memory = MemoryNote::new(
            Namespace::Global,
            "content".to_string(),
            MemoryType::Insight,
            5,
        );
        let result = SearchResult {
            memory: memory.clone(),
            score: 0.9,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.memory, memory);
        assert!((back.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_updates_default_is_all_none() {
        let u = MemoryUpdates::default();
        assert!(u.content.is_none());
        assert!(u.summary.is_none());
        assert!(u.importance.is_none());
        assert!(u.tags.is_none());
        assert!(u.context.is_none());
    }

    #[test]
    fn memory_updates_round_trip() {
        let u = MemoryUpdates {
            content: Some("new body".to_string()),
            summary: Some("new summary".to_string()),
            importance: Some(9),
            tags: Some(vec!["x".to_string(), "y".to_string()]),
            context: Some("ctx".to_string()),
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: MemoryUpdates = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content, Some("new body".to_string()));
        assert_eq!(back.summary, Some("new summary".to_string()));
        assert_eq!(back.importance, Some(9));
        assert_eq!(back.tags, Some(vec!["x".to_string(), "y".to_string()]));
        assert_eq!(back.context, Some("ctx".to_string()));
    }
}
```

  Add `mod query;` to `crates/rb-types/src/lib.rs` (the `pub use` is added in Step 3).

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types query` Expected: FAIL to compile (`cannot find type 'SearchQuery' in this scope`).

- [ ] **Step 3: Add the minimal implementation above the test module.** Prepend to `crates/rb-types/src/query.rs`:

```rust
use crate::memory::MemoryNote;
use crate::memory_type::MemoryType;
use crate::namespace::Namespace;
use serde::{Deserialize, Serialize};

/// A hybrid-search request. `Default` yields an empty, unscoped, unlimited query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub scope: Option<Namespace>,
    pub memory_type: Option<MemoryType>,
    pub tags: Vec<String>,
    pub limit: usize,
}

/// A single ranked search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub memory: MemoryNote,
    pub score: f32,
}

/// Partial update for a memory; `None` fields are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryUpdates {
    pub content: Option<String>,
    pub summary: Option<String>,
    pub importance: Option<u8>,
    pub tags: Option<Vec<String>>,
    pub context: Option<String>,
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** Add `pub use query::{MemoryUpdates, SearchQuery, SearchResult};` to `crates/rb-types/src/lib.rs`.

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-types query` Expected: PASS (5 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-types --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-types/src/query.rs crates/rb-types/src/lib.rs && git commit -m "feat(rb-types): add SearchQuery, SearchResult, and MemoryUpdates"`

---

### Task 15: rb-types `lib.rs` — finalize public re-exports + crate-wide public-API guard test

**Files:**
- Modify: `crates/rb-types/src/lib.rs` (finalize module list + flat re-exports, add crate doc)
- Create: `crates/rb-types/tests/public_api.rs` (integration test that guards the public surface)

- [ ] **Step 1: Write the public-API guard integration test.** Create `crates/rb-types/tests/public_api.rs` that imports every public type via the crate root. Integration tests in `tests/` are compiled as a separate crate target with access to rb-types' normal dependencies, so `chrono` (a normal dependency of rb-types per the spine) is already available — no dev-dependency is required:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_types::{
    Error, LinkType, MemoryId, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    Result, SearchQuery, SearchResult,
};
use std::str::FromStr;

#[test]
fn all_public_types_are_reachable_from_crate_root() {
    // Error + Result
    let r: Result<u8> = Err(Error::Storage("x".into()));
    assert!(r.is_err());

    // MemoryId
    let id = MemoryId::new();
    let id2 = MemoryId::from_str(&id.to_string()).unwrap();
    assert_eq!(id, id2);

    // Namespace
    let ns = Namespace::Project("rusty-brain".into());
    assert_eq!(ns.as_db_string(), "project:rusty-brain");

    // MemoryType + LinkType
    assert_eq!(MemoryType::BugFix.as_str(), "bug_fix");
    assert_eq!(LinkType::Extends.as_str(), "extends");

    // MemoryNote built via the constructor
    let note = MemoryNote::new(ns.clone(), "body".into(), MemoryType::Insight, 5);
    assert_eq!(note.namespace, ns);

    // MemoryLink
    let link = MemoryLink {
        source_id: MemoryId::new(),
        target_id: MemoryId::new(),
        link_type: LinkType::References,
        strength: 0.5,
        reason: "r".into(),
        created_at: chrono::Utc::now(),
    };
    assert_eq!(link.link_type, LinkType::References);

    // SearchQuery / SearchResult / MemoryUpdates
    let q = SearchQuery {
        query: "q".into(),
        limit: 3,
        ..Default::default()
    };
    assert_eq!(q.limit, 3);
    let res = SearchResult { memory: note, score: 1.0 };
    assert!((res.score - 1.0).abs() < f32::EPSILON);
    let upd = MemoryUpdates {
        importance: Some(9),
        ..Default::default()
    };
    assert_eq!(upd.importance, Some(9));
}
```

- [ ] **Step 2: Run it — expect PASS (this test is a regression guard, not a red→green driver).** Run: `cargo test -p rb-types --test public_api` Expected: PASS. By the end of Task 14, `lib.rs` already declares all eight modules and re-exports all eleven public types, so every import in this test resolves. The test exists to LOCK the public surface: if a future change drops or renames a re-export, this integration test fails to compile, catching the regression. (Unlike the per-module unit tests, this one cannot meaningfully fail-then-pass within this task — the surface it guards already exists.)

- [ ] **Step 3: Finalize `lib.rs` with the canonical module list, re-exports, and crate doc.** Set `crates/rb-types/src/lib.rs` to:

```rust
//! `rb-types` — pure domain vocabulary for rusty-brain.
//!
//! Leaf crate: no dependencies on other workspace crates. Defines the shared
//! types (`MemoryNote`, `MemoryId`, `Namespace`, `MemoryType`, `LinkType`,
//! `MemoryLink`, `SearchQuery`, `SearchResult`, `MemoryUpdates`, `Error`) used
//! across the engine, store, daemon, and binary.

mod error;
mod link;
mod link_type;
mod memory;
mod memory_id;
mod memory_type;
mod namespace;
mod query;

pub use error::{Error, Result};
pub use link::MemoryLink;
pub use link_type::LinkType;
pub use memory::MemoryNote;
pub use memory_id::MemoryId;
pub use memory_type::MemoryType;
pub use namespace::Namespace;
pub use query::{MemoryUpdates, SearchQuery, SearchResult};
```

- [ ] **Step 4: Run the full crate test suite — expect PASS.** Run: `cargo test -p rb-types` Expected: PASS (all unit tests across every module plus the `public_api` integration test). No `Cargo.toml` change is needed: `chrono`, `serde`, `serde_json`, `uuid`, and `thiserror` are already normal dependencies of rb-types and are therefore available to the integration test target.

- [ ] **Step 5: Lint + format across the workspace.** Run: `cargo clippy --workspace --all-targets -- -D warnings` Expected: no warnings (confirms the deny-`unwrap_used`/`expect_used` lints pass; test modules opted out via per-module allow). Run: `cargo fmt --all` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-types/src/lib.rs crates/rb-types/tests/public_api.rs && git commit -m "feat(rb-types): finalize public re-exports with public-API guard test"`

## Part C — rb-store: migrations, schema & open()

### Task 16: rb-store crate skeleton + migration runner (`run_migrations`)

**Files:**
- Create: `crates/rb-store/Cargo.toml`
- Create: `crates/rb-store/src/lib.rs`
- Create: `crates/rb-store/src/error.rs`
- Create: `crates/rb-store/src/migrations.rs`
- Create: `crates/rb-store/migrations/.keep`
- Test: `crates/rb-store/src/migrations.rs` (inline `#[cfg(test)] mod tests`)

This task stands up the `rb-store` crate and implements the file-discovered, checksummed, transactional migration runner. The actual DDL file lands in Task 17; here we use a tiny throwaway migration directory (`.keep` placeholder) plus an in-test fixture connection so the runner logic is provable on its own. `error.rs` only defines the helper actually used by the runner (`migration_err`); the `storage_err`/`io_err` helpers are added in Task 18 when the store first needs them, so that `cargo clippy -- -D warnings` (which denies `dead_code`) stays green at every commit.

- [ ] **Step 1: Create the crate manifest.** Write `crates/rb-store/Cargo.toml`:

  ```toml
  [package]
  name = "rb-store"
  version.workspace = true
  edition.workspace = true
  license.workspace = true

  [lib]
  name = "rb_store"

  [dependencies]
  rb-types = { path = "../rb-types" }
  rusqlite = { workspace = true }
  sqlite-vec = { workspace = true }
  deadpool-sqlite = { workspace = true }
  include_dir = { workspace = true }
  sha2 = { workspace = true }

  [dev-dependencies]
  tempfile = { workspace = true }

  [lints]
  workspace = true
  ```

- [ ] **Step 2: Add the crate to the workspace members.** Confirm `crates/rb-store` is listed under `[workspace] members` in the root `Cargo.toml` (added in the P0 workspace task). Run: `cargo metadata --no-deps --format-version 1 --manifest-path /Users/bluby/repos/rusty-brain/Cargo.toml | grep -o '"name":"rb-store"'` Expected: prints `"name":"rb-store"`.

- [ ] **Step 3: Create the error-mapping helper.** Write `crates/rb-store/src/error.rs` with only the helper the migration runner uses; `storage_err`/`io_err` are added in Task 18 when the store first needs them (defining them now would be `dead_code` and fail `clippy -- -D warnings`):

  ```rust
  //! Internal conversions from foreign error types into `rb_types::Error`.

  use rb_types::Error;

  /// Map a `rusqlite::Error` encountered during migration to a migration error.
  pub(crate) fn migration_err(e: rusqlite::Error) -> Error {
      Error::Migration(e.to_string())
  }
  ```

- [ ] **Step 4: Create the library root.** Write `crates/rb-store/src/lib.rs`:

  ```rust
  //! `rb-store`: SQLite + sqlite-vec storage engine for rusty-brain.
  //!
  //! One database file holds memories, FTS index, vectors and links so that a
  //! `remember` is a single transaction (no dual-DB desync). The embedding
  //! dimension is a single configured value, enforced fail-closed at `open`.
  #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

  mod error;
  mod migrations;

  pub use migrations::run_migrations;
  ```

- [ ] **Step 5: Create the migrations directory placeholder.** Write `crates/rb-store/migrations/.keep` with the single line:

  ```
  migration SQL files (NNN_*.sql) live here; discovered at compile time via include_dir
  ```

  (The real `001_initial_schema.sql` is added in Task 17. `include_dir!` requires the directory to exist at compile time; with only `.keep` present, `discover()` simply finds zero `.sql` files.)

- [ ] **Step 6: Write the failing tests FIRST.** Write `crates/rb-store/src/migrations.rs` with the checksum helper, the `Migration` type, an `unimplemented!` `run_migrations`, and the complete test module. The tests reference `ensure_migrations_table`/`apply_all` (added in Step 8), so this will NOT compile yet — the intended RED state. Put this complete content in the file:

  ```rust
  //! File-discovered, checksummed, transactional migration runner.

  use crate::error::migration_err;
  use include_dir::{include_dir, Dir};
  use rb_types::{Error, Result};
  use sha2::{Digest, Sha256};

  /// Migrations embedded from `crates/rb-store/migrations` at compile time.
  static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

  /// A single migration: numeric version, file name, SQL body.
  struct Migration {
      version: i64,
      name: String,
      sql: String,
  }

  /// Hex-encoded sha256 of the SQL body.
  fn checksum(sql: &str) -> String {
      let mut hasher = Sha256::new();
      hasher.update(sql.as_bytes());
      let digest = hasher.finalize();
      let mut out = String::with_capacity(digest.len() * 2);
      for byte in digest {
          out.push_str(&format!("{byte:02x}"));
      }
      out
  }

  /// Run all pending migrations against `conn`, transactionally and in order.
  ///
  /// - Creates `_migrations` if absent.
  /// - Discovers `NNN_*.sql` files, orders by the numeric prefix.
  /// - Applies each unseen version inside its own transaction, recording the
  ///   sha256 checksum.
  /// - Re-applying an already-recorded version is a no-op.
  /// - A checksum change on an already-applied version returns `Error::Migration`.
  pub fn run_migrations(_conn: &rusqlite::Connection) -> Result<()> {
      unimplemented!("implemented in Step 8")
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      // A minimal, self-contained migration set so the runner logic is provable
      // without depending on the production DDL (added in Task 17).
      const M1: &str = "CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT NOT NULL);";

      fn conn() -> rusqlite::Connection {
          rusqlite::Connection::open_in_memory().unwrap()
      }

      // Mirror of run_migrations that takes an explicit migration list, so we can
      // exercise the runner with test fixtures instead of the embedded dir.
      fn apply(c: &rusqlite::Connection, migs: &[Migration]) -> Result<()> {
          ensure_migrations_table(c)?;
          apply_all(c, migs)
      }

      fn mig(version: i64, name: &str, sql: &str) -> Migration {
          Migration { version, name: name.to_string(), sql: sql.to_string() }
      }

      #[test]
      fn applies_a_migration_and_records_it() {
          let c = conn();
          apply(&c, &[mig(1, "001_widgets.sql", M1)]).unwrap();

          // The migration ran: table exists.
          let cnt: i64 = c
              .query_row(
                  "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='widgets'",
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          assert_eq!(cnt, 1, "widgets table should exist");

          // It was recorded exactly once.
          let rows: i64 = c
              .query_row("SELECT count(*) FROM _migrations WHERE version=1", [], |r| r.get(0))
              .unwrap();
          assert_eq!(rows, 1, "version 1 recorded exactly once");
      }

      #[test]
      fn applying_twice_is_a_no_op() {
          let c = conn();
          let migs = [mig(1, "001_widgets.sql", M1)];
          apply(&c, &migs).unwrap();
          // Second run must NOT error and must NOT re-record.
          apply(&c, &migs).unwrap();

          let rows: i64 = c
              .query_row("SELECT count(*) FROM _migrations", [], |r| r.get(0))
              .unwrap();
          assert_eq!(rows, 1, "re-run is a no-op: still exactly one recorded version");
      }

      #[test]
      fn tampered_checksum_errors() {
          let c = conn();
          apply(&c, &[mig(1, "001_widgets.sql", M1)]).unwrap();

          // Same version, different SQL body => checksum mismatch => Error::Migration.
          let tampered = [mig(1, "001_widgets.sql", "CREATE TABLE other (id INTEGER);")];
          let err = apply(&c, &tampered).unwrap_err();
          assert!(
              matches!(err, Error::Migration(_)),
              "checksum mismatch must be Error::Migration, got {err:?}"
          );
      }

      #[test]
      fn checksum_is_stable_and_distinct() {
          assert_eq!(checksum("abc"), checksum("abc"));
          assert_ne!(checksum("abc"), checksum("abd"));
          // sha256("abc") known vector.
          assert_eq!(
              checksum("abc"),
              "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
          );
      }

      #[test]
      fn versions_apply_in_numeric_order() {
          let c = conn();
          let migs = [
              mig(2, "002_b.sql", "CREATE TABLE b (id INTEGER);"),
              mig(1, "001_a.sql", "CREATE TABLE a (id INTEGER);"),
              mig(10, "010_c.sql", "CREATE TABLE c (id INTEGER);"),
          ];
          apply(&c, &migs).unwrap();

          // All three applied: every table exists and the two-digit version was
          // recorded (no silent stop after the single-digit ones).
          let table = |name: &str| -> i64 {
              c.query_row(
                  "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                  rusqlite::params![name],
                  |r| r.get(0),
              )
              .unwrap()
          };
          assert_eq!(table("a"), 1, "version 1 table applied");
          assert_eq!(table("b"), 1, "version 2 table applied");
          assert_eq!(table("c"), 1, "version 10 table applied");

          let recorded: i64 = c
              .query_row("SELECT count(*) FROM _migrations", [], |r| r.get(0))
              .unwrap();
          assert_eq!(recorded, 3, "all three versions recorded");
          let max: i64 = c
              .query_row("SELECT max(version) FROM _migrations", [], |r| r.get(0))
              .unwrap();
          assert_eq!(max, 10, "two-digit version recorded");
      }
  }
  ```

  Note: the test helper references `ensure_migrations_table` and `apply_all` (added in Step 8); this will not compile yet, which is the intended RED state.

- [ ] **Step 7: Run the tests, see them fail to compile.** Run: `cargo test -p rb-store migrations` Expected: FAIL (compile errors: `ensure_migrations_table`/`apply_all` not found, and `run_migrations` is `unimplemented!`).

- [ ] **Step 8: Implement the runner (minimal, makes tests pass).** Replace the whole content of `crates/rb-store/src/migrations.rs` with the discovery + apply helpers and a real `run_migrations`. The file becomes:

  ```rust
  //! File-discovered, checksummed, transactional migration runner.

  use crate::error::migration_err;
  use include_dir::{include_dir, Dir};
  use rb_types::{Error, Result};
  use sha2::{Digest, Sha256};

  /// Migrations embedded from `crates/rb-store/migrations` at compile time.
  static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

  /// A single migration: numeric version, file name, SQL body.
  struct Migration {
      version: i64,
      name: String,
      sql: String,
  }

  /// Hex-encoded sha256 of the SQL body.
  fn checksum(sql: &str) -> String {
      let mut hasher = Sha256::new();
      hasher.update(sql.as_bytes());
      let digest = hasher.finalize();
      let mut out = String::with_capacity(digest.len() * 2);
      for byte in digest {
          out.push_str(&format!("{byte:02x}"));
      }
      out
  }

  /// Discover `NNN_*.sql` migration files from the embedded directory,
  /// ordered ascending by their numeric prefix.
  fn discover() -> Result<Vec<Migration>> {
      let mut migs: Vec<Migration> = Vec::new();
      for file in MIGRATIONS_DIR.files() {
          let name = match file.path().file_name().and_then(|n| n.to_str()) {
              Some(n) => n.to_string(),
              None => continue,
          };
          if !name.ends_with(".sql") {
              continue;
          }
          let prefix: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
          if prefix.is_empty() {
              return Err(Error::Migration(format!(
                  "migration file has no numeric prefix: {name}"
              )));
          }
          let version: i64 = prefix.parse().map_err(|_| {
              Error::Migration(format!("invalid numeric prefix in {name}"))
          })?;
          let sql = file
              .contents_utf8()
              .ok_or_else(|| Error::Migration(format!("migration {name} is not UTF-8")))?
              .to_string();
          migs.push(Migration { version, name, sql });
      }
      migs.sort_by_key(|m| m.version);
      Ok(migs)
  }

  /// Create the `_migrations` ledger if it does not already exist.
  fn ensure_migrations_table(conn: &rusqlite::Connection) -> Result<()> {
      conn.execute_batch(
          "CREATE TABLE IF NOT EXISTS _migrations (\n\
             version    INTEGER PRIMARY KEY,\n\
             name       TEXT NOT NULL,\n\
             checksum   TEXT NOT NULL,\n\
             applied_at INTEGER NOT NULL\n\
           );",
      )
      .map_err(migration_err)
  }

  /// The recorded checksum for `version`, if any.
  fn recorded_checksum(conn: &rusqlite::Connection, version: i64) -> Result<Option<String>> {
      conn.query_row(
          "SELECT checksum FROM _migrations WHERE version = ?1",
          [version],
          |row| row.get::<_, String>(0),
      )
      .map(Some)
      .or_else(|e| match e {
          rusqlite::Error::QueryReturnedNoRows => Ok(None),
          other => Err(migration_err(other)),
      })
  }

  /// Apply every not-yet-recorded migration; verify checksums of recorded ones.
  fn apply_all(conn: &rusqlite::Connection, migs: &[Migration]) -> Result<()> {
      for m in migs {
          match recorded_checksum(conn, m.version)? {
              Some(existing) => {
                  let current = checksum(&m.sql);
                  if existing != current {
                      return Err(Error::Migration(format!(
                          "checksum mismatch for migration {} ({}): \
                           recorded {existing}, file {current}",
                          m.version, m.name
                      )));
                  }
                  // Already applied and unchanged: no-op.
              }
              None => {
                  let sum = checksum(&m.sql);
                  conn.execute_batch("BEGIN;").map_err(migration_err)?;
                  let applied = (|| -> Result<()> {
                      conn.execute_batch(&m.sql).map_err(migration_err)?;
                      conn.execute(
                          "INSERT INTO _migrations (version, name, checksum, applied_at) \
                           VALUES (?1, ?2, ?3, strftime('%s','now'))",
                          rusqlite::params![m.version, m.name, sum],
                      )
                      .map_err(migration_err)?;
                      Ok(())
                  })();
                  match applied {
                      Ok(()) => {
                          conn.execute_batch("COMMIT;").map_err(migration_err)?;
                      }
                      Err(e) => {
                          // Best-effort rollback; surface the original error.
                          let _ = conn.execute_batch("ROLLBACK;");
                          return Err(e);
                      }
                  }
              }
          }
      }
      Ok(())
  }

  /// Run all pending migrations against `conn`, transactionally and in order.
  ///
  /// - Creates `_migrations` if absent.
  /// - Discovers `NNN_*.sql` files, orders by the numeric prefix.
  /// - Applies each unseen version inside its own transaction, recording the
  ///   sha256 checksum.
  /// - Re-applying an already-recorded version is a no-op.
  /// - A checksum change on an already-applied version returns `Error::Migration`.
  pub fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
      ensure_migrations_table(conn)?;
      let migs = discover()?;
      apply_all(conn, &migs)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      const M1: &str = "CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT NOT NULL);";

      fn conn() -> rusqlite::Connection {
          rusqlite::Connection::open_in_memory().unwrap()
      }

      fn apply(c: &rusqlite::Connection, migs: &[Migration]) -> Result<()> {
          ensure_migrations_table(c)?;
          apply_all(c, migs)
      }

      fn mig(version: i64, name: &str, sql: &str) -> Migration {
          Migration { version, name: name.to_string(), sql: sql.to_string() }
      }

      #[test]
      fn applies_a_migration_and_records_it() {
          let c = conn();
          apply(&c, &[mig(1, "001_widgets.sql", M1)]).unwrap();
          let cnt: i64 = c
              .query_row(
                  "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='widgets'",
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          assert_eq!(cnt, 1, "widgets table should exist");
          let rows: i64 = c
              .query_row("SELECT count(*) FROM _migrations WHERE version=1", [], |r| r.get(0))
              .unwrap();
          assert_eq!(rows, 1, "version 1 recorded exactly once");
      }

      #[test]
      fn applying_twice_is_a_no_op() {
          let c = conn();
          let migs = [mig(1, "001_widgets.sql", M1)];
          apply(&c, &migs).unwrap();
          apply(&c, &migs).unwrap();
          let rows: i64 = c
              .query_row("SELECT count(*) FROM _migrations", [], |r| r.get(0))
              .unwrap();
          assert_eq!(rows, 1, "re-run is a no-op: still exactly one recorded version");
      }

      #[test]
      fn tampered_checksum_errors() {
          let c = conn();
          apply(&c, &[mig(1, "001_widgets.sql", M1)]).unwrap();
          let tampered = [mig(1, "001_widgets.sql", "CREATE TABLE other (id INTEGER);")];
          let err = apply(&c, &tampered).unwrap_err();
          assert!(
              matches!(err, Error::Migration(_)),
              "checksum mismatch must be Error::Migration, got {err:?}"
          );
      }

      #[test]
      fn checksum_is_stable_and_distinct() {
          assert_eq!(checksum("abc"), checksum("abc"));
          assert_ne!(checksum("abc"), checksum("abd"));
          assert_eq!(
              checksum("abc"),
              "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
          );
      }

      #[test]
      fn versions_apply_in_numeric_order() {
          let c = conn();
          let migs = [
              mig(2, "002_b.sql", "CREATE TABLE b (id INTEGER);"),
              mig(1, "001_a.sql", "CREATE TABLE a (id INTEGER);"),
              mig(10, "010_c.sql", "CREATE TABLE c (id INTEGER);"),
          ];
          apply(&c, &migs).unwrap();
          let table = |name: &str| -> i64 {
              c.query_row(
                  "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                  rusqlite::params![name],
                  |r| r.get(0),
              )
              .unwrap()
          };
          assert_eq!(table("a"), 1, "version 1 table applied");
          assert_eq!(table("b"), 1, "version 2 table applied");
          assert_eq!(table("c"), 1, "version 10 table applied");
          let recorded: i64 = c
              .query_row("SELECT count(*) FROM _migrations", [], |r| r.get(0))
              .unwrap();
          assert_eq!(recorded, 3, "all three versions recorded");
          let max: i64 = c
              .query_row("SELECT max(version) FROM _migrations", [], |r| r.get(0))
              .unwrap();
          assert_eq!(max, 10, "two-digit version recorded");
      }
  }
  ```

  `discover()` (and the numeric-prefix sort it performs) is exercised end-to-end in Task 17 once the real `001_initial_schema.sql` exists; it is already reachable here through the exported `run_migrations`, so it is not dead code.

- [ ] **Step 9: Run the tests, see them pass.** Run: `cargo test -p rb-store migrations` Expected: PASS (5 tests: `applies_a_migration_and_records_it`, `applying_twice_is_a_no_op`, `tampered_checksum_errors`, `checksum_is_stable_and_distinct`, `versions_apply_in_numeric_order`).

- [ ] **Step 10: Format and lint.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings, no errors.

- [ ] **Step 11: Commit.** Run: `git add crates/rb-store && git commit -m "feat(rb-store): add crate skeleton and checksummed migration runner"` Expected: commit created.

---

### Task 17: Initial schema migration file (`001_initial_schema.sql`)

**Files:**
- Create: `crates/rb-store/migrations/001_initial_schema.sql`
- Delete: `crates/rb-store/migrations/.keep`
- Test: `crates/rb-store/src/migrations.rs` (extend `#[cfg(test)] mod tests` with an embedded-DDL test)

This task writes the full base DDL from spec §9 — `meta`, `memories` (including `archived_at`), `memory_links`, `memories_fts` (FTS5 external-content) with AFTER INSERT / UPDATE / DELETE sync triggers, and the indexes. It deliberately does NOT create `memory_vectors` (its dimension is dynamic and is created in code in Task 18). `_migrations` is created by the runner, not by this file.

- [ ] **Step 1: Write a failing test that runs the embedded migration.** Append a new test to the `tests` module in `crates/rb-store/src/migrations.rs` (inside the existing `mod tests { ... }`, before its closing brace):

  ```rust
      #[test]
      fn embedded_initial_schema_creates_expected_objects() {
          let c = conn();
          // Run the real, file-discovered migrations (001_initial_schema.sql).
          run_migrations(&c).unwrap();

          let exists = |name: &str, kind: &str| -> bool {
              let n: i64 = c
                  .query_row(
                      "SELECT count(*) FROM sqlite_master WHERE type=?1 AND name=?2",
                      rusqlite::params![kind, name],
                      |r| r.get(0),
                  )
                  .unwrap();
              n == 1
          };

          assert!(exists("meta", "table"), "meta table");
          assert!(exists("memories", "table"), "memories table");
          assert!(exists("memory_links", "table"), "memory_links table");
          assert!(exists("memories_fts", "table"), "memories_fts virtual table");
          assert!(exists("idx_mem_ns", "index"), "idx_mem_ns");
          assert!(exists("idx_mem_active", "index"), "idx_mem_active partial index");
          assert!(exists("mem_ai", "trigger"), "FTS after-insert trigger");
          assert!(exists("mem_au", "trigger"), "FTS after-update trigger");
          assert!(exists("mem_ad", "trigger"), "FTS after-delete trigger");

          // memory_vectors is created in code at open(), NOT by the migration file.
          assert!(
              !exists("memory_vectors", "table"),
              "memory_vectors must NOT be created by static migrations"
          );

          // archived_at column is present in the BASE schema (no ghost migration).
          let has_archived: i64 = c
              .query_row(
                  "SELECT count(*) FROM pragma_table_info('memories') WHERE name='archived_at'",
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          assert_eq!(has_archived, 1, "archived_at present in base memories schema");

          // The memory_type CHECK constraint accepts a valid enum value...
          c.execute(
              "INSERT INTO memories (memory_id, namespace, created_at, updated_at, content, \
               summary, keywords, tags, memory_type, importance, confidence, embedding_model) \
               VALUES ('m1','global',0,0,'c','s','[]','[]','insight',5,1.0,'')",
              [],
          )
          .unwrap();
          // ...and the FTS row was synced by the after-insert trigger.
          let fts_rows: i64 = c
              .query_row("SELECT count(*) FROM memories_fts", [], |r| r.get(0))
              .unwrap();
          assert_eq!(fts_rows, 1, "after-insert trigger populated FTS");

          // ...but rejects an invalid memory_type.
          let bad = c.execute(
              "INSERT INTO memories (memory_id, namespace, created_at, updated_at, content, \
               summary, keywords, tags, memory_type, importance, confidence, embedding_model) \
               VALUES ('m2','global',0,0,'c','s','[]','[]','not_a_type',5,1.0,'')",
              [],
          );
          assert!(bad.is_err(), "CHECK constraint rejects invalid memory_type");
      }
  ```

- [ ] **Step 2: Run the test, see it fail.** Run: `cargo test -p rb-store embedded_initial_schema` Expected: FAIL (`run_migrations` finds no `.sql` file yet, so `meta`/`memories`/etc. do not exist and the `exists(...)` assertions fail).

- [ ] **Step 3: Write the migration DDL.** Create `crates/rb-store/migrations/001_initial_schema.sql` with the full base schema (the runner wraps the whole file in one transaction, so no `BEGIN`/`COMMIT` here):

  ```sql
  -- 001_initial_schema.sql
  -- Base schema for rusty-brain. One database file; meta is the single source
  -- of truth for invariants. memory_vectors is intentionally absent here: its
  -- dimension is dynamic and the vec0 virtual table is created in code at open().
  -- _migrations is created by the migration runner before this file is applied.

  -- meta: single source of truth for invariants
  -- (seeded at init: schema_version, embedding_model, embedding_dim)
  CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
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

  CREATE INDEX idx_mem_ns         ON memories(namespace);
  CREATE INDEX idx_mem_created    ON memories(created_at);
  CREATE INDEX idx_mem_importance ON memories(importance);
  CREATE INDEX idx_mem_active     ON memories(archived_at) WHERE archived_at IS NULL;

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

  -- FTS5 external-content index over the searchable text columns of memories.
  CREATE VIRTUAL TABLE memories_fts USING fts5(
    content,
    summary,
    keywords,
    tags,
    content='memories',
    content_rowid='rowid'
  );

  -- Keep the FTS index in sync with the memories table.
  CREATE TRIGGER mem_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, summary, keywords, tags)
    VALUES (new.rowid, new.content, new.summary, new.keywords, new.tags);
  END;

  CREATE TRIGGER mem_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary, keywords, tags)
    VALUES ('delete', old.rowid, old.content, old.summary, old.keywords, old.tags);
  END;

  CREATE TRIGGER mem_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary, keywords, tags)
    VALUES ('delete', old.rowid, old.content, old.summary, old.keywords, old.tags);
    INSERT INTO memories_fts(rowid, content, summary, keywords, tags)
    VALUES (new.rowid, new.content, new.summary, new.keywords, new.tags);
  END;
  ```

- [ ] **Step 4: Remove the placeholder.** Run: `git rm crates/rb-store/migrations/.keep` Expected: `.keep` removed (the directory now contains the real `.sql` file, so `include_dir!` still resolves).

- [ ] **Step 5: Run the test, see it pass.** Run: `cargo test -p rb-store embedded_initial_schema` Expected: PASS.

- [ ] **Step 6: Run the full crate test suite (no regressions).** Run: `cargo test -p rb-store` Expected: PASS (the 5 runner tests from Task 16 plus `embedded_initial_schema_creates_expected_objects`).

- [ ] **Step 7: Format and lint.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings, no errors.

- [ ] **Step 8: Commit.** Run: `git add crates/rb-store && git commit -m "feat(rb-store): add base schema migration with FTS triggers and indexes"` Expected: commit created.

---

### Task 18: `SqliteStore::open` / `open_in_memory` with fail-closed dimension check

**Files:**
- Create: `crates/rb-store/src/store.rs`
- Modify: `crates/rb-store/src/error.rs`
- Modify: `crates/rb-store/src/lib.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod tests`)

This task adds `SqliteStore` with `open` / `open_in_memory`: register the sqlite-vec extension (one audited `unsafe` block), set `PRAGMA journal_mode=WAL` + `foreign_keys=ON`, run migrations, create the dynamic-dim `memory_vectors` vec0 table IF NOT EXISTS, seed `meta.embedding_dim` on first init, and FAIL CLOSED with `Error::DimensionMismatch` if a previously-seeded dim differs from the requested one. The `Store` trait methods are stubbed (`unimplemented!`) here and filled in by later tasks in the next cluster; only `open`/`open_in_memory` are exercised. This task also adds the `storage_err` and `io_err` helpers to `error.rs` (first used here).

- [ ] **Step 1: Extend the error helpers.** Modify `crates/rb-store/src/error.rs` to add the two helpers the store needs (`migration_err` already exists). The file becomes:

  ```rust
  //! Internal conversions from foreign error types into `rb_types::Error`.

  use rb_types::Error;

  /// Map a `rusqlite::Error` to a storage error.
  pub(crate) fn storage_err(e: rusqlite::Error) -> Error {
      Error::Storage(e.to_string())
  }

  /// Map a `rusqlite::Error` encountered during migration to a migration error.
  pub(crate) fn migration_err(e: rusqlite::Error) -> Error {
      Error::Migration(e.to_string())
  }

  /// Map an I/O error to the IO variant.
  pub(crate) fn io_err(e: std::io::Error) -> Error {
      Error::Io(e.to_string())
  }
  ```

- [ ] **Step 2: Write the failing tests FIRST.** Create `crates/rb-store/src/store.rs` with the struct, an `unimplemented!` `Store` impl, unimplemented `open*`, and the complete test module:

  ```rust
  //! `SqliteStore`: the concrete `Store` backed by SQLite + sqlite-vec.

  use crate::error::{io_err, storage_err};
  use crate::migrations::run_migrations;
  use rb_types::{
      Error, MemoryId, MemoryLink, MemoryNote, MemoryUpdates, Namespace, Result,
  };
  use std::path::Path;

  /// The synchronous storage trait. The daemon wraps this on blocking threads.
  pub trait Store {
      fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()>;
      fn get_memory(&self, id: &MemoryId) -> Result<Option<MemoryNote>>;
      fn keyword_search(&self, ns: &Namespace, query: &str, limit: usize) -> Result<Vec<MemoryId>>;
      fn vector_search(
          &self,
          ns: &Namespace,
          embedding: &[f32],
          limit: usize,
      ) -> Result<Vec<(MemoryId, f32)>>;
      fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<MemoryId>>;
      fn list(
          &self,
          ns: &Namespace,
          min_importance: Option<u8>,
          limit: usize,
      ) -> Result<Vec<MemoryNote>>;
      fn update_memory(&self, id: &MemoryId, updates: &MemoryUpdates) -> Result<()>;
      fn archive_memory(&self, id: &MemoryId) -> Result<()>;
      fn add_link(&self, link: &MemoryLink) -> Result<()>;
  }

  /// SQLite-backed store. Owns a single connection (write path); the daemon owns
  /// the read pool separately in P1.
  pub struct SqliteStore {
      conn: rusqlite::Connection,
  }

  impl SqliteStore {
      /// Open (or create) a store at `path` with the given embedding dimension.
      pub fn open(_path: &Path, _embedding_dim: usize) -> Result<Self> {
          unimplemented!("implemented in Step 4")
      }

      /// Open an ephemeral in-memory store with the given embedding dimension.
      pub fn open_in_memory(_embedding_dim: usize) -> Result<Self> {
          unimplemented!("implemented in Step 4")
      }
  }

  impl Store for SqliteStore {
      fn insert_memory(&self, _note: &MemoryNote, _embedding: Option<&[f32]>) -> Result<()> {
          unimplemented!("next cluster")
      }
      fn get_memory(&self, _id: &MemoryId) -> Result<Option<MemoryNote>> {
          unimplemented!("next cluster")
      }
      fn keyword_search(&self, _ns: &Namespace, _query: &str, _limit: usize) -> Result<Vec<MemoryId>> {
          unimplemented!("next cluster")
      }
      fn vector_search(
          &self,
          _ns: &Namespace,
          _embedding: &[f32],
          _limit: usize,
      ) -> Result<Vec<(MemoryId, f32)>> {
          unimplemented!("next cluster")
      }
      fn graph_neighbors(&self, _id: &MemoryId, _depth: u8) -> Result<Vec<MemoryId>> {
          unimplemented!("next cluster")
      }
      fn list(
          &self,
          _ns: &Namespace,
          _min_importance: Option<u8>,
          _limit: usize,
      ) -> Result<Vec<MemoryNote>> {
          unimplemented!("next cluster")
      }
      fn update_memory(&self, _id: &MemoryId, _updates: &MemoryUpdates) -> Result<()> {
          unimplemented!("next cluster")
      }
      fn archive_memory(&self, _id: &MemoryId) -> Result<()> {
          unimplemented!("next cluster")
      }
      fn add_link(&self, _link: &MemoryLink) -> Result<()> {
          unimplemented!("next cluster")
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn open_in_memory_creates_schema_and_seeds_dim() {
          let store = SqliteStore::open_in_memory(1024).unwrap();
          let c = &store.conn;

          let table = |name: &str| -> bool {
              let n: i64 = c
                  .query_row(
                      "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                      rusqlite::params![name],
                      |r| r.get(0),
                  )
                  .unwrap();
              n == 1
          };

          assert!(table("meta"), "meta exists");
          assert!(table("memories"), "memories exists");
          assert!(table("memory_links"), "memory_links exists");
          assert!(table("memories_fts"), "memories_fts exists");
          assert!(table("memory_vectors"), "memory_vectors created in code at open");

          // embedding_dim seeded to the requested value.
          let dim: String = c
              .query_row("SELECT value FROM meta WHERE key='embedding_dim'", [], |r| r.get(0))
              .unwrap();
          assert_eq!(dim, "1024", "embedding_dim seeded");

          // foreign_keys pragma is ON.
          let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
          assert_eq!(fk, 1, "foreign_keys ON");
      }

      #[test]
      fn open_persists_and_reopen_same_dim_ok() {
          let dir = tempfile::tempdir().unwrap();
          let path = dir.path().join("rb.db");

          {
              let _s = SqliteStore::open(&path, 768).unwrap();
          }
          // Re-open with the SAME dim: succeeds, dim unchanged.
          let s2 = SqliteStore::open(&path, 768).unwrap();
          let dim: String = s2
              .conn
              .query_row("SELECT value FROM meta WHERE key='embedding_dim'", [], |r| r.get(0))
              .unwrap();
          assert_eq!(dim, "768");
      }

      #[test]
      fn reopen_with_different_dim_fails_closed() {
          let dir = tempfile::tempdir().unwrap();
          let path = dir.path().join("rb.db");

          {
              let _s = SqliteStore::open(&path, 768).unwrap();
          }
          // Re-open with a DIFFERENT dim must fail closed.
          let err = SqliteStore::open(&path, 1024).unwrap_err();
          match err {
              Error::DimensionMismatch { expected, got } => {
                  assert_eq!(expected, 768, "stored dim is the expected");
                  assert_eq!(got, 1024, "requested dim is what we got");
              }
              other => panic!("expected DimensionMismatch, got {other:?}"),
          }
      }

      #[test]
      fn wal_mode_enabled_for_file_db() {
          let dir = tempfile::tempdir().unwrap();
          let path = dir.path().join("rb.db");
          let s = SqliteStore::open(&path, 256).unwrap();
          let mode: String = s
              .conn
              .query_row("PRAGMA journal_mode", [], |r| r.get(0))
              .unwrap();
          assert_eq!(mode.to_lowercase(), "wal", "file DB uses WAL");
      }
  }
  ```

- [ ] **Step 3: Run the tests, see them fail.** Run: `cargo test -p rb-store store` Expected: FAIL (`open`/`open_in_memory` are `unimplemented!`, so each test panics).

- [ ] **Step 4: Implement `open` / `open_in_memory`.** Replace the `impl SqliteStore { ... }` block (the two `unimplemented!` constructors) in `crates/rb-store/src/store.rs` with the real implementation plus private helpers. The new block:

  ```rust
  impl SqliteStore {
      /// Open (or create) a store at `path` with the given embedding dimension.
      ///
      /// Registers sqlite-vec, enables WAL + foreign keys, runs migrations,
      /// creates the dynamic-dim `memory_vectors` table, and enforces the
      /// embedding-dimension invariant fail-closed.
      pub fn open(path: &Path, embedding_dim: usize) -> Result<Self> {
          register_vec();
          let conn = rusqlite::Connection::open(path).map_err(|e| {
              io_err(std::io::Error::new(
                  std::io::ErrorKind::Other,
                  format!("open {}: {e}", path.display()),
              ))
          })?;
          Self::init(conn, embedding_dim)
      }

      /// Open an ephemeral in-memory store with the given embedding dimension.
      pub fn open_in_memory(embedding_dim: usize) -> Result<Self> {
          register_vec();
          let conn = rusqlite::Connection::open_in_memory().map_err(storage_err)?;
          Self::init(conn, embedding_dim)
      }

      /// Shared init path: pragmas, migrations, vectors table, dim invariant.
      fn init(conn: rusqlite::Connection, embedding_dim: usize) -> Result<Self> {
          // WAL gives concurrent readers + one writer with no SQLITE_BUSY storms.
          // (In-memory DBs ignore WAL and report "memory"; that is fine.)
          conn.pragma_update(None, "journal_mode", "WAL")
              .map_err(storage_err)?;
          conn.pragma_update(None, "foreign_keys", "ON")
              .map_err(storage_err)?;

          run_migrations(&conn)?;

          // Dynamic-dimension vector table. vec0 needs the literal dim baked in.
          conn.execute_batch(&format!(
              "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vectors USING vec0(\n\
                 memory_id TEXT PRIMARY KEY,\n\
                 embedding float[{embedding_dim}]\n\
               );"
          ))
          .map_err(storage_err)?;

          seed_or_verify_dim(&conn, embedding_dim)?;

          Ok(Self { conn })
      }
  }

  /// Register the sqlite-vec extension so `vec0` virtual tables and the KNN
  /// `MATCH` syntax are available on every subsequently opened connection.
  fn register_vec() {
      // SAFETY: `sqlite_vec::sqlite3_vec_init` is the FFI entry point published by
      // the sqlite-vec crate. `sqlite3_auto_extension` registers it with SQLite so
      // it runs on each connection opened AFTER this call. We cast the fn pointer
      // exactly as the sqlite-vec crate does in its own test (`transmute` of a
      // `*const ()`); the target fn-pointer type is inferred from the
      // `sqlite3_auto_extension` argument slot, so no explicit (and crate-private)
      // signature annotation is needed. The init fn is valid for the program's
      // lifetime; re-registration on subsequent `open*` calls is idempotent/benign.
      #[allow(unsafe_code)]
      #[allow(clippy::missing_transmute_annotations)]
      unsafe {
          rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
              sqlite_vec::sqlite3_vec_init as *const (),
          )));
      }
  }

  /// Seed `meta.embedding_dim` on first init, or verify it matches on re-open.
  /// Fails closed with `Error::DimensionMismatch` on disagreement.
  fn seed_or_verify_dim(conn: &rusqlite::Connection, embedding_dim: usize) -> Result<()> {
      let existing: Option<String> = conn
          .query_row(
              "SELECT value FROM meta WHERE key='embedding_dim'",
              [],
              |r| r.get::<_, String>(0),
          )
          .map(Some)
          .or_else(|e| match e {
              rusqlite::Error::QueryReturnedNoRows => Ok(None),
              other => Err(storage_err(other)),
          })?;

      match existing {
          Some(v) => {
              let stored: usize = v.parse().map_err(|_| {
                  Error::Storage(format!("meta.embedding_dim is not an integer: {v:?}"))
              })?;
              if stored != embedding_dim {
                  return Err(Error::DimensionMismatch {
                      expected: stored,
                      got: embedding_dim,
                  });
              }
          }
          None => {
              conn.execute(
                  "INSERT INTO meta (key, value) VALUES ('embedding_dim', ?1)",
                  rusqlite::params![embedding_dim.to_string()],
              )
              .map_err(storage_err)?;
          }
      }
      Ok(())
  }
  ```

  Note: `register_vec` casts `sqlite3_vec_init` to the fn-pointer type `sqlite3_auto_extension` expects. An explicit turbofish `transmute::<*const (), unsafe extern "C" fn()>` does NOT compile (the expected type takes three pointer args and returns `c_int`, so `Some(<fn()>)` fails with E0308); the inferred form does, but then trips `clippy::missing_transmute_annotations` under `-D warnings`, so the lint is suppressed with the `#[allow]` above. (verify against installed sqlite-vec at execution; if the pinned crate exposes a safe `sqlite_vec::load(&conn)` or a `rusqlite::Connection`-level loader, prefer that and drop the `unsafe`/transmute block.)

- [ ] **Step 5: Export `Store` and `SqliteStore` from the crate root.** Modify `crates/rb-store/src/lib.rs` to declare and re-export the store module:

  ```rust
  //! `rb-store`: SQLite + sqlite-vec storage engine for rusty-brain.
  //!
  //! One database file holds memories, FTS index, vectors and links so that a
  //! `remember` is a single transaction (no dual-DB desync). The embedding
  //! dimension is a single configured value, enforced fail-closed at `open`.
  #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

  mod error;
  mod migrations;
  mod store;

  pub use migrations::run_migrations;
  pub use store::{SqliteStore, Store};
  ```

- [ ] **Step 6: Run the store tests, see them pass.** Run: `cargo test -p rb-store store` Expected: PASS (4 tests: `open_in_memory_creates_schema_and_seeds_dim`, `open_persists_and_reopen_same_dim_ok`, `reopen_with_different_dim_fails_closed`, `wal_mode_enabled_for_file_db`).

- [ ] **Step 7: Run the full crate suite (no regressions).** Run: `cargo test -p rb-store` Expected: PASS (migration tests from Tasks 13-14 plus the 4 store tests).

- [ ] **Step 8: Format and lint the workspace.** Run: `cargo fmt --all` then `cargo clippy --workspace --all-targets -- -D warnings` Expected: no warnings. The single `unsafe` block is covered by `#[allow(unsafe_code)]` with a SAFETY comment (satisfying the workspace `unsafe_code="warn"` lint), and the inferred transmute is covered by `#[allow(clippy::missing_transmute_annotations)]`. All three error helpers (`storage_err`, `migration_err`, `io_err`) are now exercised across the crate, so no `dead_code` errors remain.

- [ ] **Step 9: Commit.** Run: `git add crates/rb-store && git commit -m "feat(rb-store): add SqliteStore open with sqlite-vec registration and fail-closed dim check"` Expected: commit created.

## Part D — rb-store: CRUD, FTS, vector & graph

### Task 19: `insert_memory` — single-transaction write (memories + vectors + links)

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod insert_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod insert_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, MemoryLink, LinkType};

    fn vec8(seed: f32) -> Vec<f32> {
        (0..8).map(|i| seed + i as f32 * 0.1).collect()
    }

    #[test]
    fn insert_persists_memory_vector_and_links() {
        let store = SqliteStore::open_in_memory(8).unwrap();

        let mut a = MemoryNote::new(
            Namespace::Project("rb".into()),
            "alpha content".into(),
            MemoryType::CodePattern,
            5,
        );
        let mut b = MemoryNote::new(
            Namespace::Project("rb".into()),
            "beta content".into(),
            MemoryType::Insight,
            7,
        );
        b.tags = vec!["x".into(), "y".into()];

        // Insert target first so the link FK is satisfiable.
        store.insert_memory(&b, Some(&vec8(0.5))).unwrap();

        a.keywords = vec!["k1".into()];
        a.related_files = vec!["src/lib.rs".into()];
        a.links = vec![MemoryLink {
            source_id: a.id.clone(),
            target_id: b.id.clone(),
            link_type: LinkType::References,
            strength: 0.8,
            reason: "see beta".into(),
            created_at: a.created_at,
        }];
        store.insert_memory(&a, Some(&vec8(1.0))).unwrap();

        // memories row count
        let n: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);

        // FTS populated via trigger (external-content; requires INSERT trigger from migration 001)
        let fts: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memories_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, 2);

        // vector row stored
        let vn: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vn, 2);

        // link stored
        let ln: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ln, 1);
    }

    #[test]
    fn insert_without_embedding_skips_vector() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let m = MemoryNote::new(Namespace::Global, "no vec".into(), MemoryType::Reference, 3);
        store.insert_memory(&m, None).unwrap();
        let vn: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get(0))
            .unwrap();
        assert_eq!(vn, 0);
    }
}
```

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store insert_tests` Expected: FAIL — the `insert_memory` stub returns an error / does not persist rows, so the row-count asserts fail.

- [ ] **Step 3: Minimal impl.** Replace the `insert_memory` stub in `impl Store for SqliteStore` with the full method, and add the private codec helpers above the `impl` block:
```rust
fn json_array(v: &[String]) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Serialization(e.to_string()))
}

fn ts(dt: chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp()
}

fn opt_ts(dt: Option<chrono::DateTime<chrono::Utc>>) -> Option<i64> {
    dt.map(|d| d.timestamp())
}

fn embedding_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf
}
```
Then the method. All three writes (memory row, optional vector, links) happen inside ONE transaction; if any step fails the `Transaction` is dropped without commit and rolls back, so no partial/desynced state is possible:
```rust
fn insert_memory(&self, note: &MemoryNote, embedding: Option<&[f32]>) -> Result<()> {
    let tx = self
        .conn
        .unchecked_transaction()
        .map_err(|e| Error::Storage(e.to_string()))?;

    tx.execute(
        "INSERT INTO memories (
            memory_id, namespace, created_at, updated_at, content, summary,
            keywords, tags, context, memory_type, importance, confidence,
            related_files, access_count, last_accessed_at, archived_at,
            superseded_by, embedding_model
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18
         )",
        rusqlite::params![
            note.id.to_string(),
            note.namespace.as_db_string(),
            ts(note.created_at),
            ts(note.updated_at),
            note.content,
            note.summary,
            json_array(&note.keywords)?,
            json_array(&note.tags)?,
            note.context,
            note.memory_type.as_str(),
            note.importance as i64,
            note.confidence as f64,
            json_array(&note.related_files)?,
            note.access_count as i64,
            opt_ts(note.last_accessed_at),
            opt_ts(note.archived_at),
            note.superseded_by.as_ref().map(|id| id.to_string()),
            note.embedding_model,
        ],
    )
    .map_err(|e| Error::Storage(e.to_string()))?;

    if let Some(emb) = embedding {
        tx.execute(
            "INSERT INTO memory_vectors (memory_id, embedding) VALUES (?1, ?2)",
            rusqlite::params![note.id.to_string(), embedding_bytes(emb)],
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    }

    for link in &note.links {
        tx.execute(
            "INSERT INTO memory_links
                (source_id, target_id, link_type, strength, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                link.source_id.to_string(),
                link.target_id.to_string(),
                link.link_type.as_str(),
                link.strength as f64,
                link.reason,
                ts(link.created_at),
            ],
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    }

    tx.commit().map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}
```
(verify against installed sqlite-vec at execution; if vec0 rejects a raw little-endian f32 blob, pass the embedding via the JSON form `serde_json::to_string(emb)` instead and adjust the bind.)

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store insert_tests` Expected: PASS (2 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement insert_memory single-transaction write"`

---

### Task 20: `get_memory` — explicit-column decode incl. links, JSON arrays, timestamps

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod get_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod get_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, MemoryLink, LinkType, MemoryId};

    #[test]
    fn get_round_trips_all_fields_and_links() {
        let store = SqliteStore::open_in_memory(8).unwrap();

        let target = MemoryNote::new(
            Namespace::Session { project: "rb".into(), session_id: "s1".into() },
            "target".into(),
            MemoryType::Entity,
            4,
        );
        store.insert_memory(&target, None).unwrap();

        let mut m = MemoryNote::new(
            Namespace::Project("rb".into()),
            "full content".into(),
            MemoryType::BugFix,
            9,
        );
        m.summary = "a summary".into();
        m.keywords = vec!["alpha".into(), "beta".into()];
        m.tags = vec!["t1".into()];
        m.context = "while fixing X".into();
        m.confidence = 0.75;
        m.related_files = vec!["a.rs".into(), "b.rs".into()];
        m.embedding_model = "voyage-3".into();
        m.links = vec![MemoryLink {
            source_id: m.id.clone(),
            target_id: target.id.clone(),
            link_type: LinkType::Implements,
            strength: 0.6,
            reason: "impl".into(),
            created_at: m.created_at,
        }];
        store.insert_memory(&m, None).unwrap();

        let got = store.get_memory(&m.id).unwrap().expect("memory present");
        assert_eq!(got.id, m.id);
        assert_eq!(got.namespace, Namespace::Project("rb".into()));
        assert_eq!(got.content, "full content");
        assert_eq!(got.summary, "a summary");
        assert_eq!(got.keywords, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(got.tags, vec!["t1".to_string()]);
        assert_eq!(got.context, "while fixing X");
        assert_eq!(got.memory_type, MemoryType::BugFix);
        assert_eq!(got.importance, 9);
        assert!((got.confidence - 0.75).abs() < 1e-6);
        assert_eq!(got.related_files, vec!["a.rs".to_string(), "b.rs".to_string()]);
        assert_eq!(got.embedding_model, "voyage-3");
        assert_eq!(got.created_at.timestamp(), m.created_at.timestamp());
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.links[0].target_id, target.id);
        assert_eq!(got.links[0].link_type, LinkType::Implements);
        assert!((got.links[0].strength - 0.6).abs() < 1e-6);
    }

    #[test]
    fn get_missing_returns_none() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        assert!(store.get_memory(&MemoryId::new()).unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store get_tests` Expected: FAIL (`get_memory` stub returns `None`/error, so `expect("memory present")` panics).

- [ ] **Step 3: Minimal impl.** Add private decode helpers above the `impl Store` block, then implement `get_memory`. Note all three closures (`g`, `gi`) and `load_links` borrow `conn` immutably via `prepare`, which coexists with the outer statement's immutable borrow:
```rust
fn parse_json_array(s: &str) -> Result<Vec<String>> {
    serde_json::from_str(s).map_err(|e| Error::Serialization(e.to_string()))
}

fn from_ts(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap_or_default())
}

fn from_opt_ts(secs: Option<i64>) -> Option<chrono::DateTime<chrono::Utc>> {
    secs.map(from_ts)
}

fn parse_id(s: &str) -> Result<MemoryId> {
    s.parse::<MemoryId>()
}

fn load_links(conn: &rusqlite::Connection, id: &MemoryId) -> Result<Vec<MemoryLink>> {
    let mut stmt = conn
        .prepare(
            "SELECT source_id, target_id, link_type, strength, reason, created_at
             FROM memory_links WHERE source_id = ?1",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut links = Vec::new();
    for r in rows {
        let (src, tgt, lt, strength, reason, created) =
            r.map_err(|e| Error::Storage(e.to_string()))?;
        links.push(MemoryLink {
            source_id: parse_id(&src)?,
            target_id: parse_id(&tgt)?,
            link_type: rb_types::LinkType::parse(&lt)?,
            strength: strength as f32,
            reason,
            created_at: from_ts(created),
        });
    }
    Ok(links)
}

fn row_to_note(conn: &rusqlite::Connection, row: &rusqlite::Row<'_>) -> Result<MemoryNote> {
    let id = parse_id(&row.get::<_, String>("memory_id").map_err(|e| Error::Storage(e.to_string()))?)?;
    let namespace = Namespace::parse_db_string(
        &row.get::<_, String>("namespace").map_err(|e| Error::Storage(e.to_string()))?,
    )?;
    let memory_type = MemoryType::parse(
        &row.get::<_, String>("memory_type").map_err(|e| Error::Storage(e.to_string()))?,
    )?;
    let g = |c: &str| -> Result<String> {
        row.get::<_, String>(c).map_err(|e| Error::Storage(e.to_string()))
    };
    let gi = |c: &str| -> Result<i64> {
        row.get::<_, i64>(c).map_err(|e| Error::Storage(e.to_string()))
    };
    let links = load_links(conn, &id)?;
    Ok(MemoryNote {
        id,
        namespace,
        created_at: from_ts(gi("created_at")?),
        updated_at: from_ts(gi("updated_at")?),
        content: g("content")?,
        summary: g("summary")?,
        keywords: parse_json_array(&g("keywords")?)?,
        tags: parse_json_array(&g("tags")?)?,
        context: g("context")?,
        memory_type,
        importance: gi("importance")? as u8,
        confidence: row.get::<_, f64>("confidence").map_err(|e| Error::Storage(e.to_string()))? as f32,
        related_files: parse_json_array(&g("related_files")?)?,
        access_count: gi("access_count")? as u64,
        last_accessed_at: from_opt_ts(
            row.get::<_, Option<i64>>("last_accessed_at").map_err(|e| Error::Storage(e.to_string()))?,
        ),
        archived_at: from_opt_ts(
            row.get::<_, Option<i64>>("archived_at").map_err(|e| Error::Storage(e.to_string()))?,
        ),
        superseded_by: row
            .get::<_, Option<String>>("superseded_by")
            .map_err(|e| Error::Storage(e.to_string()))?
            .map(|s| parse_id(&s))
            .transpose()?,
        embedding_model: g("embedding_model")?,
        links,
    })
}
```
Then the method:
```rust
fn get_memory(&self, id: &MemoryId) -> Result<Option<MemoryNote>> {
    let mut stmt = self
        .conn
        .prepare(
            "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                    keywords, tags, context, memory_type, importance, confidence,
                    related_files, access_count, last_accessed_at, archived_at,
                    superseded_by, embedding_model
             FROM memories WHERE memory_id = ?1",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut rows = stmt
        .query(rusqlite::params![id.to_string()])
        .map_err(|e| Error::Storage(e.to_string()))?;

    match rows.next().map_err(|e| Error::Storage(e.to_string()))? {
        Some(row) => Ok(Some(row_to_note(&self.conn, row)?)),
        None => Ok(None),
    }
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store get_tests` Expected: PASS (2 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement get_memory with explicit-column decode and links"`

---

### Task 21: `keyword_search` — FTS5 with escape helper, namespace-scoped, active only

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod keyword_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod keyword_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert(store: &SqliteStore, ns: Namespace, content: &str) -> rb_types::MemoryId {
        let m = MemoryNote::new(ns, content.into(), MemoryType::Reference, 5);
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
        id
    }

    #[test]
    fn finds_matching_and_scopes_to_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "rust async runtime tokio");
        let _miss_ns = insert(&store, Namespace::Global, "rust async runtime tokio");
        let _miss_term = insert(&store, proj.clone(), "completely different topic");

        let found = store.keyword_search(&proj, "tokio", 10).unwrap();
        assert_eq!(found, vec![hit]);
    }

    #[test]
    fn escapes_special_query_chars() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let hit = insert(&store, proj.clone(), "config flag enable-cache value");

        // The '-' would be an FTS5 operator (NOT) if unescaped. After escaping, the
        // query is treated as the literal phrase "enable cache" (unicode61 splits on
        // '-'), which matches the adjacent tokens in the document.
        let found = store.keyword_search(&proj, "enable-cache", 10).unwrap();
        assert_eq!(found, vec![hit.clone()]);

        // A query that is nothing but FTS5 operators must NOT raise a syntax error;
        // escaped, it becomes an empty/operator-free phrase that simply matches nothing.
        let none = store.keyword_search(&proj, "OR AND (", 10).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn excludes_archived() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let id = insert(&store, proj.clone(), "archivable widget");
        store.archive_memory(&id).unwrap();
        let found = store.keyword_search(&proj, "widget", 10).unwrap();
        assert!(found.is_empty());
    }
}
```
(Note: the `excludes_archived` assertion depends on `archive_memory`, implemented in Task 26; until then it returns the row instead of excluding it. In Step 4 run only the first two subtests with `cargo test -p rb-store keyword_tests::finds keyword_tests::escapes`, and re-run the full module after Task 26.)

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store keyword_tests::finds` Expected: FAIL (`keyword_search` stub returns empty / error).

- [ ] **Step 3: Minimal impl.** Add the escape helper above the `impl Store` block, then implement `keyword_search`. Do NOT alias the FTS table: an FTS5 `MATCH` must reference the table by the same name used in the FROM clause, and an alias on the FTS table breaks `table MATCH ?`:
```rust
/// Wrap the user query as a single FTS5 phrase so operators (-, OR, NEAR, *, ")
/// are treated as literal text. Internal double-quotes are doubled per FTS5 rules.
fn escape_fts5_query(query: &str) -> String {
    let escaped = query.replace('"', "\"\"");
    format!("\"{escaped}\"")
}
```
```rust
fn keyword_search(&self, ns: &Namespace, query: &str, limit: usize) -> Result<Vec<MemoryId>> {
    let match_expr = escape_fts5_query(query);
    let mut stmt = self
        .conn
        .prepare(
            "SELECT m.memory_id
             FROM memories_fts
             JOIN memories m ON m.rowid = memories_fts.rowid
             WHERE memories_fts MATCH ?1
               AND m.namespace = ?2
               AND m.archived_at IS NULL
             ORDER BY rank
             LIMIT ?3",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(
            rusqlite::params![match_expr, ns.as_db_string(), limit as i64],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut ids = Vec::new();
    for r in rows {
        let s = r.map_err(|e| Error::Storage(e.to_string()))?;
        ids.push(s.parse::<MemoryId>()?);
    }
    Ok(ids)
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store keyword_tests::finds keyword_tests::escapes` Expected: PASS (2 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement keyword_search with FTS5 escaping and ns scope"`

---

### Task 22: `vector_search` — sqlite-vec vec0 KNN returning (MemoryId, distance)

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod vector_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod vector_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_vec(store: &SqliteStore, ns: Namespace, content: &str, v: [f32; 8]) -> rb_types::MemoryId {
        let m = MemoryNote::new(ns, content.into(), MemoryType::Insight, 5);
        let id = m.id.clone();
        store.insert_memory(&m, Some(&v)).unwrap();
        id
    }

    #[test]
    fn returns_nearest_first_scoped_to_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());

        let near = insert_vec(&store, proj.clone(), "near", [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let far = insert_vec(&store, proj.clone(), "far", [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // Different namespace, identical to query: must be excluded by scope.
        let other = insert_vec(
            &store,
            Namespace::Global,
            "other",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let query = [0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let res = store.vector_search(&proj, &query, 10).unwrap();

        let ids: Vec<_> = res.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids, vec![near.clone(), far.clone()]);
        // distances are ascending
        assert!(res[0].1 <= res[1].1);
        assert!(!ids.contains(&other));
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let err = store.vector_search(&proj, &[0.0, 0.0, 0.0], 5).unwrap_err();
        assert!(matches!(err, Error::DimensionMismatch { expected: 8, got: 3 }));
    }
}
```

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store vector_tests` Expected: FAIL (`vector_search` stub).

- [ ] **Step 3: Minimal impl.** Implement `vector_search`. Fail closed on dimension mismatch first. The struct is assumed to carry the configured dim as `embedding_dim: usize` (added by the `open()`/store-open task); if that field is not present, read the dim from `meta` (`SELECT value FROM meta WHERE key='embedding_dim'`) instead. Because vec0 KNN runs FIRST and external JOIN/WHERE filters are applied AFTER, a `k = ?` set to `limit` would under-return when namespace scoping drops some of the k globally-nearest rows. To scope correctly at the spec's brute-force/small scale, over-fetch a candidate pool with `k`, then filter by namespace + active in the outer query and apply the real `LIMIT`:
```rust
fn vector_search(&self, ns: &Namespace, embedding: &[f32], limit: usize) -> Result<Vec<(MemoryId, f32)>> {
    if embedding.len() != self.embedding_dim {
        return Err(Error::DimensionMismatch {
            expected: self.embedding_dim,
            got: embedding.len(),
        });
    }

    // sqlite-vec accepts the query vector as a JSON array string.
    let query_json = serde_json::to_string(embedding)
        .map_err(|e| Error::Serialization(e.to_string()))?;

    // Over-fetch: vec0 requires an explicit `k`. Fetching more than `limit`
    // candidates lets the outer namespace/active filter still yield up to `limit`
    // in-scope nearest neighbors. vec0 returns min(k, total_rows) without error.
    let k_budget = (limit as i64).saturating_mul(10).max(limit as i64);

    let mut stmt = self
        .conn
        .prepare(
            "WITH knn AS (
                 SELECT memory_id, distance
                 FROM memory_vectors
                 WHERE embedding MATCH ?1
                   AND k = ?2
             )
             SELECT knn.memory_id, knn.distance
             FROM knn
             JOIN memories m ON m.memory_id = knn.memory_id
             WHERE m.namespace = ?3
               AND m.archived_at IS NULL
             ORDER BY knn.distance
             LIMIT ?4",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(
            rusqlite::params![query_json, k_budget, ns.as_db_string(), limit as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut out = Vec::new();
    for r in rows {
        let (id, dist) = r.map_err(|e| Error::Storage(e.to_string()))?;
        out.push((id.parse::<MemoryId>()?, dist as f32));
    }
    Ok(out)
}
```
(verify against installed sqlite-vec at execution; confirm the KNN column names are `distance` and the constraint form is `embedding MATCH ? AND k = ?`. If the installed version exposes a different distance column or supports `LIMIT` instead of `k =` for KNN, adjust the inner CTE accordingly — keep the outer namespace/active filter + LIMIT unchanged.)

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store vector_tests` Expected: PASS (2 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement vector_search KNN with dim guard and ns scope"`

---

### Task 23: `graph_neighbors` — bounded recursive CTE over memory_links

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod graph_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod graph_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, MemoryLink, LinkType};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(Namespace::Project("rb".into()), c.into(), MemoryType::Entity, 5);
        store.insert_memory(&m, None).unwrap();
        m
    }

    fn link(store: &SqliteStore, src: &MemoryNote, tgt: &MemoryNote) {
        store
            .add_link(&MemoryLink {
                source_id: src.id.clone(),
                target_id: tgt.id.clone(),
                link_type: LinkType::References,
                strength: 1.0,
                reason: String::new(),
                created_at: src.created_at,
            })
            .unwrap();
    }

    #[test]
    fn traverses_up_to_depth() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");
        let c = node(&store, "c");
        let d = node(&store, "d");
        link(&store, &a, &b); // a -> b
        link(&store, &b, &c); // b -> c
        link(&store, &c, &d); // c -> d

        let mut depth1 = store.graph_neighbors(&a.id, 1).unwrap();
        depth1.sort_by_key(|id| id.to_string());
        let mut want1 = vec![b.id.clone()];
        want1.sort_by_key(|id| id.to_string());
        assert_eq!(depth1, want1);

        let mut depth2 = store.graph_neighbors(&a.id, 2).unwrap();
        depth2.sort_by_key(|id| id.to_string());
        let mut want2 = vec![b.id.clone(), c.id.clone()];
        want2.sort_by_key(|id| id.to_string());
        assert_eq!(depth2, want2);
    }

    #[test]
    fn no_links_returns_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "lonely");
        assert!(store.graph_neighbors(&a.id, 3).unwrap().is_empty());
    }
}
```
(Note: the `link()` helper calls `add_link` (Task 27). Until `add_link` is implemented, the `traverses_up_to_depth` subtest cannot exercise links and will not pass. The module compiles because `add_link` exists in the trait impl as a stub. Implement `add_link` (Task 27) before running this module's link-dependent test; `no_links_returns_empty` can be run earlier since it adds no links.)

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store graph_tests::no_links` Expected: FAIL (`graph_neighbors` stub returns error / non-empty).

- [ ] **Step 3: Minimal impl.** Implement `graph_neighbors` with a bounded recursive CTE. `UNION` (not `UNION ALL`) dedups visited nodes and prevents infinite cycles; `WHERE w.d < ?2` bounds the depth:
```rust
fn graph_neighbors(&self, id: &MemoryId, depth: u8) -> Result<Vec<MemoryId>> {
    if depth == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = self
        .conn
        .prepare(
            "WITH RECURSIVE walk(node, d) AS (
                 SELECT target_id, 1
                 FROM memory_links
                 WHERE source_id = ?1
                 UNION
                 SELECT l.target_id, w.d + 1
                 FROM memory_links l
                 JOIN walk w ON l.source_id = w.node
                 WHERE w.d < ?2
             )
             SELECT DISTINCT node
             FROM walk
             WHERE node <> ?1",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(
            rusqlite::params![id.to_string(), depth as i64],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut ids = Vec::new();
    for r in rows {
        let s = r.map_err(|e| Error::Storage(e.to_string()))?;
        ids.push(s.parse::<MemoryId>()?);
    }
    Ok(ids)
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store graph_tests::no_links` Expected: PASS. (The full module, including `traverses_up_to_depth`, is re-run in Task 27 once `add_link` exists.)

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement graph_neighbors bounded recursive CTE"`

---

### Task 24: `list` — active only, min_importance filter, ORDER BY created_at DESC

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod list_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod list_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn insert_imp(store: &SqliteStore, ns: Namespace, content: &str, importance: u8) -> rb_types::MemoryId {
        let mut m = MemoryNote::new(ns, content.into(), MemoryType::Reference, importance);
        // Force distinct created_at ordering by nudging timestamps.
        m.created_at = m.created_at - chrono::Duration::seconds(importance as i64);
        m.updated_at = m.created_at;
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
        id
    }

    #[test]
    fn orders_by_created_desc_and_filters_importance_and_ns() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        // importance => created_at offset: lower importance = more recent (smaller subtraction)
        let high_recent = insert_imp(&store, proj.clone(), "recent high", 2); // -2s, imp 2
        let mid = insert_imp(&store, proj.clone(), "older mid", 5);           // -5s, imp 5
        let low = insert_imp(&store, proj.clone(), "oldest low", 8);          // -8s, imp 8
        let _other_ns = insert_imp(&store, Namespace::Global, "global", 1);

        // No importance filter: newest first.
        let all = store.list(&proj, None, 10).unwrap();
        let ids: Vec<_> = all.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec![high_recent.clone(), mid.clone(), low.clone()]);

        // min_importance = 5 keeps mid(5) and low(8), drops high(2).
        let filtered = store.list(&proj, Some(5), 10).unwrap();
        let fids: Vec<_> = filtered.iter().map(|m| m.id.clone()).collect();
        assert_eq!(fids, vec![mid.clone(), low.clone()]);

        // limit respected.
        let limited = store.list(&proj, None, 1).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, high_recent);
    }

    #[test]
    fn excludes_archived() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let keep = insert_imp(&store, proj.clone(), "keep", 5);
        let drop = insert_imp(&store, proj.clone(), "drop", 5);
        store.archive_memory(&drop).unwrap();
        let res = store.list(&proj, None, 10).unwrap();
        let ids: Vec<_> = res.iter().map(|m| m.id.clone()).collect();
        assert_eq!(ids, vec![keep]);
    }
}
```
(Note: `excludes_archived` depends on `archive_memory` from Task 26; run that subtest after Task 26.)

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store list_tests::orders` Expected: FAIL (`list` stub).

- [ ] **Step 3: Minimal impl.** Implement `list` reusing `row_to_note` from Task 20:
```rust
fn list(&self, ns: &Namespace, min_importance: Option<u8>, limit: usize) -> Result<Vec<MemoryNote>> {
    let min = min_importance.unwrap_or(0) as i64;
    let mut stmt = self
        .conn
        .prepare(
            "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                    keywords, tags, context, memory_type, importance, confidence,
                    related_files, access_count, last_accessed_at, archived_at,
                    superseded_by, embedding_model
             FROM memories
             WHERE namespace = ?1
               AND archived_at IS NULL
               AND importance >= ?2
             ORDER BY created_at DESC
             LIMIT ?3",
        )
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut rows = stmt
        .query(rusqlite::params![ns.as_db_string(), min, limit as i64])
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
        out.push(row_to_note(&self.conn, row)?);
    }
    Ok(out)
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store list_tests::orders` Expected: PASS.

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement list with importance filter and created_at ordering"`

---

### Task 25: `update_memory` — apply MemoryUpdates, bump updated_at, keep FTS synced

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod update_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod update_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, MemoryUpdates, MemoryId};

    #[test]
    fn updates_fields_bumps_timestamp_and_syncs_fts() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let mut m = MemoryNote::new(proj.clone(), "original term".into(), MemoryType::Insight, 3);
        m.updated_at = m.updated_at - chrono::Duration::seconds(100);
        store.insert_memory(&m, None).unwrap();

        let updates = MemoryUpdates {
            content: Some("rewritten unicorn term".into()),
            summary: Some("new summary".into()),
            importance: Some(9),
            tags: Some(vec!["alpha".into(), "beta".into()]),
            context: Some("new context".into()),
        };
        store.update_memory(&m.id, &updates).unwrap();

        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.content, "rewritten unicorn term");
        assert_eq!(got.summary, "new summary");
        assert_eq!(got.importance, 9);
        assert_eq!(got.tags, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(got.context, "new context");
        assert!(got.updated_at.timestamp() > m.updated_at.timestamp());

        // FTS reflects new content, not old.
        let new_hits = store.keyword_search(&proj, "unicorn", 10).unwrap();
        assert_eq!(new_hits, vec![m.id.clone()]);
        let old_hits = store.keyword_search(&proj, "original", 10).unwrap();
        assert!(old_hits.is_empty());
    }

    #[test]
    fn partial_update_leaves_unset_fields() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(Namespace::Global, "keep me".into(), MemoryType::Reference, 4);
        m.summary = "keep summary".into();
        store.insert_memory(&m, None).unwrap();

        let updates = MemoryUpdates { importance: Some(7), ..Default::default() };
        store.update_memory(&m.id, &updates).unwrap();

        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert_eq!(got.importance, 7);
        assert_eq!(got.content, "keep me");
        assert_eq!(got.summary, "keep summary");
    }

    #[test]
    fn update_missing_is_ok_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let updates = MemoryUpdates { importance: Some(5), ..Default::default() };
        // No row affected; method must not error.
        store.update_memory(&MemoryId::new(), &updates).unwrap();
    }
}
```

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store update_tests` Expected: FAIL (`update_memory` stub).

- [ ] **Step 3: Minimal impl.** Implement `update_memory`. Build a dynamic SET clause; always bump `updated_at`. FTS stays in sync via the external-content `AFTER UPDATE OF content, summary, keywords, tags` trigger on `memories` defined in migration 001 (the standard delete-then-special-insert pattern); the end-to-end `updates_fields_bumps_timestamp_and_syncs_fts` test verifies this. If that UPDATE trigger is missing from the migration, the FTS sync assertion will fail — that is a migration (Task 18) defect, not a defect in this method:
```rust
fn update_memory(&self, id: &MemoryId, updates: &MemoryUpdates) -> Result<()> {
    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(content) = &updates.content {
        sets.push(format!("content = ?{}", params.len() + 1));
        params.push(Box::new(content.clone()));
    }
    if let Some(summary) = &updates.summary {
        sets.push(format!("summary = ?{}", params.len() + 1));
        params.push(Box::new(summary.clone()));
    }
    if let Some(importance) = updates.importance {
        sets.push(format!("importance = ?{}", params.len() + 1));
        params.push(Box::new(importance as i64));
    }
    if let Some(tags) = &updates.tags {
        sets.push(format!("tags = ?{}", params.len() + 1));
        params.push(Box::new(json_array(tags)?));
    }
    if let Some(context) = &updates.context {
        sets.push(format!("context = ?{}", params.len() + 1));
        params.push(Box::new(context.clone()));
    }

    // Always bump updated_at.
    sets.push(format!("updated_at = ?{}", params.len() + 1));
    params.push(Box::new(chrono::Utc::now().timestamp()));

    // WHERE memory_id bind comes last.
    let id_pos = params.len() + 1;
    params.push(Box::new(id.to_string()));

    let sql = format!(
        "UPDATE memories SET {} WHERE memory_id = ?{}",
        sets.join(", "),
        id_pos
    );

    let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    self.conn
        .execute(&sql, refs.as_slice())
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store update_tests` Expected: PASS (3 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 6: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement update_memory with FTS sync and updated_at bump"`

---

### Task 26: `archive_memory` — soft delete; excluded from searches

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod archive_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod archive_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, MemoryId};

    #[test]
    fn archive_sets_timestamp_and_excludes_from_searches() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let m = MemoryNote::new(proj.clone(), "searchable banana".into(), MemoryType::Reference, 6);
        let emb = [0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        store.insert_memory(&m, Some(&emb)).unwrap();

        // Visible before archive.
        assert_eq!(store.keyword_search(&proj, "banana", 10).unwrap(), vec![m.id.clone()]);
        assert!(!store.list(&proj, None, 10).unwrap().is_empty());

        store.archive_memory(&m.id).unwrap();

        // get_memory still returns it (with archived_at set) — archive is soft.
        let got = store.get_memory(&m.id).unwrap().unwrap();
        assert!(got.archived_at.is_some());

        // Excluded from keyword, vector, and list.
        assert!(store.keyword_search(&proj, "banana", 10).unwrap().is_empty());
        assert!(store.vector_search(&proj, &emb, 10).unwrap().is_empty());
        assert!(store.list(&proj, None, 10).unwrap().is_empty());
    }

    #[test]
    fn archive_missing_is_ok_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        store.archive_memory(&MemoryId::new()).unwrap();
    }
}
```

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store archive_tests` Expected: FAIL (`archive_memory` stub does not set `archived_at`).

- [ ] **Step 3: Minimal impl.** Implement `archive_memory`:
```rust
fn archive_memory(&self, id: &MemoryId) -> Result<()> {
    self.conn
        .execute(
            "UPDATE memories
             SET archived_at = ?1, updated_at = ?1
             WHERE memory_id = ?2 AND archived_at IS NULL",
            rusqlite::params![chrono::Utc::now().timestamp(), id.to_string()],
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store archive_tests` Expected: PASS (2 tests).

- [ ] **Step 5: Re-run dependent search tests now that archive exists.** Run: `cargo test -p rb-store keyword_tests::excludes_archived list_tests::excludes_archived` Expected: PASS.

- [ ] **Step 6: Lint + format.** Run: `cargo fmt --all` then `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 7: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement archive_memory soft delete with search exclusion"`

---

### Task 27: `add_link` — insert a single memory link

**Files:**
- Modify: `crates/rb-store/src/store.rs`
- Test: `crates/rb-store/src/store.rs` (inline `#[cfg(test)] mod add_link_tests`)

- [ ] **Step 1: Write the failing test.** Add to `crates/rb-store/src/store.rs`:
```rust
#[cfg(test)]
mod add_link_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace, MemoryLink, LinkType};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(Namespace::Project("rb".into()), c.into(), MemoryType::Entity, 5);
        store.insert_memory(&m, None).unwrap();
        m
    }

    #[test]
    fn add_link_persists_and_is_returned_by_get() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let b = node(&store, "b");

        let link = MemoryLink {
            source_id: a.id.clone(),
            target_id: b.id.clone(),
            link_type: LinkType::Supersedes,
            strength: 0.9,
            reason: "newer".into(),
            created_at: a.created_at,
        };
        store.add_link(&link).unwrap();

        let got = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.links[0].target_id, b.id);
        assert_eq!(got.links[0].link_type, LinkType::Supersedes);
        assert!((got.links[0].strength - 0.9).abs() < 1e-6);
        assert_eq!(got.links[0].reason, "newer");
    }

    #[test]
    fn add_link_to_missing_target_fails_fk() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "a");
        let link = MemoryLink {
            source_id: a.id.clone(),
            target_id: rb_types::MemoryId::new(),
            link_type: LinkType::References,
            strength: 0.5,
            reason: String::new(),
            created_at: a.created_at,
        };
        // foreign_keys=ON => FK violation surfaces as a storage error.
        let err = store.add_link(&link).unwrap_err();
        assert!(matches!(err, Error::Storage(_)));
    }
}
```

- [ ] **Step 2: Run it — expect failure.** Run: `cargo test -p rb-store add_link_tests` Expected: FAIL (`add_link` stub).

- [ ] **Step 3: Minimal impl.** Implement `add_link`:
```rust
fn add_link(&self, link: &MemoryLink) -> Result<()> {
    self.conn
        .execute(
            "INSERT INTO memory_links
                (source_id, target_id, link_type, strength, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                link.source_id.to_string(),
                link.target_id.to_string(),
                link.link_type.as_str(),
                link.strength as f64,
                link.reason,
                link.created_at.timestamp(),
            ],
        )
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 4: Run it — expect pass.** Run: `cargo test -p rb-store add_link_tests` Expected: PASS (2 tests).

- [ ] **Step 5: Run the graph module now that add_link exists.** Run: `cargo test -p rb-store graph_tests` Expected: PASS (2 tests).

- [ ] **Step 6: Full crate test sweep.** Run: `cargo test -p rb-store` Expected: PASS (all CRUD + search modules green).

- [ ] **Step 7: Lint + format.** Run: `cargo fmt --all` then `cargo clippy --workspace --all-targets -- -D warnings` Expected: no warnings.

- [ ] **Step 8: Commit.** Run: `git add -A && git commit -m "feat(rb-store): implement add_link and complete Store CRUD + search surface"`

## Part E — rb-store: integration & concurrency guards

### Task 28: Migration reproducibility gate — fresh DB exercises every query path

This is the direct guard against mnemosyne's "ghost migrations". The test builds a brand-new temp-file database *only* from committed migrations via `SqliteStore::open` (which also creates the dynamic `memory_vectors` table), then calls **every** method on the `Store` trait. If any column or table the code queries was never created, one of these calls returns `Error::Storage("... no such column/table ...")` and the test fails.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-store/tests/migration_reproducibility.rs`

- [ ] **Step 1: Write the failing test file.** Create `/Users/bluby/repos/rusty-brain/crates/rb-store/tests/migration_reproducibility.rs` with the complete contents below. It opens a fresh file DB (4-dim embeddings for speed), inserts two linked memories with vectors, then calls insert/get/keyword_search/vector_search/graph_neighbors/list/update/archive/add_link and asserts on each — including a negative FTS assertion that the stale token is gone after `update_memory` (catches external-content FTS5 desync, not just "new token present").

```rust
//! Migration reproducibility gate (anti-ghost-migration guard).
//!
//! Builds a FRESH temp-file database via `SqliteStore::open` (committed
//! migrations only + the dynamic vector table) and exercises EVERY query
//! path on the `Store` trait. If any column or table the code references is
//! missing, one of these calls fails and so does this test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_store::{SqliteStore, Store};
use rb_types::{
    LinkType, MemoryId, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace,
};

const DIM: usize = 4;

fn note(ns: &Namespace, content: &str, ty: MemoryType, importance: u8) -> MemoryNote {
    MemoryNote::new(ns.clone(), content.to_string(), ty, importance)
}

#[test]
fn fresh_db_exercises_every_query_path() {
    // Fresh, isolated file-backed DB built only from committed migrations.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("repro.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    let ns = Namespace::Project("repro".to_string());

    // --- insert_memory: memory + FTS row (via trigger) + vector + (no links yet)
    let mut a = note(&ns, "rusqlite WAL mode enables concurrent readers", MemoryType::Insight, 7);
    a.summary = "wal enables concurrent readers".to_string();
    a.keywords = vec!["wal".to_string(), "sqlite".to_string()];
    a.tags = vec!["db".to_string()];
    let emb_a: [f32; DIM] = [1.0, 0.0, 0.0, 0.0];
    store.insert_memory(&a, Some(&emb_a)).unwrap();

    let mut b = note(&ns, "sqlite-vec provides brute-force KNN search", MemoryType::Reference, 5);
    b.summary = "sqlite-vec knn".to_string();
    b.keywords = vec!["sqlite".to_string(), "vector".to_string()];
    b.tags = vec!["db".to_string(), "search".to_string()];
    let emb_b: [f32; DIM] = [0.0, 1.0, 0.0, 0.0];
    store.insert_memory(&b, Some(&emb_b)).unwrap();

    // --- get_memory: decode explicit columns + links
    let got = store.get_memory(&a.id).unwrap();
    assert!(got.is_some(), "inserted memory must be retrievable");
    let got = got.unwrap();
    assert_eq!(got.id, a.id);
    assert_eq!(got.content, a.content);
    assert_eq!(got.summary, a.summary);
    assert_eq!(got.keywords, a.keywords);
    assert_eq!(got.tags, a.tags);
    assert_eq!(got.memory_type, MemoryType::Insight);
    assert_eq!(got.importance, 7);
    assert!(got.archived_at.is_none(), "fresh memory is active");

    // get on a missing id returns Ok(None), not an error.
    let missing = MemoryId::new();
    assert!(store.get_memory(&missing).unwrap().is_none());

    // --- keyword_search: FTS5, scoped to ns, active only
    let kw = store.keyword_search(&ns, "concurrent", 10).unwrap();
    assert!(kw.contains(&a.id), "FTS must match content token 'concurrent'");
    assert!(!kw.contains(&b.id), "b does not contain 'concurrent'");

    // FTS over a summary/keyword column too.
    let kw2 = store.keyword_search(&ns, "knn", 10).unwrap();
    assert!(kw2.contains(&b.id), "FTS must match summary/keyword token 'knn'");

    // --- vector_search: sqlite-vec vec0 KNN; closest to emb_a is a.
    let hits = store.vector_search(&ns, &emb_a, 2).unwrap();
    assert!(!hits.is_empty(), "vector search returns candidates");
    assert_eq!(hits[0].0, a.id, "nearest neighbour of emb_a must be a");
    // distance is finite and non-negative.
    assert!(hits[0].1.is_finite() && hits[0].1 >= 0.0);

    // --- add_link + graph_neighbors: recursive CTE over memory_links
    let link = MemoryLink {
        source_id: a.id.clone(),
        target_id: b.id.clone(),
        link_type: LinkType::References,
        strength: 0.9,
        reason: "a cites b".to_string(),
        created_at: chrono::Utc::now(),
    };
    store.add_link(&link).unwrap();

    let neighbors = store.graph_neighbors(&a.id, 1).unwrap();
    assert!(neighbors.contains(&b.id), "graph_neighbors must reach b from a at depth 1");

    // --- list: active only, ORDER BY created_at DESC
    let listed = store.list(&ns, None, 10).unwrap();
    let listed_ids: Vec<_> = listed.iter().map(|m| m.id.clone()).collect();
    assert!(listed_ids.contains(&a.id) && listed_ids.contains(&b.id));

    // min_importance filter excludes the importance-5 note.
    let important = store.list(&ns, Some(6), 10).unwrap();
    let important_ids: Vec<_> = important.iter().map(|m| m.id.clone()).collect();
    assert!(important_ids.contains(&a.id));
    assert!(!important_ids.contains(&b.id), "importance 5 < 6 must be excluded");

    // --- update_memory: bumps updated_at, keeps FTS in sync
    let updates = MemoryUpdates {
        content: Some("updated: WAL plus sqlite-vec in one transaction".to_string()),
        summary: Some("updated summary".to_string()),
        importance: Some(9),
        tags: Some(vec!["db".to_string(), "updated".to_string()]),
        context: Some("ctx".to_string()),
    };
    store.update_memory(&a.id, &updates).unwrap();
    let after = store.get_memory(&a.id).unwrap().unwrap();
    assert_eq!(after.content, "updated: WAL plus sqlite-vec in one transaction");
    assert_eq!(after.importance, 9);
    assert!(after.updated_at >= after.created_at, "updated_at must be bumped");
    // FTS reflects the NEW content: searching a new token finds a.
    let kw_after = store.keyword_search(&ns, "transaction", 10).unwrap();
    assert!(kw_after.contains(&a.id), "FTS must reflect updated content");
    // FTS desync guard: the OLD token must be removed from a's row, not merely
    // shadowed by a new one. An external-content FTS5 update that inserts the
    // new row without deleting the stale one would still satisfy the assertion
    // above but FAIL here.
    let kw_stale = store.keyword_search(&ns, "concurrent", 10).unwrap();
    assert!(
        !kw_stale.contains(&a.id),
        "stale FTS token 'concurrent' must be removed when a's content is updated"
    );

    // --- archive_memory: soft delete; dropped from active list + keyword search
    store.archive_memory(&b.id).unwrap();
    let active_after = store.list(&ns, None, 10).unwrap();
    let active_ids: Vec<_> = active_after.iter().map(|m| m.id.clone()).collect();
    assert!(!active_ids.contains(&b.id), "archived memory absent from active list");
    let kw_archived = store.keyword_search(&ns, "knn", 10).unwrap();
    assert!(!kw_archived.contains(&b.id), "archived memory absent from keyword search");
    // But still fetchable directly with archived_at set.
    let b_archived = store.get_memory(&b.id).unwrap().unwrap();
    assert!(b_archived.archived_at.is_some(), "archived_at column must be set");
}
```

- [ ] **Step 2: Run it and watch it fail (or fail to compile) before deps are wired.** Run: `cargo test -p rb-store --test migration_reproducibility -- --nocapture` Expected: FAIL — either a compile error if `tempfile`/`chrono` dev-deps are not yet present, or, once the crate compiles, a `no such column`/`no such table` failure if any migration is incomplete. This is the gate doing its job.

- [ ] **Step 3: Ensure dev-dependencies are present.** Confirm `crates/rb-store/Cargo.toml` has under `[dev-dependencies]`: `tempfile = { workspace = true }` and `chrono = { workspace = true }`. If absent, add them. (These were declared in `[workspace.dependencies]` per the spine; chrono is needed for `MemoryLink.created_at` in the test.)

- [ ] **Step 4: Re-run the gate against the real implementation.** Run: `cargo test -p rb-store --test migration_reproducibility -- --nocapture` Expected: PASS — `test fresh_db_exercises_every_query_path ... ok`. If it fails with `no such column: archived_at` (or any table), the migration file `crates/rb-store/migrations/001_initial_schema.sql` or the dynamic `memory_vectors` creation in `SqliteStore::open` is incomplete; fix the schema, not the test. If the FTS desync assertion fails, `update_memory` must delete the old FTS row before inserting the new one (external-content FTS5 `'delete'` command), not just insert; fix `update_memory`, not the test.

- [ ] **Step 5: Lint and format.** Run: `cargo fmt --all` then `cargo clippy --workspace --all-targets -- -D warnings` Expected: both clean (the `#![allow(clippy::unwrap_used, clippy::expect_used)]` at the top of the test file keeps the workspace deny lints satisfied for test code).

- [ ] **Step 6: Commit.** Run: `git add crates/rb-store/tests/migration_reproducibility.rs crates/rb-store/Cargo.toml && git commit -m "test(rb-store): ghost-migration gate exercises every query path on a fresh DB"` Expected: one commit created.

---

### Task 29: Concurrency gate — one writer + N readers on one WAL file, no Busy, no lost writes

Opens one writer `SqliteStore` plus 8 reader `SqliteStore` instances on the **same** temp-file path (WAL mode lets readers and a writer share the file). Reader threads poll `list`/`keyword_search`/`vector_search` continuously while the writer inserts `M` memories. Asserts: no reader or writer call ever returns a `SQLITE_BUSY`-class error, and all `M` memories are eventually readable (no lost writes).

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-store/tests/concurrency.rs`

- [ ] **Step 1: Write the failing concurrency test.** Create `/Users/bluby/repos/rusty-brain/crates/rb-store/tests/concurrency.rs` with the complete contents below. Each `SqliteStore` owns its own connection; sharing is via the file + WAL, exactly the daemon's reader-pool / single-writer arrangement at the storage layer.

```rust
//! Concurrency gate: single writer + N concurrent readers on one WAL file.
//!
//! Mirrors the daemon's storage-layer access pattern (one write connection,
//! many read connections, same DB file, WAL mode). Asserts no SQLITE_BUSY
//! surfaces and that every committed write is eventually readable.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace};

const DIM: usize = 4;
const READERS: usize = 8;
const WRITES: usize = 200;

/// True if a storage error looks like a SQLite busy/locked contention error.
/// WAL + a single writer must make these impossible; any occurrence fails the test.
fn is_busy(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("busy") || m.contains("database is locked") || m.contains("locked")
}

#[test]
fn single_writer_many_readers_no_busy_no_lost_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("concurrency.db");

    // Writer connection (created first so the file + schema + WAL exist).
    let writer = SqliteStore::open(&db_path, DIM).unwrap();

    let ns = Namespace::Project("conc".to_string());

    // Shared signals.
    let stop = Arc::new(AtomicBool::new(false));
    let busy_seen = Arc::new(AtomicUsize::new(0));

    // Spawn N reader threads, each with its OWN SqliteStore on the same file.
    let mut reader_handles = Vec::with_capacity(READERS);
    for _ in 0..READERS {
        let path = db_path.clone();
        let ns = ns.clone();
        let stop = Arc::clone(&stop);
        let busy_seen = Arc::clone(&busy_seen);
        let handle = thread::spawn(move || {
            // Reader opens its own connection; WAL allows concurrent reads.
            let reader = SqliteStore::open(&path, DIM).unwrap();
            let probe: [f32; DIM] = [0.25, 0.25, 0.25, 0.25];
            while !stop.load(Ordering::Relaxed) {
                if let Err(e) = reader.list(&ns, None, 50) {
                    if is_busy(&e.to_string()) {
                        busy_seen.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if let Err(e) = reader.keyword_search(&ns, "memory", 50) {
                    if is_busy(&e.to_string()) {
                        busy_seen.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if let Err(e) = reader.vector_search(&ns, &probe, 5) {
                    if is_busy(&e.to_string()) {
                        busy_seen.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });
        reader_handles.push(handle);
    }

    // Writer thread: serialized inserts through the single write connection.
    let write_busy = Arc::clone(&busy_seen);
    let write_ns = ns.clone();
    let writer_handle = thread::spawn(move || {
        for i in 0..WRITES {
            let content = format!("memory note number {i} about concurrent access");
            let mut note = MemoryNote::new(write_ns.clone(), content, MemoryType::Insight, 5);
            note.summary = format!("note {i}");
            note.keywords = vec!["memory".to_string(), "concurrent".to_string()];
            let emb: [f32; DIM] = [i as f32, 0.0, 0.0, 1.0];
            if let Err(e) = writer.insert_memory(&note, Some(&emb)) {
                if is_busy(&e.to_string()) {
                    write_busy.fetch_add(1, Ordering::Relaxed);
                } else {
                    panic!("unexpected writer error: {e}");
                }
            }
        }
    });

    // Wait for the writer to finish, then stop readers.
    writer_handle.join().unwrap();
    stop.store(true, Ordering::Relaxed);
    for h in reader_handles {
        h.join().unwrap();
    }

    // Assertion 1: no busy/locked errors anywhere.
    assert_eq!(
        busy_seen.load(Ordering::Relaxed),
        0,
        "WAL + single writer must yield zero SQLITE_BUSY/locked errors"
    );

    // Assertion 2: no lost writes. A fresh reader sees all WRITES rows.
    // Poll briefly to allow WAL visibility to settle across connections.
    let verifier = SqliteStore::open(&db_path, DIM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let count = loop {
        let n = verifier.list(&ns, None, WRITES + 10).unwrap().len();
        if n >= WRITES || Instant::now() >= deadline {
            break n;
        }
        thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(count, WRITES, "all {WRITES} writes must be readable (no lost writes)");
}
```

- [ ] **Step 2: Run it and watch it pass (or surface a real defect).** Run: `cargo test -p rb-store --test concurrency -- --nocapture` Expected: PASS — `test single_writer_many_readers_no_busy_no_lost_writes ... ok`. If it fails on `busy_seen != 0`, `SqliteStore::open` is likely not setting `PRAGMA journal_mode=WAL` (the spine requirement), or contention is surfacing before SQLite can serialize it; fix `open` (ensure WAL is actually applied, and add a `busy_timeout` if a writer-side checkpoint contends). If `count != WRITES`, a write path is dropping rows under contention.

- [ ] **Step 3: Stress for flakiness.** Run: `cargo test -p rb-store --test concurrency -- --nocapture --test-threads=1` then repeat the run 3 times. Expected: PASS every time. A single Busy in any run is a hard failure — re-check that `open` actually applies `PRAGMA journal_mode=WAL` and consider a `busy_timeout` to absorb transient checkpoint contention.

- [ ] **Step 4: Lint and format.** Run: `cargo fmt --all` then `cargo clippy --workspace --all-targets -- -D warnings` Expected: both clean.

- [ ] **Step 5: Commit.** Run: `git add crates/rb-store/tests/concurrency.rs && git commit -m "test(rb-store): concurrency gate — single writer + 8 readers on WAL, no busy, no lost writes"` Expected: one commit created.

---

### Task 30: Namespace isolation gate — scoped reads never cross namespaces

Inserts memories under `Namespace::Project("a")` and `Namespace::Project("b")` into the same DB. Asserts that `keyword_search` and `list` scoped to `"a"` never return any `"b"` rows (and vice versa). This is the storage-layer half of the §8 server-side isolation guarantee.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain/crates/rb-store/tests/namespace_isolation.rs`

- [ ] **Step 1: Write the failing isolation test.** Create `/Users/bluby/repos/rusty-brain/crates/rb-store/tests/namespace_isolation.rs` with the complete contents below.

```rust
//! Namespace isolation gate (storage layer).
//!
//! Memories in different namespaces share one DB; scoped queries must never
//! leak rows across namespaces. This is the storage-layer guarantee that the
//! daemon's server-side isolation (spec §8) builds on.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use rb_store::{SqliteStore, Store};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

const DIM: usize = 4;

fn insert(
    store: &SqliteStore,
    ns: &Namespace,
    content: &str,
    keyword: &str,
    emb: [f32; DIM],
) -> MemoryId {
    let mut note = MemoryNote::new(ns.clone(), content.to_string(), MemoryType::Insight, 5);
    note.summary = content.to_string();
    note.keywords = vec![keyword.to_string()];
    note.tags = vec![keyword.to_string()];
    let id = note.id.clone();
    store.insert_memory(&note, Some(&emb)).unwrap();
    id
}

#[test]
fn scoped_queries_never_cross_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("isolation.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());

    // Same shared token "deployment" in both namespaces to force any leak to show.
    let a1 = insert(&store, &ns_a, "alpha deployment rollback plan", "deployment", [1.0, 0.0, 0.0, 0.0]);
    let a2 = insert(&store, &ns_a, "alpha config deployment notes", "deployment", [0.9, 0.1, 0.0, 0.0]);
    let b1 = insert(&store, &ns_b, "beta deployment incident review", "deployment", [0.0, 1.0, 0.0, 0.0]);
    let b2 = insert(&store, &ns_b, "beta deployment runbook", "deployment", [0.0, 0.9, 0.1, 0.0]);

    let a_ids: HashSet<MemoryId> = [a1.clone(), a2.clone()].into_iter().collect();
    let b_ids: HashSet<MemoryId> = [b1.clone(), b2.clone()].into_iter().collect();

    // --- list scoped to "a" returns only a-rows.
    let list_a: HashSet<MemoryId> = store.list(&ns_a, None, 50).unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(list_a, a_ids, "list(a) must return exactly the a-namespace rows");
    assert!(list_a.is_disjoint(&b_ids), "list(a) must not contain any b-namespace rows");

    // --- list scoped to "b" returns only b-rows.
    let list_b: HashSet<MemoryId> = store.list(&ns_b, None, 50).unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(list_b, b_ids, "list(b) must return exactly the b-namespace rows");
    assert!(list_b.is_disjoint(&a_ids), "list(b) must not contain any a-namespace rows");

    // --- keyword_search scoped to "a" for the shared token returns only a-rows.
    let kw_a: HashSet<MemoryId> = store.keyword_search(&ns_a, "deployment", 50).unwrap().into_iter().collect();
    assert!(!kw_a.is_empty(), "keyword_search(a, 'deployment') must match a-rows");
    assert!(kw_a.is_subset(&a_ids), "keyword_search(a) must only return a-namespace ids");
    assert!(kw_a.is_disjoint(&b_ids), "keyword_search(a) must never return b-namespace ids");

    // --- keyword_search scoped to "b" for the shared token returns only b-rows.
    let kw_b: HashSet<MemoryId> = store.keyword_search(&ns_b, "deployment", 50).unwrap().into_iter().collect();
    assert!(!kw_b.is_empty(), "keyword_search(b, 'deployment') must match b-rows");
    assert!(kw_b.is_subset(&b_ids), "keyword_search(b) must only return b-namespace ids");
    assert!(kw_b.is_disjoint(&a_ids), "keyword_search(b) must never return a-namespace ids");
}

#[test]
fn distinct_project_namespaces_do_not_share_rows() {
    // Guards against namespace db-string collisions: "project:a" != "project:b".
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("isolation2.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());

    let only_a = insert(&store, &ns_a, "unique alpha marker token", "alphaonly", [1.0, 0.0, 0.0, 0.0]);

    // Searching b for a token that exists only in a must return nothing.
    let leaked = store.keyword_search(&ns_b, "alphaonly", 50).unwrap();
    assert!(leaked.is_empty(), "b must not see a's unique token");

    // And a still finds it.
    let found = store.keyword_search(&ns_a, "alphaonly", 50).unwrap();
    assert!(found.contains(&only_a), "a must find its own unique token");
}
```

- [ ] **Step 2: Run it.** Run: `cargo test -p rb-store --test namespace_isolation -- --nocapture` Expected: PASS — both `scoped_queries_never_cross_namespaces ... ok` and `distinct_project_namespaces_do_not_share_rows ... ok`. If either fails by returning the wrong namespace's rows, the `keyword_search`/`list` SQL is missing its `WHERE namespace = ?` clause (using `Namespace::as_db_string`); fix the store SQL, not the test.

- [ ] **Step 3: Lint and format.** Run: `cargo fmt --all` then `cargo clippy --workspace --all-targets -- -D warnings` Expected: both clean.

- [ ] **Step 4: Run the full rb-store integration suite together.** Run: `cargo test -p rb-store --tests` Expected: PASS — `migration_reproducibility`, `concurrency`, and `namespace_isolation` all green. This is the assembled anti-mnemosyne gate set.

- [ ] **Step 5: Commit.** Run: `git add crates/rb-store/tests/namespace_isolation.rs && git commit -m "test(rb-store): namespace isolation gate — scoped reads never cross namespaces"` Expected: one commit created.


---

## P1–P4 Outline (expanded into full bite-sized plans as each phase begins)

> Intentionally NOT broken into bite-sized steps yet. Per the writing-plans discipline, each phase gets its own complete plan when reached, so deferred work never becomes speculative placeholder. Listed here to lock the crate/seam boundaries and the roadmap.

### P1 — Core engine + daemon
- `rb-embed`: `EmbeddingProvider` trait + Voyage remote impl (reqwest, request timeout, bounded concurrency); `dim()` checked against `meta.embedding_dim`. `local` ONNX behind a feature.
- `rb-search`: pure, unit-tested hybrid ranking over store candidates (vector 0.5 / keyword 0.3 / graph-importance-recency 0.2, configurable).
- `rb-engine`: single-request orchestration (namespace resolve → embed → store → link → search); minimal heuristic enrichment in P1, LLM enrichment opt-in in P2.
- `rb-proto`: daemon wire protocol (request/response enums) + UDS client + length-delimited JSON framing + `ContractVersion` handshake.
- `rb-daemon`: single dedicated writer thread + mpsc write queue, deadpool read pool (WAL), UDS listener, `tokio::broadcast` change events, server-side namespace isolation, pidfile single-instance, graceful shutdown (drain queue, WAL checkpoint).
- `rusty-brain` bin: `serve` + client subcommands (remember/recall/get/list/...).
- Tests: daemon concurrency (N clients, no SQLITE_BUSY, no lost writes), isolation enforced server-side, proto round-trip, embedding-dim contract.

### P2 — Agent surface
- `mcp` subcommand: thin MCP stdio server translating tools → proto requests; daemon auto-start.
- Namespace detection (git root / `CLAUDE.md`).
- Graph links + traversal wired into `recall`/`graph`; optional LLM enrichment.
- MCP contract tests per tool (schema + error mapping).

### P3 — Deferred (behind existing seams)
- `subscribe` change-stream (cross-agent awareness) over the broadcast channel.
- Memory evolution (consolidation / link decay / importance recalibration) as opt-in daemon jobs.
- `local` embedding feature.

### P4 — Broader agent surface (deferred)
- `rb-hooks` / `rb-install`: capture hooks + an `install` command configuring Claude Code, OpenCode, Copilot CLI, Codex CLI, Gemini CLI to use the daemon — **fail-open**, `ContractVersion`-gated. Never compiled into core. (Ported in spirit from rusty-brain-old.)


---

## Plan provenance

Authored by a 5-cluster fan-out against a fixed interface spine; each cluster adversarially reviewed for placeholders, spine/type drift, and Rust/SQL correctness. Reviewer-confirmed flags (per cluster): workspace-ci (7 fixed); rb-types (5 fixed); store-migrations (7 fixed); store-crud (10 fixed); store-integration (5 fixed). Tasks renumbered globally for sequential execution.
