-- Podcasts (docs/v2/kahawai-podcast-spec.md, D1 and D2): subscriptions and
-- their episodes. Positions, sessions and per-feed playback settings come
-- with the playback work.
CREATE TABLE podcast_feeds (
  id                        INTEGER PRIMARY KEY,
  feed_url                  TEXT NOT NULL UNIQUE,   -- normalized
  title                     TEXT NOT NULL,
  author                    TEXT,
  description               TEXT,
  link                      TEXT,
  image_url                 TEXT,
  language                  TEXT,
  explicit                  INTEGER NOT NULL DEFAULT 0,
  last_fetched              INTEGER,                -- unix ms of the last good read
  last_error                TEXT,                   -- why the last try failed, NULL when it worked
  etag                      TEXT,
  last_modified             TEXT,
  auto_download             INTEGER NOT NULL DEFAULT 1,
  keep_n                    INTEGER NOT NULL DEFAULT 5,
  delete_played_after_days  INTEGER NOT NULL DEFAULT 7,
  sort_order                INTEGER NOT NULL DEFAULT 0,
  added_at                  INTEGER NOT NULL
);

CREATE TABLE podcast_episodes (
  id               INTEGER PRIMARY KEY,
  feed_id          INTEGER NOT NULL REFERENCES podcast_feeds(id) ON DELETE CASCADE,
  guid             TEXT NOT NULL,
  title            TEXT NOT NULL,
  description_html TEXT,
  published_at     INTEGER,
  duration_ms      INTEGER,
  enclosure_url    TEXT NOT NULL,
  enclosure_type   TEXT,
  enclosure_bytes  INTEGER,
  image_url        TEXT,
  season           INTEGER,
  episode          INTEGER,
  link             TEXT,
  file_path        TEXT,                            -- the downloaded file, once there is one
  downloaded_at    INTEGER,
  played_at        INTEGER,
  dropped_from_feed INTEGER NOT NULL DEFAULT 0,     -- the feed no longer lists it
  created_at       INTEGER NOT NULL,
  UNIQUE (feed_id, guid)
);
CREATE INDEX idx_podcast_episodes_feed ON podcast_episodes(feed_id, published_at DESC);
