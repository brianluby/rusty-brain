# PRD: "Memory Is Working" Observability (`rusty-brain doctor` / `stats`)

## Status

Delivered 2026-07-11 (PR #56, merge `33a2218`, Vikunja #460). `status` and
`stats` expose the daemon/writer, corpus, feedback, recall, contention,
retention, WAL, and embedding-model signals in human and JSON forms;
`doctor` diagnoses the live system without auto-starting or writing and exits
non-zero on failed checks. Store aggregation lives in
`crates/rb-store/src/store/stats.rs`, and binary e2e tests cover the value
signals plus permission/model failures.

Residual scope is explicit: aggregation cost has correctness coverage but no
large-corpus benchmark; doctor reports the model served by a reachable daemon
but does not independently probe a remote provider; and it does not yet invoke
the separately delivered `rusty-brain-install status` hook-wiring report. The
DB parent directory is intentionally not checked because it may be
caller-owned; socket directory ownership is checked when present.

## Owner Area

Primary: read-only aggregation surfaces over existing counters and tables.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (extend `Status`; add `Doctor`/`Stats`)
- `crates/rusty-brain/src/run.rs`
- `crates/rusty-brain/src/output.rs`
- `crates/rb-daemon/src/server.rs`
- `crates/rb-store/src/store/stats.rs`
- `crates/rb-proto/src/messages.rs` (additive `Stats` request/response)
- `crates/rb-types/src/feedback_kind.rs` (feedback aggregates)
- `crates/rb-daemon/src/change.rs` (oplog-derived activity)

## Problem

`memory_feedback` (W3.7) and `access_count` accumulate signals, but nothing
surfaces them. There is no view of recall volume, helpful/wrong/stale ratios,
never-recalled memories, contested count, or corpus growth. The user has no
visible proof the memory is earning its keep, which is the #1 retention risk.
The roadmap measures recall quality at the *system* level (W3.5 scorecard)
but the *user* sees nothing.

## Goals

- A fast, read-only, no-writer-ops (W1.8 invariant) stats surface.
- Answer "is my memory helping?" with aggregate feedback + recall numbers.
- Answer "is my brain healthy?" with daemon/DB/embedding-provider health.
- JSON-first (scriptable), human-readable second, TUI later.

## Non-Goals

- Do not write on the read path (respect W1.8).
- Do not build a GUI; a TUI is explicitly a later phase.
- Do not add new persisted counters; aggregate from what exists.
- Do not gate releases on stats thresholds (that is the W3.5 scorecard's job).

## Functional Requirements

### DOC-1. Extend `status`

Today `status` pings the daemon and reports the contract version. Extend it
to report, in one payload: daemon up/down, writer health, embedding provider
+ model identity, DB path + file mode, WAL size, vector count, live vs
archived memory count, namespace.

### DOC-2. `rusty-brain stats`

A value/health aggregate over the current namespace:

- recall volume (from oplog change events or access bumps) over a window.
- feedback ratio: helpful vs wrong vs stale counts (from the `memory_feedback`
  table), and net trust trend.
- top-recalled memory ids; never-recalled live memory count.
- contested count (active `contradicts` links).
- corpus growth over time (rows by `created_at` bucket).
- stale-vector / reembed-needed count (reuse the stale-stamp scan).

### DOC-3. `rusty-brain doctor`

A health-check + diagnostic that exits non-zero on a problem and prints
actionable guidance:

- daemon reachable; socket mode correct; DB mode 0600 / dir 0700.
- embedding provider reachable (or `deterministic` fallback noted loudly).
- embedding-model identity matches DB meta (the W0.2 fail-closed contract).
- WAL checkpoint health; oversized-WAL warning with remediation hint.
- hooks installed + wiring drift (delegates to installer `status` where
  available). DEFERRED: `rusty-brain-install` exposes no `status` surface
  yet; doctor gains this check when it does.

### DOC-4. Protocol addition (additive, no CONTRACT_VERSION bump)

Add `Request::Stats { namespace }` / `Response::Stats { ... }` following the
`Feedback` precedent: additive serde-default variant, old daemon fails to
decode and closes (handshake version gates shared result types, not ops).
Aggregation happens in the read pool; zero writer ops.

## Acceptance Criteria

- `status` reports writer health, WAL size, corpus stats, embedding model.
- `stats` reports feedback ratios and top/never-recalled counts with zero
  writer ops (asserted by a test mirroring W1.8).
- `doctor` exits non-zero and prints guidance when the DB mode is wrong or
  the embedding model mismatches meta.
- JSON and human output both supported (`--json` global flag).

## Verification

```bash
cargo test -p rusty-brain
cargo test -p rb-daemon
cargo test -p rb-store
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus a daemon e2e asserting `stats` triggers zero FTS writes and reflects a
seeded feedback distribution.

## Risks

- Aggregation cost on large corpora. Mitigate: bounded windows, indexed
  reads, and `LIMIT` on top/never-recaled.
- Misleading "helpful %" on sparse feedback. Mitigate: show raw counts and a
  low-N caveat, not a confident ratio.
- Exposing sensitive content in stats output. Mitigate: stats emit counts
  and ids only, never memory content.

## Implementation Checklist

- [x] Extend `status` payload + output.
- [x] Add `Stats` request/response (additive, no bump).
- [x] Implement read-only aggregations in the store/read pool
      (`rb-store/src/store/stats.rs`, inherent-impl module — the store split
      landed in PR #54, so there is no monolithic `store.rs` anymore).
- [x] Add `doctor` with exit codes + guidance.
- [x] Assert zero-writer-ops on the stats path.
- [x] Add JSON + human output and tests.

## Roadmap Fit

Directly satisfies the W1.6/W4.3 "status reports writer health, WAL size,
corpus stats, subscriber state" gate clause and adds the user-facing value
signal the roadmap lacks.
