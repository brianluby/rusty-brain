# Point-in-time recall — gap assessment (Vikunja #88)

- **Date:** 2026-10-18, at `main` `1694a38`. A bounded, read-only
  investigation: no code, schema or recall change.
- **Question:** can existing history and supersession data answer what was
  known, or valid, at an earlier time?
- **Source of patterns:** Agent Memory Techniques, 18_temporal_memory. Its
  patterns are treated as hypotheses, not measured benefits for rusty-brain.

## Recommendation: implement a small recording-time `as_of`, defer validity time

1. **Implement (small, read-only):** an *as-recorded* point-in-time view built
   only from columns that already exist, plus `archived_at` in `history`
   output. It answers "what did the store hold as active at instant T?". It
   cannot answer "what was true at T?", and it must say so in its output.
2. **Defer:** validity or event time (`valid_from`/`valid_to`). It needs new
   metadata, and so a schema migration, which is out of scope here. Without a
   real query that fails today, it is speculative. Revisit with evidence.

Active recall stays unchanged in both cases.

## What the store records today

All timestamps are **recording (transaction) time**, the moment rusty-brain
wrote something. No field records when a fact became or stopped being true.

| Data | Semantics | Code |
|---|---|---|
| `memories.created_at` | When the row was written. It cannot be backdated. | `001_initial_schema.sql` |
| `memories.archived_at` | When the row left the active set (archive, supersede or retention). **Write-once**: every writer guards `archived_at IS NULL`, and no path clears it. | `store/internal.rs` `archive_with_details`; `store/core.rs` supersede |
| `memories.superseded_by` | Pointer to the replacement, stamped in the same statement as `archived_at` | `store/core.rs` supersede |
| `memories.updated_at` | Last write of any kind, including archive. Not a history. | — |
| Summary, tags, context, importance, confidence | **Mutable in place.** The previous values are not kept. | `MemoryUpdates`; `engine.update` |
| Content | Immutable: the engine rejects content updates, so a changed fact must arrive as a new memory that supersedes the old one. | `rb-engine` `engine.rs` `update` |
| `memory_oplog` | One row per mutation, with `at`. Details: `supersede` carries the new id, `set_importance`/`set_confidence` carry the new value, `update` carries nothing, and `archive` carries the retention cause or nothing. | `004_provenance.sql`; `store/internal.rs` `append_oplog` |
| Hard purge | Deletes the row, its vectors, feedback and **its oplog rows**, then writes one `purge` row. Purged memories are unrecoverable by design. | `store/retention.rs` |
| Archived rows' vectors | Deleted on archive or supersede, so archived rows are no longer in the KNN index. | `internal.rs`, supersede |

Two derived facts matter:

- Because `archived_at` is write-once, **membership as recorded** at any
  instant T is well defined for every row that was never purged:
  `created_at ≤ T AND (archived_at IS NULL OR archived_at > T)`.
- Metadata at time T cannot be reconstructed. Only the current summary,
  importance, confidence and tags exist; the oplog keeps the new values of
  importance and confidence changes, not of `update` edits.

## What existing commands answer

| Question | Answered today? | How, or why not |
|---|:---:|---|
| What is the current decision on X? | yes | `recall`, and the `current` flag in `history` |
| How did decision X evolve, in order? | yes | `history <id>`: the supersede chain with `created_at`, plus contradicts/extends edges |
| When did each version stop being current? | **no** (data exists) | `HistoryEntry` exposes only `archived: bool`. `archived_at` is visible per id only through `get --json`. |
| What was in the store as active at instant T? | **no** (data exists) | `recall`/`list --until T` filters on `created_at` alone. With `state=all` it also returns rows already archived before T; with the default `state=active` it drops rows that were active at T but archived since. Neither is the membership at T. |
| What did the store believe about X at T, including its metadata? | no | Importance, confidence, summary and tags are overwritten in place |
| What was *true* at T (validity or event time)? | no | No validity or event field exists anywhere; only the content text may say it |
| Was a fact recorded late? | no | Requires event time, which is not stored |

So `created_at ≤ cutoff` alone is not historical reconstruction. It is the
wrong predicate in both `state` modes.

## Worked examples

### 1. Decision reversal (answerable as recorded; no command does it)

The fusion decision changes twice, each change written when it was made:

| Memory | Content | `created_at` | `archived_at` | `superseded_by` |
|---|---|---|---|---|
| A | "Use RRF fusion" | Jul 1 | Aug 1 | B |
| B | "Use Linear fusion (RRF regressed MRR)" | Aug 1 | Sep 1 | C |
| C | "Back to RRF with tuned k" | Sep 1 | — | — |

