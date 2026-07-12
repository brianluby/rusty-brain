# Follow-up: `SqliteStore` lifecycle concurrency + integrity hardening

- **Date:** 2026-07-11
- **Area:** `rb-store` connection lifecycle (`crates/rb-store/src/store/lifecycle.rs`, `crates/rb-store/src/store/scrub.rs`)
- **Status:** Deferred (design decisions needed; not fixed as part of PR #54)
- **Severity:** Major per CodeRabbit's PR #54 review; not exploitable today under the
  documented single-writer deployment path, but real gaps in the functions'
  own guarantees.

## Summary

CodeRabbit's review of PR #54 (the `store.rs` module split) surfaced three
pre-existing issues in code that PR #54 moved verbatim — none introduced by
the split itself, all real. Two closely related, lower-risk findings from the
same review (model-check ordering vs. vector-schema mutation, and a
dim/model meta-seeding race) were fixed directly in PR #54 since they had
safe, minimal, pattern-consistent fixes. These three did not, and are
tracked here instead of being rushed into a refactor PR.

## 1. `ensure_vector_schema` reads its create-vs-rebuild decision outside the lock

`crates/rb-store/src/store/lifecycle.rs`, `ensure_vector_schema`: the
`vector_schema_version` meta read and the `table_exists` check both happen
*before* `immediate_tx` acquires `BEGIN IMMEDIATE`. Two concurrent first
opens of the same fresh DB file could both observe "no table," then both
attempt `CREATE VIRTUAL TABLE memory_vectors` inside their own transaction —
the second fails with "already exists."

**Why not fixed now:** the function's own doc comment states the actual
deployment path is protected by an external invariant — `StoreHandle::
start_inner` sequences the writer's `SqliteStore::open` before the read pool
opens, so opens are single-flighted by construction in the daemon. The
naive fix (move both reads inside the `immediate_tx` closure) would make
*every* open take an immediate write-lock even on the fast path (schema
already current, zero writes needed) — a real perf/contention tradeoff that
deserves a benchmark before landing, not a reflexive change.

**What "done" looks like:** either (a) benchmark the cost of locking the fast
path and accept it if negligible, or (b) make the function self-contained
race-safe without penalizing the no-op path — e.g. an optimistic read
outside the lock, re-validated inside the lock only when a write is about to
happen. Either way, add a test that exercises two concurrent `SqliteStore::
open` calls against the same fresh DB file.

## 2. `seed_or_verify_model` silently adopts the configured model when the marker is absent but rows already exist

`crates/rb-store/src/store/lifecycle.rs`, `seed_or_verify_model`: on a DB
that predates the `meta.embedding_model` key but already has memory rows
(each row stamps its own `memories.embedding_model`), this function just
writes whatever model is *currently configured* as the new marker — without
checking whether it matches the model already recorded on existing rows. A
same-dimension provider swap on such a DB would pass the invariant check
unnoticed, exactly the failure mode this function's own doc comment says it
exists to prevent.

**Why not fixed now:** the correct remediation policy is a product decision,
not just a code fix — reject on any disagreement among existing rows?
require the caller to go through `accept_model_change` explicitly? auto-
reconcile if all existing rows agree with each other (even if none match the
newly configured model)? Each has different UX and migration implications.

**What "done" looks like:** decide the policy, then have `seed_or_verify_model`
query `SELECT DISTINCT embedding_model FROM memories` before seeding an
absent marker, and either fail closed on disagreement or route through the
existing `accept_model_change` flow. Needs a test seeding rows under model A,
opening with no marker and configured model B, and asserting the chosen
policy.

## 3. `scrub`'s WAL checkpoint discards the `(busy, log, checkpointed)` result

**Resolved (Vikunja #53):** scrub now returns the checkpoint result through
the daemon protocol, human and JSON CLI surfaces warn on a busy or unavailable
status, and a no-change rerun retries `TRUNCATE` after blocking readers close.
A concurrent-reader regression test pins both the busy result and successful
retry.

`crates/rb-store/src/store/scrub.rs`, `scrub`: the post-redaction
`PRAGMA wal_checkpoint(TRUNCATE)` runs via `execute_batch`, which discards
the pragma's result row. A blocked checkpoint (e.g. a long-lived reader
holding the WAL) can leave pre-redaction plaintext sitting in `-wal` while
`scrub()` still reports success (`ScrubOutcome`), a real gap against the
threat model's redaction guarantee.

**Why not fixed now:** the crate deliberately carries no logging facility
(see the comment on `rebuild_vector_table`'s stats: "rb-store carries no
logging facility, so the one-shot counts live in meta"), so "just log a
warning" isn't available. Properly surfacing this means either adding a
field to the public `ScrubOutcome` struct (an API change requiring CLI-layer
wiring so `rusty-brain scrub` can warn the operator) or persisting a durable
`meta` marker in the `rebuild_vector_table`-stats style. Either is more than
a quick patch and needs a test that provokes a busy checkpoint (a concurrent
reader holding the WAL during `scrub()`).

**What "done" looks like:** switch to `query_row` over the pragma, add
`ScrubOutcome.wal_checkpoint_busy: bool` (or similar), wire the CLI `scrub`
command to warn when set, and add a test that holds a concurrent read
transaction open during `scrub()` and asserts the flag comes back `true`
while confirming `scrub()` still succeeds (best-effort, not a hard failure).

## Provenance

Surfaced by CodeRabbit's automated review of PR #54 (`chore/store-module-split`),
2026-07-11. Findings verified by hand against the actual (moved, unmodified)
logic before deferring; two sibling findings from the same review pass
(model-check-before-schema-mutation ordering, dim/model meta-seed race) were
fixed directly in that PR since they had safe, minimal, existing-pattern
fixes (`seed_or_get_site_id`'s `INSERT OR IGNORE` + re-read idiom).
