-- Internet radio (docs/v2/kahawai-radio-spec.md): the listener's stations,
-- what they heard, and a day's cache of the online directory's answers.
--
-- Numbered 020, not 018: main's 018 is the audiobook Continue-shelf migration,
-- and a database that had run it skipped this one as "already applied" (no
-- radio tables: "internal error" in Radio). Written to run safely on a
-- database that already ran it as 018.
CREATE TABLE IF NOT EXISTS radio_favorites (
  id            INTEGER PRIMARY KEY,
  station_uuid  TEXT,                      -- NULL = added by hand
  name          TEXT NOT NULL,
  url           TEXT NOT NULL,
  url_resolved  TEXT,
  homepage      TEXT,
  favicon       TEXT,
  tags          TEXT,
  country       TEXT,
  language      TEXT,
  bitrate       INTEGER,
  codec         TEXT,
  manual        INTEGER NOT NULL DEFAULT 0,
  sort_order    INTEGER NOT NULL DEFAULT 0,
  added_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_radio_favorites_uuid ON radio_favorites(station_uuid) WHERE station_uuid IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_radio_favorites_url ON radio_favorites(url) WHERE station_uuid IS NULL;

CREATE TABLE IF NOT EXISTS radio_history (
  id            INTEGER PRIMARY KEY,
  station_name  TEXT NOT NULL,
  stream_title  TEXT NOT NULL,
  played_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_radio_history_played ON radio_history(played_at DESC);

CREATE TABLE IF NOT EXISTS radio_cache (
  key         TEXT PRIMARY KEY,
  body        TEXT NOT NULL,
  fetched_at  INTEGER NOT NULL
)
