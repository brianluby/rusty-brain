-- 009_memory_anchors.sql (PRD 2026-07-02 typed-code-anchors, ANC-1)
--
-- First-class, queryable code anchors: a structured link from a memory to a
-- file path (+ optional 1-based line range), a commit SHA, or a symbol name.
-- Multiple anchors per memory. Purely additive (new table only — no ALTER on
-- `memories`, so the FTS triggers are untouched and no backfill UPDATE runs
-- on existing rows); anchors decode alongside the row in `row_to_note` like
-- `memory_links`, and pre-anchor rows simply load an empty list.
--
-- Value placement mirrors the anchor kind (enforced by the CHECK below):
--   kind='file'            -> `path` holds the normalized file path, `ref` NULL
--   kind='commit'|'symbol' -> `ref` holds the SHA / symbol name, `path` NULL
-- `start_line`/`end_line` are 1-based, inclusive, and only meaningful for
-- file anchors (`rb_types::MemoryAnchor::validate` is the authoritative
-- boundary check; the CHECKs here are the storage backstop).
--
-- `namespace` mirrors memories.namespace (denormalized like memory_vectors'
-- partition key) so anchor lookups never need a join for scoping and the PRD's
-- path-drift mitigation (repo-relative path + namespace) holds. Any path that
-- mutates a memory's namespace MUST re-key its anchors in the same
-- transaction — `rename_namespace` (the only such path today) does.
--
-- FK to memories(memory_id): anchors are relational row attributes (like
-- memory_links), not an append-only event log (unlike memory_feedback), and
-- memories are only ever soft-deleted (archived), so the FK cannot block a
-- delete path today.
CREATE TABLE memory_anchors (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  memory_id  TEXT NOT NULL REFERENCES memories(memory_id),
  namespace  TEXT NOT NULL,
  kind       TEXT NOT NULL CHECK (kind IN ('file', 'commit', 'symbol')),  -- rb_types::anchor_kind_str
  path       TEXT,     -- normalized file path (kind='file' only)
  start_line INTEGER,  -- 1-based, file anchors only
  end_line   INTEGER,  -- 1-based inclusive, file anchors only
  ref        TEXT,     -- commit SHA / symbol name (kind='commit'|'symbol' only)
  CHECK (
    (kind = 'file' AND path IS NOT NULL AND ref IS NULL)
    OR (kind IN ('commit', 'symbol') AND ref IS NOT NULL AND path IS NULL
        AND start_line IS NULL AND end_line IS NULL)
  ),
  CHECK (start_line IS NULL OR start_line >= 1),
  CHECK (end_line IS NULL OR (start_line IS NOT NULL AND end_line >= start_line))
);

-- Per-memory anchor load (row_to_note, alongside load_links).
CREATE INDEX idx_memory_anchors_memory ON memory_anchors(memory_id);
-- The anchor-filter semi-join probes (list_filtered builds
-- `m.memory_id IN (SELECT memory_id FROM memory_anchors WHERE namespace = ?
-- AND kind = ? AND path|ref = ?)`), so a selective anchor lookup costs
-- O(matching anchors), not O(active memories) — EXPLAIN-pinned by the
-- `anchor_filters_probe_the_wide_anchor_indexes` test. Their leading
-- `namespace` column also serves rename_namespace's re-key UPDATE.
CREATE INDEX idx_memory_anchors_path ON memory_anchors(namespace, kind, path);
CREATE INDEX idx_memory_anchors_ref ON memory_anchors(namespace, kind, ref);
