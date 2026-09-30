-- Index cleanup, measured on a real library (81k tracks, 5.8k albums).
--
-- idx_tracks_missing: almost every row has missing = 0, so it narrows
-- nothing, and without statistics the planner picked it over
-- idx_tracks_album for "this album's present tracks", scanning every track
-- per album (about 2 minutes to list 5.8k albums). Dropped.
--
-- idx_albums_title: albums are matched on title_key since migration 007.
-- Nothing looks albums up by title any more. Dropped.
--
-- idx_tracks_hash: nothing looks tracks up by hash. The hashing job only
-- needs the rows still waiting for one, which a partial index covers at a
-- fraction of the size (it shrinks to nothing once everything is hashed).
--
-- album_artists is keyed (album_id, artist_id), which can't serve "this
-- artist's albums". It gets an index on artist_id.
--
-- Statistics for the planner (ANALYZE, then PRAGMA optimize) are kept by
-- db::open and after every scan, not here.

DROP INDEX IF EXISTS idx_tracks_missing;
DROP INDEX IF EXISTS idx_albums_title;
DROP INDEX IF EXISTS idx_tracks_hash;
CREATE INDEX IF NOT EXISTS idx_tracks_hash_pending ON tracks(id) WHERE hash IS NULL;
CREATE INDEX IF NOT EXISTS idx_album_artists_artist ON album_artists(artist_id);
