# Vector schema initialization race and fast-path benchmark

Date: 2026-07-12  
Task: Vikunja #506 (rusty-brain project task #54)  
Base: `9e8312d`  
Platform: macOS 26.5.2 arm64, rustc 1.96.0

## Decision

Keep the existing optimistic `meta.vector_schema_version` read for the common
current-schema open. Only a missing or outdated marker acquires
`BEGIN IMMEDIATE`. Once it owns that write lock, the slow path re-reads the
marker and checks whether `memory_vectors` exists before choosing create or
rebuild.

The revalidation removes the race where two openers both cached “table absent”
and the loser later attempted a duplicate `CREATE VIRTUAL TABLE`. It also
preserves the existing crash guarantee: create/rebuild and both schema markers
commit or roll back together.

## Deterministic race coverage

`concurrent_fresh_vector_schema_opens_agree_on_all_markers` starts from a fully
migrated database with no dynamic vector table or vector-schema marker. A
barrier in the real file-open path forces both connections past the optimistic
marker miss before either may take the initialization lock. Both opens must
succeed and report identical values for:

- `embedding_dim`
- `embedding_model`
- `vector_schema_version`
- `vector_metric`
- `site_id`

The regression test failed before the fix with
`storage error: table memory_vectors already exists`. It passed ten consecutive
debug-profile runs after the under-lock revalidation was added.

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

Task runs (two invocations of the seven-sample benchmark):

```text
run 1: optimistic=1793.7 ns/op, forced-immediate=2984.8 ns/op, ratio=1.66x
run 2: optimistic=1678.5 ns/op, forced-immediate=2550.6 ns/op, ratio=1.52x
```

This result supports keeping the write lock off the no-op path. Absolute
nanosecond values are machine-specific; the checked-in benchmark is the durable
artifact for rerunning the comparison on release hardware.
