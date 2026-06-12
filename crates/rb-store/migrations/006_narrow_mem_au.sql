-- 006_narrow_mem_au.sql (W1.8, first half of F08)
--
-- Narrow the FTS sync trigger so only updates that touch an INDEXED column
-- (content, summary, keywords, tags) rewrite the FTS index. The original
-- `AFTER UPDATE` trigger fired on EVERY memories update, so each access bump
-- (`access_count`/`last_accessed_at`), archive, importance recalibration,
-- supersede pointer, and embedding stamp deleted and re-inserted the row's
-- inverted-index entries — pure churn that grew the fts5 segment data without
-- ever changing what a query could match (read-path write amplification).
--
-- `AFTER UPDATE OF <cols>` fires when any listed column appears in the SET
-- list of an UPDATE, which is exactly the external-content sync contract:
-- those four columns are the only ones memories_fts indexes, so an UPDATE
-- that never assigns them cannot change the index's correct contents.
-- `update_memory` builds its SET clauses dynamically from the changed fields
-- only, so a metadata-only update no longer touches FTS at all.
--
-- The delete+insert body is unchanged from 001 (and survived 005's index
-- rebuild: triggers live on `memories`, not on the virtual table).
DROP TRIGGER mem_au;

CREATE TRIGGER mem_au AFTER UPDATE OF content, summary, keywords, tags ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, content, summary, keywords, tags)
  VALUES ('delete', old.rowid, old.content, old.summary, old.keywords, old.tags);
  INSERT INTO memories_fts(rowid, content, summary, keywords, tags)
  VALUES (new.rowid, new.content, new.summary, new.keywords, new.tags);
END;
