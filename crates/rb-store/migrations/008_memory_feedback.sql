-- 008_memory_feedback.sql (W3.7, F37)
--
-- The usefulness signal. `access_count` (migration 001) counts how often a
-- memory was RETURNED by recall, not whether it was actually USEFUL. This table
-- records the distinct, explicit signal a caller reports via the
-- `memory_feedback` MCP tool / `rusty-brain feedback` CLI: helpful | wrong |
-- stale. It is the non-circular input the W5c per-author trust weighting needs
-- (aggregate by the target memory's existing provenance columns) and the
-- positive signal the W1.9 importance recalibration can consume.
--
-- One row per feedback EVENT (an event log, not a counter) so a consumer can
-- reconstruct counts via GROUP BY, weight by recency (`at`), or attribute by
-- `principal` (the giver) without losing information. Recorded in the SAME
-- transaction as the confidence nudge + the `memory_oplog` row the daemon
-- appends (op = 'feedback'), so the durable change log and this table never
-- disagree. Purely additive (new table only — no ALTER on `memories`), so the
-- FTS triggers are untouched and the migration is trigger-safe.
--
-- No foreign key to `memories(memory_id)` — even though `foreign_keys=ON` (set
-- in `SqliteStore::init`): `memory_feedback` is an append-only EVENT LOG, so it
-- deliberately matches `memory_oplog` (the sibling op log, also FK-free) rather
-- than the relational tables. Coupling an append-only log to row lifecycle would
-- fight a future hard purge (W5b); memories are otherwise soft-deleted (archived,
-- never removed), and the engine + the in-transaction existence check in
-- `record_feedback` guard against orphan rows. The `kind` CHECK mirrors the
-- enum-column convention (memories.memory_type, memory_links.link_type) and MUST
-- stay in lockstep with `rb_types::FeedbackKind::as_str`.
CREATE TABLE memory_feedback (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  memory_id TEXT NOT NULL,
  namespace TEXT NOT NULL,
  kind      TEXT NOT NULL CHECK (kind IN ('helpful', 'wrong', 'stale')),  -- FeedbackKind::as_str
  principal TEXT,                     -- the giver (origin_user), or NULL when unknown
  at        INTEGER NOT NULL          -- unix seconds
);

-- Per-memory aggregation (W1.9 helpful tally; W5c per-author rollup joins via
-- memory_id -> memories provenance).
CREATE INDEX idx_memory_feedback_memory ON memory_feedback(memory_id);
-- Namespace-scoped, recency-ordered scans for team-mode trust rollups (W5c).
CREATE INDEX idx_memory_feedback_ns_at ON memory_feedback(namespace, at);
