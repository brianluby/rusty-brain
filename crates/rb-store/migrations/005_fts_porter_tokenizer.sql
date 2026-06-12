-- 005_fts_porter_tokenizer.sql (W1.2)
--
-- Recreate the FTS5 index with the porter stemmer layered over unicode61 and
-- rebuild it from the external-content `memories` table. Bare unicode61
-- requires exact word forms, so a query token like "retries" never matches a
-- memory that says "retry" — porter stems both sides at index and query time
-- and unifies the inflections.
--
-- Tokenizer decision (W1.2, recorded per the plan's ground rule). Measured on
-- the W1.0 eval corpus (72 golden queries, committed all-MiniLM-L6-v2 replay
-- vectors) under the OR-of-quoted-tokens query construction that ships with
-- this migration (see `escape_fts5_query` in store.rs for the construction
-- decision record):
--
--   unicode61:          replay recall@5 0.9491, MRR 0.9699, NDCG 0.9385
--                       deterministic   recall@5 0.7338, MRR 0.9074
--   porter unicode61:   replay recall@5 0.9630, MRR 0.9838, NDCG 0.9537
--                       deterministic   recall@5 0.7847, MRR 0.9611
--
-- porter wins every quality metric, so it ships. (Under the stricter AND
-- construction the two tokenizers measured identically — every conjunction
-- failed on question words before stemming could matter.) A trailing-token
-- prefix match (`"tok"*`) was also measured and REGRESSED under porter
-- (prefix queries bypass the stemmer: replay MRR 0.9838 -> 0.9769), so it is
-- deliberately not emitted.
--
-- Dropping an FTS5 virtual table drops its shadow tables but NOT the
-- mem_ai/mem_au/mem_ad triggers (they live on `memories`), so recreating the
-- table under the same name with the same column list keeps the sync triggers
-- valid. The 'rebuild' command repopulates the new index from `memories`
-- inside the same transaction the migration runner wraps around this file, so
-- existing DBs are re-indexed under porter in one atomic step (one-time
-- whole-table scan at open; fresh DBs rebuild an empty index for free).
DROP TABLE memories_fts;

CREATE VIRTUAL TABLE memories_fts USING fts5(
  content,
  summary,
  keywords,
  tags,
  content='memories',
  content_rowid='rowid',
  tokenize='porter unicode61'
);

INSERT INTO memories_fts(memories_fts) VALUES('rebuild');
