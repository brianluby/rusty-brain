# Vector schema initialization race and fast-path benchmark

Date: 2026-07-12  
Task: Vikunja #506 (rusty-brain project task #54)  
Base: `9e8312d`  
Platform: macOS 26.5.2 arm64, rustc 1.96.0

## Decision

Make each write-bearing stage of a first open single-winner without adding a
write transaction to current-schema opens:

1. Install the existing five-second busy handler before journal-mode setup.
   Retry only SQLite `BUSY`/`LOCKED` results from the zero-byte WAL transition,
   bounded by the same deadline.
2. Keep the migration ledger's optimistic read. Only an unseen migration takes
   `BEGIN IMMEDIATE`; after acquiring it, re-read the ledger and either validate
   the concurrent winner's checksum or apply SQL plus its ledger row atomically.
3. Keep the existing optimistic `meta.vector_schema_version` read. Only a
   missing or outdated marker takes `BEGIN IMMEDIATE`, then re-read the marker
   and table state before choosing create or rebuild.

The revalidations remove both stale decisions observed before this fix: replay
of already-applied migration DDL (`duplicate column name`) and duplicate
`CREATE VIRTUAL TABLE`. Migration SQL plus its ledger row remains one atomic
transaction; vector create/rebuild plus both schema markers also still commits
or rolls back together.

## Deterministic race coverage

`concurrent_zero_byte_public_opens_agree_on_all_markers` starts with no database
file. A path-scoped test barrier pauses both SQLite connections immediately
after `Connection::open`; both threads then call the public
`SqliteStore::open_with_model` path through WAL negotiation, all migrations,
marker seeding, and vector-table creation. Both opens must succeed and report
identical values for:

- `embedding_dim`
- `embedding_model`
- `vector_schema_version`
- `vector_metric`
- `site_id`

Before the complete fix, stress probes failed with `database is locked`,
`duplicate column name: base_strength`, or
`table memory_vectors already exists`, depending on which stale first-open
decision lost the race. The public zero-byte regression passed 100 consecutive
debug-profile runs after all three initialization stages were hardened.

The narrower
`concurrent_fresh_vector_schema_opens_agree_on_all_markers` test remains to
force both connections past the vector marker miss specifically, independent
of migration scheduling.

## Current-schema benchmark

The ignored `vector_schema_current_open_benchmark` is a release-mode
microbenchmark of the schema portion of open, not an end-to-end store-open
benchmark. It compares the shipped optimistic marker check with the alternative
of taking `BEGIN IMMEDIATE`, reading the same marker, and committing on every
current-schema open. It warms both paths, alternates their order, and reports
the median of seven samples with 20,000 iterations per sample.

Reproduce with:

```text
cargo test --release -p rb-store vector_schema_current_open_benchmark -- --ignored --nocapture
```

Task runs (three invocations of the seven-sample benchmark, including the
post-zero-byte-hardening rerun):

```text
run 1: optimistic=1793.7 ns/op, forced-immediate=2984.8 ns/op, ratio=1.66x
run 2: optimistic=1678.5 ns/op, forced-immediate=2550.6 ns/op, ratio=1.52x
run 3: optimistic=1597.7 ns/op, forced-immediate=2494.2 ns/op, ratio=1.56x
```

This result supports keeping the write lock off the no-op path. Absolute
nanosecond values are machine-specific; the checked-in benchmark is the durable
artifact for rerunning the comparison on release hardware.
