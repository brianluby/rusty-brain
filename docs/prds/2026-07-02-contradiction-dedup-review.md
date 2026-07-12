# PRD: Guided Contradiction and Dedup Resolution (`rusty-brain review`)

## Status

Delivered 2026-07-11 (branch `claude/review-sweep`, Vikunja #461). From the
2026-07-02 senior-PM product review. `contested` flags and write-time
near-dup suppression exist, but resolution is left to manual commands. This
surfaces the W2.2 trust machinery to the user instead of leaving it latent.

Delivery notes: near-dup detection reuses `near_duplicates()` (the one
similarity definition; conservative 0.95 default, floor 0.80); contradiction
pairs are the pairwise expression of `active_contradicts`, pinned by a drift
test; snooze persists in the additive `review_state` table (migration 010);
`--apply` follows the forget UX (dry-run default, TTY default-NO
confirmation, `--yes` for automation, `ForgetOutcome`-shaped partial
reporting); no MCP surface (the forget precedent: destructive surface,
tools/list budget at 897/900); stats gains the `low_confidence_live` gauge
(with `contested`/`never_recalled_live`, the review-queue trend — the
near-dup tier is deliberately uncounted: one KNN probe per live memory per
stats call).

## Owner Area

Primary: an interactive (or `--apply`) sweep over existing stores/links.

Touchpoints:

- `crates/rusty-brain/src/cli.rs` (`Review` subcommand)
- `crates/rusty-brain/src/run.rs`
- `crates/rb-store/src/store.rs` (`near_duplicates`, `supersede`, links)
- `crates/rb-engine/src/engine.rs`
- `crates/rb-proto/src/messages.rs`
- `docs/prds/2026-07-02-decision-history-timeline.md` (resolve -> history)
- `docs/prds/2026-07-02-doctor-and-stats-observability.md` (review queue size)

## Problem

Contradictions and duplicates accumulate silently. The user has no guided way
to find them, decide a winner, merge, supersede, or dismiss - they must run
individual `link`/`update`/`delete` commands against ids they have to
discover themselves. The result is a corpus that drifts, and trust erodes
because the user cannot see or clean the noise.

## Goals

- A single sweep that surfaces contradictions, near-duplicates, and
  low-confidence memories needing attention.
- Per-item actions: keep, merge (supersede), archive, lower confidence,
  dismiss (snooze).
- Both interactive (TTY) and `--apply`/`--dry-run` (scriptable) modes.
- Reuse existing atomic supersede + link + confidence primitives; no new
  schema.

## Non-Goals

- Do not auto-resolve without consent (`--apply` requires an explicit policy
  filter; interactive always prompts).
- Do not invent new link types.
- Do not replace `link`/`update`/`delete`; this orchestrates them.
- Do not change ranking.

## Functional Requirements

### REV-1. `rusty-brain review`

A queue generator over the current namespace, ordered by priority:

1. Active `contradicts` pairs (`contested` memories).
2. Near-duplicate clusters (reuse `near_duplicates()`) above a similarity
   threshold.
3. Low-confidence (`< 0.4`) live memories and never-recalled stale candidates
   (from the stats surface).

Each item renders both sides (summary, importance, confidence, age,
provenance) for a human decision.

### REV-2. Actions

Per item, the user chooses (interactive) or a policy chooses (`--apply`):

- `keep` - no-op, optionally bump confidence.
- `merge` - store a combined memory and supersede both originals (the W3.1
  update-as-supersede path).
- `archive` - soft-delete the loser.
- `demote` - lower confidence (the W2.2 knob).
- `snooze` - record a `reviewed_at`/`snooze_until` marker so it does not
  reappear immediately.

### REV-3. Determinism and safety

- `--dry-run` prints the plan with no writes.
- `--policy <name>` applies a documented policy non-interactively
  (e.g. `auto-merge-dups`, `demote-low-confidence`).
- Every mutation reuses the existing atomic supersede/link/confidence paths
  (single writer, transactional).
- A `--since <seq>` option limits the review to recent changes.

### REV-4. Provenance and audit

Each resolution records the standard provenance (`origin_source = "cli"`,
author) and an oplog entry, so review actions appear in history (PRD 5) and
stats (review queue trend).

## Acceptance Criteria

- `review` surfaces a seeded contradiction and a near-dup pair in priority
  order.
- `--dry-run` makes zero writes; `--apply --policy auto-merge-dups` merges a
  dup pair into one superseding memory (originals archived).
- Interactive mode prompts per item and applies the chosen action atomically.
- Snoozed items do not reappear until their window elapses.
- All resolution actions are transactional and oplog-recorded.

## Verification

```bash
cargo test -p rusty-brain
cargo test -p rb-engine
cargo test -p rb-store
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Plus an e2e seeding a contradiction + dup cluster and asserting the
`--apply` resolution and the resulting history.

## Risks

- Destructive auto-merge on false-positive dups. Mitigate: `--dry-run`
  default, conservative threshold, and supersede (reversible) over hard
  delete.
- Interactive mode unusable in CI/scripts. Mitigate: first-class `--apply
  --policy` parity.
- Snooze state needs persistence. Mitigate: a small `review_state` side
  table or metadata, additive and bounded.

## Implementation Checklist

- [x] Add `Review` subcommand with queue generator.
- [x] Implement interactive + `--apply --policy` + `--dry-run` modes.
- [x] Wire actions to existing supersede/link/confidence primitives.
- [x] Add snooze/reviewed_at tracking (additive, minimal).
- [x] Record provenance + oplog for every resolution.
- [x] E2E for contradiction + dup resolution.

## Roadmap Fit

Makes the W2.2 trust machinery usable, improving `teamfit` and corpus
hygiene ahead of Phase 5, and feeds the stats surface (review-queue size)
and the history timeline.
