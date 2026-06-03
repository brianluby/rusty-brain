-- 003_embedding_input_version.sql
-- P5 Feature A: composite embedding input + re-embed transition.
--
-- Stamp each memory with the composition version that produced its vector,
-- alongside the existing embedding_model stamp. The `reembed` batch re-embeds
-- any active row whose (embedding_model, embedding_input_version) differs from
-- the current pair, then stamps it to the current version. This lets the corpus
-- transition from content-only vectors (written before P5) to composite vectors
-- (content + keywords + tags + context) idempotently.
--
-- Additive and file-discovered (checksummed by the migration runner). Existing
-- rows are backfilled with 'v1-content-only' to record that their vectors were
-- built from raw content alone; reembed will recompute them on demand.

-- ADD COLUMN with NOT NULL DEFAULT backfills every existing row with
-- 'v1-content-only' in the same statement, so no explicit UPDATE is needed.
ALTER TABLE memories
  ADD COLUMN embedding_input_version TEXT NOT NULL DEFAULT 'v1-content-only';

-- Seed the global meta invariant alongside embedding_model / embedding_dim, so
-- the store records which composition the daemon currently targets. Code reads
-- the live version from rb-engine (EMBEDDING_INPUT_VERSION); this row documents
-- the target at migration time and is the anchor reembed converges the corpus
-- toward.
INSERT OR IGNORE INTO meta (key, value)
  VALUES ('embedding_input_version', 'v2-composite');
