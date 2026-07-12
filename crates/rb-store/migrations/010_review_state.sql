-- Review-sweep snooze/reviewed-at persistence (PRD 2026-07-02
-- contradiction/dedup review, REV-2 `snooze`). Additive side table, bounded:
-- at most one row per reviewed item, keyed by the canonical item key
-- (reason-prefixed, lexicographically-sorted member ids — see
-- rb_types::review_item_key), so the same pair discovered from either scan
-- anchor maps to one row. Deliberately NO FK to memories: an item key spans
-- up to two memories, and a row whose memories were purged simply stops
-- matching the queue's exclusion probe (harmless, bounded residue).
CREATE TABLE review_state (
  item_key     TEXT PRIMARY KEY,
  namespace    TEXT NOT NULL,
  reviewed_at  INTEGER NOT NULL,  -- unix seconds of the last resolution
  snooze_until INTEGER            -- unix seconds; NULL = reviewed, not hidden
);

-- The queue's snooze-exclusion probe and the snoozed gauge scan by namespace.
CREATE INDEX idx_review_state_ns ON review_state(namespace, snooze_until);
