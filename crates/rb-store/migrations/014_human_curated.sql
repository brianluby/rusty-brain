-- Explicit human curation is independent of provenance, trust, and importance.
-- Existing rows remain uncurated; only deliberate user intent sets this marker.
ALTER TABLE memories
    ADD COLUMN human_curated INTEGER NOT NULL DEFAULT 0
        CHECK (human_curated IN (0, 1));
