-- Music streaming server catalog. (Spec §3.2.)
-- Applied at startup by db::run_migrations which splits statements on
-- semicolons, so this file must not contain any semicolons inside comments.

CREATE TABLE IF NOT EXISTS tracks (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    hash        TEXT NOT NULL,
    format      TEXT NOT NULL,
    sample_rate INTEGER,
    bit_depth   INTEGER,
    channels    INTEGER,
    duration_ms INTEGER,
    bitrate     INTEGER,
    title       TEXT,
    album       TEXT,
    artist      TEXT,
    album_id    INTEGER REFERENCES albums(id),
    track_no    INTEGER,
    disc_no     INTEGER
);
CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album_id);
CREATE INDEX IF NOT EXISTS idx_tracks_hash ON tracks(hash);

CREATE TABLE IF NOT EXISTS albums (
    id           INTEGER PRIMARY KEY,
    title        TEXT NOT NULL,
    artist       TEXT,
    year         INTEGER,
    artwork_hash TEXT
);

CREATE TABLE IF NOT EXISTS artists (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS album_artists (
    album_id  INTEGER NOT NULL REFERENCES albums(id),
    artist_id INTEGER NOT NULL REFERENCES artists(id),
    PRIMARY KEY (album_id, artist_id)
);

CREATE TABLE IF NOT EXISTS track_artists (
    track_id  INTEGER NOT NULL REFERENCES tracks(id),
    artist_id INTEGER NOT NULL REFERENCES artists(id),
    role      TEXT NOT NULL DEFAULT 'performer',
    PRIMARY KEY (track_id, artist_id)
);

-- Artwork blobs, deduplicated by content hash (same scheme as Koa).
CREATE TABLE IF NOT EXISTS artwork (
    hash  TEXT PRIMARY KEY,
    mime  TEXT NOT NULL,
    bytes BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS playlists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    track_id    INTEGER NOT NULL REFERENCES tracks(id),
    PRIMARY KEY (playlist_id, position)
);

CREATE TABLE IF NOT EXISTS scan_log (
    id            INTEGER PRIMARY KEY,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    files_scanned INTEGER NOT NULL DEFAULT 0,
    files_added   INTEGER NOT NULL DEFAULT 0
);

-- Full-text search over title/album/artist. (Spec S2.)
-- TODO(S2): the search endpoint currently uses LIKE; switch it to this table.
CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5(title, album, artist);
