-- Historical classification only: never infer age from NULL provenance at read time.
-- New identity-less writes remain explicitly unverified (legacy_unknown = 0).
-- Internal jobs already have origin_source = 'job' and are not reclassified.
ALTER TABLE memories ADD COLUMN legacy_unknown INTEGER NOT NULL DEFAULT 0
  CHECK (legacy_unknown IN (0, 1));
UPDATE memories SET legacy_unknown = 1
WHERE origin_channel IS NULL AND origin_source IS NULL;
