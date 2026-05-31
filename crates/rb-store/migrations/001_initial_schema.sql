-- 001_initial_schema.sql
-- Base schema for rusty-brain. One database file; meta is the single source
-- of truth for invariants. memory_vectors is intentionally absent here: its
-- dimension is dynamic and the vec0 virtual table is created in code at open().
-- _migrations is created by the migration runner before this file is applied.

-- meta: single source of truth for invariants
-- (seeded at init: schema_version, embedding_model, embedding_dim)
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE memories (
  memory_id        TEXT PRIMARY KEY,
  namespace        TEXT NOT NULL,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  content          TEXT NOT NULL,
  summary          TEXT NOT NULL,
  keywords         TEXT NOT NULL,   -- JSON array
  tags             TEXT NOT NULL,   -- JSON array
  context          TEXT NOT NULL DEFAULT '',
  memory_type      TEXT NOT NULL CHECK (memory_type IN (
                     'architecture_decision','code_pattern','bug_fix','configuration',
                     'constraint','entity','insight','reference','preference')),
  importance       INTEGER NOT NULL CHECK (importance BETWEEN 1 AND 10),
  confidence       REAL NOT NULL CHECK (confidence BETWEEN 0.0 AND 1.0),
  related_files    TEXT NOT NULL DEFAULT '[]',
  access_count     INTEGER NOT NULL DEFAULT 0,
  last_accessed_at INTEGER,
  archived_at      INTEGER,          -- NULL = active (soft delete, in BASE schema)
  superseded_by    TEXT REFERENCES memories(memory_id),
  embedding_model  TEXT NOT NULL
);

CREATE INDEX idx_mem_ns         ON memories(namespace);
CREATE INDEX idx_mem_created    ON memories(created_at);
CREATE INDEX idx_mem_importance ON memories(importance);
CREATE INDEX idx_mem_active     ON memories(archived_at) WHERE archived_at IS NULL;

CREATE TABLE memory_links (
  source_id  TEXT NOT NULL REFERENCES memories(memory_id),
  target_id  TEXT NOT NULL REFERENCES memories(memory_id),
  link_type  TEXT NOT NULL CHECK (link_type IN
               ('extends','contradicts','implements','references','supersedes')),
  strength   REAL NOT NULL CHECK (strength BETWEEN 0.0 AND 1.0),
  reason     TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  PRIMARY KEY (source_id, target_id, link_type)
);

-- FTS5 external-content index over the searchable text columns of memories.
CREATE VIRTUAL TABLE memories_fts USING fts5(
  content,
  summary,
  keywords,
  tags,
  content='memories',
  content_rowid='rowid'
);

-- Keep the FTS index in sync with the memories table.
CREATE TRIGGER mem_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts(rowid, content, summary, keywords, tags)
  VALUES (new.rowid, new.content, new.summary, new.keywords, new.tags);
END;

CREATE TRIGGER mem_ad AFTER DELETE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, content, summary, keywords, tags)
  VALUES ('delete', old.rowid, old.content, old.summary, old.keywords, old.tags);
END;

CREATE TRIGGER mem_au AFTER UPDATE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, content, summary, keywords, tags)
  VALUES ('delete', old.rowid, old.content, old.summary, old.keywords, old.tags);
  INSERT INTO memories_fts(rowid, content, summary, keywords, tags)
  VALUES (new.rowid, new.content, new.summary, new.keywords, new.tags);
END;
