# Research: 001 Project Bootstrap

**Created**: 2026-03-01
**Status**: Complete

## R-001: memvid Dependency Sourcing

**Decision**: Git dependency from `https://github.com/brianluby/memvid/`, pinned to commit SHA `fbddef4bff6ac756f91724681234243e98d5ba04`.

**Rationale**: No tags exist in the repository. Pinning to a specific commit SHA provides reproducible builds. The crate name is `memvid-core` (v2.0.137), uses edition 2024, and requires Rust 1.85.0+ — fully aligned with our workspace settings.

**Alternatives considered**:
- Branch pin (`main`): Less reproducible, builds could break on upstream changes.
- Path dependency: Requires local clone management, complicates CI.
- crates.io: Not published there.

**Dependency overlap with workspace**: serde 1.0.228, serde_json 1.0.145, chrono 0.4.42, uuid 1.10.0, tracing 0.1.41. All are standard compatible versions — minimal conflict risk.

## R-002: Rust Edition 2024 & MSRV

**Decision**: Edition 2024 with MSRV 1.85.0. Current stable Rust is 1.93.1.

**Rationale**: Edition 2024 was stabilized in Rust 1.85.0 (February 20, 2025). The memvid-core crate also targets edition 2024 with MSRV 1.85.0, ensuring alignment.

**Key breaking changes from edition 2021**:
- `unsafe` operations in unsafe functions now require explicit inner `unsafe` blocks.
- `impl Trait` lifetime handling stricter (previously assumed `'static`).
- `gen` is a reserved keyword (use `r#gen` if needed).
- MSRV resolver enabled by default in Cargo.

**Alternatives considered**:
- Edition 2021: Would misalign with memvid-core and miss 2024 improvements.
- MSRV > 1.85.0: Unnecessary restriction for Phase 0.

## R-003: Cargo Workspace Layout

**Decision**: Virtual manifest with `crates/` subdirectory, resolver 3, workspace dependency/metadata/lints inheritance.

**Rationale**: Industry standard for multi-crate Rust projects. Virtual manifest avoids root-level src pollution. Resolver 3 is required for virtual workspaces in edition 2024.

**Key patterns**:
- Root `Cargo.toml` uses `[workspace]` with `members = ["crates/*"]` and `resolver = "3"`.
- Shared deps in `[workspace.dependencies]`, inherited via `{ workspace = true }`.
- Shared metadata in `[workspace.package]` (edition, rust-version, license, etc.).
- Shared lints in `[workspace.lints.clippy]` / `[workspace.lints.rust]`, inherited via `[lints] workspace = true`.

**Alternatives considered**:
- Flat root members (no `crates/` dir): Clutters root at scale.
- Non-virtual manifest: Adds unnecessary root package.

## R-004: CI Pipeline & Caching

**Decision**: GitHub Actions with `Swatinem/rust-cache@v2` for Cargo caching, `dtolnay/rust-toolchain@stable` for toolchain setup.

**Rationale**: Swatinem/rust-cache is the de facto standard for Rust CI caching. It automatically handles `~/.cargo` registry/git deps and workspace `target/` directories. Sets `CARGO_INCREMENTAL=0` for CI efficiency.

**CI workflow structure**:
- 4 quality gates: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `cargo build --workspace --release`.
- Triggered on push to any branch + pull requests.
- Separate cache keys for test vs. release builds.

**Alternatives considered**:
- `actions/cache` manual setup: More configuration, same result.
- sccache: Overkill for Phase 0 workspace size.
