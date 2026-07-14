-- 011_memory_links_target_index.sql
-- The memory_links primary key starts with source_id, so target-side graph
-- probes otherwise scan the entire link table. Keep target_id first so every
-- inbound-edge lookup can seek; link_type then filters contradiction probes
-- inside the same index search.

CREATE INDEX idx_memory_links_target_type
  ON memory_links(target_id, link_type);
