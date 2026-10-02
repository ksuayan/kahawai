-- Several listeners on one server: each has their own place in a book,
-- bookmarks, history, finished flag and speed. There are no accounts (the
-- server is for a trusted home network): a listener is just a name a Player
-- sends, and the library and its details stay shared. Listener 0 is the
-- default, who everything so far belongs to and whom a Player that sends no
-- name is.

CREATE TABLE IF NOT EXISTS audiobook_listeners (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
INSERT OR IGNORE INTO audiobook_listeners (id, name) VALUES (0, 'Default');

-- positions: one per book and listener
CREATE TABLE audiobook_positions_new (
    book_id        INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    user_id        INTEGER NOT NULL DEFAULT 0 REFERENCES audiobook_listeners(id) ON DELETE CASCADE,
    book_offset_ms INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    PRIMARY KEY (book_id, user_id)
);
INSERT INTO audiobook_positions_new (book_id, user_id, book_offset_ms, updated_at)
    SELECT book_id, user_id, book_offset_ms, updated_at FROM audiobook_positions;
DROP TABLE audiobook_positions;
ALTER TABLE audiobook_positions_new RENAME TO audiobook_positions;

-- settings: one per book and listener
CREATE TABLE audiobook_settings_new (
    book_id        INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    user_id        INTEGER NOT NULL DEFAULT 0 REFERENCES audiobook_listeners(id) ON DELETE CASCADE,
    speed          REAL NOT NULL DEFAULT 1.0,
    skip_back_s    INTEGER NOT NULL DEFAULT 15,
    skip_forward_s INTEGER NOT NULL DEFAULT 30,
    PRIMARY KEY (book_id, user_id)
);
INSERT INTO audiobook_settings_new (book_id, user_id, speed, skip_back_s, skip_forward_s)
    SELECT book_id, 0, speed, skip_back_s, skip_forward_s FROM audiobook_settings;
DROP TABLE audiobook_settings;
ALTER TABLE audiobook_settings_new RENAME TO audiobook_settings;

-- finished: per listener (audiobooks.finished_at is no longer used)
CREATE TABLE audiobook_finished (
    book_id     INTEGER NOT NULL REFERENCES audiobooks(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL DEFAULT 0 REFERENCES audiobook_listeners(id) ON DELETE CASCADE,
    finished_at INTEGER NOT NULL,
    PRIMARY KEY (book_id, user_id)
);
INSERT INTO audiobook_finished (book_id, user_id, finished_at)
    SELECT id, 0, finished_at FROM audiobooks WHERE finished_at IS NOT NULL;

-- bookmarks and sessions belong to a listener
ALTER TABLE audiobook_bookmarks ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE audiobook_sessions ADD COLUMN user_id INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_audiobook_bookmarks_user ON audiobook_bookmarks(user_id, book_id);
CREATE INDEX IF NOT EXISTS idx_audiobook_sessions_user ON audiobook_sessions(user_id, book_id, started_at);
