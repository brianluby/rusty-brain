-- 007_base_importance.sql (W1.9, F33)
--
-- Importance recalibration must respect author intent: the background job may
-- modulate a memory's EFFECTIVE `importance` only within a bounded delta of the
-- value its author set. That requires the author-set value to survive the first
-- recalibration write, so it gets its own column.
--
-- `base_importance` is the immutable author-set prior:
--   * stamped to the authored `importance` at insert,
--   * re-stamped ONLY when a caller explicitly updates `importance` through the
--     user-facing update path (the author re-declaring intent),
--   * NEVER written by the recalibration job (which writes only `importance`
--     via `set_recalibrated_importance`).
--
-- Nullable by necessity: ALTER TABLE cannot add a NOT NULL column whose
-- backfill depends on another column. The UPDATE below backfills every existing
-- row from its current `importance` (the best available record of author
-- intent), and `insert_memory` always populates it for new rows; readers
-- COALESCE to `importance` as defense in depth.
--
-- FTS churn: none. Under migration 006 `mem_au` fires only on updates that
-- assign content/summary/keywords/tags; this UPDATE assigns neither.
ALTER TABLE memories ADD COLUMN base_importance INTEGER;

UPDATE memories SET base_importance = importance;
