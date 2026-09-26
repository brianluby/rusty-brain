-- 012_trust_class.sql (Vikunja #63, memory trust-class ladder)
--
-- Evidence-derived trust tier per memory: `confidence` is a caller-declared
-- prior ("how sure did the author feel"); `trust_class` records WHAT backs
-- the memory, derived server-side by the daemon write path from
-- channel-substantiatable evidence (rb_types::derive_trust_class) — the wire
-- protocol has no trust-class field, so no capture path can self-promote.
--
-- Backfill rule: legacy rows default to `agent_attested` — the highest class
-- a pre-ladder capture could honestly claim for itself (its own first-party
-- attestation). This matches the serde default on `MemoryNote.trust_class`,
-- so pre-ladder payloads and pre-ladder rows agree, and it never inflates an
-- old row into a measured/human class nothing substantiates.
--
-- Ranking (strongest first, rb_types::TrustClass declaration is its Ord):
--   measured_ci > measured_local > human_confirmed  (can-satisfy classes)
--   > agent_attested > inferred_activity            (context-only classes)
--
-- Purely additive ALTER: no FTS-indexed column changes (the 006-narrowed
-- triggers name their columns explicitly, so a new column cannot fire them)
-- and no backfill UPDATE — the column DEFAULT assigns `agent_attested` to
-- existing rows at ALTER time.
ALTER TABLE memories ADD COLUMN trust_class TEXT NOT NULL DEFAULT 'agent_attested'
  CHECK (trust_class IN ('measured_ci', 'measured_local', 'human_confirmed',
                         'agent_attested', 'inferred_activity'));