- "What was the fusion decision on Aug 15?" The answer is B. The membership
  predicate gives exactly B at Aug 15, A at Jul 15, C today, and nothing
  before Jul 1.
- `history C` shows all three with `created_at` and marks C current. It does
  not show that A ended on Aug 1. Here that can be inferred from B's
  `created_at`, but not when a row was archived without a successor, or with
  fan-in near-duplicates.
- `list --state all --until 'Aug 15'` returns A **and** B, which is wrong:
  A had already been superseded. `list --until 'Aug 15'` (active only)
  returns nothing, because A and B are both archived now. Also wrong.
- **Recording versus validity:** if the switch to Linear was decided on Jul
  28 but recorded on Aug 1, the as-recorded view says A on Jul 30. That is
  correct about the store and wrong about the project. The output must label
  its basis.

### 2. Late-recorded event (not answerable)

On Oct 10 an agent records "The production database was migrated from MySQL
to Postgres on Sep 1" (memory P, `created_at` Oct 10). An older decision M,
"Production runs MySQL" (`created_at` Jun 1), is superseded by P on Oct 10.

- "What database did production run on Sep 15?" The truth is Postgres. The
  as-recorded view at Sep 15 returns M, which is **wrong**: the store did not
  yet know.
- Nothing in the metadata says P's event happened on Sep 1. Only the content
  text does. Answering correctly needs `valid_from = Sep 1` on P, with M's
  validity ending at the same instant even though it was archived on Oct 10.
  This is the **bitemporal** case, and it is exactly what the deferred
  validity-time item would add.

## Smallest proposed contract (step 1)

Read-only and additive, with no schema change, no migration and no
natural-language time parsing. Instants are RFC 3339, as `--since`/`--until`
already accept.

1. **`HistoryEntry.archived_at: Option<DateTime>`.** An additive serde field,
   following the `contested` precedent, so no contract-version bump. The
   human timeline renders "current until <time>".
2. **`RecallFilter.as_of: Option<DateTime>`**, for `list` (and `history`), but
   **not** ranked `recall`:
   - Membership is `created_at ≤ T AND (archived_at IS NULL OR archived_at > T)`.
   - Archived rows have no vectors, so a ranked as-of recall would silently
     lose its vector channel. A filtered list is honest.
   - `as_of` conflicts with `state`. Reject the combination as a validation
     error.
   - Every response carries `"temporal_basis": "recorded"` and a note that
     metadata values are current, not as of T.
3. **Limitations stated in the output and the docs:** purged rows never
   appear; metadata is as of now; this is not validity time.

### Test cases

| # | Setup | Expectation |
|---|---|---|
| 1 | Chain A→B→C with controlled `created_at`/`archived_at` | `as_of` before A: none; between A and B: {A}; between B and C: {B}; after C: {C} |
| 2 | Boundary instants | At exactly `archived_at` the row is excluded; at exactly `created_at` it is included |
| 3 | Row archived without a successor (retention or delete) | Present before `archived_at`, absent after; `history` shows `archived_at` |
| 4 | Fan-in: two near-duplicates superseded into one | Both present before the supersede, only the survivor after |
| 5 | Purged row | Never present at any `as_of`; the docs say so |
| 6 | Late-recorded event (P recorded Oct 10, about Sep 1) | `as_of = Sep 15` returns M, not P, with `temporal_basis: "recorded"`. This documents the limit rather than hiding it. |
| 7 | `as_of` combined with `state` | Validation error |
| 8 | Active recall regression | The committed semantic gate and recall goldens are unchanged, with no `as_of` set |
| 9 | Namespace isolation | `as_of` never returns rows from another namespace |
| 10 | Metadata | The returned importance and confidence are current values, and the response says so |

## Deferred: validity time (step 2)

The idea is an optional, explicitly asserted `valid_from` (and possibly
`valid_to`) with as-of-validity queries. Deferral reasons:

- It needs schema columns and a migration (out of scope here) plus a
  producer. Agents would have to extract event dates from text, which is the
  error-prone natural-language time parsing this ticket excludes.
- No measured failure exists yet. The worked example is constructed, and
  there is no observed user query where recording time gave a wrong answer
  that mattered.
- **Revisit trigger:** a real recall or history question answered wrongly
  because an event was recorded late, or a request for point-in-time audit
  that needs validity rather than recording time.

## Not changed

Active recall, ranking, the supersede and archive semantics, and the oplog are
untouched. This document is the only deliverable.
