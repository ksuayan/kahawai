-- Metadata cleanup and embedded MusicBrainz IDs
-- (docs/v2/kahawai-metadata-enrichment-spec.md, local part of Phase C).
--
-- Grouping keys (case and whitespace collapsed) decide which album a track
-- joins, sort keys ("Beatles, The") order the lists, and display strings are
-- never altered. Keys for existing rows are filled in by the scanner before
-- its next scan (it needs Rust to compute them).
--
-- albums.mbid is the MusicBrainz release ID. enrich_status says how far
-- enrichment got: pending, matched, no_match or error. enrich_source says
-- where a match came from (embedded tags, or later a MusicBrainz lookup), and
-- artwork_source where the cover came from (embedded, or later caa).
--
-- tracks.mbid_checked = 0 marks rows whose tags were read before MusicBrainz
-- IDs were: the next scan re-reads just their tags. DSD files and SACD ISOs
-- go through readers that do not return these IDs, so they count as checked
-- already.

ALTER TABLE albums ADD COLUMN title_key TEXT;
ALTER TABLE albums ADD COLUMN artist_key TEXT;
ALTER TABLE albums ADD COLUMN sort_title TEXT;
ALTER TABLE albums ADD COLUMN sort_artist TEXT;
ALTER TABLE albums ADD COLUMN mbid TEXT;
ALTER TABLE albums ADD COLUMN enrich_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE albums ADD COLUMN enrich_source TEXT;
ALTER TABLE albums ADD COLUMN artwork_source TEXT;
CREATE INDEX IF NOT EXISTS idx_albums_title_key ON albums(title_key);

ALTER TABLE artists ADD COLUMN sort_name TEXT;

ALTER TABLE tracks ADD COLUMN recording_mbid TEXT;
ALTER TABLE tracks ADD COLUMN mbid_checked INTEGER NOT NULL DEFAULT 0;
UPDATE tracks SET mbid_checked = 1 WHERE format IN ('dsf', 'dff', 'sacd_iso');

UPDATE albums SET artwork_source = 'embedded' WHERE artwork_hash IS NOT NULL;

-- Years a release can't have (typos, 0 from an empty field) become unknown.
UPDATE tracks SET year = NULL
 WHERE year IS NOT NULL AND (year < 1900 OR year > CAST(strftime('%Y', 'now') AS INTEGER) + 1);
UPDATE albums SET year = NULL
 WHERE year IS NOT NULL AND (year < 1900 OR year > CAST(strftime('%Y', 'now') AS INTEGER) + 1);
