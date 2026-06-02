-- 002_link_base_strength.sql
-- Add an immutable baseline strength to memory_links so the link-decay job is
-- idempotent. Decay is computed as base_strength * 0.5^(age_from_creation/hl),
-- a pure function of the immutable (base_strength, created_at) pair. The running
-- `strength` column holds the current decayed display value; `base_strength` is
-- never touched by decay, so re-running a pass at the same instant is a no-op.
--
-- Backfill: existing rows treat their current `strength` as the baseline. This
-- is correct for never-decayed links (base == current) and the safe choice for
-- any already-written rows (decay simply resumes from the present value as its
-- baseline rather than compounding).

ALTER TABLE memory_links
  ADD COLUMN base_strength REAL NOT NULL DEFAULT 1.0
    CHECK (base_strength BETWEEN 0.0 AND 1.0);

UPDATE memory_links SET base_strength = strength;
