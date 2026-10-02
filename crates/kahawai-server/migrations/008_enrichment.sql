-- External metadata enrichment (docs/v1/kahawai-metadata-enrichment-spec.md,
-- MusicBrainz and Cover Art Archive part).
--
-- enrich_attempts counts lookups that failed with an error, so a flaky
-- network is retried a few times and then left alone. enriched_at is when
-- the last lookup finished (Unix seconds).
--
-- mb_cache keeps every MusicBrainz response by a hash of its request, so a
-- restart or a re-run never asks MusicBrainz the same thing twice.

ALTER TABLE albums ADD COLUMN enrich_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE albums ADD COLUMN enriched_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_albums_enrich_status ON albums(enrich_status);

CREATE TABLE IF NOT EXISTS mb_cache (
    query_hash    TEXT PRIMARY KEY,
    response_json TEXT NOT NULL,
    fetched_at    INTEGER NOT NULL
);
