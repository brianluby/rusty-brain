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
| Timestamp precision | Every timestamp is stored as **whole Unix seconds** (`DateTime::timestamp()`). The oplog `seq` is the only finer, total order of mutations, and purge removes it for purged rows. | `store/core.rs` `ts`; `memory_oplog.seq` |

Two derived facts matter:

- Because `archived_at` is write-once, **membership as recorded** at a
  whole-second instant T is well defined for every row that was never purged:
  `created_at ≤ T AND (archived_at IS NULL OR archived_at > T)`. It is exact
  only to the second. A row created and archived within the same second has
  `created_at = archived_at` and is never a member. Its replacement, created
  in that same second, is a member from that second on. Sub-second order
  exists only in the oplog `seq`.
- Metadata at time T cannot be reconstructed. Only the current summary,
  importance, confidence and tags exist; the oplog keeps the new values of
  importance and confidence changes, not of `update` edits.

## What existing commands answer

| Question | Answered today? | How, or why not |
|---|:---:|---|
| What is the current decision on X? | yes | `recall`, and the `current` flag in `history` |
| How did decision X evolve, in order? | yes | `history <id>`: the supersede chain with `created_at`, plus contradicts/extends edges |
| When did each version stop being current? | **no** (data exists) | `HistoryEntry` exposes only `archived: bool`. `archived_at` is visible per id only through `get --json`. |
| What was in the store as active at instant T? | **no** (data exists) | The `until` filter tests `created_at` alone. Through the wire filter with `MemoryState::All`, it also returns rows already archived before T. With the default active state (the CLI default, or `--archived` for archived-only), it drops rows that were active at T but archived since. Neither is the membership at T. The CLI and MCP do not expose an all-states mode. |
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
- A wire-level `list` with `until: Aug 15` and `state: all` (the CLI has no
  all-states flag) returns A **and** B, which is wrong: A had already been
  superseded. `rusty-brain list --until 'Aug 15'` (active only) returns
  nothing, because A and B are both archived now. Also wrong.
  `--archived --until 'Aug 15'` returns A and B too.
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
2. **`RecallFilter.as_of: Option<DateTime>`**, for `list` only, not ranked
   `recall` and not `history`. `RecallFilter` rides only on `Request::List`
   and `Request::Recall`, and `Request::History` takes just an id and depth.
   An as-of history would need its own `History` field, and that is not part
   of step 1.
   - Membership is `created_at ≤ T AND (archived_at IS NULL OR archived_at > T)`.
   - Archived rows have no vectors, so a ranked as-of recall would silently
     lose its vector channel. A filtered list is honest.
   - `as_of` **replaces** the state predicate. `RecallFilter.state` is
     non-optional with an `Active` default, so an omitted state and an
     explicit `active` cannot be told apart on the wire. The rule is therefore:
     when `as_of` is set, `state` must be `Active` and is ignored, and
     `Archived` or `All` is a validation error. The CLI rejects
     `--as-of` combined with `--archived`.
   - **Precision:** `as_of` is whole seconds. A sub-second instant is a
     validation error rather than a silent truncation. Responses carry
     `"precision": "second"`, and the same-second collapse above is documented.
     If audits ever need finer order, the oplog `seq` is the existing source.
     That is not part of step 1.
   - Every response carries `"temporal_basis": "recorded"` and a note that
     metadata values are current, not as of T.
   - **Completeness.** `list` is clamped to 1,000 rows by the daemon
     (`MAX_LIMIT`; the CLI defaults to 20), and `Response::Listed` has no
     cursor or truncation signal. An audit must never look complete when it
     is not. An `as_of` response therefore orders by `(created_at,
     memory_id)`, carries `truncated: bool` (computed by fetching one row past
     the effective limit), and, when truncated, a `next_after` keyset cursor
     (the last row's `created_at` and `memory_id`) that a follow-up `as_of`
     request accepts. The CLI prints a warning and the cursor whenever
     `truncated` is true.
3. **Capability gate (fail closed).** `RecallFilter` fields are additive: an
   older daemon decodes the frame, drops the unknown `as_of`, and answers an
   ordinary active-only list, which would be a silently wrong audit result.
   Follow the typed-anchors precedent: the daemon advertises a new `as_of`
   capability in its handshake ack, and the client refuses to send `as_of`
   to a daemon without it (`InvalidArgument` naming the missing capability,
   like `CAP_ANCHORS`). As a second guard, the client errors if an `as_of`
   response lacks `temporal_basis`. `HistoryEntry.archived_at` needs no gate:
   from an older daemon it decodes as `None`, and the timeline omits the end
   time rather than claiming one.
4. **Limitations stated in the output and the docs:** purged rows never
   appear; metadata is as of now; this is not validity time.

### Test cases

| # | Setup | Expectation |
|---|---|---|
| 1 | Chain A→B→C with controlled `created_at`/`archived_at` | `as_of` before A: none; between A and B: {A}; between B and C: {B}; after C: {C} |
| 2 | Boundary instants | At exactly `archived_at` the row is excluded; at exactly `created_at` it is included |
| 2b | Same-second create and archive (`created_at = archived_at`), with its replacement created in that second | The archived row is never a member; the replacement is a member from that second on; a sub-second `as_of` is rejected |
| 3 | Row archived without a successor (retention or delete) | Present before `archived_at`, absent after; `history` shows `archived_at` |
| 4 | Fan-in: two near-duplicates superseded into one | Both present before the supersede, only the survivor after |
| 5 | Purged row | Never present at any `as_of`; the docs say so |
| 6 | Late-recorded event (P recorded Oct 10, about Sep 1) | `as_of = Sep 15` returns M, not P, with `temporal_basis: "recorded"`. This documents the limit rather than hiding it. |
| 7 | `as_of` with `state: archived` or `all`, or CLI `--as-of --archived` | Validation error. `as_of` with the default `active` state applies the as-of predicate, not the active one, so a row archived after T is still returned. |
| 8 | Active recall regression | The committed semantic gate and recall goldens are unchanged, with no `as_of` set |
| 9 | Namespace isolation | `as_of` never returns rows from another namespace |
| 10 | Metadata | The returned importance and confidence are current values, and the response says so |
| 11 | Client against a daemon without the `as_of` capability | The client fails before sending, naming the capability; no active-only result is returned |
| 12 | A response to an `as_of` request without `temporal_basis` | The client errors instead of returning the rows |
| 13 | More matching rows than the effective limit (a limit of 20, and also more than 1,000) | `truncated: true` with a `next_after` cursor; following the cursor returns every remaining row exactly once, in order, with no duplicates or gaps at equal `created_at` |

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
