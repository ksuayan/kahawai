-- Genre normalization (docs/v1/kahawai-genre-normalization-spec.md).
--
-- tracks.genre keeps the raw tag, untouched. genre_map says which canonical
-- genres each distinct raw value maps to (genre NULL when the value is not a
-- genre, like "Other"), and how: alias, keyword or unmapped. track_genres
-- joins tracks to canonical genres, one row per pair. Both are rebuilt from
-- tracks.genre by the server (genre::refresh_genres) at startup and after
-- every scan, so they are empty until then.
--
-- The search index gains a genre column, so a text search for "jazz" finds
-- jazz tracks. FTS5 tables cannot add columns, so it is rebuilt from tracks,
-- keeping track ids as rowids.

CREATE TABLE IF NOT EXISTS genre_map (
    raw   TEXT NOT NULL,
    genre TEXT,
    how   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_genre_map_raw ON genre_map(raw);

CREATE TABLE IF NOT EXISTS track_genres (
    track_id INTEGER NOT NULL,
    genre    TEXT NOT NULL,
    PRIMARY KEY (track_id, genre)
);
CREATE INDEX IF NOT EXISTS idx_track_genres_genre ON track_genres(genre);

DROP TABLE IF EXISTS search_fts;
CREATE VIRTUAL TABLE search_fts USING fts5(title, album, artist, genre);
INSERT INTO search_fts (rowid, title, album, artist, genre)
SELECT id, COALESCE(title, ''), COALESCE(album, ''), COALESCE(artist, ''), COALESCE(genre, '')
FROM tracks;
