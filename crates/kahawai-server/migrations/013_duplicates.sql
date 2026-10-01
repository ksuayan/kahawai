-- Duplicate copies of the same track (the same file in two music folders,
-- a folder and its backup copy).
--
-- A present track whose content hash matches another present track of the
-- same album points at that track (the lowest id) in duplicate_of, and is
-- left out of albums, genres, search, counts and the players' catalog. It
-- stays in the table, so if the kept copy goes missing the next refresh
-- promotes it. Copies on different albums (a compilation and the original
-- album) are not duplicates. db::refresh_duplicates keeps this current.
--
-- duplicate_of joins the shown columns of the revision trigger, so players
-- hear about a track that becomes, or stops being, a duplicate.

ALTER TABLE tracks ADD COLUMN duplicate_of INTEGER;

DROP TRIGGER IF EXISTS tracks_rev_update;

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
  OR OLD.duplicate_of IS NOT NEW.duplicate_of
BEGIN
    UPDATE meta SET value = value + 1 WHERE key = 'catalog_rev';
    UPDATE tracks SET rev = (SELECT value FROM meta WHERE key = 'catalog_rev') WHERE id = NEW.id;
END;
