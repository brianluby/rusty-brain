-- 004_provenance.sql
-- W0.5: provenance columns + durable oplog (the unbackfillable migration).
--
-- Provenance: who/where/what wrote each memory. All five columns are nullable
-- with NO default and NO backfill UPDATE — an UPDATE here would fire the
-- `mem_au` trigger and rewrite the FTS index for every existing row. Old rows
-- keep NULL provenance (decoded as `None`); only writes from now on carry it.
-- ALTER ... ADD COLUMN is metadata-only in SQLite (no row rewrite, no trigger),
-- and the FTS triggers reference fixed column lists, so this is trigger-safe.
ALTER TABLE memories ADD COLUMN origin_user   TEXT;
ALTER TABLE memories ADD COLUMN origin_host   TEXT;
ALTER TABLE memories ADD COLUMN origin_agent  TEXT;
ALTER TABLE memories ADD COLUMN origin_source TEXT;   -- hook|mcp|cli|job
ALTER TABLE memories ADD COLUMN session_id    TEXT;

-- Durable operation log: one row per mutation, appended IN THE SAME
-- TRANSACTION as the mutation itself, from day one. `seq` is the site-local
-- monotonic order; `site_id` (meta key, uuid v4 seeded in code at init)
-- disambiguates rows once logs from multiple machines merge (team mode).
-- `details` is a small JSON payload (e.g. link type) or empty.
CREATE TABLE memory_oplog (
  seq       INTEGER PRIMARY KEY AUTOINCREMENT,
  site_id   TEXT NOT NULL,
  op        TEXT NOT NULL,            -- insert|update|archive|supersede|link
  memory_id TEXT NOT NULL,
  namespace TEXT NOT NULL,
  at        INTEGER NOT NULL,         -- unix seconds
  details   TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_oplog_ns_seq ON memory_oplog(namespace, seq);
