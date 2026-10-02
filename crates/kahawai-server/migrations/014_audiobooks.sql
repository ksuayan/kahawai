-- Audiobooks (docs/v1/kahawai-audiobook-spec.md). Books live beside the music
-- catalog: their files are ordinary rows in tracks with kind = 'audiobook',
-- so streaming works unchanged, and the music paths leave them out.
-- A position is always book_offset_ms, the time from the start of the book.

ALTER TABLE tracks ADD COLUMN kind TEXT NOT NULL DEFAULT 'music';

CREATE TABLE IF NOT EXISTS audiobook_roots (
    id   INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audiobooks (
    id           INTEGER PRIMARY KEY,
    root_id      INTEGER NOT NULL REFERENCES audiobook_roots(id) ON DELETE CASCADE,
    path         TEXT NOT NULL UNIQUE,
    title        TEXT NOT NULL,
    author       TEXT,
    narrator     TEXT,
    series       TEXT,
    series_index REAL,
    year         INTEGER,
    cover_hash   TEXT,
    duration_ms  INTEGER NOT NULL DEFAULT 0,
    added_at     INTEGER NOT NULL,
    finished_at  INTEGER,
    meta_edited  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_audiobooks_root ON audiobooks(root_id);

CREATE TABLE IF NOT EXISTS audiobook_parts (
    id              INTEGER PRIMARY KEY,
    book_id         INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    track_id        INTEGER NOT NULL UNIQUE REFERENCES tracks(id),
    part_index      INTEGER NOT NULL,
    title           TEXT,
    start_offset_ms INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audiobook_parts_book ON audiobook_parts(book_id, part_index);

CREATE TABLE IF NOT EXISTS audiobook_chapters (
    id              INTEGER PRIMARY KEY,
    book_id         INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    part_id         INTEGER NOT NULL REFERENCES audiobook_parts(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    start_offset_ms INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audiobook_chapters_book ON audiobook_chapters(book_id, start_offset_ms);

CREATE TABLE IF NOT EXISTS audiobook_positions (
    book_id        INTEGER PRIMARY KEY REFERENCES audiobooks(id) ON DELETE CASCADE,
    book_offset_ms INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    user_id        INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS audiobook_bookmarks (
    id             INTEGER PRIMARY KEY,
    book_id        INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    book_offset_ms INTEGER NOT NULL,
    name           TEXT NOT NULL DEFAULT '',
    note           TEXT NOT NULL DEFAULT '',
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audiobook_bookmarks_book ON audiobook_bookmarks(book_id, book_offset_ms);

CREATE TABLE IF NOT EXISTS audiobook_sessions (
    id              INTEGER PRIMARY KEY,
    book_id         INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER NOT NULL,
    start_offset_ms INTEGER NOT NULL,
    end_offset_ms   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audiobook_sessions_book ON audiobook_sessions(book_id, started_at);

CREATE TABLE IF NOT EXISTS audiobook_settings (
    book_id        INTEGER PRIMARY KEY REFERENCES audiobooks(id) ON DELETE CASCADE,
    speed          REAL NOT NULL DEFAULT 1.0,
    skip_back_s    INTEGER NOT NULL DEFAULT 15,
    skip_forward_s INTEGER NOT NULL DEFAULT 30
);
