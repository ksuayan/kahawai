-- Catalog revisions for the player's cache
-- (docs/v1/kahawai-player-catalog-cache-spec.md).
--
-- meta.catalog_rev is a counter that only goes up. Every change to a track,
-- album or artist field a player shows takes the next value and stores it
-- in that row's rev, so "what changed since rev N" is rev > N. Deleted rows
-- leave a tombstone with the rev of their deletion. meta.catalog_id is
-- random per database: a player whose cache came from another database (a
-- new install, another server) sees a different id and pulls everything.
--
-- Triggers do the bookkeeping, so every write path is covered: the scan,
-- the tag backfill, album merges, key refreshes, online lookups, and any
-- added later. An update bumps rev only when a shown value actually changed
-- (a rescan that rewrites identical tags costs nothing). Content hashes,
-- file sizes and times, and internal keys are not shown, so they don't
-- count. Existing rows start at rev 1.

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value
);
INSERT OR IGNORE INTO meta (key, value) VALUES ('catalog_rev', 1);
INSERT OR IGNORE INTO meta (key, value) VALUES ('catalog_id', lower(hex(randomblob(16))));

ALTER TABLE tracks ADD COLUMN rev INTEGER NOT NULL DEFAULT 1;
ALTER TABLE albums ADD COLUMN rev INTEGER NOT NULL DEFAULT 1;
ALTER TABLE artists ADD COLUMN rev INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS idx_tracks_rev ON tracks(rev);
CREATE INDEX IF NOT EXISTS idx_albums_rev ON albums(rev);
CREATE INDEX IF NOT EXISTS idx_artists_rev ON artists(rev);

CREATE TABLE IF NOT EXISTS catalog_tombstones (
    kind TEXT NOT NULL,
    id   INTEGER NOT NULL,
    rev  INTEGER NOT NULL,
    PRIMARY KEY (kind, id)
);
CREATE INDEX IF NOT EXISTS idx_catalog_tombstones_rev ON catalog_tombstones(rev);

-- tracks ------------------------------------------------------------------

CREATE TRIGGER IF NOT EXISTS tracks_rev_insert AFTER INSERT ON tracks
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE tracks SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
    DELETE FROM catalog_tombstones WHERE kind = 'track' AND id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS tracks_rev_update AFTER UPDATE ON tracks
WHEN OLD.path IS NOT NEW.path OR OLD.format IS NOT NEW.format
  OR OLD.sample_rate IS NOT NEW.sample_rate OR OLD.bit_depth IS NOT NEW.bit_depth
  OR OLD.channels IS NOT NEW.channels OR OLD.duration_ms IS NOT NEW.duration_ms
  OR OLD.bitrate IS NOT NEW.bitrate OR OLD.title IS NOT NEW.title
  OR OLD.album IS NOT NEW.album OR OLD.artist IS NOT NEW.artist
  OR OLD.album_id IS NOT NEW.album_id OR OLD.track_no IS NOT NEW.track_no
  OR OLD.disc_no IS NOT NEW.disc_no OR OLD.genre IS NOT NEW.genre
  OR OLD.year IS NOT NEW.year OR OLD.missing IS NOT NEW.missing
  OR OLD.decodable IS NOT NEW.decodable OR OLD.mqa IS NOT NEW.mqa
  OR OLD.original_sample_rate IS NOT NEW.original_sample_rate
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE tracks SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS tracks_rev_delete AFTER DELETE ON tracks
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    INSERT OR REPLACE INTO catalog_tombstones (kind, id, rev)
    VALUES ('track', OLD.id, (SELECT value FROM meta WHERE key = 'catalog_rev'));
END;

-- albums ------------------------------------------------------------------

CREATE TRIGGER IF NOT EXISTS albums_rev_insert AFTER INSERT ON albums
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE albums SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
    DELETE FROM catalog_tombstones WHERE kind = 'album' AND id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS albums_rev_update AFTER UPDATE ON albums
WHEN OLD.title IS NOT NEW.title OR OLD.artist IS NOT NEW.artist
  OR OLD.year IS NOT NEW.year OR OLD.artwork_hash IS NOT NEW.artwork_hash
  OR OLD.sort_title IS NOT NEW.sort_title OR OLD.sort_artist IS NOT NEW.sort_artist
  OR OLD.mbid IS NOT NEW.mbid OR OLD.artwork_source IS NOT NEW.artwork_source
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE albums SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS albums_rev_delete AFTER DELETE ON albums
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    INSERT OR REPLACE INTO catalog_tombstones (kind, id, rev)
    VALUES ('album', OLD.id, (SELECT value FROM meta WHERE key = 'catalog_rev'));
END;

-- artists -----------------------------------------------------------------

CREATE TRIGGER IF NOT EXISTS artists_rev_insert AFTER INSERT ON artists
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE artists SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
    DELETE FROM catalog_tombstones WHERE kind = 'artist' AND id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS artists_rev_update AFTER UPDATE ON artists
WHEN OLD.name IS NOT NEW.name OR OLD.sort_name IS NOT NEW.sort_name
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE artists SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS artists_rev_delete AFTER DELETE ON artists
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    INSERT OR REPLACE INTO catalog_tombstones (kind, id, rev)
    VALUES ('artist', OLD.id, (SELECT value FROM meta WHERE key = 'catalog_rev'));
END;
