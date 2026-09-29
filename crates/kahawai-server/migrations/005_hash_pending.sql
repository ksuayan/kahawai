-- Fast first scan (docs/v1/kahawai-fast-first-scan-spec.md, Phase A).
--
-- The scan no longer hashes file contents: tracks.hash becomes nullable,
-- NULL meaning "hash pending". tracks.hash_algo names the algorithm that
-- produced a hash, so a future change of identity algorithm never has to
-- guess what an existing row holds. Rows hashed by earlier scans are BLAKE3.
--
-- SQLite cannot drop NOT NULL in place, so the table is rebuilt. Ids are
-- copied unchanged, so track_artists, playlist_tracks and the FTS rowids
-- stay valid. db::run_migrations runs this with foreign keys off, on one
-- connection, as SQLite's table-rebuild procedure requires.
--
-- scan_log.walk_errors counts directory entries the walk could not read.

CREATE TABLE tracks_new (
    id                   INTEGER PRIMARY KEY,
    path                 TEXT NOT NULL UNIQUE,
    hash                 TEXT,
    hash_algo            TEXT,
    format               TEXT NOT NULL,
    sample_rate          INTEGER,
    bit_depth            INTEGER,
    channels             INTEGER,
    duration_ms          INTEGER,
    bitrate              INTEGER,
    title                TEXT,
    album                TEXT,
    artist               TEXT,
    album_id             INTEGER REFERENCES albums(id),
    track_no             INTEGER,
    disc_no              INTEGER,
    file_size            INTEGER,
    file_mtime           INTEGER,
    missing              INTEGER NOT NULL DEFAULT 0,
    decodable            INTEGER NOT NULL DEFAULT 1,
    genre                TEXT,
    year                 INTEGER,
    artwork_hash         TEXT,
    mqa                  INTEGER NOT NULL DEFAULT 0,
    original_sample_rate INTEGER,
    mqa_checked          INTEGER NOT NULL DEFAULT 0
);

INSERT INTO tracks_new (
    id, path, hash, hash_algo, format, sample_rate, bit_depth, channels,
    duration_ms, bitrate, title, album, artist, album_id, track_no, disc_no,
    file_size, file_mtime, missing, decodable, genre, year, artwork_hash,
    mqa, original_sample_rate, mqa_checked
)
SELECT
    id, path, hash, 'blake3-v1', format, sample_rate, bit_depth, channels,
    duration_ms, bitrate, title, album, artist, album_id, track_no, disc_no,
    file_size, file_mtime, missing, decodable, genre, year, artwork_hash,
    mqa, original_sample_rate, mqa_checked
FROM tracks;

DROP TABLE tracks;
ALTER TABLE tracks_new RENAME TO tracks;

CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album_id);
CREATE INDEX IF NOT EXISTS idx_tracks_hash ON tracks(hash);
CREATE INDEX IF NOT EXISTS idx_tracks_missing ON tracks(missing);

ALTER TABLE scan_log ADD COLUMN walk_errors INTEGER NOT NULL DEFAULT 0;
