# PRD: Decision History and Audit Timeline (`rusty-brain history`)

## Status

Delivered 2026-07-11 (branch `claude/decision-history-timeline`, Vikunja
#456). From the 2026-07-02 senior-PM product review. The supersede chain and
`contradicts` links already exist but are not surfaced as a *story*. Surfacing
the evolution of a decision turns memory from a flat store into an
institutional decision journal - the strongest single differentiator vs
flat-recall competitors.

Delivery notes:

- The chain walk is `memories.superseded_by` based (the atomic supersede's
  artifact) — no `supersedes` link rows exist in storage, and the supersede
  op records no per-hop *reason* anywhere (the oplog `details` carry only the
  replacement id). HIST-1's "supersede `reason`" therefore has nothing to
  derive from in v1 storage; chain hops carry no reason field. Link edges DO
  carry their stored `reason`.
- Server bounds: depth clamps to 100 hops per direction (an absent `--depth`
  uses the cap, per HIST-3's "default unbounded but capped"); the edge list
  caps at 200. Both report `truncated`.
- "Active" edges follow the `contested` semantics exactly: both endpoints
  non-archived AND in the request namespace (the `active_contradicts`
  double-endpoint scoping), so edges on archived chain members do not render.
- The MCP `history` tool ships full-toolset-gated (HIST-3): the default
  `tools/list` token budget is unchanged.

## Owner Area

Primary: read-only graph traversal and presentation.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (`History` subcommand)
- `crates/rusty-brain/src/run.rs`
- `crates/rusty-brain/src/output.rs`
- `crates/rb-store/src/store.rs` (existing supersede + link queries)
- `crates/rb-engine/src/engine.rs`
- `crates/rb-proto/src/messages.rs` (additive `History` request/response)
- `crates/rb-mcp/src/tools.rs` (optional `history` tool, full-toolset-gated)
- `docs/prds/2026-07-02-contradiction-dedup-review.md` (resolve -> history)

## Problem

A memory can be superseded (update-as-supersede from W3.1) and contested
(W2.2 `contradicts`), but the user sees only the current memory and a flat
`graph`. There is no view of how a decision evolved over time, who/what
contradicted it, and why - the exact information that makes a memory store
trustworthy for decision tracking.

## Goals

- A timeline/audit view of a memory's evolution: prior versions (supersede
  chain), contradictions, extensions, and the metadata for each (when, who,
  confidence, reason).
- Read-only, zero-writer-ops (W1.8 invariant), JSON + human output.
- Reuse existing link/supersede queries; no schema change.

## Non-Goals

- Do not mutate memories (resolution is PRD 6's job).
- Do not build a full audit-log UI; a CLI/MCP view is the v1 scope.
- Do not change the supersede/link semantics.
- Do not persist a separate history table; derive from existing links.

## Functional Requirements

### HIST-1. `rusty-brain history <id>`

Prints the evolution of the memory identified by `id`:

- The supersede chain in both directions: what this memory supersedes
  (ancestors) and what supersedes it (newer), ordered by time, each with
  summary, importance, confidence, `created_at`, `origin_*`, and the
  supersede `reason`.
- Active `contradicts`/`extends`/`references` edges with the linked memory's
  summary, confidence, and `contested` state.
- A clear "current truth" pointer (the newest non-archived member of the
  chain) and flags for archived/contested members.

### HIST-2. Derivation

Built entirely from existing queries: the supersede link type, the
`memories.archived_at`/`confidence`/`origin_*` columns, and the existing
link rows. No new persistence.

### HIST-3. Output and limits

- `--depth N` bounds the chain walk (default unbounded but capped by an
  internal safety limit).
- JSON shape for scripting; a compact indented human view with age markers
  and `contested`/`superseded` markers consistent with W3.3 projection.
- Optional MCP `history` tool behind `RB_MCP_FULL_TOOLSET` (not in the
  default advertised toolset, to respect the token budget).

## Acceptance Criteria

- For a chain A (superseded by) B (superseded by) C, `history C` lists
  A->B->C with C flagged current and A/B flagged superseded.
- A `contradicts` edge appears with both memories' summaries and the
  `contested` marker.
- `history` issues zero writer ops (asserted, mirroring W1.8).
- Human and JSON output both supported.

## Verification

```bash
cargo test -p rusty-brain
cargo test -p rb-engine
cargo test -p rb-store
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus an e2e building a 3-deep supersede chain + a contradiction and
asserting the rendered timeline.

## Risks

- Deep or cyclic chains produce noisy output. Mitigate: depth cap, cycle
  detection (reusing the existing graph CTE), and de-dup.
- Exposing provenance/author names in team contexts. Mitigate: respect the
  existing provenance surface; redaction policy applies uniformly.

## Implementation Checklist

- [x] Add `History` subcommand + additive `History` request/response.
- [x] Implement derivation over existing supersede/link queries.
- [x] Add cycle handling + depth cap.
- [x] Human + JSON output with age/contested/superseded markers.
- [x] Optional MCP `history` tool behind `RB_MCP_FULL_TOOLSET`.
- [x] Zero-writer-ops + chain e2e tests.

## Roadmap Fit

Surfaces the W2.2 trust machinery (supersede/contested) to the user without
new storage, directly raising the `teamfit` dimension's decision-journal
value ahead of Phase 5.
